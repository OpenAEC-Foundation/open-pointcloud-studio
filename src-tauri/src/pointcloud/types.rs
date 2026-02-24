use serde::{Deserialize, Serialize};

/// 3D bounding box
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox3D {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl BoundingBox3D {
    pub fn new() -> Self {
        Self {
            min_x: f64::MAX,
            min_y: f64::MAX,
            min_z: f64::MAX,
            max_x: f64::MIN,
            max_y: f64::MIN,
            max_z: f64::MIN,
        }
    }

    pub fn expand(&mut self, x: f64, y: f64, z: f64) {
        self.min_x = self.min_x.min(x);
        self.min_y = self.min_y.min(y);
        self.min_z = self.min_z.min(z);
        self.max_x = self.max_x.max(x);
        self.max_y = self.max_y.max(y);
        self.max_z = self.max_z.max(z);
    }

    pub fn center(&self) -> [f64; 3] {
        [
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
            (self.min_z + self.max_z) * 0.5,
        ]
    }

    pub fn size(&self) -> [f64; 3] {
        [
            self.max_x - self.min_x,
            self.max_y - self.min_y,
            self.max_z - self.min_z,
        ]
    }

    pub fn max_extent(&self) -> f64 {
        let s = self.size();
        s[0].max(s[1]).max(s[2])
    }

    pub fn contains(&self, x: f64, y: f64, z: f64) -> bool {
        x >= self.min_x && x <= self.max_x
            && y >= self.min_y && y <= self.max_y
            && z >= self.min_z && z <= self.max_z
    }

    /// Split into 8 octants
    pub fn octant(&self, index: u8) -> BoundingBox3D {
        let c = self.center();
        let (x0, x1) = if index & 1 == 0 { (self.min_x, c[0]) } else { (c[0], self.max_x) };
        let (y0, y1) = if index & 2 == 0 { (self.min_y, c[1]) } else { (c[1], self.max_y) };
        let (z0, z1) = if index & 4 == 0 { (self.min_z, c[2]) } else { (c[2], self.max_z) };
        BoundingBox3D {
            min_x: x0, min_y: y0, min_z: z0,
            max_x: x1, max_y: y1, max_z: z1,
        }
    }
}

/// A single point record with all optional attributes
#[derive(Debug, Clone)]
pub struct PointRecord {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub intensity: u16,
    pub classification: u8,
}

/// Metadata about a loaded pointcloud
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointcloudMetadata {
    pub id: String,
    pub file_path: String,
    pub file_name: String,
    pub format: String,
    pub total_points: u64,
    pub bounds: BoundingBox3D,
    pub has_color: bool,
    pub has_intensity: bool,
    pub has_classification: bool,
    pub point_record_format: u8,
    pub las_version: String,
}

/// An octree node reference sent to the frontend
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OctreeNodeInfo {
    pub node_id: String,
    pub bounds: BoundingBox3D,
    pub level: u8,
    pub point_count: u32,
    pub has_children: bool,
}

/// A chunk of point data for rendering — positions as f32 relative to center, colors as u8
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PointChunk {
    pub node_id: String,
    pub center: [f64; 3],
    pub positions: Vec<f32>,
    pub colors: Vec<u8>,
    pub intensities: Vec<u16>,
    pub classifications: Vec<u8>,
    pub point_count: u32,
}

/// Camera frustum sent from frontend for LOD selection
#[derive(Debug, Clone, Deserialize)]
pub struct CameraState {
    pub position: [f64; 3],
    pub target: [f64; 3],
    pub fov: f64,
    pub aspect: f64,
    pub screen_height: f64,
}

/// Indexing progress
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexProgress {
    pub progress: f64,
    pub phase: String,
    pub points_processed: u64,
    pub total_points: u64,
}
