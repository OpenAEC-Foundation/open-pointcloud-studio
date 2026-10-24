//! UI 0.4 workflow apps (`design/ontwerp-0.4/05-layouts.md`): Energy, Speech,
//! Workspace Composer and Field have no ribbon. They navigate with a sidebar
//! (default) or with steps at the top; users choose in Navigatie.
use super::widgets::{bar, border, fill, glyph, mono_label};
use super::{font, mix, radius, shell, Look};
use crate::fonts;
use iced::alignment::Vertical;
use iced::widget::{button, column, container, row, stack, text, Space};
use iced::{Alignment, Border, Color, Element, Length, Shadow};
use serde::{Deserialize, Serialize};

/// User setting Navigatie, only in workflow apps (besluit 2.5-2). Default: sidebar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Navigation {
    #[default]
    Sidebar,
    Steps,
}

impl Navigation {
    pub const ALL: [Self; 2] = [Self::Sidebar, Self::Steps];
    pub fn name(self) -> &'static str {
        match self {
            Self::Sidebar => "Zijbalk",
            Self::Steps => "Stappen bovenaan",
        }
    }
}

impl std::fmt::Display for Navigation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Sidebar width (220 px) and row height (36 px).
pub const SIDEBAR_WIDTH: f32 = 220.0;
pub const SIDEBAR_ROW: f32 = 36.0;
/// Step bar (44 px) and action bar (52 px).
pub const STEP_BAR: f32 = 44.0;
pub const ACTION_BAR: f32 = 52.0;

/// One section of a workflow.
#[derive(Debug, Clone, Copy)]
pub struct Section<'a> {
    pub icon: &'static str,
    pub label: &'a str,
}

/// The sidebar: an optional mono heading and rows of 36 px; the active section
/// gets the tint and a bar, or a 2 px outline in High contrast.
pub fn sidebar<'a, M: Clone + 'a>(look: Look, heading: &str, sections: &[Section<'a>], active: usize, on_select: impl Fn(usize) -> M) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    let mut list = column![container(mono_label(look, heading)).height(shell::PANEL_HEADER).padding([0, 12]).align_y(Vertical::Center)].spacing(2);
    for (i, section) in sections.iter().enumerate() {
        let on = i == active;
        let fg = if on { c.accent_text } else { c.text };
        let lead: Element<'a, M> = if on && !hc { bar(c.selected_bar, 3.0, Length::Fill) } else { Space::with_width(3).into() };
        list = list.push(
            button(
                row![
                    lead,
                    row![glyph(section.icon, 16.0, fg, c.accent_text), text(section.label).size(font::SIZE_BODY).font(if on { fonts::STRONG } else { fonts::MEDIUM }).color(fg)]
                        .spacing(10)
                        .padding([0, 9])
                        .align_y(Alignment::Center),
                ]
                .height(Length::Fill)
                .align_y(Alignment::Center),
            )
            .height(SIDEBAR_ROW)
            .width(Length::Fill)
            .padding(0)
            .on_press(on_select(i))
            .style(move |_, status| button::Style {
                background: if on && !hc {
                    fill(c.selected)
                } else {
                    match status {
                        button::Status::Hovered if !hc => fill(c.hover),
                        button::Status::Pressed if !hc => fill(c.pressed),
                        _ => None,
                    }
                },
                text_color: fg,
                border: if hc && (on || status == button::Status::Hovered) { border(c.selected_bar, 2.0, 0.0) } else { Border::default() },
                shadow: Shadow::default(),
            }),
        );
    }
    container(list)
        .width(SIDEBAR_WIDTH)
        .height(Length::Fill)
        .padding([4, 0])
        .style(move |_| container::Style { background: fill(c.panel), border: border(c.border, 1.0, 0.0), ..Default::default() })
        .into()
}

fn step_circle<'a, M: 'a>(look: Look, number: usize, done: bool, on: bool, check: &'static str) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    let (background, edge, fg) = if on {
        (c.accent, c.accent, c.text_on_accent)
    } else if done {
        (if hc { c.panel } else { c.selected }, if hc { c.accent } else { mix(c.accent, 0.35, c.panel) }, c.accent_text)
    } else {
        (c.panel, c.border_strong, c.text_secondary)
    };
    let label: Element<'a, M> = if done && !on {
        glyph(check, 12.0, fg, fg)
    } else {
        text(number.to_string()).size(font::SIZE_LABEL).font(fonts::MONO).color(fg).into()
    };
    container(label)
        .center(22)
        .style(move |_| container::Style { background: fill(background), border: border(edge, 1.5, 11.0), ..Default::default() })
        .into()
}

/// Steps at the top (44 px): numbered steps in mono. A finished step gets a
/// tinted circle with a check, the active one an amber circle and a bar under it.
pub fn step_bar<'a, M: Clone + 'a>(look: Look, steps: &[Section<'a>], active: usize, done: &[bool], check: &'static str, on_select: impl Fn(usize) -> M) -> Element<'a, M> {
    let c = look.color;
    let mut line = row![].spacing(4).padding([0, 12]).height(Length::Fill).align_y(Alignment::Center);
    for (i, step) in steps.iter().enumerate() {
        let on = i == active;
        let finished = done.get(i).copied().unwrap_or(false);
        let label = text(step.label)
            .size(font::SIZE_BODY)
            .font(if on { fonts::STRONG } else { fonts::MEDIUM })
            .color(if on { c.text } else { c.text_secondary });
        let tab = stack![
            container(row![step_circle(look, i + 1, finished, on, check), label].spacing(8).align_y(Alignment::Center)).height(Length::Fill).align_y(Vertical::Center).padding([0, 10]),
            container(bar(if on { c.accent } else { Color::TRANSPARENT }, Length::Fill, 2.0)).height(Length::Fill).align_y(Vertical::Bottom),
        ];
        line = line.push(
            button(tab).padding(0).height(Length::Fill).on_press(on_select(i)).style(move |_, status| button::Style {
                background: if status == button::Status::Hovered && !on && !look.high_contrast() { fill(c.hover) } else { None },
                text_color: c.text,
                border: Border::default(),
                shadow: Shadow::default(),
            }),
        );
        if i + 1 < steps.len() {
            line = line.push(container(Space::new(24, 1)).style(move |_| container::Style { background: fill(c.border_strong), ..Default::default() }));
        }
    }
    column![
        container(line).height(STEP_BAR - 1.0).width(Length::Fill).style(move |_| container::Style { background: fill(c.panel), ..Default::default() }),
        bar(c.border, Length::Fill, 1.0),
    ]
    .into()
}

/// A page heading of 22 px in Space Grotesk with an optional explanation.
pub fn page_heading<'a, M: 'a>(look: Look, heading: &'a str, explanation: Option<&'a str>) -> Element<'a, M> {
    let c = look.color;
    let mut content = column![text(heading).size(font::SIZE_PAGE_HEADING).font(fonts::HEADING).color(c.text)].spacing(4);
    if let Some(e) = explanation {
        content = content.push(text(e).size(font::SIZE_BODY).font(fonts::BODY).color(c.text_secondary));
    }
    content.into()
}

/// A content card for forms and lists on a workflow page.
pub fn form_card<'a, M: 'a>(look: Look, heading: &str, content: Element<'a, M>) -> Element<'a, M> {
    let c = look.color;
    container(column![mono_label(look, heading), content].spacing(10))
        .padding([14, 16])
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: fill(c.raised),
            border: border(c.border, 1.0, radius::LARGE),
            shadow: super::Shadows::drop_shadow(look.tokens.shadow.panel).unwrap_or_default(),
            ..Default::default()
        })
        .into()
}

/// The action bar (52 px) under the content: secondary actions on the left,
/// Vorige and Volgende on the right with the primary button at the far right.
pub fn action_bar<'a, M: 'a>(look: Look, left: Vec<Element<'a, M>>, right: Vec<Element<'a, M>>) -> Element<'a, M> {
    let c = look.color;
    let mut line = row![].spacing(8).padding([0, 16]).height(Length::Fill).align_y(Alignment::Center);
    for e in left {
        line = line.push(e);
    }
    line = line.push(Space::with_width(Length::Fill));
    for e in right {
        line = line.push(e);
    }
    column![
        bar(c.border, Length::Fill, 1.0),
        container(line).height(ACTION_BAR - 1.0).width(Length::Fill).style(move |_| container::Style { background: fill(c.panel), ..Default::default() }),
    ]
    .into()
}
