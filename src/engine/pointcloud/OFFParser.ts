/**
 * Browser-side OFF file parser.
 *
 * Supports OFF, COFF (color), NOFF (normals), and CNOFF (color+normals) variants.
 * Extracts vertex positions, optional vertex colors, and face indices.
 * Quads and n-gons are triangulated via fan triangulation.
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

export function parseOFF(buffer: ArrayBuffer): ParsedPointcloud {
  const text = new TextDecoder().decode(buffer);
  const lines = text.split(/\r?\n/);

  // Parse magic line to detect variant
  let lineIdx = 0;
  while (lineIdx < lines.length && lines[lineIdx].trim() === '') lineIdx++;
  if (lineIdx >= lines.length) throw new Error('OFF file is empty');

  const magicLine = lines[lineIdx].trim();
  let hasVertexColor = false;
  let hasNormals = false;

  // Magic can be: OFF, COFF, NOFF, CNOFF — optionally followed by counts on same line
  let magicPart = '';
  if (magicLine.startsWith('CNOFF')) { hasVertexColor = true; hasNormals = true; magicPart = 'CNOFF'; }
  else if (magicLine.startsWith('COFF')) { hasVertexColor = true; magicPart = 'COFF'; }
  else if (magicLine.startsWith('NOFF')) { hasNormals = true; magicPart = 'NOFF'; }
  else if (magicLine.startsWith('OFF')) { magicPart = 'OFF'; }
  else throw new Error('Not a valid OFF file (missing OFF header)');

  // Check if counts are on the same line as magic
  const afterMagic = magicLine.substring(magicPart.length).trim();
  let vertexCount: number, faceCount: number;

  if (afterMagic.length > 0) {
    const parts = afterMagic.split(/\s+/);
    vertexCount = parseInt(parts[0], 10);
    faceCount = parseInt(parts[1], 10);
  } else {
    lineIdx++;
    // Skip comments and empty lines
    while (lineIdx < lines.length && (lines[lineIdx].trim() === '' || lines[lineIdx].trim().startsWith('#'))) lineIdx++;
    const countParts = lines[lineIdx].trim().split(/\s+/);
    vertexCount = parseInt(countParts[0], 10);
    faceCount = parseInt(countParts[1], 10);
  }
  lineIdx++;

  if (isNaN(vertexCount) || vertexCount === 0) throw new Error('OFF file has no vertices');

  // Skip comments/empty lines before vertex data
  while (lineIdx < lines.length && (lines[lineIdx].trim() === '' || lines[lineIdx].trim().startsWith('#'))) lineIdx++;

  const hasFaces = faceCount > 0;
  const maxPoints = 5_000_000;
  const stride = (!hasFaces && vertexCount > maxPoints) ? Math.ceil(vertexCount / maxPoints) : 1;
  const actualVertexCount = Math.ceil(vertexCount / stride);

  const positions = new Float32Array(actualVertexCount * 3);
  const colors = new Float32Array(actualVertexCount * 3);
  const intensities = new Float32Array(actualVertexCount);
  const classifications = new Float32Array(actualVertexCount);

  // Values per vertex line: x y z [nx ny nz] [r g b [a]]
  const colorOffset = hasNormals ? 6 : 3;

  // First pass: bounds (sample)
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  const sampleStep = Math.max(1, Math.floor(vertexCount / 10000));

  for (let i = 0; i < vertexCount; i += sampleStep) {
    const li = lineIdx + i;
    if (li >= lines.length) break;
    const parts = lines[li].trim().split(/\s+/);
    if (parts.length < 3) continue;
    const x = parseFloat(parts[0]);
    const y = parseFloat(parts[1]);
    const z = parseFloat(parts[2]);
    if (isNaN(x) || isNaN(y) || isNaN(z)) continue;
    minX = Math.min(minX, x); maxX = Math.max(maxX, x);
    minY = Math.min(minY, y); maxY = Math.max(maxY, y);
    minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);

    // Auto-detect color if not declared by magic
    if (!hasVertexColor && parts.length > colorOffset + 2) {
      hasVertexColor = true;
    }
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  // Second pass: extract vertices
  let outIdx = 0;
  for (let i = 0; i < vertexCount; i++) {
    if (i % stride !== 0) continue;
    const li = lineIdx + i;
    if (li >= lines.length) break;
    const parts = lines[li].trim().split(/\s+/);
    if (parts.length < 3) continue;

    const x = parseFloat(parts[0]);
    const y = parseFloat(parts[1]);
    const z = parseFloat(parts[2]);
    if (isNaN(x) || isNaN(y) || isNaN(z)) continue;

    // Z-up to Y-up
    positions[outIdx * 3] = x - cx;
    positions[outIdx * 3 + 1] = z - cz;
    positions[outIdx * 3 + 2] = -(y - cy);

    if (hasVertexColor && parts.length > colorOffset + 2) {
      let r = parseFloat(parts[colorOffset]);
      let g = parseFloat(parts[colorOffset + 1]);
      let b = parseFloat(parts[colorOffset + 2]);
      // Normalize: int 0-255 or float 0-1
      r = r > 1 ? r / 255 : r;
      g = g > 1 ? g / 255 : g;
      b = b > 1 ? b / 255 : b;
      colors[outIdx * 3] = r;
      colors[outIdx * 3 + 1] = g;
      colors[outIdx * 3 + 2] = b;
    } else {
      colors[outIdx * 3] = 0.8;
      colors[outIdx * 3 + 1] = 0.8;
      colors[outIdx * 3 + 2] = 0.8;
    }

    outIdx++;
  }

  // Move lineIdx past vertex data
  const faceStartLine = lineIdx + vertexCount;

  // Third pass: extract face indices
  let indices: Uint32Array | undefined;
  if (hasFaces && stride === 1) {
    const triangleIndices: number[] = [];

    for (let i = 0; i < faceCount; i++) {
      const li = faceStartLine + i;
      if (li >= lines.length) break;
      const trimmed = lines[li].trim();
      if (trimmed === '' || trimmed.startsWith('#')) continue;
      const parts = trimmed.split(/\s+/).map(Number);
      const n = parts[0]; // number of vertices in this face

      // Fan triangulation
      for (let j = 1; j < n - 1; j++) {
        triangleIndices.push(parts[1], parts[1 + j], parts[1 + j + 1]);
      }
    }

    if (triangleIndices.length > 0) {
      indices = new Uint32Array(triangleIndices);
    }
  }

  const header: LASHeader = {
    signature: 'OFF',
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
    hasColor: hasVertexColor,
    hasIntensity: false,
    hasClassification: false,
    indices,
  };
}
