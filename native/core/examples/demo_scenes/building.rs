//! The outdoor scene: a two-storey brick house with a pitched roof on a plot
//! that slopes gently up from the street. A paved path leads from the
//! sidewalk to the front door, a terrace lies behind the house, a hedge runs
//! along the sidewalk and board fences close the other sides. There are
//! five trees, shrubs in a bed in front of the house, a garden shed and a
//! street light.
//!
//! Surfaces are sampled about as densely as a combined survey of terrestrial
//! and aerial scans, with a few millimetres of noise. Colours come from
//! materials lit by a low afternoon sun from the south-west, with the shadows
//! of the house, the shed, the hedge, the fences and the tree crowns, and
//! some light taken away in corners and under trees.

use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::path::Path;

use super::common::{
    along, class, cross, dot, fbm2, fbm3, hash, mix, noise2, noise3, normalize, scale, smoothstep,
    sub, Rgb, Rng, Sample, V3,
};
use super::{las_writer, run_jobs, write_samples, Failure, Job};

const SEED: u64 = 0x0b01_1d1e_5ce7;
/// Site coordinates are metres east and north of this point of the grid.
pub const ORIGIN: V3 = [1000.0, 2000.0, 0.0];
const SITE: [f64; 2] = [64.0, 50.0];

const HOUSE_X: [f64; 2] = [25.0, 36.0];
const HOUSE_Y: [f64; 2] = [22.0, 30.5];
const RIDGE_Y: f64 = 26.25;
const PITCH: f64 = 40.0 * PI / 180.0;
/// How far the roof reaches past the long walls and past the gables.
const EAVES: f64 = 0.45;
const VERGE: f64 = 0.25;
const FASCIA: f64 = 0.2;
/// Height of the long walls above the ground floor.
const WALLS: f64 = 5.6;
/// Depth of the brick reveal in front of window and door frames.
const REVEAL: f64 = 0.10;
const CHIMNEY: [[f64; 2]; 2] = [[33.0, 33.72], [26.62, 27.12]];

const PATH_X: [f64; 2] = [28.9, 30.1];
const TERRACE: [[f64; 2]; 2] = [[25.3, 33.7], [30.5, 34.6]];
const BED_Y: [f64; 2] = [20.5, 21.62];
const DOORSTEP: [[f64; 2]; 2] = [[28.85, 30.15], [21.62, 22.0]];
const STREET_Y: f64 = 6.0;
const SIDEWALK_Y: [f64; 2] = [6.15, 8.3];
const HEDGE_X: [f64; 2] = [2.0, 61.5];
const HEDGE_Y: [f64; 2] = [8.6, 9.4];
const HEDGE_GAP: [f64; 2] = [28.6, 30.4];
const HEDGE_HEIGHT: f64 = 1.3;
const FENCES: [[[f64; 2]; 2]; 3] = [
    [[2.0, 9.6], [2.0, 47.5]],
    [[2.0, 47.5], [61.5, 47.5]],
    [[61.5, 47.5], [61.5, 9.6]],
];
const FENCE_HEIGHT: f64 = 1.8;
const SHED_X: [f64; 2] = [4.2, 7.2];
const SHED_Y: [f64; 2] = [43.8, 46.6];
/// Height of the shed's roof above the ground at its front and at its back.
const SHED_FRONT: f64 = 2.3;
const SHED_REAR: f64 = 2.0;
const LAMP: [f64; 2] = [12.0, 7.4];

const SKY: Rgb = [0.93, 0.97, 1.06];
const SUNLIGHT: Rgb = [1.06, 1.0, 0.9];

/// Height of the natural ground: a slope up from the street with gentle
/// undulations.
pub fn terrain(x: f64, y: f64) -> f64 {
    10.0 + 0.012 * (y - 8.3)
        + 0.006 * (x - 32.0)
        + 0.1 * (fbm2(SEED ^ 1, x / 11.0, y / 11.0, 3) - 0.5)
        + 0.01 * (noise2(SEED ^ 2, x / 0.8, y / 0.8) - 0.5)
}

fn sidewalk_z(x: f64, y: f64) -> f64 {
    terrain(x, SIDEWALK_Y[1]) - 0.01 - 0.025 * (SIDEWALK_Y[1] - y) / 2.15
}

fn curb_top(x: f64) -> f64 {
    sidewalk_z(x, SIDEWALK_Y[0]) + 0.005
}

/// The street has a crown in its middle and gutters at its edges.
fn street_z(x: f64, y: f64) -> f64 {
    let across = (y - 3.0) / 3.0;
    curb_top(x) - 0.13 + 0.07 * (1.0 - across * across).max(0.0)
}

fn in_range(value: f64, range: [f64; 2]) -> bool {
    value >= range[0] && value <= range[1]
}

/// Distance from a point to a rectangle in plan, zero inside it.
fn distance_to_rect(x: f64, y: f64, xs: [f64; 2], ys: [f64; 2]) -> f64 {
    let dx = (xs[0] - x).max(x - xs[1]).max(0.0);
    let dy = (ys[0] - y).max(y - ys[1]).max(0.0);
    dx.hypot(dy)
}

/// A convex solid that casts a shadow: the half-spaces `normal . p <= offset`.
struct Solid {
    planes: Vec<(V3, f64)>,
    min: V3,
    max: V3,
}

impl Solid {
    fn cuboid(min: V3, max: V3) -> Self {
        let mut planes = Vec::new();
        for axis in 0..3 {
            let mut normal = [0.0; 3];
            normal[axis] = 1.0;
            planes.push((normal, max[axis]));
            normal[axis] = -1.0;
            planes.push((normal, -min[axis]));
        }
        Self { planes, min, max }
    }

    fn hit(&self, origin: V3, dir: V3) -> bool {
        if !ray_meets_box(origin, dir, self.min, self.max) {
            return false;
        }
        let (mut near, mut far) = (0.0f64, f64::INFINITY);
        for (normal, offset) in &self.planes {
            let towards = dot(*normal, dir);
            let gap = offset - dot(*normal, origin);
            if towards.abs() < 1e-12 {
                if gap < 0.0 {
                    return false;
                }
                continue;
            }
            let t = gap / towards;
            if towards > 0.0 {
                far = far.min(t);
            } else {
                near = near.max(t);
            }
            if near > far {
                return false;
            }
        }
        true
    }
}

fn ray_meets_box(origin: V3, dir: V3, min: V3, max: V3) -> bool {
    let (mut near, mut far) = (0.0f64, f64::INFINITY);
    for axis in 0..3 {
        if dir[axis].abs() < 1e-12 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return false;
            }
            continue;
        }
        let a = (min[axis] - origin[axis]) / dir[axis];
        let b = (max[axis] - origin[axis]) / dir[axis];
        near = near.max(a.min(b));
        far = far.min(a.max(b));
        if near > far {
            return false;
        }
    }
    true
}

/// The crown of a tree as an ellipsoid that lets some sunlight through.
#[derive(Clone, Copy)]
struct Crown {
    center: V3,
    radii: V3,
}

impl Crown {
    /// Length of the part of a ray inside the crown.
    fn chord(&self, origin: V3, dir: V3) -> f64 {
        let o = [
            (origin[0] - self.center[0]) / self.radii[0],
            (origin[1] - self.center[1]) / self.radii[1],
            (origin[2] - self.center[2]) / self.radii[2],
        ];
        let d = [
            dir[0] / self.radii[0],
            dir[1] / self.radii[1],
            dir[2] / self.radii[2],
        ];
        let a = dot(d, d);
        let b = dot(o, d);
        let c = dot(o, o) - 1.0;
        let disc = b * b - a * c;
        if disc <= 0.0 {
            return 0.0;
        }
        let root = disc.sqrt();
        let (t0, t1) = ((-b - root) / a, (-b + root) / a);
        if t1 <= 0.0 {
            return 0.0;
        }
        t1 - t0.max(0.0)
    }

    /// 0 at the centre, 1 on the surface.
    fn depth(&self, p: V3) -> f64 {
        let q = [
            (p[0] - self.center[0]) / self.radii[0],
            (p[1] - self.center[1]) / self.radii[1],
            (p[2] - self.center[2]) / self.radii[2],
        ];
        dot(q, q).sqrt()
    }

    fn outward(&self, p: V3) -> V3 {
        normalize([
            (p[0] - self.center[0]) / (self.radii[0] * self.radii[0]),
            (p[1] - self.center[1]) / (self.radii[1] * self.radii[1]),
            (p[2] - self.center[2]) / (self.radii[2] * self.radii[2]),
        ])
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Species {
    Lime,
    Birch,
    Oak,
    Maple,
    Spruce,
}

#[derive(Clone, Copy)]
struct Tree {
    x: f64,
    y: f64,
    height: f64,
    crown_radius: f64,
    crown_base: f64,
    trunk_radius: f64,
    species: Species,
    seed: u64,
}

impl Tree {
    fn ground(&self) -> f64 {
        terrain(self.x, self.y)
    }

    fn crown(&self) -> Crown {
        let ground = self.ground();
        Crown {
            center: [
                self.x,
                self.y,
                ground + (self.crown_base + self.height) / 2.0,
            ],
            radii: [
                self.crown_radius,
                self.crown_radius,
                (self.height - self.crown_base) / 2.0,
            ],
        }
    }

    /// The spheres whose surfaces make up the foliage.
    fn lobes(&self) -> Vec<(V3, f64)> {
        let crown = self.crown();
        let mut rng = Rng::new(self.seed);
        let count = 15 + (rng.unit() * 7.0) as usize;
        let mut lobes = vec![(crown.center, self.crown_radius * 0.5)];
        for _ in 0..count {
            let mut direction = rng.direction();
            direction[2] = direction[2] * 0.8 + 0.2;
            let direction = normalize(direction);
            let reach = rng.range(0.38, 0.74);
            let center = [
                crown.center[0] + direction[0] * crown.radii[0] * reach,
                crown.center[1] + direction[1] * crown.radii[1] * reach,
                crown.center[2] + direction[2] * crown.radii[2] * reach,
            ];
            lobes.push((center, self.crown_radius * rng.range(0.28, 0.46)));
        }
        lobes
    }

    fn leaf_colours(&self) -> (Rgb, Rgb) {
        match self.species {
            Species::Lime => ([0.20, 0.33, 0.12], [0.36, 0.48, 0.18]),
            Species::Birch => ([0.25, 0.36, 0.13], [0.42, 0.50, 0.20]),
            Species::Oak => ([0.18, 0.28, 0.11], [0.30, 0.40, 0.15]),
            Species::Maple => ([0.22, 0.32, 0.12], [0.40, 0.44, 0.16]),
            Species::Spruce => ([0.10, 0.18, 0.11], [0.18, 0.28, 0.14]),
        }
    }
}

const TREES: [Tree; 5] = [
    Tree {
        x: 13.5,
        y: 16.0,
        height: 11.5,
        crown_radius: 4.1,
        crown_base: 3.0,
        trunk_radius: 0.24,
        species: Species::Lime,
        seed: 0x7ee1,
    },
    Tree {
        x: 47.0,
        y: 15.5,
        height: 10.0,
        crown_radius: 2.5,
        crown_base: 3.4,
        trunk_radius: 0.15,
        species: Species::Birch,
        seed: 0x7ee2,
    },
    Tree {
        x: 15.0,
        y: 38.5,
        height: 13.0,
        crown_radius: 5.0,
        crown_base: 3.2,
        trunk_radius: 0.36,
        species: Species::Oak,
        seed: 0x7ee3,
    },
    Tree {
        x: 48.0,
        y: 40.5,
        height: 9.5,
        crown_radius: 3.4,
        crown_base: 2.6,
        trunk_radius: 0.2,
        species: Species::Maple,
        seed: 0x7ee4,
    },
    Tree {
        x: 54.5,
        y: 28.5,
        height: 9.0,
        crown_radius: 2.3,
        crown_base: 1.0,
        trunk_radius: 0.2,
        species: Species::Spruce,
        seed: 0x7ee5,
    },
];

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Window,
    Door,
    GlassDoor,
}

#[derive(Clone, Copy)]
struct Opening {
    u: [f64; 2],
    z: [f64; 2],
    kind: Kind,
    /// Share of the window that a curtain behind it covers from the top.
    curtain: f64,
}

/// One outer wall: its corner at the start, the direction along it and the
/// direction it faces.
struct Facade {
    id: u64,
    start: [f64; 2],
    along: V3,
    normal: V3,
    length: f64,
    gable: bool,
    openings: Vec<Opening>,
}

impl Facade {
    /// The point `depth` behind the wall face at `u` along it and height `z`.
    fn at(&self, u: f64, z: f64, depth: f64) -> V3 {
        [
            self.start[0] + self.along[0] * u - self.normal[0] * depth,
            self.start[1] + self.along[1] * u - self.normal[1] * depth,
            z,
        ]
    }
}

struct Site {
    floor: f64,
    wall_top: f64,
    sun: V3,
    solids: Vec<Solid>,
    crowns: Vec<Crown>,
    facades: Vec<Facade>,
}

impl Site {
    fn new() -> Self {
        let floor = terrain(30.5, 26.25) + 0.22;
        let wall_top = floor + WALLS;
        // A low sun from the south-west.
        let azimuth: f64 = 215.0f64.to_radians();
        let elevation: f64 = 38.0f64.to_radians();
        let sun = [
            elevation.cos() * azimuth.sin(),
            elevation.cos() * azimuth.cos(),
            elevation.sin(),
        ];
        let mut site = Self {
            floor,
            wall_top,
            sun,
            solids: Vec::new(),
            crowns: TREES.iter().map(Tree::crown).collect(),
            facades: Vec::new(),
        };
        site.facades = site.facades();
        let ridge = site.roof_z(RIDGE_Y);
        site.solids.push(Solid::cuboid(
            [HOUSE_X[0], HOUSE_Y[0], floor - 1.0],
            [HOUSE_X[1], HOUSE_Y[1], wall_top],
        ));
        // The roof: above the walls and below both slopes.
        let tan = PITCH.tan();
        let south = normalize([0.0, -tan, 1.0]);
        let north = normalize([0.0, tan, 1.0]);
        let eave_south = HOUSE_Y[0] - EAVES;
        let eave_north = HOUSE_Y[1] + EAVES;
        let base = wall_top + FASCIA;
        let below_south = (south, dot(south, [0.0, eave_south, base]));
        let below_north = (north, dot(north, [0.0, eave_north, base]));
        site.solids.push(Solid {
            planes: vec![
                ([-1.0, 0.0, 0.0], -HOUSE_X[0]),
                ([1.0, 0.0, 0.0], HOUSE_X[1]),
                ([0.0, 0.0, -1.0], -wall_top),
                ([0.0, -1.0, 0.0], -eave_south),
                ([0.0, 1.0, 0.0], eave_north),
                below_south,
                below_north,
            ],
            min: [HOUSE_X[0], eave_south, wall_top],
            max: [HOUSE_X[1], eave_north, ridge + 0.2],
        });
        // Beyond the gables the roof is a plate 20 cm thick on each slope.
        for xs in [
            [HOUSE_X[0] - VERGE, HOUSE_X[0]],
            [HOUSE_X[1], HOUSE_X[1] + VERGE],
        ] {
            for (plane, ys) in [
                (below_south, [eave_south, RIDGE_Y]),
                (below_north, [RIDGE_Y, eave_north]),
            ] {
                let (normal, offset) = plane;
                site.solids.push(Solid {
                    planes: vec![
                        ([-1.0, 0.0, 0.0], -xs[0]),
                        ([1.0, 0.0, 0.0], xs[1]),
                        ([0.0, -1.0, 0.0], -ys[0]),
                        ([0.0, 1.0, 0.0], ys[1]),
                        plane,
                        (scale(normal, -1.0), 0.2 - offset),
                    ],
                    min: [xs[0], ys[0], wall_top],
                    max: [xs[1], ys[1], ridge + 0.2],
                });
            }
        }
        site.solids.push(Solid::cuboid(
            [CHIMNEY[0][0], CHIMNEY[1][0], wall_top],
            [CHIMNEY[0][1], CHIMNEY[1][1], ridge + 0.92],
        ));
        // The shed, under its sloping roof.
        let shed_ground = terrain(SHED_X[0], SHED_Y[0]);
        let mut shed = Solid::cuboid(
            [SHED_X[0], SHED_Y[0], shed_ground - 0.3],
            [SHED_X[1], SHED_Y[1], shed_ground + SHED_FRONT],
        );
        let fall = (SHED_REAR - SHED_FRONT) / (SHED_Y[1] - SHED_Y[0]);
        let roof = normalize([0.0, -fall, 1.0]);
        shed.planes
            .push((roof, dot(roof, [0.0, SHED_Y[0], shed_ground + SHED_FRONT])));
        site.solids.push(shed);
        // The hedge and the fences, in parts that follow the slope.
        for (from, to) in [(HEDGE_X[0], HEDGE_GAP[0]), (HEDGE_GAP[1], HEDGE_X[1])] {
            let mut x = from;
            while x < to {
                let next = (x + 3.0).min(to);
                let top = (terrain(x, 9.0) + terrain(next, 9.0)) / 2.0 + HEDGE_HEIGHT - 0.04;
                site.solids.push(Solid::cuboid(
                    [x, HEDGE_Y[0], top - 2.0],
                    [next, HEDGE_Y[1], top],
                ));
                x = next;
            }
        }
        for [from, to] in FENCES {
            let length = (to[0] - from[0]).hypot(to[1] - from[1]);
            let parts = (length / 6.0).ceil() as usize;
            for part in 0..parts {
                let a = part as f64 / parts as f64;
                let b = (part + 1) as f64 / parts as f64;
                let p = [
                    from[0] + (to[0] - from[0]) * a,
                    from[1] + (to[1] - from[1]) * a,
                ];
                let q = [
                    from[0] + (to[0] - from[0]) * b,
                    from[1] + (to[1] - from[1]) * b,
                ];
                let top = terrain(p[0], p[1]).max(terrain(q[0], q[1])) + FENCE_HEIGHT;
                site.solids.push(Solid::cuboid(
                    [p[0].min(q[0]) - 0.02, p[1].min(q[1]) - 0.02, top - 2.5],
                    [p[0].max(q[0]) + 0.02, p[1].max(q[1]) + 0.02, top],
                ));
            }
        }
        site
    }

    fn facades(&self) -> Vec<Facade> {
        let f = self.floor;
        let window = |u: [f64; 2], z: [f64; 2], curtain: f64| Opening {
            u,
            z: [f + z[0], f + z[1]],
            kind: Kind::Window,
            curtain,
        };
        let ground_floor = [0.75, 2.3];
        let upper = [3.55, 5.05];
        let attic = [6.3, 7.2];
        vec![
            Facade {
                id: 1,
                start: [HOUSE_X[0], HOUSE_Y[0]],
                along: [1.0, 0.0, 0.0],
                normal: [0.0, -1.0, 0.0],
                length: HOUSE_X[1] - HOUSE_X[0],
                gable: false,
                openings: vec![
                    window([1.0, 2.8], ground_floor, 0.0),
                    Opening {
                        u: [4.0, 5.0],
                        z: [f, f + 2.35],
                        kind: Kind::Door,
                        curtain: 0.0,
                    },
                    window([5.75, 6.35], [1.45, 2.3], 0.0),
                    window([7.3, 10.0], ground_floor, 0.3),
                    window([1.0, 2.8], upper, 0.55),
                    window([4.05, 4.95], upper, 0.0),
                    window([7.35, 8.5], upper, 0.0),
                    window([8.85, 10.0], upper, 1.0),
                ],
            },
            Facade {
                id: 2,
                start: [HOUSE_X[1], HOUSE_Y[1]],
                along: [-1.0, 0.0, 0.0],
                normal: [0.0, 1.0, 0.0],
                length: HOUSE_X[1] - HOUSE_X[0],
                gable: false,
                openings: vec![
                    Opening {
                        u: [6.0, 9.0],
                        z: [f, f + 2.3],
                        kind: Kind::GlassDoor,
                        curtain: 0.0,
                    },
                    window([1.6, 4.0], [1.0, 2.3], 0.0),
                    window([1.5, 2.7], upper, 0.4),
                    window([4.6, 5.8], upper, 0.0),
                    window([8.3, 9.5], upper, 0.7),
                ],
            },
            Facade {
                id: 3,
                start: [HOUSE_X[0], HOUSE_Y[1]],
                along: [0.0, -1.0, 0.0],
                normal: [-1.0, 0.0, 0.0],
                length: HOUSE_Y[1] - HOUSE_Y[0],
                gable: true,
                openings: vec![
                    window([3.0, 4.2], ground_floor, 0.0),
                    window([3.0, 4.2], upper, 0.25),
                    window([3.85, 4.65], attic, 0.0),
                ],
            },
            Facade {
                id: 4,
                start: [HOUSE_X[1], HOUSE_Y[0]],
                along: [0.0, 1.0, 0.0],
                normal: [1.0, 0.0, 0.0],
                length: HOUSE_Y[1] - HOUSE_Y[0],
                gable: true,
                openings: vec![
                    window([5.3, 6.3], [1.0, 2.3], 0.0),
                    window([2.0, 3.2], upper, 0.0),
                    window([5.3, 6.5], upper, 0.5),
                    window([3.85, 4.65], attic, 0.0),
                ],
            },
        ]
    }

    /// Height of the roof surface above a line across the house.
    fn roof_z(&self, y: f64) -> f64 {
        let run = (RIDGE_Y - (HOUSE_Y[0] - EAVES)) - (y - RIDGE_Y).abs();
        self.wall_top + FASCIA + run * PITCH.tan()
    }

    /// How much direct sunlight reaches a point: 0 in full shadow.
    fn sun_visibility(&self, from: V3) -> f64 {
        if self.solids.iter().any(|solid| solid.hit(from, self.sun)) {
            return 0.0;
        }
        let mut light = 1.0;
        for crown in &self.crowns {
            let chord = crown.chord(from, self.sun);
            if chord > 0.0 {
                let through = (-0.42 * chord).exp();
                let fleck = smoothstep(0.58, 0.8, noise3(SEED ^ 3, scale(from, 1.3)));
                light *= through + (1.0 - through) * 0.3 * fleck;
            }
        }
        light
    }

    /// The colour of a surface of colour `albedo` lit by the sky, which
    /// `ao` dims, and by the sun, which `sun` (0 to 1) lets through.
    fn lit(&self, albedo: Rgb, normal: V3, ao: f64, sun: f64) -> Rgb {
        let facing = dot(normal, self.sun).max(0.0);
        let sky = 0.6 * ao * (0.4 + 0.6 * (0.5 + 0.5 * normal[2]));
        let direct = 0.75 * facing * sun;
        std::array::from_fn(|channel| {
            let light = SKY[channel] * sky + SUNLIGHT[channel] * direct;
            let value = albedo[channel] * light;
            (value * 1.15 / (1.0 + 0.15 * value)).min(1.0)
        })
    }

    /// `lit` with the sun traced from just in front of the surface.
    fn shade(&self, albedo: Rgb, p: V3, normal: V3, ao: f64) -> Rgb {
        let sun = if dot(normal, self.sun) > 0.0 {
            self.sun_visibility(along(p, normal, 0.03))
        } else {
            0.0
        };
        self.lit(albedo, normal, ao, sun)
    }

    /// Sunlight on a point `depth` deep in an opening: it must come in
    /// through the opening, and then reach it from outside.
    fn opening_sun(&self, facade: &Facade, opening: &Opening, u: f64, z: f64, depth: f64) -> f64 {
        let out = dot(self.sun, facade.normal);
        if out <= 0.0 {
            return 0.0;
        }
        let t = depth / out;
        let at_u = u + t * dot(self.sun, facade.along);
        let at_z = z + t * self.sun[2];
        if !in_range(at_u, opening.u) || !in_range(at_z, opening.z) {
            return 0.0;
        }
        self.sun_visibility(facade.at(at_u, at_z, -0.02))
    }

    /// Light the sky gives the ground near walls, hedges, fences and trees.
    fn ground_ao(&self, x: f64, y: f64) -> f64 {
        let mut ao = 1.0;
        let house = distance_to_rect(x, y, HOUSE_X, HOUSE_Y);
        ao *= 1.0 - 0.3 * (-house / 0.6).exp();
        let shed = distance_to_rect(x, y, SHED_X, SHED_Y);
        ao *= 1.0 - 0.25 * (-shed / 0.4).exp();
        if !in_range(x, HEDGE_GAP) && in_range(x, HEDGE_X) {
            let hedge = distance_to_rect(x, y, HEDGE_X, HEDGE_Y);
            ao *= 1.0 - 0.35 * (-hedge / 0.35).exp();
        }
        for [from, to] in FENCES {
            let gap = if from[0] == to[0] {
                (x - from[0]).abs()
            } else {
                (y - from[1]).abs()
            };
            ao *= 1.0 - 0.22 * (-gap / 0.25).exp();
        }
        for tree in &TREES {
            let reach = (x - tree.x).hypot(y - tree.y) / tree.crown_radius;
            ao *= 1.0 - 0.35 * (-1.5 * reach * reach).exp();
        }
        ao
    }
}

/// Jittered samples of a `width` by `height` rectangle, one in every cell of
/// `spacing`.
fn grid(
    rng: &mut Rng,
    width: f64,
    height: f64,
    spacing: f64,
    mut f: impl FnMut(&mut Rng, f64, f64),
) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let columns = (width / spacing).ceil().max(1.0) as usize;
    let rows = (height / spacing).ceil().max(1.0) as usize;
    let (cell_u, cell_v) = (width / columns as f64, height / rows as f64);
    for column in 0..columns {
        for row in 0..rows {
            let u = (column as f64 + rng.unit()) * cell_u;
            let v = (row as f64 + rng.unit()) * cell_v;
            f(rng, u, v);
        }
    }
}

/// Colour noise of a scanner's camera: a little brighter or darker, and a
/// little per channel.
fn grain(rng: &mut Rng, colour: Rgb, amount: f64) -> Rgb {
    let common = 1.0 + amount * rng.normal();
    colour.map(|value| (value * common * (1.0 + 0.012 * rng.normal())).clamp(0.0, 1.0))
}

fn push(out: &mut Vec<Sample>, xyz: V3, rgb: Rgb, intensity: f64, class: u8) {
    out.push(Sample {
        xyz: [xyz[0] + ORIGIN[0], xyz[1] + ORIGIN[1], xyz[2] + ORIGIN[2]],
        rgb,
        intensity: intensity.clamp(0.0, 1.0),
        class,
        source: 0,
    });
}

pub fn write(path: &Path) -> Result<u64, Failure> {
    let site = Site::new();
    let jobs = jobs(&site);
    let mut writer = las_writer(path, true, ORIGIN)?;
    let written = run_jobs(&jobs, SEED, |samples| write_samples(&mut writer, samples))?;
    writer.close()?;
    Ok(written)
}

fn jobs(site: &Site) -> Vec<Job<'_>> {
    let mut jobs: Vec<Job> = Vec::new();
    let tile = 4.0;
    let columns = (SITE[0] / tile).ceil() as usize;
    let rows = (SITE[1] / tile).ceil() as usize;
    for row in 0..rows {
        for column in 0..columns {
            let x0 = column as f64 * tile;
            let y0 = row as f64 * tile;
            jobs.push(Box::new(move |rng, out| {
                ground_tile(site, rng, out, x0, y0, tile)
            }));
        }
    }
    jobs.push(Box::new(|rng, out| hardscape(site, rng, out)));
    for (index, facade) in site.facades.iter().enumerate() {
        let strips = facade.length.ceil() as usize;
        for strip in 0..strips {
            let from = strip as f64;
            let to = (from + 1.0).min(facade.length);
            jobs.push(Box::new(move |rng, out| {
                wall_strip(site, &site.facades[index], rng, out, from, to)
            }));
        }
        jobs.push(Box::new(move |rng, out| {
            let facade = &site.facades[index];
            for opening in &facade.openings {
                opening_points(site, facade, opening, rng, out);
            }
        }));
    }
    for north in [false, true] {
        for half in 0..4 {
            jobs.push(Box::new(move |rng, out| {
                roof_slope(site, rng, out, north, half)
            }));
        }
    }
    jobs.push(Box::new(|rng, out| ridge_and_chimney(site, rng, out)));
    jobs.push(Box::new(|rng, out| eaves(site, rng, out)));
    for (index, tree) in TREES.iter().enumerate() {
        if tree.species == Species::Spruce {
            for part in 0..4 {
                jobs.push(Box::new(move |rng, out| {
                    spruce(site, &TREES[index], rng, out, part)
                }));
            }
        } else {
            for _ in 0..4 {
                jobs.push(Box::new(move |rng, out| {
                    foliage(site, &TREES[index], rng, out)
                }));
            }
        }
        jobs.push(Box::new(move |rng, out| {
            wood(site, &TREES[index], rng, out)
        }));
    }
    jobs.push(Box::new(|rng, out| shrubs(site, rng, out)));
    for (from, to) in [(HEDGE_X[0], HEDGE_GAP[0]), (HEDGE_GAP[1], HEDGE_X[1])] {
        let mut x = from;
        while x < to - 1e-9 {
            let next = (x + 5.0).min(to);
            let (first, last) = (x == from, next == to);
            jobs.push(Box::new(move |rng, out| {
                hedge(site, rng, out, x, next, first, last)
            }));
            x = next;
        }
    }
    for (line, [from, to]) in FENCES.iter().enumerate() {
        let length = (to[0] - from[0]).hypot(to[1] - from[1]);
        let panels = (length / 1.8).round() as usize;
        for first in (0..panels).step_by(5) {
            let last = (first + 5).min(panels);
            jobs.push(Box::new(move |rng, out| {
                fence(site, rng, out, line, first, last, panels)
            }));
        }
    }
    jobs.push(Box::new(|rng, out| shed(site, rng, out)));
    jobs.push(Box::new(|rng, out| street_light(site, rng, out)));
    jobs
}

/// What covers the ground at a point outside the house.
#[derive(Clone, Copy, PartialEq)]
enum Ground {
    Street,
    Curb,
    Sidewalk,
    Path,
    Terrace,
    Gravel,
    Bed,
    UnderHedge,
    ShedFloor,
    Grass,
}

fn ground_kind(x: f64, y: f64) -> Ground {
    if y < STREET_Y {
        Ground::Street
    } else if y < SIDEWALK_Y[0] {
        Ground::Curb
    } else if y < SIDEWALK_Y[1] {
        Ground::Sidewalk
    } else if in_range(x, SHED_X) && in_range(y, SHED_Y) {
        Ground::ShedFloor
    } else if in_range(x, PATH_X) && y <= HOUSE_Y[0] {
        Ground::Path
    } else if in_range(x, TERRACE[0]) && in_range(y, TERRACE[1]) {
        Ground::Terrace
    } else if distance_to_rect(x, y, HOUSE_X, HOUSE_Y) < 0.38 {
        Ground::Gravel
    } else if in_range(y, BED_Y)
        && (in_range(x, [HOUSE_X[0] + 0.35, PATH_X[0] - 0.15])
            || in_range(x, [PATH_X[1] + 0.15, HOUSE_X[1] - 0.35]))
    {
        Ground::Bed
    } else if in_range(y, HEDGE_Y) && in_range(x, HEDGE_X) && !in_range(x, HEDGE_GAP) {
        Ground::UnderHedge
    } else {
        Ground::Grass
    }
}

fn ground_tile(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>, x0: f64, y0: f64, size: f64) {
    let width = size.min(SITE[0] - x0);
    let height = size.min(SITE[1] - y0);
    grid(rng, width, height, 0.017, |rng, du, dv| {
        let (x, y) = (x0 + du, y0 + dv);
        if in_range(x, HOUSE_X) && in_range(y, HOUSE_Y) {
            return;
        }
        if in_range(x, DOORSTEP[0]) && in_range(y, DOORSTEP[1]) {
            return;
        }
        let kind = ground_kind(x, y);
        if kind == Ground::ShedFloor {
            return;
        }
        let ground = terrain(x, y);
        let mut normal = [0.0, 0.0, 1.0];
        let (z, albedo, intensity, class) = match kind {
            Ground::Street => {
                let z = street_z(x, y);
                let (albedo, intensity) = asphalt(rng, x, y);
                (z, albedo, intensity, class::GROUND)
            }
            Ground::Curb => {
                let joint = (x % 1.0) < 0.006;
                let shade = 0.9 + 0.2 * hash(SEED ^ 40, (x / 1.0).floor() as i64, 0, 0);
                let albedo = if joint {
                    [0.3, 0.3, 0.29]
                } else {
                    [0.6 * shade, 0.59 * shade, 0.56 * shade]
                };
                (curb_top(x), albedo, 0.45, class::GROUND)
            }
            Ground::Sidewalk => {
                let (albedo, relief) =
                    tiles(rng, x, y - SIDEWALK_Y[0], 0.3, 0.003, [0.6, 0.59, 0.56], 41);
                (sidewalk_z(x, y) + relief, albedo, 0.42, class::GROUND)
            }
            Ground::Path => {
                let (albedo, relief) = pavers(rng, x - PATH_X[0], y);
                (ground + 0.02 + relief, albedo, 0.38, class::GROUND)
            }
            Ground::Terrace => {
                let (albedo, relief) = tiles(
                    rng,
                    x - TERRACE[0][0],
                    y - TERRACE[1][0],
                    0.6,
                    0.005,
                    [0.64, 0.63, 0.6],
                    42,
                );
                (site.floor - 0.16 + relief, albedo, 0.45, class::GROUND)
            }
            Ground::Gravel => {
                let pick = rng.unit();
                let albedo = if pick < 0.4 {
                    [0.63, 0.6, 0.55]
                } else if pick < 0.75 {
                    [0.5, 0.48, 0.44]
                } else {
                    [0.72, 0.69, 0.62]
                };
                normal = normalize([rng.normal() * 0.4, rng.normal() * 0.4, 1.0]);
                (
                    ground + 0.015 + 0.015 * rng.unit(),
                    albedo,
                    0.4,
                    class::GROUND,
                )
            }
            Ground::Bed => {
                let chips = noise2(SEED ^ 43, x * 9.0, y * 9.0);
                let albedo = mix([0.22, 0.16, 0.12], [0.36, 0.27, 0.19], chips);
                (
                    ground + 0.03 + 0.01 * rng.unit(),
                    albedo,
                    0.3,
                    class::GROUND,
                )
            }
            Ground::UnderHedge => {
                let albedo = mix([0.24, 0.2, 0.15], [0.33, 0.28, 0.18], rng.unit());
                (ground + 0.02, albedo, 0.3, class::GROUND)
            }
            Ground::Grass | Ground::ShedFloor => {
                let albedo = grass(rng, x, y);
                normal = normalize([rng.normal() * 0.35, rng.normal() * 0.35, 1.0]);
                let blade = rng.unit();
                (
                    ground + 0.008 + 0.035 * blade * blade.sqrt(),
                    albedo,
                    0.5 + 0.1 * rng.normal(),
                    class::GROUND,
                )
            }
        };
        let p = [x, y, z + 0.0015 * rng.normal()];
        let ao = site.ground_ao(x, y);
        let sun = site.sun_visibility(along(p, [0.0, 0.0, 1.0], 0.03));
        let colour = site.lit(albedo, normal, ao, sun);
        push(
            out,
            p,
            grain(rng, colour, 0.03),
            intensity + 0.03 * rng.normal(),
            class,
        );
    });
}

fn asphalt(rng: &mut Rng, x: f64, y: f64) -> (Rgb, f64) {
    // A centre line of dashes, a little worn.
    if (y - 3.0).abs() < 0.05
        && x.rem_euclid(4.5) < 1.5
        && noise2(SEED ^ 44, x * 6.0, y * 6.0) > 0.22
    {
        return ([0.84, 0.84, 0.8], 0.78);
    }
    // A manhole cover.
    let cover = (x - 18.0).hypot(y - 1.8);
    if cover < 0.32 {
        let ring = if (cover * 40.0).sin() > 0.3 {
            0.85
        } else {
            1.0
        };
        return ([0.2 * ring, 0.19 * ring, 0.18 * ring], 0.2);
    }
    // Concrete gutter stones along the curb.
    if y > STREET_Y - 0.3 {
        let joint = (x % 0.5) < 0.006;
        let albedo = if joint { [0.3; 3] } else { [0.47, 0.46, 0.44] };
        return (albedo, 0.38);
    }
    let mut albedo: Rgb = [0.24, 0.24, 0.25];
    if rng.chance(0.07) {
        albedo = [0.48, 0.47, 0.45];
    }
    let stains = 0.84 + 0.32 * fbm2(SEED ^ 45, x / 3.0, y / 3.0, 3);
    let track = 1.0 - 0.08 * ((-(y - 1.5).powi(2) / 0.1).exp() + (-(y - 4.5).powi(2) / 0.1).exp());
    let repair = if in_range(x, [40.0, 43.5]) && in_range(y, [3.4, 5.2]) {
        0.72
    } else {
        1.0
    };
    let speck = 0.8 + 0.4 * rng.unit();
    let factor = stains * track * repair * speck;
    (albedo.map(|value| value * factor), 0.1 + 0.05 * rng.unit())
}

/// Square tiles of `size` with joints of `joint`; returns the colour and a
/// small rise or fall of the tile.
fn tiles(rng: &mut Rng, u: f64, v: f64, size: f64, joint: f64, base: Rgb, seed: u64) -> (Rgb, f64) {
    let (i, j) = ((u / size).floor(), (v / size).floor());
    let (a, b) = (u - i * size, v - j * size);
    if a < joint || b < joint {
        return (
            mix([0.35, 0.33, 0.3], [0.45, 0.42, 0.37], rng.unit()),
            -0.003,
        );
    }
    let h = hash(SEED ^ seed, i as i64, j as i64, 0);
    let stain = 0.9 + 0.12 * fbm2(SEED ^ seed ^ 7, u / 1.7, v / 1.7, 3);
    let shade = (0.92 + 0.14 * h) * stain;
    let lichen = if noise2(SEED ^ seed ^ 9, u * 14.0, v * 14.0) > 0.86 {
        0.7
    } else {
        1.0
    };
    let tilt = 0.002 * (hash(SEED ^ seed, i as i64, j as i64, 1) - 0.5);
    (base.map(|value| value * shade * lichen), tilt)
}

/// Concrete pavers of mixed colours in a stretcher bond along the path.
fn pavers(rng: &mut Rng, u: f64, y: f64) -> (Rgb, f64) {
    let width = PATH_X[1] - PATH_X[0];
    if u < 0.06 || u > width - 0.06 {
        return ([0.55, 0.54, 0.51], 0.004);
    }
    let row = (y / 0.105).floor();
    let shift = if row as i64 % 2 == 0 { 0.0 } else { 0.105 };
    let column = ((u - 0.06 + shift) / 0.21).floor();
    let along_row = u - 0.06 + shift - column * 0.21;
    let across = y - row * 0.105;
    if along_row < 0.004 || across < 0.004 {
        return (
            mix([0.42, 0.38, 0.32], [0.5, 0.46, 0.38], rng.unit()),
            -0.002,
        );
    }
    let h = hash(SEED ^ 46, column as i64, row as i64, 0);
    let palette: [Rgb; 4] = [
        [0.5, 0.35, 0.29],
        [0.44, 0.39, 0.37],
        [0.57, 0.47, 0.39],
        [0.4, 0.3, 0.27],
    ];
    let base = palette[(h * 4.0) as usize % 4];
    let shade = 0.9 + 0.2 * hash(SEED ^ 46, column as i64, row as i64, 1);
    (base.map(|value| value * shade), 0.0)
}

fn grass(rng: &mut Rng, x: f64, y: f64) -> Rgb {
    let patch = fbm2(SEED ^ 47, x / 7.0, y / 7.0, 4);
    let mut colour = mix([0.24, 0.37, 0.14], [0.4, 0.47, 0.21], patch);
    // Mowing stripes on the lawns of the plot.
    if in_range(x, [HEDGE_X[0], HEDGE_X[1]]) && in_range(y, [HEDGE_Y[1], 47.5]) {
        let stripe = if (x / 0.9).floor() as i64 % 2 == 0 {
            1.06
        } else {
            0.95
        };
        colour = colour.map(|value| value * stripe);
    }
    let clover = smoothstep(0.62, 0.7, noise2(SEED ^ 48, x * 1.6, y * 1.6));
    colour = mix(colour, [0.19, 0.32, 0.13], clover * 0.7);
    if rng.chance(0.05) {
        colour = mix(colour, [0.56, 0.52, 0.31], 0.7);
    }
    let blade = 0.78 + 0.44 * rng.unit();
    colour.map(|value| value * blade)
}

/// The vertical faces of the curb, the terrace and the doorstep.
fn hardscape(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>) {
    let spacing = 0.008;
    // The face of the curb towards the street.
    grid(rng, SITE[0], 0.2, spacing, |rng, x, v| {
        let (bottom, top) = (street_z(x, STREET_Y), curb_top(x));
        let z = top - v;
        if z < bottom {
            return;
        }
        let p = [x, STREET_Y + 0.002 * rng.normal(), z];
        let albedo = [0.56, 0.55, 0.52];
        let colour = site.shade(albedo, p, [0.0, -1.0, 0.0], 0.85);
        push(out, p, grain(rng, colour, 0.03), 0.42, class::GROUND);
    });
    // The edges of the terrace where the lawn lies lower.
    let level = site.floor - 0.16;
    let edges: [([f64; 2], [f64; 2], V3); 3] = [
        (
            [TERRACE[0][0], TERRACE[1][1]],
            [TERRACE[0][1], TERRACE[1][1]],
            [0.0, 1.0, 0.0],
        ),
        (
            [TERRACE[0][0], TERRACE[1][0]],
            [TERRACE[0][0], TERRACE[1][1]],
            [-1.0, 0.0, 0.0],
        ),
        (
            [TERRACE[0][1], TERRACE[1][0]],
            [TERRACE[0][1], TERRACE[1][1]],
            [1.0, 0.0, 0.0],
        ),
    ];
    for (from, to, normal) in edges {
        let length = (to[0] - from[0]).hypot(to[1] - from[1]);
        grid(rng, length, 0.3, spacing, |rng, s, v| {
            let t = s / length;
            let (x, y) = (
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            );
            let z = level - v;
            if z < terrain(x, y) {
                return;
            }
            let p = [x, y, z];
            let colour = site.shade([0.58, 0.57, 0.54], p, normal, 0.8);
            push(out, p, grain(rng, colour, 0.03), 0.45, class::GROUND);
        });
    }
    // The doorstep: its top, its front and its ends.
    let step_top = site.floor - 0.02;
    let [xs, ys] = DOORSTEP;
    let stone = |rng: &mut Rng, x: f64, y: f64| {
        let base = [0.47, 0.49, 0.51];
        let fleck = 0.9 + 0.2 * noise2(SEED ^ 49, x * 30.0, y * 30.0);
        grain(rng, base.map(|value| value * fleck), 0.02)
    };
    grid(rng, xs[1] - xs[0], ys[1] - ys[0], spacing, |rng, u, v| {
        let p = [xs[0] + u, ys[0] + v, step_top];
        let albedo = stone(rng, p[0], p[1]);
        let colour = site.shade(albedo, p, [0.0, 0.0, 1.0], 0.9);
        push(out, p, colour, 0.4, class::BUILDING);
    });
    grid(rng, xs[1] - xs[0], 0.4, spacing, |rng, u, v| {
        let (x, z) = (xs[0] + u, step_top - v);
        if z < terrain(x, ys[0]) {
            return;
        }
        let p = [x, ys[0], z];
        let albedo = stone(rng, x, z);
        let colour = site.shade(albedo, p, [0.0, -1.0, 0.0], 0.85);
        push(out, p, colour, 0.4, class::BUILDING);
    });
    for (x, normal) in [(xs[0], [-1.0, 0.0, 0.0]), (xs[1], [1.0, 0.0, 0.0])] {
        grid(rng, ys[1] - ys[0], 0.4, spacing, |rng, v, w| {
            let (y, z) = (ys[0] + v, step_top - w);
            if z < terrain(x, y) {
                return;
            }
            let p = [x, y, z];
            let albedo = stone(rng, y, z);
            let colour = site.shade(albedo, p, normal, 0.8);
            push(out, p, colour, 0.4, class::BUILDING);
        });
    }
}

/// The colour of brickwork in a stretcher bond, with dark bricks along the
/// foot of the wall and upright bricks above openings. Returns the colour,
/// how far the point lies behind the face (mortar is raked back) and its
/// return strength.
fn brick(site: &Site, facade: &Facade, u: f64, z: f64) -> (Rgb, f64, f64) {
    let course_height = 0.0625;
    let v = z - (site.floor - 0.5);
    // Upright bricks above openings.
    for opening in &facade.openings {
        let z1 = opening.z[1];
        if in_range(u, [opening.u[0] - 0.06, opening.u[1] + 0.06]) && in_range(z, [z1, z1 + 0.22]) {
            let a = u - (opening.u[0] - 0.06);
            let index = (a / 0.065).floor();
            let within = a - index * 0.065;
            if within < 0.01 || z - z1 > 0.21 {
                return (mortar(u, z), 0.006, 0.55);
            }
            let h = hash(SEED ^ facade.id, index as i64, -7, 0);
            let colour = brick_colour(h, hash(SEED ^ facade.id, index as i64, -7, 1));
            return (colour.map(|value| value * 0.88), 0.0, 0.4);
        }
    }
    let course = (v / course_height).floor();
    let in_course = v - course * course_height;
    let shift = if course as i64 % 2 == 0 { 0.0 } else { 0.11 };
    let w = u + shift + facade.id as f64 * 0.037;
    let index = (w / 0.22).floor();
    let in_brick = w - index * 0.22;
    if in_course < 0.0125 || in_brick < 0.01 {
        return (mortar(u, z), 0.006, 0.55);
    }
    let h = hash(SEED ^ facade.id, index as i64, course as i64, 0);
    let mut colour = brick_colour(h, hash(SEED ^ facade.id, index as i64, course as i64, 1));
    if z < site.floor + 0.3 {
        colour = colour.map(|value| value * 0.62);
    }
    let surface = 0.94 + 0.12 * noise2(SEED ^ 50, u * 40.0, z * 40.0);
    (colour.map(|value| value * surface), 0.0, 0.38 + 0.08 * h)
}

fn brick_colour(h: f64, pick: f64) -> Rgb {
    let palette: [Rgb; 5] = [
        [0.55, 0.29, 0.21],
        [0.5, 0.26, 0.2],
        [0.6, 0.34, 0.24],
        [0.44, 0.24, 0.2],
        [0.57, 0.31, 0.25],
    ];
    let base = palette[(pick * 5.0) as usize % 5];
    base.map(|value| value * (0.86 + 0.26 * h))
}

fn mortar(u: f64, z: f64) -> Rgb {
    let shade = 0.9 + 0.15 * noise2(SEED ^ 51, u * 25.0, z * 25.0);
    [0.68 * shade, 0.66 * shade, 0.62 * shade]
}

/// The highest point of a wall at `u`: the eaves, or the roof on a gable.
fn wall_top(site: &Site, facade: &Facade, u: f64) -> f64 {
    if facade.gable {
        let y = facade.at(u, 0.0, 0.0)[1];
        (site.roof_z(y) - 0.07).max(site.wall_top)
    } else {
        site.wall_top
    }
}

fn wall_strip(
    site: &Site,
    facade: &Facade,
    rng: &mut Rng,
    out: &mut Vec<Sample>,
    from: f64,
    to: f64,
) {
    let bottom = site.floor - 0.6;
    let top = (0..=4)
        .map(|step| wall_top(site, facade, from + (to - from) * step as f64 / 4.0))
        .fold(site.wall_top, f64::max);
    grid(rng, to - from, top - bottom, 0.008, |rng, du, dz| {
        let (u, z) = (from + du, bottom + dz);
        if z > wall_top(site, facade, u) {
            return;
        }
        let face = facade.at(u, z, 0.0);
        let ground = terrain(face[0], face[1]);
        if z < ground {
            return;
        }
        for opening in &facade.openings {
            let [u0, u1] = opening.u;
            let [z0, z1] = opening.z;
            if in_range(u, [u0, u1]) && in_range(z, [z0, z1]) {
                return;
            }
            // Behind a sill or the doorstep.
            if opening.kind == Kind::Window
                && in_range(u, [u0 - 0.05, u1 + 0.05])
                && in_range(z, [z0 - 0.095, z0])
            {
                return;
            }
        }
        if facade.id == 1 && in_range(face[0], DOORSTEP[0]) && z < site.floor - 0.02 {
            return;
        }
        let (mut albedo, behind, intensity) = brick(site, facade, u, z);
        // Streaks of dirt under the sills.
        for opening in &facade.openings {
            let [u0, u1] = opening.u;
            let z0 = opening.z[0];
            if opening.kind == Kind::Window && in_range(u, [u0, u1]) && z < z0 && z > z0 - 1.4 {
                let streak = noise2(SEED ^ 52, u * 9.0, 0.0);
                let fade = (-(z0 - z) / 0.6).exp();
                albedo = albedo.map(|value| value * (1.0 - 0.18 * fade * streak));
            }
        }
        let height = z - ground;
        let mut ao = 1.0 - 0.25 * (-height / 0.4).exp();
        if !facade.gable {
            ao *= 1.0 - 0.35 * (-(site.wall_top - z) / 0.5).exp();
        }
        let p = facade.at(u, z, behind + 0.0015 * rng.normal());
        let colour = site.shade(albedo, p, facade.normal, ao);
        push(
            out,
            p,
            grain(rng, colour, 0.03),
            intensity + 0.03 * rng.normal(),
            class::BUILDING,
        );
    });
}

/// The reveal, sill, frame, glass, door and curtain of one opening.
fn opening_points(
    site: &Site,
    facade: &Facade,
    opening: &Opening,
    rng: &mut Rng,
    out: &mut Vec<Sample>,
) {
    let spacing = 0.008;
    let [u0, u1] = opening.u;
    let [z0, z1] = opening.z;
    let (width, height) = (u1 - u0, z1 - z0);
    let lit = |site: &Site, albedo: Rgb, normal: V3, u: f64, z: f64, depth: f64, ao: f64| {
        let sun = if dot(normal, site.sun) > 0.0 {
            site.opening_sun(facade, opening, u, z, depth)
        } else {
            0.0
        };
        site.lit(albedo, normal, ao, sun)
    };
    let back = scale(facade.along, -1.0);
    // The sides of the reveal.
    for (u, normal, sign) in [(u0, facade.along, -1.0), (u1, back, 1.0)] {
        grid(rng, REVEAL, height, spacing, |rng, depth, dz| {
            let z = z0 + dz;
            let (albedo, _, intensity) = brick(site, facade, u + sign * (REVEAL - depth), z);
            let ao = 1.0 - 0.35 * depth / REVEAL;
            let colour = lit(site, albedo, normal, u, z, depth, ao);
            let p = along(facade.at(u, z, depth), normal, 0.0015 * rng.normal());
            push(out, p, grain(rng, colour, 0.03), intensity, class::BUILDING);
        });
    }
    // The steel lintel above it.
    grid(rng, width, REVEAL, spacing, |rng, du, depth| {
        let u = u0 + du;
        let ao = 0.75 - 0.2 * depth / REVEAL;
        let colour = lit(site, [0.33, 0.34, 0.35], [0.0, 0.0, -1.0], u, z1, depth, ao);
        push(
            out,
            facade.at(u, z1, depth),
            grain(rng, colour, 0.02),
            0.3,
            class::BUILDING,
        );
    });
    let stone = |rng: &mut Rng, u: f64, z: f64| {
        let fleck = 0.9 + 0.2 * noise2(SEED ^ 53, u * 30.0, z * 30.0);
        grain(rng, [0.46 * fleck, 0.48 * fleck, 0.5 * fleck], 0.02)
    };
    if opening.kind == Kind::Window {
        // A stone sill that slopes outward and stands out of the wall.
        let outer = -0.06;
        let sill_z = |depth: f64| z0 - 0.045 + 0.04 * (depth - outer) / (REVEAL - outer);
        let slope = normalize(along([0.0, 0.0, 1.0], facade.normal, 0.25));
        grid(rng, width + 0.1, REVEAL - outer, spacing, |rng, du, dd| {
            let (u, depth) = (u0 - 0.05 + du, outer + dd);
            if depth > 0.0 && !in_range(u, opening.u) {
                return;
            }
            let z = sill_z(depth);
            let albedo = stone(rng, u, depth);
            let colour = lit(site, albedo, slope, u, z, depth.max(0.0), 0.95);
            push(out, facade.at(u, z, depth), colour, 0.4, class::BUILDING);
        });
        grid(rng, width + 0.1, 0.05, spacing, |rng, du, dz| {
            let (u, z) = (u0 - 0.05 + du, z0 - 0.095 + dz);
            let albedo = stone(rng, u, z);
            let colour = site.shade(albedo, facade.at(u, z, outer), facade.normal, 0.9);
            push(out, facade.at(u, z, outer), colour, 0.4, class::BUILDING);
        });
        for (u, normal) in [(u0 - 0.05, back), (u1 + 0.05, facade.along)] {
            grid(rng, -outer, 0.05, spacing, |rng, dd, dz| {
                let (depth, z) = (outer + dd, z0 - 0.095 + dz);
                let albedo = stone(rng, depth, z);
                let colour = site.shade(albedo, facade.at(u, z, depth), normal, 0.85);
                push(out, facade.at(u, z, depth), colour, 0.4, class::BUILDING);
            });
        }
    } else {
        // A threshold of the same stone.
        grid(rng, width, REVEAL + 0.04, spacing, |rng, du, dd| {
            let (u, depth) = (u0 + du, dd - 0.04);
            let albedo = stone(rng, u, depth);
            let colour = lit(site, albedo, [0.0, 0.0, 1.0], u, z0, depth.max(0.0), 0.9);
            push(
                out,
                facade.at(u, z0 - 0.005, depth),
                colour,
                0.4,
                class::BUILDING,
            );
        });
    }
    // Frame, sashes, glass and door leaf, seen from outside.
    let frame_width = if opening.kind == Kind::Window {
        0.065
    } else {
        0.075
    };
    let mullions: Vec<f64> = match opening.kind {
        Kind::Window if width > 1.3 => vec![width / 2.0],
        Kind::GlassDoor => vec![width / 3.0, 2.0 * width / 3.0],
        _ => Vec::new(),
    };
    let leaf_top = 2.0;
    grid(rng, width, height, spacing, |rng, a, b| {
        let (u, z) = (u0 + a, z0 + b);
        let edge = a.min(width - a).min(b).min(height - b);
        let mullion = mullions
            .iter()
            .map(|m| (a - m).abs())
            .fold(f64::INFINITY, f64::min);
        let transom = if opening.kind == Kind::Door {
            (b - leaf_top - 0.035).abs()
        } else {
            f64::INFINITY
        };
        let white = [0.9, 0.9, 0.88];
        let facing = facade.normal;
        if edge < frame_width || mullion < 0.04 || transom < 0.035 {
            let colour = lit(site, white, facing, u, z, REVEAL, 0.78);
            push(
                out,
                facade.at(u, z, REVEAL),
                grain(rng, colour, 0.02),
                0.78,
                class::BUILDING,
            );
            return;
        }
        if opening.kind == Kind::Door && b < leaf_top {
            let (albedo, rise, intensity) =
                door_leaf(a - frame_width, b, width - 2.0 * frame_width);
            let depth = REVEAL + 0.03 - rise;
            let colour = lit(site, albedo, facing, u, z, depth, 0.72);
            push(
                out,
                facade.at(u, z, depth),
                grain(rng, colour, 0.02),
                intensity,
                class::BUILDING,
            );
            return;
        }
        // The sash around each pane.
        let pane_edge =
            edge.min(mullion).min(transom) - if mullion < transom { 0.04 } else { 0.035 };
        let sash = if opening.kind == Kind::GlassDoor {
            0.07
        } else {
            0.05
        };
        if pane_edge < sash {
            let colour = lit(site, [0.86, 0.86, 0.84], facing, u, z, REVEAL + 0.015, 0.72);
            push(
                out,
                facade.at(u, z, REVEAL + 0.015),
                grain(rng, colour, 0.02),
                0.72,
                class::BUILDING,
            );
            return;
        }
        // Glass returns little; it mirrors the sky and the trees.
        if rng.chance(0.13) {
            let up = (b / height).clamp(0.0, 1.0);
            let mut colour = mix([0.13, 0.15, 0.18], [0.44, 0.5, 0.58], up.powf(1.3));
            let trees = smoothstep(0.52, 0.68, fbm2(SEED ^ facade.id ^ 54, u / 1.3, z / 1.3, 3));
            colour = mix(colour, [0.15, 0.19, 0.15], trees * 0.6);
            push(
                out,
                facade.at(u, z, REVEAL + 0.04),
                grain(rng, colour, 0.04),
                0.04,
                class::BUILDING,
            );
        }
        // A curtain behind the glass covers the top of some windows.
        let covered = b > height * (1.0 - opening.curtain);
        if opening.curtain > 0.0 && covered && rng.chance(0.45) {
            let fold = 0.8 + 0.2 * (TAU * a / 0.11).sin();
            let colour = [0.78, 0.74, 0.66].map(|value| value * fold * 0.5);
            push(
                out,
                facade.at(u, z, REVEAL + 0.22),
                grain(rng, colour, 0.03),
                0.6,
                class::BUILDING,
            );
        }
    });
}

/// A panelled front door, painted dark green, with a letterbox and a
/// handle of brass. Returns the colour, how far the point stands out and its
/// return strength.
fn door_leaf(a: f64, b: f64, width: f64) -> (Rgb, f64, f64) {
    let green = [0.08, 0.22, 0.16];
    let brass = [0.62, 0.52, 0.3];
    if in_range(a, [width / 2.0 - 0.15, width / 2.0 + 0.15]) && in_range(b, [0.84, 0.9]) {
        return (brass, 0.003, 0.6);
    }
    if in_range(a, [width - 0.12, width - 0.06]) && in_range(b, [0.98, 1.08]) {
        return (brass, 0.05, 0.6);
    }
    let columns = [
        [0.11, width / 2.0 - 0.04],
        [width / 2.0 + 0.04, width - 0.11],
    ];
    let rows = [[0.15, 0.78], [0.96, 1.86]];
    for column in columns {
        for row in rows {
            if in_range(a, column) && in_range(b, row) {
                let edge = (a - column[0])
                    .min(column[1] - a)
                    .min(b - row[0])
                    .min(row[1] - b);
                let bevel = if edge < 0.025 { 0.78 } else { 1.0 };
                return (green.map(|value| value * bevel), -0.012, 0.22);
            }
        }
    }
    (green, 0.0, 0.22)
}

fn roof_slope(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>, north: bool, part: usize) {
    let (sin, cos) = PITCH.sin_cos();
    let eave = if north {
        HOUSE_Y[1] + EAVES
    } else {
        HOUSE_Y[0] - EAVES
    };
    let sign = if north { -1.0 } else { 1.0 };
    let normal = [0.0, -sign * sin, cos];
    let up_slope = [0.0, sign * cos, sin];
    let slope_length = (RIDGE_Y - (HOUSE_Y[0] - EAVES)) / cos;
    let x_start = HOUSE_X[0] - VERGE;
    let width = (HOUSE_X[1] + VERGE - x_start) / 4.0;
    let x0 = x_start + width * part as f64;
    let base = site.wall_top + FASCIA;
    grid(rng, width, slope_length - 0.08, 0.0095, |rng, dx, a| {
        let x = x0 + dx;
        let y = eave + sign * a * cos;
        let z = base + a * sin;
        if north && in_range(x, CHIMNEY[0]) && in_range(y, CHIMNEY[1]) {
            return;
        }
        let surface = [x, y, z];
        // Solar panels on the south slope.
        if !north && in_range(x, [27.0, 32.3]) && in_range(a, [1.3, 4.8]) {
            solar_panel(site, rng, out, surface, normal, x - 27.0, a - 1.3);
            return;
        }
        // A roof window on the north slope.
        if north && in_range(x, [27.6, 28.38]) && in_range(a, [2.9, 4.08]) {
            let edge = (x - 27.6).min(28.38 - x).min(a - 2.9).min(4.08 - a);
            if edge < 0.06 {
                let p = along(surface, normal, 0.1);
                let colour = site.shade([0.2, 0.21, 0.22], p, normal, 0.95);
                push(out, p, grain(rng, colour, 0.02), 0.3, class::BUILDING);
            } else if rng.chance(0.35) {
                let p = along(surface, normal, 0.085);
                let up = (a - 2.9) / 1.18;
                let colour = mix([0.12, 0.14, 0.17], [0.4, 0.46, 0.55], up);
                push(out, p, grain(rng, colour, 0.04), 0.04, class::BUILDING);
            }
            return;
        }
        // Interlocking tiles in courses: each tile is thickest at its lower
        // edge and has a wave across.
        let course = (a / 0.31).floor();
        let t = a / 0.31 - course;
        let shift = if course as i64 % 2 == 0 { 0.0 } else { 0.15 };
        let across = x - x_start + shift;
        let column = (across / 0.3).floor();
        let phase = TAU * across / 0.3;
        let rise = 0.026 * (1.0 - t) + 0.01 * phase.sin();
        let tilt_across = -0.21 * phase.cos();
        let tile_normal = normalize(along(
            along(normal, [1.0, 0.0, 0.0], tilt_across),
            up_slope,
            0.084,
        ));
        let h = hash(
            SEED ^ 60 ^ u64::from(north),
            column as i64,
            course as i64,
            0,
        );
        let mut albedo: Rgb = [0.17, 0.18, 0.2].map(|value| value * (0.86 + 0.28 * h));
        if t > 0.86 {
            albedo = albedo.map(|value| value * 0.55);
        }
        let moss_noise = fbm2(SEED ^ 61, x / 1.5, a / 1.5, 3);
        let moss = smoothstep(0.55, 0.75, moss_noise) * if north { 0.8 } else { 0.25 };
        albedo = mix(albedo, [0.3, 0.34, 0.19], moss);
        let p = along(surface, normal, rise + 0.002 * rng.normal());
        let colour = site.shade(albedo, p, tile_normal, 1.0);
        push(
            out,
            p,
            grain(rng, colour, 0.03),
            0.2 + 0.04 * rng.normal(),
            class::BUILDING,
        );
    });
}

/// A panel of 6 by 10 dark cells in an aluminium frame, 13 cm above the
/// tiles. `a` and `b` are metres across the field of panels and up it.
fn solar_panel(
    site: &Site,
    rng: &mut Rng,
    out: &mut Vec<Sample>,
    surface: V3,
    normal: V3,
    a: f64,
    b: f64,
) {
    let (column, row) = ((a / 1.06).floor(), (b / 1.75).floor());
    let (u, v) = (a - column * 1.06, b - row * 1.75);
    if u > 1.04 || v > 1.73 {
        return;
    }
    let p = along(surface, normal, 0.13);
    let frame = u.min(1.04 - u).min(v).min(1.73 - v) < 0.035;
    if frame {
        let colour = site.shade([0.62, 0.63, 0.64], p, normal, 1.0);
        push(out, p, grain(rng, colour, 0.02), 0.5, class::BUILDING);
        return;
    }
    let cell_u = ((u - 0.035) / (0.97 / 6.0)).fract();
    let cell_v = ((v - 0.035) / (1.66 / 10.0)).fract();
    let gap = cell_u < 0.02 || cell_v < 0.02;
    let mut colour = if gap {
        [0.34, 0.36, 0.4]
    } else {
        [0.05, 0.07, 0.11]
    };
    let sky = 0.12 + 0.08 * noise2(SEED ^ 62, a * 0.8, b * 0.8);
    colour = mix(colour, [0.36, 0.42, 0.52], sky);
    push(out, p, grain(rng, colour, 0.03), 0.05, class::BUILDING);
}

fn ridge_and_chimney(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>) {
    let spacing = 0.009;
    let ridge = site.roof_z(RIDGE_Y);
    // Half-round ridge tiles.
    let radius = 0.12;
    let x0 = HOUSE_X[0] - VERGE;
    let length = HOUSE_X[1] + VERGE - x0;
    let arc = 1.0;
    grid(rng, length, 2.0 * arc * radius, spacing, |rng, dx, s| {
        let angle = s / radius - arc;
        let x = x0 + dx;
        let joint = x.rem_euclid(0.4) < 0.03;
        let r = radius + if joint { 0.01 } else { 0.0 };
        let normal = [0.0, angle.sin(), angle.cos()];
        let p = [x, RIDGE_Y + r * angle.sin(), ridge - 0.03 + r * angle.cos()];
        let shade = if joint { 0.7 } else { 1.0 };
        let colour = site.shade([0.16 * shade, 0.17 * shade, 0.19 * shade], p, normal, 1.0);
        push(out, p, grain(rng, colour, 0.03), 0.2, class::BUILDING);
    });
    // The chimney: brick faces above the roof, a concrete cap and a flue.
    let top = ridge + 0.85;
    let [xs, ys] = CHIMNEY;
    let chimney_facade = Facade {
        id: 9,
        start: [0.0, 0.0],
        along: [1.0, 0.0, 0.0],
        normal: [0.0, -1.0, 0.0],
        length: 1.0,
        gable: false,
        openings: Vec::new(),
    };
    let faces: [([f64; 2], [f64; 2], V3); 4] = [
        ([xs[0], ys[0]], [xs[1], ys[0]], [0.0, -1.0, 0.0]),
        ([xs[1], ys[0]], [xs[1], ys[1]], [1.0, 0.0, 0.0]),
        ([xs[1], ys[1]], [xs[0], ys[1]], [0.0, 1.0, 0.0]),
        ([xs[0], ys[1]], [xs[0], ys[0]], [-1.0, 0.0, 0.0]),
    ];
    for (from, to, normal) in faces {
        let length = (to[0] - from[0]).hypot(to[1] - from[1]);
        let low = site.roof_z(ys[1]).min(site.roof_z(ys[0]));
        grid(rng, length, top - low, spacing, |rng, s, dz| {
            let t = s / length;
            let (x, y) = (
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            );
            let z = low + dz;
            let roof = site.roof_z(y);
            if z < roof {
                return;
            }
            let (mut albedo, behind, intensity) = brick(site, &chimney_facade, s, z);
            if z < roof + 0.12 {
                albedo = [0.42, 0.43, 0.45];
            }
            let p = along([x, y, z], normal, -behind);
            let colour = site.shade(albedo, p, normal, 0.95);
            push(out, p, grain(rng, colour, 0.03), intensity, class::BUILDING);
        });
    }
    let cap = [[xs[0] - 0.05, xs[1] + 0.05], [ys[0] - 0.05, ys[1] + 0.05]];
    grid(
        rng,
        cap[0][1] - cap[0][0],
        cap[1][1] - cap[1][0],
        spacing,
        |rng, u, v| {
            let p = [cap[0][0] + u, cap[1][0] + v, top + 0.07];
            let centre = (p[0] - (xs[0] + xs[1]) / 2.0).hypot(p[1] - (ys[0] + ys[1]) / 2.0);
            if centre < 0.09 {
                return;
            }
            let colour = site.shade([0.55, 0.55, 0.53], p, [0.0, 0.0, 1.0], 1.0);
            push(out, p, grain(rng, colour, 0.03), 0.45, class::BUILDING);
        },
    );
    let rim = 2.0 * (cap[0][1] - cap[0][0] + cap[1][1] - cap[1][0]);
    grid(rng, rim, 0.07, spacing, |rng, s, dz| {
        let (w, d) = (cap[0][1] - cap[0][0], cap[1][1] - cap[1][0]);
        let (p, normal) = if s < w {
            ([cap[0][0] + s, cap[1][0], top + dz], [0.0, -1.0, 0.0])
        } else if s < w + d {
            ([cap[0][1], cap[1][0] + s - w, top + dz], [1.0, 0.0, 0.0])
        } else if s < 2.0 * w + d {
            (
                [cap[0][1] - (s - w - d), cap[1][1], top + dz],
                [0.0, 1.0, 0.0],
            )
        } else {
            (
                [cap[0][0], cap[1][1] - (s - 2.0 * w - d), top + dz],
                [-1.0, 0.0, 0.0],
            )
        };
        let colour = site.shade([0.52, 0.52, 0.5], p, normal, 0.9);
        push(out, p, grain(rng, colour, 0.03), 0.45, class::BUILDING);
    });
    let flue = [(xs[0] + xs[1]) / 2.0, (ys[0] + ys[1]) / 2.0];
    grid(rng, TAU * 0.09, 0.3, spacing, |rng, s, dz| {
        let angle = s / 0.09;
        let normal = [angle.cos(), angle.sin(), 0.0];
        let p = [
            flue[0] + 0.09 * normal[0],
            flue[1] + 0.09 * normal[1],
            top + 0.07 + dz,
        ];
        let colour = site.shade([0.26, 0.26, 0.27], p, normal, 1.0);
        push(out, p, grain(rng, colour, 0.02), 0.35, class::BUILDING);
    });
}

/// Fascias, soffits, verge boards, gutters and downpipes.
fn eaves(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>) {
    let spacing = 0.009;
    let white = [0.88, 0.88, 0.86];
    let zinc = [0.6, 0.62, 0.62];
    let x0 = HOUSE_X[0] - VERGE;
    let length = HOUSE_X[1] + VERGE - x0;
    let w = site.wall_top;
    for (eave, wall, sign) in [
        (HOUSE_Y[0] - EAVES, HOUSE_Y[0], -1.0),
        (HOUSE_Y[1] + EAVES, HOUSE_Y[1], 1.0),
    ] {
        let outward = [0.0, sign, 0.0];
        grid(rng, length, FASCIA, spacing, |rng, dx, dz| {
            let p = [x0 + dx, eave, w + dz];
            let colour = site.shade(white, p, outward, 0.9);
            push(out, p, grain(rng, colour, 0.02), 0.75, class::BUILDING);
        });
        grid(rng, length, EAVES, spacing, |rng, dx, d| {
            let p = [x0 + dx, wall + sign * d, w];
            let colour = site.shade(white, p, [0.0, 0.0, -1.0], 0.7);
            push(out, p, grain(rng, colour, 0.02), 0.75, class::BUILDING);
        });
        // A half-round gutter in front of the fascia.
        let radius = 0.075;
        let centre = [eave + sign * (radius + 0.01), w + 0.13];
        grid(rng, length, PI * radius, spacing, |rng, dx, s| {
            let angle = s / radius;
            let normal = [0.0, sign * angle.cos(), -angle.sin()];
            let p = [
                x0 + dx,
                centre[0] + radius * normal[1],
                centre[1] + radius * normal[2],
            ];
            let colour = site.shade(zinc, p, normal, 0.95);
            push(out, p, grain(rng, colour, 0.02), 0.55, class::BUILDING);
        });
        // Downpipes at both ends.
        for x in [HOUSE_X[0] + 0.15, HOUSE_X[1] - 0.15] {
            let y = wall + sign * 0.06;
            let ground = terrain(x, y);
            grid(rng, TAU * 0.04, w + 0.1 - ground, spacing, |rng, s, dz| {
                let angle = s / 0.04;
                let normal = [angle.cos(), angle.sin(), 0.0];
                let z = ground + dz;
                let p = [x + 0.04 * normal[0], y + 0.04 * normal[1], z];
                let bracket = (z - ground).rem_euclid(1.5) < 0.03;
                let colour = site.shade(if bracket { [0.4; 3] } else { zinc }, p, normal, 0.9);
                push(out, p, grain(rng, colour, 0.02), 0.55, class::BUILDING);
            });
        }
    }
    // Verge boards along the gables.
    let eave_south = HOUSE_Y[0] - EAVES;
    let span = 2.0 * EAVES + HOUSE_Y[1] - HOUSE_Y[0];
    for (x, normal) in [
        (HOUSE_X[0] - VERGE, [-1.0, 0.0, 0.0]),
        (HOUSE_X[1] + VERGE, [1.0, 0.0, 0.0]),
    ] {
        grid(rng, span, 0.22, spacing, |rng, dy, dz| {
            let y = eave_south + dy;
            let p = [x, y, site.roof_z(y) - 0.2 + dz];
            let colour = site.shade(white, p, normal, 0.9);
            push(out, p, grain(rng, colour, 0.02), 0.75, class::BUILDING);
        });
        // The underside of the roof beyond the gable.
        let inner = if normal[0] < 0.0 {
            HOUSE_X[0]
        } else {
            HOUSE_X[1]
        };
        grid(rng, VERGE, span, spacing, |rng, dx, dy| {
            let y = eave_south + dy;
            let px = inner + normal[0] * dx;
            let p = [px, y, site.roof_z(y) - 0.2];
            let colour = site.shade(white, p, [0.0, 0.0, -1.0], 0.65);
            push(out, p, grain(rng, colour, 0.02), 0.75, class::BUILDING);
        });
    }
}

fn bark(species: Species, p: V3, outward: V3) -> Rgb {
    match species {
        Species::Birch => {
            let mark = noise3(SEED ^ 70, [p[0] * 30.0, p[1] * 30.0, p[2] * 6.0]);
            if mark > 0.7 {
                [0.16, 0.15, 0.14]
            } else {
                [0.8, 0.79, 0.75].map(|value| value * (0.9 + 0.1 * mark))
            }
        }
        _ => {
            let streak = noise3(SEED ^ 71, [p[0] * 25.0, p[1] * 25.0, p[2] * 2.5]);
            let colour = [0.33, 0.29, 0.25].map(|value| value * (0.7 + 0.5 * streak));
            // Moss on the north side.
            let north = smoothstep(0.2, 0.8, outward[1]);
            mix(colour, [0.27, 0.32, 0.18], north * 0.5)
        }
    }
}

/// Points of a tapered cylinder from `from` to `to`.
fn limb(
    site: &Site,
    tree: &Tree,
    rng: &mut Rng,
    out: &mut Vec<Sample>,
    [from, to]: [V3; 2],
    radii: [f64; 2],
) {
    let spacing = 0.011;
    let axis = sub(to, from);
    let length = dot(axis, axis).sqrt();
    if length <= 0.0 {
        return;
    }
    let direction = scale(axis, 1.0 / length);
    let helper = if direction[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let side = normalize(cross(direction, helper));
    let other = cross(direction, side);
    let around = TAU * radii[0].max(radii[1]);
    let crown = tree.crown();
    grid(rng, length, around, spacing, |rng, s, c| {
        let t = s / length;
        let radius = radii[0] + (radii[1] - radii[0]) * t;
        let angle = c / around * TAU;
        if rng.unit() > radius / radii[0].max(radii[1]) {
            return;
        }
        let outward = along(scale(side, angle.cos()), other, angle.sin());
        let p = along(along(from, direction, s), outward, radius);
        let albedo = bark(tree.species, p, outward);
        let inside = crown.depth(p);
        let ao = 0.45 + 0.55 * smoothstep(0.3, 1.1, inside);
        let colour = site.shade(albedo, p, outward, ao);
        push(
            out,
            p,
            grain(rng, colour, 0.04),
            0.35,
            class::HIGH_VEGETATION,
        );
    });
}

/// The trunk and the main limbs of a tree.
fn wood(site: &Site, tree: &Tree, rng: &mut Rng, out: &mut Vec<Sample>) {
    let ground = tree.ground();
    let crown = tree.crown();
    let top = if tree.species == Species::Spruce {
        ground + tree.height - 0.3
    } else {
        crown.center[2] + 0.2 * crown.radii[2]
    };
    // A flared foot, then a trunk that tapers.
    let flare_height = 0.4;
    let foot = [tree.x, tree.y, ground + flare_height];
    let flare = [tree.trunk_radius * 1.45, tree.trunk_radius];
    limb(
        site,
        tree,
        rng,
        out,
        [[tree.x, tree.y, ground - 0.03], foot],
        flare,
    );
    let taper = [tree.trunk_radius, tree.trunk_radius * 0.35];
    limb(site, tree, rng, out, [foot, [tree.x, tree.y, top]], taper);
    if tree.species == Species::Spruce {
        return;
    }
    let lobes = tree.lobes();
    let mut branch_rng = Rng::new(tree.seed ^ 0xb7a4);
    for (centre, _) in lobes.iter().skip(1).take(7) {
        let height = ground
            + tree.crown_base
            + branch_rng.range(0.0, 0.5) * (top - ground - tree.crown_base);
        let from = [tree.x, tree.y, height];
        let to = along(*centre, [0.0, 0.0, 1.0], branch_rng.range(-0.3, 0.3));
        limb(
            site,
            tree,
            rng,
            out,
            [from, to],
            [tree.trunk_radius * 0.42, 0.05],
        );
    }
}

/// A quarter of the leaves of a broadleaved tree, in clumps on the surfaces
/// of its lobes.
fn foliage(site: &Site, tree: &Tree, rng: &mut Rng, out: &mut Vec<Sample>) {
    let crown = tree.crown();
    let lobes = tree.lobes();
    let weights: Vec<f64> = lobes.iter().map(|(_, radius)| radius * radius).collect();
    let total: f64 = weights.iter().sum();
    let leaves = 80_000.0 * tree.crown_radius * tree.crown_radius / 4.0;
    let per_clump = 56;
    // Each of the four parts makes a quarter of the clumps.
    let clumps = (leaves / per_clump as f64 / 4.0) as usize;
    let (dark, light) = tree.leaf_colours();
    let spread = 0.1 * (tree.crown_radius / 4.0).max(0.65);
    for _ in 0..clumps {
        // Choose a lobe by its surface and a place near its surface that no
        // other lobe covers deeply.
        let mut centre = None;
        for _ in 0..8 {
            let mut pick = rng.unit() * total;
            let mut index = 0;
            while index + 1 < weights.len() && pick > weights[index] {
                pick -= weights[index];
                index += 1;
            }
            let (lobe, radius) = lobes[index];
            let reach = radius * (1.0 - 0.3 * rng.unit().powi(2));
            let candidate = along(lobe, rng.direction(), reach);
            let buried = lobes.iter().enumerate().any(|(other, (c, r))| {
                other != index && {
                    let gap = sub(candidate, *c);
                    dot(gap, gap).sqrt() < 0.72 * r
                }
            });
            if !buried && candidate[2] > tree.ground() + tree.crown_base * 0.8 {
                centre = Some(candidate);
                break;
            }
        }
        let Some(centre) = centre else { continue };
        // Gaps in the foliage, through which branches and sky show.
        let leafy =
            0.15 + 0.85 * smoothstep(0.32, 0.6, noise3(tree.seed ^ 0x9a, scale(centre, 0.9)));
        if !rng.chance(leafy) {
            continue;
        }
        let hue = rng.unit();
        let clump_colour = mix(dark, light, hue);
        let outward = crown.outward(centre);
        let depth = crown.depth(centre);
        let ao = 0.42 + 0.58 * smoothstep(0.55, 1.0, depth);
        // The light of the clump: own crown and everything else between it
        // and the sun.
        let sun = site.sun_visibility(along(centre, outward, 0.05));
        for _ in 0..per_clump {
            let p = [
                centre[0] + spread * rng.normal(),
                centre[1] + spread * rng.normal(),
                centre[2] + 0.6 * spread * rng.normal(),
            ];
            let normal = normalize(along(outward, rng.direction(), 0.8));
            let albedo = clump_colour.map(|value| value * (0.86 + 0.28 * rng.unit()));
            let colour = site.lit(albedo, normal, ao, sun);
            push(
                out,
                p,
                grain(rng, colour, 0.04),
                0.42 + 0.12 * rng.normal(),
                class::HIGH_VEGETATION,
            );
        }
    }
}

/// A spruce: whorls of drooping branches, bushier at their tips, in a cone.
fn spruce(site: &Site, tree: &Tree, rng: &mut Rng, out: &mut Vec<Sample>, part: usize) {
    let ground = tree.ground();
    let (dark, light) = tree.leaf_colours();
    let start = tree.crown_base;
    let mut whorl_rng = Rng::new(tree.seed);
    let mut level = 0;
    let mut height = start;
    while height < tree.height - 0.2 {
        let reach = tree.crown_radius * ((tree.height - height) / (tree.height - start)).powf(0.9);
        let branches = 7 + (whorl_rng.unit() * 3.0) as usize;
        let turn = whorl_rng.range(0.0, TAU);
        if level % 4 == part {
            for branch in 0..branches {
                let azimuth = turn + TAU * branch as f64 / branches as f64 + 0.3 * rng.normal();
                let length = reach * rng.range(0.8, 1.1);
                let needles = (length * 2600.0) as usize;
                for _ in 0..needles {
                    let s = rng.unit().sqrt() * length;
                    let t = s / length;
                    let droop = -0.35 * t * t * length / 2.0;
                    let bush = 0.05 + 0.16 * t;
                    let axis = [azimuth.cos(), azimuth.sin(), 0.0];
                    let p = [
                        tree.x + axis[0] * s + bush * rng.normal(),
                        tree.y + axis[1] * s + bush * rng.normal(),
                        ground + height + droop + 0.6 * bush * rng.normal(),
                    ];
                    let outward = normalize([axis[0], axis[1], 0.5]);
                    let albedo = mix(dark, light, t * 0.8 + 0.2 * rng.unit());
                    let ao = 0.4 + 0.6 * t;
                    let colour = site.shade(albedo, p, outward, ao);
                    push(
                        out,
                        p,
                        grain(rng, colour, 0.04),
                        0.4 + 0.1 * rng.normal(),
                        class::HIGH_VEGETATION,
                    );
                }
            }
        }
        height += 0.33 + 0.05 * whorl_rng.normal();
        level += 1;
    }
}

/// Round shrubs in the bed in front of the house.
fn shrubs(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>) {
    let shrubs: [(f64, f64, Rgb); 8] = [
        (25.9, 0.42, [0.15, 0.27, 0.12]),
        (26.9, 0.36, [0.15, 0.27, 0.12]),
        (27.9, 0.45, [0.34, 0.15, 0.18]),
        (31.0, 0.4, [0.15, 0.27, 0.12]),
        (32.1, 0.5, [0.22, 0.34, 0.15]),
        (33.3, 0.38, [0.15, 0.27, 0.12]),
        (34.3, 0.44, [0.45, 0.38, 0.55]),
        (35.2, 0.36, [0.15, 0.27, 0.12]),
    ];
    for (x, radius, colour) in shrubs {
        let y = (BED_Y[0] + BED_Y[1]) / 2.0;
        let ground = terrain(x, y);
        let centre = [x, y, ground + radius * 0.6];
        let count = (4.0 * PI * radius * radius * 9000.0) as usize;
        for _ in 0..count {
            let direction = rng.direction();
            // A lumpy outline, a little flattened, of loose leaves.
            let lumps = noise3(SEED ^ 72, along(scale(direction, 1.7), [x, 0.0, 0.0], 1.0));
            let twigs =
                0.05 * (noise3(SEED ^ 74, scale(along(centre, direction, radius), 7.0)) - 0.5);
            let r = radius * (0.72 + 0.5 * lumps) * (1.0 - 0.18 * rng.unit().powi(2)) + twigs;
            let mut p = along(centre, direction, r);
            p[2] = centre[2] + (p[2] - centre[2]) * 0.85;
            if p[2] < ground + 0.03 {
                continue;
            }
            let albedo = colour.map(|value| value * (0.8 + 0.4 * rng.unit()));
            let ao = 0.5 + 0.5 * smoothstep(-0.6, 0.8, direction[2]);
            let shade = site.shade(albedo, p, direction, ao);
            push(out, p, grain(rng, shade, 0.04), 0.45, class::LOW_VEGETATION);
        }
    }
}

/// A clipped privet hedge from `x0` to `x1`, with a rounded top, and its
/// cut ends where the hedge stops.
fn hedge(
    site: &Site,
    rng: &mut Rng,
    out: &mut Vec<Sample>,
    x0: f64,
    x1: f64,
    first: bool,
    last: bool,
) {
    let half = (HEDGE_Y[1] - HEDGE_Y[0]) / 2.0;
    let middle = (HEDGE_Y[0] + HEDGE_Y[1]) / 2.0;
    let corner = 0.2;
    let foot = 0.15;
    let side = HEDGE_HEIGHT - corner - foot;
    let arc = FRAC_PI_2 * corner;
    let top = 2.0 * (half - corner);
    let perimeter = 2.0 * side + 2.0 * arc + top;
    // A point of the cross-section at `s` along it, from the south foot
    // over the top to the north foot: offset across, height and normal.
    let section = |s: f64| -> (f64, f64, [f64; 2]) {
        if s < side {
            (-half, foot + s, [-1.0, 0.0])
        } else if s < side + arc {
            let angle = PI - (s - side) / corner;
            let (sin, cos) = angle.sin_cos();
            (
                -half + corner + corner * cos,
                foot + side + corner * sin,
                [cos, sin],
            )
        } else if s < side + arc + top {
            (-half + corner + (s - side - arc), HEDGE_HEIGHT, [0.0, 1.0])
        } else if s < side + 2.0 * arc + top {
            let angle = FRAC_PI_2 - (s - side - arc - top) / corner;
            let (sin, cos) = angle.sin_cos();
            (
                half - corner + corner * cos,
                foot + side + corner * sin,
                [cos, sin],
            )
        } else {
            (half, foot + perimeter - s, [1.0, 0.0])
        }
    };
    let leaf = |rng: &mut Rng, p: V3, normal: V3, rel: f64| {
        let mut albedo = mix(
            [0.13, 0.25, 0.1],
            [0.25, 0.37, 0.14],
            noise3(SEED ^ 73, scale(p, 2.0)),
        );
        if normal[2] > 0.5 {
            albedo = mix(albedo, [0.3, 0.42, 0.16], 0.4);
        }
        if rng.chance(0.03) {
            albedo = [0.36, 0.3, 0.17];
        }
        let albedo = albedo.map(|value| value * (0.8 + 0.4 * rng.unit()));
        let ao = 0.5 + 0.5 * smoothstep(0.0, 0.9, rel);
        let colour = site.shade(albedo, along(p, normal, 0.12), normal, ao);
        grain(rng, colour, 0.04)
    };
    grid(rng, x1 - x0, perimeter, 0.0115, |rng, dx, s| {
        let x = x0 + dx;
        let ground = terrain(x, middle);
        let (offset, height, n2) = section(s);
        let normal = [0.0, n2[0], n2[1]];
        let base = [x, middle + offset, ground + height];
        let bump = 0.07 * (fbm3(SEED ^ 74, scale(base, 2.5), 2) - 0.5) * 2.0;
        let depth = bump + 0.02 * rng.normal() - 0.06 * rng.unit().powi(2);
        let p = along(base, normal, depth);
        let colour = leaf(rng, p, normal, height / HEDGE_HEIGHT);
        push(
            out,
            p,
            colour,
            0.5 + 0.1 * rng.normal(),
            class::MEDIUM_VEGETATION,
        );
    });
    // The cut ends.
    for (end, x, sign) in [(first, x0, -1.0), (last, x1, 1.0)] {
        if !end {
            continue;
        }
        let ground = terrain(x, middle);
        grid(rng, 2.0 * half, HEDGE_HEIGHT - foot, 0.0115, |rng, a, b| {
            let (offset, height) = (a - half, foot + b);
            // Inside the rounded corners only.
            let cx = offset.abs() - (half - corner);
            let cz = height - (HEDGE_HEIGHT - corner);
            if cx > 0.0 && cz > 0.0 && cx.hypot(cz) > corner {
                return;
            }
            let normal = [sign, 0.0, 0.0];
            let p = [
                x + sign * (0.03 * rng.normal() - 0.05 * rng.unit().powi(2)),
                middle + offset,
                ground + height,
            ];
            let colour = leaf(rng, p, normal, height / HEDGE_HEIGHT);
            push(out, p, colour, 0.5, class::MEDIUM_VEGETATION);
        });
    }
}

/// Panels `first` to `last` of a board fence along one boundary: boards on
/// both faces between square posts.
fn fence(
    site: &Site,
    rng: &mut Rng,
    out: &mut Vec<Sample>,
    line: usize,
    first: usize,
    last: usize,
    panels: usize,
) {
    let [from, to] = FENCES[line];
    let length = (to[0] - from[0]).hypot(to[1] - from[1]);
    let direction = [(to[0] - from[0]) / length, (to[1] - from[1]) / length, 0.0];
    let side = [direction[1], -direction[0], 0.0];
    let panel = length / panels as f64;
    let spacing = 0.012;
    let point = |s: f64| [from[0] + direction[0] * s, from[1] + direction[1] * s];
    for index in first..last {
        let s0 = index as f64 * panel;
        let mid = point(s0 + panel / 2.0);
        let ground = terrain(mid[0], mid[1]);
        let weathering = hash(SEED ^ 80, line as i64, index as i64, 0);
        for sign in [1.0, -1.0] {
            let normal = scale(side, sign);
            grid(
                rng,
                panel - 0.09,
                FENCE_HEIGHT - 0.06,
                spacing,
                |rng, ds, dz| {
                    let s = s0 + 0.045 + ds;
                    let board = (dz / 0.145).floor();
                    if dz - board * 0.145 > 0.135 {
                        return;
                    }
                    let z = ground + 0.06 + dz;
                    let at = point(s);
                    let p = [at[0] + normal[0] * 0.01, at[1] + normal[1] * 0.01, z];
                    let h = hash(SEED ^ 81, line as i64, index as i64, board as i64);
                    let grain_noise = noise2(SEED ^ 82, s * 3.0, z * 55.0);
                    let mut albedo = mix([0.42, 0.37, 0.28], [0.52, 0.46, 0.35], h);
                    albedo = albedo.map(|value| value * (0.85 + 0.25 * grain_noise));
                    albedo = mix(albedo, [0.34, 0.37, 0.27], smoothstep(0.35, 0.0, dz) * 0.6);
                    albedo = mix(albedo, [0.56, 0.54, 0.5], weathering * 0.35);
                    let ao = 0.7 + 0.3 * smoothstep(0.0, 0.6, dz);
                    let colour = site.shade(albedo, p, normal, ao);
                    push(out, p, grain(rng, colour, 0.03), 0.45, class::UNCLASSIFIED);
                },
            );
        }
    }
    // Posts at the start of every panel, and one at the end of the line.
    let posts: Vec<usize> = (first..last)
        .chain((last == panels).then_some(panels))
        .collect();
    for index in posts {
        let at = point(index as f64 * panel);
        let ground = terrain(at[0], at[1]);
        for (offset, normal) in [
            (side, side),
            (scale(side, -1.0), scale(side, -1.0)),
            (direction, direction),
            (scale(direction, -1.0), scale(direction, -1.0)),
        ] {
            let tangent = cross([0.0, 0.0, 1.0], normal);
            grid(rng, 0.09, FENCE_HEIGHT + 0.08, spacing, |rng, a, dz| {
                let p = [
                    at[0] + offset[0] * 0.045 + tangent[0] * (a - 0.045),
                    at[1] + offset[1] * 0.045 + tangent[1] * (a - 0.045),
                    ground + dz,
                ];
                let colour = site.shade([0.34, 0.3, 0.23], p, normal, 0.85);
                push(out, p, grain(rng, colour, 0.03), 0.4, class::UNCLASSIFIED);
            });
        }
    }
}

/// A garden shed of stained vertical boards with a roof of bitumen that
/// slopes to the back.
fn shed(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>) {
    let spacing = 0.01;
    let ground = terrain(SHED_X[0], SHED_Y[0]);
    let front = ground + SHED_FRONT;
    let rear = ground + SHED_REAR;
    let roof_at = |y: f64| front + (rear - front) * (y - SHED_Y[0]) / (SHED_Y[1] - SHED_Y[0]);
    let walls: [([f64; 2], [f64; 2], V3); 4] = [
        (
            [SHED_X[0], SHED_Y[0]],
            [SHED_X[1], SHED_Y[0]],
            [0.0, -1.0, 0.0],
        ),
        (
            [SHED_X[1], SHED_Y[0]],
            [SHED_X[1], SHED_Y[1]],
            [1.0, 0.0, 0.0],
        ),
        (
            [SHED_X[1], SHED_Y[1]],
            [SHED_X[0], SHED_Y[1]],
            [0.0, 1.0, 0.0],
        ),
        (
            [SHED_X[0], SHED_Y[1]],
            [SHED_X[0], SHED_Y[0]],
            [-1.0, 0.0, 0.0],
        ),
    ];
    for (wall, (from, to, normal)) in walls.into_iter().enumerate() {
        let length = (to[0] - from[0]).hypot(to[1] - from[1]);
        grid(rng, length, front - ground + 0.3, spacing, |rng, s, dz| {
            let t = s / length;
            let (x, y) = (
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            );
            let z = ground - 0.3 + dz;
            if z < terrain(x, y) || z > roof_at(y) - 0.05 {
                return;
            }
            let board = (s / 0.12).floor();
            let in_board = s - board * 0.12;
            let h = hash(SEED ^ 90, wall as i64, board as i64, 0);
            let mut albedo = [0.24, 0.19, 0.15].map(|value| value * (0.85 + 0.3 * h));
            let mut depth = 0.0;
            if in_board < 0.008 {
                albedo = albedo.map(|value| value * 0.5);
                depth = 0.006;
            }
            // A door in the front.
            if wall == 0 && in_range(s, [1.0, 1.9]) && z < ground + 1.95 {
                albedo = [0.2, 0.26, 0.22];
                if in_range(s, [1.75, 1.8]) && in_range(z - ground, [0.95, 1.05]) {
                    albedo = [0.6, 0.6, 0.58];
                }
            }
            let p = along([x, y, z], normal, -depth);
            let ao = 1.0 - 0.3 * (-(z - terrain(x, y)) / 0.3).exp();
            let colour = site.shade(albedo, p, normal, ao);
            push(out, p, grain(rng, colour, 0.03), 0.35, class::BUILDING);
        });
    }
    // The roof, 15 cm beyond the walls.
    let slope = normalize([0.0, (front - rear) / (SHED_Y[1] - SHED_Y[0]), 1.0]);
    grid(
        rng,
        SHED_X[1] - SHED_X[0] + 0.3,
        SHED_Y[1] - SHED_Y[0] + 0.3,
        spacing,
        |rng, u, v| {
            let (x, y) = (SHED_X[0] - 0.15 + u, SHED_Y[0] - 0.15 + v);
            let p = [x, y, roof_at(y) + 0.04];
            let albedo = [0.15, 0.15, 0.16].map(|value| value * (0.8 + 0.4 * rng.unit()));
            let colour = site.shade(albedo, p, slope, 1.0);
            push(out, p, grain(rng, colour, 0.03), 0.15, class::BUILDING);
        },
    );
}

fn street_light(site: &Site, rng: &mut Rng, out: &mut Vec<Sample>) {
    let ground = sidewalk_z(LAMP[0], LAMP[1]);
    let height = 5.5;
    let paint = [0.24, 0.27, 0.26];
    grid(rng, height, TAU * 0.08, 0.008, |rng, dz, c| {
        let radius = 0.08 - 0.03 * dz / height;
        let angle = c / 0.08;
        let normal = [angle.cos(), angle.sin(), 0.0];
        let p = [
            LAMP[0] + radius * normal[0],
            LAMP[1] + radius * normal[1],
            ground + dz,
        ];
        let colour = site.shade(paint, p, normal, 0.95);
        push(out, p, grain(rng, colour, 0.02), 0.4, class::UNCLASSIFIED);
    });
    // An arm over the street with a flat lantern.
    let arm_z = ground + height;
    grid(rng, 1.0, TAU * 0.035, 0.008, |rng, s, c| {
        let angle = c / 0.035;
        let normal = [angle.cos(), 0.0, angle.sin()];
        let p = [
            LAMP[0] + 0.035 * normal[0],
            LAMP[1] - s,
            arm_z + 0.035 * normal[2],
        ];
        let colour = site.shade(paint, p, normal, 0.95);
        push(out, p, grain(rng, colour, 0.02), 0.4, class::UNCLASSIFIED);
    });
    grid(rng, 0.3, 0.55, 0.008, |rng, a, b| {
        let p = [LAMP[0] - 0.15 + a, LAMP[1] - 0.75 - b, arm_z - 0.06];
        let colour = site.shade([0.8, 0.8, 0.76], p, [0.0, 0.0, -1.0], 0.9);
        push(out, p, grain(rng, colour, 0.02), 0.6, class::UNCLASSIFIED);
        let q = [p[0], p[1], arm_z + 0.04];
        let colour = site.shade(paint, q, [0.0, 0.0, 1.0], 1.0);
        push(out, q, grain(rng, colour, 0.02), 0.4, class::UNCLASSIFIED);
    });
}
