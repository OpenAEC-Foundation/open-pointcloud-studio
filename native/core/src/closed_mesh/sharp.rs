//! Sharp edges and corners: generated scans of shapes whose faces are known
//! exactly, meshed at several voxels and tolerances and measured against
//! those faces. A stair, the corner of a room, the outside corner of a wall
//! and a corner of 135 degrees, all turned off the axes of the lattice and
//! scanned with a point every 5 mm and 2 mm of noise.

use super::*;
use crate::local_fit::{cross, difference, dot};
use crate::mesh_quality::mesh_deviation;
use crate::region_source::SourceTransform;
use crate::test_shapes::{rectangle, Noise, Shape};
use crate::{IndexedPoint, Point, ScanRange};

const SPACING: f64 = 0.005;
const NOISE: f64 = 0.002;
/// The scenes are turned about the vertical and moved, so that no face lies
/// on a lattice plane and no horizontal edge along a lattice axis.
const TURN: f64 = 17.3;
const SHIFT: [f64; 3] = [3.1, -2.7, 0.37];

/// A rectangle of the true surface, seen from the side of `along x up`.
#[derive(Clone, Copy)]
struct Face {
    origin: [f64; 3],
    along: [f64; 3],
    up: [f64; 3],
    size: [f64; 2],
}

impl Face {
    fn normal(&self) -> [f64; 3] {
        cross(self.along, self.up)
    }

    fn distance(&self, point: [f64; 3]) -> f64 {
        let relative = difference(point, self.origin);
        let a = dot(relative, self.along).clamp(0.0, self.size[0]);
        let b = dot(relative, self.up).clamp(0.0, self.size[1]);
        let nearest: [f64; 3] = std::array::from_fn(|axis| {
            self.origin[axis] + a * self.along[axis] + b * self.up[axis]
        });
        length(difference(point, nearest))
    }
}

/// Where two faces meet, with the normals of both.
#[derive(Clone, Copy)]
struct Edge {
    from: [f64; 3],
    to: [f64; 3],
    normals: [[f64; 3]; 2],
}

impl Edge {
    fn distance(&self, point: [f64; 3]) -> f64 {
        let span = difference(self.to, self.from);
        let along = (dot(difference(point, self.from), span) / dot(span, span)).clamp(0.0, 1.0);
        let nearest: [f64; 3] = std::array::from_fn(|axis| self.from[axis] + along * span[axis]);
        length(difference(point, nearest))
    }
}

struct Scene {
    name: &'static str,
    faces: Vec<Face>,
    edges: Vec<Edge>,
    /// Where three faces meet.
    corners: Vec<[f64; 3]>,
    stations: Vec<[f64; 3]>,
    /// Only what lies in this box counts: the rims of the scan lie outside.
    inside: Bounds,
}

fn length(v: [f64; 3]) -> f64 {
    dot(v, v).sqrt()
}

const X: [f64; 3] = [1.0, 0.0, 0.0];
const Y: [f64; 3] = [0.0, 1.0, 0.0];
const Z: [f64; 3] = [0.0, 0.0, 1.0];
const NX: [f64; 3] = [-1.0, 0.0, 0.0];
const NY: [f64; 3] = [0.0, -1.0, 0.0];

fn face(origin: [f64; 3], along: [f64; 3], up: [f64; 3], size: [f64; 2]) -> Face {
    Face {
        origin,
        along,
        up,
        size,
    }
}

fn edge(from: [f64; 3], to: [f64; 3], a: [f64; 3], b: [f64; 3]) -> Edge {
    Edge {
        from,
        to,
        normals: [a, b],
    }
}

/// Six steps of 17.5 cm by 28 cm between two walls 1 m apart, from a floor
/// to a landing, seen from the foot and from the head of the stair.
fn stair() -> Scene {
    let (steps, rise, run, width) = (6usize, 0.175, 0.28, 1.0);
    let landing = 0.8;
    let top = steps as f64 * rise + 1.2;
    let end = steps as f64 * run + landing;
    let mut faces = vec![face([-1.0, 0.0, 0.0], X, Y, [1.0, width])];
    let mut edges = vec![edge([0.0, 0.0, 0.0], [0.0, width, 0.0], Z, NX)];
    let mut corners = Vec::new();
    // The walls in columns: under the floor, every step and the landing.
    let mut columns = vec![(-1.0, 0.0, 0.0)];
    for step in 0..steps {
        let (x, low, high) = (
            step as f64 * run,
            step as f64 * rise,
            (step + 1) as f64 * rise,
        );
        let deep = if step + 1 == steps {
            run + landing
        } else {
            run
        };
        faces.push(face([x, 0.0, low], Z, Y, [rise, width]));
        faces.push(face([x, 0.0, high], X, Y, [deep, width]));
        columns.push((x, x + deep, high));
        // The nosing, and the foot of the riser on the tread below.
        edges.push(edge([x, 0.0, high], [x, width, high], NX, Z));
        if step > 0 {
            edges.push(edge([x, 0.0, low], [x, width, low], Z, NX));
        }
        for (y, wall) in [(0.0, Y), (width, NY)] {
            edges.push(edge([x, y, low], [x, y, high], NX, wall));
            edges.push(edge([x, y, high], [x + deep, y, high], Z, wall));
            corners.push([x, y, low]);
            corners.push([x, y, high]);
        }
    }
    for (y, wall) in [(0.0, Y), (width, NY)] {
        edges.push(edge([-1.0, y, 0.0], [0.0, y, 0.0], Z, wall));
    }
    for (from, to, low) in columns {
        faces.push(face([from, 0.0, low], Z, X, [top - low, to - from]));
        faces.push(face([from, width, low], X, Z, [to - from, top - low]));
    }
    Scene {
        name: "stair",
        faces,
        edges,
        corners,
        stations: vec![
            [-0.8, 0.5 * width, 1.7],
            [end - 0.3, 0.5 * width, steps as f64 * rise + 1.6],
        ],
        inside: Bounds {
            min: [-0.8, -1.0, -1.0],
            max: [end - 0.2, width + 1.0, top - 0.2],
        },
    }
}

/// The corner of a room where two walls meet the floor, seen from inside.
fn room_corner() -> Scene {
    let size = 1.5;
    Scene {
        name: "room corner",
        faces: vec![
            face([0.0; 3], X, Y, [size, size]),
            face([0.0; 3], Y, Z, [size, size]),
            face([0.0; 3], Z, X, [size, size]),
        ],
        edges: vec![
            edge([0.0; 3], [size, 0.0, 0.0], Z, Y),
            edge([0.0; 3], [0.0, size, 0.0], Z, X),
            edge([0.0; 3], [0.0, 0.0, size], X, Y),
        ],
        corners: vec![[0.0; 3]],
        stations: vec![[1.0, 1.0, 1.2]],
        inside: Bounds {
            min: [-1.0; 3],
            max: [size - 0.2; 3],
        },
    }
}

/// The outside corner of a building, square, seen from outside.
fn outside_corner() -> Scene {
    let (size, height) = (1.5, 2.0);
    Scene {
        name: "outside corner",
        faces: vec![
            face([0.0; 3], X, Z, [size, height]),
            face([0.0; 3], Z, Y, [height, size]),
        ],
        edges: vec![edge([0.0; 3], [0.0, 0.0, height], NY, NX)],
        corners: Vec::new(),
        stations: vec![[-1.2, -1.2, 1.4]],
        inside: Bounds {
            min: [-1.0, -1.0, 0.2],
            max: [size - 0.2, size - 0.2, height - 0.2],
        },
    }
}

/// An outside corner where a wall turns by 45 degrees, as at a bay.
fn bay_corner() -> Scene {
    let (size, height) = (1.5, 2.0);
    let half = std::f64::consts::FRAC_1_SQRT_2;
    let turned = [half, half, 0.0];
    let normal = [half, -half, 0.0];
    Scene {
        name: "135 corner",
        faces: vec![
            face([-size, 0.0, 0.0], X, Z, [size, height]),
            face([0.0; 3], turned, Z, [size, height]),
        ],
        edges: vec![edge([0.0; 3], [0.0, 0.0, height], NY, normal)],
        corners: Vec::new(),
        stations: vec![[0.3, -1.5, 1.4]],
        inside: Bounds {
            min: [-size + 0.2, -1.0, 0.2],
            max: [size * half - 0.2, size * half - 0.2, height - 0.2],
        },
    }
}

fn scenes() -> Vec<Scene> {
    vec![stair(), room_corner(), outside_corner(), bay_corner()]
}

impl Scene {
    /// The scan: points every 5 mm, a little off a regular pattern, with
    /// noise along the normal, each given to the station that sees it best,
    /// and all turned and moved.
    fn scan(&self) -> Shape {
        let mut shape = Shape::default();
        for face in &self.faces {
            shape = shape.merged(rectangle(
                face.origin,
                face.along,
                face.up,
                face.size,
                SPACING,
            ));
        }
        shape
            .scattered(0.4 * SPACING, 11)
            .with_noise(Noise::Gaussian(NOISE), 12)
            .with_stations(&self.stations)
            .transformed(TURN, SHIFT)
    }

    fn surface_distance(&self, point: [f64; 3]) -> f64 {
        self.faces
            .iter()
            .map(|face| face.distance(point))
            .fold(f64::INFINITY, f64::min)
    }

    /// The nearest edge and how far it is.
    fn nearest_edge(&self, point: [f64; 3]) -> (f64, usize) {
        self.edges
            .iter()
            .enumerate()
            .map(|(number, edge)| (edge.distance(point), number))
            .fold((f64::INFINITY, 0), |best, next| {
                if next.0 < best.0 {
                    next
                } else {
                    best
                }
            })
    }

    fn holds(&self, point: [f64; 3]) -> bool {
        (0..3).all(|axis| {
            point[axis] >= self.inside.min[axis] && point[axis] <= self.inside.max[axis]
        })
    }
}

/// From the scene back to the frame the shape was made in.
fn local(point: [f64; 3]) -> [f64; 3] {
    let moved = difference(point, SHIFT);
    let (sin, cos) = (-TURN).to_radians().sin_cos();
    [
        cos * moved[0] - sin * moved[1],
        sin * moved[0] + cos * moved[1],
        moved[2],
    ]
}

fn ranges(shape: &Shape) -> Vec<ScanRange> {
    shape
        .station_ranges()
        .into_iter()
        .map(|(first_ordinal, station)| ScanRange {
            first_ordinal,
            station: (station != u32::MAX).then_some(station),
        })
        .collect()
}

fn mesh_scan(shape: &Shape, config: &ClosedMeshConfig) -> (MeshGeometry, ClosedMeshReport) {
    let points: Vec<IndexedPoint> = shape
        .points
        .iter()
        .enumerate()
        .map(|(ordinal, xyz)| IndexedPoint {
            point: Point {
                xyz: *xyz,
                rgb: None,
                intensity: None,
                classification: None,
            },
            ordinal: ordinal as u64,
        })
        .collect();
    let sources = [SurfelSource::with_stations(
        RegionSource::resident(&points, SourceTransform::default()),
        shape.stations.clone(),
        ranges(shape),
    )];
    mesh_closed(&sources, None, &|_, _, _| true, config, &mut |_| Ok(())).unwrap()
}

/// Mean, 95th percentile and largest of some distances, in millimetres.
#[derive(Debug, Clone, Copy, Default)]
struct Spread {
    mean: f64,
    p95: f64,
    max: f64,
}

fn spread(mut values: Vec<f64>) -> Spread {
    if values.is_empty() {
        return Spread::default();
    }
    values.sort_unstable_by(f64::total_cmp);
    let rank = (values.len() * 95).div_ceil(100).max(1);
    Spread {
        mean: 1e3 * values.iter().sum::<f64>() / values.len() as f64,
        p95: 1e3 * values[rank - 1],
        max: 1e3 * values[values.len() - 1],
    }
}

/// How well a mesh keeps the edges of a scene.
#[derive(Debug, Clone, Copy, Default)]
struct Sharpness {
    triangles: usize,
    /// From the true edges to the mesh: how far a rounded edge falls short.
    gap: Spread,
    /// From the true corners to the mesh, the largest, in millimetres.
    corner_gap: f64,
    /// From the vertices within two voxels of an edge to the true surface.
    near: Spread,
    /// From the other vertices to the true surface.
    away: Spread,
    /// Triangles with a corner under 5 degrees, inside the box.
    slivers: usize,
    non_manifold: u64,
    /// Open edges inside the box, away from the rims of the scan.
    tears: usize,
    /// Share of the area within a voxel of an edge that faces within 15
    /// degrees of one of the two faces of that edge.
    on_faces: f64,
    /// Triangles inside the box that face away from the nearest face.
    folded: usize,
    seconds: f64,
}

/// Whether a triangle faces away from the face it lies on, and near an
/// edge from both faces of that edge.
fn folded(scene: &Scene, middle: [f64; 3], face: [f64; 3], normal: [f64; 3], voxel: f64) -> bool {
    let (distance, number) = scene.nearest_edge(middle);
    let mut best = dot(face, normal);
    if distance <= 2.0 * voxel {
        for side in scene.edges[number].normals {
            best = best.max(dot(side, normal));
        }
    }
    best < 0.0
}

fn measure(scene: &Scene, mesh: &MeshGeometry, voxel: f64) -> Sharpness {
    let vertices: Vec<[f64; 3]> = mesh.vertices.iter().map(|vertex| local(*vertex)).collect();
    let local_mesh = MeshGeometry {
        vertices: vertices.clone(),
        triangles: mesh.triangles.clone(),
        colors: None,
        normals: None,
    };
    let mut found = Sharpness {
        triangles: mesh.triangles.len(),
        ..Sharpness::default()
    };
    // Samples along the true edges, a quarter voxel apart.
    let mut samples = Vec::new();
    for edge in &scene.edges {
        let span = difference(edge.to, edge.from);
        let count = (length(span) / (0.25 * voxel)).ceil() as usize;
        for step in 0..=count {
            let along = step as f64 / count as f64;
            let point = std::array::from_fn(|axis| edge.from[axis] + along * span[axis]);
            if scene.holds(point) {
                samples.push(point);
            }
        }
    }
    let gap = mesh_deviation(&local_mesh, &samples);
    found.gap = Spread {
        mean: 1e3 * gap.mean,
        p95: 1e3 * gap.p95,
        max: 1e3 * gap.max,
    };
    let corners: Vec<[f64; 3]> = scene
        .corners
        .iter()
        .copied()
        .filter(|c| scene.holds(*c))
        .collect();
    if !corners.is_empty() {
        found.corner_gap = 1e3 * mesh_deviation(&local_mesh, &corners).max;
    }
    let (mut near, mut away) = (Vec::new(), Vec::new());
    for vertex in vertices.iter().filter(|vertex| scene.holds(**vertex)) {
        let distance = scene.surface_distance(*vertex);
        if scene.nearest_edge(*vertex).0 <= 2.0 * voxel {
            near.push(distance);
        } else {
            away.push(distance);
        }
    }
    found.near = spread(near);
    found.away = spread(away);

    let mut edges: Vec<(u32, u32)> = Vec::new();
    let (mut on_faces, mut by_edges) = (0.0, 0.0);
    for triangle in &mesh.triangles {
        let [a, b, c] = triangle.map(|vertex| vertices[vertex as usize]);
        for (from, to) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            edges.push((from.min(to), from.max(to)));
        }
        let middle: [f64; 3] = std::array::from_fn(|axis| (a[axis] + b[axis] + c[axis]) / 3.0);
        if !scene.holds(middle) {
            continue;
        }
        let sides = [difference(b, a), difference(c, b), difference(a, c)];
        let smallest = (0..3)
            .map(|corner| {
                let (u, v) = (sides[corner], sides[(corner + 2) % 3].map(|value| -value));
                let cosine = dot(u, v) / (length(u) * length(v)).max(1e-300);
                cosine.clamp(-1.0, 1.0).acos().to_degrees()
            })
            .fold(f64::INFINITY, f64::min);
        if smallest < 5.0 {
            found.slivers += 1;
        }
        let nearest_face = scene
            .faces
            .iter()
            .map(|face| (face.distance(middle), face.normal()))
            .fold((f64::INFINITY, [0.0; 3]), |best, next| {
                if next.0 < best.0 {
                    next
                } else {
                    best
                }
            });
        if folded(
            scene,
            middle,
            nearest_face.1,
            cross(sides[0], difference(c, a)),
            voxel,
        ) {
            found.folded += 1;
        }
        let (distance, number) = scene.nearest_edge(middle);
        if distance <= voxel {
            let normal = cross(sides[0], difference(c, a));
            let area = 0.5 * length(normal);
            if area > 0.0 {
                let cosine = scene.edges[number]
                    .normals
                    .iter()
                    .map(|face| dot(*face, normal) / (2.0 * area))
                    .fold(f64::NEG_INFINITY, f64::max);
                by_edges += area;
                if cosine >= 15f64.to_radians().cos() {
                    on_faces += area;
                }
            }
        }
    }
    found.on_faces = if by_edges > 0.0 {
        on_faces / by_edges
    } else {
        1.0
    };
    edges.sort_unstable();
    for run in edges.chunk_by(|a, b| a == b) {
        if run.len() > 2 {
            found.non_manifold += 1;
        } else if run.len() == 1 {
            let (a, b) = (vertices[run[0].0 as usize], vertices[run[0].1 as usize]);
            let middle: [f64; 3] = std::array::from_fn(|axis| 0.5 * (a[axis] + b[axis]));
            if scene.holds(middle) {
                found.tears += 1;
            }
        }
    }
    found
}

fn settings(voxel: f64) -> [(&'static str, Option<f64>); 3] {
    [("0", Some(0.0)), ("auto", None), ("1 voxel", Some(voxel))]
}

fn run(scene: &Scene, shape: &Shape, voxel: f64, tolerance: Option<f64>) -> Sharpness {
    let config = ClosedMeshConfig {
        voxel: Some(voxel),
        simplify_tolerance: tolerance,
        ..ClosedMeshConfig::default()
    };
    let started = Instant::now();
    let (mesh, report) = mesh_scan(shape, &config);
    let seconds = started.elapsed().as_secs_f64();
    let mut found = measure(scene, &mesh, voxel);
    found.non_manifold = found.non_manifold.max(report.topology.non_manifold_edges);
    found.seconds = seconds;
    found
}

/// Measured on the four scenes, with the method before edges were taken
/// from the faces in brackets: at a convex edge the mean of the planes of
/// two faces lies inside the solid and at a concave one outside it, so the
/// edges came out rounded by about a sixth of a voxel, with triangles
/// between the two faces along them.
#[test]
fn edges_and_corners_of_generated_scans_stay_sharp() {
    let check = |scene: Scene, voxel: f64, tolerance: Option<f64>, gap: [f64; 2], corner: f64| {
        let shape = scene.scan();
        let found = run(&scene, &shape, voxel, tolerance);
        let name = scene.name;
        assert!(
            found.gap.mean < gap[0] && found.gap.p95 < gap[1],
            "{name} {voxel} {:?}",
            found.gap
        );
        assert!(
            found.corner_gap < corner,
            "{name} {voxel} {}",
            found.corner_gap
        );
        // The vertices along the edges lie on the faces, and so do the
        // triangles there: they face like one of the two faces.
        assert!(
            found.near.mean < 0.07 * voxel * 1e3,
            "{name} {voxel} {:?}",
            found.near
        );
        assert!(found.on_faces > 0.9, "{name} {voxel} {}", found.on_faces);
        assert_eq!((found.non_manifold, found.tears), (0, 0), "{name} {voxel}");
        assert!(found.folded <= 2, "{name} {voxel} {}", found.folded);
        found
    };
    // Measured 3.2 mm and 7.4 mm from the edges to the mesh (6.7 and 12.6),
    // 15.6 mm at the worst corner where a nosing meets a wall (18.5), the
    // vertices near the edges 0.9 mm off the faces (4.5), 96 % of the
    // triangles along the edges facing like a face (50 %), 28 triangles
    // with a corner under 5 degrees (73) and 1 that faces away (1).
    let stair_raw = check(stair(), 0.04, Some(0.0), [4.0, 9.5], 17.5);
    assert!(stair_raw.slivers < 45, "{}", stair_raw.slivers);
    // Simplified as a job does by default, at voxels of 2 cm: 1.7 and 3.3
    // mm (3.3 and 5.4) in 4,102 triangles (6,957). Simplification keeps
    // long thin triangles along the edges: 70 with a corner under 5
    // degrees (44).
    let stair_simplified = check(stair(), 0.02, None, [2.2, 4.3], 11.0);
    assert!(
        stair_simplified.slivers < 90,
        "{}",
        stair_simplified.slivers
    );
    // 2.6 and 5.3 mm (5.2 and 7.1); the corner where the floor meets the
    // two walls 5.9 mm off (11.1).
    check(room_corner(), 0.04, Some(0.0), [3.3, 6.5], 8.0);
    // 2.9 and 4.0 mm (6.4 and 6.9).
    check(outside_corner(), 0.04, Some(0.0), [3.8, 5.0], f64::INFINITY);
    // 1.7 and 2.1 mm (2.9 and 3.2).
    check(bay_corner(), 0.03, Some(0.0), [2.3, 2.8], f64::INFINITY);
}

/// The edges are found from the elements in reach of each corner, which
/// two tiles must see alike: the stair cut into many small tiles gives the
/// same vertices, bit for bit, as in one tile.
#[test]
fn sharp_edges_join_across_tiles_without_seams() {
    let scene = stair();
    let shape = scene.scan();
    let config = |tile_voxels| ClosedMeshConfig {
        voxel: Some(0.04),
        simplify_tolerance: Some(0.0),
        tile_voxels,
        ..ClosedMeshConfig::default()
    };
    let sorted = |mesh: &MeshGeometry| {
        let mut vertices: Vec<[u64; 3]> = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.map(f64::to_bits))
            .collect();
        vertices.sort_unstable();
        vertices
    };
    let (one, report) = mesh_scan(&shape, &config(256));
    assert_eq!(report.tiles, 1);
    let (many, report) = mesh_scan(&shape, &config(32));
    assert!(report.tiles >= 8, "{}", report.tiles);
    assert_eq!(report.seam_faults, 0);
    assert_eq!(sorted(&one), sorted(&many));
    assert_eq!(one.triangles.len(), many.triangles.len());
}

/// The whole table: `cargo test --release -p pointcloud-core sharp_edge_table
/// -- --ignored --nocapture`.
#[test]
#[ignore]
fn sharp_edge_table() {
    println!(
        "| scene | voxel | simplify | triangles | edge gap mean/p95/max mm | corner gap mm | \
         near edge mean/p95/max mm | away mean/p95/max mm | slivers | non-manifold | tears | \
         on faces | folded | s |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for scene in scenes() {
        let shape = scene.scan();
        for voxel in [0.02, 0.03, 0.04, 0.05] {
            for (name, tolerance) in settings(voxel) {
                let found = run(&scene, &shape, voxel, tolerance);
                println!(
                    "| {} | {:.0} cm | {} | {} | {:.1}/{:.1}/{:.1} | {:.1} | {:.1}/{:.1}/{:.1} | \
                     {:.1}/{:.1}/{:.1} | {} | {} | {} | {:.2} | {} | {:.1} |",
                    scene.name,
                    voxel * 100.0,
                    name,
                    found.triangles,
                    found.gap.mean,
                    found.gap.p95,
                    found.gap.max,
                    found.corner_gap,
                    found.near.mean,
                    found.near.p95,
                    found.near.max,
                    found.away.mean,
                    found.away.p95,
                    found.away.max,
                    found.slivers,
                    found.non_manifold,
                    found.tears,
                    found.on_faces,
                    found.folded,
                    found.seconds,
                );
            }
        }
    }
}
