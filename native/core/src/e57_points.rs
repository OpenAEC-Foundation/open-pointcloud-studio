//! Stream all scans in an E57 file through the pure-Rust e57 decoder.

use std::path::Path;

use e57::{CartesianCoordinate, E57Reader};

use super::{LoadError, Point};

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut file = E57Reader::from_file(path)?;
    for scan in file.pointclouds() {
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
