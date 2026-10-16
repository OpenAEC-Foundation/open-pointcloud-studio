//! Canonical controls for domain panels. Use these instead of restyling buttons per app.
use crate::{
    dual_range::DualRange,
    fonts,
    icons::Icon,
    model::{metrics, CommandIcon, RibbonControl, RibbonFormField},
    Theme,
};
use iced::{
    widget::{
        button, checkbox, column, container, pick_list, progress_bar, row,
        scrollable, slider, stack, svg, text, text_input, tooltip, Space,
    },
    Alignment, Background, Border, Element, Fill, Length, Padding,
};
pub use crate::popover::{popover, popover_aligned};

/// A destination in the shared File/Backstage navigation.
pub struct BackstagePage<M> {
    pub label: String,
    pub message: M,
    pub active: bool,
    pub enabled: bool,
}

/// Application-owned actions shown below the shared page list.
pub struct BackstageFooter<M> {
    pub settings: (String, M),
    pub back: (String, M),
    pub exit: (String, M),
}

pub const BACKSTAGE_TILE_WIDTH: f32 = 620.0;

/// The 260 px File menu used by desktop studios. Page order, labels and
/// messages belong to the application; row geometry and colours belong here.
pub fn backstage_nav_with<'a, M: Clone + 'a, F>(
    task_pages: Vec<BackstagePage<M>>,
    other_pages: Vec<BackstagePage<M>>,
    footer: BackstageFooter<M>,
    resolve_theme: F,
) -> Element<'a, M>
where
    F: Fn(&iced::Theme) -> Theme + Copy + 'a,
{
    let rule = || {
        container(Space::new(Fill, 1)).style(move |theme| {
            container::Style::default().background(resolve_theme(theme).tokens().border_subtle)
        })
    };
    let page = |entry: BackstagePage<M>| {
        let active = entry.active;
        row![
            container(Space::new(3, Fill)).style(move |theme| {
                container::Style::default().background(if active {
                    resolve_theme(theme).tokens().accent
                } else {
                    iced::Color::TRANSPARENT
                })
            }),
            button(text(entry.label).size(14))
                .on_press_maybe(entry.enabled.then_some(entry.message))
                .style(move |theme, status| {
                    ribbon_button_style(resolve_theme(theme), active, status)
                })
                .width(Fill)
                .height(Fill)
                .padding([11, 15]),
        ]
        .height(40)
    };
    let pages = |entries: Vec<BackstagePage<M>>| {
        entries.into_iter().fold(column![].width(Fill), |list, entry| list.push(page(entry)))
    };
    let item = |label: String, message: M| {
        button(text(label).size(14))
            .on_press(message)
            .style(move |theme, status| {
                ribbon_button_style(resolve_theme(theme), false, status)
            })
            .width(Fill)
            .padding([11, 18])
    };
    let top = column![
        Space::new(Fill, 14),
        pages(task_pages),
        container(rule()).padding([10, 0]),
        pages(other_pages),
    ]
    .width(Fill);
    let footer = column![
        rule(),
        item(footer.settings.0, footer.settings.1),
        button(text(footer.back.0).size(13).font(fonts::STRONG))
            .on_press(footer.back.1)
            .style(move |theme, status| {
                ribbon_button_style(resolve_theme(theme), false, status)
            })
            .width(Fill)
            .padding([13, 18]),
        rule(),
        item(footer.exit.0, footer.exit.1),
    ]
    .width(Fill);
    container(column![
        scrollable(top)
            .height(Fill)
            .style(move |theme, status| scrollable_style(resolve_theme(theme), status)),
        footer,
    ]
    .height(Fill))
    .width(260)
    .height(Fill)
    .style(move |theme| {
        let t = resolve_theme(theme).tokens();
        container::Style::default()
            .background(t.bg_lighter)
            .border(Border { color: t.border, width: 1.0, radius: 0.0.into() })
    })
    .into()
}

/// Title and lead of a File page; page content remains application-owned.
pub fn backstage_page_heading<'a, M: 'a>(
    title: impl Into<String>,
    lead: impl Into<String>,
    theme: Theme,
) -> Element<'a, M> {
    column![
        text(title.into()).size(22).font(fonts::HEADING),
        text(lead.into()).size(13).color(theme.tokens().text_secondary),
    ]
    .spacing(6)
    .into()
}

pub fn backstage_group_caption<'a, M: 'a>(
    label: impl Into<String>,
    theme: Theme,
) -> Element<'a, M> {
    container(text(label.into()).size(10).color(theme.tokens().text_muted))
        .padding(Padding { top: 26.0, bottom: 6.0, ..Padding::ZERO })
        .into()
}

/// A File action tile with the same available, unavailable and hover states
/// in every studio. The action and its explanatory text stay app-owned.
pub fn backstage_task_tile_with<'a, M: Clone + 'a, F>(
    title: impl Into<String>,
    detail: impl Into<String>,
    action: Option<M>,
    resolve_theme: F,
) -> Element<'a, M>
where
    F: Fn(&iced::Theme) -> Theme + Copy + 'a,
{
    button(
        column![
            text(title.into()).size(15).font(fonts::STRONG),
            text(detail.into()).size(12).style(move |theme| text::Style {
                color: Some(resolve_theme(theme).tokens().text_secondary),
            }),
        ]
        .spacing(3),
    )
    .on_press_maybe(action)
    .style(move |theme, status| {
        let shared = resolve_theme(theme);
        let mut style = ribbon_button_style(shared, false, status);
        let t = shared.tokens();
        style.border.color = t.border;
        style.border.width = 1.0;
        style.border.radius = 4.0.into();
        if style.background.is_none() {
            style.background = Some(t.accent_soft.into());
        }
        style
    })
    .width(Fill)
    .padding([12, 16])
    .into()
}

pub fn backstage_tile_column<'a, M: 'a>(
    tiles: impl IntoIterator<Item = Element<'a, M>>,
) -> Element<'a, M> {
    container(iced::widget::Column::with_children(tiles).spacing(8).width(Fill))
        .width(Fill)
        .max_width(BACKSTAGE_TILE_WIDTH)
        .into()
}

pub fn slider_style(theme: Theme, status: slider::Status) -> slider::Style {
    let t = theme.tokens();
    slider::Style {
        rail: slider::Rail {
            backgrounds: (t.accent.into(), theme.border_strong().into()),
            width: 4.0,
            border: Border::default().rounded(2),
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 7.0 },
            background: if status == slider::Status::Dragged {
                t.accent
            } else {
                t.bg_lighter
            }
            .into(),
            border_width: 2.0,
            border_color: if status == slider::Status::Hovered {
                t.accent_hover
            } else {
                t.accent
            },
        },
    }
}

pub fn progress_style(theme: Theme) -> progress_bar::Style {
    progress_bar::Style {
        background: theme.border_strong().into(),
        bar: theme.tokens().accent.into(),
        border: Border::default().rounded(4),
    }
}

pub fn progress_line_style(theme: Theme) -> progress_bar::Style {
    progress_bar::Style {
        border: Border::default().rounded(1),
        ..progress_style(theme)
    }
}

pub fn checkbox_style(theme: Theme, status: checkbox::Status) -> checkbox::Style {
    let t = theme.tokens();
    let (checked, hovered, disabled) = match status {
        checkbox::Status::Active { is_checked } => (is_checked, false, false),
        checkbox::Status::Hovered { is_checked } => (is_checked, true, false),
        checkbox::Status::Disabled { is_checked } => (is_checked, false, true),
    };
    let (background, border) = if checked {
        (t.accent, t.accent)
    } else if hovered {
        (t.dialog_input_bg, t.dialog_content_secondary)
    } else {
        (t.dialog_input_bg, t.dialog_input_border)
    };
    let opacity = if disabled { 0.5 } else { 1.0 };
    checkbox::Style {
        background: background.scale_alpha(opacity).into(),
        icon_color: t.btn_primary_text,
        border: Border {
            color: border.scale_alpha(opacity),
            width: 1.0,
            radius: 2.0.into(),
        },
        text_color: Some(t.text.scale_alpha(opacity)),
    }
}

pub fn scrollable_style(theme: Theme, status: scrollable::Status) -> scrollable::Style {
    let t = theme.tokens();
    let (horizontal, vertical) = match status {
        scrollable::Status::Active => (false, false),
        scrollable::Status::Hovered {
            is_horizontal_scrollbar_hovered,
            is_vertical_scrollbar_hovered,
        } => (
            is_horizontal_scrollbar_hovered,
            is_vertical_scrollbar_hovered,
        ),
        scrollable::Status::Dragged {
            is_horizontal_scrollbar_dragged,
            is_vertical_scrollbar_dragged,
        } => (
            is_horizontal_scrollbar_dragged,
            is_vertical_scrollbar_dragged,
        ),
    };
    let rail = |strong: bool| scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            color: t.accent.scale_alpha(if strong { 0.40 } else { 0.25 }),
            border: Border::default().rounded(3),
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail(vertical),
        horizontal_rail: rail(horizontal),
        gap: None,
    }
}

pub fn tooltip_style(_theme: Theme) -> container::Style {
    container::Style::default()
        .background(iced::Color::from_rgb8(0x36, 0x36, 0x3E))
        .color(iced::Color::from_rgb8(0xFA, 0xFA, 0xF9))
        .border(Border::default().rounded(4))
}

/// Shared tooltip content for ribbon, title strip and domain controls.
pub fn tooltip_box<'a, M: 'a>(label: impl text::IntoFragment<'a>, theme: Theme) -> Element<'a, M> {
    container(text(label).size(12))
        .padding([8, 12])
        .max_width(340)
        .style(move |_| tooltip_style(theme))
        .into()
}

/// A tooltip with the same typography, width and eight-pixel separation in every studio.
pub fn themed_tooltip<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    label: impl text::IntoFragment<'a>,
    position: tooltip::Position,
    theme: Theme,
) -> tooltip::Tooltip<'a, M> {
    tooltip::Tooltip::new(content, tooltip_box(label, theme), position).gap(8.0)
}

/// Square text field shared by panel forms and dialogs.
pub fn text_input_style(theme: Theme, status: text_input::Status) -> text_input::Style {
    let t = theme.tokens();
    let mut style = text_input::Style {
        background: t.dialog_input_bg.into(),
        border: Border {
            color: if status == text_input::Status::Focused {
                t.focus_color
            } else {
                t.dialog_input_border
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        icon: t.text_muted,
        placeholder: t.text_faint,
        value: t.dialog_input_text,
        selection: t.accent_soft,
    };
    if status == text_input::Status::Disabled {
        style.background = style.background.scale_alpha(0.5);
        style.border.color = style.border.color.scale_alpha(0.5);
        style.value = style.value.scale_alpha(0.5);
    }
    style
}

/// Labelled application-owned form field for full-page screens.
pub fn form_field<'a, M: Clone + 'a>(
    theme: Theme,
    label: impl Into<String>,
    value: impl Into<String>,
    on_input: impl Fn(String) -> M + 'a,
    width: Length,
) -> Element<'a, M> {
    let t = theme.tokens();
    let value = value.into();
    column![
        text(label.into()).font(fonts::MEDIUM).size(12).color(t.text_secondary),
        text_input("", &value).on_input(on_input)
            .font(fonts::BODY).size(13).padding([11, 14]).width(Fill)
            .style(move |_, status| text_input_style(theme, status)),
    ].spacing(7).width(width).into()
}

/// Full-page empty collection surface used by application workflow screens.
pub fn empty_collection<'a, M: Clone + 'a>(
    theme: Theme,
    title: impl Into<String>,
    action_label: impl Into<String>,
    empty_label: impl Into<String>,
    on_action: M,
    width: Length,
) -> Element<'a, M> {
    let t = theme.tokens();
    container(column![
        text(title.into()).font(fonts::HEADING).size(29).color(t.text),
        button(text(action_label.into()).font(fonts::STRONG).size(13))
            .padding([12, 22]).on_press(on_action)
            .style(move |_, status| primary_button_style(theme, status)),
        container(text(empty_label.into()).size(14).color(t.text_secondary))
            .width(Fill).height(88).align_x(Alignment::Center).align_y(Alignment::Center),
    ].spacing(26)).padding(32).width(width).height(300)
        .style(move |_| container::Style::default().background(t.bg_lighter)
            .border(Border { color: t.border, width: 1.0, radius: 10.0.into() }))
        .into()
}

/// Choice field and its popup use the same tokens as text fields.
pub fn pick_list_style(theme: Theme, status: pick_list::Status) -> pick_list::Style {
    let t = theme.tokens();
    pick_list::Style {
        text_color: t.dialog_input_text,
        placeholder_color: t.text_faint,
        handle_color: t.dialog_input_text.scale_alpha(0.6),
        background: t.dialog_input_bg.into(),
        border: Border {
            color: match status {
                pick_list::Status::Active => t.dialog_input_border,
                pick_list::Status::Hovered => t.dialog_content_secondary,
                pick_list::Status::Opened => t.focus_color,
            },
            width: 1.0,
            radius: 0.0.into(),
        },
    }
}

pub fn pick_list_menu_style(theme: Theme) -> iced::overlay::menu::Style {
    let t = theme.tokens();
    iced::overlay::menu::Style {
        background: Background::Color(t.bg),
        border: Border {
            color: t.dialog_input_border,
            width: 1.0,
            radius: 0.0.into(),
        },
        text_color: t.dialog_input_text,
        selected_text_color: t.accent,
        selected_background: Background::Color(t.ribbon_btn_hover),
    }
}

/// Filled confirmation button used by dialogs and forms in every studio.
pub fn primary_button_style(theme: Theme, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let style = button::Style {
        background: Some(
            if hovered {
                t.btn_primary_hover_bg
            } else {
                t.btn_primary_bg
            }
            .into(),
        ),
        text_color: if hovered {
            t.btn_primary_hover_text
        } else {
            t.btn_primary_text
        },
        border: Border {
            color: t.btn_primary_border,
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    };
    if status == button::Status::Disabled {
        faded_button(style, 0.5)
    } else {
        style
    }
}

/// Neutral companion button used by dialogs and forms in every studio.
pub fn secondary_button_style(theme: Theme, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let style = button::Style {
        background: Some(
            if hovered {
                t.btn_secondary_hover_bg
            } else {
                t.btn_secondary_bg
            }
            .into(),
        ),
        text_color: t.btn_secondary_text,
        border: Border {
            color: if hovered {
                t.btn_secondary_hover_border
            } else {
                t.dialog_input_border
            },
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    };
    if status == button::Status::Disabled {
        faded_button(style, 0.5)
    } else {
        style
    }
}

/// Text link used for source references and secondary navigation.
pub fn link_button_style(theme: Theme, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let style = button::Style {
        text_color: if hovered { t.accent_hover } else { t.accent },
        ..Default::default()
    };
    if status == button::Status::Disabled {
        faded_button(style, 0.5)
    } else {
        style
    }
}

/// Flat action on the common status bar.
pub fn status_button_style(theme: Theme, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: hovered.then_some(t.status_hover.into()),
        text_color: if status == button::Status::Disabled {
            t.status_text_label
        } else {
            t.status_text
        },
        ..Default::default()
    }
}

/// Shared dialog-button geometry: 11 px text, a 27 px line and a 75 px
/// minimum width. The caller supplies its theme adapter to the style hook.
pub fn dialog_button<'a, M: 'a>(
    label: impl text::IntoFragment<'a>,
    style: impl Fn(&iced::Theme, button::Status) -> button::Style + 'a,
) -> button::Button<'a, M> {
    const PADDING: [u16; 2] = [5, 20];
    const MIN_WIDTH: f32 = 75.0;
    const LINE_HEIGHT: f32 = 17.0;
    button(
        column![
            text(label)
                .size(11)
                .font(fonts::BODY)
                .line_height(text::LineHeight::Absolute(LINE_HEIGHT.into())),
            iced::widget::Space::with_width(MIN_WIDTH - 2.0 * f32::from(PADDING[1])),
        ]
        .align_x(iced::Alignment::Center),
    )
    .padding(PADDING)
    .style(style)
}

fn faded_button(style: button::Style, alpha: f32) -> button::Style {
    button::Style {
        background: style.background.map(|color| color.scale_alpha(alpha)),
        text_color: style.text_color.scale_alpha(alpha),
        border: Border {
            color: style.border.color.scale_alpha(alpha),
            ..style.border
        },
        ..style
    }
}

pub fn button_style(theme: Theme, selected: bool, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let disabled = matches!(status, button::Status::Disabled);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(
            if selected {
                t.ribbon_btn_active_bg
            } else if hovered {
                t.ribbon_btn_hover
            } else {
                t.bg
            }
            .into(),
        ),
        text_color: if disabled {
            t.text_faint
        } else if selected {
            t.ribbon_btn_active_text
        } else {
            t.text
        },
        border: Border {
            color: if selected {
                t.ribbon_btn_active_border
            } else if hovered {
                t.ribbon_btn_hover_border
            } else {
                iced::Color::TRANSPARENT
            },
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    }
}
/// Canonical ribbon control colors, including hover, selection, and disabled contrast.
pub fn ribbon_button_style(theme: Theme, selected: bool, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (background, text_color, border) = if selected {
        (
            Some(t.ribbon_btn_active_bg),
            t.ribbon_btn_active_text,
            t.ribbon_btn_active_border,
        )
    } else if hovered {
        (
            Some(t.ribbon_btn_hover),
            t.ribbon_text_hover,
            t.ribbon_btn_hover_border,
        )
    } else {
        (None, t.text, iced::Color::TRANSPARENT)
    };
    let style = button::Style {
        background: background.map(Into::into),
        text_color,
        border: Border {
            color: border,
            width: 1.0,
            radius: 2.0.into(),
        },
        ..Default::default()
    };
    if status == button::Status::Disabled {
        button::Style {
            background: style.background.map(|color| color.scale_alpha(0.72)),
            text_color: style.text_color.scale_alpha(0.72),
            border: Border {
                color: style.border.color.scale_alpha(0.72),
                ..style.border
            },
            ..style
        }
    } else {
        style
    }
}

/// Flat 28 px ribbon-tab strip, shared by the showcase and desktop tools.
/// The active tab takes the work-surface colour; hover uses the theme's
/// ribbon hover token. Geometry belongs to the caller's tab button.
pub fn ribbon_tab_style(theme: Theme, active: bool, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        background: if active {
            Some(t.bg.into())
        } else if hovered {
            Some(t.ribbon_btn_hover.into())
        } else {
            None
        },
        text_color: if active || hovered { t.accent } else { t.text },
        ..Default::default()
    }
}

/// The source Style Book's 28 px tab, including the active outline and the
/// gap that masks the continuous accent rule. Callers own labels and actions.
#[derive(Clone, Copy)]
pub enum RibbonTabKind { Regular { active: bool }, File }

pub fn ribbon_tab<'a, M: Clone + 'a>(
    content: Element<'a, M>,
    message: M,
    width: f32,
    kind: RibbonTabKind,
    theme: Theme,
) -> Element<'a, M> {
    let tokens = theme.tokens();
    let width = width.max(32.0);
    let outline = match kind {
        RibbonTabKind::Regular { active: true } => tokens.accent,
        RibbonTabKind::File => tokens.file_tab_bg,
        RibbonTabKind::Regular { active: false } => iced::Color::TRANSPARENT,
    };
    let gap = match kind {
        RibbonTabKind::Regular { active: true } => tokens.bg,
        RibbonTabKind::File => tokens.file_tab_bg,
        RibbonTabKind::Regular { active: false } => iced::Color::TRANSPARENT,
    };
    let edge = || container(Space::new(1, Fill)).width(1).style(move |_| {
        container::Style::default().background(outline)
    });
    let face = button(content)
        .on_press(message)
        .width(Fill)
        .height(25)
        .padding([3, 6])
        .style(move |_, status| match kind {
            RibbonTabKind::Regular { active } => ribbon_tab_style(theme, active, status),
            RibbonTabKind::File => button::Style {
                background: Some(if status == button::Status::Hovered {
                    tokens.file_tab_hover.into()
                } else {
                    tokens.file_tab_bg.into()
                }),
                text_color: tokens.file_tab_text,
                ..button::Style::default()
            },
        });
    let top = container(container(Space::new(width - 8.0, 1)).style(move |_| {
        container::Style::default().background(outline)
    }))
    .width(width)
    .height(1)
    .center_x(Fill);
    let bottom = container(Space::new(Fill, 2)).height(2).style(move |_| {
        container::Style::default().background(gap)
    });
    column![top, row![edge(), face, edge()].height(25), bottom]
        .width(width)
        .height(metrics::TABS)
        .into()
}

/// Continuous amber rule behind the tab gaps, with the Style Book's 8 px inset.
pub fn ribbon_tab_strip<'a, M: 'a>(content: Element<'a, M>, theme: Theme) -> Element<'a, M> {
    let tokens = theme.tokens();
    let baseline = column![
        Space::new(Fill, metrics::TABS - 1),
        container(Space::new(Fill, 1)).style(move |_| {
            container::Style::default().background(tokens.accent)
        }),
    ]
    .height(metrics::TABS);
    container(stack![baseline, container(content).padding([0, 8]).height(metrics::TABS)])
        .width(Fill)
        .height(metrics::TABS)
        .style(move |_| container::Style::default().background(tokens.bg_lighter))
        .into()
}

/// Tauri Style Book ribbon group: 94 px including the 4 px top inset,
/// 8 px sides, a ruled caption and a right separator. `width` is the
/// measured total group width, including both insets and the separator.
pub fn ribbon_group<'a, M: 'a>(
    theme: Theme,
    label: impl Into<String>,
    tools: Element<'a, M>,
    width: f32,
) -> Element<'a, M> {
    let t = theme.tokens();
    let width = width.max(50.0);
    let body = container(tools)
        .width(Fill)
        .height(69)
        .align_x(iced::Alignment::Center)
        .align_y(iced::Alignment::Center);
    let rule = container(iced::widget::Space::new(Fill, 1))
        .width(Fill)
        .style(move |_| container::Style::default().background(t.border_subtle));
    let caption = container(
        text(label.into().to_uppercase())
            .font(fonts::MEDIUM)
            .size(9)
            .color(t.ribbon_group_label),
    )
    .width(Fill)
    .height(20)
    .center_x(Fill)
    .align_y(iced::Alignment::Center);
    let content = container(column![body, rule, caption])
        .width(Length::Fixed(width - 1.0))
        .height(metrics::RIBBON)
        .padding(Padding {
            top: 4.0,
            right: 8.0,
            bottom: 0.0,
            left: 8.0,
        });
    let separator = container(iced::widget::Space::new(1, 90))
        .style(move |_| container::Style::default().background(t.ribbon_group_separator));
    row![content, separator]
        .align_y(iced::Alignment::End)
        .width(Length::Fixed(width))
        .height(metrics::RIBBON)
        .into()
}

/// Typed ribbon input using the shared group height and a source-specific width.
/// Values and actions remain app-owned.
#[derive(Clone)]
struct RibbonChoice { index: usize, label: String }
impl std::fmt::Display for RibbonChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.label.fmt(formatter)
    }
}
impl PartialEq for RibbonChoice {
    fn eq(&self, other: &Self) -> bool { self.index == other.index }
}

pub fn ribbon_control<'a, M: Clone + 'a>(
    theme: Theme,
    label: impl Into<String>,
    control: RibbonControl<M>,
) -> Element<'a, M> {
    let t = theme.tokens();
    let title = label.into();
    let width = control.width();
    let compact = matches!(control, RibbonControl::FormStack { .. });
    let dense_form = matches!(&control, RibbonControl::FormStack { fields, .. } if fields.len() >= 4);
    let body: Element<'a, M> = match control {
        RibbonControl::Slider { minimum, maximum, value, step, unit, on_change } => {
            column![
                row![
                    text(title).font(fonts::BODY).size(10).color(t.text),
                    iced::widget::horizontal_space(),
                    text(format!("{value:.0}{unit}")).font(fonts::STRONG).size(10).color(t.text),
                ],
                slider(minimum..=maximum, value, on_change)
                    .step(step)
                    .width(158)
                    .style(move |_, status| slider_style(theme, status)),
            ]
            .spacing(3)
            .into()
        }
        RibbonControl::Range { minimum, maximum, lower, upper, step, unit, on_lower, on_upper } => {
            column![
                row![
                    text(title).font(fonts::BODY).size(10).color(t.text),
                    iced::widget::horizontal_space(),
                    text(format!("{lower:.0}–{upper:.0}{unit}")).font(fonts::STRONG).size(10).color(t.text),
                ],
                Element::new(DualRange { theme, minimum, maximum, lower, upper, step, on_lower, on_upper }),
            ]
            .spacing(3)
            .into()
        }
        RibbonControl::Segmented { options, selected } => {
            let mut choices = iced::widget::Row::new().spacing(1);
            for (index, (name, message)) in options.into_iter().enumerate() {
                let active = selected == index;
                let caption: Element<'a, M> = if let Some((first, second)) = name.split_once('\n') {
                    column![text(first.to_owned()).size(9), text(second.to_owned()).size(9)]
                        .align_x(Alignment::Center)
                        .into()
                } else {
                    text(name).size(9).into()
                };
                choices = choices.push(
                    button(caption)
                        .padding([2, 2])
                        .width(79)
                        .height(28)
                        .on_press(message)
                        .style(move |_, status| ribbon_button_style(theme, active, status)),
                );
            }
            column![text(title).font(fonts::BODY).size(10).color(t.text), choices]
                .spacing(3)
                .into()
        }
        RibbonControl::Select { options, selected, show_label, .. } => {
            let choices: Vec<RibbonChoice> = options.iter().enumerate()
                .map(|(index, (label, _))| RibbonChoice { index, label: label.clone() })
                .collect();
            let current = choices.get(selected).cloned();
            let field = pick_list(choices, current, move |choice: RibbonChoice| options[choice.index].1.clone())
                .font(fonts::MEDIUM)
                .text_size(11)
                .padding([4, 6])
                .width(width - 12.0)
                .style(move |_, status| pick_list_style(theme, status))
                .menu_style(move |_| pick_list_menu_style(theme));
            if !show_label { field.into() }
            else { column![text(title).font(fonts::BODY).size(10).color(t.text), field].spacing(3).into() }
        }
        RibbonControl::Sliders { items, minimum, maximum, step, unit, .. } => {
            let mut rows = iced::widget::Column::new().spacing(4);
            for item in items {
                rows = rows.push(row![
                    item.icon.view(14, t.text),
                    text(item.label).font(fonts::BODY).size(11).color(t.text).width(58),
                    slider(minimum..=maximum, item.value, item.on_change)
                        .step(step).width(96).height(18)
                        .style(move |_, status| slider_style(theme, status)),
                    text(format!("{:.0}{unit}", item.value)).font(fonts::MEDIUM).size(11).color(t.text)
                        .width(34).align_x(iced::alignment::Horizontal::Right),
                ].spacing(4).align_y(Alignment::Center).height(20));
            }
            rows.into()
        }
        RibbonControl::Number { value, on_change, show_label, .. } => {
            let field = text_input("", &value)
                .on_input(on_change)
                .font(fonts::MEDIUM)
                .size(11)
                .padding([4, 6])
                .style(move |_, status| text_input_style(theme, status));
            if show_label {
                column![text(title).font(fonts::BODY).size(10).color(t.text), field]
                    .spacing(3).into()
            } else { field.into() }
        }
        RibbonControl::FormStack { fields, label_width, .. } => {
            let mut form = iced::widget::Column::new().spacing(1);
            let field_width = (width - 16.0 - label_width).max(60.0);
            let row_height = if dense_form { 16 } else { 19 };
            let padding = if dense_form { [0, 2] } else { [1, 3] };
            for field in fields {
                let (label, input): (String, Element<'a, M>) = match field {
                    RibbonFormField::Number { label, value, on_change } => {
                        let input = text_input("", &value).on_input(on_change)
                            .font(fonts::MEDIUM).size(11).padding(padding).width(field_width)
                            .style(move |_, status| text_input_style(theme, status));
                        (label, input.into())
                    }
                    RibbonFormField::Select { label, options, selected } => {
                        let choices: Vec<RibbonChoice> = options.iter().enumerate()
                            .map(|(index, (label, _))| RibbonChoice { index, label: label.clone() })
                            .collect();
                        let current = choices.get(selected).cloned();
                        let input = pick_list(choices, current,
                            move |choice: RibbonChoice| options[choice.index].1.clone())
                            .font(fonts::MEDIUM).text_size(11).padding(padding).width(field_width)
                            .style(move |_, status| pick_list_style(theme, status))
                            .menu_style(move |_| pick_list_menu_style(theme));
                        (label, input.into())
                    }
                    RibbonFormField::Slider { label, minimum, maximum, value, step,
                        value_text, unit, on_change, on_input } => {
                        let track = slider(minimum..=maximum, value, on_change)
                            .step(step).width(96).height(14)
                            .style(move |_, status| slider_style(theme, status));
                        let value = text_input("", &value_text).on_input(on_input)
                            .font(fonts::MEDIUM).size(11).padding([0, 2]).width(34)
                            .style(move |_, status| text_input_style(theme, status));
                        (label, row![track, value, text(unit).size(11).color(t.text)]
                            .spacing(2).align_y(Alignment::Center).into())
                    }
                };
                form = form.push(row![
                    text(label).font(fonts::BODY).size(11).color(t.text).width(label_width), input,
                ].spacing(4).align_y(Alignment::Center).height(row_height));
            }
            form.into()
        }
        RibbonControl::ColorSelect { options, selected, current, rows_per_column, open, on_toggle, on_close, none, custom, .. } => {
            let byte = |value: f32| (value * 255.0).round() as u8;
            let color_label = format!("#{:02x}{:02x}{:02x}", byte(current.r), byte(current.g), byte(current.b));
            let swatch = container(Space::new(16, 16)).style(move |_| container::Style::default()
                .background(current)
                .border(Border { color: theme.border_strong(), width: 1.0, radius: 1.0.into() }));
            let anchor = button(row![swatch, text(color_label).font(fonts::MEDIUM).size(11).color(t.text),
                text("▾").size(11).color(t.text_secondary)].spacing(5).align_y(Alignment::Center))
                .padding([2, 4]).width(width - 12.0).height(24)
                .on_press(on_toggle)
                .style(move |_, status| ribbon_button_style(theme, open, status));
            let selected_swatch = (selected < options.len()).then_some(selected);
            let grid = color_grid(theme, options, rows_per_column, selected_swatch);
            let has_actions = none.is_some() || custom.is_some();
            let mut actions = iced::widget::Row::new().spacing(4);
            if let Some((name, message)) = none {
                actions = actions.push(button(text(name).font(fonts::BODY).size(11))
                    .padding([3, 6]).on_press(message)
                    .style(move |_, status| ribbon_button_style(theme, false, status)));
            }
            if let Some(editor) = &custom {
                let editing = editor.editing;
                actions = actions.push(button(text(editor.label.clone()).font(fonts::BODY).size(11))
                    .padding([3, 6]).on_press(editor.on_toggle.clone())
                    .style(move |_, status| ribbon_button_style(theme, editing, status)));
            }
            let mut content = iced::widget::Column::new().spacing(5).push(grid);
            if has_actions {
                content = content.push(container(Space::new(Fill, 1))
                    .style(move |_| container::Style::default().background(t.border_subtle)))
                    .push(actions);
            }
            if let Some(editor) = custom.filter(|editor| editor.editing) {
                let field = text_input("#RRGGBB", &editor.draft)
                    .on_input(editor.on_input).font(fonts::MEDIUM).size(11)
                    .padding([4, 6]).width(Fill)
                    .style(move |_, status| text_input_style(theme, status));
                let mut apply = button(text("Toepassen").font(fonts::BODY).size(11))
                    .padding([4, 6]).width(Fill)
                    .style(move |_, status| ribbon_button_style(theme, false, status));
                if editor.valid { apply = apply.on_press(editor.on_apply); }
                content = content.push(column![field, apply].spacing(4));
            }
            let palette = container(content)
                .padding(6).width(170)
                .style(move |_| container::Style::default().background(t.bg_lighter)
                    .border(Border { color: theme.border_strong(), width: 1.0, radius: 3.0.into() }));
            column![text(title).font(fonts::BODY).size(10).color(t.text),
                popover(anchor.into(), palette.into(), open, on_close)]
                .spacing(3).into()
        }
        RibbonControl::StyleGallery { items, selected, visible, open, on_toggle, on_close } => {
            let tile = |item: crate::model::RibbonSwatch<M>, active: bool, width: f32| -> Element<'a, M> {
                let ink = item.color;
                let bg = item.background;
                let dashed = item.dashed_border;
                let sample = container(text("T").font(fonts::STRONG).size(18).color(ink))
                    .width(40).height(36).center(Fill)
                    .style(move |_| container::Style::default().background(bg)
                        .border(Border { color: ink, width: if dashed { 0.0 } else { 2.0 }, radius: 0.0.into() }));
                let sample: Element<'a, M> = if dashed {
                    let channel = |value: f32| (value * 255.0).round() as u8;
                    let border = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="40" height="36" viewBox="0 0 40 36"><rect x="1" y="1" width="38" height="34" fill="none" stroke="#{:02x}{:02x}{:02x}" stroke-width="2" stroke-dasharray="4 3"/></svg>"##,
                        channel(ink.r), channel(ink.g), channel(ink.b));
                    stack![sample, svg(svg::Handle::from_memory(border.into_bytes())).width(40).height(36)].into()
                } else { sample.into() };
                let label = text(item.label).font(fonts::BODY).size(9).color(t.text);
                button(column![sample, label].spacing(2).align_x(Alignment::Center))
                    .padding(2).width(width).height(62).on_press(item.message)
                    .style(move |_, status| ribbon_button_style(theme, active, status)).into()
            };
            let mut gallery = iced::widget::Row::new().spacing(2);
            for (index, item) in items.iter().take(visible).cloned().enumerate() {
                gallery = gallery.push(tile(item, selected == index, 50.0));
            }
            gallery = gallery.push(button(text("›").font(fonts::BODY).size(18).color(t.text_secondary))
                .width(18).height(62).on_press(on_toggle)
                .style(move |_, status| ribbon_button_style(theme, open, status)));
            let mut grid = iced::widget::Column::new().spacing(2);
            for (row_index, chunk) in items.chunks(5).enumerate() {
                let mut row = iced::widget::Row::new().spacing(2);
                for (column_index, item) in chunk.iter().cloned().enumerate() {
                    row = row.push(tile(item, selected == row_index * 5 + column_index, 56.0));
                }
                grid = grid.push(row);
            }
            let popup = container(grid).padding(6).width(300)
                .style(move |_| container::Style::default().background(t.bg_lighter)
                    .border(Border { color: theme.border_strong(), width: 1.0, radius: 3.0.into() }));
            popover_aligned(gallery.into(), popup.into(), open, on_close, true)
        }
    };
    container(body)
        .padding(if compact { [0, 6] } else { [4, 6] })
        .width(width)
        .height(if dense_form { 68 } else { 62 })
        .align_y(Alignment::Center)
        .into()
}

/// Compact palette grid for app-owned colors and actions. `rows_per_column`
/// preserves source palettes that are arranged in vertical columns.
pub fn color_grid<'a, M: Clone + 'a>(
    theme: Theme,
    options: Vec<(String, iced::Color, M)>,
    rows_per_column: usize,
    selected: Option<usize>,
) -> Element<'a, M> {
    let t = theme.tokens();
    let mut columns = iced::widget::Row::new().spacing(2);
    for (column_index, items) in options.chunks(rows_per_column.max(1)).enumerate() {
        let mut column = iced::widget::Column::new().spacing(1);
        for (row_index, (label, color, message)) in items.iter().enumerate() {
            let index = column_index * rows_per_column.max(1) + row_index;
            let chosen = selected == Some(index);
            let fill = *color;
            let cell = button(Space::new(14, 14))
                .padding(1).width(18).height(18)
                .on_press(message.clone())
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
                    button::Style {
                        background: Some(fill.into()),
                        border: Border { color: if chosen { t.accent } else if hovered { t.text }
                            else { theme.border_strong() }, width: if chosen { 2.0 } else { 1.0 },
                            radius: 1.0.into() },
                        ..Default::default()
                    }
                });
            column = column.push(tooltip(cell, text(label.clone()).size(11), tooltip::Position::Bottom));
        }
        columns = columns.push(column);
    }
    container(columns).padding(2).into()
}

/// A 24 px document tab from the pinned Tauri Style Book. The caller owns
/// its contents and actions; this style owns its palette and outline.
pub fn document_tab_style(theme: Theme, active: bool, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let over_docbar = |overlay: iced::Color| iced::Color {
        r: t.docbar_bg.r * (1.0 - overlay.a) + overlay.r * overlay.a,
        g: t.docbar_bg.g * (1.0 - overlay.a) + overlay.g * overlay.a,
        b: t.docbar_bg.b * (1.0 - overlay.a) + overlay.b * overlay.a,
        a: 1.0,
    };
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(if active {
            over_docbar(t.accent_soft).into()
        } else if hovered {
            over_docbar(t.hover).into()
        } else {
            t.docbar_bg.into()
        }),
        text_color: if active {
            t.accent
        } else if hovered {
            t.text
        } else {
            over_docbar(t.text_muted)
        },
        border: Border {
            color: if active { t.accent } else { over_docbar(t.border) },
            width: 1.0,
            radius: iced::border::Radius::default().top(3),
        },
        ..button::Style::default()
    }
}

/// Cover Iced's uniform lower rule with the document-band surface. The Tauri
/// tab reserves a transparent 2 px bottom border; its contents and actions
/// remain owned by the first child.
pub fn document_tab_bottom_gap<'a, M: 'a>(theme: Theme, tab: Element<'a, M>) -> Element<'a, M> {
    let surface = theme.tokens().docbar_bg;
    let gap = container(
        container(Space::new(Fill, 2))
            .width(Fill)
            .height(2)
            .style(move |_| container::Style::default().background(surface)),
    )
    .width(Fill)
    .height(24)
    .align_y(Alignment::End);
    stack![tab, gap].into()
}

/// Close glyph inside a shared document tab. Active and hovered glyphs use
/// full-strength ink so the 12 px action remains readable on every theme.
pub fn document_close_style(theme: Theme, active: bool, status: button::Status) -> button::Style {
    let t = theme.tokens();
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let base = if active {
        iced::Color {
            r: t.docbar_bg.r * (1.0 - t.accent_soft.a) + t.accent_soft.r * t.accent_soft.a,
            g: t.docbar_bg.g * (1.0 - t.accent_soft.a) + t.accent_soft.g * t.accent_soft.a,
            b: t.docbar_bg.b * (1.0 - t.accent_soft.a) + t.accent_soft.b * t.accent_soft.a,
            a: 1.0,
        }
    } else {
        t.docbar_bg
    };
    let background = if hovered {
        let overlay = t.ribbon_btn_hover;
        iced::Color {
            r: base.r * (1.0 - overlay.a) + overlay.r * overlay.a,
            g: base.g * (1.0 - overlay.a) + overlay.g * overlay.a,
            b: base.b * (1.0 - overlay.a) + overlay.b * overlay.a,
            a: 1.0,
        }
    } else {
        iced::Color::TRANSPARENT
    };
    button::Style {
        background: hovered.then_some(background.into()),
        text_color: if active || hovered { t.text } else { t.text_muted },
        border: Border::default().rounded(2),
        ..button::Style::default()
    }
}
pub fn surface(theme: Theme) -> container::Style {
    let t = theme.tokens();
    container::Style::default().background(t.bg).color(t.text)
}
pub fn panel_header<'a, M: 'a>(title: impl Into<String>, theme: Theme) -> Element<'a, M> {
    let t = theme.tokens();
    let title = container(
        text(title.into().to_uppercase())
            .font(fonts::STRONG)
            .size(11),
    )
    .padding([0, 6])
    .height(metrics::PANEL_TOOLBAR - 1)
    .width(Fill)
    .align_y(iced::Alignment::Center)
    .style(move |_| {
        container::Style::default()
            .background(t.bg_lighter)
            .color(t.accent)
    });
    let divider = container(iced::widget::Space::new(Fill, 1))
        .height(1)
        .width(Fill)
        .style(move |_| container::Style::default().background(t.border));
    column![title, divider]
        .height(metrics::PANEL_TOOLBAR)
        .width(Fill)
        .into()
}
pub fn panel<'a, M: 'a>(
    title: impl Into<String>,
    content: Element<'a, M>,
    theme: Theme,
) -> Element<'a, M> {
    container(column![
        panel_header(title, theme),
        iced::widget::scrollable(container(content).padding(12).width(Fill))
            .height(Fill)
            .style(move |_, status| scrollable_style(theme, status)),
    ])
    .width(Fill)
    .height(Fill)
    .style(move |_| surface(theme))
    .into()
}
pub fn action<'a, M: Clone + 'a>(
    label: impl Into<String>,
    icon: impl Into<CommandIcon>,
    message: Option<M>,
    theme: Theme,
) -> Element<'a, M> {
    let icon = icon.into();
    let color = if message.is_some() {
        theme.tokens().text
    } else {
        theme.tokens().text_faint
    };
    button(
        row![
            icon.view(metrics::ICON, color),
            text(label.into()).font(fonts::BODY).size(13)
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center),
    )
    .padding([5, 10])
    .height(metrics::CONTROL)
    .on_press_maybe(message)
    .style(move |_, status| button_style(theme, false, status))
    .into()
}
/// One title-strip action for every studio. The application supplies its
/// semantic icon and command; the shared crate owns size, hover and tooltip.
pub fn title_action<'a, M: Clone + 'a>(
    icon: Element<'a, M>,
    label: impl Into<String>,
    message: Option<M>,
    theme: Theme,
) -> Element<'a, M> {
    let control = button(container(icon).width(Fill).height(Fill).center(Fill))
        .width(30)
        .height(26)
        .padding([1, 0])
        .on_press_maybe(message)
        .style(move |_, status| title_action_style(theme, status));
    themed_tooltip(control, label.into(), tooltip::Position::Bottom, theme)
    .gap(90)
    .into()
}

pub fn title_action_style(theme: Theme, status: button::Status) -> button::Style {
    let tokens = theme.tokens();
    button::Style {
        background: matches!(status, button::Status::Hovered)
            .then_some(Background::Color(tokens.ribbon_btn_hover)),
        text_color: tokens.text,
        ..button::Style::default()
    }
}
fn modal_header_background(theme: Theme) -> Background {
    let t = theme.tokens();
    iced::gradient::Linear::new(iced::Degrees(180.0))
        .add_stop(0.0, t.dialog_header_bg)
        .add_stop(1.0, t.dialog_bg)
        .into()
}
/// Source Style Book modal chrome with an application-owned form and actions.
/// The overlay is inert; applications keep their own close and Escape messages.
pub fn dialog_modal<'a, M: Clone + 'a>(
    title: impl Into<String>,
    body: Element<'a, M>,
    footer: Element<'a, M>,
    close: M,
    width: f32,
    theme: Theme,
) -> Element<'a, M> {
    let t = theme.tokens();
    let header = container(row![
        text(title.into()).font(fonts::STRONG).size(12).color(t.dialog_header_text).width(Fill),
        button(text("×").size(16))
            .on_press(close)
            .width(30).height(30).padding(0)
            .style(move |_, status| button::Style {
                background: matches!(status, button::Status::Hovered).then_some(t.ribbon_btn_hover.into()),
                text_color: t.dialog_content_text,
                ..button::Style::default()
            }),
    ].align_y(Alignment::Center))
        .height(30).width(Fill)
        .padding(Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 10.0 })
        .style(move |_| container::Style::default().background(modal_header_background(theme)));
    let content = container(body)
        .width(Fill)
        .style(move |_| container::Style::default().background(t.dialog_content_bg).color(t.dialog_content_text));
    let footer = column![
        container(Space::new(Fill, 1)).height(1).width(Fill)
            .style(move |_| container::Style::default().background(t.dialog_border)),
        container(footer).padding([8, 12]).height(43).width(Fill)
            .style(move |_| container::Style::default().background(t.dialog_sidebar_bg)),
    ];
    let card = container(column![header, content, footer].width(width))
        .width(width)
        .style(move |_| container::Style {
            background: Some(t.dialog_bg.into()),
            border: Border { color: t.dialog_border, width: 1.0, radius: 0.0.into() },
            shadow: theme.dialog_shadow(),
            ..container::Style::default()
        });
    crate::modal::modal(card.into(), theme)
}
/// Source Style Book settings chrome (560 × 500). Applications provide their
/// own tabs, fields, footer commands and close action. The backdrop is inert.
pub fn settings_modal<'a, M: Clone + 'a>(
    title: impl Into<String>,
    sidebar: Element<'a, M>,
    content: Element<'a, M>,
    footer: Element<'a, M>,
    close: M,
    theme: Theme,
) -> Element<'a, M> {
    let t = theme.tokens();
    let header = container(row![
        text(title.into())
            .font(fonts::STRONG)
            .size(12)
            .color(t.dialog_header_text)
            .width(Fill),
        button(text("×").size(16))
            .on_press(close)
            .width(30)
            .height(30)
            .padding(0)
            .style(move |_, status| button::Style {
                background: matches!(status, button::Status::Hovered)
                    .then_some(Background::Color(t.ribbon_btn_hover)),
                text_color: t.dialog_content_text,
                ..button::Style::default()
            }),
    ].align_y(Alignment::Center))
        .width(Fill)
        .height(30)
        .padding(Padding { top: 0.0, right: 0.0, bottom: 0.0, left: 10.0 })
        .style(move |_| container::Style::default().background(modal_header_background(theme)));
    let sidebar = container(scrollable(sidebar).height(Fill))
        .width(120)
        .height(Fill)
        .padding([8, 0])
        .style(move |_| container::Style::default().background(t.dialog_sidebar_bg));
    let rule = container(Space::new(1, Fill))
        .width(1)
        .height(Fill)
        .style(move |_| container::Style::default().background(t.dialog_sidebar_border));
    let body = row![
        sidebar,
        rule,
        scrollable(container(content).padding(12).width(Fill)).height(Fill).width(Fill),
    ]
    .height(Fill)
    .width(Fill);
    let footer_rule = container(Space::new(Fill, 1))
        .height(1)
        .width(Fill)
        .style(move |_| container::Style::default().background(t.dialog_border));
    let footer = column![
        footer_rule,
        container(footer)
            .padding([8, 12])
            .height(43)
            .width(Fill)
            .style(move |_| container::Style::default().background(t.dialog_sidebar_bg)),
    ]
    .height(44);
    let card = container(column![header, body, footer].height(500).width(560))
        .width(560)
        .height(500)
        .style(move |_| container::Style {
            background: Some(t.dialog_bg.into()),
            border: Border { color: t.dialog_border, width: 1.0, radius: 0.0.into() },
            shadow: theme.dialog_shadow(),
            ..container::Style::default()
        });
    crate::modal::modal(card.into(), theme)
}
/// The source template's 340 px reset confirmation above an open settings dialog.
pub fn settings_reset_confirmation<'a, M: Clone + 'a>(
    title: impl Into<String>,
    question: impl Into<String>,
    cancel_label: impl Into<String>,
    reset_label: impl Into<String>,
    cancel: M,
    reset: M,
    theme: Theme,
) -> Element<'a, M> {
    let body = container(text(question.into()).size(12).width(Fill))
        .padding(12).width(Fill).into();
    let footer = row![
        Space::with_width(Fill),
        dialog_button(cancel_label.into(), move |_, status| secondary_button_style(theme, status))
            .on_press(cancel.clone()),
        dialog_button(reset_label.into(), move |_, status| primary_button_style(theme, status))
            .on_press(reset),
    ].spacing(6).align_y(Alignment::Center).into();
    dialog_modal(title, body, footer, cancel, 340.0, theme)
}
/// Vertical settings navigation from the pinned Style Book template.
pub fn settings_tab<'a, M: Clone + 'a>(
    label: impl Into<String>,
    active: bool,
    on_press: M,
    theme: Theme,
) -> Element<'a, M> {
    let t = theme.tokens();
    button(text(label.into()).size(11).font(fonts::MEDIUM))
        .on_press(on_press)
        .width(Fill)
        .padding([8, 12])
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if active { Some(t.dialog_tab_active_bg.into()) }
                    else if hovered { Some(t.dialog_tab_hover.into()) } else { None },
                text_color: if active { t.dialog_tab_active_text }
                    else if hovered { t.dialog_tab_hover_text } else { t.dialog_tab_text },
                ..button::Style::default()
            }
        })
        .into()
}
/// One source-sized option in the settings theme menu. The caller owns the
/// selection and supplies its localized label and action.
pub fn settings_theme_option<'a, M: Clone + 'a>(
    label: impl Into<String>,
    option: Theme,
    selected: bool,
    on_press: M,
    current_theme: Theme,
) -> Element<'a, M> {
    let t = current_theme.tokens();
    let swatches = option.swatches().into_iter().fold(row![].spacing(2), |row, color| {
        row.push(container(Space::new(14, 14)).style(move |_| container::Style {
            background: Some(color.into()),
            border: Border { color: iced::Color::from_rgba8(128, 128, 128, 0.3), width: 1.0, radius: 0.0.into() },
            ..container::Style::default()
        }))
    });
    button(row![swatches, text(label.into()).size(11).font(fonts::MEDIUM)]
        .spacing(8).align_y(Alignment::Center))
        .on_press(on_press)
        .height(26)
        .width(Fill)
        .padding([5, 8])
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: if selected { Some(t.dialog_tab_active_bg.into()) }
                    else if hovered { Some(t.dialog_tab_hover.into()) } else { None },
                text_color: if selected { t.dialog_tab_active_text } else { t.dialog_content_text },
                ..button::Style::default()
            }
        })
        .into()
}
/// Expanded 200 px theme menu shown in the pinned Style Book reference.
pub fn settings_theme_menu<'a, M: Clone + 'a>(
    options: impl IntoIterator<Item = (Theme, String, M)>,
    selected: Theme,
    current_theme: Theme,
) -> Element<'a, M> {
    let t = current_theme.tokens();
    let items = options.into_iter().fold(column![], |items, (theme, label, message)| {
        items.push(settings_theme_option(label, theme, theme == selected, message, current_theme))
    });
    container(items)
        .width(200)
        .style(move |_| container::Style {
            background: Some(t.dialog_input_bg.into()),
            border: Border { color: t.dialog_input_border, width: 1.0, radius: 0.0.into() },
            ..container::Style::default()
        })
        .into()
}
/// Tauri-style theme selector: 200 px trigger and an expandable swatch menu.
pub fn settings_theme_picker<'a, M: Clone + 'a>(
    options: Vec<(Theme, String, M)>,
    selected: Theme,
    current_theme: Theme,
    open: bool,
    on_toggle: M,
) -> Element<'a, M> {
    let t = current_theme.tokens();
    let label = options.iter().find(|(theme, _, _)| *theme == selected)
        .map(|(_, label, _)| label.clone()).unwrap_or_else(|| selected.to_string());
    let swatches = selected.swatches().into_iter().fold(row![].spacing(2), |row, color| {
        row.push(container(Space::new(14, 14)).style(move |_| container::Style {
            background: Some(color.into()),
            border: Border { color: iced::Color::from_rgba8(128, 128, 128, 0.3), width: 1.0, radius: 0.0.into() },
            ..container::Style::default()
        }))
    });
    let trigger = button(row![
        swatches,
        text(label).size(11).font(fonts::MEDIUM),
        Space::with_width(Fill),
        text("⌄").size(11).color(t.dialog_content_text),
    ].spacing(8).align_y(Alignment::Center))
        .on_press(on_toggle.clone())
        .width(200)
        .height(27)
        .padding([5, 8])
        .style(move |_, status| button::Style {
            background: Some(t.dialog_input_bg.into()),
            text_color: t.dialog_content_text,
            border: Border {
                color: if open { t.focus_color } else if matches!(status, button::Status::Hovered) { t.dialog_content_secondary } else { t.dialog_input_border },
                width: 1.0,
                radius: 0.0.into(),
            },
            ..button::Style::default()
        });
    popover(trigger.into(), settings_theme_menu(options, selected, current_theme), open, on_toggle)
}
/// Section title and underline from SettingsDialog.css.
pub fn settings_section_heading<'a, M: 'a>(
    label: impl Into<String>,
    theme: Theme,
) -> Element<'a, M> {
    let t = theme.tokens();
    column![
        text(label.into()).font(fonts::STRONG).size(11).color(t.dialog_content_text),
        container(Space::new(Fill, 1))
            .height(1)
            .width(Fill)
            .style(move |_| container::Style::default().background(t.dialog_section_border)),
    ]
    .spacing(4)
    .width(Fill)
    .into()
}
pub fn icon_button<'a, M: Clone + 'a>(
    label: &'a str,
    icon: Icon,
    message: M,
    theme: Theme,
) -> Element<'a, M> {
    themed_tooltip(
        button(icon.view(16, theme.tokens().text))
            .padding(7)
            .on_press(message)
            .style(move |_, s| button_style(theme, false, s)),
        label,
        tooltip::Position::Bottom,
        theme,
    )
    .into()
}
pub fn status_icon_button<'a, M: Clone + 'a>(
    label: &'a str,
    icon: Icon,
    message: M,
    theme: Theme,
) -> Element<'a, M> {
    let t = theme.tokens();
    themed_tooltip(
        button(icon.view(14, t.status_text))
            .padding([3, 7])
            .on_press(message)
            .style(move |_, status| status_button_style(theme, status)),
        label,
        tooltip::Position::Top,
        theme,
    )
    .into()
}
pub fn empty_state<'a, M: 'a>(
    title: impl Into<String>,
    description: impl Into<String>,
    theme: Theme,
) -> Element<'a, M> {
    container(
        column![
            Icon::Cube.view(48, theme.tokens().accent),
            text(title.into()).font(fonts::HEADING).size(28),
            text(description.into()).font(fonts::BODY).size(14)
        ]
        .spacing(16)
        .align_x(iced::Alignment::Center),
    )
    .center(Fill)
    .into()
}
pub fn property<'a, M: 'a>(
    label: impl Into<String>,
    value: impl Into<String>,
    theme: Theme,
) -> Element<'a, M> {
    row![
        text(label.into())
            .size(12)
            .color(theme.tokens().text_secondary)
            .width(Fill),
        text(value.into()).size(12).font(fonts::MEDIUM)
    ]
    .spacing(12)
    .into()
}

/// A compact 28 px property-grid row. The application supplies the field;
/// the crate owns the common label/value columns and bottom rule.
pub fn property_grid_control_with<'a, M: 'a, F>(
    label: impl Into<String>,
    control: Element<'a, M>,
    resolve_theme: F,
) -> Element<'a, M>
where
    F: Fn(&iced::Theme) -> Theme + Copy + 'a,
{
    let label = container(text(label.into()).font(fonts::BODY).size(12).style(move |theme| {
        text::Style {
            color: Some(resolve_theme(theme).tokens().text_secondary),
        }
    }))
    .width(Length::FillPortion(5))
    .height(27)
    .padding([0, 8])
    .align_y(iced::Alignment::Center);
    let value = container(control)
        .width(Length::FillPortion(6))
        .height(27)
        .padding([0, 8])
        .align_y(iced::Alignment::Center)
        .style(move |theme| {
            container::Style::default().color(resolve_theme(theme).tokens().text)
        });
    let rule = container(iced::widget::Space::new(Fill, 1))
        .width(Fill)
        .style(move |theme| {
            container::Style::default().background(resolve_theme(theme).tokens().border_subtle)
        });
    column![row![label, value].height(27), rule]
        .height(28)
        .width(Fill)
        .into()
}

pub fn property_grid_row_with<'a, M: 'a, F>(
    label: impl Into<String>,
    value: impl Into<String>,
    resolve_theme: F,
) -> Element<'a, M>
where
    F: Fn(&iced::Theme) -> Theme + Copy + 'a,
{
    property_grid_control_with(
        label,
        text(value.into()).font(fonts::BODY).size(12).into(),
        resolve_theme,
    )
}

pub fn property_grid_row<'a, M: 'a>(
    label: impl Into<String>,
    value: impl Into<String>,
    theme: Theme,
) -> Element<'a, M> {
    property_grid_row_with(label, value, move |_| theme)
}

pub fn property_section_with<'a, M: 'a, F>(
    title: impl Into<String>,
    resolve_theme: F,
) -> Element<'a, M>
where
    F: Fn(&iced::Theme) -> Theme + Copy + 'a,
{
    container(text(title.into()).font(fonts::STRONG).size(12))
        .width(Fill)
        .height(24)
        .padding([0, 6])
        .align_y(iced::Alignment::Center)
        .style(move |theme| {
            let t = resolve_theme(theme).tokens();
            container::Style::default().background(t.bg_lighter).color(t.text)
        })
        .into()
}

pub fn property_section<'a, M: 'a>(title: impl Into<String>, theme: Theme) -> Element<'a, M> {
    property_section_with(title, move |_| theme)
}
