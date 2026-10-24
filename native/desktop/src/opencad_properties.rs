// Adapted from OpenCADStudio/src/ui/properties.rs, commit 1fec34d.
// Copyright OpenCADStudio contributors. GPL-3.0.
// Changes: read-only point-cloud fields, a row that holds a control and the
// iced 0.13 palette API.

use iced::widget::{container, row, text};
use iced::{Border, Element, Length};

use crate::ui_style;
use crate::ui_theme;
use crate::Message;

/// Shared property-grid row with Pointcloud's translated field label.
pub fn property_row(label: &'static str, value: String) -> Element<'static, Message> {
    openaec_ui::widgets::property_grid_row_with(
        crate::i18n::tr(label), value, ui_theme::shared_theme,
    )
}

/// Survey bounds use the same label column and 28 px rhythm as every property.
pub fn bounds_row(axis: &'static str, min: f64, max: f64) -> Element<'static, Message> {
    let values = row![
        text(format!("{min:.3}"))
            .size(12)
            .width(Length::FillPortion(1)),
        text("→")
            .size(12)
            .style(|theme| text::Style {
                color: Some(ui_theme::colors(theme).text_secondary),
            })
            .width(Length::Fixed(22.0)),
        text(format!("{max:.3}"))
            .size(12)
            .width(Length::FillPortion(1)),
    ]
    .align_y(iced::Alignment::Center);
    openaec_ui::widgets::property_grid_control_with(axis, values.into(), ui_theme::shared_theme)
}

/// Editable property field in the shared grid.
pub fn property_input<'a>(
    label: &'static str,
    placeholder: &'static str,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
    property_control(label, ui_style::text_input(placeholder, value).on_input(on_input).into())
}

/// The two-column row with a control of the caller's choice, such as a
/// choice list, in the place of the value.
pub fn property_control<'a>(
    label: &'static str,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    openaec_ui::widgets::property_grid_control_with(
        crate::i18n::tr(label), control, ui_theme::shared_theme,
    )
}

pub fn section_header(title: &'static str) -> Element<'static, Message> {
    openaec_ui::widgets::property_section_with(
        crate::i18n::tr(title), ui_theme::shared_theme,
    )
}

/// A control whose explanation shows in a tooltip above it instead of as
/// text on the panel.
pub fn explained<'a>(
    content: impl Into<Element<'a, Message>>,
    lines: Vec<String>,
) -> Element<'a, Message> {
    if lines.is_empty() {
        return content.into();
    }
    ui_style::tooltip(content, lines.join("\n\n")).into()
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
    Some(ui_style::tooltip(mark, lines.join("\n\n")).into())
}
