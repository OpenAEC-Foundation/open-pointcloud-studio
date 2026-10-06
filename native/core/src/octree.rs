//! Disk-backed octree indexing. Only node metadata, small previews and a
//! bounded set of record blocks are held in memory; point records stay in
//! temporary files.

use std::array;
use std::cell::Cell;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::snapshots::{self, Showing, Snapshots};
use super::{
    e57_points, pcd, visit_points_with_poses, Bounds, Collector, LoadError, Point, PointCloud,
    ScanLog, ScanPose, ScanRange, SourceStamp,
};

const RECORD_BYTES: usize = 40;
const RECORD_BATCH_POINTS: usize = 8_192;
const LEAF_LOD_POINTS: usize = 2_048;
const ROOT_WRITE_BUFFER_BYTES: usize = 4 * 1024 * 1024;
/// Records a node partition reads, classifies and writes as one block.
const PARTITION_BLOCK_RECORDS: usize = 65_536;
/// Blocks a wide partition handles per round, each on its own worker.
const WIDE_ROUND_BLOCKS: usize = 8;
/// Smaller nodes are partitioned one block at a time on a single worker.
const WIDE_NODE_MIN_BLOCKS: u64 = 32;
/// Records that may wait in small nodes when the next large node is split.
const BACKLOG_RECORDS: u64 = 4 * 1024 * 1024;
/// Workers in each of the two build pools; more only contend for the disk.
const MAX_BUILD_THREADS: usize = 16;
const MAX_CLOUD_METADATA_BYTES: u64 = 4 * 1024 * 1024;
const MAX_CACHED_SCAN_POSES: usize = 4_096;

#[derive(Serialize, Deserialize)]
struct CachedCloudHeader {
    version: u8,
    total_points: u64,
    min: [f64; 3],
    max: [f64; 3],
    has_rgb: bool,
    has_intensity: bool,
    has_classification: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scan_poses: Option<Vec<ScanPose>>,
    /// Absent in a cache written before scan ranges were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scan_ranges: Option<Vec<ScanRange>>,
}

/// Scan ranges to keep in a cache, under the same limit as the poses they
/// refer to.
fn cached_scan_ranges(poses: &[ScanPose], ranges: &[ScanRange]) -> Option<Vec<ScanRange>> {
    (poses.len() <= MAX_CACHED_SCAN_POSES && ranges.len() <= MAX_CACHED_SCAN_POSES)
        .then(|| ranges.to_vec())
}

/// Whether cached scan ranges can be searched: within the limit, in ordinal
/// order from the first point on, and inside the cloud.
fn valid_scan_ranges(ranges: &[ScanRange], total_points: u64) -> bool {
    ranges.len() <= MAX_CACHED_SCAN_POSES
        && ranges.first().is_none_or(|range| range.first_ordinal == 0)
        && ranges
            .windows(2)
            .all(|pair| pair[0].first_ordinal <= pair[1].first_ordinal)
        && ranges
            .last()
            .is_none_or(|range| range.first_ordinal <= total_points)
}

/// Give a cloud opened from a cache, with its stations in place, its scan
/// ranges. A cache written before ranges were recorded has none. They are
/// then taken from what is certain without reading the points: the record
/// counts an E57 file states when they add up, and nothing to tell apart in
/// a source with at most one station. Otherwise they stay unknown, and are
/// not written back to the cache, until a pass over the source records them.
fn restore_scan_ranges(
    cloud: &mut PointCloud,
    cached: Option<Vec<ScanRange>>,
) -> Result<(), LoadError> {
    let certain = match cached {
        Some(ranges) => Some(ranges),
        None if super::is_e57(&cloud.path) => {
            e57_points::ranges_for_count(&cloud.path, cloud.total_points)?
        }
        // Every block of a PTX file can have a station of its own.
        None if cloud.scan_poses.len() > 1 => None,
        None => Some(Vec::new()),
    };
    cloud.scan_ranges_known = certain.is_some();
    cloud.scan_ranges = certain.unwrap_or_default();
    Ok(())
}

/// The scan ranges an index cache holds already, for a header that is
/// written again from a cloud that does not know them.
fn kept_scan_ranges(directory: &Path, total_points: u64) -> Option<Vec<ScanRange>> {
    let metadata_path = directory.join("cloud.json");
    if fs::metadata(&metadata_path).ok()?.len() > MAX_CLOUD_METADATA_BYTES {
        return None;
    }
    serde_json::from_slice::<CachedCloudHeader>(&fs::read(metadata_path).ok()?)
        .ok()?
        .scan_ranges
        .filter(|ranges| valid_scan_ranges(ranges, total_points))
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
    /// Points in the source: zero while reading a source that does not state
    /// its count, and the points of the cloud while building the tree.
    pub total: u64,
    pub depth: u8,
    pub leaves: u64,
    /// Points that have reached their leaf while building the tree. Against
    /// `total` this tells how far the build is; `completed` counts a point
    /// once for every level it passes.
    pub settled: u64,
}

/// Share of a tree build that splitting the root takes. No point reaches a
/// leaf before the root is split, so the build is measured by that pass
/// first and by the points in finished leaves after it.
const ROOT_SPLIT_SHARE: f64 = 0.2;

impl IndexProgress {
    /// How far the current stage is, from 0 to 1, when the size of the
    /// source is known.
    pub fn fraction(&self) -> Option<f32> {
        if self.total == 0 {
            return None;
        }
        let of_total = |count: u64| (count as f64 / self.total as f64).min(1.0);
        Some(match self.stage {
            IndexStage::ReadingSource => of_total(self.completed),
            IndexStage::BuildingTree => {
                ROOT_SPLIT_SHARE * of_total(self.completed)
                    + (1.0 - ROOT_SPLIT_SHARE) * of_total(self.settled)
            }
            IndexStage::Ready => 1.0,
        } as f32)
    }

    fn reading(completed: u64, total: u64) -> Self {
        Self {
            stage: IndexStage::ReadingSource,
            completed,
            total,
            depth: 0,
            leaves: 0,
            settled: 0,
        }
    }

    fn building(completed: u64, depth: u8, leaves: u64) -> Self {
        Self {
            stage: IndexStage::BuildingTree,
            completed,
            total: 0,
            depth,
            leaves,
            settled: 0,
        }
    }

    /// With the points that have reached their leaf, of all in the cloud.
    fn with_settled(mut self, settled: u64, total: u64) -> Self {
        self.settled = settled;
        self.total = total;
        self
    }

    fn ready(total: u64, leaves: u64) -> Self {
        Self {
            stage: IndexStage::Ready,
            completed: total,
            total,
            depth: 0,
            leaves,
            settled: total,
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
        progress: impl FnMut(IndexProgress) -> Result<(), LoadError>,
    ) -> Result<Self, LoadError> {
        Self::build_recording_scans(cloud, config, progress).map(|(index, _)| index)
    }

    /// Build the index, and return the scans its pass over the source met:
    /// a cloud that came from a cache may not know where they begin.
    fn build_recording_scans(
        cloud: &PointCloud,
        config: IndexConfig,
        mut progress: impl FnMut(IndexProgress) -> Result<(), LoadError>,
    ) -> Result<(Self, ScanLog), LoadError> {
        if config.leaf_points == 0 || config.preview_points == 0 || config.max_depth == 0 {
            return Err(LoadError::InvalidData(
                "octree limits must be positive".into(),
            ));
        }
        if cloud.provisional {
            return Err(LoadError::InvalidData(
                "the cloud is a preview that was not checked against its source".into(),
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
        let mut scans = ScanLog::default();
        // The scan callback cannot see the count the point callback holds,
        // so the count of points read is kept beside it.
        let read = Cell::new(0u64);
        progress(IndexProgress::reading(0, cloud.total_points))?;
        {
            let mut writer =
                BufWriter::with_capacity(ROOT_WRITE_BUFFER_BYTES, File::create(&root_path)?);
            visit_points_with_poses(
                &cloud.path,
                &mut |point| {
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
                    read.set(root_count);
                    Ok(())
                },
                &mut |pose| scans.begin(read.get(), pose),
            )?;
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
        progress(IndexProgress::building(0, 0, 0).with_settled(0, root_count))?;
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
        Ok((
            Self {
                root,
                storage: IndexStorage::Temporary(storage),
            },
            scans,
        ))
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
        if cloud.provisional {
            return Err(LoadError::InvalidData(
                "the cloud is a preview that was not checked against its source".into(),
            ));
        }
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
                let _ = write_cached_cloud_header(&cache_path, cloud, None);
                progress(IndexProgress::ready(
                    cloud.total_points,
                    count_leaves(&index.root),
                ))?;
                return Ok(index);
            }
            fs::remove_dir_all(&cache_path)?;
        }
        config.scratch_dir = Some(cache_root);
        let (index, scans) = Self::build_recording_scans(cloud, config, &mut progress)?;
        let Self { root, storage } = index;
        let IndexStorage::Temporary(storage) = storage else {
            unreachable!("fresh octree build uses temporary storage")
        };
        fs::write(storage.path().join("source.meta"), &fingerprint)?;
        write_cached_cloud_header(storage.path(), cloud, Some(&scans))?;
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

    /// For explicit pre-indexing, collect the preview and write the octree's
    /// root records during the same source pass. This avoids decoding large
    /// non-LAS files twice before the index is ready.
    pub fn open_and_build_cached_with_progress(
        path: &Path,
        sample_limit: usize,
        config: IndexConfig,
        progress: impl FnMut(IndexProgress) -> Result<(), LoadError>,
    ) -> Result<(PointCloud, Self), LoadError> {
        Self::open_and_build(path, sample_limit, config, None, |_| Ok(()), progress)
    }

    /// Publish a preview while the single source pass and the partitioning of
    /// the octree on disk continue on the same worker. The preview is the
    /// checked cloud once the pass finishes, or one kept from an earlier
    /// open. A source without the latter is shown while it is read, as
    /// `open_with_snapshots` does: `preview` is then called with provisional
    /// clouds that each replace the one before, and with the checked cloud
    /// when the pass ends.
    pub fn open_and_build_cached_with_preview(
        path: &Path,
        sample_limit: usize,
        config: IndexConfig,
        preview: impl FnMut(&PointCloud) -> Result<(), LoadError>,
        progress: impl FnMut(IndexProgress) -> Result<(), LoadError>,
    ) -> Result<(PointCloud, Self), LoadError> {
        Self::open_and_build(
            path,
            sample_limit,
            config,
            Some(Showing::DEFAULT),
            preview,
            progress,
        )
    }

    /// `showing` says from what size the source is shown while it is read.
    pub(crate) fn open_and_build(
        path: &Path,
        sample_limit: usize,
        config: IndexConfig,
        showing: Option<Showing>,
        mut preview: impl FnMut(&PointCloud) -> Result<(), LoadError>,
        mut progress: impl FnMut(IndexProgress) -> Result<(), LoadError>,
    ) -> Result<(PointCloud, Self), LoadError> {
        if sample_limit == 0
            || config.leaf_points == 0
            || config.preview_points == 0
            || config.max_depth == 0
        {
            return Err(LoadError::InvalidData(
                "octree and preview limits must be positive".into(),
            ));
        }
        let stamp = SourceStamp::read(path)?;
        let fingerprint = cache_fingerprint_for(path, stamp, &config)?;
        let cache_root = config.scratch_dir.clone().unwrap_or_else(cache_root);
        fs::create_dir_all(&cache_root)?;
        let cache_path = cache_directory(&cache_root, &fingerprint);
        if cache_path.exists() {
            let cloud = super::open(path, sample_limit)?;
            preview(&cloud)?;
            let index = Self::build_cached_with_progress(&cloud, config, progress)?;
            return Ok((cloud, index));
        }

        // A preview kept from an earlier open is shown before the source pass.
        let previewed = match open_preview_cache(path, sample_limit) {
            Ok(Some(cached)) => {
                preview(&cached)?;
                true
            }
            _ => false,
        };
        // A source otherwise shows nothing until all of it has been read.
        let mut snapshots = match showing.filter(|_| !previewed) {
            Some(showing) => showing.begin(path, stamp, &mut preview)?,
            None => None,
        };
        let dense = snapshots.as_ref().is_some_and(Snapshots::dense);
        let storage = tempfile::Builder::new()
            .prefix("open-pointcloud-index-")
            .tempdir_in(&cache_root)?;
        let root_path = storage.path().join("r.bin");
        let mut collector = Collector::new(if dense {
            Snapshots::sample_limit(sample_limit)
        } else {
            sample_limit
        });
        let mut scans = ScanLog::default();
        // The scan callback cannot look into the collector while the point
        // callback holds it, so the count of points read is kept beside it.
        let read = Cell::new(0u64);
        // A source that states how many points it holds tells the pass how
        // far it is. The records of an E57 file that hold no valid point make
        // the count of points smaller, never larger.
        let stated = snapshots::stated_points(path).unwrap_or(0);
        progress(IndexProgress::reading(0, stated))?;
        {
            let mut writer =
                BufWriter::with_capacity(ROOT_WRITE_BUFFER_BYTES, File::create(&root_path)?);
            visit_points_with_poses(
                path,
                &mut |point| {
                    let ordinal = collector.total;
                    collector.push(point)?;
                    read.set(collector.total);
                    write_record(&mut writer, IndexedPoint { point, ordinal })?;
                    let counted = collector.total.is_multiple_of(65_536);
                    if counted {
                        let total = if stated == 0 {
                            0
                        } else {
                            stated.max(collector.total)
                        };
                        progress(IndexProgress::reading(collector.total, total))?;
                    }
                    if let Some(snapshots) = &mut snapshots {
                        if counted || snapshots.stepped() {
                            snapshots.tick(&collector, &mut preview)?;
                        }
                    }
                    Ok(())
                },
                &mut |pose| scans.begin(read.get(), pose),
            )?;
            writer.flush()?;
        }
        drop(snapshots);
        progress(IndexProgress::reading(collector.total, collector.total))?;
        if SourceStamp::read(path)? != stamp {
            return Err(LoadError::InvalidData(
                "source changed while indexing".into(),
            ));
        }
        let mut cloud = collector.finish(path.to_path_buf())?;
        cloud.scan_poses = scans.poses;
        cloud.scan_ranges = scans.ranges;
        cloud.scan_images = super::scan_images(path);
        cloud.source_stamp = Some(stamp);
        write_preview_cache(&cloud);
        // The checked cloud takes the place of the snapshots before the tree
        // is built, so that it remains when the build fails or is cancelled.
        if !previewed {
            preview(&cloud)?;
        }

        let mut handled_records = 0u64;
        let mut ready_leaves = 0u64;
        progress(IndexProgress::building(0, 0, 0).with_settled(0, cloud.total_points))?;
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
            cloud.total_points,
            0,
            &mut context,
        )?;
        progress(IndexProgress::ready(cloud.total_points, ready_leaves))?;
        fs::write(storage.path().join("source.meta"), &fingerprint)?;
        write_cached_cloud_header(storage.path(), &cloud, None)?;
        let temporary_path = storage.keep();
        if let Err(error) = fs::rename(&temporary_path, &cache_path) {
            let _ = fs::remove_dir_all(&temporary_path);
            if cache_path.exists() {
                let index = Self::open_cached(&cloud, &cache_path, &fingerprint)?;
                return Ok((cloud, index));
            }
            return Err(error.into());
        }
        Ok((
            cloud,
            Self {
                root,
                storage: IndexStorage::Persistent(cache_path),
            },
        ))
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
        let _ = write_cached_cloud_header(&directory, cloud, None);
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

    /// The leaves whose bounds, and those of every node above them, pass a
    /// spatial test, in tree order. With `visit_leaf` a caller knows the size
    /// of a read before it starts and can read leaves on several threads:
    /// reading changes nothing in the index.
    pub fn intersecting_leaves(
        &self,
        mut intersects: impl FnMut(Bounds) -> bool,
    ) -> Vec<&IndexedNode> {
        fn collect<'a>(
            node: &'a IndexedNode,
            intersects: &mut impl FnMut(Bounds) -> bool,
            leaves: &mut Vec<&'a IndexedNode>,
        ) {
            if !intersects(node.bounds) {
                return;
            }
            if node.is_leaf() {
                leaves.push(node);
            } else {
                for child in &node.children {
                    collect(child, intersects, leaves);
                }
            }
        }
        let mut leaves = Vec::new();
        collect(&self.root, &mut intersects, &mut leaves);
        leaves
    }

    /// Visit every exact source point of one leaf of this index, each with
    /// its ordinal in the original file.
    pub fn visit_leaf(
        &self,
        leaf: &IndexedNode,
        mut visit: impl FnMut(IndexedPoint) -> Result<(), LoadError>,
    ) -> Result<(), LoadError> {
        if !leaf.is_leaf() {
            // An inner node holds a preview of the points below it.
            return Err(LoadError::InvalidData(format!(
                "octree node is not a leaf: {}",
                leaf.id
            )));
        }
        let mut count = 0u64;
        read_records(&self.storage.path().join(&leaf.data_path), |point| {
            count += 1;
            visit(point)
        })?;
        if count != leaf.stored_points {
            return Err(LoadError::InvalidData("damaged octree leaf".into()));
        }
        Ok(())
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

/// Recover exact PLY, E57, PCD, PTX or text-cloud metadata and a small preview from an
/// already validated disk index, without decoding the source points again.
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
    if fs::metadata(&metadata_path)?.len() > MAX_CLOUD_METADATA_BYTES {
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
    let is_ptx = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ptx"));
    if is_ptx && header.scan_poses.is_none() {
        // Older manifests cannot recover PTX scanner positions from a cheap
        // header read: every scan block can contain a different pose.
        return Ok(None);
    }
    if let Some(poses) = &header.scan_poses {
        if poses.len() > MAX_CACHED_SCAN_POSES
            || poses.iter().any(|pose| {
                !pose.position.iter().all(|value| value.is_finite())
                    || pose
                        .axes
                        .is_some_and(|axes| !axes.iter().flatten().all(|value| value.is_finite()))
            })
        {
            return Err(LoadError::InvalidData(
                "invalid cached scanner poses".into(),
            ));
        }
    }
    if header
        .scan_ranges
        .as_ref()
        .is_some_and(|ranges| !valid_scan_ranges(ranges, header.total_points))
    {
        return Err(LoadError::InvalidData("invalid cached scan ranges".into()));
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
        scan_poses: if is_ptx {
            header.scan_poses.unwrap_or_default()
        } else {
            Vec::new()
        },
        scan_ranges: Vec::new(),
        scan_images: Vec::new(),
        source_stamp: Some(stamp),
        provisional: false,
        scan_ranges_known: false,
    };
    let index = OctreeIndex::open_cached(&cloud, &directory, &fingerprint)?;
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("e57"))
    {
        cloud.scan_poses = e57_points::scan_poses(path)?;
        cloud.scan_images = super::scan_images(path);
    } else if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pcd"))
    {
        cloud.scan_poses = pcd::scan_poses(path)?;
    }
    restore_scan_ranges(&mut cloud, header.scan_ranges)?;
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

/// Sources smaller than this decode faster than a cache lookup is worth.
const PREVIEW_CACHE_MIN_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct CachedPreviewHeader {
    version: u8,
    total_points: u64,
    min: [f64; 3],
    max: [f64; 3],
    has_rgb: bool,
    has_intensity: bool,
    has_classification: bool,
    points: u64,
    scan_poses: Vec<ScanPose>,
    /// Absent in a cache written before scan ranges were recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scan_ranges: Option<Vec<ScanRange>>,
}

fn preview_cache_directory(root: &Path, fingerprint: &[u8]) -> PathBuf {
    cache_directory(&root.join("previews"), fingerprint)
}

/// Keep the checked preview of a large source so that reopening it unchanged
/// does not decode its points again. Failing to write only costs that time.
pub(crate) fn write_preview_cache(cloud: &PointCloud) {
    if cloud
        .source_stamp
        .is_some_and(|stamp| stamp.length >= PREVIEW_CACHE_MIN_BYTES)
    {
        let _ = write_preview_cache_in(&cache_root(), cloud);
    }
}

fn write_preview_cache_in(root: &Path, cloud: &PointCloud) -> Result<(), LoadError> {
    let fingerprint = cache_fingerprint(cloud, &IndexConfig::default())?;
    let directory = preview_cache_directory(root, &fingerprint);
    fs::create_dir_all(&directory)?;
    let mut points = tempfile::NamedTempFile::new_in(&directory)?;
    {
        let mut writer = BufWriter::new(points.as_file_mut());
        for (point, ordinal) in cloud.points.iter().zip(&cloud.point_ordinals) {
            write_record(
                &mut writer,
                IndexedPoint {
                    point: *point,
                    ordinal: *ordinal,
                },
            )?;
        }
        writer.flush()?;
    }
    points
        .persist(directory.join("points.bin"))
        .map_err(|error| error.error)?;
    let header = CachedPreviewHeader {
        version: 1,
        total_points: cloud.total_points,
        min: cloud.bounds.min,
        max: cloud.bounds.max,
        has_rgb: cloud.has_rgb,
        has_intensity: cloud.has_intensity,
        has_classification: cloud.has_classification,
        points: cloud.points.len().min(cloud.point_ordinals.len()) as u64,
        scan_poses: if cloud.scan_poses.len() <= MAX_CACHED_SCAN_POSES {
            cloud.scan_poses.clone()
        } else {
            Vec::new()
        },
        scan_ranges: cloud
            .scan_ranges_known()
            .then(|| cached_scan_ranges(&cloud.scan_poses, &cloud.scan_ranges))
            .flatten(),
    };
    let serialized = serde_json::to_vec(&header).map_err(|error| {
        LoadError::InvalidData(format!("cannot serialize preview metadata: {error}"))
    })?;
    // The metadata is written last: its presence marks a complete preview.
    let mut metadata = tempfile::NamedTempFile::new_in(&directory)?;
    metadata.as_file_mut().write_all(&serialized)?;
    metadata
        .persist(directory.join("preview.json"))
        .map_err(|error| error.error)?;
    Ok(())
}

/// Recover a large source's checked preview from an earlier open, when the
/// source has not changed since.
pub(crate) fn open_preview_cache(
    path: &Path,
    sample_limit: usize,
) -> Result<Option<PointCloud>, LoadError> {
    if SourceStamp::read(path)?.length < PREVIEW_CACHE_MIN_BYTES {
        return Ok(None);
    }
    open_preview_cache_in(&cache_root(), path, sample_limit)
}

fn open_preview_cache_in(
    root: &Path,
    path: &Path,
    sample_limit: usize,
) -> Result<Option<PointCloud>, LoadError> {
    let stamp = SourceStamp::read(path)?;
    let fingerprint = cache_fingerprint_for(path, stamp, &IndexConfig::default())?;
    let directory = preview_cache_directory(root, &fingerprint);
    let metadata_path = directory.join("preview.json");
    if !metadata_path.exists() {
        return Ok(None);
    }
    if fs::metadata(&metadata_path)?.len() > MAX_CLOUD_METADATA_BYTES {
        return Ok(None);
    }
    let Ok(header) = serde_json::from_slice::<CachedPreviewHeader>(&fs::read(metadata_path)?)
    else {
        return Ok(None);
    };
    let points_path = directory.join("points.bin");
    if header.version != 1
        || header.total_points == 0
        || header.points == 0
        || header.points > header.total_points
        || (0..3).any(|axis| {
            !header.min[axis].is_finite()
                || !header.max[axis].is_finite()
                || header.min[axis] > header.max[axis]
        })
        || header.scan_poses.len() > MAX_CACHED_SCAN_POSES
        || header.scan_poses.iter().any(|pose| {
            !pose.position.iter().all(|value| value.is_finite())
                || pose
                    .axes
                    .is_some_and(|axes| !axes.iter().flatten().all(|value| value.is_finite()))
        })
        || header
            .scan_ranges
            .as_ref()
            .is_some_and(|ranges| !valid_scan_ranges(ranges, header.total_points))
        || count_records(&points_path).ok() != Some(header.points)
    {
        return Ok(None);
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
        scan_poses: header.scan_poses,
        scan_ranges: Vec::new(),
        scan_images: super::scan_images(path),
        source_stamp: Some(stamp),
        provisional: false,
        scan_ranges_known: false,
    };
    // E57 stations come from the file itself, like its photos, and so does
    // the viewpoint of a PCD file, whose header is short.
    if super::is_e57(path) {
        cloud.scan_poses = e57_points::scan_poses(path)?;
    } else if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pcd"))
    {
        cloud.scan_poses = pcd::scan_poses(path)?;
    }
    restore_scan_ranges(&mut cloud, header.scan_ranges)?;
    read_records(&points_path, |record| {
        if cloud.points.len() < sample_limit {
            cloud.points.push(record.point);
            cloud.point_ordinals.push(record.ordinal);
        }
        Ok(())
    })?;
    if SourceStamp::read(path)? != stamp {
        return Err(LoadError::InvalidData(
            "source changed while loading cached preview".into(),
        ));
    }
    Ok(Some(cloud))
}

/// Write the cloud metadata of an index cache. `pass` holds the scans that
/// the build of this index met in the source. Without it the scans are those
/// of the cloud, or what the cache holds already when the cloud came from a
/// cache that did not record them: unknown ranges are never stored as fact.
fn write_cached_cloud_header(
    directory: &Path,
    cloud: &PointCloud,
    pass: Option<&ScanLog>,
) -> Result<(), LoadError> {
    let is_ptx = cloud
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ptx"));
    let (poses, ranges) = match pass {
        Some(scans) => (
            &scans.poses,
            cached_scan_ranges(&scans.poses, &scans.ranges),
        ),
        None if cloud.scan_ranges_known() => (
            &cloud.scan_poses,
            cached_scan_ranges(&cloud.scan_poses, &cloud.scan_ranges),
        ),
        None => (
            &cloud.scan_poses,
            kept_scan_ranges(directory, cloud.total_points),
        ),
    };
    let mut header = CachedCloudHeader {
        version: 1,
        total_points: cloud.total_points,
        min: cloud.bounds.min,
        max: cloud.bounds.max,
        has_rgb: cloud.has_rgb,
        has_intensity: cloud.has_intensity,
        has_classification: cloud.has_classification,
        scan_poses: (is_ptx && poses.len() <= MAX_CACHED_SCAN_POSES).then(|| poses.clone()),
        scan_ranges: ranges,
    };
    let mut serialized = serde_json::to_vec(&header).map_err(|error| {
        LoadError::InvalidData(format!("cannot serialize octree metadata: {error}"))
    })?;
    if serialized.len() as u64 > MAX_CLOUD_METADATA_BYTES {
        header.scan_poses = None;
        header.scan_ranges = None;
        serialized = serde_json::to_vec(&header).map_err(|error| {
            LoadError::InvalidData(format!("cannot serialize octree metadata: {error}"))
        })?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.as_file_mut().write_all(&serialized)?;
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(directory.join("cloud.json"))
        .map_err(|error| error.error)?;
    Ok(())
}

/// Where disk indexes are kept: `XDG_CACHE_HOME` when set, otherwise the
/// local application data folder on Windows and `~/.cache` elsewhere. The
/// temporary directory is the last resort because the system may empty it.
fn cache_root() -> PathBuf {
    let set = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if let Some(root) = set("XDG_CACHE_HOME") {
        return root.join("open-pointcloud-studio/indexes");
    }
    if cfg!(windows) {
        if let Some(local) = set("LOCALAPPDATA") {
            return local.join("open-pointcloud-studio/indexes");
        }
    }
    if let Some(home) = set("HOME") {
        return home.join(".cache/open-pointcloud-studio/indexes");
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

/// Block and round sizes of a node partition. Tests shrink them so that a
/// small cloud still spans many blocks and rounds.
#[derive(Clone, Copy)]
struct PartitionTuning {
    block_records: usize,
    round_blocks: usize,
    wide_node_blocks: u64,
    backlog_records: u64,
}

impl Default for PartitionTuning {
    fn default() -> Self {
        Self {
            block_records: PARTITION_BLOCK_RECORDS,
            round_blocks: WIDE_ROUND_BLOCKS,
            wide_node_blocks: WIDE_NODE_MIN_BLOCKS,
            backlog_records: BACKLOG_RECORDS,
        }
    }
}

/// Index builds share two small pools and leave the global one free. Each
/// worker of the node pool has at most one small node open, and the wide
/// pool runs the blocks of one large node at a time, so the pool sizes bound
/// the open files and block buffers however many builds run.
fn build_pool(wide: bool) -> Result<&'static rayon::ThreadPool, LoadError> {
    static POOLS: [OnceLock<Result<rayon::ThreadPool, String>>; 2] =
        [OnceLock::new(), OnceLock::new()];
    POOLS[usize::from(wide)]
        .get_or_init(|| {
            let threads = std::thread::available_parallelism()
                .map_or(1, usize::from)
                .min(MAX_BUILD_THREADS);
            let kind = if wide { "wide" } else { "node" };
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(move |index| format!("octree-{kind}-{index}"))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| LoadError::Io(std::io::Error::other(error.clone())))
}

struct NodeJob {
    id: String,
    input_path: PathBuf,
    bounds: Bounds,
    count: u64,
    depth: u8,
}

struct BuiltNode {
    bounds: Bounds,
    total_points: u64,
    stored_points: u64,
    depth: u8,
    leaf: bool,
}

#[derive(Clone, Copy)]
struct RoundLayout {
    center: [f64; 3],
    length: u64,
    block_bytes: usize,
    wide: bool,
}

/// One block of a round: its own handle on the node's input and the raw
/// records it read.
struct Block {
    reader: File,
    records: Vec<u8>,
    octants: Vec<u8>,
}

impl Block {
    fn load(&mut self, offset: u64, bytes: usize) -> Result<(), LoadError> {
        self.records.resize(bytes, 0);
        if bytes > 0 {
            self.reader.seek(SeekFrom::Start(offset))?;
            self.reader.read_exact(&mut self.records)?;
        }
        Ok(())
    }

    fn record(&self, index: usize) -> &[u8] {
        &self.records[index * RECORD_BYTES..(index + 1) * RECORD_BYTES]
    }
}

/// A block's records grouped by octant, in input order within each octant.
struct Sorted {
    bytes: Vec<u8>,
    ends: [usize; 8],
}

impl Sorted {
    fn segment(&self, child: usize) -> &[u8] {
        let start = if child == 0 { 0 } else { self.ends[child - 1] };
        &self.bytes[start..self.ends[child]]
    }
}

/// Only the coordinates are decoded; the records are copied as stored.
fn classify(center: [f64; 3], records: &[u8], octants: &mut Vec<u8>, sorted: &mut Sorted) {
    let (records, _) = records.as_chunks::<RECORD_BYTES>();
    let mut next = [0usize; 8];
    octants.clear();
    octants.extend(records.iter().map(|record| {
        let xyz = array::from_fn(|axis| {
            let start = axis * 8;
            f64::from_le_bytes(record[start..start + 8].try_into().unwrap())
        });
        let index = octant(xyz, center);
        next[index] += 1;
        index as u8
    }));
    let mut start = 0;
    for slot in &mut next {
        start += std::mem::replace(slot, start);
    }
    sorted.bytes.resize(records.len() * RECORD_BYTES, 0);
    let (grouped, _) = sorted.bytes.as_chunks_mut::<RECORD_BYTES>();
    for (record, index) in records.iter().zip(octants.iter()) {
        let slot = &mut next[usize::from(*index)];
        grouped[*slot] = *record;
        *slot += 1;
    }
    sorted.ends = next.map(|end| end * RECORD_BYTES);
}

/// The node preview: the first `limit` records, after which every record
/// replaces a pseudo-randomly chosen one with a probability of `limit / seen`.
struct Reservoir {
    records: Vec<u8>,
    limit: usize,
    seen: u64,
    random_state: u64,
    appended: usize,
    replaced: Vec<(usize, usize)>,
}

impl Reservoir {
    fn new(limit: usize, count: u64, depth: u8) -> Self {
        Self {
            records: Vec::with_capacity((limit as u64).min(count) as usize * RECORD_BYTES),
            limit,
            seen: 0,
            random_state: 0x9e37_79b9_7f4a_7c15u64 ^ (count << (depth % 32)),
            appended: 0,
            replaced: Vec::new(),
        }
    }

    /// Decide which of the next `records` records enter the preview. Only
    /// their positions matter here, so this can run while they are read.
    fn draw(&mut self, records: usize) {
        self.replaced.clear();
        self.appended = (self.limit as u64)
            .saturating_sub(self.seen)
            .min(records as u64) as usize;
        let limit = self.limit as u64;
        let mut seen = self.seen + self.appended as u64;
        let mut random_state = self.random_state;
        for index in self.appended..records {
            random_state ^= random_state << 13;
            random_state ^= random_state >> 7;
            random_state ^= random_state << 17;
            seen += 1;
            let chosen = random_state % seen;
            if chosen < limit {
                self.replaced.push((index, chosen as usize));
            }
        }
        self.seen = seen;
        self.random_state = random_state;
    }

    /// Copy the drawn records, looked up by their position in the round.
    fn copy_drawn<'a>(&mut self, record: impl Fn(usize) -> &'a [u8]) {
        for index in 0..self.appended {
            self.records.extend_from_slice(record(index));
        }
        for &(index, slot) in &self.replaced {
            self.records[slot * RECORD_BYTES..(slot + 1) * RECORD_BYTES]
                .copy_from_slice(record(index));
        }
    }
}

/// State shared by the workers of one tree build.
struct TreeBuild<'a> {
    directory: &'a Path,
    config: &'a IndexConfig,
    tuning: PartitionTuning,
    handled_records: AtomicU64,
    ready_leaves: AtomicU64,
    /// Points in finished leaves.
    settled: AtomicU64,
    aborted: AtomicBool,
    failure: Mutex<Option<LoadError>>,
    nodes: Mutex<HashMap<String, BuiltNode>>,
    /// Records in small nodes that are waiting for a worker or being built.
    backlog: Mutex<u64>,
    backlog_changed: Condvar,
}

impl TreeBuild<'_> {
    /// Keep the first error and make every worker stop at its next block.
    fn fail(&self, error: LoadError) {
        let mut failure = self.failure.lock().unwrap();
        if failure.is_none() {
            *failure = Some(error);
        }
        self.aborted.store(true, Ordering::Release);
        drop(failure);
        let _backlog = self.backlog.lock().unwrap();
        self.backlog_changed.notify_all();
    }

    fn aborted(&self) -> bool {
        self.aborted.load(Ordering::Acquire)
    }

    fn check(&self) -> Result<(), LoadError> {
        if self.aborted() {
            return Err(LoadError::Cancelled);
        }
        Ok(())
    }

    fn is_large(&self, job: &NodeJob) -> bool {
        job.count.div_ceil(self.tuning.block_records as u64) >= self.tuning.wide_node_blocks
    }

    /// Large nodes are split one after another, depth first and with a wide
    /// partition, while the node pool works on their small subtrees. Files
    /// are then read back soon after they are written, and little data is in
    /// flight at any time.
    fn descend<'scope>(
        &'scope self,
        scope: &rayon::Scope<'scope>,
        job: NodeJob,
        events: &mpsc::Sender<u8>,
    ) -> Result<(), LoadError> {
        if !self.is_large(&job) {
            *self.backlog.lock().unwrap() += job.count;
            let events = events.clone();
            scope.spawn(move |scope| self.run(scope, job, events));
            return Ok(());
        }
        // Let the node pool catch up before more small nodes are written.
        let mut backlog = self.backlog.lock().unwrap();
        while *backlog > self.tuning.backlog_records && !self.aborted() {
            backlog = self.backlog_changed.wait(backlog).unwrap();
        }
        drop(backlog);
        let (small, large): (Vec<_>, Vec<_>) = self
            .build(job, true, events)?
            .into_iter()
            .partition(|child| !self.is_large(child));
        for child in small.into_iter().chain(large) {
            self.descend(scope, child, events)?;
        }
        Ok(())
    }

    /// A small subtree on the node pool. Children are spawned rather than
    /// awaited, so a worker never holds a node open while it picks up
    /// another one.
    fn run<'scope>(
        &'scope self,
        scope: &rayon::Scope<'scope>,
        job: NodeJob,
        events: mpsc::Sender<u8>,
    ) {
        let count = job.count;
        match self.build(job, false, &events) {
            Ok(children) => {
                let added: u64 = children.iter().map(|child| child.count).sum();
                let mut backlog = self.backlog.lock().unwrap();
                *backlog = (*backlog + added).saturating_sub(count);
                drop(backlog);
                self.backlog_changed.notify_all();
                for child in children {
                    let events = events.clone();
                    scope.spawn(move |scope| self.run(scope, child, events));
                }
            }
            Err(error) => self.fail(error),
        }
    }

    /// Finish a leaf, or split an inner node and return its children.
    fn build(
        &self,
        job: NodeJob,
        wide: bool,
        events: &mpsc::Sender<u8>,
    ) -> Result<Vec<NodeJob>, LoadError> {
        self.check()?;
        let leaf = job.count <= self.config.leaf_points
            || job.depth >= self.config.max_depth
            || job.bounds.extent() <= f64::EPSILON;
        let mut children = Vec::new();
        let stored_points = if leaf {
            if job.count > (LEAF_LOD_POINTS * 4) as u64 {
                ensure_leaf_lod_where(
                    &job.input_path,
                    &leaf_lod_path(self.directory, &job.id),
                    job.count,
                    &|| self.aborted(),
                )?;
            }
            self.handled_records.fetch_add(job.count, Ordering::AcqRel);
            self.ready_leaves.fetch_add(1, Ordering::AcqRel);
            self.settled.fetch_add(job.count, Ordering::AcqRel);
            job.count
        } else {
            let (child_counts, stored_points) = self.partition(&job, wide, events)?;
            for (index, count) in child_counts.into_iter().enumerate() {
                if count == 0 {
                    continue;
                }
                let id = format!("{}{index}", job.id);
                children.push(NodeJob {
                    input_path: self.directory.join(format!("{id}.bin")),
                    id,
                    bounds: child_bounds(job.bounds, index),
                    count,
                    depth: job.depth + 1,
                });
            }
            stored_points
        };
        self.nodes.lock().unwrap().insert(
            job.id,
            BuiltNode {
                bounds: job.bounds,
                total_points: job.count,
                stored_points,
                depth: job.depth,
                leaf,
            },
        );
        let _ = events.send(job.depth);
        Ok(children)
    }

    /// Split an inner node: one block at a time on the current thread, or
    /// with several blocks per round on the wide pool.
    fn partition(
        &self,
        job: &NodeJob,
        wide: bool,
        events: &mpsc::Sender<u8>,
    ) -> Result<([u64; 8], u64), LoadError> {
        let tuning = self.tuning;
        let length = fs::metadata(&job.input_path)?.len();
        if !length.is_multiple_of(RECORD_BYTES as u64) {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
        }
        let layout = RoundLayout {
            center: job.bounds.center(),
            length,
            block_bytes: tuning.block_records * RECORD_BYTES,
            wide,
        };
        if !wide {
            return self.split(job, layout, events);
        }
        // One wide partition at a time, across all builds.
        static WIDE_TURN: Mutex<()> = Mutex::new(());
        let _turn = WIDE_TURN.lock().unwrap_or_else(PoisonError::into_inner);
        build_pool(true)?.install(|| self.split(job, layout, events))
    }

    /// Distribute a node's records over its child files and write its
    /// preview. Records keep their input order within each child, whatever
    /// the block and round sizes are.
    fn split(
        &self,
        job: &NodeJob,
        layout: RoundLayout,
        events: &mpsc::Sender<u8>,
    ) -> Result<([u64; 8], u64), LoadError> {
        let tuning = self.tuning;
        let RoundLayout {
            length,
            block_bytes,
            wide,
            ..
        } = layout;
        let width = if wide { tuning.round_blocks } else { 1 };
        let rounds = length.div_ceil((width * block_bytes) as u64);
        let capacity = length.min(block_bytes as u64) as usize;
        let mut blocks = (0..width)
            .map(|_| {
                Ok(Block {
                    reader: File::open(&job.input_path)?,
                    records: Vec::with_capacity(capacity),
                    octants: Vec::with_capacity(capacity / RECORD_BYTES),
                })
            })
            .collect::<Result<Vec<_>, LoadError>>()?;
        // A wide partition fills its next round while the current one is
        // written, which takes a second set of grouped blocks.
        let mut sorted: Vec<Vec<Sorted>> = (0..if wide { 2 } else { 1 })
            .map(|_| {
                (0..width)
                    .map(|_| Sorted {
                        bytes: Vec::with_capacity(capacity),
                        ends: [0; 8],
                    })
                    .collect()
            })
            .collect();
        let mut reservoir = Reservoir::new(self.config.preview_points, job.count, job.depth);
        let mut writers: [Option<File>; 8] = array::from_fn(|_| None);
        let mut child_counts = [0u64; 8];

        let mut records = 0;
        if rounds > 0 {
            records = self.fill_round(layout, 0, &mut blocks, &mut sorted[0], &mut reservoir)?;
        }
        for round in 0..rounds {
            let current = (round % sorted.len() as u64) as usize;
            reservoir.copy_drawn(|index| {
                blocks[index / tuning.block_records].record(index % tuning.block_records)
            });
            for (child, count) in child_counts.iter_mut().enumerate() {
                let bytes: usize = sorted[current]
                    .iter()
                    .map(|block| block.segment(child).len())
                    .sum();
                *count += (bytes / RECORD_BYTES) as u64;
            }
            self.handled_records
                .fetch_add(records as u64, Ordering::AcqRel);
            let _ = events.send(job.depth);

            let last = round + 1 == rounds;
            if wide && !last {
                let (even, odd) = sorted.split_at_mut(1);
                let (current, next) = if current == 0 {
                    (&even[0], &mut odd[0])
                } else {
                    (&odd[0], &mut even[0])
                };
                let (written, filled) = rayon::join(
                    || self.write_round(&job.id, &mut writers, current, wide),
                    || self.fill_round(layout, round + 1, &mut blocks, next, &mut reservoir),
                );
                written?;
                records = filled?;
            } else {
                self.write_round(&job.id, &mut writers, &sorted[current], wide)?;
                if !last {
                    let next = &mut sorted[current];
                    records =
                        self.fill_round(layout, round + 1, &mut blocks, next, &mut reservoir)?;
                }
            }
        }
        drop(writers);
        drop(blocks);

        let preview_path = self.directory.join(format!("{}-preview.bin", job.id));
        fs::write(preview_path, &reservoir.records)?;
        fs::remove_file(&job.input_path)?;
        Ok((
            child_counts,
            (reservoir.records.len() / RECORD_BYTES) as u64,
        ))
    }

    /// Read and classify one round of blocks and draw its preview records.
    fn fill_round(
        &self,
        layout: RoundLayout,
        round: u64,
        blocks: &mut [Block],
        sorted: &mut [Sorted],
        reservoir: &mut Reservoir,
    ) -> Result<usize, LoadError> {
        let capacity = (blocks.len() * layout.block_bytes) as u64;
        let start = round * capacity;
        let round_bytes = (layout.length - start).min(capacity) as usize;
        let records = round_bytes / RECORD_BYTES;
        let load = |(slot, (block, sorted)): (usize, (&mut Block, &mut Sorted))| {
            self.check()?;
            let skipped = (slot * layout.block_bytes).min(round_bytes);
            let bytes = (round_bytes - skipped).min(layout.block_bytes);
            block.load(start + skipped as u64, bytes)?;
            classify(layout.center, &block.records, &mut block.octants, sorted);
            Ok::<(), LoadError>(())
        };
        if layout.wide {
            let (loaded, ()) = rayon::join(
                || {
                    blocks
                        .par_iter_mut()
                        .zip(sorted.par_iter_mut())
                        .enumerate()
                        .try_for_each(load)
                },
                || reservoir.draw(records),
            );
            loaded?;
        } else {
            blocks
                .iter_mut()
                .zip(sorted.iter_mut())
                .enumerate()
                .try_for_each(load)?;
            reservoir.draw(records);
        }
        Ok(records)
    }

    /// Append every child's records of a round to its file, in block order.
    fn write_round(
        &self,
        id: &str,
        writers: &mut [Option<File>; 8],
        sorted: &[Sorted],
        wide: bool,
    ) -> Result<(), LoadError> {
        let write = |(child, writer): (usize, &mut Option<File>)| {
            for block in sorted {
                let segment = block.segment(child);
                if segment.is_empty() {
                    continue;
                }
                let file = match writer {
                    Some(file) => file,
                    None => {
                        let path = self.directory.join(format!("{id}{child}.bin"));
                        writer.insert(File::create(path)?)
                    }
                };
                file.write_all(segment)?;
            }
            Ok::<(), LoadError>(())
        };
        if wide {
            writers.par_iter_mut().enumerate().try_for_each(write)
        } else {
            writers.iter_mut().enumerate().try_for_each(write)
        }
    }
}

fn assemble_node(nodes: &mut HashMap<String, BuiltNode>, id: String) -> Option<IndexedNode> {
    let node = nodes.remove(&id)?;
    let (children, data_path) = if node.leaf {
        (Vec::new(), format!("{id}.bin"))
    } else {
        let children = (0..8)
            .filter_map(|octant| assemble_node(nodes, format!("{id}{octant}")))
            .collect();
        (children, format!("{id}-preview.bin"))
    };
    Some(IndexedNode {
        id,
        bounds: node.bounds,
        total_points: node.total_points,
        stored_points: node.stored_points,
        depth: node.depth,
        children,
        data_path: PathBuf::from(data_path),
    })
}

fn build_node<F: FnMut(IndexProgress) -> Result<(), LoadError>>(
    id: String,
    input_path: PathBuf,
    bounds: Bounds,
    count: u64,
    depth: u8,
    context: &mut BuildContext<'_, F>,
) -> Result<IndexedNode, LoadError> {
    let job = NodeJob {
        id,
        input_path,
        bounds,
        count,
        depth,
    };
    build_tree(job, PartitionTuning::default(), context)
}

/// Build the subtree of `job` on the build pools. The progress callback is
/// not required to be `Send`, so it stays on the calling thread, which
/// reports what the workers have handled each time one of them signals.
fn build_tree<F: FnMut(IndexProgress) -> Result<(), LoadError>>(
    job: NodeJob,
    tuning: PartitionTuning,
    context: &mut BuildContext<'_, F>,
) -> Result<IndexedNode, LoadError> {
    let pool = build_pool(false)?;
    let build = TreeBuild {
        directory: context.directory,
        config: context.config,
        tuning,
        handled_records: AtomicU64::new(*context.handled_records),
        ready_leaves: AtomicU64::new(*context.ready_leaves),
        settled: AtomicU64::new(0),
        aborted: AtomicBool::new(false),
        failure: Mutex::new(None),
        nodes: Mutex::new(HashMap::new()),
        backlog: Mutex::new(0),
        backlog_changed: Condvar::new(),
    };
    let root_id = job.id.clone();
    let total = job.count;
    let (events, updates) = mpsc::channel();
    std::thread::scope(|threads| {
        let build = &build;
        std::thread::Builder::new().spawn_scoped(threads, move || {
            pool.in_place_scope(|scope| {
                if let Err(error) = build.descend(scope, job, &events) {
                    build.fail(error);
                }
            });
        })?;
        // The channel closes once the last node has finished or given up.
        for mut depth in &updates {
            if build.aborted() {
                continue;
            }
            while let Ok(newer) = updates.try_recv() {
                depth = newer;
            }
            let update = IndexProgress::building(
                build.handled_records.load(Ordering::Acquire),
                depth,
                build.ready_leaves.load(Ordering::Acquire),
            )
            .with_settled(build.settled.load(Ordering::Acquire), total);
            if let Err(error) = (context.progress)(update) {
                build.fail(error);
            }
        }
        Ok::<(), LoadError>(())
    })?;
    *context.handled_records = build.handled_records.into_inner();
    *context.ready_leaves = build.ready_leaves.into_inner();
    if let Some(error) = build.failure.into_inner().unwrap() {
        return Err(error);
    }
    let mut nodes = build.nodes.into_inner().unwrap();
    assemble_node(&mut nodes, root_id)
        .filter(|_| nodes.is_empty())
        .ok_or_else(|| LoadError::InvalidData("incomplete octree build".into()))
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

    /// The record-by-record partition on one thread. The block-parallel
    /// build has to produce the same files and the same tree.
    fn reference_node(
        directory: &Path,
        config: &IndexConfig,
        id: String,
        input_path: PathBuf,
        bounds: Bounds,
        count: u64,
        depth: u8,
    ) -> Result<IndexedNode, LoadError> {
        if count <= config.leaf_points
            || depth >= config.max_depth
            || bounds.extent() <= f64::EPSILON
        {
            if count > (LEAF_LOD_POINTS * 4) as u64 {
                let preview = leaf_lod_path(directory, &id);
                ensure_leaf_lod_where(&input_path, &preview, count, &|| false)?;
            }
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
            children.push(reference_node(
                directory,
                config,
                child_id,
                child_path,
                child_bounds(bounds, index),
                child_count,
                depth + 1,
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

    /// Write pseudo-random root records and return their bounds. A clustered
    /// cloud keeps most points near one corner and repeats one coordinate
    /// often enough to reach the depth limit with a large leaf.
    fn write_root_records(path: &Path, count: u64, clustered: bool) -> Bounds {
        let mut state = 0x2545_f491_4f6c_dd1du64 ^ count;
        let mut unit = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut bounds: Option<Bounds> = None;
        let mut writer = BufWriter::new(File::create(path).unwrap());
        for ordinal in 0..count {
            let xyz = if !clustered || ordinal % 16 == 0 {
                [unit() * 100.0 - 50.0, unit() * 80.0, unit() * 30.0]
            } else if ordinal % 16 == 1 {
                [41.25, 72.5, 27.75]
            } else {
                [
                    50.0 - unit() * unit() * 12.0,
                    80.0 - unit() * unit() * 9.0,
                    30.0 - unit() * unit() * 4.0,
                ]
            };
            match &mut bounds {
                Some(bounds) => bounds.include(xyz),
                None => bounds = Some(Bounds { min: xyz, max: xyz }),
            }
            let point = Point {
                xyz,
                rgb: (ordinal % 3 != 0).then_some([ordinal as u8, (ordinal >> 8) as u8, 7]),
                intensity: (ordinal % 5 != 0).then_some((ordinal % 65_521) as u16),
                classification: (ordinal % 7 == 0).then_some((ordinal % 31) as u8),
            };
            write_record(&mut writer, IndexedPoint { point, ordinal }).unwrap();
        }
        writer.flush().unwrap();
        bounds.unwrap()
    }

    fn file_names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn assert_same_tree(built: &IndexedNode, reference: &IndexedNode, case: &str) {
        let id = &reference.id;
        assert_eq!(&built.id, id, "{case}");
        assert_eq!(built.bounds, reference.bounds, "{case}: {id}");
        assert_eq!(built.total_points, reference.total_points, "{case}: {id}");
        assert_eq!(built.stored_points, reference.stored_points, "{case}: {id}");
        assert_eq!(built.depth, reference.depth, "{case}: {id}");
        assert_eq!(built.data_path, reference.data_path, "{case}: {id}");
        assert_eq!(
            built.children.len(),
            reference.children.len(),
            "{case}: {id}"
        );
        for (built, reference) in built.children.iter().zip(&reference.children) {
            assert_same_tree(built, reference, case);
        }
    }

    fn handled_by(node: &IndexedNode) -> u64 {
        node.total_points + node.children.iter().map(handled_by).sum::<u64>()
    }

    #[test]
    fn block_parallel_build_matches_the_sequential_reference() {
        // Every inner node wide, every node on the pool, and a mix in which
        // large nodes wait for the small subtrees to catch up.
        let tunings = [
            ("default", PartitionTuning::default()),
            (
                "wide rounds",
                PartitionTuning {
                    block_records: 257,
                    round_blocks: 5,
                    wide_node_blocks: 2,
                    backlog_records: 0,
                },
            ),
            (
                "single blocks",
                PartitionTuning {
                    block_records: 1_000,
                    round_blocks: 4,
                    wide_node_blocks: u64::MAX,
                    backlog_records: 0,
                },
            ),
            (
                "large and small nodes",
                PartitionTuning {
                    block_records: 64,
                    round_blocks: 16,
                    wide_node_blocks: 40,
                    backlog_records: 4_000,
                },
            ),
        ];
        let count = 300_000;
        for clustered in [false, true] {
            // The larger preview is filled across several blocks and rounds.
            let config = IndexConfig {
                leaf_points: 1_500,
                preview_points: if clustered { 1_400 } else { 37 },
                max_depth: 7,
                scratch_dir: None,
            };
            let source = tempfile::tempdir().unwrap();
            let records = source.path().join("records.bin");
            let bounds = write_root_records(&records, count, clustered);

            let expected = tempfile::tempdir().unwrap();
            let root_path = expected.path().join("r.bin");
            fs::copy(&records, &root_path).unwrap();
            let reference = reference_node(
                expected.path(),
                &config,
                "r".to_owned(),
                root_path,
                bounds,
                count,
                0,
            )
            .unwrap();
            let names = file_names(expected.path());
            assert!(reference.children.iter().any(|child| !child.is_leaf()));
            assert_eq!(
                names.iter().any(|name| name.ends_with("-lod.bin")),
                clustered
            );

            for (name, tuning) in tunings {
                let case = format!("{name}, clustered: {clustered}");
                let directory = tempfile::tempdir().unwrap();
                let root_path = directory.path().join("r.bin");
                fs::copy(&records, &root_path).unwrap();
                let mut handled_records = 0;
                let mut ready_leaves = 0;
                let mut updates = Vec::new();
                let mut progress = |update: IndexProgress| {
                    updates.push(update);
                    Ok(())
                };
                let mut context = BuildContext {
                    directory: directory.path(),
                    config: &config,
                    handled_records: &mut handled_records,
                    ready_leaves: &mut ready_leaves,
                    progress: &mut progress,
                };
                let job = NodeJob {
                    id: "r".to_owned(),
                    input_path: root_path,
                    bounds,
                    count,
                    depth: 0,
                };
                let built = build_tree(job, tuning, &mut context).unwrap();

                assert_same_tree(&built, &reference, &case);
                assert_eq!(file_names(directory.path()), names, "{case}");
                for name in &names {
                    assert!(
                        fs::read(directory.path().join(name)).unwrap()
                            == fs::read(expected.path().join(name)).unwrap(),
                        "{case}: {name} differs"
                    );
                }
                assert_eq!(handled_records, handled_by(&reference), "{case}");
                assert_eq!(ready_leaves, count_leaves(&reference), "{case}");
                assert!(
                    updates
                        .iter()
                        .all(|update| update.stage == IndexStage::BuildingTree)
                        && updates.windows(2).all(|pair| {
                            pair[0].completed <= pair[1].completed
                                && pair[0].leaves <= pair[1].leaves
                        }),
                    "{case}"
                );
                let last = updates.last().unwrap();
                assert_eq!(
                    (last.completed, last.leaves),
                    (handled_records, ready_leaves),
                    "{case}"
                );
            }
        }
    }

    #[test]
    fn failed_progress_stops_the_workers_and_keeps_the_first_error() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("cloud.xyz");
        let mut contents = String::new();
        for index in 0..150_000u64 {
            let (x, y, z) = (index % 97, (index / 97) % 89, (index * 7_919) % 53);
            contents.push_str(&format!("{x} {y} {z}\n"));
        }
        fs::write(&source, contents).unwrap();
        let cloud = super::super::open(&source, 8).unwrap();
        let cache = directory.path().join("cache");
        let config = IndexConfig {
            leaf_points: 512,
            preview_points: 64,
            max_depth: 8,
            scratch_dir: Some(cache.clone()),
        };
        let mut calls_after_failure = 0;
        let mut failed = false;
        let result = OctreeIndex::build_cached_with_progress(&cloud, config.clone(), |update| {
            if failed {
                calls_after_failure += 1;
            }
            if update.stage == IndexStage::BuildingTree && update.completed > 0 {
                failed = true;
                return Err(LoadError::InvalidData("stopped by the caller".into()));
            }
            Ok(())
        });
        assert!(matches!(
            result,
            Err(LoadError::InvalidData(reason)) if reason == "stopped by the caller"
        ));
        assert_eq!(calls_after_failure, 0);
        assert!(file_names(&cache).is_empty());
        assert!(OctreeIndex::open_cached_if_present(&cloud, config.clone())
            .unwrap()
            .is_none());

        // Once a build has failed, no worker reads or writes another block.
        let nodes = directory.path().join("nodes");
        fs::create_dir(&nodes).unwrap();
        let bounds = write_root_records(&nodes.join("r.bin"), 20_000, false);
        let build = TreeBuild {
            directory: &nodes,
            config: &config,
            tuning: PartitionTuning::default(),
            handled_records: AtomicU64::new(0),
            ready_leaves: AtomicU64::new(0),
            settled: AtomicU64::new(0),
            aborted: AtomicBool::new(false),
            failure: Mutex::new(None),
            nodes: Mutex::new(HashMap::new()),
            backlog: Mutex::new(0),
            backlog_changed: Condvar::new(),
        };
        build.fail(LoadError::InvalidData("first".into()));
        build.fail(LoadError::InvalidData("second".into()));
        let job = || NodeJob {
            id: "r".to_owned(),
            input_path: nodes.join("r.bin"),
            bounds,
            count: 20_000,
            depth: 0,
        };
        let (events, updates) = mpsc::channel();
        for wide in [false, true] {
            assert!(matches!(
                build.partition(&job(), wide, &events),
                Err(LoadError::Cancelled)
            ));
            assert!(matches!(
                build.build(job(), wide, &events),
                Err(LoadError::Cancelled)
            ));
        }
        assert!(updates.try_recv().is_err());
        assert_eq!(file_names(&nodes), ["r.bin"]);
        assert_eq!(build.handled_records.into_inner(), 0);
        assert!(matches!(
            build.failure.into_inner().unwrap(),
            Some(LoadError::InvalidData(reason)) if reason == "first"
        ));
    }

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
    fn progress_fraction_follows_the_root_split_and_then_the_leaves() {
        assert_eq!(IndexProgress::reading(5, 0).fraction(), None);
        assert_eq!(IndexProgress::reading(25, 100).fraction(), Some(0.25));
        assert_eq!(IndexProgress::reading(250, 100).fraction(), Some(1.0));
        let building = |completed, settled| {
            IndexProgress::building(completed, 1, 0)
                .with_settled(settled, 100)
                .fraction()
                .unwrap()
        };
        assert_eq!(IndexProgress::building(0, 0, 0).fraction(), None);
        assert!((building(50, 0) - 0.1).abs() < 1e-6);
        assert!((building(100, 0) - 0.2).abs() < 1e-6);
        assert!((building(340, 50) - 0.6).abs() < 1e-6);
        assert!((building(700, 100) - 1.0).abs() < 1e-6);
        assert_eq!(IndexProgress::ready(100, 4).fraction(), Some(1.0));
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

    #[test]
    fn e57_header_and_preview_cache_open_without_decoding_points() {
        use e57::{E57Writer, Quaternion, Record, RecordValue, Transform, Translation};

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("turned-scan.e57");
        let mut writer = E57Writer::new(
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&source)
                .unwrap(),
            "{00000000-0000-4000-8000-000000000011}",
        )
        .unwrap();
        {
            let mut scan = writer
                .add_pointcloud(
                    "{00000000-0000-4000-8000-000000000012}",
                    vec![
                        Record::CARTESIAN_X_F64,
                        Record::CARTESIAN_Y_F64,
                        Record::CARTESIAN_Z_F64,
                    ],
                )
                .unwrap();
            scan.set_name(Some("Turned station".into()));
            // A quarter turn about the vertical: local +X points along +Y.
            let half = std::f64::consts::FRAC_1_SQRT_2;
            scan.set_transform(Some(Transform {
                rotation: Quaternion {
                    w: half,
                    x: 0.0,
                    y: 0.0,
                    z: half,
                },
                translation: Translation {
                    x: 10.0,
                    y: 20.0,
                    z: 3.0,
                },
            }));
            for x in [1.0, 2.0, 3.0, 4.0] {
                scan.add_point(vec![
                    RecordValue::Double(x),
                    RecordValue::Double(0.0),
                    RecordValue::Double(0.5),
                ])
                .unwrap();
            }
            scan.finalize().unwrap();
        }
        writer.finalize().unwrap();

        let exact = super::super::open(&source, 3).unwrap();
        let header = super::super::open_e57_header(&source).unwrap();
        assert!(header.points.is_empty());
        assert_eq!(header.total_points, 4);
        assert_eq!(header.scan_poses, exact.scan_poses);
        // The stated extent, placed with the scan pose, encloses every point.
        for axis in 0..3 {
            assert!(header.bounds.min[axis] <= exact.bounds.min[axis] + 1e-9);
            assert!(header.bounds.max[axis] >= exact.bounds.max[axis] - 1e-9);
            assert!(header.bounds.max[axis] - header.bounds.min[axis] < 3.1);
        }
        assert!(super::super::open_e57_header(directory.path().join("scan.xyz")).is_err());

        let root = directory.path().join("cache");
        assert!(open_preview_cache_in(&root, &source, 3).unwrap().is_none());
        write_preview_cache_in(&root, &exact).unwrap();
        let cached = open_preview_cache_in(&root, &source, 3).unwrap().unwrap();
        assert_eq!(cached.total_points, exact.total_points);
        assert_eq!(cached.bounds, exact.bounds);
        assert_eq!(cached.scan_poses, exact.scan_poses);
        assert_eq!(cached.point_ordinals, exact.point_ordinals);
        assert_eq!(cached.points.len(), 3);
        for (cached, exact) in cached.points.iter().zip(&exact.points) {
            assert_eq!(cached.xyz, exact.xyz);
        }
        assert_eq!(
            open_preview_cache_in(&root, &source, 2)
                .unwrap()
                .unwrap()
                .points
                .len(),
            2
        );

        // A changed source no longer matches its cached preview.
        fs::write(&source, b"changed").unwrap();
        assert!(open_preview_cache_in(&root, &source, 3).unwrap().is_none());
    }

    #[test]
    fn cached_pcd_preview_preserves_viewpoint_and_world_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("posed-scan.pcd");
        fs::write(
            &source,
            "FIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nWIDTH 4\nHEIGHT 1\nPOINTS 4\nVIEWPOINT 10 20 30 1 0 0 0\nDATA ascii\n0 0 0\n1 0 0\n2 0 0\n3 0 0\n",
        )
        .unwrap();
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
        assert_eq!(cached.scan_poses[0].position, [10.0, 20.0, 30.0]);
        assert_eq!(cached.point_ordinals.len(), 2);
    }

    #[test]
    fn cached_ptx_preview_preserves_multiple_scanner_poses() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("two-stations.ptx");
        let first = "1\n2\n10 20 30\n0 1 0\n-1 0 0\n0 0 1\n0 1 0 0\n-1 0 0 0\n0 0 1 0\n10 20 30 1\n0 0 0 0\n1 2 3 0.5 10 20 30\n";
        let second = "1\n2\n40 50 60\n1 0 0\n0 1 0\n0 0 1\n1 0 0 40\n0 1 0 50\n0 0 1 60\n0 0 0 1\n0 0 0 0\n1 2 3 0.5 30 20 10\n";
        fs::write(&source, format!("{first}{second}")).unwrap();
        let cloud = super::super::open(&source, 2).unwrap();
        assert_eq!(cloud.scan_poses.len(), 2);
        let config = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("cache")),
        };
        let index = OctreeIndex::build_cached(&cloud, config.clone()).unwrap();
        let metadata_path = index.storage.path().join("cloud.json");
        let cached = open_cached_preview(&source, 2, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(cached.total_points, cloud.total_points);
        assert_eq!(cached.bounds, cloud.bounds);
        assert_eq!(cached.scan_poses, cloud.scan_poses);
        assert_eq!(cached.scan_poses[0].position, [10.0, 20.0, 30.0]);
        assert_eq!(cached.scan_poses[1].position, [40.0, 50.0, 60.0]);
        assert_eq!(cached.point_ordinals.len(), 2);

        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("scan_poses");
        fs::write(&metadata_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        assert!(open_cached_preview(&source, 2, config.clone())
            .unwrap()
            .is_none());
        OctreeIndex::open_cached_if_present(&cloud, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(
            open_cached_preview(&source, 2, config)
                .unwrap()
                .unwrap()
                .scan_poses,
            cloud.scan_poses
        );
    }

    #[test]
    fn single_pass_index_keeps_ptx_poses_and_cleans_cancelled_build() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("one-station.ptx");
        fs::write(
            &source,
            "2\n2\n10 20 30\n1 0 0\n0 1 0\n0 0 1\n1 0 0 0\n0 1 0 0\n0 0 1 0\n10 20 30 1\n1 0 1 0.5 10 20 30\n2 0 1 0.5 20 30 40\n1 1 1 0.5 30 40 50\n2 1 1 0.5 40 50 60\n",
        )
        .unwrap();
        let expected = super::super::open(&source, 2).unwrap();
        let cache_root = directory.path().join("cache");
        let config = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(cache_root.clone()),
        };
        let cancelled = OctreeIndex::open_and_build_cached_with_progress(
            &source,
            2,
            config.clone(),
            |update| {
                if update.stage == IndexStage::BuildingTree {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(cancelled, Err(LoadError::Cancelled)));
        assert_eq!(fs::read_dir(&cache_root).unwrap().count(), 0);

        let saw_preview = std::cell::Cell::new(false);
        let (cloud, index) = OctreeIndex::open_and_build_cached_with_preview(
            &source,
            2,
            config.clone(),
            |preview| {
                assert_eq!(preview.total_points, 4);
                assert_eq!(preview.scan_poses, expected.scan_poses);
                saw_preview.set(true);
                Ok(())
            },
            |update| {
                if update.stage == IndexStage::BuildingTree {
                    assert!(saw_preview.get());
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(saw_preview.get());
        assert_eq!(cloud.total_points, expected.total_points);
        assert_eq!(cloud.bounds, expected.bounds);
        assert_eq!(cloud.scan_poses, expected.scan_poses);
        assert_eq!(cloud.point_ordinals, expected.point_ordinals);
        assert_eq!(index.root.total_points, 4);
        let cached = open_cached_preview(&source, 2, config).unwrap().unwrap();
        assert_eq!(cached.scan_poses, expected.scan_poses);
        assert_eq!(cached.total_points, 4);
    }

    /// Remove the scan ranges from cache metadata, as a cache written before
    /// they were recorded has it.
    fn forget_scan_ranges(metadata_path: &Path) {
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(metadata_path).unwrap()).unwrap();
        assert!(legacy
            .as_object_mut()
            .unwrap()
            .remove("scan_ranges")
            .is_some());
        fs::write(metadata_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    }

    /// The scan ranges cache metadata holds, if it has the field.
    fn stored_scan_ranges(metadata_path: &Path) -> Option<Vec<ScanRange>> {
        let metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(metadata_path).unwrap()).unwrap();
        metadata
            .get("scan_ranges")
            .map(|ranges| serde_json::from_value(ranges.clone()).unwrap())
    }

    #[test]
    fn ptx_scan_ranges_are_cached_and_a_cache_without_them_still_opens() {
        use crate::ptx::tests::{three_block_ranges, THREE_BLOCKS};

        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("three-blocks.ptx");
        fs::write(&source, THREE_BLOCKS).unwrap();
        let cloud = super::super::open(&source, 10).unwrap();
        assert_eq!(cloud.scan_ranges, three_block_ranges());
        let config = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("cache")),
        };
        let index = OctreeIndex::build_cached(&cloud, config.clone()).unwrap();
        let metadata_path = index.storage.path().join("cloud.json");
        let cached = open_cached_preview(&source, 10, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(cached.scan_ranges, cloud.scan_ranges);
        assert_eq!(cached.scan_poses, cloud.scan_poses);
        assert_eq!(cached.station_pose(5).unwrap().position, [40.0, 50.0, 60.0]);
        assert!(cached.station_pose(3).is_none());

        // A cache from before the ranges were recorded: it opens, and the
        // points of its two stations cannot be told apart.
        forget_scan_ranges(&metadata_path);
        let legacy = open_cached_preview(&source, 10, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(legacy.total_points, 6);
        assert_eq!(legacy.scan_poses, cloud.scan_poses);
        assert!(legacy.scan_ranges.is_empty() && !legacy.scan_ranges_known());
        assert!((0..6).all(|ordinal| legacy.station_of(ordinal).is_none()));
        // Attaching the index writes the metadata again, and must not turn
        // "not recorded" into "no scans".
        OctreeIndex::open_cached_if_present(&legacy, config.clone())
            .unwrap()
            .unwrap();
        OctreeIndex::build_cached(&legacy, config.clone()).unwrap();
        assert_eq!(stored_scan_ranges(&metadata_path), None);
        assert!(!open_cached_preview(&source, 10, config.clone())
            .unwrap()
            .unwrap()
            .scan_ranges_known());

        // A cloud that was read brings its ranges back into the cache, and
        // one that does not know them leaves them there.
        OctreeIndex::open_cached_if_present(&cloud, config.clone())
            .unwrap()
            .unwrap();
        OctreeIndex::open_cached_if_present(&legacy, config.clone())
            .unwrap()
            .unwrap();
        OctreeIndex::build_cached(&legacy, config.clone()).unwrap();
        let restored = open_cached_preview(&source, 10, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(restored.scan_ranges, cloud.scan_ranges);
        assert!(restored.scan_ranges_known());

        // Reading the source once more gives a cloud from the older cache
        // its ranges, and attaching the index then keeps them.
        forget_scan_ranges(&metadata_path);
        let mut reread = legacy.clone();
        let mut counts = Vec::new();
        reread
            .read_scan_ranges(|count| {
                counts.push(count);
                Ok(())
            })
            .unwrap();
        assert_eq!(counts, [6]);
        assert!(reread.scan_ranges_known());
        assert_eq!(reread.scan_ranges, cloud.scan_ranges);
        assert_eq!(reread.scan_poses, cloud.scan_poses);
        assert_eq!(reread.station_pose(5).unwrap().position, [40.0, 50.0, 60.0]);
        assert_eq!(reread.points.len(), legacy.points.len());
        OctreeIndex::open_cached_if_present(&reread, config.clone())
            .unwrap()
            .unwrap();
        assert_eq!(
            stored_scan_ranges(&metadata_path),
            Some(cloud.scan_ranges.clone())
        );
        // A cancelled or failed pass leaves the cloud as it was.
        let mut cancelled = legacy.clone();
        assert!(matches!(
            cancelled.read_scan_ranges(|_| Err(LoadError::Cancelled)),
            Err(LoadError::Cancelled)
        ));
        assert!(cancelled.scan_ranges.is_empty() && !cancelled.scan_ranges_known());

        // The pass that also writes the index records the same ranges.
        let single_pass = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("single-pass")),
        };
        let (built, _index) = OctreeIndex::open_and_build_cached_with_progress(
            &source,
            10,
            single_pass.clone(),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(built.scan_ranges, cloud.scan_ranges);
        assert_eq!(
            open_cached_preview(&source, 10, single_pass)
                .unwrap()
                .unwrap()
                .scan_ranges,
            cloud.scan_ranges
        );

        // The kept preview of a large source carries them as well.
        let previews = directory.path().join("previews");
        write_preview_cache_in(&previews, &cloud).unwrap();
        let preview = open_preview_cache_in(&previews, &source, 10)
            .unwrap()
            .unwrap();
        assert_eq!(preview.scan_ranges, cloud.scan_ranges);
        let fingerprint = cache_fingerprint(&cloud, &IndexConfig::default()).unwrap();
        forget_scan_ranges(&preview_cache_directory(&previews, &fingerprint).join("preview.json"));
        let legacy = open_preview_cache_in(&previews, &source, 10)
            .unwrap()
            .unwrap();
        assert_eq!(legacy.scan_poses, cloud.scan_poses);
        assert!(legacy.scan_ranges.is_empty() && !legacy.scan_ranges_known());

        // An index built from that preview reads the whole source, and
        // records the ranges the preview could not give.
        let two_pass = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("two-pass")),
        };
        let index = OctreeIndex::build_cached(&legacy, two_pass.clone()).unwrap();
        assert_eq!(
            stored_scan_ranges(&index.storage.path().join("cloud.json")),
            Some(cloud.scan_ranges.clone())
        );
        let indexed = open_cached_preview(&source, 10, two_pass).unwrap().unwrap();
        assert!(indexed.scan_ranges_known());
        assert_eq!(indexed.scan_ranges, cloud.scan_ranges);
        assert_eq!(indexed.scan_poses, cloud.scan_poses);
        assert!(indexed.station_pose(3).is_none());
        assert_eq!(
            indexed.station_pose(2).unwrap().position,
            [10.0, 20.0, 30.0]
        );
    }

    #[test]
    fn e57_scan_ranges_are_cached_and_older_caches_use_the_stated_counts() {
        use crate::e57_points::tests::{station_position, write_scans};

        let directory = tempfile::tempdir().unwrap();
        // Every record of the first file holds a point; one record of the
        // second does not, so its stated counts exceed its points.
        for (name, first_scan, stated_counts_fit) in [
            ("valid.e57", [true; 4], true),
            ("invalid-record.e57", [true, false, true, true], false),
        ] {
            let source = directory.path().join(name);
            write_scans(
                &source,
                &[(true, &first_scan), (false, &[true; 4]), (true, &[true; 4])],
            );
            let cloud = super::super::open(&source, 10).unwrap();
            let first = if stated_counts_fit { 4 } else { 3 };
            assert_eq!(
                cloud
                    .scan_ranges
                    .iter()
                    .map(|range| (range.first_ordinal, range.station))
                    .collect::<Vec<_>>(),
                [(0, Some(0)), (first, None), (first + 4, Some(1))],
                "{name}"
            );
            let config = IndexConfig {
                leaf_points: 2,
                preview_points: 2,
                max_depth: 4,
                scratch_dir: Some(directory.path().join(format!("{name}-cache"))),
            };
            let (built, index) = OctreeIndex::open_and_build_cached_with_progress(
                &source,
                10,
                config.clone(),
                |_| Ok(()),
            )
            .unwrap();
            assert_eq!(built.scan_ranges, cloud.scan_ranges, "{name}");
            let metadata_path = index.storage.path().join("cloud.json");
            let cached = open_cached_preview(&source, 10, config.clone())
                .unwrap()
                .unwrap();
            assert_eq!(cached.scan_ranges, cloud.scan_ranges, "{name}");
            assert_eq!(
                cached.station_pose(first + 4).unwrap().position,
                station_position(2),
                "{name}"
            );

            // A cache from before the ranges were recorded falls back on the
            // record counts the file states, when they add up to the points.
            forget_scan_ranges(&metadata_path);
            let legacy = open_cached_preview(&source, 10, config.clone())
                .unwrap()
                .unwrap();
            assert_eq!(legacy.total_points, cloud.total_points, "{name}");
            assert_eq!(legacy.scan_poses, cloud.scan_poses, "{name}");
            if stated_counts_fit {
                assert_eq!(legacy.scan_ranges, cloud.scan_ranges, "{name}");
                assert_eq!(
                    legacy.station_pose(0).unwrap().position,
                    station_position(0)
                );
                assert!(legacy.station_pose(4).is_none());
                assert!(legacy.scan_ranges_known(), "{name}");
            } else {
                // Which scan lost a record is known after a pass only, and
                // until then nothing about it is stored.
                assert!(legacy.scan_ranges.is_empty(), "{name}");
                assert!(!legacy.scan_ranges_known(), "{name}");
                assert!((0..11).all(|ordinal| legacy.station_of(ordinal).is_none()));
                OctreeIndex::open_cached_if_present(&legacy, config.clone())
                    .unwrap()
                    .unwrap();
                assert_eq!(stored_scan_ranges(&metadata_path), None, "{name}");
                let mut reread = legacy.clone();
                reread.read_scan_ranges(|_| Ok(())).unwrap();
                assert_eq!(reread.scan_ranges, cloud.scan_ranges, "{name}");
                assert_eq!(
                    reread.station_pose(7).unwrap().position,
                    station_position(2)
                );
                OctreeIndex::open_cached_if_present(&reread, config.clone())
                    .unwrap()
                    .unwrap();
                let restored = open_cached_preview(&source, 10, config.clone())
                    .unwrap()
                    .unwrap();
                assert!(restored.scan_ranges_known(), "{name}");
                assert_eq!(restored.scan_ranges, cloud.scan_ranges, "{name}");
            }

            // The kept preview of a large source behaves the same.
            let previews = directory.path().join(format!("{name}-previews"));
            write_preview_cache_in(&previews, &cloud).unwrap();
            let preview = open_preview_cache_in(&previews, &source, 10)
                .unwrap()
                .unwrap();
            assert_eq!(preview.scan_ranges, cloud.scan_ranges, "{name}");
            let fingerprint = cache_fingerprint(&cloud, &IndexConfig::default()).unwrap();
            forget_scan_ranges(
                &preview_cache_directory(&previews, &fingerprint).join("preview.json"),
            );
            let legacy = open_preview_cache_in(&previews, &source, 10)
                .unwrap()
                .unwrap();
            if stated_counts_fit {
                assert_eq!(legacy.scan_ranges, cloud.scan_ranges, "{name}");
            } else {
                assert!(legacy.scan_ranges.is_empty(), "{name}");
            }
            assert_eq!(legacy.scan_ranges_known(), stated_counts_fit, "{name}");
        }
    }

    #[test]
    fn a_kept_pcd_preview_takes_its_station_from_the_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("merged.pcd");
        fs::write(
            &source,
            "FIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nWIDTH 2\nHEIGHT 1\nPOINTS 2\nVIEWPOINT 0 0 0 1 0 0 0\nDATA ascii\n1 0 0\n2 0 0\n",
        )
        .unwrap();
        let cloud = super::super::open(&source, 2).unwrap();
        assert!(cloud.scan_poses.is_empty());
        let previews = directory.path().join("previews");
        write_preview_cache_in(&previews, &cloud).unwrap();

        // A preview kept before took the viewpoint every writer puts in the
        // header for a station, and recorded no scan ranges.
        let fingerprint = cache_fingerprint(&cloud, &IndexConfig::default()).unwrap();
        let metadata_path = preview_cache_directory(&previews, &fingerprint).join("preview.json");
        forget_scan_ranges(&metadata_path);
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        metadata["scan_poses"] = serde_json::to_value([ScanPose {
            label: "VIEWPOINT".into(),
            position: [0.0; 3],
            axes: None,
        }])
        .unwrap();
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        let kept = open_preview_cache_in(&previews, &source, 2)
            .unwrap()
            .unwrap();
        assert!(kept.scan_poses.is_empty());
        assert!((0..2).all(|ordinal| kept.station_of(ordinal).is_none()));
    }

    #[test]
    fn cached_scan_ranges_share_the_limit_of_the_poses() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("many-blocks.ptx");
        let block =
            "1\n1\n1 2 3\n1 0 0\n0 1 0\n0 0 1\n1 0 0 0\n0 1 0 0\n0 0 1 0\n1 2 3 1\n1 0 0 0.5\n";
        fs::write(&source, block.repeat(MAX_CACHED_SCAN_POSES + 1)).unwrap();
        let cloud = super::super::open(&source, 4).unwrap();
        assert_eq!(cloud.scan_ranges.len(), MAX_CACHED_SCAN_POSES + 1);
        assert_eq!(cloud.station_of(4_096), Some(4_096));

        // More scans than a cache keeps: the preview is kept without them.
        let previews = directory.path().join("previews");
        write_preview_cache_in(&previews, &cloud).unwrap();
        let preview = open_preview_cache_in(&previews, &source, 4)
            .unwrap()
            .unwrap();
        assert_eq!(preview.total_points, cloud.total_points);
        assert!(preview.scan_poses.is_empty() && preview.scan_ranges.is_empty());

        // Metadata that holds more ranges than the limit, or ranges that
        // cannot be searched, is not trusted.
        let within: Vec<ScanRange> = cloud.scan_ranges[..MAX_CACHED_SCAN_POSES].to_vec();
        assert!(valid_scan_ranges(&within, cloud.total_points));
        assert!(valid_scan_ranges(&[], cloud.total_points));
        assert!(!valid_scan_ranges(&cloud.scan_ranges, cloud.total_points));
        assert!(!valid_scan_ranges(&within[1..], cloud.total_points));
        assert!(!valid_scan_ranges(&within, 100));
        let mut unordered = within.clone();
        unordered.swap(1, 2);
        assert!(!valid_scan_ranges(&unordered, cloud.total_points));

        let fingerprint = cache_fingerprint(&cloud, &IndexConfig::default()).unwrap();
        let metadata_path = preview_cache_directory(&previews, &fingerprint).join("preview.json");
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        assert!(metadata.get("scan_ranges").is_none());
        metadata["scan_ranges"] = serde_json::to_value(&cloud.scan_ranges).unwrap();
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(open_preview_cache_in(&previews, &source, 4)
            .unwrap()
            .is_none());

        let text = directory.path().join("points.xyz");
        fs::write(&text, "0 0 0\n1 0 0\n2 0 0\n3 0 0\n").unwrap();
        let text_cloud = super::super::open(&text, 4).unwrap();
        assert!(text_cloud.scan_ranges.is_empty());
        let config = IndexConfig {
            leaf_points: 2,
            preview_points: 2,
            max_depth: 4,
            scratch_dir: Some(directory.path().join("cache")),
        };
        let index = OctreeIndex::build_cached(&text_cloud, config.clone()).unwrap();
        let metadata_path = index.storage.path().join("cloud.json");
        assert!(open_cached_preview(&text, 4, config.clone())
            .unwrap()
            .unwrap()
            .scan_ranges
            .is_empty());
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
        metadata["scan_ranges"] = serde_json::to_value(&unordered[..3]).unwrap();
        fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();
        assert!(open_cached_preview(&text, 4, config.clone()).is_err());
        // A source without scans in a cache from before the ranges existed.
        forget_scan_ranges(&metadata_path);
        let legacy = open_cached_preview(&text, 4, config).unwrap().unwrap();
        assert_eq!(legacy.total_points, 4);
        assert!(legacy.scan_ranges.is_empty());
    }

    #[test]
    fn cached_text_cloud_preview_preserves_count_bounds_and_attributes() {
        let directory = tempfile::tempdir().unwrap();
        for extension in ["xyz", "asc", "txt", "csv", "pts"] {
            let source = directory.path().join(format!("scan.{extension}"));
            let separator = if extension == "csv" { "," } else { " " };
            let mut input = if extension == "pts" {
                String::from("4\n")
            } else {
                String::new()
            };
            for x in 0..4 {
                let fields = if extension == "pts" {
                    vec![x, 0, 0, 100, 10, 20, 30]
                } else {
                    vec![x, 0, 0, 10, 20, 30]
                };
                input.push_str(
                    &fields
                        .into_iter()
                        .map(|value| value.to_string())
                        .collect::<Vec<_>>()
                        .join(separator),
                );
                input.push('\n');
            }
            fs::write(&source, input).unwrap();
            let cloud = super::super::open(&source, 2).unwrap();
            let config = IndexConfig {
                leaf_points: 2,
                preview_points: 2,
                max_depth: 4,
                scratch_dir: Some(directory.path().join("cache")),
            };
            let _index = OctreeIndex::build_cached(&cloud, config.clone()).unwrap();
            let cached = open_cached_preview(&source, 2, config).unwrap().unwrap();
            assert_eq!(cached.total_points, 4, "{extension}");
            assert_eq!(cached.bounds, cloud.bounds, "{extension}");
            assert_eq!(cached.has_rgb, cloud.has_rgb, "{extension}");
            assert_eq!(cached.has_intensity, cloud.has_intensity, "{extension}");
            assert_eq!(cached.points.len(), 2, "{extension}");
            assert_eq!(cached.point_ordinals.len(), 2, "{extension}");
        }
    }
}
