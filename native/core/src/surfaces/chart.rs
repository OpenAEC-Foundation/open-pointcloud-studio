//! Every plane laid out as a grid in its own two directions, and every
//! cylinder as a grid round its axis and along it: the second pass over the
//! raw points fills the grids and measures the distance of every point to
//! its plane or cylinder. The filled cells give the outline of a face with
//! its openings, and the arc and length of a cylinder that were scanned.
//!
//! All sums of the measuring pass are whole numbers, so that the leaves of
//! an index can be read on several threads and still give the same result
//! on every run.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU8, Ordering};

use super::cylinder::Cylinder;
use super::edges::rings_meet;
use super::segment::{Normals, Segmentation, NO_LABEL};
use super::straight::{straightened, Traced, STRAIGHT_CELLS};
use super::voxel_cloud::VoxelCloud;
use super::{Residuals, SurfaceDetectConfig, FLAT_ANGLE_DEG, MEASURE_WINDOW, MIN_EDGE_ANGLE_DEG};
use crate::grid2d::{CellRegion, Connectivity, GridFrame, Mask, Region};
use crate::local_fit::{cross, difference, dot, unit};
use crate::LoadError;

/// Classes of the deviation histogram, over the measuring window.
const BINS: usize = 128;
/// Cells of all grids together. Beyond this the cell size is doubled.
pub(crate) const MAX_CHART_CELLS: usize = 8_000_000;
/// The fewest cells round a cylinder: 15 degrees each.
const MIN_ROUND_CELLS: f64 = 24.0;
/// The most cells of the grid of one cylinder.
const MAX_ROUND_CELLS: usize = 1_000_000;
/// Deviations are summed in micrometres.
const MICRO: f64 = 1e6;

/// The two directions in a plane that its outline is drawn in: `u` to the
/// right and `v` up for someone who looks at the face from the side its
/// normal points to. `u` is level; on a floor or ceiling it follows x, until
/// `wall_direction` turns it along a wall.
pub(crate) fn plane_axes(normal: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let u = if is_level(normal) {
        None
    } else {
        unit(cross([0.0, 0.0, 1.0], normal))
    }
    .or_else(|| {
        let along = normal[0];
        unit([
            1.0 - along * normal[0],
            -along * normal[1],
            -along * normal[2],
        ])
    })
    .unwrap_or([0.0, 1.0, 0.0]);
    (u, cross(normal, u))
}

/// Whether a plane with this unit normal is a floor or a ceiling: its
/// outline has no side that is "up".
pub(crate) fn is_level(normal: [f64; 3]) -> bool {
    let level = cross([0.0, 0.0, 1.0], normal);
    dot(level, level).sqrt() <= FLAT_ANGLE_DEG.to_radians().sin()
}

/// Of the two ways along a line, the one nearer to x.
pub(crate) fn towards_x(line: [f64; 3]) -> [f64; 3] {
    if line[0] < 0.0 {
        line.map(|value| -value)
    } else {
        line
    }
}

/// The direction a floor or ceiling is laid out along: that of the line it
/// shares with the largest upright plane it touches, so that its grid lies
/// along the walls of a building that is turned from x and y. A free side of
/// the face is then traced as a straight run of cells and not as a
/// staircase. Nothing for an upright plane, or a level one no wall touches.
fn wall_direction(segmentation: &Segmentation, index: usize) -> Option<[f64; 3]> {
    let normal = segmentation.planes[index].normal;
    if !is_level(normal) {
        return None;
    }
    let label = index as u32 + 1;
    let sine_limit = MIN_EDGE_ANGLE_DEG.to_radians().sin();
    let mut best: Option<(u64, [f64; 3])> = None;
    for (a, b) in &segmentation.pairs {
        let other = if *a == label {
            *b
        } else if *b == label {
            *a
        } else {
            continue;
        };
        let wall = &segmentation.planes[other as usize - 1];
        let line = cross(normal, wall.normal);
        if dot(line, line).sqrt() >= sine_limit && best.is_none_or(|(count, _)| wall.count > count)
        {
            best = Some((wall.count, line));
        }
    }
    best.map(|(_, line)| towards_x(line))
}

/// A plane with its two directions.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frame {
    pub(crate) origin: [f64; 3],
    pub(crate) u: [f64; 3],
    pub(crate) v: [f64; 3],
    pub(crate) normal: [f64; 3],
}

impl Frame {
    pub(crate) fn new(origin: [f64; 3], normal: [f64; 3]) -> Self {
        let (u, v) = plane_axes(normal);
        Self {
            origin,
            u,
            v,
            normal,
        }
    }

    /// The same plane with `u` along a direction, as far as that lies in
    /// the plane.
    pub(crate) fn turned_to(self, direction: [f64; 3]) -> Self {
        let across = dot(direction, self.normal);
        match unit(std::array::from_fn(|axis| {
            direction[axis] - across * self.normal[axis]
        })) {
            Some(u) => Self {
                u,
                v: cross(self.normal, u),
                ..self
            },
            None => self,
        }
    }

    pub(crate) fn uv(&self, point: [f64; 3]) -> [f64; 2] {
        let from = difference(point, self.origin);
        [dot(from, self.u), dot(from, self.v)]
    }

    pub(crate) fn point(&self, uv: [f64; 2]) -> [f64; 3] {
        std::array::from_fn(|axis| self.origin[axis] + uv[0] * self.u[axis] + uv[1] * self.v[axis])
    }

    pub(crate) fn distance(&self, point: [f64; 3]) -> f64 {
        dot(difference(point, self.origin), self.normal)
    }
}

/// What one reading thread counts per plane.
#[derive(Clone)]
pub(crate) struct Tally {
    points: u64,
    inliers: u64,
    sum: i64,
    sum_abs: u64,
    sum_squares: u128,
    largest: u64,
    bins: [u32; BINS],
}

impl Tally {
    pub(crate) fn new() -> Self {
        Self {
            points: 0,
            inliers: 0,
            sum: 0,
            sum_abs: 0,
            sum_squares: 0,
            largest: 0,
            bins: [0; BINS],
        }
    }

    fn add(&mut self, micro: i64, tolerance: f64, window: f64) {
        let size = micro.unsigned_abs();
        self.points += 1;
        self.inliers += u64::from(size as f64 <= tolerance * MICRO);
        self.sum += micro;
        self.sum_abs += size;
        self.sum_squares += u128::from(size) * u128::from(size);
        self.largest = self.largest.max(size);
        let bin = (size as f64 / (window * MICRO) * BINS as f64) as usize;
        self.bins[bin.min(BINS - 1)] += 1;
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        self.points += other.points;
        self.inliers += other.inliers;
        self.sum += other.sum;
        self.sum_abs += other.sum_abs;
        self.sum_squares += other.sum_squares;
        self.largest = self.largest.max(other.largest);
        for (bin, more) in self.bins.iter_mut().zip(other.bins) {
            // One thread cannot fill a class; all of them together might.
            *bin = bin.saturating_add(more);
        }
    }

    pub(crate) fn points(&self) -> u64 {
        self.points
    }

    pub(crate) fn residuals(&self, window: f64) -> Residuals {
        if self.points == 0 {
            return Residuals::default();
        }
        let points = self.points as f64;
        // The 95th of a hundred, counted from the smallest, and where in its
        // class it falls when the class is filled evenly.
        let rank = (0.95 * points).ceil().max(1.0) as u64;
        let mut below = 0u64;
        let mut p95 = window;
        for (bin, count) in self.bins.iter().enumerate() {
            let count = u64::from(*count);
            if below + count >= rank {
                let within = (rank - below) as f64 / count as f64;
                p95 = (bin as f64 + within) * window / BINS as f64;
                break;
            }
            below += count;
        }
        let max = self.largest as f64 / MICRO;
        Residuals {
            points: self.points,
            inliers: self.inliers,
            rms: (self.sum_squares as f64 / points).sqrt() / MICRO,
            mean: self.sum as f64 / points / MICRO,
            mean_abs: self.sum_abs as f64 / points / MICRO,
            p95: p95.min(max),
            max,
        }
    }
}

/// Where in a cell its points lie, in 256ths of the cell: lowest and highest
/// along u, lowest and highest along v. Only the points that say where the
/// face is count; a cell without one keeps its lowest above its highest.
type Extent = [AtomicU8; 4];

/// A grid while it is filled from several threads.
struct Cells {
    grid: GridFrame,
    counts: Vec<AtomicU32>,
    sums: Vec<AtomicI64>,
    extents: Vec<Extent>,
}

impl Cells {
    fn new(grid: GridFrame) -> Self {
        Self {
            grid,
            counts: (0..grid.cells()).map(|_| AtomicU32::new(0)).collect(),
            sums: (0..grid.cells()).map(|_| AtomicI64::new(0)).collect(),
            extents: (0..grid.cells())
                .map(|_| {
                    [
                        AtomicU8::new(u8::MAX),
                        AtomicU8::new(0),
                        AtomicU8::new(u8::MAX),
                        AtomicU8::new(0),
                    ]
                })
                .collect(),
        }
    }

    /// Count a point at a position of the grid with its deviation in
    /// micrometres. Says whether the grid holds the position. Only a point
    /// that `outlines` tells where in its cell the face has points.
    fn add(&self, uv: [f64; 2], micro: i64, outlines: bool) -> bool {
        let Some([x, y]) = self.grid.cell_of(uv) else {
            return false;
        };
        let index = self.grid.index(x, y);
        self.counts[index].fetch_add(1, Ordering::Relaxed);
        self.sums[index].fetch_add(micro, Ordering::Relaxed);
        if !outlines {
            return true;
        }
        let within = |axis: usize, cell: u32| {
            let fraction = (uv[axis] - self.grid.origin[axis]) / self.grid.cell - f64::from(cell);
            (fraction * 256.0).clamp(0.0, 255.0) as u8
        };
        let (along, up) = (within(0, x), within(1, y));
        let extent = &self.extents[index];
        extent[0].fetch_min(along, Ordering::Relaxed);
        extent[1].fetch_max(along, Ordering::Relaxed);
        extent[2].fetch_min(up, Ordering::Relaxed);
        extent[3].fetch_max(up, Ordering::Relaxed);
        true
    }

    fn finish(self) -> Filled {
        let counts: Vec<u32> = self.counts.into_iter().map(AtomicU32::into_inner).collect();
        let means = self
            .sums
            .into_iter()
            .zip(&counts)
            .map(|(sum, count)| {
                if *count == 0 {
                    0.0
                } else {
                    (sum.into_inner() as f64 / f64::from(*count) / MICRO) as f32
                }
            })
            .collect();
        Filled {
            grid: self.grid,
            counts,
            means,
            extents: self
                .extents
                .into_iter()
                .map(|extent| extent.map(AtomicU8::into_inner))
                .collect(),
        }
    }
}

/// A grid after measuring.
struct Filled {
    grid: GridFrame,
    counts: Vec<u32>,
    means: Vec<f32>,
    extents: Vec<[u8; 4]>,
}

struct Chart {
    frame: Frame,
    cells: Cells,
}

/// The grid of a cylinder: across it the way round, as the length of arc
/// from the direction opposite `across[0]`, and up it the way along the
/// axis from the point of the cylinder.
struct RoundChart {
    cylinder: Cylinder,
    across: [[f64; 3]; 2],
    cells: Cells,
}

impl RoundChart {
    fn uv(&self, local: [f64; 3]) -> [f64; 2] {
        let (along, out) = self.cylinder.split(local);
        let angle = dot(out, self.across[1]).atan2(dot(out, self.across[0]));
        [self.cylinder.radius * angle, along]
    }
}

/// The grids of all planes and cylinders while the raw points are measured.
pub(crate) struct Charts {
    charts: Vec<Chart>,
    rounds: Vec<RoundChart>,
    tolerance: f64,
    window: f64,
    /// Cosine of the angle tolerance.
    cos_angle: f64,
    /// Sine of the smallest angle at which two planes share an edge.
    sin_edge: f64,
}

impl Charts {
    /// A grid per plane that covers its working points and the voxels round
    /// them, all with one cell size: the one asked for, the voxel size when
    /// that is larger, or what keeps all grids within `max_cells` together.
    /// A cylinder gets cells of that size or smaller, so that there are
    /// enough of them round it.
    pub(crate) fn new(
        cloud: &VoxelCloud,
        segmentation: &Segmentation,
        config: &SurfaceDetectConfig,
        max_cells: usize,
    ) -> Result<Self, LoadError> {
        let frames: Vec<Frame> = segmentation
            .planes
            .iter()
            .enumerate()
            .map(|(index, plane)| {
                let frame = Frame::new(plane.centroid, plane.normal);
                match wall_direction(segmentation, index) {
                    Some(direction) => frame.turned_to(direction),
                    None => frame,
                }
            })
            .collect();
        const EMPTY: [f64; 4] = [
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ];
        let mut spans = vec![EMPTY; frames.len()];
        // Of a cylinder only the reach along its axis is needed.
        let mut reaches = vec![[f64::INFINITY, f64::NEG_INFINITY]; segmentation.cylinders.len()];
        for point in 0..cloud.len() as u32 {
            let label = segmentation.labels[point as usize];
            if label == NO_LABEL {
                continue;
            }
            let index = label as usize - 1;
            let position = cloud.position(point);
            if let Some(frame) = frames.get(index) {
                let [u, v] = frame.uv(position);
                let span = &mut spans[index];
                *span = [
                    span[0].min(u),
                    span[1].min(v),
                    span[2].max(u),
                    span[3].max(v),
                ];
            } else {
                let round = index - frames.len();
                let (along, _) = segmentation.cylinders[round].split(position);
                let reach = &mut reaches[round];
                *reach = [reach[0].min(along), reach[1].max(along)];
            }
        }
        let mut cell = config.boundary_cell.max(cloud.voxel());
        let grids = loop {
            // A raw point lies within a voxel of the mean of its voxel, and
            // its voxel within one of a voxel of the plane.
            let margin = 2.5 * cloud.voxel() + cell;
            let grids = spans
                .iter()
                .map(|span| {
                    let span = if span[0] <= span[2] { *span } else { [0.0; 4] };
                    let low = [
                        ((span[0] - margin) / cell).floor() * cell,
                        ((span[1] - margin) / cell).floor() * cell,
                    ];
                    GridFrame::covering(low, [span[2] + margin, span[3] + margin], cell, usize::MAX)
                })
                .collect::<Result<Vec<_>, _>>();
            match grids {
                Ok(grids)
                    if grids.iter().all(|grid| grid.cell == cell)
                        && grids.iter().map(GridFrame::cells).sum::<usize>() <= max_cells =>
                {
                    break grids;
                }
                _ if cell.is_finite() => cell *= 2.0,
                _ => {
                    return Err(LoadError::InvalidData(
                        "the faces are too large to outline".into(),
                    ))
                }
            }
        };
        let rounds = segmentation
            .cylinders
            .iter()
            .zip(reaches)
            .map(|(cylinder, reach)| {
                let round = std::f64::consts::TAU * cylinder.radius;
                let columns = (round / cell).ceil().max(MIN_ROUND_CELLS);
                let step = round / columns;
                let margin = 2.5 * cloud.voxel() + step;
                let reach = if reach[0] <= reach[1] {
                    reach
                } else {
                    [0.0; 2]
                };
                let low = ((reach[0] - margin) / step).floor() * step;
                let rows = ((reach[1] + margin - low) / step).floor() + 1.0;
                // A cylinder too long for a grid is measured without one:
                // its residuals count, its extent is not found and it is
                // left out in the end.
                let grid = if columns * rows <= MAX_ROUND_CELLS as f64 {
                    GridFrame::new([-0.5 * round, low], step, columns as u32, rows as u32)
                } else {
                    GridFrame::new([-0.5 * round, low], step, columns as u32, 1)
                }?;
                Ok(RoundChart {
                    cylinder: *cylinder,
                    across: cylinder.across(),
                    cells: Cells::new(grid),
                })
            })
            .collect::<Result<Vec<_>, LoadError>>()?;
        Ok(Self {
            charts: frames
                .into_iter()
                .zip(grids)
                .map(|(frame, grid)| Chart {
                    frame,
                    cells: Cells::new(grid),
                })
                .collect(),
            rounds,
            tolerance: config.distance_tolerance,
            window: MEASURE_WINDOW * config.distance_tolerance,
            cos_angle: config.angle_tolerance_deg.to_radians().cos(),
            sin_edge: MIN_EDGE_ANGLE_DEG.to_radians().sin(),
        })
    }

    /// Planes and cylinders together: the tallies a reading thread keeps.
    pub(crate) fn len(&self) -> usize {
        self.charts.len() + self.rounds.len()
    }

    /// Distance of a position to the plane or cylinder with a label,
    /// positive on the side of the normal of a plane and outside a cylinder.
    fn distance(&self, label: u32, local: [f64; 3]) -> f64 {
        let index = label as usize - 1;
        match self.charts.get(index) {
            Some(chart) => chart.frame.distance(local),
            None => self.rounds[index - self.charts.len()]
                .cylinder
                .distance(local),
        }
    }

    /// Measure one raw point, given in the coordinates of the working set:
    /// it belongs to the nearest of the planes and cylinders of its voxel
    /// and the voxels around that it lies within reach of.
    ///
    /// The reach of a plane is the window where the surface at the voxel of
    /// the point runs along that plane: the noise of the face itself, or a
    /// bulge in it. Where the surface stands square to the plane it is a
    /// surface of its own that is no face, such as the reveal of an opening
    /// or the stub of a wall cut off by the region, and only what lies
    /// within the tolerance of the plane counts as on it. A cylinder has
    /// the window for the points of its own voxels.
    pub(crate) fn measure(
        &self,
        tallies: &mut [Tally],
        cloud: &VoxelCloud,
        segmentation: &Segmentation,
        normals: &Normals,
        local: [f64; 3],
    ) {
        let Some(point) = cloud.cell_of(local).and_then(|cell| cloud.find(cell)) else {
            return;
        };
        let point = point as usize;
        let candidates = segmentation.candidates[point]
            .iter()
            .copied()
            .take_while(|label| *label != NO_LABEL);
        let own = segmentation.labels[point];
        let facing = normals.variation[point]
            .is_finite()
            .then(|| normals.normal[point].map(f64::from));
        let mut best: Option<(f64, u32)> = None;
        for label in candidates.clone() {
            let distance = self.distance(label, local);
            let whole = match self.charts.get(label as usize - 1) {
                Some(chart) => facing
                    .is_some_and(|facing| dot(facing, chart.frame.normal).abs() >= self.cos_angle),
                None => label == own,
            };
            let reach = if whole { self.window } else { self.tolerance };
            if distance.abs() <= reach
                && best.is_none_or(|(nearest, _)| distance.abs() < nearest.abs())
            {
                best = Some((distance, label));
            }
        }
        let Some((distance, label)) = best else {
            return;
        };
        let index = label as usize - 1;
        let micro = (distance * MICRO).round() as i64;
        let held = match self.charts.get(index) {
            Some(chart) => {
                // A point within the tolerance of a second plane that
                // crosses this one lies along the line the two share. It
                // may be a point of either face, so it does not say that
                // this face reaches as far as there along that line: the
                // points of a floor would give a wall that stands on it a
                // foot past both its ends.
                let shared = candidates
                    .filter(|other| *other != label)
                    .filter_map(|other| self.charts.get(other as usize - 1))
                    .any(|other| {
                        let across = cross(chart.frame.normal, other.frame.normal);
                        other.frame.distance(local).abs() <= self.tolerance
                            && dot(across, across).sqrt() >= self.sin_edge
                    });
                chart.cells.add(chart.frame.uv(local), micro, !shared)
            }
            None => {
                let round = &self.rounds[index - self.charts.len()];
                round.cells.add(round.uv(local), micro, true)
            }
        };
        if held {
            tallies[index].add(micro, self.tolerance, self.window);
        }
    }

    /// The filled grids, once every point has been measured: those of the
    /// planes and those of the cylinders.
    pub(crate) fn finish(self) -> (Vec<Measured>, Vec<MeasuredRound>) {
        (
            self.charts
                .into_iter()
                .map(|chart| {
                    let filled = chart.cells.finish();
                    Measured {
                        frame: chart.frame,
                        grid: filled.grid,
                        counts: filled.counts,
                        means: filled.means,
                        extents: filled.extents,
                    }
                })
                .collect(),
            self.rounds
                .into_iter()
                .map(|round| MeasuredRound {
                    cylinder: round.cylinder,
                    across: round.across,
                    filled: round.cells.finish(),
                })
                .collect(),
        )
    }
}

/// The grid of one plane after measuring.
pub(crate) struct Measured {
    pub(crate) frame: Frame,
    pub(crate) grid: GridFrame,
    /// Raw points per cell.
    pub(crate) counts: Vec<u32>,
    /// Mean signed distance of the points of a cell to the plane.
    pub(crate) means: Vec<f32>,
    extents: Vec<[u8; 4]>,
}

/// The grid of one cylinder after measuring.
pub(crate) struct MeasuredRound {
    cylinder: Cylinder,
    across: [[f64; 3]; 2],
    filled: Filled,
}

/// The part of a cylinder that was scanned.
pub(crate) struct Mantle {
    pub(crate) cylinder: Cylinder,
    /// Where the scanned length begins and ends, along the axis from the
    /// point of the cylinder.
    pub(crate) along: [f64; 2],
    /// The direction from the axis to where the scanned arc begins. The arc
    /// runs from there towards `side`.
    pub(crate) arc_start: [f64; 3],
    pub(crate) side: [f64; 3],
    pub(crate) arc_deg: f64,
    /// The grid over the whole round and the scanned length: columns from
    /// `first_angle`, in degrees from `arc_start`, and rows from
    /// `first_along`.
    pub(crate) first_angle: f64,
    pub(crate) columns: u32,
    pub(crate) rows: u32,
    pub(crate) step: f64,
    pub(crate) first_along: f64,
    pub(crate) counts: Vec<u32>,
    pub(crate) means: Vec<f32>,
}

impl MeasuredRound {
    /// The length and the arc that hold points: the longest stretch of rows
    /// with points, gaps up to `max_gap` bridged, and in it the arc from
    /// the end of the widest gap round the axis to its beginning. Nothing
    /// for a cylinder without points.
    pub(crate) fn mantle(&self, config: &SurfaceDetectConfig) -> Option<Mantle> {
        let grid = self.filled.grid;
        let (width, height) = (grid.width as usize, grid.height as usize);
        let count = |x: usize, y: usize| self.filled.counts[y * width + x];
        let filled_rows: Vec<bool> = (0..height)
            .map(|y| (0..width).any(|x| count(x, y) > 0))
            .collect();
        let bridged = (config.max_gap / grid.cell).ceil() as usize;
        let mut best: Option<[usize; 2]> = None;
        let mut run: Option<[usize; 2]> = None;
        for (y, filled) in filled_rows.iter().enumerate() {
            if !filled {
                continue;
            }
            run = match run {
                Some([first, last]) if y - last <= bridged.saturating_add(1) => Some([first, y]),
                _ => Some([y, y]),
            };
            if let Some(run) = run {
                if best.is_none_or(|best| run[1] - run[0] > best[1] - best[0]) {
                    best = Some(run);
                }
            }
        }
        let [first, last] = best?;
        // The ends lie at the outermost points of the first and last row.
        let tight = |y: usize, side: usize| {
            (0..width)
                .filter(move |x| count(*x, y) > 0)
                .map(move |x| self.filled.extents[y * width + x][side])
        };
        let low = f64::from(tight(first, 2).min()?) / 256.0;
        let high = (f64::from(tight(last, 3).max()?) + 1.0) / 256.0;
        let along = [
            grid.origin[1] + (first as f64 + low) * grid.cell,
            grid.origin[1] + (last as f64 + high) * grid.cell,
        ];
        let filled_columns: Vec<bool> = (0..width)
            .map(|x| (first..=last).any(|y| count(x, y) > 0))
            .collect();
        // The widest run of empty columns, round the circle: where it ends
        // the scanned arc begins.
        let mut widest = (0usize, 0usize);
        for start in 0..width {
            if filled_columns[start] || !filled_columns[(start + width - 1) % width] {
                continue;
            }
            let length = (0..width)
                .take_while(|step| !filled_columns[(start + step) % width])
                .count();
            if length > widest.0 {
                widest = (length, (start + length) % width);
            }
        }
        let (gap, begin) = widest;
        // The arc begins and ends at the outermost points of its first and
        // last column; one that goes all the way round has no ends.
        let reach = |x: usize, side: usize| {
            (first..=last)
                .filter(move |y| count(x, *y) > 0)
                .map(move |y| self.filled.extents[y * width + x][side])
        };
        let (lead, trail) = if gap == 0 {
            (0.0, 0.0)
        } else {
            let end = (begin + width - gap - 1) % width;
            (
                f64::from(reach(begin, 0).min()?) / 256.0,
                1.0 - (f64::from(reach(end, 1).max()?) + 1.0) / 256.0,
            )
        };
        let column_deg = 360.0 / width as f64;
        let angle = ((begin as f64 + lead) / width as f64 - 0.5) * std::f64::consts::TAU;
        let (sin, cos) = angle.sin_cos();
        let [e1, e2] = self.across;
        let rows = last - first + 1;
        let mut counts = Vec::with_capacity(width * rows);
        let mut means = Vec::with_capacity(width * rows);
        for y in first..=last {
            for x in 0..width {
                let index = y * width + (begin + x) % width;
                counts.push(self.filled.counts[index]);
                means.push(self.filled.means[index]);
            }
        }
        Some(Mantle {
            cylinder: self.cylinder,
            along,
            arc_start: std::array::from_fn(|axis| cos * e1[axis] + sin * e2[axis]),
            side: std::array::from_fn(|axis| cos * e2[axis] - sin * e1[axis]),
            arc_deg: ((width - gap) as f64 - lead - trail) * column_deg,
            first_angle: -lead * column_deg,
            columns: width as u32,
            rows: rows as u32,
            step: grid.cell,
            first_along: grid.origin[1] + first as f64 * grid.cell,
            counts,
            means,
        })
    }
}

/// The outline of one plane.
pub(crate) struct Outline {
    /// The connected parts of the face, each an outer ring with its holes.
    pub(crate) patches: Vec<Region>,
    /// The cells of the face after closing small gaps.
    pub(crate) mask: Mask,
    /// The share of those cells that hold points.
    pub(crate) covered_share: f64,
}

impl Measured {
    /// Whether a cell holds a point that tells where the face is.
    fn outlined(&self, index: usize) -> bool {
        self.extents[index][0] <= self.extents[index][1]
    }

    /// Trace the cells that hold points. Gaps up to `max_gap` are closed,
    /// holes below `min_hole_area` are filled and parts below
    /// `min_region_area` are dropped; what remains is traced along the
    /// outermost points of its border cells and reduced to straight
    /// segments within half a cell. Where that is sound, every ring is then
    /// drawn with straight edges, see `straight`. `proceed` may stop the
    /// work.
    pub(crate) fn outline(
        &self,
        config: &SurfaceDetectConfig,
        proceed: &mut dyn FnMut() -> Result<(), LoadError>,
    ) -> Result<Outline, LoadError> {
        let grid = self.grid;
        let mut mask = Mask::from_fn(grid, |x, y| self.outlined(grid.index(x, y)));
        // Closing with radius r bridges gaps of up to 2 r cells. A square
        // larger than the grid closes exactly as one of the size of the
        // grid, so a large gap costs no more than that.
        let side = f64::from(grid.width.max(grid.height));
        mask.close((config.max_gap / grid.cell / 2.0).ceil().min(side) as u32);
        mask.fill_small_holes(
            grid.cells_for_area(config.min_hole_area),
            Connectivity::Four,
        );
        mask.remove_small_components(
            grid.cells_for_area(config.min_region_area),
            Connectivity::Four,
        );
        let holding = (0..grid.height)
            .flat_map(|y| (0..grid.width).map(move |x| (x, y)))
            .filter(|(x, y)| {
                mask.get(i64::from(*x), i64::from(*y)) && self.counts[grid.index(*x, *y)] > 0
            })
            .count();
        let covered_share = holding as f64 / mask.count().max(1) as f64;
        proceed()?;
        let mut parts = Vec::new();
        for traced in mask.trace(Connectivity::Four) {
            let (tight, apart) = self.tightened(&traced);
            let region = if apart {
                tight.clone()
            } else {
                traced.to_plane(&grid)
            };
            let kept = region.kept_corners(grid.cell * 0.5, proceed)?;
            let ring = |ring: &[[f64; 2]], kept: &[usize]| -> Vec<[f64; 2]> {
                kept.iter().map(|index| ring[*index]).collect()
            };
            let outer = ring(&region.outer, &kept[0]);
            if outer.len() < 3 {
                continue;
            }
            // The holes that keep three corners, each with its ring moved in
            // to the points: straightening starts from those, also where
            // they would pass each other, and keeps only a sound result.
            let holes: Vec<(Vec<[f64; 2]>, Vec<[f64; 2]>)> = region
                .holes
                .iter()
                .zip(&kept[1..])
                .zip(tight.holes)
                .map(|((hole, kept), tight)| (ring(hole, kept), tight))
                .filter(|(hole, _)| hole.len() >= 3)
                .collect();
            let dense = Region {
                outer: tight.outer,
                holes: holes.iter().map(|(_, tight)| tight.clone()).collect(),
            };
            let coarse = dense.kept_corners(grid.cell * STRAIGHT_CELLS, proceed)?;
            parts.push(Traced {
                dense,
                coarse,
                plain: Region {
                    outer,
                    holes: holes.into_iter().map(|(hole, _)| hole).collect(),
                },
            });
        }
        let patches = straightened(&parts, &grid);
        Ok(Outline {
            patches,
            mask,
            covered_share,
        })
    }

    /// The rings of a traced part, moved in to the outermost points, and
    /// whether they keep apart. The two sides of a strip one cell wide are
    /// moved by the points of different cells and can pass each other; the
    /// part then keeps its rings along the cell edges, which bound an area.
    fn tightened(&self, traced: &CellRegion) -> (Region, bool) {
        let region = Region {
            outer: self.tightened_ring(&traced.outer),
            holes: traced
                .holes
                .iter()
                .map(|hole| self.tightened_ring(hole))
                .collect(),
        };
        let rings: Vec<&[[f64; 2]]> = std::iter::once(&region.outer)
            .chain(&region.holes)
            .map(Vec::as_slice)
            .collect();
        let apart = !rings_meet(&rings);
        (region, apart)
    }

    /// A traced ring runs along cell edges, up to a cell beyond the points.
    /// Every edge is moved in to the outermost point of the cells it bounds,
    /// so that the edge of an opening lies where the scan ends and not where
    /// the grid happens to. An edge along cells without points, which only
    /// closing filled, stays.
    fn tightened_ring(&self, ring: &[[i32; 2]]) -> Vec<[f64; 2]> {
        let grid = self.grid;
        let plain = || ring.iter().map(|corner| grid.vertex(*corner)).collect();
        // A ring that meets itself in a corner could come to cross itself.
        let mut sorted = ring.to_vec();
        sorted.sort_unstable();
        if sorted.windows(2).any(|pair| pair[0] == pair[1]) {
            return plain();
        }
        let extent = |x: i32, y: i32, side: usize| -> Option<u8> {
            if x < 0 || y < 0 || x >= grid.width as i32 || y >= grid.height as i32 {
                return None;
            }
            let index = grid.index(x as u32, y as u32);
            self.outlined(index).then(|| self.extents[index][side])
        };
        // Per edge, from a corner to the next: the coordinate it moves to,
        // across its own direction. The cells with points are on its left.
        let moved: Vec<Option<f64>> = (0..ring.len())
            .map(|index| {
                let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
                if a[1] == b[1] {
                    let cells = a[0].min(b[0])..a[0].max(b[0]);
                    let rows = if b[0] > a[0] {
                        // Rightwards: the cells above, their lowest points.
                        cells
                            .filter_map(|x| extent(x, a[1], 2))
                            .min()
                            .map(|low| f64::from(a[1]) + f64::from(low) / 256.0)
                    } else {
                        cells
                            .filter_map(|x| extent(x, a[1] - 1, 3))
                            .max()
                            .map(|high| f64::from(a[1] - 1) + (f64::from(high) + 1.0) / 256.0)
                    };
                    rows.map(|rows| grid.origin[1] + rows * grid.cell)
                } else {
                    let cells = a[1].min(b[1])..a[1].max(b[1]);
                    let columns = if b[1] > a[1] {
                        // Upwards: the cells to the left, their highest u.
                        cells
                            .filter_map(|y| extent(a[0] - 1, y, 1))
                            .max()
                            .map(|high| f64::from(a[0] - 1) + (f64::from(high) + 1.0) / 256.0)
                    } else {
                        cells
                            .filter_map(|y| extent(a[0], y, 0))
                            .min()
                            .map(|low| f64::from(a[0]) + f64::from(low) / 256.0)
                    };
                    columns.map(|columns| grid.origin[0] + columns * grid.cell)
                }
            })
            .collect();
        (0..ring.len())
            .map(|index| {
                // A corner lies between the edge that arrives and the one
                // that leaves; one of them is level and gives its v.
                let before = (index + ring.len() - 1) % ring.len();
                let mut corner = grid.vertex(ring[index]);
                for edge in [before, index] {
                    let (a, b) = (ring[edge], ring[(edge + 1) % ring.len()]);
                    let axis = usize::from(a[1] == b[1]);
                    if let Some(value) = moved[edge] {
                        corner[axis] = value;
                    }
                }
                corner
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid2d::ring_signed_area;

    fn length(v: [f64; 3]) -> f64 {
        dot(v, v).sqrt()
    }

    #[test]
    fn axes_are_level_and_upright_and_turn_with_the_normal() {
        // A wall seen from the east: y to the right, z up.
        assert_eq!(
            plane_axes([1.0, 0.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0])
        );
        // A floor and a ceiling follow x.
        assert_eq!(
            plane_axes([0.0, 0.0, 1.0]),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0])
        );
        // Turned along a wall that runs at 30 degrees, a floor is still
        // seen from above: u, v and the normal are right-handed.
        let (sin, cos) = 30f64.to_radians().sin_cos();
        let floor = Frame::new([1.0, 2.0, 0.0], [0.0, 0.0, 1.0]).turned_to([cos, sin, 0.4]);
        assert!((0..3).all(|axis| (floor.u[axis] - [cos, sin, 0.0][axis]).abs() < 1e-12));
        assert!((0..3).all(|axis| (floor.v[axis] - [-sin, cos, 0.0][axis]).abs() < 1e-12));
        // A direction along the normal turns nothing.
        let same = Frame::new([0.0; 3], [0.0, 0.0, 1.0]).turned_to([0.0, 0.0, 2.0]);
        assert_eq!(same.u, [1.0, 0.0, 0.0]);
        assert_eq!(
            plane_axes([0.0, 0.0, -1.0]),
            ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0])
        );
        for normal in [
            [0.3, -0.4, 0.866],
            [0.0, 0.05, -0.99875],
            [-0.6, 0.64, 0.48],
            [0.999, 0.0, 0.0447],
        ] {
            let normal = unit(normal).unwrap();
            let (u, v) = plane_axes(normal);
            assert!((length(u) - 1.0).abs() < 1e-12 && (length(v) - 1.0).abs() < 1e-12);
            assert!(dot(u, normal).abs() < 1e-12 && dot(u, v).abs() < 1e-12);
            // Right-handed: u, v and the normal.
            let n = cross(u, v);
            assert!((0..3).all(|axis| (n[axis] - normal[axis]).abs() < 1e-12));
            if normal[2].abs() < 0.98 {
                assert!(u[2].abs() < 1e-12 && v[2] > 0.0);
            }
        }
    }

    /// A grid of 40 by 20 cells of 5 cm in the plane z = 0, filled with
    /// points where `holds` says so, each in the middle of its cell.
    fn measured(holds: impl Fn(u32, u32) -> bool) -> Measured {
        let grid = GridFrame::new([0.0, 0.0], 0.05, 40, 20).unwrap();
        let cells = Cells::new(grid);
        for y in 0..20 {
            for x in 0..40 {
                if holds(x, y) {
                    assert!(cells.add(grid.cell_center(x, y), 0, true));
                }
            }
        }
        let filled = cells.finish();
        Measured {
            frame: Frame::new([0.0; 3], [0.0, 0.0, 1.0]),
            grid,
            counts: filled.counts,
            means: filled.means,
            extents: filled.extents,
        }
    }

    #[test]
    fn a_gap_wider_than_the_grid_is_closed_at_the_cost_of_the_grid() {
        // Two parts with 0.5 m between them. The settings refuse a gap of
        // more than a few metres; a caller that skips them still gets an
        // answer, and at once: the closing is that of the size of the grid.
        let parts = measured(|x, _| !(15..25).contains(&x));
        let outline = |max_gap: f64| {
            let config = SurfaceDetectConfig {
                max_gap,
                min_region_area: 0.1,
                ..SurfaceDetectConfig::default()
            };
            parts.outline(&config, &mut || Ok(())).unwrap()
        };
        assert_eq!(outline(0.1).patches.len(), 2);
        let closed = outline(1.0);
        assert_eq!(closed.patches.len(), 1);
        assert_eq!(closed.mask.count(), 800);
        assert!((closed.covered_share - 0.75).abs() < 1e-12);
        for huge in [1e3, 1e7, 1e12, 1e300] {
            let same = outline(huge);
            assert_eq!(same.patches, closed.patches);
            assert_eq!(same.mask.count(), 800);
        }
    }

    #[test]
    fn sides_that_would_pass_each_other_at_their_points_stay_on_the_cell_edges() {
        // Three rows of cells in the shape of an S. In the middle row the
        // cells to the left hold points at their top only and those to the
        // right at their bottom only: the lower side of the left part would
        // move up past the upper side of the right part, and the ring would
        // cross itself where the two meet.
        let grid = GridFrame::new([0.0, 0.0], 0.05, 5, 3).unwrap();
        let cells = Cells::new(grid);
        let fill = |x: u32, y: u32, low: f64, high: f64| {
            for step in 0..=4 {
                let within = low + (high - low) * f64::from(step) / 4.0;
                let u = (f64::from(x) + 0.1 + 0.2 * f64::from(step)) * 0.05;
                assert!(cells.add([u, (f64::from(y) + within) * 0.05], 0, true));
            }
        };
        for x in 0..5 {
            if x < 3 {
                fill(x, 2, 0.1, 0.9);
                fill(x, 1, 0.8, 0.9);
            } else {
                fill(x, 1, 0.1, 0.2);
                fill(x, 0, 0.1, 0.9);
            }
        }
        let filled = cells.finish();
        let measured = Measured {
            frame: Frame::new([0.0; 3], [0.0, 0.0, 1.0]),
            grid,
            counts: filled.counts,
            means: filled.means,
            extents: filled.extents,
        };
        let config = SurfaceDetectConfig {
            max_gap: 0.0,
            min_hole_area: 0.0,
            min_region_area: 0.01,
            ..SurfaceDetectConfig::default()
        };
        let outline = measured.outline(&config, &mut || Ok(())).unwrap();
        assert_eq!(outline.patches.len(), 1);
        let ring = &outline.patches[0].outer;
        assert!(!rings_meet(&[ring.as_slice()]), "{ring:?}");
        // The ten cells as they are.
        assert!((outline.patches[0].area() - 10.0 * 0.0025).abs() < 1e-12);
        assert_eq!(ring.len(), 8);
        // With the points of every cell spread over all of it, the same
        // part is drawn in to its points.
        let cells = Cells::new(grid);
        for (x, y) in [
            (0, 2),
            (1, 2),
            (2, 2),
            (0, 1),
            (1, 1),
            (2, 1),
            (3, 1),
            (4, 1),
            (3, 0),
            (4, 0),
        ] {
            for (u, v) in [(0.1, 0.1), (0.9, 0.9)] {
                assert!(cells.add(
                    [(f64::from(x) + u) * 0.05, (f64::from(y) + v) * 0.05],
                    0,
                    true
                ));
            }
        }
        let filled = cells.finish();
        let even = Measured {
            frame: measured.frame,
            grid,
            counts: filled.counts,
            means: filled.means,
            extents: filled.extents,
        };
        let outline = even.outline(&config, &mut || Ok(())).unwrap();
        let area = outline.patches[0].area();
        assert!(area < 9.0 * 0.0025 && area > 7.0 * 0.0025, "{area}");
    }

    #[test]
    fn a_cell_whose_points_all_lie_along_an_edge_is_counted_but_not_outlined() {
        let grid = GridFrame::new([0.0, 0.0], 0.05, 4, 4).unwrap();
        let cells = Cells::new(grid);
        // Three points in cell (1, 1), of which the last does not tell
        // where the face is, and one such point alone in cell (2, 1).
        assert!(cells.add([0.06, 0.07], 1_000, true));
        assert!(cells.add([0.09, 0.08], 3_000, true));
        assert!(cells.add([0.051, 0.099], 2_000, false));
        assert!(cells.add([0.12, 0.07], 4_000, false));
        assert!(!cells.add([0.3, 0.07], 0, true));
        let filled = cells.finish();
        let measured = Measured {
            frame: Frame::new([0.0; 3], [0.0, 0.0, 1.0]),
            grid,
            counts: filled.counts,
            means: filled.means,
            extents: filled.extents,
        };
        let (first, second) = (grid.index(1, 1), grid.index(2, 1));
        assert_eq!((measured.counts[first], measured.counts[second]), (3, 1));
        assert!((measured.means[first] - 0.002).abs() < 1e-9);
        assert!(measured.outlined(first) && !measured.outlined(second));
        // The extent of the first cell is that of its two outlining points:
        // from 0.2 to 0.8 of the cell along u, 0.4 to 0.6 along v.
        assert_eq!(measured.extents[first], [51, 204, 102, 153]);
    }

    /// A face scanned with a point every `spacing` over a grid of cells of
    /// 5 cm from `origin`, `size` cells large, in the plane z = 0: the
    /// points where `holds` says so of a lattice from the origin up to
    /// `extent`, turned by `degrees`.
    fn scanned(
        origin: [f64; 2],
        size: [u32; 2],
        spacing: f64,
        degrees: f64,
        extent: [f64; 2],
        holds: impl Fn([f64; 2]) -> bool,
    ) -> Measured {
        let grid = GridFrame::new(origin, 0.05, size[0], size[1]).unwrap();
        let cells = Cells::new(grid);
        let (sin, cos) = degrees.to_radians().sin_cos();
        let steps = extent.map(|extent| (extent / spacing).ceil() as i64);
        for i in -1..=steps[0] {
            for j in -1..=steps[1] {
                let local = [(i as f64 + 0.5) * spacing, (j as f64 + 0.5) * spacing];
                if holds(local) {
                    let uv = [
                        cos * local[0] - sin * local[1],
                        sin * local[0] + cos * local[1],
                    ];
                    assert!(cells.add(uv, 0, true), "{uv:?}");
                }
            }
        }
        let filled = cells.finish();
        Measured {
            frame: Frame::new([0.0; 3], [0.0, 0.0, 1.0]),
            grid,
            counts: filled.counts,
            means: filled.means,
            extents: filled.extents,
        }
    }

    fn small_parts() -> SurfaceDetectConfig {
        SurfaceDetectConfig {
            min_region_area: 0.01,
            ..SurfaceDetectConfig::default()
        }
    }

    /// The angle in degrees at every corner of a ring.
    fn angles(ring: &[[f64; 2]]) -> Vec<f64> {
        let n = ring.len();
        (0..n)
            .map(|index| {
                let (a, b, c) = (
                    ring[(index + n - 1) % n],
                    ring[index],
                    ring[(index + 1) % n],
                );
                let (p, q) = ([a[0] - b[0], a[1] - b[1]], [c[0] - b[0], c[1] - b[1]]);
                (p[0] * q[1] - p[1] * q[0])
                    .abs()
                    .atan2(p[0] * q[0] + p[1] * q[1])
                    .to_degrees()
            })
            .collect()
    }

    #[test]
    fn a_free_edge_at_seven_degrees_to_the_grid_is_one_straight_edge() {
        // A wall 3 m long whose top rises at 7 degrees from 1 m: on a grid
        // along u it is a stair of cells, eight cells to a step.
        let slope = 7f64.to_radians().tan();
        let truth = |u: f64| 1.0 + slope * u;
        let wall = scanned([-0.2, -0.2], [80, 40], 0.002, 0.0, [3.1, 1.5], |[u, v]| {
            (0.0..3.0).contains(&u) && v >= 0.0 && v < truth(u)
        });
        let outline = wall.outline(&small_parts(), &mut || Ok(())).unwrap();
        assert_eq!(outline.patches.len(), 1);
        let ring = &outline.patches[0].outer;
        assert_eq!(ring.len(), 4, "{ring:?}");
        let top: Vec<&[f64; 2]> = ring.iter().filter(|corner| corner[1] > 0.5).collect();
        assert_eq!(top.len(), 2, "{ring:?}");
        for corner in top {
            // Within a few millimetres of the true edge, at the ends of the
            // wall: the outermost points lie up to 2 mm inside it.
            let off = (corner[1] - truth(corner[0])) / (1.0 + slope * slope).sqrt();
            assert!(off.abs() < 0.003, "{corner:?}: {off}");
            assert!(corner[0].abs() < 0.003 || (corner[0] - 3.0).abs() < 0.003);
        }
        for corner in ring.iter().filter(|corner| corner[1] < 0.5) {
            assert!(corner[1].abs() < 0.003, "{corner:?}");
        }
    }

    #[test]
    fn a_door_and_a_window_in_a_wall_turned_to_the_grid_are_rectangles() {
        // A wall of 4 by 3 m with a door of 0.9 by 2.1 m and a window of
        // 1.2 by 1.2 m, turned 30 degrees to the grid it is traced on.
        let within = |[u, v]: [f64; 2], low: [f64; 2], high: [f64; 2]| {
            u >= low[0] && u < high[0] && v >= low[1] && v < high[1]
        };
        let wall = scanned([-1.7, -0.2], [118, 100], 0.01, 30.0, [4.0, 3.0], |at| {
            within(at, [0.0, 0.0], [4.0, 3.0])
                && !within(at, [0.8, -1.0], [1.7, 2.1])
                && !within(at, [2.2, 0.9], [3.4, 2.1])
        });
        let outline = wall.outline(&small_parts(), &mut || Ok(())).unwrap();
        assert_eq!(outline.patches.len(), 1);
        let patch = &outline.patches[0];
        assert_eq!(patch.outer.len(), 8, "{:?}", patch.outer);
        assert_eq!(patch.holes.len(), 1);
        assert_eq!(patch.holes[0].len(), 4, "{:?}", patch.holes[0]);
        for ring in std::iter::once(&patch.outer).chain(&patch.holes) {
            for angle in angles(ring) {
                assert!((angle - 90.0).abs() < 0.5, "{angle}: {ring:?}");
            }
        }
        // Every edge runs along the wall or across it.
        let (sin, cos) = 30f64.to_radians().sin_cos();
        for (a, b) in patch.outer.iter().zip(patch.outer.iter().cycle().skip(1)) {
            let side = [b[0] - a[0], b[1] - a[1]];
            let along = (side[0] * cos + side[1] * sin) / side[0].hypot(side[1]);
            assert!(
                along.abs() < 0.5f64.to_radians().sin() || along.abs() > 0.5f64.to_radians().cos(),
                "{a:?} {b:?}"
            );
        }
        // The window, found half a point spacing larger all round.
        let hole_area = -ring_signed_area(&patch.holes[0]);
        assert!((hole_area - 1.21 * 1.21).abs() < 0.03, "{hole_area}");
    }

    #[test]
    fn a_stair_of_cells_along_a_straight_slanted_edge_is_one_segment() {
        // Every cell whose middle lies below a line that rises one in three:
        // a stair of three cells to a step.
        let stair = measured(|x, y| {
            let (u, v) = ((f64::from(x) + 0.5) * 0.05, (f64::from(y) + 0.5) * 0.05);
            v <= 0.3 + u / 3.0
        });
        let outline = stair.outline(&small_parts(), &mut || Ok(())).unwrap();
        let ring = &outline.patches[0].outer;
        assert_eq!(ring.len(), 4, "{ring:?}");
        let top: Vec<&[f64; 2]> = ring.iter().filter(|corner| corner[1] > 0.2).collect();
        assert_eq!(top.len(), 2, "{ring:?}");
        let rise = (top[1][1] - top[0][1]) / (top[1][0] - top[0][0]);
        assert!(
            (rise.atan().to_degrees() - (1f64 / 3.0).atan().to_degrees()).abs() < 1.0,
            "{ring:?}"
        );
    }

    #[test]
    fn tally_gives_the_statistics_of_what_it_counted() {
        // 1 to 100 mm in steps of a millimetre, half of them negative, with
        // a tolerance of 30 mm and a window of 120.
        let mut tally = Tally::new();
        let mut other = Tally::new();
        for value in 1..=100i64 {
            let signed = if value % 2 == 0 { value } else { -value };
            let target = if value <= 40 { &mut tally } else { &mut other };
            target.add(signed * 1_000, 0.030, 0.120);
        }
        tally.merge(&other);
        let residuals = tally.residuals(0.120);
        assert_eq!((residuals.points, residuals.inliers), (100, 30));
        assert!((residuals.max - 0.100).abs() < 1e-12);
        assert!((residuals.mean_abs - 0.0505).abs() < 1e-12);
        assert!((residuals.mean - 0.0005).abs() < 1e-12);
        // Root of the mean of the squares of 1 to 100: 58.17 mm.
        assert!((residuals.rms - 0.058_168_7).abs() < 1e-6);
        // The 95th value is 95 mm; a class is 0.94 mm wide.
        assert!((residuals.p95 - 0.095).abs() < 0.001, "{}", residuals.p95);
        assert_eq!(Tally::new().residuals(0.120), Residuals::default());
    }
}
