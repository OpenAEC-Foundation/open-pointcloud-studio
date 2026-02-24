/**
 * Browser-side OBJ file parser.
 *
 * Extracts vertex positions, vertex colors, and face indices from OBJ files.
 * When faces are present, the result includes triangle indices for mesh rendering.
 *
 * OBJ vertex lines: "v X Y Z" or "v X Y Z R G B"
 * OBJ face lines:   "f v1 v2 v3", "f v1/vt1 v2/vt2 ...", "f v1/vt1/vn1 ...", "f v1//vn1 ..."
 * Quads and n-gons are triangulated via fan triangulation.
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

export function parseOBJ(buffer: ArrayBuffer): ParsedPointcloud {
  const text = new TextDecoder().decode(buffer);
  const lines = text.split(/\r?\n/);

  // Count vertices and faces for pre-allocation
  let vertexCount = 0;
  let faceCount = 0;
  for (const line of lines) {
    if (line.startsWith('v ')) vertexCount++;
    else if (line.startsWith('f ')) faceCount++;
  }

  if (vertexCount === 0) throw new Error('OBJ file has no vertices');

  const hasFaces = faceCount > 0;

  // For mesh OBJs, keep all vertices (no downsampling) to preserve face topology.
  // For point-only OBJs, downsample if too many.
  const maxPoints = 5_000_000;
  const stride = (!hasFaces && vertexCount > maxPoints) ? Math.ceil(vertexCount / maxPoints) : 1;
  const actualVertexCount = Math.ceil(vertexCount / stride);

  const positions = new Float32Array(actualVertexCount * 3);
  const colors = new Float32Array(actualVertexCount * 3);
  const intensities = new Float32Array(actualVertexCount);
  const classifications = new Float32Array(actualVertexCount);

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

  // Second pass: extract vertex data
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

  // Third pass: extract face indices (triangulate quads/n-gons)
  let indices: Uint32Array | undefined;
  if (hasFaces && stride === 1) {
    // Estimate max triangles: each face with N verts produces N-2 triangles
    // Most faces are tris (1) or quads (2), allocate conservatively
    const triangleIndices: number[] = [];

    for (const line of lines) {
      if (!line.startsWith('f ')) continue;
      const parts = line.trim().split(/\s+/);
      // Extract vertex indices from face entries
      const faceVerts: number[] = [];
      for (let i = 1; i < parts.length; i++) {
        // Face vertex format: v, v/vt, v/vt/vn, v//vn
        const vIdx = parseInt(parts[i].split('/')[0], 10);
        if (isNaN(vIdx)) continue;
        // OBJ indices are 1-based; negative means relative to end
        faceVerts.push(vIdx > 0 ? vIdx - 1 : outIdx + vIdx);
      }

      // Fan triangulation for polygons with 3+ vertices
      for (let i = 1; i < faceVerts.length - 1; i++) {
        triangleIndices.push(faceVerts[0], faceVerts[i], faceVerts[i + 1]);
      }
    }

    if (triangleIndices.length > 0) {
      indices = new Uint32Array(triangleIndices);
    }
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
    indices,
  };
}
