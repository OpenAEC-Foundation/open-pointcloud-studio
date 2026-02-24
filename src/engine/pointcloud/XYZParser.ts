/**
 * Browser-side XYZ/TXT point cloud parser.
 *
 * Supports common text formats:
 *   X Y Z
 *   X Y Z R G B
 *   X Y Z I
 *   X Y Z I R G B
 *   X,Y,Z (comma-separated)
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

export function parseXYZ(buffer: ArrayBuffer): ParsedPointcloud {
  const text = new TextDecoder().decode(buffer);
  const lines = text.split(/\r?\n/).filter((l) => l.trim().length > 0);

  if (lines.length === 0) throw new Error('XYZ file is empty');

  // Skip comment lines at the start
  let startLine = 0;
  while (startLine < lines.length && (lines[startLine].startsWith('#') || lines[startLine].startsWith('//'))) {
    startLine++;
  }

  // Detect delimiter (space, comma, tab, semicolon)
  const sampleLine = lines[startLine].trim();
  const delimiter = sampleLine.includes(',') ? /[,\s]+/ :
                    sampleLine.includes(';') ? /[;\s]+/ :
                    sampleLine.includes('\t') ? /\t+/ : /\s+/;

  const sampleParts = sampleLine.split(delimiter);
  const numCols = sampleParts.length;

  // Try to detect if first row is a header
  if (isNaN(parseFloat(sampleParts[0]))) {
    startLine++;
  }

  const hasIntensity = numCols === 4 || numCols >= 7;
  const hasColor = numCols === 6 || numCols >= 7;

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

  // First pass: bounds (sample)
  const sampleStep = Math.max(1, Math.floor(totalLines / 10000));
  for (let i = startLine; i < lines.length; i += sampleStep) {
    const parts = lines[i].trim().split(delimiter);
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
    const parts = lines[i].trim().split(delimiter);
    if (parts.length < 3) continue;

    const x = parseFloat(parts[0]);
    const y = parseFloat(parts[1]);
    const z = parseFloat(parts[2]);
    if (isNaN(x) || isNaN(y) || isNaN(z)) continue;

    positions[outIdx * 3] = x - cx;
    positions[outIdx * 3 + 1] = z - cz;
    positions[outIdx * 3 + 2] = -(y - cy);

    if (hasIntensity) {
      intensities[outIdx] = Math.min(1, Math.max(0, parseFloat(parts[3]) / 255));
    }

    if (hasColor) {
      const rIdx = hasIntensity ? 4 : 3;
      const r = parseFloat(parts[rIdx]);
      const g = parseFloat(parts[rIdx + 1]);
      const b = parseFloat(parts[rIdx + 2]);
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

  const header: LASHeader = {
    signature: 'XYZ',
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
    hasClassification: false,
  };
}
