use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod bag_map;
mod camera_views;
mod cloud_centroid;
mod cloud_transform;
mod gpu_viewport;
mod native_api;
mod native_chrome;
mod opencad_properties;
mod opencad_ribbon;
mod preferences;
mod selection;
mod ui_theme;
mod view_cube;

use bag_map::{BagMap, MapView, TileKey};
use camera_views::SavedView;
use cloud_transform::CloudTransform;
use iced::futures::SinkExt;
use iced::mouse;
use iced::widget::canvas::{self, event, Canvas, Frame, Geometry};
use iced::widget::{
    button, checkbox, column, container, image, pick_list, row, scrollable, slider, stack, svg,
    text, text_input, tooltip,
};
use iced::{Color, Element, Fill, Font, Point as UiPoint, Rectangle, Renderer, Size, Task, Theme};
use pointcloud_core::{
    BagBounds, BagLod, Bounds, ExportFormat, IndexConfig, IndexProgress, IndexStage, IndexedPoint,
    MeshGeometry, OctreeIndex, Point, PointCloud, SurfaceMeshConfig,
};
use preferences::{MAX_POINT_BUDGET, MIN_POINT_BUDGET};
#[cfg(test)]
use selection::select_world;
use selection::{
    pick_full_transformed, pick_indexed_transformed, select_full_cancellable,
    select_world_cancellable, ClassFilter, ClassVisibility, DeletionMask, PickTarget, Projection,
    ScreenRect, SelectionMask, SelectionSource,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ui_theme::UiTheme;

const LOAD_SAMPLE_LIMIT: usize = 100_000;
const FAST_LOD_PREVIEW_LIMIT: usize = 250_000;
const EXACT_VISIBLE_LOD_ZOOM: f32 = 0.05;
const MAX_EXACT_VISIBLE_LOD_CANDIDATES: u64 = 2_000_000;
const AUTO_INDEX_MIN_POINTS: u64 = 1_000_000;
const ASPRS_CLASSIFICATIONS: &[(u8, &str)] = &[
    (0, "Never classified"),
    (1, "Unassigned"),
    (2, "Ground"),
    (3, "Low vegetation"),
    (4, "Medium vegetation"),
    (5, "High vegetation"),
    (6, "Building"),
    (7, "Low point / noise"),
    (9, "Water"),
    (10, "Rail"),
    (11, "Road surface"),
    (13, "Wire guard"),
    (14, "Wire conductor"),
    (15, "Transmission tower"),
    (17, "Bridge deck"),
];
const BAG3D_MESH_COMMENTS: &[&str] = &[
    "© 3DBAG door tudelft3d en 3DGI · CC BY 4.0",
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
    section: Bounds,
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
                    deleted.is_none_or(|mask| !mask.contains(ordinal))
                        && (0..3).all(|axis| {
                            xyz[axis] >= section.min[axis] && xyz[axis] <= section.max[axis]
                        })
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
            (0..3)
                .all(|axis| {
                    point.xyz[axis] >= section.min[axis] && point.xyz[axis] <= section.max[axis]
                })
                .then_some(point)
        })
    }
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

fn is_bag3d_obj(path: &std::path::Path) -> bool {
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"))
    {
        return false;
    }
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut first = String::new();
    BufReader::new(file).read_line(&mut first).is_ok_and(|_| {
        first.starts_with("# © 3DBAG door tudelft3d en 3DGI")
            || first.starts_with("# 3DBAG by tudelft3d and 3DGI")
    })
}

fn main() -> iced::Result {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
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
        let cloud = if is_las {
            pointcloud_core::open_las_header(&source)
        } else {
            pointcloud_core::open(&source, 100_000)
        };
        match cloud.and_then(|cloud| OctreeIndex::build_cached(&cloud, IndexConfig::default())) {
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
        let (Some(source), Some(limits), Some(destination), None) =
            (args.next(), args.next(), args.next(), args.next())
        else {
            eprintln!(
                "Usage: open-pointcloud-studio --section INPUT XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX OUTPUT"
            );
            std::process::exit(2);
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
        let section = Bounds {
            min: [values[0], values[1], values[2]],
            max: [values[3], values[4], values[5]],
        };
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
    if first.as_deref() == Some(OsStr::new("--mesh-export")) {
        let (Some(source), Some(destination), None) = (args.next(), args.next(), args.next())
        else {
            eprintln!("Usage: open-pointcloud-studio --mesh-export INPUT OUTPUT.obj");
            std::process::exit(2);
        };
        let source = PathBuf::from(source);
        let destination = PathBuf::from(destination);
        if !destination
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"))
            || camera_views::source_key(&source) == camera_views::source_key(&destination)
        {
            eprintln!("Choose a distinct .obj output path");
            std::process::exit(2);
        }
        let comments: &[&str] = if is_bag3d_obj(&source) {
            BAG3D_MESH_COMMENTS
        } else {
            &[]
        };
        match pointcloud_core::read_mesh_geometry(&source).and_then(|mesh| {
            let mesh = mesh.ok_or_else(|| {
                pointcloud_core::LoadError::InvalidData("source contains no mesh faces".into())
            })?;
            pointcloud_core::write_obj_mesh(&mesh, &destination, comments)?;
            Ok((mesh.vertices.len(), mesh.triangles.len()))
        }) {
            Ok((vertices, triangles)) => {
                println!(
                    "Mesh exported: {vertices} vertices, {triangles} triangles -> {}",
                    destination.display()
                );
                return Ok(());
            }
            Err(error) => {
                eprintln!("Mesh export failed: {error}");
                std::process::exit(1);
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
    iced::application("Open Pointcloud Studio", Studio::update, Studio::view)
        .subscription(|studio| {
            let keyboard = iced::event::listen_with(|event, status, _| match event {
                iced::Event::Window(iced::window::Event::Resized(_)) => Some(Message::RibbonReset),
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) => Some(Message::Escape),
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Delete),
                    ..
                }) if status == iced::event::Status::Ignored => Some(Message::DeleteSelection),
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
                    Some(Message::ResetCamera)
                }
                iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Character(value),
                    modifiers,
                    ..
                }) if status == iced::event::Status::Ignored && modifiers.control() => {
                    if value.eq_ignore_ascii_case("z") {
                        Some(if modifiers.shift() {
                            Message::RedoDelete
                        } else {
                            Message::UndoDelete
                        })
                    } else if value.eq_ignore_ascii_case("y") {
                        Some(Message::RedoDelete)
                    } else {
                        None
                    }
                }
                _ => None,
            });
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
            iced::Subscription::batch([keyboard, api])
        })
        .font(include_bytes!("../../assets/fonts/Inter.ttf").as_slice())
        .font(include_bytes!("../../assets/fonts/SpaceGrotesk.ttf").as_slice())
        .default_font(Font::with_name("Inter"))
        .theme(|studio: &Studio| studio.ui_theme.iced())
        .antialiasing(true)
        .window_size((1440.0, 900.0))
        .run_with(move || {
            let mut studio = Studio::default();
            if let Some((receiver, handle)) = api {
                studio.api_receiver = Some(Arc::new(tokio::sync::Mutex::new(receiver)));
                studio.api_handle = Some(handle);
            }
            let chrome = Task::perform(
                async {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                    3
                },
                Message::SyncWindowChrome,
            );
            let task = Task::batch(
                startup_files
                    .into_iter()
                    .map(|path| studio.load(path))
                    .chain(std::iter::once(chrome)),
            );
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

impl ColorMode {
    const ALL: [Self; 4] = [
        Self::Rgb,
        Self::Elevation,
        Self::Intensity,
        Self::Classification,
    ];
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RibbonTab {
    Home,
    View,
    Select,
    Tools,
}

#[derive(Debug, Clone, Copy)]
enum FileAction {
    Import,
    Activate(usize),
    ExportFull,
    ExportSelection,
    ExportSection,
    ExportMesh,
    MergeVisible,
    CancelMerge,
}

impl RibbonTab {
    fn scroll_id(self) -> scrollable::Id {
        scrollable::Id::new(match self {
            Self::Home => "ops-ribbon-home",
            Self::View => "ops-ribbon-view",
            Self::Select => "ops-ribbon-select",
            Self::Tools => "ops-ribbon-tools",
        })
    }
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
                "TOP",
            ),
            Self::Bottom => (
                -std::f32::consts::FRAC_PI_2,
                -std::f32::consts::FRAC_PI_2,
                "BOTTOM",
            ),
            Self::Front => (-std::f32::consts::FRAC_PI_2, 0.0, "FRONT"),
            Self::Back => (std::f32::consts::FRAC_PI_2, 0.0, "BACK"),
            Self::Right => (0.0, 0.0, "RIGHT"),
            Self::Left => (std::f32::consts::PI, 0.0, "LEFT"),
            Self::Isometric => (-0.8, 0.6, "ISOMETRIC"),
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    SyncWindowChrome(u8),
    ApiRequest(native_api::ApiRequest),
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
    Tab(RibbonTab),
    ToggleFile,
    FileAction(FileAction),
    RibbonScroll(f32),
    RibbonViewport(RibbonTab, f32, f32, f32),
    RibbonReset,
    Theme(UiTheme),
    PersistSettings(u64),
    Open,
    FilesChosen(Option<Vec<PathBuf>>),
    Loaded(Result<Arc<PointCloud>, String>),
    MeshLoaded(Arc<PointCloud>, Result<Option<Arc<MeshGeometry>>, String>),
    Refined(Arc<PointCloud>, Result<Arc<PointCloud>, String>),
    Export,
    ExportSection,
    SectionExportPathChosen(
        Arc<PointCloud>,
        Bounds,
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
                Arc<MeshGeometry>,
            ),
            String,
        >,
    ),
    MeshPoll,
    CancelMesh,
    ExportMesh,
    MeshExportPathChosen(
        Arc<MeshGeometry>,
        PathBuf,
        bool,
        CloudTransform,
        Option<PathBuf>,
    ),
    MeshExported(Result<(PathBuf, usize, usize), String>),
    ToggleBagPanel,
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
    BagReady(Result<(PathBuf, pointcloud_core::BagStats), String>),
    OpenBagLicense,
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
    Budget(u32),
    FilterGround(bool),
    FilterVegetation(bool),
    FilterBuildings(bool),
    FilterOther(bool),
    FilterClass(u8, bool),
    SetSectionEnabled(bool),
    SectionMin(usize, f32),
    SectionMax(usize, f32),
    SectionHandleDelta(usize, bool, f32),
    SectionCoordinate(usize, bool, String),
    ApplySectionCoordinates,
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
    Pan(f32, f32),
    FinishPan(f32, f32),
    FinishOrbit(f32, f32),
    NavigationFinished,
    Zoom(f32, [f32; 2], Size),
    ViewportSize(Size),
    ResetCamera,
    CameraPreset(CameraPreset),
    CubeCorner([i8; 3]),
    ViewName(String),
    SaveView,
    RestoreView(usize),
    DeleteView(usize),
    ShowContextMenu([f32; 2]),
    ContextAction(ContextAction),
    DismissContextMenu,
    Escape,
    CancelSelection,
    ToggleBoxSelect,
    TogglePickSelect,
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
    api_jobs: HashMap<String, Value>,
    api_job_order: VecDeque<String>,
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
    bag_pending: bool,
    bag_last_stats: Option<pointcloud_core::BagStats>,
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
    budget: u32,
    filter_ground: bool,
    filter_vegetation: bool,
    filter_buildings: bool,
    filter_other: bool,
    class_visibility: ClassVisibility,
    section_enabled: bool,
    section_export_pending: bool,
    mesh_export_pending: bool,
    mesh_dialog_pending: bool,
    mesh_job: Option<MeshJob>,
    merge_job: Option<MergeJob>,
    merge_dialog_pending: bool,
    selection_bounds_pending: bool,
    section_reference_bounds: Option<Bounds>,
    section_min_percent: [f64; 3],
    section_max_percent: [f64; 3],
    section_coordinate_inputs: [[String; 2]; 3],
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    view_label: &'static str,
    saved_views: Vec<SavedView>,
    view_name: String,
    viewport_size: Size,
    ribbon_tab: RibbonTab,
    ribbon_viewport: Option<(f32, f32, f32)>,
    file_open: bool,
    ui_theme: UiTheme,
    settings_revision: u64,
    box_select: bool,
    pick_mode: bool,
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
    auto_index: bool,
    revision: u64,
}

struct CloudEntry {
    cloud: Arc<PointCloud>,
    /// Stable identity for asynchronous work started before a LAS preview
    /// replaces the initial header-only cloud.
    load_identity: Arc<PointCloud>,
    transform: CloudTransform,
    centroid_cache: Option<CentroidCache>,
    mesh: Option<Arc<MeshGeometry>>,
    mesh_visible: bool,
    bag_source: bool,
    visible: bool,
    selection: Option<Arc<SelectionMask>>,
    deleted: Option<Arc<DeletionMask>>,
    index: Option<Arc<OctreeIndex>>,
    auto_index_queued: bool,
    index_building: bool,
    detail_points: Option<Arc<[IndexedPoint]>>,
}

struct LodRefinement {
    sources: Vec<(usize, Arc<OctreeIndex>, CloudTransform, f32)>,
    source_weights: Vec<(f32, usize)>,
    requested: Vec<usize>,
    sampled_limits: Vec<usize>,
    samples: Vec<Vec<IndexedPoint>>,
    section: Option<Bounds>,
    projection: Projection,
    cancel: Arc<AtomicBool>,
    budget: usize,
    deep_zoom: bool,
}

impl LodRefinement {
    fn sample_pass(&mut self) -> Result<(), String> {
        let workers: Vec<_> = self
            .sources
            .iter()
            .enumerate()
            .filter(|(slot, _)| self.requested[*slot] > self.sampled_limits[*slot])
            .map(|(slot, (_, tree, transform, _))| {
                let tree = Arc::clone(tree);
                let transform = *transform;
                let cancel = Arc::clone(&self.cancel);
                let limit = self.requested[slot];
                let section = self.section;
                let projection = self.projection;
                let deep_zoom = self.deep_zoom;
                std::thread::spawn(move || {
                    let projected = |node_bounds: Bounds| {
                        let node_bounds = transform.bounds(node_bounds);
                        if section.is_some_and(|clip| {
                            (0..3).any(|axis| {
                                node_bounds.max[axis] < clip.min[axis]
                                    || node_bounds.min[axis] > clip.max[axis]
                            })
                        }) {
                            return None;
                        }
                        projection.screen_span(node_bounds)
                    };
                    let exact = if deep_zoom {
                        tree.sample_visible_indexed_cancellable(
                            limit,
                            MAX_EXACT_VISIBLE_LOD_CANDIDATES,
                            |bounds| projected(bounds).is_some(),
                            |record| {
                                let xyz = transform.xyz(record.point.xyz);
                                section.is_none_or(|clip| {
                                    (0..3).all(|axis| {
                                        xyz[axis] >= clip.min[axis] && xyz[axis] <= clip.max[axis]
                                    })
                                }) && projection.project(xyz).is_some()
                            },
                            || cancel.load(Ordering::Relaxed),
                        )
                        .map_err(|error| error.to_string())?
                    } else {
                        None
                    };
                    exact
                        .map(Ok)
                        .unwrap_or_else(|| {
                            tree.sample_lod_indexed_cancellable(limit, projected, || {
                                cancel.load(Ordering::Relaxed)
                            })
                        })
                        .map(|points| (slot, points))
                        .map_err(|error| error.to_string())
                })
            })
            .collect();
        for worker in workers {
            let (slot, points) = worker
                .join()
                .map_err(|_| "detail worker panicked".to_string())??;
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

    fn snapshot(&self) -> Vec<(usize, Vec<IndexedPoint>)> {
        self.sources
            .iter()
            .zip(&self.samples)
            .map(|((index, _, _, _), points)| (*index, points.clone()))
            .collect()
    }

    fn finish(self) -> Vec<(usize, Vec<IndexedPoint>)> {
        self.sources
            .into_iter()
            .zip(self.samples)
            .map(|((index, _, _, _), points)| (index, points))
            .collect()
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
            api_jobs: HashMap::new(),
            api_job_order: VecDeque::new(),
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
            bag_pending: false,
            bag_last_stats: None,
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
            budget: settings.budget,
            filter_ground: settings.filter_ground,
            filter_vegetation: settings.filter_vegetation,
            filter_buildings: settings.filter_buildings,
            filter_other: settings.filter_other,
            class_visibility: ClassVisibility::default(),
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
            yaw: -0.8,
            pitch: 0.6,
            zoom: 1.0,
            pan: [0.0, 0.0],
            view_label: "ISOMETRIC",
            saved_views: camera_views::load(),
            view_name: String::new(),
            viewport_size: Size::new(915.0, 743.0),
            ribbon_tab: RibbonTab::Home,
            ribbon_viewport: None,
            file_open: false,
            ui_theme: UiTheme::load(),
            settings_revision: 0,
            box_select: false,
            pick_mode: false,
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
            .map(|entry| camera_views::source_key(&entry.cloud.path))
    }

    fn mesh_filter(&self) -> ClassFilter {
        ClassFilter {
            ground: self.filter_ground,
            vegetation: self.filter_vegetation,
            buildings: self.filter_buildings,
            other: self.filter_other,
            classes: self.class_visibility,
            section: self.section_bounds(),
        }
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
                            "view_sample": entry.view_len(),
                            "bounds": {"min": entry.bounds().min, "max": entry.bounds().max},
                            "transform": {"scale": entry.transform.scale, "offset": entry.transform.offset},
                        })
                    })
                    .collect();
                let section = self
                    .section_bounds()
                    .map(|bounds| json!({"min": bounds.min, "max": bounds.max}));
                let active_source = self.active_camera_source();
                let camera_views: Vec<_> = self
                    .saved_views
                    .iter()
                    .filter(|view| active_source.as_ref() == Some(&view.source))
                    .collect();
                (
                    json!({"ok": true, "result": {
                        "clouds": clouds,
                        "active": self.active,
                        "status": self.status,
                        "camera": {"yaw": self.yaw, "pitch": self.pitch, "zoom": self.zoom, "pan": self.pan, "view": self.view_label},
                        "camera_views": camera_views,
                        "section": section,
                        "selected_points": self.selected_total(),
                        "selection_pending": self.selection_pending,
                        "selection_bounds_pending": self.selection_bounds_pending,
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
                )
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
                if !path.is_absolute() || !path.is_file() {
                    (
                        json!({"ok": false, "error": "open requires an absolute path to an existing file"}),
                        Task::none(),
                    )
                } else {
                    let task = self.load(path.clone());
                    (json!({"ok": true, "accepted": true, "path": path}), task)
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
            } => {
                if !(-std::f32::consts::PI..=std::f32::consts::PI).contains(&yaw)
                    || !(-1.56..=1.56).contains(&pitch)
                    || !(0.000_001..=10_000.0).contains(&zoom)
                    || !pan.iter().all(|value| value.is_finite())
                {
                    (
                        json!({"ok": false, "error": "camera requires finite yaw within ±π, pitch within ±1.56, zoom from 0.000001 to 10000, and finite pan"}),
                        Task::none(),
                    )
                } else {
                    self.yaw = yaw;
                    self.pitch = pitch;
                    self.zoom = zoom;
                    self.pan = pan;
                    self.view_label = "CUSTOM";
                    self.revision += 1;
                    let task = self.schedule_detail();
                    (
                        json!({"ok": true, "camera": {"yaw": self.yaw, "pitch": self.pitch, "zoom": self.zoom, "pan": self.pan, "view": self.view_label}}),
                        task,
                    )
                }
            }
            ApiCommand::ZoomAll => {
                let task = self.update(Message::ResetCamera);
                (
                    json!({"ok": true, "camera": {"yaw": self.yaw, "pitch": self.pitch, "zoom": self.zoom, "pan": self.pan, "view": self.view_label}}),
                    task,
                )
            }
            ApiCommand::ListCameraViews => {
                let source = self.active_camera_source();
                let views: Vec<_> = self
                    .saved_views
                    .iter()
                    .filter(|view| source.as_ref() == Some(&view.source))
                    .collect();
                (
                    json!({"ok": true, "source": source, "views": views}),
                    Task::none(),
                )
            }
            ApiCommand::SaveCameraView { name } => {
                let name = name.trim().to_owned();
                if let Some(source) = self.active_camera_source() {
                    let matching: Vec<_> = self
                        .saved_views
                        .iter()
                        .filter(|view| view.source == source)
                        .collect();
                    if name.is_empty()
                        || name.chars().count() > 64
                        || matching.len() >= 32
                        || matching
                            .iter()
                            .any(|view| view.name.eq_ignore_ascii_case(&name))
                    {
                        (
                            json!({"ok": false, "error": "choose a unique camera view name of 1 to 64 characters; each scan allows at most 32 views"}),
                            Task::none(),
                        )
                    } else {
                        self.saved_views.push(SavedView {
                            source,
                            name: name.clone(),
                            yaw: self.yaw,
                            pitch: self.pitch,
                            zoom: self.zoom,
                            pan: self.pan,
                        });
                        match camera_views::save(&self.saved_views) {
                            Ok(()) => {
                                self.status = format!("Saved camera view {name}");
                                (json!({"ok": true, "name": name}), Task::none())
                            }
                            Err(error) => {
                                self.saved_views.pop();
                                (
                                    json!({"ok": false, "error": error.to_string()}),
                                    Task::none(),
                                )
                            }
                        }
                    }
                } else {
                    (
                        json!({"ok": false, "error": "open a scan before saving a camera view"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::RestoreCameraView { name } => {
                let source = self.active_camera_source();
                if let Some(index) = self.saved_views.iter().position(|view| {
                    source.as_ref() == Some(&view.source)
                        && view.name.eq_ignore_ascii_case(name.trim())
                }) {
                    let view = self.saved_views[index].clone();
                    let task = self.update(Message::RestoreView(index));
                    (json!({"ok": true, "view": view}), task)
                } else {
                    (
                        json!({"ok": false, "error": "camera view not found for the active scan"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::DeleteCameraView { name } => {
                let source = self.active_camera_source();
                if let Some(index) = self.saved_views.iter().position(|view| {
                    source.as_ref() == Some(&view.source)
                        && view.name.eq_ignore_ascii_case(name.trim())
                }) {
                    let view = self.saved_views.remove(index);
                    match camera_views::save(&self.saved_views) {
                        Ok(()) => {
                            self.status = format!("Deleted camera view {}", view.name);
                            (json!({"ok": true, "name": view.name}), Task::none())
                        }
                        Err(error) => {
                            self.saved_views.insert(index, view);
                            (
                                json!({"ok": false, "error": error.to_string()}),
                                Task::none(),
                            )
                        }
                    }
                } else {
                    (
                        json!({"ok": false, "error": "camera view not found for the active scan"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::SetTheme { theme } => {
                if let Some(theme) = UiTheme::from_key(&theme.to_ascii_lowercase()) {
                    let task = self.update(Message::Theme(theme));
                    (json!({"ok": true, "theme": theme.key()}), task)
                } else {
                    (json!({"ok": false, "error": "unknown theme"}), Task::none())
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
            ApiCommand::SetSection { min, max } => {
                if let Some(overall) = combined_bounds(&self.clouds) {
                    if let Some(section) = section_within_model(Bounds { min, max }, overall) {
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
                            json!({"ok": true, "section": {"min": section.min, "max": section.max}}),
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
                            section: self.section_bounds(),
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
                } else if !self
                    .active
                    .and_then(|index| self.clouds.get(index))
                    .is_some_and(|entry| entry.index.is_none())
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
            ApiCommand::Mesh { mode, path } => {
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
                if self.mesh_dialog_pending || self.mesh_job.is_some() {
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
                        json!({"ok": false, "error": "mesh mode must be terrain or surface"}),
                        Task::none(),
                    )
                }
            }
            ApiCommand::CancelMesh => {
                if self.mesh_job.is_none() {
                    (
                        json!({"ok": false, "error": "no mesh task is running"}),
                        Task::none(),
                    )
                } else {
                    let task = self.update(Message::CancelMesh);
                    (json!({"ok": true, "cancel_requested": true}), task)
                }
            }
            ApiCommand::Export { path } => self.api_export(path, ApiExportMode::Full),
            ApiCommand::ExportSection { path } => self.api_export(path, ApiExportMode::Section),
            ApiCommand::ExportSelection { path } => self.api_export(path, ApiExportMode::Selected),
            ApiCommand::ExportMinusSelection { path } => {
                self.api_export(path, ApiExportMode::WithoutSelection)
            }
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
            let Some(section) = self.section_bounds() else {
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

    fn start_index_job(&mut self, source: Arc<PointCloud>, automatic: bool) -> Task<Message> {
        let progress = Arc::new(Mutex::new(IndexProgress {
            stage: IndexStage::ReadingSource,
            completed: 0,
            total: source.total_points,
            depth: 0,
            leaves: 0,
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
            self.section_reference_bounds = combined_bounds(&self.clouds);
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
                                let edited = MeshGeometry {
                                    vertices: mesh
                                        .vertices
                                        .iter()
                                        .map(|xyz| transform.xyz(*xyz))
                                        .collect(),
                                    triangles: mesh.triangles.clone(),
                                    colors: mesh.colors.clone(),
                                    normals: mesh.normals.as_deref().and_then(|normals| {
                                        transformed_mesh_normals(normals, transform.scale)
                                    }),
                                };
                                pointcloud_core::write_obj_mesh(&edited, &path, &[])?;
                            }
                            Ok((source, path, stats, Arc::new(mesh)))
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
                        transform: CloudTransform::default(),
                        centroid_cache: None,
                        mesh: None,
                        mesh_visible: true,
                        bag_source: false,
                        visible: true,
                        selection: None,
                        deleted: None,
                        index: None,
                        auto_index_queued: false,
                        index_building: false,
                        detail_points: None,
                    });
                    self.revision += 1;
                    self.active = Some(self.clouds.len() - 1);
                    if self.section_enabled && self.section_reference_bounds.is_none() {
                        self.section_reference_bounds = combined_bounds(&self.clouds);
                        self.sync_section_coordinate_inputs();
                    }
                    let identity = Arc::clone(&header_cloud);
                    let preview_task = Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                pointcloud_core::open_las_preview(path, LOAD_SAMPLE_LIMIT)
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
        self.status = format!("Loading {}…", path.display());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || pointcloud_core::open(path, LOAD_SAMPLE_LIMIT))
                    .await
                    .map_err(|error| error.to_string())?
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
            },
            Message::Loaded,
        )
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SyncWindowChrome(retries) => {
                if !native_chrome::apply(self.ui_theme != UiTheme::Light) && retries > 0 {
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
            Message::Tab(tab) => {
                self.ribbon_tab = tab;
                self.ribbon_viewport = None;
                self.file_open = false;
            }
            Message::ToggleFile => {
                self.file_open = !self.file_open;
                self.ribbon_viewport = None;
            }
            Message::FileAction(action) => {
                self.file_open = false;
                return self.update(match action {
                    FileAction::Import => Message::Open,
                    FileAction::Activate(index) => Message::Select(index),
                    FileAction::ExportFull => Message::Export,
                    FileAction::ExportSelection => Message::ExportSelection,
                    FileAction::ExportSection => Message::ExportSection,
                    FileAction::ExportMesh => Message::ExportMesh,
                    FileAction::MergeVisible => Message::MergeVisible,
                    FileAction::CancelMerge => Message::CancelMerge,
                });
            }
            Message::RibbonScroll(direction) => {
                return scrollable::scroll_by(
                    self.ribbon_tab.scroll_id(),
                    scrollable::AbsoluteOffset {
                        x: direction * 320.0,
                        y: 0.0,
                    },
                );
            }
            Message::RibbonViewport(tab, offset, width, content_width) => {
                if tab == self.ribbon_tab {
                    self.ribbon_viewport = Some((offset, width, content_width));
                }
            }
            Message::RibbonReset => self.ribbon_viewport = None,
            Message::Theme(theme) => {
                self.ui_theme = theme;
                theme.save();
                let _ = native_chrome::apply(theme != UiTheme::Light);
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
                        rfd::AsyncFileDialog::new()
                            .add_filter(
                                "Point clouds",
                                &[
                                    "las", "laz", "ply", "xyz", "csv", "asc", "txt", "pts", "ptx",
                                    "pcd", "obj", "off", "stl", "dxf", "e57",
                                ],
                            )
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
            Message::FilesChosen(Some(paths)) => {
                return Task::batch(paths.into_iter().map(|path| self.load(path)));
            }
            Message::FilesChosen(None) => {}
            Message::Loaded(result) => match result {
                Ok(cloud) => {
                    self.cancel_selection_for_scene_change();
                    let cache_source = Arc::clone(&cloud);
                    let mesh_path = cloud.path.clone();
                    let mesh_format = mesh_path
                        .extension()
                        .and_then(|value| value.to_str())
                        .map(str::to_ascii_lowercase);
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
                        bag_source: is_bag3d_obj(&cloud.path),
                        load_identity: Arc::clone(&cloud),
                        cloud,
                        transform: CloudTransform::default(),
                        centroid_cache: None,
                        mesh: None,
                        mesh_visible: true,
                        visible: true,
                        selection: None,
                        deleted: None,
                        index: None,
                        auto_index_queued: false,
                        index_building: false,
                        detail_points: None,
                    });
                    self.revision += 1;
                    self.active = Some(self.clouds.len() - 1);
                    if self.section_enabled && self.section_reference_bounds.is_none() {
                        self.section_reference_bounds = combined_bounds(&self.clouds);
                        self.sync_section_coordinate_inputs();
                    }
                    self.yaw = -0.8;
                    self.pitch = 0.6;
                    self.zoom = 1.0;
                    self.pan = [0.0, 0.0];
                    self.view_label = "ISOMETRIC";
                    let cache_task = cached_index_task(cache_source);
                    if matches!(
                        mesh_format.as_deref(),
                        Some("obj" | "ply" | "off" | "stl" | "dxf")
                    ) {
                        let mesh_source = Arc::clone(&self.clouds.last().unwrap().cloud);
                        let mesh_task = Task::perform(
                            async move {
                                tokio::task::spawn_blocking(move || {
                                    pointcloud_core::read_mesh_geometry(mesh_path)
                                        .map(|mesh| mesh.map(Arc::new))
                                        .map_err(|error| error.to_string())
                                })
                                .await
                                .map_err(|error| error.to_string())
                                .and_then(|result| result)
                            },
                            move |result| Message::MeshLoaded(Arc::clone(&mesh_source), result),
                        );
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
                        Ok(Some(mesh)) => {
                            self.status = format!(
                                "Mesh displayed: {} vertices, {} triangles",
                                mesh.vertices.len(),
                                mesh.triangles.len()
                            );
                            entry.mesh = Some(mesh);
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
                            entry.cloud = Arc::clone(&cloud);
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
                    self.section_bounds(),
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
                if self.mesh_dialog_pending || self.mesh_job.is_some() {
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
                                .add_filter("Wavefront OBJ", &["obj"])
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
                if self.mesh_job.is_some() {
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
                            Ok((_, path, stats, _)) => json!({
                                "state": "complete",
                                "path": path,
                                "mode": mode.label(),
                                "source_points": stats.source_points,
                                "vertices": stats.vertices,
                                "triangles": stats.triangles,
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
                    Ok((source, path, stats, mesh)) => {
                        if let Some(entry) = self
                            .clouds
                            .iter_mut()
                            .find(|entry| entry.matches_source(&source))
                        {
                            entry.mesh = Some(mesh);
                            self.status = format!(
                                "{} mesh displayed: {} vertices, {} triangles from {} points → {}",
                                mode.label(),
                                stats.vertices,
                                stats.triangles,
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
            Message::ExportMesh => {
                if self.mesh_export_pending {
                    return Task::none();
                }
                let Some(entry) = self
                    .active
                    .and_then(|index| self.clouds.get(index))
                    .filter(|entry| entry.mesh.is_some())
                else {
                    self.status = "Select a cloud with a surface mesh first".into();
                    return Task::none();
                };
                let mesh = Arc::clone(entry.mesh.as_ref().unwrap());
                let source = entry.cloud.path.clone();
                let bag_source = entry.bag_source;
                let transform = entry.transform;
                let stem = source
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("surface");
                let suggestion = format!("{stem}-mesh.obj");
                self.mesh_export_pending = true;
                self.status = "Choose where to save the visible surface as OBJ…".into();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .add_filter("Wavefront OBJ", &["obj"])
                            .set_file_name(suggestion)
                            .save_file()
                            .await
                            .map(|selection| selection.path().to_path_buf())
                    },
                    move |path| {
                        Message::MeshExportPathChosen(
                            Arc::clone(&mesh),
                            source.clone(),
                            bag_source,
                            transform,
                            path,
                        )
                    },
                );
            }
            Message::MeshExportPathChosen(mesh, source, bag_source, transform, Some(path)) => {
                if !path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"))
                {
                    self.mesh_export_pending = false;
                    self.status = "Choose an .obj output file for the surface mesh".into();
                    return Task::none();
                }
                if camera_views::source_key(&source) == camera_views::source_key(&path) {
                    self.mesh_export_pending = false;
                    self.status = "Choose an OBJ path different from the source file".into();
                    return Task::none();
                }
                self.status = format!(
                    "Writing {} vertices and {} triangles…",
                    mesh.vertices.len(),
                    mesh.triangles.len()
                );
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let comments: &[&str] =
                                if bag_source { BAG3D_MESH_COMMENTS } else { &[] };
                            let edited = MeshGeometry {
                                vertices: mesh
                                    .vertices
                                    .iter()
                                    .map(|xyz| transform.xyz(*xyz))
                                    .collect(),
                                triangles: mesh.triangles.clone(),
                                colors: mesh.colors.clone(),
                                normals: mesh.normals.as_deref().and_then(|normals| {
                                    transformed_mesh_normals(normals, transform.scale)
                                }),
                            };
                            pointcloud_core::write_obj_mesh(&edited, &path, comments)
                                .map(|()| (path, mesh.vertices.len(), mesh.triangles.len()))
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    Message::MeshExported,
                );
            }
            Message::MeshExportPathChosen(_, _, _, _, None) => {
                self.mesh_export_pending = false;
                self.status = "Mesh export cancelled".into();
            }
            Message::MeshExported(result) => {
                self.mesh_export_pending = false;
                self.status = match result {
                    Ok((path, vertices, triangles)) => format!(
                        "Exported mesh: {vertices} vertices and {triangles} triangles to {}",
                        path.display()
                    ),
                    Err(error) => format!("Mesh export failed: {error}"),
                };
            }
            Message::ToggleBagPanel => {
                self.bag_panel = !self.bag_panel;
                if self.bag_panel {
                    let prefill = if self.bag_fields.iter().all(String::is_empty) {
                        self.update(Message::BagFromSection)
                    } else {
                        Task::none()
                    };
                    return Task::batch([prefill, self.schedule_bag_map()]);
                }
            }
            Message::BagField(index, value) => {
                if let Some(field) = self.bag_fields.get_mut(index) {
                    *field = value;
                }
            }
            Message::BagLod(lod) => self.bag_lod = lod,
            Message::BagFromSection => {
                let bounds = self.section_bounds().or_else(|| {
                    self.active
                        .and_then(|index| self.clouds.get(index))
                        .map(CloudEntry::bounds)
                });
                if let Some(bounds) = bounds {
                    self.bag_fields = [
                        format!("{:.2}", bounds.min[0]),
                        format!("{:.2}", bounds.min[1]),
                        format!("{:.2}", bounds.max[0]),
                        format!("{:.2}", bounds.max[1]),
                    ];
                    self.status = "3DBAG area copied from scan / section box".into();
                    let mut view = self.bag_map_view();
                    view.fit(BagBounds {
                        min_x: bounds.min[0],
                        min_y: bounds.min[1],
                        max_x: bounds.max[0],
                        max_y: bounds.max[1],
                    });
                    self.bag_map_center = view.center;
                    self.bag_map_zoom = view.zoom;
                    return self.schedule_bag_map();
                } else {
                    self.status = "Draw an area on the map or enter RD coordinates".into();
                }
            }
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
                    Err(error) => format!("Selected area: {error}"),
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
            Message::BagMapFitFields => match BagBounds::parse(&self.bag_fields.join(",")) {
                Ok(bounds) => {
                    let mut view = self.bag_map_view();
                    view.fit(bounds);
                    self.bag_map_center = view.center;
                    self.bag_map_zoom = view.zoom;
                    return self.schedule_bag_map();
                }
                Err(error) => self.status = format!("Map area: {error}"),
            },
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
            Message::BagDownload => {
                if self.bag_pending {
                    return Task::none();
                }
                let bbox = self.bag_fields.join(",");
                let bounds = match BagBounds::parse(&bbox) {
                    Ok(bounds) => bounds,
                    Err(error) => {
                        self.status = format!("3DBAG area: {error}");
                        return Task::none();
                    }
                };
                let lod = self.bag_lod;
                self.status = "Choose where to save the 3DBAG OBJ…".into();
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .add_filter("Wavefront OBJ", &["obj"])
                            .set_file_name("3dbag-buildings.obj")
                            .save_file()
                            .await
                            .map(|selection| selection.path().to_path_buf())
                    },
                    move |path| Message::BagPathChosen(bounds, lod, path),
                );
            }
            Message::BagPathChosen(bounds, lod, Some(path)) => {
                self.bag_pending = true;
                self.status = format!("Downloading 3DBAG LoD {lod} buildings…");
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            pointcloud_core::fetch_bag3d_obj(bounds, lod, &path)
                                .map(|stats| (path, stats))
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result)
                    },
                    Message::BagReady,
                );
            }
            Message::BagPathChosen(_, _, None) => {
                self.status = "3DBAG save cancelled".into();
            }
            Message::BagReady(result) => {
                self.bag_pending = false;
                match result {
                    Ok((path, stats)) => {
                        self.bag_last_stats = Some(stats);
                        let task = self.load(path);
                        self.status = format!(
                            "3DBAG downloaded: {} buildings, {} triangles from {} page(s)",
                            stats.buildings, stats.triangles, stats.pages
                        );
                        return task;
                    }
                    Err(error) => self.status = format!("3DBAG failed: {error}"),
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
                                self.section_reference_bounds = combined_bounds(&self.clouds);
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
                        self.section_reference_bounds = combined_bounds(&self.clouds);
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
                let section = self.section_bounds();
                let projection = Projection::new(
                    bounds,
                    self.yaw,
                    self.pitch,
                    self.zoom,
                    self.pan,
                    self.viewport_size.width,
                    self.viewport_size.height,
                );
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
                let initial_budget = if budget > FAST_LOD_PREVIEW_LIMIT * 2 {
                    FAST_LOD_PREVIEW_LIMIT
                } else {
                    budget
                };
                let limits = distribute_lod_budget(initial_budget, &source_weights);
                let revision = self.revision;
                let cancel = Arc::new(AtomicBool::new(false));
                self.detail_cancel = Arc::clone(&cancel);
                self.detail_pending = true;
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
                    deep_zoom: self.zoom <= EXACT_VISIBLE_LOD_ZOOM,
                };
                let stream =
                    iced::futures::stream::unfold(Some((refinement, 0u8)), |state| async move {
                        let (refinement, pass) = state?;
                        let outcome = tokio::task::spawn_blocking(move || {
                            let mut refinement = refinement;
                            let result = refinement.sample_pass();
                            (refinement, result)
                        })
                        .await;
                        match outcome {
                            Ok((mut refinement, Ok(()))) => {
                                if pass < 2 {
                                    if let Some(next) = refinement.next_limits() {
                                        let preview = refinement.snapshot();
                                        refinement.requested = next;
                                        return Some((
                                            Ok((false, preview)),
                                            Some((refinement, pass + 1)),
                                        ));
                                    }
                                }
                                Some((Ok((true, refinement.finish())), None))
                            }
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
            Message::Remove(index) => {
                if index < self.clouds.len() {
                    self.cancel_selection_for_scene_change();
                    self.clouds.remove(index);
                    self.undo_deletions.clear();
                    self.redo_deletions.clear();
                    self.pending_delete = false;
                    self.revision += 1;
                    self.active = if self.clouds.is_empty() {
                        None
                    } else {
                        Some(index.min(self.clouds.len() - 1))
                    };
                    return self.schedule_detail();
                }
            }
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
            Message::Budget(budget) => {
                self.budget = budget;
                self.revision += 1;
                return Task::batch([self.schedule_detail(), self.queue_preferences_save()]);
            }
            Message::FilterGround(value) => {
                self.filter_ground = value;
                self.revision += 1;
                return self.queue_preferences_save();
            }
            Message::FilterVegetation(value) => {
                self.filter_vegetation = value;
                self.revision += 1;
                return self.queue_preferences_save();
            }
            Message::FilterBuildings(value) => {
                self.filter_buildings = value;
                self.revision += 1;
                return self.queue_preferences_save();
            }
            Message::FilterOther(value) => {
                self.filter_other = value;
                self.revision += 1;
                return self.queue_preferences_save();
            }
            Message::FilterClass(code, visible) => {
                self.class_visibility.set(code, visible);
                self.revision += 1;
            }
            Message::SetSectionEnabled(enabled) => {
                if enabled && self.section_reference_bounds.is_none() {
                    self.section_reference_bounds = combined_bounds(&self.clouds);
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
                self.section_reference_bounds = Some(overall);
                self.section_enabled = true;
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                self.status = "Section box updated from XYZ coordinates".into();
                return self.schedule_detail();
            }
            Message::ResetSectionBox => {
                self.section_reference_bounds = combined_bounds(&self.clouds);
                self.section_min_percent = [0.0; 3];
                self.section_max_percent = [100.0; 3];
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                return self.schedule_detail();
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
                self.selection_bounds_pending = true;
                self.status = "Finding exact bounds of selected source points…".into();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || selected_source_bounds(&sources))
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
                let Some(reference) = loaded_bounds(&self.clouds) else {
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
                self.yaw = (self.yaw + dx * 0.01 + std::f32::consts::PI)
                    .rem_euclid(std::f32::consts::TAU)
                    - std::f32::consts::PI;
                self.pitch = (self.pitch + dy * 0.01).clamp(-1.56, 1.56);
                self.view_label = "CUSTOM";
                self.revision += 1;
                return self.schedule_detail();
            }
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
                let (yaw, pitch, label) = preset.orientation();
                self.yaw = yaw;
                self.pitch = pitch;
                self.view_label = label;
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::CubeCorner(corner) => {
                self.yaw = f32::from(corner[1]).atan2(f32::from(corner[0]));
                self.pitch = f32::from(corner[2]).atan2(std::f32::consts::SQRT_2);
                self.view_label = "ISO CORNER";
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::ViewName(name) => self.view_name = name,
            Message::SaveView => {
                let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) else {
                    self.status = "Open a scan before saving a camera view".into();
                    return Task::none();
                };
                let source = camera_views::source_key(&entry.cloud.path);
                let existing: Vec<_> = self
                    .saved_views
                    .iter()
                    .filter(|view| view.source == source)
                    .collect();
                if existing.len() >= 32 {
                    self.status = "A scan can have at most 32 saved camera views".into();
                    return Task::none();
                }
                let name = if self.view_name.trim().is_empty() {
                    (1..=32)
                        .map(|number| format!("View {number}"))
                        .find(|name| !existing.iter().any(|view| view.name == *name))
                        .unwrap()
                } else {
                    self.view_name.trim().to_owned()
                };
                if name.chars().count() > 64
                    || existing
                        .iter()
                        .any(|view| view.name.eq_ignore_ascii_case(&name))
                {
                    self.status =
                        "Choose a unique camera view name of 64 characters or fewer".into();
                    return Task::none();
                }
                self.saved_views.push(SavedView {
                    source,
                    name: name.clone(),
                    yaw: self.yaw,
                    pitch: self.pitch,
                    zoom: self.zoom,
                    pan: self.pan,
                });
                self.view_name.clear();
                self.status = match camera_views::save(&self.saved_views) {
                    Ok(()) => format!("Saved camera view {name}"),
                    Err(error) => format!("Camera view is in memory; saving failed: {error}"),
                };
            }
            Message::RestoreView(index) => {
                let Some(view) = self.saved_views.get(index).cloned() else {
                    return Task::none();
                };
                let active_source = self
                    .active
                    .and_then(|index| self.clouds.get(index))
                    .map(|entry| camera_views::source_key(&entry.cloud.path));
                if active_source.as_ref() != Some(&view.source) {
                    return Task::none();
                }
                self.yaw = view.yaw;
                self.pitch = view.pitch;
                self.zoom = view.zoom;
                self.pan = view.pan;
                self.view_label = "SAVED VIEW";
                self.status = format!("Restored camera view {}", view.name);
                self.revision += 1;
                return self.schedule_detail();
            }
            Message::DeleteView(index) => {
                let Some(view) = self.saved_views.get(index) else {
                    return Task::none();
                };
                let active_source = self
                    .active
                    .and_then(|index| self.clouds.get(index))
                    .map(|entry| camera_views::source_key(&entry.cloud.path));
                if active_source.as_ref() != Some(&view.source) {
                    return Task::none();
                }
                let name = self.saved_views.remove(index).name;
                self.status = match camera_views::save(&self.saved_views) {
                    Ok(()) => format!("Deleted camera view {name}"),
                    Err(error) => format!("Camera view removed in memory; saving failed: {error}"),
                };
            }
            Message::ShowContextMenu(point) => self.context_menu = Some(point),
            Message::DismissContextMenu => self.context_menu = None,
            Message::ContextAction(action) => {
                self.context_menu = None;
                match action {
                    ContextAction::Orbit => {
                        self.box_select = false;
                        self.pick_mode = false;
                        self.drag_rectangle = None;
                        self.status = "Orbit mode".into();
                    }
                    ContextAction::BoxSelect => {
                        self.box_select = true;
                        self.pick_mode = false;
                        self.ribbon_tab = RibbonTab::Select;
                        self.status = "Box selection active; Escape exits".into();
                    }
                    ContextAction::PickPoint => {
                        self.pick_mode = true;
                        self.box_select = false;
                        self.ribbon_tab = RibbonTab::Select;
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
                if self.file_open {
                    self.file_open = false;
                    return Task::none();
                }
                let cancelling = self.cancel_selection();
                self.context_menu = None;
                self.box_select = false;
                self.pick_mode = false;
                self.bag_map_drawing = false;
                self.drag_rectangle = None;
                self.status = if cancelling {
                    "Cancelling selection; orbit and right-click menu available".into()
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
                self.ribbon_tab = RibbonTab::Select;
                self.drag_rectangle = None;
            }
            Message::TogglePickSelect => {
                self.pick_mode = !self.pick_mode;
                self.box_select = false;
                self.ribbon_tab = RibbonTab::Select;
                self.drag_rectangle = None;
            }
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
                    section: self.section_bounds(),
                };
                let revision = self.revision;
                if self.pick_mode {
                    let Some((index, entry)) = self
                        .active
                        .and_then(|index| self.clouds.get(index).map(|entry| (index, entry)))
                        .filter(|(_, entry)| entry.visible)
                    else {
                        self.status = "Choose a visible point cloud to pick from".into();
                        return Task::none();
                    };
                    let tree = entry.index.as_ref().map(Arc::clone);
                    let cloud = Arc::clone(&entry.cloud);
                    let deleted = entry.deleted.as_ref().map(Arc::clone);
                    let transform = entry.transform;
                    self.selection_pending = true;
                    self.selection_cancel = Arc::new(AtomicBool::new(false));
                    let cancel = Arc::clone(&self.selection_cancel);
                    self.status = if tree.is_some() {
                        "Finding nearest point through the octree…".into()
                    } else {
                        "Scanning the full source for the nearest point…".into()
                    };
                    return Task::perform(
                        async move {
                            tokio::task::spawn_blocking(move || {
                                cloud.validate_source().map_err(|error| error.to_string())?;
                                let result = if let Some(tree) = tree {
                                    pick_indexed_transformed(
                                        &tree,
                                        projection,
                                        PickTarget {
                                            pointer: end,
                                            radius: 8.0,
                                        },
                                        filter,
                                        deleted.as_deref(),
                                        transform,
                                        &cancel,
                                    )?
                                } else {
                                    pick_full_transformed(
                                        &cloud,
                                        projection,
                                        PickTarget {
                                            pointer: end,
                                            radius: 8.0,
                                        },
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
                        move |result| Message::PickReady(revision, index, result),
                    );
                }
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

    fn section_bounds(&self) -> Option<Bounds> {
        if !self.section_enabled {
            return None;
        }
        let overall = self
            .section_reference_bounds
            .or_else(|| combined_bounds(&self.clouds))?;
        let mut bounds = overall;
        for axis in 0..3 {
            let span = overall.max[axis] - overall.min[axis];
            bounds.min[axis] = overall.min[axis] + span * self.section_min_percent[axis] / 100.0;
            bounds.max[axis] = overall.min[axis] + span * self.section_max_percent[axis] / 100.0;
        }
        Some(bounds)
    }

    fn sync_section_coordinate_inputs(&mut self) {
        if let Some(section) = self.section_bounds() {
            for axis in 0..3 {
                self.section_coordinate_inputs[axis] = [
                    format!("{:.6}", section.min[axis]),
                    format!("{:.6}", section.max[axis]),
                ];
            }
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
            .map(|field| field.parse::<f64>().ok());
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

    fn preserve_camera_for_scene_change(&mut self, old_scene: Option<Bounds>) {
        let (Some(old_scene), Some(new_scene)) = (old_scene, combined_bounds(&self.clouds)) else {
            return;
        };
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
        let tab = |label, value| {
            button(text(label).size(12))
                .on_press(Message::Tab(value))
                .style(move |theme, status| {
                    opencad_ribbon::tab_style(
                        theme,
                        !self.file_open && self.ribbon_tab == value,
                        status,
                    )
                })
                .padding([5, 13])
        };
        let tabs = row![
            button(text("File").size(12))
                .on_press(Message::ToggleFile)
                .style(|theme, status| {
                    opencad_ribbon::file_tab_style(theme, self.file_open, status)
                })
                .padding([5, 13]),
            tab("Home", RibbonTab::Home),
            tab("View", RibbonTab::View),
            tab("Select", RibbonTab::Select),
            tab("Tools", RibbonTab::Tools),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center)
        .padding([1, 8]);
        let quick_access = row![
            opencad_ribbon::quick_access_btn(
                icon_svg(ToolIcon::Open, 20.0),
                "Import point cloud",
                Some(Message::Open),
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
        let tab_bar = container(
            row![quick_access, tabs, iced::widget::horizontal_space()]
                .width(Fill)
                .align_y(iced::Alignment::Center)
                .padding([0, 8]),
        )
        .width(Fill)
        .height(29)
        .style(|theme| container::Style::default().background(ui_theme::colors(theme).tabs));
        if self.file_open {
            return container(tab_bar).width(Fill).style(ribbon_style).into();
        }

        let mesh_available = self
            .active
            .and_then(|index| self.clouds.get(index))
            .is_some_and(|entry| entry.mesh.is_some());
        let mut surface_tools = vec![
            opencad_ribbon::RibbonItem::Large(ribbon_button_when(
                "Terrain mesh",
                Message::MeshRequest(MeshMode::Terrain),
                self.active.is_some() && self.mesh_job.is_none() && !self.mesh_dialog_pending,
            )),
            opencad_ribbon::RibbonItem::Large(ribbon_button_when(
                "3D surface",
                Message::MeshRequest(MeshMode::Surface),
                self.active.is_some() && self.mesh_job.is_none() && !self.mesh_dialog_pending,
            )),
        ];
        if self.mesh_job.is_some() {
            surface_tools.push(opencad_ribbon::RibbonItem::Small(small_tool_button(
                "Cancel mesh",
                Message::CancelMesh,
                false,
            )));
        }
        surface_tools.push(opencad_ribbon::RibbonItem::Small(small_tool_button_when(
            "Export mesh",
            Message::ExportMesh,
            false,
            mesh_available && !self.mesh_export_pending,
        )));
        let mut scale_tools = row![
            column![
                row![
                    text("X").size(10).width(12),
                    text_input("1", &self.scale_inputs[0])
                        .on_input(|value| Message::ScaleAxis(0, value))
                        .size(11)
                        .padding([2, 4])
                        .width(76)
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
                row![
                    text("Y").size(10).width(12),
                    text_input("1", &self.scale_inputs[1])
                        .on_input(|value| Message::ScaleAxis(1, value))
                        .size(11)
                        .padding([2, 4])
                        .width(76)
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
                row![
                    text("Z").size(10).width(12),
                    text_input("1", &self.scale_inputs[2])
                        .on_input(|value| Message::ScaleAxis(2, value))
                        .size(11)
                        .padding([2, 4])
                        .width(76)
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(1),
            ribbon_button_when(
                if self.scale_job.is_some() {
                    "Working…"
                } else {
                    "Apply"
                },
                Message::ApplyScale,
                self.active.is_some() && self.scale_job.is_none()
            ),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        if self.scale_job.is_some() {
            scale_tools = scale_tools.push(ribbon_button("Cancel", Message::CancelScale));
        }
        let mut detail_tools = vec![
            opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                "Build index",
                Message::BuildIndex,
                false,
                self.active.is_some() && !self.index_pending,
            )),
            opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                "Refresh LOD",
                Message::LoadDetail,
                false,
                self.active
                    .and_then(|index| self.clouds.get(index))
                    .is_some_and(|entry| entry.index.is_some()),
            )),
        ];
        if self.index_pending {
            detail_tools.push(opencad_ribbon::RibbonItem::Small(small_tool_button(
                "Cancel index",
                Message::CancelIndex,
                false,
            )));
        }
        let groups: Element<'_, Message> = match self.ribbon_tab {
            RibbonTab::Home => row![
                opencad_ribbon::render_group_items(
                    "FILE",
                    vec![
                        opencad_ribbon::RibbonItem::Large(ribbon_button("Import", Message::Open)),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Export full",
                            Message::Export,
                            false,
                            self.active.is_some(),
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Export selected",
                            Message::ExportSelection,
                            false,
                            self.selected_total() > 0,
                        )),
                    ],
                ),
                opencad_ribbon::render_group_items(
                    "COLOR",
                    vec![
                        opencad_ribbon::RibbonItem::Small(small_color_button(
                            "RGB",
                            ColorMode::Rgb,
                            self.color_mode
                        )),
                        opencad_ribbon::RibbonItem::Small(small_color_button(
                            "Elevation",
                            ColorMode::Elevation,
                            self.color_mode
                        )),
                        opencad_ribbon::RibbonItem::Small(small_color_button(
                            "Intensity",
                            ColorMode::Intensity,
                            self.color_mode
                        )),
                        opencad_ribbon::RibbonItem::Small(small_color_button(
                            "Classification",
                            ColorMode::Classification,
                            self.color_mode
                        )),
                    ],
                ),
                ribbon_group(
                    "DISPLAY",
                    column![
                        row![
                            text("Size").size(11).width(43),
                            slider(0.1..=20.0, self.point_size, Message::PointSize)
                                .step(0.1_f32)
                                .width(102),
                            text(format!("{:.1}", self.point_size)).size(11).width(32),
                        ]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("Budget").size(11).width(43),
                            slider(100_000..=MAX_POINT_BUDGET, self.budget, Message::Budget)
                                .step(100_000_u32)
                                .width(102),
                            text(if self.budget >= 1_000_000 {
                                format!("{:.1}M", self.budget as f64 / 1_000_000.0)
                            } else {
                                format!("{}k", self.budget / 1_000)
                            })
                            .size(11)
                            .width(32),
                        ]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                    ]
                    .spacing(9)
                    .into()
                ),
                opencad_ribbon::render_group_items(
                    "VIEW",
                    vec![
                        opencad_ribbon::RibbonItem::Large(ribbon_button(
                            "Zoom all",
                            Message::ResetCamera
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Top",
                            Message::CameraPreset(CameraPreset::Top),
                            self.view_label == "TOP"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Front",
                            Message::CameraPreset(CameraPreset::Front),
                            self.view_label == "FRONT"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Isometric",
                            Message::CameraPreset(CameraPreset::Isometric),
                            self.view_label == "ISOMETRIC"
                        )),
                    ],
                ),
                opencad_ribbon::render_group_items(
                    "RENDER",
                    vec![opencad_ribbon::RibbonItem::Large(tool_button(
                        "Eye-dome",
                        Message::SetEyeDome(!self.eye_dome),
                        self.eye_dome,
                    ))],
                ),
                ribbon_group(
                    "THEME",
                    column![
                        text("Appearance").size(11),
                        pick_list(UiTheme::ALL, Some(self.ui_theme), Message::Theme)
                            .style(themed_pick_list_style)
                            .width(142),
                    ]
                    .spacing(5)
                    .into(),
                ),
            ]
            .spacing(6)
            .into(),
            RibbonTab::View => row![
                opencad_ribbon::render_group_items(
                    "CAMERA VIEWS",
                    vec![
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Top",
                            Message::CameraPreset(CameraPreset::Top),
                            self.view_label == "TOP"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Front",
                            Message::CameraPreset(CameraPreset::Front),
                            self.view_label == "FRONT"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Right",
                            Message::CameraPreset(CameraPreset::Right),
                            self.view_label == "RIGHT"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Bottom",
                            Message::CameraPreset(CameraPreset::Bottom),
                            self.view_label == "BOTTOM"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Back",
                            Message::CameraPreset(CameraPreset::Back),
                            self.view_label == "BACK"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Left",
                            Message::CameraPreset(CameraPreset::Left),
                            self.view_label == "LEFT"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Isometric",
                            Message::CameraPreset(CameraPreset::Isometric),
                            self.view_label == "ISOMETRIC"
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Save view",
                            Message::SaveView,
                            false,
                            self.active.is_some(),
                        )),
                    ],
                ),
                opencad_ribbon::render_group_items(
                    "SCANNERS",
                    vec![
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Stations",
                            Message::ShowScanPoses(!self.show_scan_poses),
                            self.show_scan_poses,
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Fit stations",
                            Message::FitScanPoses,
                            false,
                            self.clouds.iter().any(|entry| {
                                entry.visible && !entry.cloud.scan_poses.is_empty()
                            }),
                        )),
                    ],
                ),
                ribbon_group(
                    "POINT DISPLAY",
                    column![
                        text(format!("Point size  {:.1}", self.point_size)).size(12),
                        slider(0.1..=20.0, self.point_size, Message::PointSize)
                            .step(0.1_f32)
                            .width(140),
                    ]
                    .spacing(5)
                    .into()
                ),
                ribbon_group(
                    "DEPTH",
                    row![
                        tool_button(
                            "Eye-dome",
                            Message::SetEyeDome(!self.eye_dome),
                            self.eye_dome,
                        ),
                        column![
                            text("Strength").size(11),
                            row![
                                slider(0.0..=5.0, self.eye_dome_strength, Message::EyeDomeStrength)
                                    .step(0.1_f32)
                                    .width(85),
                                text(format!("{:.1}", self.eye_dome_strength))
                                    .size(11)
                                    .width(24),
                            ]
                            .spacing(4)
                            .align_y(iced::Alignment::Center),
                        ]
                        .spacing(5),
                    ]
                    .spacing(7)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
                opencad_ribbon::render_group_items(
                    "SECTION BOX",
                    vec![
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Section box",
                            Message::SetSectionEnabled(!self.section_enabled),
                            self.section_enabled,
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Reset box",
                            Message::ResetSectionBox,
                            false,
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Zoom box",
                            Message::ZoomToSection,
                            false,
                            self.section_bounds().is_some(),
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Fit selection",
                            Message::FitSectionToSelection,
                            false,
                            self.selected_total() > 0 && !self.selection_bounds_pending,
                        )),
                    ],
                ),
                ribbon_group(
                    "POINT BUDGET",
                    column![
                        text(format!("{} preview points", format_count(self.budget))).size(12),
                        slider(100_000..=MAX_POINT_BUDGET, self.budget, Message::Budget)
                            .step(100_000_u32)
                            .width(140),
                    ]
                    .spacing(5)
                    .into()
                ),
                ribbon_group(
                    "CLASSIFICATION",
                    column![
                        row![
                            checkbox("Ground", self.filter_ground)
                                .on_toggle(Message::FilterGround)
                                .style(muted_checkbox_style)
                                .text_size(11)
                                .size(12),
                            checkbox("Vegetation", self.filter_vegetation)
                                .on_toggle(Message::FilterVegetation)
                                .style(muted_checkbox_style)
                                .text_size(11)
                                .size(12),
                        ]
                        .spacing(9),
                        row![
                            checkbox("Buildings", self.filter_buildings)
                                .on_toggle(Message::FilterBuildings)
                                .style(muted_checkbox_style)
                                .text_size(11)
                                .size(12),
                            checkbox("Other", self.filter_other)
                                .on_toggle(Message::FilterOther)
                                .style(muted_checkbox_style)
                                .text_size(11)
                                .size(12),
                        ]
                        .spacing(9),
                    ]
                    .spacing(10)
                    .into()
                ),
            ]
            .spacing(6)
            .into(),
            RibbonTab::Select => row![
                opencad_ribbon::render_group_items(
                    "SELECTION",
                    vec![
                        opencad_ribbon::RibbonItem::Large(tool_button(
                            "Box select",
                            Message::ToggleBoxSelect,
                            self.box_select
                        )),
                        opencad_ribbon::RibbonItem::Large(tool_button(
                            "Pick point",
                            Message::TogglePickSelect,
                            self.pick_mode
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Clear",
                            Message::ClearSelection,
                            false
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Zoom selection",
                            Message::ZoomToSelection,
                            false,
                            self.selected_total() > 0 && !self.selection_bounds_pending,
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Crop",
                            Message::ExportSelection,
                            false
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Delete",
                            Message::DeleteSelection,
                            false,
                            self.selected_total() > 0,
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Undo",
                            Message::UndoDelete,
                            false,
                            !self.undo_deletions.is_empty(),
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button_when(
                            "Redo",
                            Message::RedoDelete,
                            false,
                            !self.redo_deletions.is_empty(),
                        )),
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Save minus",
                            Message::RemoveSelection,
                            false
                        )),
                    ]
                    .into_iter()
                    .chain(self.selection_pending.then(|| {
                        opencad_ribbon::RibbonItem::Small(small_tool_button(
                            "Cancel selection",
                            Message::CancelSelection,
                            false,
                        ))
                    }))
                    .collect(),
                ),
                ribbon_group(
                    "RESULT",
                    column![
                        text(format!(
                            "{} point{} selected",
                            format_count(self.selected_total()),
                            if self.selected_total() == 1 { "" } else { "s" }
                        ))
                        .size(15),
                        text("Box: octree or full scan  ·  Pick: active cloud index")
                            .size(11)
                            .color(self.ui_theme.colors().muted),
                    ]
                    .spacing(6)
                    .into()
                ),
            ]
            .spacing(6)
            .into(),
            RibbonTab::Tools => row![
                ribbon_group(
                    "TRANSLATE",
                    row![
                        column![
                            row![
                                text("X").size(10).width(12),
                                text_input("0", &self.translate_x)
                                    .on_input(Message::TranslateX)
                                    .size(11)
                                    .padding([2, 4])
                                    .width(76)
                            ]
                            .spacing(4)
                            .align_y(iced::Alignment::Center),
                            row![
                                text("Y").size(10).width(12),
                                text_input("0", &self.translate_y)
                                    .on_input(Message::TranslateY)
                                    .size(11)
                                    .padding([2, 4])
                                    .width(76)
                            ]
                            .spacing(4)
                            .align_y(iced::Alignment::Center),
                            row![
                                text("Z").size(10).width(12),
                                text_input("0", &self.translate_z)
                                    .on_input(Message::TranslateZ)
                                    .size(11)
                                    .padding([2, 4])
                                    .width(76)
                            ]
                            .spacing(4)
                            .align_y(iced::Alignment::Center),
                        ]
                        .spacing(1),
                        ribbon_button_when(
                            "Apply",
                            Message::ApplyTranslation,
                            self.active.is_some()
                        ),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
                ribbon_group("SCALE", scale_tools.into()),
                ribbon_group(
                    "THIN",
                    row![
                        column![
                            text(format!("Keep {}%", self.thin_percent)).size(11),
                            slider(1..=100, self.thin_percent, Message::ThinPercent).width(110),
                        ]
                        .spacing(7),
                        ribbon_button_when(
                            if self.thin_pending {
                                "Working…"
                            } else {
                                "Apply"
                            },
                            Message::Thin,
                            self.active.is_some() && !self.thin_pending,
                        ),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
                opencad_ribbon::render_group_items("SURFACE", surface_tools),
                ribbon_group(
                    "CITY DATA",
                    tool_button("3D BAG", Message::ToggleBagPanel, self.bag_panel),
                ),
                opencad_ribbon::render_group_items("DETAIL LOD", detail_tools),
                opencad_ribbon::render_group_items(
                    "AUTO INDEX",
                    vec![opencad_ribbon::RibbonItem::Small(small_tool_button(
                        "Auto-index scans",
                        Message::SetAutoIndex(!self.auto_index),
                        self.auto_index,
                    ))],
                ),
                ribbon_group(
                    "DECIMATE",
                    column![
                        row![
                            text("Keep 1 in").size(11),
                            pick_list(
                                [2u64, 5, 10, 20, 50, 100],
                                Some(self.decimation_stride),
                                Message::DecimationStride
                            )
                            .style(themed_pick_list_style)
                            .width(66),
                        ]
                        .spacing(5)
                        .align_y(iced::Alignment::Center),
                        small_tool_button_when(
                            "Apply decimation",
                            Message::Decimate,
                            false,
                            self.active.is_some(),
                        ),
                    ]
                    .spacing(4)
                    .into()
                ),
            ]
            .spacing(2)
            .into(),
        };
        let group_strip = scrollable(
            container(groups)
                .padding([0, 4])
                .width(iced::Length::Shrink)
                .height(opencad_ribbon::TOOL_BAR_H),
        )
        .id(self.ribbon_tab.scroll_id())
        .on_scroll(move |viewport| {
            Message::RibbonViewport(
                self.ribbon_tab,
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
                tab_bar,
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
        let cloud_count = match self.clouds.len() {
            1 => "1 point cloud".to_owned(),
            count => format!("{count} point clouds"),
        };
        let mut files = column![
            text("PROJECT")
                .size(14)
                .font(Font::with_name("Space Grotesk")),
            text(cloud_count)
                .size(11)
                .color(self.ui_theme.colors().muted),
            button("+  Add point cloud")
                .on_press(Message::Open)
                .style(flat_tool_style)
                .width(Fill),
        ]
        .spacing(9);
        for (index, entry) in self.clouds.iter().enumerate() {
            let name = display_name(&entry.cloud.path);
            let readable_name = name.replace('_', "_\u{200b}");
            let file_button = tooltip(
                button(
                    text(readable_name)
                        .size(12)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .width(Fill),
                )
                .on_press(Message::Select(index))
                .style(flat_tool_style)
                .width(Fill)
                .padding([4, 2]),
                container(text(entry.cloud.path.display().to_string()).size(11))
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
            let selected = entry.selection.as_ref().map_or(0, |mask| mask.count);
            let deleted = entry.deleted_count();
            let mut summary = format!("{} points", format_count(entry.remaining_count()));
            if selected > 0 {
                summary.push_str(&format!("  ·  {} selected", format_count(selected)));
            }
            if deleted > 0 {
                summary.push_str(&format!("  ·  {} deleted", format_count(deleted)));
            }
            let index_state = if entry.index.is_some() {
                "LOD ready"
            } else if entry.index_building {
                "Indexing"
            } else if entry.auto_index_queued {
                "LOD queued"
            } else {
                "Not indexed"
            };
            let mut item = column![
                row![
                    checkbox("", entry.visible)
                        .on_toggle(move |value| Message::SetVisible(index, value))
                        .style(muted_checkbox_style),
                    file_button,
                    button("×")
                        .on_press(Message::Remove(index))
                        .style(flat_tool_style),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
                text(summary).size(10).color(self.ui_theme.colors().muted),
                text(index_state)
                    .size(10)
                    .color(self.ui_theme.colors().muted),
            ]
            .spacing(3);
            if entry.mesh.is_some() {
                item = item.push(
                    checkbox("Surface", entry.mesh_visible)
                        .on_toggle(move |value| Message::SetMeshVisible(index, value))
                        .style(muted_checkbox_style)
                        .text_size(11)
                        .size(12),
                );
            }
            let active = self.active == Some(index);
            files = files.push(
                container(item)
                    .padding([6, 5])
                    .width(Fill)
                    .style(move |theme| {
                        let colors = ui_theme::colors(theme);
                        container::Style::default()
                            .background(if active {
                                colors.panel_alt
                            } else {
                                colors.panel
                            })
                            .border(iced::Border {
                                color: if active { colors.accent } else { colors.border },
                                width: 1.0,
                                radius: 2.0.into(),
                            })
                    }),
            );
        }
        container(scrollable(files.padding(14)).height(Fill))
            .width(255)
            .height(Fill)
            .style(sidebar_style)
            .into()
    }

    fn bag_panel_view(&self) -> Element<'_, Message> {
        let map = stack![
            image(self.bag_map_raster.clone())
                .width(Fill)
                .height(bag_map::HEIGHT)
                .content_fit(iced::ContentFit::Fill),
            Canvas::new(BagMap {
                center: self.bag_map_center,
                zoom: self.bag_map_zoom,
                drawing: self.bag_map_drawing,
                selected: self.bag_fields_bounds(),
            })
            .width(Fill)
            .height(bag_map::HEIGHT),
        ]
        .width(Fill)
        .height(bag_map::HEIGHT);
        let mut panel = column![
            row![
                text("3D BAG")
                    .size(15)
                    .font(Font::with_name("Space Grotesk"))
                    .width(Fill),
                button("×")
                    .on_press(Message::ToggleBagPanel)
                    .style(flat_tool_style),
            ]
            .align_y(iced::Alignment::Center),
            text("Download buildings in RD New + NAP (EPSG:7415).")
                .size(11)
                .color(self.ui_theme.colors().muted),
            container(map)
                .width(Fill)
                .height(bag_map::HEIGHT)
                .clip(true),
            row![
                button(if self.bag_map_drawing {
                    "Cancel draw"
                } else {
                    "Draw area"
                })
                .on_press(Message::BagMapDraw(!self.bag_map_drawing))
                .style(flat_tool_style),
                button("Fit area")
                    .on_press(Message::BagMapFitFields)
                    .style(flat_tool_style),
                button("Amsterdam")
                    .on_press(Message::BagMapHome)
                    .style(flat_tool_style),
            ]
            .spacing(6),
            row![
                button("−")
                    .on_press(Message::BagMapZoom(
                        -1.0,
                        UiPoint::new(bag_map::WIDTH * 0.5, bag_map::HEIGHT * 0.5),
                    ))
                    .style(flat_tool_style),
                button("+")
                    .on_press(Message::BagMapZoom(
                        1.0,
                        UiPoint::new(bag_map::WIDTH * 0.5, bag_map::HEIGHT * 0.5),
                    ))
                    .style(flat_tool_style),
                text("Drag to pan · scroll to zoom").size(10),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            row![
                text("© Kadaster (BRT) via PDOK · CC BY 4.0").size(10),
                button("Licentie ↗")
                    .on_press(Message::OpenPdokLicense)
                    .style(flat_tool_style),
            ]
            .spacing(5)
            .align_y(iced::Alignment::Center),
            button("Use scan / section box")
                .on_press(Message::BagFromSection)
                .style(flat_tool_style),
        ]
        .spacing(10)
        .padding(12)
        .width(Fill);
        for (index, label) in ["X min", "Y min", "X max", "Y max"].into_iter().enumerate() {
            panel = panel.push(
                column![
                    text(label).size(11),
                    text_input(label, &self.bag_fields[index])
                        .on_input(move |value| Message::BagField(index, value))
                        .width(Fill),
                ]
                .spacing(3),
            );
        }
        panel = panel
            .push(text("Level of detail").size(11))
            .push(
                pick_list(BagLod::ALL, Some(self.bag_lod), Message::BagLod)
                    .style(themed_pick_list_style),
            )
            .push(
                button(if self.bag_pending {
                    "Downloading…"
                } else {
                    "Download OBJ"
                })
                .on_press_maybe((!self.bag_pending).then_some(Message::BagDownload))
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status)),
            );
        if let Some(stats) = self.bag_last_stats {
            panel = panel.push(
                text(format!(
                    "{} buildings · {} triangles · {} pages",
                    stats.buildings, stats.triangles, stats.pages
                ))
                .size(11),
            );
        }
        panel
            .push(text("© 3DBAG door tudelft3d en 3DGI").size(10))
            .push(
                button("CC BY 4.0 · bron en licentie ↗")
                    .on_press(Message::OpenBagLicense)
                    .style(flat_tool_style),
            )
            .into()
    }

    fn point_viewport(&self) -> PointViewport<'_> {
        PointViewport {
            clouds: &self.clouds,
            color_mode: self.color_mode,
            point_size: self.point_size,
            eye_dome: self.eye_dome,
            eye_dome_strength: self.eye_dome_strength,
            show_scan_poses: self.show_scan_poses,
            budget: self.budget as usize,
            filter_ground: self.filter_ground,
            filter_vegetation: self.filter_vegetation,
            filter_buildings: self.filter_buildings,
            filter_other: self.filter_other,
            class_visibility: self.class_visibility,
            section: self.section_bounds(),
            section_reference: self.section_reference_bounds,
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
            pan: self.pan,
            box_select: self.box_select,
            pick_mode: self.pick_mode,
            drag_rectangle: self.drag_rectangle,
            context_menu: self.context_menu,
            viewport_size: self.viewport_size,
        }
    }

    fn file_view(&self) -> Element<'_, Message> {
        let action = |label: &'static str, action: FileAction, available: bool| {
            button(text(label).size(14))
                .on_press_maybe(available.then_some(Message::FileAction(action)))
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(Fill)
                .padding([11, 18])
        };
        let active_cloud = self.active.and_then(|index| self.clouds.get(index));
        let selected = self.selected_total();
        let active_selected = active_cloud
            .and_then(|entry| entry.selection.as_ref())
            .map_or(0, |selection| selection.count);
        let menu = column![
            container(text("FILE").size(12).color(self.ui_theme.colors().accent)).padding([20, 18]),
            action("Import point cloud…", FileAction::Import, true),
            container(text("EXPORT").size(10).color(self.ui_theme.colors().muted)).padding(
                iced::Padding {
                    top: 22.0,
                    right: 18.0,
                    bottom: 7.0,
                    left: 18.0,
                }
            ),
            action(
                "Full resolution…",
                FileAction::ExportFull,
                active_cloud.is_some()
            ),
            action(
                "Selected points…",
                FileAction::ExportSelection,
                active_selected > 0
            ),
            action(
                "Section box…",
                FileAction::ExportSection,
                active_cloud.is_some() && self.section_enabled && !self.section_export_pending,
            ),
            action(
                "Merge visible LAS/LAZ scans…",
                FileAction::MergeVisible,
                self.merge_job.is_none()
                    && !self.merge_dialog_pending
                    && self.visible_merge_sources().is_ok(),
            ),
            action(
                "Cancel merge",
                FileAction::CancelMerge,
                self.merge_job.is_some(),
            ),
            action(
                "Surface mesh…",
                FileAction::ExportMesh,
                active_cloud.is_some_and(|entry| entry.mesh.is_some()) && !self.mesh_export_pending,
            ),
            iced::widget::vertical_space(),
            button(text("←  Return to model").size(13))
                .on_press(Message::ToggleFile)
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(Fill)
                .padding([13, 18]),
        ]
        .width(260)
        .height(Fill);
        let menu = container(menu).width(260).height(Fill).style(sidebar_style);

        let total_points: u64 = self.clouds.iter().map(CloudEntry::remaining_count).sum();
        let active_name = active_cloud
            .map(|entry| display_name(&entry.cloud.path))
            .unwrap_or("No active scan");
        let open_scans = self.clouds.iter().enumerate().fold(
            column![].spacing(3).width(Fill),
            |rows, (index, entry)| {
                rows.push(
                    button(
                        row![
                            text(display_name(&entry.cloud.path)).size(13),
                            iced::widget::horizontal_space(),
                            text(format!("{} points", format_count(entry.remaining_count())))
                                .size(11)
                                .color(self.ui_theme.colors().muted),
                        ]
                        .spacing(16)
                        .align_y(iced::Alignment::Center),
                    )
                    .on_press(Message::FileAction(FileAction::Activate(index)))
                    .style(move |theme, status| {
                        opencad_ribbon::tool_btn_style(theme, self.active == Some(index), status)
                    })
                    .width(Fill)
                    .padding([9, 12]),
                )
            },
        );
        let mut details = column![
            text("Point cloud workspace")
                .size(26)
                .font(Font::with_name("Space Grotesk")),
            text(format!(
                "{} files  ·  {} points  ·  {} selected",
                self.clouds.len(),
                format_count(total_points),
                format_count(selected),
            ))
            .size(13)
            .color(self.ui_theme.colors().muted),
            container(
                text("CURRENT SCAN")
                    .size(10)
                    .color(self.ui_theme.colors().muted)
            )
            .padding(iced::Padding {
                top: 28.0,
                bottom: 4.0,
                ..iced::Padding::ZERO
            }),
            text(active_name).size(16),
            container(
                text("OPEN SCANS")
                    .size(10)
                    .color(self.ui_theme.colors().muted)
            )
            .padding(iced::Padding {
                top: 28.0,
                bottom: 4.0,
                ..iced::Padding::ZERO
            }),
            container(open_scans).width(Fill).max_width(560),
            container(
                text("EXPORT FORMAT")
                    .size(10)
                    .color(self.ui_theme.colors().muted)
            )
            .padding(iced::Padding {
                top: 28.0,
                bottom: 4.0,
                ..iced::Padding::ZERO
            }),
            pick_list(
                ExportFormat::ALL,
                Some(self.export_format),
                Message::ExportFormat,
            )
            .style(themed_pick_list_style)
            .width(240),
            container(
                text("APPEARANCE")
                    .size(10)
                    .color(self.ui_theme.colors().muted)
            )
            .padding(iced::Padding {
                top: 28.0,
                bottom: 4.0,
                ..iced::Padding::ZERO
            }),
            pick_list(UiTheme::ALL, Some(self.ui_theme), Message::Theme)
                .style(themed_pick_list_style)
                .width(240),
            container(
                text("Choose an export format, then save the active scan or selection.")
                    .size(12)
                    .color(self.ui_theme.colors().muted),
            )
            .padding(iced::Padding {
                top: 32.0,
                ..iced::Padding::ZERO
            }),
        ]
        .spacing(8)
        .width(Fill);
        if let Some(job) = &self.merge_job {
            let processed = job.control.processed.load(Ordering::Relaxed);
            details = details
                .push(text(job.progress_text()).size(13))
                .push(
                    iced::widget::progress_bar(
                        0.0..=1.0,
                        processed as f32 / job.control.total.max(1) as f32,
                    )
                    .height(8),
                )
                .push(
                    text(format!(
                        "{} points written to {}",
                        format_count(job.control.written.load(Ordering::Relaxed)),
                        job.path.display()
                    ))
                    .size(11),
                )
                .push(button("Cancel merge").on_press(Message::CancelMerge));
        }
        row![
            menu,
            container(scrollable(details).height(Fill))
                .padding([30, 40])
                .width(Fill)
                .height(Fill)
                .style(|theme| container::Style::default()
                    .background(ui_theme::colors(theme).panel_alt)),
        ]
        .height(Fill)
        .into()
    }

    fn view(&self) -> Element<'_, Message> {
        if self.file_open {
            let total_points: u64 = self.clouds.iter().map(CloudEntry::remaining_count).sum();
            let status_bar = row![
                text(&self.status).size(11),
                text(format!(
                    "{} files  ·  {} points  ·  {} selected",
                    self.clouds.len(),
                    format_count(total_points),
                    format_count(self.selected_total())
                ))
                .size(11),
            ]
            .spacing(24)
            .padding([7, 12]);
            return column![
                self.ribbon(),
                self.file_view(),
                container(status_bar).width(Fill).style(status_style),
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

        let active_cloud = self.active.and_then(|index| self.clouds.get(index));
        let export_button = if active_cloud.is_some() {
            button("Export full resolution")
                .on_press(Message::Export)
                .style(flat_tool_style)
        } else {
            button("Export full resolution").style(flat_tool_style)
        };
        let section_export_button = button("Export section")
            .on_press_maybe(
                (self.section_enabled && active_cloud.is_some() && !self.section_export_pending)
                    .then_some(Message::ExportSection),
            )
            .style(flat_tool_style);
        let source_points = active_cloud.map_or(0, |entry| entry.cloud.total_points);
        let view_points = active_cloud.map_or(0, CloudEntry::view_len);
        let selected_points = active_cloud
            .and_then(|entry| entry.selection.as_ref())
            .map_or(0, |selection| selection.count);
        let indexed = active_cloud.is_some_and(|entry| entry.index.is_some());
        let filename =
            active_cloud.map_or("No file loaded", |entry| display_name(&entry.cloud.path));
        let mut properties = column![
            container(
                text("Properties")
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
            opencad_properties::property_row("Indexed", if indexed { "Yes" } else { "No" }.into()),
            opencad_properties::property_row("Selected", format_count(selected_points)),
        ]
        .spacing(0)
        .width(270);
        if self.ribbon_tab == RibbonTab::Tools {
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
        if let Some(progress) = self
            .index_progress
            .as_ref()
            .and_then(|value| value.lock().ok().map(|value| *value))
        {
            let cancelling = self.index_cancel.load(Ordering::Relaxed);
            properties = properties
                .push(opencad_properties::section_header("Octree index"))
                .push(
                    container(
                        text(if cancelling {
                            "Cancelling octree build…".into()
                        } else {
                            Self::index_progress_text(progress)
                        })
                        .size(11),
                    )
                    .padding([6, 8]),
                );
            if progress.stage == IndexStage::ReadingSource {
                properties = properties.push(
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
                );
            }
            if !cancelling {
                properties = properties.push(
                    container(button("Cancel index").on_press(Message::CancelIndex))
                        .padding([5, 8]),
                );
            }
        }
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
                    container(button("Cancel mesh").on_press(Message::CancelMesh)).padding([5, 8]),
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
                    container(button("Cancel merge").on_press(Message::CancelMerge))
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
                    container(button("Cancel scale").on_press(Message::CancelScale))
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
                            button("Reset transform")
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
                    .push(
                        container(
                            row![
                                checkbox("Markers", self.show_scan_poses)
                                    .on_toggle(Message::ShowScanPoses)
                                    .style(muted_checkbox_style),
                                button(if self.expand_scan_poses {
                                    "Hide list"
                                } else {
                                    "Show list"
                                })
                                .on_press(Message::ExpandScanPoses(!self.expand_scan_poses))
                                .style(flat_tool_style),
                            ]
                            .spacing(8)
                            .align_y(iced::Alignment::Center),
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
                                        button("Center")
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
                                        None => "Orientation unavailable".into(),
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
        properties = properties
            .push(opencad_properties::section_header("Camera views"))
            .push(opencad_properties::property_row(
                "Yaw / pitch",
                format!(
                    "{:.0}° / {:.0}°",
                    self.yaw.to_degrees(),
                    self.pitch.to_degrees()
                ),
            ))
            .push(opencad_properties::property_row(
                "Zoom",
                format_zoom_level(self.zoom),
            ))
            .push(
                container(
                    row![
                        text_input("View name", &self.view_name)
                            .on_input(Message::ViewName)
                            .size(11)
                            .padding([3, 5])
                            .width(Fill),
                        button("Save")
                            .on_press_maybe(active_cloud.is_some().then_some(Message::SaveView))
                            .style(flat_tool_style),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                )
                .padding([5, 8]),
            );
        let active_source = active_cloud.map(|entry| camera_views::source_key(&entry.cloud.path));
        for (index, view) in self.saved_views.iter().enumerate() {
            if active_source.as_ref() != Some(&view.source) {
                continue;
            }
            properties = properties.push(
                container(
                    row![
                        button(text(view.name.as_str()).size(11))
                            .on_press(Message::RestoreView(index))
                            .style(flat_tool_style)
                            .width(Fill),
                        button("×")
                            .on_press(Message::DeleteView(index))
                            .style(flat_tool_style),
                    ]
                    .spacing(3),
                )
                .padding([2, 8]),
            );
        }
        properties = properties
            .push(opencad_properties::section_header("Section box"))
            .push(
                container(
                    checkbox("Enabled", self.section_enabled)
                        .on_toggle(Message::SetSectionEnabled)
                        .style(muted_checkbox_style),
                )
                .padding([6, 8]),
            );
        if self.section_enabled {
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
            properties = properties
                .push(container(text("XYZ limits · model coordinates").size(10)).padding([7, 8]));
            for (axis, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                properties = properties.push(
                    container(
                        row![
                            text(label).size(11).width(15),
                            text_input("Min", &self.section_coordinate_inputs[axis][0])
                                .on_input(move |value| Message::SectionCoordinate(
                                    axis, true, value
                                ))
                                .size(11)
                                .width(Fill),
                            text_input("Max", &self.section_coordinate_inputs[axis][1])
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
                    button("Apply XYZ limits")
                        .on_press(Message::ApplySectionCoordinates)
                        .style(flat_tool_style),
                )
                .padding([3, 8]),
            );
        }
        properties = properties.push(
            container(
                row![
                    button("Fit to selection")
                        .on_press_maybe(
                            (self.selected_total() > 0 && !self.selection_bounds_pending)
                                .then_some(Message::FitSectionToSelection),
                        )
                        .style(flat_tool_style),
                    button("Zoom box")
                        .on_press_maybe(self.section_enabled.then_some(Message::ZoomToSection))
                        .style(flat_tool_style),
                ]
                .spacing(3),
            )
            .padding([3, 8]),
        );
        properties = properties
            .push(opencad_properties::section_header("Export"))
            .push(
                container(
                    pick_list(
                        ExportFormat::ALL,
                        Some(self.export_format),
                        Message::ExportFormat,
                    )
                    .style(themed_pick_list_style),
                )
                .padding([6, 8]),
            )
            .push(container(export_button).padding([0, 8]))
            .push(container(section_export_button).padding([4, 8]));
        if let Some(mesh) = active_cloud.and_then(|entry| entry.mesh.as_ref()) {
            properties = properties
                .push(opencad_properties::section_header("Surface mesh"))
                .push(opencad_properties::property_row(
                    "Vertices",
                    format_count(mesh.vertices.len()),
                ))
                .push(opencad_properties::property_row(
                    "Triangles",
                    format_count(mesh.triangles.len()),
                ))
                .push(
                    container(
                        button("Export mesh as OBJ")
                            .on_press_maybe(
                                (!self.mesh_export_pending).then_some(Message::ExportMesh),
                            )
                            .style(flat_tool_style),
                    )
                    .padding([4, 8]),
                );
        }
        let mut properties = properties
            .push(opencad_properties::section_header("Display"))
            .push(
                container(
                    pick_list(ColorMode::ALL, Some(self.color_mode), Message::ColorMode)
                        .style(themed_pick_list_style),
                )
                .padding([6, 8]),
            )
            .push(
                container(
                    checkbox("Eye-dome", self.eye_dome)
                        .on_toggle(Message::SetEyeDome)
                        .style(muted_checkbox_style),
                )
                .padding([2, 8]),
            );
        if self.eye_dome {
            properties = properties.push(
                container(
                    row![
                        text("Strength").size(11).width(52),
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
        if self.color_mode == ColorMode::Classification
            && active_cloud.is_some_and(|entry| entry.cloud.has_classification)
        {
            properties = properties
                .push(opencad_properties::section_header("Class visibility"))
                .push(container(text("View groups also apply").size(10)).padding([4, 8]));
            for &(code, label) in ASPRS_CLASSIFICATIONS {
                properties = properties.push(
                    container(
                        checkbox(
                            format!("{code:02}  {label}"),
                            self.class_visibility.allows(Some(code)),
                        )
                        .on_toggle(move |visible| Message::FilterClass(code, visible))
                        .style(muted_checkbox_style),
                    )
                    .padding([2, 8]),
                );
            }
        }
        let properties: Element<'_, Message> = if self.bag_panel {
            self.bag_panel_view()
        } else {
            properties.into()
        };

        let viewport_header = row![
            text("MODEL SPACE")
                .size(12)
                .font(Font::with_name("Space Grotesk"))
                .color(Color::from_rgb8(250, 250, 249)),
            text(if self.box_select {
                "BOX SELECT ACTIVE"
            } else {
                self.view_label
            })
            .size(11)
            .color(Color::from_rgb8(161, 161, 170)),
        ]
        .spacing(16)
        .padding([8, 14]);
        let mut viewport = column![viewport_header, container(canvas).width(Fill).height(Fill)]
            .height(Fill)
            .width(Fill);
        if self
            .clouds
            .iter()
            .any(|entry| entry.bag_source && (entry.visible || entry.mesh_visible))
        {
            viewport = viewport.push(
                container(
                    row![
                        text("© 3DBAG door tudelft3d en 3DGI").size(10),
                        button("Bron en licentie ↗")
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
        let total_points: u64 = self.clouds.iter().map(CloudEntry::remaining_count).sum();
        let mesh_status = self.mesh_job.as_ref().map(MeshJob::progress_text);
        let status_bar = row![
            text(mesh_status.unwrap_or_else(|| self.status.clone())).size(11),
            text(format!(
                "{} files  ·  {} points  ·  {} selected",
                self.clouds.len(),
                format_count(total_points),
                format_count(self.selected_total())
            ))
            .size(11),
        ]
        .spacing(24)
        .padding([7, 12]);
        column![
            self.ribbon(),
            content,
            container(status_bar).width(Fill).style(status_style)
        ]
        .height(Fill)
        .into()
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

fn ribbon_button(label: &'static str, message: Message) -> Element<'static, Message> {
    tool_button(label, message, false)
}

fn ribbon_button_when(
    label: &'static str,
    message: Message,
    enabled: bool,
) -> Element<'static, Message> {
    tool_button_when(label, message, false, enabled)
}

fn tool_button(label: &'static str, message: Message, active: bool) -> Element<'static, Message> {
    tool_button_when(label, message, active, true)
}

fn tool_button_when(
    label: &'static str,
    message: Message,
    active: bool,
    enabled: bool,
) -> Element<'static, Message> {
    let icon = tool_icon(&message);
    button(
        column![icon_svg(icon, 30.0), text(label).size(10),]
            .spacing(3)
            .align_x(iced::Alignment::Center),
    )
    .on_press_maybe(enabled.then_some(message))
    .style(move |theme, status| opencad_ribbon::tool_btn_style(theme, active, status))
    .width(72)
    .height(64)
    .padding([5, 4])
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
        row![icon_svg(icon, 18.0), text(label).size(11),]
            .spacing(4)
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
        Message::UndoDelete => ToolIcon::Undo,
        Message::RedoDelete => ToolIcon::Redo,
        Message::ResetCamera => ToolIcon::Fit,
        Message::ZoomToSection => ToolIcon::Fit,
        Message::CameraPreset(preset) => ToolIcon::Camera(*preset),
        Message::SaveView => ToolIcon::Save,
        Message::ApplyTranslation => ToolIcon::Move,
        Message::ApplyScale => ToolIcon::Scale,
        Message::ToggleBoxSelect => ToolIcon::Select,
        Message::TogglePickSelect => ToolIcon::Pick,
        Message::ClearSelection => ToolIcon::Clear,
        Message::SetEyeDome(_) => ToolIcon::Shading,
        Message::ShowScanPoses(_) => ToolIcon::Pick,
        Message::FitScanPoses => ToolIcon::Fit,
        Message::SetSectionEnabled(_)
        | Message::ResetSectionBox
        | Message::FitSectionToSelection
        | Message::ZoomToSelection => ToolIcon::Select,
        _ => ToolIcon::Cloud,
    }
}

fn small_color_button(
    label: &'static str,
    mode: ColorMode,
    current: ColorMode,
) -> Element<'static, Message> {
    let color = match mode {
        ColorMode::Rgb => Color::from_rgb8(139, 158, 169),
        ColorMode::Elevation => Color::from_rgb8(155, 161, 144),
        ColorMode::Intensity => Color::from_rgb8(175, 178, 180),
        ColorMode::Classification => Color::from_rgb8(155, 149, 140),
    };
    button(
        row![
            container(text(" ").size(11))
                .width(12)
                .height(12)
                .style(move |_| container::Style::default().background(color)),
            text(label).size(11),
        ]
        .spacing(5)
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::ColorMode(mode))
    .style(move |theme, status| opencad_ribbon::tool_btn_style(theme, mode == current, status))
    .height(opencad_ribbon::ROW_H)
    .padding([2, 4])
    .into()
}

fn ribbon_group<'a>(label: &'static str, contents: Element<'a, Message>) -> Element<'a, Message> {
    opencad_ribbon::render_group(label, contents)
}

fn ribbon_style(theme: &Theme) -> container::Style {
    container::Style::default().background(ui_theme::colors(theme).shell)
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
    Export,
    Fit,
    Cloud,
    Select,
    Pick,
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
}

// SVG artwork is copied from OpenCADStudio/assets/icons at commit 1fec34d.
fn icon_svg(icon: ToolIcon, size: f32) -> Element<'static, Message> {
    let bytes: &'static [u8] = match icon {
        ToolIcon::Open => include_bytes!("../../assets/opencad-icons/folder_open.svg"),
        ToolIcon::Export => include_bytes!("../../assets/opencad-icons/file_export.svg"),
        ToolIcon::Fit => include_bytes!("../../assets/opencad-icons/zoom_ext.svg"),
        ToolIcon::Cloud => include_bytes!("../../assets/opencad-icons/revcloud.svg"),
        ToolIcon::Select => include_bytes!("../../assets/opencad-icons/select_objects.svg"),
        ToolIcon::Pick => include_bytes!("../../assets/opencad-icons/pick_point.svg"),
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
    };
    svg(svg::Handle::from_memory(bytes))
        .width(size)
        .height(size)
        .into()
}

fn combined_bounds(clouds: &[CloudEntry]) -> Option<Bounds> {
    let mut overall: Option<Bounds> = None;
    // Keep a loaded mesh framed when its source points are hidden for inspection.
    let any_visible = clouds
        .iter()
        .any(|entry| entry.visible || (entry.mesh_visible && entry.mesh.is_some()));
    for entry in clouds.iter().filter(|entry| {
        entry.visible || (entry.mesh_visible && entry.mesh.is_some()) || !any_visible
    }) {
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

fn source_lod_coverage(
    projection: Projection,
    bounds: Bounds,
    section: Option<Bounds>,
) -> Option<f32> {
    let visible_bounds = if let Some(section) = section {
        let clipped = Bounds {
            min: std::array::from_fn(|axis| bounds.min[axis].max(section.min[axis])),
            max: std::array::from_fn(|axis| bounds.max[axis].min(section.max[axis])),
        };
        if (0..3).any(|axis| clipped.min[axis] > clipped.max[axis]) {
            return None;
        }
        clipped
    } else {
        bounds
    };
    projection.screen_coverage(visible_bounds)
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

fn selected_source_bounds(sources: &[SelectedSource]) -> Result<(Bounds, u64), String> {
    let mut bounds = None;
    let mut count = 0u64;
    for source in sources {
        let cloud = &source.cloud;
        let selection = &source.selection;
        let deleted = &source.deleted;
        cloud.validate_source().map_err(|error| error.to_string())?;
        if deleted
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
                include_bounds(&mut bounds, source.transform.xyz(point.xyz));
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
    color_mode: ColorMode,
    point_size: f32,
    eye_dome: bool,
    eye_dome_strength: f32,
    show_scan_poses: bool,
    budget: usize,
    filter_ground: bool,
    filter_vegetation: bool,
    filter_buildings: bool,
    filter_other: bool,
    class_visibility: ClassVisibility,
    section: Option<Bounds>,
    section_reference: Option<Bounds>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    box_select: bool,
    pick_mode: bool,
    drag_rectangle: Option<([f32; 2], [f32; 2])>,
    context_menu: Option<[f32; 2]>,
    viewport_size: Size,
}

struct ScanMarker {
    x: f32,
    y: f32,
    labels: Vec<String>,
    axes: Option<[[f64; 3]; 3]>,
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
            dx * dx + dy * dy <= 12.0 * 12.0
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
    Pan,
    Select,
    RightPending,
    Section(usize, bool),
}

#[derive(Debug, Clone, Copy)]
struct DragState {
    start: UiPoint,
    position: UiPoint,
    mode: DragMode,
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

fn context_menu_bounds(at: [f32; 2], viewport: Rectangle) -> Rectangle {
    Rectangle::new(
        UiPoint::new(
            at[0].min((viewport.width - 184.0).max(0.0)).max(0.0),
            at[1].min((viewport.height - 190.0).max(0.0)).max(0.0),
        ),
        Size::new(180.0, 186.0),
    )
}

fn context_action_at(point: UiPoint, at: [f32; 2], viewport: Rectangle) -> Option<ContextAction> {
    let menu = context_menu_bounds(at, viewport);
    if !menu.contains(point) || point.y < menu.y + 22.0 {
        return None;
    }
    let index = ((point.y - menu.y - 22.0) / 25.0) as usize;
    CONTEXT_ACTIONS.get(index).map(|(action, _)| *action)
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
        position: UiPoint::new(menu.x + 10.0, menu.y + 15.0),
        size: iced::Pixels(10.0),
        color: Color::from_rgb8(161, 161, 170),
        ..canvas::Text::default()
    });
    let hovered_action = hovered.and_then(|point| context_action_at(point, at, viewport));
    for (index, (action, label)) in CONTEXT_ACTIONS.iter().enumerate() {
        let y = menu.y + 22.0 + index as f32 * 25.0;
        if hovered_action == Some(*action) {
            frame.fill_rectangle(
                UiPoint::new(menu.x + 3.0, y),
                Size::new(menu.width - 6.0, 25.0),
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
            position: UiPoint::new(menu.x + 12.0, y + 17.0),
            size: iced::Pixels(12.0),
            color: Color::from_rgb8(241, 241, 240),
            ..canvas::Text::default()
        });
    }
}

fn section_handle_world(section: Bounds, axis: usize, is_min: bool) -> [f64; 3] {
    let mut point = section.center();
    point[axis] = if is_min {
        section.min[axis]
    } else {
        section.max[axis]
    };
    point
}

fn section_handle_at(
    pointer: UiPoint,
    section: Bounds,
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

fn section_handle_delta(
    axis: usize,
    is_min: bool,
    section: Bounds,
    overall: Bounds,
    projection: Projection,
    movement: [f32; 2],
) -> Option<f32> {
    let world = section_handle_world(section, axis, is_min);
    let mut shifted = world;
    shifted[axis] += (overall.max[axis] - overall.min[axis]) * 0.01;
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
) -> Option<(usize, usize)> {
    let mut nearest: Option<(usize, usize, f32)> = None;
    for (cloud_index, entry) in clouds.iter().enumerate().filter(|(_, entry)| entry.visible) {
        for (pose_index, pose) in entry.cloud.scan_poses.iter().enumerate() {
            let Some((x, y, _)) = projection.project(entry.transform.xyz(pose.position)) else {
                continue;
            };
            let distance = (pointer.x - x).hypot(pointer.y - y);
            if distance <= 12.0
                && nearest.is_none_or(|(_, _, previous_distance)| distance < previous_distance)
            {
                nearest = Some((cloud_index, pose_index, distance));
            }
        }
    }
    nearest.map(|(cloud_index, pose_index, _)| (cloud_index, pose_index))
}

impl canvas::Program<Message> for PointViewport<'_> {
    type State = Option<DragState>;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
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
                if self.show_scan_poses && !self.box_select && !self.pick_mode {
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
                            scan_pose_at(self.clouds, projection, position)
                        {
                            *state = None;
                            return (
                                event::Status::Captured,
                                Some(Message::CenterScanPose(cloud_index, pose_index)),
                            );
                        }
                    }
                }
                *state = cursor.position_in(bounds).map(|position| DragState {
                    start: position,
                    position,
                    mode: if self.box_select || self.pick_mode {
                        DragMode::Select
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
                    mode: DragMode::Pan,
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
                    finish_viewport_drag(button, drag, position, bounds.size())
                });
                (event::Status::Captured, message)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(previous) = state.as_mut() {
                    let Some(position) = cursor.position_in(bounds) else {
                        return (event::Status::Captured, None);
                    };
                    let dx = position.x - previous.position.x;
                    let dy = position.y - previous.position.y;
                    previous.position = position;
                    if matches!(previous.mode, DragMode::RightPending) {
                        if (position.x - previous.start.x).hypot(position.y - previous.start.y)
                            < 5.0
                        {
                            return (event::Status::Captured, None);
                        }
                        previous.mode = DragMode::Pan;
                    }
                    (
                        event::Status::Captured,
                        Some(match previous.mode {
                            DragMode::Orbit => Message::Orbit(dx, dy),
                            DragMode::Pan => Message::Pan(dx, dy),
                            DragMode::RightPending => unreachable!(),
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
        let Some(overall_bounds) = combined_bounds(self.clouds) else {
            frame.fill_text(canvas::Text {
                content: "Open a point cloud to begin".into(),
                position: UiPoint::new(bounds.width * 0.5 - 120.0, bounds.height * 0.5),
                color: Color::WHITE,
                ..canvas::Text::default()
            });
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
            let vertices = [
                [section.min[0], section.min[1], section.min[2]],
                [section.max[0], section.min[1], section.min[2]],
                [section.max[0], section.max[1], section.min[2]],
                [section.min[0], section.max[1], section.min[2]],
                [section.min[0], section.min[1], section.max[2]],
                [section.max[0], section.min[1], section.max[2]],
                [section.max[0], section.max[1], section.max[2]],
                [section.min[0], section.max[1], section.max[2]],
            ];
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
            let show_labels = pose_count <= 24;
            let mut markers = Vec::with_capacity(pose_count);
            for entry in self.clouds.iter().filter(|entry| entry.visible) {
                for pose in &entry.cloud.scan_poses {
                    let Some((x, y, _)) = projection.project(entry.transform.xyz(pose.position))
                    else {
                        continue;
                    };
                    push_scan_marker(
                        &mut markers,
                        x,
                        y,
                        &pose.label,
                        entry.transform.axes(pose.axes),
                        show_labels,
                    );
                }
            }
            for marker in markers {
                let center = UiPoint::new(marker.x, marker.y);
                if show_labels {
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
                if show_labels {
                    frame.fill_text(canvas::Text {
                        content: if marker.labels.len() == 1 {
                            marker.labels.into_iter().next().unwrap_or_default()
                        } else {
                            format!("{} stations", marker.labels.len())
                        },
                        position: UiPoint::new(marker.x + 12.0, marker.y + 4.0),
                        size: iced::Pixels(10.0),
                        color: Color::from_rgb8(245, 188, 100),
                        ..canvas::Text::default()
                    });
                }
            }
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
                || (self.show_scan_poses && !self.box_select && !self.pick_mode)
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
                            )
                            .is_some()
                        })
                    })
            {
                mouse::Interaction::Pointer
            } else if self.box_select {
                mouse::Interaction::Crosshair
            } else {
                mouse::Interaction::Grab
            }
        } else {
            mouse::Interaction::default()
        }
    }
}

impl PointViewport<'_> {
    fn accepts(&self, point: &Point) -> bool {
        if let Some(section) = self.section {
            if (0..3).any(|axis| {
                point.xyz[axis] < section.min[axis] || point.xyz[axis] > section.max[axis]
            }) {
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
            ColorMode::Rgb => point.rgb.unwrap_or([210, 218, 225]),
            ColorMode::Elevation => {
                let range = (bounds.max[2] - bounds.min[2]).max(0.001);
                let t = ((point.xyz[2] - bounds.min[2]) / range).clamp(0.0, 1.0);
                [
                    (30.0 + 225.0 * t) as u8,
                    (80.0 + 150.0 * (1.0 - (2.0 * t - 1.0).abs())) as u8,
                    (230.0 * (1.0 - t)) as u8,
                ]
            }
            ColorMode::Intensity => {
                let value = (point.intensity.unwrap_or(0) / 257) as u8;
                [value; 3]
            }
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
            section: Some(Bounds {
                min: [11.0, 1.0, 2.0],
                max: [13.0, 3.0, 4.0],
            }),
        };
        assert!(mesh_accepts(0, &point, None, filter, transform));
        filter.section = Some(Bounds {
            min: [0.0, 1.0, 2.0],
            max: [2.0, 3.0, 4.0],
        });
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
        let cached = selected_source_bounds(&[source(deleted.clone())]).unwrap();
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
        let remaining = selected_source_bounds(&[source(deleted.clone())]).unwrap();
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
        assert!(selected_source_bounds(&[source(deleted)]).is_err());
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
        let bounds = selected_source_bounds(&[SelectedSource {
            index: 0,
            cloud: Arc::clone(&studio.clouds[0].cloud),
            selection: Arc::clone(&selection),
            deleted: None,
            transform: studio.clouds[0].transform,
        }])
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
        let selected = selected_source_bounds(&[SelectedSource {
            index: 0,
            cloud: Arc::clone(&cloud),
            selection: Arc::clone(&selection),
            deleted: None,
            transform: CloudTransform::default(),
        }])
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
            let bounds = selected_source_bounds(&[SelectedSource {
                index: 0,
                cloud: Arc::clone(&entry.cloud),
                selection: Arc::clone(&selection),
                deleted: None,
                transform: entry.transform,
            }])
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
            let moved_bounds = selected_source_bounds(&[SelectedSource {
                index: 0,
                cloud: Arc::clone(&cloud),
                selection: Arc::clone(&selected[0].1),
                deleted: None,
                transform: moved,
            }])
            .unwrap();
            assert!((moved_bounds.0.min[0] - (expected_x_max + 5.0)).abs() < 1e-10);
            assert_eq!(moved_bounds.0.min[1..], [205.0, 5.0]);
            assert_eq!(moved_bounds.0.max[1..], [215.0, 15.0]);
        }
        let selected_bounds = selected_source_bounds(&[SelectedSource {
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
        }])
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
                select_bounds,
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
        assert!(source_lod_coverage(projection, scene, Some(outside)).is_none());
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
mod camera_api_tests {
    use super::*;

    fn send(studio: &mut Studio, command: native_api::ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(native_api::ApiRequest { command, reply });
        receive.recv().unwrap()
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
            },
            native_api::ApiCommand::SetCamera {
                yaw: 0.4,
                pitch: -0.2,
                zoom: 1.0,
                pan: [f32::NAN, 0.0],
            },
        ] {
            assert_eq!(send(&mut studio, command)["ok"], false);
            assert_eq!(studio.zoom, 0.01);
            assert_eq!(studio.pan, [120.0, -80.0]);
        }

        let fitted = send(&mut studio, native_api::ApiCommand::ZoomAll);
        assert_eq!(fitted["ok"], true);
        assert_eq!(studio.zoom, 1.0);
        assert_eq!(studio.pan, [0.0, 0.0]);
        assert_eq!(studio.view_label, "ISOMETRIC");
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
        studio.saved_views.push(SavedView {
            source: camera_views::source_key(&first_path),
            name: "First entrance".into(),
            yaw: 0.5,
            pitch: 0.25,
            zoom: 2.0,
            pan: [12.0, -8.0],
        });
        studio.saved_views.push(SavedView {
            source: camera_views::source_key(&second_path),
            name: "Second entrance".into(),
            yaw: -0.5,
            pitch: 0.1,
            zoom: 3.0,
            pan: [4.0, 5.0],
        });

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
