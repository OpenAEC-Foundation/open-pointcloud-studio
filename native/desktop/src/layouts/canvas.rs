//! The sheet in the main area: the paper on grey in its true proportions,
//! with what the plot of the sheet holds. The lines, fills, points and
//! images are built once into a geometry at the scale of the view and moved
//! as it pans; the texts, the outlines of the viewports and what the pointer
//! does are drawn over it every frame. A drag on the paper pans, the wheel
//! zooms about the pointer, a click selects a viewport and a drag moves it,
//! and a row of VIEWS let go over the paper is placed there.

use std::sync::Arc;

use iced::advanced::graphics::geometry::Renderer as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Renderer as _};
use iced::advanced::widget::{self, Widget};
use iced::alignment;
use iced::mouse;
use iced::widget::canvas::{self, event, Frame};
use iced::{
    Background, Color, Element, Length, Pixels, Point as UiPoint, Rectangle, Renderer, Size, Theme,
    Transformation, Vector,
};

use crate::drawing_view::ViewCamera;
use crate::ui_theme::UiTheme;
use crate::Message;

use super::plot::{Align, Mark, Plot};
use super::{LayoutAction, LayoutTool};

/// A click moves the pointer at most this many pixels.
const CLICK_SLOP: f32 = 4.0;
/// Wheel steps zoom by this much each.
const ZOOM_STEP: f32 = 1.25;
/// Texts smaller than this many pixels are not drawn.
const MIN_TEXT_PIXELS: f32 = 2.5;

/// What lies around the paper.
pub(crate) fn desk(theme: UiTheme) -> Color {
    match theme {
        UiTheme::Light | UiTheme::Contrast => Color::from_rgb8(163, 163, 170),
        UiTheme::Blueprint => Color::from_rgb8(70, 82, 104),
        UiTheme::Forge | UiTheme::Night => Color::from_rgb8(74, 74, 82),
    }
}

/// What is written on the desk: dark on the light grey of the light
/// themes, light on the dark grey of the others.
pub(crate) fn desk_ink(theme: UiTheme) -> Color {
    match theme {
        UiTheme::Light | UiTheme::Contrast => Color::from_rgb8(24, 24, 27),
        UiTheme::Blueprint | UiTheme::Forge | UiTheme::Night => Color::from_rgb8(245, 245, 244),
    }
}

/// An iced colour from a colour of the plot.
fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

/// Where the geometry was built for: the plot and the pixels a millimetre
/// took.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Built {
    revision: u64,
    scale: f64,
}

/// The pixel of the geometry, whose origin is the upper left corner of the
/// paper, where a point of the paper lies at `scale` pixels a millimetre.
fn geometry_pixel(point: [f64; 2], height: f64, scale: f64) -> UiPoint {
    UiPoint::new(
        (point[0] * scale) as f32,
        ((height - point[1]) * scale) as f32,
    )
}

/// The rectangle of the geometry a rectangle of the paper covers.
fn geometry_rect(rect: [[f64; 2]; 2], height: f64, scale: f64) -> Rectangle {
    let top_left = geometry_pixel([rect[0][0], rect[1][1]], height, scale);
    let bottom_right = geometry_pixel([rect[1][0], rect[0][1]], height, scale);
    Rectangle::new(
        top_left,
        Size::new(bottom_right.x - top_left.x, bottom_right.y - top_left.y),
    )
}

/// Draw the marks of the plot but its texts into a frame whose origin is
/// the upper left corner of the paper.
fn draw_marks(frame: &mut Frame, plot: &Plot, tool: &LayoutTool, scale: f64) {
    let height = plot.size[1];
    for group in &plot.groups {
        match group.clip {
            Some(clip) => {
                let region = geometry_rect(clip, height, scale);
                let origin = Vector::new(-region.x, -region.y);
                frame.with_clip(region, |frame| {
                    for mark in &group.marks {
                        draw_mark(frame, mark, tool, height, scale, origin);
                    }
                });
            }
            None => {
                for mark in &group.marks {
                    draw_mark(frame, mark, tool, height, scale, Vector::ZERO);
                }
            }
        }
    }
}

fn draw_mark(
    frame: &mut Frame,
    mark: &Mark,
    tool: &LayoutTool,
    height: f64,
    scale: f64,
    origin: Vector,
) {
    let pixel = |point: [f64; 2]| geometry_pixel(point, height, scale) + origin;
    match mark {
        Mark::Line {
            points,
            closed,
            width,
            rgb,
        } => {
            let Some(first) = points.first() else {
                return;
            };
            let path = canvas::Path::new(|builder| {
                builder.move_to(pixel(*first));
                for point in &points[1..] {
                    builder.line_to(pixel(*point));
                }
                if *closed {
                    builder.close();
                }
            });
            frame.stroke(
                &path,
                canvas::Stroke::default()
                    .with_color(color(*rgb))
                    .with_width(((width * scale) as f32).max(0.7)),
            );
        }
        Mark::Fill { rings, rgb } => {
            let path = canvas::Path::new(|builder| {
                for ring in rings.iter().filter(|ring| ring.len() >= 3) {
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
                    style: canvas::Style::Solid(color(*rgb)),
                    rule: canvas::fill::Rule::EvenOdd,
                },
            );
        }
        Mark::Dot { at, rgb } => {
            let side = ((super::plot::DOT_SIZE * scale) as f32).max(1.0);
            let centre = pixel(*at);
            frame.fill_rectangle(
                UiPoint::new(centre.x - side / 2.0, centre.y - side / 2.0),
                Size::new(side, side),
                color(*rgb),
            );
        }
        Mark::Image { rect, key } => {
            if let Some(image) = tool.images.get(key) {
                let mut bounds = geometry_rect(*rect, height, scale);
                bounds.x += origin.x;
                bounds.y += origin.y;
                frame.draw_image(bounds, canvas::Image::new(image.handle.clone()));
            }
        }
        // Texts are drawn over the geometry every frame.
        Mark::Text { .. } => {}
    }
}

/// The paper with the cached geometry of the sheet.
pub(crate) struct Paper<'a> {
    pub tool: &'a LayoutTool,
    pub plot: Arc<Plot>,
    pub desk: Color,
    pub paper: Color,
}

impl Widget<Message, Theme, Renderer> for Paper<'_> {
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
            Background::Color(self.desk),
        );
        let size = bounds.size();
        if size.width < 1.0 || size.height < 1.0 {
            return;
        }
        let plot = &self.plot;
        if tool.fit_pending.get() {
            tool.camera
                .set(ViewCamera::fit([[0.0, 0.0], plot.size], size));
            tool.fit_pending.set(false);
        }
        let camera = tool.camera.get();
        let [width, height] = plot.size;
        let wanted = Built {
            revision: tool.drawn_revision(),
            scale: camera.scale,
        };
        if tool.built.get() != Some(wanted) {
            tool.cache.clear();
            tool.built.set(Some(wanted));
        }
        let paper = Size::new(
            (width * camera.scale) as f32,
            (height * camera.scale) as f32,
        );
        // The paper and its shadow lie under the geometry: a part of a
        // geometry drawn within a clip comes before the rest of it, so the
        // paper cannot be a part of it.
        let [x, y] = camera.to_screen([0.0, height], size);
        let corner = iced::Point::new(bounds.x + x as f32, bounds.y + y as f32);
        let shadow = 4.0;
        renderer.with_layer(bounds, |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(corner + Vector::new(shadow, shadow), paper),
                    ..renderer::Quad::default()
                },
                Background::Color(Color::from_rgba8(0, 0, 0, 0.25)),
            );
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle::new(corner, paper),
                    ..renderer::Quad::default()
                },
                Background::Color(self.paper),
            );
        });
        let geometry = tool.cache.draw(renderer, paper, |frame| {
            draw_marks(frame, plot, tool, camera.scale);
        });
        renderer.with_layer(bounds, |renderer| {
            renderer.with_transformation(
                Transformation::translate(bounds.x + x as f32, bounds.y + y as f32),
                |renderer| renderer.draw_geometry(geometry),
            );
        });
    }
}

impl<'a> From<Paper<'a>> for Element<'a, Message> {
    fn from(paper: Paper<'a>) -> Self {
        Element::new(paper)
    }
}

/// A viewport being dragged to another place.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Moving {
    /// The viewport, by its identifier: the viewports of the sheet may
    /// change while it is dragged.
    id: String,
    /// Where the pointer took hold of it and where it is now, on the paper.
    from: [f64; 2],
    to: [f64; 2],
}

#[derive(Debug, Default)]
pub(crate) struct OverlayState {
    pan: Option<UiPoint>,
    click: Option<UiPoint>,
    moving: Option<Moving>,
    /// A note of the paper held by the pointer: its identifier, where it
    /// was taken and where it is now, on the paper.
    note: Option<(String, [f64; 2], [f64; 2], UiPoint)>,
    /// A press on a locked viewport, which a click selects.
    locked_click: Option<String>,
}

/// A mark of a note moved by `shift` on the paper.
fn shifted(
    mark: &crate::drawing_notes::NoteMark,
    shift: [f64; 2],
) -> crate::drawing_notes::NoteMark {
    use crate::drawing_notes::NoteMark;
    let moved = |point: &[f64; 2]| [point[0] + shift[0], point[1] + shift[1]];
    match mark {
        NoteMark::Line(points) => NoteMark::Line(points.iter().map(moved).collect()),
        NoteMark::Fill(points) => NoteMark::Fill(points.iter().map(moved).collect()),
        NoteMark::Text {
            at,
            height,
            rotation,
            value,
            align,
        } => NoteMark::Text {
            at: moved(at),
            height: *height,
            rotation: *rotation,
            value: value.clone(),
            align: *align,
        },
    }
}

/// What is drawn over the paper every frame, and the pointer.
pub(crate) struct Overlay<'a> {
    pub tool: &'a LayoutTool,
    pub plot: Arc<Plot>,
    pub selected: Option<String>,
    /// The size a row of VIEWS dragged over the paper would take, while one
    /// is dragged.
    pub dropping: Option<[f64; 2]>,
    pub accent: Color,
    /// What the paper says while no view is placed on it, and the ink it
    /// is written in on the desk.
    pub hint: Option<String>,
    pub hint_ink: Color,
    /// The tool that places a text or a line on the paper, and the points
    /// clicked with it.
    pub tool_kind: Option<crate::drawing_notes::NoteKind>,
    pub picked: Vec<[f64; 2]>,
    /// The text of what is placed is being typed.
    pub typing: bool,
    pub selected_note: Option<String>,
}

impl Overlay<'_> {
    fn camera(&self) -> ViewCamera {
        self.tool.camera.get()
    }

    fn paper_at(&self, pixel: UiPoint, size: Size) -> [f64; 2] {
        self.camera().to_drawing([pixel.x, pixel.y], size)
    }

    fn screen_rect(&self, rect: [[f64; 2]; 2], size: Size) -> Rectangle {
        let camera = self.camera();
        let [left, top] = camera.to_screen([rect[0][0], rect[1][1]], size);
        let [right, bottom] = camera.to_screen([rect[1][0], rect[0][1]], size);
        Rectangle::new(
            UiPoint::new(left as f32, top as f32),
            Size::new((right - left) as f32, (bottom - top) as f32),
        )
    }
}

impl canvas::Program<Message> for Overlay<'_> {
    type State = OverlayState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        let action = |action| Some(Message::Layouts(action));
        let notes = |action| Some(Message::Notes(action));
        let size = bounds.size();
        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(
                button @ (mouse::Button::Left | mouse::Button::Middle),
            )) => {
                let Some(position) = cursor.position_in(bounds) else {
                    return (event::Status::Ignored, None);
                };
                if button == mouse::Button::Left {
                    let at = self.paper_at(position, size);
                    // A click of a tool places a text or a line on the
                    // paper.
                    if self.tool_kind.is_some() {
                        if self.typing {
                            return (event::Status::Captured, None);
                        }
                        return (
                            event::Status::Captured,
                            notes(crate::drawing_notes::NoteAction::Pick(at)),
                        );
                    }
                    // A note of the paper is selected, and dragged.
                    let reach = 6.0 / self.camera().scale;
                    if let Some((id, _)) = self.plot.notes.iter().rev().find(|(_, marks)| {
                        crate::drawing_notes::distance_to_marks(marks, at) <= reach
                    }) {
                        state.note = Some((id.clone(), at, at, position));
                        return (
                            event::Status::Captured,
                            notes(crate::drawing_notes::NoteAction::Select(Some(id.clone()))),
                        );
                    }
                }
                state.pan = Some(position);
                if button == mouse::Button::Left {
                    state.click = Some(position);
                    let at = self.paper_at(position, size);
                    let hit = self
                        .plot
                        .viewports
                        .iter()
                        .rposition(|placed| super::model::contains(placed.rect, at));
                    // A locked viewport is selected, not moved.
                    let hit = hit.filter(|place| {
                        let placed = &self.plot.viewports[*place];
                        if placed.locked {
                            state.locked_click = Some(placed.id.clone());
                        }
                        !placed.locked
                    });
                    if let Some(place) = hit {
                        state.pan = None;
                        state.moving = Some(Moving {
                            id: self.plot.viewports[place].id.clone(),
                            from: at,
                            to: at,
                        });
                    }
                }
                return (event::Status::Captured, None);
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(
                button @ (mouse::Button::Left | mouse::Button::Middle),
            )) => {
                // A row of VIEWS let go: over the paper it is placed there.
                if self.dropping.is_some() && button == mouse::Button::Left {
                    let dropped = cursor
                        .position_in(bounds)
                        .map(|position| self.paper_at(position, size));
                    return (
                        event::Status::Captured,
                        action(match dropped {
                            Some(at) => LayoutAction::Drop(at),
                            None => LayoutAction::DragEnd,
                        }),
                    );
                }
                if let Some((id, from, to, _)) = state.note.take() {
                    if from == to {
                        return (event::Status::Captured, None);
                    }
                    return (
                        event::Status::Captured,
                        notes(crate::drawing_notes::NoteAction::Moved(
                            id,
                            [to[0] - from[0], to[1] - from[1]],
                        )),
                    );
                }
                let click = state.click.take().filter(|_| button == mouse::Button::Left);
                if let Some(moving) = state.moving.take() {
                    // A viewport taken off the sheet meanwhile is let go.
                    let Some(placed) = self
                        .plot
                        .viewports
                        .iter()
                        .find(|placed| placed.id == moving.id)
                    else {
                        return (event::Status::Captured, None);
                    };
                    let id = placed.id.clone();
                    if click.is_some() {
                        return (
                            event::Status::Captured,
                            action(LayoutAction::Select(Some(id))),
                        );
                    }
                    let rect = placed.rect;
                    let centre = [
                        (rect[0][0] + rect[1][0]) / 2.0 + moving.to[0] - moving.from[0],
                        (rect[0][1] + rect[1][1]) / 2.0 + moving.to[1] - moving.from[1],
                    ];
                    return (
                        event::Status::Captured,
                        action(LayoutAction::Move(id, centre)),
                    );
                }
                let locked = state.locked_click.take();
                if state.pan.take().is_some() {
                    if let (Some(_), Some(id)) = (click, locked) {
                        return (
                            event::Status::Captured,
                            action(LayoutAction::Select(Some(id))),
                        );
                    }
                    if click.is_some() && self.selected.is_some() {
                        return (event::Status::Captured, action(LayoutAction::Select(None)));
                    }
                    return (event::Status::Captured, None);
                }
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let now = UiPoint::new(position.x - bounds.x, position.y - bounds.y);
                let moved = |at: UiPoint| {
                    (now.x - at.x).abs() > CLICK_SLOP || (now.y - at.y).abs() > CLICK_SLOP
                };
                if state.click.is_some_and(moved) {
                    state.click = None;
                }
                if let Some((_, _, to, pressed)) = &mut state.note {
                    if moved(*pressed) || *to != self.paper_at(*pressed, size) {
                        *to = self.paper_at(now, size);
                    }
                    return (event::Status::Captured, None);
                }
                if let Some(moving) = &mut state.moving {
                    if state.click.is_none() {
                        moving.to = self.paper_at(now, size);
                    }
                    return (event::Status::Captured, None);
                }
                if let Some(previous) = state.pan {
                    state.pan = Some(now);
                    return (
                        event::Status::Captured,
                        action(LayoutAction::Pan([now.x - previous.x, now.y - previous.y])),
                    );
                }
            }
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                if let Some(position) = cursor.position_in(bounds) {
                    let steps = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => y,
                        mouse::ScrollDelta::Pixels { y, .. } => y / 60.0,
                    };
                    return (
                        event::Status::Captured,
                        action(LayoutAction::Zoom(
                            ZOOM_STEP.powf(steps.clamp(-4.0, 4.0)),
                            [position.x, position.y],
                            size,
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
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let size = bounds.size();
        let mut frame = Frame::new(renderer, size);
        let camera = self.camera();
        // The texts of the sheet.
        for (mark, clip) in self.plot.marks() {
            let Mark::Text {
                at,
                height,
                rotation,
                value,
                rgb,
                align,
            } = mark
            else {
                continue;
            };
            if clip.is_some_and(|clip| !super::model::contains(clip, *at)) {
                continue;
            }
            let pixels = (height * camera.scale) as f32;
            if !(MIN_TEXT_PIXELS..=2_000.0).contains(&pixels) {
                continue;
            }
            let [x, y] = camera.to_screen(*at, size);
            let content = canvas::Text {
                content: value.clone(),
                position: UiPoint::ORIGIN,
                color: color(*rgb),
                size: Pixels(pixels * 1.4),
                horizontal_alignment: match align {
                    Align::Left => alignment::Horizontal::Left,
                    Align::Centre => alignment::Horizontal::Center,
                    Align::Right => alignment::Horizontal::Right,
                },
                vertical_alignment: alignment::Vertical::Bottom,
                ..canvas::Text::default()
            };
            frame.with_save(|frame| {
                frame.translate(Vector::new(x as f32, y as f32));
                if *rotation != 0.0 {
                    frame.rotate(-*rotation as f32);
                }
                frame.fill_text(content);
            });
        }
        // The outlines of the viewports: quiet, and the selected one in the
        // accent with its corners.
        let quiet = Color::from_rgba8(59, 130, 246, 0.35);
        for placed in &self.plot.viewports {
            let selected = self.selected.as_deref() == Some(placed.id.as_str());
            let mut rect = placed.rect;
            if let Some(moving) = state
                .moving
                .as_ref()
                .filter(|moving| moving.id == placed.id)
            {
                let shift = [moving.to[0] - moving.from[0], moving.to[1] - moving.from[1]];
                rect = [
                    [rect[0][0] + shift[0], rect[0][1] + shift[1]],
                    [rect[1][0] + shift[0], rect[1][1] + shift[1]],
                ];
            }
            let outline = self.screen_rect(rect, size);
            let stroke = if selected {
                canvas::Stroke::default()
                    .with_color(self.accent)
                    .with_width(2.0)
            } else {
                canvas::Stroke::default().with_color(quiet).with_width(1.0)
            };
            frame.stroke(
                &canvas::Path::rectangle(outline.position(), outline.size()),
                stroke,
            );
            if selected {
                for corner in [
                    outline.position(),
                    UiPoint::new(outline.x + outline.width, outline.y),
                    UiPoint::new(outline.x, outline.y + outline.height),
                    UiPoint::new(outline.x + outline.width, outline.y + outline.height),
                ] {
                    frame.fill_rectangle(
                        UiPoint::new(corner.x - 3.5, corner.y - 3.5),
                        Size::new(7.0, 7.0),
                        self.accent,
                    );
                }
            }
        }
        // The selected note of the paper, where it is dragged to, and what
        // a tool is placing.
        let to_screen = |point: [f64; 2]| {
            let [x, y] = camera.to_screen(point, size);
            UiPoint::new(x as f32, y as f32)
        };
        if let Some(selected) = &self.selected_note {
            if let Some((_, marks)) = self.plot.notes.iter().find(|(id, _)| id == selected) {
                let shift = state.note.as_ref().map_or([0.0, 0.0], |(_, from, to, _)| {
                    [to[0] - from[0], to[1] - from[1]]
                });
                let moved: Vec<crate::drawing_notes::NoteMark> =
                    marks.iter().map(|mark| shifted(mark, shift)).collect();
                crate::drawing_notes::draw_marks(
                    &mut frame,
                    &moved,
                    to_screen,
                    camera.scale,
                    self.accent,
                );
            }
        }
        if self.tool_kind.is_some() {
            let mut points = self.picked.clone();
            if let (false, Some(position)) = (self.typing, cursor.position_in(bounds)) {
                points.push(self.paper_at(position, size));
            }
            if points.len() >= 2 {
                crate::drawing_notes::draw_marks(
                    &mut frame,
                    &[crate::drawing_notes::NoteMark::Line(points.clone())],
                    to_screen,
                    camera.scale,
                    self.accent,
                );
            }
            for point in &self.picked {
                let at = to_screen(*point);
                frame.fill_rectangle(
                    UiPoint::new(at.x - 3.0, at.y - 3.0),
                    Size::new(6.0, 6.0),
                    self.accent,
                );
            }
        }
        // Where a row of VIEWS would be placed.
        if let (Some(dropping), Some(position)) = (self.dropping, cursor.position_in(bounds)) {
            let at = self.paper_at(position, size);
            let rect = super::model::rect_around(at, dropping);
            let outline = self.screen_rect(rect, size);
            frame.fill_rectangle(
                outline.position(),
                outline.size(),
                Color {
                    a: 0.12,
                    ..self.accent
                },
            );
            frame.stroke(
                &canvas::Path::rectangle(outline.position(), outline.size()),
                canvas::Stroke::default()
                    .with_color(self.accent)
                    .with_width(1.5),
            );
        }
        if let Some(hint) = &self.hint {
            frame.fill_text(canvas::Text {
                content: hint.clone(),
                position: UiPoint::new(size.width / 2.0, 14.0),
                color: self.hint_ink,
                size: Pixels(13.0),
                horizontal_alignment: alignment::Horizontal::Center,
                vertical_alignment: alignment::Vertical::Top,
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
        if (state.moving.is_some() && state.click.is_none()) || state.note.is_some() {
            return mouse::Interaction::Grabbing;
        }
        if self.tool_kind.is_some() && cursor.is_over(bounds) {
            return mouse::Interaction::Crosshair;
        }
        if self.dropping.is_some() && cursor.is_over(bounds) {
            return mouse::Interaction::Copy;
        }
        let over = cursor.position_in(bounds).and_then(|position| {
            self.plot
                .viewport_at(self.paper_at(position, bounds.size()))
        });
        if over.is_some() {
            mouse::Interaction::Grab
        } else if state.pan.is_some() {
            mouse::Interaction::Idle
        } else {
            mouse::Interaction::default()
        }
    }
}
