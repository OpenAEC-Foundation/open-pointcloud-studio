//! Drawing the window, or a part of it, into pixels with the software
//! renderer, for the tests that look at what the window shows: a colour at
//! a place, the height of a bar, the width of a text.

use std::sync::Once;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::Tree;
use iced::{mouse, Color, Element, Rectangle, Size, Theme, Vector};

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
    /// Save a software-rendered review image without launching a desktop window.
    pub(crate) fn save_png(&self, path: &std::path::Path) {
        let size = tiny_skia::IntSize::from_wh(self.width, self.height).expect("valid image size");
        let mut rgba = self.bytes.clone();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2); // iced_tiny_skia gives BGRA; PNG encoding expects RGBA.
        }
        tiny_skia::Pixmap::from_vec(rgba, size)
            .expect("valid software-rendered pixels")
            .save_png(path)
            .expect("write review image");
    }

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
    render_after(element, theme, size, cursor, &[]).0
}

/// As `render_under`, after `events` have reached the element with the
/// pointer at `cursor`, such as a turn of the wheel that scrolls a list;
/// with the messages those events sent.
pub(crate) fn render_after(
    mut element: Element<'_, Message>,
    theme: &Theme,
    size: Size,
    cursor: mouse::Cursor,
    events: &[iced::Event],
) -> (Picture, Vec<Message>) {
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
    let mut messages = Vec::new();
    for event in events {
        let mut shell = iced::advanced::Shell::new(&mut messages);
        let _ = element.as_widget_mut().on_event(
            &mut tree,
            event.clone(),
            Layout::new(&node),
            cursor,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &Rectangle::with_size(size),
        );
    }
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
    if let Some(mut overlay) =
        element
            .as_widget_mut()
            .overlay(&mut tree, Layout::new(&node), &renderer, Vector::ZERO)
    {
        let popup_node = overlay.layout(&renderer, size);
        overlay.draw(
            &mut renderer,
            theme,
            &renderer::Style {
                text_color: theme.palette().text,
            },
            Layout::new(&popup_node),
            cursor,
        );
    }
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
    (
        Picture {
            width,
            height,
            bytes,
        },
        messages,
    )
}

/// The whole window of `studio` in `size`, in its theme.
pub(crate) fn render_window(studio: &Studio, size: Size) -> Picture {
    render(studio.view(), &studio.ui_theme.iced(), size)
}
