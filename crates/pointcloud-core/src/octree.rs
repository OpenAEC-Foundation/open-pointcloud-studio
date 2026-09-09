//! Morton-order octree.
//!
//! The tree is built by sorting point indices along a Z-order (Morton) curve
//! and then treating nodes as contiguous ranges of that sorted array. Three
//! things follow from that, and they are the reason for this rewrite:
//!
//!  * Points are never moved or copied into per-node vectors. A node is a
//!    `(start, end)` pair, so peak memory is the cloud plus one u32 index and
//!    one u64 key per point — not two copies of the cloud.
//!  * Both expensive phases (key computation, sort) are data-parallel, so
//!    `rayon` can use every core. The previous insert-one-point-at-a-time
//!    build could not.
//!  * Subdividing a node is a partition of an already-sorted range, which is
//!    eight binary searches rather than a re-insertion of every point.
//!
//! Level-of-detail samples are taken on a spatial grid rather than by keeping
//! every Nth point. Because the range is Morton-sorted, "one point per grid
//! cell" is just "first point of each distinct deeper Morton prefix", found in
//! a single linear scan. Stride sampling instead inherits whatever ordering
//! the file had, which clumps.

use crate::types::{Bounds, PointCloud};

#[cfg(feature = "parallel")]
use rayon::prelude::*;

/// Morton keys use 21 bits per axis, the most that fits three ways in a u64.
const MORTON_BITS: u32 = 21;
const MORTON_MAX: u64 = (1 << MORTON_BITS) - 1;

/// A node holding at most this many points is not subdivided.
const MAX_LEAF_POINTS: usize = 32_768;

/// Hard cap on depth. At 21 Morton bits per axis the keys run out beyond this.
const MAX_DEPTH: u8 = MORTON_BITS as u8;

/// LOD grid resolution per node, as an octree-level offset: 5 → 32x32x32,
/// so an internal node contributes at most 32_768 sample points.
const LOD_LEVELS: u32 = 5;

/// Interleave the low 21 bits of `v` with two zero bits between each.
#[inline]
fn split3(v: u64) -> u64 {
    let mut x = v & MORTON_MAX;
    x = (x | (x << 32)) & 0x1f00000000ffff;
    x = (x | (x << 16)) & 0x1f0000ff0000ff;
    x = (x | (x << 8)) & 0x100f00f00f00f00f;
    x = (x | (x << 4)) & 0x10c30c30c30c30c3;
    x = (x | (x << 2)) & 0x1249249249249249;
    x
}

#[inline]
fn morton3(x: u32, y: u32, z: u32) -> u64 {
    split3(x as u64) | (split3(y as u64) << 1) | (split3(z as u64) << 2)
}

/// One octree node. Children are arena indices; `u32::MAX` means absent.
#[derive(Debug, Clone)]
pub struct Node {
    pub bounds: Bounds,
    pub level: u8,
    /// Range into the Morton-sorted index array covering this node's points.
    pub start: u32,
    pub end: u32,
    pub children: [u32; 8],
    /// Indices (into the cloud) chosen to represent this node at its own LOD.
    pub lod: Vec<u32>,
}

impl Node {
    pub fn is_leaf(&self) -> bool {
        self.children.iter().all(|&c| c == u32::MAX)
    }

    pub fn point_count(&self) -> u32 {
        self.end - self.start
    }

    /// Points actually drawn for this node: its LOD sample, or all of them
    /// when it is a leaf.
    pub fn render_count(&self) -> u32 {
        if self.is_leaf() {
            self.point_count()
        } else {
            self.lod.len() as u32
        }
    }
}

pub struct Octree {
    pub nodes: Vec<Node>,
    /// Point indices sorted by Morton key. Node ranges index into this.
    pub order: Vec<u32>,
    pub root_bounds: Bounds,
    pub total_points: usize,
}

impl Octree {
    pub fn build(cloud: &PointCloud) -> Self {
        let n = cloud.len();
        let cube = cloud.bounds.to_cube();

        if n == 0 {
            return Self {
                nodes: Vec::new(),
                order: Vec::new(),
                root_bounds: cube,
                total_points: 0,
            };
        }

        // --- 1. Morton key per point (parallel) --------------------------
        let size = cube.max_extent().max(f64::MIN_POSITIVE);
        let inv = MORTON_MAX as f64 / size;
        let min = cube.min;

        let key_of = |i: usize| -> u64 {
            let p = cloud.world(i);
            let gx = (((p[0] - min[0]) * inv) as i64).clamp(0, MORTON_MAX as i64) as u32;
            let gy = (((p[1] - min[1]) * inv) as i64).clamp(0, MORTON_MAX as i64) as u32;
            let gz = (((p[2] - min[2]) * inv) as i64).clamp(0, MORTON_MAX as i64) as u32;
            morton3(gx, gy, gz)
        };

        #[cfg(feature = "parallel")]
        let mut keyed: Vec<(u64, u32)> =
            (0..n).into_par_iter().map(|i| (key_of(i), i as u32)).collect();
        #[cfg(not(feature = "parallel"))]
        let mut keyed: Vec<(u64, u32)> = (0..n).map(|i| (key_of(i), i as u32)).collect();

        // --- 2. Sort along the curve (parallel) --------------------------
        #[cfg(feature = "parallel")]
        keyed.par_sort_unstable_by_key(|&(k, _)| k);
        #[cfg(not(feature = "parallel"))]
        keyed.sort_unstable_by_key(|&(k, _)| k);

        let keys: Vec<u64> = keyed.iter().map(|&(k, _)| k).collect();
        let order: Vec<u32> = keyed.into_iter().map(|(_, i)| i).collect();

        // --- 3. Node table from ranges of the sorted array ---------------
        let mut tree = Self {
            nodes: Vec::new(),
            order,
            root_bounds: cube,
            total_points: n,
        };
        tree.nodes.push(Node {
            bounds: cube,
            level: 0,
            start: 0,
            end: n as u32,
            children: [u32::MAX; 8],
            lod: Vec::new(),
        });
        tree.subdivide(0, &keys);

        // --- 4. LOD samples for internal nodes ---------------------------
        tree.build_lod(&keys);
        tree
    }

    /// Split a node's range into its eight octants.
    ///
    /// The range is sorted by Morton key, and the three bits selecting the
    /// octant at `level` sit at a known position in that key, so the split
    /// points are found by binary search instead of by re-testing points.
    fn subdivide(&mut self, node_idx: usize, keys: &[u64]) {
        let (start, end, level, bounds) = {
            let nd = &self.nodes[node_idx];
            (nd.start, nd.end, nd.level, nd.bounds)
        };

        if (end - start) as usize <= MAX_LEAF_POINTS || level >= MAX_DEPTH {
            return;
        }

        // Bits for this level occupy positions [shift, shift+3).
        let shift = 3 * (MORTON_BITS - 1 - level as u32);
        let octant_of = |k: u64| ((k >> shift) & 0b111) as u8;

        // Eight boundaries via binary search over the sorted sub-range.
        let mut cut = [start; 9];
        cut[8] = end;
        for oct in 1..8u8 {
            let lo = cut[oct as usize - 1] as usize;
            let hi = end as usize;
            let slice = &keys[lo..hi];
            let idx = slice.partition_point(|&k| octant_of(k) < oct);
            cut[oct as usize] = (lo + idx) as u32;
        }

        let mut children = [u32::MAX; 8];
        for oct in 0..8usize {
            let (cs, ce) = (cut[oct], cut[oct + 1]);
            if ce <= cs {
                continue;
            }
            let child_idx = self.nodes.len() as u32;
            children[oct] = child_idx;
            self.nodes.push(Node {
                bounds: octant_bounds(&bounds, oct as u8),
                level: level + 1,
                start: cs,
                end: ce,
                children: [u32::MAX; 8],
                lod: Vec::new(),
            });
        }

        self.nodes[node_idx].children = children;

        for oct in 0..8usize {
            let c = children[oct];
            if c != u32::MAX {
                self.subdivide(c as usize, keys);
            }
        }
    }

    /// Pick one point per grid cell for every internal node.
    fn build_lod(&mut self, keys: &[u64]) {
        for idx in 0..self.nodes.len() {
            if self.nodes[idx].is_leaf() {
                continue;
            }
            let (start, end, level) = {
                let nd = &self.nodes[idx];
                (nd.start as usize, nd.end as usize, nd.level as u32)
            };

            // Keep the key prefix down to LOD_LEVELS below this node; each
            // distinct prefix is one cell of a 2^LOD_LEVELS grid per axis.
            let depth = (level + LOD_LEVELS).min(MORTON_BITS - 1);
            let shift = 3 * (MORTON_BITS - 1 - depth);

            let mut lod: Vec<u32> = Vec::new();
            let mut last: Option<u64> = None;
            for i in start..end {
                let cell = keys[i] >> shift;
                if last != Some(cell) {
                    lod.push(self.order[i]);
                    last = Some(cell);
                }
            }
            self.nodes[idx].lod = lod;
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Deepest level present in the tree.
    pub fn depth(&self) -> u8 {
        self.nodes.iter().map(|n| n.level).max().unwrap_or(0)
    }

    /// Total points retained across all LOD samples.
    pub fn lod_points(&self) -> usize {
        self.nodes.iter().map(|n| n.lod.len()).sum()
    }
}

fn octant_bounds(b: &Bounds, oct: u8) -> Bounds {
    let c = b.center();
    let pick = |axis: usize, bit: u8| {
        if oct & bit == 0 {
            (b.min[axis], c[axis])
        } else {
            (c[axis], b.max[axis])
        }
    };
    let (x0, x1) = pick(0, 1);
    let (y0, y1) = pick(1, 2);
    let (z0, z1) = pick(2, 4);
    Bounds {
        min: [x0, y0, z0],
        max: [x1, y1, z1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_cloud(n_side: i32) -> PointCloud {
        let (mut xs, mut ys, mut zs) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..n_side {
            for j in 0..n_side {
                for k in 0..n_side {
                    xs.push(i as f64 * 0.1);
                    ys.push(j as f64 * 0.1);
                    zs.push(k as f64 * 0.1);
                }
            }
        }
        PointCloud::from_world(&xs, &ys, &zs, vec![], vec![], vec![], [0.001; 3])
    }

    #[test]
    fn morton_roundtrip_orders_by_locality() {
        // Neighbouring cells must produce nearby keys along one axis.
        assert!(morton3(0, 0, 0) < morton3(1, 0, 0));
        assert!(morton3(1, 0, 0) < morton3(0, 1, 0));
        assert_eq!(morton3(0, 0, 0), 0);
    }

    #[test]
    fn every_point_lands_in_exactly_one_leaf() {
        let cloud = grid_cloud(40); // 64_000 points
        let tree = Octree::build(&cloud);

        let mut seen = vec![false; cloud.len()];
        for node in &tree.nodes {
            if !node.is_leaf() {
                continue;
            }
            for i in node.start..node.end {
                let p = tree.order[i as usize] as usize;
                assert!(!seen[p], "point {p} appeared in two leaves");
                seen[p] = true;
            }
        }
        assert!(seen.iter().all(|&s| s), "some points reached no leaf");
    }

    #[test]
    fn child_ranges_partition_the_parent() {
        let cloud = grid_cloud(40);
        let tree = Octree::build(&cloud);
        for node in &tree.nodes {
            if node.is_leaf() {
                continue;
            }
            let sum: u32 = node
                .children
                .iter()
                .filter(|&&c| c != u32::MAX)
                .map(|&c| tree.nodes[c as usize].point_count())
                .sum();
            assert_eq!(sum, node.point_count(), "children lost or duplicated points");
        }
    }

    #[test]
    fn points_lie_within_their_node_bounds() {
        let cloud = grid_cloud(24);
        let tree = Octree::build(&cloud);
        let eps = 1e-6;
        for node in &tree.nodes {
            for i in node.start..node.end {
                let p = cloud.world(tree.order[i as usize] as usize);
                for a in 0..3 {
                    assert!(
                        p[a] >= node.bounds.min[a] - eps && p[a] <= node.bounds.max[a] + eps,
                        "point outside node bounds on axis {a}"
                    );
                }
            }
        }
    }

    #[test]
    fn lod_is_a_subset_and_smaller_than_the_node() {
        let cloud = grid_cloud(40);
        let tree = Octree::build(&cloud);
        for node in &tree.nodes {
            if node.is_leaf() {
                continue;
            }
            assert!(!node.lod.is_empty(), "internal node has no LOD sample");
            assert!(node.lod.len() as u32 <= node.point_count());
        }
    }

    #[test]
    fn empty_cloud_is_handled() {
        let cloud = PointCloud::from_world(&[], &[], &[], vec![], vec![], vec![], [0.001; 3]);
        let tree = Octree::build(&cloud);
        assert_eq!(tree.node_count(), 0);
        assert_eq!(tree.total_points, 0);
    }
}
