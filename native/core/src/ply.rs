use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use super::{LoadError, Point};

#[derive(Clone, Copy)]
enum Encoding {
    Ascii,
    BinaryLittleEndian,
}

#[derive(Clone)]
struct Property {
    name: String,
    data_type: ScalarType,
}

#[derive(Clone, Copy)]
enum ScalarType {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl ScalarType {
    fn parse(name: &str) -> Result<Self, LoadError> {
        match name {
            "char" | "int8" => Ok(Self::I8),
            "uchar" | "uint8" => Ok(Self::U8),
            "short" | "int16" => Ok(Self::I16),
            "ushort" | "uint16" => Ok(Self::U16),
            "int" | "int32" => Ok(Self::I32),
            "uint" | "uint32" => Ok(Self::U32),
            "float" | "float32" => Ok(Self::F32),
            "double" | "float64" => Ok(Self::F64),
            _ => Err(LoadError::InvalidData(format!(
                "unsupported PLY property type: {name}"
            ))),
        }
    }

    fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    fn read(self, bytes: &[u8]) -> f64 {
        match self {
            Self::I8 => i8::from_le_bytes([bytes[0]]) as f64,
            Self::U8 => bytes[0] as f64,
            Self::I16 => i16::from_le_bytes(bytes[..2].try_into().unwrap()) as f64,
            Self::U16 => u16::from_le_bytes(bytes[..2].try_into().unwrap()) as f64,
            Self::I32 => i32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            Self::U32 => u32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            Self::F32 => f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
            Self::F64 => f64::from_le_bytes(bytes[..8].try_into().unwrap()),
        }
    }
}

pub(super) fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim() != "ply" {
        return Err(LoadError::InvalidData("missing PLY signature".into()));
    }

    let mut encoding = None;
    let mut vertex_count = None;
    let mut in_vertex = false;
    let mut preceding_elements = false;
    let mut properties = Vec::new();
    let mut header_lines = 1;

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(LoadError::InvalidData(
                "PLY header has no end_header".into(),
            ));
        }
        header_lines += 1;
        if header_lines > 1024 {
            return Err(LoadError::InvalidData("PLY header is too long".into()));
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        match fields.as_slice() {
            ["format", "ascii", _] => encoding = Some(Encoding::Ascii),
            ["format", "binary_little_endian", _] => encoding = Some(Encoding::BinaryLittleEndian),
            ["format", format, _] => {
                return Err(LoadError::InvalidData(format!(
                    "unsupported PLY encoding: {format}"
                )))
            }
            ["element", "vertex", count] => {
                if preceding_elements {
                    return Err(LoadError::InvalidData(
                        "PLY vertex element must be first".into(),
                    ));
                }
                vertex_count = Some(
                    count
                        .parse::<usize>()
                        .map_err(|_| LoadError::InvalidData("invalid PLY vertex count".into()))?,
                );
                in_vertex = true;
            }
            ["element", _, count] => {
                preceding_elements |=
                    vertex_count.is_none() && count.parse::<usize>().unwrap_or(1) > 0;
                in_vertex = false;
            }
            ["property", "list", ..] if in_vertex => {
                return Err(LoadError::InvalidData(
                    "list properties in PLY vertices are unsupported".into(),
                ));
            }
            ["property", data_type, name] if in_vertex => {
                properties.push(Property {
                    name: name.to_ascii_lowercase(),
                    data_type: ScalarType::parse(data_type)?,
                });
            }
            ["end_header"] => break,
            _ => {}
        }
    }

    let encoding = encoding.ok_or_else(|| LoadError::InvalidData("missing PLY encoding".into()))?;
    let vertex_count =
        vertex_count.ok_or_else(|| LoadError::InvalidData("missing PLY vertex element".into()))?;
    if vertex_count == 0 {
        return Err(LoadError::InvalidData("PLY has no vertices".into()));
    }
    for axis in ["x", "y", "z"] {
        if !properties.iter().any(|property| property.name == axis) {
            return Err(LoadError::InvalidData(format!(
                "PLY vertex has no {axis} property"
            )));
        }
    }

    match encoding {
        Encoding::Ascii => {
            for index in 0..vertex_count {
                line.clear();
                if reader.read_line(&mut line)? == 0 {
                    return Err(LoadError::InvalidData(format!(
                        "PLY ended after {index} vertices"
                    )));
                }
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() != properties.len() {
                    return Err(LoadError::InvalidData(format!(
                        "PLY vertex {} has {} properties; expected {}",
                        index + 1,
                        fields.len(),
                        properties.len()
                    )));
                }
                let values = fields
                    .iter()
                    .map(|field| {
                        field.parse::<f64>().map_err(|_| {
                            LoadError::InvalidData(format!(
                                "invalid PLY scalar in vertex {}",
                                index + 1
                            ))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                push(to_point(&properties, &values)?)?;
            }
        }
        Encoding::BinaryLittleEndian => {
            let record_size: usize = properties
                .iter()
                .map(|property| property.data_type.size())
                .sum();
            if record_size > 4096 {
                return Err(LoadError::InvalidData(
                    "PLY vertex record is too large".into(),
                ));
            }
            let mut record = vec![0u8; record_size];
            let mut values = vec![0.0; properties.len()];
            for _ in 0..vertex_count {
                reader.read_exact(&mut record)?;
                let mut offset = 0;
                for (index, property) in properties.iter().enumerate() {
                    let size = property.data_type.size();
                    values[index] = property.data_type.read(&record[offset..offset + size]);
                    offset += size;
                }
                push(to_point(&properties, &values)?)?;
            }
        }
    }
    Ok(())
}

fn to_point(properties: &[Property], values: &[f64]) -> Result<Point, LoadError> {
    let scalar = |names: &[&str]| -> Option<(f64, ScalarType)> {
        properties.iter().zip(values).find_map(|(property, value)| {
            names
                .contains(&property.name.as_str())
                .then_some((*value, property.data_type))
        })
    };
    let value = |names: &[&str]| scalar(names).map(|(value, _)| value);
    let xyz = [
        value(&["x"]).unwrap(),
        value(&["y"]).unwrap(),
        value(&["z"]).unwrap(),
    ];
    let rgb = match (
        value(&["red", "r"]),
        value(&["green", "g"]),
        value(&["blue", "b"]),
    ) {
        (Some(red), Some(green), Some(blue)) => {
            let channels = [red, green, blue];
            if channels
                .iter()
                .any(|channel| !channel.is_finite() || *channel < 0.0 || *channel > 65535.0)
            {
                return Err(LoadError::InvalidData("invalid PLY RGB channel".into()));
            }
            if channels.iter().all(|channel| *channel <= 255.0) {
                Some(channels.map(|channel| channel as u8))
            } else {
                Some(channels.map(|channel| (channel / 257.0).round() as u8))
            }
        }
        _ => None,
    };
    let intensity = scalar(&["intensity", "scalar_intensity"]).map(|(value, data_type)| {
        if matches!(data_type, ScalarType::F32 | ScalarType::F64) && value <= 1.0 {
            (value.clamp(0.0, 1.0) * 65535.0).round() as u16
        } else {
            value.clamp(0.0, 65535.0) as u16
        }
    });
    let classification =
        value(&["classification", "class"]).map(|value| value.clamp(0.0, 255.0) as u8);
    Ok(Point {
        xyz,
        rgb,
        intensity,
        classification,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_ascii_and_binary_ply() {
        for binary in [false, true] {
            let path = std::env::temp_dir().join(format!(
                "pointcloud-ply-{}-{binary}.ply",
                std::process::id()
            ));
            let format = if binary {
                "binary_little_endian"
            } else {
                "ascii"
            };
            let header = format!("ply\nformat {format} 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n");
            let mut bytes = header.into_bytes();
            if binary {
                for coordinate in [1.0_f32, 2.0, 3.0] {
                    bytes.extend_from_slice(&coordinate.to_le_bytes());
                }
                bytes.extend_from_slice(&[10, 20, 30]);
            } else {
                bytes.extend_from_slice(b"1 2 3 10 20 30\n");
            }
            std::fs::write(&path, bytes).unwrap();
            let cloud = super::super::open(&path, 10).unwrap();
            std::fs::remove_file(path).unwrap();
            assert_eq!(cloud.total_points, 1);
            assert_eq!(cloud.points[0].xyz, [1.0, 2.0, 3.0]);
            assert_eq!(cloud.points[0].rgb, Some([10, 20, 30]));
        }
    }
}
