//! The disk caches are off in the library unless the caller turns them on.
//!
//! One test in a test program of its own: the switch is process-wide, the
//! first call of the library decides it, and the cache folder is chosen
//! before any other work by setting `XDG_CACHE_HOME`, which no other thread
//! reads at that moment.

mod common;

use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::ptr;

use common::*;
use ops_pointcloud::*;

/// The size from which opening a file writes a preview into the cache.
const PREVIEW_CACHE_MIN_BYTES: u64 = 16 * 1024 * 1024;

/// A text file of a little more than the preview-cache threshold.
fn write_large_xyz(path: &Path, shift: f64) {
    let mut text = String::with_capacity(24 << 20);
    for index in 0..720_000u32 {
        let (x, y) = (f64::from(index % 800) * 0.01, f64::from(index / 800) * 0.01);
        writeln!(text, "{:.6} {:.6} {:.6}", x + shift, y, (x * y).sin()).unwrap();
    }
    fs::write(path, text).unwrap();
    assert!(fs::metadata(path).unwrap().len() >= PREVIEW_CACHE_MIN_BYTES);
}

fn cached_index_options() -> OpsIndexOptions {
    OpsIndexOptions {
        struct_size: size_of::<OpsIndexOptions>() as u32,
        cached: 1,
        scratch_dir: ptr::null(),
        progress: None,
        user_data: ptr::null_mut(),
    }
}

#[test]
fn disk_caches_are_off_until_the_caller_turns_them_on() {
    let folder = tempfile::tempdir().unwrap();
    let cache = folder.path().join("cache");
    fs::create_dir(&cache).unwrap();
    // Before the first call of the library, while this is the only thread
    // that does anything.
    std::env::set_var("XDG_CACHE_HOME", &cache);

    let first = folder.path().join("first.xyz");
    write_large_xyz(&first, 0.0);
    let cloud = Cloud::open(&first);
    assert_eq!(
        files_below(&cache),
        Vec::<String>::new(),
        "opening wrote a preview"
    );
    let index = Index::build(&cloud, Some(&cached_index_options()));
    assert_eq!(index.info().point_count, 720_000);
    assert_eq!(
        files_below(&cache),
        Vec::<String>::new(),
        "a cached index was written"
    );
    drop(index);
    drop(cloud);

    // Turned on, the same calls write into the cache folder: the files are
    // large enough to reach the caches, so the silence above was the switch.
    // SAFETY: always safe.
    unsafe { ops_set_disk_caches(1) };
    let second = folder.path().join("second.xyz");
    write_large_xyz(&second, 100.0);
    let cloud = Cloud::open(&second);
    let previews = files_below(&cache);
    assert!(
        previews.iter().any(|file| file.contains("previews")),
        "no preview was written: {previews:?}"
    );
    let index = Index::build(&cloud, Some(&cached_index_options()));
    drop(index);
    assert!(
        files_below(&cache).len() > previews.len(),
        "no cached index was written"
    );

    // Turned off again, a third file leaves nothing new.
    // SAFETY: always safe.
    unsafe { ops_set_disk_caches(0) };
    let before = files_below(&cache);
    let third = folder.path().join("third.xyz");
    write_large_xyz(&third, 200.0);
    let cloud = Cloud::open(&third);
    drop(Index::build(&cloud, Some(&cached_index_options())));
    assert_eq!(files_below(&cache), before);
}
