//! Tests of the Pointcloud to Drawing wizard: its card, its steps and how it sits
//! among the other parts of the window.

use super::*;
use crate::i18n::{self, Language, TestLanguage};
use crate::settings_dialog::SettingsAction;
use crate::ui_style;

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
fn next_looks_ready_only_when_it_is() {
    let studio = Studio::default();
    let theme = studio.ui_theme.iced();
    let colors = studio.ui_theme.colors();
    let ready = ui_style::primary(&theme, button::Status::Active);
    let waiting = ui_style::primary(&theme, button::Status::Disabled);
    assert_eq!(
        ready.background,
        Some(iced::Background::Color(colors.accent))
    );
    // While it waits it is drawn at half its strength, as the style book
    // draws a button that cannot be pressed.
    assert_eq!(
        waiting.background,
        Some(iced::Background::Color(colors.accent.scale_alpha(0.5)))
    );
    assert_eq!(waiting.text_color, ready.text_color.scale_alpha(0.5));
}

#[test]
fn show_in_model_leaves_the_drawing_view_for_the_model() {
    let mut studio = Studio::default();
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.update(Message::DrawingView(drawing_view::DrawingViewAction::Show(
        true,
    )));
    assert!(studio.drawing_view.shown);
    let _ = studio.update(wizard(WizardAction::Minimize));
    assert!(!studio.drawing_view.shown, "the strip lies above the model");
    // Through the local API as well.
    let _ = studio.update(wizard(WizardAction::Restore));
    let _ = studio.update(Message::DrawingView(drawing_view::DrawingViewAction::Show(
        true,
    )));
    let strip = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_view", "open": true, "minimized": true})),
    );
    assert_eq!(strip["mesh_to_plans"]["minimized"], true);
    assert!(!studio.drawing_view.shown);
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
        "the Pointcloud to Drawing wizard is open; minimize it first"
    );
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.api_screenshot(reply, None, None, None, false);
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
    let _ = studio.api_screenshot(reply, None, None, None, false);
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
    assert_eq!(tr("Pointcloud to Drawing"), "Puntenwolk naar tekening");
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

/// Steps that end at once.
const QUICK: Work = Work::Placeholder {
    ticks: 3,
    tick: std::time::Duration::ZERO,
};

/// What the worker thread of the running job does, and its message to the
/// window.
fn finish(studio: &mut Studio) {
    let job = studio.mesh_to_plans.job.as_ref().expect("a job runs");
    let (serial, input, control) = (
        job.serial,
        std::sync::Arc::clone(&job.input),
        std::sync::Arc::clone(&job.control),
    );
    let end = PipelineEnd::of(pipeline::run(&input, &control));
    let _ = studio.update(wizard(WizardAction::Finished(serial, end)));
}

fn busy(studio: &mut Studio) -> Vec<&'static str> {
    let status = send(studio, crate::native_api::ApiCommand::Status);
    crate::mcp::busy(&status["result"])
}

#[test]
fn a_cancelled_job_stops_within_one_poll() {
    use std::time::{Duration, Instant};
    // The work the window gives a step, cancelled while it runs.
    let input = pipeline::JobInput {
        steps: WizardStep::ALL.to_vec(),
        work: pipeline::PLACEHOLDER,
        confirm: false,
        prepare: None,
        sources: Vec::new(),
        prepare_basis: None,
    };
    let control = pipeline::Control::default();
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| pipeline::run(&input, &control));
        let deadline = Instant::now() + Duration::from_secs(20);
        while control.snapshot().done < 2 {
            assert!(Instant::now() < deadline, "the job did not start");
            std::thread::sleep(Duration::from_millis(1));
        }
        let asked = Instant::now();
        control.cancel();
        let ended = worker.join().unwrap();
        let took = asked.elapsed();
        assert!(
            matches!(ended, Err(pointcloud_core::LoadError::Cancelled)),
            "{ended:?}"
        );
        assert!(took < pipeline::POLL, "stopped after {took:?}");
    });
    assert!(control.finished().is_empty());
    assert_eq!(control.snapshot().place, 0);
}

#[test]
fn a_step_runs_on_the_worker_and_then_waits_for_confirmation() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    studio.mesh_to_plans.work = QUICK;
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.update(wizard(WizardAction::Run));
    assert!(studio.mesh_to_plans.is_running());
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Running
    );
    assert_eq!(studio.status, "Pointcloud to Drawing: 0 Preparation…");
    assert_eq!(
        studio.mesh_to_plans.step_ready().unwrap_err().english(),
        "Wait until this step has finished"
    );
    let lines = studio.progress_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].phase, crate::open_progress::Phase::MeshToPlans);
    assert_eq!(lines[0].title, "Pointcloud to Drawing");
    assert_eq!(lines[0].detail, "Step 1 of 1  ·  0 Preparation");
    assert!(matches!(
        lines[0].cancel,
        Some(Message::MeshToPlans(WizardAction::Cancel))
    ));
    studio.track_progress();
    let _ = studio.view();

    // One job at a time; the status reports it as work under way, and its
    // job of the local API follows it.
    let _ = studio.update(wizard(WizardAction::Run));
    assert_eq!(
        studio.status,
        "Pointcloud to Drawing is already running a step"
    );
    assert_eq!(busy(&mut studio), ["mesh_to_plans"]);
    let reported = status(&mut studio);
    assert_eq!(reported["job"]["state"], "running");
    assert_eq!(reported["job"]["steps"], json!(["prepare"]));
    assert_eq!(reported["steps"][0]["status"], "running");
    let id = reported["job_id"].as_str().unwrap().to_owned();
    let _ = studio.update(wizard(WizardAction::Poll));
    let job = send(
        &mut studio,
        crate::native_api::ApiCommand::Job { id: id.clone() },
    );
    assert_eq!(job["job"]["operation"], "mesh_to_plans");
    assert_eq!(job["job"]["state"], "running");

    finish(&mut studio);
    assert!(!studio.mesh_to_plans.is_running());
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Done
    );
    assert!(studio
        .status
        .starts_with("Pointcloud to Drawing: 1 step done in "));
    assert_eq!(
        studio.mesh_to_plans.step_ready().unwrap_err().english(),
        "Confirm the result of this step first"
    );
    assert!(busy(&mut studio).is_empty());
    let reported = status(&mut studio);
    assert_eq!(reported["job"], Value::Null);
    assert_eq!(reported["job_id"], id.as_str());
    assert_eq!(reported["last"]["state"], "complete");
    assert_eq!(reported["last"]["finished"][0]["id"], "prepare");
    let job = send(&mut studio, crate::native_api::ApiCommand::Job { id });
    assert_eq!(job["job"]["state"], "complete");
    assert!(studio.progress_lines().is_empty());
    let _ = studio.view();

    let _ = studio.update(wizard(WizardAction::Confirm));
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Confirmed
    );
    let _ = studio.update(wizard(WizardAction::Next));
    assert_eq!(studio.mesh_to_plans.step, WizardStep::Mesh);
}

#[test]
fn run_all_confirms_each_step_it_runs_and_leaves_skipped_ones() {
    let mut studio = Studio::default();
    studio.mesh_to_plans.work = QUICK;
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.update(wizard(WizardAction::Step(WizardStep::Mesh)));
    let _ = studio.update(wizard(WizardAction::Skip));
    let _ = studio.update(wizard(WizardAction::RunAll));
    let steps = studio
        .mesh_to_plans
        .job
        .as_ref()
        .unwrap()
        .input
        .steps
        .clone();
    assert_eq!(steps.len(), 8);
    assert!(!steps.contains(&WizardStep::Mesh));
    finish(&mut studio);
    for step in WizardStep::ALL {
        let expected = if step == WizardStep::Mesh {
            StepStatus::Skipped
        } else {
            StepStatus::Confirmed
        };
        assert_eq!(*studio.mesh_to_plans.status(step), expected, "{step:?}");
    }
    assert!(studio
        .status
        .starts_with("Pointcloud to Drawing: 8 steps done in "));
    let _ = studio.view();

    let _ = studio.update(wizard(WizardAction::RunAll));
    assert!(!studio.mesh_to_plans.is_running());
    assert_eq!(
        studio.status,
        "Every step of Pointcloud to Drawing is confirmed or skipped"
    );
}

#[test]
fn a_cancelled_job_keeps_the_steps_it_finished() {
    use std::time::{Duration, Instant};
    let mut studio = Studio::default();
    studio.mesh_to_plans.work = Work::Placeholder {
        ticks: 2,
        tick: Duration::from_millis(20),
    };
    let _ = studio.update(wizard(WizardAction::RunAll));
    let job = studio.mesh_to_plans.job.as_ref().unwrap();
    let (serial, input, control) = (
        job.serial,
        std::sync::Arc::clone(&job.input),
        std::sync::Arc::clone(&job.control),
    );
    let ended = std::thread::scope(|scope| {
        let worker = scope.spawn(|| pipeline::run(&input, &control));
        let deadline = Instant::now() + Duration::from_secs(20);
        while control.finished().is_empty() {
            assert!(Instant::now() < deadline, "no step ended");
            std::thread::sleep(Duration::from_millis(1));
        }
        let _ = studio.update(wizard(WizardAction::Poll));
        let _ = studio.update(wizard(WizardAction::Cancel));
        assert_eq!(studio.status, "Cancelling Pointcloud to Drawing…");
        PipelineEnd::of(worker.join().unwrap())
    });
    assert_eq!(ended, PipelineEnd::Cancelled);
    let _ = studio.update(wizard(WizardAction::Finished(serial, ended)));
    let kept = control.finished().len();
    assert!((1..9).contains(&kept), "{kept}");
    for (place, step) in WizardStep::ALL.into_iter().enumerate() {
        let expected = if place < kept {
            StepStatus::Confirmed
        } else {
            StepStatus::NotRun
        };
        assert_eq!(*studio.mesh_to_plans.status(step), expected, "{step:?}");
    }
    assert!(studio
        .status
        .starts_with("Pointcloud to Drawing cancelled after "));
    assert_eq!(status(&mut studio)["last"]["state"], "cancelled");

    // A message of a job that is no longer under way changes nothing.
    let _ = studio.update(wizard(WizardAction::Finished(serial, PipelineEnd::Done)));
    assert_eq!(status(&mut studio)["last"]["state"], "cancelled");
}

#[test]
fn a_failed_step_says_why_and_exit_cancels_a_job() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    studio.mesh_to_plans.work = QUICK;
    let _ = studio.update(wizard(WizardAction::Step(WizardStep::Views)));
    let _ = studio.update(wizard(WizardAction::Run));
    let serial = studio.mesh_to_plans.job.as_ref().unwrap().serial;
    let _ = studio.update(wizard(WizardAction::Finished(
        serial,
        PipelineEnd::Failed("disk full".into()),
    )));
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Views),
        StepStatus::Failed("disk full".into())
    );
    assert_eq!(
        studio.mesh_to_plans.step_ready().unwrap_err().english(),
        "This step failed: disk full"
    );
    assert_eq!(
        studio.status,
        "Pointcloud to Drawing failed in 2 Sections, elevations and raw plans: disk full"
    );
    let last = status(&mut studio)["last"].clone();
    assert_eq!(last["state"], "failed");
    assert_eq!(last["step"], "views");
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.view();

    // Exit asks the worker to stop; Escape leaves it running.
    let _ = studio.update(wizard(WizardAction::Run));
    let control = std::sync::Arc::clone(&studio.mesh_to_plans.job.as_ref().unwrap().control);
    let _ = studio.update(Message::Escape);
    assert!(!control.cancelling());
    let _ = studio.update(Message::Exit);
    assert!(control.cancelling());
}

#[test]
fn a_job_waits_for_other_heavy_work() {
    // An octree being built.
    let mut studio = Studio::default();
    studio
        .index_jobs
        .push(crate::index_jobs::IndexJob::for_test("scan.xyz"));
    let _ = studio.update(wizard(WizardAction::Run));
    assert!(!studio.mesh_to_plans.is_running());
    assert_eq!(
        studio.status,
        "Pointcloud to Drawing waits: an octree is being made; wait for it or cancel it first"
    );
}

/// A window with the generated building of the survey tests open: two
/// storeys of 3 m and a roof, turned by 25 degrees.
fn studio_with_building(directory: &std::path::Path) -> (Studio, std::path::PathBuf) {
    let path = directory.join("building.xyz");
    std::fs::write(&path, crate::survey::tests::building_xyz()).unwrap();
    let cloud = std::sync::Arc::new(pointcloud_core::open(&path, 1_000_000).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    // An index built on its own would make the wizard wait.
    studio.index_jobs.clear();
    (studio, path)
}

/// Run step 0 of a window on the worker, as Run this step does, and write
/// the project as the window does once the changes have come to rest.
fn prepare_and_save(studio: &mut Studio) {
    let _ = studio.update(wizard(WizardAction::Step(WizardStep::Prepare)));
    let _ = studio.update(wizard(WizardAction::Run));
    assert!(studio.mesh_to_plans.is_running(), "{}", studio.status);
    finish(studio);
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
}

#[test]
fn step_0_surveys_the_scans_finds_the_levels_and_writes_the_project() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_building(directory.path());
    let _ = studio.update(wizard(WizardAction::Open));
    // The scan names a new project; the folder is chosen here.
    assert_eq!(studio.mesh_to_plans.project_name, "building");
    let folder = directory.path().join("Office");
    studio.mesh_to_plans.project_folder = folder.display().to_string();
    prepare_and_save(&mut studio);
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Done,
        "{}",
        studio.status
    );
    let prepare = &studio.mesh_to_plans.prepare;
    let peil = prepare.peil_z();
    let found: Vec<(String, String, f64)> = prepare
        .levels
        .iter()
        .map(|level| (level.id.clone(), level.name.clone(), level.floor_z - peil))
        .collect();
    assert_eq!(found.len(), 3, "{found:?}");
    for ((id, name, height), (truth_id, truth_name, truth)) in found.iter().zip([
        ("00", "Ground floor", 0.0),
        ("01", "Floor 1", 3.0),
        ("R", "Roof", 6.0),
    ]) {
        assert_eq!((id.as_str(), name.as_str()), (truth_id, truth_name));
        assert!((height - truth).abs() < 0.003, "{found:?}");
    }
    assert!(peil.abs() < 0.003);
    let frame = prepare.frame().unwrap();
    assert!((frame.rotation_deg - 25.0).abs() < 0.05, "{frame:?}");
    let survey = prepare.survey.as_ref().unwrap();
    assert!((survey.footprint_area - 40.0).abs() < 1.5);
    // The twelve stray points far below, one by one: too few for a cluster.
    assert_eq!((survey.below_points, survey.below_clusters), (12, 0));
    assert!(survey.below_z.unwrap() <= peil - 2.0 + 1e-9);
    assert!(prepare.regions.building.is_some() && prepare.regions.core.is_some());
    assert_eq!(prepare.selected, Some(0), "P is selected");
    assert!(folder.join("survey").join("profile.csv").is_file());
    assert!(folder.join("survey").join("top.png").is_file());
    let _ = studio.view();

    // The project is in its folder, first in the list of recent ones.
    let file = folder.join(project::FILE_NAME);
    let saved = project::load(&file).unwrap();
    assert_eq!(saved.name, "building");
    assert_eq!(saved.levels, studio.mesh_to_plans.prepare.levels);
    assert_eq!(saved.status(WizardStep::Prepare), StepStatus::Done);
    assert_eq!(saved.sources.len(), 1);
    assert!(saved.steps["prepare"].basis.is_some());
    assert_eq!(studio.mesh_to_plans.recent, std::slice::from_ref(&file));
    assert_eq!(
        studio
            .mesh_to_plans
            .project
            .as_ref()
            .map(|place| &place.file),
        Some(&file)
    );
    let status = status(&mut studio);
    assert_eq!(status["prepare"]["levels"][1]["id"], "01");
    assert_eq!(status["prepare"]["below_points"], 12);
    assert_eq!(status["prepare"]["below_clusters"], 0);
    let height = status["prepare"]["levels"][1]["floor_above_p"]
        .as_f64()
        .unwrap();
    assert!((height - 3.0).abs() < 0.003);
    assert_eq!(status["project"], file.display().to_string());

    // Confirm levels locks them; Edit levels opens them again.
    let _ = studio.update(wizard(WizardAction::Confirm));
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Confirmed
    );
    let _ = studio.update(wizard(WizardAction::Prepare(PrepareAction::Remove)));
    assert_eq!(studio.mesh_to_plans.prepare.levels.len(), 3);
    assert_eq!(studio.status, "Edit levels first: they are confirmed");
    let _ = studio.update(wizard(WizardAction::Prepare(PrepareAction::Unlock)));
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Done
    );
    let _ = studio.update(wizard(WizardAction::Confirm));
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
    assert_eq!(
        project::load(&file).unwrap().status(WizardStep::Prepare),
        StepStatus::Confirmed
    );
}

/// Let the tests of this thread keep their projects in `root`.
fn projects_in(root: &std::path::Path) {
    project::TEST_ROOT.with(|test_root| *test_root.borrow_mut() = Some(root.to_path_buf()));
}

#[test]
fn a_new_project_never_takes_the_folder_of_another() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Projects");
    projects_in(&root);
    let (mut first, scan) = studio_with_building(directory.path());
    let _ = first.update(wizard(WizardAction::Open));
    let folder = root.join("building");
    assert_eq!(
        first.mesh_to_plans.project_folder,
        folder.display().to_string()
    );
    prepare_and_save(&mut first);
    let _ = first.update(wizard(WizardAction::Confirm));
    let revision = first.mesh_to_plans.save_revision;
    let _ = first.update(wizard(WizardAction::Save(revision)));
    let file = folder.join(project::FILE_NAME);
    let before = std::fs::read(&file).unwrap();

    // The same scan opened again: the proposed folder is the next free one,
    // and the page offers the project that is there.
    let cloud = std::sync::Arc::new(pointcloud_core::open(&scan, 1_000_000).unwrap());
    let mut second = Studio::default();
    let _ = second.update(Message::Loaded(Ok(cloud)));
    second.index_jobs.clear();
    let _ = second.update(wizard(WizardAction::Open));
    assert_eq!(
        second.mesh_to_plans.project_folder,
        root.join("building 2").display().to_string()
    );
    assert_eq!(
        second.mesh_to_plans.existing,
        Some((file.clone(), "building".to_owned()))
    );
    assert!(second.folder_taken_as_seen().is_none());
    let _ = second.view();
    // Renamed, the folder follows the name, and back.
    act(&mut second, PrepareAction::ProjectName("Office".into()));
    assert_eq!(
        second.mesh_to_plans.project_folder,
        root.join("Office").display().to_string()
    );
    assert_eq!(second.mesh_to_plans.existing, None);
    act(&mut second, PrepareAction::ProjectName("building".into()));
    assert!(second.mesh_to_plans.project_folder.ends_with("building 2"));

    // The folder of the first project typed by hand: the run is refused and
    // the project there stays as it was.
    act(
        &mut second,
        PrepareAction::ProjectFolder(folder.display().to_string()),
    );
    let reason = format!(
        "{} holds another project: resume it, or choose another folder",
        folder.display()
    );
    assert_eq!(
        second
            .folder_taken_as_seen()
            .map(|sentence| sentence.english()),
        Some(reason.clone())
    );
    let _ = second.view();
    let _ = second.update(wizard(WizardAction::Run));
    assert!(!second.mesh_to_plans.is_running());
    assert_eq!(
        second.status,
        format!("Pointcloud to Drawing cannot prepare: {reason}")
    );
    assert_eq!(std::fs::read(&file).unwrap(), before);
    let answer = send(
        &mut second,
        command(json!({"command": "mesh_to_plans_action", "action": "run", "folder": folder})),
    );
    assert_eq!(answer["ok"], false);
    assert_eq!(
        answer["error"],
        format!("Pointcloud to Drawing cannot prepare: {reason}")
    );

    // Resume that project goes on with it.
    let (resume, _) = second.mesh_to_plans.existing.clone().unwrap();
    let _ = second.update(wizard(WizardAction::Resume(resume)));
    assert_eq!(
        second
            .mesh_to_plans
            .project
            .as_ref()
            .map(|place| &place.file),
        Some(&file)
    );
    assert_eq!(
        *second.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Confirmed
    );
    assert_eq!(second.mesh_to_plans.existing, None);
}

#[test]
fn a_project_that_appears_in_the_folder_meanwhile_is_not_written_over() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    projects_in(&directory.path().join("Projects"));
    let (mut studio, _) = studio_with_building(directory.path());
    let folder = directory.path().join("Office");
    studio.mesh_to_plans.project_folder = folder.display().to_string();
    let _ = studio.update(wizard(WizardAction::Run));
    finish(&mut studio);
    // Another window writes its project there before this one is written.
    let file = folder.join(project::FILE_NAME);
    let other = project::MeshToPlansProject::new("Other");
    project::save(&file, &other).unwrap();
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
    assert_eq!(project::load(&file).unwrap(), other);
    assert!(studio.mesh_to_plans.project.is_none());
    assert_eq!(
        studio.status,
        format!(
            "{} holds another project: resume it, or choose another folder",
            folder.display()
        )
    );
}

/// The basis of step 0 of a window without scans.
fn basis_without_scans(studio: &Studio) -> u128 {
    project::prepare_basis(&[], &studio.mesh_to_plans.prepare.regions)
}

/// A window with three confirmed levels in step 0, as a run on no scans
/// left them, whose project goes to `folder`.
fn studio_with_confirmed_levels(folder: &std::path::Path) -> Studio {
    let mut studio = studio_with_levels();
    let basis = basis_without_scans(&studio);
    let wizard = &mut studio.mesh_to_plans;
    wizard.set_status(WizardStep::Prepare, StepStatus::Confirmed);
    wizard.runs[WizardStep::Prepare.place()] = StepRun {
        basis: Some(basis),
        finished: Some(1_759_700_100),
        seconds: Some(11.0),
    };
    wizard.project_folder = folder.display().to_string();
    studio
}

#[test]
fn a_step_that_runs_is_written_as_it_was_and_exit_writes_what_waits() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Office");
    let file = folder.join(project::FILE_NAME);
    let mut studio = studio_with_confirmed_levels(&folder);
    studio.mesh_to_plans.work = Work::Placeholder {
        ticks: 1_000,
        tick: std::time::Duration::from_millis(5),
    };
    // Step 0 runs again; meanwhile a change of NAP is written.
    let _ = studio.update(wizard(WizardAction::Run));
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Running
    );
    act(&mut studio, PrepareAction::NapOffset("1.85".into()));
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
    let saved = project::load(&file).unwrap();
    assert_eq!(saved.status(WizardStep::Prepare), StepStatus::Confirmed);
    let record = &saved.steps["prepare"];
    let basis = basis_without_scans(&studio);
    assert_eq!(record.basis, Some(project::basis_text(basis)));
    assert_eq!(record.seconds, Some(11.0));
    assert_eq!(saved.datum.nap_offset, Some(1.85));

    // North typed, and the window closed before the change came to rest:
    // Exit writes it, and the step that ran is as it was.
    let control = std::sync::Arc::clone(&studio.mesh_to_plans.job.as_ref().unwrap().control);
    act(&mut studio, PrepareAction::North("12".into()));
    let _ = studio.update(Message::Exit);
    assert!(control.cancelling());
    let saved = project::load(&file).unwrap();
    assert_eq!(saved.datum.north_deg, Some(12.0));
    assert_eq!(saved.status(WizardStep::Prepare), StepStatus::Confirmed);
    assert_eq!(saved.levels, studio.mesh_to_plans.prepare.levels);
}

#[test]
fn resuming_another_project_writes_the_change_that_waits_to_the_first() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("First");
    let second = directory.path().join("Second");
    let mut other = studio_with_confirmed_levels(&second);
    other.mesh_to_plans.project_name = "Second".into();
    let revision = {
        let _ = other.update(wizard(WizardAction::Prepare(PrepareAction::Unlock)));
        other.mesh_to_plans.save_revision
    };
    let _ = other.update(wizard(WizardAction::Save(revision)));
    let second_file = second.join(project::FILE_NAME);
    assert!(second_file.is_file());

    let mut studio = studio_with_confirmed_levels(&first);
    let _ = studio.update(wizard(WizardAction::Prepare(PrepareAction::Unlock)));
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
    let first_file = first.join(project::FILE_NAME);
    // A level renamed, and another project resumed at once.
    act(&mut studio, PrepareAction::Select(1));
    act(&mut studio, PrepareAction::Name("Office floor".into()));
    let waiting = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Resume(second_file.clone())));
    let saved = project::load(&first_file).unwrap();
    assert_eq!(saved.levels[1].name, "Office floor");
    assert_eq!(studio.mesh_to_plans.project_name, "Second");
    // The save asked for before writes nothing now.
    let before = std::fs::read(&second_file).unwrap();
    let _ = studio.update(wizard(WizardAction::Save(waiting)));
    assert_eq!(std::fs::read(&second_file).unwrap(), before);
}

#[test]
fn steps_that_only_stand_in_for_their_work_are_not_kept() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Office");
    let mut studio = studio_with_confirmed_levels(&folder);
    studio.mesh_to_plans.work = QUICK;
    let _ = studio.update(wizard(WizardAction::Step(WizardStep::Mesh)));
    let _ = studio.update(wizard(WizardAction::Skip));
    let _ = studio.update(wizard(WizardAction::RunAll));
    finish(&mut studio);
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Result),
        StepStatus::Confirmed
    );
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
    let saved = project::load(&folder.join(project::FILE_NAME)).unwrap();
    let kept: Vec<&str> = saved.steps.keys().map(String::as_str).collect();
    assert_eq!(kept, ["mesh", "prepare"]);
    assert_eq!(saved.status(WizardStep::Mesh), StepStatus::Skipped);
    assert_eq!(saved.status(WizardStep::Views), StepStatus::NotRun);
    assert_eq!(saved.resume_step(), WizardStep::Views);
}

#[test]
fn a_project_resumes_on_its_next_step_and_finds_a_changed_scan() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let (mut first, scan) = studio_with_building(directory.path());
    let folder = directory.path().join("Office");
    first.mesh_to_plans.project_folder = folder.display().to_string();
    prepare_and_save(&mut first);
    let _ = first.update(wizard(WizardAction::Prepare(PrepareAction::Select(1))));
    let _ = first.update(wizard(WizardAction::Prepare(PrepareAction::Name(
        "First floor".into(),
    ))));
    let _ = first.update(wizard(WizardAction::Confirm));
    let revision = first.mesh_to_plans.save_revision;
    let _ = first.update(wizard(WizardAction::Save(revision)));
    let file = folder.join(project::FILE_NAME);

    // Another window with the same scan open offers to go on with it.
    let path = scan.clone();
    let cloud = std::sync::Arc::new(pointcloud_core::open(&path, 1_000_000).unwrap());
    let mut second = Studio::default();
    let _ = second.update(Message::Loaded(Ok(cloud)));
    assert!(
        second.mesh_to_plans_browser().is_none(),
        "no recent project"
    );
    second.mesh_to_plans.recent = vec![file.clone()];
    second.mesh_to_plans.recent_projects = project::read_recent(std::slice::from_ref(&file));
    assert_eq!(
        second.mesh_to_plans.recent_projects[0].step,
        WizardStep::Mesh
    );
    assert!(second.mesh_to_plans_browser().is_some());
    let _ = second.view();
    let _ = second.update(wizard(WizardAction::Resume(file.clone())));
    let wizard_state = &second.mesh_to_plans;
    assert!(wizard_state.covers_model());
    assert_eq!(wizard_state.step, WizardStep::Mesh);
    assert_eq!(
        *wizard_state.status(WizardStep::Prepare),
        StepStatus::Confirmed
    );
    assert_eq!(
        wizard_state.prepare.levels,
        first.mesh_to_plans.prepare.levels
    );
    assert_eq!(wizard_state.prepare.levels[1].name, "First floor");
    assert!(wizard_state.prepare.top.is_some(), "the view from above");
    assert_eq!(wizard_state.project_name, "building");
    assert_eq!(
        second.status,
        "Pointcloud to Drawing project building opened"
    );

    // The scan opened by a path typed with other separators is the same
    // scan: offered, and step 0 is as it was.
    let mut text = std::fs::read_to_string(&file).unwrap();
    let typed = scan.display().to_string();
    text = text.replace(
        &serde_json::to_string(&typed).unwrap(),
        &serde_json::to_string(&typed.replace('\\', "/")).unwrap(),
    );
    let retyped = directory.path().join("Retyped").join(project::FILE_NAME);
    std::fs::create_dir_all(retyped.parent().unwrap()).unwrap();
    std::fs::write(&retyped, text).unwrap();
    let mut retyped_studio = Studio::default();
    let cloud = std::sync::Arc::new(pointcloud_core::open(&path, 1_000_000).unwrap());
    let _ = retyped_studio.update(Message::Loaded(Ok(cloud)));
    retyped_studio.mesh_to_plans.recent_projects =
        project::read_recent(std::slice::from_ref(&retyped));
    assert!(retyped_studio.mesh_to_plans_browser().is_some());
    let _ = retyped_studio.update(wizard(WizardAction::Resume(retyped.clone())));
    assert_eq!(
        *retyped_studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Confirmed
    );
    let _ = second.update(wizard(WizardAction::Step(WizardStep::Prepare)));
    let _ = second.view();

    // The scan changed since: step 0 is out of date and the wizard shows it.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&scan)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let mut third = Studio::default();
    let cloud = std::sync::Arc::new(pointcloud_core::open(&path, 1_000_000).unwrap());
    let _ = third.update(Message::Loaded(Ok(cloud)));
    // Through the local API, by the folder of the project.
    let answer = send(
        &mut third,
        command(json!({"command": "mesh_to_plans_action", "action": "resume", "folder": folder})),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["mesh_to_plans"]["steps"][0]["status"], "stale");
    assert_eq!(
        *third.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Stale
    );
    assert_eq!(third.mesh_to_plans.step, WizardStep::Prepare);
    assert_eq!(
        third.mesh_to_plans.step_ready().unwrap_err().english(),
        "The scans or the choices changed since this step ran: run it again"
    );
    // A file that is no project is refused and changes nothing.
    let other = directory.path().join("other.json");
    std::fs::write(&other, "{}").unwrap();
    let answer = send(
        &mut third,
        command(json!({"command": "mesh_to_plans_action", "action": "resume", "folder": other})),
    );
    assert_eq!(answer["ok"], false);
    assert!(answer["error"]
        .as_str()
        .unwrap()
        .starts_with("Could not open the project: "));
    assert_eq!(third.mesh_to_plans.project.as_ref().unwrap().file, file);
    let answer = send(
        &mut third,
        command(json!({"command": "mesh_to_plans_action", "action": "resume"})),
    );
    assert_eq!(answer["error"], "resume needs the folder of the project");
}

fn prepare_status(studio: &Studio) -> StepStatus {
    studio.mesh_to_plans.status(WizardStep::Prepare).clone()
}

#[test]
fn step_0_is_out_of_date_as_soon_as_the_scans_or_the_choices_change() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_building(directory.path());
    let folder = directory.path().join("Office");
    studio.mesh_to_plans.project_folder = folder.display().to_string();
    let _ = studio.update(wizard(WizardAction::Open));
    prepare_and_save(&mut studio);
    let _ = studio.update(wizard(WizardAction::Confirm));
    assert_eq!(prepare_status(&studio), StepStatus::Confirmed);
    assert!(studio.mesh_to_plans.step_ready().is_ok());

    // A main direction typed after the run: out of date, Next waits, and
    // the confirmed levels stay locked.
    act(&mut studio, PrepareAction::Rotation("20".into()));
    assert_eq!(prepare_status(&studio), StepStatus::Stale);
    assert_eq!(status(&mut studio)["steps"][0]["status"], "stale");
    assert_eq!(
        studio.mesh_to_plans.step_ready().unwrap_err().english(),
        "The scans or the choices changed since this step ran: run it again"
    );
    act(&mut studio, PrepareAction::Remove);
    assert_eq!(studio.status, "Edit levels first: they are confirmed");
    let _ = studio.view();
    // The file keeps the step as it was with the basis it ran on, so that
    // it is out of date when the project is opened again.
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
    let saved = project::load(&folder.join(project::FILE_NAME)).unwrap();
    assert_eq!(saved.status(WizardStep::Prepare), StepStatus::Confirmed);
    assert_eq!(saved.regions.chosen_rotation, Some(20.0));
    // Taken back, the step is as it was.
    act(&mut studio, PrepareAction::Rotation(String::new()));
    assert_eq!(prepare_status(&studio), StepStatus::Confirmed);

    // The scan moved, and back.
    studio.clouds[0].transform.offset[0] += 0.5;
    let _ = studio.update(wizard(WizardAction::Poll));
    assert_eq!(prepare_status(&studio), StepStatus::Stale);
    studio.clouds[0].transform.offset[0] -= 0.5;
    let _ = studio.update(wizard(WizardAction::Poll));
    assert_eq!(prepare_status(&studio), StepStatus::Confirmed);
    // A deletion that was undone deletes nothing.
    let total = studio.clouds[0].cloud.total_points;
    studio.clouds[0].deleted = Some(std::sync::Arc::new(
        crate::selection::DeletionMask::new(total).unwrap(),
    ));
    let _ = studio.update(wizard(WizardAction::Poll));
    assert_eq!(prepare_status(&studio), StepStatus::Confirmed);
    // The core read from the section box.
    act(&mut studio, PrepareAction::SectionToBuilding);
    act(&mut studio, PrepareAction::CoreFromSection);
    assert_eq!(prepare_status(&studio), StepStatus::Stale);

    // Edit levels while out of date: the step was waiting for confirmation
    // under it, and it is that once the whole scan is read again.
    act(&mut studio, PrepareAction::Unlock);
    assert_eq!(prepare_status(&studio), StepStatus::Stale);
    act(&mut studio, PrepareAction::Select(1));
    act(&mut studio, PrepareAction::Name("Office floor".into()));
    assert_eq!(studio.mesh_to_plans.prepare.levels[1].name, "Office floor");
    act(&mut studio, PrepareAction::CoreWhole);
    assert_eq!(prepare_status(&studio), StepStatus::Done);
}

#[test]
fn a_run_again_keeps_the_levels_the_user_changed() {
    use pointcloud_core::plans::LevelStatus;
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_building(directory.path());
    studio.mesh_to_plans.project_folder = directory.path().join("Office").display().to_string();
    prepare_and_save(&mut studio);
    let floor = studio.mesh_to_plans.prepare.levels[1].floor_z;
    act(&mut studio, PrepareAction::Select(1));
    act(&mut studio, PrepareAction::Name("Office floor".into()));
    act(&mut studio, PrepareAction::Cut("1,35".into()));
    act(&mut studio, PrepareAction::Move(1, floor + 0.1));
    // The main direction typed, and the step run again.
    act(&mut studio, PrepareAction::Rotation("25".into()));
    assert_eq!(prepare_status(&studio), StepStatus::Stale);
    prepare_and_save(&mut studio);
    assert_eq!(prepare_status(&studio), StepStatus::Done);
    let levels = &studio.mesh_to_plans.prepare.levels;
    let found: Vec<(&str, &str, LevelStatus)> = levels
        .iter()
        .map(|level| (level.id.as_str(), level.name.as_str(), level.status))
        .collect();
    assert_eq!(
        found,
        [
            ("00", "Ground floor", LevelStatus::Found),
            ("01", "Office floor", LevelStatus::Edited),
            ("R", "Roof", LevelStatus::Found),
        ]
    );
    assert!((levels[1].floor_z - (floor + 0.1)).abs() < 1e-9);
    assert_eq!(levels[1].cut_height, 1.35);
    assert!(levels[0].is_peil);
}

/// Three levels as step 0 finds them, on a survey whose grid runs from 1 m
/// below P to 11 m above.
fn found_levels() -> (project::SurveyRecord, Vec<pointcloud_core::plans::Level>) {
    use pointcloud_core::plans::{BuildingFrame, Confidence, LevelKind, LevelStatus, SurveyGrid};
    let level = |id: &str, kind: LevelKind, floor_z: f64| pointcloud_core::plans::Level {
        id: id.into(),
        name: id.into(),
        kind,
        floor_z,
        ceiling_z: (kind != LevelKind::Roof).then_some(floor_z + 2.75),
        slab_underside: None,
        slab_thickness: (kind != LevelKind::Roof).then_some(0.25),
        cut_height: 1.2,
        tilt_mm_per_m: Some([0.0, 0.0]),
        share: 0.8,
        is_peil: floor_z == 0.0,
        confidence: Confidence::certain(),
        status: LevelStatus::Found,
    };
    let mut levels = vec![
        level("00", LevelKind::Ground, 0.0),
        level("01", LevelKind::Storey, 3.0),
        level("R", LevelKind::Roof, 6.0),
    ];
    for level in &mut levels {
        level.name = prepare::default_name(level);
    }
    let survey = project::SurveyRecord {
        grid: SurveyGrid {
            origin: [0.0, 0.0, -1.0],
            cell_xy: 0.05,
            cell_z: 0.02,
            size: [100, 100, 600],
        },
        frame: BuildingFrame::new(0.0, [0.0, 0.0]),
        level_histogram: (0..600)
            .map(|bin| if bin % 150 == 50 { 3000 } else { 40 })
            .collect(),
        footprint: Vec::new(),
        footprint_area: 20.0,
        ground_z: Some(-0.3),
        below_z: Some(-2.0),
        below_points: 0,
        below_clusters: 0,
        stats: pointcloud_core::plans::SurveyStats::default(),
        seconds: 1.0,
    };
    (survey, levels)
}

#[test]
fn a_level_line_snaps_to_five_centimetres_and_moves_freely_with_shift() {
    use iced::mouse::{Button, Cursor, Event as MouseEvent};
    use iced::widget::canvas::{self, Program};
    use iced::{keyboard, Point};
    let (survey, levels) = found_levels();
    let program = prepare::Histogram {
        survey: &survey,
        levels: &levels,
        selected: None,
        peil: 0.0,
        locked: false,
    };
    let size = Size::new(300.0, 600.0);
    let bounds = Rectangle::new(Point::ORIGIN, size);
    let at = |z: f64| Cursor::Available(Point::new(120.0, program.screen_y(size, z)));
    let press = canvas::Event::Mouse(MouseEvent::ButtonPressed(Button::Left));
    let moved = canvas::Event::Mouse(MouseEvent::CursorMoved {
        position: Point::ORIGIN,
    });
    let release = canvas::Event::Mouse(MouseEvent::ButtonReleased(Button::Left));
    let place_of = |message: Option<Message>| match message {
        Some(Message::MeshToPlans(WizardAction::Prepare(PrepareAction::Move(place, z)))) => {
            Some((place, z))
        }
        _ => None,
    };

    // Taken at its line, the first floor follows the pointer and lands on a
    // height of 5 cm steps above P.
    let mut state = prepare::HistogramState::default();
    let (_, selected) = program.update(&mut state, press.clone(), bounds, at(3.0));
    assert!(matches!(
        selected,
        Some(Message::MeshToPlans(WizardAction::Prepare(
            PrepareAction::Select(1)
        )))
    ));
    let _ = program.update(&mut state, moved.clone(), bounds, at(3.137));
    let (_, message) = program.update(&mut state, release.clone(), bounds, at(3.137));
    let (place, z) = place_of(message).expect("the line moved");
    assert_eq!(place, 1);
    assert!((z - 3.15).abs() < 1e-9, "{z}");
    assert_eq!(state.drag, None);

    // With Shift it lands where it was let go, to the millimetre.
    let shift = canvas::Event::Keyboard(keyboard::Event::ModifiersChanged(
        keyboard::Modifiers::SHIFT,
    ));
    let _ = program.update(&mut state, shift, bounds, Cursor::Unavailable);
    let _ = program.update(&mut state, press.clone(), bounds, at(3.0));
    let _ = program.update(&mut state, moved.clone(), bounds, at(3.137));
    let (_, message) = program.update(&mut state, release.clone(), bounds, at(3.137));
    let (_, z) = place_of(message).unwrap();
    let pixel = 1.0 / (program.screen_y(size, 0.0) - program.screen_y(size, 1.0)) as f64;
    assert!((z - 3.137).abs() <= pixel + 1e-3, "{z}");
    assert_eq!(z, (z * 1000.0).round() / 1000.0);

    // A click that does not move the line leaves it, and away from every
    // line the pointer takes none.
    let _ = program.update(&mut state, press.clone(), bounds, at(6.0));
    let (_, message) = program.update(&mut state, release.clone(), bounds, at(6.0));
    assert!(message.is_none());
    let (status, message) = program.update(&mut state, press.clone(), bounds, at(1.5));
    assert!(message.is_none() && status == canvas::event::Status::Ignored);

    // Confirmed levels are only selected.
    let locked = prepare::Histogram {
        locked: true,
        ..program
    };
    let mut state = prepare::HistogramState::default();
    let (_, message) = locked.update(&mut state, press, bounds, at(3.0));
    assert!(message.is_some());
    assert_eq!(state.drag, None);
    let _ = locked.update(&mut state, moved, bounds, at(3.5));
    let (_, message) = locked.update(&mut state, release, bounds, at(3.5));
    assert!(message.is_none());
    assert!((prepare::snap(3.137, 0.02, false) - 3.12).abs() < 1e-9);
    assert!(prepare::snap(-0.024, 0.0, false).abs() < 1e-9);
    assert!((prepare::snap(3.1374, 0.0, true) - 3.137).abs() < 1e-9);
}

/// A window with three levels in step 0, as a run would leave them.
fn studio_with_levels() -> Studio {
    let mut studio = Studio::default();
    let (survey, levels) = found_levels();
    let prepare = &mut studio.mesh_to_plans.prepare;
    prepare.survey = Some(survey);
    prepare.levels = levels;
    prepare.select(Some(0));
    studio
        .mesh_to_plans
        .set_status(WizardStep::Prepare, StepStatus::Done);
    studio
}

fn act(studio: &mut Studio, action: PrepareAction) {
    let _ = studio.update(wizard(WizardAction::Prepare(action)));
}

fn codes(studio: &Studio) -> Vec<(String, String, bool)> {
    studio
        .mesh_to_plans
        .prepare
        .levels
        .iter()
        .map(|level| (level.id.clone(), level.name.clone(), level.is_peil))
        .collect()
}

#[test]
fn levels_are_moved_named_added_merged_removed_and_renumbered_from_p() {
    use pointcloud_core::plans::LevelStatus;
    let _language = TestLanguage::hold(Language::English);
    let mut studio = studio_with_levels();
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.view();

    // The first floor dragged 20 cm up takes its ceiling along.
    act(&mut studio, PrepareAction::Move(1, 3.2));
    let prepare = &studio.mesh_to_plans.prepare;
    assert_eq!(prepare.levels[1].floor_z, 3.2);
    assert_eq!(prepare.levels[1].ceiling_z, Some(3.2 + 2.75));
    assert_eq!(prepare.levels[1].status, LevelStatus::Edited);
    assert_eq!(prepare.selected, Some(1));

    // P on the first floor makes the ground floor a basement.
    act(&mut studio, PrepareAction::SetPeil);
    assert_eq!(
        codes(&studio),
        [
            ("-01".into(), "Basement 1".into(), false),
            ("00".into(), "Ground floor".into(), true),
            ("R".into(), "Roof".into(), false),
        ]
    );
    assert!((studio.mesh_to_plans.prepare.peil_z() - 3.2).abs() < 1e-12);
    // A name given by hand stays when the codes change.
    act(&mut studio, PrepareAction::Select(0));
    act(&mut studio, PrepareAction::Name("Cellar".into()));
    act(&mut studio, PrepareAction::Select(1));
    act(&mut studio, PrepareAction::Add);
    assert_eq!(
        codes(&studio),
        [
            ("-01".into(), "Cellar".into(), false),
            ("00".into(), "Ground floor".into(), true),
            ("01".into(), "Floor 1".into(), false),
            ("R".into(), "Roof".into(), false),
        ]
    );
    let added = &studio.mesh_to_plans.prepare.levels[2];
    assert!((added.floor_z - 6.2).abs() < 1e-12);
    assert_eq!(studio.mesh_to_plans.prepare.selected, Some(2));

    // The cut height of the selected level, typed with a comma.
    act(&mut studio, PrepareAction::Cut("1,35".into()));
    assert_eq!(studio.mesh_to_plans.prepare.levels[2].cut_height, 1.35);
    act(&mut studio, PrepareAction::Cut("9".into()));
    assert_eq!(studio.mesh_to_plans.prepare.levels[2].cut_height, 1.35);

    // Merged with the roof above it, the new floor takes in the roof.
    act(&mut studio, PrepareAction::Merge);
    assert_eq!(studio.mesh_to_plans.prepare.levels.len(), 3);
    act(&mut studio, PrepareAction::Select(0));
    act(&mut studio, PrepareAction::Remove);
    assert_eq!(
        codes(&studio),
        [
            ("00".into(), "Ground floor".into(), true),
            ("01".into(), "Floor 1".into(), false),
        ]
    );
    // P moves up a floor, and the names given by default follow.
    act(&mut studio, PrepareAction::Select(1));
    act(&mut studio, PrepareAction::SetPeil);
    assert_eq!(
        codes(&studio),
        [
            ("-01".into(), "Basement 1".into(), false),
            ("00".into(), "Ground floor".into(), true),
        ]
    );
    let _ = studio.view();
}

#[test]
fn slabs_follow_the_floors_when_levels_move_come_and_go() {
    let mut studio = studio_with_levels();
    let slabs = |studio: &Studio| -> Vec<Option<f64>> {
        studio
            .mesh_to_plans
            .prepare
            .levels
            .iter()
            .map(|level| {
                level
                    .slab_thickness
                    .map(|thickness| (thickness * 1000.0).round())
            })
            .collect()
    };
    assert_eq!(slabs(&studio), [Some(250.0), Some(250.0), None]);
    // The first floor 20 cm up: the slab below it is 20 cm thicker, and its
    // ceiling, which went along, lies 5 cm under the roof.
    act(&mut studio, PrepareAction::Move(1, 3.2));
    assert_eq!(slabs(&studio), [Some(450.0), None, None]);
    act(&mut studio, PrepareAction::Move(1, 3.0));
    assert_eq!(slabs(&studio), [Some(250.0), Some(250.0), None]);
    // Without the first floor the ground floor carries no slab under the
    // roof; a level added above it has no ceiling.
    act(&mut studio, PrepareAction::Select(1));
    act(&mut studio, PrepareAction::Remove);
    assert_eq!(slabs(&studio), [None, None]);
    act(&mut studio, PrepareAction::Select(0));
    act(&mut studio, PrepareAction::Add);
    let levels = &studio.mesh_to_plans.prepare.levels;
    assert!((levels[1].floor_z - 3.0).abs() < 1e-12);
    assert_eq!(slabs(&studio), [Some(250.0), None, None]);
}

#[test]
fn show_in_model_puts_the_section_box_on_a_storey_and_back_to_wizard_takes_it_away() {
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_building(directory.path());
    studio.mesh_to_plans.project_folder = directory.path().join("Office").display().to_string();
    let _ = studio.update(wizard(WizardAction::Open));
    prepare_and_save(&mut studio);
    assert!(studio.section_box().is_none());
    act(&mut studio, PrepareAction::Show(1));
    assert!(studio.mesh_to_plans.is_open() && !studio.mesh_to_plans.covers_model());
    let section = studio.section_box().expect("the box is on");
    let frame = studio.mesh_to_plans.prepare.frame().unwrap();
    assert!(
        (section.rotation_degrees - frame.rotation_deg).abs() < 1e-6,
        "{section:?}"
    );
    let floor = studio.mesh_to_plans.prepare.levels[1].floor_z;
    assert!(
        (section.bounds.min[2] - (floor - 0.1)).abs() < 0.01,
        "{section:?}"
    );
    // From the Drawing view the storey is shown in the model.
    let _ = studio.update(wizard(WizardAction::Restore));
    let _ = studio.update(Message::DrawingView(drawing_view::DrawingViewAction::Show(
        true,
    )));
    act(&mut studio, PrepareAction::Show(1));
    assert!(!studio.drawing_view.shown);
    assert_eq!(studio.section_box(), Some(section));
    assert_eq!(studio.mesh_to_plans.prepare.selected, Some(1));
    let _ = studio.view();
    let _ = studio.update(wizard(WizardAction::Restore));
    assert!(studio.mesh_to_plans.covers_model());
    assert!(studio.section_box().is_none(), "the box was off before");

    // The section box around the building, and the core from it.
    act(&mut studio, PrepareAction::SectionToBuilding);
    let building = studio.section_box().expect("the box is on");
    assert!(building.contains(frame.to_scene([1.0, 1.0, 1.0])));
    act(&mut studio, PrepareAction::CoreFromSection);
    assert!(studio.mesh_to_plans.prepare.regions.chosen_core.is_some());
    act(&mut studio, PrepareAction::CoreWhole);
    assert!(studio.mesh_to_plans.prepare.regions.chosen_core.is_none());

    // The card opened again from the strip puts the box back as well, here
    // around the building; closed from the strip, the box stays.
    act(&mut studio, PrepareAction::Show(0));
    assert!(!studio.mesh_to_plans.covers_model());
    assert_ne!(studio.section_box(), Some(building));
    let _ = studio.update(wizard(WizardAction::Open));
    assert!(studio.mesh_to_plans.covers_model());
    let back = studio.section_box().expect("the box is on");
    let corners = back.corners().into_iter().zip(building.corners());
    for (corner, before) in corners {
        let apart = (0..3).map(|axis| (corner[axis] - before[axis]).abs());
        assert!(apart.fold(0.0, f64::max) < 1e-9, "{back:?}");
    }
    act(&mut studio, PrepareAction::Show(1));
    let storey = studio.section_box();
    let _ = studio.update(wizard(WizardAction::Close));
    assert_eq!(studio.section_box(), storey);
    assert_eq!(studio.mesh_to_plans.prepare.put_back, None);
}

/// Whether a task holds work for the runtime.
fn has_work(task: Task<Message>) -> bool {
    iced_runtime::task::into_stream(task).is_some()
}

#[test]
fn opened_from_the_strip_through_the_api_the_model_reads_its_detail_again() {
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_building(directory.path());
    studio.mesh_to_plans.project_folder = directory.path().join("Office").display().to_string();
    let _ = studio.update(wizard(WizardAction::Open));
    prepare_and_save(&mut studio);
    // The model reads its detail from the index of a scan.
    let index = pointcloud_core::OctreeIndex::build(
        &studio.clouds[0].cloud,
        pointcloud_core::IndexConfig::default(),
    )
    .unwrap();
    studio.clouds[0].index = Some(std::sync::Arc::new(index));
    act(&mut studio, PrepareAction::Show(1));
    assert!(studio.section_box().is_some() && !studio.mesh_to_plans.covers_model());
    // The section box goes back, off here, and the task that reads the
    // detail of the model for it reaches the runtime.
    let (reply, answer) = std::sync::mpsc::channel();
    let task = studio.handle_api(crate::native_api::ApiRequest {
        command: command(json!({"command": "mesh_to_plans_view", "open": true})),
        reply,
    });
    assert_eq!(answer.recv().unwrap()["ok"], true);
    assert!(studio.section_box().is_none());
    assert!(has_work(task));
}

#[test]
fn api_runs_and_confirms_steps_and_refuses_what_cannot_be_done() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    for (body, error) in [
        (
            json!({"command": "mesh_to_plans_action", "action": "fly"}),
            "unknown action; use run, run_all, confirm, skip, cancel, back, next, resume",
        ),
        (
            json!({"command": "mesh_to_plans_action", "action": "confirm"}),
            "the step shown waits for no confirmation",
        ),
        (
            json!({"command": "mesh_to_plans_action", "action": "skip"}),
            "the step shown cannot be skipped",
        ),
        (
            json!({"command": "mesh_to_plans_action", "action": "cancel"}),
            "no job is running",
        ),
        (
            json!({"command": "mesh_to_plans_action", "action": "back"}),
            "this is the first step",
        ),
        (
            json!({"command": "mesh_to_plans_action", "action": "next"}),
            "Run this step first",
        ),
        (
            json!({"command": "mesh_to_plans_action", "action": "run", "folder": "relative"}),
            "folder must be an absolute path",
        ),
        (
            json!({"command": "mesh_to_plans_action", "action": "run"}),
            "Open a scan of the building first",
        ),
    ] {
        let answer = send(&mut studio, command(body));
        assert_eq!(answer["ok"], false, "{answer}");
        assert_eq!(answer["error"], error);
    }
    assert_eq!(
        *studio.mesh_to_plans.status(WizardStep::Prepare),
        StepStatus::Failed("Open a scan of the building first".into())
    );

    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_building(directory.path());
    let folder = directory.path().join("Api");
    let answer = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_action", "action": "run", "folder": folder})),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert!(answer["job_id"].is_string());
    assert_eq!(
        answer["mesh_to_plans"]["project_folder"],
        folder.display().to_string()
    );
    finish(&mut studio);
    let answer = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_action", "action": "confirm"})),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    assert_eq!(answer["mesh_to_plans"]["steps"][0]["status"], "confirmed");
    let answer = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_action", "action": "next"})),
    );
    assert_eq!(answer["mesh_to_plans"]["step"], "mesh");
    let answer = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_action", "action": "skip"})),
    );
    assert_eq!(answer["mesh_to_plans"]["steps"][1]["status"], "skipped");
    let revision = studio.mesh_to_plans.save_revision;
    let _ = studio.update(wizard(WizardAction::Save(revision)));
    assert!(folder.join(project::FILE_NAME).is_file());
    let answer = send(
        &mut studio,
        command(json!({"command": "mesh_to_plans_action", "action": "run", "folder": folder})),
    );
    assert_eq!(answer["error"], "the project already has its folder");
}

#[test]
fn api_selects_renames_moves_and_numbers_the_levels_of_step_0() {
    let _language = TestLanguage::hold(Language::English);
    let mut empty = Studio::default();
    let answer = send(
        &mut empty,
        command(json!({"command": "mesh_to_plans_level", "level": "00"})),
    );
    assert_eq!(answer["error"], "there are no levels yet: run step 0 first");

    let mut studio = studio_with_levels();
    let level = |body: Value| {
        let mut body = body;
        body["command"] = json!("mesh_to_plans_level");
        command(body)
    };
    for (body, error) in [
        (
            json!({"level": "07"}),
            "no level 07; the levels are 00, 01, R",
        ),
        (json!({"action": "select"}), "level is required"),
        (
            json!({"level": "01", "action": "jump"}),
            "unknown action; use select, show, set_peil, add, merge, remove",
        ),
        (
            json!({"level": "01", "cut_height": 4.0}),
            "cut_height must lie between 0.3 and 3.0 m",
        ),
        (
            json!({"level": "01", "name": " "}),
            "name must not be empty",
        ),
        (
            json!({"level": "R", "action": "set_peil"}),
            "only a whole floor can be P",
        ),
        (
            json!({"level": "R", "action": "merge"}),
            "the level has no level above it to merge with",
        ),
    ] {
        let answer = send(&mut studio, level(body));
        assert_eq!(answer["ok"], false, "{answer}");
        assert_eq!(answer["error"], error);
    }

    // The first floor renamed, cut higher and moved 20 cm up.
    let answer = send(
        &mut studio,
        level(
            json!({"level": "01", "name": "Office floor", "cut_height": 1.35, "floor_above_p": 3.2}),
        ),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let levels = &answer["mesh_to_plans"]["prepare"]["levels"];
    assert_eq!(levels[1]["name"], "Office floor");
    assert_eq!(levels[1]["cut_height"], 1.35);
    assert!((levels[1]["floor_above_p"].as_f64().unwrap() - 3.2).abs() < 1e-9);
    assert_eq!(levels[1]["status"], "Edited");
    assert_eq!(answer["mesh_to_plans"]["prepare"]["selected"], 1);

    // P on the first floor; a level added above it; the ground floor gone.
    let answer = send(
        &mut studio,
        level(json!({"level": "01", "action": "set_peil"})),
    );
    let ids: Vec<&str> = answer["mesh_to_plans"]["prepare"]["levels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|level| level["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["-01", "00", "R"]);
    let answer = send(&mut studio, level(json!({"level": "00", "action": "add"})));
    assert_eq!(answer["mesh_to_plans"]["prepare"]["levels"][2]["id"], "01");
    let answer = send(
        &mut studio,
        level(json!({"level": "-01", "action": "remove"})),
    );
    let levels = answer["mesh_to_plans"]["prepare"]["levels"]
        .as_array()
        .unwrap();
    assert_eq!(levels.len(), 3);
    assert_eq!(levels[0]["name"], "Office floor");
    assert_eq!(levels[0]["is_peil"], true);
    let answer = send(
        &mut studio,
        level(json!({"level": "01", "action": "merge"})),
    );
    assert_eq!(
        answer["mesh_to_plans"]["prepare"]["levels"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    // Confirmed levels are only selected.
    let _ = studio.update(wizard(WizardAction::Confirm));
    let answer = send(&mut studio, level(json!({"level": "01", "name": "Top"})));
    assert_eq!(
        answer["error"],
        "the levels are confirmed: use Edit levels first"
    );
    let answer = send(&mut studio, level(json!({"level": "00"})));
    assert_eq!(answer["ok"], true);
    assert_eq!(answer["mesh_to_plans"]["prepare"]["selected"], 0);
}
