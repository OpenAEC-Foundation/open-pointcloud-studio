//! The interior scene: two rooms joined by a doorway, scanned from one
//! station in each room.
//!
//! The first room has a window with a radiator under it, a round concrete
//! column, a tall cabinet and a low sideboard, on an oak floor. The second
//! has a window in its end wall, a table with four chairs under a pendant
//! lamp, a print on a green wall, a low shelf and a tiled floor; its door
//! stands open against the jamb. Walls are 30 cm thick outside and 12 cm
//! between the rooms.
//!
//! Both the points and the photos are made by tracing rays through the same
//! geometry: a scanner at each station sweeps columns from 85 degrees below
//! the horizon to the zenith, and its panorama is rendered from the same
//! position and heading. Colours are those of materials lit by four lamps,
//! the sky through the windows and a low sun from the south-east, with
//! shadows and some light taken away in corners.

use std::f64::consts::{PI, TAU};
use std::path::{Path, PathBuf};

use e57::{
    E57Writer, ImageFormat, Quaternion, Record, RecordDataType, RecordName, RecordValue,
    SphericalImageProperties, Transform, Translation,
};
use pointcloud_core::{FilePhoto, PhotoProjection, ScanImageFormat};
use rayon::prelude::*;

use super::common::{
    add, along, class, cross, dot, fbm2, fbm3, frame, hash, length, mix, noise2, noise3, normalize,
    scale, smoothstep, splitmix, sub, Rgb, Rng, Sample, V3,
};
use super::{las_writer, write_samples, Failure};

const SEED: u64 = 0x0400_5ce7_0e57;
const HEIGHT: f64 = 2.7;
const WALL: f64 = 0.3;
const ROOM_1: [[f64; 2]; 2] = [[0.0, 5.4], [0.0, 4.2]];
const PARTITION: [f64; 2] = [5.4, 5.52];
const ROOM_2: [[f64; 2]; 2] = [[5.52, 9.72], [0.0, 4.2]];
const DOORWAY: [f64; 2] = [2.75, 3.65];
const DOOR_HEIGHT: f64 = 2.1;
/// The window in the south wall of the first room, across and up.
const SOUTH_WINDOW: [[f64; 2]; 2] = [[1.5, 3.3], [0.85, 2.25]];
/// The window in the east wall of the second room.
const EAST_WINDOW: [[f64; 2]; 2] = [[1.2, 2.6], [0.85, 2.25]];
const COLUMN: [f64; 2] = [3.95, 2.55];
const COLUMN_RADIUS: f64 = 0.2;

/// A scanner station: where it stands and where its zero direction points,
/// in radians anticlockwise from east.
#[derive(Clone, Copy, Debug)]
pub struct Station {
    pub name: &'static str,
    pub position: V3,
    pub heading: f64,
}

pub const STATIONS: [Station; 2] = [
    Station {
        name: "Station 1",
        position: [2.35, 1.95, 1.52],
        heading: 0.436,
    },
    Station {
        name: "Station 2",
        position: [7.95, 3.15, 1.48],
        heading: -1.92,
    },
];

impl Station {
    /// The scanner's X, Y and Z axes in the rooms.
    pub fn axes(&self) -> [V3; 3] {
        let (sin, cos) = self.heading.sin_cos();
        [[cos, sin, 0.0], [-sin, cos, 0.0], [0.0, 0.0, 1.0]]
    }

    /// A point of the scanner's own frame in the coordinates of the rooms.
    pub fn registered(&self, local: V3) -> V3 {
        let [x, y, z] = self.axes();
        add(
            self.position,
            add(
                add(scale(x, local[0]), scale(y, local[1])),
                scale(z, local[2]),
            ),
        )
    }

    pub fn transform(&self) -> Transform {
        let half = self.heading / 2.0;
        Transform {
            rotation: Quaternion {
                w: half.cos(),
                x: 0.0,
                y: 0.0,
                z: half.sin(),
            },
            translation: Translation {
                x: self.position[0],
                y: self.position[1],
                z: self.position[2],
            },
        }
    }
}

/// How finely the scenes are scanned and photographed.
#[derive(Clone, Copy, Debug)]
pub struct Detail {
    /// Columns of a full turn of the scanner; rows have the same angle.
    pub columns: usize,
    pub panorama: [u32; 2],
}

pub const FULL: Detail = Detail {
    columns: 2600,
    panorama: [4096, 2048],
};

/// The lowest elevation the scanner measures; below it stands its tripod,
/// which leaves a hole of about 25 cm across in the floor.
const LOWEST: f64 = -85.0 * PI / 180.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Material {
    Paint,
    Accent,
    Ceiling,
    Floor,
    Concrete,
    Skirting,
    Frame,
    Door,
    Sill,
    Radiator,
    Cabinet,
    Plinth,
    Handle,
    TableTop,
    Metal,
    Fabric,
    Lamp,
    Picture,
}

impl Material {
    /// How strongly the material returns the laser, from 0 to 1.
    fn reflectivity(self, p: V3) -> f64 {
        match self {
            Material::Paint => 0.8,
            Material::Accent => 0.55,
            Material::Ceiling => 0.82,
            Material::Floor if p[0] > PARTITION[1] => 0.6,
            Material::Floor => 0.45,
            Material::Concrete => 0.5,
            Material::Skirting | Material::Frame => 0.82,
            Material::Door => 0.78,
            Material::Sill => 0.7,
            Material::Radiator => 0.75,
            Material::Cabinet | Material::TableTop => 0.48,
            Material::Plinth => 0.2,
            Material::Handle => 0.6,
            Material::Metal => 0.08,
            Material::Fabric => 0.25,
            Material::Lamp => 0.95,
            Material::Picture => 0.4,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    Cuboid {
        min: V3,
        max: V3,
    },
    /// An upright cylinder with flat ends.
    Column {
        centre: [f64; 2],
        radius: f64,
        z: [f64; 2],
    },
}

impl Shape {
    fn bounds(&self) -> (V3, V3) {
        match *self {
            Shape::Cuboid { min, max } => (min, max),
            Shape::Column { centre, radius, z } => (
                [centre[0] - radius, centre[1] - radius, z[0]],
                [centre[0] + radius, centre[1] + radius, z[1]],
            ),
        }
    }

    /// Distance along a ray of unit `dir` to where it enters the shape, with
    /// the outward normal there. A ray that starts inside meets nothing.
    fn hit(&self, origin: V3, dir: V3, inverse: V3, limit: f64) -> Option<(f64, V3)> {
        const NEAR: f64 = 1e-7;
        match *self {
            Shape::Cuboid { min, max } => {
                let (mut near, mut far, mut axis) = (f64::NEG_INFINITY, f64::INFINITY, 0);
                for a in 0..3 {
                    let t1 = (min[a] - origin[a]) * inverse[a];
                    let t2 = (max[a] - origin[a]) * inverse[a];
                    let (low, high) = if t1 < t2 { (t1, t2) } else { (t2, t1) };
                    if low > near {
                        near = low;
                        axis = a;
                    }
                    far = far.min(high);
                }
                if near > far || near < NEAR || near > limit {
                    return None;
                }
                let mut normal = [0.0; 3];
                normal[axis] = -dir[axis].signum();
                Some((near, normal))
            }
            Shape::Column { centre, radius, z } => {
                let (ox, oy) = (origin[0] - centre[0], origin[1] - centre[1]);
                let mut best: Option<(f64, V3)> = None;
                let a = dir[0] * dir[0] + dir[1] * dir[1];
                if a > 1e-15 {
                    let b = ox * dir[0] + oy * dir[1];
                    let c = ox * ox + oy * oy - radius * radius;
                    let disc = b * b - a * c;
                    if c > 0.0 && disc >= 0.0 {
                        let t = (-b - disc.sqrt()) / a;
                        let height = origin[2] + dir[2] * t;
                        if t > NEAR && t < limit && height >= z[0] && height <= z[1] {
                            let normal =
                                [(ox + dir[0] * t) / radius, (oy + dir[1] * t) / radius, 0.0];
                            best = Some((t, normal));
                        }
                    }
                }
                if dir[2].abs() > 1e-15 {
                    for (level, up) in [(z[0], -1.0), (z[1], 1.0)] {
                        let t = (level - origin[2]) / dir[2];
                        if t > NEAR && t < best.map_or(limit, |(t, _)| t) && up * dir[2] < 0.0 {
                            let (x, y) = (ox + dir[0] * t, oy + dir[1] * t);
                            if x * x + y * y <= radius * radius {
                                best = Some((t, [0.0, 0.0, up]));
                            }
                        }
                    }
                }
                best
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Part {
    shape: Shape,
    material: Material,
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    t: f64,
    point: V3,
    normal: V3,
    material: Material,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    min: V3,
    max: V3,
    /// For a leaf the first of `count` parts in `order`; otherwise the
    /// left child, with the right one after it.
    first: usize,
    count: usize,
}

#[derive(Clone, Copy)]
struct Light {
    position: V3,
    power: f64,
    /// How strongly it shines sideways rather than down, 0 to 1.
    spread: f64,
}

/// A window as a light: its opening on the inner face of the wall.
#[derive(Clone, Copy)]
struct Window {
    corner: V3,
    across: V3,
    up: V3,
    /// Into the room.
    normal: V3,
}

pub struct Scene {
    parts: Vec<Part>,
    nodes: Vec<Node>,
    order: Vec<usize>,
    lights: Vec<Light>,
    windows: Vec<Window>,
    sun: V3,
}

fn ray_meets_box(origin: V3, inverse: V3, min: V3, max: V3, limit: f64) -> bool {
    let (mut near, mut far) = (0.0f64, limit);
    for a in 0..3 {
        let t1 = (min[a] - origin[a]) * inverse[a];
        let t2 = (max[a] - origin[a]) * inverse[a];
        near = near.max(t1.min(t2));
        far = far.min(t1.max(t2));
    }
    near <= far
}

fn inverse(dir: V3) -> V3 {
    dir.map(|value| {
        let safe = if value.abs() < 1e-15 {
            1e-15f64.copysign(value)
        } else {
            value
        };
        1.0 / safe
    })
}

/// Which wall a window or a radiator is set in: the axis along it, the axis
/// through it, where its inner face is and which way is out.
#[derive(Clone, Copy)]
struct Face {
    along: usize,
    through: usize,
    inner: f64,
    out: f64,
}

impl Face {
    /// A box `a` along the wall, `d` metres out from its inner face (less
    /// than zero inside the room) and `z` up.
    fn cuboid(&self, a: [f64; 2], d: [f64; 2], z: [f64; 2]) -> Shape {
        let mut min = [0.0, 0.0, z[0]];
        let mut max = [0.0, 0.0, z[1]];
        min[self.along] = a[0];
        max[self.along] = a[1];
        let (p, q) = (self.inner + self.out * d[0], self.inner + self.out * d[1]);
        min[self.through] = p.min(q);
        max[self.through] = p.max(q);
        Shape::Cuboid { min, max }
    }
}

const SOUTH: Face = Face {
    along: 0,
    through: 1,
    inner: 0.0,
    out: -1.0,
};
const EAST: Face = Face {
    along: 1,
    through: 0,
    inner: ROOM_2[0][1],
    out: 1.0,
};

impl Scene {
    pub fn new() -> Self {
        let mut parts = Vec::new();
        let mut put = |shape: Shape, material: Material| parts.push(Part { shape, material });
        let cuboid = |min: V3, max: V3| Shape::Cuboid { min, max };
        let x_out = [-WALL, ROOM_2[0][1] + WALL];
        let y_out = [-WALL, ROOM_1[1][1] + WALL];
        // Floor and ceiling slabs.
        put(
            cuboid([x_out[0], y_out[0], -0.25], [x_out[1], y_out[1], 0.0]),
            Material::Floor,
        );
        put(
            cuboid(
                [x_out[0], y_out[0], HEIGHT],
                [x_out[1], y_out[1], HEIGHT + 0.25],
            ),
            Material::Ceiling,
        );
        // Outer walls, with the openings of the windows left out.
        let mut wall =
            |min: V3, max: V3, along: usize, openings: &[[[f64; 2]; 2]], material: Material| {
                for shape in wall_with_openings(min, max, along, openings) {
                    put(shape, material);
                }
            };
        wall(
            [x_out[0], -WALL, 0.0],
            [x_out[1], 0.0, HEIGHT],
            0,
            &[SOUTH_WINDOW],
            Material::Paint,
        );
        wall(
            [ROOM_2[0][1], y_out[0], 0.0],
            [x_out[1], y_out[1], HEIGHT],
            1,
            &[EAST_WINDOW],
            Material::Paint,
        );
        wall(
            [x_out[0], y_out[0], 0.0],
            [0.0, y_out[1], HEIGHT],
            1,
            &[],
            Material::Paint,
        );
        wall(
            [x_out[0], ROOM_1[1][1], 0.0],
            [5.46, y_out[1], HEIGHT],
            0,
            &[],
            Material::Paint,
        );
        wall(
            [5.46, ROOM_1[1][1], 0.0],
            [x_out[1], y_out[1], HEIGHT],
            0,
            &[],
            Material::Accent,
        );
        // The wall between the rooms, with its doorway.
        wall(
            [PARTITION[0], 0.0, 0.0],
            [PARTITION[1], ROOM_1[1][1], HEIGHT],
            1,
            &[[DOORWAY, [0.0, DOOR_HEIGHT]]],
            Material::Paint,
        );
        drop(wall);
        // Window frames, sashes, sills and radiators.
        for (face, [across, up]) in [(SOUTH, SOUTH_WINDOW), (EAST, EAST_WINDOW)] {
            for shape in window_frame(face, across, up) {
                put(shape, Material::Frame);
            }
            put(
                face.cuboid(
                    [across[0] - 0.04, across[1] + 0.04],
                    [-0.035, 0.13],
                    [up[0], up[0] + 0.025],
                ),
                Material::Sill,
            );
            let radiator = [across[0] + 0.2, across[1] - 0.2];
            put(
                face.cuboid(radiator, [-0.13, -0.04], [0.14, 0.74]),
                Material::Radiator,
            );
            for a in [radiator[0] + 0.06, radiator[1] - 0.06] {
                let mut centre = [0.0; 2];
                centre[face.along] = a;
                centre[face.through] = face.inner - face.out * 0.085;
                put(
                    Shape::Column {
                        centre,
                        radius: 0.009,
                        z: [0.0, 0.14],
                    },
                    Material::Radiator,
                );
            }
        }
        // Skirting boards.
        let skirting = |x: [f64; 2], y: [f64; 2]| cuboid([x[0], y[0], 0.0], [x[1], y[1], 0.07]);
        for [x0, x1] in [ROOM_1[0], ROOM_2[0]] {
            put(skirting([x0, x1], [0.0, 0.012]), Material::Skirting);
            put(skirting([x0, x1], [4.188, 4.2]), Material::Skirting);
        }
        put(skirting([0.0, 0.012], ROOM_1[1]), Material::Skirting);
        put(skirting([9.708, 9.72], ROOM_2[1]), Material::Skirting);
        for (x, y) in [
            ([5.388, 5.4], [0.0, DOORWAY[0]]),
            ([5.388, 5.4], [DOORWAY[1], 4.2]),
            ([5.52, 5.532], [0.0, DOORWAY[0]]),
            ([5.52, 5.532], [DOORWAY[1], 4.2]),
        ] {
            put(skirting(x, y), Material::Skirting);
        }
        // The lining and architraves of the doorway, and the open door.
        let lining = [PARTITION[0] - 0.01, PARTITION[1] + 0.01];
        put(
            cuboid(
                [lining[0], DOORWAY[0], 0.0],
                [lining[1], DOORWAY[0] + 0.02, DOOR_HEIGHT],
            ),
            Material::Frame,
        );
        put(
            cuboid(
                [lining[0], DOORWAY[1] - 0.02, 0.0],
                [lining[1], DOORWAY[1], DOOR_HEIGHT],
            ),
            Material::Frame,
        );
        put(
            cuboid(
                [lining[0], DOORWAY[0], DOOR_HEIGHT - 0.02],
                [lining[1], DOORWAY[1], DOOR_HEIGHT],
            ),
            Material::Frame,
        );
        for x in [
            [PARTITION[0] - 0.015, PARTITION[0]],
            [PARTITION[1], PARTITION[1] + 0.015],
        ] {
            put(
                cuboid(
                    [x[0], DOORWAY[0] - 0.07, 0.0],
                    [x[1], DOORWAY[0], DOOR_HEIGHT + 0.07],
                ),
                Material::Frame,
            );
            put(
                cuboid(
                    [x[0], DOORWAY[1], 0.0],
                    [x[1], DOORWAY[1] + 0.07, DOOR_HEIGHT + 0.07],
                ),
                Material::Frame,
            );
            put(
                cuboid(
                    [x[0], DOORWAY[0] - 0.07, DOOR_HEIGHT],
                    [x[1], DOORWAY[1] + 0.07, DOOR_HEIGHT + 0.07],
                ),
                Material::Frame,
            );
        }
        let leaf_y = [DOORWAY[1] - 0.065, DOORWAY[1] - 0.025];
        put(
            cuboid(
                [PARTITION[1] + 0.015, leaf_y[0], 0.01],
                [PARTITION[1] + 0.875, leaf_y[1], DOOR_HEIGHT - 0.02],
            ),
            Material::Door,
        );
        for y in [[leaf_y[0] - 0.03, leaf_y[0]], [leaf_y[1], leaf_y[1] + 0.03]] {
            put(
                cuboid([6.27, y[0], 1.03], [6.36, y[1], 1.055]),
                Material::Handle,
            );
        }
        // The round column.
        put(
            Shape::Column {
                centre: COLUMN,
                radius: COLUMN_RADIUS,
                z: [0.0, HEIGHT],
            },
            Material::Concrete,
        );
        // A tall cabinet against the north wall and a sideboard against the
        // west wall of the first room.
        put(
            cuboid([0.35, 3.75, 0.08], [1.55, 4.2, 2.05]),
            Material::Cabinet,
        );
        put(
            cuboid([0.38, 3.78, 0.0], [1.52, 4.2, 0.08]),
            Material::Plinth,
        );
        for x in [[0.905, 0.92], [0.98, 0.995]] {
            put(
                cuboid([x[0], 3.72, 0.95], [x[1], 3.75, 1.35]),
                Material::Handle,
            );
        }
        put(
            cuboid([0.0, 1.05, 0.1], [0.45, 2.85, 0.78]),
            Material::Cabinet,
        );
        put(
            cuboid([0.0, 1.08, 0.0], [0.42, 2.82, 0.1]),
            Material::Plinth,
        );
        // A low shelf in the second room.
        put(
            cuboid([6.15, 0.0, 0.0], [7.75, 0.38, 0.86]),
            Material::Cabinet,
        );
        // The table and its chairs.
        put(
            cuboid([6.9, 1.3, 0.72], [8.5, 2.2, 0.75]),
            Material::TableTop,
        );
        for x in [[6.96, 7.02], [8.38, 8.44]] {
            for y in [[1.36, 1.42], [2.08, 2.14]] {
                put(
                    cuboid([x[0], y[0], 0.0], [x[1], y[1], 0.72]),
                    Material::Metal,
                );
            }
        }
        for (x, y, facing_north) in [
            (7.3, 1.05, true),
            (8.12, 0.96, true),
            (7.3, 2.45, false),
            (8.15, 2.5, false),
        ] {
            for (shape, material) in chair(x, y, facing_north) {
                put(shape, material);
            }
        }
        // A framed print on the green wall.
        put(
            cuboid([6.85, 4.17, 1.25], [8.25, 4.2, 2.0]),
            Material::Picture,
        );
        // Lamps: two flush in the first room, a pendant over the table and
        // one flush lamp in the second room.
        let flush = [[1.6, 2.1], [4.3, 1.4], [7.7, 3.45]];
        for centre in flush {
            put(
                Shape::Column {
                    centre,
                    radius: 0.18,
                    z: [HEIGHT - 0.06, HEIGHT],
                },
                Material::Lamp,
            );
        }
        let pendant = [7.7, 1.75];
        put(
            Shape::Column {
                centre: pendant,
                radius: 0.24,
                z: [1.5, 1.72],
            },
            Material::Lamp,
        );
        put(
            Shape::Column {
                centre: pendant,
                radius: 0.004,
                z: [1.72, HEIGHT],
            },
            Material::Metal,
        );
        let mut lights: Vec<Light> = flush
            .iter()
            .map(|[x, y]| Light {
                position: [*x, *y, HEIGHT - 0.08],
                power: 3.4,
                spread: 0.3,
            })
            .collect();
        lights.push(Light {
            position: [pendant[0], pendant[1], 1.47],
            power: 1.5,
            spread: 0.15,
        });
        let windows = vec![
            Window {
                corner: [SOUTH_WINDOW[0][0], 0.0, SOUTH_WINDOW[1][0]],
                across: [SOUTH_WINDOW[0][1] - SOUTH_WINDOW[0][0], 0.0, 0.0],
                up: [0.0, 0.0, SOUTH_WINDOW[1][1] - SOUTH_WINDOW[1][0]],
                normal: [0.0, 1.0, 0.0],
            },
            Window {
                corner: [ROOM_2[0][1], EAST_WINDOW[0][0], EAST_WINDOW[1][0]],
                across: [0.0, EAST_WINDOW[0][1] - EAST_WINDOW[0][0], 0.0],
                up: [0.0, 0.0, EAST_WINDOW[1][1] - EAST_WINDOW[1][0]],
                normal: [-1.0, 0.0, 0.0],
            },
        ];
        // A low sun from the south-east.
        let azimuth: f64 = 150.0f64.to_radians();
        let elevation: f64 = 30.0f64.to_radians();
        let sun = [
            elevation.cos() * azimuth.sin(),
            elevation.cos() * azimuth.cos(),
            elevation.sin(),
        ];
        let mut scene = Self {
            parts,
            nodes: Vec::new(),
            order: Vec::new(),
            lights,
            windows,
            sun,
        };
        scene.build();
        scene
    }

    /// Build a bounding volume hierarchy over the parts.
    fn build(&mut self) {
        let bounds: Vec<(V3, V3)> = self.parts.iter().map(|part| part.shape.bounds()).collect();
        let mut order: Vec<usize> = (0..self.parts.len()).collect();
        let mut nodes = vec![Node {
            min: [0.0; 3],
            max: [0.0; 3],
            first: 0,
            count: 0,
        }];
        split(&bounds, &mut order, 0, self.parts.len(), 0, &mut nodes);
        self.nodes = nodes;
        self.order = order;
    }

    /// The nearest surface along a ray of unit direction within `limit`.
    pub fn trace(&self, origin: V3, dir: V3, limit: f64) -> Option<Hit> {
        let inverse = inverse(dir);
        let mut best: Option<(f64, V3, usize)> = None;
        let mut stack = [0usize; 64];
        let mut depth = 1;
        while depth > 0 {
            depth -= 1;
            let node = self.nodes[stack[depth]];
            let reach = best.map_or(limit, |(t, _, _)| t);
            if !ray_meets_box(origin, inverse, node.min, node.max, reach) {
                continue;
            }
            if node.count > 0 {
                for &index in &self.order[node.first..node.first + node.count] {
                    let reach = best.map_or(limit, |(t, _, _)| t);
                    if let Some((t, normal)) =
                        self.parts[index].shape.hit(origin, dir, inverse, reach)
                    {
                        best = Some((t, normal, index));
                    }
                }
            } else {
                stack[depth] = node.first;
                stack[depth + 1] = node.first + 1;
                depth += 2;
            }
        }
        best.map(|(t, normal, index)| Hit {
            t,
            point: along(origin, dir, t),
            normal,
            material: self.parts[index].material,
        })
    }

    fn blocked(&self, origin: V3, dir: V3, limit: f64) -> bool {
        self.trace(origin, dir, limit).is_some()
    }

    /// The colour of a material at a point, without light.
    fn albedo(&self, hit: &Hit) -> Rgb {
        let [x, y, z] = hit.point;
        let n = hit.normal;
        match hit.material {
            Material::Paint => {
                let roller = 0.985 + 0.03 * noise2(SEED ^ 1, (x + y) * 14.0, z * 14.0);
                [0.86, 0.85, 0.82].map(|value| value * roller)
            }
            Material::Accent => {
                let roller = 0.98 + 0.04 * noise2(SEED ^ 2, x * 14.0, z * 14.0);
                [0.54, 0.61, 0.53].map(|value| value * roller)
            }
            Material::Ceiling => {
                let roller = 0.985 + 0.03 * noise2(SEED ^ 3, x * 10.0, y * 10.0);
                [0.9, 0.9, 0.89].map(|value| value * roller)
            }
            Material::Floor if x > PARTITION[1] => {
                let (i, j) = ((x - PARTITION[1]) / 0.6, y / 0.6);
                let (a, b) = (i - i.floor(), j - j.floor());
                if a.min(1.0 - a) * 0.6 < 0.002 || b.min(1.0 - b) * 0.6 < 0.002 {
                    return [0.5, 0.49, 0.47];
                }
                let tile = 0.95 + 0.08 * hash(SEED ^ 4, i.floor() as i64, j.floor() as i64, 0);
                let speck = 0.97 + 0.06 * noise2(SEED ^ 5, x * 60.0, y * 60.0);
                [0.74, 0.73, 0.7].map(|value| value * tile * speck)
            }
            Material::Floor if x > PARTITION[0] => [0.5, 0.37, 0.25],
            Material::Floor => oak_boards(x, y),
            Material::Concrete => {
                let mottle = 0.88 + 0.2 * fbm3(SEED ^ 6, scale(hit.point, 2.0), 3);
                let holes = if noise3(SEED ^ 7, scale(hit.point, 90.0)) > 0.91 {
                    0.72
                } else {
                    1.0
                };
                let streak = 0.97 + 0.06 * noise2(SEED ^ 8, (x + y) * 25.0, z * 1.5);
                [0.63, 0.62, 0.59].map(|value| value * mottle * holes * streak)
            }
            Material::Skirting => [0.93, 0.93, 0.91],
            Material::Frame => [0.94, 0.94, 0.93],
            Material::Door => [0.92, 0.92, 0.9],
            Material::Sill => {
                let grains = if noise3(SEED ^ 9, scale(hit.point, 90.0)) > 0.8 {
                    0.82
                } else {
                    1.0
                };
                [0.84, 0.83, 0.8].map(|value| value * grains)
            }
            Material::Radiator => {
                // Vertical fins along the wall.
                let a = if n[2].abs() > 0.5 {
                    x + y
                } else if n[0].abs() > 0.5 {
                    y
                } else {
                    x
                };
                let fin = 0.8 + 0.2 * (PI * a / 0.05).sin().abs();
                [0.93, 0.93, 0.91].map(|value| value * fin)
            }
            Material::Cabinet => {
                let across = if n[1].abs() > 0.5 { x } else { y };
                let grain = 0.9 + 0.16 * noise2(SEED ^ 10, across * 80.0, z * 3.0);
                let mut colour = [0.7, 0.58, 0.43].map(|value| value * grain);
                // The gaps between the doors of the cabinet.
                let gap = n[1] < -0.5
                    && x < 1.6
                    && ((x - 0.95).abs() < 0.002 || (z - 1.42).abs() < 0.002);
                if gap {
                    colour = colour.map(|value| value * 0.3);
                }
                colour
            }
            Material::Plinth => [0.24, 0.24, 0.25],
            Material::Handle => [0.6, 0.6, 0.62],
            Material::TableTop => {
                let grain = 0.88 + 0.18 * noise2(SEED ^ 11, x * 3.0, y * 70.0);
                [0.6, 0.45, 0.3].map(|value| value * grain)
            }
            Material::Metal => [0.08, 0.08, 0.09],
            Material::Fabric => {
                let weave = 0.92 + 0.12 * noise2(SEED ^ 12, (x + z) * 200.0, (y - z) * 200.0);
                [0.22, 0.28, 0.36].map(|value| value * weave)
            }
            Material::Lamp => [1.0, 0.98, 0.94],
            Material::Picture => print(x, z),
        }
    }

    /// The light that reaches a point from the lamps, the windows, the sun
    /// and all around, per channel. `key` seeds the samples of soft shadows
    /// and corners.
    fn irradiance(&self, hit: &Hit, key: u64) -> Rgb {
        let mut rng = Rng::new(SEED ^ key);
        let n = hit.normal;
        let p = along(hit.point, n, 1e-4);
        let mut light = [0.0; 3];
        let mut add_light = |tint: Rgb, amount: f64| {
            for channel in 0..3 {
                light[channel] += tint[channel] * amount;
            }
        };
        // Light that bounces around, less in corners and under furniture.
        let samples = 8;
        let reach = 0.45;
        let (u, v) = frame(n);
        let mut open = 0.0;
        for _ in 0..samples {
            let (a, b) = (rng.unit(), rng.unit());
            let radius = a.sqrt();
            let angle = TAU * b;
            let dir = along(
                along(scale(u, radius * angle.cos()), v, radius * angle.sin()),
                n,
                (1.0 - a).sqrt(),
            );
            open += match self.trace(p, dir, reach) {
                Some(hit) => hit.t / reach,
                None => 1.0,
            };
        }
        let ao = open / samples as f64;
        add_light(
            [0.98, 0.98, 1.0],
            (0.3 + 0.12 * (-n[2]).max(0.0)) * (0.25 + 0.75 * ao),
        );
        // The lamps, each from two points of its shade, so that shadows
        // are soft and the same for every point.
        for lamp in &self.lights {
            for offset in [[0.09, 0.06, 0.0], [-0.09, -0.06, 0.0]] {
                let to = sub(add(lamp.position, offset), p);
                let distance = dot(to, to).sqrt();
                let dir = scale(to, 1.0 / distance);
                let facing = dot(n, dir);
                if facing <= 0.0 {
                    continue;
                }
                let down = (-dir[2]).max(0.0);
                let emit = lamp.spread + (1.0 - lamp.spread) * down;
                // The lamp's own body and the ceiling around it cast no
                // shadow of it.
                if !self.blocked(p, dir, (distance - 0.3).max(0.0)) {
                    let amount = lamp.power * facing * emit / (distance * distance + 0.25);
                    add_light([1.0, 0.95, 0.86], amount / 2.0);
                }
            }
        }
        // The sky through the windows, seen at a point of each quarter of
        // each window.
        for window in &self.windows {
            let area = length(cross(window.across, window.up));
            let mut sky = 0.0;
            for quarter in 0..4 {
                let a = (f64::from(quarter % 2) + rng.unit()) / 2.0;
                let b = (f64::from(quarter / 2) + rng.unit()) / 2.0;
                let target = along(along(window.corner, window.across, a), window.up, b);
                let to = sub(target, p);
                let distance = dot(to, to).sqrt();
                let dir = scale(to, 1.0 / distance);
                let facing = dot(n, dir);
                let entering = -dot(window.normal, dir);
                if facing <= 0.0 || entering <= 0.0 {
                    continue;
                }
                if !self.blocked(p, dir, distance - 0.01) {
                    sky += facing * entering * area / (PI * (distance * distance + 0.05));
                }
            }
            add_light([0.92, 0.97, 1.05], 2.2 * sky / 4.0);
        }
        // Sunlight, where it falls through a window.
        let facing = dot(n, self.sun);
        if facing > 0.0 && !self.blocked(p, self.sun, 60.0) {
            add_light([1.0, 0.96, 0.88], 2.3 * facing);
        }
        light
    }

    /// What a camera at the station sees along a ray: the colour of the
    /// surface it meets, or of the view outside.
    pub fn colour(&self, hit: Option<&Hit>, dir: V3, key: u64) -> Rgb {
        let Some(hit) = hit else {
            return outside(dir);
        };
        if hit.material == Material::Lamp {
            return [1.0, 0.99, 0.96];
        }
        let albedo = self.albedo(hit);
        let light = self.irradiance(hit, key);
        let tone = |x: f64| ((1.0 - (-1.8 * x).exp()) / (1.0 - (-1.8f64).exp())).min(1.0);
        std::array::from_fn(|channel| tone(albedo[channel] * light[channel]))
    }
}

/// Split the parts `order[first..first + count]` into a subtree at `node`.
fn split(
    bounds: &[(V3, V3)],
    order: &mut [usize],
    first: usize,
    count: usize,
    node: usize,
    nodes: &mut Vec<Node>,
) {
    let slice = &order[first..first + count];
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut centre_min = [f64::INFINITY; 3];
    let mut centre_max = [f64::NEG_INFINITY; 3];
    for &index in slice {
        let (low, high) = bounds[index];
        for a in 0..3 {
            min[a] = min[a].min(low[a]);
            max[a] = max[a].max(high[a]);
            let centre = (low[a] + high[a]) / 2.0;
            centre_min[a] = centre_min[a].min(centre);
            centre_max[a] = centre_max[a].max(centre);
        }
    }
    nodes[node].min = min;
    nodes[node].max = max;
    if count <= 2 {
        nodes[node].first = first;
        nodes[node].count = count;
        return;
    }
    let axis = (0..3)
        .max_by(|a, b| {
            (centre_max[*a] - centre_min[*a]).total_cmp(&(centre_max[*b] - centre_min[*b]))
        })
        .unwrap_or(0);
    let centre = |index: usize| (bounds[index].0[axis] + bounds[index].1[axis]) / 2.0;
    order[first..first + count].sort_by(|a, b| centre(*a).total_cmp(&centre(*b)).then(a.cmp(b)));
    let left = nodes.len();
    let empty = Node {
        min: [0.0; 3],
        max: [0.0; 3],
        first: 0,
        count: 0,
    };
    nodes.push(empty);
    nodes.push(empty);
    nodes[node].first = left;
    nodes[node].count = 0;
    let half = count / 2;
    split(bounds, order, first, half, left, nodes);
    split(bounds, order, first + half, count - half, left + 1, nodes);
}

/// The boxes of a wall from `min` to `max` without its openings, each given
/// along the wall (`along` is its axis) and up.
fn wall_with_openings(min: V3, max: V3, along: usize, openings: &[[[f64; 2]; 2]]) -> Vec<Shape> {
    let mut cuts = vec![min[along], max[along]];
    for [across, _] in openings {
        cuts.extend([across[0], across[1]]);
    }
    cuts.sort_by(f64::total_cmp);
    let mut shapes = Vec::new();
    for pair in cuts.windows(2) {
        let (a0, a1) = (pair[0], pair[1]);
        if a1 - a0 < 1e-9 {
            continue;
        }
        let middle = (a0 + a1) / 2.0;
        let mut levels = vec![[min[2], max[2]]];
        for [across, up] in openings {
            if middle > across[0] && middle < across[1] {
                levels = vec![[min[2], up[0]], [up[1], max[2]]];
            }
        }
        for [z0, z1] in levels {
            if z1 - z0 < 1e-9 {
                continue;
            }
            let mut low = min;
            let mut high = max;
            low[along] = a0;
            high[along] = a1;
            low[2] = z0;
            high[2] = z1;
            shapes.push(Shape::Cuboid {
                min: low,
                max: high,
            });
        }
    }
    shapes
}

/// A window of two casements: an outer frame and a mullion 13 to 20 cm out
/// from the inner face of the wall, and a sash around each pane.
fn window_frame(face: Face, across: [f64; 2], up: [f64; 2]) -> Vec<Shape> {
    let frame_depth = [0.13, 0.2];
    let sash_depth = [0.12, 0.17];
    let (a0, a1, z0, z1) = (across[0], across[1], up[0], up[1]);
    let width = 0.06;
    let middle = (a0 + a1) / 2.0;
    let mut shapes = vec![
        face.cuboid([a0, a0 + width], frame_depth, [z0, z1]),
        face.cuboid([a1 - width, a1], frame_depth, [z0, z1]),
        face.cuboid([a0, a1], frame_depth, [z0, z0 + width]),
        face.cuboid([a0, a1], frame_depth, [z1 - width, z1]),
        face.cuboid(
            [middle - 0.035, middle + 0.035],
            frame_depth,
            [z0 + width, z1 - width],
        ),
    ];
    let sash = 0.05;
    for [p0, p1] in [[a0 + width, middle - 0.035], [middle + 0.035, a1 - width]] {
        let (q0, q1) = (z0 + width, z1 - width);
        shapes.push(face.cuboid([p0, p0 + sash], sash_depth, [q0, q1]));
        shapes.push(face.cuboid([p1 - sash, p1], sash_depth, [q0, q1]));
        shapes.push(face.cuboid([p0, p1], sash_depth, [q0, q0 + sash]));
        shapes.push(face.cuboid([p0, p1], sash_depth, [q1 - sash, q1]));
    }
    shapes
}

/// A chair with a fabric seat and back on thin black legs, its seat centred
/// at `x`, `y` and its back to the south when it faces north.
fn chair(x: f64, y: f64, facing_north: bool) -> Vec<(Shape, Material)> {
    let (half_x, half_y) = (0.22, 0.21);
    let back = if facing_north { y - half_y } else { y + half_y };
    let front = if facing_north { y + half_y } else { y - half_y };
    let inward = if facing_north { 1.0 } else { -1.0 };
    let mut parts = vec![
        (
            Shape::Cuboid {
                min: [x - half_x, y - half_y, 0.45],
                max: [x + half_x, y + half_y, 0.48],
            },
            Material::Fabric,
        ),
        (
            Shape::Cuboid {
                min: [x - half_x, back.min(back + inward * 0.025), 0.56],
                max: [x + half_x, back.max(back + inward * 0.025), 0.86],
            },
            Material::Fabric,
        ),
    ];
    for leg_x in [x - half_x + 0.02, x + half_x - 0.02] {
        for (leg_y, top) in [(back + inward * 0.02, 0.86), (front - inward * 0.02, 0.45)] {
            parts.push((
                Shape::Column {
                    centre: [leg_x, leg_y],
                    radius: 0.012,
                    z: [0.0, top],
                },
                Material::Metal,
            ));
        }
    }
    parts
}

/// Oak boards 15 cm wide along the first room, in lengths of 1.8 m that
/// start somewhere else in every row.
fn oak_boards(x: f64, y: f64) -> Rgb {
    let row = (y / 0.15).floor();
    let offset = 1.8 * hash(SEED ^ 20, row as i64, 0, 0);
    let along = x + offset;
    let board = (along / 1.8).floor();
    let (a, b) = (along - board * 1.8, y - row * 0.15);
    if a < 0.002 || b < 0.0015 {
        return [0.3, 0.22, 0.15];
    }
    let h = hash(SEED ^ 21, row as i64, board as i64, 0);
    let grain = 0.88 + 0.2 * noise2(SEED ^ 22, along * 2.5, y * 70.0);
    let knots = if noise2(SEED ^ 23, along * 9.0, y * 30.0) > 0.88 {
        0.75
    } else {
        1.0
    };
    [0.64, 0.49, 0.33].map(|value| value * (0.84 + 0.26 * h) * grain * knots)
}

/// A framed print: a black frame, a white mount and a landscape of a few
/// colours.
fn print(x: f64, z: f64) -> Rgb {
    let (u, v) = ((x - 6.85) / 1.4, (z - 1.25) / 0.75);
    let edge = (x - 6.85).min(8.25 - x).min(z - 1.25).min(2.0 - z);
    if edge < 0.03 {
        return [0.06, 0.06, 0.06];
    }
    if edge < 0.09 {
        return [0.93, 0.92, 0.89];
    }
    let hills = 0.42 + 0.08 * (u * 9.0).sin() + 0.05 * (u * 23.0 + 1.0).sin();
    let sun = (u - 0.7).hypot((v - 0.72) * 0.55);
    if sun < 0.07 {
        return [0.9, 0.52, 0.22];
    }
    if v > hills {
        mix(
            [0.78, 0.84, 0.86],
            [0.42, 0.6, 0.72],
            (v - hills) / (1.0 - hills),
        )
    } else if v > hills - 0.12 {
        [0.76, 0.6, 0.32]
    } else {
        mix(
            [0.16, 0.38, 0.38],
            [0.24, 0.48, 0.44],
            smoothstep(0.0, 0.3, v),
        )
    }
}

/// The view through the windows: sky, a line of trees and the lawn.
fn outside(dir: V3) -> Rgb {
    let elevation = dir[2].asin();
    let azimuth = dir[1].atan2(dir[0]);
    let treeline = 0.05 + 0.05 * fbm2(SEED ^ 30, azimuth * 6.0, 0.0, 3);
    if elevation > treeline {
        let up = (elevation / (PI / 2.0)).clamp(0.0, 1.0).powf(0.6);
        let cloud = smoothstep(
            0.55,
            0.8,
            fbm2(SEED ^ 31, azimuth * 3.0, elevation * 8.0, 4),
        );
        let sky = mix([0.9, 0.93, 0.96], [0.66, 0.78, 0.94], up);
        mix(sky, [0.97, 0.97, 0.97], cloud * 0.7)
    } else if elevation > 0.0 {
        let leaves = 0.8 + 0.3 * noise2(SEED ^ 32, azimuth * 60.0, elevation * 60.0);
        [0.42 * leaves, 0.52 * leaves, 0.36 * leaves]
    } else {
        let blades = 0.9 + 0.15 * noise2(SEED ^ 33, azimuth * 40.0, elevation * 80.0);
        [0.58 * blades, 0.66 * blades, 0.46 * blades]
    }
}

/// One measured point in the scanner's own frame.
#[derive(Clone, Copy, Debug)]
pub struct ScanPoint {
    pub local: V3,
    pub rgb: [u8; 3],
    pub intensity: u16,
    pub row: u32,
    pub column: u32,
}

fn rows(detail: &Detail) -> usize {
    let step = TAU / detail.columns as f64;
    ((PI / 2.0 - LOWEST) / step).floor() as usize + 1
}

fn to_byte(value: f64) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Sweep the scanner of a station: every column from the bottom of its
/// field to the zenith, as a scanner turns.
pub fn sweep(scene: &Scene, station: &Station, number: u64, detail: &Detail) -> Vec<ScanPoint> {
    let step = TAU / detail.columns as f64;
    let rows = rows(detail);
    let axes = station.axes();
    let columns: Vec<Vec<ScanPoint>> = (0..detail.columns)
        .into_par_iter()
        .map(|column| {
            let azimuth = column as f64 * step;
            let mut points = Vec::with_capacity(rows);
            for row in 0..rows {
                let elevation = (LOWEST + row as f64 * step).min(PI / 2.0);
                let local_dir = [
                    elevation.cos() * azimuth.cos(),
                    elevation.cos() * azimuth.sin(),
                    elevation.sin(),
                ];
                let dir = add(
                    add(scale(axes[0], local_dir[0]), scale(axes[1], local_dir[1])),
                    scale(axes[2], local_dir[2]),
                );
                let Some(hit) = scene.trace(station.position, dir, 60.0) else {
                    continue;
                };
                let key = splitmix((number << 40) ^ ((column as u64) << 20) ^ row as u64);
                let mut rng = Rng::new(key ^ 0x5ca1);
                let range = hit.t + (0.0012 + 0.00025 * hit.t) * rng.normal();
                let rgb = scene
                    .colour(Some(&hit), dir, key)
                    .map(|value| value * (1.0 + 0.01 * rng.normal()));
                let incidence = dot(hit.normal, dir).abs();
                let strength = hit.material.reflectivity(hit.point)
                    * (0.25 + 0.75 * incidence)
                    * (1.0 - 0.025 * hit.t)
                    + 0.012 * rng.normal();
                points.push(ScanPoint {
                    local: scale(local_dir, range),
                    rgb: rgb.map(to_byte),
                    intensity: (strength.clamp(0.0, 1.0) * 65535.0).round() as u16,
                    row: row as u32,
                    column: column as u32,
                });
            }
            points
        })
        .collect();
    columns.into_iter().flatten().collect()
}

/// The panorama of a station as the application reads it from the file.
pub fn panorama_photo(station: &Station, size: [u32; 2]) -> FilePhoto {
    FilePhoto {
        name: Some(station.name.to_owned()),
        station: None,
        position: station.position,
        axes: station.axes(),
        width: size[0],
        height: size[1],
        projection: PhotoProjection::Spherical {
            pixel_size: [TAU / f64::from(size[0]), PI / f64::from(size[1])],
        },
        format: ScanImageFormat::Jpeg,
        offset: 0,
        length: 0,
    }
}

/// Render the panorama of a station, pixel by pixel through the rays the
/// application casts for it, and soften it a little as a lens does.
pub fn render_panorama(scene: &Scene, photo: &FilePhoto, number: u64) -> Vec<u8> {
    let (width, height) = (photo.width as usize, photo.height as usize);
    let rows: Vec<Vec<Rgb>> = (0..height)
        .into_par_iter()
        .map(|row| {
            (0..width)
                .map(|column| {
                    let dir = normalize(photo.ray(column as f64, row as f64));
                    let hit = scene.trace(photo.position, dir, 60.0);
                    let key =
                        splitmix(0x9a4 ^ (number << 40) ^ ((row as u64) << 20) ^ column as u64);
                    scene.colour(hit.as_ref(), dir, key)
                })
                .collect()
        })
        .collect();
    let mut bytes = vec![0u8; width * height * 3];
    bytes
        .par_chunks_mut(width * 3)
        .enumerate()
        .for_each(|(row, line)| {
            for column in 0..width {
                let mut sum = [0.0; 3];
                let mut total = 0.0;
                for dr in -1i64..=1 {
                    let r = (row as i64 + dr).clamp(0, height as i64 - 1) as usize;
                    for dc in -1i64..=1 {
                        let c = (column as i64 + dc).rem_euclid(width as i64) as usize;
                        let weight = ((2 - dr.abs()) * (2 - dc.abs())) as f64;
                        for (channel, value) in sum.iter_mut().enumerate() {
                            *value += weight * rows[r][c][channel];
                        }
                        total += weight;
                    }
                }
                for channel in 0..3 {
                    line[column * 3 + channel] = to_byte(sum[channel] / total);
                }
            }
        });
    bytes
}

pub fn jpeg(bytes: &[u8], size: [u32; 2]) -> Result<Vec<u8>, Failure> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90).encode(
        bytes,
        size[0],
        size[1],
        image::ColorType::Rgb8,
    )?;
    Ok(out)
}

fn guid(kind: u32, index: usize) -> String {
    format!("{{6f70732d-6465-4d6f-8000-{kind:04x}{index:08x}}}")
}

/// Write `rooms.e57` with both stations and their panoramas, and
/// `rooms.las` with the same points, into `folder`.
pub fn write(folder: &Path) -> Result<Vec<(PathBuf, u64)>, Failure> {
    write_with(folder, &FULL)
}

pub fn write_with(folder: &Path, detail: &Detail) -> Result<Vec<(PathBuf, u64)>, Failure> {
    let scene = Scene::new();
    let e57_path = folder.join("rooms.e57");
    let las_path = folder.join("rooms.las");
    let mut e57 = E57Writer::from_file(&e57_path, &guid(1, 0))?;
    let mut las = las_writer(&las_path, false, [0.0; 3])?;
    let rows = rows(detail);
    let coordinate = |name: RecordName| Record {
        name,
        data_type: RecordDataType::ScaledInteger {
            min: -300_000,
            max: 300_000,
            scale: 0.0001,
            offset: 0.0,
        },
    };
    let mut total = 0u64;
    for (index, station) in STATIONS.iter().enumerate() {
        let number = index as u64 + 1;
        let points = sweep(&scene, station, number, detail);
        let scan_guid = guid(2, index);
        let prototype = vec![
            coordinate(RecordName::CartesianX),
            coordinate(RecordName::CartesianY),
            coordinate(RecordName::CartesianZ),
            Record::COLOR_RED_U8,
            Record::COLOR_GREEN_U8,
            Record::COLOR_BLUE_U8,
            Record::INTENSITY_U16,
            Record {
                name: RecordName::RowIndex,
                data_type: RecordDataType::Integer {
                    min: 0,
                    max: rows as i64 - 1,
                },
            },
            Record {
                name: RecordName::ColumnIndex,
                data_type: RecordDataType::Integer {
                    min: 0,
                    max: detail.columns as i64 - 1,
                },
            },
        ];
        let mut scan = e57.add_pointcloud(&scan_guid, prototype)?;
        scan.set_name(Some(station.name.to_owned()));
        scan.set_transform(Some(station.transform()));
        for point in &points {
            let fixed = |value: f64| RecordValue::ScaledInteger((value / 0.0001).round() as i64);
            scan.add_point(vec![
                fixed(point.local[0]),
                fixed(point.local[1]),
                fixed(point.local[2]),
                RecordValue::Integer(i64::from(point.rgb[0])),
                RecordValue::Integer(i64::from(point.rgb[1])),
                RecordValue::Integer(i64::from(point.rgb[2])),
                RecordValue::Integer(i64::from(point.intensity)),
                RecordValue::Integer(i64::from(point.row)),
                RecordValue::Integer(i64::from(point.column)),
            ])?;
        }
        scan.finalize()?;
        let samples: Vec<Sample> = points
            .par_iter()
            .map(|point| Sample {
                xyz: station.registered(point.local),
                rgb: point.rgb.map(|value| f64::from(value) / 255.0),
                intensity: f64::from(point.intensity) / 65535.0,
                class: class::UNCLASSIFIED,
                source: number as u16,
            })
            .collect();
        for chunk in samples.chunks(400_000) {
            write_samples(&mut las, chunk)?;
        }
        total += points.len() as u64;
        // The panorama of the station.
        let photo = panorama_photo(station, detail.panorama);
        let pixels = render_panorama(&scene, &photo, number);
        let encoded = jpeg(&pixels, detail.panorama)?;
        let mut image = e57.add_image(&guid(3, index))?;
        image.set_name(photo.name.as_deref().unwrap_or(station.name));
        image.set_pointcloud_guid(&scan_guid);
        image.set_transform(station.transform());
        image.add_spherical(
            ImageFormat::Jpeg,
            &mut encoded.as_slice(),
            SphericalImageProperties {
                width: detail.panorama[0],
                height: detail.panorama[1],
                pixel_width: TAU / f64::from(detail.panorama[0]),
                pixel_height: PI / f64::from(detail.panorama[1]),
            },
            None,
        )?;
        image.finalize()?;
    }
    e57.finalize()?;
    las.close()?;
    Ok(vec![(e57_path, total), (las_path, total)])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SMALL: Detail = Detail {
        columns: 240,
        panorama: [256, 128],
    };

    /// The pixel of a panorama that the application projects a scan point
    /// to has the colour of that point: the photo and the scan agree on
    /// where the station stands and which way it faces.
    #[test]
    fn panorama_pixels_show_what_the_scan_measured() {
        let scene = Scene::new();
        for (index, station) in STATIONS.iter().enumerate() {
            let photo = panorama_photo(station, SMALL.panorama);
            let pixels = render_panorama(&scene, &photo, index as u64);
            let points = sweep(&scene, station, index as u64, &SMALL);
            assert!(points.len() > 20_000, "{} points", points.len());
            let mut close = 0;
            for point in &points {
                let world = station.registered(point.local);
                let pixel = photo.project(sub(world, station.position)).unwrap();
                let column = (pixel[0].round() as usize).min(photo.width as usize - 1);
                let row = (pixel[1].round() as usize).min(photo.height as usize - 1);
                let at = (row * photo.width as usize + column) * 3;
                let difference = (0..3)
                    .map(|channel| {
                        (i32::from(pixels[at + channel]) - i32::from(point.rgb[channel])).abs()
                    })
                    .max()
                    .unwrap();
                close += usize::from(difference <= 40);
            }
            let share = close as f64 / points.len() as f64;
            assert!(
                share > 0.85,
                "{} matches {share:.3} of its points",
                station.name
            );
        }
    }

    /// The application reads both stations, their names and poses, and
    /// both panoramas as photos of those stations.
    #[test]
    fn the_application_reads_the_stations_and_their_panoramas() {
        let folder = tempfile::tempdir().unwrap();
        let written = write_with(folder.path(), &SMALL).unwrap();
        let e57 = &written[0].0;
        let cloud = pointcloud_core::open(e57, 1_000).unwrap();
        assert_eq!(cloud.total_points, written[0].1);
        assert!(cloud.has_rgb && cloud.has_intensity);
        let names: Vec<_> = cloud
            .scan_poses
            .iter()
            .map(|pose| pose.label.as_str())
            .collect();
        assert_eq!(names, ["Station 1", "Station 2"]);
        for (pose, station) in cloud.scan_poses.iter().zip(&STATIONS) {
            for axis in 0..3 {
                assert!((pose.position[axis] - station.position[axis]).abs() < 1e-9);
            }
        }
        let photos = pointcloud_core::file_photos(e57).unwrap();
        assert_eq!(photos.skipped, 0);
        assert_eq!(photos.photos.len(), 2);
        for (number, photo) in photos.photos.iter().enumerate() {
            assert_eq!(photo.station, Some(number));
            assert_eq!(photo.kind(), pointcloud_core::PhotoKind::Spherical);
            assert_eq!((photo.width, photo.height), (256, 128));
            assert_eq!(photo.name.as_deref(), Some(STATIONS[number].name));
            let bytes = pointcloud_core::read_file_photo(e57, photo).unwrap();
            let decoded = image::load_from_memory(&bytes).unwrap().to_rgb8();
            assert_eq!(decoded.dimensions(), (256, 128));
        }
        // Every point of the LAS copy lies inside the rooms.
        let mut reader = las::Reader::from_path(&written[1].0).unwrap();
        assert_eq!(reader.header().number_of_points(), written[1].1);
        for point in reader.points().take(5_000) {
            let point = point.unwrap();
            assert!(point.x > -WALL - 0.01 && point.x < ROOM_2[0][1] + WALL + 0.01);
            assert!(point.z > -0.01 && point.z < HEIGHT + 0.01);
        }
    }

    #[test]
    fn a_sweep_is_the_same_every_time() {
        let scene = Scene::new();
        let tiny = Detail {
            columns: 60,
            panorama: [64, 32],
        };
        let first = sweep(&scene, &STATIONS[1], 2, &tiny);
        let second = sweep(&scene, &STATIONS[1], 2, &tiny);
        assert_eq!(first.len(), second.len());
        for (a, b) in first.iter().zip(&second) {
            assert_eq!((a.local, a.rgb, a.intensity), (b.local, b.rgb, b.intensity));
        }
    }
}
