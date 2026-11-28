//! Binary glTF export for indexed triangle meshes, in metres with Y up.
use crate::{LoadError, MeshGeometry};
use serde_json::{json, Map, Value};
use std::io::Write;

pub(crate) fn write(
    mesh: &MeshGeometry,
    comments: &[&str],
    out: &mut impl Write,
) -> Result<(), LoadError> {
    // Keep survey coordinates in the node transform so nearby vertices retain
    // their separation when consumers upload the local positions as floats.
    // Importers can store the node translation as f32. Put its rounding
    // remainder in the local vertices so it survives that conversion.
    let [ox, oy, oz] = mesh.vertices[0].map(|value| f64::from(value as f32));
    let local = |[x, y, z]: [f64; 3]| [(x - ox) as f32, (z - oz) as f32, (oy - y) as f32];
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in &mesh.vertices {
        for (axis, value) in local(*point).into_iter().enumerate() {
            if !value.is_finite() {
                return Err(LoadError::InvalidData(
                    "GLB mesh exceeds the float coordinate range".into(),
                ));
            }
            min[axis] = min[axis].min(value);
            max[axis] = max[axis].max(value);
        }
    }
    let n = mesh.vertices.len();
    let mut views = vec![json!({"buffer":0,"byteLength":12*n,"target":34962})];
    let mut accessors = vec![
        json!({"bufferView":0,"componentType":5126,"type":"VEC3","count":n,"min":min,"max":max}),
    ];
    let mut attributes = Map::from_iter([("POSITION".into(), json!(0))]);
    let mut offset = 12 * n;
    let normals = mesh
        .normals
        .as_ref()
        .filter(|rows| rows.iter().all(|v| length(*v) > 0.0));
    for (name, present) in [
        ("NORMAL", normals.is_some()),
        ("COLOR_0", mesh.colors.is_some()),
    ] {
        if present {
            let index = views.len();
            views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":12*n,"target":34962}));
            accessors
                .push(json!({"bufferView":index,"componentType":5126,"type":"VEC3","count":n}));
            attributes.insert(name.into(), json!(index));
            offset += 12 * n;
        }
    }
    let indices = views.len();
    let faces = mesh.triangles.len();
    views.push(json!({"buffer":0,"byteOffset":offset,"byteLength":12*faces,"target":34963}));
    accessors
        .push(json!({"bufferView":indices,"componentType":5125,"type":"SCALAR","count":faces*3}));
    let bytes = offset + faces * 12;
    let header = json!({"asset":{"version":"2.0","generator":"Open Pointcloud Studio","extras":{"comments":comments}},
        "scene":0,"scenes":[{"nodes":[0]}],"nodes":[{"mesh":0,"translation":[ox,oz,-oy]}],
        "meshes":[{"primitives":[{"attributes":Value::Object(attributes),"indices":indices}]}],
        "buffers":[{"byteLength":bytes}],"bufferViews":views,"accessors":accessors});
    let mut header = serde_json::to_vec(&header).map_err(std::io::Error::other)?;
    header.resize(header.len().next_multiple_of(4), b' ');
    let size = u32::try_from(28 + header.len() + bytes)
        .map_err(|_| LoadError::InvalidData("GLB exceeds 4 GiB".into()))?;
    out.write_all(b"glTF")?;
    for val in [2, size, header.len() as u32] {
        out.write_all(&val.to_le_bytes())?;
    }
    out.write_all(b"JSON")?;
    out.write_all(&header)?;
    out.write_all(&(bytes as u32).to_le_bytes())?;
    out.write_all(b"BIN\0")?;
    for point in &mesh.vertices {
        write_vec(out, local(*point))?;
    }
    if let Some(normals) = normals {
        for &[x, y, z] in normals {
            let len = length([x, y, z]);
            write_vec(out, [x, z, -y].map(|v| (f64::from(v) / len) as f32))?;
        }
    }
    if let Some(colors) = &mesh.colors {
        for color in colors {
            write_vec(
                out,
                color.map(|v| {
                    let s = f32::from(v) / 255.;
                    if s <= 0.04045 {
                        s / 12.92
                    } else {
                        ((s + 0.055) / 1.055).powf(2.4)
                    }
                }),
            )?;
        }
    }
    for face in &mesh.triangles {
        for index in face {
            out.write_all(&index.to_le_bytes())?;
        }
    }
    Ok(())
}
fn length(normal: [f32; 3]) -> f64 {
    normal
        .into_iter()
        .map(|v| f64::from(v).powi(2))
        .sum::<f64>()
        .sqrt()
}
fn write_vec(out: &mut impl Write, values: [f32; 3]) -> std::io::Result<()> {
    for value in values {
        out.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn survey_mesh_has_local_positions_and_aligned_gltf_attributes() {
        let mesh = MeshGeometry {
            vertices: vec![
                [600000.007, 500000.009, 12.],
                [600000.008, 500000.009, 12.],
                [600000.007, 500001.009, 12.],
            ],
            triangles: vec![[0, 1, 2]],
            colors: Some(vec![[128, 0, 255]; 3]),
            normals: Some(vec![[0., 0., 2.]; 3]),
        };
        let mut glb = Vec::new();
        write(&mesh, &["Survey"], &mut glb).unwrap();
        let word = |at: usize| u32::from_le_bytes(glb[at..at + 4].try_into().unwrap());
        assert_eq!(&glb[..4], b"glTF");
        assert_eq!(word(4), 2);
        assert_eq!(word(8) as usize, glb.len());
        let end = 20 + word(12) as usize;
        let doc: Value = serde_json::from_slice(&glb[20..end]).unwrap();
        assert_eq!(
            doc["nodes"][0]["translation"],
            json!([600000., 12., -500000.])
        );
        assert_eq!(
            doc["meshes"][0]["primitives"][0]["attributes"],
            json!({"POSITION":0,"NORMAL":1,"COLOR_0":2})
        );
        assert_eq!(&glb[end + 4..end + 8], b"BIN\0");
        assert_eq!(word(end) as usize, glb.len() - end - 8);
        let base = end + 8;
        let float = |offset| f32::from_bits(word(base + offset));
        assert!((float(0) - 0.007).abs() < 1e-8);
        assert!((float(12) - float(0) - 0.001).abs() < 1e-8);
        assert!((float(32) + 1.009).abs() < 1e-7);
        assert_eq!(float(40), 1.);
        assert!((float(72) - 0.21586).abs() < 0.00001);
        assert_eq!(word(base + 116), 2);
    }
}
