//! Resident face loaders for OFF and STL meshes.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use super::mesh_write::stl_header_origin;
use super::obj_mesh::{MeshGeometry, MAX_TRIANGLES, MAX_VERTICES};
use super::{LoadError, SourceStamp};

pub fn read_off_mesh(path: impl AsRef<Path>) -> Result<Option<MeshGeometry>, LoadError> {
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let mut lines = BufReader::new(File::open(path)?).lines();
    let header = next_off_line(&mut lines)?
        .ok_or_else(|| LoadError::InvalidData("empty OFF mesh".into()))?;
    let counts = if let Some(rest) = header.strip_prefix("OFF") {
        if rest.trim().is_empty() {
            next_off_line(&mut lines)?
                .ok_or_else(|| LoadError::InvalidData("missing OFF counts".into()))?
        } else {
            rest.trim().to_owned()
        }
    } else {
        return Err(LoadError::InvalidData("missing OFF header".into()));
    };
    let counts = counts
        .split_whitespace()
        .take(2)
        .map(|field| {
            field
                .parse::<usize>()
                .map_err(|_| LoadError::InvalidData("invalid OFF counts".into()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if counts.len() != 2 {
        return Err(LoadError::InvalidData("missing OFF counts".into()));
    }
    let (vertex_count, face_count) = (counts[0], counts[1]);
    if face_count == 0 {
        return Ok(None);
    }
    if vertex_count == 0 || vertex_count > MAX_VERTICES || face_count > MAX_TRIANGLES {
        return Err(LoadError::InvalidData("OFF mesh limit exceeded".into()));
    }
    let mut mesh = MeshGeometry {
        vertices: Vec::with_capacity(vertex_count),
        triangles: Vec::with_capacity(face_count),
        ..MeshGeometry::default()
    };
    for _ in 0..vertex_count {
        let line = next_off_line(&mut lines)?
            .ok_or_else(|| LoadError::InvalidData("truncated OFF vertices".into()))?;
        let fields = line.split_whitespace().take(3).collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err(LoadError::InvalidData("invalid OFF vertex".into()));
        }
        let mut xyz = [0.0; 3];
        for (axis, field) in fields.into_iter().enumerate() {
            xyz[axis] = field
                .parse::<f64>()
                .map_err(|_| LoadError::InvalidData("invalid OFF vertex".into()))?;
        }
        if !xyz.iter().all(|value| value.is_finite()) {
            return Err(LoadError::InvalidData("non-finite OFF vertex".into()));
        }
        mesh.vertices.push(xyz);
    }
    for _ in 0..face_count {
        let line = next_off_line(&mut lines)?
            .ok_or_else(|| LoadError::InvalidData("truncated OFF faces".into()))?;
        let mut fields = line.split_whitespace();
        let count = fields
            .next()
            .ok_or_else(bad_off_face)?
            .parse::<usize>()
            .map_err(|_| bad_off_face())?;
        if !(3..=MAX_VERTICES).contains(&count) {
            return Err(bad_off_face());
        }
        let indices = fields
            .take(count)
            .map(|field| {
                let index = field.parse::<usize>().map_err(|_| bad_off_face())?;
                if index >= vertex_count {
                    return Err(bad_off_face());
                }
                Ok(index as u32)
            })
            .collect::<Result<Vec<_>, LoadError>>()?;
        if indices.len() != count || mesh.triangles.len() + count - 2 > MAX_TRIANGLES {
            return Err(bad_off_face());
        }
        for next in 1..count - 1 {
            mesh.triangles
                .push([indices[0], indices[next], indices[next + 1]]);
        }
    }
    if SourceStamp::read(path)? != before {
        return Err(LoadError::InvalidData("OFF changed while loading".into()));
    }
    Ok(Some(mesh))
}

pub fn read_stl_mesh(path: impl AsRef<Path>) -> Result<Option<MeshGeometry>, LoadError> {
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let mut header = [0u8; 84];
    let binary_count = if length >= 84 {
        file.read_exact(&mut header)?;
        let count = u32::from_le_bytes(header[80..84].try_into().unwrap()) as u64;
        (count
            .checked_mul(50)
            .and_then(|bytes| bytes.checked_add(84))
            == Some(length))
        .then_some(count as usize)
    } else {
        None
    };
    let mut mesh = MeshGeometry {
        vertices: Vec::new(),
        triangles: Vec::new(),
        ..MeshGeometry::default()
    };
    let mut indices = HashMap::new();
    if let Some(count) = binary_count {
        if count > MAX_TRIANGLES {
            return Err(LoadError::InvalidData("STL triangle limit exceeded".into()));
        }
        mesh.triangles.reserve(count);
        // A mesh this application wrote at survey coordinates is relative to
        // the origin in its header.
        let origin = stl_header_origin(&header[..80]);
        let mut record = [0u8; 50];
        for _ in 0..count {
            file.read_exact(&mut record)?;
            let mut triangle = [0u32; 3];
            for (corner, offset) in [12, 24, 36].into_iter().enumerate() {
                let mut xyz = std::array::from_fn(|axis| {
                    let start = offset + axis * 4;
                    f32::from_le_bytes(record[start..start + 4].try_into().unwrap()) as f64
                });
                if let Some(origin) = origin {
                    for axis in 0..3 {
                        xyz[axis] += origin[axis];
                    }
                }
                triangle[corner] = intern_vertex(&mut mesh, &mut indices, xyz)?;
            }
            mesh.triangles.push(triangle);
        }
    } else {
        let source = BufReader::new(File::open(path)?);
        let mut triangle = [0u32; 3];
        let mut corners = 0usize;
        for line in source.lines() {
            let line = line?;
            let mut fields = line.split_whitespace();
            if fields.next() != Some("vertex") {
                continue;
            }
            let mut xyz = [0.0; 3];
            for value in &mut xyz {
                *value = fields
                    .next()
                    .ok_or_else(bad_stl_face)?
                    .parse::<f64>()
                    .map_err(|_| bad_stl_face())?;
            }
            triangle[corners] = intern_vertex(&mut mesh, &mut indices, xyz)?;
            corners += 1;
            if corners == 3 {
                if mesh.triangles.len() >= MAX_TRIANGLES {
                    return Err(LoadError::InvalidData("STL triangle limit exceeded".into()));
                }
                mesh.triangles.push(triangle);
                corners = 0;
            }
        }
        if corners != 0 {
            return Err(bad_stl_face());
        }
    }
    if SourceStamp::read(path)? != before {
        return Err(LoadError::InvalidData("STL changed while loading".into()));
    }
    Ok((!mesh.triangles.is_empty()).then_some(mesh))
}

fn intern_vertex(
    mesh: &mut MeshGeometry,
    indices: &mut HashMap<[u64; 3], u32>,
    xyz: [f64; 3],
) -> Result<u32, LoadError> {
    if !xyz.iter().all(|value| value.is_finite()) {
        return Err(LoadError::InvalidData("non-finite STL vertex".into()));
    }
    let key = xyz.map(f64::to_bits);
    if let Some(index) = indices.get(&key) {
        return Ok(*index);
    }
    if mesh.vertices.len() >= MAX_VERTICES {
        return Err(LoadError::InvalidData("STL vertex limit exceeded".into()));
    }
    let index = mesh.vertices.len() as u32;
    mesh.vertices.push(xyz);
    indices.insert(key, index);
    Ok(index)
}

fn next_off_line(
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

fn bad_off_face() -> LoadError {
    LoadError::InvalidData("invalid OFF mesh face".into())
}

fn bad_stl_face() -> LoadError {
    LoadError::InvalidData("invalid STL mesh face".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_quad_triangulates_and_rejects_out_of_range_index() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "OFF\n# comment\n4 1 0\n0 0 0\n1 0 0\n1 1 0\n0 1 0\n4 0 1 2 3\n",
        )
        .unwrap();
        let mesh = read_off_mesh(file.path()).unwrap().unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
        std::fs::write(file.path(), "OFF\n3 1 0\n0 0 0\n1 0 0\n0 1 0\n3 0 1 3\n").unwrap();
        assert!(read_off_mesh(file.path()).is_err());
    }

    #[test]
    fn binary_stl_deduplicates_shared_vertices() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut bytes = vec![0u8; 80];
        bytes.extend_from_slice(&2u32.to_le_bytes());
        for triangle in [
            [[0.0_f32, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]],
            [[0.0_f32, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
        ] {
            bytes.extend_from_slice(&[0u8; 12]);
            for xyz in triangle {
                for value in xyz {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0u8; 2]);
        }
        std::fs::write(file.path(), bytes).unwrap();
        let mesh = read_stl_mesh(file.path()).unwrap().unwrap();
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn ascii_stl_triangulates_and_rejects_partial_face() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "solid s\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid s\n",
        )
        .unwrap();
        let mesh = read_stl_mesh(file.path()).unwrap().unwrap();
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        std::fs::write(file.path(), "solid s\nvertex 0 0 0\nvertex 1 0 0\n").unwrap();
        assert!(read_stl_mesh(file.path()).is_err());
    }
}
