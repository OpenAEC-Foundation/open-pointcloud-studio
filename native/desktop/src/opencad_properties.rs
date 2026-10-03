// Adapted from OpenCADStudio/src/ui/properties.rs, commit 1fec34d.
// Copyright OpenCADStudio contributors. GPL-3.0.
// Changes: read-only point-cloud fields, a row that holds a control and the
// iced 0.13 palette API.

use iced::widget::{container, row, text, text_input};
use iced::{Background, Border, Element, Fill, Length, Theme};

use crate::ui_theme;
use crate::Message;

const ROW_H: f32 = 26.0;
const FONT_SZ: f32 = ROW_H * 0.42;

/// OpenCADStudio's two-column label/value row, adapted for read-only scan data.
pub fn property_row(label: &'static str, value: String) -> Element<'static, Message> {
    let label = crate::i18n::tr(label);
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

/// Keep survey bounds readable within OpenCADStudio's narrow properties dock.
pub fn bounds_row(axis: &'static str, min: f64, max: f64) -> Element<'static, Message> {
    let axis = container(text(axis).size(FONT_SZ).style(|theme| text::Style {
        color: Some(ui_theme::colors(theme).muted),
    }))
    .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel_alt))
    .width(Length::Fixed(30.0))
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 8]);
    let values = container(
        row![
            text(format!("{min:.3}"))
                .size(FONT_SZ)
                .width(Length::FillPortion(1)),
            text("→")
                .size(FONT_SZ)
                .style(|theme| text::Style {
                    color: Some(ui_theme::colors(theme).muted),
                })
                .width(Length::Fixed(22.0)),
            text(format!("{max:.3}"))
                .size(FONT_SZ)
                .width(Length::FillPortion(1)),
        ]
        .align_y(iced::Alignment::Center),
    )
    .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel))
    .width(Fill)
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    container(row![axis, values])
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

/// Editable variant of OpenCADStudio's two-column property row.
pub fn property_input<'a>(
    label: &'static str,
    placeholder: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    let label = crate::i18n::tr(label);
    let label_col = container(text(label).size(FONT_SZ).style(|theme| text::Style {
        color: Some(ui_theme::colors(theme).muted),
    }))
    .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel_alt))
    .width(Length::FillPortion(5))
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    let value_col = container(
        text_input(placeholder, value)
            .on_input(on_input)
            .size(FONT_SZ)
            .padding([2, 4]),
    )
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

/// The two-column row with a control of the caller's choice, such as a
/// choice list, in the place of the value.
pub fn property_control<'a>(
    label: &'static str,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    let label = crate::i18n::tr(label);
    let label_col = container(text(label).size(FONT_SZ).style(|theme| text::Style {
        color: Some(ui_theme::colors(theme).muted),
    }))
    .style(|theme| container::Style::default().background(ui_theme::colors(theme).panel_alt))
    .width(Length::FillPortion(5))
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    let value_col = container(control)
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
    container(text(crate::i18n::tr(title)).size(FONT_SZ))
        .width(Fill)
        .padding([4, 6])
        .style(|theme| container::Style {
            background: Some(Background::Color(ui_theme::colors(theme).panel_title)),
            ..Default::default()
        })
        .into()
}
