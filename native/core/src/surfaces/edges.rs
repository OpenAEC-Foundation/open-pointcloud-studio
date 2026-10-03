//! Where two faces meet: the line both planes share, the stretches of it
//! along which both faces are present, and the corners of the outlines moved
//! onto those lines. An outline traced from points ends within a cell of
//! the true edge; the line of two fitted planes is as exact as the planes
//! are, so a wall then ends on the line of the floor and not near it.

use std::collections::HashSet;

use super::chart::Frame;
use super::{MIN_EDGE_ANGLE_DEG, MIN_EDGE_LENGTH};
use crate::grid2d::{ring_contains, ring_signed_area, simplify_ring, Mask, Region};
use crate::local_fit::{cross, difference, dot, unit};

/// Gaps of this many cells in the presence of a face along a line do not
/// break an edge.
const BRIDGED_CELLS: i64 = 2;
/// Corners nearer to a line than this lie on it: corners that were moved
/// onto one line lie on it to rounding only.
const ON_LINE: f64 = 1e-9;
/// How far past a corner of an outline on an edge line the face is looked
/// for, to tell a corner where it stops from one where it only turns.
const PROBE: f64 = 0.001;

/// The line two planes share and where along it both faces lie. Stretches
/// shorter than `MIN_EDGE_LENGTH` are left out by `keep_edges` at the end.
pub(crate) struct Line {
    /// Positions of the two faces in the list of faces, lower first.
    pub(crate) faces: [usize; 2],
    pub(crate) point: [f64; 3],
    pub(crate) direction: [f64; 3],
    /// Stretches along the line, as distances from `point`.
    pub(crate) segments: Vec<[f64; 2]>,
    /// The angle between the two faces on the side their normals point to.
    pub(crate) angle_deg: f64,
}

impl Line {
    pub(crate) fn at(&self, along: f64) -> [f64; 3] {
        std::array::from_fn(|axis| self.point[axis] + along * self.direction[axis])
    }
}

/// The point three planes share, given as rows `normal . x = offset`.
fn solve3(normals: [[f64; 3]; 3], offsets: [f64; 3]) -> Option<[f64; 3]> {
    let [a, b, c] = normals;
    let determinant = dot(a, cross(b, c));
    if determinant.abs() < 1e-9 {
        return None;
    }
    let (bc, ca, ab) = (cross(b, c), cross(c, a), cross(a, b));
    Some(std::array::from_fn(|axis| {
        (offsets[0] * bc[axis] + offsets[1] * ca[axis] + offsets[2] * ab[axis]) / determinant
    }))
}

/// A line of the scene as it lies in the plane of a face.
struct Trace {
    start: [f64; 2],
    direction: [f64; 2],
}

impl Trace {
    fn new(frame: &Frame, line: &Line) -> Self {
        Self {
            start: frame.uv(line.point),
            direction: [dot(line.direction, frame.u), dot(line.direction, frame.v)],
        }
    }

    /// Distance along the line and distance to its left.
    fn along_and_left(&self, uv: [f64; 2]) -> [f64; 2] {
        let from = [uv[0] - self.start[0], uv[1] - self.start[1]];
        [
            from[0] * self.direction[0] + from[1] * self.direction[1],
            self.direction[0] * from[1] - self.direction[1] * from[0],
        ]
    }

    fn at(&self, along: f64) -> [f64; 2] {
        self.beside(along, 0.0)
    }

    /// The position `left` to the left of the line at a distance along it.
    fn beside(&self, along: f64, left: f64) -> [f64; 2] {
        [
            self.start[0] + along * self.direction[0] - left * self.direction[1],
            self.start[1] + along * self.direction[1] + left * self.direction[0],
        ]
    }
}

/// The cells of a face within `reach` of a line, as cell-sized classes
/// along the line: all of them, those where the face lies clearly to the
/// left of the line and those where it lies clearly to the right. A face
/// that ends at the line has cells up to half a cell past it; only cells
/// more than a whole cell out count as lying on a side.
fn presence(frame: &Frame, mask: &Mask, line: &Line, reach: f64) -> [Vec<i64>; 3] {
    let trace = Trace::new(frame, line);
    let grid = mask.frame();
    let mut classes: [Vec<i64>; 3] = Default::default();
    for y in 0..grid.height {
        for x in 0..grid.width {
            if !mask.get(i64::from(x), i64::from(y)) {
                continue;
            }
            let [along, left] = trace.along_and_left(grid.cell_center(x, y));
            if left.abs() <= reach {
                let class = (along / grid.cell).floor() as i64;
                classes[0].push(class);
                if left > grid.cell {
                    classes[1].push(class);
                } else if left < -grid.cell {
                    classes[2].push(class);
                }
            }
        }
    }
    for list in &mut classes {
        list.sort_unstable();
        list.dedup();
    }
    classes
}

/// Runs of classes, first and last included, with small gaps bridged.
fn runs(classes: &[i64]) -> Vec<[i64; 2]> {
    let mut runs: Vec<[i64; 2]> = Vec::new();
    for class in classes {
        match runs.last_mut() {
            Some(run) if class - run[1] <= BRIDGED_CELLS + 1 => run[1] = *class,
            _ => runs.push([*class, *class]),
        }
    }
    runs
}

fn overlap(a: &[[i64; 2]], b: &[[i64; 2]]) -> Vec<[i64; 2]> {
    let mut both = Vec::new();
    for first in a {
        for second in b {
            let (low, high) = (first[0].max(second[0]), first[1].min(second[1]));
            if low <= high {
                both.push([low, high]);
            }
        }
    }
    both.sort_unstable();
    let mut merged: Vec<[i64; 2]> = Vec::new();
    for run in both {
        match merged.last_mut() {
            Some(last) if run[0] - last[1] <= BRIDGED_CELLS + 1 => last[1] = last[1].max(run[1]),
            _ => merged.push(run),
        }
    }
    merged
}

/// The edges between faces that touch. `frames` and `masks` are per face;
/// `pairs` holds positions in them. `reach` is how far from the line a
/// cell still counts as lying along it.
pub(crate) fn intersections(
    frames: &[Frame],
    masks: &[&Mask],
    pairs: &[[usize; 2]],
    reach: f64,
) -> Vec<Line> {
    let sine_limit = MIN_EDGE_ANGLE_DEG.to_radians().sin();
    let mut lines = Vec::new();
    for [a, b] in pairs {
        let (first, second) = (&frames[*a], &frames[*b]);
        let across = cross(first.normal, second.normal);
        if dot(across, across).sqrt() < sine_limit {
            continue;
        }
        let Some(direction) = unit(across) else {
            continue;
        };
        // The point of the line nearest to the middle of the two faces.
        let middle: [f64; 3] =
            std::array::from_fn(|axis| (first.origin[axis] + second.origin[axis]) * 0.5);
        let Some(point) = solve3(
            [first.normal, second.normal, direction],
            [
                dot(first.normal, first.origin),
                dot(second.normal, second.origin),
                dot(direction, middle),
            ],
        ) else {
            continue;
        };
        let mut line = Line {
            faces: [*a, *b],
            point,
            direction,
            segments: Vec::new(),
            angle_deg: 0.0,
        };
        let cell = masks[*a].frame().cell;
        let [classes_a, left_a, right_a] = presence(first, masks[*a], &line, reach);
        let [classes_b, left_b, right_b] = presence(second, masks[*b], &line, reach);
        let stretches = overlap(&runs(&classes_a), &runs(&classes_b));
        // Without a stretch of some length the two share no edge. With one,
        // the shorter stretches stay as well for now: an outline is moved
        // onto the line beside them too, and whether they are edges is
        // decided once their ends are known better than to a cell.
        let long = |run: &[i64; 2]| (run[1] + 1 - run[0]) as f64 * cell >= MIN_EDGE_LENGTH;
        if !stretches.iter().any(long) {
            continue;
        }
        line.segments = stretches
            .iter()
            .map(|run| [run[0] as f64 * cell, (run[1] + 1) as f64 * cell])
            .collect();
        // In a hollow corner each face lies on the side the other's normal
        // points to; on an outward corner on the other side. A face that
        // runs on past the line, such as a floor under a wall, lies on both
        // sides: it counts as lying on a side when at least a quarter as
        // much of the edge has it there as has it on its other side, so
        // that a few stray cells make no side.
        let along_edge = |classes: &[i64]| {
            classes
                .iter()
                .filter(|class| {
                    stretches
                        .iter()
                        .any(|run| (run[0]..=run[1]).contains(*class))
                })
                .count()
        };
        let in_front = |frame: &Frame, left: &[i64], right: &[i64], other: &Frame| {
            let (left, right) = (along_edge(left), along_edge(right));
            // To the left of the line, in the plane of the face.
            let towards = if dot(other.normal, cross(frame.normal, direction)) > 0.0 {
                left
            } else {
                right
            };
            towards > 0 && 4 * towards >= left.max(right)
        };
        let hollow = in_front(first, &left_a, &right_a, second)
            && in_front(second, &left_b, &right_b, first);
        let between = dot(first.normal, second.normal)
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees();
        line.angle_deg = if hollow {
            180.0 - between
        } else {
            180.0 + between
        };
        lines.push(line);
    }
    lines
}

/// Move the ends of every edge that stops near a corner of three faces
/// onto that corner.
pub(crate) fn snap_ends(frames: &[Frame], lines: &mut [Line], reach: f64) {
    let sine_limit = MIN_EDGE_ANGLE_DEG.to_radians().sin();
    let mut touching = vec![Vec::new(); frames.len()];
    for line in lines.iter() {
        touching[line.faces[0]].push(line.faces[1]);
        touching[line.faces[1]].push(line.faces[0]);
    }
    for line in lines.iter_mut() {
        // Where the line passes through the plane of a face that touches
        // both of its faces.
        let corners: Vec<f64> = touching[line.faces[0]]
            .iter()
            .filter(|third| touching[line.faces[1]].contains(third))
            .filter_map(|third| {
                let frame = &frames[*third];
                let slope = dot(frame.normal, line.direction);
                (slope.abs() > sine_limit).then(|| -frame.distance(line.point) / slope)
            })
            .collect();
        let nearest = |end: f64| {
            corners
                .iter()
                .copied()
                .filter(|corner| (corner - end).abs() <= reach)
                .min_by(|a, b| (a - end).abs().total_cmp(&(b - end).abs()))
        };
        for segment in &mut line.segments {
            match (nearest(segment[0]), nearest(segment[1])) {
                // A stretch shorter than the reach has both its ends near
                // one corner: the nearer one lies on it, the other is the
                // far end of the stretch.
                (Some(first), Some(last)) if first == last => {
                    if (first - segment[0]).abs() <= (last - segment[1]).abs() {
                        segment[0] = first;
                    } else {
                        segment[1] = last;
                    }
                }
                (first, last) => {
                    segment[0] = first.unwrap_or(segment[0]);
                    segment[1] = last.unwrap_or(segment[1]);
                }
            }
        }
        line.segments.retain(|segment| segment[1] > segment[0]);
    }
}

/// Move the corners of the outer rings that lie near an edge onto its line,
/// and a corner near two edges onto the point where their lines cross.
/// `patches` is per face. A corner moves at most `across` to its line and
/// counts as beside a stretch of an edge up to `along` past its ends; `cell`
/// is the size of the cells the outlines were traced on.
///
/// Holes are left as traced: an opening next to a corner of the room must
/// not come to touch the outline. For the same reason the far side of a
/// strip of the face along a line stays where it is, and a part whose ring
/// would come to cross itself or one of its openings keeps its ring as
/// traced.
pub(crate) fn snap_outlines(
    frames: &[Frame],
    patches: &mut [Vec<Region>],
    lines: &[Line],
    across: f64,
    along: f64,
    cell: f64,
) {
    for (face, patches) in patches.iter_mut().enumerate() {
        let traces: Vec<(Trace, &Line)> = lines
            .iter()
            .filter(|line| line.faces.contains(&face))
            .map(|line| (Trace::new(&frames[face], line), line))
            .collect();
        if traces.is_empty() {
            continue;
        }
        for patch in patches {
            let held: Vec<Vec<bool>> = traces
                .iter()
                .map(|(trace, _)| held_back(&patch.outer, trace, across, cell))
                .collect();
            let mut ring: Vec<[f64; 2]> = patch
                .outer
                .iter()
                .enumerate()
                .map(|(index, corner)| {
                    let free = |line: usize| !held[line][index];
                    snapped_corner(*corner, &traces, &free, across, along)
                })
                .collect();
            // Corners that came to lie on one line are no corners any more,
            // and a strip whose two sides came to lie on it has no width.
            drop_spikes(&mut ring);
            let ring = simplify_ring(&ring, 0.0);
            // Every corner is moved on its own, so the ring may have come to
            // cross itself or an opening. It then bounds no area, and the
            // part keeps its outline as traced.
            if ring.len() >= 3
                && ring_signed_area(&ring) > 0.0
                && still_bounds(&ring, &patch.outer, &patch.holes)
            {
                patch.outer = ring;
            }
        }
    }
}

fn minus(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn turn(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Take out every corner at which a ring turns back along the side it came
/// by, and every corner that repeats the one before it. Corners that were
/// moved onto one line in there-and-back order leave such a spike of no
/// width; the ring bounds the same area without it, but with it its sides
/// lie over each other.
fn drop_spikes(ring: &mut Vec<[f64; 2]>) {
    let mut index = 0;
    // Corners in a row that stayed: once round the ring and it is done.
    let mut stayed = 0;
    while ring.len() >= 3 && stayed < ring.len() {
        let n = ring.len();
        let (before, corner, after) = (
            ring[(index + n - 1) % n],
            ring[index],
            ring[(index + 1) % n],
        );
        let (arrives, leaves) = (minus(corner, before), minus(after, corner));
        let (arrived, left) = (arrives[0].hypot(arrives[1]), leaves[0].hypot(leaves[1]));
        let back = turn(arrives, leaves).abs() <= ON_LINE * arrived.max(left)
            && arrives[0] * leaves[0] + arrives[1] * leaves[1] < 0.0;
        if arrived <= ON_LINE || back {
            ring.remove(index);
            // The corner before it may turn back now.
            index = if index == 0 { n - 2 } else { index - 1 };
            stayed = 0;
        } else {
            index = (index + 1) % n;
            stayed += 1;
        }
    }
}

/// Whether two stretches cross, touch or lie along each other.
fn meet(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    // A stretch without length meets the other where it lies on it.
    let (first, second) = (minus(b, a), minus(d, c));
    if first[0].hypot(first[1]) <= ON_LINE {
        return if second[0].hypot(second[1]) <= ON_LINE {
            let apart = minus(c, a);
            apart[0].hypot(apart[1]) <= ON_LINE
        } else {
            meet(c, d, a, b)
        };
    }
    // On which side of the line from `p` to `q` a position lies: 0 on it.
    let side = |p: [f64; 2], q: [f64; 2], r: [f64; 2]| {
        let along = minus(q, p);
        let left = turn(along, minus(r, p));
        if left.abs() <= ON_LINE * along[0].hypot(along[1]) {
            0
        } else if left > 0.0 {
            1
        } else {
            -1
        }
    };
    let (c_side, d_side) = (side(a, b, c), side(a, b, d));
    if c_side == 0 && d_side == 0 {
        // On one line: they meet where their extents along it do.
        let along = minus(b, a);
        let length = along[0].hypot(along[1]);
        let at = |r: [f64; 2]| {
            let from = minus(r, a);
            (from[0] * along[0] + from[1] * along[1]) / length
        };
        let (first, second) = (at(c), at(d));
        return first.max(second) >= -ON_LINE && first.min(second) <= length + ON_LINE;
    }
    c_side * d_side <= 0 && side(c, d, a) * side(c, d, b) <= 0
}

fn ring_edges(ring: &[[f64; 2]]) -> impl Iterator<Item = [[f64; 2]; 2]> + '_ {
    (0..ring.len()).map(|index| [ring[index], ring[(index + 1) % ring.len()]])
}

/// Whether any two sides of these closed rings cross, touch or lie along
/// each other. Sides that have a corner in common, to the last digit, do
/// not count: those are neighbours, or the sides of a ring that passes
/// through one corner twice.
pub(crate) fn rings_meet(rings: &[&[[f64; 2]]]) -> bool {
    let mut sides: Vec<[[f64; 2]; 2]> = rings.iter().flat_map(|ring| ring_edges(ring)).collect();
    // In the order of where they begin along the first coordinate, a side
    // is only held against those that begin before it ends.
    let low = |side: &[[f64; 2]; 2]| side[0][0].min(side[1][0]);
    sides.sort_by(|a, b| low(a).total_cmp(&low(b)));
    sides.iter().enumerate().any(|(index, first)| {
        let high = first[0][0].max(first[1][0]) + ON_LINE;
        sides[index + 1..]
            .iter()
            .take_while(|second| low(second) <= high)
            .any(|second| {
                !first.iter().any(|corner| second.contains(corner))
                    && meet(first[0], first[1], second[0], second[1])
            })
    })
}

/// Whether a ring made by moving corners of `traced` still bounds an area
/// that holds `holes`: none of its new sides meets another side of it,
/// neighbours at their corner aside, or a side of a hole, and no hole lies
/// outside it. The sides it shares with the traced ring need no look: that
/// ring and its holes kept clear of each other.
fn still_bounds(ring: &[[f64; 2]], traced: &[[f64; 2]], holes: &[Vec<[f64; 2]>]) -> bool {
    let bits = |side: &[[f64; 2]; 2]| side.map(|corner| corner.map(f64::to_bits));
    let known: HashSet<[[u64; 2]; 2]> = ring_edges(traced).map(|side| bits(&side)).collect();
    let own: Vec<[[f64; 2]; 2]> = ring_edges(ring).collect();
    let of_holes: Vec<[[f64; 2]; 2]> = holes.iter().flat_map(|hole| ring_edges(hole)).collect();
    let n = own.len();
    for (index, side) in own.iter().enumerate() {
        if known.contains(&bits(side)) {
            continue;
        }
        let [a, b] = *side;
        let neighbour =
            |other: usize| other == index || (other + 1) % n == index || (index + 1) % n == other;
        let crossed = own
            .iter()
            .enumerate()
            .any(|(other, [c, d])| !neighbour(other) && meet(a, b, *c, *d))
            || of_holes.iter().any(|[c, d]| meet(a, b, *c, *d));
        if crossed {
            return false;
        }
    }
    holes.iter().all(|hole| {
        hole.first()
            .is_none_or(|corner| ring_contains(ring, *corner))
    })
}

/// Where an edge ends freely, such as at a door, its end is known to a
/// cell only. The outlines know better: the corner of an outline that lies
/// on the line near that end is where its face stops. With corners of both
/// faces near, the edge ends where the first of them stops.
pub(crate) fn tighten_ends(
    frames: &[Frame],
    patches: &[Vec<Region>],
    lines: &mut [Line],
    reach: f64,
) {
    for line in lines.iter_mut() {
        // Per face, where along the line it begins and where it ends: the
        // corners of its outline on the line before or after which the face
        // is absent. A corner at which the outline only crosses the line,
        // as that of a floor that runs on through a door, is neither.
        let stops: Vec<[Vec<f64>; 2]> = line
            .faces
            .iter()
            .map(|face| {
                let trace = Trace::new(&frames[*face], line);
                let mut stops = [Vec::new(), Vec::new()];
                for patch in &patches[*face] {
                    for corner in &patch.outer {
                        let [along, left] = trace.along_and_left(*corner);
                        if left.abs() >= 1e-6 {
                            continue;
                        }
                        for (end, step) in [-PROBE, PROBE].into_iter().enumerate() {
                            let present = [-0.1 * PROBE, 0.1 * PROBE].into_iter().any(|aside| {
                                ring_contains(&patch.outer, trace.beside(along + step, aside))
                            });
                            if !present {
                                stops[end].push(along);
                            }
                        }
                    }
                }
                stops
            })
            .collect();
        let nearest = |stops: &Vec<f64>, end: f64| {
            stops
                .iter()
                .copied()
                .filter(|stop| (stop - end).abs() <= reach)
                .min_by(|a, b| (a - end).abs().total_cmp(&(b - end).abs()))
        };
        for segment in &mut line.segments {
            let starts = stops
                .iter()
                .filter_map(|stops| nearest(&stops[0], segment[0]));
            if let Some(start) = starts.max_by(f64::total_cmp) {
                segment[0] = start;
            }
            let ends = stops
                .iter()
                .filter_map(|stops| nearest(&stops[1], segment[1]));
            if let Some(end) = ends.min_by(f64::total_cmp) {
                segment[1] = end;
            }
        }
        line.segments.retain(|segment| segment[1] > segment[0]);
    }
}

/// The corners of a ring that must stay off a line although they are within
/// reach of it. Where the ring runs along the line and back along it more
/// than a cell further off, the two sides bound a strip of the face, such
/// as the wall between a door and the corner of a room. The side nearer to
/// the line is the one that may lie on it; moving the ends of the other onto
/// the line as well would close the strip and widen the opening beside it.
/// Less than a cell apart the two are steps that tracing left.
fn held_back(ring: &[[f64; 2]], trace: &Trace, reach: f64, cell: f64) -> Vec<bool> {
    let count = ring.len();
    let places: Vec<[f64; 2]> = ring
        .iter()
        .map(|corner| trace.along_and_left(*corner))
        .collect();
    // The sides that run along the line with an end within reach: the
    // corner they start from, where along the line they start and end, and
    // how far from it they lie.
    let beside: Vec<(usize, [f64; 2], f64)> = (0..count)
        .filter_map(|index| {
            let (from, to) = (places[index], places[(index + 1) % count]);
            (from[1].abs().min(to[1].abs()) <= reach
                && (to[0] - from[0]).abs() > (to[1] - from[1]).abs())
            .then_some((index, [from[0], to[0]], (from[1] + to[1]).abs() * 0.5))
        })
        .collect();
    let mut held = vec![false; count];
    for (index, span, off) in &beside {
        let strip = beside.iter().any(|(_, other, nearer)| {
            (other[1] - other[0]) * (span[1] - span[0]) < 0.0
                && nearer + cell < *off
                && other[0].min(other[1]) < span[0].max(span[1])
                && span[0].min(span[1]) < other[0].max(other[1])
        });
        if strip {
            held[*index] = true;
            held[(*index + 1) % count] = true;
        }
    }
    held
}

/// `free` tells whether the corner may move onto a line, by its position in
/// `traces`.
fn snapped_corner(
    corner: [f64; 2],
    traces: &[(Trace, &Line)],
    free: &dyn Fn(usize) -> bool,
    across: f64,
    along_reach: f64,
) -> [f64; 2] {
    // The lines this corner lies along: within reach of the line, and
    // beside one of the stretches where both faces are present. The ends
    // of a stretch are known to a cell only, a corner to where its points
    // end, so the two have a reach each.
    let mut near: Vec<(f64, f64, &Trace)> = traces
        .iter()
        .enumerate()
        .filter(|(position, _)| free(*position))
        .filter_map(|(_, (trace, line))| {
            let [along, left] = trace.along_and_left(corner);
            (left.abs() <= across
                && line.segments.iter().any(|segment| {
                    along >= segment[0] - along_reach && along <= segment[1] + along_reach
                }))
            .then_some((left.abs(), along, trace))
        })
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some((_, along, first)) = near.first() else {
        return corner;
    };
    let on_first = first.at(*along);
    let sine_limit = MIN_EDGE_ANGLE_DEG.to_radians().sin();
    for (_, _, second) in &near[1..] {
        let turn =
            first.direction[0] * second.direction[1] - first.direction[1] * second.direction[0];
        if turn.abs() < sine_limit {
            continue;
        }
        // Where the first line crosses the second.
        let [_, left] = second.along_and_left(first.start);
        let crossing = first.at(left / turn);
        let moved = [crossing[0] - corner[0], crossing[1] - corner[1]];
        if moved[0].hypot(moved[1]) <= 2.0 * across {
            return crossing;
        }
    }
    on_first
}

/// Leave out the stretches that are too short for an edge, now that their
/// ends lie where the outlines end, and the lines that keep none.
pub(crate) fn keep_edges(lines: &mut Vec<Line>) {
    for line in lines.iter_mut() {
        line.segments
            .retain(|segment| segment[1] - segment[0] >= MIN_EDGE_LENGTH);
    }
    lines.retain(|line| !line.segments.is_empty());
}

/// Ends of a stretch as points of the scene of the working set.
pub(crate) fn segment_ends(line: &Line, segment: [f64; 2]) -> [[f64; 3]; 2] {
    [line.at(segment[0]), line.at(segment[1])]
}

/// The distance between two points.
pub(crate) fn apart(a: [f64; 3], b: [f64; 3]) -> f64 {
    let between = difference(a, b);
    dot(between, between).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid2d::GridFrame;

    #[test]
    fn three_planes_share_a_point() {
        let point = solve3(
            [[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [1.0, 1.0, 1.0]],
            [3.0, 8.0, 12.0],
        )
        .unwrap();
        assert_eq!(point, [3.0, 4.0, 5.0]);
        assert!(solve3(
            [[1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            [1.0; 3]
        )
        .is_none());
    }

    #[test]
    fn runs_bridge_small_gaps_only() {
        assert_eq!(
            runs(&[0, 1, 2, 5, 6, 10, 11]),
            vec![[0, 6], [10, 11]],
            "a gap of two classes is bridged, one of three is not"
        );
        assert_eq!(
            overlap(&[[0, 10], [20, 30]], &[[5, 22], [28, 40]]),
            vec![[5, 10], [20, 22], [28, 30]]
        );
        assert_eq!(overlap(&[[0, 10]], &[[2, 3], [6, 8]]), vec![[2, 8]]);
        assert!(overlap(&[[0, 3]], &[[9, 12]]).is_empty());
    }

    /// A face of `width` by `height` metres from the origin of its frame,
    /// in cells of 5 cm, all of them filled.
    fn sheet(width: f64, height: f64) -> Mask {
        let grid = GridFrame::new(
            [0.0, 0.0],
            0.05,
            (width / 0.05).round() as u32,
            (height / 0.05).round() as u32,
        )
        .unwrap();
        Mask::from_fn(grid, |_, _| true)
    }

    #[test]
    fn two_faces_at_sixty_degrees_share_one_edge_from_end_to_end() {
        // A floor of 3 by 2 m and a sloping face that rises from its edge
        // y = 0 at sixty degrees, both seen from above.
        let floor = Frame {
            origin: [0.0; 3],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        };
        let (sin, cos) = 60f64.to_radians().sin_cos();
        // Seen from its normal's side, to the right is towards -x.
        let slope = Frame {
            origin: [3.0, 0.0, 0.0],
            u: [-1.0, 0.0, 0.0],
            v: [0.0, -cos, sin],
            normal: [0.0, sin, cos],
        };
        let (floor_mask, slope_mask) = (sheet(3.0, 2.0), sheet(3.0, 1.0));
        let frames = [floor, slope];
        let mut lines = intersections(&frames, &[&floor_mask, &slope_mask], &[[0, 1]], 0.12);
        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line.segments.len(), 1);
        let [start, end] = segment_ends(line, line.segments[0]);
        let (low, high) = if start[0] < end[0] {
            (start, end)
        } else {
            (end, start)
        };
        // Within a cell of the ends of the faces.
        assert!(apart(low, [0.0; 3]) <= 0.05 && apart(high, [3.0, 0.0, 0.0]) <= 0.05);
        // The faces enclose 120 degrees on the side of their normals.
        assert!((line.angle_deg - 120.0).abs() < 1e-9, "{}", line.angle_deg);

        // The corners of both outlines that lie along the edge move onto
        // it; the others stay.
        let mut patches = vec![
            vec![Region {
                outer: vec![[0.0, -0.03], [3.0, -0.04], [3.0, 2.0], [0.0, 2.0]],
                holes: Vec::new(),
            }],
            vec![Region {
                outer: vec![[0.0, 0.02], [3.0, 0.03], [3.0, 1.0], [0.0, 1.0]],
                holes: Vec::new(),
            }],
        ];
        snap_ends(&frames, &mut lines, 0.12);
        snap_outlines(&frames, &mut patches, &lines, 0.12, 0.12, 0.05);
        assert_eq!(
            patches[0][0].outer,
            vec![[0.0, 0.0], [3.0, 0.0], [3.0, 2.0], [0.0, 2.0]]
        );
        for (corner, expected) in
            patches[1][0]
                .outer
                .iter()
                .zip([[0.0, 0.0], [3.0, 0.0], [3.0, 1.0], [0.0, 1.0]])
        {
            assert!((corner[0] - expected[0]).abs() < 1e-12);
            assert!((corner[1] - expected[1]).abs() < 1e-12);
        }
    }

    #[test]
    fn a_wall_on_a_floor_that_runs_on_behind_it_stands_in_a_hollow_corner() {
        // A floor of 4 by 3 m and a wall face at x = 2 from y = 0.5 to 2.5,
        // seen from either side. The floor lies on both sides of the edge,
        // on a grid that is shifted against the wall by parts of a cell:
        // whatever the shift and the side the wall is seen from, there is
        // floor on that side, so the corner is hollow.
        let floor = Frame {
            origin: [0.0; 3],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        };
        for shift in [0.0, 0.01, 0.02, 0.025, 0.03, 0.04] {
            let floor_mask = Mask::from_fn(
                GridFrame::new([-shift, 0.0], 0.05, 81, 60).unwrap(),
                |_, _| true,
            );
            for normal in [[1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]] {
                let wall = Frame {
                    origin: [2.0, 0.5, 0.0],
                    u: [0.0, normal[0], 0.0],
                    v: [0.0, 0.0, 1.0],
                    normal,
                };
                // To the right of a wall seen from -x is towards -y.
                let low = if normal[0] > 0.0 { 0.0 } else { -2.0 };
                let wall_mask =
                    Mask::from_fn(GridFrame::new([low, 0.0], 0.05, 40, 52).unwrap(), |_, _| {
                        true
                    });
                for pair in [[0, 1], [1, 0]] {
                    let lines =
                        intersections(&[floor, wall], &[&floor_mask, &wall_mask], &[pair], 0.12);
                    assert_eq!(lines.len(), 1);
                    assert!(
                        (lines[0].angle_deg - 90.0).abs() < 1e-9,
                        "shift {shift}, normal {normal:?}: {}",
                        lines[0].angle_deg
                    );
                }
            }
        }
        // The same wall hanging below the floor is not in front of it.
        let floor_mask =
            Mask::from_fn(GridFrame::new([0.0, 0.0], 0.05, 80, 60).unwrap(), |_, _| {
                true
            });
        let wall = Frame {
            origin: [2.0, 0.5, 0.0],
            u: [0.0, 1.0, 0.0],
            v: [0.0, 0.0, 1.0],
            normal: [1.0, 0.0, 0.0],
        };
        let below = Mask::from_fn(
            GridFrame::new([0.0, -2.6], 0.05, 40, 52).unwrap(),
            |_, _| true,
        );
        let lines = intersections(&[floor, wall], &[&floor_mask, &below], &[[0, 1]], 0.12);
        assert!(
            (lines[0].angle_deg - 270.0).abs() < 1e-9,
            "{}",
            lines[0].angle_deg
        );
    }

    /// The south wall of a room of 4 by 2.6 m seen from inside, so that to
    /// the right is towards -x, and the line it shares with the west wall:
    /// the vertical through the origin.
    fn south_wall_and_corner() -> ([Frame; 2], [Line; 1]) {
        let south = Frame {
            origin: [0.0; 3],
            u: [-1.0, 0.0, 0.0],
            v: [0.0, 0.0, 1.0],
            normal: [0.0, 1.0, 0.0],
        };
        let west = Frame::new([0.0, 1.5, 1.3], [1.0, 0.0, 0.0]);
        let lines = [Line {
            faces: [0, 1],
            point: [0.0; 3],
            direction: [0.0, 0.0, 1.0],
            segments: vec![[0.0, 2.6]],
            angle_deg: 90.0,
        }];
        ([south, west], lines)
    }

    /// The outline of that wall with a door of 0.9 by 2.1 m that leaves
    /// `strip` metres of wall beside the corner, as traced: it stops 3 mm
    /// short of the corner. After the corners were moved onto the edge.
    fn wall_with_a_door(strip: f64, across: f64, cell: f64) -> Region {
        let (frames, lines) = south_wall_and_corner();
        let mut patches = vec![
            vec![Region {
                outer: vec![
                    [-4.0, 0.0],
                    [-strip - 0.9, 0.0],
                    [-strip - 0.9, 2.1],
                    [-strip, 2.1],
                    [-strip, 0.0],
                    [-0.003, 0.0],
                    [-0.003, 2.6],
                    [-4.0, 2.6],
                ],
                holes: Vec::new(),
            }],
            Vec::new(),
        ];
        snap_outlines(&frames, &mut patches, &lines, across, 0.12, cell);
        patches.swap_remove(0).swap_remove(0)
    }

    #[test]
    fn a_strip_of_wall_between_a_door_and_a_corner_keeps_its_width() {
        // Within reach of the line or not, a strip wider than a cell keeps
        // its width and only the side along the corner moves onto the line.
        let area = 4.0 * 2.6 - 0.9 * 2.1;
        for strip in [0.06, 0.10, 0.118, 0.2] {
            let patch = wall_with_a_door(strip, 0.12, 0.05);
            assert_eq!(patch.outer.len(), 8, "{strip}: {:?}", patch.outer);
            assert_eq!(patch.outer[3], [-strip, 2.1]);
            assert!(patch.outer[5][0].abs() < 1e-12 && patch.outer[6][0].abs() < 1e-12);
            assert!(
                (patch.area() - area).abs() < 1e-9,
                "{strip}: {}",
                patch.area()
            );
        }
        // A strip narrower than a cell cannot be told from a step that
        // tracing left: both its sides move onto the line. What remains is
        // a ring without the strip, and without a spike where it was.
        let patch = wall_with_a_door(0.04, 0.12, 0.05);
        assert_eq!(
            patch.outer,
            vec![
                [-4.0, 0.0],
                [-0.04 - 0.9, 0.0],
                [-0.04 - 0.9, 2.1],
                [0.0, 2.1],
                [0.0, 2.6],
                [-4.0, 2.6]
            ]
        );
        // With cells of 24 cm the corners are still moved 12 cm at most:
        // the jamb of a door 20 cm from the corner stays where it is.
        let patch = wall_with_a_door(0.2, 0.12, 0.24);
        assert_eq!(patch.outer.len(), 8);
        assert_eq!(patch.outer[3], [-0.2, 2.1]);
        assert!((patch.area() - area).abs() < 1e-9);
    }

    #[test]
    fn a_moved_ring_must_keep_clear_of_itself_and_of_its_openings() {
        let traced = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.6], [0.0, 2.6]];
        let window = vec![vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]]];
        assert!(still_bounds(&traced, &traced, &window));
        // Moved outward: it bounds an area that holds the window.
        let wider = [[-0.1, 0.0], [4.0, 0.0], [4.0, 2.6], [-0.1, 2.6]];
        assert!(still_bounds(&wider, &traced, &window));
        // A corner moved into the window, a side moved past it, and a ring
        // that came to cross itself.
        let through = [[0.0, 0.0], [4.0, 0.0], [4.0, 2.6], [1.5, 1.5]];
        assert!(!still_bounds(&through, &traced, &window));
        let past = [[2.5, 0.0], [4.0, 0.0], [4.0, 2.6], [2.5, 2.6]];
        assert!(still_bounds(&past, &traced, &[]));
        assert!(!still_bounds(&past, &traced, &window));
        let bow = [[0.0, 0.0], [4.0, 0.0], [0.0, 2.6], [4.0, 2.6]];
        assert!(!still_bounds(&bow, &traced, &[]));
        // A side that came to lie along the side of the window.
        let along = [[1.0, 0.0], [4.0, 0.0], [4.0, 2.6], [1.0, 2.6]];
        assert!(!still_bounds(&along, &traced, &window));
    }

    #[test]
    fn a_ring_loses_its_spikes_and_keeps_its_area() {
        // A square of 2 m with a spike out along its first side, one back
        // past the corner it started from, and a repeated corner.
        let mut ring = vec![
            [0.0, 0.0],
            [2.3, 0.0],
            [2.0, 0.0],
            [2.0, 2.0],
            [2.0, 2.0],
            [0.0, 2.0],
            [0.0, 1.5],
            [0.0, 2.4],
            [0.0, 1.0],
        ];
        let area = ring_signed_area(&ring);
        drop_spikes(&mut ring);
        assert_eq!(
            ring,
            vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [0.0, 1.0]]
        );
        assert_eq!(ring_signed_area(&ring), area);
        assert_eq!(
            simplify_ring(&ring, 0.0),
            vec![[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]]
        );
        // A spike whose tip is the first corner, and a ring that is nothing
        // but a line there and back.
        let mut ring = vec![[-0.5, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 0.0]];
        drop_spikes(&mut ring);
        assert_eq!(ring, vec![[2.0, 0.0], [2.0, 2.0], [0.0, 0.0]]);
        let mut ring = vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [1.5, 0.0]];
        drop_spikes(&mut ring);
        assert!(ring.len() < 3, "{ring:?}");
    }

    #[test]
    fn rings_that_cross_or_touch_are_told_from_rings_that_share_a_corner() {
        let square = [[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]];
        assert!(!rings_meet(&[&square]));
        // A hole inside, one that crosses a side, one that touches a side
        // with a corner and one that lies along a side.
        let inside = [[0.5, 0.5], [0.5, 1.5], [1.5, 1.5], [1.5, 0.5]];
        assert!(!rings_meet(&[&square, &inside]));
        let across = [[1.5, 0.5], [1.5, 1.5], [2.5, 1.5], [2.5, 0.5]];
        assert!(rings_meet(&[&square, &across]));
        let touching = [[1.0, 1.0], [1.5, 1.5], [2.0, 1.0], [1.5, 0.5]];
        assert!(rings_meet(&[&square, &touching]));
        let along = [[0.5, 0.0], [0.5, 1.0], [1.5, 1.0], [1.5, 0.0]];
        assert!(rings_meet(&[&square, &along]));
        // A bow: two sides that pass each other.
        let bow = [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0], [2.0, 2.0]];
        assert!(rings_meet(&[&bow]));
        // A ring that passes through one corner twice, as tracing gives for
        // cells that touch at a corner, bounds its area all the same.
        let pinched = [
            [0.0, 0.0],
            [1.0, 0.0],
            [1.0, 1.0],
            [2.0, 1.0],
            [2.0, 2.0],
            [1.0, 2.0],
            [1.0, 1.0],
            [0.0, 1.0],
        ];
        assert!(!rings_meet(&[&pinched]));
        // Far apart along the first coordinate, nothing is compared.
        let far = [[5.0, 0.0], [6.0, 0.0], [6.0, 1.0]];
        assert!(!rings_meet(&[&square, &far]));
    }

    #[test]
    fn a_floor_that_runs_on_through_a_door_does_not_end_the_edge_at_its_corner() {
        // A wall along y = 0 from x = 0 to 4 with a door from 1.0 to 2.6,
        // and a floor that runs on through the door for half a metre. The
        // floor has corners on the line at 1.0, where it turns, and at 2.67,
        // where a step of its outline was moved onto the line: it stops at
        // neither, so the edge beside the door begins where the wall does,
        // at 2.6.
        let floor = Frame {
            origin: [0.0; 3],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        };
        let wall = Frame {
            origin: [0.0; 3],
            u: [-1.0, 0.0, 0.0],
            v: [0.0, 0.0, 1.0],
            normal: [0.0, 1.0, 0.0],
        };
        let patches = vec![
            vec![Region {
                outer: vec![
                    [0.0, 0.0],
                    [1.0, 0.0],
                    [1.0, -0.5],
                    [2.6, -0.5],
                    [2.6, -0.03],
                    [2.67, 0.0],
                    [4.0, 0.0],
                    [4.0, 3.0],
                    [0.0, 3.0],
                ],
                holes: Vec::new(),
            }],
            vec![Region {
                outer: vec![
                    [-4.0, 0.0],
                    [-2.6, 0.0],
                    [-2.6, 2.1],
                    [-1.0, 2.1],
                    [-1.0, 0.0],
                    [0.0, 0.0],
                    [0.0, 2.6],
                    [-4.0, 2.6],
                ],
                holes: Vec::new(),
            }],
        ];
        // The stretches as the cells gave them: within a cell of the ends.
        let mut lines = vec![Line {
            faces: [0, 1],
            point: [0.0; 3],
            direction: [1.0, 0.0, 0.0],
            segments: vec![[-0.03, 1.04], [2.63, 4.02]],
            angle_deg: 90.0,
        }];
        tighten_ends(&[floor, wall], &patches, &mut lines, 0.12);
        assert_eq!(lines[0].segments, vec![[0.0, 1.0], [2.6, 4.0]]);
        // A stretch that turns out shorter than an edge is left out.
        lines[0].segments.push([5.0, 5.08]);
        keep_edges(&mut lines);
        assert_eq!(lines[0].segments, vec![[0.0, 1.0], [2.6, 4.0]]);
        lines[0].segments = vec![[5.0, 5.08]];
        keep_edges(&mut lines);
        assert!(lines.is_empty());
    }

    #[test]
    fn an_edge_that_stops_near_a_corner_of_three_faces_ends_on_it() {
        // A floor and two walls that meet in the origin, each traced on a
        // grid that begins 2 cm before the corner.
        let floor = Frame::new([1.03, 1.03, 0.0], [0.0, 0.0, 1.0]);
        let south = Frame::new([1.03, 0.0, 1.03], [0.0, 1.0, 0.0]);
        let west = Frame::new([0.0, 1.03, 1.03], [1.0, 0.0, 0.0]);
        let sheet =
            |low: [f64; 2]| Mask::from_fn(GridFrame::new(low, 0.05, 40, 40).unwrap(), |_, _| true);
        // Each from -0.02 to 1.98 along both of its directions, in its own
        // coordinates: of the floor x and y, of the south wall -x and z, of
        // the west wall y and z, all from a point 1.03 m from the corner.
        let masks = [
            sheet([-1.05, -1.05]),
            sheet([-0.95, -1.05]),
            sheet([-1.05, -1.05]),
        ];
        let frames = [floor, south, west];
        let mut lines = intersections(
            &frames,
            &[&masks[0], &masks[1], &masks[2]],
            &[[0, 1], [0, 2], [1, 2]],
            0.12,
        );
        assert_eq!(lines.len(), 3);
        let nearest_end = |lines: &[Line]| -> Vec<f64> {
            lines
                .iter()
                .map(|line| {
                    segment_ends(line, line.segments[0])
                        .into_iter()
                        .map(|end| apart(end, [0.0; 3]))
                        .fold(f64::INFINITY, f64::min)
                })
                .collect()
        };
        // As found, every edge ends within a cell of the corner, not on it.
        assert!(nearest_end(&lines)
            .iter()
            .all(|off| *off > 0.005 && *off < 0.05));
        snap_ends(&frames, &mut lines, 0.12);
        assert!(nearest_end(&lines).iter().all(|off| *off < 1e-12));
        // The far ends, where no third face is, stay.
        for line in &lines {
            let far = segment_ends(line, line.segments[0])
                .into_iter()
                .map(|end| apart(end, [0.0; 3]))
                .fold(0.0, f64::max);
            assert!(far > 1.9 && far < 2.0, "{far}");
        }
    }

    #[test]
    fn a_short_stretch_of_an_edge_stays_until_its_ends_are_known() {
        // A floor of 3 by 2 m and a wall that stands on its edge y = 0 over
        // its first 5 cm and from 1 m on: the short stretch is no edge of
        // its own, but the outline of the wall is moved onto the line there
        // as well.
        let floor = Frame {
            origin: [0.0; 3],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        };
        let wall = Frame {
            origin: [3.0, 0.0, 0.0],
            u: [-1.0, 0.0, 0.0],
            v: [0.0, 0.0, 1.0],
            normal: [0.0, 1.0, 0.0],
        };
        let floor_mask = sheet(3.0, 2.0);
        // Seen from its normal's side the wall runs from x = 3 towards 0.
        let wall_mask = Mask::from_fn(sheet(3.0, 1.0).frame(), |x, _| !(40..59).contains(&x));
        let frames = [floor, wall];
        let mut lines = intersections(&frames, &[&floor_mask, &wall_mask], &[[0, 1]], 0.12);
        assert_eq!(lines.len(), 1);
        let mut lengths: Vec<f64> = lines[0]
            .segments
            .iter()
            .map(|segment| segment[1] - segment[0])
            .collect();
        lengths.sort_by(f64::total_cmp);
        assert_eq!(lengths.len(), 2);
        assert!((lengths[0] - 0.05).abs() < 1e-9 && (lengths[1] - 2.0).abs() < 1e-9);
        let mut patches = vec![
            Vec::new(),
            vec![Region {
                // The 5 cm at the far end of the wall, as traced: 2 cm short
                // of the floor.
                outer: vec![[2.95, 0.02], [3.0, 0.02], [3.0, 1.0], [2.95, 1.0]],
                holes: Vec::new(),
            }],
        ];
        snap_outlines(&frames, &mut patches, &lines, 0.12, 0.12, 0.05);
        assert_eq!(patches[1][0].outer[0], [2.95, 0.0]);
        assert_eq!(patches[1][0].outer[1], [3.0, 0.0]);
        keep_edges(&mut lines);
        assert_eq!(lines[0].segments.len(), 1);
        // Two faces that share short stretches only share no edge.
        let short = Mask::from_fn(sheet(3.0, 1.0).frame(), |x, _| x >= 59);
        assert!(intersections(&frames, &[&floor_mask, &short], &[[0, 1]], 0.12).is_empty());
    }

    #[test]
    fn a_short_stretch_beside_a_corner_of_three_faces_keeps_its_far_end() {
        // A floor and two walls that meet in the origin. The edge of the
        // floor with the south wall runs along x and has a stretch of 8 cm
        // beside the corner: both its ends are within reach of the corner,
        // and only the nearer one lies on it.
        let frames = [
            Frame::new([1.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            Frame::new([1.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            Frame::new([0.0, 1.0, 1.0], [1.0, 0.0, 0.0]),
        ];
        let line = |faces: [usize; 2], direction: [f64; 3], segments: Vec<[f64; 2]>| Line {
            faces,
            point: [0.0; 3],
            direction,
            segments,
            angle_deg: 90.0,
        };
        let mut lines = vec![
            line([0, 1], [1.0, 0.0, 0.0], vec![[0.01, 0.09], [1.0, 2.0]]),
            line([0, 2], [0.0, 1.0, 0.0], vec![[-0.09, -0.02], [0.02, 2.0]]),
            line([1, 2], [0.0, 0.0, 1.0], vec![[0.03, 2.0]]),
        ];
        snap_ends(&frames, &mut lines, 0.12);
        assert_eq!(lines[0].segments, vec![[0.0, 0.09], [1.0, 2.0]]);
        assert_eq!(lines[1].segments, vec![[-0.09, 0.0], [0.0, 2.0]]);
        assert_eq!(lines[2].segments, vec![[0.0, 2.0]]);
    }

    #[test]
    fn parallel_planes_and_faces_that_do_not_reach_each_other_share_no_edge() {
        let floor = Frame {
            origin: [0.0; 3],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        };
        let shelf = Frame {
            origin: [0.0, 0.0, 0.05],
            ..floor
        };
        // A wall whose plane cuts the floor along y = 0, but that begins a
        // metre above it.
        let wall = Frame {
            origin: [0.0, 0.0, 1.0],
            u: [-1.0, 0.0, 0.0],
            v: [0.0, 0.0, 1.0],
            normal: [0.0, 1.0, 0.0],
        };
        let mask = sheet(2.0, 1.0);
        let frames = [floor, shelf, wall];
        let lines = intersections(&frames, &[&mask, &mask, &mask], &[[0, 1], [0, 2]], 0.12);
        assert!(lines.is_empty());
    }

    #[test]
    fn an_outward_corner_encloses_more_than_a_straight_angle() {
        // Two faces of a pillar, seen from outside: one faces west, one
        // south, and they meet along the vertical through the origin.
        let west = Frame::new([0.0, 0.5, 0.5], [-1.0, 0.0, 0.0]);
        let south = Frame::new([0.5, 0.0, 0.5], [0.0, -1.0, 0.0]);
        // West face: y from 0 to 1 (u = -y), south face: x from 0 to 1.
        let grid =
            |low: [f64; 2]| Mask::from_fn(GridFrame::new(low, 0.05, 20, 20).unwrap(), |_, _| true);
        let (west_mask, south_mask) = (grid([-0.5, -0.5]), grid([-0.5, -0.5]));
        let lines = intersections(&[west, south], &[&west_mask, &south_mask], &[[0, 1]], 0.12);
        assert_eq!(lines.len(), 1);
        assert!(
            (lines[0].angle_deg - 270.0).abs() < 1e-9,
            "{}",
            lines[0].angle_deg
        );
    }
}
