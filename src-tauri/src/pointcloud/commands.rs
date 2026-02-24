use std::sync::Arc;
use tauri::State;

use super::manager::PointcloudManager;
use super::types::{CameraState, IndexProgress, OctreeNodeInfo, PointChunk, PointcloudMetadata};

/// Open a pointcloud file, parse header and start octree indexing
#[tauri::command]
pub fn pointcloud_open(
    file_path: String,
    state: State<'_, Arc<PointcloudManager>>,
) -> Result<PointcloudMetadata, String> {
    state.open(&file_path)
}

/// Get octree construction progress (0.0 - 1.0)
#[tauri::command]
pub fn pointcloud_get_progress(
    id: String,
    state: State<'_, Arc<PointcloudManager>>,
) -> Result<IndexProgress, String> {
    state.get_progress(&id).ok_or_else(|| "Pointcloud not found".into())
}

/// Load point data for specific octree nodes
#[tauri::command]
pub fn pointcloud_get_nodes(
    id: String,
    node_ids: Vec<String>,
    state: State<'_, Arc<PointcloudManager>>,
) -> Result<Vec<PointChunk>, String> {
    state.get_nodes(&id, &node_ids)
}

/// Get visible nodes for LOD rendering based on camera state
#[tauri::command]
pub fn pointcloud_get_visible_nodes(
    id: String,
    camera: CameraState,
    budget: u32,
    state: State<'_, Arc<PointcloudManager>>,
) -> Result<Vec<OctreeNodeInfo>, String> {
    state.get_visible_nodes(&id, &camera, budget)
}

/// Close a pointcloud and free memory
#[tauri::command]
pub fn pointcloud_close(
    id: String,
    state: State<'_, Arc<PointcloudManager>>,
) -> Result<bool, String> {
    Ok(state.close(&id))
}

/// List all loaded pointclouds
#[tauri::command]
pub fn pointcloud_list(
    state: State<'_, Arc<PointcloudManager>>,
) -> Vec<PointcloudMetadata> {
    state.list()
}
