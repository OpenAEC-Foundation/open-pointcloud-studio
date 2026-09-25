# Design — Open-source ReCap-equivalent kernel + UI-rebrand

**Datum:** 2026-05-22
**Status:** Voorstel, ter goedkeuring
**Scope:** v0.4.0 — fundamentele herstructurering van Open Pointcloud Studio

---

## 1. Samenvatting

Open Pointcloud Studio wordt herpositioneerd als open-source ReCap-equivalent. Twee sporen worden in één release uitgevoerd:

- **Spoor B — Pointcloud-kernel**: vervang in-memory pipeline door streaming-architectuur op basis van het COPC-formaat, met CCCoreLib (LGPL) als algoritmen-kernel via Rust→C++ FFI. Doel: comfortabel openen van clouds tot ~500M punten, met selectie/decimation/scan-station ondersteuning.
- **Spoor A — UI-rebrand**: volledige adoptie van `@openaec/ui` (design tokens + componenten-CSS) als enige styling-bron, waarmee Open Pointcloud Studio aansluit op de OpenAEC-Foundation huisstijl.

Het Tauri + React + TypeScript fundament blijft. Alle eigengeschreven punctelogica die door bewezen open-source equivalenten vervangen kan worden, wordt verwijderd.

---

## 2. Doelen en niet-doelen

### Doelen
- Punctclouds tot **500M punten** vlot openen en navigeren (frame-rate ≥30 fps).
- Bestandsformaten gelijk aan CloudCompare's praktische set: **LAS, LAZ, E57, PLY, PCD, PTS, PTX, XYZ/ASC**.
- Subsampling-tools beschikbaar in de UI: **random %, voxel/spatial, minimum-distance, classification filter, statistical outlier removal**.
- **Selectie** (box + lasso) en **delete** met persistente edit-mask, zonder de COPC zelf te muteren.
- **Scan-station** herkenning uit E57/PTX: navigeer-naar-station camera, panorama-thumbnail indien aanwezig.
- **Project-bestand** dat import + edits + scan-stations vasthoudt zodat sessies herbruikbaar zijn.
- UI volledig op `@openaec/ui` tokens en componenten, dark-by-default met de OpenAEC oranje accent.

### Niet-doelen (YAGNI)
- Meshing, surface reconstruction, mesh-import/export. Bestaande `SurfaceReconstruction`, `MeshExporter`, `OBJParser`, `STLParser`, `OFFParser`, `DXFParser` worden verwijderd.
- ICP-registratie tussen meerdere clouds (CCCoreLib biedt het, maar interactieve registratie-UI is een aparte spec).
- 3D Tiles / EPT als alternatief streaming-formaat (COPC alleen).
- Volledige panoramische scan-station rendering (alleen thumbnail + camera-positie in v1).
- Hot-reload van COPC tijdens schrijven door externe tools.

---

## 3. Vastgelegde architecturale keuzes

| Beslissing | Gekozen | Reden |
|---|---|---|
| Doelschaal | ~500M punten | Realistisch ReCap-niveau, dekt 95% van TLS/MMS-projecten |
| Streaming-formaat | **COPC** (Cloud Optimized Point Cloud) | Open standaard, ingebakken octree, mmap-streamen mogelijk |
| Import-converter | **`untwine`** subprocess (BSD) | Battle-tested, multi-threaded, dezelfde herkomst als COPC zelf |
| Renderer | **three.js** + custom JS-streamer | Browser-snelle WebGL, behoud van huidige stack |
| COPC-streaming naar viewer | **`axum` serveert COPC-bestand met Range** + **`copc.js`** in JS doet parsing | Hergebruikt bewezen libraries, geen eigen streamer |
| COPC Rust-side reads (manifest, bounds) | **`copc-rs`** crate | Voor headers / kleine reads in Rust |
| Algoritmen-kernel | **CCCoreLib** via Rust→C++ FFI (LGPL) | Past licentie-compatibel, optimaal interactief, exact passende API |
| FFI-strategie | Vendored git-submodule + `cmake` crate + `cxx` | Eén `cargo build` doet alles, in-proces snelheid |
| UI-styling | **`@openaec/ui`** als enige bron | Foundation-standaard, volledige adoptie zonder hybride |
| Existing TS-parsers | Verwijderd (untwine neemt over) | Reduceert te onderhouden code drastisch |
| Existing Rust-octree | Verwijderd (CCCoreLib en COPC vervangen) | Niets nieuws bouwen waar bestaand werkt |

---

## 4. Architectuur — gelaagd overzicht

```
┌──────────────────────────────────────────────────────────────────────┐
│  TAURI APP  (Windows / macOS / Linux installer + WebView2 bootstrap) │
├──────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  React UI  (TypeScript, Vite)                                        │
│  ├─ @openaec/ui CSS tokens + components (enige styling)              │
│  ├─ Ribbon / MenuBar / Panels (her-skinned)                          │
│  ├─ PointcloudViewer (three.js canvas + WebGL)                       │
│  ├─ COPC streaming-client (~400 LOC, vraagt nodes via localhost)     │
│  └─ Selection tools (box/lasso, EDL, classification filter)          │
│                                                                      │
│  ───────── Tauri IPC + lokale HTTP-server (axum, op 127.0.0.1) ───── │
│                                                                      │
│  Rust backend (open-pointcloud-studio-core crate)                    │
│  ├─ HTTP-server  →  /node/{id} → COPC-node bytes (mmap)              │
│  ├─ Import-worker  →  spawns `untwine` subprocess                    │
│  ├─ Project-manifest service (.opcs JSON I/O)                        │
│  ├─ Edit-mask service (RoaringBitmap, sidecar persist)               │
│  ├─ Scan-station service (E57/PTX metadata extraction)               │
│  └─ ccclib-bridge crate (Rust→C++ FFI via cxx)                       │
│         │                                                            │
│         ▼                                                            │
│  CCCoreLib  (LGPL, C++, vendored submodule, statisch gelinkt)        │
│  ├─ Subsample (random, voxel, min-distance)                          │
│  ├─ Statistical Outlier Removal                                      │
│  ├─ Normals computation (later)                                      │
│  └─ Octree / KdTree                                                  │
│                                                                      │
│  ────────── On-disk artefacten ──────────────────────────────────────│
│                                                                      │
│  Per project (in een door gebruiker gekozen project-map):            │
│  ├─ project.opcs            (manifest, JSON)                         │
│  ├─ source.copc.laz         (geconverteerde cloud, untwine-output)   │
│  ├─ edits.mask              (RoaringBitmap, per-punt verwijderd-vlag)│
│  └─ stations.json           (scan-station poses + thumbnails)        │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### Bewuste keuze: lokale HTTP-server in plaats van Tauri-IPC voor tile-streaming

Bestaande JS-libraries voor COPC/Potree streaming (`copc.js`, `@pnext/three-loader`) verwachten `fetch()` via HTTP. We hosten een interne `axum`-server op `127.0.0.1` (random poort, alleen-localhost). Dit:
- Hergebruikt bewezen JS-streamingcode zonder eigen IPC-encoding.
- Profiteert van browser-native `fetch` met `Range` headers — exact waar COPC voor ontworpen is.
- Voorkomt dat we een eigen JS-streamer hoeven schrijven (~1500 LOC bespaard).

Performance: localhost HTTP via WebView2/WebKit is in praktijk binnen 10% van direct mmap. Voor 500M-doel ruim voldoende.

---

## 5. Componenten

### 5.1 UI-laag — adoptie van `@openaec/ui`

**Wat:**
- `npm install @openaec/ui` toevoegen.
- In `src/main.tsx` / hoofdentry: `import "@openaec/ui/css/tokens.css"; import "@openaec/ui/css/components.css";`
- `<html data-theme="light">` (of `"openaec"`) op de root.
- Bestaande `Ribbon.css`, `SettingsDialog.css`, themes in `src/styles/` worden vervangen door `@openaec/ui` componenten of door dunne wrappers die alleen `--oaec-*` tokens consumeren.
- Tailwind blijft beschikbaar voor layout-utilities, maar **kleuren / typografie / spacing / radii komen exclusief uit `--oaec-*` custom properties**. Geen hardcoded hex-codes, geen eigen kleurpalet.

**Componenten die geport worden:**
- `MenuBar/MenuBar.tsx`
- `Ribbon/*` (Ribbon, RibbonComponents, RibbonIcons)
- `StatusBar/*`
- `SettingsDialog/*`
- `panels/PointcloudPanel.tsx`
- `panels/BAG3DPanel.tsx` (blijft functioneel ongewijzigd, alleen visueel her-skinned)
- `App.tsx` layout-frame

**Wat blijft buiten scope van de rebrand:**
- `PointcloudViewer.tsx` canvas-interne overlays (HUD-tekst etc.) — gebruiken al `--oaec-*` kleuren via inline styles, geen aparte componenten.

**Theme-keuze:** `data-theme="light"` op `<html>`. In de OpenAEC-tokens is dat een donker-grijs basis (`#36363E`) met oranje accent (`#D97706`) — verwarrend benoemd maar dat is hun conventie. Andere themes (huidige Light/Blue/HighContrast/Dark in onze app) worden verwijderd; het OpenAEC-design is consistent één thema.

### 5.2 Tauri shell — lokale HTTP-server

**Nieuwe Rust-crate sectie**: `src-tauri/src/serve/`
- Start bij app-launch een `axum`-server op `127.0.0.1:<random>`. Poort wordt via Tauri-state aan de frontend gegeven.
- Routes:
  - `GET /project/manifest` → de actieve `.opcs` JSON
  - `GET /project/copc` → het hele COPC-bestand met `Range`-support (`axum` heeft `ServeFile`)
  - `GET /project/stations` → scan-station JSON
  - `GET /project/mask` → edit-mask als binary blob (gzip)
  - `POST /project/mask` → update edit-mask na selectie-delete
- CORS strict gelimiteerd tot de Tauri-webview origin.

### 5.3 Import-pipeline

**Nieuwe Rust-module**: `src-tauri/src/import/`
- Bundelt `untwine` als platform-binary in `src-tauri/binaries/` (`untwine-x86_64-pc-windows-msvc.exe`, `untwine-x86_64-apple-darwin`, `untwine-aarch64-apple-darwin`, `untwine-x86_64-unknown-linux-gnu`).
- Tauri `tauri.conf.json` → `bundle.externalBin` registratie.
- Workflow:
  1. Gebruiker kiest bestand via Tauri file-dialog.
  2. Rust spawnt `untwine -i <input> -o <project>/source.copc.laz --temp_dir <tmp>` met `tokio::process::Command`.
  3. stderr regel-voor-regel parsen voor voortgang (`Processed N/M points`), via Tauri-event naar UI.
  4. Bij voltooiing: scan-station extractie (zie 5.7), manifest schrijven, edit-mask initialiseren als lege bitmap.
- Voor E57/PTX is `untwine` configurable; scan-station metadata moeten we apart extraheren uit het bronbestand vóór conversie (untwine gooit de pose-info weg).

### 5.4 Storage — COPC + sidecars

**Project-map structuur** (door gebruiker gekozen locatie):
```
my-scan/
├─ project.opcs             # JSON manifest, klein
├─ source.copc.laz          # de cloud (groot)
├─ edits.mask               # RoaringBitmap, gegroupeerd op LAS point-index
└─ stations.json            # array van scan-stations
```

Zie §7 voor het manifest-formaat.

**Waarom externe sidecars in plaats van in COPC inbedden:** COPC is een gestandaardiseerd LAZ-formaat; muteren van COPC zelf zou downstream-tools breken. Sidecars zijn editor-eigen state.

### 5.5 Streaming renderer

**JS-side**:
- Library: **`copc.js`** (`@hobu/copc`) voor het lezen van COPC-headers en node-hierarchie via fetch.
- Library: **three.js** voor rendering. Bestaande `PointcloudViewer.tsx` blijft als rendering host, maar:
  - Verliest `LODController.ts` (vervangen door copc.js' eigen LOD).
  - Verliest de meeste manuele BufferGeometry-management code; copc.js geeft per node een `PointData`-object dat we direct in een three.js `BufferGeometry` schuiven.
- **Custom code (~400 LOC)**: een dunne `CopcSceneManager` die:
  - copc.js node-tree volgt
  - per camera-update bepaalt welke nodes te tonen (frustum + projected pixel-size threshold)
  - nodes async laadt, recycelt three.js `BufferGeometry` via een pool, voegt toe/verwijdert uit de three.js scene
  - kleurmodi (RGB / elevatie / intensiteit / classification) aanstuurt via shader-uniforms in een custom `ShaderMaterial`
  - edit-mask als 1-bit attribute meeschuift naar de shader, gemaskeerde punten `discard`'d in fragment shader

**Bewaar features uit huidige codebase:**
- `PointcloudMaterial.ts` shader (RGB/elev/class/intensity) — wordt aangepast om edit-mask attribute te lezen
- EDL (Eye-Dome Lighting) post-processing pass — blijft
- Classification filter UI — gaat via een uniform-array naar de shader

### 5.6 Algoritmen-kernel — CCCoreLib FFI

**Nieuwe Rust-crate**: `src-tauri/crates/cccorelib-bridge/`
- Git-submodule onder `src-tauri/crates/cccorelib-bridge/vendor/CCCoreLib/`.
- `build.rs` gebruikt `cmake` crate om CCCoreLib te bouwen als static library.
- `cxx` crate definieert de FFI-bridge in `bridge.rs`. Geëxposeerde functies (v1):
  ```rust
  fn subsample_random(input: &PointCloudView, ratio: f64) -> Result<PointIndices>;
  fn subsample_voxel(input: &PointCloudView, voxel_size: f64) -> Result<PointIndices>;
  fn subsample_min_distance(input: &PointCloudView, min_dist: f64) -> Result<PointIndices>;
  fn statistical_outlier_removal(input: &PointCloudView, k: usize, std_ratio: f64) -> Result<PointIndices>;
  ```
- Resultaten zijn **point-index lijsten** (geen kopieën van punten zelf). Indices worden toegevoegd aan de edit-mask in plaats van direct toegepast.
- `PointCloudView` is een Rust-struct die een view geeft op een set COPC-nodes (volledige cloud, of een ge-selecteerde subset).

**Build-implicaties** (zie §9):
- Devs en CI hebben C++ compiler nodig: MSVC op Windows, clang op macOS, g++/clang op Linux.
- Eerste build van CCCoreLib duurt 5–10 minuten, cached daarna.
- Build-script vendored in submodule om versie-pinning te garanderen.

### 5.7 Selectie & edit-mask

**Concept**: punten worden nooit fysiek verwijderd uit de COPC. In plaats daarvan houdt `edits.mask` een `RoaringBitmap` bij met de point-indices die "verborgen / verwijderd" zijn.

**Selectie-tools** (JS-side, in `PointcloudViewer.tsx` als overlay-laag):
- **Box-select**: shift-drag rechthoek in screen-space; alle punten waarvan projected NDC binnen valt worden toegevoegd aan selectie.
- **Lasso-select**: free-form polyline; ray-casten naar bounding-box-test per node, vervolgens point-in-polygon test in screen-space.
- **Invert / clear** selectie.
- **Delete**: huidige selectie → `POST /project/mask` → backend mergt indices in de RoaringBitmap, schrijft naar disk, broadcast event terug → UI invalidateert de gemaskerde nodes.
- **Undo/redo**: stack van bitmap-deltas, max 50 stappen, in-memory (geen disk).

**Shader-integratie**: per node, bij het uploaden van de BufferGeometry, voegen we een `uint8` attribute toe (`a_masked`, 0/1). Fragment-shader doet `if (v_masked > 0.5) discard;`. Bij mask-mutatie wordt alleen de `a_masked` buffer opnieuw geüpload, niet de positions/colors.

**Persistente sleutel**: de RoaringBitmap indexeert op de **oorspronkelijke COPC point-index** (volgorde zoals untwine geschreven heeft). copc.js levert deze indices per node mee.

### 5.8 Scan-station support

**Detectie**:
- **E57**: bevat `Image2D` en `Data3D` records met `pose` (4x4 transform). Tijdens import lezen we deze met de `e57` Rust-crate (al aanwezig in onze deps).
- **PTX**: tekst-header bevat een translatie + rotatie per scan. Onze huidige `PTXParser` extraheert dit al; we verschuiven die logica naar Rust-side bij import.
- **LAS/LAZ**: heeft geen native station-concept. Skip — geen stations beschikbaar.

**Opslag** (`stations.json`):
```json
[
  {
    "id": "station_001",
    "name": "Scan_01",
    "pose": { "translation": [x, y, z], "rotation_quat": [x, y, z, w] },
    "thumbnail_jpg_base64": "...",  // optioneel, alleen als E57 Image2D aanwezig
    "spherical_bounds": { "phi_min": ..., "phi_max": ..., "theta_min": ..., "theta_max": ... }
  }
]
```

**UI-integratie** (in `PointcloudPanel`):
- Lijst van stations met thumbnail + naam.
- Click op station → camera teleport naar `pose.translation`, kijkrichting volgens `pose.rotation_quat`, FOV ~90°.
- Geen volledige panoramische reprojectie in v1 — alleen "ga naar de positie waar de scanner stond".

**Toekomst** (niet in v1): bolprojectie van de cloud rondom een station als "binnen-in-de-scan" view, à la ReCap's bubble-view.

---

## 6. Data-flow voorbeelden

### Import (LAS → COPC + project)

```
User: File → Open... → kiest scan.las
  ↓
React: dispatch openProject(path)
  ↓
Tauri IPC: invoke("import_pointcloud", { path })
  ↓
Rust: import-worker
  ├─ extract scan-stations (E57/PTX only) → stations.json
  ├─ spawn untwine -i scan.las -o project/source.copc.laz
  │    └─ stderr → emit("import_progress", { pct })
  ├─ initialize empty edits.mask
  └─ write project.opcs manifest
  ↓
Rust: emit("import_done", { project_path })
  ↓
React: load manifest via HTTP /project/manifest, init CopcSceneManager
  ↓
copc.js: reads /project/copc, builds node tree
  ↓
CopcSceneManager: streams nodes based on camera → three.js scene
```

### Decimation (random 10%)

```
User: Tools → Subsample → Random, 10% → Apply
  ↓
React: invoke("subsample", { method: "random", ratio: 0.1, target: "all" })
  ↓
Rust: ccclib-bridge → CCCoreLib::CloudSamplingTools::subsampleCloudRandomly
  ↓
Rust: returns point-indices to remove (90% of points)
  ↓
Rust: merge into edits.mask, persist to disk
  ↓
Tauri event: emit("mask_updated")
  ↓
React: CopcSceneManager invalidates affected nodes, re-uploads a_masked buffers
  ↓
Viewer: shows 10% of points
```

### Selectie + delete

```
User: drag lasso around tree → Delete key
  ↓
React: compute point-indices in selection (CPU-side, per visible node)
  ↓
React: POST /project/mask { add_indices: [...] }
  ↓
Rust: merge bitmap, persist
  ↓
React: refresh affected node BufferGeometries
```

---

## 7. Project-bestand: `.opcs`

JSON, leesbaar, gecommit als blueprint van de sessie:

```json
{
  "version": "1.0",
  "name": "Bouwplaats Rotterdam Oost",
  "created": "2026-05-22T14:30:00Z",
  "modified": "2026-05-22T16:12:00Z",
  "source": {
    "original_file": "scan-2026-05.las",
    "imported_at": "2026-05-22T14:30:00Z",
    "untwine_version": "1.4.0"
  },
  "copc_file": "source.copc.laz",
  "mask_file": "edits.mask",
  "stations_file": "stations.json",
  "view": {
    "camera_position": [x, y, z],
    "camera_target": [x, y, z],
    "color_mode": "rgb",
    "point_size": 1.5,
    "edl_strength": 0.4,
    "classification_filter": []
  },
  "applied_operations": [
    { "type": "subsample_random", "ratio": 0.1, "applied_at": "..." },
    { "type": "lasso_delete", "n_points_removed": 12483, "applied_at": "..." }
  ]
}
```

`applied_operations` is een audit-trail, niet uitvoerbaar replay. De feitelijke state zit in de mask.

---

## 8. Migratie — wat er verandert in de codebase

### Te verwijderen

**Rust:**
- `src-tauri/src/pointcloud/octree.rs` (320 LOC) — vervangen door CCCoreLib + COPC's eigen octree.
- `src-tauri/src/pointcloud/parser.rs` (397 LOC) — vervangen door untwine subprocess.
- `src-tauri/src/pointcloud/manager.rs` (188 LOC) — herschreven richting COPC streaming.

**TypeScript:**
- `src/engine/pointcloud/LASParser.ts`
- `src/engine/pointcloud/LAZParser.ts`
- `src/engine/pointcloud/E57Parser.ts`
- `src/engine/pointcloud/PCDParser.ts`
- `src/engine/pointcloud/PTXParser.ts` (logica verhuist naar Rust import-stap)
- `src/engine/pointcloud/PTSParser.ts`
- `src/engine/pointcloud/XYZParser.ts`
- `src/engine/pointcloud/PLYParser.ts`
- `src/engine/pointcloud/parsePointcloudWorker.ts`
- `src/engine/pointcloud/pointcloud.worker.ts`
- `src/engine/pointcloud/workerProtocol.ts`
- `src/engine/pointcloud/LODController.ts`
- `src/engine/pointcloud/PointcloudParser.ts` (de generieke dispatcher)
- `src/engine/pointcloud/MeshExporter.ts`
- `src/engine/pointcloud/SurfaceReconstruction.ts`
- `src/engine/pointcloud/OBJParser.ts`
- `src/engine/pointcloud/STLParser.ts`
- `src/engine/pointcloud/OFFParser.ts`
- `src/engine/pointcloud/DXFParser.ts`
- `src/engine/pointcloud/BrowserPointcloudStore.ts`
- `src/components/panels/ReconstructionProgressDialog.tsx`

**Themes / styling:**
- `src/styles/themes/light.css`, `dark.css`, `blue.css`, `highcontrast.css` (alles vervangen door `@openaec/ui`).
- `src/components/SettingsDialog/SettingsDialog.css`, `Ribbon.css` (vervangen door tokens).

### Nieuw

**Rust crates:**
- `src-tauri/crates/cccorelib-bridge/` (FFI naar CCCoreLib)
- `src-tauri/crates/copc-streaming/` (wrapper rond `copc-rs` + axum routes)

**Rust modules:**
- `src-tauri/src/serve/` (axum HTTP server, routes)
- `src-tauri/src/import/` (untwine subprocess, stations-extractie)
- `src-tauri/src/project/` (.opcs manifest I/O)
- `src-tauri/src/mask/` (RoaringBitmap mask service)
- `src-tauri/src/stations/` (scan-station model)

**Binaries:**
- `src-tauri/binaries/untwine-*` (één per platform)

**TS:**
- `src/engine/copc/CopcSceneManager.ts` (streaming-orchestrator)
- `src/engine/copc/CopcClient.ts` (dunne wrapper rond `@hobu/copc` met onze HTTP-base-URL)
- `src/engine/edit/EditMaskClient.ts` (haalt/zet mask via HTTP)
- `src/engine/edit/Selection.ts` (box + lasso geometrie)
- `src/engine/stations/StationsClient.ts`

### Aangepast

- `src/components/canvas/PointcloudViewer.tsx`: ~50% kleiner, alleen rendering host + selectie-overlay; alle parsing-logica eruit.
- `src/state/slices/pointcloudSlice.ts`: state-shape herschreven rond `Project` in plaats van `loadedPoints`.
- `src/components/layout/Ribbon/useRibbonActions.ts`: ribbon-acties wijzen naar nieuwe IPC-commands.
- `src/components/panels/PointcloudPanel.tsx`: krijgt scan-stations sectie en sub-sample tools.
- `src-tauri/tauri.conf.json`: `externalBin` voor untwine, `bundle.resources` voor WebView2Loader.dll (zie globale CLAUDE.md richtlijn).

### Behouden zonder wijziging

- BAG3D-flow (`src/engine/bag3d/`, `src/components/panels/BAG3DPanel.tsx`) — onafhankelijk feature.
- `PointcloudExporter.ts` — voor het exporteren van de huidige (gemaskeerde) cloud naar LAS/LAZ.
- `PointcloudTransforms.ts` — coördinaattransformaties.
- `PointcloudMaterial.ts` — shader, krijgt edit-mask uniform/attribute toevoeging.
- Tauri-plugins (dialog, fs, store, updater, etc.).

---

## 9. Build- en distributie-implicaties

### Voor developers
- **Nieuwe vereiste**: C++ compiler en CMake op alle dev-machines.
  - Windows: MSVC Build Tools (komt al mee met Tauri Rust setup); CMake via `winget install Kitware.CMake`.
  - macOS: Xcode Command Line Tools (al vereist voor Tauri); CMake via Homebrew.
  - Linux: `build-essential` + `cmake` via apt/dnf.
- Eerste `cargo build` duurt 5–10 minuten extra voor CCCoreLib. Cached daarna.
- Git submodule sync nodig na clone (`git submodule update --init --recursive`).

### Voor installer-grootte
- CCCoreLib statisch gelinkt: +3–5 MB.
- `untwine` binary mee-gebundeld: +8 MB per platform.
- Totale installer-toename: **~12 MB**. Acceptabel (huidige Tauri-installer is ~20 MB).

### CI
- GitHub Actions workflow `tauri-build.yml` moet:
  - C++ toolchain installeren (MSVC/Xcode/g++ al beschikbaar op runners).
  - CMake installeren (al beschikbaar op standaard runners).
  - Caching toevoegen voor CCCoreLib build-artefacten (key op submodule-commit).

### Licenties
- App blijft **LGPL-3.0-or-later**.
- CCCoreLib (LGPL-2.0): statisch linken is toegestaan mits we onze object-files distribueerbaar maken zodat een gebruiker kan re-linken met een eigen CCCoreLib-versie. Werkwijze: publiceer een `relinkable-objects.tar.gz` als release-asset bij elke release.
- untwine (BSD-3): permissive, geen verplichtingen.
- copc.js / @pnext/three-loader (MIT/BSD): geen verplichtingen.
- @openaec/ui (MIT): geen verplichtingen.

---

## 10. v1-scope grenzen

### In v1 (deze release)
- Alles in §2 "Doelen".

### Naar latere release
- ICP multi-cloud registratie.
- Volledige panoramische scan-station bubble-view.
- Annotaties / metingen / coördinaten-uitlezen op klik.
- Normalen-berekening + on-the-fly shading.
- Cross-section / clipping planes.
- Export selectie als losse LAS/COPC.
- Multi-project / merge meerdere COPC's in één view.

---

## 11. Risico's en mitigaties

| Risico | Kans | Impact | Mitigatie |
|---|---|---|---|
| CCCoreLib build faalt op een platform (vooral Windows MSVC) | Middel | Hoog | Vroeg een CI-matrix build opzetten; CCCoreLib heeft GitHub Actions zelf, dus we weten dat het bouwbaar is |
| untwine crasht op edge-case bestand | Laag | Middel | Error capture op stderr; tonen aan gebruiker met de stderr-output |
| copc.js mature genoeg? | Laag | Hoog | Library wordt actief onderhouden door Hobu (de mensen achter COPC zelf); fallback is `copc-rs` plus eigen JS-streamer |
| Edit-mask wordt erg groot bij >100M-punt clouds | Laag | Laag | RoaringBitmap comprimeert ranges; voor 500M punten is een volledig-gemaskeerde mask ~62 MB raw, ~MB-range gecomprimeerd |
| Localhost HTTP-server wordt geblokkeerd door antivirus / firewall | Laag | Hoog | Tauri-installer beschrijft `127.0.0.1` binding; Windows firewall blokkeert dit normaliter niet voor processen die zelf de socket openen |
| LGPL re-linkbaarheid bij statische link onduidelijk | Middel | Middel | Bij twijfel: shared library bouwen i.p.v. statische (kleine performance-cost) |

---

## 12. Acceptatiecriteria

De release is "klaar" wanneer:

1. Een 200M-punten LAZ-bestand wordt geopend (incl. import-conversie) in <5 minuten op een mid-range laptop.
2. Navigeren door die cloud bij ≥30 fps op een GTX 1660 / M1 / vergelijkbaar.
3. Random subsample tot 1% op die cloud is in <2 seconden klaar.
4. Box-select + delete op zichtbare subset reageert in <100 ms.
5. E57-bestand met scan-stations wordt geïmporteerd; stations verschijnen in de panel; klik teleporteert camera.
6. Project openen + sluiten + heropenen herstelt edits exact.
7. UI is volledig gestyled door `@openaec/ui`; geen hardcoded kleuren of fonts.
8. Installer ≤35 MB op Windows, build cleanroom uitvoerbaar.

---

## 13. Vervolgstappen (na goedkeuring spec)

1. Plan-document maken via writing-plans skill — opsplitsen in onafhankelijke taken die idealiter parallel uitvoerbaar zijn.
2. Implementatie in fases:
   - Fase 1: UI-rebrand (kleinste risico, snelle zichtbaarheid).
   - Fase 2: COPC-streaming end-to-end (untwine + axum + copc.js).
   - Fase 3: CCCoreLib FFI + algoritmen.
   - Fase 4: Selectie + edit-mask.
   - Fase 5: Scan-stations.
3. Verwijderingsfase: pas oude code weggooien zodra nieuwe equivalenten draaien en getest zijn.

---

## Bijlage A — Bibliotheek-versies (target)

- `@openaec/ui`: 0.1.0+ (huidige)
- `copc-rs`: 0.5+
- `@hobu/copc` (copc.js): 0.4+
- `three`: 0.183+ (huidige)
- `cxx`: 1.0+
- `cmake` crate: 0.1+
- CCCoreLib: vendored op een vaste tag (te kiezen bij plan-fase; voorkeur `2.12.4` of nieuwer)
- `untwine`: 1.4.0+
- `roaring` (Rust crate): 0.10+
- `axum`: 0.7+
- `tokio`: 1.40+
