// Adapted from OpenCADStudio/src/ui/ribbon/{mod.rs,widgets.rs,draw_panel.rs}
// at commit 1fec34d84a129a2e4c6466e184893f78ae596777.
// Copyright OpenCADStudio contributors. Licensed under GPL-3.0.
// Changes: adapted the ribbon primitives to iced 0.13 and point-cloud commands.

use iced::Element;

use crate::Message;

mod adaptive;
pub use adaptive::{AdaptivePanel, AdaptiveRibbon};

pub const TOOL_BAR_H: f32 = openaec_ui::model::metrics::RIBBON as f32;
/// Compact quick access actions on the dark title strip.
pub fn quick_access_btn<'a>(
    icon: Element<'a, Message>,
    label: &'static str,
    message: Option<Message>,
    theme: openaec_ui::Theme,
) -> Element<'a, Message> {
    openaec_ui::widgets::title_action(icon, crate::i18n::tr(label), message, theme)
}

/// The wider, single-row groups of the approved desktop reference. The
/// adaptive ribbon still measures and collapses each complete group.
pub fn render_reference_group<'a>(
    theme: openaec_ui::Theme,
    title: &'static str,
    width: f32,
    tools: Element<'a, Message>,
) -> Element<'a, Message> {
    openaec_ui::widgets::ribbon_group(theme, crate::i18n::tr(title), tools, width)
}
