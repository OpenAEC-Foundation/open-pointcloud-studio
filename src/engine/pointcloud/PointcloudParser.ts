/**
 * Unified pointcloud file parser — dispatches to format-specific parsers.
 *
 * Supported formats:
 *   .las  — ASPRS LAS (uncompressed)
 *   .laz  — ASPRS LAZ (compressed, via laz-perf WASM)
 *   .pts  — Leica PTS (text)
 *   .ply  — Stanford PLY (ASCII + binary LE)
 *   .xyz  — XYZ text
 *   .txt  — Generic text (same as XYZ)
 *   .csv  — Comma-separated (same as XYZ)
 *   .e57  — ASTM E57 (uncompressed)
 *
 * Not supported (proprietary):
 *   .rcp/.rcs — Autodesk ReCap (convert to LAS/LAZ first)
 */

import type { ParsedPointcloud } from './LASParser';
import { parseLAS } from './LASParser';
import { parsePTS } from './PTSParser';
import { parsePLY } from './PLYParser';
import { parseXYZ } from './XYZParser';
import { parseOBJ } from './OBJParser';

/** File extensions accepted by the import dialog */
export const SUPPORTED_EXTENSIONS = ['.las', '.laz', '.pts', '.ply', '.xyz', '.txt', '.csv', '.obj', '.e57'];

/** Human-readable format description for the file dialog */
export const FORMAT_DESCRIPTION = 'Point Clouds';

/** Accept string for HTML file input */
export const FILE_INPUT_ACCEPT = SUPPORTED_EXTENSIONS.join(',');

/** Unsupported formats that we should warn about */
const UNSUPPORTED_PROPRIETARY: Record<string, string> = {
  '.rcp': 'Autodesk ReCap Project — convert to LAS/LAZ using ReCap or CloudCompare',
  '.rcs': 'Autodesk ReCap Scan — convert to LAS/LAZ using ReCap or CloudCompare',
  '.fls': 'FARO Scene — convert to LAS/LAZ using FARO Scene or CloudCompare',
};

/**
 * Parse a pointcloud file from a browser File object.
 * Returns parsed geometry data ready for Three.js rendering.
 */
export async function parsePointcloudFile(file: File): Promise<ParsedPointcloud> {
  const ext = getExtension(file.name);

  // Check for unsupported proprietary formats
  const unsupported = UNSUPPORTED_PROPRIETARY[ext];
  if (unsupported) {
    throw new Error(`${ext.toUpperCase()} is a proprietary format.\n${unsupported}.`);
  }

  const buffer = await file.arrayBuffer();

  switch (ext) {
    case '.las':
      return parseLAS(buffer);

    case '.laz': {
      const { parseLAZ } = await import('./LAZParser');
      return parseLAZ(buffer);
    }

    case '.pts':
      return parsePTS(buffer);

    case '.ply':
      return parsePLY(buffer);

    case '.xyz':
    case '.txt':
    case '.csv':
      return parseXYZ(buffer);

    case '.obj':
      return parseOBJ(buffer);

    case '.e57': {
      const { parseE57 } = await import('./E57Parser');
      return parseE57(buffer);
    }

    default:
      throw new Error(`Unsupported file format: ${ext}`);
  }
}

function getExtension(filename: string): string {
  const dot = filename.lastIndexOf('.');
  return dot >= 0 ? filename.substring(dot).toLowerCase() : '';
}
