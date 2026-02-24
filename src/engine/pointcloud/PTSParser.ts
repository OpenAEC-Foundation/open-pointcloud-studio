/**
 * Browser-side PTS file parser.
 *
 * PTS (Leica) is a text-based format. Each line contains space-separated values:
 *   X Y Z [Intensity] [R G B]
 *
 * The first line may contain the point count.
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

export function parsePTS(buffer: ArrayBuffer): ParsedPointcloud {
  const text = new TextDecoder().decode(buffer);
  const lines = text.split(/\r?\n/).filter((l) => l.trim().length > 0);

  if (lines.length === 0) {
    throw new Error('PTS file is empty');
  }

  // First line might be the point count
  let startLine = 0;
  const firstParts = lines[0].trim().split(/\s+/);
  if (firstParts.length === 1 && !isNaN(Number(firstParts[0]))) {
    startLine = 1;
  }

  // Detect format from first data line
  const sampleParts = lines[startLine].trim().split(/\s+/);
  const numCols = sampleParts.length;
  // Possible formats:
  // 3: X Y Z
  // 4: X Y Z I
  // 6: X Y Z R G B
  // 7: X Y Z I R G B
  const hasIntensity = numCols === 4 || numCols >= 7;
  const hasColor = numCols >= 6;

  // Limit for browser performance
  const maxPoints = 5_000_000;
  const totalLines = lines.length - startLine;
  const stride = totalLines > maxPoints ? Math.ceil(totalLines / maxPoints) : 1;
  const estimatedCount = Math.ceil(totalLines / stride);

  const positions = new Float32Array(estimatedCount * 3);
  const colors = new Float32Array(estimatedCount * 3);
  const intensities = new Float32Array(estimatedCount);
  const classifications = new Float32Array(estimatedCount);

  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  // First pass: find bounds (sample)
  const sampleStep = Math.max(1, Math.floor(totalLines / 10000));
  for (let i = startLine; i < lines.length; i += sampleStep) {
    const parts = lines[i].trim().split(/\s+/);
    if (parts.length < 3) continue;
    const x = parseFloat(parts[0]);
    const y = parseFloat(parts[1]);
    const z = parseFloat(parts[2]);
    if (isNaN(x) || isNaN(y) || isNaN(z)) continue;
    minX = Math.min(minX, x); maxX = Math.max(maxX, x);
    minY = Math.min(minY, y); maxY = Math.max(maxY, y);
    minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  let outIdx = 0;

  for (let i = startLine; i < lines.length; i += stride) {
    const parts = lines[i].trim().split(/\s+/);
    if (parts.length < 3) continue;

    const x = parseFloat(parts[0]);
    const y = parseFloat(parts[1]);
    const z = parseFloat(parts[2]);
    if (isNaN(x) || isNaN(y) || isNaN(z)) continue;

    // Z-up to Y-up
    positions[outIdx * 3] = x - cx;
    positions[outIdx * 3 + 1] = z - cz;
    positions[outIdx * 3 + 2] = -(y - cy);

    if (hasIntensity) {
      const rawI = parseFloat(parts[3]);
      // PTS intensity can be -2048..2047 or 0..1 or 0..255
      intensities[outIdx] = rawI < 0 ? (rawI + 2048) / 4095 : rawI > 1 ? rawI / 255 : rawI;
    }

    if (hasColor) {
      const rIdx = hasIntensity ? 4 : 3;
      const r = parseFloat(parts[rIdx]);
      const g = parseFloat(parts[rIdx + 1]);
      const b = parseFloat(parts[rIdx + 2]);
      colors[outIdx * 3] = r / 255;
      colors[outIdx * 3 + 1] = g / 255;
      colors[outIdx * 3 + 2] = b / 255;
    } else {
      colors[outIdx * 3] = 0.8;
      colors[outIdx * 3 + 1] = 0.8;
      colors[outIdx * 3 + 2] = 0.8;
    }

    outIdx++;
  }

  const finalPositions = positions.slice(0, outIdx * 3);
  const finalColors = colors.slice(0, outIdx * 3);
  const finalIntensities = intensities.slice(0, outIdx);
  const finalClassifications = classifications.slice(0, outIdx);

  const header: LASHeader = {
    signature: 'PTS',
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
    positions: finalPositions,
    colors: finalColors,
    intensities: finalIntensities,
    classifications: finalClassifications,
    center: [cx, cy, cz],
    hasColor,
    hasIntensity,
    hasClassification: false,
  };
}
