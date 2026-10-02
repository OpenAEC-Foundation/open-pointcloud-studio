//! The points read so far, shown at intervals while a large source is still
//! being read, so that the scene appears and can be used long before the
//! pass ends.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{e57_quick, Bounds, Collector, LoadError, Point, PointCloud, SourceStamp};

/// Points of the pass kept for showing.
const SNAPSHOT_POINTS: usize = 2_000_000;
/// Points asked of a preview spread through the file.
const SPREAD_POINTS: usize = 2_000_000;
const SNAPSHOT_INTERVAL: Duration = Duration::from_secs(3);

/// Receives every cloud to show; an error ends the pass.
pub(crate) type Show<'a> = &'a mut dyn FnMut(&PointCloud) -> Result<(), LoadError>;

pub(crate) struct Snapshots {
    path: PathBuf,
    stamp: SourceStamp,
    /// What the file states about itself, and for a scan that allows it,
    /// points spread through the file: both are kept with every snapshot.
    known: Option<PointCloud>,
    sample: Collector,
    due: Instant,
}

impl Snapshots {
    /// Start showing a source that is about to be read, beginning with the
    /// spread preview of an E57 scan that has one.
    pub(crate) fn begin(path: &Path, stamp: SourceStamp, show: Show) -> Result<Self, LoadError> {
        let mut known = None;
        if super::is_e57(path) {
            known = match e57_quick::preview(path, SPREAD_POINTS) {
                Ok(Some(spread)) => Some(spread),
                _ => super::open_e57_header(path).ok(),
            };
        }
        if let Some(spread) = known.as_ref().filter(|known| !known.points.is_empty()) {
            show(spread)?;
        }
        Ok(Self {
            path: path.to_path_buf(),
            stamp,
            known,
            sample: Collector::new(SNAPSHOT_POINTS),
            due: Instant::now() + SNAPSHOT_INTERVAL,
        })
    }

    /// Add a point of the pass.
    pub(crate) fn push(&mut self, point: Point) -> Result<(), LoadError> {
        self.sample.push(point)
    }

    /// Show the points read so far when the last snapshot is old enough.
    pub(crate) fn tick(&mut self, show: Show) -> Result<(), LoadError> {
        if Instant::now() < self.due {
            return Ok(());
        }
        self.show(show)
    }

    /// Show everything that was read, at the end of the pass.
    pub(crate) fn show(&mut self, show: Show) -> Result<(), LoadError> {
        if let Some(cloud) = self.cloud() {
            show(&cloud)?;
        }
        self.due = Instant::now() + SNAPSHOT_INTERVAL;
        Ok(())
    }

    /// The provisional cloud of what is known now: its bounds cover only the
    /// points it holds and its count is the stated one, if any.
    fn cloud(&self) -> Option<PointCloud> {
        let known = self.known.as_ref();
        let mut bounds: Option<Bounds> = known
            .filter(|known| !known.points.is_empty())
            .map(|known| known.bounds);
        if let Some(read) = self.sample.bounds {
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
        points.extend_from_slice(&self.sample.points);
        point_ordinals.extend_from_slice(&self.sample.ordinals);
        Some(PointCloud {
            path: self.path.clone(),
            total_points: known
                .map_or(0, |known| known.total_points)
                .max(self.sample.total),
            bounds,
            points,
            point_ordinals,
            has_rgb: self.sample.has_rgb || known.is_some_and(|known| known.has_rgb),
            has_intensity: self.sample.has_intensity
                || known.is_some_and(|known| known.has_intensity),
            has_classification: self.sample.has_classification,
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
