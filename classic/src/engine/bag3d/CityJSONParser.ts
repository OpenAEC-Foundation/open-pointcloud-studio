/**
 * CityJSON to Three.js Geometry Parser
 *
 * Parses CityJSONFeatures (from 3D BAG API) into Three.js BufferGeometry.
 * Handles coordinate transformation from RD New (EPSG:7415) to local Three.js coordinates.
 */

import * as THREE from 'three';
import proj4 from 'proj4';
import type { CityJSONFeature } from './BAG3DClient';

// Define RD New (EPSG:28992) projection
const RD_NEW = '+proj=sterea +lat_0=52.15616055555555 +lon_0=5.38763888888889 +k=0.9999079 +x_0=155000 +y_0=463000 +ellps=bessel +towgs84=565.417,50.3319,465.552,-0.398957,0.343988,-1.8774,4.0725 +units=m +no_defs';
const WGS84 = 'EPSG:4326';

proj4.defs('EPSG:28992', RD_NEW);

/**
 * Convert WGS84 (lon, lat) to RD New (x, y)
 */
export function wgs84ToRD(lon: number, lat: number): [number, number] {
  return proj4(WGS84, 'EPSG:28992', [lon, lat]) as [number, number];
}

/**
 * Convert RD New (x, y) to WGS84 (lon, lat)
 */
export function rdToWGS84(x: number, y: number): [number, number] {
  return proj4('EPSG:28992', WGS84, [x, y]) as [number, number];
}

/**
 * Ear-clipping triangulation for a polygon ring.
 */
function triangulateRing(ring: number[], vertices: number[][]): number[][] {
  if (ring.length < 3) return [];
  if (ring.length === 3) return [[ring[0], ring[1], ring[2]]];

  const triangles: number[][] = [];
  const remaining = [...ring];

  let maxIter = remaining.length * 3;
  while (remaining.length > 3 && maxIter-- > 0) {
    let earFound = false;
    for (let i = 0; i < remaining.length; i++) {
      const prev = (i - 1 + remaining.length) % remaining.length;
      const next = (i + 1) % remaining.length;

      const a = remaining[prev];
      const b = remaining[i];
      const c = remaining[next];

      const va = vertices[a];
      const vb = vertices[b];
      const vc = vertices[c];
      if (!va || !vb || !vc) continue;

      // Cross product in XY plane (RD coordinates)
      const cross =
        (vb[0] - va[0]) * (vc[1] - va[1]) - (vb[1] - va[1]) * (vc[0] - va[0]);

      if (cross >= 0) {
        let pointInside = false;
        for (let j = 0; j < remaining.length; j++) {
          if (j === prev || j === i || j === next) continue;
          const vp = vertices[remaining[j]];
          if (vp && isPointInTriangle2D(vp, va, vb, vc)) {
            pointInside = true;
            break;
          }
        }

        if (!pointInside) {
          triangles.push([a, b, c]);
          remaining.splice(i, 1);
          earFound = true;
          break;
        }
      }
    }

    if (!earFound) {
      // Fallback: fan triangulation from first vertex
      for (let i = 1; i < remaining.length - 1; i++) {
        triangles.push([remaining[0], remaining[i], remaining[i + 1]]);
      }
      break;
    }
  }

  if (remaining.length === 3) {
    triangles.push([remaining[0], remaining[1], remaining[2]]);
  }

  return triangles;
}

function isPointInTriangle2D(p: number[], a: number[], b: number[], c: number[]): boolean {
  const d1 = sign2D(p, a, b);
  const d2 = sign2D(p, b, c);
  const d3 = sign2D(p, c, a);
  const hasNeg = d1 < 0 || d2 < 0 || d3 < 0;
  const hasPos = d1 > 0 || d2 > 0 || d3 > 0;
  return !(hasNeg && hasPos);
}

function sign2D(p1: number[], p2: number[], p3: number[]): number {
  return (p1[0] - p3[0]) * (p2[1] - p3[1]) - (p2[0] - p3[0]) * (p1[1] - p3[1]);
}

export interface ParsedBuildings {
  geometry: THREE.BufferGeometry;
  buildingCount: number;
}

/**
 * Extract flat polygon rings from CityJSON boundaries.
 * Boundaries format depends on geometry type:
 *   - MultiSurface: [face][ring] → ring is number[]
 *   - Solid: [shell][face][ring] → ring is number[]
 *   - CompositeSolid: [solid][shell][face][ring]
 */
function extractRingsFromBoundaries(boundaries: any, geomType: string): number[][] {
  const rings: number[][] = [];

  if (geomType === 'MultiSurface' || geomType === 'CompositeSurface') {
    // boundaries = [ [ring, ...], [ring, ...], ... ]  (face → rings)
    for (const face of boundaries) {
      if (Array.isArray(face) && face.length > 0) {
        const outerRing = face[0];
        if (Array.isArray(outerRing) && outerRing.length >= 3 && typeof outerRing[0] === 'number') {
          rings.push(outerRing);
        }
      }
    }
  } else if (geomType === 'Solid') {
    // boundaries = [ [face, ...], ... ]  (shell → faces → rings)
    for (const shell of boundaries) {
      if (!Array.isArray(shell)) continue;
      for (const face of shell) {
        if (Array.isArray(face) && face.length > 0) {
          const outerRing = face[0];
          if (Array.isArray(outerRing) && outerRing.length >= 3 && typeof outerRing[0] === 'number') {
            rings.push(outerRing);
          }
        }
      }
    }
  } else if (geomType === 'CompositeSolid') {
    for (const solid of boundaries) {
      if (!Array.isArray(solid)) continue;
      for (const shell of solid) {
        if (!Array.isArray(shell)) continue;
        for (const face of shell) {
          if (Array.isArray(face) && face.length > 0) {
            const outerRing = face[0];
            if (Array.isArray(outerRing) && outerRing.length >= 3 && typeof outerRing[0] === 'number') {
              rings.push(outerRing);
            }
          }
        }
      }
    }
  }

  return rings;
}

/**
 * Parse CityJSONFeatures into a merged Three.js BufferGeometry.
 * The transform (scale/translate) comes from the API metadata and applies to all features.
 */
export function parseCityJSON(
  features: CityJSONFeature[],
  transform: { scale: [number, number, number]; translate: [number, number, number] },
  targetLod?: string
): ParsedBuildings {
  const allPositions: number[] = [];
  const allIndices: number[] = [];
  const allColors: number[] = [];
  let globalVertexOffset = 0;
  let buildingCount = 0;

  const scale = transform.scale;
  const translate = transform.translate;

  // First pass: compute center of all transformed vertices for centering
  let sumX = 0, sumY = 0, sumZ = 0;
  let totalVerts = 0;

  for (const feature of features) {
    for (const v of feature.vertices) {
      sumX += v[0] * scale[0] + translate[0];
      sumY += v[1] * scale[1] + translate[1];
      sumZ += v[2] * scale[2] + translate[2];
      totalVerts++;
    }
  }

  const centerX = totalVerts > 0 ? sumX / totalVerts : 0;
  const centerY = totalVerts > 0 ? sumY / totalVerts : 0;
  const centerZ = totalVerts > 0 ? sumZ / totalVerts : 0;

  // Second pass: build geometry per feature
  for (const feature of features) {
    // Transform vertices: apply scale+translate, center, then convert to Three.js coords
    // RD New: X=east, Y=north, Z=height(NAP)
    // Three.js: X=east, Y=up(height), Z=south(-north)
    const transformedVerts: number[][] = feature.vertices.map((v) => {
      const rx = v[0] * scale[0] + translate[0] - centerX;
      const ry = v[1] * scale[1] + translate[1] - centerY;
      const rz = v[2] * scale[2] + translate[2] - centerZ;
      return [rx, rz, -ry]; // X=east, Y=height, Z=-north
    });

    const featureVertStart = globalVertexOffset;

    // Add transformed vertices to global arrays
    for (const v of transformedVerts) {
      allPositions.push(v[0], v[1], v[2]);
    }
    globalVertexOffset += transformedVerts.length;

    // Process all CityObjects in this feature
    for (const cityObj of Object.values(feature.CityObjects)) {
      const isBuilding = cityObj.type === 'Building' || cityObj.type === 'BuildingPart' || cityObj.type === 'BuildingInstallation';
      if (isBuilding) buildingCount++;

      const color = getBuildingColor(cityObj.type);

      for (const geom of cityObj.geometry) {
        // Filter by LoD if specified
        if (targetLod && geom.lod !== targetLod) continue;

        const rings = extractRingsFromBoundaries(geom.boundaries, geom.type);

        for (const ring of rings) {
          const tris = triangulateRing(ring, transformedVerts);
          for (const [a, b, c] of tris) {
            allIndices.push(featureVertStart + a, featureVertStart + b, featureVertStart + c);
          }
        }
      }

      // Color all vertices of this feature (overwritten per CityObject, but that's fine)
      for (let i = featureVertStart; i < globalVertexOffset; i++) {
        const ci = i * 3;
        // Only set if not yet in the array (extend as needed)
        while (allColors.length <= ci + 2) allColors.push(0.7);
        allColors[ci] = color[0];
        allColors[ci + 1] = color[1];
        allColors[ci + 2] = color[2];
      }
    }
  }

  // Ensure colors array matches positions
  while (allColors.length < allPositions.length) {
    allColors.push(0.7);
  }

  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute('position', new THREE.BufferAttribute(new Float32Array(allPositions), 3));
  geometry.setAttribute('color', new THREE.BufferAttribute(new Float32Array(allColors), 3));
  geometry.setIndex(new THREE.BufferAttribute(new Uint32Array(allIndices), 1));
  geometry.computeVertexNormals();
  geometry.computeBoundingSphere();

  return { geometry, buildingCount };
}

function getBuildingColor(type: string): [number, number, number] {
  switch (type) {
    case 'Building':
      return [0.75, 0.72, 0.65];
    case 'BuildingPart':
      return [0.68, 0.65, 0.58];
    case 'BuildingInstallation':
      return [0.6, 0.6, 0.7];
    default:
      return [0.7, 0.7, 0.7];
  }
}
