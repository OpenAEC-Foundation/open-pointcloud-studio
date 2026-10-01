//! Bounded native OBJ face loader for showing reconstructed meshes.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::{LoadError, SourceStamp};

pub(crate) const MAX_VERTICES: usize = 1_000_000;
pub(crate) const MAX_TRIANGLES: usize = 2_000_000;

#[derive(Debug)]
pub struct MeshGeometry {
    pub vertices: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// Read OBJ vertex positions and triangulate polygon faces for GPU display.
pub fn read_obj_mesh(path: impl AsRef<Path>) -> Result<MeshGeometry, LoadError> {
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let source = BufReader::new(File::open(path)?);
    let mut mesh = MeshGeometry {
        vertices: Vec::new(),
        triangles: Vec::new(),
    };
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
                mesh.vertices.push(xyz);
            }
            Some("f") => {
                let indices: Result<Vec<u32>, LoadError> = fields
                    .map(|field| {
                        let raw: i64 = field
                            .split('/')
                            .next()
                            .unwrap_or_default()
                            .parse()
                            .map_err(|_| LoadError::InvalidData("invalid OBJ face index".into()))?;
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
}
