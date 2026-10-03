//! The points of a box-shaped region, for a job that may run long: read
//! from the leaves of an octree index that touch the region or, for a layer
//! without an index, from one pass over its source file. Every point goes
//! through the layer's scale and offset and through the caller's filter, and
//! reaches the caller's visitor in batches, so memory does not grow with the
//! region. Progress is reported while reading, and an error from any
//! callback, such as `LoadError::Cancelled`, stops the read.
//!
//! A room is often spread over several scan files, so one call takes several
//! sources; each batch names the source its points come from.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use rayon::prelude::*;

use crate::{
    visit_points, Bounds, IndexedNode, IndexedPoint, LoadError, OctreeIndex, Point, PointCloud,
};

/// The most points a visitor receives at a time, and the most that are read
/// between two progress reports.
pub const BATCH_POINTS: usize = 4_096;

/// A region that holds every point.
pub const EVERYWHERE: Bounds = Bounds {
    min: [f64::NEG_INFINITY; 3],
    max: [f64::INFINITY; 3],
};

// Leaves are read on several threads, and one index serves several regions
// at a time: both need an index that can be shared.
const _: fn() = || {
    fn shared<T: Sync>() {}
    shared::<OctreeIndex>();
    shared::<PointCloud>();
};

/// The filter of a read: it gets the position of a source in the list of
/// sources, the ordinal of a point in its source file and the point in scene
/// coordinates, and says whether the point takes part.
pub type RegionFilter<'a> = dyn Fn(usize, u64, &Point) -> bool + Sync + 'a;

/// Receives the accepted points of the source at a position, a batch at a
/// time: each point in scene coordinates with its source ordinal.
pub type RegionVisitor<'a> = dyn FnMut(usize, &[IndexedPoint]) -> Result<(), LoadError> + 'a;

/// Adds a batch, as a `RegionVisitor` receives it, to the accumulator of one
/// reading thread.
pub type RegionFold<'a, T> =
    dyn Fn(&mut T, usize, &[IndexedPoint]) -> Result<(), LoadError> + Sync + 'a;

/// Whether two boxes share a point; touching counts.
pub fn overlaps(a: Bounds, b: Bounds) -> bool {
    (0..3).all(|axis| a.max[axis] >= b.min[axis] && a.min[axis] <= b.max[axis])
}

/// Whether a position lies in a box; its faces count.
pub fn contains(bounds: Bounds, xyz: [f64; 3]) -> bool {
    (0..3).all(|axis| xyz[axis] >= bounds.min[axis] && xyz[axis] <= bounds.max[axis])
}

/// Where a layer stands in the scene: source coordinates are scaled per axis
/// and then moved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceTransform {
    pub scale: [f64; 3],
    pub offset: [f64; 3],
}

impl Default for SourceTransform {
    fn default() -> Self {
        Self {
            scale: [1.0; 3],
            offset: [0.0; 3],
        }
    }
}

impl SourceTransform {
    /// Scene position of a source position.
    pub fn xyz(self, source: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| source[axis] * self.scale[axis] + self.offset[axis])
    }

    /// Source position of a scene position, unless a scale is zero.
    pub fn source_xyz(self, world: [f64; 3]) -> Option<[f64; 3]> {
        self.scale
            .iter()
            .all(|value| value.is_finite() && value.abs() > f64::EPSILON)
            .then(|| {
                std::array::from_fn(|axis| (world[axis] - self.offset[axis]) / self.scale[axis])
            })
    }

    /// Scene box of a source box. A negative scale swaps its two sides.
    pub fn bounds(self, source: Bounds) -> Bounds {
        let a = self.xyz(source.min);
        let b = self.xyz(source.max);
        Bounds {
            min: std::array::from_fn(|axis| a[axis].min(b[axis])),
            max: std::array::from_fn(|axis| a[axis].max(b[axis])),
        }
    }
}

/// Where the points of a source are read from.
#[derive(Debug, Clone, Copy)]
pub enum RegionReader<'a> {
    /// The leaves of an octree index that touch the region. Only those are
    /// read. Whether the index still matches its source file is for the
    /// caller to check.
    Index(&'a OctreeIndex),
    /// One pass over the whole source file. The file must be the revision
    /// that was loaded, before and after the pass.
    Stream(&'a PointCloud),
    /// Points that are in memory, in source coordinates with their ordinals:
    /// a small cloud read once with `resident_points` and asked for many
    /// regions. The layer transform is applied on every read, so the points
    /// that a read delivers, which are in the scene already, are not what
    /// this takes.
    Resident(&'a [IndexedPoint]),
}

/// One layer to take points from.
#[derive(Debug, Clone, Copy)]
pub struct RegionSource<'a> {
    pub reader: RegionReader<'a>,
    pub transform: SourceTransform,
}

impl<'a> RegionSource<'a> {
    /// A layer read from its index when it has one and from its source file
    /// otherwise.
    pub fn new(
        cloud: &'a PointCloud,
        index: Option<&'a OctreeIndex>,
        transform: SourceTransform,
    ) -> Self {
        Self {
            reader: match index {
                Some(index) => RegionReader::Index(index),
                None => RegionReader::Stream(cloud),
            },
            transform,
        }
    }

    /// Points in memory as a layer: the points in source coordinates, as
    /// `resident_points` returns them, and the transform of the layer.
    /// Points collected from `visit_region` have that transform in them and
    /// would be moved twice.
    pub fn resident(points: &'a [IndexedPoint], transform: SourceTransform) -> Self {
        Self {
            reader: RegionReader::Resident(points),
            transform,
        }
    }

    /// The box around all points of this layer in the scene; nothing for a
    /// layer without points.
    pub fn world_bounds(&self) -> Option<Bounds> {
        let source = match self.reader {
            RegionReader::Index(index) => index.root.bounds,
            RegionReader::Stream(cloud) => cloud.bounds,
            RegionReader::Resident(points) => {
                let mut bounds = Bounds {
                    min: points.first()?.point.xyz,
                    max: points.first()?.point.xyz,
                };
                for record in points {
                    bounds.include(record.point.xyz);
                }
                bounds
            }
        };
        Some(self.transform.bounds(source))
    }
}

/// The box around all points of several layers in the scene.
pub fn world_bounds(sources: &[RegionSource<'_>]) -> Option<Bounds> {
    let mut all: Option<Bounds> = None;
    for bounds in sources.iter().filter_map(RegionSource::world_bounds) {
        match &mut all {
            Some(all) => {
                all.include(bounds.min);
                all.include(bounds.max);
            }
            None => all = Some(bounds),
        }
    }
    all
}

/// How far a read is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionProgress {
    /// Points read so far, over all sources.
    pub read: u64,
    /// Points the read covers: those of the leaves that touch the region,
    /// and all points of a source that is streamed or in memory.
    pub total: u64,
    /// Points handed to the visitor so far.
    pub accepted: u64,
}

impl RegionProgress {
    /// The part that has been read, from 0 to 1.
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            1.0
        } else {
            (self.read as f64 / self.total as f64).min(1.0) as f32
        }
    }
}

/// What a finished read covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionStats {
    /// Points read: fewer than the sources hold when an index kept the read
    /// to the leaves near the region.
    pub read: u64,
    /// Points handed to the visitor: in the region and accepted.
    pub accepted: u64,
    /// Octree leaves read.
    pub leaves: u64,
}

/// What is read of one source.
enum Work<'a> {
    Leaves(&'a OctreeIndex, Vec<&'a IndexedNode>),
    Stream(&'a PointCloud),
    Resident(&'a [IndexedPoint]),
}

impl Work<'_> {
    fn points(&self) -> u64 {
        match self {
            Self::Leaves(_, leaves) => leaves.iter().map(|leaf| leaf.stored_points).sum(),
            Self::Stream(cloud) => cloud.total_points,
            Self::Resident(points) => points.len() as u64,
        }
    }
}

fn plan<'a>(sources: &[RegionSource<'a>], region: Bounds) -> Result<Vec<Work<'a>>, LoadError> {
    // Written so that a NaN fails the test.
    if !(0..3).all(|axis| region.min[axis] <= region.max[axis]) {
        return Err(LoadError::InvalidData(
            "the region is not a box with its minimum below its maximum".into(),
        ));
    }
    sources
        .iter()
        .map(|source| {
            Ok(match source.reader {
                RegionReader::Index(index) => Work::Leaves(
                    index,
                    index.intersecting_leaves(|node| {
                        overlaps(source.transform.bounds(node), region)
                    }),
                ),
                RegionReader::Stream(cloud) => {
                    if cloud.provisional {
                        return Err(LoadError::InvalidData(
                            "the cloud is a preview that was not checked against its source".into(),
                        ));
                    }
                    Work::Stream(cloud)
                }
                RegionReader::Resident(points) => Work::Resident(points),
            })
        })
        .collect()
}

/// Where batches and counts of one reading thread go.
trait Sink {
    fn deliver(&mut self, source: usize, points: &[IndexedPoint]) -> Result<(), LoadError>;
    /// Points read and points delivered since the report before.
    fn report(&mut self, read: u64, delivered: u64) -> Result<(), LoadError>;
}

/// The test every point passes and the batch that collects those that do.
struct Feed<'a> {
    region: Bounds,
    accept: &'a RegionFilter<'a>,
    batch: Vec<IndexedPoint>,
    read: u64,
    delivered: u64,
}

impl<'a> Feed<'a> {
    fn new(region: Bounds, accept: &'a RegionFilter<'a>) -> Self {
        Self {
            region,
            accept,
            batch: Vec::with_capacity(BATCH_POINTS),
            read: 0,
            delivered: 0,
        }
    }

    fn push(
        &mut self,
        source: usize,
        transform: SourceTransform,
        record: IndexedPoint,
        sink: &mut impl Sink,
    ) -> Result<(), LoadError> {
        self.read += 1;
        let mut point = record.point;
        point.xyz = transform.xyz(point.xyz);
        if contains(self.region, point.xyz) && (self.accept)(source, record.ordinal, &point) {
            self.batch.push(IndexedPoint {
                point,
                ordinal: record.ordinal,
            });
            if self.batch.len() == BATCH_POINTS {
                self.flush(source, sink)?;
            }
        }
        if self.read == BATCH_POINTS as u64 {
            self.report(sink)?;
        }
        Ok(())
    }

    fn flush(&mut self, source: usize, sink: &mut impl Sink) -> Result<(), LoadError> {
        if !self.batch.is_empty() {
            sink.deliver(source, &self.batch)?;
            self.delivered += self.batch.len() as u64;
            self.batch.clear();
        }
        Ok(())
    }

    fn report(&mut self, sink: &mut impl Sink) -> Result<(), LoadError> {
        let (read, delivered) = (self.read, self.delivered);
        (self.read, self.delivered) = (0, 0);
        sink.report(read, delivered)
    }

    /// The end of a leaf or of a source: a batch never mixes sources, and a
    /// cancelled job stops at the next leaf at the latest.
    fn finish(&mut self, source: usize, sink: &mut impl Sink) -> Result<(), LoadError> {
        self.flush(source, sink)?;
        self.report(sink)
    }

    fn leaf(
        &mut self,
        source: usize,
        transform: SourceTransform,
        index: &OctreeIndex,
        leaf: &IndexedNode,
        sink: &mut impl Sink,
    ) -> Result<(), LoadError> {
        index.visit_leaf(leaf, |record| self.push(source, transform, record, sink))?;
        self.finish(source, sink)
    }

    fn stream(
        &mut self,
        source: usize,
        transform: SourceTransform,
        cloud: &PointCloud,
        sink: &mut impl Sink,
    ) -> Result<(), LoadError> {
        cloud.validate_source()?;
        let mut ordinal = 0u64;
        visit_points(&cloud.path, &mut |point| {
            let record = IndexedPoint { point, ordinal };
            ordinal += 1;
            self.push(source, transform, record, sink)
        })?;
        self.finish(source, sink)?;
        if ordinal != cloud.total_points {
            return Err(LoadError::InvalidData(format!(
                "{} changed while it was read",
                cloud.path.display()
            )));
        }
        cloud.validate_source()
    }

    fn resident(
        &mut self,
        source: usize,
        transform: SourceTransform,
        points: &[IndexedPoint],
        sink: &mut impl Sink,
    ) -> Result<(), LoadError> {
        for record in points {
            self.push(source, transform, *record, sink)?;
        }
        self.finish(source, sink)
    }
}

struct Serial<'a> {
    state: RegionProgress,
    progress: &'a mut dyn FnMut(RegionProgress) -> Result<(), LoadError>,
    visit: &'a mut RegionVisitor<'a>,
}

impl Sink for Serial<'_> {
    fn deliver(&mut self, source: usize, points: &[IndexedPoint]) -> Result<(), LoadError> {
        (self.visit)(source, points)
    }

    fn report(&mut self, read: u64, delivered: u64) -> Result<(), LoadError> {
        self.state.read += read;
        self.state.accepted += delivered;
        (self.progress)(self.state)
    }
}

/// Visit the points of several layers that lie in a region of the scene.
///
/// - `region` is a box in scene coordinates, faces included; `EVERYWHERE`
///   takes all points.
/// - `accept` decides per point: it gets the position of the source in
///   `sources`, the ordinal of the point in its source file and the point in
///   scene coordinates, and is only asked for points inside the region. This
///   is where deleted points, hidden classes and a selection are left out.
/// - `progress` is called before the first read, then at least once per
///   `BATCH_POINTS` points read and at the end of every leaf and source.
///   Returning an error, such as `LoadError::Cancelled`, stops the read.
/// - `visit` receives the accepted points in batches of at most
///   `BATCH_POINTS`, with the position of their source: each point in scene
///   coordinates with its source ordinal. Nothing is kept between batches.
///
/// Sources are read one after another, in order. Within a source that is
/// streamed or in memory the points keep their file order; leaves of an
/// index come in tree order.
pub fn visit_region(
    sources: &[RegionSource<'_>],
    region: Bounds,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(RegionProgress) -> Result<(), LoadError>,
    visit: &mut RegionVisitor<'_>,
) -> Result<RegionStats, LoadError> {
    let work = plan(sources, region)?;
    let mut sink = Serial {
        state: RegionProgress {
            read: 0,
            total: work.iter().map(Work::points).sum(),
            accepted: 0,
        },
        progress,
        visit,
    };
    (sink.progress)(sink.state)?;
    let mut feed = Feed::new(region, accept);
    let mut leaves = 0u64;
    for (source, (work, layer)) in work.iter().zip(sources).enumerate() {
        match work {
            Work::Leaves(index, nodes) => {
                for leaf in nodes {
                    feed.leaf(source, layer.transform, index, leaf, &mut sink)?;
                    leaves += 1;
                }
            }
            Work::Stream(cloud) => feed.stream(source, layer.transform, cloud, &mut sink)?,
            Work::Resident(points) => feed.resident(source, layer.transform, points, &mut sink)?,
        }
    }
    Ok(RegionStats {
        read: sink.state.read,
        accepted: sink.state.accepted,
        leaves,
    })
}

/// Every point of a layer without an index, read from its source file once:
/// in source coordinates and file order with its ordinal, as
/// `RegionSource::resident` takes them. For a small cloud that is asked for
/// many regions, where streaming the file for each would cost more than
/// holding its points.
///
/// The transform of the layer is left out here on purpose: it goes to
/// `RegionSource::resident` with the points, and is applied on every read.
/// `progress` is as in `visit_region`.
pub fn resident_points(
    cloud: &PointCloud,
    progress: &mut dyn FnMut(RegionProgress) -> Result<(), LoadError>,
) -> Result<Vec<IndexedPoint>, LoadError> {
    let source = RegionSource {
        reader: RegionReader::Stream(cloud),
        transform: SourceTransform::default(),
    };
    let mut points = Vec::new();
    visit_region(
        &[source],
        EVERYWHERE,
        &|_, _, _| true,
        progress,
        &mut |_, batch| {
            points.extend_from_slice(batch);
            Ok(())
        },
    )?;
    Ok(points)
}

type SharedProgress<'a> = &'a mut (dyn FnMut(RegionProgress) -> Result<(), LoadError> + Send);

/// What the reading threads of one parallel read have in common.
struct Shared<'a> {
    progress: Mutex<(RegionProgress, SharedProgress<'a>)>,
    /// Set by the first failure, so that the other threads stop at their
    /// next report.
    stopped: AtomicBool,
    failure: Mutex<Option<LoadError>>,
}

impl Shared<'_> {
    fn fail(&self, error: LoadError) {
        // Stored before the flag is set: a thread that stops because of the
        // flag must not put its own "cancelled" in place of the cause.
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert(error);
        self.stopped.store(true, Ordering::Release);
    }
}

struct Worker<'a, 'p, T> {
    shared: &'a Shared<'p>,
    accumulator: T,
    visit: &'a RegionFold<'a, T>,
}

impl<T> Sink for Worker<'_, '_, T> {
    fn deliver(&mut self, source: usize, points: &[IndexedPoint]) -> Result<(), LoadError> {
        (self.visit)(&mut self.accumulator, source, points)
    }

    fn report(&mut self, read: u64, delivered: u64) -> Result<(), LoadError> {
        if self.shared.stopped.load(Ordering::Acquire) {
            return Err(LoadError::Cancelled);
        }
        let mut guard = self
            .shared
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        guard.0.read += read;
        guard.0.accepted += delivered;
        let state = guard.0;
        (guard.1)(state)
    }
}

enum Job<'a> {
    Leaf(usize, &'a OctreeIndex, &'a IndexedNode),
    Chunk(usize, &'a [IndexedPoint]),
}

/// `visit_region` with the leaves of the indexes read on several threads.
///
/// Every reading thread fills an accumulator of its own: `init` makes one
/// and `visit` adds a batch to it. The accumulators come back for the caller
/// to combine. There are no more of them than threads were reading at a
/// time, but how many there are and which points each one saw differs from
/// run to run, so they must combine to the same result in any order.
///
/// `region`, `accept` and the batches are as in `visit_region`. `progress`
/// is called by one thread at a time, with counts that never go down. An
/// error from any callback stops all threads and is returned. Sources that
/// are streamed are read on the calling thread after the others.
pub fn visit_region_parallel<T: Send>(
    sources: &[RegionSource<'_>],
    region: Bounds,
    accept: &RegionFilter<'_>,
    progress: &mut (dyn FnMut(RegionProgress) -> Result<(), LoadError> + Send),
    init: &(dyn Fn() -> T + Sync),
    visit: &RegionFold<'_, T>,
) -> Result<(Vec<T>, RegionStats), LoadError> {
    let work = plan(sources, region)?;
    let state = RegionProgress {
        read: 0,
        total: work.iter().map(Work::points).sum(),
        accepted: 0,
    };
    progress(state)?;
    let mut jobs = Vec::new();
    let mut leaves = 0u64;
    for (source, work) in work.iter().enumerate() {
        match work {
            Work::Leaves(index, nodes) => {
                leaves += nodes.len() as u64;
                jobs.extend(nodes.iter().map(|leaf| Job::Leaf(source, index, leaf)));
            }
            Work::Resident(points) => jobs.extend(
                points
                    .chunks(BATCH_POINTS)
                    .map(|chunk| Job::Chunk(source, chunk)),
            ),
            Work::Stream(_) => {}
        }
    }
    let shared = Shared {
        progress: Mutex::new((state, progress)),
        stopped: AtomicBool::new(false),
        failure: Mutex::new(None),
    };
    // Readers that are between two jobs. A job takes one, or makes one when
    // all are busy, so there are no more than jobs run at a time.
    let idle: Mutex<Vec<(Feed<'_>, T)>> = Mutex::new(Vec::new());
    let take = || {
        let waiting = idle.lock().unwrap_or_else(PoisonError::into_inner).pop();
        waiting.unwrap_or_else(|| (Feed::new(region, accept), init()))
    };
    let read = jobs.par_iter().try_for_each(|job| {
        let (mut feed, accumulator) = take();
        let mut sink = Worker {
            shared: &shared,
            accumulator,
            visit,
        };
        let done = match *job {
            Job::Leaf(source, index, leaf) => {
                feed.leaf(source, sources[source].transform, index, leaf, &mut sink)
            }
            Job::Chunk(source, points) => {
                feed.resident(source, sources[source].transform, points, &mut sink)
            }
        };
        match done {
            Ok(()) => {
                idle.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((feed, sink.accumulator));
                Ok(())
            }
            Err(error) => {
                shared.fail(error);
                Err(())
            }
        }
    });
    if read.is_err() {
        let failure = shared
            .failure
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        return Err(failure.unwrap_or(LoadError::Cancelled));
    }
    let mut readers = idle.into_inner().unwrap_or_else(PoisonError::into_inner);
    for (source, work) in work.iter().enumerate() {
        if let Work::Stream(cloud) = work {
            let (mut feed, accumulator) = readers
                .pop()
                .unwrap_or_else(|| (Feed::new(region, accept), init()));
            let mut sink = Worker {
                shared: &shared,
                accumulator,
                visit,
            };
            feed.stream(source, sources[source].transform, cloud, &mut sink)?;
            readers.push((feed, sink.accumulator));
        }
    }
    let accumulators = readers
        .into_iter()
        .map(|(_, accumulator)| accumulator)
        .collect();
    let state = shared
        .progress
        .into_inner()
        .unwrap_or_else(PoisonError::into_inner)
        .0;
    Ok((
        accumulators,
        RegionStats {
            read: state.read,
            accepted: state.accepted,
            leaves,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_shapes::{indexed_cloud, IndexedCloud};
    use std::sync::atomic::AtomicU64;

    /// A block of points a quarter apart, so that every coordinate and every
    /// sum of them is exact: 20 by 20 by 12, ordinals counting x fastest.
    fn lattice() -> Vec<Point> {
        let mut points = Vec::new();
        for z in 0..12 {
            for y in 0..20 {
                for x in 0..20 {
                    points.push(Point {
                        xyz: [x as f64 * 0.25, y as f64 * 0.25, z as f64 * 0.25],
                        rgb: Some([x as u8, y as u8, z as u8]),
                        intensity: None,
                        classification: Some(((x + y + z) % 5) as u8),
                    });
                }
            }
        }
        points
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

    /// The three ways to read one cloud.
    fn readers<'a>(
        cloud: &'a IndexedCloud,
        records: &'a [IndexedPoint],
    ) -> [(&'static str, RegionReader<'a>); 3] {
        [
            ("index", RegionReader::Index(&cloud.index)),
            ("stream", RegionReader::Stream(&cloud.cloud)),
            ("resident", RegionReader::Resident(records)),
        ]
    }

    type Seen = Vec<(usize, u64, [f64; 3])>;

    /// Everything the visitor receives, ordered by source and ordinal.
    fn collect(
        sources: &[RegionSource<'_>],
        region: Bounds,
        accept: &RegionFilter<'_>,
    ) -> (Seen, RegionStats) {
        let mut seen = Vec::new();
        let stats = visit_region(
            sources,
            region,
            accept,
            &mut |_| Ok(()),
            &mut |source, batch| {
                assert!(!batch.is_empty() && batch.len() <= BATCH_POINTS);
                seen.extend(
                    batch
                        .iter()
                        .map(|record| (source, record.ordinal, record.point.xyz)),
                );
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(stats.accepted, seen.len() as u64);
        seen.sort_by_key(|entry| (entry.0, entry.1));
        (seen, stats)
    }

    /// What a read must return, found by testing every point.
    fn expected(
        points: &[Point],
        source: usize,
        transform: SourceTransform,
        region: Bounds,
        accept: &dyn Fn(usize, u64, &Point) -> bool,
    ) -> Seen {
        points
            .iter()
            .enumerate()
            .filter_map(|(ordinal, point)| {
                let mut world = *point;
                world.xyz = transform.xyz(point.xyz);
                (contains(region, world.xyz) && accept(source, ordinal as u64, &world)).then_some((
                    source,
                    ordinal as u64,
                    world.xyz,
                ))
            })
            .collect()
    }

    const BOX: Bounds = Bounds {
        min: [1.0, 0.5, 0.25],
        max: [2.5, 3.0, 1.5],
    };

    #[test]
    fn index_stream_and_memory_return_exactly_the_points_inside_the_region() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points);
        let all = |_: usize, _: u64, _: &Point| true;
        let inside = expected(&points, 0, SourceTransform::default(), BOX, &all);
        // The faces of the box count: 7 by 11 by 6 lattice positions.
        assert_eq!(inside.len(), 7 * 11 * 6);
        for (name, reader) in readers(&cloud, &records) {
            let source = RegionSource {
                reader,
                transform: SourceTransform::default(),
            };
            let (seen, stats) = collect(&[source], BOX, &all);
            assert_eq!(seen, inside, "{name}");
            if name == "index" {
                // Only the leaves near the box were read.
                assert!(stats.leaves > 0);
                assert!(
                    stats.read >= inside.len() as u64 && stats.read < 1_200,
                    "{stats:?}"
                );
            } else {
                assert_eq!((stats.read, stats.leaves), (4_800, 0), "{name}");
            }
            let (everything, stats) = collect(&[source], EVERYWHERE, &all);
            assert_eq!(everything.len(), 4_800, "{name}");
            assert_eq!(stats.read, 4_800, "{name}");
            let nothing = Bounds {
                min: [50.0; 3],
                max: [60.0; 3],
            };
            let (none, stats) = collect(&[source], nothing, &all);
            assert!(none.is_empty(), "{name}");
            if name == "index" {
                assert_eq!((stats.read, stats.leaves), (0, 0));
            }
        }
    }

    #[test]
    fn points_keep_their_attributes_and_arrive_in_file_order_from_a_stream() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        for reader in [
            RegionReader::Index(&cloud.index),
            RegionReader::Stream(&cloud.cloud),
        ] {
            let streamed = matches!(reader, RegionReader::Stream(_));
            let source = RegionSource {
                reader,
                transform: SourceTransform::default(),
            };
            let mut last = None;
            let mut count = 0;
            visit_region(
                &[source],
                BOX,
                &|_, _, _| true,
                &mut |_| Ok(()),
                &mut |_, batch| {
                    for record in batch {
                        let written = points[record.ordinal as usize];
                        assert_eq!(record.point.rgb, written.rgb);
                        assert_eq!(record.point.classification, written.classification);
                        if streamed {
                            assert!(last.is_none_or(|last| last < record.ordinal));
                        }
                        last = Some(record.ordinal);
                        count += 1;
                    }
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(count, 7 * 11 * 6);
        }
    }

    #[test]
    fn the_layer_transform_is_applied_before_the_region_test() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points);
        // Mirrored in y, stretched in x and moved far away.
        let transform = SourceTransform {
            scale: [2.0, -1.0, 0.5],
            offset: [207_000.0, 474_000.0, -3.0],
        };
        let region = Bounds {
            min: [207_001.0, 473_996.5, -2.75],
            max: [207_004.5, 473_999.0, -2.0],
        };
        let all = |_: usize, _: u64, _: &Point| true;
        let inside = expected(&points, 0, transform, region, &all);
        // x 0.5 to 2.25, y 1.0 to 3.5 and z 0.5 to 2.0 in the source.
        assert_eq!(inside.len(), 8 * 11 * 7);
        assert!(inside.iter().all(|(_, _, xyz)| contains(region, *xyz)));
        for (name, reader) in readers(&cloud, &records) {
            let (seen, stats) = collect(&[RegionSource { reader, transform }], region, &all);
            assert_eq!(seen, inside, "{name}");
            if name == "index" {
                assert!(stats.read < 2_400, "{stats:?}");
            }
            // The same box in source coordinates holds other points.
            let (unmoved, _) = collect(
                &[RegionSource {
                    reader,
                    transform: SourceTransform::default(),
                }],
                region,
                &all,
            );
            assert!(unmoved.is_empty(), "{name}");
        }
    }

    #[test]
    fn a_layer_read_into_memory_is_asked_for_regions_with_its_own_transform() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let transform = SourceTransform {
            scale: [2.0, -1.0, 0.5],
            offset: [207_000.0, 474_000.0, -3.0],
        };
        let region = Bounds {
            min: [207_001.0, 473_996.5, -2.75],
            max: [207_004.5, 473_999.0, -2.0],
        };
        let all = |_: usize, _: u64, _: &Point| true;
        let layer = RegionSource::new(&cloud.cloud, None, transform);
        let (streamed, _) = collect(&[layer], region, &all);
        assert_eq!(streamed.len(), 8 * 11 * 7);

        // Read once: every point as the file has it, in file order.
        let mut reports = Vec::new();
        let records = resident_points(&cloud.cloud, &mut |progress| {
            reports.push(progress);
            Ok(())
        })
        .unwrap();
        assert_eq!(records.len(), points.len());
        for (position, (record, written)) in records.iter().zip(&points).enumerate() {
            assert_eq!(record.ordinal, position as u64);
            assert_eq!(record.point.xyz, written.xyz);
            assert_eq!(record.point.rgb, written.rgb);
            assert_eq!(record.point.classification, written.classification);
        }
        assert_eq!(reports[0].read, 0);
        assert_eq!(
            reports.last(),
            Some(&RegionProgress {
                read: 4_800,
                total: 4_800,
                accepted: 4_800
            })
        );
        // Asked for a region with the transform of the layer, the points
        // in memory give what the file gives.
        let (seen, stats) = collect(&[RegionSource::resident(&records, transform)], region, &all);
        assert_eq!(seen, streamed);
        assert_eq!(stats.read, 4_800);

        // What a read delivers is in the scene already. Given back with the
        // transform of the layer it is moved a second time and lands
        // elsewhere; it belongs with no transform at all.
        let mut delivered = Vec::new();
        visit_region(
            &[layer],
            EVERYWHERE,
            &all,
            &mut |_| Ok(()),
            &mut |_, batch| {
                delivered.extend_from_slice(batch);
                Ok(())
            },
        )
        .unwrap();
        let (twice, _) = collect(
            &[RegionSource::resident(&delivered, transform)],
            region,
            &all,
        );
        assert!(twice.is_empty());
        let unmoved = RegionSource::resident(&delivered, SourceTransform::default());
        assert_eq!(collect(&[unmoved], region, &all).0, streamed);

        let stopped = resident_points(&cloud.cloud, &mut |_| Err(LoadError::Cancelled));
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
        let mut preview = cloud.cloud.clone();
        preview.provisional = true;
        assert!(resident_points(&preview, &mut |_| Ok(())).is_err());
    }

    #[test]
    fn the_filter_decides_per_point_and_sees_only_points_in_the_region() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points);
        let transform = SourceTransform {
            scale: [1.0; 3],
            offset: [10.0, 0.0, 0.0],
        };
        let region = Bounds {
            min: [11.0, 0.5, 0.25],
            max: [12.5, 3.0, 1.5],
        };
        let asked = AtomicU64::new(0);
        let outside = AtomicU64::new(0);
        // A deleted third, one hidden class, and whatever else the source is.
        let accept = |source: usize, ordinal: u64, point: &Point| {
            asked.fetch_add(1, Ordering::Relaxed);
            if !contains(region, point.xyz) {
                outside.fetch_add(1, Ordering::Relaxed);
            }
            source == 0 && !ordinal.is_multiple_of(3) && point.classification != Some(2)
        };
        let kept = expected(&points, 0, transform, region, &|source, ordinal, point| {
            source == 0 && !ordinal.is_multiple_of(3) && point.classification != Some(2)
        });
        assert!(kept.len() > 100 && kept.len() < 7 * 11 * 6);
        for (name, reader) in readers(&cloud, &records) {
            asked.store(0, Ordering::Relaxed);
            let (seen, stats) = collect(&[RegionSource { reader, transform }], region, &accept);
            assert_eq!(seen, kept, "{name}");
            assert_eq!(stats.accepted, kept.len() as u64);
            // Asked once for every point in the region, and for no other.
            assert_eq!(asked.load(Ordering::Relaxed), 7 * 11 * 6, "{name}");
            assert_eq!(outside.load(Ordering::Relaxed), 0, "{name}");
            // As the second source, the same filter lets nothing through.
            let first = RegionSource::resident(&[], SourceTransform::default());
            let (seen, _) = collect(
                &[first, RegionSource { reader, transform }],
                region,
                &accept,
            );
            assert!(seen.is_empty(), "{name}");
        }
    }

    #[test]
    fn several_sources_are_read_in_one_call_and_named_in_every_batch() {
        let points = lattice();
        let first = indexed_cloud(&points, 64);
        let second = indexed_cloud(&points[..2_000], 64);
        let records = resident(&points[..900]);
        let transforms = [
            SourceTransform::default(),
            SourceTransform {
                scale: [1.0; 3],
                offset: [0.5, 0.0, 0.0],
            },
            SourceTransform {
                scale: [-1.0, 1.0, 1.0],
                offset: [3.0, 0.0, 0.0],
            },
        ];
        let sources = [
            RegionSource::new(&first.cloud, Some(&first.index), transforms[0]),
            RegionSource::new(&second.cloud, None, transforms[1]),
            RegionSource::resident(&records, transforms[2]),
        ];
        assert!(matches!(sources[0].reader, RegionReader::Index(_)));
        assert!(matches!(sources[1].reader, RegionReader::Stream(_)));
        let odd = |_: usize, ordinal: u64, _: &Point| ordinal % 2 == 1;
        let mut inside = expected(&points, 0, transforms[0], BOX, &odd);
        inside.extend(expected(&points[..2_000], 1, transforms[1], BOX, &odd));
        inside.extend(expected(&points[..900], 2, transforms[2], BOX, &odd));
        let per_source = |source| inside.iter().filter(|seen| seen.0 == source).count();
        assert!(per_source(0) > 0 && per_source(1) > 0 && per_source(2) > 0);
        assert_ne!(per_source(0), per_source(1));
        let (seen, stats) = collect(&sources, BOX, &odd);
        assert_eq!(seen, inside);
        assert_eq!(stats.accepted, inside.len() as u64);
        assert!(
            stats.read > 2_900 && stats.read < 4_800 + 2_900,
            "{stats:?}"
        );
        assert!(collect(&[], BOX, &odd).0.is_empty());
    }

    #[test]
    fn batches_are_bounded_and_progress_counts_up_to_the_total() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points);
        for (name, reader) in readers(&cloud, &records) {
            let source = RegionSource {
                reader,
                transform: SourceTransform::default(),
            };
            let mut reports = Vec::new();
            let mut batches = Vec::new();
            let stats = visit_region(
                &[source],
                EVERYWHERE,
                &|_, _, _| true,
                &mut |progress| {
                    reports.push(progress);
                    Ok(())
                },
                &mut |_, batch| {
                    batches.push(batch.len());
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(batches.iter().sum::<usize>(), 4_800, "{name}");
            assert!(batches.iter().all(|size| (1..=BATCH_POINTS).contains(size)));
            if name == "index" {
                // A batch ends with its leaf.
                assert!(batches.iter().all(|size| *size <= 64));
                assert_eq!(batches.len() as u64, stats.leaves);
            } else {
                assert_eq!(batches, [BATCH_POINTS, 4_800 - BATCH_POINTS], "{name}");
            }
            assert_eq!(
                reports[0],
                RegionProgress {
                    read: 0,
                    total: 4_800,
                    accepted: 0
                }
            );
            let last = reports.last().unwrap();
            assert_eq!(
                (last.read, last.accepted, last.total),
                (4_800, 4_800, 4_800)
            );
            assert_eq!(last.fraction(), 1.0);
            for pair in reports.windows(2) {
                assert!(pair[1].read >= pair[0].read && pair[1].accepted >= pair[0].accepted);
                assert!(pair[1].read - pair[0].read <= BATCH_POINTS as u64);
                assert!(pair[1].accepted <= pair[1].read);
            }
            assert!(reports
                .iter()
                .any(|report| report.fraction() > 0.0 && report.fraction() < 1.0));
        }
        let empty = RegionProgress {
            read: 0,
            total: 0,
            accepted: 0,
        };
        assert_eq!(empty.fraction(), 1.0);
    }

    #[test]
    fn a_cancel_from_progress_stops_the_read() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points);
        for (name, reader) in readers(&cloud, &records) {
            let source = RegionSource {
                reader,
                transform: SourceTransform::default(),
            };
            // Cancelled before anything is read.
            let mut visited = 0usize;
            let stopped = visit_region(
                &[source],
                EVERYWHERE,
                &|_, _, _| true,
                &mut |_| Err(LoadError::Cancelled),
                &mut |_, batch| {
                    visited += batch.len();
                    Ok(())
                },
            );
            assert!(matches!(stopped, Err(LoadError::Cancelled)), "{name}");
            assert_eq!(visited, 0, "{name}");
            // Cancelled at the first report after the start.
            let mut reports = 0;
            let mut visited = 0usize;
            let stopped = visit_region(
                &[source],
                EVERYWHERE,
                &|_, _, _| true,
                &mut |_| {
                    reports += 1;
                    if reports == 2 {
                        Err(LoadError::Cancelled)
                    } else {
                        Ok(())
                    }
                },
                &mut |_, batch| {
                    visited += batch.len();
                    Ok(())
                },
            );
            assert!(matches!(stopped, Err(LoadError::Cancelled)), "{name}");
            assert_eq!(reports, 2, "{name}");
            assert!(visited > 0 && visited < 4_800, "{name}: {visited}");
        }
    }

    #[test]
    fn an_error_from_the_visitor_stops_the_read_and_is_returned() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points);
        for (name, reader) in readers(&cloud, &records) {
            let source = RegionSource {
                reader,
                transform: SourceTransform::default(),
            };
            let mut calls = 0;
            let stopped = visit_region(
                &[source, source],
                EVERYWHERE,
                &|_, _, _| true,
                &mut |_| Ok(()),
                &mut |_, _| {
                    calls += 1;
                    Err(LoadError::InvalidData("no room for more".into()))
                },
            );
            assert!(
                matches!(stopped, Err(LoadError::InvalidData(reason)) if reason == "no room for more"),
                "{name}"
            );
            assert_eq!(calls, 1, "{name}");
        }
    }

    #[test]
    fn a_region_must_be_a_box_and_a_stream_needs_a_checked_unchanged_source() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let source = RegionSource::new(&cloud.cloud, None, SourceTransform::default());
        let read = |sources: &[RegionSource<'_>], region: Bounds| {
            visit_region(
                sources,
                region,
                &|_, _, _| true,
                &mut |_| Ok(()),
                &mut |_, _| Ok(()),
            )
        };
        for region in [
            Bounds {
                min: [1.0, 0.0, 0.0],
                max: [0.0, 1.0, 1.0],
            },
            Bounds {
                min: [0.0, f64::NAN, 0.0],
                max: [1.0, 1.0, 1.0],
            },
        ] {
            assert!(matches!(
                read(&[source], region),
                Err(LoadError::InvalidData(_))
            ));
        }
        let mut preview = cloud.cloud.clone();
        preview.provisional = true;
        let unchecked = RegionSource::new(&preview, None, SourceTransform::default());
        assert!(matches!(
            read(&[unchecked], BOX),
            Err(LoadError::InvalidData(_))
        ));
        // One point more in the file than was loaded.
        assert_eq!(read(&[source], BOX).unwrap().accepted, 7 * 11 * 6);
        let mut longer = points.clone();
        longer.push(points[0]);
        crate::test_shapes::write_cloud(&longer, &cloud.cloud.path);
        assert!(matches!(
            read(&[source], BOX),
            Err(LoadError::InvalidData(_))
        ));
        // The change is seen before a point of the other revision is handed on.
        let mut batches = 0;
        let stopped = visit_region(
            &[source],
            EVERYWHERE,
            &|_, _, _| true,
            &mut |_| Ok(()),
            &mut |_, _| {
                batches += 1;
                Ok(())
            },
        );
        assert!(matches!(stopped, Err(LoadError::InvalidData(_))));
        assert_eq!(batches, 0);
        // The index was built from the loaded revision and still answers.
        let indexed =
            RegionSource::new(&cloud.cloud, Some(&cloud.index), SourceTransform::default());
        assert_eq!(read(&[indexed], BOX).unwrap().accepted, 7 * 11 * 6);
    }

    #[test]
    fn the_bounds_of_sources_follow_their_transforms() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points[..20]);
        let mirrored = SourceTransform {
            scale: [-2.0, 1.0, 1.0],
            offset: [10.0, 100.0, 0.0],
        };
        let plain = RegionSource::new(&cloud.cloud, Some(&cloud.index), SourceTransform::default());
        assert_eq!(
            plain.world_bounds(),
            Some(Bounds {
                min: [0.0; 3],
                max: [4.75, 4.75, 2.75]
            })
        );
        let stream = RegionSource::new(&cloud.cloud, None, mirrored);
        assert_eq!(
            stream.world_bounds(),
            Some(Bounds {
                min: [0.5, 100.0, 0.0],
                max: [10.0, 104.75, 2.75]
            })
        );
        let row = RegionSource::resident(&records, SourceTransform::default());
        assert_eq!(
            row.world_bounds(),
            Some(Bounds {
                min: [0.0; 3],
                max: [4.75, 0.0, 0.0]
            })
        );
        let none = RegionSource::resident(&[], mirrored);
        assert_eq!(none.world_bounds(), None);
        assert_eq!(
            world_bounds(&[none, row, stream]),
            Some(Bounds {
                min: [0.0; 3],
                max: [10.0, 104.75, 2.75]
            })
        );
        assert_eq!(world_bounds(&[none]), None);
        assert_eq!(world_bounds(&[]), None);
    }

    #[test]
    fn transform_maps_positions_both_ways_and_orders_mirrored_bounds() {
        let transform = SourceTransform {
            scale: [2.0, -1.0, 0.5],
            offset: [10.0, 20.0, -3.0],
        };
        assert_eq!(transform.xyz([1.0, 2.0, 4.0]), [12.0, 18.0, -1.0]);
        assert_eq!(
            transform.source_xyz([12.0, 18.0, -1.0]),
            Some([1.0, 2.0, 4.0])
        );
        let bounds = transform.bounds(Bounds {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 2.0, 4.0],
        });
        assert_eq!(
            (bounds.min, bounds.max),
            ([10.0, 18.0, -3.0], [12.0, 20.0, -1.0])
        );
        let flat = SourceTransform {
            scale: [1.0, 0.0, 1.0],
            ..transform
        };
        assert_eq!(flat.source_xyz([0.0; 3]), None);
        let identity = SourceTransform::default();
        assert_eq!(identity.xyz([1.5, -2.5, 3.5]), [1.5, -2.5, 3.5]);
        let unit = Bounds {
            min: [0.0; 3],
            max: [1.0; 3],
        };
        let touching = Bounds {
            min: [1.0, 0.0, 0.0],
            max: [2.0, 1.0, 1.0],
        };
        let apart = Bounds {
            min: [1.5, 0.0, 0.0],
            max: [2.0, 1.0, 1.0],
        };
        assert!(overlaps(unit, touching) && overlaps(touching, unit));
        assert!(!overlaps(unit, apart) && !overlaps(apart, unit));
        assert!(overlaps(unit, EVERYWHERE));
        assert!(contains(unit, [1.0, 0.0, 0.5]) && !contains(unit, [1.0, 0.0, 1.5]));
        assert!(!contains(unit, [f64::NAN, 0.0, 0.5]));
    }

    #[test]
    fn leaves_of_an_index_are_listed_and_read_one_by_one() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let all = cloud.index.intersecting_leaves(|_| true);
        assert!(all.len() > 4_800 / 64);
        assert!(all
            .iter()
            .all(|leaf| leaf.is_leaf() && leaf.stored_points <= 64));
        assert_eq!(
            all.iter().map(|leaf| leaf.stored_points).sum::<u64>(),
            4_800
        );
        let near = cloud.index.intersecting_leaves(|node| overlaps(node, BOX));
        assert!(!near.is_empty() && near.len() < all.len());
        assert!(near.iter().all(|leaf| overlaps(leaf.bounds, BOX)));
        // Together the listed leaves hold every point of the box, each once.
        let mut inside = Vec::new();
        for leaf in &near {
            let mut count = 0;
            cloud
                .index
                .visit_leaf(leaf, |record| {
                    count += 1;
                    assert!(contains(leaf.bounds, record.point.xyz));
                    assert_eq!(record.point.xyz, points[record.ordinal as usize].xyz);
                    if contains(BOX, record.point.xyz) {
                        inside.push(record.ordinal);
                    }
                    Ok(())
                })
                .unwrap();
            assert_eq!(count, leaf.stored_points);
        }
        inside.sort_unstable();
        inside.dedup();
        assert_eq!(inside.len(), 7 * 11 * 6);
        assert!(cloud.index.intersecting_leaves(|_| false).is_empty());
        assert!(matches!(
            cloud.index.visit_leaf(&cloud.index.root, |_| Ok(())),
            Err(LoadError::InvalidData(_))
        ));
        let stopped = cloud
            .index
            .visit_leaf(all[0], |_| Err(LoadError::Cancelled));
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
    }

    /// The points every accumulator of a parallel read saw, in order.
    fn collect_parallel(
        sources: &[RegionSource<'_>],
        region: Bounds,
        accept: &RegionFilter<'_>,
    ) -> (Seen, RegionStats) {
        let mut reports: Vec<RegionProgress> = Vec::new();
        let (parts, stats) = visit_region_parallel(
            sources,
            region,
            accept,
            &mut |progress| {
                reports.push(progress);
                Ok(())
            },
            &Vec::new,
            &|seen: &mut Seen, source, batch| {
                assert!(!batch.is_empty() && batch.len() <= BATCH_POINTS);
                seen.extend(
                    batch
                        .iter()
                        .map(|record| (source, record.ordinal, record.point.xyz)),
                );
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(reports[0].read, 0);
        for pair in reports.windows(2) {
            assert!(pair[1].read >= pair[0].read && pair[1].accepted >= pair[0].accepted);
        }
        let last = reports.last().unwrap();
        assert_eq!((last.read, last.accepted), (stats.read, stats.accepted));
        assert_eq!(last.read, last.total);
        let mut seen: Seen = parts.into_iter().flatten().collect();
        seen.sort_by_key(|entry| (entry.0, entry.1));
        (seen, stats)
    }

    #[test]
    fn a_parallel_read_returns_the_same_points_as_a_sequential_one() {
        let points = lattice();
        let first = indexed_cloud(&points, 64);
        let second = indexed_cloud(&points[..2_000], 64);
        let records = resident(&points);
        let moved = SourceTransform {
            scale: [1.0, -1.0, 1.0],
            offset: [0.5, 3.5, 0.0],
        };
        let sources = [
            RegionSource::new(&first.cloud, Some(&first.index), SourceTransform::default()),
            RegionSource::new(&second.cloud, None, moved),
            RegionSource::resident(&records, moved),
            RegionSource::new(&second.cloud, Some(&second.index), moved),
        ];
        let accept = |source: usize, ordinal: u64, point: &Point| {
            !(ordinal + source as u64).is_multiple_of(4) && point.classification != Some(1)
        };
        for region in [BOX, EVERYWHERE] {
            let (sequential, stats) = collect(&sources, region, &accept);
            let (parallel, parallel_stats) = collect_parallel(&sources, region, &accept);
            assert!(sequential.len() > 500);
            assert_eq!(parallel, sequential);
            assert_eq!(parallel_stats, stats);
        }
        let (none, stats) = collect_parallel(&[], BOX, &accept);
        assert!(none.is_empty());
        assert_eq!((stats.read, stats.accepted, stats.leaves), (0, 0, 0));
    }

    #[test]
    fn a_parallel_read_stops_on_a_cancel_and_returns_the_first_error() {
        let points = lattice();
        let cloud = indexed_cloud(&points, 64);
        let records = resident(&points);
        let sources = [
            RegionSource::new(&cloud.cloud, Some(&cloud.index), SourceTransform::default()),
            RegionSource::resident(&records, SourceTransform::default()),
            RegionSource::new(&cloud.cloud, None, SourceTransform::default()),
        ];
        let seen = AtomicU64::new(0);
        let count = |_: &mut (), _: usize, batch: &[IndexedPoint]| {
            seen.fetch_add(batch.len() as u64, Ordering::Relaxed);
            Ok(())
        };
        let mut reports = 0;
        let stopped = visit_region_parallel(
            &sources,
            EVERYWHERE,
            &|_, _, _| true,
            &mut |_| {
                reports += 1;
                if reports >= 4 {
                    Err(LoadError::Cancelled)
                } else {
                    Ok(())
                }
            },
            &|| (),
            &count,
        );
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
        assert!(seen.load(Ordering::Relaxed) < 3 * 4_800);

        let stopped = visit_region_parallel(
            &sources,
            EVERYWHERE,
            &|_, _, _| true,
            &mut |_| Ok(()),
            &|| (),
            &|_: &mut (), source, _| {
                if source == 0 {
                    Err(LoadError::InvalidData("no room for more".into()))
                } else {
                    Ok(())
                }
            },
        );
        assert!(
            matches!(stopped, Err(LoadError::InvalidData(reason)) if reason == "no room for more")
        );

        let before = visit_region_parallel(
            &sources,
            EVERYWHERE,
            &|_, _, _| true,
            &mut |_| Err(LoadError::Cancelled),
            &|| (),
            &|_: &mut (), _, _| panic!("nothing is read after a cancel at the start"),
        );
        assert!(matches!(before, Err(LoadError::Cancelled)));
    }
}
