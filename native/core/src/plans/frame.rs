//! The frame of a building and the box around what was scanned of it.
//!
//! `robust_bounds` finds the box from the leaves of an octree index without
//! reading a point: per axis it leaves out a small share of the points at
//! either end, so that a few stray reflections metres below the ground or
//! far out over the street do not make the box of the scene ten times too
//! large, and adds a margin. Every leaf counts with its number of points at
//! the side of its box that faces the middle; a leaf is not known to hold
//! points any farther out than that. The margin covers what that may leave
//! out, so the box never reaches farther than the margin beyond the points
//! that are kept, while a leaf larger than the margin may cut into them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::region_source::{overlaps, visit_region, RegionReader, RegionSource, EVERYWHERE};
use crate::{Bounds, LoadError, OrientedBox};

/// The frame that every drawing of a building shares, so that its plans,
/// sections and site drawing lie on each other in a CAD program: a turn about
/// the vertical, an origin and the height that is zero.
///
/// Plan coordinates `u` and `v` run along the main direction of the walls
/// and square to it: `u` points `rotation_deg` counter-clockwise from the
/// scene X axis, and `v` a quarter turn further.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BuildingFrame {
    /// The main direction of the walls, in degrees counter-clockwise from the
    /// scene X axis as seen from above, between -180 and 180.
    pub rotation_deg: f64,
    /// A second direction of walls, such as that of a wing at an angle, in
    /// the same measure; none when the walls follow one direction.
    pub second_direction_deg: Option<f64>,
    /// The scene position of the origin of the plan.
    pub origin: [f64; 2],
    /// The scene height of P, the level that is zero in every drawing.
    pub peil_z: f64,
}

impl BuildingFrame {
    /// A frame along one direction, with P at scene height zero.
    pub fn new(rotation_deg: f64, origin: [f64; 2]) -> Self {
        Self {
            rotation_deg,
            second_direction_deg: None,
            origin,
            peil_z: 0.0,
        }
    }

    fn sin_cos(&self) -> (f64, f64) {
        self.rotation_deg.to_radians().sin_cos()
    }

    /// The plan position of a scene position.
    pub fn to_plan(&self, xy: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.sin_cos();
        let (dx, dy) = (xy[0] - self.origin[0], xy[1] - self.origin[1]);
        [cos * dx + sin * dy, cos * dy - sin * dx]
    }

    /// The scene position of a plan position.
    pub fn to_scene_xy(&self, uv: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.sin_cos();
        [
            self.origin[0] + cos * uv[0] - sin * uv[1],
            self.origin[1] + sin * uv[0] + cos * uv[1],
        ]
    }

    /// A scene position in the frame: plan position and height above P.
    pub fn to_frame(&self, xyz: [f64; 3]) -> [f64; 3] {
        let [u, v] = self.to_plan([xyz[0], xyz[1]]);
        [u, v, xyz[2] - self.peil_z]
    }

    /// The scene position of a position in the frame.
    pub fn to_scene(&self, frame: [f64; 3]) -> [f64; 3] {
        let [x, y] = self.to_scene_xy([frame[0], frame[1]]);
        [x, y, frame[2] + self.peil_z]
    }

    /// A box given in the frame as a box of the scene, turned as the frame
    /// is.
    pub fn oriented_box(&self, frame: Bounds) -> OrientedBox {
        let [cx, cy, cz] = self.to_scene(frame.center());
        let half: [f64; 3] = std::array::from_fn(|axis| (frame.max[axis] - frame.min[axis]) * 0.5);
        let center = [cx, cy, cz];
        OrientedBox::new(
            Bounds {
                min: std::array::from_fn(|axis| center[axis] - half[axis]),
                max: std::array::from_fn(|axis| center[axis] + half[axis]),
            },
            self.rotation_deg,
        )
    }

    /// The box in the frame around scene positions; nothing without any.
    pub fn frame_bounds(&self, scene: impl IntoIterator<Item = [f64; 3]>) -> Option<Bounds> {
        let mut around: Option<Bounds> = None;
        for at in scene {
            let at = self.to_frame(at);
            match &mut around {
                Some(around) => around.include(at),
                None => around = Some(Bounds { min: at, max: at }),
            }
        }
        around
    }
}

/// How `robust_bounds` trims the box of a scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RobustBoundsConfig {
    /// Share of the points left out at the low end of every axis.
    pub low_share: f64,
    /// Share of the points left out at the high end of every axis.
    pub high_share: f64,
    /// Added on every side, in metres.
    pub margin: f64,
}

impl Default for RobustBoundsConfig {
    /// 0.2 % at either end, and 1 m around.
    fn default() -> Self {
        Self {
            low_share: 0.002,
            high_share: 0.002,
            margin: 1.0,
        }
    }
}

/// The box around what was scanned, without what lies far out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RobustBounds {
    /// The box that holds all but the shares left out at either end of every
    /// axis, with the margin around it, and no larger than `all`.
    pub bounds: Bounds,
    /// The box around every point, as far as the index tells it.
    pub all: Bounds,
    pub points: u64,
    /// Points in the parts of the scene that lie wholly outside `bounds`.
    pub outside_points: u64,
    /// The groups those parts form: parts less than a margin apart are one
    /// group.
    pub outside_groups: u32,
}

/// A part of a scene: the box of an octree leaf with its number of points,
/// or one point.
#[derive(Debug, Clone, Copy)]
struct Piece {
    bounds: Bounds,
    weight: u64,
}

fn pieces(sources: &[RegionSource<'_>]) -> Result<Vec<Piece>, LoadError> {
    let mut pieces = Vec::new();
    for source in sources {
        match source.reader {
            RegionReader::Index(index) => {
                for leaf in index.intersecting_leaves(|_| true) {
                    if leaf.total_points > 0 {
                        pieces.push(Piece {
                            bounds: source.transform.bounds(leaf.bounds),
                            weight: leaf.total_points,
                        });
                    }
                }
            }
            RegionReader::Stream(_) | RegionReader::Resident(_) => {
                visit_region(
                    std::slice::from_ref(source),
                    EVERYWHERE,
                    &|_, _, _| true,
                    &mut |_| Ok(()),
                    &mut |_, batch| {
                        pieces.extend(batch.iter().map(|record| Piece {
                            bounds: Bounds {
                                min: record.point.xyz,
                                max: record.point.xyz,
                            },
                            weight: 1,
                        }));
                        Ok(())
                    },
                )?;
            }
        }
    }
    Ok(pieces)
}

/// Along one axis, the lowest value below which more than `share` of the
/// weight is known to lie, each piece counting at its high side; with
/// `from_top`, the highest value above which that much is known to lie,
/// each piece counting at its low side.
fn trimmed_end(pieces: &[Piece], axis: usize, share: f64, from_top: bool) -> f64 {
    let total: u64 = pieces.iter().map(|piece| piece.weight).sum();
    let allowed = (share.clamp(0.0, 1.0) * total as f64).floor() as u64;
    let mut keys: Vec<(f64, u64)> = pieces
        .iter()
        .map(|piece| {
            let key = if from_top {
                piece.bounds.min[axis]
            } else {
                piece.bounds.max[axis]
            };
            (key, piece.weight)
        })
        .collect();
    keys.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    if from_top {
        keys.reverse();
    }
    let mut seen = 0u64;
    for (key, weight) in &keys {
        seen += weight;
        if seen > allowed {
            return *key;
        }
    }
    keys.last().map_or(0.0, |key| key.0)
}

/// The box around a scene without the points that lie far out, from the
/// leaves of the octree indexes of its layers, or from the points of a layer
/// without an index (which are read for it). Nothing for a scene without
/// points.
///
/// Per axis the box leaves out `low_share` of the points at the low end and
/// `high_share` at the high end, and gets `margin` around. A leaf counts at
/// the side of its box that faces the middle, so the trimmed box lies within
/// the points that are kept; a box that would be turned inside out, as a
/// scene of one leaf gives, takes the whole range of that axis. The result
/// is never larger than the box around all points.
pub fn robust_bounds(
    sources: &[RegionSource<'_>],
    config: &RobustBoundsConfig,
) -> Result<Option<RobustBounds>, LoadError> {
    let pieces = pieces(sources)?;
    let Some(first) = pieces.first() else {
        return Ok(None);
    };
    let mut all = first.bounds;
    for piece in &pieces[1..] {
        all.include(piece.bounds.min);
        all.include(piece.bounds.max);
    }
    let margin = config.margin.max(0.0);
    let mut bounds = all;
    for axis in 0..3 {
        let low = trimmed_end(&pieces, axis, config.low_share, false);
        let high = trimmed_end(&pieces, axis, config.high_share, true);
        if low <= high {
            bounds.min[axis] = (low - margin).max(all.min[axis]);
            bounds.max[axis] = (high + margin).min(all.max[axis]);
        }
    }

    // The parts wholly outside, and the groups they form.
    let outside: Vec<&Piece> = pieces
        .iter()
        .filter(|piece| !overlaps(piece.bounds, bounds))
        .collect();
    let outside_points = outside.iter().map(|piece| piece.weight).sum();
    let outside_groups = groups(&outside, margin);
    Ok(Some(RobustBounds {
        bounds,
        all,
        points: pieces.iter().map(|piece| piece.weight).sum(),
        outside_points,
        outside_groups,
    }))
}

/// How many groups pieces form when those in neighbouring cells of a grid
/// of `reach` (or coarser, for very large pieces) belong together.
fn groups(pieces: &[&Piece], reach: f64) -> u32 {
    if pieces.is_empty() {
        return 0;
    }
    let largest = pieces
        .iter()
        .map(|piece| piece.bounds.extent())
        .fold(0.0, f64::max);
    let cell = reach.max(largest / 32.0).max(1e-3);
    let cell_of = |value: f64| (value / cell).floor() as i64;
    let mut parent: Vec<usize> = (0..pieces.len()).collect();
    fn root(parent: &mut [usize], mut at: usize) -> usize {
        while parent[at] != at {
            parent[at] = parent[parent[at]];
            at = parent[at];
        }
        at
    }
    let mut owner: BTreeMap<[i64; 3], usize> = BTreeMap::new();
    for (index, piece) in pieces.iter().enumerate() {
        let low: [i64; 3] = std::array::from_fn(|axis| cell_of(piece.bounds.min[axis]));
        let high: [i64; 3] = std::array::from_fn(|axis| cell_of(piece.bounds.max[axis]));
        for x in low[0]..=high[0] {
            for y in low[1]..=high[1] {
                for z in low[2]..=high[2] {
                    // A piece joins those in its own cells and in the cells
                    // beside them.
                    for dx in -1..=1 {
                        for dy in -1..=1 {
                            for dz in -1..=1 {
                                if let Some(other) = owner.get(&[x + dx, y + dy, z + dz]) {
                                    let (a, b) =
                                        (root(&mut parent, index), root(&mut parent, *other));
                                    parent[a.max(b)] = a.min(b);
                                }
                            }
                        }
                    }
                    owner.entry([x, y, z]).or_insert(index);
                }
            }
        }
    }
    (0..pieces.len())
        .filter(|index| root(&mut parent, *index) == *index)
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region_source::SourceTransform;
    use crate::test_shapes::{building, indexed_cloud, BuildingSpec};
    use crate::{IndexedPoint, Point};

    fn on_threads<T: Send>(threads: usize, work: impl FnOnce() -> T + Send) -> T {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(work)
    }

    #[test]
    fn a_frame_turns_and_moves_both_ways() {
        let frame = BuildingFrame {
            peil_z: 1.5,
            ..BuildingFrame::new(17.0, [85_000.0, 445_000.0])
        };
        let scene = [85_003.0, 445_004.0, 2.0];
        let at = frame.to_frame(scene);
        assert!((at[0].hypot(at[1]) - 5.0).abs() < 1e-9);
        assert_eq!(at[2], 0.5);
        let back = frame.to_scene(at);
        assert!((0..3).all(|axis| (back[axis] - scene[axis]).abs() < 1e-9));
        // The u axis points along the turn.
        let along = frame.to_scene_xy([1.0, 0.0]);
        let (sin, cos) = 17f64.to_radians().sin_cos();
        assert!((along[0] - 85_000.0 - cos).abs() < 1e-9);
        assert!((along[1] - 445_000.0 - sin).abs() < 1e-9);
        // A box in the frame as a turned box of the scene.
        let plan = Bounds {
            min: [1.0, 2.0, -0.5],
            max: [4.0, 3.0, 2.5],
        };
        let turned = frame.oriented_box(plan);
        for (u, v, z, inside) in [
            (1.1, 2.1, 0.0, true),
            (3.9, 2.9, 2.4, true),
            (0.9, 2.5, 0.0, false),
            (2.0, 3.1, 0.0, false),
            (2.0, 2.5, -0.6, false),
        ] {
            assert_eq!(
                turned.contains(frame.to_scene([u, v, z])),
                inside,
                "{u} {v} {z}"
            );
        }
        let around = frame.frame_bounds(turned.corners()).unwrap();
        assert!(
            (0..3).all(|axis| (around.min[axis] - plan.min[axis]).abs() < 1e-9
                && (around.max[axis] - plan.max[axis]).abs() < 1e-9)
        );
    }

    #[test]
    fn stray_points_far_below_stay_outside_the_box_of_the_scene() {
        let spec = BuildingSpec::default();
        let scan = building(&spec);
        let indexed = indexed_cloud(&scan.cloud_points(), 512);
        let source = RegionSource::new(
            &indexed.cloud,
            Some(&indexed.index),
            SourceTransform::default(),
        );
        let run = || robust_bounds(&[source], &RobustBoundsConfig::default()).unwrap();
        let found = on_threads(1, run).unwrap();
        assert_eq!(on_threads(6, run), Some(found));
        assert_eq!(found.points as usize, scan.points.len());

        // The box of the building with its ground, and the strays.
        let z0 = spec.translation[2];
        let ground = spec.ground_z() + z0;
        let roof = spec.roof_z() + z0;
        let (kept, strays): (Vec<[f64; 3]>, Vec<[f64; 3]>) =
            scan.points.iter().partition(|at| at[2] > ground - 1.0);
        let mut scene = Bounds {
            min: kept[0],
            max: kept[0],
        };
        for at in &kept {
            scene.include(*at);
        }
        // Within a metre of the building and its ground on every side.
        for axis in 0..3 {
            assert!(
                found.bounds.min[axis] >= scene.min[axis] - 1.0,
                "{axis} {found:?}"
            );
            assert!(
                found.bounds.max[axis] <= scene.max[axis] + 1.0,
                "{axis} {found:?}"
            );
        }
        // Below, the margin reaches under the ground; above, the box stops
        // at the highest point, on the roof.
        assert!(found.bounds.min[2] < ground - 0.05);
        assert!(found.bounds.max[2] == scene.max[2] && scene.max[2] > roof);
        let region = OrientedBox::from(found.bounds);
        assert!(strays.iter().all(|at| !region.contains(*at)));
        for [x, y] in spec.footprint() {
            assert!(region.contains([x, y, ground + 0.5]));
        }
        // The building and its ground are inside, but for the corners of
        // the ground that reach farthest out along the axes of the scene.
        let inside = kept.iter().filter(|at| region.contains(**at)).count();
        assert!(inside as f64 > 0.995 * kept.len() as f64);
        // The cluster far down is wholly outside.
        assert!(found.outside_points as f64 >= 0.7 * strays.len() as f64);
        assert!(found.outside_points as usize <= strays.len());
        assert!(found.outside_groups >= 1);
        assert!(found.all.min[2] <= strays.iter().map(|at| at[2]).fold(f64::INFINITY, f64::min));
    }

    #[test]
    fn points_without_an_index_are_trimmed_exactly() {
        // 1000 points along x from 0 to 999, and one far out at either end.
        let mut points: Vec<IndexedPoint> = (0..1000)
            .map(|step| [step as f64, 0.0, 0.0])
            .chain([[-500.0, 0.0, 0.0], [5_000.0, 0.0, 0.0]])
            .enumerate()
            .map(|(ordinal, xyz)| IndexedPoint {
                point: Point {
                    xyz,
                    rgb: None,
                    intensity: None,
                    classification: None,
                },
                ordinal: ordinal as u64,
            })
            .collect();
        points.reverse();
        let source = RegionSource::resident(&points, SourceTransform::default());
        let config = RobustBoundsConfig {
            low_share: 0.002,
            high_share: 0.002,
            margin: 0.5,
        };
        let found = robust_bounds(&[source], &config).unwrap().unwrap();
        // Of 1002 points two may be left out at either end: the far one and
        // the first of the row.
        assert_eq!(found.bounds.min[0], 1.0 - 0.5);
        assert_eq!(found.bounds.max[0], 998.0 + 0.5);
        assert_eq!((found.all.min[0], found.all.max[0]), (-500.0, 5_000.0));
        // An axis without spread keeps it.
        assert_eq!((found.bounds.min[1], found.bounds.max[1]), (0.0, 0.0));
        assert_eq!(found.outside_points, 4);
        assert_eq!(found.outside_groups, 4);
        assert_eq!(robust_bounds(&[], &config).unwrap(), None);
    }
}
