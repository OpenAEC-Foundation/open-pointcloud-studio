//! Round columns and pipes among the working points that no plane took.
//!
//! The left-over points are split into connected groups. In each group
//! pairs of points are drawn: two points with their normals fix a cylinder,
//! and the one that most points of the group lie on is the candidate. That
//! finds a column of which only a part of the round was scanned as well as
//! a whole one. The candidate is fitted to its points by least squares and
//! kept when it is long enough, round enough, fits within the tolerance and
//! is not the band round a ball, which a sphere fits better.
//!
//! The working points are the means of the scan points per voxel, so a pipe
//! must be about two voxels in radius: of a thinner one the means lie
//! inside the round and not on it.

use super::segment::{shifted, Normals, AROUND, NO_LABEL};
use super::voxel_cloud::VoxelCloud;
use super::SurfaceDetectConfig;
use crate::local_fit::{cross, difference, dot, unit};
use crate::LoadError;

/// Smaller groups of left-over points are not looked at.
const MIN_POINTS: usize = 40;
/// Pairs of points tried per group.
const DRAWS: usize = 500;
/// The most cylinders taken from one group: a bundle of pipes.
const MAX_PER_GROUP: usize = 8;
/// The points of a group that the candidates are counted on. A group of
/// more is thinned to this many, evenly.
const COUNTED_POINTS: usize = 4_000;
/// Of the points near the surface of a candidate, this share must fit it.
const MIN_SHARE: f64 = 0.6;
/// Classes round the axis for the covered arc: 5 degrees each.
const ARC_CLASSES: usize = 72;
/// A candidate is the band round a ball, and no cylinder, when a sphere
/// fits its points with less than this share of the cylinder's own misfit.
/// Measured: 0.16 to 0.22 on balls and domes of 0.5 to 1 m radius, 5.8 and
/// up on cylinders.
const BALL_SHARE: f64 = 0.5;

/// A cylinder: the axis through `point` along `axis`, and the radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Cylinder {
    pub(crate) point: [f64; 3],
    pub(crate) axis: [f64; 3],
    pub(crate) radius: f64,
}

impl Cylinder {
    /// How far along the axis a position lies, from `point`, and the vector
    /// from the axis to it.
    pub(crate) fn split(&self, position: [f64; 3]) -> (f64, [f64; 3]) {
        let from = difference(position, self.point);
        let along = dot(from, self.axis);
        (
            along,
            std::array::from_fn(|axis| from[axis] - along * self.axis[axis]),
        )
    }

    /// Distance of a position to the surface, positive outside.
    pub(crate) fn distance(&self, position: [f64; 3]) -> f64 {
        let (_, out) = self.split(position);
        dot(out, out).sqrt() - self.radius
    }

    /// Two unit directions across the axis; with the axis they are
    /// right-handed.
    pub(crate) fn across(&self) -> [[f64; 3]; 2] {
        across(self.axis)
    }
}

/// Two unit directions square to a unit direction and to each other, the
/// first as level as can be.
pub(crate) fn across(axis: [f64; 3]) -> [[f64; 3]; 2] {
    let helper = if axis[2].abs() < 0.9 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let first = unit(cross(helper, axis)).unwrap_or([1.0, 0.0, 0.0]);
    [first, cross(axis, first)]
}

/// Solve a small system of linear equations by elimination.
pub(crate) fn solve<const N: usize>(
    mut matrix: [[f64; N]; N],
    mut right: [f64; N],
) -> Option<[f64; N]> {
    for column in 0..N {
        let pivot = (column..N).max_by(|a, b| {
            matrix[*a][column]
                .abs()
                .total_cmp(&matrix[*b][column].abs())
        })?;
        if matrix[pivot][column].abs() < 1e-12 {
            return None;
        }
        matrix.swap(column, pivot);
        right.swap(column, pivot);
        let (above, below) = matrix.split_at_mut(column + 1);
        let pivot_row = &above[column];
        for (row, values) in below.iter_mut().enumerate() {
            let factor = values[column] / pivot_row[column];
            for (value, pivot) in values.iter_mut().zip(pivot_row).skip(column) {
                *value -= factor * pivot;
            }
            right[column + 1 + row] -= factor * right[column];
        }
    }
    let mut solution = [0.0; N];
    for row in (0..N).rev() {
        let known: f64 = (row + 1..N)
            .map(|index| matrix[row][index] * solution[index])
            .sum();
        solution[row] = (right[row] - known) / matrix[row][row];
    }
    solution
        .iter()
        .all(|value| value.is_finite())
        .then_some(solution)
}

/// The cylinder that two points lie on with the normals they have: its axis
/// is square to both normals, and both normals point at it.
fn from_pair(
    first: ([f64; 3], [f64; 3]),
    second: ([f64; 3], [f64; 3]),
    tolerance: f64,
) -> Option<Cylinder> {
    let (p1, n1) = first;
    let (p2, n2) = second;
    let turn = cross(n1, n2);
    let sine = dot(turn, turn).sqrt();
    // Normals that are nearly parallel leave the axis open.
    if sine < 0.25 {
        return None;
    }
    let axis = turn.map(|value| value / sine);
    // p1 + s n1 and p2 + t n2 meet on the axis, seen along it.
    let between = difference(p2, p1);
    let s = dot(cross(between, n2), axis) / sine;
    let t = dot(cross(between, n1), axis) / sine;
    // A normal has no side here, so only the distances count.
    if (s.abs() - t.abs()).abs() > tolerance {
        return None;
    }
    Some(Cylinder {
        point: std::array::from_fn(|index| p1[index] + s * n1[index]),
        axis,
        radius: 0.5 * (s.abs() + t.abs()),
    })
}

/// Fit a cylinder to points by least squares, starting from one that is
/// near: Gauss-Newton on the direction of the axis, its place across that
/// direction, and the radius.
fn fitted(start: Cylinder, points: &[[f64; 3]]) -> Option<Cylinder> {
    let mut cylinder = start;
    for _ in 0..10 {
        // Turn the axis about the middle of the points along it, so that
        // turning and moving do not do the same thing.
        let middle = points
            .iter()
            .map(|point| cylinder.split(*point).0)
            .sum::<f64>()
            / points.len() as f64;
        cylinder.point =
            std::array::from_fn(|index| cylinder.point[index] + middle * cylinder.axis[index]);
        let [e1, e2] = cylinder.across();
        let mut matrix = [[0.0; 5]; 5];
        let mut right = [0.0; 5];
        for point in points {
            let (along, out) = cylinder.split(*point);
            let reach = dot(out, out).sqrt();
            if reach < 1e-9 {
                continue;
            }
            let outward = out.map(|value| value / reach);
            let (o1, o2) = (dot(outward, e1), dot(outward, e2));
            // How the distance to the surface changes with a tilt of the
            // axis towards e1 and e2, a move of it, and a larger radius.
            let row = [-along * o1, -along * o2, -o1, -o2, -1.0];
            let off = reach - cylinder.radius;
            for a in 0..5 {
                for b in 0..5 {
                    matrix[a][b] += row[a] * row[b];
                }
                right[a] -= row[a] * off;
            }
        }
        let step = solve(matrix, right)?;
        cylinder.axis = unit(std::array::from_fn(|index| {
            cylinder.axis[index] + step[0] * e1[index] + step[1] * e2[index]
        }))?;
        cylinder.point = std::array::from_fn(|index| {
            cylinder.point[index] + step[2] * e1[index] + step[3] * e2[index]
        });
        cylinder.radius += step[4];
        if !(cylinder.radius > 0.0 && cylinder.radius.is_finite()) {
            return None;
        }
        if step.iter().all(|value| value.abs() < 1e-9) {
            break;
        }
    }
    Some(cylinder)
}

/// A small deterministic random generator (xorshift64*).
struct Draws(u64);

impl Draws {
    fn below(&mut self, limit: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 33) as usize % limit
    }
}

/// The points of a group as the search sees them.
struct Group<'a> {
    points: &'a [u32],
    positions: Vec<[f64; 3]>,
    /// Unit normal per point; none where the neighbourhood gave none.
    normals: Vec<Option<[f64; 3]>>,
}

struct Limits {
    tolerance: f64,
    /// Cosine of how far a normal may be turned from pointing at the axis.
    facing: f64,
}

impl Group<'_> {
    /// Whether a point lies on a cylinder: within the tolerance of its
    /// surface and, where it has a normal, with the normal towards the axis.
    fn fits(&self, index: usize, cylinder: &Cylinder, limits: &Limits) -> bool {
        let (_, out) = cylinder.split(self.positions[index]);
        let reach = dot(out, out).sqrt();
        if (reach - cylinder.radius).abs() > limits.tolerance {
            return false;
        }
        match self.normals[index] {
            Some(normal) if reach > 1e-9 => dot(normal, out).abs() / reach >= limits.facing,
            _ => true,
        }
    }
}

/// What is known of a cylinder from the working points.
pub(crate) struct Found {
    pub(crate) cylinder: Cylinder,
    /// The working points on it.
    pub(crate) members: Vec<u32>,
}

/// Root of the mean squared distance of points to the sphere that fits them
/// best; nothing when they fix no sphere.
fn sphere_misfit(points: &[[f64; 3]]) -> Option<f64> {
    let count = points.len() as f64;
    let mut middle = [0.0; 3];
    for point in points {
        for axis in 0..3 {
            middle[axis] += point[axis] / count;
        }
    }
    // |p|^2 = 2 c . p + (r^2 - |c|^2) is linear in the centre and the last
    // term; positions are taken from the middle to keep it well posed.
    let mut matrix = [[0.0; 4]; 4];
    let mut right = [0.0; 4];
    for point in points {
        let from = difference(*point, middle);
        let row = [2.0 * from[0], 2.0 * from[1], 2.0 * from[2], 1.0];
        let square = dot(from, from);
        for a in 0..4 {
            for b in 0..4 {
                matrix[a][b] += row[a] * row[b];
            }
            right[a] += row[a] * square;
        }
    }
    let [x, y, z, rest] = solve(matrix, right)?;
    let centre = [x, y, z];
    let radius = (rest + dot(centre, centre)).sqrt();
    if !radius.is_finite() {
        return None;
    }
    let squares: f64 = points
        .iter()
        .map(|point| {
            let from = difference(difference(*point, middle), centre);
            (dot(from, from).sqrt() - radius).powi(2)
        })
        .sum();
    Some((squares / count).sqrt())
}

/// Whether a candidate holds as a column or pipe: its points cover enough
/// of the round and of the length, they are not the band round a ball, and
/// most points near its surface are on it.
fn accepted(
    cylinder: &Cylinder,
    group: &Group<'_>,
    members: &[usize],
    config: &SurfaceDetectConfig,
) -> bool {
    if !(config.min_radius..=config.max_radius).contains(&cylinder.radius)
        || members.len() < MIN_POINTS
    {
        return false;
    }
    let [e1, e2] = cylinder.across();
    let mut arc = [false; ARC_CLASSES];
    let mut along: Vec<f64> = Vec::with_capacity(members.len());
    for member in members {
        let (at, out) = cylinder.split(group.positions[*member]);
        let angle = dot(out, e2).atan2(dot(out, e1));
        let class =
            ((angle + std::f64::consts::PI) / std::f64::consts::TAU * ARC_CLASSES as f64) as usize;
        arc[class.min(ARC_CLASSES - 1)] = true;
        along.push(at);
    }
    let covered = arc.iter().filter(|class| **class).count() as f64 * 360.0 / ARC_CLASSES as f64;
    if covered < config.min_arc_deg {
        return false;
    }
    along.sort_by(f64::total_cmp);
    let (low, high) = (along[0], along[along.len() - 1]);
    if high - low < config.min_cylinder_length {
        return false;
    }
    // The band round a ball lies within the tolerance of a cylinder too,
    // over a length that grows with its radius. A sphere fits such a band
    // far better than the cylinder does, and no real cylinder: on a
    // cylinder the misfit of the cylinder is the noise already.
    let on: Vec<[f64; 3]> = members
        .iter()
        .map(|member| group.positions[*member])
        .collect();
    let squares: f64 = on
        .iter()
        .map(|point| cylinder.distance(*point).powi(2))
        .sum();
    let misfit = (squares / on.len() as f64).sqrt();
    if sphere_misfit(&on).is_some_and(|ball| ball < BALL_SHARE * misfit) {
        return false;
    }
    // Points near the surface, within the length: a real cylinder has most
    // of them on it; a chance fit through clutter does not.
    let window = 3.0 * config.distance_tolerance;
    let near = group
        .positions
        .iter()
        .filter(|position| {
            let (at, out) = cylinder.split(**position);
            at >= low && at <= high && (dot(out, out).sqrt() - cylinder.radius).abs() <= window
        })
        .count();
    members.len() as f64 >= MIN_SHARE * near as f64
}

/// Look for cylinders in one group of left-over points.
fn search(group: &Group<'_>, config: &SurfaceDetectConfig, seed: u64) -> Vec<Found> {
    let limits = Limits {
        tolerance: config.distance_tolerance,
        facing: (2.0 * config.angle_tolerance_deg).to_radians().cos(),
    };
    let mut free: Vec<usize> = (0..group.points.len()).collect();
    let mut draws = Draws(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut found = Vec::new();
    while found.len() < MAX_PER_GROUP && free.len() >= MIN_POINTS {
        // Candidates are counted on a thinned set; the winner on all.
        let stride = free.len().div_ceil(COUNTED_POINTS);
        let counted: Vec<usize> = free.iter().copied().step_by(stride).collect();
        let count = |cylinder: &Cylinder| {
            counted
                .iter()
                .filter(|index| group.fits(**index, cylinder, &limits))
                .count()
        };
        let mut best: Option<(usize, Cylinder)> = None;
        let mut offer = |cylinder: Option<Cylinder>| {
            let Some(cylinder) = cylinder else {
                return;
            };
            if !(config.min_radius..=config.max_radius).contains(&cylinder.radius) {
                return;
            }
            let score = count(&cylinder);
            if best.as_ref().is_none_or(|(most, _)| score > *most) {
                best = Some((score, cylinder));
            }
        };
        let with_normal: Vec<usize> = free
            .iter()
            .copied()
            .filter(|index| group.normals[*index].is_some())
            .collect();
        if with_normal.len() >= 2 {
            for _ in 0..DRAWS {
                let a = with_normal[draws.below(with_normal.len())];
                let b = with_normal[draws.below(with_normal.len())];
                if let (Some(first), Some(second)) = (group.normals[a], group.normals[b]) {
                    offer(from_pair(
                        (group.positions[a], first),
                        (group.positions[b], second),
                        limits.tolerance,
                    ));
                }
            }
        }
        let Some((_, mut cylinder)) = best else {
            break;
        };
        // Fit to the points on it and take the points on the fit, a few
        // times over, because a better fit holds more points.
        let on_it = |cylinder: &Cylinder| -> Vec<usize> {
            free.iter()
                .copied()
                .filter(|index| group.fits(*index, cylinder, &limits))
                .collect()
        };
        for _ in 0..3 {
            let members = on_it(&cylinder);
            if members.len() < MIN_POINTS {
                break;
            }
            let on: Vec<[f64; 3]> = members
                .iter()
                .map(|index| group.positions[*index])
                .collect();
            match fitted(cylinder, &on) {
                Some(better) => cylinder = better,
                None => break,
            }
        }
        let members = on_it(&cylinder);
        if !accepted(&cylinder, group, &members, config) {
            break;
        }
        let mut taken = vec![false; group.points.len()];
        for member in &members {
            taken[*member] = true;
        }
        free.retain(|index| !taken[*index]);
        found.push(Found {
            cylinder,
            members: members.iter().map(|index| group.points[*index]).collect(),
        });
    }
    found
}

/// Find the cylinders among the working points without a label. `labels` is
/// only read; the points of every cylinder come back with it. `proceed` is
/// called between groups and may stop the work.
pub(crate) fn detect(
    cloud: &VoxelCloud,
    normals: &Normals,
    labels: &[u32],
    config: &SurfaceDetectConfig,
    proceed: &mut dyn FnMut() -> Result<(), LoadError>,
) -> Result<Vec<Found>, LoadError> {
    let total = cloud.len();
    let mut seen = vec![false; total];
    let mut all = Vec::new();
    for start in 0..total as u32 {
        if seen[start as usize] || labels[start as usize] != NO_LABEL {
            continue;
        }
        // The group of left-over points this one is connected to.
        let mut points = vec![start];
        seen[start as usize] = true;
        let mut head = 0;
        while head < points.len() {
            let cell = cloud.cell(points[head]);
            head += 1;
            for offset in AROUND {
                let Some(other) = cloud.find(shifted(cell, offset)) else {
                    continue;
                };
                if !seen[other as usize] && labels[other as usize] == NO_LABEL {
                    seen[other as usize] = true;
                    points.push(other);
                }
            }
        }
        if points.len() < MIN_POINTS {
            continue;
        }
        proceed()?;
        points.sort_unstable();
        let group = Group {
            positions: points.iter().map(|point| cloud.position(*point)).collect(),
            normals: points
                .iter()
                .map(|point| {
                    normals.variation[*point as usize]
                        .is_finite()
                        .then(|| normals.normal[*point as usize].map(f64::from))
                })
                .collect(),
            points: &points,
        };
        all.extend(search(&group, config, u64::from(start) + 1));
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_shapes::{cylinder, CylinderSpec, Noise};

    fn length(v: [f64; 3]) -> f64 {
        dot(v, v).sqrt()
    }

    #[test]
    fn small_systems_are_solved() {
        let solution = solve([[2.0, 1.0], [1.0, 3.0]], [5.0, 10.0]).unwrap();
        assert!((solution[0] - 1.0).abs() < 1e-12 && (solution[1] - 3.0).abs() < 1e-12);
        // A row that needs to be moved up first.
        let solution = solve(
            [[0.0, 2.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 4.0]],
            [4.0, 3.0, 8.0],
        )
        .unwrap();
        assert_eq!(solution, [3.0, 2.0, 2.0]);
        assert!(solve([[1.0, 2.0], [2.0, 4.0]], [1.0, 2.0]).is_none());
    }

    #[test]
    fn a_sphere_fits_the_points_of_a_ball_and_not_those_of_a_cylinder() {
        use crate::test_shapes::sphere;
        let ball = sphere([1.0, 2.0, 3.0], 0.5, 4_000).with_noise(Noise::Gaussian(0.002), 2);
        // The noise that was put in: 2.0 mm.
        let misfit = sphere_misfit(&ball.points).unwrap();
        assert!((0.0018..0.0022).contains(&misfit), "{misfit}");
        // A column of the same radius is nowhere near a sphere: 0.26 m.
        let column = cylinder(&CylinderSpec::column([1.0, 2.0, 0.0], 0.5, 2.0));
        let misfit = sphere_misfit(&column.points).unwrap();
        assert!(misfit > 0.1, "{misfit}");
        // Points on a line fix no sphere.
        let line: Vec<[f64; 3]> = (0..10).map(|step| [f64::from(step), 0.0, 0.0]).collect();
        assert!(sphere_misfit(&line).is_none());
    }

    #[test]
    fn two_points_with_normals_give_their_cylinder() {
        // Radius 0.2 about the vertical through (1, 2).
        let at = |degrees: f64, z: f64| {
            let (sin, cos) = degrees.to_radians().sin_cos();
            ([1.0 + 0.2 * cos, 2.0 + 0.2 * sin, z], [cos, sin, 0.0])
        };
        let cylinder = from_pair(at(10.0, 0.3), at(100.0, 1.1), 0.02).unwrap();
        assert!((cylinder.radius - 0.2).abs() < 1e-12);
        assert!((cylinder.axis[2].abs() - 1.0).abs() < 1e-12);
        assert!((cylinder.point[0] - 1.0).abs() < 1e-12 && (cylinder.point[1] - 2.0).abs() < 1e-12);
        assert!((cylinder.distance([1.3, 2.0, 5.0]) - 0.1).abs() < 1e-12);
        // Normals that point the same way give no axis, and normals that do
        // not meet at one distance give no cylinder.
        assert!(from_pair(at(10.0, 0.3), at(12.0, 1.1), 0.02).is_none());
        let (point, normal) = at(100.0, 1.1);
        let moved = ([point[0], point[1] + 0.1, point[2]], normal);
        assert!(from_pair(at(10.0, 0.3), moved, 0.02).is_none());
    }

    #[test]
    fn a_fit_recovers_a_noisy_cylinder_from_a_poor_start() {
        let spec = CylinderSpec {
            arc_degrees: 150.0,
            ..CylinderSpec::pipe([2.0, 1.0, 0.5], [0.3, 1.0, 0.2], 0.12, 1.5)
        };
        let shape = cylinder(&spec).with_noise(Noise::Gaussian(0.002), 1);
        let truth = spec.frame()[0];
        let [e1, _] = across(truth);
        let start = Cylinder {
            point: std::array::from_fn(|axis| spec.base[axis] + 0.03 * e1[axis]),
            axis: unit(std::array::from_fn(|axis| truth[axis] + 0.08 * e1[axis])).unwrap(),
            radius: 0.15,
        };
        let fit = fitted(start, &shape.points).unwrap();
        // 0.3 mm and 0.02 degree measured, from 2 mm of noise.
        assert!((fit.radius - 0.12).abs() < 0.001, "{}", fit.radius);
        let turn = length(cross(fit.axis, truth)).asin().to_degrees();
        assert!(turn < 0.1, "{turn}");
        let (_, off) = fit.split(spec.base);
        assert!(length(off) < 0.001, "{}", length(off));
    }
}
