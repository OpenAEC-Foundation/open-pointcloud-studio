use crate::{icons::Icon, Language, Theme};
use iced::widget::pane_grid;
use serde::{Deserialize, Serialize};

/// Fixed dimensions shared by every application, in logical pixels.
pub mod metrics {
    pub const TITLE: u16 = 32;
    pub const TABS: u16 = 28;
    pub const RIBBON: u16 = 94;
    pub const DOCUMENTS: u16 = 30;
    pub const STATUS: u16 = 22;
    pub const PANEL_DEFAULT: u16 = 240;
    pub const PANEL_NARROW_MIN: u16 = 190;
    pub const PROPERTY_PANEL_NARROW_MIN: u16 = 210;
    pub const WORKSPACE_NARROW_MIN: u16 = 300;
    pub const PANEL_TOOLBAR: u16 = 24;
    pub const WINDOW_DEFAULT_WIDTH: u16 = 1280;
    pub const WINDOW_MIN_WIDTH: u16 = 720;
    pub const CONTROL: u16 = 30;
    pub const ICON: u16 = 20;
    pub const GAP: u16 = 8;
}

#[derive(Debug, Clone)]
pub struct Command<M> {
    pub label: String,
    pub icon: CommandIcon,
    pub message: M,
    pub enabled: bool,
    pub selected: bool,
    pub shortcut: Option<String>,
    pub control: Option<RibbonControl<M>>,
}
impl<M> Command<M> {
    pub fn new(label: impl Into<String>, icon: impl Into<CommandIcon>, message: M) -> Self {
        Self { label: label.into(), icon: icon.into(), message, enabled: true, selected: false, shortcut: None, control: None }
    }
    pub fn enabled(mut self, value: bool) -> Self { self.enabled = value; self }
    pub fn selected(mut self, value: bool) -> Self { self.selected = value; self }
    pub fn shortcut(mut self, value: impl Into<String>) -> Self { self.shortcut = Some(value.into()); self }
    pub fn control(mut self, control: RibbonControl<M>) -> Self { self.control = Some(control); self }
}
/// Input widgets that fit inside the same 94 px ribbon-group body as commands.
/// The caller owns values and receives changes through its application message.
#[derive(Debug, Clone)]
pub enum RibbonControl<M> {
    Slider { minimum: f32, maximum: f32, value: f32, step: f32, unit: &'static str, on_change: fn(f32) -> M },
    Range { minimum: f32, maximum: f32, lower: f32, upper: f32, step: f32, unit: &'static str,
        on_lower: fn(f32) -> M, on_upper: fn(f32) -> M },
    Segmented { options: Vec<(String, M)>, selected: usize },
    Select { options: Vec<(String, M)>, selected: usize, width: f32, show_label: bool },
    Sliders { items: Vec<RibbonSlider<M>>, minimum: f32, maximum: f32, step: f32, unit: &'static str, width: f32 },
    Number { value: String, on_change: fn(String) -> M, width: f32, show_label: bool },
    FormStack { fields: Vec<RibbonFormField<M>>, width: f32, label_width: f32 },
    ColorSelect { options: Vec<(String, iced::Color, M)>, selected: usize,
        current: iced::Color, width: f32, rows_per_column: usize,
        open: bool, on_toggle: M, on_close: M, none: Option<(String, M)>,
        custom: Option<RibbonCustomColor<M>> },
    StyleGallery { items: Vec<RibbonSwatch<M>>, selected: usize, visible: usize,
        open: bool, on_toggle: M, on_close: M },
}
/// Optional application-owned custom color editor inside a ribbon palette.
#[derive(Debug, Clone)]
pub struct RibbonCustomColor<M> {
    pub label: String,
    pub editing: bool,
    pub draft: String,
    pub valid: bool,
    pub on_toggle: M,
    pub on_input: fn(String) -> M,
    pub on_apply: M,
}
/// A compact labelled row inside a ribbon form stack.
#[derive(Debug, Clone)]
pub enum RibbonFormField<M> {
    Number { label: String, value: String, on_change: fn(String) -> M },
    Select { label: String, options: Vec<(String, M)>, selected: usize },
    Slider { label: String, minimum: f32, maximum: f32, value: f32, step: f32,
        value_text: String, unit: &'static str, on_change: fn(f32) -> M,
        on_input: fn(String) -> M },
}
impl<M> RibbonControl<M> {
    pub fn width(&self) -> f32 {
        match self {
            Self::Select { width, .. } | Self::Sliders { width, .. } | Self::Number { width, .. }
                | Self::FormStack { width, .. }
                | Self::ColorSelect { width, .. } => *width,
            Self::StyleGallery { items, visible, .. } => items.len().min(*visible) as f32 * 52.0 + 30.0,
            _ => 170.0,
        }
    }
}
/// A labelled style sample. Its colour and meaning are supplied by the app.
#[derive(Debug, Clone)]
pub struct RibbonSwatch<M> {
    pub label: String,
    pub color: iced::Color,
    pub background: iced::Color,
    pub dashed_border: bool,
    pub message: M,
}
#[derive(Debug, Clone)]
pub struct RibbonSlider<M> {
    pub label: String,
    pub icon: CommandIcon,
    pub value: f32,
    pub on_change: fn(f32) -> M,
}
/// A command may use a canonical icon or an application-owned SVG.
/// Custom vectors do not become part of `Icon::ALL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandIcon {
    Canonical(Icon),
    Domain(crate::icons::DomainIcon),
    Svg(&'static [u8]),
}
impl From<Icon> for CommandIcon {
    fn from(icon: Icon) -> Self { Self::Canonical(icon) }
}
impl From<crate::icons::DomainIcon> for CommandIcon {
    fn from(icon: crate::icons::DomainIcon) -> Self { Self::Domain(icon) }
}
impl CommandIcon {
    pub fn view<'a>(self, size: u16, color: iced::Color) -> iced::widget::svg::Svg<'a> {
        match self {
            Self::Canonical(icon) => icon.view(size, color),
            Self::Domain(icon) => icon.view(size, color),
            Self::Svg(bytes) => iced::widget::svg(iced::widget::svg::Handle::from_memory(bytes))
                .width(size).height(size)
                .style(move |_, _| iced::widget::svg::Style { color: Some(color) }),
        }
    }
}
#[derive(Debug, Clone)]
pub struct RibbonGroup<M> { pub label: String, pub commands: Vec<Command<M>> }
#[derive(Debug, Clone)]
pub struct RibbonTab<M> { pub id: String, pub label: String, pub groups: Vec<RibbonGroup<M>> }
#[derive(Debug, Clone)]
pub struct Document { pub id: String, pub title: String, pub dirty: bool }
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentEvent { Activate(String), CloseRequested(String) }
#[derive(Debug, Clone)]
pub enum Message<M> { Ui(UiEvent), Command(M), Document(DocumentEvent) }
#[derive(Debug, Clone)]
pub enum UiEvent {
    Tab(String), Backstage(bool), Settings(bool), SettingsSave, SettingsResetPrompt(bool), SettingsReset,
    SettingsEscape, SettingsPage(SettingsPage), SettingsThemeMenu(bool), Theme(Theme), Language(Language),
    Scale(f64), ToggleLeft, ToggleRight, Resize(pane_grid::ResizeEvent), WindowWidth(f32),
    DragWindow, Minimize, Maximize, CloseWindow,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SettingsPage { #[default] General, Appearance, About }
#[derive(Debug, Clone, Copy)]
struct SettingsSnapshot { theme: Theme, language: Language, scale: f64 }
impl From<&Preferences> for SettingsSnapshot {
    fn from(preferences: &Preferences) -> Self {
        Self { theme: preferences.theme, language: preferences.language, scale: preferences.scale }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region { Explorer, Workspace, Properties }

/// The same tokens and window bands can host a ribbon or a guided workflow.
/// A workflow puts its navigation in the explorer slot and has no tab strip,
/// ribbon, or permanent properties panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChromeMode { #[default] Ribbon, RibbonSidebar, RibbonWorkspace, ThreePaneRibbon { left: bool, right: bool }, ThreePaneFixed { left_width: u16, right_width: u16 }, Plain, Workflow, Sidebar, SidebarCompact, TopNav, CanvasRibbon }

/// Persisted settings only; document contents remain owned by the application.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub theme: Theme,
    pub language: Language,
    pub scale: f64,
    pub explorer_visible: bool,
    pub properties_visible: bool,
    pub explorer_ratio: f32,
    pub properties_ratio: f32,
}
impl Default for Preferences {
    fn default() -> Self {
        Self { theme: Theme::Light, language: Language::Dutch, scale: 1.0,
            explorer_visible: true, properties_visible: true,
            explorer_ratio: metrics::PANEL_DEFAULT as f32 / metrics::WINDOW_DEFAULT_WIDTH as f32,
            properties_ratio: (metrics::WINDOW_DEFAULT_WIDTH - 2 * metrics::PANEL_DEFAULT) as f32
                / (metrics::WINDOW_DEFAULT_WIDTH - metrics::PANEL_DEFAULT) as f32 }
    }
}
impl Preferences {
    pub fn normalize(&mut self) {
        self.scale = if self.scale.is_finite() { self.scale.clamp(0.75, 2.0) } else { 1.0 };
        self.explorer_ratio = if self.explorer_ratio.is_finite() { self.explorer_ratio.clamp(0.1, 0.4) } else { 0.2 };
        self.properties_ratio = if self.properties_ratio.is_finite() { self.properties_ratio.clamp(0.5, 0.9) } else { 0.75 };
    }
    pub fn load(path: &std::path::Path) -> Result<Self, Box<dyn std::error::Error>> {
        let mut preferences: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        preferences.normalize(); Ok(preferences)
    }
    pub fn save(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(parent)?; }
        let mut preferences = self.clone(); preferences.normalize();
        // Write the complete payload before replacing the old settings.
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_vec_pretty(&preferences)?)?;
        std::fs::rename(temp, path)?; Ok(())
    }
}

pub struct ShellState {
    pub preferences: Preferences,
    pub active_tab: String,
    pub backstage: bool,
    pub settings: bool,
    pub settings_page: SettingsPage,
    pub settings_theme_open: bool,
    pub settings_reset_confirm: bool,
    settings_original: Option<SettingsSnapshot>,
    pub(crate) panes: pane_grid::State<Region>,
    left_split: Option<pane_grid::Split>,
    right_split: Option<pane_grid::Split>,
    window_width: f32,
}
impl Default for ShellState { fn default() -> Self { Self::new(Preferences::default()) } }
impl ShellState {
    pub fn new(mut preferences: Preferences) -> Self {
        preferences.normalize();
        let (panes, _) = pane_grid::State::new(Region::Workspace);
        let mut state = Self { preferences, active_tab: String::new(), backstage:false, settings:false, settings_page:SettingsPage::General, settings_theme_open:false, settings_reset_confirm:false, settings_original:None, panes, left_split:None, right_split:None, window_width: metrics::WINDOW_DEFAULT_WIDTH as f32 };
        state.rebuild(); state
    }
    /// Keep side panel labels readable when the window is narrowed, without
    /// changing the ratios the user saved for a wider window.
    pub fn set_window_width(&mut self, width: f32) {
        if width.is_finite() && width >= metrics::WINDOW_MIN_WIDTH as f32 && (self.window_width - width).abs() >= 0.5 {
            self.window_width = width;
            self.rebuild();
        }
    }
    fn rebuild(&mut self) {
        let width = self.window_width;
        let minimum = metrics::PANEL_NARROW_MIN as f32;
        let properties_minimum = metrics::PROPERTY_PANEL_NARROW_MIN as f32;
        let workspace_minimum = metrics::WORKSPACE_NARROW_MIN as f32;
        let left = if self.preferences.explorer_visible {
            (width * self.preferences.explorer_ratio).max(minimum).min(
                width - workspace_minimum - if self.preferences.properties_visible { properties_minimum } else { 0.0 }
            )
        } else { 0.0 };
        let remaining = width - left;
        let right = if self.preferences.properties_visible {
            (remaining * (1.0 - self.preferences.properties_ratio))
                .max(properties_minimum).min(remaining - workspace_minimum)
        } else { 0.0 };
        let (mut panes, first) = pane_grid::State::new(if self.preferences.explorer_visible { Region::Explorer } else { Region::Workspace });
        self.left_split = None; self.right_split = None;
        let workspace = if self.preferences.explorer_visible {
            let (workspace, split) = panes.split(pane_grid::Axis::Vertical, first, Region::Workspace).expect("existing pane");
            panes.resize(split, left / width); self.left_split = Some(split); workspace
        } else { first };
        if self.preferences.properties_visible {
            let (_, split) = panes.split(pane_grid::Axis::Vertical, workspace, Region::Properties).expect("existing pane");
            panes.resize(split, 1.0 - right / remaining); self.right_split = Some(split);
        }
        self.panes = panes;
    }
    fn cancel_settings(&mut self) {
        if let Some(original) = self.settings_original.take() {
            self.preferences.theme = original.theme;
            self.preferences.language = original.language;
            self.preferences.scale = original.scale;
        }
        self.settings = false;
        self.settings_theme_open = false;
        self.settings_reset_confirm = false;
    }
    /// Apply shell events. Window actions are handled by the shared runtime.
    pub fn update(&mut self, event: &UiEvent) {
        match event {
            UiEvent::Tab(id) => { self.active_tab = id.clone(); self.backstage = false; }
            UiEvent::Backstage(open) => { self.cancel_settings(); self.backstage = *open; }
            UiEvent::Settings(true) => {
                if !self.settings { self.settings_original = Some(SettingsSnapshot::from(&self.preferences)); }
                self.settings = true; self.backstage = false; self.settings_theme_open = false;
            }
            UiEvent::Settings(false) => self.cancel_settings(),
            UiEvent::SettingsSave => {
                self.settings_original = None; self.settings = false;
                self.settings_theme_open = false; self.settings_reset_confirm = false;
            }
            UiEvent::SettingsResetPrompt(open) => {
                self.settings_reset_confirm = *open && self.settings;
                if self.settings_reset_confirm { self.settings_theme_open = false; }
            }
            UiEvent::SettingsReset if self.settings && self.settings_reset_confirm => {
                let defaults = Preferences::default();
                self.preferences.theme = defaults.theme;
                self.preferences.language = defaults.language;
                self.preferences.scale = defaults.scale;
                self.settings_reset_confirm = false;
            }
            UiEvent::SettingsReset => {}
            UiEvent::SettingsEscape => {
                if self.settings_reset_confirm { self.settings_reset_confirm = false; }
                else { self.cancel_settings(); }
            }
            UiEvent::SettingsPage(page) => { self.settings_page = *page; self.settings_theme_open = false; }
            UiEvent::SettingsThemeMenu(open) => self.settings_theme_open = *open,
            UiEvent::Theme(theme) => { self.preferences.theme = *theme; self.settings_theme_open = false; }
            UiEvent::Language(language) => self.preferences.language = *language,
            UiEvent::Scale(scale) => { self.preferences.scale = *scale; self.preferences.normalize(); }
            UiEvent::ToggleLeft => { self.preferences.explorer_visible = !self.preferences.explorer_visible; self.rebuild(); }
            UiEvent::ToggleRight => { self.preferences.properties_visible = !self.preferences.properties_visible; self.rebuild(); }
            UiEvent::Resize(event) => {
                let ratio = if Some(event.split) == self.left_split { event.ratio.clamp(0.1,0.4) } else { event.ratio.clamp(0.5,0.9) };
                self.panes.resize(event.split, ratio);
                if Some(event.split) == self.left_split { self.preferences.explorer_ratio = ratio; }
                if Some(event.split) == self.right_split { self.preferences.properties_ratio = ratio; }
            }
            UiEvent::WindowWidth(width) => self.set_window_width(*width),
            _ => {}
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn narrow_window_keeps_panels_legible_and_restores_saved_ratios() {
        let mut shell = ShellState::default();
        let saved = (shell.preferences.explorer_ratio, shell.preferences.properties_ratio);
        let pane_widths = |shell: &ShellState, width: f32| {
            let regions = shell.panes.layout().pane_regions(2.0, iced::Size::new(width, 480.0));
            shell.panes.iter().map(|(pane, region)| (*region, regions[pane].width)).collect::<Vec<_>>()
        };
        shell.set_window_width(720.0);
        let narrow = pane_widths(&shell, 720.0);
        assert!(narrow.iter().any(|(region, width)| *region == Region::Explorer && *width >= 185.0));
        assert!(narrow.iter().any(|(region, width)| *region == Region::Properties && *width >= 205.0));
        assert!(narrow.iter().any(|(region, width)| *region == Region::Workspace && *width >= 300.0));
        assert_eq!((shell.preferences.explorer_ratio, shell.preferences.properties_ratio), saved);

        shell.set_window_width(1280.0);
        let wide = pane_widths(&shell, 1280.0);
        for region in [Region::Explorer, Region::Properties] {
            assert!(wide.iter().any(|(item, width)| *item == region && (*width - 240.0).abs() < 5.0));
        }
    }

    #[test]
    fn theme_choice_closes_its_menu_and_other_pages_do_not_leave_it_open() {
        let mut shell = ShellState::default();
        shell.update(&UiEvent::Settings(true));
        shell.update(&UiEvent::SettingsPage(SettingsPage::Appearance));
        shell.update(&UiEvent::SettingsThemeMenu(true));
        assert!(shell.settings_theme_open);
        shell.update(&UiEvent::Theme(Theme::Forge));
        assert_eq!(shell.preferences.theme, Theme::Forge);
        assert!(!shell.settings_theme_open);
        shell.update(&UiEvent::SettingsThemeMenu(true));
        shell.update(&UiEvent::SettingsPage(SettingsPage::About));
        assert!(!shell.settings_theme_open);
    }

    #[test]
    fn preview_cancel_and_save_follow_the_template_settings_flow() {
        let mut shell = ShellState::default();
        shell.update(&UiEvent::Settings(true));
        shell.update(&UiEvent::Theme(Theme::Forge));
        shell.update(&UiEvent::Language(Language::English));
        shell.update(&UiEvent::Scale(1.5));
        assert_eq!(shell.preferences.theme, Theme::Forge);
        shell.update(&UiEvent::Settings(false));
        assert_eq!(shell.preferences.theme, Theme::Light);
        assert_eq!(shell.preferences.language, Language::Dutch);
        assert_eq!(shell.preferences.scale, 1.0);

        shell.update(&UiEvent::Settings(true));
        shell.update(&UiEvent::Theme(Theme::Forge));
        shell.update(&UiEvent::SettingsSave);
        assert_eq!(shell.preferences.theme, Theme::Forge);
        assert!(!shell.settings);
    }

    #[test]
    fn reset_needs_confirmation_and_escape_closes_only_the_top_dialog() {
        let mut shell = ShellState::default();
        shell.preferences.theme = Theme::Forge;
        shell.update(&UiEvent::Settings(true));
        shell.update(&UiEvent::SettingsReset);
        assert_eq!(shell.preferences.theme, Theme::Forge);
        shell.update(&UiEvent::SettingsThemeMenu(true));
        shell.update(&UiEvent::SettingsResetPrompt(true));
        assert!(!shell.settings_theme_open);
        shell.update(&UiEvent::SettingsEscape);
        assert!(shell.settings);
        assert!(!shell.settings_reset_confirm);
        assert_eq!(shell.preferences.theme, Theme::Forge);
        shell.update(&UiEvent::SettingsResetPrompt(true));
        shell.update(&UiEvent::SettingsReset);
        assert_eq!(shell.preferences.theme, Theme::Light);
        shell.update(&UiEvent::SettingsEscape);
        assert_eq!(shell.preferences.theme, Theme::Forge);
        assert!(!shell.settings);
    }
}
