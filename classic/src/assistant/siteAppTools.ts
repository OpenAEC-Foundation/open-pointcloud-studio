/**
 * The `pointcloud_*` tools for the assistant of OpenAEC SuperCloud (browser build only, `window.__OPDS_APP__`).
 *
 * The site sends a tool call through its bridge to `run(name, args)` in the frame of the app. The tools read the loaded
 * clouds and set the view through `PointcloudHost` (pointcloudHost.ts: the store and the viewer); they change no point
 * data, so there is nothing to undo. No network, no popups, no messages to other windows. A refusal is an Error with a
 * short code; a value is plain JSON.
 *
 * Coordinates for the assistant are those of the file of the active cloud (X east, Y north, Z up). The scene holds
 * every cloud relative to its own centre with Y up: scene = (X - cx, Z - cz, -(Y - cy)).
 */
import type { PointcloudClipBox, PointcloudColorMode, PointcloudEntry } from '../state/slices/pointcloudSlice.ts';

export type Vec3 = [number, number, number];
export interface CloudData {
  positions: Float32Array;
  intensities: Float32Array;
  classifications: Float32Array;
  center: Vec3;
}
export interface PointcloudHost {
  state(): {
    pointclouds: PointcloudEntry[];
    activePointcloudId: string | null;
    pointcloudColorMode: PointcloudColorMode;
    pointcloudPointSize: number;
    edlEnabled: boolean;
    clipBox: PointcloudClipBox | null;
  };
  data(id: string): CloudData | undefined;
  setColorMode(mode: PointcloudColorMode): void;
  setPointSize(size: number): void;
  setVisible(id: string, visible: boolean): void;
  setActive(id: string): void;
  setClipBox(box: PointcloudClipBox | null): void;
  /** Camera in scene coordinates; null without a viewer. */
  camera(): { position: Vec3; target: Vec3; fov: number } | null;
  setCamera(position: Vec3, target: Vec3): boolean;
  /** The view as an image (PNG or JPEG data URL), longest side at most `max` px; null without WebGL. */
  capture(max: number, type: 'image/png' | 'image/jpeg'): string | null;
}
export interface SiteAppToolDefinition { name: string; description: string; inputSchema: Schema }
export interface SiteAppTools {
  tools(): string[];
  definitions(): Promise<SiteAppToolDefinition[]>;
  run(name: string, args?: Record<string, unknown>): Promise<unknown>;
}

// --- Argument schemas (a small subset of JSON Schema, checked here) -----------------------------------------------

type Schema =
  | { type: 'object'; properties: Record<string, Schema>; required?: string[]; additionalProperties: false; description?: string }
  | { type: 'number' | 'integer'; minimum?: number; maximum?: number; description?: string }
  | { type: 'string'; enum?: string[]; maxLength?: number; description?: string }
  | { type: 'boolean'; description?: string };

const object = (properties: Record<string, Schema>, required: string[] = [], description?: string): Schema =>
  ({ type: 'object', properties, ...(required.length ? { required } : {}), additionalProperties: false, ...(description ? { description } : {}) });
const number = (description?: string, minimum?: number, maximum?: number): Schema =>
  ({ type: 'number', ...(minimum !== undefined ? { minimum } : {}), ...(maximum !== undefined ? { maximum } : {}), ...(description ? { description } : {}) });
const point = (description: string) => object({ x: number(), y: number(), z: number() }, ['x', 'y', 'z'], description);
const choice = (values: string[], description?: string): Schema => ({ type: 'string', enum: values, ...(description ? { description } : {}) });

function check(value: unknown, schema: Schema, path: string): void {
  const fail = (reason: string) => { throw new Error(`ongeldige_argumenten: ${path || 'argumenten'} ${reason}`); };
  if (schema.type === 'object') {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) fail('moet een object zijn');
    const record = value as Record<string, unknown>;
    for (const key of schema.required ?? []) if (record[key] === undefined) fail(`mist ${key}`);
    for (const [key, item] of Object.entries(record)) {
      if (!Object.prototype.hasOwnProperty.call(schema.properties, key)) fail(`kent ${key} niet`);
      if (item !== undefined) check(item, schema.properties[key], path ? `${path}.${key}` : key);
    }
  } else if (schema.type === 'number' || schema.type === 'integer') {
    if (typeof value !== 'number' || !Number.isFinite(value)) fail('moet een getal zijn');
    const n = value as number;
    if (schema.type === 'integer' && !Number.isInteger(n)) fail('moet een geheel getal zijn');
    if (schema.minimum !== undefined && n < schema.minimum) fail(`moet minstens ${schema.minimum} zijn`);
    if (schema.maximum !== undefined && n > schema.maximum) fail(`mag hoogstens ${schema.maximum} zijn`);
  } else if (schema.type === 'string') {
    if (typeof value !== 'string') fail('moet tekst zijn');
    if (schema.enum && !schema.enum.includes(value as string)) fail(`moet een van ${schema.enum.join(', ')} zijn`);
    if (schema.maxLength !== undefined && (value as string).length > schema.maxLength) fail('is te lang');
  } else if (typeof value !== 'boolean') fail('moet true of false zijn');
}

const views = ['passend', 'boven', 'onder', 'voor', 'achter', 'links', 'rechts', 'iso'];
const modes: Record<string, PointcloudColorMode> = { rgb: 'rgb', intensiteit: 'intensity', hoogte: 'elevation', classificatie: 'classification' };
const modeNames: Record<PointcloudColorMode, string> = { rgb: 'rgb', intensity: 'intensiteit', elevation: 'hoogte', classification: 'classificatie' };
const cloud: Schema = { type: 'string', maxLength: 300, description: 'id of bestandsnaam; standaard de actieve wolk' };

const definitions: SiteAppToolDefinition[] = [
  { name: 'pointcloud_stand', description: 'Stand van de app: de geladen wolken (id, bestand, aantal punten, begrenzing in X/Y/Z met Z omhoog, attributen RGB/intensiteit/classificatie, zichtbaar, actief), kleurmodus, puntgrootte, camera en doorsnede.',
    inputSchema: object({}) },
  { name: 'pointcloud_camera', description: "Zet het aanzicht: 'passend' (alles in beeld), 'boven', 'onder', 'voor' (vanuit het zuiden), 'achter', 'links' (vanuit het westen), 'rechts' of 'iso'. Of geef positie en doel in X/Y/Z.",
    inputSchema: object({ aanzicht: choice(views), positie: point('camerapositie'), doel: point('punt waar de camera naar kijkt') }) },
  { name: 'pointcloud_meten', description: 'Meet tussen twee punten in X/Y/Z: afstand, horizontale afstand en hoogteverschil. Met vastklikken eerst naar het dichtstbijzijnde punt van de zichtbare wolken.',
    inputSchema: object({ van: point('beginpunt'), naar: point('eindpunt'), vastklikken: { type: 'boolean' } }, ['van', 'naar']) },
  { name: 'pointcloud_hoogte', description: 'Hoogte (Z) van de punten binnen een straal rond (x, y): laagste, hoogste, gemiddelde en aantal. Optioneel alleen één ASPRS-classificatie (2 = maaiveld, 6 = gebouw).',
    inputSchema: object({ x: number(), y: number(), straal: number('in meters, standaard 0,5', 0.01, 100), classificatie: { type: 'integer', minimum: 0, maximum: 255 } }, ['x', 'y']) },
  { name: 'pointcloud_kleuren', description: 'Kleur de punten op attribuut: rgb, intensiteit, hoogte of classificatie. Optioneel de puntgrootte (0,1–20).',
    inputSchema: object({ modus: choice(Object.keys(modes)), puntgrootte: number(undefined, 0.1, 20) }, ['modus']) },
  { name: 'pointcloud_doorsnede', description: "Toon alleen de punten in een clipbox: soort 'box' met min en max in X/Y/Z; soort 'snede' met as (x, y of z), waarde en dikte (standaard 0,5 m); soort 'uit' toont weer alles.",
    inputSchema: object({ soort: choice(['box', 'snede', 'uit']), min: point('hoek van de box'), max: point('tegenoverliggende hoek'), as: choice(['x', 'y', 'z']), waarde: number('positie van de snede op de as'), dikte: number('dikte van de snede in meters', 0.001, 1000) }, ['soort']) },
  { name: 'pointcloud_wolk', description: 'Maak een wolk zichtbaar of onzichtbaar, of maak hem actief (de coördinaten volgen de actieve wolk).',
    inputSchema: object({ wolk: { ...cloud, description: 'id of bestandsnaam' }, zichtbaar: { type: 'boolean' }, actief: { type: 'boolean' } }, ['wolk']) },
  { name: 'pointcloud_vastleggen', description: 'Een kleine schermafdruk van de 3D-weergave als data-URL, om te zien wat er in beeld is.',
    inputSchema: object({ maximaal: { type: 'integer', minimum: 64, maximum: 1024, description: 'langste zijde in px, standaard 512' } }) },
];

// --- Geometry -------------------------------------------------------------------------------------------------------

type World = { x: number; y: number; z: number };
const round = (value: number, digits = 3) => { const f = 10 ** digits; return Math.round(value * f) / f; };
const toWorld = ([sx, sy, sz]: Vec3, [cx, cy, cz]: Vec3): World => ({ x: round(sx + cx), y: round(-sz + cy), z: round(sy + cz) });
const toScene = ({ x, y, z }: World, [cx, cy, cz]: Vec3): Vec3 => [x - cx, z - cz, -(y - cy)];
/** Large enough for "the whole cloud" along an axis, small enough for float32 in the shader. */
const FAR = 1e7;
/** A data URL bigger than this costs the model too much; the capture gets smaller until it fits. */
const MAX_CAPTURE_CHARS = 1_000_000;

/** Bounds of the points of one cloud in world coordinates, cached per positions array. */
const boundsCache = new WeakMap<Float32Array, { version: number; min: World; max: World }>();
function cloudBounds(data: CloudData, version: number) {
  const cached = boundsCache.get(data.positions);
  if (cached && cached.version === version) return { min: cached.min, max: cached.max };
  const p = data.positions, lo: Vec3 = [Infinity, Infinity, Infinity], hi: Vec3 = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < p.length; i += 3) for (let k = 0; k < 3; k++) { const v = p[i + k]; if (v < lo[k]) lo[k] = v; if (v > hi[k]) hi[k] = v; }
  if (p.length === 0) return null;
  // Scene z is -Y: its minimum is the maximum Y.
  const a = toWorld(lo, data.center), b = toWorld(hi, data.center);
  const result = { min: { x: a.x, y: b.y, z: a.z }, max: { x: b.x, y: a.y, z: b.z } };
  boundsCache.set(data.positions, { version, ...result });
  return result;
}

export function createSiteAppTools(host: PointcloudHost): SiteAppTools {
  const ready = () => host.state().pointclouds.filter(pc => pc.indexingProgress >= 1 && host.data(pc.id));
  const find = (key: string | undefined) => {
    const state = host.state(), list = ready();
    if (key === undefined) {
      const found = list.find(pc => pc.id === state.activePointcloudId) ?? list[0];
      if (!found) throw new Error('geen_wolk: open eerst een puntenwolk');
      return found;
    }
    const found = list.find(pc => pc.id === key) ?? list.find(pc => pc.fileName === key);
    if (!found) throw new Error(`onbekende_wolk: ${key}`);
    return found;
  };
  /** The reference for coordinates: the centre of the active cloud. */
  const reference = () => host.data(find(undefined).id)!.center;
  const visible = () => ready().filter(pc => pc.visible).map(pc => ({ entry: pc, data: host.data(pc.id)! }));
  /** Scene-space box around everything visible. */
  const sceneBox = () => {
    const lo: Vec3 = [Infinity, Infinity, Infinity], hi: Vec3 = [-Infinity, -Infinity, -Infinity];
    for (const { entry, data } of visible()) {
      const b = cloudBounds(data, entry.transformVersion);
      if (!b) continue;
      const s1 = toScene(b.min, data.center), s2 = toScene(b.max, data.center);
      for (let k = 0; k < 3; k++) { lo[k] = Math.min(lo[k], s1[k], s2[k]); hi[k] = Math.max(hi[k], s1[k], s2[k]); }
    }
    if (!Number.isFinite(lo[0])) throw new Error('geen_wolk: er is geen zichtbare puntenwolk');
    return { lo, hi };
  };

  const stand = () => {
    const state = host.state(), list = ready(), camera = host.camera();
    const center = list.length ? reference() : null;
    return {
      wolken: state.pointclouds.map(pc => {
        const data = host.data(pc.id), bounds = data && pc.indexingProgress >= 1 ? cloudBounds(data, pc.transformVersion) : null;
        return {
          id: pc.id, bestand: pc.fileName, formaat: pc.format, punten: pc.totalPoints, zichtbaar: pc.visible, actief: pc.id === state.activePointcloudId,
          ...(pc.indexingProgress < 1 ? { laden: { voortgang: round(pc.indexingProgress, 2), fase: pc.indexingPhase } } : {}),
          ...(bounds ? { begrenzing: bounds } : {}),
          attributen: { rgb: pc.hasColor, intensiteit: pc.hasIntensity, classificatie: pc.hasClassification },
        };
      }),
      kleurmodus: modeNames[state.pointcloudColorMode], puntgrootte: state.pointcloudPointSize, edl: state.edlEnabled,
      camera: camera && center ? { positie: toWorld(camera.position, center), doel: toWorld(camera.target, center) } : null,
      doorsnede: state.clipBox && center ? clipInWorld(state.clipBox, center) : null,
    };
  };
  const clipInWorld = (box: PointcloudClipBox, center: Vec3) => {
    const a = toWorld(box.min, center), b = toWorld(box.max, center);
    const clamp = (v: number) => Math.abs(v) > FAR / 2 ? null : v;
    return { min: { x: clamp(a.x), y: clamp(b.y), z: clamp(a.z) }, max: { x: clamp(b.x), y: clamp(a.y), z: clamp(b.z) } };
  };

  const camera = (args: { aanzicht?: string; positie?: World; doel?: World }) => {
    if (!host.camera()) throw new Error('geen_beeld: de 3D-weergave is er niet');
    const center = reference();
    if (args.positie || args.doel) {
      if (!args.positie || !args.doel) throw new Error('ongeldige_argumenten: positie en doel horen samen');
      if (args.aanzicht) throw new Error('ongeldige_argumenten: kies aanzicht of positie en doel');
      host.setCamera(toScene(args.positie, center), toScene(args.doel, center));
    } else {
      const { lo, hi } = sceneBox(), target: Vec3 = [(lo[0] + hi[0]) / 2, (lo[1] + hi[1]) / 2, (lo[2] + hi[2]) / 2];
      const radius = Math.max(Math.hypot(hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]) / 2, 0.5), d = radius * 2.5, e = d * 1e-3;
      const offsets: Record<string, Vec3> = {
        passend: [d * 0.5, d * 0.7, d * 0.5], iso: [d * 0.6, d * 0.6, d * 0.6],
        boven: [0, d, e], onder: [0, -d, e], voor: [0, 0, d], achter: [0, 0, -d], links: [-d, 0, 0], rechts: [d, 0, 0],
      };
      const offset = offsets[args.aanzicht ?? 'passend'];
      host.setCamera([target[0] + offset[0], target[1] + offset[1], target[2] + offset[2]], target);
    }
    const now = host.camera()!;
    return { positie: toWorld(now.position, center), doel: toWorld(now.target, center) };
  };

  /** Nearest point of the visible clouds to a world point. */
  const nearest = (at: World) => {
    let best: { world: World; distance: number } | null = null;
    for (const { data } of visible()) {
      const [px, py, pz] = toScene(at, data.center), p = data.positions;
      let bestIndex = -1, bestSquare = Infinity;
      for (let i = 0; i < p.length; i += 3) {
        const dx = p[i] - px, dy = p[i + 1] - py, dz = p[i + 2] - pz, square = dx * dx + dy * dy + dz * dz;
        if (square < bestSquare) { bestSquare = square; bestIndex = i; }
      }
      if (bestIndex >= 0 && (!best || Math.sqrt(bestSquare) < best.distance)) {
        const scene: Vec3 = [p[bestIndex], p[bestIndex + 1], p[bestIndex + 2]];
        best = { world: toWorld(scene, data.center), distance: Math.sqrt(bestSquare) };
      }
    }
    if (!best) throw new Error('geen_wolk: er is geen zichtbare puntenwolk');
    return best;
  };

  const measure = (args: { van: World; naar: World; vastklikken?: boolean }) => {
    let from = args.van, to = args.naar;
    const snapped: Record<string, unknown> = {};
    if (args.vastklikken) {
      const a = nearest(from), b = nearest(to);
      from = a.world; to = b.world;
      snapped.vastgeklikt = { van: round(a.distance), naar: round(b.distance) };
    }
    const dx = to.x - from.x, dy = to.y - from.y, dz = to.z - from.z, horizontal = Math.hypot(dx, dy);
    return {
      van: from, naar: to, afstand: round(Math.hypot(dx, dy, dz)), horizontaal: round(horizontal), hoogteverschil: round(dz),
      hellingGraden: round(Math.atan2(Math.abs(dz), horizontal) * 180 / Math.PI, 2), ...snapped,
    };
  };

  const height = (args: { x: number; y: number; straal?: number; classificatie?: number }) => {
    const radius = args.straal ?? 0.5, square = radius * radius;
    let count = 0, sum = 0, low = Infinity, high = -Infinity;
    const list = visible();
    if (!list.length) throw new Error('geen_wolk: er is geen zichtbare puntenwolk');
    for (const { data } of list) {
      const [cx, cy, cz] = data.center, p = data.positions, cls = data.classifications;
      // Scene x = X - cx and scene z = -(Y - cy); scene y = Z - cz.
      const sx = args.x - cx, sz = -(args.y - cy);
      for (let i = 0, j = 0; i < p.length; i += 3, j++) {
        const dx = p[i] - sx, dz = p[i + 2] - sz;
        if (dx * dx + dz * dz > square) continue;
        if (args.classificatie !== undefined && cls[j] !== args.classificatie) continue;
        const z = p[i + 1] + cz;
        count++; sum += z; if (z < low) low = z; if (z > high) high = z;
      }
    }
    if (!count) throw new Error(`geen_punten: geen punten binnen ${radius} m van (${args.x}, ${args.y})`);
    return { x: args.x, y: args.y, straal: radius, aantal: count, laagste: round(low), hoogste: round(high), gemiddeld: round(sum / count) };
  };

  const colors = (args: { modus: string; puntgrootte?: number }) => {
    const mode = modes[args.modus];
    host.setColorMode(mode);
    if (args.puntgrootte !== undefined) host.setPointSize(args.puntgrootte);
    const list = ready(), missing = {
      rgb: list.length > 0 && !list.some(pc => pc.hasColor), intensity: list.length > 0 && !list.some(pc => pc.hasIntensity),
      classification: list.length > 0 && !list.some(pc => pc.hasClassification), elevation: false,
    }[mode];
    const state = host.state();
    return { kleurmodus: modeNames[state.pointcloudColorMode], puntgrootte: state.pointcloudPointSize,
      ...(missing ? { waarschuwing: `geen wolk heeft ${args.modus}; de kleuren zeggen dan weinig` } : {}) };
  };

  const section = (args: { soort: string; min?: World; max?: World; as?: string; waarde?: number; dikte?: number }) => {
    if (args.soort === 'uit') { host.setClipBox(null); return { doorsnede: null }; }
    const center = reference();
    let min: World, max: World;
    if (args.soort === 'box') {
      if (!args.min || !args.max) throw new Error('ongeldige_argumenten: box vraagt min en max');
      min = { x: Math.min(args.min.x, args.max.x), y: Math.min(args.min.y, args.max.y), z: Math.min(args.min.z, args.max.z) };
      max = { x: Math.max(args.min.x, args.max.x), y: Math.max(args.min.y, args.max.y), z: Math.max(args.min.z, args.max.z) };
    } else {
      if (!args.as || args.waarde === undefined) throw new Error('ongeldige_argumenten: snede vraagt as en waarde');
      const half = (args.dikte ?? 0.5) / 2, axis = args.as as keyof World;
      const far = (sign: number): World => ({ x: center[0] + sign * FAR, y: center[1] + sign * FAR, z: center[2] + sign * FAR });
      min = { ...far(-1), [axis]: args.waarde - half }; max = { ...far(1), [axis]: args.waarde + half };
    }
    const a = toScene(min, center), b = toScene(max, center);
    host.setClipBox({ min: [a[0], a[1], Math.min(a[2], b[2])], max: [b[0], b[1], Math.max(a[2], b[2])] });
    let inside = 0;
    for (const { data } of visible()) {
      const lo = toScene(min, data.center), hi = toScene(max, data.center), p = data.positions;
      const zLow = Math.min(lo[2], hi[2]), zHigh = Math.max(lo[2], hi[2]);
      for (let i = 0; i < p.length; i += 3) if (p[i] >= lo[0] && p[i] <= hi[0] && p[i + 1] >= lo[1] && p[i + 1] <= hi[1] && p[i + 2] >= zLow && p[i + 2] <= zHigh) inside++;
    }
    return { doorsnede: clipInWorld(host.state().clipBox!, center), puntenBinnen: inside };
  };

  const cloudTool = (args: { wolk: string; zichtbaar?: boolean; actief?: boolean }) => {
    const pc = find(args.wolk);
    if (args.zichtbaar !== undefined) host.setVisible(pc.id, args.zichtbaar);
    if (args.actief) host.setActive(pc.id);
    const now = host.state().pointclouds.find(item => item.id === pc.id)!;
    return { id: now.id, bestand: now.fileName, zichtbaar: now.visible, actief: host.state().activePointcloudId === now.id };
  };

  const capture = (args: { maximaal?: number }) => {
    let max = args.maximaal ?? 512;
    for (let attempt = 0; attempt < 4; attempt++, max = Math.round(max / 2)) {
      const image = host.capture(max, 'image/png') ?? null;
      if (image === null) throw new Error('geen_beeld: de 3D-weergave is er niet (geen WebGL)');
      if (image.length <= MAX_CAPTURE_CHARS) return { afbeelding: image };
      const jpeg = host.capture(max, 'image/jpeg');
      if (jpeg && jpeg.length <= MAX_CAPTURE_CHARS) return { afbeelding: jpeg };
    }
    throw new Error('te_groot: de schermafdruk past niet');
  };

  const handlers: Record<string, (args: never) => unknown> = {
    pointcloud_stand: stand, pointcloud_camera: camera, pointcloud_meten: measure, pointcloud_hoogte: height,
    pointcloud_kleuren: colors, pointcloud_doorsnede: section, pointcloud_wolk: cloudTool, pointcloud_vastleggen: capture,
  };
  return {
    tools: () => definitions.map(def => def.name),
    definitions: async () => definitions,
    async run(name, args = {}) {
      const def = definitions.find(item => item.name === name);
      if (!def) throw new Error('onbekende_tool');
      check(args ?? {}, def.inputSchema, '');
      return handlers[name]((args ?? {}) as never);
    },
  };
}

/** Put `window.__OPDS_APP__` in place; only in the browser build, not under Tauri. */
export function installSiteAppTools(host: PointcloudHost, target: { __OPDS_APP__?: SiteAppTools } = window as never): void {
  if ('__TAURI_INTERNALS__' in target) return;
  target.__OPDS_APP__ = createSiteAppTools(host);
}
