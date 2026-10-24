//! Library 0.4: the UI 0.4 design tokens and the components built on them.
//!
//! [`tokens`] is generated from `design/ontwerp-0.4/tokens-0.4.json`; [`widgets`]
//! draws the decided components (`design/ontwerp-0.4/04-componenten.md`), [`tiles`]
//! the tiles and cards (`03-tegels.md`), [`workflow`] the navigation of
//! workflow apps and [`chrome`] the title bar, splitter and menus (`05-layouts.md`),
//! all with those tokens only. Every component takes a [`Look`]: the theme plus the
//! user's Werkruimte and Statusbalk settings. This module is independent of the
//! prototype theme in [`crate::theme`].
pub mod chrome;
pub mod tiles;
pub mod tokens;
pub mod widgets;
pub mod workflow;

pub use tokens::*;

use iced::Color;

/// What a component needs to draw itself: the theme and the display settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub theme: ThemeId,
    pub tokens: &'static ThemeTokens,
    /// The theme palette after the Werkruimte and Statusbalk settings.
    pub color: Palette,
    /// Accent text that keeps its contrast on the current workspace.
    pub workspace_accent_text: Color,
}

impl Look {
    pub fn new(theme: ThemeId, workspace: WorkspaceSetting, statusbar: StatusbarSetting) -> Self {
        let tokens = theme.tokens();
        Self {
            theme,
            tokens,
            color: tokens.palette(workspace, statusbar),
            workspace_accent_text: tokens.workspace_accent_text(workspace),
        }
    }

    pub fn high_contrast(&self) -> bool {
        self.tokens.high_contrast
    }
}

impl From<ThemeId> for Look {
    fn from(theme: ThemeId) -> Self {
        Self::new(theme, WorkspaceSetting::Theme, StatusbarSetting::Theme)
    }
}

/// `top` laid over `bottom` with the given opacity, as the review pages compute tints.
pub fn mix(top: Color, amount: f32, bottom: Color) -> Color {
    let m = |a: f32, b: f32| a * amount + b * (1.0 - amount);
    Color::from_rgb(m(top.r, bottom.r), m(top.g, bottom.g), m(top.b, bottom.b))
}

/// `#RRGGBB` for an opaque colour, for recolouring SVG sources.
pub fn hex(color: Color) -> String {
    let [r, g, b, _] = color.into_rgba8();
    format!("#{r:02X}{g:02X}{b:02X}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_reach_the_look() {
        let look = Look::new(ThemeId::Light, WorkspaceSetting::Dark, StatusbarSetting::Dark);
        assert_eq!(hex(look.color.workspace), "#26262D");
        assert_eq!(hex(look.workspace_accent_text), hex(DARK.color.accent_text));
        assert_eq!(hex(look.color.statusbar), "#2A2A32");
        let hc = Look::new(ThemeId::HighContrastDark, WorkspaceSetting::Light, StatusbarSetting::Dark);
        assert_eq!(hc.color, HIGH_CONTRAST_DARK.color);
    }

    #[test]
    fn mix_reproduces_the_light_status_tints() {
        // In Licht a status tint is the status colour at 10% over the panel.
        assert_eq!(hex(mix(LIGHT.color.error, 0.10, LIGHT.color.panel)), hex(LIGHT.color.error_tint));
        assert_eq!(hex(mix(LIGHT.color.info, 0.10, LIGHT.color.panel)), hex(LIGHT.color.info_tint));
    }
}
