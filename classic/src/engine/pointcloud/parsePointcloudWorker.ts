import type { ParsedPointcloud } from './LASParser';
import type { WorkerRequest, WorkerResponse } from './workerProtocol';

export type ProgressCallback = (phase: string, percent: number) => void;

const WORKER_EXTENSIONS = new Set(['.las', '.laz', '.pts', '.ply', '.xyz', '.asc', '.txt', '.csv', '.obj', '.pcd', '.ptx', '.off', '.stl', '.dxf']);

let worker: Worker | null = null;

function getWorker(): Worker {
  if (!worker) {
    worker = new Worker(new URL('./pointcloud.worker.ts', import.meta.url), { type: 'module' });
  }
  return worker;
}

function parseInWorker(
  extension: string,
  buffer: ArrayBuffer,
  onProgress?: ProgressCallback,
): Promise<ParsedPointcloud> {
  return new Promise((resolve, reject) => {
    const id = crypto.randomUUID();
    const w = getWorker();

    const handler = (e: MessageEvent<WorkerResponse>) => {
      const msg = e.data;
      if (msg.id !== id) return;

      switch (msg.type) {
        case 'progress':
          onProgress?.(msg.phase, msg.percent);
          break;
        case 'result':
          w.removeEventListener('message', handler);
          w.removeEventListener('error', errorHandler);
          resolve(msg.data);
          break;
        case 'error':
          w.removeEventListener('message', handler);
          w.removeEventListener('error', errorHandler);
          reject(new Error(msg.message));
          break;
      }
    };

    const errorHandler = (e: ErrorEvent) => {
      w.removeEventListener('message', handler);
      w.removeEventListener('error', errorHandler);
      reject(new Error(e.message || 'Worker error'));
    };

    w.addEventListener('message', handler);
    w.addEventListener('error', errorHandler);

    const msg: WorkerRequest = { type: 'parse', id, extension, buffer };
    w.postMessage(msg, [buffer]);
  });
}

export async function parsePointcloud(
  extension: string,
  buffer: ArrayBuffer,
  onProgress?: ProgressCallback,
): Promise<ParsedPointcloud> {
  if (WORKER_EXTENSIONS.has(extension)) {
    return parseInWorker(extension, buffer, onProgress);
  }

  // E57 uses DOMParser (unavailable in workers) — parse on main thread
  if (extension === '.e57') {
    onProgress?.('Parsing E57...', 10);
    const { parseE57 } = await import('./E57Parser');
    const result = parseE57(buffer);
    onProgress?.('Complete', 100);
    return result;
  }

  throw new Error(`Unsupported file format: ${extension}`);
}
