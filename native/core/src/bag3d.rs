//! 3DBAG CityJSONFeatures import in EPSG:7415 (RD New + NAP).
//! Each API page has its own integer-coordinate transform. Polygon rings,
//! including holes, are triangulated in their dominant local plane.

use std::fmt;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::Url;
use serde_json::Value;

use super::LoadError;

const API_ITEMS: &str = "https://api.3dbag.nl/collections/pand/items";
const MAX_PAGES: usize = 100;
const MAX_PAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_VERTICES: usize = 1_000_000;
const MAX_TRIANGLES: usize = 2_000_000;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BagBounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl BagBounds {
    pub fn parse(text: &str) -> Result<Self, LoadError> {
        let values = text
            .split(',')
            .map(|part| part.trim().parse::<f64>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| LoadError::InvalidData("invalid 3DBAG bbox".into()))?;
        let [min_x, min_y, max_x, max_y] = values.as_slice() else {
            return Err(LoadError::InvalidData(
                "3DBAG bbox needs xmin,ymin,xmax,ymax".into(),
            ));
        };
        let bounds = Self {
            min_x: *min_x,
            min_y: *min_y,
            max_x: *max_x,
            max_y: *max_y,
        };
        bounds.validate()?;
        Ok(bounds)
    }

    pub fn validate(self) -> Result<(), LoadError> {
        if ![self.min_x, self.min_y, self.max_x, self.max_y]
            .iter()
            .all(|value| value.is_finite())
            || self.max_x <= self.min_x
            || self.max_y <= self.min_y
            || self.max_x - self.min_x > 2_000.0
            || self.max_y - self.min_y > 2_000.0
        {
            return Err(LoadError::InvalidData(
                "3DBAG bbox must have positive sides no longer than 2 km".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BagLod {
    Lod12,
    Lod13,
    Lod22,
}

impl BagLod {
    pub const ALL: [Self; 3] = [Self::Lod12, Self::Lod13, Self::Lod22];

    fn as_str(self) -> &'static str {
        match self {
            Self::Lod12 => "1.2",
            Self::Lod13 => "1.3",
            Self::Lod22 => "2.2",
        }
    }

    pub fn parse(text: &str) -> Result<Self, LoadError> {
        match text {
            "1.2" => Ok(Self::Lod12),
            "1.3" => Ok(Self::Lod13),
            "2.2" => Ok(Self::Lod22),
            _ => Err(LoadError::InvalidData("invalid 3DBAG LoD".into())),
        }
    }
}

impl fmt::Display for BagLod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BagStats {
    pub buildings: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub pages: usize,
}

#[derive(Default)]
struct BagMesh {
    vertices: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    buildings: usize,
}

/// Download 3DBAG buildings for an RD bounding box and save their chosen LoD
/// as a georeferenced OBJ. A failed or partial download never replaces output.
pub fn fetch_bag3d_obj(
    bounds: BagBounds,
    lod: BagLod,
    destination: impl AsRef<Path>,
) -> Result<BagStats, LoadError> {
    bounds.validate()?;
    let client = Client::builder()
        .timeout(Duration::from_secs(40))
        .build()
        .map_err(network_error)?;
    let mut url = Url::parse(API_ITEMS).map_err(network_error)?;
    url.query_pairs_mut()
        .append_pair(
            "bbox",
            &format!(
                "{},{},{},{}",
                bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y
            ),
        )
        .append_pair("limit", "50");
    let mut mesh = BagMesh::default();
    let mut pages = 0;
    loop {
        if pages >= MAX_PAGES {
            return Err(LoadError::InvalidData(
                "3DBAG result exceeded 100 pages; choose a smaller area".into(),
            ));
        }
        let response = client
            .get(url.clone())
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(network_error)?;
        let mut bytes = Vec::new();
        response.take(MAX_PAGE_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_PAGE_BYTES {
            return Err(LoadError::InvalidData("3DBAG page exceeded 16 MB".into()));
        }
        let page: Value = serde_json::from_slice(&bytes)
            .map_err(|error| LoadError::InvalidData(format!("invalid 3DBAG JSON: {error}")))?;
        append_page(&page, lod, &mut mesh)?;
        pages += 1;
        let next = page["links"]
            .as_array()
            .and_then(|links| links.iter().find(|link| link["rel"] == "next"))
            .and_then(|link| link["href"].as_str());
        let Some(next) = next else { break };
        let next_url = url.join(next).map_err(network_error)?;
        if next_url.scheme() != "https"
            || next_url.host_str() != Some("api.3dbag.nl")
            || !next_url.path().starts_with("/collections/pand/items")
        {
            return Err(LoadError::InvalidData(
                "3DBAG pagination left the API endpoint".into(),
            ));
        }
        url = next_url;
    }
    if mesh.triangles.is_empty() {
        return Err(LoadError::InvalidData(format!(
            "no 3DBAG buildings with LoD {lod} in this area"
        )));
    }
    write_obj(destination.as_ref(), lod, &mesh)?;
    Ok(BagStats {
        buildings: mesh.buildings,
        vertices: mesh.vertices.len(),
        triangles: mesh.triangles.len(),
        pages,
    })
}

fn network_error(error: impl fmt::Display) -> LoadError {
    LoadError::InvalidData(format!("3DBAG request failed: {error}"))
}

fn write_obj(destination: &Path, lod: BagLod, mesh: &BagMesh) -> Result<(), LoadError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writeln!(writer, "# © 3DBAG door tudelft3d en 3DGI · CC BY 4.0")?;
        writeln!(writer, "# https://docs.3dbag.nl/nl/copyright/")?;
        writeln!(
            writer,
            "# Converted and triangulated by Open Pointcloud Studio"
        )?;
        writeln!(writer, "# EPSG:7415 RD New + NAP, LoD {lod}")?;
        writeln!(writer, "o 3DBAG")?;
        for xyz in &mesh.vertices {
            writeln!(writer, "v {:.9} {:.9} {:.9}", xyz[0], xyz[1], xyz[2])?;
        }
        for [a, b, c] in &mesh.triangles {
            writeln!(writer, "f {} {} {}", a + 1, b + 1, c + 1)?;
        }
        writer.flush()?;
    }
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(())
}

fn append_page(page: &Value, lod: BagLod, mesh: &mut BagMesh) -> Result<(), LoadError> {
    let features: Vec<&Value> = if let Some(features) = page["features"].as_array() {
        features.iter().collect()
    } else if let Some(feature) = page.get("feature") {
        vec![feature]
    } else {
        return Err(LoadError::InvalidData("3DBAG page has no features".into()));
    };
    if features.is_empty() {
        return Ok(());
    }
    let transform = &page["metadata"]["transform"];
    let scale = array3(&transform["scale"])?;
    let translate = array3(&transform["translate"])?;
    for feature in features {
        let vertices = feature["vertices"]
            .as_array()
            .ok_or_else(|| LoadError::InvalidData("3DBAG feature has no vertices".into()))?;
        if mesh.vertices.len() + vertices.len() > MAX_VERTICES {
            return Err(LoadError::InvalidData(
                "3DBAG mesh exceeded one million vertices; choose a smaller area".into(),
            ));
        }
        let base = mesh.vertices.len();
        for vertex in vertices {
            let raw = array3(vertex)?;
            let xyz = std::array::from_fn(|axis| raw[axis] * scale[axis] + translate[axis]);
            if !xyz.iter().all(|value| value.is_finite()) {
                return Err(LoadError::InvalidData("non-finite 3DBAG vertex".into()));
            }
            mesh.vertices.push(xyz);
        }
        let mut has_building = false;
        if let Some(objects) = feature["CityObjects"].as_object() {
            for object in objects.values() {
                if !matches!(
                    object["type"].as_str(),
                    Some("Building" | "BuildingPart" | "BuildingInstallation")
                ) {
                    continue;
                }
                let Some(geometries) = object["geometry"].as_array() else {
                    continue;
                };
                for geometry in geometries {
                    if geometry["lod"].as_str() != Some(lod.as_str()) {
                        continue;
                    }
                    let before = mesh.triangles.len();
                    append_geometry(geometry, base, mesh)?;
                    has_building |= mesh.triangles.len() > before;
                }
            }
        }
        mesh.buildings += usize::from(has_building);
    }
    Ok(())
}

fn array3(value: &Value) -> Result<[f64; 3], LoadError> {
    let array = value
        .as_array()
        .filter(|array| array.len() == 3)
        .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG coordinate".into()))?;
    let mut result = [0.0; 3];
    for axis in 0..3 {
        result[axis] = array[axis]
            .as_f64()
            .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG coordinate".into()))?;
    }
    Ok(result)
}

fn append_geometry(geometry: &Value, base: usize, mesh: &mut BagMesh) -> Result<(), LoadError> {
    let outer = geometry["boundaries"]
        .as_array()
        .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG boundaries".into()))?;
    match geometry["type"].as_str() {
        Some("MultiSurface" | "CompositeSurface") => {
            for face in outer {
                append_face(face, base, mesh)?;
            }
        }
        Some("Solid") => {
            for shell in outer {
                for face in shell
                    .as_array()
                    .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG solid shell".into()))?
                {
                    append_face(face, base, mesh)?;
                }
            }
        }
        Some("CompositeSolid") => {
            for solid in outer {
                for shell in solid
                    .as_array()
                    .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG composite solid".into()))?
                {
                    for face in shell.as_array().ok_or_else(|| {
                        LoadError::InvalidData("invalid 3DBAG composite shell".into())
                    })? {
                        append_face(face, base, mesh)?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn append_face(face: &Value, base: usize, mesh: &mut BagMesh) -> Result<(), LoadError> {
    let rings = face
        .as_array()
        .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG face".into()))?;
    let mut vertex_ids = Vec::<u32>::new();
    let mut holes = Vec::<usize>::new();
    for (ring_number, ring) in rings.iter().enumerate() {
        let entries = ring
            .as_array()
            .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG ring".into()))?;
        if ring_number > 0 {
            holes.push(vertex_ids.len());
        }
        let mut parsed = Vec::with_capacity(entries.len());
        for entry in entries {
            let index = entry
                .as_u64()
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| LoadError::InvalidData("invalid 3DBAG vertex index".into()))?;
            if base + index >= mesh.vertices.len() {
                return Err(LoadError::InvalidData(
                    "3DBAG face index out of range".into(),
                ));
            }
            parsed.push((base + index) as u32);
        }
        if parsed.first() == parsed.last() {
            parsed.pop();
        }
        if parsed.len() < 3 {
            return Err(LoadError::InvalidData(
                "3DBAG ring has fewer than 3 points".into(),
            ));
        }
        vertex_ids.extend(parsed);
    }
    if vertex_ids.len() < 3 {
        return Ok(());
    }
    let first = mesh.vertices[vertex_ids[0] as usize];
    let mut normal = [0.0; 3];
    let outer_end = holes.first().copied().unwrap_or(vertex_ids.len());
    for i in 0..outer_end {
        let a = mesh.vertices[vertex_ids[i] as usize];
        let b = mesh.vertices[vertex_ids[(i + 1) % outer_end] as usize];
        normal[0] += (a[1] - b[1]) * (a[2] + b[2]);
        normal[1] += (a[2] - b[2]) * (a[0] + b[0]);
        normal[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let drop_axis = (0..3)
        .max_by(|a, b| normal[*a].abs().total_cmp(&normal[*b].abs()))
        .unwrap_or(2);
    let keep = match drop_axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    let mut coordinates = Vec::with_capacity(vertex_ids.len() * 2);
    for id in &vertex_ids {
        let xyz = mesh.vertices[*id as usize];
        coordinates.push(xyz[keep[0]] - first[keep[0]]);
        coordinates.push(xyz[keep[1]] - first[keep[1]]);
    }
    let indices = earcutr::earcut(&coordinates, &holes, 2)
        .map_err(|error| LoadError::InvalidData(format!("3DBAG triangulation failed: {error}")))?;
    if mesh.triangles.len() + indices.len() / 3 > MAX_TRIANGLES {
        return Err(LoadError::InvalidData(
            "3DBAG mesh exceeded two million triangles; choose a smaller area".into(),
        ));
    }
    for triangle in indices.as_chunks::<3>().0 {
        let mut face = [
            vertex_ids[triangle[0]],
            vertex_ids[triangle[1]],
            vertex_ids[triangle[2]],
        ];
        let a = mesh.vertices[face[0] as usize];
        let b = mesh.vertices[face[1] as usize];
        let c = mesh.vertices[face[2] as usize];
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        if cross
            .iter()
            .map(|component| component * component)
            .sum::<f64>()
            < 1e-18
        {
            continue;
        }
        if cross.iter().zip(normal).map(|(a, b)| a * b).sum::<f64>() < 0.0 {
            face.swap(1, 2);
        }
        mesh.triangles.push(face);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bbox_and_rejects_unbounded_area() {
        assert_eq!(BagBounds::parse("1,2,3,4").unwrap().max_y, 4.0);
        assert!(BagBounds::parse("1,2,3").is_err());
        assert!(BagBounds::parse("0,0,3000,1").is_err());
    }

    #[test]
    fn page_transform_and_polygon_hole_are_respected() {
        let page: Value = serde_json::from_str(
            r#"{"metadata":{"transform":{"scale":[0.001,0.001,0.001],"translate":[100,200,3]}},"features":[{"vertices":[[0,0,0],[10000,0,0],[10000,10000,0],[0,10000,0],[3000,3000,0],[7000,3000,0],[7000,7000,0],[3000,7000,0]],"CityObjects":{"a":{"type":"BuildingPart","geometry":[{"type":"MultiSurface","lod":"2.2","boundaries":[[[0,1,2,3],[4,5,6,7]]]}]}}}]}"#,
        )
        .unwrap();
        let mut mesh = BagMesh::default();
        append_page(&page, BagLod::Lod22, &mut mesh).unwrap();
        assert_eq!(mesh.buildings, 1);
        assert_eq!(mesh.vertices[0], [100.0, 200.0, 3.0]);
        assert_eq!(mesh.vertices[2], [110.0, 210.0, 3.0]);
        let area: f64 = mesh
            .triangles
            .iter()
            .map(|face| {
                let [a, b, c] = face.map(|i| mesh.vertices[i as usize]);
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5
            })
            .sum();
        assert!((area - 84.0).abs() < 1e-6, "area was {area}");
        let mut next_page = page.clone();
        next_page["metadata"]["transform"]["translate"] = serde_json::json!([200, 300, 4]);
        append_page(&next_page, BagLod::Lod22, &mut mesh).unwrap();
        assert_eq!(mesh.buildings, 2);
        assert_eq!(mesh.vertices[8], [200.0, 300.0, 4.0]);
    }

    #[test]
    fn empty_bbox_page_needs_no_transform() {
        let page = serde_json::json!({"features": [], "links": []});
        let mut mesh = BagMesh::default();
        append_page(&page, BagLod::Lod12, &mut mesh).unwrap();
        assert!(mesh.triangles.is_empty());
    }
}
