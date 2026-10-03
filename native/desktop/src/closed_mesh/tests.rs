//! Tests of the Closed mesh tool, on a generated room.

use std::path::Path;

use pointcloud_core::{MeshTopology, DEFAULT_CLOSED_MESH_TRIANGLES, DEFAULT_CLOSED_MESH_VERTICES};

use super::*;
use crate::i18n::{Language, TestLanguage};
use crate::mesh_export::in_scene;
use crate::native_api::{ApiCommand, ApiRequest};

/// The inside of the room: 1.6 by 1.2 m and 1.0 m high, with one corner at
/// zero.
const ROOM: [f64; 3] = [1.6, 1.2, 1.0];
/// Voxels of 4 cm keep a job on this room well under a second.
const VOXEL: f64 = 0.04;

/// Points on the six inner faces of the room, one per centimetre. They lie
/// half a step from the edges, so no two faces share a point.
fn room_points() -> Vec<[f64; 3]> {
    const STEP: f64 = 0.01;
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

/// The room as an XYZ file, which knows no station.
fn write_room_xyz(directory: &Path) -> PathBuf {
    let text: String = room_points()
        .iter()
        .map(|[x, y, z]| format!("{x:.4} {y:.4} {z:.4}\n"))
        .collect();
    let path = directory.join("room.xyz");
    std::fs::write(&path, text).unwrap();
    path
}

/// The room as a PTX scan measured from one station inside it.
fn write_room_ptx(directory: &Path) -> PathBuf {
    let station = [0.6, 0.5, 0.4];
    let points = room_points();
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

/// A window with the room open as its one layer.
fn studio_with_room(directory: &Path) -> Studio {
    let path = write_room_xyz(directory);
    let cloud = Arc::new(pointcloud_core::open(&path, 1_000).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
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
    let running = studio.closed_mesh.job.as_ref().expect("a job runs");
    let (serial, input, control) = (
        running.serial,
        Arc::clone(&running.input),
        Arc::clone(&running.control),
    );
    let end = ClosedMeshEnd::of(run(&input, &control));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Finished(serial, end)));
}

/// Start a closed mesh of the room with voxels of 4 cm and return its job.
fn start(studio: &mut Studio, more: Value) -> String {
    let mut body = json!({"command": "mesh", "mode": "closed", "voxel": VOXEL});
    for (name, value) in more.as_object().unwrap() {
        body[name.as_str()] = value.clone();
    }
    let accepted = send(studio, command(body));
    assert_eq!(accepted["ok"], true, "{accepted}");
    accepted["job_id"].as_str().unwrap().to_owned()
}

/// The box around the vertices of a mesh.
fn mesh_bounds(mesh: &MeshGeometry) -> Bounds {
    let mut bounds = Bounds {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };
    for vertex in &mesh.vertices {
        for (axis, value) in vertex.iter().enumerate() {
            bounds.min[axis] = bounds.min[axis].min(*value);
            bounds.max[axis] = bounds.max[axis].max(*value);
        }
    }
    bounds
}

/// The share of the triangles whose front, by the order of their corners,
/// looks at a point.
fn share_facing(mesh: &MeshGeometry, point: [f64; 3]) -> f64 {
    let facing = mesh
        .triangles
        .iter()
        .filter(|triangle| {
            let [a, b, c] = triangle.map(|index| mesh.vertices[index as usize]);
            let (ab, ac): ([f64; 3], [f64; 3]) = (
                std::array::from_fn(|axis| b[axis] - a[axis]),
                std::array::from_fn(|axis| c[axis] - a[axis]),
            );
            let normal = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            (0..3)
                .map(|axis| normal[axis] * (point[axis] - a[axis]))
                .sum::<f64>()
                > 0.0
        })
        .count();
    facing as f64 / mesh.triangles.len() as f64
}

#[test]
fn block_starts_with_the_defaults_of_the_core() {
    let settings = ClosedMeshSettings::default();
    assert_eq!(settings.voxel, "", "the job chooses the voxel");
    assert_eq!(settings.max_hole, "0.25");
    assert_eq!(settings.simplify, "", "the job chooses the tolerance");
    assert_eq!(
        (settings.sides, settings.layers),
        (Sides::Automatic, Layers::Active)
    );
    let config = settings.config().unwrap();
    assert_eq!(config, ClosedMeshConfig::default());
    // A job may make what a mesh may hold: the limits of the mesh files.
    assert_eq!(
        (config.max_vertices, config.max_triangles),
        (MAX_MESH_VERTICES, MAX_MESH_TRIANGLES)
    );
    assert_eq!(
        (DEFAULT_CLOSED_MESH_VERTICES, DEFAULT_CLOSED_MESH_TRIANGLES),
        (4_000_000, 8_000_000)
    );
    assert_eq!(
        settings.value(),
        json!({
            "voxel": null, "max_hole": 0.25, "simplify_mm": null,
            "sides": "automatic", "layers": "active",
        })
    );
    // Every choice has a name of its own for a command, and reads back.
    for sides in Sides::ALL {
        assert_eq!(Sides::from_name(sides.name()), Some(sides));
    }
    assert_eq!(Sides::from_name("center"), Some(Sides::Centre));
    for layers in Layers::ALL {
        assert_eq!(Layers::from_name(layers.name()), Some(layers));
    }
}

#[test]
fn typed_numbers_are_read_with_a_comma_or_a_point_and_checked() {
    let mut settings = ClosedMeshSettings {
        voxel: " 0,03 ".into(),
        max_hole: "0,5".into(),
        simplify: "4,5".into(),
        ..ClosedMeshSettings::default()
    };
    let config = settings.config().unwrap();
    assert_eq!(config.voxel, Some(0.03));
    assert_eq!(config.max_hole, 0.5);
    assert_eq!(config.simplify_tolerance, Some(0.0045));
    assert_eq!(settings.value()["simplify_mm"], 4.5);

    // Empty and "auto" leave a value to the job; 0 switches simplifying off.
    settings.voxel = "Auto".into();
    settings.simplify = "0".into();
    let config = settings.config().unwrap();
    assert_eq!(config.voxel, None);
    assert_eq!(config.simplify_tolerance, Some(0.0));
    settings.simplify = " ".into();
    assert_eq!(settings.config().unwrap().simplify_tolerance, None);

    let refused = |change: fn(&mut ClosedMeshSettings)| {
        let mut settings = ClosedMeshSettings::default();
        change(&mut settings);
        settings.config().unwrap_err().english()
    };
    assert_eq!(
        refused(|settings| settings.voxel = "fine".into()),
        "Voxel size must be a number of metres, or empty for automatic"
    );
    assert_eq!(
        refused(|settings| settings.voxel = "0.004".into()),
        "The voxel size must lie between 0.005 and 0.5 m"
    );
    assert_eq!(
        refused(|settings| settings.max_hole = "4".into()),
        "The hole limit must lie between 0 and 3.2 m"
    );
    assert_eq!(
        refused(|settings| settings.max_hole = String::new()),
        "Hole limit must be a number of metres"
    );
    assert_eq!(
        refused(|settings| settings.simplify = "2000".into()),
        "The simplification tolerance must lie between 0 and 1 m"
    );
    assert!(refused(|settings| settings.simplify = "some".into())
        .starts_with("Simplification must be a number of millimetres"));
    // The limits are those of the core, which refuses the same values in
    // the same words.
    let core = |change: fn(&mut ClosedMeshConfig)| {
        let mut config = ClosedMeshConfig::default();
        change(&mut config);
        let reason = config.validate().unwrap_err().to_string();
        plain_reason(&reason).to_owned()
    };
    assert_eq!(
        core(|config| config.voxel = Some(0.6)),
        uncapitalised(&refused(|settings| settings.voxel = "0.6".into()))
    );
    assert_eq!(
        core(|config| config.max_hole = 4.0),
        uncapitalised(&refused(|settings| settings.max_hole = "4".into()))
    );
    assert_eq!(
        core(|config| config.simplify_tolerance = Some(2.0)),
        uncapitalised(&refused(|settings| settings.simplify = "2000".into()))
    );
    // The block says a setting that is wrong in the language in use.
    let _language = TestLanguage::hold(Language::Table(0));
    let problem = ClosedMeshSettings {
        voxel: "0,6".into(),
        ..ClosedMeshSettings::default()
    }
    .config()
    .unwrap_err();
    assert_eq!(
        problem.translated(),
        "De voxelgrootte moet tussen 0.005 en 0.5 m liggen"
    );
    let dutch = |change: fn(&mut ClosedMeshSettings)| {
        let mut settings = ClosedMeshSettings::default();
        change(&mut settings);
        settings.config().unwrap_err().translated()
    };
    assert_eq!(
        dutch(|settings| settings.voxel = "x".into()),
        "Voxelgrootte moet een getal in meters zijn, of leeg voor automatisch"
    );
    assert_eq!(
        dutch(|settings| settings.max_hole = "x".into()),
        "De gatgrens moet een getal in meters zijn"
    );
    assert_eq!(
        dutch(|settings| settings.max_hole = "9".into()),
        "De gatgrens moet tussen 0 en 3.2 m liggen"
    );
    assert_eq!(
        dutch(|settings| settings.simplify = "x".into()),
        "Vereenvoudiging moet een getal in millimeters zijn, leeg voor automatisch of 0 voor geen"
    );
    assert_eq!(
        dutch(|settings| settings.simplify = "2000".into()),
        "De vereenvoudigingstolerantie moet tussen 0 en 1 m liggen"
    );
    // A field that holds no number is null in the status.
    settings.max_hole = "wide".into();
    assert_eq!(settings.value()["max_hole"], Value::Null);
}

#[test]
fn sides_decide_whether_stations_are_asked() {
    let config = |sides| {
        ClosedMeshSettings {
            sides,
            ..ClosedMeshSettings::default()
        }
        .config()
        .unwrap()
    };
    let automatic = config(Sides::Automatic);
    assert!(automatic.use_stations);
    assert_eq!(automatic.orientation, MeshOrientation::Automatic);
    // The centre is that of the region, which the core takes itself.
    let centre = config(Sides::Centre);
    assert!(!centre.use_stations);
    assert_eq!(centre.orientation, MeshOrientation::Automatic);
    let upward = config(Sides::Upward);
    assert!(!upward.use_stations);
    assert_eq!(upward.orientation, MeshOrientation::Upward);
}

#[test]
fn fields_of_a_command_go_into_the_block_and_wrong_ones_change_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let set = |studio: &mut Studio, body: Value| {
        let mut body = body;
        body["command"] = "set_closed_mesh_settings".into();
        send(studio, command(body))
    };

    let answer = set(
        &mut studio,
        json!({
            "voxel": 0.05, "max_hole": 0.1, "simplify_mm": 0,
            "sides": "Centre", "layers": "visible",
        }),
    );
    let expected = json!({
        "voxel": 0.05, "max_hole": 0.1, "simplify_mm": 0.0,
        "sides": "centre", "layers": "visible",
    });
    assert_eq!(answer, json!({"ok": true, "settings": expected}));
    assert_eq!(status(&mut studio)["closed_mesh"]["settings"], expected);
    assert_eq!(studio.closed_mesh.settings.voxel, "0.05");

    // A field that is left out keeps its value, null hands it to the job.
    let answer = set(&mut studio, json!({"voxel": null, "simplify_mm": null}));
    assert_eq!(answer["settings"]["voxel"], Value::Null);
    assert_eq!(answer["settings"]["simplify_mm"], Value::Null);
    assert_eq!(answer["settings"]["max_hole"], 0.1);
    assert_eq!(answer["settings"]["sides"], "centre");
    // Nothing named changes nothing and reports the settings.
    assert_eq!(set(&mut studio, json!({}))["settings"], answer["settings"]);

    // When one field is refused, none is taken.
    let before = studio.closed_mesh.settings.clone();
    for (body, problem) in [
        (
            json!({"max_hole": 0.3, "voxel": 0.6}),
            "the voxel size must lie between 0.005 and 0.5 m",
        ),
        (
            json!({"max_hole": 5.0}),
            "the hole limit must lie between 0 and 3.2 m",
        ),
        (
            json!({"voxel": 0.02, "sides": "inward"}),
            "sides must be automatic, centre or upward",
        ),
        (json!({"layers": "all"}), "layers must be active or visible"),
    ] {
        let answer = set(&mut studio, body);
        assert_eq!(answer, json!({"ok": false, "error": problem}));
        assert_eq!(studio.closed_mesh.settings, before);
    }

    // The settings go with the mode closed only, and that mode needs no
    // file where the other two do.
    let terrain = send(
        &mut studio,
        command(json!({"command": "mesh", "mode": "terrain", "path": "/m.obj", "voxel": 0.02})),
    );
    assert_eq!(
        terrain["error"],
        "voxel, max_hole, simplify_mm, sides and layers go with the mesh mode closed only"
    );
    let no_file = send(
        &mut studio,
        command(json!({"command": "mesh", "mode": "surface"})),
    );
    assert_eq!(
        no_file["error"],
        "mesh requires an absolute .obj destination"
    );
    let unknown = send(
        &mut studio,
        command(json!({
            "command": "mesh", "mode": "solid",
            "path": directory.path().join("m.obj"),
        })),
    );
    assert!(unknown["error"].as_str().unwrap().contains("closed"));
    assert!(studio.api_jobs.is_empty(), "a refusal starts no job");
}

#[test]
fn mesh_is_kept_in_the_frame_of_its_scan() {
    let scene = || MeshGeometry {
        vertices: vec![
            [10.0, 20.0, 1.0],
            [12.0, 20.0, 1.0],
            [12.0, 21.0, 1.5],
            [10.0, 21.0, 1.5],
        ],
        triangles: vec![[0, 1, 2], [0, 2, 3]],
        colors: Some(vec![[200, 10, 10]; 4]),
        normals: Some(vec![[0.0, -0.447_213_6, 0.894_427_2]; 4]),
    };
    for transform in [
        CloudTransform::default(),
        CloudTransform {
            scale: [1.0; 3],
            offset: [207_000.25, 474_000.5, 10.0],
        },
        CloudTransform {
            scale: [2.0, 0.5, 4.0],
            offset: [3.0, -2.0, 1.0],
        },
        // Mirrored by one negative factor, and turned by two.
        CloudTransform {
            scale: [-1.0, 1.0, 2.0],
            offset: [5.0, 0.0, 0.0],
        },
        CloudTransform {
            scale: [-1.0, -1.0, 1.0],
            offset: [0.0, 8.0, 0.0],
        },
    ] {
        let kept = to_source(scene(), transform).unwrap();
        // The scene and a saved file show the mesh where the job made it.
        let shown = in_scene(&kept, transform);
        let expected = scene();
        assert_eq!(shown.triangles, expected.triangles, "{transform:?}");
        assert_eq!(shown.colors, expected.colors);
        for (found, wanted) in shown.vertices.iter().zip(&expected.vertices) {
            for axis in 0..3 {
                assert!(
                    (found[axis] - wanted[axis]).abs() < 1e-9,
                    "{transform:?}: {found:?}"
                );
            }
        }
        let wanted = expected.normals.unwrap()[0];
        for found in shown.normals.unwrap() {
            for axis in 0..3 {
                assert!(
                    (found[axis] - wanted[axis]).abs() < 1e-5,
                    "{transform:?}: {found:?}"
                );
            }
        }
        // In the frame of the scan the corners are in the order the normals
        // ask for, as in a mesh made from the source points.
        assert!(
            share_facing(&kept, {
                let normal = kept.normals.as_ref().unwrap()[0];
                std::array::from_fn(|axis| kept.vertices[0][axis] + f64::from(normal[axis]) * 100.0)
            }) > 0.99
        );
        // A move of the scan afterwards takes the mesh along.
        let moved = CloudTransform {
            offset: std::array::from_fn(|axis| transform.offset[axis] + 5.0),
            ..transform
        };
        assert!((in_scene(&kept, moved).vertices[0][0] - 15.0).abs() < 1e-9);
    }
    // A scale of zero has no way back.
    assert!(to_source(
        scene(),
        CloudTransform {
            scale: [1.0, 0.0, 1.0],
            offset: [0.0; 3],
        }
    )
    .is_none());
}

#[test]
fn region_estimate_warns_before_the_limits_of_a_mesh_are_hit() {
    let room = Bounds {
        min: [0.0; 3],
        max: [5.0, 4.0, 2.6],
    };
    // 86.8 m2 of faces at two triangles per voxel face of 4 cm2.
    assert_eq!(box_triangles(room, 0.02), 434_000);
    assert_eq!(box_triangles(room, 0.04), 108_500);
    let automatic = ClosedMeshConfig::default();
    let unsimplified = ClosedMeshConfig {
        simplify_tolerance: Some(0.0),
        ..automatic
    };
    let most = MAX_MESH_TRIANGLES as u64;
    assert_eq!(fit(434_000, &automatic), Fit::Fits);
    assert_eq!(fit(most, &automatic), Fit::Fits);
    // Without simplification every triangle stays, and a scanned room has
    // about 1.4 times the triangles of its box: from two thirds of the limit
    // the job may stop, and the block says so.
    assert_eq!(fit(most * 2 / 3, &unsimplified), Fit::Fits);
    assert_eq!(fit(most * 2 / 3 + 1, &unsimplified), Fit::Close);
    assert_eq!(fit(most, &unsimplified), Fit::Close);
    assert_eq!(fit(most + 1, &unsimplified), Fit::TooLarge);
    // The first generated room at voxels of 5 mm, cut back to its points:
    // 7.1 million triangles for the box, and the job stopped at the limit.
    let measured = Bounds {
        min: [0.0; 3],
        max: [5.12, 4.01, 2.62],
    };
    assert_eq!(
        fit(box_triangles(measured, 0.005), &unsimplified),
        Fit::Close
    );
    // With it, flat faces lose most of theirs: in doubt up to forty times
    // the limit, and out of the question beyond.
    assert_eq!(fit(most + 1, &automatic), Fit::Doubtful);
    assert_eq!(fit(most * 40, &automatic), Fit::Doubtful);
    assert_eq!(fit(most * 40 + 1, &automatic), Fit::TooLarge);

    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let region = studio.closed_mesh_region().unwrap();
    assert!(!region.boxed);
    assert_eq!(region.layers, 1);
    assert_eq!(region.voxel, 0.02, "automatic for a region under 20 m");
    assert!(
        (0..3).all(|axis| region.bounds.min[axis] >= 0.0 && region.bounds.max[axis] <= ROOM[axis])
    );
    assert_eq!(region.fit, Fit::Fits);
    // 9.44 m2 of faces.
    assert!((46_000..48_000).contains(&region.triangles), "{region:?}");

    // The section box limits the region, cut back to where the scan lies.
    let answer = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [0.0, 0.0, 0.0],
            max: [0.8, 1.2, 1.0],
        },
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let half = studio.closed_mesh_region().unwrap();
    assert!(half.boxed);
    assert!(half.bounds.max[0] <= 0.8 + 1e-9);
    assert!(half.triangles < region.triangles);

    // A setting that cannot be read is said where the region would be...
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Voxel("x".into())));
    assert_eq!(
        studio.closed_mesh_region().unwrap_err(),
        Problem::Setting(Sentence::plain(
            "Voxel size must be a number of metres, or empty for automatic"
        ))
    );
    // ...and a job that cannot be started says why in the status bar.
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Start));
    assert!(!studio.closed_mesh.is_running());
    assert!(studio.status.starts_with("Voxel size must be"));
}

#[test]
fn block_warns_in_the_language_in_use_when_a_region_may_not_fit() {
    let limits = [
        format_count(MAX_MESH_VERTICES),
        format_count(MAX_MESH_TRIANGLES),
    ];
    assert_eq!(limits, ["4.000.000", "8.000.000"]);
    for (language, close, doubtful, too_large) in [
        (
            Language::English,
            "Without simplification this region comes close to that",
            "fits only when simplification takes most of its triangles away",
            "This region gives more: the job will stop.",
        ),
        (
            Language::Table(0),
            "Zonder vereenvoudiging komt dit gebied daar dicht bij",
            "past alleen als vereenvoudigen de meeste driehoeken wegneemt",
            "Dit gebied geeft er meer: de taak zal stoppen.",
        ),
    ] {
        let _language = TestLanguage::hold(language);
        assert_eq!(fit_warning(Fit::Fits), None);
        for (fit, words) in [
            (Fit::Close, close),
            (Fit::Doubtful, doubtful),
            (Fit::TooLarge, too_large),
        ] {
            let warning = fit_warning(fit).unwrap();
            assert!(warning.contains(words), "{warning}");
            // Both limits are filled in, each in its own place.
            assert!(!warning.contains('{'), "{warning}");
            let vertices = warning.find(&limits[0]).expect("the vertex limit");
            let triangles = warning.find(&limits[1]).expect("the triangle limit");
            assert!(vertices < triangles, "{warning}");
        }
    }

    // A region of the window that does not fit: two points two hundred
    // metres apart are a box of that size at the automatic voxel of 5 cm.
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("wide.xyz");
    std::fs::write(&source, "0 0 0\n200 200 2\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&source, 10).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Toggle));
    let region = studio.closed_mesh_region().unwrap();
    assert_eq!(region.voxel, 0.05);
    assert_eq!(region.fit, Fit::Doubtful, "{region:?}");
    assert!(studio.closed_mesh_properties().is_some());
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Simplify("0".into())));
    assert_eq!(studio.closed_mesh_region().unwrap().fit, Fit::TooLarge);
    assert!(studio.closed_mesh_properties().is_some());
    // 60 by 60 by 1 m at voxels of 5 cm is 6.0 million triangles for the
    // box: under the limit, and close to it without simplification.
    let source = directory.path().join("hall.xyz");
    std::fs::write(&source, "0 0 0\n60 60 1\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&source, 10).unwrap());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Toggle));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Voxel("0.05".into())));
    assert_eq!(studio.closed_mesh_region().unwrap().fit, Fit::Fits);
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Simplify("0".into())));
    let region = studio.closed_mesh_region().unwrap();
    assert!(
        (5_900_000..6_000_000).contains(&region.triangles),
        "{region:?}"
    );
    assert_eq!(region.fit, Fit::Close);
    assert!(studio.closed_mesh_properties().is_some());
}

#[test]
fn api_makes_a_closed_mesh_of_a_room_and_reports_how_good_it_is() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    assert_eq!(status(&mut studio)["closed_mesh"]["job"], Value::Null);
    assert_eq!(status(&mut studio)["closed_mesh"]["last"], Value::Null);

    let id = start(&mut studio, json!({"sides": "centre"}));
    // The block takes the fields of the command and opens.
    assert_eq!(studio.closed_mesh.settings.voxel, "0.04");
    assert_eq!(studio.closed_mesh.settings.sides, Sides::Centre);
    assert!(studio.closed_mesh.open);
    let running = job(&mut studio, &id);
    assert_eq!(running["state"], "running");
    assert_eq!(running["operation"], "mesh");
    assert_eq!(running["mode"], "closed");
    assert_eq!(running["path"], Value::Null);
    // A wait for an idle window sees the job as work under way.
    let result = status(&mut studio);
    assert_eq!(crate::mcp::busy(&result), ["closed_mesh"]);
    // The scan has no index, so the job reads it into memory first.
    assert_eq!(result["closed_mesh"]["job"]["stage"], "reading");
    assert_eq!(result["mesh"], Value::Null);
    // One mesh job at a time, of whatever kind.
    for mode in ["closed", "terrain"] {
        let second = send(
            &mut studio,
            command(json!({
                "command": "mesh", "mode": mode,
                "path": directory.path().join("second.obj"),
            })),
        );
        assert_eq!(second["error"], "a mesh task is already open or running");
    }

    // The strip above the scene shows the stage with a button to cancel.
    let line = studio.closed_mesh.progress_line().unwrap();
    assert_eq!(line.phase, Phase::ClosedMesh);
    assert_eq!(line.title, "Closed mesh");
    assert!(
        line.detail.starts_with("Step 1 of 5  ·  reading"),
        "{}",
        line.detail
    );
    assert!(matches!(
        line.cancel,
        Some(Message::ClosedMesh(ClosedMeshAction::Cancel))
    ));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Poll));
    assert!(studio
        .status
        .starts_with("Closed mesh: reading a scan without an index"));
    assert!(studio.progress_strip().is_some());
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Start));
    assert_eq!(studio.status, BUSY);

    finish(&mut studio);
    let done = job(&mut studio, &id);
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["mode"], "closed");
    assert_eq!(done["shown"], true);
    assert_eq!(done["path"], Value::Null);
    assert_eq!(done["voxel"], VOXEL);
    assert_eq!(done["max_hole"], 0.25);
    // The room is one closed surface.
    assert_eq!(done["open_edges"], 0, "{done}");
    assert_eq!(done["components"], 1);
    assert_eq!(done["non_manifold_edges"], 0);
    let vertices = done["vertices"].as_u64().unwrap();
    let triangles = done["triangles"].as_u64().unwrap();
    assert!(vertices > 100 && triangles > 200, "{done}");
    assert!(triangles <= done["triangles_extracted"].as_u64().unwrap());
    // It lies within a few millimetres of the points.
    let mean = done["deviation_mean"].as_f64().unwrap();
    let p95 = done["deviation_p95"].as_f64().unwrap();
    let largest = done["deviation_max"].as_f64().unwrap();
    assert!(mean > 0.0 && mean < 0.004, "{done}");
    assert!(p95 >= mean && p95 < 0.012, "{done}");
    assert!(largest >= p95 && largest < 0.03, "{done}");
    assert!(done["deviation_samples"].as_u64().unwrap() > 10_000);
    assert_eq!(done["points"], room_points().len());
    // An XYZ file knows no station: every element took the fallback, and
    // with the centre asked for there is nothing to advise.
    assert_eq!(done["sides"], "fallback");
    assert_eq!(done["surfels_by_station"], 0);
    assert_eq!(done["surfels_without_station"], done["surfels"]);
    assert_eq!(done["surfels_undecided"], 0);
    assert_eq!(done["advice"], Value::Null);
    assert!(done["seconds"].as_f64().unwrap() > 0.0);

    // The scan holds the mesh, as the status and Properties show it.
    let result = status(&mut studio);
    assert!(crate::mcp::busy(&result).is_empty());
    assert_eq!(result["closed_mesh"]["job"], Value::Null);
    assert_eq!(result["closed_mesh"]["last"], done);
    assert_eq!(
        result["clouds"][0]["mesh"],
        json!({
            "vertices": vertices, "triangles": triangles,
            "open_edges": 0, "components": 1,
        })
    );
    assert!(studio
        .status
        .starts_with("Closed mesh shown as the mesh of room.xyz: "));
    assert!(
        studio.status.contains("0 open edges, 1 connected part")
            && studio.status.contains("deviation mean")
            && studio
                .status
                .contains("sides towards the centre of the region"),
        "{}",
        studio.status
    );
    let mesh = Arc::clone(studio.clouds[0].mesh.as_ref().unwrap());
    let bounds = mesh_bounds(&mesh);
    // It ends within a voxel of the faces of the room, on either side.
    assert!((0..3).all(
        |axis| bounds.min[axis].abs() < VOXEL && (bounds.max[axis] - ROOM[axis]).abs() < VOXEL
    ));
    // Its faces look into the room.
    let centre = ROOM.map(|side| side / 2.0);
    assert!(share_facing(&mesh, centre) > 0.99);
    assert!(mesh.normals.is_some());

    // The existing export writes it in the three formats.
    for extension in ["obj", "ply", "stl"] {
        let path = directory.path().join(format!("room-mesh.{extension}"));
        let accepted = send(&mut studio, ApiCommand::ExportMesh { path: path.clone() });
        assert_eq!(accepted["ok"], true, "{accepted}");
        // What the worker thread of the export does.
        pointcloud_core::write_mesh(&mesh, &path, MeshFormat::from_path(&path).unwrap(), &[])
            .unwrap();
        let _ = studio.update(Message::MeshExported(None, Err("done by the test".into())));
        let read = pointcloud_core::read_mesh_geometry(&path).unwrap().unwrap();
        assert_eq!(read.triangles.len() as u64, triangles, "{extension}");
    }

    // A second mesh takes the place of the first.
    let id = start(&mut studio, json!({"voxel": 0.05, "sides": "automatic"}));
    finish(&mut studio);
    let again = job(&mut studio, &id);
    assert_eq!(again["voxel"], 0.05);
    assert!(!Arc::ptr_eq(&mesh, studio.clouds[0].mesh.as_ref().unwrap()));
    // Left to itself the job says where the sides came from.
    assert!(again["advice"]
        .as_str()
        .unwrap()
        .starts_with("No station is known for these points"));
}

#[test]
fn mesh_follows_a_scan_that_is_moved_or_mirrored() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    // The scan stands at survey coordinates, mirrored along X.
    let transform = CloudTransform {
        scale: [-1.0, 1.0, 1.0],
        offset: [207_000.0, 474_000.0, 10.0],
    };
    studio.clouds[0].transform = transform;
    let destination = directory.path().join("moved.ply");
    let id = start(&mut studio, json!({"path": destination, "sides": "centre"}));
    assert_eq!(
        job(&mut studio, &id)["path"],
        json!(destination),
        "the running job names its file"
    );
    finish(&mut studio);
    let done = job(&mut studio, &id);
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["open_edges"], 0);
    assert_eq!(done["format"], "ply");
    assert_eq!(done["path"], json!(destination));
    assert_eq!(done["origin"], Value::Null);
    assert!(studio.status.contains("; written as PLY to "));
    // The region is where the scan stands in the scene.
    assert!(done["region"]["min"][0].as_f64().unwrap() > 206_998.0);
    assert!(done["region"]["max"][1].as_f64().unwrap() < 474_001.3);

    // The layer keeps the mesh in its own frame: where its points are.
    let kept = Arc::clone(studio.clouds[0].mesh.as_ref().unwrap());
    let bounds = mesh_bounds(&kept);
    assert!((0..3).all(|axis| bounds.min[axis] > -VOXEL && bounds.max[axis] < ROOM[axis] + VOXEL));
    // The file has it where the scene shows it, facing into the room.
    let written = pointcloud_core::read_mesh_geometry(&destination)
        .unwrap()
        .unwrap();
    let shown = in_scene(&kept, transform);
    assert_eq!(written.triangles.len(), shown.triangles.len());
    let scene_centre = transform.xyz(ROOM.map(|side| side / 2.0));
    for mesh in [&written, &shown] {
        let bounds = mesh_bounds(mesh);
        assert!(bounds.min[0] > 207_000.0 - ROOM[0] - VOXEL && bounds.max[0] < 207_000.0 + VOXEL);
        assert!(bounds.min[1] > 474_000.0 - VOXEL && bounds.max[2] < 10.0 + ROOM[2] + VOXEL);
        assert!(share_facing(mesh, scene_centre) > 0.99);
    }
    // In its own frame the mirrored mesh faces into the room as well.
    assert!(share_facing(&kept, ROOM.map(|side| side / 2.0)) > 0.99);

    // Moving the scan afterwards moves the mesh with its points.
    let answer = send(
        &mut studio,
        ApiCommand::Translate {
            offset: [100.0, 0.0, 0.0],
        },
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let moved = in_scene(&kept, studio.clouds[0].transform);
    assert!(mesh_bounds(&moved).min[0] > 207_100.0 - ROOM[0] - VOXEL);
    assert!(Arc::ptr_eq(&kept, studio.clouds[0].mesh.as_ref().unwrap()));
}

#[test]
fn a_cancelled_or_failed_job_leaves_the_mesh_and_the_file_as_they_were() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let earlier = MeasuredMesh {
        mesh: Arc::new(MeshGeometry {
            vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            colors: None,
            normals: None,
        }),
        topology: MeshTopology::default(),
    };
    studio.clouds[0].mesh = Some(Arc::clone(&earlier.mesh));
    let destination = directory.path().join("kept.stl");
    std::fs::write(&destination, "earlier content").unwrap();

    assert_eq!(
        send(&mut studio, ApiCommand::CancelMesh)["error"],
        "no mesh task is running"
    );
    let id = start(&mut studio, json!({"path": destination}));
    assert_eq!(
        send(&mut studio, ApiCommand::CancelMesh),
        json!({"ok": true, "cancel_requested": true})
    );
    assert_eq!(studio.status, "Cancelling the closed mesh…");
    let line = studio.closed_mesh.progress_line().unwrap();
    assert_eq!(line.title, "Cancelling…");
    assert!(line.cancel.is_none());
    assert_eq!(job(&mut studio, &id)["state"], "running");
    finish(&mut studio);
    assert_eq!(
        job(&mut studio, &id),
        json!({"state": "cancelled", "operation": "mesh", "mode": "closed"})
    );
    assert!(studio.status.starts_with("Closed mesh cancelled"));
    assert!(Arc::ptr_eq(
        &earlier.mesh,
        studio.clouds[0].mesh.as_ref().unwrap()
    ));
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "earlier content"
    );

    let answer = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [0.3, 0.3, 0.3],
            max: [0.6, 0.6, 0.6],
        },
    );
    assert_eq!(answer["ok"], true, "{answer}");
    // Inside the room, away from its faces, there are no points: the job
    // starts and the core says so.
    let id = start(&mut studio, json!({"path": destination}));
    assert!(studio
        .closed_mesh
        .job
        .as_ref()
        .unwrap()
        .stages()
        .contains(&Stage::Writing));
    finish(&mut studio);
    let failed = job(&mut studio, &id);
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"], "the region holds no points to mesh");
    assert_eq!(
        studio.status,
        "Closed mesh failed: the region holds no points to mesh"
    );
    assert!(Arc::ptr_eq(
        &earlier.mesh,
        studio.clouds[0].mesh.as_ref().unwrap()
    ));
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "earlier content"
    );
    assert!(crate::mcp::busy(&status(&mut studio)).is_empty());
}

#[test]
fn a_scan_with_a_scale_of_zero_is_refused_before_anything_is_read_or_written() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let destination = directory.path().join("flat.ply");
    std::fs::write(&destination, "earlier content").unwrap();
    let flat = CloudTransform {
        scale: [1.0, 1.0, 0.0],
        offset: [0.0; 3],
    };
    // The scan that gets the mesh has no way back to its own frame: the
    // command and the Start button say so, and no job starts.
    studio.clouds[0].transform = flat;
    for layers in ["active", "visible"] {
        let answer = send(
            &mut studio,
            command(json!({
                "command": "mesh", "mode": "closed", "layers": layers, "path": destination,
            })),
        );
        assert_eq!(
            answer,
            json!({
                "ok": false,
                "error": "the scan that gets the mesh has a scale of zero: room.xyz",
            })
        );
    }
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Start));
    assert_eq!(
        studio.status,
        "room.xyz has a scale of zero, so a mesh cannot be kept with it: give it another scale first"
    );
    assert!(!studio.closed_mesh.is_running() && studio.api_jobs.is_empty());
    assert_eq!(
        studio.closed_mesh_region().unwrap_err(),
        Problem::Refused(Refusal::FlatTarget("room.xyz".into()))
    );

    // A job that is run all the same stops before it reads a point, and the
    // file at its destination stays as it was.
    studio.clouds[0].transform = CloudTransform::default();
    let mut scene = studio.closed_mesh_scene(Layers::Active).unwrap();
    scene.target_transform = flat;
    let input = JobInput {
        scene,
        config: ClosedMeshSettings {
            voxel: VOXEL.to_string(),
            ..ClosedMeshSettings::default()
        }
        .config()
        .unwrap(),
        sides: Sides::Automatic,
        destination: Some((destination.clone(), MeshFormat::Ply)),
    };
    let control = Control::default();
    let error = run(&input, &control).unwrap_err();
    assert!(
        matches!(&error, LoadError::InvalidData(reason) if reason == FLAT_TARGET),
        "{error}"
    );
    // The first stage would have been the read of the scan into memory.
    assert_eq!(control.snapshot().total, 0);
    assert_eq!(
        std::fs::read_to_string(&destination).unwrap(),
        "earlier content"
    );
}

#[test]
fn all_visible_scans_are_the_scans_that_reach_the_section_box() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    // A second layer a hundred metres away.
    let far = directory.path().join("far.xyz");
    std::fs::copy(directory.path().join("room.xyz"), &far).unwrap();
    let far = Arc::new(pointcloud_core::open(&far, 1_000).unwrap());
    let _ = studio.update(Message::Loaded(Ok(Arc::clone(&far))));
    studio.clouds[1].transform.offset = [100.0, 0.0, 0.0];
    studio.active = Some(0);
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Layers(
        Layers::Visible,
    )));
    // Without a box both give their points.
    let both = studio.closed_mesh_region().unwrap();
    assert_eq!(both.layers, 2);
    assert!(both.bounds.max[0] > 100.0);

    // While the far layer is still being read, a job on everything waits...
    let mut loading = PointCloud::clone(&far);
    loading.provisional = true;
    studio.clouds[1].cloud = Arc::new(loading);
    assert_eq!(
        studio.closed_mesh_scene(Layers::Visible).err(),
        Some(Refusal::Loading("far.xyz".into()))
    );
    // ...and with the section box around the room that layer is not part of
    // the job: it is not read, does not refuse it and does not widen the
    // region the block states and the voxel is chosen for.
    let answer = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [0.0; 3],
            max: [2.0, ROOM[1], ROOM[2]],
        },
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let scene = studio.closed_mesh_scene(Layers::Visible).unwrap();
    assert_eq!(scene.layers.len(), 1);
    assert!(Arc::ptr_eq(&scene.layers[0].cloud, &studio.clouds[0].cloud));
    let region = studio.closed_mesh_region().unwrap();
    assert_eq!(region.layers, 1);
    assert!(region.boxed && region.bounds.max[0] <= ROOM[0] + 1e-9);
    {
        let _language = TestLanguage::hold(Language::English);
        assert_eq!(
            region_note(&region, Layers::Visible),
            "Meshes the visible scans (1) inside the section box: 1.6 × 1.2 × 1.0 m."
        );
    }
    let id = start(&mut studio, json!({}));
    finish(&mut studio);
    let done = job(&mut studio, &id);
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["points"], room_points().len());
    assert!(done["region"]["max"][0].as_f64().unwrap() < ROOM[0] + VOXEL);
    assert!(studio.clouds[0].mesh.is_some() && studio.clouds[1].mesh.is_none());

    // A box between the two holds no part of either.
    let answer = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [50.0, 0.0, 0.0],
            max: [51.0, ROOM[1], ROOM[2]],
        },
    );
    assert_eq!(answer["ok"], true, "{answer}");
    for layers in Layers::ALL {
        assert_eq!(
            studio.closed_mesh_scene(layers).err(),
            Some(Refusal::Outside)
        );
    }
    studio.section_enabled = false;

    // Buildings of 3D BAG are no scan: their vertices stay out of a mesh of
    // all visible scans...
    studio.clouds[1].cloud = Arc::clone(&far);
    studio.clouds[1].bag_source = true;
    let scene = studio.closed_mesh_scene(Layers::Visible).unwrap();
    assert_eq!(scene.layers.len(), 1);
    assert!(Arc::ptr_eq(&scene.layers[0].cloud, &studio.clouds[0].cloud));
    // ...and their layer, which is the active one right after a download,
    // does not get that mesh, which an export would write with the credit of
    // 3D BAG.
    studio.active = Some(1);
    let refusal = studio.closed_mesh_scene(Layers::Visible).err().unwrap();
    assert_eq!(refusal, Refusal::BagTarget("far.xyz".into()));
    assert_eq!(
        refusal.status(),
        "far.xyz holds 3D BAG buildings and cannot take the mesh of the scans: select the scan that gets the mesh"
    );
    assert_eq!(
        refusal.api(),
        "the active layer holds 3D BAG buildings and cannot take a mesh of the visible point clouds: far.xyz"
    );
    let refused = send(
        &mut studio,
        command(json!({"command": "mesh", "mode": "closed", "layers": "visible"})),
    );
    assert_eq!(refused["error"], refusal.api());
    assert!(studio.clouds[1].mesh.is_none());
    // On its own it is meshed as any layer, and keeps its credit.
    assert!(studio.closed_mesh_scene(Layers::Active).is_ok());
    // With only such layers shown there is no scan to mesh.
    studio.active = Some(0);
    studio.clouds[0].visible = false;
    assert_eq!(
        studio.closed_mesh_scene(Layers::Visible).err(),
        Some(Refusal::NoLayer)
    );
}

#[test]
fn a_job_is_refused_before_it_starts_when_it_cannot_run() {
    let directory = tempfile::tempdir().unwrap();
    let closed = |more: Value| {
        let mut body = json!({"command": "mesh", "mode": "closed"});
        for (name, value) in more.as_object().unwrap() {
            body[name.as_str()] = value.clone();
        }
        command(body)
    };

    let mut empty = Studio::default();
    assert_eq!(
        send(&mut empty, closed(json!({})))["error"],
        "no active cloud"
    );
    let _ = empty.update(Message::ClosedMesh(ClosedMeshAction::Toggle));
    let _ = empty.update(Message::ClosedMesh(ClosedMeshAction::Start));
    assert_eq!(
        empty.status,
        "Select a scan first: the closed mesh becomes its mesh"
    );

    let mut studio = studio_with_room(directory.path());
    for (path, problem) in [
        (json!("relative.ply"), NO_FORMAT),
        (json!(directory.path().join("mesh.txt")), NO_FORMAT),
        (
            json!(directory.path().join("missing/mesh.ply")),
            "the folder of the mesh destination does not exist",
        ),
        (json!(studio.clouds[0].cloud.path), NO_FORMAT),
    ] {
        let answer = send(&mut studio, closed(json!({"path": path})));
        assert_eq!(answer["error"], problem);
    }
    let wrong = send(&mut studio, closed(json!({"voxel": 0.001})));
    assert_eq!(
        wrong["error"],
        "the voxel size must lie between 0.005 and 0.5 m"
    );
    assert_eq!(studio.closed_mesh.settings, ClosedMeshSettings::default());

    // With every visible scan asked for, one has to be visible.
    let _ = send(
        &mut studio,
        ApiCommand::SetVisible {
            index: 0,
            visible: false,
        },
    );
    let hidden = send(&mut studio, closed(json!({"layers": "visible"})));
    assert_eq!(hidden["error"], "no visible point cloud to mesh");
    // The active scan is meshed whether it is shown or not.
    assert!(studio.closed_mesh_scene(Layers::Active).is_ok());
    let _ = send(
        &mut studio,
        ApiCommand::SetVisible {
            index: 0,
            visible: true,
        },
    );

    assert!(studio.api_jobs.is_empty(), "a refusal starts no job");
    assert!(!studio.closed_mesh.is_running());

    // The words of each refusal, for the status bar and for the API.
    let name = "hall.e57".to_owned();
    for (refusal, status, api) in [
        (
            Refusal::Loading(name.clone()),
            "hall.e57 is still loading; wait for it or hide it before meshing",
            "a point cloud is still loading: hall.e57",
        ),
        (
            Refusal::NeedsIndex(name.clone()),
            "hall.e57 has more than 5.000.000 points and no index: build the index first (INDEX > Build index)",
            "a point cloud of more than 5000000 points has no index, build it first: hall.e57",
        ),
        (
            Refusal::Outside,
            "The section box holds no part of the scans to mesh",
            "the section box holds no part of the point clouds to mesh",
        ),
        (
            Refusal::BagTarget(name.clone()),
            "hall.e57 holds 3D BAG buildings and cannot take the mesh of the scans: select the scan that gets the mesh",
            "the active layer holds 3D BAG buildings and cannot take a mesh of the visible point clouds: hall.e57",
        ),
        (
            Refusal::FlatTarget(name.clone()),
            "hall.e57 has a scale of zero, so a mesh cannot be kept with it: give it another scale first",
            "the scan that gets the mesh has a scale of zero: hall.e57",
        ),
        (
            Refusal::NoPoints(name),
            "hall.e57 has no points to mesh",
            "a point cloud has no points to mesh: hall.e57",
        ),
    ] {
        assert_eq!(refusal.status(), status);
        assert_eq!(refusal.api(), api);
        // The block says each of them in the language in use.
        assert!(crate::i18n::has_entry(refusal.sentence().text), "{status}");
    }
}

#[test]
fn a_job_whose_scan_was_closed_shows_nothing_and_still_reports() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let id = start(&mut studio, json!({"sides": "upward"}));
    let answer = send(&mut studio, ApiCommand::Remove { index: 0 });
    assert_eq!(answer["ok"], true, "{answer}");
    finish(&mut studio);
    let done = job(&mut studio, &id);
    assert_eq!(done["state"], "complete");
    assert_eq!(done["shown"], false);
    assert!(studio
        .status
        .starts_with("Closed mesh ready, but its scan was closed and nothing is shown: "));
    // Upward gives the walls of a room one fixed side: the figures say so
    // and the advice names the choice that suits a room.
    assert!(done["surfels_undecided"].as_u64().unwrap() > 0);
    let advice = done["advice"].as_str().unwrap();
    assert!(
        advice.contains("Upward gives them one fixed side"),
        "{advice}"
    );
    assert!(done["open_edges"].as_u64().unwrap() > 0);
}

#[test]
fn command_line_meshes_a_box_of_a_scan_file() {
    let directory = tempfile::tempdir().unwrap();
    let source = write_room_ptx(directory.path());
    let arguments =
        |values: &[&str]| -> Vec<OsString> { values.iter().map(OsString::from).collect() };
    let destination = directory.path().join("room.ply");
    let lines = command_line(&arguments(&[
        source.to_str().unwrap(),
        destination.to_str().unwrap(),
        "--voxel",
        "0.04",
    ]))
    .unwrap();
    // The station of the PTX scan decided the sides: nothing to advise.
    assert_eq!(lines.lines().count(), 1, "{lines}");
    assert!(lines.starts_with("Closed mesh written as PLY: "), "{lines}");
    assert!(
        lines.contains("0 open edges, 1 connected part")
            && lines.contains("voxel 40 mm")
            && lines.contains("sides from the stations")
            && lines.contains(", 0 without a station;")
            && lines.ends_with(&format!(" -> {}", destination.display())),
        "{lines}"
    );
    let mesh = pointcloud_core::read_mesh_geometry(&destination)
        .unwrap()
        .unwrap();
    assert_eq!(pointcloud_core::mesh_topology(&mesh).open_edges, 0);
    assert!(share_facing(&mesh, ROOM.map(|side| side / 2.0)) > 0.99);

    // A box takes a part of the scan, here the half below 0.5 m: open at
    // its top, with the options in another order and STL as the format.
    let half = directory.path().join("half.stl");
    let lines = command_line(&arguments(&[
        source.to_str().unwrap(),
        half.to_str().unwrap(),
        "--sides",
        "centre",
        "--simplify",
        "0",
        "--max-hole",
        "0",
        "--voxel",
        "0.05",
        "--box",
        "-1,-1,-1,3,3,0.5",
    ]))
    .unwrap();
    assert!(lines.starts_with("Closed mesh written as STL: "), "{lines}");
    assert!(
        lines.contains("sides towards the centre of the region"),
        "{lines}"
    );
    assert!(!lines.contains(" 0 open edges"), "{lines}");
    // The scan knows its station, which was not asked for: the line does
    // not count the elements as being without one.
    assert!(
        lines.contains(" surface elements; ") && !lines.contains("without a station"),
        "{lines}"
    );
    let mesh = pointcloud_core::read_mesh_geometry(&half).unwrap().unwrap();
    assert!(mesh_bounds(&mesh).max[2] < 0.5 + 2.0 * 0.05);

    let refused = |values: &[&str]| command_line(&arguments(values)).unwrap_err();
    let (input, output) = (source.to_str().unwrap(), destination.to_str().unwrap());
    // The usage line stands for itself.
    assert_eq!(refused(&[input]), (2, String::new()));
    assert_eq!(refused(&[input, output, "--voxel"]), (2, String::new()));
    assert_eq!(refused(&[input, output, "--fast", "1"]), (2, String::new()));
    assert_eq!(
        refused(&[input, "room.off"]),
        (2, "Supported mesh extensions: .obj, .ply, .stl".to_owned())
    );
    assert_eq!(
        refused(&[output, output]),
        (
            2,
            "Choose an output path different from the input".to_owned()
        )
    );
    assert_eq!(
        refused(&[input, output, "--sides", "down"]),
        (2, "--sides must be automatic, centre or upward".to_owned())
    );
    assert_eq!(
        refused(&[input, output, "--box", "0,0,0,1,1"]),
        (2, "--box must be six comma-separated numbers".to_owned())
    );
    assert_eq!(
        refused(&[input, output, "--box", "0,0,0,1,1,-1"]).1,
        "--box must be finite numbers that run from the minimum to the maximum"
    );
    assert_eq!(
        refused(&[input, output, "--voxel", "2"]),
        (
            2,
            "The voxel size must lie between 0.005 and 0.5 m".to_owned()
        )
    );
    let missing = directory.path().join("missing/room.ply");
    assert_eq!(
        refused(&[input, missing.to_str().unwrap()]).1,
        "The folder of the output path does not exist"
    );
    // A box without points fails, and leaves the earlier file as it was.
    let before = std::fs::read(&destination).unwrap();
    let (code, line) = refused(&[input, output, "--box", "5,5,5,6,6,6"]);
    assert_eq!(code, 1);
    assert_eq!(
        line,
        "Closed mesh failed: the region holds no points to mesh"
    );
    assert_eq!(std::fs::read(&destination).unwrap(), before);
    let absent = directory.path().join("absent.ptx");
    assert_eq!(refused(&[absent.to_str().unwrap(), output]).0, 1);
}

#[test]
fn a_layer_reads_its_stations_from_the_cloud_found_for_it() {
    let directory = tempfile::tempdir().unwrap();
    let source = write_room_ptx(directory.path());
    let layer = Arc::new(pointcloud_core::open(&source, 100).unwrap());
    let found = Arc::new(PointCloud::clone(&layer));
    let other = Arc::new(PointCloud::clone(&layer));
    let mut tool = ClosedMeshTool::default();
    // Nothing was found out yet: a layer is read as it is.
    assert!(Arc::ptr_eq(&tool.stationed(&layer), &layer));
    tool.stations
        .push((Arc::downgrade(&layer), Arc::clone(&found)));
    assert!(Arc::ptr_eq(&tool.stationed(&layer), &found));
    // Another layer of the same file is not that layer.
    assert!(Arc::ptr_eq(&tool.stationed(&other), &other));
    // A scan that knows its stations asks for no pass over its source.
    assert!(layer.scan_ranges_known() && layer.scan_poses.len() == 1);
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(Arc::clone(&layer))));
    let _ = start(&mut studio, json!({}));
    let stages = studio.closed_mesh.job.as_ref().unwrap().stages();
    assert_eq!(stages[0], Stage::Reading);
    assert!(!stages.contains(&Stage::Stations));
    finish(&mut studio);
    let last = status(&mut studio)["closed_mesh"]["last"].clone();
    assert_eq!(last["sides"], "stations", "{last}");
    assert_eq!(last["surfels_without_station"], 0);
    assert_eq!(last["advice"], Value::Null);
    assert!(studio.status.contains("sides from the stations"));
    assert!(
        studio.closed_mesh.stations.is_empty(),
        "nothing was learned"
    );
}

/// The room scan as a cloud from an index cache written before the station
/// of every point was recorded: its station is there, which points it
/// measured is not.
fn legacy_room(directory: &Path) -> Arc<PointCloud> {
    let mut cloud = pointcloud_core::open(write_room_ptx(directory), 100).unwrap();
    cloud.forget_scan_ranges();
    assert!(!cloud.scan_ranges_known() && cloud.scan_poses.len() == 1);
    Arc::new(cloud)
}

#[test]
fn stations_a_job_found_are_kept_when_the_job_fails_after_finding_them() {
    let directory = tempfile::tempdir().unwrap();
    let layer = legacy_room(directory.path());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(Arc::clone(&layer))));
    // A box inside the room holds no points: the job reads the source for
    // the stations, reads the scan, and fails when the core finds nothing.
    let answer = send(
        &mut studio,
        ApiCommand::SetSection {
            min: [0.3, 0.3, 0.3],
            max: [0.6, 0.6, 0.6],
        },
    );
    assert_eq!(answer["ok"], true, "{answer}");
    let id = start(&mut studio, json!({}));
    let running = studio.closed_mesh.job.as_ref().unwrap();
    assert_eq!(running.stages()[..2], [Stage::Stations, Stage::Reading]);
    assert_eq!(running.progress_value()["stage"], "stations");
    assert!(Arc::ptr_eq(&running.input.scene.layers[0].cloud, &layer));
    finish(&mut studio);
    assert_eq!(job(&mut studio, &id)["state"], "failed");
    // What the pass found stays with the layer.
    assert_eq!(studio.closed_mesh.stations.len(), 1);
    let found = studio.closed_mesh.stationed(&layer);
    assert!(!Arc::ptr_eq(&found, &layer) && found.scan_ranges_known());
    assert_eq!(found.station_of(0), Some(0));

    // The next job does not read the source for them again, and the side of
    // every face comes from the station.
    studio.section_enabled = false;
    let id = start(&mut studio, json!({}));
    let running = studio.closed_mesh.job.as_ref().unwrap();
    assert_eq!(running.stages()[0], Stage::Reading);
    assert!(Arc::ptr_eq(&running.input.scene.layers[0].cloud, &found));
    finish(&mut studio);
    let done = job(&mut studio, &id);
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["sides"], "stations");
    assert_eq!(done["surfels_without_station"], 0);
    assert_eq!(
        studio.closed_mesh.stations.len(),
        1,
        "nothing new was found"
    );

    // With stations not asked for, a scan that does not know them needs no
    // pass over its source.
    let _ = start(&mut studio, json!({"sides": "centre"}));
    let running = studio.closed_mesh.job.as_ref().unwrap();
    assert!(!running.stages().contains(&Stage::Stations));
    finish(&mut studio);
}

#[test]
fn stations_a_job_found_are_kept_when_it_is_cancelled_and_go_with_their_layer() {
    let directory = tempfile::tempdir().unwrap();
    let layer = legacy_room(directory.path());
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(Arc::clone(&layer))));
    let _ = start(&mut studio, json!({}));
    let running = studio.closed_mesh.job.as_ref().unwrap();
    let (serial, control) = (running.serial, Arc::clone(&running.control));
    // What the worker does up to the end of the pass over the source, after
    // which the user cancels.
    let mut found = PointCloud::clone(&layer);
    found.read_scan_ranges(|_| Ok(())).unwrap();
    let found = Arc::new(found);
    control.learn(&layer, &found);
    studio.cancel_closed_mesh();
    assert!(matches!(
        control.report(Stage::Reading, 0, 0),
        Err(LoadError::Cancelled)
    ));
    let end = ClosedMeshEnd::of(Err(LoadError::Cancelled));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Finished(serial, end)));
    assert!(matches!(studio.closed_mesh.last, Some(Last::Cancelled)));
    assert_eq!(studio.closed_mesh.stations.len(), 1);
    assert!(
        control.take_learned().is_empty(),
        "handed to the window once"
    );
    // The next job starts from the cloud that was found.
    let scene = studio.closed_mesh_scene(Layers::Active).unwrap();
    assert!(Arc::ptr_eq(&scene.layers[0].cloud, &found));
    drop(scene);

    // The entry goes with its layer: when that is closed, the end of the
    // next job clears it.
    let answer = send(&mut studio, ApiCommand::Remove { index: 0 });
    assert_eq!(answer["ok"], true, "{answer}");
    drop(layer);
    let other = Arc::new(pointcloud_core::open(write_room_xyz(directory.path()), 1_000).unwrap());
    let _ = studio.update(Message::Loaded(Ok(other)));
    let _ = start(&mut studio, json!({}));
    finish(&mut studio);
    assert!(studio.closed_mesh.stations.is_empty());
}

#[test]
fn advice_is_worded_for_the_count_and_for_the_sides_that_were_asked() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = Studio::default();
    let layer = Arc::new(pointcloud_core::open(write_room_ptx(directory.path()), 100).unwrap());
    let _ = studio.update(Message::Loaded(Ok(layer)));
    let _ = start(&mut studio, json!({}));
    finish(&mut studio);
    let Some(Last::Done { report, .. }) = studio.closed_mesh.last.clone() else {
        panic!("the job ends well");
    };
    assert_eq!(advice(&report, Sides::Automatic), None);

    // One and many are worded apart.
    let mut torn = report.clone();
    torn.topology.non_manifold_edges = 1;
    torn.seam_faults = 1;
    let line = advice(&torn, Sides::Automatic).unwrap();
    assert!(
        line.starts_with("1 edge has more than two triangles: ")
            && line.contains(" 1 triangle was left out where two blocks did not agree"),
        "{line}"
    );
    torn.topology.non_manifold_edges = 2;
    torn.seam_faults = 1_200;
    let line = advice(&torn, Sides::Automatic).unwrap();
    assert!(
        line.starts_with("2 edges have more than two triangles: ")
            && line.contains(" 1200 triangles were left out where two blocks did not agree"),
        "{line}"
    );
    let dutch = |sentences: &[Sentence]| -> Vec<String> {
        let _language = TestLanguage::hold(Language::Table(0));
        sentences.iter().map(Sentence::translated).collect()
    };
    let grouped = |count: u64| format_count(count);
    let lines = dutch(&advice_sentences(&torn, Sides::Automatic, &grouped));
    assert!(lines[0].starts_with("2 randen hebben meer dan twee driehoeken"));
    assert!(lines[1].starts_with("1.200 driehoeken zijn weggelaten"));
    torn.topology.non_manifold_edges = 1;
    torn.seam_faults = 1;
    let lines = dutch(&advice_sentences(&torn, Sides::Automatic, &grouped));
    assert!(lines[0].starts_with("1 rand heeft meer dan twee driehoeken"));
    assert!(lines[1].starts_with("1 driehoek is weggelaten"));

    // The command line and the local API get plain numbers, as in their
    // other figures; the block groups the digits.
    let mut undecided = report.clone();
    undecided.surfels = 498_607;
    undecided.surfels_by_station = 0;
    undecided.surfels_by_default = 262_843;
    undecided.orientation = OrientationUsed::Fallback;
    let upward = advice(&undecided, Sides::Upward).unwrap();
    assert!(
        upward.starts_with("262843 of 498607 surface elements are upright: "),
        "{upward}"
    );
    assert_eq!(report_value(&undecided, Sides::Upward)["advice"], upward);
    let block = advice_sentences(&undecided, Sides::Upward, &grouped);
    assert!(block[0]
        .english()
        .starts_with("262.843 of 498.607 surface elements are upright: "));

    // With the centre asked for, stations were not used: the advice does
    // not say the scan has none, and names the choice that uses them.
    let centre = advice(&undecided, Sides::Centre).unwrap();
    assert!(
        centre.starts_with("262843 of 498607 surface elements lie edge on to the centre")
            && centre.contains("Stations were not used: choose Automatic")
            && !centre.contains("had no station"),
        "{centre}"
    );
    let automatic = advice(&undecided, Sides::Automatic).unwrap();
    assert!(
        automatic.contains("had no station")
            && automatic.contains("scans that know their stations"),
        "{automatic}"
    );
    // The row that counts the elements without a station is for Automatic.
    let rows = |sides| result_rows(&undecided, sides).len();
    assert_eq!(rows(Sides::Automatic), rows(Sides::Centre) + 1);
    assert_eq!(rows(Sides::Centre), rows(Sides::Upward));
    // Every sentence has its entry in the table.
    for sides in Sides::ALL {
        for sentence in advice_sentences(&torn, sides, &grouped)
            .iter()
            .chain(&advice_sentences(&undecided, sides, &grouped))
        {
            assert!(crate::i18n::has_entry(sentence.text), "{}", sentence.text);
        }
    }
}

#[test]
fn stages_are_those_a_job_goes_through() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    let destination = directory.path().join("staged.obj");
    let _ = start(&mut studio, json!({"simplify_mm": 0, "path": destination}));
    let job = studio.closed_mesh.job.as_ref().unwrap();
    // No index: read first. No simplification: that stage is left out. A
    // destination: writing comes last.
    assert_eq!(
        job.stages(),
        [
            Stage::Reading,
            Stage::Planning,
            Stage::Reconstructing,
            Stage::Measuring,
            Stage::Writing
        ]
    );
    let line = |stage, done, total| {
        job.control.report(stage, done, total).unwrap();
        studio.closed_mesh.progress_line().unwrap()
    };
    let planning = line(Stage::Planning, 0, 0);
    assert_eq!(
        planning.detail,
        "Step 2 of 5  ·  finding the blocks that hold points"
    );
    assert_eq!(planning.fraction, None);
    let blocks = line(Stage::Reconstructing, 3, 12);
    assert_eq!(blocks.detail, "Step 3 of 5  ·  block 3 of 12");
    assert_eq!(blocks.fraction, Some(0.25));
    assert_eq!(
        line(Stage::Reconstructing, 12, 12).detail,
        "Step 3 of 5  ·  joining the blocks"
    );
    assert_eq!(
        line(Stage::Writing, 0, 0).detail,
        "Step 5 of 5  ·  writing the file"
    );
    assert_eq!(job.progress_value()["stage"], "writing");
    // Every stage of the core has its own name in a job of the API.
    let names: Vec<&str> = Stage::ALL.into_iter().map(Stage::name).collect();
    assert_eq!(
        names,
        [
            "stations",
            "reading",
            "planning",
            "reconstructing",
            "simplifying",
            "measuring",
            "writing"
        ]
    );
    assert_eq!(Stage::of(ClosedMeshStage::Simplifying), Stage::Simplifying);
    assert_eq!(
        Step {
            stage: Stage::Simplifying,
            done: 1,
            total: 4
        }
        .text(),
        "simplifying across the blocks, 25%"
    );
    assert_eq!(
        Step {
            stage: Stage::Stations,
            done: 2_500_000,
            total: 10_000_000
        }
        .text(),
        "finding the station of every point, 2.5M of 10.0M points"
    );
}

#[test]
fn block_shows_the_region_the_job_and_the_result_in_the_language_in_use() {
    let _language = TestLanguage::hold(Language::Table(0));
    let directory = tempfile::tempdir().unwrap();
    let mut studio = studio_with_room(directory.path());
    assert!(
        studio.closed_mesh_properties().is_none(),
        "the block is closed"
    );
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Toggle));
    assert!(studio.closed_mesh_properties().is_some());
    assert!(studio
        .status
        .starts_with("Closed mesh: put the section box"));
    let _ = studio.view();
    // What the block says of the region, and of a job under way.
    let region = studio.closed_mesh_region().unwrap();
    assert_eq!(
        region_note(&region, Layers::Active),
        "Mesht de hele actieve scan: 1.6 × 1.2 × 1.0 m. Zet de snedebox aan om een deel ervan te meshen."
    );
    assert_eq!(
        region_note(&region, Layers::Visible),
        "Mesht alle zichtbare scans (1) in hun geheel: 1.6 × 1.2 × 1.0 m. Zet de snedebox aan om een deel ervan te meshen."
    );
    let boxed = RegionInfo {
        boxed: true,
        layers: 3,
        ..region
    };
    assert_eq!(
        region_note(&boxed, Layers::Active),
        "Mesht de actieve scan binnen de snedebox: 1.6 × 1.2 × 1.0 m."
    );
    assert_eq!(
        region_note(&boxed, Layers::Visible),
        "Mesht de zichtbare scans (3) binnen de snedebox: 1.6 × 1.2 × 1.0 m."
    );
    let stage = |stage, done, total| stage_line(Step { stage, done, total });
    assert_eq!(
        stage(Stage::Stations, 0, 0),
        "Stations van de punten bepalen…"
    );
    assert_eq!(stage(Stage::Reading, 5, 9), "Scan zonder index lezen…");
    assert_eq!(stage(Stage::Planning, 0, 0), "Blokken met punten zoeken…");
    assert_eq!(stage(Stage::Reconstructing, 3, 12), "Blok 3 van 12 meshen…");
    assert_eq!(stage(Stage::Simplifying, 1, 4), "Vereenvoudigen…");
    assert_eq!(stage(Stage::Measuring, 0, 0), "Resultaat meten…");
    assert_eq!(stage(Stage::Writing, 0, 0), "Bestand schrijven…");
    // A setting that cannot be read is said in the block, in its language.
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::MaxHole("9".into())));
    assert!(studio.closed_mesh_properties().is_some());
    let Err(Problem::Setting(problem)) = studio.closed_mesh_region() else {
        panic!("the hole limit is out of range");
    };
    assert_eq!(
        problem.translated(),
        "De gatgrens moet tussen 0 en 3.2 m liggen"
    );
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::MaxHole("0.2".into())));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Voxel("0.04".into())));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Simplify("2".into())));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Sides(Sides::Upward)));
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Layers(
        Layers::Visible,
    )));
    assert_eq!(
        studio.closed_mesh.settings.value(),
        json!({
            "voxel": 0.04, "max_hole": 0.2, "simplify_mm": 2.0,
            "sides": "upward", "layers": "visible",
        })
    );
    // The Start button starts a job, whose stage the block shows.
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Start));
    assert!(studio.closed_mesh.is_running());
    assert!(studio.closed_mesh_properties().is_some());
    let _ = studio.view();
    finish(&mut studio);
    assert!(matches!(studio.closed_mesh.last, Some(Last::Done { .. })));
    assert!(studio.closed_mesh_properties().is_some());
    assert!(studio.mesh_properties().is_some());
    let _ = studio.view();
    // The button closes the block again; the result stays for the next time.
    let _ = studio.update(Message::ClosedMesh(ClosedMeshAction::Toggle));
    assert!(studio.closed_mesh_properties().is_none());

    assert_eq!(tr("Closed mesh"), "Gesloten mesh");
    assert_eq!(tr("Voxel size (m)"), "Voxelgrootte (m)");
    assert_eq!(tr("Close holes up to (m)"), "Gaten dichten tot (m)");
    assert_eq!(tr("Mean deviation"), "Gemiddelde afwijking");
    // The names of the choices reach the translation through a function.
    for text in Sides::ALL
        .map(Sides::text)
        .into_iter()
        .chain(Layers::ALL.map(Layers::text))
    {
        assert!(crate::i18n::has_entry(text), "{text}");
    }
    assert_eq!(
        Choice {
            value: Sides::Centre,
            text: Sides::Centre.text()
        }
        .to_string(),
        "Naar het midden"
    );
}
