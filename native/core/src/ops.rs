//! Whole operations on files, as the command line, the cloud API and the C
//! library run them: one function each, so that none of them has a
//! calculation of its own.
//!
//! The bodies are stubs: the signatures are fixed first, so that the callers
//! can be written against them.

use std::path::{Path, PathBuf};

use crate::{
    DrawingFormat, DrawingRequest, DrawingStats, DrawingUnits, DrawingView, ExportFormat,
    IndexProgress, LoadError, MeshStats, OctreeIndex, OrientedBox, PointCloud, ScanPose,
    SurfaceMeshConfig,
};

/// Asked between the steps of a long operation; an error (normally
/// `LoadError::Cancelled`) stops it before anything is put in place.
pub type KeepGoing<'a> = &'a (dyn Fn() -> Result<(), LoadError> + Sync);

/// The `KeepGoing` of the command line: never stops.
pub fn never_stop() -> Result<(), LoadError> {
    Ok(())
}

/// The extensions of the point-cloud files the operations read.
pub const POINT_FORMATS: &[&str] = &[
    "las", "laz", "e57", "ply", "pcd", "ptx", "pts", "xyz", "asc", "txt", "csv",
];
/// The keys of the formats the operations write.
pub const EXPORT_FORMATS: &[&str] = &["las", "laz", "e57", "ply", "xyz", "pts", "csv"];

/// A LAS or LAZ file from its header, any other format with one full pass
/// (`open_with_progress(path, 1, …)`, which `open(path, 1)` is).
pub fn open_source(path: &Path, keep_going: KeepGoing<'_>) -> Result<PointCloud, LoadError> {
    let _ = (path, keep_going);
    todo!("A1")
}

/// The format a destination's extension names; `.ply` is binary, as the
/// command line writes it.
pub fn export_format_for_path(path: &Path) -> Option<ExportFormat> {
    let _ = path;
    todo!("A1")
}

/// The format of one of the keys of `EXPORT_FORMATS`.
pub fn export_format_for_key(key: &str) -> Option<ExportFormat> {
    let _ = key;
    todo!("A1")
}

/// What a source holds, as the cloud `inspect` tool and the C library report it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Inspection {
    /// The extension, lower case.
    pub format: String,
    pub points: u64,
    pub bounds: InspectionBounds,
    pub attributes: Attributes,
    pub stations: Vec<ScanPose>,
    pub station_photos: usize,
}

/// The bounds of a source, in its coordinates.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub struct InspectionBounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Which attributes the points of a source carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Attributes {
    pub rgb: bool,
    pub intensity: bool,
    pub classification: bool,
}

pub fn inspect(cloud: &PointCloud) -> Inspection {
    let _ = cloud;
    todo!("A1")
}

/// What `convert` writes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Convert {
    pub format: ExportFormat,
    pub section: Option<OrientedBox>,
    /// Most points the result may hold; more is refused (`is_too_many_points`).
    /// Without a section the count is checked before anything is written;
    /// with one, points past the limit are not written and the file is removed.
    pub max_points: Option<u64>,
}

/// An upper bound of the bytes one point takes in `format` for this cloud
/// (record length for LAS/LAZ/E57, the property layout for PLY, a stated
/// bound per line for text), used to turn a byte limit into `max_points`.
pub fn bytes_per_point(format: ExportFormat, cloud: &PointCloud) -> u64 {
    let _ = (format, cloud);
    todo!("A1")
}

/// `export_full` (count = `total_points`) or `export_section` (its count);
/// with `max_points`, `export_section_where` with a counting `include`.
/// `keep_going` is asked before the export starts; export itself cannot be
/// stopped in this version.
pub fn convert(
    cloud: &PointCloud,
    destination: &Path,
    convert: Convert,
    keep_going: KeepGoing<'_>,
) -> Result<u64, LoadError> {
    let _ = (cloud, destination, convert, keep_going);
    todo!("A1")
}

/// `open_source`, then `convert`.
pub fn convert_file(
    source: &Path,
    destination: &Path,
    convert: Convert,
    keep_going: KeepGoing<'_>,
) -> Result<u64, LoadError> {
    let _ = (source, destination, convert, keep_going);
    todo!("A1")
}

pub const TOO_MANY_POINTS: &str = "the result would hold more points than allowed";

/// Whether `error` is the refusal of a result past `Convert::max_points`.
pub fn is_too_many_points(error: &LoadError) -> bool {
    let _ = error;
    todo!("A1")
}

/// The choices of a section drawing; what is not given takes the default of
/// the view.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SectionDrawingOptions {
    /// Default plan.
    pub view: DrawingView,
    /// Default `DEFAULT_SLAB_THICKNESS`.
    pub thickness: Option<f64>,
    /// Default millimetres.
    pub units: Option<DrawingUnits>,
    /// Default `DrawingRequest::for_view(view).fill`.
    pub fill: Option<bool>,
}

impl SectionDrawingOptions {
    /// `DrawingRequest { thickness, units, fill, ..DrawingRequest::for_view(view) }`, validated.
    pub fn request(&self) -> Result<DrawingRequest, LoadError> {
        todo!("A1")
    }
}

/// Whether a section drawing may read through an index that the cache
/// already holds for the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachedIndex {
    IfPresent,
    Never,
}

/// What `section_drawing_file` wrote.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SectionDrawing {
    pub stats: DrawingStats,
    pub request: DrawingRequest,
    pub slab_thickness: f64,
    pub format: DrawingFormat,
}

/// What `drawing::command_line` did between parsing and printing: check the
/// slab, open the source, take a cached index when asked and present, read
/// through one `DrawingSource` (identity transform, every point accepted,
/// point layer named `layer`), `section_drawing`, `write_drawing_progress`,
/// and the hint of an empty slab. `keep_going` is asked by every progress step.
pub fn section_drawing_file(
    source: &Path,
    layer: &str,
    destination: &Path,
    section: OrientedBox,
    options: &SectionDrawingOptions,
    cached: CachedIndex,
    keep_going: KeepGoing<'_>,
) -> Result<SectionDrawing, LoadError> {
    let _ = (
        source,
        layer,
        destination,
        section,
        options,
        cached,
        keep_going,
    );
    todo!("A1")
}

pub const EMPTY_SLAB: &str = "the slab holds no points";

/// What to change when the slab of `view` holds no points.
pub fn empty_slab_hint(view: DrawingView) -> &'static str {
    let _ = view;
    todo!("A1")
}

/// `error` with the hint of `empty_slab_hint` when it is `EMPTY_SLAB`.
pub fn hint_empty_slab(error: LoadError, view: DrawingView) -> LoadError {
    let _ = (error, view);
    todo!("A1")
}

/// `mesh_terrain_obj_where_progress(.., MeshConfig::default(), |_, _| true, |_| keep_going())`
pub fn terrain_mesh_file(
    source: &Path,
    destination: &Path,
    keep_going: KeepGoing<'_>,
) -> Result<MeshStats, LoadError> {
    let _ = (source, destination, keep_going);
    todo!("A1")
}

/// `mesh_surface_obj_where_progress(.., config, |_, _| true, |_| keep_going())`
pub fn surface_mesh_file(
    source: &Path,
    destination: &Path,
    config: SurfaceMeshConfig,
    keep_going: KeepGoing<'_>,
) -> Result<MeshStats, LoadError> {
    let _ = (source, destination, config, keep_going);
    todo!("A1")
}

/// How `build_index` keeps the index.
#[derive(Debug, Clone, Default)]
pub struct IndexOptions {
    pub cached: bool,
    pub scratch_dir: Option<PathBuf>,
}

/// `build_cached_with_progress` when `cached` (and the caches are on), else
/// `build_with_progress` with `scratch_dir` as temporary storage.
pub fn build_index(
    cloud: &PointCloud,
    options: &IndexOptions,
    progress: &mut dyn FnMut(IndexProgress) -> Result<(), LoadError>,
) -> Result<OctreeIndex, LoadError> {
    let _ = (cloud, options, progress);
    todo!("A1")
}

/// The shape of an index, as `--index` prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct IndexSummary {
    pub points: u64,
    pub nodes: u64,
    pub leaves: u64,
    pub depth: u8,
}

/// The walk `--index` prints.
pub fn index_summary(index: &OctreeIndex) -> IndexSummary {
    let _ = index;
    todo!("A1")
}
