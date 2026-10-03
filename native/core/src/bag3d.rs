//! 3DBAG CityJSONFeatures import in EPSG:7415 (RD New + NAP).
//! Each API page has its own integer-coordinate transform. Polygon rings,
//! including holes, are triangulated in their dominant local plane.
//! The first response says how many city objects match, so the number of
//! pages is known, and a too-dense area refused, after one request. An area
//! whose buildings prove too detailed for the mesh limits is refused after a
//! few pages.

use std::fmt;
use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::Url;
use serde_json::Value;

use super::LoadError;

const API_ITEMS: &str = "https://api.3dbag.nl/collections/pand/items";
/// The service's maximum for `limit`. It counts city objects, and a building
/// usually comes as two of them (the building and its one part), so a page
/// holds about fifty buildings.
const PAGE_OBJECTS: u64 = 100;
const MAX_PAGES: usize = 100;
/// Pages to read before the mesh size is projected onto the whole area. One
/// page says little: the first page of an old city centre can hold twice the
/// vertices of the pages after it.
const PROJECTION_PAGES: usize = 5;
const MAX_PAGE_BYTES: u64 = 16 * 1024 * 1024;
// These limits are this client's own. The OBJ it writes is opened as a mesh
// layer afterwards, so they may never exceed what the OBJ reader accepts;
// raising the reader's limits does not raise these.
const MAX_VERTICES: usize = 1_000_000;
const MAX_TRIANGLES: usize = 2_000_000;
const _: () = assert!(
    MAX_VERTICES <= super::obj_mesh::MAX_VERTICES
        && MAX_TRIANGLES <= super::obj_mesh::MAX_TRIANGLES
);

/// Identifies the application and its version to the public services it
/// asks for buildings and map tiles.
pub const BAG3D_USER_AGENT: &str = concat!(
    "OpenPointcloudStudio/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/OpenAEC-Foundation/open-pointcloud-studio)"
);

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

    /// Whether the box lies inside the area RD New is valid for. A box typed
    /// in degrees or taken from a scan in local coordinates does not.
    pub fn within_rd_new(self) -> bool {
        self.min_x >= -7_000.0
            && self.max_x <= 300_000.0
            && self.min_y >= 289_000.0
            && self.max_y <= 629_000.0
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

/// State of a running download, reported after every page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BagProgress {
    /// Pages received so far, counted from one.
    pub page: usize,
    /// Pages the area needs; `None` when the service did not say how many
    /// objects matched.
    pub pages: Option<usize>,
    /// Buildings with geometry in the chosen LoD so far.
    pub buildings: usize,
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
    fetch_bag3d_obj_with(bounds, lod, destination, &|_| {}, &AtomicBool::new(false))
}

/// As `fetch_bag3d_obj`, reporting after every page and stopping with
/// `LoadError::Cancelled` once `cancelled` is set. The flag is read before
/// each request and before the file is written; a request already under way
/// is not interrupted, so a cancel can take as long as one page.
pub fn fetch_bag3d_obj_with(
    bounds: BagBounds,
    lod: BagLod,
    destination: impl AsRef<Path>,
    progress: &dyn Fn(BagProgress),
    cancelled: &AtomicBool,
) -> Result<BagStats, LoadError> {
    let client = Client::builder()
        .timeout(Duration::from_secs(40))
        .user_agent(BAG3D_USER_AGENT)
        .build()
        .map_err(network_error)?;
    fetch_from(
        bounds,
        lod,
        destination.as_ref(),
        |url| fetch_page(&client, url),
        progress,
        cancelled,
    )
}

/// The whole download with the page source handed in, so that the tests can
/// serve pages without a network.
fn fetch_from(
    bounds: BagBounds,
    lod: BagLod,
    destination: &Path,
    fetch: impl FnMut(&Url) -> Result<Value, LoadError>,
    progress: &dyn Fn(BagProgress),
    cancelled: &AtomicBool,
) -> Result<BagStats, LoadError> {
    bounds.validate()?;
    let (mesh, pages) = collect_pages(first_page_url(bounds)?, lod, fetch, progress, cancelled)?;
    if mesh.triangles.is_empty() {
        // The service answers a box outside the Netherlands, or one typed in
        // degrees or local scan coordinates, with zero matches and no error.
        // Only such a box gets the hint: over water, or where no building
        // has the chosen LoD, the coordinates are not what is wrong.
        let hint = if bounds.within_rd_new() {
            ""
        } else {
            "; the coordinates must be RD New (EPSG:28992)"
        };
        return Err(LoadError::InvalidData(format!(
            "no 3DBAG buildings with LoD {lod} in this area{hint}"
        )));
    }
    // A cancel during the last request must not still replace the output.
    if cancelled.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled);
    }
    write_obj(destination, lod, &mesh)?;
    Ok(BagStats {
        buildings: mesh.buildings,
        vertices: mesh.vertices.len(),
        triangles: mesh.triangles.len(),
        pages,
    })
}

fn first_page_url(bounds: BagBounds) -> Result<Url, LoadError> {
    let mut url = Url::parse(API_ITEMS).map_err(network_error)?;
    url.query_pairs_mut()
        .append_pair(
            "bbox",
            &format!(
                "{},{},{},{}",
                bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y
            ),
        )
        .append_pair("limit", &PAGE_OBJECTS.to_string());
    Ok(url)
}

fn fetch_page(client: &Client, url: &Url) -> Result<Value, LoadError> {
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
    serde_json::from_slice(&bytes)
        .map_err(|error| LoadError::InvalidData(format!("invalid 3DBAG JSON: {error}")))
}

/// Follow the pages from `first` and gather their buildings. Returns the
/// mesh and the number of pages read.
fn collect_pages(
    first: Url,
    lod: BagLod,
    mut fetch: impl FnMut(&Url) -> Result<Value, LoadError>,
    progress: &dyn Fn(BagProgress),
    cancelled: &AtomicBool,
) -> Result<(BagMesh, usize), LoadError> {
    let mut url = first;
    let mut mesh = BagMesh::default();
    let mut planned = None;
    let mut pages = 0;
    loop {
        // Only reached when the service gave no count to plan with, or kept
        // linking on past it.
        if pages >= MAX_PAGES {
            return Err(LoadError::InvalidData(format!(
                "3DBAG result exceeded {MAX_PAGES} pages; choose a smaller area"
            )));
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        let page = fetch(&url)?;
        if pages == 0 {
            planned = planned_pages(&page)?;
        }
        append_page(&page, lod, &mut mesh)?;
        pages += 1;
        if let Some(planned) = planned {
            check_projected_size(mesh.vertices.len(), mesh.triangles.len(), pages, planned)?;
        }
        progress(BagProgress {
            page: pages,
            // The count is the service's estimate; never show "page 3 of 2".
            pages: planned.map(|planned| planned.max(pages)),
            buildings: mesh.buildings,
        });
        let Some(next) = next_page_url(&url, &page)? else {
            break;
        };
        url = next;
    }
    Ok((mesh, pages))
}

/// How many pages the area needs, from the first response's `numberMatched`.
/// An area that needs more than `MAX_PAGES` is refused here, which costs one
/// request instead of a hundred. `None` when the response carries no count;
/// the page limit in the loop then applies.
fn planned_pages(first_page: &Value) -> Result<Option<usize>, LoadError> {
    let Some(matched) = first_page["numberMatched"].as_u64() else {
        return Ok(None);
    };
    // The response that carried the count was a page itself.
    let pages = matched.div_ceil(PAGE_OBJECTS).max(1);
    if pages > MAX_PAGES as u64 {
        return Err(LoadError::InvalidData(format!(
            "this area holds about {} 3DBAG buildings; at most {} can be downloaded at once, choose a smaller area",
            matched.div_ceil(2),
            MAX_PAGES as u64 * PAGE_OBJECTS / 2
        )));
    }
    Ok(Some(pages as usize))
}

/// Refuse an area whose mesh, at the rate of the pages read so far, will pass
/// the vertex or triangle limit. The page plan counts city objects only, and
/// the buildings of an old city centre carry several times the vertices of
/// suburban ones; without this such an area fails near its last page, after
/// a hundred requests. The margin of a quarter keeps a dense start from
/// refusing an area that would have fitted; whatever ends inside the margin
/// still meets the hard limits while its pages are read.
fn check_projected_size(
    vertices: usize,
    triangles: usize,
    pages: usize,
    planned: usize,
) -> Result<(), LoadError> {
    if pages < PROJECTION_PAGES || pages >= planned {
        return Ok(());
    }
    let projected = |count: usize| count.saturating_mul(planned) / pages;
    let (vertices, triangles) = (projected(vertices), projected(triangles));
    if vertices > MAX_VERTICES + MAX_VERTICES / 4 {
        return Err(LoadError::InvalidData(format!(
            "3DBAG mesh would exceed one million vertices (about {vertices} expected); choose a smaller area"
        )));
    }
    if triangles > MAX_TRIANGLES + MAX_TRIANGLES / 4 {
        return Err(LoadError::InvalidData(format!(
            "3DBAG mesh would exceed two million triangles (about {triangles} expected); choose a smaller area"
        )));
    }
    Ok(())
}

/// The page's `next` link, resolved against the page's own address. A link
/// that leaves the items endpoint is refused: the client only ever talks to
/// the address it was built for, whatever a response says.
fn next_page_url(current: &Url, page: &Value) -> Result<Option<Url>, LoadError> {
    let next = page["links"]
        .as_array()
        .and_then(|links| links.iter().find(|link| link["rel"] == "next"))
        .and_then(|link| link["href"].as_str());
    let Some(next) = next else {
        return Ok(None);
    };
    let next_url = current.join(next).map_err(network_error)?;
    if next_url.scheme() != "https"
        || next_url.host_str() != Some("api.3dbag.nl")
        || !next_url.path().starts_with("/collections/pand/items")
    {
        return Err(LoadError::InvalidData(
            "3DBAG pagination left the API endpoint".into(),
        ));
    }
    Ok(Some(next_url))
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
    use std::cell::{Cell, RefCell};

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

    const AREA: BagBounds = BagBounds {
        min_x: 121_000.0,
        min_y: 487_000.0,
        max_x: 121_100.0,
        max_y: 487_100.0,
    };
    const SECOND_PAGE: &str = "https://api.3dbag.nl/collections/pand/items?bbox=121000,487000,121100,487100&offset=101&limit=100";

    /// A building of 10 by 10 m and 3 m high around a courtyard of 4 by 4 m,
    /// in the shape the service delivers: a `Building` with the footprint and
    /// a `BuildingPart` with one solid per LoD (a plain box in 1.2, the
    /// courtyard cut out in 2.2). `units` is the number of integer steps in
    /// a metre, so the page transform has to bring it back to metres.
    fn courtyard_feature(id: &str, units: i64) -> Value {
        let part = format!("{id}-0");
        let (side, height) = (10 * units, 3 * units);
        let (yard_low, yard_high) = (3 * units, 7 * units);
        let ring = |low: i64, high: i64, z: i64| {
            [
                [low, low, z],
                [high, low, z],
                [high, high, z],
                [low, high, z],
            ]
        };
        let vertices = [
            ring(0, side, 0),
            ring(0, side, height),
            ring(yard_low, yard_high, 0),
            ring(yard_low, yard_high, height),
        ]
        .concat();
        serde_json::json!({
            "type": "CityJSONFeature",
            "id": id,
            "CityObjects": {
                id: {
                    "type": "Building",
                    "children": [part.as_str()],
                    "geometry": [{
                        "type": "MultiSurface",
                        "lod": "0",
                        "boundaries": [[[0, 1, 2, 3], [8, 11, 10, 9]]]
                    }]
                },
                part.as_str(): {
                    "type": "BuildingPart",
                    "parents": [id],
                    "geometry": [
                        {"type": "Solid", "lod": "1.2", "boundaries": [[
                            [[0, 3, 2, 1]], [[4, 5, 6, 7]],
                            [[0, 1, 5, 4]], [[1, 2, 6, 5]], [[2, 3, 7, 6]], [[3, 0, 4, 7]]
                        ]]},
                        {"type": "Solid", "lod": "2.2", "boundaries": [[
                            [[0, 3, 2, 1], [8, 9, 10, 11]],
                            [[4, 5, 6, 7], [12, 15, 14, 13]],
                            [[0, 1, 5, 4]], [[1, 2, 6, 5]], [[2, 3, 7, 6]], [[3, 0, 4, 7]],
                            [[9, 8, 12, 13]], [[10, 9, 13, 14]], [[11, 10, 14, 15]], [[8, 11, 15, 12]]
                        ]]}
                    ]
                }
            },
            "vertices": vertices
        })
    }

    /// A response of the items endpoint. Every feature is two city objects,
    /// which is what `numberReturned` counts.
    fn page(
        scale: f64,
        translate: [f64; 3],
        features: Vec<Value>,
        matched: u64,
        next: Option<&str>,
    ) -> Value {
        let mut links = vec![serde_json::json!({"rel": "self", "href": API_ITEMS})];
        if let Some(next) = next {
            links.push(serde_json::json!({"rel": "next", "href": next}));
        }
        serde_json::json!({
            "type": "FeatureCollection",
            "numberMatched": matched,
            "numberReturned": 2 * features.len(),
            "metadata": {
                "type": "CityJSON",
                "version": "2.0",
                "transform": {"scale": [scale, scale, scale], "translate": translate}
            },
            "features": features,
            "links": links
        })
    }

    /// Two pages that differ in scale and in translation, one building each.
    /// The count says two pages; each carries one building instead of fifty
    /// to keep the fixture readable.
    fn two_pages() -> [Value; 2] {
        [
            page(
                0.001,
                [121_000.0, 487_000.0, 0.0],
                vec![courtyard_feature("NL.IMBAG.Pand.0000000000000001", 1_000)],
                102,
                Some(SECOND_PAGE),
            ),
            page(
                0.01,
                [121_050.5, 487_020.25, 2.0],
                vec![courtyard_feature("NL.IMBAG.Pand.0000000000000002", 100)],
                102,
                None,
            ),
        ]
    }

    /// Serves `pages` in order and notes the addresses asked for. A request
    /// beyond the last page fails the test.
    fn serve<'a>(
        pages: &'a [Value],
        asked: &'a RefCell<Vec<Url>>,
    ) -> impl FnMut(&Url) -> Result<Value, LoadError> + 'a {
        move |url| {
            let mut asked = asked.borrow_mut();
            let page = pages
                .get(asked.len())
                .expect("a request beyond the served pages");
            asked.push(url.clone());
            Ok(page.clone())
        }
    }

    fn surface_area(vertices: &[[f64; 3]], triangles: &[[u32; 3]]) -> f64 {
        triangles
            .iter()
            .map(|face| {
                let [a, b, c] = face.map(|index| vertices[index as usize]);
                let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let cross = [
                    ab[1] * ac[2] - ab[2] * ac[1],
                    ab[2] * ac[0] - ab[0] * ac[2],
                    ab[0] * ac[1] - ab[1] * ac[0],
                ];
                cross.iter().map(|side| side * side).sum::<f64>().sqrt() * 0.5
            })
            .sum()
    }

    fn assert_near(actual: [f64; 3], expected: [f64; 3]) {
        assert!(
            actual
                .iter()
                .zip(expected)
                .all(|(a, e)| (a - e).abs() < 1e-6),
            "{actual:?} is not {expected:?}"
        );
    }

    #[test]
    fn first_request_asks_the_largest_page_the_service_gives() {
        let url = first_page_url(AREA).unwrap();
        assert_eq!(url.path(), "/collections/pand/items");
        let query: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert_eq!(
            query,
            [
                (
                    "bbox".to_string(),
                    "121000,487000,121100,487100".to_string()
                ),
                ("limit".to_string(), "100".to_string()),
            ]
        );
    }

    #[test]
    fn user_agent_names_the_running_version() {
        assert!(BAG3D_USER_AGENT.starts_with(concat!(
            "OpenPointcloudStudio/",
            env!("CARGO_PKG_VERSION"),
            " "
        )));
        assert!(reqwest::header::HeaderValue::from_str(BAG3D_USER_AGENT).is_ok());
    }

    #[test]
    fn pages_are_planned_from_the_matched_count() {
        let planned = |matched: u64| planned_pages(&serde_json::json!({"numberMatched": matched}));
        assert_eq!(planned(88).unwrap(), Some(1));
        assert_eq!(planned(100).unwrap(), Some(1));
        assert_eq!(planned(101).unwrap(), Some(2));
        assert_eq!(planned(10_000).unwrap(), Some(100));
        // The response that says "nothing here" was still one page.
        assert_eq!(planned(0).unwrap(), Some(1));
        let refused = planned(10_001).unwrap_err().to_string();
        assert!(
            refused.contains("about 5001 3DBAG buildings")
                && refused.contains("at most 5000 can be downloaded"),
            "{refused}"
        );
        // Without a usable count the page limit of the loop applies.
        assert_eq!(
            planned_pages(&serde_json::json!({"features": []})).unwrap(),
            None
        );
        assert_eq!(
            planned_pages(&serde_json::json!({"numberMatched": "many"})).unwrap(),
            None
        );
    }

    #[test]
    fn next_link_must_stay_on_the_items_endpoint() {
        let current = first_page_url(AREA).unwrap();
        let linking = |href: &str| {
            next_page_url(
                &current,
                &serde_json::json!({"links": [
                    {"rel": "self", "href": current.as_str()},
                    {"rel": "next", "href": href}
                ]}),
            )
        };
        let next = linking(SECOND_PAGE).unwrap().unwrap();
        assert_eq!(next.as_str(), SECOND_PAGE);
        // A relative link is resolved against the page it came from.
        let relative = linking("?offset=101&limit=100").unwrap().unwrap();
        assert_eq!(relative.host_str(), Some("api.3dbag.nl"));
        assert_eq!(relative.path(), "/collections/pand/items");
        assert_eq!(relative.query(), Some("offset=101&limit=100"));
        for href in [
            "https://example.org/collections/pand/items?offset=101&limit=100",
            "https://api.3dbag.nl.example.org/collections/pand/items?offset=101",
            "http://api.3dbag.nl/collections/pand/items?offset=101&limit=100",
            "https://api.3dbag.nl/collections/other/items?offset=101&limit=100",
            "/api",
        ] {
            assert!(
                matches!(linking(href), Err(LoadError::InvalidData(_))),
                "{href} was followed"
            );
        }
        let last = serde_json::json!({"links": [{"rel": "self", "href": current.as_str()}]});
        assert!(next_page_url(&current, &last).unwrap().is_none());
        let bare = serde_json::json!({"features": []});
        assert!(next_page_url(&current, &bare).unwrap().is_none());
    }

    #[test]
    fn pages_with_their_own_transforms_give_one_mesh() {
        let pages = two_pages();
        let asked = RefCell::new(Vec::new());
        let reports = RefCell::new(Vec::new());
        let (mesh, read) = collect_pages(
            first_page_url(AREA).unwrap(),
            BagLod::Lod22,
            serve(&pages, &asked),
            &|report| reports.borrow_mut().push(report),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(read, 2);
        assert_eq!(asked.borrow()[0], first_page_url(AREA).unwrap());
        assert_eq!(asked.borrow()[1].as_str(), SECOND_PAGE);
        assert_eq!(mesh.buildings, 2);
        assert_eq!(mesh.vertices.len(), 32);
        // Ground corner and opposite eaves corner of each building: the
        // second one is only right with the second page's own transform.
        assert_near(mesh.vertices[0], [121_000.0, 487_000.0, 0.0]);
        assert_near(mesh.vertices[6], [121_010.0, 487_010.0, 3.0]);
        assert_near(mesh.vertices[16], [121_050.5, 487_020.25, 2.0]);
        assert_near(mesh.vertices[22], [121_060.5, 487_030.25, 5.0]);
        // Per building: floor and roof of 100 - 16 m2 each, 120 m2 of outer
        // wall and 48 m2 of courtyard wall.
        assert_eq!(mesh.triangles.len(), 64);
        let area = surface_area(&mesh.vertices, &mesh.triangles);
        assert!((area - 2.0 * 336.0).abs() < 1e-6, "area was {area}");
        assert_eq!(
            *reports.borrow(),
            [
                BagProgress {
                    page: 1,
                    pages: Some(2),
                    buildings: 1
                },
                BagProgress {
                    page: 2,
                    pages: Some(2),
                    buildings: 2
                },
            ]
        );

        // The same pages in LoD 1.2 are two closed boxes.
        let asked = RefCell::new(Vec::new());
        let (boxes, _) = collect_pages(
            first_page_url(AREA).unwrap(),
            BagLod::Lod12,
            serve(&pages, &asked),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(boxes.triangles.len(), 24);
        let area = surface_area(&boxes.vertices, &boxes.triangles);
        assert!((area - 2.0 * 320.0).abs() < 1e-6, "area was {area}");
    }

    #[test]
    fn a_page_counts_buildings_not_city_objects() {
        let pages = [page(
            0.001,
            [121_000.0, 487_000.0, 0.0],
            vec![
                courtyard_feature("NL.IMBAG.Pand.0000000000000001", 1_000),
                courtyard_feature("NL.IMBAG.Pand.0000000000000002", 1_000),
            ],
            4,
            None,
        )];
        assert_eq!(pages[0]["numberReturned"], 4);
        assert_eq!(pages[0]["features"].as_array().unwrap().len(), 2);
        let asked = RefCell::new(Vec::new());
        let reports = RefCell::new(Vec::new());
        let (mesh, read) = collect_pages(
            first_page_url(AREA).unwrap(),
            BagLod::Lod22,
            serve(&pages, &asked),
            &|report| reports.borrow_mut().push(report),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(read, 1);
        assert_eq!(mesh.buildings, 2);
        // The second feature's indices start again at zero.
        assert_eq!(mesh.vertices.len(), 32);
        assert_eq!(mesh.triangles.len(), 64);
        assert_eq!(
            *reports.borrow(),
            [BagProgress {
                page: 1,
                pages: Some(1),
                buildings: 2
            }]
        );
    }

    #[test]
    fn download_writes_an_obj_the_mesh_reader_opens() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        let pages = two_pages();
        let asked = RefCell::new(Vec::new());
        let stats = fetch_from(
            AREA,
            BagLod::Lod22,
            &destination,
            serve(&pages, &asked),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            stats,
            BagStats {
                buildings: 2,
                vertices: 32,
                triangles: 64,
                pages: 2
            }
        );
        let text = std::fs::read_to_string(&destination).unwrap();
        assert!(text.starts_with("# © 3DBAG"), "attribution comes first");
        assert!(text.contains("# EPSG:7415 RD New + NAP, LoD 2.2"));
        let reopened = super::super::obj_mesh::read_obj_mesh(&destination).unwrap();
        assert_eq!(reopened.vertices.len(), 32);
        assert_eq!(reopened.triangles.len(), 64);
        assert_near(reopened.vertices[22], [121_060.5, 487_030.25, 5.0]);
        // Only the result is left beside it, no temporary file.
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancel_before_the_first_request_keeps_the_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        std::fs::write(&destination, "earlier download").unwrap();
        let requests = Cell::new(0);
        let result = fetch_from(
            AREA,
            BagLod::Lod22,
            &destination,
            |_| {
                requests.set(requests.get() + 1);
                Ok(two_pages()[1].clone())
            },
            &|_| {},
            &AtomicBool::new(true),
        );
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(requests.get(), 0);
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "earlier download"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn cancel_between_pages_stops_before_the_next_request() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        std::fs::write(&destination, "earlier download").unwrap();
        let pages = two_pages();
        let asked = RefCell::new(Vec::new());
        let cancelled = AtomicBool::new(false);
        let result = fetch_from(
            AREA,
            BagLod::Lod22,
            &destination,
            // Only the first page is served, so asking on would fail.
            serve(&pages[..1], &asked),
            &|_| cancelled.store(true, Ordering::Relaxed),
            &cancelled,
        );
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(asked.borrow().len(), 1);
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "earlier download"
        );
    }

    #[test]
    fn cancel_during_the_last_page_writes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        let pages = [two_pages()[1].clone()];
        let asked = RefCell::new(Vec::new());
        let cancelled = AtomicBool::new(false);
        let result = fetch_from(
            AREA,
            BagLod::Lod22,
            &destination,
            serve(&pages, &asked),
            &|_| cancelled.store(true, Ordering::Relaxed),
            &cancelled,
        );
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn too_dense_area_is_refused_after_one_request() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        let pages = [page(
            0.001,
            [121_000.0, 487_000.0, 0.0],
            vec![courtyard_feature("NL.IMBAG.Pand.0000000000000001", 1_000)],
            23_977,
            Some(SECOND_PAGE),
        )];
        let asked = RefCell::new(Vec::new());
        let reports = Cell::new(0);
        let error = fetch_from(
            AREA,
            BagLod::Lod22,
            &destination,
            serve(&pages, &asked),
            &|_| reports.set(reports.get() + 1),
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("about 11989 3DBAG buildings") && error.contains("smaller area"),
            "{error}"
        );
        assert_eq!(asked.borrow().len(), 1);
        assert_eq!(reports.get(), 0);
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn mesh_size_is_projected_only_after_several_pages() {
        // Vertices of the first pages of a dense city centre with 63 pages.
        // The first page alone, taken for all hundred pages of the largest
        // area, would refuse what in fact fits.
        assert!(check_projected_size(18_093, 0, 1, 100).is_ok());
        assert!(check_projected_size(18_093 + 5_400 + 4_004 + 9_200 + 9_200, 0, 5, 63).is_ok());
        // 10,000 a page is exactly the limit over a hundred pages, and a
        // quarter above it is still left to the hard limit.
        assert!(check_projected_size(50_000, 60_000, 5, 100).is_ok());
        assert!(check_projected_size(62_500, 60_000, 5, 100).is_ok());
        let vertices = check_projected_size(62_505, 60_000, 5, 100)
            .unwrap_err()
            .to_string();
        assert!(
            vertices.contains("one million vertices")
                && vertices.contains("about 1250100 expected")
                && vertices.contains("smaller area"),
            "{vertices}"
        );
        let triangles = check_projected_size(50_000, 130_000, 5, 100)
            .unwrap_err()
            .to_string();
        assert!(
            triangles.contains("two million triangles") && triangles.contains("about 2600000"),
            "{triangles}"
        );
        // The same rate is no reason to stop a short download, and the last
        // page is never projected: what was read is what there is.
        assert!(check_projected_size(62_505, 60_000, 5, 20).is_ok());
        assert!(check_projected_size(999_999, 1_999_999, 100, 100).is_ok());
        // A count that proved too low leaves nothing to project onto.
        assert!(check_projected_size(999_999, 0, 7, 6).is_ok());
    }

    /// A courtyard building whose vertex list is padded to `vertices`
    /// entries. The service's features carry the vertices of every LoD, so
    /// most of a real list is not used by the chosen one either.
    fn detailed_feature(id: &str, vertices: usize) -> Value {
        let mut feature = courtyard_feature(id, 1_000);
        feature["vertices"]
            .as_array_mut()
            .unwrap()
            .resize(vertices, serde_json::json!([0, 0, 0]));
        feature
    }

    #[test]
    fn too_detailed_area_is_refused_after_a_few_pages() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        std::fs::write(&destination, "earlier download").unwrap();
        // The largest area the page plan admits, at 13,000 vertices a page:
        // 1.3 million in all, which the vertex limit would only stop at
        // page 77.
        let dense = page(
            0.001,
            [121_000.0, 487_000.0, 0.0],
            vec![detailed_feature("NL.IMBAG.Pand.0000000000000001", 13_000)],
            10_000,
            Some(SECOND_PAGE),
        );
        let requests = Cell::new(0);
        let reports = Cell::new(0);
        let error = fetch_from(
            AREA,
            BagLod::Lod22,
            &destination,
            |_| {
                requests.set(requests.get() + 1);
                Ok(dense.clone())
            },
            &|_| reports.set(reports.get() + 1),
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("would exceed one million vertices")
                && error.contains("about 1300000 expected")
                && error.contains("smaller area"),
            "{error}"
        );
        assert_eq!(requests.get(), PROJECTION_PAGES);
        assert_eq!(reports.get(), PROJECTION_PAGES - 1);
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "earlier download"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn detailed_start_of_an_area_that_fits_is_read_to_the_end() {
        // A hundred pages. The first holds 13,000 vertices, which taken
        // alone projects 1.3 million; the others hold one plain building.
        let pages: Vec<Value> = (0..MAX_PAGES)
            .map(|number| {
                let feature = if number == 0 {
                    detailed_feature("NL.IMBAG.Pand.0000000000000001", 13_000)
                } else {
                    courtyard_feature("NL.IMBAG.Pand.0000000000000002", 1_000)
                };
                page(
                    0.001,
                    [121_000.0, 487_000.0, 0.0],
                    vec![feature],
                    10_000,
                    (number + 1 < MAX_PAGES).then_some(SECOND_PAGE),
                )
            })
            .collect();
        let asked = RefCell::new(Vec::new());
        let (mesh, read) = collect_pages(
            first_page_url(AREA).unwrap(),
            BagLod::Lod22,
            serve(&pages, &asked),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(read, MAX_PAGES);
        assert_eq!(mesh.vertices.len(), 13_000 + 99 * 16);
        assert_eq!(mesh.buildings, MAX_PAGES);
    }

    #[test]
    fn without_a_count_the_page_limit_stops_the_download() {
        // A service that never says how much there is and always links on.
        let endless = serde_json::json!({
            "features": [],
            "links": [{"rel": "next", "href": SECOND_PAGE}]
        });
        let requests = Cell::new(0);
        let last_report = Cell::new(None);
        let result = collect_pages(
            first_page_url(AREA).unwrap(),
            BagLod::Lod22,
            |_| {
                requests.set(requests.get() + 1);
                Ok(endless.clone())
            },
            &|report| last_report.set(Some(report)),
            &AtomicBool::new(false),
        );
        let error = result.err().unwrap().to_string();
        assert!(error.contains("exceeded 100 pages"), "{error}");
        assert_eq!(requests.get(), MAX_PAGES);
        assert_eq!(
            last_report.get(),
            Some(BagProgress {
                page: MAX_PAGES,
                pages: None,
                buildings: 0
            })
        );
    }

    #[test]
    fn a_count_that_proves_too_low_never_shows_a_page_past_the_end() {
        let mut pages = two_pages();
        for page in &mut pages {
            page["numberMatched"] = serde_json::json!(88);
        }
        let asked = RefCell::new(Vec::new());
        let reports = RefCell::new(Vec::new());
        collect_pages(
            first_page_url(AREA).unwrap(),
            BagLod::Lod22,
            serve(&pages, &asked),
            &|report: BagProgress| reports.borrow_mut().push((report.page, report.pages)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(*reports.borrow(), [(1, Some(1)), (2, Some(2))]);
    }

    #[test]
    fn empty_area_names_the_coordinate_system() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        std::fs::write(&destination, "earlier download").unwrap();
        // What the service answers for a box given in degrees.
        let pages = [serde_json::json!({
            "type": "FeatureCollection",
            "numberMatched": 0,
            "numberReturned": 0,
            "features": [],
            "links": [{"rel": "self", "href": API_ITEMS}]
        })];
        let asked = RefCell::new(Vec::new());
        let error = fetch_from(
            BagBounds {
                min_x: 4.845,
                min_y: 52.3515,
                max_x: 4.9562,
                max_y: 52.4035,
            },
            BagLod::Lod22,
            &destination,
            serve(&pages, &asked),
            &|_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("no 3DBAG buildings with LoD 2.2")
                && error.contains("RD New (EPSG:28992)"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "earlier download"
        );
    }

    #[test]
    fn rd_new_area_takes_rd_boxes_only() {
        let within = |text: &str| BagBounds::parse(text).unwrap().within_rd_new();
        assert!(AREA.within_rd_new());
        assert!(within("91440,398430,91460,398450"));
        // Degrees, a scan in local coordinates, and a box off the edge.
        assert!(!within("4.845,52.3515,4.9562,52.4035"));
        assert!(!within("0,0,50,50"));
        assert!(!within("299500,400000,300500,401000"));
    }

    #[test]
    fn empty_rd_area_does_not_blame_the_coordinates() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("buildings.obj");
        let refusal = |lod: BagLod, only_page: &Value| {
            fetch_from(
                AREA,
                lod,
                &destination,
                |_| Ok(only_page.clone()),
                &|_| {},
                &AtomicBool::new(false),
            )
            .unwrap_err()
            .to_string()
        };
        // Water or farmland: a valid box in which nothing matches.
        let water = page(0.001, [121_000.0, 487_000.0, 0.0], vec![], 0, None);
        let error = refusal(BagLod::Lod22, &water);
        assert!(
            error.ends_with("no 3DBAG buildings with LoD 2.2 in this area"),
            "{error}"
        );
        // Objects matched, so the box was read as RD, but none of them has
        // the chosen LoD.
        let other_lods = page(
            0.001,
            [121_000.0, 487_000.0, 0.0],
            vec![courtyard_feature("NL.IMBAG.Pand.0000000000000001", 1_000)],
            2,
            None,
        );
        let error = refusal(BagLod::Lod13, &other_lods);
        assert!(
            error.ends_with("no 3DBAG buildings with LoD 1.3 in this area"),
            "{error}"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    /// Asks the real service, so it is not part of the normal run. Run it by
    /// hand before a release:
    /// `cargo test -p pointcloud-core -- --ignored live_services`
    #[test]
    #[ignore = "needs the network and the public 3DBAG service"]
    fn live_services_answer_in_the_expected_shape() {
        let client = Client::builder()
            .timeout(Duration::from_secs(40))
            .user_agent(BAG3D_USER_AGENT)
            .build()
            .unwrap();
        let first = first_page_url(AREA).unwrap();
        let page = fetch_page(&client, &first).unwrap();
        for key in [
            "features",
            "links",
            "metadata",
            "numberMatched",
            "numberReturned",
            "type",
        ] {
            assert!(page.get(key).is_some(), "the response has no {key}");
        }
        array3(&page["metadata"]["transform"]["scale"]).unwrap();
        array3(&page["metadata"]["transform"]["translate"]).unwrap();
        assert!(planned_pages(&page).unwrap().is_some());
        let mut mesh = BagMesh::default();
        append_page(&page, BagLod::Lod22, &mut mesh).unwrap();
        assert!(mesh.buildings > 0 && !mesh.triangles.is_empty());
        // The count is in city objects, more than one per building.
        let features = page["features"].as_array().unwrap().len() as u64;
        assert!(page["numberReturned"].as_u64().unwrap() > features);

        // Asking more than the maximum still gives the maximum, with a link
        // to the rest that this client accepts.
        let mut greedy = Url::parse(API_ITEMS).unwrap();
        greedy
            .query_pairs_mut()
            .append_pair("bbox", "121000,487000,122000,488000")
            .append_pair("limit", "1000");
        let page = fetch_page(&client, &greedy).unwrap();
        assert_eq!(page["numberReturned"].as_u64(), Some(PAGE_OBJECTS));
        assert!(page["numberMatched"].as_u64().unwrap() > PAGE_OBJECTS);
        assert!(next_page_url(&greedy, &page).unwrap().is_some());
    }
}
