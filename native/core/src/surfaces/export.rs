//! Detected faces as files and as meshes for the viewer.
//!
//! - OBJ: every face and cylinder as a group of triangles, for use as a
//!   light mesh.
//! - JSON: every face with its plane, outline and residuals and every
//!   cylinder with its axis and radius, for anything that wants the
//!   numbers. The layout is described at `faces_json`.
//! - Two meshes for the viewer: the faces in a colour each, and the faces
//!   coloured by how far the scan lies from them.
//!
//! The files hold scene coordinates. The viewer meshes hold the coordinates
//! of the anchor layer's source, because the viewer places a mesh with the
//! transform of its layer.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use serde_json::{json, Value};

use super::{CylinderFace, DetectedSurfaces, FaceClass, PlaneFace};
use crate::grid2d::{ring_contains, Region};
use crate::local_fit::{cross, difference, dot, unit};
use crate::obj_mesh::{MAX_TRIANGLES, MAX_VERTICES};
use crate::region_source::SourceTransform;
use crate::{LoadError, MeshGeometry};

/// Value of `format` in the JSON export.
pub const FACES_JSON_FORMAT: &str = "open-pointcloud-studio-faces";
/// Value of `version` in the JSON export.
pub const FACES_JSON_VERSION: u32 = 1;
/// The most grid cells the deviation mesh shows before it merges cells.
pub const DEFAULT_DEVIATION_CELLS: usize = 400_000;

const COLOR_BEHIND: [u8; 3] = [33, 102, 172];
const COLOR_ON: [u8; 3] = [240, 240, 240];
const COLOR_FRONT: [u8; 3] = [178, 24, 43];

/// The colour of a class of faces.
pub fn class_color(class: FaceClass) -> [u8; 3] {
    match class {
        FaceClass::Floor => [124, 172, 112],
        FaceClass::Ceiling => [150, 182, 222],
        FaceClass::Wall => [226, 192, 132],
        FaceClass::Sloped => [206, 132, 152],
    }
}

/// The colour of a face in the flat mesh: that of its class, in one of four
/// shades so that neighbours of one class can be told apart.
pub fn face_color(face: &PlaneFace) -> [u8; 3] {
    let shade = 0.82 + 0.06 * f64::from(face.id % 4);
    class_color(face.class).map(|value| (f64::from(value) * shade).round().min(255.0) as u8)
}

/// The colour of a cylinder in the flat mesh, in one of four shades.
pub fn cylinder_color(face: &CylinderFace) -> [u8; 3] {
    let shade = 0.82 + 0.06 * f64::from(face.id % 4);
    [176u8, 152, 206].map(|value| (f64::from(value) * shade).round().min(255.0) as u8)
}

/// The scanned part of a cylinder as corners with their normals and
/// triangles: strips of at most 7.5 degrees, at least twelve, over its
/// length. The normals point to the side it was scanned from, and the
/// triangles turn that way.
struct MantleMesh {
    corners: Vec<([f64; 3], [f64; 3])>,
    triangles: Vec<[usize; 3]>,
}

fn mantle(face: &CylinderFace) -> MantleMesh {
    let strips = ((face.arc_deg / 7.5).ceil() as usize).max(12);
    let length = face.length();
    let facing = if face.seen_from_inside { -1.0 } else { 1.0 };
    let mut corners = Vec::with_capacity(2 * (strips + 1));
    for strip in 0..=strips {
        let angle = face.arc_deg * strip as f64 / strips as f64;
        let normal = face.outward(angle).map(|value| value * facing);
        corners.push((face.point(angle, 0.0), normal));
        corners.push((face.point(angle, length), normal));
    }
    let mut triangles = Vec::with_capacity(2 * strips);
    for strip in 0..strips {
        let quad = [2 * strip, 2 * strip + 2, 2 * strip + 3, 2 * strip + 1];
        for triangle in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
            triangles.push(turned(triangle, &corners));
        }
    }
    MantleMesh { corners, triangles }
}

/// A triangle with its corners in the order that makes it face the way the
/// normal of its first corner points.
fn turned(triangle: [usize; 3], corners: &[([f64; 3], [f64; 3])]) -> [usize; 3] {
    let [a, b, c] = triangle.map(|index| corners[index].0);
    let winding = cross(difference(b, a), difference(c, a));
    if dot(winding, corners[triangle[0]].1) < 0.0 {
        [triangle[0], triangle[2], triangle[1]]
    } else {
        triangle
    }
}

/// One of the three colours of the deviation mesh and the deviation it
/// stands for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeviationStop {
    pub value: f64,
    pub color: [u8; 3],
}

/// The three colours of the deviation mesh for a scale: scan behind the
/// face by `scale` or more, scan on the face, scan in front of it by
/// `scale` or more. In between the colours blend evenly.
pub fn deviation_legend(scale: f64) -> [DeviationStop; 3] {
    [
        DeviationStop {
            value: -scale,
            color: COLOR_BEHIND,
        },
        DeviationStop {
            value: 0.0,
            color: COLOR_ON,
        },
        DeviationStop {
            value: scale,
            color: COLOR_FRONT,
        },
    ]
}

/// The colour for a deviation, as `deviation_legend` lays it out.
pub fn deviation_color(value: f64, scale: f64) -> [u8; 3] {
    let part = if scale > 0.0 && value.is_finite() {
        (value / scale).clamp(-1.0, 1.0)
    } else {
        0.0
    };
    let end = if part < 0.0 {
        COLOR_BEHIND
    } else {
        COLOR_FRONT
    };
    std::array::from_fn(|channel| {
        (f64::from(COLOR_ON[channel])
            + (f64::from(end[channel]) - f64::from(COLOR_ON[channel])) * part.abs())
        .round() as u8
    })
}

/// The outline of one part of a face as triangles.
struct Triangulated {
    /// The corners of the outer ring and then of every hole.
    corners: Vec<[f64; 2]>,
    /// Positions in `corners`; every triangle runs counter-clockwise in
    /// outline coordinates.
    triangles: Vec<[usize; 3]>,
}

fn triangulate(patch: &Region) -> Result<Triangulated, LoadError> {
    let mut corners = patch.outer.clone();
    let mut hole_starts = Vec::with_capacity(patch.holes.len());
    for hole in &patch.holes {
        hole_starts.push(corners.len());
        corners.extend_from_slice(hole);
    }
    // Relative to the first corner: the triangulation works in single
    // steps of the coordinates it is given.
    let first = corners.first().copied().unwrap_or_default();
    let flat: Vec<f64> = corners
        .iter()
        .flat_map(|corner| [corner[0] - first[0], corner[1] - first[1]])
        .collect();
    let indices = earcutr::earcut(&flat, &hole_starts, 2)
        .map_err(|error| LoadError::InvalidData(format!("face triangulation failed: {error}")))?;
    let triangles = indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|triangle| {
            let [a, b, c] = triangle.map(|index| corners[index]);
            let twice = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if twice > 0.0 {
                Some(*triangle)
            } else if twice < 0.0 {
                Some([triangle[0], triangle[2], triangle[1]])
            } else {
                None
            }
        })
        .collect();
    Ok(Triangulated { corners, triangles })
}

/// Write into a temporary file beside the destination and move it into
/// place once it is complete and on disk.
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

/// Write the faces as Wavefront OBJ in scene coordinates: one group per
/// face, named `face_0001_wall` after its number and class, with the
/// outline and its openings as triangles and the normal of the face at
/// every corner; and one group per cylinder, named `face_0007_cylinder`,
/// with the scanned part of its surface. An existing file is replaced only
/// by a complete one.
pub fn write_faces_obj(
    surfaces: &DetectedSurfaces,
    destination: impl AsRef<Path>,
    comments: &[&str],
) -> Result<(), LoadError> {
    if surfaces.planes.is_empty() && surfaces.cylinders.is_empty() {
        return Err(LoadError::InvalidData(
            "there are no faces to export".into(),
        ));
    }
    if comments
        .iter()
        .any(|comment| comment.contains(['\n', '\r']))
    {
        return Err(LoadError::InvalidData("invalid OBJ comment".into()));
    }
    // Triangulated before anything is written, so that a face that cannot
    // be leaves the destination alone.
    let mut groups = Vec::with_capacity(surfaces.planes.len());
    for face in &surfaces.planes {
        let mut parts = Vec::with_capacity(face.patches.len());
        for patch in &face.patches {
            parts.push(triangulate(patch)?);
        }
        groups.push(parts);
    }
    write_atomically(destination.as_ref(), |writer| {
        writeln!(writer, "# Faces detected by Open Pointcloud Studio")?;
        for comment in comments {
            writeln!(writer, "# {comment}")?;
        }
        let mut written = 0usize;
        for (face, parts) in surfaces.planes.iter().zip(&groups) {
            writeln!(writer, "g face_{:04}_{}", face.id, face.class.name())?;
            let [nx, ny, nz] = face.normal;
            for Triangulated { corners, triangles } in parts {
                for corner in corners {
                    let [x, y, z] = face.point(*corner);
                    writeln!(writer, "v {x} {y} {z}")?;
                    writeln!(writer, "vn {nx} {ny} {nz}")?;
                }
                for triangle in triangles {
                    let [a, b, c] = triangle.map(|index| written + index + 1);
                    writeln!(writer, "f {a}//{a} {b}//{b} {c}//{c}")?;
                }
                written += corners.len();
            }
        }
        for face in &surfaces.cylinders {
            writeln!(writer, "g face_{:04}_cylinder", face.id)?;
            let MantleMesh { corners, triangles } = mantle(face);
            for ([x, y, z], [nx, ny, nz]) in &corners {
                writeln!(writer, "v {x} {y} {z}")?;
                writeln!(writer, "vn {nx} {ny} {nz}")?;
            }
            for triangle in triangles {
                let [a, b, c] = triangle.map(|index| written + index + 1);
                writeln!(writer, "f {a}//{a} {b}//{b} {c}//{c}")?;
            }
            written += corners.len();
        }
        Ok(())
    })
}

/// The faces as a JSON document, in scene coordinates and metres.
///
/// ```text
/// {
///   "format": "open-pointcloud-studio-faces",
///   "version": 1,
///   "source": "scan.e57",            file name of the anchor layer, no folder
///   "units": "metres",
///   "region": {"min": [x, y, z], "max": [x, y, z]} or null,
///                                    box round the points that took part
///   "settings": {
///     "distance_tolerance", "angle_tolerance_deg", "min_area",
///     "min_plane_width", "max_gap", "min_hole_area", "cylinders",
///     "voxel_size",                  as used: the size asked for or a doubling
///     "voxel_size_asked",
///     "boundary_cell",               as used
///     "boundary_cell_asked",
///     "coarse": false,               true when the voxels had to grow: narrow
///                                    faces and faces close together are lost
///     "density_doublings": 0         how often they were doubled because the
///                                    points lie far apart
///   },
///   "points": {"read", "source", "working", "assigned"},
///   "faces": [{
///     "id": 1,                       from 1, largest face first
///     "type": "plane",
///     "class": "floor" | "ceiling" | "wall" | "sloped",
///                                    by the direction of the normal alone: a
///                                    table top is a floor, a cabinet front a wall
///     "normal": [x, y, z],           unit, on the side the face was scanned from
///     "normal_from": "stations" | "nearest_station" | "open_side" | "centre",
///     "point": [x, y, z],            a point of the plane
///     "offset": d,                   the plane is normal . x = offset
///     "area", "covered_area", "coverage",
///     "coplanar_group": n,           equal for faces in one plane
///     "boundary": [{                 one entry per connected part
///       "outer": [[x, y, z], ...],   counter-clockwise seen from the normal's
///                                    side; closed, first corner not repeated
///       "holes": [[[x, y, z], ...]]  clockwise
///     }],
///     "residual": {"points", "inliers", "rms", "mean", "mean_abs", "p95", "max"}
///   }, {
///     "id": 7,                       the numbers go on after the planes
///     "type": "cylinder",
///     "axis_start": [x, y, z],       the axis, as far as the surface was scanned
///     "axis_end": [x, y, z],
///     "radius", "diameter", "length",
///     "arc_degrees": a,              how much of the round was scanned
///     "arc_start": [x, y, z],        unit direction from the axis to where that
///                                    arc begins
///     "arc_side": [x, y, z],         unit direction the arc runs towards
///     "seen_from_inside": false,     true for the inside of a round shaft
///     "area",                        of the scanned part
///     "residual": {...}              as for a plane; positive is outside
///   }],
///   "edges": [{"faces": [a, b], "start": [x, y, z], "end": [x, y, z],
///              "length", "angle_deg"}]
/// }
/// ```
///
/// The planes come first and then the cylinders, each largest first.
/// Residuals are distances of scan points to the plane of their face, over
/// the points within three distance tolerances of it whose surface runs
/// along the face, and those within one tolerance of what stands against
/// it: `points` of them, `inliers` within one tolerance; `mean` keeps the
/// sign (positive on the side of the normal), the others do not. `angle_deg` of an edge is the
/// angle between its two faces on the side their normals point to.
pub fn faces_json(surfaces: &DetectedSurfaces, source: &str) -> Value {
    let ring = |face: &PlaneFace, ring: &Vec<[f64; 2]>| -> Value {
        ring.iter()
            .map(|corner| json!(face.point(*corner)))
            .collect()
    };
    let faces: Vec<Value> = surfaces
        .planes
        .iter()
        .map(|face| {
            let boundary: Vec<Value> = face
                .patches
                .iter()
                .map(|patch| {
                    json!({
                        "outer": ring(face, &patch.outer),
                        "holes": patch.holes.iter().map(|hole| ring(face, hole)).collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({
                "id": face.id,
                "type": "plane",
                "class": face.class.name(),
                "normal": face.normal,
                "normal_from": face.normal_source.name(),
                "point": face.origin,
                "offset": face.offset(),
                "area": face.area,
                "covered_area": face.covered_area,
                "coverage": face.coverage(),
                "coplanar_group": face.coplanar_group,
                "boundary": boundary,
                "residual": {
                    "points": face.residuals.points,
                    "inliers": face.residuals.inliers,
                    "rms": face.residuals.rms,
                    "mean": face.residuals.mean,
                    "mean_abs": face.residuals.mean_abs,
                    "p95": face.residuals.p95,
                    "max": face.residuals.max,
                },
            })
        })
        .collect();
    let cylinders = surfaces.cylinders.iter().map(|face| {
        json!({
            "id": face.id,
            "type": "cylinder",
            "axis_start": face.axis_start,
            "axis_end": face.axis_end,
            "radius": face.radius,
            "diameter": face.diameter(),
            "length": face.length(),
            "arc_degrees": face.arc_deg,
            "arc_start": face.arc_start,
            "arc_side": face.arc_side,
            "seen_from_inside": face.seen_from_inside,
            "area": face.area(),
            "residual": {
                "points": face.residuals.points,
                "inliers": face.residuals.inliers,
                "rms": face.residuals.rms,
                "mean": face.residuals.mean,
                "mean_abs": face.residuals.mean_abs,
                "p95": face.residuals.p95,
                "max": face.residuals.max,
            },
        })
    });
    let faces: Vec<Value> = faces.into_iter().chain(cylinders).collect();
    let edges: Vec<Value> = surfaces
        .edges
        .iter()
        .map(|edge| {
            json!({
                "faces": edge.faces,
                "start": edge.start,
                "end": edge.end,
                "length": edge.length(),
                "angle_deg": edge.angle_deg,
            })
        })
        .collect();
    // Only the name: a path would tell where the scan is kept.
    let name = Path::new(source)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    json!({
        "format": FACES_JSON_FORMAT,
        "version": FACES_JSON_VERSION,
        "source": name,
        "units": "metres",
        "region": surfaces.region.map(|region| json!({"min": region.min, "max": region.max})),
        "settings": {
            "distance_tolerance": surfaces.config.distance_tolerance,
            "angle_tolerance_deg": surfaces.config.angle_tolerance_deg,
            "min_area": surfaces.config.min_region_area,
            "min_plane_width": surfaces.config.min_plane_width,
            "max_gap": surfaces.config.max_gap,
            "min_hole_area": surfaces.config.min_hole_area,
            "cylinders": surfaces.config.detect_cylinders,
            "voxel_size": surfaces.voxel_size,
            "voxel_size_asked": surfaces.config.voxel_size,
            "boundary_cell": surfaces.boundary_cell,
            "boundary_cell_asked": surfaces.config.boundary_cell,
            "coarse": surfaces.is_coarse(),
            "density_doublings": surfaces.density_doublings,
        },
        "points": {
            "read": surfaces.read_points,
            "source": surfaces.source_points,
            "working": surfaces.working_points,
            "assigned": surfaces.assigned_points,
        },
        "faces": faces,
        "edges": edges,
    })
}

/// Write `faces_json` to a file. `source` is the file name of the anchor
/// layer; a folder in it is left out. An existing file is replaced only by
/// a complete one.
pub fn write_faces_json(
    surfaces: &DetectedSurfaces,
    source: &str,
    destination: impl AsRef<Path>,
) -> Result<(), LoadError> {
    let document = faces_json(surfaces, source);
    write_atomically(destination.as_ref(), |writer| {
        serde_json::to_writer_pretty(&mut *writer, &document)
            .map_err(|error| LoadError::InvalidData(format!("cannot write the faces: {error}")))?;
        writeln!(writer)?;
        Ok(())
    })
}

/// Scene positions and normals as the source of a layer has them, so that
/// the layer's transform brings them back.
struct ToSource {
    placement: SourceTransform,
    /// A mirrored layer turns triangles round; the viewer turns the normal
    /// with them, so it is stored turned.
    mirrored: f64,
}

impl ToSource {
    fn new(placement: SourceTransform) -> Result<Self, LoadError> {
        if placement.source_xyz([0.0; 3]).is_none() {
            return Err(LoadError::InvalidData(
                "the layer of the faces has a zero scale".into(),
            ));
        }
        let negative = placement.scale.iter().filter(|value| **value < 0.0).count();
        Ok(Self {
            placement,
            mirrored: if negative.is_multiple_of(2) {
                1.0
            } else {
                -1.0
            },
        })
    }

    fn position(&self, scene: [f64; 3]) -> [f64; 3] {
        self.placement.source_xyz(scene).unwrap_or(scene)
    }

    fn normal(&self, scene: [f64; 3]) -> [f32; 3] {
        let scale = self.placement.scale;
        unit(std::array::from_fn(|axis| {
            scene[axis] * scale[axis] * self.mirrored
        }))
        .unwrap_or(scene)
        .map(|value| value as f32)
    }
}

fn limit_reached() -> LoadError {
    LoadError::InvalidData("the faces make a mesh that is too large to show".into())
}

/// The faces as a mesh for the viewer, each in its `face_color`: the
/// outlines with their openings, as triangles; and the scanned part of
/// every cylinder in its `cylinder_color`. Positions and normals are in
/// the source frame of the anchor layer. Without faces the mesh is empty.
pub fn flat_mesh(surfaces: &DetectedSurfaces) -> Result<MeshGeometry, LoadError> {
    let to_source = ToSource::new(surfaces.placement)?;
    let mut mesh = MeshGeometry::default();
    let mut colors = Vec::new();
    let mut normals = Vec::new();
    for face in &surfaces.planes {
        let color = face_color(face);
        let normal = to_source.normal(face.normal);
        for patch in &face.patches {
            let Triangulated { corners, triangles } = triangulate(patch)?;
            let base = mesh.vertices.len();
            if base + corners.len() > MAX_VERTICES
                || mesh.triangles.len() + triangles.len() > MAX_TRIANGLES
            {
                return Err(limit_reached());
            }
            for corner in corners {
                mesh.vertices.push(to_source.position(face.point(corner)));
                colors.push(color);
                normals.push(normal);
            }
            mesh.triangles.extend(
                triangles
                    .into_iter()
                    .map(|triangle| triangle.map(|index| (base + index) as u32)),
            );
        }
    }
    for face in &surfaces.cylinders {
        let color = cylinder_color(face);
        let MantleMesh { corners, triangles } = mantle(face);
        let base = mesh.vertices.len();
        if base + corners.len() > MAX_VERTICES
            || mesh.triangles.len() + triangles.len() > MAX_TRIANGLES
        {
            return Err(limit_reached());
        }
        for (position, normal) in corners {
            mesh.vertices.push(to_source.position(position));
            colors.push(color);
            normals.push(to_source.normal(normal));
        }
        mesh.triangles.extend(
            triangles
                .into_iter()
                .map(|triangle| triangle.map(|index| (base + index) as u32)),
        );
    }
    mesh.colors = Some(colors);
    mesh.normals = Some(normals);
    Ok(mesh)
}

/// The nearest point on a closed ring.
fn nearest_on_ring(ring: &[[f64; 2]], uv: [f64; 2]) -> ([f64; 2], f64) {
    let mut best = (uv, f64::INFINITY);
    for index in 0..ring.len() {
        let (a, b) = (ring[index], ring[(index + 1) % ring.len()]);
        let edge = [b[0] - a[0], b[1] - a[1]];
        let length = edge[0] * edge[0] + edge[1] * edge[1];
        let along = if length > 0.0 {
            (((uv[0] - a[0]) * edge[0] + (uv[1] - a[1]) * edge[1]) / length).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let point = [a[0] + along * edge[0], a[1] + along * edge[1]];
        let apart = (point[0] - uv[0]).hypot(point[1] - uv[1]);
        if apart < best.1 {
            best = (point, apart);
        }
    }
    best
}

/// A grid with `merge` by `merge` cells taken together: per merged cell its
/// points and the sum of their deviations.
struct Merged {
    width: u32,
    height: u32,
    cells: Vec<(u64, f64)>,
}

impl Merged {
    fn new(width: u32, height: u32, counts: &[u32], means: &[f32], merge: u32) -> Self {
        let merged = [width.div_ceil(merge), height.div_ceil(merge)];
        let mut cells = vec![(0u64, 0.0f64); merged[0] as usize * merged[1] as usize];
        for y in 0..height {
            for x in 0..width {
                let index = y as usize * width as usize + x as usize;
                let count = u64::from(counts[index]);
                if count > 0 {
                    let cell = &mut cells
                        [(y / merge) as usize * merged[0] as usize + (x / merge) as usize];
                    cell.0 += count;
                    cell.1 += f64::from(means[index]) * count as f64;
                }
            }
        }
        Self {
            width: merged[0],
            height: merged[1],
            cells,
        }
    }

    /// The mean deviation of a cell that holds points.
    fn mean(&self, x: i64, y: i64) -> Option<f64> {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return None;
        }
        let (count, sum) = self.cells[y as usize * self.width as usize + x as usize];
        (count > 0).then(|| sum / count as f64)
    }

    fn filled(&self) -> usize {
        self.cells.iter().filter(|cell| cell.0 > 0).count()
    }
}

/// A mesh under construction with what goes with every corner.
struct Colored {
    mesh: MeshGeometry,
    colors: Vec<[u8; 3]>,
    normals: Vec<[f32; 3]>,
}

impl Colored {
    /// Two triangles for every cell of a grid that holds points, with at
    /// every corner the colour for the mean deviation of the cells round it.
    /// `front` is 1 when the means of the grid are positive on the side the
    /// face was scanned from and -1 when they are positive on the other
    /// side. `corner` gives the scene position and normal of a corner of the
    /// merged grid.
    fn add_cells(
        &mut self,
        to_source: &ToSource,
        grid: &Merged,
        scale: f64,
        front: f64,
        corner: &dyn Fn(u32, u32) -> ([f64; 3], [f64; 3]),
    ) -> Result<(), LoadError> {
        let stride = grid.width as usize + 1;
        let mut vertex_of = vec![u32::MAX; stride * (grid.height as usize + 1)];
        let base = self.mesh.vertices.len();
        // Scene position and normal of the corners added here, to turn the
        // triangles by.
        let mut scene: Vec<([f64; 3], [f64; 3])> = Vec::new();
        for y in 0..grid.height {
            for x in 0..grid.width {
                if grid.mean(i64::from(x), i64::from(y)).is_none() {
                    continue;
                }
                let mut quad = [0usize; 4];
                for (slot, (cx, cy)) in [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)]
                    .into_iter()
                    .enumerate()
                {
                    let place = cy as usize * stride + cx as usize;
                    if vertex_of[place] == u32::MAX {
                        if self.mesh.vertices.len() >= MAX_VERTICES {
                            return Err(limit_reached());
                        }
                        // The mean of the cells that meet in this corner.
                        let (mut sum, mut known) = (0.0, 0.0);
                        for (dx, dy) in [(-1, -1), (0, -1), (-1, 0), (0, 0)] {
                            if let Some(value) = grid.mean(i64::from(cx) + dx, i64::from(cy) + dy) {
                                sum += value;
                                known += 1.0;
                            }
                        }
                        let (position, normal) = corner(cx, cy);
                        vertex_of[place] = self.mesh.vertices.len() as u32;
                        self.mesh.vertices.push(to_source.position(position));
                        self.colors
                            .push(deviation_color(front * sum / known, scale));
                        self.normals.push(to_source.normal(normal));
                        scene.push((position, normal));
                    }
                    quad[slot] = vertex_of[place] as usize - base;
                }
                if self.mesh.triangles.len() + 2 > MAX_TRIANGLES {
                    return Err(limit_reached());
                }
                for triangle in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                    self.mesh
                        .triangles
                        .push(turned(triangle, &scene).map(|index| (base + index) as u32));
                }
            }
        }
        Ok(())
    }
}

/// The faces as a mesh for the viewer, coloured by the distance of the scan
/// to each face or cylinder: two triangles per grid cell that holds points,
/// with at every corner the colour for the mean deviation of the cells
/// round it. `scale` is the deviation that gets the full colour (see
/// `deviation_legend`); the distance tolerance of the detection is the
/// natural choice. Above `max_cells` cells, cells are merged two by two.
/// Positions and normals are in the source frame of the anchor layer.
/// Without faces the mesh is empty.
pub fn deviation_mesh(
    surfaces: &DetectedSurfaces,
    scale: f64,
    max_cells: usize,
) -> Result<MeshGeometry, LoadError> {
    let to_source = ToSource::new(surfaces.placement)?;
    // Corners per cell tend to one; a quarter of the vertex limit leaves
    // room for the rims of many small faces.
    let max_cells = max_cells.clamp(1, MAX_VERTICES / 4);
    let grids = |merge: u32| {
        let planes: Vec<Merged> = surfaces
            .planes
            .iter()
            .map(|face| {
                let grid = &face.deviation;
                Merged::new(grid.width, grid.height, &grid.counts, &grid.means, merge)
            })
            .collect();
        let cylinders: Vec<Merged> = surfaces
            .cylinders
            .iter()
            .map(|face| {
                let grid = &face.deviation;
                Merged::new(grid.columns, grid.rows, &grid.counts, &grid.means, merge)
            })
            .collect();
        (planes, cylinders)
    };
    let mut merge = 1u32;
    let (planes, cylinders) = loop {
        let (planes, cylinders) = grids(merge);
        let cells: usize = planes.iter().chain(&cylinders).map(Merged::filled).sum();
        if cells <= max_cells {
            break (planes, cylinders);
        }
        merge = merge.checked_mul(2).ok_or_else(limit_reached)?;
    };
    let mut colored = Colored {
        mesh: MeshGeometry::default(),
        colors: Vec::new(),
        normals: Vec::new(),
    };
    for (face, merged) in surfaces.planes.iter().zip(&planes) {
        let grid = &face.deviation;
        // The normal of a flat face points to the side it was scanned from,
        // and its means are positive on that side.
        colored.add_cells(&to_source, merged, scale, 1.0, &|x, y| {
            let position = grid.corner((x * merge).min(grid.width), (y * merge).min(grid.height));
            (within_outline(face, position), face.normal)
        })?;
    }
    for (face, merged) in surfaces.cylinders.iter().zip(&cylinders) {
        let grid = &face.deviation;
        let length = face.length();
        // The means of a cylinder are positive outside it, whatever side it
        // was scanned from. The front of a face is the side it was scanned
        // from, so a shaft seen from inside turns its colours with its
        // normals.
        let facing = if face.seen_from_inside { -1.0 } else { 1.0 };
        colored.add_cells(&to_source, merged, scale, facing, &|x, y| {
            let angle = grid.first_angle
                + f64::from((x * merge).min(grid.columns)) * 360.0 / f64::from(grid.columns);
            let angle = angle.clamp(0.0, face.arc_deg);
            let along = grid.first_along + f64::from((y * merge).min(grid.rows)) * grid.step;
            (
                face.point(angle, along.clamp(0.0, length)),
                face.outward(angle).map(|value| value * facing),
            )
        })?;
    }
    colored.mesh.colors = Some(colored.colors);
    colored.mesh.normals = Some(colored.normals);
    Ok(colored.mesh)
}

/// A grid corner, moved onto the outline of its face when it lies outside
/// it or in one of its openings: the cells along an edge reach up to a cell
/// past it, and would stick through the face on the other side.
fn within_outline(face: &PlaneFace, position: [f64; 3]) -> [f64; 3] {
    let from = difference(position, face.origin);
    let uv = [dot(from, face.u), dot(from, face.v)];
    for patch in &face.patches {
        if !ring_contains(&patch.outer, uv) {
            continue;
        }
        return match patch.holes.iter().find(|hole| ring_contains(hole, uv)) {
            Some(hole) => face.point(nearest_on_ring(hole, uv).0),
            None => position,
        };
    }
    face.patches
        .iter()
        .map(|patch| nearest_on_ring(&patch.outer, uv))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map_or(position, |(nearest, _)| face.point(nearest))
}
