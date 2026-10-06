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
        for bytes in crate::FONTS {
            fonts.load_font(std::borrow::Cow::Borrowed(bytes));
        }
    });
}

/// Lay out `element` in `size` and draw it in `theme`.
pub(crate) fn render(element: Element<'_, Message>, theme: &Theme, size: Size) -> Picture {
    load_fonts();
    let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        iced::Font::with_name("Inter"),
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
        mouse::Cursor::Unavailable,
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
