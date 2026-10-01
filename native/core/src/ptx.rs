//! Streaming reader for PTX scans, including multiple scan blocks.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::{LoadError, Point};

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
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
        for _ in 0..4 {
            next_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("truncated PTX header".into()))?;
        }
        let mut transform = [[0.0f64; 4]; 4];
        for row in &mut transform {
            let line = next_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("truncated PTX transform".into()))?;
            let values = parse_values(&line, 4)?;
            row.copy_from_slice(&values[..4]);
        }
        for _ in 0..count {
            let line = next_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("truncated PTX point data".into()))?;
            let values = parse_values(&line, 3)?;
            if values[0] == 0.0 && values[1] == 0.0 && values[2] == 0.0 {
                continue;
            }
            let xyz = std::array::from_fn(|axis| {
                transform[axis][0] * values[0]
                    + transform[axis][1] * values[1]
                    + transform[axis][2] * values[2]
                    + transform[axis][3]
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
mod tests {
    #[test]
    fn reads_multiple_transformed_scans() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.ptx");
        let scan = "1\n2\n0 0 0\n1 0 0\n0 1 0\n0 0 1\n1 0 0 10\n0 1 0 20\n0 0 1 30\n0 0 0 1\n0 0 0 0\n1 2 3 0.5 10 20 30\n";
        std::fs::write(&path, format!("{scan}{scan}")).unwrap();
        let cloud = super::super::open(&path, 10).unwrap();
        assert_eq!(cloud.total_points, 2);
        assert_eq!(cloud.points[0].xyz, [11.0, 22.0, 33.0]);
        assert_eq!(cloud.points[0].rgb, Some([10, 20, 30]));
    }
}
