//! Whole operations on files, as the command line, the cloud API and the C
//! library run them: one function each, so that none of them has a
//! calculation of its own.
//!
//! With `max_points: None` and `never_stop`, each function makes the same
//! calls of the core that the command line made before it had this module.
//! Nothing here formats a message for people; the callers do.

use std::fs;
use std::path::{Path, PathBuf};

use crate::region_source::{RegionSource, SourceTransform};
use crate::{
    export, DrawingFormat, DrawingProgress, DrawingRequest, DrawingSource, DrawingStats,
    DrawingUnits, DrawingView, ExportFormat, IndexConfig, IndexProgress, LoadError, MeshConfig,
    MeshStats, OctreeIndex, OrientedBox, PointCloud, ScanPose, SurfaceMeshConfig,
    DEFAULT_SLAB_THICKNESS,
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

/// Whether `path` names a LAS or LAZ file, which opens from its header.
fn is_las(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
        })
}

/// A LAS or LAZ file from its header, any other format with one full pass
/// (`open_with_progress(path, 1, …)`, which `open(path, 1)` is).
pub fn open_source(path: &Path, keep_going: KeepGoing<'_>) -> Result<PointCloud, LoadError> {
    if is_las(path) {
        keep_going()?;
        crate::open_las_header(path)
    } else {
        crate::open_with_progress(path, 1, |_| keep_going())
    }
}

/// The format a destination's extension names; `.ply` is binary, as the
/// command line writes it.
pub fn export_format_for_path(path: &Path) -> Option<ExportFormat> {
    export_format_for_key(&path.extension()?.to_str()?.to_ascii_lowercase())
}

/// The format of one of the keys of `EXPORT_FORMATS`.
pub fn export_format_for_key(key: &str) -> Option<ExportFormat> {
    match key {
        "ply" => Some(ExportFormat::PlyBinary),
        "xyz" => Some(ExportFormat::Xyz),
        "pts" => Some(ExportFormat::Pts),
        "csv" => Some(ExportFormat::Csv),
        "las" => Some(ExportFormat::Las),
        "laz" => Some(ExportFormat::Laz),
        "e57" => Some(ExportFormat::E57),
        _ => None,
    }
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

/// What `cloud` holds: its count and bounds, its attributes, its scanner
/// stations and the photos stored with them.
pub fn inspect(cloud: &PointCloud) -> Inspection {
    Inspection {
        format: cloud
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase(),
        points: cloud.total_points,
        bounds: InspectionBounds {
            min: cloud.bounds.min,
            max: cloud.bounds.max,
        },
        attributes: Attributes {
            rgb: cloud.has_rgb,
            intensity: cloud.has_intensity,
            classification: cloud.has_classification,
        },
        stations: cloud.scan_poses.clone(),
        station_photos: cloud.scan_images.len(),
    }
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

/// What the header, the metadata and the tables of a LAS, LAZ or E57 export
/// take at most besides what the source brings along.
const FIXED_BYTES: u64 = 64 * 1024;

/// The longest text `{}` writes for a coordinate within `bounds`: a sign,
/// the digits before the point, the point and twenty decimals, which holds
/// any coordinate that is zero or at least 0.001 in size.
fn coordinate_chars(bounds: crate::Bounds) -> u64 {
    let largest = bounds
        .min
        .iter()
        .chain(&bounds.max)
        .map(|value| value.abs())
        .fold(0.0, f64::max);
    let digits = if largest.is_finite() && largest >= 1.0 {
        largest.log10().floor() as u64 + 1
    } else {
        1
    };
    1 + digits + 1 + 20
}

/// An upper bound of the bytes one point takes in `format` for this cloud
/// (record length for LAS/LAZ/E57, the property layout for PLY, a stated
/// bound per line for text), used to turn a byte limit into `max_points`.
///
/// The header and the other fixed parts of the file are spread over the
/// points of the cloud, so that the bound times `total_points` holds the
/// file a full conversion writes. A section writes the same fixed parts for
/// fewer points, so a caller with a hard limit also checks the file it got.
/// LAZ is bounded by its record length before compression, and text by the
/// longest number of each field for the bounds of the cloud.
pub fn bytes_per_point(format: ExportFormat, cloud: &PointCloud) -> u64 {
    let source_bytes = || fs::metadata(&cloud.path).map_or(0, |metadata| metadata.len());
    let source_is_e57 = cloud
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("e57"));
    let points = cloud.total_points.max(1);
    let coordinate = coordinate_chars(cloud.bounds);
    let (record, fixed) = match format {
        ExportFormat::Las | ExportFormat::Laz => {
            // A LAS or LAZ source passes its header and its variable length
            // records on, or is copied whole.
            let source = if is_las(&cloud.path) {
                source_bytes()
            } else {
                0
            };
            (export::las_point_bytes(cloud), FIXED_BYTES + source)
        }
        ExportFormat::E57 if source_is_e57 => {
            // An E57 source keeps its own fields and metadata, or is copied
            // whole.
            let source = source_bytes();
            (source.div_ceil(points) + 1, FIXED_BYTES + source)
        }
        // A page of 1,024 bytes carries 4 of checksum and every packet a
        // header: one byte a point covers both.
        ExportFormat::E57 => (export::e57_point_bytes(cloud) + 1, FIXED_BYTES),
        ExportFormat::PlyBinary => (
            export::ply_binary_point_bytes(cloud),
            export::ply_header_bytes(cloud, format),
        ),
        ExportFormat::PlyAscii => (
            3 * coordinate
                + 2
                + 12 * u64::from(cloud.has_rgb)
                + 6 * u64::from(cloud.has_intensity)
                + 4 * u64::from(cloud.has_classification)
                + 1,
            export::ply_header_bytes(cloud, format),
        ),
        ExportFormat::Xyz | ExportFormat::Pts | ExportFormat::Csv => {
            let classification = format == ExportFormat::Csv && cloud.has_classification;
            let record = 3 * coordinate
                + 2
                + 13 * u64::from(cloud.has_intensity)
                + 12 * u64::from(cloud.has_rgb)
                + 4 * u64::from(classification)
                + 1;
            let fixed = match format {
                // The count of points on a line of its own.
                ExportFormat::Pts => 21,
                // The line of column names.
                ExportFormat::Csv => 64,
                _ => 0,
            };
            (record, fixed)
        }
    };
    record + fixed.div_ceil(points)
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
    keep_going()?;
    let Convert {
        format,
        section,
        max_points,
    } = convert;
    match (section, max_points) {
        (None, limit) => {
            if limit.is_some_and(|limit| cloud.total_points > limit) {
                return Err(too_many_points());
            }
            crate::export_full(cloud, destination, format)?;
            Ok(cloud.total_points)
        }
        (Some(section), None) => crate::export_section(cloud, destination, format, section),
        (Some(section), Some(limit)) => {
            let mut inside = 0u64;
            let written =
                crate::export_section_where(cloud, destination, format, section, |_, _| {
                    inside += 1;
                    inside <= limit
                })?;
            if inside > limit {
                fs::remove_file(destination)?;
                return Err(too_many_points());
            }
            Ok(written)
        }
    }
}

/// `open_source`, then `convert`.
pub fn convert_file(
    source: &Path,
    destination: &Path,
    convert: Convert,
    keep_going: KeepGoing<'_>,
) -> Result<u64, LoadError> {
    let cloud = open_source(source, keep_going)?;
    self::convert(&cloud, destination, convert, keep_going)
}

pub const TOO_MANY_POINTS: &str = "the result would hold more points than allowed";

fn too_many_points() -> LoadError {
    LoadError::InvalidData(TOO_MANY_POINTS.into())
}

/// Whether `error` is the refusal of a result past `Convert::max_points`.
pub fn is_too_many_points(error: &LoadError) -> bool {
    matches!(error, LoadError::InvalidData(reason) if reason == TOO_MANY_POINTS)
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
        let defaults = DrawingRequest::for_view(self.view);
        let request = DrawingRequest {
            thickness: Some(self.thickness.unwrap_or(DEFAULT_SLAB_THICKNESS)),
            units: self.units.unwrap_or_default(),
            fill: self.fill.unwrap_or(defaults.fill),
            ..defaults
        };
        request.validate()?;
        Ok(request)
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
    let request = options.request()?;
    let format = DrawingFormat::from_path(destination).ok_or_else(|| {
        LoadError::UnsupportedFormat(
            destination
                .extension()
                .map(|extension| extension.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
    })?;
    let slab_thickness =
        crate::slab_from_section(section, request.view, request.thickness, request.origin)?
            .thickness;
    let cloud = open_source(source, keep_going)?;
    let index = match cached {
        CachedIndex::IfPresent => {
            OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())
                .ok()
                .flatten()
        }
        CachedIndex::Never => None,
    };
    let stats = (|| {
        // An index is read without a look at its source, so the source is
        // checked first, as the window's job does.
        if index.is_some() {
            cloud.validate_source()?;
        }
        let sources = [DrawingSource {
            source: RegionSource::new(&cloud, index.as_ref(), SourceTransform::default()),
            name: layer,
        }];
        let accept = |_: usize, _: u64, _: &crate::Point| true;
        let (drawing, mut stats) = crate::section_drawing(
            &sources,
            section,
            &request,
            &accept,
            &mut |_: DrawingProgress| keep_going(),
        )?;
        stats.bytes =
            crate::write_drawing_progress(&drawing, destination, format, request.version, |_| {
                keep_going()
            })?;
        Ok(stats)
    })()
    .map_err(|error| hint_empty_slab(error, request.view))?;
    Ok(SectionDrawing {
        stats,
        request,
        slab_thickness,
        format,
    })
}

/// The reason the core gives for a slab without points.
pub const EMPTY_SLAB: &str = "the slab holds no points";

/// What to change when the slab of `view` holds no points: which face of the
/// box the cut plane is. With a box drawn around a whole building, the face
/// that a vertical view cuts at often lies outside it.
pub fn empty_slab_hint(view: DrawingView) -> &'static str {
    match view {
        DrawingView::Plan => "the cut plane is the top face of the section box; move it down into the walls or make the slab thicker",
        DrawingView::Front => "the cut plane is the front face of the section box (Y min); move that face onto the building or make the slab thicker",
        DrawingView::Back => "the cut plane is the back face of the section box (Y max); move that face onto the building or make the slab thicker",
        DrawingView::Left => "the cut plane is the left face of the section box (X min); move that face onto the building or make the slab thicker",
        DrawingView::Right => "the cut plane is the right face of the section box (X max); move that face onto the building or make the slab thicker",
    }
}

/// `error` with the hint of `empty_slab_hint` when it is `EMPTY_SLAB`.
pub fn hint_empty_slab(error: LoadError, view: DrawingView) -> LoadError {
    match error {
        LoadError::InvalidData(reason) if reason == EMPTY_SLAB => {
            LoadError::InvalidData(format!("{EMPTY_SLAB}: {}", empty_slab_hint(view)))
        }
        other => other,
    }
}

/// `mesh_terrain_obj_where_progress(.., MeshConfig::default(), |_, _| true, |_| keep_going())`
pub fn terrain_mesh_file(
    source: &Path,
    destination: &Path,
    keep_going: KeepGoing<'_>,
) -> Result<MeshStats, LoadError> {
    let cloud = open_source(source, keep_going)?;
    crate::mesh_terrain_obj_where_progress(
        &cloud,
        destination,
        MeshConfig::default(),
        |_, _| true,
        |_| keep_going(),
    )
}

/// `mesh_surface_obj_where_progress(.., config, |_, _| true, |_| keep_going())`
pub fn surface_mesh_file(
    source: &Path,
    destination: &Path,
    config: SurfaceMeshConfig,
    keep_going: KeepGoing<'_>,
) -> Result<MeshStats, LoadError> {
    let cloud = open_source(source, keep_going)?;
    crate::mesh_surface_obj_where_progress(
        &cloud,
        destination,
        config,
        |_, _| true,
        |_| keep_going(),
    )
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
    if options.cached && crate::disk_caches() {
        OctreeIndex::build_cached_with_progress(cloud, IndexConfig::default(), progress)
    } else {
        let config = IndexConfig {
            scratch_dir: options.scratch_dir.clone(),
            ..IndexConfig::default()
        };
        OctreeIndex::build_with_progress(cloud, config, progress)
    }
}

/// The shape of an index, as `--index` prints it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct IndexSummary {
    pub points: u64,
    pub nodes: u64,
    pub leaves: u64,
    pub depth: u8,
}

/// The walk `--index` prints: every node, the leaves among them and the
/// deepest level.
pub fn index_summary(index: &OctreeIndex) -> IndexSummary {
    let mut summary = IndexSummary {
        points: index.root.total_points,
        nodes: 0,
        leaves: 0,
        depth: 0,
    };
    let mut pending = vec![&index.root];
    while let Some(node) = pending.pop() {
        summary.nodes += 1;
        summary.depth = summary.depth.max(node.depth);
        if node.is_leaf() {
            summary.leaves += 1;
        }
        pending.extend(&node.children);
    }
    summary
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::Bounds;

    /// A 10 by 10 by 10 grid of coloured points 0.25 apart, as ASCII PLY.
    fn write_grid(path: &Path) {
        let mut text = String::from("ply\nformat ascii 1.0\nelement vertex 1000\n");
        text.push_str("property float x\nproperty float y\nproperty float z\n");
        text.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
        text.push_str("end_header\n");
        for x in 0..10 {
            for y in 0..10 {
                for z in 0..10 {
                    let at = |step: i32| f64::from(step) * 0.25;
                    text.push_str(&format!(
                        "{:.2} {:.2} {:.2} {} {} {}\n",
                        at(x),
                        at(y),
                        at(z),
                        x * 20,
                        y * 20,
                        z * 20
                    ));
                }
            }
        }
        fs::write(path, text).unwrap();
    }

    /// The walls of a room of 4 by 3 m and 2.5 m high, a point every 0.1 m.
    fn write_room(path: &Path) {
        let mut text = String::new();
        for level in 0..=25 {
            let z = f64::from(level) * 0.1;
            for step in 0..=40 {
                let x = f64::from(step) * 0.1;
                text.push_str(&format!("{x:.3} 0.000 {z:.3}\n{x:.3} 3.000 {z:.3}\n"));
            }
            for step in 1..30 {
                let y = f64::from(step) * 0.1;
                text.push_str(&format!("0.000 {y:.3} {z:.3}\n4.000 {y:.3} {z:.3}\n"));
            }
        }
        fs::write(path, text).unwrap();
    }

    /// A PTX scan of 10 by 10 points from one station with a pose.
    fn write_station(path: &Path) {
        let mut text = String::from("10\n10\n1.5 2.5 1.75\n0 1 0\n-1 0 0\n0 0 1\n");
        text.push_str("0 1 0 0\n-1 0 0 0\n0 0 1 0\n1.5 2.5 1.75 1\n");
        for column in 0..10 {
            for row in 0..10 {
                text.push_str(&format!(
                    "{} {} 1 0.5 100 150 200\n",
                    f64::from(column) * 0.1 + 0.05,
                    f64::from(row) * 0.1 + 0.05
                ));
            }
        }
        fs::write(path, text).unwrap();
    }

    /// A box of the grid that holds half of its points: x up to 1.1.
    fn half_of_the_grid() -> OrientedBox {
        OrientedBox::new(
            Bounds {
                min: [-1.0, -1.0, -1.0],
                max: [1.1, 10.0, 10.0],
            },
            0.0,
        )
    }

    fn turned_room_box() -> OrientedBox {
        OrientedBox::new(
            Bounds {
                min: [0.5, -0.5, 0.2],
                max: [3.5, 2.0, 2.2],
            },
            30.0,
        )
    }

    fn convert_as(format: ExportFormat) -> Convert {
        Convert {
            format,
            section: None,
            max_points: None,
        }
    }

    #[test]
    fn convert_file_writes_what_the_export_writes() {
        let directory = tempfile::tempdir().unwrap();
        let (grid, room) = (
            directory.path().join("grid.ply"),
            directory.path().join("room.xyz"),
        );
        write_grid(&grid);
        write_room(&room);
        for (source, section) in [(&grid, half_of_the_grid()), (&room, turned_room_box())] {
            let cloud = crate::open(source, 1).unwrap();
            for format in ExportFormat::ALL {
                if format == ExportFormat::E57 {
                    // Each E57 file has identifiers of its own.
                    continue;
                }
                let name = format!("{format:?}.{}", format.extension());
                let direct = directory.path().join(format!("direct-{name}"));
                let through = directory.path().join(format!("through-{name}"));
                crate::export_full(&cloud, &direct, format).unwrap();
                let count =
                    convert_file(source, &through, convert_as(format), &never_stop).unwrap();
                assert_eq!(count, cloud.total_points);
                assert_eq!(
                    fs::read(&direct).unwrap(),
                    fs::read(&through).unwrap(),
                    "{format}"
                );

                let direct_count = crate::export_section(&cloud, &direct, format, section).unwrap();
                let boxed = Convert {
                    section: Some(section),
                    ..convert_as(format)
                };
                let count = convert_file(source, &through, boxed, &never_stop).unwrap();
                assert_eq!(count, direct_count);
                assert!(0 < count && count < cloud.total_points, "{count}");
                assert_eq!(
                    fs::read(&direct).unwrap(),
                    fs::read(&through).unwrap(),
                    "{format}"
                );
            }
        }
    }

    #[test]
    fn a_point_limit_refuses_without_leaving_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let grid = directory.path().join("grid.ply");
        write_grid(&grid);
        let destination = directory.path().join("out.ply");
        let limited = |max_points, section| Convert {
            format: ExportFormat::PlyBinary,
            section,
            max_points,
        };

        // The whole grid is refused before anything is written.
        let error =
            convert_file(&grid, &destination, limited(Some(999), None), &never_stop).unwrap_err();
        assert!(is_too_many_points(&error), "{error}");
        assert!(!destination.exists());

        // A box past the limit writes no file either.
        let section = Some(half_of_the_grid());
        let error = convert_file(
            &grid,
            &destination,
            limited(Some(499), section),
            &never_stop,
        )
        .unwrap_err();
        assert!(is_too_many_points(&error), "{error}");
        assert!(!destination.exists());

        // At or above the count the file is the one without a limit.
        let free = directory.path().join("free.ply");
        for (max_points, section, count) in [(1000, None, 1000), (500, section, 500)] {
            let unlimited = limited(None, section);
            assert_eq!(
                convert_file(&grid, &free, unlimited, &never_stop).unwrap(),
                count
            );
            for max_points in [max_points, max_points + 1, u64::MAX] {
                let written = convert_file(
                    &grid,
                    &destination,
                    limited(Some(max_points), section),
                    &never_stop,
                )
                .unwrap();
                assert_eq!(written, count);
                assert_eq!(fs::read(&free).unwrap(), fs::read(&destination).unwrap());
            }
        }
        assert!(!is_too_many_points(&LoadError::InvalidData("other".into())));
        assert!(!is_too_many_points(&LoadError::Cancelled));
    }

    #[test]
    fn bytes_per_point_bounds_every_written_file() {
        let directory = tempfile::tempdir().unwrap();
        let (grid, room, station) = (
            directory.path().join("grid.ply"),
            directory.path().join("room.xyz"),
            directory.path().join("station.ptx"),
        );
        write_grid(&grid);
        write_room(&room);
        write_station(&station);
        let (las, laz, e57) = (
            directory.path().join("grid.las"),
            directory.path().join("grid.laz"),
            directory.path().join("grid.e57"),
        );
        let cloud = crate::open(&grid, 1).unwrap();
        crate::export_full(&cloud, &las, ExportFormat::Las).unwrap();
        crate::export_full(&cloud, &laz, ExportFormat::Laz).unwrap();
        crate::export_full(&cloud, &e57, ExportFormat::E57).unwrap();
        let out = directory.path().join("out");
        fs::create_dir(&out).unwrap();
        for source in [&grid, &room, &station, &las, &laz, &e57] {
            let cloud = open_source(source, &never_stop).unwrap();
            for format in ExportFormat::ALL {
                let destination = out.join(format!("result.{}", format.extension()));
                crate::export_full(&cloud, &destination, format).unwrap();
                let size = fs::metadata(&destination).unwrap().len();
                let bound = bytes_per_point(format, &cloud) * cloud.total_points;
                assert!(
                    bound >= size,
                    "{format} from {}: {bound} < {size}",
                    source.display()
                );
                fs::remove_file(&destination).unwrap();
            }
        }
    }

    #[test]
    fn every_export_key_names_its_format() {
        for key in EXPORT_FORMATS {
            let format = export_format_for_key(key).unwrap();
            assert_eq!(format.extension(), *key);
            let upper = format!("scan.{}", key.to_uppercase());
            assert_eq!(export_format_for_path(Path::new(&upper)), Some(format));
        }
        assert_eq!(export_format_for_key("ply"), Some(ExportFormat::PlyBinary));
        assert_eq!(export_format_for_key("dxf"), None);
        assert_eq!(export_format_for_path(Path::new("scan")), None);
        // Every format the operations write is one they also read.
        assert!(EXPORT_FORMATS.iter().all(|key| POINT_FORMATS.contains(key)));
    }

    #[test]
    fn section_drawing_options_take_the_defaults_of_the_view() {
        let request = SectionDrawingOptions::default().request().unwrap();
        assert_eq!(
            request,
            DrawingRequest {
                thickness: Some(0.10),
                ..DrawingRequest::for_view(DrawingView::Plan)
            }
        );
        for view in DrawingView::ALL {
            let options = SectionDrawingOptions {
                view,
                ..SectionDrawingOptions::default()
            };
            let request = options.request().unwrap();
            assert_eq!(
                request.fill,
                DrawingRequest::for_view(view).fill,
                "{view:?}"
            );
            assert_eq!(request.units, DrawingUnits::Millimetres);
            assert_eq!(request, DrawingRequest::for_view(view));
        }
        let chosen = SectionDrawingOptions {
            view: DrawingView::Front,
            thickness: Some(0.25),
            units: Some(DrawingUnits::Metres),
            fill: Some(true),
        };
        let request = chosen.request().unwrap();
        assert_eq!(
            (request.thickness, request.units, request.fill),
            (Some(0.25), DrawingUnits::Metres, true)
        );
        let thin = SectionDrawingOptions {
            thickness: Some(0.001),
            ..SectionDrawingOptions::default()
        };
        assert!(thin.request().is_err());
    }

    #[test]
    fn inspect_reports_a_grid_and_a_station() {
        let directory = tempfile::tempdir().unwrap();
        let (grid, station) = (
            directory.path().join("grid.ply"),
            directory.path().join("Station.PTX"),
        );
        write_grid(&grid);
        write_station(&station);

        let inspection = inspect(&open_source(&grid, &never_stop).unwrap());
        assert_eq!(
            inspection,
            Inspection {
                format: "ply".into(),
                points: 1000,
                bounds: InspectionBounds {
                    min: [0.0; 3],
                    max: [2.25; 3],
                },
                attributes: Attributes {
                    rgb: true,
                    intensity: false,
                    classification: false,
                },
                stations: Vec::new(),
                station_photos: 0,
            }
        );

        let inspection = inspect(&open_source(&station, &never_stop).unwrap());
        assert_eq!(inspection.format, "ptx");
        assert_eq!(inspection.points, 100);
        assert!(inspection.attributes.rgb && inspection.attributes.intensity);
        assert_eq!(inspection.stations.len(), 1);
        assert_eq!(inspection.stations[0].position, [1.5, 2.5, 1.75]);
        assert_eq!(
            inspection.stations[0].axes,
            Some([[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]])
        );
        assert_eq!(inspection.station_photos, 0);
        let json = serde_json::to_value(&inspection).unwrap();
        assert_eq!(json["bounds"]["min"].as_array().unwrap().len(), 3);
        assert_eq!(json["attributes"]["classification"], false);
        assert_eq!(json["stations"][0]["position"][2], 1.75);
    }

    #[test]
    fn index_summary_counts_the_tree() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("block.xyz");
        let mut file = std::io::BufWriter::new(fs::File::create(&source).unwrap());
        for x in 0..60 {
            for y in 0..60 {
                for z in 0..40 {
                    let at = |step: i32| f64::from(step) * 0.1;
                    writeln!(file, "{} {} {}", at(x), at(y), at(z)).unwrap();
                }
            }
        }
        drop(file);
        let cloud = crate::open(&source, 1).unwrap();
        let options = IndexOptions {
            cached: false,
            scratch_dir: Some(directory.path().to_path_buf()),
        };
        let index = build_index(&cloud, &options, &mut |_| Ok(())).unwrap();

        fn walk(node: &crate::IndexedNode, summary: &mut IndexSummary) {
            summary.nodes += 1;
            summary.depth = summary.depth.max(node.depth);
            summary.leaves += u64::from(node.children.is_empty());
            for child in &node.children {
                walk(child, summary);
            }
        }
        let mut expected = IndexSummary {
            points: 144_000,
            nodes: 0,
            leaves: 0,
            depth: 0,
        };
        walk(&index.root, &mut expected);
        assert_eq!(index_summary(&index), expected);
        assert!(expected.depth >= 1 && expected.leaves > 1, "{expected:?}");
    }

    /// A `keep_going` that answers `Cancelled` from its call `from` on, and
    /// counts how often it was asked.
    struct StopAt {
        from: usize,
        asked: AtomicUsize,
    }

    impl StopAt {
        fn new(from: usize) -> Self {
            Self {
                from,
                asked: AtomicUsize::new(0),
            }
        }

        fn ask(&self) -> Result<(), LoadError> {
            if self.asked.fetch_add(1, Ordering::Relaxed) >= self.from {
                Err(LoadError::Cancelled)
            } else {
                Ok(())
            }
        }

        fn asked(&self) -> usize {
            self.asked.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn a_keep_going_that_fails_stops_every_long_operation() {
        let directory = tempfile::tempdir().unwrap();
        let room = directory.path().join("room.xyz");
        write_room(&room);
        let obj = directory.path().join("room.obj");
        let dxf = directory.path().join("room.dxf");
        let plan = OrientedBox::from(Bounds {
            min: [-0.5, -0.5, 0.0],
            max: [4.5, 3.5, 1.2],
        });
        type Run<'a> = Box<dyn Fn(KeepGoing<'_>) -> Result<(), LoadError> + 'a>;
        let operations: [(&str, Option<&Path>, Run<'_>); 4] = [
            (
                "open",
                None,
                Box::new(|keep_going| open_source(&room, keep_going).map(drop)),
            ),
            (
                "terrain",
                Some(&obj),
                Box::new(|keep_going| terrain_mesh_file(&room, &obj, keep_going).map(drop)),
            ),
            (
                "surface",
                Some(&obj),
                Box::new(|keep_going| {
                    surface_mesh_file(&room, &obj, SurfaceMeshConfig::default(), keep_going)
                        .map(drop)
                }),
            ),
            (
                "drawing",
                Some(&dxf),
                Box::new(|keep_going| {
                    section_drawing_file(
                        &room,
                        "room",
                        &dxf,
                        plan,
                        &SectionDrawingOptions::default(),
                        CachedIndex::Never,
                        keep_going,
                    )
                    .map(drop)
                }),
            ),
        ];
        let opening = StopAt::new(usize::MAX);
        open_source(&room, &|| opening.ask()).unwrap();
        for (name, destination, run) in operations {
            // Count the questions of a run that goes to its end.
            let counting = StopAt::new(usize::MAX);
            run(&|| counting.ask()).unwrap();
            let asked = counting.asked();
            assert!(asked >= 1, "{name}");
            if let Some(destination) = destination {
                fs::remove_file(destination).unwrap();
                // The steps after the open ask as well.
                assert!(asked > opening.asked(), "{name}: {asked}");
            }
            // Stopping at the first, a middle and the last question each
            // ends the operation, and leaves no file.
            for from in [0, asked / 2, asked - 1] {
                let stop = StopAt::new(from);
                let error = run(&|| stop.ask()).unwrap_err();
                assert!(
                    matches!(error, LoadError::Cancelled),
                    "{name} at {from}: {error}"
                );
                assert!(
                    destination.is_none_or(|destination| !destination.exists()),
                    "{name} at {from}"
                );
            }
        }
    }

    #[test]
    fn build_index_keeps_its_files_in_the_scratch_folder() {
        let directory = tempfile::tempdir().unwrap();
        let room = directory.path().join("room.xyz");
        write_room(&room);
        let scratch = directory.path().join("scratch");
        fs::create_dir(&scratch).unwrap();
        let cloud = crate::open(&room, 1).unwrap();
        let options = IndexOptions {
            cached: false,
            scratch_dir: Some(scratch.clone()),
        };
        let mut seen = Vec::new();
        let index = build_index(&cloud, &options, &mut |_| {
            seen.push(fs::read_dir(&scratch).unwrap().count());
            Ok(())
        })
        .unwrap();
        // While it is built and while it is used, the index lies in one
        // folder of the scratch folder.
        assert!(seen.iter().all(|entries| *entries == 1), "{seen:?}");
        let entries: Vec<_> = fs::read_dir(&scratch)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(entries[0]
            .file_name()
            .to_string_lossy()
            .starts_with("open-pointcloud-index-"));
        assert_eq!(index.root.total_points, cloud.total_points);
        assert!(!index.read_node("r", 10).unwrap().is_empty());
        // Beside the scratch folder, the folder holds only the source.
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
        drop(index);
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
    }

    #[test]
    fn the_hint_of_an_empty_slab_names_the_face_of_the_view() {
        for view in DrawingView::ALL {
            let hinted = hint_empty_slab(LoadError::InvalidData(EMPTY_SLAB.into()), view);
            let LoadError::InvalidData(reason) = hinted else {
                panic!("{view:?}");
            };
            assert_eq!(reason, format!("{EMPTY_SLAB}: {}", empty_slab_hint(view)));
        }
        let other = hint_empty_slab(LoadError::InvalidData("other".into()), DrawingView::Plan);
        assert!(matches!(other, LoadError::InvalidData(reason) if reason == "other"));
    }
}
