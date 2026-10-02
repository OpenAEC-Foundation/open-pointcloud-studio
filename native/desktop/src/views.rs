//! Saved views in the desktop: saving, restoring and listing them, the note
//! and line annotations of the active view with their overlay, the snapshot
//! of each view, the BCF export and the command API.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use iced::widget::canvas::{self, Frame};
use iced::widget::{button, column, container, row, text, text_input};
use iced::window::Screenshot;
use iced::{Color, Element, Fill, Point as UiPoint, Rectangle, Size, Task};
use pointcloud_core::{Bounds, IndexedPoint};
use serde_json::{json, Value};

use crate::camera_views::{
    self, Annotation, SavedView, SectionBox, ViewFrame, WalkCamera, MAX_ANNOTATIONS,
    MAX_NAME_CHARS, MAX_NOTE_CHARS, MAX_VIEWS_PER_SOURCE,
};
use crate::native_api::ApiCommand;
use crate::selection::Projection;
use crate::station_photos::WalkView;
use crate::{
    bcf, combined_bounds, i18n, measure, opencad_properties, opencad_ribbon, ColorMode, Message,
    PointViewport, Studio, ToolIcon,
};

type Xyz = [f64; 3];

/// A snapshot is taken this long after its view changed, so the change has
/// been drawn, and again after that while the points are still refining.
const SNAPSHOT_DELAY: Duration = Duration::from_millis(450);
const SNAPSHOT_TRIES: u8 = 20;
/// Longest edge of a stored snapshot in pixels.
const SNAPSHOT_MAX_EDGE: u32 = 1920;
/// Characters of a note shown in its label before it is cut.
const LABEL_CHARS: usize = 48;
const LABEL_HEIGHT: f32 = 18.0;
/// A label moves this far, at most this many times, past labels in its way.
const LABEL_STEP: f32 = LABEL_HEIGHT + 2.0;
const LABEL_STEPS: usize = 12;
/// Half the side of the square a note's marker takes.
const MARKER_REACH: f32 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationKind {
    Note,
    Line,
}

impl AnnotationKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Line => "line",
        }
    }
}

/// What the viewport shows of a view: the camera, the section box and the
/// colour mode, in a scene with these bounds and a viewport of this size.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    guid: String,
    orbit: (f32, f32, f32, [f32; 2]),
    walk: Option<WalkView>,
    section: Option<Bounds>,
    color_mode: ColorMode,
    scene: Option<Bounds>,
    viewport: Size,
}

impl Shown {
    /// The same in a viewport of the size of the other.
    fn sized_as(&self, other: &Self) -> Self {
        Self {
            viewport: other.viewport,
            ..self.clone()
        }
    }
}

/// The saved views and what the view tools are doing.
#[derive(Debug, Default)]
pub struct ViewTool {
    /// Views of every source scan, in the order they were saved.
    pub list: Vec<SavedView>,
    /// Text of the name field in Properties.
    pub name: String,
    /// The view last saved or restored, with the scan it was active on. Its
    /// annotations are shown while that scan is the active one.
    active: Option<(String, PathBuf)>,
    /// What the viewport showed when the active view was saved, updated or
    /// restored. While it shows the same, a snapshot of it is one of the view.
    shown: Option<Shown>,
    /// Annotation tool that viewport clicks feed, if one is active.
    pub tool: Option<AnnotationKind>,
    /// The scan that was active when the tool was chosen or last used.
    tool_scan: Option<PathBuf>,
    /// First point of a line that waits for its second point.
    line_start: Option<Xyz>,
    /// Point of a note that waits for its text.
    note_point: Option<Xyz>,
    note_text: String,
    /// The view being renamed and the text of its name field.
    renaming: Option<(String, String)>,
    /// Views that wait for a snapshot, with the serial of the newest request.
    snapshots: HashMap<String, u64>,
    snapshot_serial: u64,
    export_pending: bool,
    /// Where the scene canvas lies in the window, as it was last drawn.
    canvas: Cell<Option<Rectangle>>,
    /// The scan path last asked for, with its name in the list of views.
    source: RefCell<Option<(PathBuf, PathBuf)>>,
}

impl ViewTool {
    pub fn load() -> Self {
        Self {
            list: camera_views::load(),
            ..Self::default()
        }
    }

    /// The name a scan carries in the list of views. Finding it asks the
    /// file system, so the answer for the last path is kept.
    pub fn source_of(&self, path: &Path) -> PathBuf {
        let mut kept = self.source.borrow_mut();
        match kept.as_ref() {
            Some((known, source)) if known == path => source.clone(),
            _ => {
                let source = camera_views::source_key(path);
                *kept = Some((path.to_path_buf(), source.clone()));
                source
            }
        }
    }

    /// Leave the annotation tool and report whether one was active. A
    /// half-placed annotation is dropped.
    pub fn leave_tool(&mut self) -> bool {
        self.drop_placing();
        self.tool_scan = None;
        self.tool.take().is_some()
    }

    fn drop_placing(&mut self) -> bool {
        self.note_text.clear();
        self.line_start.take().is_some() | self.note_point.take().is_some()
    }

    /// Stop what Escape stops first: a rename, or a half-placed annotation.
    /// Returns the status line for it, or `None` when there was neither.
    pub fn cancel_input(&mut self) -> Option<&'static str> {
        if self.renaming.take().is_some() {
            Some("Rename cancelled")
        } else if self.drop_placing() {
            Some("Annotation cancelled; click a point to place another")
        } else {
            None
        }
    }
}

/// Everything the view tools react to. Views are named by their identifier.
#[derive(Debug, Clone)]
pub enum ViewAction {
    Name(String),
    Save,
    Restore(String),
    /// Overwrite a view with what the viewport shows now.
    Update(String),
    Delete(String),
    StartRename(String),
    RenameText(String),
    FinishRename,
    CancelRename,
    /// Stop showing the annotations of the active view.
    Deactivate,
    /// Enter an annotation tool, or leave it when it is already active.
    Tool(AnnotationKind),
    /// Left click without a drag at a viewport position.
    Click([f32; 2], Size),
    /// Answer of the pick search started by a click, with its view revision.
    Picked(u64, Result<Option<IndexedPoint>, String>),
    NoteText(String),
    NoteSubmit,
    CancelPlacing,
    /// Remove an annotation of the active view by its place in the list.
    DeleteAnnotation(usize),
    /// Time to take the snapshot of a view, when the viewport still shows it.
    Capture {
        guid: String,
        serial: u64,
        tries: u8,
    },
    Captured(String, u64, Option<Screenshot>),
    SnapshotSaved(String, u64, Result<(), String>),
    ExportBcf,
    ExportPathChosen(Option<PathBuf>),
    Exported(Result<(PathBuf, Exported), String>),
}

impl ViewAction {
    pub fn icon(&self) -> ToolIcon {
        match self {
            Self::Save => ToolIcon::Save,
            Self::Tool(AnnotationKind::Note) => ToolIcon::Note,
            Self::Tool(AnnotationKind::Line) => ToolIcon::Line,
            Self::ExportBcf => ToolIcon::Export,
            _ => ToolIcon::Cloud,
        }
    }
}

fn note_input_id() -> text_input::Id {
    text_input::Id::new("ops-note-text")
}

fn rename_input_id() -> text_input::Id {
    text_input::Id::new("ops-view-rename")
}

/// Hand the answer of a pick search to the annotation tool instead of the
/// point selection.
fn picked_message(message: Message) -> Message {
    match message {
        Message::PickReady(revision, _, result) => {
            Message::Views(ViewAction::Picked(revision, result))
        }
        other => other,
    }
}

fn finite(xyz: &Xyz) -> bool {
    xyz.iter().all(|value| value.is_finite())
}

/// A name without surrounding space, when it is one a view can carry.
fn checked_name(name: &str) -> Option<&str> {
    let name = name.trim();
    (!name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.chars().any(char::is_control))
    .then_some(name)
}

/// The text of a note as it is kept: one line, without control characters.
fn checked_note(text: &str) -> Option<String> {
    let text: String = text
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    (!text.is_empty() && text.chars().count() <= MAX_NOTE_CHARS).then_some(text)
}

/// A text cut to a number of characters, with an ellipsis when it was cut.
fn shortened(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_owned()
    } else {
        let mut cut: String = text.chars().take(max_chars.saturating_sub(1)).collect();
        cut.push('…');
        cut
    }
}

/// Who exports: the name of the user on this computer.
fn author() -> String {
    ["USERNAME", "USER"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Open Pointcloud Studio".into())
}

/// The part of a window screenshot that shows the scene canvas, as a PNG
/// image no larger than the snapshot limit. The screenshot is RGBA in
/// physical pixels; the canvas bounds are in logical pixels.
pub fn encode_snapshot(
    rgba: &[u8],
    width: u32,
    height: u32,
    scale_factor: f64,
    canvas: Rectangle,
) -> Result<Vec<u8>, String> {
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("the screenshot does not match its size".into());
    }
    let physical = |value: f32, limit: u32| -> u32 {
        (f64::from(value) * scale_factor)
            .round()
            .clamp(0.0, f64::from(limit)) as u32
    };
    let (left, top) = (physical(canvas.x, width), physical(canvas.y, height));
    let right = physical(canvas.x + canvas.width, width);
    let bottom = physical(canvas.y + canvas.height, height);
    if right <= left || bottom <= top {
        return Err("the scene is not inside the window".into());
    }
    let (crop_width, crop_height) = (right - left, bottom - top);
    let mut pixels = Vec::with_capacity(crop_width as usize * crop_height as usize * 3);
    for y in top..bottom {
        let start = (y as usize * width as usize + left as usize) * 4;
        for pixel in rgba[start..start + crop_width as usize * 4]
            .as_chunks::<4>()
            .0
        {
            pixels.extend_from_slice(&pixel[..3]);
        }
    }
    let mut image = ::image::RgbImage::from_raw(crop_width, crop_height, pixels)
        .ok_or_else(|| "the screenshot could not be cropped".to_owned())?;
    let longest = crop_width.max(crop_height);
    if longest > SNAPSHOT_MAX_EDGE {
        let shrink = |edge: u32| {
            ((u64::from(edge) * u64::from(SNAPSHOT_MAX_EDGE)) / u64::from(longest)).max(1) as u32
        };
        image = ::image::imageops::resize(
            &image,
            shrink(crop_width),
            shrink(crop_height),
            ::image::imageops::FilterType::Triangle,
        );
    }
    let mut png = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut png),
            ::image::ImageFormat::Png,
        )
        .map_err(|error| error.to_string())?;
    Ok(png)
}

/// What an export wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exported {
    views: usize,
    /// Views written with a snapshot.
    snapshots: usize,
    /// Views whose snapshot is older than the view, or missing, until the
    /// view is restored.
    due: usize,
}

impl Exported {
    fn status(&self, path: &Path) -> String {
        let mut status = format!(
            "Exported {} view(s), {} with a snapshot, to {}",
            self.views,
            self.snapshots,
            path.display()
        );
        if self.due > 0 {
            status.push_str(&format!(
                "; {} view(s) changed after their snapshot: restore them to renew it",
                self.due
            ));
        }
        status
    }
}

/// Write views as one BCF file and report what it holds. Views saved without
/// their own scene bounds and viewport size are taken to be relative to the
/// given ones.
fn write_bcf(
    path: &Path,
    views: &[SavedView],
    scene: Bounds,
    viewport: Size,
) -> Result<Exported, String> {
    let author = author();
    let now = camera_views::now_seconds();
    let context = bcf::Context {
        author: &author,
        now,
        scene,
        viewport,
    };
    let topics: Vec<bcf::Topic> = views
        .iter()
        .map(|view| bcf::Topic::from_view(view, &context, camera_views::read_snapshot(&view.guid)))
        .collect();
    let snapshots = topics
        .iter()
        .filter(|topic| topic.snapshot.is_some())
        .count();
    let bytes = bcf::archive(&topics, now).map_err(|error| error.to_string())?;
    bcf::write_file(path, &bytes).map_err(|error| error.to_string())?;
    Ok(Exported {
        views: topics.len(),
        snapshots,
        due: views.iter().filter(|view| view.snapshot_due).count(),
    })
}

fn is_bcf_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("bcf"))
}

impl Studio {
    /// Place in the list of the view whose annotations are shown: the view
    /// last saved or restored, while its scan is the active one.
    pub fn active_view_index(&self) -> Option<usize> {
        let (guid, path) = self.views.active.as_ref()?;
        let entry = self.active.and_then(|index| self.clouds.get(index))?;
        if entry.cloud.path != *path {
            return None;
        }
        self.views.list.iter().position(|view| view.guid == *guid)
    }

    fn active_scan_path(&self) -> Option<&Path> {
        self.active
            .and_then(|index| self.clouds.get(index))
            .map(|entry| entry.cloud.path.as_path())
    }

    /// Keep the annotation tool with the scan it was chosen for: a
    /// half-placed annotation is dropped when another scan becomes the active
    /// one, and the tool is left when no scan is open any more.
    pub fn settle_views(&mut self) {
        if self.views.tool.is_none() {
            return;
        }
        let active = self.active_scan_path();
        if active == self.views.tool_scan.as_deref() {
            return;
        }
        match active.map(Path::to_path_buf) {
            Some(path) => {
                self.views.tool_scan = Some(path);
                if self.views.drop_placing() {
                    self.status =
                        "Annotation cancelled because another scan became the active one".into();
                }
            }
            None => {
                self.views.leave_tool();
            }
        }
    }

    fn active_view(&self) -> Option<&SavedView> {
        self.active_view_index()
            .map(|index| &self.views.list[index])
    }

    fn activate_view(&mut self, guid: &str) {
        let path = self
            .active
            .and_then(|index| self.clouds.get(index))
            .map(|entry| entry.cloud.path.clone());
        self.views.active = path.map(|path| (guid.to_owned(), path));
    }

    /// Place in the list of a view of the active scan, by its identifier.
    fn view_index(&self, guid: &str) -> Option<usize> {
        let source = self.active_camera_source()?;
        self.views
            .list
            .iter()
            .position(|view| view.guid == guid && view.source == source)
    }

    /// Place in the list of a view of the active scan, by its name without
    /// regard to case.
    fn view_named(&self, name: &str) -> Option<usize> {
        let source = self.active_camera_source()?;
        self.views
            .list
            .iter()
            .position(|view| view.source == source && view.name.eq_ignore_ascii_case(name.trim()))
    }

    /// Size of the viewport as it was last drawn, which is what a view
    /// shows; before anything has been drawn, the size last reported.
    fn drawn_viewport(&self) -> Size {
        self.views
            .canvas
            .get()
            .map(|bounds| bounds.size())
            .filter(|size| size.width >= 1.0 && size.height >= 1.0)
            .unwrap_or(self.viewport_size)
    }

    /// What the viewport shows now, taken as the picture of a view.
    fn showing(&self, guid: &str) -> Shown {
        Shown {
            guid: guid.to_owned(),
            orbit: (self.yaw, self.pitch, self.zoom, self.pan),
            walk: self.walk,
            section: self.section_bounds(),
            color_mode: self.color_mode,
            scene: combined_bounds(&self.clouds),
            viewport: self.drawn_viewport(),
        }
    }

    /// Whether the viewport shows a view as it was saved or restored: it is
    /// the active view, and the camera, the section box, the colour mode and
    /// the scene bounds have not changed since. The viewport may have got
    /// another size; `follow_viewport` deals with that.
    fn shows_view(&self, guid: &str) -> bool {
        let now = self.showing(guid);
        self.active_view().is_some_and(|view| view.guid == guid)
            && self
                .views
                .shown
                .as_ref()
                .is_some_and(|shown| shown.sized_as(&now) == now)
    }

    /// When the viewport has another size than when the active view was
    /// shown, scale the pan of the orbit camera with the picture, as
    /// restoring the view in this viewport does. Only while the viewport
    /// shows the view. Reports whether the size had changed.
    fn follow_viewport(&mut self) -> bool {
        let size = self.drawn_viewport();
        let Some(shown) = &mut self.views.shown else {
            return false;
        };
        if shown.viewport == size {
            return false;
        }
        let grown = size.width.min(size.height) / shown.viewport.width.min(shown.viewport.height);
        if grown.is_finite() && grown > 0.0 {
            self.pan = shown.orbit.3.map(|value| value * grown);
            shown.orbit.3 = self.pan;
        }
        shown.viewport = size;
        self.revision += 1;
        true
    }

    /// Whether the section box, the colour mode and the kind of camera are
    /// the ones a view holds. They are not after a restore in a scene that
    /// the saved section box lies outside of, or that lacks the colours.
    fn holds_what_view_holds(&self, view: &SavedView) -> bool {
        let section = view.section.is_none_or(|section| {
            section.enabled == self.section_enabled
                && self.section_bounds().is_none_or(|shown| {
                    (0..3).all(|axis| {
                        let slack = (section.max[axis] - section.min[axis]).max(1.0) * 1e-6;
                        (shown.min[axis] - section.min[axis]).abs() <= slack
                            && (shown.max[axis] - section.max[axis]).abs() <= slack
                    })
                })
        });
        section
            && view.color_mode.is_none_or(|mode| mode == self.color_mode)
            && view.walk.is_some() == self.walk.is_some()
    }

    fn store_views(&self) -> Result<(), String> {
        camera_views::save(&self.views.list)
            .map_err(|error| format!("the views could not be stored: {error}"))
    }

    /// The section box as a view keeps it: its limits also while it is off.
    fn section_state(&self) -> Option<SectionBox> {
        let overall = self
            .section_reference_bounds
            .or_else(|| combined_bounds(&self.clouds))?;
        let limit = |percent: [f64; 3]| -> Xyz {
            std::array::from_fn(|axis| {
                overall.min[axis] + (overall.max[axis] - overall.min[axis]) * percent[axis] / 100.0
            })
        };
        Some(SectionBox {
            enabled: self.section_enabled,
            min: limit(self.section_min_percent),
            max: limit(self.section_max_percent),
        })
    }

    /// What the viewport shows now, as a view of a scan.
    fn current_view(&self, source: PathBuf, name: &str) -> SavedView {
        let mut view = SavedView::camera(source, name, self.yaw, self.pitch, self.zoom, self.pan);
        view.walk = self.walk.map(|walk| WalkCamera {
            eye: walk.eye,
            yaw: walk.yaw,
            pitch: walk.pitch,
            field_of_view: walk.field_of_view,
        });
        let size = self.drawn_viewport();
        view.frame = combined_bounds(&self.clouds)
            .filter(|_| size.width >= 1.0 && size.height >= 1.0)
            .map(|scene| ViewFrame {
                scene_min: scene.min,
                scene_max: scene.max,
                viewport: [size.width, size.height],
            });
        view.section = self.section_state();
        view.color_mode = Some(self.color_mode);
        view
    }

    /// Save what the viewport shows as a new view of the active scan and
    /// make it the active view. An empty name gives the first free
    /// "View 1", "View 2", ….
    pub fn save_view(&mut self, name: &str) -> Result<Task<Message>, String> {
        let source = self
            .active_camera_source()
            .ok_or_else(|| "Open a scan before saving a view".to_owned())?;
        let existing: Vec<&SavedView> = self
            .views
            .list
            .iter()
            .filter(|view| view.source == source)
            .collect();
        if existing.len() >= MAX_VIEWS_PER_SOURCE {
            return Err(format!(
                "A scan can have at most {MAX_VIEWS_PER_SOURCE} saved views"
            ));
        }
        let taken = |name: &str| {
            existing
                .iter()
                .any(|view| view.name.eq_ignore_ascii_case(name))
        };
        let name = if name.trim().is_empty() {
            (1..=MAX_VIEWS_PER_SOURCE + 1)
                .map(|number| format!("View {number}"))
                .find(|name| !taken(name))
                .unwrap_or_else(|| "View".into())
        } else {
            name.trim().to_owned()
        };
        if checked_name(&name).is_none() || taken(&name) {
            return Err(format!(
                "Choose a unique view name of 1 to {MAX_NAME_CHARS} characters"
            ));
        }
        let mut view = self.current_view(source, &name);
        view.snapshot_due = true;
        let guid = view.guid.clone();
        self.views.list.push(view);
        if let Err(error) = self.store_views() {
            self.views.list.pop();
            return Err(error);
        }
        self.activate_view(&guid);
        self.views.shown = Some(self.showing(&guid));
        self.status = format!("Saved view {name}");
        Ok(self.schedule_snapshot(&guid))
    }

    /// Put the limits of a saved section box back, as far as they lie inside
    /// the model, and switch the box on or off as the view had it.
    fn apply_section(&mut self, section: SectionBox) {
        if let Some(overall) = combined_bounds(&self.clouds) {
            let mut low = [0.0; 3];
            let mut high = [100.0; 3];
            let mut inside = true;
            for axis in 0..3 {
                let span = overall.max[axis] - overall.min[axis];
                if span > 0.0 {
                    low[axis] =
                        ((section.min[axis] - overall.min[axis]) / span * 100.0).clamp(0.0, 100.0);
                    high[axis] =
                        ((section.max[axis] - overall.min[axis]) / span * 100.0).clamp(0.0, 100.0);
                    inside &= low[axis] < high[axis];
                }
            }
            if inside {
                self.section_reference_bounds = Some(overall);
                self.section_min_percent = low;
                self.section_max_percent = high;
            }
        }
        self.section_enabled = section.enabled;
        if section.enabled {
            self.sync_section_coordinate_inputs();
        }
    }

    /// Show a saved view again: its camera, section box and colour mode, and
    /// its annotations.
    fn restore_view(&mut self, index: usize) -> Task<Message> {
        let view = self.views.list[index].clone();
        let mut tasks = Vec::new();
        self.yaw = view.yaw;
        self.pitch = view.pitch;
        self.zoom = view.zoom;
        self.pan = view.pan;
        self.leave_walk();
        if let Some(frame) = view.frame {
            // The orbit camera is relative to the viewport and to the bounds
            // of the scene. In a viewport of another size the pan grows with
            // the picture, and in a scene with other bounds the camera is
            // moved to show what it showed.
            let size = self.drawn_viewport();
            let grown = size.width.min(size.height) / frame.viewport[0].min(frame.viewport[1]);
            if grown.is_finite() && grown > 0.0 {
                self.pan = view.pan.map(|value| value * grown);
            }
            let saved_scene = Some(Bounds {
                min: frame.scene_min,
                max: frame.scene_max,
            });
            if saved_scene != combined_bounds(&self.clouds) {
                self.preserve_camera_for_scene_change(saved_scene);
            }
        }
        if let Some(walk) = view
            .walk
            .filter(|_| combined_bounds(&self.clouds).is_some())
        {
            self.walk = Some(WalkView {
                eye: walk.eye,
                yaw: walk.yaw,
                pitch: walk.pitch,
                field_of_view: walk.field_of_view,
            });
            tasks.push(self.sync_walk_station());
        }
        if let Some(section) = view.section {
            self.apply_section(section);
        }
        if let Some(mode) = view.color_mode.filter(|mode| *mode != self.color_mode) {
            tasks.push(self.update(Message::ColorMode(mode)));
        }
        self.view_label = "SAVED VIEW";
        self.views.drop_placing();
        self.activate_view(&view.guid);
        self.views.shown = self
            .holds_what_view_holds(&view)
            .then(|| self.showing(&view.guid));
        self.revision += 1;
        self.status = format!("Restored view {}", view.name);
        tasks.push(self.schedule_detail());
        // The viewport shows the view again: time for a snapshot that could
        // not be taken before.
        if self.views.shown.is_some()
            && (view.snapshot_due || !camera_views::has_snapshot(&view.guid))
        {
            tasks.push(self.schedule_snapshot(&view.guid));
        }
        Task::batch(tasks)
    }

    /// Overwrite a view with what the viewport shows now. Its name, its
    /// identifier, its time and its annotations stay.
    fn update_view(&mut self, index: usize) -> Result<Task<Message>, String> {
        let old = self.views.list[index].clone();
        let mut view = self.current_view(old.source.clone(), &old.name);
        view.guid = old.guid.clone();
        view.created = old.created;
        view.annotations = old.annotations.clone();
        view.snapshot_due = true;
        self.views.list[index] = view;
        if let Err(error) = self.store_views() {
            self.views.list[index] = old;
            return Err(error);
        }
        self.activate_view(&old.guid);
        self.views.shown = Some(self.showing(&old.guid));
        self.status = format!("Updated view {} to the current view", old.name);
        Ok(self.schedule_snapshot(&old.guid))
    }

    fn delete_view(&mut self, index: usize) -> Result<String, String> {
        let view = self.views.list.remove(index);
        if let Err(error) = self.store_views() {
            self.views.list.insert(index, view);
            return Err(error);
        }
        camera_views::remove_snapshot(&view.guid);
        self.views.snapshots.remove(&view.guid);
        if self
            .views
            .active
            .as_ref()
            .is_some_and(|(guid, _)| *guid == view.guid)
        {
            self.views.active = None;
            self.views.shown = None;
            self.views.drop_placing();
        }
        if self
            .views
            .renaming
            .as_ref()
            .is_some_and(|(guid, _)| *guid == view.guid)
        {
            self.views.renaming = None;
        }
        self.status = format!("Deleted view {}", view.name);
        Ok(view.name)
    }

    fn rename_view(&mut self, index: usize, name: &str) -> Result<String, String> {
        let source = self.views.list[index].source.clone();
        let Some(name) = checked_name(name).filter(|name| {
            !self.views.list.iter().enumerate().any(|(other, view)| {
                other != index && view.source == source && view.name.eq_ignore_ascii_case(name)
            })
        }) else {
            return Err(format!(
                "Choose a unique view name of 1 to {MAX_NAME_CHARS} characters"
            ));
        };
        let old = std::mem::replace(&mut self.views.list[index].name, name.to_owned());
        if let Err(error) = self.store_views() {
            self.views.list[index].name = old;
            return Err(error);
        }
        self.status = format!("Renamed view {old} to {name}");
        Ok(name.to_owned())
    }

    /// Add an annotation to the active view. Without an active view the
    /// current view is saved first and becomes it.
    fn add_annotation(&mut self, annotation: Annotation) -> Result<Task<Message>, String> {
        let index = match self.active_view_index() {
            Some(index) => index,
            None => {
                // The snapshot asked for here is replaced by the one below.
                let _ = self.save_view("")?;
                self.views.list.len() - 1
            }
        };
        let view = &mut self.views.list[index];
        if view.annotations.len() >= MAX_ANNOTATIONS {
            return Err(format!(
                "A view holds at most {MAX_ANNOTATIONS} annotations"
            ));
        }
        view.annotations.push(annotation);
        let was_due = std::mem::replace(&mut view.snapshot_due, true);
        let (guid, name) = (view.guid.clone(), view.name.clone());
        if let Err(error) = self.store_views() {
            let view = &mut self.views.list[index];
            view.annotations.pop();
            view.snapshot_due = was_due;
            return Err(error);
        }
        let count = self.views.list[index].annotations.len();
        self.status = format!("Annotation {count} added to view {name}");
        Ok(self.renew_snapshot(&guid))
    }

    fn add_note(&mut self, point: Xyz, text: &str) -> Result<Task<Message>, String> {
        let text = checked_note(text)
            .ok_or_else(|| format!("A note needs a text of 1 to {MAX_NOTE_CHARS} characters"))?;
        if !finite(&point) {
            return Err("A note needs a finite [x, y, z] point".into());
        }
        self.add_annotation(Annotation::note(point, &text))
    }

    fn add_line(&mut self, from: Xyz, to: Xyz) -> Result<Task<Message>, String> {
        if !finite(&from) || !finite(&to) || from == to {
            return Err("A line needs two different finite [x, y, z] points".into());
        }
        self.add_annotation(Annotation::Line { from, to })
    }

    fn delete_annotation(&mut self, place: usize) -> Result<Task<Message>, String> {
        let index = self
            .active_view_index()
            .ok_or_else(|| "No view is active".to_owned())?;
        let view = &mut self.views.list[index];
        if place >= view.annotations.len() {
            return Err("The active view has no such annotation".into());
        }
        let removed = view.annotations.remove(place);
        let was_due = std::mem::replace(&mut view.snapshot_due, true);
        let guid = view.guid.clone();
        if let Err(error) = self.store_views() {
            let view = &mut self.views.list[index];
            view.annotations.insert(place, removed);
            view.snapshot_due = was_due;
            return Err(error);
        }
        self.status = "Annotation removed".into();
        Ok(self.renew_snapshot(&guid))
    }

    /// A point picked with an annotation tool: the point of a note, or the
    /// start or the end of a line.
    fn place_point(&mut self, xyz: Xyz) -> Task<Message> {
        match self.views.tool {
            Some(AnnotationKind::Note) => {
                self.views.note_point = Some(xyz);
                self.status = "Type the note and press Enter; Escape cancels".into();
                return text_input::focus(note_input_id());
            }
            Some(AnnotationKind::Line) => match self.views.line_start.take() {
                None => {
                    self.views.line_start = Some(xyz);
                    self.status = "Start of the line picked; click its end point".into();
                }
                Some(start) => match self.add_line(start, xyz) {
                    Ok(task) => return task,
                    Err(error) => {
                        self.views.line_start = Some(start);
                        self.status = error;
                    }
                },
            },
            None => {}
        }
        Task::none()
    }

    /// Ask for a snapshot of a view once what changed has been drawn.
    fn schedule_snapshot(&mut self, guid: &str) -> Task<Message> {
        self.views.snapshot_serial += 1;
        let serial = self.views.snapshot_serial;
        self.views.snapshots.insert(guid.to_owned(), serial);
        self.snapshot_timer(guid.to_owned(), serial, SNAPSHOT_TRIES)
    }

    /// The annotations of a view changed. Its snapshot is taken anew when
    /// the viewport shows the view. When the camera, the section box, the
    /// colour mode or the scene has changed since, the snapshot it has
    /// stays, and the new one waits until the view is restored.
    fn renew_snapshot(&mut self, guid: &str) -> Task<Message> {
        if self.shows_view(guid) {
            return self.schedule_snapshot(guid);
        }
        self.views.snapshots.remove(guid);
        self.status
            .push_str("; its snapshot is renewed when the view is restored");
        Task::none()
    }

    fn snapshot_timer(&self, guid: String, serial: u64, tries: u8) -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(SNAPSHOT_DELAY).await },
            move |()| {
                Message::Views(ViewAction::Capture {
                    guid: guid.clone(),
                    serial,
                    tries,
                })
            },
        )
    }

    /// Keep the camera of a view relative to the viewport and the scene it
    /// is shown in now, so that it and the snapshot taken now belong
    /// together. Only for a view the viewport shows. Reports a change.
    fn frame_as_shown(&mut self, index: usize) -> bool {
        let view = &self.views.list[index];
        let now = self.current_view(view.source.clone(), &view.name);
        if now.frame.is_none() || now.frame == view.frame {
            return false;
        }
        let view = &mut self.views.list[index];
        view.yaw = now.yaw;
        view.pitch = now.pitch;
        view.zoom = now.zoom;
        view.pan = now.pan;
        view.walk = now.walk;
        view.frame = now.frame;
        true
    }

    pub fn update_views(&mut self, action: ViewAction) -> Task<Message> {
        match action {
            ViewAction::Name(name) => self.views.name = name,
            ViewAction::Save => {
                let name = self.views.name.clone();
                match self.save_view(&name) {
                    Ok(task) => {
                        self.views.name.clear();
                        return task;
                    }
                    Err(error) => self.status = error,
                }
            }
            ViewAction::Restore(guid) => {
                if let Some(index) = self.view_index(&guid) {
                    return self.restore_view(index);
                }
            }
            ViewAction::Update(guid) => {
                if let Some(index) = self.view_index(&guid) {
                    match self.update_view(index) {
                        Ok(task) => return task,
                        Err(error) => self.status = error,
                    }
                }
            }
            ViewAction::Delete(guid) => {
                if let Some(index) = self.view_index(&guid) {
                    if let Err(error) = self.delete_view(index) {
                        self.status = error;
                    }
                }
            }
            ViewAction::StartRename(guid) => {
                if let Some(index) = self.view_index(&guid) {
                    self.views.renaming = Some((guid, self.views.list[index].name.clone()));
                    return text_input::focus(rename_input_id());
                }
            }
            ViewAction::RenameText(value) => {
                if let Some((_, name)) = &mut self.views.renaming {
                    *name = value;
                }
            }
            ViewAction::FinishRename => {
                let Some((guid, name)) = self.views.renaming.clone() else {
                    return Task::none();
                };
                match self.view_index(&guid) {
                    Some(index) => match self.rename_view(index, &name) {
                        Ok(_) => self.views.renaming = None,
                        Err(error) => self.status = error,
                    },
                    None => self.views.renaming = None,
                }
            }
            ViewAction::CancelRename => self.views.renaming = None,
            ViewAction::Deactivate => {
                self.views.active = None;
                self.views.shown = None;
                self.views.drop_placing();
                self.status = "Annotations hidden; restore a view to show them again".into();
            }
            ViewAction::Tool(kind) => {
                let stop = self.views.tool == Some(kind);
                self.views.leave_tool();
                if stop {
                    self.status = "Annotation tool closed".into();
                } else if self.active.is_none() {
                    self.status = "Open a scan before placing annotations".into();
                } else {
                    self.views.tool = Some(kind);
                    self.views.tool_scan = self.active_scan_path().map(Path::to_path_buf);
                    self.box_select = false;
                    self.pick_mode = false;
                    self.measure.leave(true);
                    self.drag_rectangle = None;
                    self.status = match kind {
                        AnnotationKind::Note => {
                            "Note: click the point it is about, then type its text; Escape cancels"
                        }
                        AnnotationKind::Line => {
                            "Line: click its start and its end point; Escape cancels"
                        }
                    }
                    .into();
                }
            }
            ViewAction::Click(pointer, size) => {
                if self.views.tool.is_none() {
                    return Task::none();
                }
                match self.start_point_pick(pointer, measure::PICK_RADIUS, size, None) {
                    Ok(task) => return task.map(picked_message),
                    Err(error) => self.status = error,
                }
            }
            ViewAction::Picked(revision, result) => {
                self.selection_pending = false;
                if self.selection_cancel.load(Ordering::Relaxed) {
                    self.status = "Annotation pick cancelled".into();
                    return Task::none();
                }
                if revision != self.revision || self.views.tool.is_none() {
                    self.status = "Annotation pick discarded because the view changed".into();
                    return Task::none();
                }
                match result {
                    // The pick search answers in scene coordinates: the
                    // layer's live transform is already applied.
                    Ok(Some(record)) => return self.place_point(record.point.xyz),
                    Ok(None) => {
                        self.status = format!("No point within {} pixels", measure::PICK_RADIUS);
                    }
                    Err(error) => self.status = format!("Annotation pick failed: {error}"),
                }
            }
            ViewAction::NoteText(value) => self.views.note_text = value,
            ViewAction::NoteSubmit => {
                let Some(point) = self.views.note_point else {
                    return Task::none();
                };
                let text = self.views.note_text.clone();
                match self.add_note(point, &text) {
                    Ok(task) => {
                        self.views.drop_placing();
                        return task;
                    }
                    Err(error) => self.status = error,
                }
            }
            ViewAction::CancelPlacing => {
                if self.views.drop_placing() {
                    self.status = "Annotation cancelled; click a point to place another".into();
                }
            }
            ViewAction::DeleteAnnotation(place) => match self.delete_annotation(place) {
                Ok(task) => return task,
                Err(error) => self.status = error,
            },
            ViewAction::Capture {
                guid,
                serial,
                tries,
            } => {
                if self.views.snapshots.get(&guid) != Some(&serial) {
                    // A newer request for this view is on its way.
                    return Task::none();
                }
                let shown = self.shows_view(&guid)
                    && !self.file_open
                    && self.settings.is_none()
                    && self.views.canvas.get().is_some();
                if !shown {
                    // The viewport shows something else than the view holds.
                    // The snapshot stays due until the view is restored.
                    self.views.snapshots.remove(&guid);
                    return Task::none();
                }
                if self.views.line_start.is_some() || self.views.note_point.is_some() {
                    // The snapshot waits until the next annotation is placed
                    // or cancelled, so it shows neither its marker nor the
                    // field for its text.
                    return self.snapshot_timer(guid, serial, tries);
                }
                if self.follow_viewport() {
                    // The viewport got another size, by the window or by a
                    // status text of more lines. The snapshot waits for the
                    // picture of the view in this size.
                    return Task::batch([
                        self.schedule_detail(),
                        self.snapshot_timer(guid, serial, tries),
                    ]);
                }
                if tries > 0 && self.detail_pending {
                    // The points of this camera are still being read.
                    return self.snapshot_timer(guid, serial, tries - 1);
                }
                return iced::window::get_latest()
                    .then(|window| match window {
                        Some(window) => iced::window::screenshot(window).map(Some),
                        None => Task::done(None),
                    })
                    .map(move |screenshot| {
                        Message::Views(ViewAction::Captured(guid.clone(), serial, screenshot))
                    });
            }
            ViewAction::Captured(guid, serial, screenshot) => {
                if self.views.snapshots.get(&guid) != Some(&serial) {
                    return Task::none();
                }
                let (Some(screenshot), Some(canvas)) = (screenshot, self.views.canvas.get()) else {
                    self.views.snapshots.remove(&guid);
                    self.status = "The view is saved; its snapshot could not be taken".into();
                    return Task::none();
                };
                let Some(index) = self.active_view_index().filter(|_| {
                    self.views.shown.as_ref() == Some(&self.showing(&guid))
                        && self.shows_view(&guid)
                }) else {
                    // The viewport changed while the screenshot was taken.
                    self.views.snapshots.remove(&guid);
                    return Task::none();
                };
                if self.frame_as_shown(index) {
                    if let Err(error) = self.store_views() {
                        self.status = error;
                    }
                }
                let target = guid.clone();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            let png = encode_snapshot(
                                &screenshot.bytes,
                                screenshot.size.width,
                                screenshot.size.height,
                                screenshot.scale_factor,
                                canvas,
                            )?;
                            camera_views::write_snapshot(&target, &png)
                                .map_err(|error| error.to_string())
                        })
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result)
                    },
                    move |result| {
                        Message::Views(ViewAction::SnapshotSaved(guid.clone(), serial, result))
                    },
                );
            }
            ViewAction::SnapshotSaved(guid, serial, result) => {
                let newest = self.views.snapshots.get(&guid) == Some(&serial);
                if newest {
                    self.views.snapshots.remove(&guid);
                }
                let Some(view) = self.views.list.iter_mut().find(|view| view.guid == guid) else {
                    // The view was deleted while its snapshot was written.
                    camera_views::remove_snapshot(&guid);
                    return Task::none();
                };
                match result {
                    // A view that changed after this request stays due.
                    Ok(()) if newest && view.snapshot_due => {
                        view.snapshot_due = false;
                        if let Err(error) = self.store_views() {
                            self.status = error;
                        }
                    }
                    Ok(()) => {}
                    Err(error) => {
                        self.status = format!("The view is saved; its snapshot failed: {error}");
                    }
                }
            }
            ViewAction::ExportBcf => {
                if self.views.export_pending {
                    return Task::none();
                }
                let Some(entry) = self.active.and_then(|index| self.clouds.get(index)) else {
                    self.status = "Open a scan before exporting its views".into();
                    return Task::none();
                };
                if self.source_views().is_empty() {
                    self.status = "Save a view of this scan before exporting BCF".into();
                    return Task::none();
                }
                let suggested = format!(
                    "{}.bcf",
                    entry
                        .cloud
                        .path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("views")
                );
                self.views.export_pending = true;
                self.status = "Choose where to save the views as BCF…".into();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .add_filter("BIM Collaboration Format", &["bcf"])
                            .set_file_name(suggested)
                            .save_file()
                            .await
                            .map(|selection| selection.path().to_path_buf())
                    },
                    |path| Message::Views(ViewAction::ExportPathChosen(path)),
                );
            }
            ViewAction::ExportPathChosen(path) => {
                let Some(mut path) = path else {
                    self.views.export_pending = false;
                    self.status = "BCF export cancelled".into();
                    return Task::none();
                };
                if !is_bcf_path(&path) {
                    path.set_extension("bcf");
                }
                let views = self.views_to_export();
                let Some(scene) = combined_bounds(&self.clouds).filter(|_| !views.is_empty())
                else {
                    self.views.export_pending = false;
                    self.status = "There are no views to export".into();
                    return Task::none();
                };
                let viewport = self.drawn_viewport();
                self.status = "Exporting views as BCF…".into();
                return Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || {
                            write_bcf(&path, &views, scene, viewport)
                                .map(|exported| (path, exported))
                        })
                        .await
                        .map_err(|error| error.to_string())
                        .and_then(|result| result)
                    },
                    |result| Message::Views(ViewAction::Exported(result)),
                );
            }
            ViewAction::Exported(result) => {
                self.views.export_pending = false;
                self.status = match result {
                    Ok((path, exported)) => exported.status(&path),
                    Err(error) => format!("BCF export failed: {error}"),
                };
            }
        }
        Task::none()
    }

    /// The views of the active scan as an export writes them. A view saved
    /// before times were kept gets the time of its first export, so that its
    /// topic keeps one creation date.
    fn views_to_export(&mut self) -> Vec<SavedView> {
        let source = self.active_camera_source();
        let now = camera_views::now_seconds();
        let mut stamped = false;
        for view in &mut self.views.list {
            if view.created == 0 && Some(&view.source) == source.as_ref() {
                view.created = now;
                stamped = true;
            }
        }
        if stamped {
            // The export goes on with the time when it cannot be kept.
            let _ = self.store_views();
        }
        self.source_views()
    }

    /// The views of the active scan, in the order they were saved.
    fn source_views(&self) -> Vec<SavedView> {
        let source = self.active_camera_source();
        self.views
            .list
            .iter()
            .filter(|view| Some(&view.source) == source.as_ref())
            .cloned()
            .collect()
    }

    /// The view commands of the command API.
    pub fn api_views(&mut self, command: ApiCommand) -> (Value, Task<Message>) {
        let done = |value: Value| (value, Task::none());
        let failed = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let not_found = || {
            (
                json!({"ok": false, "error": "camera view not found for the active scan"}),
                Task::none(),
            )
        };
        match command {
            ApiCommand::ListCameraViews => done(json!({
                "ok": true,
                "source": self.active_camera_source(),
                "views": self.source_views(),
                "active": self.active_view().map(|view| view.name.clone()),
            })),
            ApiCommand::SaveCameraView { name } => match self.save_view(&name) {
                Ok(task) => {
                    let view = &self.views.list[self.views.list.len() - 1];
                    (
                        json!({"ok": true, "name": view.name, "guid": view.guid}),
                        task,
                    )
                }
                Err(error) => failed(error),
            },
            ApiCommand::UpdateCameraView { name } => match self.view_named(&name) {
                Some(index) => match self.update_view(index) {
                    Ok(task) => (json!({"ok": true, "view": self.views.list[index]}), task),
                    Err(error) => failed(error),
                },
                None => not_found(),
            },
            ApiCommand::RenameCameraView { name, new_name } => match self.view_named(&name) {
                Some(index) => match self.rename_view(index, &new_name) {
                    Ok(name) => done(json!({"ok": true, "name": name})),
                    Err(error) => failed(error),
                },
                None => not_found(),
            },
            ApiCommand::RestoreCameraView { name } => match self.view_named(&name) {
                Some(index) => {
                    let task = self.restore_view(index);
                    (json!({"ok": true, "view": self.views.list[index]}), task)
                }
                None => not_found(),
            },
            ApiCommand::DeleteCameraView { name } => match self.view_named(&name) {
                Some(index) => match self.delete_view(index) {
                    Ok(name) => done(json!({"ok": true, "name": name})),
                    Err(error) => failed(error),
                },
                None => not_found(),
            },
            ApiCommand::AddNote { point, text } => match self.add_note(point, &text) {
                Ok(task) => (self.annotations_value(), task),
                Err(error) => failed(error),
            },
            ApiCommand::AddLine { from, to } => match self.add_line(from, to) {
                Ok(task) => (self.annotations_value(), task),
                Err(error) => failed(error),
            },
            ApiCommand::DeleteAnnotation { index } => match self.delete_annotation(index) {
                Ok(task) => (self.annotations_value(), task),
                Err(error) => failed(error),
            },
            ApiCommand::SetAnnotationTool { tool } => {
                let kind = match tool.as_deref().map(str::to_ascii_lowercase).as_deref() {
                    None | Some("none") => None,
                    Some("note") => Some(AnnotationKind::Note),
                    Some("line") => Some(AnnotationKind::Line),
                    Some(_) => return failed("annotation tool must be note, line or null".into()),
                };
                let task = match kind {
                    Some(kind) if self.views.tool != Some(kind) => {
                        self.update_views(ViewAction::Tool(kind))
                    }
                    Some(_) => Task::none(),
                    None => {
                        if self.views.leave_tool() {
                            self.status = "Annotation tool closed".into();
                        }
                        Task::none()
                    }
                };
                if self.views.tool == kind {
                    let tool = kind.map(AnnotationKind::key);
                    (json!({"ok": true, "annotation_tool": tool}), task)
                } else {
                    failed(self.status.clone())
                }
            }
            ApiCommand::AnnotateScreen { pointer } => {
                let size = self.drawn_viewport();
                if self.views.tool.is_none() {
                    failed("choose an annotation tool with set_annotation_tool first".into())
                } else if !pointer.into_iter().all(f32::is_finite)
                    || pointer[0] < 0.0
                    || pointer[1] < 0.0
                    || pointer[0] > size.width
                    || pointer[1] > size.height
                {
                    failed("pointer must lie inside the viewport".into())
                } else if self.selection_pending {
                    failed("a full-resolution selection is already running".into())
                } else {
                    let task = self.update_views(ViewAction::Click(pointer, size));
                    if self.selection_pending {
                        (json!({"ok": true, "accepted": true}), task)
                    } else {
                        failed(self.status.clone())
                    }
                }
            }
            ApiCommand::SubmitNote { text } => {
                if self.views.note_point.is_none() {
                    failed("no note is waiting for its text".into())
                } else {
                    let before = std::mem::replace(&mut self.views.note_text, text);
                    let task = self.update_views(ViewAction::NoteSubmit);
                    if self.views.note_point.is_none() {
                        (self.annotations_value(), task)
                    } else {
                        self.views.note_text = before;
                        failed(self.status.clone())
                    }
                }
            }
            ApiCommand::ExportBcf { path } => {
                if !path.is_absolute() || !is_bcf_path(&path) {
                    return failed("export_bcf requires an absolute .bcf destination".into());
                }
                let views = self.views_to_export();
                let scene = combined_bounds(&self.clouds);
                if let (Some(scene), false) = (scene, views.is_empty()) {
                    match write_bcf(&path, &views, scene, self.drawn_viewport()) {
                        Ok(exported) => {
                            self.status = exported.status(&path);
                            done(json!({
                                "ok": true,
                                "path": path,
                                "views": exported.views,
                                "snapshots": exported.snapshots,
                                "snapshots_due": exported.due,
                            }))
                        }
                        Err(error) => failed(error),
                    }
                } else {
                    failed("save a view of the active scan before exporting BCF".into())
                }
            }
            _ => failed("not a view command".into()),
        }
    }

    /// The active view and its annotations, as the annotation commands answer.
    fn annotations_value(&self) -> Value {
        match self.active_view() {
            Some(view) => json!({
                "ok": true,
                "view": view.name,
                "guid": view.guid,
                "annotations": view.annotations,
            }),
            None => json!({"ok": false, "error": "no view is active"}),
        }
    }

    /// What `status.result.views` reports.
    pub fn views_value(&self) -> Value {
        json!({
            "active": self.active_view().map(|view| json!({
                "name": view.name,
                "guid": view.guid,
                "annotations": view.annotations,
            })),
            "annotation_tool": self.views.tool.map(AnnotationKind::key),
            "placing": self.views.note_point
                .map(|point| json!({"kind": "note", "point": point}))
                .or_else(|| {
                    self.views
                        .line_start
                        .map(|point| json!({"kind": "line", "point": point}))
                }),
            "snapshots_pending": self.views.snapshots.len(),
            "export_pending": self.views.export_pending,
        })
    }

    /// Save view, Note, Line and Export BCF for the ribbon's views group.
    pub fn views_ribbon(&self) -> Element<'static, Message> {
        let has_scan = self.active.is_some();
        let tool = |label: &'static str, kind: AnnotationKind| {
            opencad_ribbon::RibbonItem::Small(crate::small_tool_button_when(
                label,
                Message::Views(ViewAction::Tool(kind)),
                self.views.tool == Some(kind),
                has_scan,
            ))
        };
        opencad_ribbon::render_group_items(
            "VIEWS",
            vec![
                opencad_ribbon::RibbonItem::Small(crate::small_tool_button_when(
                    "Save view",
                    Message::Views(ViewAction::Save),
                    false,
                    has_scan,
                )),
                tool("Note", AnnotationKind::Note),
                tool("Line", AnnotationKind::Line),
                opencad_ribbon::RibbonItem::Small(crate::small_tool_button_when(
                    "Export BCF",
                    Message::Views(ViewAction::ExportBcf),
                    false,
                    self.can_export_bcf(),
                )),
            ],
        )
    }

    /// Whether the active scan has views to export and no export is running.
    pub fn can_export_bcf(&self) -> bool {
        !self.views.export_pending
            && self
                .active_camera_source()
                .is_some_and(|source| self.views.list.iter().any(|view| view.source == source))
    }

    /// The "Views" section of the Properties panel: the camera, the name
    /// field, the views of the active scan and the annotations of the
    /// active view.
    pub fn views_properties(&self) -> Element<'_, Message> {
        let flat = crate::flat_tool_style;
        let small = |label: &'static str, action: ViewAction| {
            button(text(i18n::tr(label)).size(10))
                .on_press(Message::Views(action))
                .style(flat)
                .padding([3, 4])
        };
        let mut section = column![
            opencad_properties::section_header("Views"),
            opencad_properties::property_row(
                "Yaw / pitch",
                format!(
                    "{:.0}° / {:.0}°",
                    self.yaw.to_degrees(),
                    self.pitch.to_degrees()
                ),
            ),
            opencad_properties::property_row("Zoom", crate::format_zoom_level(self.zoom)),
            container(
                row![
                    text_input(i18n::tr("View name"), &self.views.name)
                        .on_input(|name| Message::Views(ViewAction::Name(name)))
                        .on_submit(Message::Views(ViewAction::Save))
                        .size(11)
                        .padding([3, 5])
                        .width(Fill),
                    button(i18n::tr("Save"))
                        .on_press_maybe(
                            self.active
                                .is_some()
                                .then_some(Message::Views(ViewAction::Save)),
                        )
                        .style(flat),
                ]
                .spacing(4)
                .align_y(iced::Alignment::Center),
            )
            .padding([5, 8]),
        ]
        .spacing(0);
        let source = self.active_camera_source();
        let active = self.active_view();
        for view in self
            .views
            .list
            .iter()
            .filter(|view| Some(&view.source) == source.as_ref())
        {
            let guid = || view.guid.clone();
            let line = match &self.views.renaming {
                Some((renamed, name)) if *renamed == view.guid => row![
                    text_input(i18n::tr("View name"), name)
                        .id(rename_input_id())
                        .on_input(|name| Message::Views(ViewAction::RenameText(name)))
                        .on_submit(Message::Views(ViewAction::FinishRename))
                        .size(11)
                        .padding([3, 5])
                        .width(Fill),
                    small("OK", ViewAction::FinishRename),
                    small("Cancel", ViewAction::CancelRename),
                ],
                _ => {
                    let is_active = active.is_some_and(|active| active.guid == view.guid);
                    row![
                        button(text(view.name.as_str()).size(11))
                            .on_press(Message::Views(ViewAction::Restore(guid())))
                            .style(move |theme, status| {
                                opencad_ribbon::tool_btn_style(theme, is_active, status)
                            })
                            .padding([3, 5])
                            .width(Fill),
                        small("Rename", ViewAction::StartRename(guid())),
                        small("Update", ViewAction::Update(guid())),
                        button("×")
                            .on_press(Message::Views(ViewAction::Delete(guid())))
                            .style(flat)
                            .padding([3, 6]),
                    ]
                }
            };
            section = section
                .push(container(line.spacing(2).align_y(iced::Alignment::Center)).padding([2, 8]));
        }
        if let Some(view) = active {
            section = section.push(
                container(
                    row![
                        text(format!(
                            "{} · {}",
                            i18n::tr("Annotations"),
                            shortened(&view.name, 22)
                        ))
                        .size(10)
                        .width(Fill),
                        small("Hide", ViewAction::Deactivate),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                )
                .padding([6, 8]),
            );
            if view.annotations.is_empty() {
                section = section.push(
                    container(text(i18n::tr("Place a Note or a Line from the ribbon")).size(10))
                        .padding([2, 8]),
                );
            }
            for (place, annotation) in view.annotations.iter().enumerate() {
                let label = match annotation {
                    Annotation::Note { text, .. } => {
                        format!("{} · {}", i18n::tr("Note"), shortened(text, 30))
                    }
                    Annotation::Line { from, to } => format!(
                        "{} · {}",
                        i18n::tr("Line"),
                        measure::format_length(measure::distance(*from, *to))
                    ),
                };
                section = section.push(
                    container(
                        row![
                            text(label).size(11).width(Fill),
                            button("×")
                                .on_press(Message::Views(ViewAction::DeleteAnnotation(place)))
                                .style(flat)
                                .padding([3, 6]),
                        ]
                        .spacing(3)
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([1, 8]),
                );
            }
        }
        section.into()
    }

    /// The field for the text of a note whose point has been picked, shown
    /// over the top of the viewport.
    pub fn note_prompt(&self) -> Option<Element<'_, Message>> {
        self.views.note_point?;
        let ready = checked_note(&self.views.note_text).is_some();
        let prompt = container(
            row![
                text_input(i18n::tr("Text of the note"), &self.views.note_text)
                    .id(note_input_id())
                    .on_input(|value| Message::Views(ViewAction::NoteText(value)))
                    .on_submit(Message::Views(ViewAction::NoteSubmit))
                    .size(12)
                    .padding([4, 6])
                    .width(300),
                button(text(i18n::tr("Add")).size(12))
                    .on_press_maybe(ready.then_some(Message::Views(ViewAction::NoteSubmit)))
                    .style(crate::flat_tool_style),
                button(text(i18n::tr("Cancel")).size(12))
                    .on_press(Message::Views(ViewAction::CancelPlacing))
                    .style(crate::flat_tool_style),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding(6)
        .style(|theme| {
            let colors = crate::ui_theme::colors(theme);
            container::Style::default()
                .background(colors.panel)
                .border(iced::Border {
                    color: mark_color(),
                    width: 1.0,
                    radius: 4.0.into(),
                })
        });
        // Below the button that leaves the walking camera.
        let top = if self.walk.is_some() { 48.0 } else { 10.0 };
        Some(
            container(prompt)
                .padding(iced::Padding {
                    top,
                    left: 10.0,
                    ..iced::Padding::ZERO
                })
                .into(),
        )
    }

    /// What the viewport draws of the views: the annotations of the active
    /// view and the annotation that is being placed.
    pub fn views_overlay(&self) -> Overlay<'_> {
        Overlay {
            tool: self.views.tool,
            annotations: self
                .active_view()
                .map_or(&[], |view| view.annotations.as_slice()),
            line_start: self.views.line_start,
            note_point: self.views.note_point,
            canvas: &self.views.canvas,
        }
    }
}

/// The annotations the viewport shows, and the place of its canvas.
#[derive(Clone, Copy)]
pub struct Overlay<'a> {
    /// Annotation tool that clicks in the viewport feed, if one is active.
    pub tool: Option<AnnotationKind>,
    annotations: &'a [Annotation],
    line_start: Option<Xyz>,
    note_point: Option<Xyz>,
    canvas: &'a Cell<Option<Rectangle>>,
}

impl Overlay<'_> {
    /// Note where the scene canvas lies in the window, for its snapshot.
    pub fn drawn_at(&self, bounds: Rectangle) {
        self.canvas.set(Some(bounds));
    }
}

/// Colour of annotations, apart from the amber of measurements.
fn mark_color() -> Color {
    Color::from_rgb8(239, 68, 68)
}

fn mark_stroke(width: f32) -> canvas::Stroke<'static> {
    canvas::Stroke::default()
        .with_color(mark_color())
        .with_width(width)
}

/// A ring around a picked point.
fn draw_marker(frame: &mut Frame, at: UiPoint, radius: f32) {
    let marker = canvas::Path::circle(at, radius);
    frame.fill(&marker, Color::from_rgb8(42, 42, 50));
    frame.stroke(&marker, mark_stroke(2.0));
}

/// Where the label of a note at a viewport position stands: to the upper
/// right of the point, to its left near the right edge and below it near the
/// top, inside the viewport when it is wider than the room on either side,
/// and further from the point while it would cover a marker or a label
/// placed before.
fn label_place(at: UiPoint, width: f32, size: Size, placed: &[Rectangle]) -> Rectangle {
    let leftwards = at.x + 18.0 + width > size.width && at.x - 18.0 - width >= 0.0;
    let x = if leftwards {
        at.x - 18.0 - width
    } else {
        at.x + 18.0
    };
    let x = x.min(size.width - width).max(0.0);
    let above = at.y - 40.0 >= 0.0;
    let step = if above { -LABEL_STEP } else { LABEL_STEP };
    let mut label = Rectangle::new(
        UiPoint::new(x, if above { at.y - 40.0 } else { at.y + 22.0 }),
        Size::new(width, LABEL_HEIGHT),
    );
    for _ in 0..LABEL_STEPS {
        let next = label.y + step;
        if !placed.iter().any(|other| other.intersects(&label))
            || next < 0.0
            || next + LABEL_HEIGHT > size.height
        {
            break;
        }
        label.y = next;
    }
    label
}

/// The room the marker of a note takes around its point.
fn marker_area(at: UiPoint) -> Rectangle {
    Rectangle::new(
        UiPoint::new(at.x - MARKER_REACH, at.y - MARKER_REACH),
        Size::new(MARKER_REACH * 2.0, MARKER_REACH * 2.0),
    )
}

/// The notes of a view at their viewport positions: a marker on each point
/// and a leader to its label. The labels keep clear of each other and of the
/// markers of all notes.
fn draw_notes(frame: &mut Frame, notes: &[(UiPoint, &str)], size: Size) {
    let mut taken: Vec<Rectangle> = notes.iter().map(|(at, _)| marker_area(*at)).collect();
    let labels: Vec<(Rectangle, String)> = notes
        .iter()
        .map(|(at, content)| {
            let content = shortened(content, LABEL_CHARS);
            let width = content.chars().count() as f32 * 6.2 + 10.0;
            let label = label_place(*at, width, size, &taken);
            taken.push(label);
            (label, content)
        })
        .collect();
    for ((at, _), (label, _)) in notes.iter().zip(&labels) {
        // The leader ends on the edge of the label that faces the point.
        let anchor = UiPoint::new(
            at.x.clamp(label.x, label.x + label.width),
            if label.y < at.y {
                label.y + label.height
            } else {
                label.y
            },
        );
        frame.stroke(&canvas::Path::line(*at, anchor), mark_stroke(1.5));
    }
    for (label, content) in labels {
        frame.fill_rectangle(label.position(), label.size(), Color::from_rgb8(42, 42, 50));
        frame.stroke_rectangle(label.position(), label.size(), mark_stroke(1.0));
        frame.fill_text(canvas::Text {
            content,
            position: UiPoint::new(label.x + 5.0, label.y + 3.0),
            size: iced::Pixels(11.0),
            color: Color::from_rgb8(245, 245, 244),
            ..canvas::Text::default()
        });
    }
    for (at, _) in notes {
        draw_marker(frame, *at, 5.0);
    }
}

/// A line annotation: an arrow from its first point to its second. The head
/// is drawn when the second point itself is in view.
fn draw_arrow(frame: &mut Frame, projection: Projection, from: Xyz, to: Xyz, size: Size) {
    let Some([start, end]) = measure::project_edge(projection, from, to, size) else {
        return;
    };
    frame.stroke(&canvas::Path::line(start, end), mark_stroke(2.0));
    if let Some((x, y, _)) = projection.project(from) {
        frame.fill(&canvas::Path::circle(UiPoint::new(x, y), 3.0), mark_color());
    }
    let tip_shown = projection
        .project_unclipped(to)
        .is_some_and(|(x, y, _)| (x - end.x).hypot(y - end.y) < 1.0);
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let length = dx.hypot(dy);
    if !tip_shown || length < 6.0 {
        return;
    }
    let (ux, uy) = (dx / length, dy / length);
    let reach = length.min(13.0);
    for side in [-1.0f32, 1.0] {
        // Two strokes back from the tip, 25 degrees to either side.
        let (sin, cos) = (side * 0.436).sin_cos();
        let back = UiPoint::new(
            end.x - (ux * cos - uy * sin) * reach,
            end.y - (ux * sin + uy * cos) * reach,
        );
        frame.stroke(&canvas::Path::line(end, back), mark_stroke(2.0));
    }
}

impl PointViewport<'_> {
    /// Draw the annotations of the active view with the camera in use, so
    /// they stay on their points in the orbit view and while walking.
    pub fn draw_annotations(&self, frame: &mut Frame, size: Size) {
        let overlay = self.annotate;
        if overlay.annotations.is_empty()
            && overlay.line_start.is_none()
            && overlay.note_point.is_none()
        {
            return;
        }
        let Some(scene) = combined_bounds(self.clouds) else {
            return;
        };
        let projection = self.projection(scene, size.width, size.height);
        let on_screen = |xyz: Xyz| projection.project(xyz).map(|(x, y, _)| UiPoint::new(x, y));
        for annotation in overlay.annotations {
            if let Annotation::Line { from, to } = annotation {
                draw_arrow(frame, projection, *from, *to, size);
            }
        }
        // Notes lie over the lines.
        let notes: Vec<(UiPoint, &str)> = overlay
            .annotations
            .iter()
            .filter_map(|annotation| match annotation {
                Annotation::Note { point, text, .. } => {
                    on_screen(*point).map(|at| (at, text.as_str()))
                }
                Annotation::Line { .. } => None,
            })
            .collect();
        draw_notes(frame, &notes, size);
        for point in [overlay.line_start, overlay.note_point]
            .into_iter()
            .flatten()
        {
            if let Some(at) = on_screen(point) {
                draw_marker(frame, at, 6.0);
                frame.fill(&canvas::Path::circle(at, 2.0), mark_color());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pointcloud_core::Point;

    use super::*;
    use crate::bcf::reading::{entries, parse};
    use crate::native_api::ApiRequest;
    use crate::{finish_viewport_drag, ColorMode, DragMode, DragState};

    /// A studio with one small scan open, and a directory of its own for the
    /// views and snapshots of this test.
    fn studio_with_scan() -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let path = directory.path().join("scan.xyz");
        std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        assert!(studio.views.list.is_empty());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        (studio, directory)
    }

    fn act(studio: &mut Studio, action: ViewAction) {
        let _ = studio.update(Message::Views(action));
    }

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    fn command(json: &str) -> ApiCommand {
        serde_json::from_str(json).unwrap()
    }

    fn picked(xyz: Xyz) -> Result<Option<IndexedPoint>, String> {
        Ok(Some(IndexedPoint {
            point: Point {
                xyz,
                rgb: None,
                intensity: None,
                classification: None,
            },
            ordinal: 0,
        }))
    }

    fn pick(studio: &mut Studio, xyz: Xyz) {
        let revision = studio.revision;
        act(studio, ViewAction::Picked(revision, picked(xyz)));
    }

    fn guid_of(studio: &Studio, name: &str) -> String {
        studio
            .views
            .list
            .iter()
            .find(|view| view.name == name)
            .unwrap_or_else(|| panic!("no view {name}"))
            .guid
            .clone()
    }

    fn close(a: Xyz, b: Xyz) -> bool {
        (0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-9)
    }

    #[test]
    fn views_are_named_in_turn_and_names_stay_unique_per_scan() {
        let (mut studio, _directory) = studio_with_scan();
        act(&mut studio, ViewAction::Save);
        act(&mut studio, ViewAction::Save);
        act(&mut studio, ViewAction::Name("  Entrance ".into()));
        act(&mut studio, ViewAction::Save);
        let names: Vec<&str> = studio
            .views
            .list
            .iter()
            .map(|view| view.name.as_str())
            .collect();
        assert_eq!(names, ["View 1", "View 2", "Entrance"]);
        assert!(studio.views.name.is_empty());
        assert_eq!(studio.status, "Saved view Entrance");
        assert_eq!(studio.active_view().unwrap().name, "Entrance");

        // A name in use is refused, whatever its case, and stays in the field.
        act(&mut studio, ViewAction::Name("entrance".into()));
        act(&mut studio, ViewAction::Save);
        assert_eq!(studio.views.list.len(), 3);
        assert_eq!(studio.views.name, "entrance");
        act(
            &mut studio,
            ViewAction::Name("x".repeat(MAX_NAME_CHARS + 1)),
        );
        act(&mut studio, ViewAction::Save);
        assert_eq!(studio.views.list.len(), 3);

        // A deleted number is used again.
        let first = guid_of(&studio, "View 1");
        act(&mut studio, ViewAction::Delete(first));
        act(&mut studio, ViewAction::Name(String::new()));
        act(&mut studio, ViewAction::Save);
        assert_eq!(studio.views.list.last().unwrap().name, "View 1");

        // What is on disk is what is in the list.
        assert_eq!(camera_views::load(), studio.views.list);
        for view in &studio.views.list {
            assert!(camera_views::is_guid(&view.guid));
            assert!(view.created > 0);
        }

        // Without a scan there is nothing to save a view of.
        let mut empty = Studio::default();
        assert_eq!(empty.views.list, studio.views.list);
        act(&mut empty, ViewAction::Save);
        assert_eq!(empty.status, "Open a scan before saving a view");
        assert_eq!(camera_views::load(), studio.views.list);
    }

    #[test]
    fn restoring_puts_back_camera_section_box_colour_mode_and_annotations() {
        let (mut studio, _directory) = studio_with_scan();
        let _ = studio.update(Message::Orbit(40.0, -25.0));
        let _ = studio.update(Message::Pan(18.0, -7.0));
        let size = studio.viewport_size;
        let _ = studio.update(Message::Zoom(2.0, [300.0, 200.0], size));
        let section = send(
            &mut studio,
            command(r#"{"command":"set_section","min":[1,0.5,0.25],"max":[3,2.5,1.5]}"#),
        );
        assert_eq!(section["ok"], true);
        let _ = studio.update(Message::ColorMode(ColorMode::Elevation));
        act(&mut studio, ViewAction::Name("Detail".into()));
        act(&mut studio, ViewAction::Save);
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        pick(&mut studio, [0.0, 0.0, 0.0]);
        pick(&mut studio, [4.0, 3.0, 0.0]);
        let saved = (studio.yaw, studio.pitch, studio.zoom, studio.pan);
        let saved_section = studio.section_bounds().unwrap();
        let view = studio.views.list[0].clone();
        assert_eq!(view.section.unwrap().min, saved_section.min);
        assert_eq!(view.section.unwrap().max, saved_section.max);
        assert!(view.section.unwrap().enabled);
        assert_eq!(view.color_mode, Some(ColorMode::Elevation));
        assert_eq!(view.frame.unwrap().viewport, [size.width, size.height]);
        assert!(view.walk.is_none());

        // A second view without a section box, in another colour mode.
        let _ = studio.update(Message::SetSectionEnabled(false));
        let _ = studio.update(Message::ColorMode(ColorMode::Rgb));
        let _ = studio.update(Message::ResetCamera);
        act(&mut studio, ViewAction::Name("Overview".into()));
        act(&mut studio, ViewAction::Save);
        assert_eq!(studio.active_view().unwrap().name, "Overview");
        assert!(studio.views_overlay().annotations.is_empty());
        let _ = studio.update(Message::ResetSectionBox);

        let revision = studio.revision;
        act(&mut studio, ViewAction::Restore(view.guid.clone()));
        assert_eq!((studio.yaw, studio.pitch, studio.zoom, studio.pan), saved);
        assert!(studio.section_enabled);
        let restored = studio.section_bounds().unwrap();
        assert!(close(restored.min, saved_section.min) && close(restored.max, saved_section.max));
        assert_eq!(studio.color_mode, ColorMode::Elevation);
        assert_eq!(studio.view_label, "SAVED VIEW");
        assert_eq!(studio.status, "Restored view Detail");
        assert!(studio.revision > revision);
        assert_eq!(studio.active_view().unwrap().guid, view.guid);
        assert_eq!(
            studio.views_overlay().annotations,
            [Annotation::Line {
                from: [0.0, 0.0, 0.0],
                to: [4.0, 3.0, 0.0],
            }]
        );

        // The other view switches the section box off again.
        let overview = guid_of(&studio, "Overview");
        act(&mut studio, ViewAction::Restore(overview));
        assert!(!studio.section_enabled);
        assert_eq!(studio.color_mode, ColorMode::Rgb);
        assert_eq!((studio.zoom, studio.pan), (1.0, [0.0, 0.0]));
        assert!(studio.views_overlay().annotations.is_empty());

        // In a viewport of half the size the picture and its pan are halved.
        let _ = studio.update(Message::ViewportSize(Size::new(
            size.width * 0.5,
            size.height * 0.5,
        )));
        act(&mut studio, ViewAction::Restore(view.guid.clone()));
        assert_eq!(
            (studio.yaw, studio.pitch, studio.zoom),
            (saved.0, saved.1, saved.2)
        );
        assert!((studio.pan[0] - saved.3[0] * 0.5).abs() < 1e-3);
        assert!((studio.pan[1] - saved.3[1] * 0.5).abs() < 1e-3);

        // Read back from disk, the view restores the same.
        let mut reopened = Studio {
            views: ViewTool::load(),
            ..Studio::default()
        };
        assert_eq!(reopened.views.list, studio.views.list);
        let cloud = Arc::clone(&studio.clouds[0].cloud);
        let _ = reopened.update(Message::Loaded(Ok(cloud)));
        act(&mut reopened, ViewAction::Restore(view.guid.clone()));
        assert_eq!(
            (reopened.yaw, reopened.pitch, reopened.zoom, reopened.pan),
            saved
        );
        let restored = reopened.section_bounds().unwrap();
        assert!(close(restored.min, saved_section.min) && close(restored.max, saved_section.max));
        assert_eq!(reopened.views_overlay().annotations.len(), 1);
    }

    #[test]
    fn a_view_saved_while_walking_restores_the_walking_camera() {
        let (mut studio, _directory) = studio_with_scan();
        let walked = send(
            &mut studio,
            command(r#"{"command":"walk","eye":[1.0,1.5,1.6],"yaw":0.7,"pitch":-0.2}"#),
        );
        assert_eq!(walked["ok"], true);
        let walking = studio.walk.unwrap();
        act(&mut studio, ViewAction::Name("Inside".into()));
        act(&mut studio, ViewAction::Save);
        let view = studio.views.list[0].clone();
        assert_eq!(view.walk.unwrap().eye, [1.0, 1.5, 1.6]);

        let _ = studio.update(Message::LeaveWalk);
        act(&mut studio, ViewAction::Save);
        assert!(studio.views.list[1].walk.is_none());

        act(&mut studio, ViewAction::Restore(view.guid.clone()));
        assert_eq!(studio.walk, Some(walking));
        // A view of the orbit camera leaves the walking camera.
        let orbit = studio.views.list[1].guid.clone();
        act(&mut studio, ViewAction::Restore(orbit));
        assert!(studio.walk.is_none());
    }

    #[test]
    fn annotations_go_to_the_active_view_and_save_the_view_first_when_there_is_none() {
        let (mut studio, directory) = studio_with_scan();
        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        assert_eq!(studio.views.tool, Some(AnnotationKind::Note));
        assert!(studio.active_view().is_none());

        // The point waits for its text; nothing is saved yet.
        pick(&mut studio, [4.0, 3.0, 0.0]);
        assert_eq!(studio.views.note_point, Some([4.0, 3.0, 0.0]));
        assert!(studio.note_prompt().is_some());
        assert!(studio.views.list.is_empty());
        // An empty text is not a note.
        act(&mut studio, ViewAction::NoteSubmit);
        assert!(studio.views.list.is_empty());
        assert!(studio.views.note_point.is_some());

        act(
            &mut studio,
            ViewAction::NoteText(" Scheur in de wand ".into()),
        );
        act(&mut studio, ViewAction::NoteSubmit);
        assert_eq!(studio.views.list.len(), 1);
        let view = studio.active_view().unwrap().clone();
        assert_eq!(view.name, "View 1");
        assert!(matches!(
            &view.annotations[..],
            [Annotation::Note { point, text, .. }]
                if *point == [4.0, 3.0, 0.0] && text == "Scheur in de wand"
        ));
        assert!(studio.views.note_point.is_none() && studio.views.note_text.is_empty());
        assert!(studio.note_prompt().is_none());
        assert_eq!(studio.views.tool, Some(AnnotationKind::Note));
        assert_eq!(studio.status, "Annotation 1 added to view View 1");

        // A line takes two points and goes to the same view.
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        pick(&mut studio, [0.0, 0.0, 0.0]);
        assert_eq!(studio.views.line_start, Some([0.0, 0.0, 0.0]));
        assert_eq!(studio.active_view().unwrap().annotations.len(), 1);
        // The same point twice is no line; the start stays.
        pick(&mut studio, [0.0, 0.0, 0.0]);
        assert_eq!(studio.views.line_start, Some([0.0, 0.0, 0.0]));
        pick(&mut studio, [0.0, 3.0, 2.0]);
        assert!(studio.views.line_start.is_none());
        assert_eq!(studio.views.list.len(), 1);
        assert_eq!(
            studio.active_view().unwrap().annotations[1],
            Annotation::Line {
                from: [0.0, 0.0, 0.0],
                to: [0.0, 3.0, 2.0],
            }
        );
        assert_eq!(studio.views_overlay().annotations.len(), 2);
        assert_eq!(camera_views::load(), studio.views.list);

        // A miss, a failed search and a stale answer add nothing.
        let revision = studio.revision;
        act(&mut studio, ViewAction::Picked(revision, Ok(None)));
        assert_eq!(studio.status, "No point within 8 pixels");
        act(
            &mut studio,
            ViewAction::Picked(revision, Err("gone".into())),
        );
        act(
            &mut studio,
            ViewAction::Picked(revision + 1, picked([1.0; 3])),
        );
        assert!(studio.views.line_start.is_none());

        // Deleting one annotation leaves the other.
        act(&mut studio, ViewAction::DeleteAnnotation(0));
        assert!(matches!(
            &studio.active_view().unwrap().annotations[..],
            [Annotation::Line { .. }]
        ));
        act(&mut studio, ViewAction::DeleteAnnotation(5));
        assert_eq!(studio.active_view().unwrap().annotations.len(), 1);
        assert_eq!(camera_views::load(), studio.views.list);

        // Hidden annotations are not drawn; the next one starts a new view.
        act(&mut studio, ViewAction::Deactivate);
        assert!(studio.views_overlay().annotations.is_empty());
        pick(&mut studio, [4.0, 0.0, 0.0]);
        pick(&mut studio, [4.0, 3.0, 0.0]);
        assert_eq!(studio.views.list.len(), 2);
        assert_eq!(studio.active_view().unwrap().name, "View 2");
        assert_eq!(studio.views.list[0].annotations.len(), 1);
        assert_eq!(studio.views.list[1].annotations.len(), 1);

        // Annotations are shown for the scan their view belongs to.
        let other = directory.path().join("other.xyz");
        std::fs::write(&other, "0 0 0\n1 1 1\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&other, 10).unwrap());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        assert_eq!(studio.active, Some(1));
        assert!(studio.views_overlay().annotations.is_empty());
        assert!(studio.active_view().is_none());
        let _ = studio.update(Message::Select(0));
        assert_eq!(studio.views_overlay().annotations.len(), 1);
    }

    #[test]
    fn escape_cancels_a_half_placed_annotation_before_it_leaves_the_tool() {
        let (mut studio, _directory) = studio_with_scan();
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        pick(&mut studio, [0.0, 0.0, 0.0]);
        let _ = studio.update(Message::Escape);
        assert!(studio.views.line_start.is_none());
        assert_eq!(studio.views.tool, Some(AnnotationKind::Line));
        assert!(studio.views.list.is_empty());
        let _ = studio.update(Message::Escape);
        assert_eq!(studio.views.tool, None);
        assert_eq!(
            studio.status,
            "Annotation tool closed; orbit and right-click menu available"
        );

        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        pick(&mut studio, [4.0, 0.0, 0.0]);
        act(&mut studio, ViewAction::NoteText("half".into()));
        let _ = studio.update(Message::Escape);
        assert!(studio.views.note_point.is_none() && studio.views.note_text.is_empty());
        assert_eq!(studio.views.tool, Some(AnnotationKind::Note));
        assert!(studio.views.list.is_empty());

        // A pick that was still searching is cancelled with the tool.
        let revision = studio.revision;
        studio.selection_pending = true;
        studio.selection_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _ = studio.update(Message::Escape);
        act(&mut studio, ViewAction::Picked(revision, picked([1.0; 3])));
        assert!(studio.views.note_point.is_none());
        assert_eq!(studio.status, "Annotation pick cancelled");
        assert!(!studio.selection_pending);

        // Cancel in the note field does the same as Escape.
        studio.selection_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        pick(&mut studio, [4.0, 0.0, 0.0]);
        assert!(studio.views.note_point.is_some());
        act(&mut studio, ViewAction::CancelPlacing);
        assert!(studio.views.note_point.is_none());

        // Escape also ends a rename before anything else.
        act(&mut studio, ViewAction::Save);
        let guid = studio.views.list[0].guid.clone();
        act(&mut studio, ViewAction::StartRename(guid));
        let _ = studio.update(Message::Escape);
        assert!(studio.views.renaming.is_none());
        assert_eq!(studio.views.tool, Some(AnnotationKind::Note));
    }

    #[test]
    fn annotation_tools_measuring_and_selection_tools_exclude_each_other() {
        use crate::measure::{MeasureAction, MeasureMode};

        let (mut studio, _directory) = studio_with_scan();
        let _ = studio.update(Message::ToggleBoxSelect);
        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        assert_eq!(studio.views.tool, Some(AnnotationKind::Note));
        assert!(!studio.box_select && !studio.pick_mode);

        let _ = studio.update(Message::TogglePickSelect);
        assert!(studio.pick_mode);
        assert_eq!(studio.views.tool, None);

        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        assert!(!studio.pick_mode);
        let _ = studio.update(Message::ToggleBoxSelect);
        assert!(studio.box_select);
        assert_eq!(studio.views.tool, None);

        // Measuring and annotating replace each other.
        let _ = studio.update(Message::Measure(MeasureAction::Toggle(
            MeasureMode::Distance,
        )));
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        assert_eq!(studio.measure.mode, None);
        assert_eq!(studio.views.tool, Some(AnnotationKind::Line));
        pick(&mut studio, [0.0, 0.0, 0.0]);
        let _ = studio.update(Message::Measure(MeasureAction::Toggle(MeasureMode::Area)));
        assert_eq!(studio.measure.mode, Some(MeasureMode::Area));
        assert_eq!(studio.views.tool, None);
        assert!(studio.views.line_start.is_none());

        // The two annotation tools switch, and the active one closes itself.
        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        assert_eq!(studio.views.tool, Some(AnnotationKind::Line));
        assert_eq!(studio.measure.mode, None);
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        assert_eq!(studio.views.tool, None);

        for action in [
            crate::ContextAction::Orbit,
            crate::ContextAction::BoxSelect,
            crate::ContextAction::PickPoint,
        ] {
            act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
            let _ = studio.update(Message::ContextAction(action));
            assert_eq!(studio.views.tool, None);
        }

        // Without a scan there is nothing to annotate.
        let mut empty = Studio::default();
        act(&mut empty, ViewAction::Tool(AnnotationKind::Note));
        assert_eq!(empty.views.tool, None);
    }

    #[test]
    fn a_click_with_an_annotation_tool_picks_and_a_drag_orbits() {
        let start = UiPoint::new(100.0, 80.0);
        let size = Size::new(800.0, 600.0);
        let pending = DragState {
            start,
            position: start,
            mode: DragMode::AnnotatePending,
        };
        let click = finish_viewport_drag(
            iced::mouse::Button::Left,
            pending,
            UiPoint::new(102.0, 81.0),
            size,
        );
        assert!(matches!(
            click,
            Some(Message::Views(ViewAction::Click([102.0, 81.0], _)))
        ));
        let drag = finish_viewport_drag(
            iced::mouse::Button::Left,
            pending,
            UiPoint::new(130.0, 70.0),
            size,
        );
        assert!(matches!(drag, Some(Message::FinishOrbit(30.0, -10.0))));

        // The answer of the pick search goes to the annotation.
        assert!(matches!(
            picked_message(Message::PickReady(7, 0, picked([1.0; 3]))),
            Message::Views(ViewAction::Picked(7, Ok(Some(_))))
        ));
        assert!(matches!(picked_message(Message::Escape), Message::Escape));

        // Without a tool a click does nothing; with one it starts the search.
        let (mut studio, _directory) = studio_with_scan();
        let size = studio.viewport_size;
        act(&mut studio, ViewAction::Click([10.0, 10.0], size));
        assert!(!studio.selection_pending);
        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        act(&mut studio, ViewAction::Click([10.0, 10.0], size));
        assert!(studio.selection_pending);
    }

    #[test]
    fn viewport_press_with_an_annotation_tool_clicks_in_the_orbit_view_and_while_walking() {
        use iced::mouse::{self, Cursor};
        use iced::widget::canvas::{Event, Program};

        let (mut studio, _directory) = studio_with_scan();
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        let bounds = Rectangle::new(UiPoint::ORIGIN, studio.viewport_size);
        let at = |x: f32, y: f32| Cursor::Available(UiPoint::new(x, y));
        let moved = |x: f32, y: f32| {
            Event::Mouse(mouse::Event::CursorMoved {
                position: UiPoint::new(x, y),
            })
        };
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        {
            let viewport = studio.point_viewport();
            let mut state = crate::ViewportState::default();
            let _ = viewport.update(&mut state, press.clone(), bounds, at(300.0, 400.0));
            let (_, message) =
                viewport.update(&mut state, moved(302.0, 401.0), bounds, at(302.0, 401.0));
            assert!(message.is_none());
            let (_, message) =
                viewport.update(&mut state, release.clone(), bounds, at(302.0, 401.0));
            assert!(matches!(
                message,
                Some(Message::Views(ViewAction::Click([302.0, 401.0], _)))
            ));
            // Further than a few pixels the gesture orbits.
            let _ = viewport.update(&mut state, press.clone(), bounds, at(300.0, 400.0));
            let (_, message) =
                viewport.update(&mut state, moved(320.0, 410.0), bounds, at(320.0, 410.0));
            assert!(matches!(message, Some(Message::Orbit(20.0, 10.0))));
        }

        let walked = send(
            &mut studio,
            command(r#"{"command":"walk","eye":[1.0,1.5,1.6],"yaw":0.7,"pitch":-0.2}"#),
        );
        assert_eq!(walked["ok"], true);
        let viewport = studio.point_viewport();
        let mut state = crate::ViewportState::default();
        let _ = viewport.update(&mut state, press.clone(), bounds, at(300.0, 400.0));
        let (_, message) = viewport.update(&mut state, release.clone(), bounds, at(301.0, 400.0));
        assert!(matches!(
            message,
            Some(Message::Views(ViewAction::Click([301.0, 400.0], _)))
        ));
        // A drag looks around and picks nothing.
        let _ = viewport.update(&mut state, press, bounds, at(300.0, 400.0));
        let (_, message) =
            viewport.update(&mut state, moved(340.0, 400.0), bounds, at(340.0, 400.0));
        assert!(matches!(message, Some(Message::WalkLook(40.0, 0.0))));
        let (_, message) = viewport.update(&mut state, release, bounds, at(340.0, 400.0));
        assert!(message.is_none());
    }

    #[test]
    fn views_are_renamed_updated_and_deleted_with_their_snapshot() {
        let (mut studio, _directory) = studio_with_scan();
        act(&mut studio, ViewAction::Save);
        act(&mut studio, ViewAction::Save);
        let guid = guid_of(&studio, "View 1");
        let created = studio.views.list[0].created;
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        act(&mut studio, ViewAction::Restore(guid.clone()));
        pick(&mut studio, [0.0, 0.0, 0.0]);
        pick(&mut studio, [4.0, 0.0, 0.0]);

        // Rename: the field starts with the name; a name in use is refused.
        act(&mut studio, ViewAction::StartRename(guid.clone()));
        assert_eq!(
            studio.views.renaming,
            Some((guid.clone(), "View 1".to_owned()))
        );
        let _ = studio.view();
        act(&mut studio, ViewAction::RenameText("view 2".into()));
        act(&mut studio, ViewAction::FinishRename);
        assert!(studio.views.renaming.is_some());
        assert_eq!(studio.views.list[0].name, "View 1");
        act(&mut studio, ViewAction::RenameText(" Gevel noord ".into()));
        act(&mut studio, ViewAction::FinishRename);
        assert!(studio.views.renaming.is_none());
        assert_eq!(studio.views.list[0].name, "Gevel noord");
        assert_eq!(studio.status, "Renamed view View 1 to Gevel noord");
        // Its own name in another case is allowed.
        act(&mut studio, ViewAction::StartRename(guid.clone()));
        act(&mut studio, ViewAction::RenameText("GEVEL NOORD".into()));
        act(&mut studio, ViewAction::FinishRename);
        assert_eq!(studio.views.list[0].name, "GEVEL NOORD");
        act(&mut studio, ViewAction::StartRename(guid.clone()));
        act(&mut studio, ViewAction::CancelRename);
        assert!(studio.views.renaming.is_none());

        // Update: the camera changes; name, identifier, time and
        // annotations stay.
        let _ = studio.update(Message::Orbit(60.0, 10.0));
        let _ = studio.update(Message::ColorMode(ColorMode::Intensity));
        let other = studio.views.list[1].guid.clone();
        act(&mut studio, ViewAction::Restore(other));
        let _ = studio.update(Message::Orbit(-30.0, 5.0));
        let camera = (studio.yaw, studio.pitch);
        act(&mut studio, ViewAction::Update(guid.clone()));
        let updated = &studio.views.list[0];
        assert_eq!((updated.yaw, updated.pitch), camera);
        assert_eq!(updated.name, "GEVEL NOORD");
        assert_eq!(
            (updated.guid.as_str(), updated.created),
            (guid.as_str(), created)
        );
        assert_eq!(updated.annotations.len(), 1);
        assert_eq!(studio.active_view().unwrap().guid, guid);
        assert_eq!(camera_views::load(), studio.views.list);

        // Delete: the view, its snapshot and its place as the active view go.
        camera_views::write_snapshot(&guid, b"image").unwrap();
        act(&mut studio, ViewAction::Delete(guid.clone()));
        assert_eq!(studio.views.list.len(), 1);
        assert!(camera_views::read_snapshot(&guid).is_none());
        assert!(studio.active_view().is_none());
        assert!(!studio.views.snapshots.contains_key(&guid));
        assert_eq!(camera_views::load(), studio.views.list);
        // An unknown identifier changes nothing.
        act(&mut studio, ViewAction::Delete(guid.clone()));
        act(&mut studio, ViewAction::Restore(guid));
        assert_eq!(studio.views.list.len(), 1);
    }

    /// Draw the viewport: its canvas lies in the window with this size.
    fn draw(studio: &Studio, size: Size) {
        studio
            .views_overlay()
            .drawn_at(Rectangle::new(UiPoint::new(280.0, 150.0), size));
    }

    /// The moment the newest snapshot request of a view comes up.
    fn capture(studio: &mut Studio, guid: &str) {
        let serial = studio.views.snapshots[guid];
        act(
            studio,
            ViewAction::Capture {
                guid: guid.to_owned(),
                serial,
                tries: 0,
            },
        );
    }

    /// A screenshot of a window that the canvas of `draw` lies inside.
    fn screenshot() -> Screenshot {
        Screenshot::new(vec![80u8; 1400 * 900 * 4], Size::new(1400, 900), 1.0)
    }

    /// Take the snapshot a view waits for, as the application does: the
    /// request comes up, the window is captured and the image is written.
    fn take_snapshot(studio: &mut Studio, guid: &str, image: &[u8]) {
        let serial = studio.views.snapshots[guid];
        capture(studio, guid);
        act(
            studio,
            ViewAction::Captured(guid.to_owned(), serial, Some(screenshot())),
        );
        assert_eq!(studio.views.snapshots.get(guid), Some(&serial));
        camera_views::write_snapshot(guid, image).unwrap();
        act(
            studio,
            ViewAction::SnapshotSaved(guid.to_owned(), serial, Ok(())),
        );
    }

    #[test]
    fn a_snapshot_is_asked_for_after_each_change_and_only_the_newest_request_counts() {
        let (mut studio, _directory) = studio_with_scan();
        act(&mut studio, ViewAction::Save);
        let guid = studio.views.list[0].guid.clone();
        assert!(studio.views.list[0].snapshot_due);
        let first = studio.views.snapshots[&guid];
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        pick(&mut studio, [0.0, 0.0, 0.0]);
        pick(&mut studio, [4.0, 0.0, 0.0]);
        let second = studio.views.snapshots[&guid];
        assert!(second > first);

        // The older request is ignored and leaves the newer one waiting.
        act(
            &mut studio,
            ViewAction::Capture {
                guid: guid.clone(),
                serial: first,
                tries: 0,
            },
        );
        assert_eq!(studio.views.snapshots.get(&guid), Some(&second));
        // Before the canvas has been drawn there is nothing to crop.
        capture(&mut studio, &guid);
        assert!(studio.views.snapshots.is_empty());
        assert!(studio.views.list[0].snapshot_due);

        // A request that still shows its view goes on to the screenshot,
        // once a half-placed annotation is out of the picture.
        draw(&studio, Size::new(900.0, 700.0));
        act(&mut studio, ViewAction::Update(guid.clone()));
        let third = studio.views.snapshots[&guid];
        pick(&mut studio, [0.0, 3.0, 2.0]);
        capture(&mut studio, &guid);
        assert_eq!(studio.views.snapshots.get(&guid), Some(&third));
        act(&mut studio, ViewAction::CancelPlacing);
        capture(&mut studio, &guid);
        assert_eq!(studio.views.snapshots.get(&guid), Some(&third));
        // Neither with the File view over the viewport.
        let _ = studio.update(Message::ToggleFile);
        capture(&mut studio, &guid);
        assert!(studio.views.snapshots.is_empty());
        let _ = studio.update(Message::ToggleFile);

        // Without a screenshot the view stays as it is and nothing waits.
        act(&mut studio, ViewAction::Update(guid.clone()));
        let fourth = studio.views.snapshots[&guid];
        act(
            &mut studio,
            ViewAction::Captured(guid.clone(), fourth, None),
        );
        assert!(studio.views.snapshots.is_empty());
        assert_eq!(studio.views.list.len(), 1);
        assert_eq!(
            studio.status,
            "The view is saved; its snapshot could not be taken"
        );

        // A failed write is reported; the view is still there and its
        // snapshot is still due.
        act(&mut studio, ViewAction::Update(guid.clone()));
        let fifth = studio.views.snapshots[&guid];
        act(
            &mut studio,
            ViewAction::SnapshotSaved(guid.clone(), fifth, Err("disk full".into())),
        );
        assert!(studio.views.snapshots.is_empty());
        assert_eq!(
            studio.status,
            "The view is saved; its snapshot failed: disk full"
        );
        assert_eq!(studio.views.list.len(), 1);
        assert!(studio.views.list[0].snapshot_due);

        // A snapshot written for an older request leaves the view due; the
        // one of the newest request settles it, also on disk.
        act(&mut studio, ViewAction::Update(guid.clone()));
        let sixth = studio.views.snapshots[&guid];
        act(
            &mut studio,
            ViewAction::SnapshotSaved(guid.clone(), fifth, Ok(())),
        );
        assert!(studio.views.list[0].snapshot_due);
        assert_eq!(studio.views.snapshots.get(&guid), Some(&sixth));
        act(
            &mut studio,
            ViewAction::SnapshotSaved(guid.clone(), sixth, Ok(())),
        );
        assert!(!studio.views.list[0].snapshot_due);
        assert!(studio.views.snapshots.is_empty());
        assert_eq!(camera_views::load(), studio.views.list);

        // A snapshot that arrives for a deleted view is removed again.
        camera_views::write_snapshot(&guid, b"late").unwrap();
        studio.views.list.clear();
        act(
            &mut studio,
            ViewAction::SnapshotSaved(guid.clone(), sixth, Ok(())),
        );
        assert!(camera_views::read_snapshot(&guid).is_none());
    }

    #[test]
    fn an_annotation_placed_after_the_camera_moved_leaves_the_snapshot_until_the_view_is_restored()
    {
        let (mut studio, _directory) = studio_with_scan();
        draw(&studio, Size::new(900.0, 700.0));
        act(&mut studio, ViewAction::Save);
        let guid = studio.views.list[0].guid.clone();
        take_snapshot(&mut studio, &guid, b"the view as saved");
        assert!(!studio.views.list[0].snapshot_due);
        assert!(studio.views.snapshots.is_empty());
        let saved = studio.views.list[0].clone();
        let on_disk = |guid: &str| camera_views::read_snapshot(guid).unwrap();

        // The camera moves away, as it does to reach a point, and a line is
        // placed. The view keeps its camera, so it keeps its snapshot.
        let moved = send(
            &mut studio,
            command(r#"{"command":"set_camera","yaw":1.9,"pitch":0.4,"zoom":4,"pan":[120,-60]}"#),
        );
        assert_eq!(moved["ok"], true);
        assert!(!studio.shows_view(&guid));
        let line = send(
            &mut studio,
            command(r#"{"command":"add_line","from":[0,0,0],"to":[4,0,0]}"#),
        );
        assert_eq!(line["ok"], true);
        assert_eq!(
            studio.status,
            "Annotation 1 added to view View 1; its snapshot is renewed when the view is restored"
        );
        let view = &studio.views.list[0];
        assert_eq!(
            (view.yaw, view.pitch, view.zoom, view.pan),
            (saved.yaw, saved.pitch, saved.zoom, saved.pan)
        );
        assert_eq!(view.annotations.len(), 1);
        assert!(view.snapshot_due);
        assert!(studio.views.snapshots.is_empty());
        assert_eq!(on_disk(&guid), b"the view as saved");
        assert_eq!(camera_views::load(), studio.views.list);

        // Removing an annotation with the camera elsewhere does the same.
        act(&mut studio, ViewAction::DeleteAnnotation(0));
        assert_eq!(
            studio.status,
            "Annotation removed; its snapshot is renewed when the view is restored"
        );
        assert!(studio.views.list[0].snapshot_due && studio.views.snapshots.is_empty());
        let _ = send(
            &mut studio,
            command(r#"{"command":"add_line","from":[0,0,0],"to":[4,0,0]}"#),
        );

        // A request that is still on its way takes nothing either, neither
        // when it comes up nor when its screenshot arrives.
        studio.views.snapshots.insert(guid.clone(), 900);
        capture(&mut studio, &guid);
        assert!(studio.views.snapshots.is_empty());
        studio.views.snapshots.insert(guid.clone(), 901);
        act(
            &mut studio,
            ViewAction::Captured(guid.clone(), 901, Some(screenshot())),
        );
        assert!(studio.views.snapshots.is_empty());
        assert!(studio.views.list[0].snapshot_due);
        assert_eq!(on_disk(&guid), b"the view as saved");

        // Restored, the viewport shows the view again and the snapshot that
        // was due is taken, with the line.
        act(&mut studio, ViewAction::Restore(guid.clone()));
        assert!(studio.shows_view(&guid));
        assert!(studio.views.snapshots.contains_key(&guid));
        take_snapshot(&mut studio, &guid, b"the view with its line");
        assert!(!studio.views.list[0].snapshot_due);
        assert_eq!(camera_views::load(), studio.views.list);
        // Restoring a view whose snapshot is in order asks for nothing.
        act(&mut studio, ViewAction::Restore(guid.clone()));
        assert!(studio.views.snapshots.is_empty());

        // With the viewport on the view an annotation renews the snapshot.
        let note = send(
            &mut studio,
            command(r#"{"command":"add_note","point":[4,3,0],"text":"Kozijn"}"#),
        );
        assert_eq!(note["ok"], true);
        assert_eq!(studio.status, "Annotation 2 added to view View 1");
        take_snapshot(&mut studio, &guid, b"the view with line and note");

        // Every part of what a view holds counts: after a change of the
        // camera, the walking camera, the section box or the colour mode a
        // request takes nothing.
        let changes: [&dyn Fn(&mut Studio); 5] = [
            &|studio| {
                let _ = studio.update(Message::Orbit(25.0, 0.0));
            },
            &|studio| {
                let size = studio.viewport_size;
                let _ = studio.update(Message::Zoom(1.5, [300.0, 200.0], size));
            },
            &|studio| {
                let walked = send(
                    studio,
                    command(r#"{"command":"walk","eye":[1.0,1.5,1.6],"yaw":0.7,"pitch":-0.2}"#),
                );
                assert_eq!(walked["ok"], true);
            },
            &|studio| {
                let section = send(
                    studio,
                    command(r#"{"command":"set_section","min":[1,0.5,0.25],"max":[3,2.5,1.5]}"#),
                );
                assert_eq!(section["ok"], true);
            },
            &|studio| {
                let _ = studio.update(Message::ColorMode(ColorMode::Elevation));
            },
        ];
        for change in changes {
            act(&mut studio, ViewAction::Restore(guid.clone()));
            assert!(studio.shows_view(&guid));
            act(&mut studio, ViewAction::DeleteAnnotation(1));
            assert!(studio.views.snapshots.contains_key(&guid));
            change(&mut studio);
            assert!(!studio.shows_view(&guid));
            capture(&mut studio, &guid);
            assert!(studio.views.snapshots.is_empty());
            assert!(studio.views.list[0].snapshot_due);
            let _ = send(
                &mut studio,
                command(r#"{"command":"add_note","point":[4,3,0],"text":"Kozijn"}"#),
            );
            assert!(studio.views.snapshots.is_empty());
            assert_eq!(on_disk(&guid), b"the view with line and note");
        }

        // A view without a snapshot gets one when it is restored, and hidden
        // annotations are not what a snapshot shows.
        act(&mut studio, ViewAction::Restore(guid.clone()));
        take_snapshot(&mut studio, &guid, b"again");
        camera_views::remove_snapshot(&guid);
        act(&mut studio, ViewAction::Restore(guid.clone()));
        assert!(studio.views.snapshots.contains_key(&guid));
        act(&mut studio, ViewAction::Deactivate);
        assert!(!studio.shows_view(&guid));
        capture(&mut studio, &guid);
        assert!(studio.views.snapshots.is_empty());
        assert!(camera_views::read_snapshot(&guid).is_none());
    }

    #[test]
    fn a_restore_that_cannot_show_what_the_view_holds_takes_no_snapshot() {
        let (mut studio, directory) = studio_with_scan();
        draw(&studio, Size::new(900.0, 700.0));
        let _ = send(
            &mut studio,
            command(r#"{"command":"set_section","min":[1,0.5,0.25],"max":[3,2.5,1.5]}"#),
        );
        act(&mut studio, ViewAction::Save);
        let guid = studio.views.list[0].guid.clone();
        assert!(studio.holds_what_view_holds(&studio.views.list[0]));

        // The view of a section box that lies outside the scene of now.
        studio.views.list[0].section = Some(SectionBox {
            enabled: true,
            min: [40.0, 40.0, 40.0],
            max: [50.0, 50.0, 50.0],
        });
        act(&mut studio, ViewAction::Restore(guid.clone()));
        assert_eq!(studio.active_view().unwrap().guid, guid);
        assert!(!studio.shows_view(&guid));
        // The request left from saving takes nothing of this picture.
        capture(&mut studio, &guid);
        assert!(studio.views.snapshots.is_empty());

        // The scene grew by another scan: the camera is moved to show what
        // it showed, and the view is framed anew with its snapshot.
        studio.views.list[0].section = None;
        let before = studio.views.list[0].clone();
        let other = directory.path().join("other.xyz");
        std::fs::write(&other, "0 0 0\n9 9 9\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&other, 10).unwrap());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let _ = studio.update(Message::Select(0));
        assert!(!studio.shows_view(&guid));
        act(&mut studio, ViewAction::Restore(guid.clone()));
        assert!(studio.shows_view(&guid));
        take_snapshot(&mut studio, &guid, b"in the larger scene");
        let after = &studio.views.list[0];
        assert_ne!(after.frame, before.frame);
        assert_eq!(after.frame.unwrap().scene_max, [9.0, 9.0, 9.0]);
        assert_eq!((after.zoom, after.pan), (studio.zoom, studio.pan));
        assert_ne!(after.zoom, before.zoom);
        assert_eq!(camera_views::load(), studio.views.list);
    }

    #[test]
    fn a_snapshot_taken_in_a_viewport_of_another_size_brings_the_frame_of_its_view_along() {
        let (mut studio, _directory) = studio_with_scan();
        let saved_size = Size::new(800.0, 600.0);
        draw(&studio, saved_size);
        let _ = studio.update(Message::Pan(60.0, -40.0));
        act(&mut studio, ViewAction::Name("Orbit".into()));
        act(&mut studio, ViewAction::Save);
        let walked = send(
            &mut studio,
            command(r#"{"command":"walk","eye":[1.0,1.5,1.6],"yaw":0.7,"pitch":-0.2}"#),
        );
        assert_eq!(walked["ok"], true);
        act(&mut studio, ViewAction::Name("Inside".into()));
        act(&mut studio, ViewAction::Save);
        let (orbit, inside) = (guid_of(&studio, "Orbit"), guid_of(&studio, "Inside"));
        let saved = studio.views.list.clone();
        assert_eq!(saved[1].frame.unwrap().viewport, [800.0, 600.0]);
        let scene = combined_bounds(&studio.clouds).unwrap();
        let field_of_view =
            |view: &SavedView| bcf::camera(view, scene, Size::new(10.0, 10.0)).field_of_view;
        let upright = |walk: WalkCamera, size: Size| {
            let focal = 0.5 * f64::from(size.width) / (f64::from(walk.field_of_view) * 0.5).tan();
            (2.0 * (0.5 * f64::from(size.height) / focal).atan()).to_degrees()
        };
        assert!(
            (field_of_view(&saved[1]) - upright(saved[1].walk.unwrap(), saved_size)).abs() < 1e-4
        );

        // The window is made wide and low. The walking view restored there
        // shows less height, and the camera written with its new snapshot
        // says so.
        let wide = Size::new(1000.0, 400.0);
        draw(&studio, wide);
        act(&mut studio, ViewAction::Restore(inside.clone()));
        // Restoring alone leaves the view as it is stored.
        assert_eq!(studio.views.list, saved);
        take_snapshot(&mut studio, &inside, b"wide");
        let view = studio.views.list[1].clone();
        assert_eq!(view.frame.unwrap().viewport, [1000.0, 400.0]);
        assert_eq!(view.walk, saved[1].walk);
        assert!((field_of_view(&view) - upright(view.walk.unwrap(), wide)).abs() < 1e-4);
        assert!(field_of_view(&view) < field_of_view(&saved[1]) - 10.0);

        // The orbit view restored in a viewport of half the size is kept
        // with its pan in that viewport, and restores the same from there.
        let half = Size::new(400.0, 300.0);
        draw(&studio, half);
        act(&mut studio, ViewAction::Restore(orbit.clone()));
        assert!(studio.walk.is_none());
        take_snapshot(&mut studio, &orbit, b"half");
        let view = studio.views.list[0].clone();
        assert_eq!(view.frame.unwrap().viewport, [400.0, 300.0]);
        assert_eq!(view.pan, [saved[0].pan[0] * 0.5, saved[0].pan[1] * 0.5]);
        assert_eq!(
            (view.yaw, view.pitch, view.zoom),
            (saved[0].yaw, saved[0].pitch, saved[0].zoom)
        );
        assert_eq!(camera_views::load(), studio.views.list);
        draw(&studio, saved_size);
        act(&mut studio, ViewAction::Restore(orbit.clone()));
        assert_eq!(studio.pan, saved[0].pan);

        // The viewport changes size under the view that is shown, as it
        // does with the window or with a status text of more lines. The
        // snapshot waits one turn, in which the pan follows the picture as
        // it does on restoring, and is then taken in the new size.
        let stored = studio.views.list.clone();
        let (yaw, zoom, revision) = (studio.yaw, studio.zoom, studio.revision);
        let low = Size::new(800.0, 450.0);
        draw(&studio, low);
        assert!(studio.shows_view(&orbit));
        act(&mut studio, ViewAction::Update(orbit.clone()));
        assert_eq!(studio.views.list[0].frame.unwrap().viewport, [800.0, 450.0]);
        assert_eq!(studio.views.list[0].pan, saved[0].pan);
        draw(&studio, saved_size);
        let serial = studio.views.snapshots[&orbit];
        capture(&mut studio, &orbit);
        assert_eq!(studio.views.snapshots.get(&orbit), Some(&serial));
        let grown = 600.0 / 450.0;
        assert_eq!(
            studio.pan,
            [saved[0].pan[0] * grown, saved[0].pan[1] * grown]
        );
        assert_eq!((studio.yaw, studio.zoom), (yaw, zoom));
        assert!(studio.revision > revision);
        assert!(studio.shows_view(&orbit));
        // Until that turn the view is stored as it was.
        assert_eq!(studio.views.list[0].frame.unwrap().viewport, [800.0, 450.0]);
        take_snapshot(&mut studio, &orbit, b"followed");
        assert_eq!(studio.views.list[0].frame.unwrap().viewport, [800.0, 600.0]);
        assert_eq!(studio.views.list[0].pan, studio.pan);
        assert!(!studio.views.list[0].snapshot_due);
        // A screenshot of a viewport that changed size after the request
        // came up is not used.
        act(&mut studio, ViewAction::Update(orbit.clone()));
        let serial = studio.views.snapshots[&orbit];
        capture(&mut studio, &orbit);
        draw(&studio, low);
        act(
            &mut studio,
            ViewAction::Captured(orbit.clone(), serial, Some(screenshot())),
        );
        assert!(studio.views.snapshots.is_empty());
        assert!(studio.views.list[0].snapshot_due);
        assert_eq!(studio.views.list[0].frame.unwrap().viewport, [800.0, 600.0]);
        draw(&studio, saved_size);
        act(&mut studio, ViewAction::Restore(orbit.clone()));
        take_snapshot(&mut studio, &orbit, b"restored");

        // Back in the viewport it was saved in, the view that was kept in
        // the half-sized viewport is stored as it was saved. A snapshot of
        // one view leaves the other alone.
        studio.views.list[0] = stored[0].clone();
        act(&mut studio, ViewAction::Restore(orbit.clone()));
        let _ = send(
            &mut studio,
            command(r#"{"command":"add_line","from":[0,0,0],"to":[4,0,0]}"#),
        );
        take_snapshot(&mut studio, &orbit, b"again");
        assert_eq!(studio.views.list[1], stored[1]);
        assert_eq!(studio.views.list[0].frame.unwrap().viewport, [800.0, 600.0]);
        assert_eq!(studio.views.list[0].pan, saved[0].pan);
    }

    #[test]
    fn a_half_placed_annotation_does_not_cross_to_another_scan() {
        let (mut studio, directory) = studio_with_scan();
        let other = directory.path().join("other.xyz");
        std::fs::write(&other, "0 0 0\n1 1 1\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&other, 10).unwrap());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let _ = studio.update(Message::Select(0));

        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        pick(&mut studio, [0.0, 0.0, 0.0]);
        assert!(studio.views.line_start.is_some());
        // Another message leaves the start where it is.
        let _ = studio.update(Message::Orbit(5.0, 0.0));
        assert!(studio.views.line_start.is_some());

        // Another scan becomes the active one: the start is dropped, the
        // tool stays for that scan.
        let _ = studio.update(Message::Select(1));
        assert!(studio.views.line_start.is_none());
        assert_eq!(studio.views.tool, Some(AnnotationKind::Line));
        assert_eq!(
            studio.status,
            "Annotation cancelled because another scan became the active one"
        );
        pick(&mut studio, [1.0, 1.0, 1.0]);
        pick(&mut studio, [0.0, 0.0, 0.0]);
        let view = studio.active_view().unwrap();
        assert_eq!(view.source, studio.active_camera_source().unwrap());
        assert_eq!(
            view.annotations,
            [Annotation::Line {
                from: [1.0, 1.0, 1.0],
                to: [0.0, 0.0, 0.0],
            }]
        );
        assert_eq!(studio.views.list.len(), 1);

        // A note that waits for its text goes the same way.
        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        pick(&mut studio, [1.0, 1.0, 1.0]);
        act(&mut studio, ViewAction::NoteText("half".into()));
        assert!(studio.note_prompt().is_some());
        let _ = studio.update(Message::Select(0));
        assert!(studio.views.note_point.is_none() && studio.views.note_text.is_empty());
        assert!(studio.note_prompt().is_none());
        assert_eq!(studio.views.tool, Some(AnnotationKind::Note));

        // Without a scan there is no tool and nothing half placed.
        pick(&mut studio, [0.0, 0.0, 0.0]);
        assert!(studio.note_prompt().is_some());
        let _ = studio.update(Message::Remove(1));
        let _ = studio.update(Message::Remove(0));
        assert!(studio.clouds.is_empty());
        assert_eq!(studio.views.tool, None);
        assert!(studio.views.note_point.is_none());
        assert!(studio.note_prompt().is_none());
    }

    #[test]
    fn labels_of_notes_stay_inside_the_viewport_and_clear_of_each_other() {
        let size = Size::new(800.0, 600.0);
        let at = |x: f32, y: f32| UiPoint::new(x, y);
        // To the upper right of its point.
        let label = label_place(at(100.0, 300.0), 120.0, size, &[]);
        assert_eq!((label.x, label.y), (118.0, 260.0));
        assert_eq!((label.width, label.height), (120.0, LABEL_HEIGHT));
        // To the left near the right edge, and below near the top.
        let label = label_place(at(760.0, 20.0), 120.0, size, &[]);
        assert_eq!((label.x, label.y), (622.0, 42.0));
        // Wider than the room on either side, it ends at the right edge.
        let label = label_place(at(400.0, 300.0), 500.0, size, &[]);
        assert_eq!(label.x + label.width, size.width);
        // Wider than the viewport, it starts at the left edge.
        assert_eq!(label_place(at(400.0, 300.0), 900.0, size, &[]).x, 0.0);

        // Two notes close together: the second label moves up, the third
        // further; near the top they move down instead.
        let mut placed = Vec::new();
        for (x, y) in [(300.0, 300.0), (310.0, 304.0), (295.0, 310.0)] {
            let label = label_place(at(x, y), 150.0, size, &placed);
            assert!(!placed.iter().any(|other| other.intersects(&label)));
            assert!(label.y + LABEL_HEIGHT <= y);
            placed.push(label);
        }
        let mut placed = Vec::new();
        for (x, y) in [(300.0, 10.0), (310.0, 14.0)] {
            let label = label_place(at(x, y), 150.0, size, &placed);
            assert!(!placed.iter().any(|other| other.intersects(&label)));
            assert!(label.y >= y);
            placed.push(label);
        }
        // The label that flips to the left of its point no longer lands on
        // the label of its neighbour.
        let first = label_place(at(500.0, 300.0), 200.0, size, &[]);
        let second = label_place(at(740.0, 302.0), 200.0, size, &[first]);
        assert!(second.x < 740.0 && !second.intersects(&first));
        // The label of one note leaves the marker of another in sight.
        let other = at(200.0, 262.0);
        let label = label_place(at(100.0, 300.0), 120.0, size, &[marker_area(other)]);
        assert!(!label.intersects(&marker_area(other)));
        assert!(label.y < 260.0 - LABEL_HEIGHT);
        // Its own marker is never in its way.
        let own = at(100.0, 300.0);
        assert_eq!(
            label_place(own, 120.0, size, &[marker_area(own)]),
            label_place(own, 120.0, size, &[])
        );
        // With no room left above, the label stays in the viewport.
        let crowded: Vec<Rectangle> = (0..40)
            .map(|row| Rectangle::new(at(0.0, row as f32 * 15.0), Size::new(800.0, 15.0)))
            .collect();
        let label = label_place(at(400.0, 100.0), 100.0, size, &crowded);
        assert!(label.y >= 0.0 && label.y + LABEL_HEIGHT <= size.height);
    }

    #[test]
    fn snapshot_is_the_canvas_part_of_the_window_as_a_png() {
        // A window of 40 by 30 logical pixels at a scale of 2, with the
        // canvas at (10, 5) and 20 by 15 in size, painted red on blue.
        let (width, height, scale) = (80u32, 60u32, 2.0);
        let canvas = Rectangle::new(UiPoint::new(10.0, 5.0), Size::new(20.0, 15.0));
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                let inside = (20..60).contains(&x) && (10..40).contains(&y);
                rgba.extend(if inside {
                    [200, 30, 40, 255]
                } else {
                    [0, 0, 255, 255]
                });
            }
        }
        let png = encode_snapshot(&rgba, width, height, scale, canvas).unwrap();
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        let image = ::image::load_from_memory_with_format(&png, ::image::ImageFormat::Png)
            .unwrap()
            .into_rgb8();
        assert_eq!((image.width(), image.height()), (40, 30));
        assert!(image.pixels().all(|pixel| pixel.0 == [200, 30, 40]));

        // A canvas that reaches outside the window is cut to the window.
        let wide = Rectangle::new(UiPoint::new(30.0, 20.0), Size::new(50.0, 50.0));
        let png = encode_snapshot(&rgba, width, height, scale, wide).unwrap();
        let image = ::image::load_from_memory(&png).unwrap();
        assert_eq!((image.width(), image.height()), (20, 20));

        // A large canvas is reduced to the longest edge that is kept.
        let big = vec![90u8; 4000 * 10 * 4];
        let all = Rectangle::new(UiPoint::ORIGIN, Size::new(4000.0, 10.0));
        let png = encode_snapshot(&big, 4000, 10, 1.0, all).unwrap();
        let image = ::image::load_from_memory(&png).unwrap();
        assert_eq!((image.width(), image.height()), (SNAPSHOT_MAX_EDGE, 4));

        let outside = Rectangle::new(UiPoint::new(100.0, 100.0), Size::new(10.0, 10.0));
        assert!(encode_snapshot(&rgba, width, height, scale, outside).is_err());
        assert!(encode_snapshot(&rgba[1..], width, height, scale, canvas).is_err());
    }

    #[test]
    fn api_saves_restores_and_annotates_views() {
        let (mut studio, _directory) = studio_with_scan();
        assert_eq!(
            send(&mut studio, ApiCommand::Status)["result"]["views"]["active"],
            Value::Null
        );
        let first = send(&mut studio, command(r#"{"command":"save_camera_view"}"#));
        assert_eq!(first["ok"], true);
        assert_eq!(first["name"], "View 1");
        let named = send(
            &mut studio,
            command(r#"{"command":"save_camera_view","name":"Trap"}"#),
        );
        assert_eq!(named["name"], "Trap");
        assert_eq!(named["guid"], studio.views.list[1].guid);
        let repeated = send(
            &mut studio,
            command(r#"{"command":"save_camera_view","name":"trap"}"#),
        );
        assert_eq!(repeated["ok"], false);

        let note = send(
            &mut studio,
            command(r#"{"command":"add_note","point":[4,3,0],"text":"Kozijn <A&B>"}"#),
        );
        assert_eq!(note["ok"], true);
        assert_eq!(note["view"], "Trap");
        assert_eq!(note["annotations"][0]["kind"], "note");
        assert_eq!(note["annotations"][0]["text"], "Kozijn <A&B>");
        let line = send(
            &mut studio,
            command(r#"{"command":"add_line","from":[0,0,0],"to":[4,0,0]}"#),
        );
        assert_eq!(line["annotations"][1]["kind"], "line");
        assert_eq!(line["annotations"][1]["to"], json!([4.0, 0.0, 0.0]));
        for rejected in [
            r#"{"command":"add_note","point":[4,3,0],"text":"  "}"#,
            r#"{"command":"add_line","from":[1,1,1],"to":[1,1,1]}"#,
            r#"{"command":"delete_annotation","index":2}"#,
        ] {
            assert_eq!(send(&mut studio, command(rejected))["ok"], false);
        }
        assert_eq!(studio.views.list[1].annotations.len(), 2);

        let status = send(&mut studio, ApiCommand::Status);
        let views = &status["result"]["views"];
        assert_eq!(views["active"]["name"], "Trap");
        assert_eq!(views["active"]["annotations"].as_array().unwrap().len(), 2);
        assert_eq!(views["annotation_tool"], Value::Null);
        assert_eq!(views["snapshots_pending"], 2);
        assert_eq!(
            status["result"]["camera_views"].as_array().unwrap().len(),
            2
        );
        act(&mut studio, ViewAction::Tool(AnnotationKind::Line));
        assert_eq!(
            send(&mut studio, ApiCommand::Status)["result"]["views"]["annotation_tool"],
            "line"
        );

        let listed = send(&mut studio, ApiCommand::ListCameraViews);
        assert_eq!(listed["views"].as_array().unwrap().len(), 2);
        assert_eq!(listed["active"], "Trap");
        assert_eq!(listed["views"][1]["annotations"][0]["text"], "Kozijn <A&B>");

        let removed = send(
            &mut studio,
            command(r#"{"command":"delete_annotation","index":0}"#),
        );
        assert_eq!(removed["annotations"].as_array().unwrap().len(), 1);

        let _ = studio.update(Message::Orbit(50.0, 0.0));
        let yaw = studio.yaw;
        let updated = send(
            &mut studio,
            command(r#"{"command":"update_camera_view","name":"view 1"}"#),
        );
        assert_eq!(updated["ok"], true);
        assert_eq!(studio.views.list[0].yaw, yaw);
        let renamed = send(
            &mut studio,
            command(r#"{"command":"rename_camera_view","name":"View 1","new_name":"Hal"}"#),
        );
        assert_eq!(renamed["name"], "Hal");
        let clash = send(
            &mut studio,
            command(r#"{"command":"rename_camera_view","name":"Hal","new_name":"Trap"}"#),
        );
        assert_eq!(clash["ok"], false);

        let restored = send(
            &mut studio,
            command(r#"{"command":"restore_camera_view","name":"TRAP"}"#),
        );
        assert_eq!(restored["ok"], true);
        assert_eq!(restored["view"]["name"], "Trap");
        assert_eq!(studio.view_label, "SAVED VIEW");
        let deleted = send(
            &mut studio,
            command(r#"{"command":"delete_camera_view","name":"hal"}"#),
        );
        assert_eq!(deleted["name"], "Hal");
        for missing in [
            r#"{"command":"restore_camera_view","name":"Hal"}"#,
            r#"{"command":"delete_camera_view","name":"Hal"}"#,
            r#"{"command":"update_camera_view","name":"Hal"}"#,
        ] {
            assert_eq!(send(&mut studio, command(missing))["ok"], false);
        }
        assert_eq!(camera_views::load(), studio.views.list);
    }

    #[test]
    fn api_drives_the_annotation_tools_like_the_viewport_does() {
        let (mut studio, _directory) = studio_with_scan();
        let placing = |studio: &mut Studio| {
            send(studio, ApiCommand::Status)["result"]["views"]["placing"].clone()
        };
        // Without a tool there is nothing to click with or to give a text.
        for early in [
            r#"{"command":"annotate_screen","pointer":[10,10]}"#,
            r#"{"command":"submit_note","text":"Te vroeg"}"#,
            r#"{"command":"set_annotation_tool","tool":"arrow"}"#,
        ] {
            assert_eq!(send(&mut studio, command(early))["ok"], false);
        }
        let _ = studio.update(Message::ToggleBoxSelect);
        let chosen = send(
            &mut studio,
            command(r#"{"command":"set_annotation_tool","tool":"Note"}"#),
        );
        assert_eq!(chosen["annotation_tool"], "note");
        assert_eq!(studio.views.tool, Some(AnnotationKind::Note));
        assert!(!studio.box_select);

        // A click starts the exact pick search; its answer places the point.
        let outside = send(
            &mut studio,
            command(r#"{"command":"annotate_screen","pointer":[-5,10]}"#),
        );
        assert_eq!(outside["ok"], false);
        assert!(!studio.selection_pending);
        let clicked = send(
            &mut studio,
            command(r#"{"command":"annotate_screen","pointer":[10,10]}"#),
        );
        assert_eq!(clicked["ok"], true);
        assert!(studio.selection_pending);
        let busy = send(
            &mut studio,
            command(r#"{"command":"annotate_screen","pointer":[10,10]}"#),
        );
        assert_eq!(busy["ok"], false);
        assert_eq!(placing(&mut studio), Value::Null);
        pick(&mut studio, [4.0, 3.0, 0.0]);
        assert_eq!(
            placing(&mut studio),
            json!({"kind": "note", "point": [4.0, 3.0, 0.0]})
        );

        // An empty text leaves the note waiting; a text places it.
        let empty = send(
            &mut studio,
            command(r#"{"command":"submit_note","text":" "}"#),
        );
        assert_eq!(empty["ok"], false);
        assert!(studio.views.note_point.is_some());
        let placed = send(
            &mut studio,
            command(r#"{"command":"submit_note","text":"Via de opdrachten"}"#),
        );
        assert_eq!(placed["ok"], true);
        assert_eq!(placed["view"], "View 1");
        assert_eq!(placed["annotations"][0]["text"], "Via de opdrachten");
        assert_eq!(placing(&mut studio), Value::Null);

        // The line tool waits for its second point until the tool is left.
        let line = send(
            &mut studio,
            command(r#"{"command":"set_annotation_tool","tool":"line"}"#),
        );
        assert_eq!(line["annotation_tool"], "line");
        pick(&mut studio, [0.0, 0.0, 0.0]);
        assert_eq!(
            placing(&mut studio),
            json!({"kind": "line", "point": [0.0, 0.0, 0.0]})
        );
        let left = send(
            &mut studio,
            command(r#"{"command":"set_annotation_tool","tool":null}"#),
        );
        assert_eq!(left["ok"], true);
        assert_eq!(left["annotation_tool"], Value::Null);
        assert_eq!(studio.views.tool, None);
        assert_eq!(placing(&mut studio), Value::Null);
        assert_eq!(studio.views.list[0].annotations.len(), 1);
    }

    #[test]
    fn api_exports_the_views_of_the_active_scan_as_a_bcf_file() {
        let (mut studio, directory) = studio_with_scan();
        let path = directory.path().join("views.bcf");
        let export = ApiCommand::ExportBcf { path: path.clone() };
        let nothing = send(&mut studio, export.clone());
        assert_eq!(nothing["ok"], false);
        assert!(!path.exists());

        let _ = studio.update(Message::Orbit(35.0, -20.0));
        let section = send(
            &mut studio,
            command(r#"{"command":"set_section","min":[1,0.5,0.25],"max":[3,2.5,1.5]}"#),
        );
        assert_eq!(section["ok"], true);
        let _ = send(
            &mut studio,
            command(r#"{"command":"save_camera_view","name":"Gevel <noord> & \"dak\" 'é'"}"#),
        );
        let _ = send(
            &mut studio,
            command(
                r#"{"command":"add_note","point":[4,3,0],"text":"Scheur ≥ 2 mm <links> & 'rechts'"}"#,
            ),
        );
        let _ = send(
            &mut studio,
            command(r#"{"command":"add_line","from":[0,0,0],"to":[4,0,0]}"#),
        );
        let _ = studio.update(Message::SetSectionEnabled(false));
        let _ = send(&mut studio, command(r#"{"command":"save_camera_view"}"#));
        let snapshot = encode_snapshot(
            &[120u8; 16 * 12 * 4],
            16,
            12,
            1.0,
            Rectangle::new(UiPoint::ORIGIN, Size::new(16.0, 12.0)),
        )
        .unwrap();
        let (first, second) = (
            studio.views.list[0].guid.clone(),
            studio.views.list[1].guid.clone(),
        );
        camera_views::write_snapshot(&first, &snapshot).unwrap();

        // A view of another scan is not part of the export.
        let other = directory.path().join("other.xyz");
        std::fs::write(&other, "0 0 0\n1 1 1\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&other, 10).unwrap());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let _ = send(&mut studio, command(r#"{"command":"save_camera_view"}"#));
        let _ = studio.update(Message::Select(0));

        for rejected in [
            PathBuf::from("relative.bcf"),
            directory.path().join("views.zip"),
        ] {
            assert_eq!(
                send(&mut studio, ApiCommand::ExportBcf { path: rejected })["ok"],
                false
            );
        }
        // A view saved before times were kept gets its time with the first
        // export and keeps it.
        studio.views.list[1].created = 0;
        studio.views.list[0].snapshot_due = false;
        let exported = send(&mut studio, export.clone());
        assert_eq!(exported["ok"], true);
        assert_eq!(exported["views"], 2);
        assert_eq!(exported["snapshots"], 1);
        assert_eq!(exported["snapshots_due"], 1);
        assert!(studio
            .status
            .starts_with("Exported 2 view(s), 1 with a snapshot, to "));
        assert!(studio
            .status
            .ends_with("; 1 view(s) changed after their snapshot: restore them to renew it"));
        let stamped = studio.views.list[1].created;
        assert!(stamped > 0);
        assert_eq!(camera_views::load(), studio.views.list);
        // The view of the other scan is not touched.
        assert!(studio.views.list[2].created > 0 && studio.views.list[2].snapshot_due);
        studio.views.list[1].snapshot_due = false;
        let again = send(&mut studio, export);
        assert_eq!(again["snapshots_due"], 0);
        assert!(!studio.status.contains("restore them"));
        assert_eq!(studio.views.list[1].created, stamped);

        let bytes = std::fs::read(&path).unwrap();
        let files = entries(&bytes).unwrap();
        let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "bcf.version".to_owned(),
                format!("{first}/markup.bcf"),
                format!("{first}/viewpoint.bcfv"),
                format!("{first}/snapshot.png"),
                format!("{second}/markup.bcf"),
                format!("{second}/viewpoint.bcfv"),
            ]
        );
        let document = |name: &str| {
            let (_, content) = files.iter().find(|(entry, _)| entry == name).unwrap();
            parse(std::str::from_utf8(content).unwrap()).unwrap()
        };
        let markup = document(&format!("{first}/markup.bcf"));
        let topic = markup.child("Topic").unwrap();
        assert_eq!(topic.attribute("Guid"), Some(first.as_str()));
        assert_eq!(topic.value("Title"), Some("Gevel <noord> & \"dak\" 'é'"));
        let comments = markup.all("Comment");
        assert_eq!(comments.len(), 1);
        assert_eq!(
            comments[0].value("Comment"),
            Some("Scheur ≥ 2 mm <links> & 'rechts'")
        );
        assert_eq!(files[3].1, snapshot);

        // The camera in the file shows a point where the viewport shows it.
        let viewpoint = document(&format!("{first}/viewpoint.bcfv"));
        let camera = viewpoint.child("PerspectiveCamera").unwrap();
        let eye = camera.xyz("CameraViewPoint").unwrap();
        let direction = camera.xyz("CameraDirection").unwrap();
        let up = camera.xyz("CameraUpVector").unwrap();
        let field_of_view: f64 = camera.value("FieldOfView").unwrap().parse().unwrap();
        let dot = |a: Xyz, b: Xyz| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let right = [
            direction[1] * up[2] - direction[2] * up[1],
            direction[2] * up[0] - direction[0] * up[2],
            direction[0] * up[1] - direction[1] * up[0],
        ];
        let view = &studio.views.list[0];
        let size = studio.viewport_size;
        let scene = Bounds {
            min: view.frame.unwrap().scene_min,
            max: view.frame.unwrap().scene_max,
        };
        let projection = Projection::new(
            scene,
            view.yaw,
            view.pitch,
            view.zoom,
            view.pan,
            size.width,
            size.height,
        );
        let focal = 0.5 * f64::from(size.height) / (field_of_view.to_radians() * 0.5).tan();
        for point in [[4.0, 3.0, 0.0], [0.0, 0.0, 0.0], [0.0, 3.0, 2.0]] {
            let relative: Xyz = std::array::from_fn(|axis| point[axis] - eye[axis]);
            let depth = dot(relative, direction);
            let x = f64::from(size.width) * 0.5 + dot(relative, right) * focal / depth;
            let y = f64::from(size.height) * 0.5 - dot(relative, up) * focal / depth;
            let (shown_x, shown_y, _) = projection.project_unclipped(point).unwrap();
            assert!((x - f64::from(shown_x)).abs() < 0.02, "{x} {shown_x}");
            assert!((y - f64::from(shown_y)).abs() < 0.02, "{y} {shown_y}");
        }

        // The first view has its section box and its lines; the second,
        // saved with the box off, has neither.
        let planes = viewpoint
            .child("ClippingPlanes")
            .unwrap()
            .all("ClippingPlane");
        assert_eq!(planes.len(), 6);
        let lines = viewpoint.child("Lines").unwrap().all("Line");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].xyz("StartPoint"), Some([4.0, 3.0, 0.0]));
        assert_eq!(lines[1].xyz("EndPoint"), Some([4.0, 0.0, 0.0]));
        let plain = document(&format!("{second}/viewpoint.bcfv"));
        assert!(plain.child("ClippingPlanes").is_none() && plain.child("Lines").is_none());
        let second_markup = document(&format!("{second}/markup.bcf"));
        assert!(second_markup
            .child("Viewpoints")
            .unwrap()
            .child("Snapshot")
            .is_none());
        assert_eq!(
            second_markup.child("Topic").unwrap().value("CreationDate"),
            Some(bcf::timestamp(stamped).as_str())
        );
    }

    #[test]
    fn views_group_and_properties_build_in_every_state() {
        let (mut studio, _directory) = studio_with_scan();
        let _ = studio.view();
        assert!(!studio.can_export_bcf());
        act(&mut studio, ViewAction::Name("Hal".into()));
        act(&mut studio, ViewAction::Save);
        assert!(studio.can_export_bcf());
        let _ = studio.view();
        let _ = send(
            &mut studio,
            command(
                r#"{"command":"add_note","point":[4,3,0],"text":"Een notitie die langer is dan het label toont, veel langer"}"#,
            ),
        );
        let _ = send(
            &mut studio,
            command(r#"{"command":"add_line","from":[0,0,0],"to":[4,0,0]}"#),
        );
        act(&mut studio, ViewAction::Tool(AnnotationKind::Note));
        pick(&mut studio, [0.0, 3.0, 2.0]);
        let _ = studio.view();
        let guid = studio.views.list[0].guid.clone();
        act(&mut studio, ViewAction::StartRename(guid));
        let _ = studio.view();
        let _ = studio.update(Message::ToggleFile);
        let _ = studio.view();

        assert_eq!(shortened("kort", 10), "kort");
        assert_eq!(shortened("een lange tekst", 8), "een lan…");
        assert_eq!(checked_note(" a\tb\n"), Some("a b".to_owned()));
        assert_eq!(checked_note(&"é".repeat(MAX_NOTE_CHARS + 1)), None);
        assert_eq!(checked_name(" Hal "), Some("Hal"));
        assert_eq!(checked_name("  "), None);
        assert!(!author().is_empty());
    }
}
