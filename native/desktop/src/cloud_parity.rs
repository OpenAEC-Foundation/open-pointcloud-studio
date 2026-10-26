//! The command line, the window, the cloud API and the shared operations of
//! `pointcloud_core::ops` make the same files: each mode of the command line
//! writes what its operation writes, the window's drawing job writes what
//! the operation of a section drawing writes, and each tool of the cloud API
//! writes what its mode of the command line writes. The cloud tools also
//! take the arguments of the desktop tools with the same names and limits.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use pointcloud_core::ops::{self, CachedIndex, Convert, SectionDrawingOptions};
use pointcloud_core::{
    Bounds, DrawingFormat, DrawingView, ExportFormat, OrientedBox, SurfaceMeshConfig,
};

use pointcloud_server::payload::encode_base64;
use pointcloud_server::tools::{self as cloud, CallLimits, ToolOutput};

use crate::{cli, drawing};

/// A 10 by 10 by 10 grid of magenta points 0.25 apart, as ASCII PLY, as
/// the smoke test of the packages writes it.
fn write_grid(path: &Path) {
    let mut text = String::from("ply\nformat ascii 1.0\nelement vertex 1000\n");
    text.push_str("property float x\nproperty float y\nproperty float z\n");
    text.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
    text.push_str("end_header\n");
    for x in 0..10 {
        for y in 0..10 {
            for z in 0..10 {
                let at = |step: i32| f64::from(step) * 0.25;
                text.push_str(&format!(
                    "{:.2} {:.2} {:.2} 255 0 255\n",
                    at(x),
                    at(y),
                    at(z)
                ));
            }
        }
    }
    fs::write(path, text).unwrap();
}

/// The walls of a room of 4 by 3 m and 2.5 m high, a point every 0.1 m.
fn write_room(path: &Path) {
    let mut text = String::new();
    for level in 0..=25 {
        let z = f64::from(level) * 0.1;
        for step in 0..=40 {
            let x = f64::from(step) * 0.1;
            text.push_str(&format!("{x:.3} 0.000 {z:.3}\n{x:.3} 3.000 {z:.3}\n"));
        }
        for step in 1..30 {
            let y = f64::from(step) * 0.1;
            text.push_str(&format!("0.000 {y:.3} {z:.3}\n4.000 {y:.3} {z:.3}\n"));
        }
    }
    fs::write(path, text).unwrap();
}

/// A PTX scan of 10 by 10 points from one station with a pose.
fn write_station(path: &Path) {
    let mut text = String::from("10\n10\n1.5 2.5 1.75\n0 1 0\n-1 0 0\n0 0 1\n");
    text.push_str("0 1 0 0\n-1 0 0 0\n0 0 1 0\n1.5 2.5 1.75 1\n");
    for column in 0..10 {
        for row in 0..10 {
            text.push_str(&format!(
                "{} {} 1 0.5 100 150 200\n",
                f64::from(column) * 0.1 + 0.05,
                f64::from(row) * 0.1 + 0.05
            ));
        }
    }
    fs::write(path, text).unwrap();
}

/// The inputs of the tests, each named `input.<extension>` so that the
/// layer of a drawing is called `input` whichever way it is made.
struct Inputs {
    directory: tempfile::TempDir,
    grid: PathBuf,
    room: PathBuf,
    station: PathBuf,
}

impl Inputs {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        for folder in ["grid", "room", "station", "cli", "ops"] {
            fs::create_dir(directory.path().join(folder)).unwrap();
        }
        let grid = directory.path().join("grid/input.ply");
        let room = directory.path().join("room/input.xyz");
        let station = directory.path().join("station/input.ptx");
        write_grid(&grid);
        write_room(&room);
        write_station(&station);
        Self {
            directory,
            grid,
            room,
            station,
        }
    }

    /// Where one side of a comparison writes its file.
    fn output(&self, side: &str, name: &str) -> PathBuf {
        self.directory.path().join(side).join(name)
    }
}

fn arguments(values: &[&dyn AsRef<std::ffi::OsStr>]) -> Vec<OsString> {
    values
        .iter()
        .map(|value| value.as_ref().to_os_string())
        .collect()
}

/// A plan of the room under 1.2 m, and a box turned along the grid.
fn room_plan() -> OrientedBox {
    OrientedBox::from(Bounds {
        min: [-0.5, -0.5, 0.0],
        max: [4.5, 3.5, 1.2],
    })
}

/// The limits of `room_plan` as the command line takes them.
const ROOM_PLAN: &str = "-0.5,-0.5,0,4.5,3.5,1.2";

fn same_bytes(left: &Path, right: &Path) {
    let (left_bytes, right_bytes) = (fs::read(left).unwrap(), fs::read(right).unwrap());
    assert!(
        left_bytes == right_bytes,
        "{} and {} differ ({} and {} bytes)",
        left.display(),
        right.display(),
        left_bytes.len(),
        right_bytes.len()
    );
}

#[test]
fn the_drawing_block_asks_for_the_shared_defaults_of_every_view() {
    for view in DrawingView::ALL {
        let options = SectionDrawingOptions {
            view,
            ..SectionDrawingOptions::default()
        };
        assert_eq!(
            drawing::block_request_for_view(view).unwrap(),
            options.request().unwrap(),
            "{view:?}"
        );
    }
}

#[test]
fn the_window_job_draws_what_the_shared_operation_draws() {
    let inputs = Inputs::new();
    let front = OrientedBox::from(Bounds {
        min: [-0.5, -0.05, 0.0],
        max: [4.5, 3.5, 2.6],
    });
    for (view, section) in [
        (DrawingView::Plan, room_plan()),
        (DrawingView::Front, front),
    ] {
        for format in DrawingFormat::ALL {
            let name = format!("{}.{}", view.key(), format.extension());
            let (window, shared) = (inputs.output("cli", &name), inputs.output("ops", &name));
            let options = SectionDrawingOptions {
                view,
                ..SectionDrawingOptions::default()
            };
            let request = options.request().unwrap();
            let cloud = ops::open_source(&inputs.room, &ops::never_stop).unwrap();
            let by_window =
                drawing::window_job_of_file(cloud, None, section, request, &window, format)
                    .unwrap();
            let by_operation = ops::section_drawing_file(
                &inputs.room,
                "input",
                &shared,
                section,
                &options,
                CachedIndex::Never,
                &ops::never_stop,
            )
            .unwrap();
            assert_eq!(by_window, by_operation.stats, "{name}");
            assert!(by_window.drawn_points > 0, "{name}");
            same_bytes(&window, &shared);
        }
    }
}

#[test]
fn every_mode_of_the_command_line_writes_what_its_operation_writes() {
    let inputs = Inputs::new();
    let never_stop: ops::KeepGoing<'_> = &ops::never_stop;

    // --export, to every format but E57, whose identifiers differ per file.
    for extension in ["ply", "xyz", "pts", "csv", "las", "laz"] {
        let name = format!("export.{extension}");
        let (by_cli, by_ops) = (inputs.output("cli", &name), inputs.output("ops", &name));
        let answer = cli::export(&arguments(&[&inputs.room, &by_cli])).unwrap();
        assert_eq!(answer, None);
        let convert = Convert {
            format: ops::export_format_for_key(extension).unwrap(),
            section: None,
            max_points: None,
        };
        ops::convert_file(&inputs.room, &by_ops, convert, never_stop).unwrap();
        same_bytes(&by_cli, &by_ops);
    }

    // --section with a turn.
    let (by_cli, by_ops) = (
        inputs.output("cli", "section.ply"),
        inputs.output("ops", "section.ply"),
    );
    let answer = cli::section(&arguments(&[
        &inputs.grid,
        &"-1,-1,-1,1.1,2.5,2.5",
        &by_cli,
        &"--rotation",
        &"30",
    ]))
    .unwrap()
    .unwrap();
    let section = OrientedBox::new(
        Bounds {
            min: [-1.0, -1.0, -1.0],
            max: [1.1, 2.5, 2.5],
        },
        30.0,
    );
    let convert = Convert {
        format: ExportFormat::PlyBinary,
        section: Some(section),
        max_points: None,
    };
    let count = ops::convert_file(&inputs.grid, &by_ops, convert, never_stop).unwrap();
    assert_eq!(
        answer,
        format!("Exported {count} points to {}", by_cli.display())
    );
    same_bytes(&by_cli, &by_ops);

    // --drawing.
    let (by_cli, by_ops) = (
        inputs.output("cli", "plan.dxf"),
        inputs.output("ops", "plan.dxf"),
    );
    drawing::command_line(&arguments(&[&inputs.room, &ROOM_PLAN, &by_cli])).unwrap();
    ops::section_drawing_file(
        &inputs.room,
        "input",
        &by_ops,
        room_plan(),
        &SectionDrawingOptions::default(),
        CachedIndex::Never,
        never_stop,
    )
    .unwrap();
    same_bytes(&by_cli, &by_ops);

    // --mesh.
    let (by_cli, by_ops) = (
        inputs.output("cli", "terrain.obj"),
        inputs.output("ops", "terrain.obj"),
    );
    let answer = cli::mesh(&arguments(&[&inputs.grid, &by_cli]))
        .unwrap()
        .unwrap();
    let stats = ops::terrain_mesh_file(&inputs.grid, &by_ops, never_stop).unwrap();
    assert_eq!(
        answer,
        format!(
            "Mesh ready: {} source points, {} vertices, {} triangles -> {}",
            stats.source_points,
            stats.vertices,
            stats.triangles,
            by_cli.display()
        )
    );
    same_bytes(&by_cli, &by_ops);

    // --surface with two of its options.
    let (by_cli, by_ops) = (
        inputs.output("cli", "surface.obj"),
        inputs.output("ops", "surface.obj"),
    );
    cli::surface(&arguments(&[
        &inputs.room,
        &by_cli,
        &"--max-vertices",
        &"2000",
        &"--neighbors",
        &"10",
    ]))
    .unwrap();
    let config = SurfaceMeshConfig {
        max_vertices: 2000,
        neighbors: 10,
        ..SurfaceMeshConfig::default()
    };
    ops::surface_mesh_file(&inputs.room, &by_ops, config, never_stop).unwrap();
    same_bytes(&by_cli, &by_ops);

    // --scans.
    let answer = cli::scans(&arguments(&[&inputs.station])).unwrap().unwrap();
    let cloud = ops::open_source(&inputs.station, never_stop).unwrap();
    assert_eq!(answer, cli::station_lines(&ops::inspect(&cloud).stations));
    assert!(answer.starts_with("1 scan position(s)\n"), "{answer}");
    assert!(answer.contains(": 1.500000, 2.500000, 1.750000\n  X: +0.000000, +1.000000, +0.000000"));
}

#[test]
fn the_modes_refuse_as_they_did() {
    let inputs = Inputs::new();
    let output = inputs.output("cli", "out.ply");
    let usage = Err((2, String::new()));
    assert_eq!(cli::export(&arguments(&[&inputs.grid])), usage);
    assert_eq!(cli::mesh(&arguments(&[&inputs.grid])), usage);
    assert_eq!(cli::scans(&arguments(&[])), usage);
    assert_eq!(cli::surface(&arguments(&[&inputs.grid])), usage);
    assert_eq!(
        cli::surface(&arguments(&[&inputs.grid, &output, &"--max-vertices"])),
        usage
    );
    assert_eq!(
        cli::surface(&arguments(&[&inputs.grid, &output, &"--colour", &"red"])),
        usage
    );
    assert_eq!(
        cli::surface(&arguments(&[
            &inputs.grid,
            &output,
            &"--neighbors",
            &"many"
        ])),
        Err((2, "invalid --neighbors: many".into()))
    );
    let unsupported = Err((
        2,
        "Supported export extensions: .ply, .xyz, .pts, .csv, .las, .laz, .e57".into(),
    ));
    assert_eq!(
        cli::export(&arguments(&[&inputs.grid, &"out.dxf"])),
        unsupported
    );
    assert_eq!(
        cli::section(&arguments(&[&inputs.grid, &"0,0,0,1,1,1", &"out.dxf"])),
        unsupported
    );
    assert_eq!(
        cli::section(&arguments(&[&inputs.grid, &"0,0,0,1,1,1"])),
        usage
    );
    assert_eq!(
        cli::section(&arguments(&[
            &inputs.grid,
            &"0,0,0,1,1,1",
            &output,
            &"--rotation"
        ])),
        usage
    );
    assert_eq!(
        cli::section(&arguments(&[
            &inputs.grid,
            &"0,0,0,1,1,1",
            &output,
            &"--rotation",
            &"x"
        ])),
        Err((2, "--rotation must be a number of degrees".into()))
    );
    let limits = Err((
        2,
        "Section limits must be six comma-separated numbers".into(),
    ));
    assert_eq!(
        cli::section(&arguments(&[&inputs.grid, &"0,0,0,1,1", &output])),
        limits
    );
    assert_eq!(
        cli::section(&arguments(&[&inputs.grid, &"0,0,0,1,1,x", &output])),
        limits
    );
    let missing = inputs.output("grid", "missing.ply");
    let Err((1, line)) = cli::export(&arguments(&[&missing, &output])) else {
        panic!("a missing input fails");
    };
    assert!(line.starts_with("Export failed: I/O error: "), "{line}");
    let Err((1, line)) = cli::scans(&arguments(&[&missing])) else {
        panic!("a missing input fails");
    };
    assert!(line.starts_with("Scan positions failed: "), "{line}");
}

// --- The cloud API beside the command line --------------------------------

/// One call of a tool of the cloud API on `arguments`, as the service makes
/// it: in its own work folder, which the result lies in.
fn cloud_call(name: &str, arguments: serde_json::Value) -> (ToolOutput, tempfile::TempDir) {
    let work = tempfile::tempdir().unwrap();
    let limits = CallLimits {
        max_output: u64::MAX,
    };
    let output = cloud::call_value(name, &arguments, &limits, &work, &ops::never_stop)
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
    (output, work)
}

/// The `input` of a cloud tool for a file, named as the file is.
fn cloud_input(path: &Path) -> serde_json::Value {
    serde_json::json!({
        "format": path.extension().unwrap().to_str().unwrap(),
        "data": encode_base64(&fs::read(path).unwrap()),
        "name": path.file_name().unwrap().to_str().unwrap(),
    })
}

/// The file a cloud tool answered with.
fn cloud_file(output: &ToolOutput) -> PathBuf {
    output.file.as_ref().expect("a file result").path.clone()
}

/// A point as the comparison of two E57 files looks at it: where it lies,
/// its colour and its intensity.
type Compared = ([f64; 3], Option<[u8; 3]>, Option<u16>);

/// Every point of a file, as the core reads it back.
fn points_of(path: &Path) -> Vec<Compared> {
    let mut points = Vec::new();
    pointcloud_core::visit_points(path, &mut |point| {
        points.push((point.xyz, point.rgb, point.intensity));
        Ok(())
    })
    .unwrap();
    points
}

#[test]
fn every_cloud_tool_writes_what_its_mode_of_the_command_line_writes() {
    let inputs = Inputs::new();

    // --export, and convert without a box.
    for extension in ["ply", "xyz", "laz", "e57"] {
        let by_cli = inputs.output("cli", &format!("export.{extension}"));
        cli::export(&arguments(&[&inputs.grid, &by_cli])).unwrap();
        let (output, _work) = cloud_call(
            "convert",
            serde_json::json!({"input": cloud_input(&inputs.grid), "output_format": extension}),
        );
        assert_eq!(output.report["points"], 1000);
        if extension == "e57" {
            // Every E57 file has identifiers of its own; the points agree.
            assert_eq!(points_of(&by_cli), points_of(&cloud_file(&output)));
            assert_eq!(points_of(&by_cli).len(), 1000);
        } else {
            same_bytes(&by_cli, &cloud_file(&output));
        }
    }

    // --section with a turn, and convert with a turned box.
    let by_cli = inputs.output("cli", "section.ply");
    cli::section(&arguments(&[
        &inputs.grid,
        &"-1,-1,-1,1.1,2.5,2.5",
        &by_cli,
        &"--rotation",
        &"30",
    ]))
    .unwrap();
    let (output, _work) = cloud_call(
        "convert",
        serde_json::json!({
            "input": cloud_input(&inputs.grid),
            "output_format": "ply",
            "box": {"min": [-1.0, -1.0, -1.0], "max": [1.1, 2.5, 2.5], "rotation": 30.0},
        }),
    );
    same_bytes(&by_cli, &cloud_file(&output));

    // --drawing, and section_drawing, as a plan and a front view in DXF and
    // as a plan in DWG.
    let front_limits = "-0.5,-0.05,0,4.5,3.5,2.6";
    let front_box = serde_json::json!({"min": [-0.5, -0.05, 0.0], "max": [4.5, 3.5, 2.6]});
    let plan_box = serde_json::json!({"min": [-0.5, -0.5, 0.0], "max": [4.5, 3.5, 1.2]});
    for (view, limits, section, extension) in [
        ("plan", ROOM_PLAN, &plan_box, "dxf"),
        ("front", front_limits, &front_box, "dxf"),
        ("plan", ROOM_PLAN, &plan_box, "dwg"),
    ] {
        let by_cli = inputs.output("cli", &format!("{view}.{extension}"));
        drawing::command_line(&arguments(&[
            &inputs.room,
            &limits,
            &by_cli,
            &"--view",
            &view,
        ]))
        .unwrap();
        let (output, _work) = cloud_call(
            "section_drawing",
            serde_json::json!({
                "input": cloud_input(&inputs.room),
                "box": section,
                "view": view,
                "output_format": extension,
            }),
        );
        assert!(output.report["drawn_points"].as_u64().unwrap() > 0);
        same_bytes(&by_cli, &cloud_file(&output));
    }

    // --mesh, and terrain_mesh.
    let by_cli = inputs.output("cli", "terrain.obj");
    cli::mesh(&arguments(&[&inputs.grid, &by_cli])).unwrap();
    let (output, _work) = cloud_call(
        "terrain_mesh",
        serde_json::json!({"input": cloud_input(&inputs.grid)}),
    );
    same_bytes(&by_cli, &cloud_file(&output));

    // --surface with two options, and surface_mesh with the same two.
    let by_cli = inputs.output("cli", "surface.obj");
    cli::surface(&arguments(&[
        &inputs.room,
        &by_cli,
        &"--max-vertices",
        &"2000",
        &"--edge-factor",
        &"3",
    ]))
    .unwrap();
    let (output, _work) = cloud_call(
        "surface_mesh",
        serde_json::json!({"input": cloud_input(&inputs.room), "max_vertices": 2000, "edge_factor": 3}),
    );
    same_bytes(&by_cli, &cloud_file(&output));

    // --scans, and the stations inspect reports.
    let lines = cli::scans(&arguments(&[&inputs.station])).unwrap().unwrap();
    let (output, _work) = cloud_call(
        "inspect",
        serde_json::json!({"input": cloud_input(&inputs.station)}),
    );
    let stations: Vec<pointcloud_core::ScanPose> =
        serde_json::from_value(output.report["stations"].clone()).unwrap();
    assert_eq!(stations.len(), 1);
    assert_eq!(lines, cli::station_lines(&stations));
}

/// The parts of an argument schema that say what it accepts.
fn vocabulary(schema: &serde_json::Value) -> Vec<(&'static str, serde_json::Value)> {
    ["type", "enum", "minimum", "maximum", "exclusiveMinimum"]
        .into_iter()
        .filter_map(|key| {
            let value = schema.get(key)?;
            // A limit is a number, whether it was written as 3 or as 3.0.
            let value = match value.as_f64() {
                Some(number) if value.is_number() => serde_json::json!(number),
                _ => value.clone(),
            };
            Some((key, value))
        })
        .collect()
}

#[test]
fn the_cloud_tools_take_the_arguments_of_the_desktop_tools() {
    let desktop = |tool: &str, argument: &str| {
        let tool = crate::mcp::find_tool(tool).unwrap_or_else(|| panic!("{tool}"));
        tool.schema["properties"][argument].clone()
    };
    let cloud = |tool: &str, path: &[&str]| {
        let mut schema = cloud::find(tool).unwrap().input_schema();
        for part in path {
            schema = schema["properties"][part].clone();
        }
        schema
    };
    let pairs = [
        (
            ("set_section", "rotation"),
            ("convert", vec!["box", "rotation"]),
        ),
        (
            ("set_section", "rotation"),
            ("section_drawing", vec!["box", "rotation"]),
        ),
        (
            ("export_drawing", "view"),
            ("section_drawing", vec!["view"]),
        ),
        (
            ("export_drawing", "thickness"),
            ("section_drawing", vec!["thickness"]),
        ),
        (
            ("export_drawing", "units"),
            ("section_drawing", vec!["units"]),
        ),
        (
            ("export_drawing", "fill"),
            ("section_drawing", vec!["fill"]),
        ),
        (
            ("set_surface_settings", "max_vertices"),
            ("surface_mesh", vec!["max_vertices"]),
        ),
        (
            ("set_surface_settings", "neighbors"),
            ("surface_mesh", vec!["neighbors"]),
        ),
        (
            ("set_surface_settings", "edge_factor"),
            ("surface_mesh", vec!["edge_factor"]),
        ),
        (
            ("set_surface_settings", "mesh_size"),
            ("surface_mesh", vec!["mesh_size"]),
        ),
    ];
    for ((desktop_tool, argument), (cloud_tool, path)) in pairs {
        let (desktop, cloud) = (desktop(desktop_tool, argument), cloud(cloud_tool, &path));
        assert!(cloud.is_object(), "{cloud_tool} {path:?}");
        assert_eq!(
            vocabulary(&desktop),
            vocabulary(&cloud),
            "{desktop_tool}.{argument} and {cloud_tool}.{}",
            path.join(".")
        );
        assert!(
            !vocabulary(&desktop).is_empty(),
            "{desktop_tool}.{argument}"
        );
    }
    // The box of the cloud has the corners of set_section.
    for corner in ["min", "max"] {
        let desktop = desktop("set_section", corner);
        let cloud = cloud("convert", &["box", corner]);
        for key in ["type", "minItems", "maxItems", "items"] {
            assert_eq!(desktop[key], cloud[key], "{corner}.{key}");
        }
    }
}
