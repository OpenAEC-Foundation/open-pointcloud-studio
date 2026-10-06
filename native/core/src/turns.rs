//! Large sources are read one after another on each disk, and their octrees
//! are built one after another, in the order the sources were opened.
//!
//! A source read alone has been read sooner, so it is shown in full and its
//! octree is started sooner, and a disk serves one sequential read better
//! than several that make it seek between them. An octree of a large source
//! keeps most cores and the disk of the index busy, so a second one built
//! beside it makes both late; built one after the other, the first is ready
//! long before the second, while the next source is being read. A source
//! takes its places when it is opened, waits for the places taken before it
//! and gives each up when it is done with it, or when it is dropped.

use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, Prefix};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use super::{LoadError, LARGE_SOURCE_BYTES};

/// How often a source that waits asks whether it is still wanted.
const ASK_INTERVAL: Duration = Duration::from_millis(100);
/// The queue of the builds of large octrees.
const BUILDS: &str = "octree builds";

/// The places of every queue, in the order they were taken, by queue.
type Queues = HashMap<String, VecDeque<u64>>;

/// Every queue, and what wakes the sources that wait in them.
static QUEUES: LazyLock<(Mutex<Queues>, Condvar)> =
    LazyLock::new(|| (Mutex::new(HashMap::new()), Condvar::new()));
static NEXT_TICKET: AtomicU64 = AtomicU64::new(0);

fn queues() -> MutexGuard<'static, Queues> {
    QUEUES.0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A place in one queue. Dropping it gives the place up.
#[derive(Debug)]
struct Place {
    queue: String,
    ticket: u64,
}

impl Place {
    fn take(queue: String) -> Self {
        let ticket = NEXT_TICKET.fetch_add(1, Ordering::Relaxed);
        queues().entry(queue.clone()).or_default().push_back(ticket);
        Self { queue, ticket }
    }

    fn first_in(&self, queues: &Queues) -> bool {
        queues
            .get(&self.queue)
            .is_none_or(|queue| queue.front() == Some(&self.ticket))
    }

    /// Wait until the places taken before this one have been given up.
    /// `wanted` is asked a few times a second meanwhile, and an error from it
    /// ends the wait with that error.
    fn wait(&self, mut wanted: impl FnMut() -> Result<(), LoadError>) -> Result<(), LoadError> {
        loop {
            {
                let queues = queues();
                if self.first_in(&queues) {
                    return Ok(());
                }
                let (queues, _) = QUEUES
                    .1
                    .wait_timeout(queues, ASK_INTERVAL)
                    .unwrap_or_else(PoisonError::into_inner);
                if self.first_in(&queues) {
                    return Ok(());
                }
            }
            wanted()?;
        }
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut queues = queues();
        if let Some(queue) = queues.get_mut(&self.queue) {
            queue.retain(|ticket| *ticket != self.ticket);
            if queue.is_empty() {
                queues.remove(&self.queue);
            }
        }
        drop(queues);
        QUEUES.1.notify_all();
    }
}

/// The places of a large source that is being opened: among the reads of its
/// disk, and for a source whose octree is built in the same pass, among the
/// builds of large octrees.
#[derive(Debug)]
pub struct SourceTurn {
    read: Option<Place>,
    build: Option<Place>,
}

impl SourceTurn {
    /// The places a source takes when it is opened: `None` for a source
    /// smaller than `LARGE_SOURCE_BYTES`, which is read and indexed beside
    /// the others as soon as it is opened. With `build`, the source also
    /// takes a place among the builds of large octrees.
    pub fn for_source(path: impl AsRef<Path>, build: bool) -> Option<Self> {
        let path = path.as_ref();
        let length = std::fs::metadata(path).ok()?.len();
        (length >= LARGE_SOURCE_BYTES).then(|| Self::on(disk_of(path), build.then_some(BUILDS)))
    }

    fn on(disk: String, builds: Option<&str>) -> Self {
        Self {
            read: Some(Place::take(format!("read {disk}"))),
            build: builds.map(|queue| Place::take(queue.to_owned())),
        }
    }

    /// Whether the sources that took a place among the reads of this disk
    /// before this one have all been read.
    pub fn is_due(&self) -> bool {
        self.read
            .as_ref()
            .is_none_or(|place| place.first_in(&queues()))
    }

    /// Wait for the turn to read the source. `wanted` is asked a few times a
    /// second meanwhile, and an error from it ends the wait.
    pub(crate) fn wait_to_read(
        &self,
        wanted: impl FnMut() -> Result<(), LoadError>,
    ) -> Result<(), LoadError> {
        match &self.read {
            Some(place) => place.wait(wanted),
            None => Ok(()),
        }
    }

    /// The source has been read: the next source of its disk may be read.
    pub(crate) fn end_read(&mut self) {
        self.read = None;
    }

    /// Wait for the turn to build the octree of the source, as
    /// `wait_to_read` waits.
    pub(crate) fn wait_to_build(
        &self,
        wanted: impl FnMut() -> Result<(), LoadError>,
    ) -> Result<(), LoadError> {
        match &self.build {
            Some(place) => place.wait(wanted),
            None => Ok(()),
        }
    }
}

/// What tells the disk of a source: its drive or network share on Windows,
/// its device elsewhere. A source that cannot be looked at shares the queue
/// of the sources without a drive.
fn disk_of(path: &Path) -> String {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // A file that is not there yet would be on the disk of its folder.
        if let Some(metadata) = path
            .ancestors()
            .find_map(|folder| std::fs::metadata(folder).ok())
        {
            return format!("device {}", metadata.dev());
        }
    }
    match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                format!("{}:", char::from(letter).to_ascii_uppercase())
            }
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => format!(
                r"\\{}\{}",
                server.to_string_lossy().to_lowercase(),
                share.to_string_lossy().to_lowercase()
            ),
            _ => prefix.as_os_str().to_string_lossy().to_lowercase(),
        },
        _ => String::new(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    /// A queue of its own, so that tests running side by side do not wait
    /// for one another.
    fn own_queue(name: &str) -> String {
        format!("test {name} {}", uuid::Uuid::new_v4())
    }

    /// The places of a source on a disk of its own, and with `builds` in a
    /// queue of builds of its own.
    pub(crate) fn turn_on(disk: &str, builds: Option<&str>) -> SourceTurn {
        SourceTurn::on(disk.to_owned(), builds)
    }

    #[test]
    fn reads_of_one_disk_take_their_turns_in_order() {
        let disk = own_queue("order");
        let first = turn_on(&disk, None);
        let second = turn_on(&disk, None);
        let third = turn_on(&disk, None);
        // Another disk is read meanwhile.
        let elsewhere = turn_on(&own_queue("elsewhere"), None);
        assert!(first.is_due() && elsewhere.is_due());
        assert!(!second.is_due() && !third.is_due());

        let (sender, receiver) = mpsc::channel();
        let waiting = thread::spawn(move || {
            third.wait_to_read(|| Ok(())).unwrap();
            sender.send("third").unwrap();
            third
        });
        // The third waits for the second as well as the first.
        drop(first);
        assert!(second.is_due());
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        second.wait_to_read(|| Ok(())).unwrap();
        drop(second);
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5)).unwrap(),
            "third"
        );
        let third = waiting.join().unwrap();
        assert!(third.is_due());
        drop(third);
        assert!(
            !queues().contains_key(&format!("read {disk}")),
            "an empty queue is let go"
        );
    }

    #[test]
    fn a_source_that_is_no_longer_wanted_gives_up_its_place() {
        let disk = own_queue("cancel");
        let first = turn_on(&disk, None);
        let second = turn_on(&disk, None);
        let third = turn_on(&disk, None);
        // The second is asked a few times a second whether it is still
        // wanted, and stops waiting with the answer.
        let mut asked = 0;
        let stopped = second.wait_to_read(|| {
            asked += 1;
            if asked == 3 {
                Err(LoadError::Cancelled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
        assert_eq!(asked, 3);
        drop(second);
        // The third now waits for the first alone.
        assert!(!third.is_due());
        drop(first);
        assert!(third.is_due());
    }

    #[test]
    fn the_next_source_is_read_while_the_octree_before_it_is_built() {
        let disk = own_queue("pipeline");
        let builds = own_queue("builds");
        let mut first = turn_on(&disk, Some(&builds));
        let mut second = turn_on(&disk, Some(&builds));
        // A smaller source takes no place: it is read and indexed at once.
        let small = turn_on(&own_queue("small"), None);
        small.wait_to_build(|| unreachable!()).unwrap();

        first.wait_to_read(|| unreachable!()).unwrap();
        first.wait_to_build(|| unreachable!()).unwrap();
        assert!(!second.is_due());
        // The first has been read: the second is read while the first
        // octree is built, and its own octree waits for that one.
        first.end_read();
        assert!(second.is_due());
        second.wait_to_read(|| unreachable!()).unwrap();
        second.end_read();
        let mut asked = 0;
        let waited = second.wait_to_build(|| {
            asked += 1;
            Err(LoadError::Cancelled)
        });
        assert!(matches!(waited, Err(LoadError::Cancelled)));
        assert_eq!(asked, 1);
        drop(first);
        second.wait_to_build(|| unreachable!()).unwrap();
    }

    #[test]
    fn only_large_sources_take_turns() {
        let directory = tempfile::tempdir().unwrap();
        let small = directory.path().join("small.e57");
        std::fs::write(&small, b"e57").unwrap();
        assert!(SourceTurn::for_source(&small, true).is_none());
        assert!(SourceTurn::for_source(directory.path().join("gone.e57"), false).is_none());
        let large = directory.path().join("large.e57");
        let file = std::fs::File::create(&large).unwrap();
        // A sparse file: its length is all that is looked at.
        file.set_len(LARGE_SOURCE_BYTES).unwrap();
        drop(file);
        let turn = SourceTurn::for_source(&large, false).unwrap();
        assert!(turn.read.is_some() && turn.build.is_none());
        drop(turn);
        let turn = SourceTurn::for_source(&large, true).unwrap();
        assert!(turn.read.is_some() && turn.build.is_some());
    }

    #[test]
    fn a_disk_is_its_drive_or_share() {
        let directory = tempfile::tempdir().unwrap();
        let one = directory.path().join("one.e57");
        let two = directory.path().join("two.e57");
        std::fs::write(&one, b"").unwrap();
        std::fs::write(&two, b"").unwrap();
        assert_eq!(disk_of(&one), disk_of(&two));
        // A missing file shares the disk it would be on.
        assert_eq!(disk_of(&directory.path().join("gone.e57")), disk_of(&one));
        #[cfg(windows)]
        {
            assert_eq!(disk_of(Path::new(r"q:\scans\missing.e57")), "Q:");
            assert_eq!(disk_of(Path::new(r"\\?\Q:\scans\missing.e57")), "Q:");
            assert_eq!(
                disk_of(Path::new(r"\\Server\Scans\missing.e57")),
                r"\\server\scans"
            );
            assert_ne!(
                disk_of(Path::new(r"\\server\other\missing.e57")),
                r"\\server\scans"
            );
        }
    }
}
