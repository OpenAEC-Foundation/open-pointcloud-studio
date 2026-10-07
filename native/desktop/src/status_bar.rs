//! The status bar of the style book along the bottom of the window: 22
//! pixels on the dark status colour, with what is going on and the totals
//! at the left, the name and the version of the application in the middle
//! and what the main area shows at the right. Every item is tinted under the
//! pointer; the items that do something are buttons.

use std::sync::atomic::Ordering;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, Widget};
use iced::advanced::{overlay, renderer, Clipboard, Shell};
use iced::widget::{button, container, row, text, Space};
use iced::{
    event, mouse, Alignment, Element, Event, Fill, Length, Point, Rectangle, Renderer, Size, Theme,
    Vector,
};

use crate::i18n::tr;
use crate::ui_theme::UiColors;
use crate::view_tabs::shortened;
use crate::{app_title, fonts, format_count, format_zoom_level, ui_style, ui_theme};
use crate::{CloudEntry, Message, Studio};

/// The height of the bar, its line along the top included.
pub(crate) const HEIGHT: f32 = 22.0;

/// The most characters the name of a drawing or a sheet takes at the right.
const SHOWN_CHARS: usize = 40;

/// An item of the bar: a label in the quieter colour and, after it, a value.
fn item<'a>(colors: &UiColors, label: &'a str, value: String) -> Element<'a, Message> {
    cell(
        row![
            text(label).size(12).color(colors.status_text_label),
            text(value)
                .size(12)
                .font(fonts::MEDIUM)
                .color(colors.status_text)
                .wrapping(text::Wrapping::None),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    )
}

/// The room of an item, tinted under the pointer.
fn cell<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    ui_style::hover_tint(
        container(content)
            .padding([0, 6])
            .height(Fill)
            .align_y(Alignment::Center),
        |colors| colors.status_hover,
    )
}

/// Cancel import as an item of the bar: tinted under the pointer over the
/// whole height of the bar, its name in the middle of that height as the
/// text of every other item.
fn cancel_import<'a>(message: Option<Message>) -> Element<'a, Message> {
    button(
        container(text(tr("Cancel import")).size(12))
            .height(Fill)
            .align_y(Alignment::Center),
    )
    .on_press_maybe(message)
    .padding([0, 6])
    .height(Fill)
    .style(ui_style::status_button)
    .into()
}

/// The line between two items.
fn separator<'a>() -> Element<'a, Message> {
    container(Space::new(1, 14))
        .style(|theme| {
            container::Style::default().background(ui_theme::colors(theme).status_separator)
        })
        .into()
}

/// A row of two parts packed from the left, as the template packs the
/// items of the bar, where the first gives way: the second is laid out at
/// its own width, and the first gets the room that leaves.
struct LeadRow<'a> {
    /// The part that gives way, and the part after it.
    children: [Element<'a, Message>; 2],
    spacing: f32,
}

impl Widget<Message, Theme, Renderer> for LeadRow<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn children(&self) -> Vec<Tree> {
        self.children.iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.children);
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let size = limits.max();
        let rest = self.children[1].as_widget().layout(
            &mut tree.children[1],
            renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let room = (size.width - rest.size().width - self.spacing).max(0.0);
        let lead = self.children[0].as_widget().layout(
            &mut tree.children[0],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::new(room, size.height)),
        );
        let middle = |node: &layout::Node| (size.height - node.size().height) / 2.0;
        let lead_top = middle(&lead);
        let rest_left = lead.size().width + self.spacing;
        let rest_top = middle(&rest);
        layout::Node::with_children(
            size,
            vec![
                lead.move_to(Point::new(0.0, lead_top)),
                rest.move_to(Point::new(rest_left, rest_top)),
            ],
        )
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
        for ((child, state), layout) in self
            .children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
        {
            child
                .as_widget()
                .draw(state, renderer, theme, style, layout, cursor, viewport);
        }
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        for ((child, state), layout) in self
            .children
            .iter()
            .zip(&mut tree.children)
            .zip(layout.children())
        {
            child
                .as_widget()
                .operate(state, layout, renderer, operation);
        }
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        self.children
            .iter_mut()
            .zip(&mut tree.children)
            .zip(layout.children())
            .map(|((child, state), layout)| {
                child.as_widget_mut().on_event(
                    state,
                    event.clone(),
                    layout,
                    cursor,
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                )
            })
            .fold(event::Status::Ignored, event::Status::merge)
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(&tree.children)
            .zip(layout.children())
            .map(|((child, state), layout)| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(&mut self.children, tree, layout, renderer, translation)
    }
}

/// The text with the first letter of each sentence of it a capital and
/// the rest small, as the style book writes its texts.
pub(crate) fn sentence_case(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first
            .to_uppercase()
            .chain(characters.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

impl Studio {
    /// What the main area shows, as the item at the right of the bar names
    /// it: the view of the 3D model, or the drawing or the sheet.
    pub(crate) fn shown_item(&self) -> (&'static str, String) {
        if let Some(caption) = self.layout_caption() {
            return (tr("Sheet:"), shortened(&caption, SHOWN_CHARS));
        }
        if self.drawing_view.shown {
            (
                tr("Drawing:"),
                shortened(&self.drawing_view_caption(), SHOWN_CHARS),
            )
        } else {
            (tr("View:"), sentence_case(self.view_caption()))
        }
    }

    /// The bar along the bottom of the window with `message` at its left.
    pub(crate) fn status_bar(&self, message: String) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let total_points: u64 = self.clouds.iter().map(CloudEntry::remaining_count).sum();
        // The message comes first and gives way to the totals after it: a
        // long one is cut off at its right instead of wrapping or pushing
        // them out.
        let message = cell(
            container(
                text(message)
                    .size(12)
                    .color(colors.status_text_label)
                    .wrapping(text::Wrapping::None),
            )
            .clip(true),
        );
        let mut totals = row![
            separator(),
            item(&colors, tr("Files:"), format_count(self.clouds.len())),
            separator(),
            item(&colors, tr("Points:"), format_count(total_points)),
            separator(),
            item(
                &colors,
                tr("Selected:"),
                format_count(self.selected_total())
            ),
        ]
        .spacing(12)
        .align_y(Alignment::Center);
        if let Some(runs) = self.extension_runs_status() {
            totals = totals.push(separator()).push(cell(runs));
        }
        let left = Element::new(LeadRow {
            children: [message, totals.into()],
            spacing: 12.0,
        });

        let mut right = row![].spacing(12).align_y(Alignment::Center);
        if let Some((&id, job)) = self.imports.iter().max_by_key(|(id, _)| *id) {
            right = right
                .push(cancel_import(
                    (!job.cancel.load(Ordering::Relaxed)).then_some(Message::CancelImport(id)),
                ))
                .push(separator());
        }
        let (label, value) = self.shown_item();
        right = right.push(item(&colors, label, value));
        // The zoom of the 3D view; a drawing and a sheet say theirs in
        // Properties.
        if self.layout_caption().is_none() && !self.drawing_view.shown {
            right = right.push(separator()).push(item(
                &colors,
                tr("Zoom:"),
                format_zoom_level(self.zoom),
            ));
        }

        let bar = row![
            container(left).width(Fill).clip(true),
            text(app_title())
                .size(11)
                .color(colors.status_text_label)
                .wrapping(text::Wrapping::None),
            container(right)
                .width(Fill)
                .align_x(Alignment::End)
                .clip(true),
        ]
        .spacing(12)
        .height(Fill)
        .align_y(Alignment::Center);
        iced::widget::column![
            container(Space::new(Fill, 1)).style(|theme| {
                container::Style::default().background(ui_theme::colors(theme).status_border)
            }),
            container(bar)
                .padding([0, 12])
                .width(Fill)
                .height(Length::Fixed(HEIGHT - 1.0))
                .style(|theme| {
                    let colors = ui_theme::colors(theme);
                    container::Style::default()
                        .background(colors.status_bg)
                        .color(colors.status_text)
                }),
        ]
        .height(Length::Fixed(HEIGHT))
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, TestLanguage};
    use crate::ui_theme::UiTheme;
    use crate::CameraPreset;

    /// Where the two parts of a row of `width` lie: the left edge and the
    /// width of each.
    fn lead_row(message: &str, width: f32) -> [(f32, f32); 2] {
        let row = Element::new(LeadRow {
            children: [
                container(text(message.to_owned()).wrapping(text::Wrapping::None))
                    .clip(true)
                    .into(),
                Space::new(100, 10).into(),
            ],
            spacing: 12.0,
        });
        let renderer = Renderer::Secondary(iced_tiny_skia::Renderer::new(
            fonts::REGULAR,
            iced::Pixels(12.0),
        ));
        let mut tree = Tree::new(&row);
        let node = row.as_widget().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(width, 21.0)),
        );
        let layout = Layout::new(&node);
        let mut parts = layout.children().map(|part| {
            let bounds = part.bounds();
            (bounds.x, bounds.width)
        });
        [parts.next().unwrap(), parts.next().unwrap()]
    }

    #[test]
    fn the_totals_follow_the_message_and_a_long_message_gives_way() {
        crate::test_render::load_fonts();
        // A short message: the totals right after it.
        let [(_, message), (totals, _)] = lead_row("Ready", 600.0);
        assert!(message < 60.0, "{message}");
        assert_eq!(totals, message + 12.0);
        // A long one is cut off where the totals begin, and they stay whole.
        let long = "Reading source for octree: 5177344 / 54373904 points (10%) ".repeat(4);
        let [(_, message), (totals, width)] = lead_row(&long, 600.0);
        assert!(
            message > 400.0 && message <= 600.0 - 100.0 - 12.0,
            "{message}"
        );
        assert_eq!((totals, width), (message + 12.0, 100.0));
    }

    /// How far down a bar of 21 pixels the text of `element` lies: the
    /// middle of the text, laid out as the first leaf of its tree.
    fn text_middle(element: Element<'_, Message>) -> f32 {
        let renderer = Renderer::Secondary(iced_tiny_skia::Renderer::new(
            fonts::REGULAR,
            iced::Pixels(12.0),
        ));
        let mut tree = Tree::new(&element);
        let node = element.as_widget().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(200.0, HEIGHT - 1.0)),
        );
        let mut layout = Layout::new(&node);
        while let Some(child) = layout.children().next() {
            layout = child;
        }
        layout.bounds().center_y()
    }

    #[test]
    fn cancel_import_is_in_line_with_the_other_items() {
        crate::test_render::load_fonts();
        let colors = UiTheme::Light.colors();
        let other = text_middle(item(&colors, "View:", "Top".to_owned()));
        let cancel = text_middle(cancel_import(Some(Message::CancelImport(1))));
        assert!((other - (HEIGHT - 1.0) / 2.0).abs() < 0.5, "{other}");
        assert!((cancel - other).abs() < 0.5, "{cancel} against {other}");
    }

    #[test]
    fn the_style_book_writes_in_sentence_case() {
        assert_eq!(sentence_case("ISOMETRISCH"), "Isometrisch");
        assert_eq!(sentence_case("BOVEN RECHTS"), "Boven rechts");
        assert_eq!(sentence_case("BOX SELECT ACTIVE"), "Box select active");
        assert_eq!(sentence_case(""), "");
    }

    #[test]
    fn the_bar_names_what_the_main_area_shows_in_the_language_in_use() {
        let _language = TestLanguage::hold(Language::Table(0));
        let mut studio = Studio::default();
        assert_eq!(studio.shown_item(), ("Weergave:", "Isometrisch".to_owned()));
        let _ = studio.update(Message::CameraPreset(CameraPreset::Top));
        assert_eq!(studio.shown_item(), ("Weergave:", "Boven".to_owned()));
        crate::i18n::set(Language::English);
        assert_eq!(studio.shown_item(), ("View:", "Top".to_owned()));
        studio.box_select = true;
        assert_eq!(
            studio.shown_item(),
            ("View:", "Box select active".to_owned())
        );
    }

    /// The bottom rows of the window of `studio`: how many of the column at
    /// `x` have the colour of the bar, from the bottom up, and the colour of
    /// the row above them.
    fn bar_rows(studio: &Studio, x: u32) -> (u32, [u8; 3]) {
        let picture = crate::test_render::render_window(studio, iced::Size::new(1200.0, 700.0));
        let colors = studio.ui_theme.colors();
        let mut rows = 0;
        while picture.is(x, picture.height - 1 - rows, colors.status_bg) {
            rows += 1;
        }
        (rows, picture.rgb(x, picture.height - 1 - rows))
    }

    #[test]
    fn the_bar_is_22_pixels_of_the_status_colour_under_its_line() {
        let mut studio = Studio::default();
        for theme in UiTheme::ALL {
            studio.ui_theme = theme;
            let colors = theme.colors();
            // At the left edge, in the padding of the bar.
            let (rows, above) = bar_rows(&studio, 4);
            assert_eq!(rows, 21, "{}", theme.key());
            let [r, g, b, _] = colors.status_border.into_rgba8();
            assert_eq!(above, [r, g, b], "{}", theme.key());
        }
        // Light keeps the bar dark, as the style book does.
        assert_eq!(
            UiTheme::Light.colors().status_bg,
            iced::Color::from_rgb8(0x36, 0x36, 0x3E)
        );
    }
}
