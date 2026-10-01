//! Bounded-memory 2.5D terrain reconstruction from a full point stream.
//! One lowest point per XY cell becomes a Delaunay TIN vertex. Long edges
//! are removed so disconnected survey areas are not bridged by large faces.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use delaunator::{triangulate, Point as PlanarPoint};

use super::{visit_points, LoadError, Point, PointCloud};

#[derive(Clone, Copy, Debug)]
pub struct MeshConfig {
    pub max_vertices: usize,
    pub max_edge_cells: f64,
}

impl Default for MeshConfig {
    fn default() -> Self {
        Self {
            max_vertices: 100_000,
            max_edge_cells: 6.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshStats {
    pub source_points: u64,
    pub vertices: usize,
    pub triangles: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshStage {
    Reading,
    Reconstructing,
    Writing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MeshProgress {
    pub stage: MeshStage,
    pub completed: u64,
    pub total: u64,
}

impl MeshProgress {
    pub fn new(stage: MeshStage, completed: u64, total: u64) -> Self {
        Self {
            stage,
            completed,
            total,
        }
    }
}

/// Stream the complete source, reconstruct a terrain TIN, and atomically save
/// Wavefront OBJ. Vertical walls and overhangs require a 3D reconstruction mode.
pub fn mesh_terrain_obj(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    config: MeshConfig,
) -> Result<MeshStats, LoadError> {
    mesh_terrain_obj_where(cloud, destination, config, |_, _| true)
}

/// Terrain reconstruction over a filtered source stream.
pub fn mesh_terrain_obj_where(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    config: MeshConfig,
    include: impl FnMut(u64, &Point) -> bool,
) -> Result<MeshStats, LoadError> {
    mesh_terrain_obj_where_progress(cloud, destination, config, include, |_| Ok(()))
}

/// Terrain reconstruction with bounded progress callbacks. Returning an error
/// from the callback stops work before the destination is replaced.
pub fn mesh_terrain_obj_where_progress(
    cloud: &PointCloud,
    destination: impl AsRef<Path>,
    config: MeshConfig,
    mut include: impl FnMut(u64, &Point) -> bool,
    mut progress: impl FnMut(MeshProgress) -> Result<(), LoadError>,
) -> Result<MeshStats, LoadError> {
    mesh_terrain_obj_inner(
        cloud,
        destination.as_ref(),
        config,
        &mut include,
        &mut progress,
    )
}

// Run the point-heavy loop in pointcloud-core's optimized dev profile rather
// than monomorphizing it in the unoptimized native desktop caller.
fn mesh_terrain_obj_inner(
    cloud: &PointCloud,
    destination: &Path,
    config: MeshConfig,
    include: &mut dyn FnMut(u64, &Point) -> bool,
    progress: &mut dyn FnMut(MeshProgress) -> Result<(), LoadError>,
) -> Result<MeshStats, LoadError> {
    if destination == cloud.path
        || fs::canonicalize(destination).ok() == fs::canonicalize(&cloud.path).ok()
    {
        return Err(LoadError::InvalidData(
            "source and destination must differ".into(),
        ));
    }
    if config.max_vertices < 4 || !config.max_edge_cells.is_finite() || config.max_edge_cells <= 0.0
    {
        return Err(LoadError::InvalidData("invalid mesh settings".into()));
    }
    let x_span = cloud.bounds.max[0] - cloud.bounds.min[0];
    let y_span = cloud.bounds.max[1] - cloud.bounds.min[1];
    if !x_span.is_finite() || !y_span.is_finite() || x_span <= 0.0 || y_span <= 0.0 {
        return Err(LoadError::InvalidData(
            "terrain meshing requires a nonzero XY area".into(),
        ));
    }
    cloud.validate_source()?;
    let cells_per_side = (config.max_vertices as f64).sqrt().floor().max(2.0) as u32;
    let cell = x_span.max(y_span) / cells_per_side as f64;
    let mut cells: BTreeMap<(u32, u32), Point> = BTreeMap::new();
    let mut visited = 0u64;
    let mut source_points = 0u64;
    progress(MeshProgress::new(MeshStage::Reading, 0, cloud.total_points))?;
    visit_points(&cloud.path, &mut |point| {
        let ordinal = visited;
        visited += 1;
        if visited.is_multiple_of(65_536) {
            progress(MeshProgress::new(
                MeshStage::Reading,
                visited,
                cloud.total_points,
            ))?;
        }
        if !include(ordinal, &point) {
            return Ok(());
        }
        source_points += 1;
        if !point.xyz.iter().all(|value| value.is_finite()) {
            return Err(LoadError::InvalidData("non-finite mesh vertex".into()));
        }
        let x =
            (((point.xyz[0] - cloud.bounds.min[0]) / cell).floor() as u32).min(cells_per_side - 1);
        let y =
            (((point.xyz[1] - cloud.bounds.min[1]) / cell).floor() as u32).min(cells_per_side - 1);
        cells
            .entry((x, y))
            .and_modify(|existing| {
                if point.xyz[2] < existing.xyz[2] {
                    *existing = point;
                }
            })
            .or_insert(point);
        Ok(())
    })?;
    if visited != cloud.total_points {
        return Err(LoadError::InvalidData(format!(
            "source changed during meshing (expected {}, found {visited})",
            cloud.total_points
        )));
    }
    cloud.validate_source()?;
    progress(MeshProgress::new(
        MeshStage::Reading,
        visited,
        cloud.total_points,
    ))?;
    progress(MeshProgress::new(MeshStage::Reconstructing, 0, 1))?;
    let vertices: Vec<Point> = cells.into_values().collect();
    if vertices.len() < 3 {
        return Err(LoadError::InvalidData(
            "too few distinct XY cells for a mesh".into(),
        ));
    }
    let planar: Vec<PlanarPoint> = vertices
        .iter()
        .map(|point| PlanarPoint {
            x: point.xyz[0],
            y: point.xyz[1],
        })
        .collect();
    let topology = triangulate(&planar);
    let mut nearest = vec![f64::INFINITY; planar.len()];
    for triangle in topology.triangles.as_chunks::<3>().0 {
        for (left, right) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let distance =
                (planar[left].x - planar[right].x).hypot(planar[left].y - planar[right].y);
            nearest[left] = nearest[left].min(distance);
            nearest[right] = nearest[right].min(distance);
        }
    }
    nearest.retain(|distance| distance.is_finite());
    nearest.sort_by(f64::total_cmp);
    let typical_spacing = nearest.get(nearest.len() / 2).copied().unwrap_or(cell);
    let edge_limit_squared = (cell.max(typical_spacing) * config.max_edge_cells).powi(2);
    let faces: Vec<[usize; 3]> = topology
        .triangles
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|triangle| {
            let [a, b, c] = [triangle[0], triangle[1], triangle[2]];
            let short = [(a, b), (b, c), (c, a)].into_iter().all(|(left, right)| {
                let dx = planar[left].x - planar[right].x;
                let dy = planar[left].y - planar[right].y;
                dx * dx + dy * dy <= edge_limit_squared
            });
            let winding = (planar[b].x - planar[a].x) * (planar[c].y - planar[a].y)
                - (planar[b].y - planar[a].y) * (planar[c].x - planar[a].x);
            short.then_some(if winding >= 0.0 {
                [a + 1, b + 1, c + 1]
            } else {
                [a + 1, c + 1, b + 1]
            })
        })
        .collect();
    if faces.is_empty() {
        return Err(LoadError::InvalidData(
            "no terrain faces within the allowed edge length".into(),
        ));
    }
    let mut normals = vec![[0.0_f64; 3]; vertices.len()];
    for [a, b, c] in &faces {
        let [pa, pb, pc] = [
            vertices[a - 1].xyz,
            vertices[b - 1].xyz,
            vertices[c - 1].xyz,
        ];
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let normal = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for index in [*a, *b, *c] {
            for axis in 0..3 {
                normals[index - 1][axis] += normal[axis];
            }
        }
    }
    for normal in &mut normals {
        let length = normal.iter().map(|value| value * value).sum::<f64>().sqrt();
        if length > f64::EPSILON {
            for value in normal {
                *value /= length;
            }
        } else {
            *normal = [0.0, 0.0, 1.0];
        }
    }
    progress(MeshProgress::new(MeshStage::Reconstructing, 1, 1))?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let write_total = (vertices.len() * 2 + faces.len()) as u64;
    progress(MeshProgress::new(MeshStage::Writing, 0, write_total))?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writeln!(writer, "# Open Pointcloud Studio terrain mesh")?;
        writeln!(writer, "o Terrain")?;
        let has_color = vertices.iter().any(|point| point.rgb.is_some());
        for (index, point) in vertices.iter().enumerate() {
            if has_color {
                let rgb = point.rgb.unwrap_or([255; 3]);
                writeln!(
                    writer,
                    "v {:.9} {:.9} {:.9} {:.6} {:.6} {:.6}",
                    point.xyz[0],
                    point.xyz[1],
                    point.xyz[2],
                    f64::from(rgb[0]) / 255.0,
                    f64::from(rgb[1]) / 255.0,
                    f64::from(rgb[2]) / 255.0,
                )?;
            } else {
                writeln!(
                    writer,
                    "v {:.9} {:.9} {:.9}",
                    point.xyz[0], point.xyz[1], point.xyz[2]
                )?;
            }
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (index + 1) as u64,
                    write_total,
                ))?;
            }
        }
        for (index, normal) in normals.iter().enumerate() {
            writeln!(
                writer,
                "vn {:.8} {:.8} {:.8}",
                normal[0], normal[1], normal[2]
            )?;
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (vertices.len() + index + 1) as u64,
                    write_total,
                ))?;
            }
        }
        for (index, [a, b, c]) in faces.iter().enumerate() {
            writeln!(writer, "f {a}//{a} {b}//{b} {c}//{c}")?;
            if (index + 1).is_multiple_of(4_096) {
                progress(MeshProgress::new(
                    MeshStage::Writing,
                    (vertices.len() * 2 + index + 1) as u64,
                    write_total,
                ))?;
            }
        }
        writer.flush()?;
    }
    progress(MeshProgress::new(
        MeshStage::Writing,
        write_total,
        write_total,
    ))?;
    cloud.validate_source()?;
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(MeshStats {
        source_points,
        vertices: vertices.len(),
        triangles: faces.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read_obj_mesh;

    #[test]
    fn terrain_mesh_preserves_rgb_and_writes_normals() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("colored-terrain.xyz");
        let destination = directory.path().join("colored-terrain.obj");
        let mut data = String::new();
        for y in 0..5 {
            for x in 0..5 {
                data.push_str(&format!("{x} {y} 0 {} {} 128\n", x * 40, y * 40));
            }
        }
        fs::write(&source, data).unwrap();
        let cloud = super::super::open(&source, 1).unwrap();
        mesh_terrain_obj(&cloud, &destination, MeshConfig::default()).unwrap();
        let mesh = read_obj_mesh(destination).unwrap();
        assert!(!mesh.triangles.is_empty());
        assert_eq!(mesh.colors.as_ref().unwrap().len(), mesh.vertices.len());
        assert_eq!(mesh.normals.as_ref().unwrap().len(), mesh.vertices.len());
        assert!(mesh.normals.unwrap().iter().all(|normal| normal[2] > 0.99));
    }

    #[test]
    fn meshes_complete_stream_and_keeps_lowest_point_per_cell() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("terrain.xyz");
        let destination = directory.path().join("terrain.obj");
        fs::write(&source, "0 0 100\n0 0 2\n1 0 1\n0 1 3\n1 1 4\n").unwrap();
        let cloud = super::super::open(&source, 1).unwrap();
        let stats = mesh_terrain_obj(
            &cloud,
            &destination,
            MeshConfig {
                max_vertices: 16,
                max_edge_cells: 6.0,
            },
        )
        .unwrap();
        let obj = fs::read_to_string(destination).unwrap();
        assert_eq!(stats.source_points, 5);
        assert_eq!(stats.vertices, 4);
        assert_eq!(stats.triangles, 2);
        assert!(obj.contains("v 0.000000000 0.000000000 2.000000000"));
        assert!(!obj.contains("100.000000000"));
        let edited = directory.path().join("terrain-edited.obj");
        let stats = mesh_terrain_obj_where(
            &cloud,
            &edited,
            MeshConfig {
                max_vertices: 16,
                max_edge_cells: 6.0,
            },
            |ordinal, _| ordinal != 4,
        )
        .unwrap();
        assert_eq!(stats.source_points, 4);
        assert_eq!(stats.vertices, 3);
        assert!(!fs::read_to_string(edited).unwrap().contains("4.000000000"));
    }

    #[test]
    fn does_not_bridge_distant_scan_patches() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("patches.xyz");
        let destination = directory.path().join("patches.obj");
        fs::write(&source, "0 0 0\n0 2 0\n2 0 0\n100 0 0\n100 2 0\n102 0 0\n").unwrap();
        let cloud = super::super::open(&source, 1).unwrap();
        let stats = mesh_terrain_obj(
            &cloud,
            &destination,
            MeshConfig {
                max_vertices: 10_000,
                max_edge_cells: 4.0,
            },
        )
        .unwrap();
        assert_eq!(stats.vertices, 6);
        assert_eq!(stats.triangles, 2);
    }

    #[test]
    fn default_settings_mesh_sparse_regular_grid() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("grid.xyz");
        let destination = directory.path().join("grid.obj");
        let mut points = String::new();
        for x in 0..30 {
            for y in 0..30 {
                points.push_str(&format!("{x} {y} 0\n"));
            }
        }
        fs::write(&source, points).unwrap();
        let cloud = super::super::open(&source, 900).unwrap();
        let stats = mesh_terrain_obj(&cloud, &destination, MeshConfig::default()).unwrap();
        assert_eq!(stats.source_points, 900);
        assert!(stats.triangles >= 1_000);
    }

    #[test]
    fn cancellation_during_write_preserves_existing_mesh() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("terrain.xyz");
        let destination = directory.path().join("terrain.obj");
        fs::write(&source, "0 0 0\n1 0 0\n0 1 0\n1 1 0\n").unwrap();
        fs::write(&destination, "previous mesh").unwrap();
        let cloud = super::super::open(&source, 1).unwrap();
        let mut saw_read_complete = false;
        let result = mesh_terrain_obj_where_progress(
            &cloud,
            &destination,
            MeshConfig {
                max_vertices: 4,
                max_edge_cells: 6.0,
            },
            |_, _| true,
            |state| {
                if state.stage == MeshStage::Reading && state.completed == state.total {
                    saw_read_complete = true;
                }
                if state.stage == MeshStage::Writing {
                    return Err(LoadError::Cancelled);
                }
                Ok(())
            },
        );
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert!(saw_read_complete);
        assert_eq!(fs::read(&destination).unwrap(), b"previous mesh");
    }
}
