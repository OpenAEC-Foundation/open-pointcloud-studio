use crate::tokens::{self, Tokens};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Light,
    Forge,
    OpenAec,
    Blueprint,
    Contrast,
}
impl Theme {
    pub const ALL: [Self; 5] = [Self::Light, Self::Forge, Self::OpenAec, Self::Blueprint, Self::Contrast];
    /// Four swatches from the pinned Tauri SettingsDialog THEME_OPTIONS.
    pub fn swatches(self) -> [iced::Color; 4] {
        let hex = |value: u32| iced::Color::from_rgb8((value >> 16) as u8, (value >> 8) as u8, value as u8);
        match self {
            Self::Light => [hex(0xFAFAF9), hex(0xFFFFFF), hex(0xD97706), hex(0x36363E)],
            Self::Forge => [hex(0x36363E), hex(0x44444C), hex(0xD97706), hex(0xFAFAF9)],
            Self::OpenAec => [hex(0x27272A), hex(0x1C1917), hex(0xD97706), hex(0xFAFAF9)],
            Self::Blueprint => [hex(0x0F1B2D), hex(0x1A2C45), hex(0x60A5FA), hex(0xE0E7FF)],
            Self::Contrast => [hex(0x000000), hex(0x0A0A0A), hex(0xFFD700), hex(0xFFFFFF)],
        }
    }
    pub fn tokens(self) -> Tokens {
        match self { Self::Light => tokens::LIGHT, Self::Forge => tokens::FORGE, Self::OpenAec => tokens::OPENAEC, Self::Blueprint => tokens::BLUEPRINT, Self::Contrast => tokens::CONTRAST }
    }
    /// Brand strong-border token from the pinned Style Book colors.json (light/dark).
    pub fn border_strong(self) -> iced::Color {
        match self {
            Self::Light => iced::Color::from_rgb8(0xD6, 0xD3, 0xD1),
            _ => iced::Color::from_rgb8(0x3F, 0x3F, 0x46),
        }
    }
    /// Modal.css and themes.css at dfdcd41: 0 4px 16px with a theme-specific color.
    pub fn dialog_shadow(self) -> iced::Shadow {
        let color = match self {
            Self::Light => iced::Color::from_rgba8(0, 0, 0, 0.12),
            Self::Forge => iced::Color::from_rgba8(0, 0, 0, 0.35),
            Self::OpenAec => iced::Color::from_rgba8(0, 0, 0, 0.40),
            Self::Blueprint => iced::Color::from_rgba8(0, 0, 0, 0.45),
            Self::Contrast => iced::Color::from_rgba8(255, 215, 0, 0.40),
        };
        iced::Shadow { color, offset: iced::Vector::new(0.0, 4.0), blur_radius: 16.0 }
    }
    pub fn iced(self) -> iced::Theme {
        let t = self.tokens();
        iced::Theme::custom(self.to_string(), iced::theme::Palette {
            background: t.bg, text: t.text, primary: t.accent,
            success: iced::Color::from_rgb8(22, 163, 74), danger: t.danger_color,
        })
    }
}
impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self { Self::Light => "Light", Self::Forge => "Forge", Self::OpenAec => "OpenAEC", Self::Blueprint => "Blueprint", Self::Contrast => "High contrast" })
    }
}
