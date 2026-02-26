use std::sync::Arc;
use tauri::State;
use tauri::ipc::Response;

use super::manager::PointcloudManager;
use super::types::{CameraState, IndexProgress, OctreeNodeInfo, PointChunk, PointcloudMetadata};

/// Open a pointcloud file, parse header and start async octree indexing.
/// Returns metadata immediately; octree builds in background.
#[tauri::command]
pub fn pointcloud_open(
    file_path: String,
    state: State<'_, Arc<PointcloudManager>>,
) -> Result<PointcloudMetadata, String> {
    state.inner().open(&file_path)
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

/// Load point data for specific octree nodes as a flat binary buffer.
/// Returns raw bytes via tauri::ipc::Response, bypassing JSON serialization.
///
/// Wire format:
///   [4 bytes] chunk_count (u32 LE)
///   Per chunk:
///     [4 bytes]                node_id_len (u32 LE)
///     [N bytes]                node_id (UTF-8)
///     [0-3 bytes]              padding to 4-byte alignment
///     [24 bytes]               center: 3x f64 LE
///     [4 bytes]                level (u32 LE)
///     [4 bytes]                spacing (f32 LE)
///     [4 bytes]                point_count (u32 LE)
///     [point_count * 12 bytes] positions: f32 LE (x,y,z)
///     [point_count * 3 bytes]  colors: u8 (r,g,b)
///     [point_count * 2 bytes]  intensities: u16 LE
///     [point_count * 1 byte]   classifications: u8
///     [0-3 bytes]              padding to 4-byte alignment
#[tauri::command]
pub fn pointcloud_get_nodes_binary(
    id: String,
    node_ids: Vec<String>,
    state: State<'_, Arc<PointcloudManager>>,
) -> Result<Response, String> {
    let chunks = state.get_nodes(&id, &node_ids)?;
    let buf = pack_chunks_binary(&chunks);
    Ok(Response::new(buf))
}

fn pack_chunks_binary(chunks: &[PointChunk]) -> Vec<u8> {
    // Pre-calculate total size for a single allocation
    let mut total_size = 4usize; // chunk_count
    for chunk in chunks {
        let id_bytes = chunk.node_id.as_bytes().len();
        let id_padded = (id_bytes + 3) & !3; // round up to 4-byte alignment
        let data_size = (chunk.point_count as usize) * 12  // positions f32*3
                      + (chunk.point_count as usize) * 3   // colors u8*3
                      + (chunk.point_count as usize) * 2   // intensities u16
                      + (chunk.point_count as usize) * 1;  // classifications u8
        let chunk_size = 4 + id_padded + 24 + 4 + 4 + 4 + data_size; // +4 level, +4 spacing
        let chunk_padded = (chunk_size + 3) & !3;
        total_size += chunk_padded;
    }

    let mut buf = Vec::with_capacity(total_size);

    buf.extend_from_slice(&(chunks.len() as u32).to_le_bytes());

    for chunk in chunks {
        let id_bytes = chunk.node_id.as_bytes();
        let id_len = id_bytes.len() as u32;
        buf.extend_from_slice(&id_len.to_le_bytes());
        buf.extend_from_slice(id_bytes);
        // Pad node_id to 4-byte alignment
        let padding = (4 - (id_bytes.len() % 4)) % 4;
        for _ in 0..padding {
            buf.push(0);
        }

        // Center: 3x f64 LE
        for &val in &chunk.center {
            buf.extend_from_slice(&val.to_le_bytes());
        }

        // Level: u32 LE
        buf.extend_from_slice(&(chunk.level as u32).to_le_bytes());

        // Spacing: f32 LE
        buf.extend_from_slice(&chunk.spacing.to_le_bytes());

        // Point count
        buf.extend_from_slice(&chunk.point_count.to_le_bytes());

        // Positions: f32 LE
        for &val in &chunk.positions {
            buf.extend_from_slice(&val.to_le_bytes());
        }

        // Colors: raw u8
        buf.extend_from_slice(&chunk.colors);

        // Intensities: u16 LE
        for &val in &chunk.intensities {
            buf.extend_from_slice(&val.to_le_bytes());
        }

        // Classifications: raw u8
        buf.extend_from_slice(&chunk.classifications);

        // Pad to 4-byte alignment for next chunk
        let remainder = buf.len() % 4;
        if remainder != 0 {
            for _ in 0..(4 - remainder) {
                buf.push(0);
            }
        }
    }

    buf
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
