/**
 * In-memory store for browser-parsed pointcloud data.
 *
 * Since the Zustand store can't hold typed arrays efficiently,
 * parsed pointcloud geometry is stored here and referenced by ID.
 */

import type { ParsedPointcloud } from './LASParser';

const store = new Map<string, ParsedPointcloud>();

/**
 * Object URLs for embedded photos, created on first use and revoked together
 * with the pointcloud. Keeping them here rather than in React state ties their
 * lifetime to the data they point at instead of to component mounts.
 */
const imageUrls = new Map<string, string[]>();

export function setBrowserPointcloud(id: string, data: ParsedPointcloud): void {
  revokeImageUrls(id);
  store.set(id, data);
}

export function getBrowserPointcloud(id: string): ParsedPointcloud | undefined {
  return store.get(id);
}

/** Blob URLs for `images` of a pointcloud, in the same order; empty if none. */
export function getBrowserImageUrls(id: string): string[] {
  const cached = imageUrls.get(id);
  if (cached) return cached;
  const images = store.get(id)?.images ?? [];
  const urls = images.map((img) => URL.createObjectURL(new Blob([img.data], { type: img.mime })));
  imageUrls.set(id, urls);
  return urls;
}

function revokeImageUrls(id: string): void {
  const urls = imageUrls.get(id);
  if (!urls) return;
  urls.forEach((u) => URL.revokeObjectURL(u));
  imageUrls.delete(id);
}

export function removeBrowserPointcloud(id: string): void {
  revokeImageUrls(id);
  store.delete(id);
}
