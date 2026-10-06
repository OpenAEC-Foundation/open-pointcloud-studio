//! Time opening and indexing a set of station scans, the way the window does
//! with a scan project: every scan read on its own thread, with or without
//! snapshots on the way, and octrees built one or several at a time.
//!
//! ```text
//! cargo run --release -p pointcloud-core --example loading_bench -- write DIR SCANS POINTS
//! cargo run --release -p pointcloud-core --example loading_bench -- open DIR plain|steps
//! cargo run --release -p pointcloud-core --example loading_bench -- index DIR AT_ONCE
//! ```
//!
//! `write` makes synthetic E57 station scans in DIR. The other two read every
//! `.e57` file in DIR and keep their caches in a new folder inside it, so
//! nothing lands in the user's cache folder; delete DIR afterwards.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use e57::{
    E57Writer, Quaternion, Record, RecordDataType, RecordName, RecordValue, Transform, Translation,
};
use pointcloud_core::{IndexConfig, OctreeIndex, PointCloud};

/// The sample the window keeps of every scan it opens.
const SAMPLE_LIMIT: usize = 100_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage =
        "usage: loading_bench write DIR SCANS POINTS | open DIR plain|steps | index DIR AT_ONCE";
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["write", dir, scans, points] => {
            write_scans(Path::new(dir), scans.parse()?, points.parse()?)
        }
        ["open", dir, mode] => open_all(Path::new(dir), *mode == "steps"),
        ["index", dir, at_once] => index_all(Path::new(dir), at_once.parse()?),
        _ => Err(usage.into()),
    }
}

fn scans_in(dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let mut scans: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "e57"))
        .collect();
    scans.sort();
    if scans.is_empty() {
        return Err("no .e57 files in the folder".into());
    }
    Ok(scans)
}

/// A new cache folder inside DIR for the caches of one run.
fn fresh_cache(dir: &Path, name: &str) -> PathBuf {
    let cache = dir.join(format!("cache-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&cache);
    std::fs::create_dir_all(&cache).expect("cache folder");
    cache
}

/// Station scans of a room seen from its middle: a sweep of columns from
/// floor to ceiling, one after the other as a scanner turns, with what is
/// seen through a skylight left out as invalid records, stored as scaled
/// integers with colour, intensity and the grid indices, which is how
/// scanners store a station.
fn write_scans(dir: &Path, scans: usize, points: usize) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(dir)?;
    let rows = ((points as f64 / 1.6).sqrt().round() as usize).max(2);
    let columns = points.div_ceil(rows);
    let started = Instant::now();
    for scan in 0..scans {
        let path = dir.join(format!("station-{scan:03}.e57"));
        let mut writer =
            E57Writer::from_file(&path, &format!("{{00000000-0000-4000-8000-{scan:012}}}"))?;
        let coordinate = |name| Record {
            name,
            data_type: RecordDataType::ScaledInteger {
                min: -1_000_000,
                max: 1_000_000,
                scale: 0.0001,
                offset: 0.0,
            },
        };
        let prototype = vec![
            coordinate(RecordName::CartesianX),
            coordinate(RecordName::CartesianY),
            coordinate(RecordName::CartesianZ),
            Record {
                name: RecordName::CartesianInvalidState,
                data_type: RecordDataType::Integer { min: 0, max: 2 },
            },
            Record {
                name: RecordName::Intensity,
                data_type: RecordDataType::Integer { min: 0, max: 4095 },
            },
            Record::COLOR_RED_U8,
            Record::COLOR_GREEN_U8,
            Record::COLOR_BLUE_U8,
            Record {
                name: RecordName::RowIndex,
                data_type: RecordDataType::Integer {
                    min: 0,
                    max: rows as i64,
                },
            },
            Record {
                name: RecordName::ColumnIndex,
                data_type: RecordDataType::Integer {
                    min: 0,
                    max: columns as i64,
                },
            },
        ];
        let mut cloud = writer.add_pointcloud(
            &format!("{{00000000-0000-4000-9000-{scan:012}}}"),
            prototype,
        )?;
        cloud.set_name(Some(format!("Station {scan}")));
        cloud.set_transform(Some(Transform {
            rotation: Quaternion {
                w: 1.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            translation: Translation {
                x: (scan % 6) as f64 * 7.0,
                y: (scan / 6) as f64 * 5.0,
                z: 1.5,
            },
        }));
        let (half_x, half_y, below, above) = (3.5, 2.5, 1.5, 1.5);
        for record in 0..rows * columns {
            let (column, row) = (record / rows, record % rows);
            let elevation = (row as f64 + 0.5) / rows as f64 * std::f64::consts::PI
                - std::f64::consts::FRAC_PI_2;
            let azimuth = (column as f64 + 0.5) / columns as f64 * std::f64::consts::TAU;
            let direction = [
                elevation.cos() * azimuth.cos(),
                elevation.cos() * azimuth.sin(),
                elevation.sin(),
            ];
            // Distance to the first wall, floor or ceiling of the room.
            let mut reach = f64::INFINITY;
            for (axis, near, far) in [(0, half_x, half_x), (1, half_y, half_y), (2, below, above)] {
                let along = direction[axis];
                if along > 1e-9 {
                    reach = reach.min(far / along);
                } else if along < -1e-9 {
                    reach = reach.min(near / -along);
                }
            }
            let skylight = elevation > 1.2;
            let [x, y, z] = direction.map(|value| value * reach);
            let shade = ((x * 3.0).sin() * 60.0 + (z * 5.0).cos() * 50.0 + 140.0) as i64;
            cloud.add_point(vec![
                RecordValue::ScaledInteger((x / 0.0001) as i64),
                RecordValue::ScaledInteger((y / 0.0001) as i64),
                RecordValue::ScaledInteger((z / 0.0001) as i64),
                RecordValue::Integer(if skylight { 2 } else { 0 }),
                RecordValue::Integer((reach * 300.0).min(4095.0) as i64),
                RecordValue::Integer(shade.clamp(0, 255)),
                RecordValue::Integer((shade - 20).clamp(0, 255)),
                RecordValue::Integer((shade - 40).clamp(0, 255)),
                RecordValue::Integer(row as i64),
                RecordValue::Integer(column as i64),
            ])?;
        }
        cloud.finalize()?;
        writer.finalize()?;
        println!(
            "wrote {} ({} MB) after {:.1}s",
            path.display(),
            std::fs::metadata(&path)?.len() / 1_000_000,
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}

/// Open every scan on a thread of its own, as the window does. `steps`
/// shows each scan in steps; the snapshots are copied as the window copies
/// them. Reports when the first points could be shown and when all scans
/// were open.
fn open_all(dir: &Path, steps: bool) -> Result<(), Box<dyn std::error::Error>> {
    let scans = scans_in(dir)?;
    // The caches of an earlier run would make this one skip decoding.
    let cache = fresh_cache(dir, if steps { "steps" } else { "plain" });
    std::env::set_var("XDG_CACHE_HOME", &cache);
    let started = Instant::now();
    let first_points = Arc::new(Mutex::new(None::<Duration>));
    let snapshots = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = scans
        .iter()
        .cloned()
        .map(|path| {
            let first_points = Arc::clone(&first_points);
            let snapshots = Arc::clone(&snapshots);
            std::thread::spawn(move || {
                let note_points = || {
                    let mut first = first_points.lock().unwrap();
                    first.get_or_insert(started.elapsed());
                };
                let cloud = if steps {
                    pointcloud_core::open_with_snapshots(
                        &path,
                        SAMPLE_LIMIT,
                        |_| Ok(()),
                        |cloud| {
                            let copy = Arc::new(cloud.clone());
                            std::hint::black_box(copy);
                            snapshots.fetch_add(1, Ordering::Relaxed);
                            note_points();
                            Ok(())
                        },
                    )
                } else {
                    pointcloud_core::open_with_progress(&path, SAMPLE_LIMIT, |_| Ok(()))
                }
                .expect("open");
                note_points();
                (started.elapsed(), cloud.total_points, cloud.points.len())
            })
        })
        .collect();
    let mut points = 0u64;
    let mut slowest = Duration::ZERO;
    for thread in threads {
        let (done, total, _) = thread.join().expect("reader thread");
        points += total;
        slowest = slowest.max(done);
    }
    let first = first_points.lock().unwrap().unwrap_or_default();
    println!(
        "open {}: {} scans, {} points, all open after {:.2}s, first points after {:.2}s, {} snapshots",
        if steps { "steps" } else { "plain" },
        scans.len(),
        points,
        slowest.as_secs_f64(),
        first.as_secs_f64(),
        snapshots.load(Ordering::Relaxed)
    );
    let _ = std::fs::remove_dir_all(&cache);
    Ok(())
}

/// Build the octree of every scan, `at_once` at a time, the way the window
/// indexes the scans it opened: each build reads its source again.
fn index_all(dir: &Path, at_once: usize) -> Result<(), Box<dyn std::error::Error>> {
    let scans = scans_in(dir)?;
    let cache = fresh_cache(dir, &format!("index{at_once}"));
    std::env::set_var("XDG_CACHE_HOME", &cache);
    let clouds: Vec<Arc<PointCloud>> = scans
        .iter()
        .map(|path| pointcloud_core::open(path, SAMPLE_LIMIT).map(Arc::new))
        .collect::<Result<_, _>>()?;
    let queue = Arc::new(Mutex::new(clouds.clone()));
    let config = IndexConfig {
        scratch_dir: Some(cache.join("indexes")),
        ..IndexConfig::default()
    };
    let started = Instant::now();
    let workers: Vec<_> = (0..at_once.max(1))
        .map(|_| {
            let queue = Arc::clone(&queue);
            let config = config.clone();
            std::thread::spawn(move || {
                let mut builds = Vec::new();
                loop {
                    let Some(cloud) = queue.lock().unwrap().pop() else {
                        return builds;
                    };
                    let begun = Instant::now();
                    let index =
                        OctreeIndex::build_cached_with_progress(&cloud, config.clone(), |_| Ok(()))
                            .expect("index");
                    assert_eq!(index.root.total_points, cloud.total_points);
                    builds.push(begun.elapsed());
                }
            })
        })
        .collect();
    let builds: Vec<Duration> = workers
        .into_iter()
        .flat_map(|worker| worker.join().expect("index worker"))
        .collect();
    let total = started.elapsed();
    let mean = builds.iter().sum::<Duration>() / builds.len().max(1) as u32;
    println!(
        "index {} at once: {} scans in {:.2}s, {:.2}s per build on average, peak memory {} MB",
        at_once,
        builds.len(),
        total.as_secs_f64(),
        mean.as_secs_f64(),
        peak_memory_mb()
    );
    let _ = std::fs::remove_dir_all(&cache);
    Ok(())
}

#[cfg(windows)]
fn peak_memory_mb() -> u64 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        size: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_non_paged: usize,
        quota_non_paged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, size: u32) -> i32;
    }
    let mut counters = Counters {
        size: std::mem::size_of::<Counters>() as u32,
        ..Counters::default()
    };
    // SAFETY: the counters are a properly sized and aligned structure of the
    // layout the function fills.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.size) };
    if ok == 0 {
        0
    } else {
        counters.peak_working_set as u64 / 1_000_000
    }
}

#[cfg(not(windows))]
fn peak_memory_mb() -> u64 {
    0
}
