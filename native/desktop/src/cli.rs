//! The modes of the command line that convert, cut, mesh and list one scan
//! file: `--export`, `--section`, `--mesh`, `--surface` and `--scans`. Each
//! reads the arguments that follow its flag, makes the one call of
//! `pointcloud_core::ops` that does the work, and returns the line to print.
//! `main` keeps the flags and the usage lines, and prints.
//!
//! An answer is `Ok(Some(line))` for a line to print, `Ok(None)` when there
//! is nothing to print, `Err((2, ""))` for the usage line of the mode, and
//! `Err((code, line))` for a line that says what is wrong.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use pointcloud_core::ops::{self, Convert};
use pointcloud_core::{Bounds, OrientedBox, SurfaceMeshConfig};

/// What a mode of the command line answers.
pub(crate) type Outcome = Result<Option<String>, (i32, String)>;

const EXPORT_EXTENSIONS: &str =
    "Supported export extensions: .ply, .xyz, .pts, .csv, .las, .laz, .e57";

fn usage() -> (i32, String) {
    (2, String::new())
}

fn wrong(line: &str) -> (i32, String) {
    (2, line.to_owned())
}

/// Print what a mode answered, with `usage` for the usage line, and end the
/// process with its code when it failed.
pub(crate) fn finish(outcome: Outcome, usage: &str) {
    match outcome {
        Ok(line) => {
            if let Some(line) = line {
                println!("{line}");
            }
        }
        Err((code, line)) => {
            if line.is_empty() {
                eprintln!("{usage}");
            } else {
                eprintln!("{line}");
            }
            std::process::exit(code);
        }
    }
}

/// `--export INPUT OUTPUT`: the whole source in the format of the output's
/// extension.
pub(crate) fn export(arguments: &[OsString]) -> Outcome {
    let [source, destination] = arguments else {
        return Err(usage());
    };
    let (source, destination) = (PathBuf::from(source), PathBuf::from(destination));
    let Some(format) = ops::export_format_for_path(&destination) else {
        return Err(wrong(EXPORT_EXTENSIONS));
    };
    let convert = Convert {
        format,
        section: None,
        max_points: None,
    };
    ops::convert_file(&source, &destination, convert, &ops::never_stop)
        .map(|_| None)
        .map_err(|error| (1, format!("Export failed: {error}")))
}

/// `--section INPUT XMIN,YMIN,ZMIN,XMAX,YMAX,ZMAX OUTPUT [--rotation DEGREES]`:
/// the points of the source inside a box, turned or not.
pub(crate) fn section(arguments: &[OsString]) -> Outcome {
    let [source, limits, destination, options @ ..] = arguments else {
        return Err(usage());
    };
    let rotation = match options {
        [] => 0.0,
        [flag, value] if flag == "--rotation" => value
            .to_str()
            .and_then(crate::parse_rotation)
            .ok_or_else(|| wrong("--rotation must be a number of degrees"))?,
        _ => return Err(usage()),
    };
    let (source, destination) = (PathBuf::from(source), PathBuf::from(destination));
    let Some(format) = ops::export_format_for_path(&destination) else {
        return Err(wrong(EXPORT_EXTENSIONS));
    };
    let values: Option<Vec<f64>> = limits.to_str().and_then(|limits| {
        limits
            .split(',')
            .map(str::parse::<f64>)
            .collect::<Result<_, _>>()
            .ok()
    });
    let Some([x0, y0, z0, x1, y1, z1]) =
        values.and_then(|values| <[f64; 6]>::try_from(values).ok())
    else {
        return Err(wrong("Section limits must be six comma-separated numbers"));
    };
    let section = OrientedBox::new(
        Bounds {
            min: [x0, y0, z0],
            max: [x1, y1, z1],
        },
        rotation,
    );
    let convert = Convert {
        format,
        section: Some(section),
        max_points: None,
    };
    match ops::convert_file(&source, &destination, convert, &ops::never_stop) {
        Ok(count) => Ok(Some(format!(
            "Exported {count} points to {}",
            destination.display()
        ))),
        Err(error) => Err((1, format!("Section export failed: {error}"))),
    }
}

/// `--mesh INPUT OUTPUT.obj`: the terrain under the points as a TIN.
pub(crate) fn mesh(arguments: &[OsString]) -> Outcome {
    let [source, destination] = arguments else {
        return Err(usage());
    };
    let destination = PathBuf::from(destination);
    match ops::terrain_mesh_file(Path::new(source), &destination, &ops::never_stop) {
        Ok(stats) => Ok(Some(format!(
            "Mesh ready: {} source points, {} vertices, {} triangles -> {}",
            stats.source_points,
            stats.vertices,
            stats.triangles,
            destination.display()
        ))),
        Err(error) => Err((1, format!("Meshing failed: {error}"))),
    }
}

/// `--surface INPUT OUTPUT.obj [--max-vertices N] [--neighbors N]
/// [--edge-factor N] [--mesh-size SIZE]`: a 3D surface through the points.
pub(crate) fn surface(arguments: &[OsString]) -> Outcome {
    let [source, destination, options @ ..] = arguments else {
        return Err(usage());
    };
    let mut config = SurfaceMeshConfig::default();
    let mut options = options.iter();
    while let Some(option) = options.next() {
        let Some(value) = options.next() else {
            return Err(usage());
        };
        let value = value.to_string_lossy();
        let invalid = |flag: &str| (2, format!("invalid {flag}: {value}"));
        match option.to_str() {
            Some(flag @ "--max-vertices") => {
                config.max_vertices = value.parse().map_err(|_| invalid(flag))?;
            }
            Some(flag @ "--neighbors") => {
                config.neighbors = value.parse().map_err(|_| invalid(flag))?;
            }
            Some(flag @ "--edge-factor") => {
                config.max_edge_factor = value.parse().map_err(|_| invalid(flag))?;
            }
            Some(flag @ "--mesh-size") => {
                config.mesh_size = value.parse().map_err(|_| invalid(flag))?;
            }
            _ => return Err(usage()),
        }
    }
    let destination = PathBuf::from(destination);
    match ops::surface_mesh_file(Path::new(source), &destination, config, &ops::never_stop) {
        Ok(stats) => Ok(Some(format!(
            "3D surface ready: {} source points, {} vertices, {} triangles -> {}",
            stats.source_points,
            stats.vertices,
            stats.triangles,
            destination.display()
        ))),
        Err(error) => Err((1, format!("3D surface failed: {error}"))),
    }
}

/// `--scans INPUT`: the scanner stations of a source, each with its axes
/// when the file holds them.
pub(crate) fn scans(arguments: &[OsString]) -> Outcome {
    let [source] = arguments else {
        return Err(usage());
    };
    let cloud = ops::open_source(Path::new(source), &ops::never_stop)
        .map_err(|error| (1, format!("Scan positions failed: {error}")))?;
    Ok(Some(station_lines(&ops::inspect(&cloud).stations)))
}

/// The lines `--scans` prints for these stations.
pub(crate) fn station_lines(stations: &[pointcloud_core::ScanPose]) -> String {
    let mut lines = vec![format!("{} scan position(s)", stations.len())];
    for pose in stations {
        lines.push(format!(
            "{}: {:.6}, {:.6}, {:.6}",
            pose.label, pose.position[0], pose.position[1], pose.position[2]
        ));
        if let Some(axes) = pose.axes {
            for (label, axis) in ["X", "Y", "Z"].into_iter().zip(axes) {
                lines.push(format!(
                    "  {label}: {:+.6}, {:+.6}, {:+.6}",
                    axis[0], axis[1], axis[2]
                ));
            }
        }
    }
    lines.join("\n")
}
