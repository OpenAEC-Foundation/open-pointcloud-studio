/**
 * In-memory store for browser-parsed pointcloud data.
 *
 * Since the Zustand store can't hold typed arrays efficiently,
 * parsed pointcloud geometry is stored here and referenced by ID.
 */

import type { ParsedPointcloud } from './LASParser';

const store = new Map<string, ParsedPointcloud>();

export function setBrowserPointcloud(id: string, data: ParsedPointcloud): void {
  store.set(id, data);
}

export function getBrowserPointcloud(id: string): ParsedPointcloud | undefined {
  return store.get(id);
}

export function removeBrowserPointcloud(id: string): void {
  store.delete(id);
}
