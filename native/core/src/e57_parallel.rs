//! Every point of a large E57 scan, decoded on several threads while the
//! points reach the caller one by one and in file order.
//!
//! This works for the scans a spread preview can sample (`e57_quick`): every
//! record field fills whole bytes and every packet but the last holds the
//! same number of records, so a run of packets can be decoded without the
//! packets before it. Each thread reads runs of packets with a reader of its
//! own and decodes them with the decoder of a pass on one thread, which sees
//! the run as a section by itself: the points and their order are the same.
//! A packet that breaks the layout hands the scan to that one-thread pass,
//! which skips the points already given.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Condvar, Mutex};

use e57::E57Reader;

use super::e57_points;
use super::e57_quick::{self, Layout, Source};
use super::window_reader::WindowReader;
use super::{LoadError, Point};

/// Records decoded as one run: few enough to keep the threads busy and the
/// memory small, many enough that setting up a run costs little.
const RUN_RECORDS: u64 = 65_536;
/// Scans with fewer records are read on one thread.
const PARALLEL_MIN_RECORDS: u64 = 1 << 20;
/// Runs a thread may decode ahead of the one the caller is given.
const AHEAD_PER_THREAD: u64 = 4;
/// Most threads that decode one scan. A further one gains little: the
/// caller takes the points one by one.
const MAX_THREADS: usize = 8;
const CHECKSUM_BYTES: u64 = 4;
const SECTION_HEADER_BYTES: usize = 32;

/// How a scan is read on several threads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Parallel {
    pub threads: usize,
    pub min_records: u64,
    pub run_records: u64,
}

impl Parallel {
    /// Half the logical cores, at most `MAX_THREADS`: the rest is left to
    /// the caller, to the octree an import builds of the scan read before,
    /// and to the window.
    pub(crate) fn machine() -> Self {
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        Self {
            threads: (cores / 2).clamp(1, MAX_THREADS),
            min_records: PARALLEL_MIN_RECORDS,
            run_records: RUN_RECORDS,
        }
    }
}

/// Stream the valid points of one scan of `file`, which is `path` opened,
/// on several threads when the scan allows it and on one otherwise.
pub(crate) fn read_scan(
    path: &Path,
    file: &mut E57Reader<WindowReader>,
    scan: &e57::PointCloud,
    parallel: Parallel,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let layout = if parallel.threads > 1 && scan.records >= parallel.min_records.max(1) {
        layout(path, scan)?
    } else {
        None
    };
    let Some((layout, page)) = layout else {
        return e57_points::read_scan(file, scan, push);
    };
    let mut given = 0u64;
    let finished = decode(path, scan, &layout, page, parallel, &mut |point| {
        given += 1;
        push(point)
    })?;
    if finished {
        return Ok(());
    }
    // The one-thread pass decodes the scan from its start; the points given
    // already are the first it meets.
    e57_points::read_scan(file, scan, &mut |point| {
        if given > 0 {
            given -= 1;
            return Ok(());
        }
        push(point)
    })
}

/// Whether the points of an E57 file are decoded on several threads: every
/// scan that holds records is laid out for it, which the picture spread
/// through the whole file needs as well, and most records lie in scans large
/// enough to be read that way. A file that cannot be read is not.
pub(crate) fn decoded_in_parallel(path: &Path, parallel: Parallel) -> bool {
    if parallel.threads <= 1 {
        return false;
    }
    let Ok(file) = e57_points::open_reader(path) else {
        return false;
    };
    let (mut total, mut in_runs) = (0u64, 0u64);
    for scan in file.pointclouds() {
        if scan.records == 0 {
            continue;
        }
        if !matches!(layout(path, &scan), Ok(Some(_))) {
            return false;
        }
        total += scan.records;
        if scan.records >= parallel.min_records.max(1) {
            in_runs += scan.records;
        }
    }
    total > 0 && in_runs >= total - in_runs
}

fn layout(path: &Path, scan: &e57::PointCloud) -> Result<Option<(Layout, u64)>, LoadError> {
    let Some(page) = e57_quick::page_size(path)? else {
        return Ok(None);
    };
    let mut source = Source::open(path, page)?;
    if source.length % page != 0 {
        return Ok(None);
    }
    Ok(Layout::find(&mut source, scan)?.map(|layout| (layout, page)))
}

/// What a thread made of one run of packets.
enum Run {
    Points(Vec<Point>),
    /// A packet of the run is not laid out like the first of the scan.
    Unlike,
}

/// Where the threads are: the next run to take, and how far the caller is.
struct Progress {
    next: AtomicU64,
    stop: AtomicBool,
    given: Mutex<u64>,
    moved: Condvar,
}

impl Progress {
    fn halt(&self) {
        self.stop.store(true, Ordering::Release);
        let _guard = self.given.lock();
        self.moved.notify_all();
    }
}

/// Decode the scan in runs on several threads and give the points in file
/// order. Returns `false` when a run broke the layout: the points of the
/// runs before it have been given, and no point after them.
fn decode(
    path: &Path,
    scan: &e57::PointCloud,
    layout: &Layout,
    page: u64,
    parallel: Parallel,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<bool, LoadError> {
    let per_run = (parallel.run_records / layout.records as u64).max(1);
    let runs = layout.packets.div_ceil(per_run);
    let threads = parallel.threads.min(runs as usize).max(1);
    let ahead = AHEAD_PER_THREAD * threads as u64;
    let progress = Progress {
        next: AtomicU64::new(0),
        stop: AtomicBool::new(false),
        given: Mutex::new(0),
        moved: Condvar::new(),
    };
    let (sender, receiver) = mpsc::channel::<(u64, Result<Run, LoadError>)>();
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let sender = sender.clone();
            let progress = &progress;
            scope.spawn(move || {
                let mut decoder: Option<Decoder> = None;
                loop {
                    if progress.stop.load(Ordering::Acquire) {
                        return;
                    }
                    let run = progress.next.fetch_add(1, Ordering::AcqRel);
                    if run >= runs {
                        return;
                    }
                    {
                        let Ok(mut given) = progress.given.lock() else {
                            return;
                        };
                        while run >= *given + ahead && !progress.stop.load(Ordering::Acquire) {
                            given = match progress.moved.wait(given) {
                                Ok(given) => given,
                                Err(_) => return,
                            };
                        }
                    }
                    if progress.stop.load(Ordering::Acquire) {
                        return;
                    }
                    let first = run * per_run;
                    let last = (first + per_run).min(layout.packets);
                    let result = match &mut decoder {
                        Some(decoder) => decoder.run(layout, first..last),
                        None => match Decoder::open(path, page, scan) {
                            Ok(opened) => decoder.insert(opened).run(layout, first..last),
                            Err(error) => Err(error),
                        },
                    };
                    if sender.send((run, result)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(sender);
        let result = give(&receiver, runs, &progress, push);
        progress.halt();
        result
    })
}

/// Give the points of the runs in their order as the threads send them.
fn give(
    receiver: &mpsc::Receiver<(u64, Result<Run, LoadError>)>,
    runs: u64,
    progress: &Progress,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<bool, LoadError> {
    let mut waiting = BTreeMap::new();
    let mut expected = 0u64;
    while expected < runs {
        let Ok((run, result)) = receiver.recv() else {
            return Err(LoadError::InvalidData(
                "a thread reading the scan stopped".into(),
            ));
        };
        waiting.insert(run, result);
        while let Some(result) = waiting.remove(&expected) {
            match result? {
                Run::Points(points) => {
                    for point in points {
                        push(point)?;
                    }
                }
                Run::Unlike => return Ok(false),
            }
            expected += 1;
            if let Ok(mut given) = progress.given.lock() {
                *given = expected;
            }
            progress.moved.notify_all();
        }
    }
    Ok(true)
}

/// The bytes a thread's decoder reads: the file, except for the pages of
/// the run it decodes, which were read already, and one page after the end
/// of the file that holds a section header of the run alone.
struct Overlay {
    file: File,
    length: u64,
    position: u64,
    run: Rc<RefCell<RunPages>>,
}

struct RunPages {
    start: u64,
    pages: Vec<u8>,
    section: Vec<u8>,
}

impl Read for Overlay {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let run = self.run.borrow();
        let (source, at): (&[u8], u64) = if self.position >= self.length {
            (&run.section, self.position - self.length)
        } else if self.position >= run.start && self.position < run.start + run.pages.len() as u64 {
            (&run.pages, self.position - run.start)
        } else {
            drop(run);
            // Anything else, such as the header and the XML when the
            // decoder opens, comes from the file itself.
            let before_end = (self.length - self.position).min(buffer.len() as u64) as usize;
            self.file.seek(SeekFrom::Start(self.position))?;
            let count = self.file.read(&mut buffer[..before_end])?;
            self.position += count as u64;
            return Ok(count);
        };
        let at = at as usize;
        if at >= source.len() {
            return Ok(0);
        }
        let count = buffer.len().min(source.len() - at);
        buffer[..count].copy_from_slice(&source[at..at + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for Overlay {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let end = self.length + self.run.borrow().section.len() as u64;
        let position = match to {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(delta) => end.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.position.checked_add_signed(delta),
        };
        self.position = position
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek before the start"))?;
        Ok(self.position)
    }
}

/// One thread's reader of the scan.
struct Decoder {
    source: Source,
    image: E57Reader<Overlay>,
    run: Rc<RefCell<RunPages>>,
    scan: e57::PointCloud,
    bytes: Vec<u8>,
}

impl Decoder {
    fn open(path: &Path, page: u64, scan: &e57::PointCloud) -> Result<Self, LoadError> {
        let source = Source::open(path, page)?;
        let run = Rc::new(RefCell::new(RunPages {
            start: 0,
            pages: Vec::new(),
            section: vec![0; page as usize],
        }));
        let image = E57Reader::new(Overlay {
            file: File::open(path)?,
            length: source.length,
            position: 0,
            run: Rc::clone(&run),
        })?;
        let mut scan = scan.clone();
        // The section header of a run lies on the page after the file.
        scan.file_offset = source.length;
        Ok(Self {
            source,
            image,
            run,
            scan,
            bytes: Vec::new(),
        })
    }

    /// Read, check and decode the packets `packets` of the scan.
    fn run(&mut self, layout: &Layout, packets: std::ops::Range<u64>) -> Result<Run, LoadError> {
        let start = layout.packet_start(packets.start);
        let end = if packets.end == layout.packets {
            layout.end
        } else {
            layout.packet_start(packets.end)
        };
        self.source
            .read(start, (end - start) as usize, &mut self.bytes)?;
        let mut records = 0u64;
        let mut at = 0usize;
        for index in packets {
            let Some((length, count)) = layout.packet(index, &self.bytes[at..]) else {
                return Ok(Run::Unlike);
            };
            at += length;
            records += count as u64;
        }
        if at != self.bytes.len() {
            return Ok(Run::Unlike);
        }
        let page = self.source.page;
        {
            let mut run = self.run.borrow_mut();
            let run = &mut *run;
            std::mem::swap(&mut run.pages, &mut self.source.raw);
            run.start = self.source.raw_start;
            run.section.fill(0);
            run.section[0] = 1;
            let length = (SECTION_HEADER_BYTES as u64 + (end - start)).next_multiple_of(4);
            run.section[8..16].copy_from_slice(&length.to_le_bytes());
            let data = e57_quick::physical_offset(start, page);
            run.section[16..24].copy_from_slice(&data.to_le_bytes());
            let content = (page - CHECKSUM_BYTES) as usize;
            let checksum = crc32c::crc32c(&run.section[..content]).to_be_bytes();
            run.section[content..].copy_from_slice(&checksum);
        }
        self.scan.records = records;
        let mut points = Vec::with_capacity(records as usize);
        e57_points::read_scan(&mut self.image, &self.scan, &mut |point| {
            points.push(point);
            Ok(())
        })?;
        Ok(Run::Points(points))
    }
}

#[cfg(test)]
mod tests {
    use e57::{
        E57Writer, Quaternion, Record, RecordDataType, RecordName, RecordValue, Transform,
        Translation,
    };

    use super::*;

    /// The parallel reading the tests ask for: many threads and small runs,
    /// so that a small scan has many runs that finish out of order.
    const TEST: Parallel = Parallel {
        threads: 6,
        min_records: 1,
        run_records: 5_000,
    };

    /// A scan of byte-wide fields with a pose, optionally with a further
    /// scan whose row index of ten bits keeps it on one thread.
    fn write_scans(path: &Path, counts: &[(usize, bool)]) {
        let mut writer =
            E57Writer::from_file(path, "{00000000-0000-4000-8000-000000000041}").unwrap();
        for (number, (count, row_index)) in counts.iter().enumerate() {
            let mut prototype = vec![
                Record::CARTESIAN_X_F64,
                Record::CARTESIAN_Y_F64,
                Record::CARTESIAN_Z_F64,
                Record::COLOR_RED_U8,
                Record::COLOR_GREEN_U8,
                Record::COLOR_BLUE_U8,
                Record::INTENSITY_U16,
            ];
            if *row_index {
                prototype.push(Record {
                    name: RecordName::RowIndex,
                    data_type: RecordDataType::Integer { min: 0, max: 1000 },
                });
            }
            let guid = format!("{{00000000-0000-4000-8000-00000000005{number}}}");
            let mut scan = writer.add_pointcloud(&guid, prototype).unwrap();
            scan.set_transform(Some(Transform {
                rotation: Quaternion {
                    w: 0.6,
                    x: 0.0,
                    y: 0.0,
                    z: 0.8,
                },
                translation: Translation {
                    x: 10.0 * number as f64,
                    y: 20.0,
                    z: 3.0,
                },
            }));
            for index in 0..*count {
                let mut values = vec![
                    RecordValue::Double(index as f64 * 0.01),
                    RecordValue::Double((index % 97) as f64),
                    RecordValue::Double((index / 97) as f64 * 0.5),
                    RecordValue::Integer((index % 256) as i64),
                    RecordValue::Integer((index / 3 % 256) as i64),
                    RecordValue::Integer((index / 7 % 256) as i64),
                    RecordValue::Integer((index % 65_536) as i64),
                ];
                if *row_index {
                    values.push(RecordValue::Integer((index % 1000) as i64));
                }
                scan.add_point(values).unwrap();
            }
            scan.finalize().unwrap();
        }
        writer.finalize().unwrap();
    }

    fn read_all(path: &Path, parallel: Parallel) -> Result<Vec<Point>, LoadError> {
        let mut file = e57_points::open_reader(path)?;
        let mut points = Vec::new();
        for scan in file.pointclouds() {
            read_scan(path, &mut file, &scan, parallel, &mut |point| {
                points.push(point);
                Ok(())
            })?;
        }
        Ok(points)
    }

    fn same(read: &[Point], expected: &[Point]) {
        assert_eq!(read.len(), expected.len());
        for (index, (read, expected)) in read.iter().zip(expected).enumerate() {
            assert!(
                read.xyz == expected.xyz
                    && read.rgb == expected.rgb
                    && read.intensity == expected.intensity
                    && read.classification == expected.classification,
                "point {index}: {read:?} instead of {expected:?}"
            );
        }
    }

    /// The points as the decoder gives them on one thread.
    fn one_thread(path: &Path) -> Vec<Point> {
        let mut file = e57_points::open_reader(path).unwrap();
        let mut points = Vec::new();
        for scan in file.pointclouds() {
            e57_points::read_scan(&mut file, &scan, &mut |point| {
                points.push(point);
                Ok(())
            })
            .unwrap();
        }
        points
    }

    /// Overwrite bytes at a logical offset, keeping the page checksums right.
    fn patch(path: &Path, at: u64, bytes: &[u8]) {
        let mut file = std::fs::read(path).unwrap();
        for (offset, byte) in (at..).zip(bytes) {
            file[(offset / 1020 * 1024 + offset % 1020) as usize] = *byte;
        }
        for page in file.as_chunks_mut::<1024>().0 {
            let checksum = crc32c::crc32c(&page[..1020]).to_be_bytes();
            page[1020..].copy_from_slice(&checksum);
        }
        std::fs::write(path, file).unwrap();
    }

    fn scan_layout(path: &Path, number: usize) -> Layout {
        let scan = e57_points::open_reader(path).unwrap().pointclouds()[number].clone();
        layout(path, &scan).unwrap().unwrap().0
    }

    #[test]
    fn a_file_is_decoded_in_parallel_when_its_scans_are_laid_out_for_it() {
        let directory = tempfile::tempdir().unwrap();
        let write = |name: &str, counts: &[(usize, bool)]| {
            let path = directory.path().join(name);
            write_scans(&path, counts);
            path
        };
        let laid_out = write("laid-out.e57", &[(20_000, false), (10_000, false)]);
        assert!(decoded_in_parallel(&laid_out, TEST));
        // Not on one thread, nor when most records lie in scans too small
        // for it.
        assert!(!decoded_in_parallel(
            &laid_out,
            Parallel { threads: 1, ..TEST }
        ));
        let large = Parallel {
            min_records: 15_000,
            ..TEST
        };
        assert!(decoded_in_parallel(&laid_out, large));
        let small = write("small-scans.e57", &[(10_000, false), (12_000, false)]);
        assert!(!decoded_in_parallel(&small, large));
        // A scan with a packed row index has no such layout, and keeps the
        // whole file from it.
        let packed = write("packed.e57", &[(20_000, false), (500, true)]);
        assert!(!decoded_in_parallel(&packed, TEST));
        // Nor is a file that is not an E57 scan.
        let other = directory.path().join("other.e57");
        std::fs::write(&other, b"not a scan").unwrap();
        assert!(!decoded_in_parallel(&other, TEST));
    }

    #[test]
    fn threads_give_the_points_of_one_thread_in_the_same_order() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("scans.e57");
        // A scan read in runs, one kept on one thread, and one more in runs.
        write_scans(
            &source,
            &[(150_000, false), (30_000, true), (70_001, false)],
        );
        let expected = one_thread(&source);
        assert_eq!(expected.len(), 250_001);
        let layout = scan_layout(&source, 0);
        assert!(layout.packets > 20, "{} packets", layout.packets);
        same(&read_all(&source, TEST).unwrap(), &expected);
        // So does a single thread, and so do runs of one packet.
        let single = Parallel { threads: 1, ..TEST };
        same(&read_all(&source, single).unwrap(), &expected);
        let packets = Parallel {
            run_records: 1,
            ..TEST
        };
        same(&read_all(&source, packets).unwrap(), &expected);
    }

    #[test]
    fn a_packet_unlike_the_first_hands_the_scan_to_one_thread() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("shifted.e57");
        write_scans(&source, &[(150_000, false)]);
        let layout = scan_layout(&source, 0);
        // The twentieth packet holds one more X and one fewer Z value, and
        // the next one gives them back: one thread decodes both, but the
        // streams are out of step where the second begins.
        let stream = (layout.records * 8) as u16;
        for (packet, shift) in [(19, 8i16), (20, -8)] {
            let header = layout.packet_start(packet) + 6;
            patch(
                &source,
                header,
                &stream.wrapping_add_signed(shift).to_le_bytes(),
            );
            patch(
                &source,
                header + 4,
                &stream.wrapping_add_signed(-shift).to_le_bytes(),
            );
        }
        let expected = one_thread(&source);
        assert_eq!(expected.len(), 150_000);
        same(&read_all(&source, TEST).unwrap(), &expected);
    }

    #[test]
    fn damage_and_a_refusing_caller_end_the_pass() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("damaged.e57");
        write_scans(&source, &[(150_000, false)]);
        let layout = scan_layout(&source, 0);
        let mut file = e57_points::open_reader(&source).unwrap();
        let scan = file.pointclouds()[0].clone();
        // A caller that refuses stops every thread.
        let mut given = 0;
        let refused = read_scan(&source, &mut file, &scan, TEST, &mut |_| {
            given += 1;
            if given == 10_000 {
                Err(LoadError::Cancelled)
            } else {
                Ok(())
            }
        });
        assert!(matches!(refused, Err(LoadError::Cancelled)));
        assert_eq!(given, 10_000);

        // A page that fails its checksum fails the pass.
        let mut bytes = std::fs::read(&source).unwrap();
        let inside = layout.packet_start(30) + 100;
        bytes[(inside / 1020 * 1024 + inside % 1020) as usize] ^= 0x40;
        std::fs::write(&source, &bytes).unwrap();
        assert!(read_all(&source, TEST).is_err());
    }
}
