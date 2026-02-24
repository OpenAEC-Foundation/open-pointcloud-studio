/**
 * Browser-side LAZ file parser using laz-perf WASM.
 *
 * Decompresses LAZ files and extracts point data the same way as LASParser.
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

/** Point formats that include RGB color data */
const FORMATS_WITH_RGB = new Set([2, 3, 5, 7, 8, 10]);

function getRGBOffset(format: number): number {
  switch (format) {
    case 2: return 20;
    case 3: return 28;
    case 5: return 28;
    case 7: return 30;
    case 8: return 30;
    case 10: return 30;
    default: return -1;
  }
}

function getClassificationOffset(format: number): number {
  return format >= 6 ? 16 : 15;
}

export async function parseLAZ(buffer: ArrayBuffer): Promise<ParsedPointcloud> {
  // Dynamically import laz-perf
  const { create } = await import('laz-perf');
  const lp = await create();

  const view = new DataView(buffer);

  // Validate signature
  const sig = String.fromCharCode(view.getUint8(0), view.getUint8(1), view.getUint8(2), view.getUint8(3));
  if (sig !== 'LASF') {
    throw new Error(`Invalid LAZ file: expected "LASF" signature, got "${sig}"`);
  }

  // Read header (same as LAS)
  const versionMajor = view.getUint8(24);
  const versionMinor = view.getUint8(25);
  const headerSize = view.getUint16(94, true);
  const offsetToPointData = view.getUint32(96, true);
  const pointDataFormat = view.getUint8(104);
  const pointDataRecordLength = view.getUint16(105, true);
  const legacyPointCount = view.getUint32(107, true);

  let numberOfPoints = legacyPointCount;
  if (versionMajor === 1 && versionMinor >= 4 && legacyPointCount === 0) {
    const lo = view.getUint32(247, true);
    const hi = view.getUint32(251, true);
    numberOfPoints = hi * 0x100000000 + lo;
  }

  const scaleX = view.getFloat64(131, true);
  const scaleY = view.getFloat64(139, true);
  const scaleZ = view.getFloat64(147, true);
  const offsetX = view.getFloat64(155, true);
  const offsetY = view.getFloat64(163, true);
  const offsetZ = view.getFloat64(171, true);

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

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  // Use laz-perf to decompress
  const filePtr = lp._malloc(buffer.byteLength);
  const fileData = new Uint8Array(buffer);
  lp.HEAPU8.set(fileData, filePtr);

  const laszip = new lp.LASZip();
  laszip.open(filePtr, buffer.byteLength);

  const lazPointCount = laszip.getCount();
  const lazPointFormat = laszip.getPointFormat();
  const lazPointLength = laszip.getPointLength();

  // Use the LAZ decompressed count if available
  const totalPoints = lazPointCount > 0 ? lazPointCount : numberOfPoints;

  // Limit points for browser performance
  const maxPoints = 5_000_000;
  const stride = totalPoints > maxPoints ? Math.ceil(totalPoints / maxPoints) : 1;
  const actualCount = Math.ceil(totalPoints / stride);

  const positions = new Float32Array(actualCount * 3);
  const colors = new Float32Array(actualCount * 3);
  const intensities = new Float32Array(actualCount);
  const classifications = new Float32Array(actualCount);

  // The actual point format might differ from header if LAZ rewrites it
  const actualFormat = lazPointFormat >= 0 ? (lazPointFormat <= 10 ? lazPointFormat : pointDataFormat) : pointDataFormat;
  const hasColor = FORMATS_WITH_RGB.has(actualFormat);
  const rgbOffset = getRGBOffset(actualFormat);
  const classOffset = getClassificationOffset(actualFormat);

  // Allocate buffer for one decompressed point
  const pointPtr = lp._malloc(lazPointLength);
  const pointBuf = new DataView(lp.HEAPU8.buffer, pointPtr, lazPointLength);

  let outIdx = 0;

  for (let i = 0; i < totalPoints; i++) {
    laszip.getPoint(pointPtr);

    if (i % stride !== 0) continue;

    const rawX = pointBuf.getInt32(0, true);
    const rawY = pointBuf.getInt32(4, true);
    const rawZ = pointBuf.getInt32(8, true);

    const x = rawX * scaleX + offsetX - cx;
    const y = rawY * scaleY + offsetY - cy;
    const z = rawZ * scaleZ + offsetZ - cz;

    positions[outIdx * 3] = x;
    positions[outIdx * 3 + 1] = z;
    positions[outIdx * 3 + 2] = -y;

    const intensity = pointBuf.getUint16(12, true);
    intensities[outIdx] = intensity / 65535;

    if (classOffset < lazPointLength) {
      classifications[outIdx] = pointBuf.getUint8(classOffset);
    }

    if (hasColor && rgbOffset >= 0 && rgbOffset + 6 <= lazPointLength) {
      const r = pointBuf.getUint16(rgbOffset, true);
      const g = pointBuf.getUint16(rgbOffset + 2, true);
      const b = pointBuf.getUint16(rgbOffset + 4, true);
      colors[outIdx * 3] = r > 255 ? r / 65535 : r / 255;
      colors[outIdx * 3 + 1] = g > 255 ? g / 65535 : g / 255;
      colors[outIdx * 3 + 2] = b > 255 ? b / 65535 : b / 255;
    } else {
      colors[outIdx * 3] = 0.8;
      colors[outIdx * 3 + 1] = 0.8;
      colors[outIdx * 3 + 2] = 0.8;
    }

    outIdx++;
  }

  // Cleanup WASM memory
  laszip.delete();
  lp._free(pointPtr);
  lp._free(filePtr);

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
