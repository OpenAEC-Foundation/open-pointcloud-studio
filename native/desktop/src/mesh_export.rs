//! Saving the mesh a layer holds as OBJ, PLY or STL: the save dialog behind
//! the File view entry and the Properties button, the `export_mesh` command
//! of the local API and the `--mesh-export` mode of the command line. Also
//! what is measured of a mesh when it is made or read, and how Properties and
//! the API show that.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::widget::{button, column, container};
use iced::{Element, Fill, Task};
use pointcloud_core::{LoadError, MeshFormat, MeshGeometry, MeshTopology};
use serde_json::{json, Value};

use crate::cloud_transform::CloudTransform;
use crate::i18n::tr;
use crate::{
    camera_views, flat_tool_style, format_count, is_bag3d_mesh, opencad_properties, CloudEntry,
    Message, Studio, BAG3D_MESH_COMMENTS, BAG3D_PLY_COMMENTS,
};

/// The formats in the order the save dialog offers them, each with the name
/// of its filter. The first one is what a file name without an extension
/// gets where the system adds one.
const FORMATS: [(MeshFormat, &str); 3] = [
    (MeshFormat::Obj, "OBJ mesh"),
    (MeshFormat::Ply, "PLY mesh (binary)"),
    (MeshFormat::Stl, "STL (binary)"),
];

const NO_FORMAT: &str = "Choose an .obj, .ply or .stl file name for the mesh";
const SAME_FILE: &str = "Choose a mesh file different from the source file";

/// A mesh with what was measured of it when it was made or read.
#[derive(Debug, Clone)]
pub(crate) struct MeasuredMesh {
    pub(crate) mesh: Arc<MeshGeometry>,
    pub(crate) topology: MeshTopology,
}

impl MeasuredMesh {
    /// Count the open edges and the connected parts of a mesh that one of
    /// the mesh jobs made: its triangles share a vertex wherever they meet.
    /// It sorts every edge of the mesh, so it belongs on the worker thread
    /// that made it.
    pub(crate) fn measure(mesh: MeshGeometry) -> Self {
        let topology = pointcloud_core::mesh_topology(&mesh);
        Self {
            mesh: Arc::new(mesh),
            topology,
        }
    }

    /// The same for a mesh that was read from a file. A file can hold one
    /// vertex per face corner, and the OBJ reader gives a vertex a copy for
    /// every material colour that meets in it. Counted by index those seams
    /// would be rims of a surface that is closed, so vertices at the same
    /// position count as one here.
    fn measure_read(mesh: MeshGeometry) -> Self {
        let topology = pointcloud_core::mesh_topology_by_position(&mesh);
        Self {
            mesh: Arc::new(mesh),
            topology,
        }
    }
}

/// Read the faces of a mesh file and measure them, on the worker thread of
/// the layer that shows the file. `None` when the file holds points only.
pub(crate) fn read_measured(path: &Path) -> Result<Option<MeasuredMesh>, String> {
    pointcloud_core::read_mesh_geometry(path)
        .map(|mesh| mesh.map(MeasuredMesh::measure_read))
        .map_err(|error| error.to_string())
}

/// What a save needs of the layer it was asked for. The layer can move,
/// get another mesh or close while the dialog is open or the file is written.
#[derive(Debug, Clone)]
pub(crate) struct MeshExportRequest {
    mesh: Arc<MeshGeometry>,
    source: PathBuf,
    bag_source: bool,
    transform: CloudTransform,
}

/// A mesh file that was written.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MeshExportDone {
    path: PathBuf,
    format: MeshFormat,
    vertices: usize,
    triangles: usize,
    /// The point the coordinates of an STL file are relative to, when the
    /// mesh lies too far from zero for the 32-bit floats of that format.
    origin: Option<[f64; 3]>,
}

impl MeshExportDone {
    /// The line that tells where the mesh went, and for an STL file with an
    /// origin that another program shows it near zero.
    fn summary(&self) -> String {
        let mut text = format!(
            "Exported mesh as {}: {} vertices and {} triangles to {}",
            self.format.label(),
            self.vertices,
            self.triangles,
            self.path.display()
        );
        if let Some(origin) = self.origin {
            text.push_str(&format!("; {}", origin_note(origin)));
        }
        text
    }

    /// The finished job as the local API reports it.
    fn job_value(&self) -> Value {
        json!({
            "state": "complete",
            "operation": "export_mesh",
            "path": self.path,
            "format": self.format.extension(),
            "vertices": self.vertices,
            "triangles": self.triangles,
            "origin": self.origin,
        })
    }
}

fn origin_note([x, y, z]: [f64; 3]) -> String {
    format!("STL coordinates are relative to the origin {x} {y} {z} m named in the file header")
}

/// The open edges and connected parts of a mesh, for a status line.
pub(crate) fn topology_text(topology: MeshTopology) -> String {
    let counted = |count: u64, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    format!(
        "{}, {}",
        counted(topology.open_edges, "open edge", "open edges"),
        counted(
            u64::from(topology.components),
            "connected part",
            "connected parts"
        )
    )
}

/// The mesh of a layer as `status` of the local API lists it: null without
/// one, and the measured values null for a mesh that was not measured.
pub(crate) fn mesh_value(entry: &CloudEntry) -> Value {
    let Some(mesh) = &entry.mesh else {
        return Value::Null;
    };
    json!({
        "vertices": mesh.vertices.len(),
        "triangles": mesh.triangles.len(),
        "open_edges": entry.mesh_topology.map(|topology| topology.open_edges),
        "components": entry.mesh_topology.map(|topology| topology.components),
    })
}

/// The mesh where its layer stands in the scene, for a file. The layer keeps
/// its mesh in source coordinates, so a move or scale of the layer is applied
/// here.
///
/// A scale with an odd number of negative factors mirrors the mesh. A file
/// tells the outside of a surface by the order of the corners of a triangle,
/// and STL has nothing else, so that order is reversed for a mirrored mesh
/// and the outside stays where it was. The scene draws both sides and keeps
/// the order.
pub(crate) fn in_scene(mesh: &MeshGeometry, transform: CloudTransform) -> MeshGeometry {
    let mirrored = transform
        .scale
        .iter()
        .filter(|factor| **factor < 0.0)
        .count()
        % 2
        == 1;
    MeshGeometry {
        vertices: mesh
            .vertices
            .iter()
            .map(|xyz| transform.xyz(*xyz))
            .collect(),
        triangles: if mirrored {
            mesh.triangles
                .iter()
                .map(|[a, b, c]| [*a, *c, *b])
                .collect()
        } else {
            mesh.triangles.clone()
        },
        colors: mesh.colors.clone(),
        normals: mesh
            .normals
            .as_deref()
            .and_then(|normals| crate::transformed_mesh_normals(normals, transform.scale))
            .map(|mut normals| {
                if mirrored {
                    // `CloudTransform::normal` turns a normal to the side
                    // the unchanged order faces in the scene. With the order
                    // reversed here, it turns back to the outside.
                    for normal in &mut normals {
                        *normal = normal.map(|value| -value);
                    }
                }
                normals
            }),
    }
}

/// The format a destination asks for through its extension, or why nothing
/// is written there.
fn destination_format(source: &Path, destination: &Path) -> Result<MeshFormat, &'static str> {
    let format = MeshFormat::from_path(destination).ok_or(NO_FORMAT)?;
    if camera_views::source_key(source) == camera_views::source_key(destination) {
        return Err(SAME_FILE);
    }
    Ok(format)
}

/// Write the mesh as the scene shows it. The core writes a temporary file
/// first, so a failure leaves an existing destination as it was.
fn write(
    request: &MeshExportRequest,
    path: PathBuf,
    format: MeshFormat,
) -> Result<MeshExportDone, LoadError> {
    let comments: &[&str] = match (request.bag_source, format) {
        (false, _) => &[],
        (true, MeshFormat::Ply) => BAG3D_PLY_COMMENTS,
        (true, _) => BAG3D_MESH_COMMENTS,
    };
    let moved;
    let mesh = if request.transform.is_identity() {
        &*request.mesh
    } else {
        moved = in_scene(&request.mesh, request.transform);
        &moved
    };
    let report = pointcloud_core::write_mesh(mesh, &path, format, comments)?;
    Ok(MeshExportDone {
        path,
        format,
        vertices: mesh.vertices.len(),
        triangles: mesh.triangles.len(),
        origin: report.origin,
    })
}

/// The `--mesh-export` mode: write the faces of a mesh file in the format
/// the extension of the destination names. Returns the lines to print, or
/// the exit code with the line that says what is wrong.
pub(crate) fn convert_file(source: &Path, destination: &Path) -> Result<String, (i32, String)> {
    let format = destination_format(source, destination).map_err(|problem| {
        let line = if problem == NO_FORMAT {
            "Supported mesh export extensions: .obj, .ply, .stl"
        } else {
            "Choose an output path different from the input"
        };
        (2, line.to_owned())
    })?;
    let failed = |error: LoadError| (1, format!("Mesh export failed: {error}"));
    let mesh = pointcloud_core::read_mesh_geometry(source)
        .and_then(|mesh| {
            mesh.ok_or_else(|| LoadError::InvalidData("source contains no mesh faces".into()))
        })
        .map_err(failed)?;
    let request = MeshExportRequest {
        mesh: Arc::new(mesh),
        source: source.to_path_buf(),
        bag_source: is_bag3d_mesh(source),
        transform: CloudTransform::default(),
    };
    let done = write(&request, destination.to_path_buf(), format).map_err(failed)?;
    let mut lines = format!(
        "Mesh exported: {} vertices, {} triangles -> {}",
        done.vertices,
        done.triangles,
        done.path.display()
    );
    if let Some(origin) = done.origin {
        lines.push('\n');
        lines.push_str(&origin_note(origin));
    }
    Ok(lines)
}

impl Studio {
    /// The mesh of the active layer with what a save needs of that layer.
    fn mesh_export_request(&self) -> Option<MeshExportRequest> {
        let entry = self.active.and_then(|index| self.clouds.get(index))?;
        Some(MeshExportRequest {
            mesh: Arc::clone(entry.mesh.as_ref()?),
            source: entry.cloud.path.clone(),
            bag_source: entry.bag_source,
            transform: entry.transform,
        })
    }

    /// Ask where to save the mesh of the active layer. The dialog offers the
    /// three formats, and the extension of the chosen name decides.
    pub(crate) fn export_mesh(&mut self) -> Task<Message> {
        if self.mesh_export_pending {
            return Task::none();
        }
        let Some(request) = self.mesh_export_request() else {
            self.status = "Select a cloud with a surface mesh first".into();
            return Task::none();
        };
        let stem = request
            .source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("surface");
        let suggestion = format!("{stem}-mesh.{}", FORMATS[0].0.extension());
        self.mesh_export_pending = true;
        self.status = "Choose where to save the mesh as OBJ, PLY or STL…".into();
        Task::perform(
            async move {
                FORMATS
                    .iter()
                    .fold(rfd::AsyncFileDialog::new(), |dialog, (format, name)| {
                        dialog.add_filter(*name, &[format.extension()])
                    })
                    .set_file_name(suggestion)
                    .save_file()
                    .await
                    .map(|selection| selection.path().to_path_buf())
            },
            move |path| Message::MeshExportPathChosen(request.clone(), path),
        )
    }

    /// The save dialog closed: write the file, or say why not.
    pub(crate) fn mesh_export_path_chosen(
        &mut self,
        request: MeshExportRequest,
        path: Option<PathBuf>,
    ) -> Task<Message> {
        let Some(path) = path else {
            self.mesh_export_pending = false;
            self.status = "Mesh export cancelled".into();
            return Task::none();
        };
        match destination_format(&request.source, &path) {
            Ok(format) => self.start_mesh_export(request, path, format, None),
            Err(problem) => {
                self.mesh_export_pending = false;
                self.status = problem.into();
                Task::none()
            }
        }
    }

    /// Write the file on a worker thread. `status` of the local API reports
    /// `mesh_export_pending` until `Message::MeshExported` arrives, which is
    /// what a wait for an idle window looks at.
    fn start_mesh_export(
        &mut self,
        request: MeshExportRequest,
        path: PathBuf,
        format: MeshFormat,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        self.mesh_export_pending = true;
        self.status = format!(
            "Writing {} vertices and {} triangles as {}…",
            request.mesh.vertices.len(),
            request.mesh.triangles.len(),
            format.label()
        );
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    write(&request, path, format).map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())?
            },
            move |result| Message::MeshExported(api_job_id.clone(), result),
        )
    }

    /// The file was written, or could not be.
    pub(crate) fn mesh_exported(
        &mut self,
        api_job_id: Option<String>,
        result: Result<MeshExportDone, String>,
    ) {
        self.mesh_export_pending = false;
        if let Some(job) = api_job_id.and_then(|id| self.api_jobs.get_mut(&id)) {
            *job = match &result {
                Ok(done) => done.job_value(),
                Err(error) => json!({
                    "state": "failed",
                    "operation": "export_mesh",
                    "error": error,
                }),
            };
        }
        self.status = match result {
            Ok(done) => done.summary(),
            Err(error) => format!("Mesh export failed: {error}"),
        };
    }

    /// The `export_mesh` command of the local API: save the mesh of the
    /// active layer to a path whose extension names the format.
    pub(crate) fn api_export_mesh(&mut self, path: PathBuf) -> (Value, Task<Message>) {
        let refuse = |error: &str| (json!({"ok": false, "error": error}), Task::none());
        let Some(format) = MeshFormat::from_path(&path).filter(|_| path.is_absolute()) else {
            return refuse("export_mesh requires an absolute .obj, .ply or .stl destination");
        };
        if self.mesh_export_pending {
            return refuse("a mesh export is already open or running");
        }
        if self
            .active
            .and_then(|index| self.clouds.get(index))
            .is_none()
        {
            return refuse("no active cloud");
        }
        let Some(request) = self.mesh_export_request() else {
            return refuse("the active layer has no mesh");
        };
        if destination_format(&request.source, &path).is_err() {
            return refuse("export_mesh requires a destination different from the source file");
        }
        let id = self.record_api_job(json!({
            "state": "running",
            "operation": "export_mesh",
            "path": path,
            "format": format.extension(),
        }));
        let task = self.start_mesh_export(request, path.clone(), format, Some(id.clone()));
        (
            json!({"ok": true, "accepted": true, "job_id": id, "path": path}),
            task,
        )
    }

    /// The section of Properties about the mesh of the active layer: its
    /// size, what was measured of it, and the button that saves it.
    pub(crate) fn mesh_properties(&self) -> Option<Element<'_, Message>> {
        let entry = self.active.and_then(|index| self.clouds.get(index))?;
        let mesh = entry.mesh.as_ref()?;
        let mut rows = column![
            opencad_properties::section_header("Surface mesh"),
            opencad_properties::property_row("Vertices", format_count(mesh.vertices.len())),
            opencad_properties::property_row("Triangles", format_count(mesh.triangles.len())),
        ]
        .width(Fill);
        if let Some(topology) = entry.mesh_topology {
            rows = rows
                .push(opencad_properties::property_row(
                    "Open edges",
                    format_count(topology.open_edges),
                ))
                .push(opencad_properties::property_row(
                    "Connected parts",
                    format_count(topology.components),
                ));
        }
        Some(
            rows.push(
                container(
                    button(tr("Export mesh…"))
                        .on_press_maybe((!self.mesh_export_pending).then_some(Message::ExportMesh))
                        .style(flat_tool_style),
                )
                .padding([4, 8]),
            )
            .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use pointcloud_core::MeshStats;

    use super::*;
    use crate::native_api::{ApiCommand, ApiRequest};
    use crate::{MeshControl, MeshJob, MeshMode};

    /// A square of two triangles with a loose triangle beside it: seven open
    /// edges, four around the square and three around the triangle, in two
    /// parts. Colours and normals are per vertex.
    fn sheet() -> MeshGeometry {
        MeshGeometry {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [2.0, 0.0, 0.0],
                [2.0, 1.0, 0.5],
                [0.0, 1.0, 0.5],
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [5.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3], [4, 5, 6]],
            colors: Some(vec![
                [200, 10, 10],
                [10, 200, 10],
                [10, 10, 200],
                [90, 90, 90],
                [1, 2, 3],
                [4, 5, 6],
                [7, 8, 9],
            ]),
            normals: Some(vec![[0.0, 0.0, 1.0]; 7]),
        }
    }

    fn sheet_request(transform: CloudTransform) -> MeshExportRequest {
        MeshExportRequest {
            mesh: Arc::new(sheet()),
            source: PathBuf::from("scan.laz"),
            bag_source: false,
            transform,
        }
    }

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    fn job(studio: &mut Studio, id: &str) -> Value {
        send(studio, ApiCommand::Job { id: id.to_owned() })["job"].clone()
    }

    /// A window with one small scan open and active.
    fn studio_with_scan(directory: &Path) -> Studio {
        let path = directory.join("hall.xyz");
        std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        studio
    }

    fn close(found: [f64; 3], expected: [f64; 3], tolerance: f64) -> bool {
        (0..3).all(|axis| (found[axis] - expected[axis]).abs() <= tolerance)
    }

    #[test]
    fn extension_chooses_the_format_and_the_source_is_never_the_destination() {
        let source = Path::new("scan.laz");
        for (name, format) in [
            ("out.obj", MeshFormat::Obj),
            ("out.PLY", MeshFormat::Ply),
            ("folder.v2/out.stl", MeshFormat::Stl),
        ] {
            assert_eq!(destination_format(source, Path::new(name)), Ok(format));
        }
        for name in ["out", "out.txt", "out.off", "out.obj.bak"] {
            assert_eq!(
                destination_format(source, Path::new(name)),
                Err(NO_FORMAT),
                "{name}"
            );
        }
        assert_eq!(
            destination_format(Path::new("mesh.ply"), Path::new("mesh.ply")),
            Err(SAME_FILE)
        );
        // The dialog offers every format the core writes, each once.
        let offered: Vec<MeshFormat> = FORMATS.iter().map(|(format, _)| *format).collect();
        assert_eq!(offered, [MeshFormat::Obj, MeshFormat::Ply, MeshFormat::Stl]);
    }

    #[test]
    fn three_formats_hold_the_mesh_where_the_scene_shows_it() {
        let directory = tempfile::tempdir().unwrap();
        // A layer moved to survey coordinates, as a scan in RD New has them.
        let transform = CloudTransform {
            scale: [1.0; 3],
            offset: [207_000.25, 474_000.5, 10.0],
        };
        let request = sheet_request(transform);
        let expected: Vec<[f64; 3]> = sheet()
            .vertices
            .iter()
            .map(|xyz| transform.xyz(*xyz))
            .collect();
        for (format, tolerance) in [
            (MeshFormat::Obj, 1e-6),
            (MeshFormat::Ply, 1e-9),
            (MeshFormat::Stl, 1e-3),
        ] {
            let path = directory
                .path()
                .join(format!("sheet.{}", format.extension()));
            let done = write(&request, path.clone(), format).unwrap();
            assert_eq!((done.vertices, done.triangles), (7, 3));
            assert_eq!(done.format, format);
            let read = pointcloud_core::read_mesh_geometry(&path)
                .unwrap()
                .expect("the file holds faces");
            assert_eq!(read.triangles.len(), 3, "{format:?}");
            // Every corner of every triangle is where the scene has it.
            for (face, source) in read.triangles.iter().zip(&sheet().triangles) {
                for (corner, original) in face.iter().zip(source) {
                    assert!(
                        close(
                            read.vertices[*corner as usize],
                            expected[*original as usize],
                            tolerance
                        ),
                        "{format:?}: {:?}",
                        read.vertices[*corner as usize]
                    );
                }
            }
            match format {
                MeshFormat::Stl => {
                    // Only X and Y are too large for the floats of the format.
                    assert_eq!(done.origin, Some([207_000.0, 474_000.0, 0.0]));
                    assert_eq!(
                        pointcloud_core::read_stl_origin(&path).unwrap(),
                        done.origin
                    );
                    assert!(done.summary().contains("origin 207000 474000 0 m"));
                }
                _ => {
                    assert_eq!(done.origin, None);
                    assert!(!done.summary().contains("origin"));
                    let colors = read.colors.expect("colours are kept");
                    assert_eq!(colors[read.triangles[0][1] as usize], [10, 200, 10]);
                    let normals = read.normals.expect("normals are kept");
                    assert_eq!(normals[read.triangles[0][0] as usize], [0.0, 0.0, 1.0]);
                }
            }
        }
        // The binary PLY states double coordinates.
        let ply = std::fs::read(directory.path().join("sheet.ply")).unwrap();
        let header = String::from_utf8_lossy(&ply[..ply.len().min(400)]).into_owned();
        assert!(header.contains("format binary_little_endian 1.0"));
        assert!(header.contains("property double x"));
        assert!(header.contains("property uchar red"));
        assert!(header.contains("property float nx"));

        // A mesh near zero needs no origin in its STL.
        let near = directory.path().join("near.stl");
        let done = write(
            &sheet_request(CloudTransform::default()),
            near,
            MeshFormat::Stl,
        )
        .unwrap();
        assert_eq!(done.origin, None);
    }

    #[test]
    fn mesh_follows_the_move_and_scale_of_its_layer() {
        let mesh = sheet();
        let moved = in_scene(
            &mesh,
            CloudTransform {
                scale: [1.0, 1.0, 2.0],
                offset: [10.0, 0.0, -1.0],
            },
        );
        assert_eq!(moved.triangles, mesh.triangles);
        assert_eq!(moved.vertices[2], [12.0, 1.0, 0.0]);
        assert_eq!(moved.colors, mesh.colors);
        assert_eq!(moved.normals.unwrap()[0], [0.0, 0.0, 1.0]);

        // A layer turned half a circle by two negative factors is not
        // mirrored: the order stays.
        let turned = in_scene(
            &mesh,
            CloudTransform {
                scale: [-1.0, -1.0, 1.0],
                offset: [0.0; 3],
            },
        );
        assert_eq!(turned.triangles, mesh.triangles);
        assert_eq!(turned.normals.unwrap()[4], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn mirrored_layer_keeps_its_outside_in_the_file() {
        let mirror = CloudTransform {
            scale: [-1.0, 1.0, 1.0],
            offset: [0.0; 3],
        };
        let mesh = sheet();
        // The corners of every triangle are in reverse order, so the side
        // that faced up still does, and the normals stay on that side.
        let mirrored = in_scene(&mesh, mirror);
        assert_eq!(mirrored.vertices[5], [-6.0, 0.0, 0.0]);
        for (found, [a, b, c]) in mirrored.triangles.iter().zip(&mesh.triangles) {
            assert_eq!(found, &[*a, *c, *b]);
        }
        let [a, b, c] = mirrored.triangles[2].map(|index| mirrored.vertices[index as usize]);
        let up = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        assert!(up > 0.0);
        assert_eq!(mirrored.normals.unwrap()[4], [0.0, 0.0, 1.0]);
        // A normal along the mirrored axis points the other way, as the
        // surface it stands on does.
        let wall = MeshGeometry {
            normals: Some(vec![[1.0, 0.0, 0.0]; 7]),
            ..sheet()
        };
        assert_eq!(
            in_scene(&wall, mirror).normals.unwrap()[0],
            [-1.0, 0.0, 0.0]
        );

        // STL tells the outside by the order of the corners alone: the
        // normal the writer derives from it points up for every triangle.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mirrored.stl");
        write(&sheet_request(mirror), path.clone(), MeshFormat::Stl).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // An 80-byte header and a count, then 50 bytes per triangle that
        // begin with its normal.
        let facet_normal = |triangle: usize| -> [f32; 3] {
            let at = 84 + triangle * 50;
            std::array::from_fn(|axis| {
                let at = at + axis * 4;
                f32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
            })
        };
        assert_eq!(facet_normal(2), [0.0, 0.0, 1.0]);
        assert!((0..3).all(|triangle| facet_normal(triangle)[2] > 0.0));
    }

    /// A closed box as an OBJ file whose lid has another material than its
    /// walls and floor. The file shares its eight vertices between the faces.
    fn write_two_colour_box(directory: &Path) -> PathBuf {
        std::fs::write(
            directory.join("box.mtl"),
            "newmtl wall\nKd 0.8 0.8 0.8\nnewmtl roof\nKd 0.7 0.2 0.1\n",
        )
        .unwrap();
        let path = directory.join("box.obj");
        std::fs::write(
            &path,
            "mtllib box.mtl\n\
             v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
             usemtl wall\nf 1 4 3 2\nf 1 2 6 5\nf 2 3 7 6\nf 3 4 8 7\nf 4 1 5 8\n\
             usemtl roof\nf 5 6 7 8\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn mesh_file_with_split_vertices_is_measured_by_position() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_two_colour_box(directory.path());
        let measured = read_measured(&path).unwrap().expect("the file holds faces");
        // The reader gave the four corners of the lid a copy for its colour.
        assert_eq!(
            (measured.mesh.vertices.len(), measured.mesh.triangles.len()),
            (12, 12)
        );
        // By index that is an open box under a loose lid; the box is closed.
        let by_index = pointcloud_core::mesh_topology(&measured.mesh);
        assert_eq!((by_index.open_edges, by_index.components), (8, 2));
        assert_eq!(
            (measured.topology.open_edges, measured.topology.components),
            (0, 1)
        );

        // The figures do not depend on the format the box is opened from.
        for extension in ["stl", "ply", "obj"] {
            let copy = directory.path().join(format!("copy.{extension}"));
            convert_file(&path, &copy).unwrap();
            let again = read_measured(&copy).unwrap().unwrap();
            assert_eq!(
                (again.topology.open_edges, again.topology.components),
                (0, 1),
                "{extension}"
            );
        }

        // The layer that shows the file reports them.
        let cloud = Arc::new(pointcloud_core::open(&path, 100).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&cloud))));
        let _ = studio.update(Message::MeshLoaded(cloud, Ok(Some(measured))));
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(
            status["result"]["clouds"][0]["mesh"],
            json!({"vertices": 12, "triangles": 12, "open_edges": 0, "components": 1})
        );
    }

    #[test]
    fn credit_of_3d_bag_buildings_survives_saving_and_opening_again() {
        let directory = tempfile::tempdir().unwrap();
        let saved = |name: &str| directory.path().join(name);
        // As the download writes it: the credit on the first line.
        let downloaded = saved("buildings.obj");
        std::fs::write(
            &downloaded,
            "# © 3DBAG door tudelft3d en 3DGI · CC BY 4.0\n\
             # https://docs.3dbag.nl/nl/copyright/\n\
             o 3DBAG\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n",
        )
        .unwrap();
        assert!(is_bag3d_mesh(&downloaded));

        // Saved as OBJ and as PLY, the file is still known as 3D BAG, and so
        // is a file saved from either of those.
        for first in ["first.obj", "first.ply"] {
            convert_file(&downloaded, &saved(first)).unwrap();
            assert!(is_bag3d_mesh(&saved(first)), "{first}");
            for second in ["second.obj", "second.ply"] {
                convert_file(&saved(first), &saved(second)).unwrap();
                assert!(is_bag3d_mesh(&saved(second)), "{first} to {second}");
            }
        }

        // An OBJ file has the credit in the words of the register, after the
        // line of this application.
        let obj = std::fs::read_to_string(saved("second.obj")).unwrap();
        assert_eq!(
            obj.lines().nth(1),
            Some("# © 3DBAG door tudelft3d en 3DGI · CC BY 4.0")
        );
        // The header of a PLY file is ASCII, so the credit is too.
        let ply = std::fs::read(saved("second.ply")).unwrap();
        let end = ply
            .windows(11)
            .position(|window| window == b"end_header\n")
            .unwrap();
        assert!(ply[..end].is_ascii());
        assert!(String::from_utf8_lossy(&ply[..end])
            .contains("\ncomment (c) 3DBAG by tudelft3d and 3DGI, CC BY 4.0\n"));

        // A mesh from another source is not taken for 3D BAG.
        let plain = saved("plain.obj");
        pointcloud_core::write_obj_mesh(&sheet(), &plain, &[]).unwrap();
        assert!(!is_bag3d_mesh(&plain));
        convert_file(&plain, &saved("plain.ply")).unwrap();
        assert!(!is_bag3d_mesh(&saved("plain.ply")));
        // Only the comments that open the file count, not one further down.
        let late = saved("late.obj");
        std::fs::write(
            &late,
            "v 0 0 0\nv 1 0 0\nv 0 1 0\n# © 3DBAG door tudelft3d en 3DGI\nf 1 2 3\n",
        )
        .unwrap();
        assert!(!is_bag3d_mesh(&late));
        // STL has no room for a credit, and a missing file has none.
        convert_file(&downloaded, &saved("buildings.stl")).unwrap();
        assert!(!is_bag3d_mesh(&saved("buildings.stl")));
        assert!(!is_bag3d_mesh(&saved("missing.obj")));
    }

    #[test]
    fn failed_write_leaves_the_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("kept.ply");
        std::fs::write(&path, "earlier content").unwrap();
        let mut broken = sheet();
        broken.triangles.push([0, 1, 99]);
        let request = MeshExportRequest {
            mesh: Arc::new(broken),
            ..sheet_request(CloudTransform::default())
        };
        for format in [MeshFormat::Obj, MeshFormat::Ply, MeshFormat::Stl] {
            assert!(write(&request, path.clone(), format).is_err(), "{format:?}");
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "earlier content");
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn command_line_converts_a_mesh_file_by_the_extension_of_the_output() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("sheet.obj");
        pointcloud_core::write_obj_mesh(&sheet(), &source, &[]).unwrap();
        for extension in ["ply", "stl", "obj"] {
            let destination = directory.path().join(format!("copy.{extension}"));
            let lines = convert_file(&source, &destination).unwrap();
            assert_eq!(
                lines,
                format!(
                    "Mesh exported: 7 vertices, 3 triangles -> {}",
                    destination.display()
                )
            );
            let read = pointcloud_core::read_mesh_geometry(&destination)
                .unwrap()
                .unwrap();
            assert_eq!(read.triangles.len(), 3);
        }
        // A PLY mesh converts to STL as well: any mesh format is a source.
        let from_ply = directory.path().join("from-ply.stl");
        assert!(convert_file(&directory.path().join("copy.ply"), &from_ply).is_ok());

        let refused = |destination: &str| {
            convert_file(&source, &directory.path().join(destination)).unwrap_err()
        };
        assert_eq!(
            refused("copy.off"),
            (
                2,
                "Supported mesh export extensions: .obj, .ply, .stl".to_owned()
            )
        );
        assert_eq!(
            refused("sheet.obj"),
            (
                2,
                "Choose an output path different from the input".to_owned()
            )
        );
        // A file with points but no faces has nothing to write.
        let points = directory.path().join("points.obj");
        std::fs::write(&points, "v 0 0 0\nv 1 0 0\nv 0 1 0\n").unwrap();
        let (code, line) = convert_file(&points, &directory.path().join("points.stl")).unwrap_err();
        assert_eq!(code, 1);
        assert!(line.starts_with("Mesh export failed: "), "{line}");
        assert!(!directory.path().join("points.stl").exists());
    }

    #[test]
    fn command_line_tells_the_origin_of_an_stl_at_survey_coordinates() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("survey.ply");
        let survey = in_scene(
            &sheet(),
            CloudTransform {
                scale: [1.0; 3],
                offset: [121_000.0, 487_000.0, 0.0],
            },
        );
        pointcloud_core::write_mesh(&survey, &source, MeshFormat::Ply, &[]).unwrap();
        let destination = directory.path().join("survey.stl");
        let lines = convert_file(&source, &destination).unwrap();
        let mut lines = lines.lines();
        assert!(lines
            .next()
            .unwrap()
            .starts_with("Mesh exported: 7 vertices"));
        assert_eq!(
            lines.next(),
            Some(
                "STL coordinates are relative to the origin 121000 487000 0 m named in the file header"
            )
        );
        // This application reads the file back at its survey position.
        let read = pointcloud_core::read_mesh_geometry(&destination)
            .unwrap()
            .unwrap();
        let corner = read.vertices[read.triangles[0][0] as usize];
        assert!(
            close(corner, [121_000.0, 487_000.0, 0.0], 1e-3),
            "{corner:?}"
        );
    }

    #[test]
    fn api_exports_the_mesh_of_the_active_layer_as_a_job() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("hall-mesh.stl");
        let export = |studio: &mut Studio, path: &Path| {
            send(
                studio,
                ApiCommand::ExportMesh {
                    path: path.to_path_buf(),
                },
            )
        };

        let mut empty = Studio::default();
        assert_eq!(export(&mut empty, &destination)["error"], "no active cloud");

        let mut studio = studio_with_scan(directory.path());
        assert_eq!(
            export(&mut studio, &destination)["error"],
            "the active layer has no mesh"
        );
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["clouds"][0]["mesh"], Value::Null);

        let measured = MeasuredMesh::measure(sheet());
        studio.clouds[0].mesh = Some(Arc::clone(&measured.mesh));
        studio.clouds[0].mesh_topology = Some(measured.topology);
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(
            status["result"]["clouds"][0]["mesh"],
            json!({"vertices": 7, "triangles": 3, "open_edges": 7, "components": 2})
        );

        for refused in [
            PathBuf::from("relative.obj"),
            directory.path().join("mesh.txt"),
            directory.path().join("mesh"),
        ] {
            let answer = export(&mut studio, &refused);
            assert_eq!(
                answer["error"],
                "export_mesh requires an absolute .obj, .ply or .stl destination",
                "{}",
                refused.display()
            );
        }
        assert!(studio.api_jobs.is_empty(), "a refusal starts no job");
        assert!(!studio.mesh_export_pending);

        let accepted = export(&mut studio, &destination);
        assert_eq!(accepted["ok"], true);
        assert_eq!(accepted["accepted"], true);
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        let running = job(&mut studio, &id);
        assert_eq!(running["state"], "running");
        assert_eq!(running["operation"], "export_mesh");
        assert_eq!(running["format"], "stl");
        // A wait for an idle window sees the export as work under way.
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(crate::mcp::busy(&status["result"]), ["mesh_export"]);
        assert_eq!(
            export(&mut studio, &directory.path().join("second.obj"))["error"],
            "a mesh export is already open or running"
        );

        // What the worker thread does, and its message to the window.
        let request = studio.mesh_export_request().unwrap();
        let result = write(&request, destination.clone(), MeshFormat::Stl)
            .map_err(|error| error.to_string());
        let _ = studio.update(Message::MeshExported(Some(id.clone()), result));
        let complete = job(&mut studio, &id);
        assert_eq!(
            complete,
            json!({
                "state": "complete",
                "operation": "export_mesh",
                "path": destination,
                "format": "stl",
                "vertices": 7,
                "triangles": 3,
                "origin": null,
            })
        );
        assert!(destination.is_file());
        let status = send(&mut studio, ApiCommand::Status);
        assert!(crate::mcp::busy(&status["result"]).is_empty());
        assert!(studio
            .status
            .starts_with("Exported mesh as STL: 7 vertices"));

        // A write that fails ends its job as failed.
        let accepted = export(&mut studio, &directory.path().join("missing/x.ply"));
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        let result = write(
            &request,
            directory.path().join("missing/x.ply"),
            MeshFormat::Ply,
        )
        .map_err(|error| error.to_string());
        assert!(result.is_err());
        let _ = studio.update(Message::MeshExported(Some(id.clone()), result));
        let failed = job(&mut studio, &id);
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["operation"], "export_mesh");
        assert!(failed["error"].is_string());
        assert!(!studio.mesh_export_pending);
    }

    #[test]
    fn api_refuses_to_write_a_mesh_over_its_own_source() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("sheet.obj");
        pointcloud_core::write_obj_mesh(&sheet(), &source, &[]).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&cloud))));
        let _ = studio.update(Message::MeshLoaded(
            cloud,
            Ok(Some(MeasuredMesh::measure(sheet()))),
        ));
        assert_eq!(
            studio.clouds[0].mesh_topology.map(|found| found.open_edges),
            Some(7)
        );
        let answer = send(&mut studio, ApiCommand::ExportMesh { path: source });
        assert_eq!(
            answer["error"],
            "export_mesh requires a destination different from the source file"
        );
        assert!(!studio.mesh_export_pending);
    }

    #[test]
    fn save_dialog_result_starts_the_write_or_says_why_not() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = studio_with_scan(directory.path());
        // Without a mesh there is nothing to ask a file name for.
        let _ = studio.update(Message::ExportMesh);
        assert!(!studio.mesh_export_pending);
        assert_eq!(studio.status, "Select a cloud with a surface mesh first");

        studio.clouds[0].mesh = Some(Arc::new(sheet()));
        let _ = studio.update(Message::ExportMesh);
        assert!(studio.mesh_export_pending);
        assert_eq!(
            studio.status,
            "Choose where to save the mesh as OBJ, PLY or STL…"
        );
        let request = studio.mesh_export_request().unwrap();

        let _ = studio.update(Message::MeshExportPathChosen(request.clone(), None));
        assert!(!studio.mesh_export_pending);
        assert_eq!(studio.status, "Mesh export cancelled");

        studio.mesh_export_pending = true;
        let _ = studio.update(Message::MeshExportPathChosen(
            request.clone(),
            Some(directory.path().join("mesh.txt")),
        ));
        assert!(!studio.mesh_export_pending);
        assert_eq!(studio.status, NO_FORMAT);

        studio.mesh_export_pending = true;
        let _ = studio.update(Message::MeshExportPathChosen(
            request.clone(),
            Some(request.source.clone()),
        ));
        assert!(!studio.mesh_export_pending);
        assert_eq!(studio.status, NO_FORMAT, "a scan file is no mesh format");

        let _ = studio.update(Message::MeshExportPathChosen(
            request,
            Some(directory.path().join("mesh.ply")),
        ));
        assert!(studio.mesh_export_pending);
        assert_eq!(studio.status, "Writing 7 vertices and 3 triangles as PLY…");
    }

    #[test]
    fn mesh_job_reports_open_edges_and_connected_parts() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = studio_with_scan(directory.path());
        let path = directory.path().join("hall-terrain.obj");
        let id = studio.record_api_job(json!({"state": "running", "operation": "mesh"}));
        studio.mesh_job = Some(MeshJob {
            mode: MeshMode::Terrain,
            path: path.clone(),
            control: Arc::new(MeshControl::new(4)),
            started: Instant::now(),
            api_job_id: Some(id.clone()),
        });
        let source = Arc::clone(&studio.clouds[0].cloud);
        let stats = MeshStats {
            source_points: 4,
            vertices: 7,
            triangles: 3,
        };
        let _ = studio.update(Message::MeshReady(
            MeshMode::Terrain,
            Ok((source, path, stats, MeasuredMesh::measure(sheet()))),
        ));
        let done = job(&mut studio, &id);
        assert_eq!(done["state"], "complete");
        assert_eq!(done["vertices"], 7);
        assert_eq!(done["triangles"], 3);
        assert_eq!(done["open_edges"], 7);
        assert_eq!(done["components"], 2);
        assert!(
            studio
                .status
                .contains("7 vertices, 3 triangles, 7 open edges, 2 connected parts"),
            "{}",
            studio.status
        );
        let entry = &studio.clouds[0];
        assert!(entry.mesh.is_some());
        assert_eq!(entry.mesh_topology.map(|found| found.components), Some(2));
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["clouds"][0]["mesh"]["open_edges"], 7);
    }

    #[test]
    fn counts_in_a_status_line_read_as_a_sentence() {
        let closed = MeshTopology {
            components: 1,
            ..MeshTopology::default()
        };
        assert_eq!(topology_text(closed), "0 open edges, 1 connected part");
        let rim = MeshTopology {
            open_edges: 1,
            components: 3,
            ..MeshTopology::default()
        };
        assert_eq!(topology_text(rim), "1 open edge, 3 connected parts");
    }

    #[test]
    fn properties_show_the_measured_mesh_in_the_language_in_use() {
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::Table(0));
        let directory = tempfile::tempdir().unwrap();
        let mut studio = studio_with_scan(directory.path());
        assert!(studio.mesh_properties().is_none());
        // A mesh that was not measured shows its size and the button.
        studio.clouds[0].mesh = Some(Arc::new(sheet()));
        assert!(studio.mesh_properties().is_some());
        studio.clouds[0].mesh_topology = Some(pointcloud_core::mesh_topology(&sheet()));
        assert!(studio.mesh_properties().is_some());
        let _ = studio.view();
        assert_eq!(tr("Export mesh…"), "Mesh exporteren…");
        assert_eq!(tr("Open edges"), "Open randen");
        assert_eq!(tr("Connected parts"), "Samenhangende delen");
    }
}
