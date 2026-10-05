//! One read of a whole scene into a volume of occupied cells, in the frame
//! of the building: what the levels, the footprint and later steps are
//! derived from without reading the points again.
//!
//! The volume holds one bit per cell of 5 by 5 cm and 2 cm high: whether a
//! point fell in it. Every column of cells also has its number of points
//! and its lowest and highest point in millimetres. The reading threads
//! share one volume and set its bits, add to the counts and lower or raise
//! the heights with atomic operations, so the result is the same whatever
//! the number of threads and whatever the order the points arrive in.
//!
//! After the read, groups of fewer than 20 occupied cells that touch no
//! other (counting the 26 neighbours of a cell) are noise: stray points in
//! the air and under the ground. They are taken out of the volume and
//! counted.
//!
//! The volume of a scene of 30 by 60 by 35 m takes about 160 MB. Above the
//! memory budget the columns become 7.5 cm, then 10 cm wide; a region that
//! does not fit at 10 cm is refused.

use std::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::frame::BuildingFrame;
use crate::grid2d::{GridFrame, Mask};
use crate::region_source::{visit_region_parallel, RegionFilter, RegionProgress, RegionSource};
use crate::{Bounds, LoadError, OrientedBox};

/// The memory a survey may take: its volume, the second volume that finding
/// the noise needs for a while, and the numbers per column.
pub const DEFAULT_SURVEY_BUDGET: u64 = 512 * 1024 * 1024;

/// How a survey reads a scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurveyConfig {
    /// Width of a column, in metres; grown by half and then doubled when the
    /// region does not fit the budget.
    pub cell_xy: f64,
    /// Height of a cell, in metres.
    pub cell_z: f64,
    pub budget_bytes: u64,
    /// A group of fewer occupied cells than this is noise.
    pub min_group_cells: u32,
}

impl Default for SurveyConfig {
    fn default() -> Self {
        Self {
            cell_xy: 0.05,
            cell_z: 0.02,
            budget_bytes: DEFAULT_SURVEY_BUDGET,
            min_group_cells: 20,
        }
    }
}

/// Where the cells of a survey lie in the frame of the building.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SurveyGrid {
    /// Plan position of the corner of column (0, 0), and the scene height of
    /// the bottom of the lowest cell.
    pub origin: [f64; 3],
    pub cell_xy: f64,
    pub cell_z: f64,
    /// Columns along u, columns along v, and cells in every column.
    pub size: [u32; 3],
}

impl SurveyGrid {
    pub fn columns(&self) -> usize {
        self.size[0] as usize * self.size[1] as usize
    }

    /// The position of column `(x, y)`, row by row from `y = 0`.
    pub fn column(&self, x: u32, y: u32) -> usize {
        y as usize * self.size[0] as usize + x as usize
    }

    /// The column that holds a plan position, or nothing outside the grid.
    pub fn column_of(&self, uv: [f64; 2]) -> Option<[u32; 2]> {
        let x = ((uv[0] - self.origin[0]) / self.cell_xy).floor();
        let y = ((uv[1] - self.origin[1]) / self.cell_xy).floor();
        // Written so that a NaN fails the test.
        (x >= 0.0 && y >= 0.0 && x < self.size[0] as f64 && y < self.size[1] as f64)
            .then_some([x as u32, y as u32])
    }

    /// The cell of a column that holds a scene height, or nothing outside.
    pub fn bin_of(&self, z: f64) -> Option<u32> {
        let bin = ((z - self.origin[2]) / self.cell_z).floor();
        (bin >= 0.0 && bin < self.size[2] as f64).then_some(bin as u32)
    }

    /// The scene height of the bottom of a cell of a column.
    pub fn bin_bottom(&self, bin: u32) -> f64 {
        self.origin[2] + bin as f64 * self.cell_z
    }

    /// The scene height of the middle of a cell of a column.
    pub fn bin_center(&self, bin: u32) -> f64 {
        self.origin[2] + (bin as f64 + 0.5) * self.cell_z
    }

    /// The plan position of the middle of a column.
    pub fn column_center(&self, x: u32, y: u32) -> [f64; 2] {
        [
            self.origin[0] + (x as f64 + 0.5) * self.cell_xy,
            self.origin[1] + (y as f64 + 0.5) * self.cell_xy,
        ]
    }

    /// The columns as the grid of a plan mask.
    pub fn plan_grid(&self) -> GridFrame {
        GridFrame {
            origin: [self.origin[0], self.origin[1]],
            cell: self.cell_xy,
            width: self.size[0],
            height: self.size[1],
        }
    }

    /// 64-bit words of the volume per column.
    fn words(&self) -> usize {
        (self.size[2] as usize).div_ceil(64)
    }

    /// The memory of a survey on this grid: the volume, the volume that
    /// finding the noise needs beside it, and three numbers per column.
    pub fn bytes(&self) -> u64 {
        let columns = self.columns() as u64;
        columns * self.words() as u64 * 8 * 2 + columns * 12
    }
}

/// What a survey read and found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SurveyStats {
    /// Points read from the sources.
    pub points_read: u64,
    /// Points that fell in the volume.
    pub points: u64,
    /// Occupied cells, the noise not counted.
    pub occupied_cells: u64,
    /// Cells taken out as noise, and the groups they formed.
    pub noise_cells: u64,
    pub noise_groups: u64,
    /// The memory of the survey, see `SurveyGrid::bytes`.
    pub bytes: u64,
}

/// The occupied cells of a scene in the frame of its building, with the
/// numbers of every column. It lives as long as the steps that use it and is
/// not saved; `SurveySummary` is what is kept.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneSurvey {
    pub frame: BuildingFrame,
    pub grid: SurveyGrid,
    /// Per column `words` words; bit `b` of word `w` is the cell `64 w + b`.
    occupancy: Vec<u64>,
    counts: Vec<u32>,
    /// Lowest and highest point per column in millimetres above the bottom
    /// of the grid; `i32::MAX` and `i32::MIN` in an empty column.
    low_mm: Vec<i32>,
    high_mm: Vec<i32>,
    pub stats: SurveyStats,
}

/// The grid over a region in the frame, with its cells grown until it fits
/// the budget.
fn plan_grid(
    region: &OrientedBox,
    frame: &BuildingFrame,
    config: &SurveyConfig,
) -> Result<SurveyGrid, LoadError> {
    if !region.is_valid()
        || !(config.cell_xy.is_finite() && config.cell_xy > 0.0)
        || !(config.cell_z.is_finite() && config.cell_z > 0.0)
    {
        return Err(LoadError::InvalidData(
            "a survey needs a region and cells of a positive size".into(),
        ));
    }
    let corners = region.corners();
    let around = frame.frame_bounds(corners).expect("a box has corners");
    let (z_low, z_high) = (region.bounds.min[2], region.bounds.max[2]);
    let mut last = 0;
    for growth in [1.0, 1.5, 2.0] {
        let cell = config.cell_xy * growth;
        let count = |span: f64, cell: f64| (span / cell).floor() + 1.0;
        let size = [
            count(around.max[0] - around.min[0], cell),
            count(around.max[1] - around.min[1], cell),
            count(z_high - z_low, config.cell_z),
        ];
        if size.iter().any(|count| *count > u32::MAX as f64 / 2.0) {
            continue;
        }
        let grid = SurveyGrid {
            origin: [around.min[0], around.min[1], z_low],
            cell_xy: cell,
            cell_z: config.cell_z,
            size: size.map(|count| count as u32),
        };
        last = grid.bytes();
        if last <= config.budget_bytes {
            return Ok(grid);
        }
    }
    Err(LoadError::InvalidData(format!(
        "the region needs {} MB for a survey even with columns of {} m, more than the budget of {} MB; choose a smaller region",
        last.div_ceil(1 << 20),
        config.cell_xy * 2.0,
        config.budget_bytes / (1 << 20)
    )))
}

/// Read the points of a region into a survey in the frame of the building.
///
/// - `region` is the part of the scene to read, a box that may be turned
///   otherwise than the frame; the grid covers it in the frame.
/// - `accept` is as in `visit_region`: deleted points, hidden classes.
/// - `progress` is as in `visit_region_parallel`; an error from it stops
///   the read.
///
/// The leaves of the indexes are read on the threads of the current rayon
/// pool. The result is the same for any number of threads.
pub fn survey_scene(
    sources: &[RegionSource<'_>],
    region: OrientedBox,
    frame: &BuildingFrame,
    accept: &RegionFilter<'_>,
    config: &SurveyConfig,
    progress: &mut (dyn FnMut(RegionProgress) -> Result<(), LoadError> + Send),
) -> Result<SceneSurvey, LoadError> {
    let grid = plan_grid(&region, frame, config)?;
    let columns = grid.columns();
    let words = grid.words();
    let occupancy: Vec<AtomicU64> = (0..columns * words).map(|_| AtomicU64::new(0)).collect();
    let counts: Vec<AtomicU32> = (0..columns).map(|_| AtomicU32::new(0)).collect();
    let low: Vec<AtomicI32> = (0..columns).map(|_| AtomicI32::new(i32::MAX)).collect();
    let high: Vec<AtomicI32> = (0..columns).map(|_| AtomicI32::new(i32::MIN)).collect();
    let inside = |source: usize, ordinal: u64, point: &crate::Point| {
        region.contains(point.xyz) && accept(source, ordinal, point)
    };
    let (_, read) = visit_region_parallel(
        sources,
        region.aabb(),
        &inside,
        progress,
        &|| (),
        &|_: &mut (), _: usize, batch: &[crate::IndexedPoint]| {
            for record in batch {
                let xyz = record.point.xyz;
                let Some([x, y]) = grid.column_of(frame.to_plan([xyz[0], xyz[1]])) else {
                    continue;
                };
                let Some(bin) = grid.bin_of(xyz[2]) else {
                    continue;
                };
                let column = grid.column(x, y);
                occupancy[column * words + bin as usize / 64]
                    .fetch_or(1 << (bin % 64), Ordering::Relaxed);
                counts[column].fetch_add(1, Ordering::Relaxed);
                let millimetres = ((xyz[2] - grid.origin[2]) * 1000.0).round() as i32;
                low[column].fetch_min(millimetres, Ordering::Relaxed);
                high[column].fetch_max(millimetres, Ordering::Relaxed);
            }
            Ok(())
        },
    )?;
    let mut survey = SceneSurvey {
        frame: *frame,
        grid,
        occupancy: occupancy.into_iter().map(AtomicU64::into_inner).collect(),
        counts: counts.into_iter().map(AtomicU32::into_inner).collect(),
        low_mm: low.into_iter().map(AtomicI32::into_inner).collect(),
        high_mm: high.into_iter().map(AtomicI32::into_inner).collect(),
        stats: SurveyStats {
            points_read: read.read,
            bytes: grid.bytes(),
            ..SurveyStats::default()
        },
    };
    survey.stats.points = survey.counts.iter().map(|count| *count as u64).sum();
    let (cells, groups) = survey.remove_noise(config.min_group_cells);
    survey.stats.noise_cells = cells;
    survey.stats.noise_groups = groups;
    survey.stats.occupied_cells = survey
        .occupancy
        .iter()
        .map(|word| word.count_ones() as u64)
        .sum();
    Ok(survey)
}

impl SceneSurvey {
    fn words(&self) -> usize {
        self.grid.words()
    }

    /// Whether the cell `bin` of column `(x, y)` is occupied.
    pub fn occupied(&self, x: u32, y: u32, bin: u32) -> bool {
        let column = self.grid.column(x, y);
        self.occupancy[column * self.words() + bin as usize / 64] & (1 << (bin % 64)) != 0
    }

    /// The words of the cells of a column.
    fn column_words(&self, column: usize) -> &[u64] {
        let words = self.words();
        &self.occupancy[column * words..(column + 1) * words]
    }

    /// The occupied cells of a column, from the bottom.
    pub fn column_bins(&self, column: usize) -> impl Iterator<Item = u32> + '_ {
        self.column_words(column)
            .iter()
            .enumerate()
            .flat_map(|(index, word)| {
                let mut word = *word;
                std::iter::from_fn(move || {
                    (word != 0).then(|| {
                        let bit = word.trailing_zeros();
                        word &= word - 1;
                        index as u32 * 64 + bit
                    })
                })
            })
    }

    /// How many cells of a column are occupied.
    pub fn occupied_bins(&self, column: usize) -> u32 {
        self.column_words(column)
            .iter()
            .map(|word| word.count_ones())
            .sum()
    }

    /// The lowest occupied cell of a column.
    pub fn lowest_bin(&self, column: usize) -> Option<u32> {
        self.column_bins(column).next()
    }

    /// The highest occupied cell of a column.
    pub fn highest_bin(&self, column: usize) -> Option<u32> {
        let words = self.column_words(column);
        words.iter().enumerate().rev().find_map(|(index, word)| {
            (*word != 0).then(|| index as u32 * 64 + 63 - word.leading_zeros())
        })
    }

    /// The nearest occupied cell above `bin` in a column, and the nearest
    /// below it.
    pub fn neighbours_in_column(&self, column: usize, bin: u32) -> (Option<u32>, Option<u32>) {
        let words = self.column_words(column);
        let (word, bit) = (bin as usize / 64, bin % 64);
        let above_mask = if bit == 63 { 0 } else { !0u64 << (bit + 1) };
        let above = std::iter::once((word, words[word] & above_mask))
            .chain((word + 1..words.len()).map(|index| (index, words[index])))
            .find(|(_, value)| *value != 0)
            .map(|(index, value)| index as u32 * 64 + value.trailing_zeros());
        let below_mask = (1u64 << bit) - 1;
        let below = std::iter::once((word, words[word] & below_mask))
            .chain((0..word).rev().map(|index| (index, words[index])))
            .find(|(_, value)| *value != 0)
            .map(|(index, value)| index as u32 * 64 + 63 - value.leading_zeros());
        (above, below)
    }

    /// Points that fell in a column.
    pub fn count(&self, column: usize) -> u32 {
        self.counts[column]
    }

    /// The lowest and the highest point of a column as scene heights, the
    /// noise included; nothing for an empty column.
    pub fn point_range(&self, column: usize) -> Option<[f64; 2]> {
        (self.counts[column] > 0).then(|| {
            [self.low_mm[column], self.high_mm[column]]
                .map(|millimetres| self.grid.origin[2] + millimetres as f64 * 0.001)
        })
    }

    /// Per cell height, the number of columns occupied at that height, over
    /// all columns or over those a plan mask on `SurveyGrid::plan_grid`
    /// holds. Times the area of a column, it is the horizontal area found at
    /// every height: it does not depend on how densely a face was scanned.
    pub fn area_histogram(&self, within: Option<&Mask>) -> Vec<u32> {
        let mut histogram = vec![0u32; self.grid.size[2] as usize];
        for column in 0..self.grid.columns() {
            if within.is_some_and(|mask| !mask.cells()[column]) {
                continue;
            }
            for bin in self.column_bins(column) {
                histogram[bin as usize] += 1;
            }
        }
        histogram
    }

    /// A digest of the occupied cells and of the numbers of every column,
    /// FNV-1a over their bytes: equal surveys have equal digests.
    pub fn digest(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut add = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        for word in &self.occupancy {
            add(&word.to_le_bytes());
        }
        for column in 0..self.counts.len() {
            add(&self.counts[column].to_le_bytes());
            add(&self.low_mm[column].to_le_bytes());
            add(&self.high_mm[column].to_le_bytes());
        }
        hash
    }

    /// Take the groups of fewer than `min_cells` occupied cells that touch
    /// no other out of the volume, counting the 26 neighbours of a cell.
    /// Returns the cells taken out and the groups they formed. The cells are
    /// visited in the order of the volume, so the result does not depend on
    /// anything but the volume.
    fn remove_noise(&mut self, min_cells: u32) -> (u64, u64) {
        if min_cells <= 1 {
            return (0, 0);
        }
        let words = self.words();
        let [width, height, bins] = self.grid.size;
        let mut visited = vec![0u64; self.occupancy.len()];
        let bit =
            |column: usize, bin: u32| (column * words + bin as usize / 64, 1u64 << (bin % 64));
        let mut pending: Vec<(u32, u32, u32)> = Vec::new();
        let mut group: Vec<(usize, u32)> = Vec::new();
        let (mut noise_cells, mut noise_groups) = (0u64, 0u64);
        for y in 0..height {
            for x in 0..width {
                let column = self.grid.column(x, y);
                let starts: Vec<u32> = self.column_bins(column).collect();
                for start in starts {
                    let (word, mask) = bit(column, start);
                    if visited[word] & mask != 0 {
                        continue;
                    }
                    visited[word] |= mask;
                    pending.push((x, y, start));
                    group.clear();
                    let mut size = 0u32;
                    while let Some((x, y, bin)) = pending.pop() {
                        size = size.saturating_add(1);
                        if size < min_cells {
                            group.push((self.grid.column(x, y), bin));
                        }
                        for dz in -1i64..=1 {
                            let nz = bin as i64 + dz;
                            if nz < 0 || nz >= bins as i64 {
                                continue;
                            }
                            for dy in -1i64..=1 {
                                let ny = y as i64 + dy;
                                if ny < 0 || ny >= height as i64 {
                                    continue;
                                }
                                for dx in -1i64..=1 {
                                    let nx = x as i64 + dx;
                                    if nx < 0 || nx >= width as i64 || (dx, dy, dz) == (0, 0, 0) {
                                        continue;
                                    }
                                    let neighbour = self.grid.column(nx as u32, ny as u32);
                                    let (word, mask) = bit(neighbour, nz as u32);
                                    if self.occupancy[word] & mask != 0 && visited[word] & mask == 0
                                    {
                                        visited[word] |= mask;
                                        pending.push((nx as u32, ny as u32, nz as u32));
                                    }
                                }
                            }
                        }
                    }
                    if size < min_cells {
                        for (column, bin) in &group {
                            let (word, mask) = bit(*column, *bin);
                            self.occupancy[word] &= !mask;
                        }
                        noise_cells += size as u64;
                        noise_groups += 1;
                    }
                }
            }
        }
        (noise_cells, noise_groups)
    }
}

/// What is kept of a survey with the project: the cells, the horizontal area
/// per height and what was left out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurveySummary {
    pub grid: SurveyGrid,
    /// Columns occupied per cell height over the whole grid, from the bottom
    /// of the grid; see `SceneSurvey::area_histogram`.
    pub area_histogram: Vec<u32>,
    pub stats: SurveyStats,
    /// See `SceneSurvey::digest`.
    pub digest: u64,
}

impl SceneSurvey {
    pub fn summary(&self) -> SurveySummary {
        SurveySummary {
            grid: self.grid,
            area_histogram: self.area_histogram(None),
            stats: self.stats,
            digest: self.digest(),
        }
    }

    /// The box of the scene the grid covers, in the frame.
    pub fn frame_bounds(&self) -> Bounds {
        let grid = &self.grid;
        Bounds {
            min: [
                grid.origin[0],
                grid.origin[1],
                grid.origin[2] - self.frame.peil_z,
            ],
            max: [
                grid.origin[0] + grid.size[0] as f64 * grid.cell_xy,
                grid.origin[1] + grid.size[1] as f64 * grid.cell_xy,
                grid.origin[2] + grid.size[2] as f64 * grid.cell_z - self.frame.peil_z,
            ],
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::region_source::SourceTransform;
    use crate::test_shapes::{building, BuildingSpec};
    use crate::IndexedPoint;

    pub(crate) fn on_threads<T: Send>(threads: usize, work: impl FnOnce() -> T + Send) -> T {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .unwrap()
            .install(work)
    }

    /// The points of a scan in memory, as a layer without an index.
    pub(crate) fn resident(points: &[[f64; 3]]) -> Vec<IndexedPoint> {
        points
            .iter()
            .enumerate()
            .map(|(ordinal, xyz)| IndexedPoint {
                point: crate::Point {
                    xyz: *xyz,
                    rgb: None,
                    intensity: None,
                    classification: None,
                },
                ordinal: ordinal as u64,
            })
            .collect()
    }

    /// The frame of a generated building: along its walls, with its origin
    /// at the corner of its plan.
    pub(crate) fn true_frame(spec: &BuildingSpec) -> BuildingFrame {
        let corner = spec.to_scene([0.0, 0.0, 0.0]);
        BuildingFrame::new(spec.rotation_degrees, [corner[0], corner[1]])
    }

    /// The building and its ground, without the strays far below, as a box
    /// turned along the building.
    pub(crate) fn site_box(spec: &BuildingSpec) -> OrientedBox {
        let frame = true_frame(spec);
        let margin = spec.site_margin + 0.5;
        let z0 = spec.translation[2];
        frame.oriented_box(Bounds {
            min: [-margin, -margin, spec.ground_z() + z0 - 1.0],
            max: [
                spec.size[0] + margin,
                spec.size[1] + margin,
                spec.roof_z() + z0 + 1.0,
            ],
        })
    }

    fn everything(_: usize, _: u64, _: &crate::Point) -> bool {
        true
    }

    #[test]
    fn a_survey_is_the_same_for_one_and_many_threads_and_keeps_to_its_budget() {
        let spec = BuildingSpec {
            spacing: 0.04,
            ..BuildingSpec::default()
        };
        let scan = building(&spec);
        let points = resident(&scan.points);
        let source = RegionSource::resident(&points, SourceTransform::default());
        let frame = true_frame(&spec);
        let region = site_box(&spec);
        let config = SurveyConfig::default();
        let run = || {
            survey_scene(&[source], region, &frame, &everything, &config, &mut |_| {
                Ok(())
            })
            .unwrap()
        };
        let first = on_threads(1, run);
        for threads in [2, 5, 8] {
            let other = on_threads(threads, run);
            assert_eq!(other.digest(), first.digest(), "{threads}");
            assert!(other == first, "{threads}");
        }
        assert_eq!((first.grid.cell_xy, first.grid.cell_z), (0.05, 0.02));
        assert!(first.stats.bytes <= config.budget_bytes);
        // Every point of the building lies in the region; the strays far
        // below do not.
        let kept = scan
            .points
            .iter()
            .filter(|at| at[2] > spec.ground_z() + spec.translation[2] - 1.0)
            .count();
        assert_eq!(first.stats.points as usize, kept);
        assert_eq!(first.stats.points_read as usize, scan.points.len());

        // The horizontal area per height: the floors stand out, each about
        // as large as it is.
        let histogram = first.area_histogram(None);
        let area = |z: f64| {
            let bin = first.grid.bin_of(z + spec.translation[2]).unwrap() as usize;
            (bin - 1..=bin + 1).map(|bin| histogram[bin]).max().unwrap() as f64
                * first.grid.cell_xy
                * first.grid.cell_xy
        };
        for storey in 0..3 {
            let floor = area(spec.floor_z(storey));
            assert!(floor > 70.0 && floor < 90.0, "{storey}: {floor}");
        }
        let roof = area(spec.roof_z());
        assert!(roof > 85.0 && roof < 100.0, "{roof}");
        // Halfway up a storey only the walls are cut.
        assert!(area(1.6) < 15.0);
        // The columns know their points: over a patch of the plan inside,
        // from the floor of the ground floor up to the roof. Not every column
        // of 5 cm holds a point of every face.
        let (mut low, mut high, mut points) = (f64::INFINITY, f64::NEG_INFINITY, 0u64);
        let [x0, y0] = first.grid.column_of([6.0, 3.0]).unwrap();
        for y in y0..y0 + 6 {
            for x in x0..x0 + 6 {
                let column = first.grid.column(x, y);
                let Some([bottom, top]) = first.point_range(column) else {
                    continue;
                };
                (low, high) = (low.min(bottom), high.max(top));
                points += first.count(column) as u64;
                // Inside the building nothing is noise, so the lowest and
                // highest occupied cells hold the lowest and highest point.
                let lowest = first.grid.bin_center(first.lowest_bin(column).unwrap());
                let highest = first.grid.bin_center(first.highest_bin(column).unwrap());
                assert!((lowest - bottom).abs() <= first.grid.cell_z);
                assert!((highest - top).abs() <= first.grid.cell_z);
                assert_eq!(
                    first.column_bins(column).count() as u32,
                    first.occupied_bins(column)
                );
            }
        }
        assert!((low - (spec.floor_z(0) + spec.translation[2])).abs() < 0.01);
        assert!((high - (spec.roof_z() + spec.translation[2])).abs() < 0.01);
        assert!(points > 100);
    }

    #[test]
    fn small_groups_of_cells_are_noise_and_leave_the_volume() {
        // A floor of 2 by 2 m and, above it, single points and a group of
        // 19 cells and one of 20.
        let mut points: Vec<[f64; 3]> = Vec::new();
        for x in 0..80 {
            for y in 0..80 {
                points.push([x as f64 * 0.025 + 0.01, y as f64 * 0.025 + 0.01, 0.005]);
            }
        }
        let loose = [[0.52, 0.52, 1.01], [1.52, 0.52, 1.51], [0.52, 1.52, 1.91]];
        points.extend(loose);
        for step in 0..19 {
            points.push([0.12 + step as f64 * 0.05, 1.82, 1.21]);
        }
        for step in 0..20 {
            points.push([0.12 + step as f64 * 0.05, 1.22, 1.61]);
        }
        let records = resident(&points);
        let source = RegionSource::resident(&records, SourceTransform::default());
        let region = OrientedBox::from(Bounds {
            min: [0.0, 0.0, 0.0],
            max: [2.0, 2.0, 2.0],
        });
        let survey = survey_scene(
            &[source],
            region,
            &BuildingFrame::new(0.0, [0.0, 0.0]),
            &everything,
            &SurveyConfig::default(),
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_eq!(survey.grid.size, [41, 41, 101]);
        assert_eq!(survey.stats.noise_groups, 4);
        assert_eq!(survey.stats.noise_cells, 3 + 19);
        assert_eq!(survey.stats.occupied_cells, 40 * 40 + 20);
        let histogram = survey.area_histogram(None);
        assert_eq!(histogram[0], 1600);
        assert_eq!(histogram[80], 20);
        assert_eq!(histogram.iter().sum::<u32>(), 1620);
        // The counts and heights of a column keep the noise.
        let [x, y] = survey.grid.column_of([0.52, 0.52]).unwrap();
        let column = survey.grid.column(x, y);
        // Four points of the floor and the loose one.
        assert_eq!(survey.count(column), 5);
        let [low, high] = survey.point_range(column).unwrap();
        assert!((low - 0.005).abs() < 1e-9 && (high - 1.01).abs() < 1e-9);
        assert_eq!(survey.highest_bin(column), Some(0));
        assert_eq!(survey.neighbours_in_column(column, 0), (None, None));
    }

    #[test]
    fn a_region_too_large_for_the_budget_gets_wider_columns_or_is_refused() {
        let region = OrientedBox::from(Bounds {
            min: [0.0, 0.0, 0.0],
            max: [40.0, 40.0, 20.0],
        });
        let frame = BuildingFrame::new(30.0, [20.0, 20.0]);
        let at = |budget: u64| {
            plan_grid(
                &region,
                &frame,
                &SurveyConfig {
                    budget_bytes: budget,
                    ..SurveyConfig::default()
                },
            )
        };
        let full = at(DEFAULT_SURVEY_BUDGET).unwrap();
        // Turned by 30 degrees, the box needs a larger square in the frame.
        let side = 40.0 * (30f64.to_radians().cos() + 30f64.to_radians().sin());
        assert!((full.size[0] as f64 * 0.05 - side).abs() < 0.06);
        assert_eq!(full.size[2], 1001);
        assert_eq!(full.cell_xy, 0.05);
        let wider = at(full.bytes() - 1).unwrap();
        assert_eq!(wider.cell_xy, 0.05 * 1.5);
        let widest = at(wider.bytes() - 1).unwrap();
        assert_eq!(widest.cell_xy, 0.1);
        assert!(widest.bytes() < wider.bytes() && wider.bytes() < full.bytes());
        assert!(matches!(
            at(widest.bytes() - 1),
            Err(LoadError::InvalidData(_))
        ));
    }

    #[test]
    fn neighbours_in_a_column_cross_words() {
        let mut survey = SceneSurvey {
            frame: BuildingFrame::new(0.0, [0.0, 0.0]),
            grid: SurveyGrid {
                origin: [0.0; 3],
                cell_xy: 0.05,
                cell_z: 0.02,
                size: [1, 1, 200],
            },
            occupancy: vec![0; 4],
            counts: vec![0],
            low_mm: vec![i32::MAX],
            high_mm: vec![i32::MIN],
            stats: SurveyStats::default(),
        };
        for bin in [3u32, 63, 64, 130] {
            survey.occupancy[bin as usize / 64] |= 1 << (bin % 64);
        }
        assert_eq!(survey.column_bins(0).collect::<Vec<_>>(), [3, 63, 64, 130]);
        assert_eq!(survey.neighbours_in_column(0, 3), (Some(63), None));
        assert_eq!(survey.neighbours_in_column(0, 63), (Some(64), Some(3)));
        assert_eq!(survey.neighbours_in_column(0, 100), (Some(130), Some(64)));
        assert_eq!(survey.neighbours_in_column(0, 130), (None, Some(64)));
        assert_eq!(survey.lowest_bin(0), Some(3));
        assert_eq!(survey.highest_bin(0), Some(130));
        assert_eq!(survey.occupied_bins(0), 4);
        assert_eq!(survey.point_range(0), None);
    }
}
