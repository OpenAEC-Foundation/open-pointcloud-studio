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
fn escape_closes_settings_first_and_then_the_card() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    let _ = studio.update(wizard(WizardAction::Open));
    let _ = studio.update(Message::Settings(SettingsAction::Open));
    assert!(studio.settings.is_some());
    let _ = studio.view();

    let _ = studio.update(Message::Escape);
    assert!(studio.settings.is_none());
    assert!(studio.mesh_to_plans.is_open(), "the card waits its turn");

    let _ = studio.update(Message::Escape);
    assert!(!studio.mesh_to_plans.is_open());
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
