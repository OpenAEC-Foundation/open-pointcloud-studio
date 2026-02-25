/**
 * Browser-side E57 file parser.
 *
 * Reads ASTM E2807 E57 files from an ArrayBuffer and extracts
 * positions, colors, and intensities. Supports:
 *   - Page-based binary layout with CRC stripping
 *   - XML metadata parsing for scan descriptors
 *   - Bitpacked bytestream decoding (float, scaledInteger, integer)
 *   - Multi-scan files with pose transforms (quaternion + translation)
 *   - Spherical coordinate conversion
 *   - Z-up to Y-up conversion + centering
 *
 * Limitations:
 *   - Compressed E57 files (non-empty codecs) are NOT supported
 *   - CRC checksums are not validated (for speed)
 */

import type { ParsedPointcloud, LASHeader } from './LASParser';

// ── Types ──────────────────────────────────────────────────────────────

interface E57FileHeader {
  signature: string;
  majorVersion: number;
  minorVersion: number;
  filePhysicalLength: number;
  xmlPhysicalOffset: number;
  xmlLogicalLength: number;
  pageSize: number;
}

interface E57FieldDef {
  name: string;
  type: 'float' | 'scaledInteger' | 'integer';
  precision: number; // 32 or 64 for float; bit count for integer
  minimum: number;
  maximum: number;
  scale: number;
  offset: number;
}

interface E57ScanDescriptor {
  name: string;
  pointCount: number;
  binaryPhysicalOffset: number;
  prototype: E57FieldDef[];
  translation?: [number, number, number];
  rotation?: [number, number, number, number]; // quaternion w,x,y,z
}

// ── Header ─────────────────────────────────────────────────────────────

function readE57Header(view: DataView): E57FileHeader {
  const sig = String.fromCharCode(
    view.getUint8(0), view.getUint8(1), view.getUint8(2), view.getUint8(3),
    view.getUint8(4), view.getUint8(5), view.getUint8(6), view.getUint8(7),
  );
  if (sig !== 'ASTM-E57') {
    throw new Error(`Invalid E57 file: expected "ASTM-E57" signature, got "${sig}"`);
  }

  const majorVersion = view.getUint32(8, true);
  const minorVersion = view.getUint32(12, true);

  // uint64 fields read as lo + hi * 2^32
  const fileLenLo = view.getUint32(16, true);
  const fileLenHi = view.getUint32(20, true);
  const filePhysicalLength = fileLenLo + fileLenHi * 0x100000000;

  const xmlOffLo = view.getUint32(24, true);
  const xmlOffHi = view.getUint32(28, true);
  const xmlPhysicalOffset = xmlOffLo + xmlOffHi * 0x100000000;

  const xmlLenLo = view.getUint32(32, true);
  const xmlLenHi = view.getUint32(36, true);
  const xmlLogicalLength = xmlLenLo + xmlLenHi * 0x100000000;

  const pageSizeLo = view.getUint32(40, true);
  const pageSize = pageSizeLo; // typically 1024

  return {
    signature: sig,
    majorVersion, minorVersion,
    filePhysicalLength,
    xmlPhysicalOffset,
    xmlLogicalLength,
    pageSize,
  };
}

// ── Paged data reading ─────────────────────────────────────────────────

/**
 * Read logical data from E57's page-based layout.
 * Each page is `pageSize` bytes: (pageSize - 4) data bytes + 4 byte CRC.
 * CRC is not validated for performance.
 */
function readPagedData(
  buffer: ArrayBuffer,
  physOffset: number,
  logLength: number,
  pageSize: number,
): Uint8Array {
  const dataPerPage = pageSize - 4;
  const result = new Uint8Array(logLength);
  let written = 0;
  let currentPhys = physOffset;

  while (written < logLength) {
    const chunkSize = Math.min(dataPerPage, logLength - written);
    const src = new Uint8Array(buffer, currentPhys, chunkSize);
    result.set(src, written);
    written += chunkSize;
    currentPhys += pageSize; // skip to next page (past CRC)
  }

  return result;
}

// ── XML parsing ────────────────────────────────────────────────────────

function parseE57Xml(xmlString: string): E57ScanDescriptor[] {
  const parser = new DOMParser();
  const doc = parser.parseFromString(xmlString, 'application/xml');
  const scans: E57ScanDescriptor[] = [];

  // Find data3D vector children
  const data3D = doc.querySelector('data3D');
  if (!data3D) return scans;

  const vectorChildren = data3D.querySelectorAll('vectorChild');

  for (const child of vectorChildren) {
    const pointsEl = child.querySelector('points[type="CompressedVector"]');
    if (!pointsEl) continue;

    const fileOffset = parseInt(pointsEl.getAttribute('fileOffset') || '0', 10);
    const recordCount = parseInt(pointsEl.getAttribute('recordCount') || '0', 10);
    if (recordCount === 0) continue;

    // Check for codecs — we only support uncompressed
    const codecsEl = pointsEl.querySelector('codecs');
    if (codecsEl && codecsEl.children.length > 0) {
      throw new Error(
        'This E57 file uses compressed data (codecs), which is not yet supported.\n' +
        'Please convert to LAS/LAZ using CloudCompare.'
      );
    }

    // Parse prototype fields
    const prototype: E57FieldDef[] = [];
    const prototypeEl = pointsEl.querySelector('prototype');
    if (prototypeEl) {
      for (const fieldEl of prototypeEl.children) {
        const name = fieldEl.tagName;
        const type = fieldEl.getAttribute('type') as 'float' | 'scaledInteger' | 'integer' | null;
        if (!type) continue;

        const precision = parseFloat(fieldEl.getAttribute('precision') || (type === 'float' ? '64' : '0'));
        const minimum = parseFloat(fieldEl.getAttribute('minimum') || '0');
        const maximum = parseFloat(fieldEl.getAttribute('maximum') || '0');
        const scale = parseFloat(fieldEl.getAttribute('scale') || '1');
        const offset = parseFloat(fieldEl.getAttribute('offset') || '0');

        prototype.push({ name, type, precision, minimum, maximum, scale, offset });
      }
    }

    // Parse pose (optional)
    let translation: [number, number, number] | undefined;
    let rotation: [number, number, number, number] | undefined;

    const poseEl = child.querySelector('pose');
    if (poseEl) {
      const transEl = poseEl.querySelector('translation');
      if (transEl) {
        const tx = parseFloat(transEl.querySelector('x')?.textContent || '0');
        const ty = parseFloat(transEl.querySelector('y')?.textContent || '0');
        const tz = parseFloat(transEl.querySelector('z')?.textContent || '0');
        translation = [tx, ty, tz];
      }
      const rotEl = poseEl.querySelector('rotation');
      if (rotEl) {
        const w = parseFloat(rotEl.querySelector('w')?.textContent || '1');
        const x = parseFloat(rotEl.querySelector('x')?.textContent || '0');
        const y = parseFloat(rotEl.querySelector('y')?.textContent || '0');
        const z = parseFloat(rotEl.querySelector('z')?.textContent || '0');
        rotation = [w, x, y, z];
      }
    }

    const nameEl = child.querySelector('name');
    scans.push({
      name: nameEl?.textContent || `scan_${scans.length}`,
      pointCount: recordCount,
      binaryPhysicalOffset: fileOffset,
      prototype,
      translation,
      rotation,
    });
  }

  return scans;
}

// ── BitReader ──────────────────────────────────────────────────────────

class BitReader {
  private data: Uint8Array;
  private bytePos: number;
  private bitPos: number; // 0-7, bits consumed in current byte

  constructor(data: Uint8Array, offset = 0) {
    this.data = data;
    this.bytePos = offset;
    this.bitPos = 0;
  }

  /** Read up to 32 bits unsigned, LSB-first */
  readBits(count: number): number {
    if (count === 0) return 0;
    let result = 0;
    let bitsRead = 0;

    while (bitsRead < count) {
      if (this.bytePos >= this.data.length) return result;
      const available = 8 - this.bitPos;
      const need = count - bitsRead;
      const take = Math.min(available, need);
      const mask = (1 << take) - 1;
      const bits = (this.data[this.bytePos] >> this.bitPos) & mask;
      result |= bits << bitsRead;
      bitsRead += take;
      this.bitPos += take;
      if (this.bitPos >= 8) {
        this.bitPos = 0;
        this.bytePos++;
      }
    }

    return result;
  }

  readFloat32(): number {
    // Align to byte boundary first
    if (this.bitPos !== 0) {
      this.bitPos = 0;
      this.bytePos++;
    }
    if (this.bytePos + 4 > this.data.length) return 0;
    const dv = new DataView(this.data.buffer, this.data.byteOffset + this.bytePos, 4);
    this.bytePos += 4;
    return dv.getFloat32(0, true);
  }

  readFloat64(): number {
    if (this.bitPos !== 0) {
      this.bitPos = 0;
      this.bytePos++;
    }
    if (this.bytePos + 8 > this.data.length) return 0;
    const dv = new DataView(this.data.buffer, this.data.byteOffset + this.bytePos, 8);
    this.bytePos += 8;
    return dv.getFloat64(0, true);
  }

  get position(): number {
    return this.bytePos;
  }
}

// ── Binary section decoding ────────────────────────────────────────────

function bitsForRange(min: number, max: number): number {
  if (max <= min) return 0;
  const range = max - min;
  return Math.ceil(Math.log2(range + 1));
}

/**
 * Decode a binary section for a scan descriptor.
 * Returns arrays of decoded field values indexed by field name.
 */
function decodeBinarySection(
  buffer: ArrayBuffer,
  scan: E57ScanDescriptor,
  pageSize: number,
): Map<string, Float64Array> {
  const result = new Map<string, Float64Array>();
  const fieldCount = scan.prototype.length;

  for (const field of scan.prototype) {
    result.set(field.name, new Float64Array(scan.pointCount));
  }

  // Read paged data for this binary section
  // First 4 bytes of the section tell us the data packet header
  // E57 binary sections have data packets; for uncompressed, there's a simple structure
  const dataPerPage = pageSize - 4;

  // We need to read through the binary section page by page
  // Each data packet: 1 byte type, then bytestream data interleaved per field
  let physPos = scan.binaryPhysicalOffset;

  // Collect all binary data from pages
  // We don't know the exact logical length, so read enough pages for all points
  // Estimate: calculate bits per point across all fields
  let bitsPerPoint = 0;
  for (const field of scan.prototype) {
    if (field.type === 'float') {
      bitsPerPoint += field.precision; // 32 or 64
    } else if (field.type === 'scaledInteger' || field.type === 'integer') {
      bitsPerPoint += bitsForRange(field.minimum, field.maximum);
    }
  }

  // Generous estimate for data size (plus packet headers)
  const estimatedBytes = Math.ceil((bitsPerPoint * scan.pointCount) / 8) + fieldCount * 64 + 1024;
  const pagesToRead = Math.ceil(estimatedBytes / dataPerPage) + 2;
  const maxPhysRead = Math.min(pagesToRead * pageSize, buffer.byteLength - physPos);

  // Read raw pages, stripping CRC
  const rawData = new Uint8Array(Math.ceil(maxPhysRead / pageSize) * dataPerPage);
  let rawWritten = 0;
  let currentPhys = physPos;

  while (rawWritten < rawData.length && currentPhys < buffer.byteLength) {
    const remaining = buffer.byteLength - currentPhys;
    const chunkSize = Math.min(dataPerPage, remaining, rawData.length - rawWritten);
    const src = new Uint8Array(buffer, currentPhys, chunkSize);
    rawData.set(src, rawWritten);
    rawWritten += chunkSize;
    currentPhys += pageSize;
  }

  // Now parse data packets from rawData
  let offset = 0;
  let pointsDecoded = 0;

  while (pointsDecoded < scan.pointCount && offset < rawWritten) {
    // Packet header: 1 byte type
    const packetType = rawData[offset];
    offset++;

    if (packetType === 0) {
      // Index packet — skip (8 bytes of index data per entry)
      // Not common in point data, skip 15 bytes
      offset += 15;
      continue;
    }

    if (packetType !== 1) {
      // Unknown packet type, try to skip
      break;
    }

    // Data packet (type 1):
    // 2 bytes: packet length (LE, in bytes including header)
    // For each bytestream: data follows
    if (offset + 1 >= rawWritten) break;
    const packetLength = rawData[offset] | (rawData[offset + 1] << 8);
    offset += 2;

    // 2 bytes: bytestream count
    if (offset + 1 >= rawWritten) break;
    const bytestreamCount = rawData[offset] | (rawData[offset + 1] << 8);
    offset += 2;

    if (bytestreamCount !== fieldCount) {
      // Mismatch — skip this packet
      offset += packetLength - 6; // already read 1+2+2 = 5 bytes, packet includes header
      continue;
    }

    // Per bytestream: 2 bytes length
    const streamLengths: number[] = [];
    for (let s = 0; s < bytestreamCount; s++) {
      if (offset + 1 >= rawWritten) break;
      const len = rawData[offset] | (rawData[offset + 1] << 8);
      streamLengths.push(len);
      offset += 2;
    }

    // Now decode each bytestream
    for (let s = 0; s < fieldCount; s++) {
      const field = scan.prototype[s];
      const streamLen = streamLengths[s] || 0;
      const streamStart = offset;
      const arr = result.get(field.name)!;

      if (field.type === 'float') {
        const reader = new BitReader(rawData, streamStart);
        const bytesPerVal = field.precision === 32 ? 4 : 8;
        const valsInStream = Math.min(
          Math.floor(streamLen / bytesPerVal),
          scan.pointCount - pointsDecoded
        );

        for (let p = 0; p < valsInStream; p++) {
          arr[pointsDecoded + p] = field.precision === 32
            ? reader.readFloat32()
            : reader.readFloat64();
        }
      } else {
        // integer or scaledInteger
        const bits = bitsForRange(field.minimum, field.maximum);
        const reader = new BitReader(rawData, streamStart);

        if (bits === 0) {
          // Constant value
          const valsInStream = Math.min(
            streamLen > 0 ? scan.pointCount - pointsDecoded : 0,
            scan.pointCount - pointsDecoded
          );
          const constVal = field.type === 'scaledInteger'
            ? field.minimum * field.scale + field.offset
            : field.minimum;
          for (let p = 0; p < valsInStream; p++) {
            arr[pointsDecoded + p] = constVal;
          }
        } else {
          const valsInStream = Math.min(
            Math.floor((streamLen * 8) / bits),
            scan.pointCount - pointsDecoded
          );
          for (let p = 0; p < valsInStream; p++) {
            const raw = reader.readBits(bits) + field.minimum;
            arr[pointsDecoded + p] = field.type === 'scaledInteger'
              ? raw * field.scale + field.offset
              : raw;
          }
        }
      }

      offset += streamLen;
    }

    // Count points decoded in this packet — use first field's stream to determine
    const firstField = scan.prototype[0];
    const firstStreamLen = streamLengths[0] || 0;
    let pointsInPacket: number;
    if (firstField.type === 'float') {
      const bytesPerVal = firstField.precision === 32 ? 4 : 8;
      pointsInPacket = Math.floor(firstStreamLen / bytesPerVal);
    } else {
      const bits = bitsForRange(firstField.minimum, firstField.maximum);
      pointsInPacket = bits === 0
        ? (firstStreamLen > 0 ? scan.pointCount - pointsDecoded : 0)
        : Math.floor((firstStreamLen * 8) / bits);
    }
    pointsInPacket = Math.min(pointsInPacket, scan.pointCount - pointsDecoded);
    pointsDecoded += pointsInPacket;
  }

  return result;
}

// ── Quaternion rotation ────────────────────────────────────────────────

function rotateByQuaternion(
  x: number, y: number, z: number,
  w: number, qx: number, qy: number, qz: number,
): [number, number, number] {
  // q * v * q^-1 (for unit quaternion, q^-1 = conjugate)
  const tx = 2 * (qy * z - qz * y);
  const ty = 2 * (qz * x - qx * z);
  const tz = 2 * (qx * y - qy * x);
  return [
    x + w * tx + (qy * tz - qz * ty),
    y + w * ty + (qz * tx - qx * tz),
    z + w * tz + (qx * ty - qy * tx),
  ];
}

// ── Main parser ────────────────────────────────────────────────────────

export function parseE57(buffer: ArrayBuffer): ParsedPointcloud {
  const view = new DataView(buffer);
  const header = readE57Header(view);

  // Read XML section
  const xmlBytes = readPagedData(
    buffer,
    header.xmlPhysicalOffset,
    header.xmlLogicalLength,
    header.pageSize,
  );
  const xmlString = new TextDecoder('utf-8').decode(xmlBytes);
  const scans = parseE57Xml(xmlString);

  if (scans.length === 0) {
    throw new Error('No point cloud data found in E57 file.');
  }

  // Count total points and determine stride for sampling
  const totalPoints = scans.reduce((sum, s) => sum + s.pointCount, 0);
  const maxPoints = 5_000_000;
  const stride = totalPoints > maxPoints ? Math.ceil(totalPoints / maxPoints) : 1;
  const estimatedCount = Math.ceil(totalPoints / stride);

  const positions = new Float32Array(estimatedCount * 3);
  const colors = new Float32Array(estimatedCount * 3);
  const intensities = new Float32Array(estimatedCount);
  const classifications = new Float32Array(estimatedCount);

  // Track bounds for centering
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  let hasColor = false;
  let hasIntensity = false;
  let outIdx = 0;

  // First pass: decode all scans and find bounds
  interface DecodedScan {
    xs: Float64Array;
    ys: Float64Array;
    zs: Float64Array;
    rs?: Float64Array;
    gs?: Float64Array;
    bs?: Float64Array;
    ints?: Float64Array;
    count: number;
  }
  const decodedScans: DecodedScan[] = [];

  for (const scan of scans) {
    const fields = decodeBinarySection(buffer, scan, header.pageSize);

    // Determine coordinate system
    const hasCartesian = fields.has('cartesianX') && fields.has('cartesianY') && fields.has('cartesianZ');
    const hasSpherical = fields.has('sphericalRange') && fields.has('sphericalAzimuth') && fields.has('sphericalElevation');

    if (!hasCartesian && !hasSpherical) {
      continue; // Skip scan without coordinates
    }

    let xs: Float64Array;
    let ys: Float64Array;
    let zs: Float64Array;

    if (hasCartesian) {
      xs = fields.get('cartesianX')!;
      ys = fields.get('cartesianY')!;
      zs = fields.get('cartesianZ')!;
    } else {
      // Convert spherical to cartesian
      const range = fields.get('sphericalRange')!;
      const azimuth = fields.get('sphericalAzimuth')!;
      const elevation = fields.get('sphericalElevation')!;
      xs = new Float64Array(scan.pointCount);
      ys = new Float64Array(scan.pointCount);
      zs = new Float64Array(scan.pointCount);
      for (let i = 0; i < scan.pointCount; i++) {
        const r = range[i];
        const az = azimuth[i];
        const el = elevation[i];
        const cosEl = Math.cos(el);
        xs[i] = r * cosEl * Math.cos(az);
        ys[i] = r * cosEl * Math.sin(az);
        zs[i] = r * Math.sin(el);
      }
    }

    // Apply pose transform
    if (scan.rotation || scan.translation) {
      const [w, qx, qy, qz] = scan.rotation || [1, 0, 0, 0];
      const [tx, ty, tz] = scan.translation || [0, 0, 0];

      for (let i = 0; i < scan.pointCount; i++) {
        const [rx, ry, rz] = rotateByQuaternion(xs[i], ys[i], zs[i], w, qx, qy, qz);
        xs[i] = rx + tx;
        ys[i] = ry + ty;
        zs[i] = rz + tz;
      }
    }

    // Update bounds
    for (let i = 0; i < scan.pointCount; i++) {
      if (xs[i] < minX) minX = xs[i];
      if (xs[i] > maxX) maxX = xs[i];
      if (ys[i] < minY) minY = ys[i];
      if (ys[i] > maxY) maxY = ys[i];
      if (zs[i] < minZ) minZ = zs[i];
      if (zs[i] > maxZ) maxZ = zs[i];
    }

    const rs = fields.get('colorRed');
    const gs = fields.get('colorGreen');
    const bs = fields.get('colorBlue');
    const ints = fields.get('intensity');

    if (rs && gs && bs) hasColor = true;
    if (ints) hasIntensity = true;

    decodedScans.push({
      xs, ys, zs,
      rs: rs || undefined,
      gs: gs || undefined,
      bs: bs || undefined,
      ints: ints || undefined,
      count: scan.pointCount,
    });
  }

  if (decodedScans.length === 0) {
    throw new Error('No valid point data found in E57 file.');
  }

  // Center of bounds
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;

  // Second pass: write to output arrays with stride sampling and centering
  let globalIdx = 0;
  for (const ds of decodedScans) {
    for (let i = 0; i < ds.count; i++) {
      if (globalIdx % stride === 0 && outIdx < estimatedCount) {
        const x = ds.xs[i] - cx;
        const y = ds.ys[i] - cy;
        const z = ds.zs[i] - cz;

        // Z-up to Y-up conversion (same as LAS parser)
        positions[outIdx * 3] = x;
        positions[outIdx * 3 + 1] = z;
        positions[outIdx * 3 + 2] = -y;

        // Colors
        if (ds.rs && ds.gs && ds.bs) {
          let r = ds.rs[i];
          let g = ds.gs[i];
          let b = ds.bs[i];
          // E57 colors are typically 0-255 (uint8) or 0-65535 (uint16) or 0.0-1.0 (float)
          if (r > 1 || g > 1 || b > 1) {
            if (r > 255 || g > 255 || b > 255) {
              r /= 65535; g /= 65535; b /= 65535;
            } else {
              r /= 255; g /= 255; b /= 255;
            }
          }
          colors[outIdx * 3] = r;
          colors[outIdx * 3 + 1] = g;
          colors[outIdx * 3 + 2] = b;
        } else {
          colors[outIdx * 3] = 0.8;
          colors[outIdx * 3 + 1] = 0.8;
          colors[outIdx * 3 + 2] = 0.8;
        }

        // Intensity
        if (ds.ints) {
          let intensity = ds.ints[i];
          // Normalize if needed (E57 intensity can be various ranges)
          if (intensity > 1) {
            intensity = intensity > 255 ? intensity / 65535 : intensity / 255;
          }
          intensities[outIdx] = intensity;
        }

        outIdx++;
      }
      globalIdx++;
    }
  }

  // Trim arrays
  const finalPositions = outIdx < estimatedCount ? positions.slice(0, outIdx * 3) : positions;
  const finalColors = outIdx < estimatedCount ? colors.slice(0, outIdx * 3) : colors;
  const finalIntensities = outIdx < estimatedCount ? intensities.slice(0, outIdx) : intensities;
  const finalClassifications = outIdx < estimatedCount ? classifications.slice(0, outIdx) : classifications;

  // Create a synthetic LAS-compatible header
  const lasHeader: LASHeader = {
    signature: 'E57',
    versionMajor: header.majorVersion,
    versionMinor: header.minorVersion,
    headerSize: 48,
    offsetToPointData: 0,
    pointDataFormat: hasColor ? 2 : 0,
    pointDataRecordLength: 0,
    numberOfPoints: outIdx,
    scaleX: 1, scaleY: 1, scaleZ: 1,
    offsetX: 0, offsetY: 0, offsetZ: 0,
    minX, minY, minZ,
    maxX, maxY, maxZ,
  };

  return {
    header: lasHeader,
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
