//! Tests of the Mesh to Plans wizard: its card, its steps and how it sits
//! among the other parts of the window.

use super::*;
use crate::i18n::{self, Language, TestLanguage};
use crate::settings_dialog::SettingsAction;

fn wizard(action: WizardAction) -> Message {
    Message::MeshToPlans(action)
}

#[test]
fn card_opens_and_closes_and_keeps_its_step() {
    let mut studio = Studio::default();
    assert!(!studio.mesh_to_plans.is_open());
    assert!(studio.mesh_to_plans_view().is_none());

    let _ = studio.update(wizard(WizardAction::Open));
    assert!(studio.mesh_to_plans.is_open());
    assert!(studio.mesh_to_plans_view().is_some());
    let _ = studio.view();
    for step in WizardStep::ALL {
        let _ = studio.update(wizard(WizardAction::Step(step)));
        assert_eq!(studio.mesh_to_plans.step, step);
        let _ = studio.view();
    }

    let _ = studio.update(wizard(WizardAction::Close));
    assert!(!studio.mesh_to_plans.is_open());
    assert!(studio.mesh_to_plans_view().is_none());
    let _ = studio.view();

    // Opened again it shows the step it showed last.
    let _ = studio.update(wizard(WizardAction::Open));
    assert_eq!(studio.mesh_to_plans.step, WizardStep::Result);
}

#[test]
fn opening_from_the_file_view_leaves_that_view() {
    let mut studio = Studio::default();
    for page in [crate::FilePage::New, crate::FilePage::Export] {
        let _ = studio.update(Message::ToggleFile);
        let _ = studio.update(Message::FilePage(page));
        assert!(studio.file_open);
        let _ = studio.view();
        let _ = studio.update(Message::FileAction(
            crate::file_view::FileAction::MeshToPlans,
        ));
        assert!(!studio.file_open);
        assert!(studio.mesh_to_plans.is_open());
        let _ = studio.view();
        let _ = studio.update(wizard(WizardAction::Close));
    }
}

#[test]
fn escape_closes_settings_then_makes_the_card_a_strip_then_closes_the_file_view() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.update(Message::Settings(SettingsAction::Open));
    assert!(studio.settings.is_some());
    let _ = studio.view();

    let _ = studio.update(Message::Escape);
    assert!(studio.settings.is_none());
    assert!(
        studio.mesh_to_plans.covers_model(),
        "the card waits its turn"
    );

    let _ = studio.update(Message::Escape);
    assert!(studio.mesh_to_plans.is_open() && !studio.mesh_to_plans.covers_model());
    assert!(studio.mesh_to_plans_view().is_none());
    assert!(studio.mesh_to_plans_strip().is_some());
    let _ = studio.view();

    // The File view opened over the strip closes next; the wizard stays a
    // strip, and Escape never takes it away.
    let _ = studio.update(Message::ToggleFile);
    let _ = studio.view();
    let _ = studio.update(Message::Escape);
    assert!(!studio.file_open);
    let _ = studio.update(Message::Escape);
    assert!(studio.mesh_to_plans.is_open() && studio.mesh_to_plans.minimized);
}

#[test]
fn the_strip_goes_back_and_on_and_back_to_the_card() {
    let mut studio = Studio::default();
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.update(wizard(WizardAction::Step(WizardStep::Mesh)));
    let _ = studio.update(wizard(WizardAction::Minimize));
    assert!(studio.mesh_to_plans_strip().is_some());
    let _ = studio.view();

    let _ = studio.update(wizard(WizardAction::Back));
    assert_eq!(studio.mesh_to_plans.step, WizardStep::Prepare);
    let _ = studio.update(wizard(WizardAction::Restore));
    assert!(studio.mesh_to_plans.covers_model());
    assert!(studio.mesh_to_plans_strip().is_none());

    // The button of the ribbon brings back the card as well, and Close
    // takes the strip away.
    let _ = studio.update(wizard(WizardAction::Minimize));
    let _ = studio.update(wizard(WizardAction::Open));
    assert!(studio.mesh_to_plans.covers_model());
    let _ = studio.update(wizard(WizardAction::Minimize));
    let _ = studio.update(wizard(WizardAction::Close));
    assert!(!studio.mesh_to_plans.is_open());
    assert!(studio.mesh_to_plans_strip().is_none());
    // A closed wizard has no card to make a strip of.
    let _ = studio.update(wizard(WizardAction::Minimize));
    let _ = studio.update(wizard(WizardAction::Restore));
    assert!(!studio.mesh_to_plans.is_open());
}

#[test]
fn keys_leave_the_model_alone_under_the_card_and_act_beside_the_strip() {
    let mut studio = Studio::default();
    let _ = studio.update(wizard(WizardAction::Open));
    assert!(studio.model_covered());
    studio.zoom = 3.0;
    let _ = studio.update(Message::ModelKey(crate::ModelKey::Fit));
    assert_eq!(studio.zoom, 3.0);

    let _ = studio.update(wizard(WizardAction::Minimize));
    assert!(!studio.model_covered());
    let _ = studio.update(Message::ModelKey(crate::ModelKey::Fit));
    assert_eq!(studio.zoom, 1.0);
}

/// Send a command the way the window receives it.
fn send(studio: &mut Studio, command: crate::native_api::ApiCommand) -> Value {
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.update(Message::ApiRequest(crate::native_api::ApiRequest {
        command,
        reply,
    }));
    receive.recv().unwrap()
}

fn command(body: Value) -> crate::native_api::ApiCommand {
    serde_json::from_value(body).unwrap()
}

fn status(studio: &mut Studio) -> Value {
    send(studio, crate::native_api::ApiCommand::Status)["result"]["mesh_to_plans"].clone()
}

#[test]
fn api_shows_the_wizard_as_card_or_strip_on_a_step_and_takes_it_away() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    let closed = status(&mut studio);
    assert_eq!(closed["open"], false);
    assert_eq!(closed["minimized"], false);
    assert_eq!(closed["step"], "prepare");
    assert_eq!(closed["next_ready"], false);
    assert_eq!(closed["next_reason"], "Run this step first");
    let steps = closed["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 9);
    assert_eq!(
        steps[3],
        json!({"id": "walls", "number": "3a", "name": "Walls", "status": "not_run"})
    );

    let _ = studio.update(Message::ToggleFile);
    let shown = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_view", "open": true, "step": "Mesh"})),
    );
    assert_eq!(shown["ok"], true, "{shown}");
    assert_eq!(shown["mesh_to_plans"]["open"], true);
    assert_eq!(shown["mesh_to_plans"]["step"], "mesh");
    assert!(!studio.file_open, "the File view steps aside");
    assert_eq!(shown["mesh_to_plans"], status(&mut studio));
    let _ = studio.view();

    // The card covers the model, so the File view does not open under it
    // and a screenshot is refused.
    let file_view = send(
        &mut studio,
        command(json!({"command": "file_view", "open": true})),
    );
    assert_eq!(
        file_view["error"],
        "the Mesh to Plans wizard is open; minimize it first"
    );
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.api_screenshot(reply, None, None, None);
    assert_eq!(
        receive.try_recv().unwrap()["error"],
        crate::screenshot::COVERED
    );

    let strip = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_view", "open": true, "minimized": true})),
    );
    assert_eq!(strip["mesh_to_plans"]["minimized"], true);
    assert_eq!(strip["mesh_to_plans"]["step"], "mesh");
    assert!(!studio.model_covered());
    let skipped = send(
        &mut studio,
        command(json!({"command": "file_view", "open": true})),
    );
    assert_eq!(skipped["ok"], true, "the strip covers nothing");
    let _ = studio.update(Message::ToggleFile);

    // Opening without minimized shows the card again.
    let card = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_view", "open": true})),
    );
    assert_eq!(card["mesh_to_plans"]["minimized"], false);
    let away = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_view", "open": false})),
    );
    assert_eq!(away["mesh_to_plans"]["open"], false);
    assert!(!studio.mesh_to_plans.is_open());
}

#[test]
fn api_refuses_what_the_wizard_cannot_show() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    for (body, error) in [
        (
            json!({"command": "mesh_to_plans_view", "open": true, "step": "plans"}),
            "unknown step; use prepare, mesh, views, walls, openings, rooms, sheet, site, result",
        ),
        (
            json!({"command": "mesh_to_plans_view", "open": false, "step": "mesh"}),
            "step and minimized can only be given with open: true",
        ),
        (
            json!({"command": "mesh_to_plans_view", "open": false, "minimized": true}),
            "step and minimized can only be given with open: true",
        ),
    ] {
        let answer = send(&mut studio, command(body));
        assert_eq!(answer, json!({"ok": false, "error": error}));
        assert!(
            !studio.mesh_to_plans.is_open(),
            "a refused command changes nothing"
        );
    }
    let _ = studio.update(Message::Settings(SettingsAction::Open));
    let covered = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_view", "open": true})),
    );
    assert_eq!(covered["error"], "the Settings dialog is open");
    let _ = studio.update(Message::Escape);
    assert!(serde_json::from_value::<crate::native_api::ApiCommand>(
        json!({"command": "mesh_to_plans_view"})
    )
    .is_err());
}

/// Lay out and draw the window at `size` with the software renderer, as
/// it would be on screen; the canvas of the scene notes where it lies.
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
        Layout::new(&node),
        mouse::Cursor::Unavailable,
        &Rectangle::with_size(size),
    );
}

#[test]
fn the_strip_lies_above_the_scene_and_out_of_a_screenshot() {
    let mut studio = Studio::default();
    let window = Size::new(1440.0, 900.0);
    draw_window(&studio, window);
    let alone = studio.shown_canvas_bounds().expect("the scene was drawn");

    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.update(wizard(WizardAction::Minimize));
    draw_window(&studio, window);
    let beside = studio.shown_canvas_bounds().expect("the scene was drawn");
    // The strip takes its own height in the column of the scene: the scene
    // starts lower by that much and ends where it did, so a screenshot,
    // which is cut to the scene, has nothing of the strip in it.
    assert_eq!(beside.x, alone.x);
    assert_eq!(beside.width, alone.width);
    assert!(beside.y >= alone.y + 24.0, "{beside:?} {alone:?}");
    assert!((beside.y + beside.height - alone.y - alone.height).abs() < 0.5);
    // With the strip shown, the screenshot command goes ahead.
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.api_screenshot(reply, None, None, None);
    assert!(receive.try_recv().is_err(), "not refused");

    // The card lies over the window: the scene keeps its place beneath it.
    let _ = studio.update(wizard(WizardAction::Restore));
    draw_window(&studio, window);
    assert_eq!(studio.shown_canvas_bounds(), Some(alone));
}

#[test]
fn next_waits_until_the_step_may_be_left_and_says_why() {
    let mut studio = Studio::default();
    let _ = studio.update(wizard(WizardAction::Open));
    let reason = studio.mesh_to_plans.step_ready().unwrap_err();
    assert_eq!(reason.english(), "Run this step first");
    let _ = studio.update(wizard(WizardAction::Next));
    assert_eq!(studio.mesh_to_plans.step, WizardStep::Prepare);

    // The preparation cannot be skipped; the mesh can.
    let _ = studio.update(wizard(WizardAction::Skip));
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::NotRun
    );
    let _ = studio.update(wizard(WizardAction::Step(WizardStep::Mesh)));
    let _ = studio.view();
    let _ = studio.update(wizard(WizardAction::Skip));
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Mesh),
        StepStatus::Skipped
    );
    assert!(studio.mesh_to_plans.step_ready().is_ok());
    let _ = studio.view();
    let _ = studio.update(wizard(WizardAction::Next));
    assert_eq!(studio.mesh_to_plans.step, WizardStep::Views);

    // Back needs nothing, and stops at the first step.
    for _ in 0..4 {
        let _ = studio.update(wizard(WizardAction::Back));
    }
    assert_eq!(studio.mesh_to_plans.step, WizardStep::Prepare);

    // The last step has no next one.
    let _ = studio.update(wizard(WizardAction::Step(WizardStep::Result)));
    let reason = studio.mesh_to_plans.step_ready().unwrap_err();
    assert_eq!(reason.english(), "This is the last step");
}

#[test]
fn steps_have_their_number_and_name_and_run_in_order() {
    for step in WizardStep::ALL {
        assert!(i18n::has_entry(step.label()), "{}", step.label());
        assert!(i18n::has_entry(step.lead()), "{}", step.lead());
    }
    assert_eq!(WizardStep::Prepare.previous(), None);
    assert_eq!(WizardStep::Prepare.next(), Some(WizardStep::Mesh));
    assert_eq!(WizardStep::Sheet.next(), Some(WizardStep::Site));
    assert_eq!(WizardStep::Result.next(), None);
    let numbers: Vec<&str> = WizardStep::ALL
        .into_iter()
        .map(WizardStep::number)
        .collect();
    assert_eq!(numbers, ["0", "1", "2", "3a", "3b", "3c", "3d", "4", "5"]);
    let plans: Vec<WizardStep> = WizardStep::ALL
        .into_iter()
        .filter(|step| step.in_plans())
        .collect();
    assert_eq!(
        plans,
        [
            WizardStep::Walls,
            WizardStep::Openings,
            WizardStep::Rooms,
            WizardStep::Sheet
        ]
    );
}

#[test]
fn card_and_ribbon_build_in_dutch() {
    let _language = TestLanguage::hold(Language::Table(0));
    let mut studio = Studio::default();
    let _ = studio.view();
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.view();
    assert_eq!(tr("Mesh to Plans"), "Mesh to Plans");
    assert_eq!(tr(WizardStep::Prepare.label()), "Voorbereiding");
    assert_eq!(StepStatus::Skipped.text(), "Overgeslagen");
    assert_eq!(
        studio.mesh_to_plans.step_ready().unwrap_err().translated(),
        "Voer deze stap eerst uit"
    );
}

#[test]
fn card_takes_nine_tenths_of_the_window_and_at_least_its_least_size() {
    let share = Share::new(Space::new(1, 1), CARD_SHARE, CARD_MIN);
    let size = share.size_in(Size::new(2000.0, 1000.0));
    assert!((size.width - 1800.0).abs() < 1e-3 && (size.height - 900.0).abs() < 1e-3);
    // A smaller window keeps the least size, and a window smaller than that
    // is filled.
    assert_eq!(share.size_in(Size::new(1000.0, 700.0)), CARD_MIN);
    assert_eq!(
        share.size_in(Size::new(800.0, 600.0)),
        Size::new(800.0, 600.0)
    );
}
