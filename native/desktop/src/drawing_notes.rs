//! Annotations of the drawings of VIEWS and of sheets: texts, dimensions,
//! leaders and lines.
//!
//! On a plan, an elevation or a section in the Drawing view, **Text**,
//! **Dimension**, **Leader** and **Line** of the VIEWS group of the ribbon
//! place annotations. They are kept with the drawing, at points of the model
//! on the plane of the drawing, so they stay in place when the drawing is
//! made again. Their sizes are those of the paper at the annotation scale of
//! the drawing, 1:100 unless another is chosen: a text 2.5 mm high is 0.25 m
//! high in the model at 1:100. A dimension snaps its two points to the ends
//! and corners of the lines drawn, then takes its offset from a third click;
//! it shows the distance between its points in the model in millimetres,
//! rounded to 10 mm at 1:100 and 5 mm at 1:50 unless a value is typed. On a
//! sheet the annotations of a drawing show at the scale of its viewport with
//! their paper sizes, and **Text** and **Line** draw on the paper itself.
//! A DXF or DWG of a drawing written with **Export DXF/DWG…** holds them as
//! texts, real dimensions, leaders and polylines on layers of their own.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

use iced::alignment;
use iced::mouse;
use iced::widget::canvas::{self, event, Canvas, Frame};
use iced::widget::{button, column, container, pick_list, row, text, text_input};
use iced::{
    Color, Element, Fill, Pixels, Point as UiPoint, Rectangle, Renderer, Size, Task, Theme, Vector,
};
use pointcloud_core::{
    dimension_axes, dimension_shape, dimension_value, leader_shape, Drawing2d, DrawingEntity,
    DrawingFormat, DrawingFrame, DEFAULT_TEXT_HEIGHT, LAYER_DIMENSIONS, LAYER_LEADERS, LAYER_LINES,
    LAYER_RGB_CONTRAST, LAYER_TEXT,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::drawing_view::{DrawScene, DrawingViewAction, ViewCamera};
use crate::i18n::{key, tr};
use crate::layouts::model::PaperNote;
use crate::layouts::plot::Align;
use crate::saved_drawings::SavedDrawing;
use crate::{flat_tool_style, opencad_properties, opencad_ribbon, Message, Studio};

/// The longest text of an annotation.
pub const MAX_NOTE_CHARS: usize = 240;
/// A drawing or a sheet holds at most this many annotations.
pub const MAX_NOTES: usize = 500;
/// The text heights that can be typed, in millimetres on the paper.
const MIN_HEIGHT: f64 = 0.5;
const MAX_HEIGHT: f64 = 50.0;
/// A point snaps to an end or a corner of a line drawn within this many
/// pixels.
pub const SNAP_PIXELS: f64 = 10.0;
/// The pointer takes an annotation within this many pixels of it.
const HIT_PIXELS: f64 = 6.0;
/// A click moves the pointer at most this many pixels.
const CLICK_SLOP: f32 = 4.0;
/// The colour annotations are drawn in while they are selected or placed.
const ACTIVE: Color = Color::from_rgb(0.15, 0.39, 0.92);

fn is_none<T>(value: &Option<T>) -> bool {
    value.is_none()
}

/// What an annotation tool places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoteKind {
    Text,
    Dimension,
    Leader,
    Line,
}

impl NoteKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Dimension => "dimension",
            Self::Leader => "leader",
            Self::Line => "line",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        [Self::Text, Self::Dimension, Self::Leader, Self::Line]
            .into_iter()
            .find(|kind| kind.key().eq_ignore_ascii_case(value.trim()))
    }

    /// The clicks it takes before it is placed or asks for its text.
    fn points(self) -> usize {
        match self {
            Self::Text => 1,
            Self::Line | Self::Leader => 2,
            Self::Dimension => 3,
        }
    }

    fn takes_text(self) -> bool {
        matches!(self, Self::Text | Self::Leader)
    }
}

/// An annotation of a drawing, at points of the model on the plane of the
/// drawing. Heights are in millimetres on the paper, the offset of a
/// dimension in metres to the left of the direction from its first point
/// to its second as the drawing shows them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DrawingNote {
    Text {
        id: String,
        at: [f64; 3],
        height: f64,
        value: String,
    },
    Dimension {
        id: String,
        from: [f64; 3],
        to: [f64; 3],
        offset: f64,
        height: f64,
        /// A value typed in place of the measured one.
        #[serde(default, skip_serializing_if = "is_none")]
        text: Option<String>,
    },
    Leader {
        id: String,
        /// The point the arrow points at, and where its text stands.
        from: [f64; 3],
        to: [f64; 3],
        height: f64,
        value: String,
    },
    Line {
        id: String,
        from: [f64; 3],
        to: [f64; 3],
    },
}

impl DrawingNote {
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. }
            | Self::Dimension { id, .. }
            | Self::Leader { id, .. }
            | Self::Line { id, .. } => id,
        }
    }

    pub fn kind(&self) -> NoteKind {
        match self {
            Self::Text { .. } => NoteKind::Text,
            Self::Dimension { .. } => NoteKind::Dimension,
            Self::Leader { .. } => NoteKind::Leader,
            Self::Line { .. } => NoteKind::Line,
        }
    }

    /// The distance a dimension measures, in metres.
    pub fn measured(&self) -> Option<f64> {
        match self {
            Self::Dimension { from, to, .. } => Some(
                (0..3)
                    .map(|axis| (to[axis] - from[axis]).powi(2))
                    .sum::<f64>()
                    .sqrt(),
            ),
            _ => None,
        }
    }

    /// What it reads: its text, or the value of a dimension at a scale.
    pub fn reading(&self, scale: f64) -> String {
        match self {
            Self::Text { value, .. } | Self::Leader { value, .. } => value.clone(),
            Self::Dimension {
                text: Some(text), ..
            } => text.clone(),
            Self::Dimension { .. } => dimension_value(self.measured().unwrap_or_default(), scale),
            Self::Line { from, to, .. } => {
                let metres = (0..3)
                    .map(|axis| (to[axis] - from[axis]).powi(2))
                    .sum::<f64>()
                    .sqrt();
                format!("{metres:.2} m")
            }
        }
    }

    /// The same moved by `delta` in the model: a text and a line move, a
    /// leader moves its text and a dimension its line, across itself.
    pub fn dragged(&self, delta: [f64; 3], frame: &DrawingFrame) -> Self {
        let moved = |point: &[f64; 3]| std::array::from_fn(|axis| point[axis] + delta[axis]);
        let mut note = self.clone();
        match &mut note {
            Self::Text { at, .. } => *at = moved(at),
            Self::Line { from, to, .. } => {
                *from = moved(from);
                *to = moved(to);
            }
            Self::Leader { to, .. } => *to = moved(to),
            Self::Dimension {
                from, to, offset, ..
            } => {
                if let Some((normal, _)) = dimension_axes(frame.to_uv(*from), frame.to_uv(*to)) {
                    let shift = frame.to_uv(moved(&[0.0; 3]));
                    let zero = frame.to_uv([0.0; 3]);
                    let across = [shift[0] - zero[0], shift[1] - zero[1]];
                    *offset += across[0] * normal[0] + across[1] * normal[1];
                }
            }
        }
        note
    }

    /// The same with an identifier of its own, for a copy of its drawing.
    pub fn renewed(&self) -> Self {
        let mut note = self.clone();
        let id = match &mut note {
            Self::Text { id, .. }
            | Self::Dimension { id, .. }
            | Self::Leader { id, .. }
            | Self::Line { id, .. } => id,
        };
        *id = crate::camera_views::new_guid();
        note
    }

    fn valid(&self) -> bool {
        let finite = |point: &[f64; 3]| point.iter().all(|value| value.is_finite());
        let height_ok = |height: &f64| (MIN_HEIGHT..=MAX_HEIGHT).contains(height);
        let text_ok =
            |value: &str| !value.trim().is_empty() && value.chars().count() <= MAX_NOTE_CHARS;
        crate::camera_views::is_guid(self.id())
            && match self {
                Self::Text {
                    at, height, value, ..
                } => finite(at) && height_ok(height) && text_ok(value),
                Self::Dimension {
                    from,
                    to,
                    offset,
                    height,
                    ..
                } => finite(from) && finite(to) && offset.is_finite() && height_ok(height),
                Self::Leader {
                    from,
                    to,
                    height,
                    value,
                    ..
                } => finite(from) && finite(to) && height_ok(height) && text_ok(value),
                Self::Line { from, to, .. } => finite(from) && finite(to),
            }
    }
}

/// Drop the annotations this version cannot use.
pub(crate) fn repaired(notes: &mut Vec<DrawingNote>) {
    notes.retain(DrawingNote::valid);
    notes.truncate(MAX_NOTES);
}

/// One thing an annotation is drawn with, in metres in the frame of its
/// drawing or in millimetres on the paper.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NoteMark {
    Line(Vec<[f64; 2]>),
    Fill(Vec<[f64; 2]>),
    Text {
        at: [f64; 2],
        height: f64,
        rotation: f64,
        value: String,
        align: Align,
    },
}

/// The marks of an annotation of a drawing in metres in its frame: its
/// sizes those of the paper at `scale`, a dimension rounded at the
/// annotation scale `value_scale` of its drawing.
pub(crate) fn note_marks(
    note: &DrawingNote,
    frame: &DrawingFrame,
    scale: f64,
    value_scale: f64,
) -> Vec<NoteMark> {
    let metres = |height: f64| height * scale / 1000.0;
    match note {
        DrawingNote::Text {
            at, height, value, ..
        } => vec![NoteMark::Text {
            at: frame.to_uv(*at),
            height: metres(*height),
            rotation: 0.0,
            value: value.clone(),
            align: Align::Left,
        }],
        DrawingNote::Line { from, to, .. } => {
            vec![NoteMark::Line(vec![frame.to_uv(*from), frame.to_uv(*to)])]
        }
        DrawingNote::Dimension {
            from,
            to,
            offset,
            height,
            ..
        } => {
            let Some(shape) = dimension_shape(
                frame.to_uv(*from),
                frame.to_uv(*to),
                *offset,
                metres(*height),
            ) else {
                return Vec::new();
            };
            let mut marks: Vec<NoteMark> = shape
                .lines
                .iter()
                .map(|line| NoteMark::Line(line.to_vec()))
                .collect();
            marks.push(NoteMark::Text {
                at: shape.text_at,
                height: metres(*height),
                rotation: shape.text_rotation,
                value: note.reading(value_scale),
                align: Align::Centre,
            });
            marks
        }
        DrawingNote::Leader {
            from,
            to,
            height,
            value,
            ..
        } => {
            let shape = leader_shape(frame.to_uv(*from), frame.to_uv(*to), metres(*height));
            vec![
                NoteMark::Line(shape.line),
                NoteMark::Fill(shape.arrow.to_vec()),
                NoteMark::Text {
                    at: shape.text_at,
                    height: metres(*height),
                    rotation: 0.0,
                    value: value.clone(),
                    align: if shape.right {
                        Align::Right
                    } else {
                        Align::Left
                    },
                },
            ]
        }
    }
}

/// The marks of a note on the paper, in millimetres.
pub(crate) fn paper_marks(note: &PaperNote) -> Vec<NoteMark> {
    match note {
        PaperNote::Text {
            at, height, value, ..
        } => vec![NoteMark::Text {
            at: *at,
            height: *height,
            rotation: 0.0,
            value: value.clone(),
            align: Align::Left,
        }],
        PaperNote::Line { from, to, .. } => vec![NoteMark::Line(vec![*from, *to])],
    }
}

/// The scale annotations of a drawing are drawn at.
pub(crate) fn drawing_scale(definition: &SavedDrawing) -> f64 {
    definition
        .scale
        .filter(|scale| crate::layouts::model::scale_valid(*scale))
        .unwrap_or(crate::layouts::model::DEFAULT_SCALE)
}

/// The frame of a drawing: its cut plane as the coordinate system of the
/// drawing.
pub(crate) fn drawing_frame(definition: &SavedDrawing) -> Option<DrawingFrame> {
    let request = definition.request()?;
    crate::drawing_crop::crop_frame(definition.oriented(), request.view, request.origin)
        .map(|crop| crop.frame)
}

/// The width of a text of `height`, about.
fn text_width(value: &str, height: f64) -> f64 {
    crate::layouts::plot::text_width(value, height)
}

/// The distance from a point to a segment.
fn to_segment(at: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let length = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if length > 0.0 {
        (((at[0] - a[0]) * ab[0] + (at[1] - a[1]) * ab[1]) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let near = [a[0] + ab[0] * t, a[1] + ab[1] * t];
    (at[0] - near[0]).hypot(at[1] - near[1])
}

/// How far a point lies from the marks of an annotation, in their units.
pub(crate) fn distance_to_marks(marks: &[NoteMark], at: [f64; 2]) -> f64 {
    let mut nearest = f64::INFINITY;
    for mark in marks {
        match mark {
            NoteMark::Line(points) | NoteMark::Fill(points) => {
                for pair in points.windows(2) {
                    nearest = nearest.min(to_segment(at, pair[0], pair[1]));
                }
            }
            NoteMark::Text {
                at: anchor,
                height,
                rotation,
                value,
                align,
            } => {
                let width = text_width(value, *height);
                let start = match align {
                    Align::Left => 0.0,
                    Align::Centre => -width / 2.0,
                    Align::Right => -width,
                };
                let (sin, cos) = rotation.sin_cos();
                let local = [at[0] - anchor[0], at[1] - anchor[1]];
                let along = local[0] * cos + local[1] * sin;
                let up = -local[0] * sin + local[1] * cos;
                let inside =
                    (start..=start + width).contains(&along) && (0.0..=*height).contains(&up);
                if inside {
                    return 0.0;
                }
                let dx = (start - along).max(along - start - width).max(0.0);
                let dy = (-up).max(up - height).max(0.0);
                nearest = nearest.min(dx.hypot(dy));
            }
        }
    }
    nearest
}

/// The ends and corners of the lines and fills of the layers shown, in the
/// units of the drawing: where a point snaps.
fn snap_points(scene: &DrawScene, shown: &[bool]) -> Vec<[f64; 2]> {
    let mut points = Vec::new();
    for (index, layer) in scene.layers.iter().enumerate() {
        if !shown.get(index).copied().unwrap_or(true) {
            continue;
        }
        for line in &layer.lines {
            points.extend(line.points.iter().copied());
        }
        for fill in &layer.fills {
            points.extend(fill.iter().flatten().copied());
        }
    }
    points
}

/// The nearest of `points` within `reach` of `at`.
pub(crate) fn snapped(points: &[[f64; 2]], at: [f64; 2], reach: f64) -> Option<[f64; 2]> {
    points
        .iter()
        .map(|point| (point, (point[0] - at[0]).hypot(point[1] - at[1])))
        .filter(|(_, distance)| *distance <= reach)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(point, _)| *point)
}

/// The points a click snaps to, with the drawing and layers they are of.
type Snaps = (usize, Arc<Vec<[f64; 2]>>);

/// What the annotation tools are doing.
#[derive(Debug, Default)]
pub(crate) struct NoteTool {
    /// The tool clicks place annotations with.
    pub(crate) kind: Option<NoteKind>,
    /// The points clicked so far: in the model for a drawing, on the paper
    /// for a sheet (with Z zero).
    pub(crate) picked: Vec<[f64; 3]>,
    /// The text of a text or a leader that waits for it.
    pub(crate) typing: Option<String>,
    /// The height of new texts, as typed, in millimetres on the paper.
    pub(crate) height: String,
    /// The selected annotation, by its identifier.
    pub(crate) selected: Option<String>,
    /// What is typed for the selected annotation: its text or value.
    pub(crate) edit: Option<(String, String)>,
    /// The points a click snaps to, for the drawing they were found in.
    snaps: RefCell<Option<Snaps>>,
    pub(crate) export_pending: bool,
}

impl NoteTool {
    /// The height of new texts in millimetres.
    pub(crate) fn text_height(&self) -> f64 {
        self.height
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|height| (MIN_HEIGHT..=MAX_HEIGHT).contains(height))
            .unwrap_or(DEFAULT_TEXT_HEIGHT)
    }

    /// Stop placing: what was clicked and typed is dropped. Reports whether
    /// anything was.
    pub(crate) fn drop_placing(&mut self) -> bool {
        let had = !self.picked.is_empty() || self.typing.is_some();
        self.picked.clear();
        self.typing = None;
        had
    }

    fn snap_points(&self, scene: &DrawScene, shown: &[bool]) -> Arc<Vec<[f64; 2]>> {
        let key = scene as *const DrawScene as usize ^ shown.len().rotate_left(17);
        if let Some((known, points)) = &*self.snaps.borrow() {
            if *known == key {
                return Arc::clone(points);
            }
        }
        let points = Arc::new(snap_points(scene, shown));
        *self.snaps.borrow_mut() = Some((key, Arc::clone(&points)));
        points
    }
}

/// Where annotations go: the drawing shown or the sheet shown.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum NoteTarget {
    Drawing(String),
    Sheet(String),
}

/// Everything the annotations react to.
#[derive(Debug, Clone)]
pub(crate) enum NoteAction {
    /// Choose a tool, or leave it when it is chosen.
    Tool(NoteKind),
    /// A click with the tool at a point of the drawing, in its units, or of
    /// the paper.
    Pick([f64; 2]),
    Typed(String),
    Submit,
    Cancel,
    Select(Option<String>),
    /// The selected annotation was dragged by so much, in the units of the
    /// drawing or on the paper.
    Moved(String, [f64; 2]),
    Delete(String),
    DeleteSelected,
    Height(String),
    /// The text typed for an annotation, or the value of a dimension.
    Edit(String, String),
    /// Enter in that field.
    ApplyEdit,
    Scale(crate::layouts::ScaleChoice),
    Export,
    ExportPathChosen(Option<PathBuf>),
    Exported(Option<String>, Result<(PathBuf, u64), String>),
}

impl Studio {
    /// Where annotations go now: the sheet shown, else the drawing of VIEWS
    /// shown.
    pub(crate) fn note_target(&self) -> Option<NoteTarget> {
        if let Some(sheet) = self.drawing_view.shown_layout() {
            return Some(NoteTarget::Sheet(sheet.to_owned()));
        }
        self.drawing_view
            .shown_guid()
            .map(|guid| NoteTarget::Drawing(guid.to_owned()))
    }

    fn definition(&self, guid: &str) -> Option<&SavedDrawing> {
        self.drawing_view
            .saved
            .iter()
            .find(|drawing| drawing.guid == guid)
    }

    /// Change the annotations of a drawing with `change` and keep them.
    fn edit_drawing_notes(
        &mut self,
        guid: &str,
        change: impl FnOnce(&mut Vec<DrawingNote>) -> Result<(), String>,
    ) -> Result<(), String> {
        let saved = &mut self.drawing_view.saved;
        let place = saved
            .iter()
            .position(|drawing| drawing.guid == guid)
            .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
        let before = saved[place].annotations.clone();
        change(&mut saved[place].annotations)?;
        if saved[place].annotations.len() > MAX_NOTES {
            saved[place].annotations = before;
            return Err(format!("A drawing holds at most {MAX_NOTES} annotations"));
        }
        if let Err(error) = crate::saved_drawings::save(saved) {
            saved[place].annotations = before;
            return Err(format!("The drawings could not be stored: {error}"));
        }
        self.layouts.annotations_changed();
        Ok(())
    }

    /// Add an annotation to a drawing; answers its identifier.
    pub(crate) fn add_drawing_note(
        &mut self,
        guid: &str,
        note: DrawingNote,
    ) -> Result<String, String> {
        if !note.valid() {
            return Err(
                "An annotation needs finite points, a height of 0.5 to 50 mm and a text".into(),
            );
        }
        let id = note.id().to_owned();
        let kind = note.kind();
        self.edit_drawing_notes(guid, |notes| {
            notes.push(note);
            Ok(())
        })?;
        let name = self
            .definition(guid)
            .map(|drawing| drawing.name.clone())
            .unwrap_or_default();
        self.status = format!("{} added to {name}", kind_name(kind));
        Ok(id)
    }

    /// Add a note to the paper of a sheet; answers its identifier.
    pub(crate) fn add_paper_note(
        &mut self,
        sheet: &str,
        note: PaperNote,
    ) -> Result<String, String> {
        if !note.valid() {
            return Err("A note on a sheet needs finite points and a text".into());
        }
        let id = note.id().to_owned();
        self.set_layout_field(sheet, |layout| {
            if layout.notes.len() >= MAX_NOTES {
                return Err(format!("A sheet holds at most {MAX_NOTES} notes"));
            }
            layout.notes.push(note);
            Ok(())
        })?;
        self.status = "Note added to the sheet".into();
        Ok(id)
    }

    /// Delete an annotation of the drawing or the sheet shown, by its
    /// identifier.
    pub(crate) fn delete_note(&mut self, target: &NoteTarget, id: &str) -> Result<(), String> {
        let missing = || "That annotation is no longer kept".to_owned();
        match target {
            NoteTarget::Drawing(guid) => self.edit_drawing_notes(guid, |notes| {
                let place = notes
                    .iter()
                    .position(|note| note.id() == id)
                    .ok_or_else(missing)?;
                notes.remove(place);
                Ok(())
            })?,
            NoteTarget::Sheet(sheet) => self.set_layout_field(sheet, |layout| {
                let place = layout
                    .notes
                    .iter()
                    .position(|note| note.id() == id)
                    .ok_or_else(missing)?;
                layout.notes.remove(place);
                Ok(())
            })?,
        }
        if self.notes.selected.as_deref() == Some(id) {
            self.notes.selected = None;
            self.notes.edit = None;
        }
        self.status = "Annotation deleted".into();
        Ok(())
    }

    /// The model point of a point of the drawing shown, in its units.
    fn model_point(&self, guid: &str, at: [f64; 2]) -> Option<[f64; 3]> {
        let definition = self.definition(guid)?;
        let frame = drawing_frame(definition)?;
        let factor = definition.request()?.units.factor();
        Some(frame.to_world([at[0] / factor, at[1] / factor]))
    }

    pub(crate) fn update_notes(&mut self, action: NoteAction) -> Task<Message> {
        match action {
            NoteAction::Tool(kind) => {
                let stop = self.notes.kind == Some(kind);
                self.notes.drop_placing();
                self.notes.kind = None;
                if stop {
                    self.status = "Annotation tool closed".into();
                    return Task::none();
                }
                let Some(target) = self.note_target() else {
                    self.status =
                        "Show a plan, an elevation, a section or a sheet to annotate it".into();
                    return Task::none();
                };
                if matches!(target, NoteTarget::Sheet(_))
                    && !matches!(kind, NoteKind::Text | NoteKind::Line)
                {
                    self.status = "On a sheet, place a text or a line".into();
                    return Task::none();
                }
                // The tools of the 3D view and of the sheet let go.
                self.views.leave_tool();
                self.notes.kind = Some(kind);
                self.status = match kind {
                    NoteKind::Text => "Text: click where it starts, then type it; Escape cancels",
                    NoteKind::Dimension => {
                        "Dimension: click two points, which snap to the ends and corners of the lines, then where its line goes; Escape cancels"
                    }
                    NoteKind::Leader => {
                        "Leader: click the point it points at and where its text goes, then type it; Escape cancels"
                    }
                    NoteKind::Line => "Line: click its start and its end; Escape cancels",
                }
                .into();
            }
            NoteAction::Pick(at) => return self.note_pick(at),
            NoteAction::Typed(value) => {
                if let Some(typing) = &mut self.notes.typing {
                    *typing = value;
                }
            }
            NoteAction::Submit => {
                let Some(value) = self.notes.typing.clone() else {
                    return Task::none();
                };
                if value.trim().is_empty() {
                    self.status = "Type the text first; Escape cancels".into();
                    return Task::none();
                }
                if let Err(error) = self.place_note(Some(value.trim())) {
                    self.status = error;
                }
            }
            NoteAction::Cancel => {
                if self.notes.drop_placing() {
                    self.status = "Annotation cancelled".into();
                }
            }
            NoteAction::Select(id) => {
                if self.notes.selected != id {
                    self.notes.edit = None;
                }
                self.notes.selected = id;
            }
            NoteAction::Moved(id, delta) => {
                if let Err(error) = self.move_note(&id, delta) {
                    self.status = error;
                }
            }
            NoteAction::Delete(id) => {
                if let Some(target) = self.note_target() {
                    if let Err(error) = self.delete_note(&target, &id) {
                        self.status = error;
                    }
                }
            }
            NoteAction::DeleteSelected => {
                if let (Some(target), Some(id)) = (self.note_target(), self.notes.selected.clone())
                {
                    if let Err(error) = self.delete_note(&target, &id) {
                        self.status = error;
                    }
                }
            }
            NoteAction::Height(value) => self.notes.height = value,
            NoteAction::Edit(id, value) => self.notes.edit = Some((id, value)),
            NoteAction::ApplyEdit => {
                if let Some((id, value)) = self.notes.edit.clone() {
                    match self.set_note_text(&id, &value) {
                        Ok(()) => self.notes.edit = None,
                        Err(error) => self.status = error,
                    }
                }
            }
            NoteAction::Scale(crate::layouts::ScaleChoice(scale)) => {
                if let Some(guid) = self.drawing_view.shown_guid().map(str::to_owned) {
                    if let Err(error) = self.set_drawing_scale(&guid, scale) {
                        self.status = error;
                    }
                }
            }
            NoteAction::Export => {
                let Some(definition) = self
                    .drawing_view
                    .shown_guid()
                    .and_then(|guid| self.definition(guid))
                else {
                    return Task::none();
                };
                if self.notes.export_pending {
                    return Task::none();
                }
                let suggested = format!("{}.dxf", crate::layouts::file_stem(&definition.name));
                self.notes.export_pending = true;
                self.status = "Choose where to save the drawing as DXF or DWG…".into();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .add_filter("DXF", &["dxf"])
                            .add_filter("DWG", &["dwg"])
                            .set_file_name(suggested)
                            .save_file()
                            .await
                            .map(|chosen| chosen.path().to_path_buf())
                    },
                    |path| Message::Notes(NoteAction::ExportPathChosen(path)),
                );
            }
            NoteAction::ExportPathChosen(path) => {
                self.notes.export_pending = false;
                let (Some(path), Some(guid)) =
                    (path, self.drawing_view.shown_guid().map(str::to_owned))
                else {
                    self.status = "Export cancelled".into();
                    return Task::none();
                };
                return match self.start_drawing_file(&guid, path, None) {
                    Ok(task) => task,
                    Err(error) => {
                        self.status = error;
                        Task::none()
                    }
                };
            }
            NoteAction::Exported(job, result) => {
                self.notes.export_pending = false;
                let value = match &result {
                    Ok((path, bytes)) => {
                        self.status = format!(
                            "Drawing written to {} ({})",
                            path.display(),
                            crate::drawing::size_text(*bytes)
                        );
                        json!({"state": "complete", "operation": "export_drawing_file", "path": path, "bytes": bytes})
                    }
                    Err(error) => {
                        self.status = format!("The drawing could not be written: {error}");
                        json!({"state": "failed", "operation": "export_drawing_file", "error": error})
                    }
                };
                if let Some(entry) = job.and_then(|id| self.api_jobs.get_mut(&id)) {
                    *entry = value;
                }
                if let Ok((path, _)) = result {
                    self.cad_file_written(&path);
                }
            }
        }
        Task::none()
    }

    /// A click with an annotation tool: a point of the drawing shown, in
    /// its units, or of the paper of the sheet shown.
    fn note_pick(&mut self, at: [f64; 2]) -> Task<Message> {
        let Some(kind) = self.notes.kind else {
            return Task::none();
        };
        if self.notes.typing.is_some() {
            return Task::none();
        }
        let point = match self.note_target() {
            Some(NoteTarget::Drawing(guid)) => match self.model_point(&guid, at) {
                Some(point) => point,
                None => return Task::none(),
            },
            Some(NoteTarget::Sheet(_)) => [at[0], at[1], 0.0],
            None => return Task::none(),
        };
        self.notes.picked.push(point);
        if self.notes.picked.len() < kind.points() {
            self.status = match (kind, self.notes.picked.len()) {
                (NoteKind::Dimension, 1) => "Click the second point of the dimension",
                (NoteKind::Dimension, _) => "Click where the line of the dimension goes",
                (NoteKind::Leader, _) => "Click where the text of the leader goes",
                _ => "Click the end of the line",
            }
            .into();
            return Task::none();
        }
        if kind.takes_text() {
            self.notes.typing = Some(String::new());
            self.status = "Type the text and press Enter; Escape cancels".into();
            return text_input::focus(note_input_id());
        }
        if let Err(error) = self.place_note(None) {
            self.status = error;
            self.notes.drop_placing();
        }
        Task::none()
    }

    /// Place the annotation of the points clicked, with its text.
    fn place_note(&mut self, value: Option<&str>) -> Result<(), String> {
        let kind = self
            .notes
            .kind
            .ok_or_else(|| "No annotation tool".to_owned())?;
        let picked = std::mem::take(&mut self.notes.picked);
        self.notes.typing = None;
        let height = self.notes.text_height();
        let id = crate::camera_views::new_guid();
        let value = value
            .unwrap_or_default()
            .chars()
            .take(MAX_NOTE_CHARS)
            .collect::<String>();
        let target = self
            .note_target()
            .ok_or_else(|| "Show a drawing or a sheet to annotate it".to_owned())?;
        let added = match target {
            NoteTarget::Drawing(guid) => {
                let note = match (kind, picked.as_slice()) {
                    (NoteKind::Text, [at, ..]) => DrawingNote::Text {
                        id,
                        at: *at,
                        height,
                        value,
                    },
                    (NoteKind::Line, [from, to, ..]) => DrawingNote::Line {
                        id,
                        from: *from,
                        to: *to,
                    },
                    (NoteKind::Leader, [from, to, ..]) => DrawingNote::Leader {
                        id,
                        from: *from,
                        to: *to,
                        height,
                        value,
                    },
                    (NoteKind::Dimension, [from, to, line, ..]) => {
                        let frame = self
                            .definition(&guid)
                            .and_then(drawing_frame)
                            .ok_or_else(|| "The drawing has no plane".to_owned())?;
                        let offset = dimension_offset(&frame, *from, *to, *line)
                            .ok_or_else(|| "The two points of a dimension lie apart".to_owned())?;
                        DrawingNote::Dimension {
                            id,
                            from: *from,
                            to: *to,
                            offset,
                            height,
                            text: None,
                        }
                    }
                    _ => return Err("Click the points of the annotation".into()),
                };
                self.add_drawing_note(&guid, note)
            }
            NoteTarget::Sheet(sheet) => {
                let paper = |point: &[f64; 3]| [point[0], point[1]];
                let note = match (kind, picked.as_slice()) {
                    (NoteKind::Text, [at, ..]) => PaperNote::Text {
                        id,
                        at: paper(at),
                        height,
                        value,
                    },
                    (NoteKind::Line, [from, to, ..]) => PaperNote::Line {
                        id,
                        from: paper(from),
                        to: paper(to),
                    },
                    _ => return Err("On a sheet, place a text or a line".into()),
                };
                self.add_paper_note(&sheet, note)
            }
        }?;
        self.notes.selected = Some(added);
        Ok(())
    }

    /// Move an annotation of the drawing or the sheet shown by `delta`, in
    /// the units of the drawing or on the paper.
    fn move_note(&mut self, id: &str, delta: [f64; 2]) -> Result<(), String> {
        match self
            .note_target()
            .ok_or_else(|| "Show the drawing or the sheet of the annotation".to_owned())?
        {
            NoteTarget::Drawing(guid) => {
                let definition = self
                    .definition(&guid)
                    .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
                let frame = drawing_frame(definition)
                    .ok_or_else(|| "The drawing has no plane".to_owned())?;
                let factor = definition
                    .request()
                    .map_or(1000.0, |request| request.units.factor());
                let zero = frame.to_world([0.0, 0.0]);
                let shifted = frame.to_world([delta[0] / factor, delta[1] / factor]);
                let shift: [f64; 3] = std::array::from_fn(|axis| shifted[axis] - zero[axis]);
                self.edit_drawing_notes(&guid, |notes| {
                    let note = notes
                        .iter_mut()
                        .find(|note| note.id() == id)
                        .ok_or_else(|| "That annotation is no longer kept".to_owned())?;
                    *note = note.dragged(shift, &frame);
                    Ok(())
                })
            }
            NoteTarget::Sheet(sheet) => self.set_layout_field(&sheet, |layout| {
                let note = layout
                    .notes
                    .iter_mut()
                    .find(|note| note.id() == id)
                    .ok_or_else(|| "That note is no longer on the sheet".to_owned())?;
                *note = note.moved(delta);
                Ok(())
            }),
        }
    }

    /// The text of a text or a leader, or the value of a dimension: empty
    /// gives a dimension its measured value back.
    pub(crate) fn set_note_text(&mut self, id: &str, value: &str) -> Result<(), String> {
        let value: String = value.trim().chars().take(MAX_NOTE_CHARS).collect();
        match self
            .note_target()
            .ok_or_else(|| "Show the drawing or the sheet of the annotation".to_owned())?
        {
            NoteTarget::Drawing(guid) => self.edit_drawing_notes(&guid, |notes| {
                let note = notes
                    .iter_mut()
                    .find(|note| note.id() == id)
                    .ok_or_else(|| "That annotation is no longer kept".to_owned())?;
                match note {
                    DrawingNote::Text { value: text, .. }
                    | DrawingNote::Leader { value: text, .. } => {
                        if value.is_empty() {
                            return Err("A text cannot be empty".into());
                        }
                        *text = value;
                    }
                    DrawingNote::Dimension { text, .. } => {
                        *text = Some(value).filter(|value| !value.is_empty());
                    }
                    DrawingNote::Line { .. } => {}
                }
                Ok(())
            }),
            NoteTarget::Sheet(sheet) => self.set_layout_field(&sheet, |layout| {
                if let Some(PaperNote::Text { value: text, .. }) =
                    layout.notes.iter_mut().find(|note| note.id() == id)
                {
                    if value.is_empty() {
                        return Err("A text cannot be empty".into());
                    }
                    *text = value;
                }
                Ok(())
            }),
        }
    }

    /// The annotation scale of a drawing.
    pub(crate) fn set_drawing_scale(&mut self, guid: &str, scale: f64) -> Result<(), String> {
        if !crate::layouts::model::scale_valid(scale) {
            return Err("A scale lies between 1:1 and 1:100000".into());
        }
        let saved = &mut self.drawing_view.saved;
        let place = saved
            .iter()
            .position(|drawing| drawing.guid == guid)
            .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
        let before = saved[place].scale.replace(scale);
        if let Err(error) = crate::saved_drawings::save(saved) {
            saved[place].scale = before;
            return Err(format!("The drawings could not be stored: {error}"));
        }
        self.layouts.annotations_changed();
        Ok(())
    }

    /// Escape while annotating: what is being placed, else the tool, else
    /// the selected annotation. Reports whether there was any.
    pub(crate) fn notes_escape(&mut self) -> bool {
        if self.notes.drop_placing() {
            self.status = "Annotation cancelled".into();
            return true;
        }
        if self.notes.kind.take().is_some() {
            self.status = "Annotation tool closed".into();
            return true;
        }
        if self.notes.selected.take().is_some() {
            self.notes.edit = None;
            return true;
        }
        false
    }

    /// Keep the annotation tools with what is shown: a tool for a drawing
    /// is left once no drawing or sheet is shown.
    pub(crate) fn settle_notes(&mut self) {
        let target = self.note_target();
        if target.is_none() && (self.notes.kind.is_some() || self.notes.selected.is_some()) {
            self.notes.drop_placing();
            self.notes.kind = None;
            self.notes.selected = None;
            self.notes.edit = None;
        }
        if matches!(target, Some(NoteTarget::Sheet(_)))
            && matches!(
                self.notes.kind,
                Some(NoteKind::Dimension | NoteKind::Leader)
            )
        {
            self.notes.drop_placing();
            self.notes.kind = None;
        }
    }

    /// A drawing of VIEWS as a model of a drawing, in metres: every layer of
    /// it as it is made, and its annotations on layers of their own.
    pub(crate) fn drawing_with_notes(&self, guid: &str) -> Result<Drawing2d, String> {
        let definition = self
            .definition(guid)
            .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
        let scene = self.drawing_view.made(guid).ok_or_else(|| {
            format!(
                "Show the drawing {} first, so that it is made",
                definition.name
            )
        })?;
        let mut drawing = scene_drawing(scene)?;
        let frame =
            drawing_frame(definition).ok_or_else(|| "The drawing has no plane".to_owned())?;
        let scale = drawing_scale(definition);
        let metres = |height: f64| height * scale / 1000.0;
        let layer = |drawing: &mut Drawing2d, name: &str| {
            drawing
                .layer(name, LAYER_RGB_CONTRAST)
                .map_err(|error| error.to_string())
        };
        for note in &definition.annotations {
            match note {
                DrawingNote::Text {
                    at, height, value, ..
                } => {
                    let on = layer(&mut drawing, LAYER_TEXT)?;
                    drawing.add_text(on, frame.to_uv(*at), metres(*height), value);
                }
                DrawingNote::Line { from, to, .. } => {
                    let on = layer(&mut drawing, LAYER_LINES)?;
                    drawing.add_polyline(on, vec![frame.to_uv(*from), frame.to_uv(*to)], false);
                }
                DrawingNote::Dimension {
                    from,
                    to,
                    offset,
                    height,
                    text,
                    ..
                } => {
                    let on = layer(&mut drawing, LAYER_DIMENSIONS)?;
                    drawing.add_dimension(
                        on,
                        [frame.to_uv(*from), frame.to_uv(*to)],
                        *offset,
                        [metres(*height), scale],
                        text.as_deref(),
                    );
                }
                DrawingNote::Leader {
                    from,
                    to,
                    height,
                    value,
                    ..
                } => {
                    let on = layer(&mut drawing, LAYER_LEADERS)?;
                    drawing.add_leader(
                        on,
                        [frame.to_uv(*from), frame.to_uv(*to)],
                        [metres(*height), scale],
                        value,
                    );
                }
            }
        }
        Ok(drawing)
    }

    /// Write a drawing of VIEWS with its annotations as DXF or DWG, on a
    /// worker thread.
    pub(crate) fn start_drawing_file(
        &mut self,
        guid: &str,
        path: PathBuf,
        job: Option<String>,
    ) -> Result<Task<Message>, String> {
        let format = DrawingFormat::from_path(&path)
            .ok_or_else(|| "Save the drawing as .dxf or .dwg".to_owned())?;
        let drawing = self.drawing_with_notes(guid)?;
        let version = self
            .definition(guid)
            .and_then(SavedDrawing::request)
            .map(|request| request.version)
            .unwrap_or_default();
        self.notes.export_pending = true;
        self.status = format!("Writing {}…", path.display());
        Ok(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    pointcloud_core::write_drawing(&drawing, &path, format, version)
                        .map(|bytes| (path, bytes))
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            move |result| Message::Notes(NoteAction::Exported(job.clone(), result)),
        ))
    }
}

/// The offset of a dimension from its points to the point its line goes
/// through, all in the model, in metres to the left of the direction from
/// the first point to the second on the drawing.
pub(crate) fn dimension_offset(
    frame: &DrawingFrame,
    from: [f64; 3],
    to: [f64; 3],
    line: [f64; 3],
) -> Option<f64> {
    let [a, b, c] = [from, to, line].map(|point| frame.to_uv(point));
    let (normal, _) = dimension_axes(a, b)?;
    Some((c[0] - b[0]) * normal[0] + (c[1] - b[1]) * normal[1])
}

/// The name a kind of annotation goes by in the status bar.
fn kind_name(kind: NoteKind) -> &'static str {
    match kind {
        NoteKind::Text => key("Text"),
        NoteKind::Dimension => key("Dimension"),
        NoteKind::Leader => key("Leader"),
        NoteKind::Line => key("Line"),
    }
}

/// A drawing as the view shows it, as a model of a drawing in metres.
fn scene_drawing(scene: &DrawScene) -> Result<Drawing2d, String> {
    let factor = scene.units.factor();
    let metres = |point: [f64; 2]| [point[0] / factor, point[1] / factor];
    let mut drawing = Drawing2d::new(scene.units);
    for layer in &scene.layers {
        let on = drawing
            .layer(&layer.name, layer.rgb)
            .map_err(|error| error.to_string())?;
        for point in &layer.points {
            drawing.add_point(on, metres(point.at), point.rgb);
        }
        for fill in &layer.fills {
            let mut rings = fill
                .iter()
                .map(|ring| ring.iter().copied().map(metres).collect());
            if let Some(outer) = rings.next() {
                drawing.add_fill(on, outer, rings.collect());
            }
        }
        for line in &layer.lines {
            if line.points.len() >= 2 {
                drawing.add_polyline(
                    on,
                    line.points.iter().copied().map(metres).collect(),
                    line.closed,
                );
            }
        }
        for label in &layer.texts {
            drawing.entities.push((
                on,
                DrawingEntity::Text {
                    at: metres(label.at),
                    height: label.height / factor,
                    rotation: label.rotation,
                    value: label.value.clone(),
                },
            ));
        }
    }
    Ok(drawing)
}

pub(crate) fn note_input_id() -> text_input::Id {
    text_input::Id::new("ops-annotation-text")
}

/// The layer of the Drawing view that draws the annotations of the drawing
/// shown and takes the clicks of the annotation tools.
pub(crate) struct NotesLayer<'a> {
    pub tool: &'a NoteTool,
    pub scene: &'a DrawScene,
    pub shown_layers: Vec<bool>,
    pub notes: &'a [DrawingNote],
    pub frame: DrawingFrame,
    pub factor: f64,
    pub scale: f64,
    pub camera: ViewCamera,
}

#[derive(Debug, Default)]
pub(crate) struct NotesState {
    /// A press on an annotation that may become a drag: its identifier and
    /// where the pointer took it, in the units of the drawing.
    held: Option<(String, [f64; 2], UiPoint)>,
    /// Where the held annotation is dragged to.
    to: Option<[f64; 2]>,
    /// A middle button drag that pans while a tool is chosen.
    pan: Option<UiPoint>,
}

impl NotesLayer<'_> {
    /// The marks of an annotation in the units of the drawing.
    fn marks(&self, note: &DrawingNote) -> Vec<NoteMark> {
        let factor = self.factor;
        note_marks(note, &self.frame, self.scale, self.scale)
            .into_iter()
            .map(|mark| scaled_mark(mark, factor))
            .collect()
    }

    fn drawing_at(&self, pixel: UiPoint, size: Size) -> [f64; 2] {
        self.camera.to_drawing([pixel.x, pixel.y], size)
    }

    /// The point a click at a pixel takes: an end or a corner of a line
    /// near it, for the tools that snap.
    fn picked_at(&self, pixel: UiPoint, size: Size) -> ([f64; 2], bool) {
        let at = self.drawing_at(pixel, size);
        let snaps = matches!(
            self.tool.kind,
            Some(NoteKind::Dimension | NoteKind::Line | NoteKind::Leader)
        ) && !(self.tool.kind == Some(NoteKind::Dimension)
            && self.tool.picked.len() == 2)
            && !(self.tool.kind == Some(NoteKind::Leader) && self.tool.picked.len() == 1);
        if snaps {
            let points = self.tool.snap_points(self.scene, &self.shown_layers);
            if let Some(point) = snapped(&points, at, SNAP_PIXELS / self.camera.scale) {
                return (point, true);
            }
        }
        (at, false)
    }

    /// The annotation under a pixel.
    fn note_at(&self, pixel: UiPoint, size: Size) -> Option<&DrawingNote> {
        let at = self.drawing_at(pixel, size);
        let reach = HIT_PIXELS / self.camera.scale;
        self.notes
            .iter()
            .rev()
            .find(|note| distance_to_marks(&self.marks(note), at) <= reach)
    }

    fn uv(&self, point: [f64; 3]) -> [f64; 2] {
        let uv = self.frame.to_uv(point);
        [uv[0] * self.factor, uv[1] * self.factor]
    }
}

fn scaled_mark(mark: NoteMark, factor: f64) -> NoteMark {
    let scale = |point: [f64; 2]| [point[0] * factor, point[1] * factor];
    match mark {
        NoteMark::Line(points) => NoteMark::Line(points.into_iter().map(scale).collect()),
        NoteMark::Fill(points) => NoteMark::Fill(points.into_iter().map(scale).collect()),
        NoteMark::Text {
            at,
            height,
            rotation,
            value,
            align,
        } => NoteMark::Text {
            at: scale(at),
            height: height * factor,
            rotation,
            value,
            align,
        },
    }
}

/// Draw marks in drawing units on a frame, through a camera.
pub(crate) fn draw_marks(
    frame: &mut Frame,
    marks: &[NoteMark],
    to_screen: impl Fn([f64; 2]) -> UiPoint,
    pixels: f64,
    color: Color,
) {
    for mark in marks {
        match mark {
            NoteMark::Line(points) => {
                let Some(first) = points.first() else {
                    continue;
                };
                let path = canvas::Path::new(|builder| {
                    builder.move_to(to_screen(*first));
                    for point in &points[1..] {
                        builder.line_to(to_screen(*point));
                    }
                });
                frame.stroke(
                    &path,
                    canvas::Stroke::default().with_color(color).with_width(1.2),
                );
            }
            NoteMark::Fill(points) => {
                let Some(first) = points.first() else {
                    continue;
                };
                let path = canvas::Path::new(|builder| {
                    builder.move_to(to_screen(*first));
                    for point in &points[1..] {
                        builder.line_to(to_screen(*point));
                    }
                    builder.close();
                });
                frame.fill(&path, color);
            }
            NoteMark::Text {
                at,
                height,
                rotation,
                value,
                align,
            } => {
                let size = (height * pixels) as f32;
                if !(2.5..=2_000.0).contains(&size) {
                    continue;
                }
                let anchor = to_screen(*at);
                let content = canvas::Text {
                    content: value.clone(),
                    position: UiPoint::ORIGIN,
                    color,
                    size: Pixels(size * 1.4),
                    horizontal_alignment: match align {
                        Align::Left => alignment::Horizontal::Left,
                        Align::Centre => alignment::Horizontal::Center,
                        Align::Right => alignment::Horizontal::Right,
                    },
                    vertical_alignment: alignment::Vertical::Bottom,
                    ..canvas::Text::default()
                };
                frame.with_save(|frame| {
                    frame.translate(Vector::new(anchor.x, anchor.y));
                    if *rotation != 0.0 {
                        frame.rotate(-*rotation as f32);
                    }
                    frame.fill_text(content);
                });
            }
        }
    }
}

impl canvas::Program<Message> for NotesLayer<'_> {
    type State = NotesState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        let size = bounds.size();
        let notes = |action| Some(Message::Notes(action));
        let placing = self.tool.kind.is_some() && self.tool.typing.is_none();
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_in(bounds) else {
                    return (event::Status::Ignored, None);
                };
                if placing {
                    let (at, _) = self.picked_at(position, size);
                    return (event::Status::Captured, notes(NoteAction::Pick(at)));
                }
                if let Some(note) = self.note_at(position, size) {
                    let at = self.drawing_at(position, size);
                    state.held = Some((note.id().to_owned(), at, position));
                    state.to = None;
                    return (
                        event::Status::Captured,
                        notes(NoteAction::Select(Some(note.id().to_owned()))),
                    );
                }
                // A click beside the annotations lets the selected one go
                // and goes on to the sheet below.
                if self.tool.selected.is_some() {
                    return (event::Status::Ignored, notes(NoteAction::Select(None)));
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle)) if placing => {
                if let Some(position) = cursor.position_in(bounds) {
                    state.pan = Some(position);
                    return (event::Status::Captured, None);
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let now = UiPoint::new(position.x - bounds.x, position.y - bounds.y);
                if let Some(previous) = state.pan {
                    state.pan = Some(now);
                    return (
                        event::Status::Captured,
                        Some(Message::DrawingView(DrawingViewAction::Pan([
                            now.x - previous.x,
                            now.y - previous.y,
                        ]))),
                    );
                }
                if let Some((_, _, pressed)) = &state.held {
                    let far = (now.x - pressed.x).abs() > CLICK_SLOP
                        || (now.y - pressed.y).abs() > CLICK_SLOP;
                    if far || state.to.is_some() {
                        state.to = Some(self.drawing_at(now, size));
                    }
                    return (event::Status::Captured, None);
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                if button == mouse::Button::Middle && state.pan.take().is_some() {
                    return (event::Status::Captured, None);
                }
                if button == mouse::Button::Left {
                    if let Some((id, from, _)) = state.held.take() {
                        if let Some(to) = state.to.take() {
                            return (
                                event::Status::Captured,
                                notes(NoteAction::Moved(id, [to[0] - from[0], to[1] - from[1]])),
                            );
                        }
                        return (event::Status::Captured, None);
                    }
                }
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) if placing => {
                if let Some(position) = cursor.position_in(bounds) {
                    let steps = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => y,
                        mouse::ScrollDelta::Pixels { y, .. } => y / 60.0,
                    };
                    return (
                        event::Status::Captured,
                        Some(Message::DrawingView(DrawingViewAction::Zoom(
                            1.25f32.powf(steps.clamp(-10.0, 10.0)),
                            [position.x, position.y],
                            size,
                        ))),
                    );
                }
            }
            _ => {}
        }
        (event::Status::Ignored, None)
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let size = bounds.size();
        let mut frame = Frame::new(renderer, size);
        let camera = self.camera;
        let to_screen = |point: [f64; 2]| {
            let [x, y] = camera.to_screen(point, size);
            UiPoint::new(x as f32, y as f32)
        };
        let ink = crate::drawing_view::ink([0, 0, 0]);
        for note in self.notes {
            let selected = self.tool.selected.as_deref() == Some(note.id());
            let mut shown = note.clone();
            if let (true, Some((_, from, _)), Some(to)) = (selected, &state.held, state.to) {
                let zero = self.frame.to_world([0.0, 0.0]);
                let moved = self.frame.to_world([
                    (to[0] - from[0]) / self.factor,
                    (to[1] - from[1]) / self.factor,
                ]);
                let shift: [f64; 3] = std::array::from_fn(|axis| moved[axis] - zero[axis]);
                shown = note.dragged(shift, &self.frame);
            }
            let color = if selected { ACTIVE } else { ink };
            draw_marks(
                &mut frame,
                &self.marks(&shown),
                to_screen,
                camera.scale,
                color,
            );
        }
        // What is being placed: the points clicked, and the annotation as it
        // would be with the pointer as its next point.
        if let Some(kind) = self.tool.kind {
            let pointer = cursor
                .position_in(bounds)
                .map(|at| self.picked_at(at, size));
            let mut points: Vec<[f64; 2]> = self
                .tool
                .picked
                .iter()
                .map(|point| self.uv(*point))
                .collect();
            if let (Some((at, true)), None) = (pointer, &self.tool.typing) {
                let marker = to_screen(at);
                frame.stroke(
                    &canvas::Path::rectangle(
                        UiPoint::new(marker.x - 5.0, marker.y - 5.0),
                        Size::new(10.0, 10.0),
                    ),
                    canvas::Stroke::default().with_color(ACTIVE).with_width(1.5),
                );
            }
            if self.tool.typing.is_none() {
                if let Some((at, _)) = pointer {
                    points.push(at);
                }
            }
            let height = self.tool.text_height() * self.scale / 1000.0 * self.factor;
            let preview: Vec<NoteMark> = match (kind, points.as_slice()) {
                (NoteKind::Dimension, [a, b, c, ..]) => {
                    let offset = dimension_axes(*a, *b)
                        .map(|(normal, _)| (c[0] - b[0]) * normal[0] + (c[1] - b[1]) * normal[1])
                        .unwrap_or_default();
                    dimension_shape(*a, *b, offset, height)
                        .map(|shape| {
                            shape
                                .lines
                                .iter()
                                .map(|line| NoteMark::Line(line.to_vec()))
                                .collect()
                        })
                        .unwrap_or_default()
                }
                (NoteKind::Leader, [a, b, ..]) => {
                    let shape = leader_shape(*a, *b, height);
                    vec![
                        NoteMark::Line(shape.line),
                        NoteMark::Fill(shape.arrow.to_vec()),
                    ]
                }
                (_, [a, b, ..]) => vec![NoteMark::Line(vec![*a, *b])],
                _ => Vec::new(),
            };
            draw_marks(&mut frame, &preview, to_screen, camera.scale, ACTIVE);
            for point in self.tool.picked.iter().map(|point| self.uv(*point)) {
                let at = to_screen(point);
                frame.fill_rectangle(
                    UiPoint::new(at.x - 3.0, at.y - 3.0),
                    Size::new(6.0, 6.0),
                    ACTIVE,
                );
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if !cursor.is_over(bounds) {
            return mouse::Interaction::None;
        }
        if state.to.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if self.tool.kind.is_some() && self.tool.typing.is_none() {
            return mouse::Interaction::Crosshair;
        }
        let over = cursor
            .position_in(bounds)
            .and_then(|at| self.note_at(at, bounds.size()));
        if over.is_some() {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
        }
    }
}

impl Studio {
    /// What `status.result.drawing_annotations` reports: the tool, what is
    /// being placed, the selected annotation and those of the drawing shown.
    pub(crate) fn notes_value(&self) -> Value {
        let target = match self.note_target() {
            Some(NoteTarget::Drawing(guid)) => json!({"drawing": guid}),
            Some(NoteTarget::Sheet(guid)) => json!({"sheet": guid}),
            None => Value::Null,
        };
        json!({
            "tool": self.notes.kind.map(NoteKind::key),
            "picked": self.notes.picked.len(),
            "typing": self.notes.typing.is_some(),
            "selected": self.notes.selected,
            "text_height": self.notes.text_height(),
            "target": target,
            "annotations": self
                .drawing_view
                .shown_guid()
                .map(|guid| self.drawing_notes_value(guid)),
            "export_pending": self.notes.export_pending,
        })
    }

    /// The layer of annotations over the drawing shown in the Drawing view.
    pub(crate) fn notes_layer(&self) -> Option<Element<'_, Message>> {
        let guid = self.drawing_view.shown_guid()?;
        let definition = self.definition(guid)?;
        let scene = self.drawing_view.scene()?;
        let frame = drawing_frame(definition)?;
        let shown_layers = (0..scene.layers.len())
            .map(|index| self.drawing_view.layer_shown(index))
            .collect();
        Some(
            Canvas::new(NotesLayer {
                tool: &self.notes,
                scene,
                shown_layers,
                notes: &definition.annotations,
                frame,
                factor: scene.units.factor(),
                scale: drawing_scale(definition),
                camera: self.drawing_view.camera(),
            })
            .width(Fill)
            .height(Fill)
            .into(),
        )
    }

    /// The field for the text of a text or a leader whose points are
    /// clicked, over the top of the main area.
    pub(crate) fn notes_prompt(&self) -> Option<Element<'_, Message>> {
        let typing = self.notes.typing.as_ref()?;
        let ready = !typing.trim().is_empty();
        let prompt = container(
            row![
                text_input(tr("Text of the annotation"), typing)
                    .id(note_input_id())
                    .on_input(|value| Message::Notes(NoteAction::Typed(value)))
                    .on_submit(Message::Notes(NoteAction::Submit))
                    .size(12)
                    .padding([4, 6])
                    .width(300),
                button(text(tr("Add")).size(12))
                    .on_press_maybe(ready.then_some(Message::Notes(NoteAction::Submit)))
                    .style(flat_tool_style),
                button(text(tr("Cancel")).size(12))
                    .on_press(Message::Notes(NoteAction::Cancel))
                    .style(flat_tool_style),
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
                    color: ACTIVE,
                    width: 1.0,
                    radius: 4.0.into(),
                })
        });
        Some(container(prompt).padding(10).into())
    }

    /// The tools of the VIEWS group of the ribbon: Note and Line in the 3D
    /// view; Text, Dimension, Leader and Line on a drawing; Text and Line on
    /// a sheet. Each says in its tooltip what it does and where.
    pub(crate) fn annotation_tools(&self) -> Vec<opencad_ribbon::RibbonItem<'static>> {
        use crate::views::{AnnotationKind, ViewAction};
        let target = self.note_target();
        let on_drawing = matches!(target, Some(NoteTarget::Drawing(_)));
        let on_sheet = matches!(target, Some(NoteTarget::Sheet(_)));
        let in_3d = !self.drawing_view.shown && self.active.is_some();
        let note_tool = |kind: NoteKind| self.notes.kind == Some(kind);
        let item = |button: Element<'static, Message>, lines: Vec<String>| {
            opencad_ribbon::RibbonItem::Small(opencad_properties::explained(button, lines))
        };
        let line_message = if on_drawing || on_sheet {
            Message::Notes(NoteAction::Tool(NoteKind::Line))
        } else {
            Message::Views(ViewAction::Tool(AnnotationKind::Line))
        };
        let line_active = if on_drawing || on_sheet {
            note_tool(NoteKind::Line)
        } else {
            self.views.tool == Some(AnnotationKind::Line)
        };
        vec![
            item(
                crate::small_tool_button_when(
                    "Note",
                    Message::Views(ViewAction::Tool(AnnotationKind::Note)),
                    self.views.tool == Some(AnnotationKind::Note),
                    in_3d,
                ),
                vec![tr("A note at a point of the scan, kept with the saved view; in the 3D view").to_owned()],
            ),
            item(
                crate::small_tool_button_when(
                    "Line",
                    line_message,
                    line_active,
                    in_3d || on_drawing || on_sheet,
                ),
                vec![tr("An arrow between two points of the scan in the 3D view; a line on a drawing, snapping to its lines, or on the paper of a sheet").to_owned()],
            ),
            item(
                crate::small_tool_button_when(
                    "Text",
                    Message::Notes(NoteAction::Tool(NoteKind::Text)),
                    note_tool(NoteKind::Text),
                    on_drawing || on_sheet,
                ),
                vec![tr("A text on a plan, an elevation or a section, 2.5 mm high on the paper unless set in Properties; or on the paper of a sheet").to_owned()],
            ),
            item(
                crate::small_tool_button_when(
                    "Dimension",
                    Message::Notes(NoteAction::Tool(NoteKind::Dimension)),
                    note_tool(NoteKind::Dimension),
                    on_drawing,
                ),
                vec![tr("A dimension on a plan, an elevation or a section: two points that snap to the ends and corners of the lines, then its line; it shows the distance in millimetres").to_owned()],
            ),
            item(
                crate::small_tool_button_when(
                    "Leader",
                    Message::Notes(NoteAction::Tool(NoteKind::Leader)),
                    note_tool(NoteKind::Leader),
                    on_drawing,
                ),
                vec![tr("An arrow with a text on a plan, an elevation or a section: the point it points at, then where its text goes").to_owned()],
            ),
        ]
    }

    /// The Annotations section of Properties for the drawing shown: its
    /// annotation scale, the height of new texts, every annotation with ×,
    /// the text or value of the selected one, and Export DXF/DWG….
    pub(crate) fn drawing_notes_properties(&self) -> Option<Element<'_, Message>> {
        let guid = self.drawing_view.shown_guid()?;
        let definition = self.definition(guid)?;
        let scale = drawing_scale(definition);
        let chosen = crate::layouts::model::SCALES
            .iter()
            .find(|known| (*known - scale).abs() < 1e-9)
            .map(|known| crate::layouts::ScaleChoice(*known));
        let mut block = column![
            opencad_properties::section_header("Annotations"),
            opencad_properties::property_control(
                "Annotation scale",
                pick_list(
                    crate::layouts::model::SCALES.map(crate::layouts::ScaleChoice),
                    chosen,
                    |choice| Message::Notes(NoteAction::Scale(choice)),
                )
                .placeholder(crate::layouts::model::scale_label(scale))
                .style(crate::themed_pick_list_style)
                .text_size(11)
                .width(Fill)
                .into(),
            ),
            opencad_properties::property_input(
                "Text height (mm)",
                "2.5",
                &self.notes.height,
                |value| Message::Notes(NoteAction::Height(value)),
            ),
        ]
        .spacing(0)
        .width(Fill);
        block = block.push(
            self.note_rows(
                definition
                    .annotations
                    .iter()
                    .map(|note| (note.id().to_owned(), note.kind(), note.reading(scale)))
                    .collect(),
            ),
        );
        block = block.push(
            container(
                button(text(tr("Export DXF/DWG…")).size(11))
                    .on_press_maybe(
                        (!self.notes.export_pending && self.drawing_view.made(guid).is_some())
                            .then_some(Message::Notes(NoteAction::Export)),
                    )
                    .style(flat_tool_style),
            )
            .padding([3, 8]),
        );
        Some(block.into())
    }

    /// The annotations of the paper of the sheet shown, for Properties.
    pub(crate) fn paper_notes_properties(&self) -> Option<Element<'_, Message>> {
        let sheet = self.drawing_view.shown_layout()?;
        let layout = self.layouts.layout(sheet)?;
        let rows = layout
            .notes
            .iter()
            .map(|note| {
                let (kind, reading) = match note {
                    PaperNote::Text { value, .. } => (NoteKind::Text, value.clone()),
                    PaperNote::Line { from, to, .. } => (
                        NoteKind::Line,
                        format!("{:.0} mm", (to[0] - from[0]).hypot(to[1] - from[1])),
                    ),
                };
                (note.id().to_owned(), kind, reading)
            })
            .collect();
        Some(
            column![
                opencad_properties::section_header("Annotations"),
                opencad_properties::property_input(
                    "Text height (mm)",
                    "2.5",
                    &self.notes.height,
                    |value| Message::Notes(NoteAction::Height(value)),
                ),
                self.note_rows(rows),
            ]
            .spacing(0)
            .width(Fill)
            .into(),
        )
    }

    /// A row per annotation, which a click selects, with ×; under the
    /// selected one the field for its text or value.
    fn note_rows(&self, rows: Vec<(String, NoteKind, String)>) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let mut list = column![].spacing(1).padding([2, 8]);
        if rows.is_empty() {
            list = list.push(
                text(tr(
                    "Place a Text, a Dimension, a Leader or a Line from the ribbon",
                ))
                .size(10)
                .color(colors.muted),
            );
        }
        for (id, kind, reading) in rows {
            let selected = self.notes.selected.as_deref() == Some(id.as_str());
            let label = format!(
                "{} · {}",
                tr(kind_name(kind)),
                crate::view_tabs::shortened(&reading, 28)
            );
            list = list.push(
                row![
                    button(text(label).size(11))
                        .on_press(Message::Notes(NoteAction::Select(Some(id.clone()))))
                        .style(move |theme, status| {
                            opencad_ribbon::tool_btn_style(theme, selected, status)
                        })
                        .padding([2, 4])
                        .width(Fill),
                    crate::project_browser::remove_button(Message::Notes(NoteAction::Delete(
                        id.clone()
                    ))),
                ]
                .spacing(2)
                .align_y(iced::Alignment::Center),
            );
            if selected && kind != NoteKind::Line {
                let typed = match &self.notes.edit {
                    Some((edited, value)) if *edited == id => value.clone(),
                    _ => reading.clone(),
                };
                let id = id.clone();
                list = list.push(
                    text_input(
                        if kind == NoteKind::Dimension {
                            tr("Value; empty for the measured one")
                        } else {
                            tr("Text")
                        },
                        &typed,
                    )
                    .on_input(move |value| Message::Notes(NoteAction::Edit(id.clone(), value)))
                    .on_submit(Message::Notes(NoteAction::ApplyEdit))
                    .size(11)
                    .padding([2, 4]),
                );
            }
        }
        list.into()
    }

    /// The `annotate_drawing` command of the local API.
    pub(crate) fn api_annotate_drawing(&mut self, options: &AnnotateOptions) -> Value {
        let refuse = |error: String| json!({"ok": false, "error": error});
        let guid = match options.name.as_deref() {
            Some(name) => match self.drawing_named(name) {
                Some(guid) => guid,
                None => return refuse(format!("no drawing {name} of an open scan")),
            },
            None => match self.drawing_view.shown_guid() {
                Some(guid) => guid.to_owned(),
                None => return refuse("no drawing is shown; name one with name".into()),
            },
        };
        let Some(kind) = NoteKind::from_key(&options.kind) else {
            return refuse("kind must be text, dimension, leader or line".into());
        };
        let Some(definition) = self.definition(&guid) else {
            return refuse("That drawing is no longer kept".into());
        };
        let Some(frame) = drawing_frame(definition) else {
            return refuse("The drawing has no plane".into());
        };
        let factor = definition
            .request()
            .map_or(1000.0, |request| request.units.factor());
        let scale = drawing_scale(definition);
        let height = options.height.unwrap_or(DEFAULT_TEXT_HEIGHT);
        // A point snaps to an end or a corner of a line within 0.25 m.
        let snaps = match (options.snap, self.drawing_view.made(&guid)) {
            (false, _) | (_, None) => Vec::new(),
            (true, Some(scene)) => {
                let shown: Vec<bool> = vec![true; scene.layers.len()];
                snap_points(scene, &shown)
            }
        };
        let take = |at: [f64; 2]| -> [f64; 3] {
            let at = snapped(&snaps, at, 0.25 * factor).unwrap_or(at);
            frame.to_world([at[0] / factor, at[1] / factor])
        };
        let needs = |point: Option<[f64; 2]>, name: &str| {
            point.ok_or_else(|| format!("a {} needs {name}", kind.key()))
        };
        let value = options.text.clone().unwrap_or_default();
        let id = crate::camera_views::new_guid();
        let note = (|| -> Result<DrawingNote, String> {
            Ok(match kind {
                NoteKind::Text => DrawingNote::Text {
                    id,
                    at: frame.to_world({
                        let at = needs(options.at, "at")?;
                        [at[0] / factor, at[1] / factor]
                    }),
                    height,
                    value,
                },
                NoteKind::Line => DrawingNote::Line {
                    id,
                    from: take(needs(options.from, "from")?),
                    to: take(needs(options.to, "to")?),
                },
                NoteKind::Leader => DrawingNote::Leader {
                    id,
                    from: take(needs(options.from, "from")?),
                    to: frame.to_world({
                        let to = needs(options.to, "to")?;
                        [to[0] / factor, to[1] / factor]
                    }),
                    height,
                    value,
                },
                NoteKind::Dimension => DrawingNote::Dimension {
                    id,
                    from: take(needs(options.from, "from")?),
                    to: take(needs(options.to, "to")?),
                    offset: options.offset.unwrap_or(-factor) / factor,
                    height,
                    text: options.text.clone().filter(|text| !text.trim().is_empty()),
                },
            })
        })();
        let note = match note {
            Ok(note) => note,
            Err(error) => return refuse(error),
        };
        let reading = note.reading(scale);
        let points = note_points(&note, &frame, factor);
        match self.add_drawing_note(&guid, note) {
            Ok(id) => json!({
                "ok": true,
                "id": id,
                "kind": kind.key(),
                "reading": reading,
                "points": points,
                "annotations": self.drawing_notes_value(&guid),
            }),
            Err(error) => refuse(error),
        }
    }

    /// The annotations of a drawing as the local API reports them, in its
    /// units and coordinates.
    pub(crate) fn drawing_notes_value(&self, guid: &str) -> Value {
        let Some(definition) = self.definition(guid) else {
            return json!([]);
        };
        let Some(frame) = drawing_frame(definition) else {
            return json!([]);
        };
        let factor = definition
            .request()
            .map_or(1000.0, |request| request.units.factor());
        let scale = drawing_scale(definition);
        definition
            .annotations
            .iter()
            .map(|note| {
                let mut value = json!({
                    "id": note.id(),
                    "kind": note.kind().key(),
                    "reading": note.reading(scale),
                    "points": note_points(note, &frame, factor),
                });
                if let Some(metres) = note.measured() {
                    value["measured_mm"] = json!((metres * 1000.0 * 10.0).round() / 10.0);
                }
                value
            })
            .collect()
    }

    /// The `annotate_sheet` command of the local API.
    pub(crate) fn api_annotate_sheet(&mut self, options: &AnnotateOptions) -> Value {
        let refuse = |error: String| json!({"ok": false, "error": error});
        let sheet = match self.sheet_asked(options.sheet.as_deref()) {
            Ok(sheet) => sheet,
            Err(error) => return refuse(error),
        };
        let id = crate::camera_views::new_guid();
        let note = match (
            NoteKind::from_key(&options.kind),
            options.at,
            options.from,
            options.to,
        ) {
            (Some(NoteKind::Text), Some(at), _, _) => PaperNote::Text {
                id,
                at,
                height: options.height.unwrap_or(DEFAULT_TEXT_HEIGHT),
                value: options.text.clone().unwrap_or_default(),
            },
            (Some(NoteKind::Line), _, Some(from), Some(to)) => PaperNote::Line { id, from, to },
            (Some(NoteKind::Text | NoteKind::Line), ..) => {
                return refuse("a text needs at and text, a line from and to".into())
            }
            _ => return refuse("kind must be text or line on a sheet".into()),
        };
        match self.add_paper_note(&sheet, note) {
            Ok(id) => {
                let notes = self
                    .layouts
                    .layout(&sheet)
                    .map_or_else(Vec::new, |layout| layout.notes.clone());
                json!({"ok": true, "id": id, "notes": notes})
            }
            Err(error) => refuse(error),
        }
    }

    /// The `delete_annotation` command of the local API with an `id`: an
    /// annotation of a drawing or of the paper of a sheet.
    pub(crate) fn api_delete_note(
        &mut self,
        id: &str,
        drawing: Option<&str>,
        sheet: Option<&str>,
    ) -> Value {
        let refuse = |error: String| json!({"ok": false, "error": error});
        let target = if let Some(name) = drawing {
            match self.drawing_named(name) {
                Some(guid) => NoteTarget::Drawing(guid),
                None => return refuse(format!("no drawing {name} of an open scan")),
            }
        } else if let Some(name) = sheet {
            match self.sheet_asked(Some(name)) {
                Ok(guid) => NoteTarget::Sheet(guid),
                Err(error) => return refuse(error),
            }
        } else {
            // The drawing or the sheet that holds it.
            let drawing = self
                .drawing_view
                .saved
                .iter()
                .find(|drawing| drawing.annotations.iter().any(|note| note.id() == id))
                .map(|drawing| NoteTarget::Drawing(drawing.guid.clone()));
            let sheet = self
                .layouts
                .list
                .iter()
                .find(|layout| layout.notes.iter().any(|note| note.id() == id))
                .map(|layout| NoteTarget::Sheet(layout.guid.clone()));
            match drawing.or(sheet) {
                Some(target) => target,
                None => return refuse(format!("no annotation {id}")),
            }
        };
        match self.delete_note(&target, id) {
            Ok(()) => json!({"ok": true, "deleted": id}),
            Err(error) => refuse(error),
        }
    }

    /// The `export_drawing_file` command of the local API.
    pub(crate) fn api_export_drawing_file(
        &mut self,
        name: Option<&str>,
        path: PathBuf,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        if !path.is_absolute() || DrawingFormat::from_path(&path).is_none() {
            return refuse("export_drawing_file requires an absolute .dxf or .dwg path".into());
        }
        if self.notes.export_pending {
            return refuse("a drawing is being written; wait for it".into());
        }
        let guid = match name {
            Some(name) => match self.drawing_named(name) {
                Some(guid) => guid,
                None => return refuse(format!("no drawing {name} of an open scan")),
            },
            None => match self.drawing_view.shown_guid() {
                Some(guid) => guid.to_owned(),
                None => return refuse("no drawing is shown; name one with name".into()),
            },
        };
        let id =
            self.record_api_job(json!({"state": "running", "operation": "export_drawing_file"}));
        match self.start_drawing_file(&guid, path, Some(id.clone())) {
            Ok(task) => (json!({"ok": true, "accepted": true, "job_id": id}), task),
            Err(error) => {
                self.forget_api_job(&id);
                refuse(error)
            }
        }
    }
}

/// The points of an annotation in the units and coordinates of its
/// drawing.
fn note_points(note: &DrawingNote, frame: &DrawingFrame, factor: f64) -> Value {
    let uv = |point: &[f64; 3]| {
        let at = frame.to_uv(*point);
        [at[0] * factor, at[1] * factor]
    };
    match note {
        DrawingNote::Text { at, .. } => json!([uv(at)]),
        DrawingNote::Line { from, to, .. } | DrawingNote::Leader { from, to, .. } => {
            json!([uv(from), uv(to)])
        }
        DrawingNote::Dimension {
            from, to, offset, ..
        } => json!({"from": uv(from), "to": uv(to), "offset": offset * factor}),
    }
}

/// What `annotate_drawing` and `annotate_sheet` of the local API place.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AnnotateOptions {
    /// The drawing by its name; without it the drawing shown.
    #[serde(default)]
    pub name: Option<String>,
    /// The sheet by its guid, number or name; without it the sheet shown.
    #[serde(default)]
    pub sheet: Option<String>,
    /// `text`, `dimension`, `leader` or `line`.
    pub kind: String,
    #[serde(default)]
    pub at: Option<[f64; 2]>,
    #[serde(default)]
    pub from: Option<[f64; 2]>,
    #[serde(default)]
    pub to: Option<[f64; 2]>,
    /// The offset of the line of a dimension from its points, in the units
    /// of the drawing, to the left of the direction from `from` to `to`.
    #[serde(default)]
    pub offset: Option<f64>,
    #[serde(default)]
    pub text: Option<String>,
    /// Millimetres on the paper.
    #[serde(default)]
    pub height: Option<f64>,
    /// Whether the points of a dimension, a leader and a line snap to the
    /// ends and corners of the lines drawn within 0.25 m.
    #[serde(default = "snapping")]
    pub snap: bool,
}

fn snapping() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pointcloud_core::{Bounds, DrawingRequest, DrawingUnits, DrawingView, OrientedBox};
    use serde_json::{json, Value};

    use super::*;
    use crate::camera_views;
    use crate::drawing_view::DrawingSource;
    use crate::native_api::{ApiCommand, ApiRequest};
    use crate::sheet_dialog::SheetKind;

    fn studio_with_scan() -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let path = directory.path().join("office.xyz");
        std::fs::write(&path, "0 0 0\n12 0 0\n12 8 0\n0 8 3\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        (studio, directory)
    }

    fn send(studio: &mut Studio, body: Value) -> Value {
        let command: ApiCommand = serde_json::from_value(body).unwrap();
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    /// A plan of the box from (0, 0) to (12, 8) in millimetres, made and
    /// shown, with a room outline whose corners lie at (1, 1), (4.4512, 1)
    /// and (4.4512, 3) metres.
    fn shown_plan(studio: &mut Studio) -> String {
        let sources = studio.open_sources();
        let definition = SavedDrawing::new(
            "Plan +1.20",
            SheetKind::Plan,
            OrientedBox::new(
                Bounds {
                    min: [0.0, 0.0, 0.0],
                    max: [12.0, 8.0, 1.2],
                },
                0.0,
            ),
            &DrawingRequest::for_view(DrawingView::Plan),
            sources,
        );
        let guid = definition.guid.clone();
        studio.keep_saved_drawing(definition);
        let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
        let outline = drawing
            .layer("OPS-CUT-OUTLINE", LAYER_RGB_CONTRAST)
            .unwrap();
        drawing.add_polyline(
            outline,
            vec![[1.0, 1.0], [4.4512, 1.0], [4.4512, 3.0]],
            false,
        );
        let scene = DrawScene::from_drawing(
            &drawing,
            DrawingSource::Sheet {
                guid: guid.clone(),
                name: "Plan +1.20".into(),
            },
        );
        studio.drawing_view.keep_made(Arc::new(scene));
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            guid.clone(),
        )));
        assert_eq!(studio.drawing_view.shown_guid(), Some(guid.as_str()));
        guid
    }

    fn notes(studio: &Studio, guid: &str) -> Vec<DrawingNote> {
        studio.definition(guid).unwrap().annotations.clone()
    }

    #[test]
    fn a_dimension_snaps_to_the_corners_and_shows_the_distance_in_millimetres() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let (mut studio, _directory) = studio_with_scan();
        let guid = shown_plan(&mut studio);
        let act = |studio: &mut Studio, action: NoteAction| {
            let _ = studio.update(Message::Notes(action));
        };
        act(&mut studio, NoteAction::Tool(NoteKind::Dimension));
        assert_eq!(studio.notes.kind, Some(NoteKind::Dimension));
        // The canvas snaps the clicks: near (1, 1) and (4.45, 1) in mm.
        let scene = studio.drawing_view.made(&guid).unwrap().clone();
        let points = snap_points(&scene, &[true]);
        let first = snapped(&points, [1003.0, 996.0], 10.0).unwrap();
        let second = snapped(&points, [4449.0, 1004.0], 10.0).unwrap();
        assert_eq!((first, second), ([1000.0, 1000.0], [4451.2, 1000.0]));
        assert_eq!(snapped(&points, [2500.0, 1500.0], 10.0), None);
        act(&mut studio, NoteAction::Pick(first));
        act(&mut studio, NoteAction::Pick(second));
        act(&mut studio, NoteAction::Pick([3000.0, 200.0]));
        let placed = notes(&studio, &guid);
        assert_eq!(placed.len(), 1);
        let DrawingNote::Dimension {
            offset, from, to, ..
        } = &placed[0]
        else {
            panic!("{placed:?}");
        };
        assert!((offset + 0.8).abs() < 1e-9, "{offset}");
        assert_eq!(*from, [1.0, 1.0, 1.2]);
        assert!((to[0] - 4.4512).abs() < 1e-9);
        // 3.4512 m at 1:100 reads 3450; at 1:50, 3450 as well, and at 1:20
        // to the millimetre.
        assert_eq!(placed[0].reading(100.0), "3450");
        assert_eq!(placed[0].reading(20.0), "3451");
        // A value typed takes the place of the measured one, and empty
        // gives it back.
        let id = placed[0].id().to_owned();
        act(
            &mut studio,
            NoteAction::Edit(id.clone(), "3500 approx.".into()),
        );
        act(&mut studio, NoteAction::ApplyEdit);
        assert_eq!(notes(&studio, &guid)[0].reading(100.0), "3500 approx.");
        act(&mut studio, NoteAction::Edit(id.clone(), String::new()));
        act(&mut studio, NoteAction::ApplyEdit);
        assert_eq!(notes(&studio, &guid)[0].reading(100.0), "3450");

        // A text: a click, then its text.
        act(&mut studio, NoteAction::Tool(NoteKind::Text));
        act(&mut studio, NoteAction::Pick([2000.0, 2000.0]));
        assert!(studio.notes.typing.is_some());
        act(&mut studio, NoteAction::Typed("Office 0.01".into()));
        act(&mut studio, NoteAction::Submit);
        let placed = notes(&studio, &guid);
        assert_eq!(placed.len(), 2);
        assert!(
            matches!(&placed[1], DrawingNote::Text { value, height, .. } if value == "Office 0.01" && *height == 2.5)
        );
        // Dragged, the text moves and the dimension moves its line.
        let text_id = placed[1].id().to_owned();
        act(&mut studio, NoteAction::Moved(text_id, [500.0, -250.0]));
        let DrawingNote::Text { at, .. } = &notes(&studio, &guid)[1] else {
            panic!();
        };
        assert!((at[0] - 2.5).abs() < 1e-9 && (at[1] - 1.75).abs() < 1e-9);
        act(&mut studio, NoteAction::Moved(id.clone(), [0.0, -200.0]));
        let DrawingNote::Dimension { offset, .. } = &notes(&studio, &guid)[0] else {
            panic!();
        };
        assert!((offset + 1.0).abs() < 1e-9, "{offset}");
        // Escape leaves the tool, Delete deletes the selected annotation.
        let _ = studio.update(Message::Escape);
        let _ = studio.update(Message::Notes(NoteAction::Select(Some(id))));
        let _ = studio.update(Message::NamedKey(iced::keyboard::key::Named::Delete, true));
        assert_eq!(notes(&studio, &guid).len(), 1);
        // The annotations are kept with the drawing.
        let kept = crate::saved_drawings::load();
        assert_eq!(kept[0].annotations, notes(&studio, &guid));
        let _ = studio.view();
    }

    #[test]
    fn annotations_stay_at_their_model_points_when_the_drawing_is_made_again() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let (mut studio, _directory) = studio_with_scan();
        let guid = shown_plan(&mut studio);
        let added = send(
            &mut studio,
            json!({"command": "annotate_drawing", "kind": "leader", "from": [1002, 998], "to": [2500, 2000], "text": "Brick wall"}),
        );
        assert_eq!(added["ok"], true, "{added}");
        assert_eq!(added["points"], json!([[1000.0, 1000.0], [2500.0, 2000.0]]));
        // The drawing made from a box that starts elsewhere: the frame of a
        // plan keeps the model X and Y, so the note stays at its point.
        let saved = &mut studio.drawing_view.saved;
        saved[0].section.min = [0.5, 0.5, 0.0];
        let listed = studio.drawing_notes_value(&guid);
        assert_eq!(
            listed[0]["points"],
            json!([[1000.0, 1000.0], [2500.0, 2000.0]])
        );
        // A drawing that is locked takes annotations all the same.
        let _ = send(
            &mut studio,
            json!({"command": "lock_view", "name": "Plan +1.20"}),
        );
        let text = send(
            &mut studio,
            json!({"command": "annotate_drawing", "kind": "text", "at": [6000, 5000], "text": "Hall", "height": 3.5}),
        );
        assert_eq!(text["ok"], true, "{text}");
        let line = send(
            &mut studio,
            json!({"command": "annotate_drawing", "name": "plan +1.20", "kind": "line", "from": [0, 0], "to": [100, 0], "snap": false}),
        );
        assert_eq!(line["points"], json!([[0.0, 0.0], [100.0, 0.0]]));
        let refused = send(
            &mut studio,
            json!({"command": "annotate_drawing", "kind": "dimension", "from": [0, 0]}),
        );
        assert_eq!(refused["error"], "a dimension needs to");
        let deleted = send(
            &mut studio,
            json!({"command": "delete_annotation", "id": line["id"]}),
        );
        assert_eq!(deleted["ok"], true, "{deleted}");
        assert_eq!(notes(&studio, &guid).len(), 2);
        let status = send(&mut studio, json!({"command": "status"}));
        assert_eq!(
            status["result"]["drawing_annotations"]["annotations"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        // The annotation of the active view goes by its place, as before.
        let old = send(
            &mut studio,
            json!({"command": "delete_annotation", "index": 0}),
        );
        assert_eq!(old["ok"], false);
    }

    #[test]
    fn what_is_set_on_a_drawing_while_it_is_made_again_stays_once_it_is_made() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        // A room whose walls the slab under the top of the box cuts.
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let path = directory.path().join("room.xyz");
        let mut points = Vec::new();
        for step in 0..=200 {
            let along = 1.0 + f64::from(step) * 0.05;
            for z in [0.5, 1.15] {
                points.push(format!("{along} 1 {z}"));
                points.push(format!("{along} 7 {z}"));
                if along <= 7.0 {
                    points.push(format!("1 {along} {z}"));
                    points.push(format!("11 {along} {z}"));
                }
            }
        }
        let points = points.join("\n");
        std::fs::write(&path, points).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let guid = shown_plan(&mut studio);
        // A handle of the crop region dragged: the drawing is made again.
        let changed = send(
            &mut studio,
            json!({"command": "set_sheet_crop", "name": "Plan +1.20", "width": 6.0}),
        );
        assert_eq!(changed["accepted"], true, "{changed}");
        assert!(studio.drawing_view.remake.is_some());
        // Meanwhile an annotation is placed, the drawing locked and its
        // annotation scale set.
        let added = send(
            &mut studio,
            json!({"command": "annotate_drawing", "kind": "text", "at": [2000, 2000], "text": "Hall"}),
        );
        assert_eq!(added["ok"], true, "{added}");
        let locked = send(
            &mut studio,
            json!({"command": "lock_view", "name": "Plan +1.20"}),
        );
        assert_eq!(locked["locked"], true, "{locked}");
        studio.set_drawing_scale(&guid, 50.0).unwrap();
        studio.finish_drawing_job();
        assert!(studio.drawing_view.remake.is_none(), "{}", studio.status);
        // The drawing has the crop region of the job, and keeps what was set
        // on it, here and on disk.
        for kept in [
            studio.definition(&guid).unwrap().clone(),
            crate::saved_drawings::load()
                .into_iter()
                .find(|drawing| drawing.guid == guid)
                .unwrap(),
        ] {
            let width = kept.section.max[0] - kept.section.min[0];
            assert!((width - 6.0).abs() < 1e-6, "{width}: {}", studio.status);
            assert_eq!(kept.annotations.len(), 1);
            assert_eq!(kept.annotations[0].id(), added["id"].as_str().unwrap());
            assert!(kept.locked);
            assert_eq!(kept.scale, Some(50.0));
            assert_eq!(kept.name, "Plan +1.20");
        }
    }

    #[test]
    fn a_drawing_with_its_annotations_is_written_with_real_dimensions() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        use cadcodec_check::dimensions_in;

        let (mut studio, directory) = studio_with_scan();
        let guid = shown_plan(&mut studio);
        for body in [
            json!({"command": "annotate_drawing", "kind": "dimension", "from": [1000, 1000], "to": [4451.2, 1000], "offset": -800}),
            json!({"command": "annotate_drawing", "kind": "dimension", "from": [4451.2, 1000], "to": [4451.2, 3000], "offset": -500}),
            json!({"command": "annotate_drawing", "kind": "text", "at": [2000, 2000], "text": "Office 0.01"}),
            json!({"command": "annotate_drawing", "kind": "leader", "from": [1000, 1000], "to": [2000, 2600], "text": "Brick"}),
        ] {
            assert_eq!(send(&mut studio, body.clone())["ok"], true, "{body}");
        }
        let drawing = studio.drawing_with_notes(&guid).unwrap();
        let path = directory.path().join("plan.dxf");
        pointcloud_core::write_drawing(
            &drawing,
            &path,
            DrawingFormat::Dxf,
            pointcloud_core::DrawingVersion::default(),
        )
        .unwrap();
        // The file holds two aligned dimensions on the dimension layer, at
        // the points placed, in millimetres.
        let found = dimensions_in(&path);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0, LAYER_DIMENSIONS);
        assert!((found[0].1[0] - 1000.0).abs() < 1e-6 && (found[0].2[0] - 4451.2).abs() < 1e-6);
        // Read back with the reader of the application, the values show.
        let read = pointcloud_core::read_drawing(&path).unwrap().drawing;
        let texts: Vec<String> = read
            .entities
            .iter()
            .filter_map(|(_, entity)| match entity {
                DrawingEntity::Text { value, .. } => Some(value.clone()),
                _ => None,
            })
            .collect();
        for expected in ["3450", "2000", "Office 0.01", "Brick"] {
            assert!(
                texts.iter().any(|text| text == expected),
                "{expected}: {texts:?}"
            );
        }
        // The local API writes it as a job.
        let job = send(
            &mut studio,
            json!({"command": "export_drawing_file", "path": directory.path().join("plan.dwg")}),
        );
        assert_eq!(job["accepted"], true, "{job}");
        let refused = send(
            &mut studio,
            json!({"command": "export_drawing_file", "path": "plan.dxf"}),
        );
        assert_eq!(refused["ok"], false);
    }

    #[test]
    fn a_sheet_shows_the_annotations_of_a_drawing_at_scale_and_its_own_notes() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        use crate::layouts::{Paper, PlacedKind};

        let (mut studio, _directory) = studio_with_scan();
        let guid = shown_plan(&mut studio);
        let _ = send(
            &mut studio,
            json!({"command": "annotate_drawing", "kind": "dimension", "from": [1000, 1000], "to": [4451.2, 1000], "offset": -800}),
        );
        let sheet = studio
            .create_layout("01", "Plans", Paper::A3, true)
            .unwrap();
        let _ = studio.show_layout(&sheet);
        studio
            .place_on_layout(
                &sheet,
                PlacedKind::Drawing,
                &guid,
                Some([150.0, 150.0]),
                Some(200.0),
            )
            .unwrap();
        let text = send(
            &mut studio,
            json!({"command": "annotate_sheet", "kind": "text", "at": [30, 270], "text": "Survey of 6 October", "height": 5}),
        );
        assert_eq!(text["ok"], true, "{text}");
        let line = send(
            &mut studio,
            json!({"command": "annotate_sheet", "kind": "line", "from": [30, 265], "to": [120, 265]}),
        );
        assert_eq!(line["notes"].as_array().unwrap().len(), 2);
        let plot = studio.shown_plot().unwrap();
        let texts: Vec<&str> = plot.texts().collect();
        // The dimension keeps the value of its drawing at 1:200, and its
        // text the height it has on paper.
        assert!(texts.contains(&"3450"), "{texts:?}");
        assert!(texts.contains(&"Survey of 6 October"));
        let height = plot.marks().find_map(|(mark, _)| match mark {
            crate::layouts::plot::Mark::Text { value, height, .. } if value == "3450" => {
                Some(*height)
            }
            _ => None,
        });
        assert_eq!(height, Some(2.5));
        // Clicks of the sheet tools place a text and a line on the paper.
        let act = |studio: &mut Studio, action: NoteAction| {
            let _ = studio.update(Message::Notes(action));
        };
        act(&mut studio, NoteAction::Tool(NoteKind::Dimension));
        assert_eq!(studio.notes.kind, None, "no dimension on the paper");
        act(&mut studio, NoteAction::Tool(NoteKind::Line));
        act(&mut studio, NoteAction::Pick([200.0, 50.0]));
        act(&mut studio, NoteAction::Pick([260.0, 50.0]));
        act(&mut studio, NoteAction::Tool(NoteKind::Text));
        act(&mut studio, NoteAction::Pick([200.0, 60.0]));
        act(&mut studio, NoteAction::Typed("North".into()));
        act(&mut studio, NoteAction::Submit);
        let notes = &studio.layouts.list[0].notes;
        assert_eq!(notes.len(), 4);
        let id = notes[3].id().to_owned();
        act(&mut studio, NoteAction::Moved(id.clone(), [10.0, 5.0]));
        assert!(
            matches!(&studio.layouts.list[0].notes[3], PaperNote::Text { at, .. } if *at == [210.0, 65.0])
        );
        let deleted = send(
            &mut studio,
            json!({"command": "delete_annotation", "sheet": "01", "id": id}),
        );
        assert_eq!(deleted["ok"], true);
        let _ = studio.view();
    }

    #[test]
    fn the_layer_of_the_drawing_view_snaps_clicks_selects_and_drags_annotations() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        use iced::widget::canvas::Program;
        use iced::{mouse, Point as UiPoint, Rectangle, Size};

        let (mut studio, _directory) = studio_with_scan();
        let guid = shown_plan(&mut studio);
        let _ = send(
            &mut studio,
            json!({"command": "annotate_drawing", "kind": "text", "at": [6000, 4000], "text": "Hall"}),
        );
        let definition = studio.definition(&guid).unwrap().clone();
        let scene = studio.drawing_view.made(&guid).unwrap().clone();
        let bounds = Rectangle::new(UiPoint::ORIGIN, Size::new(800.0, 600.0));
        // A tenth of a pixel a millimetre.
        let camera = ViewCamera {
            center: [3000.0, 2000.0],
            scale: 0.1,
        };
        let at = |x: f32, y: f32| mouse::Cursor::Available(UiPoint::new(x, y));
        let press = canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        fn layer<'a>(
            tool: &'a NoteTool,
            scene: &'a DrawScene,
            definition: &'a SavedDrawing,
            camera: ViewCamera,
        ) -> NotesLayer<'a> {
            NotesLayer {
                tool,
                scene,
                shown_layers: vec![true],
                notes: &definition.annotations,
                frame: drawing_frame(definition).unwrap(),
                factor: 1000.0,
                scale: 100.0,
                camera,
            }
        }
        // With the dimension tool, a click 4 pixels beside the corner at
        // (1000, 1000) mm takes the corner.
        let mut tool = NoteTool {
            kind: Some(NoteKind::Dimension),
            ..NoteTool::default()
        };
        let mut state = NotesState::default();
        let corner = camera.to_screen([1000.0, 1000.0], bounds.size());
        let (status, message) = layer(&tool, &scene, &definition, camera).update(
            &mut state,
            press.clone(),
            bounds,
            at(corner[0] as f32 + 3.0, corner[1] as f32 - 2.0),
        );
        assert_eq!(status, event::Status::Captured);
        assert!(matches!(
            message,
            Some(Message::Notes(NoteAction::Pick(point))) if point == [1000.0, 1000.0]
        ));
        assert_eq!(
            layer(&tool, &scene, &definition, camera).mouse_interaction(
                &state,
                bounds,
                at(10.0, 10.0)
            ),
            mouse::Interaction::Crosshair
        );
        // Without a tool, a press on the text selects it and a drag moves
        // it; a press beside it goes on to the sheet below.
        tool.kind = None;
        let text = camera.to_screen([6100.0, 4050.0], bounds.size());
        let (status, message) = layer(&tool, &scene, &definition, camera).update(
            &mut state,
            press.clone(),
            bounds,
            at(text[0] as f32, text[1] as f32),
        );
        assert_eq!(status, event::Status::Captured);
        let Some(Message::Notes(NoteAction::Select(Some(id)))) = message else {
            panic!("{message:?}");
        };
        let _ = layer(&tool, &scene, &definition, camera).update(
            &mut state,
            canvas::Event::Mouse(mouse::Event::CursorMoved {
                position: UiPoint::new(text[0] as f32 + 50.0, text[1] as f32),
            }),
            bounds,
            at(text[0] as f32 + 50.0, text[1] as f32),
        );
        let (_, message) = layer(&tool, &scene, &definition, camera).update(
            &mut state,
            release,
            bounds,
            at(text[0] as f32 + 50.0, text[1] as f32),
        );
        assert!(matches!(
            message,
            Some(Message::Notes(NoteAction::Moved(moved, delta)))
                if moved == id && (delta[0] - 500.0).abs() < 1e-6 && delta[1].abs() < 1e-6
        ));
        let (status, message) = layer(&tool, &scene, &definition, camera).update(
            &mut state,
            press,
            bounds,
            at(100.0, 550.0),
        );
        assert_eq!(status, event::Status::Ignored);
        assert!(message.is_none());
        let _ = studio.view();
    }

    /// Reading DIMENSION entities back with the codec the writer uses.
    mod cadcodec_check {
        use std::path::Path;

        /// Each aligned dimension of a DXF file: its layer, its two points.
        pub fn dimensions_in(path: &Path) -> Vec<(String, [f64; 2], [f64; 2])> {
            let text = std::fs::read_to_string(path).unwrap();
            let lines: Vec<&str> = text.lines().map(str::trim).collect();
            let mut found = Vec::new();
            let mut at = 0;
            while at + 1 < lines.len() {
                if lines[at] == "0" && lines[at + 1] == "DIMENSION" {
                    let mut layer = String::new();
                    let mut first = [0.0; 2];
                    let mut second = [0.0; 2];
                    let mut code = at + 2;
                    while code + 1 < lines.len() && lines[code] != "0" {
                        let value = lines[code + 1];
                        match lines[code] {
                            "8" => layer = value.to_owned(),
                            "13" => first[0] = value.parse().unwrap(),
                            "23" => first[1] = value.parse().unwrap(),
                            "14" => second[0] = value.parse().unwrap(),
                            "24" => second[1] = value.parse().unwrap(),
                            _ => {}
                        }
                        code += 2;
                    }
                    found.push((layer, first, second));
                    at = code;
                } else {
                    at += 1;
                }
            }
            found
        }
    }
}
