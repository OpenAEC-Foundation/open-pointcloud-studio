/**
 * Browser-side OBJ file parser for point clouds / mesh vertices.
 *
 * Extracts vertex positions (and vertex colors if present) from OBJ files.
 * OBJ vertex lines: "v X Y Z" or "v X Y Z R G B"
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

export function parseOBJ(buffer: ArrayBuffer): ParsedPointcloud {
  const text = new TextDecoder().decode(buffer);
  const lines = text.split(/\r?\n/);

  // Count vertices first for pre-allocation
  let vertexCount = 0;
  for (const line of lines) {
    if (line.startsWith('v ')) vertexCount++;
  }

  if (vertexCount === 0) throw new Error('OBJ file has no vertices');

  const maxPoints = 5_000_000;
  const stride = vertexCount > maxPoints ? Math.ceil(vertexCount / maxPoints) : 1;
  const actualCount = Math.ceil(vertexCount / stride);

  const positions = new Float32Array(actualCount * 3);
  const colors = new Float32Array(actualCount * 3);
  const intensities = new Float32Array(actualCount);
  const classifications = new Float32Array(actualCount);

  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  let hasColor = false;

  // First pass: bounds (sample)
  let vi = 0;
  const sampleStep = Math.max(1, Math.floor(vertexCount / 10000));
  for (const line of lines) {
    if (!line.startsWith('v ')) continue;
    if (vi % sampleStep === 0) {
      const parts = line.trim().split(/\s+/);
      const x = parseFloat(parts[1]);
      const y = parseFloat(parts[2]);
      const z = parseFloat(parts[3]);
      if (!isNaN(x) && !isNaN(y) && !isNaN(z)) {
        minX = Math.min(minX, x); maxX = Math.max(maxX, x);
        minY = Math.min(minY, y); maxY = Math.max(maxY, y);
        minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
      }
      if (parts.length >= 7) hasColor = true;
    }
    vi++;
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  // Second pass: extract data
  vi = 0;
  let outIdx = 0;
  for (const line of lines) {
    if (!line.startsWith('v ')) continue;

    if (vi % stride === 0) {
      const parts = line.trim().split(/\s+/);
      const x = parseFloat(parts[1]);
      const y = parseFloat(parts[2]);
      const z = parseFloat(parts[3]);

      if (isNaN(x) || isNaN(y) || isNaN(z)) { vi++; continue; }

      // Z-up to Y-up
      positions[outIdx * 3] = x - cx;
      positions[outIdx * 3 + 1] = z - cz;
      positions[outIdx * 3 + 2] = -(y - cy);

      if (hasColor && parts.length >= 7) {
        const r = parseFloat(parts[4]);
        const g = parseFloat(parts[5]);
        const b = parseFloat(parts[6]);
        colors[outIdx * 3] = r > 1 ? r / 255 : r;
        colors[outIdx * 3 + 1] = g > 1 ? g / 255 : g;
        colors[outIdx * 3 + 2] = b > 1 ? b / 255 : b;
      } else {
        colors[outIdx * 3] = 0.8;
        colors[outIdx * 3 + 1] = 0.8;
        colors[outIdx * 3 + 2] = 0.8;
      }

      outIdx++;
    }
    vi++;
  }

  const header: LASHeader = {
    signature: 'OBJ',
    versionMajor: 0, versionMinor: 0,
    headerSize: 0, offsetToPointData: 0,
    pointDataFormat: 0, pointDataRecordLength: 0,
    numberOfPoints: outIdx,
    scaleX: 1, scaleY: 1, scaleZ: 1,
    offsetX: 0, offsetY: 0, offsetZ: 0,
    minX, minY, minZ, maxX, maxY, maxZ,
  };

  return {
    header,
    positions: positions.slice(0, outIdx * 3),
    colors: colors.slice(0, outIdx * 3),
    intensities: intensities.slice(0, outIdx),
    classifications: classifications.slice(0, outIdx),
    center: [cx, cy, cz],
    hasColor,
    hasIntensity: false,
    hasClassification: false,
  };
}
