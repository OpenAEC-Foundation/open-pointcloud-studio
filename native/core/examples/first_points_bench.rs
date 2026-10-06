//! Time how soon large scans show their first points, and how long they take
//! to be read and indexed, the way the window opens them: every file on a
//! thread of its own, with a copy of every picture handed to another thread.
//!
//! ```text
//! cargo run --release -p pointcloud-core --example first_points_bench -- together|alone turns|reads|parallel FILE...
//! ```
//!
//! `together` opens every file at the same moment, `alone` one after the
//! other. `turns` reads every large file in its turn on its disk and builds
//! its octree in its turn, as the window does; `reads` takes the turns to
//! read only, and lets the octrees be built side by side; `parallel` reads
//! and builds every file at once. Set `XDG_CACHE_HOME` to an empty folder
//! first, so that no index or preview of an earlier run is used and nothing
//! lands in the user's own cache; delete it afterwards. The last lines hash
//! every index and preview the run made, to compare the outcome of two
//! builds.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Instant;

use pointcloud_core::{
    IndexConfig, IndexProgress, IndexStage, LoadError, OctreeIndex, PointCloud, SourceTurn,
};

/// The sample the window keeps of every scan it opens.
const SAMPLE_LIMIT: usize = 100_000;
/// A picture with this many points counts as dense.
const DENSE_POINTS: usize = 1_000_000;

/// What happened to one file, in seconds from the moment it was opened.
#[derive(Default)]
struct Timeline {
    /// Every picture shown: when, how many points, and whether provisional.
    pictures: Vec<(f64, usize, bool)>,
    reading: Option<f64>,
    tree: Option<f64>,
    done: Option<f64>,
    cloud: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: first_points_bench together|alone turns|reads|parallel FILE...";
    let (Some(order), Some(reading), files) =
        (args.first(), args.get(1), &args[2.min(args.len())..])
    else {
        return Err(usage.into());
    };
    let turns = match reading.as_str() {
        "turns" => Some(true),
        "reads" => Some(false),
        "parallel" => None,
        _ => return Err(usage.into()),
    };
    if files.is_empty() || std::env::var_os("XDG_CACHE_HOME").is_none() {
        return Err(format!("{usage}; set XDG_CACHE_HOME to an empty folder").into());
    }
    let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
    let started = Instant::now();
    let timelines = match order.as_str() {
        "together" => open_together(&files, turns),
        "alone" => files
            .iter()
            .map(|file| open_together(std::slice::from_ref(file), turns).remove(0))
            .collect(),
        _ => return Err(usage.into()),
    };
    let total = started.elapsed().as_secs_f64();
    for (file, timeline) in files.iter().zip(&timelines) {
        report(file, timeline);
    }
    println!("all read and indexed in {total:.1} s");
    hash_caches(Path::new(&std::env::var_os("XDG_CACHE_HOME").unwrap()))?;
    Ok(())
}

/// Open the files at the same moment, each on a thread of its own. With
/// `turns`, every large file takes its turn to be read, and with `Some(true)`
/// its turn to be indexed, in the order the files are opened.
fn open_together(files: &[PathBuf], turns: Option<bool>) -> Vec<Timeline> {
    let started = Instant::now();
    let workers: Vec<_> = files
        .iter()
        .map(|file| {
            let turn = turns.and_then(|build| SourceTurn::for_source(file, build));
            let file = file.clone();
            thread::spawn(move || open(&file, turn, started))
        })
        .collect();
    workers
        .into_iter()
        .map(|worker| worker.join().expect("worker"))
        .collect()
}

fn open(file: &Path, turn: Option<SourceTurn>, started: Instant) -> Timeline {
    let timeline = Mutex::new(Timeline::default());
    let seconds = || started.elapsed().as_secs_f64();
    // The window copies every picture and hands it to another thread.
    let (sender, receiver) = mpsc::channel::<Arc<PointCloud>>();
    let window = thread::spawn(move || receiver.into_iter().count());
    let preview = |cloud: &PointCloud| {
        let _ = sender.send(Arc::new(cloud.clone()));
        timeline
            .lock()
            .unwrap()
            .pictures
            .push((seconds(), cloud.points.len(), cloud.provisional));
        Ok::<(), LoadError>(())
    };
    let progress = |update: IndexProgress| {
        let mut timeline = timeline.lock().unwrap();
        match update.stage {
            IndexStage::ReadingSource if timeline.reading.is_none() => {
                timeline.reading = Some(seconds())
            }
            IndexStage::BuildingTree if timeline.tree.is_none() => timeline.tree = Some(seconds()),
            _ => {}
        }
        Ok(())
    };
    let built = match turn {
        Some(turn) => OctreeIndex::open_and_build_cached_in_turn(
            file,
            SAMPLE_LIMIT,
            IndexConfig::default(),
            turn,
            preview,
            progress,
        ),
        None => OctreeIndex::open_and_build_cached_with_preview(
            file,
            SAMPLE_LIMIT,
            IndexConfig::default(),
            preview,
            progress,
        ),
    };
    let (cloud, _index) = built.expect("open and index");
    drop(sender);
    window.join().expect("window");
    let mut timeline = timeline.into_inner().unwrap();
    timeline.done = Some(seconds());
    timeline.cloud = Some(describe(&cloud));
    timeline
}

fn report(file: &Path, timeline: &Timeline) {
    let name = file.file_name().unwrap_or_default().to_string_lossy();
    let at = |time: Option<f64>| time.map_or("-".to_owned(), |time| format!("{time:.1} s"));
    let first = timeline.pictures.first().map(|picture| picture.0);
    let dense = timeline
        .pictures
        .iter()
        .find(|picture| picture.1 >= DENSE_POINTS)
        .map(|picture| picture.0);
    let read = timeline
        .pictures
        .iter()
        .find(|picture| !picture.2)
        .map(|picture| picture.0);
    println!(
        "{name}: first {} ({} points), dense {}, reading from {}, read {}, tree from {}, indexed {}",
        at(first),
        timeline.pictures.first().map_or(0, |picture| picture.1),
        at(dense),
        at(timeline.reading),
        at(read),
        at(timeline.tree),
        at(timeline.done),
    );
    let pictures: Vec<String> = timeline
        .pictures
        .iter()
        .map(|(time, points, provisional)| {
            format!(
                "{time:.1}s:{}k{}",
                points / 1000,
                if *provisional { "" } else { "*" }
            )
        })
        .collect();
    println!("  pictures {}", pictures.join(" "));
    println!("  cloud {}", timeline.cloud.as_deref().unwrap_or("-"));
}

/// The checked cloud in short: its counts and bounds, and a checksum of its
/// points and their ordinals.
fn describe(cloud: &PointCloud) -> String {
    let mut bytes = Vec::with_capacity(cloud.points.len() * 48);
    for (point, ordinal) in cloud.points.iter().zip(&cloud.point_ordinals) {
        for value in point.xyz {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&point.rgb.unwrap_or_default());
        bytes.extend_from_slice(&point.intensity.unwrap_or(u16::MAX).to_le_bytes());
        bytes.push(point.classification.unwrap_or(u8::MAX));
        bytes.extend_from_slice(&ordinal.to_le_bytes());
    }
    format!(
        "{} points, {} kept, {} poses, {} scans, {} photos, bounds {:?} {:?}, checksum {:08x}",
        cloud.total_points,
        cloud.points.len(),
        cloud.scan_poses.len(),
        cloud.scan_ranges.len(),
        cloud.scan_images.len(),
        cloud.bounds.min,
        cloud.bounds.max,
        crc32c::crc32c(&bytes)
    )
}

/// A checksum of every folder of indexes and previews the run made, over the
/// names, sizes and contents of its files.
fn hash_caches(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let indexes = root.join("open-pointcloud-studio").join("indexes");
    let mut folders = Vec::new();
    collect_folders(&indexes, &mut folders)?;
    for folder in folders {
        let mut files = Vec::new();
        collect_files(&folder, &mut files)?;
        files.sort();
        let mut checksum = 0u32;
        let mut size = 0u64;
        for file in &files {
            let name = file
                .strip_prefix(&folder)?
                .to_string_lossy()
                .replace('\\', "/");
            checksum = crc32c::crc32c_append(checksum, name.as_bytes());
            let mut reader = fs::File::open(file)?;
            let mut buffer = vec![0u8; 8 << 20];
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                size += count as u64;
                checksum = crc32c::crc32c_append(checksum, &buffer[..count]);
            }
        }
        println!(
            "cache {}: {} files, {} bytes, checksum {checksum:08x}",
            folder.strip_prefix(&indexes)?.display(),
            files.len(),
            size
        );
    }
    Ok(())
}

/// The folders that hold files, the index of one source or its preview.
fn collect_folders(folder: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut entries: Vec<_> = fs::read_dir(folder)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    if entries.iter().any(|entry| entry.is_file()) {
        out.push(folder.to_path_buf());
        return Ok(());
    }
    for entry in entries {
        collect_folders(&entry, out)?;
    }
    Ok(())
}

fn collect_files(folder: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(folder)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
