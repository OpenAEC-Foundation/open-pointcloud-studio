//! The shared styles of the window, after the desktop shell of the OpenAEC
//! style book: buttons, text fields, choice lists with their menu,
//! checkboxes, sliders, progress bars, scroll bars and tooltips, each in the
//! tokens of the theme, and a cursor that stays the arrow over buttons.
//!
//! The constructors here are those of iced with the style of the style book
//! already set, so that a part of the window is styled by using them; a
//! source test holds that no other part of the window makes these widgets
//! itself.

use std::borrow::Borrow;
use std::ops::RangeInclusive;

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{tree, Operation, Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::widget::{
    button, checkbox as check_box, container, pick_list as choice, progress_bar as progress,
    scrollable as scrolling, slider as slide, text, text_input as field, tooltip as tip_widget,
};
use iced::{
    event, mouse, Background, Border, Color, Element, Event, Length, Point, Rectangle, Renderer,
    Shadow, Size, Theme, Vector,
};

use crate::ui_theme::{self, UiColors};
use crate::Message;

/// What a control that cannot be used keeps of its colours: the template
/// draws buttons and fields that wait at half their strength, and the
/// buttons of the ribbon at 40 %.
const DISABLED: f32 = 0.5;
const RIBBON_DISABLED: f32 = 0.4;

fn colors(theme: &Theme) -> UiColors {
    ui_theme::colors(theme)
}

/// `style` at `share` of its strength.
fn faded(style: button::Style, share: f32) -> button::Style {
    button::Style {
        background: style
            .background
            .map(|background| background.scale_alpha(share)),
        text_color: style.text_color.scale_alpha(share),
        border: Border {
            color: style.border.color.scale_alpha(share),
            ..style.border
        },
        shadow: style.shadow,
    }
}

fn hovered(status: button::Status) -> bool {
    matches!(status, button::Status::Hovered | button::Status::Pressed)
}

/// The button that goes on: Save, Create, Next, Run. Filled with the accent
/// colour, Signal Orange under the pointer. A button is made in it with
/// [`primary_button`], which gives it its size too.
pub fn primary(theme: &Theme, status: button::Status) -> button::Style {
    let colors = colors(theme);
    let (background, text_color) = if hovered(status) {
        (colors.btn_primary_hover_bg, colors.btn_primary_hover_text)
    } else {
        (colors.btn_primary_bg, colors.btn_primary_text)
    };
    let style = button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border: Border {
            color: colors.btn_primary_border,
            width: 1.0,
            radius: 2.0.into(),
        },
        shadow: Shadow::default(),
    };
    if status == button::Status::Disabled {
        faded(style, DISABLED)
    } else {
        style
    }
}

/// Every other button of a dialog or a form: Cancel, Back, Reset. A button
/// is made in it with [`secondary_button`].
pub fn secondary(theme: &Theme, status: button::Status) -> button::Style {
    let colors = colors(theme);
    let (background, border) = if hovered(status) {
        (
            colors.btn_secondary_hover_bg,
            colors.btn_secondary_hover_border,
        )
    } else {
        (colors.btn_secondary_bg, colors.dialog_input_border)
    };
    let style = button::Style {
        background: Some(Background::Color(background)),
        text_color: colors.btn_secondary_text,
        border: Border {
            color: border,
            width: 1.0,
            radius: 2.0.into(),
        },
        shadow: Shadow::default(),
    };
    if status == button::Status::Disabled {
        faded(style, DISABLED)
    } else {
        style
    }
}

/// A button of the ribbon, and a small action in a panel drawn as one:
/// transparent with a transparent border, tinted and outlined in the accent
/// colour under the pointer and while it is on.
pub fn ribbon_button(theme: &Theme, active: bool, status: button::Status) -> button::Style {
    let colors = colors(theme);
    let (background, text_color, border) = if active {
        (
            Some(colors.ribbon_btn_active_bg),
            colors.ribbon_btn_active_text,
            colors.ribbon_btn_active_border,
        )
    } else if hovered(status) {
        (
            Some(colors.ribbon_btn_hover),
            colors.ribbon_text_hover,
            colors.ribbon_btn_hover_border,
        )
    } else {
        (None, colors.text, Color::TRANSPARENT)
    };
    let style = button::Style {
        background: background.map(Background::Color),
        text_color,
        border: Border {
            color: border,
            width: 1.0,
            radius: 2.0.into(),
        },
        shadow: Shadow::default(),
    };
    if status == button::Status::Disabled {
        faded(style, RIBBON_DISABLED)
    } else {
        style
    }
}

/// A small action in a panel or on a row: a ribbon button that is not on.
pub fn tool(theme: &Theme, status: button::Status) -> button::Style {
    ribbon_button(theme, false, status)
}

/// A text link, such as the source of a map: the accent colour, Signal
/// Orange under the pointer.
pub fn link(theme: &Theme, status: button::Status) -> button::Style {
    let colors = colors(theme);
    let style = button::Style {
        background: None,
        text_color: if hovered(status) {
            colors.accent_hover
        } else {
            colors.accent
        },
        border: Border::default(),
        shadow: Shadow::default(),
    };
    if status == button::Status::Disabled {
        faded(style, DISABLED)
    } else {
        style
    }
}

/// An item of the status bar that can be clicked, such as Cancel import.
pub fn status_button(theme: &Theme, status: button::Status) -> button::Style {
    let colors = colors(theme);
    button::Style {
        background: hovered(status).then_some(Background::Color(colors.status_hover)),
        text_color: if status == button::Status::Disabled {
            colors.status_text_label
        } else {
            colors.status_text
        },
        border: Border::default(),
        shadow: Shadow::default(),
    }
}

/// A text field: the input colours of the dialogs, square, with the focus
/// colour as its border while it has the focus, and selected text on the
/// soft accent.
pub fn input(theme: &Theme, status: field::Status) -> field::Style {
    let colors = colors(theme);
    let style = field::Style {
        background: Background::Color(colors.dialog_input_bg),
        border: Border {
            color: if status == field::Status::Focused {
                colors.focus
            } else {
                colors.dialog_input_border
            },
            width: 1.0,
            radius: 0.0.into(),
        },
        icon: colors.text_muted,
        placeholder: colors.text_faint,
        value: colors.dialog_input_text,
        selection: colors.accent_soft,
    };
    if status == field::Status::Disabled {
        field::Style {
            background: style.background.scale_alpha(DISABLED),
            border: Border {
                color: style.border.color.scale_alpha(DISABLED),
                ..style.border
            },
            value: style.value.scale_alpha(DISABLED),
            ..style
        }
    } else {
        style
    }
}

/// A choice list, as the ThemedSelect of the template.
pub fn select(theme: &Theme, status: choice::Status) -> choice::Style {
    let colors = colors(theme);
    choice::Style {
        text_color: colors.dialog_input_text,
        placeholder_color: colors.text_faint,
        handle_color: colors.dialog_input_text.scale_alpha(0.6),
        background: Background::Color(colors.dialog_input_bg),
        border: Border {
            color: match status {
                choice::Status::Active => colors.dialog_input_border,
                choice::Status::Hovered => colors.dialog_content_secondary,
                choice::Status::Opened => colors.focus,
            },
            width: 1.0,
            radius: 0.0.into(),
        },
    }
}

/// The open menu of a choice list: the shell under it, the option under the
/// pointer tinted, with its text in the accent colour.
pub fn menu(theme: &Theme) -> iced::overlay::menu::Style {
    let colors = colors(theme);
    iced::overlay::menu::Style {
        background: Background::Color(colors.bg),
        border: Border {
            color: colors.dialog_input_border,
            width: 1.0,
            radius: 0.0.into(),
        },
        text_color: colors.dialog_input_text,
        selected_text_color: colors.accent,
        selected_background: Background::Color(colors.ribbon_btn_hover),
    }
}

/// A checkbox: filled with the accent colour when it is ticked, as the
/// native checkbox of the template takes the focus colour.
pub fn check(theme: &Theme, status: check_box::Status) -> check_box::Style {
    let colors = colors(theme);
    let (checked, over, disabled) = match status {
        check_box::Status::Active { is_checked } => (is_checked, false, false),
        check_box::Status::Hovered { is_checked } => (is_checked, true, false),
        check_box::Status::Disabled { is_checked } => (is_checked, false, true),
    };
    let (background, border) = if checked {
        (colors.accent, colors.accent)
    } else if over {
        (colors.dialog_input_bg, colors.dialog_content_secondary)
    } else {
        (colors.dialog_input_bg, colors.dialog_input_border)
    };
    let share = if disabled { DISABLED } else { 1.0 };
    check_box::Style {
        background: Background::Color(background.scale_alpha(share)),
        icon_color: colors.btn_primary_text,
        border: Border {
            color: border.scale_alpha(share),
            width: 1.0,
            radius: 2.0.into(),
        },
        text_color: Some(colors.text.scale_alpha(share)),
    }
}

/// A slider, which the style book does not draw: the rail of its progress
/// bar, filled in the accent colour up to the value, and the knob of its
/// switch as a ring in the accent colour.
pub fn slider_style(theme: &Theme, status: slide::Status) -> slide::Style {
    let colors = colors(theme);
    slide::Style {
        rail: slide::Rail {
            backgrounds: (
                Background::Color(colors.accent),
                Background::Color(colors.border_strong),
            ),
            width: 4.0,
            border: Border::default().rounded(2),
        },
        handle: slide::Handle {
            shape: slide::HandleShape::Circle { radius: 7.0 },
            background: Background::Color(if status == slide::Status::Dragged {
                colors.accent
            } else {
                colors.bg_lighter
            }),
            border_width: 2.0,
            border_color: if status == slide::Status::Hovered {
                colors.accent_hover
            } else {
                colors.accent
            },
        },
    }
}

/// A progress bar of 8 pixels: the accent colour on the strong border.
pub fn progress_style(theme: &Theme) -> progress::Style {
    let colors = colors(theme);
    progress::Style {
        background: Background::Color(colors.border_strong),
        bar: Background::Color(colors.accent),
        border: Border::default().rounded(4),
    }
}

/// The line of 2 pixels under a row that says how far it is.
pub fn progress_line_style(theme: &Theme) -> progress::Style {
    progress::Style {
        border: Border::default().rounded(1),
        ..progress_style(theme)
    }
}

/// A scroll bar: no rail, a thumb in the accent colour at a quarter, at
/// 40 % under the pointer and while it is dragged.
pub fn scroll(theme: &Theme, status: scrolling::Status) -> scrolling::Style {
    let colors = colors(theme);
    let (horizontal, vertical) = match status {
        scrolling::Status::Active => (false, false),
        scrolling::Status::Hovered {
            is_horizontal_scrollbar_hovered,
            is_vertical_scrollbar_hovered,
        } => (
            is_horizontal_scrollbar_hovered,
            is_vertical_scrollbar_hovered,
        ),
        scrolling::Status::Dragged {
            is_horizontal_scrollbar_dragged,
            is_vertical_scrollbar_dragged,
        } => (
            is_horizontal_scrollbar_dragged,
            is_vertical_scrollbar_dragged,
        ),
    };
    let rail = |strong: bool| scrolling::Rail {
        background: None,
        border: Border::default(),
        scroller: scrolling::Scroller {
            color: colors.accent.scale_alpha(if strong { 0.40 } else { 0.25 }),
            border: Border::default().rounded(3),
        },
    };
    scrolling::Style {
        container: container::Style::default(),
        vertical_rail: rail(vertical),
        horizontal_rail: rail(horizontal),
        gap: None,
    }
}

/// The box of a tooltip: the tooltip of the brand, Deep Forge with
/// Blueprint White, in every theme.
pub fn tip_style(theme: &Theme) -> container::Style {
    let colors = colors(theme);
    container::Style::default()
        .background(colors.tooltip_bg)
        .color(colors.tooltip_text)
        .border(Border::default().rounded(4))
}

/// The size of a button of a dialog or a form in the style book: 20 pixels
/// beside the label and 5 above and below, at least 75 wide and 27 high,
/// the label in 11.
const BUTTON_PADDING: [u16; 2] = [5, 20];
const BUTTON_MIN_WIDTH: f32 = 75.0;
const BUTTON_TEXT: f32 = 11.0;
/// The line of the label: 27 high with the padding.
const BUTTON_LINE: f32 = 17.0;

/// A button of a dialog or a form with `label` in the middle, in `style`.
fn dialog_button<'a>(
    label: impl text::IntoFragment<'a>,
    style: fn(&Theme, button::Status) -> button::Style,
) -> button::Button<'a, Message> {
    button(
        iced::widget::column![
            text(label)
                .size(BUTTON_TEXT)
                .font(crate::fonts::REGULAR)
                .line_height(text::LineHeight::Absolute(BUTTON_LINE.into())),
            iced::widget::Space::with_width(BUTTON_MIN_WIDTH - 2.0 * f32::from(BUTTON_PADDING[1])),
        ]
        .align_x(iced::Alignment::Center),
    )
    .padding(BUTTON_PADDING)
    .style(style)
}

/// The button that goes on, in the size of the style book: Save, Create,
/// Next, Add.
pub fn primary_button<'a>(label: impl text::IntoFragment<'a>) -> button::Button<'a, Message> {
    dialog_button(label, primary)
}

/// Every other button of a dialog or a form, in the size of the style
/// book: Cancel, Back, Reset.
pub fn secondary_button<'a>(label: impl text::IntoFragment<'a>) -> button::Button<'a, Message> {
    dialog_button(label, secondary)
}

/// A text field in the style of the style book.
pub fn text_input<'a>(placeholder: &str, value: &str) -> field::TextInput<'a, Message> {
    field::TextInput::new(placeholder, value)
        .style(input)
        .size(11)
        .padding([3, 6])
}

/// A choice list in the style of the style book, with its menu.
pub fn pick_list<'a, T, L, V>(
    options: L,
    selected: Option<V>,
    on_selected: impl Fn(T) -> Message + 'a,
) -> choice::PickList<'a, T, L, V, Message>
where
    T: ToString + PartialEq + Clone + 'a,
    L: Borrow<[T]> + 'a,
    V: Borrow<T> + 'a,
{
    choice::PickList::new(options, selected, on_selected)
        .style(select)
        .menu_style(menu)
        .text_size(11)
        .padding([3, 6])
        .handle(choice::Handle::Arrow {
            size: Some(iced::Pixels(10.0)),
        })
}

/// A checkbox in the style of the style book: 13 pixels, its label in 11.
pub fn checkbox<'a>(label: impl Into<String>, checked: bool) -> check_box::Checkbox<'a, Message> {
    check_box::Checkbox::new(label, checked)
        .style(check)
        .size(13)
        .text_size(11)
        .spacing(6)
}

/// A slider in the style of the window.
pub fn slider<'a, T>(
    range: RangeInclusive<T>,
    value: T,
    on_change: impl Fn(T) -> Message + 'a,
) -> slide::Slider<'a, T, Message>
where
    T: Copy + From<u8> + PartialOrd,
{
    slide::Slider::new(range, value, on_change).style(slider_style)
}

/// A progress bar of 8 pixels.
pub fn progress_bar<'a>(range: RangeInclusive<f32>, value: f32) -> progress::ProgressBar<'a> {
    progress::ProgressBar::new(range, value)
        .height(8)
        .style(progress_style)
}

/// A progress line of 2 pixels under a row.
pub fn progress_line<'a>(range: RangeInclusive<f32>, value: f32) -> progress::ProgressBar<'a> {
    progress::ProgressBar::new(range, value)
        .height(2)
        .style(progress_line_style)
}

/// The scroll bar of the window: 6 pixels.
pub fn scrollbar() -> scrolling::Scrollbar {
    scrolling::Scrollbar::new().width(6).scroller_width(6)
}

/// Content that scrolls up and down, with the scroll bar of the window.
pub fn scrollable<'a>(
    content: impl Into<Element<'a, Message>>,
) -> scrolling::Scrollable<'a, Message> {
    scrolling::Scrollable::with_direction(content, scrolling::Direction::Vertical(scrollbar()))
        .style(scroll)
}

/// The box of a tooltip with its text.
pub fn tip<'a>(content: impl text::IntoFragment<'a>) -> Element<'a, Message> {
    container(text(content).size(12))
        .padding([8, 12])
        .max_width(340)
        .style(tip_style)
        .into()
}

/// The space between a tooltip and what it explains.
const TIP_GAP: f32 = 8.0;

/// `content` with `tip_text` in a tooltip above it, where the style book
/// opens a tooltip in the panels, the tabs, the dialogs and on the scene.
pub fn tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    tip_text: impl text::IntoFragment<'a>,
) -> tip_widget::Tooltip<'a, Message> {
    tip_widget::Tooltip::new(content, tip(tip_text), tip_widget::Position::Top).gap(TIP_GAP)
}

/// `content` of the ribbon or the top strip with `tip_text` in a tooltip
/// below it, as the style book opens the tooltips of its ribbon and title
/// bar.
pub fn ribbon_tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    tip_text: impl text::IntoFragment<'a>,
) -> tip_widget::Tooltip<'a, Message> {
    tip_widget::Tooltip::new(content, tip(tip_text), tip_widget::Position::Bottom).gap(TIP_GAP)
}

/// A wide row with `tip_text` in a tooltip above the pointer, for a tip
/// that belongs to the place pointed at rather than to the middle of the
/// row, such as the path of a scan.
pub fn pointer_tooltip<'a>(
    content: impl Into<Element<'a, Message>>,
    tip_text: impl text::IntoFragment<'a>,
) -> tip_widget::Tooltip<'a, Message> {
    tip_widget::Tooltip::new(content, tip(tip_text), tip_widget::Position::FollowCursor)
}

/// `content` with `tint` of the theme laid over it while the pointer is
/// over it, as an item of the status bar of the style book.
pub fn hover_tint<'a>(
    content: impl Into<Element<'a, Message>>,
    tint: fn(&UiColors) -> Color,
) -> Element<'a, Message> {
    Element::new(HoverTint {
        content: content.into(),
        tint,
    })
}

/// A widget that is its content, tinted under the pointer.
struct HoverTint<'a> {
    content: Element<'a, Message>,
    tint: fn(&UiColors) -> Color,
}

impl Widget<Message, Theme, Renderer> for HoverTint<'_> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget().layout(tree, renderer, limits)
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
        if cursor.is_over(layout.bounds()) {
            use iced::advanced::Renderer as _;
            renderer.fill_quad(
                renderer::Quad {
                    bounds: layout.bounds(),
                    ..renderer::Quad::default()
                },
                (self.tint)(&colors(theme)),
            );
        }
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget()
            .operate(tree, layout, renderer, operation);
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
        self.content.as_widget_mut().on_event(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, translation)
    }
}

/// The arrow over everything that can be clicked: the shell of the style
/// book shows no hand on its buttons, tabs and items.
pub fn no_hand<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(NoHand {
        content: content.into(),
    })
}

/// `interaction` with the hand of a link turned into the arrow.
fn arrow(interaction: mouse::Interaction) -> mouse::Interaction {
    match interaction {
        mouse::Interaction::Pointer => mouse::Interaction::Idle,
        other => other,
    }
}

/// A widget that is its content, but never shows the hand.
pub struct NoHand<'a> {
    content: Element<'a, Message>,
}

impl Widget<Message, Theme, Renderer> for NoHand<'_> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content.as_widget().layout(tree, renderer, limits)
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget()
            .operate(tree, layout, renderer, operation);
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
        self.content.as_widget_mut().on_event(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        arrow(
            self.content
                .as_widget()
                .mouse_interaction(tree, layout, cursor, viewport, renderer),
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, translation)
            .map(|inner| overlay::Element::new(Box::new(NoHandOverlay { inner })))
    }
}

/// An overlay, such as the menu of a choice list, that never shows the hand.
struct NoHandOverlay<'b> {
    inner: overlay::Element<'b, Message, Theme, Renderer>,
}

impl overlay::Overlay<Message, Theme, Renderer> for NoHandOverlay<'_> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        self.inner.layout(renderer, bounds)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.inner.draw(renderer, theme, style, layout, cursor);
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.inner.operate(layout, renderer, operation);
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
        self.inner
            .on_event(event, layout, cursor, renderer, clipboard, shell)
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        arrow(
            self.inner
                .mouse_interaction(layout, cursor, viewport, renderer),
        )
    }

    fn is_over(&self, layout: Layout<'_>, renderer: &Renderer, cursor_position: Point) -> bool {
        self.inner.is_over(layout, renderer, cursor_position)
    }

    fn overlay<'c>(
        &'c mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'c, Message, Theme, Renderer>> {
        self.inner
            .overlay(layout, renderer)
            .map(|inner| overlay::Element::new(Box::new(NoHandOverlay { inner })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_theme::UiTheme;

    /// Each theme as iced has it, with its tokens.
    fn themes() -> impl Iterator<Item = (Theme, UiColors)> {
        UiTheme::ALL
            .into_iter()
            .map(|theme| (theme.iced(), theme.colors()))
    }

    fn background(style: &button::Style) -> Option<Color> {
        style.background.map(|background| match background {
            Background::Color(color) => color,
            Background::Gradient(_) => unreachable!("a button of the shell is flat"),
        })
    }

    #[test]
    fn the_primary_button_is_the_accent_and_orange_under_the_pointer() {
        for (theme, colors) in themes() {
            let ready = primary(&theme, button::Status::Active);
            assert_eq!(background(&ready), Some(colors.btn_primary_bg));
            assert_eq!(ready.text_color, colors.btn_primary_text);
            assert_eq!(ready.border.color, colors.btn_primary_border);
            assert_eq!(ready.border.radius, 2.0.into());
            for status in [button::Status::Hovered, button::Status::Pressed] {
                let over = primary(&theme, status);
                assert_eq!(background(&over), Some(colors.btn_primary_hover_bg));
                assert_eq!(over.text_color, colors.btn_primary_hover_text);
            }
            let waiting = primary(&theme, button::Status::Disabled);
            assert_eq!(
                background(&waiting),
                Some(colors.btn_primary_bg.scale_alpha(0.5))
            );
            assert_eq!(waiting.text_color, colors.btn_primary_text.scale_alpha(0.5));
        }
    }

    #[test]
    fn the_secondary_button_is_outlined_as_an_input() {
        for (theme, colors) in themes() {
            let ready = secondary(&theme, button::Status::Active);
            assert_eq!(background(&ready), Some(colors.btn_secondary_bg));
            assert_eq!(ready.text_color, colors.btn_secondary_text);
            assert_eq!(ready.border.color, colors.dialog_input_border);
            let over = secondary(&theme, button::Status::Hovered);
            assert_eq!(background(&over), Some(colors.btn_secondary_hover_bg));
            assert_eq!(over.border.color, colors.btn_secondary_hover_border);
            let waiting = secondary(&theme, button::Status::Disabled);
            assert_eq!(
                waiting.text_color,
                colors.btn_secondary_text.scale_alpha(0.5)
            );
        }
    }

    /// The size `element` takes when it may take up to 400 by 100.
    fn laid_out(element: Element<'_, Message>) -> Size {
        let renderer = Renderer::Secondary(iced_tiny_skia::Renderer::new(
            crate::fonts::REGULAR,
            iced::Pixels(12.0),
        ));
        let mut tree = Tree::new(&element);
        element
            .as_widget()
            .layout(
                &mut tree,
                &renderer,
                &layout::Limits::new(Size::ZERO, Size::new(400.0, 100.0)),
            )
            .size()
    }

    #[test]
    fn a_button_of_a_dialog_is_27_high_and_at_least_75_wide() {
        let makers: [fn(&'static str) -> button::Button<'static, Message>; 2] =
            [primary_button, secondary_button];
        for make in makers {
            // A short label keeps the width of the style book.
            assert_eq!(
                laid_out(make("OK").on_press(Message::ToggleFile).into()),
                Size::new(75.0, 27.0)
            );
            // A long one widens the button by itself, 20 on each side.
            let wide = laid_out(make("Reset to Defaults").into());
            assert_eq!(wide.height, 27.0);
            assert!(wide.width > 100.0, "{wide:?}");
        }
        // The label is centred: a short label lies as far from either side.
        let picture = crate::test_render::render(
            container(primary_button("OK").on_press(Message::ToggleFile))
                .style(|_| container::Style::default().background(Color::WHITE))
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            &UiTheme::Light.iced(),
            Size::new(75.0, 27.0),
        );
        let text = UiTheme::Light.colors().btn_primary_text;
        let inked: Vec<u32> = (0..75)
            .filter(|&x| {
                (6..21).any(|y| {
                    let [r, g, b] = picture.rgb(x, y);
                    // The text colour of the button, not its fill.
                    (f32::from(r) / 255.0 - text.r).abs() < 0.2
                        && (f32::from(g) / 255.0 - text.g).abs() < 0.2
                        && (f32::from(b) / 255.0 - text.b).abs() < 0.2
                })
            })
            .collect();
        let (first, last) = (inked[0], inked[inked.len() - 1]);
        assert!(first.abs_diff(74 - last) <= 2, "{first}..{last}");
        // And in the middle between its top and bottom.
        let rows: Vec<u32> = (0..27)
            .filter(|&y| {
                (first..=last).any(|x| {
                    let [r, g, b] = picture.rgb(x, y);
                    (f32::from(r) / 255.0 - text.r).abs() < 0.2
                        && (f32::from(g) / 255.0 - text.g).abs() < 0.2
                        && (f32::from(b) / 255.0 - text.b).abs() < 0.2
                })
            })
            .collect();
        let (top, bottom) = (rows[0], rows[rows.len() - 1]);
        assert!(top.abs_diff(26 - bottom) <= 3, "{top}..{bottom}");
    }

    #[test]
    fn a_ribbon_button_is_tinted_under_the_pointer_and_while_it_is_on() {
        for (theme, colors) in themes() {
            let idle = ribbon_button(&theme, false, button::Status::Active);
            assert_eq!(background(&idle), None);
            assert_eq!(idle.border.color, Color::TRANSPARENT);
            assert_eq!(idle.text_color, colors.text);
            let over = ribbon_button(&theme, false, button::Status::Hovered);
            assert_eq!(background(&over), Some(colors.ribbon_btn_hover));
            assert_eq!(over.border.color, colors.ribbon_btn_hover_border);
            assert_eq!(over.text_color, colors.ribbon_text_hover);
            let on = ribbon_button(&theme, true, button::Status::Active);
            assert_eq!(background(&on), Some(colors.ribbon_btn_active_bg));
            assert_eq!(on.border.color, colors.ribbon_btn_active_border);
            assert_eq!(on.text_color, colors.ribbon_btn_active_text);
            let waiting = ribbon_button(&theme, false, button::Status::Disabled);
            assert_eq!(waiting.text_color, colors.text.scale_alpha(0.4));
            // A small action of a panel is a ribbon button that is not on.
            for status in [button::Status::Active, button::Status::Hovered] {
                assert_eq!(tool(&theme, status), ribbon_button(&theme, false, status));
            }
        }
    }

    #[test]
    fn a_link_and_an_item_of_the_status_bar_have_no_frame() {
        for (theme, colors) in themes() {
            let idle = link(&theme, button::Status::Active);
            assert_eq!(background(&idle), None);
            assert_eq!(idle.text_color, colors.accent);
            assert_eq!(
                link(&theme, button::Status::Hovered).text_color,
                colors.accent_hover
            );
            let item = status_button(&theme, button::Status::Active);
            assert_eq!(background(&item), None);
            assert_eq!(item.text_color, colors.status_text);
            let over = status_button(&theme, button::Status::Hovered);
            assert_eq!(background(&over), Some(colors.status_hover));
        }
    }

    #[test]
    fn a_text_field_shows_the_focus_in_its_border_only() {
        for (theme, colors) in themes() {
            let idle = input(&theme, field::Status::Active);
            assert_eq!(idle.background, Background::Color(colors.dialog_input_bg));
            assert_eq!(idle.border.color, colors.dialog_input_border);
            assert_eq!(idle.border.radius, 0.0.into());
            assert_eq!(idle.value, colors.dialog_input_text);
            assert_eq!(idle.placeholder, colors.text_faint);
            // Selected text lies on the soft accent of the theme.
            assert_eq!(idle.selection, colors.accent_soft);
            let focused = input(&theme, field::Status::Focused);
            assert_eq!(focused.border.color, colors.focus);
            assert_eq!(focused.background, idle.background);
            let waiting = input(&theme, field::Status::Disabled);
            assert_eq!(waiting.value, colors.dialog_input_text.scale_alpha(0.5));
        }
    }

    #[test]
    fn a_choice_list_and_its_menu_are_square_inputs() {
        for (theme, colors) in themes() {
            let idle = select(&theme, choice::Status::Active);
            assert_eq!(idle.background, Background::Color(colors.dialog_input_bg));
            assert_eq!(idle.border.color, colors.dialog_input_border);
            assert_eq!(idle.border.radius, 0.0.into());
            assert_eq!(idle.text_color, colors.dialog_input_text);
            assert_eq!(
                select(&theme, choice::Status::Hovered).border.color,
                colors.dialog_content_secondary
            );
            assert_eq!(
                select(&theme, choice::Status::Opened).border.color,
                colors.focus
            );
            let open = menu(&theme);
            // The menu covers what lies under it.
            assert_eq!(open.background, Background::Color(colors.bg));
            assert_eq!(colors.bg.a, 1.0);
            assert_eq!(
                open.selected_background,
                Background::Color(colors.ribbon_btn_hover)
            );
            assert_eq!(open.selected_text_color, colors.accent);
        }
    }

    #[test]
    fn a_ticked_checkbox_is_the_accent_with_the_text_colour_of_a_primary_button() {
        for (theme, colors) in themes() {
            let ticked = check(&theme, check_box::Status::Active { is_checked: true });
            assert_eq!(ticked.background, Background::Color(colors.accent));
            assert_eq!(ticked.icon_color, colors.btn_primary_text);
            let empty = check(&theme, check_box::Status::Active { is_checked: false });
            assert_eq!(empty.background, Background::Color(colors.dialog_input_bg));
            assert_eq!(empty.border.color, colors.dialog_input_border);
            assert_eq!(empty.border.radius, 2.0.into());
            let waiting = check(&theme, check_box::Status::Disabled { is_checked: true });
            assert_eq!(
                waiting.background,
                Background::Color(colors.accent.scale_alpha(0.5))
            );
        }
    }

    #[test]
    fn a_slider_fills_its_rail_and_rings_its_knob_in_the_accent_colour() {
        for (theme, colors) in themes() {
            let idle = slider_style(&theme, slide::Status::Active);
            assert_eq!(
                idle.rail.backgrounds,
                (
                    Background::Color(colors.accent),
                    Background::Color(colors.border_strong)
                )
            );
            assert_eq!(idle.rail.width, 4.0);
            assert!(matches!(
                idle.handle.shape,
                slide::HandleShape::Circle { radius } if radius == 7.0
            ));
            assert_eq!(idle.handle.background, Background::Color(colors.bg_lighter));
            assert_eq!(idle.handle.border_color, colors.accent);
            assert_eq!(idle.handle.border_width, 2.0);
            let over = slider_style(&theme, slide::Status::Hovered);
            assert_eq!(over.handle.border_color, colors.accent_hover);
            let dragged = slider_style(&theme, slide::Status::Dragged);
            assert_eq!(dragged.handle.background, Background::Color(colors.accent));
        }
    }

    #[test]
    fn progress_is_the_accent_on_the_strong_border() {
        for (theme, colors) in themes() {
            let bar = progress_style(&theme);
            assert_eq!(bar.background, Background::Color(colors.border_strong));
            assert_eq!(bar.bar, Background::Color(colors.accent));
            assert_eq!(bar.border.radius, 4.0.into());
            assert_eq!(progress_line_style(&theme).border.radius, 1.0.into());
        }
    }

    #[test]
    fn a_scroll_bar_has_a_thumb_of_the_accent_and_no_rail() {
        for (theme, colors) in themes() {
            let idle = scroll(&theme, scrolling::Status::Active);
            assert_eq!(idle.vertical_rail.background, None);
            assert_eq!(
                idle.vertical_rail.scroller.color,
                colors.accent.scale_alpha(0.25)
            );
            let over = scroll(
                &theme,
                scrolling::Status::Hovered {
                    is_horizontal_scrollbar_hovered: false,
                    is_vertical_scrollbar_hovered: true,
                },
            );
            assert_eq!(
                over.vertical_rail.scroller.color,
                colors.accent.scale_alpha(0.40)
            );
            assert_eq!(
                over.horizontal_rail.scroller.color,
                colors.accent.scale_alpha(0.25)
            );
        }
    }

    #[test]
    fn a_tooltip_is_the_tooltip_of_the_brand_in_every_theme() {
        for (theme, _) in themes() {
            let style = tip_style(&theme);
            assert_eq!(
                style.background,
                Some(Background::Color(Color::from_rgb8(0x36, 0x36, 0x3E)))
            );
            assert_eq!(style.text_color, Some(Color::from_rgb8(0xFA, 0xFA, 0xF9)));
            assert_eq!(style.border.radius, 4.0.into());
        }
    }

    /// What a tooltip made by `make` explains, a box of 40 by 20 in the
    /// middle of a window of 400 by 300, and where the tooltip opens while
    /// the pointer is on that box.
    fn tip_around(
        make: fn(Element<'static, Message>, &'static str) -> tip_widget::Tooltip<'static, Message>,
    ) -> (Rectangle, Rectangle) {
        let renderer = Renderer::Secondary(iced_tiny_skia::Renderer::new(
            crate::fonts::REGULAR,
            iced::Pixels(12.0),
        ));
        let size = Size::new(400.0, 300.0);
        let target: Element<'static, Message> = container(text("")).width(40).height(20).into();
        let mut element: Element<'static, Message> = container(make(target, "What it does"))
            .center(Length::Fill)
            .into();
        let mut tree = Tree::new(&element);
        let node = element.as_widget().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let target = node.children()[0].bounds();
        let pointer = target.center();
        let mut messages = Vec::new();
        let mut shell = Shell::new(&mut messages);
        let _ = element.as_widget_mut().on_event(
            &mut tree,
            Event::Mouse(mouse::Event::CursorMoved { position: pointer }),
            Layout::new(&node),
            mouse::Cursor::Available(pointer),
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut shell,
            &Rectangle::with_size(size),
        );
        let mut overlay = element
            .as_widget_mut()
            .overlay(&mut tree, Layout::new(&node), &renderer, Vector::ZERO)
            .expect("the tooltip while the pointer is on the box");
        let tip = overlay.layout(&renderer, size).children()[0].bounds();
        (target, tip)
    }

    #[test]
    fn a_tooltip_opens_above_except_in_the_ribbon() {
        // In the panels, the tabs, the dialogs and on the scene: above.
        let (target, tip) = tip_around(tooltip);
        assert_eq!(target.size(), Size::new(40.0, 20.0));
        assert!(
            (target.y - (tip.y + tip.height) - TIP_GAP).abs() < 0.5,
            "{tip:?} above {target:?}"
        );
        assert!((tip.center_x() - target.center_x()).abs() < 0.5);
        // In the ribbon and the top strip: below.
        let (target, tip) = tip_around(ribbon_tooltip);
        assert!(
            (tip.y - (target.y + target.height) - TIP_GAP).abs() < 0.5,
            "{tip:?} below {target:?}"
        );
        // Beside the pointer, and above it.
        let (target, tip) = tip_around(pointer_tooltip);
        let pointer = target.center();
        assert!(tip.center_x() > pointer.x, "{tip:?}");
        assert!(tip.center_y() < pointer.y, "{tip:?}");
    }

    #[test]
    fn an_item_is_tinted_while_the_pointer_is_over_it() {
        for (theme, colors) in themes() {
            // An item on the bar, as the status bar lays its items.
            let item = || -> Element<'_, Message> {
                container(hover_tint(
                    container(text("")).width(Length::Fill).height(Length::Fill),
                    |colors| colors.status_hover,
                ))
                .style(|theme| container::Style::default().background(colors_of(theme)))
                .into()
            };
            let size = Size::new(40.0, 22.0);
            let away = crate::test_render::render(item(), &theme, size);
            assert!(away.is(20, 11, colors.status_bg));
            let over = crate::test_render::render_under(
                item(),
                &theme,
                size,
                mouse::Cursor::Available(Point::new(20.0, 11.0)),
            );
            // The tint of the theme over the bar.
            let [r, g, b] = over.rgb(20, 11);
            let tinted = Color::from_rgb8(r, g, b);
            assert_ne!(tinted, colors.status_bg, "{colors:?}");
        }
    }

    fn colors_of(theme: &Theme) -> Color {
        colors(theme).status_bg
    }

    /// What the pointer looks like over the upper left of `element`.
    fn pointer_over(element: Element<'_, Message>) -> mouse::Interaction {
        let renderer = Renderer::Secondary(iced_tiny_skia::Renderer::new(
            crate::fonts::REGULAR,
            iced::Pixels(12.0),
        ));
        let size = Size::new(200.0, 40.0);
        let mut tree = Tree::new(&element);
        let node = element.as_widget().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        element.as_widget().mouse_interaction(
            &tree,
            Layout::new(&node),
            mouse::Cursor::Available(Point::new(10.0, 10.0)),
            &Rectangle::with_size(size),
            &renderer,
        )
    }

    #[test]
    fn no_hand_keeps_the_arrow_where_a_button_shows_the_hand() {
        let button = || {
            button(text("File"))
                .on_press(Message::ToggleFile)
                .width(200)
                .height(40)
        };
        assert_eq!(pointer_over(button().into()), mouse::Interaction::Pointer);
        assert_eq!(pointer_over(no_hand(button())), mouse::Interaction::Idle);
        // Other shapes of the pointer stay.
        let field: Element<'_, Message> = text_input("", "")
            .on_input(Message::SectionRotationInput)
            .width(200)
            .into();
        assert_eq!(pointer_over(no_hand(field)), mouse::Interaction::Text);
    }

    /// Where `name(` is called in `source` as a function of its own: not as
    /// a method, not through a path, not in a comment.
    fn calls(source: &str, name: &str) -> Vec<usize> {
        let pattern = format!("{name}(");
        source
            .match_indices(&pattern)
            .map(|(at, _)| at)
            .filter(|&at| {
                let before = source[..at].chars().next_back();
                let line = &source[source[..at].rfind('\n').map_or(0, |start| start + 1)..at];
                !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '.')
                    && !line.trim_start().starts_with("//")
                    && !line.contains("fn ")
            })
            .collect()
    }

    /// The methods called on the call whose bracket opens at `open`.
    fn chain(source: &str, open: usize) -> Vec<&str> {
        let bytes = source.as_bytes();
        let closing = |mut at: usize| {
            let mut depth = 0;
            loop {
                match bytes[at] {
                    b'"' => {
                        let (_, after) =
                            crate::i18n::source_scan::literal_at(source, at).expect("a string");
                        at = after;
                        continue;
                    }
                    b'(' | b'[' | b'{' => depth += 1,
                    b')' | b']' | b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            return at + 1;
                        }
                    }
                    _ => {}
                }
                at += 1;
            }
        };
        let mut at = closing(open);
        let mut methods = Vec::new();
        loop {
            let rest = source[at..].trim_start();
            let Some(method) = rest.strip_prefix('.') else {
                return methods;
            };
            let name_len = method
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(method.len());
            if !method[name_len..].starts_with('(') {
                return methods;
            }
            methods.push(&method[..name_len]);
            at = closing(source.len() - method.len() + name_len);
        }
    }

    #[test]
    fn the_window_makes_its_controls_with_the_shared_styles() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sources = Vec::new();
        let mut folders = vec![directory.clone()];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    folders.push(path);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    sources.push(path);
                }
            }
        }
        assert!(sources.len() > 50, "{}", sources.len());
        let mut found = Vec::new();
        for path in sources {
            if path.ends_with("ui_style.rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let name = path.strip_prefix(&directory).unwrap().display().to_string();
            // Widgets whose style is set here, made by hand.
            for widget in [
                "text_input",
                "pick_list",
                "checkbox",
                "slider",
                "progress_bar",
                "scrollable",
                "tooltip",
            ] {
                for at in calls(&source, widget) {
                    found.push(format!("{name}: {widget} at {at}"));
                }
            }
            // The styles that came before these.
            for old in [
                "flat_tool_style",
                "themed_pick_list_style",
                "muted_checkbox_style",
                "primary_btn_style",
                "plain_btn_style",
                "tool_btn_style",
                "tip_box",
            ] {
                if source.contains(old) {
                    found.push(format!("{name}: {old}"));
                }
            }
            // Every button chooses its style.
            for at in calls(&source, "button") {
                if !chain(&source, at + "button".len()).contains(&"style") {
                    found.push(format!("{name}: a button without a style at {at}"));
                }
            }
            // A tooltip opens where the style book has it for its part of
            // the window: above, or below in the ribbon.
            for place in ["Position::", "Tooltip::new("] {
                if source.contains(place) {
                    found.push(format!("{name}: a tooltip placed by hand ({place})"));
                }
            }
            // A primary or a secondary button is made with its size, by its
            // constructor; its style alone is only compared with in tests.
            for style in ["ui_style::primary", "ui_style::secondary"] {
                for (at, _) in source.match_indices(style) {
                    let after = &source[at + style.len()..];
                    let other_name = after.starts_with(|c: char| c.is_alphanumeric() || c == '_');
                    if !other_name && !after.starts_with("(&") {
                        found.push(format!("{name}: {style} on a button made by hand at {at}"));
                    }
                }
            }
        }
        assert!(found.is_empty(), "{found:#?}");
    }
}
