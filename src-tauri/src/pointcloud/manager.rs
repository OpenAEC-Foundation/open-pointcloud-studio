use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use super::parser::PointcloudParser;
use super::octree::Octree;
use super::types::{
    CameraState, IndexProgress, OctreeNodeInfo, PointChunk, PointcloudMetadata,
};

/// Lifecycle state for a single loaded pointcloud
struct PointcloudEntry {
    metadata: PointcloudMetadata,
    octree: Option<Octree>,
    progress: IndexProgress,
}

/// Manages all loaded pointclouds — shared via Tauri state
pub struct PointcloudManager {
    entries: RwLock<HashMap<String, PointcloudEntry>>,
    next_id: Mutex<u32>,
}

impl PointcloudManager {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(HashMap::new()),
            next_id: Mutex::new(1),
        }
    }

    /// Open a pointcloud file, parse header, and start async octree construction.
    /// Returns metadata immediately; octree builds in the background.
    pub fn open(self: &Arc<Self>, file_path: &str) -> Result<PointcloudMetadata, String> {
        let id = {
            let mut counter = self.next_id.lock().unwrap();
            let id = format!("pc_{}", *counter);
            *counter += 1;
            id
        };

        let parser = PointcloudParser::open(file_path)?;
        let metadata = parser.metadata(&id, file_path);
        let total_points = parser.total_points();

        let progress = IndexProgress {
            progress: 0.0,
            phase: "Reading points".into(),
            points_processed: 0,
            total_points,
        };

        let entry = PointcloudEntry {
            metadata: metadata.clone(),
            octree: None,
            progress: progress.clone(),
        };

        self.entries.write().unwrap().insert(id.clone(), entry);

        // Build octree in a background thread so UI doesn't block
        let manager = Arc::clone(self);
        let id_clone = id.clone();
        std::thread::spawn(move || {
            if let Err(e) = manager.build_octree(&id_clone, parser) {
                eprintln!("Octree build failed for {}: {}", id_clone, e);
                if let Ok(mut entries) = manager.entries.write() {
                    if let Some(entry) = entries.get_mut(&id_clone) {
                        entry.progress.phase = format!("Error: {}", e);
                    }
                }
            }
        });

        Ok(metadata)
    }

    /// Build octree from parser (runs on background thread)
    fn build_octree(&self, id: &str, parser: PointcloudParser) -> Result<(), String> {
        let bounds = parser.bounds();
        let total = parser.total_points();
        let mut all_points = Vec::new();

        // Update progress
        {
            let mut entries = self.entries.write().unwrap();
            if let Some(entry) = entries.get_mut(id) {
                entry.progress.phase = "Reading points".into();
            }
        }

        // Read all points in batches
        let batch_size = 100_000u64;
        let id_owned = id.to_string();
        let entries_ref = &self.entries;

        parser.stream_points(batch_size, |batch, offset| {
            all_points.extend_from_slice(batch);

            // Update progress
            if let Ok(mut entries) = entries_ref.write() {
                if let Some(entry) = entries.get_mut(&id_owned) {
                    entry.progress.points_processed = offset + batch.len() as u64;
                    entry.progress.progress = (offset + batch.len() as u64) as f64 / total as f64 * 0.5;
                }
            }
            true
        })?;

        // Build octree
        {
            let mut entries = self.entries.write().unwrap();
            if let Some(entry) = entries.get_mut(id) {
                entry.progress.phase = "Building octree".into();
                entry.progress.progress = 0.5;
            }
        }

        let octree = Octree::build(all_points, bounds);

        // Store octree
        {
            let mut entries = self.entries.write().unwrap();
            if let Some(entry) = entries.get_mut(id) {
                entry.octree = Some(octree);
                entry.progress.phase = "Complete".into();
                entry.progress.progress = 1.0;
            }
        }

        Ok(())
    }

    /// Get indexing progress
    pub fn get_progress(&self, id: &str) -> Option<IndexProgress> {
        self.entries.read().unwrap().get(id).map(|e| e.progress.clone())
    }

    /// Get metadata for a pointcloud
    pub fn get_metadata(&self, id: &str) -> Option<PointcloudMetadata> {
        self.entries.read().unwrap().get(id).map(|e| e.metadata.clone())
    }

    /// Load point data for specific nodes
    pub fn get_nodes(&self, id: &str, node_ids: &[String]) -> Result<Vec<PointChunk>, String> {
        let entries = self.entries.read().unwrap();
        let entry = entries.get(id).ok_or("Pointcloud not found")?;
        let octree = entry.octree.as_ref().ok_or("Octree not yet built")?;

        let mut chunks = Vec::new();
        for node_id in node_ids {
            if let Some(chunk) = octree.get_node_chunk(node_id) {
                chunks.push(chunk);
            }
        }
        Ok(chunks)
    }

    /// Get visible nodes for LOD rendering
    pub fn get_visible_nodes(
        &self,
        id: &str,
        camera: &CameraState,
        point_budget: u32,
    ) -> Result<Vec<OctreeNodeInfo>, String> {
        let entries = self.entries.read().unwrap();
        let entry = entries.get(id).ok_or("Pointcloud not found")?;
        let octree = entry.octree.as_ref().ok_or("Octree not yet built")?;

        let node_ids = octree.get_visible_nodes(camera, point_budget);
        let mut infos = Vec::new();
        for node_id in &node_ids {
            if let Some(info) = octree.get_node_info(node_id) {
                infos.push(info);
            }
        }
        Ok(infos)
    }

    /// Close and remove a pointcloud
    pub fn close(&self, id: &str) -> bool {
        self.entries.write().unwrap().remove(id).is_some()
    }

    /// List all loaded pointclouds
    pub fn list(&self) -> Vec<PointcloudMetadata> {
        self.entries.read().unwrap().values().map(|e| e.metadata.clone()).collect()
    }
}
