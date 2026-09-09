//! A/B benchmark: the existing src-tauri octree against the new one.
//!
//! The baseline below is the current `src-tauri/src/pointcloud/octree.rs`
//! algorithm reproduced as faithfully as it can be without its Tauri types:
//! an array-of-structs point vector, recursive one-point-at-a-time insertion
//! that redistributes a leaf when it overflows, and `i % 8` LOD subsampling.
//! Both sides index the same generated points.
//!
//! run: cargo run --release --example bench -- [n_points]

use std::time::Instant;

use pointcloud_core::octree::Octree;
use pointcloud_core::types::{Bounds, PointCloud};

// ------------------------------------------------------- baseline (old) ----

#[derive(Clone)]
struct PointRecord {
    x: f64,
    y: f64,
    z: f64,
    r: u8,
    g: u8,
    b: u8,
    intensity: u16,
    classification: u8,
}

struct OldNode {
    bounds: Bounds,
    level: u8,
    points: Vec<PointRecord>,
    children: [Option<Box<OldNode>>; 8],
}

const OLD_MAX_LEAF: usize = 65_536;
const OLD_MAX_DEPTH: u8 = 12;
const OLD_SUBSAMPLE: usize = 8;

impl OldNode {
    fn new(bounds: Bounds, level: u8) -> Self {
        Self {
            bounds,
            level,
            points: Vec::new(),
            children: Default::default(),
        }
    }
    fn is_leaf(&self) -> bool {
        self.children.iter().all(|c| c.is_none())
    }
    fn has_children(&self) -> bool {
        self.children.iter().any(|c| c.is_some())
    }
}

fn old_octant(b: &Bounds, x: f64, y: f64, z: f64) -> u8 {
    let c = b.center();
    let mut o = 0u8;
    if x >= c[0] { o |= 1; }
    if y >= c[1] { o |= 2; }
    if z >= c[2] { o |= 4; }
    o
}

fn old_octant_bounds(b: &Bounds, oct: u8) -> Bounds {
    let c = b.center();
    let (x0, x1) = if oct & 1 == 0 { (b.min[0], c[0]) } else { (c[0], b.max[0]) };
    let (y0, y1) = if oct & 2 == 0 { (b.min[1], c[1]) } else { (c[1], b.max[1]) };
    let (z0, z1) = if oct & 4 == 0 { (b.min[2], c[2]) } else { (c[2], b.max[2]) };
    Bounds { min: [x0, y0, z0], max: [x1, y1, z1] }
}

fn old_insert(node: &mut OldNode, p: PointRecord, count: &mut u32) {
    if node.is_leaf() && node.points.len() < OLD_MAX_LEAF {
        node.points.push(p);
        return;
    }
    if node.level >= OLD_MAX_DEPTH {
        node.points.push(p);
        return;
    }
    if node.is_leaf() && !node.points.is_empty() {
        let existing: Vec<PointRecord> = node.points.drain(..).collect();
        for e in existing {
            let oct = old_octant(&node.bounds, e.x, e.y, e.z);
            let child = old_ensure_child(node, oct, count);
            old_insert(child, e, count);
        }
    }
    let oct = old_octant(&node.bounds, p.x, p.y, p.z);
    let child = old_ensure_child(node, oct, count);
    old_insert(child, p, count);
}

fn old_ensure_child<'a>(node: &'a mut OldNode, oct: u8, count: &mut u32) -> &'a mut OldNode {
    if node.children[oct as usize].is_none() {
        let b = old_octant_bounds(&node.bounds, oct);
        *count += 1;
        node.children[oct as usize] = Some(Box::new(OldNode::new(b, node.level + 1)));
    }
    node.children[oct as usize].as_mut().unwrap()
}

fn old_build_lod(node: &mut OldNode) {
    for c in node.children.iter_mut().flatten() {
        old_build_lod(c);
    }
    if node.has_children() && node.points.is_empty() {
        let mut sample = Vec::new();
        for c in node.children.iter().flatten() {
            for (i, p) in c.points.iter().enumerate() {
                if i % OLD_SUBSAMPLE == 0 {
                    sample.push(p.clone());
                }
            }
        }
        node.points = sample;
    }
}

fn old_count_nodes(node: &OldNode) -> usize {
    1 + node.children.iter().flatten().map(|c| old_count_nodes(c)).sum::<usize>()
}

fn old_lod_points(node: &OldNode) -> usize {
    let own = if node.has_children() { node.points.len() } else { 0 };
    own + node.children.iter().flatten().map(|c| old_lod_points(c)).sum::<usize>()
}

// ------------------------------------------------------------------ data ----

/// A building-like scene: most points on a ground surface, some on walls.
/// Uniform noise would flatter the Morton build; real scans are clustered.
fn generate(n: usize) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut seed: u64 = 0x2545F4914F6CDD1D;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f64 / (1u64 << 53) as f64
    };

    let (mut xs, mut ys, mut zs) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    for i in 0..n {
        if i % 10 < 7 {
            let x = rnd() * 400.0 - 200.0;
            let y = rnd() * 400.0 - 200.0;
            let z = 3.0 * (x * 0.02).sin() * (y * 0.02).cos() + rnd() * 0.1;
            xs.push(x); ys.push(y); zs.push(z);
        } else {
            let bx = if i % 2 == 0 { -60.0 } else { 70.0 };
            let by = if i % 2 == 0 { -40.0 } else { 50.0 };
            xs.push(bx + rnd() * 50.0 - 25.0);
            ys.push(by + rnd() * 40.0 - 20.0);
            zs.push(rnd() * 15.0);
        }
    }
    (xs, ys, zs)
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5_000_000);

    println!("generating {n} points...");
    let (xs, ys, zs) = generate(n);

    // ---- new ----
    let t = Instant::now();
    let cloud = PointCloud::from_world(&xs, &ys, &zs, vec![], vec![], vec![], [0.001; 3]);
    let quantise_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let tree = Octree::build(&cloud);
    let new_ms = t.elapsed().as_secs_f64() * 1000.0;

    let new_bytes = n * PointCloud::BYTES_PER_POINT      // point storage
        + n * (8 + 4)                                    // morton key + index during build
        + tree.lod_points() * 4;                         // lod index lists

    println!("\nnew  (morton, struct-of-arrays, rayon)");
    println!("  quantise      {quantise_ms:8.1} ms");
    println!("  build         {new_ms:8.1} ms");
    println!("  nodes         {:8}", tree.node_count());
    println!("  depth         {:8}", tree.depth());
    println!("  lod points    {:8}", tree.lod_points());
    println!("  ~memory       {:8.1} MB", new_bytes as f64 / 1e6);

    // ---- old ----
    let t = Instant::now();
    let recs: Vec<PointRecord> = (0..n)
        .map(|i| PointRecord {
            x: xs[i], y: ys[i], z: zs[i],
            r: 0, g: 0, b: 0, intensity: 0, classification: 0,
        })
        .collect();
    let materialise_ms = t.elapsed().as_secs_f64() * 1000.0;

    let mut bounds = Bounds::empty();
    for i in 0..n {
        bounds.expand([xs[i], ys[i], zs[i]]);
    }

    let t = Instant::now();
    let mut root = OldNode::new(bounds, 0);
    let mut node_count = 1u32;
    for r in recs {
        old_insert(&mut root, r, &mut node_count);
    }
    old_build_lod(&mut root);
    let old_ms = t.elapsed().as_secs_f64() * 1000.0;

    let old_nodes = old_count_nodes(&root);
    let old_lod = old_lod_points(&root);
    // 3xf64 + 3xu8 + u16 + u8 = 30 bytes, padded to 8-byte alignment = 32.
    let old_bytes = n * 32 + old_lod * 32;

    println!("\nold  (array-of-structs, per-point insert, i%8 lod)");
    println!("  materialise   {materialise_ms:8.1} ms");
    println!("  build         {old_ms:8.1} ms");
    println!("  nodes         {old_nodes:8}");
    println!("  lod points    {old_lod:8}");
    println!("  ~memory       {:8.1} MB", old_bytes as f64 / 1e6);

    println!("\nbuild speedup   {:8.1}x", old_ms / new_ms);
    println!("memory ratio    {:8.2}x", old_bytes as f64 / new_bytes as f64);
}
