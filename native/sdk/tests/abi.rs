//! The C interface as a C program sees it: every function on generated
//! sources, its results against the core functions it stands for, its
//! errors, threads and layouts, and the header against the library.

mod common;

use std::collections::BTreeMap;
use std::ffi::{c_void, CStr};
use std::fs;
use std::mem::offset_of;
use std::path::Path;
use std::ptr;
use std::sync::Mutex;
use std::thread::{self, ThreadId};

use common::*;
use ops_pointcloud::*;
use pointcloud_core::ops::{self, Convert};
use pointcloud_core::{Bounds, ExportFormat, OrientedBox};

const HEADER: &str = include_str!("../include/ops_pointcloud.h");

fn export_options(
    format: OpsFormat,
    section: Option<&OpsBox>,
    max_points: u64,
) -> OpsExportOptions {
    OpsExportOptions {
        struct_size: size_of::<OpsExportOptions>() as u32,
        format,
        r#box: section.map_or(ptr::null(), ptr::from_ref),
        max_points,
    }
}

fn index_options(scratch: Option<&std::ffi::CString>) -> OpsIndexOptions {
    OpsIndexOptions {
        struct_size: size_of::<OpsIndexOptions>() as u32,
        cached: 0,
        scratch_dir: scratch.map_or(ptr::null(), |scratch| scratch.as_ptr()),
        progress: None,
        user_data: ptr::null_mut(),
    }
}

/// The lower half of the grid along x: 500 of its 1000 points.
const HALF_GRID: OpsBox = OpsBox {
    min: [-0.05, -0.05, -0.05],
    max: [0.45, 0.95, 0.95],
    rotation_degrees: 0.0,
};

/// Records the fractions an index build reports.
unsafe extern "C" fn record_progress(fraction: f64, user_data: *mut c_void) -> i32 {
    // SAFETY: the test gave a Vec<f64>.
    unsafe { (*user_data.cast::<Vec<f64>>()).push(fraction) };
    0
}

#[test]
fn the_whole_interface_on_a_ply_grid() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 10);

    let cloud = Cloud::open(&grid);
    let info = cloud.info();
    assert_eq!(info.point_count, 1000);
    assert_eq!(info.station_count, 0);
    assert_eq!(info.has_rgb, 1);
    assert_eq!(info.min, [0.0; 3]);
    for axis in 0..3 {
        assert!((info.max[axis] - 0.9).abs() < 1e-9);
    }
    let json: serde_json::Value = serde_json::from_str(&cloud.json()).unwrap();
    assert_eq!(json["points"], 1000);
    assert_eq!(json["format"], "ply");

    let points = cloud.read(None, 100);
    assert_eq!(points.len(), 1000);
    assert!(points
        .iter()
        .enumerate()
        .all(|(ordinal, point)| point.ordinal == ordinal as u64));
    assert!(points
        .iter()
        .all(|point| point.flags == OPS_POINT_RGB && point.reserved == 0));
    assert_eq!(
        (points[999].r, points[999].g, points[999].b),
        (225, 225, 225)
    );

    let laz = folder.path().join("grid.laz");
    let (status, written) = cloud.export(
        &laz,
        Some(&export_options(
            OPS_FORMAT_BY_EXTENSION,
            Some(&HALF_GRID),
            0,
        )),
    );
    assert_eq!(status, OPS_OK, "{:?}", last_error());
    assert_eq!(written, 500);

    let half = Cloud::open(&laz);
    assert_eq!(half.info().point_count, 500);
    let scratch = folder.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let scratch_text = c_path(&scratch);
    let mut fractions: Vec<f64> = Vec::new();
    let options = OpsIndexOptions {
        progress: Some(record_progress),
        user_data: ptr::from_mut(&mut fractions).cast(),
        ..index_options(Some(&scratch_text))
    };
    let index = Index::build(&half, Some(&options));
    assert!(!fractions.is_empty());
    assert!(
        fractions.windows(2).all(|pair| pair[0] <= pair[1]),
        "{fractions:?}"
    );
    assert_eq!(fractions.last(), Some(&1.0));
    let summary = index.info();
    assert_eq!(summary.point_count, 500);
    assert!(summary.leaf_count >= 1 && summary.node_count >= summary.leaf_count);

    let through_index = by_ordinal(index.read(Some(&HALF_GRID), 64));
    assert_eq!(through_index, half.read(Some(&HALF_GRID), 0));
    assert_eq!(through_index.len(), 500);
}

#[test]
fn the_whole_interface_on_an_xyz_room() {
    let folder = tempfile::tempdir().unwrap();
    let room = folder.path().join("room.xyz");
    write_xyz_room(&room);

    let cloud = Cloud::open(&room);
    let info = cloud.info();
    assert_eq!(
        (info.has_rgb, info.has_intensity, info.has_classification),
        (1, 1, 0)
    );
    let all = cloud.read(None, 0);
    assert_eq!(all.len() as u64, info.point_count);
    assert!(all
        .iter()
        .all(|point| point.flags == OPS_POINT_RGB | OPS_POINT_INTENSITY));

    // A box turned by 30 degrees through one corner of the room.
    let turned = OpsBox {
        min: [-0.5, -0.5, 0.5],
        max: [1.5, 1.0, 2.0],
        rotation_degrees: 30.0,
    };
    let section = OrientedBox::new(
        Bounds {
            min: turned.min,
            max: turned.max,
        },
        turned.rotation_degrees,
    );
    let inside: Vec<OpsPoint> = all
        .iter()
        .copied()
        .filter(|point| section.contains([point.x, point.y, point.z]))
        .collect();
    assert!(!inside.is_empty() && inside.len() < all.len());
    assert_eq!(cloud.read(Some(&turned), 7), inside);

    let index = Index::build(&cloud, None);
    assert_eq!(index.info().point_count, info.point_count);
    assert_eq!(by_ordinal(index.read(Some(&turned), 0)), inside);
    assert_eq!(by_ordinal(index.read(None, 0)), all);
}

#[test]
fn an_export_writes_what_the_core_writes() {
    let folder = tempfile::tempdir().unwrap();
    let room = folder.path().join("room.xyz");
    write_xyz_room(&room);
    let cloud = Cloud::open(&room);
    let core_cloud = ops::open_source(&room, &ops::never_stop).unwrap();
    let section = OpsBox {
        min: [0.0, 0.0, 0.0],
        max: [2.0, 3.0, 1.0],
        rotation_degrees: 15.0,
    };
    let formats = [
        (OPS_FORMAT_LAS, ExportFormat::Las, "las"),
        (OPS_FORMAT_LAZ, ExportFormat::Laz, "laz"),
        (OPS_FORMAT_PLY, ExportFormat::PlyBinary, "ply"),
        (OPS_FORMAT_PLY_ASCII, ExportFormat::PlyAscii, "ply"),
        (OPS_FORMAT_XYZ, ExportFormat::Xyz, "xyz"),
        (OPS_FORMAT_PTS, ExportFormat::Pts, "pts"),
        (OPS_FORMAT_CSV, ExportFormat::Csv, "csv"),
    ];
    for (code, format, extension) in formats {
        for boxed in [None, Some(&section)] {
            let label = format!("{code}-{}", boxed.is_some());
            let through_c = folder.path().join(format!("c-{label}.{extension}"));
            let through_core = folder.path().join(format!("core-{label}.{extension}"));
            let (status, written) = cloud.export(&through_c, Some(&export_options(code, boxed, 0)));
            assert_eq!(status, OPS_OK, "{label}: {:?}", last_error());
            let count = ops::convert(
                &core_cloud,
                &through_core,
                Convert {
                    format,
                    section: boxed.map(|raw| {
                        OrientedBox::new(
                            Bounds {
                                min: raw.min,
                                max: raw.max,
                            },
                            raw.rotation_degrees,
                        )
                    }),
                    max_points: None,
                },
                &ops::never_stop,
            )
            .unwrap();
            assert_eq!(written, count, "{label}");
            assert_eq!(
                fs::read(&through_c).unwrap(),
                fs::read(&through_core).unwrap(),
                "{label}"
            );
        }
    }

    // E57 files carry random identifiers, so only the count is compared.
    let e57 = folder.path().join("room.e57");
    let (status, written) = cloud.export(&e57, None);
    assert_eq!(status, OPS_OK, "{:?}", last_error());
    assert_eq!(written, cloud.info().point_count);
    assert_eq!(Cloud::open(&e57).info().point_count, written);

    // Without options the extension decides: .ply is binary.
    let by_extension = folder.path().join("by-extension.ply");
    let explicit = folder.path().join("explicit.ply");
    assert_eq!(cloud.export(&by_extension, None).0, OPS_OK);
    assert_eq!(
        cloud
            .export(&explicit, Some(&export_options(OPS_FORMAT_PLY, None, 0)))
            .0,
        OPS_OK
    );
    assert_eq!(fs::read(by_extension).unwrap(), fs::read(explicit).unwrap());
}

#[test]
fn the_json_is_the_inspection_of_the_core() {
    let folder = tempfile::tempdir().unwrap();
    for (name, write) in [
        (
            "grid.ply",
            (|path: &Path| write_ply_grid(path, 6)) as fn(&Path),
        ),
        ("room.xyz", write_xyz_room),
    ] {
        let path = folder.path().join(name);
        write(&path);
        let cloud = Cloud::open(&path);
        let inspection = ops::inspect(&ops::open_source(&path, &ops::never_stop).unwrap());
        assert_eq!(cloud.json(), serde_json::to_string(&inspection).unwrap());
        let info = cloud.info();
        assert_eq!(info.point_count, inspection.points);
        assert_eq!(
            (info.min, info.max),
            (inspection.bounds.min, inspection.bounds.max)
        );
        assert_eq!(info.station_count as usize, inspection.stations.len());
        assert_eq!(info.station_photo_count as usize, inspection.station_photos);
        assert_eq!(
            (info.has_rgb, info.has_intensity, info.has_classification),
            (
                inspection.attributes.rgb.into(),
                inspection.attributes.intensity.into(),
                inspection.attributes.classification.into()
            )
        );
    }
}

#[test]
fn max_points_refuses_a_larger_result_and_writes_no_file() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 10);
    let cloud = Cloud::open(&grid);

    for (label, section, max_points) in [("all", None, 999), ("box", Some(&HALF_GRID), 499)] {
        let destination = folder.path().join(format!("{label}.laz"));
        let (status, written) = cloud.export(
            &destination,
            Some(&export_options(OPS_FORMAT_LAZ, section, max_points)),
        );
        assert_eq!(status, OPS_ERROR_LIMIT, "{label}");
        assert_eq!(written, 0, "{label}");
        assert!(!destination.exists(), "{label}");
        assert!(last_error().is_some_and(|message| message.contains(ops::TOO_MANY_POINTS)));
    }
    assert_eq!(entries(folder.path()), ["grid.ply"]);

    // A limit the result keeps changes nothing.
    let limited = folder.path().join("limited.xyz");
    let free = folder.path().join("free.xyz");
    assert_eq!(
        cloud.export(
            &limited,
            Some(&export_options(OPS_FORMAT_XYZ, Some(&HALF_GRID), 500))
        ),
        (OPS_OK, 500)
    );
    assert_eq!(
        cloud.export(
            &free,
            Some(&export_options(OPS_FORMAT_XYZ, Some(&HALF_GRID), 0))
        ),
        (OPS_OK, 500)
    );
    assert_eq!(fs::read(limited).unwrap(), fs::read(free).unwrap());
}

#[test]
fn an_index_with_a_scratch_folder_stays_in_it() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 12);
    let scratch = folder.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let cloud = Cloud::open(&grid);
    let scratch_text = c_path(&scratch);

    let index = Index::build(&cloud, Some(&index_options(Some(&scratch_text))));
    assert_eq!(entries(folder.path()), ["grid.ply", "scratch"]);
    assert_eq!(
        entries(&scratch).len(),
        1,
        "the index lives in the scratch folder"
    );
    assert!(!files_below(&scratch).is_empty());
    assert_eq!(index.read(None, 0).len(), 1728);
    drop(index);
    assert!(
        entries(&scratch).is_empty(),
        "freeing the index removes its folder"
    );
}

/// Asks to stop at the first report.
unsafe extern "C" fn stop_progress(_fraction: f64, user_data: *mut c_void) -> i32 {
    // SAFETY: the test gave a counter.
    unsafe { *user_data.cast::<u32>() += 1 };
    1
}

/// Asks to stop after the first batch.
unsafe extern "C" fn stop_points(
    _points: *const OpsPoint,
    _count: usize,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: the test gave a counter.
    unsafe { *user_data.cast::<u32>() += 1 };
    1
}

#[test]
fn a_callback_that_asks_to_stop_cancels_the_work() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 10);
    let cloud = Cloud::open(&grid);
    let scratch = folder.path().join("scratch");
    fs::create_dir(&scratch).unwrap();
    let scratch_text = c_path(&scratch);

    let mut calls = 0u32;
    let options = OpsIndexOptions {
        progress: Some(stop_progress),
        user_data: ptr::from_mut(&mut calls).cast(),
        ..index_options(Some(&scratch_text))
    };
    let mut index = ptr::NonNull::<OpsIndex>::dangling().as_ptr();
    // SAFETY: a live cloud, sized options and room for the pointer.
    let status = unsafe { ops_index_build(cloud.0, &options, &mut index) };
    assert_eq!(status, OPS_ERROR_CANCELLED);
    assert!(index.is_null());
    assert_eq!(calls, 1);
    assert!(last_error().is_some_and(|message| message.contains("progress")));
    assert!(
        entries(&scratch).is_empty(),
        "a stopped build leaves nothing"
    );

    let built = Index::build(&cloud, None);
    let mut calls = 0u32;
    // SAFETY: a live cloud and a callback that counts.
    let status = unsafe {
        ops_cloud_read(
            cloud.0,
            ptr::null(),
            10,
            Some(stop_points),
            ptr::from_mut(&mut calls).cast(),
        )
    };
    assert_eq!((status, calls), (OPS_ERROR_CANCELLED, 1));
    let mut calls = 0u32;
    // SAFETY: a live index and a callback that counts.
    let status = unsafe {
        ops_index_read(
            built.0,
            ptr::null(),
            10,
            Some(stop_points),
            ptr::from_mut(&mut calls).cast(),
        )
    };
    assert_eq!((status, calls), (OPS_ERROR_CANCELLED, 1));
    assert!(last_error().is_some_and(|message| message.contains("points")));
}

#[test]
fn null_pointers_are_refused() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 4);
    let cloud = Cloud::open(&grid);
    let index = Index::build(&cloud, None);
    let path = c_path(&grid);
    let destination = c_path(&folder.path().join("out.xyz"));
    let mut info = OpsCloudInfo {
        struct_size: size_of::<OpsCloudInfo>() as u32,
        ..Default::default()
    };
    let mut index_info = OpsIndexInfo {
        struct_size: size_of::<OpsIndexInfo>() as u32,
        ..Default::default()
    };
    let mut opened = ptr::null_mut();
    let mut built = ptr::null_mut();
    let mut json = ptr::null_mut();
    // SAFETY: every pointer is NULL or valid.
    let statuses = unsafe {
        [
            ops_cloud_open(ptr::null(), &mut opened),
            ops_cloud_open(path.as_ptr(), ptr::null_mut()),
            ops_cloud_info(ptr::null(), &mut info),
            ops_cloud_info(cloud.0, ptr::null_mut()),
            ops_cloud_info_json(ptr::null(), &mut json),
            ops_cloud_info_json(cloud.0, ptr::null_mut()),
            ops_cloud_read(ptr::null(), ptr::null(), 0, Some(collect), ptr::null_mut()),
            ops_cloud_read(cloud.0, ptr::null(), 0, None, ptr::null_mut()),
            ops_cloud_export(
                ptr::null(),
                destination.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
            ),
            ops_cloud_export(cloud.0, ptr::null(), ptr::null(), ptr::null_mut()),
            ops_index_build(ptr::null(), ptr::null(), &mut built),
            ops_index_build(cloud.0, ptr::null(), ptr::null_mut()),
            ops_index_info(ptr::null(), &mut index_info),
            ops_index_info(index.0, ptr::null_mut()),
            ops_index_read(ptr::null(), ptr::null(), 0, Some(collect), ptr::null_mut()),
            ops_index_read(index.0, ptr::null(), 0, None, ptr::null_mut()),
        ]
    };
    assert!(
        statuses.iter().all(|status| *status == OPS_ERROR_ARGUMENT),
        "{statuses:?}"
    );
    assert!(opened.is_null() && built.is_null() && json.is_null());
    assert!(last_error().is_some_and(|message| message.contains("NULL")));
    assert!(!folder.path().join("out.xyz").exists());

    // The free functions ignore NULL.
    // SAFETY: NULL is allowed.
    unsafe {
        ops_cloud_free(ptr::null_mut());
        ops_index_free(ptr::null_mut());
        ops_string_free(ptr::null_mut());
    }
}

#[test]
fn files_that_cannot_be_read_or_written() {
    let folder = tempfile::tempdir().unwrap();
    let open = |path: &Path| {
        let mut cloud = ptr::NonNull::<OpsCloud>::dangling().as_ptr();
        // SAFETY: a string and room for the pointer.
        let status = unsafe { ops_cloud_open(c_path(path).as_ptr(), &mut cloud) };
        assert!(cloud.is_null(), "{path:?}");
        (status, last_error())
    };
    let (status, message) = open(&folder.path().join("missing.ply"));
    assert_eq!(status, OPS_ERROR_IO, "{message:?}");
    assert!(message.is_some());

    let unknown = folder.path().join("points.abc");
    fs::write(&unknown, "0 0 0\n").unwrap();
    assert_eq!(open(&unknown).0, OPS_ERROR_FORMAT);

    let broken = folder.path().join("broken.ply");
    fs::write(
        &broken,
        "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\n\
         property float z\nend_header\n1 2\n",
    )
    .unwrap();
    assert_eq!(open(&broken).0, OPS_ERROR_DATA);

    // A path that is not UTF-8.
    let mut cloud = ptr::null_mut();
    // SAFETY: a NUL-terminated string and room for the pointer.
    let status = unsafe { ops_cloud_open(c"\xff\xfe.ply".as_ptr(), &mut cloud) };
    assert_eq!(status, OPS_ERROR_ARGUMENT);
    assert!(last_error().is_some_and(|message| message.contains("UTF-8")));

    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 3);
    let cloud = Cloud::open(&grid);
    assert_eq!(
        cloud.export(&folder.path().join("out.abc"), None),
        (OPS_ERROR_FORMAT, 0)
    );
    assert_eq!(
        cloud.export(&folder.path().join("missing").join("out.xyz"), None),
        (OPS_ERROR_IO, 0)
    );
    assert_eq!(
        cloud.export(&grid, None).0,
        OPS_ERROR_DATA,
        "source and destination are one file"
    );

    // A source that changed after it was opened.
    write_ply_grid(&grid, 4);
    let mut points: Vec<OpsPoint> = Vec::new();
    // SAFETY: a live cloud and a callback that collects.
    let status = unsafe {
        ops_cloud_read(
            cloud.0,
            ptr::null(),
            0,
            Some(collect),
            ptr::from_mut(&mut points).cast(),
        )
    };
    assert_eq!(status, OPS_ERROR_DATA);
    assert!(last_error().is_some_and(|message| message.contains("changed")));
}

#[test]
fn unknown_values_and_short_structs_are_refused() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 3);
    let cloud = Cloud::open(&grid);
    let index = Index::build(&cloud, None);
    let out = folder.path().join("out.xyz");

    assert_eq!(
        cloud.export(&out, Some(&export_options(99, None, 0))).0,
        OPS_ERROR_ARGUMENT
    );
    let upside_down = OpsBox {
        min: [1.0, 0.0, 0.0],
        max: [0.0, 1.0, 1.0],
        rotation_degrees: 0.0,
    };
    let unturnable = OpsBox {
        rotation_degrees: f64::NAN,
        ..HALF_GRID
    };
    for section in [&upside_down, &unturnable] {
        assert_eq!(
            cloud
                .export(
                    &out,
                    Some(&export_options(OPS_FORMAT_XYZ, Some(section), 0))
                )
                .0,
            OPS_ERROR_ARGUMENT
        );
        // SAFETY: a live cloud and index, a box and a callback that collects.
        let statuses = unsafe {
            let mut points: Vec<OpsPoint> = Vec::new();
            let user_data = ptr::from_mut(&mut points).cast();
            [
                ops_cloud_read(cloud.0, section, 0, Some(collect), user_data),
                ops_index_read(index.0, section, 0, Some(collect), user_data),
            ]
        };
        assert_eq!(statuses, [OPS_ERROR_ARGUMENT; 2]);
    }
    // SAFETY: a live cloud and a callback that collects.
    let status = unsafe {
        let mut points: Vec<OpsPoint> = Vec::new();
        ops_cloud_read(
            cloud.0,
            ptr::null(),
            MAX_BATCH_POINTS + 1,
            Some(collect),
            ptr::from_mut(&mut points).cast(),
        )
    };
    assert_eq!(status, OPS_ERROR_ARGUMENT);
    let mut built = ptr::null_mut();
    let twice = OpsIndexOptions {
        cached: 2,
        ..index_options(None)
    };
    // SAFETY: a live cloud, sized options and room for the pointer.
    assert_eq!(
        unsafe { ops_index_build(cloud.0, &twice, &mut built) },
        OPS_ERROR_ARGUMENT
    );

    // struct_size 0 in every struct that has one.
    let mut info = OpsCloudInfo::default();
    let mut index_info = OpsIndexInfo::default();
    let short_export = OpsExportOptions {
        struct_size: 0,
        ..export_options(OPS_FORMAT_XYZ, None, 0)
    };
    let short_index = OpsIndexOptions {
        struct_size: 0,
        ..index_options(None)
    };
    let mut written = 7;
    // SAFETY: live objects and structs of the right size, whatever they say.
    let statuses = unsafe {
        [
            ops_cloud_info(cloud.0, &mut info),
            ops_index_info(index.0, &mut index_info),
            ops_cloud_export(cloud.0, c_path(&out).as_ptr(), &short_export, &mut written),
            ops_index_build(cloud.0, &short_index, &mut built),
        ]
    };
    assert_eq!(statuses, [OPS_ERROR_ARGUMENT; 4]);
    assert_eq!(
        info,
        OpsCloudInfo::default(),
        "nothing is written into a short struct"
    );
    assert_eq!(written, 0);
    assert!(built.is_null());
    assert!(last_error().is_some_and(|message| message.contains("struct_size")));
    assert!(!out.exists());
}

/// Room for a struct of a later minor version: the library writes only the
/// fields it knows.
#[repr(C, align(8))]
struct Larger([u8; 128]);

#[test]
fn a_larger_struct_of_a_later_version_keeps_its_unknown_fields() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 3);
    let cloud = Cloud::open(&grid);
    let mut larger = Larger([0xab; 128]);
    larger.0[..4].copy_from_slice(&128u32.to_ne_bytes());
    // SAFETY: 128 writable bytes that start with their size.
    let status = unsafe { ops_cloud_info(cloud.0, ptr::from_mut(&mut larger).cast()) };
    assert_eq!(status, OPS_OK, "{:?}", last_error());
    assert_eq!(u32::from_ne_bytes(larger.0[..4].try_into().unwrap()), 128);
    assert_eq!(u64::from_ne_bytes(larger.0[8..16].try_into().unwrap()), 27);
    assert!(larger.0[size_of::<OpsCloudInfo>()..]
        .iter()
        .all(|byte| *byte == 0xab));

    // Options of a later version with fields this library does not know.
    let mut options = Larger([0; 128]);
    let known = export_options(OPS_FORMAT_XYZ, None, 0);
    // SAFETY: the known part of the options, copied as bytes.
    unsafe {
        ptr::copy_nonoverlapping(
            ptr::from_ref(&known).cast::<u8>(),
            options.0.as_mut_ptr(),
            size_of::<OpsExportOptions>(),
        );
    }
    options.0[..4].copy_from_slice(&128u32.to_ne_bytes());
    options.0[size_of::<OpsExportOptions>()..].fill(0xcd);
    let (status, written) = cloud.export(
        &folder.path().join("later.xyz"),
        Some(
            // SAFETY: the start of the bytes is a valid OpsExportOptions.
            unsafe { &*ptr::from_ref(&options).cast::<OpsExportOptions>() },
        ),
    );
    assert_eq!((status, written), (OPS_OK, 27), "{:?}", last_error());
}

#[test]
fn the_message_belongs_to_its_thread_and_a_success_clears_it() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 3);
    let cloud = Cloud::open(&grid);
    assert_eq!(last_error(), None);

    let mut opened = ptr::null_mut();
    // SAFETY: room for the pointer.
    assert_eq!(
        unsafe { ops_cloud_open(ptr::null(), &mut opened) },
        OPS_ERROR_ARGUMENT
    );
    let message = last_error().expect("a message after a failure");
    assert_eq!(
        thread::spawn(last_error).join().unwrap(),
        None,
        "another thread has no message"
    );
    assert_eq!(
        last_error().as_deref(),
        Some(message.as_str()),
        "asking does not clear it"
    );
    let _ = cloud.info();
    assert_eq!(last_error(), None, "a success clears it");
}

/// The context of a callback that calls back into the library.
struct Reentry {
    cloud: *const OpsCloud,
    messages: Vec<Option<String>>,
    statuses: Vec<OpsStatus>,
}

unsafe extern "C" fn call_back_in(
    _points: *const OpsPoint,
    _count: usize,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: the test gave a Reentry with a live cloud.
    unsafe {
        let reentry = &mut *user_data.cast::<Reentry>();
        reentry.messages.push(last_error());
        let mut info = OpsCloudInfo::default();
        reentry
            .statuses
            .push(ops_cloud_info(reentry.cloud, &mut info));
        reentry.messages.push(last_error());
    }
    0
}

#[test]
fn a_callback_may_call_the_library() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 5);
    let cloud = Cloud::open(&grid);
    let mut reentry = Reentry {
        cloud: cloud.0,
        messages: Vec::new(),
        statuses: Vec::new(),
    };
    // SAFETY: a live cloud and a callback with its context.
    let status = unsafe {
        ops_cloud_read(
            cloud.0,
            ptr::null(),
            50,
            Some(call_back_in),
            ptr::from_mut(&mut reentry).cast(),
        )
    };
    assert_eq!(status, OPS_OK, "{:?}", last_error());
    assert_eq!(reentry.statuses.len(), 3);
    // The inner call fails on its short struct and sets its message, which
    // the next batch still sees.
    assert!(reentry
        .statuses
        .iter()
        .all(|status| *status == OPS_ERROR_ARGUMENT));
    assert_eq!(reentry.messages[0], None);
    assert!(reentry.messages[1..].iter().all(Option::is_some));
    assert_eq!(last_error(), None, "the outer call succeeded");
}

/// Records the thread a callback runs on.
unsafe extern "C" fn thread_of_points(
    _points: *const OpsPoint,
    _count: usize,
    user_data: *mut c_void,
) -> i32 {
    // SAFETY: the test gave a Mutex of thread ids.
    unsafe {
        (*user_data.cast::<Mutex<Vec<ThreadId>>>())
            .lock()
            .unwrap()
            .push(thread::current().id())
    };
    0
}

unsafe extern "C" fn thread_of_progress(_fraction: f64, user_data: *mut c_void) -> i32 {
    // SAFETY: the test gave a Mutex of thread ids.
    unsafe {
        (*user_data.cast::<Mutex<Vec<ThreadId>>>())
            .lock()
            .unwrap()
            .push(thread::current().id())
    };
    0
}

#[test]
fn callbacks_run_on_the_calling_thread() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 20);
    let cloud = Cloud::open(&grid);
    let threads: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());
    let user_data = ptr::from_ref(&threads).cast_mut().cast();
    let options = OpsIndexOptions {
        progress: Some(thread_of_progress),
        user_data,
        ..index_options(None)
    };
    let index = Index::build(&cloud, Some(&options));
    // SAFETY: a live cloud and index and a callback with its context.
    let statuses = unsafe {
        [
            ops_cloud_read(cloud.0, ptr::null(), 100, Some(thread_of_points), user_data),
            ops_index_read(index.0, ptr::null(), 100, Some(thread_of_points), user_data),
        ]
    };
    assert_eq!(statuses, [OPS_OK; 2]);
    let threads = threads.into_inner().unwrap();
    assert!(threads.len() > 2 * 80);
    assert!(threads.iter().all(|id| *id == thread::current().id()));
}

/// A cloud or an index that several threads read at once.
struct Shared<T>(*const T);
// SAFETY: the library promises that clouds and indexes may be read from
// several threads at once; the test frees them only after the threads end.
unsafe impl<T> Send for Shared<T> {}
unsafe impl<T> Sync for Shared<T> {}

impl<T> Shared<T> {
    /// The pointer, through the whole value, which a thread may take along.
    fn get(&self) -> *const T {
        self.0
    }
}

#[test]
fn several_threads_read_one_cloud_and_one_index_at_once() {
    let folder = tempfile::tempdir().unwrap();
    let grid = folder.path().join("grid.ply");
    write_ply_grid(&grid, 12);
    let cloud = Cloud::open(&grid);
    let index = Index::build(&cloud, None);
    let expected = cloud.read(None, 0);
    let shared_cloud = Shared(cloud.0.cast_const());
    let shared_index = Shared(index.0.cast_const());
    thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                let mut from_cloud: Vec<OpsPoint> = Vec::new();
                let mut from_index: Vec<OpsPoint> = Vec::new();
                // SAFETY: the cloud and index live until the scope ends.
                let statuses = unsafe {
                    [
                        ops_cloud_read(
                            shared_cloud.get(),
                            ptr::null(),
                            333,
                            Some(collect),
                            ptr::from_mut(&mut from_cloud).cast(),
                        ),
                        ops_index_read(
                            shared_index.get(),
                            ptr::null(),
                            333,
                            Some(collect),
                            ptr::from_mut(&mut from_index).cast(),
                        ),
                    ]
                };
                assert_eq!(statuses, [OPS_OK; 2]);
                assert_eq!(from_cloud, expected);
                assert_eq!(by_ordinal(from_index), expected);
            });
        }
    });
}

#[test]
fn the_versions_of_the_library() {
    // SAFETY: always safe.
    let (abi, version) = unsafe { (ops_abi_version(), CStr::from_ptr(ops_version())) };
    assert_eq!(abi, (OPS_ABI_MAJOR << 16) | OPS_ABI_MINOR);
    assert_eq!(abi >> 16, 1);
    assert_eq!(version.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
}

/// The sizes the header states in a comment after each struct with a fixed
/// size: `typedef struct OpsPoint {  /* 40 bytes`.
fn header_sizes() -> BTreeMap<String, usize> {
    HEADER
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("typedef struct ")?;
            let (name, comment) = rest.split_once('{')?;
            let comment = comment.trim().strip_prefix("/*")?.trim();
            let bytes = comment.split_whitespace().next()?.parse().ok()?;
            Some((name.trim().to_owned(), bytes))
        })
        .collect()
}

#[test]
fn the_layouts_are_those_of_the_header() {
    let sizes: BTreeMap<String, usize> = [
        ("OpsPoint", size_of::<OpsPoint>()),
        ("OpsBox", size_of::<OpsBox>()),
        ("OpsCloudInfo", size_of::<OpsCloudInfo>()),
        ("OpsExportOptions", size_of::<OpsExportOptions>()),
        ("OpsIndexOptions", size_of::<OpsIndexOptions>()),
        ("OpsIndexInfo", size_of::<OpsIndexInfo>()),
    ]
    .into_iter()
    .map(|(name, size)| (name.to_owned(), size))
    .collect();
    assert_eq!(header_sizes(), sizes);

    // The offsets a C compiler gives the fields of the header.
    assert_eq!(
        [
            offset_of!(OpsPoint, x),
            offset_of!(OpsPoint, y),
            offset_of!(OpsPoint, z),
            offset_of!(OpsPoint, intensity),
            offset_of!(OpsPoint, r),
            offset_of!(OpsPoint, g),
            offset_of!(OpsPoint, b),
            offset_of!(OpsPoint, classification),
            offset_of!(OpsPoint, flags),
            offset_of!(OpsPoint, reserved),
            offset_of!(OpsPoint, ordinal),
        ],
        [0, 8, 16, 24, 26, 27, 28, 29, 30, 31, 32]
    );
    assert_eq!(
        [
            offset_of!(OpsBox, min),
            offset_of!(OpsBox, max),
            offset_of!(OpsBox, rotation_degrees)
        ],
        [0, 24, 48]
    );
    assert_eq!(
        [
            offset_of!(OpsCloudInfo, struct_size),
            offset_of!(OpsCloudInfo, station_count),
            offset_of!(OpsCloudInfo, point_count),
            offset_of!(OpsCloudInfo, min),
            offset_of!(OpsCloudInfo, max),
            offset_of!(OpsCloudInfo, station_photo_count),
            offset_of!(OpsCloudInfo, has_rgb),
            offset_of!(OpsCloudInfo, has_intensity),
            offset_of!(OpsCloudInfo, has_classification),
            offset_of!(OpsCloudInfo, reserved),
        ],
        [0, 4, 8, 16, 40, 64, 68, 69, 70, 71]
    );
    assert_eq!(
        [
            offset_of!(OpsExportOptions, struct_size),
            offset_of!(OpsExportOptions, format),
            offset_of!(OpsExportOptions, r#box),
            offset_of!(OpsExportOptions, max_points),
        ],
        [0, 4, 8, 16]
    );
    assert_eq!(
        [
            offset_of!(OpsIndexOptions, struct_size),
            offset_of!(OpsIndexOptions, cached),
            offset_of!(OpsIndexOptions, scratch_dir),
            offset_of!(OpsIndexOptions, progress),
            offset_of!(OpsIndexOptions, user_data),
        ],
        [0, 4, 8, 16, 24]
    );
    assert_eq!(
        [
            offset_of!(OpsIndexInfo, struct_size),
            offset_of!(OpsIndexInfo, depth),
            offset_of!(OpsIndexInfo, point_count),
            offset_of!(OpsIndexInfo, node_count),
            offset_of!(OpsIndexInfo, leaf_count),
        ],
        [0, 4, 8, 16, 24]
    );
}

/// The `ops_` names a text declares: in the header the functions after
/// `OPS_API`, in the source the exported functions.
fn declared_names(text: &str, marker: &str) -> Vec<String> {
    let mut names: Vec<String> = text
        .lines()
        .filter(|line| line.trim_start().starts_with(marker))
        .filter_map(|line| {
            let start = line.find("ops_")?;
            let name: String = line[start..]
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect();
            line[start + name.len()..].starts_with('(').then_some(name)
        })
        .collect();
    names.sort();
    names
}

#[test]
fn the_header_declares_the_functions_of_the_library() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut exported = Vec::new();
    for entry in fs::read_dir(source).unwrap() {
        let text = fs::read_to_string(entry.unwrap().path()).unwrap();
        exported.extend(declared_names(&text, "pub unsafe extern \"C\" fn"));
    }
    exported.sort();
    let declared = declared_names(HEADER, "OPS_API");
    assert_eq!(declared.len(), 15, "{declared:?}");
    assert_eq!(declared, exported);
}

#[test]
fn the_header_defines_the_values_of_the_library() {
    let defined: BTreeMap<String, i64> = HEADER
        .lines()
        .filter_map(|line| {
            let mut words = line.strip_prefix("#define ")?.split_whitespace();
            let name = words.next()?;
            let value = words.next()?.trim_end_matches('u').parse().ok()?;
            Some((name.to_owned(), value))
        })
        .collect();
    let library: BTreeMap<String, i64> = [
        ("OPS_ABI_MAJOR", i64::from(OPS_ABI_MAJOR)),
        ("OPS_ABI_MINOR", i64::from(OPS_ABI_MINOR)),
        ("OPS_OK", OPS_OK.into()),
        ("OPS_ERROR_ARGUMENT", OPS_ERROR_ARGUMENT.into()),
        ("OPS_ERROR_IO", OPS_ERROR_IO.into()),
        ("OPS_ERROR_FORMAT", OPS_ERROR_FORMAT.into()),
        ("OPS_ERROR_DATA", OPS_ERROR_DATA.into()),
        ("OPS_ERROR_CANCELLED", OPS_ERROR_CANCELLED.into()),
        ("OPS_ERROR_INTERNAL", OPS_ERROR_INTERNAL.into()),
        ("OPS_ERROR_LIMIT", OPS_ERROR_LIMIT.into()),
        ("OPS_FORMAT_BY_EXTENSION", OPS_FORMAT_BY_EXTENSION.into()),
        ("OPS_FORMAT_LAS", OPS_FORMAT_LAS.into()),
        ("OPS_FORMAT_LAZ", OPS_FORMAT_LAZ.into()),
        ("OPS_FORMAT_E57", OPS_FORMAT_E57.into()),
        ("OPS_FORMAT_PLY", OPS_FORMAT_PLY.into()),
        ("OPS_FORMAT_PLY_ASCII", OPS_FORMAT_PLY_ASCII.into()),
        ("OPS_FORMAT_XYZ", OPS_FORMAT_XYZ.into()),
        ("OPS_FORMAT_PTS", OPS_FORMAT_PTS.into()),
        ("OPS_FORMAT_CSV", OPS_FORMAT_CSV.into()),
        ("OPS_POINT_RGB", OPS_POINT_RGB.into()),
        ("OPS_POINT_INTENSITY", OPS_POINT_INTENSITY.into()),
        ("OPS_POINT_CLASSIFICATION", OPS_POINT_CLASSIFICATION.into()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value))
    .collect();
    assert_eq!(defined, library);
}
