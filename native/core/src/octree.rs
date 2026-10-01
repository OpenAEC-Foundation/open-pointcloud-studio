//! Disk-backed octree indexing. Only node metadata and one small preview at a
//! time need to be held in memory; point records stay in temporary files.

use std::array;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::{visit_points, Bounds, LoadError, Point, PointCloud, SourceStamp};

const RECORD_BYTES: usize = 40;

#[derive(Debug, Clone, Copy)]
pub struct IndexedPoint {
    pub point: Point,
    pub ordinal: u64,
}

#[derive(Debug, Clone)]
pub struct IndexConfig {
    pub leaf_points: u64,
    pub preview_points: usize,
    pub max_depth: u8,
    pub scratch_dir: Option<PathBuf>,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            leaf_points: 65_536,
            preview_points: 2_048,
            max_depth: 12,
            scratch_dir: None,
        }
    }
}

#[derive(Debug)]
pub struct IndexedNode {
    pub id: String,
    pub bounds: Bounds,
    pub total_points: u64,
    pub stored_points: u64,
    pub depth: u8,
    pub children: Vec<IndexedNode>,
    data_path: PathBuf,
}

impl IndexedNode {
    pub fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    pub fn find(&self, id: &str) -> Option<&Self> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }
}

#[derive(Debug)]
pub struct OctreeIndex {
    pub root: IndexedNode,
    storage: IndexStorage,
}

#[derive(Debug)]
enum IndexStorage {
    Temporary(tempfile::TempDir),
    Persistent(PathBuf),
}

impl IndexStorage {
    fn path(&self) -> &Path {
        match self {
            Self::Temporary(dir) => dir.path(),
            Self::Persistent(path) => path,
        }
    }
}

impl OctreeIndex {
    pub fn build(cloud: &PointCloud, config: IndexConfig) -> Result<Self, LoadError> {
        if config.leaf_points == 0 || config.preview_points == 0 || config.max_depth == 0 {
            return Err(LoadError::InvalidData(
                "octree limits must be positive".into(),
            ));
        }
        let expected_stamp = cloud
            .source_stamp
            .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
        if SourceStamp::read(&cloud.path)? != expected_stamp {
            return Err(LoadError::InvalidData(
                "source changed since loading".into(),
            ));
        }

        let mut builder = tempfile::Builder::new();
        builder.prefix("open-pointcloud-index-");
        let storage = if let Some(dir) = &config.scratch_dir {
            builder.tempdir_in(dir)?
        } else {
            builder.tempdir()?
        };
        let root_path = storage.path().join("r.bin");
        let mut root_count = 0u64;
        {
            let mut writer = BufWriter::new(File::create(&root_path)?);
            visit_points(&cloud.path, &mut |point| {
                if !point.xyz.iter().all(|value| value.is_finite()) {
                    return Err(LoadError::InvalidData("non-finite coordinate".into()));
                }
                write_record(
                    &mut writer,
                    IndexedPoint {
                        point,
                        ordinal: root_count,
                    },
                )?;
                root_count += 1;
                Ok(())
            })?;
            writer.flush()?;
        }
        if root_count != cloud.total_points || SourceStamp::read(&cloud.path)? != expected_stamp {
            return Err(LoadError::InvalidData(
                "source changed while indexing".into(),
            ));
        }
        let root = build_node(
            storage.path(),
            "r".to_owned(),
            root_path,
            cloud.bounds,
            root_count,
            0,
            &config,
        )?;
        Ok(Self {
            root,
            storage: IndexStorage::Temporary(storage),
        })
    }

    /// Reuse a completed index for the same source revision and configuration.
    pub fn build_cached(cloud: &PointCloud, mut config: IndexConfig) -> Result<Self, LoadError> {
        let stamp = cloud
            .source_stamp
            .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
        if SourceStamp::read(&cloud.path)? != stamp {
            return Err(LoadError::InvalidData(
                "source changed since loading".into(),
            ));
        }
        let fingerprint = cache_fingerprint(cloud, &config)?;
        let cache_root = config.scratch_dir.clone().unwrap_or_else(cache_root);
        fs::create_dir_all(&cache_root)?;
        let cache_path = cache_directory(&cache_root, &fingerprint);
        if cache_path.exists() {
            if let Ok(index) = Self::open_cached(cloud, &cache_path, &fingerprint) {
                return Ok(index);
            }
            fs::remove_dir_all(&cache_path)?;
        }
        config.scratch_dir = Some(cache_root);
        let index = Self::build(cloud, config)?;
        let Self { root, storage } = index;
        let IndexStorage::Temporary(storage) = storage else {
            unreachable!("fresh octree build uses temporary storage")
        };
        fs::write(storage.path().join("source.meta"), &fingerprint)?;
        let temporary_path = storage.keep();
        if let Err(error) = fs::rename(&temporary_path, &cache_path) {
            let _ = fs::remove_dir_all(&temporary_path);
            if cache_path.exists() {
                return Self::open_cached(cloud, &cache_path, &fingerprint);
            }
            return Err(error.into());
        }
        Ok(Self {
            root,
            storage: IndexStorage::Persistent(cache_path),
        })
    }

    /// Attach a valid persistent index without starting an expensive build.
    pub fn open_cached_if_present(
        cloud: &PointCloud,
        config: IndexConfig,
    ) -> Result<Option<Self>, LoadError> {
        let stamp = cloud
            .source_stamp
            .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
        if SourceStamp::read(&cloud.path)? != stamp {
            return Err(LoadError::InvalidData(
                "source changed since loading".into(),
            ));
        }
        let fingerprint = cache_fingerprint(cloud, &config)?;
        let root = config.scratch_dir.unwrap_or_else(cache_root);
        let directory = cache_directory(&root, &fingerprint);
        if !directory.exists() {
            return Ok(None);
        }
        Self::open_cached(cloud, &directory, &fingerprint).map(Some)
    }

    fn open_cached(
        cloud: &PointCloud,
        directory: &Path,
        fingerprint: &[u8],
    ) -> Result<Self, LoadError> {
        if fs::read(directory.join("source.meta"))? != fingerprint {
            return Err(LoadError::InvalidData(
                "octree cache source mismatch".into(),
            ));
        }
        let root = open_cached_node(directory, "r".to_owned(), cloud.bounds, 0)?;
        if root.total_points != cloud.total_points {
            return Err(LoadError::InvalidData(
                "octree cache point count mismatch".into(),
            ));
        }
        Ok(Self {
            root,
            storage: IndexStorage::Persistent(directory.to_path_buf()),
        })
    }

    /// Read an evenly spaced sample of the points kept in a node.
    /// Internal nodes contain preview points; leaves contain their full payload.
    pub fn read_node(&self, id: &str, limit: usize) -> Result<Vec<Point>, LoadError> {
        self.read_node_indexed(id, limit)
            .map(|records| records.into_iter().map(|record| record.point).collect())
    }

    /// Read sampled node points with their original source ordinals.
    pub fn read_node_indexed(
        &self,
        id: &str,
        limit: usize,
    ) -> Result<Vec<IndexedPoint>, LoadError> {
        if limit == 0 {
            return Err(LoadError::InvalidData("read limit must be positive".into()));
        }
        let node = self
            .root
            .find(id)
            .ok_or_else(|| LoadError::InvalidData(format!("octree node not found: {id}")))?;
        let target = limit.min(usize::try_from(node.stored_points).unwrap_or(usize::MAX));
        let mut points = Vec::with_capacity(target);
        let mut index = 0u64;
        read_records(&self.storage.path().join(&node.data_path), |point| {
            let sample_bin =
                (u128::from(index) * target as u128) / u128::from(node.stored_points.max(1));
            if sample_bin >= points.len() as u128 {
                points.push(point);
            }
            index += 1;
            Ok(())
        })?;
        if index != node.stored_points || points.len() != target {
            return Err(LoadError::InvalidData(format!("damaged octree node: {id}")));
        }
        Ok(points)
    }

    /// Read a bounded sample of every leaf intersecting a world-space cube.
    /// This keeps deep-zoom detail independent of the coarse initial preview.
    pub fn sample_region(
        &self,
        focus: [f64; 3],
        radius: f64,
        limit: usize,
    ) -> Result<Vec<Point>, LoadError> {
        if limit == 0
            || !radius.is_finite()
            || radius <= 0.0
            || !focus.iter().all(|v| v.is_finite())
        {
            return Err(LoadError::InvalidData("invalid detail region".into()));
        }
        let mut leaves = Vec::new();
        collect_intersecting_leaves(&self.root, focus, radius, &mut leaves);
        let mut points = Vec::with_capacity(limit);
        let mut matched = 0u64;
        let mut random_state = 0x6a09_e667_f3bc_c909u64;
        for leaf in leaves {
            read_records(&self.storage.path().join(&leaf.data_path), |point| {
                if point
                    .point
                    .xyz
                    .iter()
                    .enumerate()
                    .all(|(axis, value)| (*value - focus[axis]).abs() <= radius)
                {
                    matched += 1;
                    if points.len() < limit {
                        points.push(point.point);
                    } else {
                        random_state ^= random_state << 13;
                        random_state ^= random_state >> 7;
                        random_state ^= random_state << 17;
                        let replacement = random_state % matched;
                        if replacement < limit as u64 {
                            points[replacement as usize] = point.point;
                        }
                    }
                }
                Ok(())
            })?;
        }
        Ok(points)
    }

    /// Select disk nodes by their projected screen size, then read a bounded,
    /// spatially distributed sample from the visible frontier.
    pub fn sample_lod(
        &self,
        limit: usize,
        projected_span: impl FnMut(Bounds) -> Option<f32>,
    ) -> Result<Vec<Point>, LoadError> {
        self.sample_lod_indexed(limit, projected_span)
            .map(|records| records.into_iter().map(|record| record.point).collect())
    }

    /// Spatially distributed LOD points with original source ordinals.
    pub fn sample_lod_indexed(
        &self,
        limit: usize,
        mut projected_span: impl FnMut(Bounds) -> Option<f32>,
    ) -> Result<Vec<IndexedPoint>, LoadError> {
        if limit == 0 {
            return Err(LoadError::InvalidData("read limit must be positive".into()));
        }
        let Some(root_span) = projected_span(self.root.bounds) else {
            return Ok(Vec::new());
        };
        let max_nodes = (limit / 128).clamp(8, 1_024).min(limit);
        let mut frontier = vec![(&self.root, root_span)];
        loop {
            let next = frontier
                .iter()
                .enumerate()
                .filter(|(_, (node, span))| !node.is_leaf() && *span > 96.0)
                .filter_map(|(index, (node, span))| {
                    let visible = node
                        .children
                        .iter()
                        .filter_map(|child| projected_span(child.bounds).map(|size| (child, size)))
                        .collect::<Vec<_>>();
                    (!visible.is_empty() && frontier.len() - 1 + visible.len() <= max_nodes)
                        .then_some((index, *span, visible))
                })
                .max_by(|a, b| a.1.total_cmp(&b.1));
            let Some((index, _, children)) = next else {
                break;
            };
            frontier.swap_remove(index);
            frontier.extend(children);
        }

        let mut allocations = vec![0usize; frontier.len()];
        let capacities = frontier
            .iter()
            .map(|(node, _)| usize::try_from(node.stored_points).unwrap_or(usize::MAX))
            .collect::<Vec<_>>();
        let mut remaining = limit;
        while remaining > 0 {
            let active = allocations
                .iter()
                .zip(&capacities)
                .enumerate()
                .filter_map(|(index, (assigned, capacity))| (assigned < capacity).then_some(index))
                .collect::<Vec<_>>();
            if active.is_empty() {
                break;
            }
            let share = remaining.div_ceil(active.len());
            for index in active {
                let added = share
                    .min(capacities[index] - allocations[index])
                    .min(remaining);
                allocations[index] += added;
                remaining -= added;
            }
        }

        let mut points = Vec::with_capacity(limit - remaining);
        for ((node, _), allocation) in frontier.into_iter().zip(allocations) {
            if allocation > 0 {
                points.extend(self.read_node_indexed(&node.id, allocation)?);
            }
        }
        Ok(points)
    }

    /// Visit exact source points in leaves whose bounds pass a spatial test.
    /// Every record carries its ordinal in the original file for selection.
    pub fn visit_intersecting(
        &self,
        mut intersects: impl FnMut(Bounds) -> bool,
        mut visit: impl FnMut(IndexedPoint) -> Result<(), LoadError>,
    ) -> Result<(), LoadError> {
        visit_intersecting_node(&self.root, self.storage.path(), &mut intersects, &mut visit)
    }
}

fn visit_intersecting_node(
    node: &IndexedNode,
    directory: &Path,
    intersects: &mut impl FnMut(Bounds) -> bool,
    visit: &mut impl FnMut(IndexedPoint) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    if !intersects(node.bounds) {
        return Ok(());
    }
    if node.is_leaf() {
        let mut count = 0u64;
        read_records(&directory.join(&node.data_path), |point| {
            count += 1;
            visit(point)
        })?;
        if count != node.stored_points {
            return Err(LoadError::InvalidData("damaged octree leaf".into()));
        }
    } else {
        for child in &node.children {
            visit_intersecting_node(child, directory, intersects, visit)?;
        }
    }
    Ok(())
}

fn cache_root() -> PathBuf {
    if let Some(root) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(root).join("open-pointcloud-studio/indexes");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache/open-pointcloud-studio/indexes");
    }
    std::env::temp_dir().join("open-pointcloud-studio-indexes")
}

fn cache_directory(root: &Path, fingerprint: &[u8]) -> PathBuf {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in fingerprint {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    root.join(format!("{hash:016x}"))
}

fn cache_fingerprint(cloud: &PointCloud, config: &IndexConfig) -> Result<Vec<u8>, LoadError> {
    let stamp = cloud
        .source_stamp
        .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
    let path = fs::canonicalize(&cloud.path)?;
    let modified = stamp
        .modified
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos());
    Ok(format!(
        "octree-v2\n{:?}\n{}\n{:?}\n{}\n{}\n{}\n",
        path, stamp.length, modified, config.leaf_points, config.preview_points, config.max_depth
    )
    .into_bytes())
}

fn count_records(path: &Path) -> Result<u64, LoadError> {
    let size = fs::metadata(path)?.len();
    if !size.is_multiple_of(RECORD_BYTES as u64) {
        return Err(LoadError::InvalidData("damaged octree cache record".into()));
    }
    Ok(size / RECORD_BYTES as u64)
}

fn open_cached_node(
    directory: &Path,
    id: String,
    bounds: Bounds,
    depth: u8,
) -> Result<IndexedNode, LoadError> {
    let preview_name = format!("{id}-preview.bin");
    let preview_path = directory.join(&preview_name);
    if preview_path.exists() {
        let stored_points = count_records(&preview_path)?;
        let mut children = Vec::new();
        for octant in 0..8 {
            let child_id = format!("{id}{octant}");
            if directory.join(format!("{child_id}.bin")).exists()
                || directory.join(format!("{child_id}-preview.bin")).exists()
            {
                children.push(open_cached_node(
                    directory,
                    child_id,
                    child_bounds(bounds, octant),
                    depth + 1,
                )?);
            }
        }
        if children.is_empty() {
            return Err(LoadError::InvalidData(
                "octree cache has no children".into(),
            ));
        }
        let total_points = children.iter().map(|node| node.total_points).sum();
        return Ok(IndexedNode {
            id,
            bounds,
            total_points,
            stored_points,
            depth,
            children,
            data_path: PathBuf::from(preview_name),
        });
    }
    let leaf_name = format!("{id}.bin");
    let stored_points = count_records(&directory.join(&leaf_name))?;
    Ok(IndexedNode {
        id,
        bounds,
        total_points: stored_points,
        stored_points,
        depth,
        children: Vec::new(),
        data_path: PathBuf::from(leaf_name),
    })
}

fn collect_intersecting_leaves<'a>(
    node: &'a IndexedNode,
    focus: [f64; 3],
    radius: f64,
    leaves: &mut Vec<&'a IndexedNode>,
) {
    if (0..3).any(|axis| {
        node.bounds.max[axis] < focus[axis] - radius || node.bounds.min[axis] > focus[axis] + radius
    }) {
        return;
    }
    if node.is_leaf() {
        leaves.push(node);
    } else {
        for child in &node.children {
            collect_intersecting_leaves(child, focus, radius, leaves);
        }
    }
}

fn build_node(
    directory: &Path,
    id: String,
    input_path: PathBuf,
    bounds: Bounds,
    count: u64,
    depth: u8,
    config: &IndexConfig,
) -> Result<IndexedNode, LoadError> {
    if count <= config.leaf_points || depth >= config.max_depth || bounds.extent() <= f64::EPSILON {
        let data_path = PathBuf::from(format!("{id}.bin"));
        return Ok(IndexedNode {
            id,
            bounds,
            total_points: count,
            stored_points: count,
            depth,
            children: Vec::new(),
            data_path,
        });
    }

    let center = bounds.center();
    let mut writers: [Option<BufWriter<File>>; 8] = array::from_fn(|_| None);
    let mut child_counts = [0u64; 8];
    let mut preview = Vec::with_capacity(config.preview_points);
    let mut random_state = 0x9e37_79b9_7f4a_7c15u64 ^ (count << (depth % 32));
    read_records(&input_path, |point| {
        let index = octant(point.point.xyz, center);
        if writers[index].is_none() {
            let child_path = directory.join(format!("{id}{index}.bin"));
            writers[index] = Some(BufWriter::new(File::create(child_path)?));
        }
        write_record(writers[index].as_mut().unwrap(), point)?;
        child_counts[index] += 1;

        if preview.len() < config.preview_points {
            preview.push(point);
        } else {
            random_state ^= random_state << 13;
            random_state ^= random_state >> 7;
            random_state ^= random_state << 17;
            let seen = child_counts.iter().sum::<u64>();
            let chosen = random_state % seen;
            if chosen < config.preview_points as u64 {
                preview[chosen as usize] = point;
            }
        }
        Ok(())
    })?;
    for writer in writers.iter_mut().flatten() {
        writer.flush()?;
    }
    drop(writers);

    let preview_path = directory.join(format!("{id}-preview.bin"));
    {
        let mut writer = BufWriter::new(File::create(&preview_path)?);
        for point in &preview {
            write_record(&mut writer, *point)?;
        }
        writer.flush()?;
    }
    fs::remove_file(&input_path)?;

    let mut children = Vec::new();
    for (index, child_count) in child_counts.into_iter().enumerate() {
        if child_count == 0 {
            continue;
        }
        let child_id = format!("{id}{index}");
        let child_path = directory.join(format!("{child_id}.bin"));
        children.push(build_node(
            directory,
            child_id,
            child_path,
            child_bounds(bounds, index),
            child_count,
            depth + 1,
            config,
        )?);
    }

    let data_path = PathBuf::from(format!("{id}-preview.bin"));
    Ok(IndexedNode {
        id,
        bounds,
        total_points: count,
        stored_points: preview.len() as u64,
        depth,
        children,
        data_path,
    })
}

fn octant(xyz: [f64; 3], center: [f64; 3]) -> usize {
    usize::from(xyz[0] >= center[0])
        | (usize::from(xyz[1] >= center[1]) << 1)
        | (usize::from(xyz[2] >= center[2]) << 2)
}

fn child_bounds(parent: Bounds, index: usize) -> Bounds {
    let center = parent.center();
    let mut result = parent;
    for (axis, value) in center.into_iter().enumerate() {
        if index & (1 << axis) == 0 {
            result.max[axis] = value;
        } else {
            result.min[axis] = value;
        }
    }
    result
}

fn write_record(writer: &mut impl Write, indexed: IndexedPoint) -> Result<(), LoadError> {
    let point = indexed.point;
    for coordinate in point.xyz {
        writer.write_all(&coordinate.to_le_bytes())?;
    }
    let rgb = point.rgb.unwrap_or([0; 3]);
    writer.write_all(&rgb)?;
    writer.write_all(&point.intensity.unwrap_or(0).to_le_bytes())?;
    writer.write_all(&[point.classification.unwrap_or(0)])?;
    let flags = u8::from(point.rgb.is_some())
        | (u8::from(point.intensity.is_some()) << 1)
        | (u8::from(point.classification.is_some()) << 2);
    writer.write_all(&[flags, 0])?;
    writer.write_all(&indexed.ordinal.to_le_bytes())?;
    Ok(())
}

fn read_records(
    path: &Path,
    mut push: impl FnMut(IndexedPoint) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut reader = BufReader::new(File::open(path)?);
    loop {
        let mut bytes = [0u8; RECORD_BYTES];
        if reader.read(&mut bytes[..1])? == 0 {
            break;
        }
        reader.read_exact(&mut bytes[1..])?;
        let xyz = array::from_fn(|axis| {
            let start = axis * 8;
            f64::from_le_bytes(bytes[start..start + 8].try_into().unwrap())
        });
        let flags = bytes[30];
        push(IndexedPoint {
            point: Point {
                xyz,
                rgb: (flags & 1 != 0).then_some([bytes[24], bytes[25], bytes[26]]),
                intensity: (flags & 2 != 0).then_some(u16::from_le_bytes([bytes[27], bytes[28]])),
                classification: (flags & 4 != 0).then_some(bytes[29]),
            },
            ordinal: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partitions_to_disk_without_losing_points() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("grid.xyz");
        let mut contents = String::new();
        for x in 0..16 {
            for y in 0..16 {
                contents.push_str(&format!("{x} {y} {} 12 34 56\n", (x + y) % 4));
            }
        }
        fs::write(&path, contents).unwrap();
        let cloud = super::super::open(&path, 10).unwrap();
        let index = OctreeIndex::build(
            &cloud,
            IndexConfig {
                leaf_points: 8,
                preview_points: 5,
                max_depth: 8,
                scratch_dir: Some(directory.path().to_path_buf()),
            },
        )
        .unwrap();
        assert_eq!(index.root.total_points, 256);
        assert_eq!(index.read_node("r", 100).unwrap().len(), 5);
        assert_eq!(index.read_node("r", 4).unwrap().len(), 4);
        for record in index.read_node_indexed("r", 4).unwrap() {
            assert_eq!(record.point.xyz[0] as u64, record.ordinal / 16);
            assert_eq!(record.point.xyz[1] as u64, record.ordinal % 16);
        }
        assert!(!index.root.is_leaf());

        fn verify(node: &IndexedNode, index: &OctreeIndex) -> u64 {
            if node.is_leaf() {
                assert!(node.total_points <= 8);
                let points = index.read_node(&node.id, 100).unwrap();
                assert_eq!(points.len() as u64, node.total_points);
                assert!(points.iter().all(|point| point.rgb == Some([12, 34, 56])));
                node.total_points
            } else {
                node.children.iter().map(|child| verify(child, index)).sum()
            }
        }
        assert_eq!(verify(&index.root, &index), 256);
        let mut ordinals = Vec::new();
        index
            .visit_intersecting(
                |_| true,
                |record| {
                    ordinals.push(record.ordinal);
                    assert_eq!(record.point.xyz[0] as u64, record.ordinal / 16);
                    assert_eq!(record.point.xyz[1] as u64, record.ordinal % 16);
                    Ok(())
                },
            )
            .unwrap();
        ordinals.sort_unstable();
        assert_eq!(ordinals, (0..256).collect::<Vec<_>>());
        let detail = index.sample_region([4.0, 4.0, 1.5], 2.0, 100).unwrap();
        assert_eq!(detail.len(), 25);
        assert!(detail.iter().all(|point| {
            (2.0..=6.0).contains(&point.xyz[0]) && (2.0..=6.0).contains(&point.xyz[1])
        }));
        let bounded = index.sample_region([4.0, 4.0, 1.5], 2.0, 7).unwrap();
        assert_eq!(bounded.len(), 7);
        let overview = index
            .sample_lod(48, |bounds| Some(bounds.extent() as f32 * 100.0))
            .unwrap();
        assert!(overview.len() <= 48);
        assert!(overview.len() >= 20);
        assert!(overview.iter().any(|point| point.xyz[0] < 8.0));
        assert!(overview.iter().any(|point| point.xyz[0] >= 8.0));
        for record in index
            .sample_lod_indexed(48, |bounds| Some(bounds.extent() as f32 * 100.0))
            .unwrap()
        {
            assert_eq!(record.point.xyz[0] as u64, record.ordinal / 16);
            assert_eq!(record.point.xyz[1] as u64, record.ordinal % 16);
        }
        let west = index
            .sample_lod(48, |bounds| {
                (bounds.min[0] < 7.5).then_some(bounds.extent() as f32 * 100.0)
            })
            .unwrap();
        assert!(!west.is_empty());
        assert!(west.iter().all(|point| point.xyz[0] < 8.0));
    }

    #[test]
    fn cached_index_reopens_without_rebuilding() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.xyz");
        fs::write(&source, "0 0 0\n1 0 0\n2 0 0\n3 0 0\n").unwrap();
        let cloud = super::super::open(&source, 2).unwrap();
        let config = IndexConfig {
            leaf_points: 2,
            preview_points: 1,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("cache")),
        };
        assert!(OctreeIndex::open_cached_if_present(&cloud, config.clone())
            .unwrap()
            .is_none());
        let first = OctreeIndex::build_cached(&cloud, config.clone()).unwrap();
        let cache_path = first.storage.path().to_path_buf();
        assert!(cache_path.join("source.meta").exists());
        drop(first);
        let second = OctreeIndex::open_cached_if_present(&cloud, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(second.storage.path(), cache_path);
        assert_eq!(second.root.total_points, 4);
        assert_eq!(
            second.sample_region([1.5, 0.0, 0.0], 2.0, 4).unwrap().len(),
            4
        );
        assert_eq!(
            OctreeIndex::build_cached(&cloud, config)
                .unwrap()
                .storage
                .path(),
            cache_path
        );
    }
}
