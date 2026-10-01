//! Streaming vertex extraction from common mesh formats. Bounded resident
//! face loaders for rendering live in `obj_mesh`, `ply_mesh`, `mesh_formats`
//! and `dxf`.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use super::{LoadError, Point};

pub fn read_obj(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let reader = BufReader::new(File::open(path)?);
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        let mut fields = line.split_whitespace();
        if fields.next() != Some("v") {
            continue;
        }
        let values: Vec<&str> = fields.collect();
        let xyz = parse_xyz(&values, index + 1)?;
        let rgb = if values.len() >= 7 {
            parse_rgb(&values[4..7], index + 1)?
        } else if values.len() >= 6 {
            parse_rgb(&values[3..6], index + 1)?
        } else {
            None
        };
        push(Point {
            xyz,
            rgb,
            intensity: None,
            classification: None,
        })?;
    }
    Ok(())
}

pub fn read_off(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut lines = BufReader::new(File::open(path)?).lines();
    let header = next_data_line(&mut lines)?
        .ok_or_else(|| LoadError::InvalidData("empty OFF file".into()))?;
    let counts = if let Some(rest) = header.strip_prefix("OFF") {
        if rest.trim().is_empty() {
            next_data_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("missing OFF counts".into()))?
        } else {
            rest.trim().to_owned()
        }
    } else {
        return Err(LoadError::InvalidData("missing OFF header".into()));
    };
    let vertex_count = counts
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| LoadError::InvalidData("invalid OFF vertex count".into()))?;
    for index in 0..vertex_count {
        let line = next_data_line(&mut lines)?
            .ok_or_else(|| LoadError::InvalidData("truncated OFF vertices".into()))?;
        let values: Vec<&str> = line.split_whitespace().collect();
        push(Point {
            xyz: parse_xyz(&values, index + 1)?,
            rgb: if values.len() >= 6 {
                parse_rgb(&values[3..6], index + 1)?
            } else {
                None
            },
            intensity: None,
            classification: None,
        })?;
    }
    Ok(())
}

pub fn read_stl(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut header = [0u8; 84];
    if length >= 84 {
        file.read_exact(&mut header)?;
        let triangles = u32::from_le_bytes(header[80..84].try_into().unwrap()) as u64;
        if triangles
            .checked_mul(50)
            .and_then(|bytes| bytes.checked_add(84))
            == Some(length)
        {
            let mut record = [0u8; 50];
            for _ in 0..triangles {
                file.read_exact(&mut record)?;
                for offset in [12, 24, 36] {
                    let xyz = std::array::from_fn(|axis| {
                        let start = offset + axis * 4;
                        f32::from_le_bytes(record[start..start + 4].try_into().unwrap()) as f64
                    });
                    push(Point {
                        xyz,
                        rgb: None,
                        intensity: None,
                        classification: None,
                    })?;
                }
            }
            return Ok(());
        }
    }
    let reader = BufReader::new(File::open(path)?);
    for (index, line) in reader.lines().enumerate() {
        let line = line?;
        let mut fields = line.split_whitespace();
        if fields.next() != Some("vertex") {
            continue;
        }
        let values: Vec<&str> = fields.collect();
        push(Point {
            xyz: parse_xyz(&values, index + 1)?,
            rgb: None,
            intensity: None,
            classification: None,
        })?;
    }
    Ok(())
}

fn next_data_line(
    lines: &mut impl Iterator<Item = std::io::Result<String>>,
) -> Result<Option<String>, LoadError> {
    for line in lines {
        let line = line?;
        let value = line.split('#').next().unwrap_or("").trim();
        if !value.is_empty() {
            return Ok(Some(value.to_owned()));
        }
    }
    Ok(None)
}

fn parse_xyz(values: &[&str], line: usize) -> Result<[f64; 3], LoadError> {
    if values.len() < 3 {
        return Err(LoadError::InvalidData(format!(
            "vertex on line {line} has fewer than three coordinates"
        )));
    }
    let mut xyz = [0.0; 3];
    for axis in 0..3 {
        xyz[axis] = values[axis]
            .parse()
            .map_err(|_| LoadError::InvalidData(format!("invalid vertex on line {line}")))?;
    }
    Ok(xyz)
}

fn parse_rgb(values: &[&str], line: usize) -> Result<Option<[u8; 3]>, LoadError> {
    let mut color = [0.0; 3];
    for axis in 0..3 {
        color[axis] = values[axis]
            .parse::<f64>()
            .map_err(|_| LoadError::InvalidData(format!("invalid color on line {line}")))?;
    }
    let normalized = color.iter().all(|value| (0.0..=1.0).contains(value));
    Ok(Some(color.map(|value| {
        (value * if normalized { 255.0 } else { 1.0 }).clamp(0.0, 255.0) as u8
    })))
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_mesh_vertices() {
        let dir = tempfile::tempdir().unwrap();
        let obj = dir.path().join("sample.obj");
        std::fs::write(&obj, "v 1 2 3 0.5 0.0 1.0\nf 1 1 1\n").unwrap();
        let cloud = super::super::open(&obj, 10).unwrap();
        assert_eq!(cloud.total_points, 1);
        assert_eq!(cloud.points[0].rgb, Some([127, 0, 255]));

        let off = dir.path().join("sample.off");
        std::fs::write(&off, "OFF\n2 1 0\n1 2 3\n4 5 6\n3 0 1 2\n").unwrap();
        assert_eq!(super::super::open(&off, 10).unwrap().total_points, 2);

        let stl = dir.path().join("sample.stl");
        std::fs::write(
            &stl,
            "solid x\nfacet normal 0 0 1\nvertex 1 2 3\nendsolid\n",
        )
        .unwrap();
        assert_eq!(super::super::open(&stl, 10).unwrap().total_points, 1);
    }
}
