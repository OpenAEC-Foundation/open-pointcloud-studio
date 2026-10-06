//! Octree builds that run side by side: how many the machine takes at once,
//! which scan each one builds, and the queue of scans waiting for a place.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iced::Task;
use pointcloud_core::{IndexConfig, IndexProgress, IndexStage, OctreeIndex, PointCloud};
use serde_json::{json, Value};

use crate::{display_name, i18n, CloudEntry, Message, Studio};

/// Most builds that run at once. Ten station scans of ten million points
/// each (156 MB E57 files on a local disk, 24 cores with 32 threads) were
/// indexed one at a time in 38 to 53 s, two at a time in 18 to 32 s, three
/// in 16 to 27 s and four in 16 to 22 s; five, six and eight took 15 to
/// 18 s, within the spread of four, while each further build holds another
/// decoder, write buffer and set of open files, and for scans on a network
/// share another stream for the same bandwidth.
const MAX_BUILDS: usize = 4;
/// Logical cores per build: a build decodes its source on one thread and
/// partitions its records on pools that all builds share, so a further build
/// helps while there are cores to spare.
const CORES_PER_BUILD: usize = 4;
/// Memory a build may hold: its write buffer, the decoder of its source and
/// its share of the blocks of the pools, with a wide margin; one more build
/// added about 20 MB in the measurement above.
const MEMORY_PER_BUILD: u64 = 256 * 1024 * 1024;
/// Memory left to the window and the rest of the system.
const MEMORY_RESERVE: u64 = 2 * 1024 * 1024 * 1024;

/// How often the window looks at the running builds.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// One octree build that is running.
pub(crate) struct IndexJob {
    /// The cloud whose octree is built; `None` for an import that reads its
    /// source and builds the octree in one pass.
    pub source: Option<Arc<PointCloud>>,
    /// The import that builds this octree in one pass with its reading.
    pub import_id: Option<u64>,
    pub path: PathBuf,
    pub progress: Arc<Mutex<IndexProgress>>,
    /// Stops the whole job; for an import, its reading too.
    pub cancel: Arc<AtomicBool>,
    /// Stops the octree of an import once its source has been read, which
    /// keeps the checked cloud.
    pub stop_tree: Arc<AtomicBool>,
}

impl IndexJob {
    /// A build of a file that has just started, for tests of what waits for
    /// builds.
    #[cfg(test)]
    pub(crate) fn for_test(path: &str) -> Self {
        Self {
            source: None,
            import_id: None,
            path: PathBuf::from(path),
            progress: Arc::new(Mutex::new(IndexProgress {
                stage: IndexStage::ReadingSource,
                completed: 0,
                total: 0,
                depth: 0,
                leaves: 0,
                settled: 0,
            })),
            cancel: Arc::new(AtomicBool::new(false)),
            stop_tree: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn progress(&self) -> Option<IndexProgress> {
        self.progress.lock().ok().map(|progress| *progress)
    }

    /// Whether the build waits for its turn: to read its source after the
    /// large scans opened before it on the same disk, or to build its octree
    /// after theirs.
    pub(crate) fn waits_for_turn(&self) -> bool {
        self.progress().is_some_and(|progress| {
            matches!(
                progress.stage,
                IndexStage::WaitingToRead | IndexStage::WaitingToBuild
            )
        })
    }

    pub(crate) fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.stop_tree.load(Ordering::Relaxed)
    }

    /// Stop the build: a one-pass import reads on and keeps its checked
    /// cloud; any other build stops at once.
    fn stop(&self) {
        if self.import_id.is_some() {
            self.stop_tree.store(true, Ordering::Relaxed);
        } else {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    fn value(&self) -> Value {
        let progress = self.progress();
        json!({
            "path": self.path,
            "import_id": self.import_id,
            "stage": progress.map(|progress| stage_key(progress.stage)),
            "completed": progress.map(|progress| progress.completed),
            "total": progress.map(|progress| progress.total),
            "fraction": progress.and_then(|progress| progress.fraction()),
            "cancelling": self.cancelling(),
        })
    }
}

pub(crate) fn stage_key(stage: IndexStage) -> &'static str {
    match stage {
        IndexStage::WaitingToRead => "waiting_to_read",
        IndexStage::ReadingSource => "reading_source",
        IndexStage::WaitingToBuild => "waiting_to_build",
        IndexStage::BuildingTree => "building_tree",
        IndexStage::Ready => "ready",
    }
}

/// How many builds run at once on a machine with this many logical cores
/// and this much memory available: one per few cores, as far as the memory
/// holds them, never more than the disk and the pools are faster with.
pub(crate) fn build_limit(cores: usize, available_memory: Option<u64>) -> usize {
    let by_cores = cores / CORES_PER_BUILD;
    let by_memory = available_memory.map_or(usize::MAX, |bytes| {
        usize::try_from(bytes.saturating_sub(MEMORY_RESERVE) / MEMORY_PER_BUILD)
            .unwrap_or(usize::MAX)
    });
    by_cores.min(by_memory).clamp(1, MAX_BUILDS)
}

/// The builds this machine runs at once, from its cores and the memory
/// that is available now.
pub(crate) fn machine_build_limit() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, usize::from);
    build_limit(cores, available_memory())
}

/// Physical memory that is free or can be freed at once.
#[cfg(windows)]
fn available_memory() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatus {
        length: u32,
        memory_load: u32,
        total_physical: u64,
        available_physical: u64,
        total_page_file: u64,
        available_page_file: u64,
        total_virtual: u64,
        available_virtual: u64,
        available_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    }
    let mut status = MemoryStatus {
        length: std::mem::size_of::<MemoryStatus>() as u32,
        memory_load: 0,
        total_physical: 0,
        available_physical: 0,
        total_page_file: 0,
        available_page_file: 0,
        total_virtual: 0,
        available_virtual: 0,
        available_extended_virtual: 0,
    };
    // SAFETY: the structure has the layout the function fills, and its
    // length field says so.
    let filled = unsafe { GlobalMemoryStatusEx(&mut status) };
    (filled != 0).then_some(status.available_physical)
}

#[cfg(target_os = "linux")]
fn available_memory() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

/// Other systems are judged by their cores alone.
#[cfg(not(any(windows, target_os = "linux")))]
fn available_memory() -> Option<u64> {
    None
}

impl Studio {
    /// Whether an octree build is running.
    pub(crate) fn index_pending(&self) -> bool {
        !self.index_jobs.is_empty()
    }

    fn index_place_free(&self) -> bool {
        self.index_jobs.len() < self.index_limit.max(1)
    }

    /// Whether a build may start now for an import of this file, which reads
    /// its source and builds its octree in one pass.
    pub(crate) fn index_import_allowed(&self, path: &Path) -> bool {
        self.auto_index
            && self.index_place_free()
            && !self.index_declined.contains(path)
            && !self.indexing_path(path)
    }

    fn indexing_path(&self, path: &Path) -> bool {
        self.index_jobs.iter().any(|job| job.path == path)
    }

    /// The build that makes the octree of a layer, while one runs.
    pub(crate) fn index_job_of(&self, entry: &CloudEntry) -> Option<&IndexJob> {
        self.index_jobs
            .iter()
            .find(|job| match (&job.source, job.import_id) {
                (Some(source), _) => entry.matches_source(source),
                (None, Some(id)) => {
                    entry.index_import_id == Some(id)
                        || self
                            .import_headers
                            .get(&id)
                            .is_some_and(|header| entry.matches_source(header))
                }
                (None, None) => false,
            })
    }

    /// Whether the user asked for the octree of a layer that waits for a
    /// place.
    pub(crate) fn index_requested(&self, entry: &CloudEntry) -> bool {
        self.index_requests
            .iter()
            .any(|request| entry.matches_source(request))
    }

    /// Whether a layer waits for an octree build.
    pub(crate) fn index_queued(&self, entry: &CloudEntry) -> bool {
        entry.index.is_none()
            && !entry.index_building
            && (self.index_requested(entry) || (entry.auto_index_queued && self.auto_index))
    }

    /// The layers waiting for a build, in the order they get one: those the
    /// user asked for in the order asked, then the active layer, then the
    /// others in the order of the list.
    pub(crate) fn index_queue(&self) -> Vec<usize> {
        let mut queue: Vec<usize> = self
            .index_requests
            .iter()
            .filter_map(|request| {
                self.clouds
                    .iter()
                    .position(|entry| entry.matches_source(request))
            })
            .collect();
        if self.auto_index {
            let automatic = self
                .active
                .into_iter()
                .chain(0..self.clouds.len())
                .filter(|index| {
                    self.clouds
                        .get(*index)
                        .is_some_and(|entry| entry.auto_index_queued)
                });
            for index in automatic {
                if !queue.contains(&index) {
                    queue.push(index);
                }
            }
        }
        queue.retain(|index| {
            let entry = &self.clouds[*index];
            entry.index.is_none() && !entry.index_building
        });
        queue
    }

    pub(crate) fn index_waiting(&self) -> usize {
        self.index_queue().len()
    }

    /// Start building the octree of a checked cloud on a place of its own.
    pub(crate) fn start_index_job(
        &mut self,
        source: Arc<PointCloud>,
        automatic: bool,
    ) -> Task<Message> {
        let progress = Arc::new(Mutex::new(IndexProgress {
            stage: IndexStage::ReadingSource,
            completed: 0,
            total: source.total_points,
            depth: 0,
            leaves: 0,
            settled: 0,
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        self.index_jobs.push(IndexJob {
            source: Some(Arc::clone(&source)),
            import_id: None,
            path: source.path.clone(),
            progress: Arc::clone(&progress),
            cancel: Arc::clone(&cancel),
            stop_tree: Arc::new(AtomicBool::new(false)),
        });
        let message_source = Arc::clone(&source);
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    OctreeIndex::build_cached_with_progress(
                        &source,
                        IndexConfig::default(),
                        |update| {
                            if cancel.load(Ordering::Relaxed) {
                                return Err(pointcloud_core::LoadError::Cancelled);
                            }
                            if let Ok(mut current) = progress.lock() {
                                *current = update;
                            }
                            Ok(())
                        },
                    )
                    .map(Arc::new)
                    .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| {
                if automatic {
                    Message::AutoIndexReady(Arc::clone(&message_source), result)
                } else {
                    Message::IndexReady(Arc::clone(&message_source), result)
                }
            },
        );
        Task::batch([worker, self.poll_index_jobs()])
    }

    /// Look at the running builds a few times a second while there are any;
    /// one poll serves them all.
    pub(crate) fn poll_index_jobs(&mut self) -> Task<Message> {
        if self.index_polling {
            return Task::none();
        }
        self.index_polling = true;
        Task::perform(async { tokio::time::sleep(POLL_INTERVAL).await }, |()| {
            Message::IndexPoll
        })
    }

    /// The next poll, or none when no build runs any more.
    pub(crate) fn index_poll(&mut self) -> Task<Message> {
        self.index_polling = false;
        if !self.index_pending() {
            return Task::none();
        }
        match self.index_jobs.as_slice() {
            [job] if !job.cancelling() => {
                if let Some(progress) = job.progress() {
                    self.status = Self::index_progress_text(progress);
                }
            }
            [_] => {}
            jobs if jobs.iter().all(IndexJob::cancelling) => {}
            jobs => {
                // Builds that wait for their turn wait like the queue.
                let turns = jobs.iter().filter(|job| job.waits_for_turn()).count();
                self.status = i18n::tr_args(
                    "Building {running} octrees at once; {waiting} waiting",
                    &[
                        ("running", &(jobs.len() - turns)),
                        ("waiting", &(self.index_waiting() + turns)),
                    ],
                );
            }
        }
        self.poll_index_jobs()
    }

    /// Start the builds of waiting layers while there are free places.
    pub(crate) fn start_queued_indexes(&mut self) -> Task<Message> {
        // Requests for layers that were closed are forgotten.
        let clouds = &self.clouds;
        self.index_requests.retain(|request| {
            clouds
                .iter()
                .any(|entry| entry.matches_source(request) && entry.index.is_none())
        });
        let mut tasks = Vec::new();
        let mut skipped = Vec::new();
        while self.index_place_free() {
            let Some(index) = self
                .index_queue()
                .into_iter()
                .find(|index| !skipped.contains(index))
            else {
                break;
            };
            let entry = &self.clouds[index];
            // A scan still being read is built once it has been checked, and
            // a file is never built twice at once.
            if entry.cloud.provisional
                || entry.cloud.points.is_empty()
                || self.indexing_path(&entry.cloud.path)
                || self.index_job_of(entry).is_some()
            {
                skipped.push(index);
                continue;
            }
            let requested = self.index_requested(entry);
            let entry = &mut self.clouds[index];
            entry.auto_index_queued = false;
            entry.index_building = true;
            let source = Arc::clone(&entry.cloud);
            let identity = Arc::clone(&entry.load_identity);
            self.index_requests.retain(|request| {
                !Arc::ptr_eq(request, &source) && !Arc::ptr_eq(request, &identity)
            });
            self.status = if requested {
                format!("Building disk octree for {} points…", source.total_points)
            } else {
                format!(
                    "Indexing {} points for viewport detail: {}",
                    source.total_points,
                    display_name(&source.path)
                )
            };
            tasks.push(self.start_index_job(source, !requested));
        }
        Task::batch(tasks)
    }

    /// Build the octree of a layer the user chose: at once when a place is
    /// free, otherwise first in the queue.
    pub(crate) fn request_index(&mut self, index: usize) -> Task<Message> {
        let Some(entry) = self.clouds.get(index) else {
            self.status = "Open a point cloud first".into();
            return Task::none();
        };
        if entry.index.is_some() {
            self.status = "An octree is already ready for this cloud".into();
            return Task::none();
        }
        if entry.index_building
            || self.index_job_of(entry).is_some()
            || self.indexing_path(&entry.cloud.path)
        {
            self.status = i18n::tr("The octree of this scan is already being built").to_owned();
            return Task::none();
        }
        let path = entry.cloud.path.clone();
        let name = display_name(&path).to_owned();
        let identity = Arc::clone(&entry.load_identity);
        self.index_declined.remove(&path);
        if !self.index_requested(&self.clouds[index]) {
            self.index_requests.push(identity);
        }
        self.clouds[index].auto_index_queued = false;
        let task = self.start_queued_indexes();
        let entry = &self.clouds[index];
        if entry.index_building {
            return task;
        }
        self.status = if entry.cloud.provisional || entry.cloud.points.is_empty() {
            i18n::tr_args(
                "{name} gets its octree once it has been read",
                &[("name", &name)],
            )
        } else {
            i18n::tr_args(
                "{name} waits for its octree: it starts when one of the {running} running builds ends",
                &[("name", &name), ("running", &self.index_jobs.len())],
            )
        };
        task
    }

    /// Stop every running build and empty the queue. The scans that are
    /// open or being opened are not indexed automatically again; Build index
    /// still builds the octree of one of them. A one-pass import that still
    /// reads its source reads on, and is told as an opening from then on.
    pub(crate) fn cancel_all_indexes(&mut self) -> (usize, usize) {
        let waiting = self.index_waiting();
        let reading = self
            .index_jobs
            .iter()
            .filter(|job| {
                job.import_id
                    .is_some_and(|id| self.imports.contains_key(&id))
                    && !job.cancelling()
            })
            .count();
        self.opening_total += reading;
        for job in &self.index_jobs {
            job.stop();
        }
        for entry in &mut self.clouds {
            entry.auto_index_queued = false;
            if entry.index.is_none() {
                self.index_declined.insert(entry.cloud.path.clone());
            }
        }
        self.index_declined
            .extend(self.imports.values().map(|job| job.path.clone()));
        self.index_requests.clear();
        (self.index_jobs.len(), waiting)
    }

    /// Take the build of a cloud out of the running ones when it ended.
    pub(crate) fn finish_index_job(&mut self, source: &Arc<PointCloud>) -> Option<IndexJob> {
        let position = self.index_jobs.iter().position(|job| {
            job.source
                .as_ref()
                .is_some_and(|built| Arc::ptr_eq(built, source))
        })?;
        self.index_finished += 1;
        Some(self.index_jobs.remove(position))
    }

    /// Take the build of a one-pass import out of the running ones.
    pub(crate) fn finish_index_import(&mut self, id: u64) -> Option<IndexJob> {
        let position = self
            .index_jobs
            .iter()
            .position(|job| job.import_id == Some(id))?;
        self.index_finished += 1;
        Some(self.index_jobs.remove(position))
    }

    /// Whether an import reads its source for an octree build in one pass,
    /// and that octree has not been cancelled.
    pub(crate) fn import_builds_index(&self, cancel: &Arc<AtomicBool>) -> bool {
        self.index_jobs
            .iter()
            .any(|job| Arc::ptr_eq(&job.cancel, cancel) && !job.stop_tree.load(Ordering::Relaxed))
    }

    /// Whether a build is a one-pass import that still reads its source
    /// after its octree was cancelled: only an opening now.
    pub(crate) fn only_reading(&self, job: &IndexJob) -> bool {
        job.import_id
            .is_some_and(|id| self.imports.contains_key(&id))
            && job.stop_tree.load(Ordering::Relaxed)
    }

    /// The builds that are to give an octree, in the order they started.
    pub(crate) fn tree_builds(&self) -> Vec<&IndexJob> {
        self.index_jobs
            .iter()
            .filter(|job| !self.only_reading(job))
            .collect()
    }

    /// The points an import is expected to read: what the metadata of its
    /// source states, or the total its one-pass build knows.
    pub(crate) fn expected_points(&self, id: u64) -> Option<u64> {
        self.import_expected.get(&id).copied().or_else(|| {
            self.index_jobs
                .iter()
                .find(|job| job.import_id == Some(id))
                .and_then(IndexJob::progress)
                .map(|progress| progress.total)
                .filter(|total| *total > 0)
        })
    }

    /// The running and waiting builds, for the status of the local API.
    pub(crate) fn index_value(&self) -> Value {
        json!({
            "builds": self.index_jobs.iter().map(IndexJob::value).collect::<Vec<_>>(),
            "waiting": self.index_waiting(),
            "at_once": self.index_limit,
        })
    }

    /// The progress of the first running build, as the status has given it
    /// since before builds ran side by side.
    pub(crate) fn first_index_progress_value(&self) -> Value {
        let Some(job) = self.index_jobs.first() else {
            return Value::Null;
        };
        let Some(progress) = job.progress() else {
            return Value::Null;
        };
        json!({
            "stage": stage_key(progress.stage),
            "completed": progress.completed,
            "total": progress.total,
            "depth": progress.depth,
            "leaves": progress.leaves,
            "settled": progress.settled,
            "fraction": progress.fraction(),
            "cancelling": job.cancelling(),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;

    use super::*;
    use crate::i18n::{Language, TestLanguage};
    use crate::native_api::{ApiCommand, ApiRequest};
    use crate::ImportJob;

    #[test]
    fn the_limit_follows_the_cores_and_the_memory() {
        const GB: u64 = 1024 * 1024 * 1024;
        assert_eq!(build_limit(1, None), 1);
        assert_eq!(build_limit(4, Some(16 * GB)), 1);
        assert_eq!(build_limit(8, Some(16 * GB)), 2);
        assert_eq!(build_limit(32, Some(16 * GB)), MAX_BUILDS);
        assert_eq!(build_limit(32, None), MAX_BUILDS);
        // Memory that is nearly used up leaves room for one build.
        assert_eq!(build_limit(32, Some(2 * GB + MEMORY_PER_BUILD)), 1);
        assert_eq!(build_limit(32, Some(GB)), 1);
        assert_eq!(build_limit(32, Some(2 * GB + 2 * MEMORY_PER_BUILD)), 2);
        assert!((1..=MAX_BUILDS).contains(&machine_build_limit()));
    }

    /// A checked scan in a temporary folder that counts as large enough to
    /// be indexed automatically.
    fn scan(directory: &Path, name: &str) -> Arc<PointCloud> {
        let path = directory.join(name);
        std::fs::write(&path, "0 0 1\n1 0 1\n2 0 1\n").unwrap();
        let mut cloud = pointcloud_core::open(&path, 10).unwrap();
        cloud.total_points = 2_000_000;
        Arc::new(cloud)
    }

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    /// A window with these scans open, every one waiting for its octree.
    fn studio_with(scans: &[Arc<PointCloud>], limit: usize) -> Studio {
        let mut studio = Studio {
            index_limit: limit,
            auto_index: true,
            ..Studio::default()
        };
        for cloud in scans {
            let _ = studio.update(Message::Loaded(Ok(Arc::clone(cloud))));
            studio.clouds.last_mut().unwrap().auto_index_queued = true;
        }
        studio.active = None;
        studio
    }

    fn building(studio: &Studio) -> Vec<String> {
        studio
            .index_jobs
            .iter()
            .map(|job| display_name(&job.path).to_owned())
            .collect()
    }

    #[test]
    fn builds_fill_the_places_in_the_order_of_the_queue() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        let scans: Vec<_> = ["a.xyz", "b.xyz", "c.xyz", "d.xyz", "e.xyz"]
            .into_iter()
            .map(|name| scan(directory.path(), name))
            .collect();
        let mut studio = studio_with(&scans, 2);
        // The active layer first, then the others in the order of the list.
        studio.active = Some(3);
        assert_eq!(studio.index_queue(), [3, 0, 1, 2, 4]);
        let _ = studio.start_queued_indexes();
        assert_eq!(building(&studio), ["d.xyz", "a.xyz"]);
        assert!(studio.clouds[3].index_building && studio.clouds[0].index_building);
        assert_eq!(studio.index_waiting(), 3);
        // A full set of places starts nothing more.
        let _ = studio.start_queued_indexes();
        assert_eq!(studio.index_jobs.len(), 2);

        // The user asks for the last scan: it goes before the queue.
        let _ = studio.request_index(4);
        assert_eq!(studio.index_queue(), [4, 1, 2]);
        assert!(
            studio.status.contains("waits for its octree"),
            "{}",
            studio.status
        );
        // Asking again changes nothing, and a running build is not doubled.
        let _ = studio.request_index(4);
        let _ = studio.request_index(3);
        assert_eq!(studio.index_queue(), [4, 1, 2]);
        assert_eq!(studio.index_jobs.len(), 2);

        // A build that ends gives its place to the next in the queue.
        let _ = studio.update(Message::AutoIndexReady(
            Arc::clone(&scans[3]),
            Err("damaged".into()),
        ));
        assert_eq!(building(&studio), ["a.xyz", "e.xyz"]);
        assert_eq!(studio.index_finished, 1);
        let _ = studio.update(Message::AutoIndexReady(
            Arc::clone(&scans[0]),
            Err("damaged".into()),
        ));
        let _ = studio.update(Message::IndexReady(
            Arc::clone(&scans[4]),
            Err("damaged".into()),
        ));
        assert_eq!(building(&studio), ["b.xyz", "c.xyz"]);
        assert_eq!(studio.index_waiting(), 0);
        let _ = studio.update(Message::AutoIndexReady(
            Arc::clone(&scans[1]),
            Err("damaged".into()),
        ));
        let _ = studio.update(Message::AutoIndexReady(
            Arc::clone(&scans[2]),
            Err("damaged".into()),
        ));
        assert!(!studio.index_pending());
        // A finished batch starts its count again.
        studio.track_progress();
        assert_eq!(studio.index_finished, 0);
    }

    #[test]
    fn one_file_is_never_built_twice_at_once() {
        let directory = tempfile::tempdir().unwrap();
        let first = scan(directory.path(), "same.xyz");
        let other = scan(directory.path(), "other.xyz");
        // Two layers of the same file, as a reopened scan gives.
        let twin = Arc::new((*first).clone());
        let mut studio = studio_with(&[Arc::clone(&first), twin, other], 3);
        let _ = studio.start_queued_indexes();
        assert_eq!(building(&studio), ["same.xyz", "other.xyz"]);
        assert_eq!(studio.index_waiting(), 1);
        let _ = studio.request_index(1);
        assert_eq!(studio.index_jobs.len(), 2);
        // Once the first build ends, the twin gets its turn.
        let _ = studio.update(Message::AutoIndexReady(first, Err("damaged".into())));
        assert_eq!(building(&studio), ["other.xyz", "same.xyz"]);
        assert!(studio.clouds[1].index_building);
    }

    #[test]
    fn cancel_stops_every_build_and_empties_the_queue() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        let scans: Vec<_> = ["a.xyz", "b.xyz", "c.xyz", "d.xyz"]
            .into_iter()
            .map(|name| scan(directory.path(), name))
            .collect();
        let mut studio = studio_with(&scans, 2);
        let _ = studio.start_queued_indexes();
        // A one-pass import reads a further scan.
        let import_cancel = Arc::new(AtomicBool::new(false));
        studio.imports.insert(
            9,
            ImportJob {
                path: directory.path().join("e.xyz"),
                decoded: Arc::new(AtomicU64::new(0)),
                cancel: Arc::clone(&import_cancel),
            },
        );
        studio.index_jobs.push(IndexJob {
            source: None,
            import_id: Some(9),
            path: directory.path().join("e.xyz"),
            progress: Arc::new(Mutex::new(IndexProgress {
                stage: IndexStage::ReadingSource,
                completed: 0,
                total: 0,
                depth: 0,
                leaves: 0,
                settled: 0,
            })),
            cancel: Arc::clone(&import_cancel),
            stop_tree: Arc::new(AtomicBool::new(false)),
        });
        assert_eq!(studio.index_waiting(), 2);
        assert!(studio.import_builds_index(&import_cancel));

        let response = send(&mut studio, ApiCommand::CancelIndex);
        assert_eq!(response["ok"], true);
        assert_eq!(response["cancelled"], 3);
        assert_eq!(response["dequeued"], 2);
        assert!(studio.index_jobs.iter().all(IndexJob::cancelling));
        // The import reads on and keeps its checked cloud.
        assert!(!import_cancel.load(Ordering::Relaxed));
        assert!(studio.index_jobs[2].stop_tree.load(Ordering::Relaxed));
        assert_eq!(studio.index_waiting(), 0);
        assert!(studio.clouds.iter().all(|entry| !entry.auto_index_queued));

        // Builds that end start nothing, and the scans are not queued again
        // when their cached octree is looked for.
        let _ = studio.update(Message::AutoIndexReady(
            Arc::clone(&scans[0]),
            Err("cancelled".into()),
        ));
        assert_eq!(studio.index_jobs.len(), 2);
        let _ = studio.update(Message::CachedIndexReady(Arc::clone(&scans[2]), Ok(None)));
        assert!(!studio.clouds[2].auto_index_queued);
        assert_eq!(studio.status, "Octree build cancelled");
        // Build index still builds one of them.
        studio.active = Some(3);
        let _ = studio.update(Message::BuildIndex);
        assert!(studio.index_requested(&studio.clouds[3]));
        let _ = studio.update(Message::AutoIndexReady(
            Arc::clone(&scans[1]),
            Err("cancelled".into()),
        ));
        assert!(studio.clouds[3].index_building);
        // Switching automatic indexing on again forgets the cancellation.
        let _ = studio.update(Message::SetAutoIndex(true));
        assert!(studio.index_declined.is_empty());
    }

    #[test]
    fn the_api_reports_running_and_waiting_builds() {
        let directory = tempfile::tempdir().unwrap();
        let scans: Vec<_> = ["a.xyz", "b.xyz", "c.xyz"]
            .into_iter()
            .map(|name| scan(directory.path(), name))
            .collect();
        let mut studio = studio_with(&scans, 2);
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["index_progress"], Value::Null);
        assert_eq!(status["result"]["index"]["waiting"], 3);
        assert_eq!(status["result"]["index"]["at_once"], 2);
        let _ = studio.start_queued_indexes();
        let status = send(&mut studio, ApiCommand::Status);
        let builds = status["result"]["index"]["builds"].as_array().unwrap();
        assert_eq!(builds.len(), 2);
        assert_eq!(builds[0]["stage"], "reading_source");
        assert_eq!(builds[0]["total"], 2_000_000);
        assert_eq!(builds[0]["cancelling"], false);
        assert_eq!(status["result"]["index"]["waiting"], 1);
        assert_eq!(
            status["result"]["index_progress"]["stage"],
            "reading_source"
        );

        // Building the octree of a layer that is being built is refused; of
        // one that waits, it moves the layer to the front.
        studio.active = Some(0);
        let refused = send(&mut studio, ApiCommand::BuildIndex);
        assert_eq!(refused["ok"], false);
        studio.active = Some(2);
        studio.clouds[2].auto_index_queued = false;
        let queued = send(&mut studio, ApiCommand::BuildIndex);
        assert_eq!(queued["ok"], true);
        assert_eq!(queued["queued"], true);
        assert_eq!(studio.index_queue(), [2]);
    }
}
