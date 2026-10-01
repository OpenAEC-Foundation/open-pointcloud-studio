/**
 * Browser-side STL file parser.
 *
 * Supports both ASCII and binary STL formats.
 * Extracts unique vertices from triangle mesh via spatial hash deduplication.
 * Binary STL may contain per-face color in the 2-byte attribute field (VisCAM convention).
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

/** Detect whether buffer is ASCII STL (starts with "solid" and has reasonable text) */
function isAsciiSTL(buffer: ArrayBuffer): boolean {
  const header = new Uint8Array(buffer, 0, Math.min(80, buffer.byteLength));
  // Check for "solid" prefix
  const prefix = String.fromCharCode(...header.slice(0, 5));
  if (prefix !== 'solid') return false;

  // Additional check: binary STL with "solid" in header trick
  // Read expected triangle count from binary header and validate size
  if (buffer.byteLength > 84) {
    const view = new DataView(buffer);
    const triCount = view.getUint32(80, true);
    const expectedSize = 84 + triCount * 50;
    if (Math.abs(expectedSize - buffer.byteLength) <= 1) return false; // matches binary layout
  }

  return true;
}

function parseAsciiSTL(text: string): { verts: number[][]; faceIndices: number[][] } {
  const verts: number[][] = [];
  const faceIndices: number[][] = [];
  const vertMap = new Map<string, number>();

  function addVertex(x: number, y: number, z: number): number {
    const key = `${x.toFixed(6)},${y.toFixed(6)},${z.toFixed(6)}`;
    const existing = vertMap.get(key);
    if (existing !== undefined) return existing;
    const idx = verts.length;
    verts.push([x, y, z]);
    vertMap.set(key, idx);
    return idx;
  }

  const lines = text.split(/\r?\n/);
  let faceVerts: number[] = [];

  for (const line of lines) {
    const trimmed = line.trim();
    if (trimmed.startsWith('vertex')) {
      const parts = trimmed.split(/\s+/);
      const x = parseFloat(parts[1]);
      const y = parseFloat(parts[2]);
      const z = parseFloat(parts[3]);
      if (!isNaN(x) && !isNaN(y) && !isNaN(z)) {
        faceVerts.push(addVertex(x, y, z));
      }
    } else if (trimmed.startsWith('endfacet')) {
      if (faceVerts.length === 3) {
        faceIndices.push([...faceVerts]);
      }
      faceVerts = [];
    }
  }

  return { verts, faceIndices };
}

function parseBinarySTL(buffer: ArrayBuffer): {
  verts: number[][];
  faceIndices: number[][];
  faceColors: ([number, number, number] | null)[];
} {
  const view = new DataView(buffer);
  const triCount = view.getUint32(80, true);

  const verts: number[][] = [];
  const faceIndices: number[][] = [];
  const faceColors: ([number, number, number] | null)[] = [];
  const vertMap = new Map<string, number>();

  function addVertex(x: number, y: number, z: number): number {
    const key = `${x.toFixed(6)},${y.toFixed(6)},${z.toFixed(6)}`;
    const existing = vertMap.get(key);
    if (existing !== undefined) return existing;
    const idx = verts.length;
    verts.push([x, y, z]);
    vertMap.set(key, idx);
    return idx;
  }

  let offset = 84;
  let hasAnyColor = false;

  for (let i = 0; i < triCount; i++) {
    // Skip normal (12 bytes)
    offset += 12;

    const idx0 = addVertex(
      view.getFloat32(offset, true), view.getFloat32(offset + 4, true), view.getFloat32(offset + 8, true),
    );
    offset += 12;
    const idx1 = addVertex(
      view.getFloat32(offset, true), view.getFloat32(offset + 4, true), view.getFloat32(offset + 8, true),
    );
    offset += 12;
    const idx2 = addVertex(
      view.getFloat32(offset, true), view.getFloat32(offset + 4, true), view.getFloat32(offset + 8, true),
    );
    offset += 12;

    faceIndices.push([idx0, idx1, idx2]);

    // Attribute byte count — may contain VisCAM color
    const attr = view.getUint16(offset, true);
    offset += 2;

    if (attr & 0x8000) {
      // VisCAM convention: bit 15 set = color valid, 5-bit channels (BGR order)
      const b = ((attr >> 10) & 0x1f) / 31;
      const g = ((attr >> 5) & 0x1f) / 31;
      const r = (attr & 0x1f) / 31;
      faceColors.push([r, g, b]);
      hasAnyColor = true;
    } else {
      faceColors.push(null);
    }
  }

  if (!hasAnyColor) faceColors.length = 0;

  return { verts, faceIndices, faceColors };
}

export function parseSTL(buffer: ArrayBuffer): ParsedPointcloud {
  const ascii = isAsciiSTL(buffer);

  let verts: number[][];
  let faceIndices: number[][];
  let faceColors: ([number, number, number] | null)[] = [];

  if (ascii) {
    const text = new TextDecoder().decode(buffer);
    ({ verts, faceIndices } = parseAsciiSTL(text));
  } else {
    ({ verts, faceIndices, faceColors } = parseBinarySTL(buffer));
  }

  if (verts.length === 0) throw new Error('STL file has no vertices');

  const vertexCount = verts.length;
  const positions = new Float32Array(vertexCount * 3);
  const colors = new Float32Array(vertexCount * 3);
  const intensities = new Float32Array(vertexCount);
  const classifications = new Float32Array(vertexCount);

  // Find bounds
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  for (const [x, y, z] of verts) {
    minX = Math.min(minX, x); maxX = Math.max(maxX, x);
    minY = Math.min(minY, y); maxY = Math.max(maxY, y);
    minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  // Assign per-vertex colors from face colors (if present)
  const hasColor = faceColors.length > 0;
  const vertexColors: Float32Array = new Float32Array(vertexCount * 3);
  if (hasColor) {
    // Track which vertices have been colored (first-write wins)
    const colored = new Uint8Array(vertexCount);
    for (let fi = 0; fi < faceIndices.length; fi++) {
      const color = faceColors[fi];
      if (!color) continue;
      for (const vi of faceIndices[fi]) {
        if (!colored[vi]) {
          vertexColors[vi * 3] = color[0];
          vertexColors[vi * 3 + 1] = color[1];
          vertexColors[vi * 3 + 2] = color[2];
          colored[vi] = 1;
        }
      }
    }
  }

  // Fill positions and colors
  for (let i = 0; i < vertexCount; i++) {
    const [x, y, z] = verts[i];
    // Z-up to Y-up
    positions[i * 3] = x - cx;
    positions[i * 3 + 1] = z - cz;
    positions[i * 3 + 2] = -(y - cy);

    if (hasColor) {
      colors[i * 3] = vertexColors[i * 3];
      colors[i * 3 + 1] = vertexColors[i * 3 + 1];
      colors[i * 3 + 2] = vertexColors[i * 3 + 2];
    } else {
      colors[i * 3] = 0.8;
      colors[i * 3 + 1] = 0.8;
      colors[i * 3 + 2] = 0.8;
    }
  }

  // Build triangle index buffer
  const triangleIndices: number[] = [];
  for (const face of faceIndices) {
    triangleIndices.push(face[0], face[1], face[2]);
  }
  const indices = triangleIndices.length > 0 ? new Uint32Array(triangleIndices) : undefined;

  const header: LASHeader = {
    signature: 'STL',
    versionMajor: 0, versionMinor: 0,
    headerSize: 0, offsetToPointData: 0,
    pointDataFormat: 0, pointDataRecordLength: 0,
    numberOfPoints: vertexCount,
    scaleX: 1, scaleY: 1, scaleZ: 1,
    offsetX: 0, offsetY: 0, offsetZ: 0,
    minX, minY, minZ, maxX, maxY, maxZ,
  };

  return {
    header,
    positions,
    colors,
    intensities,
    classifications,
    center: [cx, cy, cz],
    hasColor,
    hasIntensity: false,
    hasClassification: false,
    indices,
  };
}
