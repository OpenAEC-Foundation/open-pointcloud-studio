/**
 * Browser-side PCD file parser (Point Cloud Data — PCL/ROS format).
 *
 * Supports three data modes:
 *   - ascii: text-based point data
 *   - binary: raw binary point records
 *   - binary_compressed: LZF-compressed column-major binary data
 *
 * Handles packed RGB/RGBA float fields, separate R/G/B channels,
 * intensity, and label/classification fields.
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

interface PCDField {
  name: string;
  size: number;
  type: string; // F, U, I
  count: number;
  offset: number; // byte offset within a point record
}

interface PCDHeader {
  version: string;
  fields: PCDField[];
  width: number;
  height: number;
  points: number;
  data: 'ascii' | 'binary' | 'binary_compressed';
  viewpoint: number[]; // tx ty tz qw qx qy qz
  headerLength: number; // byte offset where data begins
}

/** Minimal LZF decompressor */
function decompressLZF(input: Uint8Array, outputLength: number): Uint8Array {
  const output = new Uint8Array(outputLength);
  let ip = 0;
  let op = 0;

  while (ip < input.length) {
    let ctrl = input[ip++];

    if (ctrl < 32) {
      // Literal run: copy ctrl+1 bytes
      ctrl++;
      for (let i = 0; i < ctrl; i++) {
        output[op++] = input[ip++];
      }
    } else {
      // Back-reference
      let len = ctrl >> 5;
      let ref = op - ((ctrl & 0x1f) << 8) - 1;

      if (len === 7) {
        len += input[ip++];
      }
      ref -= input[ip++];
      len += 2;

      for (let i = 0; i < len; i++) {
        output[op] = output[ref];
        op++;
        ref++;
      }
    }
  }

  return output;
}

function parsePCDHeader(buffer: ArrayBuffer): PCDHeader {
  // Read header as text (headers are always ASCII, terminated by DATA line)
  const bytes = new Uint8Array(buffer);
  let headerEnd = 0;
  let headerText = '';

  // Find end of header (line starting with DATA)
  for (let i = 0; i < Math.min(bytes.length, 65536); i++) {
    if (bytes[i] === 0x0A) { // newline
      const line = headerText.split('\n').pop()?.trim() ?? '';
      if (line.startsWith('DATA ')) {
        headerEnd = i + 1;
        headerText += String.fromCharCode(bytes[i]);
        break;
      }
    }
    headerText += String.fromCharCode(bytes[i]);
  }

  const headerLines = headerText.split('\n').map((l) => l.trim()).filter((l) => l && !l.startsWith('#'));

  let version = '0.7';
  const fieldNames: string[] = [];
  const fieldSizes: number[] = [];
  const fieldTypes: string[] = [];
  const fieldCounts: number[] = [];
  let width = 0, height = 1, points = 0;
  let data: 'ascii' | 'binary' | 'binary_compressed' = 'ascii';
  const viewpoint = [0, 0, 0, 1, 0, 0, 0];

  for (const line of headerLines) {
    const parts = line.split(/\s+/);
    const key = parts[0];

    switch (key) {
      case 'VERSION':
        version = parts[1];
        break;
      case 'FIELDS':
        for (let i = 1; i < parts.length; i++) fieldNames.push(parts[i].toLowerCase());
        break;
      case 'SIZE':
        for (let i = 1; i < parts.length; i++) fieldSizes.push(parseInt(parts[i], 10));
        break;
      case 'TYPE':
        for (let i = 1; i < parts.length; i++) fieldTypes.push(parts[i]);
        break;
      case 'COUNT':
        for (let i = 1; i < parts.length; i++) fieldCounts.push(parseInt(parts[i], 10));
        break;
      case 'WIDTH':
        width = parseInt(parts[1], 10);
        break;
      case 'HEIGHT':
        height = parseInt(parts[1], 10);
        break;
      case 'POINTS':
        points = parseInt(parts[1], 10);
        break;
      case 'VIEWPOINT':
        for (let i = 1; i < Math.min(parts.length, 8); i++) viewpoint[i - 1] = parseFloat(parts[i]);
        break;
      case 'DATA':
        data = parts[1].toLowerCase() as 'ascii' | 'binary' | 'binary_compressed';
        break;
    }
  }

  if (points === 0) points = width * height;

  // Build field descriptors with offsets
  const fields: PCDField[] = [];
  let offset = 0;
  for (let i = 0; i < fieldNames.length; i++) {
    const count = fieldCounts[i] || 1;
    fields.push({
      name: fieldNames[i],
      size: fieldSizes[i] || 4,
      type: fieldTypes[i] || 'F',
      count,
      offset,
    });
    offset += (fieldSizes[i] || 4) * count;
  }

  return { version, fields, width, height, points, data, viewpoint, headerLength: headerEnd };
}

/** Apply viewpoint quaternion rotation to a point */
function applyViewpoint(vp: number[], x: number, y: number, z: number): [number, number, number] {
  const [tx, ty, tz, qw, qx, qy, qz] = vp;

  // Rotate by quaternion
  const ix = qw * x + qy * z - qz * y;
  const iy = qw * y + qz * x - qx * z;
  const iz = qw * z + qx * y - qy * x;
  const iw = -qx * x - qy * y - qz * z;

  const rx = ix * qw + iw * -qx + iy * -qz - iz * -qy;
  const ry = iy * qw + iw * -qy + iz * -qx - ix * -qz;
  const rz = iz * qw + iw * -qz + ix * -qy - iy * -qx;

  return [rx + tx, ry + ty, rz + tz];
}

function hasNonIdentityViewpoint(vp: number[]): boolean {
  return vp[0] !== 0 || vp[1] !== 0 || vp[2] !== 0 ||
         vp[3] !== 1 || vp[4] !== 0 || vp[5] !== 0 || vp[6] !== 0;
}

function readField(view: DataView, offset: number, field: PCDField): number {
  switch (field.type) {
    case 'F':
      return field.size === 4 ? view.getFloat32(offset, true) : view.getFloat64(offset, true);
    case 'U':
      if (field.size === 1) return view.getUint8(offset);
      if (field.size === 2) return view.getUint16(offset, true);
      return view.getUint32(offset, true);
    case 'I':
      if (field.size === 1) return view.getInt8(offset);
      if (field.size === 2) return view.getInt16(offset, true);
      return view.getInt32(offset, true);
    default:
      return 0;
  }
}

/** Unpack RGB from float (reinterpret float bits as int32) */
function unpackRGB(packed: number): [number, number, number] {
  // Reinterpret float32 bits as int32
  const buf = new ArrayBuffer(4);
  new Float32Array(buf)[0] = packed;
  const int = new Uint32Array(buf)[0];

  const r = ((int >> 16) & 0xFF) / 255;
  const g = ((int >> 8) & 0xFF) / 255;
  const b = (int & 0xFF) / 255;
  return [r, g, b];
}

export function parsePCD(buffer: ArrayBuffer): ParsedPointcloud {
  const header = parsePCDHeader(buffer);
  const { fields, points, data: dataMode } = header;

  if (points === 0) throw new Error('PCD file has no points');

  // Build field index
  const fieldMap = new Map<string, PCDField>();
  for (const f of fields) fieldMap.set(f.name, f);

  const hasXYZ = fieldMap.has('x') && fieldMap.has('y') && fieldMap.has('z');
  if (!hasXYZ) throw new Error('PCD file missing x/y/z fields');

  // Calculate point record size
  let recordSize = 0;
  for (const f of fields) recordSize += f.size * f.count;

  // Downsampling
  const maxPoints = 5_000_000;
  const stride = points > maxPoints ? Math.ceil(points / maxPoints) : 1;
  const estimatedCount = Math.ceil(points / stride);

  const positions = new Float32Array(estimatedCount * 3);
  const colors = new Float32Array(estimatedCount * 3);
  const intensities = new Float32Array(estimatedCount);
  const classifications = new Float32Array(estimatedCount);

  const useViewpoint = hasNonIdentityViewpoint(header.viewpoint);

  let outIdx = 0;
  let hasColor = false;
  let hasIntensity = false;
  let hasClassification = false;

  if (dataMode === 'ascii') {
    // Parse ASCII data
    const text = new TextDecoder().decode(buffer);
    const allLines = text.split(/\r?\n/);

    // Find first data line (after header)
    const headerLineCount = new TextDecoder().decode(new Uint8Array(buffer, 0, header.headerLength))
      .split(/\r?\n/).length - 1;
    let dataLineStart = headerLineCount;

    // Skip empty lines
    while (dataLineStart < allLines.length && allLines[dataLineStart].trim() === '') dataLineStart++;

    // Find field column indices
    const xIdx = fields.findIndex((f) => f.name === 'x');
    const yIdx = fields.findIndex((f) => f.name === 'y');
    const zIdx = fields.findIndex((f) => f.name === 'z');
    const rgbIdx = fields.findIndex((f) => f.name === 'rgb' || f.name === 'rgba');
    const rIdx = fields.findIndex((f) => f.name === 'r');
    const gIdx = fields.findIndex((f) => f.name === 'g');
    const bIdx = fields.findIndex((f) => f.name === 'b');
    const intIdx = fields.findIndex((f) => f.name === 'intensity');
    const lblIdx = fields.findIndex((f) => f.name === 'label' || f.name === 'classification');

    // First pass: bounds
    let minX = Infinity, minY = Infinity, minZ = Infinity;
    let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
    const sampleStep = Math.max(1, Math.floor(points / 10000));

    for (let i = 0; i < points; i += sampleStep) {
      const li = dataLineStart + i;
      if (li >= allLines.length) break;
      const parts = allLines[li].trim().split(/\s+/);
      if (parts.length < 3) continue;
      let x = parseFloat(parts[xIdx]);
      let y = parseFloat(parts[yIdx]);
      let z = parseFloat(parts[zIdx]);
      if (isNaN(x) || isNaN(y) || isNaN(z)) continue;
      if (useViewpoint) [x, y, z] = applyViewpoint(header.viewpoint, x, y, z);
      minX = Math.min(minX, x); maxX = Math.max(maxX, x);
      minY = Math.min(minY, y); maxY = Math.max(maxY, y);
      minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
    }

    const cx = (minX + maxX) / 2;
    const cy = (minY + maxY) / 2;
    const cz = (minZ + maxZ) / 2;

    // Second pass: extract data
    for (let i = 0; i < points; i += stride) {
      const li = dataLineStart + i;
      if (li >= allLines.length) break;
      const parts = allLines[li].trim().split(/\s+/);
      if (parts.length <= zIdx) continue;

      let x = parseFloat(parts[xIdx]);
      let y = parseFloat(parts[yIdx]);
      let z = parseFloat(parts[zIdx]);
      if (isNaN(x) || isNaN(y) || isNaN(z)) continue;
      if (useViewpoint) [x, y, z] = applyViewpoint(header.viewpoint, x, y, z);

      positions[outIdx * 3] = x - cx;
      positions[outIdx * 3 + 1] = z - cz;
      positions[outIdx * 3 + 2] = -(y - cy);

      if (rgbIdx >= 0 && rgbIdx < parts.length) {
        const packed = parseFloat(parts[rgbIdx]);
        const [r, g, b] = unpackRGB(packed);
        colors[outIdx * 3] = r;
        colors[outIdx * 3 + 1] = g;
        colors[outIdx * 3 + 2] = b;
        hasColor = true;
      } else if (rIdx >= 0 && gIdx >= 0 && bIdx >= 0) {
        colors[outIdx * 3] = parseFloat(parts[rIdx]) / 255;
        colors[outIdx * 3 + 1] = parseFloat(parts[gIdx]) / 255;
        colors[outIdx * 3 + 2] = parseFloat(parts[bIdx]) / 255;
        hasColor = true;
      } else {
        colors[outIdx * 3] = 0.8;
        colors[outIdx * 3 + 1] = 0.8;
        colors[outIdx * 3 + 2] = 0.8;
      }

      if (intIdx >= 0 && intIdx < parts.length) {
        intensities[outIdx] = parseFloat(parts[intIdx]);
        hasIntensity = true;
      }

      if (lblIdx >= 0 && lblIdx < parts.length) {
        classifications[outIdx] = parseFloat(parts[lblIdx]);
        hasClassification = true;
      }

      outIdx++;
    }

    return buildResult('PCD', outIdx, positions, colors, intensities, classifications,
      minX, minY, minZ, maxX, maxY, maxZ, cx, cy, cz, hasColor, hasIntensity, hasClassification);
  }

  // Binary modes
  let dataBytes: Uint8Array;

  if (dataMode === 'binary') {
    dataBytes = new Uint8Array(buffer, header.headerLength);
  } else {
    // binary_compressed: 4-byte compressed size, 4-byte uncompressed size, then LZF data
    const compView = new DataView(buffer, header.headerLength);
    const compressedSize = compView.getUint32(0, true);
    const uncompressedSize = compView.getUint32(4, true);
    const compressedData = new Uint8Array(buffer, header.headerLength + 8, compressedSize);
    dataBytes = decompressLZF(compressedData, uncompressedSize);
  }

  const dataView = new DataView(dataBytes.buffer, dataBytes.byteOffset, dataBytes.byteLength);
  const isColumnMajor = dataMode === 'binary_compressed';

  // First pass: bounds
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  const sampleStep = Math.max(1, Math.floor(points / 10000));

  const xField = fieldMap.get('x')!;
  const yField = fieldMap.get('y')!;
  const zField = fieldMap.get('z')!;

  for (let i = 0; i < points; i += sampleStep) {
    let x: number, y: number, z: number;

    if (isColumnMajor) {
      // Column-major: each field is stored contiguously for all points
      x = readFieldColumnMajor(dataView, i, xField, points, fields);
      y = readFieldColumnMajor(dataView, i, yField, points, fields);
      z = readFieldColumnMajor(dataView, i, zField, points, fields);
    } else {
      const base = i * recordSize;
      x = readField(dataView, base + xField.offset, xField);
      y = readField(dataView, base + yField.offset, yField);
      z = readField(dataView, base + zField.offset, zField);
    }

    if (isNaN(x) || isNaN(y) || isNaN(z)) continue;
    if (useViewpoint) [x, y, z] = applyViewpoint(header.viewpoint, x, y, z);
    minX = Math.min(minX, x); maxX = Math.max(maxX, x);
    minY = Math.min(minY, y); maxY = Math.max(maxY, y);
    minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  const rgbField = fieldMap.get('rgb') || fieldMap.get('rgba');
  const rField = fieldMap.get('r');
  const gField = fieldMap.get('g');
  const bField = fieldMap.get('b');
  const intensityField = fieldMap.get('intensity');
  const labelField = fieldMap.get('label') || fieldMap.get('classification');

  // Second pass: extract data
  for (let i = 0; i < points; i += stride) {
    let x: number, y: number, z: number;

    if (isColumnMajor) {
      x = readFieldColumnMajor(dataView, i, xField, points, fields);
      y = readFieldColumnMajor(dataView, i, yField, points, fields);
      z = readFieldColumnMajor(dataView, i, zField, points, fields);
    } else {
      const base = i * recordSize;
      x = readField(dataView, base + xField.offset, xField);
      y = readField(dataView, base + yField.offset, yField);
      z = readField(dataView, base + zField.offset, zField);
    }

    if (isNaN(x) || isNaN(y) || isNaN(z)) continue;
    if (useViewpoint) [x, y, z] = applyViewpoint(header.viewpoint, x, y, z);

    positions[outIdx * 3] = x - cx;
    positions[outIdx * 3 + 1] = z - cz;
    positions[outIdx * 3 + 2] = -(y - cy);

    // Color
    if (rgbField) {
      let packed: number;
      if (isColumnMajor) {
        packed = readFieldColumnMajor(dataView, i, rgbField, points, fields);
      } else {
        packed = readField(dataView, i * recordSize + rgbField.offset, rgbField);
      }
      // For float type, reinterpret bits; for int type, use directly
      if (rgbField.type === 'F') {
        const [r, g, b] = unpackRGB(packed);
        colors[outIdx * 3] = r;
        colors[outIdx * 3 + 1] = g;
        colors[outIdx * 3 + 2] = b;
      } else {
        colors[outIdx * 3] = ((packed >> 16) & 0xFF) / 255;
        colors[outIdx * 3 + 1] = ((packed >> 8) & 0xFF) / 255;
        colors[outIdx * 3 + 2] = (packed & 0xFF) / 255;
      }
      hasColor = true;
    } else if (rField && gField && bField) {
      if (isColumnMajor) {
        colors[outIdx * 3] = readFieldColumnMajor(dataView, i, rField, points, fields) / 255;
        colors[outIdx * 3 + 1] = readFieldColumnMajor(dataView, i, gField, points, fields) / 255;
        colors[outIdx * 3 + 2] = readFieldColumnMajor(dataView, i, bField, points, fields) / 255;
      } else {
        const base = i * recordSize;
        colors[outIdx * 3] = readField(dataView, base + rField.offset, rField) / 255;
        colors[outIdx * 3 + 1] = readField(dataView, base + gField.offset, gField) / 255;
        colors[outIdx * 3 + 2] = readField(dataView, base + bField.offset, bField) / 255;
      }
      hasColor = true;
    } else {
      colors[outIdx * 3] = 0.8;
      colors[outIdx * 3 + 1] = 0.8;
      colors[outIdx * 3 + 2] = 0.8;
    }

    if (intensityField) {
      intensities[outIdx] = isColumnMajor
        ? readFieldColumnMajor(dataView, i, intensityField, points, fields)
        : readField(dataView, i * recordSize + intensityField.offset, intensityField);
      hasIntensity = true;
    }

    if (labelField) {
      classifications[outIdx] = isColumnMajor
        ? readFieldColumnMajor(dataView, i, labelField, points, fields)
        : readField(dataView, i * recordSize + labelField.offset, labelField);
      hasClassification = true;
    }

    outIdx++;
  }

  return buildResult('PCD', outIdx, positions, colors, intensities, classifications,
    minX, minY, minZ, maxX, maxY, maxZ, cx, cy, cz, hasColor, hasIntensity, hasClassification);
}

/** Read a field value from column-major layout */
function readFieldColumnMajor(
  view: DataView, pointIdx: number, field: PCDField,
  totalPoints: number, allFields: PCDField[],
): number {
  // In column-major, each field's data for all points is stored contiguously.
  // Calculate the column offset: sum of (size * count * totalPoints) for all preceding fields
  let columnOffset = 0;
  for (const f of allFields) {
    if (f === field) break;
    columnOffset += f.size * f.count * totalPoints;
  }
  const offset = columnOffset + pointIdx * field.size;
  return readField(view, offset, field);
}

function buildResult(
  sig: string, outIdx: number,
  positions: Float32Array, colors: Float32Array,
  intensities: Float32Array, classifications: Float32Array,
  minX: number, minY: number, minZ: number,
  maxX: number, maxY: number, maxZ: number,
  cx: number, cy: number, cz: number,
  hasColor: boolean, hasIntensity: boolean, hasClassification: boolean,
): ParsedPointcloud {
  const header: LASHeader = {
    signature: sig,
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
    hasIntensity,
    hasClassification,
  };
}
