use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use pointcloud_core::{visit_points, LoadError, OctreeIndex, Point, PointCloud};

use crate::selection::DeletionMask;

struct Mean {
    origin: [f64; 3],
    sum: [f64; 3],
    compensation: [f64; 3],
    count: u64,
}

impl Mean {
    fn new(cloud: &PointCloud) -> Self {
        Self {
            origin: cloud.bounds.center(),
            sum: [0.0; 3],
            compensation: [0.0; 3],
            count: 0,
        }
    }

    fn push(&mut self, point: Point) {
        for axis in 0..3 {
            let value = point.xyz[axis] - self.origin[axis] - self.compensation[axis];
            let next = self.sum[axis] + value;
            self.compensation[axis] = (next - self.sum[axis]) - value;
            self.sum[axis] = next;
        }
        self.count += 1;
    }

    fn finish(self, expected: u64) -> Result<[f64; 3], String> {
        if self.count != expected || self.count == 0 {
            return Err(format!(
                "centroid needs {expected} visible points, found {}",
                self.count
            ));
        }
        Ok(std::array::from_fn(|axis| {
            self.origin[axis] + self.sum[axis] / self.count as f64
        }))
    }
}

/// Fast path for sources whose complete point stream is resident in the GUI.
pub fn resident(
    cloud: &PointCloud,
    deleted: Option<&DeletionMask>,
) -> Option<Result<[f64; 3], String>> {
    if cloud.points.len() as u64 != cloud.total_points
        || cloud.point_ordinals.len() != cloud.points.len()
        || cloud.point_ordinals.contains(&u64::MAX)
    {
        return None;
    }
    let mut mean = Mean::new(cloud);
    for (point, ordinal) in cloud.points.iter().zip(&cloud.point_ordinals) {
        if deleted.is_none_or(|mask| !mask.contains(*ordinal)) {
            mean.push(*point);
        }
    }
    Some(mean.finish(cloud.total_points - deleted.map_or(0, |mask| mask.count)))
}

/// Stream the exact original points from the disk octree when available,
/// falling back to the source decoder. Coordinates are rebased before the
/// compensated sum so survey coordinate magnitudes do not erase precision.
pub fn streamed(
    cloud: &PointCloud,
    index: Option<&OctreeIndex>,
    deleted: Option<&DeletionMask>,
    cancel: &AtomicBool,
    progress: &AtomicU64,
) -> Result<[f64; 3], String> {
    cloud.validate_source().map_err(|error| error.to_string())?;
    let mut mean = Mean::new(cloud);
    let mut seen = 0u64;
    let mut consume = |ordinal: u64, point: Point| -> Result<(), LoadError> {
        seen += 1;
        if seen.is_multiple_of(65_536) {
            progress.store(seen, Ordering::Relaxed);
            if cancel.load(Ordering::Relaxed) {
                return Err(LoadError::Cancelled);
            }
        }
        if deleted.is_none_or(|mask| !mask.contains(ordinal)) {
            mean.push(point);
        }
        Ok(())
    };
    if let Some(index) = index {
        index.visit_intersecting(|_| true, |record| consume(record.ordinal, record.point))
    } else {
        let mut ordinal = 0u64;
        visit_points(&cloud.path, &mut |point| {
            consume(ordinal, point)?;
            ordinal += 1;
            Ok(())
        })
    }
    .map_err(|error| error.to_string())?;
    progress.store(seen, Ordering::Relaxed);
    if cancel.load(Ordering::Relaxed) {
        return Err("Operation cancelled".into());
    }
    if seen != cloud.total_points {
        return Err(format!(
            "source changed while calculating centroid: expected {}, found {seen}",
            cloud.total_points
        ));
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    mean.finish(cloud.total_points - deleted.map_or(0, |mask| mask.count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::SelectionMask;
    use pointcloud_core::{IndexConfig, IndexedPoint};
    use std::sync::Arc;

    #[test]
    fn indexed_and_resident_centroids_agree_after_deletion() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("survey.xyz");
        std::fs::write(
            &path,
            "207000.001 474000.002 10\n207000.003 474000.004 20\n207000.050 474000.060 30\n",
        )
        .unwrap();
        let cloud = pointcloud_core::open(&path, 3).unwrap();
        let record = IndexedPoint {
            point: cloud.points[2],
            ordinal: cloud.point_ordinals[2],
        };
        let selection = SelectionMask::single(3, record).unwrap();
        let mut deleted = DeletionMask::new(3).unwrap();
        deleted.apply(&selection).unwrap();
        let expected = resident(&cloud, Some(&deleted)).unwrap().unwrap();
        assert!((expected[0] - 207000.002).abs() < 1e-9);
        assert!((expected[1] - 474000.003).abs() < 1e-9);
        assert_eq!(expected[2], 15.0);
        assert!((expected[0] - cloud.bounds.center()[0]).abs() > 0.01);

        let index = Arc::new(
            OctreeIndex::build(
                &cloud,
                IndexConfig {
                    leaf_points: 1,
                    preview_points: 2,
                    max_depth: 4,
                    scratch_dir: Some(directory.path().to_path_buf()),
                },
            )
            .unwrap(),
        );
        let progress = AtomicU64::new(0);
        let actual = streamed(
            &cloud,
            Some(&index),
            Some(&deleted),
            &AtomicBool::new(false),
            &progress,
        )
        .unwrap();
        assert_eq!(actual, expected);
        assert_eq!(progress.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn indexed_centroid_uses_points_outside_the_resident_preview() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wide.xyz");
        std::fs::write(&path, "0 0 0\n1 0 0\n2 0 0\n100 0 0\n").unwrap();
        let cloud = pointcloud_core::open(&path, 1).unwrap();
        assert_eq!(cloud.points.len(), 1);
        assert!(resident(&cloud, None).is_none());
        let index = OctreeIndex::build(
            &cloud,
            IndexConfig {
                leaf_points: 1,
                preview_points: 1,
                max_depth: 4,
                scratch_dir: Some(directory.path().to_path_buf()),
            },
        )
        .unwrap();
        let centroid = streamed(
            &cloud,
            Some(&index),
            None,
            &AtomicBool::new(false),
            &AtomicU64::new(0),
        )
        .unwrap();
        assert!((centroid[0] - 25.75).abs() < 1e-12);
        assert!((centroid[0] - cloud.bounds.center()[0]).abs() > 20.0);
    }
}
