//! Tests of the shell of the window: its title, its status bar, the header
//! of the model space, the keys while the File view is open and the language
//! command of the local API.

use std::sync::Arc;

use pointcloud_core::IndexedPoint;
use serde_json::Value;

use crate::i18n::{self, Language, TestLanguage};
use crate::native_api::{ApiCommand, ApiRequest};
use crate::selection::SelectionMask;
use crate::{
    app_title, title_for, view_cube, CameraPreset, CloudEntry, Message, ModelKey, Studio, APP_NAME,
    VERSION_LABEL,
};

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
fn api_command_is_read_from_its_json_form() {
    let command: ApiCommand =
        serde_json::from_str(r#"{"command": "set_language", "language": "nl"}"#).unwrap();
    assert!(matches!(command, ApiCommand::SetLanguage { language } if language == "nl"));
    assert!(serde_json::from_str::<ApiCommand>(r#"{"command": "set_language"}"#).is_err());
}
