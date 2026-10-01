use super::types::{BoundingBox3D, CameraState, OctreeNodeInfo, PointChunk, PointRecord};

const MAX_POINTS_PER_LEAF: usize = 65_536;
const MAX_DEPTH: u8 = 12;
const SUBSAMPLE_RATIO: usize = 8; // Keep every Nth point for parent LOD

/// Internal octree node storing point data
pub struct OctreeNode {
    pub node_id: String,
    pub bounds: BoundingBox3D,
    pub level: u8,
    pub points: Vec<PointRecord>,
    pub children: [Option<Box<OctreeNode>>; 8],
}

impl OctreeNode {
    fn new(node_id: String, bounds: BoundingBox3D, level: u8) -> Self {
        Self {
            node_id,
            bounds,
            level,
            points: Vec::new(),
            children: [None, None, None, None, None, None, None, None],
        }
    }

    fn is_leaf(&self) -> bool {
        self.children.iter().all(|c| c.is_none())
    }

    fn point_count(&self) -> u32 {
        self.points.len() as u32
    }

    fn has_children(&self) -> bool {
        self.children.iter().any(|c| c.is_some())
    }
}

/// The octree spatial index
pub struct Octree {
    pub root: OctreeNode,
    pub total_points: u64,
    node_count: u32,
}

impl Octree {
    /// Build an octree from a set of points
    pub fn build(points: Vec<PointRecord>, bounds: BoundingBox3D) -> Self {
        let total_points = points.len() as u64;
        let mut tree = Self {
            root: OctreeNode::new("r".to_string(), bounds, 0),
            total_points,
            node_count: 1,
        };

        for point in points {
            tree.insert_point(point);
        }

        // Build LOD subsamples for internal nodes
        Self::build_lod(&mut tree.root as *mut OctreeNode);

        tree
    }

    /// Insert a single point into the octree
    fn insert_point(&mut self, point: PointRecord) {
        Self::insert_into_node(&mut self.root, point, &mut self.node_count);
    }

    fn insert_into_node(node: &mut OctreeNode, point: PointRecord, node_count: &mut u32) {
        // If leaf and under capacity, just add
        if node.is_leaf() && node.points.len() < MAX_POINTS_PER_LEAF {
            node.points.push(point);
            return;
        }

        // If leaf at max depth, just add (can't split further)
        if node.level >= MAX_DEPTH {
            node.points.push(point);
            return;
        }

        // Need to split: if this is a leaf with points, redistribute
        if node.is_leaf() && !node.points.is_empty() {
            let existing_points: Vec<PointRecord> = node.points.drain(..).collect();
            for p in existing_points {
                let octant = Self::get_octant(&node.bounds, p.x, p.y, p.z);
                let child = Self::ensure_child(node, octant, node_count);
                Self::insert_into_node(child, p, node_count);
            }
        }

        // Insert new point into appropriate child
        let octant = Self::get_octant(&node.bounds, point.x, point.y, point.z);
        let child = Self::ensure_child(node, octant, node_count);
        Self::insert_into_node(child, point, node_count);
    }

    fn get_octant(bounds: &BoundingBox3D, x: f64, y: f64, z: f64) -> u8 {
        let c = bounds.center();
        let mut octant = 0u8;
        if x >= c[0] { octant |= 1; }
        if y >= c[1] { octant |= 2; }
        if z >= c[2] { octant |= 4; }
        octant
    }

    fn ensure_child<'a>(node: &'a mut OctreeNode, octant: u8, node_count: &mut u32) -> &'a mut OctreeNode {
        if node.children[octant as usize].is_none() {
            let child_bounds = node.bounds.octant(octant);
            let child_id = format!("{}{}", node.node_id, octant);
            *node_count += 1;
            node.children[octant as usize] = Some(Box::new(OctreeNode::new(
                child_id,
                child_bounds,
                node.level + 1,
            )));
        }
        node.children[octant as usize].as_mut().unwrap()
    }

    /// Build LOD: each parent gets a subsample of its children's points
    fn build_lod(node_ptr: *mut OctreeNode) {
        let node = unsafe { &mut *node_ptr };

        // Recurse into children first
        for child in &mut node.children {
            if let Some(ref mut c) = child {
                Self::build_lod(&mut **c as *mut OctreeNode);
            }
        }

        // For internal nodes, subsample from children
        if node.has_children() && node.points.is_empty() {
            let mut subsample = Vec::new();
            for child in &node.children {
                if let Some(ref c) = child {
                    for (i, p) in c.points.iter().enumerate() {
                        if i % SUBSAMPLE_RATIO == 0 {
                            subsample.push(p.clone());
                        }
                    }
                }
            }
            node.points = subsample;
        }
    }

    /// Get info about a node by ID
    pub fn get_node_info(&self, node_id: &str) -> Option<OctreeNodeInfo> {
        self.find_node(&self.root, node_id).map(|n| OctreeNodeInfo {
            node_id: n.node_id.clone(),
            bounds: n.bounds.clone(),
            level: n.level,
            point_count: n.point_count(),
            has_children: n.has_children(),
        })
    }

    fn find_node<'a>(&self, node: &'a OctreeNode, node_id: &str) -> Option<&'a OctreeNode> {
        if node.node_id == node_id {
            return Some(node);
        }
        for child in &node.children {
            if let Some(ref c) = child {
                if node_id.starts_with(&c.node_id) {
                    if let Some(found) = self.find_node(c, node_id) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    /// Get point data for a node, packed for GPU transfer
    pub fn get_node_chunk(&self, node_id: &str) -> Option<PointChunk> {
        let node = self.find_node(&self.root, node_id)?;
        if node.points.is_empty() {
            return None;
        }

        let center = node.bounds.center();
        let count = node.points.len();

        let mut positions = Vec::with_capacity(count * 3);
        let mut colors = Vec::with_capacity(count * 3);
        let mut intensities = Vec::with_capacity(count);
        let mut classifications = Vec::with_capacity(count);

        for p in &node.points {
            // Store positions relative to chunk center for double-precision workaround
            positions.push((p.x - center[0]) as f32);
            positions.push((p.y - center[1]) as f32);
            positions.push((p.z - center[2]) as f32);
            colors.push(p.r);
            colors.push(p.g);
            colors.push(p.b);
            intensities.push(p.intensity);
            classifications.push(p.classification);
        }

        // Compute per-node spacing from the 2D surface footprint.
        // LiDAR points lie on surfaces, so use the two largest bbox dimensions
        // to estimate the area, then derive spacing as sqrt(area / pointCount).
        let s = node.bounds.size();
        let mut dims = [s[0], s[1], s[2]];
        dims.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let surface_area = dims[0] * dims[1]; // two largest dimensions
        let spacing = (surface_area / count as f64).sqrt() as f32;

        Some(PointChunk {
            node_id: node_id.to_string(),
            center,
            level: node.level,
            spacing,
            positions,
            colors,
            intensities,
            classifications,
            point_count: count as u32,
        })
    }

    /// Select visible nodes based on camera state and point budget.
    /// Returns node IDs sorted by priority (closest/largest screen-space first).
    pub fn get_visible_nodes(&self, camera: &CameraState, point_budget: u32) -> Vec<String> {
        let mut candidates: Vec<(String, f64, u32)> = Vec::new(); // (id, priority, point_count)
        self.collect_visible(&self.root, camera, &mut candidates);

        // Sort by priority (lower = more important = should render first)
        candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // Accumulate until budget is reached
        let mut result = Vec::new();
        let mut total = 0u32;
        for (id, _priority, count) in candidates {
            if total + count > point_budget && !result.is_empty() {
                break;
            }
            total += count;
            result.push(id);
        }

        result
    }

    fn collect_visible(
        &self,
        node: &OctreeNode,
        camera: &CameraState,
        candidates: &mut Vec<(String, f64, u32)>,
    ) {
        if node.points.is_empty() && !node.has_children() {
            return;
        }

        let node_center = node.bounds.center();
        let dx = node_center[0] - camera.position[0];
        let dy = node_center[1] - camera.position[1];
        let dz = node_center[2] - camera.position[2];
        let distance = (dx * dx + dy * dy + dz * dz).sqrt();

        let node_size = node.bounds.max_extent();

        // Screen-space error: how many pixels would this node's extent cover?
        let screen_size = if distance > 0.001 {
            (node_size / distance) * camera.screen_height / (2.0 * (camera.fov.to_radians() / 2.0).tan())
        } else {
            f64::MAX
        };

        // If the node is too small on screen, skip it and its children
        if screen_size < 1.0 {
            return;
        }

        // If leaf or screen-space error is small enough, use this node
        let should_use_node = node.is_leaf() || screen_size < 200.0;

        if should_use_node && !node.points.is_empty() {
            // Priority: distance / node_size (smaller = more important)
            let priority = distance / node_size.max(0.001);
            candidates.push((node.node_id.clone(), priority, node.point_count()));
        }

        // Recurse into children for higher detail
        if !should_use_node || !node.is_leaf() {
            for child in &node.children {
                if let Some(ref c) = child {
                    self.collect_visible(c, camera, candidates);
                }
            }
        }
    }

    /// Collect all node infos for debugging/listing
    pub fn all_node_infos(&self) -> Vec<OctreeNodeInfo> {
        let mut infos = Vec::new();
        self.collect_infos(&self.root, &mut infos);
        infos
    }

    fn collect_infos(&self, node: &OctreeNode, infos: &mut Vec<OctreeNodeInfo>) {
        infos.push(OctreeNodeInfo {
            node_id: node.node_id.clone(),
            bounds: node.bounds.clone(),
            level: node.level,
            point_count: node.point_count(),
            has_children: node.has_children(),
        });
        for child in &node.children {
            if let Some(ref c) = child {
                self.collect_infos(c, infos);
            }
        }
    }
}
