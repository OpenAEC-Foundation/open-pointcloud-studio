//! The points read so far, shown while a source is still being read, so that
//! the scene appears and can be used long before the pass ends.
//!
//! A source that states how many points it holds is shown in steps, at every
//! tenth of them, from the sample the pass keeps anyway: that sample is
//! spread over everything read so far, and the cloud the pass returns is the
//! same as without the steps. A very large source can instead be shown every
//! second from a denser sample of its own, after a first picture of points
//! spread through an E57 scan that allows it: a coarse one at once, and a
//! denser one read beside the pass.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::{e57_points, e57_quick, Bounds, Collector, LoadError, PointCloud, SourceStamp};

/// Points of the pass kept for showing a large source densely, and for the
/// checked cloud at its end.
const SNAPSHOT_POINTS: usize = 2_000_000;
/// Points of the first picture spread through the file: about what the
/// window draws by default, read in a fraction of a second.
const FIRST_SPREAD_POINTS: usize = 250_000;
/// Points of the denser picture spread through the file, read beside the
/// pass.
const SPREAD_POINTS: usize = 1_000_000;
/// Time from the start of the pass to the first dense snapshot, and from
/// each to the next.
const INTERVAL: Duration = Duration::from_secs(1);
/// Sources shown densely at any one time. A dense snapshot is a copy of
/// millions of points, so further sources are shown in steps instead. Large
/// sources on one disk are read one after another, and those that wait for
/// their turn hold only their pictures spread through the file.
const MAX_SHOWN: usize = 4;

/// A source of known size is shown at every this many parts of its points.
pub(crate) const STEPS: u64 = 10;
/// Sources that state fewer points are read too quickly to be worth showing
/// on the way.
pub(crate) const STEP_MIN_POINTS: u64 = 1_000_000;
/// A source that does not state its size is shown at intervals instead, when
/// its file is at least this large.
const TIMED_MIN_BYTES: u64 = 64 * 1024 * 1024;
const TIMED_FIRST_INTERVAL: Duration = Duration::from_secs(1);
const TIMED_LONGEST_INTERVAL: Duration = Duration::from_secs(4);
/// Header lines read to find the count a PLY or PCD file states.
const MAX_HEADER_LINES: usize = 1024;

static SHOWN: AtomicUsize = AtomicUsize::new(0);

/// Receives every cloud to show; an error ends the pass.
pub(crate) type Show<'a> = &'a mut dyn FnMut(&PointCloud) -> Result<(), LoadError>;

/// From what size a pass shows its source, and how.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Showing {
    /// Source size in bytes from which a source is shown densely, while one
    /// of the places for that is free.
    pub dense_from: u64,
    /// Stated points from which a source is shown in steps.
    pub steps_from: u64,
}

impl Showing {
    pub(crate) const DEFAULT: Self = Self {
        dense_from: super::LARGE_SOURCE_BYTES,
        steps_from: STEP_MIN_POINTS,
    };

    /// Start showing a source that is about to be read: densely when it is
    /// large enough and a place is free, otherwise in steps when it is worth
    /// it, otherwise not at all. A large source shown in steps still shows
    /// the coarse picture spread through an E57 scan that has one at once.
    pub(crate) fn begin(
        self,
        path: &Path,
        stamp: SourceStamp,
        show: Show,
    ) -> Result<Option<Snapshots>, LoadError> {
        let large = stamp.length >= self.dense_from;
        if large {
            if let Some(dense) = Snapshots::begin(path, stamp, show)? {
                return Ok(Some(dense));
            }
        }
        let mut stepped = Snapshots::progressive(path, stamp, self.steps_from);
        if let Some(stepped) = stepped.as_mut().filter(|_| large && super::is_e57(path)) {
            if let Ok(Some(first)) = e57_quick::preview(path, FIRST_SPREAD_POINTS) {
                show(&first)?;
                stepped.known = Some(first);
            }
        }
        Ok(stepped)
    }
}

/// When the next snapshot is due.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cadence {
    /// After an interval that grows with each snapshot up to `longest`.
    Timed {
        interval: Duration,
        longest: Duration,
        due: Instant,
    },
    /// When `next` points have been read, every `every` points, as long as
    /// that is short of the `stated` count.
    Steps { every: u64, next: u64, stated: u64 },
}

pub(crate) struct Snapshots {
    path: PathBuf,
    stamp: SourceStamp,
    /// What the file states about itself, and for a scan that allows it,
    /// points spread through the file: both are kept with every snapshot.
    known: Option<PointCloud>,
    /// The denser picture spread through the file while it is being read.
    spread: Option<Receiver<PointCloud>>,
    /// The count of points the source states, zero when it states none.
    stated: u64,
    cadence: Cadence,
    /// Whether this holds one of the places for showing a source densely.
    dense: bool,
}

impl Drop for Snapshots {
    fn drop(&mut self) {
        if self.dense {
            SHOWN.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Snapshots {
    /// Points the pass keeps when its source is shown densely: enough for
    /// snapshots and for a checked cloud that is as dense as the last of them.
    pub(crate) fn sample_limit(requested: usize) -> usize {
        requested.max(SNAPSHOT_POINTS)
    }

    /// Whether these snapshots hold one of the places for showing a source
    /// densely.
    #[cfg(test)]
    pub(crate) fn dense(&self) -> bool {
        self.dense
    }

    /// Whether the pass keeps its denser sample: for a source shown densely,
    /// and for one shown in steps after a picture spread through the whole
    /// file, so that the checked cloud of either is about as dense as the
    /// last picture and the scene does not thin out when it takes its place.
    pub(crate) fn keeps_dense_sample(&self) -> bool {
        self.dense
            || self
                .known
                .as_ref()
                .is_some_and(|known| !known.points.is_empty())
    }

    /// Start showing a source densely, beginning with the coarse picture
    /// spread through an E57 scan that has one, while a denser one is read
    /// beside the pass. Returns `None` when enough other sources are being
    /// shown densely already.
    pub(crate) fn begin(
        path: &Path,
        stamp: SourceStamp,
        show: Show,
    ) -> Result<Option<Self>, LoadError> {
        let mut shown = SHOWN.load(Ordering::Acquire);
        loop {
            if shown >= MAX_SHOWN {
                return Ok(None);
            }
            match SHOWN.compare_exchange_weak(shown, shown + 1, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(current) => shown = current,
            }
        }
        // From here on dropping the value gives the place back.
        let mut snapshots = Self {
            path: path.to_path_buf(),
            stamp,
            known: None,
            spread: None,
            stated: 0,
            cadence: Cadence::Timed {
                interval: INTERVAL,
                longest: INTERVAL,
                due: Instant::now(),
            },
            dense: true,
        };
        if super::is_e57(path) {
            snapshots.known = match e57_quick::preview(path, FIRST_SPREAD_POINTS) {
                Ok(Some(first)) => {
                    // A small scan is all there in the first picture.
                    if (first.points.len() as u64) < first.total_points {
                        snapshots.spread = spread_beside(path);
                    }
                    Some(first)
                }
                _ => super::open_e57_header(path).ok(),
            };
        }
        if let Some(known) = snapshots
            .known
            .as_ref()
            .filter(|known| !known.points.is_empty())
        {
            show(known)?;
        }
        snapshots.start_clock();
        Ok(Some(snapshots))
    }

    /// Time the next snapshot from now, when the pass starts: a source that
    /// waited for its turn has nothing more to show before.
    pub(crate) fn start_clock(&mut self) {
        if let Cadence::Timed { interval, due, .. } = &mut self.cadence {
            *due = Instant::now() + *interval;
        }
    }

    /// Take the denser picture spread through the file once it has been
    /// read.
    fn take_spread(&mut self) -> bool {
        let Some(spread) = &self.spread else {
            return false;
        };
        match spread.try_recv() {
            Ok(spread) => {
                self.known = Some(spread);
                self.spread = None;
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.spread = None;
                false
            }
        }
    }

    /// Show the denser picture spread through the file as soon as it has
    /// been read, while the source waits for its turn to be read.
    pub(crate) fn tick_waiting(&mut self, show: Show) -> Result<(), LoadError> {
        if self.take_spread() {
            if let Some(cloud) = self.cloud(&Collector::new(1)) {
                show(&cloud)?;
            }
        }
        Ok(())
    }

    /// Show a source from the sample its pass keeps anyway: at every tenth
    /// of the points it states, or every few seconds for a large file that
    /// states no count. `None` when the source is too small to be worth it.
    pub(crate) fn progressive(path: &Path, stamp: SourceStamp, steps_from: u64) -> Option<Self> {
        // An E57 file states its stations and photos too, which every
        // snapshot keeps.
        let known = super::is_e57(path)
            .then(|| super::open_e57_header(path).ok())
            .flatten();
        let stated = known
            .as_ref()
            .map(|known| known.total_points)
            .or_else(|| stated_points(path));
        let cadence = match stated {
            Some(stated) if stated >= steps_from.max(1) => {
                let every = stated.div_ceil(STEPS).max(1);
                Cadence::Steps {
                    every,
                    next: every,
                    stated,
                }
            }
            None if stamp.length >= TIMED_MIN_BYTES => Cadence::Timed {
                interval: TIMED_FIRST_INTERVAL,
                longest: TIMED_LONGEST_INTERVAL,
                due: Instant::now() + TIMED_FIRST_INTERVAL,
            },
            _ => return None,
        };
        Some(Self {
            path: path.to_path_buf(),
            stamp,
            known,
            spread: None,
            stated: stated.unwrap_or(0),
            cadence,
            dense: false,
        })
    }

    /// Whether a snapshot can be due after any point, rather than only when
    /// the clock is looked at.
    pub(crate) fn stepped(&self) -> bool {
        matches!(self.cadence, Cadence::Steps { .. })
    }

    /// Show the points read so far when the next step is reached or the
    /// last snapshot is old enough.
    pub(crate) fn tick(&mut self, read: &Collector, show: Show) -> Result<(), LoadError> {
        // The denser picture spread through the file is shown as soon as it
        // has been read.
        let due = self.take_spread()
            || match self.cadence {
                Cadence::Steps { next, stated, .. } => read.total >= next && next < stated,
                Cadence::Timed { due, .. } => Instant::now() >= due,
            };
        if !due {
            return Ok(());
        }
        if let Some(cloud) = self.cloud(read) {
            show(&cloud)?;
        }
        self.cadence = match self.cadence {
            Cadence::Steps { every, stated, .. } => Cadence::Steps {
                every,
                next: (read.total / every + 1) * every,
                stated,
            },
            Cadence::Timed {
                interval, longest, ..
            } => {
                let interval = interval.mul_f64(1.5).min(longest);
                Cadence::Timed {
                    interval,
                    longest,
                    due: Instant::now() + interval,
                }
            }
        };
        Ok(())
    }

    /// The provisional cloud of what is known now: its bounds cover only the
    /// points it holds and its count is the stated one, if any.
    fn cloud(&self, read: &Collector) -> Option<PointCloud> {
        let known = self.known.as_ref();
        let mut bounds: Option<Bounds> = known
            .filter(|known| !known.points.is_empty())
            .map(|known| known.bounds);
        if let Some(read) = read.bounds {
            match &mut bounds {
                Some(bounds) => {
                    bounds.include(read.min);
                    bounds.include(read.max);
                }
                None => bounds = Some(read),
            }
        }
        let bounds = bounds?;
        let mut points = known.map(|known| known.points.clone()).unwrap_or_default();
        let mut point_ordinals = vec![u64::MAX; points.len()];
        points.extend_from_slice(&read.points);
        point_ordinals.extend_from_slice(&read.ordinals);
        Some(PointCloud {
            path: self.path.clone(),
            total_points: known
                .map_or(0, |known| known.total_points)
                .max(self.stated)
                .max(read.total),
            bounds,
            points,
            point_ordinals,
            has_rgb: read.has_rgb || known.is_some_and(|known| known.has_rgb),
            has_intensity: read.has_intensity || known.is_some_and(|known| known.has_intensity),
            has_classification: read.has_classification,
            scan_poses: known
                .map(|known| known.scan_poses.clone())
                .unwrap_or_default(),
            // The scans of the pass come with the checked cloud. What the
            // file states counts records, not points, and so does not fit
            // the ordinals of the points read.
            scan_ranges: Vec::new(),
            scan_images: known
                .map(|known| known.scan_images.clone())
                .unwrap_or_default(),
            provisional: true,
            source_stamp: Some(self.stamp),
            scan_ranges_known: false,
        })
    }
}

/// Read the denser picture spread through an E57 scan on a thread of its own,
/// beside the pass that reads the file in order. A picture that cannot be
/// read is not sent: the snapshots go on with the coarse one.
fn spread_beside(path: &Path) -> Option<Receiver<PointCloud>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let path = path.to_path_buf();
    std::thread::Builder::new()
        .name("spread preview".into())
        .spawn(move || {
            if let Ok(Some(spread)) = e57_quick::preview(&path, SPREAD_POINTS) {
                let _ = sender.send(spread);
            }
        })
        .ok()?;
    Some(receiver)
}

/// The count of points a source states in its header: the records of an E57
/// file, which invalid records make more than its points, and the points of
/// a LAS, LAZ, PLY or PCD file. `None` for a format that states none, or a
/// header that cannot be read.
pub(crate) fn stated_points(path: &Path) -> Option<u64> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "e57" => e57_points::summary(path)
            .ok()
            .map(|summary| summary.records),
        "las" | "laz" => las::Reader::from_path(path)
            .ok()
            .map(|reader| reader.header().number_of_points()),
        "ply" => header_count(path, |words| match words {
            ["end_header"] => Some(None),
            ["element", "vertex", count] => Some(count.parse().ok()),
            _ => None,
        }),
        "pcd" => {
            let (mut width, mut height) = (None, 1u64);
            header_count(path, |words| {
                let (key, value) = (words.first()?.to_ascii_uppercase(), words.get(1));
                let number = || value.and_then(|value| value.parse::<u64>().ok());
                match key.as_str() {
                    "POINTS" => Some(number()),
                    "WIDTH" => {
                        width = number();
                        None
                    }
                    "HEIGHT" => {
                        height = number().unwrap_or(1);
                        None
                    }
                    "DATA" => Some(width.and_then(|width| width.checked_mul(height))),
                    _ => None,
                }
            })
        }
        _ => None,
    }
}

/// Read the text header of a file line by line until `line` answers.
fn header_count(path: &Path, mut line: impl FnMut(&[&str]) -> Option<Option<u64>>) -> Option<u64> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut text = String::new();
    for _ in 0..MAX_HEADER_LINES {
        text.clear();
        if reader.read_line(&mut text).ok()? == 0 {
            return None;
        }
        let words: Vec<&str> = text.split_whitespace().collect();
        if let Some(answer) = line(&words) {
            return answer.filter(|count| *count > 0);
        }
    }
    None
}

/// Tests that show sources hold this, so that they do not take the places
/// of one another.
#[cfg(test)]
pub(crate) static TEST_PLACES: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Point;

    fn point(x: f64) -> Point {
        Point {
            xyz: [x, 0.0, 1.0],
            rgb: None,
            intensity: Some(7),
            classification: None,
        }
    }

    fn make_due(snapshots: &mut Snapshots) {
        if let Cadence::Timed { due, .. } = &mut snapshots.cadence {
            *due = Instant::now();
        }
    }

    fn interval(snapshots: &Snapshots) -> Duration {
        match snapshots.cadence {
            Cadence::Timed { interval, .. } => interval,
            Cadence::Steps { .. } => Duration::ZERO,
        }
    }

    #[test]
    fn a_snapshot_holds_what_the_pass_has_read_and_is_provisional() {
        let _places = TEST_PLACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("points.xyz");
        std::fs::write(&source, "0 0 1\n").unwrap();
        let stamp = SourceStamp::read(&source).unwrap();
        let mut shown: Vec<PointCloud> = Vec::new();
        let mut show = |cloud: &PointCloud| {
            shown.push(cloud.clone());
            Ok(())
        };
        let mut snapshots = Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .unwrap();
        // A format without metadata has nothing to show before the pass.
        let mut read = Collector::new(3);
        snapshots.tick(&read, &mut show).unwrap();
        for index in 0..10 {
            read.push(point(f64::from(index))).unwrap();
        }
        // Not due yet, then due.
        snapshots.tick(&read, &mut show).unwrap();
        make_due(&mut snapshots);
        snapshots.tick(&read, &mut show).unwrap();
        assert_eq!(interval(&snapshots), INTERVAL);
        assert_eq!(shown.len(), 1);
        let cloud = &shown[0];
        assert!(cloud.provisional && cloud.has_intensity && !cloud.has_rgb);
        assert_eq!(cloud.total_points, 10);
        assert_eq!(cloud.points.len(), 3);
        assert_eq!(cloud.point_ordinals.len(), 3);
        for (point, ordinal) in cloud.points.iter().zip(&cloud.point_ordinals) {
            assert_eq!(point.xyz[0], *ordinal as f64);
        }
        assert_eq!(cloud.bounds.min[0], 0.0);
        assert_eq!(cloud.bounds.max[0], 9.0);

        // An error from the callback ends the pass.
        make_due(&mut snapshots);
        let mut refuse = |_: &PointCloud| Err(LoadError::Cancelled);
        assert!(matches!(
            snapshots.tick(&read, &mut refuse),
            Err(LoadError::Cancelled)
        ));
    }

    #[test]
    fn dense_snapshots_come_every_second_from_the_start_of_the_pass() {
        let _places = TEST_PLACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("points.xyz");
        std::fs::write(&source, "0 0 1\n").unwrap();
        let stamp = SourceStamp::read(&source).unwrap();
        let count = std::cell::Cell::new(0);
        let mut show = |_: &PointCloud| {
            count.set(count.get() + 1);
            Ok(())
        };
        let mut snapshots = Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .unwrap();
        let due = |snapshots: &Snapshots| match snapshots.cadence {
            Cadence::Timed { due, .. } => due.saturating_duration_since(Instant::now()),
            Cadence::Steps { .. } => unreachable!(),
        };
        // The first is due a second after the start.
        let first = due(&snapshots);
        assert!(first > Duration::from_millis(900) && first <= INTERVAL);
        let mut read = Collector::new(10);
        read.push(point(1.0)).unwrap();
        // A tick before then shows nothing.
        snapshots.tick(&read, &mut show).unwrap();
        assert_eq!(count.get(), 0);
        // Each later one is due a second after the one before, however
        // many have been shown.
        for shown in 1..=5 {
            make_due(&mut snapshots);
            snapshots.tick(&read, &mut show).unwrap();
            assert_eq!(count.get(), shown);
            assert_eq!(interval(&snapshots), INTERVAL);
            let next = due(&snapshots);
            assert!(next > Duration::from_millis(900) && next <= INTERVAL);
        }
        // A source that waited for its turn is timed again when its pass
        // starts, so its first snapshot does not come at once.
        make_due(&mut snapshots);
        snapshots.start_clock();
        assert!(due(&snapshots) > Duration::from_millis(900));
        snapshots.tick(&read, &mut show).unwrap();
        assert_eq!(count.get(), 5);
    }

    /// Tick until the denser picture spread through the file has been shown,
    /// or a while has passed.
    fn wait_for_spread(
        snapshots: &mut Snapshots,
        read: Option<&Collector>,
        show: Show,
    ) -> Result<(), LoadError> {
        let started = Instant::now();
        while snapshots.spread.is_some() && started.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(10));
            match read {
                Some(read) => {
                    // No snapshot of the pass is due meanwhile.
                    snapshots.start_clock();
                    snapshots.tick(read, show)?;
                }
                None => snapshots.tick_waiting(show)?,
            }
        }
        Ok(())
    }

    #[test]
    fn a_large_e57_scan_is_shown_coarsely_at_once_then_densely_beside_the_pass() {
        let _places = TEST_PLACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("large.e57");
        crate::e57_quick::tests::write_scan(&source, 400_000, false);
        let stamp = SourceStamp::read(&source).unwrap();
        let shown = std::cell::RefCell::new(Vec::<PointCloud>::new());
        let mut show = |cloud: &PointCloud| {
            shown.borrow_mut().push(cloud.clone());
            Ok(())
        };
        let count = || shown.borrow().len();
        let points = |index: usize| shown.borrow()[index].points.len();
        // A first picture of about a quarter of a million points spread
        // through the file, at once.
        let mut snapshots = Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .unwrap();
        assert!(snapshots.dense());
        assert_eq!(count(), 1);
        let first = points(0);
        assert!((240_000..=260_000).contains(&first), "{first}");
        // While the source waits for its turn, the denser picture is shown
        // as soon as it has been read: every point of this small scan.
        wait_for_spread(&mut snapshots, None, &mut show).unwrap();
        assert_eq!(count(), 2);
        assert!(shown.borrow().iter().all(|cloud| cloud.provisional));
        assert_eq!(points(1), 400_000);
        assert_eq!(shown.borrow()[1].total_points, 400_000);
        // Later snapshots keep it, with what the pass read.
        let mut read = Collector::new(10);
        read.push(point(1.0)).unwrap();
        make_due(&mut snapshots);
        snapshots.tick(&read, &mut show).unwrap();
        assert_eq!(points(2), 400_001);
        drop(snapshots);

        // Read during the pass, it is shown at once, with what the pass
        // read so far, and the next snapshot a second later.
        shown.borrow_mut().clear();
        let mut snapshots = Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .unwrap();
        wait_for_spread(&mut snapshots, Some(&read), &mut show).unwrap();
        assert_eq!(count(), 2);
        assert_eq!(points(1), 400_001);
        assert_eq!(interval(&snapshots), INTERVAL);
        snapshots.tick(&read, &mut show).unwrap();
        assert_eq!(count(), 2, "the next is not due yet");
        drop(snapshots);

        // A scan that the first picture holds whole is shown once.
        let small = directory.path().join("small.e57");
        crate::e57_quick::tests::write_scan(&small, 50_000, false);
        shown.borrow_mut().clear();
        let stamp = SourceStamp::read(&small).unwrap();
        let mut snapshots = Snapshots::begin(&small, stamp, &mut show).unwrap().unwrap();
        assert!(snapshots.spread.is_none());
        snapshots.tick_waiting(&mut show).unwrap();
        drop(snapshots);
        assert_eq!(count(), 1);
        assert_eq!(points(0), 50_000);
    }

    #[test]
    fn a_snapshot_tells_no_station() {
        let _places = TEST_PLACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("scans.e57");
        // The first scan states four records and holds two points, so the
        // pass meets the second scan at ordinal 2 and not at the stated 4.
        crate::e57_points::tests::write_scans(
            &source,
            &[(true, &[true, false, false, true]), (true, &[true; 4])],
        );
        let stamp = SourceStamp::read(&source).unwrap();
        let mut shown: Vec<PointCloud> = Vec::new();
        let mut show = |cloud: &PointCloud| {
            shown.push(cloud.clone());
            Ok(())
        };
        let mut snapshots = Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .unwrap();
        let mut read = Collector::new(10);
        crate::visit_points(&source, &mut |point| read.push(point)).unwrap();
        make_due(&mut snapshots);
        snapshots.tick(&read, &mut show).unwrap();
        let cloud = shown.last().unwrap();
        assert!(cloud.provisional);
        assert_eq!(cloud.scan_poses.len(), 2);
        assert_eq!(cloud.total_points, 8);
        assert_eq!(
            cloud.point_ordinals[cloud.points.len() - 6..],
            [0, 1, 2, 3, 4, 5]
        );
        // Its count is the stated one, which is no measure for the ordinals
        // of the pass: only the checked cloud tells a station.
        assert_eq!(
            (0..8)
                .map(|ordinal| cloud.station_of(ordinal))
                .collect::<Vec<_>>(),
            [None; 8]
        );
        assert!(cloud.scan_ranges.is_empty());
    }

    #[test]
    fn only_a_few_sources_are_shown_densely_at_once() {
        let _places = TEST_PLACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("points.xyz");
        std::fs::write(&source, "0 0 1\n").unwrap();
        let stamp = SourceStamp::read(&source).unwrap();
        let mut show = |_: &PointCloud| Ok(());
        let held: Vec<_> = (0..MAX_SHOWN)
            .map(|_| {
                Snapshots::begin(&source, stamp, &mut show)
                    .unwrap()
                    .unwrap()
            })
            .collect();
        assert!(held.iter().all(Snapshots::dense));
        assert!(Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .is_none());
        // Showing in steps takes no place and gives none back.
        let showing = Showing {
            dense_from: 0,
            steps_from: 0,
        };
        let stepped = showing.begin(&source, stamp, &mut show).unwrap();
        assert!(stepped.is_none(), "an xyz file states no count");
        drop(held);
        assert!(Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .is_some());
        assert_eq!(Snapshots::sample_limit(100), SNAPSHOT_POINTS);
    }

    #[test]
    fn a_large_e57_scan_shown_in_steps_still_shows_its_first_picture_at_once() {
        let _places = TEST_PLACES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let directory = tempfile::tempdir().unwrap();
        let held = directory.path().join("points.xyz");
        std::fs::write(&held, "0 0 1\n").unwrap();
        let held_stamp = SourceStamp::read(&held).unwrap();
        let mut ignore = |_: &PointCloud| Ok(());
        // Every place for showing densely is taken.
        let places: Vec<_> = (0..MAX_SHOWN)
            .map(|_| {
                Snapshots::begin(&held, held_stamp, &mut ignore)
                    .unwrap()
                    .unwrap()
            })
            .collect();
        let source = directory.path().join("large.e57");
        crate::e57_quick::tests::write_scan(&source, 400_000, false);
        let stamp = SourceStamp::read(&source).unwrap();
        let shown = std::cell::RefCell::new(Vec::<PointCloud>::new());
        let mut show = |cloud: &PointCloud| {
            shown.borrow_mut().push(cloud.clone());
            Ok(())
        };
        let showing = Showing {
            dense_from: 0,
            steps_from: 1,
        };
        let mut stepped = showing.begin(&source, stamp, &mut show).unwrap().unwrap();
        assert!(stepped.stepped() && !stepped.dense() && stepped.keeps_dense_sample());
        assert_eq!(shown.borrow().len(), 1);
        let first = shown.borrow()[0].points.len();
        assert!((240_000..=260_000).contains(&first), "{first}");
        // Every step keeps it, with the sample of what was read.
        let mut read = Collector::new(10);
        for index in 0..40_000 {
            read.push(point(f64::from(index))).unwrap();
        }
        stepped.tick(&read, &mut show).unwrap();
        assert_eq!(shown.borrow().len(), 2);
        assert_eq!(shown.borrow()[1].points.len(), first + 10);
        assert_eq!(shown.borrow()[1].total_points, 400_000);
        drop(stepped);

        // Its pass keeps the denser sample, so the checked cloud is not
        // thinner than the pictures it takes the place of.
        shown.borrow_mut().clear();
        let cloud =
            crate::open_showing(&source, 1_000, |_| Ok(()), Some((&mut show, showing)), None)
                .unwrap();
        assert!(!cloud.provisional);
        assert_eq!(cloud.points.len(), 400_000);
        let pictures = shown.borrow();
        assert!(pictures.len() > 1 && pictures.iter().all(|cloud| cloud.provisional));
        assert!(pictures.iter().all(|picture| picture.points.len() >= first));
        drop(pictures);

        // A source below the size for showing densely gets no such picture,
        // and keeps the sample it asks for.
        shown.borrow_mut().clear();
        let small = Showing {
            dense_from: u64::MAX,
            steps_from: 1,
        };
        let stepped = small.begin(&source, stamp, &mut show).unwrap().unwrap();
        assert!(!stepped.keeps_dense_sample());
        assert!(shown.borrow().is_empty());
        drop(stepped);
        let cloud = crate::open_showing(&source, 1_000, |_| Ok(()), Some((&mut show, small)), None)
            .unwrap();
        assert_eq!(cloud.points.len(), 1_000);
        drop(places);
    }

    #[test]
    fn a_source_is_shown_at_every_tenth_of_its_stated_points() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("points.ply");
        let points: Vec<Point> = (0..1_000).map(|index| point(f64::from(index))).collect();
        crate::test_shapes::write_cloud(&points, &source);
        let stamp = SourceStamp::read(&source).unwrap();
        assert_eq!(stated_points(&source), Some(1_000));
        // Too few points to be worth it with the default minimum.
        assert!(Snapshots::progressive(&source, stamp, STEP_MIN_POINTS).is_none());

        let mut snapshots = Snapshots::progressive(&source, stamp, 1_000).unwrap();
        assert!(snapshots.stepped() && !snapshots.dense());
        let mut shown: Vec<PointCloud> = Vec::new();
        let mut show = |cloud: &PointCloud| {
            shown.push(cloud.clone());
            Ok(())
        };
        let mut read = Collector::new(50);
        for point in &points {
            read.push(*point).unwrap();
            snapshots.tick(&read, &mut show).unwrap();
        }
        // At 100, 200, … 900 points; the checked cloud follows at the end.
        assert_eq!(shown.len(), 9);
        for (step, cloud) in shown.iter().enumerate() {
            let read = 100 * (step as u64 + 1);
            assert!(cloud.provisional);
            assert_eq!(cloud.total_points, 1_000, "the stated count");
            assert_eq!(cloud.points.len(), 50);
            assert_eq!(cloud.bounds.max[0], (read - 1) as f64);
            // Spread over all that was read, not the first block alone.
            let last = *cloud.point_ordinals.iter().max().unwrap();
            assert!(last < read && last >= read * 3 / 4, "{last} of {read}");
            if read >= 500 {
                for part in 0..4 {
                    let range = read * part / 4..read * (part + 1) / 4;
                    assert!(
                        cloud
                            .point_ordinals
                            .iter()
                            .any(|ordinal| range.contains(ordinal)),
                        "nothing from {range:?} of {read}"
                    );
                }
            }
        }

        // A pass that jumps past several steps shows once and goes on from
        // where it is.
        let mut snapshots = Snapshots::progressive(&source, stamp, 1).unwrap();
        let mut read = Collector::new(50);
        let mut count = 0;
        let mut counting = |_: &PointCloud| {
            count += 1;
            Ok(())
        };
        for point in &points[..450] {
            read.push(*point).unwrap();
        }
        snapshots.tick(&read, &mut counting).unwrap();
        snapshots.tick(&read, &mut counting).unwrap();
        read.push(points[450]).unwrap();
        snapshots.tick(&read, &mut counting).unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            snapshots.cadence,
            Cadence::Steps {
                every: 100,
                next: 500,
                stated: 1_000
            }
        );
    }

    #[test]
    fn headers_state_their_counts() {
        let directory = tempfile::tempdir().unwrap();
        let pcd = directory.path().join("cloud.pcd");
        std::fs::write(
            &pcd,
            "# .PCD v0.7\nFIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nCOUNT 1 1 1\nWIDTH 3\nHEIGHT 2\nDATA ascii\n0 0 0\n",
        )
        .unwrap();
        assert_eq!(stated_points(&pcd), Some(6));
        std::fs::write(
            &pcd,
            "FIELDS x y z\nwidth 3\nHEIGHT 2\nPOINTS 5\nDATA ascii\n0 0 0\n",
        )
        .unwrap();
        assert_eq!(stated_points(&pcd), Some(5));
        let e57 = directory.path().join("scans.e57");
        crate::e57_points::tests::write_scans(
            &e57,
            &[(true, &[true, false, false, true]), (true, &[true; 4])],
        );
        assert_eq!(stated_points(&e57), Some(8), "records, valid or not");
        let xyz = directory.path().join("points.xyz");
        std::fs::write(&xyz, "0 0 1\n").unwrap();
        assert_eq!(stated_points(&xyz), None);
        let ply = directory.path().join("broken.ply");
        std::fs::write(&ply, "ply\nformat ascii 1.0\nend_header\n").unwrap();
        assert_eq!(stated_points(&ply), None);
    }
}
