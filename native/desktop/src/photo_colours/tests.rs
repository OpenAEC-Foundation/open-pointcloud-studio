//! Tests of Colour from photos, on a generated room seen by a panorama in
//! its middle and photos of a scanner station.

use std::path::{Path, PathBuf};

use pointcloud_core::{ExportFormat, FilePhotos, IndexedPoint, PhotoProjection, ScanImageFormat};

use super::*;
use crate::file_photos::PhotoAction;
use crate::i18n::{Language, TestLanguage};
use crate::mcp::busy;
use crate::native_api::{ApiCommand, ApiRequest};
use crate::selection::{DeletionMask, SelectionMask};

/// The inside of the room, with one corner at zero.
const ROOM: [f64; 3] = [4.0, 3.0, 2.5];
const STEP: f64 = 0.05;
/// Where the panorama was taken.
const EYE: [f64; 3] = [1.5, 1.2, 1.4];

/// The colour of the room at a position: smooth, so that a photo sampled a
/// little beside a point gives nearly the same colour.
fn room_colour(xyz: [f64; 3]) -> [u8; 3] {
    let [x, y, z] = xyz;
    [
        128.0 + 100.0 * (1.3 * x + 0.4 * z).sin(),
        128.0 + 100.0 * (1.1 * y + 0.7 * z + 1.0).sin(),
        128.0 + 100.0 * (0.9 * x + 0.8 * y).cos(),
    ]
    .map(|value| value.round() as u8)
}

/// Points on the six inner faces of the room, half a step from the edges.
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

/// The room as an XYZ file without colours.
fn write_room(directory: &Path) -> PathBuf {
    let path = directory.join("room.xyz");
    let text: String = wall_points()
        .iter()
        .map(|[x, y, z]| format!("{x:.4} {y:.4} {z:.4}\n"))
        .collect();
    std::fs::write(&path, text).unwrap();
    path
}

/// The colour a ray from inside the room meets.
fn trace(eye: [f64; 3], direction: [f64; 3]) -> [u8; 3] {
    let nearest = (0..3)
        .filter(|axis| direction[*axis] != 0.0)
        .map(|axis| {
            let wall = if direction[axis] > 0.0 {
                ROOM[axis]
            } else {
                0.0
            };
            (wall - eye[axis]) / direction[axis]
        })
        .fold(f64::INFINITY, f64::min);
    room_colour(std::array::from_fn(|axis| {
        eye[axis] + nearest * direction[axis]
    }))
}

/// A panorama of a degree per pixel, its middle along +X.
fn panorama() -> FilePhoto {
    FilePhoto {
        name: Some("Middle".into()),
        station: None,
        position: EYE,
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        width: 360,
        height: 180,
        projection: PhotoProjection::Spherical {
            pixel_size: [std::f64::consts::TAU / 360.0, std::f64::consts::PI / 180.0],
        },
        format: ScanImageFormat::Png,
        offset: 0,
        length: 1,
    }
}

/// The photo a camera takes of the room, as the file would store it.
fn png(photo: &FilePhoto) -> Vec<u8> {
    let mut image = ::image::RgbImage::new(photo.width, photo.height);
    for (column, row, pixel) in image.enumerate_pixels_mut() {
        *pixel = ::image::Rgb(trace(
            photo.position,
            photo.ray(f64::from(column), f64::from(row)),
        ));
    }
    let mut bytes = Vec::new();
    ::image::DynamicImage::ImageRgb8(image)
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            ::image::ImageOutputFormat::Png,
        )
        .unwrap();
    bytes
}

fn open_layer(studio: &mut Studio, path: &Path) {
    let cloud = Arc::new(pointcloud_core::open(path, 100_000).unwrap());
    let _ = studio.update(Message::Loaded(Ok(cloud)));
}

/// A window with the room open and the panorama listed as its photo.
fn studio_with_photo(directory: &Path) -> (Studio, PathBuf) {
    let mut studio = Studio::default();
    let path = write_room(directory);
    open_layer(&mut studio, &path);
    let photos = FilePhotos {
        photos: vec![panorama()],
        coordinate_system: None,
        skipped: 0,
    };
    let _ = studio.update(Message::Photos(PhotoAction::Listed(
        path.clone(),
        Ok(Arc::new(photos)),
    )));
    (studio, path)
}

fn send(studio: &mut Studio, command: Value) -> Value {
    let command: ApiCommand = serde_json::from_value(command).unwrap();
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.update(Message::ApiRequest(ApiRequest { command, reply }));
    receive.recv().unwrap()
}

fn status(studio: &mut Studio) -> Value {
    send(studio, json!({"command": "status"}))["result"].clone()
}

/// What the worker of the running job does, reading the photos the test
/// made instead of the file, and its message to the window.
fn finish(studio: &mut Studio) {
    let running = studio.photo_colours.job.as_ref().expect("a job runs");
    let (serial, input, control) = (running.serial, &running.input, Arc::clone(&running.control));
    let layer = &input.layer;
    let reading = JobInput {
        layer: JobLayer {
            cloud: Arc::clone(&layer.cloud),
            identity: Arc::clone(&layer.identity),
            name: layer.name.clone(),
            index: layer.index.clone(),
            transform: layer.transform,
            deleted: layer.deleted.clone(),
        },
        section: input.section,
        filter: input.filter,
        photos: input.photos.clone(),
        previous: input.previous.clone(),
        config: input.config,
        read: Arc::new(|photo: &FilePhoto| Ok(png(photo))),
    };
    let end = ColourEnd::of(run(&reading, &control));
    let _ = studio.update(Message::PhotoColours(PhotoColourAction::Finished(
        serial, end,
    )));
}

/// Colour through the local API and return the finished job.
fn colour(studio: &mut Studio, more: Value) -> Value {
    let mut body = json!({"command": "colour_from_photos"});
    for (name, value) in more.as_object().unwrap() {
        body[name.as_str()] = value.clone();
    }
    let accepted = send(studio, body);
    assert_eq!(accepted["ok"], true, "{accepted}");
    let id = accepted["job_id"].as_str().unwrap().to_owned();
    assert_eq!(
        send(studio, json!({"command": "job", "id": id}))["job"]["operation"],
        "colour_from_photos"
    );
    finish(studio);
    send(studio, json!({"command": "job", "id": id}))["job"].clone()
}

fn difference(a: [u8; 3], b: [u8; 3]) -> u8 {
    (0..3)
        .map(|channel| a[channel].abs_diff(b[channel]))
        .max()
        .unwrap()
}

/// The points the viewport draws of a layer, with their colours.
fn drawn(studio: &Studio, layer: usize) -> Vec<IndexedPoint> {
    studio.clouds[layer].view_records().collect()
}

#[test]
fn a_panorama_colours_the_room_and_undo_takes_it_back() {
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_photo(directory.path());
    studio.color_mode = ColorMode::Elevation;
    let total = studio.clouds[0].cloud.total_points;
    assert!(studio.photo_colour_properties(0).is_some());

    let done = colour(&mut studio, json!({"max_distance": 10}));
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["region"], "layer");
    assert_eq!(done["points"], total);
    assert_eq!(done["source"], "room.xyz");
    assert_eq!(done["kept"], true);
    assert_eq!(
        (done["photos"].as_u64(), done["photos_used"].as_u64()),
        (Some(1), Some(1))
    );
    assert!(done["compared"].is_null(), "the room had no colours");
    let coloured = done["coloured"].as_u64().unwrap();
    assert!(coloured as f64 > 0.99 * total as f64, "{done}");
    assert_eq!(done["unseen"].as_u64().unwrap(), total - coloured);
    assert_eq!(done["max_distance"], 10.0);
    assert_eq!(done["blend"], true);

    // The colours are drawn, in RGB.
    assert_eq!(studio.color_mode, ColorMode::Rgb);
    let records = drawn(&studio, 0);
    let mut errors: Vec<u8> = records
        .iter()
        .filter_map(|record| {
            record
                .point
                .rgb
                .map(|rgb| difference(rgb, room_colour(record.point.xyz)))
        })
        .collect();
    assert!(errors.len() as f64 > 0.99 * records.len() as f64);
    errors.sort_unstable();
    assert!(
        errors[errors.len() / 2] <= 3,
        "{}",
        errors[errors.len() / 2]
    );
    assert!(errors[errors.len() * 99 / 100] <= 12);

    let seen = status(&mut studio);
    assert_eq!(seen["clouds"][0]["photo_colours"], coloured);
    assert_eq!(seen["colour_from_photos"]["last"]["state"], "complete");
    assert!(seen["colour_from_photos"]["job"].is_null());
    assert_eq!(
        seen["colour_from_photos"]["settings"],
        json!({"max_distance": 10.0, "blend": true})
    );

    // Undo takes the colours back, Redo gives them again.
    let undone = send(&mut studio, json!({"command": "undo_delete"}));
    assert_eq!(undone["ok"], true);
    assert_eq!(undone["status"], "Undid the photo colours of room.xyz");
    assert!(studio.clouds[0].colours.is_none());
    assert!(drawn(&studio, 0)
        .iter()
        .all(|record| record.point.rgb.is_none()));
    let redone = send(&mut studio, json!({"command": "redo_delete"}));
    assert_eq!(redone["status"], "Redid the photo colours of room.xyz");
    assert_eq!(studio.clouds[0].colours.as_ref().unwrap().len(), coloured);

    // Removing them is an edit as well.
    let cleared = send(&mut studio, json!({"command": "clear_photo_colours"}));
    assert_eq!(
        cleared,
        json!({"ok": true, "layer": 0, "source": "room.xyz"})
    );
    assert!(studio.clouds[0].colours.is_none());
    assert_eq!(
        send(&mut studio, json!({"command": "clear_photo_colours"}))["error"],
        "the layer has no photo colours"
    );
    let _ = send(&mut studio, json!({"command": "undo_delete"}));
    assert_eq!(studio.clouds[0].colours.as_ref().unwrap().len(), coloured);
}

#[test]
fn the_section_box_and_deleted_points_limit_what_is_coloured() {
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_photo(directory.path());
    let total = studio.clouds[0].cloud.total_points;
    // Delete one point of the box, on the far wall.
    let points = pointcloud_core::open(&studio.clouds[0].cloud.path, 100_000).unwrap();
    let ordinal = points
        .points
        .iter()
        .position(|point| point.xyz[0] == ROOM[0])
        .unwrap() as u64;
    let mut bits = vec![0u64; total.div_ceil(64) as usize];
    bits[(ordinal / 64) as usize] |= 1 << (ordinal % 64);
    let mut deleted = DeletionMask::new(total).unwrap();
    deleted
        .apply(&SelectionMask {
            bits,
            count: 1,
            highlights: Vec::new(),
            highlights_source: true,
            source_bounds: None,
        })
        .unwrap();
    studio.clouds[0].deleted = Some(Arc::new(deleted));
    let boxed = send(
        &mut studio,
        json!({"command": "set_section", "min": [2.0, 0.0, 0.0], "max": ROOM}),
    );
    assert_eq!(boxed["ok"], true, "{boxed}");

    let done = colour(&mut studio, json!({}));
    assert_eq!(done["region"], "section_box");
    let inside = points
        .points
        .iter()
        .filter(|point| point.xyz[0] >= 2.0)
        .count() as u64;
    assert_eq!(done["points"], inside - 1);
    let colours = Arc::clone(studio.clouds[0].colours.as_ref().unwrap());
    assert_eq!(colours.get(ordinal), None);
    for (place, point) in points.points.iter().enumerate() {
        if point.xyz[0] < 2.0 {
            assert_eq!(colours.get(place as u64), None);
        }
    }

    // Without the box the rest is coloured too, over what the first job
    // gave; Undo goes back to that.
    let _ = send(&mut studio, json!({"command": "clear_section"}));
    let done = colour(&mut studio, json!({}));
    assert_eq!(done["region"], "layer");
    assert_eq!(done["points"], total - 1);
    let all = studio.clouds[0].colours.as_ref().unwrap().len();
    assert!(all > colours.len());
    let _ = send(&mut studio, json!({"command": "undo_delete"}));
    assert_eq!(
        studio.clouds[0].colours.as_ref().unwrap().len(),
        colours.len()
    );
}

#[test]
fn an_export_writes_the_photo_colours() {
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_photo(directory.path());
    let _ = colour(&mut studio, json!({}));
    let entry = &studio.clouds[0];
    let colours = entry.colours.as_deref();
    let moved = CloudTransform {
        scale: [1.0; 3],
        offset: [10.0, 0.0, 0.0],
    };
    let whole = directory.path().join("coloured.ply");
    crate::export_edited_where(
        &entry.cloud,
        &whole,
        ExportFormat::PlyBinary,
        moved,
        colours,
        entry.cloud.total_points,
        |_, _| true,
    )
    .unwrap();
    // The room had no colours; the file has them, where the layer stands.
    let written = pointcloud_core::open(&whole, 100_000).unwrap();
    assert!(written.has_rgb);
    assert_eq!(written.total_points, entry.cloud.total_points);
    let close = written
        .points
        .iter()
        .filter(|point| {
            let source = [point.xyz[0] - 10.0, point.xyz[1], point.xyz[2]];
            point
                .rgb
                .is_some_and(|rgb| difference(rgb, room_colour(source)) <= 12)
        })
        .count();
    assert!(close as f64 > 0.98 * written.points.len() as f64, "{close}");

    let section = OrientedBox::from(Bounds {
        min: [13.0, -1.0, -1.0],
        max: [15.0, 4.0, 3.0],
    });
    let boxed = directory.path().join("section.ply");
    let count = crate::export_edited_section(
        &entry.cloud,
        &boxed,
        ExportFormat::PlyBinary,
        moved,
        colours,
        section,
        None,
    )
    .unwrap();
    let written = pointcloud_core::open(&boxed, 100_000).unwrap();
    assert_eq!(written.total_points, count);
    assert!(written.has_rgb && written.points.iter().all(|point| point.xyz[0] >= 13.0));
}

#[test]
fn station_photos_colour_points_as_well() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_room(directory.path());
    let mut cloud = pointcloud_core::open(&path, 100_000).unwrap();
    // Six cube faces of 90 degrees around the panorama's place.
    let faces: [[[f64; 3]; 3]; 6] = [
        [[0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]],
        [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]],
        [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, -1.0, 0.0]],
        [[-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]],
        [[0.0, -1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, -1.0]],
        [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
    ];
    cloud.scan_images = faces
        .iter()
        .map(|axes| ScanImage {
            station: None,
            position: EYE,
            axes: *axes,
            width: 128,
            height: 128,
            focal: [64.0, 64.0],
            principal: [63.5, 63.5],
            format: ScanImageFormat::Png,
            offset: 0,
            length: 1,
        })
        .collect();
    let mut studio = Studio::default();
    let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
    // No photos of the file, only those of the station.
    let _ = studio.update(Message::Photos(PhotoAction::Listed(
        path,
        Ok(Arc::new(FilePhotos::default())),
    )));
    assert_eq!(studio.colour_photos(0).len(), 6);
    let done = colour(&mut studio, json!({"blend": false}));
    assert_eq!(done["state"], "complete", "{done}");
    assert_eq!(done["blend"], false);
    assert_eq!(done["photos_used"], 6);
    let total = done["points"].as_u64().unwrap();
    assert!(done["coloured"].as_u64().unwrap() as f64 > 0.99 * total as f64);
    let records = drawn(&studio, 0);
    let close = records
        .iter()
        .filter(|record| {
            record
                .point
                .rgb
                .is_some_and(|rgb| difference(rgb, room_colour(record.point.xyz)) <= 12)
        })
        .count();
    assert!(close as f64 > 0.98 * records.len() as f64, "{close}");
}

#[test]
fn a_colouring_is_refused_with_a_reason_and_can_be_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let mut studio = Studio::default();
    open_layer(&mut studio, &write_room(directory.path()));
    // A layer without photos has no block and is refused.
    assert!(studio.photo_colour_properties(0).is_none());
    assert_eq!(
        send(&mut studio, json!({"command": "colour_from_photos"})),
        json!({"ok": false, "error": "no layer with photos"})
    );
    assert_eq!(
        send(
            &mut studio,
            json!({"command": "colour_from_photos", "layer": 0})
        )["error"],
        "the layer has no photos: room.xyz"
    );
    let (mut studio, _) = studio_with_photo(directory.path());
    assert_eq!(
        send(
            &mut studio,
            json!({"command": "colour_from_photos", "max_distance": 1000})
        )["error"],
        "the largest distance must be a number of metres from 0.5 to 500"
    );
    let accepted = send(&mut studio, json!({"command": "colour_from_photos"}));
    assert_eq!(accepted["ok"], true);
    assert_eq!(busy(&status(&mut studio)), ["colour_from_photos"]);
    assert_eq!(
        send(&mut studio, json!({"command": "colour_from_photos"}))["error"],
        "points are already being coloured from photos"
    );
    let running = &status(&mut studio)["colour_from_photos"]["job"];
    assert_eq!(running["state"], "running");
    assert_eq!(running["source"], "room.xyz");
    assert_eq!(
        send(&mut studio, json!({"command": "cancel_colour_from_photos"})),
        json!({"ok": true, "cancel_requested": true})
    );
    assert!(studio
        .progress_lines()
        .iter()
        .any(|line| line.cancel.is_none()));
    finish(&mut studio);
    let id = accepted["job_id"].as_str().unwrap();
    assert_eq!(
        send(&mut studio, json!({"command": "job", "id": id}))["job"],
        json!({"state": "cancelled", "operation": "colour_from_photos"})
    );
    assert!(studio.clouds[0].colours.is_none());
    assert_eq!(
        studio.status,
        "Colouring from photos cancelled; the colours stay as they were"
    );
    assert_eq!(
        send(&mut studio, json!({"command": "cancel_colour_from_photos"}))["error"],
        "no colouring from photos is running"
    );
}

#[test]
fn closing_the_window_stops_a_colouring() {
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_photo(directory.path());
    let accepted = send(&mut studio, json!({"command": "colour_from_photos"}));
    assert_eq!(accepted["ok"], true, "{accepted}");
    let control = Arc::clone(&studio.photo_colours.job.as_ref().unwrap().control);
    assert!(!control.cancelled.load(Ordering::Relaxed));
    // Exit in the File view and closing the window both arrive as this.
    let _ = studio.update(Message::Exit);
    assert!(
        control.cancelled.load(Ordering::Relaxed),
        "the job stops at its next batch instead of keeping the process alive"
    );
}

#[test]
fn undo_keeps_the_photo_colours_within_its_budget() {
    let directory = tempfile::tempdir().unwrap();
    let (mut studio, _) = studio_with_photo(directory.path());
    let total = studio.clouds[0].cloud.total_points;
    // Tables that colour every point, so that they share no block.
    let table = |shade: u8| {
        let mut colours = PointColours::new(total);
        for ordinal in 0..total {
            colours.set(ordinal, [shade; 3]);
        }
        Arc::new(colours)
    };
    let size = table(0).bytes();
    studio.photo_colours.undo_budget = 2 * size + size / 2;
    for shade in 1..=3 {
        assert_eq!(studio.set_photo_colours(0, Some(table(shade))), 0);
    }
    // Undo holds no colours, the first table and the second.
    assert_eq!(studio.undo_deletions.len(), 3);
    // A third table in Undo is beyond the budget: the oldest edits go.
    assert_eq!(studio.set_photo_colours(0, Some(table(4))), 2);
    assert_eq!(studio.undo_deletions.len(), 2);
    // A table that shares all but one block with the one before adds only
    // that block.
    let mut changed = studio.clouds[0].colours.as_deref().unwrap().clone();
    changed.set(0, [9, 9, 9]);
    assert_eq!(studio.set_photo_colours(0, Some(Arc::new(changed))), 0);
    assert_eq!(studio.undo_deletions.len(), 3);
    // The newest edit always stays, however large.
    studio.photo_colours.undo_budget = 0;
    assert_eq!(studio.clear_photo_colours(0).unwrap().1, 3);
    assert_eq!(studio.undo_deletions.len(), 1);
    let undone = send(&mut studio, json!({"command": "undo_delete"}));
    assert_eq!(undone["ok"], true);
    assert_eq!(
        studio.clouds[0].colours.as_ref().unwrap().get(0),
        Some([9, 9, 9])
    );

    // The status bar says so when a colouring lets go of edits.
    let (mut studio, _) = studio_with_photo(directory.path());
    studio.photo_colours.undo_budget = 0;
    let _ = colour(&mut studio, json!({}));
    assert!(
        studio.status.ends_with("Undo takes the colours back"),
        "{}",
        studio.status
    );
    let _ = colour(&mut studio, json!({}));
    assert!(
        studio.status.ends_with(
            "Undo takes the colours back. To save memory, Undo let go of the oldest edit"
        ),
        "{}",
        studio.status
    );
    assert_eq!(studio.undo_deletions.len(), 1);
}

#[test]
fn the_block_speaks_the_language_of_the_window() {
    let _language = TestLanguage::hold(Language::Table(0));
    let refusal = Refusal::NoPhotos("room.xyz".into());
    assert_eq!(
        refusal.sentence().translated(),
        "room.xyz heeft geen foto's om de punten mee te kleuren"
    );
    assert_eq!(refusal.api(), "the layer has no photos: room.xyz");
    assert_eq!(
        Refusal::Busy.sentence().translated(),
        "Er worden al punten uit foto's gekleurd"
    );
    assert!(crate::i18n::has_entry(BUSY));
}
