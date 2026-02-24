/**
 * Browser-side LAS file parser.
 *
 * Reads uncompressed LAS files (1.0–1.4) from an ArrayBuffer and extracts
 * positions, colors, intensities, and classifications.
 *
 * LAZ (compressed) files are NOT supported — those require the Tauri backend.
 */

export interface LASHeader {
  signature: string;
  versionMajor: number;
  versionMinor: number;
  headerSize: number;
  offsetToPointData: number;
  pointDataFormat: number;
  pointDataRecordLength: number;
  numberOfPoints: number;
  scaleX: number;
  scaleY: number;
  scaleZ: number;
  offsetX: number;
  offsetY: number;
  offsetZ: number;
  minX: number;
  minY: number;
  minZ: number;
  maxX: number;
  maxY: number;
  maxZ: number;
}

export interface ParsedPointcloud {
  header: LASHeader;
  /** Float32 XYZ positions relative to bounds center */
  positions: Float32Array;
  /** Float32 RGB colors normalized 0–1 (or white if no color) */
  colors: Float32Array;
  /** Float32 intensities normalized 0–1 */
  intensities: Float32Array;
  /** Float32 classification codes */
  classifications: Float32Array;
  /** Center of bounds [x, y, z] for world offset */
  center: [number, number, number];
  hasColor: boolean;
  hasIntensity: boolean;
  hasClassification: boolean;
  /** Optional triangle face indices (for mesh formats like OBJ) */
  indices?: Uint32Array;
}

/** Point formats that include RGB color data */
const FORMATS_WITH_RGB = new Set([2, 3, 5, 7, 8, 10]);

/** RGB offset within point record for each format */
function getRGBOffset(format: number): number {
  switch (format) {
    case 2: return 20;   // Format 0 (20 bytes) + RGB
    case 3: return 28;   // Format 1 (28 bytes) + RGB
    case 5: return 28;   // Format 1 (28 bytes) + RGB (via VLR)
    case 7: return 30;   // Format 6 (30 bytes) + RGB
    case 8: return 30;   // Format 6 (30 bytes) + RGB + NIR
    case 10: return 30;  // Format 6 (30 bytes) + RGB + NIR + Wave
    default: return -1;
  }
}

/** Classification byte offset within point record */
function getClassificationOffset(format: number): number {
  // Formats 0–5: classification at byte 15
  // Formats 6–10: classification at byte 16
  return format >= 6 ? 16 : 15;
}

export function parseLAS(buffer: ArrayBuffer): ParsedPointcloud {
  const view = new DataView(buffer);

  // Validate signature
  const sig = String.fromCharCode(view.getUint8(0), view.getUint8(1), view.getUint8(2), view.getUint8(3));
  if (sig !== 'LASF') {
    throw new Error(`Invalid LAS file: expected "LASF" signature, got "${sig}"`);
  }

  const versionMajor = view.getUint8(24);
  const versionMinor = view.getUint8(25);
  const headerSize = view.getUint16(94, true);
  const offsetToPointData = view.getUint32(96, true);
  const pointDataFormat = view.getUint8(104);
  const pointDataRecordLength = view.getUint16(105, true);
  const legacyPointCount = view.getUint32(107, true);

  // LAS 1.4 has 64-bit point count at offset 247
  let numberOfPoints = legacyPointCount;
  if (versionMajor === 1 && versionMinor >= 4 && legacyPointCount === 0) {
    // Read as two 32-bit values (JS doesn't handle uint64 well)
    const lo = view.getUint32(247, true);
    const hi = view.getUint32(251, true);
    numberOfPoints = hi * 0x100000000 + lo;
  }

  // Scale & offset (doubles at fixed positions)
  const scaleX = view.getFloat64(131, true);
  const scaleY = view.getFloat64(139, true);
  const scaleZ = view.getFloat64(147, true);
  const offsetX = view.getFloat64(155, true);
  const offsetY = view.getFloat64(163, true);
  const offsetZ = view.getFloat64(171, true);

  // Bounds
  const maxX = view.getFloat64(179, true);
  const minX = view.getFloat64(187, true);
  const maxY = view.getFloat64(195, true);
  const minY = view.getFloat64(203, true);
  const maxZ = view.getFloat64(211, true);
  const minZ = view.getFloat64(219, true);

  const header: LASHeader = {
    signature: sig,
    versionMajor, versionMinor,
    headerSize, offsetToPointData,
    pointDataFormat, pointDataRecordLength,
    numberOfPoints,
    scaleX, scaleY, scaleZ,
    offsetX, offsetY, offsetZ,
    minX, minY, minZ,
    maxX, maxY, maxZ,
  };

  // Center of bounds for coordinate offset
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  // Limit points for browser performance
  const maxPoints = 5_000_000;
  const stride = numberOfPoints > maxPoints ? Math.ceil(numberOfPoints / maxPoints) : 1;
  const actualCount = Math.ceil(numberOfPoints / stride);

  const positions = new Float32Array(actualCount * 3);
  const colors = new Float32Array(actualCount * 3);
  const intensities = new Float32Array(actualCount);
  const classifications = new Float32Array(actualCount);

  const hasColor = FORMATS_WITH_RGB.has(pointDataFormat);
  const rgbOffset = getRGBOffset(pointDataFormat);
  const classOffset = getClassificationOffset(pointDataFormat);

  let outIdx = 0;

  for (let i = 0; i < numberOfPoints; i += stride) {
    const offset = offsetToPointData + i * pointDataRecordLength;

    // Ensure we don't read past buffer
    if (offset + pointDataRecordLength > buffer.byteLength) break;

    // XYZ as int32, then apply scale + offset
    const rawX = view.getInt32(offset, true);
    const rawY = view.getInt32(offset + 4, true);
    const rawZ = view.getInt32(offset + 8, true);

    const x = rawX * scaleX + offsetX - cx;
    const y = rawY * scaleY + offsetY - cy;
    const z = rawZ * scaleZ + offsetZ - cz;

    positions[outIdx * 3] = x;
    positions[outIdx * 3 + 1] = z; // Z-up to Y-up conversion
    positions[outIdx * 3 + 2] = -y;

    // Intensity at byte 12 (uint16)
    const intensity = view.getUint16(offset + 12, true);
    intensities[outIdx] = intensity / 65535;

    // Classification
    classifications[outIdx] = view.getUint8(offset + classOffset);

    // RGB color
    if (hasColor && rgbOffset >= 0) {
      // RGB values are uint16 in LAS (0–65535)
      const r = view.getUint16(offset + rgbOffset, true);
      const g = view.getUint16(offset + rgbOffset + 2, true);
      const b = view.getUint16(offset + rgbOffset + 4, true);
      // Detect 8-bit vs 16-bit colors: if max values are ≤255, treat as 8-bit
      colors[outIdx * 3] = r > 255 ? r / 65535 : r / 255;
      colors[outIdx * 3 + 1] = g > 255 ? g / 65535 : g / 255;
      colors[outIdx * 3 + 2] = b > 255 ? b / 65535 : b / 255;
    } else {
      // No color data — use white
      colors[outIdx * 3] = 0.8;
      colors[outIdx * 3 + 1] = 0.8;
      colors[outIdx * 3 + 2] = 0.8;
    }

    outIdx++;
  }

  // Trim arrays if we read fewer points than expected
  const finalPositions = outIdx < actualCount ? positions.slice(0, outIdx * 3) : positions;
  const finalColors = outIdx < actualCount ? colors.slice(0, outIdx * 3) : colors;
  const finalIntensities = outIdx < actualCount ? intensities.slice(0, outIdx) : intensities;
  const finalClassifications = outIdx < actualCount ? classifications.slice(0, outIdx) : classifications;

  return {
    header,
    positions: finalPositions,
    colors: finalColors,
    intensities: finalIntensities,
    classifications: finalClassifications,
    center: [cx, cy, cz],
    hasColor,
    hasIntensity: true,
    hasClassification: true,
  };
}
