//! The points that the drawings of the Project Browser read, kept per
//! drawing so that a change of its crop region makes it again without
//! reading the scans: see `KeptSlab` of the core. Each drawing keeps at most
//! `DRAWING_LIMIT` bytes, and all drawings together `TOTAL_LIMIT`; past that
//! the drawing used longest ago lets go of its points first. Points kept
//! from a scan that was closed go with it: a scan opened again is another
//! layer, which the core never takes them for.

use std::sync::{Arc, Mutex, Weak};

use pointcloud_core::{KeptSlab, OctreeIndex, PointCloud};
use serde_json::{json, Value};

use crate::job_scene::JobLayer;

/// The most memory the kept points of one drawing take.
pub(crate) const DRAWING_LIMIT: usize = 384 << 20;
/// The most memory the kept points of all drawings take together.
pub(crate) const TOTAL_LIMIT: usize = 1 << 30;

/// A layer the points were kept from. Held weakly, so that a layer that is
/// closed goes, while no other layer can take its place in memory and pass
/// for it.
struct Pin {
    /// What tells the layer from the others for as long as it is open.
    identity: Weak<PointCloud>,
    cloud: Weak<PointCloud>,
    index: Option<Weak<OctreeIndex>>,
}

impl Pin {
    /// Whether the points kept from the layer can still be drawn: it is
    /// open, and what they were read from is still there.
    fn usable(&self, open: &[Arc<PointCloud>]) -> bool {
        self.cloud.strong_count() > 0
            && self
                .index
                .as_ref()
                .is_none_or(|index| index.strong_count() > 0)
            && open
                .iter()
                .any(|layer| std::ptr::eq(Arc::as_ptr(layer), self.identity.as_ptr()))
    }
}

struct Entry {
    guid: String,
    kept: Arc<Mutex<KeptSlab>>,
    pins: Vec<Pin>,
    /// When it was used last, by the count of `KeptSlabs::clock`.
    used: u64,
}

/// The kept points of every drawing of the Project Browser made in this
/// session.
#[derive(Default)]
pub(crate) struct KeptSlabs {
    entries: Vec<Entry>,
    clock: u64,
}

impl KeptSlabs {
    /// The kept points of a drawing, for a job that makes it from `layers`.
    /// The core starts them over when the layers are not those they were
    /// read from.
    pub(crate) fn for_job(&mut self, guid: &str, layers: &[JobLayer]) -> Arc<Mutex<KeptSlab>> {
        self.clock += 1;
        let pins = layers
            .iter()
            .map(|layer| Pin {
                identity: Arc::downgrade(&layer.identity),
                cloud: Arc::downgrade(&layer.cloud),
                index: layer.index.as_ref().map(Arc::downgrade),
            })
            .collect();
        let used = self.clock;
        match self.entries.iter_mut().find(|entry| entry.guid == guid) {
            Some(entry) => {
                entry.pins = pins;
                entry.used = used;
                Arc::clone(&entry.kept)
            }
            None => {
                let kept = Arc::new(Mutex::new(KeptSlab::new(DRAWING_LIMIT)));
                self.entries.push(Entry {
                    guid: guid.to_owned(),
                    kept: Arc::clone(&kept),
                    pins,
                    used,
                });
                kept
            }
        }
    }

    /// After a job: the points kept from scans that are no longer among
    /// the `open` layers go, by `load_identity`, and the drawings used
    /// longest ago let go of their points until all fit in `TOTAL_LIMIT`. A
    /// drawing whose job still runs keeps them.
    pub(crate) fn settle(&mut self, open: &[Arc<PointCloud>]) {
        self.drop_closed(open);
        self.entries
            .sort_by_key(|entry| std::cmp::Reverse(entry.used));
        let mut total = 0usize;
        for entry in &self.entries {
            let Ok(mut kept) = entry.kept.try_lock() else {
                continue;
            };
            if total + kept.bytes() > TOTAL_LIMIT {
                kept.clear();
            }
            total += kept.bytes();
        }
        self.entries
            .retain(|entry| entry.kept.try_lock().map_or(true, |kept| kept.points() > 0));
    }

    /// A drawing that is forgotten lets go of its points.
    pub(crate) fn forget(&mut self, guid: &str) {
        self.entries.retain(|entry| entry.guid != guid);
    }

    /// Let go of the points kept from a scan that is no longer among the
    /// `open` layers: they can never be drawn again. A job that still runs
    /// with them lets go of them when it ends.
    pub(crate) fn drop_closed(&mut self, open: &[Arc<PointCloud>]) {
        self.entries
            .retain(|entry| entry.pins.iter().all(|pin| pin.usable(open)));
    }

    /// What is kept, as `status` of the local API reports it.
    pub(crate) fn value(&self) -> Value {
        let (mut points, mut bytes, mut drawings) = (0usize, 0usize, 0usize);
        for entry in &self.entries {
            if let Ok(kept) = entry.kept.try_lock() {
                if kept.points() > 0 {
                    drawings += 1;
                    points += kept.points();
                    bytes += kept.bytes();
                }
            }
        }
        json!({"drawings": drawings, "points": points, "bytes": bytes})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud_transform::CloudTransform;

    fn guids(kept: &KeptSlabs) -> Vec<&str> {
        kept.entries
            .iter()
            .map(|entry| entry.guid.as_str())
            .collect()
    }

    #[test]
    fn the_points_kept_from_a_scan_go_when_it_is_closed() {
        let directory = tempfile::tempdir().unwrap();
        let cloud = |name: &str| {
            let path = directory.path().join(name);
            std::fs::write(&path, "0 0 0\n1 1 1\n").unwrap();
            pointcloud_core::open(&path, 10).unwrap()
        };
        let first = JobLayer::of_file(cloud("a.xyz"), None);
        let second = JobLayer::of_file(cloud("b.xyz"), None);
        let open = [Arc::clone(&first.identity), Arc::clone(&second.identity)];
        let mut kept = KeptSlabs::default();
        kept.for_job("one", std::slice::from_ref(&first));
        kept.for_job("two", &[first, second]);
        kept.drop_closed(&open);
        assert_eq!(guids(&kept), ["one", "two"]);
        // The second scan closes: the drawing made with it lets go.
        kept.drop_closed(&open[..1]);
        assert_eq!(guids(&kept), ["one"]);

        // A layer that is open still, but whose points were read from a
        // cloud that is gone: they cannot be drawn again either.
        let reread = JobLayer {
            cloud: Arc::new(cloud("a.xyz")),
            identity: Arc::clone(&open[0]),
            name: "a".into(),
            index: None,
            transform: CloudTransform::default(),
            deleted: None,
        };
        kept.for_job("three", std::slice::from_ref(&reread));
        kept.drop_closed(&open[..1]);
        assert_eq!(guids(&kept), ["one", "three"]);
        drop(reread);
        kept.drop_closed(&open[..1]);
        assert_eq!(guids(&kept), ["one"]);
        // After a job the same happens: no scan is open any more.
        kept.settle(&[]);
        assert!(guids(&kept).is_empty());
    }
}
