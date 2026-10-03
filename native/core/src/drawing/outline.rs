//! The filled cut: from the count of points per cell of the cut plane to the
//! closed regions of material that the plane goes through.
//!
//! A scanner records surfaces only, so a wall that the slab cuts is two lines
//! of points, one per scanned face. The cells that hold enough points are
//! turned into the main direction of the building, the gap between two faces
//! is closed up to the largest wall thickness, and what remains is traced
//! into rings, reduced to straight segments and moved back onto the points.
//! A gap wider than the largest wall thickness stays open, so door and window
//! openings do.
//!
//! What the cut holds to, at the cell of 20 mm and with 2 mm of scanner
//! noise, each measured in the tests below:
//!
//! - A face scanned on both sides of its wall is drawn on its points, within
//!   a millimetre. That holds for a face that bends, however slightly, and
//!   for one that steps by 15 mm or more: where the outline of the cells
//!   shows no corner, the points along the face do.
//! - A bump or a recess that goes less than a cell and a half out of a face
//!   and returns to it, such as a pilaster of 30 mm, is not drawn: the face
//!   runs straight on. The outline is reduced by that much to pass over the
//!   steps of one cell that every straight face leaves.
//! - Squaring moves an edge onto the main direction when that moves neither
//!   of its ends, as they are drawn, by more than the squaring tolerance; an
//!   edge that is squared is that far from its points at most.
//! - A face scanned from one side is a strip of one cell, centred on its
//!   points, from the smallest wall length on.
//! - What stands apart from everything else, by more than the closing can
//!   bridge in any direction (0.84 m at the defaults), is traced in its own
//!   direction when that differs from the main one: a second building, a
//!   loose wall, a face. What joins the rest at another direction is traced
//!   along the main direction. There the square of the closing does not
//!   reach into an inside corner, which stays filled over up to the side of
//!   the square along a wall (0.5 m; measured 0.48 m at 25 degrees), and
//!   the end of such a wall is good to about 15 mm.

use std::borrow::Cow;

use super::slab::{CellMap, CutGrid, MAX_CUT_GRID_CELLS};
use super::{DrawingRequest, DrawingView, MAX_WALL_THICKNESS, MIN_CUT_GRID};
use crate::grid2d::{ring_signed_area, Connectivity, GridFrame, Mask};
use crate::LoadError;

/// A cell counts as occupied from this many points on. Fewer are stray
/// points and the thin edge of a face that runs through a corner of the cell.
pub const CUT_MIN_POINTS_PER_CELL: u32 = 3;
/// With the smallest wall thickness this gives the smallest region that is
/// drawn: one with the area of 50 mm by 0.30 m, or one that is 0.30 m long,
/// as a face scanned from one side is whatever its length.
pub const DEFAULT_MIN_WALL_LENGTH: f64 = 0.30;
/// A hole smaller than this, in square metres, is filled.
pub const MIN_CUT_HOLE_AREA: f64 = 0.05;
/// Squaring turns an edge onto the main direction only when that moves
/// neither of its ends by more than this.
pub const SQUARE_TOLERANCE: f64 = 0.03;

/// The outline of the cells is reduced to segments that stay within this
/// many cells of it: enough to pass over the steps of one cell that a
/// straight face leaves. A bend or a step of a face that stays within it is
/// found from the points instead, see `Fit::split`; a bump that returns to
/// the face within it is not drawn.
const SIMPLIFY_CELLS: f64 = 1.5;
/// A cloud too sparse for the grid gets cells of twice the size, twice at
/// most: from 20 mm to 80 mm.
pub(super) const MAX_COARSENINGS: u32 = 2;
/// The largest wall thickness is at most this many cells: what the largest
/// thickness a request may ask is at the smallest grid it may ask.
const MAX_CLOSING_CELLS: f64 = MAX_WALL_THICKNESS / MIN_CUT_GRID;
/// A part that stands apart is traced in its own direction when that is at
/// least this far from the main direction. Below it the square of the
/// closing leaves about a cell in a corner of the part, which the reduction
/// of the outline passes over.
const OWN_DIRECTION_DEGREES: f64 = 2.5;
/// The parts that stand apart are found by going through the box around
/// each; together those boxes hold at most this many times the grid.
const APART_GRID_PASSES: usize = 4;
/// A segment is looked at for a bend or a step when one line leaves its
/// points this far off, as the root of their mean squared distance. The
/// noise of a scanner and a face that lies now in one row of cells, now in
/// the next, leave a millimetre or two.
const SPLIT_RESIDUAL: f64 = 0.003;
/// Each part of a segment that is split holds at least this many cells:
/// enough to show its direction.
const SPLIT_CELLS: usize = 15;
/// A group of fewer occupied cells than this is noise.
const MIN_NOISE_CELLS: u32 = 6;
/// The main direction is found from this many cells at most.
const DIRECTION_SAMPLES: usize = 50_000;
/// An edge is moved onto its points when this many cells carry it.
const SUPPORT_CELLS: usize = 5;
/// Below this the main direction is taken as that of the model axes, so
/// that a building that follows them is drawn exactly along them.
const SMALLEST_TURN_DEGREES: f64 = 0.02;
/// A fitted corner that lands farther than this many cells from the corner
/// of the outline is not trusted: two edges that are nearly parallel meet
/// anywhere.
const CORNER_REACH_CELLS: f64 = 5.0;
/// How often the edges of a region are fitted again after those that came to
/// cross were put back on the outline of the cells.
const FIT_ATTEMPTS: usize = 6;

/// How the filled cut is traced. Lengths are in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutlineOptions {
    pub min_points_per_cell: u32,
    /// Two scanned faces at most this far apart are one wall. Rounded to
    /// whole cells, which can add up to two cells. At most
    /// `MAX_WALL_THICKNESS`, and at most 400 cells.
    pub max_wall_thickness: f64,
    pub min_wall_thickness: f64,
    pub min_wall_length: f64,
    pub min_hole_area: f64,
    /// Whether edges are turned onto the direction they are traced in,
    /// where that moves neither end of an edge by more than
    /// `square_tolerance`.
    pub square: bool,
    pub square_tolerance: f64,
    /// The main direction in degrees from the `u` axis; `None` finds it from
    /// the points, and traces what stands apart in a direction of its own.
    /// A vertical view fixes it at zero: floors are level.
    pub fixed_direction: Option<f64>,
}

impl OutlineOptions {
    pub fn for_request(request: &DrawingRequest) -> Self {
        Self {
            min_points_per_cell: CUT_MIN_POINTS_PER_CELL,
            max_wall_thickness: request.max_wall_thickness,
            min_wall_thickness: request.min_wall_thickness,
            min_wall_length: DEFAULT_MIN_WALL_LENGTH,
            min_hole_area: MIN_CUT_HOLE_AREA,
            square: request.square,
            square_tolerance: SQUARE_TOLERANCE,
            fixed_direction: (request.view != DrawingView::Plan).then_some(0.0),
        }
    }
}

/// One closed region of cut material on the cut plane, in metres: the outer
/// ring counter-clockwise and the rings of its holes clockwise, each closed
/// by itself. An enclosed room is a hole in the region of its walls.
#[derive(Debug, Clone, PartialEq)]
pub struct CutRegion {
    pub outer: Vec<[f64; 2]>,
    pub holes: Vec<Vec<[f64; 2]>>,
}

impl CutRegion {
    /// Area inside the outer ring less the holes.
    pub fn area(&self) -> f64 {
        ring_signed_area(&self.outer).abs()
            - self
                .holes
                .iter()
                .map(|hole| ring_signed_area(hole).abs())
                .sum::<f64>()
    }

    /// Vertices of all rings together.
    pub fn vertices(&self) -> usize {
        self.outer.len() + self.holes.iter().map(Vec::len).sum::<usize>()
    }
}

/// The filled cut of one slab.
#[derive(Debug, Clone, PartialEq)]
pub struct CutOutline {
    pub regions: Vec<CutRegion>,
    /// The main direction that was found or given, in degrees from the `u`
    /// axis, between -45 and 45. A part that stands apart from the rest may
    /// have been traced in a direction of its own.
    pub direction_degrees: f64,
    /// The cell that was used: larger than the grid of the slab when the
    /// cloud was too sparse for it.
    pub cell: f64,
    /// Regions left out because they are smaller than the smallest wall.
    pub dropped: usize,
    /// Regions whose edges could not be moved onto the points without the
    /// rings crossing; they keep the outline of their cells.
    pub unfitted: usize,
}

/// Trace the regions of cut material from the points per cell of a slab.
///
/// `proceed` is asked between the steps and while outlines are reduced; an
/// error from it, such as `LoadError::Cancelled`, stops the work.
pub fn trace_cut_regions(
    grid: &CutGrid,
    options: &OutlineOptions,
    proceed: &mut dyn FnMut() -> Result<(), LoadError>,
) -> Result<CutOutline, LoadError> {
    let positive = |value: f64| value.is_finite() && value > 0.0;
    if !positive(options.max_wall_thickness)
        || options.max_wall_thickness > MAX_WALL_THICKNESS
        || !positive(options.min_wall_thickness)
        || !positive(options.min_wall_length)
        || !(options.min_hole_area.is_finite() && options.min_hole_area >= 0.0)
        || !(options.square_tolerance.is_finite() && options.square_tolerance >= 0.0)
        || options
            .fixed_direction
            .is_some_and(|degrees| !degrees.is_finite())
    {
        return Err(LoadError::InvalidData(
            "the sizes of a filled cut must be above zero and its largest wall thickness at most 2 m"
                .into(),
        ));
    }
    // Too few points per cell for the threshold: larger cells.
    let mut grid = Cow::Borrowed(grid);
    let mut coarsenings = 0;
    while coarsenings < MAX_COARSENINGS && too_sparse(&grid, options.min_points_per_cell) {
        proceed()?;
        grid = Cow::Owned(grid.coarsened());
        coarsenings += 1;
    }
    let cell = grid.frame().cell;
    // The closing works on a grid that is wider by the gap it closes, on
    // every side: the gap counted in cells bounds what it costs.
    if options.max_wall_thickness / cell > MAX_CLOSING_CELLS {
        return Err(LoadError::InvalidData(
            "the grid of a filled cut is too fine for its largest wall thickness".into(),
        ));
    }
    let mut outline = CutOutline {
        regions: Vec::new(),
        direction_degrees: options.fixed_direction.map_or(0.0, quarter_turn),
        cell,
        dropped: 0,
        unfitted: 0,
    };

    proceed()?;
    let mut occupied = grid.counts().threshold(options.min_points_per_cell.max(1));
    remove_noise(&mut occupied, options.min_wall_length * 0.5);
    let cells = occupied.count();
    if cells == 0 {
        return Ok(outline);
    }
    let occupied = Occupied {
        grid: &grid,
        mask: occupied,
    };

    // Everything from here works around the middle of the occupied cells,
    // so that a building at national grid coordinates keeps its millimetres.
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    occupied.for_each(|at, _| {
        for axis in 0..2 {
            low[axis] = low[axis].min(at[axis]);
            high[axis] = high[axis].max(at[axis]);
        }
    });
    let pivot = [(low[0] + high[0]) * 0.5, (low[1] + high[1]) * 0.5];
    proceed()?;
    let degrees = match options.fixed_direction {
        Some(degrees) => quarter_turn(degrees),
        None => {
            let found =
                occupied.direction(cells, pivot, None, &|visit| occupied.for_each_cell(visit));
            found
                .filter(|found| found.abs() >= SMALLEST_TURN_DEGREES)
                .unwrap_or(0.0)
        }
    };
    outline.direction_degrees = degrees;

    // What stands apart from the rest in a direction of its own is traced
    // in that direction. A vertical view has one direction: floors are
    // level.
    let apart = match options.fixed_direction {
        Some(_) => Apart::default(),
        None => {
            proceed()?;
            Apart::find(&occupied, options, degrees, proceed)?
        }
    };
    trace_direction(
        &|visit| {
            occupied.for_each_cell(|x, y, at, count| {
                if !apart.holds(occupied.grid.frame().index(x, y)) {
                    visit(at, count);
                }
            })
        },
        &Turn::new(pivot, degrees),
        cell,
        options,
        proceed,
        &mut outline,
    )?;
    for part in &apart.parts {
        trace_direction(
            &|visit| {
                occupied.for_each_within(part.around, |x, y, at, count| {
                    if apart.labels[occupied.grid.frame().index(x, y)] == part.label {
                        visit(at, count);
                    }
                })
            },
            &Turn::new(part.pivot, part.degrees),
            cell,
            options,
            proceed,
            &mut outline,
        )?;
    }
    Ok(outline)
}

/// Goes through occupied cells: each as the mean position of its points and
/// their number.
type Cells<'a> = dyn Fn(&mut dyn FnMut([f64; 2], u32)) + 'a;

/// Takes an occupied cell of the count grid: its column and row, the mean
/// position of its points and their number.
type CellVisit<'a> = dyn FnMut(u32, u32, [f64; 2], u32) + 'a;

/// Trace the regions of the cells that share one direction, and add them to
/// the outline.
fn trace_direction(
    cells: &Cells<'_>,
    turn: &Turn,
    cell: f64,
    options: &OutlineOptions,
    proceed: &mut dyn FnMut() -> Result<(), LoadError>,
    outline: &mut CutOutline,
) -> Result<(), LoadError> {
    // The occupied cells, each as the mean position of its points, on a
    // grid that follows the direction. There a wall face is a straight row
    // of cells, and closing with a square keeps the corners of a room
    // square. Walls at another direction that are part of the same cells
    // keep a filled triangle in every inside corner: the square does not
    // fit into it.
    proceed()?;
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    cells(&mut |at, _| {
        let at = turn.local(at);
        for axis in 0..2 {
            low[axis] = low[axis].min(at[axis]);
            high[axis] = high[axis].max(at[axis]);
        }
    });
    if low[0] > high[0] {
        return Ok(());
    }
    // Turned, the same cells can take up to twice the area.
    let frame = GridFrame::covering(low, high, cell, 2 * MAX_CUT_GRID_CELLS)?;
    outline.cell = outline.cell.max(frame.cell);
    let mut real = Mask::new(frame);
    cells(&mut |at, _| {
        if let Some([x, y]) = frame.cell_of(turn.local(at)) {
            real.set(x, y, true);
        }
    });

    proceed()?;
    let mut solid = real.clone();
    let gap = frame.cells_for_length(options.max_wall_thickness).max(1);
    // A square wider than the grid closes nothing more.
    solid.close(
        gap.div_ceil(2)
            .min(frame.width.max(frame.height).div_ceil(2)),
    );
    proceed()?;
    outline.dropped += remove_small_regions(
        &mut solid,
        frame.cells_for_area(options.min_wall_thickness * options.min_wall_length),
        options.min_wall_length,
    );
    solid.fill_small_holes(
        frame.cells_for_area(options.min_hole_area),
        Connectivity::Eight,
    );
    proceed()?;
    let traced = solid.trace(Connectivity::Eight);

    // Reduce every ring to straight segments, and note for each segment the
    // cells that carry it.
    let mut rings: Vec<Vec<TracedRing>> = Vec::with_capacity(traced.len());
    let mut means: CellMap<Mean> = CellMap::default();
    // Segments are numbered from one as they come, over all rings.
    let mut segments = 0u32;
    for region in &traced {
        let kept = region
            .to_plane(&frame)
            .kept_corners(SIMPLIFY_CELLS * frame.cell, proceed)?;
        let mut reduced = Vec::with_capacity(kept.len());
        for (number, (ring, kept)) in std::iter::once(&region.outer)
            .chain(&region.holes)
            .zip(&kept)
            .enumerate()
        {
            // A hole that is left without an area goes; a region whose
            // outer ring is goes as a whole.
            if kept.len() < 3 {
                if number == 0 {
                    break;
                }
                continue;
            }
            let mut ring = TracedRing::new(ring, kept, &frame, &real, &solid);
            let count = ring.segments.len() as u32;
            for (position, segment) in ring.segments.iter_mut().enumerate() {
                let position = position as u32;
                segment.number = segments + 1 + position;
                segment.beside = [
                    segments + 1 + (position + count - 1) % count,
                    segments + 1 + (position + 1) % count,
                ];
                for cell in &segment.supports {
                    means.entry(*cell).or_default().carry(segment.number);
                }
            }
            segments += count;
            reduced.push(ring);
        }
        if !reduced.is_empty() {
            rings.push(reduced);
        }
    }
    drop(traced);
    // The mean position of the points in those cells only: they are a thin
    // band along the outlines, also where the cut goes through a whole floor.
    proceed()?;
    cells(&mut |at, count| {
        let at = turn.local(at);
        if let Some([x, y]) = frame.cell_of(at) {
            if let Some(mean) = means.get_mut(&(frame.index(x, y) as u64)) {
                let weight = count as f64;
                mean.sum[0] += at[0] * weight;
                mean.sum[1] += at[1] * weight;
                mean.weight += weight;
            }
        }
    });

    proceed()?;
    for reduced in &rings {
        // Where moved edges come to cross, the segments they stand for go
        // back to the outline of the cells and the rest is fitted again.
        let mut plain: Vec<Vec<bool>> = reduced
            .iter()
            .map(|ring| vec![false; ring.segments.len()])
            .collect();
        let mut fitted = None;
        for _ in 0..FIT_ATTEMPTS {
            let attempt: Vec<Option<FittedRing>> = reduced
                .iter()
                .zip(&plain)
                .map(|(ring, plain)| ring.fitted(&means, options, frame.cell, plain))
                .collect();
            // The segments to put back, each run as its ring, its first
            // segment and their number. A ring that the fit left without
            // its area goes back as a whole.
            let mut back: Vec<(usize, (usize, usize))> = Vec::new();
            for (number, (ring, fit)) in reduced.iter().zip(&attempt).enumerate() {
                if !fit.as_ref().is_some_and(|fit| ring.agrees(&fit.ring)) {
                    back.push((number, (0, ring.segments.len())));
                }
            }
            if back.is_empty() {
                let attempt: Vec<FittedRing> = attempt.into_iter().flatten().collect();
                let corners: Vec<&[[f64; 2]]> = attempt.iter().map(|fit| &fit.ring[..]).collect();
                for (ring, vertex) in crossing_edges(&corners) {
                    back.extend(attempt[ring].owners[vertex].map(|segments| (ring, segments)));
                }
                if back.is_empty() {
                    fitted = Some(attempt.into_iter().map(|fit| fit.ring).collect());
                    break;
                }
            }
            let mut changed = false;
            for (ring, (first, count)) in back {
                let segments = plain[ring].len();
                for step in 0..count {
                    let plain = &mut plain[ring][(first + step) % segments];
                    changed |= !*plain;
                    *plain = true;
                }
            }
            if !changed {
                break;
            }
        }
        let local: Vec<Vec<[f64; 2]>> = fitted.unwrap_or_else(|| {
            outline.unfitted += 1;
            reduced.iter().map(|ring| ring.corners.clone()).collect()
        });
        let mut plane = local.into_iter().map(|ring| {
            ring.into_iter()
                .map(|at| turn.plane(at))
                .collect::<Vec<_>>()
        });
        if let Some(outer) = plane.next() {
            outline.regions.push(CutRegion {
                outer,
                holes: plane.collect(),
            });
        }
    }
    Ok(())
}

/// The parts of the cut that stand apart from everything else and run in a
/// direction of their own: a building beside the main one, a loose wall, a
/// face scanned from one side. On the grid of the main direction the square
/// of the closing cannot reach into their corners, and their faces are
/// stairs of cells; each is traced on a grid of its own direction instead.
#[derive(Default)]
struct Apart {
    /// Per cell of the count grid the number of its part, counted from one;
    /// empty when nothing stands apart.
    labels: Vec<u32>,
    /// Per part whether it is traced in its own direction.
    own: Vec<bool>,
    parts: Vec<Part>,
}

/// One part that is traced in its own direction.
struct Part {
    label: u32,
    /// The box around its cells: lowest and highest column and row.
    around: [u32; 4],
    pivot: [f64; 2],
    degrees: f64,
}

impl Apart {
    /// Whether a cell of the count grid belongs to a part that is traced in
    /// its own direction.
    fn holds(&self, index: usize) -> bool {
        self.labels
            .get(index)
            .is_some_and(|label| *label > 0 && self.own[*label as usize - 1])
    }

    fn find(
        occupied: &Occupied<'_>,
        options: &OutlineOptions,
        main: f64,
        proceed: &mut dyn FnMut() -> Result<(), LoadError>,
    ) -> Result<Self, LoadError> {
        let frame = occupied.grid.frame();
        // Parts farther apart than the diagonal of the square of the
        // closing are joined by a closing in no direction. Widening every
        // cell by half of that and a cell joins all that lie nearer.
        let gap = frame.cells_for_length(options.max_wall_thickness).max(1);
        let side = 2 * gap.div_ceil(2) + 1;
        let reach = (side as f64 * std::f64::consts::FRAC_1_SQRT_2).ceil() as u32 + 1;
        let mut linked = occupied.mask.clone();
        linked.dilate(reach, reach);
        let groups = linked.components(Connectivity::Eight);
        drop(linked);
        if groups.count() < 2 {
            return Ok(Self::default());
        }
        // Per part its occupied cells and the box around them.
        let mut cells = vec![0usize; groups.count()];
        let mut boxes = vec![[u32::MAX, u32::MAX, 0, 0]; groups.count()];
        for y in 0..frame.height {
            for x in 0..frame.width {
                let index = frame.index(x, y);
                if occupied.mask.cells()[index] {
                    let part = groups.labels[index] as usize - 1;
                    cells[part] += 1;
                    let around = &mut boxes[part];
                    *around = [
                        around[0].min(x),
                        around[1].min(y),
                        around[2].max(x),
                        around[3].max(y),
                    ];
                }
            }
        }
        let mut apart = Self {
            own: vec![false; groups.count()],
            labels: groups.labels,
            parts: Vec::new(),
        };
        // Finding the cells of a part goes through its box. Boxes can lie
        // inside each other, so the cells gone through are counted, and
        // what is left when they pass a few times the grid stays with the
        // main direction.
        let mut budget = APART_GRID_PASSES * frame.cells();
        for (part, (cells, around)) in cells.iter().zip(&boxes).enumerate() {
            if *cells == 0 {
                continue;
            }
            let inside =
                (around[2] - around[0] + 1) as usize * (around[3] - around[1] + 1) as usize;
            if inside > budget {
                break;
            }
            budget -= inside;
            proceed()?;
            let label = part as u32 + 1;
            let pivot = [
                frame.origin[0] + (around[0] + around[2] + 1) as f64 * 0.5 * frame.cell,
                frame.origin[1] + (around[1] + around[3] + 1) as f64 * 0.5 * frame.cell,
            ];
            let own = occupied.direction(
                *cells,
                pivot,
                Some((main, OWN_DIRECTION_DEGREES)),
                &|visit| {
                    occupied.for_each_within(*around, |x, y, at, count| {
                        if apart.labels[frame.index(x, y)] == label {
                            visit(x, y, at, count);
                        }
                    })
                },
            );
            if let Some(degrees) =
                own.filter(|own| quarter_turn(own - main).abs() >= OWN_DIRECTION_DEGREES)
            {
                apart.own[part] = true;
                apart.parts.push(Part {
                    label,
                    around: *around,
                    pivot,
                    degrees,
                });
            }
        }
        // Without such a part the labels have served.
        if apart.parts.is_empty() {
            return Ok(Self::default());
        }
        Ok(apart)
    }
}

/// Empty the groups that are smaller than the smallest wall: fewer cells than
/// its area and shorter than its length. A face scanned from one side is a
/// row of single cells whatever the wall behind it, so its length counts and
/// not the area of its strip. Returns how many groups went.
fn remove_small_regions(mask: &mut Mask, min_cells: usize, min_length: f64) -> usize {
    let frame = mask.frame();
    let groups = mask.components(Connectivity::Eight);
    // Per group the box around its cells: lowest and highest column and row.
    let mut boxes = vec![[u32::MAX, u32::MAX, 0, 0]; groups.count()];
    for y in 0..frame.height {
        for x in 0..frame.width {
            let label = groups.labels[frame.index(x, y)];
            if label > 0 {
                let around = &mut boxes[label as usize - 1];
                *around = [
                    around[0].min(x),
                    around[1].min(y),
                    around[2].max(x),
                    around[3].max(y),
                ];
            }
        }
    }
    let small: Vec<bool> = groups
        .sizes
        .iter()
        .zip(&boxes)
        .map(|(cells, around)| {
            let width = (around[2] - around[0] + 1) as f64;
            let height = (around[3] - around[1] + 1) as f64;
            (*cells as usize) < min_cells && width.hypot(height) * frame.cell < min_length
        })
        .collect();
    for y in 0..frame.height {
        for x in 0..frame.width {
            let label = groups.labels[frame.index(x, y)];
            if label > 0 && small[label as usize - 1] {
                mask.set(x, y, false);
            }
        }
    }
    small.iter().filter(|small| **small).count()
}

/// A direction in degrees brought between -45 and 45: a building has no
/// front for this purpose, only two directions a quarter turn apart.
fn quarter_turn(degrees: f64) -> f64 {
    let turned = degrees.rem_euclid(90.0);
    if turned > 45.0 {
        turned - 90.0
    } else {
        turned
    }
}

/// Whether the cells are too small for the cloud: the cells that reach the
/// threshold hold less than half of the points. Counted in points and not in
/// cells, because a furnished room leaves stray points in more cells than its
/// walls fill, and those must not make a dense scan count as sparse.
fn too_sparse(grid: &CutGrid, min_points: u32) -> bool {
    let (mut all, mut enough) = (0u64, 0u64);
    for count in grid.counts().counts() {
        all += *count as u64;
        if *count >= min_points {
            enough += *count as u64;
        }
    }
    2 * enough < all
}

/// The cells that hold enough points, with the count grid they come from.
struct Occupied<'a> {
    grid: &'a CutGrid,
    mask: Mask,
}

impl Occupied<'_> {
    /// Every occupied cell as the mean position of its points and their
    /// number, row by row.
    fn for_each(&self, mut visit: impl FnMut([f64; 2], u32)) {
        self.for_each_cell(|_, _, at, count| visit(at, count));
    }

    /// The same, with the column and row of the cell.
    fn for_each_cell(&self, visit: impl FnMut(u32, u32, [f64; 2], u32)) {
        let frame = self.grid.frame();
        self.for_each_within([0, 0, frame.width - 1, frame.height - 1], visit);
    }

    /// The same for the cells in a box: from its lowest column and row up to
    /// and including its highest.
    fn for_each_within(&self, around: [u32; 4], mut visit: impl FnMut(u32, u32, [f64; 2], u32)) {
        let frame = self.grid.frame();
        let cells = self.mask.cells();
        for y in around[1]..=around[3] {
            for x in around[0]..=around[2] {
                if cells[frame.index(x, y)] {
                    if let Some(at) = self.grid.centroid(x, y) {
                        visit(x, y, at, self.grid.counts().count(x, y));
                    }
                }
            }
        }
    }

    /// The direction of `cells` occupied cells that `each` goes through,
    /// measured about `pivot`, see `main_direction`.
    fn direction(
        &self,
        cells: usize,
        pivot: [f64; 2],
        apart_from: Option<(f64, f64)>,
        each: &dyn Fn(&mut CellVisit<'_>),
    ) -> Option<f64> {
        let stride = cells.div_ceil(DIRECTION_SAMPLES).max(1);
        let mut samples = Vec::with_capacity(cells / stride + 1);
        let mut number = 0usize;
        let around = self.grid.counts();
        let frame = around.frame();
        each(&mut |x, y, at, count| {
            if number.is_multiple_of(stride) {
                // A face on the border between two rows of cells fills
                // both, and would count twice for its length. Its share of
                // the points around it counts instead.
                let mut near = 0u64;
                for row in y.saturating_sub(1)..=(y + 1).min(frame.height - 1) {
                    for column in x.saturating_sub(1)..=(x + 1).min(frame.width - 1) {
                        near += around.count(column, row) as u64;
                    }
                }
                let weight = count as f64 / near.max(1) as f64;
                samples.push([at[0] - pivot[0], at[1] - pivot[1], weight]);
            }
            number += 1;
        });
        main_direction(&samples, frame.cell, apart_from)
    }
}

/// Empty the cells that belong to no surface: groups of a few cells, and
/// groups that are short in every direction, such as the leg of a chair or
/// a cable. They go before the closing, which would otherwise tie each of
/// them to the nearest wall.
///
/// Cells up to two empty cells apart count as one group: a face of a sparse
/// scan is a dotted line.
fn remove_noise(mask: &mut Mask, min_extent: f64) {
    let frame = mask.frame();
    let mut linked = mask.clone();
    linked.dilate(1, 1);
    let groups = linked.components(Connectivity::Eight);
    drop(linked);
    let mut cells = vec![0u32; groups.count()];
    // Per group the box around its cells: lowest and highest column and row.
    let mut boxes = vec![[u32::MAX, u32::MAX, 0, 0]; groups.count()];
    for y in 0..frame.height {
        for x in 0..frame.width {
            let index = frame.index(x, y);
            if mask.cells()[index] {
                let group = groups.labels[index] as usize - 1;
                cells[group] += 1;
                let around = &mut boxes[group];
                *around = [
                    around[0].min(x),
                    around[1].min(y),
                    around[2].max(x),
                    around[3].max(y),
                ];
            }
        }
    }
    let noise: Vec<bool> = cells
        .iter()
        .zip(&boxes)
        .map(|(cells, around)| {
            let width = (around[2].saturating_sub(around[0]) + 1) as f64;
            let height = (around[3].saturating_sub(around[1]) + 1) as f64;
            *cells < MIN_NOISE_CELLS || width.hypot(height) * frame.cell < min_extent
        })
        .collect();
    for y in 0..frame.height {
        for x in 0..frame.width {
            let index = frame.index(x, y);
            if mask.cells()[index] && noise[groups.labels[index] as usize - 1] {
                mask.set(x, y, false);
            }
        }
    }
}

/// The direction most faces run along or across, in degrees between -45 and
/// 45. Every sample is a position and its weight. For every direction tried,
/// the weights are counted along both axes of that direction; faces that
/// follow an axis pile up in few bins, and the sum of the squared counts is
/// largest.
///
/// With `apart_from`, a direction and a number of degrees, the search ends
/// with nothing as soon as it is known to end nearer to that direction than
/// that: its first stage, which is most of the work, tells to half a degree.
fn main_direction(samples: &[[f64; 3]], cell: f64, apart_from: Option<(f64, f64)>) -> Option<f64> {
    let reach = samples
        .iter()
        .map(|at| at[0].hypot(at[1]))
        .fold(0.0, f64::max);
    let score = |degrees: f64, bin: f64| -> f64 {
        let (sin, cos) = degrees.to_radians().sin_cos();
        // Counted in quarters of a bin and then spread over a whole one to
        // either side: the score of a face must not depend on where it
        // happens to lie between two bins.
        let fine = bin / 4.0;
        let bins = (2.0 * reach / fine) as usize + 3;
        let mut along = vec![0.0f64; bins];
        let mut across = vec![0.0f64; bins];
        for at in samples {
            for (histogram, position) in [
                (&mut along, cos * at[0] + sin * at[1]),
                (&mut across, cos * at[1] - sin * at[0]),
            ] {
                let place = (position + reach) / fine;
                let first = place.floor();
                let share = place - first;
                histogram[first as usize] += (1.0 - share) * at[2];
                histogram[first as usize + 1] += share * at[2];
            }
        }
        [along, across]
            .into_iter()
            .map(|histogram| {
                let spread = running_sum(&running_sum(&histogram, 4), 4);
                spread.iter().map(|count| count * count).sum::<f64>()
            })
            .sum()
    };
    // From half degrees down to five thousandths, with bins that narrow
    // along: over 5 m a bin of one cell cannot tell directions a fifth of a
    // degree apart.
    let mut best = 0.0;
    for (stage, (from, steps, step, bin)) in [
        (0.0, 180, 0.5, cell),
        (-0.5, 20, 0.05, cell / 4.0),
        (-0.05, 20, 0.005, cell / 16.0),
    ]
    .into_iter()
    .enumerate()
    {
        let start = best + from;
        let mut highest = f64::NEG_INFINITY;
        for index in 0..=steps {
            let degrees = start + index as f64 * step;
            let score = score(degrees, bin);
            if score > highest {
                highest = score;
                best = degrees;
            }
        }
        if let (0, Some((known, least))) = (stage, apart_from) {
            if quarter_turn(best - known).abs() < least - 0.5 {
                return None;
            }
        }
    }
    Some(quarter_turn(best))
}

/// Every value with its `width - 1` followers added.
fn running_sum(values: &[f64], width: usize) -> Vec<f64> {
    let mut sums = Vec::with_capacity(values.len() + width);
    let mut sum = 0.0;
    for index in 0..values.len() + width - 1 {
        if index < values.len() {
            sum += values[index];
        }
        if index >= width {
            sum -= values[index - width];
        }
        sums.push(sum);
    }
    sums
}

/// The turn between the cut plane and the grid that follows the main
/// direction, about the middle of the occupied cells.
struct Turn {
    pivot: [f64; 2],
    sin: f64,
    cos: f64,
}

impl Turn {
    fn new(pivot: [f64; 2], degrees: f64) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Self { pivot, sin, cos }
    }

    /// A position on the cut plane in the turned grid.
    fn local(&self, uv: [f64; 2]) -> [f64; 2] {
        let (u, v) = (uv[0] - self.pivot[0], uv[1] - self.pivot[1]);
        [self.cos * u + self.sin * v, self.cos * v - self.sin * u]
    }

    /// A position of the turned grid on the cut plane.
    fn plane(&self, at: [f64; 2]) -> [f64; 2] {
        [
            self.pivot[0] + self.cos * at[0] - self.sin * at[1],
            self.pivot[1] + self.sin * at[0] + self.cos * at[1],
        ]
    }
}

/// Sum of the positions of the points in a cell, for their mean, and the
/// segments the cell carries.
#[derive(Debug, Clone, Copy, Default)]
struct Mean {
    sum: [f64; 2],
    weight: f64,
    /// The numbers of the first two segments; zero is none.
    segments: [u32; 2],
    /// Whether it carries more than two.
    more: bool,
}

impl Mean {
    fn carry(&mut self, segment: u32) {
        if self.segments.contains(&segment) {
            return;
        }
        match self.segments.iter_mut().find(|slot| **slot == 0) {
            Some(slot) => *slot = segment,
            None => self.more = true,
        }
    }
}

/// One straight segment of a reduced ring, with the cells of the outline it
/// replaces.
struct Segment {
    from: [f64; 2],
    to: [f64; 2],
    /// Cells with points that lie along the stretch of outline, at most one
    /// cell behind it.
    supports: Vec<u64>,
    /// Cell edges of the stretch, and how many of them lie in front of a
    /// strip that is at most two cells thick.
    edges: u32,
    thin: u32,
    /// Its number among all segments, and those of the segments before and
    /// after it in its ring.
    number: u32,
    beside: [u32; 2],
}

/// A ring of a traced region after reduction: its corners and segments in
/// the turned grid.
struct TracedRing {
    corners: Vec<[f64; 2]>,
    /// Segment `n` runs from corner `n` to the next one.
    segments: Vec<Segment>,
}

impl TracedRing {
    /// `ring` holds the corners of the outline in cells, with the occupied
    /// cells on its left; `kept` the positions of those that stay.
    fn new(
        ring: &[[i32; 2]],
        kept: &[usize],
        frame: &GridFrame,
        real: &Mask,
        solid: &Mask,
    ) -> Self {
        let corners: Vec<[f64; 2]> = kept
            .iter()
            .map(|index| frame.vertex(ring[*index]))
            .collect();
        let n = ring.len();
        let mut segments = Vec::with_capacity(kept.len());
        for (position, first) in kept.iter().enumerate() {
            let last = kept[(position + 1) % kept.len()];
            // A ring of one segment goes all the way round.
            let steps = match (last + n - first) % n {
                0 => n,
                steps => steps,
            };
            let mut segment = Segment {
                from: corners[position],
                to: corners[(position + 1) % kept.len()],
                supports: Vec::new(),
                edges: 0,
                thin: 0,
                number: 0,
                beside: [0; 2],
            };
            for step in 0..steps {
                let (a, b) = (ring[(first + step) % n], ring[(first + step + 1) % n]);
                let heading = [(b[0] - a[0]).signum() as i64, (b[1] - a[1]).signum() as i64];
                // The cell to the left of an edge that leaves a corner, seen
                // from that corner, and the direction further into the
                // region.
                let (left, inward) = match heading {
                    [1, 0] => ([0, 0], [0, 1]),
                    [0, 1] => ([-1, 0], [-1, 0]),
                    [-1, 0] => ([-1, -1], [0, -1]),
                    _ => ([0, -1], [1, 0]),
                };
                let length = ((b[0] - a[0]).abs() + (b[1] - a[1]).abs()) as i64;
                for along in 0..length {
                    let x = a[0] as i64 + along * heading[0] + left[0];
                    let y = a[1] as i64 + along * heading[1] + left[1];
                    let behind = |depth: i64| (x + depth * inward[0], y + depth * inward[1]);
                    let is = |mask: &Mask, at: (i64, i64)| mask.get(at.0, at.1);
                    segment.edges += 1;
                    if !is(solid, behind(1)) || !is(solid, behind(2)) {
                        segment.thin += 1;
                    }
                    // A face that lies on the border between two rows of
                    // cells has its points now in the one, now in the other.
                    if let Some(at) = [behind(0), behind(1)]
                        .into_iter()
                        .find(|at| is(solid, *at) && is(real, *at))
                    {
                        let cell = frame.index(at.0 as u32, at.1 as u32) as u64;
                        if segment.supports.last() != Some(&cell) {
                            segment.supports.push(cell);
                        }
                    }
                }
            }
            segments.push(segment);
        }
        Self { corners, segments }
    }

    /// The ring with its edges moved onto the points, or nothing when that
    /// leaves no ring. A segment that is `plain` stays on the outline of the
    /// cells.
    fn fitted(
        &self,
        means: &CellMap<Mean>,
        options: &OutlineOptions,
        cell: f64,
        plain: &[bool],
    ) -> Option<FittedRing> {
        if self.segments.len() < 3 {
            return None;
        }
        let mut pieces = self.pieces(means, cell, plain)?;
        // First every edge as its points lie.
        let mut lines: Vec<Line> = pieces
            .iter()
            .map(|piece| piece.fit.drawn(piece.fit.line, cell))
            .collect();
        for position in 0..lines.len() {
            if let Some((line, _)) = joined_line(position, &lines, &pieces, cell) {
                lines[position] = line;
            }
        }
        let Some((mut edges, mut joints)) = joined(&lines, &pieces, cell) else {
            return self.strip(&pieces, options, cell);
        };
        if options.square {
            // Squaring looks at an edge as it is drawn, from the corner it
            // leaves to the corner it arrives at, and not at the segments
            // the outline happened to be cut into: pieces of a face that is
            // slightly off, each short enough to be squared by itself, would
            // make a stair of it. Those corners are where the edges begin
            // and end from here on: two edges that are both squared step
            // from the one to the other where the faces met.
            for (position, edge) in edges.iter().enumerate() {
                let before = &edges[(position + edges.len() - 1) % edges.len()];
                let last = (before.pieces.0 + before.pieces.1 - 1) % pieces.len();
                pieces[last].to = joints[position][0];
                pieces[edge.pieces.0].from = joints[position][1];
            }
            for (position, edge) in edges.iter().enumerate() {
                let ends = [joints[position][1], joints[(position + 1) % edges.len()][0]];
                let Some((axis, level)) = square_level(edge, ends, &pieces, options) else {
                    continue;
                };
                let forward = pieces[edge.lead].fit.line.direction;
                for step in 0..edge.pieces.1 {
                    let at = (edge.pieces.0 + step) % pieces.len();
                    let squared = pieces[at].squared(axis, level, forward);
                    lines[at] = pieces[at].fit.drawn(squared, cell);
                }
            }
            // A short edge follows the edge it joins, as that is drawn now:
            // the end of a wall is squared when the wall is. One that joins
            // no measured edge is squared by itself.
            let short = |position: usize| {
                let fit = &pieces[position].fit;
                !fit.line.measured && !fit.plain
            };
            for position in (0..lines.len()).filter(|position| short(*position)) {
                lines[position] = pieces[position].fit.drawn(pieces[position].fit.line, cell);
            }
            let drawn = lines.clone();
            for position in (0..lines.len()).filter(|position| short(*position)) {
                let alone = pieces[position]
                    .squared_alone(options.square_tolerance)
                    .map(|line| pieces[position].fit.drawn(line, cell));
                let beside = [
                    (position + lines.len() - 1) % lines.len(),
                    (position + 1) % lines.len(),
                ];
                match (joined_line(position, &drawn, &pieces, cell), alone) {
                    (Some((line, false)), Some(alone))
                        if dot(alone.direction, line.direction) > 1.0 - 1e-12 =>
                    {
                        lines[position] = alone;
                    }
                    (Some((line, _)), _) => lines[position] = line,
                    (None, Some(alone)) if beside.iter().all(|at| !drawn[*at].measured) => {
                        lines[position] = alone;
                    }
                    (None, _) => {}
                }
            }
            (edges, joints) = joined(&lines, &pieces, cell)?;
        }
        // The corners, each with the segments of the edge that leaves it.
        // The step between two edges that do not meet belongs to both.
        let owner = |edge: &Edge| -> (usize, usize) {
            let first = pieces[edge.pieces.0].segment;
            let last = pieces[(edge.pieces.0 + edge.pieces.1 - 1) % pieces.len()].segment;
            let count = self.segments.len();
            // An edge that goes all the way round ends in the segment it
            // began in.
            if edge.pieces.1 >= pieces.len() {
                (first, count)
            } else {
                (first, (last + count - first) % count + 1)
            }
        };
        let mut fitted = FittedRing {
            ring: Vec::with_capacity(edges.len() + 4),
            owners: Vec::with_capacity(edges.len() + 4),
        };
        for (position, [arrive, leave]) in joints.into_iter().enumerate() {
            let before = owner(&edges[(position + edges.len() - 1) % edges.len()]);
            let after = owner(&edges[position]);
            if distance(arrive, leave) >= 1e-9 {
                fitted.ring.push(arrive);
                fitted.owners.push([before, after]);
            }
            fitted.ring.push(leave);
            fitted.owners.push([after, after]);
        }
        (fitted.ring.len() >= 3).then_some(fitted)
    }

    /// The stretches of the ring that are each drawn on one line: its
    /// segments, and both parts of a segment whose points lie along two
    /// lines.
    fn pieces(&self, means: &CellMap<Mean>, cell: f64, plain: &[bool]) -> Option<Vec<Piece>> {
        let mut pieces = Vec::with_capacity(self.segments.len());
        for (position, segment) in self.segments.iter().enumerate() {
            let fit = segment.fit(means, cell, plain[position])?;
            let whole = |fit: Fit| Piece {
                segment: position,
                from: segment.from,
                to: segment.to,
                fit,
            };
            match fit.split(segment.from, segment.to, cell) {
                Some((first, second, at)) => {
                    pieces.push(Piece {
                        to: at,
                        ..whole(first)
                    });
                    pieces.push(Piece {
                        from: at,
                        ..whole(second)
                    });
                }
                None => pieces.push(whole(fit)),
            }
        }
        Some(pieces)
    }

    /// The ring of a face scanned from one side, when the reduction of its
    /// outline left no ring to fit: a strip of two rows of cells, as a face
    /// on the border between two rows fills, is reduced to a sliver whose
    /// sides all lie along the same points. It is drawn as a strip of one
    /// cell around the line through them, as long as its outline.
    fn strip(&self, pieces: &[Piece], options: &OutlineOptions, cell: f64) -> Option<FittedRing> {
        let (edges, thin) = self.segments.iter().fold((0, 0), |(edges, thin), segment| {
            (edges + segment.edges, thin + segment.thin)
        });
        // An outer ring, nearly all of it in front of a thin strip: only
        // the cells at its two ends are not.
        if ring_signed_area(&self.corners) <= 0.0 || 10 * thin < 8 * edges {
            return None;
        }
        let points: Vec<[f64; 2]> = pieces
            .iter()
            .flat_map(|piece| &piece.fit.points)
            .copied()
            .collect();
        let longest = pieces
            .iter()
            .max_by(|a, b| distance(a.from, a.to).total_cmp(&distance(b.from, b.to)))?;
        if points.len() < SUPPORT_CELLS {
            return None;
        }
        let mut line = line_through(&points, longest.fit.line.direction, cell);
        if !line.measured {
            return None;
        }
        // How far the outline reaches along the line.
        let reach = |line: &Line| {
            self.corners
                .iter()
                .map(|at| dot(minus(*at, line.point), line.direction))
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), along| {
                    (low.min(along), high.max(along))
                })
        };
        if options.square {
            let axis = level_axis(line.direction);
            let mut levels: Vec<f64> = points.iter().map(|at| at[axis]).collect();
            let level = median(&mut levels);
            let (low, high) = reach(&line);
            let moved =
                |along: f64| (line.point[axis] + along * line.direction[axis] - level).abs();
            if moved(low) <= options.square_tolerance && moved(high) <= options.square_tolerance {
                let mut direction = [0.0; 2];
                direction[1 - axis] = line.direction[1 - axis].signum();
                line.point[axis] = level;
                line.direction = direction;
            }
        }
        let (low, high) = reach(&line);
        let at = |along: f64, across: f64| {
            [
                line.point[0] + along * line.direction[0] - across * line.direction[1],
                line.point[1] + along * line.direction[1] + across * line.direction[0],
            ]
        };
        let half = cell * 0.5;
        Some(FittedRing {
            ring: vec![
                at(low, -half),
                at(high, -half),
                at(high, half),
                at(low, half),
            ],
            owners: vec![[(0, self.segments.len()); 2]; 4],
        })
    }

    /// Whether a fitted ring still bounds what this ring bounds: it runs the
    /// same way round and has kept a fair part of the area. A strip of two
    /// cells that was centred on its points keeps half.
    fn agrees(&self, fitted: &[[f64; 2]]) -> bool {
        let (before, after) = (ring_signed_area(&self.corners), ring_signed_area(fitted));
        before * after > 0.0 && after.abs() >= 0.2 * before.abs()
    }
}

/// The line a short edge takes from the edges it joins, and whether that is
/// the line of one of them. A short edge, such as the jamb of a door or the
/// end of a wall, has too few cells to show its direction, and the outline of
/// the cells is a cell off over its few cells of length. It runs across or
/// along a measured edge it joins, where that agrees with the outline to
/// within the cell and a half the outline is good for. One that runs along
/// such an edge and lies that near to its line is a step of the outline of
/// that same face, as a face slightly off the grid leaves at its end, and
/// goes onto its line.
fn joined_line(
    position: usize,
    lines: &[Line],
    pieces: &[Piece],
    cell: f64,
) -> Option<(Line, bool)> {
    let piece = &pieces[position];
    if piece.fit.line.measured || piece.fit.plain {
        return None;
    }
    let half = distance(piece.from, piece.to) * 0.5;
    let chord = piece.fit.line.direction;
    // The direction, how far it is off the outline, and the edge it is the
    // direction of.
    let mut best: Option<([f64; 2], f64, Option<usize>)> = None;
    for neighbour in [
        (position + lines.len() - 1) % lines.len(),
        (position + 1) % lines.len(),
    ] {
        if !lines[neighbour].measured {
            continue;
        }
        let [x, y] = lines[neighbour].direction;
        for (candidate, along) in [
            ([x, y], Some(neighbour)),
            ([-x, -y], None),
            ([-y, x], None),
            ([y, -x], None),
        ] {
            let off = cross(chord, candidate).abs();
            if dot(chord, candidate) > 0.0
                && half * off <= SIMPLIFY_CELLS * cell
                && best.is_none_or(|(_, least, _)| off < least)
            {
                best = Some((candidate, off, along));
            }
        }
    }
    let (direction, _, along) = best?;
    let middle = [
        (piece.from[0] + piece.to[0]) * 0.5,
        (piece.from[1] + piece.to[1]) * 0.5,
    ];
    if let Some(edge) = along.map(|neighbour| &lines[neighbour]) {
        if distance(edge.nearest(middle), middle) <= SIMPLIFY_CELLS * cell {
            return Some((
                Line {
                    measured: false,
                    ..*edge
                },
                true,
            ));
        }
    }
    Some((
        Line {
            direction,
            ..lines[position]
        },
        false,
    ))
}

/// The level an edge is squared to, with the axis of the grid it is measured
/// along, or nothing when the edge stays as measured: squaring may move
/// neither of its `ends` by more than the tolerance.
fn square_level(
    edge: &Edge,
    ends: [[f64; 2]; 2],
    pieces: &[Piece],
    options: &OutlineOptions,
) -> Option<(usize, f64)> {
    let line = &pieces[edge.lead].fit.line;
    if !line.measured {
        return None;
    }
    let axis = level_axis(line.direction);
    let mut levels: Vec<f64> = (0..edge.pieces.1)
        .flat_map(|step| &pieces[(edge.pieces.0 + step) % pieces.len()].fit.points)
        .map(|at| at[axis])
        .collect();
    if levels.is_empty() {
        return None;
    }
    let level = median(&mut levels);
    ends.iter()
        .all(|end| (line.nearest(*end)[axis] - level).abs() <= options.square_tolerance)
        .then_some((axis, level))
}

/// Per edge, where the edge before it ends and where it begins: one corner,
/// or two where they do not meet.
type Joints = Vec<[[f64; 2]; 2]>;

/// The edges of a ring on the lines of its pieces, and where they join.
/// Nothing when no ring is left.
fn joined(lines: &[Line], pieces: &[Piece], cell: f64) -> Option<(Vec<Edge>, Joints)> {
    // Pieces that continue each other become one edge, on the line of the
    // longest of them: two halves of one face, each fitted by itself, would
    // otherwise leave an edge that is slightly off. The shorter must lie
    // along that line up to its far end: a face that bends by less than a
    // degree stays two edges.
    let mut edges: Vec<Edge> = Vec::with_capacity(lines.len());
    for (position, line) in lines.iter().enumerate() {
        let piece = &pieces[position];
        let length = distance(piece.from, piece.to);
        match edges.last_mut() {
            Some(last)
                if last.line.continues(
                    line,
                    piece.from,
                    if length > last.length {
                        last.start
                    } else {
                        piece.to
                    },
                ) =>
            {
                if length > last.length {
                    (last.line, last.length, last.lead) = (*line, length, position);
                }
                last.pieces.1 += 1;
            }
            _ => edges.push(Edge {
                line: *line,
                start: piece.from,
                length,
                lead: position,
                pieces: (position, 1),
            }),
        }
    }
    while edges.len() > 1 {
        let (first, last) = (edges[0], edges[edges.len() - 1]);
        // The first edge ends where the second begins.
        let far = if last.length > first.length {
            edges[1].start
        } else {
            last.start
        };
        if !last.line.continues(&first.line, first.start, far) {
            break;
        }
        edges[0] = Edge {
            start: last.start,
            pieces: (last.pieces.0, last.pieces.1 + first.pieces.1),
            ..if last.length > first.length {
                last
            } else {
                first
            }
        };
        edges.pop();
    }
    let reach = CORNER_REACH_CELLS * cell;
    loop {
        if edges.len() < 3 {
            return None;
        }
        let joints: Joints = (0..edges.len())
            .map(|position| {
                let previous = &edges[(position + edges.len() - 1) % edges.len()];
                let before = &previous.line;
                let (after, corner) = (&edges[position].line, edges[position].start);
                let turn = cross(before.direction, after.direction);
                if turn.abs() > 1e-9 {
                    let along = cross(minus(after.point, before.point), after.direction) / turn;
                    let meet = [
                        before.point[0] + along * before.direction[0],
                        before.point[1] + along * before.direction[1],
                    ];
                    // Two edges at an angle meet in a corner.
                    if turn.abs() >= 0.05 && distance(meet, corner) <= reach {
                        return [meet, meet];
                    }
                    // A slight bend: the outline keeps its corner where the
                    // face has drifted a cell, well past the bend. The edges
                    // meet in the bend when it lies on both of them and the
                    // points between it and the corner follow the other
                    // edge; where they follow their own, the face steps at
                    // the corner.
                    let end = edges[(position + 1) % edges.len()].start;
                    let (owner, other) = if dot(minus(meet, corner), before.direction) < 0.0 {
                        (previous, after)
                    } else {
                        (&edges[position], before)
                    };
                    if dot(minus(meet, previous.start), before.direction) >= reach
                        && dot(minus(end, meet), after.direction) >= reach
                        && (distance(meet, corner) <= reach
                            || turned_between(owner, other, [meet, corner], pieces))
                    {
                        return [meet, meet];
                    }
                }
                // Edges that are parallel, or that meet beyond either of
                // them: a step from the one to the other, at the corner of
                // the outline.
                [before.nearest(corner), after.nearest(corner)]
            })
            .collect();
        // A short edge between two that moved towards each other is passed
        // before it begins: it would run backwards. Without it its
        // neighbours meet each other.
        let backwards = (0..edges.len())
            .filter(|position| {
                let (from, to) = (
                    joints[*position][1],
                    joints[(position + 1) % edges.len()][0],
                );
                dot(minus(to, from), edges[*position].line.direction) < -1e-9
            })
            .min_by(|a, b| edges[*a].length.total_cmp(&edges[*b].length));
        if let Some(position) = backwards {
            let start = edges[position].start;
            edges.remove(position);
            // The edge after it now begins where it began.
            let next = position % edges.len();
            edges[next].start = start;
            continue;
        }
        return Some((edges, joints));
    }
}

/// Whether the points of `edge` between two positions along it lie nearer to
/// the line `other` than to its own: the face had turned onto the other line
/// before the corner that the outline kept.
fn turned_between(edge: &Edge, other: &Line, [from, to]: [[f64; 2]; 2], pieces: &[Piece]) -> bool {
    let line = &edge.line;
    let place = |at: [f64; 2]| dot(minus(at, line.point), line.direction);
    let off = |line: &Line, at: [f64; 2]| cross(line.direction, minus(at, line.point)).abs();
    let (low, high) = (place(from).min(place(to)), place(from).max(place(to)));
    let (mut own, mut theirs) = (0.0, 0.0);
    for step in 0..edge.pieces.1 {
        let piece = &pieces[(edge.pieces.0 + step) % pieces.len()];
        for at in &piece.fit.points {
            if (low..=high).contains(&place(*at)) {
                own += off(line, *at);
                theirs += off(other, *at);
            }
        }
    }
    theirs < own
}

/// One edge of a fitted ring: its line, the corner of the outline it starts
/// at, the longest piece that lies on it, whose line it has, as its length
/// and position, and the pieces it stands for: the first and how many.
#[derive(Debug, Clone, Copy)]
struct Edge {
    line: Line,
    start: [f64; 2],
    length: f64,
    lead: usize,
    pieces: (usize, usize),
}

/// A ring after the fit, with for every corner the segments behind the edge
/// that leaves it, as first and number; two edges where the edge is a step
/// between them.
struct FittedRing {
    ring: Vec<[f64; 2]>,
    owners: Vec<[(usize, usize); 2]>,
}

/// A line through a point in a direction of unit length. The region lies to
/// its left.
#[derive(Debug, Clone, Copy)]
struct Line {
    point: [f64; 2],
    direction: [f64; 2],
    /// Whether the direction was measured from the points or set by
    /// squaring, and not taken from the outline of the cells.
    measured: bool,
}

impl Line {
    /// Whether `next` goes on where this line arrives at `corner`: in the
    /// same direction to within a degree, and no more than 2 mm beside it,
    /// both there and at `far`, the other end of the shorter of the two,
    /// which is drawn on the line of the longer.
    fn continues(&self, next: &Self, corner: [f64; 2], far: [f64; 2]) -> bool {
        let beside = |at: [f64; 2]| distance(self.nearest(at), next.nearest(at)) < 0.002;
        cross(self.direction, next.direction).abs() < 0.02
            && dot(self.direction, next.direction) > 0.0
            && beside(corner)
            && beside(far)
    }

    /// The point of the line nearest a position.
    fn nearest(&self, at: [f64; 2]) -> [f64; 2] {
        let along = dot(minus(at, self.point), self.direction);
        [
            self.point[0] + along * self.direction[0],
            self.point[1] + along * self.direction[1],
        ]
    }
}

/// A stretch of a ring that is drawn on one line: a segment, or one of the
/// two parts of a segment whose points lie along two lines.
struct Piece {
    /// The segment it is, or is a part of.
    segment: usize,
    from: [f64; 2],
    to: [f64; 2],
    fit: Fit,
}

impl Piece {
    /// The line of this piece along the main direction: level at `level` on
    /// `axis`, pointing the way `forward` does.
    fn squared(&self, axis: usize, level: f64, forward: [f64; 2]) -> Line {
        let mut point = [
            (self.from[0] + self.to[0]) * 0.5,
            (self.from[1] + self.to[1]) * 0.5,
        ];
        point[axis] = level;
        let mut direction = [0.0; 2];
        direction[1 - axis] = forward[1 - axis].signum();
        Line {
            point,
            direction,
            measured: true,
        }
    }

    /// This piece squared by itself, when that moves neither of its ends by
    /// more than the tolerance. Without points the ends are those of the
    /// outline.
    fn squared_alone(&self, tolerance: f64) -> Option<Line> {
        let fit = &self.fit;
        let chord = fit.line.direction;
        let axis = level_axis(chord);
        if fit.points.is_empty() {
            let spread = (self.to[axis] - self.from[axis]).abs();
            return (!fit.plain && spread <= 2.0 * tolerance)
                .then(|| self.squared(axis, (self.from[axis] + self.to[axis]) * 0.5, chord));
        }
        let mut levels: Vec<f64> = fit.points.iter().map(|at| at[axis]).collect();
        let level = median(&mut levels);
        let moved = |end: [f64; 2]| (fit.line.nearest(end)[axis] - level).abs();
        (moved(self.from) <= tolerance && moved(self.to) <= tolerance)
            .then(|| self.squared(axis, level, chord))
    }
}

/// What the points say about one stretch of a ring.
struct Fit {
    /// The line through the mean positions of the points in its cells, or
    /// along the outline of the cells where there are too few of them or
    /// the segment is to stay on the outline.
    line: Line,
    /// The positions the line was fitted through, in the order of the
    /// outline; none where it follows the outline.
    points: Vec<[f64; 2]>,
    /// Whether it bounds a face scanned from one side.
    thin: bool,
    /// Whether it stays on the outline of the cells.
    plain: bool,
}

impl Fit {
    /// A line of this stretch as it is drawn. A face scanned from one side
    /// has both edges of its strip on the same points: half a cell to either
    /// side keeps a strip of one cell, centred on the face.
    fn drawn(&self, mut line: Line, cell: f64) -> Line {
        if self.thin {
            let outward = [line.direction[1], -line.direction[0]];
            line.point = [
                line.point[0] + outward[0] * cell * 0.5,
                line.point[1] + outward[1] * cell * 0.5,
            ];
        }
        line
    }

    /// The two fits of a segment from `from` to `to` whose points lie along
    /// two lines, and where on the segment the one goes over into the other.
    ///
    /// The outline is reduced to segments that stay within a cell and a
    /// half of it, so a face that bends a little, or steps by less than
    /// that, is one segment. Its points tell: one line leaves them a few
    /// millimetres off, two lines that meet at the right place leave half
    /// of that at most.
    fn split(&self, from: [f64; 2], to: [f64; 2], cell: f64) -> Option<(Fit, Fit, [f64; 2])> {
        let count = self.points.len();
        if !self.line.measured || count < 2 * SPLIT_CELLS {
            return None;
        }
        let length = distance(from, to);
        let chord = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
        // Per point how far along the segment it lies and how far beside
        // it, and the sums over all points before each that the residual
        // of a line through any run of them follows from.
        let mut sums = Vec::with_capacity(count + 1);
        let mut running = [0.0f64; 5];
        sums.push(running);
        let places: Vec<[f64; 2]> = self
            .points
            .iter()
            .map(|at| {
                let at = minus(*at, from);
                [dot(at, chord), cross(chord, at)]
            })
            .collect();
        for [along, beside] in places.iter().copied() {
            let terms = [
                along,
                beside,
                along * along,
                along * beside,
                beside * beside,
            ];
            for (sum, term) in running.iter_mut().zip(terms) {
                *sum += term;
            }
            sums.push(running);
        }
        // The squared distances that the best line through the points from
        // `first` up to `end` leaves.
        let residual = |first: usize, end: usize| -> f64 {
            let n = (end - first) as f64;
            let [x, y, xx, xy, yy] =
                std::array::from_fn(|term| sums[end][term] - sums[first][term]);
            let (sxx, sxy, syy) = (xx - x * x / n, xy - x * y / n, yy - y * y / n);
            if sxx > 0.0 {
                (syy - sxy * sxy / sxx).max(0.0)
            } else {
                syy.max(0.0)
            }
        };
        let whole = residual(0, count);
        if (whole / count as f64).sqrt() < SPLIT_RESIDUAL {
            return None;
        }
        let (at, least) = (SPLIT_CELLS..=count - SPLIT_CELLS)
            .map(|at| (at, residual(0, at) + residual(at, count)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?;
        // Half the distance is a quarter of its square.
        if least > 0.25 * whole {
            return None;
        }
        let along = (places[at - 1][0] + places[at][0]) * 0.5;
        if along <= 0.0 || along >= length {
            return None;
        }
        let part = |points: &[[f64; 2]]| Fit {
            line: line_through(points, chord, cell),
            points: points.to_vec(),
            thin: self.thin,
            plain: false,
        };
        let (first, second) = (part(&self.points[..at]), part(&self.points[at..]));
        (first.line.measured && second.line.measured).then(|| {
            (
                first,
                second,
                [from[0] + along * chord[0], from[1] + along * chord[1]],
            )
        })
    }
}

/// The axis of the grid across a direction: the coordinate that is level
/// along an edge that runs that way.
fn level_axis(direction: [f64; 2]) -> usize {
    if direction[0].abs() >= direction[1].abs() {
        1
    } else {
        0
    }
}

impl Segment {
    /// The line through the points of this segment: through their mean
    /// positions per cell when there are enough of them.
    fn fit(&self, means: &CellMap<Mean>, cell: f64, plain: bool) -> Option<Fit> {
        let chord = minus(self.to, self.from);
        let length = chord[0].hypot(chord[1]);
        if length <= 0.0 {
            return None;
        }
        let chord = [chord[0] / length, chord[1] / length];
        let outline = |point: [f64; 2]| Fit {
            line: Line {
                point,
                direction: chord,
                measured: false,
            },
            points: Vec::new(),
            thin: false,
            plain,
        };
        if plain {
            return Some(outline(self.from));
        }
        let carried: Vec<&Mean> = self
            .supports
            .iter()
            .filter_map(|cell| means.get(cell))
            .filter(|mean| mean.weight > 0.0)
            .collect();
        let points: Vec<[f64; 2]> = carried
            .iter()
            .map(|mean| [mean.sum[0] / mean.weight, mean.sum[1] / mean.weight])
            .collect();
        // Cells that also carry a segment other than the two this one
        // joins, which share the cell in their corner: the two sides of a
        // strip that holds one row of points.
        let shared = carried
            .iter()
            .filter(|mean| {
                mean.more
                    || mean.segments.iter().any(|other| {
                        *other != 0 && *other != self.number && !self.beside.contains(other)
                    })
            })
            .count();
        // A short end, such as the jamb of a door, has few cells to show.
        let enough = points.len() >= SUPPORT_CELLS || (self.edges <= 8 && points.len() >= 3);
        if !enough {
            // An edge the closing made: no points lie along it. It keeps the
            // outline of the cells.
            return Some(outline([
                (self.from[0] + self.to[0]) * 0.5,
                (self.from[1] + self.to[1]) * 0.5,
            ]));
        }
        Some(Fit {
            line: line_through(&points, chord, cell),
            thin: 2 * self.thin > self.edges || 2 * shared > points.len(),
            points,
            plain,
        })
    }
}

/// The least-squares line through positions, pointing the way `chord` does.
/// Positions far off the line, such as the cell in a corner that also holds
/// points of the next face, are left out of a second fit.
fn line_through(points: &[[f64; 2]], chord: [f64; 2], cell: f64) -> Line {
    let fit = |points: &[[f64; 2]]| -> Line {
        let count = points.len() as f64;
        let centre = [
            points.iter().map(|at| at[0]).sum::<f64>() / count,
            points.iter().map(|at| at[1]).sum::<f64>() / count,
        ];
        let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
        for at in points {
            let (x, y) = (at[0] - centre[0], at[1] - centre[1]);
            xx += x * x;
            xy += x * y;
            yy += y * y;
        }
        let angle = 0.5 * (2.0 * xy).atan2(xx - yy);
        let mut direction = [angle.cos(), angle.sin()];
        let spread = xx * direction[0] * direction[0]
            + 2.0 * xy * direction[0] * direction[1]
            + yy * direction[1] * direction[1];
        // Positions spread evenly over a length have a variance of that
        // length squared over twelve. Over less than ten cells, as along the
        // jamb of a door, or across the segment, they do not tell a
        // direction; the outline does.
        let measured =
            (12.0 * spread / count).sqrt() >= 10.0 * cell && dot(direction, chord).abs() >= 0.9;
        if !measured {
            direction = chord;
        } else if dot(direction, chord) < 0.0 {
            direction = [-direction[0], -direction[1]];
        }
        Line {
            point: centre,
            direction,
            measured,
        }
    };
    let first = fit(points);
    let mut off: Vec<f64> = points
        .iter()
        .map(|at| cross(first.direction, minus(*at, first.point)))
        .collect();
    let middle = median(&mut off.clone());
    let mut spread: Vec<f64> = off.iter().map(|value| (value - middle).abs()).collect();
    let limit = (3.0 * 1.4826 * median(&mut spread)).max(0.1 * cell);
    off.iter_mut().for_each(|value| *value -= middle);
    let near: Vec<[f64; 2]> = points
        .iter()
        .zip(&off)
        .filter(|(_, off)| off.abs() <= limit)
        .map(|(at, _)| *at)
        .collect();
    if near.len() >= 3 && near.len() < points.len() {
        fit(&near)
    } else {
        first
    }
}

fn median(values: &mut [f64]) -> f64 {
    let middle = values.len() / 2;
    *values.select_nth_unstable_by(middle, f64::total_cmp).1
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

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The edges of the rings of a region that cross another edge, each as its
/// ring and the corner it leaves. Edges that only touch, as neighbours do,
/// do not count.
fn crossing_edges(rings: &[&[[f64; 2]]]) -> Vec<(usize, usize)> {
    struct Span {
        a: [f64; 2],
        b: [f64; 2],
        ring: usize,
        vertex: usize,
    }
    let mut edges: Vec<Span> = rings
        .iter()
        .enumerate()
        .flat_map(|(ring, corners)| {
            (0..corners.len()).map(move |vertex| Span {
                a: corners[vertex],
                b: corners[(vertex + 1) % corners.len()],
                ring,
                vertex,
            })
        })
        .collect();
    // From left to right, so that only edges that overlap sideways are
    // compared.
    let left = |edge: &Span| edge.a[0].min(edge.b[0]);
    let right = |edge: &Span| edge.a[0].max(edge.b[0]);
    edges.sort_by(|a, b| left(a).total_cmp(&left(b)));
    // Whether the ends of one edge lie on either side of the other, each by
    // more than a thousandth of a millimetre: an end that lies on the other
    // edge touches it, whatever the rounding says.
    let astride = |edge: &Span, other: &Span| {
        let length = distance(edge.a, edge.b);
        let side = |at: [f64; 2]| cross(minus(edge.b, edge.a), minus(at, edge.a)) / length;
        let (a, b) = (side(other.a), side(other.b));
        a * b < 0.0 && a.abs() > 1e-6 && b.abs() > 1e-6
    };
    let mut crossing = Vec::new();
    for (index, first) in edges.iter().enumerate() {
        for second in &edges[index + 1..] {
            if left(second) > right(first) {
                break;
            }
            if astride(first, second) && astride(second, first) {
                crossing.push((first.ring, first.vertex));
                crossing.push((second.ring, second.vertex));
            }
        }
    }
    crossing
}

#[cfg(test)]
pub(super) mod probe {
    //! Measuring a filled cut the way a drawing is measured: along a line.

    use super::CutRegion;
    use crate::test_shapes::{
        box_room, cylinder, plane_with_hole, sphere, stray_points, CylinderSpec, Noise, Opening,
        RoomSpec, Shape, Wall,
    };
    use crate::Bounds;

    fn rings(regions: &[CutRegion]) -> impl Iterator<Item = &Vec<[f64; 2]>> {
        regions
            .iter()
            .flat_map(|region| std::iter::once(&region.outer).chain(&region.holes))
    }

    /// Where the outlines of the regions cross the line on which coordinate
    /// `axis` equals `level`: the other coordinate of every crossing,
    /// ascending. Along such a line the cut is entered and left in turns.
    pub(crate) fn crossings(regions: &[CutRegion], axis: usize, level: f64) -> Vec<f64> {
        let mut found = Vec::new();
        for ring in rings(regions) {
            for index in 0..ring.len() {
                let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
                if (a[axis] > level) != (b[axis] > level) {
                    let share = (level - a[axis]) / (b[axis] - a[axis]);
                    found.push(a[1 - axis] + share * (b[1 - axis] - a[1 - axis]));
                }
            }
        }
        found.sort_by(f64::total_cmp);
        found
    }

    /// The largest amount, in degrees, by which a corner of the regions
    /// differs from a right angle.
    pub(crate) fn worst_corner(regions: &[CutRegion]) -> f64 {
        let mut worst = 0.0f64;
        for ring in rings(regions) {
            let n = ring.len();
            for index in 0..n {
                let (a, b, c) = (ring[index], ring[(index + 1) % n], ring[(index + 2) % n]);
                let (first, second) = ([b[0] - a[0], b[1] - a[1]], [c[0] - b[0], c[1] - b[1]]);
                let turn = (first[0] * second[1] - first[1] * second[0])
                    .atan2(first[0] * second[0] + first[1] * second[1])
                    .to_degrees();
                worst = worst.max((turn.abs() - 90.0).abs());
            }
        }
        worst
    }

    /// The largest distance between what was measured and what is true.
    pub(crate) fn worst_error(measured: &[f64], truth: &[f64]) -> f64 {
        assert_eq!(measured.len(), truth.len(), "{measured:?} for {truth:?}");
        measured
            .iter()
            .zip(truth)
            .map(|(measured, truth)| (measured - truth).abs())
            .fold(0.0, f64::max)
    }

    /// The slab of a plan through a room of 4.0 by 3.0 m with walls of 0.1 m
    /// scanned on both faces, a door in the south wall from x = 1.0 to 1.9
    /// and a window in the east wall from y = 0.8 to 2.0, a square column of
    /// 0.3 m at x 1.2 to 1.5, y 1.6 to 1.9, and what a furnished room adds:
    /// the leg of a chair 0.28 m from the west wall, a small object and 200
    /// stray points. Scanner noise of 2 mm on every face.
    pub(crate) fn furnished_room() -> Shape {
        let slab = Some([1.0, 1.1]);
        let room = box_room(&RoomSpec {
            spacing: 0.005,
            wall_thickness: Some(0.1),
            openings: vec![
                Opening::door(Wall::South, 1.0),
                Opening::window(Wall::East, 0.8),
            ],
            slab,
            ..RoomSpec::default()
        });
        let column = box_room(&RoomSpec {
            size: [0.3, 0.3, 2.6],
            spacing: 0.005,
            slab,
            ..RoomSpec::default()
        })
        .transformed(0.0, [1.2, 1.6, 0.0]);
        let mut leg = CylinderSpec::column([0.3, 2.5, 1.0], 0.02, 0.1);
        leg.spacing = 0.005;
        let object = sphere([3.0, 2.2, 1.05], 0.025, 400);
        let stray = stray_points(
            Bounds {
                min: [-0.1, -0.1, 1.0],
                max: [4.1, 3.1, 1.1],
            },
            200,
            7,
        );
        room.merged(column)
            .with_noise(Noise::Gaussian(0.002), 11)
            .merged(cylinder(&leg))
            .merged(object)
            .merged(stray)
    }

    /// How far the cut of the furnished room is from the truth, in metres:
    /// along lines across the wall faces and the column, and along lines
    /// through the door and the window. A count of crossings that differs
    /// from the truth, as a wall that is missing or an opening that is
    /// closed would give, fails.
    pub(crate) fn room_errors(regions: &[CutRegion]) -> (f64, f64) {
        let faces = [
            (1, 2.5, [-0.1, 0.0, 4.0, 4.1]),
            (0, 3.0, [-0.1, 0.0, 3.0, 3.1]),
            (1, 1.75, [-0.1, 0.0, 1.2, 1.5]),
        ];
        let openings = [
            (1, -0.05, [-0.1, 1.0, 1.9, 4.1]),
            (0, 4.05, [-0.1, 0.8, 2.0, 3.1]),
        ];
        let worst = |lines: &[(usize, f64, [f64; 4])]| {
            lines
                .iter()
                .map(|(axis, level, truth)| worst_error(&crossings(regions, *axis, *level), truth))
                .fold(0.0, f64::max)
        };
        (worst(&faces), worst(&openings))
    }

    /// A room to cut vertical sections through: walls of 0.1 m with a
    /// window in the east wall from y = 0.8 to 2.0 and z = 0.9 to 2.1, and a
    /// floor and a ceiling of 0.25 m scanned from above and from below.
    /// Scanner noise of 2 mm on every face.
    pub(crate) fn section_shape() -> Shape {
        let room = box_room(&RoomSpec {
            wall_thickness: Some(0.1),
            openings: vec![Opening::window(Wall::East, 0.8)],
            ..RoomSpec::default()
        });
        let under =
            |z: f64| plane_with_hole([4.2, 3.2], 0.02, None).transformed(0.0, [-0.1, -0.1, z]);
        room.merged(under(-0.25))
            .merged(under(2.85))
            .with_noise(Noise::Gaussian(0.002), 13)
    }
}

#[cfg(test)]
mod tests {
    use super::probe::{
        crossings, furnished_room, room_errors, section_shape, worst_corner, worst_error,
    };
    use super::*;
    use crate::test_shapes::{
        box_room, cylinder, stray_points, CylinderSpec, Noise, Opening, Rng, RoomSpec, Shape, Wall,
    };
    use crate::Bounds;

    /// The count grid of points seen from above: `u` is x and `v` is y.
    fn plan_grid(points: &[[f64; 3]], cell: f64) -> CutGrid {
        grid_of(points.iter().map(|point| [point[0], point[1]]), cell)
    }

    fn grid_of(points: impl Iterator<Item = [f64; 2]> + Clone, cell: f64) -> CutGrid {
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for at in points.clone() {
            for axis in 0..2 {
                low[axis] = low[axis].min(at[axis]);
                high[axis] = high[axis].max(at[axis]);
            }
        }
        let mut grid =
            CutGrid::new(GridFrame::covering(low, high, cell, MAX_CUT_GRID_CELLS).unwrap());
        points.for_each(|at| grid.add(at));
        grid
    }

    fn options() -> OutlineOptions {
        OutlineOptions::for_request(&DrawingRequest::default())
    }

    fn trace(grid: &CutGrid, options: &OutlineOptions) -> CutOutline {
        trace_cut_regions(grid, options, &mut || Ok(())).unwrap()
    }

    /// Regions drawn from a scan that was turned and moved, brought back to
    /// where the scan was before.
    fn turned_back(regions: &[CutRegion], degrees: f64, moved: [f64; 2]) -> Vec<CutRegion> {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let ring = |ring: &Vec<[f64; 2]>| -> Vec<[f64; 2]> {
            ring.iter()
                .map(|at| {
                    let (x, y) = (at[0] - moved[0], at[1] - moved[1]);
                    [cos * x + sin * y, cos * y - sin * x]
                })
                .collect()
        };
        regions
            .iter()
            .map(|region| CutRegion {
                outer: ring(&region.outer),
                holes: region.holes.iter().map(ring).collect(),
            })
            .collect()
    }

    #[test]
    fn plan_of_a_room_fills_the_walls_and_leaves_door_and_window_open() {
        let room = furnished_room();
        let outline = trace(&plan_grid(&room.points, 0.02), &options());
        // Two parts of wall, from the door to the window either way round,
        // and the column. The chair leg, the object and the stray points
        // are gone, and nothing ties them to a wall.
        assert_eq!(outline.regions.len(), 3);
        assert!(outline.regions.iter().all(|region| region.holes.is_empty()));
        assert_eq!((outline.dropped, outline.unfitted), (0, 0));
        assert_eq!(outline.direction_degrees, 0.0);
        assert_eq!(outline.cell, 0.02);
        let (faces, openings) = room_errors(&outline.regions);
        // Measured 0.06 mm on the faces and 2.3 mm at the jambs, with 2 mm
        // of scanner noise: far inside the 10 mm that is stated for
        // straight faces scanned on both sides.
        assert!(faces < 0.001, "{faces}");
        assert!(openings < 0.006, "{openings}");
        // Squared: every corner is a right angle.
        assert!(worst_corner(&outline.regions) < 1e-9);
        let mut areas: Vec<f64> = outline.regions.iter().map(CutRegion::area).collect();
        areas.sort_by(f64::total_cmp);
        // The column of 0.3 by 0.3 m, the wall from door to window by the
        // south-east corner (2.2 + 0.8 m of 0.1 m) and the rest
        // (1.1 + 3.2 + 4.0 + 1.1 m less the corner counted twice).
        for (area, truth) in areas.iter().zip([0.09, 0.30, 0.93]) {
            assert!((area - truth).abs() < 0.002, "{area} for {truth}");
        }

        // As measured, without squaring: the faces as true (0.05 mm), the
        // jambs within 5.3 mm, and corners within 0.12 degree of square.
        let free = trace(
            &plan_grid(&room.points, 0.02),
            &OutlineOptions {
                square: false,
                ..options()
            },
        );
        assert_eq!(free.regions.len(), 3);
        let (faces, openings) = room_errors(&free.regions);
        assert!(faces < 0.001, "{faces}");
        assert!(openings < 0.010, "{openings}");
        assert!(worst_corner(&free.regions) < 0.5);
    }

    #[test]
    fn a_turned_room_far_from_the_origin_is_drawn_as_true_as_a_straight_one() {
        let moved = [207_000.0, 474_000.0];
        for degrees in [30.0, 17.3, 44.0, 61.5, 89.0] {
            let room = furnished_room().transformed(degrees, [moved[0], moved[1], 0.0]);
            let grid = plan_grid(&room.points, 0.02);
            let squared = trace(&grid, &options());
            assert_eq!(squared.regions.len(), 3, "{degrees}");
            assert_eq!(squared.unfitted, 0);
            // The main direction is the same a quarter turn on.
            let found = quarter_turn(squared.direction_degrees - degrees);
            let back = turned_back(&squared.regions, degrees, moved);
            let (faces, openings) = room_errors(&back);
            // Measured over the five directions: the direction found
            // exactly, faces within 0.25 mm and jambs within 2.4 mm.
            assert!(found.abs() < 0.01, "{degrees}: {found}");
            assert!(faces < 0.001, "{degrees}: {faces}");
            assert!(openings < 0.006, "{degrees}: {openings}");
            assert!(worst_corner(&back) < 1e-6, "{degrees}");

            let free = trace(
                &grid,
                &OutlineOptions {
                    square: false,
                    ..options()
                },
            );
            assert_eq!(free.regions.len(), 3, "{degrees}");
            let back = turned_back(&free.regions, degrees, moved);
            let (faces, openings) = room_errors(&back);
            // Without squaring: faces within 0.25 mm, jambs within 3.2 mm
            // and corners within 0.38 degree of square.
            assert!(faces < 0.001, "{degrees}: {faces}");
            assert!(openings < 0.008, "{degrees}: {openings}");
            assert!(worst_corner(&back) < 1.0, "{degrees}");
        }
    }

    #[test]
    fn a_closed_room_is_a_hole_in_the_region_of_its_walls() {
        let room = box_room(&RoomSpec {
            spacing: 0.005,
            wall_thickness: Some(0.1),
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        })
        .with_noise(Noise::Gaussian(0.002), 3);
        let outline = trace(&plan_grid(&room.points, 0.02), &options());
        assert_eq!(outline.regions.len(), 1);
        let region = &outline.regions[0];
        assert_eq!(region.holes.len(), 1);
        assert_eq!((region.outer.len(), region.holes[0].len()), (4, 4));
        // Outer ring counter-clockwise, hole clockwise.
        let (outer, hole) = (
            ring_signed_area(&region.outer),
            ring_signed_area(&region.holes[0]),
        );
        // The room is 12 m2 and the walls take 4.2 by 3.2 m: measured
        // 12.00006 and 13.43986 m2.
        assert!((hole + 12.0).abs() < 0.001, "{hole}");
        assert!((outer - 13.44).abs() < 0.001, "{outer}");
        assert!((region.area() - 1.44).abs() < 0.001);
    }

    /// A free-standing wall of 5.0 by 0.2 m scanned all round, turned about
    /// its lower left corner at (0, 5).
    fn loose_wall(degrees: f64) -> Shape {
        box_room(&RoomSpec {
            size: [5.0, 0.2, 2.6],
            spacing: 0.005,
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        })
        .with_noise(Noise::Gaussian(0.002), 5)
        .transformed(degrees, [0.0, 5.0, 0.0])
    }

    #[test]
    fn squaring_turns_a_wall_only_when_no_end_moves_more_than_the_tolerance() {
        let room = furnished_room();
        // The room gives the main direction; the wall beside it is off.
        let wall_of = |outline: &CutOutline| -> CutRegion {
            outline
                .regions
                .iter()
                .find(|region| region.outer.iter().all(|at| at[1] > 4.0))
                .cloned()
                .unwrap()
        };
        // The edges of the wall along its length, as degrees from the axis.
        let long_edges = |wall: &CutRegion| -> Vec<f64> {
            let n = wall.outer.len();
            (0..n)
                .map(|index| (wall.outer[index], wall.outer[(index + 1) % n]))
                .filter(|(a, b)| (b[0] - a[0]).hypot(b[1] - a[1]) > 4.0)
                .map(|(a, b)| quarter_turn((b[1] - a[1]).atan2(b[0] - a[0]).to_degrees()))
                .collect()
        };
        // How far the true corners of the wall are from a drawn corner.
        let corner_error = |wall: &CutRegion, degrees: f64| -> f64 {
            let (sin, cos) = f64::to_radians(degrees).sin_cos();
            [[0.0, 0.0], [5.0, 0.0], [5.0, 0.2], [0.0, 0.2]]
                .into_iter()
                .map(|[x, y]| [cos * x - sin * y, 5.0 + sin * x + cos * y])
                .map(|truth| {
                    wall.outer
                        .iter()
                        .map(|at| distance(*at, truth))
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(0.0, f64::max)
        };

        // Two degrees over 5 m is 0.17 m at the far end: left as measured.
        let scan = room.clone().merged(loose_wall(2.0));
        let outline = trace(&plan_grid(&scan.points, 0.02), &options());
        assert_eq!(outline.regions.len(), 4);
        assert_eq!(outline.direction_degrees, 0.0);
        let wall = wall_of(&outline);
        let edges = long_edges(&wall);
        assert_eq!(edges.len(), 2);
        // Measured 2.0006 and 1.9998 degrees, and corners within 3.9 mm.
        assert!(
            edges.iter().all(|edge| (edge - 2.0).abs() < 0.01),
            "{edges:?}"
        );
        assert!(corner_error(&wall, 2.0) < 0.010);

        // A fifth of a degree is 17 mm at the far end, 9 mm either way from
        // the middle: squared.
        let scan = room.merged(loose_wall(0.2));
        let outline = trace(&plan_grid(&scan.points, 0.02), &options());
        let wall = wall_of(&outline);
        let edges = long_edges(&wall);
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|edge| edge.abs() < 1e-9), "{edges:?}");
        assert!(worst_corner(std::slice::from_ref(&wall)) < 1e-9);
        // The corners moved by what the squaring took, 8.7 mm at 2.5 m from
        // the middle: measured 8.9 mm.
        assert!(corner_error(&wall, 0.2) < 0.015);
    }

    #[test]
    fn a_face_scanned_from_one_side_stays_a_strip_on_its_points() {
        // One face of 2 m at v = 1.0, with 2 mm of noise, and a fragment of
        // 0.2 m well away from it.
        let mut rng = Rng::new(21);
        let mut points = Vec::new();
        for (from, length, level) in [(0.0, 2.0, 1.0), (0.5, 0.2, 2.5)] {
            for column in 0..(length / 0.005) as usize {
                for _ in 0..20 {
                    points.push([
                        from + (column as f64 + 0.5) * 0.005,
                        level + rng.gaussian() * 0.002,
                    ]);
                }
            }
        }
        for square in [true, false] {
            let outline = trace(
                &grid_of(points.iter().copied(), 0.02),
                &OutlineOptions {
                    square,
                    ..options()
                },
            );
            // The fragment is under 50 mm by 0.30 m.
            assert_eq!(outline.regions.len(), 1, "square {square}");
            assert_eq!(outline.dropped, 1);
            let strip = &outline.regions[0];
            let across = crossings(std::slice::from_ref(strip), 0, 1.0);
            let along = crossings(
                std::slice::from_ref(strip),
                1,
                (across[0] + across[1]) * 0.5,
            );
            // One cell thick, centred on the face: both edges within 10 mm
            // of the surface. Measured 20.000 mm thick and 0.03 mm off
            // centre.
            assert_eq!(across.len(), 2);
            assert!((across[1] - across[0] - 0.02).abs() < 1e-6, "{across:?}");
            assert!(
                ((across[0] + across[1]) * 0.5 - 1.0).abs() < 0.0005,
                "{across:?}"
            );
            // Its ends lie on the cells: within one cell of the true ends,
            // measured 10 mm.
            assert!(worst_error(&along, &[0.0, 2.0]) < 0.02, "{along:?}");
        }

        // The same face beside a room, 25 degrees off the direction of the
        // room: on the grid of the room it is a stair of cells, and still
        // comes out as a strip of one cell on its points.
        let (sin, cos) = 25f64.to_radians().sin_cos();
        let mut scan: Vec<[f64; 2]> = furnished_room()
            .points
            .iter()
            .map(|point| [point[0], point[1]])
            .collect();
        scan.extend(points.iter().filter(|at| at[1] < 2.0).map(|at| {
            [
                cos * at[0] - sin * (at[1] - 1.0),
                5.0 + sin * at[0] + cos * (at[1] - 1.0),
            ]
        }));
        let outline = trace(&grid_of(scan.iter().copied(), 0.02), &options());
        assert_eq!(outline.direction_degrees, 0.0);
        assert_eq!(outline.regions.len(), 4);
        let strip: Vec<CutRegion> = outline
            .regions
            .iter()
            .filter(|region| region.outer.iter().all(|at| at[1] > 4.0))
            .map(|region| CutRegion {
                outer: region
                    .outer
                    .iter()
                    .map(|at| {
                        [
                            cos * at[0] + sin * (at[1] - 5.0),
                            cos * (at[1] - 5.0) - sin * at[0],
                        ]
                    })
                    .collect(),
                holes: Vec::new(),
            })
            .collect();
        assert_eq!(strip.len(), 1);
        let across = crossings(&strip, 0, 1.0);
        let along = crossings(&strip, 1, (across[0] + across[1]) * 0.5);
        // Measured 20.01 mm thick, 0.06 mm off centre, and its ends within
        // 24 mm.
        assert_eq!(across.len(), 2);
        assert!((across[1] - across[0] - 0.02).abs() < 0.001, "{across:?}");
        assert!(((across[0] + across[1]) * 0.5).abs() < 0.001, "{across:?}");
        assert!(worst_error(&along, &[0.0, 2.0]) < 0.04, "{along:?}");
    }

    #[test]
    fn a_wall_thicker_than_the_largest_wall_is_two_faces_until_the_limit_is_raised() {
        // 3.0 by 0.6 m, scanned all round.
        let wall = box_room(&RoomSpec {
            size: [3.0, 0.6, 2.6],
            spacing: 0.005,
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        });
        let grid = plan_grid(&wall.points, 0.02);
        let hollow = trace(&grid, &options());
        assert_eq!(hollow.regions.len(), 1);
        assert_eq!(hollow.regions[0].holes.len(), 1);
        let filled = trace(
            &grid,
            &OutlineOptions {
                max_wall_thickness: 0.7,
                ..options()
            },
        );
        assert_eq!(filled.regions.len(), 1);
        assert!(filled.regions[0].holes.is_empty());
        assert!((filled.regions[0].area() - 1.8).abs() < 0.005);

        // An opening is the same question along the wall: 0.4 m between two
        // stubs is closed, 0.7 m stays open.
        let stub = |from: f64| {
            box_room(&RoomSpec {
                size: [1.0, 0.1, 2.6],
                spacing: 0.005,
                slab: Some([1.0, 1.1]),
                ..RoomSpec::default()
            })
            .transformed(0.0, [from, 0.0, 0.0])
        };
        let narrow = stub(0.0).merged(stub(1.4));
        assert_eq!(
            trace(&plan_grid(&narrow.points, 0.02), &options())
                .regions
                .len(),
            1
        );
        let wide = stub(0.0).merged(stub(1.7));
        let outline = trace(&plan_grid(&wide.points, 0.02), &options());
        assert_eq!(outline.regions.len(), 2);
        let ends = crossings(&outline.regions, 1, 0.05);
        assert!(
            worst_error(&ends, &[0.0, 1.0, 1.7, 2.7]) < 0.005,
            "{ends:?}"
        );
    }

    #[test]
    fn a_round_column_keeps_its_place_and_size() {
        let mut column = CylinderSpec::column([2.0, 1.5, 1.0], 0.15, 0.1);
        column.spacing = 0.005;
        let scan = cylinder(&column).with_noise(Noise::Gaussian(0.002), 9);
        for square in [true, false] {
            let outline = trace(
                &plan_grid(&scan.points, 0.02),
                &OutlineOptions {
                    square,
                    ..options()
                },
            );
            assert_eq!(outline.regions.len(), 1);
            let region = &outline.regions[0];
            assert!(region.holes.is_empty());
            let truth = std::f64::consts::PI * 0.15 * 0.15;
            let off = region
                .outer
                .iter()
                .map(|at| (distance(*at, [2.0, 1.5]) - 0.15).abs())
                .fold(0.0, f64::max);
            // Straight segments for a circle of 0.30 m. Measured: the area
            // 5.0 % over when squared and 0.1 % under when not, and corners
            // up to 49 mm off the circle. Arcs are not drawn.
            assert!(
                (region.area() - truth).abs() < 0.1 * truth,
                "{}",
                region.area()
            );
            assert!(off < 0.065, "{off}");
        }
    }

    #[test]
    fn a_sparse_scan_is_traced_on_larger_cells() {
        // A point every 50 mm leaves two points in a cell of 20 mm, and in
        // one of 40 mm; a cell of 80 mm holds four.
        let room = box_room(&RoomSpec {
            spacing: 0.05,
            wall_thickness: Some(0.1),
            openings: vec![Opening::door(Wall::South, 1.0)],
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        });
        let grid = plan_grid(&room.points, 0.02);
        assert!(too_sparse(&grid, CUT_MIN_POINTS_PER_CELL));
        let outline = trace(&grid, &options());
        assert_eq!(outline.cell, 0.08);
        assert_eq!(outline.regions.len(), 1);
        assert!(outline.regions[0].holes.is_empty());
        let across = crossings(&outline.regions, 1, 1.5);
        let door = crossings(&outline.regions, 1, -0.05);
        // The accuracy falls with the cell: the wall is there and the door
        // is open, each within one cell of the truth. Measured 40 mm.
        assert!(
            worst_error(&across, &[-0.1, 0.0, 4.0, 4.1]) < 0.08,
            "{across:?}"
        );
        assert!(
            worst_error(&door, &[-0.1, 1.0, 1.9, 4.1]) < 0.08,
            "{door:?}"
        );

        // A scan that is dense enough is left on the cell that was asked.
        let dense = furnished_room();
        assert!(!too_sparse(
            &plan_grid(&dense.points, 0.02),
            CUT_MIN_POINTS_PER_CELL
        ));
    }

    #[test]
    fn vertical_section_fills_floors_and_walls_and_leaves_the_window_open() {
        let scan = section_shape();
        let slab = scan
            .points
            .iter()
            .filter(|point| point[1] >= 1.5 && point[1] <= 1.6)
            .map(|point| [point[0], point[2]]);
        let outline = trace(
            &grid_of(slab, 0.02),
            &OutlineOptions {
                fixed_direction: Some(0.0),
                ..options()
            },
        );
        // Floor, west wall, ceiling and the east wall below and above the
        // window are one piece, open at the window.
        assert_eq!(outline.regions.len(), 1);
        assert!(outline.regions[0].holes.is_empty());
        assert_eq!(outline.unfitted, 0);
        let lines = [
            (0, 2.0, [-0.25, 0.0, 2.6, 2.85]),
            (1, 0.5, [-0.1, 0.0, 4.0, 4.1]),
            (0, 4.05, [-0.25, 0.9, 2.1, 2.85]),
        ];
        let worst = lines
            .iter()
            .map(|(axis, level, truth)| {
                worst_error(&crossings(&outline.regions, *axis, *level), truth)
            })
            .fold(0.0, f64::max);
        // Floor thickness, room height, wall thickness and the window:
        // measured within 0.5 mm.
        assert!(worst < 0.002, "{worst}");
        // Through the window only the west wall is cut.
        let through = crossings(&outline.regions, 1, 1.5);
        assert!(worst_error(&through, &[-0.1, 0.0]) < 0.002, "{through:?}");
        assert!(worst_corner(&outline.regions) < 1e-9);
    }

    #[test]
    fn nothing_to_trace_gives_no_regions_and_a_cancelled_trace_stops() {
        // No points at all, and nothing but stray points.
        let frame = GridFrame::new([0.0, 0.0], 0.02, 50, 50).unwrap();
        let outline = trace(&CutGrid::new(frame), &options());
        assert!(outline.regions.is_empty());
        let stray = stray_points(
            Bounds {
                min: [0.0, 0.0, 1.0],
                max: [4.0, 3.0, 1.1],
            },
            200,
            3,
        );
        let outline = trace(&plan_grid(&stray.points, 0.02), &options());
        assert!(outline.regions.is_empty());
        assert_eq!(outline.cell, 0.08);

        // Asked before every step; stopped at any of them, nothing comes
        // back but the reason.
        let room = furnished_room();
        let grid = plan_grid(&room.points, 0.02);
        let mut asked = 0;
        trace_cut_regions(&grid, &options(), &mut || {
            asked += 1;
            Ok(())
        })
        .unwrap();
        assert!(asked > 10, "{asked}");
        for stop in [1, 2, 5, asked / 2, asked] {
            let mut calls = 0;
            let result = trace_cut_regions(&grid, &options(), &mut || {
                calls += 1;
                if calls == stop {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(result, Err(LoadError::Cancelled)), "{stop}");
            assert_eq!(calls, stop);
        }
        // Sizes that cannot be are refused.
        for broken in [
            OutlineOptions {
                max_wall_thickness: 0.0,
                ..options()
            },
            OutlineOptions {
                min_wall_length: f64::NAN,
                ..options()
            },
            OutlineOptions {
                fixed_direction: Some(f64::INFINITY),
                ..options()
            },
        ] {
            assert!(matches!(
                trace_cut_regions(&grid, &broken, &mut || Ok(())),
                Err(LoadError::InvalidData(_))
            ));
        }
    }

    /// Whether the rings of a region bound an area: the outer ring runs
    /// counter-clockwise, every hole clockwise and inside it, and no two
    /// edges cross. Every edge is held against every other one here, apart
    /// from the check the tracing itself makes.
    fn sound(region: &CutRegion) -> bool {
        let edges: Vec<[[f64; 2]; 2]> = std::iter::once(&region.outer)
            .chain(&region.holes)
            .flat_map(|ring| {
                (0..ring.len()).map(move |index| [ring[index], ring[(index + 1) % ring.len()]])
            })
            .collect();
        // On which side of an edge a position lies, and how far, in metres.
        let beside = |edge: &[[f64; 2]; 2], at: [f64; 2]| {
            cross(minus(edge[1], edge[0]), minus(at, edge[0])) / distance(edge[0], edge[1])
        };
        // Both ends of each edge clear of the other by more than a
        // thousandth of a millimetre, and on either side of it.
        let apart = |edge: &[[f64; 2]; 2], other: &[[f64; 2]; 2]| {
            let (a, b) = (beside(edge, other[0]), beside(edge, other[1]));
            a.abs() > 1e-6 && b.abs() > 1e-6 && (a > 0.0) != (b > 0.0)
        };
        let crossing = edges.iter().enumerate().any(|(index, edge)| {
            edges[index + 1..]
                .iter()
                .any(|other| apart(edge, other) && apart(other, edge))
        });
        ring_signed_area(&region.outer) > 0.0
            && region.outer.len() >= 3
            && region.holes.iter().all(|hole| {
                hole.len() >= 3
                    && ring_signed_area(hole) < 0.0
                    && ring_signed_area(hole).abs() < ring_signed_area(&region.outer)
            })
            && edges
                .iter()
                .flatten()
                .all(|at| at[0].is_finite() && at[1].is_finite())
            && !crossing
    }

    #[test]
    fn any_arrangement_of_walls_gives_regions_that_bound_an_area() {
        // Rooms, loose walls and columns thrown together at random, each at
        // its own direction, with noise, some of them through each other.
        let mut rng = Rng::new(77);
        let mut regions = 0;
        let mut unfitted = 0;
        for scene in 0..40 {
            let mut scan = Shape::default();
            for _ in 0..(2 + scene % 5) {
                let size = match rng.next_u64() % 3 {
                    0 => [1.0 + rng.unit() * 4.0, 1.0 + rng.unit() * 3.0, 2.6],
                    1 => [0.5 + rng.unit() * 4.0, 0.05 + rng.unit() * 0.5, 2.6],
                    _ => [0.1 + rng.unit() * 0.5, 0.1 + rng.unit() * 0.5, 2.6],
                };
                let part = box_room(&RoomSpec {
                    size,
                    spacing: 0.01,
                    wall_thickness: (rng.unit() < 0.5).then_some(0.05 + rng.unit() * 0.4),
                    openings: vec![Opening::door(Wall::South, rng.unit() * 2.0)],
                    slab: Some([1.0, 1.1]),
                    ..RoomSpec::default()
                })
                .with_noise(Noise::Gaussian(rng.unit() * 0.004), scene)
                .transformed(
                    rng.unit() * 360.0,
                    [rng.unit() * 6.0, rng.unit() * 6.0, 0.0],
                );
                scan = scan.merged(part);
            }
            let grid = plan_grid(&scan.points, 0.02);
            for square in [true, false] {
                let outline = trace(
                    &grid,
                    &OutlineOptions {
                        square,
                        ..options()
                    },
                );
                assert!(!outline.regions.is_empty(), "scene {scene}");
                for region in &outline.regions {
                    assert!(sound(region), "scene {scene}, square {square}: {region:?}");
                }
                regions += outline.regions.len();
                unfitted += outline.unfitted;
            }
        }
        // Measured: 236 regions, and none of them had to keep the outline
        // of its cells because its fitted edges crossed.
        assert!(regions >= 220, "{regions}");
        assert!(unfitted * 20 <= regions, "{unfitted} of {regions}");
    }

    /// The points a scanner leaves of a straight face from `from` to `to` in
    /// a slab: twenty every 5 mm along it, with 2 mm of noise across.
    fn face(rng: &mut Rng, from: [f64; 2], to: [f64; 2]) -> Vec<[f64; 2]> {
        let length = distance(from, to);
        let along = [(to[0] - from[0]) / length, (to[1] - from[1]) / length];
        let mut points = Vec::new();
        for column in 0..(length / 0.005).round() as usize {
            let at = (column as f64 + 0.5) * 0.005;
            for _ in 0..20 {
                let off = rng.gaussian() * 0.002;
                points.push([
                    from[0] + at * along[0] - off * along[1],
                    from[1] + at * along[1] + off * along[0],
                ]);
            }
        }
        points
    }

    /// The faces of a wall whose outline goes through `corners` and back to
    /// the first.
    fn faces(rng: &mut Rng, corners: &[[f64; 2]]) -> Vec<[f64; 2]> {
        (0..corners.len())
            .flat_map(|index| face(rng, corners[index], corners[(index + 1) % corners.len()]))
            .collect()
    }

    /// Points turned about the origin and then moved.
    fn placed(points: &[[f64; 2]], degrees: f64, moved: [f64; 2]) -> Vec<[f64; 2]> {
        let (sin, cos) = degrees.to_radians().sin_cos();
        points
            .iter()
            .map(|at| {
                [
                    moved[0] + cos * at[0] - sin * at[1],
                    moved[1] + sin * at[0] + cos * at[1],
                ]
            })
            .collect()
    }

    /// A wall of `thickness` along the u axis from zero, `first` long, that
    /// goes on for `second` at `degrees` to it, scanned on both faces, and
    /// across its start a wall of 4 m that gives the main direction. With
    /// the points comes where the two faces of the second part lie at a
    /// position along u: the lower and the upper.
    fn bent_wall(
        seed: u64,
        thickness: f64,
        (first, second): (f64, f64),
        degrees: f64,
    ) -> (Vec<[f64; 2]>, impl Fn(f64) -> [f64; 2]) {
        let mut rng = Rng::new(seed);
        let half = thickness * 0.5;
        let (sin, cos) = degrees.to_radians().sin_cos();
        let shift = half * (degrees.to_radians() * 0.5).tan();
        // Where the faces bend, and where they end.
        let (lower, upper) = ([first + shift, -half], [first - shift, half]);
        let end = |from: [f64; 2]| [from[0] + second * cos, from[1] + second * sin];
        let mut points = faces(
            &mut rng,
            &[
                [0.0, -half],
                lower,
                end(lower),
                end(upper),
                upper,
                [0.0, half],
            ],
        );
        points.extend(faces(
            &mut rng,
            &[[-0.2, -2.0], [0.0, -2.0], [0.0, 2.0], [-0.2, 2.0]],
        ));
        let slope = sin / cos;
        (points, move |u: f64| {
            [
                lower[1] + (u - lower[0]) * slope,
                upper[1] + (u - upper[0]) * slope,
            ]
        })
    }

    /// The largest distance, over positions along u, between the two faces
    /// of a wall as drawn and as they are.
    fn worst_face(regions: &[CutRegion], truth: &dyn Fn(f64) -> [f64; 2], at: &[f64]) -> f64 {
        at.iter()
            .map(|u| worst_error(&crossings(regions, 0, *u), &truth(*u)))
            .fold(0.0, f64::max)
    }

    /// Both ways of tracing: squared and as measured.
    fn both_ways() -> [OutlineOptions; 2] {
        [
            options(),
            OutlineOptions {
                square: false,
                ..options()
            },
        ]
    }

    #[test]
    fn a_face_that_bends_slightly_is_two_edges_that_meet_in_the_bend() {
        // Per wall: its thickness, the lengths of its two parts, the bend
        // in degrees, and where along it the second part is measured.
        let walls = [
            (0.2, (6.0, 5.0), 0.9, [6.5, 7.5, 9.0, 10.5]),
            (0.1, (10.0, 9.0), 1.1, [10.5, 11.0, 14.5, 18.5]),
            (0.1, (5.0, 4.0), 4.0, [5.2, 5.5, 7.0, 8.7]),
        ];
        for (thickness, (first, second), degrees, stations) in walls {
            let (points, truth) = bent_wall(31, thickness, (first, second), degrees);
            let grid = grid_of(points.iter().copied(), 0.02);
            for options in both_ways() {
                let outline = trace(&grid, &options);
                assert_eq!(outline.direction_degrees, 0.0);
                assert_eq!((outline.regions.len(), outline.unfitted), (1, 0));
                // The first part lies along the main direction, the second
                // does not: 5 m at 0.9 degrees is 79 mm at the far end,
                // more than squaring may take. Measured within 0.8 mm on
                // the second part and 0.1 mm on the first, squared or not.
                // Drawn on one line with the first part, the second was
                // 74 mm off, and with a step where the outline kept its
                // corner 19 mm.
                let turned = worst_face(&outline.regions, &truth, &stations);
                let straight = worst_face(
                    &outline.regions,
                    &|_| [-thickness * 0.5, thickness * 0.5],
                    &[1.0, first * 0.5, first - 0.3],
                );
                assert!(turned < 0.002, "{degrees}: {turned}");
                assert!(straight < 0.002, "{degrees}: {straight}");
            }
        }

        // The same in a vertical view, where the direction is given: a
        // level floor of 0.25 m that goes over into a fall of 1.6 %.
        let (points, truth) = bent_wall(47, 0.25, (6.0, 5.0), 0.9);
        let grid = grid_of(points.iter().copied(), 0.02);
        for square in [true, false] {
            let outline = trace(
                &grid,
                &OutlineOptions {
                    square,
                    fixed_direction: Some(0.0),
                    ..options()
                },
            );
            // Measured within 0.1 mm.
            let fall = worst_face(&outline.regions, &truth, &[6.5, 7.5, 10.5]);
            assert!(fall < 0.002, "{fall}");
        }
    }

    #[test]
    fn a_step_in_a_face_is_drawn_where_it_is_and_a_shallow_bump_is_levelled() {
        // A wall of 6.0 by 0.2 m whose lower face goes through `lower`,
        // with the wall across its start that gives the main direction.
        let wall = |seed: u64, lower: &[[f64; 2]]| -> CutGrid {
            let mut rng = Rng::new(seed);
            let mut corners = lower.to_vec();
            corners.extend([[6.0, 0.2], [0.0, 0.2]]);
            let mut points = faces(&mut rng, &corners);
            points.extend(faces(
                &mut rng,
                &[[-0.2, -2.0], [0.0, -2.0], [0.0, 2.0], [-0.2, 2.0]],
            ));
            grid_of(points.iter().copied(), 0.02)
        };
        // How far the lower face is drawn from where it is, at its worst
        // over positions along the wall.
        let worst = |grid: &CutGrid, options: &OutlineOptions, truth: &dyn Fn(f64) -> f64| {
            let outline = trace(grid, options);
            [0.5, 1.5, 2.3, 2.7, 3.2, 4.0, 5.5]
                .iter()
                .map(|u| (crossings(&outline.regions, 0, *u)[0] - truth(*u)).abs())
                .fold(0.0, f64::max)
        };
        // The face steps back at 2.5 m, and may turn there as well. The
        // step is less than the cell and a half the outline is reduced by,
        // so the outline shows no corner; the points of the face do.
        for (step, degrees) in [(0.015, 0.0), (0.025, 0.0), (0.025, 1.5), (0.025, -1.5)] {
            let slope = f64::to_radians(degrees).tan();
            let grid = wall(
                41,
                &[
                    [0.0, 0.0],
                    [2.5, 0.0],
                    [2.5, -step],
                    [6.0, -step + 3.5 * slope],
                ],
            );
            let truth = |u: f64| {
                if u < 2.5 {
                    0.0
                } else {
                    -step + (u - 2.5) * slope
                }
            };
            for options in both_ways() {
                // Measured within 0.1 mm. One line through the whole face
                // was 15 mm off over the first 2.5 m when squared and 8 mm
                // when not.
                let off = worst(&grid, &options, &truth);
                assert!(off < 0.002, "{step} {degrees}: {off}");
            }
        }

        // A pilaster of 0.4 m: one of 20 mm stays within the cell and a
        // half and returns to the face, and is not drawn; one of 40 mm is.
        for (depth, drawn) in [(0.02, false), (0.04, true)] {
            let grid = wall(
                45,
                &[
                    [0.0, 0.0],
                    [2.5, 0.0],
                    [2.5, -depth],
                    [2.9, -depth],
                    [2.9, 0.0],
                    [6.0, 0.0],
                ],
            );
            for options in both_ways() {
                let outline = trace(&grid, &options);
                let face = |u: f64| crossings(&outline.regions, 0, u)[0];
                // Measured 0.0 mm beside the pilaster, and on it 0.1 mm
                // where it is drawn.
                assert!(face(1.0).abs() < 0.002 && face(5.0).abs() < 0.002);
                let on_it = face(2.7);
                if drawn {
                    assert!((on_it + depth).abs() < 0.002, "{depth}: {on_it}");
                } else {
                    assert!(on_it.abs() < 0.002, "{depth}: {on_it}");
                }
            }
        }
    }

    /// A closed room of 4.0 by 3.0 m inside, with walls of 0.2 m scanned on
    /// both faces.
    fn closed_room(size: [f64; 2], seed: u64) -> Shape {
        box_room(&RoomSpec {
            size: [size[0], size[1], 2.6],
            spacing: 0.005,
            wall_thickness: Some(0.2),
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        })
        .with_noise(Noise::Gaussian(0.002), seed)
    }

    #[test]
    fn a_building_that_stands_apart_is_traced_in_its_own_direction() {
        // A wing of 12 by 5 m along the axes, and beside it a room that
        // does not follow them.
        let wing = closed_room([12.0, 5.0], 51);
        let room = closed_room([4.0, 3.0], 52);
        // The holes of the regions that lie where the room is, as the room
        // was before it was turned and moved.
        let room_holes = |regions: &[CutRegion], degrees: f64, moved: [f64; 2]| {
            turned_back(regions, degrees, moved)
                .into_iter()
                .flat_map(|region| region.holes)
                .filter(|hole| {
                    hole.iter()
                        .all(|at| at[0] > -0.5 && at[0] < 4.5 && at[1] > -0.5 && at[1] < 3.5)
                })
                .collect::<Vec<_>>()
        };
        for degrees in [8.0, 25.0, 45.0] {
            let moved = [16.0, 1.0];
            let scan = wing
                .clone()
                .merged(room.clone().transformed(degrees, [moved[0], moved[1], 0.0]));
            let grid = plan_grid(&scan.points, 0.02);
            for options in both_ways() {
                let outline = trace(&grid, &options);
                // The wing gives the main direction.
                assert_eq!(outline.direction_degrees, 0.0);
                assert_eq!((outline.regions.len(), outline.unfitted), (2, 0));
                let holes = room_holes(&outline.regions, degrees, moved);
                assert_eq!(holes.len(), 1, "{degrees}");
                // The room keeps its four corners and its 12 m2: measured
                // 11.9996 to 12.0009 m2. Closed along the direction of
                // the wing, every corner was cut off over up to 0.50 m
                // along a wall, and the room came to 11.80 m2 at 25
                // degrees and 11.68 m2 at 45.
                let area = ring_signed_area(&holes[0]).abs();
                assert_eq!(holes[0].len(), 4, "{degrees}");
                assert!((area - 12.0).abs() < 0.005, "{degrees}: {area}");
                // Measured along a line 50 mm from the south wall the room
                // is 4.000 m wide: within 0.2 mm.
                let back = turned_back(&outline.regions, degrees, moved);
                let across: Vec<f64> = crossings(&back, 1, 0.05)
                    .into_iter()
                    .filter(|u| *u > -1.0)
                    .collect();
                let width = worst_error(&across, &[-0.2, 0.0, 4.0, 4.2]);
                assert!(width < 0.002, "{degrees}: {width}");
            }
        }

        // The limit: a room that joins the wing, here 0.3 m from it, is one
        // part with it and is closed along the direction of the wing. Its
        // inside corners stay filled: measured 0.21 to 0.48 m along the
        // walls, 8 corners and 11.80 m2.
        let moved = [14.03, 1.0];
        let scan = wing.merged(room.transformed(25.0, [moved[0], moved[1], 0.0]));
        let outline = trace(&plan_grid(&scan.points, 0.02), &options());
        assert_eq!(outline.regions.len(), 1);
        let holes = room_holes(&outline.regions, 25.0, moved);
        assert_eq!(holes.len(), 1);
        let area = ring_signed_area(&holes[0]).abs();
        assert_eq!(holes[0].len(), 8);
        assert!(area > 11.7 && area < 11.9, "{area}");
    }

    /// The largest distance from a true corner to the nearest drawn corner.
    fn corner_error(drawn: &[[f64; 2]], truth: &[[f64; 2]]) -> f64 {
        truth
            .iter()
            .map(|corner| {
                drawn
                    .iter()
                    .map(|at| distance(*at, *corner))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    }

    #[test]
    fn a_wall_off_the_main_direction_keeps_its_corners() {
        // The room gives the main direction; beside it a wall of 2.0 by
        // 0.2 m scanned all round.
        let room = furnished_room();
        let wall = box_room(&RoomSpec {
            size: [2.0, 0.2, 2.6],
            spacing: 0.005,
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        })
        .with_noise(Noise::Gaussian(0.002), 61);
        let truth = [[0.0, 0.0], [2.0, 0.0], [2.0, 0.2], [0.0, 0.2]];
        // The corners drawn within reach of the far half of the wall, as
        // the wall was before it was turned and moved.
        let far_corners = |outline: &CutOutline, degrees: f64, moved: [f64; 2]| {
            turned_back(&outline.regions, degrees, moved)
                .iter()
                .flat_map(|region| region.outer.clone())
                .filter(|at| at[0] > 0.8 && at[0] < 2.3 && at[1] > -0.3 && at[1] < 0.5)
                .collect::<Vec<_>>()
        };
        for degrees in [10.0, 20.0, 30.0, 45.0] {
            // On its own, 1.9 m from the room.
            let moved = [0.0, 5.0];
            let scan = room
                .clone()
                .merged(wall.clone().transformed(degrees, [moved[0], moved[1], 0.0]));
            let grid = plan_grid(&scan.points, 0.02);
            for options in both_ways() {
                let outline = trace(&grid, &options);
                assert_eq!(outline.direction_degrees, 0.0);
                assert_eq!(outline.regions.len(), 4);
                let drawn: Vec<CutRegion> = turned_back(&outline.regions, degrees, moved)
                    .into_iter()
                    .filter(|region| region.outer.iter().all(|at| at[1].abs() < 0.5))
                    .collect();
                assert_eq!(drawn.len(), 1, "{degrees}");
                // Four corners, each where it is: measured within 1.4 mm.
                // On the grid of the room its ends were skewed, bevelled
                // or notched, with corners 27 to 49 mm off.
                let off = corner_error(&drawn[0].outer, &truth);
                assert_eq!(drawn[0].outer.len(), 4, "{degrees}");
                assert!(off < 0.004, "{degrees}: {off}");
            }
        }
        for (degrees, limit) in [(30.0, 0.005), (45.0, 0.020)] {
            // Built onto the north wall of the room: one part with it,
            // traced along the direction of the room. Its far end follows
            // the wall, not the room: measured 0.8 mm at 30 degrees and
            // 12.8 mm at 45, where it was 31 and 24 mm when squared.
            let moved = [1.0, 3.05];
            let scan = room
                .clone()
                .merged(wall.clone().transformed(degrees, [moved[0], moved[1], 0.0]));
            let grid = plan_grid(&scan.points, 0.02);
            for options in both_ways() {
                let outline = trace(&grid, &options);
                assert_eq!(outline.regions.len(), 3);
                let off = corner_error(&far_corners(&outline, degrees, moved), &truth[1..3]);
                assert!(off < limit, "{degrees}: {off}");
                // Its long faces lie on their points: within 0.6 mm.
                let back = turned_back(&outline.regions, degrees, moved);
                let faces: Vec<f64> = crossings(&back, 0, 1.5)
                    .into_iter()
                    .filter(|v| *v > -0.1 && *v < 0.3)
                    .collect();
                let off = worst_error(&faces, &[0.0, 0.2]);
                assert!(off < 0.002, "{degrees}: {off}");
            }
        }
    }

    /// The strips that lie along the u axis after the regions were brought
    /// back to where a face was before it was turned and moved.
    fn strips_along(regions: &[CutRegion], degrees: f64, moved: [f64; 2]) -> Vec<CutRegion> {
        turned_back(regions, degrees, moved)
            .into_iter()
            .filter(|region| region.outer.iter().all(|at| at[1].abs() < 0.3))
            .collect()
    }

    #[test]
    fn a_face_scanned_from_one_side_is_a_strip_wherever_it_lies_on_the_grid() {
        let scan_of = |strip: &[[f64; 2]], degrees: f64, moved: [f64; 2]| -> CutGrid {
            let mut scan: Vec<[f64; 2]> = furnished_room()
                .points
                .iter()
                .map(|point| [point[0], point[1]])
                .collect();
            scan.extend(placed(strip, degrees, moved));
            grid_of(scan.iter().copied(), 0.02)
        };
        // Beside the room, at eight places across a cell of its grid and
        // at up to 25 degrees to it. A face on the border between two rows
        // of cells fills both, and one that is a degree off crosses from
        // row to row.
        for degrees in [0.0, 1.0, 10.0, 21.0] {
            for position in 0..8 {
                let moved = [0.0, 5.0 + position as f64 * 0.0025];
                let mut rng = Rng::new(80 + position);
                let strip = face(&mut rng, [0.0, 0.0], [2.0, 0.0]);
                let grid = scan_of(&strip, degrees, moved);
                for options in both_ways() {
                    let outline = trace(&grid, &options);
                    assert_eq!(outline.regions.len(), 4);
                    let strips = strips_along(&outline.regions, degrees, moved);
                    assert_eq!(strips.len(), 1, "{degrees} at {position}");
                    let corners = &strips[0].outer;
                    // A rectangle of one cell by the length of the face.
                    // Before, the strip of two rows was drawn as a
                    // triangle or a sliver at 1 of the 8 places at 0
                    // degrees and at 7 of them at 1 degree.
                    assert_eq!(corners.len(), 4, "{degrees} at {position}");
                    let area = ring_signed_area(corners);
                    assert!(
                        (area - 0.04).abs() < 0.001,
                        "{degrees} at {position}: {area}"
                    );
                    // As measured it is centred on the face: both edges
                    // 10 mm from it, within 0.2 mm, and its ends on the
                    // cells, within 8 mm of the true ends. Squared, a face
                    // a degree off is turned about its middle, 17 mm at
                    // either end.
                    let turned = options.square && degrees == 1.0;
                    let reach = if turned { 0.030 } else { 0.0105 };
                    assert!(
                        corners
                            .iter()
                            .all(|at| at[1].abs() < reach && at[1].abs() > 0.006),
                        "{degrees} at {position}: {corners:?}"
                    );
                    let ends = corners
                        .iter()
                        .map(|at| at[0].min((at[0] - 2.0).abs()).abs())
                        .fold(0.0, f64::max);
                    assert!(ends < 0.012, "{degrees} at {position}: {ends}");
                }
            }
        }
    }

    #[test]
    fn a_short_face_scanned_from_one_side_is_drawn_from_the_smallest_wall_length() {
        // A face is a row of single cells, so the area of its strip says
        // nothing about the wall: 0.30 m of length is what counts. At eight
        // places across a cell.
        for (length, drawn) in [(0.25, false), (0.4, true), (0.7, true)] {
            for position in 0..8 {
                let level = 1.0 + position as f64 * 0.0025;
                let mut rng = Rng::new(70 + position);
                let points = face(&mut rng, [0.3, level], [0.3 + length, level]);
                let outline = trace(&grid_of(points.iter().copied(), 0.02), &options());
                if !drawn {
                    assert!(outline.regions.is_empty(), "{length} at {position}");
                    assert_eq!(outline.dropped, 1);
                    continue;
                }
                // By the area of its strip, every face under 0.76 m was
                // dropped.
                assert_eq!(outline.regions.len(), 1, "{length} at {position}");
                assert_eq!(outline.dropped, 0);
                let across = crossings(&outline.regions, 0, 0.3 + length * 0.5);
                let along = crossings(&outline.regions, 1, (across[0] + across[1]) * 0.5);
                // One cell thick and centred on the face: measured
                // 20.000 mm and within 0.15 mm, its ends within 10 mm.
                assert!((across[1] - across[0] - 0.02).abs() < 1e-6, "{across:?}");
                assert!(
                    ((across[0] + across[1]) * 0.5 - level).abs() < 0.0005,
                    "{across:?}"
                );
                assert!(
                    worst_error(&along, &[0.3, 0.3 + length]) < 0.015,
                    "{along:?}"
                );
            }
        }
        // What is small in every direction still goes by its area: a
        // column of 0.15 m is drawn, one of 0.08 m is not.
        for (size, regions) in [(0.15, 1), (0.08, 0)] {
            let column = box_room(&RoomSpec {
                size: [size, size, 2.6],
                spacing: 0.005,
                slab: Some([1.0, 1.1]),
                ..RoomSpec::default()
            })
            .with_noise(Noise::Gaussian(0.002), 71);
            let outline = trace(&plan_grid(&column.points, 0.02), &options());
            assert_eq!(outline.regions.len(), regions, "{size}");
        }
    }

    #[test]
    fn the_closing_is_bounded_by_the_wall_thickness_and_by_the_grid() {
        let room = furnished_room();
        let grid = plan_grid(&room.points, 0.02);
        let refused = |grid: &CutGrid, options: OutlineOptions| {
            matches!(
                trace_cut_regions(grid, &options, &mut || panic!("nothing is traced")),
                Err(LoadError::InvalidData(_))
            )
        };
        // Half a metre typed as millimetres: the closing would work on a
        // grid 25,000 cells wider on every side.
        assert!(refused(
            &grid,
            OutlineOptions {
                max_wall_thickness: 500.0,
                ..options()
            }
        ));
        assert!(refused(
            &grid,
            OutlineOptions {
                max_wall_thickness: MAX_WALL_THICKNESS + 0.1,
                ..options()
            }
        ));
        // A grid of 1 mm makes half a metre 500 cells.
        let fine = CutGrid::new(GridFrame::new([0.0, 0.0], 0.001, 100, 100).unwrap());
        assert!(refused(&fine, options()));
        // The largest thickness on the grid of the request is traced: all
        // openings are closed.
        let widest = trace(
            &grid,
            &OutlineOptions {
                max_wall_thickness: MAX_WALL_THICKNESS,
                ..options()
            },
        );
        assert_eq!(widest.regions.len(), 1);

        // A square wider than the grid closes nothing more, so the radius
        // is kept to half the larger side.
        let frame = GridFrame::new([0.0, 0.0], 0.02, 40, 25).unwrap();
        let mut rng = Rng::new(3);
        let dotted = Mask::from_fn(frame, |_, _| rng.unit() < 0.1);
        let mut bounded = dotted.clone();
        bounded.close(20);
        let mut wide = dotted.clone();
        wide.close(400);
        assert!(bounded.count() > dotted.count());
        assert_eq!(bounded, wide);
    }

    #[test]
    fn main_direction_is_found_to_a_hundredth_of_a_degree() {
        let room = box_room(&RoomSpec {
            spacing: 0.005,
            wall_thickness: Some(0.1),
            slab: Some([1.0, 1.1]),
            ..RoomSpec::default()
        })
        .with_noise(Noise::Gaussian(0.002), 17);
        let mut worst = 0.0f64;
        for degrees in [
            0.0, 0.1234, 3.0, 12.3456, 22.5, 33.3333, 45.0, -12.2567, 80.0,
        ] {
            let turned = room.clone().transformed(degrees, [0.0; 3]);
            let outline = trace(&plan_grid(&turned.points, 0.02), &options());
            worst = worst.max(quarter_turn(outline.direction_degrees - degrees).abs());
        }
        // The search ends in steps of five thousandths of a degree:
        // measured within 0.0017 degree.
        assert!(worst < 0.005, "{worst}");
        assert_eq!(quarter_turn(90.0), 0.0);
        assert_eq!(quarter_turn(50.0), -40.0);
        assert_eq!(quarter_turn(-50.0), 40.0);
        assert_eq!(quarter_turn(45.0), 45.0);
    }
}
