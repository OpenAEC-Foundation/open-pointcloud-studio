//! Bounded native OBJ face loader for showing reconstructed meshes.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use super::{LoadError, SourceStamp};

pub(crate) const MAX_VERTICES: usize = 1_000_000;
pub(crate) const MAX_TRIANGLES: usize = 2_000_000;
const MAX_MTL_BYTES: u64 = 16 * 1024 * 1024;

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

fn read_material_library(
    path: &Path,
    materials: &mut HashMap<String, [u8; 3]>,
) -> Result<(), LoadError> {
    if std::fs::metadata(path)?.len() > MAX_MTL_BYTES {
        return Err(LoadError::InvalidData(
            "OBJ material library is too large".into(),
        ));
    }
    let before = SourceStamp::read(path)?;
    let source = BufReader::new(File::open(path)?);
    let mut name = None;
    let mut loaded = HashMap::new();
    for line in source.lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("newmtl") => {
                let value = fields.collect::<Vec<_>>().join(" ");
                name = (!value.is_empty()).then_some(value);
            }
            Some("Kd") => {
                let Some(name) = &name else { continue };
                let values = fields
                    .take(3)
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>();
                let Ok(values) = values else { continue };
                if values.len() != 3
                    || values
                        .iter()
                        .any(|value| !value.is_finite() || !(0.0..=255.0).contains(value))
                {
                    continue;
                }
                let scale = if values.iter().all(|value| *value <= 1.0) {
                    255.0
                } else {
                    1.0
                };
                loaded.insert(
                    name.clone(),
                    std::array::from_fn(|axis| (values[axis] * scale).round() as u8),
                );
            }
            _ => {}
        }
    }
    if SourceStamp::read(path)? != before {
        return Err(LoadError::InvalidData(
            "OBJ material library changed while loading".into(),
        ));
    }
    materials.extend(loaded);
    Ok(())
}

fn material_library_paths(parent: &Path, names: Vec<&str>) -> Vec<PathBuf> {
    if names.is_empty() {
        return Vec::new();
    }
    let joined_name = names.join(" ");
    let joined = parent.join(&joined_name);
    if !Path::new(&joined_name).is_absolute() && joined.is_file() {
        return vec![joined];
    }
    names
        .into_iter()
        .map(Path::new)
        .filter(|path| !path.is_absolute())
        .map(|path| parent.join(path))
        .collect()
}

fn apply_material_colors(
    mesh: &mut MeshGeometry,
    mut colors: Vec<[u8; 3]>,
    explicit_colors: &[bool],
    face_colors: &[Option<[u8; 3]>],
) -> Result<(), LoadError> {
    let mut assigned = vec![None; mesh.vertices.len()];
    let mut duplicates = HashMap::<(u32, [u8; 3]), u32>::new();
    for (face, material) in mesh.triangles.iter_mut().zip(face_colors) {
        for index in face {
            let original = *index as usize;
            if explicit_colors[original] {
                continue;
            }
            let desired = material.unwrap_or([255; 3]);
            if assigned[original].is_none() {
                assigned[original] = Some(desired);
                colors[original] = desired;
                continue;
            }
            if assigned[original] == Some(desired) {
                continue;
            }
            let key = (*index, desired);
            if let Some(&duplicate) = duplicates.get(&key) {
                *index = duplicate;
                continue;
            }
            if mesh.vertices.len() >= MAX_VERTICES {
                return Err(LoadError::InvalidData(
                    "OBJ material vertex limit exceeded".into(),
                ));
            }
            let duplicate = mesh.vertices.len() as u32;
            mesh.vertices.push(mesh.vertices[original]);
            colors.push(desired);
            if let Some(normals) = &mut mesh.normals {
                normals.push(normals[original]);
            }
            duplicates.insert(key, duplicate);
            *index = duplicate;
        }
    }
    mesh.colors = Some(colors);
    Ok(())
}

/// Read OBJ vertex positions and triangulate polygon faces for GPU display.
pub fn read_obj_mesh(path: impl AsRef<Path>) -> Result<MeshGeometry, LoadError> {
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let source = BufReader::new(File::open(path)?);
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut mesh = MeshGeometry::default();
    let mut colors = Vec::<[u8; 3]>::new();
    let mut explicit_colors = Vec::<bool>::new();
    let mut has_color = false;
    let mut normals = Vec::<[f32; 3]>::new();
    let mut normals_aligned = true;
    let mut materials = HashMap::<String, [u8; 3]>::new();
    let mut active_material = None;
    let mut face_colors = Vec::<Option<[u8; 3]>>::new();
    for line in source.lines() {
        let line = line?;
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("mtllib") => {
                for library in material_library_paths(parent, fields.collect()) {
                    let _ = read_material_library(&library, &mut materials);
                }
            }
            Some("usemtl") => {
                active_material = materials
                    .get(&fields.collect::<Vec<_>>().join(" "))
                    .copied();
            }
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
                let explicit_color = matches!(extras.len(), 3 | 4);
                let rgb = if explicit_color {
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
                explicit_colors.push(explicit_color);
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
                    face_colors.push(active_material);
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
    if normals_aligned && normals.len() == mesh.vertices.len() {
        mesh.normals = Some(normals);
    }
    if face_colors.iter().any(Option::is_some) {
        apply_material_colors(&mut mesh, colors, &explicit_colors, &face_colors)?;
    } else if has_color {
        mesh.colors = Some(colors);
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

    #[test]
    fn material_diffuse_colors_split_shared_vertices_and_survive_export() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("materials.obj");
        let library = directory.path().join("materials.mtl");
        let exported = directory.path().join("exported.obj");
        std::fs::write(&library, "newmtl red\nKd 1 0 0\nnewmtl blue\nKd 0 0 1\n").unwrap();
        std::fs::write(
            &source,
            "mtllib materials.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nv 0 0 1\nusemtl red\nf 1 2 3\nusemtl blue\nf 1 3 4\n",
        )
        .unwrap();
        let mesh = read_obj_mesh(&source).unwrap();
        assert_eq!(mesh.triangles.len(), 2);
        assert_eq!(mesh.vertices.len(), 6);
        let colors = mesh.colors.as_ref().unwrap();
        for index in mesh.triangles[0] {
            assert_eq!(colors[index as usize], [255, 0, 0]);
        }
        for index in mesh.triangles[1] {
            assert_eq!(colors[index as usize], [0, 0, 255]);
        }
        write_obj_mesh(&mesh, &exported, &[]).unwrap();
        let reopened = read_obj_mesh(exported).unwrap();
        assert_eq!(reopened.colors, mesh.colors);
        assert_eq!(reopened.triangles, mesh.triangles);
    }
}
