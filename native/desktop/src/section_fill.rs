//! The fill of the cut where the section box cuts a mesh: the material
//! between the two faces of a wall, floor or ceiling is closed with a cap of
//! one colour, as a section drawing fills its cut.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use iced::widget::{checkbox, column, container, row, text_input};
use iced::{Background, Border, Color, Element, Fill};
use pointcloud_core::{
    DEFAULT_CAP_MAX_THICKNESS, LAYER_RGB_CUT_FILL, MAX_CAP_MAX_THICKNESS, MIN_CAP_MAX_THICKNESS,
};
use serde_json::{json, Value};

use crate::i18n::tr;
use crate::{opencad_properties, Message};

/// The grey of the filled cut in a section drawing.
pub(crate) const DEFAULT_CAP_COLOR: [u8; 3] = LAYER_RGB_CUT_FILL;

/// How the caps are drawn, while they are switched on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CapStyle {
    pub color: [u8; 3],
    /// Two opposite faces farther apart than this are not filled.
    pub max_thickness: f64,
}

#[derive(Debug, Clone)]
pub(crate) struct SectionFill {
    pub fill_cut: bool,
    pub color: [u8; 3],
    pub max_thickness: f64,
    color_input: String,
    thickness_input: String,
    /// Set by the 3D view while it makes the caps of the scene.
    pub pending: Arc<AtomicBool>,
}

/// What the block in Properties changes.
#[derive(Debug, Clone)]
pub enum FillAction {
    Enabled(bool),
    Color(String),
    MaxThickness(String),
}

impl Default for SectionFill {
    fn default() -> Self {
        Self::new(true, DEFAULT_CAP_COLOR, DEFAULT_CAP_MAX_THICKNESS)
    }
}

pub(crate) fn valid_thickness(value: f64) -> bool {
    value.is_finite() && (MIN_CAP_MAX_THICKNESS..=MAX_CAP_MAX_THICKNESS).contains(&value)
}

/// `#rrggbb`, with or without the hash.
pub(crate) fn parse_color(value: &str) -> Option<[u8; 3]> {
    let digits = value.trim().trim_start_matches('#');
    if digits.len() != 6 || !digits.is_ascii() {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

pub(crate) fn format_color(color: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
}

fn format_thickness(value: f64) -> String {
    format!("{value:.2}")
}

impl SectionFill {
    pub fn new(fill_cut: bool, color: [u8; 3], max_thickness: f64) -> Self {
        let max_thickness = if valid_thickness(max_thickness) {
            max_thickness
        } else {
            DEFAULT_CAP_MAX_THICKNESS
        };
        Self {
            fill_cut,
            color,
            max_thickness,
            color_input: format_color(color),
            thickness_input: format_thickness(max_thickness),
            pending: Arc::default(),
        }
    }

    /// How the caps are drawn, or `None` when the cut is not filled.
    pub fn style(&self) -> Option<CapStyle> {
        self.fill_cut.then_some(CapStyle {
            color: self.color,
            max_thickness: self.max_thickness,
        })
    }

    /// Take a change from Properties. A text that is not yet a colour or a
    /// thickness is kept in its field and changes nothing else. Answers
    /// whether a kept setting changed.
    pub fn apply(&mut self, action: FillAction) -> bool {
        match action {
            FillAction::Enabled(enabled) => {
                let changed = self.fill_cut != enabled;
                self.fill_cut = enabled;
                changed
            }
            FillAction::Color(value) => {
                let parsed = parse_color(&value);
                self.color_input = value;
                match parsed {
                    Some(color) if color != self.color => {
                        self.color = color;
                        true
                    }
                    _ => false,
                }
            }
            FillAction::MaxThickness(value) => {
                let parsed = value
                    .trim()
                    .replace(',', ".")
                    .parse::<f64>()
                    .ok()
                    .filter(|value| valid_thickness(*value));
                self.thickness_input = value;
                match parsed {
                    Some(thickness) if thickness != self.max_thickness => {
                        self.max_thickness = thickness;
                        true
                    }
                    _ => false,
                }
            }
        }
    }

    /// Set what a command asks; nothing changes when any part is invalid.
    pub fn set(
        &mut self,
        fill_cut: Option<bool>,
        color: Option<&str>,
        max_thickness: Option<f64>,
    ) -> Result<(), String> {
        let color = match color {
            Some(value) => Some(
                parse_color(value)
                    .ok_or_else(|| format!("color must be #rrggbb, not {value:?}"))?,
            ),
            None => None,
        };
        if let Some(value) = max_thickness.filter(|value| !valid_thickness(*value)) {
            return Err(format!(
                "max_thickness must be between {MIN_CAP_MAX_THICKNESS} and {MAX_CAP_MAX_THICKNESS} metres, not {value}"
            ));
        }
        if let Some(enabled) = fill_cut {
            self.fill_cut = enabled;
        }
        if let Some(color) = color {
            self.color = color;
            self.color_input = format_color(color);
        }
        if let Some(thickness) = max_thickness {
            self.max_thickness = thickness;
            self.thickness_input = format_thickness(thickness);
        }
        Ok(())
    }

    pub fn value(&self) -> Value {
        json!({
            "fill_cut": self.fill_cut,
            "color": format_color(self.color),
            "max_thickness": self.max_thickness,
            "pending": self.pending.load(Ordering::Relaxed),
        })
    }

    /// The rows of the Section box block in Properties.
    pub fn properties(&self) -> Element<'_, Message> {
        let mut block = column![container(
            checkbox(tr("Fill the cut"), self.fill_cut)
                .on_toggle(|enabled| Message::SectionFill(FillAction::Enabled(enabled)))
                .style(crate::muted_checkbox_style)
                .text_size(11)
                .size(13),
        )
        .padding([4, 8])]
        .spacing(0);
        if self.fill_cut {
            let [r, g, b] = self.color;
            let swatch = container("")
                .width(16)
                .height(14)
                .style(move |_| container::Style {
                    background: Some(Background::Color(Color::from_rgb8(r, g, b))),
                    border: Border {
                        color: Color::from_rgb8(96, 96, 96),
                        width: 1.0,
                        radius: 2.0.into(),
                    },
                    ..Default::default()
                });
            block = block
                .push(opencad_properties::property_control(
                    "Cap colour",
                    row![
                        swatch,
                        text_input("#808080", &self.color_input)
                            .on_input(|value| Message::SectionFill(FillAction::Color(value)))
                            .size(11)
                            .padding([2, 4])
                            .width(Fill),
                    ]
                    .spacing(5)
                    .align_y(iced::Alignment::Center)
                    .into(),
                ))
                .push(opencad_properties::property_input(
                    "Largest thickness (m)",
                    "0.50",
                    &self.thickness_input,
                    |value| Message::SectionFill(FillAction::MaxThickness(value)),
                ));
        }
        block.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_read_and_write_as_hex() {
        assert_eq!(parse_color("#808080"), Some([128, 128, 128]));
        assert_eq!(parse_color(" a03c28 "), Some([160, 60, 40]));
        assert_eq!(parse_color("#80808"), None);
        assert_eq!(parse_color("#80808g"), None);
        assert_eq!(parse_color("#éé8080"), None);
        assert_eq!(format_color([160, 60, 40]), "#a03c28");
    }

    #[test]
    fn the_fill_is_on_in_drawing_grey_and_half_a_metre() {
        let fill = SectionFill::default();
        assert_eq!(
            fill.style(),
            Some(CapStyle {
                color: [128, 128, 128],
                max_thickness: 0.5,
            })
        );
        assert_eq!(fill.value()["color"], "#808080");
        let off = SectionFill::new(false, [1, 2, 3], 99.0);
        assert_eq!(off.style(), None);
        assert_eq!(off.max_thickness, DEFAULT_CAP_MAX_THICKNESS);
    }

    #[test]
    fn edits_take_effect_once_they_are_valid() {
        let mut fill = SectionFill::default();
        assert!(!fill.apply(FillAction::Color("#40".into())));
        assert_eq!(fill.color, DEFAULT_CAP_COLOR);
        assert_eq!(fill.color_input, "#40");
        assert!(fill.apply(FillAction::Color("#404040".into())));
        assert_eq!(fill.color, [64, 64, 64]);
        assert!(!fill.apply(FillAction::MaxThickness("3".into())));
        assert!(fill.apply(FillAction::MaxThickness("0,3".into())));
        assert_eq!(fill.max_thickness, 0.3);
        assert!(fill.apply(FillAction::Enabled(false)));
        assert_eq!(fill.style(), None);
    }

    #[test]
    fn a_command_changes_nothing_when_a_part_is_invalid() {
        let mut fill = SectionFill::default();
        assert!(fill.set(Some(false), Some("red"), None).is_err());
        assert!(fill.set(Some(false), None, Some(0.0)).is_err());
        assert!(fill.fill_cut);
        fill.set(Some(false), Some("#102030"), Some(1.2)).unwrap();
        assert_eq!(
            fill.value(),
            json!({"fill_cut": false, "color": "#102030", "max_thickness": 1.2, "pending": false})
        );
        assert_eq!(fill.thickness_input, "1.20");
    }
}
