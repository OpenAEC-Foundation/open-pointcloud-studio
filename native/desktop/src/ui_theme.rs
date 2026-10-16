//! The colours of the window, as tokens.
//!
//! The five themes are the five columns of the desktop template of the
//! OpenAEC style book, `project-templates/Tauri+React/src/themes.css` at
//! commit dfdcd41. Every `--theme-*` variable of that file is a token here,
//! named in snake case without `--theme-` (`--theme-focus-color` is `focus`
//! and `--theme-danger-color` is `danger`), with the value of its column:
//! nothing is changed and no theme borrows from another. The template keeps
//! those variables for the shell and has an application add its own colours
//! beside them, and so do these tokens:
//!
//! - `border_strong`, `success`, `warning` and `info` are tokens of the brand
//!   (`packages/tokens/tokens/colors.json` and `semantic.json`): the light
//!   values in Light, the dark values in the other four themes.
//! - `dom` holds the colours of Open Pointcloud Studio itself, its domain
//!   tokens, for what the template has no variable for: the scene, the paper
//!   of a drawing and the sheet, and what is drawn on them.

use std::fmt;
use std::path::PathBuf;

use iced::{Color, Shadow, Theme, Vector};

// The template lays `rgba(217, 119, 6, 0.08)` over its shell in sRGB, as a
// browser does; laid over in linear light, the amber of the borders and
// hovers of the dark themes would come out about twice as strong. The
// feature `web-colors` of iced blends as the template does.
const _: () = assert!(!iced::advanced::graphics::color::GAMMA_CORRECTION);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiTheme {
    Light,
    Forge,
    OpenAec,
    Blueprint,
    Contrast,
}

impl UiTheme {
    /// The themes, in the order of the theme choice of the template.
    pub const ALL: [Self; 5] = [
        Self::Light,
        Self::Forge,
        Self::OpenAec,
        Self::Blueprint,
        Self::Contrast,
    ];

    pub fn iced(self) -> Theme {
        let colors = self.colors();
        Theme::custom(
            self.to_string(),
            iced::theme::Palette {
                background: colors.bg,
                text: colors.text,
                primary: colors.accent,
                success: colors.success,
                danger: colors.danger,
            },
        )
    }

    pub fn colors(self) -> UiColors {
        match self {
            Self::Light => LIGHT,
            Self::Forge => FORGE,
            Self::OpenAec => OPENAEC,
            Self::Blueprint => BLUEPRINT,
            Self::Contrast => CONTRAST,
        }
    }

    pub fn shared(self) -> openaec_ui::Theme {
        match self {
            Self::Light => openaec_ui::Theme::Light,
            Self::Forge => openaec_ui::Theme::Forge,
            Self::OpenAec => openaec_ui::Theme::OpenAec,
            Self::Blueprint => openaec_ui::Theme::Blueprint,
            Self::Contrast => openaec_ui::Theme::Contrast,
        }
    }

    /// The background of the theme, which tells the themes apart.
    fn background(self) -> Color {
        match self {
            Self::Light => LIGHT.bg,
            Self::Forge => FORGE.bg,
            Self::OpenAec => OPENAEC.bg,
            Self::Blueprint => BLUEPRINT.bg,
            Self::Contrast => CONTRAST.bg,
        }
    }

    /// The four colours the theme choice of the template shows before the
    /// name of a theme (`THEME_OPTIONS` in `SettingsDialog.tsx`).
    pub fn swatches(self) -> [Color; 4] {
        self.shared().swatches()
    }

    /// The key of the theme in the settings and the API: its name in the
    /// template.
    pub fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Forge => "forge",
            Self::OpenAec => "openaec",
            Self::Blueprint => "blueprint",
            Self::Contrast => "contrast",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        if value == "night" {
            return Some(Self::OpenAec);
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
        // The names of the template; they are translated where Settings
        // lists the themes.
        let label = match self {
            Self::Light => crate::i18n::key("Light"),
            Self::Forge => crate::i18n::key("Forge (dark)"),
            Self::OpenAec => crate::i18n::key("OpenAEC (dark)"),
            Self::Blueprint => crate::i18n::key("Blueprint"),
            Self::Contrast => crate::i18n::key("High contrast"),
        };
        f.write_str(label)
    }
}

/// The tokens of a theme. The fields up to `btn_secondary_hover_border` are
/// the variables of `themes.css` in the order of that file; not every one of
/// them has a part of the window to colour, as in the template itself.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct UiColors {
    /// The shell: the ribbon, the panels and the active tab.
    pub bg: Color,
    /// The tab row, section heads and the File menu.
    pub bg_lighter: Color,
    /// The bar of the document tabs.
    pub docbar_bg: Color,
    pub surface: Color,
    pub border: Color,
    pub border_subtle: Color,
    pub text: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_faint: Color,
    pub accent: Color,
    pub accent_hover: Color,
    /// Text on the accent colour.
    pub accent_text: Color,
    pub accent_soft: Color,
    pub accent_tint: Color,
    pub hover: Color,
    pub hover_strong: Color,
    pub active: Color,
    pub focus: Color,
    pub danger: Color,
    pub danger_hover: Color,
    pub dialog_shadow: Shadow,
    pub panel_shadow: Shadow,
    pub popover_shadow: Shadow,
    pub file_tab_bg: Color,
    pub file_tab_hover: Color,
    pub file_tab_text: Color,
    /// The work area of the template. The scene and the paper have tokens
    /// of their own in `dom`.
    pub content_bg: Color,
    pub placeholder_icon: Color,
    pub placeholder_heading: Color,
    pub placeholder_text: Color,
    pub ribbon_btn_hover: Color,
    pub ribbon_btn_hover_border: Color,
    pub ribbon_btn_active_bg: Color,
    pub ribbon_btn_active_border: Color,
    pub ribbon_btn_active_text: Color,
    pub ribbon_icon_active: Color,
    pub ribbon_text_hover: Color,
    pub ribbon_group_separator: Color,
    pub ribbon_group_label: Color,
    pub status_bg: Color,
    pub status_border: Color,
    pub status_text: Color,
    pub status_text_label: Color,
    pub status_hover: Color,
    pub status_separator: Color,
    pub backstage_item_shortcut: Color,
    pub backstage_item_shortcut_hover: Color,
    pub dialog_overlay: Color,
    pub dialog_bg: Color,
    pub dialog_border: Color,
    pub dialog_header_bg: Color,
    pub dialog_header_text: Color,
    pub dialog_sidebar_bg: Color,
    pub dialog_sidebar_border: Color,
    pub dialog_tab_text: Color,
    pub dialog_tab_hover: Color,
    pub dialog_tab_hover_text: Color,
    pub dialog_tab_active_bg: Color,
    pub dialog_tab_active_text: Color,
    pub dialog_tab_active_accent: Color,
    pub dialog_content_bg: Color,
    pub dialog_content_text: Color,
    pub dialog_content_secondary: Color,
    pub dialog_input_bg: Color,
    pub dialog_input_border: Color,
    pub dialog_input_text: Color,
    pub dialog_btn_bg: Color,
    pub dialog_btn_text: Color,
    pub dialog_btn_hover: Color,
    pub dialog_section_border: Color,
    pub btn_primary_bg: Color,
    pub btn_primary_text: Color,
    pub btn_primary_border: Color,
    pub btn_primary_hover_bg: Color,
    pub btn_primary_hover_text: Color,
    pub btn_secondary_bg: Color,
    pub btn_secondary_text: Color,
    pub btn_secondary_hover_bg: Color,
    pub btn_secondary_hover_border: Color,
    /// The strong border of the brand: the rail of a slider, the track of a
    /// progress bar.
    pub border_strong: Color,
    /// The semantic colours of the brand. Danger is `danger` above.
    pub success: Color,
    pub warning: Color,
    pub info: Color,
    /// The tooltip of the brand (`.tooltip` of the style guide): Deep Forge
    /// with Blueprint White, in every theme.
    pub tooltip_bg: Color,
    pub tooltip_text: Color,
    /// The domain tokens of Open Pointcloud Studio.
    pub dom: DomainColors,
}

/// The colours of Open Pointcloud Studio itself, which the template leaves
/// to an application. They are colours of the brand or of the themes, the
/// same in all four dark themes; where one carries a meaning (an error is
/// red, information blue) it keeps its colour in every theme.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct DomainColors {
    /// The 3D scene: white in Light, as the light work area of the
    /// template, and Night Build in the dark themes, the canvas the layouts
    /// of the style book give a dark theme. The 3D viewer of the template
    /// has `#1A1A2E`, which is no colour of the brand.
    pub scene: Color,
    /// Text on the scene and its quieter variant: the brand's text on light
    /// (Deep Forge) or on dark (Blueprint White).
    pub scene_text: Color,
    pub scene_muted: Color,
    /// Labels beside the handles of the section box: never amber as text,
    /// Warm Gold on dark.
    pub scene_label: Color,
    /// The placeholder of an empty scene: the template's values for a light
    /// work area (light) and a dark one (openaec).
    pub scene_placeholder_icon: Color,
    pub scene_placeholder_heading: Color,
    pub scene_placeholder_text: Color,
    /// A label on the scene, as the tooltip of the brand.
    pub scene_badge_bg: Color,
    pub scene_badge_text: Color,
    /// The paper of a drawing and of a sheet: white in every theme, as the
    /// page of the template's report preview.
    pub paper: Color,
    /// What lies around the paper of a sheet, as around that page.
    pub desk: Color,
    /// The crop region of a drawing, on the paper that is always light.
    pub crop: Color,
    /// Photo marks and the slab of a drawing in the scene: information.
    pub photo_mark: Color,
    pub slab: Color,
    /// Annotations: the colour of an error.
    pub annotation: Color,
    /// The axes of a scan station: error, success and information.
    pub axis_x: Color,
    pub axis_y: Color,
    pub axis_z: Color,
    /// Charts: amber, gold and orange in that order, and two neutrals.
    pub chart_1: Color,
    pub chart_2: Color,
    pub chart_3: Color,
    pub chart_neutral: Color,
    pub chart_neutral_light: Color,
}

const DOM_LIGHT: DomainColors = DomainColors {
    scene: hex(0xFFFFFF),
    scene_text: hex(0x36363E),
    scene_muted: hex(0x71717A),
    scene_label: hex(0x36363E),
    scene_placeholder_icon: rgba(217, 119, 6, 0.25),
    scene_placeholder_heading: rgba(54, 54, 62, 0.4),
    scene_placeholder_text: rgba(54, 54, 62, 0.3),
    scene_badge_bg: hex(0x36363E),
    scene_badge_text: hex(0xFAFAF9),
    paper: hex(0xFFFFFF),
    desk: hex(0x52525B),
    crop: hex(0x2563EB),
    photo_mark: hex(0x2563EB),
    slab: hex(0x2563EB),
    annotation: hex(0xDC2626),
    axis_x: hex(0xDC2626),
    axis_y: hex(0x16A34A),
    axis_z: hex(0x2563EB),
    chart_1: hex(0xD97706),
    chart_2: hex(0xF59E0B),
    chart_3: hex(0xEA580C),
    chart_neutral: hex(0xA1A1AA),
    chart_neutral_light: hex(0xD6D3D1),
};

const DOM_DARK: DomainColors = DomainColors {
    scene: hex(0x2A2A32),
    scene_text: hex(0xFAFAF9),
    scene_muted: hex(0xA1A1AA),
    scene_label: hex(0xF59E0B),
    scene_placeholder_icon: rgba(217, 119, 6, 0.2),
    scene_placeholder_heading: rgba(250, 250, 249, 0.3),
    scene_placeholder_text: rgba(250, 250, 249, 0.25),
    scene_badge_bg: hex(0x36363E),
    scene_badge_text: hex(0xFAFAF9),
    paper: hex(0xFFFFFF),
    desk: hex(0x52525B),
    crop: hex(0x2563EB),
    photo_mark: hex(0x60A5FA),
    slab: hex(0x60A5FA),
    annotation: hex(0xF87171),
    axis_x: hex(0xF87171),
    axis_y: hex(0x4ADE80),
    axis_z: hex(0x60A5FA),
    chart_1: hex(0xD97706),
    chart_2: hex(0xF59E0B),
    chart_3: hex(0xEA580C),
    chart_neutral: hex(0xA1A1AA),
    chart_neutral_light: hex(0x3F3F46),
};

/// A colour written as `#RRGGBB`.
const fn hex(value: u32) -> Color {
    Color {
        r: ((value >> 16) & 0xFF) as f32 / 255.0,
        g: ((value >> 8) & 0xFF) as f32 / 255.0,
        b: (value & 0xFF) as f32 / 255.0,
        a: 1.0,
    }
}

/// A colour written as `rgba(r, g, b, a)`.
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color {
    Color {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a,
    }
}

/// A shadow written as `x y blur colour`; the template's shadows spread
/// nothing.
const fn shadow(x: f32, y: f32, blur_radius: f32, color: Color) -> Shadow {
    Shadow {
        color,
        offset: Vector::new(x, y),
        blur_radius,
    }
}

/// `[data-theme="light"]`, the default.
const LIGHT: UiColors = UiColors {
    bg: openaec_ui::tokens::LIGHT.bg,
    bg_lighter: openaec_ui::tokens::LIGHT.bg_lighter,
    docbar_bg: openaec_ui::tokens::LIGHT.docbar_bg,
    surface: openaec_ui::tokens::LIGHT.surface,
    border: openaec_ui::tokens::LIGHT.border,
    border_subtle: openaec_ui::tokens::LIGHT.border_subtle,
    text: openaec_ui::tokens::LIGHT.text,
    text_secondary: openaec_ui::tokens::LIGHT.text_secondary,
    text_muted: openaec_ui::tokens::LIGHT.text_muted,
    text_faint: openaec_ui::tokens::LIGHT.text_faint,
    accent: openaec_ui::tokens::LIGHT.accent,
    accent_hover: openaec_ui::tokens::LIGHT.accent_hover,
    accent_text: openaec_ui::tokens::LIGHT.accent_text,
    accent_soft: openaec_ui::tokens::LIGHT.accent_soft,
    accent_tint: openaec_ui::tokens::LIGHT.accent_tint,
    hover: openaec_ui::tokens::LIGHT.hover,
    hover_strong: openaec_ui::tokens::LIGHT.hover_strong,
    active: openaec_ui::tokens::LIGHT.active,
    focus: openaec_ui::tokens::LIGHT.focus_color,
    danger: openaec_ui::tokens::LIGHT.danger_color,
    danger_hover: openaec_ui::tokens::LIGHT.danger_hover,
    dialog_shadow: shadow(0.0, 4.0, 16.0, rgba(0, 0, 0, 0.12)),
    panel_shadow: shadow(2.0, 0.0, 8.0, rgba(0, 0, 0, 0.06)),
    popover_shadow: shadow(0.0, 8.0, 24.0, rgba(0, 0, 0, 0.15)),
    file_tab_bg: openaec_ui::tokens::LIGHT.file_tab_bg,
    file_tab_hover: openaec_ui::tokens::LIGHT.file_tab_hover,
    file_tab_text: openaec_ui::tokens::LIGHT.file_tab_text,
    content_bg: openaec_ui::tokens::LIGHT.content_bg,
    placeholder_icon: openaec_ui::tokens::LIGHT.placeholder_icon,
    placeholder_heading: openaec_ui::tokens::LIGHT.placeholder_heading,
    placeholder_text: openaec_ui::tokens::LIGHT.placeholder_text,
    ribbon_btn_hover: openaec_ui::tokens::LIGHT.ribbon_btn_hover,
    ribbon_btn_hover_border: openaec_ui::tokens::LIGHT.ribbon_btn_hover_border,
    ribbon_btn_active_bg: openaec_ui::tokens::LIGHT.ribbon_btn_active_bg,
    ribbon_btn_active_border: openaec_ui::tokens::LIGHT.ribbon_btn_active_border,
    ribbon_btn_active_text: openaec_ui::tokens::LIGHT.ribbon_btn_active_text,
    ribbon_icon_active: openaec_ui::tokens::LIGHT.ribbon_icon_active,
    ribbon_text_hover: openaec_ui::tokens::LIGHT.ribbon_text_hover,
    ribbon_group_separator: openaec_ui::tokens::LIGHT.ribbon_group_separator,
    ribbon_group_label: openaec_ui::tokens::LIGHT.ribbon_group_label,
    status_bg: openaec_ui::tokens::LIGHT.status_bg,
    status_border: openaec_ui::tokens::LIGHT.status_border,
    status_text: openaec_ui::tokens::LIGHT.status_text,
    status_text_label: openaec_ui::tokens::LIGHT.status_text_label,
    status_hover: openaec_ui::tokens::LIGHT.status_hover,
    status_separator: openaec_ui::tokens::LIGHT.status_separator,
    backstage_item_shortcut: openaec_ui::tokens::LIGHT.backstage_item_shortcut,
    backstage_item_shortcut_hover: openaec_ui::tokens::LIGHT.backstage_item_shortcut_hover,
    dialog_overlay: openaec_ui::tokens::LIGHT.dialog_overlay,
    dialog_bg: openaec_ui::tokens::LIGHT.dialog_bg,
    dialog_border: openaec_ui::tokens::LIGHT.dialog_border,
    dialog_header_bg: openaec_ui::tokens::LIGHT.dialog_header_bg,
    dialog_header_text: openaec_ui::tokens::LIGHT.dialog_header_text,
    dialog_sidebar_bg: openaec_ui::tokens::LIGHT.dialog_sidebar_bg,
    dialog_sidebar_border: openaec_ui::tokens::LIGHT.dialog_sidebar_border,
    dialog_tab_text: openaec_ui::tokens::LIGHT.dialog_tab_text,
    dialog_tab_hover: openaec_ui::tokens::LIGHT.dialog_tab_hover,
    dialog_tab_hover_text: openaec_ui::tokens::LIGHT.dialog_tab_hover_text,
    dialog_tab_active_bg: openaec_ui::tokens::LIGHT.dialog_tab_active_bg,
    dialog_tab_active_text: openaec_ui::tokens::LIGHT.dialog_tab_active_text,
    dialog_tab_active_accent: openaec_ui::tokens::LIGHT.dialog_tab_active_accent,
    dialog_content_bg: openaec_ui::tokens::LIGHT.dialog_content_bg,
    dialog_content_text: openaec_ui::tokens::LIGHT.dialog_content_text,
    dialog_content_secondary: openaec_ui::tokens::LIGHT.dialog_content_secondary,
    dialog_input_bg: openaec_ui::tokens::LIGHT.dialog_input_bg,
    dialog_input_border: openaec_ui::tokens::LIGHT.dialog_input_border,
    dialog_input_text: openaec_ui::tokens::LIGHT.dialog_input_text,
    dialog_btn_bg: openaec_ui::tokens::LIGHT.dialog_btn_bg,
    dialog_btn_text: openaec_ui::tokens::LIGHT.dialog_btn_text,
    dialog_btn_hover: openaec_ui::tokens::LIGHT.dialog_btn_hover,
    dialog_section_border: openaec_ui::tokens::LIGHT.dialog_section_border,
    btn_primary_bg: openaec_ui::tokens::LIGHT.btn_primary_bg,
    btn_primary_text: openaec_ui::tokens::LIGHT.btn_primary_text,
    btn_primary_border: openaec_ui::tokens::LIGHT.btn_primary_border,
    btn_primary_hover_bg: openaec_ui::tokens::LIGHT.btn_primary_hover_bg,
    btn_primary_hover_text: openaec_ui::tokens::LIGHT.btn_primary_hover_text,
    btn_secondary_bg: openaec_ui::tokens::LIGHT.btn_secondary_bg,
    btn_secondary_text: openaec_ui::tokens::LIGHT.btn_secondary_text,
    btn_secondary_hover_bg: openaec_ui::tokens::LIGHT.btn_secondary_hover_bg,
    btn_secondary_hover_border: openaec_ui::tokens::LIGHT.btn_secondary_hover_border,
    border_strong: hex(0xD6D3D1),
    success: hex(0x16A34A),
    warning: hex(0xF59E0B),
    info: hex(0x2563EB),
    tooltip_bg: hex(0x36363E),
    tooltip_text: hex(0xFAFAF9),
    dom: DOM_LIGHT,
};

/// `[data-theme="forge"]`.
const FORGE: UiColors = UiColors {
    bg: openaec_ui::tokens::FORGE.bg,
    bg_lighter: openaec_ui::tokens::FORGE.bg_lighter,
    docbar_bg: openaec_ui::tokens::FORGE.docbar_bg,
    surface: openaec_ui::tokens::FORGE.surface,
    border: openaec_ui::tokens::FORGE.border,
    border_subtle: openaec_ui::tokens::FORGE.border_subtle,
    text: openaec_ui::tokens::FORGE.text,
    text_secondary: openaec_ui::tokens::FORGE.text_secondary,
    text_muted: openaec_ui::tokens::FORGE.text_muted,
    text_faint: openaec_ui::tokens::FORGE.text_faint,
    accent: openaec_ui::tokens::FORGE.accent,
    accent_hover: openaec_ui::tokens::FORGE.accent_hover,
    accent_text: openaec_ui::tokens::FORGE.accent_text,
    accent_soft: openaec_ui::tokens::FORGE.accent_soft,
    accent_tint: openaec_ui::tokens::FORGE.accent_tint,
    hover: openaec_ui::tokens::FORGE.hover,
    hover_strong: openaec_ui::tokens::FORGE.hover_strong,
    active: openaec_ui::tokens::FORGE.active,
    focus: openaec_ui::tokens::FORGE.focus_color,
    danger: openaec_ui::tokens::FORGE.danger_color,
    danger_hover: openaec_ui::tokens::FORGE.danger_hover,
    dialog_shadow: shadow(0.0, 4.0, 16.0, rgba(0, 0, 0, 0.35)),
    panel_shadow: shadow(2.0, 0.0, 8.0, rgba(0, 0, 0, 0.20)),
    popover_shadow: shadow(0.0, 8.0, 24.0, rgba(0, 0, 0, 0.40)),
    file_tab_bg: openaec_ui::tokens::FORGE.file_tab_bg,
    file_tab_hover: openaec_ui::tokens::FORGE.file_tab_hover,
    file_tab_text: openaec_ui::tokens::FORGE.file_tab_text,
    content_bg: openaec_ui::tokens::FORGE.content_bg,
    placeholder_icon: openaec_ui::tokens::FORGE.placeholder_icon,
    placeholder_heading: openaec_ui::tokens::FORGE.placeholder_heading,
    placeholder_text: openaec_ui::tokens::FORGE.placeholder_text,
    ribbon_btn_hover: openaec_ui::tokens::FORGE.ribbon_btn_hover,
    ribbon_btn_hover_border: openaec_ui::tokens::FORGE.ribbon_btn_hover_border,
    ribbon_btn_active_bg: openaec_ui::tokens::FORGE.ribbon_btn_active_bg,
    ribbon_btn_active_border: openaec_ui::tokens::FORGE.ribbon_btn_active_border,
    ribbon_btn_active_text: openaec_ui::tokens::FORGE.ribbon_btn_active_text,
    ribbon_icon_active: openaec_ui::tokens::FORGE.ribbon_icon_active,
    ribbon_text_hover: openaec_ui::tokens::FORGE.ribbon_text_hover,
    ribbon_group_separator: openaec_ui::tokens::FORGE.ribbon_group_separator,
    ribbon_group_label: openaec_ui::tokens::FORGE.ribbon_group_label,
    status_bg: openaec_ui::tokens::FORGE.status_bg,
    status_border: openaec_ui::tokens::FORGE.status_border,
    status_text: openaec_ui::tokens::FORGE.status_text,
    status_text_label: openaec_ui::tokens::FORGE.status_text_label,
    status_hover: openaec_ui::tokens::FORGE.status_hover,
    status_separator: openaec_ui::tokens::FORGE.status_separator,
    backstage_item_shortcut: openaec_ui::tokens::FORGE.backstage_item_shortcut,
    backstage_item_shortcut_hover: openaec_ui::tokens::FORGE.backstage_item_shortcut_hover,
    dialog_overlay: openaec_ui::tokens::FORGE.dialog_overlay,
    dialog_bg: openaec_ui::tokens::FORGE.dialog_bg,
    dialog_border: openaec_ui::tokens::FORGE.dialog_border,
    dialog_header_bg: openaec_ui::tokens::FORGE.dialog_header_bg,
    dialog_header_text: openaec_ui::tokens::FORGE.dialog_header_text,
    dialog_sidebar_bg: openaec_ui::tokens::FORGE.dialog_sidebar_bg,
    dialog_sidebar_border: openaec_ui::tokens::FORGE.dialog_sidebar_border,
    dialog_tab_text: openaec_ui::tokens::FORGE.dialog_tab_text,
    dialog_tab_hover: openaec_ui::tokens::FORGE.dialog_tab_hover,
    dialog_tab_hover_text: openaec_ui::tokens::FORGE.dialog_tab_hover_text,
    dialog_tab_active_bg: openaec_ui::tokens::FORGE.dialog_tab_active_bg,
    dialog_tab_active_text: openaec_ui::tokens::FORGE.dialog_tab_active_text,
    dialog_tab_active_accent: openaec_ui::tokens::FORGE.dialog_tab_active_accent,
    dialog_content_bg: openaec_ui::tokens::FORGE.dialog_content_bg,
    dialog_content_text: openaec_ui::tokens::FORGE.dialog_content_text,
    dialog_content_secondary: openaec_ui::tokens::FORGE.dialog_content_secondary,
    dialog_input_bg: openaec_ui::tokens::FORGE.dialog_input_bg,
    dialog_input_border: openaec_ui::tokens::FORGE.dialog_input_border,
    dialog_input_text: openaec_ui::tokens::FORGE.dialog_input_text,
    dialog_btn_bg: openaec_ui::tokens::FORGE.dialog_btn_bg,
    dialog_btn_text: openaec_ui::tokens::FORGE.dialog_btn_text,
    dialog_btn_hover: openaec_ui::tokens::FORGE.dialog_btn_hover,
    dialog_section_border: openaec_ui::tokens::FORGE.dialog_section_border,
    btn_primary_bg: openaec_ui::tokens::FORGE.btn_primary_bg,
    btn_primary_text: openaec_ui::tokens::FORGE.btn_primary_text,
    btn_primary_border: openaec_ui::tokens::FORGE.btn_primary_border,
    btn_primary_hover_bg: openaec_ui::tokens::FORGE.btn_primary_hover_bg,
    btn_primary_hover_text: openaec_ui::tokens::FORGE.btn_primary_hover_text,
    btn_secondary_bg: openaec_ui::tokens::FORGE.btn_secondary_bg,
    btn_secondary_text: openaec_ui::tokens::FORGE.btn_secondary_text,
    btn_secondary_hover_bg: openaec_ui::tokens::FORGE.btn_secondary_hover_bg,
    btn_secondary_hover_border: openaec_ui::tokens::FORGE.btn_secondary_hover_border,
    border_strong: hex(0x3F3F46),
    success: hex(0x4ADE80),
    warning: hex(0xFBBF24),
    info: hex(0x60A5FA),
    tooltip_bg: hex(0x36363E),
    tooltip_text: hex(0xFAFAF9),
    dom: DOM_DARK,
};

/// `[data-theme="openaec"]`.
const OPENAEC: UiColors = UiColors {
    bg: openaec_ui::tokens::OPENAEC.bg,
    bg_lighter: openaec_ui::tokens::OPENAEC.bg_lighter,
    docbar_bg: openaec_ui::tokens::OPENAEC.docbar_bg,
    surface: openaec_ui::tokens::OPENAEC.surface,
    border: openaec_ui::tokens::OPENAEC.border,
    border_subtle: openaec_ui::tokens::OPENAEC.border_subtle,
    text: openaec_ui::tokens::OPENAEC.text,
    text_secondary: openaec_ui::tokens::OPENAEC.text_secondary,
    text_muted: openaec_ui::tokens::OPENAEC.text_muted,
    text_faint: openaec_ui::tokens::OPENAEC.text_faint,
    accent: openaec_ui::tokens::OPENAEC.accent,
    accent_hover: openaec_ui::tokens::OPENAEC.accent_hover,
    accent_text: openaec_ui::tokens::OPENAEC.accent_text,
    accent_soft: openaec_ui::tokens::OPENAEC.accent_soft,
    accent_tint: openaec_ui::tokens::OPENAEC.accent_tint,
    hover: openaec_ui::tokens::OPENAEC.hover,
    hover_strong: openaec_ui::tokens::OPENAEC.hover_strong,
    active: openaec_ui::tokens::OPENAEC.active,
    focus: openaec_ui::tokens::OPENAEC.focus_color,
    danger: openaec_ui::tokens::OPENAEC.danger_color,
    danger_hover: openaec_ui::tokens::OPENAEC.danger_hover,
    dialog_shadow: shadow(0.0, 4.0, 16.0, rgba(0, 0, 0, 0.40)),
    panel_shadow: shadow(2.0, 0.0, 8.0, rgba(0, 0, 0, 0.25)),
    popover_shadow: shadow(0.0, 8.0, 24.0, rgba(0, 0, 0, 0.45)),
    file_tab_bg: openaec_ui::tokens::OPENAEC.file_tab_bg,
    file_tab_hover: openaec_ui::tokens::OPENAEC.file_tab_hover,
    file_tab_text: openaec_ui::tokens::OPENAEC.file_tab_text,
    content_bg: openaec_ui::tokens::OPENAEC.content_bg,
    placeholder_icon: openaec_ui::tokens::OPENAEC.placeholder_icon,
    placeholder_heading: openaec_ui::tokens::OPENAEC.placeholder_heading,
    placeholder_text: openaec_ui::tokens::OPENAEC.placeholder_text,
    ribbon_btn_hover: openaec_ui::tokens::OPENAEC.ribbon_btn_hover,
    ribbon_btn_hover_border: openaec_ui::tokens::OPENAEC.ribbon_btn_hover_border,
    ribbon_btn_active_bg: openaec_ui::tokens::OPENAEC.ribbon_btn_active_bg,
    ribbon_btn_active_border: openaec_ui::tokens::OPENAEC.ribbon_btn_active_border,
    ribbon_btn_active_text: openaec_ui::tokens::OPENAEC.ribbon_btn_active_text,
    ribbon_icon_active: openaec_ui::tokens::OPENAEC.ribbon_icon_active,
    ribbon_text_hover: openaec_ui::tokens::OPENAEC.ribbon_text_hover,
    ribbon_group_separator: openaec_ui::tokens::OPENAEC.ribbon_group_separator,
    ribbon_group_label: openaec_ui::tokens::OPENAEC.ribbon_group_label,
    status_bg: openaec_ui::tokens::OPENAEC.status_bg,
    status_border: openaec_ui::tokens::OPENAEC.status_border,
    status_text: openaec_ui::tokens::OPENAEC.status_text,
    status_text_label: openaec_ui::tokens::OPENAEC.status_text_label,
    status_hover: openaec_ui::tokens::OPENAEC.status_hover,
    status_separator: openaec_ui::tokens::OPENAEC.status_separator,
    backstage_item_shortcut: openaec_ui::tokens::OPENAEC.backstage_item_shortcut,
    backstage_item_shortcut_hover: openaec_ui::tokens::OPENAEC.backstage_item_shortcut_hover,
    dialog_overlay: openaec_ui::tokens::OPENAEC.dialog_overlay,
    dialog_bg: openaec_ui::tokens::OPENAEC.dialog_bg,
    dialog_border: openaec_ui::tokens::OPENAEC.dialog_border,
    dialog_header_bg: openaec_ui::tokens::OPENAEC.dialog_header_bg,
    dialog_header_text: openaec_ui::tokens::OPENAEC.dialog_header_text,
    dialog_sidebar_bg: openaec_ui::tokens::OPENAEC.dialog_sidebar_bg,
    dialog_sidebar_border: openaec_ui::tokens::OPENAEC.dialog_sidebar_border,
    dialog_tab_text: openaec_ui::tokens::OPENAEC.dialog_tab_text,
    dialog_tab_hover: openaec_ui::tokens::OPENAEC.dialog_tab_hover,
    dialog_tab_hover_text: openaec_ui::tokens::OPENAEC.dialog_tab_hover_text,
    dialog_tab_active_bg: openaec_ui::tokens::OPENAEC.dialog_tab_active_bg,
    dialog_tab_active_text: openaec_ui::tokens::OPENAEC.dialog_tab_active_text,
    dialog_tab_active_accent: openaec_ui::tokens::OPENAEC.dialog_tab_active_accent,
    dialog_content_bg: openaec_ui::tokens::OPENAEC.dialog_content_bg,
    dialog_content_text: openaec_ui::tokens::OPENAEC.dialog_content_text,
    dialog_content_secondary: openaec_ui::tokens::OPENAEC.dialog_content_secondary,
    dialog_input_bg: openaec_ui::tokens::OPENAEC.dialog_input_bg,
    dialog_input_border: openaec_ui::tokens::OPENAEC.dialog_input_border,
    dialog_input_text: openaec_ui::tokens::OPENAEC.dialog_input_text,
    dialog_btn_bg: openaec_ui::tokens::OPENAEC.dialog_btn_bg,
    dialog_btn_text: openaec_ui::tokens::OPENAEC.dialog_btn_text,
    dialog_btn_hover: openaec_ui::tokens::OPENAEC.dialog_btn_hover,
    dialog_section_border: openaec_ui::tokens::OPENAEC.dialog_section_border,
    btn_primary_bg: openaec_ui::tokens::OPENAEC.btn_primary_bg,
    btn_primary_text: openaec_ui::tokens::OPENAEC.btn_primary_text,
    btn_primary_border: openaec_ui::tokens::OPENAEC.btn_primary_border,
    btn_primary_hover_bg: openaec_ui::tokens::OPENAEC.btn_primary_hover_bg,
    btn_primary_hover_text: openaec_ui::tokens::OPENAEC.btn_primary_hover_text,
    btn_secondary_bg: openaec_ui::tokens::OPENAEC.btn_secondary_bg,
    btn_secondary_text: openaec_ui::tokens::OPENAEC.btn_secondary_text,
    btn_secondary_hover_bg: openaec_ui::tokens::OPENAEC.btn_secondary_hover_bg,
    btn_secondary_hover_border: openaec_ui::tokens::OPENAEC.btn_secondary_hover_border,
    border_strong: hex(0x3F3F46),
    success: hex(0x4ADE80),
    warning: hex(0xFBBF24),
    info: hex(0x60A5FA),
    tooltip_bg: hex(0x36363E),
    tooltip_text: hex(0xFAFAF9),
    dom: DOM_DARK,
};

/// `[data-theme="blueprint"]`.
const BLUEPRINT: UiColors = UiColors {
    bg: openaec_ui::tokens::BLUEPRINT.bg,
    bg_lighter: openaec_ui::tokens::BLUEPRINT.bg_lighter,
    docbar_bg: openaec_ui::tokens::BLUEPRINT.docbar_bg,
    surface: openaec_ui::tokens::BLUEPRINT.surface,
    border: openaec_ui::tokens::BLUEPRINT.border,
    border_subtle: openaec_ui::tokens::BLUEPRINT.border_subtle,
    text: openaec_ui::tokens::BLUEPRINT.text,
    text_secondary: openaec_ui::tokens::BLUEPRINT.text_secondary,
    text_muted: openaec_ui::tokens::BLUEPRINT.text_muted,
    text_faint: openaec_ui::tokens::BLUEPRINT.text_faint,
    accent: openaec_ui::tokens::BLUEPRINT.accent,
    accent_hover: openaec_ui::tokens::BLUEPRINT.accent_hover,
    accent_text: openaec_ui::tokens::BLUEPRINT.accent_text,
    accent_soft: openaec_ui::tokens::BLUEPRINT.accent_soft,
    accent_tint: openaec_ui::tokens::BLUEPRINT.accent_tint,
    hover: openaec_ui::tokens::BLUEPRINT.hover,
    hover_strong: openaec_ui::tokens::BLUEPRINT.hover_strong,
    active: openaec_ui::tokens::BLUEPRINT.active,
    focus: openaec_ui::tokens::BLUEPRINT.focus_color,
    danger: openaec_ui::tokens::BLUEPRINT.danger_color,
    danger_hover: openaec_ui::tokens::BLUEPRINT.danger_hover,
    dialog_shadow: shadow(0.0, 4.0, 16.0, rgba(0, 0, 0, 0.45)),
    panel_shadow: shadow(2.0, 0.0, 8.0, rgba(0, 0, 0, 0.30)),
    popover_shadow: shadow(0.0, 8.0, 24.0, rgba(0, 0, 0, 0.50)),
    file_tab_bg: openaec_ui::tokens::BLUEPRINT.file_tab_bg,
    file_tab_hover: openaec_ui::tokens::BLUEPRINT.file_tab_hover,
    file_tab_text: openaec_ui::tokens::BLUEPRINT.file_tab_text,
    content_bg: openaec_ui::tokens::BLUEPRINT.content_bg,
    placeholder_icon: openaec_ui::tokens::BLUEPRINT.placeholder_icon,
    placeholder_heading: openaec_ui::tokens::BLUEPRINT.placeholder_heading,
    placeholder_text: openaec_ui::tokens::BLUEPRINT.placeholder_text,
    ribbon_btn_hover: openaec_ui::tokens::BLUEPRINT.ribbon_btn_hover,
    ribbon_btn_hover_border: openaec_ui::tokens::BLUEPRINT.ribbon_btn_hover_border,
    ribbon_btn_active_bg: openaec_ui::tokens::BLUEPRINT.ribbon_btn_active_bg,
    ribbon_btn_active_border: openaec_ui::tokens::BLUEPRINT.ribbon_btn_active_border,
    ribbon_btn_active_text: openaec_ui::tokens::BLUEPRINT.ribbon_btn_active_text,
    ribbon_icon_active: openaec_ui::tokens::BLUEPRINT.ribbon_icon_active,
    ribbon_text_hover: openaec_ui::tokens::BLUEPRINT.ribbon_text_hover,
    ribbon_group_separator: openaec_ui::tokens::BLUEPRINT.ribbon_group_separator,
    ribbon_group_label: openaec_ui::tokens::BLUEPRINT.ribbon_group_label,
    status_bg: openaec_ui::tokens::BLUEPRINT.status_bg,
    status_border: openaec_ui::tokens::BLUEPRINT.status_border,
    status_text: openaec_ui::tokens::BLUEPRINT.status_text,
    status_text_label: openaec_ui::tokens::BLUEPRINT.status_text_label,
    status_hover: openaec_ui::tokens::BLUEPRINT.status_hover,
    status_separator: openaec_ui::tokens::BLUEPRINT.status_separator,
    backstage_item_shortcut: openaec_ui::tokens::BLUEPRINT.backstage_item_shortcut,
    backstage_item_shortcut_hover: openaec_ui::tokens::BLUEPRINT.backstage_item_shortcut_hover,
    dialog_overlay: openaec_ui::tokens::BLUEPRINT.dialog_overlay,
    dialog_bg: openaec_ui::tokens::BLUEPRINT.dialog_bg,
    dialog_border: openaec_ui::tokens::BLUEPRINT.dialog_border,
    dialog_header_bg: openaec_ui::tokens::BLUEPRINT.dialog_header_bg,
    dialog_header_text: openaec_ui::tokens::BLUEPRINT.dialog_header_text,
    dialog_sidebar_bg: openaec_ui::tokens::BLUEPRINT.dialog_sidebar_bg,
    dialog_sidebar_border: openaec_ui::tokens::BLUEPRINT.dialog_sidebar_border,
    dialog_tab_text: openaec_ui::tokens::BLUEPRINT.dialog_tab_text,
    dialog_tab_hover: openaec_ui::tokens::BLUEPRINT.dialog_tab_hover,
    dialog_tab_hover_text: openaec_ui::tokens::BLUEPRINT.dialog_tab_hover_text,
    dialog_tab_active_bg: openaec_ui::tokens::BLUEPRINT.dialog_tab_active_bg,
    dialog_tab_active_text: openaec_ui::tokens::BLUEPRINT.dialog_tab_active_text,
    dialog_tab_active_accent: openaec_ui::tokens::BLUEPRINT.dialog_tab_active_accent,
    dialog_content_bg: openaec_ui::tokens::BLUEPRINT.dialog_content_bg,
    dialog_content_text: openaec_ui::tokens::BLUEPRINT.dialog_content_text,
    dialog_content_secondary: openaec_ui::tokens::BLUEPRINT.dialog_content_secondary,
    dialog_input_bg: openaec_ui::tokens::BLUEPRINT.dialog_input_bg,
    dialog_input_border: openaec_ui::tokens::BLUEPRINT.dialog_input_border,
    dialog_input_text: openaec_ui::tokens::BLUEPRINT.dialog_input_text,
    dialog_btn_bg: openaec_ui::tokens::BLUEPRINT.dialog_btn_bg,
    dialog_btn_text: openaec_ui::tokens::BLUEPRINT.dialog_btn_text,
    dialog_btn_hover: openaec_ui::tokens::BLUEPRINT.dialog_btn_hover,
    dialog_section_border: openaec_ui::tokens::BLUEPRINT.dialog_section_border,
    btn_primary_bg: openaec_ui::tokens::BLUEPRINT.btn_primary_bg,
    btn_primary_text: openaec_ui::tokens::BLUEPRINT.btn_primary_text,
    btn_primary_border: openaec_ui::tokens::BLUEPRINT.btn_primary_border,
    btn_primary_hover_bg: openaec_ui::tokens::BLUEPRINT.btn_primary_hover_bg,
    btn_primary_hover_text: openaec_ui::tokens::BLUEPRINT.btn_primary_hover_text,
    btn_secondary_bg: openaec_ui::tokens::BLUEPRINT.btn_secondary_bg,
    btn_secondary_text: openaec_ui::tokens::BLUEPRINT.btn_secondary_text,
    btn_secondary_hover_bg: openaec_ui::tokens::BLUEPRINT.btn_secondary_hover_bg,
    btn_secondary_hover_border: openaec_ui::tokens::BLUEPRINT.btn_secondary_hover_border,
    border_strong: hex(0x3F3F46),
    success: hex(0x4ADE80),
    warning: hex(0xFBBF24),
    info: hex(0x60A5FA),
    tooltip_bg: hex(0x36363E),
    tooltip_text: hex(0xFAFAF9),
    dom: DOM_DARK,
};

/// `[data-theme="contrast"]`.
const CONTRAST: UiColors = UiColors {
    bg: openaec_ui::tokens::CONTRAST.bg,
    bg_lighter: openaec_ui::tokens::CONTRAST.bg_lighter,
    docbar_bg: openaec_ui::tokens::CONTRAST.docbar_bg,
    surface: openaec_ui::tokens::CONTRAST.surface,
    border: openaec_ui::tokens::CONTRAST.border,
    border_subtle: openaec_ui::tokens::CONTRAST.border_subtle,
    text: openaec_ui::tokens::CONTRAST.text,
    text_secondary: openaec_ui::tokens::CONTRAST.text_secondary,
    text_muted: openaec_ui::tokens::CONTRAST.text_muted,
    text_faint: openaec_ui::tokens::CONTRAST.text_faint,
    accent: openaec_ui::tokens::CONTRAST.accent,
    accent_hover: openaec_ui::tokens::CONTRAST.accent_hover,
    accent_text: openaec_ui::tokens::CONTRAST.accent_text,
    accent_soft: openaec_ui::tokens::CONTRAST.accent_soft,
    accent_tint: openaec_ui::tokens::CONTRAST.accent_tint,
    hover: openaec_ui::tokens::CONTRAST.hover,
    hover_strong: openaec_ui::tokens::CONTRAST.hover_strong,
    active: openaec_ui::tokens::CONTRAST.active,
    focus: openaec_ui::tokens::CONTRAST.focus_color,
    danger: openaec_ui::tokens::CONTRAST.danger_color,
    danger_hover: openaec_ui::tokens::CONTRAST.danger_hover,
    dialog_shadow: shadow(0.0, 4.0, 16.0, rgba(255, 215, 0, 0.4)),
    panel_shadow: shadow(2.0, 0.0, 8.0, rgba(255, 215, 0, 0.3)),
    popover_shadow: shadow(0.0, 8.0, 24.0, rgba(255, 215, 0, 0.4)),
    file_tab_bg: openaec_ui::tokens::CONTRAST.file_tab_bg,
    file_tab_hover: openaec_ui::tokens::CONTRAST.file_tab_hover,
    file_tab_text: openaec_ui::tokens::CONTRAST.file_tab_text,
    content_bg: openaec_ui::tokens::CONTRAST.content_bg,
    placeholder_icon: openaec_ui::tokens::CONTRAST.placeholder_icon,
    placeholder_heading: openaec_ui::tokens::CONTRAST.placeholder_heading,
    placeholder_text: openaec_ui::tokens::CONTRAST.placeholder_text,
    ribbon_btn_hover: openaec_ui::tokens::CONTRAST.ribbon_btn_hover,
    ribbon_btn_hover_border: openaec_ui::tokens::CONTRAST.ribbon_btn_hover_border,
    ribbon_btn_active_bg: openaec_ui::tokens::CONTRAST.ribbon_btn_active_bg,
    ribbon_btn_active_border: openaec_ui::tokens::CONTRAST.ribbon_btn_active_border,
    ribbon_btn_active_text: openaec_ui::tokens::CONTRAST.ribbon_btn_active_text,
    ribbon_icon_active: openaec_ui::tokens::CONTRAST.ribbon_icon_active,
    ribbon_text_hover: openaec_ui::tokens::CONTRAST.ribbon_text_hover,
    ribbon_group_separator: openaec_ui::tokens::CONTRAST.ribbon_group_separator,
    ribbon_group_label: openaec_ui::tokens::CONTRAST.ribbon_group_label,
    status_bg: openaec_ui::tokens::CONTRAST.status_bg,
    status_border: openaec_ui::tokens::CONTRAST.status_border,
    status_text: openaec_ui::tokens::CONTRAST.status_text,
    status_text_label: openaec_ui::tokens::CONTRAST.status_text_label,
    status_hover: openaec_ui::tokens::CONTRAST.status_hover,
    status_separator: openaec_ui::tokens::CONTRAST.status_separator,
    backstage_item_shortcut: openaec_ui::tokens::CONTRAST.backstage_item_shortcut,
    backstage_item_shortcut_hover: openaec_ui::tokens::CONTRAST.backstage_item_shortcut_hover,
    dialog_overlay: openaec_ui::tokens::CONTRAST.dialog_overlay,
    dialog_bg: openaec_ui::tokens::CONTRAST.dialog_bg,
    dialog_border: openaec_ui::tokens::CONTRAST.dialog_border,
    dialog_header_bg: openaec_ui::tokens::CONTRAST.dialog_header_bg,
    dialog_header_text: openaec_ui::tokens::CONTRAST.dialog_header_text,
    dialog_sidebar_bg: openaec_ui::tokens::CONTRAST.dialog_sidebar_bg,
    dialog_sidebar_border: openaec_ui::tokens::CONTRAST.dialog_sidebar_border,
    dialog_tab_text: openaec_ui::tokens::CONTRAST.dialog_tab_text,
    dialog_tab_hover: openaec_ui::tokens::CONTRAST.dialog_tab_hover,
    dialog_tab_hover_text: openaec_ui::tokens::CONTRAST.dialog_tab_hover_text,
    dialog_tab_active_bg: openaec_ui::tokens::CONTRAST.dialog_tab_active_bg,
    dialog_tab_active_text: openaec_ui::tokens::CONTRAST.dialog_tab_active_text,
    dialog_tab_active_accent: openaec_ui::tokens::CONTRAST.dialog_tab_active_accent,
    dialog_content_bg: openaec_ui::tokens::CONTRAST.dialog_content_bg,
    dialog_content_text: openaec_ui::tokens::CONTRAST.dialog_content_text,
    dialog_content_secondary: openaec_ui::tokens::CONTRAST.dialog_content_secondary,
    dialog_input_bg: openaec_ui::tokens::CONTRAST.dialog_input_bg,
    dialog_input_border: openaec_ui::tokens::CONTRAST.dialog_input_border,
    dialog_input_text: openaec_ui::tokens::CONTRAST.dialog_input_text,
    dialog_btn_bg: openaec_ui::tokens::CONTRAST.dialog_btn_bg,
    dialog_btn_text: openaec_ui::tokens::CONTRAST.dialog_btn_text,
    dialog_btn_hover: openaec_ui::tokens::CONTRAST.dialog_btn_hover,
    dialog_section_border: openaec_ui::tokens::CONTRAST.dialog_section_border,
    btn_primary_bg: openaec_ui::tokens::CONTRAST.btn_primary_bg,
    btn_primary_text: openaec_ui::tokens::CONTRAST.btn_primary_text,
    btn_primary_border: openaec_ui::tokens::CONTRAST.btn_primary_border,
    btn_primary_hover_bg: openaec_ui::tokens::CONTRAST.btn_primary_hover_bg,
    btn_primary_hover_text: openaec_ui::tokens::CONTRAST.btn_primary_hover_text,
    btn_secondary_bg: openaec_ui::tokens::CONTRAST.btn_secondary_bg,
    btn_secondary_text: openaec_ui::tokens::CONTRAST.btn_secondary_text,
    btn_secondary_hover_bg: openaec_ui::tokens::CONTRAST.btn_secondary_hover_bg,
    btn_secondary_hover_border: openaec_ui::tokens::CONTRAST.btn_secondary_hover_border,
    border_strong: hex(0x3F3F46),
    success: hex(0x4ADE80),
    warning: hex(0xFBBF24),
    info: hex(0x60A5FA),
    tooltip_bg: hex(0x36363E),
    tooltip_text: hex(0xFAFAF9),
    dom: DOM_DARK,
};

/// The tokens of the theme a widget is drawn in, found by its background.
pub fn colors(theme: &Theme) -> UiColors {
    let background = theme.palette().background;
    UiTheme::ALL
        .into_iter()
        .find(|variant| variant.background() == background)
        .unwrap_or(UiTheme::Light)
        .colors()
}

pub fn shared_theme(theme: &Theme) -> openaec_ui::Theme {
    let background = theme.palette().background;
    UiTheme::ALL
        .into_iter()
        .find(|variant| variant.background() == background)
        .unwrap_or(UiTheme::Light)
        .shared()
}

fn theme_path() -> Option<PathBuf> {
    crate::preferences::config_directory().map(|directory| directory.join("theme"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, TestLanguage};

    /// Every variable of the template's `themes.css`, with its values in the
    /// columns light, forge, openaec, blueprint and contrast as the file
    /// writes them.
    const TEMPLATE: [(&str, [&str; 5]); 80] = [
        (
            "--theme-bg",
            ["#FAFAF9", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-bg-lighter",
            ["#FFFFFF", "#44444C", "#36363E", "#1A2C45", "#0A0A0A"],
        ),
        (
            "--theme-docbar-bg",
            ["#F5F5F4", "#2E2E36", "#1C1917", "#0A1320", "#000000"],
        ),
        (
            "--theme-surface",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-border",
            [
                "rgba(54, 54, 62, 0.12)",
                "rgba(217, 119, 6, 0.25)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(96, 165, 250, 0.20)",
                "#FFD700",
            ],
        ),
        (
            "--theme-border-subtle",
            [
                "rgba(54, 54, 62, 0.06)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(96, 165, 250, 0.12)",
                "rgba(255, 215, 0, 0.5)",
            ],
        ),
        (
            "--theme-text",
            ["#36363E", "#FAFAF9", "#FAFAF9", "#E0E7FF", "#FFFFFF"],
        ),
        (
            "--theme-text-secondary",
            [
                "rgba(54, 54, 62, 0.65)",
                "rgba(250, 250, 249, 0.6)",
                "rgba(250, 250, 249, 0.6)",
                "rgba(224, 231, 255, 0.65)",
                "#FFFFFF",
            ],
        ),
        (
            "--theme-text-muted",
            [
                "rgba(54, 54, 62, 0.5)",
                "rgba(250, 250, 249, 0.5)",
                "rgba(250, 250, 249, 0.5)",
                "rgba(224, 231, 255, 0.50)",
                "#E5E5E5",
            ],
        ),
        (
            "--theme-text-faint",
            [
                "rgba(54, 54, 62, 0.35)",
                "rgba(250, 250, 249, 0.4)",
                "rgba(250, 250, 249, 0.4)",
                "rgba(224, 231, 255, 0.35)",
                "#C0C0C0",
            ],
        ),
        (
            "--theme-accent",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-accent-hover",
            ["#EA580C", "#EA580C", "#EA580C", "#93C5FD", "#FFFF00"],
        ),
        (
            "--theme-accent-text",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-accent-soft",
            [
                "rgba(217, 119, 6, 0.08)",
                "rgba(217, 119, 6, 0.08)",
                "rgba(217, 119, 6, 0.05)",
                "rgba(96, 165, 250, 0.08)",
                "rgba(255, 215, 0, 0.15)",
            ],
        ),
        (
            "--theme-accent-tint",
            [
                "rgba(217, 119, 6, 0.85)",
                "rgba(217, 119, 6, 0.8)",
                "rgba(217, 119, 6, 0.8)",
                "rgba(96, 165, 250, 0.85)",
                "#FFD700",
            ],
        ),
        (
            "--theme-hover",
            [
                "rgba(217, 119, 6, 0.08)",
                "rgba(217, 119, 6, 0.10)",
                "rgba(217, 119, 6, 0.1)",
                "rgba(96, 165, 250, 0.10)",
                "rgba(255, 215, 0, 0.20)",
            ],
        ),
        (
            "--theme-hover-strong",
            [
                "rgba(217, 119, 6, 0.15)",
                "rgba(217, 119, 6, 0.18)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(96, 165, 250, 0.18)",
                "rgba(255, 215, 0, 0.35)",
            ],
        ),
        (
            "--theme-active",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-focus-color",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-danger-color",
            ["#DC2626", "#f87171", "#f87171", "#f87171", "#FF6B6B"],
        ),
        (
            "--theme-danger-hover",
            ["#B91C1C", "#ef4444", "#ef4444", "#ef4444", "#FF0000"],
        ),
        (
            "--theme-dialog-shadow",
            [
                "0 4px 16px rgba(0, 0, 0, 0.12)",
                "0 4px 16px rgba(0, 0, 0, 0.35)",
                "0 4px 16px rgba(0, 0, 0, 0.40)",
                "0 4px 16px rgba(0, 0, 0, 0.45)",
                "0 4px 16px rgba(255, 215, 0, 0.4)",
            ],
        ),
        (
            "--theme-panel-shadow",
            [
                "2px 0 8px rgba(0, 0, 0, 0.06)",
                "2px 0 8px rgba(0, 0, 0, 0.20)",
                "2px 0 8px rgba(0, 0, 0, 0.25)",
                "2px 0 8px rgba(0, 0, 0, 0.30)",
                "2px 0 8px rgba(255, 215, 0, 0.3)",
            ],
        ),
        (
            "--theme-popover-shadow",
            [
                "0 8px 24px rgba(0, 0, 0, 0.15)",
                "0 8px 24px rgba(0, 0, 0, 0.40)",
                "0 8px 24px rgba(0, 0, 0, 0.45)",
                "0 8px 24px rgba(0, 0, 0, 0.50)",
                "0 8px 24px rgba(255, 215, 0, 0.4)",
            ],
        ),
        (
            "--theme-file-tab-bg",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-file-tab-hover",
            ["#EA580C", "#EA580C", "#EA580C", "#93C5FD", "#FFFF00"],
        ),
        (
            "--theme-file-tab-text",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-content-bg",
            ["#FFFFFF", "#FAFAF9", "#1C1917", "#0A1320", "#000000"],
        ),
        (
            "--theme-placeholder-icon",
            [
                "rgba(217, 119, 6, 0.25)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(96, 165, 250, 0.25)",
                "#FFD700",
            ],
        ),
        (
            "--theme-placeholder-heading",
            [
                "rgba(54, 54, 62, 0.4)",
                "rgba(54, 54, 62, 0.3)",
                "rgba(250, 250, 249, 0.3)",
                "rgba(224, 231, 255, 0.35)",
                "#FFFFFF",
            ],
        ),
        (
            "--theme-placeholder-text",
            [
                "rgba(54, 54, 62, 0.3)",
                "rgba(54, 54, 62, 0.25)",
                "rgba(250, 250, 249, 0.25)",
                "rgba(224, 231, 255, 0.30)",
                "#E5E5E5",
            ],
        ),
        (
            "--theme-ribbon-btn-hover",
            [
                "rgba(217, 119, 6, 0.10)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(96, 165, 250, 0.12)",
                "rgba(255, 215, 0, 0.25)",
            ],
        ),
        (
            "--theme-ribbon-btn-hover-border",
            [
                "rgba(217, 119, 6, 0.25)",
                "rgba(217, 119, 6, 0.30)",
                "rgba(217, 119, 6, 0.30)",
                "rgba(96, 165, 250, 0.30)",
                "#FFD700",
            ],
        ),
        (
            "--theme-ribbon-btn-active-bg",
            [
                "rgba(217, 119, 6, 0.15)",
                "rgba(217, 119, 6, 0.20)",
                "rgba(217, 119, 6, 0.20)",
                "rgba(96, 165, 250, 0.18)",
                "rgba(255, 215, 0, 0.40)",
            ],
        ),
        (
            "--theme-ribbon-btn-active-border",
            [
                "rgba(217, 119, 6, 0.35)",
                "rgba(217, 119, 6, 0.40)",
                "rgba(217, 119, 6, 0.40)",
                "rgba(96, 165, 250, 0.40)",
                "#FFD700",
            ],
        ),
        (
            "--theme-ribbon-btn-active-text",
            ["#D97706", "#D97706", "#D97706", "#93C5FD", "#FFD700"],
        ),
        (
            "--theme-ribbon-icon-active",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-ribbon-text-hover",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-ribbon-group-separator",
            [
                "rgba(54, 54, 62, 0.12)",
                "rgba(250, 250, 249, 0.15)",
                "rgba(217, 119, 6, 0.25)",
                "rgba(96, 165, 250, 0.18)",
                "#FFFFFF",
            ],
        ),
        (
            "--theme-ribbon-group-label",
            [
                "rgba(217, 119, 6, 0.8)",
                "rgba(217, 119, 6, 0.8)",
                "rgba(217, 119, 6, 0.8)",
                "rgba(96, 165, 250, 0.85)",
                "#FFD700",
            ],
        ),
        (
            "--theme-status-bg",
            ["#36363E", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-status-border",
            ["#27272A", "#27272A", "#1C1917", "#0A1320", "#FFD700"],
        ),
        (
            "--theme-status-text",
            ["#A1A1AA", "#A1A1AA", "#A1A1AA", "#9CA3AF", "#FFFFFF"],
        ),
        (
            "--theme-status-text-label",
            [
                "rgba(161, 161, 170, 0.7)",
                "rgba(161, 161, 170, 0.7)",
                "rgba(161, 161, 170, 0.7)",
                "rgba(156, 163, 175, 0.7)",
                "#FFFFFF",
            ],
        ),
        (
            "--theme-status-hover",
            [
                "rgba(250, 250, 249, 0.1)",
                "rgba(250, 250, 249, 0.1)",
                "rgba(250, 250, 249, 0.1)",
                "rgba(224, 231, 255, 0.10)",
                "rgba(255, 215, 0, 0.25)",
            ],
        ),
        (
            "--theme-status-separator",
            [
                "rgba(250, 250, 249, 0.15)",
                "rgba(250, 250, 249, 0.15)",
                "rgba(250, 250, 249, 0.15)",
                "rgba(224, 231, 255, 0.15)",
                "#FFD700",
            ],
        ),
        (
            "--theme-backstage-item-shortcut",
            [
                "rgba(54, 54, 62, 0.5)",
                "rgba(250, 250, 249, 0.5)",
                "rgba(250, 250, 249, 0.5)",
                "rgba(224, 231, 255, 0.50)",
                "#FFFFFF",
            ],
        ),
        (
            "--theme-backstage-item-shortcut-hover",
            [
                "rgba(217, 119, 6, 0.85)",
                "rgba(217, 119, 6, 0.7)",
                "rgba(217, 119, 6, 0.7)",
                "rgba(96, 165, 250, 0.85)",
                "#FFD700",
            ],
        ),
        (
            "--theme-dialog-overlay",
            [
                "rgba(0, 0, 0, 0.40)",
                "rgba(0, 0, 0, 0.55)",
                "rgba(0, 0, 0, 0.60)",
                "rgba(0, 0, 0, 0.65)",
                "rgba(0, 0, 0, 0.85)",
            ],
        ),
        (
            "--theme-dialog-bg",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-dialog-border",
            [
                "rgba(54, 54, 62, 0.12)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(96, 165, 250, 0.20)",
                "#FFD700",
            ],
        ),
        (
            "--theme-dialog-header-bg",
            ["#F5F5F4", "#44444C", "#36363E", "#1A2C45", "#0A0A0A"],
        ),
        (
            "--theme-dialog-header-text",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-dialog-sidebar-bg",
            ["#F5F5F4", "#44444C", "#36363E", "#1A2C45", "#0A0A0A"],
        ),
        (
            "--theme-dialog-sidebar-border",
            [
                "rgba(54, 54, 62, 0.10)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(96, 165, 250, 0.20)",
                "#FFD700",
            ],
        ),
        (
            "--theme-dialog-tab-text",
            ["#36363E", "#FAFAF9", "#FAFAF9", "#E0E7FF", "#FFFFFF"],
        ),
        (
            "--theme-dialog-tab-hover",
            [
                "rgba(217, 119, 6, 0.08)",
                "rgba(217, 119, 6, 0.1)",
                "rgba(217, 119, 6, 0.1)",
                "rgba(96, 165, 250, 0.10)",
                "rgba(255, 215, 0, 0.20)",
            ],
        ),
        (
            "--theme-dialog-tab-hover-text",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-dialog-tab-active-bg",
            [
                "rgba(217, 119, 6, 0.12)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(96, 165, 250, 0.15)",
                "rgba(255, 215, 0, 0.30)",
            ],
        ),
        (
            "--theme-dialog-tab-active-text",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-dialog-tab-active-accent",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-dialog-content-bg",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-dialog-content-text",
            ["#36363E", "#FAFAF9", "#FAFAF9", "#E0E7FF", "#FFFFFF"],
        ),
        (
            "--theme-dialog-content-secondary",
            [
                "rgba(54, 54, 62, 0.65)",
                "rgba(250, 250, 249, 0.6)",
                "rgba(250, 250, 249, 0.6)",
                "rgba(224, 231, 255, 0.65)",
                "#E5E5E5",
            ],
        ),
        (
            "--theme-dialog-input-bg",
            [
                "#FAFAF9",
                "rgba(217, 119, 6, 0.05)",
                "rgba(217, 119, 6, 0.05)",
                "rgba(96, 165, 250, 0.05)",
                "#0A0A0A",
            ],
        ),
        (
            "--theme-dialog-input-border",
            [
                "rgba(54, 54, 62, 0.15)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(96, 165, 250, 0.20)",
                "#FFD700",
            ],
        ),
        (
            "--theme-dialog-input-text",
            ["#36363E", "#FAFAF9", "#FAFAF9", "#E0E7FF", "#FFFFFF"],
        ),
        (
            "--theme-dialog-btn-bg",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-dialog-btn-text",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-dialog-btn-hover",
            ["#EA580C", "#EA580C", "#EA580C", "#93C5FD", "#FFFF00"],
        ),
        (
            "--theme-dialog-section-border",
            [
                "rgba(54, 54, 62, 0.10)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(217, 119, 6, 0.15)",
                "rgba(96, 165, 250, 0.15)",
                "#FFD700",
            ],
        ),
        (
            "--theme-btn-primary-bg",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-btn-primary-text",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-btn-primary-border",
            ["#D97706", "#D97706", "#D97706", "#60A5FA", "#FFD700"],
        ),
        (
            "--theme-btn-primary-hover-bg",
            ["#EA580C", "#EA580C", "#EA580C", "#93C5FD", "#FFFF00"],
        ),
        (
            "--theme-btn-primary-hover-text",
            ["#FFFFFF", "#36363E", "#27272A", "#0F1B2D", "#000000"],
        ),
        (
            "--theme-btn-secondary-bg",
            [
                "#F5F5F4",
                "rgba(217, 119, 6, 0.1)",
                "rgba(217, 119, 6, 0.1)",
                "rgba(96, 165, 250, 0.10)",
                "#000000",
            ],
        ),
        (
            "--theme-btn-secondary-text",
            ["#36363E", "#FAFAF9", "#FAFAF9", "#E0E7FF", "#FFFFFF"],
        ),
        (
            "--theme-btn-secondary-hover-bg",
            [
                "rgba(217, 119, 6, 0.10)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(217, 119, 6, 0.2)",
                "rgba(96, 165, 250, 0.20)",
                "rgba(255, 215, 0, 0.25)",
            ],
        ),
        (
            "--theme-btn-secondary-hover-border",
            [
                "rgba(217, 119, 6, 0.30)",
                "rgba(217, 119, 6, 0.3)",
                "rgba(217, 119, 6, 0.3)",
                "rgba(96, 165, 250, 0.30)",
                "#FFD700",
            ],
        ),
    ];

    #[derive(Debug, PartialEq)]
    enum Token {
        Color(Color),
        Shadow(Shadow),
    }

    fn color_close(left: Color, right: Color) -> bool {
        [left.r - right.r, left.g - right.g, left.b - right.b, left.a - right.a]
            .into_iter()
            .all(|difference| difference.abs() < 0.000001)
    }

    fn token_close(left: &Token, right: &Token) -> bool {
        match (left, right) {
            (Token::Color(a), Token::Color(b)) => color_close(*a, *b),
            (Token::Shadow(a), Token::Shadow(b)) => {
                color_close(a.color, b.color)
                    && (a.offset.x - b.offset.x).abs() < 0.000001
                    && (a.offset.y - b.offset.y).abs() < 0.000001
                    && (a.blur_radius - b.blur_radius).abs() < 0.000001
            }
            _ => false,
        }
    }

    /// A colour as CSS writes it: `#RRGGBB` or `rgba(r, g, b, a)`.
    fn css_color(value: &str) -> Color {
        if let Some(digits) = value.strip_prefix('#') {
            let value = u32::from_str_radix(digits, 16).unwrap();
            let [_, r, g, b] = value.to_be_bytes();
            return Color::from_rgb8(r, g, b);
        }
        let inner = value
            .strip_prefix("rgba(")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or_else(|| panic!("not a colour: {value}"));
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        let channel = |index: usize| parts[index].parse::<u8>().unwrap();
        Color::from_rgba8(
            channel(0),
            channel(1),
            channel(2),
            parts[3].parse().unwrap(),
        )
    }

    /// A value of `themes.css`: a shadow, `x y blur colour`, or a colour.
    fn css_value(value: &str) -> Token {
        let Some((geometry, color)) = value.split_once(" rgba") else {
            return Token::Color(css_color(value));
        };
        let numbers: Vec<f32> = geometry
            .split_whitespace()
            .map(|number| number.trim_end_matches("px").parse().unwrap())
            .collect();
        assert_eq!(numbers.len(), 3, "{value}");
        Token::Shadow(Shadow {
            color: css_color(&format!("rgba{color}")),
            offset: Vector::new(numbers[0], numbers[1]),
            blur_radius: numbers[2],
        })
    }

    /// The token of a variable of `themes.css`.
    fn token(colors: &UiColors, variable: &str) -> Token {
        let color = Token::Color;
        let shadow = Token::Shadow;
        match variable {
            "--theme-bg" => color(colors.bg),
            "--theme-bg-lighter" => color(colors.bg_lighter),
            "--theme-docbar-bg" => color(colors.docbar_bg),
            "--theme-surface" => color(colors.surface),
            "--theme-border" => color(colors.border),
            "--theme-border-subtle" => color(colors.border_subtle),
            "--theme-text" => color(colors.text),
            "--theme-text-secondary" => color(colors.text_secondary),
            "--theme-text-muted" => color(colors.text_muted),
            "--theme-text-faint" => color(colors.text_faint),
            "--theme-accent" => color(colors.accent),
            "--theme-accent-hover" => color(colors.accent_hover),
            "--theme-accent-text" => color(colors.accent_text),
            "--theme-accent-soft" => color(colors.accent_soft),
            "--theme-accent-tint" => color(colors.accent_tint),
            "--theme-hover" => color(colors.hover),
            "--theme-hover-strong" => color(colors.hover_strong),
            "--theme-active" => color(colors.active),
            "--theme-focus-color" => color(colors.focus),
            "--theme-danger-color" => color(colors.danger),
            "--theme-danger-hover" => color(colors.danger_hover),
            "--theme-dialog-shadow" => shadow(colors.dialog_shadow),
            "--theme-panel-shadow" => shadow(colors.panel_shadow),
            "--theme-popover-shadow" => shadow(colors.popover_shadow),
            "--theme-file-tab-bg" => color(colors.file_tab_bg),
            "--theme-file-tab-hover" => color(colors.file_tab_hover),
            "--theme-file-tab-text" => color(colors.file_tab_text),
            "--theme-content-bg" => color(colors.content_bg),
            "--theme-placeholder-icon" => color(colors.placeholder_icon),
            "--theme-placeholder-heading" => color(colors.placeholder_heading),
            "--theme-placeholder-text" => color(colors.placeholder_text),
            "--theme-ribbon-btn-hover" => color(colors.ribbon_btn_hover),
            "--theme-ribbon-btn-hover-border" => color(colors.ribbon_btn_hover_border),
            "--theme-ribbon-btn-active-bg" => color(colors.ribbon_btn_active_bg),
            "--theme-ribbon-btn-active-border" => color(colors.ribbon_btn_active_border),
            "--theme-ribbon-btn-active-text" => color(colors.ribbon_btn_active_text),
            "--theme-ribbon-icon-active" => color(colors.ribbon_icon_active),
            "--theme-ribbon-text-hover" => color(colors.ribbon_text_hover),
            "--theme-ribbon-group-separator" => color(colors.ribbon_group_separator),
            "--theme-ribbon-group-label" => color(colors.ribbon_group_label),
            "--theme-status-bg" => color(colors.status_bg),
            "--theme-status-border" => color(colors.status_border),
            "--theme-status-text" => color(colors.status_text),
            "--theme-status-text-label" => color(colors.status_text_label),
            "--theme-status-hover" => color(colors.status_hover),
            "--theme-status-separator" => color(colors.status_separator),
            "--theme-backstage-item-shortcut" => color(colors.backstage_item_shortcut),
            "--theme-backstage-item-shortcut-hover" => color(colors.backstage_item_shortcut_hover),
            "--theme-dialog-overlay" => color(colors.dialog_overlay),
            "--theme-dialog-bg" => color(colors.dialog_bg),
            "--theme-dialog-border" => color(colors.dialog_border),
            "--theme-dialog-header-bg" => color(colors.dialog_header_bg),
            "--theme-dialog-header-text" => color(colors.dialog_header_text),
            "--theme-dialog-sidebar-bg" => color(colors.dialog_sidebar_bg),
            "--theme-dialog-sidebar-border" => color(colors.dialog_sidebar_border),
            "--theme-dialog-tab-text" => color(colors.dialog_tab_text),
            "--theme-dialog-tab-hover" => color(colors.dialog_tab_hover),
            "--theme-dialog-tab-hover-text" => color(colors.dialog_tab_hover_text),
            "--theme-dialog-tab-active-bg" => color(colors.dialog_tab_active_bg),
            "--theme-dialog-tab-active-text" => color(colors.dialog_tab_active_text),
            "--theme-dialog-tab-active-accent" => color(colors.dialog_tab_active_accent),
            "--theme-dialog-content-bg" => color(colors.dialog_content_bg),
            "--theme-dialog-content-text" => color(colors.dialog_content_text),
            "--theme-dialog-content-secondary" => color(colors.dialog_content_secondary),
            "--theme-dialog-input-bg" => color(colors.dialog_input_bg),
            "--theme-dialog-input-border" => color(colors.dialog_input_border),
            "--theme-dialog-input-text" => color(colors.dialog_input_text),
            "--theme-dialog-btn-bg" => color(colors.dialog_btn_bg),
            "--theme-dialog-btn-text" => color(colors.dialog_btn_text),
            "--theme-dialog-btn-hover" => color(colors.dialog_btn_hover),
            "--theme-dialog-section-border" => color(colors.dialog_section_border),
            "--theme-btn-primary-bg" => color(colors.btn_primary_bg),
            "--theme-btn-primary-text" => color(colors.btn_primary_text),
            "--theme-btn-primary-border" => color(colors.btn_primary_border),
            "--theme-btn-primary-hover-bg" => color(colors.btn_primary_hover_bg),
            "--theme-btn-primary-hover-text" => color(colors.btn_primary_hover_text),
            "--theme-btn-secondary-bg" => color(colors.btn_secondary_bg),
            "--theme-btn-secondary-text" => color(colors.btn_secondary_text),
            "--theme-btn-secondary-hover-bg" => color(colors.btn_secondary_hover_bg),
            "--theme-btn-secondary-hover-border" => color(colors.btn_secondary_hover_border),
            other => panic!("no token for {other}"),
        }
    }

    #[test]
    fn every_theme_is_its_column_of_the_template() {
        let names: std::collections::BTreeSet<&str> =
            TEMPLATE.iter().map(|(name, _)| *name).collect();
        assert_eq!(names.len(), TEMPLATE.len(), "a variable is listed twice");
        for (column, theme) in UiTheme::ALL.into_iter().enumerate() {
            let colors = theme.colors();
            for (variable, values) in TEMPLATE {
                let actual = token(&colors, variable);
                let expected = css_value(values[column]);
                assert!(
                    token_close(&actual, &expected),
                    "{variable} of {}: {actual:?} != {expected:?}",
                    theme.key(),
                );
            }
        }
    }

    #[test]
    fn brand_tokens_are_light_in_light_and_dark_elsewhere() {
        for theme in UiTheme::ALL {
            let colors = theme.colors();
            let [strong, success, warning, info] = if theme == UiTheme::Light {
                ["#D6D3D1", "#16A34A", "#F59E0B", "#2563EB"]
            } else {
                ["#3F3F46", "#4ADE80", "#FBBF24", "#60A5FA"]
            };
            assert_eq!(colors.border_strong, css_color(strong));
            assert_eq!(colors.success, css_color(success));
            assert_eq!(colors.warning, css_color(warning));
            assert_eq!(colors.info, css_color(info));
            assert_eq!(colors.tooltip_bg, css_color("#36363E"));
            assert_eq!(colors.tooltip_text, css_color("#FAFAF9"));
        }
    }

    #[test]
    fn the_scene_is_white_in_light_and_night_build_in_the_dark_themes() {
        for theme in UiTheme::ALL {
            let dom = theme.colors().dom;
            let (scene, text, label) = if theme == UiTheme::Light {
                ("#FFFFFF", "#36363E", "#36363E")
            } else {
                ("#2A2A32", "#FAFAF9", "#F59E0B")
            };
            assert_eq!(dom.scene, css_color(scene), "{}", theme.key());
            assert_eq!(dom.scene_text, css_color(text), "{}", theme.key());
            assert_eq!(dom.scene_label, css_color(label), "{}", theme.key());
            // The paper is white and lies on the same grey in every theme.
            assert_eq!(dom.paper, Color::WHITE);
            assert_eq!(dom.desk, css_color("#52525B"));
            assert_eq!(dom.scene_badge_bg, css_color("#36363E"));
        }
    }

    #[test]
    fn the_palette_of_iced_follows_the_tokens() {
        for theme in UiTheme::ALL {
            let colors = theme.colors();
            let palette = theme.iced().palette();
            assert_eq!(palette.background, colors.bg);
            assert_eq!(palette.text, colors.text);
            assert_eq!(palette.primary, colors.accent);
            assert_eq!(palette.success, colors.success);
            assert_eq!(palette.danger, colors.danger);
            // A widget finds the tokens of its theme again.
            assert_eq!(super::colors(&theme.iced()).bg, colors.bg);
            assert_eq!(super::colors(&theme.iced()).text, colors.text);
            // The theme choice shows the shell, the accent and the text.
            let swatches = theme.swatches();
            assert_eq!(swatches[0], colors.bg);
            assert_eq!(swatches[2], colors.accent);
            assert_eq!(swatches[3], colors.text);
        }
        assert_eq!(super::colors(&Theme::Dark).bg, LIGHT.bg);
    }

    #[test]
    fn keys_stay_those_of_the_template() {
        let keys: Vec<&str> = UiTheme::ALL.into_iter().map(UiTheme::key).collect();
        assert_eq!(keys, ["light", "forge", "openaec", "blueprint", "contrast"]);
        for theme in UiTheme::ALL {
            assert_eq!(UiTheme::from_key(theme.key()), Some(theme));
        }
        assert_eq!(UiTheme::from_key("night"), Some(UiTheme::OpenAec));
        assert_eq!(UiTheme::from_key("dark"), None);
        // The tests have no folder of settings: the default is Light.
        assert_eq!(UiTheme::load(), UiTheme::Light);
    }

    #[test]
    fn names_are_those_of_the_template_in_both_languages() {
        let _language = TestLanguage::hold(Language::English);
        let names = |language| {
            crate::i18n::set(language);
            UiTheme::ALL
                .into_iter()
                .map(|theme| crate::i18n::tr(&theme.to_string()).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(Language::English),
            [
                "Light",
                "Forge (dark)",
                "OpenAEC (dark)",
                "Blueprint",
                "High contrast"
            ]
        );
        assert_eq!(
            names(Language::Table(0)),
            [
                "Licht",
                "Forge (donker)",
                "OpenAEC (donker)",
                "Blueprint",
                "Hoog contrast"
            ]
        );
    }

    #[test]
    fn shared_modal_shadow_matches_the_pinned_pointcloud_tokens() {
        for theme in UiTheme::ALL {
            assert_eq!(theme.shared().dialog_shadow(), theme.colors().dialog_shadow,
                "{} modal shadow", theme.key());
        }
    }
}
