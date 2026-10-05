//! Detection on synthetic scans: rooms, planes and noise from
//! `test_shapes`. Every threshold is the value measured on the fixed seeds
//! with a margin; the measured value is stated beside it.

use super::*;
use crate::grid2d::ring_signed_area;
use crate::local_fit::{cross, difference};
use crate::region_source::EVERYWHERE;
use crate::test_shapes::*;
use crate::{IndexedPoint, MeshGeometry, ScanPose, ScanRange};

const EVERY: &RegionFilter<'static> = &|_, _, _| true;

fn records(shape: &Shape) -> Vec<IndexedPoint> {
    shape
        .cloud_points()
        .into_iter()
        .enumerate()
        .map(|(ordinal, point)| IndexedPoint {
            point,
            ordinal: ordinal as u64,
        })
        .collect()
}

fn resident(records: &[IndexedPoint]) -> SurfaceSource<'_> {
    SurfaceSource {
        points: RegionSource::resident(records, SourceTransform::default()),
        cloud: None,
    }
}

/// Detect in a shape held in memory, without stations.
fn detect(shape: &Shape, config: &SurfaceDetectConfig) -> DetectedSurfaces {
    let records = records(shape);
    detect_surfaces(
        &[resident(&records)],
        0,
        EVERYWHERE,
        config,
        EVERY,
        &mut |_| Ok(()),
    )
    .unwrap()
}

/// A shape as a file with an index, which knows the station of every point
/// the way a scan file read scan by scan does.
fn scanned(shape: &Shape, leaf_points: u64) -> IndexedCloud {
    let mut indexed = indexed_cloud(&shape.cloud_points(), leaf_points);
    indexed.cloud.scan_poses = shape
        .stations
        .iter()
        .enumerate()
        .map(|(index, position)| ScanPose {
            label: format!("station {index}"),
            position: *position,
            axes: None,
        })
        .collect();
    indexed.cloud.scan_ranges = shape
        .station_ranges()
        .into_iter()
        .map(|(first_ordinal, station)| ScanRange {
            first_ordinal,
            station: (station != u32::MAX).then_some(station),
        })
        .collect();
    indexed.cloud.scan_ranges_known = true;
    indexed
}

fn indexed(scan: &IndexedCloud) -> SurfaceSource<'_> {
    SurfaceSource {
        points: RegionSource::new(&scan.cloud, Some(&scan.index), SourceTransform::default()),
        cloud: Some(&scan.cloud),
    }
}

fn angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let sine = cross(a, b);
    dot(sine, sine).sqrt().atan2(dot(a, b)).to_degrees()
}

fn apart(a: [f64; 3], b: [f64; 3]) -> f64 {
    let between = difference(a, b);
    dot(between, between).sqrt()
}

/// The one face with this normal, within a degree, through this point,
/// within a centimetre.
fn face_through(found: &DetectedSurfaces, normal: [f64; 3], on: [f64; 3]) -> &PlaneFace {
    let matching: Vec<&PlaneFace> = found
        .planes
        .iter()
        .filter(|face| {
            angle_deg(face.normal, normal) < 1.0 && face.signed_distance(on).abs() < 0.01
        })
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "faces with normal {normal:?} through {on:?}"
    );
    matching[0]
}

fn corners_3d(face: &PlaneFace) -> Vec<[f64; 3]> {
    face.patches
        .iter()
        .flat_map(|patch| patch.outer.iter().chain(patch.holes.iter().flatten()))
        .map(|corner| face.point(*corner))
        .collect()
}

/// The six faces of the default room as seen from inside: inward normal, a
/// point of the plane, area and the number of points the generator puts on
/// the face at 2 cm.
const ROOM_FACES: [([f64; 3], [f64; 3], f64, u64); 6] = [
    ([0.0, 0.0, 1.0], [0.0, 0.0, 0.0], 12.0, 30_000),
    ([0.0, 0.0, -1.0], [0.0, 0.0, 2.6], 12.0, 30_000),
    ([0.0, 1.0, 0.0], [0.0, 0.0, 0.0], 10.4, 26_000),
    ([0.0, -1.0, 0.0], [0.0, 3.0, 0.0], 10.4, 26_000),
    ([1.0, 0.0, 0.0], [0.0, 0.0, 0.0], 7.8, 19_500),
    ([-1.0, 0.0, 0.0], [4.0, 0.0, 0.0], 7.8, 19_500),
];

fn room_corners() -> Vec<[f64; 3]> {
    let mut corners = Vec::new();
    for x in [0.0, 4.0] {
        for y in [0.0, 3.0] {
            for z in [0.0, 2.6] {
                corners.push([x, y, z]);
            }
        }
    }
    corners
}

fn noisy_room() -> Shape {
    box_room(&RoomSpec::default()).with_noise(Noise::Gaussian(0.003), 1)
}

fn classes(found: &DetectedSurfaces) -> [usize; 4] {
    let count = |class| {
        found
            .planes
            .iter()
            .filter(|face| face.class == class)
            .count()
    };
    [
        count(FaceClass::Floor),
        count(FaceClass::Ceiling),
        count(FaceClass::Wall),
        count(FaceClass::Sloped),
    ]
}

/// What the room test asserts of every face, with the tolerances as
/// arguments so that the coarse runs can use it too.
fn assert_room_faces(found: &DetectedSurfaces, area_share: f64, plane_distance: f64) {
    assert_eq!(found.planes.len(), 6);
    assert!(found.cylinders.is_empty());
    assert_eq!(classes(found), [1, 1, 4, 0]);
    for (normal, on, area, _) in ROOM_FACES {
        let face = face_through(found, normal, on);
        assert!(angle_deg(face.normal, normal) < 0.1, "{:?}", face.normal);
        assert!(face.signed_distance(on).abs() < plane_distance);
        assert!(
            (face.area - area).abs() < area_share * area,
            "area {} of {area}",
            face.area
        );
    }
}

#[test]
fn a_room_gives_its_floor_ceiling_and_four_walls() {
    let room = noisy_room();
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(found.voxel_size, 0.03);
    assert_eq!(found.boundary_cell, 0.05);
    assert_eq!((found.read_points, found.source_points), (151_000, 151_000));
    assert_eq!(found.assigned_points, 151_000);
    assert!(!found.is_coarse());
    assert_eq!(found.region.unwrap().min.map(f64::round), [0.0; 3]);
    // Normals within 0.1 degree (measured 0.006), planes within 1 mm
    // (0.1 mm), areas within 0.2 % (0.02 %).
    assert_room_faces(&found, 0.002, 0.001);
    let corners = room_corners();
    for (index, (normal, on, _, points)) in ROOM_FACES.into_iter().enumerate() {
        let face = face_through(&found, normal, on);
        // Largest first: floor and ceiling, then the long walls.
        assert!(face.id as usize <= 2 * (index / 2) + 2 && face.id as usize > 2 * (index / 2));
        let class = [FaceClass::Floor, FaceClass::Ceiling, FaceClass::Wall][index.min(2)];
        assert_eq!(face.class, class);
        // No station: every face has the room in front of it.
        assert_eq!(face.normal_source, NormalSource::OpenSide);
        // Every point of the face, give or take one in the corner.
        let residuals = face.residuals;
        assert!(
            residuals.points.abs_diff(points) <= 2,
            "{}",
            residuals.points
        );
        assert_eq!(residuals.inliers, residuals.points);
        // Noise of 3 mm: RMS 2.99 to 3.03 mm measured, 95 % within 1.96
        // times that (5.87 to 5.95 mm), the largest of 20,000 to 30,000
        // near four times (11.1 to 15.6 mm), and no offset (at most 0.1 mm).
        assert!(
            (0.0028..0.0032).contains(&residuals.rms),
            "{}",
            residuals.rms
        );
        assert!(
            (0.0056..0.0062).contains(&residuals.p95),
            "{}",
            residuals.p95
        );
        assert!((0.010..0.018).contains(&residuals.max), "{}", residuals.max);
        assert!(residuals.mean.abs() < 0.0003, "{}", residuals.mean);
        assert!((0.0022..0.0026).contains(&residuals.mean_abs));
        assert!(face.coverage() > 0.99);
        // A rectangle whose corners are corners of the room, within 1 mm
        // (0.25 mm measured).
        assert_eq!(face.patches.len(), 1);
        assert!(face.patches[0].holes.is_empty());
        assert_eq!(face.patches[0].outer.len(), 4);
        assert!(ring_signed_area(&face.patches[0].outer) > 0.0);
        for corner in corners_3d(face) {
            let nearest = corners
                .iter()
                .map(|known| apart(*known, corner))
                .fold(f64::INFINITY, f64::min);
            assert!(nearest < 0.001, "{corner:?} is {nearest} from a corner");
        }
        // The axes of the outline: level and to the right, and up.
        assert!(face.u[2].abs() < 1e-3);
        let n = cross(face.u, face.v);
        assert!((0..3).all(|axis| (n[axis] - face.normal[axis]).abs() < 1e-12));
        let grid = &face.deviation;
        assert_eq!(
            grid.counts.len(),
            grid.width as usize * grid.height as usize
        );
        assert_eq!(
            grid.counts
                .iter()
                .map(|count| u64::from(*count))
                .sum::<u64>(),
            residuals.points
        );
    }
    // Six different planes.
    let mut groups: Vec<u32> = found
        .planes
        .iter()
        .map(|face| face.coplanar_group)
        .collect();
    groups.sort_unstable();
    assert_eq!(groups, [1, 2, 3, 4, 5, 6]);

    // Twelve edges, each between two corners of the room that differ along
    // one axis, with its ends within 1 mm of them (0.25 mm measured) and
    // the faces square to each other within 0.05 degree (0.006).
    assert_eq!(found.edges.len(), 12);
    let mut seen = Vec::new();
    for edge in &found.edges {
        assert!((edge.angle_deg - 90.0).abs() < 0.05, "{}", edge.angle_deg);
        assert!(edge.faces[0] < edge.faces[1]);
        let ends: Vec<usize> = [edge.start, edge.end]
            .iter()
            .map(|end| {
                corners
                    .iter()
                    .position(|corner| apart(*corner, *end) < 0.001)
                    .unwrap_or_else(|| panic!("{end:?} is no corner of the room"))
            })
            .collect();
        let differing = (0..3)
            .filter(|axis| corners[ends[0]][*axis] != corners[ends[1]][*axis])
            .count();
        assert_eq!(differing, 1);
        // Both faces pass through the edge.
        for id in edge.faces {
            let face = &found.planes[id as usize - 1];
            assert_eq!(face.id, id);
            assert!(face.signed_distance(edge.start).abs() < 1e-9);
            assert!(face.signed_distance(edge.end).abs() < 1e-9);
        }
        seen.push((ends[0].min(ends[1]), ends[0].max(ends[1])));
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 12);
    // A wall and the floor end in the same corner to the last digit but a
    // few: both come from the same three planes.
    let floor = face_through(&found, [0.0, 0.0, 1.0], [0.0; 3]);
    let wall = face_through(&found, [0.0, 1.0, 0.0], [0.0; 3]);
    for corner in corners_3d(wall)
        .into_iter()
        .filter(|corner| corner[2] < 1.0)
    {
        let nearest = corners_3d(floor)
            .into_iter()
            .map(|other| apart(other, corner))
            .fold(f64::INFINITY, f64::min);
        assert!(nearest < 1e-9, "{nearest}");
    }
}

#[test]
fn a_door_is_a_notch_and_a_window_a_hole_in_the_outline_of_a_wall() {
    let room = box_room(&RoomSpec {
        openings: vec![
            Opening::door(Wall::South, 1.0),
            Opening::window(Wall::South, 2.4),
        ],
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 2);
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 6);
    assert_eq!(classes(&found), [1, 1, 4, 0]);
    let wall = face_through(&found, [0.0, 1.0, 0.0], [0.0; 3]);
    assert_eq!(wall.patches.len(), 1);
    let patch = &wall.patches[0];
    // An opening ends at the outermost scan points round it. Those lie half
    // a point spacing (1 cm) outside the true opening, so an opening is
    // found that much larger on every side, within 3 mm.
    let near = |value: f64, expected: f64| (value - expected).abs() < 0.003;
    assert_eq!(patch.outer.len(), 8, "{:?}", patch.outer);
    let outline: Vec<[f64; 3]> = patch
        .outer
        .iter()
        .map(|corner| wall.point(*corner))
        .collect();
    // The door: 0.9 m wide from x = 1.0, 2.1 m high, down to the floor.
    let jambs: Vec<&[f64; 3]> = outline
        .iter()
        .filter(|corner| corner[0] > 0.5 && corner[0] < 3.5)
        .collect();
    assert_eq!(jambs.len(), 4);
    for corner in &jambs {
        assert!(near(corner[0], 0.99) || near(corner[0], 1.91), "{corner:?}");
        assert!(
            corner[2].abs() < 0.001 || near(corner[2], 2.11),
            "{corner:?}"
        );
        assert!(corner[1].abs() < 0.001);
    }
    // The window: 1.2 by 1.2 m from x = 2.4, 0.9 m above the floor.
    assert_eq!(patch.holes.len(), 1);
    let hole: Vec<[f64; 3]> = patch.holes[0]
        .iter()
        .map(|corner| wall.point(*corner))
        .collect();
    assert_eq!(hole.len(), 4);
    assert!(ring_signed_area(&patch.holes[0]) < 0.0);
    for corner in &hole {
        assert!(near(corner[0], 2.39) || near(corner[0], 3.61), "{corner:?}");
        assert!(near(corner[2], 0.89) || near(corner[2], 2.11), "{corner:?}");
    }
    let hole_area = -ring_signed_area(&patch.holes[0]);
    assert!((hole_area - 1.22 * 1.22).abs() < 0.01, "{hole_area}");
    // The wall less its openings: 7.07 m2; found 6.972, the openings being
    // a centimetre larger all round.
    let expected = 10.4 - 0.92 * 2.11 - 1.22 * 1.22;
    assert!((wall.area - expected).abs() < 0.01, "{}", wall.area);
    assert!((wall.area - 7.07).abs() < 0.02 * 7.07);
    // The other faces are whole.
    for face in found.planes.iter().filter(|face| face.id != wall.id) {
        assert_eq!(face.patches[0].outer.len(), 4);
        assert!(face.patches[0].holes.is_empty());
    }
    // The edge of this wall with the floor stops at the door and goes on
    // beyond it, at the corners of the outline of the wall.
    let floor = face_through(&found, [0.0, 0.0, 1.0], [0.0; 3]);
    let mut pieces: Vec<[f64; 2]> = found
        .edges
        .iter()
        .filter(|edge| edge.faces == [floor.id.min(wall.id), floor.id.max(wall.id)])
        .map(|edge| {
            assert!(edge.start[1].abs() < 0.001 && edge.start[2].abs() < 0.001);
            [
                edge.start[0].min(edge.end[0]),
                edge.start[0].max(edge.end[0]),
            ]
        })
        .collect();
    pieces.sort_by(|a, b| a[0].total_cmp(&b[0]));
    assert_eq!(pieces.len(), 2);
    assert!(pieces[0][0].abs() < 0.001 && near(pieces[0][1], 0.99));
    assert!(near(pieces[1][0], 1.91) && (pieces[1][1] - 4.0).abs() < 0.001);
    assert_eq!(found.edges.len(), 13);
}

#[test]
fn stations_turn_every_face_to_the_side_it_was_scanned_from() {
    // Walls of 10 cm, scanned from inside and, each from a station of its
    // own, from outside.
    let room = box_room(&RoomSpec {
        wall_thickness: Some(0.1),
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 3);
    assert_eq!(room.stations.len(), 5);
    let scan = scanned(&room, 4_096);
    assert_eq!(scan.cloud.station_of(0), Some(0));
    assert_eq!(scan.cloud.station_of(scan.cloud.total_points - 1), Some(4));
    let config = SurfaceDetectConfig::default();
    let run = |source: SurfaceSource<'_>| {
        detect_surfaces(&[source], 0, EVERYWHERE, &config, EVERY, &mut |_| Ok(())).unwrap()
    };
    let found = run(indexed(&scan));
    assert_eq!(found.planes.len(), 10);
    assert_eq!(classes(&found), [1, 1, 8, 0]);
    assert!(found
        .planes
        .iter()
        .all(|face| face.normal_source == NormalSource::Stations));
    // The inside faces look into the room.
    for (normal, on, area, _) in ROOM_FACES {
        let face = face_through(&found, normal, on);
        assert!((face.area - area).abs() < 0.002 * area);
    }
    // The outside faces look away from it, although the middle of the
    // points lies behind them.
    for (normal, on, area) in [
        ([0.0, -1.0, 0.0], [0.0, -0.1, 0.0], 4.2 * 2.6),
        ([0.0, 1.0, 0.0], [0.0, 3.1, 0.0], 4.2 * 2.6),
        ([-1.0, 0.0, 0.0], [-0.1, 0.0, 0.0], 3.2 * 2.6),
        ([1.0, 0.0, 0.0], [4.1, 0.0, 0.0], 3.2 * 2.6),
    ] {
        let face = face_through(&found, normal, on);
        // Free edges all round: the outline ends at the outermost points,
        // 1 cm inside the true edge. 1.2 % measured.
        assert!((face.area - area).abs() < 0.02 * area, "{}", face.area);
        assert!((0.0028..0.0032).contains(&face.residuals.rms));
    }

    // Without the cloud that knows the stations, the side that lies open
    // decides. An outside face has nothing in front of it and the inside
    // face of its wall behind it, so it is taken to look inward.
    let guessed = run(SurfaceSource {
        cloud: None,
        ..indexed(&scan)
    });
    assert_eq!(guessed.planes.len(), 10);
    assert!(guessed
        .planes
        .iter()
        .all(|face| face.normal_source == NormalSource::OpenSide));
    face_through(&guessed, [0.0, 1.0, 0.0], [0.0, -0.1, 0.0]);
    // The inside faces look into the room, as with stations.
    for (normal, on, _, _) in ROOM_FACES {
        face_through(&guessed, normal, on);
    }

    // Stations are known, but not which one measured which point: the
    // nearest station in the region decides.
    let mut unknown = scanned(&room, 4_096);
    unknown.cloud.scan_ranges.clear();
    unknown.cloud.scan_ranges_known = false;
    let nearest = run(indexed(&unknown));
    assert!(nearest
        .planes
        .iter()
        .all(|face| face.normal_source == NormalSource::NearestStation));
    // The station inside the room is the nearest to every face, also to
    // the outside faces, which it did not measure.
    face_through(&nearest, [0.0, 1.0, 0.0], [0.0, -0.1, 0.0]);
    face_through(&nearest, [0.0, 1.0, 0.0], [0.0; 3]);
}

#[test]
fn a_roof_plane_is_a_sloped_face() {
    // 3 by 2 m at 30 degrees, scanned from below.
    let roof = plane_with_hole([3.0, 2.0], 0.015, None)
        .flipped()
        .tilted(30.0)
        .with_noise(Noise::Gaussian(0.003), 4)
        .with_stations(&[[1.5, 1.5, -1.0]]);
    let scan = scanned(&roof, 4_096);
    let found = detect_surfaces(
        &[indexed(&scan)],
        0,
        EVERYWHERE,
        &SurfaceDetectConfig::default(),
        EVERY,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(found.planes.len(), 1);
    let face = &found.planes[0];
    assert_eq!(face.class, FaceClass::Sloped);
    assert_eq!(face.normal_source, NormalSource::Stations);
    let (sin, cos) = 30f64.to_radians().sin_cos();
    // 0.004 degree measured.
    assert!(
        angle_deg(face.normal, [0.0, sin, -cos]) < 0.1,
        "{:?}",
        face.normal
    );
    assert!(face.signed_distance([0.0; 3]).abs() < 0.001);
    // Free edges: 1.5 cm of points less in both directions (1.2 %).
    assert!((face.area - 2.985 * 1.985).abs() < 0.02, "{}", face.area);
    assert!(found.edges.is_empty());
    // The outline is drawn level and up the slope.
    assert!(face.u[2].abs() < 1e-9 && face.v[2] > 0.49);
}

/// Two level sheets of 2 by 2 m, the second `gap` above the first.
fn two_sheets(gap: f64) -> Shape {
    let low = plane_with_hole([2.0, 2.0], 0.01, None);
    let mut high = plane_with_hole([2.0, 2.0], 0.01, None);
    for point in &mut high.points {
        point[2] += gap;
    }
    low.merged(high).with_noise(Noise::Gaussian(0.003), 5)
}

#[test]
fn two_parallel_planes_five_centimetres_apart_stay_two_faces() {
    let found = detect(&two_sheets(0.05), &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 2);
    let heights: Vec<f64> = found.planes.iter().map(|face| face.origin[2]).collect();
    let (low, high) = (
        heights.iter().copied().fold(f64::INFINITY, f64::min),
        heights.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    );
    // 0.05 mm measured.
    assert!(
        low.abs() < 0.0005 && (high - 0.05).abs() < 0.0005,
        "{heights:?}"
    );
    for face in &found.planes {
        assert_eq!(face.residuals.points, 40_000);
        assert!((0.0028..0.0032).contains(&face.residuals.rms));
        assert!((face.area - 1.99 * 1.99).abs() < 0.01, "{}", face.area);
    }
    assert_ne!(
        found.planes[0].coplanar_group,
        found.planes[1].coplanar_group
    );
    assert!(found.edges.is_empty());
}

#[test]
fn a_step_in_a_wall_is_two_faces_or_one_by_the_tolerance() {
    // One sheet beside the other, 5 cm higher.
    let low = plane_with_hole([1.5, 2.0], 0.01, None);
    let mut high = plane_with_hole([1.5, 2.0], 0.01, None);
    for point in &mut high.points {
        point[0] += 1.5;
        point[2] += 0.05;
    }
    let wall = low.merged(high).with_noise(Noise::Gaussian(0.003), 6);
    let found = detect(&wall, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 2);
    for face in &found.planes {
        assert!((0.0028..0.0032).contains(&face.residuals.rms));
        assert!((face.area - 1.49 * 1.99).abs() < 0.02, "{}", face.area);
    }
    assert_ne!(
        found.planes[0].coplanar_group,
        found.planes[1].coplanar_group
    );

    let loose = detect(
        &wall,
        &SurfaceDetectConfig {
            distance_tolerance: 0.06,
            ..SurfaceDetectConfig::default()
        },
    );
    assert_eq!(loose.planes.len(), 1);
    let face = &loose.planes[0];
    assert_eq!(face.residuals.points, 60_000);
    // A plane through the step leaves half of it on either side at the
    // step and none in the middle of a part: 1.3 cm RMS measured.
    assert!(
        (0.011..0.016).contains(&face.residuals.rms),
        "{}",
        face.residuals.rms
    );
    assert!((face.area - 2.99 * 1.99).abs() < 0.03, "{}", face.area);
}

#[test]
fn two_parts_of_a_wall_across_a_gap_share_a_plane_number() {
    let left = plane_with_hole([1.5, 2.0], 0.01, None);
    let mut right = plane_with_hole([1.5, 2.0], 0.01, None);
    for point in &mut right.points {
        point[0] += 2.5;
    }
    let found = detect(
        &left.merged(right).with_noise(Noise::Gaussian(0.003), 7),
        &SurfaceDetectConfig::default(),
    );
    assert_eq!(found.planes.len(), 2);
    assert_eq!(
        found.planes[0].coplanar_group,
        found.planes[1].coplanar_group
    );
    assert_eq!(found.planes[0].coplanar_group, 1);
}

#[test]
fn a_plane_on_a_voxel_boundary_of_a_dense_scan_is_fitted_through_all_its_points() {
    // A point every 4 mm at height zero, where the lattice has a boundary:
    // half of the points fall in the voxels below and half in those above,
    // and the means of both layers lie 2.4 mm to their side of the plane.
    let sheet = plane_with_hole([1.6, 1.6], 0.004, None).with_noise(Noise::Gaussian(0.003), 8);
    assert_eq!(sheet.points.len(), 160_000);
    let found = detect(&sheet, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 1);
    let face = &found.planes[0];
    // 0.01 mm measured; a fit through one layer only is 2.4 mm off.
    assert!(face.signed_distance([0.8, 0.8, 0.0]).abs() < 0.0002);
    assert!(
        face.residuals.mean.abs() < 0.0002,
        "{}",
        face.residuals.mean
    );
    assert!(
        (0.0029..0.0031).contains(&face.residuals.rms),
        "{}",
        face.residuals.rms
    );
    let up = [0.0, 0.0, 1.0];
    assert!(angle_deg(face.normal, up).min(180.0 - angle_deg(face.normal, up)) < 0.02);
}

#[test]
fn a_dense_plane_that_crosses_a_voxel_boundary_at_a_slant_is_fitted_through_all_its_points() {
    // A point every 4 mm on a sheet that rises 8 mm over its 1.6 m and
    // crosses the boundary of the lattice at height zero on the way: at
    // its low end the points fill the voxels below, at its high end those
    // above, and in between both, in unequal numbers.
    let rise = 1.6 * 0.3f64.to_radians().sin();
    let mut sheet = plane_with_hole([1.6, 1.6], 0.004, None)
        .tilted(0.3)
        .with_noise(Noise::Gaussian(0.003), 13);
    for point in &mut sheet.points {
        point[2] -= 0.5 * rise;
    }
    let found = detect(&sheet, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 1);
    let face = &found.planes[0];
    let (sin, cos) = 0.3f64.to_radians().sin_cos();
    let truth = |x: f64, along: f64| [x, along * cos, along * sin - 0.5 * rise];
    for corner in [
        truth(0.0, 0.0),
        truth(1.6, 0.0),
        truth(0.0, 1.6),
        truth(1.6, 1.6),
    ] {
        // 0.05 mm measured.
        let off = face.signed_distance(corner).abs();
        assert!(off < 0.0002, "{off}");
    }
    assert!(
        face.residuals.mean.abs() < 0.0001,
        "{}",
        face.residuals.mean
    );
    assert!((0.0029..0.0031).contains(&face.residuals.rms));
}

#[test]
fn an_outline_closes_small_gaps_fills_small_holes_and_keeps_large_ones() {
    // A sheet of 2 by 2 m with a slit of 8 cm and one of 20 cm, both
    // half-way in from an edge, a square hole of 0.04 m2, which is wider
    // than a gap that is closed, and a round one of 0.28 m2.
    let mut sheet = plane_with_hole([2.0, 2.0], 0.01, Some(([1.2, 1.5], 0.3)));
    let keep: Vec<bool> = sheet
        .points
        .iter()
        .map(|point| {
            let narrow = point[0] > 0.30 && point[0] < 0.38 && point[1] < 1.0;
            let wide = point[0] > 0.60 && point[0] < 0.80 && point[1] < 1.0;
            let small = (point[0] - 1.5).abs() < 0.1 && (point[1] - 0.5).abs() < 0.1;
            !(narrow || wide || small)
        })
        .collect();
    let mut kept = keep.iter();
    sheet.points.retain(|_| *kept.next().unwrap());
    let mut kept = keep.iter();
    sheet.normals.retain(|_| *kept.next().unwrap());
    let mut kept = keep.iter();
    sheet.station_of.retain(|_| *kept.next().unwrap());
    let found = detect(
        &sheet.with_noise(Noise::Gaussian(0.003), 14),
        &SurfaceDetectConfig::default(),
    );
    assert_eq!(found.planes.len(), 1);
    let face = &found.planes[0];
    assert_eq!(face.patches.len(), 1);
    let patch = &face.patches[0];
    // The wide slit is a notch in the outline; the narrow one is closed.
    assert_eq!(patch.outer.len(), 8, "{:?}", patch.outer);
    let notch: Vec<[f64; 3]> = patch
        .outer
        .iter()
        .map(|corner| face.point(*corner))
        .filter(|corner| corner[0] > 0.1 && corner[0] < 1.9)
        .collect();
    assert_eq!(notch.len(), 4);
    for corner in notch {
        assert!(
            (corner[0] - 0.595).abs() < 0.003 || (corner[0] - 0.805).abs() < 0.003,
            "{corner:?}"
        );
        assert!(
            corner[1] < 0.01 || (corner[1] - 1.005).abs() < 0.003,
            "{corner:?}"
        );
    }
    // The large hole is an opening; the small one is filled.
    assert_eq!(patch.holes.len(), 1);
    let hole_area = -ring_signed_area(&patch.holes[0]);
    // A round hole of 0.283 m2 traced on cells and reduced to straight
    // segments: 0.261 m2 found.
    assert!((0.24..0.30).contains(&hole_area), "{hole_area}");
    let expected = 1.99 * 1.99 - 0.21 * 1.005 - hole_area;
    assert!(
        (face.area - expected).abs() < 0.01,
        "{} {expected}",
        face.area
    );
    // What was closed and filled holds no points, and that shows: 0.08 m2
    // of slit and 0.04 m2 of hole in 3.49 m2.
    assert!(
        (0.95..0.985).contains(&face.coverage()),
        "{}",
        face.coverage()
    );
    assert!((face.covered_area - face.coverage() * face.area).abs() < 1e-12);
}

#[test]
fn a_patch_below_the_smallest_area_is_no_face() {
    // 0.4 by 0.4 m is 0.16 m2. On a voxel boundary it fills two layers of
    // voxels, which count for twice its area; its outline decides.
    let small = plane_with_hole([0.4, 0.4], 0.005, None).with_noise(Noise::Gaussian(0.003), 15);
    let found = detect(&small, &SurfaceDetectConfig::default());
    assert!(found.planes.is_empty());
    // Its points were measured against a plane, but belong to no face.
    assert_eq!(found.assigned_points, 0);
    let large = plane_with_hole([0.6, 0.6], 0.005, None).with_noise(Noise::Gaussian(0.003), 15);
    let found = detect(&large, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 1);
    assert!((found.planes[0].area - 0.595 * 0.595).abs() < 0.005);
    assert_eq!(found.face(1).unwrap().id, 1);
    assert!(found.face(0).is_none() && found.face(2).is_none());
    assert_eq!(found.planes[0].rings().len(), 1);
    assert_eq!(found.planes[0].rings()[0].len(), 4);
}

#[test]
fn stray_points_make_no_face_and_spoil_none() {
    let space = Bounds {
        min: [0.0; 3],
        max: [4.0, 3.0, 2.6],
    };
    // Only noise: nothing.
    let noise = detect(
        &stray_points(space, 200_000, 9),
        &SurfaceDetectConfig::default(),
    );
    assert!(noise.planes.is_empty() && noise.edges.is_empty() && noise.cylinders.is_empty());
    assert_eq!((noise.source_points, noise.assigned_points), (200_000, 0));
    assert!(noise.working_points > 150_000);

    // A room with one stray point for every twenty of its own.
    let room = noisy_room().merged(stray_points(space, 7_550, 9));
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_room_faces(&found, 0.002, 0.001);
    assert_eq!(found.edges.len(), 12);
    for face in &found.planes {
        // A stray lies in no surface that runs along a face, so it counts
        // as a point of a face only within the tolerance of it: about 50
        // per face, which leave the RMS at 3.0 to 3.1 mm (measured) and the
        // largest deviation at the tolerance.
        assert!(
            (0.0028..0.0033).contains(&face.residuals.rms),
            "{}",
            face.residuals.rms
        );
        assert!(
            face.residuals.max > 0.012 && face.residuals.max < 0.03,
            "{}",
            face.residuals.max
        );
        assert!(
            (0.0056..0.0062).contains(&face.residuals.p95),
            "{}",
            face.residuals.p95
        );
    }
    assert!(
        found.assigned_points > 151_000 && found.assigned_points < 151_600,
        "{}",
        found.assigned_points
    );
}

#[test]
fn a_budget_below_the_region_gives_larger_voxels() {
    let room = noisy_room();
    let with_budget = |budget: usize| {
        detect(
            &room,
            &SurfaceDetectConfig {
                max_working_points: budget,
                ..SurfaceDetectConfig::default()
            },
        )
    };
    // 88,031 voxels of 3 cm, 27,751 of 6 cm, 6,876 of 12 cm.
    let found = with_budget(20_000);
    assert_eq!(found.voxel_size, 0.12);
    assert_eq!(found.working_points, 6_876);
    // The outline grid is never finer than the voxels.
    assert_eq!(found.boundary_cell, 0.12);
    assert!(found.is_coarse());
    assert_eq!(found.assigned_points, 151_000);
    // The room is still found whole: areas within 0.2 % (0.02 % measured).
    assert_room_faces(&found, 0.002, 0.001);
    assert_eq!(found.edges.len(), 12);
    for face in &found.planes {
        assert!((0.0028..0.0032).contains(&face.residuals.rms));
        assert!(face.residuals.mean.abs() < 0.0003);
    }
    // One size finer fits a budget just above its count.
    let finer = with_budget(28_000);
    assert_eq!((finer.voxel_size, finer.working_points), (0.06, 27_751));
    // One doubling is coarse already: narrow faces can be lost. None of the
    // growth came from the density of the points.
    assert!(finer.is_coarse());
    assert_eq!((finer.density_doublings, found.density_doublings), (0, 0));
    // Eight times the voxel: the faces remain, less exactly (1.2 mm and
    // 0.1 % measured).
    let coarse = with_budget(3_000);
    assert_eq!(coarse.voxel_size, 0.24);
    assert_room_faces(&coarse, 0.005, 0.002);
    // Voxels this large hold so many points that the two layers of a plane
    // on a voxel boundary are each very even; the fit still takes both.
    // 0.11 mm measured; with one layer only, 2.4 mm.
    for face in &coarse.planes {
        assert!(
            face.residuals.mean.abs() < 0.0005,
            "{}",
            face.residuals.mean
        );
    }
}

#[test]
fn a_rotated_room_at_national_coordinates_is_found_as_at_the_origin() {
    let turn = 27.0f64;
    let shift = [207_000.0, 474_000.0, 12.0];
    let room = noisy_room().transformed(turn, shift);
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 6);
    assert_eq!(classes(&found), [1, 1, 4, 0]);
    let place = |point: [f64; 3]| -> [f64; 3] {
        let (sin, cos) = turn.to_radians().sin_cos();
        [
            cos * point[0] - sin * point[1] + shift[0],
            sin * point[0] + cos * point[1] + shift[1],
            point[2] + shift[2],
        ]
    };
    let corners: Vec<[f64; 3]> = room_corners().into_iter().map(place).collect();
    for (normal, on, area, _) in ROOM_FACES {
        let turned = difference(place(normal), place([0.0; 3]));
        let face = face_through(&found, turned, place(on));
        // The floor is traced on a grid that does not follow its edges;
        // the corners still land on the lines of the walls. 0.03 % measured.
        assert!((face.area - area).abs() < 0.002 * area, "{}", face.area);
        assert!((0.0028..0.0032).contains(&face.residuals.rms));
        assert_eq!(
            face.patches[0].outer.len(),
            4,
            "{:?}",
            face.patches[0].outer
        );
        for corner in corners_3d(face) {
            let nearest = corners
                .iter()
                .map(|known| apart(*known, corner))
                .fold(f64::INFINITY, f64::min);
            // 0.3 mm measured.
            assert!(nearest < 0.001, "{nearest}");
        }
    }
    assert_eq!(found.edges.len(), 12);
}

#[test]
fn index_stream_and_memory_give_the_same_faces_and_an_index_reads_only_near_the_region() {
    let room = noisy_room();
    let scan = scanned(&room, 2_048);
    let records = records(&room);
    // The half of the room with x up to 2.
    let half = Bounds {
        min: [-1.0, -1.0, -1.0],
        max: [2.0, 4.0, 4.0],
    };
    let config = SurfaceDetectConfig::default();
    let run = |source: SurfaceSource<'_>, region: Bounds| {
        detect_surfaces(&[source], 0, region, &config, EVERY, &mut |_| Ok(())).unwrap()
    };
    let from_index = run(indexed(&scan), half);
    let stream = SurfaceSource {
        points: RegionSource::new(&scan.cloud, None, SourceTransform::default()),
        cloud: Some(&scan.cloud),
    };
    let from_stream = run(stream, half);
    let memory = SurfaceSource {
        cloud: Some(&scan.cloud),
        ..resident(&records)
    };
    let from_memory = run(memory, half);
    // The stream reads every point of the file, the index the leaves that
    // touch the box: 53 % of the points here.
    assert_eq!(from_stream.read_points, 151_000);
    assert!(
        from_index.read_points < 100_000,
        "{}",
        from_index.read_points
    );
    assert!(from_index.read_points >= from_index.source_points);
    let same = |mut other: DetectedSurfaces| {
        other.read_points = from_index.read_points;
        assert!(other == from_index);
    };
    same(from_stream);
    same(from_memory);
    // Five faces of the half room, cut off at the box: no east wall.
    assert_eq!(from_index.planes.len(), 5);
    assert_eq!(classes(&from_index), [1, 1, 3, 0]);
    let floor = face_through(&from_index, [0.0, 0.0, 1.0], [0.0; 3]);
    // Up to the last points before x = 2, at 1.99.
    assert!((floor.area - 1.99 * 3.0).abs() < 0.01, "{}", floor.area);
    assert_eq!(from_index.edges.len(), 8);
    assert!(from_index
        .planes
        .iter()
        .all(|face| face.normal_source == NormalSource::Stations));
    // The whole room from the index is the whole room from memory.
    let whole = run(indexed(&scan), EVERYWHERE);
    assert_eq!(whole.read_points, 151_000);
    assert_room_faces(&whole, 0.002, 0.001);
}

#[test]
fn the_result_is_the_same_on_every_run_and_for_every_number_of_threads() {
    let room = box_room(&RoomSpec {
        openings: vec![Opening::window(Wall::East, 0.8)],
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 10);
    let scan = scanned(&room, 1_024);
    let run = |threads: usize, budget: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(|| {
                detect_surfaces(
                    &[indexed(&scan)],
                    0,
                    EVERYWHERE,
                    &SurfaceDetectConfig {
                        max_working_points: budget,
                        ..SurfaceDetectConfig::default()
                    },
                    EVERY,
                    &mut |_| Ok(()),
                )
                .unwrap()
            })
    };
    let first = run(1, 1_500_000);
    assert_eq!(first.planes.len(), 6);
    assert!(run(1, 1_500_000) == first);
    assert!(run(3, 1_500_000) == first);
    assert!(run(8, 1_500_000) == first);
    // With a budget that makes the voxels grow as well.
    let coarse = run(1, 30_000);
    assert_eq!((coarse.voxel_size, coarse.planes.len()), (0.06, 6));
    assert!(run(5, 30_000) == coarse);
}

#[test]
fn a_room_spread_over_two_layers_is_one_room() {
    let room = noisy_room();
    let all = records(&room);
    // The second layer holds the half with x above 2, in a source frame of
    // its own: millimetres, mirrored in y and moved.
    let transform = SourceTransform {
        scale: [0.001, -0.001, 0.001],
        offset: [5.0, -2.0, 0.25],
    };
    let first: Vec<IndexedPoint> = all
        .iter()
        .filter(|record| record.point.xyz[0] < 2.0)
        .copied()
        .collect();
    let second: Vec<IndexedPoint> = all
        .iter()
        .filter(|record| record.point.xyz[0] >= 2.0)
        .map(|record| {
            let mut record = *record;
            record.point.xyz = transform.source_xyz(record.point.xyz).unwrap();
            record
        })
        .collect();
    let sources = [
        resident(&first),
        SurfaceSource {
            points: RegionSource::resident(&second, transform),
            cloud: None,
        },
    ];
    let config = SurfaceDetectConfig::default();
    let found = detect_surfaces(&sources, 1, EVERYWHERE, &config, EVERY, &mut |_| Ok(())).unwrap();
    assert_eq!(found.placement, transform);
    assert_eq!(found.source_points, 151_000);
    // The same room as from one layer; the way through the source frame
    // costs the last digits only.
    assert_room_faces(&found, 0.002, 0.001);
    assert_eq!(found.edges.len(), 12);
    let single = detect(&room, &config);
    for (a, b) in found.planes.iter().zip(&single.planes) {
        assert_eq!((a.id, a.class), (b.id, b.class));
        assert!((a.area - b.area).abs() < 1e-6);
        assert!((a.residuals.rms - b.residuals.rms).abs() < 1e-7);
        assert!(a.residuals.points.abs_diff(b.residuals.points) <= 1);
    }
    // The filter names the layer and the ordinal of every point.
    let only_first = detect_surfaces(
        &sources,
        0,
        EVERYWHERE,
        &config,
        &|source, _, _| source == 0,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(only_first.placement, SourceTransform::default());
    assert_eq!(only_first.planes.len(), 5);
    assert!(only_first.source_points < 80_000);
}

#[test]
fn a_job_can_be_stopped_in_every_stage() {
    let room = noisy_room();
    let scan = scanned(&room, 4_096);
    let config = SurfaceDetectConfig::default();
    let mut stages = Vec::new();
    let mut last = 0.0f32;
    let whole = detect_surfaces(
        &[indexed(&scan)],
        0,
        EVERYWHERE,
        &config,
        EVERY,
        &mut |state| {
            if stages.last() != Some(&state.stage) {
                stages.push(state.stage);
            }
            // The share of the job that is done never goes back.
            assert!(state.fraction() >= last && state.fraction() <= 1.0);
            last = state.fraction();
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(whole.planes.len(), 6);
    assert_eq!(
        stages,
        [
            SurfaceStage::Reading,
            SurfaceStage::Segmenting,
            SurfaceStage::Measuring,
            SurfaceStage::Outlining
        ]
    );
    assert_eq!(last, 1.0);
    for stage in stages {
        let mut calls = 0;
        let stopped = detect_surfaces(
            &[indexed(&scan)],
            0,
            EVERYWHERE,
            &config,
            EVERY,
            &mut |state| {
                if state.stage == stage {
                    calls += 1;
                }
                // Not at the first report of a stage, but in the middle. A
                // thread that was about to report may still do so once the
                // job is stopped, and gets the same answer.
                if calls >= 2 {
                    assert_eq!(state.stage, stage, "the job went on to the next stage");
                    return Err(LoadError::Cancelled);
                }
                Ok(())
            },
        );
        assert!(
            matches!(stopped, Err(LoadError::Cancelled)),
            "{stage:?} gave {:?}",
            stopped.map(|found| found.planes.len())
        );
    }
    // Stopped before anything is read.
    let mut reads = 0;
    let stopped = detect_surfaces(
        &[indexed(&scan)],
        0,
        EVERYWHERE,
        &config,
        &|_, _, _| panic!("a point was read"),
        &mut |_| {
            reads += 1;
            Err(LoadError::Cancelled)
        },
    );
    assert!(matches!(stopped, Err(LoadError::Cancelled)));
    assert_eq!(reads, 1);
}

#[test]
fn a_region_without_points_gives_an_empty_result() {
    let room = noisy_room();
    let records = records(&room);
    let config = SurfaceDetectConfig::default();
    let run = |sources: &[SurfaceSource<'_>], region: Bounds, accept: &RegionFilter<'_>| {
        detect_surfaces(sources, 0, region, &config, accept, &mut |_| Ok(()))
    };
    let empty = |found: DetectedSurfaces| {
        assert!(found.planes.is_empty() && found.edges.is_empty());
        assert_eq!(found.region, None);
        assert_eq!((found.source_points, found.working_points), (0, 0));
        assert_eq!(found.voxel_size, 0.03);
        // An empty result still exports as a document and as meshes.
        assert_eq!(
            faces_json(&found, "room.e57")["faces"],
            serde_json::json!([])
        );
        assert!(flat_mesh(&found).unwrap().vertices.is_empty());
        assert!(deviation_mesh(&found, 0.02, 1_000)
            .unwrap()
            .triangles
            .is_empty());
    };
    // A box beside the room, a box inside it that holds no point, a filter
    // that takes nothing, and a layer without points.
    let beside = Bounds {
        min: [10.0; 3],
        max: [12.0; 3],
    };
    empty(run(&[resident(&records)], beside, EVERY).unwrap());
    let inside = Bounds {
        min: [1.0, 1.0, 1.0],
        max: [2.0, 2.0, 2.0],
    };
    empty(run(&[resident(&records)], inside, EVERY).unwrap());
    empty(run(&[resident(&records)], EVERYWHERE, &|_, _, _| false).unwrap());
    empty(run(&[resident(&[])], EVERYWHERE, EVERY).unwrap());
    // A box that holds a corner of the floor only: points, but too few for
    // a face of a quarter of a square metre.
    let corner = Bounds {
        min: [-1.0, -1.0, -1.0],
        max: [0.3, 0.3, 0.005],
    };
    let found = run(&[resident(&records)], corner, EVERY).unwrap();
    assert!(found.planes.is_empty());
    assert!(found.source_points > 100 && found.region.is_some());

    // What cannot be a job at all is refused.
    let backwards = Bounds {
        min: [1.0; 3],
        max: [0.0; 3],
    };
    assert!(matches!(
        run(&[resident(&records)], backwards, EVERY),
        Err(LoadError::InvalidData(_))
    ));
    assert!(matches!(
        run(&[], EVERYWHERE, EVERY),
        Err(LoadError::InvalidData(_))
    ));
    assert!(matches!(
        detect_surfaces(
            &[resident(&records)],
            1,
            EVERYWHERE,
            &config,
            EVERY,
            &mut |_| Ok(())
        ),
        Err(LoadError::InvalidData(_))
    ));
}

#[test]
fn settings_that_cannot_work_are_refused() {
    let good = SurfaceDetectConfig::default();
    assert!(good.validate().is_ok());
    let refused = |change: &dyn Fn(&mut SurfaceDetectConfig)| {
        let mut config = good.clone();
        change(&mut config);
        assert!(matches!(config.validate(), Err(LoadError::InvalidData(_))));
        // And the job does not start.
        assert!(detect_surfaces(&[], 0, EVERYWHERE, &config, EVERY, &mut |_| Ok(())).is_err());
    };
    for bad in [0.0, -0.01, f64::NAN, f64::INFINITY] {
        refused(&|config| config.distance_tolerance = bad);
        refused(&|config| config.min_region_area = bad);
        refused(&|config| config.voxel_size = bad);
        refused(&|config| config.boundary_cell = bad);
        refused(&|config| config.min_plane_width = bad);
        refused(&|config| config.curvature_max = bad);
    }
    for bad in [-0.01, f64::NAN] {
        refused(&|config| config.max_gap = bad);
        refused(&|config| config.min_hole_area = bad);
    }
    // A gap in millimetres, or anything else beyond a few metres.
    for bad in [5.01, 100.0, 1e12, f64::INFINITY] {
        refused(&|config| config.max_gap = bad);
    }
    for bad in [0.5, 45.5, f64::NAN] {
        refused(&|config| config.angle_tolerance_deg = bad);
    }
    refused(&|config| config.max_working_points = 999);
    for bad in [0.0, -1.0, f64::NAN] {
        refused(&|config| config.min_radius = bad);
        refused(&|config| config.min_cylinder_length = bad);
    }
    refused(&|config| config.max_radius = 0.005);
    refused(&|config| config.max_radius = f64::NAN);
    for bad in [5.0, 361.0, f64::NAN] {
        refused(&|config| config.min_arc_deg = bad);
    }
    let mut edge = good.clone();
    edge.angle_tolerance_deg = 45.0;
    edge.max_working_points = 1_000;
    edge.max_gap = 0.0;
    edge.min_hole_area = 0.0;
    assert!(edge.validate().is_ok());
    edge.max_gap = MAX_GAP;
    assert!(edge.validate().is_ok());
}

fn windowed_room() -> Shape {
    box_room(&RoomSpec {
        openings: vec![Opening::window(Wall::East, 0.8)],
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 11)
}

#[test]
fn faces_follow_their_layer_when_it_is_moved_or_mirrored() {
    let found = detect(&windowed_room(), &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 6);

    // Moved: every position moves along and nothing else changes.
    let shift = [10.0, -4.0, 0.5];
    let moved = found
        .placed(SourceTransform {
            scale: [1.0; 3],
            offset: shift,
        })
        .unwrap();
    assert_eq!(moved.placement.offset, shift);
    for (before, after) in found.planes.iter().zip(&moved.planes) {
        assert_eq!((before.id, before.class), (after.id, after.class));
        assert_eq!(before.normal, after.normal);
        assert!((before.area - after.area).abs() < 1e-9);
        assert_eq!(before.residuals, after.residuals);
        for (a, b) in corners_3d(before).into_iter().zip(corners_3d(after)) {
            assert!(apart([a[0] + shift[0], a[1] + shift[1], a[2] + shift[2]], b) < 1e-9);
        }
        assert!(apart(before.deviation.corner(3, 2), after.deviation.corner(3, 2)) > 10.0);
    }
    for (before, after) in found.edges.iter().zip(&moved.edges) {
        assert!((before.length() - after.length()).abs() < 1e-9);
        assert!((after.start[2] - before.start[2] - 0.5).abs() < 1e-9);
        assert_eq!(before.angle_deg, after.angle_deg);
    }
    let region = moved.region.unwrap();
    assert!((region.min[0] - found.region.unwrap().min[0] - 10.0).abs() < 1e-9);

    // Twice as large and upside down: floor and ceiling change places,
    // areas grow four times and distances twice.
    let turned = found
        .placed(SourceTransform {
            scale: [2.0, 2.0, -2.0],
            offset: [0.0; 3],
        })
        .unwrap();
    for (before, after) in found.planes.iter().zip(&turned.planes) {
        let expected = match before.class {
            FaceClass::Floor => FaceClass::Ceiling,
            FaceClass::Ceiling => FaceClass::Floor,
            other => other,
        };
        assert_eq!(after.class, expected);
        assert!((after.area - 4.0 * before.area).abs() < 1e-6);
        assert!((after.covered_area - 4.0 * before.covered_area).abs() < 1e-6);
        assert!((after.residuals.rms - 2.0 * before.residuals.rms).abs() < 1e-12);
        assert!((after.residuals.max - 2.0 * before.residuals.max).abs() < 1e-12);
        assert_eq!(after.residuals.points, before.residuals.points);
        // Still a proper outline in its own plane: counter-clockwise
        // outside, clockwise round an opening, and on the plane.
        for patch in &after.patches {
            assert!(ring_signed_area(&patch.outer) > 0.0);
            assert!(patch.holes.iter().all(|hole| ring_signed_area(hole) < 0.0));
        }
        assert_eq!(after.patches[0].holes.len(), before.patches[0].holes.len());
        // Mirrored rings run the other way, so the corners are compared as
        // a set.
        let mapped: Vec<[f64; 3]> = corners_3d(before)
            .into_iter()
            .map(|corner| [2.0 * corner[0], 2.0 * corner[1], -2.0 * corner[2]])
            .collect();
        for corner in corners_3d(after) {
            assert!(mapped.iter().any(|known| apart(*known, corner) < 1e-9));
            assert!(after.signed_distance(corner).abs() < 1e-9);
        }
        // The normal still points into the room.
        assert!(after.signed_distance([4.0, 3.0, -2.6]) > 0.0);
        let n = cross(after.u, after.v);
        assert!((0..3).all(|axis| (n[axis] - after.normal[axis]).abs() < 1e-9));
    }
    assert!(turned
        .edges
        .iter()
        .all(|edge| (edge.angle_deg - 90.0).abs() < 0.05));
    // And back again gives what was found, to rounding.
    let back = turned.placed(SourceTransform::default()).unwrap();
    for (before, after) in found.planes.iter().zip(&back.planes) {
        assert_eq!(before.class, after.class);
        assert!((before.area - after.area).abs() < 1e-9);
        assert!(angle_deg(before.normal, after.normal) < 1e-6);
        assert!((before.residuals.p95 - after.residuals.p95).abs() < 1e-12);
    }
    // A zero scale has no answer.
    assert!(found
        .placed(SourceTransform {
            scale: [1.0, 0.0, 1.0],
            offset: [0.0; 3],
        })
        .is_none());
}

/// A mesh after the transform of its layer, as the viewer shows it: the
/// summed area of its triangles and the positions of its corners.
fn shown(mesh: &MeshGeometry, transform: SourceTransform) -> (f64, Vec<[f64; 3]>) {
    let positions: Vec<[f64; 3]> = mesh
        .vertices
        .iter()
        .map(|xyz| transform.xyz(*xyz))
        .collect();
    let area = mesh
        .triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.map(|index| positions[index as usize]);
            let normal = cross(difference(b, a), difference(c, a));
            0.5 * dot(normal, normal).sqrt()
        })
        .sum();
    (area, positions)
}

#[test]
fn viewer_meshes_are_in_the_source_frame_of_the_layer() {
    // The layer as a source in millimetres, mirrored in x.
    let transform = SourceTransform {
        scale: [-0.001, 0.001, 0.001],
        offset: [4.0, 0.0, 0.0],
    };
    let source_records: Vec<IndexedPoint> = records(&windowed_room())
        .into_iter()
        .map(|mut record| {
            record.point.xyz = transform.source_xyz(record.point.xyz).unwrap();
            record
        })
        .collect();
    let source = SurfaceSource {
        points: RegionSource::resident(&source_records, transform),
        cloud: None,
    };
    let config = SurfaceDetectConfig::default();
    let found = detect_surfaces(&[source], 0, EVERYWHERE, &config, EVERY, &mut |_| Ok(())).unwrap();
    assert_eq!(found.planes.len(), 6);
    let total_area: f64 = found.planes.iter().map(|face| face.area).sum();

    let flat = flat_mesh(&found).unwrap();
    let (area, positions) = shown(&flat, transform);
    // The triangles cover the outlines exactly.
    assert!((area - total_area).abs() < 1e-6, "{area} {total_area}");
    // 4 corners per face, and 4 more round the window.
    assert_eq!(flat.vertices.len(), 6 * 4 + 4);
    let colors = flat.colors.as_ref().unwrap();
    let normals = flat.normals.as_ref().unwrap();
    assert_eq!(
        (colors.len(), normals.len()),
        (flat.vertices.len(), flat.vertices.len())
    );
    // Source coordinates: millimetres.
    assert!(flat.vertices.iter().any(|xyz| xyz[2] > 2_000.0));
    for triangle in &flat.triangles {
        let [a, b, c] = triangle.map(|index| positions[index as usize]);
        let face = found
            .planes
            .iter()
            .find(|face| {
                [a, b, c]
                    .iter()
                    .all(|corner| face.signed_distance(*corner).abs() < 1e-6)
            })
            .expect("a triangle lies in the plane of a face");
        // As shown, a triangle turns the way its face looks. For a
        // mirrored layer the viewer reverses the stored normal after it
        // divides it by the scale, so it is stored reversed.
        let winding = cross(difference(b, a), difference(c, a));
        assert!(dot(winding, face.normal) > 0.0);
        let stored = normals[triangle[0] as usize].map(f64::from);
        let viewer: [f64; 3] = std::array::from_fn(|axis| -stored[axis] / transform.scale[axis]);
        assert!(angle_deg(viewer, face.normal) < 0.01);
        assert_eq!(colors[triangle[0] as usize], face_color(face));
    }
    // Classes have their colours; shades tell faces of one class apart.
    assert_ne!(face_color(&found.planes[0]), face_color(&found.planes[2]));
    assert_ne!(face_color(&found.planes[2]), face_color(&found.planes[3]));
    assert_ne!(class_color(FaceClass::Wall), class_color(FaceClass::Sloped));

    // The deviation mesh: the cells that hold points, cut off at the
    // outlines and at the window. 99.9 % of the area of the outlines.
    let heat = deviation_mesh(&found, config.distance_tolerance, DEFAULT_DEVIATION_CELLS).unwrap();
    let (area, positions) = shown(&heat, transform);
    assert!(
        area <= total_area + 1e-6 && area > 0.995 * total_area,
        "{area} {total_area}"
    );
    assert_eq!(heat.triangles.len() % 2, 0);
    // About one corner per 5 cm cell of 59 m2.
    assert!(
        (23_000..27_000).contains(&heat.vertices.len()),
        "{}",
        heat.vertices.len()
    );
    for position in &positions {
        assert!(found
            .planes
            .iter()
            .any(|face| face.signed_distance(*position).abs() < 1e-6));
    }
    // 3 mm of noise against a scale of 2 cm: pale colours, both ways.
    let colors = heat.colors.as_ref().unwrap();
    let legend = deviation_legend(config.distance_tolerance);
    assert_eq!(legend.map(|stop| stop.value), [-0.02, 0.0, 0.02]);
    // Nearly white all over (24,540 of 24,544 corners measured); a cell in
    // a corner that holds a point or two shows more.
    let pale = colors.iter().filter(|color| color[1] > 200).count();
    assert!(pale * 1_000 >= colors.len() * 999, "{pale}");
    assert!(colors.iter().all(|color| color[1] > 100));
    assert!(colors.iter().any(|color| color[0] > color[2]));
    assert!(colors.iter().any(|color| color[2] > color[0]));
    assert_eq!(deviation_color(0.0, 0.02), legend[1].color);
    assert_eq!(deviation_color(-0.02, 0.02), legend[0].color);
    assert_eq!(deviation_color(-1.0, 0.02), legend[0].color);
    assert_eq!(deviation_color(0.05, 0.02), legend[2].color);
    assert_eq!(deviation_color(f64::NAN, 0.02), legend[1].color);
    let half = deviation_color(0.01, 0.02);
    assert!(half[0] > legend[2].color[0] && half[0] < legend[1].color[0]);

    // A limit on the cells merges them: four times fewer per step.
    let merged = deviation_mesh(&found, config.distance_tolerance, 8_000).unwrap();
    assert!(merged.triangles.len() <= 2 * 8_000);
    assert!(
        merged.triangles.len() > 2 * 5_000,
        "{}",
        merged.triangles.len()
    );
    let (merged_area, _) = shown(&merged, transform);
    // A merged cell at a corner of the window covers a little of the
    // opening: 0.04 % more than the outlines measured.
    assert!(
        (merged_area - total_area).abs() < 0.01 * total_area,
        "{merged_area} {total_area}"
    );
}

#[test]
fn faces_export_as_obj_and_as_json() {
    let room = box_room(&RoomSpec {
        openings: vec![
            Opening::door(Wall::South, 1.0),
            Opening::window(Wall::East, 0.8),
        ],
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 12);
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 6);
    let directory = tempfile::tempdir().unwrap();

    let obj = directory.path().join("faces.obj");
    write_faces_obj(&found, &obj, &["Source: room.e57"]).unwrap();
    let text = std::fs::read_to_string(&obj).unwrap();
    let groups: Vec<&str> = text.lines().filter(|line| line.starts_with("g ")).collect();
    assert_eq!(groups.len(), 6);
    for (group, face) in groups.iter().zip(&found.planes) {
        assert_eq!(
            *group,
            format!("g face_{:04}_{}", face.id, face.class.name())
        );
    }
    assert!(text.starts_with("# Faces detected by Open Pointcloud Studio\n# Source: room.e57\n"));
    let mesh = crate::read_obj_mesh(&obj).unwrap();
    let total_area: f64 = found.planes.iter().map(|face| face.area).sum();
    let (area, positions) = shown(&mesh, SourceTransform::default());
    assert!((area - total_area).abs() < 1e-9 * total_area);
    // 4 corners per face, 4 more for the door and 4 for the window.
    assert_eq!(mesh.vertices.len(), 6 * 4 + 8);
    let normals = mesh.normals.as_ref().expect("a normal per corner");
    for triangle in &mesh.triangles {
        let [a, b, c] = triangle.map(|index| positions[index as usize]);
        let winding = cross(difference(b, a), difference(c, a));
        assert!(dot(winding, normals[triangle[0] as usize].map(f64::from)) > 0.0);
    }

    let json = directory.path().join("faces.json");
    write_faces_json(&found, "C:/scans/private/room.e57", &json).unwrap();
    let text = std::fs::read_to_string(&json).unwrap();
    assert!(!text.contains("private") && !text.contains("scans"));
    let document: serde_json::Value = serde_json::from_str(&text).unwrap();
    // Reading a number back can differ in its last digit.
    let near = |value: &serde_json::Value, expected: f64| {
        (value.as_f64().unwrap() - expected).abs() <= 1e-12 * expected.abs().max(1.0)
    };
    assert_eq!(document["format"], FACES_JSON_FORMAT);
    assert_eq!(document["version"], 1);
    assert_eq!(document["source"], "room.e57");
    assert_eq!(document["units"], "metres");
    assert_eq!(document["settings"]["distance_tolerance"], 0.02);
    assert_eq!(document["settings"]["voxel_size"], 0.03);
    assert_eq!(document["settings"]["voxel_size_asked"], 0.03);
    assert_eq!(document["settings"]["boundary_cell_asked"], 0.05);
    assert_eq!(document["settings"]["coarse"], false);
    assert_eq!(document["settings"]["density_doublings"], 0);
    assert_eq!(document["points"]["source"], found.source_points);
    assert!(document["region"]["max"][2].as_f64().unwrap() > 2.5);
    let faces = document["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 6);
    // Every number in the document is a number: one that is not finite
    // would have been written as null.
    fn all_numbers(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Null => false,
            serde_json::Value::Array(items) => items.iter().all(all_numbers),
            serde_json::Value::Object(fields) => fields.values().all(all_numbers),
            _ => true,
        }
    }
    assert!(all_numbers(&document));
    for (entry, face) in faces.iter().zip(&found.planes) {
        assert_eq!(entry["id"], face.id);
        assert_eq!(entry["type"], "plane");
        assert_eq!(entry["class"], face.class.name());
        assert_eq!(entry["normal_from"], "open_side");
        assert!(near(&entry["area"], face.area));
        assert!(near(&entry["coverage"], face.coverage()));
        assert!(near(&entry["residual"]["rms"], face.residuals.rms));
        assert!(near(&entry["residual"]["p95"], face.residuals.p95));
        assert!(near(&entry["residual"]["max"], face.residuals.max));
        assert!(near(&entry["point"][0], face.origin[0]));
        assert_eq!(entry["residual"]["points"], face.residuals.points);
        // The plane equation holds for the corners of the outline.
        let normal: Vec<f64> = entry["normal"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_f64().unwrap())
            .collect();
        let offset = entry["offset"].as_f64().unwrap();
        let boundary = entry["boundary"].as_array().unwrap();
        assert_eq!(boundary.len(), 1);
        let outer = boundary[0]["outer"].as_array().unwrap();
        assert_eq!(outer.len(), face.patches[0].outer.len());
        for corner in outer {
            let along: f64 = (0..3)
                .map(|axis| normal[axis] * corner[axis].as_f64().unwrap())
                .sum();
            assert!((along - offset).abs() < 1e-9);
        }
        assert_eq!(
            boundary[0]["holes"].as_array().unwrap().len(),
            face.patches[0].holes.len()
        );
    }
    assert_eq!(
        faces.iter().filter(|face| face["class"] == "wall").count(),
        4
    );
    let edges = document["edges"].as_array().unwrap();
    assert_eq!(edges.len(), found.edges.len());
    assert_eq!(edges[0]["faces"], serde_json::json!(found.edges[0].faces));
    assert!(near(&edges[0]["length"], found.edges[0].length()));
    assert!(near(&edges[0]["start"][1], found.edges[0].start[1]));
    assert!(near(&edges[0]["angle_deg"], found.edges[0].angle_deg));

    // Nothing to export leaves an existing file as it is, and no
    // temporary file behind.
    let nothing =
        DetectedSurfaces::empty(&SurfaceDetectConfig::default(), SourceTransform::default());
    let before = std::fs::read(&obj).unwrap();
    assert!(write_faces_obj(&nothing, &obj, &[]).is_err());
    assert!(write_faces_obj(&found, &obj, &["two\nlines"]).is_err());
    assert_eq!(std::fs::read(&obj).unwrap(), before);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

/// A column of 2.6 m with a radius of 15 cm, of which the half that faces
/// a station was scanned.
fn half_column(base: [f64; 3], seed: u64) -> (CylinderSpec, Shape) {
    let spec = CylinderSpec {
        arc_degrees: 180.0,
        ..CylinderSpec::column(base, 0.15, 2.6)
    };
    let shape = cylinder(&spec).with_noise(Noise::Gaussian(0.003), seed);
    (spec, shape)
}

/// The distance of a position to the line through two points.
fn off_axis(position: [f64; 3], start: [f64; 3], end: [f64; 3]) -> f64 {
    let along = difference(end, start);
    let from = difference(position, start);
    let across = cross(along, from);
    (dot(across, across) / dot(along, along)).sqrt()
}

#[test]
fn a_column_is_a_cylinder_with_its_axis_radius_length_and_arc() {
    let (spec, column) = half_column([1.0, 1.0, 0.0], 21);
    assert_eq!(column.points.len(), 3_120);
    let middle = spec.frame()[1];
    // Seen from a station 3 m in front of the scanned half.
    let station: [f64; 3] = std::array::from_fn(|axis| [1.0, 1.0, 1.3][axis] + 3.0 * middle[axis]);
    let scan = scanned(&column.clone().with_stations(&[station]), 4_096);
    let config = SurfaceDetectConfig::default();
    let found = detect_surfaces(
        &[indexed(&scan)],
        0,
        EVERYWHERE,
        &config,
        EVERY,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert!(found.planes.is_empty() && found.edges.is_empty());
    assert_eq!(found.cylinders.len(), 1);
    let face = &found.cylinders[0];
    assert_eq!(face.id, 1);
    assert_eq!(found.cylinder(1).unwrap().id, 1);
    assert!(found.face(1).is_none() && found.cylinder(2).is_none());
    assert!(!face.seen_from_inside);
    // Radius within 1 mm (0.1 mm measured), the axis within 1 mm of its
    // place (0.2 mm) and within 0.1 degree of upright (0.01).
    assert!((face.radius - 0.15).abs() < 0.001, "{}", face.radius);
    assert_eq!(face.diameter(), 2.0 * face.radius);
    for end in [face.axis_start, face.axis_end] {
        assert!((end[0] - 1.0).hypot(end[1] - 1.0) < 0.001, "{end:?}");
    }
    assert!(
        angle_deg(face.axis(), [0.0, 0.0, 1.0]).min(angle_deg(face.axis(), [0.0, 0.0, -1.0])) < 0.1
    );
    // The scanned length and arc end at the outermost points, half a point
    // spacing in: 2.58 m, and 180 degrees less one step of 7.6.
    assert!((face.length() - 2.58).abs() < 0.005, "{}", face.length());
    assert!((face.arc_deg - 172.4).abs() < 1.0, "{}", face.arc_deg);
    // The arc runs from its start, through the middle of the scanned half.
    assert!(angle_deg(face.outward(0.5 * face.arc_deg), middle) < 1.0);
    assert!(dot(face.arc_start, face.axis()).abs() < 1e-9);
    assert!(dot(face.arc_start, face.arc_side).abs() < 1e-9);
    assert!((face.area() - 0.15 * 172.4f64.to_radians() * 2.58).abs() < 0.01);
    // Every point is on it, with the noise it was given.
    assert_eq!(face.residuals.points, 3_120);
    assert_eq!(found.assigned_points, 3_120);
    assert!(
        (0.0028..0.0032).contains(&face.residuals.rms),
        "{}",
        face.residuals.rms
    );
    assert!(
        face.residuals.mean.abs() < 0.0003,
        "{}",
        face.residuals.mean
    );
    assert!((0.0054..0.0064).contains(&face.residuals.p95));
    let grid = &face.deviation;
    assert_eq!(
        grid.counts.len(),
        grid.columns as usize * grid.rows as usize
    );
    assert_eq!(
        grid.counts
            .iter()
            .map(|count| u64::from(*count))
            .sum::<u64>(),
        3_120
    );
    assert!(grid.first_along <= 0.0 && grid.first_along > -grid.step);
    assert!(grid.first_angle <= 0.0 && grid.first_angle > -360.0 / f64::from(grid.columns));
    // The points lie where the grid says: every position of the surface
    // within the arc and the length is near a scan point.
    for (angle, along) in [
        (1.0, 0.01),
        (0.5 * face.arc_deg, 1.3),
        (face.arc_deg - 1.0, 2.57),
    ] {
        let on = face.point(angle, along);
        let nearest = column
            .points
            .iter()
            .map(|point| apart(*point, on))
            .fold(f64::INFINITY, f64::min);
        assert!(nearest < 0.02, "{nearest} at {angle} {along}");
    }

    // The same points seen from a station on the axis: the inside of a
    // round shaft.
    let shaft = scanned(
        &column.clone().flipped().with_stations(&[[1.0, 1.0, 1.3]]),
        4_096,
    );
    let found = detect_surfaces(
        &[indexed(&shaft)],
        0,
        EVERYWHERE,
        &config,
        EVERY,
        &mut |_| Ok(()),
    )
    .unwrap();
    assert_eq!(found.cylinders.len(), 1);
    assert!(found.cylinders[0].seen_from_inside);
    // Without stations a cylinder is taken as a column.
    assert!(!detect(&column, &config).cylinders[0].seen_from_inside);

    // Too little of the round, or too short, is no cylinder.
    let strict = SurfaceDetectConfig {
        min_arc_deg: 200.0,
        ..config.clone()
    };
    assert!(detect(&column, &strict).cylinders.is_empty());
    let long = SurfaceDetectConfig {
        min_cylinder_length: 3.0,
        ..config.clone()
    };
    assert!(detect(&column, &long).cylinders.is_empty());
    let thin = SurfaceDetectConfig {
        max_radius: 0.1,
        ..config
    };
    assert!(detect(&column, &thin).cylinders.is_empty());
}

#[test]
fn the_deviation_colours_of_a_cylinder_follow_the_side_it_was_scanned_from() {
    let (_, column) = half_column([1.0, 1.0, 0.0], 21);
    let mut found = detect(&column, &SurfaceDetectConfig::default());
    assert_eq!(found.cylinders.len(), 1);
    assert!(!found.cylinders[0].seen_from_inside);
    // Every cell with points lies the full scale outside the cylinder.
    let scale = 0.02;
    let grid = &mut found.cylinders[0].deviation;
    for (mean, count) in grid.means.iter_mut().zip(&grid.counts) {
        *mean = if *count > 0 { scale as f32 } else { 0.0 };
    }
    let [behind, _, front] = deviation_legend(scale);
    let colors = |surfaces: &DetectedSurfaces| {
        let mesh = deviation_mesh(surfaces, scale, DEFAULT_DEVIATION_CELLS).unwrap();
        let colors = mesh.colors.unwrap();
        assert!(!colors.is_empty());
        colors
    };
    // A column is scanned from outside: points outside it lie in front of
    // its surface.
    assert!(colors(&found).iter().all(|color| *color == front.color));
    // A shaft is scanned from inside: the same points lie behind its
    // surface. The figures of the grid stay positive outside.
    found.cylinders[0].seen_from_inside = true;
    assert!(colors(&found).iter().all(|color| *color == behind.color));
    assert!(found.cylinders[0]
        .deviation
        .means
        .iter()
        .all(|mean| *mean >= 0.0));
}

fn room_with_column() -> Shape {
    noisy_room().merged(half_column([2.5, 1.8, 0.0], 22).1)
}

#[test]
fn a_column_in_a_room_is_a_cylinder_beside_the_six_faces() {
    let room = room_with_column();
    let found = detect(&room, &SurfaceDetectConfig::default());
    // The six faces as without the column, and no strip of a plane on it.
    assert_eq!(found.planes.len(), 6);
    assert_eq!(classes(&found), [1, 1, 4, 0]);
    for (normal, on, area, _) in ROOM_FACES {
        let face = face_through(&found, normal, on);
        assert!((face.area - area).abs() < 0.002 * area);
    }
    assert_eq!(found.edges.len(), 12);
    assert_eq!(found.cylinders.len(), 1);
    let column = &found.cylinders[0];
    assert_eq!(column.id, 7);
    assert!((column.radius - 0.15).abs() < 0.001, "{}", column.radius);
    assert!(off_axis([2.5, 1.8, 1.0], column.axis_start, column.axis_end) < 0.001);
    // From floor to ceiling: 2.602 m measured, the floor and ceiling points
    // at its foot and head lying within the window of its surface.
    assert!((column.length() - 2.6).abs() < 0.03, "{}", column.length());
    assert!((column.arc_deg - 172.4).abs() < 1.5, "{}", column.arc_deg);
    assert!((0.0028..0.0032).contains(&column.residuals.rms));
    assert!(found.assigned_points >= 154_100);

    // With the search for cylinders off, the room is found as before and
    // the points of the column belong to nothing.
    let without = detect(
        &room,
        &SurfaceDetectConfig {
            detect_cylinders: false,
            ..SurfaceDetectConfig::default()
        },
    );
    assert_room_faces(&without, 0.002, 0.001);
    assert!(
        without.assigned_points < 151_200,
        "{}",
        without.assigned_points
    );
}

#[test]
fn pipes_are_cylinders_also_side_by_side() {
    // 5 cm radius, 3 m long, scanned all round, 25 cm apart under a ceiling.
    let pipe = |y: f64, seed: u64| {
        cylinder(&CylinderSpec::pipe(
            [0.5, y, 2.3],
            [1.0, 0.0, 0.0],
            0.05,
            3.0,
        ))
        .with_noise(Noise::Gaussian(0.002), seed)
    };
    let found = detect(&pipe(1.0, 23), &SurfaceDetectConfig::default());
    assert!(found.planes.is_empty());
    assert_eq!(found.cylinders.len(), 1);
    let face = &found.cylinders[0];
    // 0.2 mm measured: the voxels are more than half the radius wide, and
    // the mean of the points of a voxel lies a little inside the round.
    assert!((face.radius - 0.05).abs() < 0.001, "{}", face.radius);
    assert_eq!(face.arc_deg, 360.0);
    assert!((face.length() - 2.99).abs() < 0.005, "{}", face.length());
    assert!(off_axis([1.0, 1.0, 2.3], face.axis_start, face.axis_end) < 0.001);
    assert!((0.0018..0.0022).contains(&face.residuals.rms));
    assert_eq!(face.residuals.points, 9_300);

    let found = detect(
        &pipe(1.0, 23).merged(pipe(1.25, 24)),
        &SurfaceDetectConfig::default(),
    );
    assert_eq!(found.cylinders.len(), 2);
    assert_eq!([found.cylinders[0].id, found.cylinders[1].id], [1, 2]);
    let mut places: Vec<f64> = found
        .cylinders
        .iter()
        .map(|face| {
            assert!((face.radius - 0.05).abs() < 0.001, "{}", face.radius);
            assert_eq!(face.residuals.points, 9_300);
            assert!(
                angle_deg(face.axis(), [1.0, 0.0, 0.0])
                    .min(angle_deg(face.axis(), [-1.0, 0.0, 0.0]))
                    < 0.1
            );
            0.5 * (face.axis_start[1] + face.axis_end[1])
        })
        .collect();
    places.sort_by(f64::total_cmp);
    assert!((places[0] - 1.0).abs() < 0.001 && (places[1] - 1.25).abs() < 0.001);
    // Smaller voxels take the last of the difference away.
    let fine = detect(
        &pipe(1.0, 23),
        &SurfaceDetectConfig {
            voxel_size: 0.01,
            ..SurfaceDetectConfig::default()
        },
    );
    assert!((fine.cylinders[0].radius - 0.05).abs() < 0.0002);
    assert!(fine.cylinders[0].residuals.mean.abs() < 0.0001);
}

#[test]
fn a_thin_pipe_needs_voxels_of_half_its_radius() {
    // 2 cm radius: found with voxels of 1 cm.
    let pipe = cylinder(&CylinderSpec::pipe(
        [0.5, 1.0, 2.3],
        [1.0, 0.0, 0.0],
        0.02,
        3.0,
    ))
    .with_noise(Noise::Gaussian(0.002), 33);
    let fine = SurfaceDetectConfig {
        voxel_size: 0.01,
        ..SurfaceDetectConfig::default()
    };
    let found = detect(&pipe, &fine);
    assert_eq!(found.cylinders.len(), 1);
    let face = &found.cylinders[0];
    // 0.1 mm measured.
    assert!((face.radius - 0.02).abs() < 0.0005, "{}", face.radius);
    assert!(off_axis([1.0, 1.0, 2.3], face.axis_start, face.axis_end) < 0.0005);
    assert!((face.length() - 2.99).abs() < 0.005);
    // With the voxels of 3 cm that are the default, the mean of the points
    // of a voxel lies inside the pipe and not on it: nothing is found.
    let coarse = detect(&pipe, &SurfaceDetectConfig::default());
    assert!(coarse.cylinders.is_empty() && coarse.planes.is_empty());
    assert_eq!(coarse.assigned_points, 0);
}

#[test]
fn of_more_faces_than_the_limit_the_largest_are_kept_and_too_many_cells_make_them_larger() {
    // Four sheets of different sizes, half a metre apart.
    let mut sheets = Shape::default();
    for (index, size) in [0.8, 1.4, 1.0, 1.2].into_iter().enumerate() {
        let mut sheet = plane_with_hole([size, size], 0.01, None);
        for point in &mut sheet.points {
            point[2] += 0.5 * index as f64;
        }
        sheets = sheets.merged(sheet);
    }
    let sheets = sheets.with_noise(Noise::Gaussian(0.003), 41);
    let records = records(&sheets);
    let config = SurfaceDetectConfig::default();
    let run = |limits: Limits| {
        detect_within(
            &[resident(&records)],
            0,
            EVERYWHERE,
            &config,
            EVERY,
            &mut |_| Ok(()),
            limits,
        )
        .unwrap()
    };
    let all = run(Limits {
        faces: MAX_FACES,
        chart_cells: MAX_CHART_CELLS,
    });
    assert_eq!(all.planes.len(), 4);
    assert_eq!(all.boundary_cell, 0.05);
    let areas: Vec<f64> = all.planes.iter().map(|face| face.area).collect();
    // Room for two faces: the two largest, numbered as before.
    let two = run(Limits {
        faces: 2,
        chart_cells: MAX_CHART_CELLS,
    });
    assert_eq!(two.planes.len(), 2);
    for (face, area) in two.planes.iter().zip(&areas) {
        assert!((face.area - area).abs() < 1e-9, "{} {area}", face.area);
    }
    assert!((two.planes[0].area - 1.39 * 1.39).abs() < 0.01);
    assert!((two.planes[1].area - 1.19 * 1.19).abs() < 0.01);
    // The points of the other two belong to no face.
    assert_eq!(two.assigned_points, 140 * 140 + 120 * 120);
    // The four grids hold 3,650 cells of 5 cm. With room for 2,000 the
    // cells are doubled, and the outlines stay where the points end.
    let coarse = run(Limits {
        faces: MAX_FACES,
        chart_cells: 2_000,
    });
    assert_eq!(coarse.boundary_cell, 0.1);
    assert_eq!(coarse.planes.len(), 4);
    for (face, area) in coarse.planes.iter().zip(&areas) {
        assert!(
            (face.area - area).abs() < 0.01 * area,
            "{} {area}",
            face.area
        );
        let step = face.deviation.step_u;
        assert!((dot(step, step).sqrt() - 0.1).abs() < 1e-12);
    }
}

#[test]
fn a_ball_and_a_narrow_arc_are_no_cylinders() {
    let config = SurfaceDetectConfig::default();
    let ball = sphere([1.0, 1.0, 1.0], 0.2, 20_000).with_noise(Noise::Gaussian(0.002), 25);
    let found = detect(&ball, &config);
    assert!(found.planes.is_empty() && found.cylinders.is_empty());
    assert_eq!(found.assigned_points, 0);
    // The band round a large ball lies within the tolerance of a cylinder
    // over more than the smallest length, 0.45 m at a radius of 0.5 m, but
    // a sphere fits it far better. A point per 1.5 cm.
    for (radius, count, seed) in [(0.5, 14_000, 27), (1.0, 56_000, 28)] {
        let ball = sphere([1.5, 1.5, 1.5], radius, count).with_noise(Noise::Gaussian(0.002), seed);
        let found = detect(&ball, &config);
        assert!(found.cylinders.is_empty(), "radius {radius}");
        assert_eq!(found.assigned_points, 0);
    }
    // An eighth of the round of a wide column.
    let spec = CylinderSpec {
        arc_degrees: 45.0,
        ..CylinderSpec::column([1.0, 1.0, 0.0], 0.4, 2.6)
    };
    let narrow = cylinder(&spec).with_noise(Noise::Gaussian(0.003), 26);
    let found = detect(&narrow, &config);
    assert!(found.planes.is_empty() && found.cylinders.is_empty());
    // Asked for, it is found, but so little of a round fixes its radius
    // poorly: 1 cm off measured.
    let found = detect(
        &narrow,
        &SurfaceDetectConfig {
            min_arc_deg: 30.0,
            ..config
        },
    );
    assert_eq!(found.cylinders.len(), 1);
    assert!(
        (found.cylinders[0].radius - 0.4).abs() < 0.02,
        "{}",
        found.cylinders[0].radius
    );
    assert!(
        (found.cylinders[0].arc_deg - 42.1).abs() < 2.5,
        "{}",
        found.cylinders[0].arc_deg
    );
}

#[test]
fn a_cylinder_exports_and_follows_its_layer() {
    let found = detect(&room_with_column(), &SurfaceDetectConfig::default());
    assert_eq!((found.planes.len(), found.cylinders.len()), (6, 1));
    let column = &found.cylinders[0];
    let directory = tempfile::tempdir().unwrap();

    // OBJ: a seventh group whose corners lie on the cylinder.
    let obj = directory.path().join("faces.obj");
    write_faces_obj(&found, &obj, &[]).unwrap();
    let text = std::fs::read_to_string(&obj).unwrap();
    let groups: Vec<&str> = text.lines().filter(|line| line.starts_with("g ")).collect();
    assert_eq!(groups.len(), 7);
    assert_eq!(groups[6], "g face_0007_cylinder");
    let mesh = crate::read_obj_mesh(&obj).unwrap();
    // 24 corners of the six faces, then two per step of the arc: 24 strips
    // of at most 7.5 degrees over 173 degrees.
    assert_eq!(mesh.vertices.len(), 24 + 2 * 25);
    assert_eq!(mesh.triangles.len(), 12 + 2 * 24);
    let normals = mesh.normals.as_ref().unwrap();
    for (vertex, normal) in mesh.vertices[24..].iter().zip(&normals[24..]) {
        let radius = off_axis(*vertex, column.axis_start, column.axis_end);
        assert!((radius - column.radius).abs() < 1e-9);
        // The normal points away from the axis.
        let outward = difference(*vertex, [2.5, 1.8, vertex[2]]);
        assert!(dot(outward, normal.map(f64::from)) > 0.14);
    }
    for triangle in &mesh.triangles[12..] {
        let [a, b, c] = triangle.map(|index| mesh.vertices[index as usize]);
        let winding = cross(difference(b, a), difference(c, a));
        assert!(dot(winding, normals[triangle[0] as usize].map(f64::from)) > 0.0);
    }

    // JSON: a seventh entry of its own kind.
    let document = faces_json(&found, "room.e57");
    let faces = document["faces"].as_array().unwrap();
    assert_eq!(faces.len(), 7);
    let entry = &faces[6];
    assert_eq!(entry["id"], 7);
    assert_eq!(entry["type"], "cylinder");
    assert_eq!(entry["radius"], column.radius);
    assert_eq!(entry["diameter"], 2.0 * column.radius);
    assert_eq!(entry["length"], column.length());
    assert_eq!(entry["arc_degrees"], column.arc_deg);
    assert_eq!(entry["seen_from_inside"], false);
    assert_eq!(entry["axis_start"], serde_json::json!(column.axis_start));
    assert_eq!(entry["residual"]["points"], column.residuals.points);
    assert_eq!(document["settings"]["cylinders"], true);

    // The viewer meshes hold it too.
    let flat = flat_mesh(&found).unwrap();
    assert_eq!(flat.triangles.len(), 12 + 2 * 24);
    assert_eq!(flat.colors.as_ref().unwrap()[24], cylinder_color(column));
    let heat = deviation_mesh(&found, 0.02, DEFAULT_DEVIATION_CELLS).unwrap();
    let on_column = heat
        .vertices
        .iter()
        .filter(|vertex| {
            (off_axis(**vertex, column.axis_start, column.axis_end) - column.radius).abs() < 1e-9
                && found
                    .planes
                    .iter()
                    .all(|face| face.signed_distance(**vertex).abs() > 1e-6)
        })
        .count();
    // About one corner per cell of 3.9 cm on 1 m2.
    assert!((600..900).contains(&on_column), "{on_column}");
    for vertex in &heat.vertices {
        let on_plane = found
            .planes
            .iter()
            .any(|face| face.signed_distance(*vertex).abs() < 1e-6);
        let on_round =
            (off_axis(*vertex, column.axis_start, column.axis_end) - column.radius).abs() < 1e-9;
        assert!(on_plane || on_round, "{vertex:?}");
    }

    // Twice as large and mirrored: still a cylinder, twice as wide.
    let turned = found
        .placed(SourceTransform {
            scale: [-2.0, 2.0, 2.0],
            offset: [1.0, 0.0, 0.0],
        })
        .unwrap();
    assert_eq!(turned.cylinders.len(), 1);
    let moved = &turned.cylinders[0];
    assert!((moved.radius - 2.0 * column.radius).abs() < 1e-12);
    assert!((moved.length() - 2.0 * column.length()).abs() < 1e-9);
    assert!((moved.residuals.rms - 2.0 * column.residuals.rms).abs() < 1e-12);
    assert_eq!(moved.arc_deg, column.arc_deg);
    for (angle, along) in [(0.0, 0.0), (40.0, 1.0), (column.arc_deg, column.length())] {
        let before = column.point(angle, along);
        let expected = [1.0 - 2.0 * before[0], 2.0 * before[1], 2.0 * before[2]];
        assert!(apart(moved.point(angle, 2.0 * along), expected) < 1e-9);
    }
    assert!((moved.deviation.step - 2.0 * column.deviation.step).abs() < 1e-12);
    // Its mesh still faces outward.
    let mesh = flat_mesh(&turned).unwrap();
    let transform = turned.placement;
    let (_, positions) = shown(&mesh, transform);
    for triangle in &mesh.triangles[12..] {
        let [a, b, c] = triangle.map(|index| positions[index as usize]);
        let winding = cross(difference(b, a), difference(c, a));
        let outward = difference(a, [1.0 - 2.0 * 2.5, 2.0 * 1.8, a[2]]);
        assert!(dot(winding, outward) > 0.0);
    }
    // Scaled unequally it would be no cylinder any more, and is left out.
    let stretched = found
        .placed(SourceTransform {
            scale: [1.0, 1.0, 2.0],
            offset: [0.0; 3],
        })
        .unwrap();
    assert_eq!((stretched.planes.len(), stretched.cylinders.len()), (6, 0));
}

#[test]
fn a_wide_column_needs_a_larger_smallest_face_width() {
    // Growing a plane on a round surface gives a strip as wide as the angle
    // tolerance allows: 2 r sin(10 degrees), which is 21 cm at a radius of
    // 0.6 m and so passes for a face of at least 15 cm.
    let wide = cylinder(&CylinderSpec::column([2.0, 2.0, 0.0], 0.6, 2.6))
        .with_noise(Noise::Gaussian(0.003), 31);
    let found = detect(&wide, &SurfaceDetectConfig::default());
    assert!(found.cylinders.is_empty());
    assert!(found.planes.len() > 10, "{}", found.planes.len());
    assert!(found
        .planes
        .iter()
        .all(|face| face.class == FaceClass::Wall));
    // With faces of at least 40 cm asked for, the strips are no faces and
    // the column is found.
    let found = detect(
        &wide,
        &SurfaceDetectConfig {
            min_plane_width: 0.4,
            ..SurfaceDetectConfig::default()
        },
    );
    assert!(found.planes.is_empty());
    assert_eq!(found.cylinders.len(), 1);
    let column = &found.cylinders[0];
    assert!((column.radius - 0.6).abs() < 0.001, "{}", column.radius);
    assert_eq!(column.arc_deg, 360.0);
    assert!(off_axis([2.0, 2.0, 1.0], column.axis_start, column.axis_end) < 0.001);
    // Up to a radius of 0.35 m the default settings do.
    for radius in [0.25, 0.35] {
        let narrow = cylinder(&CylinderSpec::column([2.0, 2.0, 0.0], radius, 2.6))
            .with_noise(Noise::Gaussian(0.003), 31);
        let found = detect(&narrow, &SurfaceDetectConfig::default());
        assert!(found.planes.is_empty(), "{radius}");
        assert_eq!(found.cylinders.len(), 1);
        assert!((found.cylinders[0].radius - radius).abs() < 0.001);
    }
}

/// What holds for every result: each outer ring runs counter-clockwise
/// without a side that turns back over the one before it or crosses
/// another, each hole lies inside its outer ring, and the triangles of the
/// flat mesh cover per face what its outline bounds and no more.
fn assert_sound_outlines(found: &DetectedSurfaces) {
    let turn = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
    let minus = |a: [f64; 2], b: [f64; 2]| [a[0] - b[0], a[1] - b[1]];
    for face in &found.planes {
        for patch in &face.patches {
            let ring = &patch.outer;
            let n = ring.len();
            assert!(n >= 3 && ring_signed_area(ring) > 0.0, "face {}", face.id);
            for index in 0..n {
                let (a, b, c) = (ring[index], ring[(index + 1) % n], ring[(index + 2) % n]);
                let (arrives, leaves) = (minus(b, a), minus(c, b));
                let ahead = arrives[0] * leaves[0] + arrives[1] * leaves[1];
                assert!(
                    turn(arrives, leaves).abs() > 1e-9 || ahead > 0.0,
                    "face {} turns back at {b:?}: {ring:?}",
                    face.id
                );
                for other in index + 2..n {
                    if index == 0 && other == n - 1 {
                        continue;
                    }
                    let (c, d) = (ring[other], ring[(other + 1) % n]);
                    let crosses = turn(minus(b, a), minus(c, a)) * turn(minus(b, a), minus(d, a))
                        < 0.0
                        && turn(minus(d, c), minus(a, c)) * turn(minus(d, c), minus(b, c)) < 0.0;
                    assert!(!crosses, "face {} crosses itself: {ring:?}", face.id);
                }
            }
            for hole in &patch.holes {
                assert!(ring_signed_area(hole) < 0.0);
                assert!(hole
                    .iter()
                    .all(|corner| crate::grid2d::ring_contains(ring, *corner)));
            }
        }
    }
    let mesh = flat_mesh(found).unwrap();
    let mut first = 0;
    for face in &found.planes {
        let corners: usize = face
            .patches
            .iter()
            .map(|patch| patch.outer.len() + patch.holes.iter().map(Vec::len).sum::<usize>())
            .sum();
        let own = first..first + corners;
        let area: f64 = mesh
            .triangles
            .iter()
            .filter(|triangle| own.contains(&(triangle[0] as usize)))
            .map(|triangle| {
                let [a, b, c] = triangle.map(|index| mesh.vertices[index as usize]);
                let normal = cross(difference(b, a), difference(c, a));
                0.5 * dot(normal, normal).sqrt()
            })
            .sum();
        assert!(
            (area - face.area).abs() < 1e-9 * (1.0 + face.area),
            "face {}: triangles {area}, outline {}",
            face.id,
            face.area
        );
        first += corners;
    }
}

/// The edges between two faces, as pairs of ends.
fn edges_between<'a>(
    found: &'a DetectedSurfaces,
    a: &PlaneFace,
    b: &PlaneFace,
) -> Vec<&'a SurfaceEdge> {
    let faces = [a.id.min(b.id), a.id.max(b.id)];
    found
        .edges
        .iter()
        .filter(|edge| edge.faces == faces)
        .collect()
}

#[test]
fn a_thin_scan_gets_voxels_as_wide_as_its_points_lie_apart() {
    // A point per 5 cm and per 8 cm: in voxels of 3 cm most voxels along a
    // face are empty and no region holds together.
    for (spacing, voxel, doublings) in [(0.05, 0.06, 1), (0.08, 0.12, 2)] {
        let room = box_room(&RoomSpec {
            spacing,
            ..RoomSpec::default()
        })
        .with_noise(Noise::Gaussian(0.002), 51);
        let found = detect(&room, &SurfaceDetectConfig::default());
        assert_eq!(found.voxel_size, voxel, "spacing {spacing}");
        assert_eq!(found.density_doublings, doublings);
        assert!(found.is_coarse());
        // The room is found as from a dense scan: areas within 0.02 %
        // (measured), every point on a face.
        assert_room_faces(&found, 0.002, 0.001);
        assert_eq!(found.edges.len(), 12);
        assert_eq!(found.assigned_points, found.source_points);
        assert_sound_outlines(&found);
    }
    // A floor as it looks far from the station: a point per centimetre
    // along a scan line, the lines 6 cm apart.
    let mut floor = Shape::default();
    for line in 0..50 {
        floor = floor.merged(
            plane_with_hole([4.0, 0.01], 0.01, None)
                .transformed(0.0, [0.0, 0.06 * f64::from(line), 0.37]),
        );
    }
    let found = detect(
        &floor.with_noise(Noise::Gaussian(0.002), 52),
        &SurfaceDetectConfig::default(),
    );
    assert_eq!(found.planes.len(), 1);
    assert_eq!((found.voxel_size, found.density_doublings), (0.06, 1));
    // From the first point to the last along the lines, 3.99 m, and from
    // the first line to the last, 2.94 m: 11.73 m2 measured.
    assert!(
        (found.planes[0].area - 3.99 * 2.94).abs() < 0.02,
        "{}",
        found.planes[0].area
    );
    // A scan that is dense enough keeps the size asked for, and so do
    // points that fill space and lie on no surface, however thin.
    let dense = detect(&noisy_room(), &SurfaceDetectConfig::default());
    assert_eq!((dense.voxel_size, dense.density_doublings), (0.03, 0));
    let space = Bounds {
        min: [0.0; 3],
        max: [4.0, 3.0, 2.6],
    };
    for count in [2_000, 20_000, 200_000] {
        let found = detect(
            &stray_points(space, count, 53),
            &SurfaceDetectConfig::default(),
        );
        assert_eq!(found.voxel_size, 0.03, "{count}");
        assert!(found.planes.is_empty());
    }
    let small = Bounds {
        min: [0.0; 3],
        max: [1.4, 1.05, 0.9],
    };
    let found = detect(
        &stray_points(small, 1_900, 54),
        &SurfaceDetectConfig::default(),
    );
    assert_eq!(found.voxel_size, 0.03);
    assert!(found.planes.is_empty());
}

#[test]
fn faces_that_meet_at_a_small_angle_stay_two_faces() {
    // A level floor of 4 by 3 m and a ramp of 4 m that rises 3 degrees from
    // its edge. Where they meet they touch as closely as two parts of one
    // plane; one plane through both would lie 3 cm off them.
    let (sin, cos) = 3f64.to_radians().sin_cos();
    let floor = plane_with_hole([4.0, 3.0], 0.01, None);
    let ramp = rectangle(
        [4.0, 0.0, 0.0],
        [cos, 0.0, sin],
        [0.0, 1.0, 0.0],
        [4.0, 3.0],
        0.01,
    );
    let hinge = floor.merged(ramp).with_noise(Noise::Gaussian(0.002), 55);
    let found = detect(&hinge, &SurfaceDetectConfig::default());
    assert!(detect(&hinge, &SurfaceDetectConfig::default()) == found);
    assert_eq!(found.planes.len(), 2);
    // Each is as flat as its points, 2.0 mm measured, where one face over
    // both had 30 mm. Growing takes the first of the two 0.6 m on past the
    // line they share; the points there are given back, so the faces are
    // divided where they meet. Near that line a point belongs to either
    // plane within its noise, so both outlines hold a strip of 0.14 m.
    for face in &found.planes {
        assert!(
            (0.0018..0.0023).contains(&face.residuals.rms),
            "{}",
            face.residuals.rms
        );
        assert!(face.residuals.max < 0.012, "{}", face.residuals.max);
        assert!((face.area - 12.15).abs() < 0.25, "{}", face.area);
    }
    let floor = face_through(&found, [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]);
    let ramp = face_through(&found, [-sin, 0.0, cos], [6.0, 1.0, 2.0 * sin / cos]);
    assert_eq!(floor.class, FaceClass::Floor);
    let reach = |face: &PlaneFace| {
        let xs: Vec<f64> = corners_3d(face).iter().map(|corner| corner[0]).collect();
        (
            xs.iter().copied().fold(f64::INFINITY, f64::min),
            xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        )
    };
    // 4.02 and 3.85 measured for the line at 4.
    assert!((reach(floor).1 - 4.0).abs() < 0.06, "{:?}", reach(floor));
    assert!((reach(ramp).0 - 3.9).abs() < 0.1, "{:?}", reach(ramp));
    let total: f64 = found.planes.iter().map(|face| face.area).sum();
    assert!((total - 24.3).abs() < 0.2, "{total}");
    assert_eq!(found.assigned_points, found.source_points);
    assert_sound_outlines(&found);

    // A wall that curves with a radius of 10 m, 8 m of it: no face has its
    // points further from it than the tolerance in the mean.
    let spec = CylinderSpec {
        arc_degrees: (8.0f64 / 10.0).to_degrees(),
        ..CylinderSpec::column([0.0, 0.0, 0.0], 10.0, 2.6)
    };
    let wall = cylinder(&spec).with_noise(Noise::Gaussian(0.002), 56);
    let found = detect(&wall, &SurfaceDetectConfig::default());
    assert!(found.cylinders.is_empty());
    // Six faces of 1.3 m; the bend lies 2 cm off a plane over that width.
    // RMS 5.3 to 8.6 mm measured, where a face over more of the wall had
    // 32 mm.
    assert!(
        (5..=8).contains(&found.planes.len()),
        "{}",
        found.planes.len()
    );
    for face in &found.planes {
        assert!(face.residuals.rms < 0.012, "{}", face.residuals.rms);
    }
    assert_eq!(found.assigned_points, found.source_points);
}

/// A floor of 4 by 3 m and one wall face of 2 m that stands on it at x = 2,
/// from `start` along y, with the floor running on at both ends and on both
/// sides. The points lie at random, `spacing` apart in the mean.
fn wall_on_a_floor(start: f64, spacing: f64, seed: u64) -> Shape {
    let floor = plane_with_hole([4.0, 3.0], spacing, None);
    let wall = rectangle(
        [2.0, start, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [2.0, 2.6],
        spacing,
    );
    floor
        .merged(wall)
        .scattered(0.5 * spacing, seed)
        .with_noise(Noise::Gaussian(0.002), seed + 1)
}

#[test]
fn a_wall_on_a_floor_that_runs_on_past_it_has_a_hollow_edge_of_its_own_length() {
    // Wherever the wall stands on the grid of the floor, the floor lies on
    // the side its normal points to, so the corner is hollow.
    for start in [0.50, 0.51, 0.52] {
        let found = detect(
            &wall_on_a_floor(start, 0.02, 57),
            &SurfaceDetectConfig::default(),
        );
        assert_eq!(found.planes.len(), 2);
        assert_eq!(found.edges.len(), 1);
        assert!(
            (found.edges[0].angle_deg - 90.0).abs() < 0.1,
            "from {start}: {}",
            found.edges[0].angle_deg
        );
        assert_sound_outlines(&found);
    }
    // A point per 5 mm: floor points lie within their own noise of the
    // plane of the wall, also beyond its ends. They are no reason to take
    // the wall as longer than it is, wherever its ends fall in the cells
    // of its grid.
    for start in [0.50, 0.53, 0.56, 0.62] {
        let found = detect(
            &wall_on_a_floor(start, 0.005, 58),
            &SurfaceDetectConfig::default(),
        );
        assert_eq!((found.planes.len(), found.edges.len()), (2, 1));
        let edge = &found.edges[0];
        let (low, high) = (
            edge.start[1].min(edge.end[1]),
            edge.start[1].max(edge.end[1]),
        );
        assert!((low - start).abs() < 0.008, "{start}: {low}");
        assert!((high - start - 2.0).abs() < 0.008, "{start}: {high}");
        let wall = &found.planes[1];
        assert_eq!(wall.class, FaceClass::Wall);
        assert_eq!(
            wall.patches[0].outer.len(),
            4,
            "{:?}",
            wall.patches[0].outer
        );
        assert!((wall.area - 2.0 * 2.6).abs() < 0.02, "{}", wall.area);
        assert_sound_outlines(&found);
    }
}

#[test]
fn a_strip_of_wall_between_a_door_and_the_corner_of_the_room_stays() {
    // The jamb of a door of 0.9 m stands 10 cm from the west wall, well
    // within the 12 cm from which corners are moved onto an edge.
    let room = box_room(&RoomSpec {
        openings: vec![Opening::door(Wall::South, 0.10)],
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 59);
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(classes(&found), [1, 1, 4, 0]);
    let wall = face_through(&found, [0.0, 1.0, 0.0], [0.0; 3]);
    assert_eq!(wall.patches.len(), 1);
    let outline: Vec<[f64; 3]> = wall.patches[0]
        .outer
        .iter()
        .map(|corner| wall.point(*corner))
        .collect();
    assert_eq!(outline.len(), 8, "{outline:?}");
    // The strip: from the corner of the room to the last points before the
    // door, at 9 cm. And the door: to the first points after it, at 1.01.
    let mut jambs: Vec<f64> = outline
        .iter()
        .filter(|corner| corner[2] > 1.0 && corner[2] < 2.2)
        .map(|corner| corner[0])
        .collect();
    jambs.sort_by(f64::total_cmp);
    assert_eq!(jambs.len(), 2, "{outline:?}");
    assert!(
        (jambs[0] - 0.09).abs() < 0.005 && (jambs[1] - 1.01).abs() < 0.005,
        "{jambs:?}"
    );
    // The strip stands on the floor like the rest of the wall, although
    // the edge it shares with the floor there is shorter than an edge.
    assert!(outline
        .iter()
        .filter(|corner| corner[0] < 0.2 && corner[2] < 1.0)
        .all(|corner| corner[2].abs() < 0.001));
    let expected = 10.4 - 0.92 * 2.11;
    assert!(
        (wall.area - expected).abs() < 0.01,
        "{} {expected}",
        wall.area
    );
    assert_sound_outlines(&found);
}

#[test]
fn a_strip_narrower_than_the_reach_beside_an_opening_leaves_no_fold_in_the_outline() {
    // A door 12 cm from the corner of the room and an opening that ends
    // 8 cm below the ceiling. Corners move onto an edge from 12 cm away, so
    // both sides of the strip between such an opening and the next face
    // could.
    let openings = vec![
        Opening::door(Wall::South, 0.12),
        Opening {
            wall: Wall::South,
            start: 2.0,
            width: 0.9,
            sill: 1.82,
            height: 0.7,
        },
    ];
    let room = box_room(&RoomSpec {
        openings: openings.clone(),
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 11);
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(classes(&found), [1, 1, 4, 0]);
    assert_sound_outlines(&found);
    // None of the south wall lies in an opening.
    let wall = face_through(&found, [0.0, 1.0, 0.0], [0.0; 3]);
    for opening in &openings {
        for step in 0..100 {
            let (i, j) = (f64::from(step % 10), f64::from(step / 10));
            let x = opening.start + (i + 0.5) / 10.0 * opening.width;
            let z = opening.sill + (j + 0.5) / 10.0 * opening.height;
            let from = difference([x, 0.0, z], wall.origin);
            let uv = [dot(from, wall.u), dot(from, wall.v)];
            assert!(
                !wall.patches.iter().any(|patch| patch.contains(uv)),
                "the outline holds {x} {z}"
            );
        }
    }
}

#[test]
fn the_reveals_of_an_opening_are_no_points_of_its_wall() {
    // Walls of 10 cm with a door and a window: the reveals are surfaces of
    // their own, too narrow for a face, that stand square to their wall.
    let room = box_room(&RoomSpec {
        wall_thickness: Some(0.1),
        openings: vec![
            Opening::door(Wall::South, 1.0),
            Opening::window(Wall::East, 0.8),
        ],
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.002), 60);
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 10);
    // Noise of 2 mm. The faces with an opening: RMS 2.25 to 2.32 mm and
    // nothing further off than 10 mm (measured), from the row of reveal
    // points that lies within the tolerance of the face. With all reveal
    // points within three tolerances counted these faces had 5.5 to 7 mm
    // and a largest deviation of 5 cm.
    for face in &found.planes {
        assert!(
            (0.0019..0.0026).contains(&face.residuals.rms),
            "{} {}",
            face.id,
            face.residuals.rms
        );
        assert!(face.residuals.max < 0.012, "{}", face.residuals.max);
        assert!(face.residuals.mean.abs() < 0.0003);
    }
    assert_sound_outlines(&found);
}

/// A room in the shape of an L: 6 by 6 m less a corner of 3.5 by 3.5 m.
fn l_shaped_room() -> Shape {
    let (x, y, z) = ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]);
    let back = |v: [f64; 3]| v.map(|value: f64| -value);
    let spacing = 0.02;
    let mut room = Shape::default();
    for height in [0.0, 2.6] {
        // Seen from above for the floor, from below for the ceiling.
        for (origin, size) in [([0.0, 0.0], [6.0, 2.5]), ([0.0, 2.5], [2.5, 3.5])] {
            let part = rectangle([origin[0], origin[1], height], x, y, size, spacing);
            room = room.merged(if height > 0.0 { part.flipped() } else { part });
        }
    }
    // Each wall from a corner, along a direction, seen from inside.
    for (origin, along, length) in [
        ([0.0, 0.0], x, 6.0),
        ([6.0, 0.0], y, 2.5),
        ([6.0, 2.5], back(x), 3.5),
        ([2.5, 2.5], y, 3.5),
        ([2.5, 6.0], back(x), 2.5),
        ([0.0, 6.0], back(y), 6.0),
    ] {
        // Along the wall with the room to the left: the normal of along
        // cross up points to the right, so the face is flipped.
        room = room.merged(
            rectangle(
                [origin[0], origin[1], 0.0],
                along,
                z,
                [length, 2.6],
                spacing,
            )
            .flipped(),
        );
    }
    room
}

#[test]
fn without_stations_every_face_looks_to_the_side_that_lies_open() {
    // The middle of the box round an L-shaped room lies outside it, behind
    // the two inner walls.
    let room = l_shaped_room().with_noise(Noise::Gaussian(0.002), 61);
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 8);
    assert_eq!(classes(&found), [1, 1, 6, 0]);
    assert!(found
        .planes
        .iter()
        .all(|face| face.normal_source == NormalSource::OpenSide));
    face_through(&found, [-1.0, 0.0, 0.0], [2.5, 4.0, 1.0]);
    face_through(&found, [0.0, -1.0, 0.0], [4.0, 2.5, 1.0]);
    face_through(&found, [0.0, 0.0, 1.0], [1.0, 1.0, 0.0]);
    face_through(&found, [0.0, 0.0, -1.0], [1.0, 1.0, 2.6]);
    // Every corner of the room is hollow but the one between the two inner
    // walls.
    assert_eq!(found.edges.len(), 18);
    let outward: Vec<&SurfaceEdge> = found
        .edges
        .iter()
        .filter(|edge| (edge.angle_deg - 270.0).abs() < 0.1)
        .collect();
    assert_eq!(outward.len(), 1);
    assert!(
        apart(
            [outward[0].start[0], outward[0].start[1], 0.0],
            [2.5, 2.5, 0.0]
        ) < 0.01
    );
    assert_eq!(
        found
            .edges
            .iter()
            .filter(|edge| (edge.angle_deg - 90.0).abs() < 0.1)
            .count(),
        17
    );
    assert_sound_outlines(&found);

    // Two rooms on either side of a partition of 20 cm: the middle of the
    // box lies in the partition, behind both of its faces.
    let rooms = noisy_room().merged(
        box_room(&RoomSpec::default())
            .with_noise(Noise::Gaussian(0.003), 62)
            .transformed(0.0, [4.2, 0.0, 0.0]),
    );
    let found = detect(&rooms, &SurfaceDetectConfig::default());
    assert_eq!(found.planes.len(), 12);
    face_through(&found, [-1.0, 0.0, 0.0], [4.0, 1.0, 1.0]);
    face_through(&found, [1.0, 0.0, 0.0], [4.2, 1.0, 1.0]);
    assert_eq!(found.edges.len(), 24);
    assert!(found
        .edges
        .iter()
        .all(|edge| (edge.angle_deg - 90.0).abs() < 0.1));
}

#[test]
fn a_floor_in_a_turned_building_is_laid_out_along_its_walls() {
    // A room with a door of 1.6 m in its south wall, through which the
    // floor runs on for half a metre, turned from x and y.
    for degrees in [0.0, 17.0, 30.0, 45.0] {
        let room = box_room(&RoomSpec {
            openings: vec![Opening {
                wall: Wall::South,
                start: 1.0,
                width: 1.6,
                sill: 0.0,
                height: 2.1,
            }],
            ..RoomSpec::default()
        })
        .merged(rectangle(
            [1.0, -0.5, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [1.6, 0.5],
            0.02,
        ))
        .with_noise(Noise::Gaussian(0.003), 63)
        .transformed(degrees, [100.0, 200.0, 0.0]);
        let found = detect(&room, &SurfaceDetectConfig::default());
        assert_eq!(classes(&found), [1, 1, 4, 0], "{degrees}");
        let (sin, cos) = degrees.to_radians().sin_cos();
        let place = |point: [f64; 3]| -> [f64; 3] {
            [
                cos * point[0] - sin * point[1] + 100.0,
                sin * point[0] + cos * point[1] + 200.0,
                point[2],
            ]
        };
        let floor = face_through(&found, [0.0, 0.0, 1.0], place([1.0, 1.0, 0.0]));
        // The room and the strip through the door: eight corners.
        assert_eq!(floor.patches.len(), 1);
        assert_eq!(
            floor.patches[0].outer.len(),
            8,
            "{degrees}: {:?}",
            floor.patches[0].outer
        );
        assert!(
            (floor.area - 12.78).abs() < 0.015,
            "{degrees}: {}",
            floor.area
        );
        // Along the south wall, whichever way that runs, and still so
        // after its layer was moved.
        let along = difference(place([1.0, 0.0, 0.0]), place([0.0; 3]));
        assert!(apart(floor.u, along) < 1e-3, "{degrees}: {:?}", floor.u);
        let moved = found
            .placed(SourceTransform {
                scale: [1.0; 3],
                offset: [5.0, 0.0, 0.0],
            })
            .unwrap();
        let again = moved.face(floor.id).unwrap();
        assert!(apart(again.u, floor.u) < 1e-9);
        assert_eq!(again.patches[0].outer.len(), 8);
        // The edge of the floor with the south wall beside the door begins
        // where the wall does: at its last points before the door.
        let south = difference(place([0.0, 1.0, 0.0]), place([0.0; 3]));
        let wall = face_through(&found, south, place([0.0; 3]));
        let mut ends: Vec<f64> = edges_between(&found, floor, wall)
            .iter()
            .flat_map(|edge| [edge.start, edge.end])
            .map(|end| apart(end, place([0.0; 3])))
            .collect();
        ends.sort_by(f64::total_cmp);
        assert_eq!(ends.len(), 4, "{degrees}");
        for (end, expected) in ends.iter().zip([0.0, 0.99, 2.61, 4.0]) {
            assert!((end - expected).abs() < 0.005, "{degrees}: {ends:?}");
        }
        assert_sound_outlines(&found);
    }
}

#[test]
fn larger_voxels_make_no_faces_of_the_corners_between_faces() {
    // Three rooms in a row with partitions of 10 cm, and a budget that
    // makes the voxels 6 cm. The means of the voxels along a corner lie on
    // the diagonal between its two faces.
    let mut rooms = Shape::default();
    for index in 0..3 {
        rooms = rooms.merged(
            box_room(&RoomSpec {
                spacing: 0.01,
                ..RoomSpec::default()
            })
            .scattered(0.005, 64 + index)
            .with_noise(Noise::Gaussian(0.002), 70 + index)
            .transformed(0.0, [4.1 * index as f64, 0.0, 0.0]),
        );
    }
    let config = SurfaceDetectConfig {
        max_working_points: 100_000,
        ..SurfaceDetectConfig::default()
    };
    let found = detect(&rooms, &config);
    assert_eq!(found.voxel_size, 0.06);
    assert!(found.is_coarse());
    assert_eq!(classes(&found)[3], 0);
    assert!(found
        .planes
        .iter()
        .all(|face| face.area >= config.min_region_area && face.class != FaceClass::Sloped));
    let coarse = detect(
        &rooms,
        &SurfaceDetectConfig {
            max_working_points: 25_000,
            ..SurfaceDetectConfig::default()
        },
    );
    assert_eq!(coarse.voxel_size, 0.12);
    assert!(coarse
        .planes
        .iter()
        .all(|face| face.area >= config.min_region_area && face.class != FaceClass::Sloped));
}

#[test]
fn noise_near_the_tolerance_gives_every_face_once() {
    // 15 mm of noise against a tolerance of 20: the points of one face
    // spread over two or three layers of voxels, which grow into regions
    // of their own that lie on top of each other.
    for seed in [61, 62] {
        let room = box_room(&RoomSpec {
            spacing: 0.01,
            openings: vec![Opening::door(Wall::South, 1.0)],
            ..RoomSpec::default()
        })
        .with_noise(Noise::Gaussian(0.015), seed);
        let found = detect(&room, &SurfaceDetectConfig::default());
        assert_eq!(found.planes.len(), 6, "seed {seed}");
        assert_eq!(classes(&found), [1, 1, 4, 0]);
        for face in &found.planes {
            assert!(
                (0.013..0.017).contains(&face.residuals.rms),
                "{}",
                face.residuals.rms
            );
        }
    }
}

#[test]
fn the_class_of_a_face_is_the_direction_of_its_normal_only() {
    // A table top of 1.0 by 0.8 m at 0.75 m and the front of a cabinet of
    // 1.2 by 2.0 m, 0.6 m before the north wall: by their normals a floor
    // and a wall. That is the stated limit of the classes.
    let room = noisy_room()
        .merged(
            rectangle(
                [1.0, 0.8, 0.75],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.8],
                0.02,
            )
            .with_noise(Noise::Gaussian(0.003), 65),
        )
        .merged(
            rectangle(
                [3.4, 2.4, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [1.2, 2.0],
                0.02,
            )
            .with_noise(Noise::Gaussian(0.003), 66),
        );
    let found = detect(&room, &SurfaceDetectConfig::default());
    assert_eq!(classes(&found), [2, 1, 5, 0]);
    let top = face_through(&found, [0.0, 0.0, 1.0], [1.5, 1.2, 0.75]);
    assert_eq!(top.class, FaceClass::Floor);
    // Its points end a centimetre inside its edges.
    assert!((top.area - 0.98 * 0.78).abs() < 0.01, "{}", top.area);
    let front = face_through(&found, [0.0, -1.0, 0.0], [2.8, 2.4, 1.0]);
    assert_eq!(front.class, FaceClass::Wall);
    // What tells them from the room: the height and the area.
    assert!((top.origin[2] - 0.75).abs() < 0.001);
    assert_sound_outlines(&found);
}

#[test]
fn larger_voxels_move_the_corners_of_an_outline_no_further() {
    // The head of a door, 0.49 m under the ceiling, and a west wall that
    // ends 0.3 m before the south wall, with voxels and cells of 24 cm:
    // both within two cells of the line of the next face, and no reason to
    // move them there.
    let room = box_room(&RoomSpec {
        openings: vec![
            Opening::door(Wall::South, 1.0),
            Opening {
                wall: Wall::West,
                start: 0.0,
                width: 0.3,
                sill: 0.0,
                height: 2.6,
            },
        ],
        ..RoomSpec::default()
    })
    .with_noise(Noise::Gaussian(0.003), 67);
    for budget in [1_500_000, 6_000] {
        let found = detect(
            &room,
            &SurfaceDetectConfig {
                max_working_points: budget,
                ..SurfaceDetectConfig::default()
            },
        );
        assert_eq!(found.planes.len(), 6);
        let wall = face_through(&found, [0.0, 1.0, 0.0], [0.0; 3]);
        let head: Vec<f64> = corners_3d(wall)
            .into_iter()
            .filter(|corner| corner[0] > 0.5 && corner[0] < 2.5 && corner[2] > 1.0)
            .map(|corner| corner[2])
            .collect();
        assert_eq!(head.len(), 2, "{budget}");
        assert!(head.iter().all(|z| (z - 2.11).abs() < 0.005), "{head:?}");
        // From its first points at x = 0.01, or from the line of the west
        // wall, which with the large cells counts as reaching it: 8.43 and
        // 8.46 m2.
        assert!((wall.area - 8.447).abs() < 0.03, "{}", wall.area);
        // The west wall begins at its first points, at 0.31.
        let west = face_through(&found, [1.0, 0.0, 0.0], [0.0, 1.5, 1.0]);
        let begins = corners_3d(west)
            .iter()
            .map(|corner| corner[1])
            .fold(f64::INFINITY, f64::min);
        assert!((begins - 0.31).abs() < 0.005, "{budget}: {begins}");
        assert!((west.area - 2.69 * 2.6).abs() < 0.02, "{}", west.area);
        assert_sound_outlines(&found);
        if budget == 6_000 {
            assert_eq!((found.voxel_size, found.boundary_cell), (0.24, 0.24));
            let settings = &faces_json(&found, "room.e57")["settings"];
            assert_eq!(settings["coarse"], true);
            assert_eq!(settings["voxel_size"], 0.24);
            assert_eq!(settings["voxel_size_asked"], 0.03);
            assert_eq!(settings["boundary_cell_asked"], 0.05);
        }
    }
}

#[test]
fn a_point_far_from_the_scan_does_not_make_the_voxels_larger() {
    // A sheet at national grid coordinates and one point at the origin of
    // the scene, 488 km away.
    let sheet = plane_with_hole([2.0, 2.0], 0.01, None)
        .with_noise(Noise::Gaussian(0.003), 68)
        .transformed(0.0, [155_000.0, 463_000.0, 0.5]);
    let mut far = Shape::default();
    far.points.push([0.0; 3]);
    far.normals.push([0.0, 0.0, 1.0]);
    far.station_of.push(u32::MAX);
    let found = detect(&sheet.merged(far), &SurfaceDetectConfig::default());
    assert_eq!(found.voxel_size, 0.03);
    assert!(!found.is_coarse());
    assert_eq!(found.planes.len(), 1);
    assert!((found.planes[0].area - 1.99 * 1.99).abs() < 0.01);
    assert_eq!(found.assigned_points, 40_000);
}

#[test]
fn only_the_points_of_the_faces_in_the_result_count_as_assigned() {
    let room = noisy_room();
    // Floor and ceiling are 12 m2, the walls less.
    let with_minimum = |area: f64| {
        detect(
            &room,
            &SurfaceDetectConfig {
                min_region_area: area,
                ..SurfaceDetectConfig::default()
            },
        )
    };
    let none = with_minimum(13.0);
    assert!(none.planes.is_empty() && none.edges.is_empty());
    assert_eq!(none.assigned_points, 0);
    let two = with_minimum(11.0);
    assert_eq!(classes(&two), [1, 1, 0, 0]);
    assert_eq!(
        two.assigned_points,
        two.planes
            .iter()
            .map(|face| face.residuals.points)
            .sum::<u64>()
    );
    // Give or take the points in the corners, which the walls no longer
    // take.
    assert!(
        two.assigned_points.abs_diff(60_000) < 2_000,
        "{}",
        two.assigned_points
    );
    // A face whose outline comes out below the smallest area is not
    // reported: 0.5 by 0.5 m less half a point spacing all round.
    let small = plane_with_hole([0.5, 0.5], 0.01, None).with_noise(Noise::Gaussian(0.003), 69);
    let found = detect(&small, &SurfaceDetectConfig::default());
    assert!(found.planes.is_empty());
    assert_eq!(found.assigned_points, 0);
    // Standing on a floor, such a face leaves no edge behind either: 1 by
    // 1 m of points, of which the outline holds 0.99 m2, with 1 m2 asked.
    let floor = plane_with_hole([2.0, 2.0], 0.01, None);
    let upright = rectangle(
        [1.0, 0.5, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 1.0],
        0.01,
    );
    let both = floor.merged(upright).with_noise(Noise::Gaussian(0.003), 70);
    let found = detect(&both, &SurfaceDetectConfig::default());
    assert_eq!((found.planes.len(), found.edges.len()), (2, 1));
    let found = detect(
        &both,
        &SurfaceDetectConfig {
            min_region_area: 1.0,
            ..SurfaceDetectConfig::default()
        },
    );
    assert_eq!(found.planes.len(), 1);
    assert!(found.edges.is_empty(), "{:?}", found.edges);
    assert_eq!(found.assigned_points, found.planes[0].residuals.points);
    assert!(found.assigned_points < found.source_points - 9_000);
}

#[test]
fn faces_of_a_room_with_a_column_export_as_dxf_dwg_and_ifc() {
    use crate::cad3d::step::check;
    use crate::cad3d::tests::{read_cad, FAR};
    use crate::cad3d::{plane_layer, CAD_LAYER_CYLINDERS, CAD_LAYER_CYLINDER_AXES};
    use crate::DrawingFormat;
    use cadcodec::entities::EntityType;
    use std::collections::BTreeMap;

    let detected = detect(&room_with_column(), &SurfaceDetectConfig::default());
    assert_eq!((detected.planes.len(), detected.cylinders.len()), (6, 1));
    let directory = tempfile::tempdir().unwrap();
    for shift in [[0.0; 3], FAR] {
        let found = detected
            .placed(SourceTransform {
                scale: [1.0; 3],
                offset: shift,
            })
            .unwrap();
        let column = &found.cylinders[0];
        for format in DrawingFormat::ALL {
            let path = directory
                .path()
                .join(format!("faces.{}", format.extension()));
            write_faces_cad(&found, &path, format).unwrap();
            let document = read_cad(&path, format);
            let mut per_layer: BTreeMap<String, usize> = BTreeMap::new();
            for entity in document.entities() {
                let layer = entity.common().layer.clone();
                match entity {
                    EntityType::PolyfaceMesh(mesh) if layer == CAD_LAYER_CYLINDERS => {
                        for vertex in &mesh.vertices {
                            let at = [vertex.location.x, vertex.location.y, vertex.location.z];
                            let radius = off_axis(at, column.axis_start, column.axis_end);
                            assert!((radius - column.radius).abs() < 1e-6);
                        }
                    }
                    EntityType::PolyfaceMesh(mesh) => {
                        // The corners of the polyface are those of the
                        // outline of a face of its class, in order.
                        let corners: Vec<[f64; 3]> = mesh
                            .vertices
                            .iter()
                            .map(|vertex| [vertex.location.x, vertex.location.y, vertex.location.z])
                            .collect();
                        let face = found.planes.iter().find(|face| {
                            let rings = face.rings().concat();
                            plane_layer(face.class) == layer
                                && rings.len() == corners.len()
                                && rings
                                    .iter()
                                    .zip(&corners)
                                    .all(|(ring, corner)| apart(*ring, *corner) < 1e-6)
                        });
                        assert!(face.is_some(), "a face for every polyface on {layer}");
                    }
                    EntityType::Line(line) => {
                        assert_eq!(layer, CAD_LAYER_CYLINDER_AXES);
                        let start = [line.start.x, line.start.y, line.start.z];
                        let end = [line.end.x, line.end.y, line.end.z];
                        assert!(apart(start, column.axis_start) < 1e-6);
                        assert!(apart(end, column.axis_end) < 1e-6);
                    }
                    other => panic!("unexpected {other:?}"),
                }
                *per_layer.entry(layer).or_default() += 1;
            }
            let expected: BTreeMap<String, usize> = [
                (plane_layer(FaceClass::Floor), 1),
                (plane_layer(FaceClass::Ceiling), 1),
                (plane_layer(FaceClass::Wall), 4),
                (CAD_LAYER_CYLINDERS.to_owned(), 1),
                (CAD_LAYER_CYLINDER_AXES.to_owned(), 1),
            ]
            .into_iter()
            .collect();
            assert_eq!(per_layer, expected, "{format}");
        }

        let path = directory.path().join("faces.ifc");
        write_faces_ifc(&found, "C:/scans/room.e57", &path, &["Units: metres"]).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let instances = check::read(&text).unwrap();
        let count = |name: &str| {
            instances
                .values()
                .filter(|instance| instance.name == name)
                .count()
        };
        assert_eq!(count("IFCBUILDINGELEMENTPROXY"), 7);
        assert_eq!(count("IFCPOLYGONALFACESET"), 6);
        assert_eq!(count("IFCEXTRUDEDAREASOLID"), 1);
        assert_eq!(count("IFCTRIANGULATEDFACESET"), 0);
        assert!(text.contains("'Plane (wall)'") && text.contains("'Face 7 (cylinder)'"));
        assert!(text.contains("'Source: room.e57; Units: metres'"));
        // The room spans 4 by 3 m from the shift: the site stands at its
        // middle, rounded, on the axes far from zero.
        let origin = crate::cad3d::ifc::local_origin(&faces_model(&found, "", &[]).unwrap());
        if shift == FAR {
            assert_eq!(origin[0], 207_002.0);
            assert!([474_001.0, 474_002.0].contains(&origin[1]), "{origin:?}");
            assert_eq!(origin[2], 0.0);
        } else {
            assert_eq!(origin, [0.0; 3]);
        }
        let [x, y, z] = origin.map(crate::cad3d::step::length);
        let site_point = format!("IFCCARTESIANPOINT(({x},{y},{z}))");
        assert!(text.contains(&site_point), "{site_point}");
    }

    // An opening is an inner loop of its face.
    let windowed = detect(&windowed_room(), &SurfaceDetectConfig::default());
    let path = directory.path().join("windowed.ifc");
    write_faces_ifc(&windowed, "room.e57", &path, &[]).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let instances = check::read(&text).unwrap();
    let holes = windowed
        .planes
        .iter()
        .flat_map(|face| &face.patches)
        .filter(|patch| !patch.holes.is_empty())
        .count();
    assert!(holes >= 1);
    let with_voids = instances
        .values()
        .filter(|instance| instance.name == "IFCINDEXEDPOLYGONALFACEWITHVOIDS")
        .count();
    assert_eq!(with_voids, holes);
}
