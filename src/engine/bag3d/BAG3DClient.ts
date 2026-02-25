/**
 * 3D BAG API Client
 *
 * Fetches 3D building models from the 3dbag.nl API (CityJSONFeatures format).
 * The API uses EPSG:7415 (RD New + NAP height) coordinates.
 * Bbox is 2D RD New (same X/Y as EPSG:28992).
 */

export interface RDBBox {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export interface CityJSONFeature {
  id: string;
  type: string;
  CityObjects: Record<string, CityObject>;
  vertices: number[][];
}

export interface CityJSONFeatureCollection {
  type: string;
  features: CityJSONFeature[];
  metadata: {
    transform: {
      scale: [number, number, number];
      translate: [number, number, number];
    };
    [key: string]: any;
  };
  links?: { rel: string; href: string; type?: string }[];
  numberMatched?: number;
  numberReturned?: number;
}

export interface CityObject {
  type: string;
  attributes?: Record<string, any>;
  geometry: CityGeometry[];
  children?: string[];
  parents?: string[];
}

export interface CityGeometry {
  type: string;
  lod: string;
  boundaries: any; // nested arrays, depth depends on geometry type
  semantics?: {
    surfaces: { type: string }[];
    values: any[];
  };
}

export interface FetchProgress {
  loaded: number;
  total: number | null;
  status: string;
}

// Use Vite dev proxy to avoid CORS issues; fall back to direct URL in production
const API_BASE = import.meta.env.DEV
  ? '/api/3dbag/collections/pand/items'
  : 'https://api.3dbag.nl/collections/pand/items';

/**
 * Fetch buildings within a bounding box (RD New X/Y coordinates).
 * The transform (scale + translate) is shared across all features in a response page.
 */
export async function fetchBuildings(
  bbox: RDBBox,
  _lod: string = '1.2',
  onProgress?: (progress: FetchProgress) => void,
  limit: number = 50
): Promise<{ features: CityJSONFeature[]; transform: { scale: [number, number, number]; translate: [number, number, number] } }> {
  const allFeatures: CityJSONFeature[] = [];
  let sharedTransform = { scale: [1, 1, 1] as [number, number, number], translate: [0, 0, 0] as [number, number, number] };

  let url: string | null =
    `${API_BASE}?bbox=${bbox.minX},${bbox.minY},${bbox.maxX},${bbox.maxY}&limit=${limit}`;

  let page = 0;
  while (url) {
    page++;
    onProgress?.({
      loaded: allFeatures.length,
      total: null,
      status: `Fetching page ${page}...`,
    });

    const response: Response = await fetch(url);
    if (!response.ok) {
      throw new Error(`3D BAG API error: ${response.status} ${response.statusText}`);
    }

    const data: CityJSONFeatureCollection = await response.json();

    // Extract the shared transform from metadata
    if (data.metadata?.transform) {
      sharedTransform = data.metadata.transform;
    }

    // Collect features
    if (data.features && Array.isArray(data.features)) {
      for (const feature of data.features) {
        allFeatures.push(feature);
      }
    }

    // Check for pagination (next link)
    url = null;
    if (data.links) {
      const nextLink = data.links.find((l) => l.rel === 'next');
      if (nextLink?.href) {
        // Rewrite absolute API URLs through the dev proxy
        url = import.meta.env.DEV
          ? nextLink.href.replace('https://api.3dbag.nl', '/api/3dbag')
          : nextLink.href;
      }
    }

    // Safety limit to prevent infinite pagination
    if (page >= 10) break;
  }

  onProgress?.({
    loaded: allFeatures.length,
    total: allFeatures.length,
    status: 'Done',
  });

  return { features: allFeatures, transform: sharedTransform };
}
