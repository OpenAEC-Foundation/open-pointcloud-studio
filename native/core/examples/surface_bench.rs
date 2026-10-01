//! Profile the full-source 3D surface pipeline without changing its output.
//!
//! `cargo run -p pointcloud-core --example surface_bench -- scan.laz mesh.obj`

use std::env;
use std::time::Instant;

use pointcloud_core::{
    mesh_surface_obj_where_progress, open, open_las_header, MeshProgress, MeshStage,
    SurfaceMeshConfig,
};

fn phase(progress: MeshProgress, vertices: u64) -> &'static str {
    match progress.stage {
        MeshStage::Reading => "read and sample",
        MeshStage::Reconstructing if progress.total == 0 => "spatial thinning",
        MeshStage::Reconstructing if progress.completed < vertices => "neighbor search",
        MeshStage::Reconstructing if progress.completed < vertices * 2 => "normal estimation",
        MeshStage::Reconstructing => "triangulation and orientation",
        MeshStage::Writing => "OBJ writing",
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let source = args.next().ok_or("expected point-cloud path")?;
    let output = args.next().ok_or("expected OBJ path")?;
    let cloud = if source.to_ascii_lowercase().ends_with(".las")
        || source.to_ascii_lowercase().ends_with(".laz")
    {
        open_las_header(&source)?
    } else {
        open(&source, 1)?
    };
    let started = Instant::now();
    let mut phase_started = started;
    let mut current = "";
    let stats = mesh_surface_obj_where_progress(
        &cloud,
        output,
        SurfaceMeshConfig::default(),
        |_, _| true,
        |progress| {
            let vertices = progress.total / 3;
            let next = phase(progress, vertices);
            if next != current {
                let now = Instant::now();
                if !current.is_empty() {
                    println!(
                        "{current}: {:.3}s",
                        now.duration_since(phase_started).as_secs_f64()
                    );
                }
                current = next;
                phase_started = now;
            }
            Ok(())
        },
    )?;
    println!(
        "{current}: {:.3}s",
        Instant::now().duration_since(phase_started).as_secs_f64()
    );
    println!(
        "total: {:.3}s; {} source points, {} vertices, {} triangles",
        started.elapsed().as_secs_f64(),
        stats.source_points,
        stats.vertices,
        stats.triangles,
    );
    Ok(())
}
