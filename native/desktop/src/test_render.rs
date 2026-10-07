//! Drawing the window, or a part of it, into pixels with the software
//! renderer, for the tests that look at what the window shows: a colour at
//! a place, the height of a bar, the width of a text.

use std::sync::Once;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::Tree;
use iced::{mouse, Color, Element, Rectangle, Size, Theme};

use crate::{Message, Studio};

/// The pixels of a picture, row by row.
pub(crate) struct Picture {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// Four bytes a pixel, blue, green, red and alpha, as the renderer
    /// writes them.
    bytes: Vec<u8>,
}

impl Picture {
    /// The colour of the pixel at `x`, `y`, as red, green and blue.
    pub(crate) fn rgb(&self, x: u32, y: u32) -> [u8; 3] {
        assert!(x < self.width && y < self.height, "({x}, {y}) lies outside");
        let at = ((y * self.width + x) * 4) as usize;
        [self.bytes[at + 2], self.bytes[at + 1], self.bytes[at]]
    }

    /// Whether the pixel at `x`, `y` has the colour `color` laid over
    /// what the picture was cleared to, give or take one step a channel.
    pub(crate) fn is(&self, x: u32, y: u32, color: Color) -> bool {
        let [r, g, b, _] = color.into_rgba8();
        let [red, green, blue] = self.rgb(x, y);
        [red.abs_diff(r), green.abs_diff(g), blue.abs_diff(b)]
            .into_iter()
            .all(|difference| difference <= 1)
    }
}

/// What a picture is cleared to before the window is drawn: a colour the
/// window never uses, so that a pixel nothing covered is found out.
pub(crate) const CLEAR: Color = Color::from_rgb(1.0, 0.0, 1.0);

/// Make the fonts of the window known to the renderer, as the window does
/// when it opens.
pub(crate) fn load_fonts() {
    static LOADED: Once = Once::new();
    LOADED.call_once(|| {
        let mut fonts = iced_tiny_skia::graphics::text::font_system()
            .write()
            .expect("the font system");
        for bytes in crate::fonts::FILES {
            fonts.load_font(std::borrow::Cow::Borrowed(bytes));
        }
    });
}

/// Lay out `element` in `size` and draw it in `theme`.
pub(crate) fn render(element: Element<'_, Message>, theme: &Theme, size: Size) -> Picture {
    render_under(element, theme, size, mouse::Cursor::Unavailable)
}

/// As `render`, with the pointer at `cursor`.
pub(crate) fn render_under(
    element: Element<'_, Message>,
    theme: &Theme,
    size: Size,
    cursor: mouse::Cursor,
) -> Picture {
    load_fonts();
    let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        crate::fonts::REGULAR,
        iced::Pixels(12.0),
    ));
    let mut tree = Tree::new(&element);
    let node =
        element
            .as_widget()
            .layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    element.as_widget().draw(
        &tree,
        &mut renderer,
        theme,
        &renderer::Style {
            text_color: theme.palette().text,
        },
        Layout::new(&node),
        cursor,
        &Rectangle::with_size(size),
    );
    let iced::Renderer::Secondary(mut software) = renderer else {
        unreachable!("the software renderer was made above");
    };
    let (width, height) = (size.width as u32, size.height as u32);
    let mut bytes = vec![0u8; (width * height * 4) as usize];
    let mut pixels = tiny_skia::PixmapMut::from_bytes(&mut bytes, width, height)
        .expect("a picture of that size");
    let mut mask = tiny_skia::Mask::new(width, height).expect("a mask of that size");
    software.draw::<&str>(
        &mut pixels,
        &mut mask,
        &iced_tiny_skia::graphics::Viewport::with_physical_size(Size::new(width, height), 1.0),
        &[Rectangle::with_size(size)],
        CLEAR,
        &[],
    );
    Picture {
        width,
        height,
        bytes,
    }
}

/// The whole window of `studio` in `size`, in its theme.
pub(crate) fn render_window(studio: &Studio, size: Size) -> Picture {
    render(studio.view(), &studio.ui_theme.iced(), size)
}

/// The messages `element`, laid out in `size`, gives for `events` with the
/// pointer at `at`, after the pointer was moved there.
pub(crate) fn messages(
    mut element: Element<'_, Message>,
    size: Size,
    at: iced::Point,
    events: &[iced::Event],
) -> Vec<Message> {
    load_fonts();
    let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        crate::fonts::REGULAR,
        iced::Pixels(12.0),
    ));
    let mut tree = Tree::new(&element);
    let node =
        element
            .as_widget()
            .layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    let mut messages = Vec::new();
    let mut clipboard = iced::advanced::clipboard::Null;
    let moved = iced::Event::Mouse(mouse::Event::CursorMoved { position: at });
    for event in std::iter::once(moved).chain(events.iter().cloned()) {
        let mut shell = iced::advanced::Shell::new(&mut messages);
        let _ = element.as_widget_mut().on_event(
            &mut tree,
            event,
            Layout::new(&node),
            mouse::Cursor::Available(at),
            &renderer,
            &mut clipboard,
            &mut shell,
            &Rectangle::with_size(size),
        );
    }
    messages
}

/// Where the scrollable `id` of `element`, laid out in `size`, lies.
pub(crate) fn scrollable_bounds(
    element: Element<'_, Message>,
    size: Size,
    id: iced::widget::scrollable::Id,
) -> Option<Rectangle> {
    use iced::advanced::widget::{operation, Id, Operation};

    struct Find {
        id: Id,
        found: Option<Rectangle>,
    }
    impl Operation for Find {
        fn container(
            &mut self,
            _id: Option<&Id>,
            _bounds: Rectangle,
            operate_on_children: &mut dyn FnMut(&mut dyn Operation),
        ) {
            operate_on_children(self);
        }

        fn scrollable(
            &mut self,
            _state: &mut dyn operation::Scrollable,
            id: Option<&Id>,
            bounds: Rectangle,
            _content_bounds: Rectangle,
            _translation: iced::Vector,
        ) {
            if id == Some(&self.id) {
                self.found = Some(bounds);
            }
        }
    }

    load_fonts();
    let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        crate::fonts::REGULAR,
        iced::Pixels(12.0),
    ));
    let mut tree = Tree::new(&element);
    let node =
        element
            .as_widget()
            .layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    let mut find = Find {
        id: id.into(),
        found: None,
    };
    element
        .as_widget()
        .operate(&mut tree, Layout::new(&node), &renderer, &mut find);
    find.found
}

/// A press of `button` of the mouse at `at`, and its release.
pub(crate) fn click(button: mouse::Button) -> [iced::Event; 2] {
    [
        iced::Event::Mouse(mouse::Event::ButtonPressed(button)),
        iced::Event::Mouse(mouse::Event::ButtonReleased(button)),
    ]
}
