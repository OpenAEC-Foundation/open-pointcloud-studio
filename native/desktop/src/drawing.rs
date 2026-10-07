//! The Section drawing tool: what the section box cuts, as a 2D drawing in
//! DXF or DWG. A plan takes the slab under the top face of the box, a
//! vertical section the slab behind one of its four sides.
//!
//! This module holds the choices of the tool, the job that reads the slab
//! from every visible layer with its progress and its cancel, the preview of
//! the filled cut over the points, the Properties block, the commands of the
//! local API and the `--drawing` mode of the command line. The drawing itself
//! is made by the core.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iced::widget::canvas::{self, Frame};
use iced::widget::{button, column, container, row, text};
use iced::{Color, Element, Fill, Point as UiPoint, Size, Task};
use pointcloud_core::region_source::{RegionFilter, RegionSource};
use pointcloud_core::{
    Bounds, CutPreview, Drawing2d, DrawingFormat, DrawingOrigin, DrawingProgress, DrawingRequest,
    DrawingSource, DrawingStage, DrawingStats, DrawingUnits, DrawingVersion, DrawingView,
    IndexConfig, KeptSlab, LoadError, OctreeIndex, OrientedBox, Point, PointColor, PointLayers,
    WallDirection, DEFAULT_MIN_WALL_THICKNESS,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::bag_panel::plain_reason;
use crate::drawing_view::{DrawScene, DrawingSource as ViewSource, DrawingViewAction};
use crate::i18n::{key, tr, tr_args};
use crate::job_scene::{JobLayer, JobScene};
use crate::open_progress::{Line, Phase};
use crate::saved_drawings::SavedDrawing;
use crate::selection::{ClassFilter, ClassVisibility, Projection};
use crate::ui_style;
// The tests delete points of a layer.
#[cfg(test)]
use crate::selection::DeletionMask;
use crate::sheet_dialog::SheetJob;
use crate::{
    camera_views, compact_count, format_count, measure, opencad_properties, opencad_ribbon,
    same_deletion_mask, CloudEntry, Message, PointViewport, Studio,
};

/// The formats in the order the save dialog offers them, each with the name
/// of its filter. The first one is what a file name without an extension
/// gets where the system adds one.
const FORMATS: [(DrawingFormat, &str); 2] = [
    (DrawingFormat::Dxf, "DXF drawing"),
    (DrawingFormat::Dwg, "DWG drawing"),
];

const NO_FORMAT: &str = "Choose a .dxf or .dwg file name for the drawing";
const SAME_FILE: &str = "Choose a drawing file different from the open scans";
const BUSY: &str = "A section drawing is already open or running";

/// Why no drawing can be started, in the words of the status bar and of the
/// local API.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    NoSection,
    NoLayer,
    /// A visible layer is still being read; this is the name of its file.
    Loading(String),
}

impl Refusal {
    fn status(&self) -> String {
        match self {
            Self::NoSection => "Switch on the section box before making a drawing".into(),
            Self::NoLayer => "Show at least one scan to draw".into(),
            Self::Loading(name) => {
                format!("{name} is still loading; wait for it or hide it before making a drawing")
            }
        }
    }

    fn api(&self) -> String {
        match self {
            Self::NoSection => "section box is not enabled".into(),
            Self::NoLayer => "no visible point cloud to draw".into(),
            Self::Loading(name) => format!("a visible point cloud is still loading: {name}"),
        }
    }
}

/// The smallest wall that goes with a largest wall. The block has no field
/// for it; it never stands above the largest one that was typed.
fn min_wall_for(max_wall_thickness: f64) -> f64 {
    DEFAULT_MIN_WALL_THICKNESS.min(max_wall_thickness)
}

/// A refusal of the core as a line of its own: it starts with a capital.
fn capitalised(reason: &str) -> String {
    let mut letters = reason.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// A length in metres as it was typed, with a comma or a point.
fn metres(input: &str) -> Option<f64> {
    input
        .trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

fn view_text(view: DrawingView) -> &'static str {
    match view {
        DrawingView::Plan => key("Plan"),
        DrawingView::Front => key("Section, front"),
        DrawingView::Back => key("Section, back"),
        DrawingView::Left => key("Section, left"),
        DrawingView::Right => key("Section, right"),
    }
}

fn units_text(units: DrawingUnits) -> &'static str {
    match units {
        DrawingUnits::Millimetres => key("Millimetres"),
        DrawingUnits::Metres => key("Metres"),
    }
}

fn origin_text(origin: DrawingOrigin) -> &'static str {
    match origin {
        DrawingOrigin::Model => key("Model coordinates"),
        DrawingOrigin::BoxCorner => key("Corner of the box"),
    }
}

fn color_text(color: PointColor) -> &'static str {
    match color {
        PointColor::Layer => key("Layer colour"),
        PointColor::Rgb => key("Scan colour (RGB)"),
    }
}

fn layers_text(layers: PointLayers) -> &'static str {
    match layers {
        PointLayers::Source => key("Per scan"),
        PointLayers::Class => key("Per class"),
    }
}

/// The name of a file version; it is the same in every language.
fn version_text(version: DrawingVersion) -> &'static str {
    match version {
        DrawingVersion::R2004 => "R2004",
        DrawingVersion::R2010 => "R2010",
        DrawingVersion::R2013 => "R2013",
        DrawingVersion::R2018 => "R2018",
    }
}

/// A value of a choice list with the English text that names it; the list
/// shows the text in the language in use.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Choice<T> {
    value: T,
    text: &'static str,
}

impl<T> fmt::Display for Choice<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(tr(self.text))
    }
}

/// A list that chooses one of the values of a setting.
fn choice_list<'a, T, const N: usize>(
    all: [T; N],
    current: T,
    name: fn(T) -> &'static str,
    action: fn(T) -> DrawingAction,
) -> Element<'a, Message>
where
    T: Copy + PartialEq + 'static,
{
    let choice = move |value: T| Choice {
        value,
        text: name(value),
    };
    ui_style::pick_list(all.map(choice), Some(choice(current)), move |chosen| {
        Message::Drawing(action(chosen.value))
    })
    .width(Fill)
    .into()
}

/// The choices of the Properties block. Lengths and the point limit are kept
/// as the text that was typed, so that a number is not rewritten while it is
/// being typed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DrawingSettings {
    view: DrawingView,
    thickness: String,
    units: DrawingUnits,
    origin: DrawingOrigin,
    fill: bool,
    square: bool,
    straight_lines: bool,
    line_tolerance: String,
    line_min_length: String,
    grid: String,
    max_wall: String,
    color: PointColor,
    point_layers: PointLayers,
    max_points: String,
    version: DrawingVersion,
}

impl Default for DrawingSettings {
    fn default() -> Self {
        let request = DrawingRequest::default();
        Self {
            view: request.view,
            thickness: format!("{:.2}", request.thickness.unwrap_or_default()),
            units: request.units,
            origin: request.origin,
            fill: request.fill,
            square: request.square,
            straight_lines: false,
            line_tolerance: "0.01".into(),
            line_min_length: "0.10".into(),
            grid: format!("{:.2}", request.grid),
            max_wall: format!("{:.2}", request.max_wall_thickness),
            color: request.color,
            point_layers: request.point_layers,
            max_points: request.max_points.to_string(),
            version: request.version,
        }
    }
}

impl DrawingSettings {
    /// Another view starts with the filled cut as that view has it by
    /// default: on for a plan, off for a vertical section.
    fn set_view(&mut self, view: DrawingView) {
        if view != self.view {
            self.view = view;
            self.fill = DrawingRequest::for_view(view).fill;
        }
    }

    fn point_limit(&self) -> Option<usize> {
        self.max_points
            .trim()
            .replace(['.', ',', ' '], "")
            .parse()
            .ok()
    }

    /// The settings of the cut as `cut_of` gives them for a request, or
    /// `None` while one of their fields holds no number. The point limit and
    /// the other choices that change the file but not the regions are left
    /// out, so that typing in them never counts as another cut.
    fn cut(&self) -> Option<CutSettings> {
        let max_wall = metres(&self.max_wall)?;
        Some((
            self.view,
            Some(metres(&self.thickness)?),
            self.square,
            metres(&self.grid)?,
            max_wall,
            min_wall_for(max_wall),
        ))
    }

    /// What the core is asked for, or why these choices give no drawing.
    fn request(&self) -> Result<DrawingRequest, String> {
        let length = |input: &str, name: &str| {
            metres(input).ok_or_else(|| format!("{name} must be a number of metres"))
        };
        let max_wall_thickness = length(&self.max_wall, "Largest wall thickness")?;
        let request = DrawingRequest {
            thickness: Some(length(&self.thickness, "Slab thickness")?),
            fill: self.fill,
            grid: length(&self.grid, "Grid size")?,
            max_wall_thickness,
            min_wall_thickness: min_wall_for(max_wall_thickness),
            square: self.square,
            straight_lines: if self.straight_lines {
                Some(pointcloud_core::StraightLineOptions {
                    tolerance: length(&self.line_tolerance, "Line tolerance")?,
                    min_length: length(&self.line_min_length, "Minimum line length")?,
                })
            } else {
                None
            },
            units: self.units,
            origin: self.origin,
            max_points: self
                .point_limit()
                .ok_or("Point limit must be a whole number")?,
            point_layers: self.point_layers,
            color: self.color,
            version: self.version,
            ..DrawingRequest::for_view(self.view)
        };
        request
            .validate()
            .map_err(|error| capitalised(plain_reason(&error.to_string())))?;
        Ok(request)
    }

    /// These choices with the fields a command of the local API names.
    fn with(&self, options: &DrawingOptions) -> Result<Self, String> {
        fn chosen<T>(
            value: Option<&String>,
            from_key: fn(&str) -> Option<T>,
            problem: &str,
        ) -> Result<Option<T>, String> {
            value
                .map(|value| {
                    from_key(&value.to_ascii_lowercase()).ok_or_else(|| problem.to_owned())
                })
                .transpose()
        }
        let mut next = self.clone();
        if let Some(view) = chosen(
            options.view.as_ref(),
            DrawingView::from_key,
            "view must be plan, front, back, left or right",
        )? {
            next.set_view(view);
        }
        if let Some(units) = chosen(
            options.units.as_ref(),
            DrawingUnits::from_key,
            "units must be mm or m",
        )? {
            next.units = units;
        }
        if let Some(origin) = chosen(
            options.origin.as_ref(),
            DrawingOrigin::from_key,
            "origin must be model or box",
        )? {
            next.origin = origin;
        }
        if let Some(color) = chosen(
            options.color.as_ref(),
            PointColor::from_key,
            "color must be layer or rgb",
        )? {
            next.color = color;
        }
        if let Some(layers) = chosen(
            options.point_layers.as_ref(),
            PointLayers::from_key,
            "point_layers must be scan or class",
        )? {
            next.point_layers = layers;
        }
        if let Some(version) = chosen(
            options.version.as_ref(),
            DrawingVersion::from_key,
            "version must be r2004, r2010, r2013 or r2018",
        )? {
            next.version = version;
        }
        if let Some(fill) = options.fill {
            next.fill = fill;
        }
        if let Some(square) = options.square {
            next.square = square;
        }
        if let Some(enabled) = options.straight_lines {
            next.straight_lines = enabled;
        }
        if let Some(value) = options.line_tolerance {
            next.line_tolerance = value.to_string();
        }
        if let Some(value) = options.line_min_length {
            next.line_min_length = value.to_string();
        }
        if let Some(thickness) = options.thickness {
            next.thickness = thickness.to_string();
        }
        if let Some(grid) = options.grid {
            next.grid = grid.to_string();
        }
        if let Some(max_wall) = options.max_wall_thickness {
            next.max_wall = max_wall.to_string();
        }
        if let Some(max_points) = options.max_points {
            next.max_points = max_points.to_string();
        }
        Ok(next)
    }

    /// The choices as `status` of the local API reports them; a number that
    /// cannot be read is null.
    fn value(&self) -> Value {
        json!({
            "view": self.view.key(),
            "thickness": metres(&self.thickness),
            "units": self.units.key(),
            "origin": self.origin.key(),
            "fill": self.fill,
            "square": self.square,
            "straight_lines": self.straight_lines,
            "line_tolerance": metres(&self.line_tolerance),
            "line_min_length": metres(&self.line_min_length),
            "grid": metres(&self.grid),
            "max_wall_thickness": metres(&self.max_wall),
            "color": self.color.key(),
            "point_layers": self.point_layers.key(),
            "max_points": self.point_limit(),
            "version": self.version.key(),
        })
    }
}

/// The choices a command of the local API may name. A field that is left out
/// keeps what the Properties block has.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct DrawingOptions {
    /// `plan`, `front`, `back`, `left` or `right`.
    pub view: Option<String>,
    /// Depth of the slab behind the cut plane, in metres.
    pub thickness: Option<f64>,
    /// `mm` or `m`.
    pub units: Option<String>,
    /// `model` or `box`.
    pub origin: Option<String>,
    pub fill: Option<bool>,
    pub square: Option<bool>,
    pub straight_lines: Option<bool>,
    pub line_tolerance: Option<f64>,
    pub line_min_length: Option<f64>,
    /// Cell of the grid the filled cut is traced from, in metres.
    pub grid: Option<f64>,
    /// Two faces at most this far apart are one wall, in metres.
    pub max_wall_thickness: Option<f64>,
    /// `layer` or `rgb`.
    pub color: Option<String>,
    /// `scan` or `class`.
    pub point_layers: Option<String>,
    pub max_points: Option<u64>,
    /// `r2004`, `r2010`, `r2013` or `r2018`.
    pub version: Option<String>,
}

/// What of the scene a drawing is made from: the section box, the visible
/// layers where they stand, their deleted points and the classes shown.
struct Scene {
    section: OrientedBox,
    job: JobScene,
}

/// What the job that makes a drawing of the Project Browser needs, worked
/// out before it starts, so that what goes with the job is kept only when
/// it can start.
pub(crate) struct SheetStart {
    scene: Scene,
    request: DrawingRequest,
}

impl std::ops::Deref for Scene {
    type Target = JobScene;

    fn deref(&self) -> &JobScene {
        &self.job
    }
}

impl Scene {
    fn new(section: OrientedBox, filter: ClassFilter, layers: Vec<JobLayer>) -> Self {
        Self {
            section,
            job: JobScene::for_box(section, filter, layers),
        }
    }

    /// Hand the layers, as the core reads them, and the filter of deleted
    /// points and hidden classes to `read`. The slab lies inside the box,
    /// so the filter does not look at the box.
    fn read<R>(&self, read: impl FnOnce(&[DrawingSource<'_>], &RegionFilter<'_>) -> R) -> R {
        let sources: Vec<DrawingSource<'_>> = self
            .layers
            .iter()
            .map(|layer| DrawingSource {
                source: RegionSource::new(
                    &layer.cloud,
                    layer.index.as_deref(),
                    layer.source_transform(),
                ),
                name: layer.stem(),
            })
            .collect();
        let accept =
            |position: usize, ordinal: u64, point: &Point| self.keeps(position, ordinal, point);
        read(&sources, &accept)
    }
}

enum Target {
    Export(PathBuf, DrawingFormat),
    Preview,
    /// A drawing for the Project Browser, made as it says: as a preview is,
    /// not written, shown in the Drawing view and kept under VIEWS.
    Sheet(Box<SavedDrawing>),
}

impl Target {
    fn operation(&self) -> &'static str {
        match self {
            Self::Export(..) => "export_drawing",
            Self::Preview => "preview_drawing",
            Self::Sheet(_) => "create_drawing",
        }
    }
}

/// Everything a job was started with. A preview keeps it, to tell when the
/// scene it was made from is no longer the scene on screen.
pub(crate) struct JobInput {
    scene: Scene,
    request: DrawingRequest,
    target: Target,
    /// The points a drawing of the Project Browser read before and keeps
    /// for the next time it is made.
    kept: Option<Arc<Mutex<KeptSlab>>>,
}

/// What a finished job hands back: with the drawing it made, which the
/// Drawing view shows without making it again.
enum Done {
    Exported {
        stats: DrawingStats,
        drawing: Box<Drawing2d>,
        path: PathBuf,
    },
    Preview(Box<CutPreview>, Box<Drawing2d>),
}

/// Read the slab and write the drawing or trace the preview. This runs on a
/// worker thread, or in the process of the command line.
fn run(
    input: &JobInput,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<Done, LoadError> {
    let scene = &input.scene;
    scene.validate()?;
    scene
        .read(|sources, accept| match &input.target {
            Target::Export(path, format) => {
                let (drawing, mut stats) = pointcloud_core::section_drawing(
                    sources,
                    scene.section,
                    &input.request,
                    accept,
                    &mut *progress,
                )?;
                let total = drawing.entities.len() as u64;
                stats.bytes = pointcloud_core::write_drawing_progress(
                    &drawing,
                    path,
                    *format,
                    input.request.version,
                    |done| {
                        progress(DrawingProgress {
                            stage: DrawingStage::Writing,
                            done: done as u64,
                            total,
                        })
                    },
                )?;
                Ok(Done::Exported {
                    stats,
                    drawing: Box::new(drawing),
                    path: path.clone(),
                })
            }
            Target::Preview | Target::Sheet(_) => match &input.kept {
                Some(kept) => pointcloud_core::preview_section_drawing_kept(
                    sources,
                    scene.section,
                    &input.request,
                    accept,
                    &mut kept.lock().unwrap_or_else(PoisonError::into_inner),
                    progress,
                ),
                None => pointcloud_core::preview_section_drawing(
                    sources,
                    scene.section,
                    &input.request,
                    accept,
                    progress,
                ),
            }
            .map(|(preview, drawing)| Done::Preview(Box::new(preview), Box::new(drawing))),
        })
        .map_err(|error| hint_empty_slab(error, input.request.view))
}

/// The reason of a slab without points says which face of the box the cut
/// plane is, and what to do about it: with a box drawn around a whole
/// building, the face that a vertical view cuts at often lies outside it.
fn hint_empty_slab(error: LoadError, view: DrawingView) -> LoadError {
    match error {
        LoadError::InvalidData(reason) if reason == EMPTY_SLAB => {
            LoadError::InvalidData(format!("{EMPTY_SLAB}: {}", empty_slab_hint(view)))
        }
        other => other,
    }
}

/// The reason the core gives for a slab without points.
const EMPTY_SLAB: &str = "the slab holds no points";

fn empty_slab_hint(view: DrawingView) -> &'static str {
    match view {
        DrawingView::Plan => "the cut plane is the top face of the section box; move it down into the walls or make the slab thicker",
        DrawingView::Front => "the cut plane is the front face of the section box (Y min); move that face onto the building or make the slab thicker",
        DrawingView::Back => "the cut plane is the back face of the section box (Y max); move that face onto the building or make the slab thicker",
        DrawingView::Left => "the cut plane is the left face of the section box (X min); move that face onto the building or make the slab thicker",
        DrawingView::Right => "the cut plane is the right face of the section box (X max); move that face onto the building or make the slab thicker",
    }
}

/// Where the cut plane of a view lies, for the note under the choice of view.
fn cut_plane_text(view: DrawingView) -> &'static str {
    match view {
        DrawingView::Plan => key("Cuts at the top face of the section box, looking down. The slab is outlined in blue."),
        DrawingView::Front => key("Cuts at the front face of the section box (Y min), looking along +Y. The slab is outlined in blue."),
        DrawingView::Back => key("Cuts at the back face of the section box (Y max), looking along -Y. The slab is outlined in blue."),
        DrawingView::Left => key("Cuts at the left face of the section box (X min), looking along +X. The slab is outlined in blue."),
        DrawingView::Right => key("Cuts at the right face of the section box (X max), looking along -X. The slab is outlined in blue."),
    }
}

/// Find the direction of the walls in the section box of a scene.
fn find_walls(scene: &Scene) -> Result<Option<WallDirection>, String> {
    let found = scene.validate().and_then(|()| {
        scene.read(|sources, accept| {
            pointcloud_core::wall_direction(sources, scene.section, accept, &mut |_| Ok(()))
        })
    });
    found.map_err(|error| plain_reason(&error.to_string()).to_owned())
}

/// How a job ended, as the worker tells the window, with the drawing it
/// made as the Drawing view shows it.
#[derive(Debug, Clone)]
pub enum DrawingEnd {
    Exported(DrawingStats, Arc<DrawScene>),
    Preview(Arc<CutPreview>, Arc<DrawScene>),
    Cancelled,
    Failed(String),
}

impl DrawingEnd {
    /// Runs on the worker thread too, so that the window gets the drawing
    /// ready to show.
    fn of(result: Result<Done, LoadError>) -> Self {
        match result {
            Ok(Done::Exported {
                stats,
                drawing,
                path,
            }) => Self::Exported(
                stats,
                Arc::new(DrawScene::from_drawing(&drawing, ViewSource::Export(path))),
            ),
            Ok(Done::Preview(preview, drawing)) => Self::Preview(
                Arc::from(preview),
                Arc::new(DrawScene::from_drawing(&drawing, ViewSource::Preview)),
            ),
            Err(LoadError::Cancelled) => Self::Cancelled,
            Err(error) => Self::Failed(plain_reason(&error.to_string()).to_owned()),
        }
    }
}

fn stage_key(stage: DrawingStage) -> &'static str {
    match stage {
        DrawingStage::Reading => "reading",
        DrawingStage::Thinning => "thinning",
        DrawingStage::Tracing => "tracing",
        DrawingStage::Writing => "writing",
    }
}

/// What the worker of a job tells the window, and the window the worker.
#[derive(Default)]
pub(crate) struct DrawingControl {
    cancelled: AtomicBool,
    stage: AtomicU8,
    done: AtomicU64,
    total: AtomicU64,
}

impl DrawingControl {
    /// Keep how far the job is, and stop it when that was asked.
    fn report(&self, step: DrawingProgress) -> Result<(), LoadError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        self.stage.store(step.stage as u8, Ordering::Relaxed);
        self.done.store(step.done, Ordering::Relaxed);
        self.total.store(step.total, Ordering::Relaxed);
        Ok(())
    }

    fn snapshot(&self) -> DrawingProgress {
        let stage = self.stage.load(Ordering::Relaxed);
        DrawingProgress {
            stage: [
                DrawingStage::Thinning,
                DrawingStage::Tracing,
                DrawingStage::Writing,
            ]
            .into_iter()
            .find(|known| *known as u8 == stage)
            .unwrap_or(DrawingStage::Reading),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

/// A drawing or a preview that is being made.
pub(crate) struct DrawingJob {
    /// Tells this job from an earlier one whose answer is still on its way.
    serial: u64,
    input: Arc<JobInput>,
    control: Arc<DrawingControl>,
    started: Instant,
    api_job_id: Option<String>,
    /// The line last written to the status bar, so that a look that finds
    /// nothing new leaves the messages of other work readable.
    reported: String,
}

impl DrawingJob {
    fn cancelling(&self) -> bool {
        self.control.cancelled.load(Ordering::Relaxed)
    }

    fn preview(&self) -> bool {
        matches!(self.input.target, Target::Preview)
    }

    /// How far the job is, in the words of a stage.
    fn stage_text(&self) -> String {
        let progress = self.control.snapshot();
        match progress.stage {
            DrawingStage::Reading if progress.total == 0 => "reading the slab".to_owned(),
            DrawingStage::Reading => format!(
                "reading the slab, {} of {} points",
                compact_count(progress.done.min(progress.total)),
                compact_count(progress.total)
            ),
            DrawingStage::Thinning => format!(
                "thinning the points of the slab, {} of {}",
                compact_count(progress.done.min(progress.total)),
                compact_count(progress.total)
            ),
            DrawingStage::Tracing => "tracing the filled cut".to_owned(),
            DrawingStage::Writing => format!(
                "writing {} of {} entities",
                compact_count(progress.done.min(progress.total)),
                compact_count(progress.total)
            ),
        }
    }

    /// The line of the status bar while the job runs.
    fn status_text(&self) -> String {
        if self.cancelling() {
            return "Cancelling the section drawing…".into();
        }
        let subject = if self.preview() {
            "Preview of the filled cut"
        } else {
            "Section drawing"
        };
        format!("{subject}: {}…", self.stage_text())
    }

    /// The job as `status` and `job` of the local API report it.
    fn progress_value(&self) -> Value {
        let progress = self.control.snapshot();
        json!({
            "state": "running",
            "operation": self.input.target.operation(),
            "path": match &self.input.target {
                Target::Export(path, _) => Some(path),
                Target::Preview | Target::Sheet(_) => None,
            },
            "view": self.input.request.view.key(),
            "stage": stage_key(progress.stage),
            "done": progress.done,
            "total": progress.total,
            "fraction": progress.fraction(),
            "cancel_requested": self.cancelling(),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }
}

fn length_text(metres: f64) -> String {
    format!("{:.0} mm", metres * 1000.0)
}

pub(crate) fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{:.0} kB", (bytes as f64 / 1_000.0).max(1.0))
    }
}

/// The main direction without the sign a value that rounds to zero keeps.
fn direction_text(degrees: f64) -> String {
    let rounded = (degrees * 100.0).round() / 100.0;
    format!("{:.2}°", if rounded == 0.0 { 0.0 } else { rounded })
}

/// The thinning doubled the spacing to stay within the point limit.
fn spacing_raised(stats: &DrawingStats, request: &DrawingRequest) -> bool {
    stats.drawn_points > 0 && stats.point_spacing > request.point_spacing * 1.000_1
}

/// The filled cut was traced on larger cells than asked: the points are too
/// sparse for the cell, or they span more than the grid holds at that cell.
fn grid_raised(stats: &DrawingStats, request: &DrawingRequest) -> bool {
    stats
        .grid_cell
        .is_some_and(|cell| cell > request.grid * 1.000_1)
}

/// How deep the slab of a drawing is. The core keeps the slab within the
/// section box, so in a box that is shallower than the thickness asked this
/// is the depth of the box.
fn slab_depth(section: OrientedBox, request: &DrawingRequest) -> f64 {
    pointcloud_core::slab_from_section(section, request.view, request.thickness, request.origin)
        .map(|slab| slab.thickness)
        .ok()
        .or(request.thickness)
        .unwrap_or_default()
}

/// The box was shallower than the slab that was asked.
fn slab_cut_down(slab: f64, request: &DrawingRequest) -> bool {
    request
        .thickness
        .is_some_and(|asked| slab < asked * (1.0 - 1e-9))
}

/// What the core reports of a job, as one line: the points in the slab and
/// in the drawing with their spacing, the regions of the filled cut with the
/// cell and the main direction they were traced with. `slab` is the depth
/// that was drawn; the line names it when the box made it less than asked.
fn summary(
    stats: &DrawingStats,
    request: &DrawingRequest,
    slab: f64,
    count: &dyn Fn(u64) -> String,
) -> String {
    let mut line = format!("{} points in the slab", count(stats.slab_points));
    if slab_cut_down(slab, request) {
        line.push_str(&format!(
            " of {} (the box is thinner than the {} asked)",
            length_text(slab),
            length_text(request.thickness.unwrap_or_default())
        ));
    }
    if stats.drawn_points > 0 {
        line.push_str(&format!(
            ", {} drawn at {}",
            count(stats.drawn_points),
            length_text(stats.point_spacing)
        ));
        if spacing_raised(stats, request) {
            line.push_str(&format!(
                " (raised from {} by the limit of {} points)",
                length_text(request.point_spacing),
                count(request.max_points as u64)
            ));
        }
    }
    if let Some(cell) = stats.grid_cell {
        line.push_str(&match stats.regions {
            1 => "; 1 region".to_owned(),
            regions => format!("; {regions} regions"),
        });
        match stats.dropped_regions {
            0 => {}
            1 => line.push_str(" (1 small one dropped)"),
            dropped => line.push_str(&format!(" ({dropped} small ones dropped)")),
        }
        line.push_str(&format!(", grid {}", length_text(cell)));
        if grid_raised(stats, request) {
            line.push_str(&format!(
                " (coarser than the {} asked)",
                length_text(request.grid)
            ));
        }
        if let Some(degrees) = stats.direction_degrees {
            line.push_str(&format!(", main direction {}", direction_text(degrees)));
        }
    }
    if let Some(lines) = stats.straight_lines {
        line.push_str(&format!(
            "; {} CAD segments, max deviation {:.2} mm, RMS {:.2} mm; {} short edges kept",
            lines.segments,
            lines.max_deviation * 1000.0,
            lines.rms_deviation * 1000.0,
            lines.short_segments
        ));
    }
    line
}

/// The figures of a job for the local API. Lengths are metres; `slab` is the
/// depth of the slab that was drawn.
fn stats_value(stats: &DrawingStats, request: &DrawingRequest, slab: f64) -> Value {
    json!({
        "view": request.view.key(),
        "thickness": slab,
        "units": request.units.key(),
        "slab_points": stats.slab_points,
        "read_points": stats.read_points,
        "reused_points": stats.reused_points,
        "drawn_points": stats.drawn_points,
        "point_spacing": stats.point_spacing,
        "point_spacing_raised": spacing_raised(stats, request),
        "regions": stats.regions,
        "vertices": stats.vertices,
        "dropped_regions": stats.dropped_regions,
        "grid_cell": stats.grid_cell,
        "grid_cell_raised": grid_raised(stats, request),
        "direction_degrees": stats.direction_degrees,
        "straight_lines": stats.straight_lines,
    })
}

/// How the last job ended, for the Properties block and the local API.
#[derive(Debug, Clone, PartialEq)]
enum Last {
    Exported {
        path: PathBuf,
        format: DrawingFormat,
        request: DrawingRequest,
        /// Depth of the slab that was drawn, in metres.
        slab: f64,
        stats: DrawingStats,
    },
    Previewed {
        request: DrawingRequest,
        slab: f64,
        stats: DrawingStats,
    },
    Cancelled {
        operation: &'static str,
    },
    Failed {
        operation: &'static str,
        error: String,
    },
}

impl Last {
    /// The finished job as `job` and `status` of the local API report it.
    fn value(&self) -> Value {
        match self {
            Self::Exported {
                path,
                format,
                request,
                slab,
                stats,
            } => {
                let mut value = stats_value(stats, request, *slab);
                value["state"] = "complete".into();
                value["operation"] = "export_drawing".into();
                value["path"] = json!(path);
                value["format"] = format.extension().into();
                value["bytes"] = stats.bytes.into();
                value
            }
            Self::Previewed {
                request,
                slab,
                stats,
            } => {
                let mut value = stats_value(stats, request, *slab);
                value["state"] = "complete".into();
                value["operation"] = "preview_drawing".into();
                value
            }
            Self::Cancelled { operation } => json!({
                "state": "cancelled",
                "operation": operation,
            }),
            Self::Failed { operation, error } => json!({
                "state": "failed",
                "operation": operation,
                "error": error,
            }),
        }
    }

    /// The line of the status bar when the job has ended.
    fn status(&self) -> String {
        match self {
            Self::Exported {
                path,
                format,
                request,
                slab,
                stats,
            } => format!(
                "Section drawing exported as {format}: {}; {} to {}",
                summary(stats, request, *slab, &|count| format_count(count)),
                size_text(stats.bytes),
                path.display()
            ),
            Self::Previewed {
                request,
                slab,
                stats,
            } => format!(
                "Filled cut previewed: {}",
                summary(stats, request, *slab, &|count| format_count(count))
            ),
            Self::Cancelled {
                operation: "export_drawing",
            } => "Section drawing cancelled; an existing file is left as it was".into(),
            Self::Cancelled { .. } => "Preview of the filled cut cancelled".into(),
            Self::Failed {
                operation: "export_drawing",
                error,
            } => format!("Section drawing failed: {error}"),
            Self::Failed { error, .. } => format!("Preview of the filled cut failed: {error}"),
        }
    }
}

/// The filled cut that lies over the points, with what it was made from.
struct ShownPreview {
    cut: Arc<CutPreview>,
    input: Arc<JobInput>,
}

/// What the Section drawing tool holds: whether its block is open, its
/// choices, a job under way, the preview on screen and how the last job
/// ended.
#[derive(Default)]
pub(crate) struct DrawingTool {
    open: bool,
    settings: DrawingSettings,
    /// The save dialog of an export is open.
    dialog_pending: bool,
    job: Option<DrawingJob>,
    next_serial: u64,
    preview: Option<ShownPreview>,
    last: Option<Last>,
}

impl DrawingTool {
    pub(crate) fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// A job runs or the save dialog is open: nothing else can start.
    pub(crate) fn busy(&self) -> bool {
        self.job.is_some() || self.dialog_pending
    }

    /// The identifier of the drawing of the Project Browser the running job
    /// makes.
    pub(crate) fn running_sheet(&self) -> Option<&str> {
        match &self.job.as_ref()?.input.target {
            Target::Sheet(definition) => Some(&definition.guid),
            _ => None,
        }
    }

    /// The filled cut to lay over the points, when there is one.
    pub(crate) fn overlay(&self) -> Option<&CutPreview> {
        self.preview.as_ref().map(|shown| &*shown.cut)
    }

    /// Ask the worker of a running job to stop.
    pub(crate) fn cancel(&self) -> bool {
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
        let progress = job.control.snapshot();
        let cancelling = job.cancelling();
        // A preview traces and writes nothing; an export traces only when
        // it draws the filled cut, and writing is its last step.
        let traced = job.preview() || job.input.request.fill;
        let steps = 1 + u8::from(traced) + u8::from(!job.preview());
        let step = match progress.stage {
            // Thinning the points read is part of the first step.
            DrawingStage::Reading | DrawingStage::Thinning => 1,
            DrawingStage::Tracing => 2,
            DrawingStage::Writing => steps,
        };
        Some(Line {
            phase: Phase::Drawing,
            title: if cancelling {
                "Cancelling…".to_owned()
            } else if job.preview() {
                "Previewing the filled cut".to_owned()
            } else {
                format!("Section drawing ({})", job.input.request.view.key())
            },
            detail: format!("Step {step} of {steps}  ·  {}", job.stage_text()),
            fraction: progress.fraction(),
            timed: true,
            cancel: (!cancelling).then_some(Message::Drawing(DrawingAction::Cancel)),
        })
    }

    /// The tool as `status` of the local API reports it.
    pub(crate) fn value(&self) -> Value {
        json!({
            "settings": self.settings.value(),
            "job": self.job.as_ref().map(DrawingJob::progress_value),
            "last": self.last.as_ref().map(Last::value),
            "preview_shown": self.preview.is_some(),
            "preview_regions": self.preview.as_ref().map(|shown| shown.cut.regions.len()),
        })
    }
}

/// Everything the Section drawing tool reacts to.
#[derive(Debug, Clone)]
pub enum DrawingAction {
    /// Open the block of the tool in Properties, or close it.
    Toggle,
    View(DrawingView),
    Thickness(String),
    Units(DrawingUnits),
    Origin(DrawingOrigin),
    Fill(bool),
    Square(bool),
    StraightLines(bool),
    LineTolerance(String),
    LineMinLength(String),
    Grid(String),
    MaxWall(String),
    Color(PointColor),
    Layers(PointLayers),
    MaxPoints(String),
    Version(DrawingVersion),
    Preview,
    ClearPreview,
    /// Ask where to save the drawing.
    Export,
    /// The entry of the File view: with the block closed it opens the block
    /// first, so that the view is chosen there; with it open it exports.
    ExportFromFile,
    PathChosen(Option<PathBuf>),
    Poll,
    Cancel,
    Finished(u64, DrawingEnd),
}

/// The view, the slab thickness, squaring, the grid cell and the largest and
/// smallest wall.
type CutSettings = (DrawingView, Option<f64>, bool, f64, f64, f64);

/// The settings a traced cut depends on. Units, origin, colours, layers and
/// the point limit change the file but not the regions.
fn cut_of(request: &DrawingRequest) -> CutSettings {
    (
        request.view,
        request.thickness,
        request.square,
        request.grid,
        request.max_wall_thickness,
        request.min_wall_thickness,
    )
}

/// Whether a layer takes part in a drawing: its points are shown.
/// Names for a status line: all of a few, the first of many with how many
/// more there are.
fn named_briefly(names: &[String]) -> String {
    const SHOWN: usize = 3;
    if names.len() <= SHOWN + 1 {
        return names.join(", ");
    }
    format!(
        "{} and {} more scans",
        names[..SHOWN].join(", "),
        names.len() - SHOWN
    )
}

fn drawn(entry: &CloudEntry) -> bool {
    entry.visible && entry.cloud.total_points > 0
}

impl Studio {
    /// The section box and the visible layers, as a job reads them.
    fn drawing_scene(&self) -> Result<Scene, Refusal> {
        let section = self.section_box().ok_or(Refusal::NoSection)?;
        self.drawing_scene_for(section)
    }

    /// The visible layers read through a box of its own.
    fn drawing_scene_for(&self, section: OrientedBox) -> Result<Scene, Refusal> {
        self.drawing_scene_with(section, drawn)
    }

    /// The layers that `keep` takes, read through a box of its own.
    fn drawing_scene_with(
        &self,
        section: OrientedBox,
        keep: impl Fn(&CloudEntry) -> bool,
    ) -> Result<Scene, Refusal> {
        let drawn = |entry: &CloudEntry| keep(entry) && entry.cloud.total_points > 0;
        // A layer that is still being read holds a cloud that was not checked
        // against its source. The core refuses it only when the read reaches
        // it, after every layer before it was read in full, and leaves it out
        // without a word while its bounds do not reach the slab yet.
        if let Some(loading) = self
            .clouds
            .iter()
            .find(|entry| drawn(entry) && entry.cloud.provisional)
        {
            return Err(Refusal::Loading(
                loading
                    .cloud
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "A visible scan".into()),
            ));
        }
        let layers: Vec<JobLayer> = self
            .clouds
            .iter()
            .filter(|entry| drawn(entry))
            .map(|entry| JobLayer::of(entry, Arc::clone(&entry.cloud)))
            .collect();
        if layers.is_empty() {
            return Err(Refusal::NoLayer);
        }
        Ok(Scene::new(section, self.mesh_filter(), layers))
    }

    /// The slab the tool would draw with its settings as they are, while
    /// its block is open and the section box is on: where the cut plane
    /// lies is then in sight.
    pub(crate) fn drawing_slab(&self) -> Option<OrientedBox> {
        if !self.drawing.open {
            return None;
        }
        let section = self.section_box()?;
        let request = self.drawing.settings.request().ok()?;
        pointcloud_core::slab_from_section(section, request.view, request.thickness, request.origin)
            .ok()
            .map(|slab| slab.region)
    }

    /// Look for the direction of the walls in the section box on a worker
    /// thread. The answer turns the box along them, when the box is still
    /// the one that was looked in.
    pub(crate) fn align_section_to_walls(&mut self) -> Task<Message> {
        if self.section_align_pending {
            return Task::none();
        }
        if self.section_box().is_none() {
            self.status = "Switch on the section box before aligning it to the walls".into();
            return Task::none();
        }
        let scene = match self.drawing_scene() {
            Ok(scene) => scene,
            Err(refusal) => {
                self.status = refusal.status();
                return Task::none();
            }
        };
        let asked = scene.section;
        self.section_align_pending = true;
        self.status = "Looking for the walls in the section box…".into();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || find_walls(&scene))
                    .await
                    .map_err(|error| error.to_string())?
            },
            move |result| Message::SectionWallsFound(asked, result),
        )
    }

    /// Whether what a job was started with is still what the window shows:
    /// the same section box, the same visible layers where they stood, the
    /// same deleted points and classes, and the same settings of the cut.
    fn drawing_scene_current(&self, input: &JobInput) -> bool {
        let scene = &input.scene;
        if self.section_box() != Some(scene.section) {
            return false;
        }
        let filter = self.mesh_filter();
        let groups = |filter: &ClassFilter| {
            (
                filter.ground,
                filter.vegetation,
                filter.buildings,
                filter.other,
                filter.classes,
            )
        };
        if groups(&filter) != groups(&scene.filter) {
            return false;
        }
        let mut visible = self.clouds.iter().filter(|entry| drawn(entry));
        let same_layers = scene.layers.iter().all(|layer| {
            visible.next().is_some_and(|entry| {
                Arc::ptr_eq(&entry.cloud, &layer.cloud)
                    && entry.transform == layer.transform
                    && same_deletion_mask(entry.deleted.as_ref(), layer.deleted.as_ref())
            })
        }) && visible.next().is_none();
        // The request of the job passed the checks of the core when it
        // started, so settings that give the same cut need none here. A field
        // of the cut that cannot be read counts as changed.
        same_layers && self.drawing.settings.cut() == Some(cut_of(&input.request))
    }

    /// After every message: take the preview away when the section box, the
    /// visible layers, a layer transform, the deleted points, the classes
    /// shown or the settings of the cut are no longer what it was made from,
    /// and stop a preview that is being made for a scene that has changed.
    pub(crate) fn settle_drawing(&mut self) {
        let stale = self
            .drawing
            .preview
            .as_ref()
            .is_some_and(|shown| !self.drawing_scene_current(&shown.input));
        if stale {
            self.drawing.preview = None;
        }
        let overtaken = self
            .drawing
            .job
            .as_ref()
            .is_some_and(|job| job.preview() && !self.drawing_scene_current(&job.input));
        if overtaken {
            self.drawing.cancel();
        }
    }

    fn drawing_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |()| Message::Drawing(DrawingAction::Poll),
        )
    }

    /// Start a job on a worker thread. The window reads its progress four
    /// times a second until `DrawingAction::Finished` arrives.
    fn start_drawing_job(
        &mut self,
        scene: Scene,
        request: DrawingRequest,
        target: Target,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        // A drawing of the Project Browser keeps the points it reads.
        let kept = match &target {
            Target::Sheet(definition) => Some(
                self.drawing_view
                    .kept
                    .for_job(&definition.guid, &scene.layers),
            ),
            _ => None,
        };
        let control = Arc::new(DrawingControl::default());
        let input = Arc::new(JobInput {
            scene,
            request,
            target,
            kept,
        });
        let serial = self.drawing.next_serial;
        self.drawing.next_serial += 1;
        let mut job = DrawingJob {
            serial,
            input: Arc::clone(&input),
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id,
            reported: String::new(),
        };
        job.reported = job.status_text();
        self.status.clone_from(&job.reported);
        self.drawing.job = Some(job);
        // The result of an earlier job would read as the result of this one.
        self.drawing.last = None;
        self.drawing.open = true;
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    DrawingEnd::of(run(&input, &mut |step| control.report(step)))
                })
                .await
                .unwrap_or_else(|error| DrawingEnd::Failed(error.to_string()))
            },
            move |end| Message::Drawing(DrawingAction::Finished(serial, end)),
        );
        Task::batch([worker, Self::drawing_poll_task()])
    }

    /// Whether a destination is the source file of an open layer. A scan can
    /// be a DXF file itself, and writing over it would cut it off while it
    /// is being read.
    fn is_open_source(&self, destination: &Path) -> bool {
        let destination = camera_views::source_key(destination);
        self.clouds
            .iter()
            .any(|entry| camera_views::source_key(&entry.cloud.path) == destination)
    }

    /// Make a drawing for the Project Browser from a box that Create 2D
    /// plan / elevation / section worked out, with the other settings of the
    /// Section drawing block, from the visible scans. It shows in the
    /// Drawing view under its name and is kept under VIEWS with how it was
    /// made. Answers why it cannot start.
    pub(crate) fn create_sheet(
        &mut self,
        job: SheetJob,
        api_job_id: Option<String>,
    ) -> Result<Task<Message>, String> {
        if self.drawing.busy() {
            return Err(BUSY.into());
        }
        let scene = self
            .drawing_scene_for(job.section)
            .map_err(|refusal| refusal.status())?;
        let mut request = self.drawing.settings.request()?;
        request.view = job.view;
        request.thickness = job.thickness;
        request.fill = DrawingRequest::for_view(job.view).fill;
        request.sample_percent = job.sample_percent;
        request.straight_lines = job.straight_lines;
        request.square = job.square;
        let mut sources: Vec<PathBuf> = Vec::new();
        for entry in self.clouds.iter().filter(|entry| drawn(entry)) {
            let source = self.views.source_of(&entry.cloud.path);
            if !sources.contains(&source) {
                sources.push(source);
            }
        }
        let name = crate::saved_drawings::free_name(&self.drawing_view.saved, &sources, &job.name);
        let definition = SavedDrawing::new(&name, job.kind, job.section, &request, sources);
        Ok(self.start_sheet_job(scene, request, definition, api_job_id))
    }

    /// Make a drawing of the Project Browser again from how it was made,
    /// from the scans it was made from. Answers why it cannot start.
    pub(crate) fn remake_sheet(
        &mut self,
        definition: SavedDrawing,
        api_job_id: Option<String>,
    ) -> Result<Task<Message>, String> {
        let start = self.sheet_start(&definition)?;
        Ok(self.start_sheet(start, definition, api_job_id))
    }

    /// Start the job that makes a drawing of the Project Browser, with what
    /// `sheet_start` worked out for it.
    pub(crate) fn start_sheet(
        &mut self,
        start: SheetStart,
        definition: SavedDrawing,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        self.start_sheet_job(start.scene, start.request, definition, api_job_id)
    }

    /// What the job that makes a drawing of the Project Browser from how it
    /// was made needs, or why it cannot start: every scan it was made from
    /// is open and no other job runs.
    pub(crate) fn sheet_start(&self, definition: &SavedDrawing) -> Result<SheetStart, String> {
        let open: Vec<PathBuf> = self
            .clouds
            .iter()
            .map(|entry| self.views.source_of(&entry.cloud.path))
            .collect();
        let missing: Vec<String> = definition
            .sources
            .iter()
            .filter(|source| !open.contains(source))
            .map(|source| {
                source.file_name().map_or_else(
                    || source.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                )
            })
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "Open {} to make the drawing {}",
                named_briefly(&missing),
                definition.name
            ));
        }
        if self.drawing.busy() {
            return Err(BUSY.into());
        }
        let request = definition.request().ok_or_else(|| {
            format!(
                "The settings of the drawing {} cannot be used by this version",
                definition.name
            )
        })?;
        let scene = self
            .drawing_scene_with(definition.oriented(), |entry| {
                definition.uses(&self.views.source_of(&entry.cloud.path))
            })
            .map_err(|refusal| refusal.status())?;
        Ok(SheetStart { scene, request })
    }

    fn start_sheet_job(
        &mut self,
        scene: Scene,
        request: DrawingRequest,
        definition: SavedDrawing,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        // The block of the tool stays as it was: the dialog is the tool here.
        let was_open = self.drawing.open;
        let name = definition.name.clone();
        let task = self.start_drawing_job(
            scene,
            request,
            Target::Sheet(Box::new(definition)),
            api_job_id,
        );
        self.drawing.open = was_open;
        self.status = format!("Making the drawing {name}…");
        task
    }

    /// What a job needs from the window, or why it cannot start.
    fn drawing_start(&self) -> Result<(Scene, DrawingRequest), String> {
        if self.drawing.busy() {
            return Err(BUSY.into());
        }
        let scene = self.drawing_scene().map_err(|refusal| refusal.status())?;
        Ok((scene, self.drawing.settings.request()?))
    }

    pub(crate) fn update_drawing(&mut self, action: DrawingAction) -> Task<Message> {
        match action {
            DrawingAction::Toggle => {
                // The 3D BAG panel takes the place of Properties. With that
                // panel open the block is out of sight, and the button
                // brings it back instead of closing it.
                let hidden = self.drawing.open && self.bag_panel;
                self.drawing.open = hidden || !self.drawing.open;
                if self.drawing.open {
                    let _ = self.set_bag_panel(false);
                    self.status = "Section drawing: choose a view and a slab thickness in Properties, then Preview or Export drawing…".into();
                }
            }
            DrawingAction::View(view) => self.drawing.settings.set_view(view),
            DrawingAction::Thickness(value) => self.drawing.settings.thickness = value,
            DrawingAction::Units(units) => self.drawing.settings.units = units,
            DrawingAction::Origin(origin) => self.drawing.settings.origin = origin,
            DrawingAction::Fill(fill) => self.drawing.settings.fill = fill,
            DrawingAction::Square(square) => self.drawing.settings.square = square,
            DrawingAction::StraightLines(on) => self.drawing.settings.straight_lines = on,
            DrawingAction::LineTolerance(value) => self.drawing.settings.line_tolerance = value,
            DrawingAction::LineMinLength(value) => self.drawing.settings.line_min_length = value,
            DrawingAction::Grid(value) => self.drawing.settings.grid = value,
            DrawingAction::MaxWall(value) => self.drawing.settings.max_wall = value,
            DrawingAction::Color(color) => self.drawing.settings.color = color,
            DrawingAction::Layers(layers) => self.drawing.settings.point_layers = layers,
            DrawingAction::MaxPoints(value) => self.drawing.settings.max_points = value,
            DrawingAction::Version(version) => self.drawing.settings.version = version,
            DrawingAction::Preview => match self.drawing_start() {
                Ok((scene, request)) => {
                    return self.start_drawing_job(scene, request, Target::Preview, None)
                }
                Err(problem) => self.status = problem,
            },
            DrawingAction::ClearPreview => {
                if self.drawing.preview.take().is_some() {
                    self.status = "Preview of the filled cut cleared".into();
                }
            }
            DrawingAction::ExportFromFile if !self.drawing.open || self.bag_panel => {
                if let Err(problem) = self.drawing_start() {
                    self.status = problem;
                    return Task::none();
                }
                self.drawing.open = true;
                let _ = self.set_bag_panel(false);
                self.status = "Section drawing: choose the view, a plan or a vertical section, and the slab in Properties, then Export drawing…".into();
            }
            DrawingAction::Export | DrawingAction::ExportFromFile => {
                let request = match self.drawing_start() {
                    Ok((_, request)) => request,
                    Err(problem) => {
                        self.status = problem;
                        return Task::none();
                    }
                };
                let stem = self
                    .active
                    .and_then(|index| self.clouds.get(index))
                    .and_then(|entry| entry.cloud.path.file_stem())
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("section");
                let suggestion = match request.view {
                    DrawingView::Plan => format!("{stem}-plan"),
                    view => format!("{stem}-section-{}", view.key()),
                };
                let suggestion = format!("{suggestion}.{}", FORMATS[0].0.extension());
                self.drawing.dialog_pending = true;
                self.drawing.open = true;
                self.status = "Choose where to save the drawing as DXF or DWG…".into();
                return Task::perform(
                    async move {
                        FORMATS
                            .iter()
                            .fold(rfd::AsyncFileDialog::new(), |dialog, (format, name)| {
                                dialog.add_filter(*name, &[format.extension()])
                            })
                            .set_file_name(suggestion)
                            .save_file()
                            .await
                            .map(|selection| selection.path().to_path_buf())
                    },
                    |path| Message::Drawing(DrawingAction::PathChosen(path)),
                );
            }
            DrawingAction::PathChosen(path) => {
                self.drawing.dialog_pending = false;
                let Some(path) = path else {
                    self.status = "Section drawing cancelled".into();
                    return Task::none();
                };
                let Some(format) = DrawingFormat::from_path(&path) else {
                    self.status = NO_FORMAT.into();
                    return Task::none();
                };
                if self.is_open_source(&path) {
                    self.status = SAME_FILE.into();
                    return Task::none();
                }
                // The box or the layers may have changed while the dialog
                // was open: the drawing is of what the window shows now.
                match self.drawing_start() {
                    Ok((scene, request)) => {
                        return self.start_drawing_job(
                            scene,
                            request,
                            Target::Export(path, format),
                            None,
                        )
                    }
                    Err(problem) => self.status = problem,
                }
            }
            DrawingAction::Poll => {
                let Some(job) = &mut self.drawing.job else {
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
                return Self::drawing_poll_task();
            }
            DrawingAction::Cancel => self.cancel_drawing(),
            DrawingAction::Finished(serial, end) => {
                let Some(job) = self.drawing.job.take_if(|job| job.serial == serial) else {
                    return Task::none();
                };
                self.drawing_finished(job, end);
            }
        }
        Task::none()
    }

    /// Ask a running job to stop. The step under way ends first, and an
    /// existing file at the destination stays as it was.
    pub(crate) fn cancel_drawing(&mut self) {
        if self.drawing.cancel() {
            if let Some(job) = &mut self.drawing.job {
                job.reported = job.status_text();
                self.status.clone_from(&job.reported);
            }
        }
    }

    /// A job ended: keep what it reports, show its preview when the scene is
    /// still the one it was made from, and tell the job of the local API.
    fn drawing_finished(&mut self, job: DrawingJob, end: DrawingEnd) {
        let operation = job.input.target.operation();
        let request = job.input.request;
        // What is reported is the slab that was drawn, not the one asked.
        let slab = slab_depth(job.input.scene.section, &request);
        let mut built = None;
        let last = match (end, &job.input.target) {
            (DrawingEnd::Exported(stats, scene), Target::Export(path, format)) => {
                built = Some((scene, true));
                Last::Exported {
                    path: path.clone(),
                    format: *format,
                    request,
                    slab,
                    stats,
                }
            }
            (DrawingEnd::Preview(cut, scene), Target::Preview) => {
                let stats = cut.stats;
                if self.drawing_scene_current(&job.input) {
                    self.drawing.preview = Some(ShownPreview {
                        cut,
                        input: Arc::clone(&job.input),
                    });
                    built = Some((scene, false));
                    Last::Previewed {
                        request,
                        slab,
                        stats,
                    }
                } else {
                    Last::Failed {
                        operation,
                        error:
                            "the section box, the layers or the settings changed while it was made"
                                .into(),
                    }
                }
            }
            (DrawingEnd::Preview(cut, scene), Target::Sheet(definition)) => {
                let mut named = (*scene).clone();
                named.source = ViewSource::Sheet {
                    guid: definition.guid.clone(),
                    name: definition.name.clone(),
                };
                built = Some((Arc::new(named), true));
                Last::Previewed {
                    request,
                    slab,
                    stats: cut.stats,
                }
            }
            (DrawingEnd::Cancelled, _) => Last::Cancelled { operation },
            (DrawingEnd::Failed(error), _) => Last::Failed { operation, error },
            // A worker answers in the kind it was asked for.
            (DrawingEnd::Exported(..) | DrawingEnd::Preview(..), _) => Last::Failed {
                operation,
                error: "the job answered with another result than was asked".into(),
            },
        };
        let sheet = match &job.input.target {
            Target::Sheet(definition) => Some(definition.as_ref().clone()),
            _ => None,
        };
        // A drawing made again in place after its crop region changed.
        let remade = sheet.as_ref().and_then(|definition| {
            self.drawing_view
                .remake
                .take_if(|remake| remake.guid == definition.guid)
        });
        let mut value = last.value();
        self.status = last.status();
        if let Some(definition) = &sheet {
            // A drawing of the Project Browser reports as one.
            value["operation"] = remade
                .as_ref()
                .map_or("create_drawing", |remake| remake.operation)
                .into();
            value["name"] = definition.name.clone().into();
            value["guid"] = definition.guid.clone().into();
            value["kind"] = definition.kind.key().into();
            if remade.is_some() {
                value["crop"] = crate::drawing_crop::crop_value(definition);
            }
            self.status = match &last {
                Last::Previewed { stats, .. } => match &remade {
                    Some(remake) => Self::remade_status(
                        definition,
                        remake.operation,
                        stats.read_points == 0 && stats.reused_points > 0,
                    ),
                    None => {
                        format!("Drawing {} made; it is listed under VIEWS", definition.name)
                    }
                },
                Last::Cancelled { .. } => format!("Drawing {} cancelled", definition.name),
                Last::Failed { error, .. } => {
                    format!("The drawing {} could not be made: {error}", definition.name)
                }
                Last::Exported { .. } => self.status.clone(),
            };
        }
        if let Some(entry) = job
            .api_job_id
            .as_ref()
            .and_then(|id| self.api_jobs.get_mut(id))
        {
            *entry = value;
        }
        let written = match &last {
            Last::Exported { path, .. } => Some(path.clone()),
            _ => None,
        };
        self.drawing.last = Some(last);
        if let Some((scene, exported)) = built {
            match sheet {
                Some(definition) => {
                    self.keep_made_drawing(definition);
                    self.sheet_drawing_built(scene, remade);
                }
                None => self.section_drawing_built(scene, exported),
            }
        }
        if let Some(path) = written {
            self.cad_file_written(&path);
        }
        // The drawings used longest ago let go of the points they keep when
        // all of them take too much memory.
        let open = self.open_layer_identities();
        self.drawing_view.kept.settle(&open);
    }

    /// Start a job for a command of the local API: put the fields it names
    /// in the Properties block and draw with what the block then holds.
    fn api_start_drawing(
        &mut self,
        command: &str,
        destination: Option<PathBuf>,
        options: &DrawingOptions,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let target = match destination {
            Some(path) => match DrawingFormat::from_path(&path).filter(|_| path.is_absolute()) {
                Some(format) => Target::Export(path, format),
                None => {
                    return refuse(format!(
                        "{command} requires an absolute .dxf or .dwg destination"
                    ))
                }
            },
            None => Target::Preview,
        };
        if self.drawing.busy() {
            return refuse("a section drawing is already open or running".into());
        }
        let settings = match self.drawing.settings.with(options) {
            Ok(settings) => settings,
            Err(problem) => return refuse(problem),
        };
        let request = match settings.request() {
            Ok(request) => request,
            Err(problem) => {
                // The refusals of the core start with a capital for the
                // status bar; the answers of this API do not.
                let mut letters = problem.chars();
                return refuse(match letters.next() {
                    Some(first) => first.to_lowercase().chain(letters).collect(),
                    None => problem,
                });
            }
        };
        let scene = match self.drawing_scene() {
            Ok(scene) => scene,
            Err(refusal) => return refuse(refusal.api()),
        };
        if let Target::Export(path, _) = &target {
            // The core opens its temporary file beside the destination only
            // when the slab has been read, so a mistyped folder would cost
            // the whole read.
            if !path.parent().is_some_and(Path::is_dir) {
                return refuse(format!(
                    "the folder of the {command} destination does not exist"
                ));
            }
            if self.is_open_source(path) {
                return refuse(format!(
                    "{command} requires a destination different from the open scans"
                ));
            }
        }
        self.drawing.settings = settings;
        let path = match &target {
            Target::Export(path, _) => Some(path.clone()),
            Target::Preview | Target::Sheet(_) => None,
        };
        let id = self.record_api_job(json!({
            "state": "running",
            "operation": target.operation(),
            "path": path,
            "view": request.view.key(),
        }));
        let task = self.start_drawing_job(scene, request, target, Some(id.clone()));
        let mut answer = json!({"ok": true, "accepted": true, "job_id": id});
        if let Some(path) = path {
            answer["path"] = json!(path);
        }
        (answer, task)
    }

    /// The `export_drawing` command of the local API.
    pub(crate) fn api_export_drawing(
        &mut self,
        path: PathBuf,
        options: &DrawingOptions,
    ) -> (Value, Task<Message>) {
        self.api_start_drawing("export_drawing", Some(path), options)
    }

    /// The `preview_drawing` command of the local API.
    pub(crate) fn api_preview_drawing(
        &mut self,
        options: &DrawingOptions,
    ) -> (Value, Task<Message>) {
        self.api_start_drawing("preview_drawing", None, options)
    }

    /// The `clear_drawing_preview` command of the local API.
    pub(crate) fn api_clear_drawing_preview(&mut self) -> Value {
        let cleared = self.drawing.preview.take().is_some();
        if cleared {
            self.status = "Preview of the filled cut cleared".into();
        }
        json!({"ok": true, "cleared": cleared})
    }

    /// The `cancel_drawing` command of the local API.
    pub(crate) fn api_cancel_drawing(&mut self) -> Value {
        if !self.drawing.is_running() {
            return json!({"ok": false, "error": "no section drawing is running"});
        }
        self.cancel_drawing();
        json!({"ok": true, "cancel_requested": true})
    }

    /// Whether the ribbon button can be pressed: it needs the section box,
    /// and an open block can always be closed with it.
    pub(crate) fn drawing_button_enabled(&self) -> bool {
        self.section_enabled || self.drawing.open
    }

    /// Whether the File view entry can be chosen: the section box is on and
    /// no job or save dialog of the tool is under way.
    pub(crate) fn drawing_entry_enabled(&self) -> bool {
        self.section_enabled && !self.drawing.busy()
    }

    /// The button of the tool in the SECTION BOX group of the ribbon.
    pub(crate) fn drawing_ribbon_item(&self) -> opencad_ribbon::RibbonItem<'static> {
        opencad_ribbon::RibbonItem::Large(crate::large_tool_button_when(
            "Section drawing",
            Message::Drawing(DrawingAction::Toggle),
            self.drawing.open,
            self.drawing_button_enabled(),
        ))
    }

    /// The block of the tool in Properties: its choices, the buttons that
    /// preview and export, a job under way and what the last job reported.
    pub(crate) fn drawing_properties(&self) -> Option<Element<'_, Message>> {
        let tool = &self.drawing;
        if !tool.open {
            return None;
        }
        let settings = &tool.settings;
        let check = |label: &'static str, on: bool, action: fn(bool) -> DrawingAction| {
            container(
                ui_style::checkbox(label, on)
                    .on_toggle(move |value| Message::Drawing(action(value))),
            )
            .padding([4, 8])
        };
        let note = |content: &'static str| {
            container(
                text(content)
                    .size(10)
                    .color(self.ui_theme.colors().text_muted),
            )
            .padding([4, 8])
        };
        let mut block = column![
            opencad_properties::section_header("Section drawing"),
            opencad_properties::property_control(
                "View",
                choice_list(
                    DrawingView::ALL,
                    settings.view,
                    view_text,
                    DrawingAction::View
                ),
            ),
            opencad_properties::property_input(
                "Slab thickness (m)",
                "0.10",
                &settings.thickness,
                |value| Message::Drawing(DrawingAction::Thickness(value)),
            ),
            opencad_properties::property_control(
                "Units",
                choice_list(
                    DrawingUnits::ALL,
                    settings.units,
                    units_text,
                    DrawingAction::Units
                ),
            ),
            opencad_properties::property_control(
                "Origin",
                choice_list(
                    DrawingOrigin::ALL,
                    settings.origin,
                    origin_text,
                    DrawingAction::Origin
                ),
            ),
            check(tr("Filled cut"), settings.fill, DrawingAction::Fill),
            check(
                tr("Straight CAD lines"),
                settings.straight_lines,
                DrawingAction::StraightLines
            ),
            opencad_properties::property_input(
                "Line tolerance (m)",
                "0.01",
                &settings.line_tolerance,
                |value| Message::Drawing(DrawingAction::LineTolerance(value))
            ),
            opencad_properties::property_input(
                "Minimum line (m)",
                "0.10",
                &settings.line_min_length,
                |value| Message::Drawing(DrawingAction::LineMinLength(value))
            ),
            check(
                tr("Square to main directions"),
                settings.square,
                DrawingAction::Square
            ),
            opencad_properties::property_input(
                "Largest wall (m)",
                "0.50",
                &settings.max_wall,
                |value| Message::Drawing(DrawingAction::MaxWall(value)),
            ),
            opencad_properties::property_input("Grid size (m)", "0.02", &settings.grid, |value| {
                Message::Drawing(DrawingAction::Grid(value))
            },),
            opencad_properties::property_control(
                "Point colour",
                choice_list(
                    PointColor::ALL,
                    settings.color,
                    color_text,
                    DrawingAction::Color
                ),
            ),
            opencad_properties::property_control(
                "Point layers",
                choice_list(
                    PointLayers::ALL,
                    settings.point_layers,
                    layers_text,
                    DrawingAction::Layers
                ),
            ),
            opencad_properties::property_input(
                "Point limit",
                "150000",
                &settings.max_points,
                |value| Message::Drawing(DrawingAction::MaxPoints(value)),
            ),
            opencad_properties::property_control(
                "File version",
                choice_list(
                    DrawingVersion::ALL,
                    settings.version,
                    version_text,
                    DrawingAction::Version
                ),
            ),
        ]
        .spacing(0)
        .width(Fill);
        // What the cut plane is and how the format is chosen show in the
        // tooltips of the buttons, not as text on the panel.
        let mut explanation = vec![tr(cut_plane_text(settings.view)).to_owned()];
        if !self.section_enabled {
            explanation.insert(
                0,
                tr("Switch on the section box to make a drawing.").to_owned(),
            );
        }

        if let Some(job) = &tool.job {
            let cancelling = job.cancelling();
            let progress = job.control.snapshot();
            let state = if cancelling {
                tr("Cancelling…")
            } else {
                match progress.stage {
                    DrawingStage::Reading => tr("Reading the slab…"),
                    DrawingStage::Thinning => tr("Thinning the points of the slab…"),
                    DrawingStage::Tracing => tr("Tracing the filled cut…"),
                    DrawingStage::Writing => tr("Writing the file…"),
                }
            };
            block = block
                .push(container(text(state).size(11)).padding([6, 8]))
                .push(
                    container(ui_style::secondary_button(tr("Cancel")).on_press_maybe(
                        (!cancelling).then_some(Message::Drawing(DrawingAction::Cancel)),
                    ))
                    .padding([3, 8]),
                );
        } else {
            let ready = self.section_enabled && !tool.busy();
            let act = |action: DrawingAction| ready.then_some(Message::Drawing(action));
            block = block
                .push(
                    container(
                        row![
                            opencad_properties::explained(
                                button(tr("Preview"))
                                    .on_press_maybe(act(DrawingAction::Preview))
                                    .style(ui_style::tool),
                                explanation.clone(),
                            ),
                            button(tr("Clear preview"))
                                .on_press_maybe(
                                    tool.preview
                                        .is_some()
                                        .then_some(Message::Drawing(DrawingAction::ClearPreview),)
                                )
                                .style(ui_style::tool),
                        ]
                        .spacing(3),
                    )
                    .padding([3, 8]),
                )
                .push(
                    container(opencad_properties::explained(
                        button(tr("Export drawing…"))
                            .on_press_maybe(act(DrawingAction::Export))
                            .style(|theme, status| ui_style::ribbon_button(theme, false, status)),
                        {
                            let mut lines = explanation.clone();
                            lines.push(
                                tr("The file name chooses the format: .dxf or .dwg.").to_owned(),
                            );
                            lines
                        },
                    ))
                    .padding([3, 8]),
                );
        }
        block = block.push(
            container(
                row![
                    button(text(tr("Show drawing")).size(11))
                        .on_press_maybe(
                            self.drawing_view
                                .has_section_drawing()
                                .then_some(Message::DrawingView(DrawingViewAction::Show(true))),
                        )
                        .style(ui_style::tool),
                    ui_style::checkbox(
                        tr("Show after export"),
                        self.drawing_view.show_after_export
                    )
                    .on_toggle(|on| {
                        Message::DrawingView(DrawingViewAction::ShowAfterExport(on))
                    }),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            )
            .padding([3, 8]),
        );

        match &tool.last {
            Some(Last::Exported {
                format,
                request,
                slab,
                stats,
                ..
            }) => {
                // The status bar names the file; a row has no room for it.
                block = block.push(opencad_properties::property_row(
                    "Last drawing",
                    format!("{format} · {}", size_text(stats.bytes)),
                ));
                for line in result_rows(stats, request, *slab) {
                    block = block.push(line);
                }
            }
            Some(Last::Previewed {
                request,
                slab,
                stats,
            }) => {
                block = block.push(opencad_properties::property_row(
                    "Last drawing",
                    tr(if tool.preview.is_some() {
                        key("Preview, shown")
                    } else {
                        key("Preview, cleared")
                    })
                    .to_owned(),
                ));
                for line in result_rows(stats, request, *slab) {
                    block = block.push(line);
                }
            }
            Some(Last::Failed { error, .. }) => {
                block = block.push(note(tr("The last drawing failed:"))).push(
                    container(
                        text(error.as_str())
                            .size(11)
                            .color(self.ui_theme.colors().accent),
                    )
                    .padding([0, 8]),
                );
            }
            Some(Last::Cancelled { .. }) => {
                block = block.push(note(tr("The last drawing was cancelled.")));
            }
            None => {}
        }
        block = block.push(self.cad_viewer_controls());
        Some(block.into())
    }
}

/// What the core reported of the last job, as rows of the Properties block.
/// The depth of the slab gets a row only when the box made it less than
/// asked; otherwise it is what the block says above.
fn result_rows(
    stats: &DrawingStats,
    request: &DrawingRequest,
    slab: f64,
) -> Vec<Element<'static, Message>> {
    let mut rows = Vec::new();
    if slab_cut_down(slab, request) {
        let depth = format!("{:.0}", slab * 1000.0);
        rows.push(opencad_properties::property_row(
            "Slab drawn",
            tr_args("{depth} mm (box is thinner)", &[("depth", &depth)]),
        ));
    }
    rows.push(opencad_properties::property_row(
        "Points in slab",
        format_count(stats.slab_points),
    ));
    if stats.drawn_points > 0 {
        let spacing = format!("{:.0}", stats.point_spacing * 1000.0);
        rows.push(opencad_properties::property_row(
            "Points drawn",
            format_count(stats.drawn_points),
        ));
        rows.push(opencad_properties::property_row(
            "Point spacing",
            if spacing_raised(stats, request) {
                tr_args("{spacing} mm (raised)", &[("spacing", &spacing)])
            } else {
                format!("{spacing} mm")
            },
        ));
    }
    if let Some(cell) = stats.grid_cell {
        rows.push(opencad_properties::property_row(
            "Regions",
            if stats.dropped_regions > 0 {
                tr_args(
                    "{regions} · {dropped} dropped",
                    &[
                        ("regions", &stats.regions),
                        ("dropped", &stats.dropped_regions),
                    ],
                )
            } else {
                stats.regions.to_string()
            },
        ));
        let cell = format!("{:.0}", cell * 1000.0);
        rows.push(opencad_properties::property_row(
            "Grid cell",
            if grid_raised(stats, request) {
                tr_args("{cell} mm (coarser)", &[("cell", &cell)])
            } else {
                format!("{cell} mm")
            },
        ));
        if let Some(degrees) = stats.direction_degrees {
            rows.push(opencad_properties::property_row(
                "Main direction",
                direction_text(degrees),
            ));
        }
    }
    if let Some(lines) = stats.straight_lines {
        rows.push(opencad_properties::property_row(
            "CAD segments",
            format!("{} → {}", lines.input_segments, lines.segments),
        ));
        rows.push(opencad_properties::property_row(
            "Contour deviation",
            format!(
                "max {:.2} mm · RMS {:.2} mm",
                lines.max_deviation * 1000.0,
                lines.rms_deviation * 1000.0
            ),
        ));
        rows.push(opencad_properties::property_row(
            "Short edges kept",
            lines.short_segments.to_string(),
        ));
    }
    rows
}

/// The part of a ring that lies in front of the eye and inside the viewport
/// with a margin, on screen. Every ring is cut off on its own. The edges that
/// adds run along the near plane and the margin, where they enclose nothing,
/// so the even-odd fill of all rings of a region covers the same pixels of
/// the viewport as the whole region would. Without this a vertex behind a
/// walking camera has no place on screen, and one close to the eye or far
/// outside a zoomed-in view lands millions of pixels away.
pub(crate) fn screen_ring(projection: Projection, ring: &[[f64; 3]], size: Size) -> Vec<UiPoint> {
    // The limits `measure::project_edge` cuts the edges of the outline at.
    const NEAR: f64 = 0.02;
    const MARGIN: f64 = 16.0;
    // In the scene first: a vertex behind the eye cannot be projected.
    let mut front = Vec::with_capacity(ring.len() + 2);
    for (index, from) in ring.iter().enumerate() {
        let to = ring[(index + 1) % ring.len()];
        let (depth_from, depth_to) = (projection.depth(*from), projection.depth(to));
        if depth_from >= NEAR {
            front.push(*from);
        }
        if (depth_from >= NEAR) != (depth_to >= NEAR) {
            let t = (NEAR - depth_from) / (depth_to - depth_from);
            front.push(std::array::from_fn(|axis| {
                from[axis] + (to[axis] - from[axis]) * t
            }));
        }
    }
    let mut points: Vec<[f64; 2]> = front
        .iter()
        .filter_map(|xyz| projection.project_unclipped(*xyz))
        .map(|(x, y, _)| [f64::from(x), f64::from(y)])
        .collect();
    // Then against the four sides of the viewport, one after the other.
    for (axis, limit, above) in [
        (0, -MARGIN, true),
        (0, f64::from(size.width) + MARGIN, false),
        (1, -MARGIN, true),
        (1, f64::from(size.height) + MARGIN, false),
    ] {
        let inside = |point: &[f64; 2]| {
            if above {
                point[axis] >= limit
            } else {
                point[axis] <= limit
            }
        };
        let mut kept = Vec::with_capacity(points.len() + 2);
        for (index, from) in points.iter().enumerate() {
            let to = points[(index + 1) % points.len()];
            if inside(from) {
                kept.push(*from);
            }
            if inside(from) != inside(&to) {
                let t = (limit - from[axis]) / (to[axis] - from[axis]);
                let mut cut = [
                    from[0] + (to[0] - from[0]) * t,
                    from[1] + (to[1] - from[1]) * t,
                ];
                cut[axis] = limit;
                kept.push(cut);
            }
        }
        points = kept;
    }
    points
        .into_iter()
        .map(|[x, y]| UiPoint::new(x as f32, y as f32))
        .collect()
}

impl PointViewport<'_> {
    /// Lay the filled cut of the preview over the points: each region with
    /// its holes as one shape filled by the even-odd rule, and its outline.
    /// It is drawn on top of the points without a depth test, so it reads
    /// best looking straight at the cut plane.
    pub fn draw_drawing(&self, frame: &mut Frame, size: Size) {
        let Some(scene) = crate::combined_bounds(self.clouds) else {
            return;
        };
        let projection = self.projection(scene, size.width, size.height);
        if let Some(slab) = self.drawing_slab {
            draw_slab(frame, projection, slab, size);
        }
        let Some(preview) = self.drawing else {
            return;
        };
        let outline = canvas::Stroke::default()
            .with_color(Color::from_rgb8(245, 158, 11))
            .with_width(1.2);
        for region in &preview.regions {
            let rings = || std::iter::once(&region.outer).chain(&region.holes);
            let shape = canvas::Path::new(|path| {
                for ring in rings() {
                    let points = screen_ring(projection, ring, size);
                    let Some((first, rest)) = points.split_first().filter(|_| points.len() >= 3)
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
                    style: canvas::Style::Solid(Color::from_rgba8(168, 168, 176, 0.55)),
                    rule: canvas::fill::Rule::EvenOdd,
                },
            );
            // The outline is drawn edge by edge: a stroke of the shape would
            // also draw the edges the cutting off added.
            let edges = canvas::Path::new(|path| {
                for ring in rings() {
                    for (edge, from) in ring.iter().enumerate() {
                        let to = ring[(edge + 1) % ring.len()];
                        if let Some([start, end]) =
                            measure::project_edge(projection, *from, to, size)
                        {
                            path.move_to(start);
                            path.line_to(end);
                        }
                    }
                }
            });
            frame.stroke(&edges, outline);
        }
        let straight = canvas::Stroke::default()
            .with_color(Color::from_rgb8(40, 110, 210))
            .with_width(2.0);
        let lines = canvas::Path::new(|path| {
            for ring in &preview.straight_rings {
                for (edge, from) in ring.iter().enumerate() {
                    let to = ring[(edge + 1) % ring.len()];
                    if let Some([start, end]) = measure::project_edge(projection, *from, to, size) {
                        path.move_to(start);
                        path.line_to(end);
                    }
                }
            }
        });
        frame.stroke(&lines, straight);
    }
}

/// The colour of the slab outline, apart from the amber of the section box.
const SLAB_RGB: [u8; 3] = [59, 130, 246];

/// Outline the slab of the Section drawing tool: its twelve edges, cut off
/// where they leave the view.
fn draw_slab(frame: &mut Frame, projection: Projection, slab: OrientedBox, size: Size) {
    let corners = slab.corners();
    let edges = canvas::Path::new(|path| {
        for (from, to) in [
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
            if let Some([start, end]) =
                measure::project_edge(projection, corners[from], corners[to], size)
            {
                path.move_to(start);
                path.line_to(end);
            }
        }
    });
    frame.stroke(
        &edges,
        canvas::Stroke::default()
            .with_color(Color::from_rgb8(SLAB_RGB[0], SLAB_RGB[1], SLAB_RGB[2]))
            .with_width(1.8),
    );
}

/// The value that follows an option of the command line, by its position.
fn option_value(arguments: &[OsString], at: usize) -> Option<&str> {
    arguments.get(at).and_then(|value| value.to_str())
}

/// The `--drawing` mode of the command line: draw the slab behind one face
/// of a box in a scan file and write it as DXF or DWG. `arguments` are what
/// follows the flag. Returns the line to print, or the exit code with the
/// line that says what is wrong; an empty line stands for the usage line.
pub(crate) fn command_line(arguments: &[OsString]) -> Result<String, (i32, String)> {
    let usage = || (2, String::new());
    let wrong = |line: &str| (2, line.to_owned());
    let [source, limits, destination, options @ ..] = arguments else {
        return Err(usage());
    };
    let (source, destination) = (PathBuf::from(source), PathBuf::from(destination));
    let Some(format) = DrawingFormat::from_path(&destination) else {
        return Err(wrong("Supported drawing extensions: .dxf, .dwg"));
    };
    if camera_views::source_key(&source) == camera_views::source_key(&destination) {
        return Err(wrong("Choose an output path different from the input"));
    }
    let values: Vec<f64> = limits
        .to_str()
        .and_then(|limits| {
            limits
                .split(',')
                .map(|value| value.trim().parse().ok())
                .collect()
        })
        .unwrap_or_default();
    let [x0, y0, z0, x1, y1, z1] = values[..] else {
        return Err(wrong("Section limits must be six comma-separated numbers"));
    };
    let mut section = OrientedBox::from(Bounds {
        min: [x0, y0, z0],
        max: [x1, y1, z1],
    });
    // "nan" and "inf" are read as numbers too.
    let ordered = (0..3).all(|axis| section.bounds.min[axis] <= section.bounds.max[axis]);
    if !ordered || values.iter().any(|value| !value.is_finite()) {
        return Err(wrong(
            "Section limits must be finite numbers that run from the minimum to the maximum",
        ));
    }

    if options.len() % 2 != 0 {
        return Err(usage());
    }
    let mut settings = DrawingSettings::default();
    // The view comes first, whatever its place: it decides whether the cut
    // is filled when no option says so.
    for at in (0..options.len()).step_by(2) {
        if options[at] == "--view" {
            let view = option_value(options, at + 1)
                .and_then(DrawingView::from_key)
                .ok_or_else(|| wrong("--view must be plan, front, back, left or right"))?;
            settings.set_view(view);
        }
    }
    for at in (0..options.len()).step_by(2) {
        let value = option_value(options, at + 1).unwrap_or_default();
        match options[at].to_str() {
            Some("--view") => {}
            Some("--rotation") => {
                section.rotation_degrees = crate::parse_rotation(value)
                    .ok_or_else(|| wrong("--rotation must be a number of degrees"))?;
            }
            Some("--thickness") => settings.thickness = value.to_owned(),
            Some("--units") => {
                settings.units = DrawingUnits::from_key(value)
                    .ok_or_else(|| wrong("--units must be mm or m"))?;
            }
            Some("--fill") => {
                settings.fill = match value {
                    "on" => true,
                    "off" => false,
                    _ => return Err(wrong("--fill must be on or off")),
                };
            }
            _ => return Err(usage()),
        }
    }
    let request = settings.request().map_err(|problem| (2, problem))?;
    // The box and the folder of the output are checked before the input is
    // opened: opening a file that is not LAS or LAZ is a full pass over it,
    // and the core opens its temporary file beside the output only when the
    // slab has been read. The box is put to the test of the core itself.
    let slab = pointcloud_core::slab_from_section(
        section,
        request.view,
        request.thickness,
        request.origin,
    )
    .map_err(|error| (2, capitalised(plain_reason(&error.to_string()))))?
    .thickness;
    // A bare file name has an empty parent, which is the current folder.
    let folder = destination
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty());
    if folder.is_some_and(|folder| !folder.is_dir()) {
        return Err(wrong("The folder of the output path does not exist"));
    }

    let failed = |error: LoadError| {
        (
            1,
            format!("Drawing failed: {}", plain_reason(&error.to_string())),
        )
    };
    let cloud = crate::open_for_export(&source).map_err(failed)?;
    // An index that `--index` or the window left in the cache saves reading
    // the whole file; without one the file is read in full.
    let index = OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())
        .ok()
        .flatten();
    let input = JobInput {
        scene: Scene::new(
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
        request,
        target: Target::Export(destination.clone(), format),
        kept: None,
    };
    match run(&input, &mut |_| Ok(())).map_err(failed)? {
        Done::Exported { stats, .. } => Ok(format!(
            "Drawing written as {format}, view {}: {}; {} -> {}",
            request.view.key(),
            summary(&stats, &request, slab, &|count| count.to_string()),
            size_text(stats.bytes),
            destination.display()
        )),
        Done::Preview(..) => Err((1, "Drawing failed: nothing was written".into())),
    }
}

#[cfg(test)]
impl Studio {
    /// Do what the worker thread of the running job does, and hand its end
    /// to the window: for the tests of other modules.
    pub(crate) fn finish_drawing_job(&mut self) {
        let running = self.drawing.job.as_ref().expect("a job runs");
        let (serial, input, control) = (
            running.serial,
            Arc::clone(&running.input),
            Arc::clone(&running.control),
        );
        let end = DrawingEnd::of(run(&input, &mut |step| control.report(step)));
        let _ = self.update(Message::Drawing(DrawingAction::Finished(serial, end)));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use pointcloud_core::MAX_DRAWING_POINTS;

    use super::*;
    use crate::file_view::FileAction;
    use crate::i18n::{Language, TestLanguage};
    use crate::native_api::{ApiCommand, ApiRequest};

    /// A room of 4 by 3 m inside with walls of 0.1 m, scanned on both faces
    /// of every wall at four heights between 1.0 and 1.1 m and at four
    /// heights beside that, with a door opening of 0.9 m in the wall at
    /// y = 0. Two loose points far above and below keep the model larger
    /// than the room, so that a section box around the room lies inside it.
    /// Returns the points and how many lie between the heights 1.0 and 1.1.
    fn room_points() -> (Vec<[f64; 3]>, u64) {
        const STEP: f64 = 0.01;
        const DOOR: [f64; 2] = [1.5, 2.4];
        let mut plan: Vec<[f64; 2]> = Vec::new();
        let run = |from: f64, to: f64| {
            let count = ((to - from) / STEP).round() as usize;
            (0..=count).map(move |step| from + step as f64 * STEP)
        };
        for (low, high, offset) in [(0.0, 4.0, 0.0), (-0.1, 4.1, -0.1)] {
            // The wall at y = 0 with its door, and the wall at y = 3.
            for x in run(low, high) {
                if x <= DOOR[0] || x >= DOOR[1] {
                    plan.push([x, offset]);
                }
                plan.push([x, 3.0 - offset]);
            }
            for y in run(offset, 3.0 - offset) {
                plan.push([offset, y]);
                plan.push([4.0 - offset, y]);
            }
        }
        // The jambs of the door.
        for y in run(-0.1, 0.0) {
            plan.push([DOOR[0], y]);
            plan.push([DOOR[1], y]);
        }
        let heights = [0.96, 0.985, 1.01, 1.035, 1.06, 1.085, 1.11, 1.135];
        let mut points = vec![[-2.0, -2.0, 0.0], [6.0, 5.0, 2.0]];
        for z in heights {
            points.extend(plan.iter().map(|[x, y]| [*x, *y, z]));
        }
        let in_slab = heights.iter().filter(|z| (1.0..=1.1).contains(*z)).count();
        (points, (plan.len() * in_slab) as u64)
    }

    /// The room as the text of an XYZ file, with how many of its points lie
    /// between the heights 1.0 and 1.1.
    fn room_xyz() -> (String, u64) {
        let (points, in_slab) = room_points();
        let text = points
            .iter()
            .map(|[x, y, z]| format!("{x:.3} {y:.3} {z:.3}\n"))
            .collect();
        (text, in_slab)
    }

    /// The room 10 m further along X, as an ASCII PLY file in which every
    /// second point is ground (class 2) and the others are building
    /// (class 6).
    fn annex_ply_with_classes() -> String {
        let (points, _) = room_points();
        let mut text = format!(
            "ply\nformat ascii 1.0\nelement vertex {}\nproperty double x\nproperty double y\n\
             property double z\nproperty uchar classification\nend_header\n",
            points.len()
        );
        for (ordinal, [x, y, z]) in points.iter().enumerate() {
            let class = if ordinal % 2 == 0 { 2 } else { 6 };
            text.push_str(&format!("{:.3} {y:.3} {z:.3} {class}\n", x + 10.0));
        }
        text
    }

    /// The section box of a plan of the room: its top face at 1.1 m.
    const PLAN_BOX: Bounds = Bounds {
        min: [-0.5, -0.5, 0.5],
        max: [4.5, 3.5, 1.1],
    };

    /// A window with the room open as its one layer, and the points the
    /// slab of a plan holds.
    fn studio_with_room(directory: &Path) -> (Studio, u64) {
        let (text, in_slab) = room_xyz();
        let path = directory.join("room.xyz");
        std::fs::write(&path, text).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 1_000).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        (studio, in_slab)
    }

    /// Send a command the way the window receives it, so that the tool
    /// settles after it as after any other message.
    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.update(Message::ApiRequest(ApiRequest { command, reply }));
        receive.recv().unwrap()
    }

    fn status(studio: &mut Studio) -> Value {
        send(studio, ApiCommand::Status)["result"]["drawing"].clone()
    }

    fn job(studio: &mut Studio, id: &str) -> Value {
        send(studio, ApiCommand::Job { id: id.to_owned() })["job"].clone()
    }

    fn set_plan_box(studio: &mut Studio) {
        let answer = send(
            studio,
            ApiCommand::SetSection {
                min: PLAN_BOX.min,
                max: PLAN_BOX.max,
                rotation: None,
            },
        );
        assert_eq!(answer["ok"], true, "{answer}");
    }

    /// What the worker thread of the running job does, and its message to
    /// the window.
    fn finish(studio: &mut Studio) {
        let running = studio.drawing.job.as_ref().expect("a job runs");
        let (serial, input, control) = (
            running.serial,
            Arc::clone(&running.input),
            Arc::clone(&running.control),
        );
        let end = DrawingEnd::of(run(&input, &mut |step| control.report(step)));
        let _ = studio.update(Message::Drawing(DrawingAction::Finished(serial, end)));
    }

    fn preview(studio: &mut Studio) -> Value {
        let accepted = send(
            studio,
            ApiCommand::PreviewDrawing {
                options: DrawingOptions::default(),
            },
        );
        assert_eq!(accepted["ok"], true, "{accepted}");
        finish(studio);
        status(studio)
    }

    #[test]
    fn drawings_of_create_2d_are_kept_listed_after_a_restart_and_made_again() {
        use crate::drawing_view::DrawingViewAction;
        use crate::project_browser::{ViewKind, ViewRow};
        use crate::views::ViewAction;

        // The names are made in the language in use.
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        // A saved view with the box of a plan, to make a section from.
        set_plan_box(&mut studio);
        let _ = studio.update(Message::Views(ViewAction::Name("Room".into())));
        let _ = studio.update(Message::Views(ViewAction::Save));
        let _ = studio.update(Message::SetSectionEnabled(false));

        // A plan from the model, a section from the view and an elevation.
        let commands = [
            r#"{"command":"create_drawing","kind":"plan","height":1.05}"#,
            r#"{"command":"create_drawing","kind":"section","basis":"room","side":"left","thickness":0.6}"#,
            r#"{"command":"create_drawing","kind":"elevation","side":"back","name":"North face"}"#,
        ];
        for command in commands {
            let answer = send(&mut studio, serde_json::from_str(command).unwrap());
            assert_eq!(answer["ok"], true, "{answer}");
            finish(&mut studio);
            let made = job(&mut studio, answer["job_id"].as_str().unwrap());
            assert_eq!(made["state"], "complete", "{made}");
            assert_eq!(made["operation"], "create_drawing");
            assert!(studio.drawing_view.shown);
            assert_eq!(
                studio.drawing_view.shown_guid(),
                made["guid"].as_str(),
                "the drawing made is shown"
            );
        }
        let listed = send(&mut studio, ApiCommand::ListDrawings);
        let names: Vec<&str> = listed["drawings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|drawing| drawing["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Plan +1.05", "Section Left · Room", "North face"]);
        assert_eq!(listed["drawings"][1]["kind"], "section");
        assert_eq!(listed["drawings"][1]["view"], "left");
        assert_eq!(listed["drawings"][1]["thickness"], 0.6);
        assert_eq!(listed["drawings"][2]["shown"], true);
        assert!(listed["drawings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|drawing| drawing["made"] == true));
        // The section keeps the box of the view it was made from, and the
        // plan the cut at its height.
        let saved = studio.drawing_view.saved.clone();
        assert_eq!(saved[1].section.min, PLAN_BOX.min);
        assert_eq!(saved[1].section.max, PLAN_BOX.max);
        assert!((saved[0].section.max[2] - 1.05).abs() < 1e-9);
        assert_eq!(saved[0].request().unwrap().view, DrawingView::Plan);
        let source = camera_views::source_key(&studio.clouds[0].cloud.path);
        assert!(saved
            .iter()
            .all(|drawing| drawing.sources == [source.clone()]));
        assert_eq!(crate::saved_drawings::load(), saved, "kept on disk");

        // Previews, exports and files are limited; the drawings stay.
        for number in 0..20 {
            studio
                .drawing_view
                .set_scene(Arc::new(DrawScene::from_drawing(
                    &Drawing2d::new(DrawingUnits::Metres),
                    ViewSource::File(directory.path().join(format!("{number}.dxf"))),
                )));
        }
        assert_eq!(studio.drawing_view.sheets().len(), 16);
        assert!(saved
            .iter()
            .all(|drawing| studio.drawing_view.made(&drawing.guid).is_some()));
        let groups = studio.view_groups();
        assert_eq!(
            groups.iter().map(|(kind, _)| *kind).collect::<Vec<_>>(),
            [
                ViewKind::ThreeD,
                ViewKind::Plans,
                ViewKind::Elevations,
                ViewKind::Sections,
                ViewKind::Files,
            ]
        );
        assert_eq!(groups[0].1[0], ViewRow::Model);
        let _ = studio.view();

        // After a restart the drawings are kept but not made. Without their
        // scan they are not listed, and making one says what is missing.
        let plan = saved[0].guid.clone();
        let mut restarted = Studio::default();
        assert_eq!(restarted.drawing_view.saved, saved);
        assert!(restarted.listed_drawings().is_empty());
        let Err(refused) = restarted.show_saved_drawing(&plan, None) else {
            panic!("the drawing is made without its scan");
        };
        assert_eq!(refused, "Open room.xyz to make the drawing Plan +1.05");
        let cloud = Arc::new(pointcloud_core::open(&studio.clouds[0].cloud.path, 1_000).unwrap());
        let _ = restarted.update(Message::Loaded(Ok(cloud)));
        assert_eq!(restarted.listed_drawings().len(), 3);
        assert!(restarted.drawing_view.made(&plan).is_none());
        // A click makes it again from how it was made, also with the scan
        // hidden.
        let _ = restarted.update(Message::LayerVisible(0, false));
        let _ = restarted.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            plan.clone(),
        )));
        assert!(restarted.drawing.job.is_some(), "{}", restarted.status);
        finish(&mut restarted);
        assert_eq!(restarted.drawing_view.shown_guid(), Some(plan.as_str()));
        let again = restarted.drawing_view.made(&plan).unwrap().totals();
        assert_eq!(again, studio.drawing_view.made(&plan).unwrap().totals());
        assert_eq!(restarted.drawing_view.saved, saved, "nothing new is kept");
        // A second click shows the drawing made.
        restarted.drawing_view.shown = false;
        let _ = restarted.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            plan.clone(),
        )));
        assert!(restarted.drawing.job.is_none() && restarted.drawing_view.shown);

        // Deleted, it is gone from the list, the view and the disk.
        let deleted = send(
            &mut restarted,
            serde_json::from_str(r#"{"command":"delete_drawing","name":"plan +1.05"}"#).unwrap(),
        );
        assert_eq!(deleted["ok"], true, "{deleted}");
        assert!(!restarted.drawing_view.shown);
        assert_eq!(crate::saved_drawings::load().len(), 2);
        let missing = send(
            &mut restarted,
            serde_json::from_str(r#"{"command":"show_drawing","name":"Plan +1.05"}"#).unwrap(),
        );
        assert_eq!(missing["ok"], false);
        let section = send(
            &mut restarted,
            serde_json::from_str(r#"{"command":"show_drawing","name":"section left · room"}"#)
                .unwrap(),
        );
        assert_eq!(section["accepted"], true, "{section}");
        finish(&mut restarted);
        let made = job(&mut restarted, section["job_id"].as_str().unwrap());
        assert_eq!(made["name"], "Section Left · Room", "{made}");
        let _ = restarted.view();

        // Many missing scans are named briefly.
        let names: Vec<String> = (1..=6).map(|number| format!("s{number}.e57")).collect();
        assert_eq!(named_briefly(&names[..4]), "s1.e57, s2.e57, s3.e57, s4.e57");
        assert_eq!(
            named_briefly(&names),
            "s1.e57, s2.e57, s3.e57 and 3 more scans"
        );
    }

    /// The entities of an ASCII DXF: how many of each kind there are, and
    /// the layers they lie on.
    fn dxf_entities(path: &Path) -> (BTreeMap<String, usize>, Vec<String>) {
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let mut kinds = BTreeMap::new();
        let mut layers = Vec::new();
        let mut in_entities = false;
        for pair in lines.chunks(2) {
            let [code, value] = pair else { break };
            match (*code, *value) {
                ("2", "ENTITIES") => in_entities = true,
                ("0", "ENDSEC") => in_entities = false,
                ("0", kind) if in_entities => *kinds.entry(kind.to_owned()).or_insert(0) += 1,
                ("8", layer) if in_entities && !layers.iter().any(|known| known == layer) => {
                    layers.push(layer.to_owned());
                }
                _ => {}
            }
        }
        layers.sort();
        (kinds, layers)
    }

    #[test]
    fn block_starts_with_the_defaults_of_the_core() {
        let settings = DrawingSettings::default();
        let request = settings.request().unwrap();
        assert_eq!(request, DrawingRequest::default());
        assert_eq!(request.view, DrawingView::Plan);
        assert_eq!(request.thickness, Some(0.10));
        assert!(request.fill && request.square && request.points);
        assert_eq!(request.units, DrawingUnits::Millimetres);
        assert_eq!(request.max_points, 150_000);
        assert_eq!(settings.thickness, "0.10");

        // Every list of the block offers every value the core knows, and
        // each has a name of its own.
        let names: Vec<&str> = DrawingView::ALL.into_iter().map(view_text).collect();
        assert_eq!(names.len(), 5);
        assert!(names
            .iter()
            .all(|name| names.iter().filter(|other| *other == name).count() == 1));
        assert_eq!(
            DrawingVersion::ALL.map(version_text),
            ["R2004", "R2010", "R2013", "R2018"]
        );
        let offered: Vec<DrawingFormat> = FORMATS.iter().map(|(format, _)| *format).collect();
        assert_eq!(offered, DrawingFormat::ALL);
    }

    #[test]
    fn typed_numbers_are_read_with_a_comma_or_a_point_and_checked() {
        let mut settings = DrawingSettings {
            thickness: " 0,25 ".into(),
            max_points: "40.000".into(),
            ..DrawingSettings::default()
        };
        let request = settings.request().unwrap();
        assert_eq!(request.thickness, Some(0.25));
        assert_eq!(request.max_points, 40_000);

        settings.thickness = "thick".into();
        assert_eq!(
            settings.request().unwrap_err(),
            "Slab thickness must be a number of metres"
        );
        settings.thickness = "9".into();
        assert_eq!(
            settings.request().unwrap_err(),
            "Slab thickness must be between 0.005 and 5 m"
        );
        settings.thickness = "0.1".into();
        settings.max_points = (MAX_DRAWING_POINTS + 1).to_string();
        assert!(settings
            .request()
            .unwrap_err()
            .starts_with("A drawing holds between 1 and"));
        settings.max_points = "many".into();
        assert_eq!(
            settings.request().unwrap_err(),
            "Point limit must be a whole number"
        );
        settings.max_points = "1000".into();
        // A thin largest wall takes the smallest wall down with it.
        settings.max_wall = "0.03".into();
        let request = settings.request().unwrap();
        assert_eq!(request.max_wall_thickness, 0.03);
        assert_eq!(request.min_wall_thickness, 0.03);
        settings.max_wall = "3".into();
        assert_eq!(
            settings.request().unwrap_err(),
            "Largest wall thickness must be at most 2 m"
        );
        assert_eq!(settings.value()["thickness"], 0.1);
        settings.grid = "fine".into();
        assert_eq!(settings.value()["grid"], Value::Null);
    }

    #[test]
    fn cut_settings_are_those_of_the_request_and_ignore_the_point_limit() {
        let mut settings = DrawingSettings::default();
        for change in [
            |_: &mut DrawingSettings| {},
            |settings: &mut DrawingSettings| settings.set_view(DrawingView::Left),
            |settings: &mut DrawingSettings| settings.thickness = "0,25".into(),
            |settings: &mut DrawingSettings| settings.square = false,
            |settings: &mut DrawingSettings| settings.grid = "0.04".into(),
            // A thin largest wall takes the smallest wall down with it.
            |settings: &mut DrawingSettings| settings.max_wall = "0.03".into(),
        ] {
            change(&mut settings);
            let request = settings.request().unwrap();
            assert_eq!(settings.cut(), Some(cut_of(&request)), "{settings:?}");
            // A point limit that is being typed, or that the core would
            // refuse, leaves the cut what it is.
            for limit in ["", "many", "0", "9999999"] {
                let typing = DrawingSettings {
                    max_points: limit.into(),
                    ..settings.clone()
                };
                assert!(typing.request().is_err(), "{limit}");
                assert_eq!(typing.cut(), settings.cut(), "{limit}");
            }
        }
        // A field of the cut that holds no number gives no cut.
        for unreadable in [
            DrawingSettings {
                thickness: String::new(),
                ..settings.clone()
            },
            DrawingSettings {
                grid: "fine".into(),
                ..settings.clone()
            },
            DrawingSettings {
                max_wall: "-".into(),
                ..settings.clone()
            },
        ] {
            assert_eq!(unreadable.cut(), None);
        }
    }

    #[test]
    fn another_view_starts_with_its_own_filled_cut() {
        let mut settings = DrawingSettings::default();
        assert!(settings.fill);
        settings.set_view(DrawingView::Front);
        assert!(!settings.fill, "a vertical section starts as points only");
        settings.fill = true;
        settings.set_view(DrawingView::Front);
        assert!(settings.fill, "choosing the same view keeps the choice");
        settings.set_view(DrawingView::Plan);
        assert!(settings.fill);
        settings.fill = false;
        settings.set_view(DrawingView::Back);
        settings.set_view(DrawingView::Plan);
        assert!(settings.fill, "a plan starts filled");
    }

    #[test]
    fn fields_of_a_command_go_into_the_block_and_wrong_ones_change_nothing() {
        let settings = DrawingSettings::default();
        let next = settings
            .with(&DrawingOptions {
                view: Some("Front".into()),
                thickness: Some(0.5),
                units: Some("m".into()),
                origin: Some("box".into()),
                square: Some(false),
                grid: Some(0.04),
                max_wall_thickness: Some(0.8),
                color: Some("rgb".into()),
                point_layers: Some("class".into()),
                max_points: Some(20_000),
                version: Some("r2018".into()),
                fill: None,
                straight_lines: Some(true),
                line_tolerance: Some(0.02),
                line_min_length: Some(0.15),
            })
            .unwrap();
        let request = next.request().unwrap();
        assert_eq!(request.view, DrawingView::Front);
        assert!(!request.fill, "the view brought its own default");
        assert_eq!(request.thickness, Some(0.5));
        assert_eq!(request.units, DrawingUnits::Metres);
        assert_eq!(request.origin, DrawingOrigin::BoxCorner);
        assert!(!request.square);
        assert_eq!((request.grid, request.max_wall_thickness), (0.04, 0.8));
        assert_eq!(request.color, PointColor::Rgb);
        assert_eq!(request.point_layers, PointLayers::Class);
        assert_eq!(request.max_points, 20_000);
        assert_eq!(request.version, DrawingVersion::R2018);
        assert_eq!(
            next.value(),
            json!({
                "view": "front", "thickness": 0.5, "units": "m", "origin": "box",
                "fill": false, "square": false, "grid": 0.04, "max_wall_thickness": 0.8,
                "color": "rgb", "point_layers": "class", "max_points": 20_000,
                "version": "r2018",
                "straight_lines": true, "line_tolerance": 0.02, "line_min_length": 0.15,
            })
        );
        // A fill that is named wins over the default of the view.
        let filled = settings
            .with(&DrawingOptions {
                view: Some("left".into()),
                fill: Some(true),
                ..DrawingOptions::default()
            })
            .unwrap();
        assert!(filled.fill);
        // Nothing named keeps everything.
        assert_eq!(settings.with(&DrawingOptions::default()).unwrap(), settings);

        for (options, problem) in [
            (
                DrawingOptions {
                    view: Some("top".into()),
                    ..DrawingOptions::default()
                },
                "view must be plan, front, back, left or right",
            ),
            (
                DrawingOptions {
                    units: Some("cm".into()),
                    ..DrawingOptions::default()
                },
                "units must be mm or m",
            ),
            (
                DrawingOptions {
                    version: Some("r12".into()),
                    ..DrawingOptions::default()
                },
                "version must be r2004, r2010, r2013 or r2018",
            ),
        ] {
            assert_eq!(settings.with(&options).unwrap_err(), problem);
        }
    }

    #[test]
    fn commands_are_read_with_the_fields_they_name() {
        let command: ApiCommand = serde_json::from_value(json!({
            "command": "export_drawing", "path": "/out/plan.dwg", "view": "plan",
            "thickness": 1, "fill": false, "max_points": 5000,
        }))
        .unwrap();
        let ApiCommand::ExportDrawing { path, options } = command else {
            panic!("another command was read");
        };
        assert_eq!(path, PathBuf::from("/out/plan.dwg"));
        assert_eq!(
            options,
            DrawingOptions {
                view: Some("plan".into()),
                thickness: Some(1.0),
                fill: Some(false),
                max_points: Some(5_000),
                ..DrawingOptions::default()
            }
        );
        let command: ApiCommand =
            serde_json::from_value(json!({"command": "preview_drawing"})).unwrap();
        assert!(matches!(
            command,
            ApiCommand::PreviewDrawing { options } if options == DrawingOptions::default()
        ));
        for name in ["clear_drawing_preview", "cancel_drawing"] {
            assert!(serde_json::from_value::<ApiCommand>(json!({"command": name})).is_ok());
        }
        assert!(
            serde_json::from_value::<ApiCommand>(json!({"command": "export_drawing"})).is_err(),
            "an export needs a path"
        );
    }

    #[test]
    fn result_line_says_what_the_core_reports() {
        let request = DrawingRequest::default();
        let stats = DrawingStats {
            slab_points: 1_822_308,
            read_points: 4_000_000,
            reused_points: 0,
            drawn_points: 65_637,
            point_spacing: 0.04,
            regions: 3,
            vertices: 40,
            dropped_regions: 2,
            grid_cell: Some(0.04),
            direction_degrees: Some(-17.304),
            bytes: 5_700_000,
            straight_lines: None,
        };
        let plain = |count: u64| count.to_string();
        assert_eq!(
            summary(&stats, &request, 0.10, &plain),
            "1822308 points in the slab, 65637 drawn at 40 mm (raised from 5 mm by the limit \
             of 150000 points); 3 regions (2 small ones dropped), grid 40 mm (coarser than the \
             20 mm asked), main direction -17.30°"
        );
        let value = stats_value(&stats, &request, 0.10);
        assert_eq!(value["point_spacing_raised"], true);
        assert_eq!(value["grid_cell_raised"], true);
        assert_eq!(value["dropped_regions"], 2);
        assert_eq!(value["thickness"], 0.10);

        // Nothing raised, one region, no fill at all, and a direction that
        // rounds to zero from below.
        let exact = DrawingStats {
            point_spacing: 0.005,
            regions: 1,
            dropped_regions: 1,
            grid_cell: Some(0.02),
            direction_degrees: Some(-0.001),
            ..stats
        };
        assert_eq!(
            summary(&exact, &request, 0.10, &plain),
            "1822308 points in the slab, 65637 drawn at 5 mm; 1 region (1 small one dropped), \
             grid 20 mm, main direction 0.00°"
        );
        let points_only = DrawingStats {
            grid_cell: None,
            direction_degrees: None,
            regions: 0,
            dropped_regions: 0,
            ..exact
        };
        assert_eq!(
            summary(&points_only, &request, 0.10, &plain),
            "1822308 points in the slab, 65637 drawn at 5 mm"
        );
        // A box that is shallower than the slab asked: the line and the
        // figures give the depth that was drawn.
        assert_eq!(
            summary(&points_only, &request, 0.05, &plain),
            "1822308 points in the slab of 50 mm (the box is thinner than the 100 mm asked), \
             65637 drawn at 5 mm"
        );
        assert_eq!(stats_value(&points_only, &request, 0.05)["thickness"], 0.05);
        assert_eq!(result_rows(&points_only, &request, 0.10).len(), 3);
        assert_eq!(result_rows(&points_only, &request, 0.05).len(), 4);
        let preview = DrawingStats {
            drawn_points: 0,
            point_spacing: 0.0,
            ..exact
        };
        assert!(
            summary(&preview, &request, 0.10, &plain).starts_with("1822308 points in the slab; 1")
        );
        assert_eq!(
            stats_value(&preview, &request, 0.10)["point_spacing_raised"],
            false
        );

        // The status bar groups the digits as the rest of the window does.
        let last = Last::Exported {
            path: PathBuf::from("plan.dxf"),
            format: DrawingFormat::Dxf,
            request,
            slab: 0.10,
            stats,
        };
        assert!(
            last.status().starts_with(
                "Section drawing exported as DXF: 1.822.308 points in the slab, 65.637 drawn"
            ),
            "{}",
            last.status()
        );
        assert!(last.status().ends_with("; 5.7 MB to plan.dxf"));
        assert_eq!(size_text(790_000), "790 kB");
        assert_eq!(size_text(12), "1 kB");
    }

    #[test]
    fn worker_reports_its_stage_and_hears_a_cancel() {
        let control = DrawingControl::default();
        assert_eq!(control.snapshot().stage, DrawingStage::Reading);
        for (stage, done, total, fraction) in [
            (DrawingStage::Reading, 4_096, 16_384, Some(0.25)),
            (DrawingStage::Tracing, 0, 0, None),
            (DrawingStage::Writing, 30, 40, Some(0.75)),
        ] {
            let step = DrawingProgress { stage, done, total };
            assert!(control.report(step).is_ok());
            assert_eq!(control.snapshot(), step);
            assert_eq!(control.snapshot().fraction(), fraction);
        }
        assert_eq!(
            [
                DrawingStage::Reading,
                DrawingStage::Tracing,
                DrawingStage::Writing
            ]
            .map(stage_key),
            ["reading", "tracing", "writing"]
        );
        // A cancel is heard at the next report, which then changes nothing.
        control.cancelled.store(true, Ordering::Relaxed);
        let late = DrawingProgress {
            stage: DrawingStage::Writing,
            done: 40,
            total: 40,
        };
        assert!(matches!(control.report(late), Err(LoadError::Cancelled)));
        assert_eq!(control.snapshot().done, 30);
        assert!(matches!(
            DrawingEnd::of(Err(LoadError::Cancelled)),
            DrawingEnd::Cancelled
        ));
        // The reason of a failure comes without the words the core puts
        // before every refusal of data.
        assert!(matches!(
            DrawingEnd::of(Err(LoadError::InvalidData("the slab holds no points".into()))),
            DrawingEnd::Failed(reason) if reason == "the slab holds no points"
        ));
    }

    #[test]
    fn api_refuses_what_cannot_be_drawn_and_changes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("plan.dxf");
        let export = |studio: &mut Studio, path: &Path, options: DrawingOptions| {
            send(
                studio,
                ApiCommand::ExportDrawing {
                    path: path.to_path_buf(),
                    options,
                },
            )
        };
        let plain = DrawingOptions::default;
        let (mut studio, _) = studio_with_room(directory.path());

        for refused in [
            PathBuf::from("plan.dxf"),
            directory.path().join("plan.pdf"),
            directory.path().join("plan"),
        ] {
            assert_eq!(
                export(&mut studio, &refused, plain())["error"],
                "export_drawing requires an absolute .dxf or .dwg destination",
                "{}",
                refused.display()
            );
        }
        assert_eq!(
            export(&mut studio, &destination, plain())["error"],
            "section box is not enabled"
        );
        assert_eq!(
            send(&mut studio, ApiCommand::PreviewDrawing { options: plain() })["error"],
            "section box is not enabled"
        );
        set_plan_box(&mut studio);

        let wrong_view = DrawingOptions {
            view: Some("above".into()),
            units: Some("m".into()),
            ..plain()
        };
        assert_eq!(
            export(&mut studio, &destination, wrong_view)["error"],
            "view must be plan, front, back, left or right"
        );
        let too_thick = DrawingOptions {
            thickness: Some(-1.0),
            units: Some("m".into()),
            ..plain()
        };
        assert_eq!(
            export(&mut studio, &destination, too_thick)["error"],
            "slab thickness must be between 0.005 and 5 m"
        );
        assert_eq!(
            export(
                &mut studio,
                &directory.path().join("missing/plan.dwg"),
                plain()
            )["error"],
            "the folder of the export_drawing destination does not exist"
        );
        // The room itself is not written over. An XYZ file is no drawing
        // format, so a DXF scan stands in for it here.
        let scan = directory.path().join("scan.dxf");
        std::fs::write(
            &scan,
            "0\nSECTION\n2\nENTITIES\n0\nPOINT\n10\n1\n20\n1\n30\n1\n0\nENDSEC\n0\nEOF\n",
        )
        .unwrap();
        let cloud = Arc::new(pointcloud_core::open(&scan, 10).unwrap());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        set_plan_box(&mut studio);
        assert_eq!(
            export(&mut studio, &scan, plain())["error"],
            "export_drawing requires a destination different from the open scans"
        );

        let _ = studio.update(Message::SetVisible(0, false));
        let _ = studio.update(Message::SetVisible(1, false));
        assert_eq!(
            export(&mut studio, &destination, plain())["error"],
            "no visible point cloud to draw"
        );

        // A refusal starts no job and leaves the block as it was.
        assert!(studio.api_jobs.is_empty());
        assert!(!studio.drawing.busy());
        assert_eq!(studio.drawing.settings, DrawingSettings::default());
        assert!(!destination.exists());
        assert_eq!(
            send(&mut studio, ApiCommand::CancelDrawing)["error"],
            "no section drawing is running"
        );
        assert_eq!(
            send(&mut studio, ApiCommand::ClearDrawingPreview),
            json!({"ok": true, "cleared": false})
        );
    }

    #[test]
    fn layer_that_is_still_loading_is_refused_by_name_before_anything_is_read() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, in_slab) = studio_with_room(directory.path());
        let annex = directory.path().join("annex.ply");
        std::fs::write(&annex, annex_ply_with_classes()).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&annex, 1_000).unwrap());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        set_plan_box(&mut studio);
        // The second scan as it stands while it is read: shown, without an
        // index, and its cloud not yet checked against the file.
        let mut loading = (*studio.clouds[1].cloud).clone();
        loading.provisional = true;
        studio.clouds[1].cloud = Arc::new(loading);
        assert!(studio.clouds[1].index.is_none());

        let preview = |studio: &mut Studio| {
            send(
                studio,
                ApiCommand::PreviewDrawing {
                    options: DrawingOptions::default(),
                },
            )
        };
        assert_eq!(
            preview(&mut studio)["error"],
            "a visible point cloud is still loading: annex.ply"
        );
        assert_eq!(
            send(
                &mut studio,
                ApiCommand::ExportDrawing {
                    path: directory.path().join("plan.dxf"),
                    options: DrawingOptions::default(),
                },
            )["error"],
            "a visible point cloud is still loading: annex.ply"
        );
        let said = "annex.ply is still loading; wait for it or hide it before making a drawing";
        let _ = studio.update(Message::Drawing(DrawingAction::Preview));
        assert_eq!(studio.status, said);
        let _ = studio.update(Message::Drawing(DrawingAction::Export));
        assert_eq!(studio.status, said);
        // Nothing started: no job, no save dialog, no file.
        assert!(studio.api_jobs.is_empty());
        assert!(!studio.drawing.busy());
        assert!(!directory.path().join("plan.dxf").exists());

        // With the loading scan hidden the others are drawn.
        let _ = studio.update(Message::SetVisible(1, false));
        let accepted = preview(&mut studio);
        assert_eq!(accepted["ok"], true, "{accepted}");
        finish(&mut studio);
        assert_eq!(status(&mut studio)["last"]["slab_points"], in_slab);
    }

    #[test]
    fn job_reports_the_slab_that_was_drawn_in_a_box_thinner_than_asked() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, in_slab) = studio_with_room(directory.path());
        // A box of 0.05 m under the cut plane, and a slab of 0.10 m asked.
        let answer = send(
            &mut studio,
            ApiCommand::SetSection {
                min: [PLAN_BOX.min[0], PLAN_BOX.min[1], 1.05],
                max: PLAN_BOX.max,
                rotation: None,
            },
        );
        assert_eq!(answer["ok"], true, "{answer}");
        let destination = directory.path().join("thin.dxf");
        let accepted = send(
            &mut studio,
            ApiCommand::ExportDrawing {
                path: destination.clone(),
                options: DrawingOptions {
                    thickness: Some(0.10),
                    ..DrawingOptions::default()
                },
            },
        );
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        finish(&mut studio);
        let done = job(&mut studio, &id);
        assert_eq!(done["state"], "complete", "{done}");
        let drawn = done["thickness"].as_f64().unwrap();
        assert!((drawn - 0.05).abs() < 1e-6, "{drawn}");
        // Two of the four heights of the full slab lie in this box.
        assert_eq!(done["slab_points"], in_slab / 2);
        assert_eq!(status(&mut studio)["last"]["thickness"], done["thickness"]);
        // The block still holds what was asked.
        assert_eq!(status(&mut studio)["settings"]["thickness"], 0.1);
        assert!(
            studio
                .status
                .contains("points in the slab of 50 mm (the box is thinner than the 100 mm asked)"),
            "{}",
            studio.status
        );
        // The text in the file says the same depth.
        let text = std::fs::read_to_string(&destination).unwrap();
        assert!(text.contains("slab 0.050 m"));
        let _ = studio.view();

        // A preview reports it the same way.
        let shown = preview(&mut studio);
        let previewed = shown["last"]["thickness"].as_f64().unwrap();
        assert!((previewed - 0.05).abs() < 1e-6, "{previewed}");
    }

    #[test]
    fn api_exports_a_plan_of_the_room_as_a_job() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, in_slab) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        assert_eq!(status(&mut studio)["job"], Value::Null);
        assert_eq!(status(&mut studio)["last"], Value::Null);

        for (name, units) in [("plan.dxf", "mm"), ("plan.dwg", "m")] {
            let destination = directory.path().join(name);
            let accepted = send(
                &mut studio,
                ApiCommand::ExportDrawing {
                    path: destination.clone(),
                    options: DrawingOptions {
                        units: Some(units.into()),
                        ..DrawingOptions::default()
                    },
                },
            );
            assert_eq!(accepted["ok"], true, "{accepted}");
            assert_eq!(accepted["accepted"], true);
            assert_eq!(accepted["path"], json!(destination));
            let id = accepted["job_id"].as_str().unwrap().to_owned();
            let running = job(&mut studio, &id);
            assert_eq!(running["state"], "running");
            assert_eq!(running["operation"], "export_drawing");

            // A wait for an idle window sees the drawing as work under way,
            // and so does the strip above the scene.
            let state = send(&mut studio, ApiCommand::Status)["result"].clone();
            assert_eq!(crate::mcp::busy(&state), ["drawing"]);
            assert_eq!(state["drawing"]["job"]["stage"], "reading");
            assert_eq!(state["drawing"]["settings"]["units"], units);
            let lines = studio.progress_lines();
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0].phase, Phase::Drawing);
            assert_eq!(lines[0].title, "Section drawing (plan)");
            assert!(lines[0].detail.starts_with("Step 1 of 3"));
            assert!(studio.progress_strip().is_some());
            // Tracing the filled cut is the second step, writing the third.
            let control = Arc::clone(&studio.drawing.job.as_ref().unwrap().control);
            for (stage, step) in [
                (DrawingStage::Tracing, "Step 2 of 3"),
                (DrawingStage::Writing, "Step 3 of 3"),
                (DrawingStage::Reading, "Step 1 of 3"),
            ] {
                let (done, total) = (0, 0);
                control
                    .report(DrawingProgress { stage, done, total })
                    .unwrap();
                let detail = studio.progress_lines().remove(0).detail;
                assert!(detail.starts_with(step), "{detail}");
            }
            // No second job beside it.
            assert_eq!(
                send(
                    &mut studio,
                    ApiCommand::PreviewDrawing {
                        options: DrawingOptions::default()
                    }
                )["error"],
                "a section drawing is already open or running"
            );

            finish(&mut studio);
            let done = job(&mut studio, &id);
            assert_eq!(done["state"], "complete", "{done}");
            assert_eq!(done["operation"], "export_drawing");
            assert_eq!(done["format"], &name[5..]);
            assert_eq!(done["view"], "plan");
            assert_eq!(done["units"], units);
            // The box is deeper than the slab: what was asked was drawn.
            assert_eq!(done["thickness"], 0.1);
            assert!(!studio.status.contains("thinner"), "{}", studio.status);
            assert_eq!(done["slab_points"], in_slab);
            let drawn = done["drawn_points"].as_u64().unwrap();
            assert!(drawn > 1_000 && drawn <= in_slab, "{drawn}");
            assert_eq!(done["point_spacing"], 0.005);
            assert_eq!(done["point_spacing_raised"], false);
            // The walls are one region: the door leaves the ring open.
            assert_eq!(done["regions"], 1);
            assert_eq!(done["grid_cell"], 0.02);
            assert_eq!(done["grid_cell_raised"], false);
            assert_eq!(
                done["bytes"].as_u64().unwrap(),
                std::fs::metadata(&destination).unwrap().len()
            );
            let state = send(&mut studio, ApiCommand::Status)["result"].clone();
            assert!(crate::mcp::busy(&state).is_empty());
            assert_eq!(state["drawing"]["last"], done);
            assert!(studio.progress_lines().is_empty());
            assert!(
                studio.status.starts_with(&format!(
                    "Section drawing exported as {}: ",
                    name[5..].to_uppercase()
                )),
                "{}",
                studio.status
            );
            assert!(studio
                .status
                .contains("1 region, grid 20 mm, main direction 0.00°"));

            if name.ends_with(".dxf") {
                // The file as the reader of the core sees it: the points of
                // the drawing, a thousand times the scan in millimetres.
                let read = pointcloud_core::open(&destination, 10).unwrap();
                assert_eq!(read.total_points, drawn);
                assert!(
                    (read.bounds.max[0] - 4_100.0).abs() < 1.0,
                    "{:?}",
                    read.bounds
                );
                assert!(
                    (read.bounds.min[1] + 100.0).abs() < 1.0,
                    "{:?}",
                    read.bounds
                );
                // And as its text says it.
                let (kinds, layers) = dxf_entities(&destination);
                assert_eq!(kinds["POINT"] as u64, drawn);
                assert_eq!(kinds["HATCH"], 1);
                assert_eq!(kinds["TEXT"], 1);
                // The outline of the region and the frame of the box.
                assert_eq!(kinds["LWPOLYLINE"], 2);
                assert_eq!(
                    layers,
                    [
                        "OPS-CUT-FILL",
                        "OPS-CUT-OUTLINE",
                        "OPS-FRAME",
                        "OPS-INFO",
                        "OPS-POINTS"
                    ]
                );
            } else {
                let bytes = std::fs::read(&destination).unwrap();
                assert!(bytes.starts_with(DrawingVersion::R2013.tag().as_bytes()));
            }
        }
        // The block stands open with what was drawn, in either language.
        assert!(studio.drawing.open);
        let _language = TestLanguage::hold(Language::Table(0));
        assert!(studio.drawing_properties().is_some());
        let _ = studio.view();
    }

    #[test]
    fn point_limit_raises_the_spacing_and_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        let accepted = send(
            &mut studio,
            ApiCommand::ExportDrawing {
                path: directory.path().join("few.dxf"),
                options: DrawingOptions {
                    max_points: Some(500),
                    fill: Some(false),
                    ..DrawingOptions::default()
                },
            },
        );
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        // Without a filled cut nothing is traced: reading and writing are
        // the two steps of the job.
        let detail = |studio: &Studio| studio.progress_lines().remove(0).detail;
        assert!(
            detail(&studio).starts_with("Step 1 of 2"),
            "{}",
            detail(&studio)
        );
        let control = Arc::clone(&studio.drawing.job.as_ref().unwrap().control);
        let writing = DrawingProgress {
            stage: DrawingStage::Writing,
            done: 1,
            total: 2,
        };
        control.report(writing).unwrap();
        assert!(
            detail(&studio).starts_with("Step 2 of 2"),
            "{}",
            detail(&studio)
        );
        finish(&mut studio);
        let done = job(&mut studio, &id);
        assert_eq!(done["state"], "complete", "{done}");
        assert!(done["drawn_points"].as_u64().unwrap() <= 500);
        assert!(done["point_spacing"].as_f64().unwrap() > 0.005);
        assert_eq!(done["point_spacing_raised"], true);
        // Without a fill there are no regions and no grid to report.
        assert_eq!(done["grid_cell"], Value::Null);
        assert!(
            studio.status.contains("by the limit of 500 points"),
            "{}",
            studio.status
        );
        assert!(!studio.status.contains("region"));
        let (kinds, layers) = dxf_entities(&directory.path().join("few.dxf"));
        assert!(!kinds.contains_key("HATCH"));
        assert_eq!(layers, ["OPS-FRAME", "OPS-INFO", "OPS-POINTS"]);
    }

    #[test]
    fn every_visible_layer_is_drawn_without_its_hidden_classes() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, in_slab) = studio_with_room(directory.path());
        // The same room once more beside it, as a second scan with classes.
        let annex = directory.path().join("annex.ply");
        std::fs::write(&annex, annex_ply_with_classes()).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&annex, 1_000).unwrap());
        assert!(cloud.has_classification);
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let around_both = send(
            &mut studio,
            ApiCommand::SetSection {
                min: PLAN_BOX.min,
                max: [14.5, PLAN_BOX.max[1], PLAN_BOX.max[2]],
                rotation: None,
            },
        );
        assert_eq!(around_both["ok"], true, "{around_both}");
        let slab_points =
            |studio: &mut Studio| preview(studio)["last"]["slab_points"].as_u64().unwrap();

        assert_eq!(slab_points(&mut studio), 2 * in_slab);
        // Hiding the ground leaves half of the scan that has classes; the
        // scan without classes is not touched by it.
        let _ = studio.update(Message::FilterClass(2, false));
        let without_ground = slab_points(&mut studio);
        assert!(
            (without_ground * 2).abs_diff(3 * in_slab) <= 8,
            "{without_ground} of {in_slab} twice"
        );
        let _ = studio.update(Message::SetVisible(0, false));
        let one_layer = slab_points(&mut studio);
        assert!((one_layer * 2).abs_diff(in_slab) <= 8, "{one_layer}");
        let _ = studio.update(Message::FilterClass(2, true));
        assert_eq!(slab_points(&mut studio), in_slab);

        // Two scans get a point layer each, named after their files.
        let _ = studio.update(Message::SetVisible(0, true));
        let destination = directory.path().join("both.dxf");
        let accepted = send(
            &mut studio,
            ApiCommand::ExportDrawing {
                path: destination.clone(),
                options: DrawingOptions {
                    fill: Some(false),
                    ..DrawingOptions::default()
                },
            },
        );
        assert_eq!(accepted["ok"], true, "{accepted}");
        finish(&mut studio);
        let (_, layers) = dxf_entities(&destination);
        assert_eq!(
            layers,
            [
                "OPS-FRAME",
                "OPS-INFO",
                "OPS-POINTS-annex",
                "OPS-POINTS-room"
            ]
        );
        // One layer per class instead: the scan without classes keeps the
        // plain point layer.
        let accepted = send(
            &mut studio,
            ApiCommand::ExportDrawing {
                path: destination.clone(),
                options: DrawingOptions {
                    point_layers: Some("class".into()),
                    ..DrawingOptions::default()
                },
            },
        );
        assert_eq!(accepted["ok"], true, "{accepted}");
        finish(&mut studio);
        let (_, layers) = dxf_entities(&destination);
        assert_eq!(
            layers,
            [
                "OPS-FRAME",
                "OPS-INFO",
                "OPS-POINTS",
                "OPS-POINTS-CLASS-02",
                "OPS-POINTS-CLASS-06"
            ]
        );
    }

    #[test]
    fn moved_layers_and_deleted_points_are_drawn_as_the_scene_has_them() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, in_slab) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        // The layer moved up by a metre is no longer cut by the box.
        studio.clouds[0].transform.offset[2] = 1.0;
        let accepted = send(
            &mut studio,
            ApiCommand::PreviewDrawing {
                options: DrawingOptions::default(),
            },
        );
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        finish(&mut studio);
        let failed = job(&mut studio, &id);
        assert_eq!(failed["state"], "failed");
        // The reason names the face that is the cut plane of a plan.
        let reason = format!("{EMPTY_SLAB}: {}", empty_slab_hint(DrawingView::Plan));
        assert_eq!(failed["error"], reason.as_str());
        assert!(reason.contains("the top face of the section box"));
        assert_eq!(
            studio.status,
            format!("Preview of the filled cut failed: {reason}")
        );
        // The block says why the last job gave nothing.
        assert!(matches!(studio.drawing.last, Some(Last::Failed { .. })));
        let _ = studio.view();
        studio.clouds[0].transform.offset[2] = 0.0;

        // Every second point deleted halves what the slab holds.
        let total = studio.clouds[0].cloud.total_points;
        let every_second = crate::selection::SelectionMask {
            bits: vec![0x5555_5555_5555_5555; total.div_ceil(64) as usize],
            count: 0,
            highlights: Vec::new(),
            highlights_source: true,
            source_bounds: None,
        };
        let mut deleted = DeletionMask::new(total).unwrap();
        deleted.apply(&every_second).unwrap();
        studio.clouds[0].deleted = Some(Arc::new(deleted));
        let accepted = send(
            &mut studio,
            ApiCommand::PreviewDrawing {
                options: DrawingOptions::default(),
            },
        );
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        finish(&mut studio);
        let done = job(&mut studio, &id);
        assert_eq!(done["state"], "complete", "{done}");
        let remaining = done["slab_points"].as_u64().unwrap();
        // Each of the four heights in the slab keeps half of its points,
        // give or take one.
        assert!(
            (remaining * 2).abs_diff(in_slab) <= 8,
            "{remaining} of {in_slab}"
        );
    }

    #[test]
    fn preview_lies_on_the_cut_plane_and_goes_when_the_scene_changes() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, in_slab) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        assert_eq!(status(&mut studio)["preview_shown"], false);

        let shown = preview(&mut studio);
        assert_eq!(shown["preview_shown"], true, "{shown}");
        assert_eq!(shown["preview_regions"], 1);
        assert_eq!(shown["last"]["operation"], "preview_drawing");
        assert_eq!(shown["last"]["state"], "complete");
        assert_eq!(shown["last"]["slab_points"], in_slab);
        // A preview also builds the drawing an export would write, for the
        // Drawing view, without switching to it.
        let drawn = shown["last"]["drawn_points"].as_u64().unwrap();
        assert!(drawn > 0);
        let view = studio
            .drawing_view
            .scene()
            .expect("the preview is in the view");
        assert_eq!(view.totals()[0] as u64, drawn);
        assert_eq!(view.totals()[2], 1, "a plan holds its filled cut");
        assert!(!studio.drawing_view.shown);
        assert!(studio.status.starts_with("Filled cut previewed: "));

        // The regions lie on the top face of the box, around the room.
        let cut = studio.drawing.overlay().expect("a preview is shown");
        let region = &cut.regions[0];
        assert!(region.holes.is_empty(), "the door leaves the walls open");
        for xyz in &region.outer {
            assert!((xyz[2] - PLAN_BOX.max[2]).abs() < 1e-6, "{xyz:?}");
            assert!((-0.11..=4.11).contains(&xyz[0]) && (-0.11..=3.11).contains(&xyz[1]));
        }
        // From above, every ring is on screen and can be filled.
        let _ = studio.update(Message::CameraPreset(crate::CameraPreset::Top));
        assert!(
            studio.drawing.overlay().is_some(),
            "the camera is no scene change"
        );
        let size = studio.viewport_size;
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let projection = studio.projection(scene, size.width, size.height);
        let outer = studio.drawing.overlay().unwrap().regions[0].outer.clone();
        let ring = screen_ring(projection, &outer, size);
        assert_eq!(ring.len(), outer.len(), "nothing is cut off");
        assert!(ring.len() >= 8);
        for point in &ring {
            assert!(
                (0.0..=size.width).contains(&point.x) && (0.0..=size.height).contains(&point.y),
                "{point:?} outside {size:?}"
            );
        }
        let _ = studio.view();

        // Whether a pixel is filled: inside the ring by the even-odd rule.
        let filled = |ring: &[UiPoint], x: f32, y: f32| {
            let mut inside = false;
            for (index, from) in ring.iter().enumerate() {
                let to = ring[(index + 1) % ring.len()];
                if (from.y > y) != (to.y > y)
                    && x < from.x + (y - from.y) / (to.y - from.y) * (to.x - from.x)
                {
                    inside = !inside;
                }
            }
            inside
        };
        let within = |ring: &[UiPoint], size: Size| {
            ring.iter().all(|point| {
                (-16.0..=size.width + 16.0).contains(&point.x)
                    && (-16.0..=size.height + 16.0).contains(&point.y)
            })
        };
        // A spot in the wall at x = 4 and one on the floor of the room, both
        // on the cut plane.
        let (in_wall, in_room) = ([4.05, 1.5, PLAN_BOX.max[2]], [3.0, 1.5, PLAN_BOX.max[2]]);
        let on_screen = |projection: Projection, xyz: [f64; 3]| {
            let (x, y, _) = projection.project_unclipped(xyz).unwrap();
            (x, y)
        };
        let (x, y) = on_screen(projection, in_wall);
        assert!(filled(&ring, x, y));
        let (x, y) = on_screen(projection, in_room);
        assert!(!filled(&ring, x, y));

        // Zoomed in on that wall until it covers the whole viewport, the
        // corners of the room lie far more than a hundred thousand pixels
        // away: the ring is cut off at the viewport and still fills it.
        let zoom = 0.000_5;
        let centred = Projection::new(
            scene,
            studio.yaw,
            studio.pitch,
            zoom,
            [0.0, 0.0],
            size.width,
            size.height,
        );
        let (x, y) = on_screen(centred, in_wall);
        let pan = [size.width / 2.0 - x, size.height / 2.0 - y];
        let close = Projection::new(
            scene,
            studio.yaw,
            studio.pitch,
            zoom,
            pan,
            size.width,
            size.height,
        );
        assert!(outer.iter().any(|xyz| {
            let (x, y) = on_screen(close, *xyz);
            x.abs().max(y.abs()) > 100_000.0
        }));
        let ring = screen_ring(close, &outer, size);
        assert!(ring.len() >= 4 && within(&ring, size), "{ring:?}");
        for (x, y) in [
            (1.0, 1.0),
            (size.width / 2.0, size.height / 2.0),
            (size.width - 1.0, size.height - 1.0),
        ] {
            assert!(filled(&ring, x, y), "{x}, {y}");
        }

        // A walking camera stands in the room, 0.5 m above the cut plane,
        // and looks along +X and down: half of the walls lie behind the eye.
        // The wall in front of it is filled and the floor before it is not.
        let (sin, cos) = 0.5_f64.sin_cos();
        let eye = [2.0, 1.5, PLAN_BOX.max[2] + 0.5];
        let basis = [[0.0, -1.0, 0.0], [sin, 0.0, cos], [cos, 0.0, -sin]];
        let walking = Projection::from_eye(scene, eye, basis, 500.0, size.width, size.height);
        assert!(outer.iter().any(|xyz| walking.depth(*xyz) < 0.0));
        let ring = screen_ring(walking, &outer, size);
        assert!(ring.len() >= 4 && within(&ring, size), "{ring:?}");
        let (x, y) = on_screen(walking, in_wall);
        assert!((0.0..size.width).contains(&x) && (0.0..size.height).contains(&y));
        assert!(filled(&ring, x, y));
        let (x, y) = on_screen(walking, in_room);
        assert!((0.0..size.width).contains(&x) && (0.0..size.height).contains(&y));
        assert!(!filled(&ring, x, y));
        // A ring that lies behind the eye altogether gives nothing to fill.
        let behind: Vec<[f64; 3]> = outer.iter().map(|[x, y, z]| [x - 10.0, *y, *z]).collect();
        assert!(screen_ring(walking, &behind, size).is_empty());
        assert!(screen_ring(walking, &[], size).is_empty());

        // Cleared by hand.
        assert_eq!(
            send(&mut studio, ApiCommand::ClearDrawingPreview),
            json!({"ok": true, "cleared": true})
        );
        assert_eq!(status(&mut studio)["preview_shown"], false);

        // Cleared by everything the cut was made from.
        type Change = Box<dyn Fn(&mut Studio)>;
        let changes: Vec<(&str, Change)> = vec![
            (
                "the section box moved",
                Box::new(|studio| {
                    let _ = studio.update(Message::SectionMax(2, 40.0));
                }),
            ),
            (
                "the section box switched off",
                Box::new(|studio| {
                    let _ = studio.update(Message::SetSectionEnabled(false));
                }),
            ),
            (
                "the layer hidden",
                Box::new(|studio| {
                    let _ = studio.update(Message::SetVisible(0, false));
                }),
            ),
            (
                "a class hidden",
                Box::new(|studio| {
                    let _ = studio.update(Message::FilterClass(6, false));
                }),
            ),
            (
                "the layer moved",
                Box::new(|studio| {
                    studio.translate_x = "0.5".into();
                    let _ = studio.update(Message::ApplyTranslation);
                }),
            ),
            (
                "points deleted",
                Box::new(|studio| {
                    let total = studio.clouds[0].cloud.total_points;
                    studio.clouds[0].deleted = Some(Arc::new(DeletionMask::new(total).unwrap()));
                    let _ = studio.update(Message::Modifiers(iced::keyboard::Modifiers::default()));
                }),
            ),
            (
                "another view chosen",
                Box::new(|studio| {
                    let _ =
                        studio.update(Message::Drawing(DrawingAction::View(DrawingView::Front)));
                }),
            ),
            (
                "another slab thickness typed",
                Box::new(|studio| {
                    let _ =
                        studio.update(Message::Drawing(DrawingAction::Thickness("0.05".into())));
                }),
            ),
        ];
        for (what, change) in changes {
            let directory = tempfile::tempdir().unwrap();
            let (mut studio, _) = studio_with_room(directory.path());
            set_plan_box(&mut studio);
            assert_eq!(preview(&mut studio)["preview_shown"], true, "{what}");
            change(&mut studio);
            assert!(studio.drawing.overlay().is_none(), "{what}");
            assert_eq!(status(&mut studio)["preview_shown"], false, "{what}");
        }

        // What changes the file but not the cut leaves the preview.
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        assert_eq!(preview(&mut studio)["preview_shown"], true);
        // A point limit that is being typed is one of those, also while it
        // holds no number or one the core would refuse.
        for action in [
            DrawingAction::Units(DrawingUnits::Metres),
            DrawingAction::Origin(DrawingOrigin::BoxCorner),
            DrawingAction::Color(PointColor::Rgb),
            DrawingAction::Fill(false),
            DrawingAction::MaxPoints(String::new()),
            DrawingAction::MaxPoints("many".into()),
            DrawingAction::MaxPoints("0".into()),
            DrawingAction::MaxPoints("9999999".into()),
            DrawingAction::MaxPoints("9000".into()),
        ] {
            let what = format!("{action:?}");
            let _ = studio.update(Message::Drawing(action));
            assert!(studio.drawing.overlay().is_some(), "{what}");
        }

        // A preview that arrives for a scene that has changed is not shown,
        // and the job that makes it is told to stop.
        let accepted = send(
            &mut studio,
            ApiCommand::PreviewDrawing {
                options: DrawingOptions::default(),
            },
        );
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        // A preview reads and traces, and it is not stopped by a point limit
        // that is typed while it is made.
        let detail = studio.progress_lines().remove(0).detail;
        assert!(detail.starts_with("Step 1 of 2"), "{detail}");
        let _ = studio.update(Message::Drawing(DrawingAction::MaxPoints(String::new())));
        assert!(!studio.drawing.job.as_ref().unwrap().cancelling());
        let _ = studio.update(Message::SectionMax(0, 60.0));
        let running = studio.drawing.job.as_ref().unwrap();
        assert!(running.cancelling());
        let (serial, input) = (running.serial, Arc::clone(&running.input));
        let late = DrawingEnd::of(run(&input, &mut |_| Ok(())));
        assert!(matches!(late, DrawingEnd::Preview(..)));
        let _ = studio.update(Message::Drawing(DrawingAction::Finished(serial, late)));
        assert!(studio.drawing.overlay().is_none());
        assert_eq!(job(&mut studio, &id)["state"], "failed");
    }

    #[test]
    fn cancelled_export_leaves_the_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        let destination = directory.path().join("kept.dwg");
        std::fs::write(&destination, "earlier drawing").unwrap();
        let accepted = send(
            &mut studio,
            ApiCommand::ExportDrawing {
                path: destination.clone(),
                options: DrawingOptions::default(),
            },
        );
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        assert_eq!(
            send(&mut studio, ApiCommand::CancelDrawing),
            json!({"ok": true, "cancel_requested": true})
        );
        assert_eq!(studio.status, "Cancelling the section drawing…");
        let line = &studio.progress_lines()[0];
        assert_eq!(line.title, "Cancelling…");
        assert!(line.cancel.is_none());
        assert_eq!(status(&mut studio)["job"]["cancel_requested"], true);

        finish(&mut studio);
        let cancelled = job(&mut studio, &id);
        assert_eq!(
            cancelled,
            json!({"state": "cancelled", "operation": "export_drawing"})
        );
        assert_eq!(
            studio.status,
            "Section drawing cancelled; an existing file is left as it was"
        );
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "earlier drawing"
        );
        // The room, the earlier drawing and nothing else.
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
        assert!(!studio.drawing.is_running());
        assert!(matches!(studio.drawing.last, Some(Last::Cancelled { .. })));
        let _ = studio.view();

        // Leaving the application asks a running job to stop as well.
        let _ = send(
            &mut studio,
            ApiCommand::PreviewDrawing {
                options: DrawingOptions::default(),
            },
        );
        studio.stop_background_work();
        assert!(studio.drawing.job.as_ref().unwrap().cancelling());

        // An answer of a job that is no longer the running one is ignored.
        let serial = studio.drawing.job.as_ref().unwrap().serial;
        let _ = studio.update(Message::Drawing(DrawingAction::Finished(
            serial + 7,
            DrawingEnd::Cancelled,
        )));
        assert!(studio.drawing.is_running());
    }

    #[test]
    fn ribbon_button_and_file_view_open_the_tool_with_the_section_box() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_room(directory.path());
        assert!(studio.drawing_properties().is_none());
        // Without the section box neither the button nor the entry is
        // available.
        assert!(!studio.drawing_button_enabled());
        assert!(!studio.drawing_entry_enabled());

        // The File view entry closes the view; without a box it says why
        // nothing is drawn.
        let _ = studio.update(Message::ToggleFile);
        let _ = studio.update(Message::FileAction(FileAction::ExportDrawing));
        assert!(!studio.file_open);
        assert!(!studio.drawing.busy());
        assert_eq!(
            studio.status,
            "Switch on the section box before making a drawing"
        );

        set_plan_box(&mut studio);
        assert!(studio.drawing_button_enabled());
        assert!(studio.drawing_entry_enabled());
        let _ = studio.update(Message::Drawing(DrawingAction::Toggle));
        assert!(studio.drawing.open);
        assert!(studio.drawing_properties().is_some());
        let _ = studio.view();

        // A number that cannot be read stops the export before the dialog.
        let _ = studio.update(Message::Drawing(DrawingAction::Thickness("x".into())));
        let _ = studio.update(Message::Drawing(DrawingAction::Export));
        assert!(!studio.drawing.busy());
        assert_eq!(studio.status, "Slab thickness must be a number of metres");
        let _ = studio.update(Message::Drawing(DrawingAction::Thickness("0.1".into())));

        let _ = studio.update(Message::Drawing(DrawingAction::Export));
        assert!(studio.drawing.dialog_pending);
        assert_eq!(
            studio.status,
            "Choose where to save the drawing as DXF or DWG…"
        );
        // While the dialog is open nothing else starts, and the File view
        // entry waits for it.
        assert!(!studio.drawing_entry_enabled());
        let _ = studio.update(Message::Drawing(DrawingAction::Preview));
        assert_eq!(studio.status, BUSY);
        assert!(!studio.drawing.is_running());

        let _ = studio.update(Message::Drawing(DrawingAction::PathChosen(None)));
        assert!(!studio.drawing.busy());
        assert!(studio.drawing_entry_enabled());
        assert_eq!(studio.status, "Section drawing cancelled");

        for (name, problem) in [("plan.pdf", NO_FORMAT), ("plan", NO_FORMAT)] {
            studio.drawing.dialog_pending = true;
            let _ = studio.update(Message::Drawing(DrawingAction::PathChosen(Some(
                directory.path().join(name),
            ))));
            assert!(!studio.drawing.busy(), "{name}");
            assert_eq!(studio.status, problem, "{name}");
        }

        studio.drawing.dialog_pending = true;
        let destination = directory.path().join("room-plan.DXF");
        let _ = studio.update(Message::Drawing(DrawingAction::PathChosen(Some(
            destination.clone(),
        ))));
        assert!(studio.drawing.is_running());
        assert!(!studio.drawing_entry_enabled());
        assert!(studio.drawing_button_enabled());
        assert_eq!(studio.status, "Section drawing: reading the slab…");
        // While the job runs, the block shows its stage and a cancel button.
        let _ = studio.view();
        finish(&mut studio);
        assert!(destination.is_file());
        assert!(matches!(studio.drawing.last, Some(Last::Exported { .. })));
        let _ = studio.view();

        // With the 3D BAG panel in the place of Properties, the button
        // brings the block back; it does not close what was out of sight.
        studio.bag_panel = true;
        let _ = studio.update(Message::Drawing(DrawingAction::Toggle));
        assert!(studio.drawing.open && !studio.bag_panel);

        // The button closes the block again, also with the box off; after
        // that it needs the box once more.
        let _ = studio.update(Message::SetSectionEnabled(false));
        assert!(studio.drawing_button_enabled());
        assert!(!studio.drawing_entry_enabled());
        let _ = studio.view();
        let _ = studio.update(Message::Drawing(DrawingAction::Toggle));
        assert!(!studio.drawing.open);
        assert!(studio.drawing_properties().is_none());
        assert!(!studio.drawing_button_enabled());
    }

    /// The room turned `degrees` about its own centre (2, 1.5) as a scan.
    fn studio_with_turned_room(directory: &Path, degrees: f64) -> Studio {
        let (points, _) = room_points();
        let (sin, cos) = degrees.to_radians().sin_cos();
        let text: String = points
            .iter()
            .map(|[x, y, z]| {
                let (dx, dy) = (x - 2.0, y - 1.5);
                format!(
                    "{:.4} {:.4} {z:.3}\n",
                    2.0 + cos * dx - sin * dy,
                    1.5 + sin * dx + cos * dy
                )
            })
            .collect();
        let path = directory.join("turned.xyz");
        std::fs::write(&path, text).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 1_000).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio
    }

    #[test]
    fn a_box_turned_along_the_walls_draws_a_square_plan_and_a_section_parallel_to_a_wall() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = studio_with_turned_room(directory.path(), 30.0);
        let (room, in_slab) = room_points();

        // Around the whole model, Align to walls finds the walls at 30
        // degrees and turns the box about its centre to them.
        let _ = studio.update(Message::SetSectionEnabled(true));
        let before = studio.section_box().unwrap();
        let scene = studio.drawing_scene().unwrap();
        let asked = scene.section;
        let found = find_walls(&scene).unwrap().unwrap();
        let _ = studio.update(Message::SectionWallsFound(asked, Ok(Some(found))));
        assert!((studio.section_rotation - 30.0).abs() < 0.1, "{found:?}");
        assert!(
            studio.status.starts_with("Section box turned to "),
            "{}",
            studio.status
        );
        let turned = studio.section_box().unwrap();
        for axis in 0..3 {
            assert!((turned.center()[axis] - before.center()[axis]).abs() < 1e-6);
            assert!((turned.size()[axis] - before.size()[axis]).abs() < 1e-6);
        }
        let state = send(&mut studio, ApiCommand::Status)["result"]["section"].clone();
        assert!((state["rotation"].as_f64().unwrap() - studio.section_rotation).abs() < 1e-12);
        // An answer for a box that has changed since is not used.
        let _ = studio.update(Message::ResetSectionBox);
        let rotation = studio.section_rotation;
        let _ = studio.update(Message::SectionWallsFound(asked, Ok(Some(found))));
        assert_eq!(studio.section_rotation, rotation);
        assert_eq!(
            studio.status,
            "The section box changed while the walls were looked for"
        );

        // A box of 4.6 by 3.4 m turned 30 degrees about the centre of the
        // room holds the whole room, which a plan draws along its axes.
        // Centred on the room, so that the frame of the box is that of the
        // room before it was turned.
        let turned_box = |half: f64, z: [f64; 2]| ApiCommand::SetSection {
            min: [2.0 - 2.3, 1.5 - half, z[0]],
            max: [2.0 + 2.3, 1.5 + half, z[1]],
            rotation: Some(30.0),
        };
        let answer = send(&mut studio, turned_box(1.7, [0.9, 1.1]));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["section"]["rotation"], 30.0);
        assert!((answer["section"]["min"][0].as_f64().unwrap() + 0.3).abs() < 1e-9);
        let draw = |studio: &mut Studio, name: &str, view: &str, thickness: f64| {
            let accepted = send(
                studio,
                ApiCommand::ExportDrawing {
                    path: directory.path().join(name),
                    options: DrawingOptions {
                        view: Some(view.into()),
                        thickness: Some(thickness),
                        fill: Some(true),
                        ..DrawingOptions::default()
                    },
                },
            );
            assert_eq!(accepted["ok"], true, "{accepted}");
            let id = accepted["job_id"].as_str().unwrap().to_owned();
            finish(studio);
            job(studio, &id)
        };
        let plan = draw(&mut studio, "plan.dxf", "plan", 0.1);
        assert_eq!(plan["state"], "complete", "{plan}");
        assert_eq!(plan["slab_points"], in_slab);
        let direction = plan["direction_degrees"].as_f64().unwrap();
        assert!(direction.abs() < 0.05, "{direction}");

        // The front face of the box 0.155 m in front of the inner face of the
        // wall at y = 0 of the room: a slab of 0.1 m holds the outer face of
        // that wall and the ends of the walls beside it.
        let answer = send(&mut studio, turned_box(1.655, [0.9, 1.1]));
        assert_eq!(answer["ok"], true, "{answer}");
        let front = draw(&mut studio, "front.dxf", "front", 0.1);
        assert_eq!(front["state"], "complete", "{front}");
        let truth = room
            .iter()
            .filter(|[x, y, z]| {
                (-0.155..=-0.055).contains(y) && (-0.3..=4.3).contains(x) && (0.9..=1.1).contains(z)
            })
            .count();
        assert!(truth > 1_000, "{truth}");
        assert_eq!(front["slab_points"], truth as u64);

        // With the face farther out the slab is empty, and the reason says
        // which face of the box the cut plane is.
        let _ = send(&mut studio, turned_box(2.5, [0.9, 1.1]));
        let empty = draw(&mut studio, "empty.dxf", "front", 0.1);
        assert_eq!(empty["state"], "failed");
        let error = empty["error"].as_str().unwrap();
        assert!(
            error.starts_with("the slab holds no points: the cut plane is the front face"),
            "{error}"
        );
        assert!(!directory.path().join("empty.dxf").exists());
    }

    /// Make a plan of the model with Create 2D at a height from every
    /// point, as the local API does, and answer its identifier.
    fn make_plan(studio: &mut Studio, height: f64) -> String {
        let answer = send(
            studio,
            serde_json::from_str(&format!(
                r#"{{"command":"create_drawing","kind":"plan","height":{height},"sample_percent":100}}"#
            ))
            .unwrap(),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        finish(studio);
        let made = job(studio, answer["job_id"].as_str().unwrap());
        assert_eq!(made["state"], "complete", "{made}");
        made["guid"].as_str().unwrap().to_owned()
    }

    fn definition(studio: &Studio, guid: &str) -> SavedDrawing {
        studio
            .drawing_view
            .saved
            .iter()
            .find(|drawing| drawing.guid == guid)
            .unwrap()
            .clone()
    }

    /// How far the long lines of the outline of the filled cut of the
    /// drawing shown are off the axes of the sheet, in degrees, at most.
    fn outline_off_axis(studio: &Studio, longer_than: f64) -> f64 {
        let scene = studio.drawing_view.scene().unwrap();
        let layer = scene
            .layers
            .iter()
            .find(|layer| layer.name == pointcloud_core::LAYER_CUT_OUTLINE)
            .expect("an outline");
        let mut worst: f64 = 0.0;
        let mut long = 0;
        for line in &layer.lines {
            let mut points = line.points.clone();
            if line.closed {
                points.push(points[0]);
            }
            for pair in points.windows(2) {
                let (dx, dy) = (pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]);
                if dx.hypot(dy) < longer_than {
                    continue;
                }
                long += 1;
                let angle = dy.atan2(dx).to_degrees().rem_euclid(90.0);
                worst = worst.max(angle.min(90.0 - angle));
            }
        }
        assert!(long >= 4, "{long} long lines");
        worst
    }

    #[test]
    fn the_crop_region_of_a_plan_is_dragged_and_set_and_made_again_in_place() {
        use crate::drawing_crop::{CropAction, Field};

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        let plan = make_plan(&mut studio, 1.05);
        let before = definition(&studio, &plan);
        let unit = before.request().unwrap().units.factor();

        // Reject mixed coordinates before starting a remake. Previously
        // this changed the crop but dropped its task, leaving it busy forever.
        let invalid = send(
            &mut studio,
            serde_json::from_value(json!({
                "command": "set_sheet_crop", "rect": [[0, 0], [2000, 2000]], "cut": 1.2
            }))
            .unwrap(),
        );
        assert_eq!(invalid["ok"], false);
        assert!(studio.drawing.job.is_none());
        assert!(studio.drawing_view.remake.is_none());
        assert_eq!(definition(&studio, &plan), before);

        // The crop region is the box as the plan shows it, with handles.
        let crop = studio.crop_overlay().expect("a crop region");
        assert!(crop.editable && crop.turn.is_none());
        let model = crate::combined_bounds(&studio.clouds).unwrap();
        assert_eq!(crop.rect[0], [model.min[0] * unit, model.min[1] * unit]);
        assert_eq!(crop.rect[1], [model.max[0] * unit, model.max[1] * unit]);
        assert!(studio.drawing_view_properties().is_some());
        // The switch hides it and shows it again.
        let _ = studio.update(Message::Crop(CropAction::Show(false)));
        assert!(studio.crop_overlay().is_none());
        let _ = studio.update(Message::Crop(CropAction::Show(true)));
        assert!(studio.crop_overlay().is_some());

        // A view of its own: panned and zoomed.
        let size = Size::new(800.0, 600.0);
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Zoom(
            2.0,
            [300.0, 200.0],
            size,
        )));
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Pan([40.0, -25.0])));
        let camera = studio.drawing_view.camera();

        // Handles let go around the room: the box follows in its own plane,
        // the cut and the floor stay, and the plan is made again in place.
        let room = [[-0.5 * unit, -0.5 * unit], [4.5 * unit, 3.5 * unit]];
        let _ = studio.update(Message::Crop(CropAction::Set(plan.clone(), room)));
        assert!(studio.drawing.job.is_some(), "{}", studio.status);
        let pending = studio.crop_overlay().unwrap();
        assert!(!pending.editable, "no handles while it is made again");
        assert_eq!(pending.corners[0], room[0]);
        assert_eq!(pending.corners[2], room[1]);
        // A second change waits for the first.
        let _ = studio.update(Message::Crop(CropAction::Set(plan.clone(), crop.rect)));
        assert!(
            studio.status.contains("being made again"),
            "{}",
            studio.status
        );
        finish(&mut studio);
        let after = definition(&studio, &plan);
        assert_eq!(after.name, before.name);
        assert!(
            (after.section.min[0] + 0.5).abs() < 1e-9 && (after.section.max[0] - 4.5).abs() < 1e-9
        );
        assert!(
            (after.section.min[1] + 0.5).abs() < 1e-9 && (after.section.max[1] - 3.5).abs() < 1e-9
        );
        assert_eq!(after.section.min[2], before.section.min[2]);
        assert_eq!(after.section.max[2], before.section.max[2]);
        assert_eq!(studio.drawing_view.shown_guid(), Some(plan.as_str()));
        assert_eq!(
            studio.drawing_view.camera(),
            camera,
            "no jump to the extents"
        );
        assert!(
            studio
                .status
                .starts_with("Crop region of Plan +1.05 set to 5.00 × 4.00 m"),
            "{}",
            studio.status
        );
        assert_eq!(crate::saved_drawings::load()[0], after, "kept on disk");
        // The frame of the drawing made again is the crop region.
        let frame = studio
            .drawing_view
            .scene()
            .unwrap()
            .layers
            .iter()
            .find(|layer| layer.name == pointcloud_core::LAYER_FRAME)
            .unwrap()
            .lines[0]
            .points
            .clone();
        assert!(
            frame.contains(&room[0]) && frame.contains(&room[1]),
            "{frame:?}"
        );

        // Properties: a width typed and Enter, about the centre.
        let _ = studio.update(Message::Crop(CropAction::Field(Field::Width, "4,2".into())));
        let _ = studio.update(Message::Crop(CropAction::Apply(Field::Width)));
        finish(&mut studio);
        let narrower = definition(&studio, &plan);
        assert!((narrower.section.max[0] - narrower.section.min[0] - 4.2).abs() < 1e-9);
        assert!(((narrower.section.min[0] + narrower.section.max[0]) / 2.0 - 2.0).abs() < 1e-9);
        assert!(studio.drawing_view.crop_edits.1.is_empty());
        // Too small is refused and changes nothing.
        let _ = studio.update(Message::Crop(CropAction::Field(
            Field::Height,
            "0.05".into(),
        )));
        let _ = studio.update(Message::Crop(CropAction::Apply(Field::Height)));
        assert!(studio.drawing.job.is_none());
        assert_eq!(definition(&studio, &plan), narrower);

        // The local API sets figures and the rectangle, as a job.
        let answer = send(
            &mut studio,
            serde_json::from_str(
                r#"{"command":"set_sheet_crop","name":"plan +1.05","center":[2.0,1.5],"height":3.4,"depth":0.2}"#,
            )
            .unwrap(),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        assert!((answer["crop"]["height"].as_f64().unwrap() - 3.4).abs() < 1e-9);
        finish(&mut studio);
        let done = job(&mut studio, answer["job_id"].as_str().unwrap());
        assert_eq!(done["state"], "complete", "{done}");
        assert_eq!(done["operation"], "set_sheet_crop");
        assert_eq!(definition(&studio, &plan).thickness, Some(0.2));
        let listed = send(&mut studio, ApiCommand::ListDrawings);
        let listed_crop = &listed["drawings"][0]["crop"];
        assert!(
            (listed_crop["width"].as_f64().unwrap() - 4.2).abs() < 1e-9,
            "{listed}"
        );
        assert_eq!(listed_crop["center"][1], 1.5);
        let refused = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"set_sheet_crop","width":0.01}"#).unwrap(),
        );
        assert_eq!(refused["ok"], false);
        let nothing = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"set_sheet_crop"}"#).unwrap(),
        );
        assert_eq!(nothing["ok"], false);

        // The local API drags a handle as the pointer does: held first, with
        // the size in whole centimetres, then let go.
        let drag = |release: bool| {
            serde_json::from_str::<ApiCommand>(&format!(
                r#"{{"command":"drag_crop_handle","handle":"top","to":[0,{}],"release":{release}}}"#,
                3.004 * unit
            ))
            .unwrap()
        };
        let held = send(&mut studio, drag(false));
        assert_eq!(held["held"], true, "{held}");
        assert!(
            (held["height"].as_f64().unwrap() - 3.2).abs() < 1e-9,
            "{held}"
        );
        assert!(studio.drawing_view.held.is_some() && studio.drawing.job.is_none());
        let _ = studio.view();
        let released = send(&mut studio, drag(true));
        assert_eq!(released["accepted"], true, "{released}");
        assert!(studio.drawing_view.held.is_none());
        finish(&mut studio);
        assert!((definition(&studio, &plan).section.max[1] - 3.0).abs() < 1e-9);
        let wrong = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"drag_crop_handle","handle":"middle","to":[0,0]}"#)
                .unwrap(),
        );
        assert_eq!(wrong["ok"], false);
        let _ = studio.view();
    }

    #[test]
    fn a_duplicate_of_a_drawing_is_listed_below_it_without_making_it_again() {
        use crate::drawing_crop::CropAction;
        use crate::project_browser::{BrowserAction, ViewKind, ViewRow};

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        let plan = make_plan(&mut studio, 1.05);
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Pan([30.0, 10.0])));
        let camera = studio.drawing_view.camera();

        let _ = studio.update(Message::Browser(BrowserAction::Duplicate(
            ViewRow::Drawing(plan.clone()),
        )));
        assert!(studio.drawing.job.is_none(), "nothing is computed");
        let saved = studio.drawing_view.saved.clone();
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[1].name, "Plan +1.05 (2)");
        assert_ne!(saved[1].guid, plan);
        assert_eq!(saved[1].section, saved[0].section);
        let copy = saved[1].guid.clone();
        assert_eq!(studio.drawing_view.shown_guid(), Some(copy.as_str()));
        assert_eq!(studio.drawing_view.camera(), camera);
        assert_eq!(
            studio.drawing_view.made(&copy).unwrap().totals(),
            studio.drawing_view.made(&plan).unwrap().totals()
        );
        assert_eq!(studio.drawing_view_caption(), "Plan +1.05 (2)");
        assert_eq!(crate::saved_drawings::load(), saved, "kept on disk");

        // The copy changes on its own.
        let unit = saved[1].request().unwrap().units.factor();
        let crop = studio.crop_overlay().unwrap();
        let mut smaller = crop.rect;
        smaller[1][0] = 2.0 * unit;
        let _ = studio.update(Message::Crop(CropAction::Set(copy.clone(), smaller)));
        finish(&mut studio);
        assert!((definition(&studio, &copy).section.max[0] - 2.0).abs() < 1e-9);
        assert_eq!(definition(&studio, &plan).section, saved[0].section);

        // Another copy of the original comes right below it.
        let answer = send(
            &mut studio,
            serde_json::from_str(
                r#"{"command":"duplicate_view","name":"Plan +1.05","kind":"drawing"}"#,
            )
            .unwrap(),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["name"], "Plan +1.05 (3)");
        assert!(answer["job_id"].is_null(), "{answer}");
        let groups = studio.view_groups();
        let (kind, rows) = &groups[1];
        assert_eq!(*kind, ViewKind::Plans);
        let names: Vec<String> = rows
            .iter()
            .map(|row| match row {
                ViewRow::Drawing(guid) => definition(&studio, guid).name,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, ["Plan +1.05", "Plan +1.05 (3)", "Plan +1.05 (2)"]);

        // After a restart a duplicate of a drawing not made yet is made.
        let mut restarted = Studio::default();
        let cloud = Arc::new(pointcloud_core::open(&studio.clouds[0].cloud.path, 1_000).unwrap());
        let _ = restarted.update(Message::Loaded(Ok(cloud)));
        let duplicate = || {
            serde_json::from_str::<ApiCommand>(
                r#"{"command":"duplicate_view","name":"plan +1.05 (2)"}"#,
            )
            .unwrap()
        };
        // While another job runs the copy cannot be made, and none is kept.
        restarted.drawing.dialog_pending = true;
        let refused = send(&mut restarted, duplicate());
        assert_eq!(refused["ok"], false, "{refused}");
        assert_eq!(refused["error"], BUSY);
        let _ = restarted.update(Message::Browser(BrowserAction::Duplicate(
            ViewRow::Drawing(saved[1].guid.clone()),
        )));
        assert_eq!(restarted.status, BUSY);
        assert_eq!(restarted.drawing_view.saved.len(), 3);
        assert_eq!(
            crate::saved_drawings::load().len(),
            3,
            "nothing kept on disk"
        );
        restarted.drawing.dialog_pending = false;
        let answer = send(&mut restarted, duplicate());
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["name"], "Plan +1.05 (4)");
        assert_eq!(answer["accepted"], true);
        finish(&mut restarted);
        let made = job(&mut restarted, answer["job_id"].as_str().unwrap());
        assert_eq!(made["state"], "complete", "{made}");
        let names: Vec<&str> = restarted
            .drawing_view
            .saved
            .iter()
            .map(|drawing| drawing.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Plan +1.05",
                "Plan +1.05 (3)",
                "Plan +1.05 (2)",
                "Plan +1.05 (4)"
            ]
        );
        let _ = studio.view();
    }

    #[test]
    fn ro_turns_the_crop_region_of_a_plan_and_the_walls_come_along_the_sheet() {
        use crate::drawing_crop::{CropAction, TurnTarget};

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let mut studio = studio_with_turned_room(directory.path(), 30.0);
        let plan = make_plan(&mut studio, 1.05);
        let before = definition(&studio, &plan);
        let unit = before.request().unwrap().units.factor();
        // The walls of the room stand 30 degrees off the sheet.
        let off = outline_off_axis(&studio, 1.0 * unit);
        assert!((off - 30.0).abs() < 1.0, "{off}");
        let size = Size::new(800.0, 600.0);
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Zoom(
            1.5,
            [500.0, 260.0],
            size,
        )));
        let camera = studio.drawing_view.camera();
        let centre = crate::drawing_crop::middle(studio.crop_overlay().unwrap().rect);
        let seen = camera.to_screen(centre, size);

        // R and then O start the turn; the pointer turns the region, and a
        // typed angle wins.
        let typed = |studio: &mut Studio, key: &str| {
            let _ = studio.update(Message::KeyTyped(key.into(), false));
        };
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        let Some(TurnTarget::Plan { guid, .. }) = studio.turn.as_ref().map(|turn| &turn.target)
        else {
            panic!("no turn: {}", studio.status);
        };
        assert_eq!(*guid, plan);
        assert!(
            studio.status.starts_with("Turning the crop region: 0°"),
            "{}",
            studio.status
        );
        let reach = 2.0 * unit;
        let _ = studio.update(Message::Crop(CropAction::TurnPointer([
            centre[0] + reach,
            centre[1],
        ])));
        let pointed = [
            centre[0] + reach * 40.2f64.to_radians().cos(),
            centre[1] + reach * 40.2f64.to_radians().sin(),
        ];
        let _ = studio.update(Message::Crop(CropAction::TurnPointer(pointed)));
        assert_eq!(studio.crop_overlay().unwrap().turn.as_deref(), Some("40°"));
        let _ = studio.update(Message::Modifiers(iced::keyboard::Modifiers::SHIFT));
        assert_eq!(studio.crop_overlay().unwrap().turn.as_deref(), Some("45°"));
        let _ = studio.update(Message::Modifiers(iced::keyboard::Modifiers::default()));
        // F neither fits nor ends the turn; digits type the angle.
        typed(&mut studio, "f");
        typed(&mut studio, "3");
        typed(&mut studio, "1");
        let _ = studio.update(Message::Measure(measure::MeasureAction::RemoveLast));
        typed(&mut studio, "0");
        let shown = studio.crop_overlay().unwrap();
        assert_eq!(shown.turn.as_deref(), Some("30°"));
        assert!(!shown.editable);
        assert_eq!(studio.drawing_view.camera(), camera);
        // Enter applies: the box turns 30 degrees counter-clockwise about
        // the centre of the region and the plan is made again upright.
        let _ = studio.update(Message::Measure(measure::MeasureAction::Finish));
        assert!(studio.turn.is_none());
        assert!(studio.drawing.job.is_some(), "{}", studio.status);
        finish(&mut studio);
        let after = definition(&studio, &plan);
        assert_eq!(after.section.rotation, 30.0);
        let [was, now] = [&before, &after].map(|drawing| drawing.oriented().center());
        assert!((0..3).all(|axis| (was[axis] - now[axis]).abs() < 1e-9));
        let off = outline_off_axis(&studio, 1.0 * unit);
        assert!(off < 0.5, "the walls run along the sheet: {off}");
        // The centre of the region stays where it was on the screen, at the
        // same zoom.
        let after_crop = studio.crop_overlay().unwrap();
        let camera_after = studio.drawing_view.camera();
        assert_eq!(camera_after.scale, camera.scale);
        let seen_after = camera_after.to_screen(crate::drawing_crop::middle(after_crop.rect), size);
        assert!((seen_after[0] - seen[0]).abs() < 1e-6 && (seen_after[1] - seen[1]).abs() < 1e-6);
        assert!(studio.status.contains("stands at 30°"), "{}", studio.status);

        // A right click, or Escape, cancels.
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        typed(&mut studio, "9");
        let _ = studio.update(Message::Escape);
        assert!(studio.turn.is_none());
        assert_eq!(studio.status, "Turn cancelled");
        typed(&mut studio, "R");
        typed(&mut studio, "O");
        let _ = studio.update(Message::Crop(CropAction::TurnCancel));
        assert!(studio.turn.is_none() && studio.drawing.job.is_none());
        // Showing something else ends a turn.
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Show(false)));
        assert!(studio.turn.is_none());

        // The local API: a turn shown at an angle, then applied, back.
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Show(true)));
        let pending = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"rotate_crop","degrees":-30,"apply":false}"#)
                .unwrap(),
        );
        assert_eq!(pending["turning"]["degrees"], -30.0, "{pending}");
        assert_eq!(studio.crop_overlay().unwrap().turn.as_deref(), Some("-30°"));
        let applied = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"rotate_crop"}"#).unwrap(),
        );
        assert_eq!(applied["accepted"], true, "{applied}");
        assert!(studio.turn.is_none());
        finish(&mut studio);
        let done = job(&mut studio, applied["job_id"].as_str().unwrap());
        assert_eq!(done["operation"], "rotate_crop", "{done}");
        assert_eq!(definition(&studio, &plan).section.rotation, 0.0);
        let by_name = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"rotate_crop","name":"PLAN +1.05","degrees":30}"#)
                .unwrap(),
        );
        assert_eq!(by_name["accepted"], true, "{by_name}");
        finish(&mut studio);
        let done = job(&mut studio, by_name["job_id"].as_str().unwrap());
        assert_eq!(done["operation"], "rotate_crop", "{done}");
        assert_eq!(definition(&studio, &plan).section.rotation, 30.0);
        let _ = studio.view();
    }

    #[test]
    fn ro_turns_the_section_box_in_3d_and_only_a_plan_in_the_drawing_view() {
        use crate::drawing_crop::{CropAction, TurnTarget};

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        let typed = |studio: &mut Studio, key: &str, captured: bool| {
            let _ = studio.update(Message::KeyTyped(key.into(), captured));
        };

        // Without a section box there is nothing to turn in 3D.
        typed(&mut studio, "r", false);
        typed(&mut studio, "o", false);
        assert!(studio.turn.is_none());
        assert!(
            studio.status.contains("switch the section box on"),
            "{}",
            studio.status
        );

        set_plan_box(&mut studio);
        let original = studio.section_box().unwrap();
        // Another key between R and O, or a key a text field takes, starts
        // over.
        for (first, second) in [(false, true), (true, false)] {
            typed(&mut studio, "r", first);
            typed(&mut studio, "o", second);
            assert!(studio.turn.is_none());
        }
        typed(&mut studio, "r", false);
        typed(&mut studio, "w", false);
        typed(&mut studio, "o", false);
        assert!(studio.turn.is_none());
        // W walks; the orbit view comes back for the pointer to turn the box.
        assert!(studio.walk.is_some());
        let _ = studio.update(Message::LeaveWalk);

        // R and then O turn the section box with the pointer, live.
        typed(&mut studio, "r", false);
        typed(&mut studio, "o", false);
        assert!(matches!(
            studio.turn.as_ref().map(|turn| &turn.target),
            Some(TurnTarget::SectionBox(_))
        ));
        assert!(
            studio.status.starts_with("Turning the section box"),
            "{}",
            studio.status
        );
        let size = Size::new(800.0, 600.0);
        let _ = studio.update(Message::Crop(CropAction::TurnPointer3d(
            [600.0, 300.0],
            size,
        )));
        let _ = studio.update(Message::Crop(CropAction::TurnPointer3d(
            [400.0, 100.0],
            size,
        )));
        let turned = studio.section_box().unwrap();
        assert_ne!(turned.rotation_degrees, 0.0, "{}", studio.status);
        assert_eq!(turned.rotation_degrees, turned.rotation_degrees.round());
        // Escape puts it back.
        let _ = studio.update(Message::Escape);
        assert!(studio.turn.is_none());
        assert_eq!(studio.section_box(), Some(original));
        // A typed angle, applied with Enter.
        typed(&mut studio, "r", false);
        typed(&mut studio, "o", false);
        typed(&mut studio, "1", false);
        typed(&mut studio, "5", false);
        assert_eq!(studio.section_box().unwrap().rotation_degrees, 15.0);
        let _ = studio.update(Message::Measure(measure::MeasureAction::Finish));
        assert!(studio.turn.is_none());
        let section = studio.section_box().unwrap();
        assert_eq!(section.rotation_degrees, 15.0);
        assert!((0..3).all(|axis| (section.center()[axis] - original.center()[axis]).abs() < 1e-9));
        assert_eq!(studio.status, "Section box turned to 15° about its centre");
        // The local API turns it by an angle too.
        let answer = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"rotate_crop","degrees":-20}"#).unwrap(),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["section"]["rotation"], -5.0);

        // In the Drawing view only the crop region of a plan turns.
        let _ = studio.update(Message::SetSectionEnabled(false));
        let answer = send(
            &mut studio,
            serde_json::from_str(
                r#"{"command":"create_drawing","kind":"elevation","side":"front"}"#,
            )
            .unwrap(),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        finish(&mut studio);
        assert!(studio.drawing_view.shown);
        typed(&mut studio, "r", false);
        typed(&mut studio, "o", false);
        assert!(studio.turn.is_none());
        assert!(
            studio
                .status
                .contains("an elevation or a section keeps its direction"),
            "{}",
            studio.status
        );
        let refused = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"rotate_crop","degrees":10}"#).unwrap(),
        );
        assert_eq!(refused["ok"], false);
        // A plan turns.
        let plan = make_plan(&mut studio, 1.05);
        typed(&mut studio, "r", false);
        typed(&mut studio, "o", false);
        assert!(matches!(
            studio.turn.as_ref().map(|turn| &turn.target),
            Some(TurnTarget::Plan { guid, .. }) if *guid == plan
        ));
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["turning"]["target"], "crop_region");
        let _ = studio.view();
    }

    /// The corners of the frame of a drawing made in this session.
    fn frame_of(studio: &Studio, guid: &str) -> Vec<[f64; 2]> {
        studio
            .drawing_view
            .made(guid)
            .unwrap()
            .layers
            .iter()
            .find(|layer| layer.name == pointcloud_core::LAYER_FRAME)
            .unwrap()
            .lines[0]
            .points
            .clone()
    }

    #[test]
    fn a_drawing_made_again_in_place_leaves_the_window_on_what_was_chosen_meanwhile() {
        use crate::drawing_crop::CropAction;
        use crate::project_browser::BrowserAction;

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        let plan = make_plan(&mut studio, 1.05);
        let unit = definition(&studio, &plan).request().unwrap().units.factor();
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Pan([30.0, 10.0])));
        let camera = studio.drawing_view.camera();
        let crop = |low: f64, high: f64| [[low * unit, low * unit], [high * unit, 3.5 * unit]];

        // The 3D model chosen while the plan is made again: the window stays
        // on the model, and the plan made again waits in the Drawing view
        // with the place on the sheet it had.
        let room = crop(-0.5, 4.5);
        let _ = studio.update(Message::Crop(CropAction::Set(plan.clone(), room)));
        assert!(studio.drawing.job.is_some(), "{}", studio.status);
        let _ = studio.update(Message::Browser(BrowserAction::ShowModel));
        finish(&mut studio);
        assert!(!studio.drawing_view.shown, "the window stays on the model");
        assert!(
            studio
                .status
                .starts_with("Crop region of Plan +1.05 set to"),
            "{}",
            studio.status
        );
        let made = Arc::clone(studio.drawing_view.made(&plan).unwrap());
        assert!(studio.drawing_view.is_current(&made));
        assert!(frame_of(&studio, &plan).contains(&room[0]));
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            plan.clone(),
        )));
        assert_eq!(studio.drawing_view.shown_guid(), Some(plan.as_str()));
        assert_eq!(studio.drawing_view.camera(), camera, "in place");

        // Another drawing shown meanwhile stays shown; the plan is only
        // listed as made again.
        let other = make_plan(&mut studio, 2.0);
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            plan.clone(),
        )));
        let smaller = crop(0.0, 3.0);
        let _ = studio.update(Message::Crop(CropAction::Set(plan.clone(), smaller)));
        assert!(studio.drawing.job.is_some(), "{}", studio.status);
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            other.clone(),
        )));
        finish(&mut studio);
        assert_eq!(studio.drawing_view.shown_guid(), Some(other.as_str()));
        assert!(frame_of(&studio, &plan).contains(&smaller[0]));
        let remade = Arc::clone(studio.drawing_view.made(&plan).unwrap());
        assert!(!studio.drawing_view.is_current(&remade));

        // The File view opened meanwhile stays open over the plan, which is
        // made again in place behind it.
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            plan.clone(),
        )));
        let _ = studio.update(Message::Crop(CropAction::Set(plan.clone(), room)));
        assert!(studio.drawing.job.is_some(), "{}", studio.status);
        let _ = studio.update(Message::ToggleFile);
        assert!(studio.file_open);
        finish(&mut studio);
        assert!(studio.file_open, "the File view stays open");
        assert_eq!(studio.drawing_view.shown_guid(), Some(plan.as_str()));
        assert!(frame_of(&studio, &plan).contains(&room[0]));
        let _ = studio.view();
    }

    #[test]
    fn a_section_box_set_from_elsewhere_while_ro_turns_it_stays_as_set() {
        use crate::drawing_crop::CropAction;

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        // A view with the box turned 25 degrees, then the box upright again.
        let turned = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"rotate_crop","degrees":25}"#).unwrap(),
        );
        assert_eq!(turned["ok"], true, "{turned}");
        let saved = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"save_camera_view","name":"Turned"}"#).unwrap(),
        );
        assert_eq!(saved["ok"], true, "{saved}");
        let kept = studio.section_box().unwrap();
        set_plan_box(&mut studio);
        let typed = |studio: &mut Studio, key: &str| {
            let _ = studio.update(Message::KeyTyped(key.into(), false));
        };
        let size = Size::new(800.0, 600.0);
        let turn = |studio: &mut Studio| {
            for pixel in [[600.0, 300.0], [400.0, 100.0]] {
                let _ = studio.update(Message::Crop(CropAction::TurnPointer3d(pixel, size)));
            }
        };

        // The view restored while RO turns the box: its box stays, and the
        // pointer and Escape no longer change it.
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        assert!(studio.turn.is_some(), "{}", studio.status);
        turn(&mut studio);
        assert_ne!(studio.section_box().unwrap().rotation_degrees, 0.0);
        let restored = send(
            &mut studio,
            serde_json::from_str(r#"{"command":"restore_camera_view","name":"Turned"}"#).unwrap(),
        );
        assert_eq!(restored["ok"], true, "{restored}");
        assert!(studio.turn.is_none());
        assert!(
            studio.status.ends_with("the turn of the section box ended"),
            "{}",
            studio.status
        );
        let shown = studio.section_box().unwrap();
        assert!((shown.rotation_degrees - kept.rotation_degrees).abs() < 1e-9);
        let [was, now] = [kept, shown].map(|section| section.center());
        assert!((0..3).all(|axis| (was[axis] - now[axis]).abs() < 1e-6));
        turn(&mut studio);
        let _ = studio.update(Message::Escape);
        let _ = studio.update(Message::Crop(CropAction::TurnApply));
        assert_eq!(studio.section_box(), Some(shown));

        // Limits typed elsewhere end a turn as well.
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        turn(&mut studio);
        set_plan_box(&mut studio);
        assert!(studio.turn.is_none());
        let upright = studio.section_box().unwrap();
        assert_eq!(upright.rotation_degrees, 0.0);
        let _ = studio.update(Message::Escape);
        assert_eq!(studio.section_box(), Some(upright));
        // The box switched off still puts it back as it was.
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        turn(&mut studio);
        let _ = studio.update(Message::SetSectionEnabled(false));
        assert!(studio.turn.is_none());
        let _ = studio.update(Message::SetSectionEnabled(true));
        assert_eq!(studio.section_box(), Some(upright));
    }

    #[test]
    fn a_key_without_a_character_between_r_and_o_starts_ro_over() {
        use iced::keyboard::key::Named;

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        let original = studio.section_box().unwrap();
        let typed = |studio: &mut Studio, key: &str| {
            let _ = studio.update(Message::KeyTyped(key.into(), false));
        };
        let named = |studio: &mut Studio, key: Named| {
            let _ = studio.update(Message::NamedKey(key, true));
        };
        for between in [
            Named::Space,
            Named::Escape,
            Named::Enter,
            Named::Tab,
            Named::ArrowUp,
        ] {
            typed(&mut studio, "r");
            named(&mut studio, between);
            typed(&mut studio, "o");
            assert!(studio.turn.is_none(), "{between:?}");
            assert_eq!(studio.section_box(), Some(original));
        }
        // Backspace and Enter type and apply the angle of a turn.
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        assert!(studio.turn.is_some(), "{}", studio.status);
        typed(&mut studio, "1");
        typed(&mut studio, "5");
        named(&mut studio, Named::Backspace);
        named(&mut studio, Named::Enter);
        assert!(studio.turn.is_none());
        assert_eq!(studio.section_box().unwrap().rotation_degrees, 1.0);
        // Taken by a text field, Enter does not apply it; Escape cancels it.
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        typed(&mut studio, "9");
        let _ = studio.update(Message::NamedKey(Named::Enter, false));
        assert!(studio.turn.is_some());
        named(&mut studio, Named::Escape);
        assert!(studio.turn.is_none());
        assert_eq!(studio.section_box().unwrap().rotation_degrees, 1.0);
    }

    #[test]
    fn ro_while_walking_turns_the_section_box_by_a_typed_angle_only() {
        use crate::drawing_crop::CropAction;

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        let original = studio.section_box().unwrap();
        assert!(studio.start_walk());
        let typed = |studio: &mut Studio, key: &str| {
            let _ = studio.update(Message::KeyTyped(key.into(), false));
        };
        typed(&mut studio, "r");
        typed(&mut studio, "o");
        assert!(studio.turn.is_some(), "{}", studio.status);
        assert!(
            studio
                .status
                .contains("Type an angle; while walking the pointer does not turn it"),
            "{}",
            studio.status
        );
        // The pointer looks around while walking; it does not turn the box.
        let size = Size::new(800.0, 600.0);
        for pixel in [[600.0, 300.0], [400.0, 100.0]] {
            let _ = studio.update(Message::Crop(CropAction::TurnPointer3d(pixel, size)));
        }
        assert_eq!(studio.section_box(), Some(original));
        typed(&mut studio, "2");
        typed(&mut studio, "0");
        assert_eq!(studio.section_box().unwrap().rotation_degrees, 20.0);
        assert!(studio.status.contains("20°"), "{}", studio.status);
        // The points read for the turned box leave the angle in the status
        // bar, the one place it shows while walking.
        let revision = studio.revision;
        let _ = studio.update(Message::DetailPreview(revision, Vec::new()));
        let _ = studio.update(Message::DetailReady(revision, Ok(Vec::new())));
        assert!(
            studio.status.starts_with("Turning the section box: 20°"),
            "{}",
            studio.status
        );
        let _ = studio.update(Message::Crop(CropAction::TurnApply));
        assert!(studio.turn.is_none());
        assert_eq!(studio.section_box().unwrap().rotation_degrees, 20.0);
        assert!(studio.walk.is_some(), "still walking");
        let _ = studio.view();
    }

    #[test]
    fn the_open_block_shows_its_slab_and_the_file_view_opens_the_block_first() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_room(directory.path());
        set_plan_box(&mut studio);
        assert_eq!(studio.drawing_slab(), None);

        // The File view entry opens the block, where the view is chosen,
        // instead of saving a plan at once; the next time it saves.
        let _ = studio.update(Message::FileAction(FileAction::ExportDrawing));
        assert!(studio.drawing.open);
        assert!(!studio.drawing.dialog_pending);
        assert!(studio
            .status
            .starts_with("Section drawing: choose the view"));
        let _ = studio.update(Message::FileAction(FileAction::ExportDrawing));
        assert!(studio.drawing.dialog_pending);
        let _ = studio.update(Message::Drawing(DrawingAction::PathChosen(None)));

        // The slab of the plan lies under the top face of the box, that of
        // a front view behind its face at Y min.
        let plan = studio.drawing_slab().unwrap();
        assert_eq!(plan.bounds.max[2], PLAN_BOX.max[2]);
        assert!((plan.bounds.min[2] - (PLAN_BOX.max[2] - 0.1)).abs() < 1e-12);
        let _ = studio.update(Message::Drawing(DrawingAction::View(DrawingView::Front)));
        let front = studio.drawing_slab().unwrap();
        assert_eq!(front.bounds.min[1], PLAN_BOX.min[1]);
        assert!((front.bounds.max[1] - (PLAN_BOX.min[1] + 0.1)).abs() < 1e-12);
        assert!(tr(cut_plane_text(DrawingView::Front)).contains("Y min"));
        let _ = studio.view();
        // Without a number for the slab there is no slab to show.
        let _ = studio.update(Message::Drawing(DrawingAction::Thickness("x".into())));
        assert_eq!(studio.drawing_slab(), None);
    }

    #[test]
    fn block_is_translated() {
        let _language = TestLanguage::hold(Language::Table(0));
        assert_eq!(tr("Section drawing"), "Snedetekening");
        assert_eq!(tr(view_text(DrawingView::Plan)), "Plattegrond");
        assert_eq!(
            Choice {
                value: DrawingUnits::Millimetres,
                text: units_text(DrawingUnits::Millimetres),
            }
            .to_string(),
            "Millimeters"
        );
        assert_eq!(
            tr_args(
                "{regions} · {dropped} dropped",
                &[("regions", &3), ("dropped", &2)]
            ),
            "3 · 2 weggelaten"
        );
        assert_eq!(tr("Slab drawn"), "Getekende plak");
        assert_eq!(
            tr_args("{depth} mm (box is thinner)", &[("depth", &50)]),
            "50 mm (box is dunner)"
        );
    }

    #[test]
    fn command_line_draws_a_box_of_a_scan_file() {
        let directory = tempfile::tempdir().unwrap();
        let (text, in_slab) = room_xyz();
        let source = directory.path().join("room.xyz");
        std::fs::write(&source, text).unwrap();
        // The room, the box of its plan, an output in the folder of the test
        // and the options that follow.
        let arguments = |output: &str, options: &[&str]| -> Vec<OsString> {
            [
                source.clone().into_os_string(),
                "-0.5,-0.5,0.5,4.5,3.5,1.1".into(),
                directory.path().join(output).into_os_string(),
            ]
            .into_iter()
            .chain(options.iter().map(OsString::from))
            .collect()
        };

        let line = command_line(&arguments("plan.dxf", &[])).unwrap();
        assert!(
            line.starts_with(&format!(
                "Drawing written as DXF, view plan: {in_slab} points in the slab, "
            )),
            "{line}"
        );
        assert!(
            line.contains("; 1 region, grid 20 mm, main direction 0.00°; "),
            "{line}"
        );
        assert!(line.ends_with(&format!(
            "-> {}",
            directory.path().join("plan.dxf").display()
        )));
        let (kinds, _) = dxf_entities(&directory.path().join("plan.dxf"));
        assert_eq!(kinds["HATCH"], 1);
        let millimetres = pointcloud_core::open(directory.path().join("plan.dxf"), 10).unwrap();

        // Metres, without a fill, as DWG.
        let line =
            command_line(&arguments("plan.dwg", &["--fill", "off", "--units", "m"])).unwrap();
        assert!(
            line.starts_with("Drawing written as DWG, view plan: "),
            "{line}"
        );
        assert!(!line.contains("region"), "{line}");
        let line = command_line(&arguments("metres.dxf", &["--units", "m"])).unwrap();
        assert!(line.contains("1 region"), "{line}");
        let in_metres = pointcloud_core::open(directory.path().join("metres.dxf"), 10).unwrap();
        assert_eq!(in_metres.total_points, millimetres.total_points);
        assert!((millimetres.bounds.max[0] - in_metres.bounds.max[0] * 1000.0).abs() < 1e-3);

        // A vertical section looks at the front of the box: the whole depth
        // of the room in a slab of 4 m, points only unless a fill is asked.
        let line = command_line(&arguments(
            "front.dxf",
            &["--thickness", "4", "--view", "front"],
        ))
        .unwrap();
        assert!(
            line.starts_with("Drawing written as DXF, view front: "),
            "{line}"
        );
        assert!(!line.contains("region"), "{line}");
        let (kinds, _) = dxf_entities(&directory.path().join("front.dxf"));
        assert!(!kinds.contains_key("HATCH"));
        let line = command_line(&arguments(
            "front-filled.dxf",
            &["--view", "front", "--fill", "on", "--thickness", "4"],
        ))
        .unwrap();
        assert!(line.contains("region"), "{line}");

        let refused =
            |options: &[&str]| command_line(&arguments("refused.dxf", options)).unwrap_err();
        assert_eq!(
            command_line(&arguments("plan.pdf", &[])).unwrap_err(),
            (2, "Supported drawing extensions: .dxf, .dwg".to_owned())
        );
        assert_eq!(refused(&["--view"]), (2, String::new()));
        assert_eq!(refused(&["--scale", "2"]), (2, String::new()));
        assert_eq!(
            refused(&["--view", "top"]),
            (
                2,
                "--view must be plan, front, back, left or right".to_owned()
            )
        );
        assert_eq!(
            refused(&["--units", "cm"]),
            (2, "--units must be mm or m".to_owned())
        );
        assert_eq!(
            refused(&["--fill", "yes"]),
            (2, "--fill must be on or off".to_owned())
        );
        assert_eq!(
            refused(&["--thickness", "9"]),
            (2, "Slab thickness must be between 0.005 and 5 m".to_owned())
        );
        assert_eq!(command_line(&[]).unwrap_err(), (2, String::new()));
        for limits in ["0,0,0,1,1", "0,0,0,1,1,x", "0,0,0,1,1,1,1"] {
            let mut wrong = arguments("refused.dxf", &[]);
            wrong[1] = limits.into();
            assert_eq!(
                command_line(&wrong).unwrap_err(),
                (
                    2,
                    "Section limits must be six comma-separated numbers".to_owned()
                ),
                "{limits}"
            );
        }
        assert!(!directory.path().join("refused.dxf").exists());

        // A box that is no box and an output folder that does not exist are
        // refused before the input is opened: here there is no input at all.
        let unopened = |limits: &str, output: PathBuf| {
            command_line(&[
                directory.path().join("no-such-scan.e57").into_os_string(),
                limits.into(),
                output.into_os_string(),
            ])
            .unwrap_err()
        };
        let not_a_box = (
            2,
            "Section limits must be finite numbers that run from the minimum to the maximum"
                .to_owned(),
        );
        for limits in [
            "4.5,3.5,1.1,-0.5,-0.5,0.5",
            "0,0,nan,1,1,1",
            "0,0,0,1,inf,1",
        ] {
            assert_eq!(
                unopened(limits, directory.path().join("refused.dxf")),
                not_a_box,
                "{limits}"
            );
        }
        assert_eq!(
            unopened("0,0,0,0,1,1", directory.path().join("refused.dxf")),
            (2, "The section box has no size in this view".to_owned())
        );
        assert_eq!(
            unopened("0,0,0,1,1,1", directory.path().join("missing/plan.dxf")),
            (2, "The folder of the output path does not exist".to_owned())
        );
        // A bare file name stands in the current folder, which exists: the
        // refusal is that of the input.
        let (code, line) = unopened("0,0,0,1,1,1", PathBuf::from("bare-name.dxf"));
        assert_eq!(code, 1, "{line}");
        assert!(line.starts_with("Drawing failed: "), "{line}");
        assert!(!Path::new("bare-name.dxf").exists());

        // A box that is thinner than the slab says so in the line.
        let mut thin = arguments("thin.dxf", &[]);
        thin[1] = "-0.5,-0.5,1.05,4.5,3.5,1.1".into();
        let line = command_line(&thin).unwrap();
        assert!(
            line.contains(&format!(
                "{} points in the slab of 50 mm (the box is thinner than the 100 mm asked)",
                in_slab / 2
            )),
            "{line}"
        );

        // A box beside the scan holds nothing, and nothing is written.
        let mut beside = arguments("empty.dxf", &[]);
        beside[1] = "100,100,0,101,101,1".into();
        assert_eq!(
            command_line(&beside).unwrap_err(),
            (
                1,
                format!(
                    "Drawing failed: {EMPTY_SLAB}: {}",
                    empty_slab_hint(DrawingView::Plan)
                )
            )
        );
        assert!(!directory.path().join("empty.dxf").exists());
        // The scan itself is never the output.
        let scan = directory.path().join("plan.dxf");
        let over_itself = [
            scan.clone().into_os_string(),
            "0,0,0,1,1,1".into(),
            scan.into_os_string(),
        ];
        assert_eq!(
            command_line(&over_itself).unwrap_err(),
            (
                2,
                "Choose an output path different from the input".to_owned()
            )
        );
    }

    /// Make a plan of the room at 1.1 m with Create 2D from a share of its
    /// points, and answer the finished job.
    fn make_plan_of(studio: &mut Studio, percent: f64) -> Value {
        let answer = send(
            studio,
            serde_json::from_str(&format!(
                r#"{{"command":"create_drawing","kind":"plan","height":1.1,"sample_percent":{percent}}}"#
            ))
            .unwrap(),
        );
        assert_eq!(answer["ok"], true, "{answer}");
        finish(studio);
        job(studio, answer["job_id"].as_str().unwrap())
    }

    /// Change the crop region of a drawing and answer the finished job.
    fn crop_job(studio: &mut Studio, guid: &str, change: Value) -> Value {
        let name = definition(studio, guid).name;
        let mut command = json!({"command": "set_sheet_crop", "name": name});
        for (key, value) in change.as_object().unwrap() {
            command[key] = value.clone();
        }
        let answer = send(studio, serde_json::from_value(command).unwrap());
        assert_eq!(answer["ok"], true, "{answer}");
        finish(studio);
        job(studio, answer["job_id"].as_str().unwrap())
    }

    #[test]
    fn a_crop_region_changed_within_the_slab_is_drawn_from_the_points_read() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, in_slab) = studio_with_room(directory.path());
        let kept = |studio: &mut Studio| {
            send(studio, ApiCommand::Status)["result"]["drawing_view"]["kept"].clone()
        };

        // Every point: the slab of the plan is that of the room, read once.
        let made = make_plan_of(&mut studio, 100.0);
        assert_eq!(made["state"], "complete", "{made}");
        assert_eq!(made["slab_points"], in_slab);
        assert!(made["read_points"].as_u64().unwrap() > 0, "{made}");
        assert_eq!(made["reused_points"], 0);
        let guid = made["guid"].as_str().unwrap().to_owned();
        assert_eq!(definition(&studio, &guid).request.sample_percent, 100.0);
        let held = kept(&mut studio);
        assert_eq!(held["drawings"], 1, "{held}");
        assert!(held["points"].as_u64().unwrap() >= in_slab, "{held}");

        // A smaller crop region and one moved within it: nothing is read.
        let crop = crate::drawing_crop::crop_value(&definition(&studio, &guid));
        let (width, centre) = (crop["width"].as_f64().unwrap(), crop["center"].clone());
        for change in [
            json!({"width": width - 1.0}),
            json!({"center": [centre[0].as_f64().unwrap() + 0.3, centre[1].as_f64().unwrap()]}),
        ] {
            let remade = crop_job(&mut studio, &guid, change);
            assert_eq!(remade["state"], "complete", "{remade}");
            assert_eq!(remade["read_points"], 0, "{remade}");
            assert!(remade["reused_points"].as_u64().unwrap() > 0, "{remade}");
            assert!(
                studio
                    .status
                    .ends_with("made again from the points it had read"),
                "{}",
                studio.status
            );
        }
        // Another cut reads the slab again.
        let lower = crop_job(&mut studio, &guid, json!({"cut": 1.05}));
        assert!(lower["read_points"].as_u64().unwrap() > 0, "{lower}");
        assert_eq!(lower["reused_points"], 0);
        // The filled cut of a plan is traced from every point, which the
        // drawing keeps: other points used read nothing either.
        let half = crop_job(&mut studio, &guid, json!({"sample_percent": 50.0}));
        assert_eq!(half["read_points"], 0, "{half}");
        assert!(
            half["drawn_points"].as_u64() < lower["drawn_points"].as_u64(),
            "{half}"
        );
        assert_eq!(half["slab_points"], lower["slab_points"]);
        assert_eq!(half["crop"]["sample_percent"], 50.0);
        assert_eq!(definition(&studio, &guid).request.sample_percent, 50.0);
        let name = definition(&studio, &guid).name;
        let refused = send(
            &mut studio,
            serde_json::from_value(json!({
                "command": "set_sheet_crop",
                "name": name,
                "sample_percent": 0.05,
            }))
            .unwrap(),
        );
        assert_eq!(refused["ok"], false, "{refused}");

        // A drawing that is deleted lets go of its points.
        assert_eq!(studio.api_delete_drawing(&name)["ok"], true);
        assert_eq!(kept(&mut studio)["drawings"], 0);
    }

    #[test]
    fn the_points_a_drawing_kept_go_with_the_scans_it_was_made_from() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        let kept = |studio: &mut Studio| {
            send(studio, ApiCommand::Status)["result"]["drawing_view"]["kept"].clone()
        };
        let made = make_plan_of(&mut studio, 100.0);
        assert_eq!(made["state"], "complete", "{made}");
        assert_eq!(kept(&mut studio)["drawings"], 1);
        // New in the File view closes every scan: the points kept from
        // them could never be drawn again, so they go at once.
        let _ = studio.file_action(crate::file_view::FileAction::NewWorkspace);
        assert!(studio.clouds.is_empty());
        let held = kept(&mut studio);
        assert_eq!((&held["drawings"], &held["bytes"]), (&json!(0), &json!(0)));
    }

    #[test]
    fn a_plan_of_create_2d_uses_a_tenth_of_the_points_unless_told_otherwise() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, in_slab) = studio_with_room(directory.path());
        let mut plan = || {
            let answer = send(
                &mut studio,
                serde_json::from_str(r#"{"command":"create_drawing","kind":"plan","height":1.1}"#)
                    .unwrap(),
            );
            assert_eq!(answer["ok"], true, "{answer}");
            finish(&mut studio);
            job(&mut studio, answer["job_id"].as_str().unwrap())
        };
        let made = plan();
        let again = plan();
        let guid = made["guid"].as_str().unwrap().to_owned();
        assert_eq!(definition(&studio, &guid).request.sample_percent, 10.0);
        // The filled cut takes every point of the slab; the points drawn are
        // thinned from a tenth of them, the same points every time.
        assert_eq!(made["slab_points"], in_slab);
        let every = make_plan(&mut studio, 1.1);
        let all = studio.drawing.last.as_ref().unwrap().value();
        assert_eq!(definition(&studio, &every).request.sample_percent, 100.0);
        let drawn = made["drawn_points"].as_u64().unwrap();
        assert!(
            drawn > 0 && drawn < all["drawn_points"].as_u64().unwrap(),
            "{made} {all}"
        );
        assert_eq!(made["regions"], all["regions"]);
        assert_ne!(again["guid"], made["guid"]);
        assert_eq!(again["drawn_points"], drawn);
        // A share outside 0.1 to 100 is refused before anything is made.
        for percent in [0.05, 120.0] {
            let answer = send(
                &mut studio,
                serde_json::from_str(&format!(
                    r#"{{"command":"create_drawing","kind":"plan","height":1.1,"sample_percent":{percent}}}"#
                ))
                .unwrap(),
            );
            assert_eq!(answer["ok"], false, "{answer}");
            assert!(
                answer["error"]
                    .as_str()
                    .unwrap()
                    .starts_with("The points used must lie between"),
                "{answer}"
            );
        }
    }

    #[test]
    fn a_click_on_the_crop_region_selects_it_and_properties_shows_its_figures() {
        use crate::drawing_crop::{CropAction, Field};

        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let (mut studio, _) = studio_with_room(directory.path());
        let guid = make_plan(&mut studio, 1.1);
        let selected = |studio: &mut Studio| {
            send(studio, ApiCommand::Status)["result"]["drawing_view"]["crop_selected"].clone()
        };
        assert_eq!(selected(&mut studio), false);
        assert!(studio.crop_overlay().is_some_and(|crop| !crop.selected));
        assert!(studio.crop_properties().is_none());

        let _ = studio.update(Message::Crop(CropAction::Select(true)));
        assert_eq!(selected(&mut studio), true);
        assert!(studio.crop_overlay().is_some_and(|crop| crop.selected));
        assert!(studio.crop_properties().is_some());
        assert!(
            studio.status.starts_with("Crop region of"),
            "{}",
            studio.status
        );
        let _ = studio.view();

        // Points used is a figure of Properties as well.
        let _ = studio.update(Message::Crop(CropAction::Field(
            Field::Points,
            "25 %".into(),
        )));
        let _ = studio.update(Message::Crop(CropAction::Apply(Field::Points)));
        assert!(studio.drawing.job.is_some(), "{}", studio.status);
        finish(&mut studio);
        assert_eq!(definition(&studio, &guid).request.sample_percent, 25.0);
        // Made again, it stays selected.
        assert_eq!(selected(&mut studio), true);

        // Escape closes the File view over it first, then deselects it; so
        // does a click elsewhere, and hiding it.
        let _ = studio.update(Message::ToggleFile);
        assert!(studio.file_open);
        let _ = studio.update(Message::Escape);
        assert!(!studio.file_open);
        assert_eq!(selected(&mut studio), true, "hidden, it stays selected");
        // So does the card of the Pointcloud to Drawing wizard.
        let _ = studio.update(Message::MeshToPlans(
            crate::mesh_to_plans::WizardAction::Open,
        ));
        assert!(studio.mesh_to_plans.covers_model());
        let _ = studio.update(Message::Escape);
        assert!(!studio.mesh_to_plans.covers_model());
        assert_eq!(selected(&mut studio), true);
        let _ = studio.update(Message::MeshToPlans(
            crate::mesh_to_plans::WizardAction::Close,
        ));
        let _ = studio.update(Message::Escape);
        assert_eq!(selected(&mut studio), false);
        assert!(studio.crop_properties().is_none());
        let answer = send(&mut studio, ApiCommand::SelectCropRegion { selected: true });
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["changed"], true);
        assert_eq!(answer["crop"]["sample_percent"], 25.0);
        let _ = studio.update(Message::Crop(CropAction::Select(false)));
        assert_eq!(selected(&mut studio), false);
        let _ = studio.update(Message::Crop(CropAction::Select(true)));
        let _ = studio.update(Message::Crop(CropAction::Show(false)));
        assert_eq!(selected(&mut studio), false);
        let refused = send(&mut studio, ApiCommand::SelectCropRegion { selected: true });
        assert_eq!(refused["ok"], false, "{refused}");
        let _ = studio.update(Message::Crop(CropAction::Show(true)));

        // A drag of a handle by the local API selects it, as the pointer
        // needs it selected.
        let crop = studio.crop_overlay().unwrap();
        let answer = send(
            &mut studio,
            ApiCommand::DragCropHandle {
                handle: "right".into(),
                to: [crop.rect[1][0] - 200.0, 0.0],
                release: false,
            },
        );
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(selected(&mut studio), true);
        // Another drawing in the view starts without a selection.
        let other = make_plan(&mut studio, 1.0);
        assert_ne!(other, guid);
        assert_eq!(selected(&mut studio), false);
    }
}
