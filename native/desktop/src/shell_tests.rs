//! Tests of the shell of the window: its title, its status bar, its colours
//! in each theme, the header of the model space, the keys while the File
//! view is open, the language and theme commands of the local API and what
//! its status says is under way.

use std::sync::Arc;

use pointcloud_core::IndexedPoint;
use serde_json::Value;

use crate::i18n::{self, Language, TestLanguage};
use crate::native_api::{ApiCommand, ApiRequest};
use crate::selection::SelectionMask;
use crate::ui_theme::UiTheme;
use crate::{
    app_title, title_for, view_cube, CameraPreset, CloudEntry, Message, ModelKey, Studio, ToolIcon,
    APP_NAME, VERSION_LABEL,
};

#[test]
fn drawing_and_transform_tools_use_shared_domain_icons() {
    use openaec_ui::icons::DomainIcon;
    assert_eq!(
        crate::domain_icon(crate::ToolIcon::Note),
        Some(DomainIcon::DrawingNote)
    );
    assert_eq!(
        crate::domain_icon(crate::ToolIcon::Dimension),
        Some(DomainIcon::DrawingDimension)
    );
    assert_eq!(
        crate::domain_icon(crate::ToolIcon::Move),
        Some(DomainIcon::TransformMove)
    );
    assert_eq!(
        crate::domain_icon(crate::ToolIcon::Line),
        Some(DomainIcon::DrawingLine)
    );
    assert_eq!(
        crate::domain_icon(crate::ToolIcon::Text),
        Some(DomainIcon::DrawingText)
    );
    assert_eq!(
        crate::domain_icon(crate::ToolIcon::Leader),
        Some(DomainIcon::DrawingLeader)
    );
    for icon in [
        DomainIcon::DrawingNote,
        DomainIcon::DrawingDimension,
        DomainIcon::TransformMove,
        DomainIcon::DrawingLine,
        DomainIcon::DrawingText,
        DomainIcon::DrawingLeader,
    ] {
        assert_ne!(
            icon.bytes(16),
            icon.bytes(24),
            "small form is reviewed separately"
        );
    }
}

#[test]
fn explorer_commands_use_shared_semantic_icons() {
    use openaec_ui::icons::Icon;
    for (tool, expected) in [
        (crate::ToolIcon::Duplicate, Icon::Duplicate),
        (crate::ToolIcon::Rename, Icon::Rename),
        (crate::ToolIcon::ChevronOpen, Icon::DisclosureExpanded),
        (crate::ToolIcon::ChevronClosed, Icon::DisclosureCollapsed),
    ] {
        assert_eq!(crate::canonical_icon(tool), Some(expected));
        assert!(!expected.bytes().is_empty());
    }
}

/// Every icon of the window. The match below has no arm for the rest, so
/// an icon added to `ToolIcon` has to be added to this list as well.
fn every_tool_icon() -> Vec<ToolIcon> {
    use ToolIcon as I;
    let icons = [
        I::Open,
        I::OpenFolder,
        I::SelectionExport,
        I::Export,
        I::Fit,
        I::ZoomIn,
        I::ZoomOut,
        I::Pan,
        I::FitSelection,
        I::Cloud,
        I::Select,
        I::WindowSelect,
        I::DeletePoints,
        I::SectionBox,
        I::Pick,
        I::MeasureDistance,
        I::MeasureArea,
        I::Note,
        I::Line,
        I::Clear,
        I::Undo,
        I::Redo,
        I::Save,
        I::Move,
        I::Scale,
        I::Decimate,
        I::Mesh,
        I::Building,
        I::Shading,
        I::Drawing,
        I::ClosedMesh,
        I::Faces,
        I::MeshToPlans,
        I::MeshPointcloud,
        I::MeshTerrain,
        I::MeshSurface,
        I::Scan,
        I::Classes,
        I::Views,
        I::Model,
        I::SavedView,
        I::PlanSheet,
        I::ElevationSheet,
        I::SectionSheet,
        I::DrawingFile,
        I::Bcf,
        I::ChevronOpen,
        I::ChevronClosed,
        I::Duplicate,
        I::Rename,
        I::Update,
        I::Sheet,
        I::Locked,
        I::Unlocked,
        I::Text,
        I::Dimension,
        I::Leader,
    ];
    for icon in icons {
        match icon {
            I::Open
            | I::OpenFolder
            | I::SelectionExport
            | I::Export
            | I::Fit
            | I::ZoomIn
            | I::ZoomOut
            | I::Pan
            | I::FitSelection
            | I::Cloud
            | I::Select
            | I::WindowSelect
            | I::DeletePoints
            | I::SectionBox
            | I::Pick
            | I::MeasureDistance
            | I::MeasureArea
            | I::Note
            | I::Line
            | I::Clear
            | I::Undo
            | I::Redo
            | I::Camera(_)
            | I::Save
            | I::Move
            | I::Scale
            | I::Decimate
            | I::Mesh
            | I::Building
            | I::Shading
            | I::Drawing
            | I::ClosedMesh
            | I::Faces
            | I::MeshToPlans
            | I::MeshPointcloud
            | I::MeshTerrain
            | I::MeshSurface
            | I::Scan
            | I::Classes
            | I::Views
            | I::Model
            | I::SavedView
            | I::PlanSheet
            | I::ElevationSheet
            | I::SectionSheet
            | I::DrawingFile
            | I::Bcf
            | I::ChevronOpen
            | I::ChevronClosed
            | I::Duplicate
            | I::Rename
            | I::Update
            | I::Sheet
            | I::Locked
            | I::Unlocked
            | I::Text
            | I::Dimension
            | I::Leader => {}
        }
    }
    let cameras = [
        CameraPreset::Top,
        CameraPreset::Bottom,
        CameraPreset::Front,
        CameraPreset::Back,
        CameraPreset::Right,
        CameraPreset::Left,
        CameraPreset::Isometric,
    ]
    .map(I::Camera);
    icons.into_iter().chain(cameras).collect()
}

/// The contrast between two colours, as WCAG reckons it.
fn contrast(one: [u8; 3], other: [u8; 3]) -> f32 {
    let luminance = |[red, green, blue]: [u8; 3]| {
        let linear = |channel: u8| {
            let value = f32::from(channel) / 255.0;
            if value <= 0.040_45 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
    };
    let (one, other) = (luminance(one), luminance(other));
    (one.max(other) + 0.05) / (one.min(other) + 0.05)
}

/// How far every part of an icon has to stand out from the ribbon. The
/// fixed dark greys of the artwork stand out by 1.75 to 1 at most on the
/// dark ribbons, each other colour of it by more than 2.5 to 1.
const ICON_MIN_CONTRAST: f32 = 2.0;

#[test]
fn every_icon_stands_out_from_the_ribbon_in_the_dark_themes() {
    use iced::widget::{container, svg};

    const SIZE: f32 = 32.0;
    let size = iced::Size::new(SIZE, SIZE);
    for theme in [
        UiTheme::Forge,
        UiTheme::OpenAec,
        UiTheme::Blueprint,
        UiTheme::Contrast,
    ] {
        let colors = theme.colors();
        let [red, green, blue, _] = colors.bg.into_rgba8();
        let ribbon = [red, green, blue];
        let on_ribbon = |icon: iced::Element<'static, Message>| {
            crate::test_render::render(
                container(icon)
                    .center(iced::Fill)
                    .style(move |_| container::Style::default().background(colors.bg))
                    .into(),
                &theme.iced(),
                size,
            )
        };
        for icon in every_tool_icon() {
            let drawn = on_ribbon(crate::icon_svg(icon, SIZE));
            let shared =
                crate::canonical_icon(icon).is_some() || crate::domain_icon(icon).is_some();
            let artwork = (!shared).then(|| crate::icon_artwork(icon));
            let as_filed = |tint: Option<iced::Color>| {
                on_ribbon(
                    svg(svg::Handle::from_memory(artwork.expect("artwork")))
                        .width(SIZE)
                        .height(SIZE)
                        .style(move |_, _| svg::Style { color: tint })
                        .into(),
                )
            };
            // The shared icons are drawn in the text colour as a whole; the
            // artwork is drawn so as well to find what it covers.
            let tinted;
            let silhouette = if shared {
                &drawn
            } else {
                tinted = as_filed(Some(colors.text));
                &tinted
            };
            // The pixels the icon covers fully, which show the colour each
            // part of it is drawn in.
            let full: Vec<(u32, u32)> = (0..silhouette.height)
                .flat_map(|y| (0..silhouette.width).map(move |x| (x, y)))
                .filter(|&(x, y)| silhouette.is(x, y, colors.text))
                .collect();
            assert!(full.len() > 20, "{icon:?} in {}", theme.key());
            let faint = |picture: &crate::test_render::Picture| {
                full.iter()
                    .filter(|&&(x, y)| contrast(picture.rgb(x, y), ribbon) < ICON_MIN_CONTRAST)
                    .count()
            };
            assert_eq!(
                faint(&drawn),
                0,
                "{icon:?} in {}: pixels of {} that hardly stand out",
                theme.key(),
                full.len()
            );
            let Some(artwork) = artwork else {
                continue;
            };
            // The amber of the artwork stays.
            let source = std::str::from_utf8(artwork).unwrap();
            if source.contains("#D97706") {
                let amber = full.iter().any(|&(x, y)| {
                    let [r, g, b] = drawn.rgb(x, y);
                    r.abs_diff(0xD9) <= 8 && g.abs_diff(0x77) <= 8 && b.abs_diff(0x06) <= 8
                });
                assert!(amber, "{icon:?} in {} keeps its amber", theme.key());
            }
            // The artwork as its file draws it would not do: its dark greys
            // are lost on the ribbon.
            if crate::themed_icon::DARK_GREYS
                .iter()
                .any(|grey| source.contains(grey))
            {
                assert!(
                    faint(&as_filed(None)) > 0,
                    "{icon:?} in {} stands out untouched",
                    theme.key()
                );
            }
        }
    }
}

fn send(studio: &mut Studio, command: ApiCommand) -> Value {
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.handle_api(ApiRequest { command, reply });
    receive.recv().unwrap()
}

fn set_language(studio: &mut Studio, language: &str) -> Value {
    send(
        studio,
        ApiCommand::SetLanguage {
            language: language.into(),
        },
    )
}

#[test]
fn title_is_the_name_and_the_version_as_one_text() {
    let version = env!("CARGO_PKG_VERSION");
    assert_eq!(APP_NAME, "Open Pointcloud Studio");
    assert_eq!(VERSION_LABEL, format!("v{version}"));
    assert_eq!(app_title(), format!("Open Pointcloud Studio v{version}"));
    assert_eq!(title_for(None), app_title());
    assert_eq!(
        title_for(Some("hall.e57")),
        format!("hall.e57 - Open Pointcloud Studio v{version}")
    );
}

#[test]
fn window_title_names_the_active_scan() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = Studio::default();
    assert_eq!(studio.window_title(), app_title());
    for name in ["hall.xyz", "roof.xyz"] {
        let path = directory.path().join(name);
        std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let _ = studio.update(Message::Loaded(Ok(cloud)));
    }
    let active = studio.active.expect("an opened scan is active");
    let name = crate::display_name(&studio.clouds[active].cloud.path).to_owned();
    assert_eq!(studio.window_title(), format!("{name} - {}", app_title()));

    let other = 1 - active;
    let _ = studio.update(Message::Select(other));
    let name = crate::display_name(&studio.clouds[other].cloud.path).to_owned();
    assert_eq!(studio.window_title(), format!("{name} - {}", app_title()));
}

#[test]
fn status_bar_builds_in_the_model_and_in_the_file_view() {
    let mut studio = Studio::default();
    let _ = studio.status_bar("Ready".into());
    let _ = studio.view();
    let _ = studio.update(Message::ToggleFile);
    let _ = studio.status_bar(studio.status.clone());
    let _ = studio.view();
}

#[test]
fn shared_shell_bands_render_at_tauri_template_heights() {
    use openaec_ui::model::metrics;
    assert_eq!(
        (
            metrics::TITLE,
            metrics::TABS,
            metrics::RIBBON,
            metrics::DOCUMENTS,
            metrics::STATUS
        ),
        (32, 28, 94, 30, 22)
    );
    assert_eq!(metrics::PANEL_TOOLBAR, 24);
    let mut studio = Studio::default();
    for theme in UiTheme::ALL {
        studio.ui_theme = theme;
        let colors = theme.colors();
        let picture = crate::test_render::render_window(&studio, iced::Size::new(1200.0, 700.0));
        for (y, expected) in [
            (16, colors.bg_lighter),
            (44, colors.bg_lighter),
            (90, colors.bg),
            (169, colors.docbar_bg),
            (690, colors.status_bg),
        ] {
            assert!(picture.is(2, y, expected), "{} at y={y}", theme.key());
        }
        let panel_top =
            (metrics::TITLE + metrics::TABS + metrics::RIBBON + metrics::DOCUMENTS) as u32;
        assert!(
            picture.is(100, panel_top + 22, colors.bg_lighter),
            "{} panel title",
            theme.key()
        );
        assert_ne!(
            picture.rgb(100, panel_top + 23),
            picture.rgb(100, panel_top + 22),
            "{} panel divider",
            theme.key()
        );
    }
}

#[test]
fn document_band_matches_style_book_edges() {
    let mut studio = Studio::default();
    // Tauri DocumentBar.css at dfdcd41: a 30 px band with an amber 15%
    // top rule and the theme border over its own document background.
    for (theme, top, bottom) in [
        (UiTheme::Light, [241, 226, 208], [222, 222, 222]),
        (UiTheme::Forge, [72, 57, 47], [89, 64, 42]),
    ] {
        studio.ui_theme = theme;
        let picture = crate::test_render::render_window(&studio, iced::Size::new(1200.0, 700.0));
        let y = (openaec_ui::model::metrics::TITLE
            + openaec_ui::model::metrics::TABS
            + openaec_ui::model::metrics::RIBBON) as u32;
        assert_eq!(picture.rgb(800, y), top, "{} top", theme.key());
        assert_eq!(
            picture.rgb(800, y + openaec_ui::model::metrics::DOCUMENTS as u32 - 1),
            bottom,
            "{} bottom",
            theme.key()
        );
    }
}

#[test]
fn ribbon_group_rules_follow_style_book_geometry() {
    // Ribbon.css at dfdcd41: a 4 px top inset, full-height right rule,
    // and a caption rule above the 9 px group label.
    let mut studio = Studio::default();
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let picture = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
        let bg = picture.rgb(135, 60);
        assert_eq!(picture.rgb(135, 63), bg, "{} top inset", theme.key());
        assert_ne!(picture.rgb(135, 64), bg, "{} separator start", theme.key());
        assert_eq!(
            picture.rgb(135, 64),
            picture.rgb(135, 153),
            "{} separator height",
            theme.key()
        );
        assert_ne!(
            picture.rgb(20, 133),
            picture.rgb(20, 132),
            "{} caption rule",
            theme.key()
        );
    }
}

#[test]
fn ribbon_tabs_match_the_shared_outline_and_rule_in_both_themes() {
    let mut studio = Studio::default();
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let picture = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
        let tokens = theme.shared().tokens();
        assert!(
            picture.is(40, 50, tokens.file_tab_bg),
            "{} File tab",
            theme.key()
        );
        assert!(
            picture.is(110, 32, tokens.accent),
            "{} active top border",
            theme.key()
        );
        assert!(
            picture.is(110, 59, tokens.bg),
            "{} active bottom gap",
            theme.key()
        );
        assert!(
            picture.is(500, 59, tokens.accent),
            "{} continuous rule",
            theme.key()
        );
        studio.file_open = true;
        let file = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
        assert!(
            !file.is(110, 32, tokens.accent),
            "{} Home inactive in File view",
            theme.key()
        );
        studio.file_open = false;
    }
}

#[test]
fn shared_property_rows_keep_a_28_pixel_rhythm_in_both_themes() {
    let mut studio = Studio::default();
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let picture = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
        let rule = picture.rgb(1100, 283);
        assert_eq!(picture.rgb(1100, 311), rule, "{} second row", theme.key());
        assert_eq!(picture.rgb(1100, 339), rule, "{} third row", theme.key());
        assert_ne!(
            picture.rgb(1100, 282),
            rule,
            "{} one-pixel rule",
            theme.key()
        );
        assert_ne!(
            picture.rgb(1100, 256),
            picture.rgb(1100, 232),
            "{} section boundary",
            theme.key()
        );
    }
}

#[test]
fn loaded_geometry_bounds_use_the_shared_property_grid() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bounds.xyz");
    std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let picture = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
        let rule = picture.rgb(1100, 605);
        assert_eq!(picture.rgb(1100, 633), rule, "{} Y row", theme.key());
        assert_eq!(picture.rgb(1100, 661), rule, "{} Z row", theme.key());
        assert_ne!(
            picture.rgb(1100, 604),
            rule,
            "{} one-pixel rule",
            theme.key()
        );
        // The old CAD control drew full borders around each row. The shared
        // grid has only the horizontal rule, leaving the label column open.
        assert_eq!(
            picture.rgb(1070, 590),
            picture.rgb(1100, 590),
            "{} no inner box",
            theme.key()
        );
    }
}

#[test]
fn display_popup_is_anchored_without_reflowing_the_viewport() {
    let mut studio = Studio::default();
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let closed = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
        studio.view_controls_open = true;
        let open = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0));
        assert_ne!(
            open.rgb(710, 390),
            closed.rgb(710, 390),
            "{} popup surface",
            theme.key()
        );
        assert_eq!(
            open.rgb(690, 390),
            closed.rgb(690, 390),
            "{} left viewport",
            theme.key()
        );
        assert_eq!(
            open.rgb(1018, 390),
            closed.rgb(1018, 390),
            "{} right viewport",
            theme.key()
        );
        studio.view_controls_open = false;
    }
}

#[test]
fn display_popup_closes_when_clicking_outside() {
    use iced::advanced::{layout, widget::Tree, Layout, Shell};
    use iced::{mouse, Size, Vector};

    crate::test_render::load_fonts();
    let studio = Studio {
        view_controls_open: true,
        ..Studio::default()
    };
    let mut element = studio.view();
    let renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        crate::fonts::REGULAR,
        iced::Pixels(12.0),
    ));
    let mut tree = Tree::new(&element);
    let size = Size::new(1280.0, 760.0);
    let node =
        element
            .as_widget()
            .layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    let mut overlay = element
        .as_widget_mut()
        .overlay(&mut tree, Layout::new(&node), &renderer, Vector::ZERO)
        .expect("open display overlay");
    let popup_node = overlay.layout(&renderer, size);
    let mut clipboard = iced::advanced::clipboard::Null;
    let mut messages = Vec::new();
    let mut shell = Shell::new(&mut messages);
    overlay.on_event(
        iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Layout::new(&popup_node),
        mouse::Cursor::Available(iced::Point::new(680.0, 390.0)),
        &renderer,
        &mut clipboard,
        &mut shell,
    );
    assert!(messages
        .iter()
        .any(|message| matches!(message, Message::ToggleViewControls)));
}

#[test]
fn shared_backstage_nav_keeps_page_rhythm_and_selected_marker() {
    let mut studio = Studio {
        file_open: true,
        ..Studio::default()
    };
    studio.file_page = crate::file_view::FilePage::Extensions;
    // New, the direct file and export entries and All export options… are
    // eleven rows of 40 px from y 74; under the rule follow Workspace at 535,
    // Extensions at 575 and About at 615, all above the footer at this height.
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let picture = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 900.0));
        let accent = theme.colors().accent;
        assert!(
            picture.is(1, 595, accent),
            "{} selected marker",
            theme.key()
        );
        assert!(!picture.is(1, 555, accent), "{} row above", theme.key());
        assert!(!picture.is(1, 635, accent), "{} row below", theme.key());
        assert_ne!(
            picture.rgb(259, 595),
            picture.rgb(260, 595),
            "{} 260 px edge",
            theme.key()
        );
    }
}

#[test]
fn the_about_row_of_the_file_menu_is_reached_by_scrolling_at_760_pixels() {
    use iced::mouse;

    let mut studio = Studio {
        file_open: true,
        ..Studio::default()
    };
    // The page that is shown carries the accent bar at the left of its row.
    studio.file_page = crate::file_view::FilePage::About;
    let size = iced::Size::new(1280.0, 760.0);
    let wheel = iced::Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Lines { x: 0.0, y: -20.0 },
    });
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let accent = theme.colors().accent;
        // The marked rows of the menu, between the ribbon tabs and the
        // status bar.
        let marked = |picture: &crate::test_render::Picture| -> Vec<u32> {
            (60..738).filter(|&y| picture.is(1, y, accent)).collect()
        };
        // At this height the menu ends above the About row ...
        let still = crate::test_render::render_window(&studio, size);
        assert!(marked(&still).len() < 40, "{} About in view", theme.key());
        // ... and the wheel over the menu brings its 40 pixels in.
        let over_menu = mouse::Cursor::Available(iced::Point::new(130.0, 300.0));
        let (scrolled, _) = crate::test_render::render_after(
            studio.view(),
            &theme.iced(),
            size,
            over_menu,
            std::slice::from_ref(&wheel),
        );
        // The scroll stops part of a pixel short of the row's last one.
        let rows = marked(&scrolled);
        assert!(rows.len() >= 39, "{} About after scrolling", theme.key());
        assert_eq!(
            rows.last().unwrap() - rows[0] + 1,
            rows.len() as u32,
            "{} one row",
            theme.key()
        );
        // There a click opens the page.
        let on_row = mouse::Cursor::Available(iced::Point::new(130.0, rows[20] as f32));
        let (_, messages) = crate::test_render::render_after(
            studio.view(),
            &theme.iced(),
            size,
            on_row,
            &[
                wheel.clone(),
                iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
        );
        assert!(
            messages.iter().any(|message| matches!(
                message,
                Message::FilePage(crate::file_view::FilePage::About)
            )),
            "{} About clicked",
            theme.key()
        );
    }
}

#[test]
#[ignore = "regenerate native review images on demand"]
fn export_harmonized_shell_previews() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../design");
    std::fs::create_dir_all(&folder).unwrap();
    let mut studio = Studio::default();
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        studio.ui_theme = theme;
        crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0))
            .save_png(&folder.join(format!("pointcloud-shell-{name}.png")));
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("review-scan.xyz");
    std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        studio.ui_theme = theme;
        crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0))
            .save_png(&folder.join(format!("pointcloud-shell-loaded-{name}.png")));
    }
    let mut extensions = Studio {
        file_open: true,
        ..Studio::default()
    };
    extensions.file_page = crate::file_view::FilePage::Extensions;
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        extensions.ui_theme = theme;
        crate::test_render::render_window(&extensions, iced::Size::new(1280.0, 760.0))
            .save_png(&folder.join(format!("pointcloud-extensions-{name}.png")));
    }
}

#[test]
#[ignore = "regenerate the anchored display popup review on demand"]
fn export_display_popover_review() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../design");
    std::fs::create_dir_all(&folder).unwrap();
    let mut studio = Studio {
        view_controls_open: true,
        ..Studio::default()
    };
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        studio.ui_theme = theme;
        crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0))
            .save_png(&folder.join(format!("pointcloud-display-{name}.png")));
    }
}

#[test]
#[ignore = "regenerate File task-page review images on demand"]
fn export_file_task_pages_review() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../design");
    std::fs::create_dir_all(&folder).unwrap();
    let mut studio = Studio {
        file_open: true,
        ..Studio::default()
    };
    // Two pages with a row in the menu, which marks the one that is shown.
    for (page, page_name) in [
        (crate::file_view::FilePage::New, "new"),
        (crate::file_view::FilePage::Export, "export"),
    ] {
        studio.file_page = page;
        for (theme, theme_name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
            studio.ui_theme = theme;
            crate::test_render::render_window(&studio, iced::Size::new(1280.0, 760.0))
                .save_png(&folder.join(format!("pointcloud-file-{page_name}-{theme_name}.png")));
        }
    }
}

#[test]
#[ignore = "regenerate minimum-size File view on demand"]
fn export_file_minimum_window_review() {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../design");
    std::fs::create_dir_all(&folder).unwrap();
    let mut studio = Studio {
        file_open: true,
        ..Studio::default()
    };
    // New, the first row, as the test of this size below has it.
    studio.file_page = crate::file_view::FilePage::New;
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        studio.ui_theme = theme;
        crate::test_render::render_window(&studio, iced::Size::new(720.0, 480.0))
            .save_png(&folder.join(format!("pointcloud-file-small-{name}.png")));
    }
}

#[test]
fn file_view_keeps_selected_page_and_footer_at_minimum_window() {
    let mut studio = Studio {
        file_open: true,
        ..Studio::default()
    };
    // The Open page has no row in the menu, whose direct entries do its
    // tasks; the local API still shows it. New is the first row.
    studio.file_page = crate::file_view::FilePage::New;
    for theme in [UiTheme::Light, UiTheme::Forge] {
        studio.ui_theme = theme;
        let picture = crate::test_render::render_window(&studio, iced::Size::new(720.0, 480.0));
        assert!(
            picture.is(1, 94, theme.colors().accent),
            "{} selected New page",
            theme.key()
        );
        assert_ne!(
            picture.rgb(20, 435),
            picture.rgb(300, 435),
            "{} footer visible",
            theme.key()
        );
    }
}

#[test]
#[ignore = "regenerate settings dialog review images on demand"]
fn export_shared_settings_review() {
    let _language = TestLanguage::hold(Language::English);
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../design");
    let mut studio = Studio::default();
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        studio.ui_theme = theme;
        studio.settings_action(crate::settings_dialog::SettingsAction::Open);
        studio.settings_action(crate::settings_dialog::SettingsAction::Tab(
            crate::settings_dialog::SettingsTab::Appearance,
        ));
        studio.settings_action(crate::settings_dialog::SettingsAction::ThemeMenu(true));
        crate::test_render::render_window(&studio, iced::Size::new(1280.0, 800.0))
            .save_png(&folder.join(format!("pointcloud-settings-{name}.png")));
        if theme == UiTheme::Forge {
            studio.settings_action(crate::settings_dialog::SettingsAction::ResetPrompt(true));
            crate::test_render::render_window(&studio, iced::Size::new(1280.0, 800.0))
                .save_png(&folder.join("pointcloud-settings-reset-forge.png"));
        }
        studio.settings_action(crate::settings_dialog::SettingsAction::Cancel);
    }
}

#[test]
#[ignore = "regenerate the section-box property-grid review on demand"]
fn export_section_box_property_review() {
    let _language = TestLanguage::hold(Language::English);
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../design");
    let mut studio = Studio {
        section_enabled: true,
        ..Studio::default()
    };
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        studio.ui_theme = theme;
        crate::test_render::render_window(&studio, iced::Size::new(1280.0, 1000.0))
            .save_png(&folder.join(format!("pointcloud-section-box-{name}.png")));
    }
}

#[test]
#[ignore = "regenerate shared general-modal review images on demand"]
fn export_sheet_dialog_review() {
    let _language = TestLanguage::hold(Language::English);
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../design");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("review-scan.xyz");
    std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
    let mut studio = Studio::default();
    let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    let _ = studio.update_sheet_dialog(crate::sheet_dialog::SheetAction::Open);
    for (theme, name) in [(UiTheme::Light, "light"), (UiTheme::Forge, "forge")] {
        studio.ui_theme = theme;
        crate::test_render::render_window(&studio, iced::Size::new(1280.0, 800.0))
            .save_png(&folder.join(format!("pointcloud-sheet-dialog-{name}.png")));
    }
}

#[test]
fn open_theme_menu_paints_its_swatch_above_the_form() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio {
        ui_theme: UiTheme::Light,
        ..Studio::default()
    };
    studio.settings_action(crate::settings_dialog::SettingsAction::Open);
    studio.settings_action(crate::settings_dialog::SettingsAction::Tab(
        crate::settings_dialog::SettingsTab::Appearance,
    ));
    let closed = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 800.0));
    studio.settings_action(crate::settings_dialog::SettingsAction::ThemeMenu(true));
    let open = crate::test_render::render_window(&studio, iced::Size::new(1280.0, 800.0));
    let accent = iced::Color::from_rgb8(0xD9, 0x77, 0x06);
    assert!(!closed.is(752, 260, accent));
    assert!(open.is(752, 260, accent));
}

#[test]
fn keys_leave_the_model_alone_while_the_file_view_covers_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scan.xyz");
    std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    let entry = &mut studio.clouds[0];
    let record = IndexedPoint {
        point: entry.cloud.points[0],
        ordinal: entry.cloud.point_ordinals[0],
    };
    entry.selection = Some(Arc::new(
        SelectionMask::single(entry.cloud.total_points, record).unwrap(),
    ));
    let remaining =
        |studio: &Studio| -> u64 { studio.clouds.iter().map(CloudEntry::remaining_count).sum() };
    let key = |studio: &mut Studio, key: ModelKey| {
        let _ = studio.update(Message::ModelKey(key));
    };

    let _ = studio.update(Message::ToggleFile);
    assert!(studio.file_open);
    key(&mut studio, ModelKey::Delete);
    assert_eq!(remaining(&studio), 4);
    assert!(studio.undo_deletions.is_empty());
    assert!(studio.clouds[0].selection.is_some());
    studio.zoom = 3.0;
    key(&mut studio, ModelKey::Fit);
    assert_eq!(studio.zoom, 3.0);

    // With the model in view the same keys act on it.
    let _ = studio.update(Message::ToggleFile);
    assert!(!studio.file_open);
    key(&mut studio, ModelKey::Fit);
    assert_eq!(studio.zoom, 1.0);
    key(&mut studio, ModelKey::Delete);
    assert_eq!(remaining(&studio), 3);
    assert_eq!(studio.undo_deletions.len(), 1);

    let _ = studio.update(Message::ToggleFile);
    key(&mut studio, ModelKey::Undo);
    assert_eq!(remaining(&studio), 3);
    // The buttons beside the File button stay in view and keep working, as
    // do the commands of the API, which send the same messages.
    let _ = studio.update(Message::UndoDelete);
    assert_eq!(remaining(&studio), 4);
    key(&mut studio, ModelKey::Redo);
    assert_eq!(remaining(&studio), 4);
    let _ = studio.update(Message::RedoDelete);
    assert_eq!(remaining(&studio), 3);

    let _ = studio.update(Message::ToggleFile);
    key(&mut studio, ModelKey::Undo);
    assert_eq!(remaining(&studio), 4);
    key(&mut studio, ModelKey::Redo);
    assert_eq!(remaining(&studio), 3);

    // The Settings dialog covers the model in the same way.
    let _ = studio.update(Message::Settings(
        crate::settings_dialog::SettingsAction::Open,
    ));
    assert!(studio.settings.is_some());
    key(&mut studio, ModelKey::Undo);
    assert_eq!(remaining(&studio), 3);
}

#[test]
fn a_file_handed_over_is_opened_in_view() {
    let mut studio = Studio::default();
    let _ = studio.update(Message::ToggleFile);
    let _ = studio.update(Message::FilePage(crate::FilePage::About));
    let _ = studio.update(Message::DroppedFilesReady);
    assert!(!studio.file_open);
    assert_eq!(studio.file_page, crate::FilePage::default());
}

#[test]
fn header_of_the_model_space_names_the_view_in_the_language_in_use() {
    let _language = TestLanguage::hold(Language::Table(0));
    let mut studio = Studio::default();
    assert_eq!(studio.view_caption(), "ISOMETRISCH");
    let _ = studio.update(Message::CameraPreset(CameraPreset::Top));
    // The state keeps the English label the command API reports.
    assert_eq!(studio.view_label, "TOP");
    assert_eq!(studio.view_caption(), "BOVEN");
    let _ = studio.update(Message::CubeEdge([1, 0, 1]));
    assert_eq!(studio.view_label, "TOP RIGHT");
    assert_eq!(studio.view_caption(), "BOVEN RECHTS");

    // Every label a view can get has a translation.
    let mut views: Vec<Message> = [
        CameraPreset::Top,
        CameraPreset::Bottom,
        CameraPreset::Front,
        CameraPreset::Back,
        CameraPreset::Right,
        CameraPreset::Left,
        CameraPreset::Isometric,
    ]
    .into_iter()
    .map(Message::CameraPreset)
    .collect();
    // An edge joins two faces: one axis is zero and the other two have a sign.
    for zero in 0..3 {
        for first in [-1, 1] {
            for second in [-1, 1] {
                let mut edge = [0i8; 3];
                edge[(zero + 1) % 3] = first;
                edge[(zero + 2) % 3] = second;
                views.push(Message::CubeEdge(edge));
            }
        }
    }
    views.extend([
        Message::CubeCorner([1, 1, 1]),
        Message::Orbit(4.0, 2.0),
        Message::ResetCamera,
    ]);
    assert_eq!(views.len(), 22);
    for view in views {
        let _ = studio.update(view.clone());
        if let Message::CubeEdge(edge) = view {
            assert_eq!(studio.view_label, view_cube::edge_label(edge));
            assert_ne!(studio.view_label, "CUSTOM", "{edge:?}");
        }
        assert_ne!(studio.view_caption(), studio.view_label, "{view:?}");
    }

    studio.box_select = true;
    assert_eq!(studio.view_caption(), "VAKSELECTIE ACTIEF");

    i18n::set(Language::English);
    assert_eq!(studio.view_caption(), "BOX SELECT ACTIVE");
    studio.box_select = false;
    assert_eq!(studio.view_caption(), "ISOMETRIC");
}

#[test]
fn api_sets_the_language_and_status_reports_it() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    let status = send(&mut studio, ApiCommand::Status);
    assert_eq!(status["result"]["language"], "en");

    let answer = set_language(&mut studio, "nl");
    assert_eq!(answer["ok"], true);
    assert_eq!(answer["language"], "nl");
    assert_eq!(i18n::choice(), Language::from_key("nl").unwrap());
    assert_eq!(i18n::tr("Settings"), "Instellingen");
    let status = send(&mut studio, ApiCommand::Status);
    assert_eq!(status["result"]["language"], "nl");
    // The window is built in the new language without a restart.
    let _ = studio.view();
    let _ = studio.update(Message::ToggleFile);
    let _ = studio.view();

    // The key is read without regard to case, as the theme is.
    assert_eq!(set_language(&mut studio, "EN")["language"], "en");
    assert_eq!(i18n::tr("Settings"), "Settings");
    assert_eq!(set_language(&mut studio, "auto")["language"], "auto");
    assert_eq!(i18n::choice(), Language::Auto);
}

#[test]
fn api_refuses_an_unknown_language_and_keeps_the_one_in_use() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    assert_eq!(set_language(&mut studio, "nl")["ok"], true);
    for unknown in ["xx", "", "dutch"] {
        let answer = set_language(&mut studio, unknown);
        assert_eq!(answer["ok"], false, "{unknown}");
        assert_eq!(answer["error"], "unknown language");
    }
    assert_eq!(i18n::choice().key(), "nl");
}

#[test]
fn status_reports_a_mesh_export_and_a_wait_counts_it_as_work() {
    let mut studio = Studio::default();
    let status = send(&mut studio, ApiCommand::Status);
    assert_eq!(status["result"]["mesh_export_pending"], false);
    assert!(crate::mcp::busy(&status["result"]).is_empty());

    // From the choice of the destination until the file is written.
    studio.mesh_export_pending = true;
    let status = send(&mut studio, ApiCommand::Status);
    assert_eq!(status["result"]["mesh_export_pending"], true);
    assert_eq!(crate::mcp::busy(&status["result"]), ["mesh_export"]);
    let _ = studio.update(Message::MeshExported(None, Err("disk full".into())));
    let status = send(&mut studio, ApiCommand::Status);
    assert!(crate::mcp::busy(&status["result"]).is_empty());
}

#[test]
fn the_window_is_drawn_in_the_tokens_of_each_theme() {
    let mut studio = Studio::default();
    let size = iced::Size::new(1440.0, 900.0);
    for theme in UiTheme::ALL {
        studio.ui_theme = theme;
        let colors = theme.colors();
        let picture = crate::test_render::render_window(&studio, size);
        // The scene: white in Light, Night Build in the dark themes.
        let scene = if theme == UiTheme::Light {
            iced::Color::WHITE
        } else {
            iced::Color::from_rgb8(0x2A, 0x2A, 0x32)
        };
        assert_eq!(colors.dom.scene, scene);
        assert!(
            picture.is(500, 750, scene),
            "{}: the scene is {:?}",
            theme.key(),
            picture.rgb(500, 750)
        );
        // Panel and title strip both follow the shared theme tokens. Sample
        // the strip away from its centered accent-coloured title.
        assert!(
            picture.is(100, 700, colors.bg),
            "{}: the panel is {:?}",
            theme.key(),
            picture.rgb(100, 700)
        );
        assert!(
            picture.is(900, 14, colors.bg_lighter),
            "{}: the strip is {:?}",
            theme.key(),
            picture.rgb(900, 14)
        );
    }
}

#[test]
fn api_chooses_a_theme_by_its_key_and_reports_that_key() {
    let mut studio = Studio::default();
    for theme in UiTheme::ALL {
        let answer = send(
            &mut studio,
            ApiCommand::SetTheme {
                theme: theme.key().to_uppercase(),
            },
        );
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["theme"], theme.key());
        assert_eq!(studio.ui_theme, theme);
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["theme"], theme.key());
    }
    let answer = send(
        &mut studio,
        ApiCommand::SetTheme {
            theme: "night".into(),
        },
    );
    assert_eq!(answer["theme"], "openaec");
    for unknown in ["dark", "deep forge", ""] {
        let answer = send(
            &mut studio,
            ApiCommand::SetTheme {
                theme: unknown.into(),
            },
        );
        assert_eq!(answer["ok"], false, "{unknown}");
    }
    assert_eq!(studio.ui_theme, UiTheme::OpenAec);
}

#[test]
fn api_command_is_read_from_its_json_form() {
    let command: ApiCommand =
        serde_json::from_str(r#"{"command": "set_language", "language": "nl"}"#).unwrap();
    assert!(matches!(command, ApiCommand::SetLanguage { language } if language == "nl"));
    assert!(serde_json::from_str::<ApiCommand>(r#"{"command": "set_language"}"#).is_err());
}
