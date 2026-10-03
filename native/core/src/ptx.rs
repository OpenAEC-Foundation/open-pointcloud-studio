//! Streaming reader for PTX scans, including multiple scan blocks.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::{LoadError, Point, ScanPose};

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
    scan_begin: &mut impl FnMut(Option<ScanPose>),
) -> Result<(), LoadError> {
    let mut lines = BufReader::new(File::open(path)?).lines();
    let mut scan = 0usize;
    while let Some(columns_line) = next_line(&mut lines)? {
        scan += 1;
        let columns = columns_line
            .parse::<usize>()
            .map_err(|_| LoadError::InvalidData(format!("invalid PTX columns in scan {scan}")))?;
        let rows = next_line(&mut lines)?
            .and_then(|value| value.parse::<usize>().ok())
            .ok_or_else(|| LoadError::InvalidData(format!("invalid PTX rows in scan {scan}")))?;
        let count = columns
            .checked_mul(rows)
            .filter(|value| *value > 0)
            .ok_or_else(|| LoadError::InvalidData(format!("invalid PTX size in scan {scan}")))?;
        // Scanner position and local basis are followed by the world transform.
        let scanner_line = next_line(&mut lines)?
            .ok_or_else(|| LoadError::InvalidData("truncated PTX scanner position".into()))?;
        let scanner = parse_values(&scanner_line, 3)?;
        let mut axes = [[0.0f64; 3]; 3];
        for axis in &mut axes {
            let basis_line = next_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("truncated PTX basis".into()))?;
            let values = parse_values(&basis_line, 3)?;
            axis.copy_from_slice(&values[..3]);
        }
        let axes = axes.map(|axis| {
            let norm = axis.iter().map(|value| value * value).sum::<f64>().sqrt();
            (norm.is_finite() && norm > f64::EPSILON).then(|| axis.map(|value| value / norm))
        });
        let axes = axes
            .iter()
            .all(Option::is_some)
            .then(|| axes.map(Option::unwrap));
        let mut transform = [[0.0f64; 4]; 4];
        for row in &mut transform {
            let line = next_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("truncated PTX transform".into()))?;
            let values = parse_values(&line, 4)?;
            row.copy_from_slice(&values[..4]);
        }
        // PTX's scanner position is already registered. The reference producer writes a
        // row-vector matrix with translation in the last row; also accept the
        // last-column variant emitted by some other producers.
        let scanner_position = [scanner[0], scanner[1], scanner[2]];
        let column_translation = transform[3][..3].iter().all(|value| value.abs() < 1e-12)
            && (0..3).any(|axis| transform[axis][3].abs() >= 1e-12);
        // Every block is reported, also one whose station is unusable, so
        // that its points are not taken for those of the block before it.
        scan_begin(
            scanner_position
                .iter()
                .all(|value| value.is_finite())
                .then(|| ScanPose {
                    label: format!("Scan {scan}"),
                    position: scanner_position,
                    axes,
                }),
        );
        for _ in 0..count {
            let line = next_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("truncated PTX point data".into()))?;
            let values = parse_values(&line, 3)?;
            if values[0] == 0.0 && values[1] == 0.0 && values[2] == 0.0 {
                continue;
            }
            let xyz = std::array::from_fn(|axis| {
                if column_translation {
                    transform[axis][0] * values[0]
                        + transform[axis][1] * values[1]
                        + transform[axis][2] * values[2]
                        + transform[axis][3]
                } else {
                    transform[0][axis] * values[0]
                        + transform[1][axis] * values[1]
                        + transform[2][axis] * values[2]
                        + transform[3][axis]
                }
            });
            let intensity = values.get(3).map(|value| {
                let normalized = if *value < 0.0 {
                    (*value + 2048.0) / 4095.0
                } else if *value <= 1.0 {
                    *value
                } else {
                    *value / 255.0
                };
                (normalized.clamp(0.0, 1.0) * 65535.0).round() as u16
            });
            let rgb = if values.len() >= 7 {
                Some(std::array::from_fn(|axis| {
                    values[axis + 4].clamp(0.0, 255.0) as u8
                }))
            } else {
                None
            };
            push(Point {
                xyz,
                rgb,
                intensity,
                classification: None,
            })?;
        }
    }
    if scan == 0 {
        return Err(LoadError::InvalidData("PTX file has no scans".into()));
    }
    Ok(())
}

fn next_line(
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
) -> Result<Option<String>, LoadError> {
    for line in lines {
        let line = line?;
        let line = line.trim();
        if !line.is_empty() {
            return Ok(Some(line.to_owned()));
        }
    }
    Ok(None)
}

fn parse_values(line: &str, minimum: usize) -> Result<Vec<f64>, LoadError> {
    let values: Vec<f64> = line
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| LoadError::InvalidData("invalid PTX number".into()))?;
    if values.len() < minimum {
        return Err(LoadError::InvalidData("PTX line has too few values".into()));
    }
    Ok(values)
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::ScanRange;

    /// Three scan blocks: three points seen from (10, 20, 30) after a row
    /// that holds no point, two points of a block whose station is not a
    /// position, and one point seen from (40, 50, 60).
    pub(crate) const THREE_BLOCKS: &str = concat!(
        "2\n2\n10 20 30\n1 0 0\n0 1 0\n0 0 1\n",
        "1 0 0 0\n0 1 0 0\n0 0 1 0\n10 20 30 1\n",
        "0 0 0 0\n1 0 0 0.5\n2 0 0 0.5\n3 0 0 0.5\n",
        "1\n2\nnan nan nan\n1 0 0\n0 1 0\n0 0 1\n",
        "1 0 0 0\n0 1 0 0\n0 0 1 0\n20 20 30 1\n",
        "1 0 0 0.5\n2 0 0 0.5\n",
        "1\n1\n40 50 60\n1 0 0\n0 1 0\n0 0 1\n",
        "1 0 0 0\n0 1 0 0\n0 0 1 0\n40 50 60 1\n",
        "1 0 0 0.5\n",
    );

    /// The ranges a full pass over `THREE_BLOCKS` records.
    pub(crate) fn three_block_ranges() -> Vec<ScanRange> {
        [(0, Some(0)), (3, None), (5, Some(1))]
            .map(|(first_ordinal, station)| ScanRange {
                first_ordinal,
                station,
            })
            .to_vec()
    }

    #[test]
    fn every_scan_block_begins_a_scan_range() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("three-blocks.ptx");
        std::fs::write(&path, THREE_BLOCKS).unwrap();
        let cloud = super::super::open(&path, 10).unwrap();
        assert_eq!(cloud.total_points, 6);
        assert_eq!(cloud.scan_poses.len(), 2);
        assert_eq!(cloud.scan_ranges, three_block_ranges());
        assert_eq!(cloud.point_ordinals, [0, 1, 2, 3, 4, 5]);
        // Every point lies along X from the origin of its own block.
        for (point, ordinal) in cloud.points.iter().zip(&cloud.point_ordinals) {
            let station = cloud.station_pose(*ordinal).map(|pose| pose.position);
            let expected = match point.xyz[0] {
                x if x < 15.0 => Some([10.0, 20.0, 30.0]),
                x if x < 25.0 => None,
                _ => Some([40.0, 50.0, 60.0]),
            };
            assert_eq!(station, expected, "ordinal {ordinal}");
        }
        assert!(cloud.station_pose(6).is_none());
    }

    #[test]
    fn reads_multiple_transformed_scans() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.ptx");
        let standard = "1\n2\n10 20 30\n0 1 0\n-1 0 0\n0 0 1\n0 1 0 0\n-1 0 0 0\n0 0 1 0\n10 20 30 1\n0 0 0 0\n1 2 3 0.5 10 20 30\n";
        let legacy = "1\n2\n10 20 30\n1 0 0\n0 1 0\n0 0 1\n1 0 0 10\n0 1 0 20\n0 0 1 30\n0 0 0 1\n0 0 0 0\n1 2 3 0.5 10 20 30\n";
        std::fs::write(&path, format!("{standard}{legacy}")).unwrap();
        let cloud = super::super::open(&path, 10).unwrap();
        assert_eq!(cloud.total_points, 2);
        assert_eq!(cloud.points[0].xyz, [8.0, 21.0, 33.0]);
        assert_eq!(cloud.points[1].xyz, [11.0, 22.0, 33.0]);
        assert_eq!(cloud.points[0].rgb, Some([10, 20, 30]));
        assert_eq!(cloud.scan_poses.len(), 2);
        assert_eq!(cloud.scan_poses[0].position, [10.0, 20.0, 30.0]);
        assert_eq!(cloud.scan_poses[1].position, [10.0, 20.0, 30.0]);
        assert_eq!(
            cloud.scan_poses[0].axes,
            Some([[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]])
        );
        assert_eq!(
            cloud.scan_poses[1].axes,
            Some([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]])
        );
    }
}
