//! A closed triangle mesh from scan points: a surface without overlaps
//! that is closed wherever the scan has data or a gap narrower than the
//! hole limit.
//!
//! The points are reduced to oriented surface elements (`surfels`). Around
//! them a signed distance to the surface is known at the corners of a voxel
//! lattice: positive on the side the scanner saw, negative behind the
//! surface. It is a weighted mean of the distances to the planes of the
//! elements nearby, so scanner noise averages out and nothing but the
//! elements within a fixed reach matters. Where the fine elements end, the
//! coarser levels tell how the surface runs on, which is what closes gaps.
//! The surface is taken out by dual contouring: one vertex in every voxel
//! the surface passes through and one quad across every lattice edge whose
//! ends lie on different sides.
//!
//! The work is cut into tiles that are meshed side by side. A tile reads a
//! margin around itself, wide enough that the values on its border equal
//! those its neighbour finds, bit for bit; vertices are named after their
//! voxel, and the pieces join by those names without a seam.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use crate::local_fit::{cross, difference, dot as dot3, symmetric_eigen3};
use crate::mesh_quality::{
    mesh_deviation_progress, mesh_topology_progress, open_boundary_vertices_progress,
    MeshDeviation, MeshTopology,
};
use crate::mesh_simplify::{simplify_mesh, simplify_mesh_progress};
use crate::obj_mesh::MeshGeometry;
use crate::region_source::{overlaps, world_bounds, RegionFilter, RegionReader, RegionSource};
use crate::surfels::{
    collect_surfels, plan_tiles, Lattice, SurfelFallback, SurfelLevel, SurfelOrientation,
    SurfelSource, TileSurfelStats, TileSurfels, DEFAULT_TILE_VOXELS, GAP_REACH, MAX_HOLE_VOXELS,
    MAX_VOXEL, MIN_VOXEL,
};
use crate::{Bounds, IndexedPoint, LoadError};

/// The widest hole limit a job takes, in metres.
pub const MAX_CLOSED_MESH_HOLE: f64 = 3.2;
/// Vertices and triangles of a mesh that every mesh writer and the viewer
/// take: a job stops above them unless it is given limits of its own.
pub const DEFAULT_CLOSED_MESH_VERTICES: usize = crate::obj_mesh::MAX_VERTICES;
pub const DEFAULT_CLOSED_MESH_TRIANGLES: usize = crate::obj_mesh::MAX_TRIANGLES;
/// Above this many triangles the whole mesh is not simplified once more
/// after the tiles are joined: that pass runs on one thread.
const SEAM_PASS_TRIANGLES: usize = 4_000_000;
/// Share of the voxel that simplification may move the surface by when the
/// caller leaves the tolerance open.
const AUTOMATIC_TOLERANCE: f64 = 0.15;
/// A corner this many voxels from an element is near data: far enough for
/// every voxel the surface passes through to have all its corners, and no
/// further, because the further it is, the further the surface runs on
/// past the last points. With this it is about a voxel.
const SUPPORT: f32 = 2.2;
/// Distances for closing gaps are measured from the lattice corner nearest
/// to an element, which lies up to this far from the element itself.
const SEED_SLACK: f64 = 1.0;
/// Beyond half the hole limit from the data, the surface running on marks
/// the open side of an edge; it is looked for in a shell this many voxels
/// thick. The coarsest elements must reach that far, with a little to
/// spare.
const SHELL: f64 = 2.0;
const _: () = assert!(SEED_SLACK + SHELL + 1.5 <= GAP_REACH);
/// Across a gap the surface is only looked for this close, in voxels, to
/// where the elements around the gap say it runs.
const SHEET: f32 = 2.0;
/// Across a gap the coarser levels say where the surface runs, as one mean
/// over all their elements in reach. Where those lie on more than one face
/// that mean is the plane of none of them: the normals of one face add up
/// to their weight, those of two faces to less. A gap corner whose normals
/// add up to a smaller share of their weight than this is not closed over.
const ONE_FACE: f32 = 0.9;
/// The lattice edges leave a vertex this free to move away from the middle
/// of its crossings, and directions the crossings say less about than this
/// share of the best one are left alone.
const VERTEX_PULL: f64 = 0.05;
const VERTEX_RANK: f64 = 0.1;
/// A piece of surface smaller than this many voxel faces is dust: stray
/// points that happened to line up. One stray point just inside the reach
/// of a fit at level 1 from a real face finds a plane there and stands for
/// the corners `SUPPORT * 2` voxels around itself, a sheet of up to 61
/// voxel faces; the limit lies above that.
const DUST_FACES: f64 = 64.0;
const _: () =
    assert!(std::f64::consts::PI * (2.0 * SUPPORT as f64) * (2.0 * SUPPORT as f64) <= DUST_FACES);
/// Memory all tiles in work may take together, by what they are expected to
/// need: a tile waits while the others leave no room for it.
const TILE_MEMORY: u64 = 2 << 30;
/// What a tile needs per surface element, with all that is derived from it:
/// measured at about 100 bytes on generated rooms. Before its points are
/// read, a tile is expected to hold no more elements than twenty sheets of
/// surface through the whole block. That is what a block full of clutter
/// comes to, measured at two million elements for two million stray points
/// in 27 m3; a dense scan has many more points than elements. Once the
/// elements are counted the tile gives back what it does not need.
const ELEMENT_BYTES: u64 = 100;
const BLOCK_SHEETS: u64 = 20;
/// The total of the measuring stage as the progress callback gets it.
const MEASURING_STEPS: u64 = 2_000;
const FAR: i32 = 1 << 28;
const NO_LEVEL: u8 = u8::MAX;
const NO_VERTEX: u32 = u32::MAX;

/// Which side of the surface the triangles face where no station tells.
/// With stations the front of a triangle is the side the scanner saw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeshOrientation {
    /// The side of the middle of the region: right for a room in its
    /// section box, wrong for an object seen from around. The region is
    /// the section box cut back to where the layers have points.
    Automatic,
    /// The side of this point in the scene. A point cannot tell the side of
    /// a plane it lies in, or nearly: one that passes it within 8 % of the
    /// distance from the point to the farthest corner of the region, that
    /// distance counted up to 10 m, and within two voxels at the least.
    /// Such a plane faces up, and one fixed direction when it is upright,
    /// so a single floor or facade still comes out as one sheet. The
    /// report counts the elements this applied to in `surfels_by_default`.
    /// Two cases remain in which a face can come out torn, its elements
    /// told apart by their noise: a face that passes the point at just
    /// that distance, on the side its default does not face; and an upright
    /// face whose normal points within about a degree and a half of 118 or
    /// 298 degrees from the x axis, where the fixed direction lies in the
    /// face.
    Towards([f64; 3]),
    /// The upward side, for data measured from above. Upright faces take
    /// one fixed direction, the same for both sides of a wall, so the
    /// walls of a room do not meet its floor in a closed edge; the last
    /// case under `Towards` applies to them.
    Upward,
}

/// The settings of a closed mesh job.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosedMeshConfig {
    /// Edge of a voxel in metres, between 0.005 and 0.5. Detail smaller
    /// than about two voxels is lost. A wall with openings needs about
    /// three voxels of thickness: at two voxels it gets extra holes and
    /// bulges of a voxel and a half at the reveals. Two faces closer than
    /// two voxels are not kept apart: the result there has holes through
    /// the sheet and can have edges with more than two triangles. `None`
    /// picks 0.02 m for a region up to 20 m long, 0.03 m up to 60 m and
    /// 0.05 m beyond.
    pub voxel: Option<f64>,
    /// Gaps in the data up to this wide, in metres, are closed; 0 closes
    /// none. At most 3.2 m, and never more than 32 voxels. A gap is closed
    /// only where a single face runs around it: one along the foot of a
    /// wall, in a face with another face less than about seven voxels
    /// behind it, or in a surface curved more tightly than a radius of
    /// about ten voxels stays open.
    pub max_hole: f64,
    /// How far simplification may move the surface, in metres. `None` is
    /// 0.15 voxel and 0 switches simplification off.
    pub simplify_tolerance: Option<f64>,
    /// Simplification leaves a vertex alone where the colour differs by
    /// more than this, in any channel from 0 to 255, from the vertex next
    /// to it: with 24 the outline of a door in a wall stays and the wall
    /// itself becomes a few large triangles. 255, the default, simplifies
    /// whatever the colour. Keeping colour edges costs triangles wherever
    /// the surface has a pattern: measured with voxels of 2 cm and a step
    /// of 24, a pattern finer than 10 cm keeps 3,000 to 3,600 triangles
    /// per square metre, six or seven of every ten extracted, where the
    /// shape alone needs about 17.
    pub color_step: u8,
    /// Face the surface towards the station that measured it, where the
    /// sources know their stations.
    pub use_stations: bool,
    /// The side for points without a known station.
    pub orientation: MeshOrientation,
    /// A job that would give more vertices or triangles than this stops
    /// with an error.
    pub max_vertices: usize,
    pub max_triangles: usize,
    /// Points kept for measuring how far the scan lies from the mesh.
    pub deviation_samples: usize,
    /// Voxels along a tile; 0 takes the default of 96.
    pub tile_voxels: u32,
    /// Worker threads, each meshing a tile; 0 takes as many as there are
    /// processors. Fewer tiles are in work at a time when together they
    /// are expected to need about 2 GB.
    pub threads: usize,
}

impl Default for ClosedMeshConfig {
    fn default() -> Self {
        Self {
            voxel: None,
            max_hole: 0.25,
            simplify_tolerance: None,
            color_step: u8::MAX,
            use_stations: true,
            orientation: MeshOrientation::Automatic,
            max_vertices: DEFAULT_CLOSED_MESH_VERTICES,
            max_triangles: DEFAULT_CLOSED_MESH_TRIANGLES,
            deviation_samples: 200_000,
            tile_voxels: 0,
            threads: 0,
        }
    }
}

impl ClosedMeshConfig {
    /// Whether every setting lies in its range; the error says which does
    /// not.
    pub fn validate(&self) -> Result<(), LoadError> {
        let invalid = |reason: &str| Err(LoadError::InvalidData(reason.into()));
        if self
            .voxel
            .is_some_and(|voxel| !(MIN_VOXEL..=MAX_VOXEL).contains(&voxel))
        {
            return invalid("the voxel size must lie between 0.005 and 0.5 m");
        }
        if !(0.0..=MAX_CLOSED_MESH_HOLE).contains(&self.max_hole) {
            return invalid("the hole limit must lie between 0 and 3.2 m");
        }
        if self
            .simplify_tolerance
            .is_some_and(|tolerance| !(0.0..=1.0).contains(&tolerance))
        {
            return invalid("the simplification tolerance must lie between 0 and 1 m");
        }
        if let MeshOrientation::Towards(point) = self.orientation {
            if !point.iter().all(|value| value.is_finite()) {
                return invalid("the point the surface faces is not a position");
            }
        }
        if self.max_vertices < 3
            || self.max_triangles < 1
            || self.max_vertices >= u32::MAX as usize
            || self.max_triangles >= u32::MAX as usize
        {
            return invalid("the vertex and triangle limits are out of range");
        }
        if self.deviation_samples > 5_000_000 {
            return invalid("at most 5,000,000 points can be kept for the deviation");
        }
        if self.tile_voxels != 0 && !(16..=256).contains(&self.tile_voxels) {
            return invalid("a tile must be 16 to 256 voxels long");
        }
        Ok(())
    }

    /// The voxel used for a region.
    pub fn voxel_for(&self, region: Bounds) -> f64 {
        self.voxel.unwrap_or(match region.extent() {
            extent if extent <= 20.0 => 0.02,
            extent if extent <= 60.0 => 0.03,
            _ => 0.05,
        })
    }
}

/// What a closed mesh job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosedMeshStage {
    /// Finding the tiles that hold points; no total is known yet.
    Planning,
    /// Meshing tiles: `completed` of `total` tiles. Putting the tiles
    /// together afterwards still counts as this stage, with all tiles done.
    Reconstructing,
    /// Simplifying across the seams of the tiles; the total is an estimate
    /// that grows.
    Simplifying,
    /// Measuring the result: counting its edges and then the distances of
    /// the sampled points to it, each half of the `total`.
    Measuring,
}

/// How far a closed mesh job is within its stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClosedMeshProgress {
    pub stage: ClosedMeshStage,
    pub completed: u64,
    pub total: u64,
}

/// Where the sides of the surface came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationUsed {
    /// Every element took its side from the station that measured it.
    Stations,
    /// No station was known or used: the fallback decided everywhere.
    Fallback,
    /// Stations where they were known, the fallback elsewhere.
    Mixed,
}

/// Time spent per step. The steps inside tiles are summed over all tiles
/// and threads; the others are wall time.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClosedMeshTimings {
    pub planning: Duration,
    /// Wall time of all tiles together.
    pub tiles: Duration,
    pub tile_reading: Duration,
    pub tile_elements: Duration,
    pub tile_field: Duration,
    pub tile_surface: Duration,
    pub tile_simplifying: Duration,
    pub joining: Duration,
    pub seam_simplifying: Duration,
    pub measuring: Duration,
    pub total: Duration,
}

/// What a closed mesh job did, and how good its result is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosedMeshReport {
    /// Voxel edge used, in metres.
    pub voxel: f64,
    /// Widest gap closed, in metres: the limit asked for, cut back to 32
    /// voxels.
    pub max_hole: f64,
    /// Simplification tolerance used, in metres; 0 when it was off.
    pub simplify_tolerance: f64,
    /// The box the job covered: the one asked for, cut back to where the
    /// layers have points.
    pub region: Bounds,
    /// Tiles that were read, and those among them that gave triangles.
    pub tiles_planned: u32,
    pub tiles: u32,
    /// Worker threads of the job.
    pub threads: usize,
    /// The most tiles that were in work at the same time: as many as there
    /// are threads, or fewer when the memory they were expected to need
    /// made tiles wait.
    pub tiles_side_by_side: usize,
    /// Accepted points in the region.
    pub points: u64,
    /// Points read from the sources: more than the region holds, because a
    /// tile also reads a margin around itself.
    pub points_read: u64,
    /// Surface elements with a plane fit of their own.
    pub surfels: u64,
    pub orientation: OrientationUsed,
    /// Elements whose points had a station to take a side from.
    pub surfels_by_station: u64,
    /// Elements whose side nothing told: without a station, and seen edge
    /// on by the fallback. They face up, or one fixed direction when
    /// upright, which keeps a face whole but may be the wrong side. When
    /// this is a large share of `surfels`, a point for the surface to face
    /// (`MeshOrientation::Towards`) is worth giving.
    pub surfels_by_default: u64,
    pub vertices: usize,
    pub triangles: usize,
    /// Triangles before any simplification.
    pub triangles_extracted: u64,
    /// Whether the joined mesh was simplified across the tile seams.
    pub seams_simplified: bool,
    /// Triangles left out because the tile that owns one of their vertices
    /// did not have it. Tiles agree on what lies between them, so this is
    /// zero; anything else is a fault to report.
    pub seam_faults: u64,
    /// Open edges, edges with more than two triangles, pieces and the
    /// Euler characteristic of the result. Edges with more than two
    /// triangles are none, except where two faces lie closer together than
    /// two voxels or a face without stations came out torn.
    pub topology: MeshTopology,
    /// Distance from the sampled points to the result, in metres.
    pub deviation: MeshDeviation,
    pub timings: ClosedMeshTimings,
}

/// Reconstruct the surface of the points of several layers inside a region.
///
/// - `sources` are the layers with their stations. Every layer needs an
///   index or must be held in memory (see `SurfelSource`).
/// - `region` is the box to mesh, in scene coordinates; `None` takes all
///   the layers hold. Time and memory follow the surface inside this box,
///   not the size of the layers. The box is cut back to where the layers
///   have points before anything is derived from it, so one drawn wide
///   around the data does no harm.
/// - `accept` leaves points out: it gets the position of a source in
///   `sources`, the ordinal of a point in its source and the point in scene
///   coordinates. It is called from several threads. It does not narrow
///   the work: which tiles are read, the automatic voxel and the middle
///   that `MeshOrientation::Automatic` faces all follow `region`. A job
///   that meshes a part of the layers, such as a selection or one class,
///   must pass the box around that part as `region`, cut to the section
///   box when there is one; `None` is only right when all of the layers is
///   meshed. Otherwise every tile of the layers is read to find nearly
///   nothing in it, and the surface faces the middle of the wrong box.
/// - `progress` is called on the calling thread, at least ten times a
///   second while tiles are meshed and while the result is measured.
///   Returning an error, such as `LoadError::Cancelled`, stops the job and
///   is returned.
///
/// The mesh is in scene coordinates, with a normal per vertex on the side
/// the surface was seen from and colours when the points have them. The
/// same input gives the same mesh on every run and with every number of
/// threads. A region without points, or without enough points for any
/// surface, is an error.
pub fn mesh_closed(
    sources: &[SurfelSource<'_>],
    region: Option<Bounds>,
    accept: &RegionFilter<'_>,
    config: &ClosedMeshConfig,
    progress: &mut dyn FnMut(ClosedMeshProgress) -> Result<(), LoadError>,
) -> Result<(MeshGeometry, ClosedMeshReport), LoadError> {
    let started = Instant::now();
    config.validate()?;
    if sources
        .iter()
        .any(|source| matches!(source.points.reader, RegionReader::Stream(_)))
    {
        return Err(LoadError::InvalidData(
            "a layer without an index must be read into memory before it is meshed".into(),
        ));
    }
    let layers: Vec<RegionSource<'_>> = sources.iter().map(|source| source.points).collect();
    let empty = || LoadError::InvalidData("the region holds no points to mesh".into());
    let data = world_bounds(&layers).ok_or_else(empty)?;
    // The voxel, the lattice and the middle the surface faces follow from
    // the region. A section box drawn wide around the data must not pick a
    // coarser voxel, a lattice too long for its keys or a middle outside
    // the room, so only the part of it that can hold points counts.
    let region = match region {
        Some(region) => {
            // Checked here because cutting the box back would hide a NaN.
            if !(0..3).all(|axis| region.min[axis] <= region.max[axis]) {
                return Err(LoadError::InvalidData(
                    "the region is not a box with its minimum below its maximum".into(),
                ));
            }
            let cut = Bounds {
                min: std::array::from_fn(|axis| region.min[axis].max(data.min[axis])),
                max: std::array::from_fn(|axis| region.max[axis].min(data.max[axis])),
            };
            if (0..3).any(|axis| cut.min[axis] > cut.max[axis]) {
                return Err(empty());
            }
            cut
        }
        None => data,
    };
    let finite = |bounds: Bounds| bounds.min.iter().chain(&bounds.max).all(|v| v.is_finite());
    if !finite(region) && !finite(data) {
        return Err(LoadError::InvalidData(
            "a layer holds a point without a finite position: use a section box".into(),
        ));
    }
    let voxel = config.voxel_for(region);
    let lattice = Lattice::new(
        region,
        voxel,
        config.max_hole,
        if config.tile_voxels == 0 {
            DEFAULT_TILE_VOXELS
        } else {
            config.tile_voxels
        },
    )?;
    let tolerance = config
        .simplify_tolerance
        .unwrap_or(AUTOMATIC_TOLERANCE * voxel);
    let stage = |stage, completed, total| ClosedMeshProgress {
        stage,
        completed,
        total,
    };
    progress(stage(ClosedMeshStage::Planning, 0, 0))?;
    let tiles = plan_tiles(&lattice, sources, accept, &mut || {
        progress(stage(ClosedMeshStage::Planning, 0, 0))
    })?;
    let mut timings = ClosedMeshTimings {
        planning: started.elapsed(),
        ..ClosedMeshTimings::default()
    };
    if tiles.is_empty() {
        return Err(empty());
    }

    let threads = if config.threads == 0 {
        rayon::current_num_threads()
    } else {
        config.threads
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .map_err(|error| LoadError::InvalidData(format!("no worker threads: {error}")))?;
    let job = Job {
        lattice: &lattice,
        sources,
        accept,
        orientation: SurfelOrientation {
            stations: config.use_stations,
            fallback: match config.orientation {
                MeshOrientation::Automatic => SurfelFallback::Towards(region.center()),
                MeshOrientation::Towards(point) => SurfelFallback::Towards(point),
                MeshOrientation::Upward => SurfelFallback::Upward,
            },
        },
        tolerance,
        color_step: config.color_step,
        samples: config.deviation_samples,
        stop: AtomicBool::new(false),
        sample_limit: AtomicU64::new(u64::MAX),
        in_work: Mutex::new(InWork::default()),
        freed: Condvar::new(),
    };

    // The tiles run on the pool while this thread reports progress and
    // listens for a cancel: the callback need not be shared with threads.
    let tiles_started = Instant::now();
    let total = tiles.len() as u64;
    let mut pieces: Vec<Option<TilePiece>> = Vec::new();
    pieces.resize_with(tiles.len(), || None);
    let mut failure: Option<LoadError> = None;
    let mut samples: Vec<(u64, [f64; 3])> = Vec::new();
    let mut counts = (0usize, 0usize);
    let mut done = 0u64;
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        let (job, pool, tiles) = (&job, &pool, &tiles);
        scope.spawn(move || {
            pool.install(|| {
                tiles
                    .par_iter()
                    .enumerate()
                    .for_each_with(sender, |sender, (slot, tile)| {
                        if !job.stop.load(Ordering::Relaxed) {
                            // The receiver only goes away with this scope.
                            let _ = sender.send((slot, mesh_tile(job, *tile)));
                        }
                    });
            });
        });
        loop {
            let mut error = None;
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok((slot, Ok(mut piece))) => {
                    done += 1;
                    counts.0 += piece.keys.len();
                    counts.1 += piece.triangles.len();
                    if counts.0 > config.max_vertices || counts.1 > config.max_triangles {
                        error = Some(too_large(config));
                    }
                    samples.append(&mut piece.samples);
                    if samples.len() > 2 * config.deviation_samples.max(1) {
                        let limit = keep_lowest(&mut samples, config.deviation_samples);
                        job.sample_limit.store(limit, Ordering::Relaxed);
                    }
                    pieces[slot] = Some(piece);
                }
                // The tiles that stop because of the first failure report
                // a cancel of their own, which must not take its place.
                Ok((_, Err(failed))) => error = Some(failed),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if error.is_none() && failure.is_none() {
                error = progress(stage(ClosedMeshStage::Reconstructing, done, total)).err();
            }
            if let Some(error) = error {
                job.stop.store(true, Ordering::Relaxed);
                failure.get_or_insert(error);
            }
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    timings.tiles = tiles_started.elapsed();
    keep_lowest(&mut samples, config.deviation_samples);
    samples.sort_unstable_by_key(|sample| sample.0);

    let joining = Instant::now();
    let pieces: Vec<TilePiece> = pieces.into_iter().flatten().collect();
    let mut report = ClosedMeshReport {
        voxel,
        max_hole: config.max_hole.min(MAX_HOLE_VOXELS * voxel),
        simplify_tolerance: tolerance,
        region,
        tiles_planned: tiles.len() as u32,
        tiles: 0,
        threads,
        tiles_side_by_side: job
            .in_work
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .most,
        points: 0,
        points_read: 0,
        surfels: 0,
        orientation: OrientationUsed::Fallback,
        surfels_by_station: 0,
        surfels_by_default: 0,
        vertices: 0,
        triangles: 0,
        triangles_extracted: 0,
        seams_simplified: false,
        seam_faults: 0,
        topology: MeshTopology::default(),
        deviation: MeshDeviation::default(),
        timings,
    };
    for piece in &pieces {
        report.tiles += u32::from(!piece.triangles.is_empty());
        report.points += piece.stats.points;
        report.points_read += piece.stats.read;
        report.surfels += piece.stats.surfels;
        report.surfels_by_station += piece.stats.by_station;
        report.surfels_by_default += piece.stats.by_default;
        report.triangles_extracted += piece.extracted;
        report.timings.tile_reading += piece.times[0];
        report.timings.tile_elements += piece.times[1];
        report.timings.tile_field += piece.times[2];
        report.timings.tile_surface += piece.times[3];
        report.timings.tile_simplifying += piece.times[4];
    }
    report.orientation = match report.surfels_by_station {
        0 => OrientationUsed::Fallback,
        all if all == report.surfels => OrientationUsed::Stations,
        _ => OrientationUsed::Mixed,
    };
    if report.points == 0 {
        return Err(empty());
    }
    // Joining takes seconds on a large mesh: the callback is asked between
    // the pieces, with all tiles done, so that a cancel is still heard.
    let (mut mesh, seam_faults) = join(&lattice, pieces, &mut || {
        progress(stage(ClosedMeshStage::Reconstructing, total, total))
    })?;
    report.seam_faults = seam_faults;
    if mesh.triangles.is_empty() {
        return Err(LoadError::InvalidData(
            "the points in the region are too few or too scattered to form a surface".into(),
        ));
    }
    report.timings.joining = joining.elapsed();

    // The tiles kept their borders as they were, which leaves a line of
    // small triangles along every seam.
    if tolerance > 0.0 && mesh.triangles.len() <= SEAM_PASS_TRIANGLES {
        let seams = Instant::now();
        let mut locked = open_boundary_vertices_progress(&mesh, &mut |_, _| {
            progress(stage(ClosedMeshStage::Simplifying, 0, 0))
        })?;
        // Where the colour changes the tiles left their vertices a voxel
        // or two apart; those stay.
        mark_color_edges(&mesh, config.color_step, 3.0 * voxel, &mut locked);
        mesh = simplify_mesh_progress(&mesh, tolerance, &locked, &mut |handled, total| {
            progress(stage(ClosedMeshStage::Simplifying, handled, total))
        })?
        .mesh;
        report.seams_simplified = true;
        report.timings.seam_simplifying = seams.elapsed();
    }
    if mesh.vertices.len() > config.max_vertices || mesh.triangles.len() > config.max_triangles {
        return Err(too_large(config));
    }

    // Counting the edges is the first half of the measuring and the
    // distances of the sampled points the second, each in thousandths.
    let measuring = Instant::now();
    let half = MEASURING_STEPS / 2;
    report.vertices = mesh.vertices.len();
    report.triangles = mesh.triangles.len();
    report.topology = mesh_topology_progress(&mesh, &mut |done, total| {
        let share = done * half / total.max(1);
        progress(stage(ClosedMeshStage::Measuring, share, MEASURING_STEPS))
    })?;
    let points: Vec<[f64; 3]> = samples.iter().map(|sample| sample.1).collect();
    report.deviation = mesh_deviation_progress(&mesh, &points, &mut |handled, total| {
        let share = half + (handled + 1) * half / (total + 1);
        progress(stage(ClosedMeshStage::Measuring, share, MEASURING_STEPS))
    })?;
    report.timings.measuring = measuring.elapsed();
    report.timings.total = started.elapsed();
    Ok((mesh, report))
}

fn too_large(config: &ClosedMeshConfig) -> LoadError {
    LoadError::InvalidData(format!(
        "the mesh would exceed {} vertices or {} triangles: use a larger voxel size, a larger \
         simplification tolerance or a smaller section box",
        config.max_vertices, config.max_triangles
    ))
}

/// Keep the samples with the lowest numbers and return the highest number
/// kept: a point with a higher one cannot be among the final samples.
fn keep_lowest(samples: &mut Vec<(u64, [f64; 3])>, limit: usize) -> u64 {
    if limit == 0 {
        samples.clear();
        return 0;
    }
    if samples.len() > limit {
        samples.select_nth_unstable_by_key(limit - 1, |sample| sample.0);
        samples.truncate(limit);
    }
    if samples.len() < limit {
        return u64::MAX;
    }
    samples
        .iter()
        .map(|sample| sample.0)
        .max()
        .unwrap_or(u64::MAX)
}

/// A number per point that looks random but is the same on every run, so
/// that the points with the lowest numbers are an even sample of the scan.
fn sample_number(source: usize, ordinal: u64) -> u64 {
    let mut value = ordinal ^ (source as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value = (value ^ value >> 30).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ value >> 27).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ value >> 31
}

/// What every tile of a job shares.
struct Job<'a> {
    lattice: &'a Lattice,
    sources: &'a [SurfelSource<'a>],
    accept: &'a RegionFilter<'a>,
    orientation: SurfelOrientation,
    tolerance: f64,
    color_step: u8,
    samples: usize,
    stop: AtomicBool,
    /// Points with a sample number from this on are not among the samples.
    sample_limit: AtomicU64,
    /// What the tiles in work hold of the memory allowance.
    in_work: Mutex<InWork>,
    freed: Condvar,
}

/// The tiles in work at one moment.
#[derive(Default)]
struct InWork {
    /// Bytes they are expected to need together.
    bytes: u64,
    tiles: usize,
    /// The most tiles there were at any moment.
    most: usize,
}

impl Job<'_> {
    fn proceed(&self) -> Result<(), LoadError> {
        if self.stop.load(Ordering::Relaxed) {
            Err(LoadError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// What the grids around a tile take, which follows from the lattice.
    fn grid_memory(&self) -> u64 {
        let lattice = self.lattice;
        let block = u64::from(lattice.tile + 2 * lattice.halo);
        let known = u64::from(lattice.tile) + 2 * lattice.close_radius.ceil() as u64 + 3;
        known.pow(3) * 30 + (known + 12).pow(3).min((block + 1).pow(3)) * 4
    }

    /// What a tile is expected to need before its points are read: its
    /// grids and its surface elements. Those are no more than the points
    /// of the leaves it reads, and no more than a block full of clutter
    /// comes to.
    fn tile_memory(&self, tile: [u32; 3]) -> u64 {
        let lattice = self.lattice;
        let block = u64::from(lattice.tile + 2 * lattice.halo);
        let Some(read_box) = lattice.read_box(tile) else {
            return self.grid_memory();
        };
        let points: u64 = self
            .sources
            .iter()
            .map(|source| match source.points.reader {
                RegionReader::Index(index) => index
                    .intersecting_leaves(|node| {
                        overlaps(source.points.transform.bounds(node), read_box)
                    })
                    .iter()
                    .map(|leaf| leaf.stored_points)
                    .sum(),
                RegionReader::Resident(points) => points.len() as u64,
                RegionReader::Stream(cloud) => cloud.total_points,
            })
            .sum();
        self.grid_memory() + points.min(BLOCK_SHEETS * (2 * block).pow(2)) * ELEMENT_BYTES
    }

    /// Wait until the tiles in work leave room for one more, and take that
    /// room until the returned guard goes. A tile that needs more than all
    /// there is runs alone.
    fn reserve(&self, bytes: u64) -> Result<Reserved<'_>, LoadError> {
        let mut in_work = self.in_work.lock().unwrap_or_else(PoisonError::into_inner);
        while in_work.tiles > 0 && in_work.bytes + bytes > TILE_MEMORY {
            self.proceed()?;
            in_work = self
                .freed
                .wait_timeout(in_work, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        in_work.bytes += bytes;
        in_work.tiles += 1;
        in_work.most = in_work.most.max(in_work.tiles);
        Ok(Reserved { job: self, bytes })
    }
}

/// Memory a tile holds of the job's allowance while it is in work.
struct Reserved<'a> {
    job: &'a Job<'a>,
    bytes: u64,
}

impl Reserved<'_> {
    /// Give back what the tile turns out not to need, once its elements
    /// are counted, so that a waiting tile can start. Asking for more is
    /// not done: tiles that hold a part and wait for the rest would wait
    /// for each other.
    fn shrink_to(&mut self, bytes: u64) {
        if bytes < self.bytes {
            self.job
                .in_work
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .bytes -= self.bytes - bytes;
            self.bytes = bytes;
            self.job.freed.notify_all();
        }
    }
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        let mut in_work = self
            .job
            .in_work
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        in_work.bytes -= self.bytes;
        in_work.tiles -= 1;
        drop(in_work);
        self.job.freed.notify_all();
    }
}

/// The part of the mesh one tile owns: the vertices of its own voxels,
/// named by voxel key and ordered by it, and triangles that name vertices
/// by key, some of which belong to the tiles below it.
#[derive(Default)]
struct TilePiece {
    keys: Vec<u64>,
    positions: Vec<[f64; 3]>,
    normals: Vec<[f32; 3]>,
    /// The last value is 255 for a vertex that has a colour.
    colors: Vec<[u8; 4]>,
    triangles: Vec<[u64; 3]>,
    samples: Vec<(u64, [f64; 3])>,
    stats: TileSurfelStats,
    extracted: u64,
    /// Reading, elements, field, surface, simplifying.
    times: [Duration; 5],
}

fn mesh_tile(job: &Job<'_>, tile: [u32; 3]) -> Result<TilePiece, LoadError> {
    let mut piece = TilePiece::default();
    let mut room = job.reserve(job.tile_memory(tile))?;
    let started = Instant::now();
    let mut limit = job.sample_limit.load(Ordering::Relaxed);
    let mut samples: Vec<(u64, [f64; 3])> = Vec::new();
    let surfels = collect_surfels(
        job.lattice,
        job.sources,
        job.accept,
        job.orientation,
        tile,
        &|| job.proceed(),
        &mut |source, record: &IndexedPoint| {
            if job.samples == 0 {
                return;
            }
            let number = sample_number(source, record.ordinal);
            if number < limit {
                samples.push((number, record.point.xyz));
                if samples.len() >= 2 * job.samples {
                    limit = keep_lowest(&mut samples, job.samples);
                }
            }
        },
    )?;
    let elements: u64 = surfels.levels.iter().map(|level| level.len() as u64).sum();
    room.shrink_to(job.grid_memory() + elements * ELEMENT_BYTES);
    keep_lowest(&mut samples, job.samples);
    piece.samples = samples;
    piece.stats = surfels.stats;
    piece.times[0] = surfels.stats.read_time;
    piece.times[1] = started.elapsed().saturating_sub(surfels.stats.read_time);
    if surfels.levels.is_empty() {
        return Ok(piece);
    }

    let started = Instant::now();
    let field = Field::new(job.lattice, &surfels);
    piece.times[2] = started.elapsed();
    job.proceed()?;
    let Some(field) = field else {
        return Ok(piece);
    };

    let started = Instant::now();
    let mut surface = extract(&surfels, &field);
    drop(field);
    piece.extracted = surface.triangles.len() as u64;
    remove_dust(&surfels, &mut surface);
    piece.times[3] = started.elapsed();
    job.proceed()?;

    let started = Instant::now();
    finish_piece(job, &surfels, surface, &mut piece)?;
    piece.times[4] = started.elapsed();
    Ok(piece)
}

fn squared(v: [f32; 3]) -> f32 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalized(v: [f32; 3]) -> [f32; 3] {
    let length = squared(v).sqrt();
    if length > 1e-12 {
        v.map(|value| value / length)
    } else {
        [0.0; 3]
    }
}

/// The share of an element at a squared distance: one at the element,
/// falling smoothly to nothing at `radius`. Close to a bell curve with a
/// width of 0.4 radius, without its endless tail.
fn kernel(distance_squared: f32, radius: f32) -> f32 {
    let rest = 1.0 - distance_squared / (radius * radius);
    if rest <= 0.0 {
        return 0.0;
    }
    let square = rest * rest;
    square * square * square
}

/// The elements of one level, summed at lattice corners that lie as far
/// apart as the bell of that level is wide: every voxel for the finest
/// level, every `step` voxels above it. Between those corners the sums are
/// interpolated, which gives the distance at any voxel corner from eight
/// numbers instead of from every element within reach.
struct LevelSums {
    /// Voxels between two corners.
    step: i32,
    /// The first corner along each axis, in steps from the block's corner.
    first: i32,
    side: usize,
    weight: Vec<f32>,
    /// Summed distance to the planes of the elements, at the corner itself.
    value: Vec<f32>,
    normal: Vec<[f32; 3]>,
    /// For the finest level: whether an element lies within `SUPPORT`.
    near: Vec<bool>,
}

impl LevelSums {
    /// The sums of a level over the corners `from..=to` of the block. The
    /// elements are added in their fixed order, and each by its offset from
    /// the corner, so a corner gets the same sums in every tile.
    fn new(level: &SurfelLevel, number: usize, from: i32, to: i32) -> Self {
        let step = 1i32 << number;
        let first = from.div_euclid(step);
        let side = (to.div_euclid(step) + 1 - first + 1) as usize;
        let last = first + side as i32 - 1;
        let mut sums = Self {
            step,
            first,
            side,
            weight: vec![0.0; side.pow(3)],
            value: vec![0.0; side.pow(3)],
            normal: vec![[0.0; 3]; side.pow(3)],
            near: vec![false; if number == 0 { side.pow(3) } else { 0 }],
        };
        let radius = level.reach;
        let limit = radius * radius;
        for index in 0..level.len() {
            let weight = level.weight[index];
            if weight <= 0.0 {
                continue;
            }
            // The corners within reach of the cell, whose side is half a
            // step; which of them the element itself reaches is seen below.
            let cell = level.cell[index].map(|value| f32::from(value) * level.size);
            let low: [i32; 3] = std::array::from_fn(|axis| {
                (((cell[axis] - radius) / step as f32).floor() as i32).max(first)
            });
            let high: [i32; 3] = std::array::from_fn(|axis| {
                (((cell[axis] + level.size + radius) / step as f32).floor() as i32).min(last)
            });
            if (0..3).any(|axis| low[axis] > high[axis]) {
                continue;
            }
            // From the element to each corner in reach, per axis.
            let mut offsets = [[0f32; 12]; 3];
            for axis in 0..3 {
                for (place, corner) in (low[axis]..=high[axis]).enumerate() {
                    offsets[axis][place] = level.offset_along(index, axis, corner * step, 0.0);
                }
            }
            let normal = level.normal[index];
            for (z, across_z) in (low[2]..=high[2]).zip(offsets[2]) {
                let far_z = across_z * across_z;
                if far_z >= limit {
                    continue;
                }
                for (y, across_y) in (low[1]..=high[1]).zip(offsets[1]) {
                    let far_y = far_z + across_y * across_y;
                    if far_y >= limit {
                        continue;
                    }
                    let height = normal[2] * across_z + normal[1] * across_y;
                    let row = ((z - first) as usize * side + (y - first) as usize) * side;
                    for (x, across_x) in (low[0]..=high[0]).zip(offsets[0]) {
                        let far = far_y + across_x * across_x;
                        let share = kernel(far, radius) * weight;
                        if share > 0.0 {
                            let slot = row + (x - first) as usize;
                            if number == 0 && far <= SUPPORT * SUPPORT {
                                sums.near[slot] = true;
                            }
                            sums.weight[slot] += share;
                            sums.value[slot] += share * (height + normal[0] * across_x);
                            for (sum, part) in sums.normal[slot].iter_mut().zip(normal) {
                                *sum += share * part;
                            }
                        }
                    }
                }
            }
        }
        sums
    }

    /// Distance, summed normal and summed weight at a voxel corner, when an
    /// element of this level reaches one of the corners around it.
    fn at(&self, corner: [i32; 3]) -> Option<(f32, [f32; 3], f32)> {
        let cell = corner.map(|value| value.div_euclid(self.step));
        // How far along the cell the corner lies, in steps: exact.
        let along = corner.map(|value| value.rem_euclid(self.step) as f32 / self.step as f32);
        let base = cell.map(|value| (value - self.first) as usize);
        let (mut weight, mut value, mut normal) = (0f32, 0f32, [0f32; 3]);
        for corner in 0..8usize {
            let bits = [corner & 1, corner >> 1 & 1, corner >> 2];
            let slot = ((base[2] + bits[2]) * self.side + base[1] + bits[1]) * self.side
                + base[0]
                + bits[0];
            if self.weight[slot] <= 0.0 {
                continue;
            }
            let share: f32 = (0..3)
                .map(|axis| {
                    if bits[axis] == 1 {
                        along[axis]
                    } else {
                        1.0 - along[axis]
                    }
                })
                .product();
            if share <= 0.0 {
                continue;
            }
            // The planes summed at that corner, carried over to this one.
            let offset: [f32; 3] =
                std::array::from_fn(|axis| (along[axis] - bits[axis] as f32) * self.step as f32);
            weight += share * self.weight[slot];
            value += share * (self.value[slot] + dot(self.normal[slot], offset));
            for (sum, part) in normal.iter_mut().zip(self.normal[slot]) {
                *sum += share * part;
            }
        }
        (weight > 0.0).then(|| (value / weight, normal, weight))
    }
}

/// The signed distance around one tile, on the lattice corners from one
/// voxel below the tile, plus the reach of gap closing, to as far above.
struct Field {
    /// Corners along each axis.
    side: usize,
    /// Corner of the block the first one is.
    first: i32,
    /// Distance to the surface in voxels: positive on the side it was seen
    /// from.
    value: Vec<f32>,
    /// Weighted sum of the normals of the elements around, not normalised.
    normal: Vec<[f32; 3]>,
    /// The level of elements the value came from, `NO_LEVEL` without one.
    level: Vec<u8>,
    /// Whether the surface may pass by this corner: near data, or in a gap
    /// narrow enough to close.
    defined: Vec<bool>,
}

impl Field {
    fn index(&self, corner: [i32; 3]) -> usize {
        let [x, y, z] = corner.map(|value| (value - self.first) as usize);
        (z * self.side + y) * self.side + x
    }

    /// Nothing when the tile has no element that stands for a surface.
    fn new(lattice: &Lattice, surfels: &TileSurfels) -> Option<Self> {
        let fitted = surfels
            .levels
            .iter()
            .any(|level| level.fitted.iter().any(|fitted| *fitted));
        if !fitted {
            return None;
        }
        let block = surfels.block;
        let reach = lattice.close_radius.ceil() as i32;
        let first = block.halo as i32 - 1 - reach;
        let last = first + block.tile as i32 + 1 + 2 * reach;

        // The sums of every level; those of the finest one, which lie at
        // the corners themselves, become the field. A corner close to an
        // element of the finest level is near data.
        let mut sums: Vec<LevelSums> = surfels
            .levels
            .iter()
            .enumerate()
            .map(|(number, level)| LevelSums::new(level, number, first, last))
            .collect();
        let finest = sums.remove(0);
        // One corner more than asked for, as every level has; it is not
        // looked at.
        let side = finest.side;
        let count = side.pow(3);
        let mut field = Self {
            side,
            first,
            value: finest.value,
            normal: finest.normal,
            level: vec![NO_LEVEL; count],
            defined: finest.near,
        };
        for (slot, weight) in finest.weight.iter().enumerate() {
            if *weight > 0.0 {
                field.value[slot] /= weight;
                field.level[slot] = 0;
            }
        }
        drop(finest.weight);
        // Where the points are too sparse for the finest level, the
        // elements that found a surface at a coarser one lie further apart
        // and stand for the corners as much further around them.
        for (number, level) in surfels.levels.iter().enumerate().skip(1) {
            let size = 1i32 << (number - 1);
            let radius = SUPPORT * (1u32 << number) as f32;
            let spread = radius.ceil() as i32;
            for index in (0..level.len()).filter(|index| level.fitted[*index]) {
                let cell = level.cell[index].map(|value| i32::from(value) * size);
                let low = cell.map(|value| (value - spread).max(first));
                let high = cell.map(|value| (value + size + spread).min(last));
                for z in low[2]..=high[2] {
                    for y in low[1]..=high[1] {
                        for x in low[0]..=high[0] {
                            let offset = level.offset(index, [x, y, z], [0.0; 3]);
                            if squared(offset) <= radius * radius {
                                let slot = field.index([x, y, z]);
                                field.defined[slot] = true;
                            }
                        }
                    }
                }
            }
        }

        // For closing gaps: the squared distance from every corner to the
        // nearest corner beside an element that stands for a surface, as
        // far around the corners of the field as a distance can matter.
        let closing = lattice.close_radius > 0.0;
        let close = (lattice.close_radius + SEED_SLACK).powi(2);
        let widest = (lattice.close_radius + SEED_SLACK + SHELL).powi(2);
        let around = widest.sqrt().ceil() as i32 + 1;
        let grid_first = (first - around).max(0);
        let grid = ((last + around).min(block.size as i32) - grid_first + 1) as usize;
        let at = |corner: [i32; 3]| {
            let [x, y, z] = corner.map(|value| (value - grid_first) as usize);
            (z * grid + y) * grid + x
        };
        let mut distance = vec![FAR; if closing { grid.pow(3) } else { 0 }];
        if closing {
            let mut low = [usize::MAX; 3];
            let mut high = [0usize; 3];
            let mut seed = |corner: [i32; 3]| {
                let inside = corner
                    .iter()
                    .all(|value| (grid_first..grid_first + grid as i32).contains(value));
                if inside {
                    distance[at(corner)] = 0;
                    for axis in 0..3 {
                        let place = (corner[axis] - grid_first) as usize;
                        low[axis] = low[axis].min(place);
                        high[axis] = high[axis].max(place);
                    }
                }
            };
            for (number, level) in surfels.levels.iter().enumerate() {
                for index in (0..level.len()).filter(|index| level.fitted[*index]) {
                    // The corner nearest to the cell of the element; cells
                    // of the finest level are half a voxel.
                    seed(level.cell[index].map(|value| {
                        if number == 0 {
                            (i32::from(value) + 1) >> 1
                        } else {
                            i32::from(value) << (number - 1)
                        }
                    }));
                }
            }
            if low[0] <= high[0] {
                squared_distances(
                    &mut distance,
                    grid,
                    low.map(|value| value.saturating_sub(around as usize)),
                    high.map(|value| (value + around as usize).min(grid - 1)),
                );
            }
        }

        // Corners the finest level does not reach take the finest of the
        // coarser ones that does: in a gap, and where points are sparse.
        // Corners where the surface runs on beyond half the widest gap from
        // any data mark the open side of an edge, and so do corners where
        // more than one face is in reach of the level that tells: what is
        // told there is the plane of none of them, and a gap closed along
        // it comes out crooked or half closed. Such a gap stays open.
        let mut open = vec![FAR; if closing { count } else { 0 }];
        let mut open_low = [usize::MAX; 3];
        let mut open_high = [0usize; 3];
        for z in 0..side - 1 {
            for y in 0..side - 1 {
                for x in 0..side - 1 {
                    let slot = (z * side + y) * side + x;
                    let corner = [x, y, z].map(|value| value as i32 + first);
                    let near = field.defined[slot];
                    let away = if closing {
                        f64::from(distance[at(corner)])
                    } else {
                        f64::INFINITY
                    };
                    if !near && away > widest {
                        continue;
                    }
                    let mut mixed = false;
                    if field.level[slot] == NO_LEVEL {
                        let found = sums.iter().enumerate().find_map(|(number, sums)| {
                            sums.at(corner).map(|found| (found, number as u8 + 1))
                        });
                        let Some(((value, normal, weight), level)) = found else {
                            field.defined[slot] = false;
                            continue;
                        };
                        field.value[slot] = value;
                        field.normal[slot] = normal;
                        field.level[slot] = level;
                        mixed = squared(normal) < (ONE_FACE * weight) * (ONE_FACE * weight);
                    }
                    if !near && (away > close || mixed) && field.value[slot].abs() <= SHEET {
                        open[slot] = 0;
                        for (axis, value) in [x, y, z].into_iter().enumerate() {
                            open_low[axis] = open_low[axis].min(value);
                            open_high[axis] = open_high[axis].max(value);
                        }
                    }
                }
            }
        }
        if !closing {
            return Some(field);
        }
        if open_low[0] <= open_high[0] {
            let margin = reach as usize + 1;
            squared_distances(
                &mut open,
                side,
                open_low.map(|value| value.saturating_sub(margin)),
                std::array::from_fn(|axis| (open_high[axis] + margin).min(side - 1)),
            );
        }
        // A corner in a gap: on the sheet its surroundings predict, within
        // half the limit of the data, and out of reach of every open side.
        // A gap wider than the limit has an open side in its middle and
        // stays open as a whole; so does the rim of the scan.
        let radius = lattice.close_radius * lattice.close_radius;
        for z in 0..side - 1 {
            for y in 0..side - 1 {
                for x in 0..side - 1 {
                    let slot = (z * side + y) * side + x;
                    if field.defined[slot] || field.level[slot] == NO_LEVEL {
                        continue;
                    }
                    let corner = [x, y, z].map(|value| value as i32 + first);
                    field.defined[slot] = f64::from(distance[at(corner)]) <= close
                        && field.value[slot].abs() <= SHEET
                        && f64::from(open[slot]) > radius;
                }
            }
        }
        Some(field)
    }
}

/// Replace every cell inside a box of a cubic grid by the squared distance,
/// in cells, to the nearest cell of the box that holds zero. Cells start as
/// zero or `FAR`; what lies outside the box is left alone and not looked at.
fn squared_distances(grid: &mut [i32], side: usize, low: [usize; 3], high: [usize; 3]) {
    // Along x a cell is as far as the nearest zero in its row.
    for z in low[2]..=high[2] {
        for y in low[1]..=high[1] {
            let start = (z * side + y) * side;
            let row = &mut grid[start + low[0]..=start + high[0]];
            let mut gap = FAR;
            for cell in row.iter_mut() {
                gap = if *cell == 0 { 0 } else { (gap + 1).min(FAR) };
                *cell = gap;
            }
            let mut gap = FAR;
            for cell in row.iter_mut().rev() {
                gap = if *cell == 0 { 0 } else { (gap + 1).min(FAR) };
                let nearest = gap.min(*cell);
                *cell = if nearest >= FAR {
                    FAR
                } else {
                    nearest * nearest
                };
            }
        }
    }
    // Along the other axes the nearest is found among parabolas, one per
    // cell of the line, of which only the lowest part of each counts.
    let longest = (0..3)
        .map(|axis| high[axis] - low[axis] + 1)
        .max()
        .unwrap_or(0);
    let mut line = vec![0i32; longest];
    let mut vertex = vec![0usize; longest];
    let mut start = vec![0f32; longest + 1];
    let strides = [1, side, side * side];
    for (axis, a, b) in [(1, 0, 2), (2, 0, 1)] {
        let step = strides[axis];
        let length = high[axis] - low[axis] + 1;
        for outer in low[b]..=high[b] {
            for inner in low[a]..=high[a] {
                let base = outer * strides[b] + inner * strides[a] + low[axis] * step;
                let mut hulls = 0usize;
                for place in 0..length {
                    let height = grid[base + place * step];
                    line[place] = height;
                    if height >= FAR {
                        continue;
                    }
                    let lift = |at: usize, height: i32| (height + (at * at) as i32) as f32;
                    while hulls > 0 {
                        let before = vertex[hulls - 1];
                        let crossing = (lift(place, height) - lift(before, line[before]))
                            / (2 * (place - before)) as f32;
                        if crossing > start[hulls - 1] {
                            start[hulls] = crossing;
                            break;
                        }
                        hulls -= 1;
                    }
                    if hulls == 0 {
                        start[0] = f32::NEG_INFINITY;
                    }
                    vertex[hulls] = place;
                    hulls += 1;
                }
                if hulls == 0 {
                    continue;
                }
                let mut hull = 0usize;
                for place in 0..length {
                    while hull + 1 < hulls && start[hull + 1] < place as f32 {
                        hull += 1;
                    }
                    let from = vertex[hull];
                    let across = place.abs_diff(from) as i32;
                    grid[base + place * step] = across * across + line[from];
                }
            }
        }
    }
}

/// The surface of one tile before it is cut loose from the block: vertices
/// by the voxel they lie in.
#[derive(Default)]
struct Surface {
    /// Voxel of every vertex, from the block's corner.
    cells: Vec<[i32; 3]>,
    /// Which of the vertices of its voxel it is; nearly always the only.
    pieces: Vec<u8>,
    /// Place of the vertex within its voxel, each value from 0 to 1.
    parts: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[u8; 4]>,
    triangles: Vec<[u32; 3]>,
}

impl Surface {
    /// Position in voxels from the block's corner.
    fn position(&self, vertex: u32) -> [f64; 3] {
        let vertex = vertex as usize;
        std::array::from_fn(|axis| {
            f64::from(self.cells[vertex][axis]) + f64::from(self.parts[vertex][axis])
        })
    }
}

/// The two ends of the twelve edges of a voxel, as corner numbers with x in
/// the lowest bit: four edges along x, four along y, four along z, and
/// within each four ordered by the two other coordinates of the edge.
const VOXEL_EDGES: [(usize, usize); 12] = [
    (0, 1),
    (2, 3),
    (4, 5),
    (6, 7),
    (0, 2),
    (1, 3),
    (4, 6),
    (5, 7),
    (0, 4),
    (1, 5),
    (2, 6),
    (3, 7),
];

/// The six faces of a voxel: their corners going round, and the edge from
/// each corner to the next.
const VOXEL_FACES: [([usize; 4], [usize; 4]); 6] = [
    ([0, 2, 6, 4], [4, 10, 6, 8]),
    ([1, 3, 7, 5], [5, 11, 7, 9]),
    ([0, 1, 5, 4], [0, 9, 2, 8]),
    ([2, 3, 7, 6], [1, 11, 3, 10]),
    ([0, 1, 3, 2], [0, 5, 1, 4]),
    ([4, 5, 7, 6], [2, 7, 3, 6]),
];

/// The vertices of one voxel: usually one, more where the surface passes
/// through the voxel in separate pieces.
struct VoxelVertices {
    count: usize,
    /// Place within the voxel and normal of each.
    vertices: [([f32; 3], [f32; 3]); 4],
    /// Per edge of the voxel, two bits: the vertex of the piece of surface
    /// that crosses that edge.
    of_edge: u32,
}

/// The vertices of a voxel the surface passes through. Where the surface
/// crosses the edges of the voxel it has a position and a direction; a
/// vertex is the point that lies best on all those planes, which puts it on
/// a corner or an edge of the surface when there is one inside the voxel.
///
/// One vertex per voxel would make four triangles meet in one edge where
/// two pieces of surface pass through the same voxel, as happens when noise
/// decides the side of corners that lie on a wall. The crossings are
/// therefore grouped per piece: across each face of the voxel a piece runs
/// from one crossed edge to another, and on a face with four crossings the
/// two corners behind the surface are each cut off on their own, which both
/// voxels at that face see alike.
fn voxel_vertices(field: &Field, cell: [i32; 3]) -> Option<VoxelVertices> {
    let base = field.index(cell);
    let side = field.side;
    let slots: [usize; 8] = std::array::from_fn(|corner| {
        base + (corner & 1) + (corner >> 1 & 1) * side + (corner >> 2) * side * side
    });
    if !slots.iter().all(|slot| field.defined[*slot]) {
        return None;
    }
    let values = slots.map(|slot| field.value[slot]);
    let behind = values.map(|value| value < 0.0);
    if behind.iter().all(|side| *side) || !behind.iter().any(|side| *side) {
        return None;
    }
    let normals = slots.map(|slot| normalized(field.normal[slot]));
    let place = |corner: usize| -> [f64; 3] {
        [
            (corner & 1) as f64,
            (corner >> 1 & 1) as f64,
            (corner >> 2) as f64,
        ]
    };
    // Where the surface crosses each edge, and its direction there.
    let mut crossed = [false; 12];
    let mut crossings = [([0f64; 3], [0f64; 3]); 12];
    for (edge, (a, b)) in VOXEL_EDGES.into_iter().enumerate() {
        if behind[a] == behind[b] {
            continue;
        }
        let along = (values[a] / (values[a] - values[b])).clamp(0.0, 1.0);
        let (from, to) = (place(a), place(b));
        let point: [f64; 3] =
            std::array::from_fn(|axis| from[axis] + f64::from(along) * (to[axis] - from[axis]));
        let direction = normalized(std::array::from_fn(|axis| {
            normals[a][axis] + along * (normals[b][axis] - normals[a][axis])
        }));
        crossed[edge] = true;
        crossings[edge] = (point, direction.map(f64::from));
    }
    // The pieces: crossed edges joined across the faces.
    let mut group: [usize; 12] = std::array::from_fn(|edge| edge);
    let root = |group: &[usize; 12], mut edge: usize| {
        while group[edge] != edge {
            edge = group[edge];
        }
        edge
    };
    let mut join = |a: usize, b: usize| {
        let (a, b) = (root(&group, a), root(&group, b));
        // The lower edge stays the root, so pieces are told apart alike
        // everywhere.
        group[a.max(b)] = a.min(b);
    };
    for (corners, edges) in VOXEL_FACES {
        let on_face: [bool; 4] = std::array::from_fn(|turn| crossed[edges[turn]]);
        match on_face.iter().filter(|crossed| **crossed).count() {
            2 => {
                let mut pair = (0..4).filter(|turn| on_face[*turn]);
                if let (Some(a), Some(b)) = (pair.next(), pair.next()) {
                    join(edges[a], edges[b]);
                }
            }
            4 => {
                for turn in 0..4 {
                    if behind[corners[turn]] {
                        join(edges[(turn + 3) % 4], edges[turn]);
                    }
                }
            }
            _ => {}
        }
    }
    let mut found = VoxelVertices {
        count: 0,
        vertices: [([0.0; 3], [0.0; 3]); 4],
        of_edge: 0,
    };
    let mut roots = [usize::MAX; 4];
    for edge in (0..12).filter(|edge| crossed[*edge]) {
        let piece = root(&group, edge);
        let number = match roots[..found.count]
            .iter()
            .position(|known| *known == piece)
        {
            Some(number) => number,
            None if found.count < 4 => {
                roots[found.count] = piece;
                found.count += 1;
                found.count - 1
            }
            // Eight corners cannot hold more than four pieces.
            None => 3,
        };
        found.of_edge |= (number as u32) << (2 * edge);
    }
    for number in 0..found.count {
        let own =
            |edge: &usize| crossed[*edge] && (found.of_edge >> (2 * edge) & 3) as usize == number;
        let count = (0..12).filter(own).count() as f64;
        let mut middle = [0f64; 3];
        for edge in (0..12).filter(own) {
            for (sum, part) in middle.iter_mut().zip(crossings[edge].0) {
                *sum += part / count;
            }
        }
        // Least squares on the planes of the crossings, from their middle.
        let mut matrix = [[0f64; 3]; 3];
        let mut right = [0f64; 3];
        let mut mean_direction = [0f64; 3];
        for edge in (0..12).filter(own) {
            let (point, direction) = crossings[edge];
            let offset: f64 = (0..3)
                .map(|axis| direction[axis] * (point[axis] - middle[axis]))
                .sum();
            for a in 0..3 {
                right[a] += direction[a] * offset;
                mean_direction[a] += direction[a];
                for b in 0..3 {
                    matrix[a][b] += direction[a] * direction[b];
                }
            }
        }
        let (values, vectors) = symmetric_eigen3(matrix);
        let mut vertex = middle;
        for (value, vector) in values.iter().zip(&vectors) {
            // A direction the planes say little about is left alone: on a
            // flat face the vertex only moves across the face.
            if *value >= VERTEX_RANK * values[2] && *value > 0.0 {
                let step = (0..3).map(|axis| vector[axis] * right[axis]).sum::<f64>()
                    / (value + VERTEX_PULL);
                for axis in 0..3 {
                    vertex[axis] += step * vector[axis];
                }
            }
        }
        if !vertex.iter().all(|value| value.is_finite()) {
            vertex = middle;
        }
        let vertex = vertex.map(|value| value.clamp(0.0, 1.0) as f32);
        // The normal of the field at the vertex, between the eight
        // corners; with several pieces in the voxel that of its own
        // crossings.
        let mut normal = [0f32; 3];
        if found.count == 1 {
            for (corner, direction) in normals.iter().enumerate() {
                let share: f32 = (0..3)
                    .map(|axis| {
                        if corner >> axis & 1 == 1 {
                            vertex[axis]
                        } else {
                            1.0 - vertex[axis]
                        }
                    })
                    .product();
                for axis in 0..3 {
                    normal[axis] += share * direction[axis];
                }
            }
        }
        let mut normal = normalized(normal);
        if normal == [0.0; 3] {
            normal = normalized(mean_direction.map(|value| value as f32));
        }
        found.vertices[number] = (vertex, normal);
    }
    Some(found)
}

/// The colour of the points around a place: from the finest level that has
/// coloured points within reach.
fn color_at(levels: &[SurfelLevel], cell: [i32; 3], part: [f32; 3]) -> [u8; 4] {
    for (number, level) in levels.iter().enumerate() {
        let center: [i32; 3] = std::array::from_fn(|axis| {
            if number == 0 {
                cell[axis] * 2 + i32::from(part[axis] >= 0.5)
            } else {
                cell[axis].div_euclid(1 << (number - 1))
            }
        });
        let mut weight = 0f32;
        let mut sum = [0f32; 3];
        level.for_each_near(
            center,
            (level.reach / level.size).ceil() as i32,
            |index, _| {
                let color = level.color[index];
                if color[3] == 0 {
                    return;
                }
                let share = kernel(squared(level.offset(index, cell, part)), level.reach);
                if share > 0.0 {
                    weight += share;
                    for channel in 0..3 {
                        sum[channel] += share * f32::from(color[channel]);
                    }
                }
            },
        );
        if weight > 0.0 {
            let channel = |value: f32| (value / weight).round().clamp(0.0, 255.0) as u8;
            return [channel(sum[0]), channel(sum[1]), channel(sum[2]), 255];
        }
    }
    [0; 4]
}

/// Dual contouring of the tile: vertices in every voxel the surface passes
/// through, from one voxel below the tile, and two triangles across every
/// lattice edge of the tile whose ends lie on different sides.
fn extract(surfels: &TileSurfels, field: &Field) -> Surface {
    let halo = surfels.block.halo as i32;
    let tile = surfels.block.tile as i32;
    let along = (tile + 1) as usize;
    // Per voxel its first vertex, and which vertex each of its edges has.
    let mut first_of = vec![NO_VERTEX; along.pow(3)];
    let mut edges_of = vec![0u32; along.pow(3)];
    let slot_of = |cell: [i32; 3]| {
        let [x, y, z] = cell.map(|value| (value - (halo - 1)) as usize);
        (z * along + y) * along + x
    };
    let mut surface = Surface::default();
    for z in halo - 1..halo + tile {
        for y in halo - 1..halo + tile {
            for x in halo - 1..halo + tile {
                let cell = [x, y, z];
                let Some(found) = voxel_vertices(field, cell) else {
                    continue;
                };
                first_of[slot_of(cell)] = surface.cells.len() as u32;
                edges_of[slot_of(cell)] = found.of_edge;
                for (piece, (part, normal)) in found.vertices[..found.count].iter().enumerate() {
                    surface.cells.push(cell);
                    surface.pieces.push(piece as u8);
                    surface.parts.push(*part);
                    surface.normals.push(*normal);
                    surface.colors.push(color_at(&surfels.levels, cell, *part));
                }
            }
        }
    }
    for z in halo..halo + tile {
        for y in halo..halo + tile {
            for x in halo..halo + tile {
                let corner = [x, y, z];
                let from = field.index(corner);
                if !field.defined[from] {
                    continue;
                }
                let behind = field.value[from] < 0.0;
                for axis in 0..3 {
                    let mut end = corner;
                    end[axis] += 1;
                    let to = field.index(end);
                    if !field.defined[to] || behind == (field.value[to] < 0.0) {
                        continue;
                    }
                    // The four voxels around the edge, turning from the
                    // next axis to the one after it: counter-clockwise
                    // when looked at from the far end of the edge. In each
                    // the vertex of the piece that crosses this edge.
                    let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
                    let (lower, upper) = (b.min(c), b.max(c));
                    let around = [(1, 1), (0, 1), (0, 0), (1, 0)].map(|back: (i32, i32)| {
                        let mut cell = corner;
                        cell[b] -= back.0;
                        cell[c] -= back.1;
                        let slot = slot_of(cell);
                        if first_of[slot] == NO_VERTEX {
                            return NO_VERTEX;
                        }
                        // Where the edge lies in that voxel.
                        let mut bits = [0usize; 3];
                        bits[b] = back.0 as usize;
                        bits[c] = back.1 as usize;
                        let edge = 4 * axis + bits[lower] + 2 * bits[upper];
                        first_of[slot] + (edges_of[slot] >> (2 * edge) & 3)
                    });
                    if around.contains(&NO_VERTEX) {
                        continue;
                    }
                    // The front of a triangle is the side that was seen.
                    let quad = if behind {
                        around
                    } else {
                        [around[3], around[2], around[1], around[0]]
                    };
                    // Cut along the shorter diagonal; measured from the
                    // corner so that every tile measures the same.
                    let place = |vertex: u32| -> [f32; 3] {
                        let vertex = vertex as usize;
                        std::array::from_fn(|axis| {
                            (surface.cells[vertex][axis] - corner[axis]) as f32
                                + surface.parts[vertex][axis]
                        })
                    };
                    let span = |a: u32, b: u32| {
                        let (a, b) = (place(a), place(b));
                        squared(std::array::from_fn(|axis| a[axis] - b[axis]))
                    };
                    if span(quad[1], quad[3]) < span(quad[0], quad[2]) {
                        surface.triangles.push([quad[0], quad[1], quad[3]]);
                        surface.triangles.push([quad[1], quad[2], quad[3]]);
                    } else {
                        surface.triangles.push([quad[0], quad[1], quad[2]]);
                        surface.triangles.push([quad[0], quad[2], quad[3]]);
                    }
                }
            }
        }
    }
    surface
}

/// Whether a voxel lies in a layer a tile shares with a neighbour: the one
/// below the tile, which the tiles below own, or the tile's own last one,
/// which the tiles above build on.
fn on_border(surfels: &TileSurfels, cell: [i32; 3]) -> bool {
    let halo = surfels.block.halo as i32;
    let tile = surfels.block.tile as i32;
    cell.iter()
        .any(|value| *value == halo - 1 || *value == halo + tile - 1)
}

/// The vertex that stands for the piece of surface a vertex belongs to.
fn root(parent: &mut [u32], mut vertex: u32) -> u32 {
    while parent[vertex as usize] != vertex {
        let next = parent[vertex as usize];
        parent[vertex as usize] = parent[next as usize];
        vertex = next;
    }
    vertex
}

/// The pieces of a mesh: vertices joined by triangles, to be asked with
/// `root`.
fn pieces_of(vertices: usize, triangles: &[[u32; 3]]) -> Vec<u32> {
    let mut parent: Vec<u32> = (0..vertices as u32).collect();
    for [a, b, c] in triangles {
        let first = root(&mut parent, *a);
        for other in [*b, *c] {
            let other = root(&mut parent, other);
            parent[other as usize] = first;
        }
    }
    parent
}

/// Drop the pieces of surface that lie wholly inside the tile and are
/// smaller than `DUST_FACES`: stray points that lined up.
fn remove_dust(surfels: &TileSurfels, surface: &mut Surface) {
    let mut parent = pieces_of(surface.cells.len(), &surface.triangles);
    // Per piece its area in voxel faces; a piece that reaches the border
    // may go on in the next tile and is kept whatever its size here.
    let mut area = vec![0f64; surface.cells.len()];
    for vertex in 0..surface.cells.len() as u32 {
        if on_border(surfels, surface.cells[vertex as usize]) {
            area[root(&mut parent, vertex) as usize] = f64::INFINITY;
        }
    }
    for [a, b, c] in &surface.triangles {
        let piece = root(&mut parent, *a) as usize;
        let (a, b, c) = (
            surface.position(*a),
            surface.position(*b),
            surface.position(*c),
        );
        let normal = cross(difference(b, a), difference(c, a));
        area[piece] += 0.5 * dot3(normal, normal).sqrt();
    }
    surface
        .triangles
        .retain(|[a, _, _]| area[root(&mut parent, *a) as usize] >= DUST_FACES);
}

/// Simplify the surface of a tile and hand over what the tile owns.
fn finish_piece(
    job: &Job<'_>,
    surfels: &TileSurfels,
    surface: Surface,
    piece: &mut TilePiece,
) -> Result<(), LoadError> {
    let block = surfels.block;
    let lattice = job.lattice;
    let count = surface.cells.len();
    let world = |vertex: usize| {
        lattice.world(std::array::from_fn(|axis| {
            (block.min[axis] + i64::from(surface.cells[vertex][axis])) as f64
                + f64::from(surface.parts[vertex][axis])
        }))
    };
    // The name of a vertex: its voxel, and which vertex of that voxel.
    let key = |vertex: usize| {
        lattice.key(std::array::from_fn(|axis| {
            block.min[axis] + i64::from(surface.cells[vertex][axis])
        })) << 2
            | u64::from(surface.pieces[vertex])
    };
    // Vertices below the tile belong to the tiles there.
    let owned = |vertex: usize| {
        surface.cells[vertex]
            .iter()
            .all(|value| *value >= block.halo as i32)
    };
    let border: Vec<bool> = (0..count)
        .map(|vertex| on_border(surfels, surface.cells[vertex]))
        .collect();
    let mut mesh = MeshGeometry {
        vertices: (0..count).map(world).collect(),
        triangles: surface.triangles.clone(),
        colors: Some(
            surface
                .colors
                .iter()
                .map(|color| [color[0], color[1], color[2]])
                .collect(),
        ),
        normals: Some(surface.normals.clone()),
    };
    // Which vertex of the surface every vertex of the mesh continues.
    let mut origin: Vec<u32> = (0..count as u32).collect();
    if job.tolerance > 0.0 && !mesh.triangles.is_empty() {
        // The shared layers stay as they are, so that the neighbours, who
        // do the same, still fit; and so do the places where the colour
        // changes.
        let mut locked = border.clone();
        if surface.colors.iter().any(|color| color[3] != 0) {
            mark_color_edges(&mesh, job.color_step, f64::INFINITY, &mut locked);
        }
        let simplified = simplify_mesh(&mesh, job.tolerance, &locked)?;
        mesh = simplified.mesh;
        origin = simplified.source_vertices;
    }
    let mut used = vec![false; mesh.vertices.len()];
    for triangle in &mesh.triangles {
        for vertex in triangle {
            used[*vertex as usize] = true;
        }
    }
    let colors = mesh.colors.as_deref().unwrap_or_default();
    let normals = mesh.normals.as_deref().unwrap_or_default();
    let mut vertices: Vec<(u64, usize, bool)> = Vec::new();
    let mut present = vec![false; count];
    for (vertex, source) in origin.iter().enumerate() {
        let source = *source as usize;
        present[source] = true;
        if owned(source) && (used[vertex] || border[source]) {
            vertices.push((key(source), vertex, true));
        }
    }
    // A vertex of the last layer that no triangle of this tile uses may
    // still be one a tile above builds on.
    for source in 0..count {
        if !present[source] && border[source] && owned(source) {
            vertices.push((key(source), source, false));
        }
    }
    vertices.sort_unstable_by_key(|vertex| vertex.0);
    for (key, vertex, simplified) in vertices {
        piece.keys.push(key);
        if simplified {
            let has_color = surface.colors[origin[vertex] as usize][3];
            let [r, g, b] = colors[vertex];
            piece.positions.push(mesh.vertices[vertex]);
            piece.normals.push(normals[vertex]);
            piece.colors.push([r, g, b, has_color]);
        } else {
            piece.positions.push(world(vertex));
            piece.normals.push(surface.normals[vertex]);
            piece.colors.push(surface.colors[vertex]);
        }
    }
    piece.triangles = mesh
        .triangles
        .iter()
        .map(|triangle| triangle.map(|vertex| key(origin[vertex as usize] as usize)))
        .collect();
    Ok(())
}

/// Mark both ends of every triangle side no longer than `longest` whose
/// colours differ by more than `step` in a channel.
fn mark_color_edges(mesh: &MeshGeometry, step: u8, longest: f64, marked: &mut [bool]) {
    let Some(colors) = &mesh.colors else {
        return;
    };
    if step == u8::MAX {
        return;
    }
    for [a, b, c] in &mesh.triangles {
        for (from, to) in
            [(*a, *b), (*b, *c), (*c, *a)].map(|(from, to)| (from as usize, to as usize))
        {
            let differs =
                (0..3).any(|channel| colors[from][channel].abs_diff(colors[to][channel]) > step);
            if differs {
                let side = difference(mesh.vertices[from], mesh.vertices[to]);
                if dot3(side, side) <= longest * longest {
                    marked[from] = true;
                    marked[to] = true;
                }
            }
        }
    }
}

/// Put the pieces of all tiles together: vertices are found by the key of
/// their voxel in the piece of the tile that owns it. Also returns how many
/// triangles named a vertex that was not there. `proceed` is asked between
/// pieces and may stop the work with an error.
fn join(
    lattice: &Lattice,
    pieces: Vec<TilePiece>,
    proceed: &mut dyn FnMut() -> Result<(), LoadError>,
) -> Result<(MeshGeometry, u64), LoadError> {
    let mut first = Vec::with_capacity(pieces.len());
    let mut total = 0usize;
    for piece in &pieces {
        first.push(total);
        total += piece.keys.len();
    }
    let tile_of: HashMap<[u32; 3], usize> = pieces
        .iter()
        .enumerate()
        .filter_map(|(slot, piece)| {
            let key = *piece.keys.first()?;
            Some((lattice.tile_of(lattice.key_voxel(key >> 2))?, slot))
        })
        .collect();
    let find = |key: u64| -> Option<u32> {
        let slot = *tile_of.get(&lattice.tile_of(lattice.key_voxel(key >> 2))?)?;
        let place = pieces[slot].keys.binary_search(&key).ok()?;
        Some((first[slot] + place) as u32)
    };
    let mut triangles = Vec::new();
    let mut faults = 0u64;
    for piece in &pieces {
        proceed()?;
        for triangle in &piece.triangles {
            // A triangle whose vertex the owning tile does not have would
            // mean the two tiles disagree; it cannot be drawn.
            let [Some(a), Some(b), Some(c)] = triangle.map(find) else {
                faults += 1;
                continue;
            };
            if a != b && b != c && a != c {
                triangles.push([a, b, c]);
            }
        }
    }
    // Dust that lay across the border of two tiles was kept by both, as
    // neither could tell how far it went on. Now that can be told.
    let positions: Vec<[f64; 3]> = pieces
        .iter()
        .flat_map(|piece| piece.positions.iter().copied())
        .collect();
    proceed()?;
    let mut parent = pieces_of(total, &triangles);
    let mut area = vec![0f64; total];
    for triangle in &triangles {
        let [a, b, c] = triangle.map(|vertex| positions[vertex as usize]);
        let normal = cross(difference(b, a), difference(c, a));
        area[root(&mut parent, triangle[0]) as usize] += 0.5 * dot3(normal, normal).sqrt();
    }
    drop(positions);
    let dust = DUST_FACES * lattice.voxel * lattice.voxel;
    triangles.retain(|triangle| area[root(&mut parent, triangle[0]) as usize] >= dust);
    // Vertices that no triangle uses are left out, the others keep their
    // order: tile by tile, and within a tile by key.
    let mut used = vec![false; total];
    for vertex in triangles.iter().flatten() {
        used[*vertex as usize] = true;
    }
    let mut place = vec![NO_VERTEX; total];
    let mut mesh = MeshGeometry::default();
    let mut colors = Vec::new();
    let mut normals = Vec::new();
    let mut colored = false;
    let mut vertex = 0usize;
    for piece in &pieces {
        proceed()?;
        for index in 0..piece.keys.len() {
            if used[vertex] {
                place[vertex] = mesh.vertices.len() as u32;
                mesh.vertices.push(piece.positions[index]);
                normals.push(piece.normals[index]);
                let [r, g, b, has_color] = piece.colors[index];
                colored |= has_color != 0;
                // Grey where the points around have no colour.
                colors.push(if has_color != 0 { [r, g, b] } else { [128; 3] });
            }
            vertex += 1;
        }
    }
    mesh.triangles = triangles
        .into_iter()
        .map(|triangle| triangle.map(|vertex| place[vertex as usize]))
        .collect();
    mesh.normals = Some(normals);
    mesh.colors = colored.then_some(colors);
    Ok((mesh, faults))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh_quality::mesh_topology;
    use crate::region_source::{resident_points, SourceTransform};
    use crate::test_shapes::{
        box_room, cylinder, indexed_cloud, plane_with_hole, sphere, CylinderSpec, IndexedCloud,
        Noise, Opening, Rng, RoomSpec, Shape, Wall,
    };
    use crate::{Point, ScanRange};

    /// Points as an indexed layer.
    fn layer(shape: &Shape) -> IndexedCloud {
        indexed_cloud(&shape.cloud_points(), 4_096)
    }

    /// The scans of a shape as a reader records them.
    fn ranges(shape: &Shape) -> Vec<ScanRange> {
        shape
            .station_ranges()
            .into_iter()
            .map(|(first_ordinal, station)| ScanRange {
                first_ordinal,
                station: (station != u32::MAX).then_some(station),
            })
            .collect()
    }

    fn source<'a>(cloud: &'a IndexedCloud, shape: &Shape) -> SurfelSource<'a> {
        SurfelSource::with_stations(
            RegionSource::new(&cloud.cloud, Some(&cloud.index), SourceTransform::default()),
            shape.stations.clone(),
            ranges(shape),
        )
    }

    fn everything(_: usize, _: u64, _: &Point) -> bool {
        true
    }

    fn mesh_of(
        cloud: &IndexedCloud,
        shape: &Shape,
        config: &ClosedMeshConfig,
    ) -> (MeshGeometry, ClosedMeshReport) {
        let sources = [source(cloud, shape)];
        mesh_closed(&sources, None, &everything, config, &mut |_| Ok(())).unwrap()
    }

    fn run(shape: &Shape, config: &ClosedMeshConfig) -> (MeshGeometry, ClosedMeshReport) {
        mesh_of(&layer(shape), shape, config)
    }

    /// Voxels of 4 cm and no simplification: what the surface extraction
    /// itself gives.
    fn raw() -> ClosedMeshConfig {
        ClosedMeshConfig {
            voxel: Some(0.04),
            simplify_tolerance: Some(0.0),
            ..ClosedMeshConfig::default()
        }
    }

    /// A room of 4.0 x 3.0 x 2.6 m seen from inside, a point every 1.5 cm.
    fn room() -> Shape {
        box_room(&RoomSpec {
            spacing: 0.015,
            ..RoomSpec::default()
        })
    }

    const ROOM_MIDDLE: [f64; 3] = [2.0, 1.5, 1.3];

    fn length(v: [f64; 3]) -> f64 {
        dot3(v, v).sqrt()
    }

    fn corners(mesh: &MeshGeometry, triangle: &[u32; 3]) -> [[f64; 3]; 3] {
        triangle.map(|vertex| mesh.vertices[vertex as usize])
    }

    /// Normal of a triangle with the length of twice its area.
    fn face(mesh: &MeshGeometry, triangle: &[u32; 3]) -> [f64; 3] {
        let [a, b, c] = corners(mesh, triangle);
        cross(difference(b, a), difference(c, a))
    }

    fn area(mesh: &MeshGeometry) -> f64 {
        mesh.triangles
            .iter()
            .map(|triangle| 0.5 * length(face(mesh, triangle)))
            .sum()
    }

    /// Volume enclosed by a closed mesh: positive when the triangles face
    /// outward, negative when they face the inside.
    fn volume(mesh: &MeshGeometry, inside: [f64; 3]) -> f64 {
        mesh.triangles
            .iter()
            .map(|triangle| {
                let [a, _, _] = corners(mesh, triangle);
                dot3(difference(a, inside), face(mesh, triangle)) / 6.0
            })
            .sum()
    }

    /// Share of the area whose front looks at a point, and share of the
    /// vertices whose normal does.
    fn facing(mesh: &MeshGeometry, target: [f64; 3]) -> (f64, f64) {
        let mut towards = 0.0;
        for triangle in &mesh.triangles {
            let normal = face(mesh, triangle);
            let [a, _, _] = corners(mesh, triangle);
            if dot3(normal, difference(target, a)) > 0.0 {
                towards += 0.5 * length(normal);
            }
        }
        let normals = mesh.normals.as_ref().unwrap();
        let vertices = mesh
            .vertices
            .iter()
            .zip(normals)
            .filter(|(vertex, normal)| {
                dot3(normal.map(f64::from), difference(target, **vertex)) > 0.0
            })
            .count();
        (
            towards / area(mesh),
            vertices as f64 / mesh.vertices.len() as f64,
        )
    }

    /// Open edges of a mesh as pairs of positions.
    fn open_edges(mesh: &MeshGeometry) -> Vec<([f64; 3], [f64; 3])> {
        let mut edges: Vec<(u32, u32)> = Vec::new();
        for [a, b, c] in &mesh.triangles {
            for (from, to) in [(*a, *b), (*b, *c), (*c, *a)] {
                edges.push((from.min(to), from.max(to)));
            }
        }
        edges.sort_unstable();
        edges
            .chunk_by(|a, b| a == b)
            .filter(|run| run.len() == 1)
            .map(|run| {
                (
                    mesh.vertices[run[0].0 as usize],
                    mesh.vertices[run[0].1 as usize],
                )
            })
            .collect()
    }

    /// Triangles that use the same three vertices as another one.
    fn doubled_faces(mesh: &MeshGeometry) -> usize {
        let mut faces: Vec<[u32; 3]> = mesh
            .triangles
            .iter()
            .map(|triangle| {
                let mut sorted = *triangle;
                sorted.sort_unstable();
                sorted
            })
            .collect();
        faces.sort_unstable();
        faces.windows(2).filter(|pair| pair[0] == pair[1]).count()
    }

    fn bounds(mesh: &MeshGeometry) -> Bounds {
        let mut bounds = Bounds {
            min: [f64::INFINITY; 3],
            max: [f64::NEG_INFINITY; 3],
        };
        for vertex in &mesh.vertices {
            bounds.include(*vertex);
        }
        bounds
    }

    /// The vertices of a mesh as bits, in a fixed order: equal for two
    /// meshes whose tiles, and so the order of their vertices, differ.
    fn sorted_vertices(mesh: &MeshGeometry) -> Vec<[u64; 3]> {
        let mut vertices: Vec<[u64; 3]> = mesh
            .vertices
            .iter()
            .map(|vertex| vertex.map(f64::to_bits))
            .collect();
        vertices.sort_unstable();
        vertices
    }

    /// Whether two meshes are the same down to the last bit.
    fn same(a: &MeshGeometry, b: &MeshGeometry) -> bool {
        let bits = |mesh: &MeshGeometry| -> Vec<[u64; 3]> {
            mesh.vertices
                .iter()
                .map(|vertex| vertex.map(f64::to_bits))
                .collect()
        };
        let normals = |mesh: &MeshGeometry| -> Vec<[u32; 3]> {
            mesh.normals
                .as_ref()
                .unwrap()
                .iter()
                .map(|normal| normal.map(f32::to_bits))
                .collect()
        };
        bits(a) == bits(b)
            && a.triangles == b.triangles
            && a.colors == b.colors
            && normals(a) == normals(b)
    }

    /// One closed piece without handles, and nothing that overlaps.
    fn assert_closed(mesh: &MeshGeometry, report: &ClosedMeshReport) {
        assert_eq!(
            report.topology,
            MeshTopology {
                open_edges: 0,
                non_manifold_edges: 0,
                components: 1,
                euler: 2,
            }
        );
        assert_sound(mesh, report);
    }

    /// What holds for every result: the report describes the mesh, no face
    /// lies on another, and every vertex has a normal of unit length.
    fn assert_sound(mesh: &MeshGeometry, report: &ClosedMeshReport) {
        assert_eq!(report.topology, mesh_topology(mesh));
        assert_eq!(report.topology.non_manifold_edges, 0);
        assert_eq!(report.seam_faults, 0);
        assert_eq!(doubled_faces(mesh), 0);
        assert_eq!(report.vertices, mesh.vertices.len());
        assert_eq!(report.triangles, mesh.triangles.len());
        let normals = mesh.normals.as_ref().unwrap();
        assert_eq!(normals.len(), mesh.vertices.len());
        assert!(normals
            .iter()
            .all(|normal| (squared(*normal).sqrt() - 1.0).abs() < 1e-3));
        assert!(mesh.triangles.iter().all(|triangle| triangle
            .iter()
            .all(|vertex| (*vertex as usize) < mesh.vertices.len())));
    }

    /// Mean and 95th percentile distance from the points to the mesh stay
    /// under bounds given in millimetres.
    fn assert_deviation(report: &ClosedMeshReport, mean: f64, p95: f64, max: f64) {
        let found = report.deviation;
        assert!(
            found.mean < mean * 1e-3 && found.p95 < p95 * 1e-3 && found.max < max * 1e-3,
            "{found:?}"
        );
    }

    /// Voxels of 2 cm and no simplification.
    fn fine() -> ClosedMeshConfig {
        ClosedMeshConfig {
            voxel: Some(0.02),
            simplify_tolerance: Some(0.0),
            ..ClosedMeshConfig::default()
        }
    }

    /// Share of the area whose front looks along a direction.
    fn along(mesh: &MeshGeometry, direction: [f64; 3]) -> f64 {
        let towards: f64 = mesh
            .triangles
            .iter()
            .map(|triangle| face(mesh, triangle))
            .filter(|normal| dot3(*normal, direction) > 0.0)
            .map(|normal| 0.5 * length(normal))
            .sum();
        towards / area(mesh)
    }

    /// The points of a shape that pass a test, with their stations.
    fn part(shape: &Shape, keep: &dyn Fn(usize, [f64; 3]) -> bool) -> Shape {
        let mut part = Shape {
            stations: shape.stations.clone(),
            ..Shape::default()
        };
        for index in 0..shape.points.len() {
            if keep(index, shape.points[index]) {
                part.points.push(shape.points[index]);
                part.normals.push(shape.normals[index]);
                part.station_of.push(shape.station_of[index]);
            }
        }
        part
    }

    /// How far the vertex furthest from every plane of the test room lies
    /// from the nearest of them.
    fn off_the_room(mesh: &MeshGeometry) -> f64 {
        let size = [4.0, 3.0, 2.6];
        mesh.vertices
            .iter()
            .map(|vertex| {
                (0..3)
                    .map(|axis| vertex[axis].abs().min((size[axis] - vertex[axis]).abs()))
                    .fold(f64::INFINITY, f64::min)
            })
            .fold(0.0, f64::max)
    }

    // The bounds in these tests were measured at a voxel of 4 cm and carry
    // a margin of about a quarter: the room came to a mean of 0.44 mm, a
    // 95th percentile of 4.0 mm and a largest distance of 10.3 mm, which is
    // at the corners, where the surface is rounded.
    #[test]
    fn box_room_is_closed_and_accurate() {
        let room = room();
        let (mesh, report) = run(&room, &raw());
        assert_closed(&mesh, &report);
        assert_eq!(report.voxel, 0.04);
        assert_eq!(report.max_hole, 0.25);
        assert_eq!(report.simplify_tolerance, 0.0);
        assert_eq!(report.points, room.points.len() as u64);
        assert_eq!(report.orientation, OrientationUsed::Stations);
        assert_eq!(report.surfels_by_station, report.surfels);
        assert_eq!(report.deviation.samples, 200_000);
        assert_eq!(report.triangles_extracted, mesh.triangles.len() as u64);
        assert!(mesh.colors.is_some());
        assert_deviation(&report, 0.6, 5.1, 13.5);
        // Seen from inside: every face and every normal looks into the
        // room, so the enclosed volume counts as negative. 31.2 m3 in
        // truth; measured 31.218.
        assert_eq!(facing(&mesh, ROOM_MIDDLE), (1.0, 1.0));
        assert!((volume(&mesh, ROOM_MIDDLE) + 31.2).abs() < 0.06);
        // No vertex further than 10.3 mm from the planes of the room.
        let size = [4.0, 3.0, 2.6];
        assert!(mesh.vertices.iter().all(|vertex| {
            let nearest = (0..3)
                .map(|axis| vertex[axis].min(size[axis] - vertex[axis]))
                .fold(f64::INFINITY, f64::min);
            nearest.abs() < 0.0135
        }));

        // With 2 mm of noise: mean 1.29 mm, 95th percentile 4.0 mm, largest
        // 12.3 mm. The
        // walls lie on lattice planes here, so noise decides the side of
        // whole layers of corners; the surface must stay in one piece.
        let noisy = room.with_noise(Noise::Uniform(0.002), 7);
        let (mesh, report) = run(&noisy, &raw());
        assert_closed(&mesh, &report);
        assert_deviation(&report, 1.65, 5.1, 16.0);
        assert_eq!(facing(&mesh, ROOM_MIDDLE).1, 1.0);
        assert!((volume(&mesh, ROOM_MIDDLE) + 31.2).abs() < 0.06);
    }

    #[test]
    fn sphere_is_closed_and_accurate_from_inside_and_outside() {
        let center = [0.3, 0.2, 0.1];
        let inside = sphere(center, 1.0, 100_000)
            .flipped()
            .with_stations(&[center]);
        let (mesh, report) = run(&inside, &raw());
        assert_closed(&mesh, &report);
        // Measured: mean 0.64 mm, 95th percentile 0.83 mm, largest 1.1 mm,
        // all outside the sphere: averaging over a curved surface pushes
        // the result out by the voxel squared over three times the radius.
        assert_deviation(&report, 0.85, 1.1, 1.5);
        assert!(mesh.vertices.iter().all(|vertex| {
            let radius = length(difference(*vertex, center));
            radius > 1.0 && radius < 1.0016
        }));
        assert_eq!(facing(&mesh, center), (1.0, 1.0));
        // Area 12.566 m2 and volume 4.189 m3 in truth; measured 12.584
        // and 4.197.
        let sphere_area = 4.0 * std::f64::consts::PI;
        assert!((area(&mesh) - sphere_area).abs() < 0.004 * sphere_area);
        assert!((volume(&mesh, center) + sphere_area / 3.0).abs() < 0.004 * sphere_area / 3.0);

        // The same sphere scanned from six stations around it faces out.
        let around: Vec<[f64; 3]> = (0..6)
            .map(|side| {
                let mut station = center;
                station[side / 2] += if side % 2 == 0 { 5.0 } else { -5.0 };
                station
            })
            .collect();
        let outside = sphere(center, 1.0, 100_000).with_stations(&around);
        let (mesh, report) = run(&outside, &raw());
        assert_closed(&mesh, &report);
        assert_deviation(&report, 0.85, 1.1, 1.5);
        assert_eq!(facing(&mesh, center), (0.0, 0.0));
        assert!((volume(&mesh, center) - sphere_area / 3.0).abs() < 0.004 * sphere_area / 3.0);
    }

    #[test]
    fn cylinder_from_outside_stations_faces_outward() {
        let spec = CylinderSpec {
            caps: true,
            spacing: 0.01,
            ..CylinderSpec::column([0.0, 0.0, 0.0], 0.5, 2.0)
        };
        let column = cylinder(&spec).with_stations(&spec.stations_around(3.0));
        // Six scans, one after another in the file.
        assert_eq!(ranges(&column).len(), 6);
        let middle = [0.0, 0.0, 1.0];
        let (mesh, report) = run(&column, &raw());
        assert_closed(&mesh, &report);
        assert_eq!(report.orientation, OrientationUsed::Stations);
        // Measured: mean 0.96 mm, 95th percentile 4.1 mm, largest 7.9 mm
        // at the rims of the two ends.
        assert_deviation(&report, 1.25, 5.2, 10.5);
        assert_eq!(facing(&mesh, middle), (0.0, 0.0));
        // Volume 1.5708 m3 and area 7.854 m2 in truth; measured 1.5776
        // and 7.844.
        assert!((volume(&mesh, middle) - std::f64::consts::FRAC_PI_2).abs() < 0.012);
        assert!((area(&mesh) - 7.854).abs() < 0.04);

        // The sides come from the scan ranges: with the stations of the
        // scans mixed up, the same points face the wrong way.
        let mut mixed = column.clone();
        mixed.stations.rotate_left(2);
        let (mesh, _) = run(&mixed, &raw());
        assert!(facing(&mesh, middle).0 > 0.9);
    }

    #[test]
    fn a_room_with_walls_a_door_and_a_window_is_one_surface() {
        // Walls of 20 cm with their outside faces scanned from four
        // stations around the room, and the reveals of a door and a window
        // seen from the station inside, some at a low angle; 1.5 mm noise.
        let spec = RoomSpec {
            spacing: 0.016,
            wall_thickness: Some(0.2),
            openings: vec![
                Opening::door(Wall::South, 1.0),
                Opening::window(Wall::East, 0.8),
            ],
            ..RoomSpec::default()
        };
        let building = box_room(&spec).with_noise(Noise::Gaussian(0.0015), 4);
        assert_eq!(building.stations.len(), 5);
        let (mesh, report) = run(
            &building,
            &ClosedMeshConfig {
                voxel: Some(0.02),
                simplify_tolerance: Some(0.0),
                ..ClosedMeshConfig::default()
            },
        );
        assert_sound(&mesh, &report);
        assert_eq!(report.orientation, OrientationUsed::Stations);
        // The inside of the room and the outside of the walls, joined
        // through the two openings: one piece, open only where the walls
        // end at the top and at the bottom.
        assert_eq!((report.topology.components, report.topology.euler), (1, -2));
        let rim = open_edges(&mesh);
        assert!(rim
            .iter()
            .all(|(a, b)| a[2].max(b[2]) < 0.03 || a[2].min(b[2]) > 2.57));
        // Measured: mean 1.09 mm, 95th percentile 2.7 mm, largest 8.2 mm.
        assert_deviation(&report, 1.4, 3.5, 11.0);
        // Every face looks at free space: the reveals into their opening,
        // the outside of the east wall away from the room.
        let normals = mesh.normals.as_ref().unwrap();
        let mut checked = [0usize; 5];
        for (vertex, normal) in mesh.vertices.iter().zip(normals) {
            let [x, y, z] = *vertex;
            let in_wall = x > 4.03 && x < 4.17;
            let (across, expected) = if in_wall && (y - 0.8).abs() < 0.004 && z > 1.0 && z < 2.0 {
                (0, normal[1])
            } else if in_wall && (y - 2.0).abs() < 0.004 && z > 1.0 && z < 2.0 {
                (1, -normal[1])
            } else if in_wall && (z - 0.9).abs() < 0.004 && y > 0.9 && y < 1.9 {
                (2, normal[2])
            } else if in_wall && (z - 2.1).abs() < 0.004 && y > 0.9 && y < 1.9 {
                (3, -normal[2])
            } else if (x - 4.2).abs() < 0.004 && y > 0.1 && y < 0.7 && z > 0.1 && z < 2.5 {
                (4, normal[0])
            } else {
                continue;
            };
            checked[across] += 1;
            assert!(expected > 0.95, "{vertex:?} {normal:?}");
        }
        assert!(checked.iter().all(|count| *count > 200), "{checked:?}");
    }

    #[test]
    fn tiles_join_without_seams() {
        let room = room();
        let cloud = layer(&room);
        let (one, report) = mesh_of(
            &cloud,
            &room,
            &ClosedMeshConfig {
                tile_voxels: 256,
                ..raw()
            },
        );
        assert_eq!((report.tiles_planned, report.tiles), (1, 1));
        let (many, report) = mesh_of(
            &cloud,
            &room,
            &ClosedMeshConfig {
                tile_voxels: 32,
                ..raw()
            },
        );
        assert!(report.tiles >= 30);
        assert_closed(&many, &report);
        // The same vertices, bit for bit, and as many triangles: no crack
        // and no doubled face along any seam.
        assert_eq!(sorted_vertices(&one), sorted_vertices(&many));
        assert_eq!(one.triangles.len(), many.triangles.len());
        assert_eq!(report.deviation.samples, 200_000);

        // Simplified per tile and once more across the seams, the mesh is
        // still closed.
        let (mesh, report) = mesh_of(
            &cloud,
            &room,
            &ClosedMeshConfig {
                tile_voxels: 32,
                simplify_tolerance: None,
                ..raw()
            },
        );
        assert_closed(&mesh, &report);
        assert!(report.seams_simplified);
    }

    #[test]
    fn simplification_keeps_the_shape_with_far_fewer_triangles() {
        let room = room();
        let cloud = layer(&room);
        // Every face of the test room has a colour of its own. First
        // without regard to colour: only the shape counts.
        let config = ClosedMeshConfig {
            voxel: Some(0.04),
            ..ClosedMeshConfig::default()
        };
        assert_eq!(config.color_step, 255);
        let (mesh, report) = mesh_of(&cloud, &room, &config);
        assert_closed(&mesh, &report);
        // 0.15 voxel unless the caller says otherwise.
        assert!((report.simplify_tolerance - 0.006).abs() < 1e-12);
        // Measured: 618 of 77,444 triangles left, mean 0.78 mm, 95th
        // percentile 4.2 mm and largest 10.2 mm. The walls of this room lie
        // on lattice planes, where rounding decides the side of a corner:
        // another machine may extract up to half as many triangles more
        // and keep some of them, as the room with noise does.
        assert!(
            (75_000..125_000).contains(&report.triangles_extracted),
            "{}",
            report.triangles_extracted
        );
        assert!(mesh.triangles.len() < 2_000, "{}", mesh.triangles.len());
        assert_deviation(&report, 1.0, 5.4, 13.5);
        assert_eq!(facing(&mesh, ROOM_MIDDLE), (1.0, 1.0));
        assert!((volume(&mesh, ROOM_MIDDLE) + 31.2).abs() < 0.1);

        // With regard to colour the vertices along the edges of the room,
        // where one colour meets another, stay: measured 6,280 triangles,
        // mean 0.64 mm.
        let colored = ClosedMeshConfig {
            voxel: Some(0.04),
            color_step: 24,
            ..ClosedMeshConfig::default()
        };
        let (mesh, report) = mesh_of(&cloud, &room, &colored);
        assert_closed(&mesh, &report);
        assert!(mesh.triangles.len() < 8_500, "{}", mesh.triangles.len());
        assert_deviation(&report, 0.85, 5.4, 13.5);

        // A room that does not follow the axes, at national grid
        // coordinates: 3,316 of 89,012 triangles left, mean 0.66 mm, 95th
        // percentile 3.5 mm, largest 12.5 mm.
        let turned = room.transformed(17.3, [207_000.0, 474_000.0, 2.0]);
        let (mesh, report) = run(&turned, &config);
        assert_closed(&mesh, &report);
        assert!(mesh.triangles.len() < 5_000, "{}", mesh.triangles.len());
        assert_deviation(&report, 0.85, 4.5, 16.0);
    }

    #[test]
    fn a_hole_under_the_limit_is_closed_and_a_wider_one_stays_open() {
        // A floor of 2 x 2 m with a round hole of 0.20 m.
        let plane = plane_with_hole([2.0, 2.0], 0.01, Some(([1.0, 1.0], 0.1)))
            .with_stations(&[[1.0, 1.0, 1.5]]);
        let cloud = layer(&plane);
        let rim = |mesh: &MeshGeometry| -> (f64, f64) {
            let (mut inner, mut outer) = (0.0, 0.0);
            for (a, b) in open_edges(mesh) {
                if (a[0] - 1.0).hypot(a[1] - 1.0) < 0.5 {
                    inner += length(difference(a, b));
                } else {
                    outer += length(difference(a, b));
                }
            }
            (inner, outer)
        };
        let config = |max_hole: f64| ClosedMeshConfig {
            voxel: Some(0.02),
            max_hole,
            simplify_tolerance: Some(0.0),
            ..ClosedMeshConfig::default()
        };
        let (closed, report) = mesh_of(&cloud, &plane, &config(0.30));
        assert_sound(&closed, &report);
        assert_eq!(report.max_hole, 0.30);
        let (inner, outer) = rim(&closed);
        assert_eq!(inner, 0.0);
        // The floor ends within a voxel of its last points: measured 15 mm.
        assert!((outer - 8.08).abs() < 0.01, "{outer}");
        let found = bounds(&closed);
        assert!(found.min[0] > -0.02 && found.max[0] < 2.02);
        assert_eq!(report.topology.euler, 1);
        // Across the hole the floor runs on flat.
        assert!(closed
            .vertices
            .iter()
            .any(|vertex| (vertex[0] - 1.0).hypot(vertex[1] - 1.0) < 0.02));
        assert!(closed.vertices.iter().all(|vertex| vertex[2].abs() < 1e-6));

        let (open, report) = mesh_of(&cloud, &plane, &config(0.10));
        assert_sound(&open, &report);
        let (inner, outer) = rim(&open);
        // The rim of the hole follows the lattice in steps, which makes it
        // longer than the circle of 0.63 m: measured 0.72 m.
        assert!(inner > 0.6 && inner < 0.85, "{inner}");
        assert!((outer - 8.08).abs() < 0.01, "{outer}");
        assert_eq!(report.topology.euler, 0);
        assert!(!open
            .vertices
            .iter()
            .any(|vertex| (vertex[0] - 1.0).hypot(vertex[1] - 1.0) < 0.07));

        // How wide a hole may be for a limit: measured with voxels of 2 cm
        // and a limit of 0.25 m, round holes close up to 0.28 m across and
        // stay open from 0.30 m, and a long gap closes up to 0.24 m wide.
        for (diameter, closes) in [(0.26, true), (0.32, false)] {
            let plane = plane_with_hole([2.0, 2.0], 0.01, Some(([1.0, 1.0], diameter / 2.0)))
                .with_stations(&[[1.0, 1.0, 1.5]]);
            let (_, report) = run(&plane, &config(0.25));
            assert_eq!(report.topology.euler, i64::from(closes), "{diameter}");
        }

        // Nothing is closed without a limit, and the limit is cut back to
        // 32 voxels.
        let (open, report) = mesh_of(&cloud, &plane, &config(0.0));
        assert_eq!(report.topology.euler, 0);
        assert!(rim(&open).0 > 0.6);
        let (_, report) = mesh_of(&cloud, &plane, &config(3.2));
        assert!((report.max_hole - 0.64).abs() < 1e-12);
        assert_eq!(report.topology.euler, 1);
    }

    #[test]
    fn gaps_and_sparse_points_come_out_the_same_in_every_tiling() {
        // A floor with noise that follows no axis, with a hole of 0.20 m in
        // it, and beside it a floor with a point every 6 cm only.
        let dense = plane_with_hole([2.4, 2.4], 0.012, Some(([1.1, 1.3], 0.1)))
            .with_noise(Noise::Gaussian(0.0015), 9);
        let mut sparse = plane_with_hole([1.8, 2.4], 0.06, None);
        for point in &mut sparse.points {
            point[0] += 3.2;
        }
        let both = dense
            .merged(sparse)
            .with_stations(&[[1.5, 1.2, 2.0]])
            .transformed(23.0, [207_000.0, 474_000.0, 3.21]);
        let cloud = layer(&both);
        for max_hole in [0.30, 0.10, 0.0] {
            let config = |tile_voxels: u32| ClosedMeshConfig {
                voxel: Some(0.03),
                max_hole,
                tile_voxels,
                ..raw()
            };
            let (one, one_report) = mesh_of(&cloud, &both, &config(256));
            assert_eq!(one_report.tiles, 1);
            assert_sound(&one, &one_report);
            // The dense floor and the sparse one, the first with its hole
            // unless the limit closes it.
            let handles = if max_hole > 0.2 { 2 } else { 1 };
            assert_eq!(
                (one_report.topology.components, one_report.topology.euler),
                (2, handles),
                "{max_hole}"
            );
            for tile_voxels in [16, 40] {
                let (many, report) = mesh_of(&cloud, &both, &config(tile_voxels));
                assert!(report.tiles > 8);
                assert_eq!(
                    sorted_vertices(&one),
                    sorted_vertices(&many),
                    "{max_hole} {tile_voxels}"
                );
                assert_eq!(one.triangles.len(), many.triangles.len());
                assert_eq!(report.topology, one_report.topology);
                assert_eq!(report.deviation, one_report.deviation);
            }
        }
    }

    #[test]
    fn a_thin_wall_gives_two_sheets() {
        // Two faces 10 cm apart, each seen from its own side.
        let top = plane_with_hole([2.0, 2.0], 0.01, None).with_stations(&[[1.0, 1.0, 2.0]]);
        let mut bottom = plane_with_hole([2.0, 2.0], 0.01, None)
            .flipped()
            .with_stations(&[[1.0, 1.0, -2.0]]);
        for point in &mut bottom.points {
            point[2] -= 0.10;
        }
        let wall = top.merged(bottom);
        let (mesh, report) = run(
            &wall,
            &ClosedMeshConfig {
                voxel: Some(0.02),
                ..raw()
            },
        );
        assert_sound(&mesh, &report);
        assert_eq!(report.topology.components, 2);
        let normals = mesh.normals.as_ref().unwrap();
        for (vertex, normal) in mesh.vertices.iter().zip(normals) {
            if vertex[2] > -0.05 {
                assert!(vertex[2].abs() < 1e-6 && normal[2] > 0.999);
            } else {
                assert!((vertex[2] + 0.10).abs() < 1e-6 && normal[2] < -0.999);
            }
        }
    }

    #[test]
    fn sparse_points_are_meshed_at_a_coarser_level() {
        // A point every two voxels: too few for a plane fit among cells of
        // half a voxel.
        let plane = plane_with_hole([2.0, 2.0], 0.04, None).with_stations(&[[1.0, 1.0, 2.0]]);
        let (mesh, report) = run(
            &plane,
            &ClosedMeshConfig {
                voxel: Some(0.02),
                ..raw()
            },
        );
        assert_sound(&mesh, &report);
        // Every element found its plane one level up, where a cell is a
        // voxel, and took its side from the station.
        assert_eq!((report.surfels, report.surfels_by_station), (2_500, 2_500));
        assert_eq!(report.orientation, OrientationUsed::Stations);
        assert_eq!((report.topology.components, report.topology.euler), (1, 1));
        assert!(mesh.vertices.iter().all(|vertex| vertex[2].abs() < 1e-6));
        // It runs on further past the last points than a dense scan does:
        // measured 5 cm, 2.5 voxels.
        assert!((area(&mesh) - 4.402).abs() < 0.01);
        assert!(mesh
            .normals
            .as_ref()
            .unwrap()
            .iter()
            .all(|normal| normal[2] > 0.999));
    }

    #[test]
    fn colours_follow_the_points() {
        // A floor that is red on one side and blue on the other.
        let plane = plane_with_hole([2.0, 2.0], 0.01, None).with_stations(&[[1.0, 1.0, 2.0]]);
        let mut points = plane.cloud_points();
        for point in &mut points {
            point.rgb = Some(if point.xyz[0] < 1.0 {
                [200, 30, 20]
            } else {
                [10, 40, 220]
            });
        }
        let cloud = indexed_cloud(&points, 4_096);
        let (mesh, _) = mesh_of(&cloud, &plane, &raw());
        let colors = mesh.colors.as_ref().unwrap();
        assert_eq!(colors.len(), mesh.vertices.len());
        let mut checked = 0;
        for (vertex, color) in mesh.vertices.iter().zip(colors) {
            // Three voxels from the border the other side has no say.
            let expected = match vertex[0] {
                x if x < 1.0 - 0.12 => [200, 30, 20],
                x if x > 1.0 + 0.12 => [10, 40, 220],
                _ => continue,
            };
            checked += 1;
            assert_eq!(*color, expected);
        }
        assert!(checked > 2_000);
        // Simplified with regard to colour, each side becomes a few large
        // triangles in its own colour, and the vertices along the border
        // stay.
        let simplified = ClosedMeshConfig {
            simplify_tolerance: None,
            color_step: 24,
            ..raw()
        };
        let (kept, _) = mesh_of(&cloud, &plane, &simplified);
        assert!(kept.vertices.len() < mesh.vertices.len() / 4);
        let mut near_border = 0;
        for (vertex, color) in kept.vertices.iter().zip(kept.colors.as_ref().unwrap()) {
            if (vertex[0] - 1.0).abs() > 0.12 {
                let expected = if vertex[0] < 1.0 {
                    [200, 30, 20]
                } else {
                    [10, 40, 220]
                };
                assert_eq!(*color, expected, "{vertex:?}");
            } else {
                near_border += 1;
            }
        }
        assert!(near_border > 100);
        // Told to simplify whatever the colour, the floor becomes two
        // triangles whose corners mix both sides.
        let (plain, _) = mesh_of(
            &cloud,
            &plane,
            &ClosedMeshConfig {
                color_step: 255,
                ..simplified
            },
        );
        assert_eq!(plain.triangles.len(), 2);

        // Points without colour give a mesh without.
        for point in &mut points {
            point.rgb = None;
        }
        let cloud = indexed_cloud(&points, 4_096);
        let (mesh, _) = mesh_of(&cloud, &plane, &raw());
        assert!(mesh.colors.is_none());
    }

    #[test]
    fn the_filter_and_the_region_limit_what_is_meshed() {
        let room = room();
        let cloud = layer(&room);
        let sources = [source(&cloud, &room)];
        let config = raw();
        // Leave out one half, as a selection or deleted points would.
        let (mesh, report) = mesh_closed(
            &sources,
            None,
            &|source, _, point| source == 0 && point.xyz[0] <= 2.0,
            &config,
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_sound(&mesh, &report);
        let kept = room.points.iter().filter(|point| point[0] <= 2.0).count();
        assert_eq!(report.points, kept as u64);
        // The surface ends within a voxel of the last points, and every
        // open edge lies there.
        let found = bounds(&mesh);
        assert!(found.max[0] > 2.0 && found.max[0] < 2.04, "{found:?}");
        let rim = open_edges(&mesh);
        assert!(!rim.is_empty());
        assert!(rim.iter().all(|(a, b)| a[0] > 1.96 && b[0] > 1.96));

        // Every other point by its ordinal: still a closed room.
        let (thinned, report) = mesh_closed(
            &sources,
            None,
            &|_, ordinal, _| ordinal % 2 == 0,
            &config,
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_closed(&thinned, &report);
        assert_eq!(report.points, room.points.len().div_ceil(2) as u64);

        // A section box does the same as the filter on the same half.
        let region = Bounds {
            min: [-1.0, -1.0, -1.0],
            max: [2.0, 4.0, 3.0],
        };
        let (boxed, boxed_report) = mesh_closed(
            &sources,
            Some(region),
            &everything,
            &config,
            &mut |_| Ok(()),
        )
        .unwrap();
        assert_sound(&boxed, &boxed_report);
        // The box is cut back to where the room has points.
        let all = room.bounds();
        assert_eq!(
            boxed_report.region,
            Bounds {
                min: all.min,
                max: [2.0, all.max[1], all.max[2]],
            }
        );
        assert_eq!(boxed_report.points, kept as u64);
        let found = bounds(&boxed);
        assert!(found.max[0] > 2.0 && found.max[0] < 2.04, "{found:?}");
        assert!(open_edges(&boxed)
            .iter()
            .all(|(a, b)| a[0] > 1.96 && b[0] > 1.96));
    }

    #[test]
    fn the_work_follows_the_region_and_not_the_cloud() {
        // A floor of 40 x 2 m; the box holds a twentieth of it.
        let floor = plane_with_hole([40.0, 2.0], 0.02, None).with_stations(&[[20.0, 1.0, 2.0]]);
        let cloud = indexed_cloud(&floor.cloud_points(), 1_024);
        let sources = [source(&cloud, &floor)];
        let region = Bounds {
            min: [10.0, -1.0, -1.0],
            max: [12.0, 3.0, 1.0],
        };
        let (mesh, report) =
            mesh_closed(&sources, Some(region), &everything, &raw(), &mut |_| Ok(())).unwrap();
        assert_sound(&mesh, &report);
        assert_eq!(report.points, 10_000);
        // Only the leaves of the index that touch the box are read, once
        // by each of the two tiles the box lies in: measured 39,000 of the
        // 200,000 points.
        assert!(
            report.points_read < floor.points.len() as u64 / 4,
            "{}",
            report.points_read
        );
        let found = bounds(&mesh);
        assert!(found.min[0] > 9.96 && found.max[0] < 12.04, "{found:?}");
        assert!((area(&mesh) - 2.04 * 2.04).abs() < 0.1, "{}", area(&mesh));
    }

    #[test]
    fn without_stations_the_fallback_decides_the_side() {
        let room = room();
        let cloud = layer(&room);
        let unknown = room.clone().with_stations(&[]);
        // Towards the middle of the region: right for a room.
        let (mesh, report) = mesh_of(&cloud, &unknown, &raw());
        assert_closed(&mesh, &report);
        assert_eq!(report.orientation, OrientationUsed::Fallback);
        assert_eq!(report.surfels_by_station, 0);
        assert_eq!(facing(&mesh, ROOM_MIDDLE), (1.0, 1.0));
        // The same when the stations are known but not to be used.
        let (ignored, report) = mesh_of(
            &cloud,
            &room,
            &ClosedMeshConfig {
                use_stations: false,
                ..raw()
            },
        );
        assert_eq!(report.orientation, OrientationUsed::Fallback);
        assert!(same(&mesh, &ignored));
        // A point given by the caller does the same as the middle it is.
        let (pointed, _) = mesh_of(
            &cloud,
            &unknown,
            &ClosedMeshConfig {
                orientation: MeshOrientation::Towards(report.region.center()),
                ..raw()
            },
        );
        assert!(same(&mesh, &pointed));

        // Upward, for a surface measured from above.
        let ground = plane_with_hole([2.0, 2.0], 0.01, None);
        let (mesh, report) = run(
            &ground,
            &ClosedMeshConfig {
                orientation: MeshOrientation::Upward,
                ..raw()
            },
        );
        assert_eq!(report.orientation, OrientationUsed::Fallback);
        assert!(mesh
            .normals
            .as_ref()
            .unwrap()
            .iter()
            .all(|normal| normal[2] > 0.999));
        assert!(mesh
            .triangles
            .iter()
            .all(|triangle| face(&mesh, triangle)[2] > 0.0));
        // A station below it turns it over; one scan without a station
        // beside it is told apart in the report.
        let below = ground.clone().with_stations(&[[1.0, 1.0, -2.0]]);
        let (mesh, report) = run(&below, &raw());
        assert_eq!(report.orientation, OrientationUsed::Stations);
        assert!(mesh
            .triangles
            .iter()
            .all(|triangle| face(&mesh, triangle)[2] < 0.0));
        let mut beside = ground;
        for point in &mut beside.points {
            point[0] += 3.0;
        }
        let both = below.merged(beside);
        let (mesh, report) = run(
            &both,
            &ClosedMeshConfig {
                orientation: MeshOrientation::Upward,
                ..raw()
            },
        );
        assert_eq!(report.orientation, OrientationUsed::Mixed);
        assert!(report.surfels_by_station > 0 && report.surfels_by_station < report.surfels);
        for triangle in &mesh.triangles {
            let [a, _, _] = corners(&mesh, triangle);
            assert_eq!(face(&mesh, triangle)[2] > 0.0, a[0] > 2.5);
        }
    }

    #[test]
    fn a_single_face_without_stations_keeps_one_side() {
        // The middle of the box around a single face lies in that face, and
        // up says nothing about a wall: the fallback sees them edge on.
        // Taking the sign of what it sees there gave, on this floor, half
        // the area facing each way, 28,000 open edges and 18 m2 of mesh.
        let floor = plane_with_hole([3.0, 3.0], 0.012, None).with_noise(Noise::Gaussian(0.0015), 4);
        // The floor stood up as a wall whose normal has an azimuth.
        let wall = |azimuth: f64| -> (Shape, [f64; 3]) {
            let (sin, cos) = azimuth.to_radians().sin_cos();
            let mut wall = floor.clone();
            for (point, normal) in wall.points.iter_mut().zip(&mut wall.normals) {
                let [across, up, off] = *point;
                *point = [-sin * across + cos * off, cos * across + sin * off, up];
                *normal = [cos, sin, 0.0];
            }
            (wall, [cos, sin, 0.0])
        };
        // With the side each takes when nothing tells: up, and for a wall
        // the side of one fixed direction, which the last wall faces away
        // from.
        let faces = [
            (floor.clone(), [0.0, 0.0, 1.0], 1.0),
            (wall(0.0).0, wall(0.0).1, 1.0),
            (wall(45.0).0, wall(45.0).1, 1.0),
            (wall(110.0).0, wall(110.0).1, 1.0),
            (wall(208.0).0, wall(208.0).1, 0.0),
        ];
        for (number, (shape, normal, share)) in faces.iter().enumerate() {
            for orientation in [MeshOrientation::Automatic, MeshOrientation::Upward] {
                let config = ClosedMeshConfig {
                    orientation,
                    ..fine()
                };
                let (mesh, report) = run(shape, &config);
                let case = format!("{number} {orientation:?}");
                assert_sound(&mesh, &report);
                assert_eq!(report.orientation, OrientationUsed::Fallback);
                assert_eq!(
                    (report.topology.components, report.topology.euler),
                    (1, 1),
                    "{case}"
                );
                assert_eq!(along(&mesh, *normal), *share, "{case}");
                // 9.0 m2 in truth; measured 9.12 to 9.15 with the rim.
                assert!(area(&mesh) < 9.3, "{case} {}", area(&mesh));
                // The report says how many elements nothing told the side
                // of: all of them, but for a floor that is to face up.
                if number == 0 && orientation == MeshOrientation::Upward {
                    assert_eq!(report.surfels_by_default, 0);
                } else {
                    assert!(
                        report.surfels_by_default * 100 > report.surfels * 99,
                        "{case} {} of {}",
                        report.surfels_by_default,
                        report.surfels
                    );
                }
            }
        }
        // The side does not depend on the tiles.
        let config = |tile_voxels: u32| ClosedMeshConfig {
            tile_voxels,
            ..fine()
        };
        let cloud = layer(&floor);
        let (one, report) = mesh_of(&cloud, &floor, &config(256));
        assert_eq!(report.tiles, 1);
        let (many, report) = mesh_of(&cloud, &floor, &config(32));
        assert!(report.tiles >= 25, "{}", report.tiles);
        assert_eq!(sorted_vertices(&one), sorted_vertices(&many));
        assert_eq!(one.triangles.len(), many.triangles.len());
    }

    #[test]
    fn a_face_through_the_middle_of_a_room_keeps_one_side() {
        // A shelf at the height of the middle of the room, which the
        // surface faces when there are no stations. Deciding by the sign
        // gave the shelf twice its area and the mesh 1,700 open edges.
        let room = room()
            .with_noise(Noise::Gaussian(0.0015), 5)
            .with_stations(&[]);
        let mut shelf = plane_with_hole([1.2, 0.4], 0.015, None);
        for point in &mut shelf.points {
            *point = [point[0] + 1.4, point[1] + 1.3, 1.30];
        }
        let both = room
            .clone()
            .merged(shelf.with_noise(Noise::Gaussian(0.0015), 9));
        let (mesh, report) = run(&both, &fine());
        assert_sound(&mesh, &report);
        // The closed room and the shelf, a sheet of its own.
        assert_eq!((report.topology.components, report.topology.euler), (2, 3));
        assert!(open_edges(&mesh)
            .iter()
            .all(|(a, b)| (a[2] - 1.30).abs() < 0.01 && (b[2] - 1.30).abs() < 0.01));
        let normals = mesh.normals.as_ref().unwrap();
        let mut on_shelf = 0;
        for (vertex, normal) in mesh.vertices.iter().zip(normals) {
            let on_plan = vertex[0] > 1.3 && vertex[0] < 2.7 && vertex[1] > 1.2 && vertex[1] < 1.8;
            if (vertex[2] - 1.30).abs() < 0.01 && on_plan {
                on_shelf += 1;
                assert!(normal[2] > 0.9, "{vertex:?} {normal:?}");
            }
        }
        assert!(on_shelf > 1_000, "{on_shelf}");
        // The walls, floor and ceiling face the middle as before, so with
        // the shelf every vertex faces a point above it; only the elements
        // of the shelf took the default side. Measured: 2,160.
        assert_eq!(facing(&mesh, [2.0, 1.5, 2.0]).1, 1.0);
        assert!(
            (2_000..2_300).contains(&report.surfels_by_default),
            "{}",
            report.surfels_by_default
        );
    }

    #[test]
    fn upward_gives_every_face_of_a_room_one_side() {
        // Up says nothing about walls. Deciding by the sign of the height
        // of their normals, which is noise, gave 114,000 open edges and
        // 104 m2 of mesh for the 60.4 m2 of this room.
        let room = room()
            .with_noise(Noise::Gaussian(0.0015), 5)
            .with_stations(&[]);
        let (mesh, report) = run(
            &room,
            &ClosedMeshConfig {
                orientation: MeshOrientation::Upward,
                ..fine()
            },
        );
        assert_eq!(report.topology, mesh_topology(&mesh));
        assert_eq!(report.seam_faults, 0);
        // The floor and the ceiling face up, the walls one direction each:
        // opposite walls face the same way, so the faces cannot meet in
        // their edges and the room is open along them. Measured: 2,912
        // open edges, 9 edges with more than two triangles, 61.24 m2.
        assert!(
            report.topology.open_edges < 4_000,
            "{}",
            report.topology.open_edges
        );
        assert!(
            report.topology.non_manifold_edges < 30,
            "{}",
            report.topology.non_manifold_edges
        );
        assert!(area(&mesh) < 62.5, "{}", area(&mesh));
        let size = [4.0, 3.0, 2.6];
        let normals = mesh.normals.as_ref().unwrap();
        let mut checked = [0usize; 3];
        for (vertex, normal) in mesh.vertices.iter().zip(normals) {
            // The axis across the face a vertex lies on, for vertices that
            // lie away from the edges of the room.
            let near: Vec<usize> = (0..3)
                .filter(|axis| vertex[*axis].min(size[*axis] - vertex[*axis]) < 0.1)
                .collect();
            if let [axis] = near[..] {
                checked[axis] += 1;
                assert!(normal[axis] > 0.9, "{vertex:?} {normal:?}");
            }
        }
        assert!(checked.iter().all(|count| *count > 20_000), "{checked:?}");
        // The elements of the walls say that nothing told their side.
        // Measured: 159,588 of 267,722.
        assert!(
            (150_000..170_000).contains(&report.surfels_by_default),
            "{}",
            report.surfels_by_default
        );
    }

    #[test]
    fn a_floor_seen_at_a_low_angle_keeps_one_side() {
        // A floor of 4 x 2 m seen from one station 1.5 m above it at a
        // distance: under four degrees at 20 m and under three at 30 m.
        // The side of an element is the sign of a product that is then as
        // small as the tilt noise gives its fit. Taken per element, the
        // first case had 2,400 to 2,900 open edges, 14 to 32 edges with
        // more than two triangles and 4 to 6 % of its area facing down;
        // the second 1,400 open edges and 5 % facing down.
        for (distance, spacing, noise, voxel) in
            [(20.0, 0.015, 0.003, 0.02), (30.0, 0.020, 0.004, 0.03)]
        {
            let mut floor = plane_with_hole([4.0, 2.0], spacing, None);
            for point in &mut floor.points {
                point[0] += distance;
            }
            let floor = floor
                .with_noise(Noise::Gaussian(noise), 11)
                .with_stations(&[[0.0, 1.0, 1.5]]);
            let (mesh, report) = run(
                &floor,
                &ClosedMeshConfig {
                    voxel: Some(voxel),
                    ..fine()
                },
            );
            assert_sound(&mesh, &report);
            assert_eq!(report.orientation, OrientationUsed::Stations);
            assert_eq!(report.topology.components, 1);
            // Measured over three seeds: all of the area facing up in both
            // cases but for one seed each, with 99.96 and 99.99 %; an
            // Euler characteristic of 1, once -1; 8.15 and 8.22 m2 for
            // the 8.0 m2 of the floor, where the torn floors had 8.7 to
            // 8.9.
            let up = along(&mesh, [0.0, 0.0, 1.0]);
            assert!(up > 0.999, "{distance} {up}");
            assert!(report.topology.euler >= -3, "{}", report.topology.euler);
            assert!(area(&mesh) < 8.4, "{distance} {}", area(&mesh));
        }
    }

    #[test]
    fn points_without_a_station_take_the_side_of_those_beside_them() {
        // A sphere scanned from six stations around it, of which only
        // every fifth point knows its station. The fallback faces the
        // middle of the region, which is the middle of the sphere: the
        // wrong side. Taken per element this gave 4,900 open edges and
        // three quarters of the area facing inward.
        let center = [0.3, 0.2, 0.1];
        let around: Vec<[f64; 3]> = (0..6)
            .map(|side| {
                let mut station = center;
                station[side / 2] += if side % 2 == 0 { 5.0 } else { -5.0 };
                station
            })
            .collect();
        let ball = sphere(center, 1.0, 100_000).with_stations(&around);
        let known = part(&ball, &|index, _| index % 5 == 0);
        let unknown = part(&ball, &|index, _| index % 5 != 0);
        let check = |mesh: &MeshGeometry, report: &ClosedMeshReport| {
            assert_closed(mesh, report);
            assert_eq!(facing(mesh, center), (0.0, 0.0));
            assert_eq!(report.orientation, OrientationUsed::Mixed);
            assert!(report.surfels_by_station * 2 < report.surfels);
            assert_eq!(report.surfels_by_default, 0);
            // As the sphere with all its stations: measured radii of
            // 1.00046 to 1.00115 m and a largest deviation of 1.1 mm.
            assert_deviation(report, 0.85, 1.1, 1.5);
            assert!(mesh.vertices.iter().all(|vertex| {
                let radius = length(difference(*vertex, center));
                radius > 1.0 && radius < 1.0016
            }));
        };
        // As two layers, one with stations and one without.
        let (known_cloud, unknown_cloud) = (layer(&known), layer(&unknown));
        let sources = [
            source(&known_cloud, &known),
            SurfelSource::without_stations(RegionSource::new(
                &unknown_cloud.cloud,
                Some(&unknown_cloud.index),
                SourceTransform::default(),
            )),
        ];
        let (mesh, report) =
            mesh_closed(&sources, None, &everything, &raw(), &mut |_| Ok(())).unwrap();
        check(&mesh, &report);
        // As one file in which the last scan has no station.
        let one = known.merged(unknown.with_stations(&[]));
        assert_eq!(ranges(&one).last().unwrap().station, None);
        let (mesh, report) = run(&one, &raw());
        check(&mesh, &report);
    }

    #[test]
    fn a_gap_beside_another_face_stays_open_and_clean() {
        // Across a gap the surface runs where the coarser elements around
        // it say, which is one mean over all of them. Beside a wall that
        // mean is the plane of neither the floor nor the wall, and closing
        // the gap along it gave, for the first strip here, a non-manifold
        // edge and vertices 39 mm off every plane of the room. Such a gap
        // now stays open. A strip of floor is missing along the south
        // wall, 6 and 16 cm wide: one clean hole each. Measured: 110 and
        // 134 open edges, no vertex more than 6.1 mm off the planes.
        let base = box_room(&RoomSpec {
            spacing: 0.012,
            ..RoomSpec::default()
        });
        let without = |from: f64, to: f64| {
            part(&base, &|_, [x, y, z]| {
                !(z == 0.0 && x > 1.0 && x < 2.0 && y > from && y < to)
            })
            .with_noise(Noise::Gaussian(0.001), 7)
        };
        for width in [0.06, 0.16] {
            let (mesh, report) = run(&without(0.0, width), &fine());
            assert_sound(&mesh, &report);
            assert_eq!(
                (report.topology.components, report.topology.euler),
                (1, 1),
                "{width}"
            );
            assert!(
                report.topology.open_edges < 180,
                "{}",
                report.topology.open_edges
            );
            assert!(open_edges(&mesh)
                .iter()
                .all(|(a, _)| a[0] > 0.9 && a[0] < 2.1 && a[1] < width + 0.06 && a[2] < 0.06));
            assert!(off_the_room(&mesh) < 0.008, "{}", off_the_room(&mesh));
        }
        // The same strip five voxels from the wall is closed as before,
        // flat: measured 5.7 mm off at most, as in the room without a gap.
        let (mesh, report) = run(&without(0.10, 0.26), &fine());
        assert_closed(&mesh, &report);
        assert!(off_the_room(&mesh) < 0.008, "{}", off_the_room(&mesh));

        // A round hole of 16 cm in one face of a thin wall. With the other
        // face 14 cm behind it the hole is closed; with the other face
        // 6 cm behind it the hole stays open, where it used to stay open
        // with its rim lifted 24 mm off the face. Measured: 1.9 mm.
        for (apart, closes) in [(0.14, true), (0.06, false)] {
            let top = plane_with_hole([2.0, 2.0], 0.01, Some(([1.0, 1.0], 0.08)))
                .with_noise(Noise::Gaussian(0.001), 3)
                .with_stations(&[[1.0, 1.0, 2.0]]);
            let mut bottom = plane_with_hole([2.0, 2.0], 0.01, None)
                .flipped()
                .with_noise(Noise::Gaussian(0.001), 4)
                .with_stations(&[[1.0, 1.0, -2.0]]);
            for point in &mut bottom.points {
                point[2] -= apart;
            }
            let (mesh, report) = run(&top.merged(bottom), &fine());
            assert_sound(&mesh, &report);
            assert_eq!(
                (report.topology.components, report.topology.euler),
                (2, 1 + i64::from(closes)),
                "{apart}"
            );
            let lift = mesh
                .vertices
                .iter()
                .map(|vertex| vertex[2].abs().min((vertex[2] + apart).abs()))
                .fold(0.0, f64::max);
            assert!(lift < 0.004, "{apart} {lift}");
        }

        // A curved face counts as one face while its normals stay close
        // together within reach: a hole of 20 cm in a sphere of half a
        // metre is closed as before, with a cap that lies up to 12 mm
        // outside the sphere. Measured radii: 0.5002 to 0.5118 m. A sphere
        // of 12 cm, six voxels, keeps a hole of 8 cm.
        let center = [0.3, 0.2, 0.1];
        let around: Vec<[f64; 3]> = (0..6)
            .map(|side| {
                let mut station = center;
                station[side / 2] += if side % 2 == 0 { 5.0 } else { -5.0 };
                station
            })
            .collect();
        for (radius, count, hole, closes) in
            [(0.5, 100_000, 0.20, true), (0.12, 20_000, 0.08, false)]
        {
            let ball = part(&sphere(center, radius, count), &|_, point| {
                let from_pole = (point[0] - center[0]).hypot(point[1] - center[1]);
                point[2] < center[2] || from_pole > hole / 2.0
            })
            .with_stations(&around);
            let (mesh, report) = run(&ball, &fine());
            assert_sound(&mesh, &report);
            assert_eq!(
                (report.topology.components, report.topology.euler),
                (1, 1 + i64::from(closes)),
                "{radius}"
            );
            assert!(mesh.vertices.iter().all(|vertex| {
                let from_center = length(difference(*vertex, center));
                from_center > radius && from_center < radius + 0.015
            }));
        }

        // A plate that ends short of a wall. At 10 cm from it the plate
        // used to run on 5 cm towards the wall, in tongues up to 11 mm off
        // its plane; it now ends 2 cm past its last points, as at a free
        // rim. At 24 cm the wall is out of reach of the elements that tell
        // where the plate runs, and it still runs on: 10 cm, flat.
        for (gap, runs_on) in [(0.10, 0.03), (0.24, 0.12)] {
            let mut plate = plane_with_hole([1.5, 2.0], 0.01, None);
            for point in &mut plate.points {
                *point = [point[0] + gap, point[1], 1.0];
            }
            let plate = plate.with_stations(&[[1.0, 1.0, 2.5]]);
            let mut wall = plane_with_hole([2.0, 2.0], 0.01, None);
            for (point, normal) in wall.points.iter_mut().zip(&mut wall.normals) {
                *point = [0.0, point[1], point[0]];
                *normal = [1.0, 0.0, 0.0];
            }
            let wall = wall.with_stations(&[[2.0, 1.0, 1.5]]);
            let both = plate.merged(wall).with_noise(Noise::Gaussian(0.001), 5);
            let (mesh, report) = run(&both, &fine());
            assert_sound(&mesh, &report);
            assert_eq!((report.topology.components, report.topology.euler), (2, 2));
            let (mut starts, mut off) = (f64::INFINITY, 0f64);
            for vertex in mesh.vertices.iter().filter(|vertex| vertex[0] > 0.03) {
                starts = starts.min(vertex[0]);
                off = off.max((vertex[2] - 1.0).abs());
            }
            // Measured: 2.0 and 10.0 cm past the last points, and 2.8 and
            // 3.8 mm off the plane of the plate.
            assert!(gap - starts < runs_on, "{gap} {starts}");
            assert!(off < 0.006, "{gap} {off}");
        }
    }

    #[test]
    fn stray_points_leave_no_loose_pieces() {
        // Five hundred points at random in the room, among its 268,000.
        // One of them that lies just inside the reach of a plane fit from
        // a wall finds a plane there and stands for a sheet of up to 61
        // voxel faces. With the dust limit at 25 faces this room had six
        // loose pieces, 146 open edges and 244 vertices more than 2 cm off
        // its planes.
        use crate::test_shapes::stray_points;
        let inside = Bounds {
            min: [0.0; 3],
            max: [4.0, 3.0, 2.6],
        };
        let strays = stray_points(inside, 500, 11).with_stations(&[RoomSpec::default().station]);
        let scan = room().with_noise(Noise::Gaussian(0.0015), 5).merged(strays);
        let (mesh, report) = run(&scan, &fine());
        assert_closed(&mesh, &report);
        // Measured: 6.3 mm, as without the stray points.
        assert!(off_the_room(&mesh) < 0.009, "{}", off_the_room(&mesh));
        // The stray points count in the largest deviation, not in the
        // rest: measured a mean of 1.8 mm and a largest of 1.24 m.
        assert!(report.deviation.p95 < 0.004 && report.deviation.max > 0.5);
    }

    #[test]
    fn thin_walls_and_sheets_have_their_limits() {
        // The room with a door and a window, with walls of 3.5 and of 2
        // voxels. How far its vertices lie from the planes of its faces
        // and reveals tells bulges.
        let walled = |thickness: f64| {
            let spec = RoomSpec {
                spacing: 0.016,
                wall_thickness: Some(thickness),
                openings: vec![
                    Opening::door(Wall::South, 1.0),
                    Opening::window(Wall::East, 0.8),
                ],
                ..RoomSpec::default()
            };
            let building = box_room(&spec).with_noise(Noise::Gaussian(0.0015), 4);
            let (mesh, report) = run(&building, &fine());
            let t = thickness;
            let worst = mesh
                .vertices
                .iter()
                .map(|vertex| {
                    let [x, y, z] = *vertex;
                    let mut planes = vec![x, x - 4.0, x + t, x - 4.0 - t];
                    planes.extend([y, y - 3.0, y + t, y - 3.0 - t, z, z - 2.6]);
                    if y < 0.02 && y > -t - 0.02 {
                        planes.extend([x - 1.0, x - 1.9, z - 2.1]);
                    }
                    if x > 3.98 && x < 4.0 + t + 0.02 {
                        planes.extend([y - 0.8, y - 2.0, z - 0.9, z - 2.1]);
                    }
                    planes
                        .into_iter()
                        .map(f64::abs)
                        .fold(f64::INFINITY, f64::min)
                })
                .fold(0.0, f64::max);
            (mesh, report, worst)
        };
        // At 3.5 voxels the room is right: one piece with the two openings
        // through it. Measured over three seeds each, at 2.5, 3, 3.5, 4, 5
        // and 10 voxels: always this topology, with the furthest vertex
        // 6.7 to 9.8 mm off from 3.5 voxels on, 12 to 13 mm at 3 voxels and
        // 16 to 17 mm at 2.5.
        let (mesh, report, worst) = walled(0.07);
        assert_sound(&mesh, &report);
        assert_eq!((report.topology.components, report.topology.euler), (1, -2));
        assert!(worst < 0.0125, "{worst}");
        // At 2 voxels the reveals have extra holes and bulges of a voxel
        // and a half: measured an Euler characteristic of -8 and -4 and
        // vertices 15 and 31 mm off. This is the stated limit, pinned so
        // that a change of it is seen.
        let (mesh, report, _) = walled(0.04);
        assert_eq!(report.topology, mesh_topology(&mesh));
        assert_eq!(report.topology.components, 1);
        assert!(report.topology.euler < -2, "{}", report.topology.euler);

        // Two parallel faces, each seen from its own side. Two voxels
        // apart they are sound sheets; one and a half voxels apart they
        // merge into one with holes through it, and an edge can have more
        // than two triangles: measured one such edge in one of two seeds,
        // and an Euler characteristic of -10 and -12. Then the report is
        // what tells.
        let sheets = |apart: f64| {
            let top = plane_with_hole([2.0, 2.0], 0.008, None)
                .with_noise(Noise::Gaussian(0.001), 2)
                .with_stations(&[[1.0, 1.0, 2.0]]);
            let mut bottom = plane_with_hole([2.0, 2.0], 0.008, None)
                .flipped()
                .with_noise(Noise::Gaussian(0.001), 102)
                .with_stations(&[[1.0, 1.0, -2.0]]);
            for point in &mut bottom.points {
                point[2] -= apart;
            }
            run(&top.merged(bottom), &fine())
        };
        let (mesh, report) = sheets(0.04);
        assert_sound(&mesh, &report);
        let (mesh, report) = sheets(0.03);
        assert_eq!(report.topology, mesh_topology(&mesh));
        assert_eq!(report.topology.components, 1);
        assert!(report.topology.euler < 0, "{}", report.topology.euler);
        assert!(report.topology.non_manifold_edges <= 4);
    }

    #[test]
    fn the_region_is_cut_back_to_the_points() {
        // The voxel, the lattice and the middle the surface faces follow
        // from the region. A box drawn wide around the room used to pick a
        // voxel of 5 cm and a middle 50 m up, which turned the ceiling
        // over and left 600 open edges; a slab of 60 km was refused.
        let room = room()
            .with_noise(Noise::Gaussian(0.001), 5)
            .with_stations(&[]);
        let cloud = layer(&room);
        let sources = [source(&cloud, &room)];
        let config = ClosedMeshConfig {
            simplify_tolerance: Some(0.0),
            ..ClosedMeshConfig::default()
        };
        let boxed = |min: [f64; 3], max: [f64; 3]| {
            mesh_closed(
                &sources,
                Some(Bounds { min, max }),
                &everything,
                &config,
                &mut |_| Ok(()),
            )
        };
        let (tight, tight_report) = boxed([-0.5; 3], [4.5, 3.5, 3.1]).unwrap();
        assert_closed(&tight, &tight_report);
        assert_eq!(tight_report.voxel, 0.02);
        assert_eq!(tight_report.region, room.bounds());
        for (min, max) in [
            ([-0.5; 3], [4.5, 3.5, 100.0]),
            ([-0.5, -0.5, -30_000.0], [4.5, 3.5, 30_000.0]),
            ([-1e9; 3], [1e9; 3]),
        ] {
            let (mesh, report) = boxed(min, max).unwrap();
            assert!(same(&tight, &mesh), "{max:?}");
            assert_eq!(report.region, tight_report.region);
            assert_eq!(report.voxel, 0.02);
        }
        // The ceiling faces down, into the room.
        assert_eq!(facing(&tight, ROOM_MIDDLE), (1.0, 1.0));
        // A box that the points do not reach holds none.
        let beside = boxed([10.0, 0.0, 0.0], [12.0, 2.0, 2.0]);
        assert!(
            matches!(&beside, Err(LoadError::InvalidData(reason)) if reason.contains("no points"))
        );

        // One stray point far from the room cannot be cut away: the layer
        // has it. The job says what is the matter and what helps.
        let moved = room.clone().transformed(0.0, [207_000.0, 474_000.0, 10.0]);
        let mut points = moved.cloud_points();
        points.push(Point {
            xyz: [0.0; 3],
            ..points[0]
        });
        let cloud = indexed_cloud(&points, 4_096);
        let stray = [SurfelSource::without_stations(RegionSource::new(
            &cloud.cloud,
            Some(&cloud.index),
            SourceTransform::default(),
        ))];
        match mesh_closed(&stray, None, &everything, &config, &mut |_| Ok(())) {
            Err(LoadError::InvalidData(reason)) => {
                assert!(reason.contains("longer than") && reason.contains("section box"));
            }
            other => panic!("{:?}", other.map(|(_, report)| report)),
        }
        let around = Bounds {
            min: [206_999.0, 473_999.0, 9.0],
            max: [207_005.0, 474_004.0, 13.0],
        };
        let (mesh, report) =
            mesh_closed(&stray, Some(around), &everything, &config, &mut |_| Ok(())).unwrap();
        assert_closed(&mesh, &report);

        // A point without a position: the same, with its own words.
        let mut records: Vec<IndexedPoint> = room
            .cloud_points()
            .into_iter()
            .enumerate()
            .map(|(ordinal, point)| IndexedPoint {
                point,
                ordinal: ordinal as u64,
            })
            .collect();
        records.push(IndexedPoint {
            point: Point {
                xyz: [f64::INFINITY, 0.0, 0.0],
                ..records[0].point
            },
            ordinal: records.len() as u64,
        });
        let endless = [SurfelSource::without_stations(RegionSource::resident(
            &records,
            SourceTransform::default(),
        ))];
        let refused = mesh_closed(&endless, None, &everything, &config, &mut |_| Ok(()));
        assert!(
            matches!(&refused, Err(LoadError::InvalidData(reason)) if reason.contains("finite position")),
            "{:?}",
            refused.err()
        );
        let region = Some(Bounds {
            min: [-0.5; 3],
            max: [4.5, 3.5, 3.1],
        });
        let (mesh, _) =
            mesh_closed(&endless, region, &everything, &config, &mut |_| Ok(())).unwrap();
        assert!(same(&tight, &mesh));
    }

    #[test]
    fn a_selection_is_meshed_within_the_box_around_it() {
        // A floor of 20 x 20 m with a box of 1 m standing on it, without
        // stations; only the points of the box are selected. The filter
        // leaves points out but does not narrow the job: the region does.
        let cube = box_room(&RoomSpec {
            size: [1.0, 1.0, 1.0],
            spacing: 0.012,
            ..RoomSpec::default()
        })
        .flipped();
        let selected_points = cube.points.len() as u64;
        let mut all = cube;
        for point in &mut all.points {
            point[0] += 9.5;
            point[1] += 9.5;
        }
        let floor = plane_with_hole([20.0, 20.0], 0.032, None);
        let floor = part(&floor, &|_, [x, y, _]| {
            !(x > 9.5 && x < 10.5 && y > 9.5 && y < 10.5)
        });
        let all = all.merged(floor).with_stations(&[]);
        let records: Vec<IndexedPoint> = all
            .cloud_points()
            .into_iter()
            .enumerate()
            .map(|(ordinal, point)| IndexedPoint {
                point,
                ordinal: ordinal as u64,
            })
            .collect();
        let resident = [SurfelSource::without_stations(RegionSource::resident(
            &records,
            SourceTransform::default(),
        ))];
        let cloud = layer(&all);
        let indexed = [SurfelSource::without_stations(RegionSource::new(
            &cloud.cloud,
            Some(&cloud.index),
            SourceTransform::default(),
        ))];
        let selected = |_: usize, ordinal: u64, _: &Point| ordinal < selected_points;
        let around = Bounds {
            min: [9.5, 9.5, 0.0],
            max: [10.5, 10.5, 1.0],
        };
        // With the box around the selection as the region: one tile, and a
        // closed box whose faces look at its middle, which is the middle
        // of the region.
        for sources in [&resident, &indexed] {
            let (mesh, report) =
                mesh_closed(sources, Some(around), &selected, &fine(), &mut |_| Ok(())).unwrap();
            assert_closed(&mesh, &report);
            assert_eq!((report.tiles_planned, report.tiles), (1, 1));
            assert_eq!(report.points, selected_points);
            assert_eq!(facing(&mesh, [10.0, 10.0, 0.5]), (1.0, 1.0));
            assert_eq!(report.surfels_by_default, 0);
        }
        // Read through the index, only the leaves at the box are read:
        // measured 46,457 points for the 41,334 selected, of 431,000.
        let (_, report) =
            mesh_closed(&indexed, Some(around), &selected, &fine(), &mut |_| Ok(())).unwrap();
        assert!(report.points_read < 60_000, "{}", report.points_read);

        // Without a region the job covers the floor as well: the middle
        // it faces is that of the floor, which lies in the bottom of the
        // box, and the result is not closed. A layer in memory is looked
        // at point by point while planning, so there the filter still
        // keeps the tiles to those around the box: measured 4, where the
        // index, whose leaves are not read for it, plans 121.
        let (mesh, report) =
            mesh_closed(&resident, None, &selected, &fine(), &mut |_| Ok(())).unwrap();
        assert!(report.tiles_planned <= 8, "{}", report.tiles_planned);
        assert!(report.topology.open_edges > 100);
        assert!(facing(&mesh, [10.0, 10.0, 0.5]).1 < 0.9);
        // Nearly all its elements say that nothing told their side, which
        // is what a caller can warn by.
        assert!(report.surfels_by_default * 10 > report.surfels * 9);
        let (_, report) = mesh_closed(&indexed, None, &selected, &fine(), &mut |_| Ok(())).unwrap();
        assert!(report.tiles_planned > 100, "{}", report.tiles_planned);
    }

    #[test]
    fn several_layers_and_layers_in_memory_give_the_same_mesh() {
        let room = room();
        let cloud = layer(&room);
        let config = ClosedMeshConfig {
            tile_voxels: 32,
            simplify_tolerance: None,
            ..raw()
        };
        let (whole, whole_report) = mesh_of(&cloud, &room, &config);

        // The same room as two scan files: one half each.
        let half = |keep: &dyn Fn(f64) -> bool| -> Shape {
            let mut part = Shape {
                stations: room.stations.clone(),
                ..Shape::default()
            };
            for index in 0..room.points.len() {
                if keep(room.points[index][0]) {
                    part.points.push(room.points[index]);
                    part.normals.push(room.normals[index]);
                    part.station_of.push(room.station_of[index]);
                }
            }
            part
        };
        let (west, east) = (half(&|x| x < 1.7), half(&|x| x >= 1.7));
        let (west_cloud, east_cloud) = (layer(&west), layer(&east));
        let sources = [source(&east_cloud, &east), source(&west_cloud, &west)];
        let (joined, report) =
            mesh_closed(&sources, None, &everything, &config, &mut |_| Ok(())).unwrap();
        assert_closed(&joined, &report);
        assert_eq!(report.points, whole_report.points);
        // The points are the same and so is every sum: only which points
        // were kept for the deviation differs.
        assert!(same(&whole, &joined));

        // A layer without an index, read into memory once.
        let points = resident_points(&cloud.cloud, &mut |_| Ok(())).unwrap();
        let resident = [SurfelSource::with_stations(
            RegionSource::resident(&points, SourceTransform::default()),
            room.stations.clone(),
            ranges(&room),
        )];
        let (in_memory, report) =
            mesh_closed(&resident, None, &everything, &config, &mut |_| Ok(())).unwrap();
        assert!(same(&whole, &in_memory));
        assert_eq!(report.deviation, whole_report.deviation);

        // Streaming the file for every tile is refused.
        let streamed = [SurfelSource::without_stations(RegionSource::new(
            &cloud.cloud,
            None,
            SourceTransform::default(),
        ))];
        let refused = mesh_closed(&streamed, None, &everything, &config, &mut |_| Ok(()));
        assert!(
            matches!(&refused, Err(LoadError::InvalidData(reason)) if reason.contains("index")),
            "{:?}",
            refused.err()
        );

        // A layer that is moved and mirrored in the scene is meshed where
        // it stands.
        let transform = SourceTransform {
            scale: [-1.0, 1.0, 1.0],
            offset: [100.0, 20.0, 0.5],
        };
        let placed = [SurfelSource::with_stations(
            RegionSource::new(&cloud.cloud, Some(&cloud.index), transform),
            room.stations
                .iter()
                .map(|station| transform.xyz(*station))
                .collect(),
            ranges(&room),
        )];
        let (mesh, report) =
            mesh_closed(&placed, None, &everything, &config, &mut |_| Ok(())).unwrap();
        assert_closed(&mesh, &report);
        let found = bounds(&mesh);
        assert!((found.min[0] - 96.0).abs() < 0.02 && (found.max[0] - 100.0).abs() < 0.02);
        assert!((found.min[2] - 0.5).abs() < 0.02);
        assert_eq!(facing(&mesh, transform.xyz(ROOM_MIDDLE)), (1.0, 1.0));
    }

    #[test]
    fn runs_and_thread_counts_give_the_same_mesh() {
        let room = room().with_noise(Noise::Gaussian(0.001), 3);
        let cloud = layer(&room);
        let config = |threads: usize| ClosedMeshConfig {
            tile_voxels: 32,
            simplify_tolerance: None,
            threads,
            ..raw()
        };
        let (first, first_report) = mesh_of(&cloud, &room, &config(0));
        assert_closed(&first, &first_report);
        for threads in [0, 1, 3] {
            let (again, report) = mesh_of(&cloud, &room, &config(threads));
            assert!(same(&first, &again), "{threads} threads");
            assert_eq!(report.deviation, first_report.deviation);
            assert_eq!(report.topology, first_report.topology);
            assert_eq!(report.surfels, first_report.surfels);
            if threads > 0 {
                assert_eq!(report.threads, threads);
            }
            // As many tiles in work as there are threads: these tiles are
            // far too small for the memory allowance to hold any back.
            assert!((1..=report.threads).contains(&report.tiles_side_by_side));
            if threads == 1 {
                assert_eq!(report.tiles_side_by_side, 1);
            }
        }
    }

    #[test]
    fn progress_runs_through_the_stages_and_can_cancel() {
        let room = room();
        let cloud = layer(&room);
        let sources = [source(&cloud, &room)];
        let config = ClosedMeshConfig {
            tile_voxels: 32,
            simplify_tolerance: None,
            ..raw()
        };
        let mut seen: Vec<ClosedMeshProgress> = Vec::new();
        mesh_closed(&sources, None, &everything, &config, &mut |progress| {
            seen.push(progress);
            Ok(())
        })
        .unwrap();
        let order = |stage: ClosedMeshStage| match stage {
            ClosedMeshStage::Planning => 0,
            ClosedMeshStage::Reconstructing => 1,
            ClosedMeshStage::Simplifying => 2,
            ClosedMeshStage::Measuring => 3,
        };
        assert_eq!(seen[0].stage, ClosedMeshStage::Planning);
        assert!(seen
            .windows(2)
            .all(|pair| order(pair[0].stage) <= order(pair[1].stage)));
        for stage in [
            ClosedMeshStage::Reconstructing,
            ClosedMeshStage::Simplifying,
            ClosedMeshStage::Measuring,
        ] {
            let steps: Vec<_> = seen.iter().filter(|step| step.stage == stage).collect();
            assert!(!steps.is_empty(), "{stage:?}");
            assert!(steps.iter().all(|step| step.completed <= step.total));
            // The share that is done never goes back.
            assert!(steps.windows(2).all(|pair| {
                pair[0].completed * pair[1].total <= pair[1].completed * pair[0].total
            }));
        }
        let tiles: Vec<_> = seen
            .iter()
            .filter(|step| step.stage == ClosedMeshStage::Reconstructing)
            .collect();
        assert_eq!(tiles.last().unwrap().completed, tiles[0].total);
        assert!(tiles[0].total >= 30);
        // Putting the tiles together asks between the pieces, with all
        // tiles done.
        let joining = tiles
            .iter()
            .filter(|step| step.completed == step.total)
            .count();
        assert!(joining > 30, "{joining}");
        // Measuring counts the edges and then the sampled points, a chunk
        // at a time: half of the total each.
        let measuring: Vec<_> = seen
            .iter()
            .filter(|step| step.stage == ClosedMeshStage::Measuring)
            .collect();
        assert!(measuring.len() >= 12, "{}", measuring.len());
        assert_eq!((measuring[0].completed, measuring[0].total), (0, 2_000));
        assert!(measuring.iter().any(|step| step.completed == 1_000));
        assert_eq!(seen.last().unwrap().completed, 2_000);

        // Cancelled at any moment, the job ends with that and asks nothing
        // more.
        let cancel_when = |when: &dyn Fn(usize, ClosedMeshProgress) -> bool| {
            let (mut calls, mut cancelled, mut after) = (0, false, 0);
            let result = mesh_closed(&sources, None, &everything, &config, &mut |progress| {
                if cancelled {
                    after += 1;
                }
                calls += 1;
                if when(calls, progress) {
                    cancelled = true;
                    return Err(LoadError::Cancelled);
                }
                Ok(())
            });
            assert!(matches!(result, Err(LoadError::Cancelled)));
            assert_eq!(after, 0);
        };
        cancel_when(&|calls, _| calls == 1);
        cancel_when(&|calls, _| calls == 2);
        cancel_when(&|_, progress| {
            progress.stage == ClosedMeshStage::Reconstructing && progress.completed >= 5
        });
        cancel_when(&|_, progress| progress.stage == ClosedMeshStage::Simplifying);
        cancel_when(&|_, progress| progress.stage == ClosedMeshStage::Measuring);
        // While the tiles are put together, and half way the measuring.
        cancel_when(&|_, progress| {
            progress.stage == ClosedMeshStage::Reconstructing
                && progress.completed == progress.total
                && progress.total > 0
        });
        let joining = std::cell::Cell::new(0);
        cancel_when(&|_, progress| {
            if progress.stage == ClosedMeshStage::Reconstructing
                && progress.completed == progress.total
            {
                joining.set(joining.get() + 1);
            }
            joining.get() == 10
        });
        assert_eq!(joining.get(), 10);
        cancel_when(&|_, progress| {
            progress.stage == ClosedMeshStage::Measuring && progress.completed > 1_500
        });
        // Any other error from the callback is passed on as it is.
        let result = mesh_closed(&sources, None, &everything, &config, &mut |progress| {
            if progress.stage == ClosedMeshStage::Reconstructing && progress.completed > 2 {
                Err(LoadError::InvalidData("stop here".into()))
            } else {
                Ok(())
            }
        });
        assert!(matches!(result, Err(LoadError::InvalidData(reason)) if reason == "stop here"));
    }

    #[test]
    fn limits_and_empty_regions_are_reported() {
        let room = room();
        let cloud = layer(&room);
        let sources = [source(&cloud, &room)];
        let run = |region: Option<Bounds>, config: &ClosedMeshConfig| {
            mesh_closed(&sources, region, &everything, config, &mut |_| Ok(()))
        };
        // More triangles than allowed: the message names the ways out.
        let capped = run(
            None,
            &ClosedMeshConfig {
                max_triangles: 1_000,
                ..raw()
            },
        );
        match capped {
            Err(LoadError::InvalidData(reason)) => {
                assert!(reason.contains("1000 triangles"), "{reason}");
                assert!(reason.contains("voxel") && reason.contains("section box"));
            }
            other => panic!("{:?}", other.map(|(_, report)| report)),
        }
        let capped = run(
            None,
            &ClosedMeshConfig {
                max_vertices: 500,
                ..raw()
            },
        );
        assert!(matches!(capped, Err(LoadError::InvalidData(_))));
        // Simplified, the room fits in a limit the extracted surface
        // exceeds twenty times.
        let fits = run(
            None,
            &ClosedMeshConfig {
                max_triangles: 4_000,
                simplify_tolerance: None,
                color_step: 255,
                tile_voxels: 256,
                ..raw()
            },
        );
        assert!(fits.is_ok());

        // A box beside the room, a box in the middle of the room where no
        // surface is, and a filter that leaves nothing.
        let beside = Bounds {
            min: [10.0, 0.0, 0.0],
            max: [12.0, 2.0, 2.0],
        };
        let inside = Bounds {
            min: [1.0, 1.0, 1.0],
            max: [2.0, 2.0, 2.0],
        };
        for region in [beside, inside] {
            let empty = run(Some(region), &raw());
            assert!(
                matches!(&empty, Err(LoadError::InvalidData(reason)) if reason.contains("no points")),
                "{:?}",
                empty.err()
            );
        }
        let nothing = mesh_closed(&sources, None, &|_, _, _| false, &raw(), &mut |_| Ok(()));
        assert!(
            matches!(nothing, Err(LoadError::InvalidData(reason)) if reason.contains("no points"))
        );
        let no_layers = mesh_closed(&[], None, &everything, &raw(), &mut |_| Ok(()));
        assert!(matches!(no_layers, Err(LoadError::InvalidData(_))));
        // A handful of points is no surface.
        let few = mesh_closed(
            &sources,
            None,
            &|_, ordinal, _| ordinal % 20_000 == 0,
            &raw(),
            &mut |_| Ok(()),
        );
        assert!(matches!(few, Err(LoadError::InvalidData(reason)) if reason.contains("too few")));
        // A region that is no box.
        let inverted = Bounds {
            min: [1.0, 0.0, 0.0],
            max: [0.0, 1.0, 1.0],
        };
        assert!(matches!(
            run(Some(inverted), &raw()),
            Err(LoadError::InvalidData(_))
        ));
    }

    #[test]
    fn settings_are_checked() {
        let default = ClosedMeshConfig::default();
        assert!(default.validate().is_ok());
        assert_eq!(default.max_hole, 0.25);
        assert_eq!(
            (default.max_vertices, default.max_triangles),
            (
                crate::obj_mesh::MAX_VERTICES,
                crate::obj_mesh::MAX_TRIANGLES
            )
        );
        let bad = [
            ClosedMeshConfig {
                voxel: Some(0.004),
                ..default
            },
            ClosedMeshConfig {
                voxel: Some(0.6),
                ..default
            },
            ClosedMeshConfig {
                voxel: Some(f64::NAN),
                ..default
            },
            ClosedMeshConfig {
                max_hole: -0.1,
                ..default
            },
            ClosedMeshConfig {
                max_hole: 4.0,
                ..default
            },
            ClosedMeshConfig {
                max_hole: f64::NAN,
                ..default
            },
            ClosedMeshConfig {
                simplify_tolerance: Some(-0.001),
                ..default
            },
            ClosedMeshConfig {
                simplify_tolerance: Some(f64::INFINITY),
                ..default
            },
            ClosedMeshConfig {
                orientation: MeshOrientation::Towards([0.0, f64::NAN, 0.0]),
                ..default
            },
            ClosedMeshConfig {
                max_triangles: 0,
                ..default
            },
            ClosedMeshConfig {
                max_vertices: 2,
                ..default
            },
            ClosedMeshConfig {
                deviation_samples: 5_000_001,
                ..default
            },
            ClosedMeshConfig {
                tile_voxels: 8,
                ..default
            },
            ClosedMeshConfig {
                tile_voxels: 300,
                ..default
            },
        ];
        for config in bad {
            assert!(config.validate().is_err(), "{config:?}");
        }
        let good = [
            ClosedMeshConfig {
                voxel: Some(0.005),
                max_hole: 0.0,
                ..default
            },
            ClosedMeshConfig {
                voxel: Some(0.5),
                max_hole: 3.2,
                ..default
            },
            ClosedMeshConfig {
                simplify_tolerance: Some(0.0),
                tile_voxels: 16,
                ..default
            },
            ClosedMeshConfig {
                deviation_samples: 0,
                tile_voxels: 256,
                ..default
            },
        ];
        for config in good {
            assert!(config.validate().is_ok(), "{config:?}");
        }
        // The voxel follows the size of the region when left open.
        let cube = |side: f64| Bounds {
            min: [0.0; 3],
            max: [side, side * 0.5, 3.0],
        };
        assert_eq!(default.voxel_for(cube(20.0)), 0.02);
        assert_eq!(default.voxel_for(cube(20.5)), 0.03);
        assert_eq!(default.voxel_for(cube(60.0)), 0.03);
        assert_eq!(default.voxel_for(cube(300.0)), 0.05);
        assert_eq!(
            ClosedMeshConfig {
                voxel: Some(0.1),
                ..default
            }
            .voxel_for(cube(5.0)),
            0.1
        );
        // Without points kept for it, no deviation is measured.
        let plane = plane_with_hole([1.0, 1.0], 0.01, None);
        let (_, report) = run(
            &plane,
            &ClosedMeshConfig {
                deviation_samples: 0,
                ..default
            },
        );
        assert_eq!(report.voxel, 0.02);
        assert_eq!(report.deviation, MeshDeviation::default());
        let (_, report) = run(
            &plane,
            &ClosedMeshConfig {
                deviation_samples: 500,
                ..default
            },
        );
        assert_eq!(report.deviation.samples, 500);
    }

    #[test]
    fn distances_on_a_grid_match_a_search_through_all_cells() {
        let side = 13usize;
        let mut rng = Rng::new(11);
        for round in 0..40 {
            let mut grid = vec![FAR; side.pow(3)];
            let seeds = round % 7;
            for _ in 0..seeds {
                grid[(rng.next_u64() % side.pow(3) as u64) as usize] = 0;
            }
            // A box inside the grid; cells outside it are left alone.
            let low: [usize; 3] = std::array::from_fn(|_| (rng.next_u64() % 4) as usize);
            let high: [usize; 3] =
                std::array::from_fn(|_| side - 1 - (rng.next_u64() % 4) as usize);
            let before = grid.clone();
            squared_distances(&mut grid, side, low, high);
            let inside = |cell: [usize; 3]| {
                (0..3).all(|axis| cell[axis] >= low[axis] && cell[axis] <= high[axis])
            };
            for z in 0..side {
                for y in 0..side {
                    for x in 0..side {
                        let slot = (z * side + y) * side + x;
                        if !inside([x, y, z]) {
                            assert_eq!(grid[slot], before[slot]);
                            continue;
                        }
                        let mut nearest = FAR;
                        for (other, value) in before.iter().enumerate() {
                            let cell = [other % side, other / side % side, other / side / side];
                            if *value == 0 && inside(cell) {
                                let apart = cell[0].abs_diff(x).pow(2)
                                    + cell[1].abs_diff(y).pow(2)
                                    + cell[2].abs_diff(z).pow(2);
                                nearest = nearest.min(apart as i32);
                            }
                        }
                        assert_eq!(grid[slot], nearest, "round {round} at {x} {y} {z}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_tile_waits_for_room_and_stops_waiting_when_cancelled() {
        let lattice = Lattice::new(
            Bounds {
                min: [0.0; 3],
                max: [1.0; 3],
            },
            0.02,
            0.25,
            DEFAULT_TILE_VOXELS,
        )
        .unwrap();
        let job = Job {
            lattice: &lattice,
            sources: &[],
            accept: &everything,
            orientation: SurfelOrientation {
                stations: true,
                fallback: SurfelFallback::Upward,
            },
            tolerance: 0.0,
            color_step: 255,
            samples: 0,
            stop: AtomicBool::new(false),
            sample_limit: AtomicU64::new(u64::MAX),
            in_work: Mutex::new(InWork::default()),
            freed: Condvar::new(),
        };
        // Without points a tile needs its grids only: 113 corners along
        // the field and 125 along the distances, 51 MB.
        let grids = job.tile_memory([0, 0, 0]);
        assert_eq!(grids, 113u64.pow(3) * 30 + 125u64.pow(3) * 4);
        // A tile that needs more than there is runs when it is alone.
        let all = job.reserve(TILE_MEMORY + 1).unwrap();
        let waited = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let room = job.reserve(grids).unwrap();
                waited.store(true, Ordering::SeqCst);
                drop(room);
            });
            std::thread::sleep(Duration::from_millis(250));
            assert!(!waited.load(Ordering::SeqCst));
            drop(all);
        });
        assert!(waited.load(Ordering::SeqCst));
        assert_eq!(job.in_work.lock().unwrap().bytes, 0);
        // Several fit side by side, and a cancel ends the wait of one that
        // does not.
        let first = job.reserve(TILE_MEMORY / 2).unwrap();
        let second = job.reserve(TILE_MEMORY / 2).unwrap();
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| job.reserve(1).map(drop));
            std::thread::sleep(Duration::from_millis(50));
            job.stop.store(true, Ordering::Relaxed);
            assert!(matches!(waiting.join().unwrap(), Err(LoadError::Cancelled)));
        });
        // A tile that turns out to need less gives the rest back, and a
        // waiting tile starts.
        job.stop.store(false, Ordering::Relaxed);
        let mut first = first;
        let started = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let room = job.reserve(TILE_MEMORY / 4).unwrap();
                started.store(true, Ordering::SeqCst);
                drop(room);
            });
            std::thread::sleep(Duration::from_millis(250));
            assert!(!started.load(Ordering::SeqCst));
            first.shrink_to(TILE_MEMORY / 4);
        });
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(job.in_work.lock().unwrap().bytes, TILE_MEMORY * 3 / 4);
        // Asking for more than it holds changes nothing.
        first.shrink_to(TILE_MEMORY);
        assert_eq!(job.in_work.lock().unwrap().bytes, TILE_MEMORY * 3 / 4);
        drop((first, second));
        let in_work = job.in_work.lock().unwrap();
        // The most at any moment: the two halves and the one that waited.
        assert_eq!((in_work.bytes, in_work.tiles, in_work.most), (0, 0, 3));
    }
}
