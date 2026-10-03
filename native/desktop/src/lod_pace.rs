//! How large the first pass of a viewport refinement may be on this computer,
//! and whether an intermediate sample is worth swapping in for the one on
//! screen.

use std::sync::Mutex;
use std::time::Duration;

use pointcloud_core::{Bounds, IndexedNode};

use crate::selection::Projection;

/// A first pass never holds fewer points than this.
pub const LOD_FIRST_PASS_MIN: usize = 250_000;
/// Time a first pass may take to read, and its geometry to build on the
/// UI thread.
pub const LOD_FIRST_PASS_MS: f64 = 200.0;
pub const LOD_PREVIEW_BUILD_MS: f64 = 50.0;
/// Read pace assumed until a pass was timed: the smallest first pass in the
/// time a first pass may take.
pub const LOD_SEED_POINTS_PER_MS: f64 = 1_250.0;
/// A smaller pass or geometry build is mostly fixed cost and says little
/// about the pace.
pub const LOD_PACE_MIN_POINTS: usize = 250_000;
pub const LOD_BUILD_MIN_POINTS: usize = 65_536;
/// A slow sample counts for more than a fast one, so a cold disk shrinks the
/// next first pass at once.
pub const LOD_PACE_FALL: f64 = 0.5;
pub const LOD_PACE_RISE: f64 = 0.25;
/// A sample further than this factor from the known pace counts as that far.
pub const LOD_PACE_OUTLIER: f64 = 4.0;
/// How the octree sampler picks the nodes of a view, repeated here to count
/// them without reading: it replaces a node wider than this many pixels by
/// its children, for as long as the nodes stay within one per so many points
/// of the limit and within the most.
pub const LOD_NODE_SPLIT_SPAN: f32 = 96.0;
pub const LOD_POINTS_PER_NODE: usize = 128;
pub const LOD_MAX_NODES: usize = 1_024;
/// Points the sampler takes from a node without reading a leaf in full. A
/// leaf keeps a preview of this size once it holds over four times as many.
pub const LOD_NODE_PREVIEW_POINTS: usize = 2_048;
/// Most nodes the sampler picks times the preview points of a node. Above it
/// a pass reads whole leaves, which a first pass should not wait for.
pub const LOD_PREVIEW_TIER_POINTS: usize = LOD_MAX_NODES * LOD_NODE_PREVIEW_POINTS;
/// The viewport is compared in a raster of this many cells each way, with at
/// most this many points looked at per cloud.
pub const LOD_FILL_GRID: usize = 16;
pub const LOD_FILL_PROBES: usize = 4_096;
/// A view that holds only a few percent of a large set gets too few of those
/// looks to tell which cells the set reaches: under this many in view, the
/// set is looked at once more with this many times the looks.
pub const LOD_FILL_MIN_HITS: usize = 4 * LOD_FILL_GRID * LOD_FILL_GRID;
pub const LOD_FILL_RESWEEP: usize = 8;

/// Points per millisecond this computer reads from the octree and builds
/// into geometry, as measured while the application runs.
#[derive(Debug, Default)]
pub struct LodPace {
    read: Mutex<Option<f64>>,
    build: Mutex<Option<f64>>,
}

impl LodPace {
    pub fn record_read(&self, points: usize, elapsed: Duration) {
        if points >= LOD_PACE_MIN_POINTS {
            record(&self.read, points, elapsed);
        }
    }

    pub fn record_build(&self, points: usize, elapsed: Duration) {
        if points >= LOD_BUILD_MIN_POINTS {
            record(&self.build, points, elapsed);
        }
    }

    pub fn read_points_per_ms(&self) -> f64 {
        self.read
            .lock()
            .ok()
            .and_then(|pace| *pace)
            .unwrap_or(LOD_SEED_POINTS_PER_MS)
    }

    pub fn build_points_per_ms(&self) -> Option<f64> {
        self.build.lock().ok().and_then(|pace| *pace)
    }
}

fn record(pace: &Mutex<Option<f64>>, points: usize, elapsed: Duration) {
    let milliseconds = elapsed.as_nanos() as f64 / 1_000_000.0;
    if milliseconds <= 0.0 {
        return;
    }
    let sample = points as f64 / milliseconds;
    let Ok(mut pace) = pace.lock() else {
        return;
    };
    *pace = Some(match *pace {
        Some(known) => {
            let sample = sample.clamp(known / LOD_PACE_OUTLIER, known * LOD_PACE_OUTLIER);
            let weight = if sample < known {
                LOD_PACE_FALL
            } else {
                LOD_PACE_RISE
            };
            known + (sample - known) * weight
        }
        None => sample,
    });
}

/// Points to read in the first pass of a refinement that may show `effective`
/// points from `sources` clouds. The whole of `effective` means one pass.
pub fn first_pass_budget(
    effective: usize,
    read_pace: f64,
    build_pace: Option<f64>,
    sources: usize,
) -> usize {
    // A first pass over half of everything saves too little to be worth a
    // second read of the same nodes.
    if effective <= LOD_FIRST_PASS_MIN * 2 {
        return effective;
    }
    // The pace only sizes the first pass and never removes it: measured on
    // node previews, or on many clouds read side by side, it says little
    // about a pass that has to read whole leaves of one cloud.
    let read_reach = read_pace * LOD_FIRST_PASS_MS;
    let build_reach = build_pace.map_or(f64::INFINITY, |pace| pace * LOD_PREVIEW_BUILD_MS);
    let first = read_reach.min(build_reach).max(LOD_FIRST_PASS_MIN as f64);
    (first as usize)
        .min(LOD_PREVIEW_TIER_POINTS.saturating_mul(sources.max(1)))
        .min(effective / 2)
}

/// Most points a pass may ask of a cloud in one view without reading a leaf
/// in full, where `span` gives the size of a node on screen as the sampler
/// sees it and `limit` is what the pass would ask otherwise. `None` when the
/// cloud is out of view, or when no node in view is a leaf that holds more
/// than its preview: asking for more then costs nothing extra.
pub fn preview_tier_points(
    root: &IndexedNode,
    limit: usize,
    span: impl Fn(Bounds) -> Option<f32>,
) -> Option<usize> {
    let most = (limit / LOD_POINTS_PER_NODE)
        .clamp(8, LOD_MAX_NODES)
        .min(limit);
    // The sampler stops splitting once another node with eight children no
    // longer fits, which leaves it at most six nodes short of the most.
    let room = most.saturating_sub(6).max(1);
    let mut frontier = Frontier {
        room,
        points: 0,
        whole_leaves: false,
    };
    frontier.visit(root, span(root.bounds)?, &span);
    if frontier.room == 0 {
        // Which nodes the sampler keeps is then not known from here, only
        // about how many.
        return Some(room * LOD_NODE_PREVIEW_POINTS);
    }
    frontier.whole_leaves.then_some(frontier.points)
}

/// The nodes the sampler reads for a view, counted without reading them.
struct Frontier {
    /// Nodes that may still be counted.
    room: usize,
    /// Points the counted nodes give from their previews.
    points: usize,
    /// Whether a counted leaf holds more than its preview.
    whole_leaves: bool,
}

impl Frontier {
    fn visit(&mut self, node: &IndexedNode, node_span: f32, span: &impl Fn(Bounds) -> Option<f32>) {
        if self.room == 0 {
            return;
        }
        if !node.is_leaf() && node_span > LOD_NODE_SPLIT_SPAN {
            let visible: Vec<_> = node
                .children
                .iter()
                .filter_map(|child| span(child.bounds).map(|size| (child, size)))
                .collect();
            if !visible.is_empty() {
                for (child, size) in visible {
                    self.visit(child, size, span);
                }
                return;
            }
        }
        self.room -= 1;
        let stored = usize::try_from(node.stored_points).unwrap_or(usize::MAX);
        self.points += stored.min(LOD_NODE_PREVIEW_POINTS);
        self.whole_leaves |= node.is_leaf() && stored > LOD_NODE_PREVIEW_POINTS * 4;
    }
}

/// What a set of points puts in the viewport: about how many of them, and
/// which cells of the raster they reach.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenFill {
    pub points: usize,
    cells: [u64; LOD_FILL_GRID * LOD_FILL_GRID / 64],
}

impl ScreenFill {
    /// Count `items` from an evenly spaced part of them, looked at more
    /// closely when few of them are in view. The deletion mask and class
    /// filters are left out: they thin every set alike when drawn.
    pub fn add<T>(
        &mut self,
        items: &[T],
        position: impl Fn(&T) -> [f64; 3],
        projection: Projection,
        section: Option<Bounds>,
    ) {
        let mut sweep = |step: usize| {
            let (mut probes, mut hits) = (0u128, 0u128);
            for item in items.iter().step_by(step) {
                probes += 1;
                let xyz = position(item);
                if section.is_some_and(|clip| {
                    (0..3).any(|axis| xyz[axis] < clip.min[axis] || xyz[axis] > clip.max[axis])
                }) {
                    continue;
                }
                if let Some(cell) = projection.grid_cell(xyz, LOD_FILL_GRID) {
                    self.mark(cell);
                    hits += 1;
                }
            }
            (probes, hits)
        };
        let step = items.len().div_ceil(LOD_FILL_PROBES).max(1);
        let (mut probes, mut hits) = sweep(step);
        // Once, and not for a set with nothing in view: what a finer look
        // could still find of it is too little to matter.
        if step > 1 && (1..LOD_FILL_MIN_HITS as u128).contains(&hits) {
            (probes, hits) = sweep((step / LOD_FILL_RESWEEP).max(1));
        }
        if probes > 0 {
            self.points += (items.len() as u128 * hits / probes) as usize;
        }
    }

    /// Mark every cell a box can reach, without counting points.
    pub fn add_box(&mut self, projection: Projection, bounds: Bounds) {
        let Some([column_min, column_max, row_min, row_max]) =
            projection.grid_rect(bounds, LOD_FILL_GRID)
        else {
            return;
        };
        for row in row_min..=row_max {
            for column in column_min..=column_max {
                self.mark(row * LOD_FILL_GRID + column);
            }
        }
    }

    fn mark(&mut self, cell: usize) {
        self.cells[cell / 64] |= 1 << (cell % 64);
    }

    pub fn cell_count(&self) -> usize {
        self.cells
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Whether a first pass of `first` points that reaches about
    /// `reach_cells` cells could not improve on this picture.
    pub fn holds(&self, first: usize, reach_cells: usize) -> bool {
        self.points >= first && self.cell_count() * 2 >= reach_cells
    }

    #[cfg(test)]
    pub fn of(points: usize, cells: usize) -> Self {
        let mut fill = Self {
            points,
            ..Self::default()
        };
        (0..cells).for_each(|cell| fill.mark(cell));
        fill
    }
}

/// Whether an intermediate set may replace the one on screen: when that one
/// misses more than half of what the new view shows, or when the new set
/// brings more points and is at least as dense where it has points.
pub fn preview_improves(shown: ScreenFill, fresh: ScreenFill) -> bool {
    let (shown_cells, fresh_cells) = (shown.cell_count(), fresh.cell_count());
    shown_cells * 2 < fresh_cells
        || (fresh.points > shown.points
            && fresh.points as u128 * shown_cells as u128
                >= shown.points as u128 * fresh_cells as u128)
}

/// Read everything in one pass when a first pass of `first` points could not
/// be shown anyway.
pub fn plan_first_pass(
    effective: usize,
    first: usize,
    shown: ScreenFill,
    reach_cells: usize,
) -> usize {
    if first < effective && shown.holds(first, reach_cells) {
        effective
    } else {
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: usize = 6_000_000;

    #[test]
    fn first_pass_follows_the_measured_pace() {
        for (pace, first) in [
            (1_000.0, 250_000),
            (5_000.0, 1_000_000),
            (12_000.0, LOD_PREVIEW_TIER_POINTS),
            // However fast the last passes were, a first pass stays: the
            // next view may be one that takes long to read in full.
            (15_000.0, LOD_PREVIEW_TIER_POINTS),
            (40_000.0, LOD_PREVIEW_TIER_POINTS),
        ] {
            assert_eq!(first_pass_budget(BUDGET, pace, None, 1), first, "{pace}");
        }
        let mut previous = 0;
        for step in 0..400 {
            let first = first_pass_budget(BUDGET, f64::from(step) * 100.0, None, 1);
            assert!(first >= LOD_FIRST_PASS_MIN && first >= previous, "{step}");
            previous = first;
        }
    }

    #[test]
    fn slow_geometry_build_caps_the_first_pass() {
        assert_eq!(
            first_pass_budget(BUDGET, 5_000.0, Some(10_000.0), 1),
            500_000
        );
        assert_eq!(
            first_pass_budget(BUDGET, 5_000.0, Some(2_000.0), 1),
            250_000
        );
        assert_eq!(first_pass_budget(BUDGET, 5_000.0, Some(1e9), 1), 1_000_000);
        assert_eq!(
            first_pass_budget(BUDGET, 20_000.0, Some(2_000.0), 1),
            250_000
        );
        assert_eq!(first_pass_budget(BUDGET, 12_000.0, None, 3), 2_400_000);
        // Never more than half of everything there is to show.
        assert_eq!(first_pass_budget(1_000_000, 40_000.0, None, 1), 500_000);
        assert_eq!(first_pass_budget(600_000, 1_000.0, Some(1.0), 1), 250_000);
    }

    #[test]
    fn small_budgets_and_small_clouds_stay_single_pass() {
        for effective in [1_000, 100_000, 250_000, 500_000] {
            for pace in [1.0, 1_250.0, 1e6] {
                for build in [None, Some(1.0)] {
                    assert_eq!(first_pass_budget(effective, pace, build, 1), effective);
                }
            }
        }
    }

    #[test]
    fn unmeasured_pace_gives_the_former_first_pass() {
        let pace = LodPace::default();
        assert_eq!(pace.read_points_per_ms(), LOD_SEED_POINTS_PER_MS);
        assert_eq!(pace.read_points_per_ms(), 1_250.0);
        assert_eq!(pace.build_points_per_ms(), None);
        assert_eq!(
            first_pass_budget(BUDGET, pace.read_points_per_ms(), None, 1),
            250_000
        );
        // The first measurement replaces the assumption instead of blending
        // with it.
        pace.record_read(1_000_000, Duration::from_millis(100));
        assert_eq!(pace.read_points_per_ms(), 10_000.0);
    }

    #[test]
    fn pace_falls_fast_rises_slowly_and_ignores_noise() {
        let pace = LodPace::default();
        pace.record_read(1_000_000, Duration::from_millis(100));
        pace.record_read(250_000, Duration::from_millis(250));
        assert_eq!(pace.read_points_per_ms(), 6_250.0);
        pace.record_read(4_000_000, Duration::from_millis(100));
        assert_eq!(pace.read_points_per_ms(), 10_937.5);
        pace.record_read(100_000, Duration::from_millis(1));
        pace.record_read(4_000_000, Duration::ZERO);
        assert_eq!(pace.read_points_per_ms(), 10_937.5);

        pace.record_build(10_000, Duration::from_millis(1));
        assert_eq!(pace.build_points_per_ms(), None);
        pace.record_build(1_000_000, Duration::ZERO);
        assert_eq!(pace.build_points_per_ms(), None);
        pace.record_build(1_000_000, Duration::from_millis(100));
        assert_eq!(pace.build_points_per_ms(), Some(10_000.0));
        assert_eq!(pace.read_points_per_ms(), 10_937.5);
    }

    #[test]
    fn slow_and_fast_machine_settle_on_different_sizes() {
        // A pass costs a fixed overhead plus a time per point, in
        // milliseconds and points per millisecond.
        let settle = |overhead: f64, speed: f64| {
            let pace = LodPace::default();
            let mut first = 0;
            for _ in 0..20 {
                first = first_pass_budget(BUDGET, pace.read_points_per_ms(), None, 1);
                let milliseconds = overhead + first as f64 / speed;
                pace.record_read(first, Duration::from_secs_f64(milliseconds / 1_000.0));
            }
            first
        };
        assert_eq!(settle(400.0, 2_000.0), 250_000);
        assert!(settle(60.0, 40_000.0) > 1_000_000);
    }

    #[test]
    fn preview_never_replaces_a_richer_picture() {
        let rich = ScreenFill::of(3_000_000, 200);
        assert!(!preview_improves(rich, ScreenFill::of(1_000_000, 200)));
        assert!(preview_improves(rich, ScreenFill::of(3_200_000, 200)));
        // The picture on screen misses more than half of the new view.
        assert!(preview_improves(
            ScreenFill::of(3_000_000, 90),
            ScreenFill::of(1_000_000, 200)
        ));
        assert!(!preview_improves(
            ScreenFill::of(3_000_000, 120),
            ScreenFill::of(1_000_000, 200)
        ));
        // More points, but thinner where the picture on screen has points.
        assert!(!preview_improves(
            ScreenFill::of(1_500_000, 120),
            ScreenFill::of(1_700_000, 240)
        ));
        assert!(preview_improves(
            ScreenFill::default(),
            ScreenFill::of(1, 1)
        ));

        let points = [0, 1, 250_000, 1_000_000, 3_000_000];
        let cells = [0, 1, 64, 128, 129, 256];
        for shown in points
            .iter()
            .flat_map(|points| cells.map(|cells| ScreenFill::of(*points, cells)))
        {
            for first in points {
                for reach in cells {
                    if !shown.holds(first, reach) {
                        continue;
                    }
                    for fresh in points
                        .iter()
                        .filter(|points| **points <= first)
                        .flat_map(|points| cells.map(|cells| ScreenFill::of(*points, cells)))
                        .filter(|fresh| fresh.cell_count() <= reach)
                    {
                        assert!(
                            !preview_improves(shown, fresh),
                            "{shown:?} {fresh:?} {first} {reach}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rich_screen_plans_one_full_pass() {
        let plan = |first, shown| plan_first_pass(BUDGET, first, shown, 256);
        assert_eq!(plan(1_000_000, ScreenFill::of(5_500_000, 200)), BUDGET);
        assert_eq!(plan(1_000_000, ScreenFill::of(50_000, 200)), 1_000_000);
        assert_eq!(plan(1_000_000, ScreenFill::of(5_500_000, 100)), 1_000_000);
        assert_eq!(plan(BUDGET, ScreenFill::of(5_500_000, 200)), BUDGET);
        assert_eq!(plan(BUDGET, ScreenFill::default()), BUDGET);
    }

    #[test]
    fn screen_fill_counts_what_the_camera_sees() {
        let scene = Bounds {
            min: [0.0; 3],
            max: [100.0; 3],
        };
        let points: Vec<[f64; 3]> = (0..100)
            .flat_map(|row| {
                (0..100).map(move |column| [50.0, f64::from(column) + 0.5, f64::from(row) + 0.5])
            })
            .collect();
        let fill = |projection, section| {
            let mut fill = ScreenFill::default();
            fill.add(&points, |xyz| *xyz, projection, section);
            fill
        };
        let framed = Projection::new(scene, 0.0, 0.0, 1.0, [0.0; 2], 800.0, 600.0);
        let offscreen = Projection::new(scene, 0.0, 0.0, 1.0, [2_000.0, 0.0], 800.0, 600.0);

        let whole = fill(framed, None);
        assert_eq!(whole.points, points.len());
        assert!(whole.cell_count() > 0);

        let away = fill(offscreen, None);
        assert_eq!((away.points, away.cell_count()), (0, 0));

        let half = fill(
            framed,
            Some(Bounds {
                min: [0.0; 3],
                max: [100.0, 50.0, 100.0],
            }),
        );
        assert!(
            (4_000..=6_000).contains(&half.points),
            "{} points",
            half.points
        );
        assert!(half.cell_count() < whole.cell_count());

        let other_half = Bounds {
            min: [0.0, 50.0, 0.0],
            max: [100.0; 3],
        };
        let mut both = half;
        both.add(&points, |xyz| *xyz, framed, Some(other_half));
        assert_eq!(
            both.points,
            half.points + fill(framed, Some(other_half)).points
        );
        assert_eq!(both.cell_count(), whole.cell_count());
        let before = both.points;
        both.add(&points[..0], |xyz| *xyz, framed, None);
        assert_eq!(both.points, before);

        let mut reach = ScreenFill::default();
        reach.add_box(framed, scene);
        assert_eq!(reach.points, 0);
        assert!(reach.cell_count() >= whole.cell_count());
        let mut nothing = ScreenFill::default();
        nothing.add_box(offscreen, scene);
        assert_eq!(nothing.cell_count(), 0);
    }

    #[test]
    fn screen_fill_looks_closer_at_a_set_mostly_out_of_view() {
        let scene = Bounds {
            min: [0.0; 3],
            max: [100.0; 3],
        };
        let framed = Projection::new(scene, 0.0, 0.0, 1.0, [0.0; 2], 800.0, 600.0);
        // A large set as it is left on screen by a strong zoom-in: 3 % of it
        // spread over the whole view, the rest far outside.
        const IN_VIEW: u32 = 60_000;
        let items: Vec<u32> = (0..2_000_000).collect();
        let position = |item: &u32| {
            if *item >= IN_VIEW {
                return [50.0, 5_000.0, 50.0];
            }
            let spread = |ratio: f64| (f64::from(*item) * ratio).fract() * 100.0;
            [50.0, spread(0.754_877_666), spread(0.569_840_291)]
        };

        let mut dense = ScreenFill::default();
        dense.add(&items[..IN_VIEW as usize], position, framed, None);
        assert_eq!(dense.points, IN_VIEW as usize);
        assert!(dense.cell_count() > 60, "{} cells", dense.cell_count());

        let mut sparse = ScreenFill::default();
        sparse.add(&items, position, framed, None);
        assert!(
            sparse.cell_count() * 10 >= dense.cell_count() * 9,
            "{} of {} cells",
            sparse.cell_count(),
            dense.cell_count()
        );
        assert!(
            (48_000..=72_000).contains(&sparse.points),
            "{} points",
            sparse.points
        );
    }
}
