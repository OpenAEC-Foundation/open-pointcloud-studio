//! The Closed mesh tool: a surface without overlaps from the points of a
//! region, closed wherever the scan has data or a gap narrower than the hole
//! limit.
//!
//! This module holds the settings of the tool, what it says about the region
//! before a job starts, the job that reads the layers with its stages and its
//! cancel, the mapping of the result to the frame of the scan that keeps it,
//! the Properties block, the commands of the local API and the
//! `--closed-mesh` mode of the command line. The mesh itself is made by the
//! core.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use iced::widget::{button, column, container, pick_list, text};
use iced::{Element, Fill, Task};
use pointcloud_core::region_source::{resident_points, RegionSource, SourceTransform};
use pointcloud_core::surfels::{SurfelSource, MAX_VOXEL, MIN_VOXEL};
use pointcloud_core::{
    Bounds, ClosedMeshConfig, ClosedMeshReport, ClosedMeshStage, IndexConfig, IndexedPoint,
    LoadError, MeshFormat, MeshGeometry, MeshOrientation, OctreeIndex, OrientationUsed,
    OrientedBox, Point, PointCloud, MAX_CLOSED_MESH_HOLE, MAX_MESH_TRIANGLES, MAX_MESH_VERTICES,
};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};

use crate::bag_panel::plain_reason;
use crate::cloud_transform::CloudTransform;
use crate::i18n::{key, tr, tr_args};
use crate::mesh_export::{topology_text, MeasuredMesh};
use crate::open_progress::{Line, Phase};
use crate::selection::{ClassFilter, ClassVisibility, DeletionMask};
use crate::{
    camera_views, compact_count, display_name, flat_tool_style, format_count, opencad_properties,
    opencad_ribbon, themed_pick_list_style, CloudEntry, Message, Studio,
};

/// A layer without an index is read into memory for the job. Above this many
/// points that takes too much of it, and the index has to be built first.
pub(crate) const UNINDEXED_LIMIT: u64 = 5_000_000;
/// The share of its triangles a surface is taken to keep at the least when
/// it is simplified. Flat faces collapse furthest: of generated rooms without
/// furniture the finished mesh kept one triangle in forty-seven with a noise
/// of 1 mm and about one in twenty-three with 2 mm, at voxels of 2 cm. The limits
/// are checked before the pass across the blocks, when more are still there.
const LEAST_KEPT: f64 = 1.0 / 40.0;
/// What a scanned room gives over the faces of its box when nothing is
/// simplified: with a noise of 2 mm, a column and the reveals of a door and a
/// window, a generated room gave about 1.4 times the triangles of its box at
/// voxels of 5 and 6 mm.
const SURFACE_MARGIN: f64 = 1.5;
/// From this share of the surface elements, those whose side nothing told
/// are worth a word of advice.
const UNDECIDED_SHARE: f64 = 0.05;

const BUSY: &str = "A closed mesh is already being made";
const OTHER_MESH: &str = "A mesh task is already open or running";
const NO_FORMAT: &str =
    "mesh with mode closed takes an absolute .obj, .ply, .stl, .dxf, .dwg or .ifc destination";

/// How the side a face looks at is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sides {
    /// The station that measured a face where the scan knows its stations,
    /// the middle of the region elsewhere.
    Automatic,
    /// The middle of the region for every face; stations are not used.
    Centre,
    /// Upward for every face; stations are not used.
    Upward,
}

impl Sides {
    const ALL: [Self; 3] = [Self::Automatic, Self::Centre, Self::Upward];

    /// The name of the choice in a command and on the command line.
    fn name(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Centre => "centre",
            Self::Upward => "upward",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "automatic" => Some(Self::Automatic),
            "centre" | "center" => Some(Self::Centre),
            "upward" => Some(Self::Upward),
            _ => None,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Self::Automatic => key("Automatic"),
            Self::Centre => key("Towards the centre"),
            Self::Upward => key("Upward"),
        }
    }
}

/// Which layers give their points to a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layers {
    Active,
    Visible,
}

impl Layers {
    pub(crate) const ALL: [Self; 2] = [Self::Active, Self::Visible];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Visible => "visible",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            "active" => Some(Self::Active),
            "visible" => Some(Self::Visible),
            _ => None,
        }
    }

    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Active => key("Active scan"),
            Self::Visible => key("All visible scans"),
        }
    }
}

/// A line of the status bar as an answer of the local API, which starts small.
pub(crate) fn uncapitalised(reason: &str) -> String {
    let mut letters = reason.chars();
    match letters.next() {
        Some(first) => first.to_lowercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// A number as it was typed, with a comma or a point.
pub(crate) fn number(input: &str) -> Option<f64> {
    input
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// A field that is left empty, or says so, leaves its value to the job.
fn automatic(input: &str) -> bool {
    let input = input.trim();
    input.is_empty() || input.eq_ignore_ascii_case("auto")
}

/// A sentence with the values that go into it. The status bar, the command
/// line and the local API say it in English, the Properties block in the
/// language in use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Sentence {
    /// The English sentence, with a `{name}` where a value goes.
    text: &'static str,
    values: Vec<(&'static str, String)>,
}

impl Sentence {
    pub(crate) fn plain(text: &'static str) -> Self {
        Self {
            text,
            values: Vec::new(),
        }
    }

    pub(crate) fn with(text: &'static str, values: &[(&'static str, String)]) -> Self {
        Self {
            text,
            values: values.to_vec(),
        }
    }

    pub(crate) fn english(&self) -> String {
        let mut filled = self.text.to_owned();
        for (name, value) in &self.values {
            filled = filled.replace(&format!("{{{name}}}"), value);
        }
        filled
    }

    pub(crate) fn translated(&self) -> String {
        let values: Vec<(&str, &dyn fmt::Display)> = self
            .values
            .iter()
            .map(|(name, value)| (*name, value as &dyn fmt::Display))
            .collect();
        tr_args(self.text, &values)
    }
}

/// The settings of the Properties block. Numbers are kept as the text that
/// was typed, so that one is not rewritten while it is being typed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClosedMeshSettings {
    /// Edge of a voxel in metres; empty for automatic.
    voxel: String,
    /// Widest gap that is closed, in metres.
    max_hole: String,
    /// How far simplification may move the surface, in millimetres; empty
    /// for automatic and 0 for none.
    simplify: String,
    sample_percent: String,
    sides: Sides,
    layers: Layers,
}

impl Default for ClosedMeshSettings {
    fn default() -> Self {
        let config = ClosedMeshConfig::default();
        Self {
            voxel: config
                .voxel
                .map(|voxel| voxel.to_string())
                .unwrap_or_default(),
            max_hole: format!("{:.2}", config.max_hole),
            simplify: config
                .simplify_tolerance
                .map(|tolerance| (tolerance * 1000.0).to_string())
                .unwrap_or_default(),
            sample_percent: config.sample_percent.to_string(),
            sides: Sides::Automatic,
            layers: Layers::Active,
        }
    }
}

impl ClosedMeshSettings {
    fn voxel(&self) -> Result<Option<f64>, Sentence> {
        if automatic(&self.voxel) {
            return Ok(None);
        }
        number(&self.voxel).map(Some).ok_or_else(|| {
            Sentence::plain(key(
                "Voxel size must be a number of metres, or empty for automatic",
            ))
        })
    }

    /// The simplification tolerance in metres.
    fn tolerance(&self) -> Result<Option<f64>, Sentence> {
        if automatic(&self.simplify) {
            return Ok(None);
        }
        number(&self.simplify)
            .map(|millimetres| Some(millimetres / 1000.0))
            .ok_or_else(|| {
                Sentence::plain(key(
                    "Simplification must be a number of millimetres, empty for automatic or 0 for none",
                ))
            })
    }

    /// What the core is asked for, or why these settings give no mesh. The
    /// reason is a sentence, so that the Properties block says it in the
    /// language in use.
    fn config(&self) -> Result<ClosedMeshConfig, Sentence> {
        let (use_stations, orientation) = match self.sides {
            Sides::Automatic => (true, MeshOrientation::Automatic),
            // The middle the core takes is that of the region: the section
            // box cut back to where the layers have points, so a box drawn
            // wide around a room still gives a point inside the room.
            Sides::Centre => (false, MeshOrientation::Automatic),
            Sides::Upward => (false, MeshOrientation::Upward),
        };
        let config = ClosedMeshConfig {
            voxel: self.voxel()?,
            sample_percent: number(&self.sample_percent)
                .ok_or_else(|| Sentence::plain(key("Source sample must be a percentage")))?,
            max_hole: number(&self.max_hole)
                .ok_or_else(|| Sentence::plain(key("Hole limit must be a number of metres")))?,
            simplify_tolerance: self.tolerance()?,
            use_stations,
            orientation,
            max_vertices: MAX_MESH_VERTICES,
            max_triangles: MAX_MESH_TRIANGLES,
            ..ClosedMeshConfig::default()
        };
        // The limits of the three numbers are said here, in the words of the
        // core, because a refusal of the core is English only.
        if config
            .voxel
            .is_some_and(|voxel| !(MIN_VOXEL..=MAX_VOXEL).contains(&voxel))
        {
            return Err(Sentence::with(
                key("The voxel size must lie between {min} and {max} m"),
                &[
                    ("min", MIN_VOXEL.to_string()),
                    ("max", MAX_VOXEL.to_string()),
                ],
            ));
        }
        if !(0.0..=MAX_CLOSED_MESH_HOLE).contains(&config.max_hole) {
            return Err(Sentence::with(
                key("The hole limit must lie between 0 and {max} m"),
                &[("max", MAX_CLOSED_MESH_HOLE.to_string())],
            ));
        }
        if config
            .simplify_tolerance
            .is_some_and(|tolerance| !(0.0..=1.0).contains(&tolerance))
        {
            return Err(Sentence::plain(key(
                "The simplification tolerance must lie between 0 and 1 m",
            )));
        }
        // Nothing else of the config comes from the block.
        config
            .validate()
            .map_err(|_| Sentence::plain(key("A setting lies outside its limits")))?;
        Ok(config)
    }

    /// These settings with the fields a command of the local API names.
    fn with(&self, options: &ClosedMeshOptions) -> Result<Self, String> {
        let mut next = self.clone();
        if let Some(voxel) = options.voxel {
            next.voxel = voxel.map(|voxel| voxel.to_string()).unwrap_or_default();
        }
        if let Some(max_hole) = options.max_hole {
            next.max_hole = max_hole.to_string();
        }
        if let Some(simplify) = options.simplify_mm {
            next.simplify = simplify
                .map(|millimetres| millimetres.to_string())
                .unwrap_or_default();
        }
        if let Some(percent) = options.sample_percent {
            next.sample_percent = percent.to_string();
        }
        if let Some(sides) = &options.sides {
            next.sides = Sides::from_name(&sides.to_ascii_lowercase())
                .ok_or("sides must be automatic, centre or upward")?;
        }
        if let Some(layers) = &options.layers {
            next.layers = Layers::from_name(&layers.to_ascii_lowercase())
                .ok_or("layers must be active or visible")?;
        }
        next.config()
            .map_err(|problem| uncapitalised(&problem.english()))?;
        Ok(next)
    }

    /// The settings as `status` of the local API reports them: `null` for a
    /// value the job chooses, and for a field that holds no number.
    fn value(&self) -> Value {
        json!({
            "voxel": self.voxel().ok().flatten(),
            "max_hole": number(&self.max_hole),
            "simplify_mm": self.tolerance().ok().flatten().map(|metres| metres * 1000.0),
            "sample_percent": number(&self.sample_percent),
            "sides": self.sides.name(),
            "layers": self.layers.name(),
        })
    }
}

/// A field that tells "left out" from `null`: left out keeps what the block
/// has, and `null` leaves the value to the job.
fn given<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<f64>>, D::Error> {
    Option::<f64>::deserialize(deserializer).map(Some)
}

/// The settings a command of the local API may name. A field that is left
/// out keeps what the Properties block has.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ClosedMeshOptions {
    /// Deterministic percentage of source points used for reconstruction.
    pub sample_percent: Option<f64>,
    /// Edge of a voxel in metres, from 0.005 to 0.5; `null` for automatic.
    #[serde(default, deserialize_with = "given")]
    pub voxel: Option<Option<f64>>,
    /// Gaps up to this wide are closed, in metres, from 0 to 3.2.
    pub max_hole: Option<f64>,
    /// How far simplification may move the surface, in millimetres, from 0
    /// (none) to 1000; `null` for automatic.
    #[serde(default, deserialize_with = "given")]
    pub simplify_mm: Option<Option<f64>>,
    /// `automatic`, `centre` or `upward`.
    pub sides: Option<String>,
    /// `active` or `visible`.
    pub layers: Option<String>,
}

/// One layer as a job reads it.
struct SceneLayer {
    cloud: Arc<PointCloud>,
    index: Option<Arc<OctreeIndex>>,
    transform: CloudTransform,
    deleted: Option<Arc<DeletionMask>>,
}

/// What of the window a job is made from: the layers where they stand, their
/// deleted points, the classes shown, the section box, and the scan that
/// gets the mesh.
struct Scene {
    /// The section box, turned or not. The core is given the box around it
    /// and the filter leaves out what lies in its corners.
    section: Option<OrientedBox>,
    filter: ClassFilter,
    layers: Vec<SceneLayer>,
    /// The scan the mesh is attached to, and where it stood when the job
    /// started: the mesh is kept in the frame of that scan.
    target: Arc<PointCloud>,
    target_transform: CloudTransform,
}

/// Everything a job was started with.
pub(crate) struct JobInput {
    scene: Scene,
    config: ClosedMeshConfig,
    sides: Sides,
    /// Where the mesh is also written, as the scene shows it.
    destination: Option<(PathBuf, MeshFormat)>,
}

/// What a job is doing. The stages of the core come after the reads that
/// only some layers need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Stage {
    /// Reading a source once to learn which station measured each point.
    Stations,
    /// Reading a layer without an index into memory.
    Reading,
    Planning,
    Reconstructing,
    Simplifying,
    Measuring,
    Writing,
}

impl Stage {
    const ALL: [Self; 7] = [
        Self::Stations,
        Self::Reading,
        Self::Planning,
        Self::Reconstructing,
        Self::Simplifying,
        Self::Measuring,
        Self::Writing,
    ];

    fn of(stage: ClosedMeshStage) -> Self {
        match stage {
            ClosedMeshStage::Planning => Self::Planning,
            ClosedMeshStage::Reconstructing => Self::Reconstructing,
            ClosedMeshStage::Simplifying => Self::Simplifying,
            ClosedMeshStage::Measuring => Self::Measuring,
        }
    }

    /// The name of the stage in a job of the local API.
    fn name(self) -> &'static str {
        match self {
            Self::Stations => "stations",
            Self::Reading => "reading",
            Self::Planning => "planning",
            Self::Reconstructing => "reconstructing",
            Self::Simplifying => "simplifying",
            Self::Measuring => "measuring",
            Self::Writing => "writing",
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
            Stage::Reading => format!("reading a scan without an index, {}", of_points()),
            Stage::Planning => "finding the blocks that hold points".to_owned(),
            Stage::Reconstructing if self.total > 0 && self.done >= self.total => {
                "joining the blocks".to_owned()
            }
            Stage::Reconstructing => format!("block {} of {}", self.done, self.total),
            Stage::Simplifying => match self.fraction() {
                Some(fraction) => format!(
                    "simplifying across the blocks, {:.0}%",
                    (fraction * 100.0).floor()
                ),
                None => "simplifying across the blocks".to_owned(),
            },
            Stage::Measuring => "measuring the distance between points and mesh".to_owned(),
            Stage::Writing => "writing the file".to_owned(),
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

/// The cloud of a layer, and the same cloud with the station of every point.
pub(crate) type Learned = (Arc<PointCloud>, Arc<PointCloud>);

impl Control {
    /// Keep how far the job is, and stop it when that was asked.
    fn report(&self, stage: Stage, done: u64, total: u64) -> Result<(), LoadError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        self.stage.store(stage as u8, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        Ok(())
    }

    /// Keep a cloud whose stations were found for the cloud of a layer.
    fn learn(&self, of: &Arc<PointCloud>, found: &Arc<PointCloud>) {
        self.learned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((Arc::clone(of), Arc::clone(found)));
    }

    /// What this job found out so far; the list is empty afterwards.
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

/// Whether a job has to read the source of a cloud once to learn which
/// station measured each point: the cloud has stations and does not know.
pub(crate) fn lacks_scan_ranges(cloud: &PointCloud) -> bool {
    !cloud.scan_ranges_known() && !cloud.scan_poses.is_empty()
}

/// The cloud of a layer with the station of every point, read from its
/// source in one pass; nothing for a cloud that knows them already or has
/// no stations. `progress` gets the points read and their total, and stops
/// the pass by returning an error. For a layer with an index the answer is
/// written to the index cache as well.
pub(crate) fn with_scan_ranges(
    cloud: &Arc<PointCloud>,
    indexed: bool,
    progress: &mut dyn FnMut(u64, u64) -> Result<(), LoadError>,
) -> Result<Option<Arc<PointCloud>>, LoadError> {
    if !lacks_scan_ranges(cloud) {
        return Ok(None);
    }
    let total = cloud.total_points;
    let mut found = PointCloud::clone(cloud);
    found.read_scan_ranges(|read| progress(read, total))?;
    if indexed {
        // Failing to keep the answer costs the same pass next time.
        let _ = OctreeIndex::open_cached_if_present(&found, IndexConfig::default());
    }
    Ok(Some(Arc::new(found)))
}

/// The mesh in the frame of its scan: the inverse of
/// `mesh_export::in_scene`, so that the scene and a saved file show the mesh
/// where the job made it, and a later move or scale of the scan takes the
/// mesh along. `None` for a scan whose scale has a zero.
fn to_source(mut mesh: MeshGeometry, transform: CloudTransform) -> Option<MeshGeometry> {
    if transform.is_identity() {
        return Some(mesh);
    }
    for vertex in &mut mesh.vertices {
        *vertex = transform.source_xyz(*vertex)?;
    }
    // A scale with an odd number of negative factors mirrors the scan. The
    // corner order is reversed here and again on the way to the scene, so
    // the side that faced the scanner still does.
    let mirrored = transform
        .scale
        .iter()
        .filter(|factor| **factor < 0.0)
        .count()
        % 2
        == 1;
    if mirrored {
        for triangle in &mut mesh.triangles {
            triangle.swap(1, 2);
        }
    }
    if let Some(normals) = &mut mesh.normals {
        for normal in normals {
            let scaled: [f64; 3] =
                std::array::from_fn(|axis| f64::from(normal[axis]) * transform.scale[axis]);
            let length = scaled.iter().map(|value| value * value).sum::<f64>().sqrt();
            if length.is_finite() && length > f64::EPSILON {
                *normal = scaled.map(|value| (value / length) as f32);
            }
        }
    }
    Some(mesh)
}

/// What a finished job hands back.
#[derive(Debug, Clone)]
pub struct Finished {
    /// The mesh in the frame of the scan that gets it.
    mesh: MeasuredMesh,
    report: ClosedMeshReport,
    /// The origin of an STL file that was written far from zero.
    origin: Option<[f64; 3]>,
}

const FLAT_TARGET: &str = "the scan that gets the mesh has a scale of zero";

/// Read the layers, make the mesh, write it where that was asked and map it
/// to the frame of its scan. This runs on a worker thread, or in the process
/// of the command line.
fn run(input: &JobInput, control: &Control) -> Result<Finished, LoadError> {
    let scene = &input.scene;
    // The mesh ends in the frame of this scan. Without a way back the whole
    // job, and the file it writes, would be for nothing.
    if scene.target_transform.source_xyz([0.0; 3]).is_none() {
        return Err(LoadError::InvalidData(FLAT_TARGET.into()));
    }
    // An index is read without a look at the file it was built from, so
    // that the file is still the one that was opened is checked here.
    for layer in scene.layers.iter().filter(|layer| layer.index.is_some()) {
        layer.cloud.validate_source()?;
    }
    // An index cache written before scans were recorded per point gives a
    // cloud with stations that does not know which of them measured what.
    // One pass over its source tells, and the cache keeps the answer.
    let mut clouds: Vec<Arc<PointCloud>> = Vec::with_capacity(scene.layers.len());
    for layer in &scene.layers {
        let cloud = &layer.cloud;
        let found = match input.config.use_stations {
            true => with_scan_ranges(cloud, layer.index.is_some(), &mut |read, total| {
                control.report(Stage::Stations, read, total)
            })?,
            false => None,
        };
        clouds.push(match found {
            Some(found) => {
                control.learn(cloud, &found);
                found
            }
            None => Arc::clone(cloud),
        });
    }
    let mut resident: Vec<Option<Vec<IndexedPoint>>> = Vec::with_capacity(scene.layers.len());
    for layer in &scene.layers {
        resident.push(match layer.index {
            Some(_) => None,
            None => Some(resident_points(&layer.cloud, &mut |progress| {
                control.report(Stage::Reading, progress.read, progress.total)
            })?),
        });
    }
    let sources: Vec<SurfelSource<'_>> = scene
        .layers
        .iter()
        .zip(&clouds)
        .zip(&resident)
        .map(|((layer, cloud), resident)| {
            let transform = SourceTransform {
                scale: layer.transform.scale,
                offset: layer.transform.offset,
            };
            match resident {
                Some(points) => {
                    SurfelSource::of_cloud(RegionSource::resident(points, transform), cloud)
                }
                None => SurfelSource::new(cloud, layer.index.as_deref(), transform),
            }
        })
        .collect();
    let filter = scene.filter;
    let accept = |position: usize, ordinal: u64, point: &Point| {
        scene.layers[position]
            .deleted
            .as_ref()
            .is_none_or(|mask| !mask.contains(ordinal))
            && filter.accepts(point)
            && scene
                .section
                .is_none_or(|section| !section.is_turned() || section.contains(point.xyz))
    };
    let (mesh, report) = pointcloud_core::mesh_closed(
        &sources,
        scene.section.map(|section| section.aabb()),
        &accept,
        &input.config,
        &mut |step| control.report(Stage::of(step.stage), step.completed, step.total),
    )?;
    let origin = match &input.destination {
        Some((path, format)) => {
            control.report(Stage::Writing, 0, 0)?;
            pointcloud_core::write_mesh(&mesh, path, *format, &[])?.origin
        }
        None => None,
    };
    let mesh = to_source(mesh, scene.target_transform)
        .ok_or_else(|| LoadError::InvalidData(FLAT_TARGET.into()))?;
    Ok(Finished {
        mesh: MeasuredMesh {
            mesh: Arc::new(mesh),
            topology: report.topology,
        },
        report,
        origin,
    })
}

/// How a job ended, as the worker tells the window.
#[derive(Debug, Clone)]
pub enum ClosedMeshEnd {
    Done(Arc<Finished>),
    Cancelled,
    Failed(String),
}

impl ClosedMeshEnd {
    fn of(result: Result<Finished, LoadError>) -> Self {
        match result {
            Ok(finished) => Self::Done(Arc::new(finished)),
            Err(LoadError::Cancelled) => Self::Cancelled,
            Err(error) => Self::Failed(plain_reason(&error.to_string()).to_owned()),
        }
    }
}

/// A closed mesh that is being made.
pub(crate) struct ClosedMeshJob {
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

impl ClosedMeshJob {
    fn cancelling(&self) -> bool {
        self.control.cancelled.load(Ordering::Relaxed)
    }

    /// The stages this job goes through, in their order.
    fn stages(&self) -> Vec<Stage> {
        let input = &self.input;
        let layers = &input.scene.layers;
        Stage::ALL
            .into_iter()
            .filter(|stage| match stage {
                Stage::Stations => {
                    input.config.use_stations
                        && layers.iter().any(|layer| lacks_scan_ranges(&layer.cloud))
                }
                Stage::Reading => layers.iter().any(|layer| layer.index.is_none()),
                Stage::Simplifying => input.config.simplify_tolerance != Some(0.0),
                Stage::Writing => input.destination.is_some(),
                _ => true,
            })
            .collect()
    }

    /// The line of the status bar while the job runs.
    fn status_text(&self) -> String {
        if self.cancelling() {
            return "Cancelling the closed mesh…".into();
        }
        format!("Closed mesh: {}…", self.control.snapshot().text())
    }

    /// The job as `status` and `job` of the local API report it.
    fn progress_value(&self) -> Value {
        let step = self.control.snapshot();
        json!({
            "state": "running",
            "operation": "mesh",
            "mode": "closed",
            "path": self.input.destination.as_ref().map(|(path, _)| path),
            "stage": step.stage.name(),
            "completed": step.done,
            "total": step.total,
            "fraction": step.fraction(),
            "cancel_requested": self.cancelling(),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }
}

fn millimetres(metres: f64) -> String {
    format!("{:.1} mm", metres * 1000.0)
}

/// Where the sides of a mesh came from, in words.
fn sides_text(report: &ClosedMeshReport, sides: Sides) -> &'static str {
    match (report.orientation, sides) {
        (OrientationUsed::Stations, _) => "from the stations",
        (OrientationUsed::Mixed, _) => {
            "from the stations where known, towards the centre elsewhere"
        }
        (OrientationUsed::Fallback, Sides::Upward) => "upward",
        (OrientationUsed::Fallback, _) => "towards the centre of the region",
    }
}

/// The surface elements that took their side from the fallback. With
/// Automatic these are the ones no station was known for; with the other
/// choices stations were not asked for and this is every element, which says
/// nothing about the scan, so it is only named for Automatic.
fn without_station(report: &ClosedMeshReport) -> u64 {
    report.surfels.saturating_sub(report.surfels_by_station)
}

/// What to do about a result whose figures say something is off; nothing
/// when they do not. `count` writes a number: grouped in the Properties
/// block, plain for the command line and the local API, as in `summary`.
fn advice_sentences(
    report: &ClosedMeshReport,
    sides: Sides,
    count: &dyn Fn(u64) -> String,
) -> Vec<Sentence> {
    let mut lines = Vec::new();
    let all = count(report.surfels);
    let undecided = report.surfels_by_default;
    if undecided > 0 && undecided as f64 >= report.surfels as f64 * UNDECIDED_SHARE {
        let values = [("count", count(undecided)), ("all", all)];
        lines.push(match sides {
            Sides::Upward => Sentence::with(
                key("{count} of {all} surface elements are upright: Upward gives them one fixed side, the same for both faces of a wall, so walls can face the wrong way and do not close against floors. Use Automatic or Towards the centre for a room."),
                &values,
            ),
            // Stations were not asked for, whatever the scans know.
            Sides::Centre => Sentence::with(
                key("{count} of {all} surface elements lie edge on to the centre of the region: they face up, or one fixed side when upright, which may be the wrong side and can tear a face. Stations were not used: choose Automatic when the scans know their stations, or put the section box around one room, so that its centre lies inside the room and away from the planes of its floor, ceiling and walls."),
                &values,
            ),
            Sides::Automatic => Sentence::with(
                key("{count} of {all} surface elements had no station and lie edge on to the centre of the region: they face up, or one fixed side when upright, which may be the wrong side and can tear a face. Put the section box around one room, so that its centre lies inside the room and away from the planes of its floor, ceiling and walls, or use scans that know their stations."),
                &values,
            ),
        });
    } else if sides == Sides::Automatic && report.orientation == OrientationUsed::Mixed {
        lines.push(Sentence::with(
            key("{count} of {all} surface elements had no station and face the centre of the region, or the side of the stationed surface beside them."),
            &[("count", count(without_station(report))), ("all", all)],
        ));
    } else if sides == Sides::Automatic && report.orientation == OrientationUsed::Fallback {
        lines.push(Sentence::plain(key(
            "No station is known for these points, so every face looks at the centre of the region: right for a room inside its section box, wrong for an object seen from around.",
        )));
    }
    let edges = report.topology.non_manifold_edges;
    if edges > 0 {
        lines.push(Sentence::with(
            if edges == 1 {
                key("{count} edge has more than two triangles: two faces lie closer together than two voxels, or a face came out torn. A smaller voxel keeps thin objects apart.")
            } else {
                key("{count} edges have more than two triangles: two faces lie closer together than two voxels, or a face came out torn. A smaller voxel keeps thin objects apart.")
            },
            &[("count", count(edges))],
        ));
    }
    let faults = report.seam_faults;
    if faults > 0 {
        lines.push(Sentence::with(
            if faults == 1 {
                key("{count} triangle was left out where two blocks did not agree; please report this.")
            } else {
                key("{count} triangles were left out where two blocks did not agree; please report this.")
            },
            &[("count", count(faults))],
        ));
    }
    lines
}

/// The advice in English with plain numbers, for the status of a job and the
/// command line.
fn advice(report: &ClosedMeshReport, sides: Sides) -> Option<String> {
    let lines: Vec<String> = advice_sentences(report, sides, &|count| count.to_string())
        .iter()
        .map(Sentence::english)
        .collect();
    (!lines.is_empty()).then(|| lines.join(" "))
}

/// The figures of a finished job as one line.
fn summary(report: &ClosedMeshReport, sides: Sides, count: &dyn Fn(u64) -> String) -> String {
    format!(
        "{} vertices, {} triangles; deviation mean {}, 95% {}, largest {}; {}; voxel {:.0} mm; \
         sides {}",
        count(report.vertices as u64),
        count(report.triangles as u64),
        millimetres(report.deviation.mean),
        millimetres(report.deviation.p95),
        millimetres(report.deviation.max),
        topology_text(report.topology),
        report.voxel * 1000.0,
        sides_text(report, sides)
    )
}

/// The figures of a finished job for the local API. Lengths are metres.
fn report_value(report: &ClosedMeshReport, sides: Sides) -> Value {
    json!({
        "vertices": report.vertices,
        "triangles": report.triangles,
        "open_edges": report.topology.open_edges,
        "components": report.topology.components,
        "non_manifold_edges": report.topology.non_manifold_edges,
        "deviation_mean": report.deviation.mean,
        "deviation_p95": report.deviation.p95,
        "deviation_max": report.deviation.max,
        "deviation_samples": report.deviation.samples,
        "voxel": report.voxel,
        "max_hole": report.max_hole,
        "simplify_tolerance": report.simplify_tolerance,
        "region": {"min": report.region.min, "max": report.region.max},
        "points": report.points,
        "blocks": report.tiles,
        "triangles_extracted": report.triangles_extracted,
        "surfels": report.surfels,
        "surfels_by_station": report.surfels_by_station,
        "surfels_without_station": without_station(report),
        "surfels_undecided": report.surfels_by_default,
        "sides": match report.orientation {
            OrientationUsed::Stations => "stations",
            OrientationUsed::Mixed => "mixed",
            OrientationUsed::Fallback => "fallback",
        },
        "advice": advice(report, sides),
        "seconds": report.timings.total.as_secs_f64(),
    })
}

/// A mesh file a job wrote beside showing the mesh.
#[derive(Debug, Clone, PartialEq)]
struct Written {
    path: PathBuf,
    format: MeshFormat,
    origin: Option<[f64; 3]>,
}

/// How the last job ended, for the Properties block and the local API.
#[derive(Debug, Clone, PartialEq)]
enum Last {
    Done {
        /// Boxed, because the report is many times the size of the other
        /// ways a job can end.
        report: Box<ClosedMeshReport>,
        sides: Sides,
        written: Option<Written>,
        /// The file name of the scan that got the mesh, or nothing when
        /// that scan was closed while the job ran.
        shown_on: Option<String>,
    },
    Cancelled,
    Failed(String),
}

impl Last {
    /// The finished job as `job` and `status` of the local API report it.
    fn value(&self) -> Value {
        match self {
            Self::Done {
                report,
                sides,
                written,
                shown_on,
            } => {
                let mut value = report_value(report, *sides);
                value["state"] = "complete".into();
                value["operation"] = "mesh".into();
                value["mode"] = "closed".into();
                value["shown"] = shown_on.is_some().into();
                value["path"] = json!(written.as_ref().map(|written| &written.path));
                value["format"] = json!(written.as_ref().map(|written| written.format.extension()));
                value["origin"] = json!(written.as_ref().and_then(|written| written.origin));
                value
            }
            Self::Cancelled => json!({
                "state": "cancelled",
                "operation": "mesh",
                "mode": "closed",
            }),
            Self::Failed(error) => json!({
                "state": "failed",
                "operation": "mesh",
                "mode": "closed",
                "error": error,
            }),
        }
    }

    /// The line of the status bar when the job has ended.
    fn status(&self) -> String {
        match self {
            Self::Done {
                report,
                sides,
                written,
                shown_on,
            } => {
                let mut line = match shown_on {
                    Some(name) => format!("Closed mesh shown as the mesh of {name}: "),
                    None => "Closed mesh ready, but its scan was closed and nothing is shown: "
                        .to_owned(),
                };
                line.push_str(&summary(report, *sides, &|count| format_count(count)));
                if let Some(written) = written {
                    line.push_str(&format!(
                        "; written as {} to {}",
                        written.format.label(),
                        written.path.display()
                    ));
                }
                line
            }
            Self::Cancelled => {
                "Closed mesh cancelled; the mesh of the scan and an existing file are left as \
                 they were"
                    .into()
            }
            Self::Failed(error) => format!("Closed mesh failed: {error}"),
        }
    }
}

/// What the Closed mesh tool holds: whether its block is open, its settings,
/// a job under way and how the last job ended.
#[derive(Default)]
pub(crate) struct ClosedMeshTool {
    open: bool,
    settings: ClosedMeshSettings,
    job: Option<ClosedMeshJob>,
    next_serial: u64,
    last: Option<Last>,
    /// Clouds whose stations per point a job found out, by the cloud of the
    /// layer they stand for, so that the next job does not read the source
    /// again. An entry goes when its layer is closed.
    stations: Vec<(Weak<PointCloud>, Arc<PointCloud>)>,
}

impl ClosedMeshTool {
    pub(crate) fn is_running(&self) -> bool {
        self.job.is_some()
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

    /// The cloud a job reads the stations of a layer from. The Detect faces
    /// tool asks here too, and keeps what its jobs find with `keep_stations`.
    pub(crate) fn stationed(&self, cloud: &Arc<PointCloud>) -> Arc<PointCloud> {
        self.stations
            .iter()
            .find(|(of, _)| of.upgrade().is_some_and(|of| Arc::ptr_eq(&of, cloud)))
            .map_or_else(|| Arc::clone(cloud), |(_, found)| Arc::clone(found))
    }

    /// Keep the clouds whose stations per point a job found out, and forget
    /// those of layers that were closed.
    pub(crate) fn keep_stations(&mut self, learned: Vec<Learned>) {
        self.stations.retain(|(of, _)| of.strong_count() > 0);
        for (of, found) in learned {
            self.stations.push((Arc::downgrade(&of), found));
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
            phase: Phase::ClosedMesh,
            title: if cancelling {
                "Cancelling…".to_owned()
            } else {
                "Closed mesh".to_owned()
            },
            detail: format!("Step {place} of {}  ·  {}", stages.len(), step.text()),
            fraction: step.fraction(),
            timed: true,
            cancel: (!cancelling).then_some(Message::ClosedMesh(ClosedMeshAction::Cancel)),
        })
    }

    /// The tool as `status` of the local API reports it.
    pub(crate) fn value(&self) -> Value {
        json!({
            "settings": self.settings.value(),
            "job": self.job.as_ref().map(ClosedMeshJob::progress_value),
            "last": self.last.as_ref().map(Last::value),
        })
    }
}

/// Everything the Closed mesh tool reacts to.
#[derive(Debug, Clone)]
pub enum ClosedMeshAction {
    /// Open the block of the tool in Properties, or close it.
    Toggle,
    Voxel(String),
    MaxHole(String),
    Simplify(String),
    SamplePercent(String),
    Sides(Sides),
    Layers(Layers),
    Start,
    Poll,
    Cancel,
    Finished(u64, ClosedMeshEnd),
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
    /// A layer is too large to be read into memory; the name of its file.
    NeedsIndex(String),
    /// The section box and the layers have nothing in common.
    Outside,
    /// With every visible scan asked for, the active layer holds 3D BAG
    /// buildings: the mesh of the scans would take the place of the
    /// buildings and carry their credit. The name of its file.
    BagTarget(String),
    /// The active scan has a scale of zero, so a mesh has no place in its
    /// frame. The name of its file.
    FlatTarget(String),
}

impl Refusal {
    fn sentence(&self) -> Sentence {
        let named = |text, name: &String| Sentence::with(text, &[("name", name.clone())]);
        match self {
            Self::NoActive => Sentence::plain(key(
                "Select a scan first: the closed mesh becomes its mesh",
            )),
            Self::NoLayer => Sentence::plain(key("Show at least one scan to mesh")),
            Self::NoPoints(name) => named(key("{name} has no points to mesh"), name),
            Self::Loading(name) => named(
                key("{name} is still loading; wait for it or hide it before meshing"),
                name,
            ),
            Self::NeedsIndex(name) => Sentence::with(
                key("{name} has more than {limit} points and no index: build the index first (INDEX > Build index)"),
                &[
                    ("name", name.clone()),
                    ("limit", format_count(UNINDEXED_LIMIT)),
                ],
            ),
            Self::Outside => Sentence::plain(key(
                "The section box holds no part of the scans to mesh",
            )),
            Self::BagTarget(name) => named(
                key("{name} holds 3D BAG buildings and cannot take the mesh of the scans: select the scan that gets the mesh"),
                name,
            ),
            Self::FlatTarget(name) => named(
                key("{name} has a scale of zero, so a mesh cannot be kept with it: give it another scale first"),
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
            Self::NoLayer => "no visible point cloud to mesh".into(),
            Self::NoPoints(name) => format!("a point cloud has no points to mesh: {name}"),
            Self::Loading(name) => format!("a point cloud is still loading: {name}"),
            Self::NeedsIndex(name) => format!(
                "a point cloud of more than {UNINDEXED_LIMIT} points has no index, build it \
                 first: {name}"
            ),
            Self::Outside => "the section box holds no part of the point clouds to mesh".into(),
            Self::BagTarget(name) => format!(
                "the active layer holds 3D BAG buildings and cannot take a mesh of the visible \
                 point clouds: {name}"
            ),
            Self::FlatTarget(name) => format!("{FLAT_TARGET}: {name}"),
        }
    }
}

/// Why the block cannot say what a job would mesh.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Problem {
    /// A setting holds no number or one outside its limits; the sentence
    /// says which.
    Setting(Sentence),
    Refused(Refusal),
}

/// Whether a region of this size is expected to fit the limits of a mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    Fits,
    /// Without simplification: under the limit for the faces of the box
    /// alone, but near enough that the real surface may pass it.
    Close,
    /// Fits only when simplification takes most of the triangles away.
    Doubtful,
    /// Stops at the limit whatever the surface is like.
    TooLarge,
}

/// What a job would mesh, as the block says it before the job starts.
#[derive(Debug, Clone, Copy, PartialEq)]
struct RegionInfo {
    /// The section box cut back to the layers, or the box around the layers.
    bounds: Bounds,
    /// Whether the section box limits the region.
    boxed: bool,
    layers: usize,
    voxel: f64,
    /// Triangles before simplification, for the six faces of the region.
    triangles: u64,
    fit: Fit,
}

/// The triangles the faces of a box give before simplification: two per
/// voxel face of surface. A room with furniture and inner walls has more
/// surface than the box around it, so this is the least to expect.
fn box_triangles(bounds: Bounds, voxel: f64) -> u64 {
    let [x, y, z]: [f64; 3] = std::array::from_fn(|axis| bounds.max[axis] - bounds.min[axis]);
    let area = 2.0 * (x * y + y * z + z * x);
    (2.0 * area / (voxel * voxel)).round() as u64
}

fn fit(triangles: u64, config: &ClosedMeshConfig) -> Fit {
    // A vertex is shared by about six triangles and a triangle has three.
    let most = config.max_triangles.min(config.max_vertices * 2) as f64;
    let triangles = triangles as f64;
    if config.simplify_tolerance == Some(0.0) {
        // Every triangle stays, and a scanned surface has more of them than
        // the faces of its box.
        return if triangles > most {
            Fit::TooLarge
        } else if triangles * SURFACE_MARGIN > most {
            Fit::Close
        } else {
            Fit::Fits
        };
    }
    if triangles * LEAST_KEPT > most {
        Fit::TooLarge
    } else if triangles > most {
        Fit::Doubtful
    } else {
        Fit::Fits
    }
}

/// What a job would mesh and how large that is, as the block says it.
fn region_note(region: &RegionInfo, layers: Layers) -> String {
    let size = {
        let [x, y, z]: [f64; 3] =
            std::array::from_fn(|axis| region.bounds.max[axis] - region.bounds.min[axis]);
        format!("{x:.1} × {y:.1} × {z:.1}")
    };
    let count = region.layers;
    match (region.boxed, layers) {
        (true, Layers::Active) => tr_args(
            "Meshes the active scan inside the section box: {size} m.",
            &[("size", &size)],
        ),
        (true, Layers::Visible) => tr_args(
            "Meshes the visible scans ({count}) inside the section box: {size} m.",
            &[("count", &count), ("size", &size)],
        ),
        (false, Layers::Active) => tr_args(
            "Meshes all of the active scan: {size} m. Switch on the section box to mesh a part of it.",
            &[("size", &size)],
        ),
        (false, Layers::Visible) => tr_args(
            "Meshes all of the visible scans ({count}): {size} m. Switch on the section box to mesh a part of them.",
            &[("count", &count), ("size", &size)],
        ),
    }
}

/// The warning of the block for a region that may not fit the limits of a
/// mesh; nothing for one that fits.
fn fit_warning(fit: Fit) -> Option<String> {
    let vertices = format_count(MAX_MESH_VERTICES);
    let triangles = format_count(MAX_MESH_TRIANGLES);
    let values: [(&str, &dyn fmt::Display); 2] =
        [("vertices", &vertices), ("triangles", &triangles)];
    match fit {
        Fit::Fits => None,
        Fit::Close => Some(tr_args(
            "A mesh holds at most {vertices} vertices and {triangles} triangles. Without simplification this region comes close to that: uneven surfaces and anything standing in it add triangles, and the job stops when they pass the limit. A larger voxel, a smaller section box or simplification leaves room.",
            &values,
        )),
        Fit::Doubtful => Some(tr_args(
            "A mesh holds at most {vertices} vertices and {triangles} triangles. This region fits only when simplification takes most of its triangles away, as it does on flat walls and floors; otherwise the job stops. A larger voxel or a smaller section box makes it fit.",
            &values,
        )),
        Fit::TooLarge => Some(tr_args(
            "A mesh holds at most {vertices} vertices and {triangles} triangles. This region gives more: the job will stop. Use a larger voxel or a smaller section box.",
            &values,
        )),
    }
}

/// What a running job is doing, as the block says it.
fn stage_line(step: Step) -> String {
    match step.stage {
        Stage::Stations => tr("Finding the stations of the points…").to_owned(),
        Stage::Reading => tr("Reading a scan without an index…").to_owned(),
        Stage::Planning => tr("Finding the blocks that hold points…").to_owned(),
        Stage::Reconstructing => tr_args(
            "Meshing block {done} of {total}…",
            &[("done", &step.done), ("total", &step.total)],
        ),
        Stage::Simplifying => tr("Simplifying…").to_owned(),
        Stage::Measuring => tr("Measuring the result…").to_owned(),
        Stage::Writing => tr("Writing the file…").to_owned(),
    }
}

/// A value of a choice list with the English text that names it; the list
/// shows the text in the language in use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Choice<T> {
    value: T,
    text: &'static str,
}

impl<T> fmt::Display for Choice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(tr(self.text))
    }
}

/// A list that chooses one of the values of a setting.
pub(crate) fn choice_list<'a, T, const N: usize>(
    all: [T; N],
    current: T,
    name: fn(T) -> &'static str,
    message: fn(T) -> Message,
) -> Element<'a, Message>
where
    T: Copy + PartialEq + 'static,
{
    let choice = move |value: T| Choice {
        value,
        text: name(value),
    };
    pick_list(all.map(choice), Some(choice(current)), move |chosen| {
        message(chosen.value)
    })
    .text_size(11)
    .padding([2, 4])
    .width(Fill)
    .style(themed_pick_list_style)
    .into()
}

impl Studio {
    /// The layers a job reads and the scan that gets the mesh, or why there
    /// is nothing to mesh.
    fn closed_mesh_scene(&self, layers: Layers) -> Result<Scene, Refusal> {
        let active = self
            .active
            .and_then(|index| self.clouds.get(index))
            .ok_or(Refusal::NoActive)?;
        let name = |cloud: &PointCloud| display_name(&cloud.path).to_owned();
        // The mesh is kept in the frame of the active scan. Without a way
        // back to that frame the job would run for nothing.
        if active.transform.source_xyz([0.0; 3]).is_none() {
            return Err(Refusal::FlatTarget(name(&active.cloud)));
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
                // out, and their layer does not get a mesh made of scans,
                // which an export would write with the credit of 3D BAG.
                if active.bag_source {
                    return Err(Refusal::BagTarget(name(&active.cloud)));
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
                // point. Left out here it is not read into memory, cannot
                // refuse the job, and does not widen the region the voxel
                // and the centre are taken from.
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
                return Err(Refusal::Loading(name(&entry.cloud)));
            }
            if entry.cloud.total_points == 0 {
                return Err(Refusal::NoPoints(name(&entry.cloud)));
            }
            if entry.index.is_none() && entry.cloud.total_points > UNINDEXED_LIMIT {
                return Err(Refusal::NeedsIndex(name(&entry.cloud)));
            }
        }
        if !taken.iter().any(|entry| reaches(entry)) {
            return Err(Refusal::Outside);
        }
        Ok(Scene {
            section,
            // The region of the job is the section box already.
            filter: ClassFilter {
                section: None,
                ..self.mesh_filter()
            },
            layers: taken
                .into_iter()
                .map(|entry| SceneLayer {
                    cloud: self.closed_mesh.stationed(&entry.cloud),
                    index: entry.index.as_ref().map(Arc::clone),
                    transform: entry.transform,
                    deleted: entry.deleted.as_ref().map(Arc::clone),
                })
                .collect(),
            target: Arc::clone(&active.cloud),
            target_transform: active.transform,
        })
    }

    /// What a job with these settings would mesh.
    fn closed_mesh_region(&self) -> Result<RegionInfo, Problem> {
        let settings = &self.closed_mesh.settings;
        let scene = self
            .closed_mesh_scene(settings.layers)
            .map_err(Problem::Refused)?;
        let config = settings.config().map_err(Problem::Setting)?;
        let mut data: Option<Bounds> = None;
        for layer in &scene.layers {
            let bounds = layer.transform.bounds(layer.cloud.bounds);
            data = Some(data.map_or(bounds, |all| Bounds {
                min: std::array::from_fn(|axis| all.min[axis].min(bounds.min[axis])),
                max: std::array::from_fn(|axis| all.max[axis].max(bounds.max[axis])),
            }));
        }
        let data = data.ok_or(Problem::Refused(Refusal::NoLayer))?;
        // As the core does: only the part of the box that can hold points.
        let bounds = match scene.section.map(|section| section.aabb()) {
            Some(section) => Bounds {
                min: std::array::from_fn(|axis| section.min[axis].max(data.min[axis])),
                max: std::array::from_fn(|axis| section.max[axis].min(data.max[axis])),
            },
            None => data,
        };
        let voxel = config.voxel_for(bounds);
        let triangles = box_triangles(bounds, voxel);
        Ok(RegionInfo {
            bounds,
            boxed: scene.section.is_some(),
            layers: scene.layers.len(),
            voxel,
            triangles,
            fit: fit(triangles, &config),
        })
    }

    /// What a job needs from the window, or why it cannot start.
    fn closed_mesh_start(&self) -> Result<(Scene, ClosedMeshConfig), String> {
        if self.closed_mesh.is_running() {
            return Err(BUSY.into());
        }
        if self.mesh_job.is_some() || self.mesh_dialog_pending {
            return Err(OTHER_MESH.into());
        }
        let config = self
            .closed_mesh
            .settings
            .config()
            .map_err(|problem| problem.english())?;
        let scene = self
            .closed_mesh_scene(self.closed_mesh.settings.layers)
            .map_err(|refusal| refusal.status())?;
        Ok((scene, config))
    }

    fn closed_mesh_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |()| Message::ClosedMesh(ClosedMeshAction::Poll),
        )
    }

    /// Start a job on a worker thread. The window reads its progress four
    /// times a second until `ClosedMeshAction::Finished` arrives.
    fn start_closed_mesh_job(
        &mut self,
        scene: Scene,
        config: ClosedMeshConfig,
        destination: Option<(PathBuf, MeshFormat)>,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        let control = Arc::new(Control::default());
        let input = Arc::new(JobInput {
            scene,
            config,
            sides: self.closed_mesh.settings.sides,
            destination,
        });
        let serial = self.closed_mesh.next_serial;
        self.closed_mesh.next_serial += 1;
        let mut job = ClosedMeshJob {
            serial,
            input: Arc::clone(&input),
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id,
            reported: String::new(),
        };
        let first = job.stages().first().copied().unwrap_or(Stage::Planning);
        let _ = control.report(first, 0, 0);
        job.reported = job.status_text();
        self.status.clone_from(&job.reported);
        self.closed_mesh.job = Some(job);
        // The result of an earlier job would read as the result of this one.
        self.closed_mesh.last = None;
        self.closed_mesh.open = true;
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || ClosedMeshEnd::of(run(&input, &control)))
                    .await
                    .unwrap_or_else(|error| ClosedMeshEnd::Failed(error.to_string()))
            },
            move |end| Message::ClosedMesh(ClosedMeshAction::Finished(serial, end)),
        );
        Task::batch([worker, Self::closed_mesh_poll_task()])
    }

    pub(crate) fn update_closed_mesh(&mut self, action: ClosedMeshAction) -> Task<Message> {
        match action {
            ClosedMeshAction::Toggle => {
                // The 3D BAG panel takes the place of Properties. With that
                // panel open the block is out of sight, and the button
                // brings it back instead of closing it.
                let hidden = self.closed_mesh.open && self.bag_panel;
                self.closed_mesh.open = hidden || !self.closed_mesh.open;
                if self.closed_mesh.open {
                    let _ = self.set_bag_panel(false);
                    self.status = "Closed mesh: put the section box around the part to mesh, \
                                   check the settings in Properties and choose Start"
                        .into();
                }
            }
            ClosedMeshAction::Voxel(value) => self.closed_mesh.settings.voxel = value,
            ClosedMeshAction::MaxHole(value) => self.closed_mesh.settings.max_hole = value,
            ClosedMeshAction::Simplify(value) => self.closed_mesh.settings.simplify = value,
            ClosedMeshAction::SamplePercent(value) => {
                self.closed_mesh.settings.sample_percent = value
            }
            ClosedMeshAction::Sides(sides) => self.closed_mesh.settings.sides = sides,
            ClosedMeshAction::Layers(layers) => self.closed_mesh.settings.layers = layers,
            ClosedMeshAction::Start => match self.closed_mesh_start() {
                Ok((scene, config)) => {
                    return self.start_closed_mesh_job(scene, config, None, None)
                }
                Err(problem) => self.status = problem,
            },
            ClosedMeshAction::Poll => {
                let Some(job) = &mut self.closed_mesh.job else {
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
                return Self::closed_mesh_poll_task();
            }
            ClosedMeshAction::Cancel => self.cancel_closed_mesh(),
            ClosedMeshAction::Finished(serial, end) => {
                let Some(job) = self.closed_mesh.job.take_if(|job| job.serial == serial) else {
                    return Task::none();
                };
                self.closed_mesh_finished(job, end);
            }
        }
        Task::none()
    }

    /// Ask a running job to stop. The block under way ends first; the mesh
    /// of the scan and an existing file at the destination stay as they
    /// were.
    pub(crate) fn cancel_closed_mesh(&mut self) {
        if self.closed_mesh.cancel() {
            if let Some(job) = &mut self.closed_mesh.job {
                job.reported = job.status_text();
                self.status.clone_from(&job.reported);
            }
        }
    }

    /// A job ended: give the mesh to its scan, keep what the job reports and
    /// tell the job of the local API.
    fn closed_mesh_finished(&mut self, job: ClosedMeshJob, end: ClosedMeshEnd) {
        // The stations a job found are kept however it ended: a cancel or a
        // failure after the pass over the source must not cost it again.
        self.closed_mesh.keep_stations(job.control.take_learned());
        let last = match end {
            ClosedMeshEnd::Done(finished) => {
                let target = &job.input.scene.target;
                let entry = self
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.matches_source(target));
                let shown_on = entry.map(|entry| {
                    // A scan holds one mesh: this one takes the place of
                    // the mesh it had.
                    entry.mesh = Some(Arc::clone(&finished.mesh.mesh));
                    entry.mesh_topology = Some(finished.mesh.topology);
                    display_name(&entry.cloud.path).to_owned()
                });
                Last::Done {
                    report: Box::new(finished.report),
                    sides: job.input.sides,
                    written: job
                        .input
                        .destination
                        .as_ref()
                        .map(|(path, format)| Written {
                            path: path.clone(),
                            format: *format,
                            origin: finished.origin,
                        }),
                    shown_on,
                }
            }
            ClosedMeshEnd::Cancelled => Last::Cancelled,
            ClosedMeshEnd::Failed(error) => Last::Failed(error),
        };
        if let Some(entry) = job
            .api_job_id
            .as_ref()
            .and_then(|id| self.api_jobs.get_mut(id))
        {
            *entry = last.value();
        }
        self.status = last.status();
        let written = match &last {
            Last::Done {
                written: Some(written),
                ..
            } => Some(written.path.clone()),
            _ => None,
        };
        self.closed_mesh.last = Some(last);
        if let Some(path) = written {
            self.cad_file_written(&path);
        }
    }

    /// Whether a destination is the source file of an open layer.
    fn closed_mesh_overwrites_a_scan(&self, destination: &Path) -> bool {
        let destination = camera_views::source_key(destination);
        self.clouds
            .iter()
            .any(|entry| camera_views::source_key(&entry.cloud.path) == destination)
    }

    /// The `mesh` command of the local API with the mode `closed`: put the
    /// fields it names in the Properties block and mesh with what the block
    /// then holds. With a destination the mesh is also written there.
    pub(crate) fn api_closed_mesh(
        &mut self,
        destination: Option<PathBuf>,
        options: &ClosedMeshOptions,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let destination = match destination {
            Some(path) => match MeshFormat::from_path(&path).filter(|_| path.is_absolute()) {
                Some(format) => Some((path, format)),
                None => return refuse(NO_FORMAT.into()),
            },
            None => None,
        };
        if self.closed_mesh.is_running() || self.mesh_job.is_some() || self.mesh_dialog_pending {
            return refuse("a mesh task is already open or running".into());
        }
        let settings = match self.closed_mesh.settings.with(options) {
            Ok(settings) => settings,
            Err(problem) => return refuse(problem),
        };
        let config = match settings.config() {
            Ok(config) => config,
            Err(problem) => return refuse(uncapitalised(&problem.english())),
        };
        let scene = match self.closed_mesh_scene(settings.layers) {
            Ok(scene) => scene,
            Err(refusal) => return refuse(refusal.api()),
        };
        if let Some((path, _)) = &destination {
            // The file is written when the mesh is ready, so a mistyped
            // folder would cost the whole job.
            if !path.parent().is_some_and(Path::is_dir) {
                return refuse("the folder of the mesh destination does not exist".into());
            }
            if self.closed_mesh_overwrites_a_scan(path) {
                return refuse("mesh requires a destination different from the open scans".into());
            }
        }
        self.closed_mesh.settings = settings;
        let path = destination.as_ref().map(|(path, _)| path.clone());
        let id = self.record_api_job(json!({
            "state": "running",
            "operation": "mesh",
            "mode": "closed",
            "path": path,
        }));
        let task = self.start_closed_mesh_job(scene, config, destination, Some(id.clone()));
        let mut answer = json!({"ok": true, "accepted": true, "job_id": id});
        if let Some(path) = path {
            answer["path"] = json!(path);
        }
        (answer, task)
    }

    /// The `set_closed_mesh_settings` command of the local API: every field
    /// it names goes into the block, or none when one of them is refused.
    pub(crate) fn api_set_closed_mesh_settings(&mut self, options: &ClosedMeshOptions) -> Value {
        match self.closed_mesh.settings.with(options) {
            Ok(settings) => {
                self.closed_mesh.settings = settings;
                json!({"ok": true, "settings": self.closed_mesh.settings.value()})
            }
            Err(problem) => json!({"ok": false, "error": problem}),
        }
    }

    /// The `cancel_mesh` command of the local API for a closed mesh; nothing
    /// when none is being made.
    pub(crate) fn api_cancel_closed_mesh(&mut self) -> Option<Value> {
        if !self.closed_mesh.is_running() {
            return None;
        }
        self.cancel_closed_mesh();
        Some(json!({"ok": true, "cancel_requested": true}))
    }

    /// The button of the tool in the SURFACE group of the ribbon. It needs a
    /// scan to be active, and an open block can always be closed with it.
    pub(crate) fn closed_mesh_ribbon_item(&self) -> opencad_ribbon::RibbonItem<'static> {
        opencad_ribbon::RibbonItem::Small(crate::small_tool_button_when(
            "Closed mesh",
            Message::ClosedMesh(ClosedMeshAction::Toggle),
            self.closed_mesh.open,
            self.active.is_some() || self.closed_mesh.open,
        ))
    }

    /// The block of the tool in Properties: its settings, what a job would
    /// mesh, the button that starts one, a job under way and what the last
    /// job reported.
    pub(crate) fn closed_mesh_properties(&self) -> Option<Element<'_, Message>> {
        let tool = &self.closed_mesh;
        if !tool.open {
            return None;
        }
        let settings = &tool.settings;
        let colors = self.ui_theme.colors();
        let note =
            |content: String| container(text(content).size(10).color(colors.muted)).padding([4, 8]);
        let warning = |content: String| {
            container(text(content).size(10).color(colors.accent)).padding([4, 8])
        };
        let mut block = column![
            opencad_properties::section_header("Closed mesh"),
            opencad_properties::property_input(
                "Voxel size (m)",
                "auto",
                &settings.voxel,
                |value| { Message::ClosedMesh(ClosedMeshAction::Voxel(value)) }
            ),
            opencad_properties::property_input(
                "Close holes up to (m)",
                "0.25",
                &settings.max_hole,
                |value| Message::ClosedMesh(ClosedMeshAction::MaxHole(value)),
            ),
            opencad_properties::property_input(
                "Simplify within (mm)",
                "auto",
                &settings.simplify,
                |value| Message::ClosedMesh(ClosedMeshAction::Simplify(value)),
            ),
            opencad_properties::property_input(
                "Source points (%)",
                "100",
                &settings.sample_percent,
                |value| Message::ClosedMesh(ClosedMeshAction::SamplePercent(value)),
            ),
            opencad_properties::property_control(
                "Sides",
                choice_list(Sides::ALL, settings.sides, Sides::text, |sides| {
                    Message::ClosedMesh(ClosedMeshAction::Sides(sides))
                }),
            ),
            opencad_properties::property_control(
                "Scans",
                choice_list(Layers::ALL, settings.layers, Layers::text, |layers| {
                    Message::ClosedMesh(ClosedMeshAction::Layers(layers))
                }),
            ),
        ]
        .spacing(0)
        .width(Fill);

        if let Some(job) = &tool.job {
            let cancelling = job.cancelling();
            let state = if cancelling {
                tr("Cancelling…").to_owned()
            } else {
                stage_line(job.control.snapshot())
            };
            block = block
                .push(container(text(state).size(11)).padding([6, 8]))
                .push(
                    container(
                        button(tr("Cancel"))
                            .on_press_maybe(
                                (!cancelling)
                                    .then_some(Message::ClosedMesh(ClosedMeshAction::Cancel)),
                            )
                            .style(flat_tool_style),
                    )
                    .padding([3, 8]),
                );
        } else {
            let mut ready = self.mesh_job.is_none() && !self.mesh_dialog_pending;
            match self.closed_mesh_region() {
                Ok(region) => {
                    block = block.push(note(region_note(&region, settings.layers)));
                    let voxel = format!("{:.0}", region.voxel * 1000.0);
                    let triangles = format_count(region.triangles);
                    block = block.push(note(tr_args(
                        "Voxels of {voxel} mm: about {triangles} triangles before simplification for the faces of this box alone. Furniture and inner walls add to that.",
                        &[("voxel", &voxel), ("triangles", &triangles)],
                    )));
                    if let Some(line) = fit_warning(region.fit) {
                        block = block.push(warning(line));
                    }
                }
                Err(problem) => {
                    ready = false;
                    block = block.push(warning(match problem {
                        Problem::Setting(sentence) => sentence.translated(),
                        Problem::Refused(refusal) => refusal.sentence().translated(),
                    }));
                }
            }
            block = block
                .push(note(
                    tr("The result becomes the mesh of the active scan and takes the place of a mesh it has.")
                        .to_owned(),
                ))
                .push(
                    container(
                        button(tr("Start"))
                            .on_press_maybe(
                                ready.then_some(Message::ClosedMesh(ClosedMeshAction::Start)),
                            )
                            .style(|theme, status| {
                                opencad_ribbon::tool_btn_style(theme, false, status)
                            }),
                    )
                    .padding([3, 8]),
                );
        }

        match &tool.last {
            Some(Last::Done { report, sides, .. }) => {
                for row in result_rows(report, *sides) {
                    block = block.push(row);
                }
                for sentence in advice_sentences(report, *sides, &|count| format_count(count)) {
                    block = block.push(warning(sentence.translated()));
                }
            }
            Some(Last::Failed(error)) => {
                block = block
                    .push(note(tr("The last closed mesh failed:").to_owned()))
                    .push(warning(error.clone()));
            }
            Some(Last::Cancelled) => {
                block = block.push(note(tr("The last closed mesh was cancelled.").to_owned()));
            }
            None => {}
        }
        block = block.push(self.cad_viewer_controls());
        Some(block.into())
    }
}

/// What the core reported of the last job, as rows of the Properties block.
fn result_rows(report: &ClosedMeshReport, sides: Sides) -> Vec<Element<'static, Message>> {
    let mut rows = vec![
        opencad_properties::property_row(
            "Last mesh",
            tr_args(
                "{seconds} s, voxel {voxel} mm",
                &[
                    (
                        "seconds",
                        &format!("{:.1}", report.timings.total.as_secs_f64()),
                    ),
                    ("voxel", &format!("{:.0}", report.voxel * 1000.0)),
                ],
            ),
        ),
        opencad_properties::property_row("Vertices", format_count(report.vertices)),
        opencad_properties::property_row("Triangles", format_count(report.triangles)),
        opencad_properties::property_row("Mean deviation", millimetres(report.deviation.mean)),
        opencad_properties::property_row("95% deviation", millimetres(report.deviation.p95)),
        opencad_properties::property_row("Largest deviation", millimetres(report.deviation.max)),
        opencad_properties::property_row("Open edges", format_count(report.topology.open_edges)),
        opencad_properties::property_row(
            "Connected parts",
            format_count(report.topology.components),
        ),
        opencad_properties::property_row(
            "Sides",
            tr(match (report.orientation, sides) {
                (OrientationUsed::Stations, _) => key("From stations"),
                (OrientationUsed::Mixed, _) => key("Stations and centre"),
                (OrientationUsed::Fallback, Sides::Upward) => key("Upward"),
                (OrientationUsed::Fallback, _) => key("Towards the centre"),
            })
            .to_owned(),
        ),
    ];
    // Asked for the centre or upward, every element is without a station,
    // whatever the scan knows.
    let without = without_station(report);
    if sides == Sides::Automatic && without > 0 {
        rows.push(opencad_properties::property_row(
            "Without station",
            tr_args(
                "{count} of {all} elements",
                &[
                    ("count", &format_count(without)),
                    ("all", &format_count(report.surfels)),
                ],
            ),
        ));
    }
    if report.surfels_by_default > 0 {
        rows.push(opencad_properties::property_row(
            "Side undecided",
            tr_args(
                "{count} of {all} elements",
                &[
                    ("count", &format_count(report.surfels_by_default)),
                    ("all", &format_count(report.surfels)),
                ],
            ),
        ));
    }
    rows
}

/// The value that follows an option of the command line, by its position.
pub(crate) fn option_value(arguments: &[OsString], at: usize) -> Option<&str> {
    arguments.get(at).and_then(|value| value.to_str())
}

/// The six limits of a box as the command line gives them.
pub(crate) fn box_limits(limits: &str) -> Result<Bounds, &'static str> {
    let values: Vec<f64> = limits
        .split(',')
        .map(|value| value.trim().parse().ok())
        .collect::<Option<_>>()
        .unwrap_or_default();
    let [x0, y0, z0, x1, y1, z1] = values[..] else {
        return Err("--box must be six comma-separated numbers");
    };
    let bounds = Bounds {
        min: [x0, y0, z0],
        max: [x1, y1, z1],
    };
    // "nan" and "inf" are read as numbers too.
    let ordered = (0..3).all(|axis| bounds.min[axis] <= bounds.max[axis]);
    if !ordered || values.iter().any(|value| !value.is_finite()) {
        return Err("--box must be finite numbers that run from the minimum to the maximum");
    }
    Ok(bounds)
}

/// The value of `--rotation`: degrees about the vertical through the centre
/// of the box.
pub(crate) fn rotation_option(value: &str) -> Result<f64, &'static str> {
    crate::parse_rotation(value).ok_or("--rotation must be a number of degrees")
}

/// The box of `--box` turned by `--rotation`; a turn needs a box.
pub(crate) fn turned_box(
    section: Option<Bounds>,
    rotation: Option<f64>,
) -> Result<Option<OrientedBox>, &'static str> {
    match (section, rotation) {
        (None, Some(_)) => Err("--rotation turns the box of --box, which is missing"),
        (section, rotation) => {
            Ok(section.map(|bounds| OrientedBox::new(bounds, rotation.unwrap_or(0.0))))
        }
    }
}

/// The `--closed-mesh` mode of the command line: make a closed mesh of a
/// scan file, or of a box in it, and write it as OBJ, PLY, STL, DXF, DWG or IFC.
/// `arguments` are what follows the flag. Returns the lines to print, or the
/// exit code with the line that says what is wrong; an empty line stands for
/// the usage line.
pub(crate) fn command_line(arguments: &[OsString]) -> Result<String, (i32, String)> {
    let usage = || (2, String::new());
    let wrong = |line: &str| (2, line.to_owned());
    let [source, destination, options @ ..] = arguments else {
        return Err(usage());
    };
    let (source, destination) = (PathBuf::from(source), PathBuf::from(destination));
    let Some(format) = MeshFormat::from_path(&destination) else {
        return Err(wrong(
            "Supported mesh extensions: .obj, .ply, .stl, .dxf, .dwg, .ifc",
        ));
    };
    if camera_views::source_key(&source) == camera_views::source_key(&destination) {
        return Err(wrong("Choose an output path different from the input"));
    }
    if options.len() % 2 != 0 {
        return Err(usage());
    }
    let mut settings = ClosedMeshSettings::default();
    let mut section = None;
    let mut rotation = None;
    for at in (0..options.len()).step_by(2) {
        let value = option_value(options, at + 1).unwrap_or_default();
        match options[at].to_str() {
            Some("--box") => section = Some(box_limits(value).map_err(wrong)?),
            Some("--rotation") => rotation = Some(rotation_option(value).map_err(wrong)?),
            Some("--voxel") => settings.voxel = value.to_owned(),
            Some("--max-hole") => settings.max_hole = value.to_owned(),
            Some("--simplify") => settings.simplify = value.to_owned(),
            Some("--sample-percent") => settings.sample_percent = value.to_owned(),
            Some("--sides") => {
                settings.sides = Sides::from_name(value)
                    .ok_or_else(|| wrong("--sides must be automatic, centre or upward"))?;
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
    // mesh is written only when it is ready. A bare file name has an empty
    // parent, which is the current folder.
    let folder = destination
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty());
    if folder.is_some_and(|folder| !folder.is_dir()) {
        return Err(wrong("The folder of the output path does not exist"));
    }

    let failed = |error: LoadError| {
        (
            1,
            format!("Closed mesh failed: {}", plain_reason(&error.to_string())),
        )
    };
    let cloud = crate::open_for_export(&source).map_err(failed)?;
    // An index that `--index` or the window left in the cache is used. A
    // file without one is read into memory when it is small enough, and gets
    // an index in a temporary folder otherwise, which goes when the job is
    // done.
    let scratch;
    let mut index = OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())
        .ok()
        .flatten();
    if index.is_none() && cloud.total_points > UNINDEXED_LIMIT {
        scratch = tempfile::tempdir().map_err(|error| failed(error.into()))?;
        index = Some(
            OctreeIndex::build_cached(
                &cloud,
                IndexConfig {
                    scratch_dir: Some(scratch.path().to_path_buf()),
                    ..IndexConfig::default()
                },
            )
            .map_err(failed)?,
        );
    }
    let cloud = Arc::new(cloud);
    let input = JobInput {
        scene: Scene {
            section,
            filter: ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: None,
            },
            layers: vec![SceneLayer {
                cloud: Arc::clone(&cloud),
                index: index.map(Arc::new),
                transform: CloudTransform::default(),
                deleted: None,
            }],
            target: cloud,
            target_transform: CloudTransform::default(),
        },
        config,
        sides: settings.sides,
        destination: Some((destination.clone(), format)),
    };
    let finished = run(&input, &Control::default()).map_err(failed)?;
    let report = &finished.report;
    // How many elements had no station says something about the scan only
    // when stations were asked for.
    let without = match settings.sides {
        Sides::Automatic => format!(", {} without a station", without_station(report)),
        Sides::Centre | Sides::Upward => String::new(),
    };
    let mut lines = format!(
        "Closed mesh written as {}: {}; {} surface elements{without}; {:.1} s -> {}",
        format.label(),
        summary(report, settings.sides, &|count| count.to_string()),
        report.surfels,
        report.timings.total.as_secs_f64(),
        destination.display()
    );
    if let Some(advice) = advice(report, settings.sides) {
        lines.push('\n');
        lines.push_str(&advice);
    }
    if let Some([x, y, z]) = finished.origin {
        lines.push_str(&format!(
            "\nSTL coordinates are relative to the origin {x} {y} {z} m named in the file header"
        ));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests;
