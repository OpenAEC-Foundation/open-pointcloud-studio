//! The switch of the disk caches, in a process of its own: it is one switch
//! for the whole process, and the cache folder is chosen by the environment,
//! so no other test may run beside it.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use pointcloud_core::ops::{self, IndexOptions};
use pointcloud_core::{disk_caches, set_disk_caches, IndexConfig, OctreeIndex};

/// Write `lines` points of a text cloud; from about 800,000 lines on it is
/// larger than the 16 MiB from which the preview of a source is kept.
fn write_cloud(path: &Path, lines: u32, offset: f64) {
    let mut file = BufWriter::new(fs::File::create(path).unwrap());
    for line in 0..lines {
        let x = f64::from(line % 1000) * 0.01 + offset;
        let y = f64::from(line / 1000 % 1000) * 0.01;
        let z = f64::from(line / 1_000_000) * 0.01;
        writeln!(file, "{x:.4} {y:.4} {z:.4}").unwrap();
    }
    file.flush().unwrap();
}

/// Every file under `root` with its length and the time it was changed.
fn files(root: &Path) -> BTreeMap<PathBuf, (u64, Option<SystemTime>)> {
    let mut found = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(folder) = pending.pop() {
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let metadata = entry.metadata().unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            }
            found.insert(entry.path(), (metadata.len(), metadata.modified().ok()));
        }
    }
    found
}

#[test]
fn caches_off_read_and_write_nothing_under_the_cache_folder() {
    let directory = tempfile::tempdir().unwrap();
    let cache_home = directory.path().join("cache");
    // Before anything else, so that every cache of this process lies here.
    std::env::set_var("XDG_CACHE_HOME", &cache_home);
    let root = cache_home.join("open-pointcloud-studio/indexes");
    assert!(disk_caches(), "the caches are on by default");

    // Caches on: opening a large source keeps its preview, and the next
    // open reads it back. This shows that the test reaches the gate.
    let first = directory.path().join("first.xyz");
    write_cloud(&first, 820_000, 0.0);
    assert!(fs::metadata(&first).unwrap().len() >= 16 * 1024 * 1024);
    let opened = pointcloud_core::open(&first, 10).unwrap();
    assert_eq!(opened.points.len(), 10);
    let kept = files(&root);
    assert!(
        kept.keys().any(|path| path.ends_with("preview.json")),
        "{kept:?}"
    );
    assert_eq!(
        pointcloud_core::open(&first, 1000).unwrap().points.len(),
        10
    );

    // A third source gets an index in the cache, which the next open and
    // `open_cached_if_present` take while the caches are on.
    let third = directory.path().join("third.xyz");
    write_cloud(&third, 100_000, 50.0);
    let cloud = pointcloud_core::open(&third, 1).unwrap();
    let planted = OctreeIndex::build_cached(&cloud, IndexConfig::default()).unwrap();
    drop(planted);
    let from_cache = pointcloud_core::open(&third, 1_000_000).unwrap();
    assert!(
        from_cache.points.len() < 100_000,
        "the root of the cached index"
    );
    assert!(
        OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())
            .unwrap()
            .is_some()
    );

    // Off: nothing under the cache folder is written, changed or read.
    set_disk_caches(false);
    assert!(!disk_caches());
    let before = files(&root);

    let second = directory.path().join("second.xyz");
    write_cloud(&second, 820_000, 20.0);
    assert!(fs::metadata(&second).unwrap().len() >= 16 * 1024 * 1024);
    assert_eq!(pointcloud_core::open(&second, 10).unwrap().points.len(), 10);
    assert_eq!(
        files(&root),
        before,
        "a second large source leaves nothing new"
    );

    // The preview kept for the first source is not read: the open keeps
    // the points it was asked for.
    assert_eq!(
        pointcloud_core::open(&first, 1000).unwrap().points.len(),
        1000
    );

    // The index planted for the third source is not read either.
    let fresh = pointcloud_core::open(&third, 1_000_000).unwrap();
    assert_eq!(fresh.points.len(), 100_000);
    assert!(
        OctreeIndex::open_cached_if_present(&fresh, IndexConfig::default())
            .unwrap()
            .is_none()
    );

    // Indexes are built in temporary storage, whichever way they are asked.
    let built = OctreeIndex::build_cached(&fresh, IndexConfig::default()).unwrap();
    assert_eq!(built.root.total_points, 100_000);
    drop(built);
    let (opened, built) = OctreeIndex::open_and_build_cached_with_progress(
        &third,
        10,
        IndexConfig::default(),
        |_| Ok(()),
    )
    .unwrap();
    assert_eq!(opened.total_points, 100_000);
    assert_eq!(built.root.total_points, 100_000);
    drop(built);
    let scratch = directory.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let options = IndexOptions {
        cached: true,
        scratch_dir: Some(scratch.clone()),
    };
    let built = ops::build_index(&fresh, &options, &mut |_| Ok(())).unwrap();
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 1);
    drop(built);
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 0);
    assert_eq!(files(&root), before);
}
