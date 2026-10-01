//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::path::Path;

use e57::{CartesianCoordinate, E57Reader};

use super::{quaternion_axes, LoadError, Point, ScanPose};

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    pose_push: &mut impl FnMut(ScanPose),
) -> Result<(), LoadError> {
    let mut file = E57Reader::from_file(path)?;
    for (index, scan) in file.pointclouds().into_iter().enumerate() {
        if let Some(transform) = &scan.transform {
            let position = [
                transform.translation.x,
                transform.translation.y,
                transform.translation.z,
            ];
            if position.iter().all(|value| value.is_finite()) {
                pose_push(ScanPose {
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
                });
            }
        }
        let mut points = file.pointcloud_simple(&scan)?;
        points.spherical_to_cartesian(true);
        points.apply_pose(true);
        for point in points {
            let point = point?;
            let CartesianCoordinate::Valid { x, y, z } = point.cartesian else {
                continue;
            };
            push(Point {
                xyz: [x, y, z],
                rgb: point.color.map(|color| {
                    [color.red, color.green, color.blue]
                        .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
                }),
                intensity: point
                    .intensity
                    .map(|value| (value.clamp(0.0, 1.0) * 65535.0).round() as u16),
                classification: None,
            })?;
        }
    }
    Ok(())
}
