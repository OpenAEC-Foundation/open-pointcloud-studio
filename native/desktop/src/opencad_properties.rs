// Adapted from OpenCADStudio/src/ui/properties.rs, commit 1fec34d.
// Copyright OpenCADStudio contributors. GPL-3.0.
// Changes: read-only point-cloud fields and iced 0.13 palette API.

use iced::widget::{container, row, text};
use iced::{Background, Border, Element, Fill, Length, Theme};

use crate::ui_theme;
use crate::Message;

const ROW_H: f32 = 26.0;
const FONT_SZ: f32 = ROW_H * 0.42;

/// OpenCADStudio's two-column label/value row, adapted for read-only scan data.
pub fn property_row(label: &'static str, value: String) -> Element<'static, Message> {
    let label_col = container(text(label).size(FONT_SZ).style(|theme| text::Style {
        color: Some(ui_theme::colors(theme).muted),
    }))
    .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel_alt))
    .width(Length::FillPortion(5))
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    let value_col = container(text(value).size(FONT_SZ))
        .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel))
        .width(Length::FillPortion(6))
        .height(ROW_H)
        .align_y(iced::Alignment::Center)
        .padding([0, 5]);
    container(row![label_col, value_col])
        .height(ROW_H)
        .width(Fill)
        .style(|theme: &Theme| container::Style {
            border: Border {
                color: ui_theme::colors(theme).border,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

pub fn section_header(title: &'static str) -> Element<'static, Message> {
    container(text(title).size(FONT_SZ))
        .width(Fill)
        .padding([4, 6])
        .style(|theme| container::Style {
            background: Some(Background::Color(ui_theme::colors(theme).panel_title)),
            ..Default::default()
        })
        .into()
}
