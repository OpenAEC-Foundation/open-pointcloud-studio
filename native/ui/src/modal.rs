//! Full-window modal placement and title-bar dragging shared by every studio.
use crate::Theme;
use iced::{
    advanced::{
        layout, overlay, renderer::{self, Renderer as _}, widget::{tree::{self, Tree}, Operation},
        Clipboard, Layout, Shell, Widget,
    },
    event::{self, Event}, mouse, Element, Length, Point, Rectangle, Renderer, Size, Vector,
};

#[derive(Default)]
struct DragState {
    position: Option<Point>,
    grab: Option<Vector>,
}

pub(crate) fn modal<'a, M: 'a>(card: Element<'a, M>, theme: Theme) -> Element<'a, M> {
    Element::new(Modal { card, theme })
}

struct Modal<'a, M> {
    card: Element<'a, M>,
    theme: Theme,
}

impl<M> Widget<M, iced::Theme, Renderer> for Modal<'_, M> {
    fn tag(&self) -> tree::Tag { tree::Tag::of::<DragState>() }
    fn state(&self) -> tree::State { tree::State::new(DragState::default()) }
    fn children(&self) -> Vec<Tree> { vec![Tree::new(&self.card)] }
    fn diff(&self, tree: &mut Tree) { tree.diff_children(&[self.card.as_widget()]); }
    fn size(&self) -> Size<Length> { Size::new(Length::Fill, Length::Fill) }

    fn layout(&self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        let available = limits.max();
        let card_limits = layout::Limits::new(Size::ZERO, Size::new(
            available.width,
            available.height * 0.9,
        ));
        let card = self.card.as_widget().layout(&mut tree.children[0], renderer, &card_limits);
        let size = card.size();
        let state = tree.state.downcast_ref::<DragState>();
        let centered = Point::new(
            (available.width - size.width) / 2.0,
            (available.height - size.height) / 2.0,
        );
        let wanted = state.position.unwrap_or(centered);
        let position = Point::new(
            wanted.x.clamp(0.0, (available.width - size.width).max(0.0)),
            wanted.y.clamp(0.0, (available.height - size.height).max(0.0)),
        );
        layout::Node::with_children(available, vec![card.move_to(position)])
    }

    fn operate(&self, tree: &mut Tree, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        let child = layout.children().next().expect("modal card layout");
        self.card.as_widget().operate(&mut tree.children[0], child, renderer, operation);
    }

    fn on_event(&mut self, tree: &mut Tree, event: Event, layout: Layout<'_>, cursor: mouse::Cursor,
        renderer: &Renderer, clipboard: &mut dyn Clipboard, shell: &mut Shell<'_, M>,
        viewport: &Rectangle) -> event::Status {
        let child = layout.children().next().expect("modal card layout");
        let card = child.bounds();
        let state = tree.state.downcast_mut::<DragState>();
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(point) = cursor.position() {
                    let header = Rectangle { x: card.x, y: card.y, width: (card.width - 30.0).max(0.0), height: 30.0 };
                    if header.contains(point) {
                        state.grab = Some(point - card.position());
                        return event::Status::Captured;
                    }
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) if state.grab.is_some() => {
                let grab = state.grab.expect("active modal grab");
                let bounds = layout.bounds();
                state.position = Some(Point::new(
                    (position.x - bounds.x - grab.x).clamp(0.0, (bounds.width - card.width).max(0.0)),
                    (position.y - bounds.y - grab.y).clamp(0.0, (bounds.height - card.height).max(0.0)),
                ));
                shell.invalidate_layout();
                return event::Status::Captured;
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) if state.grab.take().is_some() => {
                return event::Status::Captured;
            }
            _ => {}
        }
        if let event::Status::Captured = self.card.as_widget_mut().on_event(
            &mut tree.children[0], event.clone(), child, cursor, renderer, clipboard, shell, viewport,
        ) { return event::Status::Captured; }
        if matches!(event, Event::Mouse(_) | Event::Touch(_)) { event::Status::Captured }
        else { event::Status::Ignored }
    }

    fn mouse_interaction(&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor,
        viewport: &Rectangle, renderer: &Renderer) -> mouse::Interaction {
        let child = layout.children().next().expect("modal card layout");
        let card = child.bounds();
        if tree.state.downcast_ref::<DragState>().grab.is_some() { return mouse::Interaction::Grabbing; }
        let header = Rectangle { x: card.x, y: card.y, width: (card.width - 30.0).max(0.0), height: 30.0 };
        if cursor.is_over(header) { return mouse::Interaction::Grab; }
        self.card.as_widget().mouse_interaction(&tree.children[0], child, cursor, viewport, renderer)
    }

    fn draw(&self, tree: &Tree, renderer: &mut Renderer, theme: &iced::Theme,
        style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle) {
        renderer.fill_quad(renderer::Quad { bounds: layout.bounds(), ..renderer::Quad::default() },
            self.theme.tokens().dialog_overlay);
        let child = layout.children().next().expect("modal card layout");
        self.card.as_widget().draw(&tree.children[0], renderer, theme, style, child, cursor, viewport);
    }

    fn overlay<'b>(&'b mut self, tree: &'b mut Tree, layout: Layout<'_>, renderer: &Renderer,
        translation: Vector) -> Option<overlay::Element<'b, M, iced::Theme, Renderer>> {
        let child = layout.children().next().expect("modal card layout");
        self.card.as_widget_mut().overlay(&mut tree.children[0], child, renderer, translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::{clipboard, layout, widget::Tree};
    use iced::widget::{container, text};

    fn test_renderer() -> Renderer {
        let software = iced_tiny_skia::Renderer::new(crate::fonts::BODY, iced::Pixels(12.0));
        #[cfg(feature = "gpu-renderer")]
        { Renderer::Secondary(software) }
        #[cfg(not(feature = "gpu-renderer"))]
        { software }
    }

    fn card() -> Element<'static, ()> {
        container(text("Dialog"))
            .width(480).height(500)
            .into()
    }

    #[test]
    fn dialog_caps_height_at_ninety_percent_of_viewport() {
        let element = modal(card(), Theme::Light);
        let mut tree = Tree::new(&element);
        let renderer = test_renderer();
        let node = element.as_widget().layout(&mut tree, &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(1280.0, 480.0)));
        let card = Layout::new(&node).children().next().unwrap().bounds();
        assert!(card.height <= 432.0, "card is {}px in 480px viewport", card.height);
    }

    #[test]
    fn settings_footer_stays_inside_short_window() {
        let element = crate::widgets::settings_modal::<()>(
            "Settings", text("General").into(), text("Fields").into(),
            text("Save").into(), (), Theme::Light,
        );
        let mut tree = Tree::new(&element);
        let renderer = test_renderer();
        let node = element.as_widget().layout(&mut tree, &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(1280.0, 480.0)));
        let card = Layout::new(&node).children().next().unwrap();
        assert!(card.bounds().height <= 432.0);
        let column = card.children().next().unwrap();
        let footer = column.children().nth(2).unwrap();
        assert!(footer.bounds().y + footer.bounds().height <= card.bounds().y + card.bounds().height,
            "settings footer extends below the capped card");
    }

    #[test]
    fn general_footer_stays_inside_short_window() {
        let element = crate::widgets::dialog_modal::<()>(
            "Dialog", iced::widget::Space::new(480, 500).into(),
            text("Save").into(), (), 480.0, Theme::Light,
        );
        let mut tree = Tree::new(&element);
        let renderer = test_renderer();
        let node = element.as_widget().layout(&mut tree, &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(1280.0, 480.0)));
        let card = Layout::new(&node).children().next().unwrap();
        let column = card.children().next().unwrap();
        let footer = column.children().nth(2).unwrap();
        assert!(footer.bounds().y + footer.bounds().height <= card.bounds().y + card.bounds().height,
            "general footer extends below the capped card");
    }

    #[test]
    fn dragging_header_clamps_dialog_to_window_edges() {
        let mut element = modal(card(), Theme::Light);
        let mut tree = Tree::new(&element);
        let renderer = test_renderer();
        let limits = layout::Limits::new(Size::ZERO, Size::new(1280.0, 800.0));
        let mut node = element.as_widget().layout(&mut tree, &renderer, &limits);
        let centered = Layout::new(&node).children().next().unwrap().bounds();
        assert_eq!((centered.x, centered.y), (400.0, 150.0));
        let mut clipboard = clipboard::Null;
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        let viewport = Rectangle::with_size(Size::new(1280.0, 800.0));
        let press = Point::new(450.0, 165.0);
        assert_eq!(element.as_widget_mut().on_event(&mut tree,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Layout::new(&node), mouse::Cursor::Available(press), &renderer,
            &mut clipboard, &mut shell, &viewport), event::Status::Captured);
        assert_eq!(element.as_widget_mut().on_event(&mut tree,
            Event::Mouse(mouse::Event::CursorMoved { position: Point::new(-100.0, -100.0) }),
            Layout::new(&node), mouse::Cursor::Available(Point::new(-100.0, -100.0)), &renderer,
            &mut clipboard, &mut shell, &viewport), event::Status::Captured);
        node = element.as_widget().layout(&mut tree, &renderer, &limits);
        let moved = Layout::new(&node).children().next().unwrap().bounds();
        assert_eq!((moved.x, moved.y), (0.0, 0.0));
        assert_eq!(element.as_widget_mut().on_event(&mut tree,
            Event::Mouse(mouse::Event::CursorMoved { position: Point::new(2000.0, 2000.0) }),
            Layout::new(&node), mouse::Cursor::Available(Point::new(2000.0, 2000.0)), &renderer,
            &mut clipboard, &mut shell, &viewport), event::Status::Captured);
        node = element.as_widget().layout(&mut tree, &renderer, &limits);
        let moved = Layout::new(&node).children().next().unwrap().bounds();
        assert_eq!((moved.x, moved.y), (800.0, 300.0));
        let _ = element.as_widget_mut().on_event(&mut tree,
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            Layout::new(&node), mouse::Cursor::Available(Point::new(2000.0, 2000.0)), &renderer,
            &mut clipboard, &mut shell, &viewport);
        assert!(tree.state.downcast_ref::<DragState>().grab.is_none());
        let close = Point::new(moved.x + moved.width - 10.0, moved.y + 15.0);
        let _ = element.as_widget_mut().on_event(&mut tree,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Layout::new(&node), mouse::Cursor::Available(close), &renderer,
            &mut clipboard, &mut shell, &viewport);
        assert!(tree.state.downcast_ref::<DragState>().grab.is_none(),
            "close-button area must not start a drag");
    }
}
