//! Width-driven ribbon groups, adapted from OpenCAD Studio's
//! `src/ui/ribbon/collapse.rs` (commit 1fec34d). The native iced 0.13
//! implementation keeps each group's full controls alive in its flyout.
//!
//! Copyright OpenCADStudio contributors. Licensed under GPL-3.0.

use std::cell::RefCell;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Renderer as _, Shell};
use iced::widget::{button, column, container, row, text};
use iced::{
    event, Background, Border, Element, Event, Length, Point, Rectangle, Renderer, Size, Theme,
    Vector,
};

use super::TOOL_BAR_H;
use crate::{ui_style, ui_theme, Message};

/// A full-width panel and its compact title button. The title button opens
/// the same live controls beneath the ribbon when the group is collapsed.
pub struct AdaptivePanel<'a> {
    id: &'static str,
    full: Element<'a, Message>,
    collapsed: Element<'a, Message>,
}

impl<'a> AdaptivePanel<'a> {
    pub(crate) fn new(
        id: &'static str,
        short_label: &'static str,
        icon: Element<'a, Message>,
        full: Element<'a, Message>,
        open: bool,
    ) -> Self {
        let toggle = button(
            column![
                row![icon, text("⌄").size(10)]
                    .spacing(2)
                    .align_y(iced::Alignment::Center),
                text(crate::i18n::tr(short_label))
                    .size(8.5)
                    .font(crate::fonts::MEDIUM)
                    .wrapping(iced::widget::text::Wrapping::None),
            ]
            .spacing(2)
            .align_x(iced::Alignment::Center),
        )
        .on_press(Message::ToggleRibbonPanel(id))
        .style(move |theme, status| ui_style::ribbon_button(theme, open, status))
        .width(60)
        .height(58)
        .padding([3, 1]);
        Self {
            id,
            full,
            collapsed: container(toggle)
                .height(TOOL_BAR_H)
                .align_y(iced::Alignment::Center)
                .into(),
        }
    }
}

/// Keep all groups visible by collapsing complete groups from right to left
/// as soon as their measured widths no longer fit the available space.
/// The full group becomes an anchored flyout, including its real controls.
pub struct AdaptiveRibbon<'a> {
    panels: Vec<AdaptivePanel<'a>>,
    open: Option<&'static str>,
    collapsed: RefCell<Vec<bool>>,
}

impl<'a> AdaptiveRibbon<'a> {
    pub fn new(panels: Vec<AdaptivePanel<'a>>, open: Option<&'static str>) -> Self {
        let count = panels.len();
        Self {
            panels,
            open,
            collapsed: RefCell::new(vec![false; count]),
        }
    }

    fn slot(panel: usize, collapsed: bool) -> usize {
        2 * panel + usize::from(collapsed)
    }

    fn shown(&self, panel: usize, collapsed: bool) -> &Element<'a, Message> {
        if collapsed {
            &self.panels[panel].collapsed
        } else {
            &self.panels[panel].full
        }
    }

    fn shown_mut(&mut self, panel: usize, collapsed: bool) -> &mut Element<'a, Message> {
        if collapsed {
            &mut self.panels[panel].collapsed
        } else {
            &mut self.panels[panel].full
        }
    }
}

/// This is OpenCAD Studio's right-to-left degradation rule with two levels.
/// A few pixels of reserve avoid a one-pixel overflow toggling on resize.
fn choose_collapsed(widths: &[(f32, f32)], available: f32) -> Vec<bool> {
    let mut result = vec![false; widths.len()];
    let mut total: f32 = widths.iter().map(|(full, _)| full).sum();
    for index in (0..widths.len()).rev() {
        if total <= (available - 8.0).max(0.0) {
            break;
        }
        result[index] = true;
        total += widths[index].1 - widths[index].0;
    }
    result
}

/// Mirror OpenCAD Studio's final squeeze step when every group is already a
/// title button. Reclaim up to eight pixels of edge padding per gap.
fn collapsed_gap(widths: &[(f32, f32)], collapsed: &[bool], available: f32) -> f32 {
    if collapsed.len() < 2 || !collapsed.iter().all(|value| *value) {
        return 0.0;
    }
    let total: f32 = widths.iter().map(|(_, button)| button).sum();
    ((total - (available - 8.0).max(0.0)) / (collapsed.len() - 1) as f32).clamp(0.0, 8.0)
}

impl Widget<Message, Theme, Renderer> for AdaptiveRibbon<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(TOOL_BAR_H))
    }

    fn children(&self) -> Vec<Tree> {
        self.panels
            .iter()
            .flat_map(|panel| [Tree::new(&panel.full), Tree::new(&panel.collapsed)])
            .collect()
    }

    fn diff(&self, tree: &mut Tree) {
        let children: Vec<&Element<'_, Message>> = self
            .panels
            .iter()
            .flat_map(|panel| [&panel.full, &panel.collapsed])
            .collect();
        tree.diff_children(&children);
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let width = limits.max().width;
        let natural = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, TOOL_BAR_H));
        let widths: Vec<(f32, f32)> = self
            .panels
            .iter()
            .enumerate()
            .map(|(index, panel)| {
                let full = panel.full.as_widget().layout(
                    &mut tree.children[Self::slot(index, false)],
                    renderer,
                    &natural,
                );
                let collapsed = panel.collapsed.as_widget().layout(
                    &mut tree.children[Self::slot(index, true)],
                    renderer,
                    &natural,
                );
                (full.size().width, collapsed.size().width)
            })
            .collect();
        let chosen = choose_collapsed(&widths, width);
        let gap = collapsed_gap(&widths, &chosen, width);
        *self.collapsed.borrow_mut() = chosen.clone();
        let mut left = 0.0;
        let mut children = Vec::with_capacity(self.panels.len());
        for (index, collapsed) in chosen.into_iter().enumerate() {
            let node = self.shown(index, collapsed).as_widget().layout(
                &mut tree.children[Self::slot(index, collapsed)],
                renderer,
                &natural,
            );
            let height = node.size().height;
            children.push(node.move_to(Point::new(left, ((TOOL_BAR_H - height) / 2.0).max(0.0))));
            left += if collapsed {
                widths[index].1
            } else {
                widths[index].0
            };
            if index + 1 < self.panels.len() {
                left -= gap;
            }
        }
        layout::Node::with_children(Size::new(width, TOOL_BAR_H), children)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let collapsed = self.collapsed.borrow();
        for (index, child_layout) in layout.children().enumerate() {
            let shown = collapsed[index];
            self.shown(index, shown).as_widget().draw(
                &tree.children[Self::slot(index, shown)],
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let collapsed = self.collapsed.borrow();
        for (index, child_layout) in layout.children().enumerate() {
            let shown = collapsed[index];
            self.shown(index, shown).as_widget().operate(
                &mut tree.children[Self::slot(index, shown)],
                child_layout,
                renderer,
                operation,
            );
        }
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        let collapsed = self.collapsed.borrow().clone();
        for (index, child_layout) in layout.children().enumerate() {
            let shown = collapsed[index];
            let status = self.shown_mut(index, shown).as_widget_mut().on_event(
                &mut tree.children[Self::slot(index, shown)],
                event.clone(),
                child_layout,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            );
            if status == event::Status::Captured {
                return status;
            }
        }
        event::Status::Ignored
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let collapsed = self.collapsed.borrow();
        layout
            .children()
            .enumerate()
            .map(|(index, child_layout)| {
                let shown = collapsed[index];
                self.shown(index, shown).as_widget().mouse_interaction(
                    &tree.children[Self::slot(index, shown)],
                    child_layout,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .find(|interaction| *interaction != mouse::Interaction::Idle)
            .unwrap_or_default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let collapsed = self.collapsed.borrow().clone();
        if let Some(open) = self.open {
            if let Some(index) = self.panels.iter().position(|panel| panel.id == open) {
                if !collapsed[index] {
                    return None;
                }
                let button = layout.children().nth(index)?.bounds();
                let anchor = Point::new(
                    button.x + translation.x,
                    button.y + button.height + translation.y,
                );
                return Some(overlay::Element::new(Box::new(Flyout {
                    content: &mut self.panels[index].full,
                    tree: &mut tree.children[Self::slot(index, false)],
                    anchor,
                    ribbon_bottom: layout.bounds().y + layout.bounds().height + translation.y,
                })));
            }
        }

        let mut overlays = Vec::new();
        let mut trees = tree.children.as_mut_slice();
        for ((index, panel), child_layout) in
            self.panels.iter_mut().enumerate().zip(layout.children())
        {
            let (slots, rest) = trees.split_at_mut(2);
            trees = rest;
            let shown = collapsed[index];
            let child = if shown {
                &mut panel.collapsed
            } else {
                &mut panel.full
            };
            if let Some(overlay) = child.as_widget_mut().overlay(
                &mut slots[usize::from(shown)],
                child_layout,
                renderer,
                translation,
            ) {
                overlays.push(overlay);
            }
        }
        (!overlays.is_empty()).then(|| overlay::Group::with_children(overlays).overlay())
    }
}

impl<'a> From<AdaptiveRibbon<'a>> for Element<'a, Message> {
    fn from(ribbon: AdaptiveRibbon<'a>) -> Self {
        Element::new(ribbon)
    }
}

struct Flyout<'a, 'b> {
    content: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    anchor: Point,
    ribbon_bottom: f32,
}

impl overlay::Overlay<Message, Theme, Renderer> for Flyout<'_, '_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let limits = layout::Limits::new(Size::ZERO, bounds);
        let child = self
            .content
            .as_widget()
            .layout(self.tree, renderer, &limits);
        let size = child.size();
        let x = self.anchor.x.min((bounds.width - size.width).max(0.0));
        let y = if self.anchor.y + size.height > bounds.height {
            (self.anchor.y - size.height).max(0.0)
        } else {
            self.anchor.y
        };
        layout::Node::with_children(size, vec![child]).move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        let bounds = layout.bounds();
        let colors = ui_theme::colors(theme);
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: colors.border,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..renderer::Quad::default()
            },
            Background::Color(colors.bg),
        );
        let child = layout.children().next().expect("flyout content");
        self.content.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            child,
            cursor,
            &child.bounds(),
        );
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let child = layout.children().next().expect("flyout content");
        self.content
            .as_widget()
            .operate(self.tree, child, renderer, operation);
    }

    fn on_event(
        &mut self,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) -> event::Status {
        let child = layout.children().next().expect("flyout content");
        if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_)))
            && !cursor.is_over(child.bounds())
        {
            // The underlying ribbon handles its own buttons. Publishing a
            // close message as well would close the newly opened group (or
            // reopen this one), depending on the message dispatch order.
            if cursor
                .position()
                .is_some_and(|position| position.y < self.ribbon_bottom)
            {
                return event::Status::Ignored;
            }
            shell.publish(Message::CloseRibbonPanel);
            return event::Status::Captured;
        }
        self.content.as_widget_mut().on_event(
            self.tree,
            event,
            child,
            cursor,
            renderer,
            clipboard,
            shell,
            &child.bounds(),
        )
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let child = layout.children().next().expect("flyout content");
        self.content
            .as_widget()
            .mouse_interaction(self.tree, child, cursor, viewport, renderer)
    }
}

#[cfg(test)]
mod tests {
    use super::{choose_collapsed, collapsed_gap};

    #[test]
    fn full_panels_collapse_from_the_right_only_as_needed() {
        let widths = [(200.0, 70.0), (180.0, 60.0), (100.0, 50.0)];
        assert_eq!(choose_collapsed(&widths, 500.0), [false, false, false]);
        assert_eq!(choose_collapsed(&widths, 440.0), [false, false, true]);
        assert_eq!(choose_collapsed(&widths, 320.0), [false, true, true]);
        assert_eq!(choose_collapsed(&widths, 160.0), [true, true, true]);
    }

    #[test]
    fn collapsed_buttons_reclaim_their_edge_padding_at_narrow_widths() {
        let widths = [(200.0, 70.0), (180.0, 60.0), (100.0, 50.0)];
        assert_eq!(collapsed_gap(&widths, &[true, true, true], 170.0), 8.0);
        assert_eq!(collapsed_gap(&widths, &[true, true, true], 190.0), 0.0);
        assert_eq!(collapsed_gap(&widths, &[false, true, true], 100.0), 0.0);
    }
}
