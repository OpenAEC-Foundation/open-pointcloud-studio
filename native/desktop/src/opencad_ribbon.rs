// Adapted from OpenCADStudio/src/ui/ribbon/{mod.rs,widgets.rs,draw_panel.rs}
// at commit 1fec34d84a129a2e4c6466e184893f78ae596777.
// Copyright OpenCADStudio contributors. Licensed under GPL-3.0.
// Changes: adapted the ribbon primitives to iced 0.13 and point-cloud commands.

use iced::widget::{button, column, container, row, text};
use iced::{Background, Border, Element, Fill, Length, Theme};

use crate::{ui_style, ui_theme, Message};

pub const ROW_H: f32 = 22.0;
pub const TOOL_BAR_H: f32 = 3.0 * ROW_H + 20.0;
pub const QUICK_ACCESS_W: f32 = 26.0;

/// OpenCADStudio's compact top-strip action, adapted to native point-cloud
/// commands and OpenAEC colors. Unavailable actions remain visible and muted.
pub fn quick_access_btn<'a>(
    icon: Element<'a, Message>,
    label: &'static str,
    message: Option<Message>,
) -> Element<'a, Message> {
    let control = button(container(icon).width(Fill).height(Fill).center(Fill))
        .on_press_maybe(message)
        .style(ui_style::tool)
        .width(Length::Fixed(QUICK_ACCESS_W))
        .height(23)
        .padding([1, 0]);
    // Below the tools of the ribbon, which it would cover.
    ui_style::ribbon_tooltip(control, crate::i18n::tr(label))
        .gap(90)
        .into()
}

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
    let col = column(std::mem::take(buf)).spacing(0);
    out.push(col.into());
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
    let tools = row(items_row).spacing(1).height(Fill).width(Length::Shrink);
    // The title counts for the width of its group, so that a long one
    // widens the group instead of being cut off; the line above it spans
    // the group.
    let title = text(crate::i18n::tr(title))
        .size(10)
        .font(crate::fonts::MEDIUM)
        .wrapping(iced::widget::text::Wrapping::None)
        .style(|theme| text::Style {
            color: Some(ui_theme::colors(theme).ribbon_group_label),
        });
    let content = column![
        container(tools)
            .height(3.0 * ROW_H)
            .align_y(iced::Alignment::Start),
        container(text("")).width(Fill).height(1).style(|theme| {
            container::Style::default().background(ui_theme::colors(theme).ribbon_group_separator)
        }),
        title,
    ]
    .align_x(iced::Alignment::Center)
    .spacing(0)
    .padding([2u16, 1])
    .width(Length::Shrink)
    .height(TOOL_BAR_H);

    row![
        content,
        container(text(""))
            .width(1)
            .height(TOOL_BAR_H - 10.0)
            .style(|theme| {
                container::Style::default()
                    .background(ui_theme::colors(theme).ribbon_group_separator)
            }),
    ]
    .spacing(1)
    .width(Length::Shrink)
    .into()
}

/// A tab of the ribbon: the open one joins the tool bar below it.
pub fn tab_style(theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        background: Some(Background::Color(if active {
            colors.bg
        } else if hovered {
            colors.ribbon_btn_hover
        } else {
            colors.bg_lighter
        })),
        text_color: if active || hovered {
            colors.accent
        } else {
            colors.text
        },
        border: Border {
            radius: iced::border::Radius::default().top(4),
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// OpenAEC's File entry stays at the start of the native ribbon and opens the
/// backstage workspace. Its solid accent is the style book's File-tab exception.
pub fn file_tab_style(theme: &Theme, _open: bool, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    button::Style {
        background: Some(Background::Color(
            if matches!(status, button::Status::Hovered) {
                colors.file_tab_hover
            } else {
                colors.file_tab_bg
            },
        )),
        text_color: colors.file_tab_text,
        border: Border {
            radius: iced::border::Radius::default().top(4),
            ..Border::default()
        },
        ..button::Style::default()
    }
}
