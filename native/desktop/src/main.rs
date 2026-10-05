use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod bag_map;
mod bag_panel;
mod bcf;
mod camera_views;
mod cli_help;
mod closed_mesh;
mod cloud_centroid;
mod cloud_transform;
mod drawing;
mod extensions;
mod faces;
mod file_view;
mod gpu_viewport;
mod i18n;
mod lod_pace;
#[cfg(target_os = "macos")]
mod macos_open;
mod mcp;
mod measure;
mod mesh_export;
mod native_api;
mod native_chrome;
mod open_progress;
mod opencad_properties;
mod opencad_ribbon;
mod orbit_point;
mod preferences;
mod project_open;
mod screenshot;
mod selection;
mod settings_dialog;
#[cfg(test)]
mod shell_tests;
mod station_photos;
mod ui_theme;
mod view_cube;
mod views;

use bag_map::{MapView, TileKey};
use cloud_transform::CloudTransform;
use file_view::{FileAction, FilePage};
use iced::futures::SinkExt;
use iced::mouse;
use iced::widget::canvas::{self, event, Canvas, Frame, Geometry};
use iced::widget::{
    button, checkbox, column, container, row, scrollable, slider, stack, svg, text, text_input,
    tooltip,
};
use iced::{Color, Element, Fill, Font, Point as UiPoint, Rectangle, Renderer, Size, Task, Theme};
use lod_pace::{
    first_pass_budget, plan_first_pass, preview_improves, preview_tier_points, LodPace, ScreenFill,
};
use pointcloud_core::{
    BagBounds, BagLod, Bounds, ExportFormat, IndexConfig, IndexProgress, IndexStage, IndexedPoint,
    MeshGeometry, OctreeIndex, OrientedBox, Point, PointCloud, SurfaceMeshConfig,
};
use preferences::{MAX_POINT_BUDGET, MIN_POINT_BUDGET};
use rayon::prelude::*;
#[cfg(test)]
use selection::select_world;
use selection::{
    pick_displayed, pick_full_transformed, pick_indexed_transformed, pick_surface,
    select_full_cancellable, select_world_cancellable, ClassFilter, ClassVisibility, DeletionMask,
    PickTarget, PickView, Projection, ScreenRect, SelectionMask, SelectionSource,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use station_photos::{PhotoAtlas, PhotoSet, WalkView};
use ui_theme::UiTheme;

/// The name of the application wherever it is shown.
pub(crate) const APP_NAME: &str = "Open Pointcloud Studio";
/// Where the source code of the application is published.
pub(crate) const SOURCE_URL: &str = "https://github.com/OpenAEC-Foundation/open-pointcloud-studio";
/// The version as the status bar shows it.
const VERSION_LABEL: &str = concat!("v", env!("CARGO_PKG_VERSION"));
/// The name window managers and launchers know the application by, which is
/// the name of the desktop entry the packages install.
#[cfg(target_os = "linux")]
const APPLICATION_ID: &str = "org.openaec.OpenPointcloudStudio";

/// The name and the version of the application as one text.
pub(crate) fn app_title() -> String {
    format!("{APP_NAME} {VERSION_LABEL}")
}

/// The title of the window: the application, after the file name of the
/// active scan when there is one.
fn title_for(active: Option<&str>) -> String {
    match active {
        Some(name) => format!("{name} - {}", app_title()),
        None => app_title(),
    }
}

const LOAD_SAMPLE_LIMIT: usize = 100_000;
const EXACT_VISIBLE_LOD_ZOOM: f32 = 0.05;
const MAX_EXACT_VISIBLE_LOD_CANDIDATES: u64 = 2_000_000;
const AUTO_INDEX_MIN_POINTS: u64 = 1_000_000;
const ONE_PASS_IMPORT_MIN_BYTES: u64 = 64 * 1024 * 1024;
/// The standard classes of LAS points; the names are translated where the
/// project panel lists them.
const ASPRS_CLASSIFICATIONS: &[(u8, &str)] = &[
    (0, i18n::key("Never classified")),
    (1, i18n::key("Unassigned")),
    (2, i18n::key("Ground")),
    (3, i18n::key("Low vegetation")),
    (4, i18n::key("Medium vegetation")),
    (5, i18n::key("High vegetation")),
    (6, i18n::key("Building")),
    (7, i18n::key("Low point / noise")),
    (9, i18n::key("Water")),
    (10, i18n::key("Rail")),
    (11, i18n::key("Road surface")),
    (13, i18n::key("Wire guard")),
    (14, i18n::key("Wire conductor")),
    (15, i18n::key("Transmission tower")),
    (17, i18n::key("Bridge deck")),
];
const BAG3D_MESH_COMMENTS: &[&str] = &[
    "© 3DBAG door tudelft3d en 3DGI · CC BY 4.0",
    "https://docs.3dbag.nl/nl/copyright/",
    "EPSG:7415 RD New + NAP",
];
/// The same credit for a PLY file. The header of that format is ASCII text
/// by definition, and a reader that holds to it stops at any other character.
const BAG3D_PLY_COMMENTS: &[&str] = &[
    "(c) 3DBAG by tudelft3d and 3DGI, CC BY 4.0",
    "https://docs.3dbag.nl/nl/copyright/",
    "EPSG:7415 RD New + NAP",
];

fn export_format_for_path(path: &Path) -> Option<ExportFormat> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ply") => Some(ExportFormat::PlyBinary),
        Some("xyz") => Some(ExportFormat::Xyz),
        Some("pts") => Some(ExportFormat::Pts),
        Some("csv") => Some(ExportFormat::Csv),
        Some("las") => Some(ExportFormat::Las),
        Some("laz") => Some(ExportFormat::Laz),
        Some("e57") => Some(ExportFormat::E57),
        _ => None,
    }
}

fn open_for_export(source: &Path) -> Result<PointCloud, pointcloud_core::LoadError> {
    let is_las = source
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
        });
    if is_las {
        pointcloud_core::open_las_header(source)
    } else {
        pointcloud_core::open(source, 1)
    }
}

/// Save every station photo of a scan file in its stored encoding and report
/// where each one looks, without decoding any point.
fn export_station_photos(
    source: &Path,
    directory: &Path,
) -> Result<Vec<String>, pointcloud_core::LoadError> {
    let (poses, images) = pointcloud_core::scan_stations(source)?;
    let encoded = pointcloud_core::read_scan_images(source, &images)?;
    std::fs::create_dir_all(directory)?;
    let mut lines = Vec::with_capacity(images.len());
    for (index, (image, bytes)) in images.iter().zip(encoded).enumerate() {
        let station = image
            .station
            .and_then(|station| poses.get(station))
            .map_or("photo", |pose| pose.label.as_str());
        let name: String = station
            .chars()
            .map(|character| {
                if character.is_alphanumeric() || matches!(character, '-' | '_' | ' ') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        let extension = match image.format {
            pointcloud_core::ScanImageFormat::Jpeg => "jpg",
            pointcloud_core::ScanImageFormat::Png => "png",
        };
        let file = directory.join(format!("{name} photo {}.{extension}", index + 1));
        std::fs::write(&file, bytes)?;
        let view = image.view_direction();
        lines.push(format!(
            "{}: {}x{}, looks {:+.3}, {:+.3}, {:+.3}",
            file.display(),
            image.width,
            image.height,
            view[0],
            view[1],
            view[2]
        ));
    }
    Ok(lines)
}

fn export_edited_where(
    cloud: &PointCloud,
    destination: &Path,
    format: ExportFormat,
    transform: CloudTransform,
    expected_count: u64,
    mut include: impl FnMut(u64, &Point) -> bool,
) -> Result<(), pointcloud_core::LoadError> {
    if transform.is_identity() {
        pointcloud_core::export_where(cloud, destination, format, expected_count, include)
    } else {
        if format == ExportFormat::E57
            && transform.scale[0] > 0.0
            && transform
                .scale
                .iter()
                .all(|value| *value == transform.scale[0])
            && pointcloud_core::export_e57_uniform_affine_where(
                cloud,
                destination,
                Some(expected_count),
                transform.scale[0],
                transform.offset,
                &mut include,
            )?
            .is_some()
        {
            return Ok(());
        }
        pointcloud_core::export_map(
            cloud,
            destination,
            format,
            expected_count,
            |ordinal, point| include(ordinal, &point).then(|| transform.point(point)),
        )
    }
}

fn transformed_mesh_normals(normals: &[[f32; 3]], scale: [f64; 3]) -> Option<Vec<[f32; 3]>> {
    let transform = CloudTransform {
        scale,
        offset: [0.0; 3],
    };
    normals
        .iter()
        .map(|normal| transform.normal(*normal))
        .collect()
}

fn export_edited_section(
    cloud: &PointCloud,
    destination: &Path,
    format: ExportFormat,
    transform: CloudTransform,
    section: OrientedBox,
    deleted: Option<&DeletionMask>,
) -> Result<u64, pointcloud_core::LoadError> {
    if transform.is_identity() {
        pointcloud_core::export_section_where(cloud, destination, format, section, |ordinal, _| {
            deleted.is_none_or(|mask| !mask.contains(ordinal))
        })
    } else {
        if format == ExportFormat::E57
            && transform.scale[0] > 0.0
            && transform
                .scale
                .iter()
                .all(|value| *value == transform.scale[0])
        {
            let translated = pointcloud_core::export_e57_uniform_affine_where(
                cloud,
                destination,
                None,
                transform.scale[0],
                transform.offset,
                |ordinal, point| {
                    let xyz = transform.xyz(point.xyz);
                    deleted.is_none_or(|mask| !mask.contains(ordinal)) && section.contains(xyz)
                },
            )?;
            if let Some(count) = translated {
                return Ok(count);
            }
        }
        pointcloud_core::export_map_auto_count(cloud, destination, format, |ordinal, point| {
            if deleted.is_some_and(|mask| mask.contains(ordinal)) {
                return None;
            }
            let point = transform.point(point);
            section.contains(point.xyz).then_some(point)
        })
    }
}

/// The reference of a section box turned by `rotation` degrees: the box
/// around the clouds in the frame of that turn about the vertical through
/// their centre. Without a turn, the box around the clouds.
fn section_frame_reference(clouds: &[CloudEntry], rotation: f64) -> Option<Bounds> {
    let overall = combined_bounds(clouds)?;
    if rotation == 0.0 {
        return Some(overall);
    }
    let center = overall.center();
    OrientedBox::frame_bounds(
        pointcloud_core::bounds_corners(overall),
        rotation,
        [center[0], center[1]],
    )
}

/// The turn of the frame a reference is given in: about the vertical through
/// the centre of the reference.
fn section_pivot_frame(reference: Bounds, rotation: f64) -> OrientedBox {
    let center = reference.center();
    OrientedBox::new(
        Bounds {
            min: center,
            max: center,
        },
        rotation,
    )
}

/// A turn in degrees as the Properties field shows it: no more digits than
/// it has, up to four.
fn format_rotation(degrees: f64) -> String {
    let text = format!("{degrees:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    match text {
        "-0" | "" => "0".into(),
        text => text.into(),
    }
}

/// Parse a turn typed in degrees.
fn parse_rotation(text: &str) -> Option<f64> {
    let text = text.trim().trim_end_matches('°').trim().replace(',', ".");
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && value.abs() <= 3_600.0)
}

fn section_within_model(requested: Bounds, model: Bounds) -> Option<Bounds> {
    let mut section = requested;
    for axis in 0..3 {
        let span = model.max[axis] - model.min[axis];
        let tolerance = (span * 0.01).clamp(0.000_001, 0.01);
        if !requested.min[axis].is_finite()
            || !requested.max[axis].is_finite()
            || requested.min[axis] > requested.max[axis]
            || requested.min[axis] < model.min[axis] - tolerance
            || requested.max[axis] > model.max[axis] + tolerance
        {
            return None;
        }
        section.min[axis] = requested.min[axis].max(model.min[axis]);
        section.max[axis] = requested.max[axis].min(model.max[axis]);
        if (span > 0.0 && section.min[axis] >= section.max[axis])
            || (span == 0.0 && section.min[axis] != section.max[axis])
        {
            return None;
        }
    }
    Some(section)
}

fn display_name(path: &std::path::Path) -> &str {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Point cloud");
    name.strip_prefix("open-pointcloud-").unwrap_or(name)
}

fn format_count(value: impl ToString) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(digit);
    }
    grouped
}

/// A count short enough for a list row: 10.0M, 544k, 812.
fn compact_count(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{:.0}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn format_zoom_level(zoom: f32) -> String {
    let magnification = 1.0 / f64::from(zoom);
    if magnification >= 100.0 {
        format!("{}×", format_count(magnification.round() as u64))
    } else if magnification >= 10.0 {
        format!("{magnification:.1}×")
    } else if magnification >= 0.01 {
        format!("{magnification:.2}×")
    } else {
        format!("{magnification:.4}×")
    }
}

/// Whether a mesh file names 3D BAG as its source: in the comment lines that
/// open an OBJ file, where the download puts the credit first and an export
/// puts it after a line of its own, or in the comment lines of a PLY header,
/// where an export puts it. A mesh that is saved and opened again keeps its
/// credit this way.
fn is_bag3d_mesh(path: &Path) -> bool {
    /// How the credit begins in a downloaded or exported OBJ file, in an
    /// OBJ file with the English wording, and in an exported PLY file.
    const CREDITS: [&str; 3] = [
        "© 3DBAG door tudelft3d en 3DGI",
        "3DBAG by tudelft3d and 3DGI",
        "(c) 3DBAG by tudelft3d and 3DGI",
    ];
    /// The credit stands in the first lines. The limit keeps a file without
    /// line ends from being read whole.
    const HEAD_BYTES: u64 = 4096;
    let ply = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("obj") => false,
        Some("ply") => true,
        _ => return false,
    };
    let mut head = Vec::new();
    if File::open(path)
        .and_then(|file| file.take(HEAD_BYTES).read_to_end(&mut head))
        .is_err()
    {
        return false;
    }
    // Bytes, not text: the header of a binary PLY is followed by its data.
    String::from_utf8_lossy(&head)
        .lines()
        .map(str::trim)
        .take_while(|line| {
            if ply {
                *line != "end_header"
            } else {
                line.is_empty() || line.starts_with('#')
            }
        })
        .filter_map(|line| line.strip_prefix(if ply { "comment" } else { "#" }))
        .any(|comment| {
            let comment = comment.trim_start();
            CREDITS.iter().any(|credit| comment.starts_with(credit))
        })
}

/// Started from the file manager, Windows opens a console window for this
/// program. Close it when the application window is about to open; a console
/// shared with a terminal or script stays attached so command-line output
/// keeps working.
#[cfg(windows)]
fn release_own_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetConsoleProcessList(processes: *mut u32, count: u32) -> u32;
        fn FreeConsole() -> i32;
    }
    let mut processes = [0u32; 2];
    // SAFETY: the buffer holds the two entries asked for, and both calls only
    // affect the console attachment of this process.
    unsafe {
        if GetConsoleProcessList(processes.as_mut_ptr(), 2) == 1 {
            FreeConsole();
        }
    }
}

#[cfg(not(windows))]
fn release_own_console() {}

fn main() -> iced::Result {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    i18n::set(i18n::load());
    // The version and the help text are answered before anything else, so
    // that a script can ask for them where no window can open.
    if let Some(text) = cli_help::answer(first.as_deref()) {
        println!("{text}");
        return Ok(());
    }
    if first.as_deref() == Some(OsStr::new("--mcp")) {
        if args.next().is_some() {
            eprintln!("Usage: open-pointcloud-studio --mcp");
            std::process::exit(2);
        }
        // Standard input and output carry the protocol, so the console stays
        // attached and no window opens in this process.
        std::process::exit(mcp::run());
    }
    if first.as_deref() == Some(OsStr::new("--index")) {
        let (Some(source), None) = (args.next(), args.next()) else {
            eprintln!("Usage: open-pointcloud-studio --index INPUT");
            std::process::exit(2);
        };
        let source = PathBuf::from(source);
        let started = Instant::now();
        let is_las = source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
            });
        let index = if is_las {
            pointcloud_core::open_las_header(&source)
                .and_then(|cloud| OctreeIndex::build_cached(&cloud, IndexConfig::default()))
        } else {
            OctreeIndex::open_and_build_cached_with_progress(
                &source,
                100_000,
                IndexConfig::default(),
                |_| Ok(()),
            )
            .map(|(_, index)| index)
        };
        match index {
            Ok(index) => {
                let mut nodes = 0u64;
                let mut leaves = 0u64;
                let mut deepest = 0u8;
                let mut pending = vec![&index.root];
                while let Some(node) = pending.pop() {
                    nodes += 1;
                    deepest = deepest.max(node.depth);
                    if node.is_leaf() {
                        leaves += 1;
                    }
                    pending.extend(&node.children);
                }
                println!(
                    "Index ready: {} points, {} nodes ({} leaves, depth {}) in {:.1}s",
                    index.root.total_points,
                    nodes,
                    leaves,
                    deepest,
                    started.elapsed().as_secs_f64()
                );
                return Ok(());
            }
            Err(error) => {
                eprintln!("Index failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--scans")) {
        let (Some(source), None) = (args.next(), args.next()) else {
            eprintln!("Usage: open-pointcloud-studio --scans INPUT");
            std::process::exit(2);
        };
        match open_for_export(&PathBuf::from(source)) {
            Ok(cloud) => {
                println!("{} scan position(s)", cloud.scan_poses.len());
                for pose in cloud.scan_poses {
                    println!(
                        "{}: {:.6}, {:.6}, {:.6}",
                        pose.label, pose.position[0], pose.position[1], pose.position[2]
                    );
                    if let Some(axes) = pose.axes {
                        for (label, axis) in ["X", "Y", "Z"].into_iter().zip(axes) {
                            println!(
                                "  {label}: {:+.6}, {:+.6}, {:+.6}",
                                axis[0], axis[1], axis[2]
                            );
                        }
                    }
                }
                return Ok(());
            }
            Err(error) => {
                eprintln!("Scan positions failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--list-scans")) {
        let paths: Vec<PathBuf> = args.map(PathBuf::from).collect();
        if paths.is_empty() {
            eprintln!("Usage: open-pointcloud-studio --list-scans PATH [PATH ...]");
            std::process::exit(2);
        }
        let expansion = project_open::expand(&paths, &[]);
        for file in &expansion.files {
            println!("{}", file.display());
        }
        eprintln!("{} scan file(s)", expansion.files.len());
        if expansion.missing > 0 {
            eprintln!("{} listed scan(s) not found", expansion.missing);
            for name in &expansion.missing_names {
                eprintln!("  {name}");
            }
        }
        for error in &expansion.errors {
            eprintln!("{error}");
        }
        if !expansion.errors.is_empty() {
            std::process::exit(1);
        }
        return Ok(());
    }
    if first.as_deref() == Some(OsStr::new("--photos")) {
        let (Some(source), Some(directory), None) = (args.next(), args.next(), args.next()) else {
            eprintln!("Usage: open-pointcloud-studio --photos INPUT OUTPUT_DIRECTORY");
            std::process::exit(2);
        };
        match export_station_photos(&PathBuf::from(source), &PathBuf::from(directory)) {
            Ok(lines) => {
                println!("{} station photo(s)", lines.len());
                for line in lines {
                    println!("{line}");
                }
                return Ok(());
            }
            Err(error) => {
                eprintln!("Station photos failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--merge")) {
        let Some(destination) = args.next() else {
            eprintln!(
                "Usage: open-pointcloud-studio --merge OUTPUT.laz INPUT1.las INPUT2.laz [...]"
            );
            std::process::exit(2);
        };
        let destination = PathBuf::from(destination);
        let sources: Vec<PathBuf> = args.map(PathBuf::from).collect();
        if sources.len() < 2 {
            eprintln!("Merge needs at least two LAS/LAZ inputs");
            std::process::exit(2);
        }
        let Some(format @ (ExportFormat::Las | ExportFormat::Laz)) =
            export_format_for_path(&destination)
        else {
            eprintln!("Merge destination must end in .las or .laz");
            std::process::exit(2);
        };
        let result = sources
            .iter()
            .map(pointcloud_core::open_las_header)
            .collect::<Result<Vec<_>, _>>()
            .and_then(|clouds| {
                let references: Vec<_> = clouds.iter().collect();
                let mut last_report = 0;
                pointcloud_core::merge_las_map_count(
                    &references,
                    &destination,
                    format,
                    None,
                    &mut |_, _, point| Some(point),
                    &mut |processed, total, written| {
                        if processed.saturating_sub(last_report) >= 5_000_000 || processed == total
                        {
                            eprintln!(
                                "Merged {processed} / {total} source points; wrote {written}"
                            );
                            last_report = processed;
                        }
                        Ok(())
                    },
                )
            });
        match result {
            Ok(count) => {
                println!("Merged {count} points into {}", destination.display());
                return Ok(());
            }
            Err(error) => {
                eprintln!("Merge failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--export")) {
        let (Some(source), Some(destination), None) = (args.next(), args.next(), args.next())
        else {
            eprintln!("Usage: open-pointcloud-studio --export INPUT OUTPUT");
            std::process::exit(2);
        };
        let source = PathBuf::from(source);
        let destination = PathBuf::from(destination);
        let Some(format) = export_format_for_path(&destination) else {
            eprintln!("Supported export extensions: .ply, .xyz, .pts, .csv, .las, .laz, .e57");
            std::process::exit(2);
        };
        match open_for_export(&source)
            .and_then(|cloud| pointcloud_core::export_full(&cloud, &destination, format))
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                eprintln!("Export failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--section")) {
        let usage = || {
            eprintln!(
                "Usage: open-pointcloud-studio --section INPUT XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX OUTPUT [--rotation DEGREES]"
            );
            std::process::exit(2);
        };
        let (Some(source), Some(limits), Some(destination)) =
            (args.next(), args.next(), args.next())
        else {
            usage()
        };
        let rotation = match (args.next(), args.next(), args.next()) {
            (None, _, _) => 0.0,
            (Some(flag), Some(value), None) if flag == "--rotation" => {
                match value.to_str().and_then(parse_rotation) {
                    Some(rotation) => rotation,
                    None => {
                        eprintln!("--rotation must be a number of degrees");
                        std::process::exit(2);
                    }
                }
            }
            _ => usage(),
        };
        let source = PathBuf::from(source);
        let destination = PathBuf::from(destination);
        let Some(format) = export_format_for_path(&destination) else {
            eprintln!("Supported export extensions: .ply, .xyz, .pts, .csv, .las, .laz, .e57");
            std::process::exit(2);
        };
        let Some(values) = limits.to_str().and_then(|value| {
            value
                .split(',')
                .map(str::parse::<f64>)
                .collect::<Result<Vec<_>, _>>()
                .ok()
        }) else {
            eprintln!("Section limits must be six comma-separated numbers");
            std::process::exit(2);
        };
        if values.len() != 6 {
            eprintln!("Section limits must be six comma-separated numbers");
            std::process::exit(2);
        }
        let section = OrientedBox::new(
            Bounds {
                min: [values[0], values[1], values[2]],
                max: [values[3], values[4], values[5]],
            },
            rotation,
        );
        match open_for_export(&source).and_then(|cloud| {
            pointcloud_core::export_section(&cloud, &destination, format, section)
        }) {
            Ok(count) => {
                println!("Exported {count} points to {}", destination.display());
                return Ok(());
            }
            Err(error) => {
                eprintln!("Section export failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--drawing")) {
        let arguments: Vec<_> = args.collect();
        match drawing::command_line(&arguments) {
            Ok(line) => {
                println!("{line}");
                return Ok(());
            }
            Err((code, line)) => {
                if line.is_empty() {
                    eprintln!(
                        "Usage: open-pointcloud-studio --drawing INPUT XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX OUTPUT.dxf|.dwg [--view plan|front|back|left|right] [--rotation DEGREES] [--thickness METRES] [--units mm|m] [--fill on|off]"
                    );
                } else {
                    eprintln!("{line}");
                }
                std::process::exit(code);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--closed-mesh")) {
        let arguments: Vec<_> = args.collect();
        match closed_mesh::command_line(&arguments) {
            Ok(lines) => {
                println!("{lines}");
                return Ok(());
            }
            Err((code, line)) => {
                if line.is_empty() {
                    eprintln!(
                        "Usage: open-pointcloud-studio --closed-mesh INPUT OUTPUT.obj|.ply|.stl [--box XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX] [--rotation DEGREES] [--voxel METRES] [--max-hole METRES] [--simplify MILLIMETRES] [--sides automatic|centre|upward]"
                    );
                } else {
                    eprintln!("{line}");
                }
                std::process::exit(code);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--faces")) {
        let arguments: Vec<_> = args.collect();
        match faces::command_line(&arguments) {
            Ok(lines) => {
                println!("{lines}");
                return Ok(());
            }
            Err((code, line)) => {
                if line.is_empty() {
                    eprintln!(
                        "Usage: open-pointcloud-studio --faces INPUT OUTPUT.json|.obj [--box XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX] [--rotation DEGREES] [--distance METRES] [--angle DEGREES] [--min-area SQUARE_METRES] [--cylinders on|off]"
                    );
                } else {
                    eprintln!("{line}");
                }
                std::process::exit(code);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--mesh-export")) {
        let (Some(source), Some(destination), None) = (args.next(), args.next(), args.next())
        else {
            eprintln!("Usage: open-pointcloud-studio --mesh-export INPUT OUTPUT");
            std::process::exit(2);
        };
        match mesh_export::convert_file(Path::new(&source), Path::new(&destination)) {
            Ok(lines) => {
                println!("{lines}");
                return Ok(());
            }
            Err((code, line)) => {
                eprintln!("{line}");
                std::process::exit(code);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--mesh")) {
        let (Some(source), Some(destination), None) = (args.next(), args.next(), args.next())
        else {
            eprintln!("Usage: open-pointcloud-studio --mesh INPUT OUTPUT.obj");
            std::process::exit(2);
        };
        let source = PathBuf::from(source);
        let destination = PathBuf::from(destination);
        let is_las = source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
            });
        let cloud = if is_las {
            pointcloud_core::open_las_header(&source)
        } else {
            pointcloud_core::open(&source, 1)
        };
        match cloud.and_then(|cloud| {
            pointcloud_core::mesh_terrain_obj(
                &cloud,
                &destination,
                pointcloud_core::MeshConfig::default(),
            )
        }) {
            Ok(stats) => {
                println!(
                    "Mesh ready: {} source points, {} vertices, {} triangles -> {}",
                    stats.source_points,
                    stats.vertices,
                    stats.triangles,
                    destination.display()
                );
                return Ok(());
            }
            Err(error) => {
                eprintln!("Meshing failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--surface")) {
        let usage = "Usage: open-pointcloud-studio --surface INPUT OUTPUT.obj [--max-vertices N] [--neighbors N] [--edge-factor N]";
        let (Some(source), Some(destination)) = (args.next(), args.next()) else {
            eprintln!("{usage}");
            std::process::exit(2);
        };
        let mut config = pointcloud_core::SurfaceMeshConfig::default();
        while let Some(option) = args.next() {
            let Some(value) = args.next() else {
                eprintln!("{usage}");
                std::process::exit(2);
            };
            let value = value.to_string_lossy();
            match option.to_str() {
                Some("--max-vertices") => {
                    config.max_vertices = value.parse().unwrap_or_else(|_| {
                        eprintln!("invalid --max-vertices: {value}");
                        std::process::exit(2)
                    });
                }
                Some("--neighbors") => {
                    config.neighbors = value.parse().unwrap_or_else(|_| {
                        eprintln!("invalid --neighbors: {value}");
                        std::process::exit(2)
                    });
                }
                Some("--edge-factor") => {
                    config.max_edge_factor = value.parse().unwrap_or_else(|_| {
                        eprintln!("invalid --edge-factor: {value}");
                        std::process::exit(2)
                    });
                }
                _ => {
                    eprintln!("{usage}");
                    std::process::exit(2);
                }
            }
        }
        let source = PathBuf::from(source);
        let destination = PathBuf::from(destination);
        let is_las = source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
            });
        let cloud = if is_las {
            pointcloud_core::open_las_header(&source)
        } else {
            pointcloud_core::open(&source, 1)
        };
        match cloud
            .and_then(|cloud| pointcloud_core::mesh_surface_obj(&cloud, &destination, config))
        {
            Ok(stats) => {
                println!(
                    "3D surface ready: {} source points, {} vertices, {} triangles -> {}",
                    stats.source_points,
                    stats.vertices,
                    stats.triangles,
                    destination.display()
                );
                return Ok(());
            }
            Err(error) => {
                eprintln!("3D surface failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--bag3d")) {
        let (Some(bbox), Some(lod), Some(destination), None) =
            (args.next(), args.next(), args.next(), args.next())
        else {
            eprintln!(
                "Usage: open-pointcloud-studio --bag3d XMIN,YMIN,XMAX,YMAX 1.2|1.3|2.2 OUTPUT.obj"
            );
            std::process::exit(2);
        };
        let bounds = BagBounds::parse(&bbox.to_string_lossy());
        let lod = BagLod::parse(&lod.to_string_lossy());
        match bounds.and_then(|bounds| {
            lod.and_then(|lod| pointcloud_core::fetch_bag3d_obj(bounds, lod, &destination))
        }) {
            Ok(stats) => {
                println!(
                    "3DBAG ready: {} buildings, {} vertices, {} triangles, {} pages -> {}",
                    stats.buildings,
                    stats.vertices,
                    stats.triangles,
                    stats.pages,
                    PathBuf::from(destination).display()
                );
                return Ok(());
            }
            Err(error) => {
                eprintln!("3DBAG failed: {error}");
                std::process::exit(1);
            }
        }
    }
    let (requested_port, startup_files): (Option<u16>, Vec<PathBuf>) =
        if first.as_deref() == Some(OsStr::new("--api-port")) {
            let Some(value) = args
                .next()
                .and_then(|value| value.to_str().and_then(|s| s.parse().ok()))
            else {
                eprintln!("Usage: open-pointcloud-studio --api-port PORT [INPUT ...]");
                std::process::exit(2);
            };
            (Some(value), args.map(PathBuf::from).collect())
        } else {
            (
                None,
                first.into_iter().chain(args).map(PathBuf::from).collect(),
            )
        };
    let api = match native_api::start(requested_port) {
        Ok(api) => Some(api),
        Err(error) => {
            eprintln!("Native API unavailable: {error}");
            None
        }
    };
    release_own_console();
    iced::application(Studio::window_title, Studio::update, Studio::view)
        // Controls without an explicit size match the compact property rows.
        .settings(iced::Settings {
            default_text_size: iced::Pixels(12.0),
            ..iced::Settings::default()
        })
        .subscription(|studio| {
            let keyboard = iced::event::listen_with(|event, status, _| match event {
                iced::Event::Window(iced::window::Event::Resized(_)) => Some(Message::RibbonReset),
                // The window is closed by the application itself, so work
                // that is under way is asked to stop first.
                iced::Event::Window(iced::window::Event::CloseRequested) => Some(Message::Exit),
                iced::Event::Window(iced::window::Event::FileDropped(path)) => {
                    Some(Message::FileDropped(path))
                }
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::Escape),
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete),
                    ..
                }) if status == iced::event::Status::Ignored => {
                    Some(Message::ModelKey(ModelKey::Delete))
                }
                // Backspace and Enter edit and finish a measurement.
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace),
                    ..
                }) if status == iced::event::Status::Ignored => {
                    Some(Message::Measure(measure::MeasureAction::RemoveLast))
                }
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
                    ..
                }) if status == iced::event::Status::Ignored => {
                    Some(Message::Measure(measure::MeasureAction::Finish))
                }
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Character(value),
                    modifiers,
                    ..
                }) if status == iced::event::Status::Ignored
                    && !modifiers.control()
                    && !modifiers.alt()
                    && !modifiers.logo()
                    && value.eq_ignore_ascii_case("f") =>
                {
                    Some(Message::ModelKey(ModelKey::Fit))
                }
                // With the command key of the system: Control, and Command on
                // macOS.
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Character(value),
                    modifiers,
                    ..
                }) if status == iced::event::Status::Ignored && modifiers.command() => {
                    if value.eq_ignore_ascii_case("z") {
                        Some(Message::ModelKey(if modifiers.shift() {
                            ModelKey::Redo
                        } else {
                            ModelKey::Undo
                        }))
                    } else if value.eq_ignore_ascii_case("y") {
                        Some(Message::ModelKey(ModelKey::Redo))
                    } else if value.as_str() == "," {
                        Some(Message::Settings(settings_dialog::SettingsAction::Open))
                    } else {
                        None
                    }
                }
                // W A S D walk, Q and E move down and up.
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Character(value),
                    modifiers,
                    ..
                }) if status == iced::event::Status::Ignored
                    && !modifiers.control()
                    && !modifiers.alt()
                    && !modifiers.logo() =>
                {
                    WalkKey::from_character(value.as_str()).map(|key| Message::WalkKey(key, true))
                }
                iced::Event::Keyboard(iced::keyboard::Event::KeyReleased {
                    key: iced::keyboard::Key::Character(value),
                    ..
                }) => {
                    WalkKey::from_character(value.as_str()).map(|key| Message::WalkKey(key, false))
                }
                iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
                    Some(Message::Modifiers(modifiers))
                }
                // A key released while another window has focus never arrives.
                iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WalkStop),
                _ => None,
            });
            let walking = if studio.walk.is_some() && studio.walk_keys.contains(&true) {
                iced::time::every(Duration::from_millis(16)).map(Message::WalkTick)
            } else {
                iced::Subscription::none()
            };
            let api = if let Some(receiver) = &studio.api_receiver {
                let receiver = Arc::clone(receiver);
                let stream = iced::stream::channel(32, move |mut output| async move {
                    loop {
                        let request = receiver.lock().await.recv().await;
                        let Some(request) = request else { break };
                        if output.send(Message::ApiRequest(request)).await.is_err() {
                            break;
                        }
                    }
                });
                iced::Subscription::run_with_id("native_api", stream)
            } else {
                iced::Subscription::none()
            };
            // Files the system hands over are opened like files dropped on
            // the window.
            let opened = if let Some(receiver) = &studio.opened_files {
                let receiver = Arc::clone(receiver);
                let stream = iced::stream::channel(32, move |mut output| async move {
                    loop {
                        let path = receiver.lock().await.recv().await;
                        let Some(path) = path else { break };
                        if output.send(Message::FileDropped(path)).await.is_err() {
                            break;
                        }
                    }
                });
                iced::Subscription::run_with_id("opened_files", stream)
            } else {
                iced::Subscription::none()
            };
            iced::Subscription::batch([keyboard, api, opened, walking])
        })
        .font(include_bytes!("../../assets/fonts/Inter.ttf").as_slice())
        .font(include_bytes!("../../assets/fonts/SpaceGrotesk.ttf").as_slice())
        .default_font(Font::with_name("Inter"))
        .theme(|studio: &Studio| studio.ui_theme.iced())
        .antialiasing(true)
        .window(iced::window::Settings {
            size: Size::new(1440.0, 900.0),
            icon: iced::window::icon::from_file_data(
                include_bytes!("../../assets/icons/icon-64.png"),
                None,
            )
            .ok(),
            #[cfg(target_os = "linux")]
            platform_specific: iced::window::settings::PlatformSpecific {
                application_id: APPLICATION_ID.into(),
                ..Default::default()
            },
            // Closing arrives as `Message::Exit`, like Exit in the File view.
            exit_on_close_request: false,
            ..iced::window::Settings::default()
        })
        .run_with(move || {
            let mut studio = Studio::default();
            if let Some((receiver, handle)) = api {
                studio.api_receiver = Some(Arc::new(tokio::sync::Mutex::new(receiver)));
                studio.api_handle = Some(handle);
            }
            // This runs on the main thread after the event loop was made and
            // before it starts, which is when the hand-over must be in place.
            #[cfg(target_os = "macos")]
            {
                studio.opened_files = macos_open::install()
                    .map(|receiver| Arc::new(tokio::sync::Mutex::new(receiver)));
            }
            let chrome = Task::perform(
                async {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    3
                },
                Message::SyncWindowChrome,
            );
            let task = Task::batch([studio.open_paths(startup_files), chrome]);
            (studio, task)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ColorMode {
    Rgb,
    Elevation,
    Intensity,
    Classification,
}

impl fmt::Display for ColorMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Rgb => "RGB",
            Self::Elevation => "Elevation",
            Self::Intensity => "Intensity",
            Self::Classification => "Classification",
        })
    }
}

fn ribbon_scroll_id() -> scrollable::Id {
    scrollable::Id::new("ops-ribbon")
}

#[derive(Debug, Clone, Copy)]
enum MeshMode {
    Terrain,
    Surface,
}

impl MeshMode {
    fn label(self) -> &'static str {
        match self {
            Self::Terrain => "Terrain",
            Self::Surface => "3D surface",
        }
    }
}

struct MeshControl {
    cancelled: AtomicBool,
    progress: Mutex<pointcloud_core::MeshProgress>,
}

impl MeshControl {
    fn new(total: u64) -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(pointcloud_core::MeshProgress::new(
                pointcloud_core::MeshStage::Reading,
                0,
                total,
            )),
        }
    }

    fn report(
        &self,
        progress: pointcloud_core::MeshProgress,
    ) -> Result<(), pointcloud_core::LoadError> {
        *self
            .progress
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = progress;
        if self.cancelled.load(Ordering::Relaxed) {
            Err(pointcloud_core::LoadError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn snapshot(&self) -> pointcloud_core::MeshProgress {
        *self
            .progress
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

struct MeshJob {
    mode: MeshMode,
    path: PathBuf,
    control: Arc<MeshControl>,
    started: Instant,
    api_job_id: Option<String>,
}

struct MeshStart {
    mode: MeshMode,
    surface_config: SurfaceMeshConfig,
    cloud: Arc<PointCloud>,
    deleted: Option<Arc<DeletionMask>>,
    filter: ClassFilter,
    transform: CloudTransform,
    path: PathBuf,
    api_job_id: Option<String>,
}

fn mesh_accepts(
    ordinal: u64,
    point: &Point,
    deleted: Option<&DeletionMask>,
    filter: ClassFilter,
    transform: CloudTransform,
) -> bool {
    if deleted.is_some_and(|mask| mask.contains(ordinal)) {
        return false;
    }
    if filter.section.is_some() {
        filter.accepts(&transform.point(*point))
    } else {
        filter.accepts(point)
    }
}

#[derive(Clone)]
struct MergeSource {
    cloud: Arc<PointCloud>,
    deleted: Option<Arc<DeletionMask>>,
    transform: CloudTransform,
}

struct MergeControl {
    cancelled: AtomicBool,
    processed: AtomicU64,
    written: AtomicU64,
    total: u64,
}

impl MergeControl {
    fn report(&self, processed: u64, written: u64) -> Result<(), pointcloud_core::LoadError> {
        self.processed.store(processed, Ordering::Relaxed);
        self.written.store(written, Ordering::Relaxed);
        if self.cancelled.load(Ordering::Relaxed) {
            Err(pointcloud_core::LoadError::Cancelled)
        } else {
            Ok(())
        }
    }
}

struct MergeJob {
    path: PathBuf,
    control: Arc<MergeControl>,
    started: Instant,
    api_job_id: Option<String>,
}

impl MergeJob {
    fn progress_value(&self) -> Value {
        json!({
            "state": "running",
            "operation": "merge_visible",
            "path": self.path,
            "processed": self.control.processed.load(Ordering::Relaxed),
            "total": self.control.total,
            "written": self.control.written.load(Ordering::Relaxed),
            "cancel_requested": self.control.cancelled.load(Ordering::Relaxed),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }

    fn progress_text(&self) -> String {
        if self.control.cancelled.load(Ordering::Relaxed) {
            return "Cancelling cloud merge…".into();
        }
        let processed = self.control.processed.load(Ordering::Relaxed);
        let percent = processed.saturating_mul(100) / self.control.total.max(1);
        format!(
            "Merging scans: {percent}% of {} source points",
            format_count(self.control.total)
        )
    }
}

impl MeshJob {
    fn progress_value(&self) -> Value {
        let progress = self.control.snapshot();
        let stage = match progress.stage {
            pointcloud_core::MeshStage::Reading => "reading",
            pointcloud_core::MeshStage::Reconstructing => "reconstructing",
            pointcloud_core::MeshStage::Writing => "writing",
        };
        json!({
            "state": "running",
            "operation": "mesh",
            "mode": self.mode.label(),
            "path": self.path,
            "stage": stage,
            "completed": progress.completed,
            "total": progress.total,
            "cancel_requested": self.control.cancelled.load(Ordering::Relaxed),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }

    fn progress_text(&self) -> String {
        let progress = self.control.snapshot();
        let stage = match progress.stage {
            pointcloud_core::MeshStage::Reading => "Reading points",
            pointcloud_core::MeshStage::Reconstructing => "Reconstructing",
            pointcloud_core::MeshStage::Writing => "Writing OBJ",
        };
        if self.control.cancelled.load(Ordering::Relaxed) {
            return format!("{}: cancelling…", self.mode.label());
        }
        if progress.total == 0 {
            format!("{}: {stage}…", self.mode.label())
        } else {
            let percent = progress
                .completed
                .saturating_mul(100)
                .checked_div(progress.total)
                .unwrap_or(0);
            format!("{}: {stage} {percent}%", self.mode.label())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CameraPreset {
    Top,
    Bottom,
    Front,
    Back,
    Right,
    Left,
    Isometric,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextAction {
    Orbit,
    BoxSelect,
    PickPoint,
    SectionBox,
    FitView,
    ClearSelection,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApiExportMode {
    Full,
    Section,
    Selected,
    WithoutSelection,
}

impl CameraPreset {
    fn orientation(self) -> (f32, f32, &'static str) {
        match self {
            Self::Top => (
                -std::f32::consts::FRAC_PI_2,
                std::f32::consts::FRAC_PI_2,
                i18n::key("TOP"),
            ),
            Self::Bottom => (
                -std::f32::consts::FRAC_PI_2,
                -std::f32::consts::FRAC_PI_2,
                i18n::key("BOTTOM"),
            ),
            Self::Front => (-std::f32::consts::FRAC_PI_2, 0.0, i18n::key("FRONT")),
            Self::Back => (std::f32::consts::FRAC_PI_2, 0.0, i18n::key("BACK")),
            Self::Right => (0.0, 0.0, i18n::key("RIGHT")),
            Self::Left => (std::f32::consts::PI, 0.0, i18n::key("LEFT")),
            Self::Isometric => (-0.8, 0.6, i18n::key("ISOMETRIC")),
        }
    }
}

/// A key that edits the model or moves its camera. The same actions come
/// from buttons and from the command API; only the keys are held back while
/// the model is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelKey {
    Delete,
    Undo,
    Redo,
    Fit,
}

impl ModelKey {
    fn message(self) -> Message {
        match self {
            Self::Delete => Message::DeleteSelection,
            Self::Undo => Message::UndoDelete,
            Self::Redo => Message::RedoDelete,
            Self::Fit => Message::ResetCamera,
        }
    }
}

/// Keys that move the walking camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalkKey {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
}

impl WalkKey {
    fn from_character(character: &str) -> Option<Self> {
        match character.to_ascii_lowercase().as_str() {
            "w" => Some(Self::Forward),
            "s" => Some(Self::Back),
            "a" => Some(Self::Left),
            "d" => Some(Self::Right),
            "e" => Some(Self::Up),
            "q" => Some(Self::Down),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    SyncWindowChrome(u8),
    ApiRequest(native_api::ApiRequest),
    ApiScreenshot(screenshot::Step),
    ApiExported(String, bool, Result<(PathBuf, u64), String>),
    MergeVisible,
    MergePathChosen(Option<PathBuf>),
    MergePoll,
    CancelMerge,
    MergeReady(Result<(PathBuf, u64), String>),
    ApiWorldSelectionReady(
        String,
        u64,
        Result<Vec<(usize, Arc<SelectionMask>)>, String>,
    ),
    ApiPickReady(String, u64, usize, Result<Option<IndexedPoint>, String>),
    ToggleFile,
    FileAction(FileAction),
    /// Show a page of the File view.
    FilePage(FilePage),
    /// Open a web page of the application in the browser of the system.
    OpenUrl(&'static str),
    Exit,
    RibbonScroll(f32),
    RibbonViewport(f32, f32, f32),
    RibbonReset,
    Theme(UiTheme),
    PersistSettings(u64),
    Open,
    OpenFolder,
    FilesChosen(Option<Vec<PathBuf>>),
    FileDropped(PathBuf),
    DroppedFilesReady,
    ScansExpanded(project_open::Expansion),
    ApiScansExpanded(
        std::sync::mpsc::Sender<Value>,
        PathBuf,
        project_open::Expansion,
    ),
    OpenProgress(u64),
    ImportLoaded(u64, Result<Arc<PointCloud>, String>),
    HeaderLoaded(u64, Result<Arc<PointCloud>, String>),
    IndexedImportPreview(u64, Arc<PointCloud>),
    /// The points of a large source known so far, while its import is still
    /// reading it.
    ImportSnapshot(u64, Arc<PointCloud>),
    IndexedImportReady(u64, Result<(Arc<PointCloud>, Arc<OctreeIndex>), String>),
    CancelImport(u64),
    Settings(settings_dialog::SettingsAction),
    /// Cancel every import that reads its source without building an octree.
    CancelOpening,
    Loaded(Result<Arc<PointCloud>, String>),
    MeshLoaded(
        Arc<PointCloud>,
        Result<Option<mesh_export::MeasuredMesh>, String>,
    ),
    Refined(Arc<PointCloud>, Result<Arc<PointCloud>, String>),
    Export,
    ExportSection,
    SectionExportPathChosen(
        Arc<PointCloud>,
        OrientedBox,
        ExportFormat,
        CloudTransform,
        Option<Arc<DeletionMask>>,
        Option<PathBuf>,
    ),
    SectionExported(Result<(PathBuf, u64), String>),
    ExportSelection,
    RemoveSelection,
    DeleteSelection,
    UndoDelete,
    RedoDelete,
    DecimationStride(u64),
    Decimate,
    ThinPercent(u8),
    Thin,
    ThinReady {
        source: Arc<PointCloud>,
        baseline: Option<Arc<DeletionMask>>,
        percent: u8,
        result: Result<Arc<SelectionMask>, String>,
    },
    SurfaceSetting(usize, String),
    MeshRequest(MeshMode),
    MeshPathChosen(
        MeshMode,
        SurfaceMeshConfig,
        Arc<PointCloud>,
        Option<Arc<DeletionMask>>,
        Option<PathBuf>,
    ),
    MeshReady(
        MeshMode,
        Result<
            (
                Arc<PointCloud>,
                PathBuf,
                pointcloud_core::MeshStats,
                mesh_export::MeasuredMesh,
            ),
            String,
        >,
    ),
    MeshPoll,
    CancelMesh,
    ExportMesh,
    MeshExportPathChosen(mesh_export::MeshExportRequest, Option<PathBuf>),
    /// A mesh file was written or could not be, with the job of the local
    /// API that asked for it.
    MeshExported(Option<String>, Result<mesh_export::MeshExportDone, String>),
    ToggleBagPanel,
    /// Open the 3D BAG panel when it is closed.
    ShowBagPanel,
    BagField(usize, String),
    BagLod(BagLod),
    BagFromSection,
    BagMapDraw(bool),
    BagMapSelected(BagBounds),
    BagMapPan([f32; 2]),
    BagMapZoom(f32, UiPoint),
    BagMapFitFields,
    BagMapHome,
    BagMapRefresh(u64),
    BagMapTilesReady(Vec<TileKey>, Result<bag_map::TileBatch, String>),
    OpenPdokLicense,
    BagDownload,
    BagPathChosen(BagBounds, BagLod, Option<PathBuf>),
    BagPoll,
    CancelBag,
    BagReady(Result<(PathBuf, pointcloud_core::BagStats), String>),
    OpenBagLicense,
    /// Switch a built-in extension, named by its id, on or off.
    ExtensionEnabled(&'static str, bool),
    TranslateX(String),
    TranslateY(String),
    TranslateZ(String),
    ScaleAxis(usize, String),
    ApplyTranslation,
    ApplyScale,
    ScaleReady(u64, Result<[f64; 3], String>),
    ScalePoll(u64),
    CancelScale,
    ResetTransform,
    BuildIndex,
    IndexPoll,
    CancelIndex,
    IndexReady(Arc<PointCloud>, Result<Arc<OctreeIndex>, String>),
    AutoIndexReady(Arc<PointCloud>, Result<Arc<OctreeIndex>, String>),
    SetAutoIndex(bool),
    CachedIndexReady(Arc<PointCloud>, Result<Option<Arc<OctreeIndex>>, String>),
    LoadDetail,
    RefreshDetail(u64),
    DetailPreview(u64, Vec<(usize, Vec<IndexedPoint>)>),
    DetailReady(u64, Result<Vec<(usize, Vec<IndexedPoint>)>, String>),
    ExportFormat(ExportFormat),
    Exported(Result<PathBuf, String>),
    SaveCompleted(Option<Result<PathBuf, String>>),
    Select(usize),
    SetVisible(usize, bool),
    /// A click on a row of the project list; Shift extends the selection to
    /// that row and Ctrl toggles it.
    LayerClick(usize),
    /// The visibility and remove controls of a row, which act on every
    /// selected row when their own row is selected.
    LayerVisible(usize, bool),
    LayerRemove(usize),
    SetMeshVisible(usize, bool),
    Remove(usize),
    ColorMode(ColorMode),
    PointSize(f32),
    SetEyeDome(bool),
    EyeDomeStrength(f32),
    ShowScanPoses(bool),
    ExpandScanPoses(bool),
    FitScanPoses,
    CenterScanPose(usize, usize),
    StationPhotosReady(PathBuf, Result<Vec<Arc<PhotoSet>>, String>),
    EnterPanorama(usize, usize),
    LeaveWalk,
    WalkLook(f32, f32),
    WalkZoom(f32),
    WalkKey(WalkKey, bool),
    ModelKey(ModelKey),
    Modifiers(iced::keyboard::Modifiers),
    WalkTick(Instant),
    WalkStop,
    PanoramaReady(PathBuf, usize, Result<Arc<PhotoSet>, String>),
    Budget(u32),
    FilterClass(u8, bool),
    SetSectionEnabled(bool),
    SectionMin(usize, f32),
    SectionMax(usize, f32),
    SectionHandleDelta(usize, bool, f32),
    SectionCoordinate(usize, bool, String),
    ApplySectionCoordinates,
    SectionRotationInput(String),
    ApplySectionRotation,
    AlignSectionToWalls,
    /// The direction of the walls found for the section box as it was set.
    SectionWallsFound(
        OrientedBox,
        Result<Option<pointcloud_core::WallDirection>, String>,
    ),
    ResetSectionBox,
    ZoomToSection,
    FitSectionToSelection,
    ZoomToSelection,
    SelectionBoundsReady(
        bool,
        u64,
        Vec<(usize, Arc<SelectionMask>)>,
        Result<(Bounds, u64), String>,
    ),
    Orbit(f32, f32),
    /// A double click at a pixel of the scene: the drawn point there becomes
    /// the orbit point.
    PickOrbitPoint([f32; 2], Size),
    OrbitPointPicked(Option<[f64; 3]>),
    Pan(f32, f32),
    FinishPan(f32, f32),
    FinishOrbit(f32, f32),
    NavigationFinished,
    Zoom(f32, [f32; 2], Size),
    ViewportSize(Size),
    ResetCamera,
    CameraPreset(CameraPreset),
    CubeCorner([i8; 3]),
    CubeEdge([i8; 3]),
    Views(views::ViewAction),
    ShowContextMenu([f32; 2]),
    ContextAction(ContextAction),
    DismissContextMenu,
    Escape,
    CancelSelection,
    ToggleBoxSelect,
    TogglePickSelect,
    Measure(measure::MeasureAction),
    Drawing(drawing::DrawingAction),
    ClosedMesh(closed_mesh::ClosedMeshAction),
    Faces(faces::FaceAction),
    ClearSelection,
    SelectionDrag([f32; 2], [f32; 2]),
    BoxSelect {
        start: [f32; 2],
        end: [f32; 2],
        size: Size,
    },
    SelectionReady(u64, Result<Vec<(usize, Arc<SelectionMask>)>, String>),
    PickReady(u64, usize, Result<Option<IndexedPoint>, String>),
}

struct Studio {
    api_receiver: Option<
        Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<native_api::ApiRequest>>>,
    >,
    api_handle: Option<native_api::ApiHandle>,
    /// Files the system hands to the application after it started; only
    /// macOS delivers files this way.
    opened_files: Option<Arc<tokio::sync::Mutex<tokio::sync::mpsc::UnboundedReceiver<PathBuf>>>>,
    api_jobs: HashMap<String, Value>,
    api_job_order: VecDeque<String>,
    imports: HashMap<u64, ImportJob>,
    /// Layers shown from scan metadata while their import still reads points.
    import_headers: HashMap<u64, Arc<PointCloud>>,
    /// Points each import expects to read, for those whose source states it.
    import_expected: HashMap<u64, u64>,
    /// Imports started since the window last had none under way.
    opening_total: usize,
    /// When each task that reports progress was first seen.
    progress_marks: HashMap<open_progress::Phase, open_progress::Mark>,
    /// The Settings dialog, while it is open.
    settings: Option<settings_dialog::SettingsDialog>,
    /// The camera as the application last framed it; a view the user changed
    /// is left alone when further scans arrive.
    auto_camera: Option<(f32, f32, f32, [f32; 2])>,
    next_import_id: u64,
    /// Paths dropped on the window that are waiting to be opened together.
    dropped_paths: Vec<PathBuf>,
    clouds: Vec<CloudEntry>,
    undo_deletions: Vec<EditBatch>,
    redo_deletions: Vec<EditBatch>,
    active: Option<usize>,
    status: String,
    export_format: ExportFormat,
    decimation_stride: u64,
    thin_percent: u8,
    thin_pending: bool,
    surface_settings: [String; 3],
    translate_x: String,
    translate_y: String,
    translate_z: String,
    scale_inputs: [String; 3],
    scale_job: Option<ScaleJob>,
    next_scale_job_id: u64,
    bag_panel: bool,
    bag_fields: [String; 4],
    bag_lod: BagLod,
    /// The save dialog of a download is open.
    bag_dialog_pending: bool,
    bag_job: Option<bag_panel::BagJob>,
    bag_last_stats: Option<pointcloud_core::BagStats>,
    /// Why the last download failed, until the next one starts.
    bag_last_error: Option<String>,
    bag_map_center: [f64; 2],
    bag_map_zoom: u8,
    bag_map_drawing: bool,
    bag_map_revision: u64,
    bag_map_tiles: HashMap<TileKey, ::image::RgbaImage>,
    bag_map_raster: iced::widget::image::Handle,
    bag_map_loading: HashSet<TileKey>,
    color_mode: ColorMode,
    point_size: f32,
    eye_dome: bool,
    eye_dome_strength: f32,
    show_scan_poses: bool,
    expand_scan_poses: bool,
    /// Small station photos in arrival order; the atlas is rebuilt from them.
    station_photos: Vec<Arc<PhotoSet>>,
    photo_atlas: Option<Arc<PhotoAtlas>>,
    photo_loading: HashSet<PathBuf>,
    /// First-person camera, while walking or standing inside a station.
    walk: Option<WalkView>,
    /// The photo station the walking camera stands in, with its full-size
    /// photos once decoded.
    walk_station: Option<(usize, usize)>,
    panorama_photos: Option<Arc<PhotoSet>>,
    /// Movement keys held down, indexed by `WalkKey`, and the faster pace.
    walk_keys: [bool; 6],
    walk_fast: bool,
    walk_tick: Option<Instant>,
    /// Modifier keys held down, for range and toggle clicks in the project list.
    modifiers: iced::keyboard::Modifiers,
    budget: u32,
    filter_ground: bool,
    filter_vegetation: bool,
    filter_buildings: bool,
    filter_other: bool,
    class_visibility: ClassVisibility,
    /// Classification codes found in the open previews, with the clouds they
    /// were collected from so the scan is repeated only when those change.
    class_codes: std::cell::RefCell<(Vec<usize>, Vec<u8>)>,
    section_enabled: bool,
    section_export_pending: bool,
    mesh_export_pending: bool,
    mesh_dialog_pending: bool,
    mesh_job: Option<MeshJob>,
    merge_job: Option<MergeJob>,
    merge_dialog_pending: bool,
    selection_bounds_pending: bool,
    /// The reference the percentages of the section box are taken of, in
    /// the frame of its turn: a turn of `section_rotation` about the
    /// vertical through the centre of the reference.
    section_reference_bounds: Option<Bounds>,
    section_min_percent: [f64; 3],
    section_max_percent: [f64; 3],
    section_coordinate_inputs: [[String; 2]; 3],
    /// The turn of the section box about the vertical, in degrees
    /// counter-clockwise from above, between -180 and 180.
    section_rotation: f64,
    section_rotation_input: String,
    /// Whether the walls in the section box are being looked for.
    section_align_pending: bool,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    /// The point of the scene the orbit camera turns about, picked with a
    /// double click; the centre of the scene when there is none.
    orbit_point: Option<[f64; 3]>,
    view_label: &'static str,
    views: views::ViewTool,
    viewport_size: Size,
    ribbon_viewport: Option<(f32, f32, f32)>,
    file_open: bool,
    /// The page the File view shows.
    file_page: FilePage,
    /// Which built-in optional features are switched on.
    extensions: extensions::Extensions,
    ui_theme: UiTheme,
    settings_revision: u64,
    box_select: bool,
    pick_mode: bool,
    measure: measure::MeasureTool,
    /// The Section drawing tool: its choices, its job and its preview.
    drawing: drawing::DrawingTool,
    /// The Closed mesh tool: its settings, its job and its last result.
    closed_mesh: closed_mesh::ClosedMeshTool,
    /// The Detect faces tool: its settings and its job. The faces it finds
    /// are kept with their scan.
    faces: faces::FaceTool,
    drag_rectangle: Option<([f32; 2], [f32; 2])>,
    context_menu: Option<[f32; 2]>,
    selection_pending: bool,
    selection_cancel: Arc<AtomicBool>,
    pending_delete: bool,
    index_pending: bool,
    index_progress: Option<Arc<Mutex<IndexProgress>>>,
    index_cancel: Arc<AtomicBool>,
    detail_pending: bool,
    detail_cancel: Arc<AtomicBool>,
    detail_loaded_revision: Option<u64>,
    detail_urgent_revision: Option<u64>,
    /// Revision the last viewport LOD request was started for.
    detail_request_revision: Option<u64>,
    lod_pace: Arc<LodPace>,
    auto_index: bool,
    revision: u64,
}

struct ImportJob {
    path: PathBuf,
    decoded: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

struct CloudEntry {
    cloud: Arc<PointCloud>,
    /// Stable identity for asynchronous work started before a LAS preview
    /// replaces the initial header-only cloud.
    load_identity: Arc<PointCloud>,
    /// Identifies a preview whose one-pass octree is still building.
    index_import_id: Option<u64>,
    transform: CloudTransform,
    centroid_cache: Option<CentroidCache>,
    mesh: Option<Arc<MeshGeometry>>,
    /// The open edges and connected parts of `mesh`, counted when it was
    /// made or read.
    mesh_topology: Option<pointcloud_core::MeshTopology>,
    mesh_visible: bool,
    /// The faces detected in this scan, a layer of its own beside the mesh.
    faces: Option<faces::FaceLayer>,
    bag_source: bool,
    visible: bool,
    selection: Option<Arc<SelectionMask>>,
    deleted: Option<Arc<DeletionMask>>,
    index: Option<Arc<OctreeIndex>>,
    auto_index_queued: bool,
    index_building: bool,
    detail_points: Option<Arc<[IndexedPoint]>>,
    /// Part of the selection in the project list, which the list's
    /// visibility and remove controls act on together.
    picked: bool,
}

/// The points read for each cloud of a refinement, by cloud index.
type LodSets = Vec<(usize, Vec<IndexedPoint>)>;

struct LodRefinement {
    sources: Vec<(usize, Arc<OctreeIndex>, CloudTransform, f32)>,
    source_weights: Vec<(f32, usize)>,
    requested: Vec<usize>,
    sampled_limits: Vec<usize>,
    samples: Vec<Vec<IndexedPoint>>,
    section: Option<OrientedBox>,
    projection: Projection,
    cancel: Arc<AtomicBool>,
    budget: usize,
    deep_zoom: bool,
    pace: Arc<LodPace>,
    /// What the sets now drawn put in the viewport of this request.
    shown: ScreenFill,
    /// Points drawn for visible clouds outside this request; they count
    /// toward the budget the renderer thins all sets to.
    drawn_elsewhere: usize,
    pass: u8,
}

impl LodRefinement {
    fn sample_pass(&mut self) -> Result<(), String> {
        let results: Result<Vec<_>, String> = self
            .sources
            .par_iter()
            .enumerate()
            .filter(|(slot, _)| self.requested[*slot] > self.sampled_limits[*slot])
            .map(|(slot, (_, tree, transform, _))| {
                let limit = self.requested[slot];
                let section = self.section;
                let projection = self.projection;
                let deep_zoom = self.deep_zoom;
                let projected =
                    |node_bounds| lod_node_span(*transform, section, projection, node_bounds);
                let exact = if deep_zoom {
                    tree.sample_visible_indexed_cancellable(
                        limit,
                        MAX_EXACT_VISIBLE_LOD_CANDIDATES,
                        |bounds| projected(bounds).is_some(),
                        |record| {
                            let xyz = transform.xyz(record.point.xyz);
                            section.is_none_or(|clip| clip.contains(xyz))
                                && projection.project(xyz).is_some()
                        },
                        || self.cancel.load(Ordering::Relaxed),
                    )
                    .map_err(|error| error.to_string())?
                } else {
                    None
                };
                exact
                    .map(Ok)
                    .unwrap_or_else(|| {
                        tree.sample_lod_indexed_cancellable(limit, projected, || {
                            self.cancel.load(Ordering::Relaxed)
                        })
                    })
                    .map(|points| (slot, points))
                    .map_err(|error| error.to_string())
            })
            .collect();
        for (slot, points) in results? {
            self.samples[slot] = points;
            self.sampled_limits[slot] = self.requested[slot];
        }
        Ok(())
    }

    fn next_limits(&self) -> Option<Vec<usize>> {
        rebalance_lod_limits(
            self.budget,
            &self.source_weights,
            &self.requested,
            &self.samples.iter().map(Vec::len).collect::<Vec<_>>(),
        )
    }

    /// The samples as a set to show in between. A slot the next pass reads
    /// again gives its points away: that pass replaces them before anything
    /// looks at them.
    fn snapshot(&mut self) -> LodSets {
        let (requested, sampled) = (&self.requested, &self.sampled_limits);
        self.sources
            .iter()
            .zip(&mut self.samples)
            .enumerate()
            .map(|(slot, ((index, _, _, _), points))| {
                let points = if requested[slot] > sampled[slot] {
                    std::mem::take(points)
                } else {
                    points.clone()
                };
                (*index, points)
            })
            .collect()
    }

    fn finish(self) -> Vec<(usize, Vec<IndexedPoint>)> {
        self.sources
            .into_iter()
            .zip(self.samples)
            .map(|((index, _, _, _), points)| (index, points))
            .collect()
    }

    /// Read on until there is a set worth showing in between, or `None` once
    /// the samples are final. A set that would thin the picture on screen is
    /// kept back, so the points do not flicker while the camera moves.
    fn advance(&mut self) -> Result<Option<LodSets>, String> {
        loop {
            let before = self.sampled_limits.clone();
            let started = Instant::now();
            self.sample_pass()?;
            // The exact scan at deep zoom costs the same for any limit, so
            // it says nothing about the pace of a regular pass.
            if !self.deep_zoom {
                let read = self
                    .samples
                    .iter()
                    .zip(before.iter().zip(&self.sampled_limits))
                    .filter(|(_, (before, after))| before != after)
                    .map(|(points, _)| points.len())
                    .sum();
                self.pace.record_read(read, started.elapsed());
            }
            if self.pass >= 2 {
                return Ok(None);
            }
            let Some(next) = self.next_limits() else {
                return Ok(None);
            };
            self.requested = next;
            self.pass += 1;
            let fresh = self.fill();
            if preview_improves(self.shown, fresh) {
                self.shown = fresh;
                return Ok(Some(self.snapshot()));
            }
        }
    }

    /// What the samples read so far would put in the viewport, thinned the
    /// way the renderer would thin them.
    fn fill(&self) -> ScreenFill {
        let mut fill = ScreenFill::default();
        let mut drawn = self.drawn_elsewhere;
        for ((_, _, transform, _), points) in self.sources.iter().zip(&self.samples) {
            fill.add(
                points,
                |record| transform.xyz(record.point.xyz),
                self.projection,
                self.section,
            );
            drawn = drawn.saturating_add(points.len());
        }
        fill.points /= drawn.div_ceil(self.budget.max(1)).max(1);
        fill
    }
}

struct CentroidCache {
    source_xyz: [f64; 3],
    deleted: Option<Arc<DeletionMask>>,
}

struct ScaleJob {
    id: u64,
    cloud_index: usize,
    source: Arc<PointCloud>,
    deleted: Option<Arc<DeletionMask>>,
    transform: CloudTransform,
    factors: [f64; 3],
    progress: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
}

fn same_deletion_mask(a: Option<&Arc<DeletionMask>>, b: Option<&Arc<DeletionMask>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

struct EditBatch {
    members: Vec<(Arc<PointCloud>, Arc<SelectionMask>)>,
}

impl CloudEntry {
    /// Put another cloud of the same source in the layer. A selection or
    /// deletion made on a provisional cloud is sized for its stated count
    /// and does not carry over to a cloud with another count.
    fn replace_cloud(&mut self, cloud: Arc<PointCloud>) {
        if self.cloud.provisional && self.cloud.total_points != cloud.total_points {
            self.selection = None;
            self.deleted = None;
        }
        self.cloud = cloud;
    }

    fn matches_source(&self, source: &Arc<PointCloud>) -> bool {
        Arc::ptr_eq(&self.cloud, source) || Arc::ptr_eq(&self.load_identity, source)
    }

    fn view_len(&self) -> usize {
        self.detail_points
            .as_ref()
            .map_or(self.cloud.points.len(), |points| points.len())
    }

    fn view_records(&self) -> Box<dyn Iterator<Item = IndexedPoint> + '_> {
        let transform = self.transform;
        if let Some(detail) = &self.detail_points {
            Box::new(
                detail
                    .iter()
                    .copied()
                    .map(move |record| transform.record(record)),
            )
        } else {
            Box::new(
                self.cloud
                    .points
                    .iter()
                    .copied()
                    .zip(self.cloud.point_ordinals.iter().copied())
                    .map(move |(point, ordinal)| transform.record(IndexedPoint { point, ordinal })),
            )
        }
    }

    fn bounds(&self) -> Bounds {
        self.transform.bounds(self.cloud.bounds)
    }

    fn deleted_count(&self) -> u64 {
        self.deleted.as_ref().map_or(0, |mask| mask.count)
    }

    fn remaining_count(&self) -> u64 {
        self.cloud.total_points - self.deleted_count()
    }

    fn record_visible(&self, record: IndexedPoint) -> bool {
        self.deleted
            .as_ref()
            .is_none_or(|mask| record.ordinal != u64::MAX && !mask.contains(record.ordinal))
    }
}

impl Default for Studio {
    fn default() -> Self {
        let surface = SurfaceMeshConfig::default();
        let settings = preferences::load();
        Self {
            api_receiver: None,
            api_handle: None,
            opened_files: None,
            api_jobs: HashMap::new(),
            api_job_order: VecDeque::new(),
            imports: HashMap::new(),
            import_headers: HashMap::new(),
            import_expected: HashMap::new(),
            opening_total: 0,
            progress_marks: HashMap::new(),
            settings: None,
            auto_camera: None,
            next_import_id: 0,
            dropped_paths: Vec::new(),
            clouds: Vec::new(),
            undo_deletions: Vec::new(),
            redo_deletions: Vec::new(),
            active: None,
            status: "Open a LAS, LAZ, PLY, PCD, PTX, OBJ, OFF or STL file".into(),
            export_format: ExportFormat::PlyBinary,
            decimation_stride: 10,
            thin_percent: 50,
            thin_pending: false,
            surface_settings: [
                surface.max_vertices.to_string(),
                surface.neighbors.to_string(),
                surface.max_edge_factor.to_string(),
            ],
            translate_x: "0".into(),
            translate_y: "0".into(),
            translate_z: "0".into(),
            scale_inputs: std::array::from_fn(|_| "1".into()),
            scale_job: None,
            next_scale_job_id: 0,
            bag_panel: false,
            bag_fields: std::array::from_fn(|_| String::new()),
            bag_lod: BagLod::Lod22,
            bag_dialog_pending: false,
            bag_job: None,
            bag_last_stats: None,
            bag_last_error: None,
            bag_map_center: [121_000.0, 487_000.0],
            bag_map_zoom: 11,
            bag_map_drawing: false,
            bag_map_revision: 0,
            bag_map_tiles: HashMap::new(),
            bag_map_raster: bag_map::compose_raster(
                MapView {
                    center: [121_000.0, 487_000.0],
                    zoom: 11,
                    width: bag_map::WIDTH,
                    height: bag_map::HEIGHT,
                },
                &HashMap::new(),
            ),
            bag_map_loading: HashSet::new(),
            color_mode: settings.color_mode,
            point_size: settings.point_size,
            eye_dome: settings.eye_dome,
            eye_dome_strength: settings.eye_dome_strength,
            show_scan_poses: settings.show_scan_poses,
            expand_scan_poses: false,
            station_photos: Vec::new(),
            photo_atlas: None,
            photo_loading: HashSet::new(),
            walk: None,
            walk_station: None,
            panorama_photos: None,
            walk_keys: [false; 6],
            walk_fast: false,
            walk_tick: None,
            modifiers: iced::keyboard::Modifiers::default(),
            budget: settings.budget,
            // The four class groups no longer have switches; classes are
            // shown or hidden one by one in the project panel.
            filter_ground: true,
            filter_vegetation: true,
            filter_buildings: true,
            filter_other: true,
            class_visibility: ClassVisibility::default(),
            class_codes: std::cell::RefCell::new((Vec::new(), Vec::new())),
            section_enabled: false,
            section_export_pending: false,
            mesh_export_pending: false,
            mesh_dialog_pending: false,
            mesh_job: None,
            merge_job: None,
            merge_dialog_pending: false,
            selection_bounds_pending: false,
            section_reference_bounds: None,
            section_min_percent: [0.0; 3],
            section_max_percent: [100.0; 3],
            section_coordinate_inputs: std::array::from_fn(|_| {
                std::array::from_fn(|_| String::new())
            }),
            section_rotation: 0.0,
            section_rotation_input: "0".into(),
            section_align_pending: false,
            yaw: -0.8,
            pitch: 0.6,
            zoom: 1.0,
            pan: [0.0, 0.0],
            orbit_point: None,
            view_label: "ISOMETRIC",
            views: views::ViewTool::load(),
            viewport_size: Size::new(915.0, 743.0),
            ribbon_viewport: None,
            file_open: false,
            file_page: FilePage::default(),
            extensions: extensions::Extensions::load(),
            ui_theme: UiTheme::load(),
            settings_revision: 0,
            box_select: false,
            pick_mode: false,
            measure: measure::MeasureTool::default(),
            drawing: drawing::DrawingTool::default(),
            closed_mesh: closed_mesh::ClosedMeshTool::default(),
            faces: faces::FaceTool::default(),
            drag_rectangle: None,
            context_menu: None,
            selection_pending: false,
            selection_cancel: Arc::new(AtomicBool::new(false)),
            pending_delete: false,
            index_pending: false,
            index_progress: None,
            index_cancel: Arc::new(AtomicBool::new(false)),
            detail_pending: false,
            detail_cancel: Arc::new(AtomicBool::new(false)),
            detail_loaded_revision: None,
            detail_urgent_revision: None,
            detail_request_revision: None,
            lod_pace: Arc::default(),
            auto_index: settings.auto_index,
            revision: 0,
        }
    }
}

impl Studio {
    fn preferences(&self) -> preferences::Preferences {
        preferences::Preferences {
            color_mode: self.color_mode,
            point_size: self.point_size,
            eye_dome: self.eye_dome,
            eye_dome_strength: self.eye_dome_strength,
            show_scan_poses: self.show_scan_poses,
            budget: self.budget,
            auto_index: self.auto_index,
            filter_ground: self.filter_ground,
            filter_vegetation: self.filter_vegetation,
            filter_buildings: self.filter_buildings,
            filter_other: self.filter_other,
        }
    }

    fn active_camera_source(&self) -> Option<PathBuf> {
        self.active
            .and_then(|index| self.clouds.get(index))
            .map(|entry| self.views.source_of(&entry.cloud.path))
    }

    fn mesh_filter(&self) -> ClassFilter {
        ClassFilter {
            ground: self.filter_ground,
            vegetation: self.filter_vegetation,
            buildings: self.filter_buildings,
            other: self.filter_other,
            classes: self.class_visibility,
            section: self.section_box(),
        }
    }

    /// Read the stations and photo list of an E57 scan from its metadata, so
    /// they can be shown while the import decodes the points.
    fn header_task(id: u64, path: PathBuf) -> Task<Message> {
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("e57"))
        {
            return Task::none();
        }
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    pointcloud_core::open_e57_header(&path)
                        .map(Arc::new)
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            move |result| Message::HeaderLoaded(id, result),
        )
    }

    /// Add the layer of an import that is still reading its source, showing
    /// what is known of it: its metadata, or the points read so far.
    fn show_import_layer(&mut self, id: u64, header: Arc<PointCloud>) -> Task<Message> {
        self.cancel_selection_for_scene_change();
        self.clouds.push(CloudEntry {
            cloud: Arc::clone(&header),
            load_identity: Arc::clone(&header),
            index_import_id: None,
            transform: CloudTransform::default(),
            centroid_cache: None,
            mesh: None,
            mesh_topology: None,
            mesh_visible: true,
            faces: None,
            bag_source: false,
            visible: true,
            selection: None,
            deleted: None,
            index: None,
            auto_index_queued: false,
            picked: false,
            index_building: false,
            detail_points: None,
        });
        self.import_headers.insert(id, Arc::clone(&header));
        self.revision += 1;
        self.active = Some(self.clouds.len() - 1);
        if self.section_enabled && self.section_reference_bounds.is_none() {
            self.reset_section_reference();
            self.sync_section_coordinate_inputs();
        }
        self.frame_new_scene();
        self.station_photos_task(&header)
    }

    /// Put the checked result of an import in place: it replaces the layer
    /// shown from metadata when there is one, and becomes a new layer otherwise.
    fn finish_import(
        &mut self,
        header: Option<Arc<PointCloud>>,
        result: Result<Arc<PointCloud>, String>,
    ) -> Task<Message> {
        let shown =
            header.filter(|header| self.clouds.iter().any(|entry| entry.matches_source(header)));
        match (shown, result) {
            (Some(header), Ok(cloud)) => {
                let scene = combined_bounds(&self.clouds);
                let photos = self.station_photos_task(&cloud);
                let refined = self.update(Message::Refined(header, Ok(Arc::clone(&cloud))));
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| Arc::ptr_eq(&entry.cloud, &cloud))
                {
                    entry.bag_source = is_bag3d_mesh(&cloud.path);
                }
                self.revision += 1;
                self.reframe_after_replacement(scene);
                let detail = Self::mesh_task(&cloud).unwrap_or_else(|| self.schedule_detail());
                Task::batch([refined, photos, detail])
            }
            (Some(header), Err(error)) => {
                self.remove_header_layer(&header);
                self.status = error;
                Task::none()
            }
            (None, result) => self.update(Message::Loaded(result)),
        }
    }

    /// Read the faces of a source that can hold a mesh, for the layer that
    /// shows `cloud`.
    fn mesh_task(cloud: &Arc<PointCloud>) -> Option<Task<Message>> {
        let format = cloud
            .path
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase);
        if !matches!(
            format.as_deref(),
            Some("obj" | "ply" | "off" | "stl" | "dxf")
        ) {
            return None;
        }
        let path = cloud.path.clone();
        let source = Arc::clone(cloud);
        Some(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || mesh_export::read_measured(&path))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
            },
            move |result| Message::MeshLoaded(Arc::clone(&source), result),
        ))
    }

    fn remove_header_layer(&mut self, header: &Arc<PointCloud>) {
        if let Some(index) = self
            .clouds
            .iter()
            .position(|entry| entry.matches_source(header))
        {
            self.clouds.remove(index);
            self.leave_walk();
            self.rebuild_photo_atlas();
            self.revision += 1;
            self.active = self
                .active
                .filter(|_| !self.clouds.is_empty())
                .map(|active| active.min(self.clouds.len() - 1));
        }
    }

    /// Box around the scanner stations with room for what they scanned.
    fn station_focus(&self) -> Option<Bounds> {
        let mut focus: Option<Bounds> = None;
        for entry in self.clouds.iter().filter(|entry| entry.visible) {
            for pose in &entry.cloud.scan_poses {
                include_bounds(&mut focus, entry.transform.xyz(pose.position));
            }
        }
        let mut focus = focus?;
        let margin = (focus.extent() * 0.5).max(6.0);
        for axis in 0..3 {
            let margin = if axis == 2 { margin.min(4.0) } else { margin };
            focus.min[axis] -= margin;
            focus.max[axis] += margin;
        }
        Some(focus)
    }

    /// Box to frame when the scene bounds would make the view tiny: around
    /// the stations of scans that have them, otherwise around the bulk of the
    /// points when a few stray far ones stretch the bounds.
    fn scene_focus(&self) -> Option<Bounds> {
        if let Some(stations) = self.station_focus() {
            return Some(stations);
        }
        let mut focus: Option<Bounds> = None;
        let mut tightened = false;
        for entry in self.clouds.iter().filter(|entry| entry.visible) {
            let bulk = bulk_bounds(&entry.cloud.points)
                .filter(|bulk| bulk.extent() < entry.cloud.bounds.extent() * 0.5);
            match bulk {
                Some(bulk) => {
                    tightened = true;
                    let margin = bulk.extent() * 0.15;
                    include_bounds(
                        &mut focus,
                        entry.transform.xyz(bulk.min.map(|v| v - margin)),
                    );
                    include_bounds(
                        &mut focus,
                        entry.transform.xyz(bulk.max.map(|v| v + margin)),
                    );
                }
                None => {
                    let bounds = entry.bounds();
                    include_bounds(&mut focus, bounds.min);
                    include_bounds(&mut focus, bounds.max);
                }
            }
        }
        focus.filter(|_| tightened)
    }

    /// Frame the scene after a scan arrived, unless the user moved the camera
    /// since the application last framed it. Scans with stations are framed
    /// around them: a few stray far points otherwise make the whole view tiny.
    fn frame_new_scene(&mut self) {
        let camera = (self.yaw, self.pitch, self.zoom, self.pan);
        if self.walk.is_some() || (self.clouds.len() > 1 && self.auto_camera != Some(camera)) {
            return;
        }
        if self.clouds.len() <= 1 {
            self.yaw = -0.8;
            self.pitch = 0.6;
            self.view_label = "ISOMETRIC";
        }
        self.zoom = 1.0;
        self.pan = [0.0, 0.0];
        self.orbit_point = None;
        if let (Some(scene), Some(focus)) = (combined_bounds(&self.clouds), self.scene_focus()) {
            if let Some((zoom, pan)) =
                camera_to_frame_bounds(scene, focus, self.yaw, self.pitch, self.viewport_size)
            {
                // Never zoom out beyond the whole scene.
                if zoom < 1.0 {
                    self.zoom = zoom;
                    self.pan = pan;
                }
            }
        }
        self.auto_camera = Some((self.yaw, self.pitch, self.zoom, self.pan));
    }

    /// Decode the small ball photos of a newly opened source in the background.
    fn station_photos_task(&mut self, cloud: &Arc<PointCloud>) -> Task<Message> {
        if cloud.scan_images.is_empty()
            || self.photo_loading.contains(&cloud.path)
            || self
                .station_photos
                .iter()
                .any(|set| set.source == cloud.path)
        {
            return Task::none();
        }
        self.photo_loading.insert(cloud.path.clone());
        let source = cloud.path.clone();
        let images = cloud.scan_images.clone();
        let key = source.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    station_photos::load_ball_photos(&source, &images)
                        .map(|sets| sets.into_iter().map(Arc::new).collect())
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            move |result| Message::StationPhotosReady(key.clone(), result),
        )
    }

    /// Keep ball photos for open sources only and publish them to the viewport.
    fn rebuild_photo_atlas(&mut self) {
        self.station_photos.retain(|set| {
            self.clouds
                .iter()
                .any(|entry| entry.cloud.path == set.source)
        });
        if self.station_photos.is_empty() {
            self.photo_atlas = None;
            return;
        }
        let (atlas, dropped) = PhotoAtlas::build(self.station_photos.iter().cloned());
        if dropped > 0 {
            self.status = format!(
                "Station photos shown for {} stations; {dropped} more do not fit",
                atlas.sets.len()
            );
        }
        self.photo_atlas = Some(Arc::new(atlas));
    }

    /// Size of the scene as it was last drawn, which the viewport only
    /// reports when the pointer moves over it or turns the wheel.
    fn scene_size(&self) -> Size {
        self.views
            .canvas_bounds()
            .map(|canvas| canvas.size())
            .filter(|size| size.width > 0.0 && size.height > 0.0)
            .unwrap_or(self.viewport_size)
    }

    fn camera_value(&self) -> Value {
        json!({
            "yaw": self.yaw,
            "pitch": self.pitch,
            "zoom": self.zoom,
            "pan": self.pan,
            "view": self.view_label,
            "orbit_point": self.orbit_point,
        })
    }

    fn orbit_camera(&self) -> orbit_point::OrbitCamera {
        orbit_point::OrbitCamera {
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
            pan: self.pan,
        }
    }

    /// Turn the orbit camera by these angles in radians: about the orbit
    /// point while it is in view, about the centre of the scene otherwise.
    fn turn_orbit(&mut self, yaw: f32, pitch: f32) {
        let yaw = (self.yaw + yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
            - std::f32::consts::PI;
        let pitch = (self.pitch + pitch).clamp(-1.56, 1.56);
        let about = self
            .orbit_point
            .zip(combined_bounds(&self.clouds))
            .and_then(|(point, scene)| {
                orbit_point::turn_about(
                    scene,
                    self.orbit_camera(),
                    yaw,
                    pitch,
                    point,
                    self.scene_size(),
                )
            });
        if let Some(camera) = about {
            self.zoom = camera.zoom;
            self.pan = camera.pan;
        }
        self.yaw = yaw;
        self.pitch = pitch;
        self.view_label = i18n::key("CUSTOM");
        self.revision += 1;
    }

    /// The drawn point nearest to the eye under a pixel of the scene.
    fn orbit_point_at(&self, pointer: [f32; 2], size: Size) -> impl FnOnce() -> Option<[f64; 3]> {
        let views: Vec<_> = self
            .clouds
            .iter()
            .map(|entry| PickView {
                cloud: Arc::clone(&entry.cloud),
                detail: entry.detail_points.as_ref().map(Arc::clone),
                deleted: entry.deleted.as_ref().map(Arc::clone),
                transform: entry.transform,
                visible: entry.visible,
            })
            .collect();
        let budget = self.budget as usize;
        let filter = self.mesh_filter();
        let projection = combined_bounds(&self.clouds)
            .filter(|_| self.walk.is_none())
            .map(|scene| self.projection(scene, size.width, size.height));
        move || {
            pick_surface(
                &views,
                budget,
                projection?,
                pointer,
                orbit_point::PICK_RADIUS,
                filter,
            )
        }
    }

    /// Look for the orbit point under a double click on a worker thread.
    fn pick_orbit_point(&mut self, pointer: [f32; 2], size: Size) -> Task<Message> {
        if self.walk.is_some() || self.clouds.is_empty() {
            return Task::none();
        }
        let pick = self.orbit_point_at(pointer, size);
        Task::perform(
            async move { tokio::task::spawn_blocking(pick).await.ok().flatten() },
            Message::OrbitPointPicked,
        )
    }

    /// Turn about this point from now on, or about the centre of the scene.
    fn set_orbit_point(&mut self, point: Option<[f64; 3]>) {
        self.orbit_point = point;
        self.status = if point.is_some() {
            i18n::tr("Orbit point set: the view turns about the point that was double-clicked")
        } else {
            i18n::tr("No point there: the view turns about the centre of the model")
        }
        .into();
    }

    fn leave_walk(&mut self) -> bool {
        self.panorama_photos = None;
        self.walk_station = None;
        self.walk_keys = [false; 6];
        self.walk_tick = None;
        self.walk.take().is_some()
    }

    fn walk_value(&self) -> Value {
        self.walk.map_or(Value::Null, |view| {
            json!({
                "eye": view.eye,
                "yaw": view.yaw,
                "pitch": view.pitch,
                "field_of_view": view.field_of_view,
                "station": self.walk_station.map(|(cloud, station)| json!({
                    "index": cloud,
                    "station": station,
                    "label": self.clouds.get(cloud)
                        .and_then(|entry| entry.cloud.scan_poses.get(station))
                        .map(|pose| pose.label.clone()),
                    "full_resolution": self.panorama_photos.is_some(),
                })),
            })
        })
    }

    /// The camera in use: the walking camera when active, the orbit camera otherwise.
    fn projection(&self, scene: Bounds, width: f32, height: f32) -> Projection {
        self.point_viewport().projection(scene, width, height)
    }

    /// Station with photos whose ball contains a position.
    fn photo_station_at(&self, position: [f64; 3]) -> Option<(usize, usize)> {
        let atlas = self.photo_atlas.as_deref()?;
        let mut nearest: Option<(usize, usize, f64)> = None;
        for (cloud, entry) in self
            .clouds
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.visible)
        {
            for (station, pose) in entry.cloud.scan_poses.iter().enumerate() {
                if atlas.slot(&entry.cloud.path, station).is_none() {
                    continue;
                }
                let centre = entry.transform.xyz(pose.position);
                let distance = (0..3)
                    .map(|axis| (centre[axis] - position[axis]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                if distance <= station_photos::BALL_RADIUS
                    && nearest.is_none_or(|(_, _, best)| distance < best)
                {
                    nearest = Some((cloud, station, distance));
                }
            }
        }
        nearest.map(|(cloud, station, _)| (cloud, station))
    }

    /// Decode the full-size photos of the station the walking camera stands in.
    fn panorama_task(&self, cloud: usize, station: usize) -> Task<Message> {
        let Some(entry) = self.clouds.get(cloud) else {
            return Task::none();
        };
        let source = entry.cloud.path.clone();
        let images = entry.cloud.scan_images.clone();
        let key = source.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    station_photos::load_panorama(&source, station, &images).map(Arc::new)
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            move |result| Message::PanoramaReady(key.clone(), station, result),
        )
    }

    /// Note which photo station the walking camera stands in after it moved,
    /// and fetch that station's photos when it stepped into one.
    fn sync_walk_station(&mut self) -> Task<Message> {
        let station = self.walk.and_then(|view| self.photo_station_at(view.eye));
        if station == self.walk_station {
            return Task::none();
        }
        self.walk_station = station;
        self.panorama_photos = None;
        match station {
            Some((cloud, station)) => {
                if let Some(pose) = self
                    .clouds
                    .get(cloud)
                    .and_then(|entry| entry.cloud.scan_poses.get(station))
                {
                    self.status = format!("Station photo: {}", pose.label);
                }
                self.panorama_task(cloud, station)
            }
            None => {
                self.status =
                    "Walking · W A S D to move, Q E down and up, Shift faster, Esc to leave".into();
                Task::none()
            }
        }
    }

    /// Start walking from where the orbit camera looks: same viewing
    /// direction, and what lies in the middle of the scene keeps its size.
    fn start_walk(&mut self) -> bool {
        let Some(scene) = combined_bounds(&self.clouds) else {
            return false;
        };
        let size = self.viewport_size;
        if size.width <= 0.0 || size.height <= 0.0 {
            return false;
        }
        let orbit = Projection::new(
            scene,
            self.yaw,
            self.pitch,
            self.zoom,
            self.pan,
            size.width,
            size.height,
        );
        let centre = scene.center();
        // Direction through the middle of the viewport, which a panned orbit
        // view does not share with its camera axis.
        let ray: [f64; 3] = std::array::from_fn(|axis| {
            orbit.right[axis] * -f64::from(self.pan[0]) + orbit.up[axis] * f64::from(self.pan[1])
                - orbit.toward_camera[axis] * orbit.scale
        });
        let length = ray.iter().map(|value| value * value).sum::<f64>().sqrt();
        if !length.is_finite() || length <= f64::EPSILON {
            return false;
        }
        let forward = ray.map(|value| value / length);
        let orbit_eye: [f64; 3] =
            std::array::from_fn(|axis| centre[axis] + orbit.toward_camera[axis] * orbit.eye[2]);
        let mut view = WalkView::new(orbit_eye, forward[1].atan2(forward[0]) as f32);
        view.pitch = (forward[2].clamp(-1.0, 1.0).asin() as f32).clamp(-1.55, 1.55);
        let depth = orbit.eye[2] * f64::from(view.focal(size)) / orbit.scale;
        view.eye =
            std::array::from_fn(|axis| orbit_eye[axis] + forward[axis] * (orbit.eye[2] - depth));
        self.walk = Some(view);
        self.walk_station = None;
        self.panorama_photos = None;
        self.context_menu = None;
        self.box_select = false;
        self.pick_mode = false;
        self.drag_rectangle = None;
        true
    }

    /// Walking pace in scene units per second.
    fn walk_speed(&self) -> f64 {
        let extent = combined_bounds(&self.clouds).map_or(10.0, Bounds::extent);
        (extent * 0.08).clamp(0.8, 400.0) * if self.walk_fast { 4.0 } else { 1.0 }
    }

    /// Cloud indices in the order the project list shows them: by name,
    /// whatever order their imports finished in.
    fn layer_order(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.clouds.len()).collect();
        order.sort_by(|a, b| {
            project_open::natural_cmp(
                display_name(&self.clouds[*a].cloud.path),
                display_name(&self.clouds[*b].cloud.path),
            )
            .then(a.cmp(b))
        });
        order
    }

    /// The clouds a control on the row of `index` acts on: all selected rows
    /// when that row is one of them, otherwise the row alone.
    fn layer_group(&self, index: usize) -> Vec<usize> {
        match self.clouds.get(index) {
            Some(entry) if entry.picked => (0..self.clouds.len())
                .filter(|cloud| self.clouds[*cloud].picked)
                .collect(),
            Some(_) => vec![index],
            None => Vec::new(),
        }
    }

    fn remove_clouds(&mut self, mut indices: Vec<usize>) -> Task<Message> {
        indices.retain(|index| *index < self.clouds.len());
        indices.sort_unstable();
        indices.dedup();
        let Some(&first) = indices.first() else {
            return Task::none();
        };
        self.cancel_selection_for_scene_change();
        for &index in indices.iter().rev() {
            if self.clouds[index].index_import_id.is_some() {
                self.index_cancel.store(true, Ordering::Relaxed);
            }
            let importing = self
                .import_headers
                .iter()
                .find(|(_, header)| self.clouds[index].matches_source(header))
                .map(|(id, _)| *id);
            if let Some(id) = importing {
                // Shown from metadata only: stop reading its points.
                self.import_headers.remove(&id);
                if let Some(job) = self.imports.get(&id) {
                    job.cancel.store(true, Ordering::Relaxed);
                }
            }
            self.clouds.remove(index);
        }
        self.leave_walk();
        self.rebuild_photo_atlas();
        self.undo_deletions.clear();
        self.redo_deletions.clear();
        self.pending_delete = false;
        self.revision += 1;
        self.active = if self.clouds.is_empty() {
            None
        } else {
            Some(first.min(self.clouds.len() - 1))
        };
        self.schedule_detail()
    }

    /// Classification codes that occur in the open clouds, ascending.
    fn class_codes(&self) -> Vec<u8> {
        let key: Vec<usize> = self
            .clouds
            .iter()
            .map(|entry| Arc::as_ptr(&entry.cloud) as usize)
            .collect();
        let mut cache = self.class_codes.borrow_mut();
        if cache.0 != key {
            let mut present = [false; 256];
            for entry in self
                .clouds
                .iter()
                .filter(|entry| entry.cloud.has_classification)
            {
                for point in &entry.cloud.points {
                    if let Some(code) = point.classification {
                        present[usize::from(code)] = true;
                    }
                }
            }
            *cache = (
                key,
                (0..=u8::MAX)
                    .filter(|code| present[usize::from(*code)])
                    .collect(),
            );
        }
        cache.1.clone()
    }

    fn queue_preferences_save(&mut self) -> Task<Message> {
        self.settings_revision = self.settings_revision.wrapping_add(1);
        let revision = self.settings_revision;
        Task::perform(
            async move {
                tokio::time::sleep(Duration::from_millis(350)).await;
                revision
            },
            Message::PersistSettings,
        )
    }

    fn surface_mesh_config(&self) -> Result<SurfaceMeshConfig, String> {
        let max_vertices = self.surface_settings[0]
            .trim()
            .parse()
            .map_err(|_| "3D surface vertices must be a whole number".to_string())?;
        let neighbors = self.surface_settings[1]
            .trim()
            .parse()
            .map_err(|_| "3D surface neighbors must be a whole number".to_string())?;
        let max_edge_factor = self.surface_settings[2]
            .trim()
            .parse()
            .map_err(|_| "3D surface edge factor must be a number".to_string())?;
        let config = SurfaceMeshConfig {
            max_vertices,
            neighbors,
            max_edge_factor,
        };
        config.validate().map_err(|error| error.to_string())?;
        Ok(config)
    }

    fn set_surface_mesh_config(&mut self, config: SurfaceMeshConfig) {
        self.surface_settings = [
            config.max_vertices.to_string(),
            config.neighbors.to_string(),
            config.max_edge_factor.to_string(),
        ];
    }

    fn handle_api(&mut self, request: native_api::ApiRequest) -> Task<Message> {
        use native_api::ApiCommand;

        let (response, task) = match request.command {
            ApiCommand::Status => {
                let clouds: Vec<_> = self
                    .clouds
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| {
                        json!({
                            "index": index,
                            "path": entry.cloud.path,
                            "points": entry.cloud.total_points,
                            "remaining": entry.remaining_count(),
                            "selected": entry.selection.as_ref().map_or(0, |mask| mask.count),
                            "deleted": entry.deleted.as_ref().map_or(0, |mask| mask.count),
                            "visible": entry.visible,
                            "indexed": entry.index.is_some(),
                            "stations": entry.cloud.scan_poses.len(),
                            "station_photos": entry.cloud.scan_images.len(),
                            "view_sample": entry.view_len(),
                            "bounds": {"min": entry.bounds().min, "max": entry.bounds().max},
                            "transform": {"scale": entry.transform.scale, "offset": entry.transform.offset},
                            "mesh": mesh_export::mesh_value(entry),
                            "faces": faces::layer_value(&self.clouds, index),
                        })
                    })
                    .collect();
                let section = self.section_box().map(|_| self.section_value());
                let active_source = self.active_camera_source();
                let camera_views: Vec<_> = self
                    .views
                    .list
                    .iter()
                    .filter(|view| active_source.as_ref() == Some(&view.source))
                    .collect();
                let mut answer = (
                    json!({"ok": true, "result": {
                        "clouds": clouds,
                        "imports": self.imports.iter().map(|(id, job)| json!({
                            "id": id,
                            "path": job.path,
                            "decoded": job.decoded.load(Ordering::Relaxed),
                            "cancelling": job.cancel.load(Ordering::Relaxed),
                        })).collect::<Vec<_>>(),
                        "active": self.active,
                        "status": self.status,
                        "camera": self.camera_value(),
                        "viewport_size": [self.viewport_size.width, self.viewport_size.height],
                        "walk": self.walk_value(),
                        "photo_stations": self.photo_atlas.as_ref().map_or(0, |atlas| atlas.sets.len()),
                        "photos_loading": self.photo_loading.len(),
                        "camera_views": camera_views,
                        "views": self.views_value(),
                        "section": section,
                        "selected_points": self.selected_total(),
                        "selection_pending": self.selection_pending,
                        "selection_bounds_pending": self.selection_bounds_pending,
                        "measure": self.measure.value(),
                        "measure_mode": self.measure.mode.map(measure::MeasureMode::key),
                        "thin_pending": self.thin_pending,
                        "color_mode": self.color_mode.to_string(),
                        "theme": self.ui_theme.key(),
                        "hidden_classes": (0..=u8::MAX)
                            .filter(|code| !self.class_visibility.allows(Some(*code)))
                            .collect::<Vec<_>>(),
                        "eye_dome": self.eye_dome,
                        "eye_dome_strength": self.eye_dome_strength,
                        "point_size": self.point_size,
                        "budget": self.budget,
                        "auto_index": self.auto_index,
                        "surface_settings": {
                            "max_vertices": self.surface_settings[0],
                            "neighbors": self.surface_settings[1],
                            "edge_factor": self.surface_settings[2],
                        },
                        "mesh": self.mesh_job.as_ref().map(MeshJob::progress_value),
                        "merge": self.merge_job.as_ref().map(MergeJob::progress_value),
                        "index_progress": self.index_progress.as_ref().and_then(|value| value.lock().ok().map(|progress| json!({
                            "stage": match progress.stage {
                                IndexStage::ReadingSource => "reading_source",
                                IndexStage::BuildingTree => "building_tree",
                                IndexStage::Ready => "ready",
                            },
                            "completed": progress.completed,
                            "total": progress.total,
                            "depth": progress.depth,
                            "leaves": progress.leaves,
                            "settled": progress.settled,
                            "fraction": progress.fraction(),
                            "cancelling": self.index_cancel.load(Ordering::Relaxed),
                        }))),
                        "scale": self.scale_job.as_ref().map(|job| json!({
                            "source_index": job.cloud_index,
                            "completed": job.progress.load(Ordering::Relaxed),
                            "total": job.source.total_points,
                        })),
                        "api_port": self.api_handle.as_ref().map(|handle| handle.port),
                    }}),
                    Task::none(),
                );
                // The literal above is as large as its macro can expand; further
                // fields are added to the answer here.
                answer.0["result"]["detail_pending"] = Value::Bool(self.detail_pending);
                answer.0["result"]["language"] = Value::from(i18n::choice().key());
                answer.0["result"]["bag3d"] =
                    json!(self.bag_job.as_ref().map(bag_panel::BagJob::progress_value));
                answer.0["result"]["mesh_export_pending"] = Value::Bool(self.mesh_export_pending);
                answer.0["result"]["file_view"] = self.file_view_value();
                answer.0["result"]["drawing"] = self.drawing.value();
                answer.0["result"]["section_align_pending"] =
                    Value::Bool(self.section_align_pending);
                answer.0["result"]["closed_mesh"] = self.closed_mesh.value();
                answer.0["result"]["faces"] = self.faces_value();
                answer
            }
            ApiCommand::Job { id } => {
                if let Some(merge) = self
                    .merge_job
                    .as_ref()
                    .filter(|job| job.api_job_id.as_deref() == Some(id.as_str()))
                {
                    (
                        json!({"ok": true, "job": merge.progress_value()}),
                        Task::none(),
                    )
                } else if let Some(download) = self
                    .bag_job
                    .as_ref()
                    .filter(|job| job.api_job_id() == Some(id.as_str()))
                {
                    (
                        json!({"ok": true, "job": download.progress_value()}),
                        Task::none(),
                    )
                } else if let Some(job) = self.api_jobs.get(&id) {
                    (json!({"ok": true, "job": job}), Task::none())
                } else {
                    (
                        json!({"ok": false, "error": "unknown or expired job ID"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::Open { path } => {
                if !path.is_absolute() {
                    (
                        json!({"ok": false, "error": "open requires an absolute path to an existing file, folder or scan project file"}),
                        Task::none(),
                    )
                } else {
                    // The answer lists the accepted files, so it is sent once
                    // the folder or project file has been read off this thread.
                    let reply = request.reply;
                    return Task::perform(
                        Self::expand_paths(vec![path.clone()], self.open_scan_paths()),
                        move |expansion| {
                            Message::ApiScansExpanded(reply.clone(), path.clone(), expansion)
                        },
                    );
                }
            }
            ApiCommand::CancelImport { id } => {
                if !self.imports.contains_key(&id) {
                    (
                        json!({"ok": false, "error": "unknown active import ID"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::CancelImport(id));
                    (json!({"ok": true, "cancelling": true, "id": id}), task)
                }
            }
            ApiCommand::Remove { index } => {
                if index >= self.clouds.len() {
                    (
                        json!({"ok": false, "error": "cloud index is out of range"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::Remove(index));
                    (json!({"ok": true, "removed": index}), task)
                }
            }
            ApiCommand::SetActive { index } => {
                if index >= self.clouds.len() {
                    (
                        json!({"ok": false, "error": "cloud index is out of range"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::Select(index));
                    (json!({"ok": true, "active": index}), task)
                }
            }
            ApiCommand::SetVisible { index, visible } => {
                if index >= self.clouds.len() {
                    (
                        json!({"ok": false, "error": "cloud index is out of range"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::SetVisible(index, visible));
                    (
                        json!({"ok": true, "index": index, "visible": visible}),
                        task,
                    )
                }
            }
            ApiCommand::Camera { preset } => {
                let preset = match preset.to_ascii_lowercase().as_str() {
                    "top" => Some(CameraPreset::Top),
                    "bottom" => Some(CameraPreset::Bottom),
                    "front" => Some(CameraPreset::Front),
                    "back" => Some(CameraPreset::Back),
                    "right" => Some(CameraPreset::Right),
                    "left" => Some(CameraPreset::Left),
                    "isometric" | "iso" => Some(CameraPreset::Isometric),
                    _ => None,
                };
                if let Some(preset) = preset {
                    let task = self.update(Message::CameraPreset(preset));
                    (json!({"ok": true, "view": self.view_label}), task)
                } else {
                    (
                        json!({"ok": false, "error": "unknown camera preset"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::SetCamera {
                yaw,
                pitch,
                zoom,
                pan,
                orbit_point,
            } => {
                if !(-std::f32::consts::PI..=std::f32::consts::PI).contains(&yaw)
                    || !(-1.56..=1.56).contains(&pitch)
                    || !(0.000_001..=10_000.0).contains(&zoom)
                    || !pan.iter().all(|value| value.is_finite())
                    || !orbit_point
                        .flatten()
                        .is_none_or(|point| point.iter().all(|value| value.is_finite()))
                {
                    (
                        json!({"ok": false, "error": "camera requires finite yaw within ±π, pitch within ±1.56, zoom from 0.000001 to 10000, finite pan and a finite orbit_point"}),
                        Task::none(),
                    )
                } else {
                    self.yaw = yaw;
                    self.pitch = pitch;
                    self.zoom = zoom;
                    self.pan = pan;
                    if let Some(point) = orbit_point {
                        self.orbit_point = point;
                    }
                    self.view_label = i18n::key("CUSTOM");
                    self.revision += 1;
                    let task = self.schedule_detail();
                    (json!({"ok": true, "camera": self.camera_value()}), task)
                }
            }
            ApiCommand::OpenPanorama { index, station } => {
                let has_photos = self.clouds.get(index).is_some_and(|entry| {
                    station < entry.cloud.scan_poses.len()
                        && entry
                            .cloud
                            .scan_images
                            .iter()
                            .any(|image| image.station == Some(station))
                });
                if has_photos {
                    let task = self.update(Message::EnterPanorama(index, station));
                    (json!({"ok": true, "walk": self.walk_value()}), task)
                } else {
                    (
                        json!({"ok": false, "error": "that cloud and station have no station photos"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::SetPanorama {
                yaw,
                pitch,
                field_of_view,
            } => {
                let valid = (-std::f32::consts::PI..=std::f32::consts::PI).contains(&yaw)
                    && (-1.55..=1.55).contains(&pitch)
                    && (station_photos::MIN_FIELD_OF_VIEW..=station_photos::MAX_FIELD_OF_VIEW)
                        .contains(&field_of_view);
                match (&mut self.walk, valid) {
                    (Some(view), true) => {
                        view.yaw = yaw;
                        view.pitch = pitch;
                        view.field_of_view = field_of_view;
                        (json!({"ok": true, "walk": self.walk_value()}), Task::none())
                    }
                    (None, _) => (
                        json!({"ok": false, "error": "the walking camera is not active"}),
                        Task::none(),
                    ),
                    (Some(_), false) => (
                        json!({"ok": false, "error": "set_panorama requires yaw within ±π, pitch within ±1.55 and field_of_view from 0.35 to 2.1 radians"}),
                        Task::none(),
                    ),
                }
            }
            ApiCommand::Walk { eye, yaw, pitch } => {
                if !eye.iter().all(|value| value.is_finite())
                    || !(-std::f32::consts::PI..=std::f32::consts::PI).contains(&yaw)
                    || !(-1.55..=1.55).contains(&pitch)
                    || combined_bounds(&self.clouds).is_none()
                {
                    (
                        json!({"ok": false, "error": "walk requires an open scene, a finite eye position, yaw within ±π and pitch within ±1.55"}),
                        Task::none(),
                    )
                } else {
                    let mut view = self.walk.unwrap_or_else(|| WalkView::new(eye, yaw));
                    view.eye = eye;
                    view.yaw = yaw;
                    view.pitch = pitch;
                    self.walk = Some(view);
                    self.revision += 1;
                    let station = self.sync_walk_station();
                    let detail = self.schedule_detail();
                    (
                        json!({"ok": true, "walk": self.walk_value()}),
                        Task::batch([station, detail]),
                    )
                }
            }
            ApiCommand::ClosePanorama => {
                let was_open = self.walk.is_some();
                let task = self.update(Message::LeaveWalk);
                (json!({"ok": true, "closed": was_open}), task)
            }
            ApiCommand::ZoomAll => {
                let task = self.update(Message::ResetCamera);
                (json!({"ok": true, "camera": self.camera_value()}), task)
            }
            command @ (ApiCommand::ListCameraViews
            | ApiCommand::SaveCameraView { .. }
            | ApiCommand::UpdateCameraView { .. }
            | ApiCommand::RenameCameraView { .. }
            | ApiCommand::RestoreCameraView { .. }
            | ApiCommand::DeleteCameraView { .. }
            | ApiCommand::AddNote { .. }
            | ApiCommand::AddLine { .. }
            | ApiCommand::DeleteAnnotation { .. }
            | ApiCommand::SetAnnotationTool { .. }
            | ApiCommand::AnnotateScreen { .. }
            | ApiCommand::SubmitNote { .. }
            | ApiCommand::ExportBcf { .. }) => self.api_views(command),
            ApiCommand::SetTheme { theme } => {
                if let Some(theme) = UiTheme::from_key(&theme.to_ascii_lowercase()) {
                    let task = self.update(Message::Theme(theme));
                    (json!({"ok": true, "theme": theme.key()}), task)
                } else {
                    (json!({"ok": false, "error": "unknown theme"}), Task::none())
                }
            }
            ApiCommand::SetLanguage { language } => {
                if let Some(language) = i18n::Language::from_key(&language.to_ascii_lowercase()) {
                    self.choose_language(language);
                    (
                        json!({"ok": true, "language": language.key()}),
                        Task::none(),
                    )
                } else {
                    (
                        json!({"ok": false, "error": "unknown language"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::SetColor { mode } => {
                let mode = match mode.to_ascii_lowercase().as_str() {
                    "rgb" => Some(ColorMode::Rgb),
                    "elevation" => Some(ColorMode::Elevation),
                    "intensity" => Some(ColorMode::Intensity),
                    "classification" => Some(ColorMode::Classification),
                    _ => None,
                };
                if let Some(mode) = mode {
                    let task = self.update(Message::ColorMode(mode));
                    (json!({"ok": true}), task)
                } else {
                    (
                        json!({"ok": false, "error": "unknown color mode"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::SetClassVisible { code, visible } => {
                let task = self.update(Message::FilterClass(code, visible));
                (json!({"ok": true, "code": code, "visible": visible}), task)
            }
            ApiCommand::SetPointSize { size } => {
                if !size.is_finite() || !(0.1..=20.0).contains(&size) {
                    (
                        json!({"ok": false, "error": "point size must be between 0.1 and 20"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::PointSize(size));
                    (json!({"ok": true, "point_size": size}), task)
                }
            }
            ApiCommand::SetEyeDome { enabled } => {
                let task = self.update(Message::SetEyeDome(enabled));
                (json!({"ok": true, "eye_dome": enabled}), task)
            }
            ApiCommand::SetEyeDomeStrength { strength } => {
                if !strength.is_finite() || !(0.0..=5.0).contains(&strength) {
                    (
                        json!({"ok": false, "error": "eye-dome strength must be between 0 and 5"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::EyeDomeStrength(strength));
                    (json!({"ok": true, "eye_dome_strength": strength}), task)
                }
            }
            ApiCommand::SetBudget { points } => {
                if !(MIN_POINT_BUDGET..=MAX_POINT_BUDGET).contains(&points) {
                    (
                        json!({"ok": false, "error": "point budget must be between 1000 and 10000000"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::Budget(points));
                    (json!({"ok": true, "budget": points}), task)
                }
            }
            ApiCommand::SetSection { min, max, rotation } => {
                let turned = OrientedBox::new(Bounds { min, max }, rotation.unwrap_or(0.0));
                if turned.is_turned() || !turned.rotation_degrees.is_finite() {
                    let overall = combined_bounds(&self.clouds);
                    let reaches = overall.is_some_and(|overall| {
                        let around = turned.aabb();
                        (0..3).all(|axis| {
                            around.min[axis] <= overall.max[axis]
                                && around.max[axis] >= overall.min[axis]
                        })
                    });
                    if overall.is_none() {
                        (
                            json!({"ok": false, "error": "open a cloud before setting a section"}),
                            Task::none(),
                        )
                    } else if !turned.is_valid()
                        || (0..2).any(|axis| turned.bounds.min[axis] >= turned.bounds.max[axis])
                        || !reaches
                        || !self.place_section(turned)
                    {
                        (
                            json!({"ok": false, "error": "a turned section box must be finite, ordered, have a width and length, and reach the model"}),
                            Task::none(),
                        )
                    } else {
                        self.section_enabled = true;
                        self.sync_section_coordinate_inputs();
                        self.revision += 1;
                        self.status = "Section box updated through native API".into();
                        let task = self.schedule_detail();
                        (json!({"ok": true, "section": self.section_value()}), task)
                    }
                } else if let Some(overall) = combined_bounds(&self.clouds) {
                    if let Some(section) = section_within_model(Bounds { min, max }, overall) {
                        self.set_section_rotation_value(0.0);
                        self.section_reference_bounds = Some(overall);
                        for axis in 0..3 {
                            let span = overall.max[axis] - overall.min[axis];
                            if span > 0.0 {
                                self.section_min_percent[axis] =
                                    (section.min[axis] - overall.min[axis]) / span * 100.0;
                                self.section_max_percent[axis] =
                                    (section.max[axis] - overall.min[axis]) / span * 100.0;
                            }
                        }
                        self.section_enabled = true;
                        self.sync_section_coordinate_inputs();
                        self.revision += 1;
                        self.status = "Section box updated through native API".into();
                        let task = self.schedule_detail();
                        (
                            json!({"ok": true, "section": {"min": section.min, "max": section.max, "rotation": 0.0}}),
                            task,
                        )
                    } else {
                        (
                            json!({"ok": false, "error": "section bounds must be finite, ordered and inside the model"}),
                            Task::none(),
                        )
                    }
                } else {
                    (
                        json!({"ok": false, "error": "open a cloud before setting a section"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::ClearSection => {
                let task = self.update(Message::SetSectionEnabled(false));
                (json!({"ok": true}), task)
            }
            ApiCommand::AlignSectionToWalls => {
                if self.section_box().is_none() {
                    (
                        json!({"ok": false, "error": "section box is not enabled"}),
                        Task::none(),
                    )
                } else if self.section_align_pending {
                    (
                        json!({"ok": false, "error": "the walls are already being looked for"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::AlignSectionToWalls);
                    (json!({"ok": true, "started": true}), task)
                }
            }
            ApiCommand::SelectWorld { min, max } => {
                if !(0..3).all(|axis| {
                    min[axis].is_finite() && max[axis].is_finite() && min[axis] <= max[axis]
                }) {
                    (
                        json!({"ok": false, "error": "selection bounds must be finite and ordered"}),
                        Task::none(),
                    )
                } else if self.selection_pending {
                    (
                        json!({"ok": false, "error": "a full-resolution selection is already running"}),
                        Task::none(),
                    )
                } else {
                    let sources: Vec<_> = self
                        .clouds
                        .iter()
                        .enumerate()
                        .filter(|(_, entry)| entry.visible)
                        .map(|(index, entry)| SelectionSource {
                            index,
                            cloud: Arc::clone(&entry.cloud),
                            tree: entry.index.as_ref().map(Arc::clone),
                            deleted: entry.deleted.as_ref().map(Arc::clone),
                            transform: entry.transform,
                        })
                        .collect();
                    if sources.is_empty() {
                        (
                            json!({"ok": false, "error": "no visible point cloud to select"}),
                            Task::none(),
                        )
                    } else {
                        let id = self.record_api_job(
                            json!({"state": "running", "operation": "select_world"}),
                        );
                        let completion_id = id.clone();
                        let revision = self.revision;
                        let bounds = Bounds { min, max };
                        let filter = ClassFilter {
                            ground: self.filter_ground,
                            vegetation: self.filter_vegetation,
                            buildings: self.filter_buildings,
                            other: self.filter_other,
                            classes: self.class_visibility,
                            section: self.section_box(),
                        };
                        self.selection_pending = true;
                        let cancel = Arc::new(AtomicBool::new(false));
                        self.selection_cancel = Arc::clone(&cancel);
                        self.pending_delete = false;
                        self.status = format!(
                            "Selecting exact points in {} visible file(s)…",
                            sources.len()
                        );
                        let task = Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    select_world_cancellable(sources, bounds, filter, cancel)
                                })
                                .await
                                .map_err(|error| error.to_string())?
                            },
                            move |result| {
                                Message::ApiWorldSelectionReady(
                                    completion_id.clone(),
                                    revision,
                                    result,
                                )
                            },
                        );
                        (json!({"ok": true, "accepted": true, "job_id": id}), task)
                    }
                }
            }
            ApiCommand::PickOrbitPoint { pointer } => {
                if self.walk.is_some() {
                    (
                        json!({"ok": false, "error": "the walking camera is active"}),
                        Task::none(),
                    )
                } else if !pointer.into_iter().all(f32::is_finite)
                    || pointer[0] < 0.0
                    || pointer[1] < 0.0
                    || pointer[0] > self.scene_size().width
                    || pointer[1] > self.scene_size().height
                {
                    (
                        json!({"ok": false, "error": "pointer must lie inside the viewport"}),
                        Task::none(),
                    )
                } else {
                    let point = self.orbit_point_at(pointer, self.scene_size())();
                    self.set_orbit_point(point);
                    (
                        json!({"ok": true, "orbit_point": self.orbit_point}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::Orbit { yaw, pitch } => {
                if self.walk.is_some() {
                    (
                        json!({"ok": false, "error": "the walking camera is active"}),
                        Task::none(),
                    )
                } else if !(-std::f32::consts::TAU..=std::f32::consts::TAU).contains(&yaw)
                    || !(-std::f32::consts::PI..=std::f32::consts::PI).contains(&pitch)
                {
                    (
                        json!({"ok": false, "error": "orbit requires yaw within ±2π and pitch within ±π"}),
                        Task::none(),
                    )
                } else {
                    self.turn_orbit(yaw, pitch);
                    let task = self.schedule_detail();
                    (json!({"ok": true, "camera": self.camera_value()}), task)
                }
            }
            ApiCommand::PickScreen { pointer, radius } => {
                let radius = radius.unwrap_or(8.0);
                if !pointer.into_iter().all(f32::is_finite)
                    || !radius.is_finite()
                    || !(1.0..=64.0).contains(&radius)
                    || pointer[0] < 0.0
                    || pointer[1] < 0.0
                    || pointer[0] > self.viewport_size.width
                    || pointer[1] > self.viewport_size.height
                {
                    (
                        json!({"ok": false, "error": "pointer must lie inside the viewport and radius must be 1–64 pixels"}),
                        Task::none(),
                    )
                } else {
                    let id = self
                        .record_api_job(json!({"state": "running", "operation": "pick_screen"}));
                    match self.start_point_pick(
                        pointer,
                        radius,
                        self.viewport_size,
                        Some(id.clone()),
                    ) {
                        Ok(task) => (json!({"ok": true, "accepted": true, "job_id": id}), task),
                        Err(error) => {
                            self.api_jobs.remove(&id);
                            self.api_job_order.retain(|job_id| job_id != &id);
                            (json!({"ok": false, "error": error}), Task::none())
                        }
                    }
                }
            }
            ApiCommand::CancelSelection => {
                if !self.selection_pending {
                    (
                        json!({"ok": false, "error": "no selection is running"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::CancelSelection);
                    (json!({"ok": true, "cancelling": true}), task)
                }
            }
            ApiCommand::ClearSelection => {
                let task = self.update(Message::ClearSelection);
                (json!({"ok": true, "selected_points": 0}), task)
            }
            ApiCommand::Measure { mode, points } => (self.api_measure(&mode, points), Task::none()),
            ApiCommand::ClearMeasure => {
                let task = self.update(Message::Measure(measure::MeasureAction::Clear));
                (json!({"ok": true, "measure": null}), task)
            }
            ApiCommand::ZoomSelection => {
                if self.selected_total() == 0 || self.selection_bounds_pending {
                    (
                        json!({"ok": false, "error": "zoom selection needs selected points and no running bounds task"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::ZoomToSelection);
                    (json!({"ok": true, "accepted": true}), task)
                }
            }
            ApiCommand::DeleteSelection => {
                if self.selection_pending || self.selected_total() == 0 {
                    (
                        json!({"ok": false, "error": "wait for selection to finish or select points first"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::DeleteSelection);
                    (
                        json!({"ok": true, "accepted": true, "status": self.status}),
                        task,
                    )
                }
            }
            ApiCommand::UndoDelete => {
                if self.undo_deletions.is_empty() {
                    (
                        json!({"ok": false, "error": "nothing to undo"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::UndoDelete);
                    (json!({"ok": true, "status": self.status}), task)
                }
            }
            ApiCommand::RedoDelete => {
                if self.redo_deletions.is_empty() {
                    (
                        json!({"ok": false, "error": "nothing to redo"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::RedoDelete);
                    (json!({"ok": true, "status": self.status}), task)
                }
            }
            ApiCommand::Thin { percent } => {
                if !(1..=100).contains(&percent) {
                    (
                        json!({"ok": false, "error": "thin percentage must be between 1 and 100"}),
                        Task::none(),
                    )
                } else if self.active.is_none() {
                    (
                        json!({"ok": false, "error": "no active point cloud to thin"}),
                        Task::none(),
                    )
                } else if self.thin_pending {
                    (
                        json!({"ok": false, "error": "thinning is already in progress"}),
                        Task::none(),
                    )
                } else {
                    self.thin_percent = percent;
                    let task = self.update(Message::Thin);
                    (
                        json!({"ok": true, "accepted": true, "percent": percent}),
                        task,
                    )
                }
            }
            ApiCommand::Translate { offset } => {
                if self.active.is_none() || !offset.iter().all(|value| value.is_finite()) {
                    (
                        json!({"ok": false, "error": "translate needs an active cloud and finite XYZ offsets"}),
                        Task::none(),
                    )
                } else {
                    [self.translate_x, self.translate_y, self.translate_z] =
                        offset.map(|value| value.to_string());
                    let task = self.update(Message::ApplyTranslation);
                    (
                        json!({"ok": self.status.starts_with("Moved "), "status": self.status}),
                        task,
                    )
                }
            }
            ApiCommand::Scale { factors } => {
                if self.active.is_none()
                    || self.scale_job.is_some()
                    || !factors.iter().all(|value| value.is_finite())
                {
                    (
                        json!({"ok": false, "error": "scale needs an active cloud, finite XYZ factors and no running scale"}),
                        Task::none(),
                    )
                } else {
                    self.scale_inputs = factors.map(|value| value.to_string());
                    let task = self.update(Message::ApplyScale);
                    let accepted = self.scale_job.is_some() || self.status.starts_with("Scaled ");
                    (
                        json!({"ok": accepted, "accepted": accepted, "running": self.scale_job.is_some(), "status": self.status}),
                        task,
                    )
                }
            }
            ApiCommand::CancelScale => {
                if self.scale_job.is_none() {
                    (
                        json!({"ok": false, "error": "no scale task is running"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::CancelScale);
                    (json!({"ok": true, "status": self.status}), task)
                }
            }
            ApiCommand::BuildIndex => {
                if self.index_pending {
                    (
                        json!({"ok": false, "error": "an octree build is already running"}),
                        Task::none(),
                    )
                } else if self
                    .active
                    .and_then(|index| self.clouds.get(index))
                    .is_none_or(|entry| entry.index.is_some())
                {
                    (
                        json!({"ok": false, "error": "choose an unindexed active cloud"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::BuildIndex);
                    (
                        json!({"ok": self.index_pending, "accepted": self.index_pending, "status": self.status}),
                        task,
                    )
                }
            }
            ApiCommand::CancelIndex => {
                if !self.index_pending {
                    (
                        json!({"ok": false, "error": "no octree build is running"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::CancelIndex);
                    (json!({"ok": true, "status": self.status}), task)
                }
            }
            ApiCommand::SetAutoIndex { enabled } => {
                let task = self.update(Message::SetAutoIndex(enabled));
                (json!({"ok": true, "auto_index": self.auto_index}), task)
            }
            ApiCommand::SetSurfaceSettings {
                max_vertices,
                neighbors,
                edge_factor,
            } => {
                let config = SurfaceMeshConfig {
                    max_vertices,
                    neighbors,
                    max_edge_factor: edge_factor,
                };
                match config.validate() {
                    Ok(()) => {
                        self.set_surface_mesh_config(config);
                        (
                            json!({"ok": true, "surface_settings": {
                                "max_vertices": max_vertices,
                                "neighbors": neighbors,
                                "edge_factor": edge_factor,
                            }}),
                            Task::none(),
                        )
                    }
                    Err(error) => (
                        json!({"ok": false, "error": error.to_string()}),
                        Task::none(),
                    ),
                }
            }
            ApiCommand::ResetTransform => {
                if self.active.is_none() {
                    (
                        json!({"ok": false, "error": "no active cloud"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::ResetTransform);
                    (json!({"ok": true, "status": self.status}), task)
                }
            }
            ApiCommand::Mesh {
                mode,
                path,
                options,
            } if mode.eq_ignore_ascii_case("closed") => self.api_closed_mesh(path, &options),
            ApiCommand::Mesh { options, .. }
                if options != closed_mesh::ClosedMeshOptions::default() =>
            {
                (
                    json!({"ok": false, "error": "voxel, max_hole, simplify_mm, sides and layers go with the mesh mode closed only"}),
                    Task::none(),
                )
            }
            ApiCommand::Mesh { mode, path, .. } => {
                // Only a closed mesh can do without a file.
                let path = path.unwrap_or_default();
                let mode = match mode.to_ascii_lowercase().as_str() {
                    "terrain" => Some(MeshMode::Terrain),
                    "surface" | "3d" => Some(MeshMode::Surface),
                    _ => None,
                };
                let config = if matches!(mode, Some(MeshMode::Surface)) {
                    self.surface_mesh_config()
                } else {
                    Ok(SurfaceMeshConfig::default())
                };
                if self.mesh_dialog_pending
                    || self.mesh_job.is_some()
                    || self.closed_mesh.is_running()
                {
                    (
                        json!({"ok": false, "error": "a mesh task is already open or running"}),
                        Task::none(),
                    )
                } else if !path.is_absolute()
                    || !path
                        .extension()
                        .is_some_and(|value| value.eq_ignore_ascii_case("obj"))
                {
                    (
                        json!({"ok": false, "error": "mesh requires an absolute .obj destination"}),
                        Task::none(),
                    )
                } else if let Err(error) = &config {
                    (json!({"ok": false, "error": error}), Task::none())
                } else if let Some(mode) = mode {
                    let config = config.expect("validated surface settings");
                    if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                        let cloud = Arc::clone(&entry.cloud);
                        let deleted = entry.deleted.as_ref().map(Arc::clone);
                        let transform = entry.transform;
                        let id = self.record_api_job(json!({
                            "state": "running", "operation": "mesh", "mode": mode.label(), "path": path
                        }));
                        let task = self.start_mesh_job(MeshStart {
                            mode,
                            surface_config: config,
                            cloud,
                            deleted,
                            filter: self.mesh_filter(),
                            transform,
                            path: path.clone(),
                            api_job_id: Some(id.clone()),
                        });
                        (
                            json!({"ok": true, "accepted": true, "job_id": id, "path": path}),
                            task,
                        )
                    } else {
                        (
                            json!({"ok": false, "error": "no active cloud"}),
                            Task::none(),
                        )
                    }
                } else {
                    (
                        json!({"ok": false, "error": "mesh mode must be terrain, surface or closed"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::SetClosedMeshSettings { options } => {
                (self.api_set_closed_mesh_settings(&options), Task::none())
            }
            ApiCommand::CancelMesh => {
                if let Some(answer) = self.api_cancel_closed_mesh() {
                    (answer, Task::none())
                } else if self.mesh_job.is_none() {
                    (
                        json!({"ok": false, "error": "no mesh task is running"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::CancelMesh);
                    (json!({"ok": true, "cancel_requested": true}), task)
                }
            }
            ApiCommand::ExportMesh { path } => self.api_export_mesh(path),
            ApiCommand::SetFaceSettings { options } => {
                (self.api_set_face_settings(&options), Task::none())
            }
            ApiCommand::DetectFaces { options } => self.api_detect_faces(&options),
            ApiCommand::CancelDetectFaces => (self.api_cancel_detect_faces(), Task::none()),
            ApiCommand::ListFaces { boundaries } => (self.api_list_faces(boundaries), Task::none()),
            ApiCommand::SelectFace { id } => (self.api_select_face(id), Task::none()),
            ApiCommand::ExportFaces { path } => self.api_export_faces(path),
            ApiCommand::ClearFaces => (self.api_clear_faces(), Task::none()),
            ApiCommand::Export { path } => self.api_export(path, ApiExportMode::Full),
            ApiCommand::ExportSection { path } => self.api_export(path, ApiExportMode::Section),
            ApiCommand::ExportSelection { path } => self.api_export(path, ApiExportMode::Selected),
            ApiCommand::ExportMinusSelection { path } => {
                self.api_export(path, ApiExportMode::WithoutSelection)
            }
            ApiCommand::ExportDrawing { path, options } => self.api_export_drawing(path, &options),
            ApiCommand::PreviewDrawing { options } => self.api_preview_drawing(&options),
            ApiCommand::ClearDrawingPreview => (self.api_clear_drawing_preview(), Task::none()),
            ApiCommand::CancelDrawing => (self.api_cancel_drawing(), Task::none()),
            ApiCommand::MergeVisible { path } => {
                if !path.is_absolute()
                    || !matches!(
                        export_format_for_path(&path),
                        Some(ExportFormat::Las | ExportFormat::Laz)
                    )
                {
                    (
                        json!({"ok": false, "error": "merge requires an absolute .las or .laz destination"}),
                        Task::none(),
                    )
                } else if self.merge_job.is_some() || self.merge_dialog_pending {
                    (
                        json!({"ok": false, "error": "a cloud merge is already running"}),
                        Task::none(),
                    )
                } else {
                    match self.visible_merge_sources() {
                        Ok(sources) => {
                            let id = self.record_api_job(json!({"state": "running", "operation": "merge_visible", "path": path}));
                            let task =
                                self.start_merge_job(path.clone(), sources, Some(id.clone()));
                            (
                                json!({"ok": true, "accepted": true, "job_id": id, "path": path}),
                                task,
                            )
                        }
                        Err(error) => (json!({"ok": false, "error": error}), Task::none()),
                    }
                }
            }
            ApiCommand::CancelMerge => {
                if self.merge_job.is_none() {
                    (
                        json!({"ok": false, "error": "no cloud merge is running"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::CancelMerge);
                    (json!({"ok": true, "cancel_requested": true}), task)
                }
            }
            ApiCommand::Bag3d { bbox, lod, path } => self.api_bag3d(bbox, &lod, path),
            ApiCommand::CancelBag3d => (self.api_cancel_bag3d(), Task::none()),
            ApiCommand::ListExtensions => (
                json!({"ok": true, "extensions": self.extensions.list()}),
                Task::none(),
            ),
            ApiCommand::SetExtensionEnabled { id, enabled } => {
                (self.api_set_extension_enabled(&id, enabled), Task::none())
            }
            ApiCommand::FileView { open, page } => {
                (self.api_file_view(open, page.as_deref()), Task::none())
            }
            ApiCommand::Screenshot {
                path,
                base64,
                max_edge,
            } => return self.api_screenshot(request.reply, path, base64, max_edge),
        };
        let _ = request.reply.send(response);
        task
    }

    fn record_api_job(&mut self, initial: Value) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.api_jobs.insert(id.clone(), initial);
        self.api_job_order.push_back(id.clone());
        if self.api_job_order.len() > 32 {
            if let Some(oldest) = self.api_job_order.pop_front() {
                self.api_jobs.remove(&oldest);
            }
        }
        id
    }

    fn api_export(&mut self, path: PathBuf, mode: ApiExportMode) -> (Value, Task<Message>) {
        if !path.is_absolute() {
            return (
                json!({"ok": false, "error": "export requires an absolute destination path"}),
                Task::none(),
            );
        }
        let Some(format) = export_format_for_path(&path) else {
            return (
                json!({"ok": false, "error": "unsupported export extension"}),
                Task::none(),
            );
        };
        let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) else {
            return (
                json!({"ok": false, "error": "no active cloud"}),
                Task::none(),
            );
        };
        let cloud = Arc::clone(&entry.cloud);
        let deleted = entry.deleted.as_ref().map(Arc::clone);
        let transform = entry.transform;
        let section = if mode == ApiExportMode::Section {
            let Some(section) = self.section_box() else {
                return (
                    json!({"ok": false, "error": "section box is not enabled"}),
                    Task::none(),
                );
            };
            Some(section)
        } else {
            None
        };
        let selection = if matches!(
            mode,
            ApiExportMode::Selected | ApiExportMode::WithoutSelection
        ) {
            let Some(mask) = entry.selection.as_ref().filter(|mask| mask.count > 0) else {
                return (
                    json!({"ok": false, "error": "select points in the active cloud first"}),
                    Task::none(),
                );
            };
            Some((Arc::clone(mask), entry.remaining_count()))
        } else {
            None
        };
        let job_id = self.record_api_job(json!({"state": "running", "path": path}));
        let response = json!({"ok": true, "accepted": true, "path": path, "job_id": job_id});
        if let Some(section) = section {
            self.section_export_pending = true;
            self.status = "Exporting section through native API…".into();
            let completion_id = job_id;
            let task = Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        export_edited_section(
                            &cloud,
                            &path,
                            format,
                            transform,
                            section,
                            deleted.as_deref(),
                        )
                        .map(|count| (path, count))
                        .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
                },
                move |result| Message::ApiExported(completion_id.clone(), true, result),
            );
            (response, task)
        } else if let Some((mask, remaining)) = selection {
            let selected = mode == ApiExportMode::Selected;
            let expected_count = if selected {
                mask.count
            } else {
                remaining.saturating_sub(mask.count)
            };
            self.status = if selected {
                "Exporting selected points through native API…"
            } else {
                "Exporting points outside selection through native API…"
            }
            .into();
            let completion_id = job_id;
            let task = Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        export_edited_where(
                            &cloud,
                            &path,
                            format,
                            transform,
                            expected_count,
                            |ordinal, _| {
                                if selected {
                                    mask.contains(ordinal)
                                } else {
                                    !mask.contains(ordinal)
                                        && deleted
                                            .as_ref()
                                            .is_none_or(|bits| !bits.contains(ordinal))
                                }
                            },
                        )
                        .map(|()| (path, expected_count))
                        .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
                },
                move |result| Message::ApiExported(completion_id.clone(), false, result),
            );
            (response, task)
        } else {
            self.status = "Exporting cloud through native API…".into();
            let completion_id = job_id;
            let task = Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        let expected_count =
                            cloud.total_points - deleted.as_ref().map_or(0, |mask| mask.count);
                        let result = if deleted.is_none() && transform.is_identity() {
                            pointcloud_core::export_full(&cloud, &path, format)
                        } else {
                            export_edited_where(
                                &cloud,
                                &path,
                                format,
                                transform,
                                expected_count,
                                |ordinal, _| {
                                    deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
                                },
                            )
                        };
                        result
                            .map(|()| (path, expected_count))
                            .map_err(|error| error.to_string())
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result)
                },
                move |result| Message::ApiExported(completion_id.clone(), false, result),
            );
            (response, task)
        }
    }

    fn mesh_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |()| Message::MeshPoll,
        )
    }

    fn merge_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(300)).await },
            |()| Message::MergePoll,
        )
    }

    fn visible_merge_sources(&self) -> Result<Vec<MergeSource>, String> {
        let sources: Vec<_> = self
            .clouds
            .iter()
            .filter(|entry| entry.visible)
            .map(|entry| MergeSource {
                cloud: Arc::clone(&entry.cloud),
                deleted: entry.deleted.as_ref().map(Arc::clone),
                transform: entry.transform,
            })
            .collect();
        if sources.len() < 2 {
            return Err("show at least two LAS/LAZ scans before merging".into());
        }
        if sources.iter().any(|source| {
            !source
                .cloud
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
                })
        }) {
            return Err("all visible layers must be LAS or LAZ scans".into());
        }
        Ok(sources)
    }

    /// Ask every job on a worker thread to stop. Leaving the application
    /// waits for those threads, so without this the process would outlive
    /// its window until a download, a merge, a mesh or an import has ended.
    /// Each of them leaves an existing destination as it was.
    fn stop_background_work(&mut self) {
        self.cancel_bag();
        self.cancel_drawing();
        self.cancel_closed_mesh();
        self.cancel_faces();
        if let Some(job) = &self.merge_job {
            job.control.cancelled.store(true, Ordering::Relaxed);
        }
        if let Some(job) = &self.mesh_job {
            job.control.cancelled.store(true, Ordering::Relaxed);
        }
        for job in self.imports.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn start_merge_job(
        &mut self,
        path: PathBuf,
        sources: Vec<MergeSource>,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        let total = sources.iter().map(|source| source.cloud.total_points).sum();
        let expected = sources
            .iter()
            .map(|source| {
                source.cloud.total_points - source.deleted.as_ref().map_or(0, |mask| mask.count)
            })
            .sum();
        let control = Arc::new(MergeControl {
            cancelled: AtomicBool::new(false),
            processed: AtomicU64::new(0),
            written: AtomicU64::new(0),
            total,
        });
        self.merge_job = Some(MergeJob {
            path: path.clone(),
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id,
        });
        self.status = format!("Merging {} visible scans…", sources.len());
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let clouds: Vec<_> =
                        sources.iter().map(|source| source.cloud.as_ref()).collect();
                    let format =
                        export_format_for_path(&path).expect("validated LAS/LAZ destination");
                    pointcloud_core::merge_las_map_count(
                        &clouds,
                        &path,
                        format,
                        Some(expected),
                        &mut |source_index, ordinal, point| {
                            let source = &sources[source_index];
                            source
                                .deleted
                                .as_ref()
                                .is_none_or(|mask| !mask.contains(ordinal))
                                .then(|| source.transform.point(point))
                        },
                        &mut |processed, _, written| control.report(processed, written),
                    )
                    .map(|count| (path, count))
                    .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            Message::MergeReady,
        );
        Task::batch([worker, Self::merge_poll_task()])
    }

    fn scale_poll_task(id: u64) -> Task<Message> {
        Task::perform(
            async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
                id
            },
            Message::ScalePoll,
        )
    }

    fn index_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |()| Message::IndexPoll,
        )
    }

    fn index_progress_text(progress: IndexProgress) -> String {
        match progress.stage {
            IndexStage::ReadingSource if progress.total == 0 => format!(
                "Reading source and preparing octree: {} points…",
                format_count(progress.completed)
            ),
            IndexStage::ReadingSource => format!(
                "Reading source for octree: {} / {} points ({:.0}%)",
                progress.completed,
                progress.total,
                if progress.total == 0 {
                    0.0
                } else {
                    progress.completed as f64 / progress.total as f64 * 100.0
                }
            ),
            IndexStage::BuildingTree => format!(
                "Building octree: {} point records, {} leaves (depth {})",
                progress.completed, progress.leaves, progress.depth
            ),
            IndexStage::Ready => format!(
                "Octree ready: {} source points, {} leaves",
                progress.completed, progress.leaves
            ),
        }
    }

    fn indexing_during_import(&self) -> bool {
        self.imports
            .values()
            .any(|job| Arc::ptr_eq(&job.cancel, &self.index_cancel))
    }

    fn start_index_job(&mut self, source: Arc<PointCloud>, automatic: bool) -> Task<Message> {
        let progress = Arc::new(Mutex::new(IndexProgress {
            stage: IndexStage::ReadingSource,
            completed: 0,
            total: source.total_points,
            depth: 0,
            leaves: 0,
            settled: 0,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        self.index_pending = true;
        self.index_progress = Some(Arc::clone(&progress));
        self.index_cancel = Arc::clone(&cancel);
        let message_source = Arc::clone(&source);
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    OctreeIndex::build_cached_with_progress(
                        &source,
                        IndexConfig::default(),
                        |update| {
                            if cancel.load(Ordering::Relaxed) {
                                return Err(pointcloud_core::LoadError::Cancelled);
                            }
                            if let Ok(mut current) = progress.lock() {
                                *current = update;
                            }
                            Ok(())
                        },
                    )
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| {
                if automatic {
                    Message::AutoIndexReady(Arc::clone(&message_source), result)
                } else {
                    Message::IndexReady(Arc::clone(&message_source), result)
                }
            },
        );
        Task::batch([worker, Self::index_poll_task()])
    }

    fn apply_scale_from_source_centroid(
        &mut self,
        cloud_index: usize,
        factors: [f64; 3],
        source_centroid: [f64; 3],
    ) -> Task<Message> {
        let old_scene = combined_bounds(&self.clouds);
        let Some(entry) = self.clouds.get_mut(cloud_index) else {
            self.status = "Scale cancelled: cloud is no longer open".into();
            return Task::none();
        };
        let pivot = entry.transform.xyz(source_centroid);
        let Some(next) = entry
            .transform
            .scaled_about(factors, pivot, entry.cloud.bounds)
        else {
            self.status = "Scale would produce non-finite coordinates".into();
            return Task::none();
        };
        entry.transform = next;
        if !self.section_enabled {
            self.reset_section_reference();
            self.sync_section_coordinate_inputs();
        }
        self.preserve_camera_for_scene_change(old_scene);
        self.revision += 1;
        self.status = format!(
            "Scaled the open cloud around the exact point centroid by X {}, Y {}, Z {}; export to save",
            factors[0], factors[1], factors[2]
        );
        self.schedule_detail()
    }

    fn start_mesh_job(&mut self, request: MeshStart) -> Task<Message> {
        let MeshStart {
            mode,
            surface_config,
            cloud,
            deleted,
            filter,
            transform,
            path,
            api_job_id,
        } = request;
        let remaining = cloud.total_points - deleted.as_ref().map_or(0, |mask| mask.count);
        let control = Arc::new(MeshControl::new(cloud.total_points));
        self.mesh_job = Some(MeshJob {
            mode,
            path: path.clone(),
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id,
        });
        self.status = format!(
            "Meshing visible points from {remaining} remaining source points; progress and Cancel are available below"
        );
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    let source = Arc::clone(&cloud);
                    let result = match mode {
                        MeshMode::Terrain => pointcloud_core::mesh_terrain_obj_where_progress(
                            &cloud,
                            &path,
                            pointcloud_core::MeshConfig::default(),
                            |ordinal, point| {
                                mesh_accepts(ordinal, point, deleted.as_deref(), filter, transform)
                            },
                            |progress| control.report(progress),
                        ),
                        MeshMode::Surface => pointcloud_core::mesh_surface_obj_where_progress(
                            &cloud,
                            &path,
                            surface_config,
                            |ordinal, point| {
                                mesh_accepts(ordinal, point, deleted.as_deref(), filter, transform)
                            },
                            |progress| control.report(progress),
                        ),
                    };
                    result
                        .and_then(|stats| {
                            let mesh = pointcloud_core::read_obj_mesh(&path)?;
                            if !transform.is_identity() {
                                let edited = mesh_export::in_scene(&mesh, transform);
                                pointcloud_core::write_obj_mesh(&edited, &path, &[])?;
                            }
                            let measured = mesh_export::MeasuredMesh::measure(mesh);
                            Ok((source, path, stats, measured))
                        })
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            move |result| Message::MeshReady(mode, result),
        );
        Task::batch([worker, Self::mesh_poll_task()])
    }

    /// Paths of the scans that are open or still being imported.
    fn open_scan_paths(&self) -> Vec<PathBuf> {
        self.clouds
            .iter()
            .map(|entry| entry.cloud.path.clone())
            .chain(self.imports.values().map(|job| job.path.clone()))
            .collect()
    }

    /// Listing a folder or reading a scan project file touches the disk,
    /// often a network share, so it runs on a worker thread.
    async fn expand_paths(paths: Vec<PathBuf>, open: Vec<PathBuf>) -> project_open::Expansion {
        tokio::task::spawn_blocking(move || project_open::expand(&paths, &open))
            .await
            .unwrap_or_else(|error| project_open::Expansion {
                errors: vec![error.to_string()],
                ..project_open::Expansion::default()
            })
    }

    /// Open scan files, folders and scan project files chosen by the user.
    fn open_paths(&mut self, paths: Vec<PathBuf>) -> Task<Message> {
        if paths.is_empty() {
            return Task::none();
        }
        self.status = "Looking for scans…".into();
        Task::perform(
            Self::expand_paths(paths, self.open_scan_paths()),
            Message::ScansExpanded,
        )
    }

    /// Start loading the scans of an expanded selection and report how many
    /// are being opened. Returns what was accepted.
    fn open_expanded(
        &mut self,
        mut expansion: project_open::Expansion,
    ) -> (project_open::Expansion, Task<Message>) {
        // Scans opened while this selection was being expanded are skipped too.
        expansion.skip_open(&self.open_scan_paths());
        let tasks: Vec<_> = expansion
            .files
            .iter()
            .map(|path| self.load(path.clone()))
            .collect();
        // A single file keeps the more detailed status of its own loader.
        if expansion.files.len() != 1 || expansion.has_notes() {
            self.status = expansion.summary();
        }
        (expansion, Task::batch(tasks))
    }

    fn load(&mut self, path: PathBuf) -> Task<Message> {
        self.cancel_selection_for_scene_change();
        let is_las = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("las") || extension.eq_ignore_ascii_case("laz")
            });
        if is_las {
            match pointcloud_core::open_las_header(&path) {
                Ok(header_cloud) => {
                    self.status = format!(
                        "Opened {} points; building preview…",
                        header_cloud.total_points
                    );
                    let header_cloud = Arc::new(header_cloud);
                    self.clouds.push(CloudEntry {
                        cloud: Arc::clone(&header_cloud),
                        load_identity: Arc::clone(&header_cloud),
                        index_import_id: None,
                        transform: CloudTransform::default(),
                        centroid_cache: None,
                        mesh: None,
                        mesh_topology: None,
                        mesh_visible: true,
                        faces: None,
                        bag_source: false,
                        visible: true,
                        selection: None,
                        deleted: None,
                        index: None,
                        auto_index_queued: false,
                        picked: false,
                        index_building: false,
                        detail_points: None,
                    });
                    self.revision += 1;
                    self.active = Some(self.clouds.len() - 1);
                    if self.section_enabled && self.section_reference_bounds.is_none() {
                        self.reset_section_reference();
                        self.sync_section_coordinate_inputs();
                    }
                    let identity = Arc::clone(&header_cloud);
                    let preview_task = Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                // Spaced sampling needs to seek; files whose
                                // compressed chunks vary in size cannot, so
                                // those are sampled from a full pass instead.
                                pointcloud_core::open_las_preview(&path, LOAD_SAMPLE_LIMIT)
                                    .or_else(|_| pointcloud_core::open(&path, LOAD_SAMPLE_LIMIT))
                            })
                            .await
                            .map_err(|error| error.to_string())?
                            .map(Arc::new)
                            .map_err(|error| error.to_string())
                        },
                        move |result| Message::Refined(Arc::clone(&identity), result),
                    );
                    return Task::batch([cached_index_task(header_cloud), preview_task]);
                }
                Err(error) => {
                    self.status = format!("Open failed: {error}");
                    return Task::none();
                }
            }
        }
        let indexable = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "ply" | "e57" | "pcd" | "ptx" | "xyz" | "asc" | "txt" | "csv" | "pts"
                )
            });
        if self.auto_index
            && !self.index_pending
            && indexable
            && std::fs::metadata(&path)
                .is_ok_and(|metadata| metadata.len() >= ONE_PASS_IMPORT_MIN_BYTES)
        {
            return self.load_indexed(path);
        }
        self.status = format!("Loading {}…", path.display());
        self.next_import_id += 1;
        let id = self.next_import_id;
        let header = Self::header_task(id, path.clone());
        let decoded = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        self.opening_total += 1;
        self.imports.insert(
            id,
            ImportJob {
                path: path.clone(),
                decoded: Arc::clone(&decoded),
                cancel: Arc::clone(&cancel),
            },
        );
        let done = Arc::new(AtomicBool::new(false));
        let worker_done = Arc::clone(&done);
        let (snapshot_tx, snapshot_rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = Task::perform(
            async move {
                let result = tokio::task::spawn_blocking(move || {
                    pointcloud_core::open_with_snapshots(
                        path,
                        LOAD_SAMPLE_LIMIT,
                        |processed| {
                            if cancel.load(Ordering::Relaxed) {
                                return Err(pointcloud_core::LoadError::Cancelled);
                            }
                            decoded.store(processed, Ordering::Relaxed);
                            Ok(())
                        },
                        |cloud| {
                            snapshot_tx
                                .send(Arc::new(cloud.clone()))
                                .map_err(|_| pointcloud_core::LoadError::Cancelled)
                        },
                    )
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result.map(Arc::new).map_err(|error| error.to_string()));
                worker_done.store(true, Ordering::Relaxed);
                result
            },
            move |result| Message::ImportLoaded(id, result),
        );
        let progress = iced::futures::stream::unfold(Some(done), move |state| async move {
            let done = state?;
            tokio::time::sleep(Duration::from_millis(500)).await;
            if done.load(Ordering::Relaxed) {
                return None;
            }
            Some((Message::OpenProgress(id), Some(done)))
        });
        let snapshots = iced::futures::stream::unfold(snapshot_rx, |mut receiver| async move {
            receiver.recv().await.map(|cloud| (cloud, receiver))
        });
        Task::batch([
            worker,
            Task::run(progress, |message| message),
            header,
            Task::run(snapshots, move |cloud| Message::ImportSnapshot(id, cloud)),
        ])
    }

    fn load_indexed(&mut self, path: PathBuf) -> Task<Message> {
        self.status = format!("Loading and indexing {}…", path.display());
        self.next_import_id += 1;
        let id = self.next_import_id;
        let header = Self::header_task(id, path.clone());
        let decoded = Arc::new(AtomicU64::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(IndexProgress {
            stage: IndexStage::ReadingSource,
            completed: 0,
            total: 0,
            depth: 0,
            leaves: 0,
            settled: 0,
        }));
        self.imports.insert(
            id,
            ImportJob {
                path: path.clone(),
                decoded: Arc::clone(&decoded),
                cancel: Arc::clone(&cancel),
            },
        );
        self.index_pending = true;
        self.index_cancel = Arc::clone(&cancel);
        self.index_progress = Some(Arc::clone(&progress));
        let (preview_tx, preview_rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    OctreeIndex::open_and_build_cached_with_preview(
                        &path,
                        LOAD_SAMPLE_LIMIT,
                        IndexConfig::default(),
                        |cloud| {
                            if cancel.load(Ordering::Relaxed) {
                                return Err(pointcloud_core::LoadError::Cancelled);
                            }
                            preview_tx
                                .send(Arc::new(cloud.clone()))
                                .map_err(|_| pointcloud_core::LoadError::Cancelled)
                        },
                        |update| {
                            if cancel.load(Ordering::Relaxed) {
                                return Err(pointcloud_core::LoadError::Cancelled);
                            }
                            if update.stage == IndexStage::ReadingSource {
                                decoded.store(update.completed, Ordering::Relaxed);
                            }
                            if let Ok(mut current) = progress.lock() {
                                *current = update;
                            }
                            Ok(())
                        },
                    )
                    .map(|(cloud, index)| (Arc::new(cloud), Arc::new(index)))
                    .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Message::IndexedImportReady(id, result),
        );
        let previews = iced::futures::stream::unfold(preview_rx, |mut receiver| async move {
            receiver.recv().await.map(|cloud| (cloud, receiver))
        });
        Task::batch([
            worker,
            Task::run(previews, move |cloud| {
                Message::IndexedImportPreview(id, cloud)
            }),
            Self::index_poll_task(),
            header,
        ])
    }

    fn start_point_pick(
        &mut self,
        pointer: [f32; 2],
        radius: f32,
        size: Size,
        api_job_id: Option<String>,
    ) -> Result<Task<Message>, String> {
        if self.selection_pending {
            return Err("a full-resolution selection is already running".into());
        }
        let bounds = combined_bounds(&self.clouds)
            .ok_or_else(|| "open a point cloud before picking a point".to_string())?;
        let (index, entry) = self
            .active
            .and_then(|index| self.clouds.get(index).map(|entry| (index, entry)))
            .filter(|(_, entry)| entry.visible)
            .ok_or_else(|| "choose a visible point cloud to pick from".to_string())?;
        let tree = entry.index.as_ref().map(Arc::clone);
        let cloud = Arc::clone(&entry.cloud);
        let deleted = entry.deleted.as_ref().map(Arc::clone);
        let transform = entry.transform;
        let display_views: Vec<_> = self
            .clouds
            .iter()
            .map(|entry| PickView {
                cloud: Arc::clone(&entry.cloud),
                detail: entry.detail_points.as_ref().map(Arc::clone),
                deleted: entry.deleted.as_ref().map(Arc::clone),
                transform: entry.transform,
                visible: entry.visible,
            })
            .collect();
        let display_budget = self.budget as usize;
        let projection = self.projection(bounds, size.width, size.height);
        let sphere_radius = gpu_viewport::display_point_radius(self.point_size, self.zoom);
        let filter = ClassFilter {
            ground: self.filter_ground,
            vegetation: self.filter_vegetation,
            buildings: self.filter_buildings,
            other: self.filter_other,
            classes: self.class_visibility,
            section: self.section_box(),
        };
        let revision = self.revision;
        self.pending_delete = false;
        self.selection_pending = true;
        self.selection_cancel = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&self.selection_cancel);
        self.status = "Finding the visible point…".into();
        Ok(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    cloud.validate_source().map_err(|error| error.to_string())?;
                    let target = PickTarget {
                        pointer,
                        radius,
                        sphere_radius,
                    };
                    let result = if let Some(record) = pick_displayed(
                        &display_views,
                        index,
                        display_budget,
                        projection,
                        target,
                        filter,
                        &cancel,
                    )? {
                        Some(record)
                    } else if let Some(tree) = tree {
                        pick_indexed_transformed(
                            &tree,
                            projection,
                            target,
                            filter,
                            deleted.as_deref(),
                            transform,
                            &cancel,
                        )?
                    } else {
                        pick_full_transformed(
                            &cloud,
                            projection,
                            target,
                            filter,
                            deleted.as_deref(),
                            transform,
                            &cancel,
                        )?
                    };
                    cloud.validate_source().map_err(|error| error.to_string())?;
                    Ok(result)
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| match &api_job_id {
                Some(id) => Message::ApiPickReady(id.clone(), revision, index, result),
                None => Message::PickReady(revision, index, result),
            },
        ))
    }

    /// Give the system title bar the colours of the strip below it.
    fn sync_window_chrome(&self) -> bool {
        let colors = self.ui_theme.colors();
        let bytes = |color: Color| [color.r, color.g, color.b].map(|c| (c * 255.0).round() as u8);
        native_chrome::apply(
            self.ui_theme != UiTheme::Light,
            bytes(colors.tabs),
            bytes(colors.text),
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.track_progress();
        self.settle_views();
        self.settle_drawing();
        self.settle_faces();
        task
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SyncWindowChrome(retries) => {
                if !self.sync_window_chrome() && retries > 0 {
                    return Task::perform(
                        async move {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                            retries - 1
                        },
                        Message::SyncWindowChrome,
                    );
                }
            }
            Message::ApiRequest(request) => return self.handle_api(request),
            Message::ApiScreenshot(step) => return self.screenshot_step(step),
            Message::ApiExported(id, section_only, result) => {
                if section_only {
                    self.section_export_pending = false;
                }
                let job = match result {
                    Ok((path, count)) => {
                        self.status = format!(
                            "Exported {} points to {}",
                            format_count(count),
                            path.display()
                        );
                        json!({"state": "complete", "path": path, "points": count})
                    }
                    Err(error) => {
                        self.status = format!("API export failed: {error}");
                        json!({"state": "failed", "error": error})
                    }
                };
                if let Some(entry) = self.api_jobs.get_mut(&id) {
                    *entry = job;
                }
            }
            Message::MergeVisible => {
                if self.merge_job.is_some() || self.merge_dialog_pending {
                    return Task::none();
                }
                if let Err(error) = self.visible_merge_sources() {
                    self.status = error;
                    return Task::none();
                }
                self.merge_dialog_pending = true;
                self.status = "Choose where to save the merged visible scans…".into();
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("LAZ point cloud", &["laz"])
                            .add_filter("LAS point cloud", &["las"])
                            .set_file_name("merged-scans.laz")
                            .save_file()
                            .await
                            .map(|selection| selection.path().to_path_buf())
                    },
                    Message::MergePathChosen,
                );
            }
            Message::MergePathChosen(path) => {
                self.merge_dialog_pending = false;
                let Some(path) = path else {
                    self.status = "Cloud merge cancelled".into();
                    return Task::none();
                };
                if !matches!(
                    export_format_for_path(&path),
                    Some(ExportFormat::Las | ExportFormat::Laz)
                ) {
                    self.status = "Choose a .las or .laz destination".into();
                    return Task::none();
                }
                match self.visible_merge_sources() {
                    Ok(sources) => return self.start_merge_job(path, sources, None),
                    Err(error) => self.status = error,
                }
            }
            Message::MergePoll => {
                if let Some(job) = &self.merge_job {
                    self.status = job.progress_text();
                    if let Some(id) = &job.api_job_id {
                        if let Some(entry) = self.api_jobs.get_mut(id) {
                            *entry = job.progress_value();
                        }
                    }
                    return Self::merge_poll_task();
                }
            }
            Message::CancelMerge => {
                if let Some(job) = &self.merge_job {
                    job.control.cancelled.store(true, Ordering::Relaxed);
                    self.status = "Cancelling cloud merge…".into();
                }
            }
            Message::MergeReady(result) => {
                if let Some(job) = self.merge_job.take() {
                    if let Some(id) = job.api_job_id {
                        let state = match &result {
                            Ok((path, count)) => {
                                json!({"state": "complete", "path": path, "points": count})
                            }
                            Err(error) if error == "Operation cancelled" => {
                                json!({"state": "cancelled", "path": job.path})
                            }
                            Err(error) => json!({"state": "failed", "error": error}),
                        };
                        if let Some(entry) = self.api_jobs.get_mut(&id) {
                            *entry = state;
                        }
                    }
                }
                self.status = match result {
                    Ok((path, count)) => format!(
                        "Merged {} points into {}",
                        format_count(count),
                        path.display()
                    ),
                    Err(error) if error == "Operation cancelled" => {
                        "Cloud merge cancelled; output left unchanged".into()
                    }
                    Err(error) => format!("Cloud merge failed: {error}"),
                };
            }
            Message::ApiWorldSelectionReady(id, revision, result) => {
                self.selection_pending = false;
                let job = if self.selection_cancel.load(Ordering::Relaxed) {
                    self.status = "Selection cancelled".into();
                    json!({"state": "cancelled"})
                } else if revision != self.revision {
                    let error = "selection discarded because files or view filters changed";
                    self.status = error.into();
                    json!({"state": "failed", "error": error})
                } else {
                    match result {
                        Ok(masks) => {
                            for entry in &mut self.clouds {
                                entry.selection = None;
                            }
                            let mut layers = Vec::with_capacity(masks.len());
                            for (index, mask) in masks {
                                layers.push(json!({"index": index, "points": mask.count}));
                                if let Some(entry) = self.clouds.get_mut(index) {
                                    entry.selection = Some(mask);
                                }
                            }
                            let count = self.selected_total();
                            self.status = self.selection_status();
                            json!({"state": "complete", "points": count, "layers": layers})
                        }
                        Err(error) => {
                            self.status = format!("Selection failed: {error}");
                            json!({"state": "failed", "error": error})
                        }
                    }
                };
                if let Some(entry) = self.api_jobs.get_mut(&id) {
                    *entry = job;
                }
            }
            Message::ApiPickReady(id, revision, index, result) => {
                let job = if self.selection_cancel.load(Ordering::Relaxed) {
                    json!({"state": "cancelled"})
                } else if revision != self.revision {
                    json!({"state": "failed", "error": "point pick discarded because the view changed"})
                } else {
                    match &result {
                        Ok(Some(record)) => json!({
                            "state": "complete",
                            "points": 1,
                            "layer": index,
                            "point": {
                                "ordinal": record.ordinal,
                                "xyz": record.point.xyz,
                                "rgb": record.point.rgb,
                                "intensity": record.point.intensity,
                                "classification": record.point.classification,
                            },
                        }),
                        Ok(None) => json!({"state": "complete", "points": 0, "point": null}),
                        Err(error) => json!({"state": "failed", "error": error}),
                    }
                };
                if let Some(entry) = self.api_jobs.get_mut(&id) {
                    *entry = job;
                }
                return self.update(Message::PickReady(revision, index, result));
            }
            Message::ToggleFile => {
                self.file_open = !self.file_open;
                self.file_page = FilePage::default();
                self.ribbon_viewport = None;
            }
            Message::FileAction(action) => return self.file_action(action),
            Message::FilePage(page) => self.file_page = page,
            Message::OpenUrl(url) => {
                if let Err(error) = open::that(url) {
                    self.status = format!("Could not open {url}: {error}");
                }
            }
            Message::Exit => {
                self.stop_background_work();
                return iced::exit();
            }
            Message::RibbonScroll(direction) => {
                return scrollable::scroll_by(
                    ribbon_scroll_id(),
                    scrollable::AbsoluteOffset {
                        x: direction * 320.0,
                        y: 0.0,
                    },
                );
            }
            Message::RibbonViewport(offset, width, content_width) => {
                self.ribbon_viewport = Some((offset, width, content_width));
            }
            Message::RibbonReset => self.ribbon_viewport = None,
            Message::Theme(theme) => {
                self.ui_theme = theme;
                theme.save();
                let _ = self.sync_window_chrome();
            }
            Message::PersistSettings(revision) => {
                if revision == self.settings_revision {
                    if let Err(error) = preferences::save(&self.preferences()) {
                        self.status = format!("Could not save settings: {error}");
                    }
                }
            }
            Message::Open => {
                return Task::perform(
                    async {
                        let mut extensions = project_open::SCAN_EXTENSIONS.to_vec();
                        extensions.push(project_open::PROJECT_EXTENSION);
                        rfd::AsyncFileDialog::new()
                            .add_filter("Point clouds and scan projects", &extensions)
                            .pick_files()
                            .await
                            .map(|files| {
                                files
                                    .into_iter()
                                    .map(|file| file.path().to_path_buf())
                                    .collect()
                            })
                    },
                    Message::FilesChosen,
                );
            }
            Message::OpenFolder => {
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title("Open scan folder")
                            .pick_folder()
                            .await
                            .map(|folder| vec![folder.path().to_path_buf()])
                    },
                    Message::FilesChosen,
                );
            }
            Message::FilesChosen(Some(paths)) => return self.open_paths(paths),
            Message::FilesChosen(None) => {}
            Message::FileDropped(path) => {
                // Several dropped items arrive one event each; open them as
                // one selection so they are ordered and reported together.
                self.dropped_paths.push(path);
                if self.dropped_paths.len() == 1 {
                    return Task::perform(tokio::time::sleep(Duration::from_millis(100)), |_| {
                        Message::DroppedFilesReady
                    });
                }
            }
            Message::DroppedFilesReady => {
                let paths = std::mem::take(&mut self.dropped_paths);
                // A file handed over while the File view covers the model
                // must be seen opening.
                if self.file_open {
                    self.file_open = false;
                    self.file_page = FilePage::default();
                    self.ribbon_viewport = None;
                }
                return self.open_paths(paths);
            }
            Message::ScansExpanded(expansion) => return self.open_expanded(expansion).1,
            Message::ApiScansExpanded(reply, path, expansion) => {
                let before = self.next_import_id;
                let (expansion, task) = self.open_expanded(expansion);
                let response = if expansion.files.is_empty() {
                    json!({"ok": false, "error": expansion.summary()})
                } else {
                    let import_ids: Vec<u64> = (before + 1..=self.next_import_id).collect();
                    json!({
                        "ok": true,
                        "accepted": true,
                        "path": path,
                        "files": expansion.files,
                        "missing": expansion.missing,
                        "missing_names": expansion.missing_names,
                        "already_open": expansion.already_open,
                        "errors": expansion.errors,
                        "import_id": import_ids.last(),
                        "import_ids": import_ids,
                    })
                };
                let _ = reply.send(response);
                return task;
            }
            Message::OpenProgress(id) => {
                if let Some(job) = self.imports.get(&id) {
                    let label = display_name(&job.path);
                    self.status = if job.cancel.load(Ordering::Relaxed) {
                        format!("Cancelling import of {label}…")
                    } else {
                        format!(
                            "Loading {label}: {} points decoded…",
                            format_count(job.decoded.load(Ordering::Relaxed))
                        )
                    };
                }
            }
            Message::CancelOpening => {
                let indexing = self.index_pending.then(|| Arc::clone(&self.index_cancel));
                for job in self.imports.values() {
                    if !indexing
                        .as_ref()
                        .is_some_and(|cancel| Arc::ptr_eq(cancel, &job.cancel))
                    {
                        job.cancel.store(true, Ordering::Relaxed);
                    }
                }
                self.status = "Cancelling imports…".into();
            }
            Message::Settings(action) => self.settings_action(action),
            Message::CancelImport(id) => {
                if let Some(job) = self.imports.get(&id) {
                    job.cancel.store(true, Ordering::Relaxed);
                    self.status = format!("Cancelling import of {}…", display_name(&job.path));
                }
            }
            Message::ImportLoaded(id, result) => {
                let Some(job) = self.imports.remove(&id) else {
                    return Task::none();
                };
                let header = self.import_headers.remove(&id);
                if job.cancel.load(Ordering::Relaxed) {
                    if let Some(header) = header {
                        self.remove_header_layer(&header);
                    }
                    self.status = format!("Import cancelled: {}", display_name(&job.path));
                } else {
                    return self.finish_import(header, result);
                }
            }
            Message::HeaderLoaded(id, result) => {
                let (Ok(header), Some(job)) = (result, self.imports.get(&id)) else {
                    // The points arrived first, or the file has no usable metadata.
                    return Task::none();
                };
                self.import_expected.insert(id, header.total_points);
                if job.cancel.load(Ordering::Relaxed) || self.import_headers.contains_key(&id) {
                    return Task::none();
                }
                return self.show_import_layer(id, header);
            }
            Message::ImportSnapshot(id, cloud) => {
                // Only while the import is still reading; the checked cloud
                // replaces whatever its layer shows.
                let Some(job) = self.imports.get(&id) else {
                    return Task::none();
                };
                if job.cancel.load(Ordering::Relaxed) {
                    return Task::none();
                }
                if self.selection_pending {
                    // A snapshot changes the scene, which would discard the
                    // selection being computed; the next one is shown.
                    return Task::none();
                }
                let Some(header) = self.import_headers.get(&id).map(Arc::clone) else {
                    return self.show_import_layer(id, cloud);
                };
                let scene = combined_bounds(&self.clouds);
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.matches_source(&header))
                {
                    entry.replace_cloud(cloud);
                    self.revision += 1;
                    self.reframe_after_replacement(scene);
                    return self.schedule_detail();
                }
            }
            Message::IndexedImportPreview(id, cloud) => {
                if !self.imports.contains_key(&id) {
                    // A later look at a scan whose layer is already shown. A
                    // provisional one waits for a selection being computed;
                    // the checked cloud does not.
                    if self.selection_pending && cloud.provisional {
                        return Task::none();
                    }
                    let scene = combined_bounds(&self.clouds);
                    if let Some(entry) = self
                        .clouds
                        .iter_mut()
                        .find(|entry| entry.index_import_id == Some(id) && entry.cloud.provisional)
                    {
                        entry.replace_cloud(cloud);
                        self.revision += 1;
                        self.reframe_after_replacement(scene);
                        return self.schedule_detail();
                    }
                    return Task::none();
                }
                let Some(job) = self.imports.get(&id) else {
                    return Task::none();
                };
                if job.cancel.load(Ordering::Relaxed) {
                    return Task::none();
                }
                self.imports.remove(&id);
                let header = self.import_headers.remove(&id);
                let task = self.finish_import(header.clone(), Ok(Arc::clone(&cloud)));
                let index = header
                    .and_then(|header| {
                        self.clouds
                            .iter()
                            .position(|entry| entry.matches_source(&header))
                    })
                    .or_else(|| self.clouds.len().checked_sub(1));
                if let Some(entry) = index.and_then(|index| self.clouds.get_mut(index)) {
                    entry.index_import_id = Some(id);
                    entry.index_building = true;
                }
                self.status = format!(
                    "Preview ready: {} points; building disk octree…",
                    format_count(cloud.total_points)
                );
                return task;
            }
            Message::IndexedImportReady(id, result) => {
                self.index_pending = false;
                self.index_progress = None;
                let cancelled = self.index_cancel.load(Ordering::Relaxed);
                let import = self.imports.remove(&id);
                let header = self.import_headers.remove(&id);
                let mut loaded = Task::none();
                let mut ready = false;
                if let (Some(header), true) = (&header, cancelled || result.is_err()) {
                    // No points will follow the metadata that was shown.
                    self.remove_header_layer(header);
                }
                match result {
                    Ok((cloud, index)) if !cancelled => {
                        if import.is_some() {
                            loaded = self.finish_import(header.clone(), Ok(Arc::clone(&cloud)));
                            let position = header
                                .and_then(|header| {
                                    self.clouds
                                        .iter()
                                        .position(|entry| entry.matches_source(&header))
                                })
                                .or_else(|| self.clouds.len().checked_sub(1));
                            if let Some(entry) =
                                position.and_then(|position| self.clouds.get_mut(position))
                            {
                                entry.index_import_id = Some(id);
                            }
                        }
                        let scene = combined_bounds(&self.clouds);
                        let mut replaced = false;
                        if let Some(entry) = self
                            .clouds
                            .iter_mut()
                            .find(|entry| entry.index_import_id == Some(id))
                        {
                            entry.index_import_id = None;
                            entry.index_building = false;
                            // A preview sampled before the full pass has
                            // loose bounds: the checked cloud takes its place.
                            if entry.cloud.provisional {
                                entry.replace_cloud(Arc::clone(&cloud));
                                replaced = true;
                            }
                            entry.index = Some(index);
                            self.revision += 1;
                            self.status = format!(
                                "Octree ready: {} points from {}",
                                format_count(cloud.total_points),
                                display_name(&cloud.path)
                            );
                            ready = true;
                        }
                        if replaced {
                            self.reframe_after_replacement(scene);
                        }
                    }
                    Err(error) => {
                        let mut preview_remains = false;
                        let mut unchecked = None;
                        if let Some(entry) = self
                            .clouds
                            .iter_mut()
                            .find(|entry| entry.index_import_id == Some(id))
                        {
                            entry.index_import_id = None;
                            entry.index_building = false;
                            if entry.cloud.provisional {
                                unchecked = Some(Arc::clone(&entry.cloud));
                            } else {
                                preview_remains = true;
                            }
                        }
                        if let Some(preview) = unchecked {
                            // Its points were never checked against the
                            // source, so the layer cannot stay.
                            self.remove_header_layer(&preview);
                        }
                        self.status = if cancelled {
                            if import.is_some() {
                                "Import cancelled".into()
                            } else if preview_remains {
                                "Octree build cancelled; preview remains open".into()
                            } else {
                                "Octree build cancelled".into()
                            }
                        } else {
                            format!("Import or octree failed: {error}")
                        };
                    }
                    Ok((cloud, _)) => {
                        let scene = combined_bounds(&self.clouds);
                        let mut preview_remains = false;
                        let mut replaced = false;
                        if let Some(entry) = self
                            .clouds
                            .iter_mut()
                            .find(|entry| entry.index_import_id == Some(id))
                        {
                            entry.index_import_id = None;
                            entry.index_building = false;
                            if entry.cloud.provisional {
                                // The full pass did finish: its checked cloud
                                // takes the place of the sampled preview.
                                entry.replace_cloud(cloud);
                                replaced = true;
                            }
                            preview_remains = true;
                        }
                        if replaced {
                            self.revision += 1;
                            self.reframe_after_replacement(scene);
                        }
                        self.status = if import.is_some() {
                            "Import cancelled".into()
                        } else if preview_remains {
                            "Octree build cancelled; preview remains open".into()
                        } else {
                            "Octree build cancelled".into()
                        };
                    }
                }
                let detail = if ready {
                    self.schedule_detail()
                } else {
                    self.pending_delete = false;
                    Task::none()
                };
                let pending_delete = if ready && self.pending_delete {
                    self.update(Message::DeleteSelection)
                } else {
                    Task::none()
                };
                return Task::batch([loaded, detail, pending_delete, self.start_next_auto_index()]);
            }
            Message::Loaded(result) => match result {
                Ok(cloud) => {
                    self.cancel_selection_for_scene_change();
                    let cache_source = Arc::clone(&cloud);
                    let name = cloud
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Point cloud");
                    self.status = format!(
                        "{} · {} points · {} sampled",
                        name,
                        format_count(cloud.total_points),
                        format_count(cloud.points.len())
                    );
                    self.clouds.push(CloudEntry {
                        bag_source: is_bag3d_mesh(&cloud.path),
                        load_identity: Arc::clone(&cloud),
                        index_import_id: None,
                        cloud,
                        transform: CloudTransform::default(),
                        centroid_cache: None,
                        mesh: None,
                        mesh_topology: None,
                        mesh_visible: true,
                        faces: None,
                        visible: true,
                        selection: None,
                        deleted: None,
                        index: None,
                        auto_index_queued: false,
                        picked: false,
                        index_building: false,
                        detail_points: None,
                    });
                    self.revision += 1;
                    self.active = Some(self.clouds.len() - 1);
                    if self.section_enabled && self.section_reference_bounds.is_none() {
                        self.reset_section_reference();
                        self.sync_section_coordinate_inputs();
                    }
                    self.frame_new_scene();
                    let photos_task = self.station_photos_task(&cache_source);
                    let cache_task = Task::batch([cached_index_task(cache_source), photos_task]);
                    if let Some(mesh_task) = Self::mesh_task(&self.clouds.last().unwrap().cloud) {
                        return Task::batch([cache_task, mesh_task]);
                    }
                    return Task::batch([cache_task, self.schedule_detail()]);
                }
                Err(error) => self.status = error,
            },
            Message::MeshLoaded(source, result) => {
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| Arc::ptr_eq(&entry.cloud, &source))
                {
                    match result {
                        Ok(Some(measured)) => {
                            self.status = format!(
                                "Mesh displayed: {} vertices, {} triangles",
                                measured.mesh.vertices.len(),
                                measured.mesh.triangles.len()
                            );
                            entry.mesh = Some(measured.mesh);
                            entry.mesh_topology = Some(measured.topology);
                        }
                        Ok(None) => {}
                        Err(error) => self.status = format!("Mesh display failed: {error}"),
                    }
                }
            }
            Message::Refined(source, result) => {
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.matches_source(&source))
                {
                    match result {
                        Ok(cloud) => {
                            let count = cloud.total_points;
                            let indexed = entry.index.is_some();
                            entry.replace_cloud(Arc::clone(&cloud));
                            self.status = format!(
                                "Ready: {} points from {}",
                                format_count(count),
                                source.path.display()
                            );
                            return if indexed {
                                self.schedule_detail()
                            } else {
                                cached_index_task(cloud)
                            };
                        }
                        Err(error) => self.status = format!("Preview failed: {error}"),
                    }
                }
            }
            Message::Export => {
                if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                    let format = self.export_format;
                    let stem = entry
                        .cloud
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("pointcloud");
                    let suggested = format!("{stem}.{}", format.extension());
                    self.status = "Choose where to export the full cloud…".into();
                    let cloud = Arc::clone(&entry.cloud);
                    let deleted = entry.deleted.as_ref().map(Arc::clone);
                    let transform = entry.transform;
                    return save_task(suggested, format, move |path| {
                        let result = if deleted.is_none() && transform.is_identity() {
                            pointcloud_core::export_full(&cloud, &path, format)
                        } else {
                            export_edited_where(
                                &cloud,
                                &path,
                                format,
                                transform,
                                cloud.total_points - deleted.as_ref().map_or(0, |mask| mask.count),
                                |ordinal, _| {
                                    deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
                                },
                            )
                        };
                        result.map(|()| path).map_err(|error| error.to_string())
                    });
                }
            }
            Message::ExportSection => {
                if let (Some(section), Some(entry)) = (
                    self.section_box(),
                    self.active.and_then(|index| self.clouds.get(index)),
                ) {
                    let format = self.export_format;
                    let stem = entry
                        .cloud
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("pointcloud");
                    let suggested = format!("{stem}-section.{}", format.extension());
                    let cloud = Arc::clone(&entry.cloud);
                    let deleted = entry.deleted.as_ref().map(Arc::clone);
                    let transform = entry.transform;
                    self.status = "Choose where to export the full-resolution section…".into();
                    return Task::perform(
                        async move {
                            rfd::AsyncFileDialog::new()
                                .add_filter(format.to_string(), &[format.extension()])
                                .set_file_name(suggested)
                                .save_file()
                                .await
                                .map(|selection| selection.path().to_path_buf())
                        },
                        move |path| {
                            Message::SectionExportPathChosen(
                                Arc::clone(&cloud),
                                section,
                                format,
                                transform,
                                deleted.as_ref().map(Arc::clone),
                                path,
                            )
                        },
                    );
                }
            }
            Message::SectionExportPathChosen(
                cloud,
                section,
                format,
                transform,
                deleted,
                Some(path),
            ) => {
                self.section_export_pending = true;
                self.status = format!(
                    "Exporting section from {} source points…",
                    cloud.total_points
                );
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            export_edited_section(
                                &cloud,
                                &path,
                                format,
                                transform,
                                section,
                                deleted.as_deref(),
                            )
                            .map(|count| (path, count))
                            .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    Message::SectionExported,
                );
            }
            Message::SectionExportPathChosen(_, _, _, _, _, None) => {
                self.status = "Section export cancelled".into();
            }
            Message::SectionExported(result) => {
                self.section_export_pending = false;
                match result {
                    Ok((path, count)) => {
                        self.status =
                            format!("Exported {count} section points to {}", path.display());
                    }
                    Err(error) => self.status = format!("Section export failed: {error}"),
                }
            }
            Message::SurfaceSetting(index, value) => {
                if let Some(field) = self.surface_settings.get_mut(index) {
                    *field = value;
                }
            }
            Message::MeshRequest(mode) => {
                if self.mesh_dialog_pending
                    || self.mesh_job.is_some()
                    || self.closed_mesh.is_running()
                {
                    self.status = "A mesh task is already open or running".into();
                    return Task::none();
                }
                let config = if matches!(mode, MeshMode::Surface) {
                    match self.surface_mesh_config() {
                        Ok(config) => config,
                        Err(error) => {
                            self.status = error;
                            return Task::none();
                        }
                    }
                } else {
                    SurfaceMeshConfig::default()
                };
                if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                    let stem = entry
                        .cloud
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("pointcloud");
                    let suggested = format!(
                        "{stem}-{}.obj",
                        match mode {
                            MeshMode::Terrain => "terrain",
                            MeshMode::Surface => "surface",
                        }
                    );
                    let cloud = Arc::clone(&entry.cloud);
                    let deleted = entry.deleted.as_ref().map(Arc::clone);
                    self.mesh_dialog_pending = true;
                    self.status = match mode {
                        MeshMode::Terrain => "Choose where to save the terrain mesh…",
                        MeshMode::Surface => "Choose where to save the 3D surface mesh…",
                    }
                    .into();
                    return Task::perform(
                        async move {
                            rfd::AsyncFileDialog::new()
                                .add_filter("OBJ mesh", &["obj"])
                                .set_file_name(suggested)
                                .save_file()
                                .await
                                .map(|selection| selection.path().to_path_buf())
                        },
                        move |path| {
                            Message::MeshPathChosen(
                                mode,
                                config,
                                Arc::clone(&cloud),
                                deleted.as_ref().map(Arc::clone),
                                path,
                            )
                        },
                    );
                }
            }
            Message::MeshPathChosen(mode, config, cloud, deleted, Some(path)) => {
                self.mesh_dialog_pending = false;
                if self.mesh_job.is_some() || self.closed_mesh.is_running() {
                    self.status = "A mesh task is already running".into();
                    return Task::none();
                }
                let Some(transform) = self
                    .clouds
                    .iter()
                    .find(|entry| Arc::ptr_eq(&entry.cloud, &cloud))
                    .map(|entry| entry.transform)
                else {
                    self.status = "Mesh source is no longer open".into();
                    return Task::none();
                };
                return self.start_mesh_job(MeshStart {
                    mode,
                    surface_config: config,
                    cloud,
                    deleted,
                    filter: self.mesh_filter(),
                    transform,
                    path,
                    api_job_id: None,
                });
            }
            Message::MeshPathChosen(_, _, _, _, None) => {
                self.mesh_dialog_pending = false;
                self.status = "Mesh save cancelled".into();
            }
            Message::MeshPoll => {
                if let Some(job) = &self.mesh_job {
                    if let Some(id) = &job.api_job_id {
                        if let Some(entry) = self.api_jobs.get_mut(id) {
                            *entry = job.progress_value();
                        }
                    }
                    return Self::mesh_poll_task();
                }
            }
            Message::CancelMesh => {
                if let Some(job) = &self.mesh_job {
                    job.control.cancelled.store(true, Ordering::Relaxed);
                    self.status = format!("Cancelling {} mesh…", job.mode.label());
                }
            }
            Message::MeshReady(mode, result) => {
                if let Some(job) = self.mesh_job.take() {
                    if let Some(id) = job.api_job_id {
                        let state = match &result {
                            Ok((_, path, stats, measured)) => json!({
                                "state": "complete",
                                "path": path,
                                "mode": mode.label(),
                                "source_points": stats.source_points,
                                "vertices": stats.vertices,
                                "triangles": stats.triangles,
                                "open_edges": measured.topology.open_edges,
                                "components": measured.topology.components,
                            }),
                            Err(error) if error == "Operation cancelled" => {
                                json!({"state": "cancelled", "path": job.path})
                            }
                            Err(error) => json!({"state": "failed", "error": error}),
                        };
                        if let Some(entry) = self.api_jobs.get_mut(&id) {
                            *entry = state;
                        }
                    }
                }
                match result {
                    Ok((source, path, stats, measured)) => {
                        if let Some(entry) = self
                            .clouds
                            .iter_mut()
                            .find(|entry| entry.matches_source(&source))
                        {
                            entry.mesh = Some(measured.mesh);
                            entry.mesh_topology = Some(measured.topology);
                            self.status = format!(
                                "{} mesh displayed: {} vertices, {} triangles, {} from {} points → {}",
                                mode.label(),
                                stats.vertices,
                                stats.triangles,
                                mesh_export::topology_text(measured.topology),
                                stats.source_points,
                                path.display()
                            );
                        }
                    }
                    Err(error) if error == "Operation cancelled" => {
                        self.status =
                            format!("{} mesh cancelled; output left unchanged", mode.label())
                    }
                    Err(error) => self.status = format!("Meshing failed: {error}"),
                }
            }
            Message::ExportMesh => return self.export_mesh(),
            Message::MeshExportPathChosen(request, path) => {
                return self.mesh_export_path_chosen(request, path);
            }
            Message::MeshExported(api_job_id, result) => self.mesh_exported(api_job_id, result),
            Message::ToggleBagPanel => return self.set_bag_panel(!self.bag_panel),
            Message::ShowBagPanel => return self.set_bag_panel(true),
            Message::BagField(index, value) => {
                if let Some(field) = self.bag_fields.get_mut(index) {
                    *field = value;
                }
            }
            Message::BagLod(lod) => self.bag_lod = lod,
            Message::BagFromSection => return self.bag_from_section(),
            Message::BagMapDraw(enabled) => {
                self.bag_map_drawing = enabled;
                self.status = if enabled {
                    "Drag a rectangle on the RD map to choose buildings".into()
                } else {
                    "Map panning enabled".into()
                };
            }
            Message::BagMapSelected(bounds) => {
                self.bag_map_drawing = false;
                self.bag_fields = [
                    format!("{:.2}", bounds.min_x),
                    format!("{:.2}", bounds.min_y),
                    format!("{:.2}", bounds.max_x),
                    format!("{:.2}", bounds.max_y),
                ];
                self.status = match bounds.validate() {
                    Ok(()) => format!(
                        "3DBAG area selected: {:.0} × {:.0} m",
                        bounds.max_x - bounds.min_x,
                        bounds.max_y - bounds.min_y
                    ),
                    Err(error) => format!(
                        "Selected area: {}",
                        bag_panel::plain_reason(&error.to_string())
                    ),
                };
            }
            Message::BagMapPan(delta) => {
                let mut view = self.bag_map_view();
                view.pan(delta);
                self.bag_map_center = view.center;
                return self.schedule_bag_map();
            }
            Message::BagMapZoom(amount, point) => {
                if amount.abs() >= 0.1 {
                    let mut view = self.bag_map_view();
                    view.zoom_at(if amount > 0.0 { 1 } else { -1 }, point);
                    self.bag_map_center = view.center;
                    self.bag_map_zoom = view.zoom;
                    return self.schedule_bag_map();
                }
            }
            Message::BagMapFitFields => return self.bag_fit_fields(),
            Message::BagMapHome => {
                self.bag_map_center = [121_000.0, 487_000.0];
                self.bag_map_zoom = 11;
                return self.schedule_bag_map();
            }
            Message::BagMapRefresh(revision) => {
                if !self.bag_panel || revision != self.bag_map_revision {
                    return Task::none();
                }
                let keys: Vec<_> = self
                    .bag_map_view()
                    .visible_tiles()
                    .into_iter()
                    .filter(|key| {
                        !self.bag_map_tiles.contains_key(key) && !self.bag_map_loading.contains(key)
                    })
                    .take(12)
                    .collect();
                if keys.is_empty() {
                    return Task::none();
                }
                self.bag_map_loading.extend(keys.iter().copied());
                let requested = keys.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || bag_map::fetch_tiles(keys))
                            .await
                            .map_err(|error| error.to_string())
                    },
                    move |result| Message::BagMapTilesReady(requested.clone(), result),
                );
            }
            Message::BagMapTilesReady(requested, result) => {
                for key in requested {
                    self.bag_map_loading.remove(&key);
                }
                match result {
                    Ok(tiles) => {
                        let mut failed = None;
                        for (key, result) in tiles {
                            match result {
                                Ok(bytes) => match ::image::load_from_memory_with_format(
                                    &bytes,
                                    ::image::ImageFormat::Png,
                                ) {
                                    Ok(image) if image.width() == 256 && image.height() == 256 => {
                                        self.bag_map_tiles.insert(key, image.to_rgba8());
                                    }
                                    Ok(_) => {
                                        failed = Some("PDOK tile has unexpected dimensions".into())
                                    }
                                    Err(error) => failed = Some(error.to_string()),
                                },
                                Err(error) => failed = Some(error),
                            }
                        }
                        if self.bag_map_tiles.len() > 128 {
                            let visible: HashSet<_> =
                                self.bag_map_view().visible_tiles().into_iter().collect();
                            self.bag_map_tiles.retain(|key, _| visible.contains(key));
                        }
                        self.rebuild_bag_raster();
                        if let Some(error) = failed {
                            self.status = format!("PDOK map unavailable: {error}");
                        }
                    }
                    Err(error) => self.status = format!("PDOK map failed: {error}"),
                }
            }
            Message::OpenPdokLicense => {
                if let Err(error) = open::that("https://www.pdok.nl/copyright") {
                    self.status = format!("Could not open PDOK license: {error}");
                }
            }
            Message::BagDownload => return self.bag_download(),
            Message::BagPathChosen(bounds, lod, path) => {
                return self.bag_path_chosen(bounds, lod, path)
            }
            Message::BagPoll => return self.bag_poll(),
            Message::CancelBag => self.cancel_bag(),
            Message::BagReady(result) => return self.bag_ready(result),
            Message::ExtensionEnabled(id, enabled) => {
                if let Err(error) = self.set_extension_enabled(id, enabled) {
                    self.status = error;
                }
            }
            Message::OpenBagLicense => {
                if let Err(error) = open::that("https://docs.3dbag.nl/nl/copyright/") {
                    self.status = format!("Could not open 3DBAG license: {error}");
                }
            }
            Message::ExportSelection => {
                if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                    if let Some(mask) = entry.selection.as_ref().filter(|mask| mask.count > 0) {
                        let format = self.export_format;
                        let stem = entry
                            .cloud
                            .path
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .unwrap_or("pointcloud");
                        let suggestion = format!("{stem}-selection.{}", format.extension());
                        let cloud = Arc::clone(&entry.cloud);
                        let mask = Arc::clone(mask);
                        let transform = entry.transform;
                        self.status =
                            format!("Choose where to export {} selected points…", mask.count);
                        return save_task(suggestion, format, move |path| {
                            export_edited_where(
                                &cloud,
                                &path,
                                format,
                                transform,
                                mask.count,
                                |ordinal, _| mask.contains(ordinal),
                            )
                            .map(|()| path)
                            .map_err(|error| error.to_string())
                        });
                    }
                }
            }
            Message::RemoveSelection => {
                if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                    if let Some(mask) = entry.selection.as_ref().filter(|mask| mask.count > 0) {
                        let format = self.export_format;
                        let stem = entry
                            .cloud
                            .path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("pointcloud");
                        let suggestion = format!("{stem}-without-selection.{}", format.extension());
                        let cloud = Arc::clone(&entry.cloud);
                        let mask = Arc::clone(mask);
                        let deleted = entry.deleted.as_ref().map(Arc::clone);
                        let transform = entry.transform;
                        let expected = entry.remaining_count().saturating_sub(mask.count);
                        self.status =
                            format!("Choose output file without {} selected points…", mask.count);
                        return save_task(suggestion, format, move |path| {
                            export_edited_where(
                                &cloud,
                                &path,
                                format,
                                transform,
                                expected,
                                |ordinal, _| {
                                    !mask.contains(ordinal)
                                        && deleted
                                            .as_ref()
                                            .is_none_or(|bits| !bits.contains(ordinal))
                                },
                            )
                            .map(|()| path)
                            .map_err(|error| error.to_string())
                        });
                    }
                }
            }
            Message::DeleteSelection => {
                if let Some(index) = self.clouds.iter().position(|entry| {
                    entry.selection.as_ref().is_some_and(|mask| mask.count > 0)
                        && entry.index.is_none()
                        && entry.cloud.point_ordinals.contains(&u64::MAX)
                }) {
                    self.pending_delete = true;
                    self.active = Some(index);
                    if self.index_pending {
                        self.status = "Waiting for the octree before deleting LAZ points…".into();
                        return Task::none();
                    }
                    return self.update(Message::BuildIndex);
                }
                self.pending_delete = false;
                let mut updates = Vec::new();
                let mut removed = 0u64;
                for (index, entry) in self.clouds.iter().enumerate() {
                    let Some(selection) = entry.selection.as_ref().filter(|mask| mask.count > 0)
                    else {
                        continue;
                    };
                    let mut deleted = match entry.deleted.as_deref() {
                        Some(mask) => mask.clone(),
                        None => match DeletionMask::new(entry.cloud.total_points) {
                            Ok(mask) => mask,
                            Err(error) => {
                                self.status = format!("Delete failed: {error}");
                                return Task::none();
                            }
                        },
                    };
                    match deleted.apply(selection) {
                        Ok(added) if added > 0 => {
                            removed += added;
                            updates.push((
                                index,
                                Arc::new(deleted),
                                Arc::clone(&entry.cloud),
                                Arc::clone(selection),
                            ));
                        }
                        Ok(_) => {}
                        Err(error) => {
                            self.status = format!("Delete failed: {error}");
                            return Task::none();
                        }
                    }
                }
                if updates.is_empty() {
                    self.status = "Select visible points to delete first".into();
                    return Task::none();
                }
                let mut members = Vec::with_capacity(updates.len());
                for (index, deleted, cloud, selection) in updates {
                    let entry = &mut self.clouds[index];
                    entry.deleted = Some(deleted);
                    entry.selection = None;
                    members.push((cloud, selection));
                }
                self.undo_deletions.push(EditBatch { members });
                if self.undo_deletions.len() > 8 {
                    self.undo_deletions.remove(0);
                }
                self.redo_deletions.clear();
                self.revision += 1;
                self.status = format!(
                    "Deleted {} points in the open view; Undo restores them",
                    format_count(removed)
                );
                return self.schedule_detail();
            }
            Message::UndoDelete => {
                let Some(batch) = self.undo_deletions.pop() else {
                    return Task::none();
                };
                let mut restored = 0u64;
                for (source, selection) in &batch.members {
                    if let Some(entry) = self
                        .clouds
                        .iter_mut()
                        .find(|entry| entry.matches_source(source))
                    {
                        if let Some(deleted) = entry.deleted.as_mut() {
                            match Arc::make_mut(deleted).undo(selection) {
                                Ok(count) => restored += count,
                                Err(error) => {
                                    self.status = format!("Undo failed: {error}");
                                    return Task::none();
                                }
                            }
                        }
                    }
                }
                self.redo_deletions.push(batch);
                self.revision += 1;
                self.status = format!("Restored {} points", format_count(restored));
                return self.schedule_detail();
            }
            Message::RedoDelete => {
                let Some(batch) = self.redo_deletions.pop() else {
                    return Task::none();
                };
                let mut removed = 0u64;
                for (source, selection) in &batch.members {
                    if let Some(entry) = self
                        .clouds
                        .iter_mut()
                        .find(|entry| entry.matches_source(source))
                    {
                        if let Some(deleted) = entry.deleted.as_mut() {
                            match Arc::make_mut(deleted).apply(selection) {
                                Ok(count) => removed += count,
                                Err(error) => {
                                    self.status = format!("Redo failed: {error}");
                                    return Task::none();
                                }
                            }
                        }
                    }
                }
                self.undo_deletions.push(batch);
                self.revision += 1;
                self.status = format!("Deleted {} points again", format_count(removed));
                return self.schedule_detail();
            }
            Message::DecimationStride(stride) => self.decimation_stride = stride,
            Message::ThinPercent(percent) => self.thin_percent = percent,
            Message::Thin => {
                if self.thin_pending {
                    self.status = "Thinning is already in progress".into();
                    return Task::none();
                }
                if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                    let source = Arc::clone(&entry.cloud);
                    let baseline = entry.deleted.as_ref().map(Arc::clone);
                    let percent = self.thin_percent;
                    self.thin_pending = true;
                    self.status = format!(
                        "Keeping {percent}% of {} points in the open view…",
                        format_count(entry.remaining_count())
                    );
                    return Task::perform(
                        async move {
                            let worker_source = Arc::clone(&source);
                            let worker_baseline = baseline.as_ref().map(Arc::clone);
                            let result = tokio::task::spawn_blocking(move || {
                                SelectionMask::thin_removed(
                                    worker_source.total_points,
                                    worker_baseline.as_deref(),
                                    percent,
                                )
                                .map(Arc::new)
                            })
                            .await
                            .map_err(|error| error.to_string())
                            .and_then(|result| result);
                            (source, baseline, percent, result)
                        },
                        |(source, baseline, percent, result)| Message::ThinReady {
                            source,
                            baseline,
                            percent,
                            result,
                        },
                    );
                }
            }
            Message::ThinReady {
                source,
                baseline,
                percent,
                result,
            } => {
                self.thin_pending = false;
                let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| Arc::ptr_eq(&entry.cloud, &source))
                else {
                    self.status = "Thin cancelled: source is no longer open".into();
                    return Task::none();
                };
                let same_baseline = match (&entry.deleted, &baseline) {
                    (None, None) => true,
                    (Some(current), Some(original)) => Arc::ptr_eq(current, original),
                    _ => false,
                };
                if !same_baseline {
                    self.status =
                        "Thin cancelled: the point cloud changed during processing".into();
                    return Task::none();
                }
                let mask = match result {
                    Ok(mask) if mask.count > 0 => mask,
                    Ok(_) => {
                        self.status = "All visible points are already kept".into();
                        return Task::none();
                    }
                    Err(error) => {
                        self.status = format!("Thin failed: {error}");
                        return Task::none();
                    }
                };
                let mut deleted = match entry.deleted.as_deref() {
                    Some(mask) => mask.clone(),
                    None => match DeletionMask::new(source.total_points) {
                        Ok(mask) => mask,
                        Err(error) => {
                            self.status = format!("Thin failed: {error}");
                            return Task::none();
                        }
                    },
                };
                let removed = match deleted.apply(&mask) {
                    Ok(count) => count,
                    Err(error) => {
                        self.status = format!("Thin failed: {error}");
                        return Task::none();
                    }
                };
                entry.deleted = Some(Arc::new(deleted));
                entry.selection = None;
                self.undo_deletions.push(EditBatch {
                    members: vec![(source, mask)],
                });
                if self.undo_deletions.len() > 8 {
                    self.undo_deletions.remove(0);
                }
                self.redo_deletions.clear();
                self.revision += 1;
                self.status = format!(
                    "Kept {percent}% of the open cloud; hidden {} points. Undo restores them",
                    format_count(removed)
                );
                return self.schedule_detail();
            }
            Message::Decimate => {
                if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                    let format = self.export_format;
                    let stride = self.decimation_stride;
                    let stem = entry
                        .cloud
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("pointcloud");
                    let suggestion = format!("{stem}-1-in-{stride}.{}", format.extension());
                    let cloud = Arc::clone(&entry.cloud);
                    let deleted = entry.deleted.as_ref().map(Arc::clone);
                    let transform = entry.transform;
                    let expected = entry.remaining_count().div_ceil(stride);
                    self.status = format!("Choose output for one point in every {stride}…");
                    return save_task(suggestion, format, move |path| {
                        let mut kept_ordinal = 0u64;
                        export_edited_where(
                            &cloud,
                            &path,
                            format,
                            transform,
                            expected,
                            |ordinal, _| {
                                if deleted.as_ref().is_some_and(|mask| mask.contains(ordinal)) {
                                    return false;
                                }
                                let keep = kept_ordinal.is_multiple_of(stride);
                                kept_ordinal += 1;
                                keep
                            },
                        )
                        .map(|()| path)
                        .map_err(|error| error.to_string())
                    });
                }
            }
            Message::TranslateX(value) => self.translate_x = value,
            Message::TranslateY(value) => self.translate_y = value,
            Message::TranslateZ(value) => self.translate_z = value,
            Message::ScaleAxis(axis, value) => {
                if let Some(input) = self.scale_inputs.get_mut(axis) {
                    *input = value;
                }
            }
            Message::ApplyTranslation => {
                let parsed = [
                    self.translate_x.parse::<f64>(),
                    self.translate_y.parse::<f64>(),
                    self.translate_z.parse::<f64>(),
                ];
                if let [Ok(x), Ok(y), Ok(z)] = parsed {
                    let old_scene = combined_bounds(&self.clouds);
                    if let Some(entry) = self.active.and_then(|index| self.clouds.get_mut(index)) {
                        if let Some(next) =
                            entry.transform.translated([x, y, z], entry.cloud.bounds)
                        {
                            entry.transform = next;
                            if !self.section_enabled {
                                self.reset_section_reference();
                                self.sync_section_coordinate_inputs();
                            }
                            self.preserve_camera_for_scene_change(old_scene);
                            self.revision += 1;
                            self.status = format!(
                                "Moved the open cloud by X {x}, Y {y}, Z {z}; export to save"
                            );
                            return self.schedule_detail();
                        }
                        self.status = "Translation would produce non-finite coordinates".into();
                        return Task::none();
                    }
                }
                self.status = "Enter valid X, Y and Z offsets".into();
            }
            Message::ApplyScale => {
                let parsed = std::array::from_fn(|axis| self.scale_inputs[axis].parse::<f64>());
                let [Ok(x), Ok(y), Ok(z)] = parsed else {
                    self.status = "Enter finite X, Y and Z scale factors".into();
                    return Task::none();
                };
                let factors = [x, y, z];
                if !factors.iter().all(|value| value.is_finite()) {
                    self.status = "Enter finite X, Y and Z scale factors".into();
                    return Task::none();
                }
                if self.scale_job.is_some() {
                    self.status = "A point-centroid calculation is already running".into();
                    return Task::none();
                }
                let Some(cloud_index) = self.active else {
                    self.status = "Open a point cloud first".into();
                    return Task::none();
                };
                let entry = &self.clouds[cloud_index];
                if entry.remaining_count() == 0 {
                    self.status = "Scale needs at least one visible point".into();
                    return Task::none();
                }
                if let Err(error) = entry.cloud.validate_source() {
                    self.status = format!("Scale failed: {error}");
                    return Task::none();
                }
                let cached = entry.centroid_cache.as_ref().and_then(|cache| {
                    same_deletion_mask(cache.deleted.as_ref(), entry.deleted.as_ref())
                        .then_some(cache.source_xyz)
                });
                let resident = cached
                    .map(Ok)
                    .or_else(|| cloud_centroid::resident(&entry.cloud, entry.deleted.as_deref()));
                if let Some(result) = resident {
                    let source_centroid = match result {
                        Ok(value) => value,
                        Err(error) => {
                            self.status = format!("Scale failed: {error}");
                            return Task::none();
                        }
                    };
                    self.clouds[cloud_index].centroid_cache = Some(CentroidCache {
                        source_xyz: source_centroid,
                        deleted: self.clouds[cloud_index].deleted.clone(),
                    });
                    return self.apply_scale_from_source_centroid(
                        cloud_index,
                        factors,
                        source_centroid,
                    );
                }

                let source = Arc::clone(&entry.cloud);
                let index = entry.index.clone();
                let deleted = entry.deleted.clone();
                let transform = entry.transform;
                let progress = Arc::new(AtomicU64::new(0));
                let cancel = Arc::new(AtomicBool::new(false));
                self.next_scale_job_id = self.next_scale_job_id.wrapping_add(1);
                let id = self.next_scale_job_id;
                self.scale_job = Some(ScaleJob {
                    id,
                    cloud_index,
                    source: Arc::clone(&source),
                    deleted: deleted.clone(),
                    transform,
                    factors,
                    progress: Arc::clone(&progress),
                    cancel: Arc::clone(&cancel),
                });
                self.status = format!(
                    "Calculating exact centroid of {} points; progress and Cancel are available below",
                    entry.remaining_count()
                );
                let worker = Task::perform(
                    async move {
                        let result = tokio::task::spawn_blocking(move || {
                            cloud_centroid::streamed(
                                &source,
                                index.as_deref(),
                                deleted.as_deref(),
                                &cancel,
                                &progress,
                            )
                        })
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result);
                        (id, result)
                    },
                    |(id, result)| Message::ScaleReady(id, result),
                );
                return Task::batch([worker, Self::scale_poll_task(id)]);
            }
            Message::ScalePoll(id) => {
                if let Some(job) = self.scale_job.as_ref().filter(|job| job.id == id) {
                    let done = job.progress.load(Ordering::Relaxed);
                    self.status = format!(
                        "Calculating exact point centroid: {done} / {} source points",
                        job.source.total_points
                    );
                    return Self::scale_poll_task(id);
                }
            }
            Message::CancelScale => {
                if let Some(job) = self.scale_job.take() {
                    job.cancel.store(true, Ordering::Relaxed);
                    self.status = "Scale cancelled; the original coordinates remain".into();
                }
            }
            Message::ScaleReady(id, result) => {
                let Some(job) = self.scale_job.take() else {
                    return Task::none();
                };
                if job.id != id {
                    self.scale_job = Some(job);
                    return Task::none();
                }
                let Some(entry) = self.clouds.get(job.cloud_index) else {
                    self.status = "Scale cancelled: cloud is no longer open".into();
                    return Task::none();
                };
                if !entry.matches_source(&job.source)
                    || entry.transform != job.transform
                    || !same_deletion_mask(entry.deleted.as_ref(), job.deleted.as_ref())
                {
                    self.status =
                        "Scale cancelled: the point cloud changed during processing".into();
                    return Task::none();
                }
                let source_centroid = match result {
                    Ok(value) => value,
                    Err(error) => {
                        self.status = format!("Scale failed: {error}");
                        return Task::none();
                    }
                };
                self.clouds[job.cloud_index].centroid_cache = Some(CentroidCache {
                    source_xyz: source_centroid,
                    deleted: job.deleted,
                });
                return self.apply_scale_from_source_centroid(
                    job.cloud_index,
                    job.factors,
                    source_centroid,
                );
            }
            Message::ResetTransform => {
                let old_scene = combined_bounds(&self.clouds);
                let reset_section = self.section_bounds().is_some_and(|section| {
                    self.active
                        .and_then(|index| self.clouds.get(index))
                        .is_some_and(|entry| {
                            (0..3).any(|axis| {
                                section.max[axis] < entry.cloud.bounds.min[axis]
                                    || section.min[axis] > entry.cloud.bounds.max[axis]
                            })
                        })
                });
                if let Some(entry) = self.active.and_then(|index| self.clouds.get_mut(index)) {
                    entry.transform = CloudTransform::default();
                    if reset_section || !self.section_enabled {
                        self.reset_section_reference();
                        if reset_section {
                            self.section_min_percent = [0.0; 3];
                            self.section_max_percent = [100.0; 3];
                        }
                        self.sync_section_coordinate_inputs();
                    }
                    self.preserve_camera_for_scene_change(old_scene);
                    self.revision += 1;
                    self.status = if reset_section {
                        "Source coordinates restored; section box reset to show the cloud"
                    } else {
                        "Restored the source coordinates in the open view"
                    }
                    .into();
                    return self.schedule_detail();
                }
            }
            Message::BuildIndex => {
                if self.index_pending {
                    self.status = "An octree build is already running".into();
                    return Task::none();
                }
                let Some(entry) = self.active.and_then(|index| self.clouds.get_mut(index)) else {
                    self.status = "Open a point cloud first".into();
                    return Task::none();
                };
                if entry.index.is_some() {
                    self.status = "An octree is already ready for this cloud".into();
                    return Task::none();
                }
                entry.auto_index_queued = false;
                entry.index_building = true;
                let source = Arc::clone(&entry.cloud);
                self.status = format!("Building disk octree for {} points…", source.total_points);
                return self.start_index_job(source, false);
            }
            Message::IndexPoll => {
                if self.index_pending {
                    if !self.index_cancel.load(Ordering::Relaxed) {
                        if let Some(snapshot) = self
                            .index_progress
                            .as_ref()
                            .and_then(|value| value.lock().ok().map(|value| *value))
                        {
                            self.status = Self::index_progress_text(snapshot);
                        }
                    }
                    return Self::index_poll_task();
                }
            }
            Message::CancelIndex => {
                if self.index_pending {
                    self.index_cancel.store(true, Ordering::Relaxed);
                    self.status = "Cancelling octree build…".into();
                }
            }
            Message::IndexReady(source, result) | Message::AutoIndexReady(source, result) => {
                self.index_pending = false;
                self.index_progress = None;
                let was_cancelled = self.index_cancel.load(Ordering::Relaxed);
                let mut ready = false;
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.matches_source(&source))
                {
                    entry.index_building = false;
                    match result {
                        Ok(index) => {
                            entry.index = Some(index);
                            self.revision += 1;
                            self.status = format!("Octree ready for {}", source.path.display());
                            ready = true;
                        }
                        Err(error) => {
                            self.status = if was_cancelled {
                                "Octree build cancelled".into()
                            } else {
                                format!("Octree failed: {error}")
                            };
                        }
                    }
                }
                let detail = if ready {
                    self.schedule_detail()
                } else {
                    self.pending_delete = false;
                    Task::none()
                };
                let pending_delete = if ready && self.pending_delete {
                    self.update(Message::DeleteSelection)
                } else {
                    Task::none()
                };
                return Task::batch([detail, pending_delete, self.start_next_auto_index()]);
            }
            Message::CachedIndexReady(source, result) => {
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.matches_source(&source))
                {
                    if entry.index_import_id.is_some() || entry.index.is_some() {
                        return Task::none();
                    }
                    match result {
                        Ok(Some(index)) => {
                            entry.index = Some(index);
                            self.revision += 1;
                            entry.auto_index_queued = false;
                            self.status =
                                format!("Cached octree attached: {}", source.path.display());
                            let detail = self.schedule_detail();
                            let pending_delete = if self.pending_delete {
                                self.update(Message::DeleteSelection)
                            } else {
                                Task::none()
                            };
                            return Task::batch([detail, pending_delete]);
                        }
                        Ok(None) => {
                            if self.auto_index
                                && entry.cloud.total_points >= AUTO_INDEX_MIN_POINTS
                                && !entry.cloud.points.is_empty()
                                && entry.index.is_none()
                                && !entry.index_building
                            {
                                entry.auto_index_queued = true;
                                return self.start_next_auto_index();
                            }
                        }
                        Err(error) => {
                            self.status = format!("Octree cache unavailable: {error}");
                        }
                    }
                }
            }
            Message::SetAutoIndex(enabled) => {
                self.auto_index = enabled;
                let save = self.queue_preferences_save();
                if enabled {
                    let tasks = self
                        .clouds
                        .iter()
                        .filter(|entry| {
                            entry.index.is_none()
                                && !entry.index_building
                                && entry.cloud.total_points >= AUTO_INDEX_MIN_POINTS
                                && !entry.cloud.points.is_empty()
                        })
                        .map(|entry| cached_index_task(Arc::clone(&entry.cloud)));
                    return Task::batch([Task::batch(tasks), save]);
                }
                for entry in &mut self.clouds {
                    entry.auto_index_queued = false;
                }
                return save;
            }
            Message::LoadDetail => {
                if self.detail_pending {
                    self.status = "A viewport LOD request is already running".into();
                    return Task::none();
                }
                let Some(bounds) = combined_bounds(&self.clouds) else {
                    return Task::none();
                };
                let section = self.section_box();
                let size = self.scene_size();
                let projection = self.projection(bounds, size.width, size.height);
                let indexed_sources: Vec<_> = self
                    .clouds
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        (entry.visible)
                            .then(|| {
                                entry
                                    .index
                                    .as_ref()
                                    .map(|tree| (index, Arc::clone(tree), entry.transform))
                            })
                            .flatten()
                    })
                    .collect();
                if indexed_sources.is_empty() {
                    self.status = "Build an octree for a visible cloud first".into();
                    return Task::none();
                }
                let sources: Vec<_> = indexed_sources
                    .into_iter()
                    .filter_map(|(index, tree, transform)| {
                        let coverage = source_lod_coverage(
                            projection,
                            transform.bounds(tree.root.bounds),
                            section,
                        )?;
                        Some((index, tree, transform, coverage))
                    })
                    .collect();
                if sources.is_empty() {
                    self.detail_loaded_revision = Some(self.revision);
                    self.status = "No indexed cloud intersects the current view".into();
                    return Task::none();
                }
                let budget = self.budget as usize;
                let source_weights: Vec<_> = sources
                    .iter()
                    .map(|(_, tree, _, coverage)| {
                        (
                            *coverage,
                            usize::try_from(tree.root.total_points).unwrap_or(usize::MAX),
                        )
                    })
                    .collect();
                let deep_zoom = self.walk.is_some() || self.zoom <= EXACT_VISIBLE_LOD_ZOOM;
                let shown = self.shown_fill(&sources, projection, section);
                let limits = self.first_read_limits(
                    &sources,
                    &source_weights,
                    projection,
                    section,
                    deep_zoom,
                    shown,
                );
                let drawn_elsewhere = self
                    .clouds
                    .iter()
                    .enumerate()
                    .filter(|(index, entry)| {
                        entry.visible && !sources.iter().any(|(source, _, _, _)| source == index)
                    })
                    .map(|(_, entry)| entry.view_len())
                    .fold(0usize, usize::saturating_add);
                let revision = self.revision;
                let cancel = Arc::new(AtomicBool::new(false));
                self.detail_cancel = Arc::clone(&cancel);
                self.detail_pending = true;
                self.detail_request_revision = Some(revision);
                if !self.section_export_pending {
                    self.status = format!(
                        "Refining visible octree nodes in {} cloud(s)…",
                        sources.len()
                    );
                }
                let refinement = LodRefinement {
                    sampled_limits: vec![0; sources.len()],
                    samples: vec![Vec::new(); sources.len()],
                    sources,
                    source_weights,
                    requested: limits,
                    section,
                    projection,
                    cancel,
                    budget,
                    deep_zoom,
                    pace: Arc::clone(&self.lod_pace),
                    shown,
                    drawn_elsewhere,
                    pass: 0,
                };
                let stream = iced::futures::stream::unfold(Some(refinement), |state| async move {
                    let mut refinement = state?;
                    let outcome = tokio::task::spawn_blocking(move || {
                        let step = refinement.advance();
                        (refinement, step)
                    })
                    .await;
                    match outcome {
                        Ok((refinement, Ok(Some(preview)))) => {
                            Some((Ok((false, preview)), Some(refinement)))
                        }
                        Ok((refinement, Ok(None))) => Some((Ok((true, refinement.finish())), None)),
                        Ok((_, Err(error))) => Some((Err(error), None)),
                        Err(error) => Some((Err(error.to_string()), None)),
                    }
                });
                return Task::run(stream, move |result| match result {
                    Ok((false, details)) => Message::DetailPreview(revision, details),
                    Ok((true, details)) => Message::DetailReady(revision, Ok(details)),
                    Err(error) => Message::DetailReady(revision, Err(error)),
                });
            }
            Message::RefreshDetail(revision) => {
                if revision == self.revision
                    && !self.detail_pending
                    && self.detail_loaded_revision != Some(revision)
                {
                    return self.update(Message::LoadDetail);
                }
            }
            Message::DetailPreview(revision, details) => {
                if revision == self.revision {
                    let mut count = 0usize;
                    for (index, points) in details {
                        count += points.len();
                        if let Some(entry) = self.clouds.get_mut(index) {
                            entry.detail_points = Some(points.into());
                        }
                    }
                    if !self.section_export_pending {
                        self.status = format!(
                            "Viewport LOD: {} points; adding detail…",
                            format_count(count)
                        );
                    }
                }
            }
            Message::DetailReady(revision, result) => {
                self.detail_pending = false;
                let urgent = self.detail_urgent_revision.take() == Some(self.revision);
                if revision != self.revision {
                    return if urgent {
                        self.update(Message::LoadDetail)
                    } else {
                        self.schedule_detail()
                    };
                }
                match result {
                    Ok(details) => {
                        let mut count = 0usize;
                        for (index, points) in details {
                            count += points.len();
                            if let Some(entry) = self.clouds.get_mut(index) {
                                entry.detail_points = Some(points.into());
                            }
                        }
                        self.detail_loaded_revision = Some(revision);
                        if !self.section_export_pending {
                            self.status = format!(
                                "Viewport LOD ready: {} points from disk octree",
                                format_count(count)
                            );
                        }
                    }
                    Err(error) if error == "Operation cancelled" => {
                        return if urgent {
                            self.update(Message::LoadDetail)
                        } else {
                            self.schedule_detail()
                        };
                    }
                    Err(error) if !self.section_export_pending => {
                        self.status = format!("Detail failed: {error}")
                    }
                    Err(_) => {}
                }
            }
            Message::ExportFormat(format) => self.export_format = format,
            Message::Exported(result) => match result {
                Ok(path) => self.status = format!("Exported {}", path.display()),
                Err(error) => self.status = format!("Export failed: {error}"),
            },
            Message::SaveCompleted(Some(result)) => return self.update(Message::Exported(result)),
            Message::SaveCompleted(None) => self.status = "Save cancelled".into(),
            Message::Select(index) => {
                if index < self.clouds.len() {
                    self.active = Some(index);
                }
            }
            Message::LayerClick(index) => {
                if index >= self.clouds.len() {
                    return Task::none();
                }
                let extend = self.modifiers.shift();
                let toggle = self.modifiers.command();
                if extend {
                    // Every row from the active one to the clicked one, as listed.
                    let order = self.layer_order();
                    let place = |cloud: usize| {
                        order
                            .iter()
                            .position(|listed| *listed == cloud)
                            .unwrap_or_default()
                    };
                    let (from, to) = (place(self.active.unwrap_or(index)), place(index));
                    let range = from.min(to)..=from.max(to);
                    for (row, cloud) in order.into_iter().enumerate() {
                        let entry = &mut self.clouds[cloud];
                        entry.picked = range.contains(&row) || (toggle && entry.picked);
                    }
                } else if toggle {
                    if let Some(active) = self.active {
                        if !self.clouds.iter().any(|entry| entry.picked) {
                            self.clouds[active].picked = true;
                        }
                    }
                    let entry = &mut self.clouds[index];
                    entry.picked = !entry.picked;
                    if entry.picked {
                        self.active = Some(index);
                    }
                } else {
                    for (cloud, entry) in self.clouds.iter_mut().enumerate() {
                        entry.picked = cloud == index;
                    }
                    self.active = Some(index);
                }
            }
            Message::LayerVisible(index, visible) => {
                let group = self.layer_group(index);
                if group.is_empty() {
                    return Task::none();
                }
                self.cancel_selection_for_scene_change();
                for cloud in group {
                    self.clouds[cloud].visible = visible;
                }
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::LayerRemove(index) => {
                let group = self.layer_group(index);
                return self.remove_clouds(group);
            }
            Message::SetVisible(index, visible) => {
                if index < self.clouds.len() {
                    self.cancel_selection_for_scene_change();
                }
                if let Some(entry) = self.clouds.get_mut(index) {
                    entry.visible = visible;
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::SetMeshVisible(index, visible) => {
                if let Some(entry) = self.clouds.get_mut(index) {
                    entry.mesh_visible = visible;
                }
            }
            Message::Remove(index) => return self.remove_clouds(vec![index]),
            Message::ColorMode(mode) => {
                self.color_mode = mode;
                return self.queue_preferences_save();
            }
            Message::PointSize(size) => {
                self.point_size = size;
                return self.queue_preferences_save();
            }
            Message::SetEyeDome(enabled) => {
                self.eye_dome = enabled;
                return self.queue_preferences_save();
            }
            Message::EyeDomeStrength(strength) => {
                self.eye_dome_strength = strength;
                return self.queue_preferences_save();
            }
            Message::ShowScanPoses(enabled) => {
                self.show_scan_poses = enabled;
                return self.queue_preferences_save();
            }
            Message::ExpandScanPoses(expanded) => self.expand_scan_poses = expanded,
            Message::FitScanPoses => {
                let (Some(scene), Some(focus)) = (
                    combined_bounds(&self.clouds),
                    bounds_with_scan_poses(&self.clouds),
                ) else {
                    self.status = "No scanner positions to frame".into();
                    return Task::none();
                };
                let Some((zoom, pan)) =
                    camera_to_frame_bounds(scene, focus, self.yaw, self.pitch, self.viewport_size)
                else {
                    self.status = "Scanner positions cannot be framed in this view".into();
                    return Task::none();
                };
                self.zoom = zoom;
                self.pan = pan;
                self.show_scan_poses = true;
                self.revision += 1;
                self.status = "Point cloud and scanner positions framed".into();
                return self.schedule_detail();
            }
            Message::CenterScanPose(cloud_index, pose_index) => {
                let (Some(scene), Some((pose, transform))) = (
                    combined_bounds(&self.clouds),
                    self.clouds
                        .get(cloud_index)
                        .filter(|entry| entry.visible)
                        .and_then(|entry| {
                            entry
                                .cloud
                                .scan_poses
                                .get(pose_index)
                                .map(|pose| (pose, entry.transform))
                        }),
                ) else {
                    return Task::none();
                };
                let Some(pan) = pan_to_world(
                    scene,
                    transform.xyz(pose.position),
                    self.yaw,
                    self.pitch,
                    self.zoom,
                    self.viewport_size,
                ) else {
                    self.status =
                        "Station is behind the current view; rotate the camera first".into();
                    return Task::none();
                };
                self.pan = pan;
                self.show_scan_poses = true;
                self.status = format!("Centered on {}", pose.label);
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::StationPhotosReady(source, result) => {
                self.photo_loading.remove(&source);
                match result {
                    Ok(sets) => {
                        self.station_photos.extend(sets);
                        self.rebuild_photo_atlas();
                    }
                    Err(error) => {
                        self.status = format!(
                            "Station photos unavailable for {}: {error}",
                            display_name(&source)
                        );
                    }
                }
            }
            Message::EnterPanorama(cloud_index, station) => {
                let Some((entry, pose)) = self.clouds.get(cloud_index).and_then(|entry| {
                    entry
                        .cloud
                        .scan_poses
                        .get(station)
                        .map(|pose| (entry, pose))
                }) else {
                    return Task::none();
                };
                if !entry
                    .cloud
                    .scan_images
                    .iter()
                    .any(|image| image.station == Some(station))
                {
                    self.status = format!("{} has no station photos", pose.label);
                    return Task::none();
                }
                let position = entry.transform.xyz(pose.position);
                let view = match self.walk {
                    // Keep looking the same way when stepping to another station.
                    Some(previous) => WalkView {
                        eye: position,
                        ..previous
                    },
                    None => WalkView::from_orbit(position, self.yaw, 0.0),
                };
                self.status = format!(
                    "Station photo: {} · drag to look around, scroll to zoom, W A S D to walk out, Esc to leave",
                    pose.label
                );
                self.walk = Some(view);
                self.walk_station = Some((cloud_index, station));
                self.panorama_photos = None;
                self.context_menu = None;
                self.box_select = false;
                self.pick_mode = false;
                self.drag_rectangle = None;
                return self.panorama_task(cloud_index, station);
            }
            Message::PanoramaReady(source, station, result) => {
                let current = self.walk_station.is_some_and(|(cloud, current)| {
                    current == station
                        && self
                            .clouds
                            .get(cloud)
                            .is_some_and(|entry| entry.cloud.path == source)
                });
                if current {
                    match result {
                        Ok(set) => self.panorama_photos = Some(set),
                        Err(error) => self.status = format!("Station photo failed: {error}"),
                    }
                }
            }
            Message::LeaveWalk => {
                if self.leave_walk() {
                    self.status = "Back in the 3D view".into();
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::WalkLook(dx, dy) => {
                let size = self.viewport_size;
                if let Some(view) = &mut self.walk {
                    view.look(dx, dy, size);
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::WalkZoom(steps) => {
                if let Some(view) = &mut self.walk {
                    view.zoom(steps);
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::Modifiers(modifiers) => {
                self.walk_fast = modifiers.shift();
                self.modifiers = modifiers;
            }
            Message::WalkStop => {
                self.walk_keys = [false; 6];
                self.walk_tick = None;
                // A modifier released while another window has focus never arrives.
                self.walk_fast = false;
                self.modifiers = iced::keyboard::Modifiers::default();
            }
            Message::ModelKey(key) => {
                // The File view and the Settings dialog cover the model: a key
                // must not change what is not shown.
                if self.file_open || self.settings.is_some() {
                    return Task::none();
                }
                return self.update(key.message());
            }
            Message::WalkKey(key, pressed) => {
                if pressed && self.file_open {
                    return Task::none();
                }
                if pressed && self.walk.is_none() && !self.start_walk() {
                    return Task::none();
                }
                if self.walk_keys[key as usize] != pressed {
                    self.walk_keys[key as usize] = pressed;
                    if !self.walk_keys.contains(&true) {
                        self.walk_tick = None;
                    }
                }
            }
            Message::WalkTick(now) => {
                let seconds = self
                    .walk_tick
                    .replace(now)
                    .map_or(0.016, |previous| {
                        now.saturating_duration_since(previous).as_secs_f64()
                    })
                    .min(0.1);
                let held = |key: WalkKey| f64::from(u8::from(self.walk_keys[key as usize]));
                let forward = held(WalkKey::Forward) - held(WalkKey::Back);
                let right = held(WalkKey::Right) - held(WalkKey::Left);
                let up = held(WalkKey::Up) - held(WalkKey::Down);
                let step = self.walk_speed() * seconds;
                let Some(view) = &mut self.walk else {
                    return Task::none();
                };
                if forward == 0.0 && right == 0.0 && up == 0.0 {
                    return Task::none();
                }
                view.advance(forward * step, right * step, up * step);
                self.revision += 1;
                return Task::batch([self.sync_walk_station(), self.schedule_detail()]);
            }
            Message::Budget(budget) => {
                self.budget = budget;
                self.revision += 1;
                return Task::batch([self.schedule_detail(), self.queue_preferences_save()]);
            }
            Message::FilterClass(code, visible) => {
                self.class_visibility.set(code, visible);
                self.revision += 1;
            }
            Message::SetSectionEnabled(enabled) => {
                if enabled && self.section_reference_bounds.is_none() {
                    self.reset_section_reference();
                }
                self.section_enabled = enabled;
                if enabled {
                    self.sync_section_coordinate_inputs();
                }
                self.revision += 1;
                self.status = if enabled {
                    "Section box enabled; adjust X, Y and Z in Properties".into()
                } else {
                    "Section box disabled".into()
                };
                return self.schedule_detail();
            }
            Message::SectionMin(axis, value) => {
                if axis < 3 {
                    let upper = (self.section_max_percent[axis] - 0.000_001).max(0.0);
                    self.section_min_percent[axis] = f64::from(value).clamp(0.0, upper);
                    self.sync_section_coordinate_inputs();
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::SectionMax(axis, value) => {
                if axis < 3 {
                    let lower = (self.section_min_percent[axis] + 0.000_001).min(100.0);
                    self.section_max_percent[axis] = f64::from(value).clamp(lower, 100.0);
                    self.sync_section_coordinate_inputs();
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::SectionHandleDelta(axis, is_min, delta) => {
                if axis < 3 && delta.is_finite() {
                    if is_min {
                        let upper = (self.section_max_percent[axis] - 0.000_001).max(0.0);
                        self.section_min_percent[axis] =
                            (self.section_min_percent[axis] + f64::from(delta)).clamp(0.0, upper);
                    } else {
                        let lower = (self.section_min_percent[axis] + 0.000_001).min(100.0);
                        self.section_max_percent[axis] =
                            (self.section_max_percent[axis] + f64::from(delta)).clamp(lower, 100.0);
                    }
                    self.sync_section_coordinate_inputs();
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::SectionCoordinate(axis, is_min, value) => {
                if axis < 3 {
                    self.section_coordinate_inputs[axis][usize::from(!is_min)] = value;
                }
            }
            Message::ApplySectionCoordinates => {
                let Some(overall) = self
                    .section_reference_bounds
                    .or_else(|| combined_bounds(&self.clouds))
                else {
                    self.status = "Open a point cloud before setting section coordinates".into();
                    return Task::none();
                };
                let mut limits = [[0.0; 2]; 3];
                for (axis, pair) in self.section_coordinate_inputs.iter().enumerate() {
                    for (side, input) in pair.iter().enumerate() {
                        let Ok(value) = input.trim().parse::<f64>() else {
                            self.status = format!("Invalid {} coordinate", ["X", "Y", "Z"][axis]);
                            return Task::none();
                        };
                        if !value.is_finite() {
                            self.status = "Section coordinates must be finite".into();
                            return Task::none();
                        }
                        limits[axis][side] = value;
                    }
                }
                let requested = Bounds {
                    min: limits.map(|pair| pair[0]),
                    max: limits.map(|pair| pair[1]),
                };
                let Some(rotation) = parse_rotation(&self.section_rotation_input) else {
                    self.status = "The rotation must be a number of degrees".into();
                    return Task::none();
                };
                let turned = OrientedBox::new(requested, rotation);
                if turned.is_turned() {
                    // A turned box reaches past the model in its corners; it
                    // only has to be a box that reaches the model.
                    let around = turned.aabb();
                    let model = combined_bounds(&self.clouds).unwrap_or(overall);
                    let reaches = (0..3).all(|axis| {
                        around.min[axis] <= model.max[axis] && around.max[axis] >= model.min[axis]
                    });
                    if !turned.is_valid()
                        || (0..2).any(|axis| requested.min[axis] >= requested.max[axis])
                        || !reaches
                        || !self.place_section(turned)
                    {
                        self.status =
                            "Section limits must be ordered and the box must reach the model"
                                .into();
                        return Task::none();
                    }
                    self.section_enabled = true;
                    self.sync_section_coordinate_inputs();
                    self.revision += 1;
                    self.status = "Section box updated from XYZ coordinates and rotation".into();
                    return self.schedule_detail();
                }
                // Back from a turned box to one along the model axes: the
                // reference is the model again, not the frame of the turn.
                let overall = if self.section_rotation != 0.0 {
                    combined_bounds(&self.clouds).unwrap_or(overall)
                } else {
                    overall
                };
                let Some(section) = section_within_model(requested, overall) else {
                    self.status =
                        "Section limits must be ordered and inside the model bounds".into();
                    return Task::none();
                };
                for axis in 0..3 {
                    let span = overall.max[axis] - overall.min[axis];
                    if span > 0.0 {
                        self.section_min_percent[axis] =
                            ((section.min[axis] - overall.min[axis]) / span * 100.0)
                                .clamp(0.0, 100.0);
                        self.section_max_percent[axis] =
                            ((section.max[axis] - overall.min[axis]) / span * 100.0)
                                .clamp(0.0, 100.0);
                    }
                }
                self.set_section_rotation_value(0.0);
                self.section_reference_bounds = Some(overall);
                self.section_enabled = true;
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                self.status = "Section box updated from XYZ coordinates".into();
                return self.schedule_detail();
            }
            Message::ResetSectionBox => {
                // Around the whole model again, in the turn the box has.
                self.reset_section_reference();
                self.section_min_percent = [0.0; 3];
                self.section_max_percent = [100.0; 3];
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::SectionRotationInput(value) => {
                self.section_rotation_input = value;
            }
            Message::ApplySectionRotation => {
                let Some(rotation) = parse_rotation(&self.section_rotation_input) else {
                    self.status = "The rotation must be a number of degrees".into();
                    return Task::none();
                };
                if !self.turn_section(rotation) {
                    self.status = "Open a point cloud before turning the section box".into();
                    return Task::none();
                }
                self.section_enabled = true;
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                self.status = format!(
                    "Section box turned to {}° about its centre",
                    format_rotation(self.section_rotation)
                );
                return self.schedule_detail();
            }
            Message::AlignSectionToWalls => return self.align_section_to_walls(),
            Message::SectionWallsFound(asked, result) => {
                self.section_align_pending = false;
                if self.section_box() != Some(asked) {
                    self.status = "The section box changed while the walls were looked for".into();
                    return Task::none();
                }
                match result {
                    Ok(Some(found)) => {
                        self.turn_section(found.rotation_degrees);
                        self.sync_section_coordinate_inputs();
                        self.revision += 1;
                        self.status = format!(
                            "Section box turned to {}° along the walls, {}° from before",
                            format_rotation(self.section_rotation),
                            format_rotation(found.change_degrees)
                        );
                        return self.schedule_detail();
                    }
                    Ok(None) => {
                        self.status = "No walls found in the middle of the section box; put the box around a few walls and try again".into();
                    }
                    Err(error) => self.status = format!("Could not find the walls: {error}"),
                }
            }
            Message::ZoomToSection => {
                let (Some(scene), Some(section)) =
                    (combined_bounds(&self.clouds), self.section_bounds())
                else {
                    self.status = "Enable a section box before zooming to it".into();
                    return Task::none();
                };
                let Some((zoom, pan)) = camera_to_frame_bounds(
                    scene,
                    section,
                    self.yaw,
                    self.pitch,
                    self.viewport_size,
                ) else {
                    self.status = "Section box cannot be framed in this view".into();
                    return Task::none();
                };
                self.zoom = zoom;
                self.pan = pan;
                self.revision += 1;
                self.status = "Section box framed in the viewport".into();
                return self.schedule_detail();
            }
            purpose @ (Message::FitSectionToSelection | Message::ZoomToSelection) => {
                if self.selection_bounds_pending {
                    return Task::none();
                }
                let focus_camera = matches!(purpose, Message::ZoomToSelection);
                let sources: Vec<_> = self
                    .clouds
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        entry
                            .selection
                            .as_ref()
                            .filter(|mask| mask.count > 0)
                            .map(|mask| SelectedSource {
                                index,
                                cloud: Arc::clone(&entry.cloud),
                                selection: Arc::clone(mask),
                                deleted: entry.deleted.as_ref().map(Arc::clone),
                                transform: entry.transform,
                            })
                    })
                    .collect();
                if sources.is_empty() {
                    self.status = "Select points before framing them".into();
                    return Task::none();
                }
                let snapshots: Vec<(usize, Arc<SelectionMask>)> = sources
                    .iter()
                    .map(|source| (source.index, Arc::clone(&source.selection)))
                    .collect();
                let revision = self.revision;
                // A turned box is fitted in the frame of its turn.
                let frame = (!focus_camera && self.section_rotation != 0.0)
                    .then(|| section_frame_reference(&self.clouds, self.section_rotation))
                    .flatten()
                    .map(|reference| section_pivot_frame(reference, self.section_rotation));
                self.selection_bounds_pending = true;
                self.status = "Finding exact bounds of selected source points…".into();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || selected_source_bounds(&sources, frame))
                            .await
                            .map_err(|error| error.to_string())?
                    },
                    move |result| {
                        Message::SelectionBoundsReady(
                            focus_camera,
                            revision,
                            snapshots.clone(),
                            result,
                        )
                    },
                );
            }
            Message::SelectionBoundsReady(focus_camera, revision, snapshots, result) => {
                self.selection_bounds_pending = false;
                let current_count = self
                    .clouds
                    .iter()
                    .filter(|entry| entry.selection.as_ref().is_some_and(|mask| mask.count > 0))
                    .count();
                if revision != self.revision
                    || current_count != snapshots.len()
                    || snapshots.iter().any(|(index, mask)| {
                        self.clouds
                            .get(*index)
                            .and_then(|entry| entry.selection.as_ref())
                            .is_none_or(|current| !Arc::ptr_eq(current, mask))
                    })
                {
                    self.status = "Selection changed while fitting the section box".into();
                    return Task::none();
                }
                let (selected, count) = match result {
                    Ok(result) => result,
                    Err(error) => {
                        self.status = format!("Could not frame selection: {error}");
                        return Task::none();
                    }
                };
                if focus_camera {
                    let Some(scene) = combined_bounds(&self.clouds) else {
                        return Task::none();
                    };
                    let focus = padded_selection_bounds(scene, selected);
                    let Some((zoom, pan)) = camera_to_frame_bounds(
                        scene,
                        focus,
                        self.yaw,
                        self.pitch,
                        self.viewport_size,
                    ) else {
                        self.status = "Selected points cannot be framed in this view".into();
                        return Task::none();
                    };
                    self.zoom = zoom;
                    self.pan = pan;
                    self.revision += 1;
                    self.status = format!("Framed {count} selected points");
                    return self.schedule_detail();
                }
                let reference = if self.section_rotation != 0.0 {
                    section_frame_reference(&self.clouds, self.section_rotation)
                } else {
                    loaded_bounds(&self.clouds)
                };
                let Some(reference) = reference else {
                    return Task::none();
                };
                self.section_reference_bounds = Some(reference);
                for axis in 0..3 {
                    let span = reference.max[axis] - reference.min[axis];
                    if span <= 0.0 {
                        self.section_min_percent[axis] = 0.0;
                        self.section_max_percent[axis] = 100.0;
                        continue;
                    }
                    let padding = if selected.min[axis] == selected.max[axis] {
                        (span * 0.005).max(0.001)
                    } else {
                        span * 1e-9
                    };
                    let low = (selected.min[axis] - padding).max(reference.min[axis]);
                    let high = (selected.max[axis] + padding).min(reference.max[axis]);
                    self.section_min_percent[axis] = (low - reference.min[axis]) / span * 100.0;
                    self.section_max_percent[axis] = (high - reference.min[axis]) / span * 100.0;
                }
                self.section_enabled = true;
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                self.status = format!("Section box fitted to {count} selected points");
                return self.schedule_detail();
            }
            Message::Orbit(dx, dy) => {
                self.turn_orbit(dx * 0.01, dy * 0.01);
                return self.schedule_detail();
            }
            Message::PickOrbitPoint(pointer, size) => return self.pick_orbit_point(pointer, size),
            Message::OrbitPointPicked(point) => self.set_orbit_point(point),
            Message::Pan(dx, dy) => {
                self.pan[0] += dx;
                self.pan[1] += dy;
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::FinishPan(dx, dy) => {
                let move_task = if dx != 0.0 || dy != 0.0 {
                    self.update(Message::Pan(dx, dy))
                } else {
                    Task::none()
                };
                return Task::batch([move_task, self.update(Message::NavigationFinished)]);
            }
            Message::FinishOrbit(dx, dy) => {
                let move_task = if dx != 0.0 || dy != 0.0 {
                    self.update(Message::Orbit(dx, dy))
                } else {
                    Task::none()
                };
                return Task::batch([move_task, self.update(Message::NavigationFinished)]);
            }
            Message::NavigationFinished => {
                if self.detail_loaded_revision == Some(self.revision)
                    || !self
                        .clouds
                        .iter()
                        .any(|entry| entry.visible && entry.index.is_some())
                {
                    return Task::none();
                }
                // A release without movement: the running request already
                // reads this view, so restarting it would only lose time.
                if self.detail_pending
                    && self.detail_request_revision == Some(self.revision)
                    && !self.detail_cancel.load(Ordering::Relaxed)
                {
                    return Task::none();
                }
                if self.detail_pending {
                    self.detail_cancel.store(true, Ordering::Relaxed);
                    self.detail_urgent_revision = Some(self.revision);
                    return Task::none();
                }
                return self.update(Message::LoadDetail);
            }
            Message::Zoom(delta, pointer, size) => {
                self.viewport_size = size;
                let previous = self.zoom;
                let factor = (-delta * 0.14).exp();
                self.zoom = (self.zoom * factor).clamp(0.000_001, 10_000.0);
                let magnification = previous / self.zoom;
                for (axis, value) in pointer.into_iter().enumerate() {
                    let center = if axis == 0 { size.width } else { size.height } * 0.5;
                    self.pan[axis] =
                        (value - center) * (1.0 - magnification) + self.pan[axis] * magnification;
                }
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::ViewportSize(size) => {
                if size.width > 0.0 && size.height > 0.0 {
                    self.viewport_size = size;
                    self.ribbon_viewport = None;
                    self.revision += 1;
                    return self.schedule_detail();
                }
            }
            Message::ResetCamera => {
                self.leave_walk();
                self.orbit_point = None;
                let (yaw, pitch, label) = CameraPreset::Isometric.orientation();
                self.yaw = yaw;
                self.pitch = pitch;
                self.zoom = 1.0;
                self.pan = [0.0, 0.0];
                self.view_label = label;
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::CameraPreset(preset) => {
                self.leave_walk();
                let (yaw, pitch, label) = preset.orientation();
                self.yaw = yaw;
                self.pitch = pitch;
                self.view_label = label;
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::CubeCorner(corner) => {
                (self.yaw, self.pitch) = view_cube::view_from(corner);
                self.view_label = i18n::key("ISO CORNER");
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::CubeEdge(edge) => {
                (self.yaw, self.pitch) = view_cube::view_from(edge);
                self.view_label = view_cube::edge_label(edge);
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::Views(action) => return self.update_views(action),
            Message::ShowContextMenu(point) => self.context_menu = Some(point),
            Message::DismissContextMenu => self.context_menu = None,
            Message::ContextAction(action) => {
                self.context_menu = None;
                match action {
                    ContextAction::Orbit => {
                        self.box_select = false;
                        self.pick_mode = false;
                        self.measure.leave(true);
                        self.views.leave_tool();
                        self.drag_rectangle = None;
                        self.status = "Orbit mode".into();
                    }
                    ContextAction::BoxSelect => {
                        self.box_select = true;
                        self.pick_mode = false;
                        self.measure.leave(true);
                        self.views.leave_tool();
                        self.status = "Box selection active; Escape exits".into();
                    }
                    ContextAction::PickPoint => {
                        self.pick_mode = true;
                        self.box_select = false;
                        self.measure.leave(true);
                        self.views.leave_tool();
                        self.status = "Point picking active; Escape exits".into();
                    }
                    ContextAction::SectionBox => {
                        return self.update(Message::SetSectionEnabled(!self.section_enabled));
                    }
                    ContextAction::FitView => return self.update(Message::ResetCamera),
                    ContextAction::ClearSelection => return self.update(Message::ClearSelection),
                }
            }
            Message::Escape => {
                if self.settings.is_some() {
                    self.settings_action(settings_dialog::SettingsAction::Cancel);
                    return Task::none();
                }
                if self.file_open {
                    self.file_open = false;
                    return Task::none();
                }
                // A rename or a half-placed annotation ends before anything else.
                if let Some(status) = self.views.cancel_input() {
                    self.status = status.into();
                    return Task::none();
                }
                if self.walk.is_some() {
                    return self.update(Message::LeaveWalk);
                }
                let cancelling = self.cancel_selection();
                self.context_menu = None;
                self.box_select = false;
                self.pick_mode = false;
                // An unfinished measurement is dropped; a finished one stays.
                let measuring = self.measure.leave(false);
                let annotating = self.views.leave_tool();
                self.bag_map_drawing = false;
                self.drag_rectangle = None;
                let deselected = self.selected_total() > 0;
                if deselected {
                    self.pending_delete = false;
                    self.revision += 1;
                    for entry in &mut self.clouds {
                        entry.selection = None;
                    }
                }
                self.status = if cancelling {
                    "Cancelling selection; orbit and right-click menu available".into()
                } else if deselected {
                    "Selection cleared; orbit and right-click menu available".into()
                } else if measuring {
                    "Measuring stopped; orbit and right-click menu available".into()
                } else if annotating {
                    "Annotation tool closed; orbit and right-click menu available".into()
                } else {
                    "Selection tool closed; orbit and right-click menu available".into()
                };
            }
            Message::CancelSelection => {
                if self.cancel_selection() {
                    self.status = "Cancelling full-resolution selection…".into();
                }
            }
            Message::ToggleBoxSelect => {
                self.box_select = !self.box_select;
                self.pick_mode = false;
                self.measure.leave(true);
                self.views.leave_tool();
                self.drag_rectangle = None;
            }
            Message::TogglePickSelect => {
                self.pick_mode = !self.pick_mode;
                self.box_select = false;
                self.measure.leave(true);
                self.views.leave_tool();
                self.drag_rectangle = None;
            }
            Message::Measure(action) => return self.update_measure(action),
            Message::Drawing(action) => return self.update_drawing(action),
            Message::ClosedMesh(action) => return self.update_closed_mesh(action),
            Message::Faces(action) => return self.update_faces(action),
            Message::ClearSelection => {
                self.pending_delete = false;
                if self.selection_pending {
                    self.selection_cancel.store(true, Ordering::Relaxed);
                }
                self.revision += 1;
                for entry in &mut self.clouds {
                    entry.selection = None;
                }
                self.status = "Selection cleared".into();
            }
            Message::SelectionDrag(start, end) => {
                self.drag_rectangle = Some((start, end));
            }
            Message::BoxSelect { start, end, size } => {
                self.pending_delete = false;
                self.drag_rectangle = None;
                self.viewport_size = size;
                if self.selection_pending {
                    self.status = "A full-resolution selection is already running".into();
                    return Task::none();
                }
                if self.pick_mode {
                    return match self.start_point_pick(end, 8.0, size, None) {
                        Ok(task) => task,
                        Err(error) => {
                            self.status = error;
                            Task::none()
                        }
                    };
                }
                let Some(bounds) = combined_bounds(&self.clouds) else {
                    return Task::none();
                };
                let sources: Vec<_> = self
                    .clouds
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| entry.visible)
                    .map(|(index, entry)| SelectionSource {
                        index,
                        cloud: Arc::clone(&entry.cloud),
                        tree: entry.index.as_ref().map(Arc::clone),
                        deleted: entry.deleted.as_ref().map(Arc::clone),
                        transform: entry.transform,
                    })
                    .collect();
                let projection = Projection::new(
                    bounds,
                    self.yaw,
                    self.pitch,
                    self.zoom,
                    self.pan,
                    size.width,
                    size.height,
                );
                let filter = ClassFilter {
                    ground: self.filter_ground,
                    vegetation: self.filter_vegetation,
                    buildings: self.filter_buildings,
                    other: self.filter_other,
                    classes: self.class_visibility,
                    section: self.section_box(),
                };
                let revision = self.revision;
                let rectangle = ScreenRect::from_corners(start, end);
                self.selection_pending = true;
                let cancel = Arc::new(AtomicBool::new(false));
                self.selection_cancel = Arc::clone(&cancel);
                self.status = if sources.iter().all(|source| source.tree.is_some()) {
                    format!(
                        "Selecting exact points through octrees in {} file(s)…",
                        sources.len()
                    )
                } else {
                    format!("Scanning full resolution across {} file(s)…", sources.len())
                };
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            select_full_cancellable(sources, projection, rectangle, filter, cancel)
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    move |result| Message::SelectionReady(revision, result),
                );
            }
            Message::SelectionReady(revision, result) => {
                self.selection_pending = false;
                if self.selection_cancel.load(Ordering::Relaxed) {
                    self.status = "Selection cancelled".into();
                    return Task::none();
                }
                if revision != self.revision {
                    self.status = "Selection discarded because files changed".into();
                    return Task::none();
                }
                match result {
                    Ok(masks) => {
                        for entry in &mut self.clouds {
                            entry.selection = None;
                        }
                        for (index, mask) in masks {
                            if let Some(entry) = self.clouds.get_mut(index) {
                                entry.selection = Some(mask);
                            }
                        }
                        self.status = self.selection_status();
                    }
                    Err(error) => self.status = format!("Selection failed: {error}"),
                }
            }
            Message::PickReady(revision, index, result) => {
                self.selection_pending = false;
                if self.selection_cancel.load(Ordering::Relaxed) {
                    self.status = "Point pick cancelled".into();
                    return Task::none();
                }
                if revision != self.revision {
                    self.status = "Point pick discarded because the view changed".into();
                    return Task::none();
                }
                match result {
                    Ok(record) => {
                        for entry in &mut self.clouds {
                            entry.selection = None;
                        }
                        if let Some(record) = record {
                            let Some(entry) = self.clouds.get_mut(index) else {
                                return Task::none();
                            };
                            let source_xyz = entry.transform.source_xyz(record.point.xyz);
                            match SelectionMask::single_with_source(
                                entry.cloud.total_points,
                                record,
                                source_xyz,
                            ) {
                                Ok(mask) => {
                                    entry.selection = Some(Arc::new(mask));
                                    self.status = format!(
                                        "Point {} selected at X {:.3}, Y {:.3}, Z {:.3}",
                                        record.ordinal + 1,
                                        record.point.xyz[0],
                                        record.point.xyz[1],
                                        record.point.xyz[2]
                                    );
                                }
                                Err(error) => self.status = format!("Point pick failed: {error}"),
                            }
                        } else {
                            self.status = "No point within 8 pixels".into();
                        }
                    }
                    Err(error) => self.status = format!("Point pick failed: {error}"),
                }
            }
        }
        Task::none()
    }

    fn selected_total(&self) -> u64 {
        self.clouds
            .iter()
            .filter_map(|entry| entry.selection.as_ref())
            .map(|selection| selection.count)
            .sum()
    }

    fn cancel_selection(&mut self) -> bool {
        if !self.selection_pending {
            return false;
        }
        if !self.selection_cancel.swap(true, Ordering::Relaxed) {
            self.revision += 1;
        }
        true
    }

    fn cancel_selection_for_scene_change(&self) {
        if self.selection_pending {
            self.selection_cancel.store(true, Ordering::Relaxed);
        }
    }

    fn selection_status(&self) -> String {
        let count = self.selected_total();
        let shown: usize = self
            .clouds
            .iter()
            .filter_map(|entry| entry.selection.as_ref())
            .map(|selection| selection.highlights.len())
            .sum();
        let noun = if count == 1 { "point" } else { "points" };
        let mut status = format!("{} {noun} selected at full resolution", format_count(count));
        if count > shown as u64 {
            status.push_str(&format!(" · {} highlighted", format_count(shown)));
        }
        status
    }

    /// The section box while it is on: its limits before the turn and the
    /// turn about the vertical through its centre.
    fn section_box(&self) -> Option<OrientedBox> {
        self.section_enabled.then(|| self.section_shape()).flatten()
    }

    /// The axis-aligned box around the section box while it is on, for what
    /// only needs to know where the box can hold points.
    fn section_bounds(&self) -> Option<Bounds> {
        self.section_box().map(|section| section.aabb())
    }

    /// The section box as it is set, also while it is off.
    fn section_shape(&self) -> Option<OrientedBox> {
        let reference = self
            .section_reference_bounds
            .or_else(|| section_frame_reference(&self.clouds, self.section_rotation))?;
        let mut local = reference;
        for axis in 0..3 {
            let span = reference.max[axis] - reference.min[axis];
            local.min[axis] = reference.min[axis] + span * self.section_min_percent[axis] / 100.0;
            local.max[axis] = reference.min[axis] + span * self.section_max_percent[axis] / 100.0;
        }
        if self.section_rotation == 0.0 {
            return Some(local.into());
        }
        let center = section_pivot_frame(reference, self.section_rotation).to_scene(local.center());
        let half: [f64; 3] = std::array::from_fn(|axis| (local.max[axis] - local.min[axis]) * 0.5);
        Some(OrientedBox::new(
            Bounds {
                min: std::array::from_fn(|axis| center[axis] - half[axis]),
                max: std::array::from_fn(|axis| center[axis] + half[axis]),
            },
            self.section_rotation,
        ))
    }

    /// Put a section box in place, turned as it is: its turn becomes the
    /// turn of the box and its limits the percentages of a reference in the
    /// frame of that turn, around the clouds and the box. Nothing happens
    /// without a cloud or with a box that is not one.
    fn place_section(&mut self, section: OrientedBox) -> bool {
        if !section.is_valid() {
            return false;
        }
        let rotation = if section.is_turned() {
            pointcloud_core::normalized_degrees(section.rotation_degrees)
        } else {
            0.0
        };
        let Some(mut reference) = section_frame_reference(&self.clouds, rotation) else {
            return false;
        };
        let local = if rotation == 0.0 {
            section.bounds
        } else {
            let center = section_pivot_frame(reference, rotation).to_box(section.center());
            let half = section.size().map(|size| size * 0.5);
            Bounds {
                min: std::array::from_fn(|axis| center[axis] - half[axis]),
                max: std::array::from_fn(|axis| center[axis] + half[axis]),
            }
        };
        // The reference grows to hold the box: about its centre across, so
        // that the turn keeps its pivot, and up and down as far as needed.
        for (axis, pivot) in reference.center().into_iter().enumerate() {
            if axis < 2 && rotation != 0.0 {
                let reach = [
                    reference.min[axis],
                    reference.max[axis],
                    local.min[axis],
                    local.max[axis],
                ]
                .iter()
                .map(|value| (value - pivot).abs())
                .fold(0.0, f64::max);
                reference.min[axis] = pivot - reach;
                reference.max[axis] = pivot + reach;
            } else {
                reference.min[axis] = reference.min[axis].min(local.min[axis]);
                reference.max[axis] = reference.max[axis].max(local.max[axis]);
            }
        }
        for axis in 0..3 {
            let span = reference.max[axis] - reference.min[axis];
            if span > 0.0 {
                self.section_min_percent[axis] =
                    ((local.min[axis] - reference.min[axis]) / span * 100.0).clamp(0.0, 100.0);
                self.section_max_percent[axis] =
                    ((local.max[axis] - reference.min[axis]) / span * 100.0).clamp(0.0, 100.0);
            } else {
                self.section_min_percent[axis] = 0.0;
                self.section_max_percent[axis] = 100.0;
            }
        }
        self.section_reference_bounds = Some(reference);
        self.set_section_rotation_value(rotation);
        true
    }

    /// Turn the section box about the vertical through its centre, keeping
    /// its size.
    fn turn_section(&mut self, degrees: f64) -> bool {
        let Some(shape) = self.section_shape() else {
            return false;
        };
        self.place_section(OrientedBox::new(shape.bounds, degrees))
    }

    fn set_section_rotation_value(&mut self, rotation: f64) {
        self.section_rotation = rotation;
        self.section_rotation_input = format_rotation(rotation);
    }

    /// The reference of the section box for the clouds as they are now, in
    /// the frame of its turn: the box that a reset gives.
    fn reset_section_reference(&mut self) {
        self.section_reference_bounds =
            section_frame_reference(&self.clouds, self.section_rotation);
    }

    /// The section box as the local API reports it: its limits before the
    /// turn and the turn in degrees.
    fn section_value(&self) -> Value {
        match self.section_box() {
            Some(section) => json!({
                "min": section.bounds.min,
                "max": section.bounds.max,
                "rotation": self.section_rotation,
            }),
            None => Value::Null,
        }
    }

    fn sync_section_coordinate_inputs(&mut self) {
        if let Some(section) = self.section_box() {
            for axis in 0..3 {
                self.section_coordinate_inputs[axis] = [
                    format!("{:.6}", section.bounds.min[axis]),
                    format!("{:.6}", section.bounds.max[axis]),
                ];
            }
            self.section_rotation_input = format_rotation(self.section_rotation);
        }
    }

    fn bag_map_view(&self) -> MapView {
        MapView {
            center: self.bag_map_center,
            zoom: self.bag_map_zoom,
            width: bag_map::WIDTH,
            height: bag_map::HEIGHT,
        }
    }

    fn bag_fields_bounds(&self) -> Option<BagBounds> {
        let numbers = self
            .bag_fields
            .each_ref()
            .map(|field| field.trim().parse::<f64>().ok());
        let [Some(min_x), Some(min_y), Some(max_x), Some(max_y)] = numbers else {
            return None;
        };
        (min_x.is_finite()
            && min_y.is_finite()
            && max_x.is_finite()
            && max_y.is_finite()
            && max_x > min_x
            && max_y > min_y)
            .then_some(BagBounds {
                min_x,
                min_y,
                max_x,
                max_y,
            })
    }

    fn schedule_bag_map(&mut self) -> Task<Message> {
        self.rebuild_bag_raster();
        self.bag_map_revision += 1;
        let revision = self.bag_map_revision;
        Task::perform(
            async move {
                tokio::time::sleep(Duration::from_millis(140)).await;
                revision
            },
            Message::BagMapRefresh,
        )
    }

    fn rebuild_bag_raster(&mut self) {
        self.bag_map_raster = bag_map::compose_raster(self.bag_map_view(), &self.bag_map_tiles);
    }

    fn start_next_auto_index(&mut self) -> Task<Message> {
        if self.index_pending || !self.auto_index {
            return Task::none();
        }
        let next = self
            .active
            .filter(|index| {
                self.clouds
                    .get(*index)
                    .is_some_and(|entry| entry.auto_index_queued)
            })
            .or_else(|| self.clouds.iter().position(|entry| entry.auto_index_queued));
        let Some(index) = next else {
            return Task::none();
        };
        let entry = &mut self.clouds[index];
        entry.auto_index_queued = false;
        entry.index_building = true;
        let source = Arc::clone(&entry.cloud);
        self.status = format!(
            "Indexing {} points for viewport detail: {}",
            source.total_points,
            display_name(&source.path)
        );
        self.start_index_job(source, true)
    }

    /// What the sets now drawn for the clouds of a refinement put in the
    /// viewport of its camera, thinned the way the renderer thins them.
    fn shown_fill(
        &self,
        sources: &[(usize, Arc<OctreeIndex>, CloudTransform, f32)],
        projection: Projection,
        section: Option<OrientedBox>,
    ) -> ScreenFill {
        let mut shown = ScreenFill::default();
        for entry in sources
            .iter()
            .filter_map(|(index, _, _, _)| self.clouds.get(*index))
        {
            let transform = entry.transform;
            match &entry.detail_points {
                Some(detail) => shown.add(
                    detail,
                    |record| transform.xyz(record.point.xyz),
                    projection,
                    section,
                ),
                None => shown.add(
                    &entry.cloud.points,
                    |point| transform.xyz(point.xyz),
                    projection,
                    section,
                ),
            }
        }
        let drawn: usize = self
            .clouds
            .iter()
            .filter(|entry| entry.visible)
            .map(CloudEntry::view_len)
            .sum();
        shown.points /= drawn.div_ceil((self.budget as usize).max(1)).max(1);
        shown
    }

    /// Cells of the viewport that a first pass over these clouds can bring
    /// points to. The sample a cloud keeps in memory outlines it better than
    /// its box, which a few stray far points stretch well past the scan. At
    /// deep zoom too little of that sample is in view to outline anything.
    fn first_pass_reach(
        &self,
        sources: &[(usize, Arc<OctreeIndex>, CloudTransform, f32)],
        projection: Projection,
        section: Option<OrientedBox>,
        deep_zoom: bool,
    ) -> usize {
        // A union over the clouds: overlapping scans reach the same part of
        // the viewport only once.
        let mut reach = ScreenFill::default();
        for (index, tree, transform, _) in sources {
            let sample = self
                .clouds
                .get(*index)
                .map(|entry| entry.cloud.points.as_slice())
                .filter(|points| !deep_zoom && !points.is_empty());
            if let Some(points) = sample {
                reach.add(
                    points,
                    |point| transform.xyz(point.xyz),
                    projection,
                    section,
                );
            } else if let Some(bounds) = section_clipped(
                transform.bounds(tree.root.bounds),
                section.map(|clip| clip.aabb()),
            ) {
                reach.add_box(projection, bounds);
            }
        }
        reach.cell_count()
    }

    /// Limits per source for the first read of a refinement: the whole
    /// budget when one pass is the plan, else a first pass sized for this
    /// computer and bound by what this view gives from node previews.
    fn first_read_limits(
        &self,
        sources: &[(usize, Arc<OctreeIndex>, CloudTransform, f32)],
        source_weights: &[(f32, usize)],
        projection: Projection,
        section: Option<OrientedBox>,
        deep_zoom: bool,
        shown: ScreenFill,
    ) -> Vec<usize> {
        let budget = self.budget as usize;
        let effective = budget.min(
            source_weights
                .iter()
                .fold(0usize, |sum, (_, capacity)| sum.saturating_add(*capacity)),
        );
        let first = first_pass_budget(
            effective,
            self.lod_pace.read_points_per_ms(),
            self.lod_pace.build_points_per_ms(),
            sources.len(),
        );
        let first = if first < effective {
            let reach = self.first_pass_reach(sources, projection, section, deep_zoom);
            plan_first_pass(effective, first, shown, reach)
        } else {
            first
        };
        (first < effective)
            .then(|| {
                first_pass_limits(first, source_weights, |slot, limit| {
                    // The exact scan at deep zoom reads the leaves in view
                    // whatever the limit, so there is nothing to stay within.
                    if deep_zoom {
                        return None;
                    }
                    let (_, tree, transform, _) = &sources[slot];
                    preview_tier_points(&tree.root, limit, |bounds| {
                        lod_node_span(*transform, section, projection, bounds)
                    })
                })
            })
            .flatten()
            .unwrap_or_else(|| distribute_lod_budget(budget, source_weights))
    }

    fn schedule_detail(&self) -> Task<Message> {
        self.detail_cancel.store(true, Ordering::Relaxed);
        if !self
            .clouds
            .iter()
            .any(|entry| entry.visible && entry.index.is_some())
        {
            return Task::none();
        }
        let revision = self.revision;
        Task::perform(
            async move {
                tokio::time::sleep(Duration::from_millis(220)).await;
                revision
            },
            Message::RefreshDetail,
        )
    }

    /// Keep the view after a preview gave way to its checked cloud: framed
    /// anew when the user left the camera alone, otherwise where they put it.
    fn reframe_after_replacement(&mut self, old_scene: Option<Bounds>) {
        if self.auto_camera == Some((self.yaw, self.pitch, self.zoom, self.pan)) {
            self.frame_new_scene();
        } else {
            self.preserve_camera_for_scene_change(old_scene);
        }
    }

    fn preserve_camera_for_scene_change(&mut self, old_scene: Option<Bounds>) {
        let (Some(old_scene), Some(new_scene)) = (old_scene, combined_bounds(&self.clouds)) else {
            return;
        };
        if self.walk.is_some() {
            // The walking camera has its own position; nothing to compensate.
            return;
        }
        let size = self.viewport_size;
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let anchor = old_scene.center();
        let old_projection = Projection::new(
            old_scene,
            self.yaw,
            self.pitch,
            self.zoom,
            self.pan,
            size.width,
            size.height,
        );
        let next_zoom = (f64::from(self.zoom) * old_scene.extent().max(0.001)
            / new_scene.extent().max(0.001))
        .clamp(0.000_001, 10_000.0) as f32;
        let new_projection = Projection::new(
            new_scene,
            self.yaw,
            self.pitch,
            next_zoom,
            [0.0; 2],
            size.width,
            size.height,
        );
        if let (Some(old), Some(new)) = (
            old_projection.project_unclipped(anchor),
            new_projection.project_unclipped(anchor),
        ) {
            let pan = [old.0 - new.0, old.1 - new.1];
            if pan.iter().all(|value| value.is_finite()) {
                self.zoom = next_zoom;
                self.pan = pan;
            }
        }
    }

    fn ribbon(&self) -> Element<'_, Message> {
        use opencad_ribbon::RibbonItem;

        let file_button = container(
            button(text(i18n::tr("File")).size(12))
                .on_press(Message::ToggleFile)
                .style(|theme, status| {
                    opencad_ribbon::file_tab_style(theme, self.file_open, status)
                })
                .padding([5, 13]),
        )
        .padding([1, 8]);
        // The one tab of the ribbon; it also leads back from the File view.
        let home_tab = button(text(i18n::tr("Home")).size(12))
            .on_press_maybe(self.file_open.then_some(Message::ToggleFile))
            .style(|theme, status| opencad_ribbon::tab_style(theme, !self.file_open, status))
            .padding([5, 13]);
        let quick_access = row![
            opencad_ribbon::quick_access_btn(
                icon_svg(ToolIcon::Open, 20.0),
                "Import point cloud",
                Some(Message::Open),
            ),
            opencad_ribbon::quick_access_btn(
                icon_svg(ToolIcon::OpenFolder, 20.0),
                "Open scan folder",
                Some(Message::OpenFolder),
            ),
            opencad_ribbon::quick_access_btn(
                icon_svg(ToolIcon::Export, 20.0),
                "Export active point cloud",
                self.active.map(|_| Message::Export),
            ),
            opencad_ribbon::quick_access_btn(
                icon_svg(ToolIcon::Undo, 20.0),
                "Undo delete",
                (!self.undo_deletions.is_empty()).then_some(Message::UndoDelete),
            ),
            opencad_ribbon::quick_access_btn(
                icon_svg(ToolIcon::Redo, 20.0),
                "Redo delete",
                (!self.redo_deletions.is_empty()).then_some(Message::RedoDelete),
            ),
        ]
        .spacing(4);
        let settings_button = button(text(i18n::tr("Settings")).size(12))
            .on_press(Message::Settings(settings_dialog::SettingsAction::Open))
            .style(|theme, status| opencad_ribbon::tab_style(theme, false, status))
            .padding([5, 13]);
        let logo = svg(svg::Handle::from_memory(
            include_bytes!("../../assets/icons/logo.svg").as_slice(),
        ))
        .width(20)
        .height(20);
        let top_strip = container(
            row![
                logo,
                file_button,
                home_tab,
                iced::widget::horizontal_space(),
                quick_access,
                settings_button
            ]
            .width(Fill)
            .align_y(iced::Alignment::Center)
            .padding([0, 8]),
        )
        .width(Fill)
        .height(29)
        .style(|theme| container::Style::default().background(ui_theme::colors(theme).tabs));
        if self.file_open {
            return container(top_strip).width(Fill).style(ribbon_style).into();
        }

        let has_active = self.active.is_some();
        let selected = self.selected_total();
        let preset = |label: &'static str, preset: CameraPreset, name: &'static str| {
            RibbonItem::Small(small_tool_button(
                label,
                Message::CameraPreset(preset),
                self.view_label == name,
            ))
        };
        let view = opencad_ribbon::render_group_items(
            "VIEW",
            vec![
                RibbonItem::Small(small_tool_button("Zoom all", Message::ResetCamera, false)),
                RibbonItem::Small(small_tool_button_when(
                    "Fit stations",
                    Message::FitScanPoses,
                    false,
                    self.clouds
                        .iter()
                        .any(|entry| entry.visible && !entry.cloud.scan_poses.is_empty()),
                )),
                preset("Isometric", CameraPreset::Isometric, "ISOMETRIC"),
                preset("Top", CameraPreset::Top, "TOP"),
                preset("Front", CameraPreset::Front, "FRONT"),
                preset("Right", CameraPreset::Right, "RIGHT"),
                preset("Bottom", CameraPreset::Bottom, "BOTTOM"),
                preset("Back", CameraPreset::Back, "BACK"),
                preset("Left", CameraPreset::Left, "LEFT"),
            ],
        );
        let display = ribbon_group(
            "DISPLAY",
            column![
                row![
                    small_color_button("RGB", ColorMode::Rgb, self.color_mode),
                    small_color_button("Elevation", ColorMode::Elevation, self.color_mode),
                    small_color_button("Intensity", ColorMode::Intensity, self.color_mode),
                    small_color_button(
                        "Classification",
                        ColorMode::Classification,
                        self.color_mode
                    ),
                ]
                .spacing(2),
                row![
                    small_tool_button(
                        "Eye-dome",
                        Message::SetEyeDome(!self.eye_dome),
                        self.eye_dome,
                    ),
                    small_tool_button(
                        "Stations",
                        Message::ShowScanPoses(!self.show_scan_poses),
                        self.show_scan_poses,
                    ),
                    iced::widget::Space::with_width(4),
                    text(i18n::tr("Size")).size(12),
                    slider(0.1..=20.0, self.point_size, Message::PointSize)
                        .step(0.1_f32)
                        .width(88),
                    text(format!("{:.1}", self.point_size)).size(11).width(26),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .height(opencad_ribbon::ROW_H),
                row![
                    text(i18n::tr("Budget")).size(12),
                    slider(100_000..=MAX_POINT_BUDGET, self.budget, Message::Budget)
                        .step(100_000_u32)
                        .width(230),
                    text(if self.budget >= 1_000_000 {
                        format!("{:.1}M", self.budget as f64 / 1_000_000.0)
                    } else {
                        format!("{}k", self.budget / 1_000)
                    })
                    .size(11)
                    .width(34),
                ]
                .spacing(5)
                .align_y(iced::Alignment::Center)
                .height(opencad_ribbon::ROW_H),
            ]
            .spacing(1)
            .into(),
        );
        let section = opencad_ribbon::render_group_items(
            "SECTION BOX",
            vec![
                RibbonItem::Small(small_tool_button(
                    "Section box",
                    Message::SetSectionEnabled(!self.section_enabled),
                    self.section_enabled,
                )),
                RibbonItem::Small(small_tool_button_when(
                    "Fit selection",
                    Message::FitSectionToSelection,
                    false,
                    selected > 0 && !self.selection_bounds_pending,
                )),
                RibbonItem::Small(small_tool_button(
                    "Reset box",
                    Message::ResetSectionBox,
                    false,
                )),
                self.drawing_ribbon_item(),
            ],
        );
        // A running selection scan offers its cancel action instead of the zoom.
        let zoom_selection = if self.selection_pending {
            small_tool_button("Cancel selection", Message::CancelSelection, false)
        } else {
            small_tool_button_when(
                "Zoom selection",
                Message::ZoomToSelection,
                false,
                selected > 0 && !self.selection_bounds_pending,
            )
        };
        let selection = ribbon_group(
            "SELECTION",
            column![
                row![
                    small_tool_button("Box select", Message::ToggleBoxSelect, self.box_select),
                    small_tool_button("Pick point", Message::TogglePickSelect, self.pick_mode),
                ]
                .spacing(2),
                row![
                    small_tool_button("Clear", Message::ClearSelection, false),
                    small_tool_button_when("Delete", Message::DeleteSelection, false, selected > 0),
                ]
                .spacing(2),
                zoom_selection,
            ]
            .spacing(1)
            .into(),
        );
        let edit_action = |tool: Element<'static, Message>| container(tool).width(66);
        let scale_action = if self.scale_job.is_some() {
            small_tool_button("Cancel", Message::CancelScale, false)
        } else {
            small_tool_button_when("Scale", Message::ApplyScale, false, has_active)
        };
        let edit = ribbon_group(
            "EDIT",
            column![
                row![
                    axis_input("X", "0", &self.translate_x, Message::TranslateX),
                    axis_input("Y", "0", &self.translate_y, Message::TranslateY),
                    axis_input("Z", "0", &self.translate_z, Message::TranslateZ),
                    edit_action(small_tool_button_when(
                        "Move",
                        Message::ApplyTranslation,
                        false,
                        has_active,
                    )),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .height(opencad_ribbon::ROW_H),
                row![
                    axis_input("X", "1", &self.scale_inputs[0], |value| {
                        Message::ScaleAxis(0, value)
                    }),
                    axis_input("Y", "1", &self.scale_inputs[1], |value| {
                        Message::ScaleAxis(1, value)
                    }),
                    axis_input("Z", "1", &self.scale_inputs[2], |value| {
                        Message::ScaleAxis(2, value)
                    }),
                    edit_action(scale_action),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .height(opencad_ribbon::ROW_H),
                row![
                    text(i18n::tr("Keep")).size(12).width(44),
                    slider(1..=100, self.thin_percent, Message::ThinPercent).width(102),
                    text(format!("{}%", self.thin_percent)).size(11).width(30),
                    edit_action(small_tool_button_when(
                        "Thin",
                        Message::Thin,
                        false,
                        has_active && !self.thin_pending,
                    )),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center)
                .height(opencad_ribbon::ROW_H),
            ]
            .spacing(1)
            .into(),
        );
        let mesh_idle = has_active
            && self.mesh_job.is_none()
            && !self.mesh_dialog_pending
            && !self.closed_mesh.is_running();
        let mut surface_tools = vec![
            RibbonItem::Small(small_tool_button_when(
                "Terrain mesh",
                Message::MeshRequest(MeshMode::Terrain),
                false,
                mesh_idle,
            )),
            RibbonItem::Small(small_tool_button_when(
                "3D surface",
                Message::MeshRequest(MeshMode::Surface),
                false,
                mesh_idle,
            )),
            self.closed_mesh_ribbon_item(),
            self.faces_ribbon_item(),
        ];
        if self.mesh_job.is_some() {
            surface_tools.push(RibbonItem::Small(small_tool_button(
                "Cancel mesh",
                Message::CancelMesh,
                false,
            )));
        }
        // A running manual build offers its cancel action instead of the start.
        let build_index = if self.index_pending && !self.indexing_during_import() {
            small_tool_button("Cancel index", Message::CancelIndex, false)
        } else {
            small_tool_button_when(
                "Build index",
                Message::BuildIndex,
                false,
                has_active && !self.index_pending,
            )
        };
        let index = opencad_ribbon::render_group_items(
            "INDEX",
            vec![
                RibbonItem::Small(build_index),
                RibbonItem::Small(small_tool_button_when(
                    "Refresh LOD",
                    Message::LoadDetail,
                    false,
                    self.active
                        .and_then(|index| self.clouds.get(index))
                        .is_some_and(|entry| entry.index.is_some()),
                )),
                RibbonItem::Small(small_tool_button(
                    "Auto-index",
                    Message::SetAutoIndex(!self.auto_index),
                    self.auto_index,
                )),
            ],
        );
        let groups = row![
            view,
            display,
            section,
            selection,
            self.measure.ribbon(),
            self.views_ribbon(),
            edit,
            opencad_ribbon::render_group_items("SURFACE", surface_tools),
            index,
        ]
        .spacing(2);
        let group_strip = scrollable(
            container(groups)
                .padding([0, 4])
                .width(iced::Length::Shrink)
                .height(opencad_ribbon::TOOL_BAR_H),
        )
        .id(ribbon_scroll_id())
        .on_scroll(|viewport| {
            Message::RibbonViewport(
                viewport.absolute_offset().x,
                viewport.bounds().width,
                viewport.content_bounds().width,
            )
        })
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(5).scroller_width(5),
        ))
        .width(Fill)
        .height(opencad_ribbon::TOOL_BAR_H);
        let scroll_button = |label: &'static str, direction: f32, enabled: bool| {
            container(
                button(text(label).size(26))
                    .on_press_maybe(enabled.then_some(Message::RibbonScroll(direction)))
                    .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                    .width(26)
                    .height(38)
                    .padding(0),
            )
            .width(30)
            .height(opencad_ribbon::TOOL_BAR_H)
            .align_y(iced::Alignment::Center)
            .align_x(iced::Alignment::Center)
        };
        let tool_strip: Element<'_, Message> = if let Some((offset, width, content_width)) = self
            .ribbon_viewport
            .filter(|(_, width, content_width)| *content_width > *width + 1.0)
        {
            row![
                scroll_button("‹", -1.0, offset > 1.0),
                group_strip,
                scroll_button("›", 1.0, offset + width < content_width - 1.0),
            ]
            .height(opencad_ribbon::TOOL_BAR_H)
            .align_y(iced::Alignment::Center)
            .into()
        } else {
            group_strip.into()
        };
        container(
            column![
                top_strip,
                container(text(""))
                    .width(Fill)
                    .height(1)
                    .style(|theme| container::Style::default()
                        .background(ui_theme::colors(theme).accent)),
                tool_strip,
            ]
            .spacing(0),
        )
        .width(Fill)
        .style(ribbon_style)
        .into()
    }

    fn project_panel(&self) -> Element<'_, Message> {
        let mut cloud_count = match self.clouds.len() {
            1 => "1 point cloud".to_owned(),
            count => format!("{count} point clouds"),
        };
        let picked = self.clouds.iter().filter(|entry| entry.picked).count();
        if picked > 1 {
            cloud_count.push_str(&format!("  ·  {picked} selected"));
        }
        let mut files = column![
            text(i18n::tr("PROJECT"))
                .size(14)
                .font(Font::with_name("Space Grotesk")),
            text(cloud_count)
                .size(11)
                .color(self.ui_theme.colors().muted),
            button(i18n::tr("+  Add point cloud"))
                .on_press(Message::Open)
                .style(flat_tool_style)
                .width(Fill),
            button(i18n::tr("+  Open scan folder…"))
                .on_press(Message::OpenFolder)
                .style(flat_tool_style)
                .width(Fill),
        ]
        .spacing(9);
        let mut layers = column![].spacing(2);
        for index in self.layer_order() {
            let entry = &self.clouds[index];
            let name = display_name(&entry.cloud.path);
            let readable_name = name.replace('_', "_\u{200b}");
            let remaining = entry.remaining_count();
            let file_button = tooltip(
                button(
                    text(readable_name)
                        .size(12)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .width(Fill),
                )
                .on_press(Message::LayerClick(index))
                .style(flat_tool_style)
                .width(Fill)
                .padding([2, 2]),
                container(
                    text(format!(
                        "{}\n{} points",
                        entry.cloud.path.display(),
                        format_count(remaining)
                    ))
                    .size(11),
                )
                .padding([4, 7])
                .style(|theme| {
                    let colors = ui_theme::colors(theme);
                    container::Style::default()
                        .background(colors.panel_alt)
                        .color(colors.text)
                }),
                tooltip::Position::FollowCursor,
            )
            .gap(5);
            let mut item = column![row![
                checkbox("", entry.visible)
                    .on_toggle(move |value| Message::LayerVisible(index, value))
                    .style(muted_checkbox_style)
                    .size(14),
                file_button,
                text(compact_count(remaining))
                    .size(10)
                    .color(self.ui_theme.colors().muted),
                button(text("×").size(12))
                    .on_press(Message::LayerRemove(index))
                    .style(flat_tool_style)
                    .padding([1, 5]),
            ]
            .spacing(3)
            .align_y(iced::Alignment::Center)]
            .spacing(1);
            // Only what needs attention gets a second line.
            let progress = self.layer_progress(entry);
            let mut notes: Vec<String> = progress.iter().map(|(note, _)| note.clone()).collect();
            let selected = entry.selection.as_ref().map_or(0, |mask| mask.count);
            if selected > 0 {
                notes.push(format!("{} selected", format_count(selected)));
            }
            let deleted = entry.deleted_count();
            if deleted > 0 {
                notes.push(format!("{} deleted", format_count(deleted)));
            }
            if !notes.is_empty() {
                item = item.push(
                    container(
                        text(notes.join("  ·  "))
                            .size(10)
                            .color(self.ui_theme.colors().muted),
                    )
                    .padding(iced::Padding {
                        left: 20.0,
                        ..iced::Padding::ZERO
                    }),
                );
            }
            if let Some(fraction) = progress.and_then(|(_, fraction)| fraction) {
                item = item.push(
                    container(
                        iced::widget::progress_bar(0.0..=1.0, fraction)
                            .height(2)
                            .style(|theme| {
                                let colors = ui_theme::colors(theme);
                                iced::widget::progress_bar::Style {
                                    background: colors.border.into(),
                                    bar: colors.accent.into(),
                                    border: iced::Border::default(),
                                }
                            }),
                    )
                    .padding(iced::Padding {
                        left: 20.0,
                        right: 4.0,
                        ..iced::Padding::ZERO
                    }),
                );
            }
            if entry.mesh.is_some() {
                item = item.push(
                    checkbox(i18n::tr("Surface"), entry.mesh_visible)
                        .on_toggle(move |value| Message::SetMeshVisible(index, value))
                        .style(muted_checkbox_style)
                        .text_size(11)
                        .size(12),
                );
            }
            if let Some(switch) = faces::layer_switch(index, entry) {
                item = item.push(switch);
            }
            let active = self.active == Some(index);
            let picked = entry.picked;
            layers = layers.push(
                container(item)
                    .padding([1, 4])
                    .width(Fill)
                    .style(move |theme| {
                        let colors = ui_theme::colors(theme);
                        container::Style::default()
                            .background(if active || picked {
                                colors.panel_alt
                            } else {
                                colors.panel
                            })
                            .border(iced::Border {
                                color: if active {
                                    colors.accent
                                } else {
                                    Color::TRANSPARENT
                                },
                                width: 1.0,
                                radius: 2.0.into(),
                            })
                    }),
            );
        }
        files = files.push(layers);
        // Classes that occur in the open clouds, each shown or hidden like a layer.
        let classes = self.class_codes();
        if !classes.is_empty() {
            let mut list = column![text(i18n::tr("CLASSES"))
                .size(11)
                .color(self.ui_theme.colors().muted)]
            .spacing(3);
            for code in classes {
                let label = ASPRS_CLASSIFICATIONS
                    .iter()
                    .find(|(known, _)| *known == code)
                    .map_or_else(
                        || format!("{code:02}  {} {code}", i18n::tr("Class")),
                        |(_, label)| format!("{code:02}  {}", i18n::tr(label)),
                    );
                list = list.push(
                    checkbox(label, self.class_visibility.allows(Some(code)))
                        .on_toggle(move |visible| Message::FilterClass(code, visible))
                        .style(muted_checkbox_style)
                        .text_size(11)
                        .size(13),
                );
            }
            files = files.push(list);
        }
        container(scrollable(files.padding(14)).height(Fill))
            .width(255)
            .height(Fill)
            .style(sidebar_style)
            .into()
    }

    fn point_viewport(&self) -> PointViewport<'_> {
        PointViewport {
            clouds: &self.clouds,
            loading_status: (!self.imports.is_empty()).then_some(self.status.as_str()),
            color_mode: self.color_mode,
            point_size: self.point_size,
            eye_dome: self.eye_dome,
            eye_dome_strength: self.eye_dome_strength,
            show_scan_poses: self.show_scan_poses,
            budget: self.budget as usize,
            lod_pace: &self.lod_pace,
            filter_ground: self.filter_ground,
            filter_vegetation: self.filter_vegetation,
            filter_buildings: self.filter_buildings,
            filter_other: self.filter_other,
            class_visibility: self.class_visibility,
            section: self.section_box(),
            section_reference: self.section_reference_bounds,
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
            pan: self.pan,
            orbit_point: self.orbit_point,
            box_select: self.box_select,
            pick_mode: self.pick_mode,
            measure: &self.measure,
            drawing: self.drawing.overlay(),
            drawing_slab: self.drawing_slab(),
            annotate: self.views_overlay(),
            drag_rectangle: self.drag_rectangle,
            context_menu: self.context_menu,
            viewport_size: self.viewport_size,
            photo_atlas: self.photo_atlas.as_ref(),
            walk: self.walk,
            walk_station: self.walk_station.filter(|(cloud, station)| {
                self.walk.is_some()
                    && self
                        .clouds
                        .get(*cloud)
                        .is_some_and(|entry| *station < entry.cloud.scan_poses.len())
            }),
            panorama_photos: self.panorama_photos.as_ref(),
        }
    }

    /// The title of the window: the application with its version, after the
    /// file name of the active scan.
    fn window_title(&self) -> String {
        let active = self.active.and_then(|index| self.clouds.get(index));
        title_for(active.map(|entry| display_name(&entry.cloud.path)))
    }

    /// The bar along the bottom of the window: what is going on at the left,
    /// the totals beside it and the version of the application at the right.
    fn status_bar(&self, message: String) -> Element<'_, Message> {
        let total_points: u64 = self.clouds.iter().map(CloudEntry::remaining_count).sum();
        let mut details = row![
            text(message).size(11),
            text(format!(
                "{} files  ·  {} points  ·  {} selected",
                self.clouds.len(),
                format_count(total_points),
                format_count(self.selected_total())
            ))
            .size(11),
        ]
        .spacing(24)
        .align_y(iced::Alignment::Center)
        .width(Fill);
        if let Some((&id, job)) = self.imports.iter().max_by_key(|(id, _)| *id) {
            details = details.push(
                button(i18n::tr("Cancel import"))
                    .on_press_maybe(
                        (!job.cancel.load(Ordering::Relaxed)).then_some(Message::CancelImport(id)),
                    )
                    .style(flat_tool_style),
            );
        }
        // The version is measured first and the rest fills what is left, so
        // a long message cannot push the version out of the window.
        let status_bar = row![
            details,
            text(VERSION_LABEL)
                .size(11)
                .color(self.ui_theme.colors().muted),
        ]
        .spacing(24)
        .padding([7, 12])
        .align_y(iced::Alignment::Center);
        container(status_bar).width(Fill).style(status_style).into()
    }

    /// What the header of the model space shows beside its name: the view,
    /// or that a box is being drawn. The label of the view stays English in
    /// the state, since the command API reports it, and is translated here.
    fn view_caption(&self) -> &'static str {
        i18n::tr(if self.box_select {
            i18n::key("BOX SELECT ACTIVE")
        } else {
            self.view_label
        })
    }

    fn view(&self) -> Element<'_, Message> {
        if self.file_open {
            return column![
                self.ribbon(),
                self.file_view(),
                self.status_bar(self.status.clone()),
            ]
            .height(Fill)
            .into();
        }
        let point_view = self.point_viewport();
        let canvas = stack![
            gpu_viewport::GpuViewport {
                overlay: point_view
            }
            .widget()
            .width(Fill)
            .height(Fill),
            Canvas::new(point_view).width(Fill).height(Fill),
        ]
        .width(Fill)
        .height(Fill);
        let canvas = if self.walk.is_some() {
            canvas.push(
                container(
                    button(text(i18n::tr("Back to 3D view (Esc)")).size(12))
                        .on_press(Message::LeaveWalk)
                        .style(|_, status| button::Style {
                            // Readable over any photo or point cloud.
                            background: Some(
                                if matches!(
                                    status,
                                    button::Status::Hovered | button::Status::Pressed
                                ) {
                                    Color::from_rgb8(217, 119, 6)
                                } else {
                                    Color::from_rgba8(42, 42, 50, 0.9)
                                }
                                .into(),
                            ),
                            text_color: Color::from_rgb8(245, 245, 244),
                            border: iced::Border {
                                radius: 6.0.into(),
                                color: Color::from_rgb8(245, 158, 11),
                                width: 1.0,
                            },
                            ..button::Style::default()
                        }),
                )
                .padding(10),
            )
        } else {
            canvas
        };
        let canvas = match self.note_prompt() {
            Some(prompt) => canvas.push(prompt),
            None => canvas,
        };

        let active_cloud = self.active.and_then(|index| self.clouds.get(index));
        let source_points = active_cloud.map_or(0, |entry| entry.cloud.total_points);
        let view_points = active_cloud.map_or(0, CloudEntry::view_len);
        let selected_points = active_cloud
            .and_then(|entry| entry.selection.as_ref())
            .map_or(0, |selection| selection.count);
        let indexed = active_cloud.is_some_and(|entry| entry.index.is_some());
        let filename = active_cloud.map_or(i18n::tr("No file loaded"), |entry| {
            display_name(&entry.cloud.path)
        });
        let mut properties = column![
            container(
                text(i18n::tr("Properties"))
                    .size(12)
                    .font(Font::with_name("Space Grotesk"))
            )
            .padding([5, 8])
            .width(Fill),
            container(text(filename).size(11))
                .padding([5, 8])
                .width(Fill)
                .style(|theme| container::Style::default()
                    .background(ui_theme::colors(theme).panel_alt)),
        ]
        .spacing(0)
        .width(270);
        // The block of the Section drawing tool comes first: it is opened
        // from the ribbon and stands in view without scrolling.
        if let Some(drawing) = self.drawing_properties() {
            properties = properties.push(drawing);
        }
        // So does the block of the Closed mesh tool.
        if let Some(closed_mesh) = self.closed_mesh_properties() {
            properties = properties.push(closed_mesh);
        }
        // And the block of the Detect faces tool, with the faces it found.
        if let Some(faces) = self.faces_properties() {
            properties = properties.push(faces);
        }
        for row in [
            opencad_properties::section_header("General"),
            opencad_properties::property_row("Source points", format_count(source_points)),
            opencad_properties::property_row(
                "Remaining",
                format_count(active_cloud.map_or(0, CloudEntry::remaining_count)),
            ),
            opencad_properties::property_row(
                "Deleted",
                format_count(active_cloud.map_or(0, CloudEntry::deleted_count)),
            ),
            opencad_properties::property_row("View sample", format_count(view_points)),
            opencad_properties::property_row(
                "Indexed",
                if indexed {
                    i18n::tr("Yes")
                } else {
                    i18n::tr("No")
                }
                .into(),
            ),
            opencad_properties::property_row("Selected", format_count(selected_points)),
        ] {
            properties = properties.push(row);
        }
        if active_cloud.is_some() {
            properties = properties
                .push(opencad_properties::section_header("3D surface settings"))
                .push(opencad_properties::property_input(
                    "Max vertices",
                    "50000",
                    &self.surface_settings[0],
                    |value| Message::SurfaceSetting(0, value),
                ))
                .push(opencad_properties::property_input(
                    "Neighbors",
                    "12",
                    &self.surface_settings[1],
                    |value| Message::SurfaceSetting(1, value),
                ))
                .push(opencad_properties::property_input(
                    "Edge factor",
                    "4",
                    &self.surface_settings[2],
                    |value| Message::SurfaceSetting(2, value),
                ));
        }
        properties = properties.push(opencad_properties::section_header("Geometry"));
        if let Some(job) = &self.mesh_job {
            let progress = job.control.snapshot();
            properties = properties
                .push(opencad_properties::section_header("Mesh progress"))
                .push(container(text(job.progress_text()).size(11)).padding([6, 8]))
                .push(
                    container(
                        iced::widget::progress_bar(
                            0.0..=1.0,
                            if progress.total == 0 {
                                0.0
                            } else {
                                progress.completed as f32 / progress.total as f32
                            },
                        )
                        .height(8)
                        .style(|theme| {
                            let colors = ui_theme::colors(theme);
                            iced::widget::progress_bar::Style {
                                background: colors.panel_alt.into(),
                                bar: colors.accent.into(),
                                border: iced::Border::default(),
                            }
                        }),
                    )
                    .padding([2, 8])
                    .width(Fill),
                )
                .push(
                    container(button(i18n::tr("Cancel mesh")).on_press(Message::CancelMesh))
                        .padding([5, 8]),
                );
        }
        if let Some(job) = &self.merge_job {
            let processed = job.control.processed.load(Ordering::Relaxed);
            properties = properties
                .push(opencad_properties::section_header("Merge progress"))
                .push(container(text(job.progress_text()).size(11)).padding([6, 8]))
                .push(
                    container(
                        iced::widget::progress_bar(
                            0.0..=1.0,
                            processed as f32 / job.control.total.max(1) as f32,
                        )
                        .height(8),
                    )
                    .padding([2, 8])
                    .width(Fill),
                )
                .push(
                    container(button(i18n::tr("Cancel merge")).on_press(Message::CancelMerge))
                        .padding([5, 8]),
                );
        }
        if let Some(job) = &self.scale_job {
            let completed = job.progress.load(Ordering::Relaxed);
            let total = job.source.total_points;
            properties = properties
                .push(opencad_properties::section_header("Scale centroid"))
                .push(
                    container(text(format!("{completed} / {total} source points")).size(11))
                        .padding([6, 8]),
                )
                .push(
                    container(
                        iced::widget::progress_bar(
                            0.0..=1.0,
                            if total == 0 {
                                0.0
                            } else {
                                completed as f32 / total as f32
                            },
                        )
                        .height(8)
                        .style(|theme| {
                            let colors = ui_theme::colors(theme);
                            iced::widget::progress_bar::Style {
                                background: colors.panel_alt.into(),
                                bar: colors.accent.into(),
                                border: iced::Border::default(),
                            }
                        }),
                    )
                    .padding([2, 8])
                    .width(Fill),
                )
                .push(
                    container(button(i18n::tr("Cancel scale")).on_press(Message::CancelScale))
                        .padding([5, 8]),
                );
        }
        if let Some(entry) = active_cloud {
            for (axis, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                properties = properties.push(opencad_properties::bounds_row(
                    label,
                    entry.bounds().min[axis],
                    entry.bounds().max[axis],
                ));
            }
            if !entry.transform.is_identity() {
                let source_center = entry.cloud.bounds.center();
                let edited_center = entry.bounds().center();
                properties = properties
                    .push(opencad_properties::section_header("Live transform"))
                    .push(opencad_properties::property_row(
                        "Scale XYZ",
                        format!(
                            "{:.3}, {:.3}, {:.3}",
                            entry.transform.scale[0],
                            entry.transform.scale[1],
                            entry.transform.scale[2]
                        ),
                    ))
                    .push(opencad_properties::property_row(
                        "Centre shift",
                        format!(
                            "{:.3}, {:.3}, {:.3}",
                            edited_center[0] - source_center[0],
                            edited_center[1] - source_center[1],
                            edited_center[2] - source_center[2]
                        ),
                    ))
                    .push(
                        container(
                            button(i18n::tr("Reset transform"))
                                .on_press(Message::ResetTransform)
                                .style(flat_tool_style),
                        )
                        .padding([3, 8]),
                    );
            }
            if let Some(point) = entry
                .selection
                .as_deref()
                .filter(|selection| selection.count == 1)
                .and_then(|selection| selection.highlights.first())
            {
                let point = if entry
                    .selection
                    .as_ref()
                    .is_some_and(|selection| selection.highlights_source)
                {
                    entry.transform.point(*point)
                } else {
                    *point
                };
                properties = properties.push(opencad_properties::section_header("Selected point"));
                for (axis, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                    properties = properties.push(opencad_properties::property_row(
                        label,
                        format!("{:.3}", point.xyz[axis]),
                    ));
                }
                if let Some(rgb) = point.rgb {
                    properties = properties.push(opencad_properties::property_row(
                        "RGB",
                        format!("{}, {}, {}", rgb[0], rgb[1], rgb[2]),
                    ));
                }
                if let Some(intensity) = point.intensity {
                    properties = properties.push(opencad_properties::property_row(
                        "Intensity",
                        intensity.to_string(),
                    ));
                }
                if let Some(classification) = point.classification {
                    properties = properties.push(opencad_properties::property_row(
                        "Class",
                        classification.to_string(),
                    ));
                }
            }
            if !entry.cloud.scan_poses.is_empty() {
                properties = properties
                    .push(opencad_properties::section_header("Scan positions"))
                    .push(opencad_properties::property_row(
                        "Stations",
                        entry.cloud.scan_poses.len().to_string(),
                    ))
                    .push(opencad_properties::property_row(
                        "Station photos",
                        entry.cloud.scan_images.len().to_string(),
                    ))
                    .push(
                        container(
                            button(if self.expand_scan_poses {
                                i18n::tr("Hide list")
                            } else {
                                i18n::tr("Show list")
                            })
                            .on_press(Message::ExpandScanPoses(!self.expand_scan_poses))
                            .style(flat_tool_style),
                        )
                        .padding([4, 8]),
                    );
                if self.expand_scan_poses {
                    for (pose_index, pose) in entry.cloud.scan_poses.iter().enumerate() {
                        properties = properties.push(
                            container(
                                column![
                                    row![
                                        text(pose.label.as_str()).size(11).width(Fill),
                                        button(i18n::tr("Photo"))
                                            .on_press_maybe(
                                                self.active
                                                    .filter(|_| {
                                                        entry.cloud.scan_images.iter().any(
                                                            |image| {
                                                                image.station == Some(pose_index)
                                                            },
                                                        )
                                                    })
                                                    .map(|cloud_index| {
                                                        Message::EnterPanorama(
                                                            cloud_index,
                                                            pose_index,
                                                        )
                                                    }),
                                            )
                                            .style(flat_tool_style),
                                        button(i18n::tr("Center"))
                                            .on_press_maybe(self.active.map(|cloud_index| {
                                                Message::CenterScanPose(cloud_index, pose_index)
                                            }))
                                            .style(flat_tool_style),
                                    ]
                                    .align_y(iced::Alignment::Center),
                                    text(format!(
                                        "{:.3}, {:.3}, {:.3}",
                                        entry.transform.xyz(pose.position)[0],
                                        entry.transform.xyz(pose.position)[1],
                                        entry.transform.xyz(pose.position)[2]
                                    ))
                                    .size(10),
                                    text(match entry.transform.axes(pose.axes) {
                                        Some(axes) => format!(
                                            "X {:+.2} {:+.2} {:+.2}\nY {:+.2} {:+.2} {:+.2}\nZ {:+.2} {:+.2} {:+.2}",
                                            axes[0][0], axes[0][1], axes[0][2],
                                            axes[1][0], axes[1][1], axes[1][2],
                                            axes[2][0], axes[2][1], axes[2][2],
                                        ),
                                        None => i18n::tr("Orientation unavailable").into(),
                                    })
                                    .size(9),
                                ]
                                .spacing(2),
                            )
                            .padding([4, 8]),
                        );
                    }
                }
            }
        }
        if let Some(section) = self.measure.properties() {
            properties = properties.push(section);
        }
        properties = properties.push(self.views_properties());
        if self.section_enabled {
            properties = properties.push(opencad_properties::section_header("Section box"));
            for (axis, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                properties = properties
                    .push(
                        container(
                            row![
                                text(format!(
                                    "{label} min {:.0}%",
                                    self.section_min_percent[axis]
                                ))
                                .size(10)
                                .width(78),
                                slider(
                                    0.0..=100.0,
                                    self.section_min_percent[axis] as f32,
                                    move |value| { Message::SectionMin(axis, value) }
                                )
                                .width(156),
                            ]
                            .spacing(6)
                            .align_y(iced::Alignment::Center),
                        )
                        .padding([2, 8]),
                    )
                    .push(
                        container(
                            row![
                                text(format!(
                                    "{label} max {:.0}%",
                                    self.section_max_percent[axis]
                                ))
                                .size(10)
                                .width(78),
                                slider(
                                    0.0..=100.0,
                                    self.section_max_percent[axis] as f32,
                                    move |value| { Message::SectionMax(axis, value) }
                                )
                                .width(156),
                            ]
                            .spacing(6)
                            .align_y(iced::Alignment::Center),
                        )
                        .padding([2, 8]),
                    );
            }
            properties = properties.push(
                container(text(i18n::tr("XYZ limits · model coordinates")).size(10))
                    .padding([7, 8]),
            );
            for (axis, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                properties = properties.push(
                    container(
                        row![
                            text(label).size(11).width(15),
                            text_input(i18n::tr("Min"), &self.section_coordinate_inputs[axis][0])
                                .on_input(move |value| Message::SectionCoordinate(
                                    axis, true, value
                                ))
                                .size(11)
                                .width(Fill),
                            text_input(i18n::tr("Max"), &self.section_coordinate_inputs[axis][1])
                                .on_input(move |value| Message::SectionCoordinate(
                                    axis, false, value
                                ))
                                .size(11)
                                .width(Fill),
                        ]
                        .spacing(4)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([2, 8]),
                );
            }
            properties = properties.push(
                container(
                    row![
                        text(i18n::tr("Rotation (°)")).size(11).width(78),
                        text_input("0", &self.section_rotation_input)
                            .on_input(Message::SectionRotationInput)
                            .on_submit(Message::ApplySectionRotation)
                            .size(11)
                            .width(Fill),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                )
                .padding([2, 8]),
            );
            if self.section_rotation != 0.0 {
                properties = properties.push(
                    container(
                        text(i18n::tr(
                            "The limits are those of the box before it is turned about its centre.",
                        ))
                        .size(10)
                        .color(self.ui_theme.colors().muted),
                    )
                    .padding([2, 8]),
                );
            }
            properties = properties.push(
                container(
                    row![
                        button(i18n::tr("Apply XYZ limits"))
                            .on_press(Message::ApplySectionCoordinates)
                            .style(flat_tool_style),
                        button(i18n::tr("Zoom box"))
                            .on_press(Message::ZoomToSection)
                            .style(flat_tool_style),
                    ]
                    .spacing(3),
                )
                .padding([3, 8]),
            );
            properties = properties.push(
                container(
                    button(i18n::tr(if self.section_align_pending {
                        i18n::key("Looking for walls…")
                    } else {
                        i18n::key("Align to walls")
                    }))
                    .on_press_maybe(
                        (!self.section_align_pending).then_some(Message::AlignSectionToWalls),
                    )
                    .style(flat_tool_style),
                )
                .padding([3, 8]),
            );
        }
        if let Some(mesh) = self.mesh_properties() {
            properties = properties.push(mesh);
        }
        // The ribbon switches eye-dome lighting; its strength is set here.
        if self.eye_dome {
            properties = properties
                .push(opencad_properties::section_header("Eye-dome lighting"))
                .push(
                    container(
                        row![
                            text(i18n::tr("Strength")).size(11).width(52),
                            slider(0.0..=5.0, self.eye_dome_strength, Message::EyeDomeStrength,)
                                .step(0.1_f32)
                                .width(155),
                            text(format!("{:.1}", self.eye_dome_strength)).size(11),
                        ]
                        .spacing(5)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([3, 8]),
                );
        }
        let properties: Element<'_, Message> = if self.bag_panel {
            self.bag_panel_view()
        } else {
            properties.into()
        };

        let viewport_header = row![
            text(i18n::tr("MODEL SPACE"))
                .size(12)
                .font(Font::with_name("Space Grotesk"))
                .color(Color::from_rgb8(250, 250, 249)),
            text(self.view_caption())
                .size(11)
                .color(Color::from_rgb8(161, 161, 170)),
        ]
        .spacing(16)
        .padding([8, 14]);
        let mut viewport = column![viewport_header].height(Fill).width(Fill);
        if let Some(progress) = self.progress_strip() {
            viewport = viewport.push(progress);
        }
        viewport = viewport.push(container(canvas).width(Fill).height(Fill));
        if self
            .clouds
            .iter()
            .any(|entry| entry.bag_source && (entry.visible || entry.mesh_visible))
        {
            viewport = viewport.push(
                container(
                    row![
                        text(i18n::tr("© 3DBAG by tudelft3d and 3DGI")).size(10),
                        button(i18n::tr("Source and license ↗"))
                            .on_press(Message::OpenBagLicense)
                            .style(flat_tool_style),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center),
                )
                .width(Fill)
                .align_x(iced::Alignment::End)
                .padding([0, 8]),
            );
        }
        let content = row![
            self.project_panel(),
            container(viewport)
                .width(Fill)
                .height(Fill)
                .style(viewport_style),
            container(scrollable(properties).height(Fill))
                .width(if self.bag_panel { 440 } else { 270 })
                .height(Fill)
                .style(sidebar_style),
        ]
        .height(Fill);
        let mesh_status = self.mesh_job.as_ref().map(MeshJob::progress_text);
        let window = column![
            self.ribbon(),
            content,
            self.status_bar(mesh_status.unwrap_or_else(|| self.status.clone())),
        ]
        .height(Fill);
        match self.settings_view() {
            Some(dialog) => stack![window, dialog].into(),
            None => window.into(),
        }
    }
}

fn cached_index_task(cloud: Arc<PointCloud>) -> Task<Message> {
    Task::perform(
        async move {
            let source = Arc::clone(&cloud);
            let result = tokio::task::spawn_blocking(move || {
                OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())
                    .map(|index| index.map(Arc::new))
                    .map_err(|error| error.to_string())
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
            (source, result)
        },
        |(source, result)| Message::CachedIndexReady(source, result),
    )
}

fn save_task(
    suggested: String,
    format: ExportFormat,
    work: impl FnOnce(PathBuf) -> Result<PathBuf, String> + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            let chosen = rfd::AsyncFileDialog::new()
                .add_filter(format.to_string(), &[format.extension()])
                .set_file_name(suggested)
                .save_file()
                .await?;
            let path = chosen.path().to_path_buf();
            Some(
                tokio::task::spawn_blocking(move || work(path))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result),
            )
        },
        Message::SaveCompleted,
    )
}

/// One labelled axis field in a ribbon row of X, Y and Z values.
fn axis_input<'a>(
    axis: &'static str,
    placeholder: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    row![
        text(axis).size(10).width(8),
        text_input(placeholder, value)
            .on_input(on_input)
            .size(11)
            .padding([2, 4])
            .width(44),
    ]
    .spacing(2)
    .align_y(iced::Alignment::Center)
    .into()
}

fn small_tool_button(
    label: &'static str,
    message: Message,
    active: bool,
) -> Element<'static, Message> {
    small_tool_button_when(label, message, active, true)
}

fn small_tool_button_when(
    label: &'static str,
    message: Message,
    active: bool,
    enabled: bool,
) -> Element<'static, Message> {
    let icon = tool_icon(&message);
    button(
        row![icon_svg(icon, 24.0), text(i18n::tr(label)).size(12),]
            .spacing(6)
            .align_y(iced::Alignment::Center),
    )
    .on_press_maybe(enabled.then_some(message))
    .style(move |theme, status| opencad_ribbon::tool_btn_style(theme, active, status))
    .height(opencad_ribbon::ROW_H)
    .padding([2, 4])
    .into()
}

fn tool_icon(message: &Message) -> ToolIcon {
    match message {
        Message::Open => ToolIcon::Open,
        Message::Export
        | Message::ExportSelection
        | Message::ExportSection
        | Message::ExportMesh => ToolIcon::Export,
        Message::Decimate | Message::Thin => ToolIcon::Decimate,
        Message::MeshRequest(_) => ToolIcon::Mesh,
        Message::ToggleBagPanel => ToolIcon::Building,
        Message::BuildIndex => ToolIcon::Cloud,
        Message::LoadDetail => ToolIcon::Fit,
        Message::RemoveSelection | Message::DeleteSelection => ToolIcon::Clear,
        Message::CancelSelection
        | Message::CancelScale
        | Message::CancelMesh
        | Message::CancelIndex => ToolIcon::Clear,
        Message::SetSectionEnabled(_) | Message::ResetSectionBox => ToolIcon::SectionBox,
        Message::ZoomToSelection => ToolIcon::Fit,
        Message::UndoDelete => ToolIcon::Undo,
        Message::RedoDelete => ToolIcon::Redo,
        Message::ResetCamera => ToolIcon::Fit,
        Message::ZoomToSection => ToolIcon::Fit,
        Message::CameraPreset(preset) => ToolIcon::Camera(*preset),
        Message::Views(action) => action.icon(),
        Message::ApplyTranslation => ToolIcon::Move,
        Message::ApplyScale => ToolIcon::Scale,
        Message::ToggleBoxSelect => ToolIcon::Select,
        Message::TogglePickSelect => ToolIcon::Pick,
        Message::Measure(action) => action.icon(),
        Message::Drawing(_) => ToolIcon::Drawing,
        Message::ClosedMesh(_) => ToolIcon::ClosedMesh,
        Message::Faces(_) => ToolIcon::Faces,
        Message::ClearSelection => ToolIcon::Clear,
        Message::SetEyeDome(_) => ToolIcon::Shading,
        Message::ShowScanPoses(_) => ToolIcon::Pick,
        Message::FitScanPoses => ToolIcon::Fit,
        Message::FitSectionToSelection => ToolIcon::Select,
        _ => ToolIcon::Cloud,
    }
}

fn small_color_button(
    label: &'static str,
    mode: ColorMode,
    current: ColorMode,
) -> Element<'static, Message> {
    button(
        row![
            Canvas::new(ColorModeGlyph(mode, mode == current))
                .width(24)
                .height(24),
            text(i18n::tr(label)).size(12),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::ColorMode(mode))
    .style(move |theme, status| opencad_ribbon::tool_btn_style(theme, mode == current, status))
    .height(opencad_ribbon::ROW_H)
    .padding([2, 6])
    .into()
}

/// Compact, theme-aware line icons for the four point-cloud color modes.
/// These use the same visual scale as the OpenCADStudio ribbon icons while
/// remaining native Iced geometry rather than web assets.
struct ColorModeGlyph(ColorMode, bool);

impl canvas::Program<Message> for ColorModeGlyph {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        // The glyphs are drawn on an 18-unit square.
        frame.scale(bounds.width.min(bounds.height) / 18.0);
        let colors = ui_theme::colors(theme);
        let highlight = if self.1 { colors.accent } else { colors.muted };
        let stroke = canvas::Stroke::default()
            .with_color(colors.text)
            .with_width(1.35);
        let line = |points: &[[f32; 2]]| {
            canvas::Path::new(|path| {
                path.move_to(UiPoint::new(points[0][0], points[0][1]));
                for point in &points[1..] {
                    path.line_to(UiPoint::new(point[0], point[1]));
                }
            })
        };
        match self.0 {
            ColorMode::Rgb => {
                frame.stroke(&canvas::Path::circle(UiPoint::new(9.0, 9.0), 6.5), stroke);
                for (point, color) in [
                    ([6.3, 6.5], colors.muted),
                    ([11.7, 6.5], colors.muted),
                    ([9.0, 11.6], highlight),
                ] {
                    frame.fill(
                        &canvas::Path::circle(UiPoint::new(point[0], point[1]), 1.5),
                        color,
                    );
                }
            }
            ColorMode::Elevation => {
                frame.stroke(
                    &line(&[
                        [2.0, 14.0],
                        [5.7, 9.0],
                        [8.2, 11.0],
                        [11.6, 4.0],
                        [16.0, 14.0],
                    ]),
                    stroke,
                );
                frame.stroke(&line(&[[2.0, 16.0], [16.0, 16.0]]), stroke);
                frame.stroke(&line(&[[10.1, 9.2], [13.1, 9.2]]), stroke);
            }
            ColorMode::Intensity => {
                frame.fill(
                    &canvas::Path::new(|path| {
                        path.move_to(UiPoint::new(9.0, 2.5));
                        for point in [
                            [5.7, 3.5],
                            [3.5, 5.7],
                            [2.5, 9.0],
                            [3.5, 12.3],
                            [5.7, 14.5],
                            [9.0, 15.5],
                        ] {
                            path.line_to(UiPoint::new(point[0], point[1]));
                        }
                        path.close();
                    }),
                    colors.muted,
                );
                frame.stroke(&canvas::Path::circle(UiPoint::new(9.0, 9.0), 6.5), stroke);
                frame.stroke(&line(&[[9.0, 2.5], [9.0, 15.5]]), stroke);
            }
            ColorMode::Classification => {
                for (origin, filled) in [
                    ([2.5, 2.5], false),
                    ([10.0, 2.5], false),
                    ([2.5, 10.0], false),
                    ([10.0, 10.0], true),
                ] {
                    if filled {
                        frame.fill_rectangle(
                            UiPoint::new(origin[0], origin[1]),
                            Size::new(5.5, 5.5),
                            highlight,
                        );
                    } else {
                        frame.stroke_rectangle(
                            UiPoint::new(origin[0], origin[1]),
                            Size::new(5.5, 5.5),
                            stroke,
                        );
                    }
                }
            }
        }
        vec![frame.into_geometry()]
    }
}

fn ribbon_group<'a>(label: &'static str, contents: Element<'a, Message>) -> Element<'a, Message> {
    opencad_ribbon::render_group(label, contents)
}

fn ribbon_style(theme: &Theme) -> container::Style {
    container::Style::default().background(ui_theme::colors(theme).shell)
}

#[cfg(test)]
mod ribbon_tests {
    use super::*;

    #[test]
    fn file_button_opens_the_file_view_and_escape_closes_it() {
        let mut studio = Studio::default();
        let _ = studio.update(Message::ToggleFile);
        assert!(studio.file_open);
        let _ = studio.view();

        let _ = studio.update(Message::Escape);
        assert!(!studio.file_open);
        let _ = studio.view();
    }

    #[test]
    fn file_view_exports_close_the_view_and_need_a_scan() {
        for action in [
            FileAction::ExportWithoutSelection,
            FileAction::ExportDecimated,
        ] {
            let mut studio = Studio::default();
            let _ = studio.update(Message::ToggleFile);
            let _ = studio.update(Message::FileAction(action));
            assert!(!studio.file_open);
            assert!(studio.api_jobs.is_empty());
        }
    }

    #[test]
    fn ribbon_overflow_is_measured_again_after_a_resize() {
        let mut studio = Studio::default();
        let _ = studio.update(Message::RibbonViewport(0.0, 900.0, 1400.0));
        assert_eq!(studio.ribbon_viewport, Some((0.0, 900.0, 1400.0)));
        let _ = studio.view();

        let _ = studio.update(Message::RibbonReset);
        assert_eq!(studio.ribbon_viewport, None);
    }

    #[test]
    fn ribbon_builds_with_a_scan_in_every_tool_mode() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.xyz");
        std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));

        for message in [
            Message::ToggleBoxSelect,
            Message::TogglePickSelect,
            Message::Measure(measure::MeasureAction::Toggle(
                measure::MeasureMode::Distance,
            )),
            Message::SetSectionEnabled(true),
            Message::CameraPreset(CameraPreset::Top),
        ] {
            let _ = studio.update(message);
            assert!(!studio.file_open);
            let _ = studio.view();
        }
        assert_eq!(studio.measure.mode, Some(measure::MeasureMode::Distance));
        assert!(studio.section_enabled);
        assert_eq!(studio.view_label, "TOP");
    }
}

fn sidebar_style(theme: &Theme) -> container::Style {
    let colors = ui_theme::colors(theme);
    container::Style::default()
        .background(colors.panel)
        .border(iced::Border {
            color: colors.border,
            width: 1.0,
            radius: 0.0.into(),
        })
}

fn viewport_style(_: &Theme) -> container::Style {
    container::Style::default().background(Color::from_rgb8(42, 42, 50))
}

fn status_style(theme: &Theme) -> container::Style {
    container::Style::default().background(ui_theme::colors(theme).panel)
}

fn themed_pick_list_style(
    theme: &Theme,
    status: iced::widget::pick_list::Status,
) -> iced::widget::pick_list::Style {
    let colors = ui_theme::colors(theme);
    iced::widget::pick_list::Style {
        text_color: colors.text,
        placeholder_color: colors.muted,
        handle_color: colors.muted,
        background: iced::Background::Color(colors.panel_alt),
        border: iced::Border {
            color: if matches!(status, iced::widget::pick_list::Status::Opened) {
                colors.accent
            } else {
                colors.border
            },
            width: 1.0,
            radius: 2.0.into(),
        },
    }
}

fn flat_tool_style(theme: &Theme, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(iced::Background::Color(colors.hover)),
        text_color: colors.text,
        ..button::Style::default()
    }
}

fn muted_checkbox_style(theme: &Theme, status: checkbox::Status) -> checkbox::Style {
    let colors = ui_theme::colors(theme);
    let checked = match status {
        checkbox::Status::Active { is_checked }
        | checkbox::Status::Hovered { is_checked }
        | checkbox::Status::Disabled { is_checked } => is_checked,
    };
    checkbox::Style {
        background: iced::Background::Color(if checked {
            colors.accent
        } else {
            colors.panel_alt
        }),
        icon_color: if colors.shell == Color::BLACK {
            Color::BLACK
        } else {
            Color::WHITE
        },
        border: iced::Border {
            color: colors.border,
            width: 1.0,
            radius: 2.0.into(),
        },
        text_color: Some(colors.text),
    }
}

#[derive(Debug, Clone, Copy)]
enum ToolIcon {
    Open,
    OpenFolder,
    Export,
    Fit,
    Cloud,
    Select,
    SectionBox,
    Pick,
    MeasureDistance,
    MeasureArea,
    Note,
    Line,
    Clear,
    Undo,
    Redo,
    Camera(CameraPreset),
    Save,
    Move,
    Scale,
    Decimate,
    Mesh,
    Building,
    Shading,
    Drawing,
    ClosedMesh,
    Faces,
}

// SVG artwork is copied from OpenCADStudio/assets/icons at commit 1fec34d.
fn icon_svg(icon: ToolIcon, size: f32) -> Element<'static, Message> {
    let bytes: &'static [u8] = match icon {
        ToolIcon::Open => include_bytes!("../../assets/opencad-icons/folder_open.svg"),
        // The scan-folder and section-box icons are drawn for this app in the
        // same palette.
        ToolIcon::OpenFolder => include_bytes!("../../assets/opencad-icons/folder_scans.svg"),
        ToolIcon::Export => include_bytes!("../../assets/opencad-icons/file_export.svg"),
        ToolIcon::Fit => include_bytes!("../../assets/opencad-icons/zoom_ext.svg"),
        ToolIcon::Cloud => include_bytes!("../../assets/opencad-icons/revcloud.svg"),
        ToolIcon::Select => include_bytes!("../../assets/opencad-icons/select_objects.svg"),
        ToolIcon::SectionBox => include_bytes!("../../assets/opencad-icons/section_box.svg"),
        ToolIcon::Pick => include_bytes!("../../assets/opencad-icons/pick_point.svg"),
        // The two measure icons are drawn for this app in the same palette.
        ToolIcon::MeasureDistance => {
            include_bytes!("../../assets/opencad-icons/measure_distance.svg")
        }
        ToolIcon::MeasureArea => include_bytes!("../../assets/opencad-icons/measure_area.svg"),
        // So are the two annotation icons.
        ToolIcon::Note => include_bytes!("../../assets/opencad-icons/annotation_note.svg"),
        ToolIcon::Line => include_bytes!("../../assets/opencad-icons/annotation_line.svg"),
        ToolIcon::Clear => include_bytes!("../../assets/opencad-icons/xclip_remove.svg"),
        ToolIcon::Undo => include_bytes!("../../assets/opencad-icons/undo.svg"),
        ToolIcon::Redo => include_bytes!("../../assets/opencad-icons/redo.svg"),
        ToolIcon::Camera(CameraPreset::Top | CameraPreset::Bottom) => {
            include_bytes!("../../assets/opencad-icons/view_top.svg")
        }
        ToolIcon::Camera(CameraPreset::Front | CameraPreset::Back) => {
            include_bytes!("../../assets/opencad-icons/view_front.svg")
        }
        ToolIcon::Camera(CameraPreset::Right | CameraPreset::Left) => {
            include_bytes!("../../assets/opencad-icons/view_right.svg")
        }
        ToolIcon::Camera(CameraPreset::Isometric) => {
            include_bytes!("../../assets/opencad-icons/view_iso.svg")
        }
        ToolIcon::Save => include_bytes!("../../assets/opencad-icons/save.svg"),
        ToolIcon::Move => include_bytes!("../../assets/opencad-icons/move.svg"),
        ToolIcon::Scale => include_bytes!("../../assets/opencad-icons/scale.svg"),
        ToolIcon::Decimate => include_bytes!("../../assets/opencad-icons/point.svg"),
        ToolIcon::Mesh => include_bytes!("../../assets/opencad-icons/region.svg"),
        ToolIcon::Building => include_bytes!("../../assets/opencad-icons/solid.svg"),
        ToolIcon::Shading => include_bytes!("../../assets/opencad-icons/sphere.svg"),
        // The section drawing icon is drawn for this app in the same palette.
        ToolIcon::Drawing => include_bytes!("../../assets/opencad-icons/section_drawing.svg"),
        // So is the closed mesh icon.
        ToolIcon::ClosedMesh => include_bytes!("../../assets/opencad-icons/closed_mesh.svg"),
        // And so is the icon of the Detect faces tool.
        ToolIcon::Faces => include_bytes!("../../assets/opencad-icons/detect_faces.svg"),
    };
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .into()
}

fn combined_bounds(clouds: &[CloudEntry]) -> Option<Bounds> {
    let mut overall: Option<Bounds> = None;
    // Keep a loaded mesh, and detected faces, framed when the source points
    // are hidden for inspection.
    let shown = |entry: &CloudEntry| {
        entry.visible
            || (entry.mesh_visible && entry.mesh.is_some())
            || entry.faces.as_ref().is_some_and(faces::FaceLayer::visible)
    };
    let any_visible = clouds.iter().any(shown);
    for entry in clouds.iter().filter(|entry| shown(entry) || !any_visible) {
        match &mut overall {
            Some(bounds) => {
                for axis in 0..3 {
                    bounds.min[axis] = bounds.min[axis].min(entry.bounds().min[axis]);
                    bounds.max[axis] = bounds.max[axis].max(entry.bounds().max[axis]);
                }
            }
            None => overall = Some(entry.bounds()),
        }
    }
    overall
}

/// Size on screen of an octree node of a placed cloud, as the sampler is
/// told it: `None` when the section box or the camera leaves the node out.
fn lod_node_span(
    transform: CloudTransform,
    section: Option<OrientedBox>,
    projection: Projection,
    node_bounds: Bounds,
) -> Option<f32> {
    let node_bounds = transform.bounds(node_bounds);
    if section.is_some_and(|clip| {
        let clip = clip.aabb();
        (0..3).any(|axis| {
            node_bounds.max[axis] < clip.min[axis] || node_bounds.min[axis] > clip.max[axis]
        })
    }) {
        return None;
    }
    projection.screen_span(node_bounds)
}

/// The part of a box that the section box leaves visible, if any.
fn section_clipped(bounds: Bounds, section: Option<Bounds>) -> Option<Bounds> {
    let Some(section) = section else {
        return Some(bounds);
    };
    let clipped = Bounds {
        min: std::array::from_fn(|axis| bounds.min[axis].max(section.min[axis])),
        max: std::array::from_fn(|axis| bounds.max[axis].min(section.max[axis])),
    };
    if (0..3).any(|axis| clipped.min[axis] > clipped.max[axis]) {
        return None;
    }
    Some(clipped)
}

fn source_lod_coverage(
    projection: Projection,
    bounds: Bounds,
    section: Option<OrientedBox>,
) -> Option<f32> {
    projection.screen_coverage(section_clipped(bounds, section.map(|clip| clip.aabb()))?)
}

/// Reserve a small sample for each visible scan, then share the remaining
/// viewport budget by on-screen coverage. Reassign quota left by small scans.
fn distribute_lod_budget(budget: usize, sources: &[(f32, usize)]) -> Vec<usize> {
    let mut allocated = vec![0usize; sources.len()];
    if sources.is_empty() || budget == 0 {
        return allocated;
    }
    let available = sources
        .iter()
        .fold(0usize, |sum, (_, capacity)| sum.saturating_add(*capacity));
    let mut remaining = budget.min(available);
    let reserve = if remaining >= sources.len() {
        (remaining / sources.len() / 16).clamp(1, 1_024)
    } else {
        0
    };
    for (allocation, (_, capacity)) in allocated.iter_mut().zip(sources) {
        let initial = reserve.min(*capacity).min(remaining);
        *allocation = initial;
        remaining -= initial;
    }
    while remaining > 0 {
        let active: Vec<_> = sources
            .iter()
            .enumerate()
            .filter(|(index, (_, capacity))| allocated[*index] < *capacity)
            .map(|(index, (coverage, _))| {
                (
                    index,
                    f64::from(if coverage.is_finite() {
                        coverage.max(1.0)
                    } else {
                        1.0
                    }),
                )
            })
            .collect();
        if active.is_empty() {
            break;
        }
        let weight_sum: f64 = active.iter().map(|(_, weight)| weight).sum();
        let shares: Vec<_> = active
            .iter()
            .map(|(index, weight)| (*index, remaining as f64 * weight / weight_sum))
            .collect();
        let mut granted = 0usize;
        for (index, share) in &shares {
            let add = (*share as usize).min(sources[*index].1 - allocated[*index]);
            allocated[*index] += add;
            granted += add;
        }
        remaining -= granted;
        if remaining == 0 {
            break;
        }
        let mut remainders = shares;
        remainders.sort_by(|a, b| b.1.fract().total_cmp(&a.1.fract()));
        for (index, _) in remainders {
            if remaining == 0 {
                break;
            }
            if allocated[index] < sources[index].1 {
                allocated[index] += 1;
                remaining -= 1;
                granted += 1;
            }
        }
        if granted == 0 {
            break;
        }
    }
    allocated
}

/// Limits per source for a first pass of `first` points, each kept within
/// what `preview_tier` allows that source at its limit. `None` when that
/// leaves less than a first pass is worth: the view then holds so few leaves
/// that reading them in full once is the shorter way.
fn first_pass_limits(
    first: usize,
    sources: &[(f32, usize)],
    preview_tier: impl Fn(usize, usize) -> Option<usize>,
) -> Option<Vec<usize>> {
    let mut limits = distribute_lod_budget(first, sources);
    for (slot, limit) in limits.iter_mut().enumerate() {
        if let Some(tier) = preview_tier(slot, *limit) {
            *limit = (*limit).min(tier);
        }
    }
    (limits.iter().sum::<usize>() >= lod_pace::LOD_FIRST_PASS_MIN).then_some(limits)
}

fn rebalance_lod_limits(
    budget: usize,
    sources: &[(f32, usize)],
    requested: &[usize],
    returned: &[usize],
) -> Option<Vec<usize>> {
    let unused = budget.saturating_sub(returned.iter().sum());
    if unused == 0 {
        return None;
    }
    let expandable: Vec<_> = sources
        .iter()
        .enumerate()
        .filter(|(index, (_, capacity))| {
            returned[*index] >= requested[*index] && requested[*index] < *capacity
        })
        .map(|(index, (coverage, capacity))| (index, *coverage, capacity - requested[index]))
        .collect();
    if expandable.is_empty() {
        return None;
    }
    let extras = distribute_lod_budget(
        unused,
        &expandable
            .iter()
            .map(|(_, coverage, capacity)| (*coverage, *capacity))
            .collect::<Vec<_>>(),
    );
    let mut next = requested.to_vec();
    for ((index, _, _), extra) in expandable.into_iter().zip(extras) {
        next[index] += extra;
    }
    (next != requested).then_some(next)
}

fn loaded_bounds(clouds: &[CloudEntry]) -> Option<Bounds> {
    let mut overall: Option<Bounds> = None;
    for entry in clouds {
        include_bounds(&mut overall, entry.bounds().min);
        include_bounds(&mut overall, entry.bounds().max);
    }
    overall
}

fn bounds_with_scan_poses(clouds: &[CloudEntry]) -> Option<Bounds> {
    let mut bounds = combined_bounds(clouds);
    let mut has_scan_poses = false;
    for entry in clouds.iter().filter(|entry| entry.visible) {
        for pose in &entry.cloud.scan_poses {
            include_bounds(&mut bounds, entry.transform.xyz(pose.position));
            has_scan_poses = true;
        }
    }
    has_scan_poses.then_some(bounds).flatten()
}

fn camera_to_frame_bounds(
    scene: Bounds,
    focus: Bounds,
    yaw: f32,
    pitch: f32,
    size: Size,
) -> Option<(f32, [f32; 2])> {
    if size.width <= 0.0 || size.height <= 0.0 {
        return None;
    }
    let projection = Projection::new(scene, yaw, pitch, 1.0, [0.0; 2], size.width, size.height);
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for corner in 0..8 {
        let xyz = std::array::from_fn(|axis| {
            if corner & (1 << axis) == 0 {
                focus.min[axis]
            } else {
                focus.max[axis]
            }
        });
        let (x, y, _) = projection.project_unclipped(xyz)?;
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let span_x = (max_x - min_x).max(1.0);
    let span_y = (max_y - min_y).max(1.0);
    let magnification = (size.width * 0.74 / span_x)
        .min(size.height * 0.74 / span_y)
        .clamp(0.000_1, 1_000_000.0);
    let zoom = (1.0 / magnification).clamp(0.000_001, 10_000.0);
    let actual_magnification = 1.0 / zoom;
    let pan = [
        -(0.5 * (min_x + max_x) - size.width * 0.5) * actual_magnification,
        -(0.5 * (min_y + max_y) - size.height * 0.5) * actual_magnification,
    ];
    Some((zoom, pan))
}

fn padded_selection_bounds(scene: Bounds, selected: Bounds) -> Bounds {
    let minimum_span = (scene.extent() * 0.004).clamp(0.5, 50.0);
    let mut focus = selected;
    for axis in 0..3 {
        if focus.max[axis] - focus.min[axis] < minimum_span {
            let center = (focus.min[axis] + focus.max[axis]) * 0.5;
            focus.min[axis] = center - minimum_span * 0.5;
            focus.max[axis] = center + minimum_span * 0.5;
        }
    }
    focus
}

fn pan_to_world(
    scene: Bounds,
    target: [f64; 3],
    yaw: f32,
    pitch: f32,
    zoom: f32,
    size: Size,
) -> Option<[f32; 2]> {
    if size.width <= 0.0 || size.height <= 0.0 {
        return None;
    }
    let projection = Projection::new(scene, yaw, pitch, zoom, [0.0; 2], size.width, size.height);
    let (x, y, _) = projection.project_unclipped(target)?;
    let pan = [size.width * 0.5 - x, size.height * 0.5 - y];
    pan.iter().all(|value| value.is_finite()).then_some(pan)
}

/// Box that holds all but the outermost fiftieth of the points on each side
/// of every axis, judged from an even sample of them.
fn bulk_bounds(points: &[Point]) -> Option<Bounds> {
    const SAMPLES: usize = 20_000;
    if points.len() < 1_000 {
        return None;
    }
    let step = points.len().div_ceil(SAMPLES);
    let mut bounds = Bounds {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    for axis in 0..3 {
        let mut values: Vec<f64> = points
            .iter()
            .step_by(step)
            .map(|point| point.xyz[axis])
            .collect();
        let cut = values.len() / 50;
        let last = values.len() - 1 - cut;
        bounds.min[axis] = *values.select_nth_unstable_by(cut, f64::total_cmp).1;
        bounds.max[axis] = *values.select_nth_unstable_by(last, f64::total_cmp).1;
    }
    Some(bounds)
}

fn include_bounds(bounds: &mut Option<Bounds>, xyz: [f64; 3]) {
    if let Some(bounds) = bounds {
        for (axis, value) in xyz.into_iter().enumerate() {
            bounds.min[axis] = bounds.min[axis].min(value);
            bounds.max[axis] = bounds.max[axis].max(value);
        }
    } else {
        *bounds = Some(Bounds { min: xyz, max: xyz });
    }
}

struct SelectedSource {
    index: usize,
    cloud: Arc<PointCloud>,
    selection: Arc<SelectionMask>,
    deleted: Option<Arc<DeletionMask>>,
    transform: CloudTransform,
}

/// The box around the selected points of every source, and their count. In
/// the frame of a turn when `frame` is given: each point is turned into it
/// with `OrientedBox::to_box`, so the box is the one a turned section box
/// fits the points with.
fn selected_source_bounds(
    sources: &[SelectedSource],
    frame: Option<OrientedBox>,
) -> Result<(Bounds, u64), String> {
    let frame = frame.filter(OrientedBox::is_turned);
    let mut bounds = None;
    let mut count = 0u64;
    for source in sources {
        let cloud = &source.cloud;
        let selection = &source.selection;
        let deleted = &source.deleted;
        cloud.validate_source().map_err(|error| error.to_string())?;
        // The kept bounds of a selection are along the model axes.
        if frame.is_none()
            && deleted
                .as_ref()
                .is_none_or(|mask| !mask.overlaps_selection(selection))
        {
            if let Some(source_bounds) = selection.source_bounds {
                let world_bounds = source.transform.bounds(source_bounds);
                include_bounds(&mut bounds, world_bounds.min);
                include_bounds(&mut bounds, world_bounds.max);
                count += selection.count;
                continue;
            }
        }
        let mut ordinal = 0u64;
        pointcloud_core::visit_points(&cloud.path, &mut |point| {
            if selection.contains(ordinal)
                && deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
            {
                let xyz = source.transform.xyz(point.xyz);
                include_bounds(&mut bounds, frame.map_or(xyz, |frame| frame.to_box(xyz)));
                count += 1;
            }
            ordinal += 1;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        cloud.validate_source().map_err(|error| error.to_string())?;
        if ordinal != cloud.total_points {
            return Err(format!(
                "{} contains {ordinal} points; expected {}",
                cloud.path.display(),
                cloud.total_points
            ));
        }
    }
    bounds
        .map(|bounds| (bounds, count))
        .ok_or_else(|| "the selected points are no longer present in the visible source".into())
}

#[derive(Clone, Copy)]
struct PointViewport<'a> {
    clouds: &'a [CloudEntry],
    loading_status: Option<&'a str>,
    color_mode: ColorMode,
    point_size: f32,
    eye_dome: bool,
    eye_dome_strength: f32,
    show_scan_poses: bool,
    budget: usize,
    lod_pace: &'a LodPace,
    filter_ground: bool,
    filter_vegetation: bool,
    filter_buildings: bool,
    filter_other: bool,
    class_visibility: ClassVisibility,
    section: Option<OrientedBox>,
    section_reference: Option<Bounds>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    orbit_point: Option<[f64; 3]>,
    box_select: bool,
    pick_mode: bool,
    measure: &'a measure::MeasureTool,
    /// The filled cut of a section drawing that is previewed.
    drawing: Option<&'a pointcloud_core::CutPreview>,
    /// The slab the Section drawing tool cuts, while its block is open.
    drawing_slab: Option<OrientedBox>,
    annotate: views::Overlay<'a>,
    drag_rectangle: Option<([f32; 2], [f32; 2])>,
    context_menu: Option<[f32; 2]>,
    viewport_size: Size,
    photo_atlas: Option<&'a Arc<PhotoAtlas>>,
    walk: Option<WalkView>,
    walk_station: Option<(usize, usize)>,
    panorama_photos: Option<&'a Arc<PhotoSet>>,
}

struct ScanMarker {
    x: f32,
    y: f32,
    labels: Vec<String>,
    axes: Option<[[f64; 3]; 3]>,
}

const SCAN_MARKER_GROUP_RADIUS: f32 = 32.0;

fn scan_marker_label(marker: &ScanMarker) -> String {
    if marker.labels.len() == 1 {
        marker.labels[0].clone()
    } else {
        format!("{} stations", marker.labels.len())
    }
}

/// Place the most informative labels first without covering another station.
/// Unplaced labels remain available in the Properties station list.
fn scan_marker_label_positions(markers: &[ScanMarker], viewport: Size) -> Vec<Option<UiPoint>> {
    let mut positions = vec![None; markers.len()];
    let mut placed = Vec::<[f32; 4]>::new();
    let mut order: Vec<usize> = (0..markers.len()).collect();
    order.sort_by_key(|index| std::cmp::Reverse(markers[*index].labels.len()));
    for index in order {
        let marker = &markers[index];
        let width = scan_marker_label(marker).chars().count() as f32 * 6.0 + 2.0;
        let candidates = [
            (marker.x + 13.0, marker.y - 7.0),
            (marker.x - width - 13.0, marker.y - 7.0),
            (marker.x - width * 0.5, marker.y - 24.0),
            (marker.x - width * 0.5, marker.y + 14.0),
        ];
        for (x, y) in candidates {
            let rect = [x, y, x + width, y + 13.0];
            if x < 2.0
                || y < 2.0
                || rect[2] > viewport.width - 2.0
                || rect[3] > viewport.height - 2.0
                || placed.iter().any(|other| {
                    rect[0] < other[2] + 4.0
                        && rect[2] + 4.0 > other[0]
                        && rect[1] < other[3] + 4.0
                        && rect[3] + 4.0 > other[1]
                })
                || markers.iter().enumerate().any(|(other_index, other)| {
                    other_index != index
                        && rect[0] < other.x + 9.0
                        && rect[2] > other.x - 9.0
                        && rect[1] < other.y + 9.0
                        && rect[3] > other.y - 9.0
                })
            {
                continue;
            }
            positions[index] = Some(UiPoint::new(x, y));
            placed.push(rect);
            break;
        }
    }
    positions
}

fn push_scan_marker(
    markers: &mut Vec<ScanMarker>,
    x: f32,
    y: f32,
    label: &str,
    axes: Option<[[f64; 3]; 3]>,
    group: bool,
) {
    if group {
        if let Some(marker) = markers.iter_mut().find(|marker| {
            let dx = marker.x - x;
            let dy = marker.y - y;
            dx * dx + dy * dy <= SCAN_MARKER_GROUP_RADIUS * SCAN_MARKER_GROUP_RADIUS
        }) {
            marker.labels.push(label.to_owned());
            // Nearby stations can have different orientations. Do not draw
            // one station's axes on a marker representing several scans.
            marker.axes = None;
            return;
        }
    }
    markers.push(ScanMarker {
        x,
        y,
        labels: if group {
            vec![label.to_owned()]
        } else {
            Vec::new()
        },
        axes,
    });
}

#[derive(Debug, Clone, Copy)]
enum DragMode {
    Orbit,
    /// Orbit whose horizontal sense is reversed: the view turns with the pointer.
    Turn,
    Pan,
    Select,
    RightPending,
    /// Left press while measuring: a click picks a point, a drag orbits.
    MeasurePending,
    /// Left press with an annotation tool: a click picks a point, a drag orbits.
    AnnotatePending,
    Section(usize, bool),
}

#[derive(Debug, Clone, Copy)]
struct DragState {
    start: UiPoint,
    position: UiPoint,
    mode: DragMode,
}

#[derive(Debug, Clone, Copy, Default)]
struct ViewportState {
    drag: Option<DragState>,
    modifiers: iced::keyboard::Modifiers,
    /// When and where the last left click without a drag was, to tell a
    /// double click.
    last_click: Option<(Instant, UiPoint)>,
}

fn middle_drag_mode(modifiers: iced::keyboard::Modifiers) -> DragMode {
    if modifiers.shift() {
        DragMode::Turn
    } else {
        DragMode::Pan
    }
}

/// A left release in the orbit mode: the second click of a double click
/// asks for the orbit point there. A release after a drag is no click.
fn orbit_click(
    last_click: &mut Option<(Instant, UiPoint)>,
    drag: DragState,
    position: UiPoint,
    now: Instant,
    size: Size,
) -> Option<Message> {
    let moved = (position.x - drag.start.x).hypot(position.y - drag.start.y);
    if !matches!(drag.mode, DragMode::Orbit) || moved > orbit_point::DOUBLE_CLICK_REACH {
        *last_click = None;
        return None;
    }
    if orbit_point::is_double_click(*last_click, now, position) {
        *last_click = None;
        return Some(Message::PickOrbitPoint([position.x, position.y], size));
    }
    *last_click = Some((now, position));
    None
}

fn finish_viewport_drag(
    button: mouse::Button,
    drag: DragState,
    position: UiPoint,
    size: Size,
) -> Option<Message> {
    let total = (position.x - drag.start.x).hypot(position.y - drag.start.y);
    let dx = position.x - drag.position.x;
    let dy = position.y - drag.position.y;
    match (button, drag.mode) {
        (mouse::Button::Right, DragMode::RightPending) if total >= 5.0 => Some(Message::FinishPan(
            position.x - drag.start.x,
            position.y - drag.start.y,
        )),
        (mouse::Button::Right, DragMode::RightPending) => {
            Some(Message::ShowContextMenu([position.x, position.y]))
        }
        (mouse::Button::Middle | mouse::Button::Right, DragMode::Pan) if total > 0.5 => {
            Some(Message::FinishPan(dx, dy))
        }
        (mouse::Button::Left, DragMode::Orbit) if total > 0.5 => Some(Message::FinishOrbit(dx, dy)),
        (mouse::Button::Left, DragMode::MeasurePending | DragMode::AnnotatePending)
            if total >= 5.0 =>
        {
            Some(Message::FinishOrbit(
                position.x - drag.start.x,
                position.y - drag.start.y,
            ))
        }
        (mouse::Button::Left, DragMode::MeasurePending) => Some(Message::Measure(
            measure::MeasureAction::Click([position.x, position.y], size),
        )),
        (mouse::Button::Left, DragMode::AnnotatePending) => Some(Message::Views(
            views::ViewAction::Click([position.x, position.y], size),
        )),
        (mouse::Button::Middle, DragMode::Turn) if total > 0.5 => {
            Some(Message::FinishOrbit(-dx, dy))
        }
        (mouse::Button::Left, DragMode::Select) => Some(Message::BoxSelect {
            start: [drag.start.x, drag.start.y],
            end: [position.x, position.y],
            size,
        }),
        _ => None,
    }
}

const CONTEXT_ACTIONS: [(ContextAction, &str); 6] = [
    (ContextAction::Orbit, "Orbit"),
    (ContextAction::BoxSelect, "Box select"),
    (ContextAction::PickPoint, "Pick point"),
    (ContextAction::SectionBox, "Section box"),
    (ContextAction::FitView, "Zoom all"),
    (ContextAction::ClearSelection, "Clear selection"),
];
const CONTEXT_MENU_HEADER_H: f32 = 22.0;
const CONTEXT_MENU_ROW_H: f32 = 25.0;

fn context_menu_bounds(at: [f32; 2], viewport: Rectangle) -> Rectangle {
    let height = CONTEXT_MENU_HEADER_H + CONTEXT_MENU_ROW_H * CONTEXT_ACTIONS.len() as f32 + 6.0;
    Rectangle::new(
        UiPoint::new(
            at[0].min((viewport.width - 184.0).max(0.0)).max(0.0),
            at[1]
                .min((viewport.height - height - 4.0).max(0.0))
                .max(0.0),
        ),
        Size::new(180.0, height),
    )
}

fn context_action_at(point: UiPoint, at: [f32; 2], viewport: Rectangle) -> Option<ContextAction> {
    let menu = context_menu_bounds(at, viewport);
    if !menu.contains(point) || point.y < menu.y + CONTEXT_MENU_HEADER_H {
        return None;
    }
    let index = ((point.y - menu.y - CONTEXT_MENU_HEADER_H) / CONTEXT_MENU_ROW_H) as usize;
    CONTEXT_ACTIONS.get(index).map(|(action, _)| *action)
}

#[cfg(test)]
mod context_menu_tests {
    use super::*;

    #[test]
    fn visible_menu_rows_match_their_click_targets() {
        let viewport = Rectangle::new(UiPoint::ORIGIN, Size::new(400.0, 300.0));
        let at = [100.0, 50.0];
        let menu = context_menu_bounds(at, viewport);
        for (index, (action, _)) in CONTEXT_ACTIONS.iter().enumerate() {
            let center = UiPoint::new(
                menu.x + 24.0,
                menu.y + CONTEXT_MENU_HEADER_H + (index as f32 + 0.5) * CONTEXT_MENU_ROW_H,
            );
            assert_eq!(context_action_at(center, at, viewport), Some(*action));
        }
        assert_eq!(
            context_action_at(
                UiPoint::new(menu.x + 24.0, menu.y + CONTEXT_MENU_HEADER_H * 0.5),
                at,
                viewport
            ),
            None
        );
    }
}

fn draw_context_menu(
    frame: &mut Frame,
    at: [f32; 2],
    viewport: Rectangle,
    hovered: Option<UiPoint>,
    section_active: bool,
) {
    let menu = context_menu_bounds(at, viewport);
    frame.fill_rectangle(menu.position(), menu.size(), Color::from_rgb8(42, 42, 50));
    frame.stroke_rectangle(
        menu.position(),
        menu.size(),
        canvas::Stroke::default()
            .with_color(Color::from_rgb8(105, 105, 114))
            .with_width(1.0),
    );
    frame.fill_text(canvas::Text {
        content: "VIEWPORT".into(),
        position: UiPoint::new(menu.x + 10.0, menu.y + 5.0),
        size: iced::Pixels(10.0),
        color: Color::from_rgb8(161, 161, 170),
        ..canvas::Text::default()
    });
    let hovered_action = hovered.and_then(|point| context_action_at(point, at, viewport));
    for (index, (action, label)) in CONTEXT_ACTIONS.iter().enumerate() {
        let y = menu.y + CONTEXT_MENU_HEADER_H + index as f32 * CONTEXT_MENU_ROW_H;
        if hovered_action == Some(*action) {
            frame.fill_rectangle(
                UiPoint::new(menu.x + 3.0, y),
                Size::new(menu.width - 6.0, CONTEXT_MENU_ROW_H),
                Color::from_rgb8(78, 63, 47),
            );
        }
        let label = if matches!(action, ContextAction::SectionBox) && section_active {
            "Section box (on)"
        } else {
            *label
        };
        frame.fill_text(canvas::Text {
            content: label.into(),
            position: UiPoint::new(menu.x + 12.0, y + 5.0),
            size: iced::Pixels(12.0),
            color: Color::from_rgb8(241, 241, 240),
            ..canvas::Text::default()
        });
    }
}

/// The handle in the middle of a face of the section box, in the scene. The
/// faces of a turned box are those along its own axes.
fn section_handle_world(section: OrientedBox, axis: usize, is_min: bool) -> [f64; 3] {
    let mut point = section.center();
    point[axis] = if is_min {
        section.bounds.min[axis]
    } else {
        section.bounds.max[axis]
    };
    section.to_scene(point)
}

fn section_handle_at(
    pointer: UiPoint,
    section: OrientedBox,
    projection: Projection,
) -> Option<(usize, bool)> {
    let mut nearest: Option<(usize, bool, f32)> = None;
    for axis in 0..3 {
        for is_min in [true, false] {
            let Some((x, y, _)) =
                projection.project_unclipped(section_handle_world(section, axis, is_min))
            else {
                continue;
            };
            let distance = (pointer.x - x).hypot(pointer.y - y);
            if distance <= 10.0
                && nearest.is_none_or(|(_, _, previous_distance)| distance < previous_distance)
            {
                nearest = Some((axis, is_min, distance));
            }
        }
    }
    nearest.map(|(axis, is_min, _)| (axis, is_min))
}

/// How far a drag moves a handle, in percent of the reference along the own
/// axis of the box that the handle moves on. `overall` is the reference,
/// whose sizes are along the axes of the box.
fn section_handle_delta(
    axis: usize,
    is_min: bool,
    section: OrientedBox,
    overall: Bounds,
    projection: Projection,
    movement: [f32; 2],
) -> Option<f32> {
    let world = section_handle_world(section, axis, is_min);
    let step = (overall.max[axis] - overall.min[axis]) * 0.01;
    let direction = match axis {
        2 => [0.0, 0.0, 1.0],
        _ => section.axes()[axis],
    };
    let shifted: [f64; 3] = std::array::from_fn(|index| world[index] + direction[index] * step);
    let before = projection.project_unclipped(world)?;
    let after = projection.project_unclipped(shifted)?;
    let direction = [after.0 - before.0, after.1 - before.1];
    let squared = direction[0] * direction[0] + direction[1] * direction[1];
    (squared > 0.5).then(|| {
        ((movement[0] * direction[0] + movement[1] * direction[1]) / squared).clamp(-20.0, 20.0)
    })
}

fn scan_pose_at(
    clouds: &[CloudEntry],
    projection: Projection,
    pointer: UiPoint,
    photos: Option<&PhotoAtlas>,
) -> Option<(usize, usize)> {
    let mut nearest: Option<(usize, usize, f32)> = None;
    for (cloud_index, entry) in clouds.iter().enumerate().filter(|(_, entry)| entry.visible) {
        for (pose_index, pose) in entry.cloud.scan_poses.iter().enumerate() {
            let Some((x, y, depth)) = projection.project(entry.transform.xyz(pose.position)) else {
                continue;
            };
            // A station drawn as a photo ball can be clicked anywhere on the ball.
            let reach = photos
                .and_then(|atlas| atlas.slot(&entry.cloud.path, pose_index))
                .map_or(12.0, |_| {
                    station_photos::ball_pixel_radius(projection.scale, depth).max(12.0)
                });
            let distance = (pointer.x - x).hypot(pointer.y - y);
            if distance <= reach
                && nearest.is_none_or(|(_, _, previous_distance)| distance < previous_distance)
            {
                nearest = Some((cloud_index, pose_index, distance));
            }
        }
    }
    nearest.map(|(cloud_index, pose_index, _)| (cloud_index, pose_index))
}

impl canvas::Program<Message> for PointViewport<'_> {
    type State = ViewportState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        if let canvas::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) = event {
            state.modifiers = modifiers;
            return (event::Status::Ignored, None);
        }
        let modifiers = state.modifiers;
        let last_click = &mut state.last_click;
        let state = &mut state.drag;
        if let Some(view) = self.walk {
            return self.update_walk(view, state, event, bounds, cursor);
        }
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(menu) = self.context_menu {
                    *state = None;
                    let action = cursor
                        .position_in(bounds)
                        .and_then(|point| context_action_at(point, menu, bounds));
                    return (
                        event::Status::Captured,
                        Some(action.map_or(Message::DismissContextMenu, Message::ContextAction)),
                    );
                }
                if let Some(target) = cursor
                    .position_in(bounds)
                    .and_then(|point| view_cube::hit(point, bounds, self.yaw, self.pitch))
                {
                    *state = None;
                    let message = match target {
                        view_cube::CubeTarget::Face(preset) => Message::CameraPreset(preset),
                        view_cube::CubeTarget::Edge(edge) => Message::CubeEdge(edge),
                        view_cube::CubeTarget::Corner(corner) => Message::CubeCorner(corner),
                        view_cube::CubeTarget::Home => {
                            Message::CameraPreset(CameraPreset::Isometric)
                        }
                    };
                    return (event::Status::Captured, Some(message));
                }
                if let (Some(section), Some(overall), Some(position)) = (
                    self.section,
                    combined_bounds(self.clouds),
                    cursor.position_in(bounds),
                ) {
                    let projection = Projection::new(
                        overall,
                        self.yaw,
                        self.pitch,
                        self.zoom,
                        self.pan,
                        bounds.width,
                        bounds.height,
                    );
                    if let Some((axis, is_min)) = section_handle_at(position, section, projection) {
                        *state = Some(DragState {
                            start: position,
                            position,
                            mode: DragMode::Section(axis, is_min),
                        });
                        return (event::Status::Captured, None);
                    }
                }
                if self.show_scan_poses
                    && !self.box_select
                    && !self.pick_mode
                    && self.measure.mode.is_none()
                    && self.annotate.tool.is_none()
                {
                    if let (Some(overall), Some(position)) =
                        (combined_bounds(self.clouds), cursor.position_in(bounds))
                    {
                        let projection = Projection::new(
                            overall,
                            self.yaw,
                            self.pitch,
                            self.zoom,
                            self.pan,
                            bounds.width,
                            bounds.height,
                        );
                        if let Some((cloud_index, pose_index)) =
                            scan_pose_at(self.clouds, projection, position, self.photos())
                        {
                            *state = None;
                            return (
                                event::Status::Captured,
                                Some(if self.station_has_photos(cloud_index, pose_index) {
                                    Message::EnterPanorama(cloud_index, pose_index)
                                } else {
                                    Message::CenterScanPose(cloud_index, pose_index)
                                }),
                            );
                        }
                    }
                }
                *state = cursor.position_in(bounds).map(|position| DragState {
                    start: position,
                    position,
                    mode: if self.box_select || self.pick_mode {
                        DragMode::Select
                    } else if self.measure.mode.is_some() {
                        DragMode::MeasurePending
                    } else if self.annotate.tool.is_some() {
                        DragMode::AnnotatePending
                    } else {
                        DragMode::Orbit
                    },
                });
                (
                    event::Status::Captured,
                    state.as_ref().and_then(|drag| {
                        (matches!(drag.mode, DragMode::Select) && self.box_select).then_some(
                            Message::SelectionDrag(
                                [drag.start.x, drag.start.y],
                                [drag.position.x, drag.position.y],
                            ),
                        )
                    }),
                )
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                *state = cursor.position_in(bounds).map(|position| DragState {
                    start: position,
                    position,
                    mode: DragMode::RightPending,
                });
                (event::Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) => {
                *state = cursor.position_in(bounds).map(|position| DragState {
                    start: position,
                    position,
                    mode: middle_drag_mode(modifiers),
                });
                (event::Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(button))
                if matches!(
                    button,
                    mouse::Button::Left | mouse::Button::Right | mouse::Button::Middle
                ) =>
            {
                let message = state.take().and_then(|drag| {
                    let position = cursor
                        .position_from(bounds.position())
                        .unwrap_or(drag.position);
                    if button == mouse::Button::Left {
                        let click =
                            orbit_click(last_click, drag, position, Instant::now(), bounds.size());
                        if click.is_some() {
                            return click;
                        }
                    }
                    finish_viewport_drag(button, drag, position, bounds.size())
                });
                (event::Status::Captured, message)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(previous) = state.as_mut() {
                    let Some(position) = cursor.position_from(bounds.position()) else {
                        return (event::Status::Captured, None);
                    };
                    let mut dx = position.x - previous.position.x;
                    let mut dy = position.y - previous.position.y;
                    previous.position = position;
                    if matches!(
                        previous.mode,
                        DragMode::RightPending
                            | DragMode::MeasurePending
                            | DragMode::AnnotatePending
                    ) {
                        if (position.x - previous.start.x).hypot(position.y - previous.start.y)
                            < 5.0
                        {
                            return (event::Status::Captured, None);
                        }
                        previous.mode = if matches!(previous.mode, DragMode::RightPending) {
                            DragMode::Pan
                        } else {
                            DragMode::Orbit
                        };
                        dx = position.x - previous.start.x;
                        dy = position.y - previous.start.y;
                    }
                    (
                        event::Status::Captured,
                        Some(match previous.mode {
                            DragMode::Orbit => Message::Orbit(dx, dy),
                            DragMode::Turn => Message::Orbit(-dx, dy),
                            DragMode::Pan => Message::Pan(dx, dy),
                            DragMode::RightPending
                            | DragMode::MeasurePending
                            | DragMode::AnnotatePending => unreachable!(),
                            DragMode::Section(axis, is_min) => {
                                let (Some(section), Some(scene_bounds)) =
                                    (self.section, combined_bounds(self.clouds))
                                else {
                                    return (event::Status::Captured, None);
                                };
                                let projection = Projection::new(
                                    scene_bounds,
                                    self.yaw,
                                    self.pitch,
                                    self.zoom,
                                    self.pan,
                                    bounds.width,
                                    bounds.height,
                                );
                                let Some(delta) = section_handle_delta(
                                    axis,
                                    is_min,
                                    section,
                                    self.section_reference.unwrap_or(scene_bounds),
                                    projection,
                                    [dx, dy],
                                ) else {
                                    return (event::Status::Captured, None);
                                };
                                Message::SectionHandleDelta(axis, is_min, delta)
                            }
                            DragMode::Select => {
                                if self.box_select {
                                    Message::SelectionDrag(
                                        [previous.start.x, previous.start.y],
                                        [position.x, position.y],
                                    )
                                } else {
                                    return (event::Status::Captured, None);
                                }
                            }
                        }),
                    )
                } else {
                    let size = bounds.size();
                    if (size.width - self.viewport_size.width).abs() > 1.0
                        || (size.height - self.viewport_size.height).abs() > 1.0
                    {
                        (event::Status::Ignored, Some(Message::ViewportSize(size)))
                    } else {
                        (event::Status::Ignored, None)
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.is_over(bounds) =>
            {
                let amount = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                let position = cursor
                    .position_in(bounds)
                    .unwrap_or(UiPoint::new(bounds.width * 0.5, bounds.height * 0.5));
                (
                    event::Status::Captured,
                    Some(Message::Zoom(
                        amount,
                        [position.x, position.y],
                        bounds.size(),
                    )),
                )
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        self.annotate.drawn_at(bounds);
        if let Some(view) = self.walk {
            self.draw_walk_overlay(&mut frame, view, bounds.size());
            return vec![frame.into_geometry()];
        }
        let Some(overall_bounds) = combined_bounds(self.clouds) else {
            frame.fill_text(canvas::Text {
                content: if self.loading_status.is_some() {
                    "Importing point cloud…"
                } else {
                    "Open a point cloud to begin"
                }
                .into(),
                position: UiPoint::new(bounds.width * 0.5, bounds.height * 0.5 - 14.0),
                horizontal_alignment: iced::alignment::Horizontal::Center,
                vertical_alignment: iced::alignment::Vertical::Center,
                size: iced::Pixels(16.0),
                color: Color::WHITE,
                ..canvas::Text::default()
            });
            if let Some(status) = self.loading_status {
                frame.fill_text(canvas::Text {
                    content: status.into(),
                    position: UiPoint::new(bounds.width * 0.5, bounds.height * 0.5 + 18.0),
                    horizontal_alignment: iced::alignment::Horizontal::Center,
                    vertical_alignment: iced::alignment::Vertical::Center,
                    size: iced::Pixels(12.0),
                    color: Color::from_rgb8(180, 183, 191),
                    ..canvas::Text::default()
                });
            }
            view_cube::draw(
                &mut frame,
                bounds,
                self.yaw,
                self.pitch,
                _cursor.position_in(bounds),
            );
            if let Some(menu) = self.context_menu {
                draw_context_menu(&mut frame, menu, bounds, _cursor.position_in(bounds), false);
            }
            return vec![frame.into_geometry()];
        };

        let projection = Projection::new(
            overall_bounds,
            self.yaw,
            self.pitch,
            self.zoom,
            self.pan,
            bounds.width,
            bounds.height,
        );
        for entry in self.clouds.iter().filter(|entry| entry.visible) {
            if let Some(selection) = &entry.selection {
                for source_point in &selection.highlights {
                    let point = if selection.highlights_source {
                        entry.transform.point(*source_point)
                    } else {
                        *source_point
                    };
                    if !self.accepts(&point) {
                        continue;
                    }
                    if let Some((x, y, _)) = projection.project(point.xyz) {
                        if selection.count == 1 {
                            frame.stroke_rectangle(
                                UiPoint::new(x - 5.0, y - 5.0),
                                Size::new(11.0, 11.0),
                                canvas::Stroke::default()
                                    .with_color(Color::from_rgb8(217, 119, 6))
                                    .with_width(1.5),
                            );
                        } else {
                            frame.fill_rectangle(
                                UiPoint::new(x, y),
                                Size::new(1.0, 1.0),
                                Color::from_rgb8(217, 119, 6),
                            );
                        }
                    }
                }
            }
        }
        if let Some(section) = self.section {
            let vertices = section.corners();
            for (start, end) in [
                (0, 1),
                (1, 2),
                (2, 3),
                (3, 0),
                (4, 5),
                (5, 6),
                (6, 7),
                (7, 4),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ] {
                if let (Some(a), Some(b)) = (
                    projection.project_unclipped(vertices[start]),
                    projection.project_unclipped(vertices[end]),
                ) {
                    let path = canvas::Path::line(UiPoint::new(a.0, a.1), UiPoint::new(b.0, b.1));
                    frame.stroke(
                        &path,
                        canvas::Stroke::default()
                            .with_color(Color::from_rgb8(217, 119, 6))
                            .with_width(1.4),
                    );
                }
            }
            let hovered_handle = _cursor
                .position_in(bounds)
                .and_then(|point| section_handle_at(point, section, projection));
            for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                for is_min in [true, false] {
                    if let Some((x, y, _)) =
                        projection.project_unclipped(section_handle_world(section, axis, is_min))
                    {
                        if x < -10.0
                            || y < -10.0
                            || x > bounds.width + 10.0
                            || y > bounds.height + 10.0
                        {
                            continue;
                        }
                        let point = UiPoint::new(x, y);
                        let hovered = hovered_handle == Some((axis, is_min));
                        let circle = canvas::Path::circle(point, if hovered { 7.0 } else { 5.0 });
                        frame.fill(
                            &circle,
                            if hovered {
                                Color::from_rgb8(245, 158, 11)
                            } else {
                                Color::from_rgb8(83, 60, 38)
                            },
                        );
                        frame.stroke(
                            &circle,
                            canvas::Stroke::default()
                                .with_color(Color::from_rgb8(245, 158, 11))
                                .with_width(1.3),
                        );
                        frame.fill_text(canvas::Text {
                            content: format!("{name}{}", if is_min { "-" } else { "+" }),
                            position: UiPoint::new(x + 8.0, y + 3.0),
                            size: iced::Pixels(10.0),
                            color: Color::from_rgb8(245, 188, 100),
                            ..canvas::Text::default()
                        });
                    }
                }
            }
        }
        if self.show_scan_poses {
            let pose_count: usize = self
                .clouds
                .iter()
                .filter(|entry| entry.visible)
                .map(|entry| entry.cloud.scan_poses.len())
                .sum();
            let show_labels = pose_count <= MAX_LABELLED_STATIONS;
            let mut markers = Vec::with_capacity(pose_count);
            // Parallel to `markers`: the station is drawn as a photo ball.
            let mut photo_markers = Vec::with_capacity(pose_count);
            for entry in self.clouds.iter().filter(|entry| entry.visible) {
                for (station, pose) in entry.cloud.scan_poses.iter().enumerate() {
                    let Some((x, y, _)) = projection.project(entry.transform.xyz(pose.position))
                    else {
                        continue;
                    };
                    let known = markers.len();
                    push_scan_marker(
                        &mut markers,
                        x,
                        y,
                        &pose.label,
                        entry.transform.axes(pose.axes),
                        show_labels,
                    );
                    if markers.len() > known {
                        photo_markers.push(
                            self.photos().is_some_and(|atlas| {
                                atlas.slot(&entry.cloud.path, station).is_some()
                            }),
                        );
                    }
                }
            }
            let label_positions = if show_labels {
                scan_marker_label_positions(&markers, bounds.size())
            } else {
                Vec::new()
            };
            for (marker_index, marker) in markers.into_iter().enumerate() {
                let center = UiPoint::new(marker.x, marker.y);
                let ball = photo_markers[marker_index] && marker.labels.len() <= 1;
                if show_labels && !ball {
                    if let Some(axes) = marker.axes {
                        for (axis, label, color) in [
                            (axes[0], "X", Color::from_rgb8(190, 104, 98)),
                            (axes[1], "Y", Color::from_rgb8(124, 171, 116)),
                            (axes[2], "Z", Color::from_rgb8(112, 153, 192)),
                        ] {
                            let horizontal = axis
                                .iter()
                                .zip(projection.right)
                                .map(|(a, b)| a * b)
                                .sum::<f64>() as f32;
                            let vertical = -axis
                                .iter()
                                .zip(projection.up)
                                .map(|(a, b)| a * b)
                                .sum::<f64>() as f32;
                            let tip = UiPoint::new(
                                marker.x + horizontal * 22.0,
                                marker.y + vertical * 22.0,
                            );
                            if (tip.x - marker.x).hypot(tip.y - marker.y) < 8.0 {
                                continue;
                            }
                            frame.stroke(
                                &canvas::Path::line(center, tip),
                                canvas::Stroke::default().with_color(color).with_width(2.0),
                            );
                            frame.fill_text(canvas::Text {
                                content: label.into(),
                                position: UiPoint::new(tip.x + 2.0, tip.y + 2.0),
                                size: iced::Pixels(9.0),
                                color,
                                ..canvas::Text::default()
                            });
                        }
                    }
                }
                if !ball {
                    let ring = canvas::Path::circle(center, 6.0);
                    frame.fill(&ring, Color::from_rgb8(42, 42, 50));
                    frame.stroke(
                        &ring,
                        canvas::Stroke::default()
                            .with_color(Color::from_rgb8(245, 158, 11))
                            .with_width(2.0),
                    );
                    for (start, end) in [
                        (
                            UiPoint::new(marker.x - 10.0, marker.y),
                            UiPoint::new(marker.x + 10.0, marker.y),
                        ),
                        (
                            UiPoint::new(marker.x, marker.y - 10.0),
                            UiPoint::new(marker.x, marker.y + 10.0),
                        ),
                    ] {
                        frame.stroke(
                            &canvas::Path::line(start, end),
                            canvas::Stroke::default()
                                .with_color(Color::from_rgb8(245, 158, 11))
                                .with_width(1.0),
                        );
                    }
                }
                if let Some(position) = label_positions.get(marker_index).copied().flatten() {
                    let content = scan_marker_label(&marker);
                    let width = content.chars().count() as f32 * 6.0 + 2.0;
                    let badge_position = UiPoint::new(position.x - 3.0, position.y - 2.0);
                    let badge_size = Size::new(width + 6.0, 17.0);
                    frame.fill_rectangle(badge_position, badge_size, Color::from_rgb8(42, 42, 50));
                    frame.stroke_rectangle(
                        badge_position,
                        badge_size,
                        canvas::Stroke::default()
                            .with_color(Color::from_rgb8(126, 88, 44))
                            .with_width(1.0),
                    );
                    frame.fill_text(canvas::Text {
                        content,
                        position,
                        size: iced::Pixels(10.0),
                        color: Color::from_rgb8(245, 188, 100),
                        ..canvas::Text::default()
                    });
                }
            }
        }
        self.draw_drawing(&mut frame, bounds.size());
        self.draw_faces(&mut frame, bounds.size());
        self.draw_measure(&mut frame, bounds.size());
        self.draw_annotations(&mut frame, bounds.size());
        // While the camera turns about the orbit point, the point is marked.
        let turning = _state.drag.is_some_and(|drag| {
            matches!(drag.mode, DragMode::Orbit | DragMode::Turn) && drag.position != drag.start
        });
        if let Some((x, y, _)) = self
            .orbit_point
            .filter(|_| turning)
            .and_then(|point| projection.project(point))
        {
            orbit_point::draw_marker(&mut frame, UiPoint::new(x, y));
        }
        view_cube::draw(
            &mut frame,
            bounds,
            self.yaw,
            self.pitch,
            _cursor.position_in(bounds),
        );
        if let Some((start, end)) = self.drag_rectangle {
            let rectangle = ScreenRect::from_corners(start, end);
            frame.stroke_rectangle(
                UiPoint::new(rectangle.left, rectangle.top),
                Size::new(
                    rectangle.right - rectangle.left,
                    rectangle.bottom - rectangle.top,
                ),
                canvas::Stroke::default()
                    .with_color(Color::from_rgb8(217, 119, 6))
                    .with_width(1.5),
            );
        }
        if let Some(menu) = self.context_menu {
            draw_context_menu(
                &mut frame,
                menu,
                bounds,
                _cursor.position_in(bounds),
                self.section.is_some(),
            );
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if let Some(view) = self.walk {
            return match cursor.position_in(bounds) {
                Some(point) if self.walk_station_at(view, point, bounds.size()).is_some() => {
                    mouse::Interaction::Pointer
                }
                Some(_) => mouse::Interaction::Grab,
                None => mouse::Interaction::default(),
            };
        }
        if cursor.is_over(bounds) {
            if self.context_menu.is_some_and(|menu| {
                cursor
                    .position_in(bounds)
                    .and_then(|point| context_action_at(point, menu, bounds))
                    .is_some()
            }) || cursor
                .position_in(bounds)
                .and_then(|point| view_cube::hit(point, bounds, self.yaw, self.pitch))
                .is_some()
                || cursor.position_in(bounds).is_some_and(|point| {
                    self.section.is_some_and(|section| {
                        combined_bounds(self.clouds).is_some_and(|overall| {
                            let projection = Projection::new(
                                overall,
                                self.yaw,
                                self.pitch,
                                self.zoom,
                                self.pan,
                                bounds.width,
                                bounds.height,
                            );
                            section_handle_at(point, section, projection).is_some()
                        })
                    })
                })
                || (self.show_scan_poses
                    && !self.box_select
                    && !self.pick_mode
                    && self.measure.mode.is_none()
                    && self.annotate.tool.is_none())
                    && cursor.position_in(bounds).is_some_and(|point| {
                        combined_bounds(self.clouds).is_some_and(|overall| {
                            scan_pose_at(
                                self.clouds,
                                Projection::new(
                                    overall,
                                    self.yaw,
                                    self.pitch,
                                    self.zoom,
                                    self.pan,
                                    bounds.width,
                                    bounds.height,
                                ),
                                point,
                                self.photos(),
                            )
                            .is_some()
                        })
                    })
            {
                mouse::Interaction::Pointer
            } else if self.box_select || self.measure.mode.is_some() || self.annotate.tool.is_some()
            {
                mouse::Interaction::Crosshair
            } else {
                mouse::Interaction::Grab
            }
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Above this many visible stations the overview shows markers without labels.
const MAX_LABELLED_STATIONS: usize = 64;
/// A station with photos as it appears from the walking camera.
struct WalkStation<'a> {
    cloud: usize,
    station: usize,
    x: f32,
    y: f32,
    /// Pointer reach in pixels: the ball as drawn, or a marker.
    reach: f32,
    label: &'a str,
}

impl PointViewport<'_> {
    fn photos(&self) -> Option<&PhotoAtlas> {
        self.photo_atlas.map(|atlas| atlas.as_ref())
    }

    fn station_has_photos(&self, cloud_index: usize, station: usize) -> bool {
        self.clouds.get(cloud_index).is_some_and(|entry| {
            self.photos()
                .is_some_and(|atlas| atlas.slot(&entry.cloud.path, station).is_some())
        })
    }

    /// The camera in use: the walking camera when active, the orbit camera otherwise.
    fn projection(&self, scene: Bounds, width: f32, height: f32) -> Projection {
        match self.walk {
            Some(view) => Projection::from_eye(
                scene,
                view.eye,
                view.basis(),
                view.focal(Size::new(width, height)),
                width,
                height,
            ),
            None => Projection::new(
                scene, self.yaw, self.pitch, self.zoom, self.pan, width, height,
            ),
        }
    }

    /// Other stations with photos, placed where the walking camera sees them.
    fn walk_stations(&self, view: WalkView, size: Size) -> Vec<WalkStation<'_>> {
        let focal = f64::from(view.focal(size));
        let mut stations = Vec::new();
        for (cloud, entry) in self
            .clouds
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.visible)
        {
            for (station, pose) in entry.cloud.scan_poses.iter().enumerate() {
                if self.walk_station == Some((cloud, station))
                    || !self.station_has_photos(cloud, station)
                {
                    continue;
                }
                let position = entry.transform.xyz(pose.position);
                let Some((x, y)) = view.project(position, size) else {
                    continue;
                };
                if x < 0.0 || y < 0.0 || x > size.width || y > size.height {
                    continue;
                }
                stations.push(WalkStation {
                    cloud,
                    station,
                    x,
                    y,
                    reach: station_photos::ball_pixel_radius(focal, view.distance_to(position)),
                    label: &pose.label,
                });
            }
        }
        stations
    }

    fn walk_station_at(
        &self,
        view: WalkView,
        pointer: UiPoint,
        size: Size,
    ) -> Option<(usize, usize)> {
        self.walk_stations(view, size)
            .into_iter()
            .map(|station| {
                let distance = (pointer.x - station.x).hypot(pointer.y - station.y);
                (station, distance)
            })
            .filter(|(station, distance)| *distance <= station.reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(station, _)| (station.cloud, station.station))
    }

    /// While walking every drag looks around and a click on a station steps
    /// into its photo.
    fn update_walk(
        &self,
        view: WalkView,
        drag: &mut Option<DragState>,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(button))
                if matches!(
                    button,
                    mouse::Button::Left | mouse::Button::Right | mouse::Button::Middle
                ) =>
            {
                let Some(position) = cursor.position_in(bounds) else {
                    return (event::Status::Ignored, None);
                };
                if button == mouse::Button::Left {
                    if let Some((cloud, station)) =
                        self.walk_station_at(view, position, bounds.size())
                    {
                        *drag = None;
                        return (
                            event::Status::Captured,
                            Some(Message::EnterPanorama(cloud, station)),
                        );
                    }
                }
                *drag = Some(DragState {
                    start: position,
                    position,
                    mode: DragMode::Orbit,
                });
                (event::Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(
                button @ (mouse::Button::Left | mouse::Button::Right | mouse::Button::Middle),
            )) => {
                // With an annotation tool a left click without a drag picks a point.
                let click = drag
                    .take()
                    .filter(|_| button == mouse::Button::Left && self.annotate.tool.is_some())
                    .and_then(|pressed| {
                        let position = cursor.position_from(bounds.position())?;
                        let moved =
                            (position.x - pressed.start.x).hypot(position.y - pressed.start.y);
                        (moved < 5.0).then(|| {
                            Message::Views(views::ViewAction::Click(
                                [position.x, position.y],
                                bounds.size(),
                            ))
                        })
                    });
                (event::Status::Captured, click)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(previous) = drag.as_mut() {
                    let Some(position) = cursor.position_from(bounds.position()) else {
                        return (event::Status::Captured, None);
                    };
                    let dx = position.x - previous.position.x;
                    let dy = position.y - previous.position.y;
                    previous.position = position;
                    (event::Status::Captured, Some(Message::WalkLook(dx, dy)))
                } else {
                    let size = bounds.size();
                    if (size.width - self.viewport_size.width).abs() > 1.0
                        || (size.height - self.viewport_size.height).abs() > 1.0
                    {
                        (event::Status::Ignored, Some(Message::ViewportSize(size)))
                    } else {
                        (event::Status::Ignored, None)
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta })
                if cursor.is_over(bounds) =>
            {
                let amount = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                (event::Status::Captured, Some(Message::WalkZoom(amount)))
            }
            _ => (event::Status::Ignored, None),
        }
    }

    /// Labels over the walking view: the other stations, and inside a station
    /// its name.
    fn draw_walk_overlay(&self, frame: &mut Frame, view: WalkView, size: Size) {
        let amber = Color::from_rgb8(245, 158, 11);
        let badge = |frame: &mut Frame, content: String, position: UiPoint| {
            let width = content.chars().count() as f32 * 6.0 + 2.0;
            frame.fill_rectangle(
                UiPoint::new(position.x - 3.0, position.y - 2.0),
                Size::new(width + 6.0, 17.0),
                Color::from_rgba8(42, 42, 50, 0.85),
            );
            frame.fill_text(canvas::Text {
                content,
                position,
                size: iced::Pixels(10.0),
                color: Color::from_rgb8(245, 188, 100),
                ..canvas::Text::default()
            });
        };
        self.draw_drawing(frame, size);
        self.draw_faces(frame, size);
        self.draw_measure(frame, size);
        self.draw_annotations(frame, size);
        let inside = self.walk_station.is_some();
        for station in self.walk_stations(view, size) {
            if inside {
                // From inside a photo the neighbours are not drawn as balls.
                let ring = canvas::Path::circle(UiPoint::new(station.x, station.y), 7.0);
                frame.fill(&ring, Color::from_rgba8(42, 42, 50, 0.7));
                frame.stroke(
                    &ring,
                    canvas::Stroke::default().with_color(amber).with_width(2.0),
                );
            }
            badge(
                frame,
                station.label.to_owned(),
                UiPoint::new(
                    station.x + if inside { 13.0 } else { station.reach + 5.0 },
                    station.y - 7.0,
                ),
            );
        }
        let title = match self.walk_station.and_then(|(cloud, station)| {
            self.clouds
                .get(cloud)
                .and_then(|entry| entry.cloud.scan_poses.get(station))
        }) {
            Some(pose) if self.panorama_photos.is_some() => pose.label.clone(),
            Some(pose) => format!("{} · loading full resolution…", pose.label),
            None => "Walking · W A S D to move, Q E down and up, Shift faster".into(),
        };
        badge(frame, title, UiPoint::new(14.0, size.height - 24.0));
    }

    fn accepts(&self, point: &Point) -> bool {
        if let Some(section) = self.section {
            if !section.contains(point.xyz) {
                return false;
            }
        }
        if !self.class_visibility.allows(point.classification) {
            return false;
        }
        match point.classification {
            Some(2) => self.filter_ground,
            Some(3..=5) => self.filter_vegetation,
            Some(6) => self.filter_buildings,
            _ => self.filter_other,
        }
    }

    fn color(&self, point: &Point, bounds: Bounds) -> Color {
        let rgb = match self.color_mode {
            ColorMode::Rgb => point.rgb.unwrap_or([245, 247, 250]),
            ColorMode::Elevation => {
                let range = (bounds.max[2] - bounds.min[2]).max(0.001);
                let t = ((point.xyz[2] - bounds.min[2]) / range).clamp(0.0, 1.0);
                [
                    (30.0 + 225.0 * t) as u8,
                    (80.0 + 150.0 * (1.0 - (2.0 * t - 1.0).abs())) as u8,
                    (230.0 * (1.0 - t)) as u8,
                ]
            }
            ColorMode::Intensity => point.intensity.map_or([210, 218, 225], |intensity| {
                let value = (intensity / 257) as u8;
                [value; 3]
            }),
            ColorMode::Classification => match point.classification.unwrap_or(0) {
                2 => [150, 110, 75],
                3..=5 => [80, 190, 95],
                6 => [225, 80, 75],
                9 => [75, 140, 225],
                _ => [210, 210, 210],
            },
        };
        Color::from_rgb8(rgb[0], rgb[1], rgb[2])
    }
}

#[cfg(test)]
mod surface_settings_tests {
    use super::*;

    #[test]
    fn meshing_uses_world_section_and_visible_classification() {
        let point = Point {
            xyz: [1.0, 2.0, 3.0],
            rgb: None,
            intensity: None,
            classification: Some(2),
        };
        let transform = CloudTransform {
            scale: [2.0, 1.0, 1.0],
            offset: [10.0, 0.0, 0.0],
        };
        let mut filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: Some(OrientedBox::from(Bounds {
                min: [11.0, 1.0, 2.0],
                max: [13.0, 3.0, 4.0],
            })),
        };
        assert!(mesh_accepts(0, &point, None, filter, transform));
        filter.section = Some(OrientedBox::from(Bounds {
            min: [0.0, 1.0, 2.0],
            max: [2.0, 3.0, 4.0],
        }));
        assert!(!mesh_accepts(0, &point, None, filter, transform));
        filter.section = None;
        filter.ground = false;
        assert!(!mesh_accepts(0, &point, None, filter, transform));
        filter.ground = true;
        filter.classes.set(2, false);
        assert!(!mesh_accepts(0, &point, None, filter, transform));
    }

    #[test]
    fn ui_surface_settings_validate_before_meshing() {
        let mut studio = Studio::default();
        let defaults = studio.surface_mesh_config().unwrap();
        assert_eq!(defaults.max_vertices, 50_000);
        assert_eq!(defaults.neighbors, 12);
        assert_eq!(defaults.max_edge_factor, 4.0);

        let _ = studio.update(Message::SurfaceSetting(0, "25000".into()));
        let _ = studio.update(Message::SurfaceSetting(1, "16".into()));
        let _ = studio.update(Message::SurfaceSetting(2, "5.5".into()));
        let chosen = studio.surface_mesh_config().unwrap();
        assert_eq!(chosen.max_vertices, 25_000);
        assert_eq!(chosen.neighbors, 16);
        assert_eq!(chosen.max_edge_factor, 5.5);

        let _ = studio.update(Message::SurfaceSetting(1, "33".into()));
        assert!(studio.surface_mesh_config().is_err());
        let _ = studio.update(Message::SurfaceSetting(1, "12".into()));
        let _ = studio.update(Message::SurfaceSetting(2, "NaN".into()));
        assert!(studio.surface_mesh_config().is_err());
    }
}

#[cfg(test)]
mod section_box_tests {
    use super::*;

    #[test]
    fn selected_bounds_reuse_cache_only_when_deletions_do_not_overlap() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.xyz");
        std::fs::write(&path, "0 0 0\n10 0 0\n20 0 0\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 3).unwrap());
        let selected = Arc::new(SelectionMask {
            bits: vec![0b110],
            count: 2,
            highlights: Vec::new(),
            highlights_source: true,
            source_bounds: Some(Bounds {
                min: [10.0, 0.0, 0.0],
                max: [20.0, 0.0, 0.0],
            }),
        });
        let first = SelectionMask::single(
            cloud.total_points,
            IndexedPoint {
                point: cloud.points[0],
                ordinal: 0,
            },
        )
        .unwrap();
        let mut deleted = DeletionMask::new(cloud.total_points).unwrap();
        deleted.apply(&first).unwrap();
        let source = |deleted: DeletionMask| SelectedSource {
            index: 0,
            cloud: Arc::clone(&cloud),
            selection: Arc::clone(&selected),
            deleted: Some(Arc::new(deleted)),
            transform: CloudTransform::default(),
        };
        let cached = selected_source_bounds(&[source(deleted.clone())], None).unwrap();
        assert_eq!(
            (cached.0.min[0], cached.0.max[0], cached.1),
            (10.0, 20.0, 2)
        );
        let middle = SelectionMask::single(
            cloud.total_points,
            IndexedPoint {
                point: cloud.points[1],
                ordinal: 1,
            },
        )
        .unwrap();
        deleted.apply(&middle).unwrap();
        let remaining = selected_source_bounds(&[source(deleted.clone())], None).unwrap();
        assert_eq!(
            (remaining.0.min[0], remaining.0.max[0], remaining.1),
            (20.0, 20.0, 1)
        );
        let last = SelectionMask::single(
            cloud.total_points,
            IndexedPoint {
                point: cloud.points[2],
                ordinal: 2,
            },
        )
        .unwrap();
        deleted.apply(&last).unwrap();
        assert!(selected_source_bounds(&[source(deleted)], None).is_err());
    }

    #[test]
    fn selected_point_frames_camera_without_enabling_section() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("survey.xyz");
        std::fs::write(&path, "0 0 0\n100 100 10\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let record = IndexedPoint {
            point: cloud.points[1],
            ordinal: 1,
        };
        let selection = Arc::new(SelectionMask::single(cloud.total_points, record).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio.clouds[0].selection = Some(Arc::clone(&selection));
        let _ = studio.update(Message::ZoomToSelection);
        assert!(studio.selection_bounds_pending);
        let bounds = selected_source_bounds(
            &[SelectedSource {
                index: 0,
                cloud: Arc::clone(&studio.clouds[0].cloud),
                selection: Arc::clone(&selection),
                deleted: None,
                transform: studio.clouds[0].transform,
            }],
            None,
        )
        .unwrap();
        let _ = studio.update(Message::SelectionBoundsReady(
            true,
            studio.revision,
            vec![(0, selection)],
            Ok(bounds),
        ));
        assert!(!studio.selection_bounds_pending);
        assert!(!studio.section_enabled);
        assert!(studio.zoom < 1.0);
        let scene = combined_bounds(&studio.clouds).unwrap();
        let projection = Projection::new(
            scene,
            studio.yaw,
            studio.pitch,
            studio.zoom,
            studio.pan,
            studio.viewport_size.width,
            studio.viewport_size.height,
        );
        let (x, y, _) = projection.project(record.point.xyz).unwrap();
        assert!((x - studio.viewport_size.width * 0.5).abs() < 5.0);
        assert!((y - studio.viewport_size.height * 0.5).abs() < 5.0);
    }

    #[test]
    fn zoom_label_reports_magnification_instead_of_inverse_scale() {
        assert_eq!(format_zoom_level(1.0), "1.00×");
        assert_eq!(format_zoom_level(0.01), "100×");
        assert_eq!(format_zoom_level(0.000_001), "1.000.000×");
    }

    #[test]
    fn displayed_rounded_limits_clamp_to_precise_survey_bounds() {
        let model = Bounds {
            min: [206600.0, 474000.0, 0.803],
            max: [208600.0, 474999.999, 79.363],
        };
        let section = section_within_model(
            Bounds {
                min: [206600.0, 474000.0, 0.8],
                max: [206700.0, 475000.0, 79.36],
            },
            model,
        )
        .unwrap();
        assert_eq!(section.min[2], 0.803);
        assert_eq!(section.max[1], 474999.999);
        assert!(section_within_model(
            Bounds {
                min: [206600.0, 474000.0, 0.5],
                max: [206700.0, 475000.0, 79.36],
            },
            model,
        )
        .is_none());
    }

    #[test]
    fn zoom_box_frames_a_small_survey_section_on_all_axes() {
        let scene = Bounds {
            min: [207_000.0, 474_000.0, 0.0],
            max: [208_000.0, 475_000.0, 80.0],
        };
        let section = Bounds {
            min: [207_450.0, 474_720.0, 5.0],
            max: [207_465.0, 474_740.0, 12.0],
        };
        let size = Size::new(915.0, 740.0);
        let (zoom, pan) = camera_to_frame_bounds(scene, section, -0.8, 0.6, size).unwrap();
        assert!(zoom < 1.0);
        let projection = Projection::new(scene, -0.8, 0.6, zoom, pan, size.width, size.height);
        for corner in 0..8 {
            let xyz = std::array::from_fn(|axis| {
                if corner & (1 << axis) == 0 {
                    section.min[axis]
                } else {
                    section.max[axis]
                }
            });
            let (x, y, _) = projection.project_unclipped(xyz).unwrap();
            assert!((size.width * 0.1..=size.width * 0.9).contains(&x));
            assert!((size.height * 0.1..=size.height * 0.9).contains(&y));
        }
    }

    #[test]
    fn fit_section_uses_selected_points_outside_preview() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.xyz");
        let contents = (0..10_000)
            .map(|index| format!("{index} {} {}\n", index * 2, index * 3))
            .collect::<String>();
        std::fs::write(&path, contents).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 1).unwrap());
        assert_eq!(cloud.points.len(), 1);
        let mut bits = vec![0u64; 10_000usize.div_ceil(64)];
        for ordinal in [2usize, 9_999] {
            bits[ordinal / 64] |= 1u64 << (ordinal % 64);
        }
        let selection = Arc::new(SelectionMask {
            bits,
            count: 2,
            highlights: Vec::new(),
            highlights_source: true,
            source_bounds: None,
        });
        let selected = selected_source_bounds(
            &[SelectedSource {
                index: 0,
                cloud: Arc::clone(&cloud),
                selection: Arc::clone(&selection),
                deleted: None,
                transform: CloudTransform::default(),
            }],
            None,
        )
        .unwrap();
        assert_eq!(selected.0.min, [2.0, 4.0, 6.0]);
        assert_eq!(selected.0.max, [9_999.0, 19_998.0, 29_997.0]);
        assert_eq!(selected.1, 2);

        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio.clouds[0].selection = Some(Arc::clone(&selection));
        let _ = studio.update(Message::FitSectionToSelection);
        assert!(studio.selection_bounds_pending);
        let _ = studio.update(Message::SelectionBoundsReady(
            false,
            studio.revision,
            vec![(0, selection)],
            Ok(selected),
        ));
        assert!(studio.section_enabled);
        assert!(!studio.selection_bounds_pending);
        let section = studio.section_bounds().unwrap();
        for axis in 0..3 {
            assert!(section.min[axis] <= selected.0.min[axis]);
            assert!(section.max[axis] >= selected.0.max[axis]);
            assert!(selected.0.min[axis] - section.min[axis] < 0.001);
            assert!(section.max[axis] - selected.0.max[axis] < 0.001);
        }
    }

    #[test]
    fn xyz_limits_apply_to_world_coordinates_without_losing_survey_precision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("survey.xyz");
        std::fs::write(
            &path,
            "207000.0000004 474000.0000004 0.83\n207999.9999996 474999.9999996 79.363\n",
        )
        .unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let _ = studio.update(Message::SetSectionEnabled(true));
        let _ = studio.update(Message::ApplySectionCoordinates);
        assert!(studio.status.starts_with("Section box updated"));
        studio.section_coordinate_inputs = [
            ["207250.123456".into(), "207750.654321".into()],
            ["474200.123456".into(), "474800.654321".into()],
            ["10.000001".into(), "60.000002".into()],
        ];
        let _ = studio.update(Message::ApplySectionCoordinates);
        let section = studio.section_bounds().unwrap();
        for axis in 0..3 {
            let expected = [
                [207250.123456, 207750.654321],
                [474200.123456, 474800.654321],
                [10.000001, 60.000002],
            ][axis];
            assert!((section.min[axis] - expected[0]).abs() < 0.000_001);
            assert!((section.max[axis] - expected[1]).abs() < 0.000_001);
        }

        let second_path = directory.path().join("distant.xyz");
        std::fs::write(&second_path, "300000 600000 1\n301000 601000 90\n").unwrap();
        let second = Arc::new(pointcloud_core::open(&second_path, 10).unwrap());
        let _ = studio.update(Message::Loaded(Ok(second)));
        assert_eq!(studio.section_bounds(), Some(section));
        let _ = studio.update(Message::SetVisible(1, false));
        assert_eq!(studio.section_bounds(), Some(section));

        studio.section_coordinate_inputs[0] = ["208100".into(), "208200".into()];
        let _ = studio.update(Message::ApplySectionCoordinates);
        assert_eq!(studio.section_bounds(), Some(section));
    }

    fn send(studio: &mut Studio, command: native_api::ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.update(Message::ApiRequest(native_api::ApiRequest {
            command,
            reply,
        }));
        receive.recv().unwrap()
    }

    /// A grid of points 20 by 10 m at RD New coordinates, 3 m high.
    fn studio_with_grid(directory: &Path) -> Studio {
        let path = directory.join("grid.xyz");
        let mut text = String::new();
        for x in 0..=40 {
            for y in 0..=20 {
                for z in 0..=3 {
                    text.push_str(&format!(
                        "{} {} {z}\n",
                        207_000.0 + f64::from(x) * 0.5,
                        474_000.0 + f64::from(y) * 0.5
                    ));
                }
            }
        }
        std::fs::write(&path, text).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 100_000).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio
    }

    fn near(a: [f64; 3], b: [f64; 3], slack: f64) -> bool {
        (0..3).all(|axis| (a[axis] - b[axis]).abs() <= slack)
    }

    #[test]
    fn a_turned_box_keeps_its_limits_and_rotation_through_the_api_and_the_fields() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = studio_with_grid(directory.path());
        let (min, max) = ([207_004.0, 474_002.0, 0.5], [207_012.0, 474_006.0, 2.5]);
        let answer = send(
            &mut studio,
            native_api::ApiCommand::SetSection {
                min,
                max,
                rotation: Some(390.0),
            },
        );
        assert_eq!(answer["ok"], true, "{answer}");
        // A turn is kept between -180 and 180 degrees.
        assert_eq!(answer["section"]["rotation"], 30.0);
        let turned = studio.section_box().unwrap();
        assert_eq!(turned.rotation_degrees, 30.0);
        assert!(near(turned.bounds.min, min, 1e-6) && near(turned.bounds.max, max, 1e-6));
        let status = send(&mut studio, native_api::ApiCommand::Status);
        assert_eq!(status["result"]["section"]["rotation"], 30.0);
        assert_eq!(studio.section_rotation_input, "30");
        assert_eq!(studio.section_coordinate_inputs[0][0], "207004.000000");

        // The filters keep what lies inside the turned box only: along its
        // own X axis 3.9 m from the centre is in, the corner of the limits
        // before the turn is out.
        let filter = studio.mesh_filter();
        let center = turned.center();
        let along = |distance: f64| Point {
            xyz: [
                center[0] + distance * 30f64.to_radians().cos(),
                center[1] + distance * 30f64.to_radians().sin(),
                1.0,
            ],
            rgb: None,
            intensity: None,
            classification: None,
        };
        assert!(filter.accepts(&along(3.9)));
        assert!(!filter.accepts(&along(4.1)));
        let corner = Point {
            xyz: [max[0], max[1], 1.0],
            ..along(0.0)
        };
        assert!(!filter.accepts(&corner));

        // A turn typed in the field turns the box about its centre and keeps
        // its size.
        let _ = studio.update(Message::SectionRotationInput("north".into()));
        let _ = studio.update(Message::ApplySectionRotation);
        assert_eq!(studio.status, "The rotation must be a number of degrees");
        assert_eq!(studio.section_box(), Some(turned));
        // A comma is read as the decimal mark, and a degree sign is left out.
        let _ = studio.update(Message::SectionRotationInput("-15,5°".into()));
        let _ = studio.update(Message::ApplySectionRotation);
        let again = studio.section_box().unwrap();
        assert_eq!(again.rotation_degrees, -15.5);
        assert!(near(again.center(), turned.center(), 1e-6));
        assert!(near(again.size(), turned.size(), 1e-6));

        // A box that does not reach the model, or has no width, is refused
        // and changes nothing.
        for (min, max) in [
            ([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            ([207_004.0, 474_002.0, 0.5], [207_004.0, 474_006.0, 2.5]),
        ] {
            let refused = send(
                &mut studio,
                native_api::ApiCommand::SetSection {
                    min,
                    max,
                    rotation: Some(10.0),
                },
            );
            assert_eq!(refused["ok"], false, "{refused}");
        }
        assert_eq!(studio.section_box(), Some(again));

        // Back to a box along the axes with the fields: limits inside the
        // model and no turn, exactly as before boxes could turn.
        studio.section_rotation_input = "0".into();
        studio.section_coordinate_inputs = [
            ["207004".into(), "207012".into()],
            ["474002".into(), "474006".into()],
            ["0.5".into(), "2.5".into()],
        ];
        let _ = studio.update(Message::ApplySectionCoordinates);
        let plain = studio.section_box().unwrap();
        assert!(!plain.is_turned());
        assert_eq!(studio.section_rotation, 0.0);
        assert_eq!(
            studio.section_reference_bounds,
            combined_bounds(&studio.clouds)
        );
        assert!(near(plain.bounds.min, min, 1e-6) && near(plain.bounds.max, max, 1e-6));
    }

    #[test]
    fn handles_reset_and_fit_follow_the_axes_of_a_turned_box() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = studio_with_grid(directory.path());
        let model = combined_bounds(&studio.clouds).unwrap();
        let answer = send(
            &mut studio,
            native_api::ApiCommand::SetSection {
                min: [207_004.0, 474_002.0, 0.5],
                max: [207_012.0, 474_006.0, 2.5],
                rotation: Some(30.0),
            },
        );
        assert_eq!(answer["ok"], true, "{answer}");
        let before = studio.section_box().unwrap();

        // Dragging the handle at the own X max of the box moves that face
        // along the axis of the box; the face at X min stays.
        let x_min_face = section_handle_world(before, 0, true);
        let x_max_face = section_handle_world(before, 0, false);
        let _ = studio.update(Message::SectionHandleDelta(0, false, 2.0));
        let after = studio.section_box().unwrap();
        assert!(near(section_handle_world(after, 0, true), x_min_face, 1e-6));
        let moved = section_handle_world(after, 0, false);
        let step: [f64; 3] = std::array::from_fn(|axis| moved[axis] - x_max_face[axis]);
        let [along, _] = before.axes();
        let length = step[0].hypot(step[1]);
        assert!(length > 0.1);
        assert!((step[0] / length - along[0]).abs() < 1e-6);
        assert!((step[1] / length - along[1]).abs() < 1e-6);
        assert_eq!(after.rotation_degrees, 30.0);

        // The turned box is drawn with its own corners, and its handles are
        // found where they are drawn.
        let projection = Projection::new(model, 0.0, 1.5, 1.0, [0.0; 2], 800.0, 600.0);
        let (x, y, _) = projection
            .project_unclipped(section_handle_world(after, 1, false))
            .unwrap();
        assert_eq!(
            section_handle_at(UiPoint::new(x, y), after, projection),
            Some((1, false))
        );

        // Reset keeps the turn and holds the whole model.
        let _ = studio.update(Message::ResetSectionBox);
        let reset = studio.section_box().unwrap();
        assert_eq!(reset.rotation_degrees, 30.0);
        for corner in pointcloud_core::bounds_corners(model) {
            let local = reset.to_box(corner);
            assert!((0..3).all(|axis| {
                local[axis] >= reset.bounds.min[axis] - 1e-6
                    && local[axis] <= reset.bounds.max[axis] + 1e-6
            }));
        }

        // Fit selection measures the selected points in the frame of the
        // turn: two points along the own X axis of the box give a box that
        // is long along that axis and thin across it.
        let cloud = Arc::clone(&studio.clouds[0].cloud);
        let ordinal = |x: usize, y: usize, z: usize| (x * 21 + y) * 4 + z;
        let mut bits = vec![0u64; (cloud.total_points as usize).div_ceil(64)];
        // (207 002, 474 001) and (207 012.5, 474 007): 30 degrees apart along
        // the grid within half a millimetre.
        for at in [ordinal(4, 2, 1), ordinal(25, 14, 2)] {
            bits[at / 64] |= 1u64 << (at % 64);
        }
        let selection = Arc::new(SelectionMask {
            bits,
            count: 2,
            highlights: Vec::new(),
            highlights_source: true,
            source_bounds: None,
        });
        studio.clouds[0].selection = Some(Arc::clone(&selection));
        let reference = section_frame_reference(&studio.clouds, 30.0).unwrap();
        let frame = section_pivot_frame(reference, 30.0);
        let selected = selected_source_bounds(
            &[SelectedSource {
                index: 0,
                cloud,
                selection: Arc::clone(&selection),
                deleted: None,
                transform: CloudTransform::default(),
            }],
            Some(frame),
        )
        .unwrap();
        let _ = studio.update(Message::SelectionBoundsReady(
            false,
            studio.revision,
            vec![(0, selection)],
            Ok(selected),
        ));
        let fitted = studio.section_box().unwrap();
        assert_eq!(fitted.rotation_degrees, 30.0);
        let size = fitted.size();
        assert!((size[0] - 12.1).abs() < 0.05, "{size:?}");
        assert!(size[1] < 0.1, "{size:?}");
        for xyz in [[207_002.0, 474_001.0, 1.0], [207_012.5, 474_007.0, 2.0]] {
            let local = fitted.to_box(xyz);
            assert!(
                (0..3).all(|axis| local[axis] >= fitted.bounds.min[axis] - 1e-6
                    && local[axis] <= fitted.bounds.max[axis] + 1e-6)
            );
        }
    }

    #[test]
    fn a_turned_section_is_exported_with_the_points_inside_it() {
        let directory = tempfile::tempdir().unwrap();
        let studio = studio_with_grid(directory.path());
        let cloud = &studio.clouds[0].cloud;
        let turned = OrientedBox::new(
            Bounds {
                min: [207_004.0, 474_004.0, 0.5],
                max: [207_012.0, 474_005.0, 2.5],
            },
            30.0,
        );
        let destination = directory.path().join("turned.ply");
        let count = export_edited_section(
            cloud,
            &destination,
            ExportFormat::PlyBinary,
            CloudTransform::default(),
            turned,
            None,
        )
        .unwrap();
        let truth = pointcloud_core::open(&cloud.path, 100_000)
            .unwrap()
            .points
            .iter()
            .filter(|point| turned.contains(point.xyz))
            .count() as u64;
        assert!(truth > 20, "{truth}");
        assert_eq!(count, truth);
        // Moved by a metre in Z the layer keeps the same points out of the
        // box as its scene positions say.
        let moved = CloudTransform {
            scale: [1.0; 3],
            offset: [0.0, 0.0, 1.0],
        };
        let shifted = export_edited_section(
            cloud,
            &directory.path().join("moved.ply"),
            ExportFormat::PlyBinary,
            moved,
            turned,
            None,
        )
        .unwrap();
        let truth = pointcloud_core::open(&cloud.path, 100_000)
            .unwrap()
            .points
            .iter()
            .filter(|point| turned.contains(moved.xyz(point.xyz)))
            .count() as u64;
        assert_eq!(shifted, truth);
    }
}

#[cfg(test)]
mod scan_marker_tests {
    use super::*;

    #[test]
    fn center_station_keeps_camera_orientation_and_zoom() {
        let scene = Bounds {
            min: [0.0; 3],
            max: [100.0; 3],
        };
        let target = [70.0, 80.0, 20.0];
        let size = Size::new(900.0, 700.0);
        let pan = pan_to_world(scene, target, -0.8, 0.6, 2.0, size).unwrap();
        let projection = Projection::new(scene, -0.8, 0.6, 2.0, pan, size.width, size.height);
        let (x, y, _) = projection.project(target).unwrap();
        assert!((x - size.width * 0.5).abs() < 0.01);
        assert!((y - size.height * 0.5).abs() < 0.01);
    }

    #[test]
    fn nearby_scan_positions_share_one_marker_and_distant_ones_remain_distinct() {
        let mut markers = Vec::new();
        let axes = Some([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
        push_scan_marker(&mut markers, 100.0, 100.0, "Scan 1", axes, true);
        push_scan_marker(&mut markers, 107.0, 104.0, "Scan 2", axes, true);
        push_scan_marker(&mut markers, 140.0, 100.0, "Scan 3", axes, true);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].labels, ["Scan 1", "Scan 2"]);
        assert_eq!(markers[0].axes, None);
        assert_eq!(markers[1].labels, ["Scan 3"]);
        assert_eq!(markers[1].axes, axes);

        push_scan_marker(&mut markers, 100.0, 100.0, "Scan 4", axes, false);
        assert_eq!(markers.len(), 3);
    }

    #[test]
    fn crowded_station_labels_group_and_avoid_each_other() {
        let mut markers = Vec::new();
        push_scan_marker(&mut markers, 100.0, 100.0, "Scan 1", None, true);
        push_scan_marker(&mut markers, 125.0, 110.0, "Scan 2", None, true);
        push_scan_marker(&mut markers, 170.0, 100.0, "Scan 3", None, true);
        assert_eq!(markers.len(), 2);
        assert_eq!(scan_marker_label(&markers[0]), "2 stations");
        let positions = scan_marker_label_positions(&markers, Size::new(300.0, 200.0));
        let first = positions[0].unwrap();
        let second = positions[1].unwrap();
        let first_right = first.x + scan_marker_label(&markers[0]).len() as f32 * 6.0 + 2.0;
        let second_right = second.x + scan_marker_label(&markers[1]).len() as f32 * 6.0 + 2.0;
        assert!(
            first_right <= second.x
                || second_right <= first.x
                || first.y + 13.0 <= second.y
                || second.y + 13.0 <= first.y
        );
    }
}

#[cfg(test)]
mod editing_tests {
    use super::*;

    #[test]
    fn selection_stays_on_source_points_through_live_transforms() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("selected.xyz");
        std::fs::write(&path, "0 0 0\n10 0 0\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let selection = Arc::new(
            SelectionMask::single(
                cloud.total_points,
                IndexedPoint {
                    point: cloud.points[1],
                    ordinal: 1,
                },
            )
            .unwrap(),
        );
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio.clouds[0].selection = Some(Arc::clone(&selection));
        let selected_x = |studio: &Studio| {
            let entry = &studio.clouds[0];
            assert!(Arc::ptr_eq(entry.selection.as_ref().unwrap(), &selection));
            let bounds = selected_source_bounds(
                &[SelectedSource {
                    index: 0,
                    cloud: Arc::clone(&entry.cloud),
                    selection: Arc::clone(&selection),
                    deleted: None,
                    transform: entry.transform,
                }],
                None,
            )
            .unwrap()
            .0;
            assert_eq!(bounds.min[0], bounds.max[0]);
            assert_eq!(
                entry.transform.point(selection.highlights[0]).xyz[0],
                bounds.min[0]
            );
            bounds.min[0]
        };
        assert_eq!(selected_x(&studio), 10.0);
        studio.translate_x = "5".into();
        let _ = studio.update(Message::ApplyTranslation);
        assert_eq!(selected_x(&studio), 15.0);
        studio.scale_inputs = ["2".into(), "1".into(), "1".into()];
        let _ = studio.update(Message::ApplyScale);
        assert_eq!(selected_x(&studio), 20.0);
        let _ = studio.update(Message::ResetTransform);
        assert_eq!(selected_x(&studio), 10.0);
    }

    #[test]
    fn mesh_normals_follow_nonuniform_and_reflected_scale() {
        let normal = [std::f32::consts::FRAC_1_SQRT_2; 2];
        let transformed =
            transformed_mesh_normals(&[[normal[0], normal[1], 0.0]], [2.0, 1.0, 1.0]).unwrap();
        assert!((transformed[0][0] - 0.447_213_6).abs() < 1e-5);
        assert!((transformed[0][1] - 0.894_427_2).abs() < 1e-5);
        assert_eq!(
            transformed_mesh_normals(&[[0.0, 0.0, 1.0]], [-1.0, 1.0, 1.0]).unwrap(),
            vec![[0.0, 0.0, -1.0]]
        );
        assert!(transformed_mesh_normals(&[[0.0, 0.0, 1.0]], [0.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn live_transform_is_shared_by_view_selection_and_export() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("source.xyz");
        std::fs::write(&path, "0 0 0\n10 0 0\n10 10 10\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let tree = Arc::new(
            OctreeIndex::build(
                &cloud,
                IndexConfig {
                    leaf_points: 1,
                    preview_points: 2,
                    max_depth: 4,
                    scratch_dir: Some(directory.path().to_path_buf()),
                },
            )
            .unwrap(),
        );
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&cloud))));
        studio.translate_x = "100".into();
        studio.translate_y = "200".into();
        let _ = studio.update(Message::ApplyTranslation);
        studio.scale_inputs[0] = "2".into();
        let _ = studio.update(Message::ApplyScale);
        assert!(
            studio.zoom < 1.0,
            "camera should retain its world scale after scaling"
        );

        let entry = &studio.clouds[0];
        let expected_x_min = 100.0 - 20.0 / 3.0;
        let expected_x_max = 120.0 - 20.0 / 3.0;
        assert!((entry.bounds().min[0] - expected_x_min).abs() < 1e-10);
        assert!((entry.bounds().max[0] - expected_x_max).abs() < 1e-10);
        assert_eq!(entry.bounds().min[1..], [200.0, 0.0]);
        assert_eq!(entry.bounds().max[1..], [210.0, 10.0]);
        assert!((entry.view_records().next().unwrap().point.xyz[0] - expected_x_min).abs() < 1e-10);
        assert!((entry.centroid_cache.as_ref().unwrap().source_xyz[0] - 20.0 / 3.0).abs() < 1e-10);
        let select_bounds = Bounds {
            min: [112.0, 199.0, -1.0],
            max: [114.0, 211.0, 11.0],
        };
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        };
        for index in [None, Some(Arc::clone(&tree))] {
            let selected = select_world(
                vec![SelectionSource {
                    index: 0,
                    cloud: Arc::clone(&cloud),
                    tree: index,
                    deleted: None,
                    transform: entry.transform,
                }],
                select_bounds,
                filter,
            )
            .unwrap();
            assert_eq!(selected[0].1.count, 2);
            assert!(!selected[0].1.contains(0));
            assert!(selected[0].1.highlights_source);
            assert_eq!(selected[0].1.highlights[0].xyz[0], 10.0);
            assert!(
                (entry.transform.point(selected[0].1.highlights[0]).xyz[0] - expected_x_max).abs()
                    < 1e-10
            );
            assert_eq!(
                selected[0].1.source_bounds,
                Some(Bounds {
                    min: [10.0, 0.0, 0.0],
                    max: [10.0, 10.0, 10.0],
                })
            );
            let moved = CloudTransform {
                offset: std::array::from_fn(|axis| entry.transform.offset[axis] + 5.0),
                ..entry.transform
            };
            let moved_bounds = selected_source_bounds(
                &[SelectedSource {
                    index: 0,
                    cloud: Arc::clone(&cloud),
                    selection: Arc::clone(&selected[0].1),
                    deleted: None,
                    transform: moved,
                }],
                None,
            )
            .unwrap();
            assert!((moved_bounds.0.min[0] - (expected_x_max + 5.0)).abs() < 1e-10);
            assert_eq!(moved_bounds.0.min[1..], [205.0, 5.0]);
            assert_eq!(moved_bounds.0.max[1..], [215.0, 15.0]);
        }
        let selected_bounds = selected_source_bounds(
            &[SelectedSource {
                index: 0,
                cloud: Arc::clone(&cloud),
                selection: Arc::new(SelectionMask {
                    bits: vec![0b110],
                    count: 2,
                    highlights: Vec::new(),
                    highlights_source: true,
                    source_bounds: None,
                }),
                deleted: None,
                transform: entry.transform,
            }],
            None,
        )
        .unwrap();
        assert!((selected_bounds.0.min[0] - expected_x_max).abs() < 1e-10);
        assert!((selected_bounds.0.max[0] - expected_x_max).abs() < 1e-10);
        assert_eq!(selected_bounds.0.min[1..], [200.0, 0.0]);
        assert_eq!(selected_bounds.0.max[1..], [210.0, 10.0]);

        let full = directory.path().join("moved.ply");
        export_edited_where(
            &cloud,
            &full,
            ExportFormat::PlyBinary,
            entry.transform,
            3,
            |_, _| true,
        )
        .unwrap();
        let reopened = pointcloud_core::open(&full, 10).unwrap();
        assert_eq!(reopened.bounds, entry.bounds());
        let section = directory.path().join("section.ply");
        assert_eq!(
            export_edited_section(
                &cloud,
                &section,
                ExportFormat::PlyBinary,
                entry.transform,
                select_bounds.into(),
                None,
            )
            .unwrap(),
            2
        );
        assert_eq!(pointcloud_core::open(section, 10).unwrap().total_points, 2);
        studio.section_reference_bounds = Some(studio.clouds[0].bounds());
        studio.section_min_percent = [90.0, 0.0, 0.0];
        studio.section_max_percent = [100.0; 3];
        studio.section_enabled = true;
        let _ = studio.update(Message::ResetTransform);
        assert_eq!(studio.clouds[0].bounds(), cloud.bounds);
        assert_eq!(studio.section_bounds(), Some(cloud.bounds));
    }

    #[test]
    fn delete_undo_redo_keep_source_ordinals_across_two_clouds() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        for (name, x) in [("first.xyz", 0), ("second.xyz", 10)] {
            let path = directory.path().join(name);
            std::fs::write(&path, format!("{x} 0 0\n{} 0 0\n", x + 1)).unwrap();
            let cloud = Arc::new(pointcloud_core::open(&path, 2).unwrap());
            let _ = studio.update(Message::Loaded(Ok(cloud)));
        }
        for entry in &mut studio.clouds {
            let record = IndexedPoint {
                point: entry.cloud.points[0],
                ordinal: entry.cloud.point_ordinals[0],
            };
            entry.selection = Some(Arc::new(
                SelectionMask::single(entry.cloud.total_points, record).unwrap(),
            ));
        }
        let _ = studio.update(Message::DeleteSelection);
        assert_eq!(
            studio
                .clouds
                .iter()
                .map(CloudEntry::remaining_count)
                .sum::<u64>(),
            2
        );
        assert_eq!(studio.undo_deletions.len(), 1);
        assert!(studio.clouds.iter().all(|entry| {
            entry
                .view_records()
                .filter(|record| entry.record_visible(*record))
                .count()
                == 1
        }));
        let _ = studio.update(Message::UndoDelete);
        assert_eq!(
            studio
                .clouds
                .iter()
                .map(CloudEntry::remaining_count)
                .sum::<u64>(),
            4
        );
        let _ = studio.update(Message::RedoDelete);
        assert_eq!(
            studio
                .clouds
                .iter()
                .map(CloudEntry::remaining_count)
                .sum::<u64>(),
            2
        );

        let first = &studio.clouds[0];
        let destination = directory.path().join("edited.ply");
        let hidden = first.deleted.as_ref().unwrap();
        pointcloud_core::export_where(
            &first.cloud,
            &destination,
            ExportFormat::PlyBinary,
            first.remaining_count(),
            |ordinal, _| !hidden.contains(ordinal),
        )
        .unwrap();
        let reopened = pointcloud_core::open(destination, 10).unwrap();
        assert_eq!(reopened.total_points, 1);
    }
}

#[cfg(test)]
mod lod_budget_tests {
    use super::*;

    #[test]
    fn visible_scan_gets_most_of_budget_and_small_scan_returns_unused_quota() {
        let shares = distribute_lod_budget(
            80_000,
            &[(900.0, 100_000), (50.0, 100_000), (50.0, 100_000)],
        );
        assert_eq!(shares.iter().sum::<usize>(), 80_000);
        assert!(shares[0] > 70_000, "{shares:?}");
        assert!(shares[1] > 0 && shares[2] > 0);

        let capped = distribute_lod_budget(80_000, &[(900.0, 1_000), (100.0, 100_000)]);
        assert_eq!(capped, vec![1_000, 79_000]);

        let weights = [(45.0, 100_000), (30.0, 100_000), (25.0, 100_000)];
        let requested = distribute_lod_budget(80_000, &weights);
        let returned = [requested[0], 2_048, 4_096];
        let next = rebalance_lod_limits(80_000, &weights, &requested, &returned).unwrap();
        assert!(next[0] > 70_000, "{next:?}");
        assert_eq!(next[1], requested[1]);
        assert_eq!(next[2], requested[2]);
    }

    #[test]
    fn section_and_camera_cull_sources_before_budgeting() {
        let scene = Bounds {
            min: [0.0; 3],
            max: [100.0; 3],
        };
        let projection = Projection::new(scene, 0.0, 0.0, 1.0, [0.0; 2], 800.0, 600.0);
        let offscreen = Projection::new(scene, 0.0, 0.0, 1.0, [2_000.0, 0.0], 800.0, 600.0);
        assert!(source_lod_coverage(projection, scene, None).unwrap() > 0.0);
        assert!(source_lod_coverage(offscreen, scene, None).is_none());
        let outside = Bounds {
            min: [200.0; 3],
            max: [300.0; 3],
        };
        assert!(source_lod_coverage(projection, scene, Some(outside.into())).is_none());
    }
}

#[cfg(test)]
mod lod_transition_tests {
    use super::*;

    #[test]
    fn progressive_preview_keeps_request_pending_and_ignores_stale_frames() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("scan.xyz");
        std::fs::write(&source, "0 0 0\n1 0 0\n2 0 0\n3 0 0\n").unwrap();
        let cloud = pointcloud_core::open(&source, 4).unwrap();
        let first = IndexedPoint {
            point: cloud.points[0],
            ordinal: 0,
        };
        let second = IndexedPoint {
            point: cloud.points[1],
            ordinal: 1,
        };
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        studio.detail_pending = true;
        let revision = studio.revision;

        let _ = studio.update(Message::DetailPreview(revision, vec![(0, vec![first])]));
        assert!(studio.detail_pending);
        assert_ne!(studio.detail_loaded_revision, Some(revision));
        assert_eq!(studio.clouds[0].view_records().next().unwrap().ordinal, 0);

        studio.revision += 1;
        let _ = studio.update(Message::DetailPreview(revision, vec![(0, vec![second])]));
        assert_eq!(studio.clouds[0].view_records().next().unwrap().ordinal, 0);
        let current_revision = studio.revision;
        let _ = studio.update(Message::DetailReady(
            current_revision,
            Ok(vec![(0, vec![second])]),
        ));
        assert!(!studio.detail_pending);
        assert_eq!(studio.detail_loaded_revision, Some(current_revision));
        assert_eq!(studio.clouds[0].view_records().next().unwrap().ordinal, 1);
    }

    #[test]
    fn drag_release_starts_current_lod_without_a_second_debounced_request() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("scan.xyz");
        std::fs::write(&source, "0 0 0\n1 0 0\n2 0 0\n3 0 0\n").unwrap();
        let cloud = pointcloud_core::open(&source, 4).unwrap();
        let index = Arc::new(OctreeIndex::build(&cloud, IndexConfig::default()).unwrap());
        let point = IndexedPoint {
            point: cloud.points[0],
            ordinal: 0,
        };
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        studio.clouds[0].index = Some(index);
        studio.revision += 1;

        let _ = studio.update(Message::Pan(20.0, 0.0));
        assert!(!studio.detail_pending);
        let _ = studio.update(Message::NavigationFinished);
        assert!(studio.detail_pending);
        let revision = studio.revision;
        let _ = studio.update(Message::DetailReady(revision, Ok(vec![(0, vec![point])])));
        assert_eq!(studio.detail_loaded_revision, Some(revision));
        let _ = studio.update(Message::RefreshDetail(revision));
        assert!(!studio.detail_pending);

        let _ = studio.update(Message::Pan(20.0, 0.0));
        let old_revision = studio.revision - 1;
        studio.detail_pending = true;
        studio.detail_cancel = Arc::new(AtomicBool::new(false));
        let _ = studio.update(Message::NavigationFinished);
        assert!(studio.detail_cancel.load(Ordering::Relaxed));
        assert_eq!(studio.detail_urgent_revision, Some(studio.revision));
        let _ = studio.update(Message::DetailReady(
            old_revision,
            Err("Operation cancelled".into()),
        ));
        assert!(studio.detail_pending);
        assert_eq!(studio.detail_urgent_revision, None);
    }

    #[test]
    fn navigation_keeps_old_lod_until_matching_replacement_arrives() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("scan.xyz");
        std::fs::write(&source, "0 0 0\n1 0 0\n2 0 0\n3 0 0\n").unwrap();
        let mut cloud = pointcloud_core::open(&source, 4).unwrap();
        let records = cloud
            .points
            .iter()
            .copied()
            .zip(cloud.point_ordinals.iter().copied())
            .take(2)
            .map(|(point, ordinal)| IndexedPoint { point, ordinal })
            .collect::<Vec<_>>();
        let replacement_point = cloud.points[3];
        // LAS header loading has exact bounds and counts but no preview yet.
        cloud.points.clear();
        cloud.point_ordinals.clear();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        assert!(studio.clouds[0].cloud.points.is_empty());
        let old: Arc<[IndexedPoint]> = records.into();
        studio.clouds[0].detail_points = Some(Arc::clone(&old));
        let previous_request = Arc::clone(&studio.detail_cancel);

        for message in [
            Message::Orbit(20.0, 5.0),
            Message::Pan(50.0, 25.0),
            Message::Zoom(1.0, [400.0, 300.0], Size::new(800.0, 600.0)),
            Message::Budget(40_000),
            Message::SetSectionEnabled(true),
        ] {
            let _ = studio.update(message);
            assert!(Arc::ptr_eq(
                studio.clouds[0].detail_points.as_ref().unwrap(),
                &old
            ));
        }
        assert!(previous_request.load(Ordering::Relaxed));
        let stale_revision = studio.revision - 1;
        let _ = studio.update(Message::DetailReady(stale_revision, Ok(vec![(0, vec![])])));
        assert!(Arc::ptr_eq(
            studio.clouds[0].detail_points.as_ref().unwrap(),
            &old
        ));

        let replacement = vec![IndexedPoint {
            point: replacement_point,
            ordinal: 3,
        }];
        let _ = studio.update(Message::DetailReady(
            studio.revision,
            Ok(vec![(0, replacement)]),
        ));
        assert_eq!(studio.clouds[0].view_len(), 1);
        assert_eq!(studio.clouds[0].view_records().next().unwrap().ordinal, 3);
    }

    /// A window with one indexed scan of four points, and those points.
    fn indexed_studio(directory: &Path) -> (Studio, Vec<IndexedPoint>) {
        let source = directory.join("scan.xyz");
        std::fs::write(&source, "0 0 0\n1 0 0\n2 0 0\n3 0 0\n").unwrap();
        let cloud = pointcloud_core::open(&source, 4).unwrap();
        let index = Arc::new(OctreeIndex::build(&cloud, IndexConfig::default()).unwrap());
        let records = cloud
            .points
            .iter()
            .copied()
            .zip(cloud.point_ordinals.iter().copied())
            .map(|(point, ordinal)| IndexedPoint { point, ordinal })
            .collect();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        studio.clouds[0].index = Some(index);
        (studio, records)
    }

    /// A refinement of a 20,000-point scan seen from above that reads a
    /// quarter of its budget first, and what the whole scan puts in view.
    fn grid_refinement(directory: &Path) -> (LodRefinement, ScreenFill) {
        let source = directory.join("grid.xyz");
        let lines: String = (0..20_000)
            .map(|index| format!("{} {} 0\n", index % 200, index / 200))
            .collect();
        std::fs::write(&source, lines).unwrap();
        let cloud = pointcloud_core::open(&source, 20_000).unwrap();
        let tree = Arc::new(OctreeIndex::build(&cloud, IndexConfig::default()).unwrap());
        let projection = Projection::new(cloud.bounds, 0.0, 1.5, 1.0, [0.0; 2], 800.0, 600.0);
        let mut whole = ScreenFill::default();
        whole.add(&cloud.points, |point| point.xyz, projection, None);
        assert_eq!(whole.points, 20_000);
        (refinement_of(tree, projection, 5_000), whole)
    }

    /// A refinement of one scan, with all its points as the budget, that
    /// asks for `requested` points first.
    fn refinement_of(
        tree: Arc<OctreeIndex>,
        projection: Projection,
        requested: usize,
    ) -> LodRefinement {
        let total = usize::try_from(tree.root.total_points).unwrap();
        LodRefinement {
            sources: vec![(0, tree, CloudTransform::default(), 1.0)],
            source_weights: vec![(1.0, total)],
            requested: vec![requested],
            sampled_limits: vec![0],
            samples: vec![Vec::new()],
            section: None,
            projection,
            cancel: Arc::new(AtomicBool::new(false)),
            budget: total,
            deep_zoom: false,
            pace: Arc::default(),
            shown: ScreenFill::default(),
            drawn_elsewhere: 0,
            pass: 0,
        }
    }

    /// A flat scan of 512 by 512 points, indexed in sixteen leaves that each
    /// keep a preview next to their points.
    fn leafy_scan(directory: &Path) -> (PointCloud, Arc<OctreeIndex>) {
        let source = directory.join("leafy.xyz");
        let lines: String = (0..512 * 512)
            .map(|index| format!("{} {} 0\n", index % 512, index / 512))
            .collect();
        std::fs::write(&source, lines).unwrap();
        let cloud = pointcloud_core::open(&source, 512 * 512).unwrap();
        let config = IndexConfig {
            leaf_points: 20_000,
            ..IndexConfig::default()
        };
        let tree = Arc::new(OctreeIndex::build(&cloud, config).unwrap());
        (cloud, tree)
    }

    /// The scan seen from above, at a zoom of the framed view.
    fn from_above(cloud: &PointCloud, zoom: f32) -> Projection {
        Projection::new(cloud.bounds, 0.0, 1.5, zoom, [0.0; 2], 800.0, 600.0)
    }

    #[test]
    fn refinement_pass_feeds_the_read_pace() {
        let directory = tempfile::tempdir().unwrap();
        let (cloud, tree) = leafy_scan(directory.path());
        let total = cloud.points.len();
        let seed = LodPace::default().read_points_per_ms();

        let mut regular = refinement_of(Arc::clone(&tree), from_above(&cloud, 1.0), total);
        let pace = Arc::clone(&regular.pace);
        assert!(regular.advance().unwrap().is_none());
        assert_eq!(regular.finish()[0].1.len(), total);
        assert_ne!(pace.read_points_per_ms(), seed);

        // The exact scan at deep zoom is no measure of a regular pass.
        let mut exact = refinement_of(tree, from_above(&cloud, 1.0), total);
        exact.deep_zoom = true;
        assert!(exact.advance().unwrap().is_none());
        assert!(!exact.samples[0].is_empty());
        assert_eq!(exact.pace.read_points_per_ms(), seed);
    }

    #[test]
    fn preview_tier_follows_the_nodes_in_view() {
        let directory = tempfile::tempdir().unwrap();
        let (cloud, tree) = leafy_scan(directory.path());
        let total = cloud.points.len();
        let leaf = total / 16;
        let tier = |projection: Projection| {
            preview_tier_points(&tree.root, 1_000_000, |bounds| {
                projection.screen_span(bounds)
            })
        };
        // What the sampler itself reads for a view when asked for all there
        // is: the measure of which nodes it picks.
        let read = |projection: Projection| {
            tree.sample_lod_indexed(total, |bounds| projection.screen_span(bounds))
                .unwrap()
                .len()
        };

        // Framed, every leaf is in view and gives its preview.
        let framed = from_above(&cloud, 1.0);
        assert_eq!(read(framed), total);
        assert_eq!(tier(framed), Some(16 * lod_pace::LOD_NODE_PREVIEW_POINTS));

        // Zoomed in, only the leaves around the middle are.
        let close = from_above(&cloud, 0.2);
        let leaves = read(close) / leaf;
        assert!((1..16).contains(&leaves), "{leaves} leaves");
        assert_eq!(read(close), leaves * leaf);
        assert_eq!(
            tier(close),
            Some(leaves * lod_pace::LOD_NODE_PREVIEW_POINTS)
        );

        // From afar the sampler keeps to the root, which holds no more than
        // its preview, so a larger request costs nothing extra.
        let afar = from_above(&cloud, 8.0);
        assert_eq!(read(afar), lod_pace::LOD_NODE_PREVIEW_POINTS);
        assert_eq!(tier(afar), None);

        let away = Projection::new(cloud.bounds, 0.0, 1.5, 1.0, [5_000.0, 0.0], 800.0, 600.0);
        assert_eq!(tier(away), None);
    }

    #[test]
    fn walking_close_over_a_floor_reads_the_nodes_in_front() {
        let directory = tempfile::tempdir().unwrap();
        let (cloud, tree) = leafy_scan(directory.path());
        let total = cloud.points.len();
        let leaf = total / 16;
        // Half a unit above the floor, looking ahead and down along +X: the
        // floor fills the view, and the first column of leaves lies behind.
        let mut view = WalkView::new([200.0, 256.0, 0.5], 0.0);
        view.pitch = -0.6;
        let size = Size::new(800.0, 600.0);
        let projection = Projection::from_eye(
            cloud.bounds,
            view.eye,
            view.basis(),
            view.focal(size),
            size.width,
            size.height,
        );
        let in_view = |records: &[IndexedPoint]| {
            records
                .iter()
                .filter(|record| projection.project(record.point.xyz).is_some())
                .count()
        };
        let seen = cloud
            .points
            .iter()
            .filter(|point| projection.project(point.xyz).is_some())
            .count();
        assert!(seen > 10_000, "{seen}");
        assert!(source_lod_coverage(projection, tree.root.bounds, None).is_some());

        let span = |bounds| lod_node_span(CloudTransform::default(), None, projection, bounds);
        let sample = tree.sample_lod_indexed(total, span).unwrap();
        assert!(
            in_view(&sample) * 2 > seen,
            "{} of {seen}",
            in_view(&sample)
        );
        assert!(sample.len() <= total - 4 * leaf, "{}", sample.len());

        // Walking reads the leaves in view exactly.
        let mut walking = refinement_of(Arc::clone(&tree), projection, total);
        walking.deep_zoom = true;
        assert!(walking.advance().unwrap().is_none());
        let exact = &walking.finish()[0].1;
        assert_eq!(exact.len(), seen);
        assert_eq!(in_view(exact), seen);
    }

    #[test]
    fn first_pass_is_bound_by_the_previews_of_each_cloud() {
        let weights = [(900.0, 50_000_000), (100.0, 50_000_000)];
        let open = distribute_lod_budget(1_000_000, &weights);
        assert_eq!(
            first_pass_limits(1_000_000, &weights, |_, _| None),
            Some(open.clone())
        );
        // The cloud that fills the view has 300 leaves in it.
        assert_eq!(
            first_pass_limits(1_000_000, &weights, |slot, limit| {
                assert_eq!(limit, open[slot]);
                (slot == 0).then_some(300 * 2_048)
            }),
            Some(vec![300 * 2_048, open[1]])
        );
        // With 60 leaves in view of each cloud the previews are too few for
        // a first pass, and reading those leaves once is the shorter way.
        assert_eq!(
            first_pass_limits(1_000_000, &weights, |_, _| Some(60 * 2_048)),
            None
        );
    }

    #[test]
    fn first_read_stays_within_the_previews_or_is_the_only_read() {
        let directory = tempfile::tempdir().unwrap();
        let (cloud, tree) = leafy_scan(directory.path());
        let studio = Studio {
            budget: 6_000_000,
            ..Studio::default()
        };
        // As for a scan with far more points than the budget.
        let weights = [(1.0, 50_000_000)];
        let sources = [(0, tree, CloudTransform::default(), 1.0)];
        let limits = |zoom: f32, deep_zoom: bool| {
            studio.first_read_limits(
                &sources,
                &weights,
                from_above(&cloud, zoom),
                None,
                deep_zoom,
                ScreenFill::default(),
            )
        };

        // From afar only the root is read, which no limit makes slower.
        assert_eq!(limits(8.0, false), vec![lod_pace::LOD_FIRST_PASS_MIN]);
        // Framed, a first pass of that size would read all sixteen leaves in
        // full and the next pass would read them again.
        assert_eq!(limits(1.0, false), vec![6_000_000]);
        // The exact scan at deep zoom has its own way of reading.
        assert_eq!(limits(1.0, true), vec![lod_pace::LOD_FIRST_PASS_MIN]);
    }

    #[test]
    fn stray_points_do_not_widen_the_reach_of_a_first_pass() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("stray.xyz");
        // One point far from the scan makes the box of the cloud a hundred
        // times as wide as the scan itself.
        let lines: String = (0..10_000)
            .map(|index| format!("{} {} 0\n", index % 100, index / 100))
            .chain(["10000 10000 0\n".to_string()])
            .collect();
        std::fs::write(&source, lines).unwrap();
        let cloud = pointcloud_core::open(&source, 10_001).unwrap();
        let tree = Arc::new(OctreeIndex::build(&cloud, IndexConfig::default()).unwrap());
        let projection = from_above(&cloud, 1.0);
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        studio.budget = 6_000_000;
        let sources = [(0, tree, studio.clouds[0].transform, 1.0)];

        let shown = studio.shown_fill(&sources, projection, None);
        assert_eq!(shown.points, 10_001);
        let reach = studio.first_pass_reach(&sources, projection, None, false);
        assert_eq!(reach, shown.cell_count());
        // What is on screen already holds, so everything is read in one go.
        assert_eq!(plan_first_pass(10_001, 5_000, shown, reach), 10_001);

        // The box of the cloud promises far more of the viewport than a
        // first pass could fill; at deep zoom it is all there is to go by.
        let boxed = studio.first_pass_reach(&sources, projection, None, true);
        assert!(boxed > reach * 10, "{boxed} against {reach} cells");
        assert_eq!(plan_first_pass(10_001, 5_000, shown, boxed), 5_000);
    }

    #[test]
    fn withheld_preview_reads_on_to_the_final_set() {
        let directory = tempfile::tempdir().unwrap();
        let (mut withheld, whole) = grid_refinement(directory.path());
        withheld.shown = whole;
        assert!(withheld.advance().unwrap().is_none());
        assert_eq!(withheld.pass, 1);
        let details = withheld.finish();
        assert_eq!(details.len(), 1);
        assert_eq!(details[0].1.len(), 20_000);

        let (mut bare, _) = grid_refinement(directory.path());
        let preview = bare.advance().unwrap().unwrap();
        assert_eq!(preview[0].1.len(), 5_000);
        // The next pass reads this scan again, so the set was handed over
        // instead of copied.
        assert!(bare.samples[0].is_empty());
        assert!(bare.advance().unwrap().is_none());
        // Passes this small say nothing about the pace of the computer.
        assert_eq!(
            bare.pace.read_points_per_ms(),
            LodPace::default().read_points_per_ms()
        );
        assert_eq!(bare.finish()[0].1.len(), 20_000);
    }

    #[test]
    fn fresh_set_is_judged_as_the_renderer_would_draw_it() {
        let directory = tempfile::tempdir().unwrap();
        let (mut refinement, _) = grid_refinement(directory.path());
        refinement.sample_pass().unwrap();
        let alone = refinement.fill();
        assert_eq!(alone.points, 5_000);
        // Sets of visible clouds outside the request use the budget too, and
        // past it the renderer draws every second point of all of them.
        refinement.drawn_elsewhere = 30_000;
        let crowded = refinement.fill();
        assert_eq!(crowded.points, 2_500);
        assert_eq!(crowded.cell_count(), alone.cell_count());
    }

    #[test]
    fn cancelled_refinement_keeps_the_rich_set() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, records) = indexed_studio(directory.path());
        let rich: Arc<[IndexedPoint]> = records.into();
        studio.clouds[0].detail_points = Some(Arc::clone(&rich));
        let kept =
            |studio: &Studio| Arc::ptr_eq(studio.clouds[0].detail_points.as_ref().unwrap(), &rich);

        let _ = studio.update(Message::Orbit(20.0, 5.0));
        let _ = studio.update(Message::NavigationFinished);
        assert!(studio.detail_pending);
        assert!(kept(&studio));
        let old_revision = studio.revision;
        let running = Arc::clone(&studio.detail_cancel);

        let _ = studio.update(Message::Orbit(20.0, 5.0));
        assert!(running.load(Ordering::Relaxed));
        assert!(kept(&studio));
        let _ = studio.update(Message::DetailReady(
            old_revision,
            Err("Operation cancelled".into()),
        ));
        assert!(!studio.detail_pending);
        assert_ne!(studio.detail_loaded_revision, Some(studio.revision));
        assert!(kept(&studio));
    }

    #[test]
    fn release_without_movement_keeps_the_running_request() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = indexed_studio(directory.path());

        let _ = studio.update(Message::Pan(20.0, 0.0));
        let _ = studio.update(Message::NavigationFinished);
        let revision = studio.revision;
        assert!(studio.detail_pending);
        assert_eq!(studio.detail_request_revision, Some(revision));
        let running = Arc::clone(&studio.detail_cancel);

        let _ = studio.update(Message::NavigationFinished);
        assert!(Arc::ptr_eq(&running, &studio.detail_cancel));
        assert!(!running.load(Ordering::Relaxed));
        assert_eq!(studio.detail_urgent_revision, None);
        assert!(studio.detail_pending);

        // A request that was cancelled in the meantime is restarted at once.
        running.store(true, Ordering::Relaxed);
        let _ = studio.update(Message::NavigationFinished);
        assert_eq!(studio.detail_urgent_revision, Some(revision));
    }
}

#[cfg(test)]
mod viewport_drag_tests {
    use super::*;

    #[test]
    fn right_release_uses_final_pointer_even_without_move_events() {
        let start = UiPoint::new(10.0, 20.0);
        let drag = DragState {
            start,
            position: start,
            mode: DragMode::RightPending,
        };
        let message = finish_viewport_drag(
            mouse::Button::Right,
            drag,
            UiPoint::new(110.0, 60.0),
            Size::new(800.0, 600.0),
        );
        assert!(matches!(message, Some(Message::FinishPan(100.0, 40.0))));

        let message = finish_viewport_drag(
            mouse::Button::Right,
            drag,
            UiPoint::new(12.0, 23.0),
            Size::new(800.0, 600.0),
        );
        assert!(matches!(
            message,
            Some(Message::ShowContextMenu([12.0, 23.0]))
        ));
    }

    #[test]
    fn shift_middle_drag_orbits_while_plain_middle_drag_pans() {
        assert!(matches!(
            middle_drag_mode(iced::keyboard::Modifiers::SHIFT),
            DragMode::Turn
        ));
        assert!(matches!(
            middle_drag_mode(iced::keyboard::Modifiers::default()),
            DragMode::Pan
        ));
        assert!(matches!(
            middle_drag_mode(iced::keyboard::Modifiers::CTRL),
            DragMode::Pan
        ));

        let start = UiPoint::new(10.0, 20.0);
        let mut studio = Studio::default();
        let orbit = DragState {
            start,
            position: UiPoint::new(25.0, 25.0),
            mode: middle_drag_mode(iced::keyboard::Modifiers::SHIFT),
        };
        let message = finish_viewport_drag(
            mouse::Button::Middle,
            orbit,
            UiPoint::new(35.0, 30.0),
            Size::new(800.0, 600.0),
        )
        .unwrap();
        // The Shift + middle drag turns sideways the opposite way to the
        // left-button orbit and tilts the same way.
        assert!(matches!(message, Message::FinishOrbit(-10.0, 5.0)));
        let yaw = studio.yaw;
        let pitch = studio.pitch;
        let _ = studio.update(message);
        assert!((studio.yaw - yaw + 0.1).abs() < 0.0001);
        assert!((studio.pitch - pitch - 0.05).abs() < 0.0001);
        assert_eq!(studio.pan, [0.0, 0.0]);
    }

    #[test]
    fn release_applies_unreported_motion_to_camera_and_selection() {
        let start = UiPoint::new(10.0, 20.0);
        let mut studio = Studio::default();
        let drag = DragState {
            start,
            position: UiPoint::new(50.0, 40.0),
            mode: DragMode::Pan,
        };
        let message = finish_viewport_drag(
            mouse::Button::Middle,
            drag,
            UiPoint::new(60.0, 50.0),
            Size::new(800.0, 600.0),
        )
        .unwrap();
        assert!(matches!(message, Message::FinishPan(10.0, 10.0)));
        let _ = studio.update(message);
        assert_eq!(studio.pan, [10.0, 10.0]);

        let orbit = DragState {
            start,
            position: UiPoint::new(25.0, 25.0),
            mode: DragMode::Orbit,
        };
        let message = finish_viewport_drag(
            mouse::Button::Left,
            orbit,
            UiPoint::new(35.0, 30.0),
            Size::new(800.0, 600.0),
        )
        .unwrap();
        assert!(matches!(message, Message::FinishOrbit(10.0, 5.0)));
        let yaw = studio.yaw;
        let pitch = studio.pitch;
        let _ = studio.update(message);
        assert!((studio.yaw - yaw - 0.1).abs() < 0.0001);
        assert!((studio.pitch - pitch - 0.05).abs() < 0.0001);

        let selection = DragState {
            start,
            position: start,
            mode: DragMode::Select,
        };
        let message = finish_viewport_drag(
            mouse::Button::Left,
            selection,
            UiPoint::new(80.0, 90.0),
            Size::new(800.0, 600.0),
        );
        assert!(matches!(
            message,
            Some(Message::BoxSelect {
                end: [80.0, 90.0],
                ..
            })
        ));
    }
}

#[cfg(test)]
mod import_api_tests {
    use super::*;

    fn send(studio: &mut Studio, command: native_api::ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(native_api::ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    #[test]
    fn cancelling_import_discards_a_late_successful_result() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.xyz");
        std::fs::write(&path, "1 2 3\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let cancel = Arc::new(AtomicBool::new(false));
        studio.imports.insert(
            7,
            ImportJob {
                path,
                decoded: Arc::new(AtomicU64::new(1)),
                cancel: Arc::clone(&cancel),
            },
        );

        let status = send(&mut studio, native_api::ApiCommand::Status);
        assert_eq!(status["result"]["imports"][0]["decoded"], 1);
        let response = send(&mut studio, native_api::ApiCommand::CancelImport { id: 7 });
        assert_eq!(response["cancelling"], true);
        assert!(cancel.load(Ordering::Relaxed));
        let _ = studio.update(Message::ImportLoaded(7, Ok(cloud)));
        assert!(studio.clouds.is_empty());
        assert!(studio.imports.is_empty());
        assert!(studio.status.starts_with("Import cancelled:"));
    }

    #[test]
    fn indexed_import_shows_preview_before_attaching_finished_octree() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.xyz");
        std::fs::write(&path, "1 2 3\n2 3 4\n3 4 5\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let index = Arc::new(
            OctreeIndex::build_cached(
                &cloud,
                IndexConfig {
                    scratch_dir: Some(dir.path().join("cache")),
                    ..IndexConfig::default()
                },
            )
            .unwrap(),
        );
        let mut studio = Studio {
            index_pending: true,
            ..Studio::default()
        };
        studio.imports.insert(
            17,
            ImportJob {
                path,
                decoded: Arc::new(AtomicU64::new(3)),
                cancel: Arc::clone(&studio.index_cancel),
            },
        );
        assert!(studio.indexing_during_import());

        let _ = studio.update(Message::IndexedImportPreview(17, Arc::clone(&cloud)));
        assert!(studio.imports.is_empty());
        assert!(!studio.indexing_during_import());
        assert_eq!(studio.clouds.len(), 1);
        assert!(studio.clouds[0].index.is_none());
        assert!(studio.clouds[0].index_building);
        assert_eq!(studio.clouds[0].index_import_id, Some(17));

        let preview = Arc::clone(&studio.clouds[0].cloud);
        let _ = studio.update(Message::IndexedImportReady(17, Ok((cloud, index))));
        assert!(!studio.index_pending);
        assert!(studio.clouds[0].index.is_some());
        assert!(!studio.clouds[0].index_building);
        assert_eq!(studio.clouds[0].index_import_id, None);
        // A checked preview stays in place when its octree arrives.
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &preview));
    }

    #[test]
    fn spread_preview_stands_in_for_the_metadata_until_the_import_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.xyz");
        std::fs::write(&path, "1 2 3\n2 3 4\n3 4 5\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 3).unwrap());
        let mut header = (*cloud).clone();
        header.points.clear();
        header.point_ordinals.clear();
        header.provisional = true;
        let header = Arc::new(header);
        let mut spread = (*cloud).clone();
        spread.points.truncate(2);
        spread.point_ordinals = vec![u64::MAX; 2];
        spread.bounds.max = [2.0, 3.0, 4.0];
        spread.provisional = true;
        let spread = Arc::new(spread);

        let mut studio = Studio::default();
        let job = || ImportJob {
            path: path.clone(),
            decoded: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        // A snapshot takes the place of the layer of metadata.
        studio.imports.insert(31, job());
        let _ = studio.update(Message::HeaderLoaded(31, Ok(Arc::clone(&header))));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &header));
        let _ = studio.update(Message::ImportSnapshot(31, Arc::clone(&spread)));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &spread));
        assert_eq!(studio.clouds.len(), 1);

        let _ = studio.update(Message::ImportLoaded(31, Ok(Arc::clone(&cloud))));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &cloud));
        assert_eq!(studio.clouds.len(), 1);
        // A snapshot that arrives after the checked cloud is dropped.
        let _ = studio.update(Message::ImportSnapshot(31, Arc::clone(&spread)));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &cloud));

        // A source without metadata gets its layer from the first snapshot,
        // and each later one replaces it.
        let mut studio = Studio::default();
        studio.imports.insert(32, job());
        let _ = studio.update(Message::ImportSnapshot(32, Arc::clone(&spread)));
        assert_eq!(studio.clouds.len(), 1);
        let later = Arc::new((*spread).clone());
        let _ = studio.update(Message::ImportSnapshot(32, Arc::clone(&later)));
        assert_eq!(studio.clouds.len(), 1);
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &later));
        let _ = studio.update(Message::ImportLoaded(32, Ok(Arc::clone(&cloud))));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &cloud));
        assert_eq!(studio.clouds.len(), 1);

        // A failed import closes the layer, whichever of the two it showed.
        let mut studio = Studio::default();
        studio.imports.insert(33, job());
        let _ = studio.update(Message::HeaderLoaded(33, Ok(header)));
        let _ = studio.update(Message::ImportSnapshot(33, spread));
        let _ = studio.update(Message::ImportLoaded(33, Err("damaged".into())));
        assert!(studio.clouds.is_empty());
    }

    #[test]
    fn indexed_import_replaces_a_loose_preview_with_the_checked_cloud() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.xyz");
        std::fs::write(&path, "1 2 3\n2 3 4\n3 4 5\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let index = Arc::new(
            OctreeIndex::build_cached(
                &cloud,
                IndexConfig {
                    scratch_dir: Some(dir.path().join("cache")),
                    ..IndexConfig::default()
                },
            )
            .unwrap(),
        );
        // Sampled before the full pass: the bounds miss the farthest point.
        let mut loose = (*cloud).clone();
        loose.bounds.max = [2.0, 3.0, 4.0];
        loose.provisional = true;
        let loose = Arc::new(loose);
        let preview = |studio: &mut Studio, id: u64| {
            studio.index_pending = true;
            studio.index_cancel = Arc::new(AtomicBool::new(false));
            studio.imports.insert(
                id,
                ImportJob {
                    path: path.clone(),
                    decoded: Arc::new(AtomicU64::new(0)),
                    cancel: Arc::clone(&studio.index_cancel),
                },
            );
            let _ = studio.update(Message::IndexedImportPreview(id, Arc::clone(&loose)));
            assert!(studio.clouds[0].cloud.provisional);
        };

        let mut studio = Studio::default();
        preview(&mut studio, 23);
        // The user moved the camera while the scan was still being read.
        studio.yaw = 1.0;
        studio.zoom = 0.4;
        let _ = studio.update(Message::IndexedImportReady(
            23,
            Ok((Arc::clone(&cloud), Arc::clone(&index))),
        ));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &cloud));
        assert!(studio.clouds[0].index.is_some());
        assert_eq!(studio.clouds.len(), 1);
        assert_eq!(studio.yaw, 1.0);

        // While the scan is read, each later look replaces the one before.
        let mut studio = Studio::default();
        preview(&mut studio, 27);
        let later = Arc::new((*loose).clone());
        let _ = studio.update(Message::IndexedImportPreview(27, Arc::clone(&later)));
        assert_eq!(studio.clouds.len(), 1);
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &later));
        assert!(studio.clouds[0].index_building);

        // A full pass that fails leaves no layer of unchecked points behind.
        let mut studio = Studio::default();
        preview(&mut studio, 24);
        let _ = studio.update(Message::IndexedImportReady(24, Err("damaged".into())));
        assert!(studio.clouds.is_empty());
        assert_eq!(studio.status, "Import or octree failed: damaged");

        // Cancelled while reading: the same.
        let mut studio = Studio::default();
        preview(&mut studio, 25);
        let _ = studio.update(Message::CancelIndex);
        let _ = studio.update(Message::IndexedImportReady(25, Err("cancelled".into())));
        assert!(studio.clouds.is_empty());

        // Cancelled as the pass finished: its checked cloud stays, unindexed.
        let mut studio = Studio::default();
        preview(&mut studio, 26);
        let _ = studio.update(Message::CancelIndex);
        let _ = studio.update(Message::IndexedImportReady(
            26,
            Ok((Arc::clone(&cloud), index)),
        ));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &cloud));
        assert!(studio.clouds[0].index.is_none());

        // The core refuses to index a cloud that was not checked.
        assert!(OctreeIndex::build_cached(
            &loose,
            IndexConfig {
                scratch_dir: Some(dir.path().join("cache")),
                ..IndexConfig::default()
            },
        )
        .is_err());
    }

    #[test]
    fn indexed_import_finishing_before_preview_does_not_add_a_duplicate_layer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.xyz");
        std::fs::write(&path, "1 2 3\n2 3 4\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let index = Arc::new(
            OctreeIndex::build_cached(
                &cloud,
                IndexConfig {
                    scratch_dir: Some(dir.path().join("cache")),
                    ..IndexConfig::default()
                },
            )
            .unwrap(),
        );
        let mut studio = Studio {
            index_pending: true,
            ..Studio::default()
        };
        studio.imports.insert(
            20,
            ImportJob {
                path,
                decoded: Arc::new(AtomicU64::new(2)),
                cancel: Arc::clone(&studio.index_cancel),
            },
        );

        let _ = studio.update(Message::IndexedImportReady(
            20,
            Ok((Arc::clone(&cloud), index)),
        ));
        let _ = studio.update(Message::IndexedImportPreview(20, cloud));
        assert!(studio.imports.is_empty());
        assert_eq!(studio.clouds.len(), 1);
        assert!(studio.clouds[0].index.is_some());
        assert!(!studio.index_pending);
    }

    #[test]
    fn indexed_import_cancel_discards_unshown_cloud_but_keeps_shown_preview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan.xyz");
        std::fs::write(&path, "1 2 3\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 1).unwrap());
        let mut studio = Studio {
            index_pending: true,
            ..Studio::default()
        };
        studio.imports.insert(
            18,
            ImportJob {
                path: path.clone(),
                decoded: Arc::new(AtomicU64::new(1)),
                cancel: Arc::clone(&studio.index_cancel),
            },
        );
        let _ = studio.update(Message::CancelImport(18));
        let _ = studio.update(Message::IndexedImportPreview(18, Arc::clone(&cloud)));
        let _ = studio.update(Message::IndexedImportReady(18, Err("cancelled".into())));
        assert!(studio.clouds.is_empty());
        assert!(!studio.index_pending);

        studio.index_pending = true;
        studio.index_cancel = Arc::new(AtomicBool::new(false));
        studio.imports.insert(
            19,
            ImportJob {
                path,
                decoded: Arc::new(AtomicU64::new(1)),
                cancel: Arc::clone(&studio.index_cancel),
            },
        );
        let _ = studio.update(Message::IndexedImportPreview(19, cloud));
        let _ = studio.update(Message::CancelIndex);
        let _ = studio.update(Message::IndexedImportReady(19, Err("cancelled".into())));
        assert_eq!(studio.clouds.len(), 1);
        assert!(studio.clouds[0].index.is_none());
        assert!(!studio.clouds[0].index_building);
        assert!(!studio.index_pending);

        studio.index_pending = true;
        studio.index_cancel = Arc::new(AtomicBool::new(false));
        studio.imports.insert(
            21,
            ImportJob {
                path: studio.clouds[0].cloud.path.clone(),
                decoded: Arc::new(AtomicU64::new(1)),
                cancel: Arc::clone(&studio.index_cancel),
            },
        );
        let cloud = Arc::clone(&studio.clouds[0].cloud);
        let _ = studio.update(Message::IndexedImportPreview(21, cloud));
        let _ = studio.update(Message::Remove(1));
        assert!(studio.index_cancel.load(Ordering::Relaxed));
        let _ = studio.update(Message::IndexedImportReady(21, Err("cancelled".into())));
        assert_eq!(studio.clouds.len(), 1);
        assert_eq!(studio.status, "Octree build cancelled");
    }

    #[test]
    fn expanded_folder_opens_each_scan_once_and_answers_the_api() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().to_path_buf();
        let scans: Vec<PathBuf> = ["scan 2.xyz", "scan 10.xyz"]
            .iter()
            .map(|name| {
                let path = folder.join(name);
                std::fs::write(&path, "1 2 3\n").unwrap();
                path
            })
            .collect();
        let expansion = project_open::expand(std::slice::from_ref(&folder), &[]);
        let mut studio = Studio::default();

        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.update(Message::ApiScansExpanded(
            reply,
            folder.clone(),
            expansion.clone(),
        ));
        let response = receive.recv().unwrap();
        assert_eq!(response["ok"], true);
        assert_eq!(response["files"], json!(scans));
        assert_eq!(response["import_ids"], json!([1, 2]));
        assert_eq!(response["import_id"], 2);
        assert_eq!(studio.imports.len(), 2);
        assert_eq!(studio.status, "Opening 2 scans…");

        // Both scans are still being imported, so nothing is opened twice.
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.update(Message::ApiScansExpanded(reply, folder, expansion));
        let response = receive.recv().unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(response["error"], "No scans opened: 2 already open");
        assert_eq!(studio.imports.len(), 2);
    }
}

#[cfg(test)]
mod camera_api_tests {
    use super::*;

    fn send(studio: &mut Studio, command: native_api::ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(native_api::ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    #[test]
    fn screen_pick_rejects_invalid_coordinates_without_starting_a_job() {
        let mut studio = Studio::default();
        let invalid = send(
            &mut studio,
            native_api::ApiCommand::PickScreen {
                pointer: [f32::NAN, 10.0],
                radius: None,
            },
        );
        assert_eq!(invalid["ok"], false);
        let outside_x = studio.viewport_size.width + 1.0;
        let outside = send(
            &mut studio,
            native_api::ApiCommand::PickScreen {
                pointer: [outside_x, 10.0],
                radius: Some(8.0),
            },
        );
        assert_eq!(outside["ok"], false);
        assert!(studio.api_jobs.is_empty());
    }

    #[test]
    fn exact_camera_and_zoom_all_validate_and_update_one_view() {
        let mut studio = Studio::default();
        let accepted = send(
            &mut studio,
            native_api::ApiCommand::SetCamera {
                yaw: 0.4,
                pitch: -0.2,
                zoom: 0.01,
                pan: [120.0, -80.0],
                orbit_point: None,
            },
        );
        assert_eq!(accepted["ok"], true);
        assert_eq!(studio.yaw, 0.4);
        assert_eq!(studio.pitch, -0.2);
        assert_eq!(studio.zoom, 0.01);
        assert_eq!(studio.pan, [120.0, -80.0]);
        assert_eq!(studio.view_label, "CUSTOM");

        for command in [
            native_api::ApiCommand::SetCamera {
                yaw: 0.4,
                pitch: -0.2,
                zoom: 0.0,
                pan: [0.0, 0.0],
                orbit_point: None,
            },
            native_api::ApiCommand::SetCamera {
                yaw: 0.4,
                pitch: -0.2,
                zoom: 1.0,
                pan: [f32::NAN, 0.0],
                orbit_point: None,
            },
            native_api::ApiCommand::SetCamera {
                yaw: 0.4,
                pitch: -0.2,
                zoom: 1.0,
                pan: [0.0, 0.0],
                orbit_point: Some(Some([0.0, f64::INFINITY, 0.0])),
            },
        ] {
            assert_eq!(send(&mut studio, command)["ok"], false);
            assert_eq!(studio.zoom, 0.01);
            assert_eq!(studio.pan, [120.0, -80.0]);
            assert_eq!(studio.orbit_point, None);
        }

        // The orbit point is kept when left out and cleared by null.
        let camera = |orbit_point: &str| {
            serde_json::from_str::<native_api::ApiCommand>(&format!(
                r#"{{"command":"set_camera","yaw":0.4,"pitch":-0.2,"zoom":0.01,"pan":[120,-80]{orbit_point}}}"#
            ))
            .unwrap()
        };
        let set = send(&mut studio, camera(r#","orbit_point":[1,2,3]"#));
        assert_eq!(set["camera"]["orbit_point"], json!([1.0, 2.0, 3.0]));
        let _ = send(&mut studio, camera(""));
        assert_eq!(studio.orbit_point, Some([1.0, 2.0, 3.0]));
        let cleared = send(&mut studio, camera(r#","orbit_point":null"#));
        assert_eq!(cleared["camera"]["orbit_point"], Value::Null);
        let _ = send(&mut studio, camera(r#","orbit_point":[1,2,3]"#));

        let fitted = send(&mut studio, native_api::ApiCommand::ZoomAll);
        assert_eq!(fitted["ok"], true);
        assert_eq!(studio.zoom, 1.0);
        assert_eq!(studio.pan, [0.0, 0.0]);
        assert_eq!(studio.view_label, "ISOMETRIC");
        // Zoom all turns about the centre of the model again.
        assert_eq!(fitted["camera"]["orbit_point"], Value::Null);
    }

    #[test]
    fn the_camera_turns_about_the_orbit_point_picked_on_screen() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("floor.xyz");
        // A floor of 41 by 21 points one unit apart, and a post at one end.
        let mut lines: String = (0..41 * 21)
            .map(|index| format!("{} {} 0\n", index % 41, index / 41))
            .collect();
        lines.push_str("38 18 4\n");
        std::fs::write(&source, lines).unwrap();
        let cloud = pointcloud_core::open(&source, 10_000).unwrap();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        studio.viewport_size = Size::new(800.0, 600.0);
        let scene = combined_bounds(&studio.clouds).unwrap();
        let post = [38.0, 18.0, 4.0];
        let on_screen = |studio: &Studio| {
            let (x, y, _) = studio
                .projection(scene, 800.0, 600.0)
                .project(post)
                .unwrap();
            [x, y]
        };

        // Without an orbit point the post moves while the camera turns.
        let before = on_screen(&studio);
        let turned = send(
            &mut studio,
            native_api::ApiCommand::Orbit {
                yaw: 0.3,
                pitch: 0.1,
            },
        );
        assert_eq!(turned["ok"], true);
        let after = on_screen(&studio);
        assert!((after[0] - before[0]).hypot(after[1] - before[1]) > 20.0);

        // Picked on screen, the post stays where it is.
        let picked = send(
            &mut studio,
            native_api::ApiCommand::PickOrbitPoint {
                pointer: [after[0] + 2.0, after[1] - 1.0],
            },
        );
        assert_eq!(picked["orbit_point"], json!(post));
        for (yaw, pitch) in [(0.4, 0.0), (-1.2, 0.3), (2.0, -0.5)] {
            let turned = send(&mut studio, native_api::ApiCommand::Orbit { yaw, pitch });
            assert_eq!(turned["camera"]["orbit_point"], json!(post));
            let now = on_screen(&studio);
            assert!(
                (now[0] - after[0]).abs() < 0.05 && (now[1] - after[1]).abs() < 0.05,
                "{now:?} {after:?}"
            );
        }

        // Nothing under the pointer: the centre of the model again.
        let missed = send(
            &mut studio,
            native_api::ApiCommand::PickOrbitPoint {
                pointer: [2.0, 2.0],
            },
        );
        assert_eq!(missed["orbit_point"], Value::Null);
        assert_eq!(studio.orbit_point, None);
        let outside = send(
            &mut studio,
            native_api::ApiCommand::PickOrbitPoint {
                pointer: [900.0, 2.0],
            },
        );
        assert_eq!(outside["ok"], false);
    }

    #[test]
    fn the_scene_is_measured_as_it_was_last_drawn() {
        let studio = Studio::default();
        assert_eq!(studio.scene_size(), studio.viewport_size);
        // Drawn smaller than the viewport last reported, without the pointer
        // over it to report the new size.
        let drawn = Size::new(915.0, 694.0);
        studio
            .views_overlay()
            .drawn_at(Rectangle::new(UiPoint::new(280.0, 150.0), drawn));
        assert_ne!(studio.viewport_size, drawn);
        assert_eq!(studio.scene_size(), drawn);
    }

    #[test]
    fn a_double_click_without_moving_asks_for_the_orbit_point() {
        let start = UiPoint::new(200.0, 150.0);
        let size = Size::new(800.0, 600.0);
        let click = DragState {
            start,
            position: start,
            mode: DragMode::Orbit,
        };
        let first = Instant::now();
        let mut last = None;
        assert!(orbit_click(&mut last, click, start, first, size).is_none());
        let second = first + Duration::from_millis(250);
        assert!(matches!(
            orbit_click(&mut last, click, UiPoint::new(201.0, 151.0), second, size),
            Some(Message::PickOrbitPoint([201.0, 151.0], _))
        ));
        // A third click starts a new pair.
        assert!(orbit_click(&mut last, click, start, second, size).is_none());
        // A drag in between is no click.
        let mut last = None;
        assert!(orbit_click(&mut last, click, start, first, size).is_none());
        assert!(orbit_click(&mut last, click, UiPoint::new(260.0, 150.0), second, size).is_none());
        assert!(orbit_click(&mut last, click, start, second, size).is_none());
        // Measuring keeps its own clicks.
        let measuring = DragState {
            mode: DragMode::MeasurePending,
            ..click
        };
        let mut last = None;
        assert!(orbit_click(&mut last, measuring, start, first, size).is_none());
        assert!(orbit_click(&mut last, measuring, start, second, size).is_none());
    }

    #[test]
    fn project_list_selects_a_range_with_shift_and_acts_on_it() {
        use iced::keyboard::Modifiers;

        let directory = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        // Opened out of name order; the list shows scan 1, 2, 3 and 10.
        for name in ["scan 10.xyz", "scan 2.xyz", "scan 1.xyz", "scan 3.xyz"] {
            let path = directory.path().join(name);
            std::fs::write(&path, "0 0 0\n1 0 0\n").unwrap();
            let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
            let _ = studio.update(Message::Loaded(Ok(cloud)));
        }
        assert_eq!(studio.layer_order(), [2, 1, 3, 0]);
        let picked = |studio: &Studio| -> Vec<usize> {
            (0..studio.clouds.len())
                .filter(|cloud| studio.clouds[*cloud].picked)
                .collect()
        };

        let _ = studio.update(Message::LayerClick(1));
        assert_eq!(picked(&studio), [1]);
        assert_eq!(studio.active, Some(1));

        // Shift extends from the active row to the clicked one, as listed.
        let _ = studio.update(Message::Modifiers(Modifiers::SHIFT));
        let _ = studio.update(Message::LayerClick(0));
        assert_eq!(picked(&studio), [0, 1, 3]);
        assert_eq!(studio.active, Some(1));
        let _ = studio.update(Message::LayerClick(2));
        assert_eq!(picked(&studio), [1, 2]);

        // Ctrl adds or drops a single row.
        let _ = studio.update(Message::Modifiers(Modifiers::COMMAND));
        let _ = studio.update(Message::LayerClick(0));
        assert_eq!(picked(&studio), [0, 1, 2]);
        let _ = studio.update(Message::LayerClick(0));
        assert_eq!(picked(&studio), [1, 2]);
        let _ = studio.update(Message::LayerClick(0));

        // A control on a selected row acts on the selection, on another row
        // only on that row; the single-cloud messages stay single.
        let _ = studio.update(Message::Modifiers(Modifiers::default()));
        let _ = studio.update(Message::LayerVisible(2, false));
        let visible = |studio: &Studio| -> Vec<bool> {
            studio.clouds.iter().map(|entry| entry.visible).collect()
        };
        assert_eq!(visible(&studio), [false, false, false, true]);
        let _ = studio.update(Message::LayerVisible(3, false));
        let _ = studio.update(Message::SetVisible(1, true));
        assert_eq!(visible(&studio), [false, true, false, false]);

        let _ = studio.update(Message::LayerRemove(1));
        assert_eq!(studio.clouds.len(), 1);
        assert_eq!(display_name(&studio.clouds[0].cloud.path), "scan 3.xyz");
        assert_eq!(studio.active, Some(0));

        // Keys released while another window has focus are forgotten.
        let _ = studio.update(Message::Modifiers(Modifiers::SHIFT));
        let _ = studio.update(Message::WalkStop);
        assert!(studio.modifiers.is_empty() && !studio.walk_fast);
    }

    #[test]
    fn camera_api_lists_and_restores_only_the_active_scan_views() {
        let directory = tempfile::tempdir().unwrap();
        let first_path = directory.path().join("first.xyz");
        let second_path = directory.path().join("second.xyz");
        std::fs::write(&first_path, "0 0 0\n1 0 0\n").unwrap();
        std::fs::write(&second_path, "0 1 0\n1 1 0\n").unwrap();
        let first = Arc::new(pointcloud_core::open(&first_path, 10).unwrap());
        let second = Arc::new(pointcloud_core::open(&second_path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(first)));
        let _ = studio.update(Message::Loaded(Ok(second)));
        studio.views.list.push(camera_views::SavedView::camera(
            camera_views::source_key(&first_path),
            "First entrance",
            0.5,
            0.25,
            2.0,
            [12.0, -8.0],
        ));
        studio.views.list.push(camera_views::SavedView::camera(
            camera_views::source_key(&second_path),
            "Second entrance",
            -0.5,
            0.1,
            3.0,
            [4.0, 5.0],
        ));

        let _ = studio.update(Message::Select(0));
        let listed = send(&mut studio, native_api::ApiCommand::ListCameraViews);
        assert_eq!(listed["views"].as_array().unwrap().len(), 1);
        assert_eq!(listed["views"][0]["name"], "First entrance");
        let restored = send(
            &mut studio,
            native_api::ApiCommand::RestoreCameraView {
                name: "first ENTRANCE".into(),
            },
        );
        assert_eq!(restored["ok"], true);
        assert_eq!(studio.yaw, 0.5);
        assert_eq!(studio.pan, [12.0, -8.0]);
        assert_eq!(studio.view_label, "SAVED VIEW");
        assert_eq!(
            send(
                &mut studio,
                native_api::ApiCommand::RestoreCameraView {
                    name: "Second entrance".into(),
                },
            )["ok"],
            false
        );
    }
}

#[cfg(test)]
mod duplicate_layer_tests {
    use super::*;

    #[test]
    fn background_results_update_their_own_copy_of_a_source() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("same.xyz");
        std::fs::write(&path, "0 0 0\n1 0 0\n2 0 0\n3 0 0\n").unwrap();
        let first = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let second = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let refined = Arc::new(pointcloud_core::open(&path, 4).unwrap());
        std::fs::create_dir_all(directory.path().join("index")).unwrap();
        let index = Arc::new(
            OctreeIndex::build(
                &first,
                IndexConfig {
                    scratch_dir: Some(directory.path().join("index")),
                    ..IndexConfig::default()
                },
            )
            .unwrap(),
        );
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&first))));
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&second))));
        assert_eq!(studio.clouds.len(), 2);

        let _ = studio.update(Message::Refined(
            Arc::clone(&first),
            Ok(Arc::clone(&refined)),
        ));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &refined));
        assert!(Arc::ptr_eq(&studio.clouds[1].cloud, &second));

        let _ = studio.update(Message::CachedIndexReady(
            first,
            Ok(Some(Arc::clone(&index))),
        ));
        assert!(studio.clouds[0].index.is_some());
        assert!(studio.clouds[1].index.is_none());

        let _ = studio.update(Message::CachedIndexReady(second, Ok(Some(index))));
        assert!(studio.clouds[1].index.is_some());
    }
}

#[cfg(test)]
mod selection_scene_change_tests {
    use super::*;

    #[test]
    fn opening_hiding_or_removing_a_layer_stops_stale_selection() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.xyz");
        std::fs::write(&path, "0 0 0\n1 0 0\n").unwrap();
        let mut studio = Studio::default();
        let first = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let second = Arc::new(pointcloud_core::open(&path, 2).unwrap());
        let _ = studio.update(Message::Loaded(Ok(first)));

        studio.selection_pending = true;
        studio.selection_cancel = Arc::new(AtomicBool::new(false));
        let _ = studio.load(path.clone());
        assert!(studio.selection_cancel.load(Ordering::Relaxed));
        studio.selection_pending = false;

        for change in [0, 1, 2] {
            studio.selection_pending = true;
            studio.selection_cancel = Arc::new(AtomicBool::new(false));
            let cancel = Arc::clone(&studio.selection_cancel);
            let revision = studio.revision;
            let _ = studio.update(match change {
                0 => Message::Loaded(Ok(Arc::clone(&second))),
                1 => Message::SetVisible(1, false),
                _ => Message::Remove(0),
            });
            assert!(cancel.load(Ordering::Relaxed));
            assert!(studio.revision > revision);
            let _ = studio.update(Message::SelectionReady(revision, Ok(Vec::new())));
            assert!(!studio.selection_pending);
            assert_eq!(studio.status, "Selection cancelled");
        }
        assert_eq!(studio.clouds.len(), 1);
        assert!(studio.clouds[0].selection.is_none());
    }
}
