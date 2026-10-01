use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod bag_map;
mod camera_views;
mod gpu_viewport;
mod opencad_properties;
mod opencad_ribbon;
mod selection;
mod ui_theme;
mod view_cube;

use bag_map::{BagMap, MapView, TileKey};
use camera_views::SavedView;
use iced::mouse;
use iced::widget::canvas::{self, event, Canvas, Frame, Geometry};
use iced::widget::{
    button, checkbox, column, container, image, pick_list, row, scrollable, slider, stack, svg,
    text, text_input,
};
use iced::{Color, Element, Fill, Font, Point as UiPoint, Rectangle, Renderer, Size, Task, Theme};
use pointcloud_core::{
    BagBounds, BagLod, Bounds, ExportFormat, IndexConfig, IndexedPoint, MeshGeometry, OctreeIndex,
    Point, PointCloud,
};
use selection::{
    pick_full, pick_indexed, select_full, ClassFilter, DeletionMask, Projection, ScreenRect,
    SelectionMask, SelectionSource,
};
use ui_theme::UiTheme;

const LOAD_SAMPLE_LIMIT: usize = 100_000;
const AUTO_INDEX_MIN_POINTS: u64 = 1_000_000;
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

fn display_name(path: &std::path::Path) -> &str {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Point cloud");
    name.strip_prefix("open-pointcloud-").unwrap_or(name)
}

fn compact_filename(name: &str, max_chars: usize) -> String {
    let length = name.chars().count();
    if length <= max_chars {
        return name.to_owned();
    }
    name.chars().skip(length - max_chars).collect()
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
            eprintln!("Usage: open-pointcloud-studio-native --index INPUT");
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
            eprintln!("Usage: open-pointcloud-studio-native --scans INPUT");
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
                }
                return Ok(());
            }
            Err(error) => {
                eprintln!("Scan positions failed: {error}");
                std::process::exit(1);
            }
        }
    }
    if first.as_deref() == Some(OsStr::new("--export")) {
        let (Some(source), Some(destination), None) = (args.next(), args.next(), args.next())
        else {
            eprintln!("Usage: open-pointcloud-studio-native --export INPUT OUTPUT");
            std::process::exit(2);
        };
        let source = PathBuf::from(source);
        let destination = PathBuf::from(destination);
        let Some(format) = export_format_for_path(&destination) else {
            eprintln!("Supported export extensions: .ply, .xyz, .pts, .csv, .las, .laz");
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
                "Usage: open-pointcloud-studio-native --section INPUT XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX OUTPUT"
            );
            std::process::exit(2);
        };
        let source = PathBuf::from(source);
        let destination = PathBuf::from(destination);
        let Some(format) = export_format_for_path(&destination) else {
            eprintln!("Supported export extensions: .ply, .xyz, .pts, .csv, .las, .laz");
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
            eprintln!("Usage: open-pointcloud-studio-native --mesh-export INPUT OUTPUT.obj");
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
            eprintln!("Usage: open-pointcloud-studio-native --mesh INPUT OUTPUT.obj");
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
        let (Some(source), Some(destination), None) = (args.next(), args.next(), args.next())
        else {
            eprintln!("Usage: open-pointcloud-studio-native --surface INPUT OUTPUT.obj");
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
            pointcloud_core::mesh_surface_obj(
                &cloud,
                &destination,
                pointcloud_core::SurfaceMeshConfig::default(),
            )
        }) {
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
                "Usage: open-pointcloud-studio-native --bag3d XMIN,YMIN,XMAX,YMAX 1.2|1.3|2.2 OUTPUT.obj"
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
    let startup_files: Vec<PathBuf> = first.into_iter().chain(args).map(PathBuf::from).collect();
    iced::application("Open Pointcloud Studio", Studio::update, Studio::view)
        .subscription(|_| {
            iced::event::listen_with(|event, status, _| match event {
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
            })
        })
        .font(include_bytes!("../../assets/fonts/Inter.ttf").as_slice())
        .font(include_bytes!("../../assets/fonts/SpaceGrotesk.ttf").as_slice())
        .default_font(Font::with_name("Inter"))
        .theme(|studio: &Studio| studio.ui_theme.iced())
        .antialiasing(true)
        .window_size((1440.0, 900.0))
        .run_with(move || {
            let mut studio = Studio::default();
            let task = Task::batch(startup_files.into_iter().map(|path| studio.load(path)));
            (studio, task)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
enum MeshMode {
    Terrain,
    Surface,
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
    Tab(RibbonTab),
    Theme(UiTheme),
    Open,
    FilesChosen(Option<Vec<PathBuf>>),
    Loaded(Result<Arc<PointCloud>, String>),
    MeshLoaded(Arc<PointCloud>, Result<Option<Arc<MeshGeometry>>, String>),
    Refined(PathBuf, Result<Arc<PointCloud>, String>),
    Export,
    ExportSection,
    SectionExportPathChosen(
        Arc<PointCloud>,
        Bounds,
        ExportFormat,
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
    MeshRequest(MeshMode),
    MeshPathChosen(
        MeshMode,
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
    ExportMesh,
    MeshExportPathChosen(Arc<MeshGeometry>, PathBuf, bool, Option<PathBuf>),
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
    BuildIndex,
    IndexReady(Arc<PointCloud>, Result<Arc<OctreeIndex>, String>),
    AutoIndexReady(Arc<PointCloud>, Result<Arc<OctreeIndex>, String>),
    SetAutoIndex(bool),
    CachedIndexReady(Arc<PointCloud>, Result<Option<Arc<OctreeIndex>>, String>),
    LoadDetail,
    RefreshDetail(u64),
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
    ShowScanPoses(bool),
    ExpandScanPoses(bool),
    FitScanPoses,
    CenterScanPose(usize, usize),
    Budget(u32),
    FilterGround(bool),
    FilterVegetation(bool),
    FilterBuildings(bool),
    FilterOther(bool),
    SetSectionEnabled(bool),
    SectionMin(usize, f32),
    SectionMax(usize, f32),
    SectionHandleDelta(usize, bool, f32),
    SectionCoordinate(usize, bool, String),
    ApplySectionCoordinates,
    ResetSectionBox,
    ZoomToSection,
    FitSectionToSelection,
    SectionFitReady(
        u64,
        Vec<(usize, Arc<SelectionMask>)>,
        Result<(Bounds, u64), String>,
    ),
    Orbit(f32, f32),
    Pan(f32, f32),
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
    clouds: Vec<CloudEntry>,
    undo_deletions: Vec<EditBatch>,
    redo_deletions: Vec<EditBatch>,
    active: Option<usize>,
    status: String,
    export_format: ExportFormat,
    decimation_stride: u64,
    thin_percent: u8,
    translate_x: String,
    translate_y: String,
    translate_z: String,
    scale_inputs: [String; 3],
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
    show_scan_poses: bool,
    expand_scan_poses: bool,
    budget: u32,
    filter_ground: bool,
    filter_vegetation: bool,
    filter_buildings: bool,
    filter_other: bool,
    section_enabled: bool,
    section_export_pending: bool,
    mesh_export_pending: bool,
    section_fit_pending: bool,
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
    ui_theme: UiTheme,
    box_select: bool,
    pick_mode: bool,
    drag_rectangle: Option<([f32; 2], [f32; 2])>,
    context_menu: Option<[f32; 2]>,
    selection_pending: bool,
    pending_delete: bool,
    index_pending: bool,
    detail_pending: bool,
    auto_index: bool,
    revision: u64,
}

struct CloudEntry {
    cloud: Arc<PointCloud>,
    mesh: Option<Arc<MeshGeometry>>,
    mesh_visible: bool,
    bag_source: bool,
    visible: bool,
    selection: Option<Arc<SelectionMask>>,
    deleted: Option<Arc<DeletionMask>>,
    index: Option<Arc<OctreeIndex>>,
    auto_index_queued: bool,
    index_building: bool,
    detail_points: Option<Vec<IndexedPoint>>,
}

struct EditBatch {
    members: Vec<(Arc<PointCloud>, Arc<SelectionMask>)>,
}

impl CloudEntry {
    fn view_len(&self) -> usize {
        self.detail_points
            .as_ref()
            .map_or(self.cloud.points.len(), Vec::len)
    }

    fn view_records(&self) -> Box<dyn Iterator<Item = IndexedPoint> + '_> {
        if let Some(detail) = &self.detail_points {
            Box::new(detail.iter().copied())
        } else {
            Box::new(
                self.cloud
                    .points
                    .iter()
                    .copied()
                    .zip(self.cloud.point_ordinals.iter().copied())
                    .map(|(point, ordinal)| IndexedPoint { point, ordinal }),
            )
        }
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
        Self {
            clouds: Vec::new(),
            undo_deletions: Vec::new(),
            redo_deletions: Vec::new(),
            active: None,
            status: "Open a LAS, LAZ, PLY, PCD, PTX, OBJ, OFF or STL file".into(),
            export_format: ExportFormat::PlyBinary,
            decimation_stride: 10,
            thin_percent: 50,
            translate_x: "0".into(),
            translate_y: "0".into(),
            translate_z: "0".into(),
            scale_inputs: std::array::from_fn(|_| "1".into()),
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
            color_mode: ColorMode::Rgb,
            point_size: 2.0,
            eye_dome: true,
            show_scan_poses: true,
            expand_scan_poses: false,
            budget: 80_000,
            filter_ground: true,
            filter_vegetation: true,
            filter_buildings: true,
            filter_other: true,
            section_enabled: false,
            section_export_pending: false,
            mesh_export_pending: false,
            section_fit_pending: false,
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
            ui_theme: UiTheme::load(),
            box_select: false,
            pick_mode: false,
            drag_rectangle: None,
            context_menu: None,
            selection_pending: false,
            pending_delete: false,
            index_pending: false,
            detail_pending: false,
            auto_index: true,
            revision: 0,
        }
    }
}

impl Studio {
    fn load(&mut self, path: PathBuf) -> Task<Message> {
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
                    let identity = path.clone();
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
                        move |result| Message::Refined(identity.clone(), result),
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
            Message::Tab(tab) => self.ribbon_tab = tab,
            Message::Theme(theme) => {
                self.ui_theme = theme;
                theme.save();
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
                        cloud.total_points,
                        cloud.points.len()
                    );
                    self.clouds.push(CloudEntry {
                        bag_source: is_bag3d_obj(&cloud.path),
                        cloud,
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
            Message::Refined(path, result) => {
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.cloud.path == path)
                {
                    match result {
                        Ok(cloud) => {
                            let count = cloud.total_points;
                            let indexed = entry.index.is_some();
                            entry.cloud = Arc::clone(&cloud);
                            self.status =
                                format!("Ready: {} points from {}", count, path.display());
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
                    return save_task(suggested, format, move |path| {
                        let result = if let Some(mask) = deleted {
                            pointcloud_core::export_where(
                                &cloud,
                                &path,
                                format,
                                cloud.total_points - mask.count,
                                |ordinal, _| !mask.contains(ordinal),
                            )
                        } else {
                            pointcloud_core::export_full(&cloud, &path, format)
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
                                deleted.as_ref().map(Arc::clone),
                                path,
                            )
                        },
                    );
                }
            }
            Message::SectionExportPathChosen(cloud, section, format, deleted, Some(path)) => {
                self.section_export_pending = true;
                self.status = format!(
                    "Exporting section from {} source points…",
                    cloud.total_points
                );
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            pointcloud_core::export_section_where(
                                &cloud,
                                &path,
                                format,
                                section,
                                |ordinal, _| {
                                    deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
                                },
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
            Message::SectionExportPathChosen(_, _, _, _, None) => {
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
            Message::MeshRequest(mode) => {
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
                                Arc::clone(&cloud),
                                deleted.as_ref().map(Arc::clone),
                                path,
                            )
                        },
                    );
                }
            }
            Message::MeshPathChosen(mode, cloud, deleted, Some(path)) => {
                let remaining = cloud.total_points - deleted.as_ref().map_or(0, |mask| mask.count);
                self.status = format!(
                    "Meshing {remaining} remaining points; the viewport remains responsive…"
                );
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let source = Arc::clone(&cloud);
                            match mode {
                                MeshMode::Terrain => pointcloud_core::mesh_terrain_obj_where(
                                    &cloud,
                                    &path,
                                    pointcloud_core::MeshConfig::default(),
                                    |ordinal, _| {
                                        deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
                                    },
                                ),
                                MeshMode::Surface => pointcloud_core::mesh_surface_obj_where(
                                    &cloud,
                                    &path,
                                    pointcloud_core::SurfaceMeshConfig::default(),
                                    |ordinal, _| {
                                        deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
                                    },
                                ),
                            }
                            .and_then(|stats| {
                                pointcloud_core::read_obj_mesh(&path)
                                    .map(|mesh| (source, path, stats, Arc::new(mesh)))
                            })
                            .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result)
                    },
                    move |result| Message::MeshReady(mode, result),
                );
            }
            Message::MeshPathChosen(_, _, _, None) => {
                self.status = "Mesh save cancelled".into();
            }
            Message::MeshReady(mode, result) => match result {
                Ok((source, path, stats, mesh)) => {
                    if let Some(entry) = self
                        .clouds
                        .iter_mut()
                        .find(|entry| entry.cloud.same_source_revision(&source))
                    {
                        entry.mesh = Some(mesh);
                        self.status = format!(
                            "{} mesh displayed: {} vertices, {} triangles from {} points → {}",
                            match mode {
                                MeshMode::Terrain => "Terrain",
                                MeshMode::Surface => "3D surface",
                            },
                            stats.vertices,
                            stats.triangles,
                            stats.source_points,
                            path.display()
                        );
                    }
                }
                Err(error) => self.status = format!("Meshing failed: {error}"),
            },
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
                            path,
                        )
                    },
                );
            }
            Message::MeshExportPathChosen(mesh, source, bag_source, Some(path)) => {
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
                            pointcloud_core::write_obj_mesh(&mesh, &path, comments)
                                .map(|()| (path, mesh.vertices.len(), mesh.triangles.len()))
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    Message::MeshExported,
                );
            }
            Message::MeshExportPathChosen(_, _, _, None) => {
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
                        .map(|entry| entry.cloud.bounds)
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
                        self.status =
                            format!("Choose where to export {} selected points…", mask.count);
                        return save_task(suggestion, format, move |path| {
                            pointcloud_core::export_where(
                                &cloud,
                                &path,
                                format,
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
                        let expected = entry.remaining_count().saturating_sub(mask.count);
                        self.status =
                            format!("Choose output file without {} selected points…", mask.count);
                        return save_task(suggestion, format, move |path| {
                            pointcloud_core::export_where(
                                &cloud,
                                &path,
                                format,
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
                self.status =
                    format!("Deleted {removed} points in the open view; Undo restores them");
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
                        .find(|entry| entry.cloud.same_source_revision(source))
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
                self.status = format!("Restored {restored} points");
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
                        .find(|entry| entry.cloud.same_source_revision(source))
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
                self.status = format!("Deleted {removed} points again");
                return self.schedule_detail();
            }
            Message::DecimationStride(stride) => self.decimation_stride = stride,
            Message::ThinPercent(percent) => self.thin_percent = percent,
            Message::Thin => {
                if let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) {
                    let format = self.export_format;
                    let percent = self.thin_percent;
                    let remaining = entry.remaining_count();
                    let stem = entry
                        .cloud
                        .path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("pointcloud");
                    let suggestion = format!("{stem}-thin-{percent}pct.{}", format.extension());
                    let cloud = Arc::clone(&entry.cloud);
                    let deleted = entry.deleted.as_ref().map(Arc::clone);
                    self.status =
                        format!("Choose output for keeping {percent}% of {remaining} points…");
                    return save_task(suggestion, format, move |path| {
                        pointcloud_core::export_thin_percent_where(
                            &cloud,
                            &path,
                            format,
                            remaining,
                            percent,
                            |ordinal, _| {
                                deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
                            },
                        )
                        .map(|_| path)
                        .map_err(|error| error.to_string())
                    });
                }
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
                    let expected = entry.remaining_count().div_ceil(stride);
                    self.status = format!("Choose output for one point in every {stride}…");
                    return save_task(suggestion, format, move |path| {
                        let mut kept_ordinal = 0u64;
                        pointcloud_core::export_where(
                            &cloud,
                            &path,
                            format,
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
                    return self.save_transform([x, y, z], [1.0; 3], "translated");
                }
                self.status = "Enter valid X, Y and Z offsets".into();
            }
            Message::ApplyScale => {
                let parsed = std::array::from_fn(|axis| self.scale_inputs[axis].parse::<f64>());
                if let [Ok(x), Ok(y), Ok(z)] = parsed {
                    let scale = [x, y, z];
                    if scale.iter().all(|value| value.is_finite()) {
                        return self.save_transform([0.0; 3], scale, "scaled");
                    }
                }
                self.status = "Enter finite X, Y and Z scale factors".into();
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
                let cloud = Arc::clone(&entry.cloud);
                let source = Arc::clone(&cloud);
                self.index_pending = true;
                self.status = format!("Building disk octree for {} points…", cloud.total_points);
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            OctreeIndex::build_cached(&cloud, IndexConfig::default())
                                .map(Arc::new)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    move |result| Message::IndexReady(Arc::clone(&source), result),
                );
            }
            Message::IndexReady(source, result) | Message::AutoIndexReady(source, result) => {
                self.index_pending = false;
                let mut ready = false;
                if let Some(entry) = self
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.cloud.same_source_revision(&source))
                {
                    entry.index_building = false;
                    match result {
                        Ok(index) => {
                            entry.index = Some(index);
                            self.status = format!("Octree ready for {}", source.path.display());
                            ready = true;
                        }
                        Err(error) => self.status = format!("Octree failed: {error}"),
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
                    .find(|entry| entry.cloud.same_source_revision(&source))
                {
                    match result {
                        Ok(Some(index)) => {
                            entry.index = Some(index);
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
                    return Task::batch(tasks);
                }
                for entry in &mut self.clouds {
                    entry.auto_index_queued = false;
                }
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
                let sources: Vec<_> = self
                    .clouds
                    .iter()
                    .enumerate()
                    .filter_map(|(index, entry)| {
                        (entry.visible)
                            .then(|| entry.index.as_ref().map(|tree| (index, Arc::clone(tree))))
                            .flatten()
                    })
                    .collect();
                if sources.is_empty() {
                    self.status = "Build an octree for a visible cloud first".into();
                    return Task::none();
                }
                let projection = Projection::new(
                    bounds,
                    self.yaw,
                    self.pitch,
                    self.zoom,
                    self.pan,
                    self.viewport_size.width,
                    self.viewport_size.height,
                );
                let limit = (self.budget as usize / sources.len()).max(1);
                let revision = self.revision;
                self.detail_pending = true;
                if !self.section_export_pending {
                    self.status = format!(
                        "Refining visible octree nodes in {} cloud(s)…",
                        sources.len()
                    );
                }
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let workers: Vec<_> = sources
                                .into_iter()
                                .map(|(index, tree)| {
                                    std::thread::spawn(move || {
                                        tree.sample_lod_indexed(limit, |node_bounds| {
                                            if section.is_some_and(|clip| {
                                                (0..3).any(|axis| {
                                                    node_bounds.max[axis] < clip.min[axis]
                                                        || node_bounds.min[axis] > clip.max[axis]
                                                })
                                            }) {
                                                return None;
                                            }
                                            projection.screen_span(node_bounds)
                                        })
                                        .map(|points| (index, points))
                                        .map_err(|error| error.to_string())
                                    })
                                })
                                .collect();
                            workers
                                .into_iter()
                                .map(|worker| {
                                    worker
                                        .join()
                                        .map_err(|_| "detail worker panicked".to_string())?
                                })
                                .collect::<Result<Vec<_>, String>>()
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    move |result| Message::DetailReady(revision, result),
                );
            }
            Message::RefreshDetail(revision) => {
                if revision == self.revision && !self.detail_pending {
                    return self.update(Message::LoadDetail);
                }
            }
            Message::DetailReady(revision, result) => {
                self.detail_pending = false;
                if revision != self.revision {
                    return self.schedule_detail();
                }
                match result {
                    Ok(details) => {
                        let mut count = 0usize;
                        for (index, points) in details {
                            count += points.len();
                            if let Some(entry) = self.clouds.get_mut(index) {
                                entry.detail_points = Some(points);
                            }
                        }
                        if !self.section_export_pending {
                            self.status =
                                format!("Viewport LOD ready: {count} points from disk octree");
                        }
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
                if let Some(entry) = self.clouds.get_mut(index) {
                    entry.visible = visible;
                    self.revision += 1;
                    self.clear_detail();
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
                    self.clouds.remove(index);
                    self.undo_deletions.clear();
                    self.redo_deletions.clear();
                    self.pending_delete = false;
                    self.revision += 1;
                    self.clear_detail();
                    self.active = if self.clouds.is_empty() {
                        None
                    } else {
                        Some(index.min(self.clouds.len() - 1))
                    };
                    return self.schedule_detail();
                }
            }
            Message::ColorMode(mode) => self.color_mode = mode,
            Message::PointSize(size) => self.point_size = size,
            Message::SetEyeDome(enabled) => self.eye_dome = enabled,
            Message::ShowScanPoses(enabled) => self.show_scan_poses = enabled,
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
                self.clear_detail();
                self.status = "Point cloud and scanner positions framed".into();
                return self.schedule_detail();
            }
            Message::CenterScanPose(cloud_index, pose_index) => {
                let (Some(scene), Some(pose)) = (
                    combined_bounds(&self.clouds),
                    self.clouds
                        .get(cloud_index)
                        .filter(|entry| entry.visible)
                        .and_then(|entry| entry.cloud.scan_poses.get(pose_index)),
                ) else {
                    return Task::none();
                };
                let Some(pan) = pan_to_world(
                    scene,
                    pose.position,
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
                self.clear_detail();
                return self.schedule_detail();
            }
            Message::Budget(budget) => {
                self.budget = budget;
                self.revision += 1;
                self.clear_detail();
                return self.schedule_detail();
            }
            Message::FilterGround(value) => self.filter_ground = value,
            Message::FilterVegetation(value) => self.filter_vegetation = value,
            Message::FilterBuildings(value) => self.filter_buildings = value,
            Message::FilterOther(value) => self.filter_other = value,
            Message::SetSectionEnabled(enabled) => {
                if enabled && self.section_reference_bounds.is_none() {
                    self.section_reference_bounds = combined_bounds(&self.clouds);
                }
                self.section_enabled = enabled;
                if enabled {
                    self.sync_section_coordinate_inputs();
                }
                self.revision += 1;
                self.clear_detail();
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
                    self.clear_detail();
                    return self.schedule_detail();
                }
            }
            Message::SectionMax(axis, value) => {
                if axis < 3 {
                    let lower = (self.section_min_percent[axis] + 0.000_001).min(100.0);
                    self.section_max_percent[axis] = f64::from(value).clamp(lower, 100.0);
                    self.sync_section_coordinate_inputs();
                    self.revision += 1;
                    self.clear_detail();
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
                    self.clear_detail();
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
                for (axis, pair) in limits.iter().enumerate() {
                    let span = overall.max[axis] - overall.min[axis];
                    if (span > 0.0 && pair[0] >= pair[1])
                        || (span == 0.0 && pair[0] != pair[1])
                        || pair[0] < overall.min[axis] - 0.000_001
                        || pair[1] > overall.max[axis] + 0.000_001
                    {
                        self.status = format!(
                            "{} section limits must be ordered and inside the model bounds",
                            ["X", "Y", "Z"][axis]
                        );
                        return Task::none();
                    }
                }
                for (axis, pair) in limits.iter().enumerate() {
                    let span = overall.max[axis] - overall.min[axis];
                    if span > 0.0 {
                        self.section_min_percent[axis] =
                            ((pair[0] - overall.min[axis]) / span * 100.0).clamp(0.0, 100.0);
                        self.section_max_percent[axis] =
                            ((pair[1] - overall.min[axis]) / span * 100.0).clamp(0.0, 100.0);
                    }
                }
                self.section_reference_bounds = Some(overall);
                self.section_enabled = true;
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                self.clear_detail();
                self.status = "Section box updated from XYZ coordinates".into();
                return self.schedule_detail();
            }
            Message::ResetSectionBox => {
                self.section_reference_bounds = combined_bounds(&self.clouds);
                self.section_min_percent = [0.0; 3];
                self.section_max_percent = [100.0; 3];
                self.sync_section_coordinate_inputs();
                self.revision += 1;
                self.clear_detail();
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
                self.clear_detail();
                self.status = "Section box framed in the viewport".into();
                return self.schedule_detail();
            }
            Message::FitSectionToSelection => {
                if self.section_fit_pending {
                    return Task::none();
                }
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
                            })
                    })
                    .collect();
                if sources.is_empty() {
                    self.status = "Select points before fitting the section box".into();
                    return Task::none();
                }
                let snapshots: Vec<(usize, Arc<SelectionMask>)> = sources
                    .iter()
                    .map(|source| (source.index, Arc::clone(&source.selection)))
                    .collect();
                let revision = self.revision;
                self.section_fit_pending = true;
                self.status = "Finding exact bounds of selected source points…".into();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || selected_source_bounds(&sources))
                            .await
                            .map_err(|error| error.to_string())?
                    },
                    move |result| Message::SectionFitReady(revision, snapshots.clone(), result),
                );
            }
            Message::SectionFitReady(revision, snapshots, result) => {
                self.section_fit_pending = false;
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
                        self.status = format!("Could not fit section box: {error}");
                        return Task::none();
                    }
                };
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
                self.clear_detail();
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
                self.clear_detail();
                return self.schedule_detail();
            }
            Message::Pan(dx, dy) => {
                self.pan[0] += dx;
                self.pan[1] += dy;
                self.revision += 1;
                self.clear_detail();
                return self.schedule_detail();
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
                self.clear_detail();
                return self.schedule_detail();
            }
            Message::ViewportSize(size) => {
                if size.width > 0.0 && size.height > 0.0 {
                    self.viewport_size = size;
                    self.revision += 1;
                    self.clear_detail();
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
                self.clear_detail();
                return self.schedule_detail();
            }
            Message::CameraPreset(preset) => {
                let (yaw, pitch, label) = preset.orientation();
                self.yaw = yaw;
                self.pitch = pitch;
                self.view_label = label;
                self.revision += 1;
                self.clear_detail();
                return self.schedule_detail();
            }
            Message::CubeCorner(corner) => {
                self.yaw = f32::from(corner[1]).atan2(f32::from(corner[0]));
                self.pitch = f32::from(corner[2]).atan2(std::f32::consts::SQRT_2);
                self.view_label = "ISO CORNER";
                self.revision += 1;
                self.clear_detail();
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
                self.clear_detail();
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
                self.context_menu = None;
                self.box_select = false;
                self.pick_mode = false;
                self.bag_map_drawing = false;
                self.drag_rectangle = None;
                self.status = "Selection tool closed; orbit and right-click menu available".into();
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
                    self.selection_pending = true;
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
                                    pick_indexed(
                                        &tree,
                                        projection,
                                        end,
                                        8.0,
                                        filter,
                                        deleted.as_deref(),
                                    )?
                                } else {
                                    pick_full(
                                        &cloud,
                                        projection,
                                        end,
                                        8.0,
                                        filter,
                                        deleted.as_deref(),
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
                            select_full(sources, projection, rectangle, filter)
                        })
                        .await
                        .map_err(|error| error.to_string())?
                    },
                    move |result| Message::SelectionReady(revision, result),
                );
            }
            Message::SelectionReady(revision, result) => {
                self.selection_pending = false;
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
                        self.status = format!(
                            "{} points selected at full resolution",
                            self.selected_total()
                        );
                    }
                    Err(error) => self.status = format!("Selection failed: {error}"),
                }
            }
            Message::PickReady(revision, index, result) => {
                self.selection_pending = false;
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
                            match SelectionMask::single(entry.cloud.total_points, record) {
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

    fn clear_detail(&mut self) {
        for entry in &mut self.clouds {
            if entry.deleted_count() == 0 {
                entry.detail_points = None;
            }
        }
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
        let cloud = Arc::clone(&entry.cloud);
        let source = Arc::clone(&cloud);
        self.index_pending = true;
        self.status = format!(
            "Indexing {} points for viewport detail: {}",
            cloud.total_points,
            display_name(&cloud.path)
        );
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    OctreeIndex::build_cached(&cloud, IndexConfig::default())
                        .map(Arc::new)
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Message::AutoIndexReady(Arc::clone(&source), result),
        )
    }

    fn schedule_detail(&self) -> Task<Message> {
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

    fn save_transform(
        &mut self,
        translation: [f64; 3],
        scale: [f64; 3],
        suffix: &str,
    ) -> Task<Message> {
        let Some((cloud, deleted, remaining)) = self
            .active
            .and_then(|index| self.clouds.get(index))
            .map(|entry| {
                (
                    Arc::clone(&entry.cloud),
                    entry.deleted.as_ref().map(Arc::clone),
                    entry.remaining_count(),
                )
            })
        else {
            self.status = "Open a point cloud first".into();
            return Task::none();
        };
        let format = self.export_format;
        let stem = cloud
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("pointcloud");
        let suggestion = format!("{stem}-{suffix}.{}", format.extension());
        self.status = format!("Choose output for transforming {} points…", remaining);
        save_task(suggestion, format, move |path| {
            pointcloud_core::export_affine_axes_where(
                &cloud,
                &path,
                format,
                translation,
                scale,
                remaining,
                |ordinal, _| deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal)),
            )
            .map(|()| path)
            .map_err(|error| error.to_string())
        })
    }

    fn ribbon(&self) -> Element<'_, Message> {
        let tab = |label, value| {
            button(text(label).size(12))
                .on_press(Message::Tab(value))
                .style(move |theme, status| {
                    opencad_ribbon::tab_style(theme, self.ribbon_tab == value, status)
                })
                .padding([5, 13])
        };
        let tabs = row![
            tab("Home", RibbonTab::Home),
            tab("View", RibbonTab::View),
            tab("Select", RibbonTab::Select),
            tab("Tools", RibbonTab::Tools),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center)
        .padding([1, 8]);
        let history = row![
            button(icon_svg(ToolIcon::Undo, 16.0))
                .on_press_maybe((!self.undo_deletions.is_empty()).then_some(Message::UndoDelete))
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(27)
                .height(24)
                .padding(4),
            button(icon_svg(ToolIcon::Redo, 16.0))
                .on_press_maybe((!self.redo_deletions.is_empty()).then_some(Message::RedoDelete))
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(27)
                .height(24)
                .padding(4),
        ]
        .spacing(2);
        let tab_bar = container(
            row![tabs, iced::widget::horizontal_space(), history]
                .width(Fill)
                .align_y(iced::Alignment::Center)
                .padding([0, 8]),
        )
        .width(Fill)
        .height(29)
        .style(|theme| container::Style::default().background(ui_theme::colors(theme).tabs));

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
                            slider(1.0..=8.0, self.point_size, Message::PointSize).width(102),
                            text(format!("{:.1}", self.point_size)).size(11).width(32),
                        ]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                        row![
                            text("Budget").size(11).width(43),
                            slider(1_000..=2_000_000, self.budget, Message::Budget).width(102),
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
                ribbon_group(
                    "CAMERA VIEWS",
                    row![
                        tool_button(
                            "Top",
                            Message::CameraPreset(CameraPreset::Top),
                            self.view_label == "TOP"
                        ),
                        tool_button(
                            "Front",
                            Message::CameraPreset(CameraPreset::Front),
                            self.view_label == "FRONT"
                        ),
                        tool_button(
                            "Right",
                            Message::CameraPreset(CameraPreset::Right),
                            self.view_label == "RIGHT"
                        ),
                        tool_button(
                            "Isometric",
                            Message::CameraPreset(CameraPreset::Isometric),
                            self.view_label == "ISOMETRIC"
                        ),
                        tool_button_when(
                            "Save view",
                            Message::SaveView,
                            false,
                            self.active.is_some(),
                        ),
                    ]
                    .spacing(2)
                    .into()
                ),
                ribbon_group(
                    "SCANNERS",
                    row![
                        tool_button(
                            "Stations",
                            Message::ShowScanPoses(!self.show_scan_poses),
                            self.show_scan_poses,
                        ),
                        tool_button_when(
                            "Fit stations",
                            Message::FitScanPoses,
                            false,
                            self.clouds.iter().any(|entry| {
                                entry.visible && !entry.cloud.scan_poses.is_empty()
                            }),
                        ),
                    ]
                    .spacing(2)
                    .into()
                ),
                ribbon_group(
                    "POINT DISPLAY",
                    column![
                        text(format!("Point size  {:.1}", self.point_size)).size(12),
                        slider(1.0..=8.0, self.point_size, Message::PointSize).width(180),
                    ]
                    .spacing(5)
                    .into()
                ),
                ribbon_group(
                    "DEPTH",
                    tool_button(
                        "Eye-dome",
                        Message::SetEyeDome(!self.eye_dome),
                        self.eye_dome,
                    )
                ),
                ribbon_group(
                    "SECTION BOX",
                    row![
                        tool_button(
                            "Section box",
                            Message::SetSectionEnabled(!self.section_enabled),
                            self.section_enabled,
                        ),
                        tool_button("Reset box", Message::ResetSectionBox, false),
                        tool_button_when(
                            "Zoom box",
                            Message::ZoomToSection,
                            false,
                            self.section_bounds().is_some(),
                        ),
                        tool_button_when(
                            "Fit selection",
                            Message::FitSectionToSelection,
                            false,
                            self.selected_total() > 0 && !self.section_fit_pending,
                        ),
                    ]
                    .spacing(2)
                    .into()
                ),
                ribbon_group(
                    "POINT BUDGET",
                    column![
                        text(format!("{} preview points", self.budget)).size(12),
                        slider(1_000..=2_000_000, self.budget, Message::Budget).width(200),
                    ]
                    .spacing(5)
                    .into()
                ),
                ribbon_group(
                    "CLASSIFICATION",
                    row![
                        checkbox("Ground", self.filter_ground)
                            .on_toggle(Message::FilterGround)
                            .style(muted_checkbox_style),
                        checkbox("Vegetation", self.filter_vegetation)
                            .on_toggle(Message::FilterVegetation)
                            .style(muted_checkbox_style),
                        checkbox("Buildings", self.filter_buildings)
                            .on_toggle(Message::FilterBuildings)
                            .style(muted_checkbox_style),
                        checkbox("Other", self.filter_other)
                            .on_toggle(Message::FilterOther)
                            .style(muted_checkbox_style),
                    ]
                    .spacing(12)
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
                    ],
                ),
                ribbon_group(
                    "RESULT",
                    column![
                        text(format!(
                            "{} point{} selected",
                            self.selected_total(),
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
                    "DETAIL LOD",
                    row![
                        ribbon_button_when(
                            "Build index",
                            Message::BuildIndex,
                            self.active.is_some()
                        ),
                        ribbon_button_when(
                            "Refresh LOD",
                            Message::LoadDetail,
                            self.active
                                .and_then(|index| self.clouds.get(index))
                                .is_some_and(|entry| entry.index.is_some())
                        ),
                    ]
                    .spacing(3)
                    .into()
                ),
                ribbon_group(
                    "AUTO INDEX",
                    container(
                        checkbox("Auto-index large scans", self.auto_index)
                            .on_toggle(Message::SetAutoIndex)
                            .style(muted_checkbox_style)
                            .text_size(11)
                            .size(12),
                    )
                    .height(64)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
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
                ribbon_group(
                    "SCALE",
                    row![
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
                        ribbon_button_when("Apply", Message::ApplyScale, self.active.is_some()),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
                ribbon_group(
                    "THIN",
                    row![
                        column![
                            text(format!("Keep {}%", self.thin_percent)).size(11),
                            slider(1..=100, self.thin_percent, Message::ThinPercent).width(110),
                        ]
                        .spacing(7),
                        ribbon_button_when("Apply", Message::Thin, self.active.is_some()),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
                ribbon_group(
                    "DECIMATE",
                    row![
                        pick_list(
                            [2u64, 5, 10, 20, 50, 100],
                            Some(self.decimation_stride),
                            Message::DecimationStride
                        )
                        .style(themed_pick_list_style),
                        ribbon_button_when("Keep 1 in N", Message::Decimate, self.active.is_some()),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
                ribbon_group(
                    "SURFACE",
                    row![
                        ribbon_button_when(
                            "Terrain mesh",
                            Message::MeshRequest(MeshMode::Terrain),
                            self.active.is_some()
                        ),
                        ribbon_button_when(
                            "3D surface",
                            Message::MeshRequest(MeshMode::Surface),
                            self.active.is_some()
                        ),
                        ribbon_button_when(
                            "Export mesh",
                            Message::ExportMesh,
                            self.active
                                .and_then(|index| self.clouds.get(index))
                                .is_some_and(|entry| entry.mesh.is_some())
                                && !self.mesh_export_pending,
                        ),
                    ]
                    .spacing(2)
                    .into(),
                ),
                ribbon_group(
                    "CITY DATA",
                    tool_button("3D BAG", Message::ToggleBagPanel, self.bag_panel),
                ),
                ribbon_group(
                    "EXPORT",
                    row![
                        pick_list(
                            ExportFormat::ALL,
                            Some(self.export_format),
                            Message::ExportFormat
                        )
                        .style(themed_pick_list_style),
                        ribbon_button_when("Export", Message::Export, self.active.is_some()),
                    ]
                    .spacing(8)
                    .align_y(iced::Alignment::Center)
                    .into()
                ),
            ]
            .spacing(6)
            .into(),
        };
        let group_strip = scrollable(
            container(groups)
                .padding([0, 4])
                .width(iced::Length::Shrink)
                .height(opencad_ribbon::TOOL_BAR_H),
        )
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(3).scroller_width(3),
        ))
        .width(Fill)
        .height(opencad_ribbon::TOOL_BAR_H);
        container(
            column![
                tab_bar,
                container(text(""))
                    .width(Fill)
                    .height(1)
                    .style(|theme| container::Style::default()
                        .background(ui_theme::colors(theme).accent)),
                group_strip,
            ]
            .spacing(0),
        )
        .width(Fill)
        .style(ribbon_style)
        .into()
    }

    fn project_panel(&self) -> Element<'_, Message> {
        let mut files = column![
            text("PROJECT")
                .size(14)
                .font(Font::with_name("Space Grotesk")),
            text(format!("{} point cloud(s)", self.clouds.len()))
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
            let short_name = compact_filename(name, 19);
            let file_button = button(
                text(short_name)
                    .size(12)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .on_press(Message::Select(index))
            .style(if self.active == Some(index) {
                active_tool_style
            } else {
                flat_tool_style
            })
            .width(Fill);
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
                text(format!(
                    "{} points  ·  {} selected  ·  {} deleted{}",
                    entry.remaining_count(),
                    entry
                        .selection
                        .as_ref()
                        .map_or(0, |selection| selection.count),
                    entry.deleted_count(),
                    if entry.index.is_some() {
                        "  ·  LOD ready"
                    } else if entry.index_building {
                        "  ·  Indexing"
                    } else if entry.auto_index_queued {
                        "  ·  LOD queued"
                    } else {
                        ""
                    }
                ))
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
            files = files.push(item);
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

    fn view(&self) -> Element<'_, Message> {
        let point_view = PointViewport {
            clouds: &self.clouds,
            color_mode: self.color_mode,
            point_size: self.point_size,
            eye_dome: self.eye_dome,
            show_scan_poses: self.show_scan_poses,
            budget: self.budget as usize,
            filter_ground: self.filter_ground,
            filter_vegetation: self.filter_vegetation,
            filter_buildings: self.filter_buildings,
            filter_other: self.filter_other,
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
        };
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
        let preview_points = active_cloud.map_or(0, |entry| entry.cloud.points.len());
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
            opencad_properties::property_row("Source points", source_points.to_string()),
            opencad_properties::property_row(
                "Remaining",
                active_cloud
                    .map_or(0, CloudEntry::remaining_count)
                    .to_string(),
            ),
            opencad_properties::property_row(
                "Deleted",
                active_cloud
                    .map_or(0, CloudEntry::deleted_count)
                    .to_string(),
            ),
            opencad_properties::property_row("Preview points", preview_points.to_string()),
            opencad_properties::property_row("Indexed", if indexed { "Yes" } else { "No" }.into()),
            opencad_properties::property_row("Selected", selected_points.to_string()),
            opencad_properties::section_header("Geometry"),
        ]
        .spacing(0)
        .width(270);
        if let Some(entry) = active_cloud {
            for (axis, label) in ["X", "Y", "Z"].into_iter().enumerate() {
                properties = properties.push(opencad_properties::property_row(
                    label,
                    format!(
                        "{:.2} … {:.2}",
                        entry.cloud.bounds.min[axis], entry.cloud.bounds.max[axis]
                    ),
                ));
            }
            if let Some(point) = entry
                .selection
                .as_deref()
                .filter(|selection| selection.count == 1)
                .and_then(|selection| selection.highlights.first())
            {
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
                                        pose.position[0], pose.position[1], pose.position[2]
                                    ))
                                    .size(10),
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
                format!("{:.3}×", self.zoom),
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
                            (self.selected_total() > 0 && !self.section_fit_pending)
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
                    mesh.vertices.len().to_string(),
                ))
                .push(opencad_properties::property_row(
                    "Triangles",
                    mesh.triangles.len().to_string(),
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
        let properties = properties
            .push(opencad_properties::section_header("Display"))
            .push(
                container(
                    pick_list(ColorMode::ALL, Some(self.color_mode), Message::ColorMode)
                        .style(themed_pick_list_style),
                )
                .padding([6, 8]),
            );
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
        let status_bar = row![
            text(&self.status).size(11),
            text(format!(
                "{} files  ·  {} points  ·  {} selected",
                self.clouds.len(),
                total_points,
                self.selected_total()
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
        | Message::FitSectionToSelection => ToolIcon::Select,
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

fn active_tool_style(theme: &Theme, _: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    button::Style {
        background: Some(iced::Background::Color(colors.active)),
        text_color: if colors.shell == Color::BLACK {
            Color::BLACK
        } else {
            colors.accent
        },
        border: iced::Border {
            color: colors.accent,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..button::Style::default()
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
                    bounds.min[axis] = bounds.min[axis].min(entry.cloud.bounds.min[axis]);
                    bounds.max[axis] = bounds.max[axis].max(entry.cloud.bounds.max[axis]);
                }
            }
            None => overall = Some(entry.cloud.bounds),
        }
    }
    overall
}

fn loaded_bounds(clouds: &[CloudEntry]) -> Option<Bounds> {
    let mut overall: Option<Bounds> = None;
    for entry in clouds {
        include_bounds(&mut overall, entry.cloud.bounds.min);
        include_bounds(&mut overall, entry.cloud.bounds.max);
    }
    overall
}

fn bounds_with_scan_poses(clouds: &[CloudEntry]) -> Option<Bounds> {
    let mut bounds = combined_bounds(clouds);
    let mut has_scan_poses = false;
    for pose in clouds
        .iter()
        .filter(|entry| entry.visible)
        .flat_map(|entry| &entry.cloud.scan_poses)
    {
        include_bounds(&mut bounds, pose.position);
        has_scan_poses = true;
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
}

fn selected_source_bounds(sources: &[SelectedSource]) -> Result<(Bounds, u64), String> {
    let mut bounds = None;
    let mut count = 0u64;
    for source in sources {
        let cloud = &source.cloud;
        let selection = &source.selection;
        let deleted = &source.deleted;
        cloud.validate_source().map_err(|error| error.to_string())?;
        if deleted.is_none() && selection.count == selection.highlights.len() as u64 {
            for point in &selection.highlights {
                include_bounds(&mut bounds, point.xyz);
                count += 1;
            }
            continue;
        }
        let mut ordinal = 0u64;
        pointcloud_core::visit_points(&cloud.path, &mut |point| {
            if selection.contains(ordinal)
                && deleted.as_ref().is_none_or(|mask| !mask.contains(ordinal))
            {
                include_bounds(&mut bounds, point.xyz);
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
    show_scan_poses: bool,
    budget: usize,
    filter_ground: bool,
    filter_vegetation: bool,
    filter_buildings: bool,
    filter_other: bool,
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
}

fn push_scan_marker(markers: &mut Vec<ScanMarker>, x: f32, y: f32, label: &str, group: bool) {
    if group {
        if let Some(marker) = markers.iter_mut().find(|marker| {
            let dx = marker.x - x;
            let dy = marker.y - y;
            dx * dx + dy * dy <= 12.0 * 12.0
        }) {
            marker.labels.push(label.to_owned());
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
            let Some((x, y, _)) = projection.project(pose.position) else {
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
                    if matches!(
                        (button, drag.mode),
                        (mouse::Button::Right, DragMode::RightPending)
                    ) {
                        return Some(Message::ShowContextMenu([drag.position.x, drag.position.y]));
                    }
                    matches!((button, drag.mode), (mouse::Button::Left, DragMode::Select))
                        .then_some(Message::BoxSelect {
                            start: [drag.start.x, drag.start.y],
                            end: [drag.position.x, drag.position.y],
                            size: bounds.size(),
                        })
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
                for point in &selection.highlights {
                    if !self.accepts(point) {
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
                    let Some((x, y, _)) = projection.project(pose.position) else {
                        continue;
                    };
                    push_scan_marker(&mut markers, x, y, &pose.label, show_labels);
                }
            }
            for marker in markers {
                let center = UiPoint::new(marker.x, marker.y);
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
mod section_box_tests {
    use super::*;

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
        });
        let selected = selected_source_bounds(&[SelectedSource {
            index: 0,
            cloud: Arc::clone(&cloud),
            selection: Arc::clone(&selection),
            deleted: None,
        }])
        .unwrap();
        assert_eq!(selected.0.min, [2.0, 4.0, 6.0]);
        assert_eq!(selected.0.max, [9_999.0, 19_998.0, 29_997.0]);
        assert_eq!(selected.1, 2);

        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio.clouds[0].selection = Some(Arc::clone(&selection));
        let _ = studio.update(Message::FitSectionToSelection);
        assert!(studio.section_fit_pending);
        let _ = studio.update(Message::SectionFitReady(
            studio.revision,
            vec![(0, selection)],
            Ok(selected),
        ));
        assert!(studio.section_enabled);
        assert!(!studio.section_fit_pending);
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
        push_scan_marker(&mut markers, 100.0, 100.0, "Scan 1", true);
        push_scan_marker(&mut markers, 107.0, 104.0, "Scan 2", true);
        push_scan_marker(&mut markers, 140.0, 100.0, "Scan 3", true);
        assert_eq!(markers.len(), 2);
        assert_eq!(markers[0].labels, ["Scan 1", "Scan 2"]);
        assert_eq!(markers[1].labels, ["Scan 3"]);

        push_scan_marker(&mut markers, 100.0, 100.0, "Scan 4", false);
        assert_eq!(markers.len(), 3);
    }
}

#[cfg(test)]
mod editing_tests {
    use super::*;

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
