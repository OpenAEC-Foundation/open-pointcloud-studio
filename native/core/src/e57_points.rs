//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::path::Path;

use e57::{CartesianCoordinate, E57Reader, PointCloud};

use super::{quaternion_axes, LoadError, Point, ScanPose};

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
    let file = E57Reader::from_file(path)?;
    Ok(file
        .pointclouds()
        .iter()
        .enumerate()
        .filter_map(|(index, scan)| scan_pose(index, scan))
        .collect())
}

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    pose_push: &mut impl FnMut(ScanPose),
) -> Result<(), LoadError> {
    let mut file = E57Reader::from_file(path)?;
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
