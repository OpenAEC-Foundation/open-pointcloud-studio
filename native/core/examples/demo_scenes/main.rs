//! Write the neutral demo scenes that the pictures of the repository are
//! made from. Every run writes the same files: all randomness is seeded.
//!
//! ```text
//! cargo run -p pointcloud-core --release --example demo_scenes -- OUTPUT_FOLDER
//!     [--only building|rooms] [--threads N]
//! ```
//!
//! - `building.laz`: a two-storey brick house with a pitched roof on a
//!   sloping plot, with a street, a path, a terrace, a hedge, fences, trees
//!   and shrubs. LAS 1.4, point format 7 (colour), millimetre coordinates in
//!   a local grid, with intensity and the classes ground, low, medium and
//!   high vegetation, building and unclassified.
//! - `rooms.e57`: two rooms joined by a doorway, with windows, a round
//!   column, a table with chairs and a cabinet, scanned from two stations
//!   ("Station 1" and "Station 2") with their poses, colour and intensity.
//!   Each station has an equirectangular panorama rendered from the same
//!   geometry and colours, so that the photos fit the points.
//! - `rooms.las`: the same points without photos, for tools that read LAS.
//!
//! At most eight threads are used unless `--threads` says otherwise.

mod building;
mod common;
mod rooms;

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;

use common::{splitmix, Rng, Sample, V3};

pub type Failure = Box<dyn std::error::Error + Send + Sync>;

/// A part of a scene that writes its points with its own random stream.
pub type Job<'a> = Box<dyn Fn(&mut Rng, &mut Vec<Sample>) + Send + Sync + 'a>;

/// Jobs that run at once before their points are written.
const WAVE: usize = 32;

fn main() -> Result<(), Failure> {
    let mut folder: Option<PathBuf> = None;
    let mut only: Option<String> = None;
    let mut threads = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .min(8);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--only" => only = Some(args.next().ok_or("--only needs building or rooms")?),
            "--threads" => {
                threads = args
                    .next()
                    .ok_or("--threads needs a number")?
                    .parse::<usize>()?
                    .max(1)
            }
            _ if folder.is_none() && !arg.starts_with("--") => folder = Some(arg.into()),
            _ => return Err(format!("unknown argument {arg}").into()),
        }
    }
    let folder =
        folder.ok_or("usage: demo_scenes OUTPUT_FOLDER [--only building|rooms] [--threads N]")?;
    std::fs::create_dir_all(&folder)?;
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()?;
    pool.install(|| -> Result<(), Failure> {
        if only.as_deref().is_none_or(|part| part == "building") {
            let started = Instant::now();
            let path = folder.join("building.laz");
            let points = building::write(&path)?;
            report(&path, points, started);
        }
        if only.as_deref().is_none_or(|part| part == "rooms") {
            let started = Instant::now();
            let written = rooms::write(&folder)?;
            for (path, points) in written {
                report(&path, points, started);
            }
        }
        Ok(())
    })
}

fn report(path: &Path, points: u64, started: Instant) {
    let bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    println!(
        "{}: {points} points, {:.1} MB, {:.1} s",
        path.display(),
        bytes as f64 / 1e6,
        started.elapsed().as_secs_f64()
    );
}

/// A LAS 1.4 writer of point format 7 (with colour) whose coordinates are
/// whole millimetres from `offset`.
pub fn las_writer(
    path: &Path,
    compressed: bool,
    offset: V3,
) -> Result<las::Writer<BufWriter<File>>, Failure> {
    let mut builder = las::Builder::from((1, 4));
    builder.point_format = las::point::Format::new(7)?;
    builder.point_format.is_compressed = compressed;
    builder.generating_software = "Open Pointcloud Studio".into();
    builder.system_identifier = "Generated demo scene".into();
    // No creation date, so that every run writes the same bytes.
    builder.date = None;
    let millimetres = |offset: f64| las::Transform {
        scale: 0.001,
        offset,
    };
    builder.transforms = las::Vector {
        x: millimetres(offset[0]),
        y: millimetres(offset[1]),
        z: millimetres(offset[2]),
    };
    let options = las::WriterOptions::default().with_laz_parallelism(las::LazParallelism::Yes);
    Ok(las::Writer::with_options(
        BufWriter::new(File::create(path)?),
        builder.into_header()?,
        options,
    )?)
}

/// The record of a sample in a LAS file. Colours are stored with 16 bits;
/// every channel keeps at least 2 of 255, so that no point reads as a file
/// of 8-bit colours.
pub fn las_point(sample: &Sample) -> Result<las::Point, Failure> {
    let channel = |value: f64| ((value.clamp(2.0 / 255.0, 1.0) * 255.0).round() as u16) * 257;
    let [red, green, blue] = sample.rgb.map(channel);
    Ok(las::Point {
        x: sample.xyz[0],
        y: sample.xyz[1],
        z: sample.xyz[2],
        intensity: (sample.intensity.clamp(0.0, 1.0) * 65535.0).round() as u16,
        return_number: 1,
        number_of_returns: 1,
        classification: las::point::Classification::new(sample.class)?,
        point_source_id: sample.source,
        gps_time: Some(0.0),
        color: Some(las::Color::new(red, green, blue)),
        ..las::Point::default()
    })
}

pub fn write_samples(
    writer: &mut las::Writer<BufWriter<File>>,
    samples: &[Sample],
) -> Result<(), Failure> {
    let points = samples
        .par_iter()
        .map(las_point)
        .collect::<Result<Vec<_>, _>>()?;
    writer.write_points(&points)?;
    Ok(())
}

/// Run the jobs a wave at a time and hand their points on in job order. A
/// job's stream depends only on `seed` and its place in the list, so the
/// order of the threads does not change the result.
pub fn run_jobs(
    jobs: &[Job],
    seed: u64,
    mut write: impl FnMut(&[Sample]) -> Result<(), Failure>,
) -> Result<u64, Failure> {
    let mut written = 0u64;
    for (wave, chunk) in jobs.chunks(WAVE).enumerate() {
        let outputs: Vec<Vec<Sample>> = chunk
            .par_iter()
            .enumerate()
            .map(|(index, job)| {
                let number = (wave * WAVE + index) as u64;
                let mut rng = Rng::new(seed ^ splitmix(number));
                let mut out = Vec::new();
                job(&mut rng, &mut out);
                out
            })
            .collect();
        for out in outputs {
            write(&out)?;
            written += out.len() as u64;
        }
    }
    Ok(written)
}
