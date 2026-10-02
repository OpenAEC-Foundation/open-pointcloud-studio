//! The points read so far, shown at intervals while a large source is still
//! being read, so that the scene appears and can be used long before the
//! pass ends.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::{e57_quick, Bounds, Collector, LoadError, PointCloud, SourceStamp};

/// Points of the pass kept for showing, and for the checked cloud at its end.
const SNAPSHOT_POINTS: usize = 2_000_000;
/// Points asked of a preview spread through the file.
const SPREAD_POINTS: usize = 1_000_000;
/// Time to the first snapshot; later ones follow at growing intervals.
const FIRST_INTERVAL: Duration = Duration::from_secs(3);
const LONGEST_INTERVAL: Duration = Duration::from_secs(10);
/// Sources shown while they are read at any one time. A snapshot is a copy
/// of millions of points, so further sources are read without them.
const MAX_SHOWN: usize = 2;

static SHOWN: AtomicUsize = AtomicUsize::new(0);

/// Receives every cloud to show; an error ends the pass.
pub(crate) type Show<'a> = &'a mut dyn FnMut(&PointCloud) -> Result<(), LoadError>;

pub(crate) struct Snapshots {
    path: PathBuf,
    stamp: SourceStamp,
    /// What the file states about itself, and for a scan that allows it,
    /// points spread through the file: both are kept with every snapshot.
    known: Option<PointCloud>,
    interval: Duration,
    due: Instant,
}

impl Drop for Snapshots {
    fn drop(&mut self) {
        SHOWN.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Snapshots {
    /// Points the pass keeps when its source is shown: enough for snapshots
    /// and for a checked cloud that is as dense as the last of them.
    pub(crate) fn sample_limit(requested: usize) -> usize {
        requested.max(SNAPSHOT_POINTS)
    }

    /// Start showing a source that is about to be read, beginning with the
    /// spread preview of an E57 scan that has one. Returns `None` when enough
    /// other sources are being shown already.
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
            interval: FIRST_INTERVAL,
            due: Instant::now(),
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
        snapshots.due = Instant::now() + snapshots.interval;
        Ok(Some(snapshots))
    }

    /// Show the points read so far when the last snapshot is old enough.
    pub(crate) fn tick(&mut self, read: &Collector, show: Show) -> Result<(), LoadError> {
        if Instant::now() < self.due {
            return Ok(());
        }
        if let Some(cloud) = self.cloud(read) {
            show(&cloud)?;
        }
        self.interval = self.interval.mul_f64(1.5).min(LONGEST_INTERVAL);
        self.due = Instant::now() + self.interval;
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
            total_points: known.map_or(0, |known| known.total_points).max(read.total),
            bounds,
            points,
            point_ordinals,
            has_rgb: read.has_rgb || known.is_some_and(|known| known.has_rgb),
            has_intensity: read.has_intensity || known.is_some_and(|known| known.has_intensity),
            has_classification: read.has_classification,
            scan_poses: known
                .map(|known| known.scan_poses.clone())
                .unwrap_or_default(),
            scan_images: known
                .map(|known| known.scan_images.clone())
                .unwrap_or_default(),
            provisional: true,
            source_stamp: Some(self.stamp),
        })
    }
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
        snapshots.due = Instant::now();
        snapshots.tick(&read, &mut show).unwrap();
        assert!(snapshots.interval > FIRST_INTERVAL);
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
        snapshots.due = Instant::now();
        let mut refuse = |_: &PointCloud| Err(LoadError::Cancelled);
        assert!(matches!(
            snapshots.tick(&read, &mut refuse),
            Err(LoadError::Cancelled)
        ));
    }

    #[test]
    fn only_a_few_sources_are_shown_at_once() {
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
        assert!(Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .is_none());
        drop(held);
        assert!(Snapshots::begin(&source, stamp, &mut show)
            .unwrap()
            .is_some());
        assert_eq!(Snapshots::sample_limit(100), SNAPSHOT_POINTS);
    }
}
