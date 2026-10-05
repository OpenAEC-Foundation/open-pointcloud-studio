//! The levels of a building: its floors, ceilings, slabs and roof, found in
//! the horizontal area per height of a survey and refined from the points.
//!
//! Within the footprint, without the columns of the walls, every height of
//! 2 cm counts the columns that hold an occupied cell there. A floor or a
//! ceiling is a sharp peak in that count. Per peak the columns tell which way
//! the surface faces: a floor has 1.5 m of free space above it up to a
//! ceiling, a ceiling has as much free space below it down to a floor. A
//! chain of rules then names the peaks: floor to ceiling is a storey, ceiling
//! to the next floor is a slab, a small surface just above a floor is
//! furniture, two ceilings over one floor are a suspended ceiling under the
//! slab, and the highest surface open to the sky is the roof. The ground
//! comes from the lowest occupied cells in a band around the footprint, and
//! P, the level that is zero in every drawing, is the floor nearest above the
//! ground that covers at least half the footprint.
//!
//! `refine_levels` reads the points within 5 cm of every floor once more, in
//! whole bins of 2 mm: the median is the height of the floor, and the
//! medians of its halves tell how much it slopes. The plan of a storey is cut
//! 1.20 m above its floor, a little higher or lower where a surface such as a
//! counter lies at that height.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use super::survey::{Footprint, SceneSurvey};
use super::{Confidence, Criterion};
use crate::grid2d::Mask;
use crate::region_source::{visit_region_parallel, RegionFilter, RegionProgress, RegionSource};
use crate::{Bounds, IndexedPoint, LoadError, Point};

/// How `detect_levels` reads the area per height. Lengths in metres, areas
/// in square metres, shares from 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelConfig {
    /// A peak needs at least this area, and at least `min_share` of the
    /// footprint.
    pub min_area: f64,
    pub min_share: f64,
    /// Of two peaks nearer than this, the larger one is kept.
    pub separation: f64,
    /// A peak wider than this at half its height is no level surface.
    pub max_width: f64,
    /// A column with this much occupied height is a wall and counts for no
    /// level.
    pub wall_height: f64,
    /// Free space above a floor or below a ceiling.
    pub free_height: f64,
    /// From a floor to its ceiling.
    pub storey: [f64; 2],
    /// From a ceiling to the floor above, through the slab.
    pub slab: [f64; 2],
    /// A slab needs the ceiling and the floor above to overlap this much.
    pub slab_overlap: f64,
    /// A surface this high above a floor, with less than `furniture_share`
    /// of its area, is furniture.
    pub furniture: [f64; 2],
    pub furniture_share: f64,
    /// A floor covering less than this share of the footprint is partial.
    pub partial_share: f64,
    /// The ground is looked for between these distances outside the
    /// footprint.
    pub ground_band: [f64; 2],
    /// P covers at least this share of the footprint.
    pub peil_share: f64,
    /// The plan of a storey is cut this high above its floor.
    pub cut_height: f64,
}

impl Default for LevelConfig {
    fn default() -> Self {
        Self {
            min_area: 4.0,
            min_share: 0.10,
            separation: 0.10,
            max_width: 0.08,
            wall_height: 1.0,
            free_height: 1.5,
            storey: [2.1, 6.0],
            slab: [0.12, 0.90],
            slab_overlap: 0.5,
            furniture: [0.3, 1.3],
            furniture_share: 0.6,
            partial_share: 0.6,
            ground_band: [0.5, 3.0],
            peil_share: 0.5,
            cut_height: 1.2,
        }
    }
}

/// Which way a level surface faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Facing {
    /// Seen from above: a floor, a desk, a roof.
    Up,
    /// Seen from below: a ceiling.
    Down,
    Unclear,
}

/// What a peak of the area per height turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeakRole {
    Floor,
    PartialFloor,
    Roof,
    Ceiling,
    /// The lower of two ceilings over one floor.
    LoweredCeiling,
    /// The higher of two ceilings over one floor.
    SlabUnderside,
    /// Desks, sills, counters: just above a floor and smaller than it.
    Furniture,
    Unused,
}

/// A peak of the horizontal area per height within the footprint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelPeak {
    /// Scene height: the mean of the cells of the peak.
    pub z: f64,
    /// Area of the columns occupied at the peak, and its share of the
    /// footprint.
    pub area: f64,
    pub share: f64,
    /// Width at half the height of the peak above what lies around it.
    pub width: f64,
    /// Shares of the columns with free space above up to something, with
    /// nothing at all above, with free space below down to something, and
    /// with nothing below.
    pub free_above: f64,
    pub open_above: f64,
    pub free_below: f64,
    pub open_below: f64,
    pub facing: Facing,
    pub role: PeakRole,
    /// The columns occupied at the peak, ascending.
    #[serde(skip)]
    columns: Vec<u32>,
}

/// What a level is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LevelKind {
    /// Below P.
    Basement,
    /// The level of P.
    Ground,
    /// Above P.
    Storey,
    /// A floor over part of a storey, such as a mezzanine.
    Partial,
    Roof,
}

/// Whether a level is as found or as the user changed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LevelStatus {
    Found,
    Edited,
}

/// One level of a building. Heights are scene heights in metres.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    /// "00" for the level of P, "01" and up above it, "-01" and down below
    /// it; a partial floor has the code of the storey it stands in with
    /// "M", and the roof is "R".
    pub id: String,
    pub name: String,
    pub kind: LevelKind,
    /// The floor, or the top of the roof.
    pub floor_z: f64,
    /// The ceiling over the floor; the lower one where there are two.
    pub ceiling_z: Option<f64>,
    /// The underside of the slab above a suspended ceiling.
    pub slab_underside: Option<f64>,
    /// From the ceiling, or the slab underside, to the floor above.
    pub slab_thickness: Option<f64>,
    /// Height of the cut of the plan above the floor.
    pub cut_height: f64,
    /// How much the floor rises along u and along v, in millimetres per
    /// metre; known once the level is refined.
    pub tilt_mm_per_m: Option<[f64; 2]>,
    /// Share of the footprint the floor covers.
    pub share: f64,
    pub is_peil: bool,
    pub confidence: Confidence,
    pub status: LevelStatus,
}

impl Level {
    /// Whether the level is a floor of a whole storey, P included.
    pub fn is_storey(&self) -> bool {
        matches!(
            self.kind,
            LevelKind::Basement | LevelKind::Ground | LevelKind::Storey
        )
    }

    /// The scene height of the cut of its plan.
    pub fn cut_z(&self) -> f64 {
        self.floor_z + self.cut_height
    }
}

/// The levels of a building with what they were found from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LevelDetection {
    /// From the lowest up, the roof last.
    pub levels: Vec<Level>,
    /// The ground around the footprint, and the share of the band around it
    /// that holds points.
    pub ground_z: Option<f64>,
    pub ground_share: f64,
    /// The height of P: the floor of the level that is P.
    pub peil_z: Option<f64>,
    pub peaks: Vec<LevelPeak>,
    /// Columns occupied per cell height within the footprint, the walls
    /// left out, from the bottom of the survey grid.
    pub area_histogram: Vec<u32>,
}

impl LevelDetection {
    /// The floors that are not partial, from the lowest up.
    pub fn floors(&self) -> impl Iterator<Item = &Level> {
        self.levels.iter().filter(|level| level.is_storey())
    }

    pub fn roof(&self) -> Option<&Level> {
        self.levels
            .iter()
            .find(|level| level.kind == LevelKind::Roof)
    }
}

/// A value that is 1 up to `good`, 0 from `bad` on and straight between.
fn ramp(value: f64, good: f64, bad: f64) -> f64 {
    ((bad - value) / (bad - good)).clamp(0.0, 1.0)
}

/// The confidence of a level: how sharp its peak is, how much of the
/// footprint it covers and whether the height of its storey is a usual one.
fn level_confidence(peak: &LevelPeak, storey_height: Option<f64>) -> Confidence {
    let sharpness = ramp(peak.width, 0.04, 0.15);
    let area = (peak.share / 0.4).clamp(0.0, 1.0);
    let consistency = match storey_height {
        Some(height) if !(2.4..=4.5).contains(&height) => 0.6,
        _ => 1.0,
    };
    Confidence::of(vec![
        (Criterion::Sharpness, sharpness as f32),
        (Criterion::Area, area as f32),
        (Criterion::Consistency, consistency as f32),
    ])
}

/// How many of two ascending lists of columns are in both, as a share of
/// the shorter one.
fn overlap(a: &[u32], b: &[u32]) -> f64 {
    let (mut i, mut j, mut both) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                both += 1;
                i += 1;
                j += 1;
            }
        }
    }
    let shorter = a.len().min(b.len());
    if shorter == 0 {
        0.0
    } else {
        both as f64 / shorter as f64
    }
}

/// The columns of the footprint that are no wall: those every level is
/// found in.
pub(crate) fn level_mask(survey: &SceneSurvey, footprint: &Footprint, wall_height: f64) -> Mask {
    let walls = survey.wall_columns(wall_height);
    let frame = footprint.mask.frame();
    Mask::from_fn(frame, |x, y| {
        footprint.mask.get(x as i64, y as i64) && !walls.get(x as i64, y as i64)
    })
}

/// The peaks of the area per height within the footprint, from the lowest
/// up, each with the columns it holds and which way it faces.
fn find_peaks(
    survey: &SceneSurvey,
    mask: &Mask,
    footprint: usize,
    histogram: &[u32],
    config: &LevelConfig,
) -> Vec<LevelPeak> {
    let grid = &survey.grid;
    let cell_area = grid.cell_xy * grid.cell_xy;
    if footprint == 0 || histogram.is_empty() {
        return Vec::new();
    }
    let count = histogram.len();
    let at = |index: i64| -> u64 {
        if index < 0 || index >= count as i64 {
            0
        } else {
            histogram[index as usize] as u64
        }
    };
    // Smoothed with the binomial kernel of a standard deviation of one
    // cell, in whole numbers: sixteen times the mean.
    let smooth: Vec<u64> = (0..count as i64)
        .map(|i| at(i - 2) + 4 * at(i - 1) + 6 * at(i) + 4 * at(i + 1) + at(i + 2))
        .collect();
    let least = config
        .min_area
        .max(config.min_share * footprint as f64 * cell_area)
        / cell_area;
    // A surface in one cell or two keeps at least a quarter of its count
    // in the middle of the kernel; whether it is large enough is told by
    // its columns below.
    let mut candidates: Vec<usize> = (0..count)
        .filter(|&i| {
            smooth[i] as f64 >= 4.0 * least
                && (i == 0 || smooth[i] >= smooth[i - 1])
                && (i + 1 == count || smooth[i] > smooth[i + 1])
        })
        .collect();
    // Of peaks nearer than the separation, the higher count wins; at an
    // equal count the lower one.
    candidates.sort_by(|a, b| smooth[*b].cmp(&smooth[*a]).then(a.cmp(b)));
    let apart = (config.separation / grid.cell_z).round() as usize;
    let mut kept: Vec<usize> = Vec::new();
    for candidate in candidates {
        if kept.iter().all(|other| other.abs_diff(candidate) > apart) {
            kept.push(candidate);
        }
    }
    kept.sort_unstable();

    let free = (config.free_height / grid.cell_z).round() as u32;
    // What lies around a peak is the least count within this many cells.
    let around = ((0.3 / grid.cell_z).round() as usize).max(2);
    let mut peaks = Vec::new();
    for centre in kept {
        // The highest count near the smoothed peak, and the cells around it
        // that rise at least halfway from what lies around to that count.
        let low = centre.saturating_sub(2);
        let high = (centre + 2).min(count - 1);
        let top = (low..=high)
            .max_by(|a, b| histogram[*a].cmp(&histogram[*b]).then(b.cmp(a)))
            .expect("a range of cells");
        let base = (top.saturating_sub(around)..=(top + around).min(count - 1))
            .map(|bin| histogram[bin] as u64)
            .min()
            .unwrap_or(0);
        let peak = histogram[top] as u64;
        let half = |bin: usize| 2 * (histogram[bin] as u64).saturating_sub(base) >= peak - base;
        let (mut first, mut last) = (top, top);
        while first > 0 && half(first - 1) {
            first -= 1;
        }
        while last + 1 < count && half(last + 1) {
            last += 1;
        }
        let width = (last - first + 1) as f64 * grid.cell_z;
        if width > config.max_width + 1e-9 {
            continue;
        }
        // The foot of the peak: one cell more on either side where that
        // still rises clearly above what lies around, as a floor that slopes
        // a little does over several cells.
        let foot = |bin: usize| 10 * (histogram[bin] as u64).saturating_sub(base) > peak - base;
        let (first, last) = (
            if first > 0 && foot(first - 1) {
                first - 1
            } else {
                first
            },
            if last + 1 < count && foot(last + 1) {
                last + 1
            } else {
                last
            },
        );
        let (mut weighted, mut total) = (0u64, 0u64);
        for (bin, count) in histogram.iter().enumerate().take(last + 1).skip(first) {
            let above = (*count as u64).saturating_sub(base);
            weighted += above * (2 * bin as u64 + 1);
            total += above;
        }
        let z = grid.origin[2] + weighted as f64 / (2 * total.max(1)) as f64 * grid.cell_z;

        let mut columns = Vec::new();
        let [mut free_above, mut open_above, mut free_below, mut open_below] = [0usize; 4];
        for (column, inside) in mask.cells().iter().enumerate() {
            if !inside {
                continue;
            }
            let Some([bottom, top]) = survey.occupied_between(column, first as u32, last as u32)
            else {
                continue;
            };
            columns.push(column as u32);
            let (above, _) = survey.neighbours_in_column(column, top);
            match above {
                None => open_above += 1,
                Some(above) if above - top > free => free_above += 1,
                Some(_) => {}
            }
            let (_, below) = survey.neighbours_in_column(column, bottom);
            match below {
                None => open_below += 1,
                Some(below) if bottom - below > free => free_below += 1,
                Some(_) => {}
            }
        }
        // The area of a peak is what rises above what lies around it.
        if (columns.len() as f64) < least {
            continue;
        }
        let held = columns.len().max(1) as f64;
        let [free_above, open_above, free_below, open_below] =
            [free_above, open_above, free_below, open_below].map(|count| count as f64 / held);
        let facing = if free_above.max(free_below) >= 0.3 {
            if free_above >= free_below {
                Facing::Up
            } else {
                Facing::Down
            }
        } else if open_above.max(open_below) >= 0.3 {
            if open_above >= open_below {
                Facing::Up
            } else {
                Facing::Down
            }
        } else {
            Facing::Unclear
        };
        peaks.push(LevelPeak {
            z,
            area: columns.len() as f64 * cell_area,
            share: columns.len() as f64 / footprint as f64,
            width,
            free_above,
            open_above,
            free_below,
            open_below,
            facing,
            role: PeakRole::Unused,
            columns,
        });
    }
    peaks
}

/// The height of the ground in a band around the footprint: the median of
/// the lowest occupied cell of every column in it, and the share of its
/// columns that hold a cell at all.
fn ground_around(survey: &SceneSurvey, mask: &Mask, band: [f64; 2]) -> (Option<f64>, f64) {
    let grid = &survey.grid;
    let cells = |distance: f64| (distance / grid.cell_xy).round().max(0.0) as u32;
    let mut outer = mask.clone();
    outer.dilate(cells(band[1]), cells(band[1]));
    let mut inner = mask.clone();
    inner.dilate(cells(band[0]), cells(band[0]));
    let mut lowest = Vec::new();
    let mut columns = 0usize;
    for (column, (outer, inner)) in outer.cells().iter().zip(inner.cells()).enumerate() {
        if *outer && !*inner {
            columns += 1;
            if let Some(bin) = survey.lowest_bin(column) {
                lowest.push(bin);
            }
        }
    }
    if lowest.is_empty() {
        return (None, 0.0);
    }
    lowest.sort_unstable();
    let median = lowest[(lowest.len() - 1) / 2];
    (
        Some(grid.bin_center(median)),
        lowest.len() as f64 / columns as f64,
    )
}

/// The code of a level `number` storeys above P: "00", "01", "-01".
fn level_code(number: i64) -> String {
    if number < 0 {
        format!("-{:02}", -number)
    } else {
        format!("{number:02}")
    }
}

/// Find the levels of a building in its survey, within its footprint.
///
/// The result lists the floors from the lowest up, each with its ceiling,
/// the slab above it and its confidence, a partial floor after the storey
/// it stands in, and the roof last. Furniture is no level. Without a
/// footprint there are no levels. Every name is the code of the level; the
/// window gives them names in its language.
pub fn detect_levels(
    survey: &SceneSurvey,
    footprint: &Footprint,
    config: &LevelConfig,
) -> LevelDetection {
    let mask = level_mask(survey, footprint, config.wall_height);
    let histogram = survey.area_histogram(Some(&mask));
    let mut peaks = find_peaks(survey, &mask, footprint.mask.count(), &histogram, config);
    let (ground_z, ground_share) = ground_around(survey, &footprint.mask, config.ground_band);

    // Furniture: a surface seen from above just over a floor, smaller than
    // it. The floor is the nearest surface below that is no furniture.
    for index in 0..peaks.len() {
        if peaks[index].facing != Facing::Up {
            continue;
        }
        let floor = (0..index).rev().find(|below| {
            peaks[*below].facing == Facing::Up && peaks[*below].role != PeakRole::Furniture
        });
        if let Some(floor) = floor {
            let rise = peaks[index].z - peaks[floor].z;
            if rise >= config.furniture[0]
                && rise <= config.furniture[1]
                && (peaks[index].columns.len() as f64)
                    < config.furniture_share * peaks[floor].columns.len() as f64
            {
                peaks[index].role = PeakRole::Furniture;
            }
        }
    }
    // The roof: the highest surface seen from above, open to the sky.
    let ups: Vec<usize> = (0..peaks.len())
        .filter(|index| {
            peaks[*index].facing == Facing::Up && peaks[*index].role != PeakRole::Furniture
        })
        .collect();
    if let Some(&highest) = ups.last() {
        if peaks[highest].open_above > peaks[highest].free_above {
            peaks[highest].role = PeakRole::Roof;
        }
    }
    for &index in &ups {
        if peaks[index].role == PeakRole::Unused {
            peaks[index].role = if peaks[index].share < config.partial_share {
                PeakRole::PartialFloor
            } else {
                PeakRole::Floor
            };
        }
    }
    let floors: Vec<usize> = ups
        .iter()
        .copied()
        .filter(|index| peaks[*index].role == PeakRole::Floor)
        .collect();
    let roof = ups
        .iter()
        .copied()
        .find(|index| peaks[*index].role == PeakRole::Roof);

    // Ceilings belong to the floor below them, up to the next floor; of two
    // or more, the lowest is the ceiling and the highest the underside of
    // the slab.
    let mut ceilings: Vec<Vec<usize>> = vec![Vec::new(); floors.len()];
    for index in 0..peaks.len() {
        if peaks[index].facing != Facing::Down {
            continue;
        }
        let z = peaks[index].z;
        let owner = floors.iter().rposition(|floor| peaks[*floor].z < z);
        let Some(owner) = owner else {
            continue;
        };
        let rise = z - peaks[floors[owner]].z;
        let next = floors
            .get(owner + 1)
            .copied()
            .or(roof)
            .map(|next| peaks[next].z);
        if rise >= config.storey[0] && rise <= config.storey[1] && next.is_none_or(|next| z < next)
        {
            ceilings[owner].push(index);
        }
    }
    for list in &ceilings {
        match list.as_slice() {
            [] => {}
            [only] => peaks[*only].role = PeakRole::Ceiling,
            [lowest, .., highest] => {
                peaks[*lowest].role = PeakRole::LoweredCeiling;
                peaks[*highest].role = PeakRole::SlabUnderside;
            }
        }
    }

    // P: the lowest floor above the ground that covers enough of the
    // footprint; without a ground the lowest such floor.
    let covering: Vec<usize> = (0..floors.len())
        .filter(|at| peaks[floors[*at]].share >= config.peil_share)
        .collect();
    let peil = covering
        .iter()
        .copied()
        .find(|at| ground_z.is_none_or(|ground| peaks[floors[*at]].z >= ground - 0.1))
        .or(covering.first().copied())
        .or((!floors.is_empty()).then_some(0));

    let mut levels: Vec<Level> = Vec::new();
    for (at, &floor) in floors.iter().enumerate() {
        let peak = &peaks[floor];
        let number = at as i64 - peil.unwrap_or(0) as i64;
        let ceiling = match ceilings[at].as_slice() {
            [] => None,
            [only] => Some(*only),
            [lowest, ..] => Some(*lowest),
        };
        let underside = match ceilings[at].as_slice() {
            [_, .., highest] => Some(*highest),
            _ => None,
        };
        let next = floors.get(at + 1).copied().or(roof);
        let upper = underside.or(ceiling);
        let slab_thickness = match (upper, next) {
            (Some(upper), Some(next)) => {
                let thickness = peaks[next].z - peaks[upper].z;
                (thickness >= config.slab[0]
                    && thickness <= config.slab[1]
                    && overlap(&peaks[upper].columns, &peaks[next].columns) >= config.slab_overlap)
                    .then_some(thickness)
            }
            _ => None,
        };
        let id = level_code(number);
        levels.push(Level {
            name: id.clone(),
            id,
            kind: match number.cmp(&0) {
                std::cmp::Ordering::Less => LevelKind::Basement,
                std::cmp::Ordering::Equal => LevelKind::Ground,
                std::cmp::Ordering::Greater => LevelKind::Storey,
            },
            floor_z: peak.z,
            ceiling_z: ceiling.map(|ceiling| peaks[ceiling].z),
            slab_underside: underside.map(|underside| peaks[underside].z),
            slab_thickness,
            cut_height: config.cut_height,
            tilt_mm_per_m: None,
            share: peak.share,
            is_peil: Some(at) == peil,
            confidence: level_confidence(peak, next.map(|next| peaks[next].z - peak.z)),
            status: LevelStatus::Found,
        });
    }
    // Partial floors after the storey they stand in.
    for peak in peaks.iter() {
        if peak.role != PeakRole::PartialFloor {
            continue;
        }
        let storey = levels
            .iter()
            .rposition(|level| level.kind != LevelKind::Partial && level.floor_z < peak.z);
        let base = storey.map_or_else(|| "00".to_owned(), |at| levels[at].id.clone());
        let taken = levels
            .iter()
            .filter(|level| {
                level.kind == LevelKind::Partial && level.id.starts_with(&format!("{base}M"))
            })
            .count();
        let id = if taken == 0 {
            format!("{base}M")
        } else {
            format!("{base}M{}", taken + 1)
        };
        let at = storey.map_or(0, |at| {
            at + 1
                + levels[at + 1..]
                    .iter()
                    .take_while(|level| level.kind == LevelKind::Partial)
                    .count()
        });
        levels.insert(
            at,
            Level {
                name: id.clone(),
                id,
                kind: LevelKind::Partial,
                floor_z: peak.z,
                ceiling_z: None,
                slab_underside: None,
                slab_thickness: None,
                cut_height: config.cut_height,
                tilt_mm_per_m: None,
                share: peak.share,
                is_peil: false,
                confidence: level_confidence(peak, None),
                status: LevelStatus::Found,
            },
        );
    }
    if let Some(roof) = roof {
        let peak = &peaks[roof];
        let below = floors.last().map(|floor| peak.z - peaks[*floor].z);
        levels.push(Level {
            id: "R".to_owned(),
            name: "R".to_owned(),
            kind: LevelKind::Roof,
            floor_z: peak.z,
            ceiling_z: None,
            slab_underside: None,
            slab_thickness: None,
            cut_height: config.cut_height,
            tilt_mm_per_m: None,
            share: peak.share,
            is_peil: false,
            confidence: level_confidence(peak, below),
            status: LevelStatus::Found,
        });
    }
    let peil_z = peil.map(|at| peaks[floors[at]].z);
    LevelDetection {
        levels,
        ground_z,
        ground_share,
        peil_z,
        peaks,
        area_histogram: histogram,
    }
}

/// How `refine_levels` reads the floors. Lengths in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RefineConfig {
    /// The points this near the found height of a floor are read.
    pub window: f64,
    /// The bins they are counted in.
    pub bin: f64,
    /// See `LevelConfig::wall_height`; the columns next to a wall are left
    /// out as well.
    pub wall_height: f64,
    /// The plan is cut this high above the floor ...
    pub cut_height: f64,
    /// ... or up to this much higher or lower, where a surface larger than
    /// `cut_clear_area`, or `cut_clear_share` of the footprint, lies at that
    /// height.
    pub cut_shift: f64,
    pub cut_clear_area: f64,
    pub cut_clear_share: f64,
}

impl Default for RefineConfig {
    fn default() -> Self {
        Self {
            window: 0.05,
            bin: 0.002,
            wall_height: 1.0,
            cut_height: 1.2,
            cut_shift: 0.15,
            cut_clear_area: 0.5,
            cut_clear_share: 0.005,
        }
    }
}

/// What the points of one floor tell.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LevelRefinement {
    /// The height of the floor as found in the survey, and as read now.
    pub found_z: f64,
    pub floor_z: f64,
    /// How much it rises along u and along v, in millimetres per metre.
    pub tilt_mm_per_m: [f64; 2],
    /// Half the height between the 16th and the 84th percentile of its
    /// points: the noise and the slope of the floor together.
    pub spread: f64,
    pub points: u64,
    /// The height of the cut of the plan above the floor.
    pub cut_height: f64,
}

/// The counts of one read of a floor, shared by the reading threads.
struct FloorCounts {
    /// All points, then those on the low and the high half along u, and
    /// along v; one count per bin each.
    bins: [Vec<AtomicU32>; 5],
    /// Sums of u on the two halves along u, and of v on those along v, in
    /// tenths of a millimetre.
    sums: [AtomicI64; 4],
}

impl FloorCounts {
    fn new(bins: usize) -> Self {
        Self {
            bins: std::array::from_fn(|_| (0..bins).map(|_| AtomicU32::new(0)).collect()),
            sums: std::array::from_fn(|_| AtomicI64::new(0)),
        }
    }

    fn take(self) -> ([Vec<u32>; 5], [i64; 4]) {
        let [a, b, c, d, e] = self.bins;
        let take = |bins: Vec<AtomicU32>| -> Vec<u32> {
            bins.into_iter().map(AtomicU32::into_inner).collect()
        };
        (
            [take(a), take(b), take(c), take(d), take(e)],
            self.sums.map(AtomicI64::into_inner),
        )
    }
}

/// The height at which a share of the counts of the bins lies, the bins
/// starting at `low`, with the bin it falls in taken as evenly filled.
fn quantile(bins: &[u32], low: f64, width: f64, share: f64) -> Option<f64> {
    let total: u64 = bins.iter().map(|count| *count as u64).sum();
    if total == 0 {
        return None;
    }
    let target = share * total as f64;
    let mut before = 0u64;
    for (bin, count) in bins.iter().enumerate() {
        let after = before + *count as u64;
        if *count > 0 && after as f64 >= target {
            let inside = (target - before as f64) / *count as f64;
            return Some(low + (bin as f64 + inside.clamp(0.0, 1.0)) * width);
        }
        before = after;
    }
    Some(low + bins.len() as f64 * width)
}

/// The height of the cut of a storey above its floor: `cut_height`, or the
/// nearest height within `cut_shift` of it where no larger surface lies.
fn cut_height_for(
    survey: &SceneSurvey,
    histogram: &[u32],
    floor_z: f64,
    clear: u32,
    config: &RefineConfig,
) -> f64 {
    let grid = &survey.grid;
    // The most columns occupied within 2 cm of a height.
    let area = |z: f64| -> u32 {
        let low = grid.bin_of(z - 0.02);
        let high = grid.bin_of(z + 0.02);
        match (low, high) {
            (Some(low), Some(high)) => (low..=high)
                .map(|bin| histogram[bin as usize])
                .max()
                .unwrap_or(0),
            _ => 0,
        }
    };
    let steps = (config.cut_shift / 0.01).round() as i64;
    let mut best: Option<(u32, i64)> = None;
    // From the nominal height outwards, below before above.
    for offset in (0..=steps).flat_map(|step| [-step, step]) {
        let height = config.cut_height + offset as f64 * 0.01;
        let found = area(floor_z + height);
        if found <= clear {
            return height;
        }
        if best.is_none_or(|(least, _)| found < least) {
            best = Some((found, offset));
        }
    }
    best.map_or(config.cut_height, |(_, offset)| {
        config.cut_height + offset as f64 * 0.01
    })
}

/// The floor that the slab over the level at `place` carries: the next
/// whole floor or the roof above it, a partial floor in between left out.
/// The levels are sorted from the lowest up with the roof last.
pub fn floor_above(levels: &[Level], place: usize) -> Option<f64> {
    levels
        .get(place + 1..)?
        .iter()
        .find(|level| level.is_storey() || level.kind == LevelKind::Roof)
        .map(|level| level.floor_z)
}

/// Set the thickness of the slab over every whole floor again, from its
/// ceiling, or the underside of the slab above a suspended ceiling, to the
/// floor above, as after levels were moved, added or removed by hand. A
/// thickness outside `config.slab`, as between a ceiling and a floor that is
/// no longer the one above it, is none.
pub fn slab_thicknesses(levels: &mut [Level], config: &LevelConfig) {
    let above: Vec<Option<f64>> = (0..levels.len())
        .map(|place| floor_above(levels, place))
        .collect();
    for (level, next) in levels.iter_mut().zip(above) {
        let upper = level.slab_underside.or(level.ceiling_z);
        level.slab_thickness = match (level.is_storey(), upper, next) {
            (true, Some(upper), Some(next)) => {
                let thickness = next - upper;
                (thickness >= config.slab[0] && thickness <= config.slab[1]).then_some(thickness)
            }
            _ => None,
        };
    }
}

/// Read the points near every floor of `levels` once more and set its
/// height, its slope and the height of its cut from them, in a box per
/// floor. The roof gets its height and slope; the rest keeps what it had.
/// A level the user changed is left alone. P follows its floor.
///
/// Only the columns of the footprint count, without the walls and the
/// columns beside them. The result is the same for any number of threads.
#[allow(clippy::too_many_arguments)]
pub fn refine_levels(
    sources: &[RegionSource<'_>],
    survey: &SceneSurvey,
    footprint: &Footprint,
    detection: &mut LevelDetection,
    accept: &RegionFilter<'_>,
    config: &RefineConfig,
    progress: &mut (dyn FnMut(usize, RegionProgress) -> Result<(), LoadError> + Send),
) -> Result<Vec<Option<LevelRefinement>>, LoadError> {
    let grid = survey.grid;
    let frame = survey.frame;
    let mut mask = level_mask(survey, footprint, config.wall_height);
    // The columns beside a wall hold the foot of the wall as well.
    let walls = {
        let mut walls = survey.wall_columns(config.wall_height);
        walls.dilate(1, 1);
        walls
    };
    for y in 0..grid.size[1] {
        for x in 0..grid.size[0] {
            if walls.get(x as i64, y as i64) {
                mask.set(x, y, false);
            }
        }
    }
    let histogram = survey.area_histogram(Some(&mask));
    let footprint_columns = footprint.mask.count();
    let cell_area = grid.cell_xy * grid.cell_xy;
    let clear = (config
        .cut_clear_area
        .max(config.cut_clear_share * footprint_columns as f64 * cell_area)
        / cell_area)
        .floor() as u32;
    // The middle of the columns that count and their extent, in the plan.
    let (mut count, mut sum_x, mut sum_y) = (0u64, 0u64, 0u64);
    let mut extent: Option<[u32; 4]> = None;
    for y in 0..grid.size[1] {
        for x in 0..grid.size[0] {
            if mask.get(x as i64, y as i64) {
                count += 1;
                sum_x += x as u64;
                sum_y += y as u64;
                extent = Some(match extent {
                    None => [x, y, x, y],
                    Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
                });
            }
        }
    }
    let Some([x0, y0, x1, y1]) = extent else {
        return Ok(vec![None; detection.levels.len()]);
    };
    let middle = [
        grid.origin[0] + (sum_x as f64 / count as f64 + 0.5) * grid.cell_xy,
        grid.origin[1] + (sum_y as f64 / count as f64 + 0.5) * grid.cell_xy,
    ];
    let low_corner = grid.column_center(x0, y0);
    let high_corner = grid.column_center(x1, y1);
    let half_cell = grid.cell_xy * 0.5;

    let bins = ((2.0 * config.window) / config.bin).round().max(1.0) as usize;
    let mut results = Vec::with_capacity(detection.levels.len());
    for (place, level) in detection.levels.iter().enumerate() {
        if level.status == LevelStatus::Edited {
            results.push(None);
            continue;
        }
        let low = level.floor_z - config.window;
        let region = frame.oriented_box(Bounds {
            min: [
                low_corner[0] - half_cell,
                low_corner[1] - half_cell,
                low - frame.peil_z,
            ],
            max: [
                high_corner[0] + half_cell,
                high_corner[1] + half_cell,
                low + bins as f64 * config.bin - frame.peil_z,
            ],
        });
        let counts = FloorCounts::new(bins);
        let inside = |source: usize, ordinal: u64, point: &Point| {
            region.contains(point.xyz) && accept(source, ordinal, point)
        };
        let mut report = |read: RegionProgress| progress(place, read);
        visit_region_parallel(
            sources,
            region.aabb(),
            &inside,
            &mut report,
            &|| (),
            &|_: &mut (), _: usize, batch: &[IndexedPoint]| {
                for record in batch {
                    let xyz = record.point.xyz;
                    let uv = frame.to_plan([xyz[0], xyz[1]]);
                    let Some([x, y]) = grid.column_of(uv) else {
                        continue;
                    };
                    if !mask.get(x as i64, y as i64) {
                        continue;
                    }
                    let bin = ((xyz[2] - low) / config.bin).floor();
                    if !(bin >= 0.0 && bin < bins as f64) {
                        continue;
                    }
                    let bin = bin as usize;
                    counts.bins[0][bin].fetch_add(1, Ordering::Relaxed);
                    let along_u = usize::from(uv[0] >= middle[0]);
                    let along_v = usize::from(uv[1] >= middle[1]);
                    counts.bins[1 + along_u][bin].fetch_add(1, Ordering::Relaxed);
                    counts.bins[3 + along_v][bin].fetch_add(1, Ordering::Relaxed);
                    let tenths = |value: f64| (value * 10_000.0).round() as i64;
                    counts.sums[along_u].fetch_add(tenths(uv[0] - middle[0]), Ordering::Relaxed);
                    counts.sums[2 + along_v]
                        .fetch_add(tenths(uv[1] - middle[1]), Ordering::Relaxed);
                }
                Ok(())
            },
        )?;
        let (counted, sums) = counts.take();
        let points: u64 = counted[0].iter().map(|count| *count as u64).sum();
        let Some(floor_z) = quantile(&counted[0], low, config.bin, 0.5) else {
            results.push(None);
            continue;
        };
        let spread = match (
            quantile(&counted[0], low, config.bin, 0.16),
            quantile(&counted[0], low, config.bin, 0.84),
        ) {
            (Some(lower), Some(upper)) => (upper - lower) * 0.5,
            _ => 0.0,
        };
        // The slope from the medians of the two halves along each axis, over
        // the distance between the middles of their points.
        let slope = |lower: usize, sum: usize| -> f64 {
            let total = |bins: &Vec<u32>| bins.iter().map(|count| *count as u64).sum::<u64>();
            let (low_count, high_count) = (total(&counted[lower]), total(&counted[lower + 1]));
            if low_count == 0 || high_count == 0 {
                return 0.0;
            }
            let (Some(low_z), Some(high_z)) = (
                quantile(&counted[lower], low, config.bin, 0.5),
                quantile(&counted[lower + 1], low, config.bin, 0.5),
            ) else {
                return 0.0;
            };
            let low_at = sums[sum] as f64 / 10_000.0 / low_count as f64;
            let high_at = sums[sum + 1] as f64 / 10_000.0 / high_count as f64;
            if high_at - low_at < 0.5 {
                return 0.0;
            }
            (high_z - low_z) / (high_at - low_at) * 1000.0
        };
        let tilt = [slope(1, 0), slope(3, 2)];
        let cut_height = if level.kind == LevelKind::Roof {
            level.cut_height
        } else {
            cut_height_for(survey, &histogram, floor_z, clear, config)
        };
        results.push(Some(LevelRefinement {
            found_z: level.floor_z,
            floor_z,
            tilt_mm_per_m: tilt,
            spread,
            points,
            cut_height,
        }));
    }
    for (level, refined) in detection.levels.iter_mut().zip(&results) {
        if let Some(refined) = refined {
            level.floor_z = refined.floor_z;
            level.tilt_mm_per_m = Some(refined.tilt_mm_per_m);
            level.cut_height = refined.cut_height;
        }
    }
    // The slabs between a ceiling and the refined floor above it.
    let above: Vec<Option<f64>> = (0..detection.levels.len())
        .map(|place| floor_above(&detection.levels, place))
        .collect();
    for (level, next) in detection.levels.iter_mut().zip(above) {
        if level.slab_thickness.is_none() || level.kind == LevelKind::Partial {
            continue;
        }
        if let (Some(upper), Some(next)) = (level.slab_underside.or(level.ceiling_z), next) {
            level.slab_thickness = Some(next - upper);
        }
    }
    if let Some(peil) = detection.levels.iter().find(|level| level.is_peil) {
        detection.peil_z = Some(peil.floor_z);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plans::survey::tests::{on_threads, resident, site_box, true_frame};
    use crate::plans::survey::{survey_scene, FootprintConfig, SurveyConfig};
    use crate::region_source::SourceTransform;
    use crate::test_shapes::{building, BuildingSpec};

    fn everything(_: usize, _: u64, _: &Point) -> bool {
        true
    }

    /// The survey, the footprint and the levels of a generated building.
    fn levels_of(spec: &BuildingSpec, threads: usize) -> (SceneSurvey, Footprint, LevelDetection) {
        let scan = building(spec);
        let points = resident(&scan.points);
        let source = RegionSource::resident(&points, SourceTransform::default());
        let frame = true_frame(spec);
        on_threads(threads, || {
            let survey = survey_scene(
                &[source],
                site_box(spec),
                &frame,
                &everything,
                &SurveyConfig::default(),
                &mut |_| Ok(()),
            )
            .unwrap();
            let footprint = survey.footprint_proposal(&FootprintConfig::default());
            let mut detection = detect_levels(&survey, &footprint, &LevelConfig::default());
            refine_levels(
                &[source],
                &survey,
                &footprint,
                &mut detection,
                &everything,
                &RefineConfig::default(),
                &mut |_, _| Ok(()),
            )
            .unwrap();
            (survey, footprint, detection)
        })
    }

    #[test]
    fn three_floors_a_roof_and_the_ground_and_no_level_at_a_desk_or_a_cabinet() {
        let spec = BuildingSpec {
            spacing: 0.04,
            ..BuildingSpec::default()
        };
        let (_, _, found) = levels_of(&spec, 1);
        let z0 = spec.translation[2];
        let kinds: Vec<LevelKind> = found.levels.iter().map(|level| level.kind).collect();
        assert_eq!(
            kinds,
            [
                LevelKind::Ground,
                LevelKind::Storey,
                LevelKind::Storey,
                LevelKind::Roof
            ],
            "{:#?}",
            found.peaks
        );
        let ids: Vec<&str> = found.levels.iter().map(|level| level.id.as_str()).collect();
        assert_eq!(ids, ["00", "01", "02", "R"]);
        for (storey, level) in found.floors().enumerate() {
            let truth = spec.floor_z(storey) + z0;
            assert!(
                (level.floor_z - truth).abs() <= 0.002,
                "{storey}: {} {truth}",
                level.floor_z
            );
            assert!(level.share > 0.7 && level.share < 0.95, "{}", level.share);
            assert_eq!(level.is_peil, storey == 0);
            assert!(level.confidence.score > 0.75, "{:?}", level.confidence);
            let tilt = level.tilt_mm_per_m.unwrap();
            assert!(tilt[0].abs() < 0.5 && tilt[1].abs() < 0.5, "{tilt:?}");
            // The slab above, from its underside to the next floor.
            let slab = level.slab_thickness.unwrap();
            assert!((slab - spec.slab).abs() < 0.015, "{storey}: {slab}");
        }
        let roof = found.roof().unwrap();
        assert!((roof.floor_z - (spec.roof_z() + z0)).abs() <= 0.002);
        assert_eq!(found.peil_z, Some(found.levels[0].floor_z));
        let ground = found.ground_z.unwrap();
        assert!((ground - (spec.ground_z() + z0)).abs() < 0.02, "{ground}");
        assert!(found.ground_share > 0.9);

        // The desks at 0.75 m are a peak, and furniture; the counter at
        // 1.2 m and the cabinet that reaches the ceiling are no peak at all.
        let desks: Vec<&LevelPeak> = found
            .peaks
            .iter()
            .filter(|peak| peak.role == PeakRole::Furniture)
            .collect();
        assert_eq!(desks.len(), 2, "{:#?}", found.peaks);
        for (storey, desk) in desks.iter().enumerate() {
            assert!((desk.z - (spec.floor_z(storey) + 0.75 + z0)).abs() < 0.02);
        }
        for level in &found.levels {
            for height in [0.75, 1.2] {
                assert!(
                    found
                        .levels
                        .iter()
                        .all(|other| (other.floor_z - (level.floor_z + height)).abs() > 0.1),
                    "a level at {height} m above {}",
                    level.id
                );
            }
        }

        // The suspended ceiling of the ground floor under its slab, and the
        // plain ceilings above.
        let ground_floor = &found.levels[0];
        assert!((ground_floor.ceiling_z.unwrap() - (2.45 + z0)).abs() < 0.02);
        assert!((ground_floor.slab_underside.unwrap() - (spec.ceiling_z(0) + z0)).abs() < 0.02);
        assert!(found
            .peaks
            .iter()
            .any(|peak| peak.role == PeakRole::LoweredCeiling));
        for storey in 1..3 {
            let level = &found.levels[storey];
            assert!((level.ceiling_z.unwrap() - (spec.ceiling_z(storey) + z0)).abs() < 0.02);
            assert_eq!(level.slab_underside, None);
        }

        // The plan of the first floor is cut beside the top of the counter
        // at 1.2 m; that of the others at 1.2 m.
        assert_eq!(found.levels[0].cut_height, 1.2);
        assert_eq!(found.levels[2].cut_height, 1.2);
        let moved = found.levels[1].cut_height;
        assert!(
            (moved - 1.2).abs() > 0.02 && (moved - 1.2).abs() <= 0.15 + 1e-9,
            "{moved}"
        );
    }

    #[test]
    fn levels_are_the_same_for_one_and_many_threads() {
        let spec = BuildingSpec {
            storeys: 2,
            spacing: 0.045,
            ..BuildingSpec::default()
        };
        let (_, _, one) = levels_of(&spec, 1);
        for threads in [3, 8] {
            let (_, _, many) = levels_of(&spec, threads);
            assert_eq!(many, one, "{threads}");
        }
        assert_eq!(one.floors().count(), 2);
        assert!(one.roof().is_some());
    }

    #[test]
    fn a_floor_that_slopes_is_found_with_its_slope() {
        let spec = BuildingSpec {
            spacing: 0.04,
            sloped_floor: Some((1, 3.0)),
            ..BuildingSpec::default()
        };
        let (_, _, found) = levels_of(&spec, 4);
        let z0 = spec.translation[2];
        let floors: Vec<&Level> = found.floors().collect();
        assert_eq!(floors.len(), 3, "{:#?}", found.peaks);
        // About the middle of the building the floor is where it would be.
        let sloped = floors[1];
        assert!(
            (sloped.floor_z - (spec.floor_z(1) + z0)).abs() <= 0.002,
            "{}",
            sloped.floor_z
        );
        let [along_u, along_v] = sloped.tilt_mm_per_m.unwrap();
        assert!((along_u - 3.0).abs() < 0.3, "{along_u}");
        assert!(along_v.abs() < 0.3, "{along_v}");
        for flat in [floors[0], floors[2]] {
            let tilt = flat.tilt_mm_per_m.unwrap();
            assert!(tilt[0].abs() < 0.3 && tilt[1].abs() < 0.3, "{tilt:?}");
        }
    }

    #[test]
    fn a_slab_reaches_from_the_ceiling_to_the_next_whole_floor() {
        let level = |id: &str, kind: LevelKind, floor_z: f64, ceiling: Option<f64>| Level {
            id: id.into(),
            name: id.into(),
            kind,
            floor_z,
            ceiling_z: ceiling,
            slab_underside: None,
            slab_thickness: ceiling.map(|_| 0.25),
            cut_height: 1.2,
            tilt_mm_per_m: None,
            share: 1.0,
            is_peil: false,
            confidence: Confidence::certain(),
            status: LevelStatus::Found,
        };
        let mut levels = vec![
            level("00", LevelKind::Ground, 0.0, Some(2.95)),
            level("00M", LevelKind::Partial, 1.5, None),
            level("01", LevelKind::Storey, 3.4, Some(6.15)),
            level("R", LevelKind::Roof, 6.4, None),
        ];
        // The mezzanine carries no slab of the storey it stands in.
        assert_eq!(floor_above(&levels, 0), Some(3.4));
        assert_eq!(floor_above(&levels, 2), Some(6.4));
        assert_eq!(floor_above(&levels, 3), None);
        levels[2].slab_underside = Some(6.35);
        slab_thicknesses(&mut levels, &LevelConfig::default());
        let thickness: Vec<Option<f64>> = levels.iter().map(|level| level.slab_thickness).collect();
        assert!((thickness[0].unwrap() - 0.45).abs() < 1e-9, "{thickness:?}");
        // 5 cm from the underside of the slab to the roof is no slab.
        assert_eq!(thickness[1..], [None, None, None]);
        levels[2].slab_underside = None;
        slab_thicknesses(&mut levels, &LevelConfig::default());
        assert!((levels[2].slab_thickness.unwrap() - 0.25).abs() < 1e-9);
        // Without the first floor, the ceiling of the ground floor lies far
        // below the floor above it.
        levels.remove(2);
        slab_thicknesses(&mut levels, &LevelConfig::default());
        assert_eq!(levels[0].slab_thickness, None);
    }

    #[test]
    fn a_quantile_lies_inside_its_bin() {
        assert_eq!(quantile(&[], 0.0, 0.002, 0.5), None);
        assert_eq!(quantile(&[0, 0], 0.0, 0.002, 0.5), None);
        // Four in the second bin: the median is in its middle.
        let median = quantile(&[0, 4, 0], 1.0, 0.002, 0.5).unwrap();
        assert!((median - 1.003).abs() < 1e-12);
        let median = quantile(&[2, 2], 0.0, 0.01, 0.5).unwrap();
        assert!((median - 0.01).abs() < 1e-12);
    }
}
