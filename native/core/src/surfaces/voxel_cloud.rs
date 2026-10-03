//! The working set of the face detection: one point per occupied voxel,
//! built while the source points stream by, so that its size follows the
//! surface inside the region and not the number of points read.
//!
//! Each voxel keeps the mean of its points. Next to a plane the mean lies on
//! it, whatever the density of the scan, where a single point picked from the
//! voxel would carry that point's noise into every fit. The sums are whole
//! numbers, so the set does not depend on the order in which the points
//! arrive, as long as the voxel size did not have to grow on the way.
//!
//! When more voxels are occupied than the budget allows, the voxel size is
//! doubled and the set is folded into the coarser lattice; reading goes on.
//! The lattice is nested, so the size reached at the end is the smallest
//! doubling that fits the budget, whatever the order of the points.
//!
//! Once all points are in, the voxel size is doubled further when the scan
//! turns out to be thinner than the voxels: regions grow from voxel to
//! neighbouring voxel, so a face only holds together when the voxels along
//! it are occupied.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hash, Hasher};

use rayon::prelude::*;

/// Voxels along an axis that a key can tell apart: 32,000 km of voxels of
/// 3 cm, so that the extent of the sources never decides the voxel size. A
/// cell number plus the offset to a neighbour still fits an `i32`.
pub(crate) const MAX_AXIS_CELLS: i64 = 1 << 30;
/// Offsets from a voxel centre are counted in this many steps per voxel.
const STEPS: i64 = 4_096;
/// Points a voxel takes into its mean. More change nothing that matters, and
/// the sums of this many offsets still fit their 32 bits.
const MAX_COUNT: i64 = 1 << 19;
/// A voxel inside a scanned surface has the eight voxels of that surface
/// round it. With fewer than this many of them occupied, the points lie
/// further apart than a voxel is wide.
const DENSE_AROUND: u32 = 6;
/// A surface at any angle, also one on a voxel boundary, passes through at
/// most 18 of the 26 voxels round one of its own. More than this many
/// occupied means points on all sides.
const FILLED_AROUND: u32 = 23;
/// All of them occupied is what no meeting of surfaces gives: in the corner
/// of three walls that each fill two layers, one is still empty.
const ENCLOSED_AROUND: u32 = 26;
/// The voxel size is doubled at most this often for the density: what is
/// still thin after that is no surface.
pub(crate) const DENSITY_DOUBLINGS: u32 = 2;
/// Station number of a point whose station is not known.
pub(crate) const NO_STATION: u32 = u32::MAX;

/// The cell of a voxel: x, y, z. Twelve bytes, so that a slot of the index
/// is as large as with a packed 64-bit number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Key([u32; 3]);

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let [x, y, z] = self.0.map(u64::from);
        state.write_u64((x | y << 32) ^ z.wrapping_mul(0xc2b2_ae3d_27d4_eb4f));
    }
}

/// The order of the working points: by z, then y, then x.
impl Ord for Key {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        let [x, y, z] = self.0;
        [z, y, x].cmp(&[other.0[2], other.0[1], other.0[0]])
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A key is hashed as one number, so one multiplication spreads it.
#[derive(Default)]
pub(crate) struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn write_u64(&mut self, key: u64) {
        let spread = key.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = spread ^ (spread >> 32);
    }
}

type KeyMap = HashMap<Key, u32, BuildHasherDefault<KeyHasher>>;
/// Tells whether a voxel is occupied.
type Holds<'a> = &'a (dyn Fn(&Key) -> bool + Sync);

fn pack(cell: [i32; 3]) -> Option<Key> {
    cell.iter()
        .all(|value| (0..MAX_AXIS_CELLS).contains(&i64::from(*value)))
        .then(|| Key(cell.map(|value| value as u32)))
}

fn unpack(key: Key) -> [i32; 3] {
    key.0.map(|value| value as i32)
}

pub(crate) struct VoxelCloud {
    origin: [f64; 3],
    voxel: f64,
    budget: usize,
    index: KeyMap,
    keys: Vec<Key>,
    /// Sum of the offsets of the points from the voxel centre, in steps.
    sums: Vec<[i32; 3]>,
    counts: Vec<u32>,
    /// The lowest station number among the points of the voxel.
    stations: Vec<u32>,
}

impl VoxelCloud {
    /// An empty set of voxels of `voxel` metres that holds at most `budget`
    /// of them. `origin` is the corner of voxel (0, 0, 0); every point must
    /// lie at or beyond it on all axes.
    pub(crate) fn new(origin: [f64; 3], voxel: f64, budget: usize) -> Self {
        Self {
            origin,
            voxel,
            budget: budget.max(1),
            index: KeyMap::default(),
            keys: Vec::new(),
            sums: Vec::new(),
            counts: Vec::new(),
            stations: Vec::new(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }

    /// The voxel size in use, which is the size asked for times a power of
    /// two.
    pub(crate) fn voxel(&self) -> f64 {
        self.voxel
    }

    /// Double the voxel size until `extent` metres from the origin fit the
    /// keys. Only before the first point.
    pub(crate) fn fit_extent(&mut self, extent: f64) {
        debug_assert!(self.keys.is_empty());
        while extent / self.voxel >= (MAX_AXIS_CELLS - 1) as f64 && self.voxel.is_finite() {
            self.voxel *= 2.0;
        }
    }

    /// The voxel that holds a position given relative to the origin.
    pub(crate) fn cell_of(&self, local: [f64; 3]) -> Option<[i32; 3]> {
        let mut cell = [0; 3];
        for axis in 0..3 {
            let whole = (local[axis] / self.voxel).floor();
            // Written so that a NaN fails the test.
            if !(whole >= 0.0 && whole < MAX_AXIS_CELLS as f64) {
                return None;
            }
            cell[axis] = whole as i32;
        }
        Some(cell)
    }

    /// The working point of a voxel, if the voxel holds points.
    pub(crate) fn find(&self, cell: [i32; 3]) -> Option<u32> {
        self.index.get(&pack(cell)?).copied()
    }

    pub(crate) fn cell(&self, point: u32) -> [i32; 3] {
        unpack(self.keys[point as usize])
    }

    /// The mean of the points of a voxel, relative to the origin.
    pub(crate) fn position(&self, point: u32) -> [f64; 3] {
        let cell = self.cell(point);
        let sum = self.sums[point as usize];
        let steps = f64::from(self.counts[point as usize]) * STEPS as f64;
        std::array::from_fn(|axis| {
            (f64::from(cell[axis]) + 0.5 + f64::from(sum[axis]) / steps) * self.voxel
        })
    }

    /// How many points the mean of a voxel stands for.
    pub(crate) fn weight(&self, point: u32) -> u64 {
        u64::from(self.counts[point as usize])
    }

    pub(crate) fn station(&self, point: u32) -> u32 {
        self.stations[point as usize]
    }

    /// Take a point, given in the coordinates of the origin, into the mean
    /// of its voxel. A point outside the lattice is left out.
    pub(crate) fn add(&mut self, xyz: [f64; 3], station: u32) {
        let local: [f64; 3] = std::array::from_fn(|axis| xyz[axis] - self.origin[axis]);
        let Some(cell) = self.cell_of(local) else {
            return;
        };
        let offset: [i32; 3] = std::array::from_fn(|axis| {
            let within = local[axis] / self.voxel - f64::from(cell[axis]) - 0.5;
            (within * STEPS as f64)
                .round()
                .clamp(-(STEPS as f64) / 2.0, STEPS as f64 / 2.0) as i32
        });
        let Some(key) = pack(cell) else {
            return;
        };
        let next = self.keys.len() as u32;
        let point = *self.index.entry(key).or_insert(next) as usize;
        if point == self.keys.len() {
            self.keys.push(key);
            self.sums.push(offset);
            self.counts.push(1);
            self.stations.push(station);
            while self.keys.len() > self.budget && self.keys.len() > 1 && self.voxel.is_finite() {
                self.double();
            }
            return;
        }
        self.stations[point] = self.stations[point].min(station);
        if i64::from(self.counts[point]) < MAX_COUNT {
            self.counts[point] += 1;
            for (sum, offset) in self.sums[point].iter_mut().zip(offset) {
                *sum += offset;
            }
        }
    }

    /// Double the voxel size until the voxels are no finer than the points
    /// lie apart: until half of them have the voxels of a surface round
    /// them. Regions grow from voxel to neighbouring voxel, so the faces of
    /// a thin scan fall apart in voxels that are mostly empty. Points that
    /// fill space instead of lying on surfaces keep their size: their means
    /// in large voxels would form a lattice, which is full of planes.
    /// Returns how often the size was doubled. Before `finish`; the lattice
    /// is nested, so the size reached does not depend on the order of the
    /// points.
    pub(crate) fn fit_density(&mut self) -> u32 {
        let Some(doublings) = (0..=DENSITY_DOUBLINGS).find(|level| self.layout(*level).0) else {
            return 0;
        };
        if doublings == 0 || self.layout(doublings + 1).1 {
            return 0;
        }
        for _ in 0..doublings {
            self.double();
        }
        doublings
    }

    /// Of the voxels `level` doublings on: whether half of them have the
    /// voxels of a surface round them, and whether they fill space. They do
    /// when a quarter of them have points on all sides, which the voxels at
    /// the outside of a filled space have not, or when an eighth of those
    /// inside the box round them are enclosed altogether, which tells for a
    /// space so small that most of it is outside.
    fn layout(&self, level: u32) -> (bool, bool) {
        // A scan that is dense enough is only looked at as it is, which
        // needs no second set of keys.
        let coarse: HashSet<Key, BuildHasherDefault<KeyHasher>>;
        let folded: Vec<Key>;
        let (list, holds): (&[Key], Holds<'_>) = if level == 0 {
            (&self.keys, &|key| self.index.contains_key(key))
        } else {
            coarse = self
                .keys
                .iter()
                .map(|key| Key(key.0.map(|value| value >> level)))
                .collect();
            folded = coarse.iter().copied().collect();
            (&folded, &|key| coarse.contains(key))
        };
        let mut low = [u32::MAX; 3];
        let mut high = [0u32; 3];
        for key in list {
            for axis in 0..3 {
                low[axis] = low[axis].min(key.0[axis]);
                high[axis] = high[axis].max(key.0[axis]);
            }
        }
        let [joined, filled, enclosed, inside] = list
            .par_iter()
            .map(|key| {
                let cell = unpack(*key);
                // The occupied neighbours as seen along each axis: a surface
                // that lies on a voxel boundary fills two layers, and seen
                // along its normal those count once.
                let mut seen = [0u32; 3];
                let mut around = 0;
                for index in 0..27 {
                    let offset = [index % 3, index / 3 % 3, index / 9];
                    let other: [i32; 3] = std::array::from_fn(|axis| cell[axis] + offset[axis] - 1);
                    if index != 13 && pack(other).is_some_and(|key| holds(&key)) {
                        around += 1;
                        seen[0] |= 1 << (offset[1] + 3 * offset[2]);
                        seen[1] |= 1 << (offset[0] + 3 * offset[2]);
                        seen[2] |= 1 << (offset[0] + 3 * offset[1]);
                    }
                }
                // Bit 4 is the voxel itself.
                let joined = seen
                    .iter()
                    .any(|mask| (mask & !(1 << 4)).count_ones() >= DENSE_AROUND);
                let inside = (0..3).all(|axis| key.0[axis] > low[axis] && key.0[axis] < high[axis]);
                [
                    usize::from(joined),
                    usize::from(around >= FILLED_AROUND),
                    usize::from(around >= ENCLOSED_AROUND),
                    usize::from(inside),
                ]
            })
            .reduce(
                || [0; 4],
                |a, b| std::array::from_fn(|index| a[index] + b[index]),
            );
        (
            2 * joined >= list.len(),
            4 * filled >= list.len() || (inside > 0 && 8 * enclosed >= inside),
        )
    }

    /// Fold the set into voxels of twice the size.
    fn double(&mut self) {
        let keys = std::mem::take(&mut self.keys);
        let sums = std::mem::take(&mut self.sums);
        let counts = std::mem::take(&mut self.counts);
        let stations = std::mem::take(&mut self.stations);
        // Freed before the new one grows: the keys are all in `keys`.
        self.index = KeyMap::default();
        self.voxel *= 2.0;
        // Sums in the steps of the finer voxel about the coarser centre.
        let mut folded: Vec<([i64; 3], i64)> = Vec::new();
        for (point, key) in keys.iter().enumerate() {
            let cell = unpack(*key);
            let parent = pack(cell.map(|value| value >> 1)).unwrap_or_default();
            let next = self.keys.len() as u32;
            let target = *self.index.entry(parent).or_insert(next) as usize;
            if target == self.keys.len() {
                self.keys.push(parent);
                self.stations.push(NO_STATION);
                folded.push(([0; 3], 0));
            }
            let count = i64::from(counts[point]);
            for axis in 0..3 {
                // The finer centre lies half a finer voxel to one side.
                let shift = if cell[axis] & 1 == 1 {
                    STEPS / 2
                } else {
                    -STEPS / 2
                };
                folded[target].0[axis] += i64::from(sums[point][axis]) + count * shift;
            }
            folded[target].1 += count;
            self.stations[target] = self.stations[target].min(stations[point]);
        }
        for (sum, count) in folded {
            // A coarser step is two finer ones. A voxel over the limit keeps
            // its mean in a sum of the largest count.
            let kept = count.min(MAX_COUNT);
            self.sums.push(sum.map(|value| {
                let coarse = (value + 1) >> 1;
                (if kept < count {
                    coarse * kept / count
                } else {
                    coarse
                }) as i32
            }));
            self.counts.push(kept as u32);
        }
    }

    /// Put the working points in the order of their voxels, so that
    /// everything that follows depends on the set and not on the order in
    /// which the source delivered it.
    pub(crate) fn finish(&mut self) {
        let mut order: Vec<u32> = (0..self.keys.len() as u32).collect();
        order.sort_unstable_by_key(|point| self.keys[*point as usize]);
        self.keys = order
            .iter()
            .map(|point| self.keys[*point as usize])
            .collect();
        self.sums = order
            .iter()
            .map(|point| self.sums[*point as usize])
            .collect();
        self.counts = order
            .iter()
            .map(|point| self.counts[*point as usize])
            .collect();
        self.stations = order
            .iter()
            .map(|point| self.stations[*point as usize])
            .collect();
        for (point, key) in self.keys.iter().enumerate() {
            self.index.insert(*key, point as u32);
        }
        self.keys.shrink_to_fit();
        self.sums.shrink_to_fit();
        self.counts.shrink_to_fit();
        self.stations.shrink_to_fit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_shapes::{plane_with_hole, stray_points, Noise, Rng};
    use crate::Bounds;

    #[test]
    fn keys_pack_and_unpack() {
        for cell in [[0, 0, 0], [1, 2, 3], [1_073_741_823, 0, 1_073_741_823]] {
            assert_eq!(unpack(pack(cell).unwrap()), cell);
        }
        assert_eq!(pack([-1, 0, 0]), None);
        assert_eq!(pack([0, 1_073_741_824, 0]), None);
        // The order of the packed number of before: z first, x last.
        assert!(pack([9, 9, 0]) < pack([0, 0, 1]));
        assert!(pack([9, 0, 5]) < pack([0, 1, 5]));
    }

    #[test]
    fn a_voxel_keeps_the_mean_of_its_points() {
        let mut cloud = VoxelCloud::new([200_000.0, 475_000.0, 0.0], 0.03, 1_000);
        let points = [
            [200_000.301, 475_000.002, 1.501],
            [200_000.311, 475_000.012, 1.511],
            [200_000.321, 475_000.022, 1.521],
        ];
        for point in points {
            cloud.add(point, 4);
        }
        cloud.add([200_000.5, 475_000.5, 1.5], 2);
        cloud.finish();
        assert_eq!(cloud.len(), 2);
        let first = cloud.find([10, 0, 50]).unwrap();
        let mean = cloud.position(first);
        // A step is 7 micrometres; far coordinates cost nothing.
        for (value, expected) in mean.iter().zip([0.311, 0.012, 1.511]) {
            assert!((value - expected).abs() < 1e-5, "{value} {expected}");
        }
        assert_eq!(cloud.station(first), 4);
        assert_eq!(cloud.find([11, 0, 50]), None);
        assert_eq!(cloud.find([-1, 0, 50]), None);
        assert_eq!(cloud.cell_of([0.31, 0.01, 1.51]), Some([10, 0, 50]));
        assert_eq!(cloud.cell_of([-0.01, 0.0, 0.0]), None);
        assert_eq!(cloud.cell_of([f64::NAN, 0.0, 0.0]), None);
    }

    #[test]
    fn the_set_does_not_depend_on_the_order_of_the_points() {
        let shape = plane_with_hole([2.0, 1.5], 0.007, None).with_noise(Noise::Gaussian(0.003), 5);
        let build = |points: &[[f64; 3]]| {
            let mut cloud = VoxelCloud::new([-0.3, -0.3, -0.3], 0.03, 100_000);
            for point in points {
                // The station goes with the point, not with its place.
                cloud.add(*point, (point[0] * 1_000.0) as u32 % 7);
            }
            cloud.finish();
            (0..cloud.len() as u32)
                .map(|point| {
                    (
                        cloud.cell(point),
                        cloud.position(point),
                        cloud.station(point),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut shuffled = shape.points.clone();
        let mut rng = Rng::new(9);
        for index in (1..shuffled.len()).rev() {
            shuffled.swap(index, (rng.next_u64() % (index as u64 + 1)) as usize);
        }
        let plain = build(&shape.points);
        assert!(plain.len() > 3_000);
        assert_eq!(plain, build(&shuffled));
    }

    #[test]
    fn the_voxel_doubles_until_the_set_fits_its_budget() {
        // 200,000 points on a plane of 4 by 2 m: 8,900 voxels of 3 cm,
        // 2,200 of 6 cm.
        let shape = plane_with_hole([4.0, 2.0], 0.006_33, None);
        assert!(shape.points.len() > 199_000);
        let mut cloud = VoxelCloud::new([0.0, 0.0, -0.105], 0.03, 5_000);
        for point in &shape.points {
            cloud.add(*point, NO_STATION);
        }
        cloud.finish();
        assert_eq!(cloud.voxel(), 0.06);
        // Every voxel of the plane once: 67 by 34.
        assert_eq!(cloud.len(), 67 * 34);
        let mut seen = std::collections::HashSet::new();
        for point in 0..cloud.len() as u32 {
            assert!(seen.insert(cloud.cell(point)));
            assert_eq!(cloud.find(cloud.cell(point)), Some(point));
            let position = cloud.position(point);
            let cell = cloud.cell(point);
            for axis in 0..3 {
                let low = f64::from(cell[axis]) * 0.06;
                assert!(position[axis] >= low - 1e-9 && position[axis] <= low + 0.06 + 1e-9);
            }
        }
        // The mean of a voxel inside the plane is its centre: the points lie
        // evenly, and folding the finer voxels kept their sums.
        let inner = cloud.find([30, 15, 1]).unwrap();
        let mean = cloud.position(inner);
        assert!((mean[0] - 1.83).abs() < 0.004 && (mean[1] - 0.93).abs() < 0.004);
        assert!((mean[2] - 0.105).abs() < 1e-5);

        // The size reached is the same when the points come in another
        // order, and so are the voxels.
        let mut reversed = VoxelCloud::new([0.0, 0.0, -0.105], 0.03, 5_000);
        for point in shape.points.iter().rev() {
            reversed.add(*point, NO_STATION);
        }
        reversed.finish();
        assert_eq!(reversed.voxel(), 0.06);
        assert_eq!(reversed.len(), cloud.len());
        for point in 0..cloud.len() as u32 {
            assert_eq!(reversed.cell(point), cloud.cell(point));
            let (a, b) = (reversed.position(point), cloud.position(point));
            // Halving the sums rounds, so the means agree to a few steps.
            assert!((0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-4));
        }
    }

    /// The voxel size and the number of voxels after the points of a shape
    /// went in and the density was looked at.
    fn fitted(points: &[[f64; 3]]) -> (f64, u32, usize) {
        let mut cloud = VoxelCloud::new([-0.3, -0.3, -0.3], 0.03, 1_000_000);
        for point in points {
            cloud.add(*point, NO_STATION);
        }
        let doublings = cloud.fit_density();
        cloud.finish();
        (cloud.voxel(), doublings, cloud.len())
    }

    #[test]
    fn the_voxel_doubles_until_the_points_of_a_surface_fill_their_neighbours() {
        let sheet = |spacing: f64| {
            plane_with_hole([3.0, 2.0], spacing, None)
                .scattered(0.5 * spacing, 3)
                .with_noise(Noise::Gaussian(0.002), 4)
                .points
        };
        // A point per 2 cm fills voxels of 3 cm; a point per 5 cm leaves
        // most of them empty and fills those of 6 cm; a point per 9 cm
        // needs 12 cm.
        assert_eq!(fitted(&sheet(0.02)).0, 0.03);
        let (voxel, doublings, count) = fitted(&sheet(0.05));
        assert_eq!((voxel, doublings), (0.06, 1));
        // 50 by 34 voxels, in one or two layers.
        assert!((1_700..3_500).contains(&count), "{count}");
        assert_eq!(fitted(&sheet(0.09)), (0.12, 2, fitted(&sheet(0.09)).2));
        // Thinner than that is no surface: the size asked for stays.
        assert_eq!(fitted(&sheet(0.30)).0, 0.03);
        // The size reached does not depend on the order of the points.
        let mut reversed = sheet(0.05);
        reversed.reverse();
        assert_eq!(fitted(&reversed), fitted(&sheet(0.05)));
    }

    #[test]
    fn points_that_fill_space_keep_their_voxels() {
        // Stray points make no surface. In larger voxels their means would
        // lie on a lattice, in which planes are found everywhere.
        let room = Bounds {
            min: [0.0; 3],
            max: [4.0, 3.0, 2.6],
        };
        for count in [1_000, 5_000, 20_000, 100_000] {
            let stray = stray_points(room, count, 7);
            assert_eq!(fitted(&stray.points).0, 0.03, "{count}");
        }
        // Also in a space so small that most of its voxels are on its
        // outside.
        let small = Bounds {
            min: [0.0; 3],
            max: [1.4, 1.05, 0.9],
        };
        for count in [1_000, 1_900, 4_000] {
            let stray = stray_points(small, count, 8);
            assert_eq!(fitted(&stray.points).0, 0.03, "{count}");
        }
    }

    #[test]
    fn a_wide_extent_starts_with_larger_voxels() {
        let mut cloud = VoxelCloud::new([0.0; 3], 0.03, 1_000);
        // 32,212 km fit at 3 cm: a country does, with one far point too.
        cloud.fit_extent(500_000.0);
        assert_eq!(cloud.voxel(), 0.03);
        cloud.fit_extent(40_000_000.0);
        assert_eq!(cloud.voxel(), 0.06);
        cloud.add([39_999_999.99, 0.0, 0.0], NO_STATION);
        cloud.finish();
        assert_eq!(cloud.len(), 1);
        assert!((cloud.position(0)[0] - 39_999_999.99).abs() < 1e-4);
    }
}
