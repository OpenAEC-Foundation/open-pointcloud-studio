//! Native OpenAEC palettes. Values follow the local OpenAEC style book's
//! `project-templates/Tauri+React/src/themes.css` at commit dfdcd41.

use std::fmt;
use std::path::PathBuf;

use iced::{Color, Theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiTheme {
    Forge,
    Light,
    Night,
    Blueprint,
    Contrast,
}

impl UiTheme {
    pub const ALL: [Self; 5] = [
        Self::Forge,
        Self::Light,
        Self::Night,
        Self::Blueprint,
        Self::Contrast,
    ];

    pub fn iced(self) -> Theme {
        let colors = self.colors();
        Theme::custom(
            self.to_string(),
            iced::theme::Palette {
                background: colors.shell,
                text: colors.text,
                primary: colors.accent,
                success: Color::from_rgb8(74, 222, 128),
                danger: Color::from_rgb8(248, 113, 113),
            },
        )
    }

    pub fn colors(self) -> UiColors {
        match self {
            Self::Forge => UiColors {
                shell: Color::from_rgb8(54, 54, 62),
                tabs: Color::from_rgb8(68, 68, 76),
                panel: Color::from_rgb8(42, 42, 50),
                panel_alt: Color::from_rgb8(54, 54, 62),
                panel_title: Color::from_rgb8(63, 63, 70),
                border: Color::from_rgb8(63, 63, 70),
                text: Color::from_rgb8(250, 250, 249),
                muted: Color::from_rgb8(161, 161, 170),
                accent: Color::from_rgb8(217, 119, 6),
                ribbon_active_text: Color::from_rgb8(217, 119, 6),
                ribbon_group_label: Color::from_rgba8(217, 119, 6, 0.8),
                ribbon_group_separator: Color::from_rgba8(250, 250, 249, 0.15),
                ribbon_hover: Color::from_rgba8(217, 119, 6, 0.15),
                ribbon_hover_border: Color::from_rgba8(217, 119, 6, 0.30),
                ribbon_active_bg: Color::from_rgba8(217, 119, 6, 0.20),
                ribbon_active_border: Color::from_rgba8(217, 119, 6, 0.40),
                file_tab_text: Color::from_rgb8(54, 54, 62),
                file_tab_hover: Color::from_rgb8(234, 88, 12),
                hover: Color::from_rgba8(161, 161, 170, 0.12),
            },
            Self::Light => UiColors {
                shell: Color::from_rgb8(250, 250, 249),
                tabs: Color::WHITE,
                panel: Color::WHITE,
                panel_alt: Color::from_rgb8(245, 245, 244),
                panel_title: Color::from_rgb8(231, 229, 228),
                border: Color::from_rgb8(214, 211, 209),
                text: Color::from_rgb8(54, 54, 62),
                muted: Color::from_rgb8(87, 83, 78),
                accent: Color::from_rgb8(217, 119, 6),
                ribbon_active_text: Color::from_rgb8(217, 119, 6),
                ribbon_group_label: Color::from_rgba8(217, 119, 6, 0.8),
                ribbon_group_separator: Color::from_rgba8(54, 54, 62, 0.12),
                ribbon_hover: Color::from_rgba8(217, 119, 6, 0.10),
                ribbon_hover_border: Color::from_rgba8(217, 119, 6, 0.25),
                ribbon_active_bg: Color::from_rgba8(217, 119, 6, 0.15),
                ribbon_active_border: Color::from_rgba8(217, 119, 6, 0.35),
                file_tab_text: Color::WHITE,
                file_tab_hover: Color::from_rgb8(234, 88, 12),
                hover: Color::from_rgba8(54, 54, 62, 0.06),
            },
            Self::Night => UiColors {
                shell: Color::from_rgb8(39, 39, 42),
                tabs: Color::from_rgb8(54, 54, 62),
                panel: Color::from_rgb8(28, 25, 23),
                panel_alt: Color::from_rgb8(39, 39, 42),
                panel_title: Color::from_rgb8(63, 63, 70),
                border: Color::from_rgb8(63, 63, 70),
                text: Color::from_rgb8(250, 250, 249),
                muted: Color::from_rgb8(161, 161, 170),
                accent: Color::from_rgb8(217, 119, 6),
                ribbon_active_text: Color::from_rgb8(217, 119, 6),
                ribbon_group_label: Color::from_rgba8(217, 119, 6, 0.8),
                ribbon_group_separator: Color::from_rgba8(217, 119, 6, 0.25),
                ribbon_hover: Color::from_rgba8(217, 119, 6, 0.15),
                ribbon_hover_border: Color::from_rgba8(217, 119, 6, 0.30),
                ribbon_active_bg: Color::from_rgba8(217, 119, 6, 0.20),
                ribbon_active_border: Color::from_rgba8(217, 119, 6, 0.40),
                file_tab_text: Color::from_rgb8(39, 39, 42),
                file_tab_hover: Color::from_rgb8(234, 88, 12),
                hover: Color::from_rgba8(161, 161, 170, 0.12),
            },
            Self::Blueprint => UiColors {
                shell: Color::from_rgb8(15, 27, 45),
                tabs: Color::from_rgb8(26, 44, 69),
                panel: Color::from_rgb8(10, 19, 32),
                panel_alt: Color::from_rgb8(26, 44, 69),
                panel_title: Color::from_rgb8(37, 58, 82),
                border: Color::from_rgb8(61, 90, 128),
                text: Color::from_rgb8(224, 231, 255),
                muted: Color::from_rgb8(152, 193, 217),
                accent: Color::from_rgb8(96, 165, 250),
                ribbon_active_text: Color::from_rgb8(147, 197, 253),
                ribbon_group_label: Color::from_rgba8(96, 165, 250, 0.85),
                ribbon_group_separator: Color::from_rgba8(96, 165, 250, 0.18),
                ribbon_hover: Color::from_rgba8(96, 165, 250, 0.12),
                ribbon_hover_border: Color::from_rgba8(96, 165, 250, 0.30),
                ribbon_active_bg: Color::from_rgba8(96, 165, 250, 0.18),
                ribbon_active_border: Color::from_rgba8(96, 165, 250, 0.40),
                file_tab_text: Color::from_rgb8(15, 27, 45),
                file_tab_hover: Color::from_rgb8(147, 197, 253),
                hover: Color::from_rgba8(152, 193, 217, 0.12),
            },
            Self::Contrast => UiColors {
                shell: Color::BLACK,
                tabs: Color::from_rgb8(10, 10, 10),
                panel: Color::BLACK,
                panel_alt: Color::from_rgb8(10, 10, 10),
                panel_title: Color::from_rgb8(25, 25, 25),
                border: Color::from_rgb8(255, 215, 0),
                text: Color::WHITE,
                muted: Color::from_rgb8(229, 229, 229),
                accent: Color::from_rgb8(255, 215, 0),
                ribbon_active_text: Color::from_rgb8(255, 215, 0),
                ribbon_group_label: Color::from_rgb8(255, 215, 0),
                ribbon_group_separator: Color::WHITE,
                ribbon_hover: Color::from_rgba8(255, 215, 0, 0.25),
                ribbon_hover_border: Color::from_rgb8(255, 215, 0),
                ribbon_active_bg: Color::from_rgba8(255, 215, 0, 0.40),
                ribbon_active_border: Color::from_rgb8(255, 215, 0),
                file_tab_text: Color::BLACK,
                file_tab_hover: Color::from_rgb8(255, 255, 0),
                hover: Color::from_rgba8(255, 215, 0, 0.25),
            },
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Forge => "forge",
            Self::Light => "light",
            Self::Night => "openaec",
            Self::Blueprint => "blueprint",
            Self::Contrast => "contrast",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        if value == "night" {
            return Some(Self::Night);
        }
        Self::ALL.into_iter().find(|theme| theme.key() == value)
    }

    pub fn load() -> Self {
        theme_path()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|value| Self::from_key(value.trim()))
            .unwrap_or(Self::Light)
    }

    pub fn save(self) {
        if let Some(path) = theme_path() {
            if let Some(directory) = path.parent() {
                if std::fs::create_dir_all(directory).is_ok() {
                    let _ = std::fs::write(path, self.key());
                }
            }
        }
    }
}

impl fmt::Display for UiTheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The names are translated where Settings lists the themes.
        let label = match self {
            Self::Forge => crate::i18n::key("Deep Forge"),
            Self::Light => crate::i18n::key("Blueprint Light"),
            Self::Night => crate::i18n::key("Night Build"),
            Self::Blueprint => crate::i18n::key("Blueprint Blue"),
            Self::Contrast => crate::i18n::key("High Contrast"),
        };
        f.write_str(label)
    }
}

#[derive(Clone, Copy)]
pub struct UiColors {
    pub shell: Color,
    pub tabs: Color,
    pub panel: Color,
    pub panel_alt: Color,
    pub panel_title: Color,
    pub border: Color,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub ribbon_active_text: Color,
    pub ribbon_group_label: Color,
    pub ribbon_group_separator: Color,
    pub ribbon_hover: Color,
    pub ribbon_hover_border: Color,
    pub ribbon_active_bg: Color,
    pub ribbon_active_border: Color,
    pub file_tab_text: Color,
    pub file_tab_hover: Color,
    pub hover: Color,
}

pub fn colors(theme: &Theme) -> UiColors {
    let background = theme.palette().background;
    UiTheme::ALL
        .into_iter()
        .find(|variant| variant.colors().shell == background)
        .unwrap_or(UiTheme::Forge)
        .colors()
}

fn theme_path() -> Option<PathBuf> {
    crate::preferences::config_directory().map(|directory| directory.join("theme"))
}
