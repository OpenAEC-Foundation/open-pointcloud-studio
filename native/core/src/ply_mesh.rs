//! Bounded PLY face loader for the native mesh renderer.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use super::obj_mesh::{MeshGeometry, MAX_TRIANGLES, MAX_VERTICES};
use super::{LoadError, SourceStamp};

const MAX_FACE_ITEMS: usize = 100_000;

#[derive(Clone, Copy)]
enum Encoding {
    Ascii,
    BinaryLittleEndian,
}

#[derive(Clone, Copy)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
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
                "unsupported PLY mesh scalar: {name}"
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

#[derive(Clone)]
enum Property {
    Scalar(String, Scalar),
    List(String, Scalar, Scalar),
}

#[derive(Default)]
struct Header {
    encoding: Option<Encoding>,
    vertices: Option<usize>,
    faces: Option<usize>,
    vertex_properties: Vec<Property>,
    face_properties: Vec<Property>,
}

/// Load polygon faces from an ASCII or little-endian binary PLY file.
/// Meshes larger than the GPU's bounded resident geometry are rejected.
pub fn read_ply_mesh(path: impl AsRef<Path>) -> Result<Option<MeshGeometry>, LoadError> {
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let mut reader = BufReader::new(File::open(path)?);
    let header = read_header(&mut reader)?;
    let encoding = header
        .encoding
        .ok_or_else(|| LoadError::InvalidData("PLY mesh encoding is missing".into()))?;
    let vertex_count = header
        .vertices
        .ok_or_else(|| LoadError::InvalidData("PLY mesh has no vertex element".into()))?;
    let Some(face_count) = header.faces.filter(|count| *count > 0) else {
        return Ok(None);
    };
    if vertex_count == 0 {
        return Err(LoadError::InvalidData("PLY mesh has no vertices".into()));
    }
    if vertex_count > MAX_VERTICES || face_count > MAX_TRIANGLES {
        return Err(LoadError::InvalidData("PLY mesh limit exceeded".into()));
    }
    let axes = ["x", "y", "z"]
        .into_iter()
        .map(|axis| {
            header
                .vertex_properties
                .iter()
                .position(|property| matches!(property, Property::Scalar(name, _) if name == axis))
                .ok_or_else(|| LoadError::InvalidData(format!("PLY vertex has no {axis} property")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !header
        .face_properties
        .iter()
        .any(|property| matches!(property, Property::List(name, _, _) if is_vertex_indices(name)))
    {
        return Err(LoadError::InvalidData(
            "PLY face has no vertex_indices list".into(),
        ));
    }
    let mut mesh = MeshGeometry {
        vertices: Vec::with_capacity(vertex_count),
        triangles: Vec::with_capacity(face_count.min(MAX_TRIANGLES)),
    };
    match encoding {
        Encoding::Ascii => {
            let mut line = String::new();
            for _ in 0..vertex_count {
                read_data_line(&mut reader, &mut line)?;
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() != header.vertex_properties.len() {
                    return Err(LoadError::InvalidData("invalid PLY mesh vertex".into()));
                }
                let mut xyz = [0.0; 3];
                for (axis, property) in axes.iter().copied().enumerate() {
                    xyz[axis] = fields[property]
                        .parse::<f64>()
                        .map_err(|_| LoadError::InvalidData("invalid PLY coordinate".into()))?;
                }
                add_vertex(&mut mesh, xyz)?;
            }
            for _ in 0..face_count {
                read_data_line(&mut reader, &mut line)?;
                let fields: Vec<_> = line.split_whitespace().collect();
                let mut cursor = 0usize;
                let mut indices = None;
                for property in &header.face_properties {
                    match property {
                        Property::Scalar(_, _) => {
                            cursor = cursor.checked_add(1).ok_or_else(bad_face)?;
                        }
                        Property::List(name, _, _) => {
                            let count = fields
                                .get(cursor)
                                .ok_or_else(bad_face)?
                                .parse::<usize>()
                                .map_err(|_| bad_face())?;
                            if count > MAX_FACE_ITEMS {
                                return Err(LoadError::InvalidData(
                                    "PLY face list is too long".into(),
                                ));
                            }
                            cursor = cursor.checked_add(1).ok_or_else(bad_face)?;
                            let end = cursor.checked_add(count).ok_or_else(bad_face)?;
                            let items = fields.get(cursor..end).ok_or_else(bad_face)?;
                            if is_vertex_indices(name) {
                                indices = Some(
                                    items
                                        .iter()
                                        .map(|item| item.parse::<u32>().map_err(|_| bad_face()))
                                        .collect::<Result<Vec<_>, _>>()?,
                                );
                            }
                            cursor = end;
                        }
                    }
                }
                if cursor != fields.len() {
                    return Err(bad_face());
                }
                add_face(&mut mesh, indices.ok_or_else(bad_face)?)?;
            }
        }
        Encoding::BinaryLittleEndian => {
            for _ in 0..vertex_count {
                let mut xyz = [0.0; 3];
                for (property_index, property) in header.vertex_properties.iter().enumerate() {
                    let Property::Scalar(_, scalar) = property else {
                        return Err(LoadError::InvalidData(
                            "list properties in PLY vertices are unsupported".into(),
                        ));
                    };
                    let value = read_scalar(&mut reader, *scalar)?;
                    if let Some(axis) = axes.iter().position(|index| *index == property_index) {
                        xyz[axis] = value;
                    }
                }
                add_vertex(&mut mesh, xyz)?;
            }
            for _ in 0..face_count {
                let mut indices = None;
                for property in &header.face_properties {
                    match property {
                        Property::Scalar(_, scalar) => {
                            read_scalar(&mut reader, *scalar)?;
                        }
                        Property::List(name, count_type, item_type) => {
                            let count = integer(read_scalar(&mut reader, *count_type)?)?;
                            if count > MAX_FACE_ITEMS {
                                return Err(LoadError::InvalidData(
                                    "PLY face list is too long".into(),
                                ));
                            }
                            if is_vertex_indices(name) {
                                let mut face = Vec::with_capacity(count);
                                for _ in 0..count {
                                    face.push(
                                        u32::try_from(integer(read_scalar(
                                            &mut reader,
                                            *item_type,
                                        )?)?)
                                        .map_err(|_| bad_face())?,
                                    );
                                }
                                indices = Some(face);
                            } else {
                                for _ in 0..count {
                                    read_scalar(&mut reader, *item_type)?;
                                }
                            }
                        }
                    }
                }
                add_face(&mut mesh, indices.ok_or_else(bad_face)?)?;
            }
        }
    }
    if SourceStamp::read(path)? != before {
        return Err(LoadError::InvalidData(
            "PLY source changed while loading faces".into(),
        ));
    }
    Ok(Some(mesh))
}

fn read_header(reader: &mut impl BufRead) -> Result<Header, LoadError> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if line.trim() != "ply" {
        return Err(LoadError::InvalidData("missing PLY signature".into()));
    }
    let mut header = Header::default();
    let mut element = String::new();
    let mut intervening = false;
    for _ in 0..1024 {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(LoadError::InvalidData("PLY header is incomplete".into()));
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        match fields.as_slice() {
            ["format", "ascii", _] => header.encoding = Some(Encoding::Ascii),
            ["format", "binary_little_endian", _] => {
                header.encoding = Some(Encoding::BinaryLittleEndian);
            }
            ["format", _, _] => {
                return Err(LoadError::InvalidData(
                    "unsupported PLY mesh encoding".into(),
                ));
            }
            ["element", name, count] => {
                let count = count
                    .parse::<usize>()
                    .map_err(|_| LoadError::InvalidData("invalid PLY element count".into()))?;
                if *name == "vertex" {
                    if header.vertices.is_some() || intervening {
                        return Err(LoadError::InvalidData(
                            "PLY vertices must be the first element".into(),
                        ));
                    }
                    header.vertices = Some(count);
                } else if *name == "face" {
                    if header.vertices.is_none() || header.faces.is_some() || intervening {
                        return Err(LoadError::InvalidData(
                            "PLY faces must follow vertices".into(),
                        ));
                    }
                    header.faces = Some(count);
                } else if header.faces.is_none() && count > 0 {
                    intervening = true;
                }
                element = name.to_string();
            }
            ["property", "list", count, item, name] if element == "face" => {
                header.face_properties.push(Property::List(
                    name.to_ascii_lowercase(),
                    Scalar::parse(count)?,
                    Scalar::parse(item)?,
                ));
            }
            ["property", "list", ..] if element == "vertex" => {
                return Err(LoadError::InvalidData(
                    "list properties in PLY vertices are unsupported".into(),
                ));
            }
            ["property", data_type, name] if element == "vertex" => {
                header.vertex_properties.push(Property::Scalar(
                    name.to_ascii_lowercase(),
                    Scalar::parse(data_type)?,
                ));
            }
            ["property", data_type, name] if element == "face" => {
                header.face_properties.push(Property::Scalar(
                    name.to_ascii_lowercase(),
                    Scalar::parse(data_type)?,
                ));
            }
            ["end_header"] => return Ok(header),
            _ => {}
        }
    }
    Err(LoadError::InvalidData("PLY header is too long".into()))
}

fn read_data_line(reader: &mut impl BufRead, line: &mut String) -> Result<(), LoadError> {
    line.clear();
    if reader.read_line(line)? == 0 {
        return Err(LoadError::InvalidData("truncated PLY mesh".into()));
    }
    Ok(())
}

fn read_scalar(reader: &mut impl Read, scalar: Scalar) -> Result<f64, LoadError> {
    let mut bytes = [0u8; 8];
    reader.read_exact(&mut bytes[..scalar.size()])?;
    Ok(scalar.read(&bytes))
}

fn integer(value: f64) -> Result<usize, LoadError> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > usize::MAX as f64 {
        return Err(bad_face());
    }
    Ok(value as usize)
}

fn add_vertex(mesh: &mut MeshGeometry, xyz: [f64; 3]) -> Result<(), LoadError> {
    if !xyz.iter().all(|coordinate| coordinate.is_finite()) {
        return Err(LoadError::InvalidData("non-finite PLY vertex".into()));
    }
    mesh.vertices.push(xyz);
    Ok(())
}

fn add_face(mesh: &mut MeshGeometry, indices: Vec<u32>) -> Result<(), LoadError> {
    if indices.len() < 3
        || indices
            .iter()
            .any(|index| *index as usize >= mesh.vertices.len())
    {
        return Err(bad_face());
    }
    let triangles = indices.len() - 2;
    if mesh.triangles.len() + triangles > MAX_TRIANGLES {
        return Err(LoadError::InvalidData("PLY triangle limit exceeded".into()));
    }
    for next in 1..indices.len() - 1 {
        mesh.triangles
            .push([indices[0], indices[next], indices[next + 1]]);
    }
    Ok(())
}

fn is_vertex_indices(name: &str) -> bool {
    name == "vertex_indices" || name == "vertex_index"
}

fn bad_face() -> LoadError {
    LoadError::InvalidData("invalid PLY mesh face".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangulates_ascii_quad_with_extra_face_property() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "ply\nformat ascii 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nelement face 1\nproperty uchar material\nproperty list uchar int vertex_indices\nend_header\n0 0 0 255\n1 0 0 255\n1 1 0 255\n0 1 0 255\n7 4 0 1 2 3\n",
        )
        .unwrap();
        let mesh = read_ply_mesh(file.path()).unwrap().unwrap();
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn reads_binary_triangle_and_rejects_bad_index() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let header = "ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n";
        let mut bytes = header.as_bytes().to_vec();
        for xyz in [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            for value in xyz {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes.push(3);
        for index in [0_i32, 1, 2] {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        std::fs::write(file.path(), &bytes).unwrap();
        let mesh = read_ply_mesh(file.path()).unwrap().unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        bytes.truncate(bytes.len() - 4);
        bytes.extend_from_slice(&3_i32.to_le_bytes());
        std::fs::write(file.path(), bytes).unwrap();
        assert!(read_ply_mesh(file.path()).is_err());
    }

    #[test]
    fn point_only_ply_has_no_mesh() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "ply\nformat ascii 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nend_header\n1 2 3\n",
        )
        .unwrap();
        assert!(read_ply_mesh(file.path()).unwrap().is_none());
    }
}
