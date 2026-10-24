//! UI 0.4 components, drawn only with [`super::tokens`].
//!
//! Each function follows `design/ontwerp-0.4/04-componenten.md`. Icons are SVG
//! sources in the B2 format (`currentColor` for the main colour and
//! `var(--icon-accent, currentColor)` for the accent); [`glyph`] recolours them
//! per state. Two parts of the spec need more than iced 0.13 offers and are
//! approximated: focus is drawn as a 2 px border inside the control instead of
//! an outer ring, and High contrast draws disabled borders solid, not dashed.
use super::{font, hex, mix, radius, shell, Look, ShadowLayer, Shadows};
use crate::fonts;
use iced::alignment::{Horizontal, Vertical};
use iced::border::Radius;
use iced::widget::{
    button as iced_button, checkbox as iced_checkbox, column, container, pick_list, progress_bar as iced_progress,
    mouse_area, radio as iced_radio, row, slider as iced_slider, stack, svg, text, text_input, toggler as iced_toggler, tooltip, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Shadow};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Height and inner spacing of controls (besluit 1: 28 px, 24 px compact).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Density {
    #[default]
    Regular,
    Compact,
}

impl Density {
    pub fn height(self) -> f32 {
        match self {
            Self::Regular => 28.0,
            Self::Compact => 24.0,
        }
    }
    fn pad_x(self) -> f32 {
        match self {
            Self::Regular => 12.0,
            Self::Compact => 8.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// The main action; at most one per screen or dialog.
    Primary,
    Secondary,
    /// No surface and no border, such as "Niet opslaan".
    Quiet,
    /// Delete and other irreversible actions.
    Danger,
}

/// Status of a notification. Colour is never the only signal: each tone has a word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Error,
    Warning,
    Success,
    Info,
}

impl Tone {
    pub fn color(self, look: Look) -> Color {
        let c = look.color;
        match self {
            Self::Error => c.error,
            Self::Warning => c.warning,
            Self::Success => c.success,
            Self::Info => c.info,
        }
    }
    pub fn tint(self, look: Look) -> Color {
        let c = look.color;
        match self {
            Self::Error => c.error_tint,
            Self::Warning => c.warning_tint,
            Self::Success => c.success_tint,
            Self::Info => c.info_tint,
        }
    }
    /// The Dutch word in front of an in-page notification.
    pub fn word(self) -> &'static str {
        match self {
            Self::Error => "Fout.",
            Self::Warning => "Let op.",
            Self::Success => "Gelukt.",
            Self::Info => "Info.",
        }
    }
}

pub(crate) fn border(color: Color, width: f32, r: f32) -> Border {
    Border { color, width, radius: r.into() }
}

pub(crate) fn fill(color: Color) -> Option<Background> {
    Some(Background::Color(color))
}

// ---------- Icons and text ----------

/// A B2 icon source drawn at `size` with its main and accent colour.
///
/// Recoloured handles are cached per source and colour pair, so a view rebuild
/// neither recolours nor rehashes the SVG and the renderer keeps its raster cache.
/// Sources are `'static`, so their address identifies them.
pub fn glyph<'a, M: 'a>(source: &'static str, size: f32, color: Color, accent: Color) -> Element<'a, M> {
    svg(glyph_handle(source, color, accent)).width(size).height(size).into()
}

fn glyph_handle(source: &'static str, color: Color, accent: Color) -> svg::Handle {
    type Key = (usize, usize, [u8; 4], [u8; 4]);
    static CACHE: OnceLock<Mutex<HashMap<Key, svg::Handle>>> = OnceLock::new();
    let key = (source.as_ptr() as usize, source.len(), color.into_rgba8(), accent.into_rgba8());
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache
        .entry(key)
        .or_insert_with(|| {
            let svg_text = source
                .replace("var(--icon-accent,currentColor)", &hex(accent))
                .replace("var(--icon-accent, currentColor)", &hex(accent))
                .replace("currentColor", &hex(color));
            svg::Handle::from_memory(svg_text.into_bytes())
        })
        .clone()
}

/// Body text, 12 px.
pub fn body<'a>(look: Look, content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).size(font::SIZE_BODY).font(fonts::BODY).color(look.color.text)
}

/// Secondary text, 12 px.
pub fn secondary<'a>(look: Look, content: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(content).size(font::SIZE_BODY).font(fonts::BODY).color(look.color.text_secondary)
}

/// Mono label: JetBrains Mono, 11 px, uppercase. iced 0.13 has no letter spacing.
pub fn mono_label<'a>(look: Look, content: &str) -> text::Text<'a> {
    text(content.to_uppercase()).size(font::SIZE_LABEL).font(fonts::MONO).color(look.color.text_secondary)
}

/// Heading in Space Grotesk, for dialogs (16 px) and pages (22 px).
pub fn heading<'a>(look: Look, content: impl text::IntoFragment<'a>, size: f32) -> text::Text<'a> {
    text(content).size(size).font(fonts::HEADING).color(look.color.text)
}

/// A 1 px line in `border`.
pub fn divider<'a, M: 'a>(look: Look) -> Element<'a, M> {
    let color = look.color.border;
    container(Space::new(Length::Fill, 1)).height(1).style(move |_| container::Style { background: fill(color), ..Default::default() }).into()
}

fn vline<'a, M: 'a>(color: Color) -> Element<'a, M> {
    container(Space::new(1, Length::Fill)).width(1).style(move |_| container::Style { background: fill(color), ..Default::default() }).into()
}

pub(crate) fn bar<'a, M: 'a>(color: Color, width: impl Into<Length>, height: impl Into<Length>) -> Element<'a, M> {
    let (width, height) = (width.into(), height.into());
    container(Space::new(width, height))
        .width(width)
        .height(height).style(move |_| container::Style { background: fill(color), ..Default::default() }).into()
}

// ---------- Surfaces ----------

/// Style for a raised surface (menu, tooltip, toast, dialog) with the theme shadow.
/// A ring layer without blur, as in High contrast, becomes the border.
pub fn surface(look: Look, layers: &[ShadowLayer], r: f32) -> container::Style {
    let ring = layers.iter().find(|l| !l.inset && l.blur == 0.0 && l.spread > 0.0);
    container::Style {
        text_color: Some(look.color.text),
        background: fill(look.color.raised),
        border: match ring {
            Some(l) => border(l.color, l.spread, r),
            None => border(look.color.border, 1.0, r),
        },
        shadow: Shadows::drop_shadow(layers).unwrap_or_default(),
    }
}

/// Panel surface: `panel` with the panel shadow.
pub fn panel_style(look: Look) -> container::Style {
    container::Style {
        text_color: Some(look.color.text),
        background: fill(look.color.panel),
        border: border(look.color.border, 1.0, 0.0),
        shadow: Shadows::drop_shadow(look.tokens.shadow.panel).unwrap_or_default(),
    }
}

// ---------- Buttons ----------

pub fn button_style(look: Look, kind: ButtonKind, status: iced_button::Status) -> iced_button::Style {
    use iced_button::Status as S;
    let c = look.color;
    let hc = look.high_contrast();
    let r = radius::MEDIUM;
    if status == S::Disabled {
        return iced_button::Style {
            background: match kind {
                ButtonKind::Primary => fill(c.hover),
                ButtonKind::Secondary => fill(c.raised),
                _ => None,
            },
            text_color: c.text_disabled,
            border: match kind {
                ButtonKind::Quiet => Border::default(),
                _ => border(if hc { c.text_disabled } else { c.border }, 1.0, r),
            },
            shadow: Shadow::default(),
        };
    }
    let hovered = status == S::Hovered;
    let pressed = status == S::Pressed;
    let hc_ring = if hc && (hovered || pressed) { Some(border(c.accent, 2.0, r)) } else { None };
    match kind {
        ButtonKind::Primary => iced_button::Style {
            background: fill(if pressed { c.accent_pressed } else if hovered { c.accent_hover } else { c.accent }),
            text_color: c.text_on_accent,
            border: border(Color::TRANSPARENT, 0.0, r),
            shadow: Shadow::default(),
        },
        ButtonKind::Secondary => iced_button::Style {
            background: fill(if pressed { c.pressed } else if hovered { c.hover } else { c.raised }),
            text_color: c.text,
            border: hc_ring.unwrap_or(border(c.border_strong, 1.0, r)),
            shadow: Shadow::default(),
        },
        ButtonKind::Quiet => iced_button::Style {
            background: if pressed { fill(c.pressed) } else if hovered { fill(c.hover) } else { None },
            text_color: c.text,
            border: hc_ring.unwrap_or(border(Color::TRANSPARENT, 0.0, r)),
            shadow: Shadow::default(),
        },
        ButtonKind::Danger => {
            let (h, p) = if look.tokens.dark { (0.06, 0.10) } else { (0.08, 0.16) };
            let tint = |a| if hc { None } else { fill(mix(c.error, a, c.panel)) };
            iced_button::Style {
                background: if pressed { tint(p) } else if hovered { tint(h) } else { None },
                text_color: c.error,
                border: border(c.error, if hc && (hovered || pressed) { 2.0 } else { 1.0 }, r),
                shadow: Shadow::default(),
            }
        }
    }
}

/// A text button, with an optional 16 px icon on the left. `None` disables it.
pub fn button<'a, M: Clone + 'a>(
    look: Look,
    label: &'a str,
    kind: ButtonKind,
    density: Density,
    icon: Option<&'static str>,
    on_press: Option<M>,
) -> Element<'a, M> {
    let c = look.color;
    let fg = if on_press.is_none() {
        c.text_disabled
    } else {
        match kind {
            ButtonKind::Primary => c.text_on_accent,
            ButtonKind::Danger => c.error,
            _ => c.text,
        }
    };
    let accent = if kind == ButtonKind::Secondary || kind == ButtonKind::Quiet { c.accent_text } else { fg };
    let mut content = row![].spacing(6).align_y(Alignment::Center);
    if let Some(source) = icon {
        content = content.push(glyph(source, 16.0, fg, if on_press.is_none() { fg } else { accent }));
    }
    content = content.push(text(label).size(font::SIZE_BODY).font(fonts::STRONG));
    iced_button(container(content).height(Length::Fill).align_y(Vertical::Center))
        .height(density.height())
        .padding(Padding::from([0.0, density.pad_x()]))
        .on_press_maybe(on_press)
        .style(move |_, status| button_style(look, kind, status))
        .into()
}

/// A square icon button: 28 px, or 20 px in a panel header. The icon is 16 px.
/// `selected` is the on state of a toggle.
pub fn icon_button<'a, M: Clone + 'a>(
    look: Look,
    icon: &'static str,
    size: f32,
    selected: bool,
    on_press: Option<M>,
) -> Element<'a, M> {
    let c = look.color;
    let disabled = on_press.is_none();
    let fg = if disabled { c.text_disabled } else if selected { c.accent_text } else { c.text };
    let accent = if disabled { fg } else { c.accent_text };
    iced_button(container(glyph(icon, 16.0, fg, accent)).center(Length::Fill))
        .width(size)
        .height(size)
        .padding(0)
        .on_press_maybe(on_press)
        .style(move |_, status| {
            use iced_button::Status as S;
            let hc = look.high_contrast();
            let r = radius::SMALL;
            let active = status == S::Hovered || status == S::Pressed;
            let background = match status {
                S::Disabled => None,
                _ if selected && !hc => fill(c.selected),
                S::Pressed if !hc => fill(c.pressed),
                S::Hovered if !hc => fill(c.hover),
                _ => None,
            };
            let edge = if selected {
                border(c.selected_bar, if hc { 2.0 } else { 1.0 }, r)
            } else if hc && active {
                border(c.accent, 1.0, r)
            } else {
                border(Color::TRANSPARENT, 0.0, r)
            };
            iced_button::Style { background, text_color: fg, border: edge, shadow: Shadow::default() }
        })
        .into()
}

/// Wraps `content` in a tooltip with the theme colours: name and shortcut in mono.
pub fn with_tooltip<'a, M: 'a>(look: Look, content: Element<'a, M>, name: &'a str, shortcut: Option<&'a str>) -> Element<'a, M> {
    let mut tip = row![text(name).size(font::SIZE_LABEL).font(fonts::BODY).color(look.color.text)].spacing(8);
    if let Some(keys) = shortcut {
        tip = tip.push(text(keys).size(font::SIZE_LABEL).font(fonts::MONO).color(look.color.text_secondary));
    }
    let tip = container(tip).padding([4, 8]).style(move |_| surface(look, look.tokens.shadow.popup, radius::SMALL));
    tooltip(content, tip, tooltip::Position::Bottom).gap(4).into()
}

// ---------- Fields ----------

fn input_style(look: Look, status: text_input::Status, invalid: bool) -> text_input::Style {
    use text_input::Status as S;
    let c = look.color;
    let r = radius::MEDIUM;
    let edge = match status {
        S::Disabled => border(if look.high_contrast() { c.text_disabled } else { c.border }, 1.0, r),
        _ if invalid => border(c.error, 2.0, r),
        S::Focused => border(c.focus, 2.0, r),
        S::Hovered => border(c.text, 1.0, r),
        S::Active => border(c.border_strong, 1.0, r),
    };
    text_input::Style {
        background: Background::Color(if status == S::Disabled { c.panel } else { c.raised }),
        border: edge,
        icon: c.text_secondary,
        placeholder: if status == S::Disabled { c.text_disabled } else { c.text_secondary },
        value: if status == S::Disabled { c.text_disabled } else { c.text },
        selection: mix(c.accent, 0.35, c.raised),
    }
}

/// A text field with an optional error message of 11 px below it.
/// Without `on_input` the field is disabled.
pub fn text_field<'a, M: Clone + 'a>(
    look: Look,
    placeholder: &'a str,
    value: &'a str,
    on_input: Option<impl Fn(String) -> M + 'a>,
    density: Density,
    error: Option<&'a str>,
) -> Element<'a, M> {
    let pad_y = (density.height() - 16.0) / 2.0;
    let invalid = error.is_some();
    let field = text_input(placeholder, value)
        .on_input_maybe(on_input)
        .size(font::SIZE_BODY)
        .font(fonts::BODY)
        .padding(Padding::from([pad_y, 8.0]))
        .style(move |_, status| input_style(look, status, invalid));
    match error {
        Some(message) => column![field, text(message).size(font::SIZE_LABEL).font(fonts::BODY).color(look.color.error)].spacing(4).into(),
        None => field.into(),
    }
}

/// A select list with the arrow on the right and a menu with a 10 px corner.
pub fn select<'a, T, M>(
    look: Look,
    options: &'a [T],
    selected: Option<T>,
    on_select: impl Fn(T) -> M + 'a,
    density: Density,
) -> Element<'a, M>
where
    T: ToString + PartialEq + Clone + 'a,
    M: Clone + 'a,
{
    let c = look.color;
    let pad_y = (density.height() - 16.0) / 2.0;
    pick_list(options, selected, on_select)
        .width(Length::Fill)
        .text_size(font::SIZE_BODY)
        .font(fonts::BODY)
        .padding(Padding::from([pad_y, 8.0]))
        .style(move |_, status| {
            use pick_list::Status as S;
            pick_list::Style {
                text_color: c.text,
                placeholder_color: c.text_secondary,
                handle_color: c.text_secondary,
                background: Background::Color(c.raised),
                border: match status {
                    S::Opened => border(c.focus, 2.0, radius::MEDIUM),
                    S::Hovered => border(c.text, 1.0, radius::MEDIUM),
                    S::Active => border(c.border_strong, 1.0, radius::MEDIUM),
                },
            }
        })
        .menu_style(move |_| {
            let s = surface(look, look.tokens.shadow.popup, radius::LARGE);
            iced::overlay::menu::Style {
                background: Background::Color(c.raised),
                border: s.border,
                text_color: c.text,
                selected_text_color: c.text,
                selected_background: Background::Color(if look.high_contrast() { c.hover } else { c.selected }),
            }
        })
        .into()
}

// ---------- Choices ----------

/// A 16 px checkbox; use it in forms that are confirmed later.
pub fn checkbox<'a, M: 'a>(look: Look, label: &'a str, checked: bool, on_toggle: Option<impl Fn(bool) -> M + 'a>) -> Element<'a, M> {
    let c = look.color;
    iced_checkbox(label, checked)
        .on_toggle_maybe(on_toggle)
        .size(16)
        .spacing(8)
        .text_size(font::SIZE_BODY)
        .font(fonts::BODY)
        .style(move |_, status| {
            use iced_checkbox::Status as S;
            let (on, hovered, disabled) = match status {
                S::Active { is_checked } => (is_checked, false, false),
                S::Hovered { is_checked } => (is_checked, true, false),
                S::Disabled { is_checked } => (is_checked, false, true),
            };
            let r = radius::SMALL;
            iced_checkbox::Style {
                background: Background::Color(if disabled { c.panel } else if on { if hovered { c.accent_hover } else { c.accent } } else { c.raised }),
                icon_color: if disabled { c.text_disabled } else { c.text_on_accent },
                border: if disabled {
                    border(c.text_disabled, 1.5, r)
                } else if on {
                    border(c.accent, 1.5, r)
                } else {
                    border(if hovered { c.text } else { c.border_strong }, 1.5, r)
                },
                text_color: Some(if disabled { c.text_disabled } else { c.text }),
            }
        })
        .into()
}

/// A 16 px radio button with its dot in `selected-bar`.
pub fn radio<'a, V, M>(look: Look, label: &'a str, value: V, selected: Option<V>, on_click: impl FnOnce(V) -> M) -> Element<'a, M>
where
    V: Copy + Eq,
    M: Clone + 'a,
{
    let c = look.color;
    iced_radio(label, value, selected, on_click)
        .size(16)
        .spacing(8)
        .text_size(font::SIZE_BODY)
        .font(fonts::BODY)
        .style(move |_, status| {
            use iced_radio::Status as S;
            let (on, hovered) = match status {
                S::Active { is_selected } => (is_selected, false),
                S::Hovered { is_selected } => (is_selected, true),
            };
            iced_radio::Style {
                background: Background::Color(c.raised),
                dot_color: c.selected_bar,
                border_width: 1.5,
                border_color: if on { c.selected_bar } else if hovered { c.text } else { c.border_strong },
                text_color: Some(c.text),
            }
        })
        .into()
}

/// A switch that acts at once, such as showing a grid. Height 18 px.
pub fn switch<'a, M: 'a>(look: Look, label: &'a str, on: bool, on_toggle: Option<impl Fn(bool) -> M + 'a>) -> Element<'a, M> {
    let c = look.color;
    iced_toggler(on)
        .label(label)
        .on_toggle_maybe(on_toggle)
        .size(18)
        .spacing(8)
        .text_size(font::SIZE_BODY)
        .font(fonts::BODY)
        .style(move |_, status| {
            use iced_toggler::Status as S;
            let (toggled, hovered, disabled) = match status {
                S::Active { is_toggled } => (is_toggled, false, false),
                S::Hovered { is_toggled } => (is_toggled, true, false),
                S::Disabled => (on, false, true),
            };
            iced_toggler::Style {
                background: if disabled { c.border } else if toggled { if hovered { c.accent_hover } else { c.accent } } else { c.border_strong },
                background_border_width: 0.0,
                background_border_color: Color::TRANSPARENT,
                foreground: if disabled { c.text_disabled } else { Color::WHITE },
                foreground_border_width: 0.0,
                foreground_border_color: Color::TRANSPARENT,
            }
        })
        .into()
}

/// A slider: 4 px track, accent fill and a 14 px knob with an accent edge,
/// with the value in mono next to it.
pub fn slider<'a, M: Clone + 'a>(look: Look, range: std::ops::RangeInclusive<f32>, value: f32, on_change: impl Fn(f32) -> M + 'a, value_label: String) -> Element<'a, M> {
    let c = look.color;
    let control = iced_slider(range, value, on_change).style(move |_, _| iced_slider::Style {
        rail: iced_slider::Rail {
            backgrounds: (Background::Color(c.accent), Background::Color(c.border_strong)),
            width: 4.0,
            border: border(Color::TRANSPARENT, 0.0, 2.0),
        },
        handle: iced_slider::Handle {
            shape: iced_slider::HandleShape::Circle { radius: 7.0 },
            background: Background::Color(c.raised),
            border_width: 2.0,
            border_color: c.accent,
        },
    });
    row![control, text(value_label).size(font::SIZE_BODY).font(fonts::MONO).color(c.text)].spacing(8).align_y(Alignment::Center).into()
}

/// A 6 px progress bar in `accent`.
pub fn progress<'a, M: 'a>(look: Look, value: f32) -> Element<'a, M> {
    let c = look.color;
    iced_progress(0.0..=1.0, value)
        .height(6)
        .style(move |_| iced_progress::Style {
            background: Background::Color(c.border),
            bar: Background::Color(c.accent),
            border: border(Color::TRANSPARENT, 0.0, 3.0),
        })
        .into()
}

// ---------- Panels, tree, properties, table ----------

/// A panel header of 24 px with a mono label and 20 px icon buttons.
pub fn panel_header<'a, M: 'a>(look: Look, title: &str, actions: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut bar = row![mono_label(look, title), Space::with_width(Length::Fill)].spacing(2).align_y(Alignment::Center);
    for action in actions {
        bar = bar.push(action);
    }
    container(bar).height(shell::PANEL_HEADER).padding([0, 8]).align_y(Vertical::Center).into()
}

/// A side panel: header, line and content on `panel`.
/// The width is 240 px by default (`shell::PANEL_WIDTH`); users may drag it.
pub fn panel<'a, M: 'a>(look: Look, title: &str, width: f32, actions: Vec<Element<'a, M>>, content: Element<'a, M>) -> Element<'a, M> {
    container(column![panel_header(look, title, actions), divider(look), content])
        .width(width)
        .height(Length::Fill)
        .style(move |_| panel_style(look))
        .into()
}

fn selectable<'a, M: 'a>(look: Look, content: Element<'a, M>, selected: bool, height: f32) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    let lead: Element<'a, M> = if selected && !hc { bar(c.selected_bar, 3.0, Length::Fill) } else { Space::with_width(3).into() };
    container(row![lead, content].height(Length::Fill).align_y(Alignment::Center))
        .height(height)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: if selected && !hc { fill(c.selected) } else { None },
            border: if selected && hc { border(c.selected_bar, 2.0, 0.0) } else { Border::default() },
            ..Default::default()
        })
        .into()
}

/// A tree row of 24 px: 16 px indent per level, a 12 px arrow and a 14 px icon.
/// With `on_toggle` the arrow is its own button that expands or collapses the row.
#[allow(clippy::too_many_arguments)]
pub fn tree_row<'a, M: Clone + 'a>(
    look: Look,
    depth: u16,
    arrow: Option<&'static str>,
    on_toggle: Option<M>,
    icon: Option<&'static str>,
    label: &'a str,
    selected: bool,
    on_press: Option<M>,
    on_menu: Option<M>,
) -> Element<'a, M> {
    let c = look.color;
    let mut line = row![Space::with_width(4.0 + 16.0 * f32::from(depth))].spacing(4).align_y(Alignment::Center);
    let toggle: Element<'a, M> = match (arrow, on_toggle) {
        (Some(source), Some(toggle)) => iced_button(container(glyph(source, 12.0, c.text_secondary, c.text_secondary)).center(16))
            .width(16)
            .height(16)
            .padding(0)
            .on_press(toggle)
            .style(move |_, status| iced_button::Style {
                background: if status == iced_button::Status::Hovered && !look.high_contrast() { fill(c.pressed) } else { None },
                text_color: c.text,
                border: border(if status == iced_button::Status::Hovered && look.high_contrast() { c.accent } else { Color::TRANSPARENT }, 1.0, radius::SMALL),
                shadow: Shadow::default(),
            })
            .into(),
        (Some(source), None) => container(glyph(source, 12.0, c.text_secondary, c.text_secondary)).center(16).into(),
        (None, _) => Space::with_width(16).into(),
    };
    line = line.push(toggle);
    if let Some(source) = icon {
        line = line.push(glyph(source, 14.0, c.text, c.accent_text));
    }
    line = line.push(text(label).size(font::SIZE_BODY).font(if selected { fonts::STRONG } else { fonts::BODY }).color(c.text));
    let content = iced_button(container(line).height(Length::Fill).align_y(Vertical::Center))
        .padding(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .on_press_maybe(on_press).style(move |_, status| iced_button::Style {
        background: if status == iced_button::Status::Hovered && !selected && !look.high_contrast() { fill(c.hover) } else { None },
        text_color: c.text,
        border: if status == iced_button::Status::Hovered && look.high_contrast() { border(c.accent, 1.0, 0.0) } else { Border::default() },
        shadow: Shadow::default(),
    });
    let content: Element<'a, M> = match on_menu {
        Some(menu) => mouse_area(content).on_right_press(menu).into(),
        None => content.into(),
    };
    selectable(look, content, selected, 24.0)
}

/// A section header of 24 px in the property grid, with an arrow.
pub fn section_header<'a, M: 'a>(look: Look, arrow: &'static str, title: &str) -> Element<'a, M> {
    let c = look.color;
    container(row![glyph(arrow, 12.0, c.text_secondary, c.text_secondary), mono_label(look, title)].spacing(6).align_y(Alignment::Center))
        .height(24)
        .padding([0, 8])
        .align_y(Vertical::Center)
        .into()
}

/// A property row of 28 px: label in 40% of the width, value in the rest.
pub fn property_row<'a, M: 'a>(look: Look, label: &'a str, value: Element<'a, M>) -> Element<'a, M> {
    container(
        row![
            container(secondary(look, label)).width(Length::FillPortion(2)),
            container(value).width(Length::FillPortion(3)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .height(28)
    .padding([0, 8])
    .align_y(Vertical::Center)
    .into()
}

/// A number or code in mono, right-aligned, as in tables and the property grid.
pub fn number<'a>(look: Look, value: impl text::IntoFragment<'a>) -> text::Text<'a> {
    text(value).size(font::SIZE_BODY).font(fonts::MONO).color(look.color.text).width(Length::Fill).align_x(Horizontal::Right)
}

/// One table column.
#[derive(Debug, Clone, Copy)]
pub struct TableColumn<'a> {
    pub title: &'a str,
    pub width: Length,
    /// Numbers and amounts: mono and right-aligned.
    pub numeric: bool,
}

/// A table: 28 px mono header with a `border-strong` line, 28 px rows with a
/// `border` line, no zebra stripes. The selected row gets the tint and a bar.
/// With `on_select` each row is clickable.
pub fn table<'a, M: Clone + 'a>(
    look: Look,
    columns: &[TableColumn<'a>],
    rows: Vec<Vec<String>>,
    selected: Option<usize>,
    on_select: Option<impl Fn(usize) -> M>,
    on_menu: Option<impl Fn(usize) -> M>,
) -> Element<'a, M> {
    let c = look.color;
    let cell_row = |cells: Vec<Element<'a, M>>| -> Element<'a, M> {
        let mut line = row![].spacing(12).align_y(Alignment::Center).padding([0, 8]);
        for (cell, col) in cells.into_iter().zip(columns) {
            line = line.push(container(cell).width(col.width));
        }
        line.into()
    };
    let header = cell_row(
        columns
            .iter()
            .map(|col| {
                let label = mono_label(look, col.title);
                if col.numeric { label.width(Length::Fill).align_x(Horizontal::Right).into() } else { label.into() }
            })
            .collect(),
    );
    let mut sheet = column![container(header).height(28).align_y(Vertical::Center), bar(c.border_strong, Length::Fill, 1.0)];
    for (i, cells) in rows.into_iter().enumerate() {
        let on = selected == Some(i);
        let line = cell_row(
            cells
                .into_iter()
                .zip(columns)
                .map(|(value, col)| if col.numeric { number(look, value).into() } else { body(look, value).into() })
                .collect(),
        );
        let line: Element<'a, M> = match &on_select {
            Some(select) => iced_button(container(line).height(Length::Fill).align_y(Vertical::Center))
                .padding(0)
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(select(i))
                .style(move |_, status| iced_button::Style {
                    background: if status == iced_button::Status::Hovered && !on && !look.high_contrast() { fill(c.hover) } else { None },
                    text_color: c.text,
                    border: if status == iced_button::Status::Hovered && look.high_contrast() { border(c.accent, 1.0, 0.0) } else { Border::default() },
                    shadow: Shadow::default(),
                })
                .into(),
            None => line,
        };
        let line: Element<'a, M> = match &on_menu {
            Some(menu) => mouse_area(line).on_right_press(menu(i)).into(),
            None => line,
        };
        sheet = sheet.push(selectable(look, line, on, 28.0));
        sheet = sheet.push(divider(look));
    }
    sheet.width(Length::Fill).into()
}

// ---------- Notifications and dialogs ----------

/// An in-page notification: status tint, a 3 px bar, icon, bold word, text and close.
pub fn banner<'a, M: Clone + 'a>(look: Look, tone: Tone, icon: &'static str, message: &'a str, close_icon: &'static str, on_close: Option<M>) -> Element<'a, M> {
    banner_with_action(look, tone, icon, message, None, close_icon, on_close)
}

/// An in-page notification with an action button before the close button,
/// such as "Nu bijwerken". The bar grows to 40 px to fit a compact button.
pub fn banner_with_action<'a, M: Clone + 'a>(
    look: Look,
    tone: Tone,
    icon: &'static str,
    message: &'a str,
    action: Option<Element<'a, M>>,
    close_icon: &'static str,
    on_close: Option<M>,
) -> Element<'a, M> {
    let c = look.color;
    let height = if action.is_some() { 40.0 } else { 32.0 };
    let hc = look.high_contrast();
    let fg = tone.color(look);
    let tint = if hc { c.panel } else { tone.tint(look) };
    let content = row![
        glyph(icon, 16.0, fg, fg),
        text(tone.word()).size(font::SIZE_BODY).font(fonts::STRONG).color(c.text),
        text(message).size(font::SIZE_BODY).font(fonts::BODY).color(c.text).width(Length::Fill),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    let content = match action {
        Some(action) => content.push(action),
        None => content,
    }
    .push(icon_button(look, close_icon, 20.0, false, on_close));
    container(row![bar(fg, 3.0, Length::Fill), container(content).padding([0, 10]).height(Length::Fill).align_y(Vertical::Center).width(Length::Fill)])
        .width(Length::Fill)
        .height(height)
        .style(move |_| container::Style {
            background: fill(tint),
            border: if hc { border(fg, 1.0, 0.0) } else { Border::default() },
            ..Default::default()
        })
        .into()
}

/// A notification at the bottom right: raised, popup shadow, 10 px corner.
pub fn toast<'a, M: Clone + 'a>(look: Look, tone: Tone, icon: &'static str, title: &'a str, message: &'a str, close_icon: &'static str, on_close: Option<M>) -> Element<'a, M> {
    let fg = tone.color(look);
    container(
        row![
            glyph(icon, 16.0, fg, fg),
            column![text(title).size(font::SIZE_BODY).font(fonts::STRONG).color(look.color.text), secondary(look, message)].spacing(2).width(Length::Fill),
            icon_button(look, close_icon, 20.0, false, on_close),
        ]
        .spacing(10),
    )
    .width(320)
    .padding(12)
    .style(move |_| surface(look, look.tokens.shadow.popup, radius::LARGE))
    .into()
}

/// A dialog card: 420 px (or 560 px), title in Space Grotesk 16 px with a close
/// button, content, and buttons at the bottom right. Draw it over [`overlay`].
#[allow(clippy::too_many_arguments)] // Public layout slots map directly to the reviewed dialog spec.
pub fn dialog<'a, M: Clone + 'a>(
    look: Look,
    title: &'a str,
    close_icon: &'static str,
    on_close: Option<M>,
    content: Element<'a, M>,
    quiet: Option<Element<'a, M>>,
    actions: Vec<Element<'a, M>>,
    wide: bool,
) -> Element<'a, M> {
    let mut footer = row![].spacing(8).align_y(Alignment::Center);
    if let Some(button) = quiet {
        footer = footer.push(button);
    }
    footer = footer.push(Space::with_width(Length::Fill));
    for action in actions {
        footer = footer.push(action);
    }
    container(
        column![
            row![heading(look, title, font::SIZE_DIALOG_HEADING).width(Length::Fill), icon_button(look, close_icon, 28.0, false, on_close)].align_y(Alignment::Center),
            content,
            footer,
        ]
        .spacing(16),
    )
    .width(if wide { 560 } else { 420 })
    .padding(20)
    .style(move |_| surface(look, look.tokens.shadow.dialog, radius::LARGE))
    .into()
}

/// The theme overlay behind a dialog, with the dialog centred.
pub fn overlay<'a, M: 'a>(look: Look, dialog: Element<'a, M>) -> Element<'a, M> {
    let shade = look.tokens.overlay;
    container(dialog).center(Length::Fill).style(move |_| container::Style { background: fill(shade), ..Default::default() }).into()
}

// ---------- Ribbon ----------

/// Ribbon tabs (28 px): the Bestand button and text tabs with an accent bar under the active one.
pub fn ribbon_tabs<'a, M: Clone + 'a>(look: Look, file_label: &'a str, on_file: Option<M>, tabs: &[&'a str], active: usize, on_select: impl Fn(usize) -> M) -> Element<'a, M> {
    let c = look.color;
    let file = iced_button(container(text(file_label).size(font::SIZE_BODY).font(fonts::STRONG)).height(Length::Fill).align_y(Vertical::Center))
        .height(22)
        .padding([0, 12])
        .on_press_maybe(on_file)
        .style(move |_, status| button_style(look, ButtonKind::Primary, status));
    let mut line = row![container(file).height(Length::Fill).align_y(Vertical::Center).padding([0, 8])].height(Length::Fill);
    for (i, &name) in tabs.iter().enumerate() {
        let on = i == active;
        let label = text(name)
            .size(font::SIZE_BODY)
            .font(if on { fonts::STRONG } else { fonts::MEDIUM })
            .color(if on { c.text } else { c.text_secondary });
        // The label sets the size; the 2 px accent bar is a second layer at its bottom.
        let tab = stack![
            container(label).height(Length::Fill).align_y(Vertical::Center).padding([0, 12]),
            container(bar(if on { c.accent } else { Color::TRANSPARENT }, Length::Fill, 2.0)).height(Length::Fill).align_y(Vertical::Bottom),
        ];
        line = line.push(iced_button(tab).padding(0).height(Length::Fill).on_press(on_select(i)).style(move |_, status| iced_button::Style {
            background: if status == iced_button::Status::Hovered && !on && !look.high_contrast() { fill(c.hover) } else { None },
            text_color: c.text,
            border: Border::default(),
            shadow: Shadow::default(),
        }));
    }
    container(line).height(shell::RIBBON_TABS).width(Length::Fill).style(move |_| container::Style { background: fill(c.panel), ..Default::default() }).into()
}

fn ribbon_button_style(look: Look, active: bool, status: iced_button::Status) -> iced_button::Style {
    use iced_button::Status as S;
    let c = look.color;
    let hc = look.high_contrast();
    let r = radius::MEDIUM;
    iced_button::Style {
        background: match status {
            S::Pressed if !hc => fill(c.pressed),
            S::Hovered if !hc => fill(c.hover),
            _ => None,
        },
        text_color: if status == S::Disabled { c.text_disabled } else { c.text },
        border: if hc && !active && matches!(status, S::Hovered | S::Pressed) { border(c.accent, 1.0, r) } else { border(Color::TRANSPARENT, 0.0, r) },
        shadow: Shadow::default(),
    }
}

fn icon_tile<'a, M: 'a>(look: Look, icon: Element<'a, M>, active: bool, size: f32) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    container(icon)
        .center(size)
        .style(move |_| container::Style {
            background: if active && !hc { fill(c.selected) } else { None },
            border: if active {
                border(if hc { c.accent } else { mix(c.accent, 0.35, c.panel) }, if hc { 2.0 } else { 1.0 }, radius::MEDIUM)
            } else {
                Border::default()
            },
            ..Default::default()
        })
        .into()
}

/// A large ribbon button: 56 × 66 px, a 22 px icon in a 32 px tile, label of 11 px.
pub fn ribbon_large<'a, M: Clone + 'a>(look: Look, icon: &'static str, label: &'a str, active: bool, on_press: Option<M>) -> Element<'a, M> {
    let c = look.color;
    let disabled = on_press.is_none();
    let fg = if disabled { c.text_disabled } else if active { c.accent_text } else { c.text };
    let accent = if disabled { fg } else { c.accent_text };
    // At least 56 px wide; a longer one-word label widens the button instead of being cut.
    let content = column![
        Space::with_width(48),
        icon_tile(look, glyph(icon, 22.0, fg, accent), active, 32.0),
        text(label).size(font::SIZE_LABEL).font(fonts::BODY).align_x(Horizontal::Center),
    ]
    .spacing(4)
    .align_x(Alignment::Center);
    iced_button(content)
        .height(66)
        .padding([0, 4])
        .on_press_maybe(on_press)
        .style(move |_, status| ribbon_button_style(look, active, status))
        .into()
}

/// A small ribbon button: 22 px high, 16 px icon, label of 11 px.
pub fn ribbon_small<'a, M: Clone + 'a>(look: Look, icon: &'static str, label: &'a str, active: bool, on_press: Option<M>) -> Element<'a, M> {
    let c = look.color;
    let disabled = on_press.is_none();
    let fg = if disabled { c.text_disabled } else if active { c.accent_text } else { c.text };
    let accent = if disabled { fg } else { c.accent_text };
    iced_button(row![glyph(icon, 16.0, fg, accent), text(label).size(font::SIZE_LABEL).font(fonts::BODY)].spacing(6).align_y(Alignment::Center))
        .height(22)
        .padding([0, 4])
        .on_press_maybe(on_press)
        .style(move |_, status| {
            let mut s = ribbon_button_style(look, active, status);
            s.border.radius = radius::SMALL.into();
            if active {
                s.background = if look.high_contrast() { None } else { fill(c.selected) };
                s.border = border(if look.high_contrast() { c.accent } else { mix(c.accent, 0.35, c.panel) }, 1.0, radius::SMALL);
            }
            s
        })
        .into()
}

/// A ribbon group: its buttons on top and a centred mono label at the bottom.
pub fn ribbon_group<'a, M: 'a>(look: Look, label: &str, buttons: Vec<Element<'a, M>>) -> Element<'a, M> {
    let mut items = row![].spacing(2).align_y(Alignment::Start);
    for button in buttons {
        items = items.push(button);
    }
    column![items, Space::with_height(Length::Fill), mono_label(look, label)]
        .align_x(Alignment::Center)
        .padding([4, 6])
        .height(Length::Fill)
        .into()
}

/// The ribbon band (94 px): groups separated by a short line in `border`.
pub fn ribbon<'a, M: 'a>(look: Look, groups: Vec<Element<'a, M>>) -> Element<'a, M> {
    let c = look.color;
    let mut band = row![].height(Length::Fill);
    let count = groups.len();
    for (i, group) in groups.into_iter().enumerate() {
        band = band.push(group);
        if i + 1 < count {
            band = band.push(container(vline(c.border)).padding([10, 0]).height(Length::Fill));
        }
    }
    container(band)
        .height(shell::RIBBON)
        .width(Length::Fill)
        .style(move |_| container::Style { background: fill(c.panel), border: border(c.border, 0.0, 0.0), ..Default::default() })
        .into()
}

// ---------- Document tabs, status bar, empty state ----------

/// One document tab.
#[derive(Debug, Clone, Copy)]
pub struct DocumentTab<'a> {
    pub title: &'a str,
    /// Unsaved changes: a 6 px accent dot.
    pub dirty: bool,
    /// A fixed tab, such as a catalogue, has no close button.
    pub pinned: bool,
}

impl<'a> DocumentTab<'a> {
    pub fn new(title: &'a str, dirty: bool) -> Self {
        Self { title, dirty, pinned: false }
    }
    pub fn pinned(title: &'a str) -> Self {
        Self { title, dirty: false, pinned: true }
    }
}

/// Document tabs (30 px bar in `docbar`): the active tab takes the workspace colour
/// and rounded top corners, so it flows into the workspace.
pub fn document_tabs<'a, M: Clone + 'a>(
    look: Look,
    tabs: &[DocumentTab<'a>],
    active: usize,
    close_icon: &'static str,
    on_select: impl Fn(usize) -> M,
    on_close: impl Fn(usize) -> M,
    on_menu: impl Fn(usize) -> M,
) -> Element<'a, M> {
    let c = look.color;
    let mut line = row![].spacing(4).padding(Padding { top: 6.0, right: 8.0, bottom: 0.0, left: 8.0 }).height(Length::Fill);
    for (i, tab) in tabs.iter().enumerate() {
        let on = i == active;
        let fg = if on { c.workspace_text } else { c.text_secondary };
        let mut inner = row![].spacing(6).align_y(Alignment::Center);
        if tab.dirty {
            inner = inner.push(container(Space::new(6, 6)).style(move |_| container::Style {
                background: fill(if on { look.workspace_accent_text } else { c.accent }),
                border: border(Color::TRANSPARENT, 0.0, 3.0),
                ..Default::default()
            }));
        }
        inner = inner.push(text(tab.title).size(font::SIZE_BODY).font(if on { fonts::STRONG } else { fonts::BODY }).color(fg));
        // The 16 px close button is its own button inside the tab.
        let hover_bg = if on { mix(c.workspace_text, 0.12, c.workspace) } else { c.pressed };
        if !tab.pinned {
        inner = inner.push(
            iced_button(container(glyph(close_icon, 12.0, fg, fg)).center(16))
                .width(16)
                .height(16)
                .padding(0)
                .on_press(on_close(i))
                .style(move |_, status| iced_button::Style {
                    background: if status == iced_button::Status::Hovered && !look.high_contrast() { fill(hover_bg) } else { None },
                    text_color: fg,
                    border: border(if status == iced_button::Status::Hovered && look.high_contrast() { c.accent } else { Color::TRANSPARENT }, 1.0, radius::SMALL),
                    shadow: Shadow::default(),
                }),
        );
        }
        let workspace = c.workspace;
        let tab = iced_button(inner).height(24).padding([0, 10]).on_press(on_select(i)).style(move |_, status| iced_button::Style {
            background: if on { fill(workspace) } else if status == iced_button::Status::Hovered && !look.high_contrast() { fill(c.hover) } else { None },
            text_color: fg,
            border: Border {
                color: if look.high_contrast() && on { c.accent } else { Color::TRANSPARENT },
                width: if look.high_contrast() && on { 1.0 } else { 0.0 },
                radius: Radius { top_left: radius::MEDIUM, top_right: radius::MEDIUM, bottom_right: 0.0, bottom_left: 0.0 },
            },
            shadow: Shadow::default(),
        });
        line = line.push(mouse_area(tab).on_right_press(on_menu(i)));
    }
    container(line).height(shell::DOCUMENT_TABS).width(Length::Fill).style(move |_| container::Style { background: fill(c.docbar), ..Default::default() }).into()
}

/// The status bar (22 px). It follows the theme with a line above it, unless the
/// Statusbalk setting makes it dark.
pub fn status_bar<'a, M: 'a>(look: Look, left: Vec<String>, right: Vec<String>) -> Element<'a, M> {
    let c = look.color;
    let item = move |s: String| -> Element<'a, M> { text(s).size(font::SIZE_LABEL).font(fonts::BODY).color(c.statusbar_text).into() };
    let mut line = row![].spacing(16).align_y(Alignment::Center).padding([0, 10]);
    for s in left {
        line = line.push(item(s));
    }
    line = line.push(Space::with_width(Length::Fill));
    for s in right {
        line = line.push(text(s).size(font::SIZE_LABEL).font(fonts::MONO).color(c.statusbar_text));
    }
    column![
        divider(look),
        container(line).height(shell::STATUSBAR - 1.0).width(Length::Fill).align_y(Vertical::Center).style(move |_| container::Style { background: fill(c.statusbar), ..Default::default() }),
    ]
    .into()
}

/// An empty state where the document would be: it follows the theme.
pub fn empty_state<'a, M: 'a>(look: Look, icon: &'static str, title: &'a str, explanation: &'a str, action: Option<Element<'a, M>>) -> Element<'a, M> {
    let c = look.color;
    let mut content = column![
        glyph(icon, 32.0, c.accent_text, c.accent_text),
        text(title).size(font::SIZE_PANEL_HEADING).font(fonts::STRONG).color(c.text),
        secondary(look, explanation).align_x(Horizontal::Center),
    ]
    .spacing(8)
    .align_x(Alignment::Center)
    .max_width(320);
    if let Some(action) = action {
        content = content.push(Space::with_height(4)).push(action);
    }
    container(content).center(Length::Fill).style(move |_| container::Style { background: fill(c.panel), ..Default::default() }).into()
}
