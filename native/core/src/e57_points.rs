//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::path::Path;

use e57::{Blob, CartesianCoordinate, E57Reader, ImageFormat, PointCloud, Projection};

use super::window_reader::WindowReader;
use super::{quaternion_axes, LoadError, Point, ScanImage, ScanImageFormat, ScanPose};

/// A stored photo larger than this is treated as damaged metadata.
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
/// Photos this close to a station belong to it when the file names no scan.
const STATION_MATCH_DISTANCE: f64 = 0.01;

/// Open an E57 file through a reader that keeps its buffer across the
/// decoder's page-by-page seeks.
pub(crate) fn open_reader(path: &Path) -> Result<E57Reader<WindowReader>, LoadError> {
    Ok(E57Reader::new(WindowReader::open(path)?)?)
}

fn scan_pose(index: usize, scan: &PointCloud) -> Option<ScanPose> {
    let transform = scan.transform.as_ref()?;
    let position = [
        transform.translation.x,
        transform.translation.y,
        transform.translation.z,
    ];
    position
        .iter()
        .all(|value| value.is_finite())
        .then(|| ScanPose {
            label: scan
                .name
                .clone()
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Scan {}", index + 1)),
            position,
            axes: quaternion_axes([
                transform.rotation.w,
                transform.rotation.x,
                transform.rotation.y,
                transform.rotation.z,
            ]),
        })
}

/// Read scanner stations from E57 metadata without decoding point records.
pub(crate) fn scan_poses(path: &Path) -> Result<Vec<ScanPose>, LoadError> {
    let file = open_reader(path)?;
    Ok(file
        .pointclouds()
        .iter()
        .enumerate()
        .filter_map(|(index, scan)| scan_pose(index, scan))
        .collect())
}

/// Read the station photos listed in E57 metadata without decoding them.
/// Only pinhole photos with a pose are returned.
pub(crate) fn scan_images(path: &Path) -> Result<Vec<ScanImage>, LoadError> {
    let file = open_reader(path)?;
    // Stations are numbered like `scan_poses`: scans without a pose are skipped.
    let mut stations = Vec::new();
    for (index, scan) in file.pointclouds().iter().enumerate() {
        if let Some(pose) = scan_pose(index, scan) {
            stations.push((scan.guid.clone(), pose.position));
        }
    }
    let mut images = Vec::new();
    for image in file.images() {
        let (Some(Projection::Pinhole(pinhole)), Some(transform)) =
            (&image.projection, &image.transform)
        else {
            continue;
        };
        let position = [
            transform.translation.x,
            transform.translation.y,
            transform.translation.z,
        ];
        let Some(axes) = quaternion_axes([
            transform.rotation.w,
            transform.rotation.x,
            transform.rotation.y,
            transform.rotation.z,
        ]) else {
            continue;
        };
        let properties = &pinhole.properties;
        let focal = [
            properties.focal_length / properties.pixel_width,
            properties.focal_length / properties.pixel_height,
        ];
        let principal = [properties.principal_x, properties.principal_y];
        let blob = &pinhole.blob.data;
        if properties.width == 0
            || properties.height == 0
            || blob.length == 0
            || blob.length > MAX_IMAGE_BYTES
            || !position
                .iter()
                .chain(&focal)
                .chain(&principal)
                .all(|value| value.is_finite())
            || focal.iter().any(|value| *value <= 0.0)
        {
            continue;
        }
        let station = image
            .pointcloud_guid
            .as_ref()
            .and_then(|guid| {
                stations
                    .iter()
                    .position(|(scan, _)| scan.as_ref() == Some(guid))
            })
            .or_else(|| {
                stations.iter().position(|(_, station)| {
                    (0..3).all(|axis| {
                        (station[axis] - position[axis]).abs() <= STATION_MATCH_DISTANCE
                    })
                })
            });
        images.push(ScanImage {
            station,
            position,
            axes,
            width: properties.width,
            height: properties.height,
            focal,
            principal,
            format: match pinhole.blob.format {
                ImageFormat::Png => ScanImageFormat::Png,
                ImageFormat::Jpeg => ScanImageFormat::Jpeg,
            },
            offset: blob.offset,
            length: blob.length,
        });
    }
    Ok(images)
}

/// Read the encoded bytes of station photos, in the order given.
pub(crate) fn read_images(path: &Path, images: &[ScanImage]) -> Result<Vec<Vec<u8>>, LoadError> {
    let mut file = open_reader(path)?;
    let source_length = std::fs::metadata(path)?.len();
    images
        .iter()
        .map(|image| {
            if image.length > MAX_IMAGE_BYTES
                || image
                    .offset
                    .checked_add(image.length)
                    .is_none_or(|end| end > source_length)
            {
                return Err(LoadError::InvalidData(
                    "station photo lies outside the file".into(),
                ));
            }
            let mut bytes = Vec::with_capacity(image.length as usize);
            let written = file.blob(&Blob::new(image.offset, image.length), &mut bytes)?;
            if written != image.length {
                return Err(LoadError::InvalidData("station photo is truncated".into()));
            }
            Ok(bytes)
        })
        .collect()
}

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    pose_push: &mut impl FnMut(ScanPose),
) -> Result<(), LoadError> {
    let mut file = open_reader(path)?;
    for (index, scan) in file.pointclouds().into_iter().enumerate() {
        if let Some(pose) = scan_pose(index, &scan) {
            pose_push(pose);
        }
        let mut points = file.pointcloud_simple(&scan)?;
        points.spherical_to_cartesian(true);
        // Preserve whether the scan actually contains RGB. The e57 crate's
        // default converts intensity-only points into synthetic grey colors.
        points.intensity_to_color(false);
        points.apply_pose(true);
        for point in points {
            let point = point?;
            let CartesianCoordinate::Valid { x, y, z } = &point.cartesian else {
                continue;
            };
            let xyz = [*x, *y, *z];
            push(simple_point(point, xyz))?;
        }
    }
    Ok(())
}

pub(crate) fn simple_point(point: e57::Point, xyz: [f64; 3]) -> Point {
    Point {
        xyz,
        rgb: point.color.map(|color| {
            [color.red, color.green, color.blue]
                .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
        }),
        intensity: point
            .intensity
            .map(|value| (value.clamp(0.0, 1.0) * 65535.0).round() as u16),
        classification: None,
    }
}
