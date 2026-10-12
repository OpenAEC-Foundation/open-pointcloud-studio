// Adapted from OpenCADStudio/src/ui/ribbon/{mod.rs,widgets.rs,draw_panel.rs}
// at commit 1fec34d84a129a2e4c6466e184893f78ae596777.
// Copyright OpenCADStudio contributors. Licensed under GPL-3.0.
// Changes: adapted the ribbon primitives to iced 0.13 and point-cloud commands.

use iced::widget::{button, column, container, row, text};
use iced::{Background, Color, Element, Fill, Length, Theme};

use crate::{ui_style, ui_theme, Message};

mod adaptive;
pub use adaptive::{AdaptivePanel, AdaptiveRibbon};

pub const TOOL_BAR_H: f32 = 114.0;
pub const QUICK_ACCESS_W: f32 = 40.0;

/// Compact quick access actions on the dark title strip.
pub fn quick_access_btn<'a>(
    icon: Element<'a, Message>,
    label: &'static str,
    message: Option<Message>,
) -> Element<'a, Message> {
    let control = button(container(icon).width(Fill).height(Fill).center(Fill))
        .on_press_maybe(message)
        .style(title_action_style)
        .width(Length::Fixed(QUICK_ACCESS_W))
        .height(32)
        .padding([1, 0]);
    // Below the tools of the ribbon, which it would cover.
    ui_style::ribbon_tooltip(control, crate::i18n::tr(label))
        .gap(90)
        .into()
}

/// The wider, single-row groups of the approved desktop reference. The
/// adaptive ribbon still measures and collapses each complete group.
pub fn render_reference_group<'a>(
    title: &'static str,
    width: f32,
    tools: Element<'a, Message>,
) -> Element<'a, Message> {
    let caption = text(crate::i18n::tr(title))
        .size(12)
        .font(crate::fonts::MEDIUM)
        .style(|theme| text::Style {
            color: Some(ui_theme::colors(theme).ribbon_group_label),
        });
    let content = column![
        container(tools)
            .width(Fill)
            .height(77)
            .center_x(Fill)
            .align_y(iced::Alignment::Center),
        container(caption).width(Fill).center_x(Fill),
    ]
    .spacing(4)
    .padding([7, 0])
    .width(Length::Fixed(width - 1.0))
    .height(TOOL_BAR_H);
    row![
        content,
        container(text(""))
            .width(1)
            .height(96)
            .style(|theme| container::Style::default()
                .background(ui_theme::colors(theme).ribbon_group_separator)),
    ]
    .align_y(iced::Alignment::Center)
    .width(Length::Fixed(width))
    .into()
}

fn title_action_style(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        background: hovered.then_some(Background::Color(Color::from_rgb8(78, 78, 86))),
        text_color: Color::from_rgb8(250, 250, 249),
        ..button::Style::default()
    }
}

/// Dark title tabs with a separate amber underline on the active tab.
pub fn tab_style(_theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        background: hovered.then_some(Background::Color(Color::from_rgb8(78, 78, 86))),
        text_color: if active || hovered {
            Color::from_rgb8(250, 250, 249)
        } else {
            Color::from_rgb8(188, 188, 194)
        },
        ..button::Style::default()
    }
}
