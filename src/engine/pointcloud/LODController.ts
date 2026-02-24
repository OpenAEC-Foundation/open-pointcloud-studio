/**
 * LOD Controller — Manages loaded octree nodes for pointcloud rendering.
 *
 * Calls Tauri commands to determine which nodes are visible based on camera,
 * loads/unloads geometry on-demand, and throttles updates to max 10Hz.
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

interface PointChunk {
  node_id: string;
  center: [number, number, number];
  positions: number[];
  colors: number[];
  intensities: number[];
  classifications: number[];
  point_count: number;
}

interface LoadedNode {
  nodeId: string;
  points: THREE.Points;
  lastUsed: number;
}

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

  /** Update visible nodes based on current camera. Throttled to max 10Hz. */
  async update(camera: THREE.PerspectiveCamera, pointBudget: number): Promise<void> {
    if (this.disposed || this.isUpdating) return;

    const now = Date.now();
    if (now - this.lastUpdateTime < this.updateInterval) return;
    this.lastUpdateTime = now;
    this.isUpdating = true;

    try {
      const cameraState = {
        position: [
          camera.position.x + this.worldOffset[0],
          camera.position.y + this.worldOffset[1],
          camera.position.z + this.worldOffset[2],
        ],
        target: [0, 0, 0], // orbit target not easily available, use approximate
        fov: camera.fov,
        aspect: camera.aspect,
        screen_height: camera.getFilmHeight() > 0 ? window.innerHeight : 800,
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
        await this.loadNodes(toLoad);
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

  /** Load point chunks from the Rust backend and create Three.js geometry */
  private async loadNodes(nodeIds: string[]): Promise<void> {
    try {
      const chunks: PointChunk[] = await invoke('pointcloud_get_nodes', {
        id: this.pointcloudId,
        nodeIds,
      });

      for (const chunk of chunks) {
        if (this.disposed) return;
        this.createPointsObject(chunk);
      }
    } catch (error) {
      console.error('[LODController] Failed to load nodes:', error);
    }
  }

  /** Create a THREE.Points object from a PointChunk */
  private createPointsObject(chunk: PointChunk): void {
    const geometry = new THREE.BufferGeometry();

    // Positions (already relative to chunk center)
    const positions = new Float32Array(chunk.positions);
    geometry.setAttribute('position', new THREE.BufferAttribute(positions, 3));

    // Colors (0-255 → 0-1)
    const colors = new Float32Array(chunk.colors.length);
    for (let i = 0; i < chunk.colors.length; i++) {
      colors[i] = chunk.colors[i] / 255;
    }
    geometry.setAttribute('aColor', new THREE.BufferAttribute(colors, 3));

    // Intensities (0-65535 → 0-1)
    const intensities = new Float32Array(chunk.intensities.length);
    for (let i = 0; i < chunk.intensities.length; i++) {
      intensities[i] = chunk.intensities[i] / 65535;
    }
    geometry.setAttribute('aIntensity', new THREE.BufferAttribute(intensities, 1));

    // Classifications
    const classifications = new Float32Array(chunk.classifications.length);
    for (let i = 0; i < chunk.classifications.length; i++) {
      classifications[i] = chunk.classifications[i];
    }
    geometry.setAttribute('aClassification', new THREE.BufferAttribute(classifications, 1));

    const points = new THREE.Points(geometry, this.material);

    // Position the chunk at its world center, offset by worldOffset for precision
    points.position.set(
      chunk.center[0] - this.worldOffset[0],
      chunk.center[1] - this.worldOffset[1],
      chunk.center[2] - this.worldOffset[2],
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
