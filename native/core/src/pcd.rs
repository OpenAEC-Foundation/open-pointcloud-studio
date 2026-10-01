//! Streaming PCD reader, including disk-backed LZF binary-compressed records.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use memmap2::MmapOptions;

use super::{LoadError, Point};

struct Field {
    name: String,
    size: usize,
    kind: char,
    offset: usize,
    column: usize,
    span: usize,
}

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut reader = BufReader::new(File::open(path)?);
    let (mut names, mut sizes, mut kinds, mut counts) = (vec![], vec![], vec![], vec![]);
    let (mut width, mut height, mut points) = (0u64, 1u64, 0u64);
    let mode = loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Err(invalid("PCD header has no DATA line"));
        }
        let mut words = line.split_whitespace();
        let Some(key) = words.next() else { continue };
        match key.to_ascii_uppercase().as_str() {
            "FIELDS" => names = words.map(str::to_ascii_lowercase).collect(),
            "SIZE" => sizes = words.map(parse_usize).collect::<Result<_, _>>()?,
            "TYPE" => {
                kinds = words
                    .map(|word| word.chars().next().unwrap_or(' '))
                    .collect()
            }
            "COUNT" => counts = words.map(parse_usize).collect::<Result<_, _>>()?,
            "WIDTH" => width = parse_u64(words.next())?,
            "HEIGHT" => height = parse_u64(words.next())?,
            "POINTS" => points = parse_u64(words.next())?,
            "DATA" => break words.next().unwrap_or("").to_ascii_lowercase(),
            _ => {}
        }
    };
    if points == 0 {
        points = width
            .checked_mul(height)
            .ok_or_else(|| invalid("PCD point count overflow"))?;
    }
    if names.is_empty() || points == 0 {
        return Err(invalid("PCD file has no points or fields"));
    }
    let (mut record_size, mut column) = (0usize, 0usize);
    let mut fields = Vec::with_capacity(names.len());
    for (index, name) in names.into_iter().enumerate() {
        let size = *sizes.get(index).unwrap_or(&4);
        let count = *counts.get(index).unwrap_or(&1);
        let kind = *kinds.get(index).unwrap_or(&'F');
        if count == 0 || !matches!((kind, size), ('F', 4 | 8) | ('U' | 'I', 1 | 2 | 4 | 8)) {
            return Err(invalid("unsupported PCD field type"));
        }
        let span = size
            .checked_mul(count)
            .ok_or_else(|| invalid("PCD record overflow"))?;
        fields.push(Field {
            name,
            size,
            kind,
            offset: record_size,
            column,
            span,
        });
        record_size = record_size
            .checked_add(span)
            .ok_or_else(|| invalid("PCD record overflow"))?;
        column += count;
    }
    if !["x", "y", "z"]
        .iter()
        .all(|name| fields.iter().any(|field| field.name == *name))
    {
        return Err(invalid("PCD file is missing x, y or z"));
    }
    if record_size > 1_048_576 {
        return Err(invalid("PCD record is too large"));
    }
    match mode.as_str() {
        "ascii" => {
            let mut line = String::new();
            for _ in 0..points {
                loop {
                    line.clear();
                    if reader.read_line(&mut line)? == 0 {
                        return Err(invalid("truncated PCD ASCII data"));
                    }
                    if !line.trim().is_empty() {
                        break;
                    }
                }
                let values: Vec<&str> = line.split_whitespace().collect();
                let get = |name: &str| -> Result<Option<f64>, LoadError> {
                    let Some(field) = fields.iter().find(|field| field.name == name) else {
                        return Ok(None);
                    };
                    values
                        .get(field.column)
                        .ok_or_else(|| invalid("short PCD ASCII record"))?
                        .parse()
                        .map(Some)
                        .map_err(|_| invalid("invalid PCD number"))
                };
                let rgb = packed_field(&fields)
                    .map(|field| -> Result<[u8; 3], LoadError> {
                        let raw = *values
                            .get(field.column)
                            .ok_or_else(|| invalid("short PCD RGB record"))?;
                        let bits = if field.kind == 'F' {
                            raw.parse::<f32>()
                                .map_err(|_| invalid("invalid PCD RGB"))?
                                .to_bits()
                        } else {
                            raw.parse::<u32>().map_err(|_| invalid("invalid PCD RGB"))?
                        };
                        Ok(unpack_rgb(bits))
                    })
                    .transpose()?
                    .or(separate_rgb(&get)?);
                emit(&get, rgb, push)?;
            }
        }
        "binary" => {
            let mut record = vec![0u8; record_size];
            for _ in 0..points {
                reader.read_exact(&mut record)?;
                let get = |name: &str| -> Result<Option<f64>, LoadError> {
                    fields
                        .iter()
                        .find(|field| field.name == name)
                        .map(|field| {
                            number(&record[field.offset..field.offset + field.size], field)
                        })
                        .transpose()
                };
                let rgb = packed_field(&fields)
                    .map(|field| -> Result<[u8; 3], LoadError> {
                        if field.size != 4 {
                            return Err(invalid("unsupported PCD RGB size"));
                        }
                        Ok(unpack_rgb(u32::from_le_bytes(
                            record[field.offset..field.offset + 4].try_into().unwrap(),
                        )))
                    })
                    .transpose()?
                    .or(separate_rgb(&get)?);
                emit(&get, rgb, push)?;
            }
        }
        "binary_compressed" => {
            read_compressed(&mut reader, &fields, points, push)?;
        }
        _ => return Err(invalid("unsupported PCD DATA mode")),
    }
    Ok(())
}

fn read_compressed(
    reader: &mut impl Read,
    fields: &[Field],
    points: u64,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut header = [0u8; 8];
    reader.read_exact(&mut header)?;
    let compressed_len = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
    let uncompressed_len = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
    let points = usize::try_from(points).map_err(|_| invalid("PCD point count overflow"))?;
    let mut planes = Vec::with_capacity(fields.len());
    let mut expected_len = 0usize;
    for field in fields {
        if field.name == "_" {
            planes.push(None);
            continue;
        }
        planes.push(Some(expected_len));
        expected_len = expected_len
            .checked_add(
                field
                    .span
                    .checked_mul(points)
                    .ok_or_else(|| invalid("PCD payload overflow"))?,
            )
            .ok_or_else(|| invalid("PCD payload overflow"))?;
    }
    if uncompressed_len != expected_len || compressed_len == 0 {
        return Err(invalid("PCD compressed payload has an invalid size"));
    }
    let mut output = tempfile::tempfile()?;
    {
        let mut writer = std::io::BufWriter::new(&mut output);
        decompress_lzf(reader, compressed_len, uncompressed_len, &mut writer)?;
        writer.flush()?;
    }
    // The temporary file remains open and immutable for the lifetime of the mapping.
    let data = unsafe { MmapOptions::new().map(&output)? };
    for point_index in 0..points {
        let bytes_for = |field: &Field| -> Result<&[u8], LoadError> {
            let field_index = fields
                .iter()
                .position(|candidate| std::ptr::eq(candidate, field))
                .ok_or_else(|| invalid("PCD field lookup failed"))?;
            let plane =
                planes[field_index].ok_or_else(|| invalid("PCD padding field has no payload"))?;
            let start = plane + point_index * field.span;
            Ok(&data[start..start + field.size])
        };
        let get = |name: &str| -> Result<Option<f64>, LoadError> {
            fields
                .iter()
                .find(|field| field.name == name)
                .map(|field| number(bytes_for(field)?, field))
                .transpose()
        };
        let rgb = packed_field(fields)
            .map(|field| -> Result<[u8; 3], LoadError> {
                if field.size != 4 {
                    return Err(invalid("unsupported PCD RGB size"));
                }
                Ok(unpack_rgb(u32::from_le_bytes(
                    bytes_for(field)?.try_into().unwrap(),
                )))
            })
            .transpose()?
            .or(separate_rgb(&get)?);
        emit(&get, rgb, push)?;
    }
    Ok(())
}

fn read_lzf_byte(reader: &mut impl Read, remaining: &mut usize) -> Result<u8, LoadError> {
    if *remaining == 0 {
        return Err(invalid("truncated PCD LZF stream"));
    }
    let mut byte = [0u8];
    reader.read_exact(&mut byte)?;
    *remaining -= 1;
    Ok(byte[0])
}

fn decompress_lzf(
    reader: &mut impl Read,
    compressed_len: usize,
    uncompressed_len: usize,
    output: &mut impl Write,
) -> Result<(), LoadError> {
    let mut remaining = compressed_len;
    let mut written = 0usize;
    let mut history = [0u8; 8192];
    while remaining > 0 {
        let control = read_lzf_byte(reader, &mut remaining)?;
        if control < 32 {
            let length = control as usize + 1;
            if length > remaining || length > uncompressed_len.saturating_sub(written) {
                return Err(invalid("invalid PCD LZF literal length"));
            }
            let mut bytes = [0u8; 32];
            reader.read_exact(&mut bytes[..length])?;
            remaining -= length;
            output.write_all(&bytes[..length])?;
            for &byte in &bytes[..length] {
                history[written % history.len()] = byte;
                written += 1;
            }
        } else {
            let mut length = (control >> 5) as usize;
            if length == 7 {
                length += read_lzf_byte(reader, &mut remaining)? as usize;
            }
            let distance = (((control & 31) as usize) << 8)
                + read_lzf_byte(reader, &mut remaining)? as usize
                + 1;
            length += 2;
            if distance > written || length > uncompressed_len.saturating_sub(written) {
                return Err(invalid("invalid PCD LZF back-reference"));
            }
            let mut bytes = [0u8; 264];
            for byte in &mut bytes[..length] {
                *byte = history[(written - distance) % history.len()];
                history[written % history.len()] = *byte;
                written += 1;
            }
            output.write_all(&bytes[..length])?;
        }
    }
    if written != uncompressed_len {
        return Err(invalid("PCD LZF output size does not match header"));
    }
    Ok(())
}

fn emit(
    get: &impl Fn(&str) -> Result<Option<f64>, LoadError>,
    rgb: Option<[u8; 3]>,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let xyz = [get("x")?.unwrap(), get("y")?.unwrap(), get("z")?.unwrap()];
    if !xyz.iter().all(|value| value.is_finite()) {
        return Ok(());
    }
    push(Point {
        xyz,
        rgb,
        intensity: get("intensity")?.map(|value| {
            (if value <= 1.0 { value * 65535.0 } else { value }).clamp(0.0, 65535.0) as u16
        }),
        classification: get("label")?
            .or(get("classification")?)
            .map(|value| value.clamp(0.0, 255.0) as u8),
    })
}

fn packed_field(fields: &[Field]) -> Option<&Field> {
    fields
        .iter()
        .find(|field| field.name == "rgb" || field.name == "rgba")
}

fn separate_rgb(
    get: &impl Fn(&str) -> Result<Option<f64>, LoadError>,
) -> Result<Option<[u8; 3]>, LoadError> {
    let (Some(r), Some(g), Some(b)) = (get("r")?, get("g")?, get("b")?) else {
        return Ok(None);
    };
    Ok(Some([
        r.clamp(0.0, 255.0) as u8,
        g.clamp(0.0, 255.0) as u8,
        b.clamp(0.0, 255.0) as u8,
    ]))
}

fn number(bytes: &[u8], field: &Field) -> Result<f64, LoadError> {
    Ok(match (field.kind, field.size) {
        ('F', 4) => f32::from_le_bytes(bytes.try_into().unwrap()) as f64,
        ('F', 8) => f64::from_le_bytes(bytes.try_into().unwrap()),
        ('U', 1) => bytes[0] as f64,
        ('U', 2) => u16::from_le_bytes(bytes.try_into().unwrap()) as f64,
        ('U', 4) => u32::from_le_bytes(bytes.try_into().unwrap()) as f64,
        ('U', 8) => u64::from_le_bytes(bytes.try_into().unwrap()) as f64,
        ('I', 1) => (bytes[0] as i8) as f64,
        ('I', 2) => i16::from_le_bytes(bytes.try_into().unwrap()) as f64,
        ('I', 4) => i32::from_le_bytes(bytes.try_into().unwrap()) as f64,
        ('I', 8) => i64::from_le_bytes(bytes.try_into().unwrap()) as f64,
        _ => return Err(invalid("unsupported PCD number")),
    })
}

fn unpack_rgb(bits: u32) -> [u8; 3] {
    [(bits >> 16) as u8, (bits >> 8) as u8, bits as u8]
}
fn parse_usize(value: &str) -> Result<usize, LoadError> {
    value.parse().map_err(|_| invalid("invalid PCD header"))
}
fn parse_u64(value: Option<&str>) -> Result<u64, LoadError> {
    value
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| invalid("invalid PCD count"))
}
fn invalid(reason: &str) -> LoadError {
    LoadError::InvalidData(reason.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_ascii_and_binary() {
        let dir = tempfile::tempdir().unwrap();
        let ascii = dir.path().join("ascii.pcd");
        std::fs::write(&ascii, "FIELDS x y z r g b\nSIZE 4 4 4 1 1 1\nTYPE F F F U U U\nWIDTH 1\nHEIGHT 1\nPOINTS 1\nDATA ascii\n1 2 3 10 20 30\n").unwrap();
        let cloud = super::super::open(&ascii, 10).unwrap();
        assert_eq!(cloud.points[0].rgb, Some([10, 20, 30]));
        let binary = dir.path().join("binary.pcd");
        let mut bytes = b"FIELDS x y z rgb\nSIZE 4 4 4 4\nTYPE F F F F\nWIDTH 1\nHEIGHT 1\nPOINTS 1\nDATA binary\n".to_vec();
        for value in [1.0f32, 2.0, 3.0] {
            bytes.extend(value.to_le_bytes());
        }
        bytes.extend(0x00112233u32.to_le_bytes());
        std::fs::write(&binary, bytes).unwrap();
        let cloud = super::super::open(&binary, 10).unwrap();
        assert_eq!(cloud.points[0].rgb, Some([0x11, 0x22, 0x33]));
    }

    #[test]
    fn reads_field_major_lzf_and_rejects_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let compressed = dir.path().join("compressed.pcd");
        let mut payload = Vec::new();
        for values in [[1.0f32, 4.0], [2.0, 5.0], [3.0, 6.0]] {
            for value in values {
                payload.extend(value.to_le_bytes());
            }
        }
        for rgb in [0x00112233u32, 0x00445566] {
            payload.extend(rgb.to_le_bytes());
        }
        assert_eq!(payload.len(), 32);
        let mut bytes = b"FIELDS x y z rgb\nSIZE 4 4 4 4\nTYPE F F F F\nWIDTH 2\nHEIGHT 1\nPOINTS 2\nDATA binary_compressed\n".to_vec();
        bytes.extend(33u32.to_le_bytes());
        bytes.extend(32u32.to_le_bytes());
        bytes.push(31); // One LZF literal run of 32 bytes.
        bytes.extend(payload);
        std::fs::write(&compressed, bytes).unwrap();
        let cloud = super::super::open(&compressed, 10).unwrap();
        assert_eq!(cloud.total_points, 2);
        assert_eq!(cloud.points[0].xyz, [1.0, 2.0, 3.0]);
        assert_eq!(cloud.points[1].xyz, [4.0, 5.0, 6.0]);
        assert_eq!(cloud.points[1].rgb, Some([0x44, 0x55, 0x66]));

        let mut expanded = Vec::new();
        super::decompress_lzf(&mut &[2, b'A', b'B', b'C', 128, 2][..], 6, 9, &mut expanded)
            .unwrap();
        assert_eq!(expanded, b"ABCABCABC");
        assert!(super::decompress_lzf(&mut &[128, 2][..], 2, 4, &mut Vec::new()).is_err());
    }
}
