//! Bounded 3D surface reconstruction from a complete point stream.
//! A reservoir chosen by a hash of the point ordinals covers the full source,
//! then spatial thinning spreads its points across the scan; a k-d tree
//! supports local tangent-plane triangulation
//! without assuming that the surface is a height field.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use rayon::prelude::*;

use super::{visit_points, LoadError, Point, PointCloud};
use crate::local_fit::{cross, difference, dot, symmetric_eigen3, unit};
use crate::mesher::{MeshProgress, MeshStage, MeshStats};
use crate::{IndexConfig, OctreeIndex};

#[derive(Clone, Copy, Debug)]
pub struct SurfaceMeshConfig {
    pub max_vertices: usize,
    pub neighbors: usize,
    pub max_edge_factor: f64,
    /// Percentage of eligible source points considered before the bounded reservoir.
    pub sample_percent: f64,
    /// Minimum voxel width in source units; zero lets the vertex budget choose it.
    pub mesh_size: f64,
}

impl Default for SurfaceMeshConfig {
    fn default() -> Self {
        Self {
            max_vertices: 50_000,
            neighbors: 12,
            max_edge_factor: 4.0,
            sample_percent: 100.0,
            mesh_size: 0.0,
        }
    }
}

impl SurfaceMeshConfig {
    pub fn validate(self) -> Result<(), LoadError> {
        if !(3..=1_000_000).contains(&self.max_vertices) {
            return Err(LoadError::InvalidData(
                "3D surface vertices must be 3..=1,000,000".into(),
            ));
        }
        if !(3..=32).contains(&self.neighbors) {
            return Err(LoadError::InvalidData(
                "3D surface neighbors must be 3..=32".into(),
            ));
        }
        if !self.max_edge_factor.is_finite() || self.max_edge_factor <= 0.0 {
            return Err(LoadError::InvalidData(
                "3D surface edge factor must be finite and positive".into(),
            ));
        }
        if !self.sample_percent.is_finite()
            || !(0.0..=100.0).contains(&self.sample_percent)
            || self.sample_percent == 0.0
        {
            return Err(LoadError::InvalidData(
                "3D surface sample percent must be >0 and <=100".into(),
            ));
        }
        if !self.mesh_size.is_finite() || self.mesh_size < 0.0 {
            return Err(LoadError::InvalidData(
                "3D surface mesh size must be nonnegative and finite".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct KdNode {
    vertex: usize,
    axis: usize,
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct Near {
    index: usize,
    distance_sq: f64,
}

impl PartialEq for Near {
    fn eq(&self, other: &Self) -> bool {
        self.distance_sq == other.distance_sq && self.index == other.index
    }
}
impl Eq for Near {}
impl PartialOrd for Near {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Near {
    fn cmp(&self, other: &Self) -> Ordering {
        self.distance_sq
            .total_cmp(&other.distance_sq)
            .then(self.index.cmp(&other.index))
    }
}

fn build_tree(
    indices: &mut [usize],
    depth: usize,
    vertices: &[[f64; 3]],
    nodes: &mut Vec<KdNode>,
) -> Option<usize> {
    if indices.is_empty() {
        return None;
    }
    let axis = depth % 3;
    let middle = indices.len() / 2;
    indices.select_nth_unstable_by(middle, |a, b| {
        vertices[*a][axis]
            .total_cmp(&vertices[*b][axis])
            .then(a.cmp(b))
    });
    let (left, remaining) = indices.split_at_mut(middle);
    let (pivot, right) = remaining.split_first_mut().expect("nonempty pivot");
    let node = nodes.len();
    nodes.push(KdNode {
        vertex: *pivot,
        axis,
        left: None,
        right: None,
    });
    let left = build_tree(left, depth + 1, vertices, nodes);
    let right = build_tree(right, depth + 1, vertices, nodes);
    nodes[node].left = left;
    nodes[node].right = right;
    Some(node)
}

fn nearest(
    node: Option<usize>,
    target: usize,
    limit: usize,
    vertices: &[[f64; 3]],
    nodes: &[KdNode],
    found: &mut BinaryHeap<Near>,
) {
    let Some(node) = node else { return };
    let entry = nodes[node];
    let origin = vertices[target];
    let point = vertices[entry.vertex];
    let delta = origin[entry.axis] - point[entry.axis];
    let (first, second) = if delta <= 0.0 {
        (entry.left, entry.right)
    } else {
        (entry.right, entry.left)
    };
    nearest(first, target, limit, vertices, nodes, found);
    if entry.vertex != target {
        let distance_sq = (0..3)
            .map(|axis| (origin[axis] - point[axis]).powi(2))
            .sum();
        if found.len() < limit {
            found.push(Near {
                index: entry.vertex,
                distance_sq,
            });
        } else if found
            .peek()
            .is_some_and(|worst| distance_sq < worst.distance_sq)
        {
            found.pop();
            found.push(Near {
                index: entry.vertex,
                distance_sq,
            });
        }
    }
    if found.len() < limit
        || found
            .peek()
            .is_some_and(|worst| delta * delta < worst.distance_sq)
    {
        nearest(second, target, limit, vertices, nodes, found);
    }
}

/// Smallest-eigenvalue eigenvector of a symmetric 3x3 covariance matrix.
fn surface_normal(neighbors: &[Near], vertices: &[[f64; 3]]) -> [f64; 3] {
    if neighbors.len() < 3 {
        return [0.0, 0.0, 1.0];
    }
    let mut mean = [0.0; 3];
    for near in neighbors {
        for axis in 0..3 {
            mean[axis] += vertices[near.index][axis];
        }
    }
    for value in &mut mean {
        *value /= neighbors.len() as f64;
    }
    let mut matrix = [[0.0; 3]; 3];
    for near in neighbors {
        let delta = difference(vertices[near.index], mean);
        for a in 0..3 {
            for b in a..3 {
                matrix[a][b] += delta[a] * delta[b];
            }
        }
    }
    let (_, eigenvectors) = symmetric_eigen3(matrix);
    let mut normal = unit(eigenvectors[0]).unwrap_or([0.0, 0.0, 1.0]);
    let dominant = (0..3)
        .max_by(|a, b| normal[*a].abs().total_cmp(&normal[*b].abs()))
        .unwrap_or(2);
    if normal[dominant] < 0.0 {
        for value in &mut normal {
            *value = -*value;
        }
    }
    normal
}

fn voxel_key(point: [f64; 3], origin: [f64; 3], width: f64) -> [i64; 3] {
    std::array::from_fn(|axis| ((point[axis] - origin[axis]) / width).floor() as i64)
}

/// Keep one point near each occupied voxel's center. A larger reservoir is
/// needed here: thinning only the final vertex budget cannot repair regions
/// that a density-weighted sample already missed.
fn spatially_thin_indices(candidates: &[[f64; 3]], budget: usize) -> Vec<usize> {
    if candidates.len() <= budget {
        return (0..candidates.len()).collect();
    }
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for point in candidates {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(point[axis]);
            maximum[axis] = maximum[axis].max(point[axis]);
        }
    }
    let extent = (0..3)
        .map(|axis| maximum[axis] - minimum[axis])
        .fold(0.0, f64::max);
    if extent <= f64::EPSILON {
        return (0..budget).collect();
    }
    let mut low = extent / (candidates.len() as f64 * 2.0);
    let mut high = extent * 2.0;
    for _ in 0..24 {
        let width = (low * high).sqrt();
        let mut occupied = HashSet::with_capacity(budget + 1);
        for point in candidates {
            occupied.insert(voxel_key(*point, minimum, width));
            if occupied.len() > budget {
                break;
            }
        }
        if occupied.len() > budget {
            low = width;
        } else {
            high = width;
        }
    }
    let mut representatives = HashMap::<[i64; 3], (usize, f64)>::with_capacity(budget);
    for (index, point) in candidates.iter().enumerate() {
        let key = voxel_key(*point, minimum, high);
        let distance_sq = (0..3)
            .map(|axis| {
                let center = minimum[axis] + (key[axis] as f64 + 0.5) * high;
                (point[axis] - center).powi(2)
            })
            .sum();
        match representatives.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                if distance_sq < entry.get().1 {
                    *entry.get_mut() = (index, distance_sq);
                }
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert((index, distance_sq));
            }
        }
    }
    let mut indices: Vec<_> = representatives.values().map(|(index, _)| *index).collect();
    indices.sort_unstable();
    if indices.len() < budget {
        // Coarse voxel sizes can jump over the exact target count. Spend the
        // remaining budget on points farthest from their voxel representative.
        let mut selected = vec![false; candidates.len()];
        for &index in &indices {
            selected[index] = true;
        }
        let mut extras = Vec::new();
        for (index, point) in candidates.iter().enumerate() {
            if selected[index] {
                continue;
            }
            let representative = representatives[&voxel_key(*point, minimum, high)].0;
            let distance_sq = dot(
                difference(*point, candidates[representative]),
                difference(*point, candidates[representative]),
            );
            if distance_sq > 1e-20 {
                extras.push((index, distance_sq));
            }
        }
        extras.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        indices.extend(
            extras
                .into_iter()
                .take(budget - indices.len())
                .map(|(index, _)| index),
        );
        indices.sort_unstable();
    }
    indices.into_iter().take(budget).collect()
}

fn voxel_thin_indices(candidates: &[[f64; 3]], width: f64) -> Vec<usize> {
    if width <= 0.0 {
        return (0..candidates.len()).collect();
    }
    let origin = std::array::from_fn(|axis| {
        candidates
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min)
    });
    let mut representatives = HashMap::<[i64; 3], (usize, f64)>::new();
    for (index, point) in candidates.iter().enumerate() {
        let key = voxel_key(*point, origin, width);
        let distance_sq = (0..3)
            .map(|axis| {
                let center = origin[axis] + (key[axis] as f64 + 0.5) * width;
                (point[axis] - center).powi(2)
            })
            .sum();
        match representatives.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry)
                if distance_sq < entry.get().1 =>
            {
                *entry.get_mut() = (index, distance_sq)
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert((index, distance_sq));
            }
            _ => {}
        }
    }
    let mut indices: Vec<_> = representatives.values().map(|entry| entry.0).collect();
    indices.sort_unstable();
    indices
}

/// A well-mixed hash of the ordinal of a source point: the finaliser of
/// splitmix64 with a fixed seed. It is a bijection, so two ordinals never
/// share a hash.
fn ordinal_hash(ordinal: u64) -> u64 {
    let mut hash = ordinal.wrapping_add(0x9e37_79b9_7f4a_7c15);
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^ (hash >> 31)
}

pub(crate) fn sampled_ordinal(ordinal: u64, percent: f64) -> bool {
    if percent >= 100.0 {
        return true;
    }
    (ordinal_hash(ordinal) as f64 / u64::MAX as f64) < percent / 100.0
}

/// The points a 3D surface is made from: of the points offered, the `limit`
/// whose ordinals have the smallest hashes. The choice depends on the
/// ordinals alone and not on the order in which the points arrive, so the
/// source file and an index of it, which gives its points leaf by leaf, give
/// the same candidates.
struct Candidates {
    limit: usize,
    points: Vec<(u64, Point)>,
    /// The hash of every candidate with its place in `points`, the largest
    /// on top: the one a point with a smaller hash takes the place of.
    largest: BinaryHeap<(u64, usize)>,
}

impl Candidates {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            points: Vec::with_capacity(limit),
            largest: BinaryHeap::with_capacity(limit),
        }
    }

    fn offer(&mut self, ordinal: u64, point: Point) {
        let hash = ordinal_hash(ordinal);
        if self.points.len() < self.limit {
            self.largest.push((hash, self.points.len()));
            self.points.push((ordinal, point));
        } else if let Some(mut top) = self.largest.peek_mut() {
            if hash < top.0 {
                self.points[top.1] = (ordinal, point);
                top.0 = hash;
            }
        }
    }

    fn len(&self) -> usize {
        self.points.len()
    }

    /// The candidates in the order of their ordinals, so that what comes
    /// after does not depend on the order they were offered in either.
    fn into_points(self) -> Vec<Point> {
        let mut points = self.points;
        points.sort_unstable_by_key(|(ordinal, _)| *ordinal);
        points.into_iter().map(|(_, point)| point).collect()
    }
}

/// Where a 3D surface reads the points of its cloud. Each gives the same
/// mesh: an index only saves decoding the source file once more.
#[derive(Clone, Copy)]
pub enum SurfacePoints<'a> {
    /// The source file.
    Source,
    /// This index of the cloud.
    Index(&'a OctreeIndex),
    /// The index a cache with these settings holds for the cloud when it
    /// holds a valid one, and the source file otherwise. A provisional cloud
    /// is always read from its file and leaves the cache as it is.
    Cached(&'a IndexConfig),
}

/// Reconstruct a general 3D surface and atomically write an OBJ. The point
/// reservoir is sampled across the complete source, not from the GUI preview.
pub fn mesh_surface_obj(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    config: SurfaceMeshConfig,
) -> Result<MeshStats, LoadError> {
    mesh_surface_obj_where(cloud, destination, config, |_, _| true)
}

/// Reconstruct a 3D surface from a filtered source stream.
pub fn mesh_surface_obj_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    config: SurfaceMeshConfig,
    include: impl FnMut(u64, &Point) -> bool,
) -> Result<MeshStats, LoadError> {
    mesh_surface_obj_where_progress(cloud, destination, config, include, |_| Ok(()))
}

/// Full-stream surface reconstruction with progress and cancellation support.
/// The points are read from the index in the default cache when it holds
/// one for the cloud.
pub fn mesh_surface_obj_where_progress(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    config: SurfaceMeshConfig,
    include: impl FnMut(u64, &Point) -> bool,
    progress: impl FnMut(MeshProgress) -> Result<(), LoadError>,
) -> Result<MeshStats, LoadError> {
    mesh_surface_obj_from(
        cloud,
        SurfacePoints::Cached(&IndexConfig::default()),
        destination,
        config,
        include,
        progress,
    )
}

/// Full-stream surface reconstruction that reads the points from where
/// `points` says.
pub fn mesh_surface_obj_from(
    cloud: &PointCloud,
    points: SurfacePoints<'_>,
    destination: impl AsRef<Path>,
    config: SurfaceMeshConfig,
    mut include: impl FnMut(u64, &Point) -> bool,
    mut progress: impl FnMut(MeshProgress) -> Result<(), LoadError>,
) -> Result<MeshStats, LoadError> {
    mesh_surface_obj_inner(
        cloud,
        points,
        destination.as_ref(),
        config,
        &mut include,
        &mut progress,
    )
}

// Keep the point-heavy reconstruction in pointcloud-core's optimized dev
// profile. A generic body is monomorphized in the unoptimized desktop crate
// when called from its CLI or GUI and makes a large LAZ mesh many times slower.
fn mesh_surface_obj_inner(
    cloud: &PointCloud,
    points: SurfacePoints<'_>,
    destination: &Path,
    config: SurfaceMeshConfig,
    include: &mut dyn FnMut(u64, &Point) -> bool,
    progress: &mut dyn FnMut(MeshProgress) -> Result<(), LoadError>,
) -> Result<MeshStats, LoadError> {
    if destination == cloud.path
        || fs::canonicalize(destination).ok() == fs::canonicalize(&cloud.path).ok()
    {
        return Err(LoadError::InvalidData(
            "source and destination must differ".into(),
        ));
    }
    config.validate()?;
    cloud.validate_source()?;
    // Keep a bounded surplus so occupied regions have candidates even when
    // source density varies by orders of magnitude across a scan.
    let candidate_limit = config
        .max_vertices
        .saturating_mul(4)
        .min(config.max_vertices.saturating_add(150_000));
    let mut candidates = Candidates::new(candidate_limit);
    let mut visited = 0u64;
    let mut source_points = 0u64;
    progress(MeshProgress::new(MeshStage::Reading, 0, cloud.total_points))?;
    let mut take = |ordinal: u64, point: Point| {
        visited += 1;
        if visited.is_multiple_of(65_536) {
            progress(MeshProgress::new(
                MeshStage::Reading,
                visited,
                cloud.total_points,
            ))?;
        }
        if !include(ordinal, &point) {
            return Ok(());
        }
        if !sampled_ordinal(ordinal, config.sample_percent) {
            return Ok(());
        }
        if !point.xyz.iter().all(|value| value.is_finite()) {
            return Err(LoadError::InvalidData("non-finite mesh vertex".into()));
        }
        source_points += 1;
        candidates.offer(ordinal, point);
        Ok(())
    };
    // A validated on-disk octree stores every point with its original file
    // ordinal. Reuse it when present: reconstructing a large E57 repeatedly
    // need not decode the compressed source for every parameter change.
    // An absent cache or one that fails validation falls back to the source.
    let cached;
    let index = match points {
        SurfacePoints::Source => None,
        SurfacePoints::Index(index) => Some(index),
        // Opening a cache writes the bounds of the cloud into it, and those
        // of a provisional cloud are loose: such a cloud is read from its
        // file.
        SurfacePoints::Cached(_) if cloud.provisional => None,
        SurfacePoints::Cached(config) => {
            cached = OctreeIndex::open_cached_if_present(cloud, config.clone())
                .ok()
                .flatten();
            cached.as_ref()
        }
    };
    if let Some(index) = index {
        index.visit_intersecting(|_| true, |record| take(record.ordinal, record.point))?;
    } else {
        let mut ordinal = 0;
        visit_points(&cloud.path, &mut |point| {
            let current = ordinal;
            ordinal += 1;
            take(current, point)
        })?;
    }
    if visited != cloud.total_points {
        return Err(LoadError::InvalidData(format!(
            "source changed during meshing (expected {}, found {visited})",
            cloud.total_points
        )));
    }
    cloud.validate_source()?;
    progress(MeshProgress::new(
        MeshStage::Reading,
        visited,
        cloud.total_points,
    ))?;
    if candidates.len() < 3 {
        return Err(LoadError::InvalidData(
            "too few points for a 3D surface".into(),
        ));
    }
    progress(MeshProgress::new(MeshStage::Reconstructing, 0, 0))?;
    let candidates = candidates.into_points();
    let candidate_xyz: Vec<_> = candidates.iter().map(|point| point.xyz).collect();
    let spaced = voxel_thin_indices(&candidate_xyz, config.mesh_size);
    let spaced_xyz: Vec<_> = spaced.iter().map(|&index| candidate_xyz[index]).collect();
    let vertex_points: Vec<_> = spatially_thin_indices(&spaced_xyz, config.max_vertices)
        .into_iter()
        .map(|index| candidates[spaced[index]])
        .collect();
    let vertices: Vec<_> = vertex_points.iter().map(|point| point.xyz).collect();
    let reconstruct_total = vertices.len() as u64 * 3;
    let mut indices: Vec<usize> = (0..vertices.len()).collect();
    let mut nodes = Vec::with_capacity(vertices.len());
    let root = build_tree(&mut indices, 0, &vertices, &mut nodes);
    let mut neighborhoods = Vec::with_capacity(vertices.len());
    let mut nearest_distances = Vec::with_capacity(vertices.len());
    // Each query reads the same immutable tree. Keep batches bounded so the
    // UI can report progress and cancel between parallel search waves.
    for start in (0..vertices.len()).step_by(4_096) {
        progress(MeshProgress::new(
            MeshStage::Reconstructing,
            start as u64,
            reconstruct_total,
        ))?;
        let end = (start + 4_096).min(vertices.len());
        let batch: Vec<_> = (start..end)
            .into_par_iter()
            .map(|index| {
                let mut heap = BinaryHeap::with_capacity(config.neighbors + 1);
                nearest(root, index, config.neighbors, &vertices, &nodes, &mut heap);
                let mut nearby = heap.into_sorted_vec();
                nearby.retain(|near| near.distance_sq > 1e-20);
                let first_distance = nearby.first().map(|near| near.distance_sq.sqrt());
                (nearby, first_distance)
            })
            .collect();
        for (nearby, first_distance) in batch {
            if let Some(distance) = first_distance {
                nearest_distances.push(distance);
            }
            neighborhoods.push(nearby);
        }
    }
    if nearest_distances.is_empty() {
        return Err(LoadError::InvalidData("all 3D points coincide".into()));
    }
    let middle = nearest_distances.len() / 2;
    nearest_distances.select_nth_unstable_by(middle, f64::total_cmp);
    let max_edge_sq = (nearest_distances[middle] * config.max_edge_factor).powi(2);
    let mut normals = Vec::with_capacity(vertices.len());
    for start in (0..vertices.len()).step_by(4_096) {
        progress(MeshProgress::new(
            MeshStage::Reconstructing,
            vertices.len() as u64 + start as u64,
            reconstruct_total,
        ))?;
        let end = (start + 4_096).min(vertices.len());
        normals.extend(
            neighborhoods[start..end]
                .par_iter()
                .map(|neighbors| surface_normal(neighbors, &vertices))
                .collect::<Vec<_>>(),
        );
    }
    let mut faces = Vec::<[u32; 3]>::new();
    let mut known = HashSet::<[u32; 3]>::new();
    let mut edge_uses = HashMap::<(u32, u32), u8>::new();
    for (center, neighbors) in neighborhoods.iter().enumerate() {
        if center.is_multiple_of(1_024) {
            progress(MeshProgress::new(
                MeshStage::Reconstructing,
                vertices.len() as u64 * 2 + center as u64,
                reconstruct_total,
            ))?;
        }
        let normal = normals[center];
        let reference = if normal[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let Some(u) = unit(cross(normal, reference)) else {
            continue;
        };
        let v = cross(normal, u);
        let mut ring: Vec<_> = neighbors
            .iter()
            .filter(|near| near.distance_sq <= max_edge_sq)
            .map(|near| {
                let delta = difference(vertices[near.index], vertices[center]);
                (near.index, dot(delta, v).atan2(dot(delta, u)))
            })
            .collect();
        if ring.len() < 2 {
            continue;
        }
        ring.sort_by(|a, b| a.1.total_cmp(&b.1));
        for pair in 0..ring.len() {
            let next = (pair + 1) % ring.len();
            let (b, angle) = ring[pair];
            let (c, next_angle) = ring[next];
            let gap = (next_angle - angle).rem_euclid(std::f64::consts::TAU);
            if gap > std::f64::consts::FRAC_PI_2 || b == c {
                continue;
            }
            let bc = difference(vertices[b], vertices[c]);
            if dot(bc, bc) > max_edge_sq {
                continue;
            }
            let ab = difference(vertices[b], vertices[center]);
            let ac = difference(vertices[c], vertices[center]);
            let face_normal = cross(ab, ac);
            if dot(face_normal, face_normal) <= 1e-20 {
                continue;
            }
            let face = if dot(face_normal, normal) >= 0.0 {
                [center as u32, b as u32, c as u32]
            } else {
                [center as u32, c as u32, b as u32]
            };
            let mut key = face;
            key.sort_unstable();
            if known.contains(&key) {
                continue;
            }
            let edges = [
                edge_key(face[0], face[1]),
                edge_key(face[1], face[2]),
                edge_key(face[2], face[0]),
            ];
            if edges
                .iter()
                .any(|edge| edge_uses.get(edge).is_some_and(|count| *count >= 2))
            {
                continue;
            }
            known.insert(key);
            for edge in edges {
                *edge_uses.entry(edge).or_default() += 1;
            }
            faces.push(face);
        }
    }
    if faces.is_empty() {
        return Err(LoadError::InvalidData(
            "3D surface reconstruction produced no triangles".into(),
        ));
    }
    let mut face_normals = orient_surface_faces(&vertices, &normals, &mut faces);
    if fill_small_planar_holes(
        &vertices,
        &mut faces,
        &mut edge_uses,
        &mut known,
        max_edge_sq,
    ) > 0
    {
        face_normals = vertex_normals_from_faces(&vertices, &normals, &faces);
    }
    progress(MeshProgress::new(
        MeshStage::Reconstructing,
        reconstruct_total,
        reconstruct_total,
    ))?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let write_total = (vertices.len() * 2 + faces.len()) as u64;
    progress(MeshProgress::new(MeshStage::Writing, 0, write_total))?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writeln!(writer, "# Open Pointcloud Studio 3D surface mesh")?;
        writeln!(writer, "o Surface")?;
        let has_color = vertex_points.iter().any(|point| point.rgb.is_some());
        for (index, vertex) in vertices.iter().enumerate() {
            if has_color {
                let rgb = vertex_points[index].rgb.unwrap_or([255; 3]);
                writeln!(
                    writer,
                    "v {:.9} {:.9} {:.9} {:.6} {:.6} {:.6}",
                    vertex[0],
                    vertex[1],
                    vertex[2],
                    f64::from(rgb[0]) / 255.0,
                    f64::from(rgb[1]) / 255.0,
                    f64::from(rgb[2]) / 255.0,
                )?;
            } else {
                writeln!(
                    writer,
                    "v {:.9} {:.9} {:.9}",
                    vertex[0], vertex[1], vertex[2]
                )?;
            }
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (index + 1) as u64,
                    write_total,
                ))?;
            }
        }
        for (index, normal) in face_normals.iter().enumerate() {
            writeln!(
                writer,
                "vn {:.8} {:.8} {:.8}",
                normal[0], normal[1], normal[2]
            )?;
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (vertices.len() + index + 1) as u64,
                    write_total,
                ))?;
            }
        }
        for (index, [a, b, c]) in faces.iter().enumerate() {
            writeln!(
                writer,
                "f {}//{} {}//{} {}//{}",
                a + 1,
                a + 1,
                b + 1,
                b + 1,
                c + 1,
                c + 1
            )?;
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (vertices.len() * 2 + index + 1) as u64,
                    write_total,
                ))?;
            }
        }
        writer.flush()?;
    }
    progress(MeshProgress::new(
        MeshStage::Writing,
        write_total,
        write_total,
    ))?;
    cloud.validate_source()?;
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(MeshStats {
        source_points,
        vertices: vertices.len(),
        triangles: faces.len(),
    })
}

fn edge_key(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

#[derive(Clone, Copy)]
struct BoundaryEdge {
    start: u32,
    end: u32,
    face: usize,
}

/// Fill only short, convex inner loops. Outer borders, non-manifold vertices,
/// warped loops and long gaps remain open rather than inventing a surface.
fn fill_small_planar_holes(
    vertices: &[[f64; 3]],
    faces: &mut Vec<[u32; 3]>,
    edge_uses: &mut HashMap<(u32, u32), u8>,
    known: &mut HashSet<[u32; 3]>,
    max_edge_sq: f64,
) -> usize {
    const MAX_HOLE_EDGES: usize = 8;
    let mut boundary = Vec::<BoundaryEdge>::new();
    let mut outgoing = HashMap::<u32, usize>::new();
    let mut incoming = HashMap::<u32, u8>::new();
    let mut ambiguous = HashSet::<u32>::new();
    for (face_index, &[a, b, c]) in faces.iter().enumerate() {
        for (start, end) in [(a, b), (b, c), (c, a)] {
            if edge_uses.get(&edge_key(start, end)) != Some(&1) {
                continue;
            }
            let index = boundary.len();
            boundary.push(BoundaryEdge {
                start,
                end,
                face: face_index,
            });
            if outgoing.insert(start, index).is_some() {
                ambiguous.insert(start);
            }
            let count = incoming.entry(end).or_default();
            *count = count.saturating_add(1);
        }
    }
    let mut visited = vec![false; boundary.len()];
    let mut added = 0;
    for seed in 0..boundary.len() {
        if visited[seed] {
            continue;
        }
        let start = boundary[seed].start;
        if ambiguous.contains(&start) || incoming.get(&start) != Some(&1) {
            visited[seed] = true;
            continue;
        }
        let mut loop_edges = Vec::new();
        let mut current = seed;
        let mut closed = false;
        while !visited[current] {
            visited[current] = true;
            loop_edges.push(boundary[current]);
            let end = boundary[current].end;
            if end == boundary[seed].start {
                closed = true;
                break;
            }
            if ambiguous.contains(&end) || incoming.get(&end) != Some(&1) {
                break;
            }
            let Some(&next) = outgoing.get(&end) else {
                break;
            };
            current = next;
        }
        if !closed || !(3..=MAX_HOLE_EDGES).contains(&loop_edges.len()) {
            continue;
        }
        let loop_vertices: Vec<u32> = loop_edges.iter().map(|edge| edge.start).collect();
        if loop_vertices.iter().copied().collect::<HashSet<_>>().len() != loop_vertices.len() {
            continue;
        }
        let mut normal_sum = [0.0; 3];
        for edge in &loop_edges {
            let [a, b, c] = faces[edge.face];
            let normal = cross(
                difference(vertices[b as usize], vertices[a as usize]),
                difference(vertices[c as usize], vertices[a as usize]),
            );
            for axis in 0..3 {
                normal_sum[axis] += normal[axis];
            }
        }
        let Some(normal) = unit(normal_sum) else {
            continue;
        };
        let origin = vertices[loop_vertices[0] as usize];
        let mut area_vector = [0.0; 3];
        let mut valid = true;
        for index in 0..loop_vertices.len() {
            let previous = vertices
                [loop_vertices[(index + loop_vertices.len() - 1) % loop_vertices.len()] as usize];
            let current = vertices[loop_vertices[index] as usize];
            let next = vertices[loop_vertices[(index + 1) % loop_vertices.len()] as usize];
            if dot(difference(current, origin), normal).abs() > max_edge_sq.sqrt() * 0.15
                || dot(
                    cross(difference(current, previous), difference(next, current)),
                    normal,
                ) >= -max_edge_sq * 1e-10
            {
                valid = false;
                break;
            }
            let cross_segment = cross(difference(current, origin), difference(next, origin));
            for axis in 0..3 {
                area_vector[axis] += cross_segment[axis];
            }
            for &other in &loop_vertices {
                let delta = difference(current, vertices[other as usize]);
                if dot(delta, delta) > max_edge_sq {
                    valid = false;
                    break;
                }
            }
            if !valid {
                break;
            }
        }
        let area = dot(area_vector, normal) * 0.5;
        if !valid || area >= -max_edge_sq * 1e-10 || area.abs() > max_edge_sq * 2.0 {
            continue;
        }
        let mut patch = Vec::with_capacity(loop_vertices.len() - 2);
        let mut new_edge_uses = HashMap::<(u32, u32), u8>::new();
        for index in 1..loop_vertices.len() - 1 {
            let face = [
                loop_vertices[0],
                loop_vertices[index + 1],
                loop_vertices[index],
            ];
            let mut key = face;
            key.sort_unstable();
            if known.contains(&key) {
                valid = false;
                break;
            }
            for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
                *new_edge_uses.entry(edge_key(a, b)).or_default() += 1;
            }
            patch.push((face, key));
        }
        if !valid
            || new_edge_uses
                .iter()
                .any(|(edge, count)| edge_uses.get(edge).copied().unwrap_or_default() + *count > 2)
        {
            continue;
        }
        added += patch.len();
        for (face, key) in patch {
            known.insert(key);
            for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
                *edge_uses.entry(edge_key(a, b)).or_default() += 1;
            }
            faces.push(face);
        }
    }
    added += fill_triangular_gaps(vertices, faces, edge_uses, known, &boundary, max_edge_sq);
    added
}

/// A three-edge gap can share vertices with other boundary chains. Cap it only
/// when every neighboring face agrees with the new face's orientation.
fn fill_triangular_gaps(
    vertices: &[[f64; 3]],
    faces: &mut Vec<[u32; 3]>,
    edge_uses: &mut HashMap<(u32, u32), u8>,
    known: &mut HashSet<[u32; 3]>,
    boundary: &[BoundaryEdge],
    max_edge_sq: f64,
) -> usize {
    const MAX_BOUNDARY_BRANCH: usize = 32;
    let mut outgoing = HashMap::<u32, Vec<usize>>::new();
    let mut directed = HashMap::<(u32, u32), usize>::new();
    for (index, edge) in boundary.iter().enumerate() {
        outgoing.entry(edge.start).or_default().push(index);
        directed.insert((edge.start, edge.end), index);
    }
    let mut added = 0;
    for &first in boundary {
        let Some(seconds) = outgoing.get(&first.end) else {
            continue;
        };
        if seconds.len() > MAX_BOUNDARY_BRANCH {
            continue;
        }
        for &second_index in seconds {
            let second = boundary[second_index];
            if second.end == first.start {
                continue;
            }
            if outgoing
                .get(&second.end)
                .is_some_and(|edges| edges.len() > MAX_BOUNDARY_BRANCH)
            {
                continue;
            }
            if let Some(&third_index) = directed.get(&(second.end, first.start)) {
                let third = boundary[third_index];
                let patch = [first.start, second.end, first.end];
                let mut key = patch;
                key.sort_unstable();
                if known.contains(&key) {
                    continue;
                }
                let edges = [first, second, third];
                if edges
                    .iter()
                    .any(|edge| edge_uses.get(&edge_key(edge.start, edge.end)) != Some(&1))
                {
                    continue;
                }
                let [a, b, c] = patch;
                let ab = difference(vertices[b as usize], vertices[a as usize]);
                let ac = difference(vertices[c as usize], vertices[a as usize]);
                if [edge_key(a, b), edge_key(b, c), edge_key(c, a)]
                    .iter()
                    .any(|edge| {
                        let delta =
                            difference(vertices[edge.0 as usize], vertices[edge.1 as usize]);
                        dot(delta, delta) > max_edge_sq
                    })
                {
                    continue;
                }
                let Some(patch_normal) = unit(cross(ab, ac)) else {
                    continue;
                };
                if edges.iter().any(|edge| {
                    let [a, b, c] = faces[edge.face];
                    let adjacent = cross(
                        difference(vertices[b as usize], vertices[a as usize]),
                        difference(vertices[c as usize], vertices[a as usize]),
                    );
                    unit(adjacent).is_none_or(|normal| dot(normal, patch_normal) < 0.5)
                }) {
                    continue;
                }
                known.insert(key);
                for edge in edges {
                    *edge_uses.entry(edge_key(edge.start, edge.end)).or_default() += 1;
                }
                faces.push(patch);
                added += 1;
            }
        }
    }
    added
}

/// Propagate edge orientation across each connected patch and then derive
/// normals from the resulting faces. Contradictory cycles from independently
/// triangulated neighborhoods remain unresolved rather than removing faces.
fn orient_surface_faces(
    vertices: &[[f64; 3]],
    estimated_normals: &[[f64; 3]],
    faces: &mut [[u32; 3]],
) -> Vec<[f64; 3]> {
    let mut first_edge = HashMap::<(u32, u32), (usize, bool)>::with_capacity(faces.len() * 2);
    let mut adjacent = vec![Vec::<(usize, bool)>::new(); faces.len()];
    for (face_index, &[a, b, c]) in faces.iter().enumerate() {
        for (start, end) in [(a, b), (b, c), (c, a)] {
            let key = edge_key(start, end);
            let forward = start == key.0;
            if let Some(&(other, other_forward)) = first_edge.get(&key) {
                let opposite_flip = forward == other_forward;
                adjacent[face_index].push((other, opposite_flip));
                adjacent[other].push((face_index, opposite_flip));
            } else {
                first_edge.insert(key, (face_index, forward));
            }
        }
    }
    let mut flip = vec![None; faces.len()];
    let mut queue = VecDeque::new();
    for seed in 0..faces.len() {
        if flip[seed].is_some() {
            continue;
        }
        flip[seed] = Some(false);
        queue.push_back(seed);
        let mut component = Vec::new();
        while let Some(face_index) = queue.pop_front() {
            component.push(face_index);
            let current = flip[face_index].expect("queued face has an orientation");
            for &(neighbor, opposite_flip) in &adjacent[face_index] {
                if flip[neighbor].is_none() {
                    flip[neighbor] = Some(current ^ opposite_flip);
                    queue.push_back(neighbor);
                }
            }
        }
        let alignment: f64 = component
            .iter()
            .map(|&index| {
                let [a, b, c] = faces[index];
                let normal = cross(
                    difference(vertices[b as usize], vertices[a as usize]),
                    difference(vertices[c as usize], vertices[a as usize]),
                );
                let sign = if flip[index] == Some(true) { -1.0 } else { 1.0 };
                sign * dot(normal, estimated_normals[a as usize])
            })
            .sum();
        if alignment < 0.0 {
            for &index in &component {
                flip[index] = Some(!flip[index].expect("component face has an orientation"));
            }
        }
    }
    for (face, should_flip) in faces.iter_mut().zip(flip) {
        if should_flip == Some(true) {
            face.swap(1, 2);
        }
    }
    vertex_normals_from_faces(vertices, estimated_normals, faces)
}

fn vertex_normals_from_faces(
    vertices: &[[f64; 3]],
    estimated_normals: &[[f64; 3]],
    faces: &[[u32; 3]],
) -> Vec<[f64; 3]> {
    let mut normals = vec![[0.0_f64; 3]; vertices.len()];
    for &[a, b, c] in faces.iter() {
        let normal = cross(
            difference(vertices[b as usize], vertices[a as usize]),
            difference(vertices[c as usize], vertices[a as usize]),
        );
        for index in [a, b, c] {
            for axis in 0..3 {
                normals[index as usize][axis] += normal[axis];
            }
        }
    }
    normals
        .into_iter()
        .enumerate()
        .map(|(index, normal)| unit(normal).unwrap_or(estimated_normals[index]))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open, read_obj_mesh};

    fn face_edges(faces: &[[u32; 3]]) -> HashMap<(u32, u32), u8> {
        let mut uses = HashMap::new();
        for &[a, b, c] in faces {
            for (start, end) in [(a, b), (b, c), (c, a)] {
                *uses.entry(edge_key(start, end)).or_default() += 1;
            }
        }
        uses
    }

    #[test]
    fn repairs_small_planar_inner_hole_without_capping_outer_border() {
        let vertices: Vec<[f64; 3]> = (0..5)
            .flat_map(|y| (0..5).map(move |x| [f64::from(x), f64::from(y), 0.0]))
            .collect();
        let mut faces = Vec::new();
        for y in 0..4 {
            for x in 0..4 {
                if (x, y) == (1, 1) {
                    continue;
                }
                let a = y * 5 + x;
                let b = a + 1;
                let c = a + 5;
                let d = c + 1;
                faces.push([a, b, d]);
                faces.push([a, d, c]);
            }
        }
        let mut edge_uses = face_edges(&faces);
        assert_eq!(edge_uses.values().filter(|&&count| count == 1).count(), 20);
        let mut known: HashSet<[u32; 3]> = faces
            .iter()
            .map(|face| {
                let mut key = *face;
                key.sort_unstable();
                key
            })
            .collect();
        assert_eq!(
            fill_small_planar_holes(&vertices, &mut faces, &mut edge_uses, &mut known, 4.0),
            2
        );
        assert_eq!(
            face_edges(&faces)
                .values()
                .filter(|&&count| count == 1)
                .count(),
            16
        );
        assert_eq!(faces.len(), 32);
    }

    #[test]
    fn leaves_a_small_open_patch_uncapped() {
        let vertices = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        let mut faces = vec![[0, 1, 2], [0, 2, 3]];
        let mut edge_uses = face_edges(&faces);
        let mut known = HashSet::new();
        assert_eq!(
            fill_small_planar_holes(&vertices, &mut faces, &mut edge_uses, &mut known, 4.0),
            0
        );
        assert_eq!(faces.len(), 2);
    }

    #[test]
    fn repairs_triangular_gap_at_branching_boundary_vertices() {
        let vertices = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.5, 0.8, 0.0],
            [0.5, -1.0, 0.0],
            [1.5, 0.8, 0.0],
            [-0.5, 0.8, 0.0],
        ];
        let mut faces = vec![[1, 0, 3], [2, 1, 4], [0, 2, 5]];
        let mut edge_uses = face_edges(&faces);
        let mut known = HashSet::new();
        assert_eq!(
            fill_small_planar_holes(&vertices, &mut faces, &mut edge_uses, &mut known, 4.0),
            1
        );
        assert_eq!(faces.len(), 4);
        assert!(faces.contains(&[1, 2, 0]));
        assert_eq!(edge_uses.values().filter(|&&count| count == 1).count(), 6);
    }

    #[test]
    fn leaves_sharply_folded_triangular_gap_open() {
        let vertices = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.5, 0.8, 0.0],
            [0.5, -1.0, 0.0],
            [1.5, 0.8, 0.0],
            [-0.5, 0.8, 3.0],
        ];
        let mut faces = vec![[1, 0, 3], [2, 1, 4], [0, 2, 5]];
        let mut edge_uses = face_edges(&faces);
        let mut known = HashSet::new();
        assert_eq!(
            fill_small_planar_holes(&vertices, &mut faces, &mut edge_uses, &mut known, 4.0),
            0
        );
        assert_eq!(faces.len(), 3);
    }

    #[test]
    fn orients_adjacent_surface_faces_and_recomputes_normals() {
        let vertices = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
        ];
        let mut faces = vec![[0, 1, 2], [1, 2, 3]];
        let estimated = vec![[0.0, 0.0, 1.0]; 4];
        let normals = orient_surface_faces(&vertices, &estimated, &mut faces);
        assert_eq!(faces, vec![[0, 1, 2], [1, 3, 2]]);
        assert_eq!(normals, estimated);
    }

    #[test]
    fn surface_mesh_preserves_rgb_and_estimated_normals() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("colored-wall.xyz");
        let destination = directory.path().join("colored-wall.obj");
        let mut data = String::new();
        for y in 0..6 {
            for z in 0..6 {
                data.push_str(&format!("0 {y} {z} {} {} 128\n", y * 40, z * 40));
            }
        }
        fs::write(&source, data).unwrap();
        let cloud = open(&source, 1).unwrap();
        mesh_surface_obj(&cloud, &destination, SurfaceMeshConfig::default()).unwrap();
        let mesh = read_obj_mesh(destination).unwrap();
        assert!(!mesh.triangles.is_empty());
        assert_eq!(mesh.colors.as_ref().unwrap().len(), mesh.vertices.len());
        assert_eq!(mesh.normals.as_ref().unwrap().len(), mesh.vertices.len());
        assert!(mesh
            .normals
            .unwrap()
            .iter()
            .all(|normal| normal[0].abs() > 0.99));
    }

    #[test]
    fn cancellation_during_surface_write_preserves_existing_mesh() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("wall.xyz");
        let target = dir.path().join("wall.obj");
        let mut data = String::new();
        for y in 0..8 {
            for z in 0..8 {
                data.push_str(&format!("0 {y} {z}\n"));
            }
        }
        fs::write(&source, data).unwrap();
        fs::write(&target, "previous surface").unwrap();
        let cloud = open(&source, 1).unwrap();
        let result = mesh_surface_obj_where_progress(
            &cloud,
            &target,
            SurfaceMeshConfig::default(),
            |_, _| true,
            |state| {
                if state.stage == MeshStage::Writing {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(fs::read(&target).unwrap(), b"previous surface");
    }

    #[test]
    fn cancellation_between_parallel_neighborhood_batches_preserves_existing_mesh() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("large-grid.xyz");
        let target = dir.path().join("large-grid.obj");
        let mut data = String::new();
        for y in 0..100 {
            for x in 0..100 {
                data.push_str(&format!("{x} {y} 0\n"));
            }
        }
        fs::write(&source, data).unwrap();
        fs::write(&target, "previous surface").unwrap();
        let cloud = open(&source, 1).unwrap();
        let mut reached_second_batch = false;
        let result = mesh_surface_obj_where_progress(
            &cloud,
            &target,
            SurfaceMeshConfig {
                max_vertices: 8_000,
                ..SurfaceMeshConfig::default()
            },
            |_, _| true,
            |state| {
                if state.stage == MeshStage::Reconstructing && state.completed >= 4_096 {
                    reached_second_batch = true;
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(reached_second_batch);
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(fs::read(&target).unwrap(), b"previous surface");
    }

    #[test]
    fn spatial_thinning_preserves_sparsely_sampled_regions() {
        let mut candidates = Vec::new();
        for index in 0..120 {
            candidates.push([index as f64 / 120.0, 0.0, 0.0]);
        }
        for x in 0..8 {
            for y in 0..5 {
                candidates.push([10.0 + x as f64, y as f64, 0.0]);
            }
        }
        let selected: Vec<_> = spatially_thin_indices(&candidates, 40)
            .into_iter()
            .map(|index| candidates[index])
            .collect();
        assert_eq!(selected.len(), 40);
        assert!(selected.iter().filter(|point| point[0] >= 10.0).count() >= 30);
    }

    #[test]
    fn reconstructs_vertical_wall_and_keeps_distant_patches_separate() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("walls.xyz");
        let target = dir.path().join("walls.obj");
        let mut data = String::new();
        for offset in [0.0, 100.0] {
            for y in 0..6 {
                for z in 0..6 {
                    data.push_str(&format!("{offset} {y} {z}\n"));
                }
            }
        }
        fs::write(&source, data).unwrap();
        let cloud = open(&source, 1).unwrap();
        let stats = mesh_surface_obj(&cloud, &target, SurfaceMeshConfig::default()).unwrap();
        assert_eq!(stats.source_points, 72);
        assert!(stats.triangles > 0);
        let mesh = read_obj_mesh(&target).unwrap();
        for face in mesh.triangles {
            let xs = face.map(|i| mesh.vertices[i as usize][0]);
            assert_eq!(xs[0], xs[1]);
            assert_eq!(xs[1], xs[2]);
        }
    }

    #[test]
    fn samples_entire_source_with_bounded_vertices() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("wall.xyz");
        let target = dir.path().join("wall.obj");
        let mut data = String::new();
        for y in 0..20 {
            for z in 0..20 {
                data.push_str(&format!("0 {y} {z}\n"));
            }
        }
        fs::write(&source, data).unwrap();
        let cloud = open(&source, 1).unwrap();
        assert_eq!(cloud.points.len(), 1);
        let stats = mesh_surface_obj(
            &cloud,
            &target,
            SurfaceMeshConfig {
                max_vertices: 40,
                ..SurfaceMeshConfig::default()
            },
        )
        .unwrap();
        assert_eq!(stats.source_points, 400);
        assert_eq!(stats.vertices, 40);
        assert!(stats.triangles > 0);
        let edited = dir.path().join("wall-edited.obj");
        let stats = mesh_surface_obj_where(
            &cloud,
            &edited,
            SurfaceMeshConfig {
                max_vertices: 40,
                ..SurfaceMeshConfig::default()
            },
            |ordinal, _| ordinal < 200,
        )
        .unwrap();
        assert_eq!(stats.source_points, 200);
        let edited = read_obj_mesh(edited).unwrap();
        assert!(edited.vertices.iter().all(|vertex| vertex[1] < 10.0));
    }

    #[test]
    fn reconstructs_stacked_surfaces_without_bridging_them() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("slabs.xyz");
        let target = dir.path().join("slabs.obj");
        let mut data = String::new();
        for z in [0, 10] {
            for x in 0..6 {
                for y in 0..6 {
                    data.push_str(&format!("{x} {y} {z}\n"));
                }
            }
        }
        fs::write(&source, data).unwrap();
        let cloud = open(&source, 1).unwrap();
        let stats = mesh_surface_obj(&cloud, &target, SurfaceMeshConfig::default()).unwrap();
        assert!(stats.triangles > 0);
        let mesh = read_obj_mesh(&target).unwrap();
        for face in mesh.triangles {
            let zs = face.map(|i| mesh.vertices[i as usize][2]);
            assert_eq!(zs[0], zs[1]);
            assert_eq!(zs[1], zs[2]);
        }
    }

    /// A floor and two walls of 3,321 points in all, written as XYZ.
    fn corner_of_a_room(path: &Path) {
        let mut data = String::new();
        for x in 0..41 {
            for y in 0..41 {
                data.push_str(&format!(
                    "{} {} 0\n",
                    f64::from(x) * 0.1,
                    f64::from(y) * 0.1
                ));
            }
        }
        for along in 0..41 {
            for z in 1..21 {
                let (along, z) = (f64::from(along) * 0.1, f64::from(z) * 0.1);
                data.push_str(&format!("{along} 0 {z}\n0 {along} {z}\n"));
            }
        }
        fs::write(path, data).unwrap();
    }

    #[test]
    fn the_index_and_the_source_give_the_same_surface() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("corner.xyz");
        corner_of_a_room(&source);
        let cloud = open(&source, 1).unwrap();
        assert_eq!(cloud.total_points, 3_321);
        let cache = IndexConfig {
            leaf_points: 32,
            scratch_dir: Some(dir.path().join("cache")),
            ..IndexConfig::default()
        };
        let index = OctreeIndex::build_cached(&cloud, cache.clone()).unwrap();
        // The index gives its points leaf by leaf, not in the order of the
        // file.
        let mut order = Vec::new();
        index
            .visit_intersecting(
                |_| true,
                |record| {
                    order.push(record.ordinal);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(order.len(), 3_321);
        assert!(!order.is_sorted());

        // At most 400 candidates for 100 vertices: fewer than the points.
        let config = SurfaceMeshConfig {
            max_vertices: 100,
            ..SurfaceMeshConfig::default()
        };
        let surface = |points: SurfacePoints<'_>, name: &str, include: fn(u64) -> bool| {
            let target = dir.path().join(name);
            mesh_surface_obj_from(
                &cloud,
                points,
                &target,
                config,
                |ordinal, _| include(ordinal),
                |_| Ok(()),
            )
            .unwrap();
            fs::read(target).unwrap()
        };
        // Every point, and the points an edit left: two in three.
        let filters: [fn(u64) -> bool; 2] = [|_| true, |ordinal| ordinal % 3 != 0];
        for include in filters {
            let from_source = surface(SurfacePoints::Source, "source.obj", include);
            assert_eq!(
                surface(SurfacePoints::Index(&index), "index.obj", include),
                from_source
            );
            assert_eq!(
                surface(SurfacePoints::Cached(&cache), "cached.obj", include),
                from_source
            );
        }
    }

    #[test]
    fn a_provisional_cloud_leaves_the_index_cache_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("corner.xyz");
        corner_of_a_room(&source);
        let cloud = open(&source, 1).unwrap();
        let cache = IndexConfig {
            leaf_points: 256,
            scratch_dir: Some(dir.path().join("cache")),
            ..IndexConfig::default()
        };
        OctreeIndex::build_cached(&cloud, cache.clone()).unwrap();
        let header = fs::read_dir(dir.path().join("cache"))
            .unwrap()
            .map(|entry| entry.unwrap().path().join("cloud.json"))
            .find(|path| path.exists())
            .unwrap();
        let written = fs::read(&header).unwrap();
        // Shown before the full pass over its file: the bounds are loose.
        let mut loading = cloud.clone();
        loading.provisional = true;
        loading.bounds.max = [100.0; 3];
        let surface = |cloud: &PointCloud, name: &str| {
            let target = dir.path().join(name);
            mesh_surface_obj_from(
                cloud,
                SurfacePoints::Cached(&cache),
                &target,
                SurfaceMeshConfig::default(),
                |_, _| true,
                |_| Ok(()),
            )
            .unwrap();
            fs::read(target).unwrap()
        };
        let from_file = surface(&loading, "loading.obj");
        assert_eq!(fs::read(&header).unwrap(), written);
        assert_eq!(surface(&cloud, "checked.obj"), from_file);
        assert_eq!(fs::read(&header).unwrap(), written);
    }

    #[test]
    fn percentage_sampling_covers_the_entire_source_deterministically() {
        let sample: Vec<_> = (0..10_000)
            .filter(|&ordinal| sampled_ordinal(ordinal, 10.0))
            .collect();
        assert!((850..=1_150).contains(&sample.len()));
        assert!(sample.iter().any(|&ordinal| ordinal > 9_000));
        assert_eq!(
            sample,
            (0..10_000)
                .filter(|&ordinal| sampled_ordinal(ordinal, 10.0))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn mesh_size_keeps_one_representative_per_voxel() {
        let points = [
            [0.1, 0.1, 0.0],
            [0.2, 0.2, 0.0],
            [1.2, 0.1, 0.0],
            [0.1, 1.2, 0.0],
        ];
        let selected = voxel_thin_indices(&points, 1.0);
        assert_eq!(selected.len(), 3);
        assert!(selected.contains(&2));
        assert!(selected.contains(&3));
    }
}
