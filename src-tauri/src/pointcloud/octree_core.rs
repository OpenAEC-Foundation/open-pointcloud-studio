//! Adapter putting `pointcloud-core` behind the octree API the manager and
//! the IPC commands already use.
//!
//! The wire protocol does not change: node ids stay opaque strings, chunks
//! keep the same fields, so the frontend needs no edits. What changes is what
//! sits underneath — a Morton-ordered, rayon-parallel tree with real frustum
//! culling instead of the per-point-insert tree with distance-only selection.

use pointcloud_core::frustum::{select_visible, Camera};
use pointcloud_core::octree::Octree as CoreTree;
use pointcloud_core::types::{Bounds as CoreBounds, PointCloud};

use super::types::{BoundingBox3D, CameraState, OctreeNodeInfo, PointChunk, PointRecord};

/// Positions are quantised to this many metres. LAS itself commonly uses 1 mm.
const QUANTISATION: [f64; 3] = [0.001, 0.001, 0.001];

/// Nodes smaller than this on screen are not worth loading.
const MIN_SCREEN_SIZE: f64 = 1.0;

pub struct Octree {
    cloud: PointCloud,
    tree: CoreTree,
}

fn from_core_bounds(b: &CoreBounds) -> BoundingBox3D {
    BoundingBox3D {
        min_x: b.min[0], min_y: b.min[1], min_z: b.min[2],
        max_x: b.max[0], max_y: b.max[1], max_z: b.max[2],
    }
}

/// Node ids are arena indices. The frontend treats them as opaque keys, so
/// they only need to be stable and cheap to resolve — the old path-style ids
/// cost a recursive string-prefix search on every single node fetch.
fn node_id(index: u32) -> String {
    format!("n{index}")
}

fn parse_node_id(id: &str) -> Option<u32> {
    id.strip_prefix('n')?.parse().ok()
}

impl Octree {
    pub fn build(points: Vec<PointRecord>, _bounds: BoundingBox3D) -> Self {
        let n = points.len();
        let mut xs = Vec::with_capacity(n);
        let mut ys = Vec::with_capacity(n);
        let mut zs = Vec::with_capacity(n);
        let mut rgb = Vec::with_capacity(n);
        let mut intensity = Vec::with_capacity(n);
        let mut classification = Vec::with_capacity(n);

        for p in &points {
            xs.push(p.x);
            ys.push(p.y);
            zs.push(p.z);
            rgb.push([p.r, p.g, p.b]);
            intensity.push(p.intensity);
            classification.push(p.classification);
        }
        drop(points);

        let cloud = PointCloud::from_world(
            &xs, &ys, &zs, rgb, intensity, classification, QUANTISATION,
        );
        drop(xs);
        drop(ys);
        drop(zs);

        let tree = CoreTree::build(&cloud);
        Self { cloud, tree }
    }

    pub fn get_node_info(&self, id: &str) -> Option<OctreeNodeInfo> {
        let idx = parse_node_id(id)?;
        let node = self.tree.nodes.get(idx as usize)?;
        Some(OctreeNodeInfo {
            node_id: id.to_string(),
            bounds: from_core_bounds(&node.bounds),
            level: node.level,
            point_count: node.render_count(),
            has_children: !node.is_leaf(),
        })
    }

    /// Select nodes for the current view.
    ///
    /// `CameraState` carries no up vector or clip planes, so they are derived:
    /// the scene is Z-up (LAS convention), and the far plane is sized from the
    /// root extent so nothing in the dataset is ever clipped away by it.
    pub fn get_visible_nodes(&self, camera: &CameraState, point_budget: u32) -> Vec<String> {
        if self.tree.nodes.is_empty() {
            return Vec::new();
        }

        let root = &self.tree.root_bounds;
        let centre = root.center();
        let extent = root.max_extent().max(1.0);
        let dx = centre[0] - camera.position[0];
        let dy = centre[1] - camera.position[1];
        let dz = centre[2] - camera.position[2];
        let dist_to_scene = (dx * dx + dy * dy + dz * dz).sqrt();

        // A camera aimed straight up or down would make the cross product with
        // a Z up-vector degenerate.
        let fx = camera.target[0] - camera.position[0];
        let fy = camera.target[1] - camera.position[1];
        let fz = camera.target[2] - camera.position[2];
        let flen = (fx * fx + fy * fy + fz * fz).sqrt().max(f64::MIN_POSITIVE);
        let up = if (fz / flen).abs() > 0.999 {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };

        let cam = Camera {
            position: camera.position,
            target: camera.target,
            up,
            fov: camera.fov,
            aspect: if camera.aspect > 0.0 { camera.aspect } else { 1.0 },
            near: (extent * 1e-4).max(1e-3),
            far: dist_to_scene + extent * 4.0,
            screen_height: camera.screen_height,
        };

        let (visible, _stats) =
            select_visible(&self.tree, &cam, point_budget as u64, MIN_SCREEN_SIZE);

        visible.into_iter().map(|v| node_id(v.index)).collect()
    }

    /// Pack a node's points for GPU upload, positioned relative to the node
    /// centre so f32 coordinates keep their precision.
    pub fn get_node_chunk(&self, id: &str) -> Option<PointChunk> {
        let idx = parse_node_id(id)?;
        let node = self.tree.nodes.get(idx as usize)?;

        // Internal nodes render their LOD sample; leaves render everything.
        let indices: Vec<u32> = if node.is_leaf() {
            self.tree.order[node.start as usize..node.end as usize].to_vec()
        } else {
            node.lod.clone()
        };
        if indices.is_empty() {
            return None;
        }

        let centre = node.bounds.center();
        let count = indices.len();

        let mut positions = Vec::with_capacity(count * 3);
        let mut colors = Vec::with_capacity(count * 3);
        let mut intensities = Vec::with_capacity(count);
        let mut classifications = Vec::with_capacity(count);

        let has_color = self.cloud.has_color;
        let has_intensity = self.cloud.has_intensity;
        let has_class = self.cloud.has_classification;

        for &i in &indices {
            let i = i as usize;
            let p = self.cloud.world(i);
            positions.push((p[0] - centre[0]) as f32);
            positions.push((p[1] - centre[1]) as f32);
            positions.push((p[2] - centre[2]) as f32);

            if has_color {
                let c = self.cloud.rgb[i];
                colors.extend_from_slice(&c);
            } else {
                colors.extend_from_slice(&[255, 255, 255]);
            }
            intensities.push(if has_intensity { self.cloud.intensity[i] } else { 0 });
            classifications.push(if has_class { self.cloud.classification[i] } else { 0 });
        }

        // Spacing from the surface footprint: LiDAR points sit on surfaces, so
        // the two largest box dimensions estimate the area they cover.
        let s = node.bounds.size();
        let mut dims = [s[0], s[1], s[2]];
        dims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let spacing = ((dims[0] * dims[1]) / count as f64).sqrt() as f32;

        Some(PointChunk {
            node_id: id.to_string(),
            center: centre,
            level: node.level,
            spacing,
            positions,
            colors,
            intensities,
            classifications,
            point_count: count as u32,
        })
    }
}
