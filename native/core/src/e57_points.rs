//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::io::{Read, Seek};
use std::path::Path;

use e57::{Blob, CartesianCoordinate, E57Reader, ImageFormat, PointCloud, Projection, RecordName};

use super::window_reader::WindowReader;
use super::{quaternion_axes, Bounds, LoadError, Point, ScanImage, ScanImageFormat, ScanPose};

/// A stored photo larger than this is treated as damaged metadata.
const MAX_IMAGE_BYTES: u64 = 256 * 1024 * 1024;
/// Photos this close to a station belong to it when the file names no scan.
const STATION_MATCH_DISTANCE: f64 = 0.01;

/// Open an E57 file through a reader that keeps its buffer across the
/// decoder's page-by-page seeks.
pub(crate) fn open_reader(path: &Path) -> Result<E57Reader<WindowReader>, LoadError> {
    Ok(E57Reader::new(WindowReader::open(path)?)?)
}

/// The scanner station of every scan in a file that has one. A merged cloud
/// is stored as a scan too, but its pose only places its coordinates: the
/// only scan of a file has no station when it has neither the grid or angles
/// of a scanner sweep nor a name.
pub(crate) fn stations(scans: &[PointCloud]) -> Vec<Option<ScanPose>> {
    scans
        .iter()
        .enumerate()
        .map(|(index, scan)| scan_pose(index, scan, scans.len() == 1))
        .collect()
}

fn scan_pose(index: usize, scan: &PointCloud, alone: bool) -> Option<ScanPose> {
    let transform = scan.transform.as_ref()?;
    let swept = !alone
        || scan.index_bounds.is_some()
        || scan.spherical_bounds.is_some()
        || scan.prototype.iter().any(|record| {
            matches!(
                record.name,
                RecordName::RowIndex
                    | RecordName::ColumnIndex
                    | RecordName::SphericalRange
                    | RecordName::SphericalAzimuth
                    | RecordName::SphericalElevation
            )
        });
    if !swept && scan.name.as_ref().is_none_or(|name| name.is_empty()) {
        return None;
    }
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
    Ok(stations(&file.pointclouds())
        .into_iter()
        .flatten()
        .collect())
}

/// What E57 metadata says about a file before any point is decoded.
pub(crate) struct Summary {
    /// Stored point records; invalid records make the decoded count smaller.
    pub records: u64,
    /// Registered box around every scan that states its extent.
    pub bounds: Option<Bounds>,
    pub has_rgb: bool,
    pub has_intensity: bool,
    pub poses: Vec<ScanPose>,
}

/// Summarise an E57 file from its metadata alone.
pub(crate) fn summary(path: &Path) -> Result<Summary, LoadError> {
    let file = open_reader(path)?;
    let mut summary = Summary {
        records: 0,
        bounds: None,
        has_rgb: false,
        has_intensity: false,
        poses: Vec::new(),
    };
    let scans = file.pointclouds();
    for (scan, pose) in scans.iter().zip(stations(&scans)) {
        summary.records += scan.records;
        summary.has_rgb |= scan.has_color();
        summary.has_intensity |= scan.has_intensity();
        if let Some(local) = scan.get_cartesian_bounds() {
            let (Some(x0), Some(x1), Some(y0), Some(y1), Some(z0), Some(z1)) = (
                local.x_min,
                local.x_max,
                local.y_min,
                local.y_max,
                local.z_min,
                local.z_max,
            ) else {
                continue;
            };
            // The stated extent is in the scan's own frame: register its
            // corners with the scan pose.
            let placed = scan.transform.as_ref().and_then(|transform| {
                quaternion_axes([
                    transform.rotation.w,
                    transform.rotation.x,
                    transform.rotation.y,
                    transform.rotation.z,
                ])
                .map(|axes| {
                    (
                        axes,
                        [
                            transform.translation.x,
                            transform.translation.y,
                            transform.translation.z,
                        ],
                    )
                })
            });
            for corner in 0..8 {
                let local = [
                    if corner & 1 == 0 { x0 } else { x1 },
                    if corner & 2 == 0 { y0 } else { y1 },
                    if corner & 4 == 0 { z0 } else { z1 },
                ];
                let xyz = match placed {
                    Some((axes, origin)) => std::array::from_fn(|axis| {
                        origin[axis]
                            + axes[0][axis] * local[0]
                            + axes[1][axis] * local[1]
                            + axes[2][axis] * local[2]
                    }),
                    None => local,
                };
                if !xyz.iter().all(|value: &f64| value.is_finite()) {
                    continue;
                }
                match &mut summary.bounds {
                    Some(bounds) => bounds.include(xyz),
                    None => summary.bounds = Some(Bounds { min: xyz, max: xyz }),
                }
            }
        }
        if let Some(pose) = pose {
            summary.poses.push(pose);
        }
    }
    Ok(summary)
}

/// Read the station photos listed in E57 metadata without decoding them.
/// Only pinhole photos with a pose are returned.
pub(crate) fn scan_images(path: &Path) -> Result<Vec<ScanImage>, LoadError> {
    let file = open_reader(path)?;
    // Stations are numbered like `scan_poses`: scans without a pose are skipped.
    let scans = file.pointclouds();
    let stations: Vec<_> = scans
        .iter()
        .zip(stations(&scans))
        .filter_map(|(scan, pose)| Some((scan.guid.clone(), pose?.position)))
        .collect();
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
    let scans = file.pointclouds();
    for (scan, pose) in scans.iter().zip(stations(&scans)) {
        if let Some(pose) = pose {
            pose_push(pose);
        }
        read_scan(&mut file, scan, push)?;
    }
    Ok(())
}

/// Stream the registered, valid points of one scan.
pub(crate) fn read_scan<T: Read + Seek>(
    file: &mut E57Reader<T>,
    scan: &PointCloud,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut points = file.pointcloud_simple(scan)?;
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
