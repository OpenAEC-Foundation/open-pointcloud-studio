//! Tests of the sheets: the paper and the scale, where views are placed,
//! how sheets are kept, what a sheet shows and the PDF it is written as.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use iced::advanced::layout::{self, Layout as WidgetLayout};
use iced::advanced::renderer;
use iced::{mouse, Color, Rectangle, Size};
use pointcloud_core::{Bounds, Drawing2d, DrawingRequest, DrawingUnits, DrawingView, OrientedBox};
use serde_json::{json, Value};

use super::model::{self, Layout, Paper, PlacedKind};
use super::plot::{self, Content, Mark};
use super::*;
use crate::camera_views;
use crate::drawing_view::{DrawScene, DrawingSource};
use crate::native_api::{ApiCommand, ApiRequest};
use crate::saved_drawings::SavedDrawing;
use crate::sheet_dialog::SheetKind;
use crate::view_tabs::TabId;

/// A studio with one small scan open and a folder of its own for what it
/// keeps.
fn studio_with_scan() -> (Studio, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    camera_views::use_test_directory(&directory.path().join("config"));
    let path = directory.path().join("office.xyz");
    std::fs::write(&path, "0 0 0\n12 0 0\n12 8 0\n0 8 3\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    (studio, directory)
}

fn send(studio: &mut Studio, body: Value) -> Value {
    let command: ApiCommand = serde_json::from_value(body).unwrap();
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.handle_api(ApiRequest { command, reply });
    receive.recv().unwrap()
}

/// A plan of the box from (0, 0) to (12, 8) with an outline around it, as
/// made in this session, kept under VIEWS with the scan of the studio.
fn made_plan(studio: &mut Studio, name: &str) -> String {
    let sources = studio.open_sources();
    let mut request = DrawingRequest::for_view(DrawingView::Plan);
    request.units = DrawingUnits::Millimetres;
    let definition = SavedDrawing::new(
        name,
        SheetKind::Plan,
        OrientedBox::new(
            Bounds {
                min: [0.0, 0.0, 0.0],
                max: [12.0, 8.0, 3.0],
            },
            0.0,
        ),
        &request,
        sources,
    );
    let guid = definition.guid.clone();
    let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
    let outline = drawing.layer("OPS-CUT-OUTLINE", [255, 255, 255]).unwrap();
    drawing.add_polyline(
        outline,
        vec![[0.5, 0.5], [11.5, 0.5], [11.5, 7.5], [0.5, 7.5]],
        true,
    );
    drawing.add_fill(
        outline,
        vec![[0.5, 0.5], [0.8, 0.5], [0.8, 7.5], [0.5, 7.5]],
        Vec::new(),
    );
    let points = drawing.layer("OPS-POINTS", [200, 30, 40]).unwrap();
    drawing.add_point(points, [6.0, 4.0], None);
    studio.keep_saved_drawing(definition);
    let scene = DrawScene::from_drawing(
        &drawing,
        DrawingSource::Sheet {
            guid: guid.clone(),
            name: name.to_owned(),
        },
    );
    studio.drawing_view.keep_made(Arc::new(scene));
    guid
}

/// A small PNG of `width` by `height` pixels.
fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, _| {
        image::Rgb([(x * 7 % 255) as u8, 120, 200])
    });
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgb8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    bytes
}

/// Lay out and draw the window with the software renderer.
fn draw_window(studio: &Studio, size: Size) {
    use iced::advanced::widget::Tree;
    let mut renderer = iced::Renderer::Secondary(iced_tiny_skia::Renderer::new(
        iced::Font::DEFAULT,
        iced::Pixels(12.0),
    ));
    let element = studio.view();
    let mut tree = Tree::new(&element);
    let node =
        element
            .as_widget()
            .layout(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    element.as_widget().draw(
        &tree,
        &mut renderer,
        &studio.ui_theme.iced(),
        &renderer::Style {
            text_color: Color::BLACK,
        },
        WidgetLayout::new(&node),
        mouse::Cursor::Unavailable,
        &Rectangle::with_size(size),
    );
}

/// The streams of a PDF, inflated.
fn pdf_streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut streams = Vec::new();
    let mut rest = pdf;
    while let Some(start) = find(rest, b"stream\n") {
        let body = &rest[start + 7..];
        let Some(end) = find(body, b"\nendstream") else {
            break;
        };
        let mut inflated = Vec::new();
        if flate2::read::ZlibDecoder::new(&body[..end])
            .read_to_end(&mut inflated)
            .is_ok()
        {
            streams.push(inflated);
        }
        rest = &body[end + b"\nendstream".len()..];
    }
    streams
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn papers_of_the_a_series_lie_and_stand() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    assert_eq!(Paper::A4.size(false), [210.0, 297.0]);
    assert_eq!(Paper::A3.size(true), [420.0, 297.0]);
    assert_eq!(Paper::A2.size(true), [594.0, 420.0]);
    assert_eq!(Paper::A1.size(false), [594.0, 841.0]);
    assert_eq!(Paper::A0.size(true), [1189.0, 841.0]);
    // Every paper is the one before it halved.
    for pair in Paper::ALL.windows(2) {
        let [small, large] = [pair[0].portrait(), pair[1].portrait()];
        assert!((large[0] - small[1]).abs() <= 1.0, "{pair:?}");
    }
    assert_eq!(Paper::from_key("A3"), Some(Paper::A3));
    assert_eq!(Paper::from_key("b4"), None);
    assert_eq!(Paper::A2.to_string(), "A2");
    let sheet = Layout::new("01", "Plans", Paper::A3, true);
    assert_eq!(sheet.border(), [[10.0, 10.0], [410.0, 287.0]]);
    // The title block lies in the lower right corner inside the border.
    assert_eq!(sheet.title_block(), [[230.0, 10.0], [410.0, 42.0]]);
    let upright = Layout::new("02", "Detail", Paper::A4, false);
    let [min, _] = upright.title_block();
    assert!(min[0] >= 10.0, "the title block fits on A4 standing");
}

#[test]
fn a_metre_is_ten_millimetres_at_one_to_a_hundred() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    assert_eq!(model::paper_mm(1.0, 100.0), 10.0);
    assert_eq!(model::paper_mm(1.0, 50.0), 20.0);
    assert_eq!(model::paper_mm(2.5, 200.0), 12.5);
    assert_eq!(
        model::drawing_size([[0.0, 0.0], [12.0, 8.0]], 100.0),
        [120.0, 80.0]
    );
    assert_eq!(model::scale_label(100.0), "1:100");
    assert_eq!(model::scale_label(75.5), "1:75.5");
    assert_eq!(model::parse_scale("1:50"), Some(50.0));
    assert_eq!(model::parse_scale(" 1/200 "), Some(200.0));
    assert_eq!(model::parse_scale("75"), Some(75.0));
    assert_eq!(model::parse_scale("12,5"), Some(12.5));
    assert_eq!(model::parse_scale("0"), None);
    assert_eq!(model::parse_scale("1:abc"), None);
    // A picture of 1600 pixels is 203.2 mm wide at 200 dpi, and made
    // smaller with its proportions to fit.
    let [width, height] = model::image_size([1600, 1000], [400.0, 400.0]);
    assert!((width - 203.2).abs() < 1e-9 && (height - 127.0).abs() < 1e-9);
    let fitted = model::image_size([1600, 1000], [100.0, 100.0]);
    assert!((fitted[0] - 100.0).abs() < 1e-9 && (fitted[1] - 62.5).abs() < 1e-9);
    assert!((model::dots_per_inch([1600, 1000], [203.2, 127.0]) - 200.0).abs() < 1e-6);
    assert_eq!(model::png_size(&png(64, 40)), Some([64, 40]));
    assert_eq!(model::png_size(b"not a picture"), None);
}

#[test]
fn views_are_placed_where_the_paper_is_free() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let mut sheet = Layout::new("01", "Plans", Paper::A3, true);
    let first = sheet.free_place([120.0, 80.0], None);
    let place = |at: [f64; 2], size: [f64; 2]| model::rect_around(at, size);
    // At the upper left inside the border.
    let rect = place(first, [120.0, 80.0]);
    assert!(rect[0][0] >= 10.0 && rect[1][1] <= 287.0, "{rect:?}");
    sheet.viewports.push(model::Viewport::new(
        PlacedKind::Drawing,
        &camera_views::new_guid(),
        "Plan",
        first,
        [120.0, 80.0],
    ));
    let second = sheet.free_place([150.0, 100.0], None);
    let other = place(second, [150.0, 100.0]);
    assert!(!model::overlap(rect, other), "{rect:?} {other:?}");
    assert!(!model::overlap(sheet.title_block(), other));
    // Something larger than the paper goes to its middle.
    let middle = sheet.free_place([500.0, 400.0], None);
    assert_eq!(middle[0], 210.0);
}

#[test]
fn sheets_are_kept_and_what_cannot_be_read_is_left_out() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config/sheets.json");
    let mut sheet = Layout::new("A-101", "Ground floor", Paper::A2, false);
    sheet.project = "Office".into();
    let mut viewport = model::Viewport::new(
        PlacedKind::View,
        &camera_views::new_guid(),
        "Entrance",
        [100.0, 200.0],
        [80.0, 50.0],
    );
    viewport.title = Some("Entrance seen from the street".into());
    sheet.viewports.push(viewport);
    camera_views::write_json(&path, &vec![sheet.clone()]).unwrap();
    assert_eq!(model::load_from(&path), vec![sheet.clone()]);

    // A sheet without a name, and a viewport with a scale of zero, are
    // dropped; the rest stays.
    let mut stored = serde_json::to_value(vec![sheet.clone(), sheet.clone()]).unwrap();
    stored[1]["guid"] = json!(camera_views::new_guid());
    stored[1]["name"] = json!("");
    stored[0]["viewports"][0]["scale"] = json!(0.0);
    std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
    let loaded = model::load_from(&path);
    assert_eq!(loaded.len(), 1);
    assert!(loaded[0].viewports.is_empty());
    std::fs::write(&path, b"[{").unwrap();
    assert!(model::load_from(&path).is_empty());
    assert!(path.with_extension("unreadable.json").is_file());
}

#[test]
fn a_sheet_with_a_plan_and_a_view_comes_back_after_a_restart() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, directory) = studio_with_scan();
    let plan = made_plan(&mut studio, "Plan +1.20");
    let _ = studio.update(Message::Views(crate::views::ViewAction::Save));
    let view = studio.listed_views()[0].guid.clone();
    camera_views::write_snapshot(&view, &png(160, 100)).unwrap();

    let _ = studio.update(Message::Layouts(LayoutAction::NewSheet));
    let form = studio.layouts.form.clone().expect("the form opened");
    assert_eq!(
        (form.number.as_str(), form.name.as_str()),
        ("01", "Sheet 1")
    );
    assert_eq!(
        (form.paper, form.orientation),
        (Paper::A3, Orientation::Landscape)
    );
    let _ = studio.update(Message::Layouts(LayoutAction::Create));
    let sheet = studio.layouts.list[0].guid.clone();
    assert_eq!(studio.drawing_view.shown_layout(), Some(sheet.as_str()));
    assert_eq!(studio.shown_tab(), Some(TabId::Layout(sheet.clone())));

    let id = studio
        .place_on_layout(
            &sheet,
            PlacedKind::Drawing,
            &plan,
            Some([150.0, 180.0]),
            None,
        )
        .unwrap();
    let placed = studio.layouts.list[0].viewport(&id).unwrap().clone();
    assert_eq!(placed.size, [120.0, 80.0], "12 by 8 m at 1:100");
    assert_eq!(placed.scale, 100.0);
    studio
        .place_on_layout(&sheet, PlacedKind::View, &view, None, None)
        .unwrap();
    let _ = studio.update(Message::Layouts(LayoutAction::Select(Some(id.clone()))));
    let _ = studio.update(Message::Layouts(LayoutAction::ScaleChosen(ScaleChoice(
        200.0,
    ))));
    assert_eq!(
        studio.layouts.list[0].viewport(&id).unwrap().size,
        [60.0, 40.0]
    );
    let _ = studio.update(Message::Layouts(LayoutAction::Move(
        id.clone(),
        [100.0, 120.0],
    )));
    assert_eq!(
        studio.layouts.list[0].viewport(&id).unwrap().centre,
        [100.0, 120.0]
    );
    let _ = studio.update(Message::Layouts(LayoutAction::Project(
        "Office at the canal".into(),
    )));
    draw_window(&studio, Size::new(1440.0, 900.0));
    assert!(studio.layouts.bounds.get().is_some(), "the paper was drawn");
    assert_eq!(
        studio.layouts.list[0].scale_text().as_deref(),
        Some("1:200")
    );

    // A new window reads the sheet back, with its tab.
    let preferences = studio.preferences();
    assert!(preferences.view_tabs.contains(&format!("layout:{sheet}")));
    let restarted = Studio::default();
    assert_eq!(restarted.layouts.list, studio.layouts.list);
    let tabs =
        crate::view_tabs::ViewTabs::new(&preferences.view_tabs, preferences.view_tab.as_deref());
    assert!(tabs.open().contains(&TabId::Layout(sheet)));
    drop(directory);
}

#[test]
fn a_deleted_view_leaves_view_missing_and_a_viewport_follows_its_view() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let english = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let plan = made_plan(&mut studio, "Plan +1.20");
    let _ = studio.update(Message::Views(crate::views::ViewAction::Save));
    let view = studio.listed_views()[0].guid.clone();
    let sheet = studio
        .create_layout("01", "Sheet 1", Paper::A3, true)
        .unwrap();
    let _ = studio.show_layout(&sheet);
    studio
        .place_on_layout(&sheet, PlacedKind::Drawing, &plan, None, None)
        .unwrap();
    studio
        .place_on_layout(&sheet, PlacedKind::View, &view, None, None)
        .unwrap();
    let texts = |studio: &Studio| -> Vec<String> {
        studio
            .shown_plot()
            .unwrap()
            .texts()
            .map(str::to_owned)
            .collect()
    };
    let shown = texts(&studio);
    assert!(shown.iter().any(|text| text == "Plan +1.20"), "{shown:?}");
    assert!(shown.iter().any(|text| text == "1:100"), "{shown:?}");
    assert!(shown.iter().any(|text| text == "View 1"));

    // A view renamed is renamed on the sheet.
    let _ = studio.update(Message::Views(crate::views::ViewAction::StartRename(
        view.clone(),
    )));
    let _ = studio.update(Message::Views(crate::views::ViewAction::RenameText(
        "Entrance".into(),
    )));
    let _ = studio.update(Message::Views(crate::views::ViewAction::FinishRename));
    assert!(texts(&studio).iter().any(|text| text == "Entrance"));

    // Deleted, the view and the drawing leave a frame that says so.
    let _ = studio.update(Message::Views(crate::views::ViewAction::Delete(view)));
    let _ = studio.update(Message::DrawingView(
        crate::drawing_view::DrawingViewAction::DeleteDrawing(plan),
    ));
    let shown = texts(&studio);
    assert_eq!(
        shown.iter().filter(|text| *text == "view missing").count(),
        2,
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|text| text == "Entrance"),
        "the last name stays"
    );
    let listed = send(&mut studio, json!({"command": "list_sheets"}));
    let viewports = &listed["sheets"][0]["viewports"];
    assert_eq!(viewports[0]["shows"], "missing");
    assert_eq!(viewports[1]["shows"], "missing");
    {
        drop(english);
        let _dutch = crate::i18n::TestLanguage::hold(crate::i18n::Language::Table(0));
        let _ = studio.update(Message::Layouts(LayoutAction::Fit));
        assert!(texts(&studio).iter().any(|text| text == "view ontbreekt"));
        draw_window(&studio, Size::new(1440.0, 900.0));
    }
}

#[test]
fn the_pdf_holds_the_paper_the_drawing_the_picture_and_the_texts() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let plan = made_plan(&mut studio, "Plan +1.20");
    let _ = studio.update(Message::Views(crate::views::ViewAction::Save));
    let view = studio.listed_views()[0].guid.clone();
    camera_views::write_snapshot(&view, &png(64, 40)).unwrap();
    let sheet = studio
        .create_layout("A-01", "Ground floor", Paper::A3, true)
        .unwrap();
    let _ = studio.show_layout(&sheet);
    studio
        .place_on_layout(
            &sheet,
            PlacedKind::Drawing,
            &plan,
            Some([140.0, 170.0]),
            None,
        )
        .unwrap();
    studio
        .place_on_layout(&sheet, PlacedKind::View, &view, None, None)
        .unwrap();
    let _ = studio.settle_layouts();
    let plot = studio.shown_plot().unwrap();
    assert!(plot
        .marks()
        .any(|(mark, _)| matches!(mark, Mark::Image { .. })));
    // The outline of the plan lies 5 mm inside the viewport at 1:100.
    let outline = plot.marks().find_map(|(mark, clip)| match mark {
        Mark::Line {
            points,
            closed: true,
            ..
        } if points.len() == 4 && clip.is_some() => Some((points.clone(), clip.unwrap())),
        _ => None,
    });
    let (points, clip) = outline.expect("the outline of the plan is drawn");
    assert_eq!(clip, [[80.0, 130.0], [200.0, 210.0]]);
    assert!((points[0][0] - 85.0).abs() < 1e-9 && (points[0][1] - 135.0).abs() < 1e-9);

    let pictures = studio
        .layouts
        .images
        .iter()
        .map(|(key, picture)| (key.clone(), Arc::clone(&picture.png)))
        .collect();
    let bytes = pdf::pdf_bytes(&plot, &pictures, "A-01 Ground floor").unwrap();
    assert!(bytes.starts_with(b"%PDF-"));
    // A3 lying: 420 by 297 mm in points.
    let text = String::from_utf8_lossy(&bytes);
    let media: Vec<f64> = text
        .split("/MediaBox [")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap()
        .split_whitespace()
        .map(|number| number.parse().unwrap())
        .collect();
    assert!((media[2] - 420.0 * 72.0 / 25.4).abs() < 0.01, "{media:?}");
    assert!((media[3] - 297.0 * 72.0 / 25.4).abs() < 0.01, "{media:?}");
    assert!(text.contains("/BaseFont /Helvetica"));
    assert!(text.contains("/Subtype /Image"));
    let streams = pdf_streams(&bytes);
    let content = streams
        .iter()
        .map(|stream| String::from_utf8_lossy(stream).into_owned())
        .find(|stream| stream.contains("BT"))
        .expect("a content stream with text");
    for expected in [
        "(Ground floor) Tj",
        "(Plan +1.20) Tj",
        "(1:100) Tj",
        "(A-01) Tj",
        "/Im0 Do",
        "W\nn",
        " re",
    ] {
        assert!(content.contains(expected), "{expected} in the content");
    }
    // The picture is the PNG of the view, 64 by 40 pixels of RGB.
    assert!(streams.iter().any(|stream| stream.len() == 64 * 40 * 3));

    // Written as a job of the local API, to a file.
    let path = _directory.path().join("ground floor.pdf");
    let answer = send(
        &mut studio,
        json!({"command": "export_sheet_pdf", "path": path}),
    );
    assert_eq!(answer["accepted"], true, "{answer}");
    let refused = send(
        &mut studio,
        json!({"command": "export_sheet_pdf", "path": "relative.pdf"}),
    );
    assert_eq!(refused["ok"], false);
}

#[test]
fn texts_are_written_in_the_encoding_of_the_standard_fonts() {
    assert_eq!(pdf::win_ansi("Plan +1.20"), b"Plan +1.20");
    assert_eq!(pdf::win_ansi("café – 2×"), b"caf\xe9 \x96 2\xd7");
    assert_eq!(pdf::win_ansi("平面"), b"??");
    assert!((plot::text_width("A", 0.718) - 0.667).abs() < 1e-9);
}

#[test]
fn the_api_makes_shows_places_changes_and_deletes_sheets() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let plan = made_plan(&mut studio, "Plan +1.20");
    let _ = studio.update(Message::Views(crate::views::ViewAction::Save));
    let made = send(
        &mut studio,
        json!({"command": "create_sheet", "name": "Elevations", "paper": "a2", "orientation": "portrait", "project": "Office"}),
    );
    assert_eq!(made["ok"], true, "{made}");
    assert_eq!(made["sheet"]["size_mm"], json!([420.0, 594.0]));
    assert_eq!(made["sheet"]["number"], "01");
    assert_eq!(made["sheet"]["project"], "Office");
    assert_eq!(made["sheet"]["shown"], true);
    let placed = send(
        &mut studio,
        json!({"command": "place_view", "name": "plan +1.20", "scale": "1:50", "at": [200, 300]}),
    );
    assert_eq!(placed["ok"], true, "{placed}");
    let viewport = &placed["sheet"]["viewports"][0];
    assert_eq!(viewport["guid"], plan);
    assert_eq!(viewport["size"], json!([240.0, 160.0]));
    assert_eq!(viewport["scale_label"], "1:50");
    assert_eq!(viewport["shows"], "drawing");
    let view = send(
        &mut studio,
        json!({"command": "place_view", "name": "View 1", "kind": "view"}),
    );
    assert_eq!(view["sheet"]["viewports"][1]["kind"], "view");
    let changed = send(
        &mut studio,
        json!({"command": "update_viewport", "viewport": 0, "scale": 100, "at": [150, 400], "title": "Ground floor"}),
    );
    assert_eq!(
        changed["sheet"]["viewports"][0]["size"],
        json!([120.0, 80.0])
    );
    assert_eq!(
        changed["sheet"]["viewports"][0]["centre"],
        json!([150.0, 400.0])
    );
    assert_eq!(changed["sheet"]["viewports"][0]["title"], "Ground floor");
    let sized = send(
        &mut studio,
        json!({"command": "update_viewport", "viewport": 1, "size": [100, null]}),
    );
    let size = &sized["sheet"]["viewports"][1]["size"];
    assert_eq!(size[0], 100.0, "{sized}");
    let no_scale = send(
        &mut studio,
        json!({"command": "update_viewport", "viewport": 1, "scale": 100}),
    );
    assert_eq!(no_scale["ok"], false);
    let copy = send(
        &mut studio,
        json!({"command": "duplicate_sheet", "sheet": "elevations"}),
    );
    assert_eq!(copy["sheet"]["name"], "Elevations (2)");
    assert_eq!(copy["sheet"]["viewports"].as_array().unwrap().len(), 2);
    let updated = send(
        &mut studio,
        json!({"command": "update_sheet", "sheet": "Elevations (2)", "number": "02", "paper": "a3", "orientation": "landscape"}),
    );
    assert_eq!(updated["sheet"]["size_mm"], json!([420.0, 297.0]));
    let removed = send(
        &mut studio,
        json!({"command": "remove_viewport", "viewport": 0}),
    );
    assert_eq!(removed["removed"], "Ground floor");
    let listed = send(&mut studio, json!({"command": "list_sheets"}));
    assert_eq!(listed["sheets"].as_array().unwrap().len(), 2);
    let tabs = send(&mut studio, json!({"command": "list_tabs"}));
    assert!(tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tab| tab["kind"] == "sheet"));
    let shown = send(&mut studio, json!({"command": "show_sheet", "sheet": "01"}));
    assert_eq!(shown["sheet"]["name"], "Elevations");
    let status = send(&mut studio, json!({"command": "status"}));
    assert_eq!(status["result"]["sheets"]["count"], 2);
    let deleted = send(
        &mut studio,
        json!({"command": "delete_sheet", "sheet": "02"}),
    );
    assert_eq!(deleted["ok"], true, "{deleted}");
    let unknown = send(
        &mut studio,
        json!({"command": "show_sheet", "sheet": "nothing"}),
    );
    assert_eq!(unknown["ok"], false);
    let wrong = send(
        &mut studio,
        json!({"command": "create_sheet", "paper": "letter"}),
    );
    assert_eq!(wrong["error"], "paper must be a4, a3, a2, a1 or a0");
}

#[test]
fn a_row_of_views_let_go_over_the_paper_is_placed_there() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let plan = made_plan(&mut studio, "Plan +1.20");
    let sheet = studio
        .create_layout("01", "Sheet 1", Paper::A3, true)
        .unwrap();
    let _ = studio.show_layout(&sheet);
    let _ = studio.update(Message::Layouts(LayoutAction::DragRow(
        PlacedKind::Drawing,
        plan.clone(),
    )));
    assert!(studio.layouts.drag_row.is_some());
    let _ = studio.update(Message::Layouts(LayoutAction::Drop([200.0, 150.0])));
    let viewport = &studio.layouts.list[0].viewports[0];
    assert_eq!(
        (viewport.guid.as_str(), viewport.centre),
        (plan.as_str(), [200.0, 150.0])
    );
    assert!(studio.layouts.drag_row.is_none());
    // A drag that ends elsewhere places nothing, and Escape lets go too.
    let _ = studio.update(Message::Layouts(LayoutAction::DragRow(
        PlacedKind::Drawing,
        plan.clone(),
    )));
    let _ = studio.update(Message::Layouts(LayoutAction::DragEnd));
    let _ = studio.update(Message::Layouts(LayoutAction::DragRow(
        PlacedKind::Drawing,
        plan,
    )));
    let _ = studio.update(Message::Escape);
    assert!(studio.layouts.drag_row.is_none());
    assert_eq!(studio.layouts.list[0].viewports.len(), 1);
    // Delete takes the selected viewport off the sheet.
    let id = studio.layouts.list[0].viewports[0].id.clone();
    let _ = studio.update(Message::Layouts(LayoutAction::Select(Some(id))));
    let _ = studio.update(Message::NamedKey(iced::keyboard::key::Named::Delete, true));
    assert!(studio.layouts.list[0].viewports.is_empty());
    // The 3D model shown takes the sheet away; its tab stays.
    let _ = studio.update(Message::Browser(
        crate::project_browser::BrowserAction::ShowModel,
    ));
    assert!(studio.drawing_view.shown_layout().is_none());
    assert!(studio.tabs.open().contains(&TabId::Layout(sheet)));
}

#[test]
fn a_viewport_that_waits_says_why() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (studio, _directory) = studio_with_scan();
    let mut sheet = Layout::new("01", "Sheet 1", Paper::A4, false);
    sheet.viewports.push(model::Viewport::new(
        PlacedKind::Drawing,
        &camera_views::new_guid(),
        "Section A",
        [100.0, 150.0],
        [60.0, 40.0],
    ));
    let plot = plot::plot(&sheet, |_| Content::Waiting("Being made…".into()));
    assert!(plot.texts().any(|text| text == "Being made…"));
    assert_eq!(plot.viewports.len(), 1);
    assert_eq!(
        plot.viewport_at([100.0, 150.0])
            .map(|placed| placed.id.clone()),
        Some(sheet.viewports[0].id.clone())
    );
    assert!(plot.viewport_at([20.0, 20.0]).is_none());
    drop(studio);
}

#[test]
fn file_names_from_captions_hold_no_separators() {
    assert_eq!(file_stem("A-01 Ground floor"), "A-01 Ground floor");
    assert_eq!(file_stem("01 Plan/Section: 1"), "01 Plan-Section- 1");
    assert_eq!(file_stem("..."), "sheet");
    let _ = PathBuf::new();
}

#[test]
fn the_picture_of_a_view_keeps_its_proportions_when_its_snapshot_changes() {
    // What it reads is in the language of the window; a test in Dutch
    // may run at the same time.
    let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
    let (mut studio, _directory) = studio_with_scan();
    let _ = studio.update(Message::Views(crate::views::ViewAction::Save));
    let view = studio.listed_views()[0].guid.clone();
    camera_views::remove_snapshot(&view);
    let sheet = studio
        .create_layout("01", "Views", Paper::A3, true)
        .unwrap();
    let _ = studio.show_layout(&sheet);
    // Placed before it has a picture, its frame takes a guess.
    let id = studio
        .place_on_layout(&sheet, PlacedKind::View, &view, Some([200.0, 150.0]), None)
        .unwrap();
    let guessed = studio.layouts.list[0].viewport(&id).unwrap().size;
    assert!((guessed[1] / guessed[0] - 1000.0 / 1600.0).abs() < 1e-9);
    // The view shown in another window size gets a picture of other
    // proportions: the frame keeps its width and takes them.
    camera_views::write_snapshot(&view, &png(1348, 1041)).unwrap();
    studio.layouts.pictures_checked = None;
    let _ = studio.update(Message::Layouts(LayoutAction::Fit));
    let size = studio.layouts.list[0].viewport(&id).unwrap().size;
    assert!((size[0] - guessed[0]).abs() < 1e-9, "{size:?}");
    assert!(
        (size[1] / size[0] - 1041.0 / 1348.0).abs() < 1e-9,
        "{size:?}"
    );
    let image = |studio: &Studio| {
        studio
            .shown_plot()
            .unwrap()
            .marks()
            .find_map(|(mark, _)| match mark {
                Mark::Image { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("the picture is drawn")
    };
    let rect = image(&studio);
    let drawn = [rect[1][0] - rect[0][0], rect[1][1] - rect[0][1]];
    assert!((drawn[1] / drawn[0] - 1041.0 / 1348.0).abs() < 1e-9);
    // Both sides given: the largest of its proportions within them.
    let sized = send(
        &mut studio,
        json!({"command": "update_viewport", "viewport": 0, "size": [100, 100]}),
    );
    let size = &sized["sheet"]["viewports"][0]["size"];
    assert!((size[0].as_f64().unwrap() - 100.0).abs() < 1e-9, "{sized}");
    assert!((size[1].as_f64().unwrap() - 100.0 * 1041.0 / 1348.0).abs() < 1e-9);
    // A frame of other proportions shows the picture in its own, in the
    // middle.
    assert_eq!(
        model::contained([[0.0, 0.0], [100.0, 100.0]], [200, 100]),
        [[0.0, 25.0], [100.0, 75.0]]
    );
    assert_eq!(
        model::contained([[0.0, 0.0], [100.0, 100.0]], [100, 200]),
        [[25.0, 0.0], [75.0, 100.0]]
    );
}

#[test]
fn a_copy_of_a_sheet_has_notes_of_its_own() {
    let (mut studio, _directory) = studio_with_scan();
    let sheet = studio
        .create_layout("01", "Plans", Paper::A3, true)
        .unwrap();
    let _ = studio.show_layout(&sheet);
    let noted = send(
        &mut studio,
        json!({"command": "annotate_sheet", "kind": "text", "at": [30, 30], "text": "North"}),
    );
    assert_eq!(noted["ok"], true, "{noted}");
    let copy = studio.duplicate_layout(&sheet).unwrap();
    let original_id = studio.layouts.layout(&sheet).unwrap().notes[0]
        .id()
        .to_owned();
    let copy_id = studio.layouts.layout(&copy).unwrap().notes[0]
        .id()
        .to_owned();
    assert_ne!(original_id, copy_id);
    assert!(camera_views::is_guid(&copy_id));
    // Deleted by its id, the note of the copy goes, that of the original
    // stays.
    let deleted = send(
        &mut studio,
        json!({"command": "delete_annotation", "id": copy_id}),
    );
    assert_eq!(deleted["ok"], true, "{deleted}");
    assert!(studio.layouts.layout(&copy).unwrap().notes.is_empty());
    assert_eq!(studio.layouts.layout(&sheet).unwrap().notes.len(), 1);
}
