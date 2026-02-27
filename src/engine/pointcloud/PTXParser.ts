/**
 * Browser-side PTX file parser (Leica structured scan format).
 *
 * PTX files contain one or more scans, each with a header:
 *   columns
 *   rows
 *   scanner_x scanner_y scanner_z
 *   3x3 rotation matrix (3 lines of 3 values)
 *   4x4 transformation matrix (4 lines of 4 values)
 *
 * Followed by rows*columns point lines:
 *   X Y Z intensity [R G B]
 *
 * Invalid points have coordinates 0 0 0 (scanner origin) and are skipped.
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

interface ScanHeader {
  columns: number;
  rows: number;
  transform: number[]; // 4x4 column-major
}

function parseScanHeader(lines: string[], startIdx: number): { header: ScanHeader; nextLine: number } {
  let i = startIdx;

  // Skip empty lines
  while (i < lines.length && lines[i].trim() === '') i++;
  const columns = parseInt(lines[i++].trim(), 10);
  while (i < lines.length && lines[i].trim() === '') i++;
  const rows = parseInt(lines[i++].trim(), 10);

  if (isNaN(columns) || isNaN(rows) || columns <= 0 || rows <= 0) {
    throw new Error(`Invalid PTX scan header at line ${startIdx + 1}`);
  }

  // Scanner position (skip)
  while (i < lines.length && lines[i].trim() === '') i++;
  i++; // scanner x y z

  // 3x3 rotation matrix (skip — we use the 4x4 below)
  for (let r = 0; r < 3; r++) {
    while (i < lines.length && lines[i].trim() === '') i++;
    i++;
  }

  // 4x4 transformation matrix (row-major in file, store as flat array row-major)
  const transform: number[] = [];
  for (let r = 0; r < 4; r++) {
    while (i < lines.length && lines[i].trim() === '') i++;
    const parts = lines[i++].trim().split(/\s+/);
    for (let c = 0; c < 4; c++) {
      transform.push(parseFloat(parts[c]));
    }
  }

  return { header: { columns, rows, transform }, nextLine: i };
}

/** Apply 4x4 transform (row-major) to a point */
function transformPoint(t: number[], x: number, y: number, z: number): [number, number, number] {
  return [
    t[0] * x + t[1] * y + t[2] * z + t[3],
    t[4] * x + t[5] * y + t[6] * z + t[7],
    t[8] * x + t[9] * y + t[10] * z + t[11],
  ];
}

/** Check if transform is identity (skip transform for speed) */
function isIdentity(t: number[]): boolean {
  return t[0] === 1 && t[1] === 0 && t[2] === 0 && t[3] === 0 &&
         t[4] === 0 && t[5] === 1 && t[6] === 0 && t[7] === 0 &&
         t[8] === 0 && t[9] === 0 && t[10] === 1 && t[11] === 0 &&
         t[12] === 0 && t[13] === 0 && t[14] === 0 && t[15] === 1;
}

/** Normalize PTS-style intensity: can be 0–1, 0–255, or -2048..2047 */
function normalizeIntensity(raw: number): number {
  if (raw < 0) return (raw + 2048) / 4095;
  if (raw > 1) return raw / 255;
  return raw;
}

export function parsePTX(buffer: ArrayBuffer): ParsedPointcloud {
  const text = new TextDecoder().decode(buffer);
  const lines = text.split(/\r?\n/);

  // Collect all points across scans in two passes
  // First, discover scans and count points
  interface ScanInfo {
    header: ScanHeader;
    dataStart: number;
    hasColor: boolean;
  }
  const scans: ScanInfo[] = [];
  let totalPoints = 0;
  let lineIdx = 0;

  // Skip leading empty lines
  while (lineIdx < lines.length && lines[lineIdx].trim() === '') lineIdx++;

  while (lineIdx < lines.length) {
    // Try to parse a scan header
    const trimmed = lines[lineIdx].trim();
    if (trimmed === '') { lineIdx++; continue; }

    const { header, nextLine } = parseScanHeader(lines, lineIdx);
    const pointCount = header.columns * header.rows;
    const dataStart = nextLine;

    // Detect color from first data line
    let hasColor = false;
    if (dataStart < lines.length) {
      const sampleParts = lines[dataStart].trim().split(/\s+/);
      hasColor = sampleParts.length >= 7;
    }

    scans.push({ header, dataStart, hasColor });
    totalPoints += pointCount;
    lineIdx = dataStart + pointCount;
  }

  if (scans.length === 0 || totalPoints === 0) {
    throw new Error('PTX file has no scans or points');
  }

  // Apply downsampling if needed
  const maxPoints = 5_000_000;
  const stride = totalPoints > maxPoints ? Math.ceil(totalPoints / maxPoints) : 1;
  const estimatedCount = Math.ceil(totalPoints / stride);

  const positions = new Float32Array(estimatedCount * 3);
  const colors = new Float32Array(estimatedCount * 3);
  const intensities = new Float32Array(estimatedCount);
  const classifications = new Float32Array(estimatedCount);

  let anyColor = false;
  let anyIntensity = false;

  // First pass: bounds (sample across all scans)
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  for (const scan of scans) {
    const pointCount = scan.header.columns * scan.header.rows;
    const sampleStep = Math.max(1, Math.floor(pointCount / 5000));
    const identity = isIdentity(scan.header.transform);

    for (let i = 0; i < pointCount; i += sampleStep) {
      const li = scan.dataStart + i;
      if (li >= lines.length) break;
      const parts = lines[li].trim().split(/\s+/);
      if (parts.length < 3) continue;

      let x = parseFloat(parts[0]);
      let y = parseFloat(parts[1]);
      let z = parseFloat(parts[2]);
      if (isNaN(x) || isNaN(y) || isNaN(z)) continue;

      // Skip invalid points (origin)
      if (x === 0 && y === 0 && z === 0) continue;

      if (!identity) {
        [x, y, z] = transformPoint(scan.header.transform, x, y, z);
      }

      minX = Math.min(minX, x); maxX = Math.max(maxX, x);
      minY = Math.min(minY, y); maxY = Math.max(maxY, y);
      minZ = Math.min(minZ, z); maxZ = Math.max(maxZ, z);
    }
  }

  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  // Second pass: extract points
  let outIdx = 0;
  let globalPointIdx = 0;

  for (const scan of scans) {
    const pointCount = scan.header.columns * scan.header.rows;
    const identity = isIdentity(scan.header.transform);

    for (let i = 0; i < pointCount; i++) {
      if (globalPointIdx % stride !== 0) { globalPointIdx++; continue; }

      const li = scan.dataStart + i;
      if (li >= lines.length) { globalPointIdx++; continue; }

      const parts = lines[li].trim().split(/\s+/);
      if (parts.length < 3) { globalPointIdx++; continue; }

      let x = parseFloat(parts[0]);
      let y = parseFloat(parts[1]);
      let z = parseFloat(parts[2]);
      if (isNaN(x) || isNaN(y) || isNaN(z)) { globalPointIdx++; continue; }

      // Skip invalid points
      if (x === 0 && y === 0 && z === 0) { globalPointIdx++; continue; }

      if (!identity) {
        [x, y, z] = transformPoint(scan.header.transform, x, y, z);
      }

      // Z-up to Y-up
      positions[outIdx * 3] = x - cx;
      positions[outIdx * 3 + 1] = z - cz;
      positions[outIdx * 3 + 2] = -(y - cy);

      // Intensity (column 3)
      if (parts.length >= 4) {
        const rawI = parseFloat(parts[3]);
        if (!isNaN(rawI)) {
          intensities[outIdx] = normalizeIntensity(rawI);
          anyIntensity = true;
        }
      }

      // Color (columns 4, 5, 6)
      if (scan.hasColor && parts.length >= 7) {
        const r = parseFloat(parts[4]);
        const g = parseFloat(parts[5]);
        const b = parseFloat(parts[6]);
        colors[outIdx * 3] = r > 1 ? r / 255 : r;
        colors[outIdx * 3 + 1] = g > 1 ? g / 255 : g;
        colors[outIdx * 3 + 2] = b > 1 ? b / 255 : b;
        anyColor = true;
      } else {
        colors[outIdx * 3] = 0.8;
        colors[outIdx * 3 + 1] = 0.8;
        colors[outIdx * 3 + 2] = 0.8;
      }

      outIdx++;
      globalPointIdx++;
    }
  }

  const header: LASHeader = {
    signature: 'PTX',
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
    hasColor: anyColor,
    hasIntensity: anyIntensity,
    hasClassification: false,
  };
}
