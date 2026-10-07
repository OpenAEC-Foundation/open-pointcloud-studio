//! The Detect faces tool: the flat faces of a region (floors, ceilings, walls
//! and sloped planes) and its round columns and pipes, each with its
//! outline, its size and the measured distance of the scan points to it.
//!
//! This module holds the settings of the tool, what it says about the region
//! before a job starts, the job that reads the layers with its stages and its
//! cancel, the layer of faces a scan keeps beside its mesh, what makes that
//! layer out of date, what the Options and Run steps of Mesh Pointcloud show
//! of it, the Properties section with the list of faces, the exports, the
//! commands of the local API and the `--faces` mode of the command line. The
//! faces themselves are found by the core.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use iced::widget::canvas::{self, Frame};
use iced::widget::{button, column, container, horizontal_space, row, text};
use iced::{Color, Element, Fill, Length, Size, Task};
use pointcloud_core::region_source::{resident_points, RegionSource, SourceTransform, EVERYWHERE};
use pointcloud_core::surfaces::{
    cylinder_color, detect_surfaces, deviation_legend, deviation_mesh, face_color, faces_json,
    flat_mesh, write_faces_cad, write_faces_ifc, write_faces_json, write_faces_obj, CylinderFace,
    DetectedSurfaces, FaceClass, PlaneFace, Residuals, SurfaceDetectConfig, SurfaceSource,
    SurfaceStage, DEFAULT_DEVIATION_CELLS,
};
use pointcloud_core::{
    Bounds, DrawingFormat, IndexConfig, IndexedPoint, LoadError, MeshGeometry, OctreeIndex, Point,
    PointCloud,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::bag_panel::plain_reason;
use crate::closed_mesh::{
    box_limits, choice_list, lacks_scan_ranges, number, option_value, rotation_option, turned_box,
    uncapitalised, with_scan_ranges, Layers, Learned, Sentence,
};
use crate::ui_style;
// The tests of how a layer is read size a scan by the limit of memory.
#[cfg(test)]
use crate::closed_mesh::UNINDEXED_LIMIT;
use crate::cloud_transform::CloudTransform;
use crate::i18n::{key, tr, tr_args};
use crate::job_scene::{JobLayer, JobScene};
use crate::open_progress::{Line, Phase};
use crate::selection::{ClassFilter, ClassVisibility, DeletionMask};
use crate::{
    camera_views, compact_count, display_name, drawing, format_count, measure, mesh_wizard,
    opencad_properties, same_deletion_mask, CloudEntry, Message, PointViewport, Studio,
};

/// The limits of the settings. The core asks for a tolerance and an area
/// above zero only, so the tool states the range that makes sense for a scan
/// of a building: a value outside it is one in the wrong unit.
pub(crate) const MIN_TOLERANCE: f64 = 0.001;
pub(crate) const MAX_TOLERANCE: f64 = 0.5;
pub(crate) const MIN_ANGLE: f64 = 1.0;
pub(crate) const MAX_ANGLE: f64 = 45.0;
pub(crate) const MIN_FACE_AREA: f64 = 0.01;
pub(crate) const MAX_FACE_AREA: f64 = 10_000.0;
/// The block lists this many flat faces and this many cylinders, the largest
/// of each; the exports and `list_faces` hold them all.
const LIST_ROWS: usize = 200;
/// The height of a row of the list, and of the list at its tallest.
const LIST_ROW_H: f32 = 22.0;
const LIST_MAX_H: f32 = 220.0;

const BUSY: &str = "Faces are already being detected";
const NO_FACES: &str = "Detect faces in the active scan first";
const NO_FORMAT: &str = "Choose a .json, .obj, .dxf, .dwg or .ifc file name for the faces";
const SAME_FILE: &str = "Choose a faces file different from the open scans";
const FLAT_TARGET: &str = "the scan that keeps the faces has a scale of zero";

/// How the faces are coloured in the scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colouring {
    /// Every face in a colour of its own, by its class.
    Face,
    /// The distance of the scan points to their face.
    Deviation,
}

impl Colouring {
    const ALL: [Self; 2] = [Self::Face, Self::Deviation];

    /// The name of the choice in a command.
    fn name(self) -> &'static str {
        match self {
            Self::Face => "face",
            Self::Deviation => "deviation",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|each| each.name() == name)
    }

    fn text(self) -> &'static str {
        match self {
            Self::Face => key("One colour per face"),
            Self::Deviation => key("Deviation of the points"),
        }
    }
}

/// A file the faces are written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceFormat {
    /// The planes and cylinders with their parameters, outlines, residuals
    /// and edges.
    Json,
    /// The faces as triangles, one group per face.
    Obj,
    /// The faces as 3D polyface meshes on a layer per kind, with the axes
    /// of the cylinders as lines.
    Dxf,
    /// As `Dxf`, in the binary drawing format.
    Dwg,
    /// The faces as IFC4 building element proxies: flat faces as polygonal
    /// face sets, cylinders as extruded circles.
    Ifc,
}

impl FaceFormat {
    /// The formats in the order the save dialog offers them, each with the
    /// name of its filter. The first one is what a file name without an
    /// extension gets where the system adds one.
    const ALL: [(Self, &'static str); 5] = [
        (Self::Json, "Faces with their parameters (JSON)"),
        (Self::Obj, "Faces as a mesh (OBJ)"),
        (Self::Dxf, "Faces as 3D CAD geometry (DXF)"),
        (Self::Dwg, "Faces as 3D CAD geometry (DWG)"),
        (Self::Ifc, "Faces as BIM elements (IFC)"),
    ];

    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Obj => "obj",
            Self::Dxf => "dxf",
            Self::Dwg => "dwg",
            Self::Ifc => "ifc",
        }
    }

    fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .map(|(format, _)| format)
            .find(|format| format.extension() == extension)
    }

    fn title(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Obj => "OBJ",
            Self::Dxf => "DXF",
            Self::Dwg => "DWG",
            Self::Ifc => "IFC",
        }
    }
}

/// A number without the digits that arithmetic adds, as a field shows it.
fn shown_number(value: f64) -> String {
    ((value * 1e6).round() / 1e6).to_string()
}

/// The settings of the Properties block. Numbers are kept as the text that
/// was typed, so that one is not rewritten while it is being typed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FaceSettings {
    /// How far a point may lie from its face, in millimetres.
    distance: String,
    /// How far the surface at a point may be turned from its face, in degrees.
    angle: String,
    /// The smallest face that is reported, in square metres.
    min_area: String,
    cylinders: bool,
    layers: Layers,
    colouring: Colouring,
}

impl Default for FaceSettings {
    fn default() -> Self {
        let config = SurfaceDetectConfig::default();
        Self {
            distance: shown_number(config.distance_tolerance * 1000.0),
            angle: shown_number(config.angle_tolerance_deg),
            min_area: shown_number(config.min_region_area),
            cylinders: config.detect_cylinders,
            layers: Layers::Active,
            colouring: Colouring::Face,
        }
    }
}

impl FaceSettings {
    /// What the core is asked for, or why these settings give no faces. The
    /// reason is a sentence, so that the Properties block says it in the
    /// language in use.
    fn config(&self) -> Result<SurfaceDetectConfig, Sentence> {
        let within = |value: f64, min: f64, max: f64, problem: &'static str, scale: f64| {
            if (min..=max).contains(&value) {
                Ok(value)
            } else {
                Err(Sentence::with(
                    problem,
                    &[
                        ("min", shown_number(min * scale)),
                        ("max", shown_number(max * scale)),
                    ],
                ))
            }
        };
        let millimetres = number(&self.distance).ok_or_else(|| {
            Sentence::plain(key("Distance tolerance must be a number of millimetres"))
        })?;
        let distance_tolerance = within(
            millimetres / 1000.0,
            MIN_TOLERANCE,
            MAX_TOLERANCE,
            key("The distance tolerance must lie between {min} and {max} mm"),
            1000.0,
        )?;
        let degrees = number(&self.angle)
            .ok_or_else(|| Sentence::plain(key("Angle tolerance must be a number of degrees")))?;
        let angle_tolerance_deg = within(
            degrees,
            MIN_ANGLE,
            MAX_ANGLE,
            key("The angle tolerance must lie between {min} and {max} degrees"),
            1.0,
        )?;
        let area = number(&self.min_area).ok_or_else(|| {
            Sentence::plain(key("Smallest face must be a number of square metres"))
        })?;
        let min_region_area = within(
            area,
            MIN_FACE_AREA,
            MAX_FACE_AREA,
            key("The smallest face must lie between {min} and {max} m²"),
            1.0,
        )?;
        let config = SurfaceDetectConfig {
            distance_tolerance,
            angle_tolerance_deg,
            min_region_area,
            detect_cylinders: self.cylinders,
            ..SurfaceDetectConfig::default()
        };
        // Nothing else of the config comes from the block.
        config
            .validate()
            .map_err(|_| Sentence::plain(key("A setting lies outside its limits")))?;
        Ok(config)
    }

    /// These settings with the fields a command of the local API names.
    fn with(&self, options: &FaceOptions) -> Result<Self, String> {
        let mut next = self.clone();
        if let Some(metres) = options.distance_tolerance {
            // The command names metres, so the refusal does too; the
            // sentence of the block is in millimetres.
            if !(MIN_TOLERANCE..=MAX_TOLERANCE).contains(&metres) {
                return Err(format!(
                    "distance_tolerance must lie between {MIN_TOLERANCE} and {MAX_TOLERANCE} m"
                ));
            }
            next.distance = shown_number(metres * 1000.0);
        }
        if let Some(degrees) = options.angle_tolerance {
            next.angle = shown_number(degrees);
        }
        if let Some(area) = options.min_area {
            next.min_area = shown_number(area);
        }
        if let Some(cylinders) = options.cylinders {
            next.cylinders = cylinders;
        }
        if let Some(layers) = &options.layers {
            next.layers = Layers::from_name(&layers.to_ascii_lowercase())
                .ok_or("layers must be active or visible")?;
        }
        if let Some(color) = &options.color {
            next.colouring = Colouring::from_name(&color.to_ascii_lowercase())
                .ok_or("color must be face or deviation")?;
        }
        next.config()
            .map_err(|problem| uncapitalised(&problem.english()))?;
        Ok(next)
    }

    /// The settings as `status` of the local API reports them: lengths in
    /// metres, and `null` for a field that holds no number.
    fn value(&self) -> Value {
        json!({
            "distance_tolerance": number(&self.distance).map(|millimetres| millimetres / 1000.0),
            "angle_tolerance": number(&self.angle),
            "min_area": number(&self.min_area),
            "cylinders": self.cylinders,
            "layers": self.layers.name(),
            "color": self.colouring.name(),
        })
    }
}

/// The settings a command of the local API may name. A field that is left
/// out keeps what the Properties block has.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct FaceOptions {
    /// How far a point may lie from its face, in metres, from 0.001 to 0.5.
    pub distance_tolerance: Option<f64>,
    /// How far the surface at a point may be turned from its face, in
    /// degrees, from 1 to 45.
    pub angle_tolerance: Option<f64>,
    /// The smallest face that is reported, in square metres, from 0.01 to
    /// 10000.
    pub min_area: Option<f64>,
    /// Whether round columns and pipes are looked for.
    pub cylinders: Option<bool>,
    /// `active` or `visible`.
    pub layers: Option<String>,
    /// `face` or `deviation`.
    pub color: Option<String>,
}

/// What of the window a job is made from: the layers where they stand, their
/// deleted points, the classes shown, the section box, and the scan that
/// keeps the faces. A layer without an index is held in memory when it is
/// small enough, and read from its file twice otherwise.
struct Scene {
    /// The layers in the section box, turned or not.
    job: JobScene,
    /// The place in `layers` of the scan that keeps the faces. The result
    /// holds for where that scan stood when the job started.
    anchor: usize,
}

impl std::ops::Deref for Scene {
    type Target = JobScene;

    fn deref(&self) -> &JobScene {
        &self.job
    }
}

/// Everything a job was started with.
pub(crate) struct JobInput {
    scene: Scene,
    config: SurfaceDetectConfig,
    /// Whether the two meshes of the viewer are built; the command line has
    /// no use for them.
    meshes: bool,
}

/// What a job is doing. The stages of the core come after the reads that
/// only some layers need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Stage {
    /// Reading a source once to learn which station measured each point.
    Stations,
    /// Reading a layer without an index into memory.
    Loading,
    Reading,
    Segmenting,
    Measuring,
    Outlining,
    /// Building the two meshes the viewer shows.
    Meshing,
}

impl Stage {
    const ALL: [Self; 7] = [
        Self::Stations,
        Self::Loading,
        Self::Reading,
        Self::Segmenting,
        Self::Measuring,
        Self::Outlining,
        Self::Meshing,
    ];

    fn of(stage: SurfaceStage) -> Self {
        match stage {
            SurfaceStage::Reading => Self::Reading,
            SurfaceStage::Segmenting => Self::Segmenting,
            SurfaceStage::Measuring => Self::Measuring,
            SurfaceStage::Outlining => Self::Outlining,
        }
    }

    /// The name of the stage in a job of the local API.
    fn name(self) -> &'static str {
        match self {
            Self::Stations => "stations",
            Self::Loading => "loading",
            Self::Reading => "reading",
            Self::Segmenting => "segmenting",
            Self::Measuring => "measuring",
            Self::Outlining => "outlining",
            Self::Meshing => "meshing",
        }
    }
}

/// How far a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Step {
    stage: Stage,
    done: u64,
    total: u64,
}

impl Step {
    fn fraction(self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f64 / self.total as f64).min(1.0) as f32)
    }

    /// The stage in words, with how far it is.
    fn text(self) -> String {
        let of_points = || {
            format!(
                "{} of {} points",
                compact_count(self.done.min(self.total)),
                compact_count(self.total)
            )
        };
        match self.stage {
            Stage::Stations => format!("finding the station of every point, {}", of_points()),
            Stage::Loading => format!("reading a scan without an index, {}", of_points()),
            Stage::Reading => format!("reading the points, {}", of_points()),
            Stage::Segmenting => match self.fraction() {
                Some(fraction) => {
                    format!("finding flat regions, {:.0}%", (fraction * 100.0).floor())
                }
                None => "finding flat regions".to_owned(),
            },
            Stage::Measuring => format!("measuring the points against the faces, {}", of_points()),
            Stage::Outlining => format!("tracing outline {} of {}", self.done, self.total),
            Stage::Meshing => "building the faces for the viewer".to_owned(),
        }
    }
}

/// What the worker of a job tells the window, and the window the worker.
#[derive(Default)]
pub(crate) struct Control {
    cancelled: AtomicBool,
    stage: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
    /// Clouds whose stations per point this job found out, with the cloud of
    /// the layer they stand for. Kept here and not in the result, so that a
    /// job that is cancelled or fails after the pass over the source does
    /// not cost that pass again.
    learned: Mutex<Vec<Learned>>,
}

impl Control {
    /// Keep how far the job is, and stop it when that was asked. The second
    /// pass of the core calls this from several threads; a thread that was
    /// about to report when the job was stopped gets the same answer.
    fn report(&self, stage: Stage, done: u64, total: u64) -> Result<(), LoadError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        self.stage.store(stage as u8, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        Ok(())
    }

    fn learn(&self, of: &Arc<PointCloud>, found: &Arc<PointCloud>) {
        self.learned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Arc::clone(of), Arc::clone(found)));
    }

    fn take_learned(&self) -> Vec<Learned> {
        std::mem::take(&mut *self.learned.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn snapshot(&self) -> Step {
        let stage = self.stage.load(Ordering::Relaxed);
        Step {
            stage: Stage::ALL
                .into_iter()
                .find(|known| *known as u8 == stage)
                .unwrap_or(Stage::Stations),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

fn source_transform(transform: CloudTransform) -> SourceTransform {
    SourceTransform {
        scale: transform.scale,
        offset: transform.offset,
    }
}

/// What a finished job hands back.
#[derive(Debug, Clone)]
pub struct Finished {
    surfaces: Arc<DetectedSurfaces>,
    /// The faces in a colour each and coloured by deviation, in the frame of
    /// the scan that keeps them; empty when nothing was found or no meshes
    /// were asked for.
    flat: Arc<MeshGeometry>,
    deviation: Arc<MeshGeometry>,
    seconds: f64,
}

/// The two meshes of the viewer for a result, in the source frame of the
/// scan it belongs to.
fn viewer_meshes(
    surfaces: &DetectedSurfaces,
    scale: f64,
) -> Result<(MeshGeometry, MeshGeometry), LoadError> {
    Ok((
        flat_mesh(surfaces)?,
        deviation_mesh(surfaces, scale, DEFAULT_DEVIATION_CELLS)?,
    ))
}

/// Read the layers and find their faces. This runs on a worker thread, or in
/// the process of the command line.
fn run(input: &JobInput, control: &Control) -> Result<Finished, LoadError> {
    let started = Instant::now();
    let scene = &input.scene;
    // The meshes of the viewer are kept in the frame of this scan. Without a
    // way back to that frame the whole job would be for nothing.
    if scene.layers[scene.anchor]
        .transform
        .source_xyz([0.0; 3])
        .is_none()
    {
        return Err(LoadError::InvalidData(FLAT_TARGET.into()));
    }
    scene.validate()?;
    // The side of a face comes from the station that measured its points.
    // An index cache written before scans were recorded per point gives a
    // cloud with stations that does not know which of them measured what.
    // One pass over its source tells, and the cache keeps the answer.
    let mut clouds: Vec<Arc<PointCloud>> = Vec::with_capacity(scene.layers.len());
    for layer in &scene.layers {
        let found = with_scan_ranges(&layer.cloud, layer.index.is_some(), &mut |read, total| {
            control.report(Stage::Stations, read, total)
        })?;
        clouds.push(match found {
            Some(found) => {
                control.learn(&layer.cloud, &found);
                found
            }
            None => Arc::clone(&layer.cloud),
        });
    }
    // A small layer without an index is held in memory: the job reads its
    // points twice, and its file only once.
    let mut resident: Vec<Option<Vec<IndexedPoint>>> = Vec::with_capacity(scene.layers.len());
    for layer in &scene.layers {
        resident.push(match layer.resident() {
            true => Some(resident_points(&layer.cloud, &mut |progress| {
                control.report(Stage::Loading, progress.read, progress.total)
            })?),
            false => None,
        });
    }
    let sources: Vec<SurfaceSource<'_>> = scene
        .layers
        .iter()
        .zip(&clouds)
        .zip(&resident)
        .map(|((layer, cloud), resident)| {
            let transform = layer.source_transform();
            let cloud: &PointCloud = cloud;
            SurfaceSource {
                points: match resident {
                    Some(points) => RegionSource::resident(points, transform),
                    None => RegionSource::new(cloud, layer.index.as_deref(), transform),
                },
                cloud: Some(cloud),
            }
        })
        .collect();
    let accept =
        |position: usize, ordinal: u64, point: &Point| scene.accepts(position, ordinal, point);
    let surfaces = detect_surfaces(
        &sources,
        scene.anchor,
        scene.region().unwrap_or(EVERYWHERE),
        &input.config,
        &accept,
        &mut |step| control.report(Stage::of(step.stage), step.completed, step.total),
    )?;
    let found = !surfaces.planes.is_empty() || !surfaces.cylinders.is_empty();
    let (flat, deviation) = if input.meshes && found {
        control.report(Stage::Meshing, 0, 0)?;
        viewer_meshes(&surfaces, input.config.distance_tolerance)?
    } else {
        Default::default()
    };
    Ok(Finished {
        surfaces: Arc::new(surfaces),
        flat: Arc::new(flat),
        deviation: Arc::new(deviation),
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// How a job ended, as the worker tells the window.
#[derive(Debug, Clone)]
pub enum FaceEnd {
    Done(Arc<Finished>),
    Cancelled,
    Failed(String),
}

impl FaceEnd {
    fn of(result: Result<Finished, LoadError>) -> Self {
        match result {
            Ok(finished) => Self::Done(Arc::new(finished)),
            Err(LoadError::Cancelled) => Self::Cancelled,
            Err(error) => Self::Failed(plain_reason(&error.to_string()).to_owned()),
        }
    }
}

/// A detection that is under way.
pub(crate) struct FaceJob {
    /// Tells this job from an earlier one whose answer is still on its way.
    serial: u64,
    input: Arc<JobInput>,
    control: Arc<Control>,
    started: Instant,
    api_job_id: Option<String>,
    /// The line last written to the status bar, so that a look that finds
    /// nothing new leaves the messages of other work readable.
    reported: String,
}

impl FaceJob {
    fn cancelling(&self) -> bool {
        self.control.cancelled.load(Ordering::Relaxed)
    }

    /// The stages this job goes through, in their order.
    fn stages(&self) -> Vec<Stage> {
        let layers = &self.input.scene.layers;
        Stage::ALL
            .into_iter()
            .filter(|stage| match stage {
                Stage::Stations => layers.iter().any(|layer| lacks_scan_ranges(&layer.cloud)),
                Stage::Loading => layers.iter().any(JobLayer::resident),
                Stage::Meshing => self.input.meshes,
                _ => true,
            })
            .collect()
    }

    /// The line of the status bar while the job runs.
    fn status_text(&self) -> String {
        if self.cancelling() {
            return "Cancelling the face detection…".into();
        }
        format!("Detecting faces: {}…", self.control.snapshot().text())
    }

    /// The job as `status` and `job` of the local API report it.
    fn progress_value(&self) -> Value {
        let step = self.control.snapshot();
        json!({
            "state": "running",
            "operation": "detect_faces",
            "stage": step.stage.name(),
            "completed": step.done,
            "total": step.total,
            "fraction": step.fraction(),
            "cancel_requested": self.cancelling(),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }
}

/// A value as it is written with three decimals, without the minus sign of
/// one that rounds to zero.
fn tidy(value: f64) -> f64 {
    let rounded = (value * 1000.0).round() / 1000.0;
    if rounded == 0.0 {
        0.0
    } else {
        rounded
    }
}

fn millimetres(metres: f64) -> String {
    format!("{:.1} mm", metres * 1000.0)
}

/// A count with the word for one or for several.
fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// What a detection found, in figures.
#[derive(Debug, Clone, PartialEq)]
struct Summary {
    floors: usize,
    ceilings: usize,
    walls: usize,
    sloped: usize,
    cylinders: usize,
    edges: usize,
    /// The voxel the job ended with, in metres.
    voxel: f64,
    coarse: bool,
    density_doublings: u32,
    read_points: u64,
    source_points: u64,
    working_points: u64,
    assigned_points: u64,
    region: Option<Bounds>,
    seconds: f64,
}

impl Summary {
    fn of(surfaces: &DetectedSurfaces, seconds: f64) -> Self {
        let class = |class: FaceClass| {
            surfaces
                .planes
                .iter()
                .filter(|face| face.class == class)
                .count()
        };
        Self {
            floors: class(FaceClass::Floor),
            ceilings: class(FaceClass::Ceiling),
            walls: class(FaceClass::Wall),
            sloped: class(FaceClass::Sloped),
            cylinders: surfaces.cylinders.len(),
            edges: surfaces.edges.len(),
            voxel: surfaces.voxel_size,
            coarse: surfaces.is_coarse(),
            density_doublings: surfaces.density_doublings,
            read_points: surfaces.read_points,
            source_points: surfaces.source_points,
            working_points: surfaces.working_points,
            assigned_points: surfaces.assigned_points,
            region: surfaces.region,
            seconds,
        }
    }

    fn planes(&self) -> usize {
        self.floors + self.ceilings + self.walls + self.sloped
    }

    fn count(&self) -> usize {
        self.planes() + self.cylinders
    }

    /// The faces per type, those that occur: "1 floor, 4 walls".
    fn counts(&self) -> String {
        [
            (self.floors, "floor", "floors"),
            (self.ceilings, "ceiling", "ceilings"),
            (self.walls, "wall", "walls"),
            (self.sloped, "sloped plane", "sloped planes"),
            (self.cylinders, "cylinder", "cylinders"),
        ]
        .into_iter()
        .filter(|(count, _, _)| *count > 0)
        .map(|(count, one, many)| counted(count, one, many))
        .collect::<Vec<_>>()
        .join(", ")
    }

    /// What larger voxels mean for the result, when the job needed them.
    fn coarse_note(&self) -> Option<String> {
        let voxel = format!("{:.0}", self.voxel * 1000.0);
        if self.density_doublings > 0 {
            Some(format!(
                "the points lie far apart, so voxels of {voxel} mm were used and narrow faces \
                 are lost"
            ))
        } else if self.coarse {
            Some(format!(
                "the region is large, so voxels of {voxel} mm were used and narrow faces and \
                 faces close together are lost: a smaller section box brings them back"
            ))
        } else {
            None
        }
    }

    /// The figures of a result as one line.
    fn line(&self) -> String {
        let mut line = format!(
            "{}; {:.1} s; voxels of {:.0} mm",
            self.counts(),
            self.seconds,
            self.voxel * 1000.0
        );
        if let Some(note) = self.coarse_note() {
            line.push_str("; ");
            line.push_str(&note);
        }
        line
    }

    /// The figures for the local API. Lengths are metres.
    fn value(&self) -> Value {
        json!({
            "count": self.count(),
            "planes": self.planes(),
            "cylinders": self.cylinders,
            "floors": self.floors,
            "ceilings": self.ceilings,
            "walls": self.walls,
            "sloped": self.sloped,
            "edges": self.edges,
            "voxel_size": self.voxel,
            "coarse": self.coarse,
            "density_doublings": self.density_doublings,
            "points": {
                "read": self.read_points,
                "source": self.source_points,
                "working": self.working_points,
                "assigned": self.assigned_points,
            },
            "region": self.region.map(|region| json!({"min": region.min, "max": region.max})),
            "seconds": self.seconds,
        })
    }
}

/// What a layer of faces was made from, for one of the scans that took part.
/// A change in it makes the faces out of date.
struct Basis {
    identity: Weak<PointCloud>,
    name: String,
    deleted: Option<Arc<DeletionMask>>,
    index: Option<Arc<OctreeIndex>>,
    /// Where the scan stood when the faces were detected.
    transform: CloudTransform,
}

impl Basis {
    /// What changed about this scan since the faces were detected. `several`
    /// says that more scans took part: the faces follow the scan that keeps
    /// them, so a move or scale of one of several scans takes the faces and
    /// the points of the other scans apart.
    fn changed(&self, clouds: &[CloudEntry], several: bool) -> Option<Stale> {
        let entry = self.identity.upgrade().and_then(|identity| {
            clouds
                .iter()
                .find(|entry| Arc::ptr_eq(&entry.load_identity, &identity))
        });
        let Some(entry) = entry else {
            return Some(Stale::Removed(self.name.clone()));
        };
        if !same_deletion_mask(self.deleted.as_ref(), entry.deleted.as_ref()) {
            return Some(Stale::Points(self.name.clone()));
        }
        let same_index = match (&self.index, &entry.index) {
            (Some(kept), Some(now)) => Arc::ptr_eq(kept, now),
            // A job on a scan without an index read the file itself: an
            // index built since holds those same points.
            (None, _) => true,
            (Some(_), None) => false,
        };
        if !same_index {
            return Some(Stale::Index(self.name.clone()));
        }
        // A move or scale changes one scan at a time and the mark stays, so
        // where each scan stood is enough to compare: scans that are moved
        // one after the other pass through a placement that differs.
        (several && entry.transform != self.transform).then(|| Stale::Moved(self.name.clone()))
    }
}

/// Why a layer of faces no longer belongs to the points as they are. Each
/// holds the file name of the scan it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stale {
    /// Points were deleted, restored or thinned.
    Points(String),
    /// The index the points were read through was replaced by another, or
    /// taken away.
    Index(String),
    /// A scan that took part was closed.
    Removed(String),
    /// One of several scans that took part was moved or scaled, so the scans
    /// no longer stand together as they did.
    Moved(String),
}

impl Stale {
    /// The name of the reason in the local API.
    fn name(&self) -> &'static str {
        match self {
            Self::Points(_) => "points",
            Self::Index(_) => "index",
            Self::Removed(_) => "scan",
            Self::Moved(_) => "moved",
        }
    }

    fn sentence(&self) -> Sentence {
        let (text, name) = match self {
            Self::Points(name) => (
                key("Out of date: points of {name} were deleted, restored or thinned after these faces were detected. The residuals and the deviation colours are those of the points as they were. Detect again."),
                name,
            ),
            Self::Index(name) => (
                key("Out of date: the index of {name} that the points were read through was replaced after these faces were detected. Detect again to be sure of the result."),
                name,
            ),
            Self::Removed(name) => (
                key("Out of date: {name}, a scan that took part, was closed. Detect again for the faces of the scans that are open."),
                name,
            ),
            Self::Moved(name) => (
                key("Out of date: {name} was moved or scaled after these faces were detected in several scans, so the scans no longer stand together as they did. Detect again."),
                name,
            ),
        };
        Sentence::with(text, &[("name", name.clone())])
    }
}

/// The faces a scan keeps, as a layer of its own beside its mesh.
pub(crate) struct FaceLayer {
    /// The faces as the job found them, for where the scan stood then.
    detected: Arc<DetectedSurfaces>,
    /// The same faces for where the scan stands now: what the list, the
    /// highlight and the exports use.
    placed: Arc<DetectedSurfaces>,
    /// The two meshes of the viewer, in the frame of the scan, so that a
    /// move or scale of the scan takes them along.
    flat: Arc<MeshGeometry>,
    deviation: Arc<MeshGeometry>,
    colouring: Colouring,
    visible: bool,
    /// The number of the face that is highlighted.
    selected: Option<u32>,
    basis: Vec<Basis>,
    stale: Option<Stale>,
    seconds: f64,
}

impl FaceLayer {
    /// The mesh the viewer draws for these faces; nothing while they are
    /// switched off.
    pub(crate) fn shown(&self) -> Option<&Arc<MeshGeometry>> {
        self.visible.then_some(match self.colouring {
            Colouring::Face => &self.flat,
            Colouring::Deviation => &self.deviation,
        })
    }

    /// Whether the faces are switched on, whether or not they fit the
    /// buffers of the graphics device.
    pub(crate) fn visible(&self) -> bool {
        self.visible
    }

    fn summary(&self) -> Summary {
        Summary::of(&self.placed, self.seconds)
    }

    /// How much larger lengths are now than when the faces were detected:
    /// the legend of the deviation colours follows a scaled scan.
    fn growth(&self) -> f64 {
        let (from, now) = (self.detected.placement.scale, self.placed.placement.scale);
        let volume: f64 = (0..3).map(|axis| (now[axis] / from[axis]).abs()).product();
        if volume.is_finite() && volume > 0.0 {
            volume.cbrt()
        } else {
            1.0
        }
    }

    /// The deviation that gets the full colour, in metres as the scan
    /// stands now.
    fn deviation_scale(&self) -> f64 {
        self.detected.config.distance_tolerance * self.growth()
    }

    /// Follow the scan to where it stands now. Positions, areas and
    /// residuals of the list are those of the new placement; the meshes need
    /// no change, because the viewer places them with the scan. A scan that
    /// is scaled unequally along its axes loses its cylinders, and gets them
    /// back when the scales are equal again.
    fn place(&mut self, now: SourceTransform) {
        let Some(placed) = self.detected.placed(now) else {
            // A scale of zero: the list keeps what it had.
            return;
        };
        let cylinders = placed.cylinders.len();
        let rebuilt = cylinders != self.placed.cylinders.len();
        self.placed = Arc::new(placed);
        if rebuilt {
            if let Ok((flat, deviation)) = viewer_meshes(&self.placed, self.deviation_scale()) {
                self.flat = Arc::new(flat);
                self.deviation = Arc::new(deviation);
            }
        }
        let known = self
            .selected
            .is_some_and(|id| self.placed.face(id).is_some() || self.placed.cylinder(id).is_some());
        if !known {
            self.selected = None;
        }
    }

    /// The layer as `status` of the local API reports it.
    fn value(&self, drawn: bool) -> Value {
        let mut value = self.summary().value();
        value["selected"] = json!(self.selected);
        value["visible"] = self.visible.into();
        value["drawn"] = drawn.into();
        value["color"] = self.colouring.name().into();
        value["stale"] = json!(self.stale.as_ref().map(Stale::name));
        value
    }
}

#[cfg(test)]
impl FaceLayer {
    /// A layer without faces that shows one mesh in both colourings, for
    /// the tests of the viewer.
    pub(crate) fn showing(mesh: Arc<MeshGeometry>, visible: bool) -> Self {
        let config = SurfaceDetectConfig::default();
        let surfaces = Arc::new(DetectedSurfaces {
            planes: Vec::new(),
            cylinders: Vec::new(),
            edges: Vec::new(),
            region: None,
            voxel_size: config.voxel_size,
            density_doublings: 0,
            boundary_cell: config.boundary_cell,
            read_points: 0,
            source_points: 0,
            working_points: 0,
            assigned_points: 0,
            config,
            placement: SourceTransform::default(),
        });
        Self {
            detected: Arc::clone(&surfaces),
            placed: surfaces,
            flat: Arc::clone(&mesh),
            deviation: mesh,
            colouring: Colouring::Face,
            visible,
            selected: None,
            basis: Vec::new(),
            stale: None,
            seconds: 0.0,
        }
    }
}

/// The faces of a layer as `status` of the local API lists them: null
/// without any.
pub(crate) fn layer_value(clouds: &[CloudEntry], index: usize) -> Value {
    match clouds.get(index).and_then(|entry| entry.faces.as_ref()) {
        Some(layer) => layer.value(crate::gpu_viewport::drawn_faces(clouds).contains(&index)),
        None => Value::Null,
    }
}

/// The switch of a row of the project list that shows or hides the faces of
/// its scan; nothing for a scan without faces.
pub(crate) fn layer_switch(index: usize, entry: &CloudEntry) -> Option<Element<'static, Message>> {
    let layer = entry.faces.as_ref()?;
    let name = if layer.stale.is_some() {
        tr("Faces (out of date)")
    } else {
        tr("Faces")
    };
    Some(
        ui_style::checkbox(name, layer.visible)
            .on_toggle(move |visible| Message::Faces(FaceAction::Visible(index, visible)))
            .into(),
    )
}

/// A faces file that was written.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportDone {
    path: PathBuf,
    format: FaceFormat,
    planes: usize,
    cylinders: usize,
    edges: usize,
    bytes: u64,
    stale: bool,
}

impl ExportDone {
    fn summary(&self) -> String {
        let mut line = format!(
            "Exported {} and {} as {} to {}",
            counted(self.planes, "flat face", "flat faces"),
            counted(self.cylinders, "cylinder", "cylinders"),
            self.format.title(),
            self.path.display()
        );
        if self.stale {
            line.push_str("; the faces are out of date: the points changed after the detection");
        }
        line
    }

    /// The finished job as the local API reports it.
    fn job_value(&self) -> Value {
        json!({
            "state": "complete",
            "operation": "export_faces",
            "path": self.path,
            "format": self.format.extension(),
            "planes": self.planes,
            "cylinders": self.cylinders,
            "edges": self.edges,
            "bytes": self.bytes,
            "stale": self.stale,
        })
    }
}

/// What a save needs of the faces it was asked for. The scan can move, get
/// other faces or close while the dialog is open or the file is written.
#[derive(Debug, Clone)]
pub struct ExportRequest {
    /// The faces in scene coordinates, for where their scan stood.
    surfaces: Arc<DetectedSurfaces>,
    /// The file of the scan; only its name goes into the export.
    source: PathBuf,
    stale: bool,
}

/// Write the faces as the scene shows them. The core writes a temporary
/// file first, so a failure leaves an existing destination as it was.
fn write(
    request: &ExportRequest,
    path: &Path,
    format: FaceFormat,
) -> Result<ExportDone, LoadError> {
    // A file name can hold what a comment of an OBJ file cannot.
    let name: String = display_name(&request.source)
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    match format {
        FaceFormat::Json => write_faces_json(&request.surfaces, &name, path)?,
        FaceFormat::Obj => {
            let mut comments = vec![format!("Source: {name}"), "Units: metres".to_owned()];
            if request.stale {
                comments
                    .push("The points of the scan changed after the faces were detected".into());
            }
            let comments: Vec<&str> = comments.iter().map(String::as_str).collect();
            write_faces_obj(&request.surfaces, path, &comments)?;
        }
        FaceFormat::Dxf => {
            write_faces_cad(&request.surfaces, path, DrawingFormat::Dxf)?;
        }
        FaceFormat::Dwg => {
            write_faces_cad(&request.surfaces, path, DrawingFormat::Dwg)?;
        }
        FaceFormat::Ifc => {
            let mut notes = vec!["Units: metres"];
            if request.stale {
                notes.push("The points of the scan changed after the faces were detected");
            }
            write_faces_ifc(&request.surfaces, &name, path, &notes)?;
        }
    }
    Ok(ExportDone {
        path: path.to_path_buf(),
        format,
        planes: request.surfaces.planes.len(),
        cylinders: request.surfaces.cylinders.len(),
        edges: request.surfaces.edges.len(),
        bytes: std::fs::metadata(path).map_or(0, |metadata| metadata.len()),
        stale: request.stale,
    })
}

/// The faces a job found, without keeping them: the Run step of Mesh
/// Pointcloud offers them for as long as a scan keeps these faces. The
/// allocation stays while it is looked at, so its address is never that of
/// other faces.
#[derive(Debug, Clone)]
struct FacesOf(Weak<DetectedSurfaces>);

impl FacesOf {
    /// The scan that keeps these faces now, by its place.
    fn holder(&self, clouds: &[CloudEntry]) -> Option<usize> {
        clouds.iter().position(|entry| {
            entry
                .faces
                .as_ref()
                .is_some_and(|layer| std::ptr::eq(self.0.as_ptr(), Arc::as_ptr(&layer.detected)))
        })
    }
}

impl PartialEq for FacesOf {
    fn eq(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.0, &other.0)
    }
}

/// How the last job ended, for the Run step of Mesh Pointcloud and the
/// local API.
#[derive(Debug, Clone, PartialEq)]
enum Last {
    Done {
        summary: Summary,
        /// The file name of the scan that keeps the faces, or nothing when
        /// that scan was closed while the job ran.
        kept_with: Option<String>,
        faces: FacesOf,
    },
    Cancelled,
    Failed(String),
}

impl Last {
    /// The finished job as `job` and `status` of the local API report it.
    fn value(&self) -> Value {
        match self {
            Self::Done {
                summary, kept_with, ..
            } => {
                let mut value = summary.value();
                value["state"] = "complete".into();
                value["operation"] = "detect_faces".into();
                value["source"] = json!(kept_with);
                value["kept"] = (kept_with.is_some() && summary.count() > 0).into();
                value["note"] = json!(summary.coarse_note());
                value
            }
            Self::Cancelled => json!({"state": "cancelled", "operation": "detect_faces"}),
            Self::Failed(error) => json!({
                "state": "failed",
                "operation": "detect_faces",
                "error": error,
            }),
        }
    }

    /// The line of the status bar when the job has ended.
    fn status(&self) -> String {
        match self {
            Self::Done {
                summary, kept_with, ..
            } if summary.count() == 0 => {
                let scan = kept_with.as_deref().unwrap_or("the region");
                format!(
                    "No faces found in {scan}: {} points of the region took part. Check the \
                     section box and the tolerances; faces the scan had are kept",
                    format_count(summary.source_points)
                )
            }
            Self::Done {
                summary, kept_with, ..
            } => {
                let faces = counted(summary.count(), "face", "faces");
                match kept_with {
                    Some(name) => format!("Detected {faces} in {name}: {}", summary.line()),
                    None => format!(
                        "Detected {faces}, but their scan was closed and nothing is kept: {}",
                        summary.line()
                    ),
                }
            }
            Self::Cancelled => "Face detection cancelled; faces the scan had are kept".into(),
            Self::Failed(error) => format!("Face detection failed: {error}"),
        }
    }
}

/// What the Detect faces tool holds: its settings, which the Options step of
/// Mesh Pointcloud shows, a job under way, how the last job ended and
/// whether a faces file is being saved. The faces themselves are kept with
/// their scan.
#[derive(Default)]
pub(crate) struct FaceTool {
    settings: FaceSettings,
    job: Option<FaceJob>,
    next_serial: u64,
    last: Option<Last>,
    /// The save dialog is open or a faces file is being written.
    export_pending: bool,
}

impl FaceTool {
    pub(crate) fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// The save dialog of a faces file is open or the file is written.
    pub(crate) fn is_exporting(&self) -> bool {
        self.export_pending
    }

    /// Which scans a job reads.
    pub(crate) fn layers(&self) -> Layers {
        self.settings.layers
    }

    /// Ask the worker of a running job to stop.
    fn cancel(&self) -> bool {
        match &self.job {
            Some(job) => {
                job.control.cancelled.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// The line of the strip above the scene while a job runs.
    pub(crate) fn progress_line(&self) -> Option<Line> {
        let job = self.job.as_ref()?;
        let step = job.control.snapshot();
        let cancelling = job.cancelling();
        let stages = job.stages();
        let place = stages
            .iter()
            .position(|stage| *stage == step.stage)
            .map_or(1, |place| place + 1);
        Some(Line {
            phase: Phase::Faces,
            title: if cancelling {
                "Cancelling…".to_owned()
            } else {
                "Detect faces".to_owned()
            },
            detail: format!("Step {place} of {}  ·  {}", stages.len(), step.text()),
            fraction: step.fraction(),
            timed: true,
            cancel: (!cancelling).then_some(Message::Faces(FaceAction::Cancel)),
        })
    }
}

/// Everything the Detect faces tool reacts to.
#[derive(Debug, Clone)]
pub enum FaceAction {
    Distance(String),
    Angle(String),
    MinArea(String),
    Cylinders(bool),
    Layers(Layers),
    Colouring(Colouring),
    Start,
    Poll,
    Cancel,
    Finished(u64, FaceEnd),
    /// Highlight a face of the active scan by its number, or none.
    Select(Option<u32>),
    /// Show or hide the faces of the layer at a place in the project list.
    Visible(usize, bool),
    /// Ask where to save the faces of the active scan.
    Export,
    PathChosen(ExportRequest, Option<PathBuf>),
    Exported(Option<String>, Result<ExportDone, String>),
    /// Remove the faces of the active scan.
    Clear,
}

/// Why no job can be started, in the words of the status bar and of the
/// local API.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    NoActive,
    NoLayer,
    NoPoints(String),
    /// A layer is still being read; this is the name of its file.
    Loading(String),
    /// The section box and the layers have nothing in common.
    Outside,
    /// With every visible scan asked for, the active scan takes no part: it
    /// is hidden or lies outside the section box. The name of its file.
    ActiveOut(String),
    /// With every visible scan asked for, the active layer holds 3D BAG
    /// buildings, which are no scan. The name of its file.
    BagTarget(String),
    /// The active scan has a scale of zero, so faces have no place in its
    /// frame. The name of its file.
    FlatTarget(String),
}

impl Refusal {
    fn sentence(&self) -> Sentence {
        let named = |text, name: &String| Sentence::with(text, &[("name", name.clone())]);
        match self {
            Self::NoActive => {
                Sentence::plain(key("Select a scan first: the faces are kept with it"))
            }
            Self::NoLayer => Sentence::plain(key("Show at least one scan to detect faces in")),
            Self::NoPoints(name) => named(key("{name} has no points to detect faces in"), name),
            Self::Loading(name) => named(
                key("{name} is still loading; wait for it or hide it before detecting faces"),
                name,
            ),
            Self::Outside => Sentence::plain(key(
                "The section box holds no part of the scans to detect faces in",
            )),
            Self::ActiveOut(name) => named(
                key("{name} takes no part: it is hidden or lies outside the section box. Select one of the scans that do; the faces are kept with it"),
                name,
            ),
            Self::BagTarget(name) => named(
                key("{name} holds 3D BAG buildings and cannot keep the faces of the scans: select the scan that keeps them"),
                name,
            ),
            Self::FlatTarget(name) => named(
                key("{name} has a scale of zero, so faces cannot be kept with it: give it another scale first"),
                name,
            ),
        }
    }

    fn status(&self) -> String {
        self.sentence().english()
    }

    fn api(&self) -> String {
        match self {
            Self::NoActive => "no active cloud".into(),
            Self::NoLayer => "no visible point cloud to detect faces in".into(),
            Self::NoPoints(name) => {
                format!("a point cloud has no points to detect faces in: {name}")
            }
            Self::Loading(name) => format!("a point cloud is still loading: {name}"),
            Self::Outside => {
                "the section box holds no part of the point clouds to detect faces in".into()
            }
            Self::ActiveOut(name) => format!(
                "the active layer is hidden or lies outside the section box and takes no part: \
                 {name}"
            ),
            Self::BagTarget(name) => format!(
                "the active layer holds 3D BAG buildings and cannot keep faces of the visible \
                 point clouds: {name}"
            ),
            Self::FlatTarget(name) => format!("{FLAT_TARGET}: {name}"),
        }
    }
}

/// Why the block cannot say what a job would search.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Problem {
    /// A setting holds no number or one outside its limits; the sentence
    /// says which.
    Setting(Sentence),
    Refused(Refusal),
}

/// What a job would search, as the block says it before the job starts.
#[derive(Debug, Clone, PartialEq)]
struct RegionInfo {
    /// The section box cut back to the layers, or the box around the layers.
    bounds: Bounds,
    /// Whether the section box limits the region.
    boxed: bool,
    layers: usize,
    /// The voxel the budget gives for the faces of this box alone.
    voxel: f64,
    /// Whether that is more than the voxel a job starts with.
    coarse: bool,
    budget: usize,
    /// The scans without an index that are too large to hold in memory:
    /// their files are read twice.
    streamed: Vec<String>,
}

/// The voxel a job ends with when its region holds the six faces of a box
/// and nothing else: the size it starts with, doubled until those faces fit
/// the budget of working points. A region never needs more voxels than it
/// has points, and what stands inside the box adds to them, so this is the
/// least to expect.
fn expected_voxel(bounds: Bounds, points: u64, config: &SurfaceDetectConfig) -> f64 {
    let [x, y, z]: [f64; 3] = std::array::from_fn(|axis| bounds.max[axis] - bounds.min[axis]);
    let area = 2.0 * (x * y + y * z + z * x);
    let budget = config.max_working_points as f64;
    let mut voxel = config.voxel_size;
    if points as f64 <= budget {
        return voxel;
    }
    // The size doubles at most as often as a length has binary digits.
    for _ in 0..64 {
        if area / (voxel * voxel) <= budget {
            break;
        }
        voxel *= 2.0;
    }
    voxel
}

/// What a job would search and how large that is, as the block says it.
fn region_note(region: &RegionInfo, layers: Layers) -> String {
    let size = {
        let [x, y, z]: [f64; 3] =
            std::array::from_fn(|axis| region.bounds.max[axis] - region.bounds.min[axis]);
        format!("{x:.1} × {y:.1} × {z:.1}")
    };
    let count = region.layers;
    match (region.boxed, layers) {
        (true, Layers::Active) => tr_args(
            "Searches the active scan inside the section box: {size} m.",
            &[("size", &size)],
        ),
        (true, Layers::Visible) => tr_args(
            "Searches the visible scans ({count}) inside the section box: {size} m.",
            &[("count", &count), ("size", &size)],
        ),
        (false, Layers::Active) => tr_args(
            "Searches all of the active scan: {size} m. Switch on the section box to search a part of it.",
            &[("size", &size)],
        ),
        (false, Layers::Visible) => tr_args(
            "Searches all of the visible scans ({count}): {size} m. Switch on the section box to search a part of them.",
            &[("count", &count), ("size", &size)],
        ),
    }
}

/// What a running job is doing, as the block says it.
fn stage_line(step: Step) -> String {
    match step.stage {
        Stage::Stations => tr("Finding the stations of the points…").to_owned(),
        Stage::Loading => tr("Reading a scan without an index…").to_owned(),
        Stage::Reading => tr("Reading the points…").to_owned(),
        Stage::Segmenting => tr("Finding flat regions…").to_owned(),
        Stage::Measuring => tr("Measuring the points against the faces…").to_owned(),
        Stage::Outlining => tr_args(
            "Tracing outline {done} of {total}…",
            &[("done", &step.done), ("total", &step.total)],
        ),
        Stage::Meshing => tr("Building the faces for the viewer…").to_owned(),
    }
}

/// The English name of what a face is, for the list and the details.
fn class_text(class: FaceClass) -> &'static str {
    match class {
        FaceClass::Floor => key("Floor"),
        FaceClass::Ceiling => key("Ceiling"),
        FaceClass::Wall => key("Wall"),
        FaceClass::Sloped => key("Sloped plane"),
    }
}

/// A stop of the legend in millimetres: to a tenth of a millimetre, as the
/// list shows its residuals, and without ".0" for a whole number. The
/// tolerance may hold a fraction, and a scaled scan multiplies it.
fn legend_millimetres(metres: f64) -> String {
    shown_number((metres.abs() * 10_000.0).round() / 10.0)
}

/// One line of the list of faces.
struct ListRow {
    id: u32,
    kind: &'static str,
    size: String,
    rms: f64,
    color: [u8; 3],
}

fn list_rows(surfaces: &DetectedSurfaces) -> impl Iterator<Item = ListRow> + '_ {
    let planes = surfaces.planes.iter().map(|face| ListRow {
        id: face.id,
        kind: class_text(face.class),
        size: format!("{:.2} m²", face.area),
        rms: face.residuals.rms,
        color: face_color(face),
    });
    let cylinders = surfaces.cylinders.iter().map(|face| ListRow {
        id: face.id,
        kind: key("Cylinder"),
        size: format!("⌀ {:.2} × {:.2} m", face.diameter(), face.length()),
        rms: face.residuals.rms,
        color: cylinder_color(face),
    });
    // A limit for each kind, so that many flat faces leave the cylinders in
    // the list.
    planes.take(LIST_ROWS).chain(cylinders.take(LIST_ROWS))
}

fn swatch(color: [u8; 3]) -> Element<'static, Message> {
    let [red, green, blue] = color;
    container(text(""))
        .width(10)
        .height(10)
        .style(move |_| container::Style::default().background(Color::from_rgb8(red, green, blue)))
        .into()
}

/// What a detection found, in figures: the time, the voxel, the faces per
/// type and the points on a face.
fn summary_rows(summary: &Summary) -> Vec<Element<'static, Message>> {
    vec![
        opencad_properties::property_row(
            "Last detection",
            tr_args(
                "{seconds} s, voxel {voxel} mm",
                &[
                    ("seconds", &format!("{:.1}", summary.seconds)),
                    ("voxel", &format!("{:.0}", summary.voxel * 1000.0)),
                ],
            ),
        ),
        opencad_properties::property_row(
            "Floors and ceilings",
            format!("{} + {}", summary.floors, summary.ceilings),
        ),
        opencad_properties::property_row(
            "Walls and sloped planes",
            format!("{} + {}", summary.walls, summary.sloped),
        ),
        opencad_properties::property_row("Columns and pipes", summary.cylinders.to_string()),
        opencad_properties::property_row(
            "Points on a face",
            tr_args(
                "{count} of {all}",
                &[
                    ("count", &compact_count(summary.assigned_points)),
                    ("all", &compact_count(summary.source_points)),
                ],
            ),
        ),
    ]
}

/// The rows the details of a face have in common: how many points it has
/// and how far they lie from it.
fn residual_rows(residuals: &Residuals) -> Vec<Element<'static, Message>> {
    vec![
        opencad_properties::property_row("Points", format_count(residuals.points)),
        opencad_properties::property_row("Residual (RMS)", millimetres(residuals.rms)),
        opencad_properties::property_row("95th percentile", millimetres(residuals.p95)),
        opencad_properties::property_row("Largest deviation", millimetres(residuals.max)),
    ]
}

fn plane_rows(face: &PlaneFace) -> Vec<Element<'static, Message>> {
    let [x, y, z] = face.normal.map(tidy);
    let mut rows = vec![
        opencad_properties::property_row(
            "Selected face",
            format!("{} · {}", face.id, tr(class_text(face.class))),
        ),
        opencad_properties::property_row("Normal", format!("{x:.3}  {y:.3}  {z:.3}")),
        opencad_properties::property_row("Area", format!("{:.2} m²", face.area)),
        opencad_properties::property_row(
            "Coverage",
            format!("{:.0}%", (face.coverage() * 100.0).floor()),
        ),
    ];
    rows.extend(residual_rows(&face.residuals));
    rows
}

fn cylinder_rows(face: &CylinderFace) -> Vec<Element<'static, Message>> {
    let mut rows = vec![
        opencad_properties::property_row(
            "Selected face",
            format!("{} · {}", face.id, tr(key("Cylinder"))),
        ),
        opencad_properties::property_row("Diameter", format!("{:.3} m", face.diameter())),
        opencad_properties::property_row("Length", format!("{:.2} m", face.length())),
        opencad_properties::property_row("Arc", format!("{:.0}°", face.arc_deg)),
        opencad_properties::property_row("Area", format!("{:.2} m²", face.area())),
        // A column is seen from outside and a round shaft from inside; the
        // stations tell which, and without them it is taken as a column.
        opencad_properties::property_row(
            "Scanned from",
            tr(if face.seen_from_inside {
                key("The inside")
            } else {
                key("The outside")
            })
            .to_owned(),
        ),
    ];
    rows.extend(residual_rows(&face.residuals));
    rows
}

/// The faces of a layer for `list_faces` of the local API: the JSON of the
/// export, without the outlines and the edges unless they are asked for.
fn list_value(layer: &FaceLayer, name: &str, boundaries: bool) -> Value {
    let mut document = faces_json(&layer.placed, name);
    let mut faces = document["faces"].take();
    if !boundaries {
        for face in faces.as_array_mut().into_iter().flatten() {
            if let Some(fields) = face.as_object_mut() {
                fields.remove("boundary");
            }
        }
    }
    let mut answer = json!({
        "ok": true,
        "source": document["source"],
        "count": layer.placed.planes.len() + layer.placed.cylinders.len(),
        "selected": layer.selected,
        "stale": layer.stale.as_ref().map(Stale::name),
        "region": document["region"],
        "settings": document["settings"],
        "points": document["points"],
        "faces": faces,
        "edge_count": layer.placed.edges.len(),
    });
    if boundaries {
        answer["edges"] = document["edges"].take();
    }
    answer
}

/// One face of a layer as the export describes it, outline included.
fn face_value(layer: &FaceLayer, name: &str, id: u32) -> Option<Value> {
    faces_json(&layer.placed, name)["faces"]
        .as_array_mut()?
        .iter_mut()
        .find(|face| face["id"] == id)
        .map(Value::take)
}

impl Studio {
    /// The layers a job reads and the scan that keeps the faces, or why
    /// there is nothing to search.
    fn faces_scene(&self, layers: Layers) -> Result<Scene, Refusal> {
        let active_index = self.active.filter(|index| *index < self.clouds.len());
        let active = active_index
            .map(|index| &self.clouds[index])
            .ok_or(Refusal::NoActive)?;
        let name = |entry: &CloudEntry| display_name(&entry.cloud.path).to_owned();
        // The faces are kept with the active scan and drawn in its frame.
        if active.transform.source_xyz([0.0; 3]).is_none() {
            return Err(Refusal::FlatTarget(name(active)));
        }
        let section = self.section_box();
        let around = section.map(|section| section.aabb());
        let reaches = |entry: &CloudEntry| {
            around.is_none_or(|section| {
                let bounds = entry.bounds();
                (0..3).all(|axis| {
                    bounds.min[axis] <= section.max[axis] && bounds.max[axis] >= section.min[axis]
                })
            })
        };
        let taken: Vec<&CloudEntry> = match layers {
            Layers::Active => vec![active],
            Layers::Visible => {
                // The buildings of 3D BAG are no scan: their vertices stay
                // out, and their layer does not keep the faces of scans.
                if active.bag_source {
                    return Err(Refusal::BagTarget(name(active)));
                }
                let shown: Vec<&CloudEntry> = self
                    .clouds
                    .iter()
                    .filter(|entry| {
                        entry.visible && entry.cloud.total_points > 0 && !entry.bag_source
                    })
                    .collect();
                if shown.is_empty() {
                    return Err(Refusal::NoLayer);
                }
                // A layer that does not reach the section box gives no
                // point: left out here it is not read and cannot refuse the
                // job.
                let inside: Vec<&CloudEntry> =
                    shown.into_iter().filter(|entry| reaches(entry)).collect();
                if inside.is_empty() {
                    return Err(Refusal::Outside);
                }
                inside
            }
        };
        for entry in &taken {
            // A layer that is still being read holds a cloud that was not
            // checked against its source, with a count that is not final.
            if entry.cloud.provisional {
                return Err(Refusal::Loading(name(entry)));
            }
            if entry.cloud.total_points == 0 {
                return Err(Refusal::NoPoints(name(entry)));
            }
        }
        if !taken.iter().any(|entry| reaches(entry)) {
            return Err(Refusal::Outside);
        }
        // The result holds for the placement of the scan that keeps it, so
        // that scan is one of those the job reads.
        let anchor = taken
            .iter()
            .position(|entry| std::ptr::eq(*entry, active))
            .ok_or_else(|| Refusal::ActiveOut(name(active)))?;
        let layers = taken
            .into_iter()
            .map(|entry| JobLayer::of(entry, self.closed_mesh.stationed(&entry.cloud)))
            .collect();
        Ok(Scene {
            job: JobScene::new(section, self.mesh_filter(), layers),
            anchor,
        })
    }

    /// What a job with these settings would search.
    fn faces_region(&self) -> Result<RegionInfo, Problem> {
        let settings = &self.faces.settings;
        let scene = self
            .faces_scene(settings.layers)
            .map_err(Problem::Refused)?;
        let config = settings.config().map_err(Problem::Setting)?;
        // As the core does: only the part of the box that can hold points.
        let bounds = scene.bounds().ok_or(Problem::Refused(Refusal::NoLayer))?;
        let points = scene.layers.iter().fold(0u64, |points, layer| {
            points.saturating_add(layer.cloud.total_points)
        });
        let voxel = expected_voxel(bounds, points, &config);
        Ok(RegionInfo {
            bounds,
            boxed: scene.section.is_some(),
            layers: scene.layers.len(),
            voxel,
            coarse: voxel > config.voxel_size,
            budget: config.max_working_points,
            streamed: scene
                .layers
                .iter()
                .filter(|layer| layer.streamed())
                .map(|layer| layer.name.clone())
                .collect(),
        })
    }

    fn faces_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |()| Message::Faces(FaceAction::Poll),
        )
    }

    /// Start a job on a worker thread. The window reads its progress four
    /// times a second until `FaceAction::Finished` arrives.
    fn start_face_job(
        &mut self,
        scene: Scene,
        config: SurfaceDetectConfig,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        let control = Arc::new(Control::default());
        let input = Arc::new(JobInput {
            scene,
            config,
            meshes: true,
        });
        let serial = self.faces.next_serial;
        self.faces.next_serial += 1;
        let mut job = FaceJob {
            serial,
            input: Arc::clone(&input),
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id,
            reported: String::new(),
        };
        let first = job.stages().first().copied().unwrap_or(Stage::Reading);
        let _ = control.report(first, 0, 0);
        job.reported = job.status_text();
        self.status.clone_from(&job.reported);
        self.faces.job = Some(job);
        // The result of an earlier job would read as the result of this one.
        self.faces.last = None;
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || FaceEnd::of(run(&input, &control)))
                    .await
                    .unwrap_or_else(|error| FaceEnd::Failed(error.to_string()))
            },
            move |end| Message::Faces(FaceAction::Finished(serial, end)),
        );
        Task::batch([worker, Self::faces_poll_task()])
    }

    /// The faces of the active scan, with the place of that scan.
    fn active_faces(&self) -> Option<(usize, &CloudEntry, &FaceLayer)> {
        let index = self.active?;
        let entry = self.clouds.get(index)?;
        Some((index, entry, entry.faces.as_ref()?))
    }

    fn active_faces_mut(&mut self) -> Option<&mut FaceLayer> {
        self.clouds.get_mut(self.active?)?.faces.as_mut()
    }

    pub(crate) fn update_faces(&mut self, action: FaceAction) -> Task<Message> {
        match action {
            FaceAction::Distance(value) => self.faces.settings.distance = value,
            FaceAction::Angle(value) => self.faces.settings.angle = value,
            FaceAction::MinArea(value) => self.faces.settings.min_area = value,
            FaceAction::Cylinders(value) => self.faces.settings.cylinders = value,
            FaceAction::Layers(layers) => self.faces.settings.layers = layers,
            FaceAction::Colouring(colouring) => self.set_face_colouring(colouring),
            FaceAction::Start => {
                if self.faces.is_running() {
                    self.status = BUSY.into();
                    return Task::none();
                }
                let config = match self.faces.settings.config() {
                    Ok(config) => config,
                    Err(problem) => {
                        self.status = problem.english();
                        return Task::none();
                    }
                };
                match self.faces_scene(self.faces.settings.layers) {
                    Ok(scene) => return self.start_face_job(scene, config, None),
                    Err(refusal) => self.status = refusal.status(),
                }
            }
            FaceAction::Poll => {
                let Some(job) = &mut self.faces.job else {
                    return Task::none();
                };
                let text = job.status_text();
                if text != job.reported {
                    self.status.clone_from(&text);
                    job.reported = text;
                }
                if let Some(entry) = job
                    .api_job_id
                    .as_ref()
                    .and_then(|id| self.api_jobs.get_mut(id))
                {
                    *entry = job.progress_value();
                }
                return Self::faces_poll_task();
            }
            FaceAction::Cancel => self.cancel_faces(),
            FaceAction::Finished(serial, end) => {
                let Some(job) = self.faces.job.take_if(|job| job.serial == serial) else {
                    return Task::none();
                };
                self.faces_finished(job, end);
            }
            FaceAction::Select(id) => {
                if let Some(layer) = self.active_faces_mut() {
                    // A click on the highlighted row takes the highlight off.
                    layer.selected = id.filter(|id| layer.selected != Some(*id));
                }
            }
            FaceAction::Visible(index, visible) => {
                if let Some(layer) = self
                    .clouds
                    .get_mut(index)
                    .and_then(|entry| entry.faces.as_mut())
                {
                    layer.visible = visible;
                }
            }
            FaceAction::Export => return self.export_faces_of(self.active),
            FaceAction::PathChosen(request, path) => {
                let Some(path) = path else {
                    self.faces.export_pending = false;
                    self.status = "Faces export cancelled".into();
                    return Task::none();
                };
                match self.faces_destination(&path) {
                    Ok(format) => return self.start_faces_export(request, path, format, None),
                    Err(problem) => {
                        self.faces.export_pending = false;
                        self.status = problem.into();
                    }
                }
            }
            FaceAction::Exported(api_job_id, result) => {
                self.faces.export_pending = false;
                if let Some(job) = api_job_id.and_then(|id| self.api_jobs.get_mut(&id)) {
                    *job = match &result {
                        Ok(done) => done.job_value(),
                        Err(error) => json!({
                            "state": "failed",
                            "operation": "export_faces",
                            "error": error,
                        }),
                    };
                }
                match result {
                    Ok(done) => {
                        self.status = done.summary();
                        self.cad_file_written(&done.path);
                    }
                    Err(error) => self.status = format!("Faces export failed: {error}"),
                }
            }
            FaceAction::Clear => {
                self.status = match self.clear_faces() {
                    Some(name) => format!("Faces of {name} cleared"),
                    None => "The active scan has no faces to clear".into(),
                };
            }
        }
        Task::none()
    }

    /// Colour the faces of every scan the way the block says.
    fn set_face_colouring(&mut self, colouring: Colouring) {
        self.faces.settings.colouring = colouring;
        for layer in self
            .clouds
            .iter_mut()
            .filter_map(|entry| entry.faces.as_mut())
        {
            layer.colouring = colouring;
        }
    }

    /// Take the faces of the active scan away; the name of that scan when
    /// it had any.
    fn clear_faces(&mut self) -> Option<String> {
        let entry = self.clouds.get_mut(self.active?)?;
        entry.faces.take()?;
        Some(display_name(&entry.cloud.path).to_owned())
    }

    /// Ask a running job to stop. The step under way ends first; the faces
    /// the scan had stay as they were.
    pub(crate) fn cancel_faces(&mut self) {
        if self.faces.cancel() {
            if let Some(job) = &mut self.faces.job {
                job.reported = job.status_text();
                self.status.clone_from(&job.reported);
            }
        }
    }

    /// A job ended: give the faces to their scan, keep what the job reports
    /// and tell the job of the local API.
    fn faces_finished(&mut self, job: FaceJob, end: FaceEnd) {
        // The stations a job found are kept however it ended, with those
        // the Closed mesh tool found: both tools ask there before a job.
        self.closed_mesh.keep_stations(job.control.take_learned());
        let last = match end {
            FaceEnd::Done(finished) => {
                let scene = &job.input.scene;
                let target = &scene.layers[scene.anchor].identity;
                let colouring = self.faces.settings.colouring;
                let summary = Summary::of(&finished.surfaces, finished.seconds);
                let entry = self
                    .clouds
                    .iter_mut()
                    .find(|entry| Arc::ptr_eq(&entry.load_identity, target));
                let kept_with = entry.map(|entry| {
                    // Nothing found leaves the scan what it had.
                    if summary.count() > 0 {
                        entry.faces = Some(FaceLayer {
                            detected: Arc::clone(&finished.surfaces),
                            placed: Arc::clone(&finished.surfaces),
                            flat: Arc::clone(&finished.flat),
                            deviation: Arc::clone(&finished.deviation),
                            colouring,
                            visible: true,
                            selected: None,
                            basis: scene
                                .layers
                                .iter()
                                .map(|layer| Basis {
                                    identity: Arc::downgrade(&layer.identity),
                                    name: layer.name.clone(),
                                    deleted: layer.deleted.as_ref().map(Arc::clone),
                                    index: layer.index.as_ref().map(Arc::clone),
                                    transform: layer.transform,
                                })
                                .collect(),
                            stale: None,
                            seconds: finished.seconds,
                        });
                    }
                    display_name(&entry.cloud.path).to_owned()
                });
                Last::Done {
                    summary,
                    kept_with,
                    faces: FacesOf(Arc::downgrade(&finished.surfaces)),
                }
            }
            FaceEnd::Cancelled => Last::Cancelled,
            FaceEnd::Failed(error) => Last::Failed(error),
        };
        if let Some(entry) = job
            .api_job_id
            .as_ref()
            .and_then(|id| self.api_jobs.get_mut(id))
        {
            *entry = last.value();
        }
        self.status = last.status();
        self.faces.last = Some(last);
    }

    /// After every message: let the faces of a scan follow a move or scale
    /// of that scan, and mark them out of date when the points they were
    /// made from are no longer the points of the scene. That is so when
    /// points of a scan that took part were deleted, restored or thinned,
    /// when the index such a scan was read through was replaced, when one
    /// was closed, and, for faces made from several scans, when one of those
    /// scans was moved or scaled: the faces follow the scan that keeps them,
    /// the points of the other scans do not. A change of the classes shown
    /// or of the section box leaves the faces as they are, as it leaves a
    /// mesh.
    pub(crate) fn settle_faces(&mut self) {
        for index in 0..self.clouds.len() {
            let entry = &self.clouds[index];
            let Some(layer) = &entry.faces else {
                continue;
            };
            let now = source_transform(entry.transform);
            let moved = layer.placed.placement != now;
            let stale = match layer.stale {
                Some(_) => None,
                None => {
                    let several = layer.basis.len() > 1;
                    layer
                        .basis
                        .iter()
                        .find_map(|basis| basis.changed(&self.clouds, several))
                }
            };
            if !moved && stale.is_none() {
                continue;
            }
            let Some(layer) = self.clouds[index].faces.as_mut() else {
                continue;
            };
            if let Some(stale) = stale {
                layer.stale = Some(stale);
                // What the faces were made from is no longer looked at, and
                // need not be held.
                layer.basis.clear();
            }
            if moved {
                layer.place(now);
            }
        }
    }

    /// The format a destination asks for through its extension, or why
    /// nothing is written there.
    fn faces_destination(&self, destination: &Path) -> Result<FaceFormat, &'static str> {
        let format = FaceFormat::from_path(destination).ok_or(NO_FORMAT)?;
        // An OBJ file can be open as a layer of its own.
        let key = camera_views::source_key(destination);
        if self
            .clouds
            .iter()
            .any(|entry| camera_views::source_key(&entry.cloud.path) == key)
        {
            return Err(SAME_FILE);
        }
        Ok(format)
    }

    /// The faces of the active scan with what a save needs of that scan.
    fn faces_export_request(&self) -> Option<ExportRequest> {
        self.faces_export_request_of(self.active)
    }

    /// What an export of the faces of a scan, by its place, writes.
    pub(crate) fn faces_export_request_of(&self, index: Option<usize>) -> Option<ExportRequest> {
        let entry = self.clouds.get(index?)?;
        let layer = entry.faces.as_ref()?;
        Some(ExportRequest {
            surfaces: Arc::clone(&layer.placed),
            source: entry.cloud.path.clone(),
            stale: layer.stale.is_some(),
        })
    }

    /// Write the file on a worker thread. `status` of the local API reports
    /// `faces.export_pending` until `FaceAction::Exported` arrives, which is
    /// what a wait for an idle window looks at.
    fn start_faces_export(
        &mut self,
        request: ExportRequest,
        path: PathBuf,
        format: FaceFormat,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        self.faces.export_pending = true;
        self.status = format!(
            "Writing {} as {}…",
            counted(
                request.surfaces.planes.len() + request.surfaces.cylinders.len(),
                "face",
                "faces"
            ),
            format.title()
        );
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    write(&request, &path, format)
                        .map_err(|error| plain_reason(&error.to_string()).to_owned())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Message::Faces(FaceAction::Exported(api_job_id.clone(), result)),
        )
    }

    /// Ask where to save the faces a scan keeps, by its place: the active
    /// scan for Export faces… and the File view, the scan that keeps the
    /// result for the Run step of Mesh Pointcloud.
    pub(crate) fn export_faces_of(&mut self, index: Option<usize>) -> Task<Message> {
        if self.faces.export_pending {
            return Task::none();
        }
        let Some(request) = self.faces_export_request_of(index) else {
            self.status = NO_FACES.into();
            return Task::none();
        };
        let stem = request
            .source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("scan");
        let suggestion = format!("{stem}-faces.{}", FaceFormat::ALL[0].0.extension());
        self.faces.export_pending = true;
        self.status = "Choose where to save the faces as JSON, OBJ, DXF, DWG or IFC…".into();
        Task::perform(
            async move {
                FaceFormat::ALL
                    .iter()
                    .fold(rfd::AsyncFileDialog::new(), |dialog, (format, name)| {
                        dialog.add_filter(*name, &[format.extension()])
                    })
                    .set_file_name(suggestion)
                    .save_file()
                    .await
                    .map(|selection| selection.path().to_path_buf())
            },
            move |path| Message::Faces(FaceAction::PathChosen(request.clone(), path)),
        )
    }

    /// Whether the File view offers to save faces: the active scan has them
    /// and no save is under way.
    pub(crate) fn faces_entry_enabled(&self) -> bool {
        self.active_faces().is_some() && !self.faces.export_pending
    }

    /// The tool as `status` of the local API reports it.
    pub(crate) fn faces_value(&self) -> Value {
        let tool = &self.faces;
        json!({
            "settings": tool.settings.value(),
            "job": tool.job.as_ref().map(FaceJob::progress_value),
            "last": tool.last.as_ref().map(Last::value),
            "export_pending": tool.export_pending,
            "result": self
                .active
                .map_or(Value::Null, |index| layer_value(&self.clouds, index)),
        })
    }

    /// The `set_face_settings` command of the local API: every field it
    /// names goes into the block, or none when one of them is refused.
    pub(crate) fn api_set_face_settings(&mut self, options: &FaceOptions) -> Value {
        match self.faces.settings.with(options) {
            Ok(settings) => {
                let colouring = settings.colouring;
                self.faces.settings = settings;
                self.set_face_colouring(colouring);
                json!({"ok": true, "settings": self.faces.settings.value()})
            }
            Err(problem) => json!({"ok": false, "error": problem}),
        }
    }

    /// The `detect_faces` command of the local API: put the fields it names
    /// in the Properties block and detect with what the block then holds.
    pub(crate) fn api_detect_faces(&mut self, options: &FaceOptions) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        if self.faces.is_running() {
            return refuse(uncapitalised(BUSY));
        }
        let settings = match self.faces.settings.with(options) {
            Ok(settings) => settings,
            Err(problem) => return refuse(problem),
        };
        let config = match settings.config() {
            Ok(config) => config,
            Err(problem) => return refuse(uncapitalised(&problem.english())),
        };
        let scene = match self.faces_scene(settings.layers) {
            Ok(scene) => scene,
            Err(refusal) => return refuse(refusal.api()),
        };
        let colouring = settings.colouring;
        self.faces.settings = settings;
        self.set_face_colouring(colouring);
        let id = self.record_api_job(json!({"state": "running", "operation": "detect_faces"}));
        let task = self.start_face_job(scene, config, Some(id.clone()));
        (json!({"ok": true, "accepted": true, "job_id": id}), task)
    }

    /// The `cancel_detect_faces` command of the local API.
    pub(crate) fn api_cancel_detect_faces(&mut self) -> Value {
        if !self.faces.is_running() {
            return json!({"ok": false, "error": "no face detection is running"});
        }
        self.cancel_faces();
        json!({"ok": true, "cancel_requested": true})
    }

    /// The `list_faces` command of the local API: the faces of the active
    /// scan as the export describes them, in scene coordinates.
    pub(crate) fn api_list_faces(&self, boundaries: bool) -> Value {
        if self
            .active
            .and_then(|index| self.clouds.get(index))
            .is_none()
        {
            return json!({"ok": false, "error": "no active cloud"});
        }
        match self.active_faces() {
            Some((_, entry, layer)) => {
                list_value(layer, display_name(&entry.cloud.path), boundaries)
            }
            None => json!({"ok": false, "error": "the active layer has no detected faces"}),
        }
    }

    /// The `select_face` command of the local API: highlight a face of the
    /// active scan by its number, or none.
    pub(crate) fn api_select_face(&mut self, id: Option<u32>) -> Value {
        let Some((_, entry, layer)) = self.active_faces() else {
            return json!({"ok": false, "error": "the active layer has no detected faces"});
        };
        let face = match id {
            Some(id) => match face_value(layer, display_name(&entry.cloud.path), id) {
                Some(face) => face,
                None => {
                    return json!({
                        "ok": false,
                        "error": format!(
                            "no face has the number {id}; list_faces gives the numbers"
                        ),
                    })
                }
            },
            None => Value::Null,
        };
        if let Some(layer) = self.active_faces_mut() {
            layer.selected = id;
        }
        json!({"ok": true, "selected": id, "face": face})
    }

    /// The `export_faces` command of the local API: save the faces of the
    /// active scan to a path whose extension names the format.
    pub(crate) fn api_export_faces(&mut self, path: PathBuf) -> (Value, Task<Message>) {
        let refuse = |error: &str| (json!({"ok": false, "error": error}), Task::none());
        let Some(format) = FaceFormat::from_path(&path).filter(|_| path.is_absolute()) else {
            return refuse(
                "export_faces requires an absolute .json, .obj, .dxf, .dwg or .ifc destination",
            );
        };
        if self.faces.export_pending {
            return refuse("a faces export is already open or running");
        }
        if self
            .active
            .and_then(|index| self.clouds.get(index))
            .is_none()
        {
            return refuse("no active cloud");
        }
        let Some(request) = self.faces_export_request() else {
            return refuse("the active layer has no detected faces");
        };
        if self.faces_destination(&path).is_err() {
            return refuse("export_faces requires a destination different from the open scans");
        }
        if !path.parent().is_some_and(Path::is_dir) {
            return refuse("the folder of the faces destination does not exist");
        }
        let id = self.record_api_job(json!({
            "state": "running",
            "operation": "export_faces",
            "path": path,
            "format": format.extension(),
        }));
        let task = self.start_faces_export(request, path.clone(), format, Some(id.clone()));
        (
            json!({"ok": true, "accepted": true, "job_id": id, "path": path}),
            task,
        )
    }

    /// The `clear_faces` command of the local API.
    pub(crate) fn api_clear_faces(&mut self) -> Value {
        if self
            .active
            .and_then(|index| self.clouds.get(index))
            .is_none()
        {
            return json!({"ok": false, "error": "no active cloud"});
        }
        let cleared = self.clear_faces().is_some();
        if cleared {
            self.status = "Faces cleared".into();
        }
        json!({"ok": true, "cleared": cleared})
    }

    /// The settings of a detection as the Options step of Mesh Pointcloud
    /// shows them, each with its default and its explanation.
    pub(crate) fn faces_options(&self) -> Element<'_, Message> {
        let settings = &self.faces.settings;
        let defaults = FaceSettings::default();
        let send = Message::Faces;
        column![
            mesh_wizard::option_input(
                "Distance tolerance (mm)",
                &settings.distance,
                "20",
                defaults.distance.clone(),
                move |value| send(FaceAction::Distance(value)),
                key("How far a point may lie from the plane of its face, from 1 to 500 mm. Take about three times the noise of the scan or more: with less a wall falls apart into pieces, with more a step in a wall is taken into the same face."),
            ),
            mesh_wizard::option_input(
                "Angle tolerance (°)",
                &settings.angle,
                "10",
                defaults.angle.clone(),
                move |value| send(FaceAction::Angle(value)),
                key("How far the surface at a point may be turned from the plane of its face, from 1 to 45 degrees."),
            ),
            mesh_wizard::option_input(
                "Smallest face (m²)",
                &settings.min_area,
                "0.25",
                defaults.min_area.clone(),
                move |value| send(FaceAction::MinArea(value)),
                key("Smaller faces are not reported, from 0.01 to 10000 m². A value above the faces that are there finds nothing and makes the job slow."),
            ),
            mesh_wizard::option_control(
                "Cylinders",
                ui_style::checkbox(tr("Columns and pipes"), settings.cylinders)
                    .on_toggle(|value| Message::Faces(FaceAction::Cylinders(value)))
                    .into(),
                tr(if defaults.cylinders { key("On") } else { key("Off") }).to_owned(),
                key("Whether round columns and pipes are looked for among the points that no flat face took."),
            ),
            mesh_wizard::option_control(
                "Scans",
                choice_list(Layers::ALL, settings.layers, Layers::text, |layers| {
                    Message::Faces(FaceAction::Layers(layers))
                }),
                tr(defaults.layers.text()).to_owned(),
                key("The active scan, or every visible scan that reaches the section box, without layers of 3D BAG buildings. The faces are kept with the active scan, which has to be one of them."),
            ),
        ]
        .spacing(4)
        .into()
    }

    /// What the Options step says of a detection before it runs: what it
    /// would search, the voxel the budget gives, warnings, and why it cannot
    /// start.
    pub(crate) fn faces_notes(&self) -> mesh_wizard::Notes {
        let mut notes = mesh_wizard::Notes::default();
        if self.faces.is_running() {
            notes.refusal = Some(tr("Faces are already being detected").to_owned());
        }
        match self.faces_region() {
            Ok(region) => {
                notes
                    .lines
                    .push(region_note(&region, self.faces.settings.layers));
                let voxel = format!("{:.0}", region.voxel * 1000.0);
                let budget = format_count(region.budget);
                let values: [(&str, &dyn fmt::Display); 2] =
                    [("voxel", &voxel), ("budget", &budget)];
                if region.coarse {
                    notes.warnings.push(tr_args(
                        "This region needs voxels of {voxel} mm or more to stay within {budget} working points: narrow faces and faces close together are lost. A smaller section box brings them back.",
                        &values,
                    ));
                } else {
                    notes.lines.push(tr_args(
                        "Voxels of {voxel} mm: the budget of {budget} working points holds the faces of this box. Walls and objects inside it take more and can make the voxels larger.",
                        &values,
                    ));
                }
                for name in &region.streamed {
                    notes.warnings.push(tr_args(
                        "{name} has no index: its file is read twice. Build the index first (INDEX > Build index) for a faster job.",
                        &[("name", name)],
                    ));
                }
            }
            Err(problem) => {
                let sentence = match problem {
                    Problem::Setting(sentence) => sentence.translated(),
                    Problem::Refused(refusal) => refusal.sentence().translated(),
                };
                notes.refusal.get_or_insert(sentence);
            }
        }
        notes.lines.push(
            tr("The faces are kept with the active scan, as a layer beside its points and its mesh; faces it had are replaced.")
                .to_owned(),
        );
        notes
    }

    /// Put the recommended settings in the options: those a detection starts
    /// with. Which scans take part, and the colouring of the faces shown,
    /// stay as they were chosen.
    pub(crate) fn recommend_faces(&mut self) {
        let settings = &self.faces.settings;
        self.faces.settings = FaceSettings {
            layers: settings.layers,
            colouring: settings.colouring,
            ..FaceSettings::default()
        };
    }

    /// Whether the options hold the recommended settings.
    pub(crate) fn faces_recommended(&self) -> bool {
        let settings = &self.faces.settings;
        *settings
            == FaceSettings {
                layers: settings.layers,
                colouring: settings.colouring,
                ..FaceSettings::default()
            }
    }

    /// The scan that keeps the faces of the last job, by its place; nothing
    /// when they were cleared or replaced, or their scan closed.
    pub(crate) fn faces_holder(&self) -> Option<usize> {
        match &self.faces.last {
            Some(Last::Done { faces, .. }) => faces.holder(&self.clouds),
            _ => None,
        }
    }

    /// A job under way, or how the last one ended, as the Run step of Mesh
    /// Pointcloud shows it.
    pub(crate) fn faces_run(&self) -> mesh_wizard::RunState {
        let tool = &self.faces;
        if let Some(job) = &tool.job {
            let step = job.control.snapshot();
            let stages = job.stages();
            let place = stages
                .iter()
                .position(|stage| *stage == step.stage)
                .map_or(1, |place| place + 1);
            let cancelling = job.cancelling();
            return mesh_wizard::RunState::Running(mesh_wizard::Progress {
                stage: if cancelling {
                    tr("Cancelling…").to_owned()
                } else {
                    stage_line(step)
                },
                steps: Some((place, stages.len())),
                fraction: step.fraction(),
                seconds: job.started.elapsed().as_secs(),
                cancelling,
            });
        }
        match &tool.last {
            None => mesh_wizard::RunState::Idle,
            Some(Last::Done {
                summary, kept_with, ..
            }) => {
                let mut lines = Vec::new();
                let mut warnings = Vec::new();
                let mut kept = None;
                if summary.count() == 0 {
                    warnings.push(
                        tr("The last face detection found no faces. Check the section box and the tolerances.")
                            .to_owned(),
                    );
                } else {
                    match kept_with {
                        Some(name) => {
                            kept = Some(tr_args(
                                "Kept with {name}, as a layer beside its points and its mesh; faces it had are replaced.",
                                &[("name", name)],
                            ));
                        }
                        None => lines.push(
                            tr("Its scan was closed while the job ran, so nothing is kept.")
                                .to_owned(),
                        ),
                    }
                }
                if summary.density_doublings > 0 {
                    warnings.push(
                        tr("The points lie far apart, so larger voxels were used: narrow faces are lost.")
                            .to_owned(),
                    );
                } else if summary.coarse {
                    warnings.push(
                        tr("The region was large, so larger voxels were used: narrow faces and faces close together are lost. A smaller section box brings them back.")
                            .to_owned(),
                    );
                }
                mesh_wizard::RunState::Done {
                    rows: summary_rows(summary),
                    lines,
                    warnings,
                    kept,
                }
            }
            Some(Last::Cancelled) => mesh_wizard::RunState::Cancelled(
                tr("The last face detection was cancelled.").to_owned(),
            ),
            Some(Last::Failed(error)) => mesh_wizard::RunState::Failed(error.clone()),
        }
    }

    /// The faces the active scan keeps, in Properties: their figures, the
    /// colouring with its legend, the list, the details of the face that is
    /// highlighted and the buttons that save and clear them. Nothing for a
    /// scan without faces; the settings of a detection are in Mesh
    /// Pointcloud.
    pub(crate) fn faces_properties(&self) -> Option<Element<'_, Message>> {
        let (index, _, layer) = self.active_faces()?;
        Some(
            column(self.faces_result(index, layer))
                .spacing(0)
                .width(Fill)
                .into(),
        )
    }

    /// The faces of the active scan as the block shows them: the figures,
    /// the colouring with its legend, the list, the details of the face
    /// that is highlighted and the buttons that save and clear.
    fn faces_result<'a>(&'a self, index: usize, layer: &'a FaceLayer) -> Vec<Element<'a, Message>> {
        let colors = self.ui_theme.colors();
        let note = |content: String| {
            container(text(content).size(10).color(colors.text_muted)).padding([4, 8])
        };
        let warning = |content: String| {
            container(text(content).size(10).color(colors.accent)).padding([4, 8])
        };
        let summary = layer.summary();
        let surfaces = &layer.placed;
        let mut parts: Vec<Element<'a, Message>> =
            vec![opencad_properties::section_header("Detected faces")];
        parts.extend(summary_rows(&summary));
        if let Some(stale) = &layer.stale {
            parts.push(warning(stale.sentence().translated()).into());
        }
        if summary.density_doublings > 0 {
            parts.push(
                warning(
                    tr("The points lie far apart, so larger voxels were used: narrow faces are lost.")
                        .to_owned(),
                )
                .into(),
            );
        } else if summary.coarse {
            parts.push(
                warning(
                    tr("The region was large, so larger voxels were used: narrow faces and faces close together are lost. A smaller section box brings them back.")
                        .to_owned(),
                )
                .into(),
            );
        }
        // All meshes share the buffers of the graphics device. Faces that no
        // longer fit beside the meshes are held, listed and saved, but not
        // drawn.
        if layer.visible && !crate::gpu_viewport::drawn_faces(&self.clouds).contains(&index) {
            parts.push(
                warning(
                    tr("Not drawn: the meshes shown together hold more than the graphics device takes. Switch off the surface of another scan.")
                        .to_owned(),
                )
                .into(),
            );
        }
        parts.push(opencad_properties::property_control(
            "Colour by",
            choice_list(
                Colouring::ALL,
                layer.colouring,
                Colouring::text,
                |colouring| Message::Faces(FaceAction::Colouring(colouring)),
            ),
        ));
        if layer.colouring == Colouring::Deviation {
            let [behind, on, front] = deviation_legend(layer.deviation_scale());
            let stop = |color: [u8; 3], content: String| -> Element<'a, Message> {
                container(
                    row![swatch(color), text(content).size(10)]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                )
                .padding([2, 8])
                .into()
            };
            let value = legend_millimetres;
            parts.push(stop(
                behind.color,
                tr_args(
                    "{value} mm or more behind the face",
                    &[("value", &value(behind.value))],
                ),
            ));
            parts.push(stop(on.color, tr("On the face").to_owned()));
            parts.push(stop(
                front.color,
                tr_args(
                    "{value} mm or more in front of the face",
                    &[("value", &value(front.value))],
                ),
            ));
            parts.push(
                note(
                    tr("In between the colours blend. The front of a face is the side it was scanned from; the light of the scene shades the colours.")
                        .to_owned(),
                )
                .into(),
            );
        }

        let total = surfaces.planes.len() + surfaces.cylinders.len();
        let list = list_rows(surfaces).fold(column![].spacing(0).width(Fill), |list, face| {
            let chosen = layer.selected == Some(face.id);
            list.push(
                button(
                    row![
                        swatch(face.color),
                        text(face.id.to_string()).size(10).width(22),
                        text(tr(face.kind)).size(10),
                        horizontal_space(),
                        text(face.size).size(10),
                        text(millimetres(face.rms))
                            .size(10)
                            .width(48)
                            .align_x(iced::Alignment::End),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center),
                )
                .on_press(Message::Faces(FaceAction::Select(Some(face.id))))
                .style(move |theme, status| ui_style::ribbon_button(theme, chosen, status))
                .width(Fill)
                .height(LIST_ROW_H)
                .padding([2, 8]),
            )
        });
        let shown = surfaces.planes.len().min(LIST_ROWS) + surfaces.cylinders.len().min(LIST_ROWS);
        parts.push(
            note(tr("Number, type, area and residual (RMS). Click a face to highlight it in the scene.").to_owned())
                .into(),
        );
        parts.push(
            ui_style::scrollable(list)
                .height(Length::Fixed((shown as f32 * LIST_ROW_H).min(LIST_MAX_H)))
                .into(),
        );
        if total > shown {
            parts.push(
                note(tr_args(
                    "The list shows {shown} of {total} faces, the largest of each kind; an export holds them all.",
                    &[("shown", &shown), ("total", &total)],
                ))
                .into(),
            );
        }
        if let Some(id) = layer.selected {
            if let Some(face) = surfaces.face(id) {
                parts.extend(plane_rows(face));
            } else if let Some(face) = surfaces.cylinder(id) {
                parts.extend(cylinder_rows(face));
            }
        }
        parts.push(
            container(
                row![
                    button(tr("Export faces…"))
                        .on_press_maybe(
                            (!self.faces.export_pending)
                                .then_some(Message::Faces(FaceAction::Export)),
                        )
                        .style(ui_style::tool),
                    button(tr("Clear faces"))
                        .on_press(Message::Faces(FaceAction::Clear))
                        .style(ui_style::tool),
                ]
                .spacing(8),
            )
            .padding([4, 8])
            .into(),
        );
        parts.push(self.cad_viewer_controls());
        parts
    }
}

impl PointViewport<'_> {
    /// Lay the face that is highlighted over the scene, for every scan whose
    /// faces are switched on: a flat face as its outline with its openings,
    /// filled by the even-odd rule, and a cylinder as the lines along its
    /// scanned part with its axis. It is drawn on top of the points without
    /// a depth test.
    pub fn draw_faces(&self, frame: &mut Frame, size: Size) {
        let Some(scene) = crate::combined_bounds(self.clouds) else {
            return;
        };
        let projection = self.projection(scene, size.width, size.height);
        let accent = Color::from_rgb8(245, 158, 11);
        let outline = canvas::Stroke::default().with_color(accent).with_width(2.0);
        let stroke_edges = |frame: &mut Frame, edges: &[[[f64; 3]; 2]]| {
            let path = canvas::Path::new(|path| {
                for [from, to] in edges {
                    if let Some([start, end]) = measure::project_edge(projection, *from, *to, size)
                    {
                        path.move_to(start);
                        path.line_to(end);
                    }
                }
            });
            frame.stroke(&path, outline);
        };
        for layer in self
            .clouds
            .iter()
            .filter_map(|entry| entry.faces.as_ref())
            .filter(|layer| layer.visible)
        {
            let Some(id) = layer.selected else {
                continue;
            };
            if let Some(face) = layer.placed.face(id) {
                let rings = face.rings();
                let shape = canvas::Path::new(|path| {
                    for ring in &rings {
                        let points = drawing::screen_ring(projection, ring, size);
                        let Some((first, rest)) =
                            points.split_first().filter(|_| points.len() >= 3)
                        else {
                            continue;
                        };
                        path.move_to(*first);
                        for point in rest {
                            path.line_to(*point);
                        }
                        path.close();
                    }
                });
                frame.fill(
                    &shape,
                    canvas::Fill {
                        style: canvas::Style::Solid(Color { a: 0.28, ..accent }),
                        rule: canvas::fill::Rule::EvenOdd,
                    },
                );
                // Edge by edge: a stroke of the shape would also draw the
                // edges that cutting it off at the viewport adds.
                let edges: Vec<[[f64; 3]; 2]> = rings
                    .iter()
                    .flat_map(|ring| {
                        (0..ring.len()).map(move |edge| [ring[edge], ring[(edge + 1) % ring.len()]])
                    })
                    .collect();
                stroke_edges(frame, &edges);
            } else if let Some(face) = layer.placed.cylinder(id) {
                stroke_edges(frame, &cylinder_edges(face));
            }
        }
    }
}

/// The lines that show a cylinder: its axis, the arc of its scanned part at
/// both ends and halfway, and a line along it every 30 degrees or less.
fn cylinder_edges(face: &CylinderFace) -> Vec<[[f64; 3]; 2]> {
    let length = face.length();
    let mut edges = vec![[face.axis_start, face.axis_end]];
    let strips = ((face.arc_deg / 7.5).ceil() as usize).max(12);
    let angle = |strip: usize| face.arc_deg * strip as f64 / strips as f64;
    for along in [0.0, length / 2.0, length] {
        for strip in 0..strips {
            edges.push([
                face.point(angle(strip), along),
                face.point(angle(strip + 1), along),
            ]);
        }
    }
    let lines = ((face.arc_deg / 30.0).ceil() as usize).max(1);
    for line in 0..=lines {
        let angle = face.arc_deg * line as f64 / lines as f64;
        edges.push([face.point(angle, 0.0), face.point(angle, length)]);
    }
    edges
}

/// One line of the command line per face: its number, what it is, its size
/// and how far the points lie from it.
fn face_lines(surfaces: &DetectedSurfaces) -> Vec<String> {
    let residual = |residuals: &Residuals| {
        format!(
            "rms {:.1} mm, 95% {:.1} mm, largest {:.1} mm, {} points",
            residuals.rms * 1000.0,
            residuals.p95 * 1000.0,
            residuals.max * 1000.0,
            residuals.points
        )
    };
    let planes = surfaces.planes.iter().map(|face| {
        let [x, y, z] = face.normal.map(tidy);
        let holes: usize = face.patches.iter().map(|patch| patch.holes.len()).sum();
        format!(
            "{:>4}  {:<8} {:>9.3} m2  normal {x:.3} {y:.3} {z:.3}  offset {:.3} m  {}, {}, \
             coverage {:.0}%  {}",
            face.id,
            face.class.name(),
            face.area,
            tidy(face.offset()),
            counted(face.patches.len(), "part", "parts"),
            counted(holes, "opening", "openings"),
            (face.coverage() * 100.0).floor(),
            residual(&face.residuals)
        )
    });
    let cylinders = surfaces.cylinders.iter().map(|face| {
        let [[x0, y0, z0], [x1, y1, z1]] =
            [face.axis_start, face.axis_end].map(|end| end.map(tidy));
        format!(
            "{:>4}  cylinder diameter {:.3} m, length {:.3} m, arc {:.0} degrees, seen from {}  \
             axis {x0:.3} {y0:.3} {z0:.3} to {x1:.3} {y1:.3} {z1:.3}  {}",
            face.id,
            face.diameter(),
            face.length(),
            face.arc_deg,
            if face.seen_from_inside {
                "inside"
            } else {
                "outside"
            },
            residual(&face.residuals)
        )
    });
    planes.chain(cylinders).collect()
}

/// The `--faces` mode of the command line: detect the faces of a scan file,
/// or of a box in it, and write them as JSON, OBJ, DXF, DWG or IFC. `arguments` are what
/// follows the flag. Returns the lines to print, or the exit code with the
/// line that says what is wrong; an empty line stands for the usage line.
pub(crate) fn command_line(arguments: &[OsString]) -> Result<String, (i32, String)> {
    let usage = || (2, String::new());
    let wrong = |line: &str| (2, line.to_owned());
    let [source, destination, options @ ..] = arguments else {
        return Err(usage());
    };
    let (source, destination) = (PathBuf::from(source), PathBuf::from(destination));
    let Some(format) = FaceFormat::from_path(&destination) else {
        return Err(wrong(
            "Supported faces extensions: .json, .obj, .dxf, .dwg, .ifc",
        ));
    };
    if camera_views::source_key(&source) == camera_views::source_key(&destination) {
        return Err(wrong("Choose an output path different from the input"));
    }
    if options.len() % 2 != 0 {
        return Err(usage());
    }
    let mut settings = FaceSettings::default();
    let mut section = None;
    let mut rotation = None;
    for at in (0..options.len()).step_by(2) {
        let value = option_value(options, at + 1).unwrap_or_default();
        match options[at].to_str() {
            Some("--box") => section = Some(box_limits(value).map_err(wrong)?),
            Some("--rotation") => rotation = Some(rotation_option(value).map_err(wrong)?),
            // The command line takes metres, as the local API does; the
            // block shows millimetres, and so does its sentence about the
            // range.
            Some("--distance") => {
                let metres =
                    number(value).ok_or_else(|| wrong("--distance must be a number of metres"))?;
                if !(MIN_TOLERANCE..=MAX_TOLERANCE).contains(&metres) {
                    return Err(wrong(&format!(
                        "--distance must lie between {MIN_TOLERANCE} and {MAX_TOLERANCE} metres"
                    )));
                }
                settings.distance = shown_number(metres * 1000.0);
            }
            Some("--angle") => settings.angle = value.to_owned(),
            Some("--min-area") => settings.min_area = value.to_owned(),
            Some("--cylinders") => {
                settings.cylinders = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err(wrong("--cylinders must be on or off")),
                };
            }
            _ => return Err(usage()),
        }
    }
    let section = turned_box(section, rotation).map_err(wrong)?;
    let config = settings
        .config()
        .map_err(|problem| (2, problem.english()))?;
    // The folder of the output is checked before the input is opened:
    // opening a file that is not LAS or LAZ is a full pass over it, and the
    // faces are written only when they are found. A bare file name has an
    // empty parent, which is the current folder.
    let folder = destination
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty());
    if folder.is_some_and(|folder| !folder.is_dir()) {
        return Err(wrong("The folder of the output path does not exist"));
    }

    let failed = |error: LoadError| {
        (
            1,
            format!(
                "Face detection failed: {}",
                plain_reason(&error.to_string())
            ),
        )
    };
    let cloud = crate::open_for_export(&source).map_err(failed)?;
    // An index that `--index` or the window left in the cache is used.
    // Without one a small file is read into memory and a larger one is read
    // from start to end twice.
    let index = OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())
        .ok()
        .flatten();
    let input = JobInput {
        scene: Scene {
            job: JobScene::new(
                section,
                ClassFilter {
                    ground: true,
                    vegetation: true,
                    buildings: true,
                    other: true,
                    classes: ClassVisibility::default(),
                    section: None,
                },
                vec![JobLayer::of_file(cloud, index)],
            ),
            anchor: 0,
        },
        config,
        meshes: false,
    };
    let finished = run(&input, &Control::default()).map_err(failed)?;
    let surfaces = &finished.surfaces;
    let summary = Summary::of(surfaces, finished.seconds);
    if summary.count() == 0 {
        return Err((
            1,
            format!(
                "No faces found: {} points of the region took part. Check the box and the \
                 tolerances; nothing was written",
                summary.source_points
            ),
        ));
    }
    let request = ExportRequest {
        surfaces: Arc::clone(surfaces),
        source: source.clone(),
        stale: false,
    };
    let done = write(&request, &destination, format).map_err(failed)?;
    let mut lines = vec![format!(
        "Detected {}: {}; {} edges; {} of {} points on a face -> {} ({}, {} bytes)",
        counted(summary.count(), "face", "faces"),
        summary.line(),
        summary.edges,
        summary.assigned_points,
        summary.source_points,
        destination.display(),
        done.format.title(),
        done.bytes
    )];
    lines.extend(face_lines(surfaces));
    Ok(lines.join("\n"))
}

#[cfg(test)]
impl Studio {
    /// Do the work of the running job here, as its worker thread would, and
    /// hand its end to the window: for the tests of other modules.
    pub(crate) fn finish_faces_here(&mut self) {
        let Some(job) = &self.faces.job else {
            return;
        };
        let (serial, input, control) =
            (job.serial, Arc::clone(&job.input), Arc::clone(&job.control));
        let end = FaceEnd::of(run(&input, &control));
        let _ = self.update(Message::Faces(FaceAction::Finished(serial, end)));
    }
}

#[cfg(test)]
mod tests;
