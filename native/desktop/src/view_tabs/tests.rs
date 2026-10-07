use std::path::PathBuf;
use std::sync::Arc;

use iced::Size;
use pointcloud_core::{Drawing2d, DrawingUnits};
use serde_json::Value;

use super::*;
use crate::camera_views;
use crate::drawing_view::{DrawScene, DrawingSource, DrawingViewAction};
use crate::native_api::{ApiCommand, ApiRequest};
use crate::saved_drawings::SavedDrawing;
use crate::sheet_dialog::SheetKind;
use crate::views::ViewAction;
use crate::{CameraPreset, Message, Studio};

fn view(guid: &str) -> TabId {
    TabId::View(guid.into())
}

fn drawing(guid: &str) -> TabId {
    TabId::Drawing(guid.into())
}

#[test]
fn tabs_are_kept_by_their_kind_and_identifier() {
    for tab in [TabId::Model, view("a1"), drawing("b2")] {
        let kept = tab.key().unwrap();
        assert_eq!(TabId::from_key(&kept), Some(tab));
    }
    assert_eq!(
        TabId::File(Some(PathBuf::from("C:/plans/ground.dxf"))).key(),
        None
    );
    assert_eq!(TabId::File(None).key(), None, "the preview lasts a session");
    for damaged in [
        "",
        "view:",
        "drawing:",
        "sheet:a1",
        "Model",
        "file:C:/a.dxf",
    ] {
        assert_eq!(TabId::from_key(damaged), None, "{damaged}");
    }
    assert!(!TabId::Model.closable() && view("a").closable());
    assert!(TabId::Model.is_3d() && view("a").is_3d() && !drawing("a").is_3d());
}

#[test]
fn a_tab_opens_once_after_the_others_and_the_oldest_makes_room() {
    let mut tabs = ViewTabs::default();
    assert!(!tabs.add(TabId::Model), "the 3D model is always there");
    assert!(tabs.add(view("a")));
    assert!(tabs.add(drawing("b")));
    assert!(
        !tabs.add(view("a")),
        "an open tab is activated, not opened again"
    );
    assert_eq!(tabs.open(), [view("a"), drawing("b")]);
    assert!(tabs.remove(&view("a")));
    assert!(!tabs.remove(&view("a")));
    assert_eq!(tabs.open(), [drawing("b")]);
    for number in 0..MAX_TABS + 3 {
        tabs.add(view(&number.to_string()));
    }
    assert_eq!(tabs.open().len(), MAX_TABS);
    assert_eq!(tabs.open()[0], view("3"), "the oldest tabs closed");
    assert_eq!(tabs.open()[MAX_TABS - 1], view(&(MAX_TABS + 2).to_string()));
}

#[test]
fn the_kept_tabs_are_read_and_the_active_one_waits_to_be_shown() {
    let keys: Vec<String> = ["view:a", "model", "drawing:b", "nonsense", "view:a"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let tabs = ViewTabs::new(&keys, Some("drawing:c"));
    assert_eq!(tabs.open(), [view("a"), drawing("b"), drawing("c")]);
    assert_eq!(tabs.pending, Some(drawing("c")));
    assert_eq!(
        tabs.kept(),
        &(
            vec!["view:a".to_owned(), "drawing:b".into(), "drawing:c".into()],
            Some("drawing:c".to_owned())
        )
    );
    let model = ViewTabs::new(&keys, Some("model"));
    assert_eq!(model.pending, None, "the 3D model is shown at the start");
    let mut closed = ViewTabs::new(&keys, Some("view:a"));
    closed.remove(&view("a"));
    assert_eq!(closed.pending, None, "a closed tab is not shown again");
}

#[test]
fn a_closed_tab_gives_way_to_the_one_after_it_else_the_one_before() {
    let listed = [TabId::Model, view("a"), drawing("b"), drawing("c")];
    assert_eq!(after_closing(&listed, &view("a")), drawing("b"));
    assert_eq!(after_closing(&listed, &drawing("c")), drawing("b"));
    assert_eq!(after_closing(&listed[..2], &view("a")), TabId::Model);
    assert_eq!(after_closing(&listed, &view("gone")), TabId::Model);
}

#[test]
fn ctrl_tab_steps_round_through_the_tabs() {
    let listed = [TabId::Model, view("a"), drawing("b")];
    let step = |active: Option<&TabId>, forward| cycled(&listed, active, forward).unwrap();
    assert_eq!(step(Some(&TabId::Model), true), view("a"));
    assert_eq!(step(Some(&drawing("b")), true), TabId::Model);
    assert_eq!(step(Some(&TabId::Model), false), drawing("b"));
    assert_eq!(step(Some(&view("a")), false), TabId::Model);
    assert_eq!(step(None, true), TabId::Model);
    assert_eq!(step(None, false), drawing("b"));
    assert_eq!(
        cycled(&listed[..1], Some(&TabId::Model), true),
        Some(TabId::Model)
    );
    assert_eq!(cycled(&[], None, true), None);
}

#[test]
fn names_shrink_with_an_ellipsis_before_the_strip_scrolls() {
    assert_eq!(shortened("Plan +1.20", 32), "Plan +1.20");
    assert_eq!(shortened("Section through the stair", 9), "Section…");
    assert_eq!(shortened("abcdefghij", 5), "abcd…");
    let names: Vec<(String, bool)> = [
        ("3D model", false),
        ("Ground floor plan +1.20 m", true),
        ("Front", true),
    ]
    .into_iter()
    .map(|(name, closable)| (name.to_owned(), closable))
    .collect();
    let natural: f32 = names
        .iter()
        .map(|(name, closable)| tab_width(name.chars().count(), *closable))
        .sum();
    // Wide enough: every name as it is.
    let (shown, scrolls) = fitted_names(&names, natural);
    assert_eq!(shown, ["3D model", "Ground floor plan +1.20 m", "Front"]);
    assert!(!scrolls);
    // Narrower: the long name is shortened first, the short ones stay.
    let (shown, scrolls) = fitted_names(&names, natural - 60.0);
    assert!(!scrolls);
    assert_eq!(shown[0], "3D model");
    assert_eq!(shown[2], "Front");
    assert!(shown[1].ends_with('…') && shown[1].starts_with("Ground floor"));
    let widths: f32 = shown
        .iter()
        .zip(&names)
        .map(|(name, (_, closable))| tab_width(name.chars().count(), *closable))
        .sum();
    assert!(widths <= natural - 60.0, "{widths}");
    // Too narrow even so: the names stop at the shortest and the strip
    // scrolls.
    let (shown, scrolls) = fitted_names(&names, 150.0);
    assert!(scrolls);
    assert!(shown[1].chars().count() <= MIN_CHARS && shown[1].ends_with('…'));
    assert_eq!(shown[2], "Front");
    // A name is never shown wider than the 160 pixels of the style book,
    // however wide the bar.
    assert_eq!(MAX_CHARS, 25);
    assert!(MAX_CHARS as f32 * CHAR_WIDTH <= 160.0);
    let long = vec![("Ground floor plan at +1.20 m".to_owned(), true)];
    let (shown, _) = fitted_names(&long, 2000.0);
    assert_eq!(shown[0].chars().count(), MAX_CHARS);
    assert!(shown[0].ends_with('…'));
}

#[test]
fn copies_that_would_read_the_same_are_shortened_in_their_middle() {
    assert_eq!(shortened_in_the_middle("Entree (3)", 8), "Entr…(3)");
    assert_eq!(shortened_in_the_middle("Entree (12)", 8), "Ent…(12)");
    assert_eq!(shortened_in_the_middle("Begane grond +1.20", 8), "Bega….20");
    assert_eq!(shortened_in_the_middle("Entree", 8), "Entree");
    let names: Vec<(String, bool)> = [
        ("3D model", false),
        ("Entree", true),
        ("Entree (2)", true),
        ("Entree (3)", true),
        ("Doorsnede A-A", true),
    ]
    .into_iter()
    .map(|(name, closable)| (name.to_owned(), closable))
    .collect();
    let (shown, scrolls) = fitted_names(&names, 300.0);
    assert!(scrolls);
    assert_eq!(shown[2], "Entr…(2)");
    assert_eq!(shown[3], "Entr…(3)");
    // A name that reads as no other keeps its start.
    assert_eq!(shown[4], "Doorsne…");
    assert_eq!(shown[1], "Entree");
    let distinct: std::collections::HashSet<&String> = shown.iter().collect();
    assert_eq!(distinct.len(), shown.len(), "{shown:?}");
}

#[test]
fn the_strip_scrolls_to_the_start_of_a_whole_tab() {
    let widths = [80.0, 100.0, 120.0, 90.0];
    // The tab before the one shown shows whole at the left.
    assert_eq!(scroll_offset(&widths, 2, 300.0), 80.0);
    assert_eq!(scroll_offset(&widths, 3, 300.0), 180.0);
    // Without room for both, the tab shown starts the strip.
    assert_eq!(scroll_offset(&widths, 3, 200.0), 300.0);
    assert_eq!(scroll_offset(&widths, 0, 300.0), 0.0);
}

#[test]
fn a_tab_is_outlined_and_the_active_one_is_the_accent_on_the_soft_accent() {
    for theme in crate::ui_theme::UiTheme::ALL {
        let iced = theme.iced();
        let colors = theme.colors();
        let quiet = tab_style(&iced, false, false);
        assert_eq!(quiet.background, None);
        assert_eq!(quiet.text_color, colors.text_muted);
        assert_eq!(quiet.border.color, colors.border);
        assert_eq!(quiet.border.radius, iced::border::Radius::default().top(3));
        let over = tab_style(&iced, false, true);
        assert_eq!(over.background, Some(Background::Color(colors.hover)));
        assert_eq!(over.text_color, colors.text);
        for hovered in [false, true] {
            let active = tab_style(&iced, true, hovered);
            assert_eq!(
                active.background,
                Some(Background::Color(colors.accent_soft))
            );
            assert_eq!(active.text_color, colors.accent);
            assert_eq!(active.border.color, colors.accent);
        }
        // The × is muted, in the accent colour under the pointer.
        assert_eq!(
            close_style(&iced, button::Status::Active).text_color,
            colors.text_muted
        );
        let over = close_style(&iced, button::Status::Hovered);
        assert_eq!(over.text_color, colors.accent);
        assert_eq!(
            over.background,
            Some(Background::Color(colors.ribbon_btn_hover))
        );
    }
    // The × shows on the active tab and the one under the pointer only.
    assert!(shows_cross(true, false) && shows_cross(false, true) && shows_cross(true, true));
    assert!(!shows_cross(false, false));
}

/// A studio with one scan, its views and drawings in a folder of the test.
fn studio_with_scan() -> (Studio, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    camera_views::use_test_directory(&directory.path().join("config"));
    let mut studio = Studio::default();
    let path = directory.path().join("room.xyz");
    std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n2 1 1\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    (studio, directory)
}

/// A drawing of Create 2D of the scan of `studio`, kept and made, with a
/// frame of `size` metres.
fn made_drawing(studio: &mut Studio, name: &str, size: f64) -> String {
    let source = studio.browser.source_of(&studio.clouds[0].cloud.path);
    let request = pointcloud_core::DrawingRequest::for_view(pointcloud_core::DrawingView::Plan);
    let definition = SavedDrawing::new(
        name,
        SheetKind::Plan,
        pointcloud_core::OrientedBox::new(
            pointcloud_core::Bounds {
                min: [0.0; 3],
                max: [4.0, 3.0, 2.0],
            },
            0.0,
        ),
        &request,
        vec![source],
    );
    let guid = definition.guid.clone();
    let mut sheet = Drawing2d::new(DrawingUnits::Metres);
    sheet.add_frame([0.0, 0.0], [size, size * 0.75]).unwrap();
    studio.keep_saved_drawing(definition);
    studio
        .drawing_view
        .keep_made(Arc::new(DrawScene::from_drawing(
            &sheet,
            DrawingSource::Sheet {
                guid: guid.clone(),
                name: name.into(),
            },
        )));
    guid
}

fn act(studio: &mut Studio, action: TabAction) {
    let _ = studio.update(Message::Tabs(action));
}

fn send(studio: &mut Studio, command: Value) -> Value {
    let (reply, receive) = std::sync::mpsc::channel();
    let command: ApiCommand = serde_json::from_value(command).unwrap();
    let _ = studio.update(Message::ApiRequest(ApiRequest { command, reply }));
    receive.recv().unwrap()
}

fn names(studio: &Studio) -> Vec<String> {
    studio
        .listed_tabs()
        .iter()
        .map(|tab| studio.tab_name(tab))
        .collect()
}

#[test]
fn what_views_shows_opens_a_tab_and_a_click_on_a_tab_shows_it_again() {
    // The tabs are named in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    assert_eq!(studio.listed_tabs(), [TabId::Model]);
    assert_eq!(studio.shown_tab(), Some(TabId::Model));

    // A saved view and two drawings shown from their rows each open a tab,
    // in the order they were opened.
    let _ = studio.update(Message::Views(ViewAction::Save));
    let entrance = studio.listed_views()[0].guid.clone();
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    let roof = made_drawing(&mut studio, "Roof", 8.0);
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        plan.clone(),
    )));
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        roof.clone(),
    )));
    assert_eq!(
        names(&studio),
        ["3D model", "View 1", "Ground floor", "Roof"]
    );
    assert_eq!(studio.shown_tab(), Some(drawing(&roof)));
    assert_eq!(
        studio.shown_row(),
        Some(crate::project_browser::ViewRow::Drawing(roof.clone())),
        "the row of the active tab is highlighted"
    );

    // A row of an open tab activates it, without a second tab.
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        plan.clone(),
    )));
    assert_eq!(studio.listed_tabs().len(), 4);
    assert_eq!(studio.shown_tab(), Some(drawing(&plan)));

    // A click on a tab shows it.
    act(&mut studio, TabAction::Show(view(&entrance)));
    assert!(!studio.drawing_view.shown);
    assert_eq!(studio.shown_tab(), Some(view(&entrance)));
    act(&mut studio, TabAction::Show(TabId::Model));
    assert_eq!(studio.shown_tab(), Some(TabId::Model));
    assert!(studio.active_view_index().is_none());
    act(&mut studio, TabAction::Show(drawing(&roof)));
    assert_eq!(studio.drawing_view.shown_guid(), Some(roof.as_str()));

    // Ctrl+Tab steps on and round, Ctrl+Shift+Tab back.
    act(&mut studio, TabAction::Cycle(true));
    assert_eq!(studio.shown_tab(), Some(TabId::Model));
    act(&mut studio, TabAction::Cycle(false));
    assert_eq!(studio.shown_tab(), Some(drawing(&roof)));
    act(&mut studio, TabAction::Cycle(false));
    assert_eq!(studio.shown_tab(), Some(drawing(&plan)));
    // Not while the File view covers the tabs.
    let _ = studio.update(Message::ToggleFile);
    act(&mut studio, TabAction::Cycle(true));
    assert_eq!(studio.shown_tab(), Some(drawing(&plan)));
    let _ = studio.update(Message::ToggleFile);

    // The window builds with the tabs in a light and a dark theme, also
    // when they have to scroll.
    for theme in [
        crate::ui_theme::UiTheme::Light,
        crate::ui_theme::UiTheme::OpenAec,
    ] {
        studio.ui_theme = theme;
        let _ = studio.view();
    }
    studio.viewport_size = Size::new(1400.0, 800.0);
    let wide = studio.strip_layout();
    assert_eq!(wide.fitted, names(&studio), "every name as it is");
    // Too narrow: the names shrink and the strip scrolls to the tab shown.
    studio.viewport_size = Size::new(260.0, 300.0);
    let narrow = studio.strip_layout();
    assert!(narrow.fitted.iter().any(|name| name.ends_with('…')));
    assert!(studio.scroll_to_shown_tab().is_some());
    // The window says how wide the strip is, once it has opened.
    let _ = studio.update(Message::WindowResized(Size::new(1600.0, 900.0)));
    assert_eq!(studio.strip_layout().fitted, names(&studio));
    let _ = studio.view();
}

#[test]
fn the_document_bar_draws_the_active_tab_in_the_accent_and_the_hovered_one_tinted() {
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    let roof = made_drawing(&mut studio, "Roof", 8.0);
    for guid in [&plan, &roof] {
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            guid.clone(),
        )));
    }
    assert_eq!(studio.shown_tab(), Some(drawing(&roof)));
    let size = Size::new(800.0, 30.0);
    for theme in [
        crate::ui_theme::UiTheme::Light,
        crate::ui_theme::UiTheme::Forge,
    ] {
        studio.ui_theme = theme;
        studio.tabs.hovered = None;
        let colors = theme.colors();
        let picture = crate::test_render::render(studio.tab_strip(), &theme.iced(), size);
        // The bar: its line of the accent above, its border below, the
        // tabs in between.
        assert!(picture.is(790, 15, colors.docbar_bg), "{}", theme.key());
        let line = crate::ui_theme::DOCBAR_RULE;
        assert!(
            picture.is(790, 0, mixed(line, colors.docbar_bg, line.a)),
            "{}",
            theme.key()
        );
        assert!(
            picture.is(
                790,
                29,
                mixed(colors.border, colors.docbar_bg, colors.border.a)
            ),
            "{}",
            theme.key()
        );
        // The active tab is filled with the soft accent over the bar and
        // outlined in the accent colour, from 5 to the bottom.
        let fill = tab_fill(&colors, true, false);
        let filled: Vec<u32> = (0..800).filter(|&x| picture.is(x, 8, fill)).collect();
        assert!(filled.len() > 20, "{}: {filled:?}", theme.key());
        // The sides of a tab may fall between two pixels, which then share
        // the accent of its outline.
        let accent_near = |x: u32, y: u32| {
            let [r, g, b, _] = colors.accent.into_rgba8();
            let [red, green, blue] = picture.rgb(x, y);
            red.abs_diff(r) < 48 && green.abs_diff(g) < 48 && blue.abs_diff(b) < 48
        };
        let left = filled[0];
        assert!(
            accent_near(left - 1, 12) || accent_near(left - 2, 12),
            "{}: {:?}",
            theme.key(),
            (left - 3..=left)
                .map(|x| picture.rgb(x, 12))
                .collect::<Vec<_>>()
        );
        assert!(accent_near(filled[1], 5), "{}", theme.key());
        // Open at its bottom: its fill reaches the line of the bar.
        assert!(picture.is(filled[1], 28, fill), "{}", theme.key());
        // The tab under the pointer is tinted, and shows its ×.
        studio.tabs.hovered = Some(drawing(&plan));
        let over = crate::test_render::render(studio.tab_strip(), &theme.iced(), size);
        // (The tint may be the soft accent itself, as in Light.)
        let tint = tab_fill(&colors, false, true);
        let tinted = |picture: &crate::test_render::Picture| {
            (0..800).filter(|&x| picture.is(x, 8, tint)).count()
        };
        assert!(
            tinted(&over) > tinted(&picture) + 20,
            "{}: the hovered tab",
            theme.key()
        );
    }
    // The pointer leaving another tab leaves the hovered one as it is.
    let _ = studio.update(Message::Tabs(TabAction::Leave(drawing(&roof))));
    assert_eq!(studio.tabs.hovered, Some(drawing(&plan)));
    let _ = studio.update(Message::Tabs(TabAction::Leave(drawing(&plan))));
    assert_eq!(studio.tabs.hovered, None);
}

/// The tabs a tab of the strip says the pointer came over, from its left
/// to its right, with the first place where each says so.
fn tabs_under_the_pointer(studio: &Studio, size: Size) -> Vec<(TabId, f32)> {
    let mut found: Vec<(TabId, f32)> = Vec::new();
    for x in (0..size.width as u32).step_by(2) {
        let at = iced::Point::new(x as f32, 15.0);
        for message in crate::test_render::messages(studio.tab_strip(), size, at, &[]) {
            if let Message::Tabs(TabAction::Hover(tab)) = message {
                if found.last().map(|(last, _)| last) != Some(&tab) {
                    found.push((tab, x as f32));
                }
            }
        }
    }
    found
}

#[test]
fn the_tab_that_slides_under_the_pointer_when_the_one_there_closes_shows_its_cross() {
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    let roof = made_drawing(&mut studio, "Roof", 8.0);
    for guid in [&plan, &roof] {
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            guid.clone(),
        )));
    }
    let _ = studio.update(Message::Tabs(TabAction::Show(TabId::Model)));
    let size = Size::new(800.0, 30.0);
    // Every tab says when the pointer comes over it, in their order.
    let under = tabs_under_the_pointer(&studio, size);
    let order: Vec<&TabId> = under.iter().map(|(tab, _)| tab).collect();
    assert_eq!(order, [&TabId::Model, &drawing(&plan), &drawing(&roof)]);
    // The pointer on the plan, near its left: it is the tab under the
    // pointer, which keeps saying so no longer once the window knows.
    let at = iced::Point::new(under[1].1 + 4.0, 15.0);
    let mut window = crate::test_render::Session::default();
    let messages = window.send(studio.tab_strip(), size, at, &[]);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Tabs(TabAction::Hover(tab)) if *tab == drawing(&plan))),
        "{messages:?}"
    );
    for message in messages {
        let _ = studio.update(message);
    }
    assert_eq!(studio.tabs.hovered, Some(drawing(&plan)));
    let messages = window.send(studio.tab_strip(), size, at, &[]);
    assert!(
        !messages
            .iter()
            .any(|message| matches!(message, Message::Tabs(_))),
        "{messages:?}"
    );
    // Its × closes it: it is under the pointer no longer, and the roof,
    // which slides into its place under the pointer, says so at the next
    // frame, though the pointer did not move.
    let _ = studio.update(Message::Tabs(TabAction::Close(drawing(&plan))));
    assert_eq!(studio.tabs.hovered, None);
    let frame = iced::Event::Window(iced::window::Event::RedrawRequested(
        std::time::Instant::now(),
    ));
    let messages = window.send(studio.tab_strip(), size, at, &[frame]);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Tabs(TabAction::Hover(tab)) if *tab == drawing(&roof))),
        "{messages:?}"
    );
    for message in messages {
        let _ = studio.update(message);
    }
    assert_eq!(studio.tabs.hovered, Some(drawing(&roof)));
    // Opened again, the plan is not the tab under the pointer.
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        plan.clone(),
    )));
    let _ = studio.update(Message::Tabs(TabAction::Show(TabId::Model)));
    assert_ne!(studio.tabs.hovered, Some(drawing(&plan)));
    // The pointer gone from the strip, the roof says it left.
    let away = iced::Point::new(790.0, 15.0);
    let messages = window.send(studio.tab_strip(), size, away, &[]);
    assert!(
        messages
            .iter()
            .any(|message| matches!(message, Message::Tabs(TabAction::Leave(tab)) if *tab == drawing(&roof))),
        "{messages:?}"
    );
}

#[test]
fn a_tab_that_closes_is_under_the_pointer_no_longer() {
    let mut tabs = ViewTabs::default();
    tabs.add(drawing("a"));
    tabs.add(drawing("b"));
    tabs.hovered = Some(drawing("a"));
    assert!(tabs.remove(&drawing("b")));
    assert_eq!(tabs.hovered, Some(drawing("a")));
    assert!(tabs.remove(&drawing("a")));
    assert_eq!(tabs.hovered, None);
    // Nor does the oldest tab, closed when one too many opens.
    for index in 0..MAX_TABS {
        tabs.add(drawing(&format!("tab {index}")));
    }
    tabs.hovered = Some(drawing("tab 0"));
    tabs.add(drawing("one too many"));
    assert!(!tabs.open().contains(&drawing("tab 0")));
    assert_eq!(tabs.hovered, None);
}

#[test]
fn a_closed_tab_leaves_its_view_or_drawing_and_shows_its_neighbour() {
    // The tabs are named in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let _ = studio.update(Message::Views(ViewAction::Save));
    let entrance = studio.listed_views()[0].guid.clone();
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    let roof = made_drawing(&mut studio, "Roof", 8.0);
    for guid in [&plan, &roof] {
        let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
            guid.clone(),
        )));
    }
    act(&mut studio, TabAction::Show(drawing(&plan)));

    // The active tab closes: the one after it is shown.
    act(&mut studio, TabAction::Close(drawing(&plan)));
    assert_eq!(names(&studio), ["3D model", "View 1", "Roof"]);
    assert_eq!(studio.shown_tab(), Some(drawing(&roof)));
    assert!(
        studio
            .drawing_view
            .saved
            .iter()
            .any(|kept| kept.guid == plan),
        "closing never deletes the drawing"
    );
    assert!(studio.drawing_view.made(&plan).is_some());
    // The last one closes: the one before it is shown.
    act(&mut studio, TabAction::Close(drawing(&roof)));
    assert_eq!(studio.shown_tab(), Some(view(&entrance)));
    assert!(!studio.drawing_view.shown);
    // A tab that is not shown closes without changing what is shown.
    act(&mut studio, TabAction::Show(drawing(&roof)));
    act(&mut studio, TabAction::Close(view(&entrance)));
    assert_eq!(names(&studio), ["3D model", "Roof"]);
    assert_eq!(studio.shown_tab(), Some(drawing(&roof)));
    assert_eq!(studio.listed_views().len(), 1, "the view stays");
    // The 3D model never closes.
    act(&mut studio, TabAction::Close(TabId::Model));
    assert_eq!(names(&studio), ["3D model", "Roof"]);

    // A deleted drawing takes its tab along.
    let _ = studio.update(Message::DrawingView(DrawingViewAction::DeleteDrawing(
        roof.clone(),
    )));
    assert_eq!(names(&studio), ["3D model"]);
    assert_eq!(studio.shown_tab(), Some(TabId::Model));
}

#[test]
fn the_3d_model_keeps_its_camera_box_and_colours_and_a_drawing_its_zoom() {
    // The tabs are named in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    // A view saved from the default camera, without a section box.
    let _ = studio.update(Message::Views(ViewAction::Save));
    let entrance = studio.listed_views()[0].guid.clone();
    let saved = (studio.yaw, studio.pitch);
    act(&mut studio, TabAction::Show(TabId::Model));

    // The 3D model looks from above with a section box and other colours.
    let _ = studio.update(Message::CameraPreset(CameraPreset::Top));
    let _ = studio.update(Message::SetSectionEnabled(true));
    let _ = studio.update(Message::SectionMax(2, 60.0));
    let _ = studio.update(Message::ColorMode(crate::ColorMode::Elevation));
    let model = (studio.yaw, studio.pitch, studio.zoom, studio.pan);
    let label = studio.view_label;
    let section = studio.section_box();
    assert!(section.is_some());

    // The view shows itself as it was saved.
    let _ = studio.update(Message::Views(ViewAction::Restore(entrance.clone())));
    assert_eq!((studio.yaw, studio.pitch), saved);
    assert!(!studio.section_enabled);
    // A drawing in between leaves the scene as it is.
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    act(&mut studio, TabAction::Show(drawing(&plan)));
    // Orbited in the view, the view keeps the orbit while the scene is its.
    act(&mut studio, TabAction::Show(view(&entrance)));
    let _ = studio.update(Message::Orbit(12.0, 0.0));
    let orbited = studio.yaw;
    assert_ne!(orbited, saved.0);
    act(&mut studio, TabAction::Show(drawing(&plan)));
    act(&mut studio, TabAction::Show(view(&entrance)));
    assert_eq!(studio.yaw, orbited, "the view comes back as it was left");

    // The tab of the 3D model gives it back its own camera, box and colours.
    act(&mut studio, TabAction::Show(TabId::Model));
    assert_eq!((studio.yaw, studio.pitch, studio.zoom, studio.pan), model);
    assert_eq!(studio.view_label, label);
    assert_eq!(studio.section_box(), section);
    assert_eq!(studio.color_mode, crate::ColorMode::Elevation);
    assert!(studio.active_view_index().is_none());
    // Its row does the same as its tab.
    act(&mut studio, TabAction::Show(view(&entrance)));
    let _ = studio.update(Message::Browser(
        crate::project_browser::BrowserAction::ShowModel,
    ));
    assert_eq!((studio.yaw, studio.pitch, studio.zoom, studio.pan), model);

    // A drawing keeps where it was zoomed to while another is shown.
    let roof = made_drawing(&mut studio, "Roof", 8.0);
    act(&mut studio, TabAction::Show(drawing(&plan)));
    studio.drawing_view.zoom_extents(Size::new(800.0, 600.0));
    let _ = studio.update(Message::DrawingView(DrawingViewAction::Zoom(
        2.0,
        [100.0, 80.0],
        Size::new(800.0, 600.0),
    )));
    let _ = studio.update(Message::DrawingView(DrawingViewAction::Layer(0, false)));
    let zoomed = studio.drawing_view.camera();
    act(&mut studio, TabAction::Show(drawing(&roof)));
    studio.drawing_view.zoom_extents(Size::new(800.0, 600.0));
    assert_ne!(studio.drawing_view.camera(), zoomed);
    act(&mut studio, TabAction::Show(TabId::Model));
    act(&mut studio, TabAction::Show(drawing(&plan)));
    assert_eq!(studio.drawing_view.camera(), zoomed);
    assert!(!studio.drawing_view.layer_shown(0), "and its layers");
    // Closed and opened again, it starts from its extents.
    act(&mut studio, TabAction::Close(drawing(&plan)));
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        plan.clone(),
    )));
    assert!(studio.drawing_view.layer_shown(0));
}

#[test]
fn a_deleted_view_that_had_the_scene_gives_the_3d_model_its_camera_back() {
    // The tabs are named in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let _ = studio.update(Message::Views(ViewAction::Save));
    let entrance = studio.listed_views()[0].guid.clone();
    act(&mut studio, TabAction::Show(TabId::Model));
    let _ = studio.update(Message::CameraPreset(CameraPreset::Top));
    let model = (studio.yaw, studio.pitch, studio.zoom, studio.pan);
    act(&mut studio, TabAction::Show(view(&entrance)));
    let _ = studio.update(Message::Orbit(12.0, 0.0));
    assert_ne!((studio.yaw, studio.pitch, studio.zoom, studio.pan), model);

    // The × of its row under VIEWS deletes it: its tab goes, and the 3D
    // model shows with its own camera, as when its tab is closed.
    let _ = studio.update(Message::Views(ViewAction::Delete(entrance.clone())));
    assert_eq!(names(&studio), ["3D model"]);
    assert_eq!(studio.shown_tab(), Some(TabId::Model));
    assert_eq!((studio.yaw, studio.pitch, studio.zoom, studio.pan), model);
    // And keeps it: it is the camera of the 3D model again.
    act(&mut studio, TabAction::Show(TabId::Model));
    assert_eq!((studio.yaw, studio.pitch, studio.zoom, studio.pan), model);

    // A deleted view that no longer had the scene leaves the camera alone.
    let _ = studio.update(Message::Views(ViewAction::Save));
    let hall = studio.listed_views()[0].guid.clone();
    act(&mut studio, TabAction::Show(TabId::Model));
    let _ = studio.update(Message::CameraPreset(CameraPreset::Front));
    let front = (studio.yaw, studio.pitch, studio.zoom, studio.pan);
    let _ = studio.update(Message::Views(ViewAction::Delete(hall)));
    assert_eq!((studio.yaw, studio.pitch, studio.zoom, studio.pan), front);
}

#[test]
fn ctrl_tab_and_show_tab_leave_what_lies_under_a_dialog_or_the_wizard_card() {
    // The tabs are named in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        plan.clone(),
    )));
    assert_eq!(studio.shown_tab(), Some(drawing(&plan)));

    // The dialog of Create 2D.
    let _ = studio.update(Message::Sheet(crate::sheet_dialog::SheetAction::Open));
    assert!(studio.sheet_dialog.is_some());
    act(&mut studio, TabAction::Cycle(true));
    assert_eq!(studio.shown_tab(), Some(drawing(&plan)));
    let refused = send(
        &mut studio,
        serde_json::json!({"command": "show_tab", "index": 0}),
    );
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(studio.shown_tab(), Some(drawing(&plan)));
    let _ = studio.update(Message::Escape);
    assert!(studio.sheet_dialog.is_none());

    // The card of the Pointcloud to Drawing wizard.
    let _ = studio.update(Message::MeshToPlans(
        crate::mesh_to_plans::WizardAction::Open,
    ));
    assert!(studio.mesh_to_plans.covers_model());
    act(&mut studio, TabAction::Cycle(false));
    assert_eq!(studio.shown_tab(), Some(drawing(&plan)));
    let refused = send(
        &mut studio,
        serde_json::json!({"command": "show_tab", "index": 0}),
    );
    assert_eq!(refused["ok"], false, "{refused}");
    // As a strip beside the scene it covers nothing.
    let _ = studio.update(Message::Escape);
    assert!(!studio.mesh_to_plans.covers_model());
    act(&mut studio, TabAction::Cycle(true));
    assert_eq!(studio.shown_tab(), Some(TabId::Model));
}

#[test]
fn the_local_api_names_the_3d_model_alike_in_every_language() {
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::Table(0));
    let (mut studio, _directory) = studio_with_scan();
    // The strip speaks the language of the window...
    assert_eq!(studio.tab_name(&TabId::Model), "3D-model");
    // ...the local API does not, as for the Project Browser it reports.
    let listed = send(&mut studio, serde_json::json!({"command": "list_tabs"}));
    assert_eq!(listed["tabs"][0]["name"], "3D model", "{listed}");
    let status = send(&mut studio, serde_json::json!({"command": "status"}));
    assert_eq!(status["result"]["view_tabs"]["tabs"][0]["name"], "3D model");
    // Either name finds it.
    for name in ["3D model", "3d-MODEL"] {
        let shown = send(
            &mut studio,
            serde_json::json!({"command": "show_tab", "name": name}),
        );
        assert_eq!(shown["ok"], true, "{shown}");
        assert_eq!(shown["shown"], "3D model");
    }
}

#[test]
fn the_tabs_are_kept_and_the_active_one_comes_back_once_its_scan_is_read() {
    // The tabs are named in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, directory) = studio_with_scan();
    let _ = studio.update(Message::Views(ViewAction::Save));
    let entrance = studio.listed_views()[0].guid.clone();
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        plan.clone(),
    )));
    act(&mut studio, TabAction::Show(view(&entrance)));
    // A file opened this session is a tab for the session only.
    let file = directory.path().join("site.dxf");
    studio
        .drawing_view
        .set_scene(Arc::new(DrawScene::from_drawing(
            &Drawing2d::new(DrawingUnits::Metres),
            DrawingSource::File(file.clone()),
        )));
    studio.drawing_view.shown = true;
    let _ = studio.update(Message::Tabs(TabAction::Show(TabId::File(Some(file)))));
    act(&mut studio, TabAction::Show(view(&entrance)));
    assert_eq!(
        names(&studio),
        ["3D model", "View 1", "Ground floor", "site.dxf"]
    );
    let kept = studio.preferences();
    assert_eq!(
        kept.view_tabs,
        [format!("view:{entrance}"), format!("drawing:{plan}")]
    );
    assert_eq!(kept.view_tab, Some(format!("view:{entrance}")));

    // The next session opens the same tabs, without the file, and shows the
    // view again once the scan is read.
    let restarted = || Studio {
        tabs: ViewTabs::new(&kept.view_tabs, kept.view_tab.as_deref()),
        ..Studio::default()
    };
    let mut next = restarted();
    assert_eq!(
        next.listed_tabs(),
        [TabId::Model],
        "they wait for their scan"
    );
    let cloud = Arc::new(pointcloud_core::open(&studio.clouds[0].cloud.path, 10).unwrap());
    let _ = next.update(Message::Loaded(Ok(cloud)));
    assert_eq!(names(&next), ["3D model", "View 1", "Ground floor"]);
    assert_eq!(next.shown_tab(), Some(view(&entrance)));

    // Chosen otherwise first, the kept tab stays where it is.
    let mut chosen = restarted();
    let _ = chosen.update(Message::Tabs(TabAction::Show(TabId::Model)));
    let cloud = Arc::new(pointcloud_core::open(&studio.clouds[0].cloud.path, 10).unwrap());
    let _ = chosen.update(Message::Loaded(Ok(cloud)));
    assert_eq!(chosen.shown_tab(), Some(TabId::Model));
}

#[test]
fn the_local_api_lists_shows_and_closes_tabs() {
    // The tabs are named in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let _ = studio.update(Message::Views(ViewAction::Save));
    let plan = made_drawing(&mut studio, "Ground floor", 4.0);
    let _ = studio.update(Message::DrawingView(DrawingViewAction::ShowDrawing(
        plan.clone(),
    )));

    let listed = send(&mut studio, serde_json::json!({"command": "list_tabs"}));
    assert_eq!(listed["ok"], true, "{listed}");
    assert_eq!(listed["active"], 2);
    assert_eq!(listed["tabs"][0]["name"], "3D model");
    assert_eq!(listed["tabs"][0]["kind"], "model");
    assert_eq!(listed["tabs"][0]["closable"], false);
    assert_eq!(listed["tabs"][1]["kind"], "view");
    assert_eq!(listed["tabs"][2]["guid"], plan.as_str());
    assert_eq!(listed["tabs"][2]["active"], true);
    let status = send(&mut studio, serde_json::json!({"command": "status"}));
    assert_eq!(status["result"]["view_tabs"]["active"], 2);

    let shown = send(
        &mut studio,
        serde_json::json!({"command": "show_tab", "name": "view 1"}),
    );
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(shown["shown"], "View 1");
    assert_eq!(shown["active"], 1);
    let shown = send(
        &mut studio,
        serde_json::json!({"command": "show_tab", "index": 0}),
    );
    assert_eq!(shown["active"], 0);

    let closed = send(
        &mut studio,
        serde_json::json!({"command": "close_tab", "name": "GROUND FLOOR"}),
    );
    assert_eq!(closed["ok"], true, "{closed}");
    assert_eq!(closed["closed"], "Ground floor");
    assert_eq!(closed["tabs"].as_array().unwrap().len(), 2);
    for refused in [
        serde_json::json!({"command": "close_tab", "index": 0}),
        serde_json::json!({"command": "close_tab", "name": "Ground floor"}),
        serde_json::json!({"command": "show_tab", "index": 9}),
        serde_json::json!({"command": "show_tab"}),
    ] {
        let answer = send(&mut studio, refused.clone());
        assert_eq!(answer["ok"], false, "{refused} {answer}");
    }
}
