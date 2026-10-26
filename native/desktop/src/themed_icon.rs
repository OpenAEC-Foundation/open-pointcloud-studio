//! The artwork of this application's own icons, drawn in the theme it is
//! shown in. The older pictures draw their outlines, and the disc under the
//! cross of Clear, in fixed dark greys, which are clear on the light theme
//! and next to invisible on the dark ones; those greys take the text colour
//! of the theme, and the amber and the other colours of a picture stay as
//! they are.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget};
use iced::widget::svg;
use iced::{mouse, Color, Element, Length, Rectangle, Renderer, Size, Theme};

use crate::{ui_theme, Message};

/// The dark greys of the older artwork, as its files write them.
pub(crate) const DARK_GREYS: [&str; 3] = ["#27272A", "#36363E", "#1a1a1a"];

/// `source`, an SVG of this application, as an icon of `size` pixels whose
/// dark greys are the text colour of the theme it is drawn in.
pub(crate) fn themed_icon<'a>(source: &'static [u8], size: f32) -> Element<'a, Message> {
    Element::new(ThemedIcon { source, size })
}

/// `source` with its dark greys in `ink`. A handle is made once for each
/// picture and colour, so that drawing the window again neither rewrites
/// nor hashes the SVG and the renderer keeps what it rasterized.
pub(crate) fn handle(source: &'static [u8], ink: Color) -> svg::Handle {
    type Key = (usize, usize, [u8; 4]);
    static CACHE: OnceLock<Mutex<HashMap<Key, svg::Handle>>> = OnceLock::new();
    // The pictures are included in the program, so their address tells
    // them apart.
    let key = (source.as_ptr() as usize, source.len(), ink.into_rgba8());
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache
        .entry(key)
        .or_insert_with(|| {
            let [red, green, blue, _] = ink.into_rgba8();
            let hex = format!("#{red:02X}{green:02X}{blue:02X}");
            let text = String::from_utf8_lossy(source);
            let recoloured = DARK_GREYS
                .into_iter()
                .fold(text.into_owned(), |text, grey| text.replace(grey, &hex));
            svg::Handle::from_memory(recoloured.into_bytes())
        })
        .clone()
}

/// An icon that finds the colours of its picture in the theme it is drawn
/// in, as a style finds those of a button.
struct ThemedIcon {
    source: &'static [u8],
    size: f32,
}

impl Widget<Message, Theme, Renderer> for ThemedIcon {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.resolve(self.size, self.size, Size::ZERO))
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let ink = ui_theme::colors(theme).text;
        let picture: svg::Svg<'_, Theme> = svg(handle(self.source, ink))
            .width(self.size)
            .height(self.size);
        Widget::<Message, Theme, Renderer>::draw(
            &picture, tree, renderer, theme, style, layout, cursor, viewport,
        );
    }
}
