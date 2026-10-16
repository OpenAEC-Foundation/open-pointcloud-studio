//! Native document search controls, based on the pinned PDF Studio FindBar.
//! The host performs search and owns all state; this module owns geometry and
//! paints the five verified UI icon sites from that source component.
use crate::{widgets, Language, Theme};
use iced::{
    widget::{button, checkbox, column, container, row, scrollable, svg, text, text_input, Space},
    Alignment, Border, Element, Fill,
};

#[derive(Debug, Clone)]
pub struct ResultGroup {
    pub page: usize,
    pub count: usize,
    pub kinds: String,
    pub current: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FindBarState {
    pub query: String,
    pub current: usize,
    pub total: usize,
    pub searching: bool,
    pub match_case: bool,
    pub whole_word: bool,
    pub highlight_all: bool,
    pub search_text: bool,
    pub search_annotations: bool,
    pub results_open: bool,
    pub groups: Vec<ResultGroup>,
}

pub struct FindBarActions<M> {
    pub query_changed: fn(String) -> M,
    pub previous: M,
    pub next: M,
    pub toggle_match_case: M,
    pub toggle_whole_word: M,
    pub toggle_highlight_all: M,
    pub toggle_text: M,
    pub toggle_annotations: M,
    pub toggle_results: M,
    pub select_page: fn(usize) -> M,
    pub close: M,
}

#[derive(Clone, Copy)]
enum FindIcon {
    Previous,
    Next,
    WholeWord,
    HighlightAll,
    Close,
}
impl FindIcon {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Previous => include_bytes!("../assets/find-bar/previous.svg"),
            Self::Next => include_bytes!("../assets/find-bar/next.svg"),
            Self::WholeWord => include_bytes!("../assets/find-bar/whole-word.svg"),
            Self::HighlightAll => include_bytes!("../assets/find-bar/highlight-all.svg"),
            Self::Close => include_bytes!("../assets/find-bar/close.svg"),
        }
    }
    fn view<'a>(self, theme: Theme, selected: bool) -> svg::Svg<'a> {
        svg(svg::Handle::from_memory(self.bytes()))
            .width(if matches!(self, Self::Close) { 12 } else { 14 })
            .height(if matches!(self, Self::Close) { 12 } else { 14 })
            .style(move |_, _| svg::Style {
                color: Some(if selected {
                    theme.tokens().accent_text
                } else {
                    theme.tokens().text_secondary
                }),
            })
    }
}

fn control<'a, M: Clone + 'a>(
    icon: Option<FindIcon>,
    label: Option<&'static str>,
    hint: &'static str,
    theme: Theme,
    selected: bool,
    enabled: bool,
    message: M,
    width: u16,
) -> Element<'a, M> {
    let child: Element<'a, M> = if let Some(icon) = icon {
        icon.view(theme, selected).into()
    } else {
        text(label.unwrap_or_default()).size(12).into()
    };
    widgets::themed_tooltip(
        button(child)
            .width(width)
            .height(22)
            .padding(0)
            .on_press_maybe(enabled.then_some(message))
            .style(move |_, status| {
                let t = theme.tokens();
                let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                button::Style {
                    background: if selected {
                        Some(t.accent.into())
                    } else if hovered {
                        Some(t.hover.into())
                    } else {
                        None
                    },
                    text_color: if selected {
                        t.accent_text
                    } else {
                        t.text_secondary
                    },
                    border: Border {
                        color: if selected {
                            t.accent
                        } else {
                            iced::Color::TRANSPARENT
                        },
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..button::Style::default()
                }
            }),
        hint,
        iced::widget::tooltip::Position::Bottom,
        theme,
    )
    .into()
}

/// Viewport width is the document area, not the full window. At widths below
/// 370 px the option buttons move onto a second row so the query stays usable.
pub fn view<'a, M: Clone + 'a>(
    state: &'a FindBarState,
    actions: FindBarActions<M>,
    theme: Theme,
    language: Language,
    viewport_width: f32,
) -> Element<'a, M> {
    let t = theme.tokens();
    let compact = viewport_width < 370.0;
    let available = (viewport_width - 24.0).max(220.0);
    let query_width = if compact {
        (available - 80.0).max(90.0)
    } else {
        232.0
    };
    let nav_enabled = state.total > 0;
    let count = if state.searching {
        language.text("Zoeken…", "Searching…").to_owned()
    } else if state.total > 0 {
        format!(
            "{} {} {}",
            state.current,
            language.text("van", "of"),
            state.total
        )
    } else {
        String::new()
    };
    let search = row![
        text_input(
            language.text("Zoeken in document…", "Find in document…"),
            &state.query
        )
        .on_input(actions.query_changed)
        .size(12)
        .padding([3, 6])
        .width(query_width - 52.0)
        .style(move |_, status| widgets::text_input_style(theme, status)),
        container(text(count).size(10).color(t.text_secondary))
            .width(52)
            .height(24)
            .align_x(iced::alignment::Horizontal::Right)
            .align_y(iced::alignment::Vertical::Center)
            .padding([0, 5]),
    ]
    .spacing(0)
    .align_y(Alignment::Center);
    let input = container(search)
        .width(query_width)
        .height(24)
        .style(move |_| {
            container::Style::default()
                .background(t.dialog_input_bg)
                .border(Border {
                    color: if !state.query.is_empty() && !state.searching && state.total == 0 {
                        t.danger_color
                    } else {
                        t.dialog_input_border
                    },
                    width: 1.0,
                    radius: 0.0.into(),
                })
        });
    let previous = control(
        Some(FindIcon::Previous),
        None,
        language.text("Vorige (Shift+Enter)", "Previous (Shift+Enter)"),
        theme,
        false,
        nav_enabled,
        actions.previous,
        22,
    );
    let next = control(
        Some(FindIcon::Next),
        None,
        language.text("Volgende (Enter)", "Next (Enter)"),
        theme,
        false,
        nav_enabled,
        actions.next,
        22,
    );
    let case = control(
        None,
        Some("Aa"),
        language.text("Hoofdlettergevoelig", "Match case"),
        theme,
        state.match_case,
        true,
        actions.toggle_match_case,
        24,
    );
    let whole = control(
        Some(FindIcon::WholeWord),
        None,
        language.text("Hele woorden", "Whole words"),
        theme,
        state.whole_word,
        true,
        actions.toggle_whole_word,
        24,
    );
    let highlight = control(
        Some(FindIcon::HighlightAll),
        None,
        language.text("Alles markeren", "Highlight all"),
        theme,
        state.highlight_all,
        true,
        actions.toggle_highlight_all,
        24,
    );
    let close = control(
        Some(FindIcon::Close),
        None,
        language.text("Sluiten (Esc)", "Close (Esc)"),
        theme,
        false,
        true,
        actions.close,
        22,
    );
    let rule = container(Space::new(1, 16))
        .width(1)
        .height(16)
        .style(move |_| container::Style::default().background(t.border));
    let mut rows = column![].spacing(4);
    if compact {
        rows = rows.push(
            row![input, previous, next, close]
                .spacing(2)
                .align_y(Alignment::Center),
        );
        rows = rows.push(
            row![Space::new(2, 1), rule, case, whole, highlight]
                .spacing(2)
                .align_y(Alignment::Center),
        );
    } else {
        rows = rows.push(
            row![input, previous, next, rule, case, whole, highlight, close]
                .spacing(2)
                .align_y(Alignment::Center),
        );
    }
    let sources = row![
        checkbox(language.text("Tekst", "Text"), state.search_text)
            .on_toggle(move |_| actions.toggle_text.clone())
            .size(13)
            .text_size(11)
            .style(move |_, status| widgets::checkbox_style(theme, status)),
        checkbox(
            language.text("Annotaties", "Annotations"),
            state.search_annotations
        )
        .on_toggle(move |_| actions.toggle_annotations.clone())
        .size(13)
        .text_size(11)
        .style(move |_, status| widgets::checkbox_style(theme, status)),
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    rows = rows.push(sources);
    if !state.search_text && !state.search_annotations {
        rows = rows.push(
            text(language.text("Kies Tekst of Annotaties", "Select Text or Annotations"))
                .size(11)
                .color(t.danger_color),
        );
    }
    if !state.query.is_empty() && !state.searching && state.total == 0 {
        rows = rows.push(
            text(language.text("Geen resultaten gevonden", "No results found"))
                .size(11)
                .color(t.danger_color),
        );
    }
    if !state.groups.is_empty() {
        let caption = format!(
            "{} ({})",
            language.text("Resultaten", "Results"),
            state.total
        );
        rows = rows.push(
            button(
                text(format!(
                    "{} {caption}",
                    if state.results_open { "▾" } else { "▸" }
                ))
                .size(11),
            )
            .on_press(actions.toggle_results)
            .padding([3, 4])
            .style(move |_, status| widgets::secondary_button_style(theme, status)),
        );
        if state.results_open {
            let mut list = column![].spacing(1);
            for group in &state.groups {
                let title = format!(
                    "{} {}     {} {}     {}",
                    language.text("Pagina", "Page"),
                    group.page,
                    group.count,
                    if group.count == 1 {
                        language.text("treffer", "hit")
                    } else {
                        language.text("treffers", "hits")
                    },
                    group.kinds
                );
                let page = group.page;
                let current = group.current;
                list = list.push(
                    button(text(title).size(11))
                        .on_press((actions.select_page)(page))
                        .width(Fill)
                        .padding([3, 5])
                        .style(move |_, status| widgets::button_style(theme, current, status)),
                );
            }
            rows = rows.push(
                scrollable(list)
                    .height((state.groups.len() as f32 * 24.0).min(150.0))
                    .width(Fill),
            );
        }
    }
    container(rows)
        .padding(4)
        .width(if compact { available } else { 393.0 })
        .style(move |_| {
            container::Style::default()
                .background(t.surface)
                .border(Border {
                    color: t.border,
                    width: 1.0,
                    radius: 0.0.into(),
                })
                .shadow(iced::Shadow {
                    color: iced::Color::from_rgba8(0, 0, 0, 0.2),
                    offset: iced::Vector::new(0.0, 2.0),
                    blur_radius: 8.0,
                })
        })
        .into()
}
