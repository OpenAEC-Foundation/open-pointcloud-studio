//! Bounded 3D surface reconstruction from a complete point stream.
//! A reservoir covers the full source, then spatial thinning spreads its points
//! across the scan; a k-d tree supports local tangent-plane triangulation
//! without assuming that the surface is a height field.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use super::{visit_points, LoadError, Point, PointCloud};
use crate::mesher::{MeshProgress, MeshStage, MeshStats};

#[derive(Clone, Copy, Debug)]
pub struct SurfaceMeshConfig {
    pub max_vertices: usize,
    pub neighbors: usize,
    pub max_edge_factor: f64,
}

impl Default for SurfaceMeshConfig {
    fn default() -> Self {
        Self {
            max_vertices: 50_000,
            neighbors: 12,
            max_edge_factor: 4.0,
        }
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

fn difference(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let length = dot(v, v).sqrt();
    (length > 1e-12).then(|| [v[0] / length, v[1] / length, v[2] / length])
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
    matrix[1][0] = matrix[0][1];
    matrix[2][0] = matrix[0][2];
    matrix[2][1] = matrix[1][2];
    let mut eigenvectors = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..20 {
        let (mut p, mut q) = (0, 1);
        for (a, b) in [(0, 2), (1, 2)] {
            if matrix[a][b].abs() > matrix[p][q].abs() {
                (p, q) = (a, b);
            }
        }
        if matrix[p][q].abs() < 1e-12 {
            break;
        }
        let angle = 0.5 * (2.0 * matrix[p][q]).atan2(matrix[q][q] - matrix[p][p]);
        let (s, c) = angle.sin_cos();
        let app = matrix[p][p];
        let aqq = matrix[q][q];
        let apq = matrix[p][q];
        matrix[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
        matrix[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
        matrix[p][q] = 0.0;
        matrix[q][p] = 0.0;
        for r in 0..3 {
            if r != p && r != q {
                let arp = matrix[r][p];
                let arq = matrix[r][q];
                matrix[r][p] = c * arp - s * arq;
                matrix[p][r] = matrix[r][p];
                matrix[r][q] = s * arp + c * arq;
                matrix[q][r] = matrix[r][q];
            }
            let vrp = eigenvectors[r][p];
            let vrq = eigenvectors[r][q];
            eigenvectors[r][p] = c * vrp - s * vrq;
            eigenvectors[r][q] = s * vrp + c * vrq;
        }
    }
    let smallest = (0..3)
        .min_by(|a, b| matrix[*a][*a].total_cmp(&matrix[*b][*b]))
        .unwrap_or(2);
    let mut normal = unit([
        eigenvectors[0][smallest],
        eigenvectors[1][smallest],
        eigenvectors[2][smallest],
    ])
    .unwrap_or([0.0, 0.0, 1.0]);
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
fn spatially_thin(candidates: Vec<[f64; 3]>, budget: usize) -> Vec<[f64; 3]> {
    if candidates.len() <= budget {
        return candidates;
    }
    let mut minimum = [f64::INFINITY; 3];
    let mut maximum = [f64::NEG_INFINITY; 3];
    for point in &candidates {
        for axis in 0..3 {
            minimum[axis] = minimum[axis].min(point[axis]);
            maximum[axis] = maximum[axis].max(point[axis]);
        }
    }
    let extent = (0..3)
        .map(|axis| maximum[axis] - minimum[axis])
        .fold(0.0, f64::max);
    if extent <= f64::EPSILON {
        return candidates.into_iter().take(budget).collect();
    }
    let mut low = extent / (candidates.len() as f64 * 2.0);
    let mut high = extent * 2.0;
    for _ in 0..24 {
        let width = (low * high).sqrt();
        let mut occupied = HashSet::with_capacity(budget + 1);
        for point in &candidates {
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
    indices
        .into_iter()
        .take(budget)
        .map(|index| candidates[index])
        .collect()
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
pub fn mesh_surface_obj_where_progress(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    config: SurfaceMeshConfig,
    mut include: impl FnMut(u64, &Point) -> bool,
    mut progress: impl FnMut(MeshProgress) -> Result<(), LoadError>,
) -> Result<MeshStats, LoadError> {
    let destination = destination.as_ref();
    if destination == cloud.path
        || fs::canonicalize(destination).ok() == fs::canonicalize(&cloud.path).ok()
    {
        return Err(LoadError::InvalidData(
            "source and destination must differ".into(),
        ));
    }
    if !(3..=1_000_000).contains(&config.max_vertices)
        || !(3..=32).contains(&config.neighbors)
        || !config.max_edge_factor.is_finite()
        || config.max_edge_factor <= 0.0
    {
        return Err(LoadError::InvalidData("invalid 3D mesh settings".into()));
    }
    cloud.validate_source()?;
    // Keep a bounded surplus so occupied regions have candidates even when
    // source density varies by orders of magnitude across a scan.
    let candidate_limit = config
        .max_vertices
        .saturating_mul(4)
        .min(config.max_vertices.saturating_add(150_000));
    let mut candidates = Vec::<[f64; 3]>::with_capacity(candidate_limit);
    let mut visited = 0u64;
    let mut source_points = 0u64;
    let mut random = 0x7a81_09e6_63d1_c207u64;
    progress(MeshProgress::new(MeshStage::Reading, 0, cloud.total_points))?;
    visit_points(&cloud.path, &mut |point| {
        let ordinal = visited;
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
        if !point.xyz.iter().all(|value| value.is_finite()) {
            return Err(LoadError::InvalidData("non-finite mesh vertex".into()));
        }
        source_points += 1;
        if candidates.len() < candidate_limit {
            candidates.push(point.xyz);
        } else {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            let slot = random % source_points;
            if slot < candidate_limit as u64 {
                candidates[slot as usize] = point.xyz;
            }
        }
        Ok(())
    })?;
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
    let vertices = spatially_thin(candidates, config.max_vertices);
    let reconstruct_total = vertices.len() as u64 * 3;
    let mut indices: Vec<usize> = (0..vertices.len()).collect();
    let mut nodes = Vec::with_capacity(vertices.len());
    let root = build_tree(&mut indices, 0, &vertices, &mut nodes);
    let mut neighborhoods = Vec::with_capacity(vertices.len());
    let mut nearest_distances = Vec::with_capacity(vertices.len());
    for index in 0..vertices.len() {
        if index.is_multiple_of(1_024) {
            progress(MeshProgress::new(
                MeshStage::Reconstructing,
                index as u64,
                reconstruct_total,
            ))?;
        }
        let mut heap = BinaryHeap::with_capacity(config.neighbors + 1);
        nearest(root, index, config.neighbors, &vertices, &nodes, &mut heap);
        let mut nearby = heap.into_sorted_vec();
        nearby.retain(|near| near.distance_sq > 1e-20);
        if let Some(first) = nearby.first() {
            nearest_distances.push(first.distance_sq.sqrt());
        }
        neighborhoods.push(nearby);
    }
    if nearest_distances.is_empty() {
        return Err(LoadError::InvalidData("all 3D points coincide".into()));
    }
    let middle = nearest_distances.len() / 2;
    nearest_distances.select_nth_unstable_by(middle, f64::total_cmp);
    let max_edge_sq = (nearest_distances[middle] * config.max_edge_factor).powi(2);
    let mut normals = Vec::with_capacity(vertices.len());
    for (index, neighbors) in neighborhoods.iter().enumerate() {
        if index.is_multiple_of(1_024) {
            progress(MeshProgress::new(
                MeshStage::Reconstructing,
                vertices.len() as u64 + index as u64,
                reconstruct_total,
            ))?;
        }
        normals.push(surface_normal(neighbors, &vertices));
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
    let write_total = (vertices.len() + faces.len()) as u64;
    progress(MeshProgress::new(MeshStage::Writing, 0, write_total))?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writeln!(writer, "# Open Pointcloud Studio 3D surface mesh")?;
        writeln!(writer, "o Surface")?;
        for (index, vertex) in vertices.iter().enumerate() {
            writeln!(
                writer,
                "v {:.9} {:.9} {:.9}",
                vertex[0], vertex[1], vertex[2]
            )?;
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (index + 1) as u64,
                    write_total,
                ))?;
            }
        }
        for (index, [a, b, c]) in faces.iter().enumerate() {
            writeln!(writer, "f {} {} {}", a + 1, b + 1, c + 1)?;
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (vertices.len() + index + 1) as u64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{open, read_obj_mesh};

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
        let selected = spatially_thin(candidates, 40);
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
}
