//! Bounded native OBJ face loader for showing reconstructed meshes.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use super::{LoadError, SourceStamp};

pub(crate) const MAX_VERTICES: usize = 1_000_000;
pub(crate) const MAX_TRIANGLES: usize = 2_000_000;

#[derive(Debug, Default)]
pub struct MeshGeometry {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub colors: Option<Vec<[u8; 3]>>,
    pub normals: Option<Vec<[f32; 3]>>,
}

/// Atomically write the triangles currently held by the native viewer.
pub fn write_obj_mesh(
    mesh: &MeshGeometry,
    destination: impl AsRef<Path>,
    comments: &[&str],
) -> Result<(), LoadError> {
    if mesh.vertices.is_empty()
        || mesh.triangles.is_empty()
        || mesh.vertices.len() > MAX_VERTICES
        || mesh.triangles.len() > MAX_TRIANGLES
        || mesh
            .vertices
            .iter()
            .any(|vertex| !vertex.iter().all(|value| value.is_finite()))
        || mesh.triangles.iter().any(|face| {
            face.iter()
                .any(|index| *index as usize >= mesh.vertices.len())
        })
        || mesh
            .colors
            .as_ref()
            .is_some_and(|colors| colors.len() != mesh.vertices.len())
        || mesh.normals.as_ref().is_some_and(|normals| {
            normals.len() != mesh.vertices.len()
                || normals
                    .iter()
                    .any(|normal| !normal.iter().all(|value| value.is_finite()))
        })
        || comments
            .iter()
            .any(|comment| comment.contains(['\n', '\r']))
    {
        return Err(LoadError::InvalidData("invalid OBJ mesh export".into()));
    }
    let destination = destination.as_ref();
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writeln!(writer, "# Mesh exported by Open Pointcloud Studio")?;
        for comment in comments {
            writeln!(writer, "# {comment}")?;
        }
        for (index, [x, y, z]) in mesh.vertices.iter().enumerate() {
            if let Some(rgb) = mesh.colors.as_ref().map(|colors| colors[index]) {
                writeln!(
                    writer,
                    "v {x} {y} {z} {:.6} {:.6} {:.6}",
                    f64::from(rgb[0]) / 255.0,
                    f64::from(rgb[1]) / 255.0,
                    f64::from(rgb[2]) / 255.0,
                )?;
            } else {
                writeln!(writer, "v {x} {y} {z}")?;
            }
        }
        if let Some(normals) = &mesh.normals {
            for [x, y, z] in normals {
                writeln!(writer, "vn {x} {y} {z}")?;
            }
        }
        for [a, b, c] in &mesh.triangles {
            if mesh.normals.is_some() {
                writeln!(
                    writer,
                    "f {}//{} {}//{} {}//{}",
                    a + 1,
                    a + 1,
                    b + 1,
                    b + 1,
                    c + 1,
                    c + 1
                )?;
            } else {
                writeln!(writer, "f {} {} {}", a + 1, b + 1, c + 1)?;
            }
        }
        writer.flush()?;
    }
    temporary.as_file_mut().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(())
}

/// Read OBJ vertex positions and triangulate polygon faces for GPU display.
pub fn read_obj_mesh(path: impl AsRef<Path>) -> Result<MeshGeometry, LoadError> {
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let source = BufReader::new(File::open(path)?);
    let mut mesh = MeshGeometry::default();
    let mut colors = Vec::<[u8; 3]>::new();
    let mut has_color = false;
    let mut normals = Vec::<[f32; 3]>::new();
    let mut normals_aligned = true;
    for line in source.lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("v") => {
                let mut xyz = [0.0_f64; 3];
                for value in &mut xyz {
                    *value = fields
                        .next()
                        .ok_or_else(|| LoadError::InvalidData("incomplete OBJ vertex".into()))?
                        .parse()
                        .map_err(|_| LoadError::InvalidData("invalid OBJ vertex".into()))?;
                }
                if !xyz.iter().all(|value| value.is_finite()) {
                    return Err(LoadError::InvalidData("non-finite OBJ vertex".into()));
                }
                if mesh.vertices.len() >= MAX_VERTICES {
                    return Err(LoadError::InvalidData("OBJ vertex limit exceeded".into()));
                }
                let extras: Vec<_> = fields.collect();
                let rgb = if matches!(extras.len(), 3 | 4) {
                    let values = extras[extras.len() - 3..]
                        .iter()
                        .map(|field| field.parse::<f64>())
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|_| LoadError::InvalidData("invalid OBJ vertex color".into()))?;
                    if values
                        .iter()
                        .any(|value| !value.is_finite() || !(0.0..=255.0).contains(value))
                    {
                        return Err(LoadError::InvalidData("invalid OBJ vertex color".into()));
                    }
                    has_color = true;
                    let unit_scale = values.iter().all(|value| *value <= 1.0);
                    values
                        .into_iter()
                        .map(|value| (if unit_scale { value * 255.0 } else { value }).round() as u8)
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap()
                } else {
                    [255; 3]
                };
                colors.push(rgb);
                mesh.vertices.push(xyz);
            }
            Some("vn") => {
                let values = fields
                    .take(3)
                    .map(str::parse::<f32>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| LoadError::InvalidData("invalid OBJ normal".into()))?;
                if values.len() != 3 || values.iter().any(|value| !value.is_finite()) {
                    return Err(LoadError::InvalidData("invalid OBJ normal".into()));
                }
                normals.push(values.try_into().unwrap());
            }
            Some("f") => {
                let indices: Result<Vec<u32>, LoadError> = fields
                    .map(|field| {
                        let mut components = field.split('/');
                        let raw: i64 =
                            components.next().unwrap_or_default().parse().map_err(|_| {
                                LoadError::InvalidData("invalid OBJ face index".into())
                            })?;
                        let index = if raw > 0 {
                            raw - 1
                        } else if raw < 0 {
                            mesh.vertices.len() as i64 + raw
                        } else {
                            -1
                        };
                        if index < 0 || index >= mesh.vertices.len() as i64 {
                            return Err(LoadError::InvalidData(
                                "OBJ face index out of range".into(),
                            ));
                        }
                        let normal = components.nth(1).unwrap_or_default();
                        if normal.parse::<i64>().ok() != Some(index + 1) {
                            normals_aligned = false;
                        }
                        Ok(index as u32)
                    })
                    .collect();
                let indices = indices?;
                if indices.len() < 3 {
                    return Err(LoadError::InvalidData(
                        "OBJ face has fewer than 3 vertices".into(),
                    ));
                }
                for next in 1..indices.len() - 1 {
                    if mesh.triangles.len() >= MAX_TRIANGLES {
                        return Err(LoadError::InvalidData("OBJ triangle limit exceeded".into()));
                    }
                    mesh.triangles
                        .push([indices[0], indices[next], indices[next + 1]]);
                }
            }
            _ => {}
        }
    }
    if mesh.triangles.is_empty() || SourceStamp::read(path)? != before {
        return Err(LoadError::InvalidData(
            "OBJ has no faces or changed while loading".into(),
        ));
    }
    if has_color {
        mesh.colors = Some(colors);
    }
    if normals_aligned && normals.len() == mesh.vertices.len() {
        mesh.normals = Some(normals);
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangulates_quad_and_negative_indices() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        use std::io::Write;
        writeln!(file, "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf -4 -3 -2 -1").unwrap();
        let mesh = read_obj_mesh(file.path()).unwrap();
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn writes_resident_mesh_with_precise_coordinates_and_rejects_invalid_faces() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("surface.obj");
        let mesh = MeshGeometry {
            vertices: vec![
                [301_336.231_998_1, 5_042_597.236_764_2, 15.466_498],
                [301_337.0, 5_042_597.0, 15.0],
                [301_336.0, 5_042_598.0, 16.0],
            ],
            triangles: vec![[0, 1, 2]],
            ..MeshGeometry::default()
        };
        write_obj_mesh(&mesh, &destination, &["Source attribution"]).unwrap();
        let saved = std::fs::read_to_string(&destination).unwrap();
        assert!(saved.contains("# Source attribution"));
        let reopened = read_obj_mesh(&destination).unwrap();
        assert_eq!(reopened.vertices, mesh.vertices);
        assert_eq!(reopened.triangles, mesh.triangles);

        let invalid = MeshGeometry {
            vertices: mesh.vertices,
            triangles: vec![[0, 1, 3]],
            ..MeshGeometry::default()
        };
        assert!(write_obj_mesh(&invalid, &destination, &[]).is_err());
        assert_eq!(std::fs::read_to_string(destination).unwrap(), saved);
    }

    #[test]
    fn preserves_obj_vertex_colors_and_aligned_normals() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("colored.obj");
        let mesh = MeshGeometry {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            colors: Some(vec![[255, 0, 0], [0, 128, 0], [0, 0, 255]]),
            normals: Some(vec![[0.0, 0.0, 1.0]; 3]),
        };
        write_obj_mesh(&mesh, &destination, &[]).unwrap();
        let saved = std::fs::read_to_string(&destination).unwrap();
        assert!(saved.contains("f 1//1 2//2 3//3"));
        let reopened = read_obj_mesh(&destination).unwrap();
        assert_eq!(reopened.colors, mesh.colors);
        assert_eq!(reopened.normals, mesh.normals);

        let raw_color = directory.path().join("raw-color.obj");
        std::fs::write(
            &raw_color,
            "v 0 0 0 255 0 128\nv 1 0 0 0 255 0\nv 0 1 0 0 0 255\nf 1 2 3\n",
        )
        .unwrap();
        let raw = read_obj_mesh(raw_color).unwrap();
        assert_eq!(raw.colors.unwrap()[0], [255, 0, 128]);

        let invalid = MeshGeometry {
            colors: Some(vec![[255, 0, 0]]),
            ..mesh
        };
        assert!(write_obj_mesh(&invalid, &destination, &[]).is_err());
        assert_eq!(std::fs::read_to_string(destination).unwrap(), saved);
    }
}
