/**
 * Unified pointcloud file parser — dispatches to format-specific parsers
 * via a Web Worker (main thread stays responsive).
 *
 * Supported formats:
 *   .las  — ASPRS LAS (uncompressed)
 *   .laz  — ASPRS LAZ (compressed, via laz-perf WASM)
 *   .pts  — Leica PTS (text)
 *   .ply  — Stanford PLY (ASCII + binary LE)
 *   .xyz  — XYZ text
 *   .asc  — ASCII point cloud (same as XYZ)
 *   .txt  — Generic text (same as XYZ)
 *   .csv  — Comma-separated (same as XYZ)
 *   .obj  — Wavefront OBJ mesh/points
 *   .pcd  — Point Cloud Data (PCL/ROS)
 *   .ptx  — Leica PTX structured scan
 *   .off  — Object File Format
 *   .stl  — Stereolithography (ASCII + binary)
 *   .dxf  — AutoCAD Drawing Exchange Format
 *   .e57  — ASTM E57 (uncompressed, main-thread only)
 *
 * Not supported (proprietary):
 *   .rcp/.rcs — Autodesk ReCap (convert to LAS/LAZ first)
 */

import type { ParsedPointcloud } from './LASParser';
import { parsePointcloud, type ProgressCallback } from './parsePointcloudWorker';

/** File extensions accepted by the import dialog */
export const SUPPORTED_EXTENSIONS = ['.las', '.laz', '.pts', '.ply', '.xyz', '.asc', '.txt', '.csv', '.obj', '.pcd', '.ptx', '.off', '.stl', '.dxf', '.e57'];

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
 * Parsing runs in a Web Worker so the UI stays responsive.
 */
export async function parsePointcloudFile(
  file: File,
  onProgress?: ProgressCallback,
): Promise<ParsedPointcloud> {
  const ext = getExtension(file.name);

  const unsupported = UNSUPPORTED_PROPRIETARY[ext];
  if (unsupported) {
    throw new Error(`${ext.toUpperCase()} is a proprietary format.\n${unsupported}.`);
  }

  onProgress?.('Reading file...', 0);
  const buffer = await file.arrayBuffer();

  return parsePointcloud(ext, buffer, onProgress);
}

function getExtension(filename: string): string {
  const dot = filename.lastIndexOf('.');
  return dot >= 0 ? filename.substring(dot).toLowerCase() : '';
}
