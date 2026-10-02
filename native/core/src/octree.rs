//! Disk-backed octree indexing. Only node metadata and one small preview at a
//! time need to be held in memory; point records stay in temporary files.

use std::array;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use super::{e57_points, visit_points, Bounds, LoadError, Point, PointCloud, SourceStamp};

const RECORD_BYTES: usize = 40;
const RECORD_BATCH_POINTS: usize = 8_192;
const LEAF_LOD_POINTS: usize = 2_048;

#[derive(Serialize, Deserialize)]
struct CachedCloudHeader {
    version: u8,
    total_points: u64,
    min: [f64; 3],
    max: [f64; 3],
    has_rgb: bool,
    has_intensity: bool,
    has_classification: bool,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexStage {
    ReadingSource,
    BuildingTree,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexProgress {
    pub stage: IndexStage,
    /// Source points read, or cumulative point records handled by tree nodes.
    pub completed: u64,
    /// Known only while reading the original source.
    pub total: u64,
    pub depth: u8,
    pub leaves: u64,
}

impl IndexProgress {
    fn reading(completed: u64, total: u64) -> Self {
        Self {
            stage: IndexStage::ReadingSource,
            completed,
            total,
            depth: 0,
            leaves: 0,
        }
    }

    fn building(completed: u64, depth: u8, leaves: u64) -> Self {
        Self {
            stage: IndexStage::BuildingTree,
            completed,
            total: 0,
            depth,
            leaves,
        }
    }

    fn ready(total: u64, leaves: u64) -> Self {
        Self {
            stage: IndexStage::Ready,
            completed: total,
            total,
            depth: 0,
            leaves,
        }
    }
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
        Self::build_with_progress(cloud, config, |_| Ok(()))
    }

    pub fn build_with_progress(
        cloud: &PointCloud,
        config: IndexConfig,
        mut progress: impl FnMut(IndexProgress) -> Result<(), LoadError>,
    ) -> Result<Self, LoadError> {
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
        progress(IndexProgress::reading(0, cloud.total_points))?;
        {
            let mut writer = BufWriter::new(File::create(&root_path)?);
            visit_points(&cloud.path, &mut |point| {
                if root_count.is_multiple_of(65_536) {
                    progress(IndexProgress::reading(root_count, cloud.total_points))?;
                }
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
        progress(IndexProgress::reading(root_count, cloud.total_points))?;
        if root_count != cloud.total_points || SourceStamp::read(&cloud.path)? != expected_stamp {
            return Err(LoadError::InvalidData(
                "source changed while indexing".into(),
            ));
        }
        let mut handled_records = 0u64;
        let mut ready_leaves = 0u64;
        progress(IndexProgress::building(0, 0, 0))?;
        let mut context = BuildContext {
            directory: storage.path(),
            config: &config,
            handled_records: &mut handled_records,
            ready_leaves: &mut ready_leaves,
            progress: &mut progress,
        };
        let root = build_node(
            "r".to_owned(),
            root_path,
            cloud.bounds,
            root_count,
            0,
            &mut context,
        )?;
        progress(IndexProgress::ready(root_count, ready_leaves))?;
        Ok(Self {
            root,
            storage: IndexStorage::Temporary(storage),
        })
    }

    /// Reuse a completed index for the same source revision and configuration.
    pub fn build_cached(cloud: &PointCloud, config: IndexConfig) -> Result<Self, LoadError> {
        Self::build_cached_with_progress(cloud, config, |_| Ok(()))
    }

    pub fn build_cached_with_progress(
        cloud: &PointCloud,
        mut config: IndexConfig,
        mut progress: impl FnMut(IndexProgress) -> Result<(), LoadError>,
    ) -> Result<Self, LoadError> {
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
                let _ = write_cached_cloud_header(&cache_path, cloud);
                progress(IndexProgress::ready(
                    cloud.total_points,
                    count_leaves(&index.root),
                ))?;
                return Ok(index);
            }
            fs::remove_dir_all(&cache_path)?;
        }
        config.scratch_dir = Some(cache_root);
        let index = Self::build_with_progress(cloud, config, &mut progress)?;
        let Self { root, storage } = index;
        let IndexStorage::Temporary(storage) = storage else {
            unreachable!("fresh octree build uses temporary storage")
        };
        fs::write(storage.path().join("source.meta"), &fingerprint)?;
        write_cached_cloud_header(storage.path(), cloud)?;
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
        let index = Self::open_cached(cloud, &directory, &fingerprint)?;
        let _ = write_cached_cloud_header(&directory, cloud);
        Ok(Some(index))
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
        self.read_node_indexed_where(id, limit, &|| false)
    }

    fn read_node_indexed_where(
        &self,
        id: &str,
        limit: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<IndexedPoint>, LoadError> {
        if limit == 0 {
            return Err(LoadError::InvalidData("read limit must be positive".into()));
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let node = self
            .root
            .find(id)
            .ok_or_else(|| LoadError::InvalidData(format!("octree node not found: {id}")))?;
        let target = limit.min(usize::try_from(node.stored_points).unwrap_or(usize::MAX));
        let mut points = Vec::with_capacity(target);
        let path = self.storage.path().join(&node.data_path);
        let (path, stored_points) = if node.is_leaf()
            && target <= LEAF_LOD_POINTS
            && node.stored_points > (LEAF_LOD_POINTS * 4) as u64
        {
            let preview = leaf_lod_path(self.storage.path(), id);
            match ensure_leaf_lod_where(&path, &preview, node.stored_points, cancelled) {
                Ok(()) => (preview, LEAF_LOD_POINTS as u64),
                // A read-only cache still remains usable through the full leaf.
                Err(LoadError::Io(_)) => (path, node.stored_points),
                Err(error) => return Err(error),
            }
        } else {
            (path, node.stored_points)
        };
        let mut index = 0u64;
        read_records(&path, |point| {
            if index.is_multiple_of(4_096) && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let sample_bin =
                (u128::from(index) * target as u128) / u128::from(stored_points.max(1));
            if sample_bin >= points.len() as u128 {
                points.push(point);
            }
            index += 1;
            Ok(())
        })?;
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        if index != stored_points || points.len() != target {
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
        projected_span: impl FnMut(Bounds) -> Option<f32>,
    ) -> Result<Vec<IndexedPoint>, LoadError> {
        self.sample_lod_indexed_cancellable(limit, projected_span, || false)
    }

    /// Stop a stale viewport request while it scans node or leaf-preview files.
    pub fn sample_lod_indexed_cancellable(
        &self,
        limit: usize,
        mut projected_span: impl FnMut(Bounds) -> Option<f32>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Vec<IndexedPoint>, LoadError> {
        if limit == 0 {
            return Err(LoadError::InvalidData("read limit must be positive".into()));
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let Some(root_span) = projected_span(self.root.bounds) else {
            return Ok(Vec::new());
        };
        let max_nodes = (limit / 128).clamp(8, 1_024).min(limit);
        let mut frontier = vec![(&self.root, root_span)];
        loop {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
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
                points.extend(self.read_node_indexed_where(&node.id, allocation, &cancelled)?);
            }
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        Ok(points)
    }

    /// At deep zoom, read nearby leaves exactly and sample only points that
    /// project into the viewport. Return `None` when the candidate leaves are
    /// too large, allowing the caller to use the regular node-preview LOD.
    pub fn sample_visible_indexed_cancellable(
        &self,
        limit: usize,
        max_scan_points: u64,
        mut visible_node: impl FnMut(Bounds) -> bool,
        mut visible_point: impl FnMut(IndexedPoint) -> bool,
        cancelled: impl Fn() -> bool,
    ) -> Result<Option<Vec<IndexedPoint>>, LoadError> {
        if limit == 0 || max_scan_points == 0 {
            return Err(LoadError::InvalidData("read limit must be positive".into()));
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let mut leaves = Vec::new();
        let mut candidates = 0u64;
        if collect_visible_leaves(
            &self.root,
            &mut visible_node,
            &mut leaves,
            &mut candidates,
            max_scan_points,
        ) {
            return Ok(None);
        }
        let initial_capacity = limit
            .min(usize::try_from(candidates).unwrap_or(usize::MAX))
            .min(65_536);
        let mut points = Vec::with_capacity(initial_capacity);
        let mut matched = 0u64;
        let mut random_state = 0x6a09_e667_f3bc_c909u64;
        for leaf in leaves {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            let mut read = 0u64;
            read_records(&self.storage.path().join(&leaf.data_path), |record| {
                if read.is_multiple_of(4_096) && cancelled() {
                    return Err(LoadError::Cancelled);
                }
                read += 1;
                if visible_point(record) {
                    matched += 1;
                    if points.len() < limit {
                        points.push(record);
                    } else {
                        random_state ^= random_state << 13;
                        random_state ^= random_state >> 7;
                        random_state ^= random_state << 17;
                        let replacement = random_state % matched;
                        if replacement < limit as u64 {
                            points[replacement as usize] = record;
                        }
                    }
                }
                Ok(())
            })?;
            if read != leaf.stored_points {
                return Err(LoadError::InvalidData("damaged octree leaf".into()));
            }
        }
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        Ok(Some(points))
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

fn collect_visible_leaves<'a>(
    node: &'a IndexedNode,
    visible: &mut impl FnMut(Bounds) -> bool,
    leaves: &mut Vec<&'a IndexedNode>,
    candidates: &mut u64,
    max_scan_points: u64,
) -> bool {
    if !visible(node.bounds) {
        return false;
    }
    if node.is_leaf() {
        *candidates = candidates.saturating_add(node.stored_points);
        if *candidates > max_scan_points {
            return true;
        }
        leaves.push(node);
        return false;
    }
    node.children
        .iter()
        .any(|child| collect_visible_leaves(child, visible, leaves, candidates, max_scan_points))
}

/// Recover exact PLY or E57 metadata and a small preview from an already
/// validated disk index, without decoding the multi-gigabyte source again.
pub(crate) fn open_cached_preview(
    path: &Path,
    sample_limit: usize,
    config: IndexConfig,
) -> Result<Option<PointCloud>, LoadError> {
    let stamp = SourceStamp::read(path)?;
    let fingerprint = cache_fingerprint_for(path, stamp, &config)?;
    let root = config.scratch_dir.unwrap_or_else(cache_root);
    let directory = cache_directory(&root, &fingerprint);
    if !directory.exists() || !directory.join("cloud.json").exists() {
        return Ok(None);
    }
    if fs::read(directory.join("source.meta"))? != fingerprint {
        return Err(LoadError::InvalidData(
            "octree cache source mismatch".into(),
        ));
    }
    let metadata_path = directory.join("cloud.json");
    if fs::metadata(&metadata_path)?.len() > 4_096 {
        return Err(LoadError::InvalidData(
            "oversized octree cloud metadata".into(),
        ));
    }
    let header: CachedCloudHeader =
        serde_json::from_slice(&fs::read(metadata_path)?).map_err(|error| {
            LoadError::InvalidData(format!("invalid octree cloud metadata: {error}"))
        })?;
    if header.version != 1
        || header.total_points == 0
        || (0..3).any(|axis| {
            !header.min[axis].is_finite()
                || !header.max[axis].is_finite()
                || header.min[axis] > header.max[axis]
        })
    {
        return Err(LoadError::InvalidData("invalid octree cloud bounds".into()));
    }
    let mut cloud = PointCloud {
        path: path.to_path_buf(),
        total_points: header.total_points,
        bounds: Bounds {
            min: header.min,
            max: header.max,
        },
        points: Vec::new(),
        point_ordinals: Vec::new(),
        has_rgb: header.has_rgb,
        has_intensity: header.has_intensity,
        has_classification: header.has_classification,
        scan_poses: Vec::new(),
        source_stamp: Some(stamp),
    };
    let index = OctreeIndex::open_cached(&cloud, &directory, &fingerprint)?;
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("e57"))
    {
        cloud.scan_poses = e57_points::scan_poses(path)?;
    }
    for record in index.read_node_indexed("r", sample_limit)? {
        cloud.points.push(record.point);
        cloud.point_ordinals.push(record.ordinal);
    }
    if SourceStamp::read(path)? != stamp {
        return Err(LoadError::InvalidData(
            "source changed while loading cached preview".into(),
        ));
    }
    Ok(Some(cloud))
}

fn write_cached_cloud_header(directory: &Path, cloud: &PointCloud) -> Result<(), LoadError> {
    let header = CachedCloudHeader {
        version: 1,
        total_points: cloud.total_points,
        min: cloud.bounds.min,
        max: cloud.bounds.max,
        has_rgb: cloud.has_rgb,
        has_intensity: cloud.has_intensity,
        has_classification: cloud.has_classification,
    };
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(temporary.as_file_mut(), &header).map_err(|error| {
        LoadError::InvalidData(format!("cannot write octree metadata: {error}"))
    })?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(directory.join("cloud.json"))
        .map_err(|error| error.error)?;
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
    cache_fingerprint_for(&cloud.path, stamp, config)
}

fn cache_fingerprint_for(
    source: &Path,
    stamp: SourceStamp,
    config: &IndexConfig,
) -> Result<Vec<u8>, LoadError> {
    let path = fs::canonicalize(source)?;
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

fn count_leaves(node: &IndexedNode) -> u64 {
    if node.is_leaf() {
        1
    } else {
        node.children.iter().map(count_leaves).sum()
    }
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

struct BuildContext<'a, F> {
    directory: &'a Path,
    config: &'a IndexConfig,
    handled_records: &'a mut u64,
    ready_leaves: &'a mut u64,
    progress: &'a mut F,
}

impl<F: FnMut(IndexProgress) -> Result<(), LoadError>> BuildContext<'_, F> {
    fn emit(&mut self, depth: u8) -> Result<(), LoadError> {
        (self.progress)(IndexProgress::building(
            *self.handled_records,
            depth,
            *self.ready_leaves,
        ))
    }
}

fn build_node<F: FnMut(IndexProgress) -> Result<(), LoadError>>(
    id: String,
    input_path: PathBuf,
    bounds: Bounds,
    count: u64,
    depth: u8,
    context: &mut BuildContext<'_, F>,
) -> Result<IndexedNode, LoadError> {
    context.emit(depth)?;
    if count <= context.config.leaf_points
        || depth >= context.config.max_depth
        || bounds.extent() <= f64::EPSILON
    {
        if count > (LEAF_LOD_POINTS * 4) as u64 {
            ensure_leaf_lod(&input_path, &leaf_lod_path(context.directory, &id), count)?;
        }
        *context.handled_records = context.handled_records.saturating_add(count);
        *context.ready_leaves += 1;
        context.emit(depth)?;
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
    let mut preview = Vec::with_capacity(context.config.preview_points);
    let mut random_state = 0x9e37_79b9_7f4a_7c15u64 ^ (count << (depth % 32));
    read_records(&input_path, |point| {
        if context.handled_records.is_multiple_of(65_536) {
            context.emit(depth)?;
        }
        let index = octant(point.point.xyz, center);
        if writers[index].is_none() {
            let child_path = context.directory.join(format!("{id}{index}.bin"));
            writers[index] = Some(BufWriter::new(File::create(child_path)?));
        }
        write_record(writers[index].as_mut().unwrap(), point)?;
        child_counts[index] += 1;
        *context.handled_records += 1;

        if preview.len() < context.config.preview_points {
            preview.push(point);
        } else {
            random_state ^= random_state << 13;
            random_state ^= random_state >> 7;
            random_state ^= random_state << 17;
            let seen = child_counts.iter().sum::<u64>();
            let chosen = random_state % seen;
            if chosen < context.config.preview_points as u64 {
                preview[chosen as usize] = point;
            }
        }
        Ok(())
    })?;
    for writer in writers.iter_mut().flatten() {
        writer.flush()?;
    }
    drop(writers);

    let preview_path = context.directory.join(format!("{id}-preview.bin"));
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
        let child_path = context.directory.join(format!("{child_id}.bin"));
        children.push(build_node(
            child_id,
            child_path,
            child_bounds(bounds, index),
            child_count,
            depth + 1,
            context,
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
    let mut record = [0u8; RECORD_BYTES];
    for (axis, coordinate) in point.xyz.into_iter().enumerate() {
        let start = axis * 8;
        record[start..start + 8].copy_from_slice(&coordinate.to_le_bytes());
    }
    record[24..27].copy_from_slice(&point.rgb.unwrap_or([0; 3]));
    record[27..29].copy_from_slice(&point.intensity.unwrap_or(0).to_le_bytes());
    record[29] = point.classification.unwrap_or(0);
    let flags = u8::from(point.rgb.is_some())
        | (u8::from(point.intensity.is_some()) << 1)
        | (u8::from(point.classification.is_some()) << 2);
    record[30] = flags;
    record[32..40].copy_from_slice(&indexed.ordinal.to_le_bytes());
    writer.write_all(&record)?;
    Ok(())
}

fn read_records(
    path: &Path,
    mut push: impl FnMut(IndexedPoint) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut batch = vec![0u8; RECORD_BATCH_POINTS * RECORD_BYTES];
    loop {
        let mut filled = 0;
        while filled < batch.len() {
            let amount = reader.read(&mut batch[filled..])?;
            if amount == 0 {
                break;
            }
            filled += amount;
        }
        if filled == 0 {
            break;
        }
        if !filled.is_multiple_of(RECORD_BYTES) {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        for bytes in batch[..filled].as_chunks::<RECORD_BYTES>().0 {
            push(decode_record(bytes))?;
        }
        if filled < batch.len() {
            break;
        }
    }
    Ok(())
}

fn leaf_lod_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}-lod.bin"))
}

fn ensure_leaf_lod(source: &Path, preview: &Path, count: u64) -> Result<(), LoadError> {
    ensure_leaf_lod_where(source, preview, count, &|| false)
}

fn ensure_leaf_lod_where(
    source: &Path,
    preview: &Path,
    count: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<(), LoadError> {
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    let source_bytes = count
        .checked_mul(RECORD_BYTES as u64)
        .ok_or_else(|| LoadError::InvalidData("damaged octree leaf".into()))?;
    if fs::metadata(source)?.len() != source_bytes {
        return Err(LoadError::InvalidData("damaged octree leaf".into()));
    }
    let preview_bytes = LEAF_LOD_POINTS as u64 * RECORD_BYTES as u64;
    if fs::metadata(preview).is_ok_and(|metadata| metadata.len() == preview_bytes) {
        return Ok(());
    }
    let directory = preview
        .parent()
        .ok_or_else(|| LoadError::InvalidData("invalid octree preview path".into()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    let mut seen = 0u64;
    let mut written = 0usize;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        read_records(source, |point| {
            if seen.is_multiple_of(4_096) && cancelled() {
                return Err(LoadError::Cancelled);
            }
            let bin = (u128::from(seen) * LEAF_LOD_POINTS as u128) / u128::from(count);
            if bin >= written as u128 {
                write_record(&mut writer, point)?;
                written += 1;
            }
            seen += 1;
            Ok(())
        })?;
        writer.flush()?;
    }
    if seen != count || written != LEAF_LOD_POINTS {
        return Err(LoadError::InvalidData("damaged octree leaf".into()));
    }
    if cancelled() {
        return Err(LoadError::Cancelled);
    }
    temporary
        .persist(preview)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(())
}

fn decode_record(bytes: &[u8; RECORD_BYTES]) -> IndexedPoint {
    let xyz = array::from_fn(|axis| {
        let start = axis * 8;
        f64::from_le_bytes(bytes[start..start + 8].try_into().unwrap())
    });
    let flags = bytes[30];
    IndexedPoint {
        point: Point {
            xyz,
            rgb: (flags & 1 != 0).then_some([bytes[24], bytes[25], bytes[26]]),
            intensity: (flags & 2 != 0).then_some(u16::from_le_bytes([bytes[27], bytes[28]])),
            classification: (flags & 4 != 0).then_some(bytes[29]),
        },
        ordinal: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batched_records_keep_ordinals_attributes_and_reject_truncation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("records.bin");
        let count = RECORD_BATCH_POINTS + 3;
        let mut writer = BufWriter::new(File::create(&path).unwrap());
        for ordinal in 0..count {
            write_record(
                &mut writer,
                IndexedPoint {
                    point: Point {
                        xyz: [ordinal as f64, -2.5, 3.25],
                        rgb: (ordinal % 2 == 0).then_some([10, 20, 30]),
                        intensity: (ordinal % 3 == 0).then_some(12_345),
                        classification: (ordinal % 5 == 0).then_some(6),
                    },
                    ordinal: ordinal as u64,
                },
            )
            .unwrap();
        }
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            (count * RECORD_BYTES) as u64
        );
        let mut seen = 0;
        read_records(&path, |indexed| {
            assert_eq!(indexed.ordinal, seen as u64);
            assert_eq!(indexed.point.xyz, [seen as f64, -2.5, 3.25]);
            assert_eq!(indexed.point.rgb, (seen % 2 == 0).then_some([10, 20, 30]));
            assert_eq!(indexed.point.intensity, (seen % 3 == 0).then_some(12_345));
            assert_eq!(indexed.point.classification, (seen % 5 == 0).then_some(6));
            seen += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, count);

        let file = File::options().write(true).open(&path).unwrap();
        file.set_len((count * RECORD_BYTES - 1) as u64).unwrap();
        assert!(matches!(
            read_records(&path, |_| Ok(())),
            Err(LoadError::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn indexing_reports_work_and_cancel_leaves_no_partial_cache() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("progress.xyz");
        let mut contents = String::new();
        for x in 0..32 {
            for y in 0..16 {
                contents.push_str(&format!("{x} {y} 0\n"));
            }
        }
        fs::write(&source, contents).unwrap();
        let cloud = super::super::open(&source, 8).unwrap();
        let config = IndexConfig {
            leaf_points: 8,
            preview_points: 4,
            max_depth: 8,
            scratch_dir: Some(directory.path().join("cache")),
        };
        let mut updates = Vec::new();
        let index = OctreeIndex::build_cached_with_progress(&cloud, config.clone(), |value| {
            updates.push(value);
            Ok(())
        })
        .unwrap();
        assert_eq!(index.root.total_points, 512);
        assert!(updates.iter().any(|value| {
            value.stage == IndexStage::ReadingSource && value.completed == 512 && value.total == 512
        }));
        assert!(updates
            .iter()
            .any(|value| value.stage == IndexStage::BuildingTree && value.leaves > 0));
        assert_eq!(updates.last().unwrap().stage, IndexStage::Ready);
        assert_eq!(updates.last().unwrap().leaves, count_leaves(&index.root));
        drop(index);

        let mut cached_updates = Vec::new();
        OctreeIndex::build_cached_with_progress(&cloud, config.clone(), |value| {
            cached_updates.push(value);
            Ok(())
        })
        .unwrap();
        assert_eq!(cached_updates.len(), 1);
        assert_eq!(cached_updates[0].stage, IndexStage::Ready);

        let cancelled_config = IndexConfig {
            leaf_points: 4,
            ..config
        };
        let result =
            OctreeIndex::build_cached_with_progress(&cloud, cancelled_config.clone(), |value| {
                if value.stage == IndexStage::BuildingTree && value.leaves > 0 {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            });
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert!(
            OctreeIndex::open_cached_if_present(&cloud, cancelled_config)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn leaf_lod_preview_keeps_source_ordinals_and_repairs_cache() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("line.xyz");
        let mut lines = String::new();
        for ordinal in 0..16384 {
            lines.push_str(&format!("{ordinal} 0 0\n"));
        }
        fs::write(&source, lines).unwrap();
        let cloud = super::super::open(&source, 16).unwrap();
        let index = OctreeIndex::build(
            &cloud,
            IndexConfig {
                leaf_points: 20000,
                preview_points: 16,
                max_depth: 4,
                scratch_dir: Some(directory.path().to_path_buf()),
            },
        )
        .unwrap();
        let preview = leaf_lod_path(index.storage.path(), "r");
        assert_eq!(
            fs::metadata(&preview).unwrap().len(),
            2048 * RECORD_BYTES as u64
        );
        fs::remove_file(&preview).unwrap();
        let checks = std::sync::atomic::AtomicUsize::new(0);
        let cancelled = index.sample_lod_indexed_cancellable(
            61,
            |_| Some(100.0),
            || checks.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 5,
        );
        assert!(matches!(cancelled, Err(LoadError::Cancelled)));
        assert!(!preview.exists());
        let sample = index.read_node_indexed("r", 61).unwrap();
        assert!(preview.exists());
        assert_eq!(sample.len(), 61);
        for (bin, record) in sample.into_iter().enumerate() {
            let preview_index = (bin as u64 * 2048).div_ceil(61);
            let expected = preview_index * 8;
            assert_eq!(record.ordinal, expected);
            assert_eq!(record.point.xyz, [expected as f64, 0.0, 0.0]);
        }
        fs::write(&preview, [0]).unwrap();
        assert_eq!(index.read_node_indexed("r", 61).unwrap().len(), 61);
        assert_eq!(
            fs::metadata(&preview).unwrap().len(),
            2048 * RECORD_BYTES as u64
        );
        let path = index.storage.path().join(&index.root.data_path);
        let file = File::options().write(true).open(path).unwrap();
        file.set_len(16383 * RECORD_BYTES as u64).unwrap();
        assert!(index.read_node_indexed("r", 61).is_err());
    }

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
    fn exact_visible_sample_uses_source_ordinals_and_skips_large_scans() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("grid.xyz");
        let mut contents = String::new();
        for x in 0..32 {
            for y in 0..16 {
                contents.push_str(&format!("{x} {y} 0\n"));
            }
        }
        fs::write(&path, contents).unwrap();
        let cloud = super::super::open(&path, 8).unwrap();
        let index = OctreeIndex::build(
            &cloud,
            IndexConfig {
                leaf_points: 8,
                preview_points: 4,
                max_depth: 8,
                scratch_dir: Some(directory.path().to_path_buf()),
            },
        )
        .unwrap();
        let overlaps = |bounds: Bounds| {
            bounds.min[0] <= 12.0
                && bounds.max[0] >= 8.0
                && bounds.min[1] <= 8.0
                && bounds.max[1] >= 4.0
        };
        let visible = |record: IndexedPoint| {
            (8.0..=12.0).contains(&record.point.xyz[0])
                && (4.0..=8.0).contains(&record.point.xyz[1])
        };
        let exact = index
            .sample_visible_indexed_cancellable(100, 512, overlaps, visible, || false)
            .unwrap()
            .unwrap();
        assert_eq!(exact.len(), 25);
        assert!(exact.iter().all(|record| visible(*record)));
        assert!(exact.iter().all(|record| {
            record.ordinal == record.point.xyz[0] as u64 * 16 + record.point.xyz[1] as u64
        }));

        let sample = index
            .sample_visible_indexed_cancellable(7, 512, overlaps, visible, || false)
            .unwrap()
            .unwrap();
        assert_eq!(sample.len(), 7);
        assert!(sample.iter().all(|record| visible(*record)));
        assert!(index
            .sample_visible_indexed_cancellable(7, 1, overlaps, visible, || false)
            .unwrap()
            .is_none());
        assert!(matches!(
            index.sample_visible_indexed_cancellable(7, 512, overlaps, visible, || true),
            Err(LoadError::Cancelled)
        ));
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

    #[test]
    fn cached_ply_preview_uses_exact_index_metadata_and_rejects_stale_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("scan.ply");
        fs::write(
            &source,
            "ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nend_header\n0 0 0\n1 2 3\n2 4 6\n3 6 9\n",
        )
        .unwrap();
        let cloud = super::super::open(&source, 2).unwrap();
        let config = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("cache")),
        };
        let index = OctreeIndex::build_cached(&cloud, config.clone()).unwrap();
        let cache_path = index.storage.path().to_path_buf();
        assert!(cache_path.join("cloud.json").exists());

        let reopened = open_cached_preview(&source, 2, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(reopened.total_points, 4);
        assert_eq!(reopened.bounds, cloud.bounds);
        assert_eq!(reopened.points.len(), 2);
        assert_eq!(reopened.point_ordinals.len(), 2);

        fs::remove_file(cache_path.join("cloud.json")).unwrap();
        assert!(open_cached_preview(&source, 2, config.clone())
            .unwrap()
            .is_none());
        OctreeIndex::open_cached_if_present(&cloud, config.clone())
            .unwrap()
            .unwrap();
        assert!(cache_path.join("cloud.json").exists());

        fs::write(&source, "changed source").unwrap();
        assert!(open_cached_preview(&source, 2, config).unwrap().is_none());
    }

    #[test]
    fn cached_e57_preview_preserves_scanner_pose_without_decoding_points() {
        use e57::{E57Writer, Record, RecordValue, Transform, Translation};

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("posed-scan.e57");
        let mut writer = E57Writer::new(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&source)
                .unwrap(),
            "{00000000-0000-4000-8000-000000000001}",
        )
        .unwrap();
        {
            let mut scan = writer
                .add_pointcloud(
                    "{00000000-0000-4000-8000-000000000002}",
                    vec![
                        Record::CARTESIAN_X_F64,
                        Record::CARTESIAN_Y_F64,
                        Record::CARTESIAN_Z_F64,
                    ],
                )
                .unwrap();
            scan.set_name(Some("West station".into()));
            scan.set_transform(Some(Transform {
                rotation: Default::default(),
                translation: Translation {
                    x: 100.0,
                    y: 200.0,
                    z: 10.0,
                },
            }));
            for x in [1.0, 2.0, 3.0, 4.0] {
                scan.add_point(vec![
                    RecordValue::Double(x),
                    RecordValue::Double(0.0),
                    RecordValue::Double(0.0),
                ])
                .unwrap();
            }
            scan.finalize().unwrap();
        }
        writer.finalize().unwrap();

        let cloud = super::super::open(&source, 2).unwrap();
        let config = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("cache")),
        };
        let _index = OctreeIndex::build_cached(&cloud, config.clone()).unwrap();
        let cached = open_cached_preview(&source, 2, config).unwrap().unwrap();
        assert_eq!(cached.total_points, cloud.total_points);
        assert_eq!(cached.bounds, cloud.bounds);
        assert_eq!(cached.scan_poses, cloud.scan_poses);
        assert_eq!(cached.scan_poses[0].label, "West station");
        assert_eq!(cached.scan_poses[0].position, [100.0, 200.0, 10.0]);
        assert_eq!(cached.points.len(), 2);
        assert_eq!(cached.point_ordinals.len(), 2);
    }
}
