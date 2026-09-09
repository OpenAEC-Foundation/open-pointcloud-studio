/**
 * Browser-side E57 (ASTM E2807) reader.
 *
 * What it reads:
 *   - Every data3D scan: cartesian or spherical points, colour, intensity,
 *     with the scan's pose applied so multi-scan files register correctly
 *   - Scanner stations (one per scan, from the pose translation)
 *   - Embedded photos (images2D: JPEG/PNG blobs with their pose)
 *
 * How the binary is laid out, because every bug so far came from getting
 * one of these wrong:
 *   - The file is 1024-byte pages: 1020 payload bytes then a 4-byte CRC.
 *     Every offset in the XML is *physical*; reading N logical bytes means
 *     skipping a CRC every 1020.
 *   - A CompressedVector is a 32-byte section header followed by data
 *     packets. Each packet holds one byte buffer per prototype field, in
 *     prototype order. Each field is its own LSB-first bit-packed stream.
 *   - A field's stream continues across packets: a 13-bit value can start in
 *     one packet and end in the next. The stream state must therefore be kept
 *     per field for the whole scan, never reset per packet.
 *   - Node types in the XML are capitalised ("ScaledInteger", "Float",
 *     "Integer"). Comparing against lowercase names silently routes every
 *     coordinate through the integer path and drops the scale factor.
 *
 * Points flagged by cartesianInvalidState / sphericalInvalidState are dropped.
 * Output is subsampled to MAX_POINTS, centred, and converted Z-up → Y-up like
 * the other parsers. CRCs are not verified.
 *
 * Not supported: CompressedVectors with codecs (none exist in practice; the
 * spec defines none) and files over 2 GB (ArrayBuffer limit).
 */

import type { ParsedPointcloud, LASHeader, ScanStation, ScanImage } from './LASParser';

const MAX_POINTS = 1_000_000;

/** Logical bytes decoded per read window; packets are at most 64 KiB. */
const WINDOW_BYTES = 4 * 1024 * 1024;

// ── Types ──────────────────────────────────────────────────────────────

interface E57FileHeader {
  majorVersion: number;
  minorVersion: number;
  xmlPhysicalOffset: number;
  xmlLogicalLength: number;
  pageSize: number;
}

type FieldKind = 'float' | 'scaledInteger' | 'integer';

interface E57FieldDef {
  name: string;
  kind: FieldKind;
  /** Packed width in bits. */
  bits: number;
  minimum: number;
  scale: number;
  offset: number;
}

interface E57Pose {
  translation: [number, number, number];
  rotation: [number, number, number, number]; // w, x, y, z
}

interface E57Scan {
  name: string;
  guid?: string;
  recordCount: number;
  /** Physical offset of the CompressedVector section header. */
  sectionOffset: number;
  prototype: E57FieldDef[];
  pose?: E57Pose;
  colorMax?: number;
  intensityMax?: number;
}

interface E57ImageDesc {
  name: string;
  guid?: string;
  scanGuid?: string;
  representation: ScanImage['representation'];
  mime: ScanImage['mime'];
  blobOffset: number;
  blobLength: number;
  width?: number;
  height?: number;
  pose?: E57Pose;
}

// ── Header ─────────────────────────────────────────────────────────────

function u64(view: DataView, offset: number): number {
  return view.getUint32(offset, true) + view.getUint32(offset + 4, true) * 0x100000000;
}

function readE57Header(view: DataView): E57FileHeader {
  let sig = '';
  for (let i = 0; i < 8; i++) sig += String.fromCharCode(view.getUint8(i));
  if (sig !== 'ASTM-E57') {
    throw new Error(`Invalid E57 file: expected "ASTM-E57" signature, got "${sig}"`);
  }
  return {
    majorVersion: view.getUint32(8, true),
    minorVersion: view.getUint32(12, true),
    xmlPhysicalOffset: u64(view, 24),
    xmlLogicalLength: u64(view, 32),
    pageSize: view.getUint32(40, true) || 1024,
  };
}

// ── Paged reading ──────────────────────────────────────────────────────

/**
 * Read `logLength` logical bytes starting at physical `physOffset`, skipping
 * the CRC at the end of every page. Returns fewer bytes at end of file.
 */
function readPagedData(buffer: ArrayBuffer, physOffset: number, logLength: number, pageSize: number): Uint8Array {
  const payload = pageSize - 4;
  const bytes = new Uint8Array(buffer);
  const out = new Uint8Array(Math.min(logLength, buffer.byteLength));
  let written = 0;
  let phys = physOffset;

  while (written < out.length && phys < buffer.byteLength) {
    const pageStart = Math.floor(phys / pageSize) * pageSize;
    const inPage = phys - pageStart;
    if (inPage >= payload) {
      phys = pageStart + pageSize;
      continue;
    }
    const take = Math.min(payload - inPage, out.length - written, buffer.byteLength - phys);
    if (take <= 0) break;
    out.set(bytes.subarray(phys, phys + take), written);
    written += take;
    phys += take;
    if (phys - pageStart >= payload) phys = pageStart + pageSize;
  }
  return written < out.length ? out.subarray(0, written) : out;
}

/** The physical offset reached after consuming `logical` bytes from `phys`. */
function advancePhysical(phys: number, logical: number, pageSize: number): number {
  const payload = pageSize - 4;
  let remaining = logical;
  while (remaining > 0) {
    const pageStart = Math.floor(phys / pageSize) * pageSize;
    const inPage = phys - pageStart;
    if (inPage >= payload) {
      phys = pageStart + pageSize;
      continue;
    }
    const step = Math.min(payload - inPage, remaining);
    phys += step;
    remaining -= step;
    if (phys - pageStart >= payload) phys = pageStart + pageSize;
  }
  return phys;
}

// ── XML ────────────────────────────────────────────────────────────────

function childText(el: Element | null, tag: string): string | undefined {
  if (!el) return undefined;
  for (const c of el.children) {
    if (c.tagName === tag) return c.textContent ?? undefined;
  }
  return undefined;
}

function childEl(el: Element | null, tag: string): Element | null {
  if (!el) return null;
  for (const c of el.children) {
    if (c.tagName === tag) return c;
  }
  return null;
}

function childNumber(el: Element | null, tag: string): number | undefined {
  const t = childText(el, tag);
  if (t === undefined) return undefined;
  const v = parseFloat(t.trim());
  return Number.isFinite(v) ? v : undefined;
}

function parsePose(el: Element | null): E57Pose | undefined {
  if (!el) return undefined;
  const t = childEl(el, 'translation');
  const r = childEl(el, 'rotation');
  return {
    translation: [
      childNumber(t, 'x') ?? 0,
      childNumber(t, 'y') ?? 0,
      childNumber(t, 'z') ?? 0,
    ],
    rotation: [
      childNumber(r, 'w') ?? 1,
      childNumber(r, 'x') ?? 0,
      childNumber(r, 'y') ?? 0,
      childNumber(r, 'z') ?? 0,
    ],
  };
}

function parsePrototype(protoEl: Element): E57FieldDef[] {
  const fields: E57FieldDef[] = [];
  for (const f of protoEl.children) {
    const rawType = (f.getAttribute('type') ?? '').toLowerCase();
    const attr = (n: string) => f.getAttribute(n);

    if (rawType === 'float') {
      const bits = (attr('precision') ?? 'double').toLowerCase() === 'single' ? 32 : 64;
      fields.push({ name: f.tagName, kind: 'float', bits, minimum: 0, scale: 1, offset: 0 });
    } else if (rawType === 'scaledinteger' || rawType === 'integer') {
      const minimum = parseFloat(attr('minimum') ?? '0');
      const maximum = parseFloat(attr('maximum') ?? '0');
      const range = maximum - minimum;
      const bits = range <= 0 ? 0 : Math.ceil(Math.log2(range + 1));
      fields.push({
        name: f.tagName,
        kind: rawType === 'integer' ? 'integer' : 'scaledInteger',
        bits,
        minimum,
        scale: parseFloat(attr('scale') ?? '1'),
        offset: parseFloat(attr('offset') ?? '0'),
      });
    }
    // Other node types cannot appear in a point prototype.
  }
  return fields;
}

function parseXml(xml: string): { scans: E57Scan[]; images: E57ImageDesc[] } {
  const doc = new DOMParser().parseFromString(xml, 'application/xml');
  const scans: E57Scan[] = [];
  const images: E57ImageDesc[] = [];

  const data3D = doc.getElementsByTagName('data3D')[0];
  if (data3D) {
    for (const child of data3D.children) {
      const points = childEl(child, 'points');
      if (!points || points.getAttribute('type') !== 'CompressedVector') continue;

      const recordCount = parseInt(points.getAttribute('recordCount') ?? '0', 10);
      if (!recordCount) continue;

      const codecs = childEl(points, 'codecs');
      if (codecs && codecs.children.length > 0) {
        throw new Error('This E57 file declares codecs on its point data, which is not supported.');
      }

      const proto = childEl(points, 'prototype');
      const colorLimits = childEl(child, 'colorLimits');
      const intensityLimits = childEl(child, 'intensityLimits');

      scans.push({
        name: childText(child, 'name')?.trim() || `scan_${scans.length + 1}`,
        guid: childText(child, 'guid')?.trim(),
        recordCount,
        sectionOffset: parseInt(points.getAttribute('fileOffset') ?? '0', 10),
        prototype: proto ? parsePrototype(proto) : [],
        pose: parsePose(childEl(child, 'pose')),
        colorMax: childNumber(colorLimits, 'colorRedMaximum'),
        intensityMax: childNumber(intensityLimits, 'intensityMaximum'),
      });
    }
  }

  const images2D = doc.getElementsByTagName('images2D')[0];
  if (images2D) {
    for (const child of images2D.children) {
      const reps: [string, ScanImage['representation']][] = [
        ['pinholeRepresentation', 'pinhole'],
        ['sphericalRepresentation', 'spherical'],
        ['cylindricalRepresentation', 'cylindrical'],
        ['visualReferenceRepresentation', 'visual'],
      ];
      let repEl: Element | null = null;
      let representation: ScanImage['representation'] = 'unknown';
      for (const [tag, kind] of reps) {
        repEl = childEl(child, tag);
        if (repEl) { representation = kind; break; }
      }
      if (!repEl) continue;

      const jpeg = childEl(repEl, 'jpegImage');
      const png = childEl(repEl, 'pngImage');
      const blob = jpeg ?? png;
      if (!blob) continue;

      images.push({
        name: childText(child, 'name')?.trim() || `image_${images.length + 1}`,
        guid: childText(child, 'guid')?.trim(),
        scanGuid: childText(child, 'associatedData3DGuid')?.trim(),
        representation,
        mime: jpeg ? 'image/jpeg' : 'image/png',
        blobOffset: parseInt(blob.getAttribute('fileOffset') ?? '0', 10),
        blobLength: parseInt(blob.getAttribute('length') ?? '0', 10),
        width: childNumber(repEl, 'imageWidth'),
        height: childNumber(repEl, 'imageHeight'),
        pose: parsePose(childEl(child, 'pose')),
      });
    }
  }

  return { scans, images };
}

// ── Bit streams ────────────────────────────────────────────────────────

/**
 * One field's bit-packed stream. Bytes are appended packet by packet; the
 * bit position survives across appends so values that straddle a packet
 * boundary decode correctly.
 */
class FieldStream {
  private buf = new Uint8Array(0);
  private bitPos = 0;

  push(chunk: Uint8Array): void {
    const keep = this.buf.subarray(this.bitPos >> 3);
    const merged = new Uint8Array(keep.length + chunk.length);
    merged.set(keep, 0);
    merged.set(chunk, keep.length);
    this.buf = merged;
    this.bitPos &= 7;
  }

  availableBits(): number {
    return this.buf.length * 8 - this.bitPos;
  }

  /** Read up to 32 bits, LSB-first. */
  read(count: number): number {
    let value = 0;
    let got = 0;
    while (got < count) {
      const byte = this.buf[this.bitPos >> 3];
      const bit = this.bitPos & 7;
      const take = Math.min(8 - bit, count - got);
      value += ((byte >> bit) & ((1 << take) - 1)) * 2 ** got;
      got += take;
      this.bitPos += take;
    }
    return value;
  }
}

const scratch = new DataView(new ArrayBuffer(8));

function decodeValue(f: E57FieldDef, s: FieldStream): number {
  if (f.kind === 'float') {
    if (f.bits === 32) {
      scratch.setUint32(0, s.read(32), true);
      return scratch.getFloat32(0, true);
    }
    scratch.setUint32(0, s.read(32), true);
    scratch.setUint32(4, s.read(32), true);
    return scratch.getFloat64(0, true);
  }

  let raw = 0;
  if (f.bits > 32) {
    const lo = s.read(32);
    const hi = s.read(f.bits - 32);
    raw = lo + hi * 0x100000000;
  } else if (f.bits > 0) {
    raw = s.read(f.bits);
  }
  const v = raw + f.minimum;
  return f.kind === 'scaledInteger' ? v * f.scale + f.offset : v;
}

// ── CompressedVector decoding ──────────────────────────────────────────

/**
 * Walk a scan's data packets and call `onRecord` once per complete record,
 * with one value per prototype field.
 */
function decodeScan(
  buffer: ArrayBuffer,
  scan: E57Scan,
  pageSize: number,
  onRecord: (values: Float64Array) => void,
): number {
  const fields = scan.prototype;
  if (fields.length === 0) return 0;

  const section = readPagedData(buffer, scan.sectionOffset, 32, pageSize);
  if (section.length < 32 || section[0] !== 1) {
    throw new Error(`E57 scan "${scan.name}": bad CompressedVector section header`);
  }
  const sv = new DataView(section.buffer, section.byteOffset, 32);
  const sectionLogicalLength = u64(sv, 8);
  const dataPhysicalOffset = u64(sv, 16);

  const streams = fields.map(() => new FieldStream());
  const record = new Float64Array(fields.length);
  let decoded = 0;
  let logicalLeft = Math.max(0, sectionLogicalLength - 32);
  let phys = dataPhysicalOffset;

  // Emit every record for which all streams hold at least one whole value.
  const drain = () => {
    let n = Infinity;
    for (let i = 0; i < fields.length; i++) {
      const b = fields[i].bits;
      if (b > 0) n = Math.min(n, Math.floor(streams[i].availableBits() / b));
    }
    if (!Number.isFinite(n)) n = 0;
    n = Math.min(n, scan.recordCount - decoded);
    for (let k = 0; k < n; k++) {
      for (let i = 0; i < fields.length; i++) record[i] = decodeValue(fields[i], streams[i]);
      onRecord(record);
    }
    decoded += n;
  };

  while (decoded < scan.recordCount && logicalLeft > 0 && phys < buffer.byteLength) {
    const window = readPagedData(buffer, phys, Math.min(WINDOW_BYTES, logicalLeft), pageSize);
    if (window.length < 6) break;

    let off = 0;
    while (off + 6 <= window.length) {
      const type = window[off];
      const packetLength = (window[off + 2] | (window[off + 3] << 8)) + 1;
      if (off + packetLength > window.length) break; // partial packet: re-read from here

      if (type === 1) {
        const streamCount = window[off + 4] | (window[off + 5] << 8);
        let p = off + 6 + streamCount * 2;
        for (let s = 0; s < streamCount; s++) {
          const len = window[off + 6 + s * 2] | (window[off + 7 + s * 2] << 8);
          if (s < streams.length) streams[s].push(window.subarray(p, p + len));
          p += len;
        }
        drain();
      } else if (type !== 0 && type !== 2) {
        throw new Error(`E57 scan "${scan.name}": unknown packet type ${type}`);
      }
      off += packetLength;
      if (decoded >= scan.recordCount) break;
    }

    if (off === 0) break; // a single packet larger than the window cannot happen (64 KiB max)
    phys = advancePhysical(phys, off, pageSize);
    logicalLeft -= off;
  }

  return decoded;
}

// ── Geometry helpers ───────────────────────────────────────────────────

function rotateByQuaternion(
  x: number, y: number, z: number,
  w: number, qx: number, qy: number, qz: number,
): [number, number, number] {
  const tx = 2 * (qy * z - qz * y);
  const ty = 2 * (qz * x - qx * z);
  const tz = 2 * (qx * y - qy * x);
  return [
    x + w * tx + (qy * tz - qz * ty),
    y + w * ty + (qz * tx - qx * tz),
    z + w * tz + (qx * ty - qy * tx),
  ];
}

// ── Main ───────────────────────────────────────────────────────────────

export type E57ProgressCallback = (phase: string, percent: number) => void;

export function parseE57(buffer: ArrayBuffer, onProgress?: E57ProgressCallback): ParsedPointcloud {
  const view = new DataView(buffer);
  const header = readE57Header(view);

  const xmlBytes = readPagedData(buffer, header.xmlPhysicalOffset, header.xmlLogicalLength, header.pageSize);
  const { scans, images: imageDescs } = parseXml(new TextDecoder('utf-8').decode(xmlBytes));

  if (scans.length === 0) {
    throw new Error('No point cloud data found in E57 file.');
  }

  const totalRecords = scans.reduce((sum, s) => sum + s.recordCount, 0);
  const stride = Math.max(1, Math.ceil(totalRecords / MAX_POINTS));
  const capacity = Math.ceil(totalRecords / stride) + 1;

  // World-space accumulators; converted to the viewer frame at the end.
  const wx = new Float64Array(capacity);
  const wy = new Float64Array(capacity);
  const wz = new Float64Array(capacity);
  const colors = new Float32Array(capacity * 3);
  const intensities = new Float32Array(capacity);

  let kept = 0;
  let validSeen = 0;
  let hasColor = false;
  let hasIntensity = false;
  let intensityObservedMax = 0;

  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  const stations: ScanStation[] = [];

  scans.forEach((scan, scanIndex) => {
    onProgress?.(`Decoding scan ${scanIndex + 1}/${scans.length}`, 5 + (scanIndex / scans.length) * 80);

    const idx = (name: string) => scan.prototype.findIndex((f) => f.name === name);
    const iX = idx('cartesianX'), iY = idx('cartesianY'), iZ = idx('cartesianZ');
    const iR = idx('sphericalRange'), iA = idx('sphericalAzimuth'), iE = idx('sphericalElevation');
    const cartesian = iX >= 0 && iY >= 0 && iZ >= 0;
    const spherical = !cartesian && iR >= 0 && iA >= 0 && iE >= 0;
    if (!cartesian && !spherical) return;

    const iInvalid = idx(cartesian ? 'cartesianInvalidState' : 'sphericalInvalidState');
    const iRed = idx('colorRed'), iGreen = idx('colorGreen'), iBlue = idx('colorBlue');
    const iInt = idx('intensity');
    const scanHasColor = iRed >= 0 && iGreen >= 0 && iBlue >= 0;
    const scanHasIntensity = iInt >= 0;
    if (scanHasColor) hasColor = true;
    if (scanHasIntensity) hasIntensity = true;

    // Colour range: declared limit when present, else the usual 8-bit.
    const colorScale = 1 / (scan.colorMax && scan.colorMax > 0 ? scan.colorMax : 255);

    const pose = scan.pose;
    const [qw, qx, qy, qz] = pose?.rotation ?? [1, 0, 0, 0];
    const [tx, ty, tz] = pose?.translation ?? [0, 0, 0];
    const hasRotation = !(qw === 1 && qx === 0 && qy === 0 && qz === 0);

    const keptBefore = kept;

    decodeScan(buffer, scan, header.pageSize, (v) => {
      if (iInvalid >= 0 && v[iInvalid] !== 0) return;

      // Subsample over valid points only, so invalid ones cost nothing.
      if (validSeen++ % stride !== 0 || kept >= capacity) return;

      let x: number, y: number, z: number;
      if (cartesian) {
        x = v[iX]; y = v[iY]; z = v[iZ];
      } else {
        const r = v[iR], az = v[iA], el = v[iE];
        const cosEl = Math.cos(el);
        x = r * cosEl * Math.cos(az);
        y = r * cosEl * Math.sin(az);
        z = r * Math.sin(el);
      }

      if (hasRotation) [x, y, z] = rotateByQuaternion(x, y, z, qw, qx, qy, qz);
      x += tx; y += ty; z += tz;

      wx[kept] = x; wy[kept] = y; wz[kept] = z;
      if (x < minX) minX = x; if (x > maxX) maxX = x;
      if (y < minY) minY = y; if (y > maxY) maxY = y;
      if (z < minZ) minZ = z; if (z > maxZ) maxZ = z;

      if (scanHasColor) {
        colors[kept * 3] = v[iRed] * colorScale;
        colors[kept * 3 + 1] = v[iGreen] * colorScale;
        colors[kept * 3 + 2] = v[iBlue] * colorScale;
      } else {
        colors[kept * 3] = colors[kept * 3 + 1] = colors[kept * 3 + 2] = 0.8;
      }

      if (scanHasIntensity) {
        const raw = v[iInt];
        const max = scan.intensityMax && scan.intensityMax > 0 ? scan.intensityMax : 0;
        intensities[kept] = max ? raw / max : raw;
        if (!max && raw > intensityObservedMax) intensityObservedMax = raw;
      }

      kept++;
    });

    stations.push({
      name: scan.name,
      guid: scan.guid,
      position: [tx, ty, tz],
      rotation: pose?.rotation,
      recordCount: scan.recordCount,
      keptCount: kept - keptBefore,
    });
  });

  if (kept === 0) {
    throw new Error('No valid points found in E57 file.');
  }

  // Scans without declared intensity limits: normalise by what was seen.
  if (intensityObservedMax > 1) {
    const inv = 1 / intensityObservedMax;
    for (let i = 0; i < kept; i++) if (intensities[i] > 1) intensities[i] *= inv;
  }

  // Photos
  onProgress?.('Reading images', 88);
  const images: ScanImage[] = [];
  for (const d of imageDescs) {
    if (!d.blobLength) continue;
    // A Blob section is a 16-byte header (id 0, reserved, u64 length) then data.
    const raw = readPagedData(buffer, d.blobOffset, 16 + d.blobLength, header.pageSize);
    if (raw.length <= 16) continue;
    images.push({
      name: d.name,
      guid: d.guid,
      scanGuid: d.scanGuid,
      mime: d.mime,
      representation: d.representation,
      width: d.width,
      height: d.height,
      position: d.pose?.translation,
      rotation: d.pose?.rotation,
      data: raw.slice(16),
    });
  }

  // Centre and convert Z-up → Y-up, matching the other parsers.
  onProgress?.('Packing points', 94);
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  const cz = (minZ + maxZ) / 2;
  const positions = new Float32Array(kept * 3);
  for (let i = 0; i < kept; i++) {
    positions[i * 3] = wx[i] - cx;
    positions[i * 3 + 1] = wz[i] - cz;
    positions[i * 3 + 2] = -(wy[i] - cy);
  }

  const lasHeader: LASHeader = {
    signature: 'E57',
    versionMajor: header.majorVersion,
    versionMinor: header.minorVersion,
    headerSize: 48,
    offsetToPointData: 0,
    pointDataFormat: hasColor ? 2 : 0,
    pointDataRecordLength: 0,
    numberOfPoints: kept,
    scaleX: 1, scaleY: 1, scaleZ: 1,
    offsetX: 0, offsetY: 0, offsetZ: 0,
    minX, minY, minZ, maxX, maxY, maxZ,
  };

  return {
    header: lasHeader,
    positions,
    colors: colors.slice(0, kept * 3),
    intensities: intensities.slice(0, kept),
    classifications: new Float32Array(kept),
    center: [cx, cy, cz],
    hasColor,
    hasIntensity,
    hasClassification: false,
    stations,
    images,
  };
}
