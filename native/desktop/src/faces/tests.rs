//! Tests of the Detect faces tool, on a generated room with a column.

use std::path::Path;

use pointcloud_core::surfaces::{FACES_JSON_FORMAT, FACES_JSON_VERSION};

use super::*;
use crate::i18n::{Language, TestLanguage};
use crate::mcp::busy;
use crate::native_api::{ApiCommand, ApiRequest};

/// The inside of the room: 1.6 by 1.2 m and 1.0 m high, with one corner at
/// zero.
const ROOM: [f64; 3] = [1.6, 1.2, 1.0];
/// A round column from the floor to the ceiling.
const COLUMN: [f64; 2] = [0.5, 0.6];
const COLUMN_RADIUS: f64 = 0.15;
const STEP: f64 = 0.01;

/// Points on the six inner faces of the room, one per centimetre. They lie
/// half a step from the edges, so no two faces share a point.
fn wall_points() -> Vec<[f64; 3]> {
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

/// Points all round the column, one per centimetre.
fn column_points() -> Vec<[f64; 3]> {
    let around = (std::f64::consts::TAU * COLUMN_RADIUS / STEP).round() as usize;
    let up = (ROOM[2] / STEP).round() as usize;
    let mut points = Vec::with_capacity(around * up);
    for turn in 0..around {
        let (sin, cos) = (std::f64::consts::TAU * turn as f64 / around as f64).sin_cos();
        for level in 0..up {
            points.push([
                COLUMN[0] + COLUMN_RADIUS * cos,
                COLUMN[1] + COLUMN_RADIUS * sin,
                (level as f64 + 0.5) * STEP,
            ]);
        }
    }
    points
}

fn room_points() -> Vec<[f64; 3]> {
    let mut points = wall_points();
    points.extend(column_points());
    points
}

fn write_xyz(path: &Path, points: &[[f64; 3]]) {
    let text: String = points
        .iter()
        .map(|[x, y, z]| format!("{x:.4} {y:.4} {z:.4}\n"))
        .collect();
    std::fs::write(path, text).unwrap();
}

/// The room as an XYZ file, which knows no station.
fn write_room_xyz(directory: &Path) -> PathBuf {
    let path = directory.join("room.xyz");
    write_xyz(&path, &room_points());
    path
}

/// The walls of the room as a PTX scan measured from one station inside it.
fn write_room_ptx(directory: &Path) -> PathBuf {
    let station = [1.1, 0.5, 0.4];
    let points = wall_points();
    let mut text = format!(
        "{}\n1\n{} {} {}\n1 0 0\n0 1 0\n0 0 1\n1 0 0 0\n0 1 0 0\n0 0 1 0\n{} {} {} 1\n",
        points.len(),
        station[0],
        station[1],
        station[2],
        station[0],
        station[1],
        station[2]
    );
    for point in points {
        let [x, y, z]: [f64; 3] = std::array::from_fn(|axis| point[axis] - station[axis]);
        text.push_str(&format!("{x:.4} {y:.4} {z:.4} 0.5\n"));
    }
    let path = directory.join("room.ptx");
    std::fs::write(&path, text).unwrap();
    path
}

fn open_layer(studio: &mut Studio, path: &Path) {
    let cloud = Arc::new(pointcloud_core::open(path, 1_000).unwrap());
    let _ = studio.update(Message::Loaded(Ok(cloud)));
}

/// A window with the room open as its one layer.
fn studio_with_room(directory: &Path) -> Studio {
    let mut studio = Studio::default();
    open_layer(&mut studio, &write_room_xyz(directory));
    studio
}

/// Send a command the way the window receives it.
fn send(studio: &mut Studio, command: ApiCommand) -> Value {
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.update(Message::ApiRequest(ApiRequest { command, reply }));
    receive.recv().unwrap()
}

fn command(body: Value) -> ApiCommand {
    serde_json::from_value(body).unwrap()
}

fn status(studio: &mut Studio) -> Value {
    send(studio, ApiCommand::Status)["result"].clone()
}

fn job(studio: &mut Studio, id: &str) -> Value {
    send(studio, ApiCommand::Job { id: id.to_owned() })["job"].clone()
}

/// What the worker thread of the running job does, and its message to the
/// window.
fn finish(studio: &mut Studio) {
    let running = studio.faces.job.as_ref().expect("a job runs");
    let (serial, input, control) = (
        running.serial,
        Arc::clone(&running.input),
        Arc::clone(&running.control),
    );
    let end = FaceEnd::of(run(&input, &control));
    let _ = studio.update(Message::Faces(FaceAction::Finished(serial, end)));
}

/// Start a detection through the local API and return its job.
fn start(studio: &mut Studio, more: Value) -> String {
    let mut body = json!({"command": "detect_faces"});
    for (name, value) in more.as_object().unwrap() {
        body[name.as_str()] = value.clone();
    }
    let accepted = send(studio, command(body));
    assert_eq!(accepted["ok"], true, "{accepted}");
    accepted["job_id"].as_str().unwrap().to_owned()
}

/// Detect the faces of the layers as they are and return the finished job.
fn detect(studio: &mut Studio, more: Value) -> Value {
    let id = start(studio, more);
    finish(studio);
    job(studio, &id)
}

fn list(studio: &mut Studio, boundaries: bool) -> Value {
    send(studio, ApiCommand::ListFaces { boundaries })
}

/// What the worker thread of a faces export does, and its message to the
/// window.
fn finish_export(studio: &mut Studio, id: &str, path: &Path) {
    let request = studio.faces_export_request().expect("faces to export");
    let format = FaceFormat::from_path(path).unwrap();
    let result = write(&request, path, format).map_err(|error| error.to_string());
    let _ = studio.update(Message::Faces(FaceAction::Exported(
        Some(id.to_owned()),
        result,
    )));
}

/// The deletion mask of a scan of which one point was deleted.
fn one_deleted(total: u64, ordinal: u64) -> Arc<DeletionMask> {
    let mut bits = vec![0u64; total.div_ceil(64) as usize];
    bits[(ordinal / 64) as usize] |= 1 << (ordinal % 64);
    let selection = crate::selection::SelectionMask {
        bits,
        count: 1,
        highlights: Vec::new(),
        highlights_source: true,
        source_bounds: None,
    };
    let mut deleted = DeletionMask::new(total).unwrap();
    assert_eq!(deleted.apply(&selection).unwrap(), 1);
    Arc::new(deleted)
}

/// Build the window in Dutch and in English. The language is one setting of
/// the whole process: the test that calls this holds it, in English, and
/// gets it back in English.
fn view_in_both_languages(studio: &Studio) {
    for language in [Language::Table(0), Language::English] {
        crate::i18n::set(language);
        let _ = studio.view();
    }
}

/// Whether two JSON values are the same, with numbers a rounding step apart
/// taken as equal.
fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() <= 1e-9
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| close(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(name, value)| b.get(name).is_some_and(|other| close(value, other)))
        }
        _ => a == b,
    }
}

fn near(value: &Value, expected: f64, tolerance: f64) -> bool {
    value
        .as_f64()
        .is_some_and(|value| (value - expected).abs() <= tolerance)
}

/// The faces of a listing with a class, by their areas.
fn areas(listing: &Value, class: &str) -> Vec<f64> {
    listing["faces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|face| face["class"] == class)
        .map(|face| face["area"].as_f64().unwrap())
        .collect()
}

#[test]
fn block_starts_with_the_defaults_of_the_core() {
    let settings = FaceSettings::default();
    assert_eq!(settings.distance, "20", "millimetres");
    assert_eq!(settings.angle, "10");
    assert_eq!(settings.min_area, "0.25");
    assert!(settings.cylinders);
    assert_eq!(
        (settings.layers, settings.colouring),
        (Layers::Active, Colouring::Face)
    );
    assert_eq!(settings.config().unwrap(), SurfaceDetectConfig::default());
    assert_eq!(
        settings.value(),
        json!({
            "distance_tolerance": 0.02, "angle_tolerance": 10.0, "min_area": 0.25,
            "cylinders": true, "layers": "active", "color": "face",
        })
    );
    // Every choice has a name of its own for a command, and reads back.
    for colouring in Colouring::ALL {
        assert_eq!(Colouring::from_name(colouring.name()), Some(colouring));
    }
    assert_eq!(Colouring::from_name("residual"), None);
    for (format, _) in FaceFormat::ALL {
        let path = PathBuf::from(format!("faces.{}", format.extension().to_uppercase()));
        assert_eq!(FaceFormat::from_path(&path), Some(format));
    }
    assert_eq!(FaceFormat::from_path(Path::new("faces.ply")), None);
}

#[test]
fn typed_numbers_are_read_with_a_comma_or_a_point_and_checked() {
    let with = |change: &dyn Fn(&mut FaceSettings)| {
        let mut settings = FaceSettings::default();
        change(&mut settings);
        settings.config()
    };
    let config = with(&|settings| {
        settings.distance = " 12,5 ".into();
        settings.angle = "7.5".into();
        settings.min_area = "1,5".into();
        settings.cylinders = false;
    })
    .unwrap();
    assert_eq!(config.distance_tolerance, 0.0125);
    assert_eq!(config.angle_tolerance_deg, 7.5);
    assert_eq!(config.min_region_area, 1.5);
    assert!(!config.detect_cylinders);
    // Nothing else of the config comes from the block.
    let defaults = SurfaceDetectConfig::default();
    assert_eq!(config.voxel_size, defaults.voxel_size);
    assert_eq!(config.max_working_points, defaults.max_working_points);

    let problem = |change: &dyn Fn(&mut FaceSettings)| with(change).unwrap_err().english();
    assert_eq!(
        problem(&|settings| settings.distance = "wide".into()),
        "Distance tolerance must be a number of millimetres"
    );
    assert_eq!(
        problem(&|settings| settings.distance = "0.5".into()),
        "The distance tolerance must lie between 1 and 500 mm"
    );
    assert_eq!(
        problem(&|settings| settings.distance = "501".into()),
        "The distance tolerance must lie between 1 and 500 mm"
    );
    assert_eq!(
        problem(&|settings| settings.angle = String::new()),
        "Angle tolerance must be a number of degrees"
    );
    for angle in ["0.9", "45.1", "nan"] {
        let said = problem(&|settings| settings.angle = angle.into());
        assert!(
            said == "The angle tolerance must lie between 1 and 45 degrees"
                || said == "Angle tolerance must be a number of degrees",
            "{angle}: {said}"
        );
    }
    assert_eq!(
        problem(&|settings| settings.min_area = "-".into()),
        "Smallest face must be a number of square metres"
    );
    assert_eq!(
        problem(&|settings| settings.min_area = "0.001".into()),
        "The smallest face must lie between 0.01 and 10000 m²"
    );
    // The ends of every range are settings the core takes.
    for (distance, angle, area) in [("1", "1", "0.01"), ("500", "45", "10000")] {
        let config = with(&|settings| {
            settings.distance = distance.into();
            settings.angle = angle.into();
            settings.min_area = area.into();
        })
        .unwrap();
        config.validate().unwrap();
    }
    // The block says a problem in the language in use.
    let _language = TestLanguage::hold(Language::Table(0));
    assert_eq!(
        with(&|settings| settings.distance = "0".into())
            .unwrap_err()
            .translated(),
        "De afstandstolerantie moet tussen 1 en 500 mm liggen"
    );
}

#[test]
fn fields_of_a_command_go_into_the_block_and_wrong_ones_change_nothing() {
    let mut studio = Studio::default();
    let answer = send(
        &mut studio,
        command(json!({
            "command": "set_face_settings", "distance_tolerance": 0.015,
            "angle_tolerance": 8, "min_area": 0.5, "cylinders": false,
            "layers": "Visible", "color": "deviation",
        })),
    );
    let expected = json!({
        "distance_tolerance": 0.015, "angle_tolerance": 8.0, "min_area": 0.5,
        "cylinders": false, "layers": "visible", "color": "deviation",
    });
    assert_eq!(answer, json!({"ok": true, "settings": expected}));
    assert_eq!(
        studio.faces.settings.distance, "15",
        "the block shows millimetres"
    );
    assert_eq!(status(&mut studio)["faces"]["settings"], expected);

    // One field alone leaves the others.
    let answer = send(
        &mut studio,
        command(json!({"command": "set_face_settings", "cylinders": true})),
    );
    assert_eq!(answer["settings"]["cylinders"], true);
    assert_eq!(answer["settings"]["min_area"], 0.5);

    // A command with a field that is refused changes nothing.
    let before = studio.faces.settings.clone();
    for (body, error) in [
        (
            json!({"distance_tolerance": 2.0, "min_area": 3}),
            "distance_tolerance must lie between 0.001 and 0.5 m",
        ),
        (
            // The millimetres of the block are not the unit of a command.
            json!({"distance_tolerance": 20}),
            "distance_tolerance must lie between 0.001 and 0.5 m",
        ),
        (
            json!({"distance_tolerance": 0.0005}),
            "distance_tolerance must lie between 0.001 and 0.5 m",
        ),
        (
            json!({"angle_tolerance": 60, "cylinders": false}),
            "the angle tolerance must lie between 1 and 45 degrees",
        ),
        (
            json!({"min_area": 0.0}),
            "the smallest face must lie between 0.01 and 10000 m²",
        ),
        (
            json!({"layers": "all", "min_area": 3}),
            "layers must be active or visible",
        ),
        (
            json!({"color": "rainbow"}),
            "color must be face or deviation",
        ),
    ] {
        for name in ["set_face_settings", "detect_faces"] {
            let mut body = body.clone();
            body["command"] = name.into();
            let answer = send(&mut studio, command(body));
            assert_eq!(answer, json!({"ok": false, "error": error}), "{name}");
        }
        assert_eq!(studio.faces.settings, before);
    }
    assert!(!studio.faces.is_running());
}

#[test]
fn api_detects_the_faces_of_a_room_and_lists_them() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let total = room_points().len() as u64;
    assert_eq!(status(&mut studio)["clouds"][0]["faces"], Value::Null);
    assert_eq!(
        list(&mut studio, false),
        json!({"ok": false, "error": "the active layer has no detected faces"})
    );

    let id = start(&mut studio, json!({}));
    // A running job is work under way for a wait, and shows in the strip.
    let running = status(&mut studio);
    assert_eq!(busy(&running), ["faces"]);
    assert_eq!(running["faces"]["job"]["operation"], "detect_faces");
    assert_eq!(running["faces"]["job"]["stage"], "loading");
    assert_eq!(job(&mut studio, &id)["state"], "running");
    let line = studio.faces.progress_line().unwrap();
    assert_eq!(
        (line.phase, line.title.as_str()),
        (Phase::Faces, "Detect faces")
    );
    assert!(line.detail.starts_with("Step 1 of 6"), "{}", line.detail);
    assert!(line.cancel.is_some());
    assert!(studio
        .progress_lines()
        .iter()
        .any(|line| line.phase == Phase::Faces));
    let _ = studio.view();
    // A second job has to wait for the first.
    assert_eq!(
        send(&mut studio, command(json!({"command": "detect_faces"}))),
        json!({"ok": false, "error": "faces are already being detected"})
    );

    finish(&mut studio);
    let done = job(&mut studio, &id);
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["operation"], "detect_faces");
    assert_eq!(
        (
            &done["floors"],
            &done["ceilings"],
            &done["walls"],
            &done["sloped"]
        ),
        (&json!(1), &json!(1), &json!(4), &json!(0)),
        "{done}"
    );
    assert_eq!((&done["cylinders"], &done["count"]), (&json!(1), &json!(7)));
    assert_eq!(
        (&done["source"], &done["kept"]),
        (&json!("room.xyz"), &json!(true))
    );
    assert_eq!(done["points"]["source"], total);
    assert_eq!(done["voxel_size"], 0.03);
    assert_eq!(
        (&done["coarse"], &done["note"]),
        (&json!(false), &Value::Null)
    );
    assert!(
        studio
            .status
            .starts_with("Detected 7 faces in room.xyz: 1 floor, 1 ceiling, 4 walls, 1 cylinder; ")
            && studio.status.ends_with("; voxels of 30 mm"),
        "{}",
        studio.status
    );
    assert!(!studio.faces.is_running());

    // The layer keeps the faces beside its mesh, which it does not have.
    let after = status(&mut studio);
    assert!(busy(&after).is_empty());
    let layer = &after["clouds"][0];
    assert_eq!(layer["mesh"], Value::Null);
    assert_eq!(layer["faces"]["count"], 7);
    assert_eq!(layer["faces"]["planes"], 6);
    assert_eq!(
        (&layer["faces"]["visible"], &layer["faces"]["drawn"]),
        (&json!(true), &json!(true))
    );
    assert_eq!(layer["faces"]["stale"], Value::Null);
    assert_eq!(layer["faces"]["color"], "face");
    assert_eq!(after["faces"]["result"], layer["faces"]);
    assert_eq!(after["faces"]["last"]["state"], "complete");
    assert_eq!(after["faces"]["job"], Value::Null);
    assert!(studio.clouds[0].mesh.is_none());
    assert_eq!(crate::gpu_viewport::drawn_faces(&studio.clouds), [0]);

    // The list: planes first, largest first, then the column.
    let listing = list(&mut studio, false);
    assert_eq!(listing["ok"], true, "{listing}");
    assert_eq!(
        (&listing["count"], &listing["source"]),
        (&json!(7), &json!("room.xyz"))
    );
    let faces = listing["faces"].as_array().unwrap();
    let ids: Vec<u64> = faces
        .iter()
        .map(|face| face["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6, 7]);
    assert!(faces.iter().all(|face| face.get("boundary").is_none()));
    assert!(listing.get("edges").is_none());
    assert_eq!(listing["edge_count"], 12, "the twelve edges of the room");
    let floor = (ROOM[0] * ROOM[1], 0.02 * ROOM[0] * ROOM[1]);
    for class in ["floor", "ceiling"] {
        let found = areas(&listing, class);
        assert_eq!(found.len(), 1, "{class}");
        assert!((found[0] - floor.0).abs() <= floor.1, "{class}: {found:?}");
    }
    let mut walls = areas(&listing, "wall");
    walls.sort_by(f64::total_cmp);
    for (found, expected) in walls.iter().zip([1.2, 1.2, 1.6, 1.6]) {
        assert!((found - expected).abs() <= 0.02 * expected, "{walls:?}");
    }
    let column = &faces[6];
    assert_eq!(column["type"], "cylinder");
    assert!(
        near(&column["diameter"], 2.0 * COLUMN_RADIUS, 0.003),
        "{column}"
    );
    assert!(near(&column["length"], ROOM[2], 0.05), "{column}");
    for face in faces {
        assert!(near(&face["residual"]["rms"], 0.0, 0.001), "{face}");
    }
    // With the outlines and the edges for whoever asks.
    let full = list(&mut studio, true);
    assert_eq!(full["edges"].as_array().unwrap().len(), 12);
    let outline = &full["faces"][0]["boundary"][0]["outer"];
    assert!(outline.as_array().unwrap().len() >= 4, "{outline}");

    // A face is highlighted by its number, in the list and in the scene.
    let chosen = send(
        &mut studio,
        command(json!({"command": "select_face", "id": 7})),
    );
    assert_eq!(chosen["ok"], true, "{chosen}");
    assert_eq!(
        (&chosen["selected"], &chosen["face"]["type"]),
        (&json!(7), &json!("cylinder"))
    );
    assert_eq!(status(&mut studio)["clouds"][0]["faces"]["selected"], 7);
    let _ = studio.view();
    let chosen = send(
        &mut studio,
        command(json!({"command": "select_face", "id": 1})),
    );
    assert!(chosen["face"]["boundary"].is_array(), "{chosen}");
    let _ = studio.view();
    assert_eq!(
        send(
            &mut studio,
            command(json!({"command": "select_face", "id": 8}))
        ),
        json!({"ok": false, "error": "no face has the number 8; list_faces gives the numbers"})
    );
    assert_eq!(
        list(&mut studio, false)["selected"],
        1,
        "a refusal changes nothing"
    );
    // A click on the highlighted row takes the highlight off.
    let _ = studio.update(Message::Faces(FaceAction::Select(Some(1))));
    assert_eq!(list(&mut studio, false)["selected"], Value::Null);
    let _ = studio.update(Message::Faces(FaceAction::Select(Some(3))));
    assert_eq!(
        send(&mut studio, command(json!({"command": "select_face"}))),
        json!({"ok": true, "selected": null, "face": null})
    );

    // The switch of the project list hides the faces and keeps them.
    let _ = studio.update(Message::Faces(FaceAction::Visible(0, false)));
    assert!(crate::gpu_viewport::drawn_faces(&studio.clouds).is_empty());
    let hidden = status(&mut studio);
    assert_eq!(
        (
            &hidden["clouds"][0]["faces"]["visible"],
            &hidden["clouds"][0]["faces"]["drawn"]
        ),
        (&json!(false), &json!(false))
    );
    let _ = studio.view();
    let _ = studio.update(Message::Faces(FaceAction::Visible(0, true)));

    // Clear faces removes the layer.
    assert_eq!(
        send(&mut studio, ApiCommand::ClearFaces),
        json!({"ok": true, "cleared": true})
    );
    assert!(studio.clouds[0].faces.is_none());
    assert_eq!(status(&mut studio)["clouds"][0]["faces"], Value::Null);
    assert_eq!(
        send(&mut studio, ApiCommand::ClearFaces),
        json!({"ok": true, "cleared": false})
    );
}

#[test]
fn colouring_is_one_colour_per_face_or_the_deviation_of_the_points() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    detect(&mut studio, json!({}));
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    let (flat, deviation) = (Arc::clone(&layer.flat), Arc::clone(&layer.deviation));
    assert!(Arc::ptr_eq(layer.shown().unwrap(), &flat));
    // The flat mesh holds the outlines as triangles, the deviation mesh two
    // triangles per cell of 5 cm that holds points.
    assert!(flat.triangles.len() >= 12, "{}", flat.triangles.len());
    assert!(deviation.triangles.len() > 20 * flat.triangles.len());
    for mesh in [&flat, &deviation] {
        let colors = mesh.colors.as_ref().unwrap();
        assert_eq!(colors.len(), mesh.vertices.len());
        assert_eq!(mesh.normals.as_ref().unwrap().len(), mesh.vertices.len());
    }
    // Without noise every point lies on its face, within a millimetre: the
    // colour of zero, or one close to it.
    let on_face = deviation_legend(0.02)[1].color;
    let colors = deviation.colors.as_ref().unwrap();
    let on = colors
        .iter()
        .filter(|color| (0..3).all(|channel| color[channel].abs_diff(on_face[channel]) <= 12))
        .count();
    assert!(on * 10 >= colors.len() * 9, "{on} of {}", colors.len());

    let _ = studio.update(Message::Faces(FaceAction::Colouring(Colouring::Deviation)));
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(Arc::ptr_eq(layer.shown().unwrap(), &deviation));
    assert_eq!(
        status(&mut studio)["clouds"][0]["faces"]["color"],
        "deviation"
    );
    // The legend of the block: the three stops at the tolerance, in metres.
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert_eq!(layer.deviation_scale(), 0.02);
    let [behind, on, front] = deviation_legend(layer.deviation_scale());
    assert_eq!((behind.value, on.value, front.value), (-0.02, 0.0, 0.02));
    // It says them in millimetres, with the tenth a tolerance with a
    // fraction or a scaled scan gives.
    assert_eq!(legend_millimetres(behind.value), "20");
    assert_eq!(legend_millimetres(front.value), "20");
    assert_eq!(legend_millimetres(0.0025), "2.5");
    assert_eq!(legend_millimetres(-0.0015), "1.5");
    assert_eq!(legend_millimetres(0.02 * 0.3048), "6.1");
    assert_eq!(legend_millimetres(0.5), "500");
    view_in_both_languages(&studio);
    // A command sets it as the block does, for the faces that are shown.
    let answer = send(
        &mut studio,
        command(json!({"command": "set_face_settings", "color": "face"})),
    );
    assert_eq!(answer["settings"]["color"], "face");
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(Arc::ptr_eq(layer.shown().unwrap(), &flat));
    // Hidden faces show no mesh.
    let _ = studio.update(Message::Faces(FaceAction::Visible(0, false)));
    assert!(studio.clouds[0].faces.as_ref().unwrap().shown().is_none());
    // A layer that does not exist has no switch.
    let _ = studio.update(Message::Faces(FaceAction::Visible(5, true)));
}

#[test]
fn faces_follow_a_scan_that_is_moved_and_scaled() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    detect(&mut studio, json!({}));
    let before = list(&mut studio, true);
    let (flat, deviation) = {
        let layer = studio.clouds[0].faces.as_ref().unwrap();
        (Arc::clone(&layer.flat), Arc::clone(&layer.deviation))
    };

    // A move takes the faces along: the list and the exports give them where
    // the scan stands, and the meshes of the viewer stay as they are.
    let moved = send(
        &mut studio,
        ApiCommand::Translate {
            offset: [10.0, -20.0, 5.0],
        },
    );
    assert_eq!(moved["ok"], true, "{moved}");
    let after = list(&mut studio, true);
    assert_eq!(after["stale"], Value::Null, "a move keeps the faces");
    assert_eq!(after["count"], 7);
    for (was, is) in before["faces"]
        .as_array()
        .unwrap()
        .iter()
        .zip(after["faces"].as_array().unwrap())
    {
        assert_eq!(was["id"], is["id"]);
        if was["type"] == "plane" {
            assert!(near(&is["area"], was["area"].as_f64().unwrap(), 1e-9));
            let (was, is) = (
                &was["boundary"][0]["outer"][0],
                &is["boundary"][0]["outer"][0],
            );
            for (axis, offset) in [10.0, -20.0, 5.0].into_iter().enumerate() {
                assert!(
                    near(&is[axis], was[axis].as_f64().unwrap() + offset, 1e-9),
                    "{was} {is}"
                );
            }
        } else {
            assert!(near(&is["axis_start"][0], COLUMN[0] + 10.0, 0.005), "{is}");
            assert!(near(&is["axis_start"][1], COLUMN[1] - 20.0, 0.005), "{is}");
        }
    }
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(Arc::ptr_eq(&layer.flat, &flat) && Arc::ptr_eq(&layer.deviation, &deviation));

    // A scale that is the same along every axis keeps the column, twice as
    // wide, and makes every area four times as large.
    let _ = studio.update(Message::Faces(FaceAction::Select(Some(7))));
    studio.clouds[0].transform.scale = [2.0; 3];
    studio.settle_faces();
    let doubled = list(&mut studio, false);
    assert_eq!(doubled["stale"], Value::Null, "a scale keeps the faces");
    assert_eq!(doubled["selected"], 7);
    let floor = areas(&doubled, "floor")[0];
    assert!((floor - 4.0 * ROOM[0] * ROOM[1]).abs() < 0.2, "{floor}");
    assert!(near(
        &doubled["faces"][6]["diameter"],
        4.0 * COLUMN_RADIUS,
        0.006
    ));
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(Arc::ptr_eq(&layer.flat, &flat));
    // The legend follows: the full colour stands for twice the tolerance.
    assert!((layer.deviation_scale() - 0.04).abs() < 1e-12);

    // Scaled unequally a column is no cylinder: it leaves the list and the
    // meshes, with its highlight, and comes back with an equal scale.
    studio.clouds[0].transform.scale = [2.0, 1.0, 1.0];
    studio.settle_faces();
    let squeezed = list(&mut studio, false);
    assert_eq!(
        (&squeezed["count"], &squeezed["selected"]),
        (&json!(6), &Value::Null)
    );
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(layer.flat.vertices.len() < flat.vertices.len());
    assert_eq!(status(&mut studio)["clouds"][0]["faces"]["cylinders"], 0);
    studio.clouds[0].transform = CloudTransform::default();
    studio.settle_faces();
    let back = list(&mut studio, true);
    assert_eq!(back["count"], 7);
    assert!(close(&back["faces"], &before["faces"]), "{}", back["faces"]);
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert_eq!(layer.flat.vertices.len(), flat.vertices.len());

    // A scale of zero has no faces to give: the list keeps what it had.
    studio.clouds[0].transform.scale = [0.0, 1.0, 1.0];
    studio.settle_faces();
    assert!(close(&list(&mut studio, true)["faces"], &before["faces"]));
    let _ = studio.view();
}

#[test]
fn faces_are_out_of_date_when_their_points_are_no_longer_those_of_the_scene() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let stale = |studio: &mut Studio| status(studio)["clouds"][0]["faces"]["stale"].clone();
    let total = studio.clouds[0].cloud.total_points;
    detect(&mut studio, json!({}));
    assert_eq!(stale(&mut studio), Value::Null);
    // What does not touch the points leaves the faces as they are: the
    // camera, the classes shown, the section box, a highlight.
    let _ = studio.update(Message::ResetCamera);
    let _ = studio.update(Message::FilterClass(2, false));
    let _ = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [0.0; 3],
            max: [1.0; 3],
        },
    );
    let _ = send(&mut studio, ApiCommand::ClearSection);
    let _ = studio.update(Message::FilterClass(2, true));
    assert_eq!(stale(&mut studio), Value::Null);

    // Deleted points: the residuals are no longer those of the scan.
    let deleted = one_deleted(total, 3);
    studio.clouds[0].deleted = Some(deleted);
    let _ = studio.update(Message::ResetCamera);
    assert_eq!(stale(&mut studio), "points");
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(layer.basis.is_empty(), "nothing is held to compare with");
    assert_eq!(
        layer.stale.as_ref().unwrap().sentence().english(),
        "Out of date: points of room.xyz were deleted, restored or thinned after these faces \
         were detected. The residuals and the deviation colours are those of the points as they \
         were. Detect again."
    );
    // The faces stay in view and in the list, and say what they are.
    assert_eq!(crate::gpu_viewport::drawn_faces(&studio.clouds), [0]);
    let listing = list(&mut studio, false);
    assert_eq!(
        (&listing["count"], &listing["stale"]),
        (&json!(7), &json!("points"))
    );
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    view_in_both_languages(&studio);
    // Restoring the points does not bring the faces up to date: the scan
    // they were made from is not known to be the same.
    studio.clouds[0].deleted = None;
    studio.settle_faces();
    assert_eq!(stale(&mut studio), "points");
    // A new detection does, and leaves the deleted point out.
    let deleted = one_deleted(total, 3);
    studio.clouds[0].deleted = Some(deleted);
    let done = detect(&mut studio, json!({}));
    assert_eq!(done["points"]["source"], total - 1);
    assert_eq!(stale(&mut studio), Value::Null);

    // The first index of a scan holds the points the job read from the file
    // itself: the faces stay as they are.
    let index =
        Arc::new(OctreeIndex::build(&studio.clouds[0].cloud, IndexConfig::default()).unwrap());
    studio.clouds[0].index = Some(index);
    studio.settle_faces();
    assert_eq!(stale(&mut studio), Value::Null);
    // With the index a job reads the leaves, and finds the same faces.
    let done = detect(&mut studio, json!({}));
    assert_eq!(
        (&done["count"], &done["points"]["source"]),
        (&json!(7), &json!(total - 1))
    );
    assert_eq!(stale(&mut studio), Value::Null);
    let stages = {
        let _ = start(&mut studio, json!({}));
        let stages = studio.faces.job.as_ref().unwrap().stages();
        finish(&mut studio);
        stages
    };
    assert!(
        !stages.contains(&Stage::Loading),
        "an index is not read into memory"
    );
    // Another index in its place: the points were read through the one that
    // is gone.
    assert_eq!(stale(&mut studio), Value::Null);
    let other =
        Arc::new(OctreeIndex::build(&studio.clouds[0].cloud, IndexConfig::default()).unwrap());
    studio.clouds[0].index = Some(other);
    studio.settle_faces();
    assert_eq!(stale(&mut studio), "index");
    assert_eq!(
        studio.clouds[0]
            .faces
            .as_ref()
            .unwrap()
            .stale
            .as_ref()
            .unwrap()
            .sentence()
            .english(),
        "Out of date: the index of room.xyz that the points were read through was replaced after \
         these faces were detected. Detect again to be sure of the result."
    );
    view_in_both_languages(&studio);

    // Removing the scan takes its faces along.
    let _ = studio.update(Message::Remove(0));
    assert!(studio.clouds.is_empty());
}

#[test]
fn thinning_undo_and_redo_make_the_faces_out_of_date_and_a_first_index_does_not() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let stale = |studio: &Studio| {
        studio.clouds[0]
            .faces
            .as_ref()
            .unwrap()
            .stale
            .as_ref()
            .map(Stale::name)
    };
    let cloud = Arc::clone(&studio.clouds[0].cloud);
    let total = cloud.total_points;
    // Thin, as the worker of the window answers it.
    let thin = |studio: &mut Studio, percent: u8| {
        let baseline = studio.clouds[0].deleted.as_ref().map(Arc::clone);
        let removed =
            crate::selection::SelectionMask::thin_removed(total, baseline.as_deref(), percent)
                .map(Arc::new);
        let _ = studio.update(Message::ThinReady {
            source: Arc::clone(&cloud),
            baseline,
            percent,
            result: removed,
        });
    };

    detect(&mut studio, json!({}));
    assert_eq!(stale(&studio), None);
    thin(&mut studio, 50);
    assert!(studio.status.starts_with("Kept 50%"), "{}", studio.status);
    assert_eq!(stale(&studio), Some("points"));

    // Undo changes the mask the new faces were made with, in place.
    let done = detect(&mut studio, json!({}));
    assert!(done["points"]["source"].as_u64().unwrap() < total * 6 / 10);
    assert_eq!(stale(&studio), None);
    let _ = studio.update(Message::UndoDelete);
    assert!(studio.status.starts_with("Restored "), "{}", studio.status);
    assert_eq!(stale(&studio), Some("points"));
    detect(&mut studio, json!({}));
    assert_eq!(stale(&studio), None);
    let _ = studio.update(Message::RedoDelete);
    assert_eq!(stale(&studio), Some("points"));

    // The index the window builds for a scan that had none: the job read
    // the file itself, and the index holds those same points.
    detect(&mut studio, json!({}));
    assert_eq!(stale(&studio), None);
    let index = Arc::new(OctreeIndex::build(&cloud, IndexConfig::default()).unwrap());
    let _ = studio.update(Message::IndexReady(Arc::clone(&cloud), Ok(index)));
    assert!(studio.clouds[0].index.is_some());
    assert_eq!(stale(&studio), None);
    assert_eq!(list(&mut studio, false)["stale"], Value::Null);
    assert!(!studio.faces_export_request().unwrap().stale);
    // Faces whose points were read through an index are out of date when
    // that index is gone.
    detect(&mut studio, json!({}));
    assert_eq!(stale(&studio), None);
    studio.clouds[0].index = None;
    studio.settle_faces();
    assert_eq!(stale(&studio), Some("index"));
    // The faces are still there for the list, the scene and an export.
    assert_eq!(list(&mut studio, false)["stale"], "index");
    assert_eq!(crate::gpu_viewport::drawn_faces(&studio.clouds), [0]);
    assert!(studio.faces_export_request().unwrap().stale);
}

#[test]
fn all_visible_scans_take_part_and_the_active_one_keeps_the_faces() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    // The room in two files, as two stations would give it.
    let (left, right): (Vec<_>, Vec<_>) = wall_points()
        .into_iter()
        .partition(|point| point[0] < ROOM[0] / 2.0);
    let mut studio = Studio::default();
    for (name, points) in [("left.xyz", &left), ("right.xyz", &right)] {
        let path = directory.path().join(name);
        write_xyz(&path, points);
        open_layer(&mut studio, &path);
    }
    let place = |studio: &Studio, name: &str| {
        studio
            .clouds
            .iter()
            .position(|entry| display_name(&entry.cloud.path) == name)
            .unwrap()
    };
    let (left, right) = (place(&studio, "left.xyz"), place(&studio, "right.xyz"));
    studio.active = Some(left);

    // The active scan alone: half the floor.
    detect(&mut studio, json!({"cylinders": false}));
    let half = areas(&list(&mut studio, false), "floor")[0];
    assert!((half - 0.96).abs() < 0.03, "{half}");
    // Every visible scan: the whole floor, kept with the active scan.
    let done = detect(&mut studio, json!({"layers": "visible"}));
    assert_eq!(done["source"], "left.xyz");
    let whole = areas(&list(&mut studio, false), "floor")[0];
    assert!((whole - 1.92).abs() < 0.04, "{whole}");
    assert!(studio.clouds[left].faces.is_some() && studio.clouds[right].faces.is_none());
    assert_eq!(studio.clouds[left].faces.as_ref().unwrap().basis.len(), 2);
    let region = studio.faces_region().unwrap();
    assert_eq!((region.layers, region.boxed), (2, false));
    assert!(!region.coarse && region.streamed.is_empty());
    assert_eq!(
        region_note(&region, Layers::Visible),
        "Searches all of the visible scans (2): 1.6 × 1.2 × 1.0 m. Switch on the section box to \
         search a part of them."
    );

    // Deleted points of the other scan make the faces out of date too.
    let deleted = one_deleted(studio.clouds[right].cloud.total_points, 0);
    studio.clouds[right].deleted = Some(deleted);
    studio.settle_faces();
    assert_eq!(
        studio.clouds[left].faces.as_ref().unwrap().stale,
        Some(Stale::Points("right.xyz".into()))
    );
    detect(&mut studio, json!({}));
    assert_eq!(studio.clouds[left].faces.as_ref().unwrap().stale, None);

    // A scan that is hidden takes no part, and the active one has to.
    studio.clouds[left].visible = false;
    assert_eq!(
        studio.faces_scene(Layers::Visible).err(),
        Some(Refusal::ActiveOut("left.xyz".into()))
    );
    assert_eq!(
        send(&mut studio, command(json!({"command": "detect_faces"}))),
        json!({
            "ok": false,
            "error": "the active layer is hidden or lies outside the section box and takes no \
                      part: left.xyz",
        })
    );
    assert!(
        studio.faces_scene(Layers::Active).is_ok(),
        "alone it may be hidden"
    );
    studio.clouds[left].visible = true;
    // So with the section box around the other scan only.
    let boxed = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [1.0, 0.0, 0.0],
            max: ROOM,
        },
    );
    assert_eq!(boxed["ok"], true, "{boxed}");
    assert_eq!(
        studio.faces_scene(Layers::Visible).err(),
        Some(Refusal::ActiveOut("left.xyz".into()))
    );
    assert_eq!(
        studio.faces_scene(Layers::Active).err(),
        Some(Refusal::Outside)
    );
    let _ = send(&mut studio, ApiCommand::ClearSection);

    // Closing the other scan leaves the faces, out of date.
    studio.active = Some(left);
    let _ = studio.update(Message::Remove(right));
    let left = place(&studio, "left.xyz");
    assert_eq!(
        studio.clouds[left].faces.as_ref().unwrap().stale,
        Some(Stale::Removed("right.xyz".into()))
    );
    assert_eq!(
        status(&mut studio)["clouds"][left]["faces"]["stale"],
        "scan"
    );
}

#[test]
fn a_move_of_one_of_several_scans_makes_their_faces_out_of_date() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let (left, right): (Vec<_>, Vec<_>) = wall_points()
        .into_iter()
        .partition(|point| point[0] < ROOM[0] / 2.0);
    let mut studio = Studio::default();
    for (name, points) in [("left.xyz", &left), ("right.xyz", &right)] {
        let path = directory.path().join(name);
        write_xyz(&path, points);
        open_layer(&mut studio, &path);
    }
    let place = |studio: &Studio, name: &str| {
        studio
            .clouds
            .iter()
            .position(|entry| display_name(&entry.cloud.path) == name)
            .unwrap()
    };
    let (left, right) = (place(&studio, "left.xyz"), place(&studio, "right.xyz"));
    studio.active = Some(left);
    let stale = |studio: &Studio| studio.clouds[left].faces.as_ref().unwrap().stale.clone();
    let visible = json!({"layers": "visible", "cylinders": false});

    // The other scan is moved: its points leave the faces made from them.
    detect(&mut studio, visible.clone());
    assert_eq!(studio.clouds[left].faces.as_ref().unwrap().basis.len(), 2);
    assert_eq!(stale(&studio), None);
    studio.clouds[right].transform.offset = [10.0, 0.0, 0.0];
    studio.settle_faces();
    assert_eq!(stale(&studio), Some(Stale::Moved("right.xyz".into())));
    assert_eq!(
        stale(&studio).unwrap().sentence().english(),
        "Out of date: right.xyz was moved or scaled after these faces were detected in several \
         scans, so the scans no longer stand together as they did. Detect again."
    );
    assert_eq!(list(&mut studio, false)["stale"], "moved");
    assert_eq!(
        status(&mut studio)["clouds"][left]["faces"]["stale"],
        "moved"
    );
    assert!(studio.faces_export_request().unwrap().stale);
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    view_in_both_languages(&studio);
    // Moving it back does not bring the faces up to date.
    studio.clouds[right].transform = CloudTransform::default();
    studio.settle_faces();
    assert_eq!(stale(&studio), Some(Stale::Moved("right.xyz".into())));

    // The scan that keeps the faces is moved, through the command of the
    // window: the faces follow it, away from the points of the other scan.
    detect(&mut studio, visible.clone());
    assert_eq!(stale(&studio), None);
    let before = areas(&list(&mut studio, false), "floor");
    let moved = send(
        &mut studio,
        ApiCommand::Translate {
            offset: [0.0, 5.0, 0.0],
        },
    );
    assert_eq!(moved["ok"], true, "{moved}");
    assert_eq!(stale(&studio), Some(Stale::Moved("left.xyz".into())));
    let layer = studio.clouds[left].faces.as_ref().unwrap();
    assert_eq!(layer.placed.placement.offset, [0.0, 5.0, 0.0]);
    let after = areas(&list(&mut studio, false), "floor");
    assert!((after[0] - before[0]).abs() < 1e-9, "{after:?} {before:?}");
    assert!(studio.faces_export_request().unwrap().stale);

    // So with a scale of one of them.
    studio.clouds[left].transform = CloudTransform::default();
    detect(&mut studio, visible.clone());
    assert_eq!(stale(&studio), None);
    studio.clouds[right].transform.scale = [2.0; 3];
    studio.settle_faces();
    assert_eq!(stale(&studio), Some(Stale::Moved("right.xyz".into())));
    studio.clouds[right].transform = CloudTransform::default();

    // Faces made from one scan follow that scan and stay up to date, also
    // when every visible scan was asked for and one was in view.
    studio.clouds[right].visible = false;
    detect(&mut studio, visible);
    assert_eq!(studio.clouds[left].faces.as_ref().unwrap().basis.len(), 1);
    studio.clouds[left].transform.offset = [3.0, 0.0, 0.0];
    studio.clouds[right].transform.offset = [-3.0, 0.0, 0.0];
    studio.settle_faces();
    assert_eq!(stale(&studio), None);
    assert!(!studio.faces_export_request().unwrap().stale);
}

#[test]
fn the_list_keeps_the_cylinders_when_there_are_many_flat_faces() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    detect(&mut studio, json!({}));
    let found = Arc::clone(&studio.clouds[0].faces.as_ref().unwrap().placed);
    let rows: Vec<ListRow> = list_rows(&found).collect();
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[6].kind, "Cylinder");

    // More flat faces than the list takes, and the column after them.
    let mut many = (*found).clone();
    let wall = many.planes[0].clone();
    many.planes = (1..=LIST_ROWS as u32 + 50)
        .map(|id| PlaneFace { id, ..wall.clone() })
        .collect();
    let column = LIST_ROWS as u32 + 51;
    many.cylinders[0].id = column;
    let rows: Vec<ListRow> = list_rows(&many).collect();
    assert_eq!(rows.len(), LIST_ROWS + 1);
    assert_eq!(rows[LIST_ROWS - 1].id, LIST_ROWS as u32);
    assert_eq!(
        (rows[LIST_ROWS].id, rows[LIST_ROWS].kind),
        (column, "Cylinder")
    );
    // The block shows that list, with the note that says what is left out,
    // and the details of the column when it is chosen.
    let layer = studio.clouds[0].faces.as_mut().unwrap();
    layer.detected = Arc::new(many.clone());
    layer.placed = Arc::new(many);
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    let _ = studio.update(Message::Faces(FaceAction::Select(Some(column))));
    assert_eq!(
        studio.clouds[0].faces.as_ref().unwrap().selected,
        Some(column)
    );
    view_in_both_languages(&studio);
}

#[test]
fn a_job_is_refused_before_it_starts_when_it_cannot_run() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let mut studio = Studio::default();
    let refused = |studio: &mut Studio, more: Value| {
        let mut body = json!({"command": "detect_faces"});
        for (name, value) in more.as_object().unwrap() {
            body[name.as_str()] = value.clone();
        }
        send(studio, command(body))
    };
    assert_eq!(
        refused(&mut studio, json!({})),
        json!({"ok": false, "error": "no active cloud"})
    );
    let _ = studio.update(Message::Faces(FaceAction::Start));
    assert_eq!(
        studio.status,
        "Select a scan first: the faces are kept with it"
    );
    assert_eq!(
        studio.faces_region().err(),
        Some(Problem::Refused(Refusal::NoActive))
    );
    for command in [
        ApiCommand::ListFaces { boundaries: false },
        ApiCommand::SelectFace { id: Some(1) },
        ApiCommand::ClearFaces,
        ApiCommand::ExportFaces {
            path: directory.path().join("faces.json"),
        },
    ] {
        let answer = send(&mut studio, command);
        assert_eq!(answer["ok"], false, "{answer}");
    }
    assert_eq!(
        send(&mut studio, ApiCommand::CancelDetectFaces),
        json!({"ok": false, "error": "no face detection is running"})
    );

    open_layer(&mut studio, &write_room_xyz(directory.path()));
    // A scan that is still being read.
    let checked = Arc::clone(&studio.clouds[0].cloud);
    let mut loading = PointCloud::clone(&checked);
    loading.provisional = true;
    studio.clouds[0].cloud = Arc::new(loading);
    assert_eq!(
        refused(&mut studio, json!({})),
        json!({"ok": false, "error": "a point cloud is still loading: room.xyz"})
    );
    studio.clouds[0].cloud = checked;
    // A scan with a scale of zero has no frame to keep faces in.
    studio.clouds[0].transform.scale = [1.0, 0.0, 1.0];
    assert_eq!(
        refused(&mut studio, json!({})),
        json!({
            "ok": false,
            "error": "the scan that keeps the faces has a scale of zero: room.xyz",
        })
    );
    studio.clouds[0].transform = CloudTransform::default();
    // 3D BAG buildings are no scan.
    studio.clouds[0].bag_source = true;
    assert_eq!(
        studio.faces_scene(Layers::Visible).err(),
        Some(Refusal::BagTarget("room.xyz".into()))
    );
    studio.clouds[0].bag_source = false;
    assert!(!studio.faces.is_running());
    assert!(studio.clouds[0].faces.is_none());
    // Every refusal has its sentence in both languages.
    for refusal in [
        Refusal::NoActive,
        Refusal::NoLayer,
        Refusal::NoPoints("a.xyz".into()),
        Refusal::Loading("a.xyz".into()),
        Refusal::Outside,
        Refusal::ActiveOut("a.xyz".into()),
        Refusal::BagTarget("a.xyz".into()),
        Refusal::FlatTarget("a.xyz".into()),
    ] {
        let english = refusal.status();
        assert!(
            !refusal.api().is_empty() && !english.contains('{'),
            "{english}"
        );
        crate::i18n::set(Language::Table(0));
        let dutch = refusal.sentence().translated();
        crate::i18n::set(Language::English);
        assert!(dutch != english && !dutch.contains('{'), "{dutch}");
    }
}

#[test]
fn a_cancelled_or_failed_job_leaves_the_faces_as_they_were() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    detect(&mut studio, json!({}));
    let kept = Arc::clone(&studio.clouds[0].faces.as_ref().unwrap().detected);

    // Cancelled while it runs.
    let id = start(&mut studio, json!({}));
    assert_eq!(
        send(&mut studio, ApiCommand::CancelDetectFaces),
        json!({"ok": true, "cancel_requested": true})
    );
    assert_eq!(studio.status, "Cancelling the face detection…");
    let line = studio.faces.progress_line().unwrap();
    assert_eq!(line.title, "Cancelling…");
    assert!(line.cancel.is_none());
    let _ = studio.update(Message::Faces(FaceAction::Poll));
    assert_eq!(job(&mut studio, &id)["cancel_requested"], true);
    finish(&mut studio);
    assert_eq!(
        job(&mut studio, &id),
        json!({"state": "cancelled", "operation": "detect_faces"})
    );
    assert_eq!(
        studio.status,
        "Face detection cancelled; faces the scan had are kept"
    );
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(Arc::ptr_eq(&layer.detected, &kept));

    // Failed: the file of the scan is gone.
    let id = start(&mut studio, json!({}));
    std::fs::remove_file(directory.path().join("room.xyz")).unwrap();
    finish(&mut studio);
    let failed = job(&mut studio, &id);
    assert_eq!(failed["state"], "failed", "{failed}");
    assert!(studio.status.starts_with("Face detection failed: "));
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(Arc::ptr_eq(&layer.detected, &kept));
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    let _ = studio.view();

    // The answer of a job that was replaced is not taken for the new one.
    let _ = studio.update(Message::Faces(FaceAction::Finished(
        99,
        FaceEnd::Failed("late".into()),
    )));
    assert!(studio.status.starts_with("Face detection failed: "));
    assert!(!studio.status.contains("late"));
}

#[test]
fn a_detection_without_faces_leaves_the_scan_what_it_had() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    detect(&mut studio, json!({}));
    let kept = Arc::clone(&studio.clouds[0].faces.as_ref().unwrap().detected);
    // No face of this room is five square metres.
    let done = detect(&mut studio, json!({"min_area": 5, "cylinders": false}));
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!((&done["count"], &done["kept"]), (&json!(0), &json!(false)));
    assert!(
        studio.status.starts_with("No faces found in room.xyz: "),
        "{}",
        studio.status
    );
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    assert!(Arc::ptr_eq(&layer.detected, &kept));
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    let _ = studio.view();

    // A scan that was closed while its job ran gets nothing.
    let id = start(&mut studio, json!({"min_area": 0.25, "cylinders": true}));
    let running = studio.faces.job.as_ref().unwrap();
    let (serial, input, control) = (
        running.serial,
        Arc::clone(&running.input),
        Arc::clone(&running.control),
    );
    let end = FaceEnd::of(run(&input, &control));
    let _ = studio.update(Message::Remove(0));
    let _ = studio.update(Message::Faces(FaceAction::Finished(serial, end)));
    let done = job(&mut studio, &id);
    assert_eq!((&done["count"], &done["kept"]), (&json!(7), &json!(false)));
    assert_eq!(done["source"], Value::Null);
    assert!(
        studio
            .status
            .starts_with("Detected 7 faces, but their scan was closed and nothing is kept: "),
        "{}",
        studio.status
    );
}

#[test]
fn faces_are_exported_as_json_and_obj_in_scene_coordinates() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let export = |studio: &mut Studio, path: &Path| {
        send(
            studio,
            ApiCommand::ExportFaces {
                path: path.to_path_buf(),
            },
        )
    };
    let json_path = directory.path().join("out").join("faces.json");
    std::fs::create_dir(json_path.parent().unwrap()).unwrap();
    assert!(!studio.faces_entry_enabled());
    assert_eq!(
        export(&mut studio, &json_path),
        json!({"ok": false, "error": "the active layer has no detected faces"})
    );
    let _ = studio.update(Message::Faces(FaceAction::Export));
    assert_eq!(studio.status, "Detect faces in the active scan first");

    detect(&mut studio, json!({}));
    assert!(studio.faces_entry_enabled());
    // What cannot be written is refused before anything is started.
    for (path, error) in [
        (
            PathBuf::from("faces.json"),
            "export_faces requires an absolute .json or .obj destination",
        ),
        (
            directory.path().join("faces.ply"),
            "export_faces requires an absolute .json or .obj destination",
        ),
        (
            directory.path().join("room.xyz").with_extension("XYZ"),
            "export_faces requires an absolute .json or .obj destination",
        ),
        (
            directory.path().join("missing").join("faces.obj"),
            "the folder of the faces destination does not exist",
        ),
    ] {
        assert_eq!(
            export(&mut studio, &path),
            json!({"ok": false, "error": error}),
            "{}",
            path.display()
        );
    }
    assert!(!studio.faces.export_pending);

    // The scan stands somewhere else by the time it is exported.
    studio.clouds[0].transform.offset = [100.0, 200.0, 0.0];
    studio.settle_faces();
    let accepted = export(&mut studio, &json_path);
    assert_eq!(accepted["ok"], true, "{accepted}");
    let id = accepted["job_id"].as_str().unwrap().to_owned();
    assert_eq!(job(&mut studio, &id)["state"], "running");
    assert_eq!(busy(&status(&mut studio)), ["faces_export"]);
    assert!(!studio.faces_entry_enabled(), "one export at a time");
    assert_eq!(
        export(&mut studio, &json_path)["error"],
        "a faces export is already open or running"
    );
    finish_export(&mut studio, &id, &json_path);
    let done = job(&mut studio, &id);
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(
        (
            &done["format"],
            &done["planes"],
            &done["cylinders"],
            &done["edges"]
        ),
        (&json!("json"), &json!(6), &json!(1), &json!(12))
    );
    assert_eq!(done["stale"], false);
    assert!(done["bytes"].as_u64().unwrap() > 1_000);
    assert!(busy(&status(&mut studio)).is_empty());
    assert!(
        studio
            .status
            .starts_with("Exported 6 flat faces and 1 cylinder as JSON to "),
        "{}",
        studio.status
    );

    let document: Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    assert_eq!(document["format"], FACES_JSON_FORMAT);
    assert_eq!(document["version"], FACES_JSON_VERSION);
    assert_eq!(document["source"], "room.xyz", "the name, not the folder");
    assert!(!std::fs::read_to_string(&json_path)
        .unwrap()
        .contains(directory.path().file_name().unwrap().to_str().unwrap()));
    assert_eq!(document["units"], "metres");
    assert_eq!(document["faces"].as_array().unwrap().len(), 7);
    assert_eq!(document["edges"].as_array().unwrap().len(), 12);
    assert_eq!(document["settings"]["distance_tolerance"], 0.02);
    // In scene coordinates: where the scan stands now.
    assert!(
        near(&document["region"]["min"][0], 100.0, 0.02),
        "{}",
        document["region"]
    );
    assert!(near(&document["region"]["max"][1], 200.0 + ROOM[1], 0.02));
    let column = &document["faces"][6];
    assert!(
        near(&column["axis_start"][0], 100.0 + COLUMN[0], 0.005),
        "{column}"
    );
    assert!(
        near(&column["axis_start"][1], 200.0 + COLUMN[1], 0.005),
        "{column}"
    );

    // The same faces as a mesh, a group per face.
    let obj_path = directory.path().join("faces.obj");
    let accepted = export(&mut studio, &obj_path);
    let id = accepted["job_id"].as_str().unwrap().to_owned();
    finish_export(&mut studio, &id, &obj_path);
    assert_eq!(job(&mut studio, &id)["format"], "obj");
    let text = std::fs::read_to_string(&obj_path).unwrap();
    let groups: Vec<&str> = text.lines().filter(|line| line.starts_with("g ")).collect();
    assert_eq!(groups.len(), 7, "{groups:?}");
    assert!(groups[0].starts_with("g face_0001_"));
    assert_eq!(groups[6], "g face_0007_cylinder");
    assert!(text.contains("# Source: room.xyz\n"));
    let mesh = pointcloud_core::read_mesh_geometry(&obj_path)
        .unwrap()
        .unwrap();
    assert!(mesh
        .vertices
        .iter()
        .all(|vertex| vertex[0] > 99.9 && vertex[1] > 199.9));
    // An OBJ that is open as a layer cannot be the destination.
    open_layer(&mut studio, &obj_path);
    let room = studio
        .clouds
        .iter()
        .position(|entry| entry.faces.is_some())
        .unwrap();
    studio.active = Some(room);
    assert_eq!(
        export(&mut studio, &obj_path)["error"],
        "export_faces requires a destination different from the open scans"
    );

    // Faces that are out of date say so in their file and their job.
    let deleted = one_deleted(studio.clouds[room].cloud.total_points, 0);
    studio.clouds[room].deleted = Some(deleted);
    studio.settle_faces();
    let stale_path = directory.path().join("stale.obj");
    let accepted = export(&mut studio, &stale_path);
    let id = accepted["job_id"].as_str().unwrap().to_owned();
    finish_export(&mut studio, &id, &stale_path);
    assert_eq!(job(&mut studio, &id)["stale"], true);
    assert!(std::fs::read_to_string(&stale_path)
        .unwrap()
        .contains("# The points of the scan changed after the faces were detected\n"));
    assert!(studio
        .status
        .ends_with("the points changed after the detection"));

    // A failed write is reported and frees the export.
    let _ = studio.update(Message::Faces(FaceAction::Exported(
        None,
        Err("disk full".into()),
    )));
    assert_eq!(studio.status, "Faces export failed: disk full");

    // The save dialog: a name without a known extension is refused, a
    // cancelled dialog writes nothing.
    let request = studio.faces_export_request().unwrap();
    studio.faces.export_pending = true;
    let _ = studio.update(Message::Faces(FaceAction::PathChosen(
        request.clone(),
        None,
    )));
    assert_eq!(studio.status, "Faces export cancelled");
    studio.faces.export_pending = true;
    let _ = studio.update(Message::Faces(FaceAction::PathChosen(
        request.clone(),
        Some(directory.path().join("faces.txt")),
    )));
    assert_eq!(
        studio.status,
        "Choose a .json or .obj file name for the faces"
    );
    assert!(!studio.faces.export_pending);
    let _ = studio.update(Message::Faces(FaceAction::PathChosen(
        request,
        Some(directory.path().join("dialog.JSON")),
    )));
    assert!(studio.faces.export_pending);
    assert_eq!(studio.status, "Writing 7 faces as JSON…");
}

#[test]
fn stations_a_job_found_are_kept_for_the_next_job_of_either_tool() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_room_ptx(directory.path());
    // A cloud as an index cache of an earlier version gives it: with its
    // station, without knowing which points that station measured.
    let mut cloud = pointcloud_core::open(&path, 1_000).unwrap();
    assert!(cloud.scan_ranges_known());
    cloud.forget_scan_ranges();
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
    let id = start(&mut studio, json!({}));
    let stages = studio.faces.job.as_ref().unwrap().stages();
    assert_eq!(stages[0], Stage::Stations);
    let _ = studio.update(Message::Faces(FaceAction::Poll));
    assert_eq!(job(&mut studio, &id)["stage"], "stations");
    finish(&mut studio);
    let done = job(&mut studio, &id);
    assert_eq!(
        (&done["count"], &done["cylinders"]),
        (&json!(6), &json!(0)),
        "{done}"
    );
    // The side of every face came from the station that measured it.
    let listing = list(&mut studio, false);
    for face in listing["faces"].as_array().unwrap() {
        assert_eq!(face["normal_from"], "stations", "{face}");
    }
    // The next job, of this tool or of Closed mesh, reads them from the
    // cloud that was found.
    let found = studio.closed_mesh.stationed(&studio.clouds[0].cloud);
    assert!(found.scan_ranges_known());
    let _ = start(&mut studio, json!({}));
    let job = studio.faces.job.as_ref().unwrap();
    assert!(!job.stages().contains(&Stage::Stations));
    assert!(Arc::ptr_eq(&job.input.scene.layers[0].cloud, &found));
    finish(&mut studio);
}

#[test]
fn block_says_what_a_job_would_search_and_what_voxel_the_budget_gives() {
    let _language = TestLanguage::hold(Language::English);
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    // The button opens the block, and closes it.
    assert!(studio.faces_properties().is_none());
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    assert!(studio.faces.open && studio.faces_properties().is_some());
    assert!(studio
        .status
        .starts_with("Detect faces: put the section box"));
    let region = studio.faces_region().unwrap();
    assert!(!region.boxed && !region.coarse);
    assert_eq!(
        (region.layers, region.voxel, region.budget),
        (1, 0.03, 1_500_000)
    );
    assert_eq!(
        region_note(&region, Layers::Active),
        "Searches all of the active scan: 1.6 × 1.2 × 1.0 m. Switch on the section box to \
         search a part of it."
    );
    // The section box, cut back to where the scan has points.
    let boxed = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [0.0; 3],
            max: [0.8, ROOM[1], ROOM[2]],
        },
    );
    assert_eq!(boxed["ok"], true, "{boxed}");
    let region = studio.faces_region().unwrap();
    assert!(region.boxed);
    assert!(
        (region.bounds.max[0] - 0.8).abs() < 0.02,
        "{:?}",
        region.bounds
    );
    assert!(region_note(&region, Layers::Active)
        .starts_with("Searches the active scan inside the section box: 0.8 × 1.2 × 1.0 m."));
    let _ = send(&mut studio, ApiCommand::ClearSection);

    // The voxel the budget gives: the size a job starts with for a region
    // whose faces fit, doubled until they do for a larger one.
    let config = SurfaceDetectConfig::default();
    let cube = |edge: f64| Bounds {
        min: [0.0; 3],
        max: [edge; 3],
    };
    let many = 1_000_000_000;
    // Six faces of 10 by 10 m hold 667,000 voxels of 3 cm.
    assert_eq!(expected_voxel(cube(10.0), many, &config), 0.03);
    // Of 20 by 20 m they hold 2.7 million: one doubling.
    assert_eq!(expected_voxel(cube(20.0), many, &config), 0.06);
    assert_eq!(expected_voxel(cube(100.0), many, &config), 0.24);
    // A scan with fewer points than the budget never needs larger voxels.
    assert_eq!(expected_voxel(cube(100.0), 1_500_000, &config), 0.03);
    assert_eq!(expected_voxel(cube(100.0), 1_500_001, &config), 0.24);

    // A scan without an index that is too large to hold in memory is read
    // from its file twice, which the block says; and a region whose faces
    // do not fit the budget gets a warning with the voxel it needs.
    let checked = Arc::clone(&studio.clouds[0].cloud);
    let mut large = PointCloud::clone(&checked);
    large.total_points = UNINDEXED_LIMIT + 1;
    studio.clouds[0].cloud = Arc::new(large);
    studio.clouds[0].transform.scale = [100.0; 3];
    let region = studio.faces_region().unwrap();
    assert_eq!(region.streamed, ["room.xyz"]);
    assert!(region.coarse);
    assert_eq!(region.voxel, 0.48, "160 by 120 by 100 m of faces");
    let scene = studio.faces_scene(Layers::Active).unwrap();
    assert!(scene.layers[0].streamed() && !scene.layers[0].resident());
    view_in_both_languages(&studio);
    studio.clouds[0].cloud = checked;
    studio.clouds[0].transform = CloudTransform::default();
    let scene = studio.faces_scene(Layers::Active).unwrap();
    assert!(scene.layers[0].resident() && !scene.layers[0].streamed());

    // A setting that cannot be read is said in the place of the region.
    let _ = studio.update(Message::Faces(FaceAction::Distance("wide".into())));
    assert!(matches!(studio.faces_region(), Err(Problem::Setting(_))));
    let _ = studio.update(Message::Faces(FaceAction::Start));
    assert_eq!(
        studio.status,
        "Distance tolerance must be a number of millimetres"
    );
    assert!(!studio.faces.is_running());
    view_in_both_languages(&studio);
    // Every field of the block reaches the settings.
    for action in [
        FaceAction::Distance("15".into()),
        FaceAction::Angle("12".into()),
        FaceAction::MinArea("0,5".into()),
        FaceAction::Cylinders(false),
        FaceAction::Layers(Layers::Visible),
    ] {
        let _ = studio.update(Message::Faces(action));
    }
    assert_eq!(
        studio.faces.settings.value(),
        json!({
            "distance_tolerance": 0.015, "angle_tolerance": 12.0, "min_area": 0.5,
            "cylinders": false, "layers": "visible", "color": "face",
        })
    );
    // Start from the block runs a job without a job of the local API.
    let _ = studio.update(Message::Faces(FaceAction::Start));
    assert!(studio.faces.is_running());
    assert!(studio.faces.job.as_ref().unwrap().api_job_id.is_none());
    let _ = studio.update(Message::Faces(FaceAction::Start));
    assert_eq!(studio.status, "Faces are already being detected");
    view_in_both_languages(&studio);
    finish(&mut studio);
    assert_eq!(
        studio.clouds[0].faces.as_ref().unwrap().summary().count(),
        6
    );
    let _ = studio.update(Message::Faces(FaceAction::Clear));
    assert_eq!(studio.status, "Faces of room.xyz cleared");
    let _ = studio.update(Message::Faces(FaceAction::Clear));
    assert_eq!(studio.status, "The active scan has no faces to clear");
    let _ = studio.update(Message::Faces(FaceAction::Toggle));
    assert!(!studio.faces.open);
}

#[test]
fn stages_and_results_are_worded_for_the_status_bar() {
    let step = |stage, done, total| Step { stage, done, total }.text();
    assert_eq!(
        step(Stage::Stations, 5_000, 20_000),
        "finding the station of every point, 5000 of 20k points"
    );
    assert_eq!(
        step(Stage::Loading, 30_000, 20_000),
        "reading a scan without an index, 20k of 20k points"
    );
    assert_eq!(
        step(Stage::Reading, 1_200_000, 5_800_000),
        "reading the points, 1.2M of 5.8M points"
    );
    assert_eq!(step(Stage::Segmenting, 1, 3), "finding flat regions, 33%");
    assert_eq!(step(Stage::Segmenting, 0, 0), "finding flat regions");
    assert_eq!(step(Stage::Outlining, 3, 9), "tracing outline 3 of 9");
    assert_eq!(
        step(Stage::Meshing, 0, 0),
        "building the faces for the viewer"
    );
    // Every stage has a name of its own for the local API.
    let names: std::collections::BTreeSet<_> = Stage::ALL.into_iter().map(Stage::name).collect();
    assert_eq!(names.len(), Stage::ALL.len());

    let summary = Summary {
        floors: 1,
        ceilings: 2,
        walls: 5,
        sloped: 0,
        cylinders: 1,
        edges: 14,
        voxel: 0.03,
        coarse: false,
        density_doublings: 0,
        read_points: 10,
        source_points: 9,
        working_points: 8,
        assigned_points: 7,
        region: None,
        seconds: 1.26,
    };
    assert_eq!((summary.planes(), summary.count()), (8, 9));
    assert_eq!(summary.counts(), "1 floor, 2 ceilings, 5 walls, 1 cylinder");
    assert_eq!(
        summary.line(),
        "1 floor, 2 ceilings, 5 walls, 1 cylinder; 1.3 s; voxels of 30 mm"
    );
    // Larger voxels are said with their reason: a region beyond the budget
    // can be made smaller, a thin scan cannot.
    let large = Summary {
        voxel: 0.12,
        coarse: true,
        ..summary.clone()
    };
    assert!(large.line().ends_with(
        "; the region is large, so voxels of 120 mm were used and narrow faces and faces close \
         together are lost: a smaller section box brings them back"
    ));
    let thin = Summary {
        voxel: 0.06,
        coarse: true,
        density_doublings: 1,
        ..summary
    };
    assert!(thin.line().ends_with(
        "; the points lie far apart, so voxels of 60 mm were used and narrow faces are lost"
    ));
    assert_eq!(thin.value()["density_doublings"], 1);
    assert_eq!(tidy(-0.0004), 0.0);
    assert!(tidy(-0.0004).is_sign_positive());
    assert_eq!(tidy(-0.0006), -0.001);
}

#[test]
fn a_highlighted_cylinder_is_drawn_along_its_scanned_part() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    detect(&mut studio, json!({}));
    let layer = studio.clouds[0].faces.as_ref().unwrap();
    let column = layer.placed.cylinder(7).unwrap();
    let edges = cylinder_edges(column);
    assert_eq!(edges[0], [column.axis_start, column.axis_end]);
    // Every other line lies on the surface.
    for [from, to] in &edges[1..] {
        for point in [from, to] {
            let radius = (point[0] - COLUMN[0]).hypot(point[1] - COLUMN[1]);
            assert!((radius - COLUMN_RADIUS).abs() < 0.003, "{radius}");
        }
    }
    // The arcs at both ends and halfway, and the lines along the column:
    // all round, 48 strips per arc and a line every 30 degrees.
    assert_eq!(column.arc_deg, 360.0);
    assert_eq!(edges.len(), 1 + 3 * 48 + 13);

    // A column scanned over a third of its round is drawn over that third
    // only, from where its arc begins.
    let part = CylinderFace {
        arc_deg: 120.0,
        ..column.clone()
    };
    let edges = cylinder_edges(&part);
    assert_eq!(edges.len(), 1 + 3 * 16 + 5);
    let (mut least, mut most) = (f64::MAX, f64::MIN);
    for point in edges[1..].iter().flatten() {
        // Both directions are square to the axis, so the part of the point
        // along the axis drops out.
        let out: [f64; 3] = std::array::from_fn(|axis| point[axis] - part.axis_start[axis]);
        let dot = |direction: [f64; 3]| (0..3).map(|axis| out[axis] * direction[axis]).sum::<f64>();
        let angle = dot(part.arc_side).atan2(dot(part.arc_start)).to_degrees();
        assert!((-1e-6..=120.0 + 1e-6).contains(&angle), "{angle}");
        (least, most) = (least.min(angle), most.max(angle));
    }
    assert!(
        least.abs() < 1e-6 && (most - 120.0).abs() < 1e-6,
        "{least} {most}"
    );
}

#[test]
fn command_line_detects_the_faces_of_a_box_of_a_scan_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = write_room_xyz(directory.path());
    let arguments = |values: &[&str]| -> Vec<OsString> {
        values.iter().map(|value| OsString::from(*value)).collect()
    };
    let (source, folder) = (source.to_str().unwrap(), directory.path());
    let json_path = folder.join("faces.json");
    let lines = command_line(&arguments(&[source, json_path.to_str().unwrap()])).unwrap();
    let mut printed = lines.lines();
    let first = printed.next().unwrap();
    assert!(
        first.starts_with("Detected 7 faces: 1 floor, 1 ceiling, 4 walls, 1 cylinder; "),
        "{first}"
    );
    assert!(first.contains("; 12 edges; "), "{first}");
    assert!(first.contains("faces.json (JSON, "), "{first}");
    let faces: Vec<&str> = printed.collect();
    assert_eq!(faces.len(), 7, "{lines}");
    assert!(
        faces[0].contains("1  floor") || faces[0].contains("1  ceiling"),
        "{}",
        faces[0]
    );
    assert!(faces[0].contains("1 part, 0 openings"), "{}", faces[0]);
    assert!(faces[6].contains("cylinder diameter 0.30"), "{}", faces[6]);
    assert!(faces[6].contains("seen from outside"), "{}", faces[6]);
    assert!(!lines.contains("-0.000"), "{lines}");
    let document: Value = serde_json::from_slice(&std::fs::read(&json_path).unwrap()).unwrap();
    assert_eq!(document["faces"].as_array().unwrap().len(), 7);
    assert_eq!(document["source"], "room.xyz");

    // A box and the settings of the block, written as OBJ.
    let obj_path = folder.join("half.obj");
    let lines = command_line(&arguments(&[
        source,
        obj_path.to_str().unwrap(),
        "--box",
        "0.9,-1,-1,2,2,2",
        "--distance",
        "0.01",
        "--angle",
        "8",
        "--min-area",
        "0.3",
        "--cylinders",
        "off",
    ]))
    .unwrap();
    let first = lines.lines().next().unwrap();
    assert!(
        first.starts_with("Detected 5 faces: 1 floor, 1 ceiling, 3 walls; "),
        "{first}"
    );
    assert!(first.contains("half.obj (OBJ, "), "{first}");
    let text = std::fs::read_to_string(&obj_path).unwrap();
    assert_eq!(
        text.lines().filter(|line| line.starts_with("g ")).count(),
        5
    );

    // What is wrong is said before the scan is read, with the exit code.
    let refused = |values: &[&str]| command_line(&arguments(values)).unwrap_err();
    let out = json_path.to_str().unwrap();
    assert_eq!(refused(&[source]), (2, String::new()), "the usage line");
    assert_eq!(refused(&[source, out, "--box"]), (2, String::new()));
    assert_eq!(
        refused(&[source, out, "--voxel", "0.01"]),
        (2, String::new())
    );
    assert_eq!(
        refused(&[source, "faces.ply"]),
        (2, "Supported faces extensions: .json, .obj".into())
    );
    assert_eq!(
        refused(&[out, out]),
        (2, "Choose an output path different from the input".into())
    );
    assert_eq!(
        refused(&[source, out, "--box", "1,2,3"]),
        (2, "--box must be six comma-separated numbers".into())
    );
    assert_eq!(
        refused(&[source, out, "--distance", "2 cm"]),
        (2, "--distance must be a number of metres".into())
    );
    // The option is in metres, and so is what is said about its range.
    for distance in ["20", "0.0005"] {
        assert_eq!(
            refused(&[source, out, "--distance", distance]),
            (2, "--distance must lie between 0.001 and 0.5 metres".into())
        );
    }
    assert_eq!(
        refused(&[source, out, "--angle", "90"]),
        (
            2,
            "The angle tolerance must lie between 1 and 45 degrees".into()
        )
    );
    assert_eq!(
        refused(&[source, out, "--cylinders", "yes"]),
        (2, "--cylinders must be on or off".into())
    );
    let missing = folder.join("missing").join("faces.json");
    assert_eq!(
        refused(&[source, missing.to_str().unwrap()]),
        (2, "The folder of the output path does not exist".into())
    );
    // Nothing found writes nothing and fails.
    let empty = folder.join("empty.json");
    let (code, line) = refused(&[
        source,
        empty.to_str().unwrap(),
        "--min-area",
        "5",
        "--cylinders",
        "off",
    ]);
    assert_eq!(code, 1);
    assert!(line.starts_with("No faces found: "), "{line}");
    assert!(!empty.exists());
    let (code, line) = refused(&[folder.join("none.xyz").to_str().unwrap(), out]);
    assert_eq!(code, 1);
    assert!(line.starts_with("Face detection failed: "), "{line}");
}
