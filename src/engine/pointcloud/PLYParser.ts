/**
 * Browser-side PLY file parser for point clouds.
 *
 * Supports ASCII and little-endian binary PLY files containing vertex data.
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

interface PLYProperty {
  name: string;
  type: string;
}

function typeSize(t: string): number {
  switch (t) {
    case 'char': case 'int8': case 'uchar': case 'uint8': return 1;
    case 'short': case 'int16': case 'ushort': case 'uint16': return 2;
    case 'int': case 'int32': case 'uint': case 'uint32': case 'float': case 'float32': return 4;
    case 'double': case 'float64': return 8;
    default: return 4;
  }
}

function readTyped(view: DataView, offset: number, type: string): number {
  switch (type) {
    case 'char': case 'int8': return view.getInt8(offset);
    case 'uchar': case 'uint8': return view.getUint8(offset);
    case 'short': case 'int16': return view.getInt16(offset, true);
    case 'ushort': case 'uint16': return view.getUint16(offset, true);
    case 'int': case 'int32': return view.getInt32(offset, true);
    case 'uint': case 'uint32': return view.getUint32(offset, true);
    case 'float': case 'float32': return view.getFloat32(offset, true);
    case 'double': case 'float64': return view.getFloat64(offset, true);
    default: return view.getFloat32(offset, true);
  }
}

export function parsePLY(buffer: ArrayBuffer): ParsedPointcloud {
  // Parse header as text
  const bytes = new Uint8Array(buffer);
  let headerEnd = 0;
  for (let i = 0; i < Math.min(bytes.length, 10000); i++) {
    // Look for "end_header\n"
    if (bytes[i] === 0x65 && i + 10 < bytes.length) { // 'e'
      const chunk = new TextDecoder().decode(bytes.slice(i, i + 11));
      if (chunk.startsWith('end_header')) {
        headerEnd = i + 10;
        // Skip \r\n or \n
        if (bytes[headerEnd] === 0x0d) headerEnd++;
        if (bytes[headerEnd] === 0x0a) headerEnd++;
        break;
      }
    }
  }

  if (headerEnd === 0) throw new Error('Invalid PLY file: no end_header found');

  const headerText = new TextDecoder().decode(bytes.slice(0, headerEnd));
  const headerLines = headerText.split(/\r?\n/);

  if (!headerLines[0].trim().startsWith('ply')) {
    throw new Error('Invalid PLY file: missing "ply" magic');
  }

  let format = 'ascii';
  let vertexCount = 0;
  const properties: PLYProperty[] = [];
  let inVertex = false;

  for (const line of headerLines) {
    const parts = line.trim().split(/\s+/);
    if (parts[0] === 'format') {
      format = parts[1];
    } else if (parts[0] === 'element' && parts[1] === 'vertex') {
      vertexCount = parseInt(parts[2]);
      inVertex = true;
    } else if (parts[0] === 'element') {
      inVertex = false;
    } else if (parts[0] === 'property' && inVertex && parts[1] !== 'list') {
      properties.push({ type: parts[1], name: parts[2] });
    }
  }

  if (vertexCount === 0) throw new Error('PLY file has no vertices');

  // Find property indices
  const findProp = (names: string[]) => properties.findIndex((p) => names.includes(p.name));
  const xIdx = findProp(['x']);
  const yIdx = findProp(['y']);
  const zIdx = findProp(['z']);
  const rIdx = findProp(['red', 'r']);
  const gIdx = findProp(['green', 'g']);
  const bIdx = findProp(['blue', 'b']);
  const iIdx = findProp(['intensity', 'scalar_intensity']);

  if (xIdx < 0 || yIdx < 0 || zIdx < 0) {
    throw new Error('PLY file missing x/y/z vertex properties');
  }

  const hasColor = rIdx >= 0 && gIdx >= 0 && bIdx >= 0;
  const hasIntensity = iIdx >= 0;

  const maxPoints = 5_000_000;
  const stride = vertexCount > maxPoints ? Math.ceil(vertexCount / maxPoints) : 1;
  const actualCount = Math.ceil(vertexCount / stride);

  const positions = new Float32Array(actualCount * 3);
  const colors = new Float32Array(actualCount * 3);
  const intensities = new Float32Array(actualCount);
  const classifications = new Float32Array(actualCount);

  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  let outIdx = 0;

  if (format === 'ascii') {
    const dataText = new TextDecoder().decode(bytes.slice(headerEnd));
    const dataLines = dataText.split(/\r?\n/);

    // First pass: bounds (sample)
    const sampleStep = Math.max(1, Math.floor(vertexCount / 10000));
    for (let i = 0; i < Math.min(dataLines.length, vertexCount); i += sampleStep) {
      const vals = dataLines[i].trim().split(/\s+/);
      if (vals.length < properties.length) continue;
      const x = parseFloat(vals[xIdx]);
      const y = parseFloat(vals[yIdx]);
      const z = parseFloat(vals[zIdx]);
      if (isNaN(x) || isNaN(y) || isNaN(z)) continue;
      minX = Math.min(minX, x); maxX = Math.max(maxX, x);
      minY = Math.min(minY, y); maxY = Math.max(maxY, y);
      minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
    }

    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    const cz = (minZ + maxZ) / 2;

    for (let i = 0; i < Math.min(dataLines.length, vertexCount); i += stride) {
      const vals = dataLines[i].trim().split(/\s+/);
      if (vals.length < 3) continue;

      const x = parseFloat(vals[xIdx]);
      const y = parseFloat(vals[yIdx]);
      const z = parseFloat(vals[zIdx]);
      if (isNaN(x) || isNaN(y) || isNaN(z)) continue;

      positions[outIdx * 3] = x - cx;
      positions[outIdx * 3 + 1] = z - cz;
      positions[outIdx * 3 + 2] = -(y - cy);

      if (hasColor) {
        const r = parseFloat(vals[rIdx]);
        const g = parseFloat(vals[gIdx]);
        const b = parseFloat(vals[bIdx]);
        colors[outIdx * 3] = r > 1 ? r / 255 : r;
        colors[outIdx * 3 + 1] = g > 1 ? g / 255 : g;
        colors[outIdx * 3 + 2] = b > 1 ? b / 255 : b;
      } else {
        colors[outIdx * 3] = 0.8;
        colors[outIdx * 3 + 1] = 0.8;
        colors[outIdx * 3 + 2] = 0.8;
      }

      if (hasIntensity) {
        intensities[outIdx] = parseFloat(vals[iIdx]);
      }

      outIdx++;
    }

    return buildResult(outIdx, positions, colors, intensities, classifications, minX, minY, minZ, maxX, maxY, maxZ, hasColor, hasIntensity);
  }

  // Binary format
  if (format !== 'binary_little_endian') {
    throw new Error(`Unsupported PLY format: ${format}. Only ascii and binary_little_endian are supported.`);
  }

  const recordSize = properties.reduce((sum, p) => sum + typeSize(p.type), 0);
  const propOffsets: number[] = [];
  let off = 0;
  for (const p of properties) {
    propOffsets.push(off);
    off += typeSize(p.type);
  }

  const dataView = new DataView(buffer, headerEnd);

  // First pass: bounds (sample)
  const sampleStep = Math.max(1, Math.floor(vertexCount / 10000));
  for (let i = 0; i < vertexCount; i += sampleStep) {
    const base = i * recordSize;
    if (base + recordSize > dataView.byteLength) break;
    const x = readTyped(dataView, base + propOffsets[xIdx], properties[xIdx].type);
    const y = readTyped(dataView, base + propOffsets[yIdx], properties[yIdx].type);
    const z = readTyped(dataView, base + propOffsets[zIdx], properties[zIdx].type);
    minX = Math.min(minX, x); maxX = Math.max(maxX, x);
    minY = Math.min(minY, y); maxY = Math.max(maxY, y);
    minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  for (let i = 0; i < vertexCount; i += stride) {
    const base = i * recordSize;
    if (base + recordSize > dataView.byteLength) break;

    const x = readTyped(dataView, base + propOffsets[xIdx], properties[xIdx].type);
    const y = readTyped(dataView, base + propOffsets[yIdx], properties[yIdx].type);
    const z = readTyped(dataView, base + propOffsets[zIdx], properties[zIdx].type);

    positions[outIdx * 3] = x - cx;
    positions[outIdx * 3 + 1] = z - cz;
    positions[outIdx * 3 + 2] = -(y - cy);

    if (hasColor) {
      const r = readTyped(dataView, base + propOffsets[rIdx], properties[rIdx].type);
      const g = readTyped(dataView, base + propOffsets[gIdx], properties[gIdx].type);
      const b = readTyped(dataView, base + propOffsets[bIdx], properties[bIdx].type);
      colors[outIdx * 3] = r > 1 ? r / 255 : r;
      colors[outIdx * 3 + 1] = g > 1 ? g / 255 : g;
      colors[outIdx * 3 + 2] = b > 1 ? b / 255 : b;
    } else {
      colors[outIdx * 3] = 0.8;
      colors[outIdx * 3 + 1] = 0.8;
      colors[outIdx * 3 + 2] = 0.8;
    }

    if (hasIntensity) {
      intensities[outIdx] = readTyped(dataView, base + propOffsets[iIdx], properties[iIdx].type);
    }

    outIdx++;
  }

  return buildResult(outIdx, positions, colors, intensities, classifications, minX, minY, minZ, maxX, maxY, maxZ, hasColor, hasIntensity);
}

function buildResult(
  count: number,
  positions: Float32Array, colors: Float32Array,
  intensities: Float32Array, classifications: Float32Array,
  minX: number, minY: number, minZ: number,
  maxX: number, maxY: number, maxZ: number,
  hasColor: boolean, hasIntensity: boolean,
): ParsedPointcloud {
  const header: LASHeader = {
    signature: 'PLY',
    versionMajor: 0, versionMinor: 0,
    headerSize: 0, offsetToPointData: 0,
    pointDataFormat: 0, pointDataRecordLength: 0,
    numberOfPoints: count,
    scaleX: 1, scaleY: 1, scaleZ: 1,
    offsetX: 0, offsetY: 0, offsetZ: 0,
    minX, minY, minZ, maxX, maxY, maxZ,
  };

  return {
    header,
    positions: positions.slice(0, count * 3),
    colors: colors.slice(0, count * 3),
    intensities: intensities.slice(0, count),
    classifications: classifications.slice(0, count),
    center: [(minX + maxX) / 2, (minY + maxY) / 2, (minZ + maxZ) / 2],
    hasColor,
    hasIntensity,
    hasClassification: false,
  };
}
