//! The points of the slab of a drawing, kept in memory so that the drawing
//! can be made again from them when its crop region changes.
//!
//! What is kept is, per octree leaf that the slab touched, every point of
//! that leaf in the band of the scene the slab lies in: from the cut plane as
//! deep as the drawing sees, however far the leaf reaches across the drawing.
//! A crop region that shrinks, or moves over leaves that were read, is drawn
//! from memory without reading anything; one that grows reads only the
//! leaves it touches for the first time. Another cut, depth or share of the
//! points, other layers or a layer that moved start over.
//!
//! Deleted points and hidden classes are left out each time the points are
//! drawn, not when they are kept, so that a deletion or another class shown
//! needs no read either.
//!
//! A kept point takes 24 bytes: its place as an offset in single precision
//! from the middle of its leaf, which stays within a micrometre for a leaf
//! of tens of metres, its colour, its class and its ordinal. Its intensity
//! is not kept; a drawing has no use for it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;

use super::section::{DrawingProgress, DrawingStage};
use super::slab::{
    check_options, collect_passes, collect_slab, finish_passes, slab_work, Gathered, Slab, SlabCut,
    SlabOptions,
};
use crate::region_source::{
    contains, overlaps, visit_region, RegionFilter, RegionProgress, RegionReader, RegionSource,
    SourceTransform, EVERYWHERE,
};
use crate::surface_mesh::sampled_ordinal;
use crate::{Bounds, IndexedNode, IndexedPoint, LoadError, OctreeIndex, Point, PointCloud};

/// The most threads that read leaves, or draw kept points, at a time.
const MAX_THREADS: usize = 8;

/// Points between two reports while the kept points are drawn.
const REPORT_POINTS: usize = 65_536;

/// Points a thread takes at a time, between two looks at a cancel.
const CHUNK_POINTS: usize = 4_096;

/// How far a point may lie outside the band and still be kept, in metres:
/// the band of a turned box is worked out from its corners, and its own test
/// may round the other way.
const BAND_MARGIN: f64 = 1e-6;

/// Below this many points the kept points are drawn on one thread.
const PARALLEL_FROM: usize = 400_000;

/// The copies of the grid of the filled cut that the threads drawing the
/// kept points fill take at most this much memory together.
const MAX_GRID_COPIES: usize = 256 << 20;

/// The largest ordinal a kept point holds.
const MAX_ORDINAL: u64 = (1 << 48) - 1;

/// A point as it is kept: where it stands in the scene, from the anchor of
/// its leaf, what it carries and its ordinal in its source file.
#[derive(Debug, Clone, Copy, PartialEq)]
struct KeptPoint {
    offset: [f32; 3],
    ordinal_low: u32,
    ordinal_high: u16,
    rgb: [u8; 3],
    classification: u8,
    /// Which of the colour and the class the point has.
    has: u8,
}

const HAS_RGB: u8 = 1;
const HAS_CLASSIFICATION: u8 = 2;

/// Bytes of memory a kept point takes.
pub const KEPT_POINT_BYTES: usize = std::mem::size_of::<KeptPoint>();

impl KeptPoint {
    fn of(point: &Point, ordinal: u64, anchor: [f64; 3]) -> Self {
        let mut has = 0;
        if point.rgb.is_some() {
            has |= HAS_RGB;
        }
        if point.classification.is_some() {
            has |= HAS_CLASSIFICATION;
        }
        Self {
            offset: std::array::from_fn(|axis| (point.xyz[axis] - anchor[axis]) as f32),
            ordinal_low: ordinal as u32,
            ordinal_high: (ordinal >> 32) as u16,
            rgb: point.rgb.unwrap_or_default(),
            classification: point.classification.unwrap_or_default(),
            has,
        }
    }

    fn xyz(&self, anchor: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| anchor[axis] + f64::from(self.offset[axis]))
    }

    fn record(&self, anchor: [f64; 3]) -> IndexedPoint {
        let with = |flag: u8| self.has & flag != 0;
        IndexedPoint {
            point: Point {
                xyz: self.xyz(anchor),
                rgb: with(HAS_RGB).then_some(self.rgb),
                intensity: None,
                classification: with(HAS_CLASSIFICATION).then_some(self.classification),
            },
            ordinal: (u64::from(self.ordinal_high) << 32) | u64::from(self.ordinal_low),
        }
    }
}

/// The kept points of a leaf, or of a layer without an index, with the
/// place they are kept from.
#[derive(Debug, Clone, PartialEq)]
struct KeptUnit {
    anchor: [f64; 3],
    points: Vec<KeptPoint>,
}

/// What a layer is, as far as the kept points go: what its points are read
/// from and where it stands.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SourceIdentity {
    /// The address of the index, the cloud or the points in memory.
    reader: usize,
    points: u64,
    transform: SourceTransform,
}

impl SourceIdentity {
    fn of(source: &RegionSource<'_>) -> Self {
        let (reader, points) = match source.reader {
            RegionReader::Index(index) => {
                (std::ptr::from_ref(index) as usize, index.root.total_points)
            }
            RegionReader::Stream(cloud) => (std::ptr::from_ref(cloud) as usize, cloud.total_points),
            RegionReader::Resident(points) => (points.as_ptr() as usize, points.len() as u64),
        };
        Self {
            reader,
            points,
            transform: source.transform,
        }
    }
}

/// The part of the scene between two planes that the slab lies in: the
/// points whose position along `axis` lies between the two ends of `range`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Band {
    /// The direction the drawing looks, as a unit vector in the scene.
    axis: [f64; 3],
    range: [f64; 2],
}

impl Band {
    fn of(slab: &Slab) -> Self {
        let axis = match slab.view.depth_axis() {
            2 => [0.0, 0.0, 1.0],
            own => slab.region.axes()[own],
        };
        let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
        for corner in slab.region.corners() {
            let at = dot(axis, corner);
            low = low.min(at);
            high = high.max(at);
        }
        Self {
            axis,
            range: [low, high],
        }
    }

    fn holds(&self, xyz: [f64; 3]) -> bool {
        let at = dot(self.axis, xyz);
        at >= self.range[0] - BAND_MARGIN && at <= self.range[1] + BAND_MARGIN
    }

    /// The part of a box that lies in the band, measured along its axis;
    /// the whole box when it is flat along it.
    fn share_of(&self, bounds: Bounds) -> f64 {
        let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
        for corner in crate::bounds_corners(bounds) {
            let at = dot(self.axis, corner);
            low = low.min(at);
            high = high.max(at);
        }
        let inside = high.min(self.range[1]) - low.max(self.range[0]);
        if inside < 0.0 {
            0.0
        } else if high - low <= 0.0 {
            1.0
        } else {
            (inside / (high - low)).clamp(0.0, 1.0)
        }
    }

    /// The same band, but for what working it out again may round
    /// differently.
    fn same(&self, other: &Self) -> bool {
        (0..3).all(|axis| (self.axis[axis] - other.axis[axis]).abs() <= 1e-12)
            && (0..2).all(|end| (self.range[end] - other.range[end]).abs() <= 1e-7)
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// What the kept points were read for: the layers, the band and the share.
#[derive(Debug, Clone, PartialEq)]
struct KeptKey {
    sources: Vec<SourceIdentity>,
    band: Band,
    percent: f64,
}

impl KeptKey {
    fn of(sources: &[RegionSource<'_>], slab: &Slab, percent: f64) -> Self {
        Self {
            sources: sources.iter().map(SourceIdentity::of).collect(),
            band: Band::of(slab),
            percent,
        }
    }

    fn same(&self, other: &Self) -> bool {
        self.sources == other.sources
            && self.band.same(&other.band)
            && self.percent == other.percent
    }
}

/// A part of a layer whose points are kept together: a leaf of its index,
/// by its address, or the whole of a layer without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct UnitKey {
    source: usize,
    leaf: Option<usize>,
}

/// The points of the slab of one drawing that were read, for the next time
/// it is made. A drawing that keeps one of these and hands it to every make
/// reads its slab once, as long as the cut, the depth, the share of the
/// points and the layers stay.
///
/// It holds no more than its limit: a slab whose band holds more points than
/// that is read as a drawing without kept points reads it, and nothing is
/// kept.
#[derive(Debug)]
pub struct KeptSlab {
    key: Option<KeptKey>,
    units: HashMap<UnitKey, KeptUnit>,
    points: usize,
    limit: usize,
    /// From this many points on, the kept points are drawn on several
    /// threads.
    parallel_from: usize,
}

impl KeptSlab {
    /// Kept points of at most `limit` bytes.
    pub fn new(limit: usize) -> Self {
        Self {
            key: None,
            units: HashMap::new(),
            points: 0,
            limit,
            parallel_from: PARALLEL_FROM,
        }
    }

    /// The memory the kept points take, in bytes.
    pub fn bytes(&self) -> usize {
        self.points * KEPT_POINT_BYTES
    }

    /// How many points are kept.
    pub fn points(&self) -> usize {
        self.points
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Forget every kept point.
    pub fn clear(&mut self) {
        self.key = None;
        self.units.clear();
        self.points = 0;
    }

    fn start_over(&mut self, key: KeptKey) {
        self.clear();
        self.key = Some(key);
    }

    /// Keep only the units of `needed`.
    fn keep_only(&mut self, needed: &[Unit<'_>]) {
        let wanted: Vec<UnitKey> = needed.iter().filter_map(Unit::key).collect();
        self.units.retain(|key, _| wanted.contains(key));
        self.points = self.units.values().map(|unit| unit.points.len()).sum();
    }

    fn insert(&mut self, key: UnitKey, unit: KeptUnit) {
        self.points += unit.points.len();
        if let Some(earlier) = self.units.insert(key, unit) {
            self.points -= earlier.points.len();
        }
    }
}

/// A part of a layer that the slab touches.
#[derive(Clone, Copy)]
enum Unit<'a> {
    Leaf {
        source: usize,
        index: &'a OctreeIndex,
        leaf: &'a IndexedNode,
        transform: SourceTransform,
    },
    /// A layer without an index, read from its file in full.
    Whole {
        source: usize,
        cloud: &'a PointCloud,
        transform: SourceTransform,
    },
    /// A layer whose points are in memory already; they are not kept twice.
    Resident {
        source: usize,
        points: &'a [IndexedPoint],
        transform: SourceTransform,
    },
}

impl Unit<'_> {
    fn key(&self) -> Option<UnitKey> {
        match self {
            Self::Leaf { source, leaf, .. } => Some(UnitKey {
                source: *source,
                leaf: Some(std::ptr::from_ref(*leaf) as usize),
            }),
            Self::Whole { source, .. } => Some(UnitKey {
                source: *source,
                leaf: None,
            }),
            Self::Resident { .. } => None,
        }
    }

    fn source(&self) -> usize {
        match self {
            Self::Leaf { source, .. }
            | Self::Whole { source, .. }
            | Self::Resident { source, .. } => *source,
        }
    }

    /// Points a read of the unit goes through.
    fn stored(&self) -> u64 {
        match self {
            Self::Leaf { leaf, .. } => leaf.stored_points,
            Self::Whole { cloud, .. } => cloud.total_points,
            Self::Resident { points, .. } => points.len() as u64,
        }
    }

    /// Where its points lie in the scene.
    fn bounds(&self) -> Option<Bounds> {
        match self {
            Self::Leaf {
                leaf, transform, ..
            } => Some(transform.bounds(leaf.bounds)),
            Self::Whole {
                cloud, transform, ..
            } => Some(transform.bounds(cloud.bounds)),
            Self::Resident { .. } => None,
        }
    }

    /// The place its points are kept from: the middle of where they lie.
    fn anchor(&self) -> [f64; 3] {
        self.bounds().map_or([0.0; 3], |bounds| {
            std::array::from_fn(|axis| {
                let middle = (bounds.min[axis] + bounds.max[axis]) / 2.0;
                if middle.is_finite() {
                    middle
                } else {
                    0.0
                }
            })
        })
    }

    /// About how many of its points a read keeps: those in the band, as if
    /// its points were spread evenly over its box.
    fn estimate(&self, band: &Band, percent: f64) -> f64 {
        self.bounds().map_or(0.0, |bounds| {
            self.stored() as f64 * (percent / 100.0).min(1.0) * band.share_of(bounds)
        })
    }
}

/// The parts of the layers that touch the slab, in the order a read without
/// kept points takes them: layer by layer, the leaves of an index in tree
/// order.
fn units_of<'a>(sources: &[RegionSource<'a>], slab: &Slab) -> Vec<Unit<'a>> {
    let mut units = Vec::new();
    for (source, layer) in sources.iter().enumerate() {
        let transform = layer.transform;
        let reaches = layer
            .world_bounds()
            .is_some_and(|bounds| overlaps(bounds, slab.bounds));
        if !reaches {
            continue;
        }
        match layer.reader {
            RegionReader::Index(index) => units.extend(
                index
                    .intersecting_leaves(|node| overlaps(transform.bounds(node), slab.bounds))
                    .into_iter()
                    .map(|leaf| Unit::Leaf {
                        source,
                        index,
                        leaf,
                        transform,
                    }),
            ),
            RegionReader::Stream(cloud) => units.push(Unit::Whole {
                source,
                cloud,
                transform,
            }),
            RegionReader::Resident(points) => units.push(Unit::Resident {
                source,
                points,
                transform,
            }),
        }
    }
    units
}

/// The points of one leaf in the band and the share, and how many were
/// read; nothing when an ordinal is too large to keep. Stops at the next
/// few thousand points once `stop` is set.
fn read_leaf(
    unit: &Unit<'_>,
    band: Band,
    percent: f64,
    stop: &AtomicBool,
) -> Result<Option<(KeptUnit, u64)>, LoadError> {
    let Unit::Leaf {
        index,
        leaf,
        transform,
        ..
    } = *unit
    else {
        return Ok(None);
    };
    let anchor = unit.anchor();
    let mut points = Vec::new();
    let mut read = 0u64;
    let mut fits = true;
    index.visit_leaf(leaf, |record| {
        read += 1;
        if read.is_multiple_of(CHUNK_POINTS as u64) && stop.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        if !sampled_ordinal(record.ordinal, percent) {
            return Ok(());
        }
        let mut point = record.point;
        point.xyz = transform.xyz(point.xyz);
        if band.holds(point.xyz) {
            fits &= record.ordinal <= MAX_ORDINAL;
            points.push(KeptPoint::of(&point, record.ordinal, anchor));
        }
        Ok(())
    })?;
    Ok(fits.then_some((KeptUnit { anchor, points }, read)))
}

/// What reading the missing units gave: the points of each by its place in
/// the list of units, and the points read; nothing when they hold more than
/// `room` points.
type Read = Option<(Vec<(usize, KeptUnit)>, u64)>;

/// Read the units at the places `missing` of `units`: the leaves on several
/// threads, a layer without an index on this one. `progress` hears the
/// points read so far, on this thread.
fn read_missing(
    units: &[Unit<'_>],
    missing: &[usize],
    band: Band,
    percent: f64,
    room: usize,
    progress: &mut dyn FnMut(u64) -> Result<(), LoadError>,
) -> Result<Read, LoadError> {
    let leaves: Vec<usize> = missing
        .iter()
        .copied()
        .filter(|place| matches!(units[*place], Unit::Leaf { .. }))
        .collect();
    let mut fresh: Vec<(usize, KeptUnit)> = Vec::with_capacity(missing.len());
    let mut read = 0u64;
    let mut held = 0usize;
    if !leaves.is_empty() {
        let threads = threads_for(leaves.len());
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let mut failure: Option<LoadError> = None;
        let mut over = false;
        std::thread::scope(|scope| {
            let (sender, receiver) = mpsc::channel();
            for _ in 0..threads {
                let sender = sender.clone();
                let (next, stop, leaves) = (&next, &stop, &leaves);
                scope.spawn(move || loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(&place) = leaves.get(next.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    let result = read_leaf(&units[place], band, percent, stop);
                    if sender.send((place, result)).is_err() {
                        break;
                    }
                });
            }
            drop(sender);
            // The answers come as the threads finish their leaves; this
            // thread keeps them and reports.
            for (place, result) in receiver {
                if failure.is_some() || over {
                    continue;
                }
                match result {
                    Ok(Some((unit, count))) => {
                        read += count;
                        held += unit.points.len();
                        fresh.push((place, unit));
                        if held > room {
                            over = true;
                            stop.store(true, Ordering::Relaxed);
                        } else if let Err(error) = progress(read) {
                            failure = Some(error);
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                    Ok(None) => {
                        over = true;
                        stop.store(true, Ordering::Relaxed);
                    }
                    Err(error) => {
                        failure = Some(error);
                        stop.store(true, Ordering::Relaxed);
                    }
                }
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        if over {
            return Ok(None);
        }
    }
    for &place in missing {
        let unit = units[place];
        let Unit::Whole {
            cloud, transform, ..
        } = unit
        else {
            continue;
        };
        let anchor = unit.anchor();
        let layer = RegionSource {
            reader: RegionReader::Stream(cloud),
            transform,
        };
        let mut points = Vec::new();
        let before = read;
        let mut over = false;
        let stats = visit_region(
            &[layer],
            EVERYWHERE,
            &|_, ordinal, point| sampled_ordinal(ordinal, percent) && band.holds(point.xyz),
            &mut |step| progress(before + step.read),
            &mut |_, batch| {
                points.extend(
                    batch
                        .iter()
                        .map(|record| KeptPoint::of(&record.point, record.ordinal, anchor)),
                );
                let too_late = batch.iter().any(|record| record.ordinal > MAX_ORDINAL);
                if too_late || held + points.len() > room {
                    over = true;
                    return Err(LoadError::Cancelled);
                }
                Ok(())
            },
        );
        match stats {
            Ok(stats) => read = before + stats.read,
            Err(LoadError::Cancelled) if over => return Ok(None),
            Err(error) => return Err(error),
        }
        held += points.len();
        fresh.push((place, KeptUnit { anchor, points }));
    }
    Ok(Some((fresh, read)))
}

/// How many threads take `jobs` jobs.
fn threads_for(jobs: usize) -> usize {
    std::thread::available_parallelism()
        .map_or(1, |count| count.get())
        .min(MAX_THREADS)
        .min(jobs)
        .max(1)
}

/// A unit as it is drawn: its kept points, or the points in memory.
#[derive(Clone, Copy)]
enum Drawn<'a> {
    Kept(usize, &'a KeptUnit),
    Resident(usize, &'a [IndexedPoint], SourceTransform),
}

impl Drawn<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Kept(_, unit) => unit.points.len(),
            Self::Resident(_, points, _) => points.len(),
        }
    }
}

/// What every unit that is drawn needs: the slab, the share and the filter.
struct Drawing<'s, 'a> {
    slab: &'s Slab,
    percent: f64,
    accept: &'s RegionFilter<'a>,
}

impl Drawing<'_, '_> {
    fn in_slab(&self, xyz: [f64; 3]) -> bool {
        contains(self.slab.bounds, xyz)
            && (!self.slab.region.is_turned() || self.slab.region.contains(xyz))
    }

    /// Hand the points of a unit that take part to `take`, a chunk at a
    /// time, telling `tick` how many were gone through after each chunk.
    /// Answers how many took part.
    fn draw(
        &self,
        unit: Drawn<'_>,
        take: &mut dyn FnMut([f64; 2], u32, &IndexedPoint),
        tick: &mut dyn FnMut(usize) -> Result<(), LoadError>,
    ) -> Result<u64, LoadError> {
        let mut accepted = 0u64;
        match unit {
            Drawn::Kept(source, kept) => {
                for chunk in kept.points.chunks(CHUNK_POINTS) {
                    for point in chunk {
                        let xyz = point.xyz(kept.anchor);
                        if !self.in_slab(xyz) {
                            continue;
                        }
                        let record = point.record(kept.anchor);
                        if (self.accept)(source, record.ordinal, &record.point) {
                            take(self.slab.frame.to_uv(xyz), source as u32, &record);
                            accepted += 1;
                        }
                    }
                    tick(chunk.len())?;
                }
            }
            Drawn::Resident(source, points, transform) => {
                for chunk in points.chunks(CHUNK_POINTS) {
                    for record in chunk {
                        if !sampled_ordinal(record.ordinal, self.percent) {
                            continue;
                        }
                        let mut point = record.point;
                        point.xyz = transform.xyz(point.xyz);
                        if self.in_slab(point.xyz) && (self.accept)(source, record.ordinal, &point)
                        {
                            let record = IndexedPoint {
                                point,
                                ordinal: record.ordinal,
                            };
                            take(self.slab.frame.to_uv(point.xyz), source as u32, &record);
                            accepted += 1;
                        }
                    }
                    tick(chunk.len())?;
                }
            }
        }
        Ok(accepted)
    }
}

/// The units split in `parts` runs of about as many points each, in order.
fn split(units: &[Drawn<'_>], parts: usize) -> Vec<Range<usize>> {
    let total: usize = units.iter().map(Drawn::len).sum();
    let share = total.div_ceil(parts.max(1)).max(1);
    let mut runs = Vec::new();
    let (mut start, mut filled) = (0, 0);
    for (place, unit) in units.iter().enumerate() {
        filled += unit.len();
        if filled >= share {
            runs.push(start..place + 1);
            start = place + 1;
            filled = 0;
        }
    }
    if start < units.len() {
        runs.push(start..units.len());
    }
    runs
}

/// What a thread that draws kept points tells the thread that waits.
enum Told {
    Went(usize),
    Done(usize, Result<(Gathered, u64), LoadError>),
}

/// The first pass over the kept points on several threads, each into a copy
/// of `start`, merged in the order of the units. Answers what was gathered
/// and how many points took part.
fn draw_in_parts(
    drawing: &Drawing<'_, '_>,
    units: &[Drawn<'_>],
    start: &Gathered,
    parts: usize,
    progress: &mut dyn FnMut(u64) -> Result<(), LoadError>,
) -> Result<(Gathered, u64), LoadError> {
    let runs = split(units, parts);
    let stop = AtomicBool::new(false);
    let mut failure: Option<LoadError> = None;
    let mut done: Vec<Option<(Gathered, u64)>> = (0..runs.len()).map(|_| None).collect();
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        for (place, run) in runs.iter().enumerate() {
            let sender = sender.clone();
            let stop = &stop;
            let run = run.clone();
            scope.spawn(move || {
                let mut gathered = start.clone();
                let mut accepted = 0u64;
                let mut result = Ok(());
                for unit in &units[run] {
                    let drawn = drawing.draw(
                        *unit,
                        &mut |uv, source, record| gathered.add(uv, source, record),
                        &mut |count| {
                            if stop.load(Ordering::Relaxed) {
                                return Err(LoadError::Cancelled);
                            }
                            let _ = sender.send(Told::Went(count));
                            Ok(())
                        },
                    );
                    match drawn {
                        Ok(count) => accepted += count,
                        Err(error) => {
                            result = Err(error);
                            break;
                        }
                    }
                }
                let _ = sender.send(Told::Done(place, result.map(|()| (gathered, accepted))));
            });
        }
        drop(sender);
        let mut went = 0u64;
        let mut since = 0usize;
        for told in receiver {
            match told {
                Told::Went(count) => {
                    went += count as u64;
                    since += count;
                    if since >= REPORT_POINTS && failure.is_none() {
                        since = 0;
                        if let Err(error) = progress(went) {
                            failure = Some(error);
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                }
                Told::Done(place, Ok(gathered)) => done[place] = Some(gathered),
                Told::Done(_, Err(error)) => {
                    failure.get_or_insert(error);
                    stop.store(true, Ordering::Relaxed);
                }
            }
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    let mut merged = start.clone();
    let mut accepted = 0u64;
    for (gathered, count) in done.into_iter().flatten() {
        merged.merge(gathered);
        accepted += count;
    }
    Ok((merged, accepted))
}

/// As `collect_slab`, with the points of the slab kept in `kept` for the
/// next time the same slab, or another crop region of it, is collected.
///
/// The points that `kept` holds from an earlier call with the same layers,
/// band and share are taken from memory; only the leaves that the slab
/// touches and that are not kept yet are read, on several threads. A large
/// number of kept points is drawn on several threads as well. The result is
/// the one `collect_slab` gives, to the rounding of the kept positions, with
/// `read_points` the points read now and `reused_points` the points taken
/// from memory.
///
/// When the band holds more points than the limit of `kept`, the slab is
/// read as `collect_slab` reads it and `kept` is emptied. `progress` hears
/// the stage `Reading` while leaves are read and `Thinning` while the kept
/// points are drawn.
pub fn collect_slab_kept(
    sources: &[RegionSource<'_>],
    slab: &Slab,
    options: &SlabOptions,
    accept: &RegionFilter<'_>,
    kept: &mut KeptSlab,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<SlabCut, LoadError> {
    check_options(options)?;
    let percent = options.read_percent();
    let key = KeptKey::of(sources, slab, percent);
    if !kept.key.as_ref().is_some_and(|known| known.same(&key)) {
        kept.start_over(key);
    }
    let band = Band::of(slab);
    let units = units_of(sources, slab);
    let room = kept.limit / KEPT_POINT_BYTES;
    let missing: Vec<usize> = (0..units.len())
        .filter(|place| {
            units[*place]
                .key()
                .is_some_and(|key| !kept.units.contains_key(&key))
        })
        .collect();
    let reused: usize = units
        .iter()
        .filter_map(Unit::key)
        .filter_map(|key| kept.units.get(&key))
        .map(|unit| unit.points.len())
        .sum();
    let expected = missing
        .iter()
        .map(|place| units[*place].estimate(&band, percent))
        .sum::<f64>()
        .ceil() as usize;
    // Heard from several places below, one at a time.
    let progress = RefCell::new(progress);
    let report = |stage: DrawingStage, done: u64, total: u64| {
        (*progress.borrow_mut())(DrawingProgress { stage, done, total })
    };
    let mut reading = |step: RegionProgress| report(DrawingStage::Reading, step.read, step.total);
    if reused + expected > room {
        kept.clear();
        return collect_slab(sources, slab, options, accept, &mut reading);
    }
    if kept.points + expected > room {
        kept.keep_only(&units);
    }
    let to_read: u64 = missing.iter().map(|place| units[*place].stored()).sum();
    reading(RegionProgress {
        read: 0,
        total: to_read,
        accepted: 0,
    })?;
    let fresh = read_missing(
        &units,
        &missing,
        band,
        percent,
        room.saturating_sub(kept.points),
        &mut |read| {
            reading(RegionProgress {
                read,
                total: to_read,
                accepted: 0,
            })
        },
    )?;
    let Some((fresh, read_now)) = fresh else {
        kept.clear();
        return collect_slab(sources, slab, options, accept, &mut reading);
    };
    for (place, unit) in fresh {
        if let Some(key) = units[place].key() {
            kept.insert(key, unit);
        }
    }

    // Draw the slab from memory.
    let (_, covered) = slab_work(sources, slab);
    let store = &*kept;
    let drawn: Vec<Drawn<'_>> = units
        .iter()
        .filter_map(|unit| match *unit {
            Unit::Resident {
                source,
                points,
                transform,
            } => Some(Drawn::Resident(source, points, transform)),
            _ => {
                let kept = store.units.get(&unit.key()?)?;
                Some(Drawn::Kept(unit.source(), kept))
            }
        })
        .collect();
    let total: u64 = drawn.iter().map(|unit| unit.len() as u64).sum();
    let resident: u64 = drawn
        .iter()
        .filter(|unit| matches!(unit, Drawn::Resident(..)))
        .map(|unit| unit.len() as u64)
        .sum();
    let drawing = Drawing {
        slab,
        percent,
        accept,
    };
    let thinning = |done: u64| report(DrawingStage::Thinning, done, total);
    thinning(0)?;
    // A pass on this thread, for a slab with few points and for the passes
    // that lay the grid closer.
    let mut pass = |take: &mut dyn FnMut([f64; 2], u32, &IndexedPoint),
                    _again: Option<RegionProgress>| {
        let mut went = 0u64;
        let mut since = 0usize;
        let mut accepted = 0u64;
        for unit in &drawn {
            accepted += drawing.draw(*unit, take, &mut |count| {
                went += count as u64;
                since += count;
                if since >= REPORT_POINTS {
                    since = 0;
                    thinning(went)?;
                }
                Ok(())
            })?;
        }
        Ok(RegionProgress {
            read: resident,
            total,
            accepted,
        })
    };
    let start = Gathered::start(slab, options, covered)?;
    let parts = if total as usize >= store.parallel_from {
        let copies = MAX_GRID_COPIES / start.grid_bytes().max(1);
        threads_for(drawn.len()).min(copies.max(1))
    } else {
        1
    };
    let mut cut = if parts > 1 {
        let (gathered, accepted) =
            draw_in_parts(&drawing, &drawn, &start, parts, &mut |went| thinning(went))?;
        let state = RegionProgress {
            read: resident,
            total,
            accepted,
        };
        finish_passes(options, gathered, state, &mut pass)?
    } else {
        collect_passes(slab, options, covered, &mut pass)?
    };
    cut.read_points += read_now;
    cut.reused_points = reused as u64;
    Ok(cut)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::drawing::slab::slab_from_section;
    use crate::drawing::{DrawingOrigin, DrawingView};
    use crate::test_shapes::{box_room, indexed_cloud, RoomSpec};
    use crate::OrientedBox;

    fn room_points() -> Vec<Point> {
        let mut points = box_room(&RoomSpec::default()).cloud_points();
        for (ordinal, point) in points.iter_mut().enumerate() {
            point.classification = Some((ordinal % 3) as u8 + 1);
        }
        points
    }

    fn options(percent: f64) -> SlabOptions {
        SlabOptions {
            point_spacing: Some(0.005),
            max_points: 150_000,
            grid: Some(0.02),
            sample_percent: percent,
            grid_percent: percent,
        }
    }

    fn plan(min: [f64; 3], max: [f64; 3], degrees: f64) -> Slab {
        slab_from_section(
            OrientedBox::new(Bounds { min, max }, degrees),
            DrawingView::Plan,
            Some(0.1),
            DrawingOrigin::Model,
        )
        .unwrap()
    }

    fn every(_: usize, _: u64, _: &Point) -> bool {
        true
    }

    fn streamed(
        sources: &[RegionSource<'_>],
        slab: &Slab,
        options: &SlabOptions,
        accept: &RegionFilter<'_>,
    ) -> SlabCut {
        collect_slab(sources, slab, options, accept, &mut |_| Ok(())).unwrap()
    }

    fn from_kept(
        sources: &[RegionSource<'_>],
        slab: &Slab,
        options: &SlabOptions,
        accept: &RegionFilter<'_>,
        kept: &mut KeptSlab,
    ) -> SlabCut {
        collect_slab_kept(sources, slab, options, accept, kept, &mut |_| Ok(())).unwrap()
    }

    /// The same slab as a first read with an empty `KeptSlab` gives.
    fn fresh(
        sources: &[RegionSource<'_>],
        slab: &Slab,
        options: &SlabOptions,
        accept: &RegionFilter<'_>,
    ) -> SlabCut {
        from_kept(sources, slab, options, accept, &mut KeptSlab::new(64 << 20))
    }

    /// The same drawing: the same points and counts, and the same means of
    /// the cells but for the order their sums were added in. The counts of
    /// what was read may differ.
    fn same_cut(a: &SlabCut, b: &SlabCut) {
        assert_eq!(a.points, b.points);
        assert_eq!(a.spacing, b.spacing);
        assert_eq!(a.slab_points, b.slab_points);
        assert_eq!(a.grid.is_some(), b.grid.is_some());
        if let (Some(g), Some(h)) = (&a.grid, &b.grid) {
            assert_eq!(g.frame(), h.frame());
            assert_eq!(g.counts(), h.counts());
            let near = |x: f64, y: f64| (x - y).abs() < 1e-9;
            let frame = g.frame();
            for y in 0..frame.height {
                for x in 0..frame.width {
                    match (g.centroid(x, y), h.centroid(x, y)) {
                        (Some(c), Some(d)) => assert!(near(c[0], d[0]) && near(c[1], d[1])),
                        (c, d) => assert_eq!(c, d),
                    }
                }
            }
        }
    }

    /// The drawing a read without kept points gives, to the rounding of the
    /// kept positions to a fraction of a micrometre: the same points of the
    /// slab, and the same number drawn but for the few that the rounding
    /// moves over the edge of a cell where the points lie on a lattice.
    fn like_streamed(kept: &SlabCut, read: &SlabCut) {
        assert_eq!(kept.slab_points, read.slab_points);
        assert_eq!(kept.spacing, read.spacing);
        let (a, b) = (kept.points.len(), read.points.len());
        assert!(a.abs_diff(b) <= 2 + b / 50, "{a} and {b} points");
        let total = |cut: &SlabCut| {
            cut.grid.as_ref().map(|grid| {
                grid.counts()
                    .counts()
                    .iter()
                    .map(|count| u64::from(*count))
                    .sum::<u64>()
            })
        };
        assert_eq!(total(kept), total(read));
    }

    fn indexed(cloud: &crate::test_shapes::IndexedCloud) -> [RegionSource<'_>; 1] {
        [RegionSource::new(
            &cloud.cloud,
            Some(&cloud.index),
            SourceTransform::default(),
        )]
    }

    #[test]
    fn a_crop_region_within_what_was_read_is_drawn_without_reading() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 2_048);
        let sources = indexed(&cloud);
        let mut kept = KeptSlab::new(64 << 20);
        let whole = plan([-1.0, -1.0, 0.0], [5.0, 4.0, 1.1], 0.0);
        let first = from_kept(&sources, &whole, &options(100.0), &every, &mut kept);
        like_streamed(&first, &streamed(&sources, &whole, &options(100.0), &every));
        assert!(first.read_points > 0 && first.reused_points == 0);
        // What is kept is the band of the leaves read: the slab and more.
        assert!(kept.points() as u64 >= first.slab_points);
        assert_eq!(kept.bytes(), kept.points() * KEPT_POINT_BYTES);
        assert_eq!(KEPT_POINT_BYTES, 24);

        // Smaller, moved, and turned within the leaves read: nothing is read,
        // and the drawing is the one a read gives.
        for slab in [
            plan([0.5, 0.5, 0.0], [3.0, 2.5, 1.1], 0.0),
            plan([1.5, -0.5, 0.0], [4.5, 2.0, 1.1], 0.0),
            plan([0.5, 0.5, 0.0], [3.5, 2.5, 1.1], 25.0),
        ] {
            let cut = from_kept(&sources, &slab, &options(100.0), &every, &mut kept);
            assert_eq!(cut.read_points, 0);
            assert!(cut.reused_points > 0);
            same_cut(&cut, &fresh(&sources, &slab, &options(100.0), &every));
        }

        // Deleted points and hidden classes are left out each time: no read.
        let filter = |_: usize, ordinal: u64, point: &Point| {
            ordinal.is_multiple_of(2) && point.classification != Some(1)
        };
        let slab = plan([0.5, 0.5, 0.0], [3.0, 2.5, 1.1], 0.0);
        let cut = from_kept(&sources, &slab, &options(100.0), &filter, &mut kept);
        assert_eq!(cut.read_points, 0);
        same_cut(&cut, &fresh(&sources, &slab, &options(100.0), &filter));

        // Another cut height is another band: read again, kept anew.
        let lower = plan([-1.0, -1.0, 0.0], [5.0, 4.0, 0.9], 0.0);
        let cut = from_kept(&sources, &lower, &options(100.0), &every, &mut kept);
        assert!(cut.read_points > 0 && cut.reused_points == 0);
        same_cut(&cut, &fresh(&sources, &lower, &options(100.0), &every));
        // So is another share of the points.
        let cut = from_kept(&sources, &lower, &options(50.0), &every, &mut kept);
        assert!(cut.read_points > 0 && cut.reused_points == 0);
        same_cut(&cut, &fresh(&sources, &lower, &options(50.0), &every));
        // And a layer that moved.
        let moved = [RegionSource::new(
            &cloud.cloud,
            Some(&cloud.index),
            SourceTransform {
                scale: [1.0; 3],
                offset: [0.5, 0.0, 0.0],
            },
        )];
        let cut = from_kept(&moved, &lower, &options(50.0), &every, &mut kept);
        assert!(cut.read_points > 0 && cut.reused_points == 0);
        same_cut(&cut, &fresh(&moved, &lower, &options(50.0), &every));
    }

    #[test]
    fn a_crop_region_that_grows_reads_only_the_leaves_it_adds() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 512);
        let sources = indexed(&cloud);
        let mut kept = KeptSlab::new(64 << 20);
        let west = plan([-0.5, -0.5, 0.0], [1.2, 3.5, 1.1], 0.0);
        let first = from_kept(&sources, &west, &options(100.0), &every, &mut kept);
        let whole = plan([-0.5, -0.5, 0.0], [4.5, 3.5, 1.1], 0.0);
        let grown = from_kept(&sources, &whole, &options(100.0), &every, &mut kept);
        let all = streamed(&sources, &whole, &options(100.0), &every);
        same_cut(&grown, &fresh(&sources, &whole, &options(100.0), &every));
        assert!(grown.read_points > 0 && grown.reused_points > 0);
        // The leaves of the first read are not read again.
        assert!(
            grown.read_points < all.read_points,
            "{} then {} of {}",
            first.read_points,
            grown.read_points,
            all.read_points
        );
        // Back to the first region: from memory.
        let again = from_kept(&sources, &west, &options(100.0), &every, &mut kept);
        assert_eq!(again.read_points, 0);
        same_cut(&again, &first);
    }

    #[test]
    fn a_share_of_the_points_keeps_the_same_points_whatever_the_crop() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 2_048);
        let sources = indexed(&cloud);
        let seen = Mutex::new(Vec::new());
        let record = |_: usize, ordinal: u64, _: &Point| {
            seen.lock().unwrap().push(ordinal);
            true
        };
        let whole = plan([-1.0, -1.0, 0.0], [5.0, 4.0, 1.1], 0.0);
        let part = plan([0.5, 0.5, 0.0], [3.0, 2.5, 1.1], 0.0);
        let all = streamed(&sources, &whole, &options(100.0), &every);
        let tenth = streamed(&sources, &whole, &options(10.0), &record);
        let mut large: Vec<u64> = std::mem::take(&mut *seen.lock().unwrap());
        // About a tenth of the points.
        let share = tenth.slab_points as f64 / all.slab_points as f64;
        assert!((0.07..0.13).contains(&share), "{share}");
        let mut kept = KeptSlab::new(64 << 20);
        let from_memory = from_kept(&sources, &whole, &options(10.0), &every, &mut kept);
        like_streamed(&from_memory, &tenth);
        let small = from_kept(&sources, &part, &options(10.0), &record, &mut kept);
        assert_eq!(small.read_points, 0);
        same_cut(&small, &fresh(&sources, &part, &options(10.0), &every));
        // The points of the smaller crop are those of the larger one that
        // lie in it.
        let mut smaller = std::mem::take(&mut *seen.lock().unwrap());
        large.sort_unstable();
        smaller.sort_unstable();
        let inside: Vec<u64> = large
            .iter()
            .copied()
            .filter(|ordinal| contains(part.bounds, points[*ordinal as usize].xyz))
            .collect();
        assert_eq!(smaller, inside);

        // A filled cut from every point and a tenth of them drawn: the grid
        // of every point, the points of a tenth, and kept is every point,
        // so that another share of the points drawn reads nothing.
        let mixed = SlabOptions {
            grid_percent: 100.0,
            ..options(10.0)
        };
        assert_eq!(mixed.read_percent(), 100.0);
        let mut both = KeptSlab::new(64 << 20);
        let cut = from_kept(&sources, &whole, &mixed, &every, &mut both);
        let every_point = fresh(&sources, &whole, &options(100.0), &every);
        assert_eq!(cut.grid, every_point.grid);
        assert_eq!(
            cut.points,
            fresh(&sources, &whole, &options(10.0), &every).points
        );
        assert_eq!(cut.slab_points, all.slab_points);
        like_streamed(&cut, &streamed(&sources, &whole, &mixed, &every));
        let more = SlabOptions {
            sample_percent: 50.0,
            ..mixed
        };
        let half = from_kept(&sources, &whole, &more, &every, &mut both);
        assert_eq!(half.read_points, 0);
        assert!(half.points.len() > cut.points.len());
        // A share outside its limits is refused.
        for percent in [0.0, -1.0, 100.5, f64::NAN] {
            assert!(collect_slab_kept(
                &sources,
                &part,
                &options(percent),
                &every,
                &mut kept,
                &mut |_| Ok(())
            )
            .is_err());
        }
    }

    #[test]
    fn a_slab_that_does_not_fit_is_read_as_before_and_nothing_is_kept() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 2_048);
        let in_memory: Vec<IndexedPoint> = points
            .iter()
            .enumerate()
            .map(|(ordinal, point)| IndexedPoint {
                point: *point,
                ordinal: ordinal as u64,
            })
            .collect();
        let whole = plan([-1.0, -1.0, 0.0], [5.0, 4.0, 1.1], 0.0);
        let part = plan([0.5, 0.5, 0.0], [3.0, 2.5, 1.1], 0.0);
        for index in [Some(&cloud.index), None] {
            let sources = [RegionSource::new(
                &cloud.cloud,
                index,
                SourceTransform::default(),
            )];
            let truth = streamed(&sources, &whole, &options(100.0), &every);
            let mut small = KeptSlab::new(100 * KEPT_POINT_BYTES);
            let cut = from_kept(&sources, &whole, &options(100.0), &every, &mut small);
            // Read as before: the very drawing of a read.
            assert_eq!(cut, truth);
            assert_eq!(small.points(), 0);
            // With room, a layer without an index is kept as a whole.
            let mut room = KeptSlab::new(64 << 20);
            let first = from_kept(&sources, &whole, &options(100.0), &every, &mut room);
            like_streamed(&first, &truth);
            let again = from_kept(&sources, &part, &options(100.0), &every, &mut room);
            assert_eq!(again.read_points, 0);
            same_cut(&again, &fresh(&sources, &part, &options(100.0), &every));
        }
        // Points in memory are drawn from where they are, never kept twice.
        let sources = [RegionSource::resident(
            &in_memory,
            SourceTransform::default(),
        )];
        let mut kept = KeptSlab::new(64 << 20);
        let cut = from_kept(&sources, &whole, &options(100.0), &every, &mut kept);
        let read = streamed(&sources, &whole, &options(100.0), &every);
        assert_eq!((&cut.points, &cut.grid), (&read.points, &read.grid));
        assert_eq!(kept.points(), 0);
    }

    #[test]
    fn many_kept_points_are_drawn_on_several_threads_as_on_one() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 512);
        let sources = indexed(&cloud);
        let filter = |_: usize, ordinal: u64, _: &Point| !ordinal.is_multiple_of(5);
        let mut one = KeptSlab::new(64 << 20);
        let mut several = KeptSlab::new(64 << 20);
        several.parallel_from = 1;
        for (slab, max_points) in [
            (plan([-1.0, -1.0, 0.0], [5.0, 4.0, 1.1], 0.0), 150_000),
            (plan([0.5, -0.5, 0.0], [3.5, 3.5, 1.1], 15.0), 150_000),
            // So few points may be drawn that the spacing doubles: on
            // several threads it doubles as often.
            (plan([-1.0, -1.0, 0.0], [5.0, 4.0, 1.1], 0.0), 300),
        ] {
            let options = SlabOptions {
                max_points,
                ..options(100.0)
            };
            let alone = from_kept(&sources, &slab, &options, &filter, &mut one);
            let split = from_kept(&sources, &slab, &options, &filter, &mut several);
            same_cut(&split, &alone);
            like_streamed(&split, &streamed(&sources, &slab, &options, &filter));
        }
        // A cancel stops the threads.
        let slab = plan([-1.0, -1.0, 0.0], [5.0, 4.0, 1.1], 0.0);
        let stopped = collect_slab_kept(
            &sources,
            &slab,
            &options(100.0),
            &every,
            &mut several,
            &mut |step| {
                if step.stage == DrawingStage::Thinning {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
    }

    #[test]
    fn a_vertical_section_keeps_its_band_and_a_cancel_stops_the_read() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 512);
        let sources = indexed(&cloud);
        let front = |min: [f64; 3], max: [f64; 3]| {
            slab_from_section(
                Bounds { min, max },
                DrawingView::Front,
                Some(0.1),
                DrawingOrigin::Model,
            )
            .unwrap()
        };
        let mut kept = KeptSlab::new(64 << 20);
        let whole = front([-0.5, -0.045, -0.5], [4.5, 3.5, 3.0]);
        let first = from_kept(&sources, &whole, &options(100.0), &every, &mut kept);
        like_streamed(&first, &streamed(&sources, &whole, &options(100.0), &every));
        let part = front([1.0, -0.045, 0.5], [3.0, 3.5, 2.0]);
        let cut = from_kept(&sources, &part, &options(100.0), &every, &mut kept);
        assert_eq!(cut.read_points, 0);
        same_cut(&cut, &fresh(&sources, &part, &options(100.0), &every));

        // A cancel while leaves are read stops the read; the slab can be
        // drawn after it all the same.
        let deeper = front([-0.5, 2.905, -0.5], [4.5, 3.5, 3.0]);
        let stopped = collect_slab_kept(
            &sources,
            &deeper,
            &options(100.0),
            &every,
            &mut kept,
            &mut |step| {
                if step.stage == DrawingStage::Reading && step.done > 0 {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            },
        );
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
        let cut = from_kept(&sources, &deeper, &options(100.0), &every, &mut kept);
        same_cut(&cut, &fresh(&sources, &deeper, &options(100.0), &every));
    }
}
