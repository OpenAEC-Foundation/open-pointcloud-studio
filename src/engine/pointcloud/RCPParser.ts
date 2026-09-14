/**
 * Autodesk ReCap project (.rcp) reader.
 *
 * An .rcp is a ZIP archive holding one XML document that lists the project's
 * scans with their registration (translation + 3×3 rotation) and the paths of
 * the .rcs files that hold the points. The points themselves are not readable
 * — see docs/formats/recap-rcs.md — so this yields the project's scanner
 * stations, registered, with no geometry. Loading the E57 export of each scan
 * next to it gives the points.
 *
 * Runs on the main thread: needs DOMParser and DecompressionStream.
 */

import type { ParsedPointcloud, LASHeader, ScanStation } from './LASParser';

/** One scan entry of the project. */
export interface RCPScan {
  name: string;
  id?: string;
  /** PCPid — the GUID that also appears in the .rcs header. */
  pcpId?: string;
  /** Absolute path of the .rcs on the machine that saved the project. */
  rawScanPath?: string;
  /** Registration: world = R · local + T, R row-major. */
  translation: [number, number, number];
  rotation: [number, number, number, number, number, number, number, number, number];
}

export interface RCPProject {
  scans: RCPScan[];
  application?: string;
  savedBy?: string;
  savedAt?: string;
}

// ── ZIP ────────────────────────────────────────────────────────────────

const LOCAL_FILE_HEADER = 0x04034b50;

/** Extract every entry of a stored-or-deflated ZIP as {name, bytes}. */
async function unzip(buffer: ArrayBuffer): Promise<{ name: string; bytes: Uint8Array }[]> {
  const view = new DataView(buffer);
  const bytes = new Uint8Array(buffer);
  const out: { name: string; bytes: Uint8Array }[] = [];
  let off = 0;

  while (off + 30 <= buffer.byteLength && view.getUint32(off, true) === LOCAL_FILE_HEADER) {
    const flags = view.getUint16(off + 6, true);
    const method = view.getUint16(off + 8, true);
    const compressedSize = view.getUint32(off + 18, true);
    const nameLen = view.getUint16(off + 26, true);
    const extraLen = view.getUint16(off + 28, true);
    const name = new TextDecoder().decode(bytes.subarray(off + 30, off + 30 + nameLen));
    const dataStart = off + 30 + nameLen + extraLen;

    // Sizes may be deferred to a data descriptor (flag bit 3); .rcp files
    // written by ReCap carry them inline, which is all that is supported.
    if (flags & 0x8 && compressedSize === 0) {
      throw new Error('Unsupported .rcp: ZIP entry uses a data descriptor');
    }

    const data = bytes.subarray(dataStart, dataStart + compressedSize);
    let content: Uint8Array;
    if (method === 0) {
      content = data;
    } else if (method === 8) {
      const stream = new Blob([data]).stream().pipeThrough(new DecompressionStream('deflate-raw'));
      content = new Uint8Array(await new Response(stream).arrayBuffer());
    } else {
      throw new Error(`Unsupported .rcp: ZIP compression method ${method}`);
    }
    out.push({ name, bytes: content });
    off = dataStart + compressedSize;
  }

  if (out.length === 0) throw new Error('Not a ReCap project: no ZIP entries found');
  return out;
}

// ── XML ────────────────────────────────────────────────────────────────

function num(el: Element | null, attr: string, fallback = 0): number {
  const v = el?.getAttribute(attr);
  if (v === null || v === undefined) return fallback;
  const n = parseFloat(v);
  return Number.isFinite(n) ? n : fallback;
}

export function parseRCPXml(xml: string): RCPProject {
  const doc = new DOMParser().parseFromString(xml, 'application/xml');
  const scans: RCPScan[] = [];

  for (const shot of doc.getElementsByTagName('ShotInfo')) {
    const tform = shot.getElementsByTagName('tform')[0] ?? null;
    const T = tform?.getElementsByTagName('T')[0] ?? null;
    const R = tform?.getElementsByTagName('R')[0] ?? null;
    scans.push({
      name: shot.getAttribute('name') ?? `scan_${scans.length + 1}`,
      id: shot.getAttribute('id') ?? undefined,
      pcpId: shot.getAttribute('PCPid') ?? undefined,
      rawScanPath: shot.getAttribute('rawScanPath') ?? undefined,
      translation: [num(T, 'x'), num(T, 'y'), num(T, 'z')],
      rotation: [
        num(R, 'xx', 1), num(R, 'xy'), num(R, 'xz'),
        num(R, 'yx'), num(R, 'yy', 1), num(R, 'yz'),
        num(R, 'zx'), num(R, 'zy'), num(R, 'zz', 1),
      ],
    });
  }

  const save = doc.getElementsByTagName('SaveInformation')[0];
  const project = doc.getElementsByTagName('Project')[0];
  return {
    scans,
    application: project?.getAttribute('app') ?? undefined,
    savedBy: save?.getAttribute('Application') ?? undefined,
    savedAt: save?.getAttribute('Timestamp') ?? undefined,
  };
}

/** Row-major 3×3 rotation → unit quaternion [w, x, y, z]. */
function matrixToQuaternion(m: RCPScan['rotation']): [number, number, number, number] {
  const [xx, xy, xz, yx, yy, yz, zx, zy, zz] = m;
  const trace = xx + yy + zz;
  let w: number, x: number, y: number, z: number;
  if (trace > 0) {
    const s = Math.sqrt(trace + 1) * 2;
    w = s / 4; x = (zy - yz) / s; y = (xz - zx) / s; z = (yx - xy) / s;
  } else if (xx > yy && xx > zz) {
    const s = Math.sqrt(1 + xx - yy - zz) * 2;
    w = (zy - yz) / s; x = s / 4; y = (xy + yx) / s; z = (xz + zx) / s;
  } else if (yy > zz) {
    const s = Math.sqrt(1 + yy - xx - zz) * 2;
    w = (xz - zx) / s; x = (xy + yx) / s; y = s / 4; z = (yz + zy) / s;
  } else {
    const s = Math.sqrt(1 + zz - xx - yy) * 2;
    w = (yx - xy) / s; x = (xz + zx) / s; y = (yz + zy) / s; z = s / 4;
  }
  return [w, x, y, z];
}

// ── Main ───────────────────────────────────────────────────────────────

export async function parseRCPProject(buffer: ArrayBuffer): Promise<RCPProject> {
  const entries = await unzip(buffer);
  const xmlEntry = entries.find((e) => e.name.toLowerCase().endsWith('.xml')) ?? entries[0];
  return parseRCPXml(new TextDecoder().decode(xmlEntry.bytes));
}

/**
 * Present a ReCap project as a pointcloud with no points: its stations carry
 * the registration, so they render as markers and can be flown to, and the
 * matching E57 files can be loaded alongside.
 */
export async function parseRCP(buffer: ArrayBuffer): Promise<ParsedPointcloud> {
  const project = await parseRCPProject(buffer);
  if (project.scans.length === 0) {
    throw new Error('ReCap project contains no scans.');
  }

  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;
  const stations: ScanStation[] = project.scans.map((s) => {
    const [x, y, z] = s.translation;
    if (x < minX) minX = x; if (x > maxX) maxX = x;
    if (y < minY) minY = y; if (y > maxY) maxY = y;
    if (z < minZ) minZ = z; if (z > maxZ) maxZ = z;
    return {
      name: s.name,
      guid: s.pcpId ?? s.id,
      position: [x, y, z],
      rotation: matrixToQuaternion(s.rotation),
      recordCount: 0,
      keptCount: 0,
    };
  });

  // A single station has no extent; give the bounds a little room so the
  // viewer's fit and the shader's elevation ramp have something to work with.
  if (minX === maxX) { minX -= 1; maxX += 1; }
  if (minY === maxY) { minY -= 1; maxY += 1; }
  if (minZ === maxZ) { minZ -= 1; maxZ += 1; }

  const header: LASHeader = {
    signature: 'RCP',
    versionMajor: 1, versionMinor: 0,
    headerSize: 0, offsetToPointData: 0,
    pointDataFormat: 0, pointDataRecordLength: 0,
    numberOfPoints: 0,
    scaleX: 1, scaleY: 1, scaleZ: 1,
    offsetX: 0, offsetY: 0, offsetZ: 0,
    minX, minY, minZ, maxX, maxY, maxZ,
  };

  return {
    header,
    positions: new Float32Array(0),
    colors: new Float32Array(0),
    intensities: new Float32Array(0),
    classifications: new Float32Array(0),
    center: [(minX + maxX) / 2, (minY + maxY) / 2, (minZ + maxZ) / 2],
    hasColor: false,
    hasIntensity: false,
    hasClassification: false,
    stations,
    images: [],
  };
}
