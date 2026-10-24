//! One-track, two-thumb ribbon input matching the Pile Plan range geometry.
use crate::Theme;
use iced::advanced::renderer::Renderer as _;
use iced::{
    advanced::{
        layout, renderer,
        widget::tree::{self, Tree},
        Clipboard, Layout, Shell, Widget,
    },
    event, mouse, touch, Background, Border, Event, Length, Rectangle, Renderer, Size,
};

const THUMB_RADIUS: f32 = 7.5;
const TRACK_HEIGHT: f32 = 4.0;
const WIDTH: f32 = 158.0;
const HEIGHT: f32 = 18.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Thumb {
    Lower,
    Upper,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DualRange<M> {
    pub theme: Theme,
    pub minimum: f32,
    pub maximum: f32,
    pub lower: f32,
    pub upper: f32,
    pub step: f32,
    pub on_lower: fn(f32) -> M,
    pub on_upper: fn(f32) -> M,
}

impl<M> DualRange<M> {
    fn thumb_x(&self, width: f32, value: f32) -> f32 {
        let usable = (width - 2.0 * THUMB_RADIUS).max(1.0);
        THUMB_RADIUS
            + usable * ((value - self.minimum) / (self.maximum - self.minimum)).clamp(0.0, 1.0)
    }

    fn value_at(&self, width: f32, x: f32, thumb: Thumb) -> f32 {
        let usable = (width - 2.0 * THUMB_RADIUS).max(1.0);
        let raw = self.minimum
            + ((x - THUMB_RADIUS) / usable).clamp(0.0, 1.0) * (self.maximum - self.minimum);
        let snapped = if self.step > 0.0 {
            self.minimum + ((raw - self.minimum) / self.step).round() * self.step
        } else {
            raw
        };
        match thumb {
            Thumb::Lower => snapped.clamp(self.minimum, self.upper),
            Thumb::Upper => snapped.clamp(self.lower, self.maximum),
        }
    }

    fn nearest_thumb(&self, width: f32, x: f32) -> Thumb {
        let lower_distance = (x - self.thumb_x(width, self.lower)).abs();
        let upper_distance = (x - self.thumb_x(width, self.upper)).abs();
        if lower_distance <= upper_distance {
            Thumb::Lower
        } else {
            Thumb::Upper
        }
    }

    fn message_at(&self, width: f32, x: f32, thumb: Thumb) -> M {
        let value = self.value_at(width, x, thumb);
        match thumb {
            Thumb::Lower => (self.on_lower)(value),
            Thumb::Upper => (self.on_upper)(value),
        }
    }
}

impl<M> DualRange<M> {
    fn pointer_event(
        &self,
        active: &mut Option<Thumb>,
        event: Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<M>) {
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(point) = cursor.position_in(bounds) else {
                    return (event::Status::Ignored, None);
                };
                let thumb = self.nearest_thumb(bounds.width, point.x);
                *active = Some(thumb);
                (
                    event::Status::Captured,
                    Some(self.message_at(bounds.width, point.x, thumb)),
                )
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if active.is_some() => {
                let thumb = active.expect("active thumb");
                (
                    event::Status::Captured,
                    Some(self.message_at(bounds.width, position.x - bounds.x, thumb)),
                )
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if active.take().is_some() =>
            {
                (event::Status::Captured, None)
            }
            Event::Touch(touch::Event::FingerPressed { position, .. })
                if bounds.contains(position) =>
            {
                let x = position.x - bounds.x;
                let thumb = self.nearest_thumb(bounds.width, x);
                *active = Some(thumb);
                (
                    event::Status::Captured,
                    Some(self.message_at(bounds.width, x, thumb)),
                )
            }
            Event::Touch(touch::Event::FingerMoved { position, .. }) if active.is_some() => {
                let thumb = active.expect("active thumb");
                (
                    event::Status::Captured,
                    Some(self.message_at(bounds.width, position.x - bounds.x, thumb)),
                )
            }
            Event::Touch(touch::Event::FingerLifted { .. } | touch::Event::FingerLost { .. })
                if active.take().is_some() =>
            {
                (event::Status::Captured, None)
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw_track(&self, renderer: &mut Renderer, bounds: Rectangle) {
        let tokens = self.theme.tokens();
        let center_y = bounds.y + bounds.height / 2.0;
        let lower_x = self.thumb_x(bounds.width, self.lower);
        let upper_x = self.thumb_x(bounds.width, self.upper);
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x,
                    y: center_y - TRACK_HEIGHT / 2.0,
                    width: bounds.width,
                    height: TRACK_HEIGHT,
                },
                border: Border::default().rounded(2),
                ..renderer::Quad::default()
            },
            Background::Color(tokens.text_muted),
        );
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x: bounds.x + lower_x,
                    y: center_y - TRACK_HEIGHT / 2.0,
                    width: (upper_x - lower_x).max(0.0),
                    height: TRACK_HEIGHT,
                },
                ..renderer::Quad::default()
            },
            Background::Color(tokens.accent),
        );
        for x in [lower_x, upper_x] {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: bounds.x + x - THUMB_RADIUS,
                        y: center_y - THUMB_RADIUS,
                        width: 2.0 * THUMB_RADIUS,
                        height: 2.0 * THUMB_RADIUS,
                    },
                    border: Border::default().rounded(THUMB_RADIUS),
                    ..renderer::Quad::default()
                },
                Background::Color(tokens.accent),
            );
        }
    }
}

impl<M: Clone> Widget<M, iced::Theme, Renderer> for DualRange<M> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Option<Thumb>>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(None::<Thumb>)
    }
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fixed(WIDTH),
            height: Length::Fixed(HEIGHT),
        }
    }
    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fixed(WIDTH), Length::Fixed(HEIGHT))
    }
    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        _viewport: &Rectangle,
    ) -> event::Status {
        let (status, message) = self.pointer_event(
            tree.state.downcast_mut::<Option<Thumb>>(),
            event,
            layout.bounds(),
            cursor,
        );
        if let Some(message) = message {
            shell.publish(message);
        }
        status
    }
    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        self.draw_track(renderer, layout.bounds());
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        let active = tree.state.downcast_ref::<Option<Thumb>>();
        let bounds = layout.bounds();
        if active.is_some() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(bounds) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::Point;

    fn lower(value: f32) -> f32 {
        value
    }
    fn upper(value: f32) -> f32 {
        value
    }
    fn range() -> DualRange<f32> {
        DualRange {
            theme: Theme::Forge,
            minimum: 0.0,
            maximum: 100.0,
            lower: 20.0,
            upper: 80.0,
            step: 1.0,
            on_lower: lower,
            on_upper: upper,
        }
    }

    #[test]
    fn two_handles_share_a_track_without_crossing() {
        let range = range();
        assert_eq!(range.nearest_thumb(158.0, 36.0), Thumb::Lower);
        assert_eq!(range.nearest_thumb(158.0, 123.0), Thumb::Upper);
        assert!((range.thumb_x(158.0, 80.0) - 121.9).abs() < 0.1);
        assert_eq!(range.value_at(158.0, 158.0, Thumb::Lower), 80.0);
        assert_eq!(range.value_at(158.0, 0.0, Thumb::Upper), 20.0);
        assert_eq!(range.value_at(158.0, 78.6, Thumb::Lower), 50.0);
    }

    #[test]
    fn drag_keeps_the_selected_thumb_after_crossing_the_midpoint() {
        let range = range();
        let bounds = Rectangle {
            x: 10.0,
            y: 20.0,
            width: 158.0,
            height: 18.0,
        };
        let mut active = None;
        let (_, pressed) = range.pointer_event(
            &mut active,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            bounds,
            mouse::Cursor::Available(Point::new(46.0, 29.0)),
        );
        assert_eq!(active, Some(Thumb::Lower));
        assert_eq!(pressed, Some(20.0));
        let (_, moved) = range.pointer_event(
            &mut active,
            Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(170.0, 29.0),
            }),
            bounds,
            mouse::Cursor::Available(Point::new(170.0, 29.0)),
        );
        assert_eq!(moved, Some(80.0));
    }
}
