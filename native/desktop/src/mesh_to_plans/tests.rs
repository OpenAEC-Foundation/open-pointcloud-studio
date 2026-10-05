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
    assert_eq!(studio.status, "Mesh to Plans: 0 Preparation…");
    assert_eq!(
        studio.mesh_to_plans.step_ready().unwrap_err().english(),
        "Wait until this step has finished"
    );
    let lines = studio.progress_lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].phase, crate::open_progress::Phase::MeshToPlans);
    assert_eq!(lines[0].title, "Mesh to Plans");
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
    assert_eq!(studio.status, "Mesh to Plans is already running a step");
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
    assert!(studio.status.starts_with("Mesh to Plans: 1 step done in "));
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
    assert!(studio.status.starts_with("Mesh to Plans: 8 steps done in "));
    let _ = studio.view();

    let _ = studio.update(wizard(WizardAction::RunAll));
    assert!(!studio.mesh_to_plans.is_running());
    assert_eq!(
        studio.status,
        "Every step of Mesh to Plans is confirmed or skipped"
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
        assert_eq!(studio.status, "Cancelling Mesh to Plans…");
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
    assert!(studio.status.starts_with("Mesh to Plans cancelled after "));
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
        "Mesh to Plans failed in 2 Sections, elevations and raw plans: disk full"
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
    let mut studio = Studio {
        index_pending: true,
        ..Studio::default()
    };
    let _ = studio.update(wizard(WizardAction::Run));
    assert!(!studio.mesh_to_plans.is_running());
    assert_eq!(
        studio.status,
        "Mesh to Plans waits: an octree is being made; wait for it or cancel it first"
    );
}
