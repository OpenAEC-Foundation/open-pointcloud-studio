//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::path::Path;

use e57::{CartesianCoordinate, E57Reader, Transform};

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

/// Match the E57 reader's local-to-file pose when a filtered export keeps
/// each scan's original local coordinates and scanner position.
pub(crate) fn world_xyz(local: [f64; 3], pose: Option<&Transform>) -> [f64; 3] {
    let Some(pose) = pose else { return local };
    let q = &pose.rotation;
    let [x, y, z] = local;
    [
        (q.w * q.w + q.x * q.x - q.y * q.y - q.z * q.z) * x
            + 2.0 * (q.x * q.y - q.w * q.z) * y
            + 2.0 * (q.x * q.z + q.w * q.y) * z
            + pose.translation.x,
        2.0 * (q.x * q.y + q.w * q.z) * x
            + (q.w * q.w + q.y * q.y - q.x * q.x - q.z * q.z) * y
            + 2.0 * (q.y * q.z - q.w * q.x) * z
            + pose.translation.y,
        2.0 * (q.x * q.z - q.w * q.y) * x
            + 2.0 * (q.y * q.z + q.w * q.x) * y
            + (q.w * q.w + q.z * q.z - q.x * q.x - q.y * q.y) * z
            + pose.translation.z,
    ]
}
