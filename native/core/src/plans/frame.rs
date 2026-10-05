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

use super::survey::SceneSurvey;
use crate::drawing::{wall_direction, DrawingProgress, DrawingSource};
use crate::grid2d::Mask;
use crate::region_source::{
    overlaps, visit_region, RegionFilter, RegionReader, RegionSource, EVERYWHERE,
};
use crate::{normalized_degrees, Bounds, LoadError, OrientedBox};

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
    /// Of those, the points in the parts wholly below the box, such as the
    /// reflections of a puddle metres under the ground, and their groups.
    pub below_points: u64,
    pub below_groups: u32,
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
    let below: Vec<&Piece> = outside
        .iter()
        .copied()
        .filter(|piece| piece.bounds.max[2] < bounds.min[2])
        .collect();
    Ok(Some(RobustBounds {
        bounds,
        all,
        points: pieces.iter().map(|piece| piece.weight).sum(),
        outside_points,
        outside_groups,
        below_points: below.iter().map(|piece| piece.weight).sum(),
        below_groups: groups(&below, margin),
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

/// The main direction of the walls is read from a box no higher than this
/// around the middle of the core: its middle half, a slab of 3 m, holds the
/// walls of a storey and one floor at most.
const DIRECTION_BOX_HEIGHT: f64 = 6.0;

/// The frame of a building whose core is a box of the scene: turned along
/// the main direction of its walls, as `wall_direction` finds it in the
/// middle of the core, with its origin at the middle of the core rounded to
/// whole metres and P at height zero until the levels are known. Nothing
/// when that part of the core holds no faces.
pub fn building_frame(
    sources: &[RegionSource<'_>],
    core: Bounds,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<Option<BuildingFrame>, LoadError> {
    let mut part = core;
    if core.max[2] - core.min[2] > DIRECTION_BOX_HEIGHT {
        let middle = (core.min[2] + core.max[2]) * 0.5;
        part.min[2] = middle - DIRECTION_BOX_HEIGHT * 0.5;
        part.max[2] = middle + DIRECTION_BOX_HEIGHT * 0.5;
    }
    let layers: Vec<DrawingSource<'_>> = sources
        .iter()
        .map(|source| DrawingSource {
            source: *source,
            name: "",
        })
        .collect();
    let Some(found) = wall_direction(&layers, part, accept, progress)? else {
        return Ok(None);
    };
    let center = core.center();
    Ok(Some(BuildingFrame::new(
        found.rotation_degrees,
        [center[0].round(), center[1].round()],
    )))
}

/// Half the side of the square of columns in which the direction of a wall
/// is read at each of its columns.
const ORIENTATION_REACH: i64 = 3;
/// The columns in that square must lie along a line this clearly, from 0
/// for a round patch to 1 for a straight row.
const MIN_LINEARITY: f64 = 0.8;
/// A second direction holds at least a quarter of the columns of the main
/// one, at least this many degrees apart from it.
const SECOND_SHARE: u64 = 4;
const SECOND_APART_DEGREES: usize = 6;
/// The second direction is the mean of the columns this near its peak.
const SECOND_REACH_DEGREES: f64 = 3.0;

/// A second direction of walls in a survey made in the frame of the
/// building, such as that of a wing at an angle, in degrees counter-clockwise
/// from the scene X axis, within 45 degrees of the main direction. Nothing
/// when the walls follow the main direction and the one square to it.
///
/// At every column of `walls` the direction of the wall is read from the
/// columns around it; the directions, taken modulo a quarter turn, make a
/// histogram of whole degrees. The highest peak is the main direction, along
/// the frame; a second peak at least 6 degrees from it with at least a
/// quarter of its columns is the second direction, which is the mean of the
/// columns within 3 degrees of it.
pub fn second_direction(survey: &SceneSurvey, walls: &Mask) -> Option<f64> {
    let frame = walls.frame();
    let (width, height) = (frame.width as i64, frame.height as i64);
    let mut histogram = [0u64; 90];
    let mut found: Vec<f64> = Vec::new();
    for y in 0..height {
        for x in 0..width {
            if !walls.get(x, y) {
                continue;
            }
            let [mut count, mut sx, mut sy, mut sxx, mut syy, mut sxy] = [0i64; 6];
            for dy in -ORIENTATION_REACH..=ORIENTATION_REACH {
                for dx in -ORIENTATION_REACH..=ORIENTATION_REACH {
                    if walls.get(x + dx, y + dy) {
                        count += 1;
                        sx += dx;
                        sy += dy;
                        sxx += dx * dx;
                        syy += dy * dy;
                        sxy += dx * dy;
                    }
                }
            }
            if count <= 2 * ORIENTATION_REACH {
                continue;
            }
            let n = count as f64;
            let cxx = sxx as f64 - (sx * sx) as f64 / n;
            let cyy = syy as f64 - (sy * sy) as f64 / n;
            let cxy = sxy as f64 - (sx * sy) as f64 / n;
            let spread = cxx + cyy;
            let linear = ((cxx - cyy).powi(2) + 4.0 * cxy * cxy).sqrt();
            if spread <= 0.0 || linear < MIN_LINEARITY * spread {
                continue;
            }
            let degrees = 0.5 * (2.0 * cxy).atan2(cxx - cyy).to_degrees();
            let along = (degrees + 45.0).rem_euclid(90.0) - 45.0;
            histogram[((along + 45.0).floor() as usize).min(89)] += 1;
            found.push(along);
        }
    }
    let smooth: Vec<u64> = (0..90)
        .map(|bin| histogram[(bin + 89) % 90] + 2 * histogram[bin] + histogram[(bin + 1) % 90])
        .collect();
    let main = (0..90).max_by(|a, b| smooth[*a].cmp(&smooth[*b]).then(b.cmp(a)))?;
    if smooth[main] == 0 {
        return None;
    }
    let apart = |a: usize, b: usize| {
        let gap = a.abs_diff(b);
        gap.min(90 - gap)
    };
    let second = (0..90)
        .filter(|bin| {
            apart(*bin, main) >= SECOND_APART_DEGREES
                && smooth[*bin] >= smooth[(bin + 89) % 90]
                && smooth[*bin] >= smooth[(bin + 1) % 90]
                && SECOND_SHARE * smooth[*bin] >= smooth[main]
        })
        .max_by(|a, b| smooth[*a].cmp(&smooth[*b]).then(b.cmp(a)))?;
    // The mean of the directions near the peak, over a quarter turn.
    let centre = second as f64 - 45.0 + 0.5;
    let [mut cos, mut sin] = [0.0f64; 2];
    for along in &found {
        let gap = (along - centre + 45.0).rem_euclid(90.0) - 45.0;
        if gap.abs() <= SECOND_REACH_DEGREES {
            let angle = (4.0 * along).to_radians();
            cos += angle.cos();
            sin += angle.sin();
        }
    }
    let mean = sin.atan2(cos).to_degrees() / 4.0;
    Some(normalized_degrees(survey.frame.rotation_deg + mean))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plans::survey::tests::{resident, true_frame};
    use crate::plans::survey::{survey_scene, FootprintConfig, SurveyConfig};
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
        // The strays far below are the parts below the box.
        assert!(found.below_points as f64 >= 0.7 * strays.len() as f64);
        assert!(found.below_points <= found.outside_points);
        assert!(found.below_groups >= 1 && found.below_groups <= found.outside_groups);
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
        assert_eq!((found.below_points, found.below_groups), (0, 0));
        assert_eq!(robust_bounds(&[], &config).unwrap(), None);
    }

    /// The distance from a position to the nearest edge of a closed ring.
    fn to_ring(at: [f64; 2], ring: &[[f64; 2]]) -> f64 {
        (0..ring.len())
            .map(|index| {
                let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let along = (((at[0] - a[0]) * dx + (at[1] - a[1]) * dy) / (dx * dx + dy * dy))
                    .clamp(0.0, 1.0);
                (at[0] - a[0] - along * dx).hypot(at[1] - a[1] - along * dy)
            })
            .fold(f64::INFINITY, f64::min)
    }

    fn everything(_: usize, _: u64, _: &Point) -> bool {
        true
    }

    #[test]
    fn the_frame_follows_the_walls_and_the_footprint_the_facades() {
        let spec = BuildingSpec {
            spacing: 0.04,
            ..BuildingSpec::default()
        };
        let scan = building(&spec);
        let points = resident(&scan.points);
        let source = RegionSource::resident(&points, SourceTransform::default());
        let core = robust_bounds(&[source], &RobustBoundsConfig::default())
            .unwrap()
            .unwrap()
            .bounds;
        let find = || {
            building_frame(&[source], core, &everything, &mut |_| Ok(()))
                .unwrap()
                .unwrap()
        };
        let frame = on_threads(1, find);
        assert_eq!(on_threads(6, find), frame);
        // 17 degrees, to within 0.05.
        assert!((frame.rotation_deg - 17.0).abs() < 0.05, "{frame:?}");
        assert_eq!(frame.second_direction_deg, None);
        assert_eq!(
            frame.origin,
            [core.center()[0].round(), core.center()[1].round()]
        );

        let survey = survey_scene(
            &[source],
            OrientedBox::from(core),
            &frame,
            &everything,
            &SurveyConfig::default(),
            &mut |_| Ok(()),
        )
        .unwrap();
        let walls = survey.wall_columns(1.0);
        assert_eq!(second_direction(&survey, &walls), None);
        let footprint = survey.footprint_proposal(&FootprintConfig::default());
        assert_eq!(footprint.regions.len(), 1, "{:?}", footprint.regions);
        let ring = &footprint.regions[0].outer;
        assert!(footprint.regions[0].holes.is_empty());
        // Within 0.10 m of the outer faces of the facades, both ways.
        let truth: Vec<[f64; 2]> = spec
            .footprint()
            .iter()
            .map(|corner| frame.to_plan(*corner))
            .collect();
        for corner in ring {
            assert!(to_ring(*corner, &truth) < 0.10, "{corner:?} {truth:?}");
        }
        for corner in &truth {
            assert!(to_ring(*corner, ring) < 0.10, "{corner:?} {ring:?}");
        }
        assert!(ring.len() <= 8, "{ring:?}");
        let area = footprint.area();
        assert!(area > 96.0 && area < 98.5, "{area}");
        // The mask is the building: inside the rooms and the walls, not on
        // the ground around it.
        let grid = survey.grid;
        let inside = |uv: [f64; 2]| {
            let [x, y] = grid.column_of(uv).unwrap();
            footprint.mask.get(x as i64, y as i64)
        };
        let local = |x: f64, y: f64| {
            let at = spec.to_scene([x, y, 0.0]);
            frame.to_plan([at[0], at[1]])
        };
        for (x, y) in [
            (2.0, 2.0),
            (6.0, 4.0),
            (11.85, 7.85),
            (0.15, 0.15),
            (10.0, 7.5),
        ] {
            assert!(inside(local(x, y)), "{x} {y}");
        }
        for (x, y) in [
            (-0.2, 3.0),
            (12.2, 3.0),
            (6.0, -0.2),
            (6.0, 8.2),
            (-3.0, -3.0),
        ] {
            assert!(!inside(local(x, y)), "{x} {y}");
        }
    }

    #[test]
    fn a_wing_at_another_angle_is_a_second_direction() {
        // One storey each, with a point every 3 cm: a wall at an angle to
        // the columns has points in most of the columns it crosses.
        let main = BuildingSpec {
            storeys: 1,
            spacing: 0.03,
            stray_share: 0.0,
            ..BuildingSpec::default()
        };
        let at = main.to_scene([18.0, -2.0, 0.0]);
        let wing = BuildingSpec {
            size: [8.0, 6.0],
            storeys: 1,
            partitions: Vec::new(),
            openings: Vec::new(),
            lowered_ceiling: None,
            desks: Vec::new(),
            cabinets: Vec::new(),
            site_margin: 1.0,
            rotation_degrees: 47.0,
            translation: [at[0], at[1], main.translation[2]],
            seed: 3,
            ..main.clone()
        };
        let mut all = building(&main).points;
        all.extend(building(&wing).points);
        let points = resident(&all);
        let source = RegionSource::resident(&points, SourceTransform::default());
        let frame = true_frame(&main);
        let core = robust_bounds(&[source], &RobustBoundsConfig::default())
            .unwrap()
            .unwrap()
            .bounds;
        let run = || {
            let survey = survey_scene(
                &[source],
                OrientedBox::from(core),
                &frame,
                &everything,
                &SurveyConfig::default(),
                &mut |_| Ok(()),
            )
            .unwrap();
            let walls = survey.wall_columns(1.0);
            (
                second_direction(&survey, &walls),
                survey.footprint_proposal(&FootprintConfig::default()),
            )
        };
        let (second, footprint) = on_threads(1, run);
        assert_eq!(on_threads(5, run), (second, footprint.clone()));
        let second = second.unwrap();
        assert!((second - 47.0).abs() < 1.0, "{second}");
        // Two buildings, each with its own outline.
        assert_eq!(footprint.regions.len(), 2);
        assert!((footprint.regions[1].area() - 48.0).abs() < 1.5);
    }
}
