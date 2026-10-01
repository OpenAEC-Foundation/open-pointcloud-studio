// Adapted from OpenCADStudio/src/ui/ribbon/{mod.rs,widgets.rs,draw_panel.rs}
// at commit 1fec34d84a129a2e4c6466e184893f78ae596777.
// Copyright OpenCADStudio contributors. Licensed under GPL-3.0.
// Changes: adapted the ribbon primitives to iced 0.13 and point-cloud commands.

use iced::widget::{button, column, container, row, text};
use iced::{Background, Border, Color, Element, Fill, Length, Theme};

use crate::ui_theme;
use crate::Message;

pub const ROW_H: f32 = 26.0;
pub const TOOL_BAR_H: f32 = 3.0 * ROW_H + 18.0;

pub enum RibbonItem<'a> {
    Large(Element<'a, Message>),
    Small(Element<'a, Message>),
}

/// OpenCADStudio's three-small-tools-per-column ribbon layout helper.
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

/// OpenCADStudio button behavior with OpenAEC dark ribbon tokens.
pub fn tool_btn_style(theme: &Theme, is_active: bool, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    let (background, text_color, border) = match (is_active, status) {
        (_, button::Status::Disabled) => (None, colors.muted, Color::TRANSPARENT),
        (true, _) => (
            Some(colors.active),
            if colors.shell == Color::BLACK {
                Color::BLACK
            } else {
                colors.accent
            },
            Color {
                a: 0.4,
                ..colors.accent
            },
        ),
        (_, button::Status::Hovered | button::Status::Pressed) => (
            Some(colors.hover),
            colors.text,
            Color {
                a: 0.3,
                ..colors.accent
            },
        ),
        _ => (None, colors.text, Color::TRANSPARENT),
    };
    button::Style {
        background: background.map(Background::Color),
        text_color,
        border: Border {
            radius: 2.0.into(),
            color: border,
            width: 1.0,
        },
        shadow: iced::Shadow::default(),
    }
}

/// OpenCADStudio's panel structure: tools above a centered muted title.
pub fn render_group<'a>(title: &'static str, tools: Element<'a, Message>) -> Element<'a, Message> {
    render_group_items(title, vec![RibbonItem::Large(tools)])
}

/// Port of OpenCADStudio's `render_group`: large tools get full-height cells;
/// small tools are packed in columns of three, followed by the panel title.
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
    let tools = row(items_row).spacing(2).height(Fill).width(Length::Shrink);
    let content = column![
        container(tools)
            .height(TOOL_BAR_H - 18.0)
            .align_y(iced::Alignment::Start),
        container(
            column![
                container(text(""))
                    .width(Fill)
                    .height(1)
                    .style(|theme| container::Style::default().background(Color {
                        a: 0.15,
                        ..ui_theme::colors(theme).accent
                    })),
                text(title).size(9).style(|theme| text::Style {
                    color: Some(Color {
                        a: 0.8,
                        ..ui_theme::colors(theme).accent
                    }),
                }),
            ]
            .align_x(iced::Alignment::Center)
            .spacing(0),
        )
        .width(Fill)
        .align_x(iced::Alignment::Center),
    ]
    .align_x(iced::Alignment::Center)
    .spacing(0)
    .padding([2u16, 4])
    .width(Length::Shrink)
    .height(TOOL_BAR_H);

    row![
        content,
        container(text(""))
            .width(1)
            .height(TOOL_BAR_H - 10.0)
            .style(|theme| container::Style::default().background(Color {
                a: 0.15,
                ..ui_theme::colors(theme).text
            })),
    ]
    .spacing(5)
    .width(Length::Shrink)
    .into()
}

/// Compact document tabs, with the active tab joined visually to the ribbon.
pub fn tab_style(theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    button::Style {
        background: Some(Background::Color(if active {
            colors.shell
        } else if matches!(status, button::Status::Hovered) {
            colors.hover
        } else {
            colors.tabs
        })),
        text_color: if active { colors.accent } else { colors.text },
        border: Border {
            color: if active {
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
