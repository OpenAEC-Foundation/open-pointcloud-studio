//! UI 0.4 window chrome: the title bar with window buttons, the panel splitter
//! and menus (`design/ontwerp-0.4/04-componenten.md` and `05-layouts.md`).
//!
//! The application owns the window: it turns the messages from these parts into
//! `iced::window` tasks (drag, minimise, maximise, close) and keeps the panel
//! widths and the open menu in its state.
use super::widgets::{border, fill, glyph, icon_button, surface};
use super::{font, radius, shell as size, Look};
use crate::fonts;
use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, mouse_area, row, text, Space};
use iced::{mouse, Alignment, Border, Color, Element, Length, Padding, Point, Shadow, Size};

/// Messages of the window buttons and the drag area of the title bar.
#[derive(Debug, Clone)]
pub struct WindowActions<M> {
    /// Pressed on the empty title bar: start dragging (or maximise on a double click).
    pub drag: M,
    pub minimize: M,
    pub maximize: M,
    pub close: M,
    pub maximized: bool,
}

/// Icons of the window buttons, as B2 sources.
#[derive(Debug, Clone, Copy)]
pub struct WindowIcons {
    pub minimize: &'static str,
    pub maximize: &'static str,
    pub restore: &'static str,
    pub close: &'static str,
}

fn window_button<'a, M: Clone + 'a>(look: Look, icon: &'static str, on_press: M, close: bool) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    button(container(glyph(icon, 16.0, c.text, c.text)).center(Length::Fill))
        .width(40)
        .height(size::TITLEBAR)
        .padding(0)
        .on_press(on_press)
        .style(move |_, status| {
            use button::Status as S;
            let active = matches!(status, S::Hovered | S::Pressed);
            button::Style {
                // Close turns into the error colour, as people expect from a window.
                background: match status {
                    S::Hovered | S::Pressed if close && !hc => fill(c.error),
                    S::Pressed if !hc => fill(c.pressed),
                    S::Hovered if !hc => fill(c.hover),
                    _ => None,
                },
                text_color: c.text,
                border: if hc && active { border(c.accent, 2.0, 0.0) } else { Border::default() },
                shadow: Shadow::default(),
            }
        })
        .into()
}

/// The title bar (32 px) in `frame`: quick actions on the left, the name in the
/// middle, extra buttons and the window buttons on the right. The empty part
/// is the drag area of the window.
pub fn title_bar<'a, M: Clone + 'a>(
    look: Look,
    name: String,
    quick: Vec<Element<'a, M>>,
    extra: Vec<Element<'a, M>>,
    window: WindowActions<M>,
    icons: WindowIcons,
) -> Element<'a, M> {
    let c = look.color;
    let mut left = row![].spacing(2).align_y(Alignment::Center).padding(Padding { left: 4.0, ..Padding::ZERO });
    for e in quick {
        left = left.push(e);
    }
    let middle = mouse_area(
        container(text(name).size(font::SIZE_BODY).font(fonts::MEDIUM).color(c.text))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Horizontal::Center)
            .align_y(Vertical::Center),
    )
    .on_press(window.drag.clone());
    let mut right = row![].spacing(2).align_y(Alignment::Center);
    for e in extra {
        right = right.push(e);
    }
    let maximize_icon = if window.maximized { icons.restore } else { icons.maximize };
    right = right
        .push(Space::with_width(6))
        .push(window_button(look, icons.minimize, window.minimize, false))
        .push(window_button(look, maximize_icon, window.maximize, false))
        .push(window_button(look, icons.close, window.close, true));
    container(row![left, middle, right].height(Length::Fill).align_y(Alignment::Center))
        .height(size::TITLEBAR)
        .width(Length::Fill)
        .style(move |_| container::Style { background: fill(c.frame), ..Default::default() })
        .into()
}

/// A vertical splitter between a panel and the workspace: a 5 px handle with a
/// 1 px line, accent while dragging. Press starts the drag; the application
/// follows the cursor and ends the drag on release.
pub fn splitter<'a, M: Clone + 'a>(look: Look, active: bool, on_press: M) -> Element<'a, M> {
    let c = look.color;
    let line = if active { c.accent } else { c.border };
    let width = if active { 2.0 } else { 1.0 };
    mouse_area(
        container(container(Space::new(width, Length::Fill)).style(move |_| container::Style { background: fill(line), ..Default::default() }))
            .width(5)
            .height(Length::Fill)
            .center_x(5)
            .style(move |_| container::Style { background: fill(c.panel), ..Default::default() }),
    )
    .on_press(on_press)
    .interaction(mouse::Interaction::ResizingHorizontally)
    .into()
}

/// One line in a menu.
#[derive(Debug, Clone)]
pub enum MenuItem<'a, M> {
    Action {
        icon: Option<&'static str>,
        label: &'a str,
        shortcut: Option<&'a str>,
        /// Shows a check mark: the option is on.
        checked: bool,
        /// Delete and other irreversible actions, in the error colour.
        danger: bool,
        /// `None` disables the line.
        on_press: Option<M>,
    },
    Separator,
    Heading(&'a str),
}

impl<'a, M> MenuItem<'a, M> {
    pub fn action(icon: Option<&'static str>, label: &'a str, shortcut: Option<&'a str>, on_press: Option<M>) -> Self {
        Self::Action { icon, label, shortcut, checked: false, danger: false, on_press }
    }
    pub fn check(label: &'a str, shortcut: Option<&'a str>, checked: bool, on_press: M) -> Self {
        Self::Action { icon: None, label, shortcut, checked, danger: false, on_press: Some(on_press) }
    }
    pub fn danger(icon: Option<&'static str>, label: &'a str, shortcut: Option<&'a str>, on_press: Option<M>) -> Self {
        Self::Action { icon, label, shortcut, checked: false, danger: true, on_press }
    }
}

/// Menu width in pixels.
pub const MENU_WIDTH: f32 = 232.0;

/// A menu (or an open select list): `raised`, popup shadow and a 10 px corner.
/// Lines of 28 px with a 14 px icon, the shortcut on the right in mono and a check
/// mark for options that are on. Separators are lines in `border`.
pub fn menu<'a, M: Clone + 'a>(look: Look, items: Vec<MenuItem<'a, M>>, check_icon: &'static str) -> Element<'a, M> {
    let c = look.color;
    let hc = look.high_contrast();
    let mut list = column![];
    for item in items {
        match item {
            MenuItem::Separator => {
                list = list.push(container(container(Space::new(Length::Fill, 1)).style(move |_| container::Style { background: fill(c.border), ..Default::default() })).padding([4, 0]));
            }
            MenuItem::Heading(label) => {
                list = list.push(container(text(label.to_uppercase()).size(font::SIZE_LABEL).font(fonts::MONO).color(c.text_secondary)).height(24).padding([0, 10]).align_y(Vertical::Center));
            }
            MenuItem::Action { icon, label, shortcut, checked, danger, on_press } => {
                let disabled = on_press.is_none();
                let fg = if disabled { c.text_disabled } else if danger { c.error } else { c.text };
                let lead: Element<'a, M> = if checked {
                    glyph(check_icon, 14.0, if disabled { fg } else { c.accent_text }, c.accent_text)
                } else if let Some(source) = icon {
                    glyph(source, 14.0, fg, if disabled || danger { fg } else { c.accent_text })
                } else {
                    Space::with_width(14).into()
                };
                let mut line = row![lead, text(label).size(font::SIZE_BODY).font(fonts::BODY).color(fg).width(Length::Fill)].spacing(10).align_y(Alignment::Center);
                if let Some(keys) = shortcut {
                    line = line.push(text(keys).size(font::SIZE_LABEL).font(fonts::MONO).color(if disabled { c.text_disabled } else { c.text_secondary }));
                }
                list = list.push(
                    button(container(line).height(Length::Fill).align_y(Vertical::Center))
                        .height(28)
                        .width(Length::Fill)
                        .padding([0, 10])
                        .on_press_maybe(on_press)
                        .style(move |_, status| {
                            use button::Status as S;
                            let active = matches!(status, S::Hovered | S::Pressed);
                            button::Style {
                                background: match status {
                                    S::Hovered | S::Pressed if hc => None,
                                    S::Pressed => fill(if danger { super::mix(c.error, 0.16, c.raised) } else { c.pressed }),
                                    S::Hovered => fill(if danger { super::mix(c.error, 0.08, c.raised) } else { c.hover }),
                                    _ => None,
                                },
                                text_color: fg,
                                // Dense component: the focus and High contrast hover sit inside.
                                border: if hc && active { border(c.accent, 2.0, radius::SMALL) } else { border(Color::TRANSPARENT, 0.0, radius::SMALL) },
                                shadow: Shadow::default(),
                            }
                        }),
                );
            }
        }
    }
    container(list).width(MENU_WIDTH).padding(4).style(move |_| surface(look, look.tokens.shadow.popup, radius::LARGE)).into()
}

/// Rough height of a menu, to keep it inside the window.
pub fn menu_height<M>(items: &[MenuItem<'_, M>]) -> f32 {
    8.0 + items
        .iter()
        .map(|i| match i {
            MenuItem::Separator => 9.0,
            MenuItem::Heading(_) => 24.0,
            MenuItem::Action { .. } => 28.0,
        })
        .sum::<f32>()
}

/// A layer that shows `menu` at `at` (kept inside `window`) and closes it on a
/// click anywhere else. Push it on top of the screen in a stack.
pub fn menu_layer<'a, M: Clone + 'a>(menu: Element<'a, M>, at: Point, menu_size: Size, window: Size, on_dismiss: M) -> Element<'a, M> {
    let x = at.x.min(window.width - menu_size.width - 4.0).max(4.0);
    let y = if at.y + menu_size.height + 4.0 > window.height { (at.y - menu_size.height).max(4.0) } else { at.y };
    mouse_area(container(menu).padding(Padding { top: y, left: x, right: 0.0, bottom: 0.0 }).width(Length::Fill).height(Length::Fill))
        .on_press(on_dismiss.clone())
        .on_right_press(on_dismiss)
        .into()
}

/// A small icon button for the title bar (28 px), such as save, undo or settings.
pub fn title_button<'a, M: Clone + 'a>(look: Look, icon: &'static str, selected: bool, on_press: Option<M>) -> Element<'a, M> {
    icon_button(look, icon, 28.0, selected, on_press)
}
