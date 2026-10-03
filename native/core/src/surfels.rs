//! Oriented surface elements on a world lattice.
//!
//! A scan holds far more points than a surface needs. Here the points of one
//! tile of a lattice are reduced to one element per small cell: where the
//! points of that cell lie on average, which way their surface faces and
//! what colour it has. The points themselves are not kept, so the memory of
//! a tile follows the surface inside it and not the number of points.
//!
//! The direction of an element is the normal of a plane through its
//! neighbours. Its side comes from the station that measured the points,
//! found through the scan ranges of the source; points without a known
//! station take the side of their neighbours that have one, or else that
//! of a given point or the upward side. Where none of these can tell, a
//! plane takes a default side that follows from its normal alone, so that
//! a face is never split by its own noise.
//!
//! Elements come in levels. Level 0 has cells of half a voxel; every next
//! level doubles the cell and merges the elements below it. The coarse
//! levels describe the surface where the points are too sparse for a fine
//! plane fit, and across gaps in the data.
//!
//! Everything a tile computes for a cell depends only on the points near
//! that cell and is done in the same order whatever tile asks, so two tiles
//! that overlap hold the same elements, bit for bit, where both have read
//! far enough around them. Meshing tile by tile relies on that.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::local_fit::{dot, unit, Moments};
use crate::region_source::{
    contains, overlaps, visit_region, RegionFilter, RegionReader, RegionSource, SourceTransform,
};
use crate::{Bounds, IndexedPoint, LoadError, OctreeIndex, PointCloud, ScanRange};

/// The smallest and largest voxel a lattice takes, in metres.
pub const MIN_VOXEL: f64 = 0.005;
pub const MAX_VOXEL: f64 = 0.5;
/// Voxels along a tile unless the caller asks otherwise.
pub const DEFAULT_TILE_VOXELS: u32 = 96;
/// The widest gap that can be closed, in voxels. A wider one would need a
/// margin around every tile that costs more than the tile itself.
pub const MAX_HOLE_VOXELS: f64 = 32.0;
/// How far the weight of an element reaches, in cells of its level: 2.5
/// voxels at level 0, and twice as far at every next level.
const KERNEL_CELLS: u32 = 5;
/// Where gaps are closed, the coarsest level reaches this many voxels
/// further than half the widest gap, so that it still tells where the
/// surface runs just beyond what may be closed.
pub const GAP_REACH: f64 = 4.5;
/// Neighbours of an element within this many cells give its plane.
const FIT_CELLS: f64 = 4.0;
/// An element beside an edge takes its side from the neighbours within
/// this many cells whose own fit was flat.
const SIDE_CELLS: f32 = 6.0;
/// The mean cosine between its normal and theirs below which the neighbours
/// turn an element around: less than a face at 70 degrees gives.
const SIDE_AGREEMENT: f32 = 0.3;
/// Stations less far in front of an element's own plane than this, as the
/// mean cosine between its normal and the directions to them, look along
/// the face: under about nine degrees the tilt that noise gives a fit
/// decides on which side of the plane they lie, element by element.
const WEAK_VIEW: f64 = 0.15;
/// An element whose side had that little to go by takes the side of the
/// larger part of all its fitted neighbours within this many cells. Tilts
/// of fits next to each other go together, so the neighbours must reach
/// well past one fit.
const WEAK_SIDE_CELLS: f32 = 8.0;
/// How far inside the block a tile read an element must lie, in voxels,
/// for nothing to be missing from what it takes from its neighbours: the
/// reach of a fit at level 1, and at level 0 that of a fit beyond the
/// furthest neighbours a side comes from.
const FIT_MARGIN: u32 = 6;
const _: () = assert!(WEAK_SIDE_CELLS + FIT_CELLS as f32 <= 2.0 * FIT_MARGIN as f32);
/// A fit whose smallest eigenvalue is a larger share of the three than this
/// is bent: its neighbours lie on more than one face, or their noise is
/// large for the cell.
const FIT_BENT: f64 = 0.005;
/// Stations this far in front of a plane, as the mean cosine between the
/// normal and the directions from the points to their stations, saw the
/// face itself and not a face next to it.
const SURE_VIEW: f64 = 0.8;
/// Without a station, the point a fallback names tells the side of a plane
/// only when it lies clearly off that plane. The tilt that noise gives a
/// fit moves the plane, where it passes the point, by that tilt times the
/// distance to the element: about 0.03 of it with noise of 0.15 voxel, and
/// the same for all the elements of a patch a few voxels wide. So the
/// point must lie off the plane by this share of the distance from the
/// point to the farthest corner of the region, with that distance counted
/// up to `SURE_REACH` metres and the result no less than `SURE_DISTANCE`
/// voxels. The measure is the same for every element, so that a face is
/// judged as a whole; it is capped so that in a large region the floors and
/// ceilings of a storey still count as seen from its middle.
const SURE_SIDE: f64 = 0.08;
const SURE_REACH: f64 = 10.0;
const SURE_DISTANCE: f64 = 2.0;
/// A point seen from an element at less than this cosine off its plane
/// tells its side with little to go by, and so does the direction of
/// `UPRIGHT_SIDE` at less than this cosine with the normal.
const WEAK_SIDE: f64 = 0.2;
/// A plane whose normal rises or falls less than this is upright: up and
/// down do not tell its sides.
const UPRIGHT: f64 = 0.2;
/// The side an upright plane faces when nothing tells: a direction of unit
/// length that is square to no wall of a building laid out along the axes
/// or at 45 degrees to them.
const UPRIGHT_SIDE: [f64; 2] = [0.883, 0.469];
/// A bent fit is repeated with the neighbours within this many cells of the
/// plane found: once with a wide margin, to find the face, and then with
/// a narrow one, which leaves out most of what the other face has near
/// the edge.
const FIT_SLABS: [f32; 3] = [1.0, 0.6, 0.6];
/// The share of the neighbours a repeated fit must keep to be taken, and
/// how wide they must lie for their length: half a disc of a face beside
/// an edge is 0.28 wide, a strip along the edge that cuts through both
/// faces 0.05.
const FIT_SHARE: f64 = 0.4;
const FIT_FACE_WIDTH: f64 = 0.15;
/// A plane through fewer elements is not trusted: stray points and the
/// mixed returns at edges have few neighbours.
const MIN_FIT_ELEMENTS: u64 = 6;
/// Elements along a line have no plane: the second eigenvalue must be at
/// least this share of the third.
const MIN_FIT_WIDTH: f64 = 0.01;
/// Levels above this one only merge the elements below them. A plane fit of
/// their own would need neighbours from further away than a tile reads.
const MAX_FITTED_LEVEL: usize = 1;
/// Cell coordinates in a key, per axis.
const KEY_BITS: u32 = 20;
/// Steps a position within a level-0 cell is counted in. Sums of whole
/// numbers do not depend on the order of the points.
const CELL_STEPS: f64 = 65_536.0;
/// Scale of the unit vector towards a station, for the same reason.
const VIEW_STEPS: f64 = 4_096.0;
/// Leaves that reach over more tiles than this are read once while
/// planning, so that only the tiles near their points are meshed.
const MAX_LEAF_TILES: u64 = 27;
/// Tiles a job may plan. More means a voxel far too small for the region.
const MAX_TILES: usize = 1 << 20;

/// The lattice of one job: voxels of one size at whole multiples of that
/// size in world coordinates, grouped in cubic tiles. A different region
/// gives other tiles but the same voxels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lattice {
    /// Edge of a voxel in metres.
    pub voxel: f64,
    /// The box points are taken from.
    pub region: Bounds,
    /// Voxels along a tile.
    pub tile: u32,
    /// Voxels read around a tile, so that what the tile computes for its
    /// own cells does not depend on where the tile ends.
    pub halo: u32,
    /// The coarsest level of elements; level 0 is the finest.
    pub levels: usize,
    /// Half the widest gap that is closed, in voxels; zero closes none.
    pub close_radius: f64,
    /// How far an element of the coarsest level reaches, in voxels.
    top_reach: f32,
    /// Tiles along each axis.
    pub tiles: [u32; 3],
    /// World index of the voxel with local index zero.
    anchor: [i64; 3],
}

/// The voxels one tile reads: its own and the halo around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileBlock {
    /// Local index of the first voxel read.
    pub min: [i64; 3],
    /// Voxels read along each axis.
    pub size: u32,
    /// Voxels of the halo: the tile's own voxels start this far in.
    pub halo: u32,
    /// The tile's own voxels along each axis.
    pub tile: u32,
}

impl Lattice {
    /// A lattice over a region. `max_hole` is the widest gap to close, in
    /// metres; what exceeds `MAX_HOLE_VOXELS` voxels is cut back to that.
    pub fn new(
        region: Bounds,
        voxel: f64,
        max_hole: f64,
        tile_voxels: u32,
    ) -> Result<Self, LoadError> {
        let finite = region
            .min
            .iter()
            .chain(&region.max)
            .all(|value| value.is_finite());
        if !finite || (0..3).any(|axis| region.min[axis] > region.max[axis]) {
            return Err(LoadError::InvalidData(
                "the region is not a box with its minimum below its maximum".into(),
            ));
        }
        if !(MIN_VOXEL..=MAX_VOXEL).contains(&voxel) {
            return Err(LoadError::InvalidData(format!(
                "the voxel size must lie between {MIN_VOXEL} and {MAX_VOXEL} m"
            )));
        }
        if !max_hole.is_finite() || max_hole < 0.0 {
            return Err(LoadError::InvalidData(
                "the hole limit must be zero or more".into(),
            ));
        }
        let close_radius = (max_hole / voxel).min(MAX_HOLE_VOXELS) * 0.5;
        let reach = close_radius.ceil() as u32;
        // The coarsest level must tell where the surface runs across the
        // widest gap. Its reach may be stretched to 3.5 instead of 2.5 of
        // its steps before another level is taken, as every level doubles
        // the margin a tile reads.
        let needed = if reach > 0 {
            close_radius + GAP_REACH
        } else {
            0.0
        };
        let mut levels = 1usize;
        while 3.5 * f64::from(1u32 << levels) < needed {
            levels += 1;
        }
        let step = 1u32 << levels;
        let top_reach = (f64::from(KERNEL_CELLS * step) * 0.5).max(needed);
        // Blocks of four cells of the coarsest level start on this many
        // voxels; tiles do too, so that every tile sees the same blocks.
        let unit = 2 * step;
        let coarsest = step / 2;
        // From the tile outward: one cell for the neighbours of its edges;
        // then what the closing of gaps looks at, or what the coarsest
        // elements reach over, counted from a corner of their own level;
        // then the margin in which a plane fit lacks neighbours.
        let chain = if reach > 0 {
            (2 * reach + 9).max(reach + top_reach.ceil() as u32 + step)
        } else {
            top_reach.ceil() as u32 + step
        };
        let halo = (1 + chain + FIT_MARGIN + coarsest).next_multiple_of(unit);
        let tile = tile_voxels.clamp(unit, 256).next_multiple_of(unit);
        let mut anchor = [0i64; 3];
        let mut tiles = [0u32; 3];
        for axis in 0..3 {
            let first = (region.min[axis] / voxel).floor();
            let last = (region.max[axis] / voxel).floor();
            // A surface may run a few voxels past the outermost points.
            let span = last - first + f64::from(unit) + 4.0;
            if span >= f64::from((1u32 << KEY_BITS) - 2 * tile) {
                // Tiles are only made where points are, but every voxel of
                // the region needs a number of its own.
                return Err(LoadError::InvalidData(format!(
                    "the region to mesh is longer than {:.0} km, the most that voxels of \
                     {voxel} m can cover: use a section box around the part to mesh, or a \
                     larger voxel",
                    (f64::from(1u32 << KEY_BITS) * voxel / 1000.0).floor()
                )));
            }
            anchor[axis] =
                (first as i64).div_euclid(i64::from(unit)) * i64::from(unit) - i64::from(unit);
            tiles[axis] = ((last as i64 - anchor[axis] + 4) / i64::from(tile)) as u32 + 1;
        }
        Ok(Self {
            voxel,
            region,
            tile,
            halo,
            levels,
            close_radius,
            top_reach: top_reach as f32,
            tiles,
            anchor,
        })
    }

    /// World position of the lattice point with local index zero.
    pub fn origin(&self) -> [f64; 3] {
        self.anchor.map(|index| index as f64 * self.voxel)
    }

    /// World position of a place given in voxels from the local origin.
    pub fn world(&self, local: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| (self.anchor[axis] as f64 + local[axis]) * self.voxel)
    }

    /// The voxel that holds a world position, as a local index.
    pub fn voxel_of(&self, xyz: [f64; 3]) -> [i64; 3] {
        std::array::from_fn(|axis| (xyz[axis] / self.voxel).floor() as i64 - self.anchor[axis])
    }

    /// The tile that owns a voxel, when the voxel lies in the lattice.
    pub fn tile_of(&self, voxel: [i64; 3]) -> Option<[u32; 3]> {
        let mut tile = [0u32; 3];
        for axis in 0..3 {
            let index = voxel[axis].div_euclid(i64::from(self.tile));
            if index < 0 || index >= i64::from(self.tiles[axis]) {
                return None;
            }
            tile[axis] = index as u32;
        }
        Some(tile)
    }

    /// The voxels a tile reads.
    pub fn block(&self, tile: [u32; 3]) -> TileBlock {
        TileBlock {
            min: tile.map(|index| i64::from(index) * i64::from(self.tile) - i64::from(self.halo)),
            size: self.tile + 2 * self.halo,
            halo: self.halo,
            tile: self.tile,
        }
    }

    /// The part of the region a tile reads, or nothing when its block lies
    /// outside the region.
    pub fn read_box(&self, tile: [u32; 3]) -> Option<Bounds> {
        let block = self.block(tile);
        let mut bounds = self.region;
        for axis in 0..3 {
            let low = (self.anchor[axis] + block.min[axis]) as f64 * self.voxel;
            let high =
                (self.anchor[axis] + block.min[axis] + i64::from(block.size)) as f64 * self.voxel;
            bounds.min[axis] = bounds.min[axis].max(low);
            bounds.max[axis] = bounds.max[axis].min(high);
            if bounds.min[axis] > bounds.max[axis] {
                return None;
            }
        }
        Some(bounds)
    }

    /// One number for a voxel of the lattice, the same in every tile.
    pub fn key(&self, voxel: [i64; 3]) -> u64 {
        debug_assert!(
            voxel.iter().all(|index| (0..1 << KEY_BITS).contains(index)),
            "{voxel:?} {self:?}"
        );
        (voxel[2] as u64) << (2 * KEY_BITS) | (voxel[1] as u64) << KEY_BITS | voxel[0] as u64
    }

    /// The voxel a key stands for.
    pub fn key_voxel(&self, key: u64) -> [i64; 3] {
        let mask = (1u64 << KEY_BITS) - 1;
        [
            (key & mask) as i64,
            (key >> KEY_BITS & mask) as i64,
            (key >> (2 * KEY_BITS)) as i64,
        ]
    }

    /// How far from a point, in voxels, a tile can still get a surface from
    /// it: the reach of an element plus the widest gap that is closed.
    fn influence(&self) -> i64 {
        self.close_radius.ceil() as i64 + i64::from(KERNEL_CELLS) + 3
    }

    /// How far an element of a level reaches, in voxels.
    pub fn reach(&self, level: usize) -> f32 {
        if level == self.levels {
            self.top_reach
        } else {
            (KERNEL_CELLS << level) as f32 * 0.5
        }
    }
}

/// One layer to take points from, with the stations that measured it.
#[derive(Debug, Clone)]
pub struct SurfelSource<'a> {
    pub points: RegionSource<'a>,
    /// Scanner positions in the scene.
    stations: Vec<[f64; 3]>,
    /// The scans of the source in ordinal order, as `PointCloud::scan_ranges`.
    ranges: Vec<ScanRange>,
    /// Ordinals from this one on belong to no scan.
    total: u64,
}

impl<'a> SurfelSource<'a> {
    /// A layer with the stations its source file states. A cloud that does
    /// not know which scan holds each point (`scan_ranges_known` is false)
    /// gives a layer without stations: `PointCloud::read_scan_ranges` finds
    /// them out first. Without an index the points would be streamed from
    /// the file for every tile, which the tile reader refuses; read such a
    /// layer once with `region_source::resident_points` and use
    /// `with_stations` or `without_stations`.
    pub fn new(
        cloud: &'a PointCloud,
        index: Option<&'a OctreeIndex>,
        transform: SourceTransform,
    ) -> Self {
        Self::of_cloud(RegionSource::new(cloud, index, transform), cloud)
    }

    /// Points that come from somewhere else than the cloud's index, such as
    /// a layer held in memory, with the stations of that cloud.
    pub fn of_cloud(points: RegionSource<'a>, cloud: &PointCloud) -> Self {
        if !cloud.scan_ranges_known() || cloud.scan_poses.is_empty() {
            return Self::without_stations(points);
        }
        Self {
            stations: cloud
                .scan_poses
                .iter()
                .map(|pose| points.transform.xyz(pose.position))
                .collect(),
            ranges: cloud.scan_ranges.clone(),
            total: cloud.total_points,
            points,
        }
    }

    /// A layer of which no station is known.
    pub fn without_stations(points: RegionSource<'a>) -> Self {
        Self {
            points,
            stations: Vec::new(),
            ranges: Vec::new(),
            total: 0,
        }
    }

    /// A layer with stations given in scene coordinates and the scans in
    /// ordinal order. Without ranges a single station measured every point.
    pub fn with_stations(
        points: RegionSource<'a>,
        stations: Vec<[f64; 3]>,
        ranges: Vec<ScanRange>,
    ) -> Self {
        Self {
            points,
            stations,
            ranges,
            total: u64::MAX,
        }
    }

    /// Scanner positions in the scene.
    pub fn stations(&self) -> &[[f64; 3]] {
        &self.stations
    }

    /// Where the scanner stood that measured the point with this source
    /// ordinal, when that is known. The rules are those of
    /// `PointCloud::station_of`.
    pub fn station_of(&self, ordinal: u64) -> Option<[f64; 3]> {
        if ordinal >= self.total {
            return None;
        }
        if self.ranges.is_empty() {
            return (self.stations.len() == 1).then(|| self.stations[0]);
        }
        let after = self
            .ranges
            .partition_point(|range| range.first_ordinal <= ordinal);
        let station = self.ranges[after.checked_sub(1)?].station? as usize;
        self.stations.get(station).copied()
    }
}

/// Which side of its plane an element faces when no station tells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SurfelFallback {
    /// The side of this point in the scene: the middle of a room.
    Towards([f64; 3]),
    /// The upward side, for data measured from above.
    Upward,
}

/// How elements find their side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfelOrientation {
    /// Use the station that measured the points where it is known.
    pub stations: bool,
    pub fallback: SurfelFallback,
}

/// The elements of one level of a tile, in an order that does not depend on
/// the tile. Positions are in cells of the level from the block's corner.
#[derive(Debug, Default)]
pub struct SurfelLevel {
    /// Edge of a cell in voxels: a half at level 0, doubling per level.
    pub size: f32,
    /// How far the weight of an element reaches, in voxels.
    pub reach: f32,
    /// Cells along the block.
    pub cells: u32,
    /// The cell of every element.
    pub cell: Vec<[u16; 3]>,
    /// Mean position of the points of the cell, from the cell's corner, in
    /// cells: each value lies between 0 and 1.
    pub frac: Vec<[f32; 3]>,
    /// Unit normal on the side the surface is seen from; meaningless while
    /// the weight is zero.
    pub normal: Vec<[f32; 3]>,
    /// What the element counts for: its confidence at level 0, the summed
    /// weight of the elements it merges above. Zero for an element without
    /// a usable plane.
    pub weight: Vec<f32>,
    /// The normal comes from a plane fit at this level and not from the
    /// elements below: this is the level at which the points here form a
    /// surface.
    pub fitted: Vec<bool>,
    /// Mean colour; the last value is 255 when the points had a colour.
    pub color: Vec<[u8; 4]>,
    /// Points in the cell.
    pub count: Vec<u32>,
    // Blocks of 4 x 4 x 4 cells: per block the place of its entry plus one,
    // and per entry the occupied cells as bits and the first element.
    blocks: u32,
    entries: Vec<u32>,
    masks: Vec<u64>,
    bases: Vec<u32>,
}

impl SurfelLevel {
    pub fn len(&self) -> usize {
        self.cell.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cell.is_empty()
    }

    /// Position of an element in voxels from the block's corner.
    pub fn position(&self, index: usize) -> [f32; 3] {
        std::array::from_fn(|axis| {
            (f32::from(self.cell[index][axis]) + self.frac[index][axis]) * self.size
        })
    }

    /// From an element to a place, in voxels. The place is given from the
    /// block's corner as whole voxels plus a part of one, which keeps the
    /// result free of where the block lies: two tiles get the same bits.
    pub fn offset(&self, index: usize, whole: [i32; 3], part: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|axis| self.offset_along(index, axis, whole[axis], part[axis]))
    }

    /// `offset` along one axis.
    pub fn offset_along(&self, index: usize, axis: usize, whole: i32, part: f32) -> f32 {
        // Whole and half voxels: exact.
        let cell = whole as f32 - f32::from(self.cell[index][axis]) * self.size;
        (cell + part) - self.frac[index][axis] * self.size
    }

    /// Call `visit` for every element whose cell lies at most `reach` cells
    /// from `center` along each axis, with the element and the cell's offset
    /// from `center`. The order is the same in every tile.
    pub fn for_each_near(
        &self,
        center: [i32; 3],
        reach: i32,
        mut visit: impl FnMut(usize, [i32; 3]),
    ) {
        let last = self.cells as i32 - 1;
        let low = center.map(|value| (value - reach).max(0));
        let high = center.map(|value| (value + reach).min(last));
        if (0..3).any(|axis| low[axis] > high[axis]) {
            return;
        }
        let blocks = self.blocks as usize;
        for bz in low[2] >> 2..=high[2] >> 2 {
            for by in low[1] >> 2..=high[1] >> 2 {
                let row = (bz as usize * blocks + by as usize) * blocks;
                for bx in low[0] >> 2..=high[0] >> 2 {
                    let entry = self.entries[row + bx as usize];
                    if entry == 0 {
                        continue;
                    }
                    // Elements of a block follow the order of its bits.
                    let mut index = self.bases[entry as usize - 1] as usize;
                    let mut bits = self.masks[entry as usize - 1];
                    while bits != 0 {
                        let bit = bits.trailing_zeros() as i32;
                        bits &= bits - 1;
                        let cell = [
                            bx * 4 + (bit & 3),
                            by * 4 + (bit >> 2 & 3),
                            bz * 4 + (bit >> 4),
                        ];
                        if (0..3).all(|axis| cell[axis] >= low[axis] && cell[axis] <= high[axis]) {
                            visit(index, std::array::from_fn(|axis| cell[axis] - center[axis]));
                        }
                        index += 1;
                    }
                }
            }
        }
    }

    /// The elements of cells given in tile-independent order.
    fn new(level: usize, reach: f32, cells: u32, sums: &[Sums]) -> Self {
        let blocks = cells / 4;
        let mut entries = vec![0u32; (blocks as usize).pow(3)];
        let mut masks = Vec::new();
        let mut bases = Vec::new();
        let mut current = usize::MAX;
        for (index, sum) in sums.iter().enumerate() {
            let key = order_key(sum.cell, blocks);
            let block = (key >> 6) as usize;
            if block != current {
                current = block;
                masks.push(0u64);
                bases.push(index as u32);
                entries[block] = masks.len() as u32;
            }
            *masks.last_mut().expect("an entry was just added") |= 1 << (key & 63);
        }
        // Positions were summed in steps of a level-0 cell.
        let steps = CELL_STEPS * (1u64 << level) as f64;
        Self {
            size: 0.5 * (1u32 << level) as f32,
            reach,
            cells,
            cell: sums.iter().map(|sum| sum.cell).collect(),
            frac: sums
                .iter()
                .map(|sum| {
                    sum.position
                        .map(|value| (value as f64 / (f64::from(sum.count) * steps)) as f32)
                })
                .collect(),
            normal: vec![[0.0; 3]; sums.len()],
            weight: vec![0.0; sums.len()],
            fitted: vec![false; sums.len()],
            color: sums
                .iter()
                .map(|sum| {
                    if sum.colored == 0 {
                        return [0; 4];
                    }
                    let mean = |value: u64| {
                        ((value + u64::from(sum.colored) / 2) / u64::from(sum.colored)) as u8
                    };
                    [mean(sum.rgb[0]), mean(sum.rgb[1]), mean(sum.rgb[2]), 255]
                })
                .collect(),
            count: sums.iter().map(|sum| sum.count).collect(),
            blocks,
            entries,
            masks,
            bases,
        }
    }

    /// The plane through the neighbours of an element: its normal, with no
    /// side yet, how much it can be trusted, and, when the neighbours lie
    /// on more than one face, the normal of the plane through all of them.
    /// `around` is room to work in.
    fn fit(&self, index: usize, around: &mut Vec<[f32; 3]>) -> Option<Fit> {
        // The neighbours within reach, as offsets from the element in
        // cells: the same numbers in every tile.
        let own = self.frac[index];
        around.clear();
        self.for_each_near(
            self.cell[index].map(i32::from),
            FIT_CELLS as i32 + 1,
            |other, offset| {
                let frac = self.frac[other];
                let relative = [
                    (offset[0] as f32 + frac[0]) - own[0],
                    (offset[1] as f32 + frac[1]) - own[1],
                    (offset[2] as f32 + frac[2]) - own[2],
                ];
                let far = relative[0] * relative[0]
                    + relative[1] * relative[1]
                    + relative[2] * relative[2];
                if far <= (FIT_CELLS * FIT_CELLS) as f32 {
                    around.push(relative);
                }
            },
        );
        // With a normal and a width, the plane through those within that
        // width of the plane through the element across that normal, and
        // how many they are.
        let plane = |slab: Option<([f32; 3], f32)>| {
            let mut moments = Moments::around([0.0; 3]);
            for relative in around.iter() {
                let beside = slab.is_some_and(|(normal, width)| {
                    (normal[0] * relative[0] + normal[1] * relative[1] + normal[2] * relative[2])
                        .abs()
                        > width
                });
                if !beside {
                    moments.add(relative.map(f64::from));
                }
            }
            let plane = moments
                .plane()
                .filter(|_| moments.count() >= MIN_FIT_ELEMENTS)?;
            let [_, middle, largest] = plane.eigenvalues;
            (largest > 0.0 && middle >= MIN_FIT_WIDTH * largest).then_some((plane, moments.count()))
        };
        let (mut found, all) = plane(None)?;
        let mut share = 1.0;
        // Beside an edge the neighbours lie on two faces and their plane is
        // that of neither. The fit is then repeated with the neighbours
        // close to the plane found, through the element itself, which
        // turns it onto the face the element lies on and rounds the edge
        // less. A plane that leaves most neighbours aside, or keeps only a
        // strip of them, is not a face but a cut through the edge, and is
        // not taken.
        let bent = (found.surface_variation() > FIT_BENT).then_some(found.normal);
        if bent.is_some() {
            for width in FIT_SLABS {
                let normal = found.normal.map(|value| value as f32);
                let Some((closer, count)) = plane(Some((normal, width))) else {
                    break;
                };
                let [_, middle, largest] = closer.eigenvalues;
                if (count as f64) < FIT_SHARE * all as f64 || middle < FIT_FACE_WIDTH * largest {
                    break;
                }
                found = closer;
                share = count as f64 / all as f64;
            }
        }
        // What stays bent after that counts for little, and so does a
        // plane that few of the neighbours lie on.
        let confidence = ((1.0 - 3.0 * found.surface_variation()) * share).clamp(0.05, 1.0);
        Some(Fit {
            normal: found.normal,
            confidence: confidence as f32,
            bent,
        })
    }

    /// Whether the neighbours of an element say that its normal points to
    /// the wrong side; nothing when none of them is in reach. The
    /// neighbours marked in `voters` within `cells` cells count, the nearer
    /// the more: those of the element's own face lie along its normal, and
    /// at an edge those of both faces lie along a normal between the two.
    /// Faces across the normal say nothing, so beside an edge the
    /// neighbours must clearly disagree: their mean cosine with the normal
    /// must lie below minus `margin`.
    fn neighbours_disagree(
        &self,
        index: usize,
        voters: &[bool],
        cells: f32,
        margin: f32,
    ) -> Option<bool> {
        let own = self.frac[index];
        let normal = self.normal[index];
        let reach = cells * cells;
        let (mut agreement, mut weight) = (0f32, 0f32);
        self.for_each_near(
            self.cell[index].map(i32::from),
            cells as i32 + 1,
            |other, offset| {
                if !voters[other] || other == index {
                    return;
                }
                let frac = self.frac[other];
                let relative = [
                    (offset[0] as f32 + frac[0]) - own[0],
                    (offset[1] as f32 + frac[1]) - own[1],
                    (offset[2] as f32 + frac[2]) - own[2],
                ];
                let far = relative[0] * relative[0]
                    + relative[1] * relative[1]
                    + relative[2] * relative[2];
                if far < reach {
                    let theirs = self.normal[other];
                    let share = 1.0 - far / reach;
                    weight += share;
                    agreement += share
                        * (normal[0] * theirs[0] + normal[1] * theirs[1] + normal[2] * theirs[2]);
                }
            },
        );
        (weight > 0.0).then_some(agreement < -margin * weight)
    }
}

/// What told the side of an element's plane.
#[derive(Clone, Copy)]
struct Side {
    /// The normal of the fit points to the other side.
    flip: bool,
    /// The stations that measured its points told.
    by_station: bool,
    /// Nothing told: the side is the one `default_side` gives.
    by_default: bool,
    /// What told had little to go by: noise may have decided.
    weak: bool,
}

/// The side of a plane when nothing tells it: up, and for an upright plane
/// the side of `UPRIGHT_SIDE`. It follows from the normal alone, so a flat
/// face gets one side from end to end and in every tile, where the sign of
/// a number that is zero but for noise would differ from element to
/// element. Whether to turn the normal, and whether the normal lies close
/// to where the answer changes.
fn default_side(normal: [f64; 3]) -> (bool, bool) {
    let rise = normal[2].abs();
    if rise >= UPRIGHT {
        (normal[2] < 0.0, rise < 1.5 * UPRIGHT)
    } else {
        let along = normal[0] * UPRIGHT_SIDE[0] + normal[1] * UPRIGHT_SIDE[1];
        (along < 0.0, along.abs() < WEAK_SIDE || rise > 0.5 * UPRIGHT)
    }
}

/// The plane fitted at an element.
struct Fit {
    normal: [f64; 3],
    confidence: f32,
    /// For a fit beside an edge: the normal of the plane through all the
    /// neighbours, before it was turned onto one face.
    bent: Option<[f64; 3]>,
}

/// What reading one tile came to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TileSurfelStats {
    /// Points read from the sources, the halo included.
    pub read: u64,
    /// Accepted points in the tile's own voxels: every point of the region
    /// is counted by exactly one tile.
    pub points: u64,
    /// Elements in the tile's own voxels with a plane fit of their own: at
    /// level 0 or, where the points are too sparse for that, one level up.
    pub surfels: u64,
    /// Of those, the ones whose points had a station to take a side from.
    pub by_station: u64,
    /// Of those, the ones whose side nothing told: no station measured
    /// their points, none of their neighbours had one either, and the
    /// fallback sees their plane edge on. They face up, or one fixed
    /// direction when upright.
    pub by_default: u64,
    /// Time spent reading the points and adding them up.
    pub read_time: Duration,
}

/// The elements of one tile and its halo.
#[derive(Debug)]
pub struct TileSurfels {
    pub tile: [u32; 3],
    pub block: TileBlock,
    /// Level 0 first.
    pub levels: Vec<SurfelLevel>,
    pub stats: TileSurfelStats,
}

/// The tiles that can hold a surface, in a fixed order. Leaves of an index
/// tell where points are without reading them; a leaf that covers many
/// tiles, and a layer in memory, is looked at point by point, and there
/// `accept`, the filter of `collect_surfels`, leaves points out. The points
/// of the other leaves are not asked, so a filter that keeps a small part
/// of a layer still gets the tiles of all its leaves in the region: such a
/// job must be given the box around that part as its region. `proceed` is
/// asked between leaves and may stop the work with an error.
pub fn plan_tiles(
    lattice: &Lattice,
    sources: &[SurfelSource<'_>],
    accept: &RegionFilter<'_>,
    proceed: &mut dyn FnMut() -> Result<(), LoadError>,
) -> Result<Vec<[u32; 3]>, LoadError> {
    let mut tiles = BTreeSet::new();
    let reach = lattice.influence();
    // The tiles whose own voxels come within reach of a box of voxels.
    let range = |low: [i64; 3], high: [i64; 3]| -> [(i64, i64); 3] {
        std::array::from_fn(|axis| {
            let last = i64::from(lattice.tiles[axis]) - 1;
            (
                (low[axis] - reach)
                    .div_euclid(i64::from(lattice.tile))
                    .clamp(0, last),
                (high[axis] + reach)
                    .div_euclid(i64::from(lattice.tile))
                    .clamp(0, last),
            )
        })
    };
    let mark = |tiles: &mut BTreeSet<[u32; 3]>, range: [(i64, i64); 3]| {
        for z in range[2].0..=range[2].1 {
            for y in range[1].0..=range[1].1 {
                for x in range[0].0..=range[0].1 {
                    // Ordered by z, then y, then x.
                    tiles.insert([z as u32, y as u32, x as u32]);
                }
            }
        }
    };
    for (number, source) in sources.iter().enumerate() {
        let transform = source.points.transform;
        let mut last = None;
        let mut point = |tiles: &mut BTreeSet<[u32; 3]>, record: &IndexedPoint| {
            let mut scene = record.point;
            scene.xyz = transform.xyz(scene.xyz);
            let xyz = scene.xyz;
            if contains(lattice.region, xyz) && accept(number, record.ordinal, &scene) {
                let voxel = lattice.voxel_of(xyz);
                let found = range(voxel, voxel);
                // Neighbouring points mark the same tiles.
                if last != Some(found) {
                    last = Some(found);
                    mark(tiles, found);
                }
            }
        };
        match source.points.reader {
            RegionReader::Index(index) => {
                let leaves = index
                    .intersecting_leaves(|node| overlaps(transform.bounds(node), lattice.region));
                for leaf in leaves {
                    proceed()?;
                    let bounds = transform.bounds(leaf.bounds);
                    let clipped = Bounds {
                        min: std::array::from_fn(|axis| {
                            bounds.min[axis].max(lattice.region.min[axis])
                        }),
                        max: std::array::from_fn(|axis| {
                            bounds.max[axis].min(lattice.region.max[axis])
                        }),
                    };
                    let found = range(lattice.voxel_of(clipped.min), lattice.voxel_of(clipped.max));
                    let count: u64 = found
                        .iter()
                        .map(|(low, high)| (high - low + 1) as u64)
                        .product();
                    if count <= MAX_LEAF_TILES {
                        mark(&mut tiles, found);
                    } else {
                        index.visit_leaf(leaf, |record| {
                            point(&mut tiles, &record);
                            Ok(())
                        })?;
                    }
                    if tiles.len() > MAX_TILES {
                        break;
                    }
                }
            }
            RegionReader::Resident(points) => {
                proceed()?;
                for record in points {
                    point(&mut tiles, record);
                }
            }
            RegionReader::Stream(_) => {
                return Err(LoadError::InvalidData(
                    "a layer without an index must be read into memory before it is meshed".into(),
                ));
            }
        }
        if tiles.len() > MAX_TILES {
            return Err(LoadError::InvalidData(
                "the voxel size is too small for a region of this size".into(),
            ));
        }
    }
    Ok(tiles.into_iter().map(|[z, y, x]| [x, y, z]).collect())
}

/// What the points of one cell add up to. Whole numbers only, so that the
/// order in which the points arrive does not matter.
#[derive(Debug, Clone, Copy, Default)]
struct Sums {
    cell: [u16; 3],
    count: u32,
    colored: u32,
    /// Positions from the cell's corner, in steps of a level-0 cell.
    position: [u64; 3],
    rgb: [u64; 3],
    /// Sum of the unit vectors from the points to their stations, and the
    /// number of points that have one.
    view: [i64; 3],
    seen: u32,
}

/// The place of a cell in the fixed order of a level: block by block, and
/// within a block cell by cell.
fn order_key(cell: [u16; 3], blocks: u32) -> u64 {
    let [x, y, z] = cell.map(u64::from);
    let block = ((z >> 2) * u64::from(blocks) + (y >> 2)) * u64::from(blocks) + (x >> 2);
    block << 6 | (z & 3) << 4 | (y & 3) << 2 | (x & 3)
}

/// From cell to the place of its sums while points come in: chunks of
/// 8 x 8 x 8 cells, made when their first point arrives.
struct CellTable {
    chunks_along: usize,
    chunk_of: Vec<u32>,
    slots: Vec<u32>,
}

impl CellTable {
    fn new(cells: u32) -> Self {
        let chunks_along = cells.div_ceil(8) as usize;
        Self {
            chunks_along,
            chunk_of: vec![0; chunks_along.pow(3)],
            slots: Vec::new(),
        }
    }

    /// The slot of a cell: zero until the caller stores a place plus one.
    fn slot(&mut self, cell: [u32; 3]) -> &mut u32 {
        let [x, y, z] = cell.map(|value| value as usize);
        let chunk = &mut self.chunk_of
            [((z >> 3) * self.chunks_along + (y >> 3)) * self.chunks_along + (x >> 3)];
        if *chunk == 0 {
            self.slots.resize(self.slots.len() + 512, 0);
            *chunk = (self.slots.len() / 512) as u32;
        }
        &mut self.slots[(*chunk as usize - 1) * 512 + ((z & 7) << 6 | (y & 7) << 3 | (x & 7))]
    }
}

/// Read the points of a tile and its halo and reduce them to elements.
///
/// - `accept` is the filter of `region_source::visit_region`: it leaves out
///   deleted points, hidden classes and what lies outside a selection.
/// - `proceed` is asked at least every few thousand points read; an error
///   from it, such as `LoadError::Cancelled`, stops the work.
/// - `own_point` receives every accepted point in the tile's own voxels
///   with the position of its source: each point of the region reaches
///   exactly one tile this way.
///
/// A source that streams its file is refused: see `SurfelSource::new`.
pub fn collect_surfels(
    lattice: &Lattice,
    sources: &[SurfelSource<'_>],
    accept: &RegionFilter<'_>,
    orientation: SurfelOrientation,
    tile: [u32; 3],
    proceed: &dyn Fn() -> Result<(), LoadError>,
    own_point: &mut dyn FnMut(usize, &IndexedPoint),
) -> Result<TileSurfels, LoadError> {
    if sources
        .iter()
        .any(|source| matches!(source.points.reader, RegionReader::Stream(_)))
    {
        return Err(LoadError::InvalidData(
            "a layer without an index must be read into memory before it is meshed".into(),
        ));
    }
    let block = lattice.block(tile);
    let mut found = TileSurfels {
        tile,
        block,
        levels: Vec::new(),
        stats: TileSurfelStats::default(),
    };
    let Some(read_box) = lattice.read_box(tile) else {
        return Ok(found);
    };
    let cells = block.size * 2;
    let origin = lattice.origin();
    let per_cell = 2.0 / lattice.voxel;
    let own = i64::from(block.halo)..i64::from(block.halo + block.tile);
    let mut table = CellTable::new(cells);
    let mut sums: Vec<Sums> = Vec::new();
    let regions: Vec<RegionSource<'_>> = sources.iter().map(|source| source.points).collect();
    let reading = Instant::now();
    let stats = visit_region(
        &regions,
        read_box,
        accept,
        &mut |_| proceed(),
        &mut |source, batch| {
            for record in batch {
                let xyz = record.point.xyz;
                // The cell and the place within it follow from the world
                // position alone, so every tile finds the same.
                let mut cell = [0u32; 3];
                let mut step = [0u64; 3];
                let mut inside = true;
                let mut in_tile = true;
                for axis in 0..3 {
                    let position = (xyz[axis] - origin[axis]) * per_cell;
                    let whole = position.floor();
                    let local = whole as i64 - block.min[axis] * 2;
                    if local < 0 || local >= i64::from(cells) {
                        inside = false;
                        break;
                    }
                    cell[axis] = local as u32;
                    step[axis] = (((position - whole) * CELL_STEPS) as u64).min(65_535);
                    in_tile &= own.contains(&(local >> 1));
                }
                if !inside {
                    // On the far face of the box: it belongs to the next.
                    continue;
                }
                let slot = table.slot(cell);
                if *slot == 0 {
                    sums.push(Sums {
                        cell: cell.map(|value| value as u16),
                        ..Sums::default()
                    });
                    *slot = sums.len() as u32;
                }
                let sum = &mut sums[*slot as usize - 1];
                sum.count += 1;
                for (sum, step) in sum.position.iter_mut().zip(step) {
                    *sum += step;
                }
                if let Some(rgb) = record.point.rgb {
                    sum.colored += 1;
                    for (sum, value) in sum.rgb.iter_mut().zip(rgb) {
                        *sum += u64::from(value);
                    }
                }
                if orientation.stations {
                    let towards = sources[source]
                        .station_of(record.ordinal)
                        .and_then(|station| {
                            unit(std::array::from_fn(|axis| station[axis] - xyz[axis]))
                        });
                    if let Some(towards) = towards {
                        sum.seen += 1;
                        for (sum, part) in sum.view.iter_mut().zip(towards) {
                            *sum += (part * VIEW_STEPS).round() as i64;
                        }
                    }
                }
                if in_tile {
                    found.stats.points += 1;
                    own_point(source, record);
                }
            }
            Ok(())
        },
    )?;
    found.stats.read = stats.read;
    found.stats.read_time = reading.elapsed();
    drop(table);
    if sums.is_empty() {
        return Ok(found);
    }
    sums.sort_unstable_by_key(|sum| order_key(sum.cell, cells / 4));

    // The side of a plane at an element: that of the stations which saw
    // its points, or else the side the fallback names. A fallback cannot
    // tell the side of a plane it sees edge on: the number whose sign it
    // takes is zero there but for the noise of the fit, another at every
    // element, and deciding by it tears the face in two. Such a plane
    // takes its default side, which is the same over the whole face.
    // `passing` is how far off a plane the point of the fallback must lie
    // to tell its side: see `SURE_SIDE`.
    let passing = match orientation.fallback {
        SurfelFallback::Upward => 0.0,
        SurfelFallback::Towards(target) => {
            let region = lattice.region;
            let far: f64 = (0..3)
                .map(|axis| {
                    let low = (target[axis] - region.min[axis]).abs();
                    low.max((target[axis] - region.max[axis]).abs()).powi(2)
                })
                .sum();
            (SURE_SIDE * far.sqrt().min(SURE_REACH)).max(SURE_DISTANCE * lattice.voxel)
        }
    };
    let side = |level: &SurfelLevel, index: usize, sum: &Sums, normal: [f64; 3]| -> Side {
        let seen = dot(normal, sum.view.map(|value| value as f64));
        if seen != 0.0 {
            return Side {
                flip: seen < 0.0,
                by_station: true,
                by_default: false,
                weak: seen.abs() < WEAK_VIEW * VIEW_STEPS * f64::from(sum.seen),
            };
        }
        let (flip, close) = default_side(normal);
        let default = Side {
            flip,
            by_station: false,
            by_default: true,
            weak: close,
        };
        match orientation.fallback {
            // Up tells the side of every plane that is not upright.
            SurfelFallback::Upward => Side {
                by_default: normal[2].abs() < UPRIGHT,
                ..default
            },
            SurfelFallback::Towards(target) => {
                // The cell counts in whole numbers and the place in it is
                // a short fraction, so this is the same in every tile.
                let world = lattice.world(std::array::from_fn(|axis| {
                    block.min[axis] as f64
                        + (f64::from(level.cell[index][axis]) + f64::from(level.frac[index][axis]))
                            * f64::from(level.size)
                }));
                let towards: [f64; 3] = std::array::from_fn(|axis| target[axis] - world[axis]);
                let front = dot(normal, towards).abs();
                if front >= passing {
                    Side {
                        flip: dot(normal, towards) < 0.0,
                        by_station: false,
                        by_default: false,
                        weak: front < 2.0 * passing
                            || front < WEAK_SIDE * dot(towards, towards).sqrt(),
                    }
                } else {
                    Side {
                        weak: close || front > 0.5 * passing,
                        ..default
                    }
                }
            }
        }
    };
    let own_cells = |level: &SurfelLevel, index: usize| {
        level.cell[index]
            .iter()
            .all(|value| own.contains(&((f32::from(*value) * level.size) as i64)))
    };

    let mut level = SurfelLevel::new(0, lattice.reach(0), cells, &sums);
    let mut around = Vec::new();
    let mut flat = vec![false; level.len()];
    let mut doubtful = vec![false; level.len()];
    let mut sides = vec![None; level.len()];
    for (index, sum) in sums.iter().enumerate() {
        proceed_every(index, proceed)?;
        let Some(fit) = level.fit(index, &mut around) else {
            continue;
        };
        let side = side(&level, index, sum, fit.normal);
        level.normal[index] = fit.normal.map(|value| {
            if side.flip {
                -value as f32
            } else {
                value as f32
            }
        });
        level.weight[index] = fit.confidence;
        level.fitted[index] = true;
        flat[index] = fit.bent.is_none();
        // A fit beside an edge may have turned onto the face next to the
        // one its points lie on, which their stations never saw from the
        // front. Only stations well in front of the plane through all the
        // neighbours, which lies between the faces, leave no doubt: that
        // is noise on a single face. The mean over the points counts, so
        // that stations on different sides leave a doubt as well.
        doubtful[index] = fit.bent.is_some_and(|first| {
            let front = dot(first, sum.view.map(|value| value as f64)).abs();
            sum.seen == 0 || front < SURE_VIEW * VIEW_STEPS * f64::from(sum.seen)
        });
        sides[index] = Some(side);
    }
    // Three kinds of elements take their side from their neighbours. One
    // in doubt beside an edge takes the side its neighbours with a flat
    // fit agree on. One whose points have no station takes the side of the
    // neighbours that stations told theirs: beside them the fallback may
    // name the other side, and elements that face both ways cancel each
    // other out. And one whose side had little to go by takes the side of
    // most of its fitted neighbours, which are no surer one by one but
    // mostly right together. All are judged by the sides as the fits gave
    // them and turned afterwards, so the outcome depends neither on the
    // order nor on the tile.
    let sure: Vec<bool> = (0..level.len())
        .map(|index| flat[index] && sides[index].is_some_and(|side| side.by_station && !side.weak))
        .collect();
    let any_sure = sure.contains(&true);
    let mut turned = Vec::new();
    for index in 0..level.len() {
        proceed_every(index, proceed)?;
        let Some(side) = &mut sides[index] else {
            continue;
        };
        let most = |level: &SurfelLevel| {
            level.neighbours_disagree(index, &level.fitted, WEAK_SIDE_CELLS, 0.0) == Some(true)
        };
        let against = if side.by_station && side.weak {
            most(&level)
        } else {
            let beside = if any_sure && (doubtful[index] || !side.by_station) {
                level.neighbours_disagree(index, &sure, SIDE_CELLS, SIDE_AGREEMENT)
            } else {
                None
            };
            match beside {
                Some(against) => {
                    side.by_default = false;
                    against
                }
                None if side.weak => most(&level),
                // With noise large for the cell no neighbour has a flat
                // fit, and a fit that is bent may be far off its face:
                // then all the neighbours count, as for a weak side.
                None => {
                    doubtful[index]
                        && level
                            .neighbours_disagree(index, &flat, SIDE_CELLS, SIDE_AGREEMENT)
                            .unwrap_or_else(|| most(&level))
                }
            }
        };
        if against {
            turned.push(index);
        }
    }
    for index in turned {
        level.normal[index] = level.normal[index].map(|value| -value);
    }
    for (index, side) in sides.into_iter().enumerate() {
        if let Some(side) = side.filter(|_| own_cells(&level, index)) {
            found.stats.surfels += 1;
            found.stats.by_station += u64::from(side.by_station);
            found.stats.by_default += u64::from(side.by_default);
        }
    }
    found.levels.push(level);

    for number in 1..=lattice.levels {
        let below = &found.levels[number - 1];
        let cells = cells >> number;
        // Children next to each other in the order of their parents.
        let mut order: Vec<(u64, u32)> = sums
            .iter()
            .enumerate()
            .map(|(index, sum)| {
                (
                    order_key(sum.cell.map(|value| value >> 1), cells / 4),
                    index as u32,
                )
            })
            .collect();
        order.sort_unstable();
        let mut merged: Vec<Sums> = Vec::new();
        // Per parent the summed weighted normals of its children.
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut any_child: Vec<bool> = Vec::new();
        let child_steps = (CELL_STEPS as u64) << (number - 1);
        for group in order.chunk_by(|a, b| a.0 == b.0) {
            let mut parent = Sums {
                cell: sums[group[0].1 as usize].cell.map(|value| value >> 1),
                ..Sums::default()
            };
            let mut normal = [0f32; 3];
            let mut usable = false;
            for (_, child) in group {
                let child = *child as usize;
                let sum = &sums[child];
                parent.count += sum.count;
                parent.colored += sum.colored;
                parent.seen += sum.seen;
                for axis in 0..3 {
                    parent.position[axis] += sum.position[axis]
                        + u64::from(sum.count) * u64::from(sum.cell[axis] & 1) * child_steps;
                    parent.rgb[axis] += sum.rgb[axis];
                    parent.view[axis] += sum.view[axis];
                }
                let weight = below.weight[child];
                if weight > 0.0 {
                    usable = true;
                    for (sum, part) in normal.iter_mut().zip(below.normal[child]) {
                        *sum += weight * part;
                    }
                }
            }
            merged.push(parent);
            normals.push(normal);
            any_child.push(usable);
        }
        let mut level = SurfelLevel::new(number, lattice.reach(number), cells, &merged);
        for index in 0..level.len() {
            proceed_every(index, proceed)?;
            if any_child[index] {
                // Two sheets back to back in one cell cancel each other
                // out: such a cell says nothing about either.
                let normal = normals[index];
                let length =
                    (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
                if length > 1e-3 {
                    level.normal[index] = normal.map(|value| value / length);
                    level.weight[index] = length;
                }
            } else if number <= MAX_FITTED_LEVEL {
                // Too few points for a plane at the finer level: the
                // surface is found at this one.
                if let Some(fit) = level.fit(index, &mut around) {
                    let side = side(&level, index, &merged[index], fit.normal);
                    level.normal[index] = fit.normal.map(|value| {
                        if side.flip {
                            -value as f32
                        } else {
                            value as f32
                        }
                    });
                    level.weight[index] = fit.confidence;
                    level.fitted[index] = true;
                    if own_cells(&level, index) {
                        found.stats.surfels += 1;
                        found.stats.by_station += u64::from(side.by_station);
                        found.stats.by_default += u64::from(side.by_default);
                    }
                }
            }
        }
        found.levels.push(level);
        sums = merged;
    }
    Ok(found)
}

/// Ask whether to go on once in a while during a long loop.
fn proceed_every(
    index: usize,
    proceed: &dyn Fn() -> Result<(), LoadError>,
) -> Result<(), LoadError> {
    if index.is_multiple_of(4_096) {
        proceed()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_shapes::{
        box_room, indexed_cloud, plane_with_hole, IndexedCloud, Noise, RoomSpec, Shape,
    };
    use crate::Point;
    use std::collections::HashMap;

    const BY_STATION: SurfelOrientation = SurfelOrientation {
        stations: true,
        fallback: SurfelFallback::Upward,
    };

    fn everything(_: usize, _: u64, _: &Point) -> bool {
        true
    }

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

    fn collect(
        lattice: &Lattice,
        sources: &[SurfelSource<'_>],
        orientation: SurfelOrientation,
        tile: [u32; 3],
    ) -> TileSurfels {
        collect_surfels(
            lattice,
            sources,
            &everything,
            orientation,
            tile,
            &|| Ok(()),
            &mut |_, _| {},
        )
        .unwrap()
    }

    /// World position of an element.
    fn world(lattice: &Lattice, surfels: &TileSurfels, level: usize, index: usize) -> [f64; 3] {
        let position = surfels.levels[level].position(index);
        lattice.world(std::array::from_fn(|axis| {
            surfels.block.min[axis] as f64 + f64::from(position[axis])
        }))
    }

    /// A floor of 1 x 1 m at a height that is no multiple of anything.
    fn floor(spacing: f64) -> Shape {
        let mut plane = plane_with_hole([1.0, 1.0], spacing, None);
        for point in &mut plane.points {
            point[2] = 0.3137;
        }
        plane
    }

    fn room_region() -> Bounds {
        Bounds {
            min: [0.0; 3],
            max: [4.0, 3.0, 2.6],
        }
    }

    #[test]
    fn lattice_follows_the_voxel_and_the_hole_limit() {
        // The default job: 2 cm voxels and gaps up to 25 cm.
        let lattice = Lattice::new(room_region(), 0.02, 0.25, DEFAULT_TILE_VOXELS).unwrap();
        assert_eq!((lattice.tile, lattice.halo, lattice.levels), (96, 32, 2));
        assert_eq!(lattice.close_radius, 6.25);
        assert_eq!(lattice.tiles, [3, 2, 2]);
        assert_eq!(lattice.reach(0), 2.5);
        assert_eq!(lattice.reach(1), 5.0);
        // The coarsest level reaches across half the widest gap and on.
        assert_eq!(lattice.reach(2), 10.75);
        let block = lattice.block([1, 0, 1]);
        assert_eq!((block.size, block.halo, block.tile), (160, 32, 96));
        assert_eq!(block.min, [64, -32, 64]);

        // Without closing gaps the margin is smallest.
        let open = Lattice::new(room_region(), 0.02, 0.0, DEFAULT_TILE_VOXELS).unwrap();
        assert_eq!((open.tile, open.halo, open.levels), (96, 16, 1));
        assert_eq!(open.close_radius, 0.0);
        // A wider limit takes a coarser level and a wider margin, and is
        // cut back to 32 voxels.
        let wide = Lattice::new(room_region(), 0.02, 0.5, DEFAULT_TILE_VOXELS).unwrap();
        assert_eq!((wide.halo, wide.levels, wide.close_radius), (64, 3, 12.5));
        let widest = Lattice::new(room_region(), 0.02, 3.2, DEFAULT_TILE_VOXELS).unwrap();
        assert_eq!(
            (widest.halo, widest.levels, widest.close_radius),
            (64, 3, 16.0)
        );
        // Tiles are whole blocks of the coarsest level.
        let odd = Lattice::new(room_region(), 0.02, 0.25, 50).unwrap();
        assert_eq!(odd.tile, 56);
        let small = Lattice::new(room_region(), 0.02, 0.25, 1).unwrap();
        assert_eq!(small.tile, 8);

        // Voxels lie at whole multiples of their size, whatever the region.
        let moved = Bounds {
            min: [207_000.123, 474_000.456, -3.3],
            max: [207_004.0, 474_003.0, 2.0],
        };
        for region in [room_region(), moved] {
            let lattice = Lattice::new(region, 0.02, 0.25, 96).unwrap();
            let origin = lattice.origin();
            let first = lattice.voxel_of(region.min);
            let last = lattice.voxel_of(region.max);
            for (axis, start) in origin.into_iter().enumerate() {
                let steps = start / 0.02;
                assert!((steps - steps.round()).abs() < 1e-6);
                // The region starts at least one voxel past the origin and
                // ends inside the last tile.
                assert!((8..24).contains(&first[axis]));
                assert!(last[axis] + 4 < i64::from(lattice.tiles[axis] * lattice.tile));
            }
            assert_eq!(lattice.world([0.0; 3]), origin);
            let voxel = lattice.voxel_of(region.max);
            assert_eq!(lattice.key_voxel(lattice.key(voxel)), voxel);
            let tile = lattice.tile_of(voxel).unwrap();
            assert_eq!(
                tile,
                voxel.map(|index| (index / i64::from(lattice.tile)) as u32)
            );
            // The part of the region a tile reads lies inside the region.
            let read = lattice.read_box(tile).unwrap();
            assert!((0..3).all(
                |axis| read.min[axis] >= region.min[axis] && read.max[axis] <= region.max[axis]
            ));
        }
        assert_eq!(lattice.tile_of([-1, 5, 5]), None);
        assert_eq!(lattice.tile_of([5, 5, 96 * 2]), None);

        // What a lattice cannot be made for.
        let refused = [
            Lattice::new(room_region(), 0.004, 0.25, 96),
            Lattice::new(room_region(), 0.6, 0.25, 96),
            Lattice::new(room_region(), f64::NAN, 0.25, 96),
            Lattice::new(room_region(), 0.02, -1.0, 96),
            Lattice::new(room_region(), 0.02, f64::INFINITY, 96),
            Lattice::new(
                Bounds {
                    min: [1.0, 0.0, 0.0],
                    max: [0.0, 1.0, 1.0],
                },
                0.02,
                0.25,
                96,
            ),
            Lattice::new(
                Bounds {
                    min: [0.0, f64::NEG_INFINITY, 0.0],
                    max: [1.0; 3],
                },
                0.02,
                0.25,
                96,
            ),
            // Six kilometres in voxels of 5 mm.
            Lattice::new(
                Bounds {
                    min: [0.0; 3],
                    max: [6_000.0, 10.0, 10.0],
                },
                0.005,
                0.25,
                96,
            ),
        ];
        for lattice in refused {
            assert!(matches!(lattice, Err(LoadError::InvalidData(_))));
        }
        // A region too long for its voxels says how long it may be and
        // what to do: a million voxels of 5 mm are 5 km.
        let long = Bounds {
            min: [0.0; 3],
            max: [6_000.0, 10.0, 10.0],
        };
        match Lattice::new(long, 0.005, 0.25, 96) {
            Err(LoadError::InvalidData(reason)) => {
                assert!(reason.contains("longer than 5 km"), "{reason}");
                assert!(reason.contains("section box") && reason.contains("larger voxel"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_tiles_near_points_are_planned() {
        let room = box_room(&RoomSpec::default());
        let cloud = indexed_cloud(&room.cloud_points(), 256);
        let sources = [source(&cloud, &room)];
        // Tiles of 0.64 m: the room is 7 x 5 x 5 of them.
        let lattice = Lattice::new(room.bounds(), 0.02, 0.0, 32).unwrap();
        let planned = plan_tiles(&lattice, &sources, &everything, &mut || Ok(())).unwrap();
        let all: u32 = lattice.tiles.iter().product();
        // Measured: 151 of the 175 tiles, six of them only because the
        // box of a leaf reaches into them.
        assert!(planned.len() < all as usize * 9 / 10, "{}", planned.len());
        let mut sorted = planned.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), planned.len());
        // The tile of every point is there, and no tile in the middle of
        // the room, which is empty.
        for point in &room.points {
            let tile = lattice.tile_of(lattice.voxel_of(*point)).unwrap();
            assert!(planned.contains(&tile));
        }
        let middle = lattice.tile_of(lattice.voxel_of([2.0, 1.5, 1.3])).unwrap();
        assert!(!planned.contains(&middle));

        // Points in memory are looked at one by one: the same or fewer
        // tiles than the boxes of the leaves gave.
        let points = crate::region_source::resident_points(&cloud.cloud, &mut |_| Ok(())).unwrap();
        let resident = [SurfelSource::without_stations(RegionSource::resident(
            &points,
            SourceTransform::default(),
        ))];
        let exact = plan_tiles(&lattice, &resident, &everything, &mut || Ok(())).unwrap();
        assert!(exact.iter().all(|tile| planned.contains(tile)));
        assert!(!exact.contains(&middle));
        assert!(exact.len() > 120 && exact.len() < planned.len());
        // A filter narrows the tiles where the points themselves are looked
        // at: here it keeps the points beside the west wall, in the first
        // of the seven tiles along x. Leaves of an index that reach over a
        // few tiles are marked by their box without being read, so there
        // the filter changes nothing.
        let west = |_: usize, _: u64, point: &Point| point.xyz[0] < 0.3;
        let narrowed = plan_tiles(&lattice, &resident, &west, &mut || Ok(())).unwrap();
        assert!(!narrowed.is_empty() && narrowed.len() < exact.len() / 3);
        assert!(narrowed
            .iter()
            .all(|tile| tile[0] <= 1 && exact.contains(tile)));
        let unread = plan_tiles(&lattice, &sources, &west, &mut || Ok(())).unwrap();
        assert_eq!(unread, planned);
        let nothing = |_: usize, _: u64, _: &Point| false;
        assert!(plan_tiles(&lattice, &resident, &nothing, &mut || Ok(()))
            .unwrap()
            .is_empty());

        // A region away from the points plans nothing.
        let away = Lattice::new(
            Bounds {
                min: [10.0, 0.0, 0.0],
                max: [12.0, 1.0, 1.0],
            },
            0.02,
            0.0,
            32,
        )
        .unwrap();
        assert!(plan_tiles(&away, &sources, &everything, &mut || Ok(()))
            .unwrap()
            .is_empty());
        assert!(plan_tiles(&away, &resident, &everything, &mut || Ok(()))
            .unwrap()
            .is_empty());

        // A stop is passed on, and a layer that streams its file is refused.
        let stopped = plan_tiles(&lattice, &sources, &everything, &mut || {
            Err(LoadError::Cancelled)
        });
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
        let streamed = [SurfelSource::new(
            &cloud.cloud,
            None,
            SourceTransform::default(),
        )];
        let refused = plan_tiles(&lattice, &streamed, &everything, &mut || Ok(()));
        assert!(matches!(refused, Err(LoadError::InvalidData(_))));
        let refused = collect_surfels(
            &lattice,
            &streamed,
            &everything,
            BY_STATION,
            planned[0],
            &|| Ok(()),
            &mut |_, _| {},
        );
        assert!(matches!(refused, Err(LoadError::InvalidData(_))));
    }

    #[test]
    fn a_plane_becomes_elements_that_face_its_station() {
        let plane = floor(0.005).with_stations(&[[0.5, 0.5, 2.0]]);
        let cloud = indexed_cloud(&plane.cloud_points(), 4_096);
        let sources = [source(&cloud, &plane)];
        let lattice = Lattice::new(plane.bounds(), 0.02, 0.25, 96).unwrap();
        assert_eq!(lattice.tiles, [1, 1, 1]);
        let mut own = 0;
        let surfels = collect_surfels(
            &lattice,
            &sources,
            &everything,
            BY_STATION,
            [0, 0, 0],
            &|| Ok(()),
            &mut |source, record| {
                assert_eq!(source, 0);
                assert_eq!(record.point.xyz, plane.points[record.ordinal as usize]);
                own += 1;
            },
        )
        .unwrap();
        assert_eq!(own, 40_000);
        assert_eq!(surfels.stats.points, 40_000);
        assert_eq!(surfels.stats.read, 40_000);
        assert_eq!(surfels.levels.len(), 3);
        // Cells of 1 cm: four points in each.
        let finest = &surfels.levels[0];
        assert_eq!(finest.len(), 10_000);
        assert_eq!(surfels.stats.surfels, 10_000);
        assert_eq!(surfels.stats.by_station, 10_000);
        assert_eq!((finest.size, finest.reach), (0.5, 2.5));
        for index in 0..finest.len() {
            assert_eq!(finest.count[index], 4);
            assert!(finest.fitted[index]);
            // Flat all around: full confidence.
            assert!((finest.weight[index] - 1.0).abs() < 1e-6);
            let normal = finest.normal[index];
            assert!(normal[2] > 0.999_999 && normal[0].abs() < 1e-4 && normal[1].abs() < 1e-4);
            let position = world(&lattice, &surfels, 0, index);
            assert!((position[2] - 0.3137).abs() < 1e-6, "{position:?}");
            // The mean of the four points lies in the middle of the cell.
            assert!((position[0] * 100.0).fract() > 0.49 && (position[0] * 100.0).fract() < 0.51);
            assert!(finest.frac[index]
                .iter()
                .all(|part| (0.0..1.0).contains(part)));
            // The colour of the test points encodes their normal.
            assert_eq!(finest.color[index], [128, 128, 228, 255]);
        }
        // Every next level merges 2 x 2 cells of the plane and adds up
        // their weights.
        for (number, cells) in [(1, 2_500), (2, 625)] {
            let level = &surfels.levels[number];
            assert_eq!(level.len(), cells);
            for index in 0..level.len() {
                assert!(!level.fitted[index]);
                assert_eq!(level.count[index], 4 * 4u32.pow(number as u32));
                assert!((level.weight[index] - 4f32.powi(number as i32)).abs() < 1e-3);
                assert!(level.normal[index][2] > 0.999_999);
                let position = world(&lattice, &surfels, number, index);
                assert!((position[2] - 0.3137).abs() < 1e-6);
            }
        }
        // All elements within a few cells are found, each once, with the
        // offset of its cell.
        let center = finest.cell[5_000].map(i32::from);
        let mut found = Vec::new();
        finest.for_each_near(center, 3, |index, offset| {
            assert_eq!(
                finest.cell[index].map(i32::from),
                std::array::from_fn(|axis| center[axis] + offset[axis])
            );
            found.push(index);
        });
        assert_eq!(found.len(), 49);
        assert!(found.windows(2).all(|pair| pair[0] < pair[1]));
        // From an element to a place: here to a point 3 voxels above the
        // corner of its own voxel.
        let cell = finest.cell[5_000];
        let whole = [
            i32::from(cell[0] / 2),
            i32::from(cell[1] / 2),
            i32::from(cell[2] / 2) + 3,
        ];
        let offset = finest.offset(5_000, whole, [0.0; 3]);
        let position = finest.position(5_000);
        for axis in 0..3 {
            assert!((offset[axis] - (whole[axis] as f32 - position[axis])).abs() < 1e-5);
        }

        // Seen from below the same plane faces down.
        let below = floor(0.005).with_stations(&[[0.5, 0.5, -2.0]]);
        let sources = [source(&cloud, &below)];
        let surfels = collect(&lattice, &sources, BY_STATION, [0, 0, 0]);
        assert!(surfels.levels[0]
            .normal
            .iter()
            .all(|normal| normal[2] < -0.999_999));
        assert!(surfels.levels[2]
            .normal
            .iter()
            .all(|normal| normal[2] < -0.999_999));
    }

    #[test]
    fn without_a_station_the_fallback_gives_the_side() {
        let plane = floor(0.005);
        let cloud = indexed_cloud(&plane.cloud_points(), 4_096);
        let lattice = Lattice::new(plane.bounds(), 0.02, 0.0, 96).unwrap();
        let unknown = [source(&cloud, &plane)];
        let sides = |sources: &[SurfelSource<'_>], orientation: SurfelOrientation| {
            let surfels = collect(&lattice, sources, orientation, [0, 0, 0]);
            let up = surfels.levels[0]
                .normal
                .iter()
                .filter(|normal| normal[2] > 0.999)
                .count();
            let down = surfels.levels[0]
                .normal
                .iter()
                .filter(|normal| normal[2] < -0.999)
                .count();
            (up, down, surfels.stats.by_station, surfels.stats.by_default)
        };
        let towards = |target: [f64; 3], stations: bool| SurfelOrientation {
            stations,
            fallback: SurfelFallback::Towards(target),
        };
        assert_eq!(sides(&unknown, BY_STATION), (10_000, 0, 0, 0));
        assert_eq!(
            sides(&unknown, towards([0.5, 0.5, 5.0], true)),
            (10_000, 0, 0, 0)
        );
        assert_eq!(
            sides(&unknown, towards([0.5, 0.5, -5.0], true)),
            (0, 10_000, 0, 0)
        );
        // A point in the plane of the floor, or a voxel above or below it,
        // tells no side: the floor takes its default side, which is up,
        // and says so.
        for height in [0.3137, 0.3337, 0.2937] {
            assert_eq!(
                sides(&unknown, towards([0.5, 0.5, height], true)),
                (10_000, 0, 0, 10_000),
                "{height}"
            );
        }
        // A station wins over the fallback, unless stations are not used.
        let below = floor(0.005).with_stations(&[[0.5, 0.5, -2.0]]);
        let known = [source(&cloud, &below)];
        assert_eq!(sides(&known, BY_STATION), (0, 10_000, 10_000, 0));
        assert_eq!(
            sides(&known, towards([0.5, 0.5, 5.0], true)),
            (0, 10_000, 10_000, 0)
        );
        assert_eq!(
            sides(&known, towards([0.5, 0.5, 5.0], false)),
            (10_000, 0, 0, 0)
        );
        let upward = SurfelOrientation {
            stations: false,
            fallback: SurfelFallback::Upward,
        };
        assert_eq!(sides(&known, upward), (10_000, 0, 0, 0));
    }

    #[test]
    fn the_default_side_follows_from_the_normal_alone() {
        // Up for everything that is not upright.
        assert_eq!(default_side([0.0, 0.0, 1.0]), (false, false));
        assert_eq!(default_side([0.0, 0.0, -1.0]), (true, false));
        assert_eq!(default_side([0.6, 0.0, 0.8]), (false, false));
        assert_eq!(default_side([0.0, -0.9, -0.436]), (true, false));
        // One direction for upright planes: walls along the axes and at 45
        // degrees to them, with a little tilt either way, all face it.
        for degrees in [0.0f64, 45.0, 90.0, 135.0, 180.0, 225.0, 270.0, 315.0] {
            let (sin, cos) = degrees.to_radians().sin_cos();
            let front = cos * UPRIGHT_SIDE[0] + sin * UPRIGHT_SIDE[1] > 0.0;
            for tilt in [-0.05, 0.0, 0.05] {
                assert_eq!(
                    default_side([cos, sin, tilt]),
                    (!front, false),
                    "{degrees} {tilt}"
                );
            }
        }
        // The two normals of one plane get the same side.
        let mut rng = crate::test_shapes::Rng::new(3);
        for _ in 0..1_000 {
            let normal: [f64; 3] = std::array::from_fn(|_| rng.gaussian());
            let Some(normal) = unit(normal) else {
                continue;
            };
            let turned = normal.map(|value| -value);
            assert_ne!(default_side(normal).0, default_side(turned).0);
            assert_eq!(default_side(normal).1, default_side(turned).1);
        }
        // Close to where the answer changes, the neighbours are asked:
        // between upright and not, and across the direction for upright
        // planes.
        assert!(default_side([0.97, 0.0, 0.25]).1);
        assert!(default_side([0.99, 0.0, 0.15]).1);
        assert!(!default_side([0.9, 0.0, 0.436]).1);
        assert!(default_side([-0.469, 0.883, 0.0]).1);
        assert!(default_side([-0.38, 0.925, 0.0]).1);
        assert!(!default_side([-0.2, 0.98, 0.0]).1);
        assert!((UPRIGHT_SIDE[0].hypot(UPRIGHT_SIDE[1]) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn elements_without_a_station_follow_those_beside_them_that_have_one() {
        // A floor seen from above, of which only every fifth point knows
        // its station; the fallback names the other side.
        let plane = floor(0.005);
        let total = plane.points.len() as u64;
        let records = |keep: &dyn Fn(u64) -> bool| -> Vec<IndexedPoint> {
            plane
                .cloud_points()
                .into_iter()
                .enumerate()
                .filter(|(ordinal, _)| keep(*ordinal as u64))
                .map(|(ordinal, point)| IndexedPoint {
                    point,
                    ordinal: ordinal as u64,
                })
                .collect()
        };
        let (known, unknown) = (
            records(&|ordinal| ordinal % 5 == 0),
            records(&|ordinal| ordinal % 5 != 0),
        );
        assert_eq!((known.len() + unknown.len()) as u64, total);
        let transform = SourceTransform::default();
        let layers = [
            SurfelSource::with_stations(
                RegionSource::resident(&known, transform),
                vec![[0.5, 0.5, 2.0]],
                Vec::new(),
            ),
            SurfelSource::without_stations(RegionSource::resident(&unknown, transform)),
        ];
        let lattice = Lattice::new(plane.bounds(), 0.02, 0.0, 96).unwrap();
        let downward = SurfelOrientation {
            stations: true,
            fallback: SurfelFallback::Towards([0.5, 0.5, -5.0]),
        };
        let surfels = collect(&lattice, &layers, downward, [0, 0, 0]);
        let level = &surfels.levels[0];
        assert_eq!(level.len(), 10_000);
        assert!(level.normal.iter().all(|normal| normal[2] > 0.999_999));
        // Measured: 4,000 of the cells hold a point that knows its station.
        // The others say they have none, and that something still told
        // their side.
        let stats = surfels.stats;
        assert_eq!((stats.surfels, stats.by_default), (10_000, 0));
        assert!(
            stats.by_station > 3_500 && stats.by_station < 4_500,
            "{}",
            stats.by_station
        );
        // On their own the points without a station follow the fallback.
        let alone = collect(&lattice, &layers[1..], downward, [0, 0, 0]);
        assert!(alone.levels[0]
            .normal
            .iter()
            .all(|normal| normal[2] < -0.999_999));
        assert_eq!(alone.stats.by_station, 0);
    }

    #[test]
    fn beside_an_edge_elements_keep_their_face_and_its_side() {
        // The top of a block seen at a low angle from far to the left, and
        // its right face seen from straight in front of it.
        let mut block = Shape {
            stations: vec![[-5.0, 0.5, 0.3], [5.0, 0.5, -0.5]],
            ..Shape::default()
        };
        let steps =
            |length: f64| (0..(length / 0.005) as usize).map(|step| (step as f64 + 0.5) * 0.005);
        for y in steps(1.0) {
            for along in steps(1.0) {
                block.points.push([along, y, 0.0]);
                block.normals.push([0.0, 0.0, 1.0]);
                block.station_of.push(0);
            }
        }
        for y in steps(1.0) {
            for down in steps(1.0) {
                block.points.push([1.0, y, -down]);
                block.normals.push([1.0, 0.0, 0.0]);
                block.station_of.push(1);
            }
        }
        // Turned a little, so that neither face follows the lattice.
        let block = block.transformed(7.0, [0.013, 0.007, 0.004]);
        let cloud = indexed_cloud(&block.cloud_points(), 4_096);
        let sources = [source(&cloud, &block)];
        let lattice = Lattice::new(block.bounds(), 0.02, 0.0, 256).unwrap();
        let surfels = collect(&lattice, &sources, BY_STATION, [0, 0, 0]);
        let level = &surfels.levels[0];
        let (sin, cos) = 7f64.to_radians().sin_cos();
        let (mut top, mut side, mut at_edge) = (0, 0, 0);
        for index in 0..level.len() {
            assert!(level.fitted[index]);
            let position = world(&lattice, &surfels, 0, index);
            let normal = level.normal[index].map(f64::from);
            // How far the element lies from the edge, along each face.
            let from_top = 0.004 - position[2];
            let from_side = 1.0 - (cos * (position[0] - 0.013) + sin * (position[1] - 0.007));
            let outward = cos * normal[0] + sin * normal[1];
            if from_top < 0.001 && from_side > 0.015 {
                // On the top, a cell and a half from the edge or more.
                // Measured: the cosine is 0.9992 or more.
                top += 1;
                assert!(normal[2] > 0.995, "{position:?} {normal:?}");
            } else if from_side < 0.001 && from_top > 0.015 {
                side += 1;
                assert!(outward > 0.995, "{position:?} {normal:?}");
            } else {
                // At the edge itself the plane may be either face or lie
                // between them, but it looks out of the block.
                at_edge += 1;
                assert!(
                    normal[2] > -0.1 && outward > -0.1,
                    "{position:?} {normal:?}"
                );
                assert!(normal[2] + outward > 0.9, "{position:?} {normal:?}");
            }
        }
        assert!(
            top > 9_000 && side > 9_000 && at_edge > 200,
            "{top} {side} {at_edge}"
        );
    }

    #[test]
    fn stations_are_found_through_the_scan_ranges() {
        // Three scans in one file: at (10, 20, 30), without a station, and
        // at (40, 50, 60).
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("three-blocks.ptx");
        std::fs::write(&path, crate::ptx::tests::THREE_BLOCKS).unwrap();
        let cloud = crate::open(&path, 10).unwrap();
        assert_eq!(cloud.scan_ranges, crate::ptx::tests::three_block_ranges());
        // The layer stands mirrored and moved in the scene; so do its
        // stations.
        let transform = SourceTransform {
            scale: [1.0, -1.0, 1.0],
            offset: [100.0, 0.0, 5.0],
        };
        let source = SurfelSource::new(&cloud, None, transform);
        assert_eq!(
            source.stations(),
            [[110.0, -20.0, 35.0], [140.0, -50.0, 65.0]]
        );
        let station = |ordinal| source.station_of(ordinal);
        assert_eq!(station(0), Some([110.0, -20.0, 35.0]));
        assert_eq!(station(2), Some([110.0, -20.0, 35.0]));
        assert_eq!(station(3), None);
        assert_eq!(station(4), None);
        assert_eq!(station(5), Some([140.0, -50.0, 65.0]));
        // Beyond the points of the file, and the mark for an ordinal that
        // is not known.
        assert_eq!(station(6), None);
        assert_eq!(station(u64::MAX), None);

        // Given by hand: one station without ranges measured everything,
        // several stations without ranges tell nothing.
        let points = RegionSource::resident(&[], SourceTransform::default());
        let one = SurfelSource::with_stations(points, vec![[1.0, 2.0, 3.0]], Vec::new());
        assert_eq!(one.station_of(123_456), Some([1.0, 2.0, 3.0]));
        let two = SurfelSource::with_stations(points, vec![[1.0; 3], [2.0; 3]], Vec::new());
        assert_eq!(two.station_of(0), None);
        // A range that names a station the layer does not have.
        let short = SurfelSource::with_stations(
            points,
            vec![[1.0; 3]],
            vec![
                ScanRange {
                    first_ordinal: 0,
                    station: Some(0),
                },
                ScanRange {
                    first_ordinal: 10,
                    station: Some(4),
                },
            ],
        );
        assert_eq!(short.station_of(9), Some([1.0; 3]));
        assert_eq!(short.station_of(10), None);
        assert_eq!(SurfelSource::without_stations(points).station_of(0), None);

        // A cloud without scans has no stations.
        let plain = indexed_cloud(&floor(0.05).cloud_points(), 256);
        let source =
            SurfelSource::new(&plain.cloud, Some(&plain.index), SourceTransform::default());
        assert!(source.stations().is_empty());
        assert_eq!(source.station_of(0), None);
    }

    #[test]
    fn sparse_points_are_fitted_one_level_up() {
        // A point every two voxels.
        let plane = floor(0.04).with_stations(&[[0.5, 0.5, 2.0]]);
        let cloud = indexed_cloud(&plane.cloud_points(), 4_096);
        let sources = [source(&cloud, &plane)];
        let lattice = Lattice::new(plane.bounds(), 0.02, 0.0, 96).unwrap();
        let surfels = collect(&lattice, &sources, BY_STATION, [0, 0, 0]);
        let (finest, coarser) = (&surfels.levels[0], &surfels.levels[1]);
        assert_eq!((finest.len(), coarser.len()), (625, 625));
        // The elements that found their plane one level up are counted, and
        // so is where their side came from.
        assert_eq!(
            (
                surfels.stats.surfels,
                surfels.stats.by_station,
                surfels.stats.by_default
            ),
            (625, 625, 0)
        );
        assert!(finest.weight.iter().all(|weight| *weight == 0.0));
        assert!(finest.fitted.iter().all(|fitted| !fitted));
        assert!(coarser.fitted.iter().all(|fitted| *fitted));
        assert!(coarser
            .weight
            .iter()
            .all(|weight| (*weight - 1.0).abs() < 1e-6));
        assert!(coarser.normal.iter().all(|normal| normal[2] > 0.999_999));

        // A line of points has no plane at any level.
        let mut line = floor(0.005);
        line.points.retain(|point| (point[1] - 0.5025).abs() < 1e-9);
        assert_eq!(line.points.len(), 200);
        line.normals.truncate(200);
        line.station_of.truncate(200);
        let cloud = indexed_cloud(&line.cloud_points(), 4_096);
        let sources = [source(&cloud, &line)];
        let lattice = Lattice::new(line.bounds(), 0.02, 0.0, 96).unwrap();
        let surfels = collect(&lattice, &sources, BY_STATION, [0, 0, 0]);
        assert_eq!(surfels.levels[0].len(), 100);
        for level in &surfels.levels {
            assert!(level.weight.iter().all(|weight| *weight == 0.0));
        }
    }

    /// Every element of a tile by level and world cell, with all it holds
    /// as bits.
    type Elements = HashMap<(usize, [i64; 3]), ([u32; 3], [u32; 3], u32, bool, [u8; 4], u32)>;

    /// The elements of a tile that lie `margin` voxels or more inside the
    /// block the tile read.
    fn elements(surfels: &TileSurfels, margin: f32) -> Elements {
        let mut found = HashMap::new();
        for (number, level) in surfels.levels.iter().enumerate() {
            let cells_per_voxel = (1.0 / level.size) as i64;
            for index in 0..level.len() {
                let low = level.cell[index].map(|value| f32::from(value) * level.size);
                let inside = low.iter().all(|value| {
                    *value >= margin && value + level.size <= surfels.block.size as f32 - margin
                });
                if !inside {
                    continue;
                }
                // The cell counted from the lattice origin: per voxel two
                // cells at the finest level, half a cell and less above.
                let cell: [i64; 3] = std::array::from_fn(|axis| {
                    let own = i64::from(level.cell[index][axis]);
                    if number == 0 {
                        surfels.block.min[axis] * cells_per_voxel + own
                    } else {
                        surfels.block.min[axis] / (1 << (number - 1)) + own
                    }
                });
                found.insert(
                    (number, cell),
                    (
                        level.frac[index].map(f32::to_bits),
                        level.normal[index].map(f32::to_bits),
                        level.weight[index].to_bits(),
                        level.fitted[index],
                        level.color[index],
                        level.count[index],
                    ),
                );
            }
        }
        found
    }

    #[test]
    fn neighbouring_tiles_hold_the_same_elements_bit_for_bit() {
        let room = box_room(&RoomSpec::default()).with_noise(Noise::Gaussian(0.002), 5);
        let cloud = indexed_cloud(&room.cloud_points(), 1_024);
        // The room as scanned; seen from a station far outside it, which
        // looks along the floor, the ceiling and two walls, so that their
        // elements take their side from their neighbours; with a station
        // for the first half of the points only; and without stations,
        // facing a point that lies in the floor.
        let far = room.clone().with_stations(&[[-25.0, 1.4, 1.2]]);
        let mut half = ranges(&room);
        half.push(ScanRange {
            first_ordinal: room.points.len() as u64 / 2,
            station: None,
        });
        let towards = |target: [f64; 3]| SurfelOrientation {
            stations: true,
            fallback: SurfelFallback::Towards(target),
        };
        let cases = [
            (source(&cloud, &room), BY_STATION),
            (source(&cloud, &far), BY_STATION),
            (
                SurfelSource::with_stations(
                    RegionSource::new(&cloud.cloud, Some(&cloud.index), SourceTransform::default()),
                    room.stations.clone(),
                    half,
                ),
                towards([2.0, 1.5, 5.0]),
            ),
            (
                source(&cloud, &room.clone().with_stations(&[])),
                towards([2.0, 1.5, 0.0]),
            ),
        ];
        let lattice = Lattice::new(room.bounds(), 0.04, 0.25, 32).unwrap();
        // Within this many voxels of the edge of what a tile read, a plane
        // fit may lack neighbours.
        let margin = 6.0 + (1 << (lattice.levels - 1)) as f32;
        for (number, (source, orientation)) in cases.into_iter().enumerate() {
            let sources = [source];
            let mut compared = 0;
            for (a, b) in [
                ([0, 0, 0], [1, 0, 0]),
                ([1, 1, 0], [1, 1, 1]),
                ([1, 0, 1], [2, 1, 1]),
            ] {
                let first = elements(&collect(&lattice, &sources, orientation, a), margin);
                let second = elements(&collect(&lattice, &sources, orientation, b), margin);
                let mut shared = 0;
                for (key, element) in &first {
                    if let Some(other) = second.get(key) {
                        assert_eq!(element, other, "{number} {key:?}");
                        shared += 1;
                    }
                }
                assert!(shared > 500, "{number} {shared}");
                compared += shared;
            }
            assert!(compared > 5_000);
        }
    }

    #[test]
    fn elements_do_not_depend_on_how_the_points_are_split_or_ordered() {
        let room = box_room(&RoomSpec::default()).with_noise(Noise::Gaussian(0.002), 5);
        let lattice = Lattice::new(room.bounds(), 0.04, 0.25, 96).unwrap();
        let whole = indexed_cloud(&room.cloud_points(), 1_024);
        let sources = [source(&whole, &room)];
        let expected = elements(&collect(&lattice, &sources, BY_STATION, [0, 0, 0]), 0.0);
        assert!(expected.len() > 30_000);

        // The same points as two layers, the second one written backwards,
        // read from memory.
        let records = |keep: &dyn Fn(usize) -> bool, backwards: bool| -> Vec<IndexedPoint> {
            let mut records: Vec<IndexedPoint> = room
                .cloud_points()
                .into_iter()
                .enumerate()
                .filter(|(ordinal, _)| keep(*ordinal))
                .map(|(ordinal, point)| IndexedPoint {
                    point,
                    ordinal: ordinal as u64,
                })
                .collect();
            if backwards {
                records.reverse();
            }
            records
        };
        let (even, odd) = (
            records(&|ordinal| ordinal % 2 == 0, false),
            records(&|ordinal| ordinal % 2 == 1, true),
        );
        let layers = [&odd, &even].map(|points| {
            SurfelSource::with_stations(
                RegionSource::resident(points, SourceTransform::default()),
                room.stations.clone(),
                Vec::new(),
            )
        });
        let split = elements(&collect(&lattice, &layers, BY_STATION, [0, 0, 0]), 0.0);
        assert_eq!(expected, split);

        // A filter leaves points out before they are counted.
        let half = collect_surfels(
            &lattice,
            &sources,
            &|_, ordinal, _| ordinal % 2 == 0,
            BY_STATION,
            [0, 0, 0],
            &|| Ok(()),
            &mut |_, record| assert_eq!(record.ordinal % 2, 0),
        )
        .unwrap();
        let only_even = [SurfelSource::with_stations(
            RegionSource::resident(&even, SourceTransform::default()),
            room.stations.clone(),
            Vec::new(),
        )];
        assert_eq!(
            elements(&half, 0.0),
            elements(&collect(&lattice, &only_even, BY_STATION, [0, 0, 0]), 0.0)
        );
        // And a stop is passed on.
        let stopped = collect_surfels(
            &lattice,
            &sources,
            &everything,
            BY_STATION,
            [0, 0, 0],
            &|| Err(LoadError::Cancelled),
            &mut |_, _| {},
        );
        assert!(matches!(stopped, Err(LoadError::Cancelled)));
    }
}
