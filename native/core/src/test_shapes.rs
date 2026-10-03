//! Synthetic scans for tests: a room, a sphere, a cylinder and a plane, each
//! with the direction every point was seen from, and a helper that puts such
//! a cloud in a temporary file with an octree index. Everything is
//! generated, from fixed seeds, so the tests need no scan files and give the
//! same numbers on every run.

use std::io::Write;
use std::path::Path;

use crate::local_fit::{cross, difference, dot, unit};
use crate::{open, Bounds, IndexConfig, OctreeIndex, Point, PointCloud};

/// A small deterministic random generator (xorshift64*).
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        // Spread the seed so that small seeds do not start with small states.
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in 0 up to, but not including, 1.
    pub(crate) fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Normally distributed with mean 0 and standard deviation 1.
    pub(crate) fn gaussian(&mut self) -> f64 {
        let radius = (-2.0 * (1.0 - self.unit()).ln()).sqrt();
        radius * (std::f64::consts::TAU * self.unit()).cos()
    }
}

/// Measurement noise along the surface normal.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Noise {
    /// Normally distributed with this standard deviation.
    Gaussian(f64),
    /// Evenly spread between minus and plus this amplitude.
    Uniform(f64),
}

/// Points on a surface with what a scanner would know about them.
#[derive(Debug, Clone, Default)]
pub(crate) struct Shape {
    pub(crate) points: Vec<[f64; 3]>,
    /// Unit normal of the surface at each point, on the side it is seen from.
    pub(crate) normals: Vec<[f64; 3]>,
    /// Scanner positions.
    pub(crate) stations: Vec<[f64; 3]>,
    /// Per point the station that saw it, or `u32::MAX` without stations.
    /// Points of one station follow each other, as in a scan file.
    pub(crate) station_of: Vec<u32>,
}

impl Shape {
    fn push(&mut self, point: [f64; 3], normal: [f64; 3], station: u32) {
        self.points.push(point);
        self.normals.push(normal);
        self.station_of.push(station);
    }

    /// The same points seen from the other side of the surface.
    pub(crate) fn flipped(mut self) -> Self {
        for normal in &mut self.normals {
            *normal = normal.map(|value| -value);
        }
        self
    }

    /// Displace every point along its normal.
    pub(crate) fn with_noise(mut self, noise: Noise, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        for (point, normal) in self.points.iter_mut().zip(&self.normals) {
            let offset = match noise {
                Noise::Gaussian(sigma) => rng.gaussian() * sigma,
                Noise::Uniform(amplitude) => (rng.unit() * 2.0 - 1.0) * amplitude,
            };
            for axis in 0..3 {
                point[axis] += offset * normal[axis];
            }
        }
        self
    }

    /// Turn the shape about the vertical through the origin and then move
    /// it, as a building that does not follow the axes at national grid
    /// coordinates.
    pub(crate) fn transformed(mut self, degrees: f64, translation: [f64; 3]) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let turn = |v: [f64; 3]| [cos * v[0] - sin * v[1], sin * v[0] + cos * v[1], v[2]];
        for point in self.points.iter_mut().chain(&mut self.stations) {
            let turned = turn(*point);
            *point = std::array::from_fn(|axis| turned[axis] + translation[axis]);
        }
        for normal in &mut self.normals {
            *normal = turn(*normal);
        }
        self
    }

    /// Tip the shape about the x axis through the origin: a level plane
    /// becomes a roof plane that rises towards +y at this slope.
    pub(crate) fn tilted(mut self, degrees: f64) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let turn = |v: [f64; 3]| [v[0], cos * v[1] - sin * v[2], sin * v[1] + cos * v[2]];
        for point in self.points.iter_mut().chain(&mut self.stations) {
            *point = turn(*point);
        }
        for normal in &mut self.normals {
            *normal = turn(*normal);
        }
        self
    }

    /// Move every point at random within its surface, up to `amount` along
    /// each of two directions square to its normal: a scan does not put its
    /// points on a lattice.
    pub(crate) fn scattered(mut self, amount: f64, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        for (point, normal) in self.points.iter_mut().zip(&self.normals) {
            let helper = if normal[2].abs() < 0.9 {
                [0.0, 0.0, 1.0]
            } else {
                [1.0, 0.0, 0.0]
            };
            let Some(first) = unit(cross(helper, *normal)) else {
                continue;
            };
            let second = cross(*normal, first);
            let (a, b) = (
                (rng.unit() * 2.0 - 1.0) * amount,
                (rng.unit() * 2.0 - 1.0) * amount,
            );
            for axis in 0..3 {
                point[axis] += a * first[axis] + b * second[axis];
            }
        }
        self
    }

    /// Give every point to the station that sees its surface most squarely,
    /// and order the points station by station. A point no station can see
    /// goes to the nearest one.
    pub(crate) fn with_stations(mut self, stations: &[[f64; 3]]) -> Self {
        self.stations = stations.to_vec();
        if stations.is_empty() {
            self.station_of = vec![u32::MAX; self.points.len()];
            return self;
        }
        let station_of: Vec<u32> = self
            .points
            .iter()
            .zip(&self.normals)
            .map(|(point, normal)| {
                let mut best = (f64::NEG_INFINITY, 0);
                for (index, station) in stations.iter().enumerate() {
                    let towards = difference(*station, *point);
                    let distance = dot(towards, towards).sqrt();
                    let facing = dot(*normal, towards) / distance.max(1e-12);
                    // Facing decides; among stations behind the surface the
                    // nearest wins.
                    let score = if facing > 0.0 {
                        1.0 + facing
                    } else {
                        -distance
                    };
                    if score > best.0 {
                        best = (score, index as u32);
                    }
                }
                best.1
            })
            .collect();
        let mut order: Vec<usize> = (0..self.points.len()).collect();
        order.sort_by_key(|index| station_of[*index]);
        self.points = order.iter().map(|index| self.points[*index]).collect();
        self.normals = order.iter().map(|index| self.normals[*index]).collect();
        self.station_of = order.iter().map(|index| station_of[*index]).collect();
        self
    }

    /// Both shapes as one scan; the stations of the other are added.
    pub(crate) fn merged(mut self, other: Shape) -> Self {
        let shift = self.stations.len() as u32;
        self.points.extend(other.points);
        self.normals.extend(other.normals);
        self.stations.extend(other.stations);
        self.station_of
            .extend(other.station_of.into_iter().map(|station| {
                if station == u32::MAX {
                    station
                } else {
                    station + shift
                }
            }));
        self
    }

    /// The first point of every run of points of one station, with that
    /// station: what a reader records while it walks the scans of a file.
    pub(crate) fn station_ranges(&self) -> Vec<(u64, u32)> {
        let mut ranges: Vec<(u64, u32)> = Vec::new();
        for (ordinal, station) in self.station_of.iter().enumerate() {
            if ranges.last().is_none_or(|last| last.1 != *station) {
                ranges.push((ordinal as u64, *station));
            }
        }
        ranges
    }

    pub(crate) fn bounds(&self) -> Bounds {
        let mut bounds = Bounds {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
        };
        for point in &self.points {
            bounds.include(*point);
        }
        bounds
    }

    /// The points as a reader delivers them. The colour encodes the normal,
    /// so that every face of a room has its own.
    pub(crate) fn cloud_points(&self) -> Vec<Point> {
        self.points
            .iter()
            .zip(&self.normals)
            .map(|(point, normal)| Point {
                xyz: *point,
                rgb: Some(normal.map(|value| (value * 100.0 + 128.0).round() as u8)),
                intensity: None,
                classification: None,
            })
            .collect()
    }
}

/// One of the four walls of a room, named by the side of the plan it is on
/// with x to the east and y to the north.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Wall {
    South,
    East,
    North,
    West,
}

/// A rectangular opening through a wall.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Opening {
    pub(crate) wall: Wall,
    /// Where the opening begins along the wall, measured along x for the
    /// south and north wall and along y for the east and west wall.
    pub(crate) start: f64,
    pub(crate) width: f64,
    /// Height of its lower edge above the floor.
    pub(crate) sill: f64,
    pub(crate) height: f64,
}

impl Opening {
    /// A door opening of 0.9 by 2.1 m from the floor.
    pub(crate) fn door(wall: Wall, start: f64) -> Self {
        Self {
            wall,
            start,
            width: 0.9,
            sill: 0.0,
            height: 2.1,
        }
    }

    /// A window opening of 1.2 by 1.2 m, 0.9 m above the floor.
    pub(crate) fn window(wall: Wall, start: f64) -> Self {
        Self {
            wall,
            start,
            width: 1.2,
            sill: 0.9,
            height: 1.2,
        }
    }

    fn holds(&self, along: f64, z: f64) -> bool {
        along > self.start
            && along < self.start + self.width
            && z > self.sill
            && z < self.sill + self.height
    }
}

/// A rectangular room with its lower south-west inside corner at the origin.
#[derive(Debug, Clone)]
pub(crate) struct RoomSpec {
    /// Inside length (x), width (y) and height (z).
    pub(crate) size: [f64; 3],
    /// Distance between neighbouring points on a face.
    pub(crate) spacing: f64,
    /// With a thickness, the outside faces of the walls are scanned as well
    /// (from a station outside each wall) and openings get their reveals.
    pub(crate) wall_thickness: Option<f64>,
    pub(crate) openings: Vec<Opening>,
    /// Keep only the points between two heights: the slab of a plan.
    pub(crate) slab: Option<[f64; 2]>,
    /// The scanner inside the room.
    pub(crate) station: [f64; 3],
}

impl Default for RoomSpec {
    /// 4.0 by 3.0 by 2.6 m, inside faces only, a point every 2 cm.
    fn default() -> Self {
        Self {
            size: [4.0, 3.0, 2.6],
            spacing: 0.02,
            wall_thickness: None,
            openings: Vec::new(),
            slab: None,
            station: [1.3, 1.1, 1.2],
        }
    }
}

/// Positions along a length, evenly spaced as near to `spacing` as divides
/// the length, half a step in from both ends. An empty length has none.
fn lattice(from: f64, to: f64, spacing: f64) -> impl Iterator<Item = f64> + Clone {
    let count = if to > from {
        ((to - from) / spacing).round().max(1.0) as usize
    } else {
        0
    };
    let step = (to - from) / count as f64;
    (0..count).map(move |index| from + (index as f64 + 0.5) * step)
}

/// The scan of a box-shaped room: floor, ceiling and four walls seen from
/// the station inside. Station 0 is the one inside; with a wall thickness
/// stations 1 to 4 stand 3 m outside the south, east, north and west wall.
pub(crate) fn box_room(spec: &RoomSpec) -> Shape {
    let [sx, sy, sz] = spec.size;
    let spacing = spec.spacing;
    let (z_min, z_max) = match spec.slab {
        Some([low, high]) => (low.max(0.0), high.min(sz)),
        None => (0.0, sz),
    };
    let in_slab = |z: f64| spec.slab.is_none_or(|[low, high]| z >= low && z <= high);
    let open = |wall: Wall, along: f64, z: f64| {
        spec.openings
            .iter()
            .any(|opening| opening.wall == wall && opening.holds(along, z))
    };
    let mut shape = Shape {
        stations: vec![spec.station],
        ..Shape::default()
    };
    for (z, normal) in [(0.0, [0.0, 0.0, 1.0]), (sz, [0.0, 0.0, -1.0])] {
        if in_slab(z) {
            for x in lattice(0.0, sx, spacing) {
                for y in lattice(0.0, sy, spacing) {
                    shape.push([x, y, z], normal, 0);
                }
            }
        }
    }
    // Per wall: the axis it runs along, its length, where its inside face
    // lies across it and the direction from that face into the room.
    let walls = [
        (Wall::South, 0, sx, 0.0, 1.0),
        (Wall::East, 1, sy, sx, -1.0),
        (Wall::North, 0, sx, sy, -1.0),
        (Wall::West, 1, sy, 0.0, 1.0),
    ];
    let place = |along_axis: usize, along: f64, across: f64, z: f64| {
        let mut point = [0.0, 0.0, z];
        point[along_axis] = along;
        point[1 - along_axis] = across;
        point
    };
    let direction = |along_axis: usize, across: f64| {
        let mut normal = [0.0; 3];
        normal[1 - along_axis] = across;
        normal
    };
    for (wall, along_axis, length, face, inward) in walls {
        for along in lattice(0.0, length, spacing) {
            for z in lattice(z_min, z_max, spacing) {
                if !open(wall, along, z) {
                    shape.push(
                        place(along_axis, along, face, z),
                        direction(along_axis, inward),
                        0,
                    );
                }
            }
        }
    }
    let Some(thickness) = spec.wall_thickness else {
        return shape;
    };
    // The reveals of the openings, seen from inside.
    for (wall, along_axis, _, face, inward) in walls {
        for opening in spec.openings.iter().filter(|opening| opening.wall == wall) {
            let (low, high) = (opening.sill, opening.sill + opening.height);
            let (from, to) = (opening.start, opening.start + opening.width);
            let (near, far) = (
                face.min(face - inward * thickness),
                face.max(face - inward * thickness),
            );
            for across in lattice(near, far, spacing) {
                for z in lattice(low.max(z_min), high.min(z_max), spacing) {
                    for (along, side) in [(from, 1.0), (to, -1.0)] {
                        let mut normal = [0.0; 3];
                        normal[along_axis] = side;
                        shape.push(place(along_axis, along, across, z), normal, 0);
                    }
                }
                for along in lattice(from, to, spacing) {
                    if low > 0.0 && in_slab(low) {
                        shape.push(place(along_axis, along, across, low), [0.0, 0.0, 1.0], 0);
                    }
                    if high < sz && in_slab(high) {
                        shape.push(place(along_axis, along, across, high), [0.0, 0.0, -1.0], 0);
                    }
                }
            }
        }
    }
    // The outside faces, each from its own station.
    for (index, (wall, along_axis, length, face, inward)) in walls.into_iter().enumerate() {
        let outside = face - inward * thickness;
        let mut station = [sx * 0.5, sy * 0.5, spec.station[2]];
        station[1 - along_axis] = outside - inward * 3.0;
        shape.stations.push(station);
        for along in lattice(-thickness, length + thickness, spacing) {
            for z in lattice(z_min, z_max, spacing) {
                if !open(wall, along, z) {
                    shape.push(
                        place(along_axis, along, outside, z),
                        direction(along_axis, -inward),
                        index as u32 + 1,
                    );
                }
            }
        }
    }
    shape
}

/// Points spread evenly over a sphere (a Fibonacci lattice), with normals
/// pointing outward and no station.
pub(crate) fn sphere(center: [f64; 3], radius: f64, count: usize) -> Shape {
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    let mut shape = Shape::default();
    for index in 0..count {
        let z = 1.0 - (2.0 * index as f64 + 1.0) / count as f64;
        let ring = (1.0 - z * z).sqrt();
        let angle = golden * index as f64;
        let normal = [ring * angle.cos(), ring * angle.sin(), z];
        shape.push(
            std::array::from_fn(|axis| center[axis] + radius * normal[axis]),
            normal,
            u32::MAX,
        );
    }
    shape
}

/// A cylinder: a column when it stands, a pipe when it lies.
#[derive(Debug, Clone)]
pub(crate) struct CylinderSpec {
    /// Centre of the end the axis starts at.
    pub(crate) base: [f64; 3],
    /// Direction of the axis; it need not be of unit length.
    pub(crate) axis: [f64; 3],
    pub(crate) radius: f64,
    pub(crate) length: f64,
    pub(crate) spacing: f64,
    /// Part of the circumference that is scanned, in degrees; 360 is all.
    pub(crate) arc_degrees: f64,
    /// Whether the two end faces are scanned as well.
    pub(crate) caps: bool,
}

impl CylinderSpec {
    /// A standing column on `base`.
    pub(crate) fn column(base: [f64; 3], radius: f64, height: f64) -> Self {
        Self {
            base,
            axis: [0.0, 0.0, 1.0],
            radius,
            length: height,
            spacing: 0.02,
            arc_degrees: 360.0,
            caps: false,
        }
    }

    /// A pipe from `start` in a direction.
    pub(crate) fn pipe(start: [f64; 3], direction: [f64; 3], radius: f64, length: f64) -> Self {
        Self {
            base: start,
            axis: direction,
            radius,
            length,
            spacing: 0.01,
            arc_degrees: 360.0,
            caps: false,
        }
    }

    /// The axis and two directions across it, all of unit length.
    pub(crate) fn frame(&self) -> [[f64; 3]; 3] {
        let axis = unit(self.axis).expect("a cylinder needs an axis");
        let helper = if axis[2].abs() < 0.9 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let u = unit(cross(helper, axis)).expect("the helper is not along the axis");
        [axis, u, cross(axis, u)]
    }

    /// Four stations around the middle of the cylinder at a distance from
    /// its axis, one beyond its far end and one before its base.
    pub(crate) fn stations_around(&self, distance: f64) -> Vec<[f64; 3]> {
        let [axis, u, v] = self.frame();
        let at = |along: f64, across: [f64; 3], out: f64| -> [f64; 3] {
            std::array::from_fn(|i| self.base[i] + along * axis[i] + out * across[i])
        };
        let middle = self.length * 0.5;
        vec![
            at(middle, u, distance),
            at(middle, v, distance),
            at(middle, u, -distance),
            at(middle, v, -distance),
            at(self.length + distance, u, 0.0),
            at(-distance, u, 0.0),
        ]
    }
}

/// The scan of a cylinder, with normals pointing outward and no station.
/// A partial arc is centred on the first direction of `CylinderSpec::frame`.
pub(crate) fn cylinder(spec: &CylinderSpec) -> Shape {
    let [axis, u, v] = spec.frame();
    let at = |along: f64, a: f64, b: f64| -> [f64; 3] {
        std::array::from_fn(|i| spec.base[i] + along * axis[i] + a * u[i] + b * v[i])
    };
    let arc = spec.arc_degrees.clamp(0.0, 360.0).to_radians();
    let mut shape = Shape::default();
    for along in lattice(0.0, spec.length, spec.spacing) {
        for angle in lattice(-arc * 0.5, arc * 0.5, spec.spacing / spec.radius) {
            let (sin, cos) = angle.sin_cos();
            shape.push(
                at(along, spec.radius * cos, spec.radius * sin),
                std::array::from_fn(|i| cos * u[i] + sin * v[i]),
                u32::MAX,
            );
        }
    }
    if spec.caps {
        for (along, side) in [(0.0, -1.0), (spec.length, 1.0)] {
            for a in lattice(-spec.radius, spec.radius, spec.spacing) {
                for b in lattice(-spec.radius, spec.radius, spec.spacing) {
                    if a * a + b * b <= spec.radius * spec.radius {
                        shape.push(at(along, a, b), axis.map(|value| value * side), u32::MAX);
                    }
                }
            }
        }
    }
    shape
}

/// A horizontal rectangle from the origin at height zero, seen from above,
/// with an optional round hole given by its centre and radius.
pub(crate) fn plane_with_hole(
    size: [f64; 2],
    spacing: f64,
    hole: Option<([f64; 2], f64)>,
) -> Shape {
    let mut shape = Shape::default();
    for x in lattice(0.0, size[0], spacing) {
        for y in lattice(0.0, size[1], spacing) {
            let outside = hole.is_none_or(|(center, radius)| {
                (x - center[0]).powi(2) + (y - center[1]).powi(2) > radius * radius
            });
            if outside {
                shape.push([x, y, 0.0], [0.0, 0.0, 1.0], u32::MAX);
            }
        }
    }
    shape
}

/// A rectangle anywhere: from `origin`, `size[0]` along one unit direction
/// and `size[1]` along another that is square to it. It is seen from the
/// side that the cross product of the two points to.
pub(crate) fn rectangle(
    origin: [f64; 3],
    along: [f64; 3],
    up: [f64; 3],
    size: [f64; 2],
    spacing: f64,
) -> Shape {
    let normal = unit(cross(along, up)).expect("two directions that span a plane");
    let mut shape = Shape::default();
    for a in lattice(0.0, size[0], spacing) {
        for b in lattice(0.0, size[1], spacing) {
            shape.push(
                std::array::from_fn(|axis| origin[axis] + a * along[axis] + b * up[axis]),
                normal,
                u32::MAX,
            );
        }
    }
    shape
}

/// Points at random in a box: stray reflections that lie on no surface.
/// Their normals are random directions.
pub(crate) fn stray_points(bounds: Bounds, count: usize, seed: u64) -> Shape {
    let mut rng = Rng::new(seed);
    let mut shape = Shape::default();
    for _ in 0..count {
        let point = std::array::from_fn(|axis| {
            bounds.min[axis] + rng.unit() * (bounds.max[axis] - bounds.min[axis])
        });
        let normal = loop {
            if let Some(normal) = unit(std::array::from_fn(|_| rng.gaussian())) {
                break normal;
            }
        };
        shape.push(point, normal, u32::MAX);
    }
    shape
}

/// Sources of this size and larger get a preview cache in the user's cache
/// folder when they are opened; test files must stay below it.
const PREVIEW_CACHE_BYTES: usize = 16 * 1024 * 1024;

/// Write points to a file that every reader returns exactly as given, in the
/// same order: coordinates as doubles, with colour, intensity and
/// classification when the first point has them.
pub(crate) fn write_cloud(points: &[Point], path: &Path) {
    let first = points.first().expect("a cloud needs a point");
    let (rgb, intensity, classification) = (
        first.rgb.is_some(),
        first.intensity.is_some(),
        first.classification.is_some(),
    );
    let mut bytes = Vec::with_capacity(points.len() * 30 + 256);
    write!(
        bytes,
        "ply\nformat binary_little_endian 1.0\nelement vertex {}\nproperty double x\nproperty double y\nproperty double z\n",
        points.len()
    )
    .unwrap();
    if rgb {
        bytes.extend_from_slice(b"property uchar red\nproperty uchar green\nproperty uchar blue\n");
    }
    if intensity {
        bytes.extend_from_slice(b"property ushort intensity\n");
    }
    if classification {
        bytes.extend_from_slice(b"property uchar classification\n");
    }
    bytes.extend_from_slice(b"end_header\n");
    for point in points {
        assert_eq!(
            (
                point.rgb.is_some(),
                point.intensity.is_some(),
                point.classification.is_some()
            ),
            (rgb, intensity, classification),
            "every point of a test cloud carries the same attributes"
        );
        for value in point.xyz {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        if let Some(rgb) = point.rgb {
            bytes.extend_from_slice(&rgb);
        }
        if let Some(intensity) = point.intensity {
            bytes.extend_from_slice(&intensity.to_le_bytes());
        }
        if let Some(classification) = point.classification {
            bytes.push(classification);
        }
    }
    assert!(
        bytes.len() < PREVIEW_CACHE_BYTES,
        "a test cloud of this size would leave a preview cache behind"
    );
    std::fs::write(path, bytes).unwrap();
}

/// A cloud in a temporary file with its octree index. The file and the
/// index are removed when this is dropped.
pub(crate) struct IndexedCloud {
    pub(crate) cloud: PointCloud,
    pub(crate) index: OctreeIndex,
    // Dropped last: the index lives in this directory.
    _directory: tempfile::TempDir,
}

/// Write points to a temporary file, open it and build its octree index
/// with leaves of at most `leaf_points` points. Source ordinals are the
/// positions in `points`.
pub(crate) fn indexed_cloud(points: &[Point], leaf_points: u64) -> IndexedCloud {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cloud.ply");
    write_cloud(points, &path);
    let cloud = open(&path, 1_000).unwrap();
    let index = OctreeIndex::build(
        &cloud,
        IndexConfig {
            leaf_points,
            preview_points: leaf_points.clamp(1, 2_048) as usize,
            max_depth: 12,
            scratch_dir: Some(directory.path().to_path_buf()),
        },
    )
    .unwrap();
    IndexedCloud {
        cloud,
        index,
        _directory: directory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_fit::fit_plane;
    use crate::visit_points;

    fn length(v: [f64; 3]) -> f64 {
        dot(v, v).sqrt()
    }

    #[test]
    fn generator_is_deterministic_and_well_spread() {
        let draw = |seed| {
            let mut rng = Rng::new(seed);
            (0..5).map(|_| rng.next_u64()).collect::<Vec<_>>()
        };
        assert_eq!(draw(1), draw(1));
        assert_ne!(draw(1), draw(2));
        let mut rng = Rng::new(42);
        let uniform: Vec<f64> = (0..100_000).map(|_| rng.unit()).collect();
        assert!(uniform.iter().all(|value| (0.0..1.0).contains(value)));
        let mean = uniform.iter().sum::<f64>() / uniform.len() as f64;
        assert!((mean - 0.5).abs() < 0.005);
        let normal: Vec<f64> = (0..100_000).map(|_| rng.gaussian()).collect();
        let mean = normal.iter().sum::<f64>() / normal.len() as f64;
        let variance = normal
            .iter()
            .map(|value| (value - mean).powi(2))
            .sum::<f64>()
            / normal.len() as f64;
        assert!(mean.abs() < 0.01 && (variance - 1.0).abs() < 0.02);
        assert!(normal.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn room_has_six_faces_of_the_stated_size_seen_from_inside() {
        let spec = RoomSpec {
            spacing: 0.015,
            ..RoomSpec::default()
        };
        let room = box_room(&spec);
        // Area 60.4 m2 at one point per 1.5 cm in both directions.
        let expected = 60.4 / (0.015 * 0.015);
        assert!((room.points.len() as f64 - expected).abs() < 0.01 * expected);
        assert_eq!(room.stations, [[1.3, 1.1, 1.2]]);
        assert!(room.station_of.iter().all(|station| *station == 0));
        let bounds = room.bounds();
        assert_eq!((bounds.min, bounds.max), ([0.0; 3], [4.0, 3.0, 2.6]));
        for (point, normal) in room.points.iter().zip(&room.normals) {
            assert!((length(*normal) - 1.0).abs() < 1e-12);
            // Every face looks at the station, and every point is on a face.
            assert!(dot(*normal, difference(spec.station, *point)) > 0.0);
            let on_face = (0..3)
                .filter(|axis| point[*axis] == 0.0 || point[*axis] == spec.size[*axis])
                .count();
            assert_eq!(on_face, 1);
        }
        // Points per face follow its area.
        let floor = room.points.iter().filter(|point| point[2] == 0.0).count();
        let east = room.points.iter().filter(|point| point[0] == 4.0).count();
        assert!((floor as f64 - 12.0 / 0.000225).abs() < 0.01 * 12.0 / 0.000225);
        assert!((east as f64 - 7.8 / 0.000225).abs() < 0.01 * 7.8 / 0.000225);
        assert_eq!(box_room(&spec).points, room.points);
    }

    #[test]
    fn room_openings_are_free_of_points_on_both_faces_and_have_reveals() {
        let spec = RoomSpec {
            wall_thickness: Some(0.1),
            openings: vec![
                Opening::door(Wall::South, 1.0),
                Opening::window(Wall::East, 0.8),
            ],
            ..RoomSpec::default()
        };
        let room = box_room(&spec);
        assert_eq!(room.stations.len(), 5);
        assert_eq!(room.stations[1], [2.0, -3.1, 1.2]);
        assert_eq!(room.stations[2], [7.1, 1.5, 1.2]);
        let near = |value: f64, target: f64| (value - target).abs() < 1e-9;
        // Nothing on either face of the south wall inside the door opening.
        assert!(!room.points.iter().any(|point| {
            (near(point[1], 0.0) || near(point[1], -0.1))
                && point[0] > 1.01
                && point[0] < 1.89
                && point[2] < 2.09
        }));
        // Both faces are there beside and above it.
        for y in [0.0, -0.1] {
            assert!(room
                .points
                .iter()
                .any(|point| near(point[1], y) && point[0] < 0.99 && point[2] < 2.0));
            assert!(room.points.iter().any(|point| near(point[1], y)
                && point[0] > 1.2
                && point[0] < 1.7
                && point[2] > 2.11));
        }
        // The window in the east wall: free between 0.9 and 2.1 m only.
        let in_window = |point: &&[f64; 3]| {
            (near(point[0], 4.0) || near(point[0], 4.1)) && point[1] > 0.81 && point[1] < 1.99
        };
        assert!(!room
            .points
            .iter()
            .filter(in_window)
            .any(|point| point[2] > 0.91 && point[2] < 2.09));
        assert!(room
            .points
            .iter()
            .filter(in_window)
            .any(|point| point[2] < 0.89));
        assert!(room
            .points
            .iter()
            .filter(in_window)
            .any(|point| point[2] > 2.11));
        // Door jambs at x = 1.0 and 1.9 through the wall, facing each other.
        for (x, facing) in [(1.0, 1.0), (1.9, -1.0)] {
            let jamb: Vec<usize> = (0..room.points.len())
                .filter(|index| {
                    let point = room.points[*index];
                    near(point[0], x) && point[1] > -0.1 && point[1] < 0.0
                })
                .collect();
            assert!(jamb.len() > 100);
            assert!(jamb
                .iter()
                .all(|index| room.normals[*index] == [facing, 0.0, 0.0]
                    && room.points[*index][2] < 2.1
                    && room.station_of[*index] == 0));
        }
        // The window has a sill and a head, the door only a head.
        let level = |z: f64, normal: [f64; 3]| {
            (0..room.points.len())
                .filter(|index| {
                    let point = room.points[*index];
                    near(point[2], z)
                        && (point[1] < 0.0 || point[0] > 4.0)
                        && room.normals[*index] == normal
                })
                .count()
        };
        assert!(level(0.9, [0.0, 0.0, 1.0]) > 100);
        assert!(level(2.1, [0.0, 0.0, -1.0]) > 200);
        // Outside faces reach past the corners and look away from the room.
        let outside: Vec<usize> = (0..room.points.len())
            .filter(|index| room.station_of[*index] > 0)
            .collect();
        assert!(outside.iter().all(|index| {
            let (point, normal) = (room.points[*index], room.normals[*index]);
            dot(
                normal,
                difference(room.stations[room.station_of[*index] as usize], point),
            ) > 0.0
                && dot(normal, difference(spec.station, point)) < 0.0
        }));
        let bounds = room.bounds();
        assert!(near(bounds.min[0], -0.1) && near(bounds.max[0], 4.1));
        assert!(near(bounds.min[1], -0.1) && near(bounds.max[1], 3.1));
        // Points come station by station.
        let ranges = room.station_ranges();
        assert_eq!(
            ranges.iter().map(|range| range.1).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
        assert_eq!(ranges[0].0, 0);
        assert_eq!(
            ranges[1].0 as usize,
            room.station_of.iter().filter(|s| **s == 0).count()
        );
    }

    #[test]
    fn room_slab_keeps_a_thin_slice_of_the_walls() {
        let spec = RoomSpec {
            spacing: 0.005,
            wall_thickness: Some(0.1),
            openings: vec![Opening::door(Wall::North, 2.0)],
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        };
        let room = box_room(&spec);
        assert!(room
            .points
            .iter()
            .all(|point| point[2] >= 1.0 && point[2] <= 1.1));
        // No floor or ceiling, and two faces per wall less the door.
        let inside = 2.0 * (4.0 + 3.0) - 0.9;
        let outside = 2.0 * (4.2 + 3.2) - 0.9;
        let jambs = 2.0 * 0.1;
        let expected = (inside + outside + jambs) * 0.1 / (0.005 * 0.005);
        assert!((room.points.len() as f64 - expected).abs() < 0.01 * expected);
        // Wall thickness is measured between the two faces.
        assert!(room.points.iter().any(|point| point[1] == 3.0));
        assert!(room
            .points
            .iter()
            .any(|point| (point[1] - 3.1).abs() < 1e-12));
        assert!(!room
            .points
            .iter()
            .any(|point| point[1] >= 3.0 && point[0] > 2.005 && point[0] < 2.895));
    }

    #[test]
    fn noise_moves_points_along_their_normal_by_the_stated_amount() {
        let plane = plane_with_hole([2.0, 2.0], 0.01, None);
        assert_eq!(plane.points.len(), 40_000);
        let gaussian = plane.clone().with_noise(Noise::Gaussian(0.003), 5);
        let fit = fit_plane(gaussian.points.iter().copied()).unwrap();
        assert!((fit.rms() - 0.003).abs() < 0.0001);
        assert!(gaussian
            .points
            .iter()
            .zip(&plane.points)
            .all(|(moved, point)| moved[0] == point[0] && moved[1] == point[1]));
        let uniform = plane.clone().with_noise(Noise::Uniform(0.002), 5);
        assert!(uniform.points.iter().all(|point| point[2].abs() <= 0.002));
        // A uniform spread of +/- a has a standard deviation of a / sqrt(3).
        let rms = fit_plane(uniform.points.iter().copied()).unwrap().rms();
        assert!((rms - 0.002 / 3f64.sqrt()).abs() < 0.00005);
        let again = plane.clone().with_noise(Noise::Gaussian(0.003), 5);
        assert_eq!(again.points, gaussian.points);
        let other = plane.with_noise(Noise::Gaussian(0.003), 6);
        assert_ne!(other.points, gaussian.points);
    }

    #[test]
    fn plane_hole_has_the_stated_radius() {
        let plane = plane_with_hole([2.0, 2.0], 0.01, Some(([1.0, 1.0], 0.2)));
        let removed = 40_000 - plane.points.len();
        let expected = std::f64::consts::PI * 0.04 / 0.0001;
        assert!((removed as f64 - expected).abs() < 0.02 * expected);
        assert!(plane
            .points
            .iter()
            .all(|point| (point[0] - 1.0).hypot(point[1] - 1.0) > 0.2 && point[2] == 0.0));
        assert!(plane
            .normals
            .iter()
            .all(|normal| *normal == [0.0, 0.0, 1.0]));
        assert!(plane.station_of.iter().all(|station| *station == u32::MAX));
        assert_eq!(plane.station_ranges(), [(0, u32::MAX)]);
    }

    #[test]
    fn sphere_points_lie_on_the_sphere_and_spread_evenly() {
        let center = [2.0, -1.0, 0.5];
        let ball = sphere(center, 1.0, 100_000);
        assert_eq!(ball.points.len(), 100_000);
        for (point, normal) in ball.points.iter().zip(&ball.normals) {
            let radial = difference(*point, center);
            assert!((length(radial) - 1.0).abs() < 1e-12);
            assert!(dot(radial, *normal) > 0.999_999);
        }
        // Each octant holds an eighth of the points.
        for octant in 0..8 {
            let inside = ball
                .points
                .iter()
                .filter(|point| {
                    (0..3).all(|axis| (point[axis] > center[axis]) == ((octant >> axis) & 1 == 1))
                })
                .count();
            assert!((inside as f64 - 12_500.0).abs() < 250.0, "{inside}");
        }
        // Seen from its centre, every point looks inward.
        let inside = ball.flipped().with_stations(&[center]);
        assert!(inside.station_of.iter().all(|station| *station == 0));
        assert!(inside
            .points
            .iter()
            .zip(&inside.normals)
            .all(|(point, normal)| dot(*normal, difference(center, *point)) > 0.999_999));
    }

    #[test]
    fn cylinder_column_and_pipe_have_their_radius_length_and_arc() {
        let column = cylinder(&CylinderSpec {
            arc_degrees: 180.0,
            ..CylinderSpec::column([1.0, 2.0, 0.0], 0.15, 2.6)
        });
        let mut angles = (f64::INFINITY, f64::NEG_INFINITY);
        for (point, normal) in column.points.iter().zip(&column.normals) {
            let radial = [point[0] - 1.0, point[1] - 2.0, 0.0];
            assert!((length(radial) - 0.15).abs() < 1e-12);
            assert!(point[2] > 0.0 && point[2] < 2.6);
            assert!((dot(radial, *normal) - 0.15).abs() < 1e-12);
            // The arc is centred on the first cross direction.
            let [_, u, v] = CylinderSpec::column([0.0; 3], 0.15, 2.6).frame();
            let angle = dot(radial, v).atan2(dot(radial, u)).to_degrees();
            angles = (angles.0.min(angle), angles.1.max(angle));
        }
        assert!(angles.0 < -85.0 && angles.0 >= -90.0);
        assert!(angles.1 > 85.0 && angles.1 <= 90.0);
        let expected = std::f64::consts::PI * 0.15 * 2.6 / 0.0004;
        assert!((column.points.len() as f64 - expected).abs() < 0.03 * expected);

        let spec = CylinderSpec {
            caps: true,
            ..CylinderSpec::pipe([0.0, 0.0, 2.4], [3.0, 4.0, 0.0], 0.05, 3.0)
        };
        let pipe = cylinder(&spec);
        let axis = [0.6, 0.8, 0.0];
        let mut caps = 0;
        for (point, normal) in pipe.points.iter().zip(&pipe.normals) {
            let offset = difference(*point, spec.base);
            let along = dot(offset, axis);
            let radial = difference(offset, axis.map(|value| value * along));
            assert!((length(*normal) - 1.0).abs() < 1e-12);
            if dot(*normal, axis).abs() > 0.5 {
                // An end face: flat, within the radius, looking outward.
                caps += 1;
                assert!(along.abs() < 1e-9 || (along - 3.0).abs() < 1e-9);
                assert!(length(radial) <= 0.05 + 1e-12);
                assert_eq!(dot(*normal, axis) > 0.0, along > 1.5);
            } else {
                assert!((length(radial) - 0.05).abs() < 1e-9);
                assert!(along > 0.0 && along < 3.0);
                assert!((dot(radial, *normal) - 0.05).abs() < 1e-9);
            }
        }
        let expected = 2.0 * std::f64::consts::PI * 0.0025 / 0.0001;
        assert!((caps as f64 - expected).abs() < 0.1 * expected);
    }

    #[test]
    fn stations_take_the_points_they_face_in_file_order() {
        let spec = CylinderSpec {
            caps: true,
            ..CylinderSpec::column([0.0, 0.0, 0.0], 0.5, 2.0)
        };
        let stations = spec.stations_around(3.0);
        assert_eq!(stations.len(), 6);
        assert_eq!(stations[4], [0.0, 0.0, 5.0]);
        assert_eq!(stations[5], [0.0, 0.0, -3.0]);
        for station in &stations[..4] {
            assert!((station[0].hypot(station[1]) - 3.0).abs() < 1e-12);
            assert_eq!(station[2], 1.0);
        }
        let scan = cylinder(&spec).with_stations(&stations);
        assert_eq!(scan.stations, stations);
        // Every station sees its points from the front.
        for index in 0..scan.points.len() {
            let station = scan.stations[scan.station_of[index] as usize];
            assert!(dot(scan.normals[index], difference(station, scan.points[index])) > 0.0);
        }
        // The top goes to the station above, the bottom to the one below.
        for index in 0..scan.points.len() {
            if scan.normals[index] == [0.0, 0.0, 1.0] {
                assert_eq!(scan.station_of[index], 4);
            }
            if scan.normals[index] == [0.0, 0.0, -1.0] {
                assert_eq!(scan.station_of[index], 5);
            }
        }
        let ranges = scan.station_ranges();
        assert_eq!(
            ranges.iter().map(|range| range.1).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        // The four stations around share the mantle about equally.
        let mantle = (ranges[4].0 - ranges[0].0) as f64;
        for pair in ranges[..5].windows(2) {
            assert!(((pair[1].0 - pair[0].0) as f64 - mantle / 4.0).abs() < 0.05 * mantle);
        }
        // The same points as before, only reordered.
        let mut before = cylinder(&spec).points;
        let mut after = scan.points.clone();
        let order = |a: &[f64; 3], b: &[f64; 3]| {
            a[0].total_cmp(&b[0])
                .then(a[1].total_cmp(&b[1]))
                .then(a[2].total_cmp(&b[2]))
        };
        before.sort_by(order);
        after.sort_by(order);
        assert_eq!(before, after);
        assert!(cylinder(&spec)
            .with_stations(&[])
            .station_of
            .iter()
            .all(|station| *station == u32::MAX));
    }

    #[test]
    fn a_shape_can_be_turned_moved_and_joined_with_another() {
        let room = box_room(&RoomSpec {
            spacing: 0.1,
            ..RoomSpec::default()
        });
        let moved = room.clone().transformed(17.3, [207_000.0, 474_000.0, 2.0]);
        let (sin, cos) = 17.3f64.to_radians().sin_cos();
        for index in [0, room.points.len() / 2, room.points.len() - 1] {
            let (from, to) = (room.points[index], moved.points[index]);
            assert!((to[0] - (cos * from[0] - sin * from[1] + 207_000.0)).abs() < 1e-9);
            assert!((to[1] - (sin * from[0] + cos * from[1] + 474_000.0)).abs() < 1e-9);
            assert_eq!(to[2], from[2] + 2.0);
            // Still seen from the station, which moved along.
            assert!(dot(moved.normals[index], difference(moved.stations[0], to)) > 0.0);
        }
        // Distances are kept.
        let span = |shape: &Shape| length(difference(shape.points[0], shape.points[57]));
        assert!((span(&room) - span(&moved)).abs() < 1e-9);

        let column = cylinder(&CylinderSpec::column([2.0, 1.5, 0.0], 0.15, 2.6))
            .with_stations(&[[1.0, 1.0, 1.0]]);
        let (room_points, column_points) = (room.points.len(), column.points.len());
        let both = room.merged(column);
        assert_eq!(both.points.len(), room_points + column_points);
        assert_eq!(both.stations.len(), 2);
        assert_eq!(both.station_of[room_points - 1], 0);
        assert_eq!(both.station_of[room_points], 1);
        assert_eq!(both.station_ranges(), [(0, 0), (room_points as u64, 1)]);
        let loose = both.merged(plane_with_hole([1.0, 1.0], 0.5, None));
        assert_eq!(*loose.station_of.last().unwrap(), u32::MAX);
    }

    #[test]
    fn stray_points_stay_in_their_box() {
        let bounds = Bounds {
            min: [1.0, 2.0, 3.0],
            max: [2.0, 4.0, 3.5],
        };
        let stray = stray_points(bounds, 1_000, 9);
        assert_eq!(stray.points.len(), 1_000);
        assert!(stray.points.iter().all(|point| {
            (0..3).all(|axis| point[axis] >= bounds.min[axis] && point[axis] < bounds.max[axis])
        }));
        assert!(stray
            .normals
            .iter()
            .all(|normal| (length(*normal) - 1.0).abs() < 1e-12));
        let found = stray.bounds();
        assert!(found.min[0] < 1.01 && found.max[1] > 3.98);
        assert_eq!(stray_points(bounds, 1_000, 9).points, stray.points);
    }

    #[test]
    fn a_rectangle_lies_where_it_is_put_and_scattering_keeps_it_in_its_plane() {
        // A wall face of 2 by 1 m at x = 3 that runs towards -y, seen from
        // the side of -x.
        let wall = rectangle(
            [3.0, 5.0, 0.5],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.0, 1.0],
            0.1,
        );
        assert_eq!(wall.points.len(), 20 * 10);
        assert!(wall
            .normals
            .iter()
            .all(|normal| *normal == [-1.0, 0.0, 0.0]));
        let bounds = wall.bounds();
        assert_eq!((bounds.min[0], bounds.max[0]), (3.0, 3.0));
        assert!((bounds.min[1] - 3.05).abs() < 1e-12 && (bounds.max[1] - 4.95).abs() < 1e-12);
        assert!((bounds.min[2] - 0.55).abs() < 1e-12 && (bounds.max[2] - 1.45).abs() < 1e-12);
        // Scattered by up to 4 cm: off the lattice, still at x = 3.
        let scattered = wall.clone().scattered(0.04, 3);
        let mut moved = 0;
        for (before, after) in wall.points.iter().zip(&scattered.points) {
            assert_eq!(after[0], 3.0);
            assert!((after[1] - before[1]).abs() <= 0.04 && (after[2] - before[2]).abs() <= 0.04);
            moved += usize::from(after != before);
        }
        assert_eq!(moved, 200);
        assert_eq!(wall.scattered(0.04, 3).points, scattered.points);
    }

    #[test]
    fn written_cloud_reads_back_exactly_and_is_indexed_by_ordinal() {
        let shape = sphere([207_000.123_456_789, 474_000.5, 10.0], 1.0, 5_000);
        let mut points = shape.cloud_points();
        for (ordinal, point) in points.iter_mut().enumerate() {
            point.intensity = Some(ordinal as u16);
            point.classification = Some((ordinal % 7) as u8);
        }
        let indexed = indexed_cloud(&points, 64);
        assert_eq!(indexed.cloud.total_points, 5_000);
        assert!(indexed.cloud.has_rgb && indexed.cloud.has_classification);
        let mut read = Vec::new();
        visit_points(&indexed.cloud.path, &mut |point| {
            read.push(point);
            Ok(())
        })
        .unwrap();
        assert_eq!(read.len(), points.len());
        for (read, written) in read.iter().zip(&points) {
            assert_eq!(read.xyz, written.xyz);
            assert_eq!(read.rgb, written.rgb);
            assert_eq!(read.intensity, written.intensity);
            assert_eq!(read.classification, written.classification);
        }
        assert!(!indexed.index.root.is_leaf());
        let mut seen = vec![false; points.len()];
        indexed
            .index
            .visit_intersecting(
                |_| true,
                |record| {
                    assert_eq!(record.point.xyz, points[record.ordinal as usize].xyz);
                    assert!(!std::mem::replace(&mut seen[record.ordinal as usize], true));
                    Ok(())
                },
            )
            .unwrap();
        assert!(seen.iter().all(|seen| *seen));
        // Nothing is left behind once the cloud is dropped.
        let directory = indexed.cloud.path.parent().unwrap().to_path_buf();
        assert!(directory.exists());
        drop(indexed);
        assert!(!directory.exists());
    }
}
