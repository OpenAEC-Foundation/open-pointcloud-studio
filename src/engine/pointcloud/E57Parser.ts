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
 *
 * Handles non-page-aligned physical offsets: the first read starts
 * mid-page and reads only the remaining data bytes before the CRC.
 */
function readPagedData(
  buffer: ArrayBuffer,
  physOffset: number,
  logLength: number,
  pageSize: number,
): Uint8Array {
  const dataPerPage = pageSize - 4;
  // Clamp logLength so we never read past the buffer
  const safeLogLength = Math.min(logLength, buffer.byteLength);
  const result = new Uint8Array(safeLogLength);
  let written = 0;

  // Determine position within the first page
  const offsetInPage = physOffset % pageSize;
  const firstPageStart = physOffset - offsetInPage;

  // First page: read from physOffset to end of data portion (before CRC)
  const firstChunkAvail = dataPerPage - offsetInPage;
  const firstChunkSize = Math.min(firstChunkAvail, safeLogLength);
  if (physOffset + firstChunkSize > buffer.byteLength) {
    // Not enough data — return what we have
    const avail = Math.max(0, buffer.byteLength - physOffset);
    result.set(new Uint8Array(buffer, physOffset, avail), 0);
    return result.slice(0, avail);
  }
  result.set(new Uint8Array(buffer, physOffset, firstChunkSize), 0);
  written += firstChunkSize;

  // Subsequent pages: read full data portions
  let currentPageStart = firstPageStart + pageSize;
  while (written < safeLogLength && currentPageStart < buffer.byteLength) {
    const avail = Math.min(dataPerPage, buffer.byteLength - currentPageStart);
    const chunkSize = Math.min(avail, safeLogLength - written);
    if (chunkSize <= 0) break;
    result.set(new Uint8Array(buffer, currentPageStart, chunkSize), written);
    written += chunkSize;
    currentPageStart += pageSize;
  }

  return written < safeLogLength ? result.slice(0, written) : result;
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

    // JS bitwise ops produce signed int32; convert to unsigned for 32-bit reads
    return result >>> 0;
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
 *
 * The binary layout is:
 *   1. CompressedVectorSectionHeader (32 bytes at binaryPhysicalOffset):
 *        uint8  sectionId (= 1)
 *        7 bytes reserved
 *        uint64 sectionLogicalLength
 *        uint64 dataPhysicalOffset   ← actual data packets start here
 *        uint64 indexPhysicalOffset
 *   2. Data packets (at dataPhysicalOffset):
 *        uint8  packetType (1 = data)
 *        uint8  reserved
 *        uint16 packetLengthMinus1
 *        uint16 bytestreamCount
 *        uint16 bytestreamLength[bytestreamCount]
 *        <bytestream data>
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

  // Read the CompressedVector section header (32 bytes)
  const sectionHeader = readPagedData(buffer, scan.binaryPhysicalOffset, 32, pageSize);
  const shView = new DataView(sectionHeader.buffer, sectionHeader.byteOffset, sectionHeader.byteLength);

  // Validate section ID
  if (sectionHeader[0] !== 1) {
    throw new Error(`Invalid CompressedVector section ID: ${sectionHeader[0]}`);
  }

  // Data packets start at dataPhysicalOffset (uint64 at byte 16)
  const dataPhysLo = shView.getUint32(16, true);
  const dataPhysHi = shView.getUint32(20, true);
  const dataPhysicalOffset = dataPhysLo + dataPhysHi * 0x100000000;

  // Estimate logical data size for all points
  let bitsPerPoint = 0;
  for (const field of scan.prototype) {
    if (field.type === 'float') {
      bitsPerPoint += field.precision; // 32 or 64
    } else if (field.type === 'scaledInteger' || field.type === 'integer') {
      bitsPerPoint += bitsForRange(field.minimum, field.maximum);
    }
  }

  // Generous estimate (plus packet headers overhead)
  const packetOverhead = Math.ceil(scan.pointCount / 1000) * (6 + fieldCount * 2) + 4096;
  const estimatedLogicalBytes = Math.ceil((bitsPerPoint * scan.pointCount) / 8) + packetOverhead;

  // Read paged data starting from the actual data packet offset
  const rawData = readPagedData(buffer, dataPhysicalOffset, estimatedLogicalBytes, pageSize);
  const rawWritten = rawData.length;

  // Parse data packets
  let offset = 0;
  let pointsDecoded = 0;

  while (pointsDecoded < scan.pointCount && offset < rawWritten) {
    // Packet header: 1 byte type
    const packetType = rawData[offset];

    if (packetType === 0) {
      // Index packet (16 bytes total)
      offset += 16;
      continue;
    }

    if (packetType !== 1) {
      // Unknown packet type — stop
      break;
    }

    // Data packet (type 1):
    //   byte 0: packetType = 1
    //   byte 1: reserved
    //   bytes 2-3: packetLengthMinus1 (uint16 LE)
    //   bytes 4-5: bytestreamCount (uint16 LE)
    //   then 2 bytes per bytestream: buffer length
    if (offset + 5 >= rawWritten) break;

    const packetLengthMinus1 = rawData[offset + 2] | (rawData[offset + 3] << 8);
    const packetLength = packetLengthMinus1 + 1;
    const bytestreamCount = rawData[offset + 4] | (rawData[offset + 5] << 8);

    // Packet header size: 6 + 2 * bytestreamCount
    const headerSize = 6 + bytestreamCount * 2;
    if (offset + headerSize > rawWritten) break;

    // Read per-bytestream lengths
    const streamLengths: number[] = [];
    for (let s = 0; s < bytestreamCount; s++) {
      const lenOff = offset + 6 + s * 2;
      streamLengths.push(rawData[lenOff] | (rawData[lenOff + 1] << 8));
    }

    // Decode each bytestream (only process fields we know about)
    let dataOffset = offset + headerSize;
    const fieldsToProcess = Math.min(fieldCount, bytestreamCount);

    for (let s = 0; s < fieldsToProcess; s++) {
      const field = scan.prototype[s];
      const streamLen = streamLengths[s] || 0;
      const streamStart = dataOffset;
      const arr = result.get(field.name)!;

      if (field.type === 'float') {
        const reader = new BitReader(rawData, streamStart);
        const bytesPerVal = field.precision === 32 ? 4 : 8;
        const valsInStream = Math.min(
          Math.floor(streamLen / bytesPerVal),
          scan.pointCount - pointsDecoded,
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
          const valsInStream = Math.min(
            streamLen > 0 ? scan.pointCount - pointsDecoded : 0,
            scan.pointCount - pointsDecoded,
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
            scan.pointCount - pointsDecoded,
          );
          for (let p = 0; p < valsInStream; p++) {
            const raw = reader.readBits(bits) + field.minimum;
            arr[pointsDecoded + p] = field.type === 'scaledInteger'
              ? raw * field.scale + field.offset
              : raw;
          }
        }
      }

      dataOffset += streamLen;
    }

    // Skip remaining bytestreams we didn't process
    for (let s = fieldsToProcess; s < bytestreamCount; s++) {
      dataOffset += streamLengths[s] || 0;
    }

    // Count points decoded — use first field's stream
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

    // Advance to next packet
    offset += packetLength;
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
