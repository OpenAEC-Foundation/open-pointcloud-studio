//! Tests of the card of Mesh Pointcloud on a generated room: its steps, the
//! options each method keeps, a job that goes on while the card is closed,
//! the save dialog of a terrain mesh, what the methods work on, and the
//! command of the local API.

use std::path::Path;
use std::sync::Arc;

use pointcloud_core::MeshGeometry;
use serde_json::{json, Value};

use super::*;
use crate::i18n::{Language, TestLanguage};
use crate::mesh_export::MeasuredMesh;
use crate::native_api::{ApiCommand, ApiRequest};

/// The inside of a room of 1.6 by 1.2 m and 1.0 m high, one point every two
/// centimetres on each of its six faces.
fn room_points() -> Vec<[f64; 3]> {
    const ROOM: [f64; 3] = [1.6, 1.2, 1.0];
    const STEP: f64 = 0.02;
    let along = |length: f64| {
        let count = (length / STEP).round() as usize;
        (0..count).map(move |step| (step as f64 + 0.5) * STEP)
    };
    let mut points = Vec::new();
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for side in [0.0, ROOM[axis]] {
            for a in along(ROOM[u]) {
                for b in along(ROOM[v]) {
                    let mut point = [0.0; 3];
                    point[axis] = side;
                    point[u] = a;
                    point[v] = b;
                    points.push(point);
                }
            }
        }
    }
    points
}

/// A window with the room open as its one layer.
fn studio_with_room(directory: &Path) -> Studio {
    let path = directory.join("room.xyz");
    let text: String = room_points()
        .iter()
        .map(|[x, y, z]| format!("{x:.4} {y:.4} {z:.4}\n"))
        .collect();
    std::fs::write(&path, text).unwrap();
    let cloud = Arc::new(pointcloud_core::open(&path, 2_000).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    studio
}

fn act(studio: &mut Studio, action: MeshWizardAction) {
    let _ = studio.update(Message::MeshWizard(action));
}

/// Send a command the way the window receives it.
fn send(studio: &mut Studio, body: Value) -> Value {
    let command: ApiCommand = serde_json::from_value(body).unwrap();
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.update(Message::ApiRequest(ApiRequest { command, reply }));
    receive.recv().unwrap()
}

fn wizard(studio: &mut Studio) -> Value {
    send(studio, json!({"command": "status"}))["result"]["mesh_wizard"].clone()
}

/// Every step of every method, built in English and in Dutch.
fn view_everything(studio: &mut Studio) {
    let (step, method) = (studio.mesh_wizard.step, studio.mesh_wizard.method);
    for language in [Language::English, Language::Table(0)] {
        let _language = TestLanguage::hold(language);
        for each in MeshMethod::ALL {
            for shown in WizardStep::ALL {
                studio.mesh_wizard.method = each;
                studio.mesh_wizard.step = shown;
                let _ = studio.view();
            }
        }
    }
    studio.mesh_wizard.step = step;
    studio.mesh_wizard.method = method;
}

#[test]
fn the_card_goes_through_its_steps_and_keeps_its_place() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    assert!(!studio.mesh_wizard.is_open());
    act(&mut studio, MeshWizardAction::Open);
    let wizard_now = &studio.mesh_wizard;
    assert!(wizard_now.is_open());
    assert_eq!(
        (wizard_now.step, wizard_now.method),
        (WizardStep::Method, MeshMethod::Closed)
    );
    // The card lies over the model.
    assert!(studio.model_covered());

    // Next and Back walk the three steps and stop at both ends.
    let mut seen = Vec::new();
    for _ in 0..3 {
        act(&mut studio, MeshWizardAction::Next);
        seen.push(studio.mesh_wizard.step);
    }
    assert_eq!(
        seen,
        [WizardStep::Options, WizardStep::Run, WizardStep::Run]
    );
    seen.clear();
    for _ in 0..3 {
        act(&mut studio, MeshWizardAction::Back);
        seen.push(studio.mesh_wizard.step);
    }
    assert_eq!(
        seen,
        [WizardStep::Options, WizardStep::Method, WizardStep::Method]
    );

    // Closed and opened again, the card shows the step and the method it
    // showed, as nothing runs.
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Surface));
    act(&mut studio, MeshWizardAction::Step(WizardStep::Options));
    act(&mut studio, MeshWizardAction::Close);
    assert!(!studio.mesh_wizard.is_open() && !studio.model_covered());
    act(&mut studio, MeshWizardAction::Open);
    assert_eq!(
        (studio.mesh_wizard.step, studio.mesh_wizard.method),
        (WizardStep::Options, MeshMethod::Surface)
    );
    view_everything(&mut studio);

    // Escape closes the card; the next Escape goes on to the model.
    let _ = studio.update(Message::Escape);
    assert!(!studio.mesh_wizard.is_open());
    // The File view steps aside for the card.
    let _ = studio.update(Message::ToggleFile);
    assert!(studio.file_open);
    act(&mut studio, MeshWizardAction::Open);
    assert!(!studio.file_open && studio.mesh_wizard.is_open());
}

#[test]
fn each_method_keeps_its_options_and_the_preset_resets_only_its_own() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    act(&mut studio, MeshWizardAction::Open);
    act(&mut studio, MeshWizardAction::Next);
    assert_eq!(studio.mesh_wizard.step, WizardStep::Options);
    for method in MeshMethod::ALL {
        act(&mut studio, MeshWizardAction::Method(method));
        assert_eq!(wizard(&mut studio)["recommended"], true, "{method:?}");
    }

    // Each method gets settings of its own.
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Closed));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Voxel("0.04".into())));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::MaxHole("0.1".into())));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Layers(
        Layers::Visible,
    )));
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Faces));
    let _ = studio.update(Message::Faces(FaceAction::Distance("15".into())));
    let _ = studio.update(Message::Faces(FaceAction::Cylinders(false)));
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Surface));
    let _ = studio.update(Message::SurfaceSetting(0, "20000".into()));
    assert_eq!(wizard(&mut studio)["recommended"], false);
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Terrain));
    assert_eq!(wizard(&mut studio)["recommended"], true);

    // Switching back finds them as they were left.
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Closed));
    let status = send(&mut studio, json!({"command": "status"}))["result"].clone();
    assert_eq!(status["closed_mesh"]["settings"]["voxel"], 0.04);
    assert_eq!(status["closed_mesh"]["settings"]["max_hole"], 0.1);
    assert_eq!(status["closed_mesh"]["settings"]["layers"], "visible");
    assert_eq!(status["faces"]["settings"]["distance_tolerance"], 0.015);
    assert_eq!(status["faces"]["settings"]["cylinders"], false);
    assert_eq!(status["surface_settings"]["max_vertices"], "20000");
    assert_eq!(status["mesh_wizard"]["method"], "closed");
    assert_eq!(status["mesh_wizard"]["recommended"], false);
    let _ = studio.view();

    // The preset of the closed mesh puts back its own settings, and keeps
    // the scans it reads; the other methods keep theirs.
    act(&mut studio, MeshWizardAction::Recommended);
    let status = send(&mut studio, json!({"command": "status"}))["result"].clone();
    assert_eq!(
        status["closed_mesh"]["settings"],
        json!({
            "voxel": null, "max_hole": 0.25, "simplify_mm": null, "sample_percent": 100.0,
            "sides": "automatic", "layers": "visible",
        })
    );
    assert_eq!(status["mesh_wizard"]["recommended"], true);
    assert_eq!(status["faces"]["settings"]["distance_tolerance"], 0.015);
    assert_eq!(status["surface_settings"]["max_vertices"], "20000");

    act(&mut studio, MeshWizardAction::Method(MeshMethod::Faces));
    act(&mut studio, MeshWizardAction::Recommended);
    let faces = send(&mut studio, json!({"command": "status"}))["result"]["faces"].clone();
    assert_eq!(faces["settings"]["distance_tolerance"], 0.02);
    assert_eq!(faces["settings"]["cylinders"], true);
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Surface));
    act(&mut studio, MeshWizardAction::Recommended);
    assert_eq!(studio.surface_settings[0], "50000");
    assert_eq!(wizard(&mut studio)["recommended"], true);

    // A setting that cannot be read holds the Run button back, with the
    // reason in the language in use.
    let _ = studio.update(Message::SurfaceSetting(1, "40".into()));
    let shown = wizard(&mut studio);
    assert_eq!(shown["run_ready"], false);
    assert_eq!(
        shown["run_reason"],
        "Neighbors must be a whole number from 3 to 32"
    );
    act(&mut studio, MeshWizardAction::Run);
    assert!(studio.mesh_job.is_none() && !studio.mesh_dialog_pending);
    assert_eq!(studio.mesh_wizard.step, WizardStep::Options);
    {
        let _language = TestLanguage::hold(Language::Table(0));
        assert_eq!(
            studio.mesh_refusal(MeshMethod::Surface).unwrap(),
            "Buren moet een geheel getal van 3 tot en met 32 zijn"
        );
    }
}

#[test]
fn a_job_goes_on_while_the_card_is_closed_and_the_card_opens_on_its_run_step() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    act(&mut studio, MeshWizardAction::Open);
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Closed));
    act(&mut studio, MeshWizardAction::Step(WizardStep::Run));
    assert_eq!(wizard(&mut studio)["run_state"], "idle");
    act(&mut studio, MeshWizardAction::Step(WizardStep::Options));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Voxel("0.04".into())));
    act(&mut studio, MeshWizardAction::Run);
    assert!(studio.closed_mesh.is_running());
    assert_eq!(studio.mesh_wizard.step, WizardStep::Run);
    let shown = wizard(&mut studio);
    assert_eq!(
        (&shown["running"], &shown["run_state"], &shown["run_ready"]),
        (&json!("closed"), &json!("running"), &json!(false))
    );
    // A second job of the same method waits for this one.
    assert_eq!(
        studio.mesh_refusal(MeshMethod::Closed).unwrap(),
        "A closed mesh is already being made"
    );
    // So does a terrain mesh: one mesh job at a time.
    assert!(studio.mesh_refusal(MeshMethod::Terrain).is_some());
    view_everything(&mut studio);

    // Another method is looked at and the card is closed: the job goes on,
    // and the button in the ribbon says so.
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Faces));
    act(&mut studio, MeshWizardAction::Step(WizardStep::Method));
    act(&mut studio, MeshWizardAction::Close);
    assert!(studio.closed_mesh.is_running());
    assert_eq!(studio.mesh_running(), Some(MeshMethod::Closed));
    let _ = studio.view();
    // Opened again, the card shows the Run step of that job.
    act(&mut studio, MeshWizardAction::Open);
    assert_eq!(
        (studio.mesh_wizard.step, studio.mesh_wizard.method),
        (WizardStep::Run, MeshMethod::Closed)
    );

    studio.finish_closed_mesh_here();
    assert!(!studio.closed_mesh.is_running());
    assert_eq!(studio.mesh_running(), None);
    let shown = wizard(&mut studio);
    assert_eq!(
        (&shown["run_state"], &shown["running"]),
        (&json!("done"), &Value::Null)
    );
    let RunState::Done { rows, lines, .. } = studio.method_run(MeshMethod::Closed) else {
        panic!("the job is done");
    };
    assert!(rows.len() >= 9, "{}", rows.len());
    assert!(
        lines[0].starts_with("Shown as the mesh of room.xyz"),
        "{lines:?}"
    );
    view_everything(&mut studio);
    // The mesh can be saved, and Properties gives its figures under the
    // mesh of the scan, how far the points lie from it included.
    assert!(studio.mesh_result_exportable(MeshMethod::Closed));
    let mesh = Arc::clone(studio.clouds[0].mesh.as_ref().unwrap());
    assert!(!studio.closed_mesh_figures(&mesh).is_empty());
    assert!(studio
        .closed_mesh_figures(&Arc::new(MeshGeometry::default()))
        .is_empty());
    assert!(studio.mesh_properties().is_some());

    // Show in model closes the card and switches the mesh on.
    studio.clouds[0].mesh_visible = false;
    act(&mut studio, MeshWizardAction::ShowInModel);
    assert!(!studio.mesh_wizard.is_open());
    assert!(studio.clouds[0].mesh_visible);
    // With nothing running the card opens where it was.
    act(&mut studio, MeshWizardAction::Open);
    assert_eq!(studio.mesh_wizard.step, WizardStep::Run);
    act(&mut studio, MeshWizardAction::Step(WizardStep::Options));
    act(&mut studio, MeshWizardAction::Close);
    act(&mut studio, MeshWizardAction::Open);
    assert_eq!(studio.mesh_wizard.step, WizardStep::Options);
}

#[test]
fn faces_are_found_from_the_card_and_shown_in_properties() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    assert!(studio.faces_properties().is_none());
    act(&mut studio, MeshWizardAction::Open);
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Faces));
    act(&mut studio, MeshWizardAction::Next);
    act(&mut studio, MeshWizardAction::Run);
    assert!(studio.faces.is_running());
    assert_eq!(studio.mesh_wizard.step, WizardStep::Run);
    assert_eq!(wizard(&mut studio)["running"], "faces");
    let RunState::Running(progress) = studio.method_run(MeshMethod::Faces) else {
        panic!("the job runs");
    };
    assert_eq!(progress.steps.map(|(place, _)| place), Some(1));
    assert!(!progress.cancelling);
    // Cancel asks the job to stop.
    act(&mut studio, MeshWizardAction::Cancel);
    let RunState::Running(progress) = studio.method_run(MeshMethod::Faces) else {
        panic!("the job ends its step first");
    };
    assert!(progress.cancelling);
    studio.finish_faces_here();
    assert_eq!(wizard(&mut studio)["run_state"], "cancelled");
    act(&mut studio, MeshWizardAction::Step(WizardStep::Options));
    act(&mut studio, MeshWizardAction::Run);
    studio.finish_faces_here();
    assert_eq!(wizard(&mut studio)["run_state"], "done");
    // The faces of the active scan are in Properties, whatever the card.
    act(&mut studio, MeshWizardAction::Close);
    assert!(studio.faces_properties().is_some());
    act(&mut studio, MeshWizardAction::Open);
    assert!(studio.mesh_result_exportable(MeshMethod::Faces));
    let RunState::Done { rows, .. } = studio.method_run(MeshMethod::Faces) else {
        panic!("the job is done");
    };
    assert_eq!(rows.len(), 5);
    view_everything(&mut studio);
}

#[test]
fn a_terrain_mesh_asks_for_its_file_and_its_end_is_kept() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    act(&mut studio, MeshWizardAction::Open);
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Terrain));
    act(&mut studio, MeshWizardAction::Next);
    act(&mut studio, MeshWizardAction::Run);
    // The save dialog is open; the Run step waits for it.
    assert!(studio.mesh_dialog_pending);
    assert_eq!(studio.mesh_wizard.step, WizardStep::Run);
    let shown = wizard(&mut studio);
    assert_eq!(
        (&shown["running"], &shown["run_state"]),
        (&json!("terrain"), &json!("choosing"))
    );
    let _ = studio.view();
    // Closed without a file, the card goes back to the options.
    let cloud = Arc::clone(&studio.clouds[0].cloud);
    let _ = studio.update(Message::MeshPathChosen(
        MeshMode::Terrain,
        SurfaceMeshConfig::default(),
        Arc::clone(&cloud),
        None,
        None,
    ));
    assert_eq!(studio.mesh_wizard.step, WizardStep::Options);
    assert_eq!(wizard(&mut studio)["run_state"], "idle");

    // With a file the job starts, and the Run step follows it.
    act(&mut studio, MeshWizardAction::Run);
    let path = directory.path().join("room-terrain.obj");
    let _ = studio.update(Message::MeshPathChosen(
        MeshMode::Terrain,
        SurfaceMeshConfig::default(),
        Arc::clone(&cloud),
        None,
        Some(path.clone()),
    ));
    assert!(studio.mesh_job.is_some());
    let RunState::Running(progress) = studio.method_run(MeshMethod::Terrain) else {
        panic!("the job runs");
    };
    assert_eq!(progress.steps, Some((1, 3)));
    // The other method of the same job waits, and is not shown as running.
    assert!(!studio.method_runs(MeshMethod::Surface));
    let _ = studio.view();

    // How it ended is kept for the Run step.
    let _ = studio.update(Message::MeshReady(
        MeshMode::Terrain,
        Err("Operation cancelled".into()),
    ));
    assert_eq!(wizard(&mut studio)["run_state"], "cancelled");
    let mesh = MeshGeometry {
        vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        triangles: vec![[0, 1, 2]],
        ..MeshGeometry::default()
    };
    let _ = studio.update(Message::MeshReady(
        MeshMode::Terrain,
        Ok((
            Arc::clone(&cloud),
            path.clone(),
            pointcloud_core::MeshStats {
                source_points: 23_600,
                vertices: 3,
                triangles: 1,
            },
            MeasuredMesh::measure(mesh),
        )),
    ));
    assert_eq!(wizard(&mut studio)["run_state"], "done");
    let Some(FileMeshLast::Done {
        vertices,
        triangles,
        shown_on,
        topology,
        ..
    }) = &studio.mesh_wizard.file_last
    else {
        panic!("the job is done");
    };
    assert_eq!((*vertices, *triangles), (3, 1));
    assert_eq!(shown_on.as_deref(), Some("room.xyz"));
    assert_eq!(topology.open_edges, 3);
    // The 3D surface has its own Run step.
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Surface));
    assert_eq!(wizard(&mut studio)["run_state"], "idle");
    act(&mut studio, MeshWizardAction::Method(MeshMethod::Terrain));
    view_everything(&mut studio);
    let _ = studio.update(Message::MeshReady(
        MeshMode::Terrain,
        Err("disk full".into()),
    ));
    assert_eq!(wizard(&mut studio)["run_state"], "failed");
}

#[test]
fn the_card_says_what_the_methods_work_on() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let total = room_points().len() as u64;
    act(&mut studio, MeshWizardAction::Open);
    let scope = wizard(&mut studio)["scope"].clone();
    assert_eq!(
        scope["active"],
        json!({"name": "room.xyz", "points": total})
    );
    assert_eq!(
        (&scope["visible_scans"], &scope["visible_points"]),
        (&json!(1), &json!(total))
    );
    assert_eq!(scope["section"], Value::Null);
    assert_eq!(scope["selected"], 0);

    // The lower half of the room in the section box: about half of the
    // points of the walls and those of the floor.
    let answer = send(
        &mut studio,
        json!({"command": "set_section", "min": [0.0, 0.0, 0.0], "max": [1.6, 1.2, 0.5]}),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let section = wizard(&mut studio)["scope"]["section"].clone();
    let inside = section["active_points"].as_u64().unwrap();
    let expected = room_points().iter().filter(|xyz| xyz[2] <= 0.5).count() as f64;
    assert!(
        (inside as f64 - expected).abs() < expected * 0.15,
        "{inside} of about {expected}"
    );
    assert_eq!(section["visible_points"], inside);
    let size: Vec<f64> = serde_json::from_value(section["size"].clone()).unwrap();
    assert!(
        (size[0] - 1.6).abs() < 1e-6 && (size[2] - 0.5).abs() < 1e-6,
        "{size:?}"
    );
    view_everything(&mut studio);

    // The switch of the card takes the box away again.
    let _ = studio.update(Message::SetSectionEnabled(false));
    assert_eq!(wizard(&mut studio)["scope"]["section"], Value::Null);
}

#[test]
fn the_local_api_opens_the_card_on_a_step_and_a_method() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let answer = send(
        &mut studio,
        json!({"command": "mesh_wizard", "open": true, "method": "faces", "step": "options"}),
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let shown = &answer["mesh_wizard"];
    assert_eq!(
        (&shown["open"], &shown["method"], &shown["step"]),
        (&json!(true), &json!("faces"), &json!("options"))
    );
    assert_eq!(shown["run_ready"], true);
    // The card covers the model: a screenshot of the scene is refused, and
    // the File view does not open over it.
    let shot = send(&mut studio, json!({"command": "screenshot"}));
    assert_eq!(shot["error"], crate::screenshot::COVERED);
    let file = send(&mut studio, json!({"command": "file_view", "open": true}));
    assert_eq!(
        file["error"],
        "the Mesh Pointcloud card is open; close it first"
    );

    for (body, error) in [
        (
            json!({"command": "mesh_wizard", "open": true, "step": "finish"}),
            "unknown step; use method, options, run",
        ),
        (
            json!({"command": "mesh_wizard", "open": true, "method": "nurbs"}),
            "unknown method; use closed, terrain, surface, faces",
        ),
        (
            json!({"command": "mesh_wizard", "open": false, "step": "run"}),
            "step and method can only be given with open: true",
        ),
    ] {
        assert_eq!(send(&mut studio, body)["error"], error);
    }

    // A job started by the command of the local API is shown on its Run
    // step when the card opens without a step.
    let closed = send(
        &mut studio,
        json!({"command": "mesh_wizard", "open": false}),
    );
    assert_eq!(closed["mesh_wizard"]["open"], false);
    let started = send(
        &mut studio,
        json!({"command": "mesh", "mode": "closed", "voxel": 0.04}),
    );
    assert_eq!(started["ok"], true, "{started}");
    let opened = send(&mut studio, json!({"command": "mesh_wizard", "open": true}));
    assert_eq!(
        (
            &opened["mesh_wizard"]["step"],
            &opened["mesh_wizard"]["method"]
        ),
        (&json!("run"), &json!("closed"))
    );
    studio.finish_closed_mesh_here();
    assert_eq!(wizard(&mut studio)["run_state"], "done");
    // The Settings dialog lies over everything: the card does not open
    // under it.
    let _ = send(
        &mut studio,
        json!({"command": "mesh_wizard", "open": false}),
    );
    let _ = studio.update(Message::Settings(
        crate::settings_dialog::SettingsAction::Open,
    ));
    let refused = send(&mut studio, json!({"command": "mesh_wizard", "open": true}));
    assert_eq!(refused["error"], "the Settings dialog is open");
}

#[test]
fn the_ribbon_has_one_button_that_shows_a_job_that_runs() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let source = include_str!("../main.rs");
    for gone in [
        "self.closed_mesh_ribbon_item()",
        "self.faces_ribbon_item()",
        "\"3D surface settings\"",
    ] {
        assert!(!source.contains(gone), "{gone}");
    }
    let _ = studio.view();
    let _ = studio.update(Message::Faces(FaceAction::Start));
    assert_eq!(studio.mesh_running(), Some(MeshMethod::Faces));
    // The button shows the job in both languages.
    for language in [Language::English, Language::Table(0)] {
        let _language = TestLanguage::hold(language);
        let _ = studio.view();
    }
    {
        let _language = TestLanguage::hold(Language::Table(0));
        assert_eq!(tr("Mesh Pointcloud"), "Puntenwolk meshen");
        for method in MeshMethod::ALL {
            for text in [
                method.label(),
                method.makes(),
                method.when(),
                method.preset(),
            ] {
                assert!(crate::i18n::has_entry(text), "{text}");
            }
        }
        for step in WizardStep::ALL {
            assert!(crate::i18n::has_entry(step.label()), "{:?}", step);
        }
    }
    studio.finish_faces_here();
    assert_eq!(studio.mesh_running(), None);
}
