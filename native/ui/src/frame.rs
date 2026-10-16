//! Shared vertical application frame; tools provide their domain content.
use crate::{model::metrics, Theme};
use iced::{
    widget::{column, container, Column, Row, Space},
    Alignment, Color, Element, Fill, Shrink,
};

/// Dimensions for a workspace shell whose navigation may wrap onto several
/// rows. The application still owns the commands, panels, and resize state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceFrameProfile {
    pub title_height: u16,
    pub status_height: u16,
    pub left_panel_width: u16,
    pub right_panel_width: u16,
    pub panel_header_height: u16,
}

/// Source-measured shell for navigation-first tools without a permanent ribbon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SidebarFrameProfile {
    pub title_height: u16,
    pub sidebar_width: u16,
    pub status_height: u16,
}

impl SidebarFrameProfile {
    /// Workspace Composer at source commit 33327e0.
    pub const WORKSPACE_COMPOSER: Self = Self {
        title_height: 32,
        sidebar_width: 220,
        status_height: 22,
    };
}

/// Keep navigation, application content, and status in a reusable native frame.
pub fn sidebar_frame<'a, M: 'a>(
    profile: SidebarFrameProfile,
    titlebar: Element<'a, M>,
    sidebar: Element<'a, M>,
    content: Element<'a, M>,
    status: Element<'a, M>,
) -> Element<'a, M> {
    column![
        container(titlebar).width(Fill).height(profile.title_height),
        Row::new()
            .push(container(sidebar).width(profile.sidebar_width).height(Fill))
            .push(container(content).width(Fill).height(Fill))
            .width(Fill)
            .height(Fill),
        container(status).width(Fill).height(profile.status_height),
    ]
    .width(Fill)
    .height(Fill)
    .into()
}

impl WorkspaceFrameProfile {
    /// Source-measured Open Frame Studio defaults at commit eb77f9e.
    pub const OPEN_FRAME: Self = Self {
        title_height: 36,
        status_height: 28,
        left_panel_width: 220,
        right_panel_width: 290,
        panel_header_height: 26,
    };
}

/// Tauri Style Book document band: 30 px, one-pixel rules and 4 px inset.
/// The caller owns document identity, content and activation behavior.
pub fn document_bar<'a, M: 'a>(theme: Theme, content: Element<'a, M>) -> Element<'a, M> {
    let tokens = theme.tokens();
    let blend = |overlay: Color| Color {
        r: tokens.docbar_bg.r * (1.0 - overlay.a) + overlay.r * overlay.a,
        g: tokens.docbar_bg.g * (1.0 - overlay.a) + overlay.g * overlay.a,
        b: tokens.docbar_bg.b * (1.0 - overlay.a) + overlay.b * overlay.a,
        a: 1.0,
    };
    let top = blend(Color::from_rgba8(217, 119, 6, 0.15));
    let bottom = blend(tokens.border);
    column![
        container(Space::new(Fill, 1))
            .width(Fill)
            .style(move |_| container::Style::default().background(top)),
        container(content)
            .width(Fill)
            .height(metrics::DOCUMENTS - 2)
            .padding([0, 4])
            .align_y(Alignment::End)
            .style(move |_| container::Style::default().background(tokens.docbar_bg)),
        container(Space::new(Fill, 1))
            .width(Fill)
            .style(move |_| container::Style::default().background(bottom)),
    ]
    .width(Fill)
    .height(metrics::DOCUMENTS)
    .into()
}

/// Common 22 px status band with a one-pixel top rule. Applications supply
/// their own status content while the crate owns its geometry and palette.
pub fn status_bar<'a, M: 'a>(
    theme: Theme,
    content: Element<'a, M>,
    horizontal_padding: u16,
) -> Element<'a, M> {
    status_bar_with_height(theme, content, horizontal_padding, metrics::STATUS)
}

/// The same status treatment at an application-specific band height.
pub fn status_bar_with_height<'a, M: 'a>(
    theme: Theme,
    content: Element<'a, M>,
    horizontal_padding: u16,
    height: u16,
) -> Element<'a, M> {
    let tokens = theme.tokens();
    column![
        container(Space::new(Fill, 1))
            .width(Fill)
            .style(move |_| container::Style::default().background(tokens.status_border)),
        container(content)
            .padding([0, horizontal_padding])
            .width(Fill)
            .height(height.saturating_sub(1))
            .style(move |_| {
                container::Style::default()
                    .background(tokens.status_bg)
                    .color(tokens.status_text)
            }),
    ]
    .width(Fill)
    .height(height)
    .into()
}

/// Shell for tools with a ribbon that includes its own tab row, followed by
/// workspace tabs whose height is determined by their wrapped content.
pub struct WorkspaceFrameSlots<'a, M> {
    pub titlebar: Element<'a, M>,
    pub ribbon: Element<'a, M>,
    pub workspace_tabs: Element<'a, M>,
    pub content: Element<'a, M>,
    pub status: Element<'a, M>,
}

pub fn workspace_frame<'a, M: 'a>(
    slots: WorkspaceFrameSlots<'a, M>,
    profile: WorkspaceFrameProfile,
) -> Element<'a, M> {
    column![
        container(slots.titlebar)
            .height(profile.title_height)
            .width(Fill),
        container(slots.ribbon).height(Shrink).width(Fill),
        container(slots.workspace_tabs).height(Shrink).width(Fill),
        container(slots.content).height(Fill).width(Fill),
        container(slots.status)
            .height(profile.status_height)
            .width(Fill),
    ]
    .height(Fill)
    .width(Fill)
    .into()
}

/// Frame Studio's two resizable side regions can be omitted independently.
/// Widths are defaults; callers may derive a profile with persisted values.
pub fn workspace_panes<'a, M: 'a>(
    profile: WorkspaceFrameProfile,
    left: Option<Element<'a, M>>,
    content: Element<'a, M>,
    right: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let mut regions = Row::new();
    if let Some(left) = left {
        regions = regions.push(container(left).width(profile.left_panel_width).height(Fill));
    }
    regions = regions.push(container(content).width(Fill).height(Fill));
    if let Some(right) = right {
        regions = regions.push(
            container(right)
                .width(profile.right_panel_width)
                .height(Fill),
        );
    }
    regions.width(Fill).height(Fill).into()
}

pub struct FrameSlots<'a, M> {
    pub titlebar: Element<'a, M>,
    pub tabs: Element<'a, M>,
    pub ribbon: Option<Element<'a, M>>,
    pub documents: Option<Element<'a, M>>,
    pub content: Element<'a, M>,
    pub status: Element<'a, M>,
}

pub fn frame<'a, M: 'a>(slots: FrameSlots<'a, M>) -> Element<'a, M> {
    let mut bands = Column::new()
        .push(container(slots.titlebar).height(metrics::TITLE).width(Fill))
        .push(container(slots.tabs).height(metrics::TABS).width(Fill));
    if let Some(ribbon) = slots.ribbon {
        bands = bands.push(container(ribbon).height(metrics::RIBBON).width(Fill));
    }
    if let Some(documents) = slots.documents {
        bands = bands.push(container(documents).height(metrics::DOCUMENTS).width(Fill));
    }
    bands
        .push(container(slots.content).height(Fill).width(Fill))
        .push(container(slots.status).height(metrics::STATUS).width(Fill))
        .height(Fill)
        .width(Fill)
        .into()
}

/// Compact outer frame for step-by-step applications such as Energy Studio.
/// The title, document and status bands keep their shared dimensions.
pub fn workflow_frame<'a, M: 'a>(
    titlebar: Element<'a, M>,
    documents: Option<Element<'a, M>>,
    content: Element<'a, M>,
    status: Element<'a, M>,
) -> Element<'a, M> {
    let mut bands = Column::new().push(container(titlebar).height(metrics::TITLE).width(Fill));
    if let Some(documents) = documents {
        bands = bands.push(container(documents).height(metrics::DOCUMENTS).width(Fill));
    }
    bands
        .push(container(content).height(Fill).width(Fill))
        .push(container(status).height(metrics::STATUS).width(Fill))
        .height(Fill)
        .width(Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::{layout, widget::Tree};
    use iced::Size;

    fn band(height: u16) -> Element<'static, ()> {
        container(Space::new(Fill, height)).width(Fill).into()
    }

    #[test]
    fn workspace_bands_grow_with_wrapped_navigation() {
        let profile = WorkspaceFrameProfile::OPEN_FRAME;
        let element = workspace_frame(
            WorkspaceFrameSlots {
                titlebar: band(profile.title_height),
                ribbon: band(132),
                workspace_tabs: band(145),
                content: band(1),
                status: band(profile.status_height),
            },
            profile,
        );
        let renderer = iced_tiny_skia::Renderer::new(crate::fonts::BODY, iced::Pixels(12.0));
        let mut tree = Tree::new(&element);
        let node = element.as_widget().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(1200.0, 800.0)),
        );
        let bands: Vec<_> = node.children().iter().map(|child| child.bounds()).collect();
        assert_eq!(bands.len(), 5);
        assert_eq!(bands[0].height, 36.0);
        assert_eq!(bands[1].height, 132.0);
        assert_eq!(bands[2].height, 145.0);
        assert_eq!(bands[3].y, 313.0);
        assert_eq!(bands[4].height, 28.0);
        assert_eq!(bands[4].y, 772.0);
    }

    #[test]
    fn optional_workspace_panes_keep_asymmetric_source_widths() {
        let profile = WorkspaceFrameProfile::OPEN_FRAME;
        let element = workspace_panes(profile, Some(band(1)), band(1), Some(band(1)));
        let renderer = iced_tiny_skia::Renderer::new(crate::fonts::BODY, iced::Pixels(12.0));
        let mut tree = Tree::new(&element);
        let node = element.as_widget().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(1200.0, 600.0)),
        );
        let panes: Vec<_> = node.children().iter().map(|child| child.bounds()).collect();
        assert_eq!(panes.len(), 3);
        assert_eq!(panes[0].width, 220.0);
        assert_eq!(panes[1].width, 690.0);
        assert_eq!(panes[2].width, 290.0);
    }

    #[test]
    fn composer_sidebar_frame_preserves_source_geometry() {
        let profile = SidebarFrameProfile::WORKSPACE_COMPOSER;
        let element = sidebar_frame(profile, band(1), band(1), band(1), band(1));
        let renderer = iced_tiny_skia::Renderer::new(crate::fonts::BODY, iced::Pixels(12.0));
        let mut tree = Tree::new(&element);
        let node = element.as_widget().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(1200.0, 800.0)),
        );
        let bands = node.children();
        assert_eq!(bands.len(), 3);
        assert_eq!(bands[0].bounds().height, 32.0);
        assert_eq!(bands[1].bounds().height, 746.0);
        assert_eq!(bands[2].bounds().height, 22.0);
        let panes = bands[1].children();
        assert_eq!(panes.len(), 2);
        assert_eq!(panes[0].bounds().width, 220.0);
        assert_eq!(panes[1].bounds().width, 980.0);
    }
}
