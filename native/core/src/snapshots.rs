//! The points read so far, shown while a source is still being read, so that
//! the scene appears and can be used long before the pass ends.
//!
//! A source that states how many points it holds is shown in steps, at every
//! tenth of them, from the sample the pass keeps anyway: that sample is
//! spread over everything read so far, and the cloud the pass returns is the
//! same as without the steps. A very large source can instead be shown every
//! few seconds from a denser sample of its own.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::{e57_points, e57_quick, Bounds, Collector, LoadError, PointCloud, SourceStamp};

/// Points of the pass kept for showing a large source densely, and for the
/// checked cloud at its end.
const SNAPSHOT_POINTS: usize = 2_000_000;
/// Points asked of a preview spread through the file.
const SPREAD_POINTS: usize = 1_000_000;
/// Time to the first dense snapshot; later ones follow at growing intervals.
const FIRST_INTERVAL: Duration = Duration::from_secs(3);
const LONGEST_INTERVAL: Duration = Duration::from_secs(10);
/// Sources shown densely while they are read at any one time. A dense
/// snapshot is a copy of millions of points, so further sources are shown in
/// steps instead.
const MAX_SHOWN: usize = 2;

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
    /// it, otherwise not at all.
    pub(crate) fn begin(
        self,
        path: &Path,
        stamp: SourceStamp,
        show: Show,
    ) -> Result<Option<Snapshots>, LoadError> {
        if stamp.length >= self.dense_from {
            if let Some(dense) = Snapshots::begin(path, stamp, show)? {
                return Ok(Some(dense));
            }
        }
        Ok(Snapshots::progressive(path, stamp, self.steps_from))
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

    /// Whether the pass keeps its denser sample for these snapshots.
    pub(crate) fn dense(&self) -> bool {
        self.dense
    }

    /// Start showing a source densely, beginning with the spread preview of
    /// an E57 scan that has one. Returns `None` when enough other sources
    /// are being shown densely already.
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
            stated: 0,
            cadence: Cadence::Timed {
                interval: FIRST_INTERVAL,
                longest: LONGEST_INTERVAL,
                due: Instant::now(),
            },
            dense: true,
        };
        if super::is_e57(path) {
            snapshots.known = match e57_quick::preview(path, SPREAD_POINTS) {
                Ok(Some(spread)) => Some(spread),
                _ => super::open_e57_header(path).ok(),
            };
        }
        if let Some(spread) = snapshots
            .known
            .as_ref()
            .filter(|known| !known.points.is_empty())
        {
            show(spread)?;
        }
        snapshots.cadence = Cadence::Timed {
            interval: FIRST_INTERVAL,
            longest: LONGEST_INTERVAL,
            due: Instant::now() + FIRST_INTERVAL,
        };
        Ok(Some(snapshots))
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
        let due = match self.cadence {
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
        assert!(interval(&snapshots) > FIRST_INTERVAL);
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
