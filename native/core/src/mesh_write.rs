//! Writers for the mesh the viewer holds: OBJ, binary PLY and binary STL.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

use super::obj_mesh::{write_obj_mesh, MeshGeometry, MAX_TRIANGLES, MAX_VERTICES};
use super::LoadError;

/// The PLY mesh loader stops reading a header after 1,024 lines; staying far
/// below that keeps every written file readable.
const MAX_PLY_COMMENTS: usize = 256;
/// STL stores 32-bit floats. Up to this distance from zero they still resolve
/// a quarter of a millimetre; beyond it an axis is written relative to an
/// origin.
const STL_LOCAL_LIMIT: f64 = 2_048.0;
/// Size of the origin's grid in metres.
const STL_ORIGIN_STEP: f64 = 1.0;
const STL_HEADER_BYTES: usize = 80;
/// Must not begin with "solid", which marks the text variant of the format.
const STL_HEADER_TEXT: &str = "Open Pointcloud Studio mesh";
const STL_ORIGIN_WORD: &str = "origin";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshFormat {
    Obj,
    Ply,
    Stl,
    /// `MESH` entities on the layer `OPS-MESH`, in metres.
    Dxf,
    /// As `Dxf`, in the binary drawing format.
    Dwg,
    /// IFC4: a building element proxy with a triangulated face set.
    Ifc,
}

impl MeshFormat {
    /// The format a destination asks for through its extension.
    pub fn from_path(path: impl AsRef<Path>) -> Option<Self> {
        match path
            .as_ref()
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("obj") => Some(Self::Obj),
            Some("ply") => Some(Self::Ply),
            Some("stl") => Some(Self::Stl),
            Some("dxf") => Some(Self::Dxf),
            Some("dwg") => Some(Self::Dwg),
            Some("ifc") => Some(Self::Ifc),
            _ => None,
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Obj => "obj",
            Self::Ply => "ply",
            Self::Stl => "stl",
            Self::Dxf => "dxf",
            Self::Dwg => "dwg",
            Self::Ifc => "ifc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Obj => "OBJ",
            Self::Ply => "PLY",
            Self::Stl => "STL",
            Self::Dxf => "DXF",
            Self::Dwg => "DWG",
            Self::Ifc => "IFC",
        }
    }
}

/// What a caller has to tell the user about a written mesh file.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MeshWriteReport {
    /// Set when the coordinates in the file are relative to this point. Only
    /// STL needs it, and only for coordinates too large for its floats.
    pub origin: Option<[f64; 3]>,
}

/// Atomically write a mesh in the given format; an existing destination is
/// only replaced by a complete file.
///
/// OBJ and PLY keep double coordinates, colours and normals. STL holds
/// triangles only, as 32-bit floats: an axis whose coordinates exceed 2,048 m
/// is written relative to a whole-metre origin, which the file header names
/// and the report returns. The STL readers of this crate add that origin
/// again; another program shows such a file near zero. `comments` go into
/// the OBJ and PLY headers; those of a PLY file must be ASCII.
pub fn write_mesh(
    mesh: &MeshGeometry,
    destination: impl AsRef<Path>,
    format: MeshFormat,
    comments: &[&str],
) -> Result<MeshWriteReport, LoadError> {
    let destination = destination.as_ref();
    match format {
        MeshFormat::Obj => {
            write_obj_mesh(mesh, destination, comments)?;
            Ok(MeshWriteReport::default())
        }
        MeshFormat::Ply => {
            validate(mesh, comments, format)?;
            // The header of a PLY file is ASCII text by definition, and a
            // reader that holds to it stops at any other character.
            if comments.len() > MAX_PLY_COMMENTS
                || comments.iter().any(|comment| !comment.is_ascii())
            {
                return Err(LoadError::InvalidData("invalid PLY mesh export".into()));
            }
            write_atomically(destination, |writer| write_ply(mesh, comments, writer))?;
            Ok(MeshWriteReport::default())
        }
        MeshFormat::Stl => {
            validate(mesh, comments, format)?;
            let origin = stl_origin(mesh);
            let header = stl_header(origin)?;
            write_atomically(destination, |writer| {
                write_stl(mesh, origin.unwrap_or_default(), &header, writer)
            })?;
            Ok(MeshWriteReport { origin })
        }
        MeshFormat::Dxf | MeshFormat::Dwg | MeshFormat::Ifc => {
            validate(mesh, comments, format)?;
            let source = comments
                .iter()
                .find_map(|comment| comment.strip_prefix("Source: "))
                .unwrap_or_default();
            let notes: Vec<&str> = comments
                .iter()
                .copied()
                .filter(|comment| !comment.starts_with("Source: "))
                .collect();
            let model = crate::cad3d::mesh_model(mesh, source, &notes);
            match format {
                MeshFormat::Dxf => crate::cad3d::cad::write_model_cad(
                    &model,
                    destination,
                    crate::DrawingFormat::Dxf,
                )?,
                MeshFormat::Dwg => crate::cad3d::cad::write_model_cad(
                    &model,
                    destination,
                    crate::DrawingFormat::Dwg,
                )?,
                _ => crate::cad3d::ifc::write_model_ifc(&model, destination)?,
            };
            Ok(MeshWriteReport::default())
        }
    }
}

/// The origin a binary STL written by `write_mesh` is relative to, read from
/// its header. `None` for a file without one: its coordinates are final.
/// The STL readers of this crate already return coordinates with the origin
/// added; this is for telling the user about it.
pub fn read_stl_origin(path: impl AsRef<Path>) -> Result<Option<[f64; 3]>, LoadError> {
    let mut header = Vec::with_capacity(STL_HEADER_BYTES);
    File::open(path)?
        .take(STL_HEADER_BYTES as u64)
        .read_to_end(&mut header)?;
    Ok(stl_header_origin(&header))
}

/// The origin named in the header of a binary STL, for the readers: without
/// it a mesh exported at survey coordinates would come back near zero.
pub(crate) fn stl_header_origin(header: &[u8]) -> Option<[f64; 3]> {
    let header = &header[..header.len().min(STL_HEADER_BYTES)];
    let text = header.split(|byte| *byte == 0).next().unwrap_or_default();
    let text = std::str::from_utf8(text)
        .ok()?
        .strip_prefix(STL_HEADER_TEXT)?;
    let mut fields = text
        .split_whitespace()
        .skip_while(|field| *field != STL_ORIGIN_WORD)
        .skip(1);
    let mut origin = [0.0; 3];
    for value in &mut origin {
        *value = fields
            .next()
            .and_then(|field| field.parse::<f64>().ok())
            .filter(|parsed| parsed.is_finite())?;
    }
    Some(origin)
}

/// The same rules as the OBJ writer, so every format refuses the same meshes.
fn validate(mesh: &MeshGeometry, comments: &[&str], format: MeshFormat) -> Result<(), LoadError> {
    let finite = |values: &[f64; 3]| values.iter().all(|value| value.is_finite());
    let valid = !mesh.vertices.is_empty()
        && !mesh.triangles.is_empty()
        && mesh.vertices.len() <= MAX_VERTICES
        && mesh.triangles.len() <= MAX_TRIANGLES
        && mesh.vertices.iter().all(finite)
        && mesh.triangles.iter().all(|face| {
            face.iter()
                .all(|index| (*index as usize) < mesh.vertices.len())
        })
        && mesh
            .colors
            .as_ref()
            .is_none_or(|colors| colors.len() == mesh.vertices.len())
        && mesh.normals.as_ref().is_none_or(|normals| {
            normals.len() == mesh.vertices.len()
                && normals
                    .iter()
                    .all(|normal| normal.iter().all(|value| value.is_finite()))
        })
        && comments
            .iter()
            .all(|comment| !comment.contains(['\n', '\r']));
    if valid {
        Ok(())
    } else {
        Err(LoadError::InvalidData(format!(
            "invalid {} mesh export",
            format.label()
        )))
    }
}

/// Write into a temporary file beside the destination and move it into place
/// once it is complete and on disk.
fn write_atomically(
    destination: &Path,
    write: impl FnOnce(&mut BufWriter<&mut File>) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        write(&mut writer)?;
        writer.flush()?;
    }
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(())
}

fn write_ply(
    mesh: &MeshGeometry,
    comments: &[&str],
    writer: &mut impl Write,
) -> Result<(), LoadError> {
    writeln!(writer, "ply")?;
    writeln!(writer, "format binary_little_endian 1.0")?;
    writeln!(writer, "comment Mesh exported by Open Pointcloud Studio")?;
    for comment in comments {
        writeln!(writer, "comment {comment}")?;
    }
    writeln!(writer, "element vertex {}", mesh.vertices.len())?;
    // Doubles: survey coordinates need more digits than a float holds.
    for axis in ["x", "y", "z"] {
        writeln!(writer, "property double {axis}")?;
    }
    if mesh.colors.is_some() {
        for channel in ["red", "green", "blue"] {
            writeln!(writer, "property uchar {channel}")?;
        }
    }
    if mesh.normals.is_some() {
        for axis in ["nx", "ny", "nz"] {
            writeln!(writer, "property float {axis}")?;
        }
    }
    writeln!(writer, "element face {}", mesh.triangles.len())?;
    writeln!(writer, "property list uchar int vertex_indices")?;
    writeln!(writer, "end_header")?;
    for (index, xyz) in mesh.vertices.iter().enumerate() {
        for value in xyz {
            writer.write_all(&value.to_le_bytes())?;
        }
        if let Some(colors) = &mesh.colors {
            writer.write_all(&colors[index])?;
        }
        if let Some(normals) = &mesh.normals {
            for value in normals[index] {
                writer.write_all(&value.to_le_bytes())?;
            }
        }
    }
    for face in &mesh.triangles {
        writer.write_all(&[3])?;
        for index in face {
            // The vertex limit keeps every index inside a signed 32-bit value.
            writer.write_all(&(*index as i32).to_le_bytes())?;
        }
    }
    Ok(())
}

/// Per axis: zero while the coordinates fit a float well, otherwise the lower
/// bound rounded down to the origin grid. Only vertices that a triangle uses
/// count, because only those reach the file.
fn stl_origin(mesh: &MeshGeometry) -> Option<[f64; 3]> {
    let mut origin = [0.0; 3];
    for (axis, value) in origin.iter_mut().enumerate() {
        let (min, max) = mesh.triangles.iter().flatten().fold(
            (f64::INFINITY, f64::NEG_INFINITY),
            |(min, max), index| {
                let coordinate = mesh.vertices[*index as usize][axis];
                (min.min(coordinate), max.max(coordinate))
            },
        );
        if min.abs().max(max.abs()) > STL_LOCAL_LIMIT {
            *value = (min / STL_ORIGIN_STEP).floor() * STL_ORIGIN_STEP;
        }
    }
    origin.iter().any(|value| *value != 0.0).then_some(origin)
}

fn stl_header(origin: Option<[f64; 3]>) -> Result<[u8; STL_HEADER_BYTES], LoadError> {
    let text = match origin {
        Some([x, y, z]) => format!("{STL_HEADER_TEXT}, {STL_ORIGIN_WORD} {x} {y} {z} m"),
        None => STL_HEADER_TEXT.to_owned(),
    };
    if text.len() > STL_HEADER_BYTES {
        return Err(LoadError::InvalidData(
            "STL coordinates are too large for the file header".into(),
        ));
    }
    let mut header = [0_u8; STL_HEADER_BYTES];
    header[..text.len()].copy_from_slice(text.as_bytes());
    Ok(header)
}

fn write_stl(
    mesh: &MeshGeometry,
    origin: [f64; 3],
    header: &[u8; STL_HEADER_BYTES],
    writer: &mut impl Write,
) -> Result<(), LoadError> {
    writer.write_all(header)?;
    writer.write_all(&(mesh.triangles.len() as u32).to_le_bytes())?;
    let mut record = [0_u8; 50];
    for face in &mesh.triangles {
        let corners = face.map(|index| {
            let vertex = mesh.vertices[index as usize];
            std::array::from_fn::<f64, 3, _>(|axis| vertex[axis] - origin[axis])
        });
        let normal = facet_normal(corners);
        for (axis, value) in normal.into_iter().enumerate() {
            record[axis * 4..axis * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (corner, xyz) in corners.into_iter().enumerate() {
            for (axis, value) in xyz.into_iter().enumerate() {
                let value = value as f32;
                if !value.is_finite() {
                    return Err(LoadError::InvalidData(
                        "STL coordinates are too large for the format".into(),
                    ));
                }
                let start = 12 + corner * 12 + axis * 4;
                record[start..start + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        // The last two bytes are the unused attribute count.
        writer.write_all(&record)?;
    }
    Ok(())
}

/// Unit normal of the facet by the right-hand rule, zero for a triangle
/// without area, as the format prescribes.
fn facet_normal([a, b, c]: [[f64; 3]; 3]) -> [f32; 3] {
    let u: [f64; 3] = std::array::from_fn(|axis| b[axis] - a[axis]);
    let v: [f64; 3] = std::array::from_fn(|axis| c[axis] - a[axis]);
    let cross = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let length = cross.iter().map(|value| value * value).sum::<f64>().sqrt();
    if length > 0.0 && length.is_finite() {
        cross.map(|value| (value / length) as f32)
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{read_mesh_geometry, read_obj_mesh, read_ply_mesh, read_stl_mesh};

    /// The coordinates as a binary STL file holds them, three per corner.
    fn stl_floats(bytes: &[u8]) -> Vec<f32> {
        bytes[84..]
            .as_chunks::<50>()
            .0
            .iter()
            .flat_map(|record| record[12..48].as_chunks::<4>().0)
            .map(|value| f32::from_le_bytes(*value))
            .collect()
    }

    /// Coloured tetrahedron with outward faces at the given corner.
    fn tetrahedron(at: [f64; 3]) -> MeshGeometry {
        let vertices = [
            [0.0, 0.0, 0.0],
            [1.25, 0.0, 0.0],
            [0.0, 1.5, 0.0],
            [0.0, 0.0, 0.75],
        ]
        .map(|offset: [f64; 3]| std::array::from_fn(|axis| at[axis] + offset[axis]))
        .to_vec();
        let diagonal = (1.0_f32 / 3.0).sqrt();
        MeshGeometry {
            vertices,
            triangles: vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
            colors: Some(vec![[255, 0, 0], [0, 128, 0], [0, 0, 255], [17, 34, 51]]),
            normals: Some(vec![
                [-diagonal, -diagonal, -diagonal],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ]),
        }
    }

    /// Corner positions per triangle, the one thing every format keeps.
    fn corners(mesh: &MeshGeometry) -> Vec<[[f64; 3]; 3]> {
        mesh.triangles
            .iter()
            .map(|face| face.map(|index| mesh.vertices[index as usize]))
            .collect()
    }

    fn largest_difference(a: &[[[f64; 3]; 3]], b: &[[[f64; 3]; 3]]) -> f64 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .flatten()
            .flatten()
            .zip(b.iter().flatten().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max)
    }

    fn file_names(directory: &Path) -> Vec<String> {
        let mut names = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        names
    }

    #[test]
    fn format_follows_the_extension() {
        assert_eq!(MeshFormat::from_path("room.obj"), Some(MeshFormat::Obj));
        assert_eq!(
            MeshFormat::from_path("C:/out/Room.PLY"),
            Some(MeshFormat::Ply)
        );
        assert_eq!(MeshFormat::from_path("room.v2.Stl"), Some(MeshFormat::Stl));
        assert_eq!(MeshFormat::from_path("room.off"), None);
        assert_eq!(MeshFormat::from_path("room"), None);
        for format in [MeshFormat::Obj, MeshFormat::Ply, MeshFormat::Stl] {
            let name = format!("mesh.{}", format.extension());
            assert_eq!(MeshFormat::from_path(name), Some(format));
            assert_eq!(format.label(), format.extension().to_ascii_uppercase());
        }
    }

    #[test]
    fn formats_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let mesh = tetrahedron([207_000.123_456_7, 474_000.765_432_1, 10.5]);
        let expected = corners(&mesh);

        let obj = directory.path().join("mesh.obj");
        let report = write_mesh(&mesh, &obj, MeshFormat::Obj, &["Source: test"]).unwrap();
        assert_eq!(report.origin, None);
        let reopened = read_obj_mesh(&obj).unwrap();
        assert_eq!(reopened.vertices, mesh.vertices);
        assert_eq!(reopened.triangles, mesh.triangles);
        assert_eq!(reopened.colors, mesh.colors);
        assert_eq!(reopened.normals, mesh.normals);
        assert!(std::fs::read_to_string(&obj)
            .unwrap()
            .contains("# Source: test"));

        let ply = directory.path().join("mesh.ply");
        let report = write_mesh(&mesh, &ply, MeshFormat::Ply, &["Source: test"]).unwrap();
        assert_eq!(report.origin, None);
        let reopened = read_ply_mesh(&ply).unwrap().unwrap();
        // Doubles: not one bit of the survey coordinates is lost.
        assert_eq!(reopened.vertices, mesh.vertices);
        assert_eq!(reopened.triangles, mesh.triangles);
        assert_eq!(reopened.colors, mesh.colors);
        assert_eq!(reopened.normals, mesh.normals);
        let bytes = std::fs::read(&ply).unwrap();
        let header_end = bytes
            .windows(11)
            .position(|window| window == b"end_header\n")
            .unwrap()
            + 11;
        let header = std::str::from_utf8(&bytes[..header_end]).unwrap();
        assert!(header.starts_with("ply\nformat binary_little_endian 1.0\n"));
        assert!(header.contains("comment Source: test\n"));
        assert!(header.contains("property double x\n"));
        assert!(header.contains("property uchar red\n"));
        assert!(header.contains("property float nx\n"));
        // 4 vertices of 3 doubles, 3 bytes and 3 floats; 4 faces of 1 + 12 bytes.
        assert_eq!(bytes.len() - header_end, 4 * (24 + 3 + 12) + 4 * 13);

        let stl = directory.path().join("mesh.stl");
        let report = write_mesh(&mesh, &stl, MeshFormat::Stl, &["Source: test"]).unwrap();
        // X and Y are survey coordinates, Z fits a float as it is.
        assert_eq!(report.origin, Some([207_000.0, 474_000.0, 0.0]));
        assert_eq!(read_stl_origin(&stl).unwrap(), report.origin);
        let bytes = std::fs::read(&stl).unwrap();
        assert_eq!(bytes.len(), 84 + 4 * 50);
        assert!(bytes.starts_with(b"Open Pointcloud Studio mesh, origin 207000 474000 0 m\0"));
        // Without the origin a float could not hold these coordinates.
        assert!(stl_floats(&bytes).iter().all(|value| value.abs() < 16.0));
        // The reader adds the origin again: the mesh comes back where it was.
        let reopened = read_stl_mesh(&stl).unwrap().unwrap();
        assert_eq!(reopened.vertices.len(), 4);
        assert!(reopened.colors.is_none() && reopened.normals.is_none());
        let difference = largest_difference(&corners(&reopened), &expected);
        assert!(difference < 1e-4, "{difference}");

        // The format dispatch of the readers gives the same place for all
        // three files, and so does opening the STL as points.
        for path in [&obj, &ply, &stl] {
            let reopened = read_mesh_geometry(path).unwrap().unwrap();
            assert_eq!(reopened.triangles.len(), 4);
            let difference = largest_difference(&corners(&reopened), &expected);
            assert!(difference < 1e-4, "{}: {difference}", path.display());
        }
        let cloud = crate::open(&stl, 16).unwrap();
        assert_eq!(cloud.total_points, 12);
        for (point, corner) in cloud.points.iter().zip(expected.iter().flatten()) {
            for (found, wanted) in point.xyz.iter().zip(corner) {
                assert!((found - wanted).abs() < 1e-4);
            }
        }
        assert_eq!(
            file_names(directory.path()),
            ["mesh.obj", "mesh.ply", "mesh.stl"]
        );
    }

    #[test]
    fn ply_without_attributes_keeps_only_positions() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("plain.ply");
        let mesh = MeshGeometry {
            colors: None,
            normals: None,
            ..tetrahedron([-3.5, 0.25, 1.0e-3])
        };
        write_mesh(&mesh, &destination, MeshFormat::Ply, &[]).unwrap();
        let bytes = std::fs::read(&destination).unwrap();
        assert!(!bytes.windows(3).any(|window| window == b"red"));
        assert!(!bytes.windows(2).any(|window| window == b"nx"));
        let reopened = read_ply_mesh(&destination).unwrap().unwrap();
        assert_eq!(reopened.vertices, mesh.vertices);
        assert_eq!(reopened.triangles, mesh.triangles);
        assert!(reopened.colors.is_none() && reopened.normals.is_none());

        // The point reader of the same format accepts the vertices as well.
        let cloud = crate::open(&destination, 16).unwrap();
        assert_eq!(cloud.total_points, 4);
    }

    #[test]
    fn stl_near_zero_needs_no_origin_and_carries_facet_normals() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("local.stl");
        let mesh = tetrahedron([-12.5, 2_000.0, 3.0]);
        let report = write_mesh(&mesh, &destination, MeshFormat::Stl, &[]).unwrap();
        assert_eq!(report.origin, None);
        assert_eq!(read_stl_origin(&destination).unwrap(), None);
        let reopened = read_stl_mesh(&destination).unwrap().unwrap();
        let difference = largest_difference(&corners(&reopened), &corners(&mesh));
        assert!(difference < 1e-4, "{difference}");

        let bytes = std::fs::read(&destination).unwrap();
        assert!(bytes.starts_with(b"Open Pointcloud Studio mesh\0"));
        let float =
            |offset: usize| f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        // First facet lies in the plane z = 3 and faces down; the third is
        // the slanted one, facing away from the corner.
        assert_eq!([float(84), float(88), float(92)], [0.0, 0.0, -1.0]);
        let slanted = [float(184), float(188), float(192)];
        assert!(slanted.iter().all(|value| *value > 0.0));
        let length = slanted
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((length - 1.0).abs() < 1e-6);
        // Attribute bytes stay zero.
        assert_eq!(&bytes[132..134], &[0, 0]);
    }

    #[test]
    fn stl_origin_rounds_down_per_axis() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("survey.stl");
        let mesh = tetrahedron([-301_336.25, 5_042_597.75, -2_500.5]);
        let report = write_mesh(&mesh, &destination, MeshFormat::Stl, &[]).unwrap();
        let origin = report.origin.unwrap();
        assert_eq!(origin, [-301_337.0, 5_042_597.0, -2_501.0]);
        assert_eq!(read_stl_origin(&destination).unwrap(), Some(origin));
        // The coordinates in the file start at the origin and stay small.
        assert!(stl_floats(&std::fs::read(&destination).unwrap())
            .iter()
            .all(|value| (0.0..4.0).contains(value)));
        let reopened = read_stl_mesh(&destination).unwrap().unwrap();
        let difference = largest_difference(&corners(&reopened), &corners(&mesh));
        assert!(difference < 1e-6, "{difference}");

        // A vertex that no triangle uses does not reach the file, so it does
        // not pull the origin away from the triangles either.
        let mut stray = tetrahedron([-301_336.25, 5_042_597.75, -2_500.5]);
        stray.vertices.push([-900_000.0, 0.0, 0.0]);
        stray.colors = None;
        stray.normals = None;
        let report = write_mesh(&stray, &destination, MeshFormat::Stl, &[]).unwrap();
        assert_eq!(report.origin, Some(origin));

        // A file from elsewhere has no origin, whatever its header says,
        // and is read as it is.
        let foreign = directory.path().join("foreign.stl");
        std::fs::write(&foreign, [b' '; 84]).unwrap();
        assert_eq!(read_stl_origin(&foreign).unwrap(), None);
        std::fs::write(&foreign, b"Open Pointcloud Studio mesh, origin 1 two 3 m").unwrap();
        assert_eq!(read_stl_origin(&foreign).unwrap(), None);
        let mut bytes = std::fs::read(&destination).unwrap();
        bytes[..80].fill(0);
        bytes[..25].copy_from_slice(b"mesh, origin 5 5 5 m, far");
        std::fs::write(&foreign, bytes).unwrap();
        assert_eq!(read_stl_origin(&foreign).unwrap(), None);
        let unmoved = read_stl_mesh(&foreign).unwrap().unwrap();
        assert!(unmoved
            .vertices
            .iter()
            .flatten()
            .all(|value| (0.0..4.0).contains(value)));
        assert!(read_stl_origin(directory.path().join("missing.stl")).is_err());

        // Coordinates no header can describe are refused, not truncated.
        let huge = tetrahedron([1.0e30, -1.0e30, 1.0e30]);
        assert!(write_mesh(&huge, &destination, MeshFormat::Stl, &[]).is_err());
        assert_eq!(read_stl_origin(&destination).unwrap(), Some(origin));
    }

    #[test]
    fn invalid_mesh_leaves_an_existing_file_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let good = tetrahedron([0.0; 3]);
        let invalid = [
            MeshGeometry {
                triangles: vec![[0, 1, 4]],
                ..tetrahedron([0.0; 3])
            },
            MeshGeometry {
                colors: Some(vec![[255, 0, 0]]),
                ..tetrahedron([0.0; 3])
            },
            MeshGeometry {
                normals: Some(vec![[f32::NAN, 0.0, 1.0]; 4]),
                ..tetrahedron([0.0; 3])
            },
            tetrahedron([f64::NAN, 0.0, 0.0]),
            MeshGeometry {
                triangles: Vec::new(),
                ..tetrahedron([0.0; 3])
            },
            MeshGeometry::default(),
        ];
        let too_many = vec!["note"; MAX_PLY_COMMENTS + 1];
        for format in [MeshFormat::Obj, MeshFormat::Ply, MeshFormat::Stl] {
            let destination = directory
                .path()
                .join(format!("kept.{}", format.extension()));
            write_mesh(&good, &destination, format, &[]).unwrap();
            let saved = std::fs::read(&destination).unwrap();
            for mesh in &invalid {
                assert!(matches!(
                    write_mesh(mesh, &destination, format, &[]),
                    Err(LoadError::InvalidData(_))
                ));
            }
            assert!(write_mesh(&good, &destination, format, &["two\nlines"]).is_err());
            if format == MeshFormat::Ply {
                assert!(write_mesh(&good, &destination, format, &too_many).is_err());
                // A PLY header is ASCII; other characters would stop a
                // reader that holds to the format.
                assert!(write_mesh(&good, &destination, format, &["© source"]).is_err());
            }
            assert_eq!(std::fs::read(&destination).unwrap(), saved);
        }
        // No temporary file stays behind after a refusal.
        assert_eq!(
            file_names(directory.path()),
            ["kept.obj", "kept.ply", "kept.stl"]
        );

        // A destination that cannot be created reports the I/O error.
        let missing = directory.path().join("no-such-folder").join("mesh.ply");
        assert!(matches!(
            write_mesh(&good, missing, MeshFormat::Ply, &[]),
            Err(LoadError::Io(_))
        ));
    }
}
