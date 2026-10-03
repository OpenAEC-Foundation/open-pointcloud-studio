//! File loading and bounded point sampling shared by the native UI and future clients.

use std::cell::Cell;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

mod bag3d;
mod drawing;
mod dxf;
mod e57_points;
mod e57_quick;
mod export;
pub mod grid2d;
pub mod local_fit;
mod mesh_formats;
mod mesh_points;
mod mesh_quality;
mod mesh_simplify;
mod mesh_write;
mod mesher;
mod obj_mesh;
mod octree;
mod pcd;
mod ply;
mod ply_mesh;
mod ptx;
pub mod region_source;
mod scan_image;
mod snapshots;
mod surface_mesh;
#[cfg(test)]
mod test_shapes;
mod window_reader;

pub use bag3d::{
    fetch_bag3d_obj, fetch_bag3d_obj_with, BagBounds, BagLod, BagProgress, BagStats,
    BAG3D_USER_AGENT,
};
pub use drawing::{
    class_point_layer, drawing_info_text, layer_name, source_point_layer, write_drawing,
    write_drawing_progress, Drawing2d, DrawingEntity, DrawingFormat, DrawingFrame, DrawingLayer,
    DrawingOrigin, DrawingRequest, DrawingStats, DrawingUnits, DrawingVersion, DrawingView,
    PointColor, PointLayers, DEFAULT_CUT_GRID, DEFAULT_DRAWING_POINTS, DEFAULT_MAX_WALL_THICKNESS,
    DEFAULT_MIN_WALL_THICKNESS, DEFAULT_POINT_SPACING, DEFAULT_SLAB_THICKNESS, LAYER_CUT_FILL,
    LAYER_CUT_OUTLINE, LAYER_FRAME, LAYER_INFO, LAYER_POINTS, LAYER_RGB_CONTRAST,
    LAYER_RGB_CUT_FILL, LAYER_RGB_FRAME, MAX_DRAWING_POINTS, MAX_SLAB_THICKNESS,
    MIN_SLAB_THICKNESS,
};
pub use dxf::read_mesh as read_dxf_mesh;
pub use export::{
    export_affine, export_affine_axes, export_affine_axes_where, export_affine_where,
    export_e57_translated_where, export_e57_uniform_affine_where, export_full, export_map,
    export_map_auto_count, export_section, export_section_where, export_thin_percent_where,
    export_where, merge_las_map_count, ExportFormat,
};
pub use mesh_formats::{read_off_mesh, read_stl_mesh};
pub use mesh_quality::{
    mesh_deviation, mesh_topology, mesh_topology_by_position, open_boundary_vertices,
    MeshDeviation, MeshTopology,
};
pub use mesh_simplify::{simplify_mesh, simplify_mesh_progress, SimplifiedMesh};
pub use mesh_write::{read_stl_origin, write_mesh, MeshFormat, MeshWriteReport};
pub use mesher::{
    mesh_terrain_obj, mesh_terrain_obj_where, mesh_terrain_obj_where_progress, MeshConfig,
    MeshProgress, MeshStage, MeshStats,
};
pub use obj_mesh::{read_obj_mesh, write_obj_mesh, MeshGeometry};
pub use octree::{IndexConfig, IndexProgress, IndexStage, IndexedNode, IndexedPoint, OctreeIndex};
pub use ply_mesh::read_ply_mesh;
pub use scan_image::{select_scan_image, ScanImage, ScanImageFormat};
pub use surface_mesh::{
    mesh_surface_obj, mesh_surface_obj_where, mesh_surface_obj_where_progress, SurfaceMeshConfig,
};

pub fn read_mesh_geometry(path: impl AsRef<Path>) -> Result<Option<MeshGeometry>, LoadError> {
    let path = path.as_ref();
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("obj") => read_obj_mesh(path).map(Some),
        Some("ply") => read_ply_mesh(path),
        Some("off") => read_off_mesh(path),
        Some("stl") => read_stl_mesh(path),
        Some("dxf") => read_dxf_mesh(path),
        _ => Err(LoadError::UnsupportedFormat(
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_owned(),
        )),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Point {
    pub xyz: [f64; 3],
    pub rgb: Option<[u8; 3]>,
    pub intensity: Option<u16>,
    pub classification: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScanPose {
    pub label: String,
    pub position: [f64; 3],
    /// Registered unit directions of the scanner's local X, Y and Z axes.
    pub axes: Option<[[f64; 3]; 3]>,
}

/// The points of one scan in a source that holds several: scans are read one
/// after another, so a scan owns the source ordinals from its first one up to
/// the first one of the next scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ScanRange {
    pub first_ordinal: u64,
    /// Index into `PointCloud::scan_poses`; a scan without a station has none.
    pub station: Option<u32>,
}

/// The scans a source pass has met so far: their stations in file order and
/// the ordinal each scan began at.
#[derive(Default)]
struct ScanLog {
    poses: Vec<ScanPose>,
    ranges: Vec<ScanRange>,
}

impl ScanLog {
    fn begin(&mut self, first_ordinal: u64, pose: Option<ScanPose>) {
        self.ranges.push(ScanRange {
            first_ordinal,
            station: pose
                .as_ref()
                .and_then(|_| u32::try_from(self.poses.len()).ok()),
        });
        self.poses.extend(pose);
    }
}

fn quaternion_axes(rotation: [f64; 4]) -> Option<[[f64; 3]; 3]> {
    if !rotation.iter().all(|value| value.is_finite()) {
        return None;
    }
    let norm = rotation
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return None;
    }
    let [w, x, y, z] = rotation.map(|value| value / norm);
    Some([
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ],
        [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ],
        [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ])
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Bounds {
    pub fn center(self) -> [f64; 3] {
        std::array::from_fn(|axis| (self.min[axis] + self.max[axis]) * 0.5)
    }

    pub fn extent(self) -> f64 {
        (0..3)
            .map(|axis| self.max[axis] - self.min[axis])
            .fold(0.0, f64::max)
    }

    fn include(&mut self, xyz: [f64; 3]) {
        for (axis, value) in xyz.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
    }
}

#[derive(Debug, Clone)]
pub struct PointCloud {
    pub path: PathBuf,
    pub total_points: u64,
    pub bounds: Bounds,
    pub points: Vec<Point>,
    /// Source ordinals parallel to the bounded preview points. A value of
    /// `u64::MAX` means the compressed random-access reader could not prove
    /// the exact source ordinal; the disk octree always has exact ordinals.
    pub point_ordinals: Vec<u64>,
    pub has_rgb: bool,
    pub has_intensity: bool,
    pub has_classification: bool,
    pub scan_poses: Vec<ScanPose>,
    /// The scans of a source that is read scan by scan, in ordinal order, as
    /// a pass over the source met them. Empty for other sources, and while
    /// `scan_ranges_known` is false.
    pub scan_ranges: Vec<ScanRange>,
    /// Photos stored with the scanner stations, listed without decoding them.
    pub scan_images: Vec<ScanImage>,
    /// Shown ahead of a full pass over the source: the count is the stated
    /// one and the bounds are loose. Such a cloud is replaced by the checked
    /// result and cannot be indexed.
    pub provisional: bool,
    source_stamp: Option<SourceStamp>,
    /// Whether `scan_ranges` say for every point which scan holds it. Kept
    /// apart from the ranges because "no scans" and "not recorded" are both
    /// empty, and only the first may be written to a cache as a fact.
    scan_ranges_known: bool,
}

impl PointCloud {
    /// Compare the loaded file revision, even when one view contains only a header.
    pub fn same_source_revision(&self, other: &Self) -> bool {
        self.path == other.path && self.source_stamp == other.source_stamp
    }

    /// Ensure a long-running operation still reads the loaded source revision.
    pub fn validate_source(&self) -> Result<(), LoadError> {
        let stamp = self
            .source_stamp
            .ok_or_else(|| LoadError::InvalidData("source identity is unavailable".into()))?;
        if SourceStamp::read(&self.path)? != stamp {
            return Err(LoadError::InvalidData(
                "source changed since loading".into(),
            ));
        }
        Ok(())
    }

    /// Whether the scan that holds each point is known, so that `station_of`
    /// can answer. It is not for a provisional cloud, whose count and
    /// ordinals are not checked yet, and not for a cloud from a cache written
    /// before scan ranges were recorded when its source holds several scans
    /// and nothing else tells where they begin. `read_scan_ranges` finds out.
    pub fn scan_ranges_known(&self) -> bool {
        self.scan_ranges_known && !self.provisional
    }

    /// Read the source once more, without keeping any point, to record where
    /// its scans begin when the cache this cloud came from did not say. Does
    /// nothing when they are known. `progress` receives the points read so
    /// far and may cancel, as in `open_with_progress`. Afterwards
    /// `OctreeIndex::open_cached_if_present` or `OctreeIndex::build_cached`
    /// with this cloud keeps the ranges in the index cache.
    pub fn read_scan_ranges(
        &mut self,
        mut progress: impl FnMut(u64) -> Result<(), LoadError>,
    ) -> Result<(), LoadError> {
        if self.provisional {
            return Err(LoadError::InvalidData(
                "the cloud is a preview that was not checked against its source".into(),
            ));
        }
        if self.scan_ranges_known {
            return Ok(());
        }
        self.validate_source()?;
        let mut scans = ScanLog::default();
        let read = Cell::new(0u64);
        visit_points_with_poses(
            &self.path,
            &mut |_| {
                read.set(read.get() + 1);
                if read.get().is_multiple_of(65_536) {
                    progress(read.get())?;
                }
                Ok(())
            },
            &mut |pose| scans.begin(read.get(), pose),
        )?;
        progress(read.get())?;
        self.validate_source()?;
        if read.get() != self.total_points {
            return Err(LoadError::InvalidData(
                "source no longer holds the points of the loaded cloud".into(),
            ));
        }
        self.scan_poses = scans.poses;
        self.scan_ranges = scans.ranges;
        self.scan_ranges_known = true;
        Ok(())
    }

    /// Index into `scan_poses` of the station that measured the point with
    /// this source ordinal. `None` when the point belongs to a scan without a
    /// station, or when it is not known which scan it belongs to.
    pub fn station_of(&self, ordinal: u64) -> Option<usize> {
        if !self.scan_ranges_known() || ordinal >= self.total_points {
            return None;
        }
        if self.scan_ranges.is_empty() {
            // One file per station needs no ranges: all points are its own.
            return (self.scan_poses.len() == 1).then_some(0);
        }
        // An empty scan begins where the next one does; the last range that
        // begins at or before the ordinal is the scan that holds the point.
        let after = self
            .scan_ranges
            .partition_point(|range| range.first_ordinal <= ordinal);
        let station = self.scan_ranges[after.checked_sub(1)?].station? as usize;
        (station < self.scan_poses.len()).then_some(station)
    }

    /// The pose of the station that measured the point with this source
    /// ordinal, as `station_of` finds it.
    pub fn station_pose(&self, ordinal: u64) -> Option<&ScanPose> {
        self.station_of(ordinal)
            .map(|station| &self.scan_poses[station])
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceStamp {
    length: u64,
    modified: Option<SystemTime>,
}

impl SourceStamp {
    fn read(path: &Path) -> Result<Self, LoadError> {
        let metadata = fs::metadata(path)?;
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified().ok(),
        })
    }
}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Las(las::Error),
    E57(e57::Error),
    UnsupportedFormat(String),
    InvalidData(String),
    Cancelled,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O error: {error}"),
            Self::Las(error) => write!(f, "LAS/LAZ error: {error}"),
            Self::E57(error) => write!(f, "E57 error: {error}"),
            Self::UnsupportedFormat(extension) => write!(f, "Unsupported format: {extension}"),
            Self::InvalidData(reason) => write!(f, "Invalid point cloud: {reason}"),
            Self::Cancelled => f.write_str("Operation cancelled"),
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<las::Error> for LoadError {
    fn from(error: las::Error) -> Self {
        Self::Las(error)
    }
}

impl From<e57::Error> for LoadError {
    fn from(error: e57::Error) -> Self {
        Self::E57(error)
    }
}

/// Open a point cloud while keeping at most `sample_limit` points in memory.
/// Bounds and point counts are computed from the full input stream.
pub fn open(path: impl AsRef<Path>, sample_limit: usize) -> Result<PointCloud, LoadError> {
    open_with_progress(path, sample_limit, |_| Ok(()))
}

/// Open a point cloud while reporting the count of decoded, finite points.
/// The callback runs on the reader's thread after each 65,536-point batch and
/// once more at completion. Returning an error cancels the load.
pub fn open_with_progress(
    path: impl AsRef<Path>,
    sample_limit: usize,
    progress: impl FnMut(u64) -> Result<(), LoadError>,
) -> Result<PointCloud, LoadError> {
    open_showing(path.as_ref(), sample_limit, progress, None)
}

/// Source size from which a file is shown while it is still being read.
pub const LARGE_SOURCE_BYTES: u64 = 512 * 1024 * 1024;

/// Open a point cloud like `open_with_progress`, and show a source of at
/// least `LARGE_SOURCE_BYTES` while it is being read: `snapshot` receives the
/// points known so far every few seconds, first those spread through an E57
/// scan that allows it and then, added to them, the ones the pass has read.
/// These clouds are provisional: each replaces the one before, and the
/// returned cloud replaces the last. For a source that was shown, the
/// returned cloud keeps up to two million points instead of `sample_limit`,
/// so that it is as dense as the last snapshot. At most two sources are
/// shown at a time; further ones are read without snapshots.
pub fn open_with_snapshots(
    path: impl AsRef<Path>,
    sample_limit: usize,
    progress: impl FnMut(u64) -> Result<(), LoadError>,
    mut snapshot: impl FnMut(&PointCloud) -> Result<(), LoadError>,
) -> Result<PointCloud, LoadError> {
    open_showing(path.as_ref(), sample_limit, progress, Some(&mut snapshot))
}

fn open_showing(
    path: &Path,
    sample_limit: usize,
    mut progress: impl FnMut(u64) -> Result<(), LoadError>,
    mut snapshot: Option<snapshots::Show>,
) -> Result<PointCloud, LoadError> {
    if sample_limit == 0 {
        return Err(LoadError::InvalidData(
            "sample limit must be greater than zero".into(),
        ));
    }
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            [
                "ply", "e57", "pcd", "ptx", "xyz", "asc", "txt", "csv", "pts",
            ]
            .iter()
            .any(|format| extension.eq_ignore_ascii_case(format))
        })
    {
        if let Ok(Some(cached)) =
            octree::open_cached_preview(path, sample_limit, octree::IndexConfig::default())
        {
            progress(cached.total_points)?;
            return Ok(cached);
        }
        if let Ok(Some(cached)) = octree::open_preview_cache(path, sample_limit) {
            progress(cached.total_points)?;
            return Ok(cached);
        }
    }
    let before = SourceStamp::read(path)?;
    let mut snapshots = match &mut snapshot {
        Some(show) if before.length >= LARGE_SOURCE_BYTES => {
            snapshots::Snapshots::begin(path, before, &mut **show)?
        }
        _ => None,
    };
    // A source that is shown keeps enough points for its snapshots, and its
    // checked cloud is as dense as the last of them.
    let mut collector = Collector::new(if snapshots.is_some() {
        snapshots::Snapshots::sample_limit(sample_limit)
    } else {
        sample_limit
    });
    let mut scans = ScanLog::default();
    // The scan callback cannot look into the collector while the point
    // callback holds it, so the count of points read is kept beside it.
    let read = Cell::new(0u64);
    visit_points_with_poses(
        path,
        &mut |point| {
            collector.push(point)?;
            read.set(collector.total);
            if collector.total.is_multiple_of(65_536) {
                progress(collector.total)?;
                if let (Some(snapshots), Some(show)) = (&mut snapshots, &mut snapshot) {
                    snapshots.tick(&collector, &mut **show)?;
                }
            }
            Ok(())
        },
        &mut |pose| scans.begin(read.get(), pose),
    )?;
    drop(snapshots);
    progress(collector.total)?;
    let after = SourceStamp::read(path)?;
    if before != after {
        return Err(LoadError::InvalidData(
            "source changed while loading".into(),
        ));
    }
    let mut cloud = collector.finish(path.to_path_buf())?;
    cloud.scan_poses = scans.poses;
    cloud.scan_ranges = scans.ranges;
    cloud.scan_images = scan_images(path);
    cloud.source_stamp = Some(after);
    // The next open of an unchanged source then needs no point decoding.
    octree::write_preview_cache(&cloud);
    Ok(cloud)
}

/// Open an E57 scan from its metadata alone: stations, photos, stated extent
/// and record count are available at once, with no points yet. The count is
/// an upper bound and the bounds are a loose box, so the caller replaces this
/// cloud with `open`'s checked result and must not index or export it.
pub fn open_e57_header(path: impl AsRef<Path>) -> Result<PointCloud, LoadError> {
    let path = path.as_ref();
    if !is_e57(path) {
        return Err(LoadError::UnsupportedFormat(
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase(),
        ));
    }
    let stamp = SourceStamp::read(path)?;
    let summary = e57_points::summary(path)?;
    // A scan that states no extent is framed by its stations instead.
    let mut bounds = summary.bounds;
    if bounds.is_none() {
        for pose in &summary.poses {
            match &mut bounds {
                Some(bounds) => bounds.include(pose.position),
                None => {
                    bounds = Some(Bounds {
                        min: pose.position,
                        max: pose.position,
                    })
                }
            }
        }
    }
    let (Some(bounds), true) = (bounds, summary.records > 0) else {
        return Err(LoadError::InvalidData(
            "scan metadata states no points or extent".into(),
        ));
    };
    Ok(PointCloud {
        path: path.to_path_buf(),
        total_points: summary.records,
        bounds,
        points: Vec::new(),
        point_ordinals: Vec::new(),
        has_rgb: summary.has_rgb,
        has_intensity: summary.has_intensity,
        has_classification: false,
        scan_poses: summary.poses,
        // The metadata counts records, not points: where a scan begins among
        // the points is known once they have been read.
        scan_ranges: Vec::new(),
        scan_images: scan_images(path),
        provisional: true,
        source_stamp: Some(stamp),
        scan_ranges_known: false,
    })
}

/// Open a bounded preview of a large E57 scan by reading point records
/// spread through the file, on several threads, instead of all of it. `None`
/// means the file is not stored in a way that allows this. The count is the
/// stated number of records and the bounds cover only the sampled points, so
/// the caller replaces this cloud with `open`'s checked result and must not
/// index or export it.
pub fn open_e57_quick_preview(
    path: impl AsRef<Path>,
    sample_limit: usize,
) -> Result<Option<PointCloud>, LoadError> {
    let path = path.as_ref();
    if !is_e57(path) {
        return Ok(None);
    }
    e57_quick::preview(path, sample_limit)
}

fn is_e57(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("e57"))
}

/// List the station photos of a source. Photos are optional extras: a file
/// whose photo metadata cannot be read still opens, without photos.
fn scan_images(path: &Path) -> Vec<ScanImage> {
    if !is_e57(path) {
        return Vec::new();
    }
    e57_points::scan_images(path).unwrap_or_default()
}

/// Read scanner stations and their photo list from file metadata alone,
/// without decoding any point. Formats without such metadata return nothing.
pub fn scan_stations(path: impl AsRef<Path>) -> Result<(Vec<ScanPose>, Vec<ScanImage>), LoadError> {
    let path = path.as_ref();
    if !is_e57(path) {
        return Ok((Vec::new(), Vec::new()));
    }
    Ok((
        e57_points::scan_poses(path)?,
        e57_points::scan_images(path)?,
    ))
}

/// Read the encoded bytes (JPEG or PNG) of station photos from their source
/// file, in the order given. The descriptors come from `PointCloud::scan_images`.
pub fn read_scan_images(
    path: impl AsRef<Path>,
    images: &[ScanImage],
) -> Result<Vec<Vec<u8>>, LoadError> {
    let path = path.as_ref();
    if !is_e57(path) {
        return Err(LoadError::InvalidData(
            "this format stores no station photos".into(),
        ));
    }
    e57_points::read_images(path, images)
}

/// Open LAS/LAZ metadata immediately, before the preview sampling pass finishes.
/// The caller may replace this cloud with `open`'s fully checked result later.
pub fn open_las_header(path: impl AsRef<Path>) -> Result<PointCloud, LoadError> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "las" && extension != "laz" {
        return Err(LoadError::UnsupportedFormat(extension));
    }
    let stamp = SourceStamp::read(path)?;
    let reader = las::Reader::from_path(path)?;
    let header = reader.header();
    let count = header.number_of_points();
    let raw = header.bounds();
    let bounds = Bounds {
        min: [raw.min.x, raw.min.y, raw.min.z],
        max: [raw.max.x, raw.max.y, raw.max.z],
    };
    if count == 0
        || !bounds
            .min
            .iter()
            .chain(bounds.max.iter())
            .all(|v| v.is_finite())
    {
        return Err(LoadError::InvalidData("empty or invalid LAS header".into()));
    }
    Ok(PointCloud {
        path: path.to_path_buf(),
        total_points: count,
        bounds,
        points: Vec::new(),
        point_ordinals: Vec::new(),
        has_rgb: header.point_format().has_color,
        has_intensity: true,
        has_classification: true,
        scan_poses: Vec::new(),
        scan_ranges: Vec::new(),
        scan_images: Vec::new(),
        provisional: false,
        source_stamp: Some(stamp),
        scan_ranges_known: true,
    })
}

/// Sample LAS/LAZ in bounded, evenly spaced source ranges. Header bounds and
/// point count remain exact while a large preview no longer requires decoding
/// every point. Editing and selection still stream the full source.
pub fn open_las_preview(
    path: impl AsRef<Path>,
    sample_limit: usize,
) -> Result<PointCloud, LoadError> {
    if sample_limit == 0 {
        return Err(LoadError::InvalidData(
            "sample limit must be greater than zero".into(),
        ));
    }
    let path = path.as_ref();
    let stamp = SourceStamp::read(path)?;
    let mut cloud = open_las_header(path)?;
    let count = cloud.total_points;
    let target = count.min(sample_limit as u64);
    let compressed = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("laz"));
    let blocks = if count <= target { 1 } else { 256.min(target) };
    let block_len = target.div_ceil(blocks);
    cloud.points.reserve(target as usize);
    let mut reader = las::Reader::from_path(path)?;
    for block in 0..blocks {
        let start = if blocks == 1 {
            0
        } else {
            block * (count - block_len) / (blocks - 1)
        };
        reader.seek(start)?;
        let length = block_len
            .min(count - start)
            .min(target - cloud.points.len() as u64);
        for (offset, point) in reader.read_points(length)?.into_iter().enumerate() {
            cloud.points.push(convert_las_point(&point));
            cloud.point_ordinals.push(if compressed && count > target {
                u64::MAX
            } else {
                start + offset as u64
            });
        }
    }
    if cloud.points.len() as u64 != target || SourceStamp::read(path)? != stamp {
        return Err(LoadError::InvalidData(
            "LAS source changed while sampling".into(),
        ));
    }
    Ok(cloud)
}

/// Stream every point from a supported source file. The callback may stop the
/// read by returning an error; the caller controls any retained memory.
pub fn visit_points(
    path: impl AsRef<Path>,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    visit_points_with_poses(path, push, &mut |_| {})
}

/// Like `visit_points`, and tell when a scan begins in a format that is read
/// scan by scan: `scan_begin` is called before the first point of every scan,
/// with the station of the scan if it has one.
fn visit_points_with_poses(
    path: impl AsRef<Path>,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    scan_begin: &mut impl FnMut(Option<ScanPose>),
) -> Result<(), LoadError> {
    let path = path.as_ref();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "las" | "laz" => read_las(path, push),
        "ply" => ply::read(path, push),
        "obj" => mesh_points::read_obj(path, push),
        "off" => mesh_points::read_off(path, push),
        "stl" => mesh_points::read_stl(path, push),
        "ptx" => ptx::read(path, push, scan_begin),
        "pcd" => pcd::read(path, push, scan_begin),
        "dxf" => dxf::read(path, push),
        "e57" => e57_points::read(path, push, scan_begin),
        "xyz" | "asc" | "txt" | "csv" | "pts" => read_text(path, push, extension == "pts"),
        _ => Err(LoadError::UnsupportedFormat(extension)),
    }
}

struct Collector {
    points: Vec<Point>,
    ordinals: Vec<u64>,
    limit: usize,
    total: u64,
    bounds: Option<Bounds>,
    has_rgb: bool,
    has_intensity: bool,
    has_classification: bool,
    random_state: u64,
}

impl Collector {
    fn new(limit: usize) -> Self {
        Self {
            points: Vec::with_capacity(limit.min(100_000)),
            ordinals: Vec::with_capacity(limit.min(100_000)),
            limit,
            total: 0,
            bounds: None,
            has_rgb: false,
            has_intensity: false,
            has_classification: false,
            random_state: 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn push(&mut self, point: Point) -> Result<(), LoadError> {
        if !point.xyz.iter().all(|value| value.is_finite()) {
            return Err(LoadError::InvalidData("non-finite coordinate".into()));
        }
        let ordinal = self.total;
        self.total += 1;
        match &mut self.bounds {
            Some(bounds) => bounds.include(point.xyz),
            None => {
                self.bounds = Some(Bounds {
                    min: point.xyz,
                    max: point.xyz,
                })
            }
        }
        self.has_rgb |= point.rgb.is_some();
        self.has_intensity |= point.intensity.is_some();
        self.has_classification |= point.classification.is_some();

        if self.points.len() < self.limit {
            self.points.push(point);
            self.ordinals.push(ordinal);
        } else {
            // Reservoir sampling gives every input point the same inclusion chance.
            self.random_state ^= self.random_state << 13;
            self.random_state ^= self.random_state >> 7;
            self.random_state ^= self.random_state << 17;
            let index = self.random_state % self.total;
            if index < self.limit as u64 {
                self.points[index as usize] = point;
                self.ordinals[index as usize] = ordinal;
            }
        }
        Ok(())
    }

    fn finish(self, path: PathBuf) -> Result<PointCloud, LoadError> {
        let bounds = self
            .bounds
            .ok_or_else(|| LoadError::InvalidData("file contains no points".into()))?;
        Ok(PointCloud {
            path,
            total_points: self.total,
            bounds,
            points: self.points,
            point_ordinals: self.ordinals,
            has_rgb: self.has_rgb,
            has_intensity: self.has_intensity,
            has_classification: self.has_classification,
            scan_poses: Vec::new(),
            scan_ranges: Vec::new(),
            scan_images: Vec::new(),
            provisional: false,
            source_stamp: None,
            // The pass that fills a collector meets every scan of the source.
            scan_ranges_known: true,
        })
    }
}

fn read_las(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut reader = las::Reader::from_path(path)?;
    // The LAZ reader only uses its parallel decompressor for read_points_into;
    // calling read() for every point silently takes the serial path. Eight
    // default 50,000-point chunks bound memory while using multiple cores.
    let batch_limit = if reader.header().point_format().is_compressed {
        400_000
    } else {
        16_384
    };
    read_las_batches(&mut reader, push, batch_limit)
}

fn read_las_batches(
    reader: &mut las::Reader,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    batch_limit: usize,
) -> Result<(), LoadError> {
    let mut batch = Vec::with_capacity(batch_limit);
    loop {
        batch.clear();
        if reader.read_points_into(batch_limit as u64, &mut batch)? == 0 {
            break;
        }
        for point in &batch {
            push(convert_las_point(point))?;
        }
    }
    Ok(())
}

fn convert_las_point(point: &las::Point) -> Point {
    let rgb = point.color.map(|color| {
        let channels = [color.red, color.green, color.blue];
        if channels.iter().all(|channel| *channel <= 255) {
            channels.map(|channel| channel as u8)
        } else {
            channels.map(|channel| (channel / 257) as u8)
        }
    });
    Point {
        xyz: [point.x, point.y, point.z],
        rgb,
        intensity: Some(point.intensity),
        classification: Some(u8::from(point.classification)),
    }
}

fn read_text(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    pts: bool,
) -> Result<(), LoadError> {
    let reader = BufReader::new(File::open(path)?);
    let mut first_content_line = true;
    let mut seen_points = false;
    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        if pts && first_content_line && line.parse::<u64>().is_ok() {
            first_content_line = false;
            continue;
        }
        first_content_line = false;
        let fields: Vec<&str> = line
            .split(|character: char| {
                character.is_ascii_whitespace() || character == ',' || character == ';'
            })
            .filter(|field| !field.is_empty())
            .collect();
        if fields.len() < 3 {
            return Err(LoadError::InvalidData(format!(
                "line {} has fewer than three coordinates",
                line_number + 1
            )));
        }
        let coordinates: Option<Vec<f64>> =
            fields[..3].iter().map(|field| field.parse().ok()).collect();
        let Some(coordinates) = coordinates else {
            // Allow a single header row such as x,y,z,r,g,b.
            if !seen_points && fields[0].eq_ignore_ascii_case("x") {
                continue;
            }
            return Err(LoadError::InvalidData(format!(
                "invalid coordinate on line {}",
                line_number + 1
            )));
        };
        let has_intensity = fields.len() == 4 || fields.len() >= 7;
        let intensity = if has_intensity {
            let raw = fields[3].parse::<f64>().map_err(|_| {
                LoadError::InvalidData(format!("invalid intensity on line {}", line_number + 1))
            })?;
            let normalized = if raw < 0.0 {
                (raw + 2048.0) / 4095.0
            } else if raw <= 1.0 {
                raw
            } else {
                raw / 255.0
            };
            Some((normalized.clamp(0.0, 1.0) * 65535.0).round() as u16)
        } else {
            None
        };
        let rgb = if fields.len() >= 6 {
            let start = if has_intensity { 4 } else { 3 };
            let channels: Vec<u8> = fields[start..start + 3]
                .iter()
                .map(|field| field.parse().ok())
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(|| {
                    LoadError::InvalidData(format!("invalid RGB color on line {}", line_number + 1))
                })?;
            Some([channels[0], channels[1], channels[2]])
        } else {
            None
        };
        push(Point {
            xyz: [coordinates[0], coordinates[1], coordinates[2]],
            rgb,
            intensity,
            classification: None,
        })?;
        seen_points = true;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reservoir_keeps_bound_and_count() {
        let mut collector = Collector::new(3);
        for index in 0..10_000 {
            collector
                .push(Point {
                    xyz: [index as f64, 0.0, 0.0],
                    rgb: None,
                    intensity: None,
                    classification: None,
                })
                .unwrap();
        }
        let cloud = collector.finish(PathBuf::from("test.xyz")).unwrap();
        assert_eq!(cloud.total_points, 10_000);
        assert_eq!(cloud.points.len(), 3);
        assert_eq!(cloud.bounds.min, [0.0, 0.0, 0.0]);
        assert_eq!(cloud.bounds.max, [9_999.0, 0.0, 0.0]);
    }

    #[test]
    fn station_lookup_searches_the_scan_ranges() {
        let mut collector = Collector::new(10);
        for index in 0..10 {
            collector
                .push(Point {
                    xyz: [f64::from(index), 0.0, 0.0],
                    rgb: None,
                    intensity: None,
                    classification: None,
                })
                .unwrap();
        }
        let mut cloud = collector.finish(PathBuf::from("scans.xyz")).unwrap();
        let pose = |x: f64| ScanPose {
            label: format!("Scan {x}"),
            position: [x, 0.0, 0.0],
            axes: None,
        };
        assert!(cloud.scan_ranges.is_empty());
        assert_eq!(cloud.station_of(0), None);

        // One file per station: every point is its own, without ranges.
        cloud.scan_poses = vec![pose(1.0)];
        assert_eq!(cloud.station_of(0), Some(0));
        assert_eq!(cloud.station_of(9), Some(0));
        assert_eq!(cloud.station_of(10), None);
        assert_eq!(cloud.station_of(u64::MAX), None);

        // Several stations and nothing that tells their points apart.
        cloud.scan_poses = vec![pose(1.0), pose(2.0), pose(3.0)];
        assert!((0..10).all(|ordinal| cloud.station_of(ordinal).is_none()));

        let range = |first_ordinal, station| ScanRange {
            first_ordinal,
            station,
        };
        cloud.scan_ranges = vec![
            range(0, Some(0)),
            // A scan without a station.
            range(3, None),
            // An empty scan begins where the next one does.
            range(5, Some(1)),
            range(5, Some(2)),
            // A station the cloud does not have.
            range(8, Some(9)),
        ];
        let stations: Vec<_> = (0..11).map(|ordinal| cloud.station_of(ordinal)).collect();
        assert_eq!(
            stations,
            [
                Some(0),
                Some(0),
                Some(0),
                None,
                None,
                Some(2),
                Some(2),
                Some(2),
                None,
                None,
                None
            ]
        );
        assert_eq!(cloud.station_pose(2).unwrap().position, [1.0, 0.0, 0.0]);
        assert_eq!(cloud.station_pose(7).unwrap().position, [3.0, 0.0, 0.0]);
        assert!(cloud.station_pose(4).is_none());
        assert!(cloud.station_pose(u64::MAX).is_none());

        // A single station does not claim the points of a scan without one.
        cloud.scan_poses = vec![pose(1.0)];
        cloud.scan_ranges = vec![range(0, None), range(4, Some(0))];
        assert_eq!(cloud.station_of(3), None);
        assert_eq!(cloud.station_of(4), Some(0));

        // A provisional cloud tells no station: its count is the stated one
        // and its ordinals are not checked against it.
        assert!(cloud.scan_ranges_known());
        cloud.provisional = true;
        assert!(!cloud.scan_ranges_known());
        assert_eq!(cloud.station_of(4), None);
        assert!(cloud.station_pose(4).is_none());
        assert!(cloud.read_scan_ranges(|_| Ok(())).is_err());
    }

    #[test]
    fn open_reports_bounded_progress_and_allows_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("progress.xyz");
        std::fs::write(&path, "1 2 3\n".repeat(65_537)).unwrap();
        let mut updates = Vec::new();
        let cloud = open_with_progress(&path, 10, |count| {
            updates.push(count);
            Ok(())
        })
        .unwrap();
        assert_eq!(updates, [65_536, 65_537]);
        assert_eq!(cloud.total_points, 65_537);
        assert_eq!(cloud.points.len(), 10);
        assert!(matches!(
            open_with_progress(&path, 10, |_| Err(LoadError::Cancelled)),
            Err(LoadError::Cancelled)
        ));
    }

    #[test]
    fn parses_text_header_and_color() {
        let path = std::env::temp_dir().join(format!("pointcloud-core-{}.csv", std::process::id()));
        std::fs::write(&path, "x,y,z,r,g,b\n1,2,3,12,34,56\n4,5,6,77,88,99\n").unwrap();
        let cloud = open(&path, 1).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(cloud.total_points, 2);
        assert_eq!(cloud.bounds.min, [1.0, 2.0, 3.0]);
        assert_eq!(cloud.bounds.max, [4.0, 5.0, 6.0]);
        assert!(cloud.has_rgb);
    }

    #[test]
    fn parses_pts_intensity_before_rgb() {
        let path = std::env::temp_dir().join(format!("pointcloud-core-{}.pts", std::process::id()));
        std::fs::write(&path, "1\n1 2 3 128 10 20 30\n").unwrap();
        let cloud = open(&path, 10).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(cloud.total_points, 1);
        assert_eq!(cloud.points[0].rgb, Some([10, 20, 30]));
        assert!(cloud.points[0].intensity.is_some());
    }

    #[test]
    fn reads_las_and_laz_color_and_classification() {
        for extension in ["las", "laz"] {
            let path = std::env::temp_dir().join(format!(
                "pointcloud-core-{}-{extension}.{extension}",
                std::process::id()
            ));
            let mut builder = las::Builder::from((1, 2));
            builder.point_format = las::point::Format::new(2).unwrap();
            let header = builder.into_header().unwrap();
            let mut writer = las::Writer::from_path(&path, header).unwrap();
            writer
                .write_point(las::Point {
                    x: 12.0,
                    y: 34.0,
                    z: 56.0,
                    color: Some(las::Color {
                        red: 65535,
                        green: 0,
                        blue: 32768,
                    }),
                    classification: las::point::Classification::Ground,
                    intensity: 1234,
                    ..las::Point::default()
                })
                .unwrap();
            drop(writer);

            let quick = open_las_header(&path).unwrap();
            assert_eq!(quick.total_points, 1);
            assert_eq!(quick.bounds.min, [12.0, 34.0, 56.0]);
            assert!(quick.points.is_empty());
            quick.validate_source().unwrap();

            let preview = open_las_preview(&path, 1).unwrap();
            assert!(quick.same_source_revision(&preview));
            assert_eq!(preview.total_points, 1);
            assert_eq!(preview.bounds, quick.bounds);
            assert_eq!(preview.points.len(), 1);

            let cloud = open(&path, 10).unwrap();
            assert!(cloud.same_source_revision(&quick));
            assert_eq!(cloud.total_points, 1);
            assert_eq!(cloud.points[0].xyz, [12.0, 34.0, 56.0]);
            assert_eq!(cloud.points[0].rgb, Some([255, 0, 127]));
            assert_eq!(cloud.points[0].classification, Some(2));
            assert_eq!(cloud.points[0].intensity, Some(1234));

            let exported = path.with_extension("ply");
            export_full(&cloud, &exported, ExportFormat::PlyBinary).unwrap();
            let reopened = open(&exported, 10).unwrap();
            assert_eq!(reopened.points[0].classification, Some(2));
            assert_eq!(reopened.points[0].intensity, Some(1234));
            assert_eq!(reopened.points[0].rgb, Some([255, 0, 127]));
            std::fs::remove_file(path).unwrap();
            std::fs::remove_file(exported).unwrap();
        }
    }

    #[test]
    fn las_and_laz_batches_keep_point_order_and_stop_at_callback_error() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["las", "laz"] {
            let path = dir.path().join(format!("ordered.{extension}"));
            let mut builder = las::Builder::from((1, 2));
            builder.point_format = las::point::Format::new(2).unwrap();
            let mut writer = las::Writer::from_path(&path, builder.into_header().unwrap()).unwrap();
            for index in 0..7 {
                writer
                    .write_point(las::Point {
                        x: index as f64,
                        intensity: 100 + index as u16,
                        color: Some(las::Color {
                            red: index as u16 * 257,
                            green: 0,
                            blue: 0,
                        }),
                        ..las::Point::default()
                    })
                    .unwrap();
            }
            drop(writer);

            let mut reader = las::Reader::from_path(&path).unwrap();
            let mut points = Vec::new();
            read_las_batches(
                &mut reader,
                &mut |point| {
                    points.push(point);
                    Ok(())
                },
                2,
            )
            .unwrap();
            assert_eq!(points.len(), 7);
            for (index, point) in points.iter().enumerate() {
                assert_eq!(point.xyz[0], index as f64);
                assert_eq!(point.intensity, Some(100 + index as u16));
                assert_eq!(point.rgb, Some([index as u8, 0, 0]));
            }

            let mut reader = las::Reader::from_path(&path).unwrap();
            let mut visited = Vec::new();
            let stopped = read_las_batches(
                &mut reader,
                &mut |point| {
                    visited.push(point.xyz[0]);
                    if visited.len() == 3 {
                        Err(LoadError::Cancelled)
                    } else {
                        Ok(())
                    }
                },
                2,
            );
            assert!(matches!(stopped, Err(LoadError::Cancelled)));
            assert_eq!(visited, [0.0, 1.0, 2.0]);
        }
    }

    #[test]
    fn las_preview_samples_across_entire_source() {
        let dir = tempfile::tempdir().unwrap();
        for extension in ["las", "laz"] {
            let path = dir.path().join(format!("spread.{extension}"));
            let header = las::Builder::from((1, 2)).into_header().unwrap();
            let mut writer = las::Writer::from_path(&path, header).unwrap();
            for x in 0..1_000 {
                writer
                    .write_point(las::Point {
                        x: f64::from(x),
                        ..las::Point::default()
                    })
                    .unwrap();
            }
            drop(writer);
            let preview = open_las_preview(&path, 40).unwrap();
            assert_eq!(preview.total_points, 1_000);
            assert_eq!(preview.points.len(), 40);
            assert_eq!(preview.point_ordinals.len(), preview.points.len());
            for (point, ordinal) in preview.points.iter().zip(&preview.point_ordinals) {
                if extension == "laz" {
                    assert_eq!(*ordinal, u64::MAX);
                } else {
                    assert_eq!(point.xyz[0] as u64, *ordinal);
                }
            }
            assert_eq!(preview.bounds.min[0], 0.0);
            assert_eq!(preview.bounds.max[0], 999.0);
            assert_eq!(preview.points.first().unwrap().xyz[0], 0.0);
            // LAZ chunk seeking may land a few points before the requested
            // ordinal; preview coverage still reaches the far end.
            assert!(preview.points.last().unwrap().xyz[0] > 900.0);
        }
    }
}
