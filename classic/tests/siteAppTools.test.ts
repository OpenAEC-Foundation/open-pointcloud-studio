// node --test tests/*.test.ts (Node 22.18+ strips the types itself)
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createSiteAppTools, installSiteAppTools, type CloudData, type PointcloudHost, type Vec3 } from '../src/assistant/siteAppTools.ts';
import type { PointcloudClipBox, PointcloudColorMode, PointcloudEntry } from '../src/state/slices/pointcloudSlice.ts';

/** World points (X east, Y north, Z up) as the parsers store them: relative to the centre, Y up. */
function cloud(points: [number, number, number, number?][], center: Vec3): CloudData {
  const positions = new Float32Array(points.length * 3), classifications = new Float32Array(points.length);
  points.forEach(([x, y, z, c = 1], i) => { positions.set([x - center[0], z - center[2], -(y - center[1])], i * 3); classifications[i] = c; });
  return { positions, intensities: new Float32Array(points.length), classifications, center };
}
function entry(id: string, fileName: string, extra: Partial<PointcloudEntry> = {}): PointcloudEntry {
  return { id, fileName, filePath: fileName, format: 'XYZ', totalPoints: 0, bounds: { minX: 0, minY: 0, minZ: 0, maxX: 0, maxY: 0, maxZ: 0 },
    hasColor: false, hasIntensity: true, hasClassification: true, visible: true, indexingProgress: 1, indexingPhase: 'Ready', transformVersion: 0, ...extra };
}
function setup(withViewer = true) {
  const center: Vec3 = [155000, 463000, 5];
  // A ground plane at Z 0 and a small "building" (class 6) up to Z 9.
  const points: [number, number, number, number?][] = [];
  for (let x = 0; x <= 10; x++) for (let y = 0; y <= 10; y++) points.push([155000 + x, 463000 + y, 0, 2]);
  points.push([155005, 463005, 9, 6], [155005, 463005.2, 6, 6]);
  const data = new Map<string, CloudData>([['a', cloud(points, center)]]);
  const state = {
    pointclouds: [entry('a', 'terrein.xyz', { totalPoints: points.length }), entry('b', 'laden.las', { indexingProgress: 0.4, indexingPhase: 'Parsing' })],
    activePointcloudId: 'a' as string | null, pointcloudColorMode: 'rgb' as PointcloudColorMode, pointcloudPointSize: 2, edlEnabled: true,
    clipBox: null as PointcloudClipBox | null,
  };
  let cam = withViewer ? { position: [0, 50, 100] as Vec3, target: [0, 0, 0] as Vec3, fov: 60 } : null;
  const captures: number[] = [];
  const host: PointcloudHost = {
    state: () => state, data: id => data.get(id),
    setColorMode: mode => { state.pointcloudColorMode = mode; },
    setPointSize: size => { state.pointcloudPointSize = size; },
    setVisible: (id, visible) => { state.pointclouds.find(pc => pc.id === id)!.visible = visible; },
    setActive: id => { state.activePointcloudId = id; },
    setClipBox: box => { state.clipBox = box; },
    camera: () => cam,
    setCamera: (position, target) => { if (!cam) return false; cam = { ...cam, position, target }; return true; },
    capture: (max, type) => { if (!cam) return null; captures.push(max); return `data:${type};base64,${'A'.repeat(max > 300 ? 2_000_000 : 1000)}`; },
  };
  return { app: createSiteAppTools(host), state, captures };
}

test('definitions: 6–12 pointcloud_ tools with a Dutch description and an object schema', async () => {
  const { app } = setup();
  const definitions = await app.definitions();
  assert.ok(definitions.length >= 6 && definitions.length <= 12);
  assert.deepEqual(app.tools(), definitions.map(def => def.name));
  for (const def of definitions) {
    assert.match(def.name, /^pointcloud_[a-z_]+$/);
    assert.ok(def.description.length > 20);
    assert.equal(def.inputSchema.type, 'object');
  }
  assert.ok(app.tools().includes('pointcloud_stand') && app.tools().includes('pointcloud_vastleggen'));
});

test('arguments are checked and errors are short codes', async () => {
  const { app } = setup();
  await assert.rejects(app.run('pointcloud_nope'), /^Error: onbekende_tool$/);
  await assert.rejects(app.run('pointcloud_stand', { x: 1 }), /ongeldige_argumenten: argumenten kent x niet/);
  await assert.rejects(app.run('pointcloud_kleuren', { modus: 'regenboog' }), /ongeldige_argumenten: modus moet een van/);
  await assert.rejects(app.run('pointcloud_meten', { van: { x: 0, y: 0, z: 0 } }), /mist naar/);
  await assert.rejects(app.run('pointcloud_meten', { van: { x: 0, y: 0 }, naar: { x: 1, y: 1, z: 1 } }), /van mist z/);
  await assert.rejects(app.run('pointcloud_hoogte', { x: 1, y: 1, straal: 0 }), /straal moet minstens/);
  await assert.rejects(app.run('pointcloud_hoogte', { x: 0, y: 0 }), /^Error: geen_punten/);
  await assert.rejects(app.run('pointcloud_wolk', { wolk: 'laden.las', actief: true }), /onbekende_wolk/);
  await assert.rejects(app.run('pointcloud_doorsnede', { soort: 'snede', as: 'z' }), /snede vraagt as en waarde/);
  await assert.rejects(app.run('pointcloud_camera', { aanzicht: 'boven', positie: { x: 0, y: 0, z: 0 } }), /positie en doel horen samen/);
});

test('stand reports clouds in file coordinates, loading clouds and attributes', async () => {
  const { app } = setup();
  const out = await app.run('pointcloud_stand') as Record<string, any>;
  assert.equal(out.wolken.length, 2);
  assert.deepEqual(out.wolken[0].begrenzing, { min: { x: 155000, y: 463000, z: 0 }, max: { x: 155010, y: 463010, z: 9 } });
  assert.deepEqual(out.wolken[0].attributen, { rgb: false, intensiteit: true, classificatie: true });
  assert.equal(out.wolken[0].actief, true);
  assert.deepEqual(out.wolken[1].laden, { voortgang: 0.4, fase: 'Parsing' });
  assert.equal(out.wolken[1].begrenzing, undefined);
  assert.equal(out.kleurmodus, 'rgb');
  assert.deepEqual(out.camera.positie, { x: 155000, y: 462900, z: 55 });
  assert.equal(out.doorsnede, null);
  assert.doesNotThrow(() => JSON.stringify(out));
});

test('no cloud and no viewer give clear refusals', async () => {
  const { app, state } = setup(false);
  await assert.rejects(app.run('pointcloud_camera', { aanzicht: 'boven' }), /^Error: geen_beeld/);
  await assert.rejects(app.run('pointcloud_vastleggen'), /^Error: geen_beeld/);
  state.pointclouds = [];
  await assert.rejects(app.run('pointcloud_doorsnede', { soort: 'box', min: { x: 0, y: 0, z: 0 }, max: { x: 1, y: 1, z: 1 } }), /^Error: geen_wolk/);
  assert.deepEqual((await app.run('pointcloud_stand') as { wolken: unknown[] }).wolken, []);
});

test('camera views look at the centre from the right side', async () => {
  const { app } = setup();
  const top = await app.run('pointcloud_camera', { aanzicht: 'boven' }) as { positie: any; doel: any };
  assert.deepEqual(top.doel, { x: 155005, y: 463005, z: 4.5 });
  assert.equal(top.positie.x, 155005); assert.ok(top.positie.z > 20); assert.ok(Math.abs(top.positie.y - 463005) < 0.1);
  const front = await app.run('pointcloud_camera', { aanzicht: 'voor' }) as { positie: any };
  assert.ok(front.positie.y < 463005 - 10); assert.equal(front.positie.z, 4.5);
  const left = await app.run('pointcloud_camera', { aanzicht: 'links' }) as { positie: any };
  assert.ok(left.positie.x < 155005 - 10);
  const free = await app.run('pointcloud_camera', { positie: { x: 155020, y: 463020, z: 30 }, doel: { x: 155005, y: 463005, z: 0 } });
  assert.deepEqual(free, { positie: { x: 155020, y: 463020, z: 30 }, doel: { x: 155005, y: 463005, z: 0 } });
});

test('measuring between points, with snapping to the cloud', async () => {
  const { app } = setup();
  const plain = await app.run('pointcloud_meten', { van: { x: 155000, y: 463000, z: 0 }, naar: { x: 155003, y: 463004, z: 12 } }) as Record<string, number>;
  assert.equal(plain.afstand, 13); assert.equal(plain.horizontaal, 5); assert.equal(plain.hoogteverschil, 12);
  assert.equal(plain.hellingGraden, 67.38);
  const snapped = await app.run('pointcloud_meten', { van: { x: 155000.2, y: 463000.1, z: 0.3 }, naar: { x: 155005, y: 463005, z: 8.5 }, vastklikken: true }) as Record<string, any>;
  assert.deepEqual(snapped.van, { x: 155000, y: 463000, z: 0 });
  assert.deepEqual(snapped.naar, { x: 155005, y: 463005, z: 9 });
  assert.equal(snapped.hoogteverschil, 9);
  assert.ok(snapped.vastgeklikt.van > 0 && snapped.vastgeklikt.naar === 0.5);
});

test('height around a point, all classes or one', async () => {
  const { app } = setup();
  const all = await app.run('pointcloud_hoogte', { x: 155005, y: 463005, straal: 0.5 }) as Record<string, number>;
  assert.deepEqual(all, { x: 155005, y: 463005, straal: 0.5, aantal: 3, laagste: 0, hoogste: 9, gemiddeld: 5 });
  const ground = await app.run('pointcloud_hoogte', { x: 155005, y: 463005, straal: 1.5, classificatie: 2 }) as Record<string, number>;
  assert.equal(ground.hoogste, 0); assert.equal(ground.aantal, 9);
});

test('colouring by attribute, with a warning for a missing attribute', async () => {
  const { app, state } = setup();
  assert.deepEqual(await app.run('pointcloud_kleuren', { modus: 'hoogte', puntgrootte: 3 }), { kleurmodus: 'hoogte', puntgrootte: 3 });
  assert.equal(state.pointcloudColorMode, 'elevation');
  const rgb = await app.run('pointcloud_kleuren', { modus: 'rgb' }) as { waarschuwing?: string };
  assert.match(rgb.waarschuwing!, /rgb/);
  await app.run('pointcloud_kleuren', { modus: 'classificatie' });
  assert.equal(state.pointcloudColorMode, 'classification');
});

test('a clip box and a section in file coordinates, and off again', async () => {
  const { app, state } = setup();
  const box = await app.run('pointcloud_doorsnede', { soort: 'box', min: { x: 155004, y: 463004, z: -1 }, max: { x: 155006, y: 463006, z: 10 } }) as Record<string, any>;
  assert.equal(box.puntenBinnen, 11);
  assert.deepEqual(box.doorsnede, { min: { x: 155004, y: 463004, z: -1 }, max: { x: 155006, y: 463006, z: 10 } });
  // Scene: x = X - cx, y = Z - cz, z = -(Y - cy).
  assert.deepEqual(state.clipBox, { min: [4, -6, -6], max: [6, 5, -4] });
  const slice = await app.run('pointcloud_doorsnede', { soort: 'snede', as: 'z', waarde: 7.5, dikte: 3 }) as Record<string, any>;
  assert.equal(slice.puntenBinnen, 2);
  assert.deepEqual(slice.doorsnede, { min: { x: null, y: null, z: 6 }, max: { x: null, y: null, z: 9 } });
  const stand = await app.run('pointcloud_stand') as { doorsnede: unknown };
  assert.deepEqual(stand.doorsnede, slice.doorsnede);
  assert.deepEqual(await app.run('pointcloud_doorsnede', { soort: 'uit' }), { doorsnede: null });
  assert.equal(state.clipBox, null);
});

test('visibility and the active cloud', async () => {
  const { app, state } = setup();
  assert.deepEqual(await app.run('pointcloud_wolk', { wolk: 'terrein.xyz', zichtbaar: false }), { id: 'a', bestand: 'terrein.xyz', zichtbaar: false, actief: true });
  await assert.rejects(app.run('pointcloud_hoogte', { x: 155005, y: 463005 }), /geen_wolk/);
  await app.run('pointcloud_wolk', { wolk: 'a', zichtbaar: true });
  assert.equal(state.pointclouds[0].visible, true);
});

test('capture shrinks until it fits and falls back from PNG', async () => {
  const { app, captures } = setup();
  const shot = await app.run('pointcloud_vastleggen') as { afbeelding: string };
  assert.match(shot.afbeelding, /^data:image\/png;base64,/);
  assert.deepEqual(captures, [512, 512, 256]);
});

test('installs on a browser window, not under Tauri', () => {
  const { app } = setup(); void app;
  const browser: { __OPDS_APP__?: unknown } = {};
  installSiteAppTools({} as PointcloudHost, browser as never);
  assert.equal(typeof (browser.__OPDS_APP__ as { run: unknown }).run, 'function');
  const tauri: { __OPDS_APP__?: unknown; __TAURI_INTERNALS__: object } = { __TAURI_INTERNALS__: {} };
  installSiteAppTools({} as PointcloudHost, tauri as never);
  assert.equal(tauri.__OPDS_APP__, undefined);
});
