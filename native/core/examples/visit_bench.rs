//! Time the shared full-source reader on a bounded prefix of a scan.
//!
//! `cargo run -p pointcloud-core --example visit_bench -- scan.laz 2000000`

use std::env;
use std::time::Instant;

use pointcloud_core::{visit_points, LoadError};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args.next().ok_or("expected point-cloud path")?;
    let limit: u64 = args.next().ok_or("expected point limit")?.parse()?;
    if limit == 0 {
        return Err("point limit must be positive".into());
    }
    let started = Instant::now();
    let mut points = 0u64;
    let result = visit_points(path, &mut |_| {
        points += 1;
        if points >= limit {
            Err(LoadError::Cancelled)
        } else {
            Ok(())
        }
    });
    if !matches!(result, Ok(()) | Err(LoadError::Cancelled)) {
        result?;
    }
    println!("{points} points in {:.3}s", started.elapsed().as_secs_f64());
    Ok(())
}
