//! UI 0.4 tiles and cards (`design/ontwerp-0.4/03-tegels.md`): the icon tile,
//! task tiles, the recent-files list, project cards, choice tiles, info cards,
//! figure tiles and the navigation of the Bestand screen.
use super::widgets::{bar, border, fill, glyph, mono_label, Tone};
use super::{font, mix, radius, Look, Shadows};
use crate::fonts;
use iced::alignment::Vertical;
use iced::widget::{button, column, container, row, text, Space};
use iced::{Alignment, Border, Color, Element, Length, Shadow};

/// The icon tile: 40 px (32 px in compact lists), `selected` with a border of
/// accent mixed with `raised`, and a 22 px icon in `accent-text`.
pub fn icon_tile<'a, M: 'a>(look: Look, icon: &'static str, size: f32, disabled: bool) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    let fg = if disabled { c.text_disabled } else { c.accent_text };
    container(glyph(icon, if size >= 40.0 { 22.0 } else { 18.0 }, fg, fg))
        .center(size)
        .style(move |_| container::Style {
            background: fill(if disabled || hc { c.panel } else { c.selected }),
            border: border(
                if disabled { c.border } else if hc { c.accent } else { mix(c.accent, 0.35, c.raised) },
                1.0,
                radius::MEDIUM,
            ),
            ..Default::default()
        })
        .into()
}

/// Shared states of clickable tiles: hover gives an accent border, pressed the
/// accent tint; High contrast uses 2 and 4 px borders. Tiles never lift.
pub fn tile_style(look: Look, status: button::Status, chosen: bool, r: f32) -> button::Style {
    use button::Status as S;
    let c = look.color;
    let hc = look.high_contrast();
    let (background, edge) = match status {
        S::Disabled => (c.raised, border(c.border, 1.0, r)),
        S::Pressed if hc => (c.raised, border(c.accent, 4.0, r)),
        S::Pressed => (c.selected, border(c.accent, 1.0, r)),
        _ if chosen => (c.raised, border(c.selected_bar, 2.0, r)),
        S::Hovered => (c.raised, border(c.accent, if hc { 2.0 } else { 1.0 }, r)),
        S::Active => (c.raised, border(c.border, 1.0, r)),
    };
    button::Style { background: fill(background), text_color: c.text, border: edge, shadow: Shadow::default() }
}

fn title<'a>(look: Look, content: impl text::IntoFragment<'a>, disabled: bool) -> text::Text<'a> {
    text(content).size(font::SIZE_PANEL_HEADING).font(fonts::STRONG).color(if disabled { look.color.text_disabled } else { look.color.text })
}

fn description<'a>(look: Look, content: impl text::IntoFragment<'a>, disabled: bool) -> text::Text<'a> {
    text(content).size(font::SIZE_BODY).font(fonts::BODY).color(if disabled { look.color.text_disabled } else { look.color.text_secondary })
}

/// A task tile (variant B): icon tile on the left, title and description beside it.
/// Use it in a column of at most 620 px with 8 px between the tiles.
pub fn task_tile<'a, M: Clone + 'a>(look: Look, icon: &'static str, heading: &'a str, explanation: &'a str, on_press: Option<M>) -> Element<'a, M> {
    let disabled = on_press.is_none();
    button(
        row![icon_tile(look, icon, 40.0, disabled), column![title(look, heading, disabled), description(look, explanation, disabled)].spacing(2)]
            .spacing(12)
            .align_y(Alignment::Center),
    )
    .padding([12, 16])
    .width(Length::Fill)
    .on_press_maybe(on_press)
    .style(move |_, status| tile_style(look, status, false, radius::LARGE))
    .into()
}

/// One row in the recent-files list.
#[derive(Debug, Clone)]
pub struct RecentFile<'a, M> {
    pub icon: &'static str,
    pub name: &'a str,
    pub path: &'a str,
    pub date: &'a str,
    pub on_press: Option<M>,
}

/// Recent files: one frame with a 10 px corner, rows of 40 px separated by a line.
pub fn recent_files<'a, M: Clone + 'a>(look: Look, files: Vec<RecentFile<'a, M>>) -> Element<'a, M> {
    let c = look.color;
    let count = files.len();
    let mut list = column![];
    for (i, file) in files.into_iter().enumerate() {
        let line = row![
            glyph(file.icon, 18.0, c.text_secondary, c.accent_text),
            text(file.name).size(font::SIZE_BODY).font(fonts::MEDIUM).color(c.text),
            text(file.path).size(font::SIZE_LABEL).font(fonts::BODY).color(c.text_secondary).width(Length::Fill),
            text(file.date).size(font::SIZE_LABEL).font(fonts::MONO).color(c.text_secondary),
        ]
        .spacing(10)
        .align_y(Alignment::Center);
        list = list.push(
            button(container(line).height(Length::Fill).align_y(Vertical::Center))
                .height(40)
                .width(Length::Fill)
                .padding([0, 12])
                .on_press_maybe(file.on_press)
                .style(move |_, status| button::Style {
                    background: match status {
                        button::Status::Hovered if !look.high_contrast() => fill(c.hover),
                        button::Status::Pressed if !look.high_contrast() => fill(c.pressed),
                        _ => None,
                    },
                    text_color: c.text,
                    border: if look.high_contrast() && status == button::Status::Hovered { border(c.accent, 2.0, 0.0) } else { Border::default() },
                    shadow: Shadow::default(),
                }),
        );
        if i + 1 < count {
            list = list.push(bar(c.border, Length::Fill, 1.0));
        }
    }
    container(list)
        .width(Length::Fill)
        .clip(true)
        .style(move |_| container::Style { background: fill(c.raised), border: border(c.border, 1.0, radius::LARGE), ..Default::default() })
        .into()
}

/// A round status label in mono 11 px, in the status colour.
pub fn status_label<'a, M: 'a>(look: Look, tone: Tone, label: &'a str) -> Element<'a, M> {
    let fg = tone.color(look);
    let tint = if look.high_contrast() { look.color.panel } else { tone.tint(look) };
    container(text(label).size(font::SIZE_LABEL).font(fonts::MONO).color(fg))
        .padding([1, 8])
        .style(move |_| container::Style { background: fill(tint), border: border(fg, 1.0, radius::FULL), ..Default::default() })
        .into()
}

/// A project card: a 96 px preview in the workspace colour (or the app icon
/// when the app cannot make a preview), name, metadata in mono and a status label.
pub fn project_card<'a, M: Clone + 'a>(
    look: Look,
    preview: Option<Element<'a, M>>,
    app_icon: &'static str,
    name: &'a str,
    meta: &'a str,
    status: Option<(Tone, &'a str)>,
    on_press: Option<M>,
) -> Element<'a, M> {
    let c = look.color;
    let image = container(preview.unwrap_or_else(|| glyph(app_icon, 32.0, c.workspace_text, look.workspace_accent_text)))
        .center_x(Length::Fill)
        .height(96)
        .align_y(Vertical::Center)
        .style(move |_| container::Style {
            background: fill(c.workspace),
            border: Border { color: c.border, width: 0.0, radius: iced::border::Radius { top_left: radius::LARGE - 1.0, top_right: radius::LARGE - 1.0, bottom_right: 0.0, bottom_left: 0.0 } },
            ..Default::default()
        });
    let mut info = column![
        text(name).size(font::SIZE_PANEL_HEADING).font(fonts::STRONG).color(c.text),
        text(meta).size(font::SIZE_LABEL).font(fonts::MONO).color(c.text_secondary),
    ]
    .spacing(4);
    if let Some((tone, label)) = status {
        info = info.push(Space::with_height(2)).push(status_label(look, tone, label));
    }
    button(column![image, bar(c.border, Length::Fill, 1.0), container(info).padding([10, 12])])
        .padding(1)
        .width(Length::Fill)
        .on_press_maybe(on_press)
        .style(move |_, status| {
            let mut s = tile_style(look, status, false, radius::LARGE);
            s.shadow = Shadows::drop_shadow(look.tokens.shadow.panel).unwrap_or_default();
            s
        })
        .into()
}

/// A choice tile: icon tile, title and description, with a radio at the top right.
/// The chosen tile has a 2 px border in `selected-bar` and a filled radio.
pub fn choice_tile<'a, M: Clone + 'a>(look: Look, icon: &'static str, heading: &'a str, explanation: &'a str, chosen: bool, on_press: M) -> Element<'a, M> {
    let c = look.color;
    let dot = container(container(Space::new(8, 8)).style(move |_| container::Style {
        background: fill(if chosen { c.selected_bar } else { Color::TRANSPARENT }),
        border: border(Color::TRANSPARENT, 0.0, 4.0),
        ..Default::default()
    }))
    .center(16)
    .style(move |_| container::Style { background: fill(c.raised), border: border(if chosen { c.selected_bar } else { c.border_strong }, 1.5, 8.0), ..Default::default() });
    button(
        column![
            row![icon_tile(look, icon, 40.0, false), Space::with_width(Length::Fill), dot].align_y(Alignment::Start),
            title(look, heading, false),
            description(look, explanation, false),
        ]
        .spacing(6),
    )
    .padding([12, 14])
    .width(Length::Fill)
    .on_press(on_press)
    .style(move |_, status| tile_style(look, status, chosen, radius::LARGE))
    .into()
}

/// An info card: a mono heading and key–value rows. Not clickable, so no hover.
pub fn info_card<'a, M: 'a>(look: Look, heading: &str, rows: Vec<(&'a str, String)>) -> Element<'a, M> {
    let c = look.color;
    let mut content = column![mono_label(look, heading), Space::with_height(4)].spacing(6);
    for (key, value) in rows {
        content = content.push(
            row![
                text(key).size(font::SIZE_BODY).font(fonts::BODY).color(c.text_secondary).width(Length::FillPortion(2)),
                text(value).size(font::SIZE_BODY).font(fonts::MEDIUM).color(c.text).width(Length::FillPortion(3)),
            ]
            .spacing(8),
        );
    }
    container(content)
        .padding([14, 16])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: fill(c.raised),
            border: border(c.border, 1.0, radius::LARGE),
            shadow: Shadows::drop_shadow(look.tokens.shadow.panel).unwrap_or_default(),
            ..Default::default()
        })
        .into()
}

/// A figure tile: a number in Space Grotesk 22 px with a mono label below it.
pub fn figure_tile<'a, M: 'a>(look: Look, value: impl text::IntoFragment<'a>, label: &str, accent: bool) -> Element<'a, M> {
    let c = look.color;
    container(
        column![
            text(value).size(font::SIZE_PAGE_HEADING).font(fonts::HEADING).color(if accent { c.accent_text } else { c.text }),
            mono_label(look, label),
        ]
        .spacing(4),
    )
    .padding([14, 16])
    .width(Length::Fill)
    .style(move |_| container::Style { background: fill(c.raised), border: border(c.border, 1.0, radius::LARGE), ..Default::default() })
    .into()
}

/// A row in the Bestand navigation: 40 px; the active row gets a 3 px bar, the
/// accent tint and its text in `accent-text`. The column is 260 px wide.
pub fn file_nav_row<'a, M: Clone + 'a>(look: Look, icon: &'static str, label: &'a str, active: bool, on_press: Option<M>) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    let disabled = on_press.is_none();
    let fg = if disabled { c.text_disabled } else if active { c.accent_text } else { c.text };
    let lead: Element<'a, M> = if active && !hc { bar(c.selected_bar, 3.0, Length::Fill) } else { Space::with_width(3).into() };
    button(
        row![
            lead,
            row![glyph(icon, 16.0, fg, if disabled { fg } else { c.accent_text }), text(label).size(font::SIZE_BODY).font(if active { fonts::STRONG } else { fonts::MEDIUM }).color(fg)]
                .spacing(10)
                .padding([0, 13])
                .align_y(Alignment::Center),
        ]
        .height(Length::Fill)
        .align_y(Alignment::Center),
    )
    .height(40)
    .width(Length::Fill)
    .padding(0)
    .on_press_maybe(on_press)
    .style(move |_, status| button::Style {
        background: if active && !hc {
            fill(c.selected)
        } else {
            match status {
                button::Status::Hovered if !hc => fill(c.hover),
                button::Status::Pressed if !hc => fill(c.pressed),
                _ => None,
            }
        },
        text_color: fg,
        border: if hc && (active || status == button::Status::Hovered) { border(c.selected_bar, 2.0, 0.0) } else { Border::default() },
        shadow: Shadow::default(),
    })
    .into()
}
