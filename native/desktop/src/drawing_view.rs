//! The Drawing view: a 2D drawing in the main area in place of the 3D scene.
//! It shows the drawing the Section drawing tool made last, by a preview or
//! an export, or a DXF or DWG file opened from the File view, on a light
//! sheet: points as dots, filled cuts with their holes, polylines, the frame
//! and the texts, with a switch per layer, pan and zoom, a scale bar and the
//! coordinates under the pointer in drawing units.
//!
//! The lines, fills and points are tessellated once into a cached geometry
//! that is moved as the view pans; zooming scales it until the wheel rests,
//! and only then is it built again at the new scale. Texts, the scale bar
//! and the coordinates are drawn over it every frame.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use iced::advanced::graphics::geometry::Renderer as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::widget::{self, Widget};
use iced::alignment;
use iced::mouse;
use iced::widget::canvas::{self, event, Canvas, Frame};
use iced::widget::{button, checkbox, column, container, row, stack, text};
use iced::{
    Background, Color, Element, Fill, Length, Pixels, Point as UiPoint, Rectangle, Renderer, Size,
    Task, Theme, Transformation, Vector,
};
use pointcloud_core::{Drawing2d, DrawingEntity, DrawingUnits, ReadDrawing};
use serde_json::{json, Value};

use crate::bag_panel::plain_reason;
use crate::i18n::{tr, tr_args};
use crate::saved_drawings::SavedDrawing;
use crate::ui_theme::UiTheme;
use crate::{
    flat_tool_style, format_count, muted_checkbox_style, opencad_properties, Message, Studio,
};

/// Wheel steps of a mouse turn the scale by this much each.
const ZOOM_STEP: f32 = 1.25;
/// After the last wheel step the geometry is built again at the new scale
/// once this much time has passed; until then it is scaled.
const ZOOM_SETTLE: Duration = Duration::from_millis(180);
/// A scaled geometry is used while the scale stays within this factor of the
/// one it was built at.
const MAX_SCALED: f64 = 16.0;
/// The part of the sheet around a drawing that zoom extents leaves free.
const FIT_MARGIN: f64 = 0.05;
/// Pixels per drawing unit, at least and at most.
const MIN_SCALE: f64 = 1e-9;
const MAX_SCALE: f64 = 1e6;
/// The strip at the bottom of the sheet with the scale bar and the
/// coordinates, which zoom extents keeps free.
const BOTTOM_STRIP: f32 = 30.0;
/// Texts smaller than this many pixels are not drawn.
const MIN_TEXT_PIXELS: f32 = 3.0;
/// The most texts drawn in one frame.
const MAX_TEXTS: usize = 4_000;
/// The width of the scale bar is about this many pixels.
const SCALE_BAR_PIXELS: f64 = 120.0;
/// Up to this many points a dot is two pixels wide; with more it is one
/// and a half, so that a dense plan stays readable.
const LARGE_DOTS_UP_TO: usize = 100_000;

/// Where the drawing in the view came from.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DrawingSource {
    /// The preview of the Section drawing tool.
    Preview,
    /// An export of the Section drawing tool, to this file.
    Export(PathBuf),
    /// A DXF or DWG file that was opened.
    File(PathBuf),
    /// A drawing made with Create 2D plan / elevation / section, by the
    /// identifier of how it was made and its name.
    Sheet { guid: String, name: String },
}

impl DrawingSource {
    fn kind(&self) -> &'static str {
        match self {
            Self::Preview => "preview",
            Self::Export(_) => "export",
            Self::File(_) => "file",
            Self::Sheet { .. } => "sheet",
        }
    }

    fn path(&self) -> Option<&Path> {
        match self {
            Self::Preview | Self::Sheet { .. } => None,
            Self::Export(path) | Self::File(path) => Some(path),
        }
    }

    /// The identifier of how a drawing of the Project Browser was made.
    pub(crate) fn sheet_guid(&self) -> Option<&str> {
        match self {
            Self::Sheet { guid, .. } => Some(guid),
            _ => None,
        }
    }

    /// What the header of the view and the Project Browser say it shows.
    pub(crate) fn caption(&self) -> String {
        if let Self::Sheet { name, .. } = self {
            return name.clone();
        }
        match self.path().and_then(Path::file_name) {
            Some(name) => name.to_string_lossy().into_owned(),
            None => tr("Preview of the section drawing").to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScenePoint {
    pub at: [f64; 2],
    pub rgb: Option<[u8; 3]>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SceneLine {
    pub points: Vec<[f64; 2]>,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SceneText {
    pub at: [f64; 2],
    pub height: f64,
    pub rotation: f64,
    pub value: String,
}

/// One layer of a drawing, its entities sorted by kind. Coordinates are in
/// drawing units.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SceneLayer {
    pub name: String,
    pub rgb: [u8; 3],
    pub points: Vec<ScenePoint>,
    pub lines: Vec<SceneLine>,
    /// Each fill as its rings, the outer one first.
    pub fills: Vec<Vec<Vec<[f64; 2]>>>,
    pub texts: Vec<SceneText>,
}

impl SceneLayer {
    fn entities(&self) -> usize {
        self.points.len() + self.lines.len() + self.fills.len() + self.texts.len()
    }
}

/// A drawing as the view shows it: per layer what it holds, in the units of
/// the drawing, and what the file held that is not shown.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DrawScene {
    pub source: DrawingSource,
    pub units: DrawingUnits,
    pub layers: Vec<SceneLayer>,
    /// Lower left and upper right corner of everything drawn; a text counts
    /// with its insertion point.
    pub extents: Option<[[f64; 2]; 2]>,
    /// Layers that start hidden: those the file has switched off.
    pub hidden: Vec<String>,
    pub units_named: bool,
    pub skipped: BTreeMap<String, usize>,
    pub skipped_3d: usize,
    pub inserts: usize,
}

impl DrawScene {
    /// The view of a drawing model, whose coordinates are in metres.
    pub(crate) fn from_drawing(drawing: &Drawing2d, source: DrawingSource) -> Self {
        let factor = drawing.units.factor();
        let scaled = |uv: [f64; 2]| [uv[0] * factor, uv[1] * factor];
        let mut layers: Vec<SceneLayer> = drawing
            .layers
            .iter()
            .map(|layer| SceneLayer {
                name: layer.name.clone(),
                rgb: layer.rgb,
                points: Vec::new(),
                lines: Vec::new(),
                fills: Vec::new(),
                texts: Vec::new(),
            })
            .collect();
        for (layer, entity) in &drawing.entities {
            let Some(layer) = layers.get_mut(usize::from(*layer)) else {
                continue;
            };
            match entity {
                DrawingEntity::Point { uv, rgb } => layer.points.push(ScenePoint {
                    at: scaled(*uv),
                    rgb: *rgb,
                }),
                DrawingEntity::Polyline { points, closed } => layer.lines.push(SceneLine {
                    points: points.iter().copied().map(scaled).collect(),
                    closed: *closed,
                }),
                DrawingEntity::Fill { outer, holes } => layer.fills.push(
                    std::iter::once(outer)
                        .chain(holes)
                        .map(|ring| ring.iter().copied().map(scaled).collect())
                        .collect(),
                ),
                DrawingEntity::Text {
                    at,
                    height,
                    rotation,
                    value,
                } => layer.texts.push(SceneText {
                    at: scaled(*at),
                    height: height * factor,
                    rotation: *rotation,
                    value: value.clone(),
                }),
            }
        }
        Self {
            source,
            units: drawing.units,
            layers,
            extents: drawing
                .extents()
                .map(|[min, max]| [scaled(min), scaled(max)]),
            hidden: Vec::new(),
            units_named: true,
            skipped: BTreeMap::new(),
            skipped_3d: 0,
            inserts: 0,
        }
    }

    /// The view of a file that was read.
    pub(crate) fn from_file(read: ReadDrawing, path: PathBuf) -> Self {
        let mut scene = Self::from_drawing(&read.drawing, DrawingSource::File(path));
        scene.hidden = read.hidden_layers;
        scene.units_named = read.units_named;
        scene.skipped = read.skipped;
        scene.skipped_3d = read.skipped_3d;
        scene.inserts = read.inserts;
        scene
    }

    /// Points, polylines, fills and texts in the whole drawing.
    pub(crate) fn totals(&self) -> [usize; 4] {
        self.layers.iter().fold([0; 4], |mut sum, layer| {
            sum[0] += layer.points.len();
            sum[1] += layer.lines.len();
            sum[2] += layer.fills.len();
            sum[3] += layer.texts.len();
            sum
        })
    }

    fn skipped_total(&self) -> usize {
        self.skipped.values().sum()
    }
}

/// What the view looks at: the drawing point in the middle of the sheet and
/// how many pixels a drawing unit takes. The sheet has its Y axis down, the
/// drawing up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ViewCamera {
    pub center: [f64; 2],
    pub scale: f64,
}

impl Default for ViewCamera {
    fn default() -> Self {
        Self {
            center: [0.0, 0.0],
            scale: 1.0,
        }
    }
}

impl ViewCamera {
    /// The pixel of the sheet, from its top left corner, where a drawing
    /// point lies.
    pub(crate) fn to_screen(self, point: [f64; 2], size: Size) -> [f64; 2] {
        [
            (point[0] - self.center[0]) * self.scale + f64::from(size.width) / 2.0,
            f64::from(size.height) / 2.0 - (point[1] - self.center[1]) * self.scale,
        ]
    }

    /// The drawing point under a pixel of the sheet.
    pub(crate) fn to_drawing(self, pixel: [f32; 2], size: Size) -> [f64; 2] {
        [
            self.center[0] + (f64::from(pixel[0]) - f64::from(size.width) / 2.0) / self.scale,
            self.center[1] - (f64::from(pixel[1]) - f64::from(size.height) / 2.0) / self.scale,
        ]
    }

    /// The camera zoomed by `factor` about a pixel: the drawing point under
    /// it stays there.
    pub(crate) fn zoomed(&self, factor: f64, pixel: [f32; 2], size: Size) -> Self {
        let anchor = self.to_drawing(pixel, size);
        let scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        Self {
            center: [
                anchor[0] - (f64::from(pixel[0]) - f64::from(size.width) / 2.0) / scale,
                anchor[1] + (f64::from(pixel[1]) - f64::from(size.height) / 2.0) / scale,
            ],
            scale,
        }
    }

    /// The camera after the sheet was dragged by some pixels.
    pub(crate) fn panned(&self, delta: [f32; 2]) -> Self {
        Self {
            center: [
                self.center[0] - f64::from(delta[0]) / self.scale,
                self.center[1] + f64::from(delta[1]) / self.scale,
            ],
            scale: self.scale,
        }
    }

    /// The camera that shows all of `extents` with a margin around it, above
    /// the strip at the bottom of the sheet that holds the scale bar.
    pub(crate) fn fit(extents: [[f64; 2]; 2], size: Size) -> Self {
        let [min, max] = extents;
        let width = (max[0] - min[0]).max(0.0);
        let height = (max[1] - min[1]).max(0.0);
        let above = (size.height - BOTTOM_STRIP).max(size.height / 2.0);
        let room = |pixels: f32| f64::from(pixels.max(1.0)) * (1.0 - 2.0 * FIT_MARGIN);
        let scale = [(width, room(size.width)), (height, room(above))]
            .into_iter()
            .filter(|(length, _)| *length > 0.0)
            .map(|(length, pixels)| pixels / length)
            .fold(f64::INFINITY, f64::min);
        let scale = if scale.is_finite() { scale } else { 1.0 }.clamp(MIN_SCALE, MAX_SCALE);
        // The middle of the drawing goes to the middle of the part above
        // the strip.
        let lift = f64::from(size.height - above) / 2.0 / scale;
        Self {
            center: [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0 - lift],
            scale,
        }
    }

    /// The part of the drawing the sheet shows.
    pub(crate) fn visible(&self, size: Size) -> [[f64; 2]; 2] {
        let half = [
            f64::from(size.width) / 2.0 / self.scale,
            f64::from(size.height) / 2.0 / self.scale,
        ];
        [
            [self.center[0] - half[0], self.center[1] - half[1]],
            [self.center[0] + half[0], self.center[1] + half[1]],
        ]
    }
}

fn contains(outer: [[f64; 2]; 2], inner: [[f64; 2]; 2]) -> bool {
    (0..2).all(|axis| outer[0][axis] <= inner[0][axis] && inner[1][axis] <= outer[1][axis])
}

fn overlaps(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> bool {
    (0..2).all(|axis| a[0][axis] <= b[1][axis] && b[0][axis] <= a[1][axis])
}

fn bounds_of<'a>(points: impl IntoIterator<Item = &'a [f64; 2]>) -> Option<[[f64; 2]; 2]> {
    let mut bounds: Option<[[f64; 2]; 2]> = None;
    for point in points {
        let [min, max] = bounds.get_or_insert([*point, *point]);
        for axis in 0..2 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    bounds
}

/// The ink of a layer or point colour on the light sheet. White and black
/// are the colour that is black on a light background; colours too light to
/// read on the sheet are darkened.
pub(crate) fn ink(rgb: [u8; 3]) -> Color {
    match rgb {
        [0, 0, 0] | [255, 255, 255] => Color::from_rgb8(24, 24, 27),
        [r, g, b] => {
            let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
            if luminance > 215.0 {
                let darker = |value: u8| (f32::from(value) * 0.6) as u8;
                Color::from_rgb8(darker(r), darker(g), darker(b))
            } else {
                Color::from_rgb8(r, g, b)
            }
        }
    }
}

/// The sheet of a theme: white for the light themes, warm or cool off-white
/// beside the dark ones.
pub(crate) fn paper(theme: UiTheme) -> Color {
    match theme {
        UiTheme::Light | UiTheme::Contrast => Color::WHITE,
        UiTheme::Blueprint => Color::from_rgb8(242, 246, 252),
        UiTheme::Forge | UiTheme::Night => Color::from_rgb8(248, 247, 244),
    }
}

/// The geometry that was built last, and for what.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Built {
    camera: ViewCamera,
    /// The drawing point at the top left corner of the geometry.
    origin: [f64; 2],
    /// The part of the drawing the geometry holds.
    region: [[f64; 2]; 2],
    /// Whether that part holds the whole drawing.
    whole: bool,
    size: Size,
    revision: u64,
}

/// A file being read into the view.
#[derive(Debug, Clone)]
struct Reading {
    serial: u64,
    path: PathBuf,
    api_job_id: Option<String>,
}

/// The state of the Drawing view.
pub(crate) struct DrawingViewTool {
    /// The main area shows the drawing instead of the 3D scene.
    pub(crate) shown: bool,
    scene: Option<Arc<DrawScene>>,
    /// Per layer of the scene whether it is shown.
    visible: Vec<bool>,
    camera: Cell<ViewCamera>,
    /// Zoom extents as soon as the size of the sheet is known.
    fit_pending: Cell<bool>,
    /// The wheel has rested since the last zoom step.
    settled: bool,
    zoom_serial: u64,
    /// Where the sheet lies in the window, as it was last drawn.
    bounds: Cell<Option<Rectangle>>,
    cache: canvas::Cache,
    built: RefCell<Option<Built>>,
    /// Changes when what the geometry shows changes.
    revision: u64,
    reading: Option<Reading>,
    next_read: u64,
    /// Switch to the view when an export of the Section drawing tool ends.
    pub(crate) show_after_export: bool,
    /// Why the last file could not be read.
    last_error: Option<String>,
    /// The drawings of this session that the project browser lists under
    /// Files, the newest first: the last preview, every export and every
    /// opened file.
    sheets: Vec<Arc<DrawScene>>,
    /// How each drawing made with Create 2D plan / elevation / section was
    /// made, kept across sessions, in the order they were made.
    pub(crate) saved: Vec<SavedDrawing>,
    /// The drawings of `saved` that have been made in this session.
    made: Vec<Arc<DrawScene>>,
}

/// The project browser keeps this many previews, exports and opened files
/// of a session. Drawings made with Create 2D plan / elevation / section
/// stay, however many there are.
const MAX_SHEETS: usize = 16;

impl Default for DrawingViewTool {
    fn default() -> Self {
        Self::new(true)
    }
}

impl DrawingViewTool {
    pub(crate) fn new(show_after_export: bool) -> Self {
        Self {
            shown: false,
            scene: None,
            visible: Vec::new(),
            camera: Cell::new(ViewCamera::default()),
            fit_pending: Cell::new(false),
            settled: true,
            zoom_serial: 0,
            bounds: Cell::new(None),
            cache: canvas::Cache::new(),
            built: RefCell::new(None),
            revision: 0,
            reading: None,
            next_read: 0,
            show_after_export,
            last_error: None,
            sheets: Vec::new(),
            saved: crate::saved_drawings::load(),
            made: Vec::new(),
        }
    }

    pub(crate) fn scene(&self) -> Option<&DrawScene> {
        self.scene.as_deref()
    }

    /// Whether a drawing of the Section drawing tool is in the view.
    pub(crate) fn has_section_drawing(&self) -> bool {
        self.scene
            .as_ref()
            .is_some_and(|scene| !matches!(scene.source, DrawingSource::File(_)))
    }

    pub(crate) fn camera(&self) -> ViewCamera {
        self.camera.get()
    }

    /// Where the sheet lies in the window, when it was drawn.
    pub(crate) fn canvas_bounds(&self) -> Option<Rectangle> {
        self.bounds.get()
    }

    pub(crate) fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    /// The size of the sheet: as it was last drawn, else `fallback`.
    fn sheet_size(&self, fallback: Size) -> Size {
        self.bounds
            .get()
            .map(|bounds| bounds.size())
            .filter(|size| size.width > 0.0 && size.height > 0.0)
            .unwrap_or(fallback)
    }

    fn invalidate(&mut self) {
        self.revision += 1;
        self.cache.clear();
        self.built.replace(None);
    }

    /// Show a drawing and list it in the project browser: a new preview
    /// takes the place of the last one, and a file the place of an earlier
    /// drawing of the same file.
    pub(crate) fn set_scene(&mut self, scene: Arc<DrawScene>) {
        if let Some(guid) = scene.source.sheet_guid() {
            // A drawing of Create 2D takes the place of the one made before
            // in the same way, and is never dropped for want of room.
            self.made
                .retain(|earlier| earlier.source.sheet_guid() != Some(guid));
            self.made.push(Arc::clone(&scene));
            self.show_scene(scene);
            return;
        }
        let same = |earlier: &Arc<DrawScene>| match (scene.source.path(), earlier.source.path()) {
            (Some(path), Some(other)) => path == other,
            (None, None) => earlier.source == scene.source,
            _ => false,
        };
        self.sheets.retain(|earlier| !same(earlier));
        self.sheets.insert(0, Arc::clone(&scene));
        self.sheets.truncate(MAX_SHEETS);
        self.show_scene(scene);
    }

    /// The previews, exports and opened files the project browser lists,
    /// the newest first.
    pub(crate) fn sheets(&self) -> &[Arc<DrawScene>] {
        &self.sheets
    }

    /// The drawing made in this session from how a drawing was made.
    pub(crate) fn made(&self, guid: &str) -> Option<&Arc<DrawScene>> {
        self.made
            .iter()
            .find(|scene| scene.source.sheet_guid() == Some(guid))
    }

    /// Whether this listed drawing is the one in the view.
    pub(crate) fn is_current(&self, sheet: &Arc<DrawScene>) -> bool {
        self.scene
            .as_ref()
            .is_some_and(|scene| Arc::ptr_eq(scene, sheet))
    }

    /// The identifier of the drawing of Create 2D the view shows, while it
    /// is shown.
    pub(crate) fn shown_guid(&self) -> Option<&str> {
        self.scene
            .as_ref()
            .filter(|_| self.shown)
            .and_then(|scene| scene.source.sheet_guid())
    }

    /// Take a drawing out of the view when it is the one shown.
    fn drop_scene(&mut self, sheet: &Arc<DrawScene>) {
        if self.is_current(sheet) {
            self.scene = None;
            self.visible.clear();
            self.shown = false;
            self.invalidate();
        }
    }

    /// Show a drawing: every layer the file has on is shown, and the view
    /// zooms to its extents.
    fn show_scene(&mut self, scene: Arc<DrawScene>) {
        self.visible = scene
            .layers
            .iter()
            .map(|layer| {
                !scene
                    .hidden
                    .iter()
                    .any(|hidden| hidden.eq_ignore_ascii_case(&layer.name))
            })
            .collect();
        self.scene = Some(scene);
        self.fit_pending.set(true);
        self.settled = true;
        self.invalidate();
    }

    /// Show or hide a layer by its place; false when there is no such layer.
    pub(crate) fn set_layer(&mut self, index: usize, on: bool) -> bool {
        match self.visible.get_mut(index) {
            Some(shown) => {
                if *shown != on {
                    *shown = on;
                    self.invalidate();
                }
                true
            }
            None => false,
        }
    }

    pub(crate) fn set_all_layers(&mut self, on: bool) {
        self.visible.iter_mut().for_each(|shown| *shown = on);
        self.invalidate();
    }

    /// Zoom to the extents of the drawing, for a sheet of this size when it
    /// has not been drawn yet.
    pub(crate) fn zoom_extents(&mut self, fallback: Size) {
        if let Some(extents) = self.scene().and_then(|scene| scene.extents) {
            self.camera
                .set(ViewCamera::fit(extents, self.sheet_size(fallback)));
            self.fit_pending.set(false);
            self.settled = true;
        }
    }

    pub(crate) fn layer_shown(&self, index: usize) -> bool {
        self.visible.get(index).copied().unwrap_or(false)
    }

    /// The view as `status` of the local API reports it.
    pub(crate) fn value(&self) -> Value {
        let camera = self.camera();
        let scene = self.scene();
        json!({
            "shown": self.shown,
            "show_after_export": self.show_after_export,
            "reading": self.reading.as_ref().map(|reading| &reading.path),
            "error": self.last_error,
            "drawing": scene.map(|scene| {
                let [points, polylines, fills, texts] = scene.totals();
                json!({
                    "source": scene.source.kind(),
                    "path": scene.source.path(),
                    "units": scene.units.key(),
                    "units_named": scene.units_named,
                    "points": points,
                    "polylines": polylines,
                    "fills": fills,
                    "texts": texts,
                    "extents": scene.extents,
                    "inserts": scene.inserts,
                    "skipped": scene.skipped,
                    "skipped_3d": scene.skipped_3d,
                    "layers": scene.layers.iter().enumerate().map(|(index, layer)| json!({
                        "name": layer.name,
                        "visible": self.layer_shown(index),
                        "color": layer.rgb,
                        "points": layer.points.len(),
                        "polylines": layer.lines.len(),
                        "fills": layer.fills.len(),
                        "texts": layer.texts.len(),
                    })).collect::<Vec<_>>(),
                })
            }),
            "camera": {
                "center": camera.center,
                "pixels_per_unit": camera.scale,
            },
            "viewport_size": self.bounds.get().map(|bounds| [bounds.width, bounds.height]),
        })
    }

    /// The geometry of the sheet, built again when what it shows or its
    /// scale changed, and the map that puts it on the sheet as the camera
    /// is now.
    fn geometry(
        &self,
        renderer: &Renderer,
        scene: &DrawScene,
        size: Size,
    ) -> (canvas::Geometry, Transformation) {
        let camera = self.camera.get();
        let mut built = self.built.borrow_mut();
        let reusable = built
            .as_ref()
            .is_some_and(|built| built.serves(camera, size, self.revision, self.settled));
        if !reusable {
            let next = plan_build(scene, camera, size, self.revision);
            self.cache.clear();
            *built = Some(next);
        }
        let Some(built) = *built else {
            unreachable!("a geometry was just planned");
        };
        let large_dots = scene.totals()[0] <= LARGE_DOTS_UP_TO;
        let geometry = self.cache.draw(renderer, built.size, |frame| {
            draw_scene(frame, scene, &self.visible, &built, large_dots);
        });
        let ([x, y], k) = built.placement(camera, size);
        let transform =
            Transformation::translate(x as f32, y as f32) * Transformation::scale(k as f32);
        (geometry, transform)
    }
}

impl Built {
    /// Whether this geometry can be shown for a camera: it shows what is to
    /// be shown, holds what the sheet shows, and is at the scale of the
    /// camera, or near it while the wheel still turns.
    fn serves(&self, camera: ViewCamera, size: Size, revision: u64, settled: bool) -> bool {
        let ratio = camera.scale / self.camera.scale;
        self.revision == revision
            && (self.whole || contains(self.region, camera.visible(size)))
            && (ratio == 1.0 || (!settled && (1.0 / MAX_SCALED..=MAX_SCALED).contains(&ratio)))
    }

    /// Where the top left corner of the geometry goes on the sheet, and how
    /// much it is scaled, for a camera.
    fn placement(&self, camera: ViewCamera, size: Size) -> ([f64; 2], f64) {
        (
            camera.to_screen(self.origin, size),
            camera.scale / self.camera.scale,
        )
    }

    /// The pixel of the geometry where a drawing point lies.
    #[cfg(test)]
    fn pixel(&self, point: [f64; 2]) -> [f64; 2] {
        [
            (point[0] - self.origin[0]) * self.camera.scale,
            (self.origin[1] - point[1]) * self.camera.scale,
        ]
    }
}

/// The part of the drawing to build at a camera: what the sheet shows and a
/// sheet's width and height around it, or the whole drawing when that is
/// no more.
fn plan_build(scene: &DrawScene, camera: ViewCamera, size: Size, revision: u64) -> Built {
    let view = camera.visible(size);
    let span = [view[1][0] - view[0][0], view[1][1] - view[0][1]];
    let around = [
        [view[0][0] - span[0], view[0][1] - span[1]],
        [view[1][0] + span[0], view[1][1] + span[1]],
    ];
    // A few pixels more, for the dots and lines on the edge.
    let margin = 4.0 / camera.scale;
    let extents = scene.extents.map(|[min, max]| {
        [
            [min[0] - margin, min[1] - margin],
            [max[0] + margin, max[1] + margin],
        ]
    });
    let (region, whole) = match extents {
        Some(extents) if contains(around, extents) => (extents, true),
        Some(extents) if overlaps(around, extents) => (
            [
                [
                    around[0][0].max(extents[0][0]),
                    around[0][1].max(extents[0][1]),
                ],
                [
                    around[1][0].min(extents[1][0]),
                    around[1][1].min(extents[1][1]),
                ],
            ],
            false,
        ),
        _ => (around, false),
    };
    let width = ((region[1][0] - region[0][0]) * camera.scale)
        .ceil()
        .max(1.0);
    let height = ((region[1][1] - region[0][1]) * camera.scale)
        .ceil()
        .max(1.0);
    Built {
        camera,
        origin: [region[0][0], region[1][1]],
        region,
        whole,
        size: Size::new(width as f32, height as f32),
        revision,
    }
}

/// Tessellate the visible layers in the part of the drawing to build: the
/// fills first, then the points, then the lines over them.
fn draw_scene(
    frame: &mut Frame,
    scene: &DrawScene,
    visible: &[bool],
    built: &Built,
    large_dots: bool,
) {
    let scale = built.camera.scale;
    let pixel = |point: [f64; 2]| {
        UiPoint::new(
            ((point[0] - built.origin[0]) * scale) as f32,
            ((built.origin[1] - point[1]) * scale) as f32,
        )
    };
    let shown = || {
        scene
            .layers
            .iter()
            .enumerate()
            .filter(|(index, _)| visible.get(*index).copied().unwrap_or(false))
            .map(|(_, layer)| layer)
    };
    for layer in shown() {
        let color = ink(layer.rgb);
        for fill in &layer.fills {
            let reaches = bounds_of(fill.iter().flatten())
                .is_some_and(|bounds| overlaps(bounds, built.region));
            if !reaches {
                continue;
            }
            let path = canvas::Path::new(|builder| {
                for ring in fill.iter().filter(|ring| ring.len() >= 3) {
                    builder.move_to(pixel(ring[0]));
                    for point in &ring[1..] {
                        builder.line_to(pixel(*point));
                    }
                    builder.close();
                }
            });
            frame.fill(
                &path,
                canvas::Fill {
                    style: canvas::Style::Solid(color),
                    rule: canvas::fill::Rule::EvenOdd,
                },
            );
        }
    }
    let dot = if large_dots { 2.0 } else { 1.5 };
    let dot_size = Size::new(dot, dot);
    let [min, max] = built.region;
    for layer in shown() {
        let color = ink(layer.rgb);
        for point in &layer.points {
            let at = point.at;
            if at[0] < min[0] || at[0] > max[0] || at[1] < min[1] || at[1] > max[1] {
                continue;
            }
            let center = pixel(at);
            frame.fill_rectangle(
                UiPoint::new(center.x - dot / 2.0, center.y - dot / 2.0),
                dot_size,
                point.rgb.map_or(color, ink),
            );
        }
    }
    for layer in shown() {
        let lines: Vec<&SceneLine> = layer
            .lines
            .iter()
            .filter(|line| {
                line.points.len() >= 2
                    && bounds_of(&line.points).is_some_and(|bounds| overlaps(bounds, built.region))
            })
            .collect();
        if lines.is_empty() {
            continue;
        }
        let path = canvas::Path::new(|builder| {
            for line in &lines {
                builder.move_to(pixel(line.points[0]));
                for point in &line.points[1..] {
                    builder.line_to(pixel(*point));
                }
                if line.closed {
                    builder.close();
                }
            }
        });
        frame.stroke(
            &path,
            canvas::Stroke::default()
                .with_color(ink(layer.rgb))
                .with_width(1.0),
        );
    }
}

/// The sheet with the cached geometry of the drawing.
struct Sheet<'a> {
    tool: &'a DrawingViewTool,
    paper: Color,
}

impl Widget<Message, Theme, Renderer> for Sheet<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let tool = self.tool;
        tool.bounds.set(Some(bounds));
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            Background::Color(self.paper),
        );
        let Some(scene) = tool.scene() else {
            return;
        };
        let size = bounds.size();
        if size.width < 1.0 || size.height < 1.0 {
            return;
        }
        if tool.fit_pending.get() {
            if let Some(extents) = scene.extents {
                tool.camera.set(ViewCamera::fit(extents, size));
            }
            tool.fit_pending.set(false);
        }
        let (geometry, transform) = tool.geometry(renderer, scene, size);
        renderer.with_layer(bounds, |renderer| {
            renderer.with_transformation(
                Transformation::translate(bounds.x, bounds.y) * transform,
                |renderer| renderer.draw_geometry(geometry),
            );
        });
    }
}

impl<'a> From<Sheet<'a>> for Element<'a, Message> {
    fn from(sheet: Sheet<'a>) -> Self {
        Element::new(sheet)
    }
}

/// What is drawn over the sheet every frame, and the pointer: the texts,
/// the scale bar and the coordinates under the pointer. A drag pans and the
/// wheel zooms about the pointer.
struct Overlay<'a> {
    tool: &'a DrawingViewTool,
}

/// A step of the wheel as a zoom factor.
fn wheel_factor(delta: mouse::ScrollDelta) -> f32 {
    let steps = match delta {
        mouse::ScrollDelta::Lines { y, .. } => y,
        mouse::ScrollDelta::Pixels { y, .. } => y / 60.0,
    };
    ZOOM_STEP.powf(steps.clamp(-10.0, 10.0))
}

/// A length of 1, 2 or 5 times a power of ten close to `about`.
pub(crate) fn nice_length(about: f64) -> f64 {
    if !(about.is_finite() && about > 0.0) {
        return 1.0;
    }
    let power = 10f64.powf(about.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|step| step * power)
        .min_by(|a, b| (a - about).abs().total_cmp(&(b - about).abs()))
        .unwrap_or(power)
}

/// A length in the units of the drawing, as the scale bar names it.
pub(crate) fn length_label(length: f64, units: DrawingUnits) -> String {
    let digits = if length >= 1.0 {
        0
    } else {
        (-length.log10()).ceil() as usize
    };
    format!("{length:.digits$} {}", units.key())
}

/// A coordinate in the units of the drawing.
fn coordinate(value: f64, units: DrawingUnits) -> String {
    match units {
        DrawingUnits::Millimetres => format!("{value:.1}"),
        DrawingUnits::Metres => format!("{value:.3}"),
    }
}

impl canvas::Program<Message> for Overlay<'_> {
    type State = Option<UiPoint>;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        let action = |action| Some(Message::DrawingView(action));
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left | mouse::Button::Middle | mouse::Button::Right,
            )) => {
                if let Some(position) = cursor.position_in(bounds) {
                    *state = Some(position);
                    return (event::Status::Captured, None);
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left | mouse::Button::Middle | mouse::Button::Right,
            )) => {
                if state.take().is_some() {
                    return (event::Status::Captured, None);
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(previous) = *state {
                    let now = UiPoint::new(position.x - bounds.x, position.y - bounds.y);
                    *state = Some(now);
                    return (
                        event::Status::Captured,
                        action(DrawingViewAction::Pan([
                            now.x - previous.x,
                            now.y - previous.y,
                        ])),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(position) = cursor.position_in(bounds) {
                    return (
                        event::Status::Captured,
                        action(DrawingViewAction::Zoom(
                            wheel_factor(delta),
                            [position.x, position.y],
                            bounds.size(),
                        )),
                    );
                }
            }
            _ => {}
        }
        (event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let tool = self.tool;
        let muted = Color::from_rgb8(113, 113, 122);
        let ink_color = ink([0, 0, 0]);
        let Some(scene) = tool.scene() else {
            let message = if tool.is_reading() {
                tr("Reading the drawing…")
            } else {
                tr("No drawing yet. Preview or export a section drawing, or open a DXF or DWG file from File › Open.")
            };
            frame.fill_text(canvas::Text {
                content: message.to_owned(),
                position: UiPoint::new(size.width / 2.0, size.height / 2.0),
                color: muted,
                size: Pixels(14.0),
                horizontal_alignment: alignment::Horizontal::Center,
                vertical_alignment: alignment::Vertical::Center,
                ..canvas::Text::default()
            });
            return vec![frame.into_geometry()];
        };
        let camera = tool.camera.get();
        let mut drawn = 0;
        'layers: for (index, layer) in scene.layers.iter().enumerate() {
            if !tool.layer_shown(index) {
                continue;
            }
            let color = ink(layer.rgb);
            for label in &layer.texts {
                let pixels = (label.height * camera.scale) as f32;
                if !(MIN_TEXT_PIXELS..=2_000.0).contains(&pixels) {
                    continue;
                }
                let [x, y] = camera.to_screen(label.at, size);
                let reach = f64::from(pixels) * label.value.chars().count() as f64;
                if x < -reach
                    || y < -reach
                    || x > f64::from(size.width) + reach
                    || y > f64::from(size.height) + reach
                {
                    continue;
                }
                if drawn == MAX_TEXTS {
                    break 'layers;
                }
                drawn += 1;
                let content = canvas::Text {
                    content: label.value.clone(),
                    position: UiPoint::ORIGIN,
                    color,
                    // The height of a drawing text is that of its capitals.
                    size: Pixels(pixels * 1.4),
                    horizontal_alignment: alignment::Horizontal::Left,
                    vertical_alignment: alignment::Vertical::Bottom,
                    ..canvas::Text::default()
                };
                frame.with_save(|frame| {
                    frame.translate(Vector::new(x as f32, y as f32));
                    if label.rotation != 0.0 {
                        frame.rotate(-label.rotation as f32);
                    }
                    frame.fill_text(content);
                });
            }
        }

        // The scale bar, at the lower left.
        let length = nice_length(SCALE_BAR_PIXELS / camera.scale);
        let bar = (length * camera.scale) as f32;
        let left = 16.0;
        let base = size.height - 18.0;
        let path = canvas::Path::new(|builder| {
            builder.move_to(UiPoint::new(left, base - 6.0));
            builder.line_to(UiPoint::new(left, base));
            builder.line_to(UiPoint::new(left + bar, base));
            builder.line_to(UiPoint::new(left + bar, base - 6.0));
            builder.move_to(UiPoint::new(left + bar / 2.0, base - 3.0));
            builder.line_to(UiPoint::new(left + bar / 2.0, base));
        });
        frame.stroke(
            &path,
            canvas::Stroke::default()
                .with_color(ink_color)
                .with_width(1.5),
        );
        frame.fill_text(canvas::Text {
            content: length_label(length, scene.units),
            position: UiPoint::new(left + bar + 8.0, base + 1.0),
            color: ink_color,
            size: Pixels(12.0),
            vertical_alignment: alignment::Vertical::Bottom,
            ..canvas::Text::default()
        });

        // The coordinates under the pointer, at the lower right.
        if let Some(position) = cursor.position_in(bounds) {
            let at = camera.to_drawing([position.x, position.y], size);
            frame.fill_text(canvas::Text {
                content: format!(
                    "X {}   Y {}  {}",
                    coordinate(at[0], scene.units),
                    coordinate(at[1], scene.units),
                    scene.units.key()
                ),
                position: UiPoint::new(size.width - 14.0, base + 1.0),
                color: ink_color,
                size: Pixels(12.0),
                horizontal_alignment: alignment::Horizontal::Right,
                vertical_alignment: alignment::Vertical::Bottom,
                ..canvas::Text::default()
            });
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.is_some() {
            mouse::Interaction::Idle
        } else if cursor.is_over(bounds) && self.tool.scene().is_some() {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

/// Everything the Drawing view reacts to.
#[derive(Debug, Clone)]
pub enum DrawingViewAction {
    /// Show the drawing in the main area, or the 3D scene again.
    Show(bool),
    /// The sheet was dragged by some pixels.
    Pan([f32; 2]),
    /// Zoom by a factor about a pixel of a sheet of a size.
    Zoom(f32, [f32; 2], Size),
    /// The wheel has rested since the zoom step of this serial.
    Settle(u64),
    ZoomExtents,
    Layer(usize, bool),
    AllLayers(bool),
    /// Ask which DXF or DWG file to open.
    OpenFile,
    FileChosen(Option<PathBuf>),
    /// A file of this serial was read.
    Read(u64, Result<Arc<DrawScene>, String>),
    ShowAfterExport(bool),
    /// Show the drawing at this place in the project browser.
    ShowSheet(usize),
    /// Take the drawing at this place out of the project browser.
    RemoveSheet(usize),
    /// Show a drawing of Create 2D, made again when it is not made yet.
    ShowDrawing(String),
    /// Forget a drawing of Create 2D.
    DeleteDrawing(String),
}

impl Studio {
    pub(crate) fn update_drawing_view(&mut self, action: DrawingViewAction) -> Task<Message> {
        let view = &mut self.drawing_view;
        match action {
            DrawingViewAction::Show(on) => {
                view.shown = on;
                if on {
                    self.file_open = false;
                }
            }
            DrawingViewAction::Pan(delta) => view.camera.set(view.camera.get().panned(delta)),
            DrawingViewAction::Zoom(factor, pixel, size) => {
                view.camera
                    .set(view.camera.get().zoomed(f64::from(factor), pixel, size));
                view.settled = false;
                view.zoom_serial += 1;
                let serial = view.zoom_serial;
                return Task::perform(async { tokio::time::sleep(ZOOM_SETTLE).await }, move |()| {
                    Message::DrawingView(DrawingViewAction::Settle(serial))
                });
            }
            DrawingViewAction::Settle(serial) => {
                if serial == view.zoom_serial {
                    view.settled = true;
                }
            }
            DrawingViewAction::ZoomExtents => {
                let fallback = self.viewport_size;
                self.drawing_view.zoom_extents(fallback);
            }
            DrawingViewAction::Layer(index, on) => {
                view.set_layer(index, on);
            }
            DrawingViewAction::AllLayers(on) => view.set_all_layers(on),
            DrawingViewAction::OpenFile => {
                self.file_open = false;
                return Task::perform(
                    async {
                        rfd::AsyncFileDialog::new()
                            .set_title(tr("Open a drawing"))
                            .add_filter(tr("Drawings (DXF, DWG)"), &["dxf", "dwg"])
                            .pick_file()
                            .await
                            .map(|file| file.path().to_path_buf())
                    },
                    |path| Message::DrawingView(DrawingViewAction::FileChosen(path)),
                );
            }
            DrawingViewAction::FileChosen(Some(path)) => {
                return self.read_drawing_file(path, None);
            }
            DrawingViewAction::FileChosen(None) => {}
            DrawingViewAction::Read(serial, result) => self.drawing_file_read(serial, result),
            DrawingViewAction::ShowAfterExport(on) => {
                view.show_after_export = on;
                return self.queue_preferences_save();
            }
            DrawingViewAction::ShowSheet(place) => {
                if let Some(sheet) = view.sheets.get(place).cloned() {
                    if !view.is_current(&sheet) {
                        view.show_scene(sheet);
                    }
                    view.shown = true;
                    self.file_open = false;
                }
            }
            DrawingViewAction::RemoveSheet(place) => {
                if place < view.sheets.len() {
                    let sheet = view.sheets.remove(place);
                    view.drop_scene(&sheet);
                }
            }
            DrawingViewAction::ShowDrawing(guid) => match self.show_saved_drawing(&guid, None) {
                Ok(task) => return task.unwrap_or_else(Task::none),
                Err(reason) => self.status = reason,
            },
            DrawingViewAction::DeleteDrawing(guid) => {
                if let Err(reason) = self.delete_saved_drawing(&guid) {
                    self.status = reason;
                }
            }
        }
        Task::none()
    }

    /// Show a drawing of Create 2D: the one made in this session, or, when
    /// it has not been made yet, make it from how it was made. Answers the
    /// job that makes it, or why it cannot be shown.
    pub(crate) fn show_saved_drawing(
        &mut self,
        guid: &str,
        api_job_id: Option<String>,
    ) -> Result<Option<Task<Message>>, String> {
        let definition = self
            .drawing_view
            .saved
            .iter()
            .find(|drawing| drawing.guid == guid)
            .cloned()
            .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
        if let Some(scene) = self.drawing_view.made(guid).cloned() {
            if !self.drawing_view.is_current(&scene) {
                self.drawing_view.show_scene(scene);
            }
            self.drawing_view.shown = true;
            self.file_open = false;
            self.status = format!("Drawing {}", definition.name);
            return Ok(None);
        }
        self.remake_sheet(definition, api_job_id).map(Some)
    }

    /// Forget how a drawing of Create 2D was made, and the drawing.
    pub(crate) fn delete_saved_drawing(&mut self, guid: &str) -> Result<String, String> {
        let view = &mut self.drawing_view;
        let place = view
            .saved
            .iter()
            .position(|drawing| drawing.guid == guid)
            .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
        let removed = view.saved.remove(place);
        if let Err(error) = crate::saved_drawings::save(&view.saved) {
            view.saved.insert(place, removed);
            return Err(format!("The drawings could not be stored: {error}"));
        }
        if let Some(scene) = view.made(guid).cloned() {
            view.made
                .retain(|made| made.source.sheet_guid() != Some(guid));
            view.drop_scene(&scene);
        }
        self.status = format!("Drawing {} deleted", removed.name);
        Ok(removed.name)
    }

    /// Keep how a drawing of Create 2D was made, in place of what was kept
    /// for it before.
    pub(crate) fn keep_saved_drawing(&mut self, definition: SavedDrawing) {
        let saved = &mut self.drawing_view.saved;
        match saved
            .iter_mut()
            .find(|drawing| drawing.guid == definition.guid)
        {
            Some(kept) => *kept = definition,
            None => saved.push(definition),
        }
        if let Err(error) = crate::saved_drawings::save(saved) {
            self.status = format!(
                "{}; it could not be kept for a next session: {error}",
                self.status
            );
        }
    }

    /// Read a DXF or DWG file into the view on a worker thread.
    fn read_drawing_file(&mut self, path: PathBuf, api_job_id: Option<String>) -> Task<Message> {
        let view = &mut self.drawing_view;
        let serial = view.next_read;
        view.next_read += 1;
        if let Some(earlier) = view.reading.take() {
            if let Some(entry) = earlier
                .api_job_id
                .as_ref()
                .and_then(|id| self.api_jobs.get_mut(id))
            {
                *entry = json!({"state": "cancelled", "operation": "open_drawing", "path": earlier.path});
            }
        }
        self.drawing_view.reading = Some(Reading {
            serial,
            path: path.clone(),
            api_job_id,
        });
        self.drawing_view.shown = true;
        self.file_open = false;
        self.status = format!("Reading the drawing {}…", path.display());
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    pointcloud_core::read_drawing(&path)
                        .map(|read| Arc::new(DrawScene::from_file(read, path)))
                        .map_err(|error| plain_reason(&error.to_string()).to_owned())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            move |result| Message::DrawingView(DrawingViewAction::Read(serial, result)),
        )
    }

    fn drawing_file_read(&mut self, serial: u64, result: Result<Arc<DrawScene>, String>) {
        let Some(reading) = self
            .drawing_view
            .reading
            .take_if(|reading| reading.serial == serial)
        else {
            return;
        };
        let name = reading
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let answer = match result {
            Ok(scene) => {
                let [points, polylines, fills, texts] = scene.totals();
                let mut line = format!(
                    "Drawing {name} opened: points {}, polylines {}, fills {}, texts {}, layers {}",
                    format_count(points),
                    format_count(polylines),
                    format_count(fills),
                    format_count(texts),
                    scene.layers.len()
                );
                if scene.skipped_total() > 0 {
                    line += &format!("; {} entities not shown", scene.skipped_total());
                }
                if scene.skipped_3d > 0 {
                    line += &format!("; 3D content not shown ({} entities)", scene.skipped_3d);
                }
                if !scene.units_named {
                    line += "; the file names no units, read as millimetres";
                }
                self.status = line;
                let value = json!({
                    "state": "complete",
                    "operation": "open_drawing",
                    "path": reading.path,
                    "units": scene.units.key(),
                    "units_named": scene.units_named,
                    "layers": scene.layers.len(),
                    "points": points,
                    "polylines": polylines,
                    "fills": fills,
                    "texts": texts,
                    "inserts": scene.inserts,
                    "skipped": scene.skipped,
                    "skipped_3d": scene.skipped_3d,
                });
                self.drawing_view.last_error = None;
                self.drawing_view.set_scene(scene);
                self.drawing_view.shown = true;
                value
            }
            Err(error) => {
                self.status = format!("Could not open the drawing {name}: {error}");
                self.drawing_view.last_error = Some(error.clone());
                json!({
                    "state": "failed",
                    "operation": "open_drawing",
                    "path": reading.path,
                    "error": error,
                })
            }
        };
        if let Some(entry) = reading
            .api_job_id
            .as_ref()
            .and_then(|id| self.api_jobs.get_mut(id))
        {
            *entry = answer;
        }
    }

    /// A drawing of the Section drawing tool arrived: it goes into the view,
    /// and after an export the view is shown when that was asked.
    pub(crate) fn section_drawing_built(&mut self, scene: Arc<DrawScene>, exported: bool) {
        self.drawing_view.set_scene(scene);
        if exported && self.drawing_view.show_after_export {
            self.drawing_view.shown = true;
        }
    }

    /// The `drawing_view` command of the local API.
    pub(crate) fn api_drawing_view(&mut self, show: bool) -> Value {
        if show && self.settings.is_some() {
            return json!({"ok": false, "error": "the Settings dialog is open"});
        }
        let _ = self.update_drawing_view(DrawingViewAction::Show(show));
        json!({"ok": true, "drawing_view": self.drawing_view.value()})
    }

    /// The `open_drawing` command of the local API.
    pub(crate) fn api_open_drawing(&mut self, path: PathBuf) -> (Value, Task<Message>) {
        let is_drawing = pointcloud_core::DrawingFormat::from_path(&path).is_some();
        if !path.is_absolute() || !is_drawing {
            return (
                json!({"ok": false, "error": "open_drawing requires an absolute .dxf or .dwg path"}),
                Task::none(),
            );
        }
        if !path.is_file() {
            return (
                json!({"ok": false, "error": format!("{} does not exist", path.display())}),
                Task::none(),
            );
        }
        let id = self.record_api_job(json!({
            "state": "running",
            "operation": "open_drawing",
            "path": path,
        }));
        let task = self.read_drawing_file(path.clone(), Some(id.clone()));
        (
            json!({"ok": true, "accepted": true, "job_id": id, "path": path}),
            task,
        )
    }

    /// The `drawing_zoom_extents` command of the local API.
    pub(crate) fn api_drawing_zoom_extents(&mut self) -> Value {
        if self.drawing_view.scene().is_none() {
            return json!({"ok": false, "error": "the Drawing view has no drawing"});
        }
        let _ = self.update_drawing_view(DrawingViewAction::ZoomExtents);
        json!({"ok": true, "camera": self.drawing_view.value()["camera"]})
    }

    /// The `set_drawing_layer` command of the local API: a layer by its
    /// name, any case, or `*` for every layer.
    pub(crate) fn api_set_drawing_layer(&mut self, layer: &str, visible: bool) -> Value {
        let Some(scene) = self.drawing_view.scene() else {
            return json!({"ok": false, "error": "the Drawing view has no drawing"});
        };
        if layer == "*" {
            self.drawing_view.set_all_layers(visible);
            return json!({"ok": true, "layers": scene_layer_count(&self.drawing_view), "visible": visible});
        }
        let Some(index) = scene
            .layers
            .iter()
            .position(|known| known.name.eq_ignore_ascii_case(layer))
        else {
            return json!({"ok": false, "error": format!("the drawing has no layer {layer}")});
        };
        let name = scene.layers[index].name.clone();
        self.drawing_view.set_layer(index, visible);
        json!({"ok": true, "layer": name, "visible": visible})
    }

    /// The `list_drawings` command of the local API: the drawings of Create
    /// 2D made from an open scan, with how each was made and whether it is
    /// made in this session, and the previews, exports and files.
    pub(crate) fn api_list_drawings(&self) -> Value {
        let shown = self.drawing_view.shown_guid();
        let drawings: Vec<Value> = self
            .listed_drawings()
            .into_iter()
            .map(|drawing| {
                json!({
                    "name": drawing.name,
                    "guid": drawing.guid,
                    "kind": drawing.kind.key(),
                    "view": drawing.view,
                    "thickness": drawing.thickness,
                    "box": drawing.section,
                    "settings": drawing.request,
                    "sources": drawing.sources,
                    "created": drawing.created,
                    "made": self.drawing_view.made(&drawing.guid).is_some(),
                    "shown": shown == Some(drawing.guid.as_str()),
                })
            })
            .collect();
        let files: Vec<Value> = self
            .drawing_view
            .sheets()
            .iter()
            .map(|sheet| {
                json!({
                    "name": sheet.source.caption(),
                    "source": sheet.source.kind(),
                    "path": sheet.source.path(),
                    "shown": self.drawing_view.shown && self.drawing_view.is_current(sheet),
                })
            })
            .collect();
        json!({"ok": true, "drawings": drawings, "files": files})
    }

    /// A drawing of Create 2D made from an open scan, by its name without
    /// regard to case.
    fn drawing_named(&self, name: &str) -> Option<String> {
        self.listed_drawings()
            .into_iter()
            .find(|drawing| drawing.name.eq_ignore_ascii_case(name.trim()))
            .map(|drawing| drawing.guid.clone())
    }

    /// The `show_drawing` command of the local API.
    pub(crate) fn api_show_drawing(&mut self, name: &str) -> (Value, Task<Message>) {
        let Some(guid) = self.drawing_named(name) else {
            return (
                json!({"ok": false, "error": format!("no drawing {name} of an open scan")}),
                Task::none(),
            );
        };
        if self.drawing_view.made(&guid).is_some() {
            return match self.show_saved_drawing(&guid, None) {
                Ok(_) => (
                    json!({"ok": true, "shown": true, "guid": guid}),
                    Task::none(),
                ),
                Err(error) => (json!({"ok": false, "error": error}), Task::none()),
            };
        }
        let id = self.record_api_job(json!({"state": "running", "operation": "create_drawing"}));
        match self.show_saved_drawing(&guid, Some(id.clone())) {
            Ok(task) => (
                json!({"ok": true, "accepted": true, "job_id": id, "guid": guid}),
                task.unwrap_or_else(Task::none),
            ),
            Err(error) => {
                self.forget_api_job(&id);
                (json!({"ok": false, "error": error}), Task::none())
            }
        }
    }

    /// The `delete_drawing` command of the local API.
    pub(crate) fn api_delete_drawing(&mut self, name: &str) -> Value {
        let Some(guid) = self.drawing_named(name) else {
            return json!({"ok": false, "error": format!("no drawing {name} of an open scan")});
        };
        match self.delete_saved_drawing(&guid) {
            Ok(name) => json!({"ok": true, "name": name}),
            Err(error) => json!({"ok": false, "error": error}),
        }
    }

    /// What the header of the main area says beside the tabs while the
    /// drawing is shown.
    pub(crate) fn drawing_view_caption(&self) -> String {
        match self.drawing_view.scene() {
            Some(scene) => scene.source.caption(),
            None => tr("No drawing").to_owned(),
        }
    }

    /// The sheet with the drawing and what is drawn over it.
    pub(crate) fn drawing_sheet(&self) -> Element<'_, Message> {
        let tool = &self.drawing_view;
        stack![
            Sheet {
                tool,
                paper: paper(self.ui_theme),
            },
            Canvas::new(Overlay { tool }).width(Fill).height(Fill),
        ]
        .width(Fill)
        .height(Fill)
        .into()
    }

    /// The block of the Drawing view in Properties, while the view is shown:
    /// where the drawing came from, what it holds, a switch per layer and
    /// zoom extents.
    pub(crate) fn drawing_view_properties(&self) -> Option<Element<'_, Message>> {
        let tool = &self.drawing_view;
        if !tool.shown {
            return None;
        }
        let colors = self.ui_theme.colors();
        let note =
            |content: String| container(text(content).size(10).color(colors.muted)).padding([4, 8]);
        let mut block = column![opencad_properties::section_header("Drawing view")]
            .spacing(0)
            .width(Fill);
        let open = button(text(tr("Open drawing…")).size(11))
            .on_press(Message::DrawingView(DrawingViewAction::OpenFile))
            .style(flat_tool_style);
        let Some(scene) = tool.scene() else {
            block = block.push(note(
                tr("Preview or export a section drawing, or open a DXF or DWG file.").to_owned(),
            ));
            if let Some(error) = &tool.last_error {
                block = block.push(note(error.clone()));
            }
            return Some(block.push(container(open).padding([3, 8])).into());
        };
        let [points, polylines, fills, texts] = scene.totals();
        block = block
            .push(opencad_properties::property_row(
                "Source",
                scene.source.caption(),
            ))
            .push(opencad_properties::property_row(
                "Units",
                scene.units.key().to_owned(),
            ))
            .push(opencad_properties::property_row(
                "Points",
                format_count(points),
            ))
            .push(opencad_properties::property_row(
                "Polylines",
                format_count(polylines),
            ))
            .push(opencad_properties::property_row(
                "Fills",
                format_count(fills),
            ))
            .push(opencad_properties::property_row(
                "Texts",
                format_count(texts),
            ));
        if !scene.units_named {
            block = block.push(note(
                tr("The file names no units; it is read as millimetres.").to_owned(),
            ));
        }
        if scene.skipped_total() > 0 {
            let kinds: Vec<String> = scene
                .skipped
                .iter()
                .map(|(kind, count)| format!("{kind} {count}"))
                .collect();
            block = block.push(note(tr_args(
                "Not shown: {list}",
                &[("list", &kinds.join(", "))],
            )));
        }
        if scene.skipped_3d > 0 {
            block = block.push(note(tr_args(
                "3D content not shown ({count} entities).",
                &[("count", &scene.skipped_3d)],
            )));
        }
        block = block.push(
            container(
                row![
                    button(text(tr("Zoom extents")).size(11))
                        .on_press(Message::DrawingView(DrawingViewAction::ZoomExtents))
                        .style(flat_tool_style),
                    open,
                ]
                .spacing(3),
            )
            .padding([3, 8]),
        );
        block = block
            .push(opencad_properties::section_header("Layers"))
            .push(
                container(
                    row![
                        button(text(tr("Show all")).size(11))
                            .on_press(Message::DrawingView(DrawingViewAction::AllLayers(true)))
                            .style(flat_tool_style),
                        button(text(tr("Hide all")).size(11))
                            .on_press(Message::DrawingView(DrawingViewAction::AllLayers(false)))
                            .style(flat_tool_style),
                    ]
                    .spacing(3),
                )
                .padding([3, 8]),
            );
        for (index, layer) in scene.layers.iter().enumerate() {
            let swatch = ink(layer.rgb);
            let label = format!("{}  ({})", layer.name, format_count(layer.entities()));
            block = block.push(
                container(
                    row![
                        container(text(""))
                            .width(10)
                            .height(10)
                            .style(move |_| container::Style::default().background(swatch)),
                        checkbox(label, tool.layer_shown(index))
                            .on_toggle(move |on| {
                                Message::DrawingView(DrawingViewAction::Layer(index, on))
                            })
                            .style(muted_checkbox_style)
                            .text_size(11)
                            .size(13),
                    ]
                    .spacing(6)
                    .align_y(iced::Alignment::Center),
                )
                .padding([2, 8]),
            );
        }
        Some(block.into())
    }
}

fn scene_layer_count(tool: &DrawingViewTool) -> usize {
    tool.scene().map_or(0, |scene| scene.layers.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_api::ApiCommand;
    use pointcloud_core::{
        write_drawing, DrawingFormat, DrawingVersion, LAYER_CUT_FILL, LAYER_CUT_OUTLINE,
        LAYER_FRAME, LAYER_INFO, LAYER_POINTS, LAYER_RGB_CONTRAST,
    };

    fn sample(units: DrawingUnits) -> Drawing2d {
        let mut drawing = Drawing2d::new(units);
        let points = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
        for index in 0..10 {
            drawing.add_point(points, [index as f64 * 0.5, 1.0], None);
        }
        drawing.add_point(points, [2.0, 2.0], Some([200, 30, 40]));
        drawing
            .add_cut_region(
                vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                vec![vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]]],
            )
            .unwrap();
        drawing.add_frame([-0.5, -1.5], [4.5, 3.5]).unwrap();
        drawing.add_info([-0.5, -1.75], 0.1, "Plan").unwrap();
        drawing
    }

    fn per_layer(scene: &DrawScene) -> Vec<(String, [usize; 4])> {
        scene
            .layers
            .iter()
            .map(|layer| {
                (
                    layer.name.clone(),
                    [
                        layer.points.len(),
                        layer.lines.len(),
                        layer.fills.len(),
                        layer.texts.len(),
                    ],
                )
            })
            .collect()
    }

    #[test]
    fn a_drawing_model_becomes_a_scene_in_drawing_units() {
        let scene =
            DrawScene::from_drawing(&sample(DrawingUnits::Millimetres), DrawingSource::Preview);
        assert_eq!(
            per_layer(&scene),
            [
                (LAYER_POINTS.to_owned(), [11, 0, 0, 0]),
                (LAYER_CUT_FILL.to_owned(), [0, 0, 1, 0]),
                (LAYER_CUT_OUTLINE.to_owned(), [0, 2, 0, 0]),
                (LAYER_FRAME.to_owned(), [0, 1, 0, 0]),
                (LAYER_INFO.to_owned(), [0, 0, 0, 1]),
            ]
        );
        assert_eq!(scene.totals(), [11, 3, 1, 1]);
        // Millimetres: a thousand drawing units per metre.
        assert_eq!(scene.extents, Some([[-500.0, -1750.0], [4500.0, 3500.0]]));
        assert_eq!(scene.layers[1].fills[0].len(), 2, "the fill keeps its hole");
        assert_eq!(scene.layers[4].texts[0].height, 100.0);
        assert_eq!(scene.layers[0].points[10].rgb, Some([200, 30, 40]));

        let metres = DrawScene::from_drawing(&sample(DrawingUnits::Metres), DrawingSource::Preview);
        assert_eq!(metres.extents, Some([[-0.5, -1.75], [4.5, 3.5]]));
        assert_eq!(metres.totals(), scene.totals());
    }

    #[test]
    fn a_dxf_and_a_dwg_of_the_own_writer_read_back_into_the_same_scene() {
        let directory = tempfile::tempdir().unwrap();
        let drawing = sample(DrawingUnits::Millimetres);
        let model = DrawScene::from_drawing(&drawing, DrawingSource::Preview);
        for format in DrawingFormat::ALL {
            let path = directory
                .path()
                .join(format!("plan.{}", format.extension()));
            write_drawing(&drawing, &path, format, DrawingVersion::default()).unwrap();
            let read = pointcloud_core::read_drawing(&path).unwrap();
            let scene = DrawScene::from_file(read, path.clone());
            assert_eq!(scene.source, DrawingSource::File(path));
            assert_eq!(per_layer(&scene), per_layer(&model), "{format}");
            assert_eq!(scene.units, DrawingUnits::Millimetres);
            let [min, max] = scene.extents.unwrap();
            let [model_min, model_max] = model.extents.unwrap();
            for axis in 0..2 {
                assert!((min[axis] - model_min[axis]).abs() < 1e-6);
                assert!((max[axis] - model_max[axis]).abs() < 1e-6);
            }
            assert!(scene.skipped.is_empty());
        }
    }

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
    }

    #[test]
    fn zoom_keeps_the_point_under_the_pointer_and_pan_follows_the_drag() {
        let size = Size::new(800.0, 600.0);
        let camera = ViewCamera {
            center: [1000.0, 2000.0],
            scale: 0.5,
        };
        // The middle of the sheet is the centre; up on the sheet is up in
        // the drawing.
        assert_eq!(camera.to_screen([1000.0, 2000.0], size), [400.0, 300.0]);
        assert_eq!(camera.to_screen([1100.0, 2100.0], size), [450.0, 250.0]);
        assert!(near(
            camera.to_drawing([450.0, 250.0], size),
            [1100.0, 2100.0]
        ));
        for pixel in [[0.0, 0.0], [123.0, 456.0], [800.0, 600.0]] {
            let under = camera.to_drawing(pixel, size);
            for factor in [1.25, 0.8, 10.0] {
                let zoomed = camera.zoomed(factor, pixel, size);
                assert!((zoomed.scale - 0.5 * factor).abs() < 1e-12);
                assert!(near(zoomed.to_drawing(pixel, size), under));
                let back = zoomed.to_screen(under, size);
                assert!((back[0] - f64::from(pixel[0])).abs() < 1e-6);
                assert!((back[1] - f64::from(pixel[1])).abs() < 1e-6);
            }
        }
        // A drag of 20 pixels right and 10 down moves the drawing with it.
        let panned = camera.panned([20.0, 10.0]);
        assert_eq!(panned.to_screen([1000.0, 2000.0], size), [420.0, 310.0]);
        // Zoom extents puts the middle of the drawing in the middle, with
        // the longer side filling the sheet less the margin.
        let fit = ViewCamera::fit([[0.0, 0.0], [4000.0, 1000.0]], size);
        assert!((fit.scale - 800.0 * 0.9 / 4000.0).abs() < 1e-12);
        // The middle of the drawing lies in the middle of the sheet above
        // the strip with the scale bar.
        let middle = fit.to_screen([2000.0, 500.0], size);
        assert!((middle[0] - 400.0).abs() < 1e-9);
        assert!((middle[1] - f64::from(600.0 - BOTTOM_STRIP) / 2.0).abs() < 1e-9);
        // A drawing taller than wide fills the height above the strip.
        let tall = ViewCamera::fit([[0.0, 0.0], [100.0, 1000.0]], size);
        assert!((tall.scale - f64::from(600.0 - BOTTOM_STRIP) * 0.9 / 1000.0).abs() < 1e-12);
        let visible = fit.visible(size);
        assert!(visible[0][0] < 0.0 && visible[1][0] > 4000.0);
        // A single point zooms to something finite.
        let point = ViewCamera::fit([[5.0, 5.0], [5.0, 5.0]], size);
        assert_eq!(point.center[0], 5.0);
        assert!(point.scale.is_finite());
    }

    #[test]
    fn scale_bar_lengths_are_round_and_labelled_in_drawing_units() {
        assert_eq!(nice_length(120.0), 100.0);
        assert_eq!(nice_length(180.0), 200.0);
        assert_eq!(nice_length(4.0), 5.0);
        assert_eq!(nice_length(0.03), 0.02);
        assert_eq!(length_label(500.0, DrawingUnits::Millimetres), "500 mm");
        assert_eq!(length_label(0.5, DrawingUnits::Metres), "0.5 m");
        assert_eq!(length_label(0.02, DrawingUnits::Metres), "0.02 m");
        assert_eq!(ink([255, 255, 255]), ink([0, 0, 0]));
        assert_eq!(ink([128, 128, 128]), Color::from_rgb8(128, 128, 128));
        // Yellow is darkened to read on the sheet.
        assert_ne!(ink([255, 255, 0]), Color::from_rgb8(255, 255, 0));
    }

    #[test]
    fn geometry_is_planned_for_the_whole_drawing_or_the_part_around_the_view() {
        let scene =
            DrawScene::from_drawing(&sample(DrawingUnits::Millimetres), DrawingSource::Preview);
        let size = Size::new(800.0, 600.0);
        let fit = ViewCamera::fit(scene.extents.unwrap(), size);
        let whole = plan_build(&scene, fit, size, 0);
        assert!(whole.whole);
        assert!(contains(whole.region, scene.extents.unwrap()));
        // Deep in, only what lies around the sheet is built.
        let close = ViewCamera {
            center: [2000.0, 1000.0],
            scale: 50.0,
        };
        let part = plan_build(&scene, close, size, 0);
        assert!(!part.whole);
        assert!(contains(part.region, close.visible(size)));
        assert!(part.size.width <= 3.0 * 800.0 + 1.0);
        assert_eq!(part.origin, [part.region[0][0], part.region[1][1]]);

        // Placed for any camera, a point of the geometry lands where that
        // camera shows the point: panned, and scaled while the wheel turns.
        for camera in [
            close,
            close.panned([120.0, -40.0]),
            close.zoomed(1.5, [10.0, 20.0], size),
        ] {
            let ([x, y], k) = part.placement(camera, size);
            for point in [[2000.0, 1000.0], [1990.0, 1007.5], [2003.0, 996.0]] {
                let [gx, gy] = part.pixel(point);
                let expected = camera.to_screen(point, size);
                assert!((x + k * gx - expected[0]).abs() < 1e-6);
                assert!((y + k * gy - expected[1]).abs() < 1e-6);
            }
        }
        // It serves a pan within what it holds, not one past it, and a
        // zoom only until the wheel rests.
        assert!(part.serves(close, size, 0, true));
        assert!(part.serves(close.panned([200.0, 100.0]), size, 0, true));
        assert!(!part.serves(close.panned([2000.0, 0.0]), size, 0, true));
        let zoomed = close.zoomed(1.25, [400.0, 300.0], size);
        assert!(part.serves(zoomed, size, 0, false));
        assert!(!part.serves(zoomed, size, 0, true));
        assert!(!part.serves(close, size, 1, true), "a layer was switched");
        // The whole drawing serves every pan at its scale.
        assert!(whole.serves(fit.panned([5000.0, 5000.0]), size, 0, true));
    }

    #[test]
    fn a_drag_pans_and_the_wheel_zooms_about_the_pointer() {
        let tool = DrawingViewTool::default();
        let overlay = Overlay { tool: &tool };
        let bounds = Rectangle::new(UiPoint::new(100.0, 50.0), Size::new(800.0, 600.0));
        let at = |x: f32, y: f32| mouse::Cursor::Available(UiPoint::new(x, y));
        let mut state = None;
        let (status, message) = canvas::Program::update(
            &overlay,
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            at(300.0, 250.0),
        );
        assert_eq!(status, event::Status::Captured);
        assert!(message.is_none());
        let (_, message) = canvas::Program::update(
            &overlay,
            &mut state,
            canvas::Event::Mouse(mouse::Event::CursorMoved {
                position: UiPoint::new(330.0, 240.0),
            }),
            bounds,
            at(330.0, 240.0),
        );
        assert!(matches!(
            message,
            Some(Message::DrawingView(DrawingViewAction::Pan([30.0, -10.0])))
        ));
        let _ = canvas::Program::update(
            &overlay,
            &mut state,
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            bounds,
            at(330.0, 240.0),
        );
        assert!(state.is_none());
        // Moving without a button pressed pans nothing.
        let (_, message) = canvas::Program::update(
            &overlay,
            &mut state,
            canvas::Event::Mouse(mouse::Event::CursorMoved {
                position: UiPoint::new(340.0, 240.0),
            }),
            bounds,
            at(340.0, 240.0),
        );
        assert!(message.is_none());
        let (_, message) = canvas::Program::update(
            &overlay,
            &mut state,
            canvas::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            }),
            bounds,
            at(340.0, 240.0),
        );
        let Some(Message::DrawingView(DrawingViewAction::Zoom(factor, pixel, size))) = message
        else {
            panic!("the wheel zooms");
        };
        assert_eq!(factor, ZOOM_STEP);
        // The pixel is that of the sheet, from its top left corner.
        assert_eq!(pixel, [240.0, 190.0]);
        assert_eq!(size, bounds.size());
        assert!(wheel_factor(mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 }) < 1.0);
        // Outside the sheet the wheel is left alone.
        let (status, _) = canvas::Program::update(
            &overlay,
            &mut state,
            canvas::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            }),
            bounds,
            at(10.0, 10.0),
        );
        assert_eq!(status, event::Status::Ignored);
    }

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(crate::native_api::ApiRequest { command, reply });
        receive.try_recv().unwrap()
    }

    #[test]
    fn layers_switch_and_the_api_shows_zooms_and_reports_the_view() {
        let mut studio = Studio::default();
        // Without a drawing the view shows, but has nothing to zoom to.
        let shown = send(&mut studio, ApiCommand::DrawingView { show: true });
        assert_eq!(shown["ok"], true);
        assert_eq!(shown["drawing_view"]["shown"], true);
        assert_eq!(shown["drawing_view"]["drawing"], Value::Null);
        assert_eq!(
            send(&mut studio, ApiCommand::DrawingZoomExtents)["ok"],
            false
        );
        assert_eq!(
            send(
                &mut studio,
                ApiCommand::SetDrawingLayer {
                    layer: LAYER_FRAME.into(),
                    visible: false
                }
            )["ok"],
            false
        );

        let scene =
            DrawScene::from_drawing(&sample(DrawingUnits::Millimetres), DrawingSource::Preview);
        studio.section_drawing_built(Arc::new(scene), false);
        let hidden = send(
            &mut studio,
            ApiCommand::SetDrawingLayer {
                layer: "ops-frame".into(),
                visible: false,
            },
        );
        assert_eq!(
            hidden,
            json!({"ok": true, "layer": LAYER_FRAME, "visible": false})
        );
        let missing = send(
            &mut studio,
            ApiCommand::SetDrawingLayer {
                layer: "WALLS".into(),
                visible: false,
            },
        );
        assert_eq!(missing["error"], "the drawing has no layer WALLS");
        let status = send(&mut studio, ApiCommand::Status);
        let view = &status["result"]["drawing_view"];
        assert_eq!(view["drawing"]["source"], "preview");
        assert_eq!(view["drawing"]["units"], "mm");
        assert_eq!(view["drawing"]["points"], 11);
        let layers = view["drawing"]["layers"].as_array().unwrap();
        assert_eq!(layers.len(), 5);
        assert_eq!(layers[3]["name"], LAYER_FRAME);
        assert_eq!(layers[3]["visible"], false);
        assert_eq!(layers[0]["visible"], true);

        let _ = studio.update(Message::DrawingView(DrawingViewAction::Layer(3, true)));
        assert!(studio.drawing_view.layer_shown(3));
        let _ = studio.update(Message::DrawingView(DrawingViewAction::AllLayers(false)));
        assert!((0..5).all(|index| !studio.drawing_view.layer_shown(index)));
        assert_eq!(
            send(
                &mut studio,
                ApiCommand::SetDrawingLayer {
                    layer: "*".into(),
                    visible: true
                }
            )["ok"],
            true
        );
        assert!((0..5).all(|index| studio.drawing_view.layer_shown(index)));

        let zoomed = send(&mut studio, ApiCommand::DrawingZoomExtents);
        assert_eq!(zoomed["ok"], true);
        assert_eq!(zoomed["camera"]["center"][0], 2000.0);

        // Panning and zooming change the camera; zoom extents brings it back.
        let before = studio.drawing_view.camera();
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Pan([30.0, -12.0])));
        let size = Size::new(400.0, 300.0);
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Zoom(
            2.0,
            [100.0, 50.0],
            size,
        )));
        assert!(!studio.drawing_view.settled);
        let serial = studio.drawing_view.zoom_serial;
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Settle(serial)));
        assert!(studio.drawing_view.settled);
        assert_ne!(studio.drawing_view.camera(), before);
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ZoomExtents));
        assert_eq!(studio.drawing_view.camera(), before);

        let back = send(&mut studio, ApiCommand::DrawingView { show: false });
        assert_eq!(back["drawing_view"]["shown"], false);
    }

    #[test]
    fn open_drawing_reads_a_file_as_a_job_and_refuses_what_is_no_drawing() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        for (path, error) in [
            (
                PathBuf::from("plan.dxf"),
                "open_drawing requires an absolute .dxf or .dwg path",
            ),
            (
                directory.path().join("plan.pdf"),
                "open_drawing requires an absolute .dxf or .dwg path",
            ),
        ] {
            let answer = send(&mut studio, ApiCommand::OpenDrawing { path });
            assert_eq!(answer["error"], error);
        }
        let missing = directory.path().join("missing.dwg");
        assert!(
            send(&mut studio, ApiCommand::OpenDrawing { path: missing })["error"]
                .as_str()
                .unwrap()
                .ends_with("does not exist")
        );

        let path = directory.path().join("plan.dwg");
        write_drawing(
            &sample(DrawingUnits::Metres),
            &path,
            DrawingFormat::Dwg,
            DrawingVersion::default(),
        )
        .unwrap();
        let answer = send(&mut studio, ApiCommand::OpenDrawing { path: path.clone() });
        assert_eq!(answer["ok"], true);
        let id = answer["job_id"].as_str().unwrap().to_owned();
        assert!(studio.drawing_view.is_reading());
        assert_eq!(
            crate::mcp::busy(&send(&mut studio, ApiCommand::Status)["result"]),
            ["drawing_view"]
        );
        // The worker's answer, as it arrives.
        let serial = studio.drawing_view.reading.as_ref().unwrap().serial;
        let read = pointcloud_core::read_drawing(&path).unwrap();
        let scene = Arc::new(DrawScene::from_file(read, path.clone()));
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Read(
            serial,
            Ok(scene),
        )));
        assert!(!studio.drawing_view.is_reading());
        assert!(studio.drawing_view.shown);
        let job = send(&mut studio, ApiCommand::Job { id });
        assert_eq!(job["job"]["state"], "complete", "{job}");
        assert_eq!(job["job"]["points"], 11);
        assert_eq!(job["job"]["units"], "m");
        assert!(studio
            .status
            .starts_with("Drawing plan.dwg opened: points 11, "));

        // A late answer of an earlier read changes nothing.
        let _ = studio.update(Message::DrawingView(DrawingViewAction::Read(
            serial,
            Err("late".into()),
        )));
        assert_eq!(studio.drawing_view.last_error, None);
    }

    #[test]
    fn the_window_builds_with_the_drawing_view_in_english_and_dutch() {
        for language in [
            crate::i18n::Language::English,
            crate::i18n::Language::Table(0),
        ] {
            let _held = crate::i18n::TestLanguage::hold(language);
            let mut studio = Studio::default();
            let _ = studio.update(Message::DrawingView(DrawingViewAction::Show(true)));
            let _ = studio.view();
            let scene = DrawScene::from_drawing(
                &sample(DrawingUnits::Millimetres),
                DrawingSource::Export(PathBuf::from("/drawings/plan.dxf")),
            );
            studio.section_drawing_built(Arc::new(scene), true);
            assert!(studio.drawing_view.shown);
            assert_eq!(studio.drawing_view_caption(), "plan.dxf");
            let _ = studio.view();
        }
    }
}
