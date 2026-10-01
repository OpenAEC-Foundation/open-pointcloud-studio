//! Streaming ASCII DXF POINT and 3DFACE extraction.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use super::obj_mesh::{MeshGeometry, MAX_TRIANGLES, MAX_VERTICES};
use super::{LoadError, Point, SourceStamp};

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntityType {
    Point,
    Face,
}

#[derive(Default)]
struct Entity {
    kind: Option<EntityType>,
    coordinates: [[Option<f64>; 3]; 4],
    aci: Option<i32>,
    true_color: Option<u32>,
}

pub fn read(
    path: &Path,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    scan(path, &mut |entity| flush(entity, push))
}

pub fn read_mesh(path: impl AsRef<Path>) -> Result<Option<MeshGeometry>, LoadError> {
    let path = path.as_ref();
    let before = SourceStamp::read(path)?;
    let mut mesh = MeshGeometry {
        vertices: Vec::new(),
        triangles: Vec::new(),
    };
    let mut indices = HashMap::<[u64; 3], u32>::new();
    scan(path, &mut |entity| {
        if entity.kind != Some(EntityType::Face) {
            return Ok(());
        }
        let mut corners = Vec::with_capacity(4);
        for values in entity.coordinates.iter().take(3) {
            let [Some(x), Some(y), z] = *values else {
                return Err(LoadError::InvalidData("incomplete DXF 3DFACE".into()));
            };
            corners.push([x, y, z.unwrap_or(0.0)]);
        }
        let [x, y, z] = entity.coordinates[3];
        if let (Some(x), Some(y)) = (x, y) {
            let fourth = [x, y, z.unwrap_or(0.0)];
            if fourth != corners[2] {
                corners.push(fourth);
            }
        }
        if mesh.triangles.len() + corners.len() - 2 > MAX_TRIANGLES {
            return Err(LoadError::InvalidData("DXF triangle limit exceeded".into()));
        }
        let mut face = Vec::with_capacity(corners.len());
        for xyz in corners {
            if !xyz.iter().all(|value| value.is_finite()) {
                return Err(LoadError::InvalidData("non-finite DXF 3DFACE".into()));
            }
            let key = xyz.map(f64::to_bits);
            let index = if let Some(index) = indices.get(&key) {
                *index
            } else {
                if mesh.vertices.len() >= MAX_VERTICES {
                    return Err(LoadError::InvalidData("DXF vertex limit exceeded".into()));
                }
                let index = mesh.vertices.len() as u32;
                mesh.vertices.push(xyz);
                indices.insert(key, index);
                index
            };
            face.push(index);
        }
        for next in 1..face.len() - 1 {
            mesh.triangles.push([face[0], face[next], face[next + 1]]);
        }
        Ok(())
    })?;
    if SourceStamp::read(path)? != before {
        return Err(LoadError::InvalidData("DXF changed while loading".into()));
    }
    Ok((!mesh.triangles.is_empty()).then_some(mesh))
}

fn scan(
    path: &Path,
    visit: &mut impl FnMut(&Entity) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let mut lines = BufReader::new(File::open(path)?).lines();
    let mut entity = Entity::default();
    let mut in_entities = false;
    let mut section_name_next = false;
    while let Some(code_line) = lines.next() {
        let code = code_line?
            .trim()
            .parse::<i32>()
            .map_err(|_| LoadError::InvalidData("invalid DXF group code".into()))?;
        let value = lines
            .next()
            .ok_or_else(|| LoadError::InvalidData("odd number of DXF lines".into()))??;
        let value = value.trim();
        if code == 0 {
            visit(&entity)?;
            entity = Entity::default();
            match value {
                "SECTION" => section_name_next = true,
                "ENDSEC" => in_entities = false,
                "POINT" if in_entities => entity.kind = Some(EntityType::Point),
                "3DFACE" if in_entities => entity.kind = Some(EntityType::Face),
                _ => {}
            }
            continue;
        }
        if section_name_next && code == 2 {
            in_entities = value == "ENTITIES";
            section_name_next = false;
            continue;
        }
        if entity.kind.is_none() {
            continue;
        }
        match code {
            62 => entity.aci = value.parse().ok(),
            420 => entity.true_color = value.parse().ok(),
            10..=13 | 20..=23 | 30..=33 => {
                let axis = ((code / 10) - 1) as usize;
                let vertex = (code % 10) as usize;
                if let Some(target) = entity.coordinates.get_mut(vertex) {
                    target[axis] =
                        Some(value.parse().map_err(|_| {
                            LoadError::InvalidData("invalid DXF coordinate".into())
                        })?);
                }
            }
            _ => {}
        }
    }
    visit(&entity)
}

fn flush(
    entity: &Entity,
    push: &mut impl FnMut(Point) -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    let color = entity
        .true_color
        .map(|bits| [(bits >> 16) as u8, (bits >> 8) as u8, bits as u8])
        .or_else(|| entity.aci.and_then(aci_rgb));
    let count = match entity.kind {
        Some(EntityType::Point) => 1,
        Some(EntityType::Face) => 4,
        None => 0,
    };
    let mut previous = None;
    for values in entity.coordinates.iter().take(count) {
        let [Some(x), Some(y), z] = *values else {
            continue;
        };
        let xyz = [x, y, z.unwrap_or(0.0)];
        if previous == Some(xyz) {
            continue;
        }
        push(Point {
            xyz,
            rgb: color,
            intensity: None,
            classification: None,
        })?;
        previous = Some(xyz);
    }
    Ok(())
}

fn aci_rgb(index: i32) -> Option<[u8; 3]> {
    Some(match index {
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        7 => [255, 255, 255],
        8 | 9 => [128, 128, 128],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_point_and_face_vertices() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.dxf");
        std::fs::write(&path, "0\nSECTION\n2\nENTITIES\n0\nPOINT\n10\n1\n20\n2\n30\n3\n62\n1\n0\n3DFACE\n10\n0\n20\n0\n11\n1\n21\n0\n12\n0\n22\n1\n0\nENDSEC\n0\nEOF\n").unwrap();
        let cloud = super::super::open(&path, 10).unwrap();
        assert_eq!(cloud.total_points, 4);
        assert_eq!(cloud.points[0].rgb, Some([255, 0, 0]));
        let mesh = super::read_mesh(&path).unwrap().unwrap();
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn triangulates_four_vertex_face() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("quad.dxf");
        std::fs::write(&path, "0\nSECTION\n2\nENTITIES\n0\n3DFACE\n10\n0\n20\n0\n11\n1\n21\n0\n12\n1\n22\n1\n13\n0\n23\n1\n0\nENDSEC\n0\nEOF\n").unwrap();
        let mesh = super::read_mesh(&path).unwrap().unwrap();
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }
}
