/**
 * Mesh Exporter — Export reconstructed meshes to OBJ format.
 *
 * Generates Wavefront OBJ text from positions, indices, and optional
 * normals/colors. Provides a helper to trigger a browser download.
 */

/**
 * Export a mesh to Wavefront OBJ format string.
 *
 * @param positions - Float32Array of xyz vertex positions (length = numVerts * 3)
 * @param indices   - Uint32Array of triangle face indices (length = numTris * 3)
 * @param normals   - Optional Float32Array of per-vertex normals (length = numVerts * 3)
 * @param colors    - Optional Float32Array of per-vertex RGB colors 0-1 (length = numVerts * 3)
 * @returns OBJ file content as a string
 */
export function exportToOBJ(
  positions: Float32Array,
  indices: Uint32Array,
  normals?: Float32Array,
  colors?: Float32Array
): string {
  const numVerts = positions.length / 3;
  const numFaces = indices.length / 3;

  // Pre-allocate rough size estimate for performance
  const lines: string[] = [];
  lines.push('# Exported from Open Pointcloud Studio');
  lines.push(`# Vertices: ${numVerts}`);
  lines.push(`# Faces: ${numFaces}`);
  lines.push('');

  // Vertices (with optional vertex colors as OBJ extension: v x y z r g b)
  for (let i = 0; i < numVerts; i++) {
    const x = positions[i * 3];
    const y = positions[i * 3 + 1];
    const z = positions[i * 3 + 2];

    if (colors) {
      const r = colors[i * 3];
      const g = colors[i * 3 + 1];
      const b = colors[i * 3 + 2];
      lines.push(`v ${x.toFixed(6)} ${y.toFixed(6)} ${z.toFixed(6)} ${r.toFixed(4)} ${g.toFixed(4)} ${b.toFixed(4)}`);
    } else {
      lines.push(`v ${x.toFixed(6)} ${y.toFixed(6)} ${z.toFixed(6)}`);
    }
  }

  lines.push('');

  // Normals
  if (normals) {
    for (let i = 0; i < numVerts; i++) {
      const nx = normals[i * 3];
      const ny = normals[i * 3 + 1];
      const nz = normals[i * 3 + 2];
      lines.push(`vn ${nx.toFixed(6)} ${ny.toFixed(6)} ${nz.toFixed(6)}`);
    }
    lines.push('');
  }

  // Faces (OBJ uses 1-based indices)
  for (let i = 0; i < numFaces; i++) {
    const a = indices[i * 3] + 1;
    const b = indices[i * 3 + 1] + 1;
    const c = indices[i * 3 + 2] + 1;

    if (normals) {
      lines.push(`f ${a}//${a} ${b}//${b} ${c}//${c}`);
    } else {
      lines.push(`f ${a} ${b} ${c}`);
    }
  }

  lines.push('');
  return lines.join('\n');
}

/**
 * Trigger a browser download of an OBJ file.
 *
 * @param objContent - The OBJ file content string
 * @param filename   - Download filename (e.g., "mesh.obj")
 */
export function downloadOBJ(objContent: string, filename: string): void {
  const blob = new Blob([objContent], { type: 'text/plain' });
  const url = URL.createObjectURL(blob);

  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  link.style.display = 'none';

  document.body.appendChild(link);
  link.click();

  // Cleanup
  setTimeout(() => {
    document.body.removeChild(link);
    URL.revokeObjectURL(url);
  }, 100);
}
