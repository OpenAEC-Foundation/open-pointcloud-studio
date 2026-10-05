//! Distance and area measurement between exactly picked points.
//!
//! The first half is plain geometry and text formatting in scene units. The
//! second half connects it to the desktop: the tool state, viewport clicks,
//! the overlay, the Properties rows and the command API.

use std::sync::atomic::Ordering;

use iced::widget::canvas::{self, Frame};
use iced::widget::{button, column, container};
use iced::{Color, Element, Point as UiPoint, Size, Task};
use pointcloud_core::IndexedPoint;
use serde_json::{json, Value};

use crate::i18n::{key, tr};
use crate::selection::Projection;
use crate::{opencad_properties, opencad_ribbon, Message, PointViewport, Studio, ToolIcon};

/// Pixel reach of a measuring click, the same as the point-pick tool.
pub const PICK_RADIUS: f32 = 8.0;
/// Most points in one measurement, which keeps the overlay bounded.
pub const MAX_POINTS: usize = 256;
/// Segment rows listed in Properties before the rest is summarised.
const MAX_SEGMENT_ROWS: usize = 24;

type Xyz = [f64; 3];

fn sub(a: Xyz, b: Xyz) -> Xyz {
    std::array::from_fn(|axis| a[axis] - b[axis])
}

fn cross(a: Xyz, b: Xyz) -> Xyz {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn norm(vector: Xyz) -> f64 {
    vector.iter().map(|value| value * value).sum::<f64>().sqrt()
}

pub fn distance(a: Xyz, b: Xyz) -> f64 {
    norm(sub(b, a))
}

/// Length of a segment as seen from above.
pub fn horizontal_distance(a: Xyz, b: Xyz) -> f64 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// Edges of the path through the points. A closed path of three or more
/// points also returns to the first one.
pub fn edges(points: &[Xyz], closed: bool) -> Vec<(Xyz, Xyz)> {
    let mut edges: Vec<_> = points.windows(2).map(|pair| (pair[0], pair[1])).collect();
    if closed && points.len() >= 3 {
        edges.push((points[points.len() - 1], points[0]));
    }
    edges
}

pub fn segment_lengths(points: &[Xyz], closed: bool) -> Vec<f64> {
    edges(points, closed)
        .into_iter()
        .map(|(a, b)| distance(a, b))
        .collect()
}

pub fn path_length(points: &[Xyz], closed: bool) -> f64 {
    segment_lengths(points, closed).into_iter().sum()
}

/// Length of the open path as seen from above.
pub fn horizontal_length(points: &[Xyz]) -> f64 {
    edges(points, false)
        .into_iter()
        .map(|(a, b)| horizontal_distance(a, b))
        .sum()
}

/// Height of the last point above the first one.
pub fn height_difference(points: &[Xyz]) -> f64 {
    match (points.first(), points.last()) {
        (Some(first), Some(last)) => last[2] - first[2],
        _ => 0.0,
    }
}

/// Vector area of the closed polygon through the points (Newell's method).
/// Its length is the true area of a flat polygon whatever its slope, and its
/// Z component the area seen from above. Coordinates are taken relative to
/// the first point so large survey coordinates keep their precision.
pub fn vector_area(points: &[Xyz]) -> Xyz {
    let Some(&origin) = points.first() else {
        return [0.0; 3];
    };
    let mut sum = [0.0; 3];
    for pair in points[1..].windows(2) {
        let part = cross(sub(pair[0], origin), sub(pair[1], origin));
        for axis in 0..3 {
            sum[axis] += part[axis];
        }
    }
    sum.map(|value| value * 0.5)
}

pub fn area(points: &[Xyz]) -> f64 {
    norm(vector_area(points))
}

/// Area of the polygon projected on the horizontal plane.
pub fn plan_area(points: &[Xyz]) -> f64 {
    vector_area(points)[2].abs()
}

pub fn format_length(value: f64) -> String {
    format!("{value:.3} m")
}

pub fn format_area(value: f64) -> String {
    format!("{value:.3} m²")
}

/// A height difference with its sign; a difference that rounds to zero has none.
pub fn format_height(value: f64) -> String {
    let rounded = (value * 1000.0).round() / 1000.0;
    if rounded == 0.0 {
        "0.000 m".into()
    } else {
        format!("{rounded:+.3} m")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasureMode {
    Distance,
    Area,
}

impl MeasureMode {
    pub fn key(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Area => "area",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "distance" => Some(Self::Distance),
            "area" => Some(Self::Area),
            _ => None,
        }
    }

    /// The English name of the mode, translated where it is shown.
    pub fn label(self) -> &'static str {
        match self {
            Self::Distance => key("Distance"),
            Self::Area => key("Area"),
        }
    }

    /// Fewest points that make a measurement of this kind.
    pub fn min_points(self) -> usize {
        match self {
            Self::Distance => 2,
            Self::Area => 3,
        }
    }
}

/// A polyline (distance) or a closed polygon (area) through scene positions.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub mode: MeasureMode,
    pub points: Vec<Xyz>,
    pub finished: bool,
}

impl Measurement {
    fn closed(&self) -> bool {
        self.mode == MeasureMode::Area
    }

    pub fn complete(&self) -> bool {
        self.points.len() >= self.mode.min_points()
    }

    pub fn edges(&self) -> Vec<(Xyz, Xyz)> {
        edges(&self.points, self.closed())
    }

    pub fn segment_lengths(&self) -> Vec<f64> {
        segment_lengths(&self.points, self.closed())
    }

    /// Total length of the polyline, or the perimeter of the polygon.
    pub fn length(&self) -> f64 {
        path_length(&self.points, self.closed())
    }

    /// The one value the overlay names: total length, or area.
    pub fn label(&self) -> String {
        match self.mode {
            MeasureMode::Distance => format!("Total {}", format_length(self.length())),
            MeasureMode::Area => format!("Area {}", format_area(area(&self.points))),
        }
    }

    pub fn summary(&self) -> String {
        match self.mode {
            MeasureMode::Distance => format!(
                "length {} · horizontal {} · height {}",
                format_length(self.length()),
                format_length(horizontal_length(&self.points)),
                format_height(height_difference(&self.points)),
            ),
            MeasureMode::Area => format!(
                "area {} · plan {} · perimeter {}",
                format_area(area(&self.points)),
                format_area(plan_area(&self.points)),
                format_length(self.length()),
            ),
        }
    }

    /// Status line after a point was added or removed.
    pub fn progress(&self) -> String {
        let count = self.points.len();
        match self.mode {
            MeasureMode::Distance if count < 2 => {
                format!("Point {count} picked; click the next point")
            }
            MeasureMode::Distance => format!(
                "Length {} over {count} points; Enter finishes",
                format_length(self.length())
            ),
            MeasureMode::Area if count < 3 => {
                format!("Point {count} picked; an area needs 3 points")
            }
            MeasureMode::Area => format!(
                "Area {} over {count} points; Enter or a click on the first point finishes",
                format_area(area(&self.points))
            ),
        }
    }

    /// Label and value rows for the Properties panel. The labels are
    /// English and translated by the row that shows them.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![
            (key("Type"), tr(self.mode.label()).to_owned()),
            (
                key("Points"),
                if self.finished {
                    self.points.len().to_string()
                } else {
                    format!("{} · {}", self.points.len(), tr("picking"))
                },
            ),
        ];
        match self.mode {
            MeasureMode::Distance => rows.extend([
                (key("Length"), format_length(self.length())),
                (
                    key("Horizontal length"),
                    format_length(horizontal_length(&self.points)),
                ),
                (
                    key("Height difference"),
                    format_height(height_difference(&self.points)),
                ),
            ]),
            MeasureMode::Area => rows.extend([
                (key("Area"), format_area(area(&self.points))),
                (key("Plan area"), format_area(plan_area(&self.points))),
                (key("Perimeter"), format_length(self.length())),
            ]),
        }
        let lengths = self.segment_lengths();
        for (index, length) in lengths.iter().take(MAX_SEGMENT_ROWS).enumerate() {
            rows.push((
                key("Segment"),
                format!("{} · {}", index + 1, format_length(*length)),
            ));
        }
        if lengths.len() > MAX_SEGMENT_ROWS {
            rows.push((
                key("Segments"),
                format!("{} more", lengths.len() - MAX_SEGMENT_ROWS),
            ));
        }
        rows
    }

    /// The measurement and its computed values for the command API.
    pub fn value(&self) -> Value {
        let mut value = json!({
            "mode": self.mode.key(),
            "finished": self.finished,
            "points": self.points,
            "segments": self.segment_lengths(),
        });
        match self.mode {
            MeasureMode::Distance => {
                value["length"] = json!(self.length());
                value["horizontal_length"] = json!(horizontal_length(&self.points));
                value["height_difference"] = json!(height_difference(&self.points));
            }
            MeasureMode::Area => {
                value["area"] = json!(area(&self.points));
                value["plan_area"] = json!(plan_area(&self.points));
                value["perimeter"] = json!(self.length());
            }
        }
        value
    }
}

/// Part of a screen segment inside a rectangle (Liang–Barsky clipping).
pub fn clip_segment(
    a: [f32; 2],
    b: [f32; 2],
    min: [f32; 2],
    max: [f32; 2],
) -> Option<([f32; 2], [f32; 2])> {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let (mut enter, mut leave) = (0.0f32, 1.0f32);
    for axis in 0..2 {
        for (direction, room) in [
            (-delta[axis], a[axis] - min[axis]),
            (delta[axis], max[axis] - a[axis]),
        ] {
            if direction == 0.0 {
                if room < 0.0 {
                    return None;
                }
            } else if direction < 0.0 {
                enter = enter.max(room / direction);
            } else {
                leave = leave.min(room / direction);
            }
        }
    }
    (enter <= leave).then(|| {
        (
            [a[0] + delta[0] * enter, a[1] + delta[1] * enter],
            [a[0] + delta[0] * leave, a[1] + delta[1] * leave],
        )
    })
}

/// Screen segment of a scene edge. The edge is first cut off in front of the
/// eye, so one that runs past a walking camera is still drawn up to the edge
/// of the view, and then limited to the viewport and a small margin.
pub fn project_edge(projection: Projection, a: Xyz, b: Xyz, size: Size) -> Option<[UiPoint; 2]> {
    const NEAR: f64 = 0.02;
    const MARGIN: f32 = 16.0;
    let (depth_a, depth_b) = (projection.depth(a), projection.depth(b));
    if depth_a < NEAR && depth_b < NEAR {
        return None;
    }
    let at_near = || -> Xyz {
        let t = (NEAR - depth_a) / (depth_b - depth_a);
        std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * t)
    };
    let start = if depth_a < NEAR { at_near() } else { a };
    let end = if depth_b < NEAR { at_near() } else { b };
    let (start_x, start_y, _) = projection.project_unclipped(start)?;
    let (end_x, end_y, _) = projection.project_unclipped(end)?;
    let (start, end) = clip_segment(
        [start_x, start_y],
        [end_x, end_y],
        [-MARGIN; 2],
        [size.width + MARGIN, size.height + MARGIN],
    )?;
    Some([
        UiPoint::new(start[0], start[1]),
        UiPoint::new(end[0], end[1]),
    ])
}

/// What the measuring tool is doing: the active mode and the measurement
/// being picked, or the last finished one.
#[derive(Debug, Clone, Default)]
pub struct MeasureTool {
    /// Measuring mode that viewport clicks feed, if one is active.
    pub mode: Option<MeasureMode>,
    pub current: Option<Measurement>,
}

impl MeasureTool {
    /// Append a picked point. A finished measurement, or one of another
    /// kind, is replaced by a new measurement that starts at this point.
    pub fn push(&mut self, xyz: Xyz) -> Result<&Measurement, String> {
        let mode = self
            .mode
            .ok_or_else(|| "Choose Distance or Area before measuring".to_string())?;
        if !self
            .current
            .as_ref()
            .is_some_and(|open| !open.finished && open.mode == mode)
        {
            self.current = None;
        }
        let measurement = self.current.get_or_insert_with(|| Measurement {
            mode,
            points: Vec::new(),
            finished: false,
        });
        if measurement.points.len() >= MAX_POINTS {
            return Err(format!("A measurement holds at most {MAX_POINTS} points"));
        }
        measurement.points.push(xyz);
        Ok(&*measurement)
    }

    /// Drop the last point of the unfinished measurement.
    pub fn remove_last(&mut self) -> bool {
        let Some(open) = self.current.as_mut().filter(|open| !open.finished) else {
            return false;
        };
        open.points.pop();
        if open.points.is_empty() {
            self.current = None;
        }
        true
    }

    /// Finish the unfinished measurement; `None` when there is none.
    pub fn finish(&mut self) -> Result<Option<&Measurement>, String> {
        let Some(open) = self.current.as_mut().filter(|open| !open.finished) else {
            return Ok(None);
        };
        if !open.complete() {
            return Err(format!(
                "{} needs at least {} points",
                open.mode.label(),
                open.mode.min_points()
            ));
        }
        open.finished = true;
        Ok(Some(&*open))
    }

    /// Leave the measuring mode and report whether one was active. An
    /// unfinished measurement is dropped, unless `keep` is set and it has
    /// enough points to stand as a finished one.
    pub fn leave(&mut self, keep: bool) -> bool {
        let was_active = self.mode.take().is_some();
        if self.current.as_ref().is_some_and(|open| !open.finished) {
            match self.current.as_mut().filter(|open| keep && open.complete()) {
                Some(open) => open.finished = true,
                None => self.current = None,
            }
        }
        was_active
    }

    /// The current measurement for `status.result.measure`.
    pub fn value(&self) -> Value {
        self.current
            .as_ref()
            .map_or(Value::Null, Measurement::value)
    }

    /// Distance, Area and Clear for the ribbon's measure group.
    pub fn ribbon(&self) -> Element<'static, Message> {
        let mode_button = |mode: MeasureMode| {
            opencad_ribbon::RibbonItem::Large(crate::large_tool_button(
                mode.label(),
                Message::Measure(MeasureAction::Toggle(mode)),
                self.mode == Some(mode),
            ))
        };
        opencad_ribbon::render_group_items(
            "MEASURE",
            vec![
                mode_button(MeasureMode::Distance),
                mode_button(MeasureMode::Area),
                opencad_ribbon::RibbonItem::Small(crate::small_tool_button_when(
                    "Clear",
                    Message::Measure(MeasureAction::Clear),
                    false,
                    self.current.is_some(),
                )),
            ],
        )
    }

    /// The "Measure" section of the Properties panel, while there is
    /// something to show.
    pub fn properties(&self) -> Option<Element<'static, Message>> {
        if self.mode.is_none() && self.current.is_none() {
            return None;
        }
        let mut section = column![opencad_properties::section_header("Measure")].spacing(0);
        match &self.current {
            Some(measurement) => {
                for (label, value) in measurement.rows() {
                    section = section.push(opencad_properties::property_row(label, value));
                }
            }
            None => {
                section = section.push(opencad_properties::property_row(
                    "Points",
                    tr("Click the first point").into(),
                ));
            }
        }
        Some(
            section
                .push(
                    container(
                        button(tr("Clear"))
                            .on_press_maybe(
                                self.current
                                    .is_some()
                                    .then_some(Message::Measure(MeasureAction::Clear)),
                            )
                            .style(crate::flat_tool_style),
                    )
                    .padding([3, 8]),
                )
                .into(),
        )
    }
}

/// Everything the measuring tool reacts to.
#[derive(Debug, Clone)]
pub enum MeasureAction {
    /// Enter a measuring mode, or leave it when it is already active.
    Toggle(MeasureMode),
    /// Left click without a drag at a viewport position.
    Click([f32; 2], Size),
    /// Answer of the pick search started by a click, with its view revision.
    Picked(u64, Result<Option<IndexedPoint>, String>),
    RemoveLast,
    Finish,
    Clear,
}

impl MeasureAction {
    pub fn icon(&self) -> ToolIcon {
        match self {
            Self::Toggle(MeasureMode::Distance) => ToolIcon::MeasureDistance,
            Self::Toggle(MeasureMode::Area) => ToolIcon::MeasureArea,
            _ => ToolIcon::Clear,
        }
    }
}

/// Hand the answer of a pick search to the measuring tool instead of the
/// point selection.
fn picked_message(message: Message) -> Message {
    match message {
        Message::PickReady(revision, _, result) => {
            Message::Measure(MeasureAction::Picked(revision, result))
        }
        other => other,
    }
}

impl Studio {
    pub fn update_measure(&mut self, action: MeasureAction) -> Task<Message> {
        match action {
            MeasureAction::Toggle(mode) => {
                let stop = self.measure.mode == Some(mode);
                self.measure.leave(true);
                if stop {
                    self.status = "Measuring stopped".into();
                } else {
                    self.measure.mode = Some(mode);
                    self.views.leave_tool();
                    self.box_select = false;
                    self.pick_mode = false;
                    self.drag_rectangle = None;
                    self.status = format!(
                        "{} measuring active; click points, Enter finishes, Backspace removes the last point, Escape cancels",
                        mode.label()
                    );
                }
            }
            MeasureAction::Click(pointer, size) => {
                if self.measure.mode.is_none() {
                    return Task::none();
                }
                if self.click_closes_area(pointer, size) {
                    return self.update_measure(MeasureAction::Finish);
                }
                match self.start_point_pick(pointer, PICK_RADIUS, size, None) {
                    Ok(task) => return task.map(picked_message),
                    Err(error) => self.status = error,
                }
            }
            MeasureAction::Picked(revision, result) => {
                self.selection_pending = false;
                if self.selection_cancel.load(Ordering::Relaxed) {
                    self.status = "Measure pick cancelled".into();
                    return Task::none();
                }
                if revision != self.revision || self.measure.mode.is_none() {
                    self.status = "Measure pick discarded because the view changed".into();
                    return Task::none();
                }
                match result {
                    // The pick search answers in scene coordinates: the
                    // layer's live transform is already applied.
                    Ok(Some(record)) => match self.measure.push(record.point.xyz) {
                        Ok(measurement) => self.status = measurement.progress(),
                        Err(error) => self.status = error,
                    },
                    Ok(None) => self.status = format!("No point within {PICK_RADIUS} pixels"),
                    Err(error) => self.status = format!("Measure pick failed: {error}"),
                }
            }
            MeasureAction::RemoveLast => {
                if self.model_covered() || self.measure.mode.is_none() {
                    return Task::none();
                }
                if self.measure.remove_last() {
                    self.status = match &self.measure.current {
                        Some(measurement) => {
                            format!("Removed the last point · {}", measurement.progress())
                        }
                        None => "Removed the last point; click the first point".into(),
                    };
                }
            }
            MeasureAction::Finish => {
                if self.model_covered() || self.measure.mode.is_none() {
                    return Task::none();
                }
                match self.measure.finish() {
                    Ok(Some(measurement)) => {
                        self.status = format!("Measured {}", measurement.summary());
                    }
                    Ok(None) => {}
                    Err(error) => self.status = error,
                }
            }
            MeasureAction::Clear => {
                self.measure.current = None;
                self.status = "Measurement cleared".into();
            }
        }
        Task::none()
    }

    /// A click on the first point closes an area that has enough points.
    fn click_closes_area(&self, pointer: [f32; 2], size: Size) -> bool {
        let Some(open) = self
            .measure
            .current
            .as_ref()
            .filter(|open| !open.finished && open.mode == MeasureMode::Area && open.complete())
        else {
            return false;
        };
        let Some(scene) = crate::combined_bounds(&self.clouds) else {
            return false;
        };
        self.projection(scene, size.width, size.height)
            .project_unclipped(open.points[0])
            .is_some_and(|(x, y, _)| (x - pointer[0]).hypot(y - pointer[1]) <= PICK_RADIUS)
    }

    /// Set a finished measurement from the command API.
    pub fn api_measure(&mut self, mode: &str, points: Vec<Xyz>) -> Value {
        let Some(mode) = MeasureMode::from_key(&mode.to_ascii_lowercase()) else {
            return json!({"ok": false, "error": "measure mode must be distance or area"});
        };
        if !(mode.min_points()..=MAX_POINTS).contains(&points.len())
            || !points.iter().flatten().all(|value| value.is_finite())
        {
            return json!({"ok": false, "error": format!(
                "{} needs {} to {MAX_POINTS} points of finite [x, y, z] scene coordinates",
                mode.key(),
                mode.min_points()
            )});
        }
        let measurement = Measurement {
            mode,
            points,
            finished: true,
        };
        self.status = format!("Measured {}", measurement.summary());
        let value = measurement.value();
        self.measure.current = Some(measurement);
        json!({"ok": true, "measure": value})
    }
}

/// Dark badge with amber text, as used for station labels, centred on a
/// position. The summary badge has a brighter outline.
fn badge(frame: &mut Frame, content: String, centre: UiPoint, summary: bool) {
    let width = content.chars().count() as f32 * 6.0 + 2.0;
    let position = UiPoint::new(centre.x - width * 0.5, centre.y - 6.5);
    let badge_position = UiPoint::new(position.x - 3.0, position.y - 2.0);
    let badge_size = Size::new(width + 6.0, 17.0);
    frame.fill_rectangle(badge_position, badge_size, Color::from_rgb8(42, 42, 50));
    frame.stroke_rectangle(
        badge_position,
        badge_size,
        canvas::Stroke::default()
            .with_color(if summary {
                Color::from_rgb8(245, 158, 11)
            } else {
                Color::from_rgb8(126, 88, 44)
            })
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

impl PointViewport<'_> {
    /// Draw the measurement with the camera in use, so it follows the orbit
    /// view and the walking camera alike.
    pub fn draw_measure(&self, frame: &mut Frame, size: Size) {
        let (Some(measurement), Some(scene)) = (
            self.measure.current.as_ref(),
            crate::combined_bounds(self.clouds),
        ) else {
            return;
        };
        let projection = self.projection(scene, size.width, size.height);
        let amber = Color::from_rgb8(245, 158, 11);
        let edges = measurement.edges();
        let lengths = measurement.segment_lengths();
        // Middle of the part of each edge that is inside the viewport.
        let mut middles = Vec::with_capacity(edges.len());
        for (index, (a, b)) in edges.iter().enumerate() {
            let Some([start, end]) = project_edge(projection, *a, *b, size) else {
                middles.push(None);
                continue;
            };
            // The edge that closes a polygon is a preview until it is finished.
            let preview = measurement.closed()
                && !measurement.finished
                && measurement.points.len() >= 3
                && index + 1 == edges.len();
            frame.stroke(
                &canvas::Path::line(start, end),
                canvas::Stroke::default()
                    .with_color(if preview {
                        Color { a: 0.45, ..amber }
                    } else {
                        amber
                    })
                    .with_width(1.6),
            );
            middles.push(
                clip_segment(
                    [start.x, start.y],
                    [end.x, end.y],
                    [0.0; 2],
                    [size.width, size.height],
                )
                .filter(|(from, to)| (to[0] - from[0]).hypot(to[1] - from[1]) >= 28.0)
                .map(|(from, to)| UiPoint::new((from[0] + to[0]) * 0.5, (from[1] + to[1]) * 0.5)),
            );
        }
        for (index, point) in measurement.points.iter().enumerate() {
            let Some((x, y, _)) = projection.project(*point) else {
                continue;
            };
            let marker =
                canvas::Path::circle(UiPoint::new(x, y), if index == 0 { 4.5 } else { 3.5 });
            frame.fill(&marker, Color::from_rgb8(42, 42, 50));
            frame.stroke(
                &marker,
                canvas::Stroke::default().with_color(amber).with_width(1.5),
            );
        }
        // One segment is its own total, so it carries the only label.
        let single = measurement.mode == MeasureMode::Distance && edges.len() == 1;
        for (middle, length) in middles.iter().zip(&lengths) {
            if let Some(middle) = middle {
                badge(frame, format_length(*length), *middle, single);
            }
        }
        if single || !measurement.complete() {
            return;
        }
        let anchor = match measurement.mode {
            // Above the last point of the polyline.
            MeasureMode::Distance => measurement
                .points
                .last()
                .and_then(|last| projection.project(*last))
                .map(|(x, y, _)| UiPoint::new(x, y - 20.0)),
            // In the middle of the polygon.
            MeasureMode::Area => {
                let count = measurement.points.len() as f64;
                let centre: Xyz = std::array::from_fn(|axis| {
                    measurement
                        .points
                        .iter()
                        .map(|point| point[axis])
                        .sum::<f64>()
                        / count
                });
                projection
                    .project(centre)
                    .map(|(x, y, _)| UiPoint::new(x, y))
            }
        };
        if let Some(anchor) = anchor {
            badge(frame, measurement.label(), anchor, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pointcloud_core::{Bounds, Point};

    use super::*;
    use crate::native_api::{self, ApiCommand};
    use crate::selection::SelectionMask;
    use crate::{finish_viewport_drag, DragMode, DragState};

    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() < 1e-9
    }

    #[test]
    fn polyline_reports_segments_total_horizontal_length_and_height_difference() {
        let points = [[0.0, 0.0, 0.0], [3.0, 4.0, 0.0], [3.0, 4.0, 12.0]];
        assert_eq!(segment_lengths(&points, false), vec![5.0, 12.0]);
        assert!(close(path_length(&points, false), 17.0));
        assert!(close(horizontal_length(&points), 5.0));
        assert!(close(height_difference(&points), 12.0));
        // A sloped segment is longer than it looks from above.
        let sloped = [[10.0, 10.0, 2.0], [13.0, 14.0, -10.0]];
        assert!(close(path_length(&sloped, false), 13.0));
        assert!(close(horizontal_length(&sloped), 5.0));
        assert!(close(height_difference(&sloped), -12.0));
    }

    #[test]
    fn unit_square_has_area_one_in_any_orientation() {
        let flat = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ];
        assert!(close(area(&flat), 1.0));
        assert!(close(plan_area(&flat), 1.0));

        let vertical = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        assert!(close(area(&vertical), 1.0));
        assert!(close(plan_area(&vertical), 0.0));

        // Tilted about two axes and placed at survey coordinates.
        let (sin_a, cos_a) = 0.7f64.sin_cos();
        let (sin_b, cos_b) = 1.1f64.sin_cos();
        let u = [cos_a, sin_a * cos_b, sin_a * sin_b];
        let v = [-sin_a, cos_a * cos_b, cos_a * sin_b];
        let origin = [207_440.25, 474_000.5, 12.75];
        let corner = |s: f64, t: f64| -> Xyz {
            std::array::from_fn(|axis| origin[axis] + u[axis] * s + v[axis] * t)
        };
        let tilted = [
            corner(0.0, 0.0),
            corner(1.0, 0.0),
            corner(1.0, 1.0),
            corner(0.0, 1.0),
        ];
        assert!(close(area(&tilted), 1.0));
        assert!(close(plan_area(&tilted), cos_b.abs()));
        assert!(close(path_length(&tilted, true), 4.0));

        // The winding direction does not change the result.
        let mut reversed = tilted;
        reversed.reverse();
        assert!(close(area(&reversed), 1.0));
    }

    #[test]
    fn sloped_rectangle_has_true_area_plan_area_and_perimeter() {
        let roof = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 3.0, 4.0],
            [0.0, 3.0, 4.0],
        ];
        assert!(close(area(&roof), 10.0));
        assert!(close(plan_area(&roof), 6.0));
        assert!(close(path_length(&roof, true), 14.0));
        assert_eq!(segment_lengths(&roof, true), vec![2.0, 5.0, 2.0, 5.0]);
    }

    #[test]
    fn concave_polygon_area_counts_the_notch_once() {
        let l_shape = [
            [0.0, 0.0, 5.0],
            [2.0, 0.0, 5.0],
            [2.0, 1.0, 5.0],
            [1.0, 1.0, 5.0],
            [1.0, 2.0, 5.0],
            [0.0, 2.0, 5.0],
        ];
        assert!(close(area(&l_shape), 3.0));
        assert!(close(plan_area(&l_shape), 3.0));
        assert!(close(path_length(&l_shape, true), 8.0));
    }

    #[test]
    fn degenerate_inputs_measure_zero() {
        let none: [Xyz; 0] = [];
        let one = [[4.0, 5.0, 6.0]];
        let two = [[0.0, 0.0, 0.0], [3.0, 4.0, 0.0]];
        for points in [&none[..], &one[..]] {
            assert!(segment_lengths(points, false).is_empty());
            assert_eq!(path_length(points, true), 0.0);
            assert_eq!(horizontal_length(points), 0.0);
            assert_eq!(height_difference(points), 0.0);
            assert_eq!(area(points), 0.0);
            assert_eq!(plan_area(points), 0.0);
        }
        // Two points have no area, and closing them adds no second edge.
        assert_eq!(area(&two), 0.0);
        assert_eq!(segment_lengths(&two, true), vec![5.0]);
        assert!(close(path_length(&two, true), 5.0));

        let collinear = [
            [100.0, 200.0, 10.0],
            [101.5, 203.0, 11.0],
            [103.0, 206.0, 12.0],
            [106.0, 212.0, 14.0],
        ];
        assert!(close(area(&collinear), 0.0));
        assert!(close(plan_area(&collinear), 0.0));
        assert!(path_length(&collinear, true) > 0.0);
        // Repeating a point adds neither area nor length.
        let repeated = [[1.0, 1.0, 1.0]; 4];
        assert_eq!(area(&repeated), 0.0);
        assert_eq!(path_length(&repeated, true), 0.0);
    }

    #[test]
    fn values_are_formatted_with_three_decimals_and_units() {
        assert_eq!(format_length(1.23456), "1.235 m");
        assert_eq!(format_length(0.0), "0.000 m");
        assert_eq!(format_area(2.0), "2.000 m²");
        assert_eq!(format_height(1.5), "+1.500 m");
        assert_eq!(format_height(-2.25), "-2.250 m");
        assert_eq!(format_height(-0.0004), "0.000 m");
    }

    #[test]
    fn rows_and_api_value_carry_the_values_of_each_mode() {
        // The values hold translated words; the language is one setting of
        // the whole process.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let distance = Measurement {
            mode: MeasureMode::Distance,
            points: vec![[0.0, 0.0, 0.0], [3.0, 4.0, 0.0], [3.0, 4.0, 12.0]],
            finished: true,
        };
        let rows = distance.rows();
        assert!(rows.contains(&("Length", "17.000 m".into())));
        assert!(rows.contains(&("Horizontal length", "5.000 m".into())));
        assert!(rows.contains(&("Height difference", "+12.000 m".into())));
        assert!(rows.contains(&("Segment", "2 · 12.000 m".into())));
        assert_eq!(distance.label(), "Total 17.000 m");
        let value = distance.value();
        assert_eq!(value["mode"], "distance");
        assert_eq!(value["length"], 17.0);
        assert_eq!(value["horizontal_length"], 5.0);
        assert_eq!(value["height_difference"], 12.0);
        assert_eq!(value["segments"], json!([5.0, 12.0]));
        assert!(value.get("area").is_none());

        let roof = Measurement {
            mode: MeasureMode::Area,
            points: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 3.0, 4.0],
                [0.0, 3.0, 4.0],
            ],
            finished: false,
        };
        let rows = roof.rows();
        assert!(rows.contains(&("Points", "4 · picking".into())));
        assert!(rows.contains(&("Area", "10.000 m²".into())));
        assert!(rows.contains(&("Plan area", "6.000 m²".into())));
        assert!(rows.contains(&("Perimeter", "14.000 m".into())));
        assert_eq!(roof.label(), "Area 10.000 m²");
        let value = roof.value();
        assert_eq!(value["finished"], false);
        assert_eq!(value["area"], 10.0);
        assert_eq!(value["plan_area"], 6.0);
        assert_eq!(value["perimeter"], 14.0);
        assert_eq!(value["segments"].as_array().unwrap().len(), 4);

        let long = Measurement {
            mode: MeasureMode::Distance,
            points: (0..40).map(|index| [f64::from(index), 0.0, 0.0]).collect(),
            finished: true,
        };
        let rows = long.rows();
        assert_eq!(
            rows.iter().filter(|(label, _)| *label == "Segment").count(),
            MAX_SEGMENT_ROWS
        );
        assert_eq!(rows.last(), Some(&("Segments", "15 more".into())));
    }

    #[test]
    fn row_labels_stay_english_keys_and_the_kind_is_translated() {
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::Table(0));
        let labels = |measurement: &Measurement| -> Vec<&'static str> {
            measurement
                .rows()
                .into_iter()
                .map(|(label, _)| label)
                .collect()
        };
        let shown = |measurement: &Measurement| -> Vec<&'static str> {
            labels(measurement).into_iter().map(tr).collect()
        };

        let mut distance = Measurement {
            mode: MeasureMode::Distance,
            points: vec![[0.0, 0.0, 0.0], [3.0, 4.0, 0.0]],
            finished: false,
        };
        assert_eq!(
            distance.rows()[..2],
            [
                ("Type", "Afstand".to_owned()),
                ("Points", "2 · bezig".to_owned()),
            ]
        );
        distance.finished = true;
        assert_eq!(distance.rows()[1], ("Points", "2".to_owned()));
        // The labels are the keys of the table; the row that shows them
        // looks them up, so a label translated here would be looked up twice.
        assert_eq!(
            labels(&distance),
            [
                "Type",
                "Points",
                "Length",
                "Horizontal length",
                "Height difference",
                "Segment"
            ]
        );
        assert_eq!(
            shown(&distance),
            [
                "Type",
                "Punten",
                "Lengte",
                "Horizontale lengte",
                "Hoogteverschil",
                "Segment"
            ]
        );

        let area = Measurement {
            mode: MeasureMode::Area,
            points: vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 3.0, 0.0]],
            finished: true,
        };
        assert_eq!(area.rows()[0], ("Type", "Oppervlakte".to_owned()));
        assert_eq!(
            labels(&area),
            [
                "Type",
                "Points",
                "Area",
                "Plan area",
                "Perimeter",
                "Segment",
                "Segment",
                "Segment"
            ]
        );
        assert_eq!(
            shown(&area)[..5],
            [
                "Type",
                "Punten",
                "Oppervlakte",
                "Horizontale oppervlakte",
                "Omtrek"
            ]
        );

        // The row past the last segment that is shown.
        let long = Measurement {
            mode: MeasureMode::Distance,
            points: (0..40).map(|index| [f64::from(index), 0.0, 0.0]).collect(),
            finished: true,
        };
        assert_eq!(labels(&long).last().copied(), Some("Segments"));
        assert_eq!(shown(&long).last().copied(), Some("Segmenten"));
    }

    #[test]
    fn tool_keeps_or_drops_an_unfinished_measurement_when_the_mode_ends() {
        let mut tool = MeasureTool::default();
        assert!(tool.push([0.0; 3]).is_err());

        tool.mode = Some(MeasureMode::Distance);
        tool.push([0.0, 0.0, 0.0]).unwrap();
        assert_eq!(
            tool.finish().unwrap_err(),
            "Distance needs at least 2 points"
        );
        tool.push([1.0, 0.0, 0.0]).unwrap();
        tool.push([1.0, 1.0, 0.0]).unwrap();
        assert!(tool.remove_last());
        assert_eq!(tool.current.as_ref().unwrap().points.len(), 2);
        assert!(tool.finish().unwrap().unwrap().finished);
        // Nothing is left to finish or to shorten.
        assert!(tool.finish().unwrap().is_none());
        assert!(!tool.remove_last());

        // The next point starts a new measurement.
        tool.push([5.0, 5.0, 5.0]).unwrap();
        assert_eq!(tool.current.as_ref().unwrap().points, vec![[5.0, 5.0, 5.0]]);
        assert!(tool.remove_last());
        assert!(tool.current.is_none());

        // Cancelling drops unfinished points; switching tools keeps a
        // measurement that is complete and drops one that is not.
        tool.push([0.0; 3]).unwrap();
        tool.push([1.0, 0.0, 0.0]).unwrap();
        assert!(tool.leave(false));
        assert!(tool.current.is_none() && tool.mode.is_none());
        assert!(!tool.leave(false));

        tool.mode = Some(MeasureMode::Area);
        tool.push([0.0; 3]).unwrap();
        tool.push([1.0, 0.0, 0.0]).unwrap();
        assert!(tool.leave(true));
        assert!(tool.current.is_none());

        tool.mode = Some(MeasureMode::Area);
        for point in [[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]] {
            tool.push(point).unwrap();
        }
        assert!(tool.leave(true));
        assert!(tool.current.as_ref().unwrap().finished);
        // A finished measurement survives cancelling a later mode.
        tool.mode = Some(MeasureMode::Distance);
        assert!(tool.leave(false));
        assert!(tool.current.is_some());

        tool.mode = Some(MeasureMode::Distance);
        for index in 0..MAX_POINTS {
            tool.push([index as f64, 0.0, 0.0]).unwrap();
        }
        assert!(tool.push([0.0; 3]).is_err());
        assert_eq!(tool.current.unwrap().points.len(), MAX_POINTS);
    }

    #[test]
    fn segments_are_clipped_to_the_viewport() {
        let (min, max) = ([0.0, 0.0], [100.0, 50.0]);
        assert_eq!(
            clip_segment([10.0, 10.0], [90.0, 40.0], min, max),
            Some(([10.0, 10.0], [90.0, 40.0]))
        );
        assert_eq!(
            clip_segment([-50.0, 25.0], [150.0, 25.0], min, max),
            Some(([0.0, 25.0], [100.0, 25.0]))
        );
        assert_eq!(
            clip_segment([50.0, -100.0], [50.0, 25.0], min, max),
            Some(([50.0, 0.0], [50.0, 25.0]))
        );
        assert_eq!(clip_segment([-10.0, 5.0], [-5.0, 45.0], min, max), None);
        assert_eq!(clip_segment([120.0, -5.0], [200.0, 60.0], min, max), None);
    }

    #[test]
    fn edge_running_past_a_walking_camera_is_drawn_up_to_the_view_edge() {
        let scene = Bounds {
            min: [-10.0; 3],
            max: [10.0; 3],
        };
        let size = Size::new(800.0, 600.0);
        // Eye at the origin, looking along +X.
        let projection = Projection::from_eye(
            scene,
            [0.0; 3],
            [[0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]],
            400.0,
            size.width,
            size.height,
        );
        let ahead = [5.0, 0.0, -1.0];
        let behind = [-5.0, 0.0, -1.0];
        let (x, y, _) = projection.project(ahead).unwrap();
        assert!(projection.project_unclipped(behind).is_none());

        let [start, end] = project_edge(projection, ahead, behind, size).unwrap();
        assert!((start.x - x).abs() < 0.01 && (start.y - y).abs() < 0.01);
        // The floor line leaves the view through its lower edge.
        assert!((end.x - x).abs() < 0.01);
        assert!(end.y > size.height && end.y <= size.height + 16.0);
        let [from_behind, _] = project_edge(projection, behind, ahead, size).unwrap();
        assert!((from_behind.y - end.y).abs() < 0.01);

        assert!(project_edge(projection, behind, [-9.0, 2.0, 0.0], size).is_none());
        let [a, b] = project_edge(projection, ahead, [5.0, -1.0, 0.0], size).unwrap();
        assert!((a.x - 400.0).abs() < 0.01 && (a.y - 380.0).abs() < 0.01);
        assert!((b.x - 480.0).abs() < 0.01 && (b.y - 300.0).abs() < 0.01);
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

    /// A studio with one small scan open, as the viewport needs for a camera.
    fn studio_with_scan() -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.xyz");
        std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        (studio, directory)
    }

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(native_api::ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    fn apply(studio: &mut Studio, action: MeasureAction) {
        let _ = studio.update(Message::Measure(action));
    }

    #[test]
    fn measuring_and_selection_tools_exclude_each_other() {
        let mut studio = Studio::default();
        let _ = studio.update(Message::ToggleBoxSelect);
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        assert_eq!(studio.measure.mode, Some(MeasureMode::Distance));
        assert!(!studio.box_select && !studio.pick_mode);

        let _ = studio.update(Message::TogglePickSelect);
        assert!(studio.pick_mode);
        assert_eq!(studio.measure.mode, None);

        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Area));
        assert_eq!(studio.measure.mode, Some(MeasureMode::Area));
        assert!(!studio.pick_mode);
        let _ = studio.update(Message::ToggleBoxSelect);
        assert!(studio.box_select);
        assert_eq!(studio.measure.mode, None);

        // Switching between the two modes, and pressing the active one again.
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Area));
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        assert_eq!(studio.measure.mode, Some(MeasureMode::Distance));
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        assert_eq!(studio.measure.mode, None);

        for action in [
            crate::ContextAction::Orbit,
            crate::ContextAction::BoxSelect,
            crate::ContextAction::PickPoint,
        ] {
            apply(&mut studio, MeasureAction::Toggle(MeasureMode::Area));
            let _ = studio.update(Message::ContextAction(action));
            assert_eq!(studio.measure.mode, None);
        }
    }

    #[test]
    fn picked_points_build_a_polyline_that_backspace_shortens_and_enter_finishes() {
        let mut studio = Studio::default();
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        let revision = studio.revision;
        for xyz in [[0.0, 0.0, 0.0], [3.0, 4.0, 0.0], [9.0, 9.0, 9.0]] {
            studio.selection_pending = true;
            apply(&mut studio, MeasureAction::Picked(revision, picked(xyz)));
            assert!(!studio.selection_pending);
        }
        assert_eq!(studio.measure.current.as_ref().unwrap().points.len(), 3);

        apply(&mut studio, MeasureAction::RemoveLast);
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked([3.0, 4.0, 12.0])),
        );
        // A miss, a failed search and a stale answer add nothing.
        apply(&mut studio, MeasureAction::Picked(revision, Ok(None)));
        assert_eq!(studio.status, "No point within 8 pixels");
        apply(
            &mut studio,
            MeasureAction::Picked(revision, Err("gone".into())),
        );
        apply(
            &mut studio,
            MeasureAction::Picked(revision + 1, picked([7.0; 3])),
        );
        let open = studio.measure.current.as_ref().unwrap();
        assert!(!open.finished);
        assert_eq!(
            open.points,
            vec![[0.0, 0.0, 0.0], [3.0, 4.0, 0.0], [3.0, 4.0, 12.0]]
        );

        apply(&mut studio, MeasureAction::Finish);
        assert!(studio.measure.current.as_ref().unwrap().finished);
        assert_eq!(
            studio.status,
            "Measured length 17.000 m · horizontal 5.000 m · height +12.000 m"
        );
        assert_eq!(studio.measure.mode, Some(MeasureMode::Distance));
        // Backspace leaves a finished measurement alone; a click starts anew.
        apply(&mut studio, MeasureAction::RemoveLast);
        assert_eq!(studio.measure.current.as_ref().unwrap().points.len(), 3);
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked([1.0; 3])),
        );
        let next = studio.measure.current.as_ref().unwrap();
        assert!(!next.finished);
        assert_eq!(next.points, vec![[1.0; 3]]);

        apply(&mut studio, MeasureAction::Clear);
        assert!(studio.measure.current.is_none());
        assert_eq!(studio.measure.mode, Some(MeasureMode::Distance));
    }

    #[test]
    fn escape_cancels_measuring_and_still_clears_the_selection() {
        let (mut studio, _directory) = studio_with_scan();
        let record = picked([4.0, 3.0, 0.0]).unwrap().unwrap();
        studio.clouds[0].selection = Some(Arc::new(SelectionMask::single(4, record).unwrap()));
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        let revision = studio.revision;
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked([0.0; 3])),
        );
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked([4.0, 0.0, 0.0])),
        );

        let _ = studio.update(Message::Escape);
        assert_eq!(studio.measure.mode, None);
        assert!(studio.measure.current.is_none());
        assert_eq!(studio.selected_total(), 0);

        // A pick that was still searching is cancelled with the mode.
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        let revision = studio.revision;
        studio.selection_pending = true;
        studio.selection_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _ = studio.update(Message::Escape);
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked([1.0; 3])),
        );
        assert!(studio.measure.current.is_none());
        assert_eq!(studio.status, "Measure pick cancelled");

        // A finished measurement stays until it is cleared.
        studio.selection_cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        let revision = studio.revision;
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked([0.0; 3])),
        );
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked([4.0, 0.0, 0.0])),
        );
        apply(&mut studio, MeasureAction::Finish);
        let _ = studio.update(Message::Escape);
        assert_eq!(studio.measure.mode, None);
        assert!(studio.measure.current.as_ref().unwrap().finished);
        assert_eq!(
            studio.status,
            "Measuring stopped; orbit and right-click menu available"
        );
    }

    #[test]
    fn click_picks_a_point_and_a_click_on_the_first_point_closes_an_area() {
        let (mut studio, _directory) = studio_with_scan();
        let size = studio.viewport_size;
        // Without a measuring mode a click does nothing.
        apply(&mut studio, MeasureAction::Click([10.0, 10.0], size));
        assert!(!studio.selection_pending);

        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Area));
        let revision = studio.revision;
        let corners = [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [4.0, 3.0, 0.0]];
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let (x, y, _) = studio
            .projection(scene, size.width, size.height)
            .project_unclipped(corners[0])
            .unwrap();

        // With fewer than three points a click on the first point is a pick.
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked(corners[0])),
        );
        apply(&mut studio, MeasureAction::Click([x, y], size));
        assert!(studio.selection_pending);
        assert!(!studio.measure.current.as_ref().unwrap().finished);
        studio.selection_pending = false;

        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked(corners[1])),
        );
        apply(
            &mut studio,
            MeasureAction::Picked(revision, picked(corners[2])),
        );
        // Away from the first point the click starts the exact pick search.
        apply(
            &mut studio,
            MeasureAction::Click([x + 60.0, y + 60.0], size),
        );
        assert!(studio.selection_pending);
        assert!(!studio.measure.current.as_ref().unwrap().finished);
        studio.selection_pending = false;

        apply(&mut studio, MeasureAction::Click([x + 3.0, y - 3.0], size));
        assert!(!studio.selection_pending);
        let area = studio.measure.current.as_ref().unwrap();
        assert!(area.finished);
        assert_eq!(area.points, corners);
        assert_eq!(
            studio.status,
            "Measured area 6.000 m² · plan 6.000 m² · perimeter 12.000 m"
        );
    }

    #[test]
    fn a_left_click_measures_and_a_left_drag_orbits() {
        let start = UiPoint::new(100.0, 80.0);
        let size = Size::new(800.0, 600.0);
        let pending = DragState {
            start,
            position: start,
            mode: DragMode::MeasurePending,
        };
        let click = finish_viewport_drag(
            iced::mouse::Button::Left,
            pending,
            UiPoint::new(102.0, 81.0),
            size,
        );
        assert!(matches!(
            click,
            Some(Message::Measure(MeasureAction::Click([102.0, 81.0], _)))
        ));
        let drag = finish_viewport_drag(
            iced::mouse::Button::Left,
            pending,
            UiPoint::new(130.0, 70.0),
            size,
        );
        assert!(matches!(drag, Some(Message::FinishOrbit(-30.0, -10.0))));

        // The answer of the pick search goes to the measurement.
        assert!(matches!(
            picked_message(Message::PickReady(7, 0, picked([1.0; 3]))),
            Message::Measure(MeasureAction::Picked(7, Ok(Some(_))))
        ));
        assert!(matches!(picked_message(Message::Escape), Message::Escape));
    }

    #[test]
    fn viewport_press_in_a_measuring_mode_clicks_until_it_moves_far_enough_to_orbit() {
        use iced::mouse::{self, Cursor};
        use iced::widget::canvas::{Event, Program};

        let (mut studio, _directory) = studio_with_scan();
        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Distance));
        let bounds = iced::Rectangle::new(UiPoint::ORIGIN, studio.viewport_size);
        let viewport = studio.point_viewport();
        let mut state = crate::ViewportState::default();
        let at = |x: f32, y: f32| Cursor::Available(UiPoint::new(x, y));
        let moved = |x: f32, y: f32| {
            Event::Mouse(mouse::Event::CursorMoved {
                position: UiPoint::new(x, y),
            })
        };
        let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));

        // The pointer may tremble a few pixels during a click.
        let (_, message) = viewport.update(&mut state, press.clone(), bounds, at(300.0, 400.0));
        assert!(message.is_none());
        let (_, message) =
            viewport.update(&mut state, moved(302.0, 401.0), bounds, at(302.0, 401.0));
        assert!(message.is_none());
        let (_, message) = viewport.update(&mut state, release.clone(), bounds, at(302.0, 401.0));
        assert!(matches!(
            message,
            Some(Message::Measure(MeasureAction::Click([302.0, 401.0], _)))
        ));

        // Further than that the whole gesture orbits and no point is picked.
        let _ = viewport.update(&mut state, press, bounds, at(300.0, 400.0));
        let (_, message) =
            viewport.update(&mut state, moved(320.0, 410.0), bounds, at(320.0, 410.0));
        assert!(matches!(message, Some(Message::Orbit(-20.0, 10.0))));
        let (_, message) = viewport.update(&mut state, release, bounds, at(325.0, 410.0));
        assert!(matches!(message, Some(Message::FinishOrbit(-5.0, 0.0))));
    }

    #[test]
    fn api_sets_reports_and_clears_a_measurement() {
        let command: ApiCommand = serde_json::from_str(
            r#"{"command":"measure","mode":"area","points":[[0,0,0],[2,0,0],[2,3,4],[0,3,4]]}"#,
        )
        .unwrap();
        let mut studio = Studio::default();
        assert_eq!(
            send(&mut studio, ApiCommand::Status)["result"]["measure"],
            Value::Null
        );

        let response = send(&mut studio, command);
        assert_eq!(response["ok"], true);
        assert_eq!(response["measure"]["area"], 10.0);
        let status = send(&mut studio, ApiCommand::Status);
        let reported = &status["result"]["measure"];
        assert_eq!(reported["mode"], "area");
        assert_eq!(reported["finished"], true);
        assert_eq!(reported["points"].as_array().unwrap().len(), 4);
        assert_eq!(reported["area"], 10.0);
        assert_eq!(reported["plan_area"], 6.0);
        assert_eq!(reported["perimeter"], 14.0);
        assert_eq!(status["result"]["measure_mode"], Value::Null);

        let distance = send(
            &mut studio,
            ApiCommand::Measure {
                mode: "Distance".into(),
                points: vec![[0.0, 0.0, 0.0], [3.0, 4.0, 12.0]],
            },
        );
        assert_eq!(distance["measure"]["length"], 13.0);
        assert_eq!(distance["measure"]["horizontal_length"], 5.0);
        assert_eq!(distance["measure"]["height_difference"], 12.0);

        for (mode, points) in [
            ("volume", vec![[0.0; 3], [1.0; 3]]),
            ("distance", vec![[0.0; 3]]),
            ("area", vec![[0.0; 3], [1.0; 3]]),
            ("distance", vec![[0.0; 3], [f64::NAN, 0.0, 0.0]]),
            ("distance", vec![[0.0; 3]; MAX_POINTS + 1]),
        ] {
            let rejected = send(
                &mut studio,
                ApiCommand::Measure {
                    mode: mode.into(),
                    points,
                },
            );
            assert_eq!(rejected["ok"], false);
            assert_eq!(
                studio.measure.current.as_ref().unwrap().mode,
                MeasureMode::Distance
            );
        }

        apply(&mut studio, MeasureAction::Toggle(MeasureMode::Area));
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["measure_mode"], "area");
        assert_eq!(status["result"]["measure"]["mode"], "distance");

        let cleared = send(&mut studio, ApiCommand::ClearMeasure);
        assert_eq!(cleared["ok"], true);
        assert_eq!(
            send(&mut studio, ApiCommand::Status)["result"]["measure"],
            Value::Null
        );
    }
}
