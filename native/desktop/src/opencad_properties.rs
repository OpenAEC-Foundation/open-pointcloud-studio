// Adapted from OpenCADStudio/src/ui/properties.rs, commit 1fec34d.
// Copyright OpenCADStudio contributors. GPL-3.0.
// Changes: read-only point-cloud fields, a row that holds a control and the
// iced 0.13 palette API.

use iced::widget::{container, row, text, text_input, tooltip};
use iced::{Background, Border, Element, Fill, Length, Theme};

use crate::ui_theme;
use crate::Message;

const ROW_H: f32 = 26.0;
const FONT_SZ: f32 = ROW_H * 0.42;

/// OpenCADStudio's two-column label/value row, adapted for read-only scan data.
pub fn property_row(label: &'static str, value: String) -> Element<'static, Message> {
    let label = crate::i18n::tr(label);
    let label_col = container(text(label).size(FONT_SZ).style(|theme| text::Style {
        color: Some(ui_theme::colors(theme).text_secondary),
    }))
    .width(Length::FillPortion(5))
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    let value_col = container(text(value).size(FONT_SZ))
        .width(Length::FillPortion(6))
        .height(ROW_H)
        .align_y(iced::Alignment::Center)
        .padding([0, 5]);
    container(row![label_col, value_col])
        .height(ROW_H)
        .width(Fill)
        .style(|theme: &Theme| container::Style {
            border: Border {
                color: ui_theme::colors(theme).border_subtle,
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
        color: Some(ui_theme::colors(theme).text_secondary),
    }))
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
                    color: Some(ui_theme::colors(theme).text_secondary),
                })
                .width(Length::Fixed(22.0)),
            text(format!("{max:.3}"))
                .size(FONT_SZ)
                .width(Length::FillPortion(1)),
        ]
        .align_y(iced::Alignment::Center),
    )
    .width(Fill)
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    container(row![axis, values])
        .height(ROW_H)
        .width(Fill)
        .style(|theme: &Theme| container::Style {
            border: Border {
                color: ui_theme::colors(theme).border_subtle,
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
        color: Some(ui_theme::colors(theme).text_secondary),
    }))
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
    .width(Length::FillPortion(6))
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 5]);
    container(row![label_col, value_col])
        .height(ROW_H)
        .width(Fill)
        .style(|theme: &Theme| container::Style {
            border: Border {
                color: ui_theme::colors(theme).border_subtle,
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
        color: Some(ui_theme::colors(theme).text_secondary),
    }))
    .width(Length::FillPortion(5))
    .height(ROW_H)
    .align_y(iced::Alignment::Center)
    .padding([0, 6]);
    let value_col = container(control)
        .width(Length::FillPortion(6))
        .height(ROW_H)
        .align_y(iced::Alignment::Center)
        .padding([0, 5]);
    container(row![label_col, value_col])
        .height(ROW_H)
        .width(Fill)
        .style(|theme: &Theme| container::Style {
            border: Border {
                color: ui_theme::colors(theme).border_subtle,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

pub fn section_header(title: &'static str) -> Element<'static, Message> {
    container(
        text(crate::i18n::tr(title))
            .size(11)
            .font(crate::fonts::SEMIBOLD),
    )
    .width(Fill)
    .padding([4, 6])
    .style(|theme| container::Style {
        background: Some(Background::Color(ui_theme::colors(theme).bg_lighter)),
        ..Default::default()
    })
    .into()
}

/// The box a tooltip of the panels is shown in.
fn tip_box<'a>(lines: &[String]) -> Element<'a, Message> {
    container(text(lines.join("\n\n")).size(11))
        .padding([6, 8])
        .max_width(340)
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.tooltip_bg)
                .color(colors.tooltip_text)
                .border(Border::default().rounded(4))
        })
        .into()
}

/// A control whose explanation shows in a tooltip instead of as text on the
/// panel.
pub fn explained<'a>(
    content: impl Into<Element<'a, Message>>,
    lines: Vec<String>,
) -> Element<'a, Message> {
    if lines.is_empty() {
        return content.into();
    }
    tooltip(content, tip_box(&lines), tooltip::Position::Bottom)
        .gap(4)
        .into()
}

/// A small mark in the accent colour whose tooltip holds warnings or
/// advice; nothing when there are none.
pub fn warning_mark<'a>(lines: Vec<String>) -> Option<Element<'a, Message>> {
    if lines.is_empty() {
        return None;
    }
    let mark = container(text("!").size(12).style(|theme| text::Style {
        color: Some(ui_theme::colors(theme).accent),
    }))
    .padding([2, 7])
    .style(|theme| {
        let colors = ui_theme::colors(theme);
        container::Style::default().border(Border {
            color: colors.accent,
            width: 1.0,
            radius: 9.0.into(),
        })
    });
    Some(
        tooltip(mark, tip_box(&lines), tooltip::Position::Bottom)
            .gap(4)
            .into(),
    )
}
