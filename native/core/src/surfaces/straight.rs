//! Straight edges for the outline of a face.
//!
//! A ring traced along the cells of a face runs in steps of one cell
//! wherever the edge of the face runs at an angle to the grid. Here every
//! ring is reduced to stretches that stay within a cell and a half of it, a
//! line is fitted to each stretch, the lines that lie within a few degrees
//! of the main direction of the face or across it are turned onto it, lines
//! that continue each other become one, and every corner is put where two
//! lines cross. An edge that is truly slanted, such as that of a sloped roof
//! or of a diagonal opening, stays slanted but is one straight line.
//!
//! A ring that would come out unsound, crossing itself or another ring of
//! its part, or with a hole outside the outer ring, keeps the outline it
//! had.

use super::edges::rings_meet;
use crate::grid2d::{
    ring_contains, ring_perimeter, ring_signed_area, simplify_ring, GridFrame, Region,
};

/// The outline is reduced to stretches that stay within this many cells of
/// it: enough to pass over the steps of one cell that a slanted edge leaves.
pub(crate) const STRAIGHT_CELLS: f64 = 1.5;
/// A line that runs within this many degrees of the main direction, or of
/// the direction across it, is turned onto that direction.
const SQUARE_DEGREES: f64 = 3.0;
/// ... as long as that moves neither end of its stretch by more than this
/// many cells.
const SQUARE_CELLS: f64 = 1.5;
/// Over fewer cells than this a stretch does not show its own direction:
/// it runs from the corner it starts at to the one it ends at.
const SHORT_CELLS: f64 = 4.0;
/// A slanted stretch steps over at least this many rows or columns of cells
/// before its inner corners are taken as lying on its edge.
const STAIR_CELLS: f64 = 1.5;
/// Two neighbouring lines whose directions differ by less than this many
/// degrees, and that lie within `MERGE_CELLS` of each other, are one line.
const MERGE_DEGREES: f64 = 1.0;
const MERGE_CELLS: f64 = 0.5;
/// A corner where two lines cross is used when it lies within this many
/// cells of the corner of the outline it replaces; two lines that are
/// nearly parallel cross anywhere.
const CORNER_REACH_CELLS: f64 = 5.0;
/// A line of at most this many cells between two longer lines that cross at
/// more than `CHAMFER_DEGREES` is left out when they cross near it.
const CHAMFER_CELLS: f64 = 8.0;
const CHAMFER_DEGREES: f64 = 30.0;
/// A short line whose ends lie within this many cells of the line before
/// or after it is a step of the cells along that line.
const STEP_CELLS: f64 = 1.0;
/// The line of a stair goes through the inner corner that has this share of
/// the others further out: the outermost but for a stray one.
const OUTER_SHARE: f64 = 0.2;
/// A ring shorter than this many cells round is too small to straighten.
const MIN_RING_CELLS: f64 = 24.0;
/// The corners of the traced ring lie within this many cells of the
/// straightened one, as the reduction and the squaring may each move an
/// edge by a cell and a half; but for one in `SPIKE_SHARE` at most, such as
/// the end of a spike of one cell wide that the reduction passes over, and
/// none further than twice as far. Every corner of the straightened ring
/// stays within `REACH_CELLS` of the traced one: the stair of cells cuts
/// across a corner of an opening.
const FOLLOW_CELLS: f64 = 3.0;
const SPIKE_SHARE: usize = 10;
const REACH_CELLS: f64 = 4.0;
/// The corners of the ring along a short line that is left out lie within
/// this many cells of one of the lines beside it.
const KEEP_CELLS: f64 = 2.0;
/// Positions along a ring are weighed in pieces of this many cells.
const SAMPLE_CELLS: f64 = 0.25;

/// One part of a face as it was traced: its rings with every step of the
/// cells moved in to the outermost points, the corners that the reduction
/// to `STRAIGHT_CELLS` keeps of each, and the outline it has without
/// straightening. The three hold the same holes in the same order.
pub(crate) struct Traced {
    pub(crate) dense: Region,
    pub(crate) coarse: Vec<Vec<usize>>,
    pub(crate) plain: Region,
}

/// A straight line fitted to a stretch of a ring.
#[derive(Debug, Clone, Copy)]
struct Line {
    point: [f64; 2],
    /// Unit, along the ring: the face lies to its left.
    direction: [f64; 2],
    /// The length of the stretch, from its first corner to its last.
    length: f64,
    /// Whether the direction comes from the stretch and not from its ends.
    measured: bool,
    /// The corners of the outline where its stretch begins and ends.
    start: [f64; 2],
    end: [f64; 2],
    /// The stretch of the ring: the position of its first corner and the
    /// number of sides it has.
    span: [usize; 2],
}

/// The outlines of the parts of one face with straight edges. Every ring
/// that cannot be straightened soundly keeps its plain outline.
pub(crate) fn straightened(parts: &[Traced], grid: &GridFrame) -> Vec<Region> {
    let cell = grid.cell;
    let lines: Vec<Vec<Option<Vec<Line>>>> = parts
        .iter()
        .map(|part| {
            rings_of(&part.dense)
                .zip(&part.coarse)
                .map(|(ring, kept)| fitted_lines(ring, kept, grid))
                .collect()
        })
        .collect();
    let main = main_direction(lines.iter().flatten().flatten().flatten());
    parts
        .iter()
        .zip(&lines)
        .map(|(part, lines)| {
            let rings: Vec<Option<Vec<[f64; 2]>>> = lines
                .iter()
                .zip(rings_of(&part.dense))
                .map(|(lines, traced)| {
                    let straight = straight_ring(traced, lines.clone()?, main, cell)?;
                    follows(&straight, traced, cell).then_some(straight)
                })
                .collect();
            sound_region(part, rings)
        })
        .collect()
}

fn rings_of(region: &Region) -> impl Iterator<Item = &[[f64; 2]]> {
    std::iter::once(region.outer.as_slice()).chain(region.holes.iter().map(Vec::as_slice))
}

/// The plain outline of a part with every straightened ring put in that
/// keeps the part sound, the outer ring first, then the holes in turn.
fn sound_region(part: &Traced, rings: Vec<Option<Vec<[f64; 2]>>>) -> Region {
    let mut region = part.plain.clone();
    for (index, straight) in rings.into_iter().enumerate() {
        let Some(straight) = straight else { continue };
        let mut trial = region.clone();
        match index {
            0 => trial.outer = straight,
            _ => trial.holes[index - 1] = straight,
        }
        if is_sound(&trial, part) {
            region = trial;
        }
    }
    region
}

/// Whether a region bounds an area as the traced part does: rings that
/// neither cross nor touch, each the way round it was, keeping a fair part
/// of its area, and every hole inside the outer ring.
fn is_sound(region: &Region, part: &Traced) -> bool {
    let rings: Vec<&[[f64; 2]]> = rings_of(region).collect();
    let before: Vec<f64> = rings_of(&part.plain).map(ring_signed_area).collect();
    rings.iter().zip(&before).all(|(ring, before)| {
        let after = ring_signed_area(ring);
        ring.len() >= 3
            && after * before > 0.0
            && after.abs() >= 0.5 * before.abs()
            && after.abs() <= 2.0 * before.abs()
            && !turns_back(ring)
    }) && !rings_meet(&rings)
        && region.holes.iter().all(|hole| {
            hole.iter()
                .all(|corner| ring_contains(&region.outer, *corner))
        })
}

/// Whether a ring has a corner where it turns back along the side it came
/// by, or two corners in the same place.
fn turns_back(ring: &[[f64; 2]]) -> bool {
    let n = ring.len();
    (0..n).any(|index| {
        let (a, b, c) = (
            ring[(index + n - 1) % n],
            ring[index],
            ring[(index + 1) % n],
        );
        let (arrives, leaves) = (minus(b, a), minus(c, b));
        let length = norm(arrives).max(norm(leaves));
        norm(arrives) <= 1e-9
            || (cross(arrives, leaves).abs() <= 1e-9 * length && dot(arrives, leaves) < 0.0)
    })
}

/// The lines fitted to the stretches of a ring between the corners `kept`,
/// or nothing when it has fewer than three.
fn fitted_lines(ring: &[[f64; 2]], kept: &[usize], grid: &GridFrame) -> Option<Vec<Line>> {
    if kept.len() < 3 || ring_perimeter(ring) < MIN_RING_CELLS * grid.cell {
        return None;
    }
    let n = ring.len();
    Some(
        (0..kept.len())
            .map(|position| {
                let first = kept[position];
                let last = kept[(position + 1) % kept.len()];
                let steps = (last + n - first) % n;
                let stretch: Vec<[f64; 2]> =
                    (0..=steps).map(|step| ring[(first + step) % n]).collect();
                Line {
                    span: [first, steps],
                    ..fitted_line(&stretch, grid)
                }
            })
            .collect(),
    )
}

/// The line of a stretch of a ring, from its first corner to its last.
///
/// A slanted edge leaves a stair of cells. With every row and column moved
/// in to the outermost points, the ends of its runs tell where the edge is,
/// see `run_ends`, and the line goes through those. Where they are too few,
/// the inner corners of the stair, which lie on the edge or inside it by at
/// most the rise of the edge over one cell, give the line along the
/// outermost of them; the outer corners stick out by up to a cell and are
/// not used. Along an edge that follows the grid, the line lies where most
/// of the stretch lies.
fn fitted_line(stretch: &[[f64; 2]], grid: &GridFrame) -> Line {
    let cell = grid.cell;
    let start = stretch[0];
    let end = stretch[stretch.len() - 1];
    let chord = minus(end, start);
    let length = norm(chord);
    let along = if length > 0.0 {
        [chord[0] / length, chord[1] / length]
    } else {
        [1.0, 0.0]
    };
    let short = Line {
        point: [(start[0] + end[0]) * 0.5, (start[1] + end[1]) * 0.5],
        direction: along,
        length,
        measured: false,
        start,
        end,
        span: [0, 0],
    };
    if length < SHORT_CELLS * cell {
        return short;
    }
    // Positions along the stretch, each weighed by the length it stands for.
    let piece = SAMPLE_CELLS * cell;
    let mut samples: Vec<([f64; 2], f64)> = Vec::new();
    for pair in stretch.windows(2) {
        let side = minus(pair[1], pair[0]);
        let side_length = norm(side);
        let count = (side_length / piece).ceil().max(1.0);
        for index in 0..count as usize {
            let at = (index as f64 + 0.5) / count;
            samples.push((
                [pair[0][0] + at * side[0], pair[0][1] + at * side[1]],
                side_length / count,
            ));
        }
    }
    let Some((centre, mut direction)) = principal(&samples) else {
        return short;
    };
    if dot(direction, along) < 0.0 {
        direction = [-direction[0], -direction[1]];
    }
    // The face lies to the left.
    let inward = [-direction[1], direction[0]];
    let rise = length * direction[0].abs().min(direction[1].abs());
    let inner: Vec<([f64; 2], f64)> = stretch
        .windows(3)
        .filter(|three| cross(minus(three[1], three[0]), minus(three[2], three[1])) < -1e-12)
        .map(|three| (three[1], 1.0))
        .collect();
    if rise >= STAIR_CELLS * cell {
        let on = run_ends(stretch, direction, grid);
        if let Some((point, mut through)) = principal(&on) {
            if dot(through, direction) < 0.0 {
                through = [-through[0], -through[1]];
            }
            let spread = on
                .iter()
                .map(|(at, _)| dot(minus(*at, point), through))
                .fold([f64::INFINITY, f64::NEG_INFINITY], |[low, high], along| {
                    [low.min(along), high.max(along)]
                });
            if on.len() >= 3
                && spread[1] - spread[0] >= 0.5 * length
                && cross(through, direction).abs() <= (2.0 * cell / length).min(0.1)
            {
                return Line {
                    point,
                    direction: through,
                    length,
                    measured: true,
                    start,
                    end,
                    span: [0, 0],
                };
            }
        }
    }
    if rise >= STAIR_CELLS * cell && inner.len() >= 2 {
        // The inner corners give the direction when they agree with the
        // stretch on it.
        let (point, direction) = match principal(&inner) {
            Some((point, mut through)) if inner.len() >= 3 => {
                if dot(through, direction) < 0.0 {
                    through = [-through[0], -through[1]];
                }
                if cross(through, direction).abs() <= (cell / length).min(0.1) {
                    (point, through)
                } else {
                    (centre, direction)
                }
            }
            _ => (centre, direction),
        };
        // An inner corner lies on the edge or up to the rise of the edge
        // over a cell inside it: the line goes along the outer ones, in
        // each half of the stretch.
        let inward = [-direction[1], direction[0]];
        let placed: Vec<[f64; 2]> = inner
            .iter()
            .map(|(at, _)| {
                let from = minus(*at, point);
                [dot(from, direction), dot(from, inward)]
            })
            .collect();
        let outer = |placed: &[[f64; 2]]| -> [f64; 2] {
            let mut offsets: Vec<f64> = placed.iter().map(|at| at[1]).collect();
            offsets.sort_by(f64::total_cmp);
            let along = placed.iter().map(|at| at[0]).sum::<f64>() / placed.len() as f64;
            [
                along,
                offsets[((offsets.len() - 1) as f64 * OUTER_SHARE) as usize],
            ]
        };
        let at = |[along, off]: [f64; 2]| {
            [
                point[0] + along * direction[0] + off * inward[0],
                point[1] + along * direction[1] + off * inward[1],
            ]
        };
        let whole = outer(&placed);
        let mut line = Line {
            point: at([0.0, whole[1]]),
            direction,
            length,
            measured: true,
            start,
            end,
            span: [0, 0],
        };
        if placed.len() >= 4 {
            let mut sorted = placed.clone();
            sorted.sort_by(|a, b| a[0].total_cmp(&b[0]));
            let (first, second) = sorted.split_at(sorted.len() / 2);
            let (a, b) = (at(outer(first)), at(outer(second)));
            let chord = minus(b, a);
            let size = norm(chord);
            if size >= 0.25 * length {
                let through = [chord[0] / size, chord[1] / size];
                if cross(through, direction).abs() <= (cell / length).min(0.1) {
                    line.point = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
                    line.direction = through;
                }
            }
        }
        return line;
    }
    let mut offsets: Vec<(f64, f64)> = samples
        .iter()
        .map(|(at, weight)| (dot(minus(*at, centre), inward), *weight))
        .collect();
    let offset = weighted_median(&mut offsets);
    Line {
        point: [
            centre[0] + offset * inward[0],
            centre[1] + offset * inward[1],
        ],
        direction,
        length,
        measured: true,
        start,
        end,
        span: [0, 0],
    }
}

/// The places on the edge that a stair of cells tells exactly. Every run of
/// the stair lies at the outermost points of its cells, and those lie where
/// the edge leaves the last whole row or column of cells of the run, at the
/// end where the stair steps out: on the grid line at or just inside that
/// end. Runs are the sides of the stair that run more along the stretch than
/// across it.
fn run_ends(stretch: &[[f64; 2]], direction: [f64; 2], grid: &GridFrame) -> Vec<([f64; 2], f64)> {
    let cell = grid.cell;
    let axis = usize::from(direction[0].abs() < direction[1].abs());
    let mut ends = Vec::new();
    for index in 0..stretch.len().saturating_sub(1) {
        let (a, b) = (stretch[index], stretch[index + 1]);
        let side = minus(b, a);
        let length = side[axis].abs();
        if side[1 - axis].abs() > 1e-9 * cell || length < 1e-9 * cell {
            continue;
        }
        let sign = side[axis].signum();
        let mut along = [0.0; 2];
        along[axis] = sign;
        let outward = [along[1], -along[0]];
        let out_before = index > 0 && dot(minus(a, stretch[index - 1]), outward) < -1e-9 * cell;
        let out_after =
            index + 2 < stretch.len() && dot(minus(stretch[index + 2], b), outward) > 1e-9 * cell;
        for (end, out, inward) in [(a, out_before, sign), (b, out_after, -sign)] {
            if !out {
                continue;
            }
            let lines = (end[axis] - grid.origin[axis]) / cell;
            // At or just past the end, into the run; a hair before a grid
            // line counts as on it.
            let line = if inward > 0.0 {
                (lines - 1e-6).ceil()
            } else {
                (lines + 1e-6).floor()
            };
            let at = grid.origin[axis] + line * cell;
            if (at - end[axis]).abs() <= length.min(cell) {
                let mut place = end;
                place[axis] = at;
                ends.push((place, 1.0));
            }
        }
    }
    ends
}

/// The weighted centre of positions and the direction in which they spread
/// most, or nothing for positions without weight.
fn principal(points: &[([f64; 2], f64)]) -> Option<([f64; 2], [f64; 2])> {
    let total: f64 = points.iter().map(|(_, weight)| weight).sum();
    if total <= 0.0 {
        return None;
    }
    let centre = [
        points
            .iter()
            .map(|(at, weight)| at[0] * weight)
            .sum::<f64>()
            / total,
        points
            .iter()
            .map(|(at, weight)| at[1] * weight)
            .sum::<f64>()
            / total,
    ];
    let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
    for (at, weight) in points {
        let (x, y) = (at[0] - centre[0], at[1] - centre[1]);
        xx += weight * x * x;
        xy += weight * x * y;
        yy += weight * y * y;
    }
    let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
    Some((centre, [angle.cos(), angle.sin()]))
}

/// The value below which half of the weight lies.
fn weighted_median(values: &mut [(f64, f64)]) -> f64 {
    values.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = values.iter().map(|(_, weight)| weight).sum::<f64>() * 0.5;
    let mut below = 0.0;
    for (value, weight) in values.iter() {
        below += weight;
        if below >= half {
            return *value;
        }
    }
    values.last().map_or(0.0, |(value, _)| *value)
}

/// The main direction of a face in radians from `u`, from 0 up to a quarter
/// turn: that of the most length of its measured lines, each counted along
/// with the line across it.
fn main_direction<'a>(lines: impl Iterator<Item = &'a Line>) -> f64 {
    let quarter = std::f64::consts::FRAC_PI_2;
    let angles: Vec<(f64, f64)> = lines
        .filter(|line| line.measured)
        .map(|line| {
            (
                line.direction[1]
                    .atan2(line.direction[0])
                    .rem_euclid(quarter),
                line.length,
            )
        })
        .collect();
    let window = 2f64.to_radians();
    // How far apart two directions are, a quarter turn being none.
    let apart = |a: f64, b: f64| {
        let difference = (a - b).rem_euclid(quarter);
        difference.min(quarter - difference)
    };
    let score = |angle: f64| -> f64 {
        angles
            .iter()
            .map(|(other, weight)| weight * (1.0 - apart(angle, *other) / window).max(0.0))
            .sum()
    };
    let Some(best) = angles
        .iter()
        .map(|(angle, _)| *angle)
        .max_by(|a, b| score(*a).total_cmp(&score(*b)).then(b.total_cmp(a)))
    else {
        return 0.0;
    };
    // The weighted mean of the directions near the best, about the best.
    let (mut sum, mut weight) = (0.0, 0.0);
    for (angle, length) in &angles {
        if apart(*angle, best) <= window {
            let mut difference = (angle - best).rem_euclid(quarter);
            if difference > quarter * 0.5 {
                difference -= quarter;
            }
            sum += difference * length;
            weight += length;
        }
    }
    (best + sum / weight).rem_euclid(quarter)
}

/// A ring with straight edges from its fitted lines, or nothing when too
/// few lines remain.
fn straight_ring(
    ring: &[[f64; 2]],
    mut lines: Vec<Line>,
    main: f64,
    cell: f64,
) -> Option<Vec<[f64; 2]>> {
    for line in &mut lines {
        square(line, main, cell);
    }
    drop_steps(&mut lines, cell);
    drop_chamfers(ring, &mut lines, cell);
    merge_collinear(&mut lines, cell);
    if lines.len() < 3 {
        return None;
    }
    let n = lines.len();
    let mut corners = Vec::with_capacity(n + 4);
    for position in 0..n {
        let before = &lines[(position + n - 1) % n];
        let after = &lines[position];
        let turn = cross(before.direction, after.direction);
        let crossing = (turn.abs() >= 1e-6).then(|| {
            // Where the line before meets the line after.
            let along = cross(minus(after.point, before.point), after.direction) / turn;
            [
                before.point[0] + along * before.direction[0],
                before.point[1] + along * before.direction[1],
            ]
        });
        // The outline turns from the one line to the other between the end
        // of the stretch before and the start of the stretch after.
        match crossing {
            Some(at)
                if segment_distance(at, before.end, after.start) <= CORNER_REACH_CELLS * cell =>
            {
                corners.push(at)
            }
            _ => {
                // The two lines step from the one to the other there.
                corners.push(nearest(before, before.end));
                corners.push(nearest(after, after.start));
            }
        }
    }
    let corners = simplify_ring(&corners, 0.0);
    (corners.len() >= 3).then_some(corners)
}

/// Whether a straightened ring follows its traced ring: the corners of
/// either lie near the other.
fn follows(straight: &[[f64; 2]], traced: &[[f64; 2]], cell: f64) -> bool {
    let near = |at: [f64; 2], ring: &[[f64; 2]], cells: f64| {
        (0..ring.len()).any(|index| {
            segment_distance(at, ring[index], ring[(index + 1) % ring.len()]) <= cells * cell
        })
    };
    let off = traced
        .iter()
        .filter(|at| !near(**at, straight, FOLLOW_CELLS))
        .collect::<Vec<_>>();
    straight.iter().all(|at| near(*at, traced, REACH_CELLS))
        && off.len() * SPIKE_SHARE <= traced.len()
        && off
            .iter()
            .all(|at| near(**at, straight, 2.0 * FOLLOW_CELLS))
}

fn segment_distance(at: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let side = minus(b, a);
    let length = dot(side, side);
    let along = if length > 0.0 {
        (dot(minus(at, a), side) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    norm(minus(at, [a[0] + along * side[0], a[1] + along * side[1]]))
}

/// Turn a line onto the main direction or the one across it, when it lies
/// that near and its ends move little; a line too short to show its
/// direction is turned when its ends move little.
fn square(line: &mut Line, main: f64, cell: f64) {
    let angle = line.direction[1].atan2(line.direction[0]);
    let quarter = std::f64::consts::FRAC_PI_2;
    let mut off = (angle - main).rem_euclid(quarter);
    if off > quarter * 0.5 {
        off -= quarter;
    }
    let moved = 0.5 * line.length * off.sin().abs();
    if moved > SQUARE_CELLS * cell || (line.measured && off.abs() > SQUARE_DEGREES.to_radians()) {
        return;
    }
    let turned = angle - off;
    line.direction = [turned.cos(), turned.sin()];
}

/// Leave out a short line whose two ends lie on the line before or after
/// it: a step of the cells that the reduction kept.
fn drop_steps(lines: &mut Vec<Line>, cell: f64) {
    let mut position = 0;
    while lines.len() > 3 && position < lines.len() {
        let n = lines.len();
        let line = &lines[position];
        let on = |other: &Line| {
            other.measured
                && [line.start, line.end].iter().all(|at| {
                    cross(other.direction, minus(*at, other.point)).abs() <= STEP_CELLS * cell
                })
        };
        if !line.measured && (on(&lines[(position + n - 1) % n]) || on(&lines[(position + 1) % n]))
        {
            lines.remove(position);
        } else {
            position += 1;
        }
    }
}

/// Leave out a short line between two longer lines that cross near it,
/// such as the cut that the reduction leaves across a corner of a stair:
/// the corner is where those two lines cross. A short line between two
/// lines that run alongside each other, such as the side of a small step
/// or the head of a narrow opening, stays.
fn drop_chamfers(ring: &[[f64; 2]], lines: &mut Vec<Line>, cell: f64) {
    let limit = CHAMFER_DEGREES.to_radians().sin();
    let mut position = 0;
    while lines.len() > 3 && position < lines.len() {
        let n = lines.len();
        let (before, line, after) = (
            &lines[(position + n - 1) % n],
            &lines[position],
            &lines[(position + 1) % n],
        );
        let turn = cross(before.direction, after.direction);
        let chamfer = line.length <= CHAMFER_CELLS * cell
            && before.length > line.length
            && after.length > line.length
            && before.measured
            && after.measured
            && turn.abs() >= limit;
        let crossing = chamfer.then(|| {
            let along = cross(minus(after.point, before.point), after.direction) / turn;
            [
                before.point[0] + along * before.direction[0],
                before.point[1] + along * before.direction[1],
            ]
        });
        // Every corner of the ring along the short line lies near one of
        // the two lines: what the line cut off was a corner and not a side
        // of the face.
        let near = |line: &Line, at: [f64; 2]| {
            cross(line.direction, minus(at, line.point)).abs() <= KEEP_CELLS * cell
        };
        let cut = |at: [f64; 2]| {
            cross(line.direction, minus(at, line.point)).abs() <= REACH_CELLS * cell
                && (0..=line.span[1])
                    .map(|step| ring[(line.span[0] + step) % ring.len()])
                    .all(|corner| near(before, corner) || near(after, corner))
        };
        match crossing {
            Some(at) if cut(at) => {
                lines.remove(position);
                position = position.saturating_sub(1);
            }
            _ => position += 1,
        }
    }
}

/// Put neighbouring lines that continue each other together into one.
fn merge_collinear(lines: &mut Vec<Line>, cell: f64) {
    let limit = MERGE_DEGREES.to_radians().sin();
    let mut position = 0;
    let mut unchanged = 0;
    while lines.len() > 3 && unchanged < lines.len() {
        let next = (position + 1) % lines.len();
        let (a, b) = (lines[position], lines[next]);
        let aligned =
            cross(a.direction, b.direction).abs() <= limit && dot(a.direction, b.direction) > 0.0;
        let near = |line: &Line, at: [f64; 2]| cross(line.direction, minus(at, line.point)).abs();
        if aligned
            && near(&a, b.point) <= MERGE_CELLS * cell
            && near(&b, a.point) <= MERGE_CELLS * cell
        {
            let total = (a.length + b.length).max(1e-12);
            let (wa, wb) = (a.length / total, b.length / total);
            let mut direction = if a.measured == b.measured {
                [
                    wa * a.direction[0] + wb * b.direction[0],
                    wa * a.direction[1] + wb * b.direction[1],
                ]
            } else if a.measured {
                a.direction
            } else {
                b.direction
            };
            let size = norm(direction);
            direction = [direction[0] / size, direction[1] / size];
            let merged = Line {
                point: [
                    wa * a.point[0] + wb * b.point[0],
                    wa * a.point[1] + wb * b.point[1],
                ],
                direction,
                length: a.length + b.length,
                measured: a.measured || b.measured,
                start: a.start,
                end: b.end,
                span: [a.span[0], a.span[1] + b.span[1]],
            };
            lines[position] = merged;
            lines.remove(next);
            if next < position {
                position -= 1;
            }
            unchanged = 0;
        } else {
            position = next;
            unchanged += 1;
        }
    }
}

/// The point of a line nearest to a position.
fn nearest(line: &Line, at: [f64; 2]) -> [f64; 2] {
    let along = dot(minus(at, line.point), line.direction);
    [
        line.point[0] + along * line.direction[0],
        line.point[1] + along * line.direction[1],
    ]
}

fn minus(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn norm(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}
