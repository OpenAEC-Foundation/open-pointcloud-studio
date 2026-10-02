// Adapted from OpenCADStudio/src/ui/ribbon/{mod.rs,widgets.rs,draw_panel.rs}
// at commit 1fec34d84a129a2e4c6466e184893f78ae596777.
// Copyright OpenCADStudio contributors. Licensed under GPL-3.0.
// Changes: adapted the ribbon primitives to iced 0.13 and point-cloud commands.

use iced::widget::{button, column, container, row, text, tooltip};
use iced::{Background, Border, Color, Element, Fill, Length, Theme};

use crate::ui_theme;
use crate::Message;

pub const ROW_H: f32 = 26.0;
pub const TOOL_BAR_H: f32 = 3.0 * ROW_H + 18.0;
pub const QUICK_ACCESS_W: f32 = 30.0;

/// OpenCADStudio's compact top-strip action, adapted to native point-cloud
/// commands and OpenAEC colors. Unavailable actions remain visible and muted.
pub fn quick_access_btn<'a>(
    icon: Element<'a, Message>,
    label: &'static str,
    message: Option<Message>,
) -> Element<'a, Message> {
    let control = button(container(icon).width(Fill).height(Fill).center(Fill))
        .on_press_maybe(message)
        .style(|theme, status| tool_btn_style(theme, false, status))
        .width(Length::Fixed(QUICK_ACCESS_W))
        .height(26)
        .padding([2, 0]);
    tooltip(
        control,
        container(text(label).size(11))
            .padding([4, 7])
            .style(|theme| {
                let colors = ui_theme::colors(theme);
                container::Style::default()
                    .background(colors.panel_alt)
                    .color(colors.text)
            }),
        tooltip::Position::Bottom,
    )
    .gap(98)
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
    let col = column(std::mem::take(buf)).spacing(1);
    out.push(col.into());
}

/// OpenCADStudio button behavior with OpenAEC dark ribbon tokens.
pub fn tool_btn_style(theme: &Theme, is_active: bool, status: button::Status) -> button::Style {
    let colors = ui_theme::colors(theme);
    let (background, text_color, border) = match (is_active, status) {
        (_, button::Status::Disabled) => (None, colors.muted, Color::TRANSPARENT),
        (true, _) => (
            Some(colors.ribbon_active_bg),
            colors.ribbon_active_text,
            colors.ribbon_active_border,
        ),
        (_, button::Status::Hovered | button::Status::Pressed) => (
            Some(colors.ribbon_hover),
            colors.text,
            colors.ribbon_hover_border,
        ),
        _ => (None, colors.text, Color::TRANSPARENT),
    };
    button::Style {
        background: background.map(Background::Color),
        text_color,
        border: Border {
            radius: 3.0.into(),
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
                container(text("")).width(Fill).height(1).style(|theme| {
                    container::Style::default()
                        .background(ui_theme::colors(theme).ribbon_group_separator)
                }),
                text(title).size(9).style(|theme| text::Style {
                    color: Some(ui_theme::colors(theme).ribbon_group_label),
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
            .style(|theme| {
                container::Style::default()
                    .background(ui_theme::colors(theme).ribbon_group_separator)
            }),
    ]
    .spacing(2)
    .width(Length::Shrink)
    .into()
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
                colors.accent
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
