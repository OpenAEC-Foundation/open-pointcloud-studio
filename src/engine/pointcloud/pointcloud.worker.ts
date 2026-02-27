import type { WorkerRequest, WorkerResponse } from './workerProtocol';
import { parseLAS } from './LASParser';
import { parsePTS } from './PTSParser';
import { parsePLY } from './PLYParser';
import { parseXYZ } from './XYZParser';
import { parseOBJ } from './OBJParser';
import { parsePCD } from './PCDParser';
import { parsePTX } from './PTXParser';
import { parseOFF } from './OFFParser';
import { parseSTL } from './STLParser';
import { parseDXF } from './DXFParser';

declare function postMessage(message: unknown, options?: StructuredSerializeOptions): void;

function post(msg: WorkerResponse, transfer?: Transferable[]) {
  if (transfer) {
    postMessage(msg, { transfer });
  } else {
    postMessage(msg);
  }
}

self.onmessage = async (e: MessageEvent<WorkerRequest>) => {
  const req = e.data;
  if (req.type !== 'parse') return;

  const { id, extension, buffer } = req;

  post({ type: 'progress', id, phase: 'Parsing...', percent: 10 });
  console.time(`[worker] parse ${extension}`);

  try {
    let parsed;

    switch (extension) {
      case '.las':
        parsed = parseLAS(buffer);
        break;
      case '.laz': {
        const { parseLAZ } = await import('./LAZParser');
        parsed = await parseLAZ(buffer);
        break;
      }
      case '.pts':
        parsed = parsePTS(buffer);
        break;
      case '.ply':
        parsed = parsePLY(buffer);
        break;
      case '.xyz':
      case '.asc':
      case '.txt':
      case '.csv':
        parsed = parseXYZ(buffer);
        break;
      case '.obj':
        parsed = parseOBJ(buffer);
        break;
      case '.pcd':
        parsed = parsePCD(buffer);
        break;
      case '.ptx':
        parsed = parsePTX(buffer);
        break;
      case '.off':
        parsed = parseOFF(buffer);
        break;
      case '.stl':
        parsed = parseSTL(buffer);
        break;
      case '.dxf':
        parsed = parseDXF(buffer);
        break;
      default:
        post({ type: 'error', id, message: `Unsupported format in worker: ${extension}` });
        return;
    }

    console.timeEnd(`[worker] parse ${extension}`);
    console.log(`[worker] parsed ${parsed.positions.length / 3} points`);
    post({ type: 'progress', id, phase: 'Transferring data...', percent: 90 });

    const transferables: Transferable[] = [
      parsed.positions.buffer,
      parsed.colors.buffer,
      parsed.intensities.buffer,
      parsed.classifications.buffer,
    ];
    if (parsed.indices) {
      transferables.push(parsed.indices.buffer);
    }

    post({ type: 'result', id, data: parsed }, transferables);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    post({ type: 'error', id, message });
  }
};
