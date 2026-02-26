/**
 * LOD Controller — Manages loaded octree nodes for pointcloud rendering.
 *
 * Calls Tauri commands to determine which nodes are visible based on camera,
 * loads/unloads geometry on-demand, and throttles updates to max 10Hz.
 * Uses binary IPC for fast point data transfer from the Rust backend.
 */

import * as THREE from 'three';
import { invoke } from '@tauri-apps/api/core';
import { createPointcloudMaterial, updatePointcloudMaterial, type PointcloudMaterialOptions } from './PointcloudMaterial';
interface OctreeNodeInfo {
  node_id: string;
  bounds: {
    min_x: number; min_y: number; min_z: number;
    max_x: number; max_y: number; max_z: number;
  };
  level: number;
  point_count: number;
  has_children: boolean;
}

interface DecodedChunk {
  node_id: string;
  center: [number, number, number];
  level: number;
  spacing: number;
  point_count: number;
  positions: Float32Array;
  colors: Uint8Array;
  intensities: Uint16Array;
  classifications: Uint8Array;
}

interface LoadedNode {
  nodeId: string;
  points: THREE.Points;
  lastUsed: number;
}

const BATCH_SIZE = 15;

export class LODController {
  private scene: THREE.Scene;
  private loadedNodes: Map<string, LoadedNode> = new Map();
  private material: THREE.ShaderMaterial;
  private pointcloudId: string;
  private lastUpdateTime = 0;
  private updateInterval = 100; // 10Hz throttle
  private isUpdating = false;
  private disposed = false;

  // Offset applied to all pointcloud positions to avoid floating point issues
  private worldOffset: [number, number, number] = [0, 0, 0];

  // Camera change detection
  private lastCameraPosition = new THREE.Vector3();
  private lastCameraRotation = new THREE.Euler();
  private lastPointBudget = 0;
  private cameraInitialized = false;

  constructor(scene: THREE.Scene, pointcloudId: string, options?: PointcloudMaterialOptions) {
    this.scene = scene;
    this.pointcloudId = pointcloudId;
    this.material = createPointcloudMaterial(options);
  }

  /** Set the world offset (typically the center of the pointcloud bounds) */
  setWorldOffset(offset: [number, number, number]): void {
    this.worldOffset = offset;
  }

  /** Update material settings */
  updateMaterial(options: Partial<PointcloudMaterialOptions>): void {
    updatePointcloudMaterial(this.material, options);
  }

  /** Check if camera has moved since last update */
  private hasCameraMoved(camera: THREE.PerspectiveCamera, pointBudget: number): boolean {
    if (!this.cameraInitialized) {
      this.lastCameraPosition.copy(camera.position);
      this.lastCameraRotation.copy(camera.rotation);
      this.lastPointBudget = pointBudget;
      this.cameraInitialized = true;
      return true;
    }

    const posDelta = camera.position.distanceTo(this.lastCameraPosition);
    const rotDelta = Math.abs(camera.rotation.x - this.lastCameraRotation.x)
                   + Math.abs(camera.rotation.y - this.lastCameraRotation.y)
                   + Math.abs(camera.rotation.z - this.lastCameraRotation.z);
    const budgetChanged = pointBudget !== this.lastPointBudget;

    if (posDelta > 0.001 || rotDelta > 0.001 || budgetChanged) {
      this.lastCameraPosition.copy(camera.position);
      this.lastCameraRotation.copy(camera.rotation);
      this.lastPointBudget = pointBudget;
      return true;
    }

    return false;
  }

  /** Update visible nodes based on current camera. Throttled to max 10Hz. */
  async update(camera: THREE.PerspectiveCamera, pointBudget: number): Promise<void> {
    if (this.disposed || this.isUpdating) return;

    const now = Date.now();
    if (now - this.lastUpdateTime < this.updateInterval) return;
    this.lastUpdateTime = now;

    if (!this.hasCameraMoved(camera, pointBudget)) return;

    this.isUpdating = true;

    try {
      const cameraState = {
        position: [
          camera.position.x + this.worldOffset[0],
          camera.position.z + this.worldOffset[1],
          camera.position.y + this.worldOffset[2],
        ],
        target: [0, 0, 0],
        fov: camera.fov,
        aspect: camera.aspect,
        screen_height: window.innerHeight || 800,
      };

      // Get visible node list from Rust backend
      const visibleNodes: OctreeNodeInfo[] = await invoke('pointcloud_get_visible_nodes', {
        id: this.pointcloudId,
        camera: cameraState,
        budget: pointBudget,
      });

      const visibleIds = new Set(visibleNodes.map((n) => n.node_id));

      // Unload nodes that are no longer visible
      for (const [nodeId, loaded] of this.loadedNodes) {
        if (!visibleIds.has(nodeId)) {
          this.scene.remove(loaded.points);
          loaded.points.geometry.dispose();
          this.loadedNodes.delete(nodeId);
        }
      }

      // Find nodes that need loading
      const toLoad = visibleNodes
        .filter((n) => !this.loadedNodes.has(n.node_id))
        .map((n) => n.node_id);

      if (toLoad.length > 0) {
        for (let i = 0; i < toLoad.length; i += BATCH_SIZE) {
          if (this.disposed) return;
          const batch = toLoad.slice(i, i + BATCH_SIZE);
          await this.loadNodes(batch);
        }
      }

      // Update last-used timestamp for visible nodes
      const timestamp = Date.now();
      for (const id of visibleIds) {
        const node = this.loadedNodes.get(id);
        if (node) node.lastUsed = timestamp;
      }
    } catch (error) {
      console.error('[LODController] Update failed:', error);
    } finally {
      this.isUpdating = false;
    }
  }

  /** Load point chunks from the Rust backend via binary IPC */
  private async loadNodes(nodeIds: string[]): Promise<void> {
    try {
      const buffer: ArrayBuffer = await invoke('pointcloud_get_nodes_binary', {
        id: this.pointcloudId,
        nodeIds,
      });

      const chunks = decodeBinaryChunks(buffer);

      for (const chunk of chunks) {
        if (this.disposed) return;
        this.createPointsObject(chunk);
      }
    } catch (error) {
      console.error('[LODController] Failed to load nodes:', error);
    }
  }

  /** Create a THREE.Points object from a decoded binary chunk */
  private createPointsObject(chunk: DecodedChunk): void {
    const geometry = new THREE.BufferGeometry();

    // Positions — swap Y/Z to convert from Z-up (LAS) to Y-up (Three.js)
    const positions = chunk.positions;
    for (let i = 0; i < chunk.point_count; i++) {
      const base = i * 3;
      const tmp = positions[base + 1];
      positions[base + 1] = positions[base + 2];
      positions[base + 2] = tmp;
    }
    geometry.setAttribute('position', new THREE.BufferAttribute(positions, 3));

    // Colors (0-255 → 0-1)
    const colors = new Float32Array(chunk.point_count * 3);
    for (let i = 0; i < chunk.point_count * 3; i++) {
      colors[i] = chunk.colors[i] / 255;
    }
    geometry.setAttribute('aColor', new THREE.BufferAttribute(colors, 3));

    // Intensities (0-65535 → 0-1)
    const intensities = new Float32Array(chunk.point_count);
    for (let i = 0; i < chunk.point_count; i++) {
      intensities[i] = chunk.intensities[i] / 65535;
    }
    geometry.setAttribute('aIntensity', new THREE.BufferAttribute(intensities, 1));

    // Classifications
    const classifications = new Float32Array(chunk.point_count);
    for (let i = 0; i < chunk.point_count; i++) {
      classifications[i] = chunk.classifications[i];
    }
    geometry.setAttribute('aClassification', new THREE.BufferAttribute(classifications, 1));

    const points = new THREE.Points(geometry, this.material);

    // Position the chunk at its world center, offset by worldOffset for precision
    // Swap Y/Z to convert from Z-up (LAS) to Y-up (Three.js)
    points.position.set(
      chunk.center[0] - this.worldOffset[0],
      chunk.center[2] - this.worldOffset[2],
      chunk.center[1] - this.worldOffset[1],
    );

    this.scene.add(points);
    this.loadedNodes.set(chunk.node_id, {
      nodeId: chunk.node_id,
      points,
      lastUsed: Date.now(),
    });
  }

  /** Get the total number of loaded points */
  getLoadedPointCount(): number {
    let total = 0;
    for (const node of this.loadedNodes.values()) {
      const attr = node.points.geometry.getAttribute('position');
      if (attr) total += attr.count;
    }
    return total;
  }

  /** Get number of loaded nodes */
  getLoadedNodeCount(): number {
    return this.loadedNodes.size;
  }

  /** Clean up all resources */
  dispose(): void {
    this.disposed = true;
    for (const node of this.loadedNodes.values()) {
      this.scene.remove(node.points);
      node.points.geometry.dispose();
    }
    this.loadedNodes.clear();
    this.material.dispose();
  }
}

/**
 * Decode the binary buffer returned by pointcloud_get_nodes_binary.
 *
 * Wire format:
 *   [4 bytes] chunk_count (u32 LE)
 *   Per chunk:
 *     [4 bytes]                node_id_len (u32 LE)
 *     [N bytes]                node_id (UTF-8)
 *     [0-3 bytes]              padding to 4-byte alignment
 *     [24 bytes]               center: 3x f64 LE
 *     [4 bytes]                level (u32 LE)
 *     [4 bytes]                spacing (f32 LE)
 *     [4 bytes]                point_count (u32 LE)
 *     [point_count * 12 bytes] positions: f32 LE (x,y,z)
 *     [point_count * 3 bytes]  colors: u8 (r,g,b)
 *     [point_count * 2 bytes]  intensities: u16 LE
 *     [point_count * 1 byte]   classifications: u8
 *     [0-3 bytes]              padding to 4-byte alignment
 */
function decodeBinaryChunks(buffer: ArrayBuffer): DecodedChunk[] {
  const view = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  let offset = 0;

  const chunkCount = view.getUint32(offset, true);
  offset += 4;

  const chunks: DecodedChunk[] = new Array(chunkCount);
  const textDecoder = new TextDecoder();

  for (let i = 0; i < chunkCount; i++) {
    // Node ID
    const idLen = view.getUint32(offset, true);
    offset += 4;
    const nodeId = textDecoder.decode(bytes.subarray(offset, offset + idLen));
    offset += idLen;
    // Pad to 4-byte alignment
    offset = (offset + 3) & ~3;

    // Center: 3x f64
    const cx = view.getFloat64(offset, true); offset += 8;
    const cy = view.getFloat64(offset, true); offset += 8;
    const cz = view.getFloat64(offset, true); offset += 8;

    // Level
    const level = view.getUint32(offset, true);
    offset += 4;

    // Spacing
    const spacing = view.getFloat32(offset, true);
    offset += 4;

    // Point count
    const pointCount = view.getUint32(offset, true);
    offset += 4;

    // Positions: f32 LE — create a copy since the offset may not be aligned for Float32Array view
    const posBytes = pointCount * 12;
    const positions = new Float32Array(pointCount * 3);
    for (let j = 0; j < pointCount * 3; j++) {
      positions[j] = view.getFloat32(offset + j * 4, true);
    }
    offset += posBytes;

    // Colors: u8 — direct slice copy
    const colorBytes = pointCount * 3;
    const colors = new Uint8Array(buffer.slice(offset, offset + colorBytes));
    offset += colorBytes;

    // Intensities: u16 LE
    const intBytes = pointCount * 2;
    const intensities = new Uint16Array(pointCount);
    for (let j = 0; j < pointCount; j++) {
      intensities[j] = view.getUint16(offset + j * 2, true);
    }
    offset += intBytes;

    // Classifications: u8
    const classifications = new Uint8Array(buffer.slice(offset, offset + pointCount));
    offset += pointCount;

    // Pad to 4-byte alignment
    offset = (offset + 3) & ~3;

    chunks[i] = {
      node_id: nodeId,
      center: [cx, cy, cz],
      level,
      spacing,
      point_count: pointCount,
      positions,
      colors,
      intensities,
      classifications,
    };
  }

  return chunks;
}
