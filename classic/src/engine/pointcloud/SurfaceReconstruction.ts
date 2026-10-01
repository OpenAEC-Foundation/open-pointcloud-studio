/**
 * Surface Reconstruction — Greedy Projection Triangulation
 *
 * Reconstructs a triangle mesh from an unstructured pointcloud.
 * Uses a grid-based spatial index for neighbor queries, PCA-based
 * normal estimation, and greedy projection triangulation onto
 * local tangent planes.
 *
 * Browser-compatible, no native dependencies.
 */

// ============================================================================
// Spatial Index (uniform grid)
// ============================================================================

class SpatialGrid {
  private cells: Map<string, number[]> = new Map();
  private cellSize: number;

  constructor(cellSize: number) {
    this.cellSize = cellSize;
  }

  private key(ix: number, iy: number, iz: number): string {
    return `${ix},${iy},${iz}`;
  }

  insert(index: number, x: number, y: number, z: number): void {
    const ix = Math.floor(x / this.cellSize);
    const iy = Math.floor(y / this.cellSize);
    const iz = Math.floor(z / this.cellSize);
    const k = this.key(ix, iy, iz);
    let cell = this.cells.get(k);
    if (!cell) {
      cell = [];
      this.cells.set(k, cell);
    }
    cell.push(index);
  }

  /**
   * Find k-nearest neighbors of the given point.
   * Searches progressively larger neighborhoods in the grid.
   */
  findKNearest(
    x: number,
    y: number,
    z: number,
    k: number,
    positions: Float32Array,
    excludeIndex: number
  ): number[] {
    const cx = Math.floor(x / this.cellSize);
    const cy = Math.floor(y / this.cellSize);
    const cz = Math.floor(z / this.cellSize);

    // Start with radius 1 and expand if needed
    let candidates: { idx: number; dist: number }[] = [];
    let radius = 1;
    const maxRadius = 5;

    while (candidates.length < k && radius <= maxRadius) {
      candidates = [];
      for (let dx = -radius; dx <= radius; dx++) {
        for (let dy = -radius; dy <= radius; dy++) {
          for (let dz = -radius; dz <= radius; dz++) {
            const cell = this.cells.get(this.key(cx + dx, cy + dy, cz + dz));
            if (!cell) continue;
            for (const idx of cell) {
              if (idx === excludeIndex) continue;
              const px = positions[idx * 3] - x;
              const py = positions[idx * 3 + 1] - y;
              const pz = positions[idx * 3 + 2] - z;
              candidates.push({ idx, dist: px * px + py * py + pz * pz });
            }
          }
        }
      }
      radius++;
    }

    candidates.sort((a, b) => a.dist - b.dist);
    return candidates.slice(0, k).map((c) => c.idx);
  }
}

// ============================================================================
// Normal Estimation via PCA
// ============================================================================

/**
 * Estimate normals for all points using PCA on k-nearest neighborhoods.
 * Returns a Float32Array of normals (nx, ny, nz per point).
 */
function estimateNormals(
  positions: Float32Array,
  grid: SpatialGrid,
  k: number
): Float32Array {
  const numPoints = positions.length / 3;
  const normals = new Float32Array(numPoints * 3);

  for (let i = 0; i < numPoints; i++) {
    const px = positions[i * 3];
    const py = positions[i * 3 + 1];
    const pz = positions[i * 3 + 2];

    const neighbors = grid.findKNearest(px, py, pz, k, positions, i);
    if (neighbors.length < 3) {
      // Default to up vector if not enough neighbors
      normals[i * 3 + 1] = 1;
      continue;
    }

    // Compute centroid of neighborhood
    let mx = 0, my = 0, mz = 0;
    for (const ni of neighbors) {
      mx += positions[ni * 3];
      my += positions[ni * 3 + 1];
      mz += positions[ni * 3 + 2];
    }
    const n = neighbors.length;
    mx /= n; my /= n; mz /= n;

    // Compute covariance matrix (symmetric 3x3)
    let cxx = 0, cxy = 0, cxz = 0;
    let cyy = 0, cyz = 0, czz = 0;

    for (const ni of neighbors) {
      const dx = positions[ni * 3] - mx;
      const dy = positions[ni * 3 + 1] - my;
      const dz = positions[ni * 3 + 2] - mz;
      cxx += dx * dx; cxy += dx * dy; cxz += dx * dz;
      cyy += dy * dy; cyz += dy * dz; czz += dz * dz;
    }

    // Find the eigenvector with smallest eigenvalue using power iteration
    // on the inverse (or just find the normal as the cross product of the
    // two largest eigenvectors). For simplicity, use iterative approach.
    const normal = smallestEigenvector(cxx, cxy, cxz, cyy, cyz, czz);

    // Orient normal: ensure it points "outward" (use heuristic: dot with
    // vector from centroid to point should be positive)
    const vx = px - mx, vy = py - my, vz = pz - mz;
    const dot = normal[0] * vx + normal[1] * vy + normal[2] * vz;
    const sign = dot < 0 ? -1 : 1;

    normals[i * 3] = normal[0] * sign;
    normals[i * 3 + 1] = normal[1] * sign;
    normals[i * 3 + 2] = normal[2] * sign;
  }

  return normals;
}

/**
 * Find the eigenvector corresponding to the smallest eigenvalue of
 * a 3x3 symmetric matrix using the analytic cubic formula.
 */
function smallestEigenvector(
  a: number, b: number, c: number,
  d: number, e: number, f: number
): [number, number, number] {
  // Matrix:
  // | a b c |
  // | b d e |
  // | c e f |

  // Characteristic polynomial: det(M - lambda*I) = 0
  // -lambda^3 + (a+d+f)*lambda^2 - (ad+af+df-b^2-c^2-e^2)*lambda + det = 0

  const trace = a + d + f;
  const q = trace / 3;

  const p1 = b * b + c * c + e * e;
  const p2 = (a - q) * (a - q) + (d - q) * (d - q) + (f - q) * (f - q) + 2 * p1;
  const p = Math.sqrt(p2 / 6);

  if (p < 1e-15) {
    // Matrix is essentially scalar, any vector works
    return [0, 1, 0];
  }

  // B = (1/p) * (M - q*I)
  const invP = 1 / p;
  const b00 = (a - q) * invP, b01 = b * invP, b02 = c * invP;
  const b11 = (d - q) * invP, b12 = e * invP;
  const b22 = (f - q) * invP;

  // det(B) / 2
  const detB = b00 * (b11 * b22 - b12 * b12) -
               b01 * (b01 * b22 - b12 * b02) +
               b02 * (b01 * b12 - b11 * b02);
  const halfDetB = detB / 2;

  // Clamp to [-1, 1] for acos
  const phi = Math.acos(Math.max(-1, Math.min(1, halfDetB))) / 3;

  // Eigenvalues (sorted: eig0 >= eig1 >= eig2)
  const eig2 = q + 2 * p * Math.cos(phi + (2 * Math.PI / 3)); // smallest

  // Find eigenvector for eig2 using (M - eig2*I) null space
  const m00 = a - eig2, m01 = b, m02 = c;
  const m10 = b, m11 = d - eig2, m12 = e;
  const m20 = c, m21 = e, m22 = f - eig2;

  // Cross product of two rows gives the null space vector
  let nx = m01 * m12 - m02 * m11;
  let ny = m02 * m10 - m00 * m12;
  let nz = m00 * m11 - m01 * m10;

  let len = Math.sqrt(nx * nx + ny * ny + nz * nz);

  if (len < 1e-12) {
    // Try another pair of rows
    nx = m11 * m22 - m12 * m21;
    ny = m12 * m20 - m10 * m22;
    nz = m10 * m21 - m11 * m20;
    len = Math.sqrt(nx * nx + ny * ny + nz * nz);
  }

  if (len < 1e-12) {
    // Try third pair
    nx = m01 * m22 - m02 * m21;
    ny = m02 * m20 - m00 * m22;
    nz = m00 * m21 - m01 * m20;
    len = Math.sqrt(nx * nx + ny * ny + nz * nz);
  }

  if (len < 1e-12) {
    return [0, 1, 0]; // fallback
  }

  return [nx / len, ny / len, nz / len];
}

// ============================================================================
// Greedy Projection Triangulation
// ============================================================================

/**
 * Triangulate using greedy projection onto local tangent planes.
 *
 * For each point, project its k-nearest neighbors onto the tangent plane
 * defined by the estimated normal, then perform a 2D Delaunay-like fan
 * triangulation sorted by angle.
 *
 * Produces a set of triangles, deduplicating by using a canonical edge set.
 */
function greedyProjectionTriangulate(
  positions: Float32Array,
  normals: Float32Array,
  grid: SpatialGrid,
  k: number,
  maxEdgeLength: number
): Uint32Array {
  const numPoints = positions.length / 3;
  const maxEdgeSq = maxEdgeLength * maxEdgeLength;

  const triangles: number[] = [];

  function triKey(a: number, b: number, c: number): string {
    const sorted = [a, b, c].sort((x, y) => x - y);
    return `${sorted[0]},${sorted[1]},${sorted[2]}`;
  }

  const addedTriangles = new Set<string>();

  for (let i = 0; i < numPoints; i++) {
    const px = positions[i * 3];
    const py = positions[i * 3 + 1];
    const pz = positions[i * 3 + 2];

    const nx = normals[i * 3];
    const ny = normals[i * 3 + 1];
    const nz = normals[i * 3 + 2];

    const neighbors = grid.findKNearest(px, py, pz, k, positions, i);
    if (neighbors.length < 2) continue;

    // Build a local coordinate frame on the tangent plane
    // u = arbitrary vector perpendicular to normal
    let ux: number, uy: number, uz: number;
    if (Math.abs(nx) < 0.9) {
      // cross(normal, [1,0,0])
      ux = 0; uy = nz; uz = -ny;
    } else {
      // cross(normal, [0,1,0])
      ux = -nz; uy = 0; uz = nx;
    }
    let uLen = Math.sqrt(ux * ux + uy * uy + uz * uz);
    if (uLen < 1e-12) continue;
    ux /= uLen; uy /= uLen; uz /= uLen;

    // v = cross(normal, u)
    const vx = ny * uz - nz * uy;
    const vy = nz * ux - nx * uz;
    const vz = nx * uy - ny * ux;

    // Project neighbors onto tangent plane and compute angles
    const projected: { idx: number; angle: number; distSq: number }[] = [];
    for (const ni of neighbors) {
      const dx = positions[ni * 3] - px;
      const dy = positions[ni * 3 + 1] - py;
      const dz = positions[ni * 3 + 2] - pz;
      const distSq = dx * dx + dy * dy + dz * dz;

      if (distSq > maxEdgeSq) continue;

      // Project onto tangent plane
      const u2d = dx * ux + dy * uy + dz * uz;
      const v2d = dx * vx + dy * vy + dz * vz;
      const angle = Math.atan2(v2d, u2d);

      projected.push({ idx: ni, angle, distSq });
    }

    if (projected.length < 2) continue;

    // Sort by angle
    projected.sort((a, b) => a.angle - b.angle);

    // Create fan triangles: for consecutive neighbor pairs, create a triangle
    for (let j = 0; j < projected.length; j++) {
      const jNext = (j + 1) % projected.length;
      const a = i;
      const b = projected[j].idx;
      const c = projected[jNext].idx;

      if (b === c) continue;

      // Check edge lengths
      const bcx = positions[c * 3] - positions[b * 3];
      const bcy = positions[c * 3 + 1] - positions[b * 3 + 1];
      const bcz = positions[c * 3 + 2] - positions[b * 3 + 2];
      if (bcx * bcx + bcy * bcy + bcz * bcz > maxEdgeSq) continue;

      // Check angular gap is reasonable (skip if > 90 degrees between neighbors)
      let angleDiff = projected[jNext].angle - projected[j].angle;
      if (angleDiff < 0) angleDiff += 2 * Math.PI;
      if (angleDiff > Math.PI / 2) continue;

      // Deduplicate triangle
      const tk = triKey(a, b, c);
      if (addedTriangles.has(tk)) continue;
      addedTriangles.add(tk);

      // Ensure consistent winding (normal should agree with face normal)
      const e1x = positions[b * 3] - px;
      const e1y = positions[b * 3 + 1] - py;
      const e1z = positions[b * 3 + 2] - pz;
      const e2x = positions[c * 3] - px;
      const e2y = positions[c * 3 + 1] - py;
      const e2z = positions[c * 3 + 2] - pz;
      const fnx = e1y * e2z - e1z * e2y;
      const fny = e1z * e2x - e1x * e2z;
      const fnz = e1x * e2y - e1y * e2x;
      const fDot = fnx * nx + fny * ny + fnz * nz;

      if (fDot >= 0) {
        triangles.push(a, b, c);
      } else {
        triangles.push(a, c, b);
      }
    }
  }

  return new Uint32Array(triangles);
}

// ============================================================================
// Public API
// ============================================================================

export interface ReconstructionProgress {
  phase: string;
  percent: number;
}

export interface ReconstructionOptions {
  /** Number of nearest neighbors to use (default: 15) */
  kNeighbors?: number;
  /** Maximum edge length for triangles (auto-computed if not set) */
  maxEdgeLength?: number;
  /** Progress callback, called at key stages */
  onProgress?: (progress: ReconstructionProgress) => void;
  /** If set to true, the reconstruction will be cancelled */
  cancelled?: { value: boolean };
}

export interface ReconstructionResult {
  /** Triangle face indices (3 per triangle) */
  indices: Uint32Array;
  /** Per-vertex normals */
  normals: Float32Array;
}

/**
 * Reconstruct a triangle mesh from an unstructured pointcloud.
 *
 * Algorithm:
 * 1. Build a uniform grid spatial index
 * 2. For each point, find k nearest neighbors
 * 3. Estimate surface normals via PCA on neighborhoods
 * 4. Greedy projection triangulation: project neighborhoods onto
 *    tangent planes and create fan triangles
 *
 * @param positions - Float32Array of xyz positions (length = numPoints * 3)
 * @param options   - Optional parameters
 * @returns Triangle indices and per-vertex normals
 */
export async function reconstructSurface(
  positions: Float32Array,
  options?: ReconstructionOptions
): Promise<ReconstructionResult> {
  const k = options?.kNeighbors ?? 15;
  const onProgress = options?.onProgress;
  const cancelled = options?.cancelled;
  const numPoints = positions.length / 3;

  if (numPoints < 3) {
    throw new Error('Need at least 3 points for surface reconstruction');
  }

  const checkCancelled = () => {
    if (cancelled?.value) throw new Error('Reconstruction cancelled');
  };

  // Phase 1: Building spatial index (10%)
  onProgress?.({ phase: 'Building spatial index', percent: 10 });

  // Compute bounding box for auto cell size
  let minX = Infinity, minY = Infinity, minZ = Infinity;
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity;

  for (let i = 0; i < numPoints; i++) {
    const x = positions[i * 3];
    const y = positions[i * 3 + 1];
    const z = positions[i * 3 + 2];
    if (x < minX) minX = x; if (x > maxX) maxX = x;
    if (y < minY) minY = y; if (y > maxY) maxY = y;
    if (z < minZ) minZ = z; if (z > maxZ) maxZ = z;
  }

  const extent = Math.max(maxX - minX, maxY - minY, maxZ - minZ);
  // Cell size: roughly so that each cell has ~k points on average
  // For uniform distribution: cellSize = extent / cbrt(numPoints / k)
  const cellSize = extent / Math.cbrt(numPoints / k) || 1;

  // Auto-compute max edge length if not provided
  // Use ~2x the average nearest-neighbor distance estimate
  const maxEdgeLength = options?.maxEdgeLength ?? (cellSize * 2);

  // Build spatial index
  const grid = new SpatialGrid(cellSize);
  for (let i = 0; i < numPoints; i++) {
    grid.insert(i, positions[i * 3], positions[i * 3 + 1], positions[i * 3 + 2]);
  }

  // Yield to browser event loop periodically for large pointclouds
  await yieldToMain();
  checkCancelled();

  // Phase 2: Estimating normals (30-60%)
  onProgress?.({ phase: 'Estimating normals', percent: 30 });
  const normals = estimateNormals(positions, grid, k);
  onProgress?.({ phase: 'Estimating normals', percent: 60 });

  await yieldToMain();
  checkCancelled();

  // Phase 3: Triangulating (60-90%)
  onProgress?.({ phase: 'Triangulating', percent: 60 });
  const indices = greedyProjectionTriangulate(
    positions,
    normals,
    grid,
    k,
    maxEdgeLength
  );
  onProgress?.({ phase: 'Triangulating', percent: 90 });

  await yieldToMain();
  checkCancelled();

  // Phase 4: Finalizing (95%)
  onProgress?.({ phase: 'Finalizing', percent: 95 });

  if (indices.length === 0) {
    throw new Error(
      'Surface reconstruction produced no triangles. ' +
      'Try increasing kNeighbors or maxEdgeLength.'
    );
  }

  onProgress?.({ phase: 'Complete', percent: 100 });

  return { indices, normals };
}

/** Yield control to the browser event loop */
function yieldToMain(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}
