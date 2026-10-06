//! The cut plane and the slab of a section box, and the one pass over the
//! scans that collects what a drawing of that slab needs: the points thinned
//! to a drawable number, and the count of points per cell of the cut plane
//! that the filled cut is traced from.
//!
//! Nothing is kept per point that was read. What the pass holds is bounded by
//! the point limit of the drawing and by the cell limit of the grid, whatever
//! the size of the scans.

use std::borrow::Cow;
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use super::outline::{CUT_MIN_POINTS_PER_CELL, MAX_COARSENINGS};
use super::{DrawingFrame, DrawingOrigin, DrawingView};
use crate::grid2d::{CountGrid, GridFrame};
use crate::region_source::{
    overlaps, visit_region, RegionFilter, RegionProgress, RegionReader, RegionSource,
};
use crate::surface_mesh::sampled_ordinal;
use crate::{bounds_corners, Bounds, IndexedPoint, LoadError, OrientedBox, Point};

/// The most cells the grid of a filled cut gets: 20 bytes each while the
/// points are read, and about 15 more while the cut is traced (measured:
/// 540 MB in all for 15.4 million cells). A storey of 15 by 12 m at 20 mm is
/// 450,000 cells; an extent that would need more than the limit gets a
/// larger cell.
pub const MAX_CUT_GRID_CELLS: usize = 16_000_000;

/// How often the slab is read again to lay the count grid closer around its
/// points, after the bounds of the layers made its cells larger than asked.
const MAX_GRID_REREADS: usize = 2;

/// The part of a section box that one view draws.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slab {
    pub view: DrawingView,
    /// The axis-aligned box around the slab in scene coordinates: the slab
    /// itself when the section box is not turned.
    pub bounds: Bounds,
    /// The slab, from the cut plane into the box, turned as the section box
    /// is.
    pub region: OrientedBox,
    /// Depth of the slab behind the cut plane, after it was kept within the
    /// box.
    pub thickness: f64,
    /// The cut plane as the coordinate system of the drawing.
    pub frame: DrawingFrame,
    /// Lower left and upper right corner of the box as the view sees it, in
    /// drawing coordinates.
    pub extent: [[f64; 2]; 2],
}

/// The slab behind the face of a section box that a view looks at.
///
/// The cut plane is that face: the top for a plan, and for a vertical view
/// the side the viewer stands at. `thickness` is how deep the slab goes into
/// the box, no deeper than the box itself; `None` takes the whole box, as an
/// elevation does. The drawing is at scale 1:1 with `u` to the right and `v`
/// up as the viewer sees it; `origin` says where its zero lies.
///
/// A box turned about the vertical takes its faces along: the views look
/// along its own axes, so that a box turned to follow the walls gives a plan
/// with walls along `u` and `v` and sections parallel to a wall. A plan of a
/// turned box with the model origin has the model X and Y turned with it,
/// about the model origin.
pub fn slab_from_section(
    section: impl Into<OrientedBox>,
    view: DrawingView,
    thickness: Option<f64>,
    origin: DrawingOrigin,
) -> Result<Slab, LoadError> {
    let invalid = |reason: &str| Err(LoadError::InvalidData(reason.into()));
    let turned: OrientedBox = section.into();
    if !turned.is_valid() {
        return invalid("the section box is not a box");
    }
    // Everything below is in the frame of the box before it was turned, and
    // is turned into the scene at the end.
    let section = turned.bounds;
    let depth_axis = view.depth_axis();
    if (0..3).any(|axis| axis != depth_axis && section.max[axis] <= section.min[axis]) {
        return invalid("the section box has no size in this view");
    }
    let depth = section.max[depth_axis] - section.min[depth_axis];
    let thickness = match thickness {
        Some(thickness) if !thickness.is_finite() || thickness <= 0.0 => {
            return invalid("slab thickness must be above zero");
        }
        Some(thickness) => thickness.min(depth),
        None => depth,
    };

    // Per view: the direction to the right as seen, whether the cut plane is
    // the face at the maximum of the depth axis, and the corner of that face
    // that is at the lower left as seen.
    let (min, max) = (section.min, section.max);
    let (right, face_at_max, corner) = match view {
        DrawingView::Plan => ([1.0, 0.0, 0.0], true, [min[0], min[1], max[2]]),
        DrawingView::Front => ([1.0, 0.0, 0.0], false, [min[0], min[1], min[2]]),
        DrawingView::Back => ([-1.0, 0.0, 0.0], true, [max[0], max[1], min[2]]),
        DrawingView::Left => ([0.0, -1.0, 0.0], false, [min[0], max[1], min[2]]),
        DrawingView::Right => ([0.0, 1.0, 0.0], true, [max[0], min[1], min[2]]),
    };
    let up = match view {
        DrawingView::Plan => [0.0, 1.0, 0.0],
        _ => [0.0, 0.0, 1.0],
    };
    let mut bounds = section;
    if face_at_max {
        bounds.min[depth_axis] = max[depth_axis] - thickness;
    } else {
        bounds.max[depth_axis] = min[depth_axis] + thickness;
    }
    // Into the scene: the directions follow the axes of the box, and the
    // corner is where the turn puts it.
    let [along, across] = turned.axes();
    let into_scene = |direction: [f64; 3]| -> [f64; 3] {
        std::array::from_fn(|axis| {
            direction[0] * along[axis] + direction[1] * across[axis] + direction[2] * up_axis(axis)
        })
    };
    let (right_local, up_local) = (right, up);
    let (right, up) = (into_scene(right_local), into_scene(up_local));
    let corner_local = corner;
    let corner = turned.to_scene(corner_local);
    let region = turned.part(bounds);
    let zero = match (origin, view) {
        (DrawingOrigin::BoxCorner, _) => corner,
        // A plan keeps model X and Y.
        (DrawingOrigin::Model, DrawingView::Plan) => [0.0, 0.0, corner[2]],
        // A vertical view keeps model Z, so that levels read as heights.
        (DrawingOrigin::Model, _) => [corner[0], corner[1], 0.0],
    };
    let frame = DrawingFrame {
        right,
        up,
        origin: zero,
    };
    let lower_left = frame.to_uv(corner);
    let side = |along: [f64; 3]| -> f64 {
        (0..3)
            .map(|axis| (max[axis] - min[axis]) * along[axis].abs())
            .sum()
    };
    Ok(Slab {
        view,
        bounds: region.aabb(),
        region,
        thickness,
        frame,
        extent: [
            lower_left,
            [
                lower_left[0] + side(right_local),
                lower_left[1] + side(up_local),
            ],
        ],
    })
}

/// The vertical unit vector, one axis at a time.
fn up_axis(axis: usize) -> f64 {
    if axis == 2 {
        1.0
    } else {
        0.0
    }
}

/// A point of the slab that is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlabPoint {
    /// Position on the cut plane, in metres.
    pub uv: [f64; 2],
    pub rgb: Option<[u8; 3]>,
    pub classification: Option<u8>,
    /// Position of its scan in the list of sources.
    pub source: u32,
}

/// What to collect from the slab.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlabOptions {
    /// The points are thinned to one per cell of this size; `None` collects
    /// no points.
    pub point_spacing: Option<f64>,
    /// When thinning leaves more points than this, the spacing doubles.
    pub max_points: usize,
    /// Cell of the count grid for the filled cut; `None` builds no grid.
    pub grid: Option<f64>,
    /// The share of the points of the scans that is thinned to the points
    /// drawn, in percent: below 100 the same points whatever the slab,
    /// chosen by their ordinal in the source file.
    pub sample_percent: f64,
    /// The share of the points that is counted on the grid of the filled
    /// cut, in the same way. A sparse scan needs every point for its walls.
    pub grid_percent: f64,
}

impl SlabOptions {
    /// The share of the points that is read: the larger of the shares of
    /// what is collected.
    pub fn read_percent(&self) -> f64 {
        let points = self.point_spacing.map(|_| self.sample_percent);
        let grid = self.grid.map(|_| self.grid_percent);
        match (points, grid) {
            (Some(points), Some(grid)) => points.max(grid),
            (points, grid) => points.or(grid).unwrap_or(self.sample_percent),
        }
    }
}

/// What one pass over the slab collected.
#[derive(Debug, Clone, PartialEq)]
pub struct SlabCut {
    /// The thinned points, row by row from the bottom of the view.
    pub points: Vec<SlabPoint>,
    /// The spacing the points were thinned to.
    pub spacing: f64,
    pub grid: Option<CutGrid>,
    /// Points in the slab that the filter accepted.
    pub slab_points: u64,
    /// Points read to find them: those of the octree leaves that touch the
    /// slab, and all points of a layer without an index. Counted again for
    /// every time the slab was read once more to lay the grid closer.
    pub read_points: u64,
    /// Points taken from memory, kept from an earlier read, instead of
    /// being read again.
    pub reused_points: u64,
}

/// Points per cell of the cut plane, and where in its cell they lie on
/// average. A scanned face crosses a cell anywhere; the mean position of its
/// points gives the face back to a fraction of the cell.
#[derive(Debug, Clone, PartialEq)]
pub struct CutGrid {
    counts: CountGrid,
    /// Per cell, the sum of the offsets of its points from the lower left
    /// corner of the cell. In double precision, so that the mean stays true
    /// for as many points as the count holds: a sum in single precision
    /// takes in less and less of every offset once a cell holds millions of
    /// points, and the mean of twenty million is a millimetre off.
    sum_u: Vec<f64>,
    sum_v: Vec<f64>,
}

impl CutGrid {
    /// Bytes a cell takes: its count and the two sums.
    pub(super) const CELL_BYTES: usize = 20;

    pub fn new(frame: GridFrame) -> Self {
        Self {
            counts: CountGrid::new(frame),
            sum_u: vec![0.0; frame.cells()],
            sum_v: vec![0.0; frame.cells()],
        }
    }

    pub fn frame(&self) -> GridFrame {
        self.counts.frame()
    }

    pub fn counts(&self) -> &CountGrid {
        &self.counts
    }

    /// Count a point; one outside the grid is left out.
    pub fn add(&mut self, uv: [f64; 2]) {
        let frame = self.counts.frame();
        if let Some(index) = self.counts.add(uv) {
            let x = (index % frame.width as usize) as f64;
            let y = (index / frame.width as usize) as f64;
            self.sum_u[index] += uv[0] - frame.origin[0] - x * frame.cell;
            self.sum_v[index] += uv[1] - frame.origin[1] - y * frame.cell;
        }
    }

    /// Add the counts and sums of another grid on the same frame.
    fn absorb(&mut self, other: &Self) {
        debug_assert_eq!(self.frame(), other.frame());
        self.counts.absorb(&other.counts);
        for (sum, more) in self.sum_u.iter_mut().zip(&other.sum_u) {
            *sum += more;
        }
        for (sum, more) in self.sum_v.iter_mut().zip(&other.sum_v) {
            *sum += more;
        }
    }

    /// The mean position of the points in a cell; nothing for an empty cell.
    pub fn centroid(&self, x: u32, y: u32) -> Option<[f64; 2]> {
        let frame = self.counts.frame();
        let index = frame.index(x, y);
        let count = self.counts.counts()[index];
        (count > 0).then(|| {
            [
                frame.origin[0] + x as f64 * frame.cell + self.sum_u[index] / count as f64,
                frame.origin[1] + y as f64 * frame.cell + self.sum_v[index] / count as f64,
            ]
        })
    }

    /// The same points in cells of twice the size, for a cloud too sparse
    /// for the cell that was asked.
    pub fn coarsened(&self) -> Self {
        let fine = self.counts.frame();
        let counts = self.counts.coarsened();
        let frame = counts.frame();
        let mut sum_u = vec![0.0f64; frame.cells()];
        let mut sum_v = vec![0.0f64; frame.cells()];
        for y in 0..fine.height {
            for x in 0..fine.width {
                let from = fine.index(x, y);
                let count = self.counts.counts()[from] as f64;
                if count == 0.0 {
                    continue;
                }
                // The offsets now count from the corner of the larger cell.
                let to = frame.index(x / 2, y / 2);
                let shift = |odd: u32| count * (odd % 2) as f64 * fine.cell;
                sum_u[to] += self.sum_u[from] + shift(x);
                sum_v[to] += self.sum_v[from] + shift(y);
            }
        }
        Self {
            counts,
            sum_u,
            sum_v,
        }
    }

    /// The rectangle around the cells that hold at least `min_count` points,
    /// one cell wider on every side, or nothing when no cell does. The cells
    /// are counted at a size of `cell` or larger, so that the rectangle
    /// holds every cell of that size, or of a halving of it, that reaches
    /// the count in a grid laid over the rectangle.
    pub fn counted_extent(&self, min_count: u32, cell: f64) -> Option<[[f64; 2]; 2]> {
        let mut counts = Cow::Borrowed(&self.counts);
        while counts.frame().cell < cell {
            counts = Cow::Owned(counts.coarsened());
        }
        let frame = counts.frame();
        let (mut low, mut high) = ([u32::MAX; 2], [0u32; 2]);
        for (index, count) in counts.counts().iter().enumerate() {
            if *count >= min_count {
                let at = [
                    (index % frame.width as usize) as u32,
                    (index / frame.width as usize) as u32,
                ];
                for axis in 0..2 {
                    low[axis] = low[axis].min(at[axis]);
                    high[axis] = high[axis].max(at[axis]);
                }
            }
        }
        (low[0] <= high[0]).then(|| {
            [
                frame.vertex([low[0] as i32 - 1, low[1] as i32 - 1]),
                frame.vertex([high[0] as i32 + 2, high[1] as i32 + 2]),
            ]
        })
    }
}

/// A hash for cell numbers. Two multiplications spread them over the table;
/// the default hasher costs more than the rest of the work per point.
#[derive(Default)]
pub(super) struct CellHasher(u64);

impl Hasher for CellHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.write_u64(*byte as u64);
        }
    }

    fn write_u64(&mut self, value: u64) {
        let mixed = (self.0 ^ value).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = (mixed ^ (mixed >> 32)).wrapping_mul(0xd6e8_feb8_6659_fd93);
        self.0 ^= self.0 >> 32;
    }
}

pub(super) type CellMap<V> = HashMap<u64, V, BuildHasherDefault<CellHasher>>;

/// The point that stands for its cell, with what decides between two points
/// of one cell.
#[derive(Debug, Clone, Copy)]
struct Kept {
    point: SlabPoint,
    /// Squared distance to the centre of the cell of the spacing that was
    /// asked, in cells.
    off_centre: f64,
    ordinal: u64,
}

impl Kept {
    /// Whether this point stands for a cell rather than the other. The
    /// order does not depend on the cell size or on which point came first,
    /// so the thinned points are the same in whatever order the scans are
    /// read, also after the spacing doubled halfway.
    fn before(&self, other: &Self) -> bool {
        self.off_centre
            .total_cmp(&other.off_centre)
            .then(self.point.source.cmp(&other.point.source))
            .then(self.ordinal.cmp(&other.ordinal))
            .is_lt()
    }
}

/// One point per cell of the cut plane: of the points in a cell, the one
/// nearest the centre of its cell at the spacing that was asked. At that
/// spacing the points form an even pattern. When more cells fill than the
/// drawing may hold points, cells are joined four by four.
#[derive(Debug, Clone)]
struct Thinning {
    anchor: [f64; 2],
    spacing: f64,
    /// How often the cell size doubled.
    level: u32,
    max_points: usize,
    cells: CellMap<Kept>,
}

impl Thinning {
    fn new(anchor: [f64; 2], spacing: f64, max_points: usize) -> Self {
        Self {
            anchor,
            spacing,
            level: 0,
            max_points: max_points.max(1),
            cells: CellMap::default(),
        }
    }

    fn add(&mut self, uv: [f64; 2], source: u32, record: &IndexedPoint) {
        let point = SlabPoint {
            uv,
            rgb: record.point.rgb,
            classification: record.point.classification,
            source,
        };
        let at = [
            (point.uv[0] - self.anchor[0]) / self.spacing,
            (point.uv[1] - self.anchor[1]) / self.spacing,
        ];
        // The slab lies to the upper right of the anchor; rounding may put a
        // point on its edge a hair below zero.
        let cell = at.map(|value| value.floor().clamp(0.0, u32::MAX as f64));
        let kept = Kept {
            point,
            off_centre: (at[0] - cell[0] - 0.5).powi(2) + (at[1] - cell[1] - 0.5).powi(2),
            ordinal: record.ordinal,
        };
        let key = cell_key(cell[0] as u32 >> self.level, cell[1] as u32 >> self.level);
        self.keep(key, kept);
        self.fit();
    }

    /// Join cells four by four while more fill than the drawing may hold.
    fn fit(&mut self) {
        while self.cells.len() > self.max_points && self.level < 31 {
            self.double();
        }
    }

    fn double(&mut self) {
        self.level += 1;
        for (key, kept) in std::mem::take(&mut self.cells) {
            let (x, y) = cell_of_key(key);
            self.keep(cell_key(x >> 1, y >> 1), kept);
        }
    }

    /// Take in the points another thinning of the same slab kept. The order
    /// of `Kept::before` does not depend on which points came first, so this
    /// keeps what one thinning of all the points keeps.
    fn merge(&mut self, mut other: Self) {
        while self.level < other.level {
            self.double();
        }
        while other.level < self.level {
            other.double();
        }
        for (key, kept) in other.cells {
            self.keep(key, kept);
        }
        self.fit();
    }

    fn keep(&mut self, key: u64, kept: Kept) {
        self.cells
            .entry(key)
            .and_modify(|current| {
                if kept.before(current) {
                    *current = kept;
                }
            })
            .or_insert(kept);
    }

    fn spacing(&self) -> f64 {
        self.spacing * (1u64 << self.level) as f64
    }

    /// The kept points row by row, each row from the left.
    fn finish(self) -> Vec<SlabPoint> {
        let mut cells: Vec<(u64, SlabPoint)> = self
            .cells
            .into_iter()
            .map(|(key, kept)| (key, kept.point))
            .collect();
        cells.sort_unstable_by_key(|(key, _)| *key);
        cells.into_iter().map(|(_, point)| point).collect()
    }
}

/// Row in the upper half, column in the lower, so that keys sort by row.
fn cell_key(x: u32, y: u32) -> u64 {
    (y as u64) << 32 | x as u64
}

fn cell_of_key(key: u64) -> (u32, u32) {
    (key as u32, (key >> 32) as u32)
}

/// The box two boxes have in common, if any.
fn common(a: Bounds, b: Bounds) -> Option<Bounds> {
    overlaps(a, b).then(|| Bounds {
        min: std::array::from_fn(|axis| a.min[axis].max(b.min[axis])),
        max: std::array::from_fn(|axis| a.max[axis].min(b.max[axis])),
    })
}

/// Points a read of one layer goes through to find those in a box.
fn points_to_read(source: &RegionSource<'_>, region: Bounds) -> u64 {
    match source.reader {
        RegionReader::Index(index) => index
            .intersecting_leaves(|node| overlaps(source.transform.bounds(node), region))
            .iter()
            .map(|leaf| leaf.stored_points)
            .sum(),
        RegionReader::Stream(cloud) => cloud.total_points,
        RegionReader::Resident(points) => points.len() as u64,
    }
}

/// Read the points of the slab from every layer once and collect what the
/// drawing needs of them.
///
/// A layer with an index is read from the octree leaves that touch the slab
/// only, so the work follows the slab and not the size of the scan; a layer
/// without one is read in full, and one that lies outside the slab is not
/// read at all. `accept` is asked for every point in the slab, with the
/// position of its layer in `sources`, its ordinal in the source file and the
/// point in scene coordinates: deleted points and hidden classes are left out
/// there. `progress` is called before the first read and at least every 4,096
/// points read; an error from it, such as `LoadError::Cancelled`, stops the
/// pass.
///
/// The count grid covers the part of the cut plane where the layers have
/// points, not the whole box: a box far larger than the scans costs no more
/// than one that fits them. Where the bounds of the layers are so wide that
/// the grid would get larger cells than asked, as a single stray point far
/// from the building makes them, the grid is laid again over the cells that
/// hold enough points to be drawn and the slab is read once more for it, at
/// most twice. Only a slab whose surfaces themselves span more than the grid
/// can hold keeps the larger cell.
pub fn collect_slab(
    sources: &[RegionSource<'_>],
    slab: &Slab,
    options: &SlabOptions,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(RegionProgress) -> Result<(), LoadError>,
) -> Result<SlabCut, LoadError> {
    check_options(options)?;
    let (work, covered) = slab_work(sources, slab);
    let first = RegionProgress {
        read: 0,
        total: work.iter().map(|(_, points)| points).sum(),
        accepted: 0,
    };
    progress(first)?;
    let percent = options.read_percent();
    let sampled = |position: usize, ordinal: u64, point: &Point| {
        sampled_ordinal(ordinal, percent) && accept(position, ordinal, point)
    };
    collect_passes(slab, options, covered, &mut |take, again| match again {
        None => read_slab(sources, &work, slab, &sampled, progress, take),
        // The job hears the count of the first read while it goes on.
        Some(state) => read_slab(
            sources,
            &work,
            slab,
            &sampled,
            &mut |_| progress(state),
            take,
        ),
    })
}

/// Refuses options that cannot collect anything, before a point is read.
pub(super) fn check_options(options: &SlabOptions) -> Result<(), LoadError> {
    let invalid = |reason: &str| Err(LoadError::InvalidData(reason.into()));
    let positive = |value: f64| value.is_finite() && value > 0.0;
    if options
        .point_spacing
        .is_some_and(|spacing| !positive(spacing))
    {
        return invalid("point spacing must be above zero");
    }
    if options.grid.is_some_and(|cell| !positive(cell)) {
        return invalid("grid size must be above zero");
    }
    // Written so that a NaN fails the test.
    let share = |percent: f64| percent > 0.0 && percent <= 100.0;
    if !share(options.sample_percent) || !share(options.grid_percent) {
        return invalid("the share of the points must lie above 0 and at most 100 percent");
    }
    Ok(())
}

/// The layers that reach into the slab, each as its position in `sources`
/// with the points a read of it goes through, and the part of the slab
/// they can have points in.
pub(super) fn slab_work(
    sources: &[RegionSource<'_>],
    slab: &Slab,
) -> (Vec<(usize, u64)>, Option<Bounds>) {
    let mut work = Vec::new();
    let mut covered: Option<Bounds> = None;
    for (position, source) in sources.iter().enumerate() {
        let Some(part) = source
            .world_bounds()
            .and_then(|bounds| common(bounds, slab.bounds))
        else {
            continue;
        };
        work.push((position, points_to_read(source, slab.bounds)));
        match &mut covered {
            Some(covered) => {
                for axis in 0..3 {
                    covered.min[axis] = covered.min[axis].min(part.min[axis]);
                    covered.max[axis] = covered.max[axis].max(part.max[axis]);
                }
            }
            None => covered = Some(part),
        }
    }
    (work, covered)
}

/// One pass over the points of the slab: it hands every point of the slab
/// that takes part to `take`, with its place on the cut plane and the
/// position of its layer, and answers with the count of the pass. The
/// second argument is `None` for the first pass and the count of the first
/// pass for a pass that lays the grid closer.
pub(super) type SlabPass<'a> = dyn FnMut(
        &mut dyn FnMut([f64; 2], u32, &IndexedPoint),
        Option<RegionProgress>,
    ) -> Result<RegionProgress, LoadError>
    + 'a;

/// What a pass over the slab collects: the thinned points and the count
/// grid, as far as the options ask for them. Two of these, filled with
/// different points of the same slab, merge into what one pass over all of
/// them gives, but for the order in which the sums of the grid are added.
#[derive(Debug, Clone)]
pub(super) struct Gathered {
    thinning: Option<Thinning>,
    grid: Option<CutGrid>,
    /// The shares of the points that are thinned and counted.
    shares: [f64; 2],
}

impl Gathered {
    /// Nothing collected yet, for the slab, the options and the part of the
    /// slab the layers can have points in.
    pub(super) fn start(
        slab: &Slab,
        options: &SlabOptions,
        covered: Option<Bounds>,
    ) -> Result<Self, LoadError> {
        let thinning = options
            .point_spacing
            .map(|spacing| Thinning::new(slab.extent[0], spacing, options.max_points));
        let grid = match (options.grid, covered) {
            (Some(cell), Some(covered)) => {
                // The corners of the box give the extent on the plane.
                let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
                for corner in bounds_corners(covered) {
                    let at = slab.frame.to_uv(corner);
                    for axis in 0..2 {
                        low[axis] = low[axis].min(at[axis]);
                        high[axis] = high[axis].max(at[axis]);
                    }
                }
                if slab.region.is_turned() {
                    // No farther than the box as the view sees it.
                    for axis in 0..2 {
                        low[axis] = low[axis].max(slab.extent[0][axis]);
                        high[axis] = high[axis].min(slab.extent[1][axis]);
                        if low[axis] > high[axis] {
                            // The layers reach the corners around the box only.
                            low[axis] = slab.extent[0][axis];
                            high[axis] = slab.extent[0][axis];
                        }
                    }
                }
                let frame = GridFrame::covering(low, high, cell, MAX_CUT_GRID_CELLS)?;
                Some(CutGrid::new(frame))
            }
            _ => None,
        };
        Ok(Self {
            thinning,
            grid,
            shares: [options.sample_percent, options.grid_percent],
        })
    }

    /// Take a point of the slab, at its place on the cut plane: on the grid
    /// and among the points to thin when it is in their share.
    pub(super) fn add(&mut self, uv: [f64; 2], source: u32, record: &IndexedPoint) {
        if let Some(grid) = &mut self.grid {
            if sampled_ordinal(record.ordinal, self.shares[1]) {
                grid.add(uv);
            }
        }
        if let Some(thinning) = &mut self.thinning {
            if sampled_ordinal(record.ordinal, self.shares[0]) {
                thinning.add(uv, source, record);
            }
        }
    }

    /// Take in what another pass over other points of the same slab
    /// collected.
    pub(super) fn merge(&mut self, other: Self) {
        if let (Some(grid), Some(more)) = (&mut self.grid, &other.grid) {
            grid.absorb(more);
        }
        if let (Some(thinning), Some(more)) = (&mut self.thinning, other.thinning) {
            thinning.merge(more);
        }
    }

    /// Memory the grid takes, which every copy of this takes again.
    pub(super) fn grid_bytes(&self) -> usize {
        self.grid
            .as_ref()
            .map_or(0, |grid| grid.frame().cells() * CutGrid::CELL_BYTES)
    }
}

/// Thin the points and count them on the grid, in as many passes over the
/// slab as the grid needs: see `collect_slab`. `covered` is the part of the
/// slab the layers can have points in.
pub(super) fn collect_passes(
    slab: &Slab,
    options: &SlabOptions,
    covered: Option<Bounds>,
    pass: &mut SlabPass<'_>,
) -> Result<SlabCut, LoadError> {
    let mut gathered = Gathered::start(slab, options, covered)?;
    let state = pass(
        &mut |uv, source, record| gathered.add(uv, source, record),
        None,
    )?;
    finish_passes(options, gathered, state, pass)
}

/// After the first pass, that gathered what `state` counts: lay the grid
/// closer when its cells came out larger than asked, with further passes,
/// and hand over what was collected.
pub(super) fn finish_passes(
    options: &SlabOptions,
    gathered: Gathered,
    state: RegionProgress,
    pass: &mut SlabPass<'_>,
) -> Result<SlabCut, LoadError> {
    let Gathered {
        thinning,
        mut grid,
        shares,
    } = gathered;
    let mut read_points = state.read;

    // Cells larger than asked: the bounds of the layers span more than the
    // grid can hold. The surfaces in the slab may not, so the grid is laid
    // over the cells that hold enough points for the cut, taken at the
    // largest size the trace may fall back to, and the slab is counted
    // again. The points are not thinned again, and the job hears the count
    // of the first read while it goes on.
    if let Some(asked) = options.grid {
        for _ in 0..MAX_GRID_REREADS {
            let Some(coarse) = grid.as_ref().filter(|grid| grid.frame().cell > asked) else {
                break;
            };
            let largest = asked * f64::from(1u32 << MAX_COARSENINGS);
            let Some([low, high]) = coarse.counted_extent(CUT_MIN_POINTS_PER_CELL, largest) else {
                break;
            };
            let frame = GridFrame::covering(low, high, asked, MAX_CUT_GRID_CELLS)?;
            if frame.cell >= coarse.frame().cell {
                break;
            }
            // The grid that served goes before the next one is made.
            drop(grid.take());
            let mut closer = CutGrid::new(frame);
            let again = pass(
                &mut |uv, _, record| {
                    if sampled_ordinal(record.ordinal, shares[1]) {
                        closer.add(uv);
                    }
                },
                Some(state),
            )?;
            read_points += again.read;
            grid = Some(closer);
        }
    }
    let spacing = thinning.as_ref().map_or(0.0, |thinning| thinning.spacing());
    Ok(SlabCut {
        points: thinning.map(Thinning::finish).unwrap_or_default(),
        spacing,
        grid,
        slab_points: state.accepted,
        read_points,
        reused_points: 0,
    })
}

/// One read of the slab from the layers in `work`, each as its position in
/// `sources` and the points a read of it goes through. `take` gets every
/// accepted point with its place on the cut plane and the position of its
/// layer; the count of the read comes back.
fn read_slab(
    sources: &[RegionSource<'_>],
    work: &[(usize, u64)],
    slab: &Slab,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(RegionProgress) -> Result<(), LoadError>,
    take: &mut dyn FnMut([f64; 2], u32, &IndexedPoint),
) -> Result<RegionProgress, LoadError> {
    let total: u64 = work.iter().map(|(_, points)| points).sum();
    let mut state = RegionProgress {
        read: 0,
        total,
        accepted: 0,
    };
    for (position, _) in work {
        let position = *position;
        let done = state;
        let turned = slab.region.is_turned();
        let stats = visit_region(
            &sources[position..=position],
            slab.bounds,
            &|_, ordinal, point| {
                // The leaves and the box are taken axis-aligned; a turned
                // slab also leaves out what lies in its corners.
                (!turned || slab.region.contains(point.xyz)) && accept(position, ordinal, point)
            },
            &mut |step| {
                state = RegionProgress {
                    read: done.read + step.read,
                    total,
                    accepted: done.accepted + step.accepted,
                };
                progress(state)
            },
            &mut |_, batch| {
                for record in batch {
                    take(slab.frame.to_uv(record.point.xyz), position as u32, record);
                }
                Ok(())
            },
        )?;
        state = RegionProgress {
            read: done.read + stats.read,
            total,
            accepted: done.accepted + stats.accepted,
        };
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region_source::SourceTransform;
    use crate::test_shapes::{box_room, indexed_cloud, Rng, RoomSpec};
    use crate::Point;

    /// 4.0 by 3.0 by 2.5 m, away from the origin.
    const BOX: Bounds = Bounds {
        min: [10.0, 20.0, 1.0],
        max: [14.0, 23.0, 3.5],
    };

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    fn near2(a: [f64; 2], b: [f64; 2]) -> bool {
        near(a[0], b[0]) && near(a[1], b[1])
    }

    #[test]
    fn every_view_cuts_at_the_face_it_looks_at() {
        // Per view: the axis and the range of the slab of 0.1 m, where the
        // point (11, 21, 2) lands with the model origin, the extent of the
        // box as seen with the model origin, and its size as seen.
        let cases = [
            (
                DrawingView::Plan,
                2,
                [3.4, 3.5],
                [11.0, 21.0],
                [10.0, 20.0],
                [4.0, 3.0],
            ),
            (
                DrawingView::Front,
                1,
                [20.0, 20.1],
                [1.0, 2.0],
                [0.0, 1.0],
                [4.0, 2.5],
            ),
            (
                DrawingView::Back,
                1,
                [22.9, 23.0],
                [3.0, 2.0],
                [0.0, 1.0],
                [4.0, 2.5],
            ),
            (
                DrawingView::Left,
                0,
                [10.0, 10.1],
                [2.0, 2.0],
                [0.0, 1.0],
                [3.0, 2.5],
            ),
            (
                DrawingView::Right,
                0,
                [13.9, 14.0],
                [1.0, 2.0],
                [0.0, 1.0],
                [3.0, 2.5],
            ),
        ];
        let point = [11.0, 21.0, 2.0];
        for (view, axis, range, uv, lower_left, size) in cases {
            assert_eq!(view.depth_axis(), axis);
            let slab = slab_from_section(BOX, view, Some(0.1), DrawingOrigin::Model).unwrap();
            assert_eq!(slab.view, view);
            assert!(near(slab.thickness, 0.1));
            // Only the depth of the box changes.
            for other in 0..3 {
                if other == axis {
                    assert!(near(slab.bounds.min[other], range[0]), "{view:?}");
                    assert!(near(slab.bounds.max[other], range[1]), "{view:?}");
                } else {
                    assert_eq!(slab.bounds.min[other], BOX.min[other]);
                    assert_eq!(slab.bounds.max[other], BOX.max[other]);
                }
            }
            assert!(near2(slab.frame.to_uv(point), uv), "{view:?}");
            assert!(near2(slab.extent[0], lower_left), "{view:?}");
            assert!(near2(
                slab.extent[1],
                [lower_left[0] + size[0], lower_left[1] + size[1]]
            ));
            // Back in the model the point lies on the cut plane, which is
            // the face of the box the slab starts from.
            let face = if range[0] == BOX.min[axis] {
                range[0]
            } else {
                range[1]
            };
            let mut on_plane = point;
            on_plane[axis] = face;
            let back = slab.frame.to_world(slab.frame.to_uv(point));
            assert!(
                (0..3).all(|index| near(back[index], on_plane[index])),
                "{view:?}"
            );
            assert!(near(slab.frame.origin[axis], face));

            // From the corner of the box the same drawing starts at zero.
            let corner = slab_from_section(BOX, view, Some(0.1), DrawingOrigin::BoxCorner).unwrap();
            assert_eq!(corner.bounds, slab.bounds);
            assert!(near2(corner.extent[0], [0.0, 0.0]), "{view:?}");
            assert!(near2(corner.extent[1], size), "{view:?}");
            let shift = [uv[0] - lower_left[0], uv[1] - lower_left[1]];
            assert!(near2(corner.frame.to_uv(point), shift), "{view:?}");
            // Right is to the right and up is up in both.
            assert_eq!(corner.frame.right, slab.frame.right);
            assert_eq!(corner.frame.up, slab.frame.up);
        }
        // A vertical view keeps model heights; a plan keeps model X and Y.
        let front = slab_from_section(BOX, DrawingView::Front, None, DrawingOrigin::Model).unwrap();
        assert_eq!(front.frame.to_uv([10.0, 22.0, 2.75]), [0.0, 2.75]);
        // Looking the other way, right is the other way.
        let back = slab_from_section(BOX, DrawingView::Back, None, DrawingOrigin::Model).unwrap();
        assert_eq!(back.frame.to_uv([14.0, 22.0, 2.75]), [0.0, 2.75]);
        assert_eq!(back.frame.to_uv([10.0, 22.0, 2.75]), [4.0, 2.75]);
    }

    #[test]
    fn slab_is_no_deeper_than_the_box_and_a_box_without_size_is_refused() {
        for thickness in [None, Some(2.5), Some(4.0)] {
            let slab =
                slab_from_section(BOX, DrawingView::Plan, thickness, DrawingOrigin::Model).unwrap();
            assert_eq!(slab.bounds, BOX);
            assert_eq!(slab.thickness, 2.5);
        }
        let elevation =
            slab_from_section(BOX, DrawingView::Left, None, DrawingOrigin::Model).unwrap();
        assert_eq!(elevation.bounds, BOX);
        assert_eq!(elevation.thickness, 4.0);

        let refused = |section: Bounds, view: DrawingView, thickness: Option<f64>| {
            matches!(
                slab_from_section(section, view, thickness, DrawingOrigin::Model),
                Err(LoadError::InvalidData(_))
            )
        };
        assert!(refused(BOX, DrawingView::Plan, Some(0.0)));
        assert!(refused(BOX, DrawingView::Plan, Some(-0.1)));
        assert!(refused(BOX, DrawingView::Plan, Some(f64::NAN)));
        let mut flat = BOX;
        flat.max[0] = flat.min[0];
        // No width to draw in a plan or from the front; from the left the
        // box is a plane, and a plane can be drawn.
        assert!(refused(flat, DrawingView::Plan, None));
        assert!(refused(flat, DrawingView::Front, None));
        let plane = slab_from_section(flat, DrawingView::Left, None, DrawingOrigin::Model).unwrap();
        assert_eq!(plane.thickness, 0.0);
        let mut inverted = BOX;
        inverted.min[2] = 4.0;
        assert!(refused(inverted, DrawingView::Plan, None));
        let mut open = BOX;
        open.max[1] = f64::INFINITY;
        assert!(refused(open, DrawingView::Plan, None));
    }

    fn resident(points: &[[f64; 3]]) -> Vec<IndexedPoint> {
        points
            .iter()
            .enumerate()
            .map(|(ordinal, xyz)| IndexedPoint {
                point: Point {
                    xyz: *xyz,
                    rgb: None,
                    intensity: None,
                    classification: None,
                },
                ordinal: ordinal as u64,
            })
            .collect()
    }

    fn collect(sources: &[RegionSource<'_>], slab: &Slab, options: &SlabOptions) -> SlabCut {
        collect_slab(sources, slab, options, &|_, _, _| true, &mut |_| Ok(())).unwrap()
    }

    #[test]
    fn thinning_keeps_one_point_per_cell_whatever_the_order_of_reading() {
        // 400,000 points at random in 1 by 1 m: ten per cell of 5 mm.
        let mut rng = Rng::new(5);
        let points: Vec<[f64; 3]> = (0..400_000)
            .map(|_| [rng.unit(), rng.unit(), rng.unit() * 0.1])
            .collect();
        let forward = resident(&points);
        let mut backward = forward.clone();
        backward.reverse();
        let mut shuffled = forward.clone();
        for index in (1..shuffled.len()).rev() {
            shuffled.swap(index, (rng.next_u64() % (index as u64 + 1)) as usize);
        }
        let section = Bounds {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 1.0, 0.1],
        };
        let slab =
            slab_from_section(section, DrawingView::Plan, None, DrawingOrigin::Model).unwrap();
        let thinned = |records: &[IndexedPoint], max_points: usize| {
            collect(
                &[RegionSource::resident(records, SourceTransform::default())],
                &slab,
                &SlabOptions {
                    point_spacing: Some(0.005),
                    max_points,
                    grid: None,
                    sample_percent: 100.0,
                    grid_percent: 100.0,
                },
            )
        };
        let cut = thinned(&forward, 150_000);
        assert_eq!(cut.slab_points, 400_000);
        assert_eq!(cut.spacing, 0.005);
        assert!(cut.grid.is_none());
        let cell = |uv: [f64; 2]| ((uv[1] / 0.005) as u32, (uv[0] / 0.005) as u32);
        // Row by row, one point per cell.
        assert!(cut
            .points
            .windows(2)
            .all(|pair| cell(pair[0].uv) < cell(pair[1].uv)));
        // Each is the point of its cell nearest the centre of the cell.
        let off_centre = |uv: [f64; 2]| {
            let (row, column) = cell(uv);
            (uv[0] / 0.005 - column as f64 - 0.5).hypot(uv[1] / 0.005 - row as f64 - 0.5)
        };
        let mut nearest = vec![f64::INFINITY; 40_000];
        for point in &points {
            let (row, column) = cell([point[0], point[1]]);
            let slot = &mut nearest[(row * 200 + column) as usize];
            *slot = slot.min(off_centre([point[0], point[1]]));
        }
        for point in &cut.points {
            let (row, column) = cell(point.uv);
            assert!((off_centre(point.uv) - nearest[(row * 200 + column) as usize]).abs() < 1e-9);
        }
        // 200 by 200 cells, and every cell that holds a point has one drawn.
        // With ten points per cell on average, two cells happen to be empty.
        let filled = nearest.iter().filter(|off| off.is_finite()).count();
        assert_eq!(filled, 39_998);
        assert_eq!(cut.points.len(), filled);
        // The same points whatever the order they are read in.
        assert_eq!(thinned(&backward, 150_000).points, cut.points);
        assert_eq!(thinned(&shuffled, 150_000).points, cut.points);

        // More cells than the drawing may hold points: the spacing doubles
        // until they fit, and the result still does not depend on the order,
        // although the cells were joined at another moment in each.
        for (max_points, spacing, count) in
            [(10_000, 0.01, 10_000), (9_999, 0.02, 2_500), (1, 1.28, 1)]
        {
            let coarse = thinned(&forward, max_points);
            assert_eq!(coarse.spacing, spacing);
            assert_eq!(coarse.points.len(), count);
            assert_eq!(coarse.slab_points, 400_000);
            assert_eq!(thinned(&backward, max_points).points, coarse.points);
            assert_eq!(thinned(&shuffled, max_points).points, coarse.points);
            // Every point that is drawn is a point of the scan.
            assert!(coarse.points.iter().all(|point| cut.points.contains(point)));
        }
    }

    #[test]
    fn count_grid_keeps_the_mean_position_also_in_larger_cells() {
        let frame = GridFrame::new([100.0, 200.0], 0.02, 4, 4).unwrap();
        let mut grid = CutGrid::new(frame);
        // Three points in cell (1, 1), one in (0, 1), one in (3, 2).
        let points = [
            [100.021, 200.025],
            [100.023, 200.035],
            [100.037, 200.030],
            [100.005, 200.039],
            [100.079, 200.041],
        ];
        for point in points {
            grid.add(point);
        }
        grid.add([99.0, 200.0]);
        assert_eq!(grid.counts().count(1, 1), 3);
        assert_eq!(grid.counts().counts().iter().sum::<u32>(), 5);
        assert_eq!(grid.centroid(0, 0), None);
        let close = |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-6;
        assert!(close(grid.centroid(1, 1).unwrap(), [100.027, 200.030]));
        assert!(close(grid.centroid(3, 2).unwrap(), [100.079, 200.041]));

        // Cells of 40 mm: (0, 0) now holds the four points of the left half.
        let coarse = grid.coarsened();
        assert_eq!(coarse.frame().cell, 0.04);
        assert_eq!((coarse.frame().width, coarse.frame().height), (2, 2));
        assert_eq!(coarse.counts().count(0, 0), 4);
        assert_eq!(coarse.counts().count(1, 1), 1);
        assert!(close(
            coarse.centroid(0, 0).unwrap(),
            [
                (100.021 + 100.023 + 100.037 + 100.005) / 4.0,
                (200.025 + 200.035 + 200.030 + 200.039) / 4.0
            ]
        ));
        assert!(close(coarse.centroid(1, 1).unwrap(), [100.079, 200.041]));
        assert_eq!(coarse.centroid(1, 0), None);
    }

    /// A room of 4.0 by 3.0 by 2.6 m with a point every 2 cm, each with a
    /// colour and one of three classes.
    fn room_points() -> Vec<Point> {
        let mut points = box_room(&RoomSpec::default()).cloud_points();
        for (ordinal, point) in points.iter_mut().enumerate() {
            point.classification = Some((ordinal % 3) as u8 + 1);
        }
        points
    }

    fn records(points: &[Point]) -> Vec<IndexedPoint> {
        points
            .iter()
            .enumerate()
            .map(|(ordinal, point)| IndexedPoint {
                point: *point,
                ordinal: ordinal as u64,
            })
            .collect()
    }

    const OPTIONS: SlabOptions = SlabOptions {
        point_spacing: Some(0.005),
        max_points: 150_000,
        grid: Some(0.02),
        sample_percent: 100.0,
        grid_percent: 100.0,
    };

    #[test]
    fn index_stream_and_memory_give_the_same_slab_and_the_index_reads_less() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 2_048);
        let in_memory = records(&points);
        let section = Bounds {
            min: [-1.0, -1.0, 0.0],
            max: [5.0, 4.0, 1.1],
        };
        let slab =
            slab_from_section(section, DrawingView::Plan, Some(0.1), DrawingOrigin::Model).unwrap();
        let identity = SourceTransform::default();
        let indexed = collect(
            &[RegionSource::new(
                &cloud.cloud,
                Some(&cloud.index),
                identity,
            )],
            &slab,
            &OPTIONS,
        );
        let streamed = collect(
            &[RegionSource::new(&cloud.cloud, None, identity)],
            &slab,
            &OPTIONS,
        );
        let resident = collect(
            &[RegionSource::resident(&in_memory, identity)],
            &slab,
            &OPTIONS,
        );
        let truth = points
            .iter()
            .filter(|point| point.xyz[2] >= 1.0 && point.xyz[2] <= 1.1)
            .count() as u64;
        // Four walls of 14 m in a slab of 0.1 m at a point per 2 cm.
        assert_eq!(truth, 3_500);
        for cut in [&indexed, &streamed, &resident] {
            assert_eq!(cut.slab_points, truth);
            assert_eq!(cut.points, indexed.points);
            assert_eq!(cut.grid, indexed.grid);
            assert_eq!(cut.spacing, 0.005);
        }
        // Without an index every point is read; with one only the leaves
        // that touch the slab.
        assert_eq!(streamed.read_points, points.len() as u64);
        assert_eq!(resident.read_points, points.len() as u64);
        assert!(indexed.read_points >= truth);
        assert!(
            indexed.read_points * 4 < points.len() as u64,
            "{} of {}",
            indexed.read_points,
            points.len()
        );
        // The points are those of the scan, in plan: model X and Y. Seen
        // from above the five rows of the slab fall on each other, so one
        // point of each is drawn.
        assert_eq!(indexed.points.len(), 700);
        for point in &indexed.points {
            assert!(points.iter().any(|scan| {
                scan.xyz[0] == point.uv[0]
                    && scan.xyz[1] == point.uv[1]
                    && scan.rgb == point.rgb
                    && scan.classification == point.classification
            }));
            assert_eq!(point.source, 0);
        }

        // The grid lies over the scan, not over the box: a box a thousand
        // times the size gives the same grid.
        let grid = indexed.grid.as_ref().unwrap();
        assert_eq!(grid.frame().origin, [0.0, 0.0]);
        assert_eq!((grid.frame().width, grid.frame().height), (201, 151));
        assert_eq!(
            grid.counts()
                .counts()
                .iter()
                .map(|count| *count as u64)
                .sum::<u64>(),
            truth
        );
        let wide = Bounds {
            min: [-2_000.0, -2_000.0, 0.0],
            max: [2_000.0, 2_000.0, 1.1],
        };
        let wide_slab =
            slab_from_section(wide, DrawingView::Plan, Some(0.1), DrawingOrigin::Model).unwrap();
        let far = collect(
            &[RegionSource::new(
                &cloud.cloud,
                Some(&cloud.index),
                identity,
            )],
            &wide_slab,
            &OPTIONS,
        );
        assert_eq!(far.grid, indexed.grid);
        assert_eq!(far.read_points, indexed.read_points);
        assert_eq!(far.points.len(), 700);
    }

    #[test]
    fn layer_transform_and_filter_apply_before_a_point_is_drawn() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 2_048);
        // The layer is twice the size and stands elsewhere: the slab that
        // holds source heights 1.0 to 1.1 lies at 12.0 to 12.2.
        let transform = SourceTransform {
            scale: [2.0, 2.0, 2.0],
            offset: [100.0, 200.0, 10.0],
        };
        let section = Bounds {
            min: [90.0, 190.0, 11.0],
            max: [120.0, 220.0, 12.2],
        };
        let slab =
            slab_from_section(section, DrawingView::Plan, Some(0.2), DrawingOrigin::Model).unwrap();
        for index in [Some(&cloud.index), None] {
            let sources = [
                // A layer that lies outside the slab is not read.
                RegionSource::new(&cloud.cloud, index, SourceTransform::default()),
                RegionSource::new(&cloud.cloud, index, transform),
            ];
            let all = collect(&sources, &slab, &OPTIONS);
            assert_eq!(all.slab_points, 3_500);
            assert!(all.read_points <= points.len() as u64);
            assert!(all.points.iter().all(|point| point.source == 1
                && (100.0..=108.0).contains(&point.uv[0])
                && (200.0..=206.0).contains(&point.uv[1])));
            let grid = all.grid.as_ref().unwrap();
            assert_eq!(grid.frame().origin, [100.0, 200.0]);

            // Deleted points and a hidden class are left out by the filter,
            // which gets the position of the layer, the source ordinal and
            // the point in the scene.
            let mut asked = std::sync::atomic::AtomicU64::new(0);
            let kept = collect_slab(
                &sources,
                &slab,
                &OPTIONS,
                &|source, ordinal, point| {
                    asked.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    assert_eq!(source, 1);
                    assert_eq!(point.xyz[2], points[ordinal as usize].xyz[2] * 2.0 + 10.0);
                    ordinal % 2 == 0 && point.classification != Some(1)
                },
                &mut |_| Ok(()),
            )
            .unwrap();
            // Only points in the slab are asked about.
            assert_eq!(*asked.get_mut(), 3_500);
            let truth = points
                .iter()
                .enumerate()
                .filter(|(ordinal, point)| {
                    (1.0..=1.1).contains(&point.xyz[2])
                        && ordinal % 2 == 0
                        && point.classification != Some(1)
                })
                .count() as u64;
            assert!(truth > 1_000 && truth < 1_500);
            assert_eq!(kept.slab_points, truth);
            // One point is drawn for every place in plan that has one left.
            let places: std::collections::HashSet<[u64; 2]> = points
                .iter()
                .enumerate()
                .filter(|(ordinal, point)| {
                    (1.0..=1.1).contains(&point.xyz[2])
                        && ordinal % 2 == 0
                        && point.classification != Some(1)
                })
                .map(|(_, point)| [point.xyz[0].to_bits(), point.xyz[1].to_bits()])
                .collect();
            assert_eq!(kept.points.len(), places.len());
            assert!(kept
                .points
                .iter()
                .all(|point| point.classification != Some(1)));
            let counted: u64 = kept
                .grid
                .as_ref()
                .unwrap()
                .counts()
                .counts()
                .iter()
                .map(|count| *count as u64)
                .sum();
            assert_eq!(counted, truth);
        }
    }

    #[test]
    fn mean_position_stays_true_for_millions_of_points_in_one_cell() {
        // One cell of 20 mm at national grid coordinates with eight million
        // points in it, as a slab of the whole depth puts in the cells of a
        // floor seen from the side.
        let frame = GridFrame::new([155_000.0, 463_000.0], 0.02, 2, 2).unwrap();
        let mut grid = CutGrid::new(frame);
        let mut rng = Rng::new(9);
        let mut sum = [0.0f64; 2];
        let count = 8_000_000u32;
        for _ in 0..count {
            // Around 15 and 10 mm into the cell.
            let offset = [
                0.015 + (rng.unit() - 0.5) * 0.004,
                0.010 + (rng.unit() - 0.5) * 0.004,
            ];
            sum = [sum[0] + offset[0], sum[1] + offset[1]];
            grid.add([frame.origin[0] + offset[0], frame.origin[1] + offset[1]]);
        }
        assert_eq!(grid.counts().count(0, 0), count);
        let mean = grid.centroid(0, 0).unwrap();
        for axis in 0..2 {
            let off = (mean[axis] - frame.origin[axis] - sum[axis] / count as f64).abs();
            // Within a thousandth of a millimetre; measured 2e-11 m, the
            // rounding of a coordinate of this size. Summed in single
            // precision the mean was 0.5 mm off here, and 5 mm with forty
            // million points.
            assert!(off < 1e-6, "{off}");
        }
    }

    #[test]
    fn counted_extent_is_the_rectangle_around_the_cells_with_enough_points() {
        let frame = GridFrame::new([10.0, 20.0], 0.02, 100, 100).unwrap();
        let mut grid = CutGrid::new(frame);
        // Points per cell: three in (10, 20) and in (50, 60), two in (0, 0)
        // and one in (1, 1) beside it, one in (90, 5).
        for (x, y, count) in [(10, 20, 3), (50, 60, 3), (0, 0, 2), (1, 1, 1), (90, 5, 1)] {
            for _ in 0..count {
                grid.add(frame.cell_center(x, y));
            }
        }
        let near = |extent: Option<[[f64; 2]; 2]>, truth: [[f64; 2]; 2]| {
            let extent = extent.unwrap();
            (0..2).all(|corner| near2(extent[corner], truth[corner]))
        };
        // The cells with three points, and one cell more on every side.
        assert!(near(
            grid.counted_extent(3, 0.02),
            [[10.18, 20.38], [11.04, 21.24]]
        ));
        // In cells of 80 mm the two cells in the corner hold three points
        // together, and the rectangle starts a cell before them.
        assert!(near(
            grid.counted_extent(3, 0.08),
            [[9.92, 19.92], [11.12, 21.36]]
        ));
        assert_eq!(grid.counted_extent(4, 0.02), None);
        assert_eq!(CutGrid::new(frame).counted_extent(1, 0.02), None);
    }

    #[test]
    fn a_stray_point_far_from_the_scan_does_not_coarsen_the_count_grid() {
        use crate::drawing::outline::{trace_cut_regions, OutlineOptions};
        use crate::drawing::DrawingRequest;

        let points = room_points();
        // The whole extent of the scan in plan, as a box that is only moved
        // up and down gives.
        let section = Bounds {
            min: [-3_000.0, -3_000.0, 0.0],
            max: [3_000.0, 3_000.0, 1.1],
        };
        let slab =
            slab_from_section(section, DrawingView::Plan, Some(0.1), DrawingOrigin::Model).unwrap();
        let cut_of = |points: &[Point]| {
            let in_memory = records(points);
            collect(
                &[RegionSource::resident(
                    &in_memory,
                    SourceTransform::default(),
                )],
                &slab,
                &OPTIONS,
            )
        };
        let regions = |cut: &SlabCut| {
            let options = OutlineOptions::for_request(&DrawingRequest::default());
            trace_cut_regions(cut.grid.as_ref().unwrap(), &options, &mut || Ok(()))
                .unwrap()
                .regions
        };
        let clean = cut_of(&points);
        let frame = clean.grid.as_ref().unwrap().frame();
        assert_eq!((frame.cell, frame.width, frame.height), (0.02, 201, 151));
        assert_eq!(clean.read_points, points.len() as u64);
        let drawn = regions(&clean);
        assert_eq!(drawn.len(), 1);

        // One point 5 m above the slab, one in it, and one 2 km away. The
        // bounds of the layer then span 300 m and more, and a grid over them
        // has cells of 80, 80 and 640 mm.
        for stray in [
            [300.0, 300.0, 6.0],
            [300.0, 300.0, 1.05],
            [2_000.0, 2_000.0, 6.0],
        ] {
            let mut with_stray = points.clone();
            with_stray.push(Point {
                xyz: stray,
                ..points[0]
            });
            let cut = cut_of(&with_stray);
            let in_slab = u64::from(stray[2] <= 1.1);
            assert_eq!(cut.slab_points, 3_500 + in_slab);
            // The slab was read a second time, for a grid of the cell that
            // was asked over the walls alone.
            assert_eq!(cut.read_points, 2 * with_stray.len() as u64, "{stray:?}");
            let grid = cut.grid.as_ref().unwrap();
            assert_eq!(grid.frame().cell, 0.02, "{stray:?}");
            assert!(grid.frame().cells() < 100_000, "{:?}", grid.frame());
            let counted: u64 = grid
                .counts()
                .counts()
                .iter()
                .map(|count| *count as u64)
                .sum();
            assert_eq!(counted, 3_500);
            // The points are thinned once: those of the room, and the stray
            // point where it lies in the slab.
            assert_eq!(cut.points.len() as u64, 700 + in_slab);
            assert_eq!(cut.points[..700], clean.points[..]);
            // The cut is the one of the scan without the stray point.
            // Before, its outer faces were drawn 40 mm off at 300 m, and
            // at 2 km the wall was a block.
            let traced = regions(&cut);
            assert_eq!(traced.len(), drawn.len());
            for (region, truth) in traced.iter().zip(&drawn) {
                assert_eq!(region.holes.len(), truth.holes.len());
                let rings = std::iter::once(&region.outer).chain(&region.holes);
                let truths = std::iter::once(&truth.outer).chain(&truth.holes);
                for (ring, truth) in rings.zip(truths) {
                    assert_eq!(ring.len(), truth.len());
                    assert!(ring
                        .iter()
                        .zip(truth)
                        .all(|(a, b)| (a[0] - b[0]).hypot(a[1] - b[1]) < 1e-9));
                }
            }
        }
    }

    #[test]
    fn empty_slab_reads_nothing_and_a_cancelled_read_stops() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 2_048);
        let identity = SourceTransform::default();
        // A box beside the scan: neither the index nor the file is read.
        let beside = Bounds {
            min: [10.0, 0.0, 0.0],
            max: [14.0, 3.0, 2.6],
        };
        let slab =
            slab_from_section(beside, DrawingView::Plan, None, DrawingOrigin::Model).unwrap();
        for index in [Some(&cloud.index), None] {
            let mut calls = Vec::new();
            let cut = collect_slab(
                &[RegionSource::new(&cloud.cloud, index, identity)],
                &slab,
                &OPTIONS,
                &|_, _, _| true,
                &mut |step| {
                    calls.push(step);
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!((cut.slab_points, cut.read_points), (0, 0));
            assert!(cut.points.is_empty() && cut.grid.is_none());
            assert_eq!(
                calls,
                [RegionProgress {
                    read: 0,
                    total: 0,
                    accepted: 0
                }]
            );
        }
        // A slab in the open middle of the room, between floor and ceiling
        // and away from the walls: points are read, none lies in it.
        let middle = Bounds {
            min: [1.0, 1.0, 1.0],
            max: [3.0, 2.0, 1.5],
        };
        let slab =
            slab_from_section(middle, DrawingView::Plan, None, DrawingOrigin::Model).unwrap();
        let cut = collect(
            &[RegionSource::new(
                &cloud.cloud,
                Some(&cloud.index),
                identity,
            )],
            &slab,
            &OPTIONS,
        );
        assert_eq!(cut.slab_points, 0);
        assert!(cut.points.is_empty());
        assert_eq!(cut.grid.as_ref().unwrap().counts().occupied(), 0);

        // Progress counts up to the points of the leaves that are read, and
        // an error from it ends the pass.
        let section = Bounds {
            min: [-1.0, -1.0, 0.0],
            max: [5.0, 4.0, 1.1],
        };
        let slab =
            slab_from_section(section, DrawingView::Plan, Some(0.1), DrawingOrigin::Model).unwrap();
        let sources = [RegionSource::new(
            &cloud.cloud,
            Some(&cloud.index),
            identity,
        )];
        let mut steps = Vec::new();
        let cut = collect_slab(&sources, &slab, &OPTIONS, &|_, _, _| true, &mut |step| {
            steps.push(step);
            Ok(())
        })
        .unwrap();
        assert!(steps.len() > 3);
        assert!(steps.windows(2).all(|pair| pair[0].read <= pair[1].read
            && pair[0].accepted <= pair[1].accepted
            && pair[0].total == pair[1].total));
        let last = steps[steps.len() - 1];
        assert_eq!(
            (last.read, last.accepted),
            (cut.read_points, cut.slab_points)
        );
        assert_eq!(last.total, cut.read_points);
        for stop in [1, 2, steps.len()] {
            let mut calls = 0;
            let result = collect_slab(&sources, &slab, &OPTIONS, &|_, _, _| true, &mut |_| {
                calls += 1;
                if calls == stop {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(result, Err(LoadError::Cancelled)), "{stop}");
        }
        // Sizes that cannot be are refused before anything is read.
        for broken in [
            SlabOptions {
                point_spacing: Some(0.0),
                ..OPTIONS
            },
            SlabOptions {
                grid: Some(f64::NAN),
                ..OPTIONS
            },
        ] {
            assert!(matches!(
                collect_slab(&sources, &slab, &broken, &|_, _, _| true, &mut |_| panic!()),
                Err(LoadError::InvalidData(_))
            ));
        }
    }
}
