//! Time the stages of the closed mesher on a scan, or on a generated
//! building, and report the memory it took.
//!
//! `cargo run -p pointcloud-core --example closed_mesh_bench -- scan.e57
//!     [--box X0 Y0 Z0 X1 Y1 Z1] [--voxel M] [--max-hole M] [--simplify MM]
//!     [--color-step N] [--tile VOXELS] [--threads N] [--limit TRIANGLES]
//!     [--out mesh.ply]`
//!
//! With `--synthetic MILLIONS` in place of a file, rooms of 4.0 x 3.0 x 2.6 m
//! with a point every centimetre and a millimetre of noise are written to a
//! temporary file, each with its own station; room number n has its corner
//! at x = 4.2 * (n mod 4), y = 3.2 * (n div 4).
//!
//! A scan is read through its cached index when it has one; otherwise an
//! index is built in a temporary folder first, which is timed apart.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use pointcloud_core::region_source::{RegionSource, SourceTransform};
use pointcloud_core::surfels::SurfelSource;
use pointcloud_core::{
    mesh_closed, open, open_las_header, write_mesh, Bounds, ClosedMeshConfig, ClosedMeshStage,
    IndexConfig, MeshFormat, OctreeIndex, PointCloud, ScanRange,
};

/// Counts the bytes in use, to report the most a job held at one time.
struct Counting;

static IN_USE: AtomicUsize = AtomicUsize::new(0);
static MOST: AtomicUsize = AtomicUsize::new(0);

fn grew(bytes: usize) {
    let now = IN_USE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    MOST.fetch_max(now, Ordering::Relaxed);
}

// Every call is handed to the system allocator unchanged; only the sizes
// are added up.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let block = System.alloc(layout);
        if !block.is_null() {
            grew(layout.size());
        }
        block
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        System.dealloc(block, layout);
        IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = System.realloc(block, layout, size);
        if !moved.is_null() {
            IN_USE.fetch_sub(layout.size(), Ordering::Relaxed);
            grew(size);
        }
        moved
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const ROOM: [f64; 3] = [4.0, 3.0, 2.6];
const ROOM_SPACING: f64 = 0.01;

/// The station of a scan and its first point in the file.
type Scan = ([f64; 3], u64);
type Failure = Box<dyn std::error::Error>;

/// Write rooms to a LAS file, scan after scan, and return the scans.
fn write_rooms(path: &Path, rooms: usize) -> Result<Vec<Scan>, Failure> {
    let mut builder = las::Builder::from((1, 2));
    builder.point_format = las::point::Format::new(2)?;
    let scale = las::Transform {
        scale: 0.0001,
        offset: 0.0,
    };
    builder.transforms = las::Vector {
        x: scale,
        y: scale,
        z: scale,
    };
    let mut writer = las::Writer::from_path(path, builder.into_header()?)?;
    let mut state = 0x2545_f491_4f6c_dd1du64;
    // Roughly normal: the sum of four even draws, a millimetre wide.
    let mut noise = move || {
        let mut sum = 0.0;
        for _ in 0..4 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            sum += (state >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
        }
        sum * 0.001 * 3f64.sqrt()
    };
    let mut scans = Vec::new();
    let mut written = 0u64;
    for room in 0..rooms {
        let corner = [4.2 * (room % 4) as f64, 3.2 * (room / 4) as f64, 0.0];
        scans.push(([corner[0] + 1.3, corner[1] + 1.1, corner[2] + 1.2], written));
        // The six faces: the axis across each, and where it lies.
        for across in 0..3 {
            let (u, v) = ((across + 1) % 3, (across + 2) % 3);
            let steps = |axis: usize| (ROOM[axis] / ROOM_SPACING).round() as usize;
            for (side, at) in [0.0, ROOM[across]].into_iter().enumerate() {
                for a in 0..steps(u) {
                    for b in 0..steps(v) {
                        let mut xyz = corner;
                        xyz[across] += at + noise();
                        xyz[u] += (a as f64 + 0.5) * ROOM_SPACING;
                        xyz[v] += (b as f64 + 0.5) * ROOM_SPACING;
                        // A colour per face, with a darker band on it.
                        let shade = if (b / 40) % 4 == 0 { 30_000 } else { 52_000 };
                        let mut color = [shade; 3];
                        color[across] = if side == 0 { 20_000 } else { 60_000 };
                        writer.write_point(las::Point {
                            x: xyz[0],
                            y: xyz[1],
                            z: xyz[2],
                            color: Some(las::Color::new(color[0], color[1], color[2])),
                            ..las::Point::default()
                        })?;
                        written += 1;
                    }
                }
            }
        }
    }
    writer.close()?;
    Ok(scans)
}

fn megabytes(bytes: usize) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn main() -> Result<(), Failure> {
    let mut input: Option<PathBuf> = None;
    let mut synthetic: Option<f64> = None;
    let mut region: Option<Bounds> = None;
    let mut output: Option<PathBuf> = None;
    let mut config = ClosedMeshConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut number = |name: &str| -> Result<f64, Failure> {
            Ok(args
                .next()
                .ok_or_else(|| format!("{name} needs a number"))?
                .parse()?)
        };
        match arg.as_str() {
            "--synthetic" => synthetic = Some(number("--synthetic")?),
            "--box" => {
                let mut corners = [0.0; 6];
                for corner in &mut corners {
                    *corner = number("--box")?;
                }
                region = Some(Bounds {
                    min: [corners[0], corners[1], corners[2]],
                    max: [corners[3], corners[4], corners[5]],
                });
            }
            "--voxel" => config.voxel = Some(number("--voxel")?),
            "--max-hole" => config.max_hole = number("--max-hole")?,
            "--simplify" => config.simplify_tolerance = Some(number("--simplify")? / 1000.0),
            "--color-step" => config.color_step = number("--color-step")? as u8,
            "--tile" => config.tile_voxels = number("--tile")? as u32,
            "--threads" => config.threads = number("--threads")? as usize,
            "--limit" => {
                config.max_triangles = number("--limit")? as usize;
                config.max_vertices = config.max_triangles;
            }
            "--out" => output = Some(args.next().ok_or("--out needs a path")?.into()),
            _ if input.is_none() && !arg.starts_with("--") => input = Some(arg.into()),
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }

    // The generated file and its index live here and go with it.
    let scratch = tempfile::tempdir()?;
    let mut scans = Vec::new();
    let path = match (input, synthetic) {
        (Some(path), None) => path,
        (None, Some(millions)) => {
            let per_room: f64 = 2.0 * (ROOM[0] * ROOM[1] + ROOM[1] * ROOM[2] + ROOM[0] * ROOM[2])
                / (ROOM_SPACING * ROOM_SPACING);
            let rooms = (millions * 1e6 / per_room).ceil().max(1.0) as usize;
            let path = scratch.path().join("rooms.las");
            let started = Instant::now();
            scans = write_rooms(&path, rooms)?;
            println!(
                "wrote {rooms} rooms in {:.1}s",
                started.elapsed().as_secs_f64()
            );
            path
        }
        _ => return Err("expected a point cloud or --synthetic MILLIONS, not both".into()),
    };
    let lower = path.to_string_lossy().to_ascii_lowercase();
    let cloud: PointCloud = if lower.ends_with(".las") || lower.ends_with(".laz") {
        open_las_header(&path)?
    } else {
        open(&path, 1)?
    };
    let started = Instant::now();
    let index = match OctreeIndex::open_cached_if_present(&cloud, IndexConfig::default())? {
        Some(index) => {
            println!("cached index opened");
            index
        }
        None => {
            let index = OctreeIndex::build(
                &cloud,
                IndexConfig {
                    scratch_dir: Some(scratch.path().to_path_buf()),
                    ..IndexConfig::default()
                },
            )?;
            println!("index built in {:.1}s", started.elapsed().as_secs_f64());
            index
        }
    };
    let leaves = index.intersecting_leaves(|_| true);
    println!(
        "{} points in {} leaves, {:.1} MB in use before the job",
        cloud.total_points,
        leaves.len(),
        megabytes(IN_USE.load(Ordering::Relaxed))
    );
    drop(leaves);

    let points = RegionSource::new(&cloud, Some(&index), SourceTransform::default());
    let source = if scans.is_empty() {
        if !cloud.scan_poses.is_empty() && !cloud.scan_ranges_known() {
            println!("the cache does not tell which scan holds a point: stations are not used");
        }
        SurfelSource::of_cloud(points, &cloud)
    } else {
        SurfelSource::with_stations(
            points,
            scans.iter().map(|scan| scan.0).collect(),
            scans
                .iter()
                .enumerate()
                .map(|(station, scan)| ScanRange {
                    first_ordinal: scan.1,
                    station: Some(station as u32),
                })
                .collect(),
        )
    };
    println!("{} stations", source.stations().len());

    let before = IN_USE.load(Ordering::Relaxed);
    MOST.store(before, Ordering::Relaxed);
    let started = Instant::now();
    let name = |stage: ClosedMeshStage| match stage {
        ClosedMeshStage::Planning => "planning",
        ClosedMeshStage::Reconstructing => "tiles",
        ClosedMeshStage::Simplifying => "seams",
        ClosedMeshStage::Measuring => "measuring",
    };
    let mut stage = None;
    let mut stage_started = started;
    // The longest the job went without asking whether to go on: how long a
    // cancel can take to be heard.
    let mut asked = started;
    let mut silence = (0.0, ClosedMeshStage::Planning);
    let (mesh, report) = mesh_closed(
        &[source],
        region,
        &|_, _, _| true,
        &config,
        &mut |progress| {
            let waited = asked.elapsed().as_secs_f64();
            asked = Instant::now();
            if waited > silence.0 {
                silence = (waited, stage.unwrap_or(progress.stage));
            }
            if stage != Some(progress.stage) {
                if let Some(stage) = stage {
                    println!(
                        "{}: {:.2}s",
                        name(stage),
                        stage_started.elapsed().as_secs_f64()
                    );
                }
                stage = Some(progress.stage);
                stage_started = Instant::now();
            }
            Ok(())
        },
    )?;
    let elapsed = started.elapsed();
    println!(
        "longest time without a progress call: {:.2}s, after one of {}",
        silence.0,
        name(silence.1)
    );
    let most = MOST.load(Ordering::Relaxed);
    let times = report.timings;
    println!(
        "total {:.2}s; most memory {:.0} MB above the {:.0} MB before, the mesh itself {:.0} MB",
        elapsed.as_secs_f64(),
        megabytes(most - before),
        megabytes(before),
        megabytes(mesh.vertices.len() * (24 + 12 + 3) + mesh.triangles.len() * 12),
    );
    println!(
        "voxel {} m, holes to {} m, simplified within {:.1} mm, {} tiles of {} planned, \
         {} threads, at most {} tiles in work at a time",
        report.voxel,
        report.max_hole,
        report.simplify_tolerance * 1000.0,
        report.tiles,
        report.tiles_planned,
        report.threads,
        report.tiles_side_by_side
    );
    println!(
        "{} points in the region, {} read ({:.1} times), {} elements, sides from {:?} ({} by \
         station, {} by default)",
        report.points,
        report.points_read,
        report.points_read as f64 / report.points.max(1) as f64,
        report.surfels,
        report.orientation,
        report.surfels_by_station,
        report.surfels_by_default
    );
    println!(
        "wall time: planning {:.2}s, tiles {:.2}s, joining {:.2}s, seams {:.2}s, measuring {:.2}s",
        times.planning.as_secs_f64(),
        times.tiles.as_secs_f64(),
        times.joining.as_secs_f64(),
        times.seam_simplifying.as_secs_f64(),
        times.measuring.as_secs_f64()
    );
    println!(
        "inside the tiles, summed over threads: reading {:.2}s, elements {:.2}s, field {:.2}s, \
         surface {:.2}s, simplifying {:.2}s",
        times.tile_reading.as_secs_f64(),
        times.tile_elements.as_secs_f64(),
        times.tile_field.as_secs_f64(),
        times.tile_surface.as_secs_f64(),
        times.tile_simplifying.as_secs_f64()
    );
    println!(
        "{} vertices, {} triangles ({} before simplification)",
        report.vertices, report.triangles, report.triangles_extracted
    );
    println!(
        "{} open edges, {} edges with more than two triangles, {} pieces, {} seam faults",
        report.topology.open_edges,
        report.topology.non_manifold_edges,
        report.topology.components,
        report.seam_faults
    );
    println!(
        "points to mesh: mean {:.2} mm, 95% within {:.2} mm, largest {:.1} mm, over {} points",
        report.deviation.mean * 1000.0,
        report.deviation.p95 * 1000.0,
        report.deviation.max * 1000.0,
        report.deviation.samples
    );
    if let Some(output) = output {
        let format =
            MeshFormat::from_path(&output).ok_or("the output must be .obj, .ply or .stl")?;
        let started = Instant::now();
        write_mesh(&mesh, &output, format, &[])?;
        println!(
            "{} written in {:.2}s",
            format.label(),
            started.elapsed().as_secs_f64()
        );
    }
    Ok(())
}
