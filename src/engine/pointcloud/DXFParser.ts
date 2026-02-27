/**
 * Browser-side DXF file parser (AutoCAD Drawing Exchange Format).
 *
 * Extracts POINT entities and optionally 3DFACE vertex data from the ENTITIES section.
 * Supports ACI (AutoCAD Color Index) and true color (group code 420).
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

/** Standard ACI colors (indices 1–7). Index 0 = BYBLOCK, 256 = BYLAYER */
const ACI_STANDARD: Record<number, [number, number, number]> = {
  1: [1, 0, 0],       // Red
  2: [1, 1, 0],       // Yellow
  3: [0, 1, 0],       // Green
  4: [0, 1, 1],       // Cyan
  5: [0, 0, 1],       // Blue
  6: [1, 0, 1],       // Magenta
  7: [1, 1, 1],       // White/Black (depends on background)
};

/** Compute approximate RGB for ACI indices 8–255 */
function aciToRGB(index: number): [number, number, number] {
  if (index <= 0 || index > 255) return [0.8, 0.8, 0.8]; // default gray
  if (index <= 7) return ACI_STANDARD[index] || [0.8, 0.8, 0.8];
  if (index <= 9) return [0.5, 0.5, 0.5]; // grays for 8-9

  // ACI 10–249: computed from hue/saturation/value pattern
  // Simplified: use HSV-based approximation
  if (index >= 10 && index <= 249) {
    const i = index - 10;
    const row = Math.floor(i / 10);   // 0-23: hue steps
    const col = i % 10;               // 0-9: shade variants

    const hue = (row / 24) * 360;
    let sat: number, val: number;

    if (col < 2) {
      sat = 1.0;
      val = col === 0 ? 1.0 : 0.65;
    } else if (col < 4) {
      sat = 0.5;
      val = col === 2 ? 1.0 : 0.65;
    } else if (col < 6) {
      sat = 0.25;
      val = col === 4 ? 1.0 : 0.65;
    } else if (col < 8) {
      sat = 1.0;
      val = col === 6 ? 0.5 : 0.35;
    } else {
      sat = 0.5;
      val = col === 8 ? 0.5 : 0.35;
    }

    return hsvToRGB(hue, sat, val);
  }

  // ACI 250–255: grays
  const grays: Record<number, number> = {
    250: 0.33, 251: 0.44, 252: 0.56, 253: 0.67, 254: 0.78, 255: 0.89,
  };
  const g = grays[index] ?? 0.5;
  return [g, g, g];
}

function hsvToRGB(h: number, s: number, v: number): [number, number, number] {
  const c = v * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = v - c;
  let r = 0, g = 0, b = 0;

  if (h < 60) { r = c; g = x; }
  else if (h < 120) { r = x; g = c; }
  else if (h < 180) { g = c; b = x; }
  else if (h < 240) { g = x; b = c; }
  else if (h < 300) { r = x; b = c; }
  else { r = c; b = x; }

  return [r + m, g + m, b + m];
}

interface DXFPoint {
  x: number;
  y: number;
  z: number;
  color: [number, number, number] | null;
}

export function parseDXF(buffer: ArrayBuffer): ParsedPointcloud {
  const text = new TextDecoder().decode(buffer);
  const lines = text.split(/\r?\n/);

  const points: DXFPoint[] = [];
  const faceIndices: number[][] = [];

  // Parse DXF as group code/value pairs within the ENTITIES section
  let inEntities = false;
  let i = 0;

  while (i < lines.length - 1) {
    const code = parseInt(lines[i].trim(), 10);
    const value = lines[i + 1].trim();
    i += 2;

    // Track section
    if (code === 0 && value === 'SECTION') {
      // Next pair should be section name
      if (i < lines.length - 1) {
        const nameCode = parseInt(lines[i].trim(), 10);
        const nameValue = lines[i + 1].trim();
        if (nameCode === 2 && nameValue === 'ENTITIES') {
          inEntities = true;
        }
        // Don't advance i — let the outer loop handle it
      }
      continue;
    }

    if (code === 0 && value === 'ENDSEC') {
      inEntities = false;
      continue;
    }

    if (!inEntities) continue;

    // Parse POINT entity
    if (code === 0 && value === 'POINT') {
      const point = parsePointEntity(lines, i);
      if (point.point) points.push(point.point);
      i = point.nextIdx;
      continue;
    }

    // Parse 3DFACE entity
    if (code === 0 && value === '3DFACE') {
      const face = parse3DFaceEntity(lines, i);
      if (face.verts.length >= 3) {
        const baseIdx = points.length;
        for (const v of face.verts) points.push(v);
        // Triangulate the face
        if (face.verts.length === 3) {
          faceIndices.push([baseIdx, baseIdx + 1, baseIdx + 2]);
        } else if (face.verts.length === 4) {
          faceIndices.push([baseIdx, baseIdx + 1, baseIdx + 2]);
          faceIndices.push([baseIdx, baseIdx + 2, baseIdx + 3]);
        }
      }
      i = face.nextIdx;
      continue;
    }
  }

  if (points.length === 0) {
    throw new Error('DXF file has no POINT or 3DFACE entities');
  }

  // Downsampling
  const hasFaces = faceIndices.length > 0;
  const maxPoints = 5_000_000;
  const stride = (!hasFaces && points.length > maxPoints) ? Math.ceil(points.length / maxPoints) : 1;
  const actualCount = hasFaces ? points.length : Math.ceil(points.length / stride);

  const positions = new Float32Array(actualCount * 3);
  const colors = new Float32Array(actualCount * 3);
  const intensities = new Float32Array(actualCount);
  const classifications = new Float32Array(actualCount);

  // Bounds
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  let hasColor = false;

  for (const p of points) {
    minX = Math.min(minX, p.x); maxX = Math.max(maxX, p.x);
    minY = Math.min(minY, p.y); maxY = Math.max(maxY, p.y);
    minZ = Math.min(minZ, p.z); maxZ = Math.max(maxZ, p.z);
    if (p.color) hasColor = true;
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  let outIdx = 0;
  for (let pi = 0; pi < points.length; pi += stride) {
    const p = points[pi];

    // Z-up to Y-up
    positions[outIdx * 3] = p.x - cx;
    positions[outIdx * 3 + 1] = p.z - cz;
    positions[outIdx * 3 + 2] = -(p.y - cy);

    if (p.color) {
      colors[outIdx * 3] = p.color[0];
      colors[outIdx * 3 + 1] = p.color[1];
      colors[outIdx * 3 + 2] = p.color[2];
    } else {
      colors[outIdx * 3] = 0.8;
      colors[outIdx * 3 + 1] = 0.8;
      colors[outIdx * 3 + 2] = 0.8;
    }

    outIdx++;
  }

  // Build face indices (adjusted for stride=1 when faces present)
  let indices: Uint32Array | undefined;
  if (hasFaces) {
    const triIndices: number[] = [];
    for (const face of faceIndices) {
      triIndices.push(...face);
    }
    indices = new Uint32Array(triIndices);
  }

  const header: LASHeader = {
    signature: 'DXF',
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

function parsePointEntity(lines: string[], startIdx: number): { point: DXFPoint | null; nextIdx: number } {
  let i = startIdx;
  let x = 0, y = 0, z = 0;
  let hasCoords = false;
  let aciColor = -1;
  let trueColor = -1;

  while (i < lines.length - 1) {
    const code = parseInt(lines[i].trim(), 10);
    const value = lines[i + 1].trim();

    // Next entity starts
    if (code === 0) break;

    switch (code) {
      case 10: x = parseFloat(value); hasCoords = true; break;
      case 20: y = parseFloat(value); break;
      case 30: z = parseFloat(value); break;
      case 62: aciColor = parseInt(value, 10); break;
      case 420: trueColor = parseInt(value, 10); break;
    }

    i += 2;
  }

  if (!hasCoords) return { point: null, nextIdx: i };

  let color: [number, number, number] | null = null;
  if (trueColor >= 0) {
    color = [(trueColor >> 16 & 0xFF) / 255, (trueColor >> 8 & 0xFF) / 255, (trueColor & 0xFF) / 255];
  } else if (aciColor >= 0) {
    color = aciToRGB(aciColor);
  }

  return { point: { x, y, z, color }, nextIdx: i };
}

function parse3DFaceEntity(lines: string[], startIdx: number): { verts: DXFPoint[]; nextIdx: number } {
  let i = startIdx;
  const coords: number[][] = [[], [], [], []]; // up to 4 corners
  let aciColor = -1;
  let trueColor = -1;

  while (i < lines.length - 1) {
    const code = parseInt(lines[i].trim(), 10);
    const value = lines[i + 1].trim();

    if (code === 0) break;

    // 3DFACE vertex codes: (10,20,30), (11,21,31), (12,22,32), (13,23,33)
    if (code >= 10 && code <= 13) {
      coords[code - 10][0] = parseFloat(value);
    } else if (code >= 20 && code <= 23) {
      coords[code - 20][1] = parseFloat(value);
    } else if (code >= 30 && code <= 33) {
      coords[code - 30][2] = parseFloat(value);
    } else if (code === 62) {
      aciColor = parseInt(value, 10);
    } else if (code === 420) {
      trueColor = parseInt(value, 10);
    }

    i += 2;
  }

  let color: [number, number, number] | null = null;
  if (trueColor >= 0) {
    color = [(trueColor >> 16 & 0xFF) / 255, (trueColor >> 8 & 0xFF) / 255, (trueColor & 0xFF) / 255];
  } else if (aciColor >= 0) {
    color = aciToRGB(aciColor);
  }

  const verts: DXFPoint[] = [];
  for (const c of coords) {
    if (c.length === 3 && !isNaN(c[0]) && !isNaN(c[1]) && !isNaN(c[2])) {
      verts.push({ x: c[0], y: c[1], z: c[2], color });
    }
  }

  // 3DFACE: if 4th vertex equals 3rd, it's a triangle (deduplicate)
  if (verts.length === 4) {
    const v2 = verts[2], v3 = verts[3];
    if (v2.x === v3.x && v2.y === v3.y && v2.z === v3.z) {
      verts.pop();
    }
  }

  return { verts, nextIdx: i };
}
