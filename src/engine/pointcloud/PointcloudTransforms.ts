/**
 * Pointcloud Transform Operations
 *
 * Pure functions for translating, scaling, and thinning pointcloud data.
 * Operates on ParsedPointcloud data in the BrowserPointcloudStore.
 */

import { getBrowserPointcloud, setBrowserPointcloud } from './BrowserPointcloudStore';
import type { ParsedPointcloud } from './LASParser';

/**
 * Translate all points by (dx, dy, dz). Mutates positions in-place.
 */
export function translatePointcloud(pcId: string, dx: number, dy: number, dz: number): ParsedPointcloud | null {
  const parsed = getBrowserPointcloud(pcId);
  if (!parsed) return null;

  const pos = parsed.positions;
  const count = pos.length / 3;
  for (let i = 0; i < count; i++) {
    pos[i * 3] += dx;
    pos[i * 3 + 1] += dy;
    pos[i * 3 + 2] += dz;
  }

  return parsed;
}

/**
 * Scale all points around the centroid by (sx, sy, sz). Mutates positions in-place.
 */
export function scalePointcloud(pcId: string, sx: number, sy: number, sz: number): ParsedPointcloud | null {
  const parsed = getBrowserPointcloud(pcId);
  if (!parsed) return null;

  const pos = parsed.positions;
  const count = pos.length / 3;

  // Compute centroid
  let cx = 0, cy = 0, cz = 0;
  for (let i = 0; i < count; i++) {
    cx += pos[i * 3];
    cy += pos[i * 3 + 1];
    cz += pos[i * 3 + 2];
  }
  cx /= count;
  cy /= count;
  cz /= count;

  // Scale around centroid
  for (let i = 0; i < count; i++) {
    pos[i * 3] = cx + (pos[i * 3] - cx) * sx;
    pos[i * 3 + 1] = cy + (pos[i * 3 + 1] - cy) * sy;
    pos[i * 3 + 2] = cz + (pos[i * 3 + 2] - cz) * sz;
  }

  return parsed;
}

/**
 * Thin the pointcloud to keep only `percentage`% of points via random sampling.
 * Creates new arrays and updates the store entry.
 */
export function thinPointcloud(pcId: string, percentage: number): ParsedPointcloud | null {
  const parsed = getBrowserPointcloud(pcId);
  if (!parsed) return null;

  const clampedPct = Math.max(1, Math.min(100, percentage));
  const oldCount = parsed.positions.length / 3;
  const keepCount = Math.max(1, Math.round(oldCount * clampedPct / 100));

  if (keepCount >= oldCount) return parsed;

  // Fisher-Yates partial shuffle to select random indices
  const indices = new Uint32Array(oldCount);
  for (let i = 0; i < oldCount; i++) indices[i] = i;
  for (let i = 0; i < keepCount; i++) {
    const j = i + Math.floor(Math.random() * (oldCount - i));
    const tmp = indices[i];
    indices[i] = indices[j];
    indices[j] = tmp;
  }
  // Sort kept indices for cache-friendly access
  const kept = Array.from(indices.subarray(0, keepCount)).sort((a, b) => a - b);

  const newPositions = new Float32Array(keepCount * 3);
  const newColors = new Float32Array(keepCount * 3);
  const newIntensities = new Float32Array(keepCount);
  const newClassifications = new Float32Array(keepCount);

  for (let w = 0; w < keepCount; w++) {
    const src = kept[w];
    newPositions[w * 3] = parsed.positions[src * 3];
    newPositions[w * 3 + 1] = parsed.positions[src * 3 + 1];
    newPositions[w * 3 + 2] = parsed.positions[src * 3 + 2];
    newColors[w * 3] = parsed.colors[src * 3];
    newColors[w * 3 + 1] = parsed.colors[src * 3 + 1];
    newColors[w * 3 + 2] = parsed.colors[src * 3 + 2];
    newIntensities[w] = parsed.intensities[src];
    newClassifications[w] = parsed.classifications[src];
  }

  const thinned: ParsedPointcloud = {
    ...parsed,
    positions: newPositions,
    colors: newColors,
    intensities: newIntensities,
    classifications: newClassifications,
  };

  setBrowserPointcloud(pcId, thinned);
  return thinned;
}
