//! The points of scans that are still being read. Every reader sends what it
//! has read so far at each step of its source; the window gathers those
//! snapshots and shows them together a few times a second, so that many
//! scans read side by side neither flood it nor make the camera jump.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::Task;
use pointcloud_core::PointCloud;

use crate::{combined_bounds, format_count, Message, Studio};

/// Shortest time between two showings of snapshots.
pub(crate) const FLUSH_INTERVAL: Duration = Duration::from_millis(250);

/// The camera as the application or the user left it.
struct Camera {
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    orbit_point: Option<[f64; 3]>,
    view_label: &'static str,
    auto_camera: Option<(f32, f32, f32, [f32; 2])>,
}

impl Studio {
    /// Keep the latest snapshot of an import until the next flush, and
    /// plan that flush. `indexed` says the import builds its octree in the
    /// same pass.
    pub(crate) fn queue_snapshot(
        &mut self,
        id: u64,
        indexed: bool,
        cloud: Arc<PointCloud>,
    ) -> Task<Message> {
        self.pending_snapshots.insert(id, (indexed, cloud));
        self.plan_snapshot_flush()
    }

    /// Keep the latest snapshot of a layer read without an import, such as
    /// a LAS or LAZ file that cannot be sampled in place, until the next
    /// flush.
    pub(crate) fn queue_layer_snapshot(
        &mut self,
        identity: Arc<PointCloud>,
        cloud: Arc<PointCloud>,
    ) -> Task<Message> {
        match self
            .pending_layer_snapshots
            .iter_mut()
            .find(|(pending, _)| Arc::ptr_eq(pending, &identity))
        {
            Some((_, pending)) => *pending = cloud,
            None => self.pending_layer_snapshots.push((identity, cloud)),
        }
        self.plan_snapshot_flush()
    }

    fn plan_snapshot_flush(&mut self) -> Task<Message> {
        if self.snapshot_flush_scheduled {
            return Task::none();
        }
        self.snapshot_flush_scheduled = true;
        // The first points are shown at once; later ones wait their turn.
        let wait = self.last_snapshot_flush.map_or(Duration::ZERO, |last| {
            FLUSH_INTERVAL.saturating_sub(last.elapsed())
        });
        Task::perform(
            async move {
                if !wait.is_zero() {
                    tokio::time::sleep(wait).await;
                }
            },
            |()| Message::FlushSnapshots,
        )
    }

    /// Show every snapshot that arrived since the last flush. The camera
    /// frames the scene when its first points appear; after that it stays
    /// where it is while further points come in.
    pub(crate) fn flush_snapshots(&mut self) -> Task<Message> {
        self.snapshot_flush_scheduled = false;
        if self.pending_snapshots.is_empty() && self.pending_layer_snapshots.is_empty() {
            return Task::none();
        }
        if self.selection_pending {
            // A snapshot changes the scene, which would discard the
            // selection being computed: they wait for it.
            self.last_snapshot_flush = Some(Instant::now());
            return self.plan_snapshot_flush();
        }
        self.last_snapshot_flush = Some(Instant::now());
        let camera = self.camera();
        let had_points = self.scene_has_points();
        let fresh = self.clouds.is_empty();
        let scene = combined_bounds(&self.clouds);
        let mut pending: Vec<_> = self.pending_snapshots.drain().collect();
        pending.sort_unstable_by_key(|(id, _)| *id);
        let mut tasks = Vec::new();
        for (id, (indexed, cloud)) in pending {
            let shown = if indexed {
                self.show_indexed_snapshot(id, cloud)
            } else {
                self.show_import_snapshot(id, cloud)
            };
            tasks.extend(shown);
        }
        for (identity, cloud) in std::mem::take(&mut self.pending_layer_snapshots) {
            tasks.extend(self.show_layer_snapshot(&identity, cloud));
        }
        if tasks.is_empty() {
            return Task::none();
        }
        // Layers that were added or filled moved nothing yet.
        self.restore_camera(camera);
        let automatic = fresh || self.auto_camera == Some(self.camera_key());
        if automatic && !had_points && self.scene_has_points() {
            self.auto_camera = Some(self.camera_key());
            self.frame_new_scene();
        } else {
            self.preserve_camera_for_scene_change(scene);
            if automatic {
                self.auto_camera = Some(self.camera_key());
            }
        }
        self.revision += 1;
        tasks.push(self.schedule_detail());
        Task::batch(tasks)
    }

    /// Put the snapshot of an import that reads without building an octree
    /// in its layer: the layer of its metadata, or a new one.
    fn show_import_snapshot(&mut self, id: u64, cloud: Arc<PointCloud>) -> Option<Task<Message>> {
        let job = self.imports.get(&id)?;
        if job.cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Some(header) = self.import_headers.get(&id).map(Arc::clone) else {
            return Some(self.add_import_layer(id, cloud));
        };
        let entry = self
            .clouds
            .iter_mut()
            .find(|entry| entry.matches_source(&header))?;
        entry.replace_cloud(cloud);
        Some(Task::none())
    }

    /// Put the snapshot of a layer read without an import in its place,
    /// until the points it is read for arrive.
    fn show_layer_snapshot(
        &mut self,
        identity: &Arc<PointCloud>,
        cloud: Arc<PointCloud>,
    ) -> Option<Task<Message>> {
        let entry = self.clouds.iter_mut().find(|entry| {
            entry.matches_source(identity)
                && (entry.cloud.points.is_empty() || entry.cloud.provisional)
        })?;
        entry.replace_cloud(cloud);
        Some(Task::none())
    }

    /// Put the snapshot of a one-pass import in its layer. The first one
    /// takes the place of the metadata, as the checked cloud would.
    fn show_indexed_snapshot(&mut self, id: u64, cloud: Arc<PointCloud>) -> Option<Task<Message>> {
        if self.imports.contains_key(&id) {
            return self.show_indexed_preview(id, cloud);
        }
        let entry = self
            .clouds
            .iter_mut()
            .find(|entry| entry.index_import_id == Some(id) && entry.cloud.provisional)?;
        entry.replace_cloud(cloud);
        Some(Task::none())
    }

    /// Show the first look at a one-pass import: it ends the import as far
    /// as the list of imports goes, and its layer waits for the octree.
    pub(crate) fn show_indexed_preview(
        &mut self,
        id: u64,
        cloud: Arc<PointCloud>,
    ) -> Option<Task<Message>> {
        let job = self.imports.get(&id)?;
        if job.cancel.load(Ordering::Relaxed) {
            return None;
        }
        self.imports.remove(&id);
        let header = self.import_headers.remove(&id);
        let task = self.finish_import(header.clone(), Ok(Arc::clone(&cloud)));
        let index = header
            .and_then(|header| {
                self.clouds
                    .iter()
                    .position(|entry| entry.matches_source(&header))
            })
            .or_else(|| self.clouds.len().checked_sub(1));
        if let Some(entry) = index.and_then(|index| self.clouds.get_mut(index)) {
            entry.index_import_id = Some(id);
            entry.index_building = true;
        }
        if !cloud.provisional {
            self.status = format!(
                "Preview ready: {} points; building disk octree…",
                format_count(cloud.total_points)
            );
        }
        Some(task)
    }

    fn scene_has_points(&self) -> bool {
        self.clouds
            .iter()
            .any(|entry| !entry.cloud.points.is_empty() || entry.mesh.is_some())
    }

    fn camera_key(&self) -> (f32, f32, f32, [f32; 2]) {
        (self.yaw, self.pitch, self.zoom, self.pan)
    }

    fn camera(&self) -> Camera {
        Camera {
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
            pan: self.pan,
            orbit_point: self.orbit_point,
            view_label: self.view_label,
            auto_camera: self.auto_camera,
        }
    }

    fn restore_camera(&mut self, camera: Camera) {
        self.yaw = camera.yaw;
        self.pitch = camera.pitch;
        self.zoom = camera.zoom;
        self.pan = camera.pan;
        self.orbit_point = camera.orbit_point;
        self.view_label = camera.view_label;
        self.auto_camera = camera.auto_camera;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64};

    use super::*;
    use crate::ImportJob;

    fn job(path: &std::path::Path) -> ImportJob {
        ImportJob {
            path: path.to_path_buf(),
            decoded: Arc::new(AtomicU64::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A provisional look at `cloud` with its first `points` points.
    fn look(cloud: &PointCloud, points: usize) -> Arc<PointCloud> {
        let mut look = cloud.clone();
        look.points.truncate(points);
        look.point_ordinals.truncate(points);
        for axis in 0..3 {
            let values = look.points.iter().map(|point| point.xyz[axis]);
            look.bounds.min[axis] = values.clone().fold(f64::INFINITY, f64::min);
            look.bounds.max[axis] = values.fold(f64::NEG_INFINITY, f64::max);
        }
        look.provisional = true;
        Arc::new(look)
    }

    #[test]
    fn snapshots_of_many_scans_are_shown_together_at_most_a_few_times_a_second() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        let mut scans = Vec::new();
        for id in 1..=3u64 {
            let path = directory.path().join(format!("scan{id}.xyz"));
            let text: String = (0..10)
                .map(|index| format!("{} {} 1\n", index, id * 10))
                .collect();
            std::fs::write(&path, text).unwrap();
            studio.imports.insert(id, job(&path));
            scans.push(pointcloud_core::open(&path, 10).unwrap());
        }

        // The first snapshot is shown at once: its flush needs no wait.
        let _ = studio.update(Message::ImportSnapshot(1, look(&scans[0], 3)));
        assert!(studio.snapshot_flush_scheduled);
        assert!(
            studio.clouds.is_empty(),
            "nothing is shown before the flush"
        );
        let _ = studio.update(Message::FlushSnapshots);
        assert_eq!(studio.clouds.len(), 1);
        let first_frame = (studio.zoom, studio.pan);
        let revision = studio.revision;

        // Later snapshots of all scans wait for one flush; of each scan only
        // the latest is shown.
        let later = look(&scans[0], 6);
        let _ = studio.update(Message::ImportSnapshot(1, look(&scans[0], 5)));
        let _ = studio.update(Message::ImportSnapshot(2, look(&scans[1], 4)));
        let _ = studio.update(Message::ImportSnapshot(1, Arc::clone(&later)));
        let _ = studio.update(Message::ImportSnapshot(3, look(&scans[2], 2)));
        assert_eq!(studio.pending_snapshots.len(), 3);
        assert!(studio.snapshot_flush_scheduled);
        let _ = studio.update(Message::FlushSnapshots);
        assert_eq!(studio.clouds.len(), 3);
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &later));
        // One change of the scene for the snapshot that replaced a layer,
        // and one for each new layer.
        assert_eq!(studio.revision, revision + 3);
        assert!(studio.pending_snapshots.is_empty());

        // The camera framed the first points and stays: the scene grew, so
        // the camera makes up for it instead of framing anew.
        assert!(studio.auto_camera.is_some());
        assert_ne!((studio.zoom, studio.pan), first_frame);
        assert_eq!(
            studio.auto_camera,
            Some((studio.yaw, studio.pitch, studio.zoom, studio.pan))
        );
        let before = (studio.zoom, studio.pan);
        studio.frame_new_scene();
        assert_ne!(
            (studio.zoom, studio.pan),
            before,
            "framing anew would have moved the camera"
        );

        // A flush planned right after the last one waits for its turn.
        let planned = studio.update(Message::ImportSnapshot(2, look(&scans[1], 8)));
        let _ = planned;
        assert!(studio.snapshot_flush_scheduled);
        assert!(studio
            .last_snapshot_flush
            .is_some_and(|last| last.elapsed() < FLUSH_INTERVAL));

        // A snapshot of an import that ended is dropped.
        studio.imports.remove(&3);
        let _ = studio.update(Message::ImportSnapshot(3, look(&scans[2], 9)));
        let _ = studio.update(Message::FlushSnapshots);
        assert_eq!(studio.clouds[2].cloud.points.len(), 2);
        assert_eq!(studio.clouds[1].cloud.points.len(), 8);
    }

    #[test]
    fn a_file_read_without_an_import_shows_its_points_in_its_header_layer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.xyz");
        std::fs::write(
            &path,
            "0 0 1
1 0 1
2 0 1
3 0 1
",
        )
        .unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        // The layer of a header: the count and bounds, no points yet.
        let mut header = (*cloud).clone();
        header.points.clear();
        header.point_ordinals.clear();
        let header = Arc::new(header);
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&header))));

        let snapshot = look(&cloud, 2);
        let _ = studio.update(Message::LayerSnapshot(Arc::clone(&header), look(&cloud, 1)));
        let _ = studio.update(Message::LayerSnapshot(
            Arc::clone(&header),
            Arc::clone(&snapshot),
        ));
        assert_eq!(studio.pending_layer_snapshots.len(), 1);
        let _ = studio.update(Message::FlushSnapshots);
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &snapshot));

        // The points it was read for take its place, and a late snapshot
        // does not come back.
        let _ = studio.update(Message::Refined(
            Arc::clone(&header),
            Ok(Arc::clone(&cloud)),
        ));
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &cloud));
        let _ = studio.update(Message::LayerSnapshot(Arc::clone(&header), snapshot));
        let _ = studio.update(Message::FlushSnapshots);
        assert!(Arc::ptr_eq(&studio.clouds[0].cloud, &cloud));
    }

    #[test]
    fn snapshots_wait_for_a_selection_being_computed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.xyz");
        std::fs::write(&path, "0 0 1\n1 0 1\n2 0 1\n").unwrap();
        let cloud = pointcloud_core::open(&path, 10).unwrap();
        let mut studio = Studio::default();
        studio.imports.insert(4, job(&path));
        studio.selection_pending = true;
        let _ = studio.update(Message::ImportSnapshot(4, look(&cloud, 2)));
        let _ = studio.update(Message::FlushSnapshots);
        assert!(studio.clouds.is_empty());
        assert!(studio.snapshot_flush_scheduled, "a later flush is planned");
        studio.selection_pending = false;
        let _ = studio.update(Message::FlushSnapshots);
        assert_eq!(studio.clouds.len(), 1);
        assert!(studio.clouds[0].cloud.provisional);
    }
}
