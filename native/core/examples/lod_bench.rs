//! Time a viewport-sized sample from an existing disk octree.
//! Usage: cargo run -p pointcloud-core --example lod_bench -- INPUT.laz 80000

use std::path::Path;
use std::time::Instant;

use pointcloud_core::{open_las_header, IndexConfig, OctreeIndex};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(path), Some(limit), None) = (args.next(), args.next(), args.next()) else {
        return Err("usage: lod_bench INPUT.las|INPUT.laz POINT_LIMIT".into());
    };
    let limit = limit.to_string_lossy().parse::<usize>()?;
    let cloud = open_las_header(Path::new(&path))?;
    let index = OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())?
        .ok_or("no cached octree; run --index first")?;
    let started = Instant::now();
    let points = index.sample_lod_indexed(limit, |bounds| Some(bounds.extent() as f32))?;
    println!(
        "sampled {} of {} points in {:.3}s",
        points.len(),
        cloud.total_points,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
