//! Generated sources and small wrappers that the tests of the C library share.

#![allow(dead_code)]

use std::ffi::{c_void, CStr, CString};
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::ptr;

use ops_pointcloud::*;

/// A path as the C interface takes it.
pub fn c_path(path: &Path) -> CString {
    CString::new(path.to_str().expect("a UTF-8 test path")).expect("a path without NUL")
}

/// The message of the last failed call on this thread.
pub fn last_error() -> Option<String> {
    // SAFETY: the library returns NULL or a NUL-terminated string.
    let message = unsafe { ops_last_error() };
    (!message.is_null()).then(|| {
        // SAFETY: as above, valid until the next call on this thread.
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    })
}

/// An ASCII PLY grid of `side`³ points 0.1 apart from the origin, coloured
/// by position.
pub fn write_ply_grid(path: &Path, side: usize) {
    let mut text = format!(
        "ply\nformat ascii 1.0\nelement vertex {}\n\
         property double x\nproperty double y\nproperty double z\n\
         property uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n",
        side * side * side
    );
    for i in 0..side {
        for j in 0..side {
            for k in 0..side {
                writeln!(
                    text,
                    "{} {} {} {} {} {}",
                    i as f64 * 0.1,
                    j as f64 * 0.1,
                    k as f64 * 0.1,
                    i * 25,
                    j * 25,
                    k * 25
                )
                .unwrap();
            }
        }
    }
    fs::write(path, text).unwrap();
}

/// The walls, floor and ceiling of a room of 4 × 3 × 2.5 m sampled every
/// 0.25 m, with intensity and colour: `x y z intensity r g b` per line.
pub fn write_xyz_room(path: &Path) {
    let (width, depth, height) = (4.0f64, 3.0f64, 2.5f64);
    let steps = |length: f64| (length / 0.25).round() as usize;
    let mut text = String::new();
    let mut line = |x: f64, y: f64, z: f64| {
        let shade = ((x + y + z) * 20.0) as u32 % 256;
        writeln!(
            text,
            "{x:.3} {y:.3} {z:.3} {} {} {} {}",
            (z / height * 200.0) as u32 + 20,
            shade,
            255 - shade,
            128
        )
        .unwrap();
    };
    for a in 0..=steps(width) {
        for b in 0..=steps(height) {
            let (x, z) = (a as f64 * 0.25, b as f64 * 0.25);
            line(x, 0.0, z);
            line(x, depth, z);
        }
    }
    for a in 1..steps(depth) {
        for b in 0..=steps(height) {
            let (y, z) = (a as f64 * 0.25, b as f64 * 0.25);
            line(0.0, y, z);
            line(width, y, z);
        }
    }
    for a in 1..steps(width) {
        for b in 1..steps(depth) {
            let (x, y) = (a as f64 * 0.25, b as f64 * 0.25);
            line(x, y, 0.0);
            line(x, y, height);
        }
    }
    fs::write(path, text).unwrap();
}

/// An opened cloud, freed when it goes.
pub struct Cloud(pub *mut OpsCloud);

impl Cloud {
    pub fn open(path: &Path) -> Self {
        let mut cloud = ptr::null_mut();
        // SAFETY: a string and room for the pointer.
        let status = unsafe { ops_cloud_open(c_path(path).as_ptr(), &mut cloud) };
        assert_eq!(status, OPS_OK, "{path:?}: {:?}", last_error());
        assert!(!cloud.is_null());
        Self(cloud)
    }

    pub fn info(&self) -> OpsCloudInfo {
        let mut info = OpsCloudInfo {
            struct_size: size_of::<OpsCloudInfo>() as u32,
            ..Default::default()
        };
        // SAFETY: a live cloud and a sized struct.
        let status = unsafe { ops_cloud_info(self.0, &mut info) };
        assert_eq!(status, OPS_OK, "{:?}", last_error());
        info
    }

    pub fn json(&self) -> String {
        let mut json = ptr::null_mut();
        // SAFETY: a live cloud and room for the pointer.
        let status = unsafe { ops_cloud_info_json(self.0, &mut json) };
        assert_eq!(status, OPS_OK, "{:?}", last_error());
        // SAFETY: a string of the library, freed by it after the copy.
        let text = unsafe { CStr::from_ptr(json) }.to_str().unwrap().to_owned();
        unsafe { ops_string_free(json) };
        text
    }

    /// Every point, or those in `section`, in batches of `batch`.
    pub fn read(&self, section: Option<&OpsBox>, batch: usize) -> Vec<OpsPoint> {
        let mut points = Vec::new();
        // SAFETY: a live cloud, a valid box and a callback that collects.
        let status = unsafe {
            ops_cloud_read(
                self.0,
                section.map_or(ptr::null(), ptr::from_ref),
                batch,
                Some(collect),
                ptr::from_mut(&mut points).cast(),
            )
        };
        assert_eq!(status, OPS_OK, "{:?}", last_error());
        points
    }

    /// Export with the given options; the status and the count written.
    pub fn export(
        &self,
        destination: &Path,
        options: Option<&OpsExportOptions>,
    ) -> (OpsStatus, u64) {
        let mut written = u64::MAX;
        // SAFETY: a live cloud, a string, sized options and room for the count.
        let status = unsafe {
            ops_cloud_export(
                self.0,
                c_path(destination).as_ptr(),
                options.map_or(ptr::null(), ptr::from_ref),
                &mut written,
            )
        };
        (status, written)
    }
}

impl Drop for Cloud {
    fn drop(&mut self) {
        // SAFETY: the cloud of `open`, freed once.
        unsafe { ops_cloud_free(self.0) };
    }
}

/// A built index, freed when it goes.
pub struct Index(pub *mut OpsIndex);

impl Index {
    pub fn build(cloud: &Cloud, options: Option<&OpsIndexOptions>) -> Self {
        let mut index = ptr::null_mut();
        // SAFETY: a live cloud, sized options and room for the pointer.
        let status = unsafe {
            ops_index_build(
                cloud.0,
                options.map_or(ptr::null(), ptr::from_ref),
                &mut index,
            )
        };
        assert_eq!(status, OPS_OK, "{:?}", last_error());
        assert!(!index.is_null());
        Self(index)
    }

    pub fn info(&self) -> OpsIndexInfo {
        let mut info = OpsIndexInfo {
            struct_size: size_of::<OpsIndexInfo>() as u32,
            ..Default::default()
        };
        // SAFETY: a live index and a sized struct.
        let status = unsafe { ops_index_info(self.0, &mut info) };
        assert_eq!(status, OPS_OK, "{:?}", last_error());
        info
    }

    pub fn read(&self, section: Option<&OpsBox>, batch: usize) -> Vec<OpsPoint> {
        let mut points = Vec::new();
        // SAFETY: a live index, a valid box and a callback that collects.
        let status = unsafe {
            ops_index_read(
                self.0,
                section.map_or(ptr::null(), ptr::from_ref),
                batch,
                Some(collect),
                ptr::from_mut(&mut points).cast(),
            )
        };
        assert_eq!(status, OPS_OK, "{:?}", last_error());
        points
    }
}

impl Drop for Index {
    fn drop(&mut self) {
        // SAFETY: the index of `build`, freed once.
        unsafe { ops_index_free(self.0) };
    }
}

/// A points callback that appends every batch to the `Vec<OpsPoint>` that
/// `user_data` points at.
///
/// # Safety
/// `user_data` must point at a `Vec<OpsPoint>`.
pub unsafe extern "C" fn collect(
    points: *const OpsPoint,
    count: usize,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: the library hands over `count` points; the test gave the Vec.
    unsafe {
        let all = &mut *user_data.cast::<Vec<OpsPoint>>();
        all.extend_from_slice(std::slice::from_raw_parts(points, count));
    }
    0
}

/// The points sorted by their ordinal, for comparing reads of another order.
pub fn by_ordinal(mut points: Vec<OpsPoint>) -> Vec<OpsPoint> {
    points.sort_by_key(|point| point.ordinal);
    points
}

/// The names in a folder, sorted.
pub fn entries(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Every file below a folder, as paths relative to it.
pub fn files_below(folder: &Path) -> Vec<String> {
    fn walk(root: &Path, folder: &Path, found: &mut Vec<String>) {
        let Ok(listing) = fs::read_dir(folder) else {
            return;
        };
        for entry in listing {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, found);
            } else {
                found.push(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    let mut found = Vec::new();
    walk(folder, folder, &mut found);
    found.sort();
    found
}
