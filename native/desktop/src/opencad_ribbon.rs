// Adapted from OpenCADStudio/src/ui/ribbon/{mod.rs,widgets.rs,draw_panel.rs}
// at commit 1fec34d84a129a2e4c6466e184893f78ae596777.
// Copyright OpenCADStudio contributors. Licensed under GPL-3.0.
// Changes: adapted the ribbon primitives to iced 0.13 and point-cloud
// commands, and drawn as the ribbon of the OpenAEC style book: a tab row of
// 28 over content of 94, buttons in three sizes, groups with their caption
// under a line and a separator between them.

use std::cell::RefCell;
use std::collections::HashMap;

use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{button, column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Fill, Length, Padding, Theme};

use crate::{fonts, ui_style, ui_theme, Message};

/// The rows of the ribbon: the tabs, and the groups under them.
pub const TAB_ROW_H: f32 = 28.0;
pub const CONTENT_H: f32 = 94.0;
/// The ribbon from the top of its tabs to the bottom of its groups.
#[cfg(test)]
pub const HEIGHT: f32 = TAB_ROW_H + CONTENT_H;
/// The space above the groups, and their height under it.
const CONTENT_TOP: f32 = 4.0;
const GROUP_H: f32 = CONTENT_H - CONTENT_TOP;
/// The height of a large, a medium and a small button.
pub const LARGE_H: f32 = 66.0;
pub const MEDIUM_H: f32 = 33.0;
pub const SMALL_H: f32 = 22.0;
/// The narrowest a large button and a small or medium button get.
const LARGE_MIN_W: f32 = 44.0;
const SMALL_MIN_W: f32 = 70.0;
/// The narrowest a group gets.
const GROUP_MIN_W: f32 = 50.0;
/// The line of the caption of a group and the space above and below it.
const CAPTION_LINE: f32 = 13.5;
const CAPTION_PADDING: f32 = 2.0;
/// The width the name of a large button wraps in, in two lines at most.
pub const LABEL_WIDTH: f32 = 58.0;
/// The size of the name of a large button, and its line.
pub const LABEL_SIZE: f32 = 10.0;
const LABEL_LINE: f32 = 1.15;
/// The width and the height of a button in the quick-access row.
const QUICK_W: f32 = 28.0;
const QUICK_H: f32 = 24.0;

/// The size of a ribbon button, as the style book has three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonSize {
    /// The icon of 24 above a name of two lines at most, 66 high.
    Large,
    /// The icon of 20 before its name, 33 high.
    Medium,
    /// The icon of 16 before its name, 22 high.
    Small,
}

/// The font of the name of a ribbon button: medium while it is on.
pub fn label_font(active: bool) -> iced::Font {
    if active {
        fonts::MEDIUM
    } else {
        fonts::REGULAR
    }
}

/// What the font system had loaded when a width was measured.
type FontVersion = iced::advanced::graphics::text::Version;

/// How wide a name is, by its text and whether it is set in medium,
/// measured with the fonts of a version of the font system.
type Widths = (FontVersion, HashMap<(String, bool), f32>);

thread_local! {
    static WIDTHS: RefCell<Widths> = RefCell::new((FontVersion::default(), HashMap::new()));
}

/// The lines the name of a large button is set in: on one line when it fits
/// the label width of the style book, else broken at a space into two lines,
/// the first as full as the label width allows it. A word is never broken
/// and nothing is cut off: where the best of the two-line breaks is wider
/// than the label width, the label, and the button, are that wide.
pub fn label_lines(label: &str, active: bool) -> Vec<String> {
    let version = iced::advanced::graphics::text::font_system()
        .read()
        .map(|fonts| fonts.version())
        .unwrap_or_default();
    WIDTHS.with(|cache| {
        let (measured_with, widths) = &mut *cache.borrow_mut();
        // Widths measured before the fonts of the window were loaded are
        // those of another font.
        if *measured_with != version {
            *measured_with = version;
            widths.clear();
        }
        let mut measure = |part: &str| -> f32 {
            *widths
                .entry((part.to_owned(), active))
                .or_insert_with(|| fonts::width(part, LABEL_SIZE, label_font(active)))
        };
        lines_within(label, &mut measure)
    })
}

/// `label_lines` with the width of a text as `measure` gives it.
pub fn lines_within(label: &str, measure: &mut impl FnMut(&str) -> f32) -> Vec<String> {
    let words: Vec<&str> = label.split(' ').filter(|word| !word.is_empty()).collect();
    if words.len() < 2 || measure(label) <= LABEL_WIDTH {
        return vec![label.to_owned()];
    }
    // The width the label needs: that of the label width, or of the wider
    // line of the narrowest break in two.
    let narrowest = (1..words.len())
        .map(|split| measure(&words[..split].join(" ")).max(measure(&words[split..].join(" "))))
        .fold(f32::INFINITY, f32::min);
    let room = narrowest.max(LABEL_WIDTH);
    // The first line takes as many words as fit the room, as the template
    // fills its lines; the second line takes the rest.
    let mut first = 1;
    while first < words.len() - 1 && measure(&words[..=first].join(" ")) <= room {
        first += 1;
    }
    vec![words[..first].join(" "), words[first..].join(" ")]
}

/// A ribbon button: `icon` and the name `label`, already in the language of
/// the window, in the size of the style book, tinted under the pointer and
/// while it is on, at 40 % while it cannot be used (no `message`).
pub fn tool_button<'a>(
    size: ButtonSize,
    icon: impl Into<Element<'a, Message>>,
    label: &str,
    active: bool,
    message: Option<Message>,
) -> Element<'a, Message> {
    let font = label_font(active);
    let (content, padding, height): (Element<'a, Message>, Padding, f32) = match size {
        ButtonSize::Large => {
            let mut content = column![container(icon).center(28)]
                .spacing(1)
                .align_x(iced::Alignment::Center);
            for line in label_lines(label, active) {
                content = content.push(
                    text(line)
                        .size(LABEL_SIZE)
                        .line_height(LineHeight::Relative(LABEL_LINE))
                        .font(font)
                        .wrapping(Wrapping::None),
                );
            }
            // The padding of the style book inside its border of 1.
            (
                content.push(Space::with_width(LARGE_MIN_W - 8.0)).into(),
                Padding {
                    top: 4.0,
                    right: 4.0,
                    bottom: 2.0,
                    left: 4.0,
                },
                LARGE_H,
            )
        }
        ButtonSize::Medium | ButtonSize::Small => {
            let (icon_size, height) = if size == ButtonSize::Medium {
                (20.0, MEDIUM_H)
            } else {
                (16.0, SMALL_H)
            };
            let line = row![
                container(icon).center(icon_size),
                text(label.to_owned())
                    .size(11)
                    .font(font)
                    .wrapping(Wrapping::None),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            (
                column![line, Space::with_width(SMALL_MIN_W - 14.0)].into(),
                Padding {
                    top: (height - icon_size) / 2.0,
                    bottom: (height - icon_size) / 2.0,
                    left: 7.0,
                    right: 7.0,
                },
                height,
            )
        }
    };
    button(content)
        .padding(padding)
        .height(height)
        .on_press_maybe(message)
        .style(move |theme, status| ui_style::ribbon_button(theme, active, status))
        .into()
}

/// A small ribbon button without a name, as tall as a large one: the
/// arrows that scroll the ribbon. Its name is in its tooltip.
pub fn arrow_button<'a>(
    icon: impl Into<Element<'a, Message>>,
    name: &str,
    message: Option<Message>,
) -> Element<'a, Message> {
    let control = button(container(icon).center(16))
        .padding([(LARGE_H - 16.0) / 2.0, 3.0])
        .height(LARGE_H)
        .on_press_maybe(message)
        .style(|theme, status| ui_style::ribbon_button(theme, false, status));
    ui_style::ribbon_tooltip(control, name.to_owned()).into()
}

/// A button of the quick-access row: its icon of 16 in the accent colour,
/// named in its tooltip, with the key that does the same. Unavailable
/// actions stay visible and fade.
pub fn quick_access_btn<'a>(
    icon: impl Into<Element<'a, Message>>,
    label: &'static str,
    shortcut: Option<&str>,
    message: Option<Message>,
) -> Element<'a, Message> {
    let control = button(container(icon).center(16))
        .on_press_maybe(message)
        .style(ui_style::quick_button)
        .width(QUICK_W)
        .height(QUICK_H)
        .padding([(QUICK_H - 16.0) / 2.0, (QUICK_W - 16.0) / 2.0]);
    let name = crate::i18n::tr(label);
    let tip = match shortcut {
        Some(keys) => format!("{name} ({keys})"),
        None => name.to_owned(),
    };
    ui_style::ribbon_tooltip(control, tip).into()
}

pub enum RibbonItem<'a> {
    Large(Element<'a, Message>),
    Small(Element<'a, Message>),
}

/// OpenCADStudio's three-small-tools-per-column ribbon layout helper, with
/// the gap of 1 of the style book between the buttons of a stack.
pub fn flush_small_col<'a>(
    buf: &mut Vec<Element<'a, Message>>,
    out: &mut Vec<Element<'a, Message>>,
) {
    if buf.is_empty() {
        return;
    }
    let col = column(std::mem::take(buf)).spacing(1);
    out.push(col.into());
}

/// A group of tools of any shape above its caption.
pub fn render_group<'a>(title: &'static str, tools: Element<'a, Message>) -> Element<'a, Message> {
    render_group_items(title, vec![RibbonItem::Large(tools)])
}

/// A group of the ribbon: large tools side by side, small tools in stacks of
/// three, and under them its caption under a line, which sets the width of
/// the group when it is wider than the tools.
pub fn render_group_items<'a>(
    title: &'static str,
    items: Vec<RibbonItem<'a>>,
) -> Element<'a, Message> {
    let mut items_row: Vec<Element<'a, Message>> = Vec::new();
    let mut small_buf: Vec<Element<'a, Message>> = Vec::new();
    for item in items {
        match item {
            RibbonItem::Large(element) => {
                flush_small_col(&mut small_buf, &mut items_row);
                items_row.push(element);
            }
            RibbonItem::Small(element) => {
                small_buf.push(element);
                if small_buf.len() == 3 {
                    flush_small_col(&mut small_buf, &mut items_row);
                }
            }
        }
    }
    flush_small_col(&mut small_buf, &mut items_row);
    let tools = row(items_row)
        .spacing(1)
        .align_y(iced::Alignment::Start)
        .padding([1, 0]);
    let caption = text(crate::i18n::tr(title))
        .size(9)
        .font(fonts::MEDIUM)
        .line_height(LineHeight::Absolute(CAPTION_LINE.into()))
        .wrapping(Wrapping::None)
        .style(|theme| text::Style {
            color: Some(ui_theme::colors(theme).ribbon_group_label),
        });
    let body = column![
        container(tools).height(Fill),
        Space::with_height(1),
        container(caption).padding([CAPTION_PADDING, 0.0]),
        Space::with_width(GROUP_MIN_W - 16.0),
    ]
    .align_x(iced::Alignment::Center)
    .width(Length::Shrink)
    .height(Fill);
    // The line over the caption spans the group, as wide as its tools or
    // its caption, whichever is wider: a layer over the group, which knows
    // its width once the group is laid out.
    let rule = column![
        Space::with_height(Fill),
        line(|colors| colors.border_subtle).width(Fill).height(1),
        Space::with_height(CAPTION_LINE + 2.0 * CAPTION_PADDING),
    ]
    .width(Fill)
    .height(Fill);
    container(iced::widget::stack![body, rule])
        .padding([0, 8])
        .width(Length::Shrink)
        .height(GROUP_H)
        .into()
}

/// The groups of the ribbon side by side, each but the last with the line
/// of the style book at its right, over its whole height.
pub fn groups<'a>(groups: Vec<Element<'a, Message>>) -> iced::widget::Row<'a, Message> {
    let count = groups.len();
    let mut strip = row![].height(GROUP_H);
    for (place, group) in groups.into_iter().enumerate() {
        strip = strip.push(group);
        if place + 1 < count {
            strip = strip.push(
                line(|colors| colors.ribbon_group_separator)
                    .width(1)
                    .height(GROUP_H),
            );
        }
    }
    strip
}

/// The content of the ribbon under its tabs: 94 high on the shell, with
/// the space of the style book around the groups.
pub fn content<'a>(strip: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(strip)
        .padding(Padding {
            top: CONTENT_TOP,
            right: 8.0,
            bottom: 0.0,
            left: 2.0,
        })
        .width(Fill)
        .height(CONTENT_H)
        .style(|theme| container::Style::default().background(ui_theme::colors(theme).bg))
        .into()
}

/// The height of the groups, for what stands beside them.
pub const fn group_height() -> f32 {
    GROUP_H
}

/// A line or a bar in a colour of the theme.
pub fn line<'a>(color: fn(&ui_theme::UiColors) -> Color) -> container::Container<'a, Message> {
    container(Space::new(0, 0))
        .style(move |theme| container::Style::default().background(color(&ui_theme::colors(theme))))
}

/// The row of the tabs: the lighter shell with the accent line at its
/// bottom, the tabs standing on it from the left. The tab that is open
/// covers the line under it, so that it reads as one with the groups below.
pub fn tab_row<'a>(tabs: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let base = column![
        Space::with_height(TAB_ROW_H - 1.0),
        line(|colors| colors.accent).width(Fill).height(1),
    ]
    .width(Fill)
    .height(TAB_ROW_H);
    let strip = row(tabs).spacing(2).height(TAB_ROW_H).padding([0, 8]);
    container(iced::widget::stack![base, strip])
        .width(Fill)
        .height(TAB_ROW_H)
        .style(|theme| container::Style::default().background(ui_theme::colors(theme).bg_lighter))
        .into()
}

/// The text of a tab, 14 high in the 28 of the row.
fn tab_text<'a>(label: &str, font: iced::Font) -> iced::widget::Text<'a> {
    text(label.to_owned())
        .size(12)
        .font(font)
        .line_height(LineHeight::Absolute(14.0.into()))
        .wrapping(Wrapping::None)
}

/// The width of a tab with `label`: 16 beside it and the border, in whole
/// pixels, so that the outline of the open tab and the gap under it fall on
/// pixels.
fn tab_width(label: &str, font: iced::Font) -> f32 {
    (fonts::width(label, 12.0, font) + 34.0).ceil()
}

/// A tab of the ribbon. The open one has the colour of the groups and the
/// accent around it, open at its bottom.
pub fn tab<'a>(label: &str, open: bool, message: Option<Message>) -> Element<'a, Message> {
    let control = button(tab_text(label, fonts::MEDIUM))
        .on_press_maybe(message)
        .padding([7, 17])
        .width(tab_width(label, fonts::MEDIUM))
        .height(TAB_ROW_H)
        .style(move |theme, status| tab_style(theme, open, status));
    if !open {
        return control.into();
    }
    // The gap of the style book: the shell over the bottom of the outline
    // and the line under it, between the sides of the tab.
    let gap = container(line(|colors| colors.bg).width(Fill).height(2))
        .padding(Padding {
            left: 1.0,
            right: 1.0,
            ..Padding::ZERO
        })
        .width(Fill)
        .height(TAB_ROW_H)
        .align_y(iced::Alignment::End);
    iced::widget::stack![control, gap].into()
}

/// The File tab: always first, in the accent colour, which opens the File
/// view and is never the open tab.
pub fn file_tab<'a>(label: &str, message: Message) -> Element<'a, Message> {
    button(tab_text(label, fonts::SEMIBOLD))
        .on_press(message)
        .padding([7, 17])
        .width(tab_width(label, fonts::SEMIBOLD))
        .height(TAB_ROW_H)
        .style(file_tab_style)
        .into()
}

/// A tab of the ribbon: the open one joins the groups below it, outlined in
/// the accent colour; the others are tinted under the pointer.
pub fn tab_style(theme: &Theme, open: bool, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let background = if open {
        Some(colors.bg)
    } else if hovered {
        Some(colors.ribbon_btn_hover)
    } else {
        None
    };
    button::Style {
        background: background.map(Background::Color),
        text_color: if open || hovered {
            colors.accent
        } else {
            colors.text
        },
        border: Border {
            color: if open {
                colors.accent
            } else {
                Color::TRANSPARENT
            },
            width: 1.0,
            radius: iced::border::Radius::default().top(4),
        },
        ..button::Style::default()
    }
}

/// The File tab, filled with the accent colour, Signal Orange under the
/// pointer, with its border in its fill.
pub fn file_tab_style(theme: &Theme, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    let fill = if matches!(status, button::Status::Hovered | button::Status::Pressed) {
        colors.file_tab_hover
    } else {
        colors.file_tab_bg
    };
    button::Style {
        background: Some(Background::Color(fill)),
        text_color: colors.file_tab_text,
        border: Border {
            color: fill,
            width: 1.0,
            radius: iced::border::Radius::default().top(4),
        },
        ..button::Style::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A width of seven pixels a character, enough to tell the breaks apart.
    fn sevens(part: &str) -> f32 {
        part.chars().count() as f32 * 7.0
    }

    #[test]
    fn a_name_breaks_at_a_space_into_two_lines_at_most() {
        let lines = |label: &str| lines_within(label, &mut sevens);
        // Within the label width: one line.
        assert_eq!(lines("Zoom all"), ["Zoom all"]);
        // Wider: two lines, the first as full as the room allows.
        assert_eq!(lines("Section drawing"), ["Section", "drawing"]);
        // "Pointcloud" and "to Drawing" (10 each) make the narrowest break.
        assert_eq!(lines("Pointcloud to Drawing"), ["Pointcloud", "to Drawing"]);
        // The room is the wider line of the narrowest break when that is
        // wider than the label width: "naar tekening" (13) is narrower than
        // "Puntenwolk naar" (15).
        assert_eq!(
            lines("Puntenwolk naar tekening"),
            ["Puntenwolk", "naar tekening"]
        );
        // A long word is never broken, nor a name of three words put in
        // three lines.
        assert_eq!(lines("Snedetekening"), ["Snedetekening"]);
        assert_eq!(lines("a b c d e f g h i j"), ["a b c d e", "f g h i j"]);
        for label in [
            "Zoom all",
            "Section drawing",
            "Puntenwolk naar tekening",
            "Index annuleren",
        ] {
            let found = lines(label);
            assert!(found.len() <= 2);
            assert_eq!(found.join(" "), label);
        }
    }
}
