//! A reusable anchored overlay for application-owned controls.
use iced::{
    advanced::{layout, mouse, overlay, renderer::{self, Renderer as _}, widget::Tree, Clipboard, Layout, Shell, Widget},
    event::{self, Event},
    Element, Length, Point, Rectangle, Size, Vector,
};

pub fn popover<'a, M: Clone + 'a>(
    anchor: Element<'a, M>,
    content: Element<'a, M>,
    open: bool,
    on_close: M,
) -> Element<'a, M> {
    popover_aligned(anchor, content, open, on_close, false)
}

/// Place the trailing edge of the popup at the trailing edge of its anchor.
pub fn popover_aligned<'a, M: Clone + 'a>(
    anchor: Element<'a, M>, content: Element<'a, M>, open: bool, on_close: M, align_end: bool,
) -> Element<'a, M> {
    Element::new(Popover { anchor, content, open, on_close, align_end })
}

struct Popover<'a, M> {
    anchor: Element<'a, M>,
    content: Element<'a, M>,
    open: bool,
    on_close: M,
    align_end: bool,
}

impl<M: Clone> Widget<M, iced::Theme, iced::Renderer> for Popover<'_, M> {
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.anchor), Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&[self.anchor.as_widget(), self.content.as_widget()]);
    }

    fn size(&self) -> Size<Length> { self.anchor.as_widget().size() }

    fn layout(&self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) -> layout::Node {
        self.anchor.as_widget().layout(&mut tree.children[0], renderer, limits)
    }

    fn on_event(&mut self, tree: &mut Tree, event: Event, layout: Layout<'_>, cursor: mouse::Cursor,
        renderer: &iced::Renderer, clipboard: &mut dyn Clipboard, shell: &mut Shell<'_, M>,
        viewport: &Rectangle) -> event::Status {
        self.anchor.as_widget_mut().on_event(&mut tree.children[0], event, layout, cursor,
            renderer, clipboard, shell, viewport)
    }

    fn mouse_interaction(&self, tree: &Tree, layout: Layout<'_>, cursor: mouse::Cursor,
        viewport: &Rectangle, renderer: &iced::Renderer) -> mouse::Interaction {
        self.anchor.as_widget().mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn draw(&self, tree: &Tree, renderer: &mut iced::Renderer, theme: &iced::Theme,
        style: &renderer::Style, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle) {
        self.anchor.as_widget().draw(&tree.children[0], renderer, theme, style, layout, cursor, viewport);
    }

    fn overlay<'b>(&'b mut self, tree: &'b mut Tree, layout: Layout<'_>, renderer: &iced::Renderer,
        translation: Vector) -> Option<overlay::Element<'b, M, iced::Theme, iced::Renderer>> {
        let mut children = tree.children.iter_mut();
        let anchor_overlay = self.anchor.as_widget_mut().overlay(
            children.next().expect("popover anchor tree"), layout, renderer, translation);
        let content_tree = children.next().expect("popover content tree");
        let popup = self.open.then(|| overlay::Element::new(Box::new(PopoverOverlay {
            anchor: layout.bounds(), translation, content: &mut self.content,
            tree: content_tree, on_close: self.on_close.clone(), align_end: self.align_end,
        })));
        let overlays: Vec<_> = anchor_overlay.into_iter().chain(popup).collect();
        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

struct PopoverOverlay<'a, 'b, M> {
    anchor: Rectangle,
    translation: Vector,
    content: &'b mut Element<'a, M>,
    tree: &'b mut Tree,
    on_close: M,
    align_end: bool,
}

impl<M: Clone> overlay::Overlay<M, iced::Theme, iced::Renderer> for PopoverOverlay<'_, '_, M> {
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let limits = layout::Limits::new(Size::ZERO, bounds);
        let node = self.content.as_widget().layout(self.tree, renderer, &limits);
        let size = node.size();
        let anchor = self.anchor.position() + self.translation;
        let target_x = if self.align_end { anchor.x + self.anchor.width - size.width } else { anchor.x };
        let x = target_x.min((bounds.width - size.width).max(0.0)).max(0.0);
        let below = anchor.y + self.anchor.height;
        let y = if below + size.height <= bounds.height { below }
            else { (anchor.y - size.height).max(0.0) };
        node.move_to(Point::new(x, y))
    }

    fn draw(&self, renderer: &mut iced::Renderer, theme: &iced::Theme, style: &renderer::Style,
        layout: Layout<'_>, cursor: mouse::Cursor) {
        renderer.with_layer(layout.bounds(), |renderer| {
            self.content.as_widget().draw(self.tree, renderer, theme, style, layout, cursor,
                &Rectangle::with_size(Size::INFINITY));
        });
    }

    fn on_event(&mut self, event: Event, layout: Layout<'_>, cursor: mouse::Cursor,
        renderer: &iced::Renderer, clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>) -> event::Status {
        if matches!(event, Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left))
            | Event::Touch(iced::touch::Event::FingerPressed { .. }))
            && cursor.position().is_some_and(|point| !layout.bounds().contains(point)) {
            shell.publish(self.on_close.clone());
            return event::Status::Captured;
        }
        self.content.as_widget_mut().on_event(self.tree, event, layout, cursor, renderer,
            clipboard, shell, &Rectangle::with_size(Size::INFINITY))
    }

    fn mouse_interaction(&self, layout: Layout<'_>, cursor: mouse::Cursor, viewport: &Rectangle,
        renderer: &iced::Renderer) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(self.tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(&'b mut self, layout: Layout<'_>, renderer: &iced::Renderer)
        -> Option<overlay::Element<'b, M, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(self.tree, layout, renderer, Vector::ZERO)
    }
}
