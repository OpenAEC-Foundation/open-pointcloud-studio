//! The points that the drawings of the Project Browser read, kept per
//! drawing so that a change of its crop region makes it again without
//! reading the scans: see `KeptSlab` of the core. Each drawing keeps at most
//! `DRAWING_LIMIT` bytes, and all drawings together `TOTAL_LIMIT`; past that
//! the drawing used longest ago lets go of its points first.

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
    _cloud: Weak<PointCloud>,
    _index: Option<Weak<OctreeIndex>>,
}

struct Entry {
    guid: String,
    kept: Arc<Mutex<KeptSlab>>,
    _pins: Vec<Pin>,
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
                _cloud: Arc::downgrade(&layer.cloud),
                _index: layer.index.as_ref().map(Arc::downgrade),
            })
            .collect();
        let used = self.clock;
        match self.entries.iter_mut().find(|entry| entry.guid == guid) {
            Some(entry) => {
                entry._pins = pins;
                entry.used = used;
                Arc::clone(&entry.kept)
            }
            None => {
                let kept = Arc::new(Mutex::new(KeptSlab::new(DRAWING_LIMIT)));
                self.entries.push(Entry {
                    guid: guid.to_owned(),
                    kept: Arc::clone(&kept),
                    _pins: pins,
                    used,
                });
                kept
            }
        }
    }

    /// After a job: the drawings used longest ago let go of their points
    /// until all fit in `TOTAL_LIMIT`. A drawing whose job still runs keeps
    /// them.
    pub(crate) fn settle(&mut self) {
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
