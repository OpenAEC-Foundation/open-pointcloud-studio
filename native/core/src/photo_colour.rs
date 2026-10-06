//! Colour the points of a cloud from the photos that see them.
//!
//! Every point takes the colour of a photo that sees it: the one that sees
//! it best, or a blend of all that see it, weighted strongly to the best.
//! A photo sees a point best from near by and, for a pinhole photo, near
//! the middle of the picture rather than at its edges.
//!
//! A photo only sees a point when nothing of the cloud lies in front of it.
//! For every photo a depth image is made from the cloud itself, at about a
//! fifth of a degree per pixel: from the nodes of the octree index, read at
//! a detail that follows their distance to the photo, or from the points in
//! memory for a cloud without an index. A point that lies further from the
//! photo than its pixel of the depth image says, by more than a tolerance
//! that grows with the distance, is hidden from that photo.
//!
//! A large region is coloured in parts of at most `PART_POINTS` points, so
//! that memory follows a part and not the whole region. The colours of a
//! part are handed over as soon as it is done.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;

use crate::region_source::{
    overlaps, visit_region_parallel, RegionFilter, RegionProgress, RegionReader, RegionSource,
    SourceTransform,
};
use crate::{
    Bounds, FilePhoto, IndexedNode, IndexedPoint, LoadError, OctreeIndex, PhotoProjection, Point,
};

/// How far from a photo a point may lie and still take its colour, in
/// metres, unless asked otherwise.
pub const DEFAULT_MAX_DISTANCE: f64 = 20.0;
/// The range a caller may ask for, in metres.
pub const MIN_MAX_DISTANCE: f64 = 0.5;
pub const MAX_MAX_DISTANCE: f64 = 500.0;
/// Most points coloured at a time; a larger region is coloured in parts.
pub const PART_POINTS: u64 = 8_000_000;

/// The angle across one pixel of a depth image, in radians: a fifth of a
/// degree. A depth image is the photo scaled down by a whole factor to
/// about this.
const DEPTH_ANGLE: f64 = 0.0035;
/// A node of the index is read whole, without the nodes below it, once it
/// covers no more than this many pixels of a depth image across.
const NODE_SPAN: f64 = 24.0;
/// Points read of a node per pixel of the depth image it covers, counting
/// the square around it; at least `MIN_NODE_SAMPLE`.
const SAMPLES_PER_PIXEL: f64 = 1.0;
const MIN_NODE_SAMPLE: usize = 64;
/// A depth point covers a square of pixels about as wide as the room
/// between it and its neighbours, times this; at least the pixels around
/// its own.
const SPLAT_SCALE: f64 = 0.6;
/// The widest square a depth point covers, in pixels either side.
const MAX_SPLAT: i64 = 12;
/// A point is hidden when it lies further than the depth image says by
/// more than this, in metres, or by more than `SLOPE` pixels seen edge-on
/// when that is more.
const MIN_TOLERANCE: f64 = 0.05;
/// How steeply a surface may be seen and still see itself, as the distance
/// it recedes over one pixel of the depth image, in that pixel's width at
/// its distance: 3.5 is about 74 degrees from straight on. A depth point
/// covers the pixels around its own as a cone that recedes this fast, so
/// that its neighbours on such a surface are not hidden by it.
const SLOPE: f64 = 3.5;
/// Points nearer a photo than this, in metres, are left out for it.
const MIN_RANGE: f64 = 0.05;
/// Edge of the cells points are grouped in, in metres, to leave out whole
/// groups that a photo cannot reach.
const CELL: f64 = 1.0;
/// Most points in a group: the groups a photo reaches are coloured on
/// several threads.
const GROUP_POINTS: usize = 16_384;
/// Photos read and decoded at the same time.
const PHOTO_BATCH: usize = 4;
/// How much worse a point in the corner of a pinhole photo is seen than one
/// at its middle, at the same distance.
const EDGE_PENALTY: f64 = 0.25;

/// How a colouring is done.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotoColourConfig {
    /// A photo gives no colour to points further from it than this, in
    /// metres.
    pub max_distance: f64,
    /// Blend the colours of every photo that sees a point, weighted to the
    /// one that sees it best; otherwise that photo alone. A blend evens out
    /// the exposure of photos taken one after another and a small error in
    /// their poses.
    pub blend: bool,
    /// Most points coloured at a time.
    pub part_points: u64,
}

impl Default for PhotoColourConfig {
    fn default() -> Self {
        Self {
            max_distance: DEFAULT_MAX_DISTANCE,
            blend: true,
            part_points: PART_POINTS,
        }
    }
}

/// A decoded photo: four bytes per pixel (red, green, blue and alpha), row
/// by row from the top. It may be smaller than the photo is stored; its
/// pixels are found by scaling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhotoPixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl PhotoPixels {
    fn valid(&self) -> bool {
        self.width > 0
            && self.height > 0
            && self.rgba.len() == self.width as usize * self.height as usize * 4
    }

    fn texel(&self, column: i64, row: i64) -> [f32; 3] {
        let at = (row as usize * self.width as usize + column as usize) * 4;
        [
            f32::from(self.rgba[at]),
            f32::from(self.rgba[at + 1]),
            f32::from(self.rgba[at + 2]),
        ]
    }

    /// The colour at a pixel of the photo at its stored size, with integer
    /// values at pixel centres: bilinear between the four nearest decoded
    /// pixels, across the seam of a panorama that goes all the way round.
    fn sample(&self, pixel: [f64; 2], stored: [u32; 2], wraps: bool) -> [f32; 3] {
        let width = i64::from(self.width);
        let height = i64::from(self.height);
        let x = (pixel[0] + 0.5) * f64::from(self.width) / f64::from(stored[0]) - 0.5;
        let y = (pixel[1] + 0.5) * f64::from(self.height) / f64::from(stored[1]) - 0.5;
        let (left, top) = (x.floor(), y.floor());
        let (across, down) = ((x - left) as f32, (y - top) as f32);
        let column = |at: i64| {
            if wraps {
                at.rem_euclid(width)
            } else {
                at.clamp(0, width - 1)
            }
        };
        let row = |at: i64| at.clamp(0, height - 1);
        let (left, top) = (left as i64, top as i64);
        let corners = [
            self.texel(column(left), row(top)),
            self.texel(column(left + 1), row(top)),
            self.texel(column(left), row(top + 1)),
            self.texel(column(left + 1), row(top + 1)),
        ];
        std::array::from_fn(|channel| {
            let upper = corners[0][channel] + (corners[1][channel] - corners[0][channel]) * across;
            let lower = corners[2][channel] + (corners[3][channel] - corners[2][channel]) * across;
            upper + (lower - upper) * down
        })
    }
}

/// What a colouring is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourStage {
    /// Reading the points of a part of the region.
    Reading,
    /// Reading the photos that reach the part, making their depth images
    /// and colouring the points they see.
    Photos,
}

/// How far a colouring is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColourProgress {
    pub stage: ColourStage,
    /// The part being coloured, from 1, of how many.
    pub part: usize,
    pub parts: usize,
    /// Points read of the part, or photos done of those that reach it.
    pub done: u64,
    pub total: u64,
}

/// The photo colours against the colours the points had, per channel, for
/// the points that had a colour and were seen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColourComparison {
    pub points: u64,
    /// The mean of photo minus stored colour.
    pub mean_difference: [f64; 3],
    pub mean_abs_difference: [f64; 3],
    pub median_abs_difference: [u8; 3],
}

/// How long the stages of a colouring took, in seconds. Reading the photos
/// and making their depth images run side by side on several threads; their
/// times add up the time each photo took.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ColourTimes {
    pub reading: f64,
    pub photos: f64,
    pub decoding: f64,
    pub depth: f64,
    pub colouring: f64,
}

/// What a colouring did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoColouring {
    /// Points in the region that were taken.
    pub points: u64,
    /// Points that a photo sees and that were coloured.
    pub seen: u64,
    /// Photos close enough to a part of the region to be read.
    pub photos: usize,
    /// Photos that gave the colour of at least one point; with a blend, the
    /// photo that saw it best.
    pub photos_used: usize,
    /// Photos that could not be read or decoded, and were left out.
    pub photos_failed: usize,
    /// The parts the region was coloured in that held points.
    pub parts: usize,
    /// `None` when no point that was seen had a colour before.
    pub compared: Option<ColourComparison>,
    pub times: ColourTimes,
}

impl PhotoColouring {
    /// The share of the points that no photo sees, from 0 to 1.
    pub fn unseen_share(&self) -> f64 {
        if self.points == 0 {
            0.0
        } else {
            (self.points - self.seen) as f64 / self.points as f64
        }
    }
}

/// Receives the colours of a part: the source ordinal of every point that
/// was seen, with its colour.
pub type ColourSink<'a> = dyn FnMut(&[(u64, [u8; 3])]) -> Result<(), LoadError> + 'a;

/// Give the points of a region the colour the photos see them with.
///
/// - `source` is one layer, read with the transform of the source left out:
///   its points, the nodes of its index and the photos are all in source
///   coordinates. A source with another transform is refused.
/// - `region` and `accept` choose the points, as in `visit_region`; only
///   those are coloured. Every point of the cloud, chosen or not, can hide
///   a point from a photo.
/// - `photos` are the photos with their poses, and `decode` reads and
///   decodes the photo at a place in that list. A photo that cannot be
///   decoded is left out and counted.
/// - `deliver` gets the colours of each part as soon as it is done: the
///   source ordinal of every point that was seen with its colour. Points no
///   photo sees are not delivered.
/// - `progress` is called on the calling thread, and may stop the job with
///   an error; `cancelled` is asked on every thread, and stops the job with
///   `LoadError::Cancelled`.
#[allow(clippy::too_many_arguments)]
pub fn colour_from_photos(
    source: RegionSource<'_>,
    region: Bounds,
    accept: &RegionFilter<'_>,
    photos: &[FilePhoto],
    decode: &(dyn Fn(usize) -> Result<PhotoPixels, LoadError> + Sync),
    config: &PhotoColourConfig,
    progress: &mut (dyn FnMut(ColourProgress) -> Result<(), LoadError> + Send),
    deliver: &mut ColourSink<'_>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<PhotoColouring, LoadError> {
    if source.transform != SourceTransform::default() {
        return Err(LoadError::InvalidData(
            "photos colour points in source coordinates; the transform of the layer is applied afterwards"
                .into(),
        ));
    }
    if !(MIN_MAX_DISTANCE..=MAX_MAX_DISTANCE).contains(&config.max_distance) {
        return Err(LoadError::InvalidData(format!(
            "the largest distance to a photo must be from {MIN_MAX_DISTANCE} to {MAX_MAX_DISTANCE} metres"
        )));
    }
    let mut result = PhotoColouring::default();
    let Some(data) = source.world_bounds() else {
        return Ok(result);
    };
    let Some(region) = intersect(region, data) else {
        return Ok(result);
    };
    let cells = plan_parts(&source, region, config.part_points.max(1));
    let mut tally = Tally::new(photos.len());
    // One origin for every part, so that a point is seen the same whichever
    // part it falls in.
    let origin = region.min.map(f64::floor);
    for (place, cell) in cells.iter().enumerate() {
        let part = Part {
            place: place + 1,
            parts: cells.len(),
            cell: *cell,
            origin,
        };
        colour_part(
            &source, region, accept, &part, photos, decode, config, progress, deliver, cancelled,
            &mut tally,
        )?;
    }
    tally.finish(&mut result);
    Ok(result)
}

/// The parts a region is coloured in: boxes whose faces run through the
/// region, the outer ones open to infinity. A point belongs to the one box
/// it lies in, counting a lower face and not an upper one, so that a point
/// on a face between two parts is coloured once.
fn plan_parts(source: &RegionSource<'_>, region: Bounds, part_points: u64) -> Vec<Bounds> {
    let everywhere = Bounds {
        min: [f64::NEG_INFINITY; 3],
        max: [f64::INFINITY; 3],
    };
    let RegionReader::Index(index) = source.reader else {
        return vec![everywhere];
    };
    let mut cells = Vec::new();
    split_part(index, region, everywhere, part_points, 0, &mut cells);
    cells
}

fn split_part(
    index: &OctreeIndex,
    region: Bounds,
    cell: Bounds,
    part_points: u64,
    depth: u32,
    cells: &mut Vec<Bounds>,
) {
    let Some(area) = intersect(region, cell) else {
        return;
    };
    // A leaf that lies partly in the part counts with the share of its
    // box that does, as if its points filled it evenly.
    let leaves: Vec<(Bounds, f64)> = index
        .intersecting_leaves(|bounds| overlaps(bounds, area))
        .iter()
        .map(|leaf| {
            let inside = intersect(leaf.bounds, area).unwrap_or(leaf.bounds);
            (inside, leaf.stored_points as f64 * share(leaf.bounds, area))
        })
        .collect();
    let points: f64 = leaves.iter().map(|(_, points)| points).sum();
    if points <= 0.0 {
        return;
    }
    let axis = (0..3)
        .max_by(|a, b| (area.max[*a] - area.min[*a]).total_cmp(&(area.max[*b] - area.min[*b])))
        .unwrap_or(0);
    let length = area.max[axis] - area.min[axis];
    if points <= part_points as f64 || depth >= 24 || length < CELL {
        cells.push(cell);
        return;
    }
    // Where half the points lie on either side, counting each leaf at the
    // middle of what it has in the part; kept off the faces of the part.
    let mut middles: Vec<(f64, f64)> = leaves
        .iter()
        .map(|(inside, points)| ((inside.min[axis] + inside.max[axis]) * 0.5, *points))
        .collect();
    middles.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut below, half) = (0.0, points * 0.5);
    let median = middles
        .iter()
        .find(|(_, weight)| {
            below += weight;
            below >= half
        })
        .map_or((area.min[axis] + area.max[axis]) * 0.5, |(middle, _)| {
            *middle
        });
    let margin = length * 0.1;
    let middle = median.clamp(area.min[axis] + margin, area.max[axis] - margin);
    let mut lower = cell;
    lower.max[axis] = middle;
    let mut upper = cell;
    upper.min[axis] = middle;
    split_part(index, region, lower, part_points, depth + 1, cells);
    split_part(index, region, upper, part_points, depth + 1, cells);
}

/// The share of a box that lies in another, along the sides it has; a
/// box that is flat along an axis counts as wholly in along it.
fn share(bounds: Bounds, area: Bounds) -> f64 {
    (0..3)
        .map(|axis| {
            let length = bounds.max[axis] - bounds.min[axis];
            if length <= f64::EPSILON {
                return 1.0;
            }
            let inside =
                bounds.max[axis].min(area.max[axis]) - bounds.min[axis].max(area.min[axis]);
            (inside / length).clamp(0.0, 1.0)
        })
        .product()
}

/// Whether a position lies in a part: its lower faces count, its upper
/// faces belong to the next part.
fn in_cell(cell: Bounds, xyz: [f64; 3]) -> bool {
    (0..3).all(|axis| xyz[axis] >= cell.min[axis] && xyz[axis] < cell.max[axis])
}

/// The box two boxes share, faces included; none when they share nothing.
fn intersect(a: Bounds, b: Bounds) -> Option<Bounds> {
    let shared = Bounds {
        min: std::array::from_fn(|axis| a.min[axis].max(b.min[axis])),
        max: std::array::from_fn(|axis| a.max[axis].min(b.max[axis])),
    };
    // Written so that a NaN shares nothing.
    (0..3)
        .all(|axis| shared.min[axis] <= shared.max[axis])
        .then_some(shared)
}

struct Part {
    place: usize,
    parts: usize,
    cell: Bounds,
    /// Where the positions of the points are measured from.
    origin: [f64; 3],
}

/// A point to colour, with what the photos have given it so far.
#[derive(Debug, Clone, Copy)]
struct Target {
    ordinal: u64,
    /// Position relative to the origin of its part.
    xyz: [f32; 3],
    stored: [u8; 3],
    has_stored: bool,
    /// The colour and the score of the photo that sees it best so far: the
    /// lower the better, infinite for none.
    rgb: [u8; 3],
    best: f32,
    photo: u32,
    /// The colours of every photo that sees it, weighted, and their weight.
    sum: [f32; 3],
    weight: f32,
}

impl Target {
    fn new(record: &IndexedPoint, origin: [f64; 3]) -> Self {
        Self {
            ordinal: record.ordinal,
            xyz: std::array::from_fn(|axis| (record.point.xyz[axis] - origin[axis]) as f32),
            stored: record.point.rgb.unwrap_or_default(),
            has_stored: record.point.rgb.is_some(),
            rgb: [0; 3],
            best: f32::INFINITY,
            photo: u32::MAX,
            sum: [0.0; 3],
            weight: 0.0,
        }
    }

    fn seen(&self) -> bool {
        self.best.is_finite()
    }

    /// The colour the point ends with.
    fn colour(&self, blend: bool) -> [u8; 3] {
        if blend && self.weight > 0.0 {
            self.sum
                .map(|sum| (sum / self.weight).round().clamp(0.0, 255.0) as u8)
        } else {
            self.rgb
        }
    }
}

/// Points of a part that lie in one cell, next to each other in the list,
/// with the sphere around them.
struct Group {
    start: usize,
    end: usize,
    centre: [f64; 3],
    radius: f64,
}

/// Interleave the bits of three cell numbers, so that cells near each
/// other sort near each other.
fn morton(cell: [u32; 3]) -> u64 {
    fn spread(value: u32) -> u64 {
        let mut x = u64::from(value) & 0x1f_ffff;
        x = (x | (x << 32)) & 0x001f_0000_0000_ffff;
        x = (x | (x << 16)) & 0x001f_0000_ff00_00ff;
        x = (x | (x << 8)) & 0x100f_00f0_0f00_f00f;
        x = (x | (x << 4)) & 0x10c3_0c30_c30c_30c3;
        x = (x | (x << 2)) & 0x1249_2492_4924_9249;
        x
    }
    spread(cell[0]) | (spread(cell[1]) << 1) | (spread(cell[2]) << 2)
}

/// Sort the points of a part by cell and find the groups they make.
fn group(targets: &mut [Target], low: [f32; 3]) -> Vec<Group> {
    let cell_of = |target: &Target| -> [u32; 3] {
        std::array::from_fn(|axis| {
            (((target.xyz[axis] - low[axis]) as f64 / CELL).max(0.0) as u32).min(0x1f_ffff)
        })
    };
    targets.par_sort_by_cached_key(|target| (morton(cell_of(target)), target.ordinal));
    let mut groups = Vec::new();
    let mut start = 0;
    while start < targets.len() {
        let cell = cell_of(&targets[start]);
        let mut end = start + 1;
        while end < targets.len() && end - start < GROUP_POINTS && cell_of(&targets[end]) == cell {
            end += 1;
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for target in &targets[start..end] {
            for axis in 0..3 {
                min[axis] = min[axis].min(target.xyz[axis]);
                max[axis] = max[axis].max(target.xyz[axis]);
            }
        }
        let centre =
            std::array::from_fn(|axis| (f64::from(min[axis]) + f64::from(max[axis])) * 0.5);
        let radius = (0..3)
            .map(|axis| {
                let half = (f64::from(max[axis]) - f64::from(min[axis])) * 0.5;
                half * half
            })
            .sum::<f64>()
            .sqrt();
        groups.push(Group {
            start,
            end,
            centre,
            radius,
        });
        start = end;
    }
    groups
}

/// The mutable slices of the groups that pass a test, to fill on several
/// threads.
fn group_slices<'a>(
    targets: &'a mut [Target],
    groups: &[Group],
    keep: impl Fn(&Group) -> bool,
) -> Vec<&'a mut [Target]> {
    let mut slices = Vec::new();
    let mut rest = targets;
    let mut at = 0;
    for group in groups {
        let (_, tail) = std::mem::take(&mut rest).split_at_mut(group.start - at);
        let (own, tail) = tail.split_at_mut(group.end - group.start);
        rest = tail;
        at = group.end;
        if keep(group) {
            slices.push(own);
        }
    }
    slices
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Distance from a position to the nearest point of a box; zero inside.
fn box_distance(xyz: [f64; 3], bounds: Bounds) -> f64 {
    (0..3)
        .map(|axis| {
            let outside = (bounds.min[axis] - xyz[axis]).max(xyz[axis] - bounds.max[axis]);
            outside.max(0.0).powi(2)
        })
        .sum::<f64>()
        .sqrt()
}

/// A photo with what the colouring needs of it at hand.
struct View<'a> {
    photo: &'a FilePhoto,
    index: u32,
    /// Where the photo was taken, relative to the origin of the part.
    eye: [f64; 3],
    /// The direction the middle of a pinhole photo looks along, and half the
    /// angle of the cone around it that holds the whole photo; none for a
    /// panorama.
    cone: Option<([f64; 3], f64)>,
    wraps: bool,
}

impl<'a> View<'a> {
    fn new(photo: &'a FilePhoto, index: usize, origin: [f64; 3]) -> Self {
        let width = f64::from(photo.width);
        let height = f64::from(photo.height);
        let (cone, wraps) = match photo.projection {
            PhotoProjection::Pinhole { focal, principal } => {
                let across = (principal[0] + 0.5).max(width - principal[0] - 0.5) / focal[0].abs();
                let down = (principal[1] + 0.5).max(height - principal[1] - 0.5) / focal[1].abs();
                (
                    Some((photo.view_direction(), across.hypot(down).atan())),
                    false,
                )
            }
            PhotoProjection::Spherical { pixel_size }
            | PhotoProjection::Cylindrical { pixel_size, .. } => (
                None,
                pixel_size[0] * width >= std::f64::consts::TAU * (1.0 - 1e-6),
            ),
        };
        Self {
            photo,
            index: u32::try_from(index).unwrap_or(u32::MAX),
            eye: sub(photo.position, origin),
            cone,
            wraps,
        }
    }

    /// Whether some of a sphere, relative to the origin, can be in the
    /// photo within `reach` of its camera.
    fn may_see(&self, centre: [f64; 3], radius: f64, reach: f64) -> bool {
        let towards = sub(centre, self.eye);
        let distance = dot(towards, towards).sqrt();
        if distance - radius > reach {
            return false;
        }
        let Some((forward, half_angle)) = self.cone else {
            return true;
        };
        if distance <= radius {
            return true;
        }
        let cosine = (dot(towards, forward) / distance).clamp(-1.0, 1.0);
        cosine.acos() <= half_angle + (radius / distance).asin()
    }

    /// How much worse than straight on the photo sees what it shows at a
    /// pixel: one at its middle, more towards the corners of a pinhole
    /// photo.
    fn penalty(&self, pixel: [f64; 2]) -> f64 {
        match self.photo.projection {
            PhotoProjection::Pinhole { principal, .. } => {
                let across = (pixel[0] - principal[0]) / (f64::from(self.photo.width) * 0.5);
                let down = (pixel[1] - principal[1]) / (f64::from(self.photo.height) * 0.5);
                1.0 + EDGE_PENALTY * (across * across + down * down)
            }
            _ => 1.0,
        }
    }
}

/// Where a photo looks for the points of a part: what lies further away
/// than `far`, or outside the cone from the camera around the part, cannot
/// hide them.
struct Sight {
    far: f64,
    /// The direction to the middle of the part and half the angle of the
    /// cone around the part; none when the camera is in it.
    cone: Option<([f64; 3], f64)>,
}

impl Sight {
    /// The sight of a photo on the sphere around a part, relative to the
    /// origin, up to `reach`.
    fn new(view: &View<'_>, centre: [f64; 3], radius: f64, reach: f64) -> Self {
        let towards = sub(centre, view.eye);
        let distance = dot(towards, towards).sqrt();
        Self {
            far: reach.min(distance + radius),
            cone: (distance > radius).then(|| {
                (
                    towards.map(|value| value / distance),
                    (radius / distance).asin(),
                )
            }),
        }
    }

    /// Whether something in a sphere, relative to the origin, can stand
    /// between the camera and the part.
    fn crosses(&self, view: &View<'_>, centre: [f64; 3], radius: f64) -> bool {
        let towards = sub(centre, view.eye);
        let distance = dot(towards, towards).sqrt();
        if distance - radius > self.far {
            return false;
        }
        let Some((axis, half_angle)) = self.cone else {
            return true;
        };
        if distance <= radius {
            return true;
        }
        let cosine = (dot(towards, axis) / distance).clamp(-1.0, 1.0);
        cosine.acos() <= half_angle + (radius / distance).asin()
    }
}

/// The distance of the nearest point of the cloud per pixel of a photo
/// scaled down to about `DEPTH_ANGLE` per pixel.
struct DepthImage {
    width: i64,
    height: i64,
    /// Pixels of the photo across one pixel of this image.
    scale: f64,
    /// Radians across one pixel of this image, at the middle of a pinhole
    /// photo.
    angle: f64,
    wraps: bool,
    /// For a spherical photo, the radians down one row of the photo and its
    /// height in rows: a square near a pole is widened to cover the same
    /// angle as at the horizon.
    rows: Option<(f64, f64)>,
    /// Distances as the bits of positive floats, which order as the floats
    /// do; infinity where no point was seen.
    cells: Vec<AtomicU32>,
}

impl DepthImage {
    fn new(view: &View<'_>) -> Self {
        let photo = view.photo;
        let pixel_angle = match photo.projection {
            PhotoProjection::Pinhole { focal, .. } => 1.0 / focal[0].abs().max(f64::EPSILON),
            PhotoProjection::Spherical { pixel_size }
            | PhotoProjection::Cylindrical { pixel_size, .. } => pixel_size[0].abs(),
        };
        let scale = (DEPTH_ANGLE / pixel_angle).round().clamp(1.0, 64.0);
        let width = (f64::from(photo.width) / scale).ceil().max(1.0) as i64;
        let height = (f64::from(photo.height) / scale).ceil().max(1.0) as i64;
        let empty = f32::INFINITY.to_bits();
        Self {
            width,
            height,
            scale,
            angle: pixel_angle * scale,
            wraps: view.wraps,
            rows: match photo.projection {
                PhotoProjection::Spherical { pixel_size } => {
                    Some((pixel_size[1], f64::from(photo.height)))
                }
                _ => None,
            },
            cells: (0..width * height).map(|_| AtomicU32::new(empty)).collect(),
        }
    }

    fn cell(&self, pixel: [f64; 2]) -> (i64, i64) {
        let column = ((pixel[0] + 0.5) / self.scale).floor() as i64;
        let row = ((pixel[1] + 0.5) / self.scale).floor() as i64;
        (
            column.clamp(0, self.width - 1),
            row.clamp(0, self.height - 1),
        )
    }

    /// Mark a point of the cloud a photo sees at `range` metres, at a
    /// pixel of the photo, with `radius` the room around it in pixels of
    /// this image. The pixels around its own get the distance a surface
    /// seen as steeply as `SLOPE` allows would have there.
    fn mark(&self, pixel: [f64; 2], range: f32, radius: f64) {
        let (column, row) = self.cell(pixel);
        let down = (radius.round() as i64).clamp(1, MAX_SPLAT);
        // Near a pole a pixel of a spherical photo spans a smaller angle
        // across; as many are taken as span the angle of a row.
        let squeeze = match self.rows {
            Some((angle, height)) => ((height * 0.5 - (pixel[1] + 0.5)) * angle).cos().max(0.05),
            None => 1.0,
        };
        // At most half the image each way, so that a splat does not come
        // round a panorama to itself; an image of a pixel or two across, of
        // a narrow photo, still takes one.
        let across = ((down as f64 / squeeze).round() as i64)
            .max(down)
            .min((self.width / 2).max(1));
        let rise = f64::from(range) * self.angle * SLOPE;
        for y in (row - down).max(0)..=(row + down).min(self.height - 1) {
            for x in column - across..=column + across {
                let place = if self.wraps {
                    x.rem_euclid(self.width)
                } else if (0..self.width).contains(&x) {
                    x
                } else {
                    continue;
                };
                let away = ((x - column) as f64 * squeeze).hypot((y - row) as f64);
                let depth = range + (rise * away) as f32;
                self.cells[(y * self.width + place) as usize]
                    .fetch_min(depth.to_bits(), Ordering::Relaxed);
            }
        }
    }

    /// Mark a point of the cloud, relative to the origin, with `spacing`
    /// the room between it and its neighbours in metres.
    fn splat(&self, view: &View<'_>, xyz: [f64; 3], spacing: f64, reach: f64) {
        let towards = sub(xyz, view.eye);
        let squared = dot(towards, towards);
        if !(MIN_RANGE * MIN_RANGE..=reach * reach).contains(&squared) {
            return;
        }
        let Some(pixel) = view.photo.project(towards) else {
            return;
        };
        let range = squared.sqrt();
        self.mark(
            pixel,
            range as f32,
            SPLAT_SCALE * spacing / (range * self.angle),
        );
    }

    /// Whether a point at `range` metres that the photo shows at a pixel is
    /// hidden behind a nearer point of the cloud.
    fn hides(&self, pixel: [f64; 2], range: f64) -> bool {
        let (column, row) = self.cell(pixel);
        let nearest = f32::from_bits(
            self.cells[(row * self.width + column) as usize].load(Ordering::Relaxed),
        );
        let tolerance = MIN_TOLERANCE.max(range * self.angle * SLOPE);
        range > f64::from(nearest) + tolerance
    }
}

/// The nodes of an index that make the depth image of a photo, each with
/// how many of its points to read: those that can stand between the camera
/// and the part, read once a node is small enough in the image, with about
/// one point per pixel it covers.
fn depth_nodes<'a>(
    node: &'a IndexedNode,
    view: &View<'_>,
    sight: &Sight,
    origin: [f64; 3],
    angle: f64,
    nodes: &mut Vec<(&'a IndexedNode, usize)>,
) {
    if node.stored_points == 0 {
        return;
    }
    let bounds = Bounds {
        min: sub(node.bounds.min, origin),
        max: sub(node.bounds.max, origin),
    };
    let half: [f64; 3] = std::array::from_fn(|axis| (bounds.max[axis] - bounds.min[axis]) * 0.5);
    let centre = std::array::from_fn(|axis| bounds.min[axis] + half[axis]);
    let radius = dot(half, half).sqrt();
    if !view.may_see(centre, radius, sight.far) || !sight.crosses(view, centre, radius) {
        return;
    }
    let near = box_distance(view.eye, bounds);
    let extent = (0..3)
        .map(|axis| bounds.max[axis] - bounds.min[axis])
        .fold(0.0, f64::max);
    let span = extent / near.max(1e-6) / angle;
    if node.is_leaf() || span <= NODE_SPAN {
        let wanted = (span * span * SAMPLES_PER_PIXEL)
            .ceil()
            .min(usize::MAX as f64) as usize;
        let stored = usize::try_from(node.stored_points).unwrap_or(usize::MAX);
        nodes.push((node, wanted.max(MIN_NODE_SAMPLE).min(stored)));
    } else {
        for child in &node.children {
            depth_nodes(child, view, sight, origin, angle, nodes);
        }
    }
}

/// What can hide points from a photo.
enum Occluders<'a> {
    /// The nodes of an index, at a detail that follows their distance.
    Index(&'a OctreeIndex),
    /// Every point of a cloud in memory, with the room between neighbours.
    Points(&'a [IndexedPoint], f64),
    /// The points being coloured, with the room between neighbours: for a
    /// cloud that is read from its file, nothing else of it is at hand.
    Targets(f64),
}

/// The room between neighbouring points, in metres, of `count` points that
/// lie on surfaces as large as the faces of the box around them: a guess
/// for points of which nothing else is known.
fn spacing(bounds: Bounds, count: usize) -> f64 {
    let [x, y, z] = std::array::from_fn(|axis| bounds.max[axis] - bounds.min[axis]);
    (2.0 * (x * y + y * z + z * x) / count.max(1) as f64).sqrt()
}

/// Make the depth image of a photo.
fn depth_image(
    view: &View<'_>,
    sight: &Sight,
    occluders: &Occluders<'_>,
    targets: &[Target],
    origin: [f64; 3],
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<DepthImage, LoadError> {
    let reach = sight.far;
    let image = DepthImage::new(view);
    let stop = || {
        if cancelled() {
            Err(LoadError::Cancelled)
        } else {
            Ok(())
        }
    };
    match occluders {
        Occluders::Index(index) => {
            let mut nodes = Vec::new();
            depth_nodes(&index.root, view, sight, origin, image.angle, &mut nodes);
            nodes.par_iter().try_for_each(|(node, limit)| {
                stop()?;
                let points = index.read_node_sample(node, *limit, &|| cancelled())?;
                let extent = (0..3)
                    .map(|axis| node.bounds.max[axis] - node.bounds.min[axis])
                    .fold(0.0, f64::max);
                let spacing = extent / (points.len().max(1) as f64).sqrt();
                for record in &points {
                    image.splat(view, sub(record.point.xyz, origin), spacing, reach);
                }
                Ok::<(), LoadError>(())
            })?;
        }
        Occluders::Points(points, spacing) => {
            points.par_chunks(65_536).try_for_each(|chunk| {
                stop()?;
                for record in chunk {
                    image.splat(view, sub(record.point.xyz, origin), *spacing, reach);
                }
                Ok::<(), LoadError>(())
            })?;
        }
        Occluders::Targets(spacing) => {
            targets.par_chunks(65_536).try_for_each(|chunk| {
                stop()?;
                for target in chunk {
                    image.splat(view, target.xyz.map(f64::from), *spacing, reach);
                }
                Ok::<(), LoadError>(())
            })?;
        }
    }
    Ok(image)
}

/// Give the points of a part that a photo sees its colour, where it sees
/// them better than the photos before it, and add it to their blend.
fn colour_with(
    view: &View<'_>,
    image: &DepthImage,
    pixels: &PhotoPixels,
    targets: &mut [Target],
    groups: &[Group],
    reach: f64,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), LoadError> {
    let stored = [view.photo.width, view.photo.height];
    let slices = group_slices(targets, groups, |group| {
        view.may_see(group.centre, group.radius, reach)
    });
    slices.into_par_iter().try_for_each(|slice| {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        for target in slice.iter_mut() {
            let towards = sub(target.xyz.map(f64::from), view.eye);
            let squared = dot(towards, towards);
            if !(MIN_RANGE * MIN_RANGE..=reach * reach).contains(&squared) {
                continue;
            }
            let Some(pixel) = view.photo.project(towards) else {
                continue;
            };
            let range = squared.sqrt();
            if image.hides(pixel, range) {
                continue;
            }
            let colour = pixels.sample(pixel, stored, view.wraps);
            let score = (range * view.penalty(pixel)) as f32;
            if score < target.best {
                target.best = score;
                target.rgb = colour.map(|value| value.round().clamp(0.0, 255.0) as u8);
                target.photo = view.index;
            }
            let weight = score.max(0.01).powi(-4);
            for (sum, value) in target.sum.iter_mut().zip(colour) {
                *sum += weight * value;
            }
            target.weight += weight;
        }
        Ok(())
    })
}

/// What the parts add up to.
struct Tally {
    parts: usize,
    points: u64,
    seen: u64,
    reached: Vec<bool>,
    used: Vec<bool>,
    failed: Vec<bool>,
    compared: u64,
    sums: [f64; 3],
    absolute: [f64; 3],
    histograms: [[u64; 256]; 3],
    times: ColourTimes,
}

impl Tally {
    fn new(photos: usize) -> Self {
        Self {
            parts: 0,
            points: 0,
            seen: 0,
            reached: vec![false; photos],
            used: vec![false; photos],
            failed: vec![false; photos],
            compared: 0,
            sums: [0.0; 3],
            absolute: [0.0; 3],
            histograms: [[0; 256]; 3],
            times: ColourTimes::default(),
        }
    }

    fn add(&mut self, target: &Target, colour: [u8; 3]) {
        self.seen += 1;
        if let Some(used) = self.used.get_mut(target.photo as usize) {
            *used = true;
        }
        if target.has_stored {
            self.compared += 1;
            for (channel, (photo, stored)) in colour.iter().zip(target.stored).enumerate() {
                let difference = i32::from(*photo) - i32::from(stored);
                self.sums[channel] += f64::from(difference);
                self.absolute[channel] += f64::from(difference.abs());
                self.histograms[channel][difference.unsigned_abs() as usize] += 1;
            }
        }
    }

    fn finish(&self, result: &mut PhotoColouring) {
        let count = |flags: &[bool]| flags.iter().filter(|flag| **flag).count();
        result.parts = self.parts;
        result.points = self.points;
        result.seen = self.seen;
        result.photos = count(&self.reached);
        result.photos_used = count(&self.used);
        result.photos_failed = count(&self.failed);
        result.times = self.times;
        result.compared = (self.compared > 0).then(|| {
            let total = self.compared as f64;
            let median = |histogram: &[u64; 256]| {
                // The lower median: the first value with half the points at
                // or below it.
                let half = self.compared.div_ceil(2);
                let mut below = 0u64;
                histogram
                    .iter()
                    .position(|count| {
                        below += count;
                        below >= half
                    })
                    .unwrap_or(255) as u8
            };
            ColourComparison {
                points: self.compared,
                mean_difference: self.sums.map(|sum| sum / total),
                mean_abs_difference: self.absolute.map(|sum| sum / total),
                median_abs_difference: std::array::from_fn(|channel| {
                    median(&self.histograms[channel])
                }),
            }
        });
    }
}

/// A photo read and decoded, with its depth image.
struct Prepared {
    image: DepthImage,
    pixels: PhotoPixels,
}

#[allow(clippy::too_many_arguments)]
fn colour_part(
    source: &RegionSource<'_>,
    region: Bounds,
    accept: &RegionFilter<'_>,
    part: &Part,
    photos: &[FilePhoto],
    decode: &(dyn Fn(usize) -> Result<PhotoPixels, LoadError> + Sync),
    config: &PhotoColourConfig,
    progress: &mut (dyn FnMut(ColourProgress) -> Result<(), LoadError> + Send),
    deliver: &mut ColourSink<'_>,
    cancelled: &(dyn Fn() -> bool + Sync),
    tally: &mut Tally,
) -> Result<(), LoadError> {
    let Some(area) = intersect(region, part.cell) else {
        return Ok(());
    };
    let origin = part.origin;
    let started = Instant::now();
    let (parts, place) = (part.parts, part.place);
    let cell = part.cell;
    let member = |source: usize, ordinal: u64, point: &Point| {
        in_cell(cell, point.xyz) && accept(source, ordinal, point)
    };
    let (batches, _) = visit_region_parallel(
        std::slice::from_ref(source),
        area,
        &member,
        &mut |read: RegionProgress| {
            if cancelled() {
                return Err(LoadError::Cancelled);
            }
            progress(ColourProgress {
                stage: ColourStage::Reading,
                part: place,
                parts,
                done: read.read,
                total: read.total,
            })
        },
        &Vec::new,
        &|targets: &mut Vec<Target>, _, batch: &[IndexedPoint]| {
            targets.extend(batch.iter().map(|record| Target::new(record, origin)));
            Ok(())
        },
    )?;
    let mut targets = Vec::with_capacity(batches.iter().map(Vec::len).sum());
    for mut batch in batches {
        targets.append(&mut batch);
    }
    tally.times.reading += started.elapsed().as_secs_f64();
    tally.points += targets.len() as u64;
    if targets.is_empty() {
        return Ok(());
    }
    tally.parts += 1;
    let (low, high) = targets.iter().fold(
        ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
        |(low, high), target| {
            (
                std::array::from_fn(|axis| low[axis].min(target.xyz[axis])),
                std::array::from_fn(|axis| high[axis].max(target.xyz[axis])),
            )
        },
    );
    let bounds = Bounds {
        min: low.map(f64::from),
        max: high.map(f64::from),
    };
    let groups = group(&mut targets, low);
    let reach = config.max_distance;
    // The sphere around the points of the part, which the photos look at.
    let half: [f64; 3] = std::array::from_fn(|axis| (bounds.max[axis] - bounds.min[axis]) * 0.5);
    let aim_centre: [f64; 3] = std::array::from_fn(|axis| bounds.min[axis] + half[axis]);
    let aim_radius = dot(half, half).sqrt() + 1e-3;
    let views: Vec<View<'_>> = photos
        .iter()
        .enumerate()
        .filter(|(_, photo)| photo.width > 0 && photo.height > 0)
        .map(|(index, photo)| View::new(photo, index, origin))
        .filter(|view| {
            groups
                .iter()
                .any(|group| view.may_see(group.centre, group.radius, reach))
        })
        .collect();
    for view in &views {
        tally.reached[view.index as usize] = true;
    }
    let occluders = match source.reader {
        RegionReader::Index(index) => Occluders::Index(index),
        RegionReader::Resident(points) => Occluders::Points(
            points,
            source
                .world_bounds()
                .map_or(0.0, |bounds| spacing(bounds, points.len())),
        ),
        RegionReader::Stream(_) => Occluders::Targets(spacing(bounds, targets.len())),
    };
    let total = views.len() as u64;
    let report = |done: u64| ColourProgress {
        stage: ColourStage::Photos,
        part: place,
        parts,
        done,
        total,
    };
    progress(report(0))?;
    let photos_started = Instant::now();
    let decoding = AtomicU64::new(0);
    let depth = AtomicU64::new(0);
    let mut done = 0u64;
    for batch in views.chunks(PHOTO_BATCH) {
        if cancelled() {
            return Err(LoadError::Cancelled);
        }
        let shared: &[Target] = &targets;
        let prepared: Vec<Result<Option<Prepared>, LoadError>> = batch
            .par_iter()
            .map(|view| {
                let started = Instant::now();
                let pixels = match decode(view.index as usize) {
                    Ok(pixels) if pixels.valid() => pixels,
                    Err(LoadError::Cancelled) => return Err(LoadError::Cancelled),
                    // A photo that cannot be read takes no part.
                    Ok(_) | Err(_) => return Ok(None),
                };
                decoding.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                let started = Instant::now();
                let sight = Sight::new(view, aim_centre, aim_radius, reach);
                let image = depth_image(view, &sight, &occluders, shared, origin, cancelled)?;
                depth.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                Ok(Some(Prepared { image, pixels }))
            })
            .collect();
        for (view, prepared) in batch.iter().zip(prepared) {
            match prepared? {
                Some(prepared) => {
                    let started = Instant::now();
                    colour_with(
                        view,
                        &prepared.image,
                        &prepared.pixels,
                        &mut targets,
                        &groups,
                        reach,
                        cancelled,
                    )?;
                    tally.times.colouring += started.elapsed().as_secs_f64();
                }
                None => tally.failed[view.index as usize] = true,
            }
            done += 1;
            progress(report(done))?;
        }
    }
    tally.times.photos += photos_started.elapsed().as_secs_f64();
    tally.times.decoding += decoding.into_inner() as f64 * 1e-9;
    tally.times.depth += depth.into_inner() as f64 * 1e-9;
    let mut colours = Vec::with_capacity(targets.len());
    for target in targets.iter().filter(|target| target.seen()) {
        let colour = target.colour(config.blend);
        tally.add(target, colour);
        colours.push((target.ordinal, colour));
    }
    drop(targets);
    deliver(&colours)
}

/// Points of a cloud per block of the colour table.
const BLOCK_BITS: u32 = 12;
const BLOCK_POINTS: usize = 1 << BLOCK_BITS;

#[derive(Clone)]
struct ColourBlock {
    painted: [u64; BLOCK_POINTS / 64],
    rgb: [[u8; 3]; BLOCK_POINTS],
}

/// Colours given to points of a cloud, by their ordinal in its source, in
/// place of the colours the source stores. The table is kept in blocks of
/// ordinals that copies of it share until one of them changes a block, so
/// that a copy with a few more colours costs little.
#[derive(Clone, Default)]
pub struct PointColours {
    blocks: Vec<Option<Arc<ColourBlock>>>,
    total: u64,
    count: u64,
}

impl std::fmt::Debug for PointColours {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PointColours")
            .field("total", &self.total)
            .field("count", &self.count)
            .finish()
    }
}

impl PointColours {
    /// No colours yet, for a cloud of `total` points.
    pub fn new(total: u64) -> Self {
        Self {
            blocks: Vec::new(),
            total,
            count: 0,
        }
    }

    /// Points that have a colour.
    pub fn len(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The points of the cloud this table is for.
    pub fn total(&self) -> u64 {
        self.total
    }

    fn place(ordinal: u64) -> (usize, usize) {
        (
            (ordinal >> BLOCK_BITS) as usize,
            (ordinal as usize) & (BLOCK_POINTS - 1),
        )
    }

    /// The colour given to a point, if any.
    pub fn get(&self, ordinal: u64) -> Option<[u8; 3]> {
        if ordinal >= self.total {
            return None;
        }
        let (block, at) = Self::place(ordinal);
        let block = self.blocks.get(block)?.as_ref()?;
        (block.painted[at / 64] & (1u64 << (at % 64)) != 0).then_some(block.rgb[at])
    }

    /// Give a point a colour. An ordinal outside the cloud, such as the one
    /// of a point whose ordinal is unknown, is left out.
    pub fn set(&mut self, ordinal: u64, rgb: [u8; 3]) {
        if ordinal >= self.total {
            return;
        }
        let (block, at) = Self::place(ordinal);
        if block >= self.blocks.len() {
            self.blocks.resize(block + 1, None);
        }
        let block = Arc::make_mut(self.blocks[block].get_or_insert_with(|| {
            Arc::new(ColourBlock {
                painted: [0; BLOCK_POINTS / 64],
                rgb: [[0; 3]; BLOCK_POINTS],
            })
        }));
        let bit = 1u64 << (at % 64);
        if block.painted[at / 64] & bit == 0 {
            block.painted[at / 64] |= bit;
            self.count += 1;
        }
        block.rgb[at] = rgb;
    }

    /// A point with the colour it was given, if any.
    pub fn paint(&self, mut record: IndexedPoint) -> IndexedPoint {
        if let Some(rgb) = self.get(record.ordinal) {
            record.point.rgb = Some(rgb);
        }
        record
    }

    /// Bytes the table holds, counting a block shared with a copy as well.
    pub fn bytes(&self) -> usize {
        self.blocks.len() * std::mem::size_of::<Option<Arc<ColourBlock>>>()
            + self.blocks.iter().flatten().count() * std::mem::size_of::<ColourBlock>()
    }

    /// Bytes the table adds to those of the tables before it: its blocks
    /// that are not among `held`, the blocks those tables hold, and that
    /// `held` then holds as well.
    pub fn bytes_beside(&self, held: &mut std::collections::HashSet<usize>) -> usize {
        let blocks = self
            .blocks
            .iter()
            .flatten()
            .filter(|block| held.insert(Arc::as_ptr(block) as usize))
            .count();
        self.blocks.len() * std::mem::size_of::<Option<Arc<ColourBlock>>>()
            + blocks * std::mem::size_of::<ColourBlock>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region_source::EVERYWHERE;
    use crate::test_shapes::indexed_cloud;
    use crate::ScanImageFormat;

    /// The room: inside faces of a box of 6 by 4 by 3 m.
    const ROOM: [f64; 3] = [6.0, 4.0, 3.0];
    /// A panel at x = 4.5 between a camera and the far wall.
    const PANEL_X: f64 = 4.5;
    const PANEL_Y: [f64; 2] = [1.5, 2.5];
    const PANEL_Z: [f64; 2] = [1.0, 2.0];
    const PANEL_COLOUR: [u8; 3] = [250, 20, 220];
    /// The pinhole camera that looks at the panel, and the panorama behind
    /// it.
    const NEAR: [f64; 3] = [3.0, 2.0, 1.5];
    const FAR: [f64; 3] = [1.0, 2.0, 1.5];

    /// The colour of the room at a position: smooth, so a photo sampled a
    /// little beside a point gives nearly the same colour.
    fn room_colour(xyz: [f64; 3]) -> [u8; 3] {
        let [x, y, z] = xyz;
        [
            128.0 + 100.0 * (1.3 * x + 0.4 * z).sin(),
            128.0 + 100.0 * (1.1 * y + 0.7 * z + 1.0).sin(),
            128.0 + 100.0 * (0.9 * x + 0.8 * y).cos(),
        ]
        .map(|value| value.round() as u8)
    }

    fn lattice(from: f64, to: f64, spacing: f64) -> Vec<f64> {
        let count = ((to - from) / spacing).round().max(1.0) as usize;
        let step = (to - from) / count as f64;
        (0..count)
            .map(|index| from + (index as f64 + 0.5) * step)
            .collect()
    }

    /// The points of the room and of the panel, every `spacing` metres, the
    /// room first. With `stored` they carry their true colours.
    fn scene(spacing: f64, stored: bool) -> Vec<Point> {
        let mut positions = Vec::new();
        let [sx, sy, sz] = ROOM;
        for x in lattice(0.0, sx, spacing) {
            for y in lattice(0.0, sy, spacing) {
                positions.push([x, y, 0.0]);
                positions.push([x, y, sz]);
            }
            for z in lattice(0.0, sz, spacing) {
                positions.push([x, 0.0, z]);
                positions.push([x, sy, z]);
            }
        }
        for y in lattice(0.0, sy, spacing) {
            for z in lattice(0.0, sz, spacing) {
                positions.push([0.0, y, z]);
                positions.push([sx, y, z]);
            }
        }
        let room = positions.len();
        for y in lattice(PANEL_Y[0], PANEL_Y[1], spacing) {
            for z in lattice(PANEL_Z[0], PANEL_Z[1], spacing) {
                positions.push([PANEL_X, y, z]);
            }
        }
        positions
            .into_iter()
            .enumerate()
            .map(|(index, xyz)| Point {
                xyz,
                rgb: stored.then(|| {
                    if index < room {
                        room_colour(xyz)
                    } else {
                        PANEL_COLOUR
                    }
                }),
                intensity: None,
                classification: None,
            })
            .collect()
    }

    fn on_panel(xyz: [f64; 3]) -> bool {
        xyz[0] == PANEL_X
            && (PANEL_Y[0]..=PANEL_Y[1]).contains(&xyz[1])
            && (PANEL_Z[0]..=PANEL_Z[1]).contains(&xyz[2])
    }

    /// What a ray from `eye` along `direction` meets first: the panel or a
    /// face of the room, with its colour.
    fn trace(eye: [f64; 3], direction: [f64; 3]) -> [u8; 3] {
        let mut nearest = f64::INFINITY;
        for axis in 0..3 {
            let wall = if direction[axis] > 0.0 {
                ROOM[axis]
            } else if direction[axis] < 0.0 {
                0.0
            } else {
                continue;
            };
            nearest = nearest.min((wall - eye[axis]) / direction[axis]);
        }
        if direction[0] > 0.0 {
            let t = (PANEL_X - eye[0]) / direction[0];
            let y = eye[1] + t * direction[1];
            let z = eye[2] + t * direction[2];
            if t > 0.0
                && t < nearest
                && (PANEL_Y[0]..=PANEL_Y[1]).contains(&y)
                && (PANEL_Z[0]..=PANEL_Z[1]).contains(&z)
            {
                return PANEL_COLOUR;
            }
        }
        room_colour(std::array::from_fn(|axis| {
            eye[axis] + nearest * direction[axis]
        }))
    }

    /// The photo a camera takes of the scene.
    fn render(photo: &FilePhoto) -> PhotoPixels {
        let mut rgba = Vec::with_capacity(photo.width as usize * photo.height as usize * 4);
        for row in 0..photo.height {
            for column in 0..photo.width {
                let colour = trace(photo.position, photo.ray(f64::from(column), f64::from(row)));
                rgba.extend_from_slice(&[colour[0], colour[1], colour[2], 255]);
            }
        }
        PhotoPixels {
            width: photo.width,
            height: photo.height,
            rgba,
        }
    }

    /// A pinhole photo of 90 degrees across, looking along +X with +Z up.
    fn pinhole(position: [f64; 3]) -> FilePhoto {
        FilePhoto {
            name: None,
            station: None,
            position,
            axes: [[0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]],
            width: 240,
            height: 180,
            projection: PhotoProjection::Pinhole {
                focal: [120.0, 120.0],
                principal: [119.5, 89.5],
            },
            format: ScanImageFormat::Png,
            offset: 0,
            length: 1,
        }
    }

    /// A panorama of half a degree per pixel, its middle along +X.
    fn panorama(position: [f64; 3]) -> FilePhoto {
        FilePhoto {
            name: None,
            station: None,
            position,
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            width: 720,
            height: 360,
            projection: PhotoProjection::Spherical {
                pixel_size: [std::f64::consts::TAU / 720.0, std::f64::consts::PI / 360.0],
            },
            format: ScanImageFormat::Jpeg,
            offset: 0,
            length: 1,
        }
    }

    struct Run {
        colours: std::collections::HashMap<u64, [u8; 3]>,
        result: PhotoColouring,
        deliveries: usize,
    }

    fn run_with(
        source: RegionSource<'_>,
        region: Bounds,
        accept: &RegionFilter<'_>,
        photos: &[FilePhoto],
        config: PhotoColourConfig,
    ) -> Result<Run, LoadError> {
        let rendered: Vec<PhotoPixels> = photos.iter().map(render).collect();
        let mut colours = std::collections::HashMap::new();
        let mut deliveries = 0;
        let result = colour_from_photos(
            source,
            region,
            accept,
            photos,
            &|index| Ok(rendered[index].clone()),
            &config,
            &mut |_| Ok(()),
            &mut |part| {
                deliveries += 1;
                for (ordinal, rgb) in part {
                    assert!(colours.insert(*ordinal, *rgb).is_none(), "coloured twice");
                }
                Ok(())
            },
            &|| false,
        )?;
        Ok(Run {
            colours,
            result,
            deliveries,
        })
    }

    fn resident(points: &[Point]) -> Vec<IndexedPoint> {
        points
            .iter()
            .enumerate()
            .map(|(ordinal, point)| IndexedPoint {
                point: *point,
                ordinal: ordinal as u64,
            })
            .collect()
    }

    fn run(points: &[Point], photos: &[FilePhoto], config: PhotoColourConfig) -> Run {
        let records = resident(points);
        run_with(
            RegionSource::resident(&records, SourceTransform::default()),
            EVERYWHERE,
            &|_, _, _| true,
            photos,
            config,
        )
        .unwrap()
    }

    /// Each point from the photo that sees it best alone.
    fn best_photo() -> PhotoColourConfig {
        PhotoColourConfig {
            blend: false,
            ..PhotoColourConfig::default()
        }
    }

    fn difference(a: [u8; 3], b: [u8; 3]) -> u8 {
        (0..3)
            .map(|channel| a[channel].abs_diff(b[channel]))
            .max()
            .unwrap()
    }

    /// Where on the far wall the panel hides a camera's view of: the
    /// panel's shadow, as y and z ranges.
    fn shadow(eye: [f64; 3]) -> [[f64; 2]; 2] {
        let stretch = (ROOM[0] - eye[0]) / (PANEL_X - eye[0]);
        let around =
            |centre: f64, ends: [f64; 2]| ends.map(|end| centre + (end - centre) * stretch);
        [around(eye[1], PANEL_Y), around(eye[2], PANEL_Z)]
    }

    /// Whether a point of the far wall lies inside a shadow by `margin`, or
    /// outside it by `margin` when `margin` is negative.
    fn in_shadow(xyz: [f64; 3], shadow: [[f64; 2]; 2], margin: f64) -> bool {
        xyz[0] == ROOM[0]
            && (shadow[0][0] + margin..=shadow[0][1] - margin).contains(&xyz[1])
            && (shadow[1][0] + margin..=shadow[1][1] - margin).contains(&xyz[2])
    }

    fn outside_shadow(xyz: [f64; 3], shadow: [[f64; 2]; 2], margin: f64) -> bool {
        xyz[0] != ROOM[0]
            || !((shadow[0][0] - margin..=shadow[0][1] + margin).contains(&xyz[1])
                && (shadow[1][0] - margin..=shadow[1][1] + margin).contains(&xyz[2]))
    }

    #[test]
    fn a_nearer_face_hides_what_lies_behind_it_from_a_photo() {
        let points = scene(0.04, false);
        let near = pinhole(NEAR);
        let run = run(&points, &[near], best_photo());
        let behind = shadow(NEAR);
        let mut hidden = 0;
        let mut checked = 0;
        for (ordinal, point) in points.iter().enumerate() {
            let colour = run.colours.get(&(ordinal as u64)).copied();
            if on_panel(point.xyz) {
                assert!(difference(colour.unwrap(), PANEL_COLOUR) <= 2, "{colour:?}");
            } else if in_shadow(point.xyz, behind, 0.1) {
                // Never the colour of the panel in front of it.
                assert_eq!(colour, None, "{:?}", point.xyz);
                hidden += 1;
            } else if let Some(colour) = colour {
                if outside_shadow(point.xyz, behind, 0.1) {
                    assert!(
                        difference(colour, room_colour(point.xyz)) <= 8,
                        "{:?}: {colour:?} against {:?}",
                        point.xyz,
                        room_colour(point.xyz)
                    );
                    checked += 1;
                }
            }
        }
        assert!(hidden > 500, "{hidden}");
        assert!(checked > 5_000, "{checked}");
        assert_eq!(run.result.photos, 1);
        assert_eq!(run.result.photos_used, 1);
        assert_eq!(run.result.points, points.len() as u64);
        assert_eq!(run.result.seen, run.colours.len() as u64);
        assert!(run.result.compared.is_none());
    }

    #[test]
    fn a_farther_photo_colours_what_the_nearer_one_cannot_see() {
        let points = scene(0.04, false);
        let photos = [pinhole(NEAR), panorama(FAR)];
        let run = run(&points, &photos, best_photo());
        let (near, far) = (shadow(NEAR), shadow(FAR));
        let mut from_far = 0;
        let mut unseen = 0;
        let mut errors = Vec::new();
        for (ordinal, point) in points.iter().enumerate() {
            let colour = run.colours.get(&(ordinal as u64)).copied();
            let truth = if on_panel(point.xyz) {
                PANEL_COLOUR
            } else {
                room_colour(point.xyz)
            };
            if in_shadow(point.xyz, far, 0.15) {
                assert_eq!(colour, None, "{:?}", point.xyz);
                unseen += 1;
            } else if in_shadow(point.xyz, near, 0.1) && outside_shadow(point.xyz, far, 0.15) {
                // Only the panorama sees it, though the pinhole photo is
                // nearer and shows the panel there.
                let colour = colour.unwrap();
                assert!(difference(colour, truth) <= 12, "{:?}", point.xyz);
                from_far += 1;
            } else if outside_shadow(point.xyz, far, 0.15) && outside_shadow(point.xyz, near, 0.15)
            {
                errors.push(difference(colour.unwrap(), truth));
            }
        }
        assert!(unseen > 100 && from_far > 100, "{unseen} {from_far}");
        // Apart from what the panel hides from both, hardly a point is
        // left unseen.
        let elsewhere = points
            .iter()
            .enumerate()
            .filter(|(ordinal, point)| {
                outside_shadow(point.xyz, far, 0.15)
                    && !run.colours.contains_key(&(*ordinal as u64))
            })
            .count();
        assert!(elsewhere < points.len() / 200, "{elsewhere}");
        errors.sort_unstable();
        let median = errors[errors.len() / 2];
        let worst = errors[errors.len() * 99 / 100];
        assert!(median <= 3 && worst <= 15, "{median} {worst}");
        assert_eq!(run.result.photos_used, 2);
        assert!(
            run.result.unseen_share() < 0.03,
            "{}",
            run.result.unseen_share()
        );
    }

    #[test]
    fn colours_compare_with_the_colours_the_points_had() {
        let mut points = scene(0.05, true);
        let photos = [pinhole(NEAR), panorama(FAR)];
        let run_true = run(&points, &photos, PhotoColourConfig::default());
        let compared = run_true.result.compared.unwrap();
        assert_eq!(compared.points, run_true.result.seen);
        for channel in 0..3 {
            assert!(compared.mean_abs_difference[channel] < 3.0, "{compared:?}");
            assert!(compared.median_abs_difference[channel] <= 2, "{compared:?}");
            assert!(
                compared.mean_difference[channel].abs() < 1.5,
                "{compared:?}"
            );
        }
        // Stored colours 30 levels redder than the photos see them.
        for point in &mut points {
            if let Some(rgb) = &mut point.rgb {
                rgb[0] = rgb[0].saturating_add(30);
            }
        }
        let shifted = run(&points, &photos, PhotoColourConfig::default())
            .result
            .compared
            .unwrap();
        assert!(
            (shifted.mean_difference[0] + 30.0).abs() < 4.0,
            "{shifted:?}"
        );
        assert!(
            (25..=31).contains(&shifted.median_abs_difference[0]),
            "{shifted:?}"
        );
        assert!(shifted.median_abs_difference[1] <= 2);
    }

    #[test]
    fn a_blend_keeps_close_to_the_best_photo() {
        let points = scene(0.05, true);
        let photos = [pinhole(NEAR), panorama(FAR), panorama([5.0, 1.0, 2.0])];
        let blended = run(&points, &photos, PhotoColourConfig::default());
        assert!(PhotoColourConfig::default().blend);
        let compared = blended.result.compared.unwrap();
        for channel in 0..3 {
            assert!(compared.mean_abs_difference[channel] < 4.0, "{compared:?}");
        }
        assert_eq!(blended.result.photos_used, 3);
    }

    #[test]
    fn photos_beyond_the_largest_distance_give_no_colour() {
        let points = scene(0.05, false);
        let run = run(
            &points,
            &[pinhole(NEAR)],
            PhotoColourConfig {
                max_distance: 2.0,
                ..PhotoColourConfig::default()
            },
        );
        assert!(!run.colours.is_empty());
        for ordinal in run.colours.keys() {
            let xyz = points[*ordinal as usize].xyz;
            let distance = sub(xyz, NEAR)
                .map(|value| value * value)
                .iter()
                .sum::<f64>()
                .sqrt();
            assert!(distance <= 2.0 + 1e-9, "{xyz:?}");
        }
        // A photo too far from every point is not read at all.
        let none = run_with_far_photo(&points);
        assert_eq!((none.result.photos, none.result.seen), (0, 0));
    }

    fn run_with_far_photo(points: &[Point]) -> Run {
        let records = resident(points);
        run_with(
            RegionSource::resident(&records, SourceTransform::default()),
            EVERYWHERE,
            &|_, _, _| true,
            &[panorama([100.0, 2.0, 1.5])],
            PhotoColourConfig::default(),
        )
        .unwrap()
    }

    #[test]
    fn an_index_hides_points_as_the_points_in_memory_do_and_parts_change_nothing() {
        let points = scene(0.04, false);
        let photos = [pinhole(NEAR), panorama(FAR)];
        let in_memory = run(&points, &photos, PhotoColourConfig::default());
        let indexed = indexed_cloud(&points, 4_096);
        assert!(!indexed.index.root.is_leaf());
        let source = RegionSource::new(
            &indexed.cloud,
            Some(&indexed.index),
            SourceTransform::default(),
        );
        let whole = run_with(
            source,
            EVERYWHERE,
            &|_, _, _| true,
            &photos,
            PhotoColourConfig::default(),
        )
        .unwrap();
        assert_eq!(whole.result.parts, 1);
        assert_eq!(whole.result.points, points.len() as u64);
        // The depth images of the index are made of samples of its nodes:
        // nearly every point is seen or hidden as with every point at hand.
        let agree = points
            .iter()
            .enumerate()
            .filter(|(ordinal, _)| {
                let ordinal = *ordinal as u64;
                in_memory.colours.contains_key(&ordinal) == whole.colours.contains_key(&ordinal)
            })
            .count();
        assert!(agree as f64 > 0.99 * points.len() as f64, "{agree}");
        let (near, far) = (shadow(NEAR), shadow(FAR));
        for (ordinal, point) in points.iter().enumerate() {
            if in_shadow(point.xyz, far, 0.15) {
                assert!(!whole.colours.contains_key(&(ordinal as u64)));
            }
            if in_shadow(point.xyz, near, 0.1) && outside_shadow(point.xyz, far, 0.15) {
                let colour = whole.colours[&(ordinal as u64)];
                assert!(difference(colour, room_colour(point.xyz)) <= 12);
            }
        }

        // In parts of at most 10,000 points every point is coloured once,
        // the same as in one part: the depth images do not depend on parts.
        let split = run_with(
            source,
            EVERYWHERE,
            &|_, _, _| true,
            &photos,
            PhotoColourConfig {
                part_points: 10_000,
                ..PhotoColourConfig::default()
            },
        )
        .unwrap();
        assert!(split.result.parts > 4, "{}", split.result.parts);
        assert_eq!(split.deliveries, split.result.parts);
        assert_eq!(split.result.points, points.len() as u64);
        assert_eq!(split.colours, whole.colours);
    }

    #[test]
    fn only_the_points_of_the_region_that_are_accepted_are_coloured() {
        let points = scene(0.05, false);
        let records = resident(&points);
        let region = Bounds {
            min: [3.0, -1.0, -1.0],
            max: [7.0, 5.0, 4.0],
        };
        let taken = |ordinal: u64, point: &Point| {
            ordinal.is_multiple_of(2) && point.xyz[0] >= 3.0 && !on_panel(point.xyz)
        };
        let run = run_with(
            RegionSource::resident(&records, SourceTransform::default()),
            region,
            &|_, ordinal, point| ordinal.is_multiple_of(2) && !on_panel(point.xyz),
            &[pinhole(NEAR), panorama(FAR)],
            PhotoColourConfig::default(),
        )
        .unwrap();
        let expected = points
            .iter()
            .enumerate()
            .filter(|(ordinal, point)| taken(*ordinal as u64, point))
            .count() as u64;
        assert_eq!(run.result.points, expected);
        for ordinal in run.colours.keys() {
            assert!(taken(*ordinal, &points[*ordinal as usize]));
        }
        // Points left out still hide what lies behind them: the panel is not
        // coloured, and the wall behind it stays unseen.
        let far = shadow(FAR);
        for (ordinal, point) in points.iter().enumerate() {
            if in_shadow(point.xyz, far, 0.15) {
                assert!(!run.colours.contains_key(&(ordinal as u64)));
            }
        }
    }

    #[test]
    fn a_photo_that_cannot_be_decoded_is_left_out_and_cancelling_stops() {
        let points = scene(0.1, false);
        let records = resident(&points);
        let source = RegionSource::resident(&records, SourceTransform::default());
        let photos = [pinhole(NEAR), panorama(FAR)];
        let good = render(&photos[1]);
        let result = colour_from_photos(
            source,
            EVERYWHERE,
            &|_, _, _| true,
            &photos,
            &|index| match index {
                0 => Err(LoadError::InvalidData("not a photo".into())),
                _ => Ok(good.clone()),
            },
            &PhotoColourConfig::default(),
            &mut |_| Ok(()),
            &mut |_| Ok(()),
            &|| false,
        )
        .unwrap();
        assert_eq!(
            (result.photos, result.photos_failed, result.photos_used),
            (2, 1, 1)
        );
        let cancelled = colour_from_photos(
            source,
            EVERYWHERE,
            &|_, _, _| true,
            &photos,
            &|_| Ok(good.clone()),
            &PhotoColourConfig::default(),
            &mut |_| Ok(()),
            &mut |_| Ok(()),
            &|| true,
        );
        assert!(matches!(cancelled, Err(LoadError::Cancelled)));
        let moved = colour_from_photos(
            RegionSource::resident(
                &records,
                SourceTransform {
                    scale: [1.0; 3],
                    offset: [1.0, 0.0, 0.0],
                },
            ),
            EVERYWHERE,
            &|_, _, _| true,
            &photos,
            &|_| Ok(good.clone()),
            &PhotoColourConfig::default(),
            &mut |_| Ok(()),
            &mut |_| Ok(()),
            &|| false,
        );
        assert!(matches!(moved, Err(LoadError::InvalidData(_))));
    }

    /// A photo whose depth image is a few pixels across, narrower than the
    /// splat of a point it sees, still marks its points and colours them.
    #[test]
    fn a_narrow_photo_has_a_depth_image_of_a_few_pixels() {
        let points = scene(0.25, false);
        // A photo of one pixel.
        let speck = FilePhoto {
            width: 1,
            height: 1,
            projection: PhotoProjection::Pinhole {
                focal: [1.0, 1.0],
                principal: [0.0, 0.0],
            },
            ..pinhole(NEAR)
        };
        // Two degrees across, aimed at a point of the panel: a depth image of
        // ten pixels, where that point covers more than ten.
        let tele = FilePhoto {
            width: 600,
            height: 450,
            projection: PhotoProjection::Pinhole {
                focal: [17_000.0, 17_000.0],
                principal: [299.5, 224.5],
            },
            ..pinhole([3.0, 1.875, 1.625])
        };
        let image = DepthImage::new(&View::new(&tele, 0, [0.0; 3]));
        assert_eq!((image.width, image.height), (10, 8));
        for photo in [speck, tele] {
            let run = run(&points, &[photo], best_photo());
            assert_eq!(run.result.photos_used, 1);
            assert!(run.result.seen > 0, "{:?}", run.result);
        }
    }

    #[test]
    fn a_panorama_is_sampled_across_its_seam() {
        let pixels = PhotoPixels {
            width: 4,
            height: 1,
            rgba: [[200, 0, 0, 255], [0; 4], [0; 4], [0, 0, 100, 255]].concat(),
        };
        // Between the last and the first column.
        let colour = pixels.sample([3.5, 0.0], [4, 1], true);
        assert_eq!(colour.map(f32::round), [100.0, 0.0, 50.0]);
        let clamped = pixels.sample([3.5, 0.0], [4, 1], false);
        assert_eq!(clamped.map(f32::round), [0.0, 0.0, 100.0]);
        // A photo decoded at half its size is sampled where it was stored.
        let half = pixels.sample([1.5, 0.0], [8, 2], false);
        assert_eq!(half.map(f32::round), [100.0, 0.0, 0.0]);
    }

    #[test]
    fn a_colour_table_shares_its_blocks_with_its_copies() {
        let mut colours = PointColours::new(10_000);
        assert!(colours.is_empty());
        colours.set(5, [1, 2, 3]);
        colours.set(9_999, [4, 5, 6]);
        colours.set(10_000, [7, 8, 9]);
        colours.set(u64::MAX, [7, 8, 9]);
        assert_eq!(colours.len(), 2);
        assert_eq!(colours.get(5), Some([1, 2, 3]));
        assert_eq!(colours.get(6), None);
        assert_eq!(colours.get(10_000), None);
        let mut copy = colours.clone();
        copy.set(5, [9, 9, 9]);
        copy.set(6, [8, 8, 8]);
        assert_eq!(
            (colours.get(5), copy.get(5)),
            (Some([1, 2, 3]), Some([9, 9, 9]))
        );
        assert_eq!((colours.len(), copy.len()), (2, 3));
        // The block of the last point was not changed and is still shared.
        assert!(Arc::ptr_eq(
            colours.blocks[2].as_ref().unwrap(),
            copy.blocks[2].as_ref().unwrap()
        ));
        let record = IndexedPoint {
            point: Point {
                xyz: [0.0; 3],
                rgb: None,
                intensity: None,
                classification: None,
            },
            ordinal: 6,
        };
        assert_eq!(copy.paint(record).point.rgb, Some([8, 8, 8]));
        assert_eq!(colours.paint(record).point.rgb, None);

        // Beside the copy, the table adds only the block that differs.
        let block = std::mem::size_of::<ColourBlock>();
        let list = 3 * std::mem::size_of::<Option<Arc<ColourBlock>>>();
        assert_eq!(colours.bytes(), list + 2 * block);
        let mut held = std::collections::HashSet::new();
        assert_eq!(copy.bytes_beside(&mut held), list + 2 * block);
        assert_eq!(colours.bytes_beside(&mut held), list + block);
        assert_eq!(colours.bytes_beside(&mut held), list);
    }
}
