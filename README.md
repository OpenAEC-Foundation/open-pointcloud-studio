# Open Pointcloud Studio

A cross-platform pointcloud viewer built with Tauri, React, and Three.js, styled with the [OpenAEC design tokens](https://github.com/OpenAEC-Foundation/openaec-ui).

## Features

- **Formats**: LAS 1.2–1.4 (point formats 0–10), LAZ (via laz-perf), E57, PLY (ASCII, binary little- and big-endian), PCD (ASCII, binary), PTS, PTX, XYZ/ASC/TXT/CSV, OBJ, OFF, STL, DXF
- **E57**: multi-scan files with pose registration, colour and intensity, invalid-point filtering, **scanner stations** (fly to any station for the surveyor's view) and **embedded photos** (thumbnails and full-size viewer)
- Color modes: RGB, Elevation, Classification, Intensity
- **Eye-Dome Lighting** — a real screen-space pass (depth-based, Potree-style response), not a toggle that does nothing
- Adjustable point size and point budget
- Classification filtering (ASPRS)
- **Octree LOD** in the desktop app, backed by the shared `pointcloud-core` Rust crate: Morton-ordered build, rayon-parallel, grid-based LOD sampling, real view-frustum culling
- On-demand rendering: zero GPU work while the view is static
- OpenAEC Dark and OpenAEC Slate themes

### Not supported

- Autodesk ReCap `.rcp` / `.rcs` and FARO `.fls` — proprietary. `.rcp` is a ZIP containing project XML (scan list, registration matrices); `.rcs` is an `ADOCT` octree container whose point encoding is not yet decoded. Convert to E57 or LAS first.
- Files over 2 GB in the browser build (ArrayBuffer limit). The desktop app streams LAS/LAZ from disk.

## Architecture

```
src/                     React + Three.js frontend (Vite)
  engine/pointcloud/     Parsers (Web Worker where possible), EDL pass, LOD client
  engine/render/         Render scheduling
crates/pointcloud-core/  Octree, LOD selection, frustum culling — native for Tauri, compiles to wasm32
src-tauri/               Tauri desktop shell; uses pointcloud-core through a thin adapter
```

The browser build parses everything client-side and renders up to 1M points directly. The desktop build additionally indexes LAS/LAZ into an octree in Rust and streams visible nodes to the viewer.

## Getting Started

### Prerequisites

- Node.js 18+ (22 recommended)
- Rust 1.70+ (desktop build only)
- Windows: Visual Studio Build Tools with the C++ workload; WebView2 runtime

### Development

```bash
npm install
npm run dev          # Frontend only, http://localhost:3013
npm run tauri dev    # Full desktop app with hot reload
```

### Build

```bash
npm run build                       # Web bundle → dist/
npm run tauri build                 # Desktop installers (release)
npm run tauri build -- --debug      # Desktop, debug profile
```

### Rust core

```bash
cd crates/pointcloud-core
cargo test --release
cargo run --release --example bench -- 5000000   # A/B against the previous octree
cargo build --release --target wasm32-unknown-unknown --no-default-features
```

## Test data

Drop files in `dev-fixtures/` (git-ignored). A public COPC sample worth having is Autzen Stadium from [PDAL/data](https://github.com/PDAL/data) (`autzen-classified.copc.laz`, ~80 MB).

## License

LGPL-3.0-or-later — see [LICENSE.md](LICENSE.md).
