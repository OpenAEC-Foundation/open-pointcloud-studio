//! The title bar of the OpenAEC style book, in place of the one of the
//! system: 32 high on the lighter shell, with the logo and the quick-access
//! buttons at its left, the name and the version of the application in its
//! middle, and Send Feedback and the buttons of the window at its right.
//!
//! On Windows the window has no frame of the system: the bar is dragged to
//! move the window, a double click maximises it or gives it back its size,
//! and the edges of the window, 6 pixels wide, resize it. On macOS the buttons of the system stay at the
//! left of the bar, which runs under them; on Linux, where iced 0.13 cannot
//! have a window without a frame resized by the window manager of X11 or
//! the compositor of Wayland, the frame of the system stays and the bar
//! holds no title and no window buttons. Closing goes the way the system's
//! close button goes, through `Message::Exit`.

use std::time::{Duration, Instant};

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{Tree, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::widget::{button, container, mouse_area, row, stack, text, Space};
use iced::{event, mouse, Element, Event, Fill, Length, Point, Rectangle, Renderer, Size};
use iced::{Task, Theme};

use crate::icons::{self, WindowGlyph};
use crate::{i18n, opencad_ribbon, ui_style, ui_theme, Message, Studio};

/// The height of the bar.
pub const HEIGHT: f32 = 32.0;
/// The smallest window, as the style book has it.
pub const MIN_SIZE: Size = Size::new(800.0, 600.0);
/// Two presses on the bar within this time are a double click.
const DOUBLE_CLICK: Duration = Duration::from_millis(400);
/// The width of the edges that resize the window, and how far a corner
/// reaches along them.
const EDGE: f32 = 6.0;
const CORNER: f32 = 12.0;
/// The room the buttons of macOS take at the left of the bar.
const MACOS_BUTTONS: f32 = 72.0;
/// Where Send Feedback leads: the issues of the repository.
pub const ISSUES_URL: &str = "https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues";

/// Who frames the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    /// The application: Windows.
    Own,
    /// The system, whose buttons stay at the left of the bar: macOS.
    MacOs,
    /// The system, whose frame stays above the bar: Linux, on X11 and on
    /// Wayland.
    System,
}

impl Frame {
    /// The frame of the window on this system.
    pub fn current() -> Self {
        if cfg!(windows) {
            Self::Own
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::System
        }
    }

    /// Whether the system draws a frame around the window.
    pub fn decorations(self) -> bool {
        self != Self::Own
    }

    /// What the local API calls it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Own => "own",
            Self::MacOs => "macos",
            Self::System => "system",
        }
    }
}

/// An edge or a corner of the window that a press resizes it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    North,
    South,
    East,
    West,
    NorthEast,
    NorthWest,
    SouthEast,
    SouthWest,
}

impl Edge {
    /// The edge at `point` of a window of `size`, if the point lies on one.
    pub fn at(point: Point, size: Size) -> Option<Self> {
        if !(0.0..size.width).contains(&point.x) || !(0.0..size.height).contains(&point.y) {
            return None;
        }
        let near = |value: f32, reach: f32, length: f32| {
            if value < reach {
                Some(false)
            } else if value >= length - reach {
                Some(true)
            } else {
                None
            }
        };
        let (on_x, on_y) = (
            near(point.x, EDGE, size.width),
            near(point.y, EDGE, size.height),
        );
        if on_x.is_none() && on_y.is_none() {
            return None;
        }
        // Along an edge, the last pixels before a corner resize both ways.
        let x = on_x.or_else(|| near(point.x, CORNER, size.width));
        let y = on_y.or_else(|| near(point.y, CORNER, size.height));
        Some(match (x, y) {
            (Some(false), Some(false)) => Self::NorthWest,
            (Some(true), Some(false)) => Self::NorthEast,
            (Some(false), Some(true)) => Self::SouthWest,
            (Some(true), Some(true)) => Self::SouthEast,
            (Some(false), None) => Self::West,
            (Some(true), None) => Self::East,
            (None, Some(false)) => Self::North,
            (None, Some(true)) => Self::South,
            (None, None) => unreachable!("the point lies on an edge"),
        })
    }

    /// The pointer over the edge.
    pub fn interaction(self) -> mouse::Interaction {
        match self {
            Self::North | Self::South => mouse::Interaction::ResizingVertically,
            Self::East | Self::West => mouse::Interaction::ResizingHorizontally,
            Self::NorthEast | Self::SouthWest => mouse::Interaction::ResizingDiagonallyUp,
            Self::NorthWest | Self::SouthEast => mouse::Interaction::ResizingDiagonallyDown,
        }
    }
}

/// What the bar and the edges of the window ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// A press on the empty part of the bar: a drag, or the second press of
    /// a double click.
    BarPressed,
    Minimize,
    ToggleMaximize,
    /// The close button: the way the system's close button goes.
    Close,
    /// A press on an edge of the window.
    Resize(Edge),
    /// The system says whether the window is maximised.
    Maximized(bool),
    /// Send Feedback.
    Feedback,
    /// A right click on the empty part of the bar.
    SystemMenu,
}

/// What the window is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Drag,
    ToggleMaximize,
    Minimize,
    Resize(Edge),
    /// The menu of the window of the system, at the pointer.
    SystemMenu,
}

/// The state of the bar: the frame, whether the window is maximised, and
/// the last press on the bar, for a double click.
#[derive(Debug, Clone, Copy)]
pub struct TitleBar {
    pub frame: Frame,
    pub maximized: bool,
    last_press: Option<Instant>,
}

impl Default for TitleBar {
    fn default() -> Self {
        Self::new(Frame::current())
    }
}

impl TitleBar {
    pub fn new(frame: Frame) -> Self {
        Self {
            frame,
            maximized: false,
            last_press: None,
        }
    }

    /// A press on the empty part of the bar at `now`: a second press soon
    /// after the first maximises the window or gives it back its size,
    /// where the application frames the window; any other press drags it.
    pub fn press(&mut self, now: Instant) -> Request {
        let double = self.frame == Frame::Own
            && self
                .last_press
                .is_some_and(|last| now.saturating_duration_since(last) <= DOUBLE_CLICK);
        if double {
            self.last_press = None;
            Request::ToggleMaximize
        } else {
            self.last_press = Some(now);
            Request::Drag
        }
    }

    /// Whether the edges of the window resize it: not while it is
    /// maximised, and only where the application frames it.
    pub fn resizes(&self) -> bool {
        self.frame == Frame::Own && !self.maximized
    }
}

/// The task that does `request` with the window.
fn request_task(request: Request) -> Task<Message> {
    let window = iced::window::get_latest();
    match request {
        Request::Drag => window.and_then(iced::window::drag),
        Request::ToggleMaximize => window.and_then(iced::window::toggle_maximize),
        Request::Minimize => window.and_then(|id| iced::window::minimize(id, true)),
        Request::Resize(edge) => window.and_then(move |id| {
            iced::window::run_with_handle(id, move |handle| {
                crate::native_chrome::begin_resize(handle, edge)
            })
            .discard()
        }),
        Request::SystemMenu => window.and_then(iced::window::show_system_menu),
    }
}

/// Ask the system whether the window is maximised, for the glyph of its
/// button and the edges.
pub fn ask_maximized() -> Task<Message> {
    iced::window::get_latest().and_then(|id| {
        iced::window::get_maximized(id)
            .map(|maximized| Message::Window(WindowAction::Maximized(maximized)))
    })
}

impl Studio {
    /// What the bar and the edges of the window ask for.
    pub(crate) fn window_action(&mut self, action: WindowAction) -> Task<Message> {
        let request = match action {
            WindowAction::BarPressed => self.title_bar.press(Instant::now()),
            WindowAction::Minimize => Request::Minimize,
            WindowAction::ToggleMaximize => Request::ToggleMaximize,
            WindowAction::Close => return Task::done(Message::Exit),
            WindowAction::Resize(edge) => {
                if !self.title_bar.resizes() {
                    return Task::none();
                }
                Request::Resize(edge)
            }
            WindowAction::Maximized(maximized) => {
                self.title_bar.maximized = maximized;
                return Task::none();
            }
            WindowAction::Feedback => return Task::done(Message::OpenUrl(ISSUES_URL)),
            // Where the system frames the window, its frame has the menu.
            WindowAction::SystemMenu => {
                if self.title_bar.frame != Frame::Own {
                    return Task::none();
                }
                Request::SystemMenu
            }
        };
        request_task(request)
    }

    /// The bar at the top of the window. While a dialog covers the window
    /// under it, `covered`, its quick-access buttons wait, as the keys do,
    /// and it still moves, minimises, maximises and closes the window.
    pub(crate) fn title_bar(&self, covered: bool) -> Element<'_, Message> {
        let frame = self.title_bar.frame;
        let logo =
            container(icons::glyph(icons::logo(), 16.0).tint(|colors| colors.accent)).center(20);
        // The quick-access buttons, after a line in the accent colour over
        // the height of the bar.
        let quick = row![
            opencad_ribbon::line(|colors| colors.accent)
                .width(1)
                .height(Fill),
            container(self.quick_access(covered))
                .padding([0, 4])
                .center_y(Fill),
        ]
        .height(Fill);
        // The line 2 after the gap of 4 beside the logo, as the style book
        // sets it.
        let quick = container(quick).padding(iced::Padding {
            left: 2.0,
            ..iced::Padding::ZERO
        });
        let left = row![logo, quick]
            .spacing(4)
            .height(HEIGHT)
            .align_y(iced::Alignment::Center)
            .padding(iced::Padding {
                left: if frame == Frame::MacOs {
                    MACOS_BUTTONS
                } else {
                    8.0
                },
                ..iced::Padding::ZERO
            });
        let feedback = container(
            button(text(i18n::tr("Send Feedback")).size(11))
                .on_press(Message::Window(WindowAction::Feedback))
                .padding([0, 4])
                .style(ui_style::link),
        )
        .padding(iced::Padding {
            right: 12.0,
            ..iced::Padding::ZERO
        })
        .center_y(HEIGHT);
        let mut right = row![feedback].height(HEIGHT);
        if frame == Frame::Own {
            let (glyph, name) = if self.title_bar.maximized {
                (WindowGlyph::Restore, i18n::tr("Restore"))
            } else {
                (WindowGlyph::Maximize, i18n::tr("Maximise"))
            };
            right = right
                .push(window_button(
                    WindowGlyph::Minimize,
                    i18n::tr("Minimise"),
                    WindowAction::Minimize,
                    false,
                ))
                .push(window_button(
                    glyph,
                    name,
                    WindowAction::ToggleMaximize,
                    false,
                ))
                .push(window_button(
                    WindowGlyph::Close,
                    i18n::tr("Close"),
                    WindowAction::Close,
                    true,
                ));
        }
        let bar = row![left, Space::with_width(Fill), right]
            .width(Fill)
            .height(HEIGHT)
            .align_y(iced::Alignment::Center);
        // The empty part of the bar moves the window and opens the menu of
        // the window on a right click; the title lets the pointer through
        // to it.
        let drag = mouse_area(Space::new(Fill, HEIGHT))
            .on_press(Message::Window(WindowAction::BarPressed))
            .on_right_press(Message::Window(WindowAction::SystemMenu));
        let mut layers = stack![drag, bar].width(Fill).height(HEIGHT);
        if frame != Frame::System {
            let accent = |theme: &Theme| text::Style {
                color: Some(ui_theme::colors(theme).accent),
            };
            let title = row![
                text(crate::APP_NAME).size(12).style(accent),
                text(crate::VERSION_LABEL)
                    .size(10)
                    .style(|theme: &Theme| text::Style {
                        color: Some(ui_theme::colors(theme).accent.scale_alpha(0.5)),
                    }),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center);
            layers = layers.push(container(title).center_x(Fill).center_y(HEIGHT));
        }
        container(layers)
            .width(Fill)
            .height(HEIGHT)
            .style(|theme| {
                container::Style::default().background(ui_theme::colors(theme).bg_lighter)
            })
            .into()
    }

    /// The window with the edges that resize it over everything, while
    /// they do.
    pub(crate) fn with_edges<'a>(&self, window: Element<'a, Message>) -> Element<'a, Message> {
        if self.title_bar.resizes() {
            stack![window, Element::new(Edges)]
                .width(Fill)
                .height(Fill)
                .into()
        } else {
            window
        }
    }
}

/// A button of the window, 40 by 32 with its glyph of 10 in the accent
/// colour, named in its tooltip.
fn window_button<'a>(
    glyph: WindowGlyph,
    name: &str,
    action: WindowAction,
    close: bool,
) -> Element<'a, Message> {
    let control = button(container(icons::glyph(glyph.handle(), 10.0)).center(10))
        .on_press(Message::Window(action))
        .width(40)
        .height(HEIGHT)
        .padding([(HEIGHT - 10.0) / 2.0, 15.0])
        .style(move |theme, status| ui_style::window_button(theme, close, status));
    ui_style::ribbon_tooltip(control, name.to_owned()).into()
}

/// The edges of the window, over the whole of it: they show the pointer of
/// resizing and start a resize on a press, and leave the rest of the
/// window to what lies under them.
struct Edges;

impl Edges {
    fn edge(layout: Layout<'_>, cursor: mouse::Cursor) -> Option<Edge> {
        let bounds = layout.bounds();
        let position = cursor.position_in(bounds)?;
        Edge::at(position, bounds.size())
    }
}

impl Widget<Message, Theme, Renderer> for Edges {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::Node::new(limits.max())
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn on_event(
        &mut self,
        _tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) -> event::Status {
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(edge) = Self::edge(layout, cursor) {
                shell.publish(Message::Window(WindowAction::Resize(edge)));
                return event::Status::Captured;
            }
        }
        event::Status::Ignored
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        Self::edge(layout, cursor).map_or(mouse::Interaction::None, Edge::interaction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_theme::UiTheme;
    use iced::Color;

    /// The messages the work of `task` gives, as the window would run it.
    fn outputs(task: Task<Message>) -> Vec<Message> {
        use iced::futures::StreamExt;
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Vec::new();
        };
        let actions: Vec<_> = iced::futures::executor::block_on(stream.collect());
        actions
            .into_iter()
            .filter_map(|action| match action {
                iced_runtime::Action::Output(message) => Some(message),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn two_presses_close_in_time_maximise_and_others_drag() {
        let start = Instant::now();
        let mut bar = TitleBar::new(Frame::Own);
        assert_eq!(bar.press(start), Request::Drag);
        assert_eq!(
            bar.press(start + Duration::from_millis(300)),
            Request::ToggleMaximize
        );
        // A third press starts again.
        assert_eq!(bar.press(start + Duration::from_millis(500)), Request::Drag);
        // A press long after the last is a drag of its own.
        assert_eq!(
            bar.press(start + Duration::from_millis(1000)),
            Request::Drag
        );
        assert_eq!(
            bar.press(start + Duration::from_millis(1400)),
            Request::ToggleMaximize
        );
        // Where the system frames the window, a double click is its own.
        for frame in [Frame::MacOs, Frame::System] {
            let mut bar = TitleBar::new(frame);
            assert_eq!(bar.press(start), Request::Drag);
            assert_eq!(bar.press(start + Duration::from_millis(100)), Request::Drag);
        }
    }

    #[test]
    fn an_edge_is_found_by_where_the_pointer_is() {
        let size = Size::new(1440.0, 900.0);
        let at = |x: f32, y: f32| Edge::at(Point::new(x, y), size);
        assert_eq!(at(0.0, 0.0), Some(Edge::NorthWest));
        assert_eq!(at(3.0, 450.0), Some(Edge::West));
        assert_eq!(at(720.0, 2.0), Some(Edge::North));
        assert_eq!(at(1439.0, 450.0), Some(Edge::East));
        assert_eq!(at(720.0, 899.0), Some(Edge::South));
        assert_eq!(at(1439.0, 899.0), Some(Edge::SouthEast));
        assert_eq!(at(1439.0, 0.0), Some(Edge::NorthEast));
        assert_eq!(at(0.0, 899.0), Some(Edge::SouthWest));
        // Along an edge, near a corner, both ways.
        assert_eq!(at(10.0, 2.0), Some(Edge::NorthWest));
        assert_eq!(at(2.0, 890.0), Some(Edge::SouthWest));
        assert_eq!(at(13.0, 2.0), Some(Edge::North));
        // Inside the window, and outside it: none.
        assert_eq!(at(720.0, 450.0), None);
        assert_eq!(at(6.0, 450.0), None);
        assert_eq!(at(1433.0, 450.0), None);
        assert_eq!(at(1434.0, 450.0), Some(Edge::East));
        assert_eq!(at(-1.0, 450.0), None);
        assert_eq!(at(720.0, 900.0), None);
        // The pointer says which way.
        assert_eq!(
            Edge::North.interaction(),
            mouse::Interaction::ResizingVertically
        );
        assert_eq!(
            Edge::West.interaction(),
            mouse::Interaction::ResizingHorizontally
        );
        assert_eq!(
            Edge::NorthEast.interaction(),
            mouse::Interaction::ResizingDiagonallyUp
        );
        assert_eq!(
            Edge::SouthEast.interaction(),
            mouse::Interaction::ResizingDiagonallyDown
        );
    }

    #[test]
    fn only_windows_has_the_frame_of_the_application() {
        // Linux keeps the frame of the system, on X11 as on Wayland, until a
        // window without one can be resized there.
        let expected = if cfg!(windows) {
            Frame::Own
        } else if cfg!(target_os = "macos") {
            Frame::MacOs
        } else {
            Frame::System
        };
        assert_eq!(Frame::current(), expected);
        assert!(!Frame::Own.decorations());
        assert!(Frame::MacOs.decorations() && Frame::System.decorations());
    }

    #[test]
    fn the_edges_resize_only_a_window_the_application_frames_and_that_is_not_maximised() {
        let mut studio = Studio {
            title_bar: TitleBar::new(Frame::Own),
            ..Studio::default()
        };
        assert!(studio.title_bar.resizes());
        let _ = studio.update(Message::Window(WindowAction::Maximized(true)));
        assert!(studio.title_bar.maximized);
        assert!(!studio.title_bar.resizes());
        let _ = studio.update(Message::Window(WindowAction::Maximized(false)));
        assert!(studio.title_bar.resizes());
        studio.title_bar = TitleBar::new(Frame::System);
        assert!(!studio.title_bar.resizes());
    }

    #[test]
    fn a_resize_asks_whether_the_window_is_maximised_in_every_frame() {
        for frame in [Frame::Own, Frame::MacOs, Frame::System] {
            let mut studio = Studio {
                title_bar: TitleBar::new(frame),
                ..Studio::default()
            };
            let task = studio.update(Message::WindowResized(Size::new(1200.0, 800.0)));
            assert!(iced_runtime::task::into_stream(task).is_some(), "{frame:?}");
            let _ = studio.update(Message::Window(WindowAction::Maximized(true)));
            assert!(studio.title_bar.maximized, "{frame:?}");
            // `status` reports it.
            let command = serde_json::from_value(serde_json::json!({"command": "status"})).unwrap();
            let (reply, receive) = std::sync::mpsc::channel();
            let _ = studio.handle_api(crate::native_api::ApiRequest { command, reply });
            let status = receive.recv().unwrap();
            assert_eq!(
                status["result"]["window"],
                serde_json::json!({"frame": frame.key(), "maximized": true})
            );
        }
    }

    /// `studio`'s window drawn in a window of 1440 by 900, in Light.
    fn picture(studio: &Studio) -> crate::test_render::Picture {
        crate::test_render::render_window(studio, Size::new(1440.0, 900.0))
    }

    /// Whether a pixel of `picture` in the columns `columns` of the bar has
    /// the colour `color`, give or take a little.
    fn inked(
        picture: &crate::test_render::Picture,
        columns: std::ops::Range<u32>,
        color: Color,
    ) -> bool {
        let [r, g, b, _] = color.into_rgba8();
        columns.into_iter().any(|x| {
            (0..HEIGHT as u32).any(|y| {
                let [red, green, blue] = picture.rgb(x, y);
                red.abs_diff(r) < 24 && green.abs_diff(g) < 24 && blue.abs_diff(b) < 24
            })
        })
    }

    #[test]
    fn the_bar_holds_the_title_and_the_window_buttons_of_its_frame() {
        let _language = i18n::TestLanguage::hold(i18n::Language::English);
        let mut studio = Studio::default();
        for theme in [UiTheme::Light, UiTheme::Forge] {
            studio.ui_theme = theme;
            let colors = theme.colors();
            studio.title_bar = TitleBar::new(Frame::Own);
            let own = picture(&studio);
            // The bar on the lighter shell, the ribbon under it.
            assert!(own.is(560, 2, colors.bg_lighter), "{}", theme.key());
            assert!(own.is(560, 30, colors.bg_lighter), "{}", theme.key());
            assert!(
                own.is(1000, HEIGHT as u32 + 27, colors.accent),
                "{}",
                theme.key()
            );
            // The title in the middle in the accent colour, and the three
            // glyphs of the window buttons at the right.
            assert!(inked(&own, 660..780, colors.accent), "{}", theme.key());
            assert!(inked(&own, 1320..1440, colors.accent), "{}", theme.key());
            // The logo, and the line before the quick-access buttons 6
            // after it, as the style book sets them: the logo from 8 to 28,
            // the line at 34.
            assert!(inked(&own, 8..28, colors.accent), "{}", theme.key());
            assert!(own.is(34, 16, colors.accent), "{}", theme.key());
            for x in [29, 33, 35, 38] {
                assert!(own.is(x, 16, colors.bg_lighter), "{} {x}", theme.key());
            }
            assert!(inked(&own, 1410..1430, colors.accent), "{}", theme.key());
            // A maximised window shows the glyph that restores it.
            studio.title_bar.maximized = true;
            let maximized = picture(&studio);
            let differs = (1360..1400)
                .any(|x| (0..HEIGHT as u32).any(|y| own.rgb(x, y) != maximized.rgb(x, y)));
            assert!(differs, "{}", theme.key());
            // Under the frame of the system on Linux: no title and no
            // window buttons, Send Feedback at the right end.
            studio.title_bar = TitleBar::new(Frame::System);
            let system = picture(&studio);
            assert!(!inked(&system, 660..780, colors.accent), "{}", theme.key());
            assert!(
                !inked(&system, 1430..1440, colors.accent),
                "{}",
                theme.key()
            );
            assert!(inked(&system, 1300..1430, colors.accent), "{}", theme.key());
        }
    }

    /// The places of the bar a test presses on: the empty part, Import
    /// point cloud, Export active point cloud, Undo, and the minimise and
    /// the close buttons of a window of 1440. The quick-access buttons are
    /// 28 wide from 39, 1 apart.
    const EMPTY: Point = Point::new(400.0, 16.0);
    const IMPORT: Point = Point::new(53.0, 16.0);
    const EXPORT: Point = Point::new(111.0, 16.0);
    const UNDO: Point = Point::new(140.0, 16.0);
    const MINIMISE: Point = Point::new(1340.0, 16.0);
    const CLOSE: Point = Point::new(1420.0, 16.0);
    const WINDOW: Size = Size::new(1440.0, 900.0);

    fn window_actions(messages: &[Message]) -> Vec<WindowAction> {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::Window(action) => Some(*action),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_press_on_a_quick_access_button_that_waits_does_not_move_the_window() {
        let _language = i18n::TestLanguage::hold(i18n::Language::English);
        let mut studio = Studio::default();
        studio.title_bar = TitleBar::new(Frame::Own);
        let press = [Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left,
        ))];
        // Without a scan or an edit, Export and Undo cannot be used: a press
        // on them, as the third of three quick ones on Undo, neither drags
        // nor maximises the window.
        for at in [EXPORT, UNDO] {
            let messages =
                crate::test_render::messages(studio.title_bar(false), WINDOW, at, &press);
            assert!(messages.is_empty(), "{at:?}: {messages:?}");
        }
        // The empty part of the bar drags the window, and Import, which can
        // always be used, imports.
        let messages = crate::test_render::messages(studio.title_bar(false), WINDOW, EMPTY, &press);
        assert_eq!(window_actions(&messages), [WindowAction::BarPressed]);
        let click = crate::test_render::click(mouse::Button::Left);
        let messages =
            crate::test_render::messages(studio.title_bar(false), WINDOW, IMPORT, &click);
        assert!(
            matches!(messages.as_slice(), [Message::Open]),
            "{messages:?}"
        );
    }

    #[test]
    fn the_bar_moves_minimises_and_closes_the_window_while_a_dialog_is_open() {
        let _language = i18n::TestLanguage::hold(i18n::Language::English);
        let mut studio = Studio::default();
        studio.title_bar = TitleBar::new(Frame::Own);
        let _ = studio.update(Message::Settings(
            crate::settings_dialog::SettingsAction::Open,
        ));
        assert!(studio.settings_view().is_some());
        let click = crate::test_render::click(mouse::Button::Left);
        let at = |point: Point, events: &[Event]| {
            window_actions(&crate::test_render::messages(
                studio.view(),
                WINDOW,
                point,
                events,
            ))
        };
        assert_eq!(at(EMPTY, &click), [WindowAction::BarPressed]);
        assert_eq!(
            at(EMPTY, &crate::test_render::click(mouse::Button::Right)),
            [WindowAction::SystemMenu]
        );
        assert_eq!(at(MINIMISE, &click), [WindowAction::Minimize]);
        assert_eq!(at(CLOSE, &click), [WindowAction::Close]);
        // The quick-access buttons wait until the dialog is closed, as the
        // keys do.
        let messages = crate::test_render::messages(studio.view(), WINDOW, IMPORT, &click);
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::Open)),
            "{messages:?}"
        );
        let _ = studio.update(Message::Settings(
            crate::settings_dialog::SettingsAction::Cancel,
        ));
        assert!(studio.settings_view().is_none());
        let messages = crate::test_render::messages(studio.view(), WINDOW, IMPORT, &click);
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, Message::Open)),
            "{messages:?}"
        );
    }

    #[test]
    fn a_right_click_on_the_bar_opens_the_menu_of_the_window_where_the_application_frames_it() {
        for (frame, opens) in [
            (Frame::Own, true),
            (Frame::MacOs, false),
            (Frame::System, false),
        ] {
            let mut studio = Studio {
                title_bar: TitleBar::new(frame),
                ..Studio::default()
            };
            let task = studio.window_action(WindowAction::SystemMenu);
            assert_eq!(
                iced_runtime::task::into_stream(task).is_some(),
                opens,
                "{frame:?}"
            );
        }
    }

    #[test]
    fn the_close_button_goes_the_way_of_the_system_close_button() {
        let mut studio = Studio::default();
        let task = studio.window_action(WindowAction::Close);
        // The task gives Exit, which asks work under way to stop first.
        let messages = outputs(task);
        assert!(
            matches!(messages.as_slice(), [Message::Exit]),
            "{messages:?}"
        );
    }
}
