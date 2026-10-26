//! Opens a generated point cloud, reports what it holds and converts half of
//! it to LAZ, with the operations of `pointcloud_core::ops`.
//!
//!     cargo run --manifest-path native/examples/rust-consumer/Cargo.toml [FOLDER]
//!
//! FOLDER is where the two files are written, the system temp folder by
//! default.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use pointcloud_core::ops::{self, Convert};
use pointcloud_core::{Bounds, ExportFormat, LoadError, OrientedBox};

fn run() -> Result<String, LoadError> {
    let folder = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let source = folder.join("rust-consumer-grid.xyz");
    let destination = folder.join("rust-consumer-half.laz");

    // A grid of 10 × 10 × 10 points, 0.1 apart.
    let mut text = String::new();
    for i in 0..10 {
        for j in 0..10 {
            for k in 0..10 {
                let _ = writeln!(
                    text,
                    "{} {} {}",
                    i as f64 * 0.1,
                    j as f64 * 0.1,
                    k as f64 * 0.1
                );
            }
        }
    }
    std::fs::write(&source, text)?;

    let cloud = ops::open_source(&source, &ops::never_stop)?;
    let inspection = ops::inspect(&cloud);
    // The lower half along x.
    let half = OrientedBox::new(
        Bounds {
            min: [-1.0, -1.0, -1.0],
            max: [0.45, 2.0, 2.0],
        },
        0.0,
    );
    let written = ops::convert(
        &cloud,
        &destination,
        Convert {
            format: ExportFormat::Laz,
            section: Some(half),
            max_points: None,
        },
        &ops::never_stop,
    )?;
    if inspection.points != 1000 || written != 500 {
        return Err(LoadError::InvalidData(format!(
            "expected 1000 points and 500 written, found {} and {written}",
            inspection.points
        )));
    }
    Ok(format!(
        "{}: {} points from {:?} to {:?}; {written} written to {}",
        source.display(),
        inspection.points,
        inspection.bounds.min,
        inspection.bounds.max,
        destination.display()
    ))
}

fn main() -> ExitCode {
    match run() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("rust-consumer: {error}");
            ExitCode::FAILURE
        }
    }
}
