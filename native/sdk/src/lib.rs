//! The C library of the point-cloud core of Open Pointcloud Studio.
//!
//! `include/ops_pointcloud.h` declares the fifteen functions below, and
//! docs/library.md describes them. Each function is a thin layer over
//! `pointcloud_core::ops` and the core's readers: it checks the arguments,
//! calls the one function of the core that does the work, and turns the
//! outcome into a status and a message. None of them calculates anything of
//! its own, and none calls another exported function, so that a host that
//! defines a function of the same name cannot reach inside.

mod error;
mod pin;

use std::ffi::{c_char, c_void, CStr, CString};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Once;

use pointcloud_core::ops::{self, Convert, IndexOptions};
use pointcloud_core::region_source::overlaps;
use pointcloud_core::{
    set_disk_caches, visit_points, Bounds, ExportFormat, IndexProgress, IndexStage, LoadError,
    OctreeIndex, OrientedBox, Point, PointCloud,
};

use error::Failure;

/// The major version of the interface: it changes only when a function or a
/// layout is removed or changed.
pub const OPS_ABI_MAJOR: u32 = 1;
/// The minor version: it grows with additions.
pub const OPS_ABI_MINOR: u32 = 0;

/// What a function that can fail returns.
pub type OpsStatus = i32;
pub const OPS_OK: OpsStatus = 0;
/// A null pointer, an unknown value, a `struct_size` too small, a path not in UTF-8.
pub const OPS_ERROR_ARGUMENT: OpsStatus = 1;
/// The file system refused.
pub const OPS_ERROR_IO: OpsStatus = 2;
/// The extension names no supported format.
pub const OPS_ERROR_FORMAT: OpsStatus = 3;
/// The file is not a readable point cloud, or the source changed.
pub const OPS_ERROR_DATA: OpsStatus = 4;
/// A callback asked to stop.
pub const OPS_ERROR_CANCELLED: OpsStatus = 5;
/// An internal error was caught; the message says where.
pub const OPS_ERROR_INTERNAL: OpsStatus = 6;
/// The result would hold more than `max_points` points.
pub const OPS_ERROR_LIMIT: OpsStatus = 7;

/// The format an export writes.
pub type OpsFormat = i32;
pub const OPS_FORMAT_BY_EXTENSION: OpsFormat = 0;
pub const OPS_FORMAT_LAS: OpsFormat = 1;
pub const OPS_FORMAT_LAZ: OpsFormat = 2;
pub const OPS_FORMAT_E57: OpsFormat = 3;
/// Binary little-endian.
pub const OPS_FORMAT_PLY: OpsFormat = 4;
pub const OPS_FORMAT_PLY_ASCII: OpsFormat = 5;
pub const OPS_FORMAT_XYZ: OpsFormat = 6;
pub const OPS_FORMAT_PTS: OpsFormat = 7;
pub const OPS_FORMAT_CSV: OpsFormat = 8;

/// Bits of `OpsPoint::flags`: which attributes of the point are valid.
pub const OPS_POINT_RGB: u8 = 1;
pub const OPS_POINT_INTENSITY: u8 = 2;
pub const OPS_POINT_CLASSIFICATION: u8 = 4;

/// The default and the largest number of points in one batch of a read.
pub const DEFAULT_BATCH_POINTS: usize = 65_536;
pub const MAX_BATCH_POINTS: usize = 1_048_576;

/// One point as a read hands it over. Fixed for every ABI 1.x.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OpsPoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub intensity: u16,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub classification: u8,
    pub flags: u8,
    pub reserved: u8,
    pub ordinal: u64,
}

impl OpsPoint {
    fn new(point: &Point, ordinal: u64) -> Self {
        let mut flags = 0;
        let [r, g, b] = point
            .rgb
            .inspect(|_| flags |= OPS_POINT_RGB)
            .unwrap_or_default();
        let intensity = point
            .intensity
            .inspect(|_| flags |= OPS_POINT_INTENSITY)
            .unwrap_or_default();
        let classification = point
            .classification
            .inspect(|_| flags |= OPS_POINT_CLASSIFICATION)
            .unwrap_or_default();
        Self {
            x: point.xyz[0],
            y: point.xyz[1],
            z: point.xyz[2],
            intensity,
            r,
            g,
            b,
            classification,
            flags,
            reserved: 0,
            ordinal,
        }
    }
}

/// A box turned about the vertical through its centre. Fixed for every ABI 1.x.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OpsBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// Counter-clockwise seen from above.
    pub rotation_degrees: f64,
}

/// What `ops_cloud_info` fills in.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OpsCloudInfo {
    pub struct_size: u32,
    pub station_count: u32,
    pub point_count: u64,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub station_photo_count: u32,
    pub has_rgb: u8,
    pub has_intensity: u8,
    pub has_classification: u8,
    pub reserved: u8,
}

/// What `ops_cloud_export` writes.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OpsExportOptions {
    pub struct_size: u32,
    pub format: OpsFormat,
    /// NULL: every point.
    pub r#box: *const OpsBox,
    /// 0: no limit.
    pub max_points: u64,
}

impl Default for OpsExportOptions {
    fn default() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            format: OPS_FORMAT_BY_EXTENSION,
            r#box: ptr::null(),
            max_points: 0,
        }
    }
}

/// Called during long work with the fraction done, or -1 when it is unknown;
/// anything but 0 stops the work.
pub type OpsProgressFn = Option<unsafe extern "C" fn(fraction: f64, user_data: *mut c_void) -> i32>;

/// How `ops_index_build` keeps the index.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OpsIndexOptions {
    pub struct_size: u32,
    /// 1: in the cache folder of the application.
    pub cached: i32,
    /// A UTF-8 folder for an uncached index; NULL: the system temp folder.
    pub scratch_dir: *const c_char,
    pub progress: OpsProgressFn,
    pub user_data: *mut c_void,
}

impl Default for OpsIndexOptions {
    fn default() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            cached: 0,
            scratch_dir: ptr::null(),
            progress: None,
            user_data: ptr::null_mut(),
        }
    }
}

/// What `ops_index_info` fills in.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpsIndexInfo {
    pub struct_size: u32,
    pub depth: u32,
    pub point_count: u64,
    pub node_count: u64,
    pub leaf_count: u64,
}

/// Receives a batch of points; anything but 0 stops the read.
pub type OpsPointsFn = Option<
    unsafe extern "C" fn(points: *const OpsPoint, count: usize, user_data: *mut c_void) -> i32,
>;

/// An opened source file.
pub struct OpsCloud {
    cloud: PointCloud,
}

/// An octree index on disk; its folder goes when it is freed, unless it is
/// in the cache folder.
pub struct OpsIndex {
    index: OctreeIndex,
}

// A cloud or an index may be read from several threads at once. A field that
// is not thread-safe in either type breaks this build instead of the promise.
const _: fn() = || {
    fn ok<T: Send + Sync>() {}
    ok::<PointCloud>();
    ok::<OctreeIndex>();
};

static FIRST_USE: Once = Once::new();

/// What the first call of the library does: pin the module, and turn the
/// disk caches of the core off. `ops_set_disk_caches` runs this before it
/// sets the switch, so a caller's choice is never undone.
pub(crate) fn first_use() {
    FIRST_USE.call_once(|| {
        pin::pin_module();
        set_disk_caches(false);
    });
}

static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// A reference to what a pointer of the caller points at.
///
/// # Safety
/// A non-null `pointer` must point at a valid `T` for as long as the
/// reference is used.
unsafe fn given<'a, T>(pointer: *const T, name: &str) -> Result<&'a T, Failure> {
    // SAFETY: the caller promises a valid `T` when the pointer is not null.
    unsafe { pointer.as_ref() }.ok_or_else(|| Failure::argument(format!("{name} is NULL")))
}

/// A path the caller gave as a UTF-8 string.
///
/// # Safety
/// A non-null `text` must point at a NUL-terminated string.
unsafe fn given_path(text: *const c_char, name: &str) -> Result<PathBuf, Failure> {
    if text.is_null() {
        return Err(Failure::argument(format!("{name} is NULL")));
    }
    // SAFETY: the caller promises a NUL-terminated string.
    let text = unsafe { CStr::from_ptr(text) };
    let text = text
        .to_str()
        .map_err(|_| Failure::argument(format!("{name} is not UTF-8")))?;
    Ok(PathBuf::from(text))
}

/// The `struct_size` at the start of a struct of the caller, checked against
/// the size of the struct in ABI 1.0.
///
/// # Safety
/// `pointer` must point at a struct that starts with a `u32`.
unsafe fn struct_size<T>(pointer: *const T, name: &str) -> Result<usize, Failure> {
    // SAFETY: every struct with a size starts with it.
    let size = unsafe { ptr::read_unaligned(pointer.cast::<u32>()) } as usize;
    if size < size_of::<T>() {
        return Err(Failure::argument(format!(
            "{name}.struct_size is {size}, less than the {} bytes of ABI 1.0",
            size_of::<T>()
        )));
    }
    Ok(size)
}

/// Read a struct of the caller that has a `struct_size`: the bytes it says
/// it has, as far as this library knows them, over the defaults.
///
/// # Safety
/// A non-null `from` must point at `struct_size` readable bytes.
unsafe fn read_sized<T: Copy + Default>(from: *const T, name: &str) -> Result<T, Failure> {
    if from.is_null() {
        return Ok(T::default());
    }
    // SAFETY: the caller promises a struct with a size field.
    let size = unsafe { struct_size(from, name) }?;
    let mut value = T::default();
    // SAFETY: both hold at least `size_of::<T>()` bytes, and a byte copy
    // into a local value of plain fields cannot overlap the caller's.
    unsafe {
        ptr::copy_nonoverlapping(
            from.cast::<u8>(),
            ptr::from_mut(&mut value).cast::<u8>(),
            size.min(size_of::<T>()),
        );
    }
    Ok(value)
}

/// Write a struct of the caller that has a `struct_size`, as many of its
/// bytes as both sides know; the caller's `struct_size` stays.
///
/// # Safety
/// A non-null `to` must point at `struct_size` writable bytes.
unsafe fn write_sized<T: Copy>(to: *mut T, value: T, name: &str) -> Result<(), Failure> {
    if to.is_null() {
        return Err(Failure::argument(format!("{name} is NULL")));
    }
    // SAFETY: the caller promises a struct with a size field.
    let size = unsafe { struct_size(to.cast_const(), name) }?;
    let mut value = value;
    // SAFETY: the caller's struct holds `size` bytes, of which the first
    // `size_of::<T>()` at most are written from a local value. Its first
    // field, the size, is written over with the value it already has.
    unsafe {
        ptr::write_unaligned(ptr::from_mut(&mut value).cast::<u32>(), size as u32);
        ptr::copy_nonoverlapping(
            ptr::from_ref(&value).cast::<u8>(),
            to.cast::<u8>(),
            size.min(size_of::<T>()),
        );
    }
    Ok(())
}

/// The box of the caller, which must have finite limits in order and a
/// finite turn.
fn section_box(raw: &OpsBox) -> Result<OrientedBox, Failure> {
    let section = OrientedBox::new(
        Bounds {
            min: raw.min,
            max: raw.max,
        },
        raw.rotation_degrees,
    );
    if section.is_valid() {
        Ok(section)
    } else {
        Err(Failure::argument(
            "box needs finite limits with min not above max and a finite rotation",
        ))
    }
}

/// The number of points in one batch of a read.
fn batch_size(batch_points: usize) -> Result<usize, Failure> {
    match batch_points {
        0 => Ok(DEFAULT_BATCH_POINTS),
        1..=MAX_BATCH_POINTS => Ok(batch_points),
        _ => Err(Failure::argument(format!(
            "batch_points is {batch_points}, more than {MAX_BATCH_POINTS}"
        ))),
    }
}

/// The format of an export: the one named, or the one the destination's
/// extension names.
fn export_format(format: OpsFormat, destination: &Path) -> Result<ExportFormat, Failure> {
    Ok(match format {
        OPS_FORMAT_BY_EXTENSION => {
            return ops::export_format_for_path(destination).ok_or_else(|| {
                LoadError::UnsupportedFormat(
                    destination
                        .extension()
                        .map(|extension| extension.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                )
                .into()
            })
        }
        OPS_FORMAT_LAS => ExportFormat::Las,
        OPS_FORMAT_LAZ => ExportFormat::Laz,
        OPS_FORMAT_E57 => ExportFormat::E57,
        OPS_FORMAT_PLY => ExportFormat::PlyBinary,
        OPS_FORMAT_PLY_ASCII => ExportFormat::PlyAscii,
        OPS_FORMAT_XYZ => ExportFormat::Xyz,
        OPS_FORMAT_PTS => ExportFormat::Pts,
        OPS_FORMAT_CSV => ExportFormat::Csv,
        other => return Err(Failure::argument(format!("format {other} is unknown"))),
    })
}

/// The function a read hands its batches to.
type PointsCallback = unsafe extern "C" fn(*const OpsPoint, usize, *mut c_void) -> i32;

/// Collects the points of a read into batches and hands each to the caller.
struct Batches {
    points: Vec<OpsPoint>,
    size: usize,
    callback: PointsCallback,
    user_data: *mut c_void,
    stopped: bool,
}

impl Batches {
    fn new(size: usize, callback: PointsCallback, user_data: *mut c_void) -> Self {
        Self {
            points: Vec::with_capacity(size),
            size,
            callback,
            user_data,
            stopped: false,
        }
    }

    fn push(&mut self, point: OpsPoint) -> Result<(), LoadError> {
        self.points.push(point);
        if self.points.len() == self.size {
            self.hand_over()?;
        }
        Ok(())
    }

    fn hand_over(&mut self) -> Result<(), LoadError> {
        if self.points.is_empty() {
            return Ok(());
        }
        // SAFETY: the caller's callback receives a buffer that is valid for
        // the call; what it does with `user_data` is its own affair.
        let answer =
            unsafe { (self.callback)(self.points.as_ptr(), self.points.len(), self.user_data) };
        self.points.clear();
        if answer != 0 {
            self.stopped = true;
            return Err(LoadError::Cancelled);
        }
        Ok(())
    }

    /// The outcome of a read, with a stop that the callback asked for as a
    /// failure of its own.
    fn outcome(&self, result: Result<(), LoadError>) -> Result<(), Failure> {
        match result {
            Err(LoadError::Cancelled) if self.stopped => Err(Failure::stopped("points")),
            other => other.map_err(Failure::from),
        }
    }
}

/// The ABI version of the loaded library: `(OPS_ABI_MAJOR << 16) | OPS_ABI_MINOR`.
///
/// # Safety
/// Always safe to call.
#[no_mangle]
pub unsafe extern "C" fn ops_abi_version() -> u32 {
    const VERSION: u32 = (OPS_ABI_MAJOR << 16) | OPS_ABI_MINOR;
    error::quietly(VERSION, || VERSION)
}

/// The product version, such as "2026.44.0": a static string.
///
/// # Safety
/// Always safe to call; the string must not be freed or written.
#[no_mangle]
pub unsafe extern "C" fn ops_version() -> *const c_char {
    error::quietly(VERSION.as_ptr().cast(), || VERSION.as_ptr().cast())
}

/// The message of the last failed call on this thread, or NULL. It belongs
/// to the library and is valid until the next call on this thread that
/// returns a status.
///
/// # Safety
/// Always safe to call; the string must not be freed or written.
#[no_mangle]
pub unsafe extern "C" fn ops_last_error() -> *const c_char {
    error::quietly(ptr::null(), error::last)
}

/// Free a string that the library returned through a `char **`. NULL is ignored.
///
/// # Safety
/// `text` must be NULL or a string from this library that was not freed yet.
#[no_mangle]
pub unsafe extern "C" fn ops_string_free(text: *mut c_char) {
    error::quietly((), || {
        if !text.is_null() {
            // SAFETY: the caller promises a string of `CString::into_raw`.
            drop(unsafe { CString::from_raw(text) });
        }
    });
}

/// Whether index and preview caches are read and written in the cache folder
/// of the application, for the whole process: on with 1, off otherwise. They
/// are off unless a caller sets them on.
///
/// # Safety
/// Always safe to call. Work that runs on other threads at the same time
/// may see either setting.
#[no_mangle]
pub unsafe extern "C" fn ops_set_disk_caches(enabled: i32) {
    error::quietly((), || set_disk_caches(enabled == 1));
}

/// Open a point-cloud file: a LAS or LAZ file from its header, any other
/// format with one pass over its points. `*cloud` is NULL after a failure.
///
/// # Safety
/// `path` must be NULL or a NUL-terminated string; `cloud` must be NULL or
/// point at writable room for a pointer.
#[no_mangle]
pub unsafe extern "C" fn ops_cloud_open(
    path: *const c_char,
    cloud: *mut *mut OpsCloud,
) -> OpsStatus {
    error::run("ops_cloud_open", || {
        if cloud.is_null() {
            return Err(Failure::argument("cloud is NULL"));
        }
        // SAFETY: checked above that the caller gave room for the pointer.
        unsafe { cloud.write(ptr::null_mut()) };
        // SAFETY: the caller promises a string.
        let path = unsafe { given_path(path, "path") }?;
        let opened = ops::open_source(&path, &ops::never_stop)?;
        // SAFETY: as above.
        unsafe { cloud.write(Box::into_raw(Box::new(OpsCloud { cloud: opened }))) };
        Ok(())
    })
}

/// Free a cloud. NULL is ignored.
///
/// # Safety
/// `cloud` must be NULL or a cloud of `ops_cloud_open` that was not freed
/// yet and that no other thread uses.
#[no_mangle]
pub unsafe extern "C" fn ops_cloud_free(cloud: *mut OpsCloud) {
    error::quietly((), || {
        if !cloud.is_null() {
            // SAFETY: the caller promises a cloud of `Box::into_raw`.
            drop(unsafe { Box::from_raw(cloud) });
        }
    });
}

/// Fill in what a cloud holds.
///
/// # Safety
/// `cloud` must be NULL or a live cloud; `info` must be NULL or point at an
/// `OpsCloudInfo` with its `struct_size` set.
#[no_mangle]
pub unsafe extern "C" fn ops_cloud_info(
    cloud: *const OpsCloud,
    info: *mut OpsCloudInfo,
) -> OpsStatus {
    error::run("ops_cloud_info", || {
        // SAFETY: the caller promises a live cloud.
        let cloud = unsafe { given(cloud, "cloud") }?;
        if info.is_null() {
            return Err(Failure::argument("info is NULL"));
        }
        let inspection = ops::inspect(&cloud.cloud);
        let value = OpsCloudInfo {
            struct_size: 0,
            station_count: u32::try_from(inspection.stations.len()).unwrap_or(u32::MAX),
            point_count: inspection.points,
            min: inspection.bounds.min,
            max: inspection.bounds.max,
            station_photo_count: u32::try_from(inspection.station_photos).unwrap_or(u32::MAX),
            has_rgb: inspection.attributes.rgb.into(),
            has_intensity: inspection.attributes.intensity.into(),
            has_classification: inspection.attributes.classification.into(),
            reserved: 0,
        };
        // SAFETY: the caller promises an `OpsCloudInfo` of `struct_size` bytes.
        unsafe { write_sized(info, value, "info") }
    })
}

/// What a cloud holds, as JSON: the answer of the cloud API's `inspect`.
/// Free `*json` with `ops_string_free`; it is NULL after a failure.
///
/// # Safety
/// `cloud` must be NULL or a live cloud; `json` must be NULL or point at
/// writable room for a pointer.
#[no_mangle]
pub unsafe extern "C" fn ops_cloud_info_json(
    cloud: *const OpsCloud,
    json: *mut *mut c_char,
) -> OpsStatus {
    error::run("ops_cloud_info_json", || {
        if json.is_null() {
            return Err(Failure::argument("json is NULL"));
        }
        // SAFETY: checked above that the caller gave room for the pointer.
        unsafe { json.write(ptr::null_mut()) };
        // SAFETY: the caller promises a live cloud.
        let cloud = unsafe { given(cloud, "cloud") }?;
        let text = serde_json::to_string(&ops::inspect(&cloud.cloud))
            .map_err(|error| Failure::from(LoadError::InvalidData(error.to_string())))?;
        let text = CString::new(text)
            .map_err(|_| Failure::from(LoadError::InvalidData("JSON with a NUL".into())))?;
        // SAFETY: as above.
        unsafe { json.write(text.into_raw()) };
        Ok(())
    })
}

/// Read the points of a cloud from its source, in source order, the points
/// in `box` only when it is given, in batches of `batch_points`.
///
/// # Safety
/// `cloud` must be NULL or a live cloud; `box` NULL or a valid `OpsBox`;
/// `callback` NULL or a function that does not unwind.
#[no_mangle]
pub unsafe extern "C" fn ops_cloud_read(
    cloud: *const OpsCloud,
    r#box: *const OpsBox,
    batch_points: usize,
    callback: OpsPointsFn,
    user_data: *mut c_void,
) -> OpsStatus {
    error::run("ops_cloud_read", || {
        // SAFETY: the caller promises a live cloud and a valid box.
        let cloud = &unsafe { given(cloud, "cloud") }?.cloud;
        let section = unsafe { r#box.as_ref() }.map(section_box).transpose()?;
        let size = batch_size(batch_points)?;
        let callback = callback.ok_or_else(|| Failure::argument("callback is NULL"))?;
        cloud.validate_source()?;
        let mut batches = Batches::new(size, callback, user_data);
        let mut read = 0u64;
        let result = visit_points(&cloud.path, &mut |point| {
            let ordinal = read;
            read += 1;
            if section.is_some_and(|section| !section.contains(point.xyz)) {
                return Ok(());
            }
            batches.push(OpsPoint::new(&point, ordinal))
        });
        let result = result.and_then(|()| batches.hand_over());
        batches.outcome(result)?;
        if read != cloud.total_points {
            return Err(LoadError::InvalidData("source changed since loading".into()).into());
        }
        Ok(())
    })
}

/// Write the points of a cloud, or those in `options->box`, to `destination`.
/// `*written`, when given, is the number written, and 0 after a failure.
/// An export cannot be stopped in ABI 1.0.
///
/// # Safety
/// `cloud` must be NULL or a live cloud; `destination` NULL or a
/// NUL-terminated string; `options` NULL or an `OpsExportOptions` with its
/// `struct_size` set and a `box` that is NULL or valid; `written` NULL or
/// writable.
#[no_mangle]
pub unsafe extern "C" fn ops_cloud_export(
    cloud: *const OpsCloud,
    destination: *const c_char,
    options: *const OpsExportOptions,
    written: *mut u64,
) -> OpsStatus {
    error::run("ops_cloud_export", || {
        if !written.is_null() {
            // SAFETY: the caller promises room for the count.
            unsafe { written.write(0) };
        }
        // SAFETY: the caller promises a live cloud, a string and options.
        let cloud = &unsafe { given(cloud, "cloud") }?.cloud;
        let destination = unsafe { given_path(destination, "destination") }?;
        let options: OpsExportOptions = unsafe { read_sized(options, "options") }?;
        let format = export_format(options.format, &destination)?;
        // SAFETY: the caller promises a valid box when it gives one.
        let section = unsafe { options.r#box.as_ref() }
            .map(section_box)
            .transpose()?;
        let max_points = (options.max_points != 0).then_some(options.max_points);
        let count = ops::convert(
            cloud,
            &destination,
            Convert {
                format,
                section,
                max_points,
            },
            &ops::never_stop,
        )?;
        if !written.is_null() {
            // SAFETY: as above.
            unsafe { written.write(count) };
        }
        Ok(())
    })
}

/// The fraction of a whole index build: reading the source is the first
/// half, building the tree the second.
fn build_fraction(progress: &IndexProgress) -> f64 {
    let Some(stage) = progress.fraction().map(f64::from) else {
        return -1.0;
    };
    match progress.stage {
        IndexStage::WaitingToRead => 0.0,
        IndexStage::ReadingSource => 0.5 * stage,
        IndexStage::WaitingToBuild => 0.5,
        IndexStage::BuildingTree => 0.5 + 0.5 * stage,
        IndexStage::Ready => 1.0,
    }
}

/// Build an octree index of a cloud on disk. `*index` is NULL after a failure.
///
/// # Safety
/// `cloud` must be NULL or a live cloud; `options` NULL or an
/// `OpsIndexOptions` with its `struct_size` set, a `scratch_dir` that is
/// NULL or a NUL-terminated string and a `progress` that does not unwind;
/// `index` NULL or writable room for a pointer.
#[no_mangle]
pub unsafe extern "C" fn ops_index_build(
    cloud: *const OpsCloud,
    options: *const OpsIndexOptions,
    index: *mut *mut OpsIndex,
) -> OpsStatus {
    error::run("ops_index_build", || {
        if index.is_null() {
            return Err(Failure::argument("index is NULL"));
        }
        // SAFETY: checked above that the caller gave room for the pointer.
        unsafe { index.write(ptr::null_mut()) };
        // SAFETY: the caller promises a live cloud and options.
        let cloud = &unsafe { given(cloud, "cloud") }?.cloud;
        let options: OpsIndexOptions = unsafe { read_sized(options, "options") }?;
        let cached = match options.cached {
            0 => false,
            1 => true,
            other => {
                return Err(Failure::argument(format!(
                    "options.cached is {other}, not 0 or 1"
                )))
            }
        };
        let scratch_dir = if options.scratch_dir.is_null() {
            None
        } else {
            // SAFETY: the caller promises a string.
            Some(unsafe { given_path(options.scratch_dir, "options.scratch_dir") }?)
        };
        let mut stopped = false;
        let built = ops::build_index(
            cloud,
            &IndexOptions {
                cached,
                scratch_dir,
            },
            &mut |progress| {
                let Some(report) = options.progress else {
                    return Ok(());
                };
                // SAFETY: the caller's callback; what it does with
                // `user_data` is its own affair.
                if unsafe { report(build_fraction(&progress), options.user_data) } != 0 {
                    stopped = true;
                    return Err(LoadError::Cancelled);
                }
                Ok(())
            },
        );
        let built = match built {
            Err(LoadError::Cancelled) if stopped => return Err(Failure::stopped("progress")),
            other => other?,
        };
        // SAFETY: as above.
        unsafe { index.write(Box::into_raw(Box::new(OpsIndex { index: built }))) };
        Ok(())
    })
}

/// Free an index, and its folder unless it is in the cache folder. NULL is
/// ignored.
///
/// # Safety
/// `index` must be NULL or an index of `ops_index_build` that was not freed
/// yet and that no other thread uses.
#[no_mangle]
pub unsafe extern "C" fn ops_index_free(index: *mut OpsIndex) {
    error::quietly((), || {
        if !index.is_null() {
            // SAFETY: the caller promises an index of `Box::into_raw`.
            drop(unsafe { Box::from_raw(index) });
        }
    });
}

/// Fill in the shape of an index.
///
/// # Safety
/// `index` must be NULL or a live index; `info` NULL or an `OpsIndexInfo`
/// with its `struct_size` set.
#[no_mangle]
pub unsafe extern "C" fn ops_index_info(
    index: *const OpsIndex,
    info: *mut OpsIndexInfo,
) -> OpsStatus {
    error::run("ops_index_info", || {
        // SAFETY: the caller promises a live index.
        let index = &unsafe { given(index, "index") }?.index;
        if info.is_null() {
            return Err(Failure::argument("info is NULL"));
        }
        let summary = ops::index_summary(index);
        let value = OpsIndexInfo {
            struct_size: 0,
            depth: summary.depth.into(),
            point_count: summary.points,
            node_count: summary.nodes,
            leaf_count: summary.leaves,
        };
        // SAFETY: the caller promises an `OpsIndexInfo` of `struct_size` bytes.
        unsafe { write_sized(info, value, "info") }
    })
}

/// Read the points of an index, those in `box` only when it is given, in
/// the order of its leaves, in batches of `batch_points`.
///
/// # Safety
/// `index` must be NULL or a live index; `box` NULL or a valid `OpsBox`;
/// `callback` NULL or a function that does not unwind.
#[no_mangle]
pub unsafe extern "C" fn ops_index_read(
    index: *const OpsIndex,
    r#box: *const OpsBox,
    batch_points: usize,
    callback: OpsPointsFn,
    user_data: *mut c_void,
) -> OpsStatus {
    error::run("ops_index_read", || {
        // SAFETY: the caller promises a live index and a valid box.
        let index = &unsafe { given(index, "index") }?.index;
        let section = unsafe { r#box.as_ref() }.map(section_box).transpose()?;
        let size = batch_size(batch_points)?;
        let callback = callback.ok_or_else(|| Failure::argument("callback is NULL"))?;
        let around = section.map(|section| section.aabb());
        let mut batches = Batches::new(size, callback, user_data);
        let result = index.visit_intersecting(
            |bounds| around.is_none_or(|around| overlaps(bounds, around)),
            |indexed| {
                if section.is_some_and(|section| !section.contains(indexed.point.xyz)) {
                    return Ok(());
                }
                batches.push(OpsPoint::new(&indexed.point, indexed.ordinal))
            },
        );
        let result = result.and_then(|()| batches.hand_over());
        batches.outcome(result)
    })
}
