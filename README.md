# Open Pointcloud Studio

An open-source point-cloud studio being rebuilt as a native Rust desktop application. The active Rust workspace is [native/](native/README.md); the earlier Tauri, React and Three.js app is available as a separate [Classic desktop tool](classic/README.md) until feature parity is reached.

## Native Rust development

```bash
cd native
cargo run -p open-pointcloud-studio-native
cargo test --workspace
```

The native application currently opens LAS/LAZ/E57 and other point formats, handles multi-gigabyte surveys with bounded previews and WGPU rendering, offers an OpenAEC-styled ribbon, an interactive 3D view cube, a right-click navigation menu and a three-axis section box and a native 3D BAG area map, and performs indexed full-resolution point selection, live Delete/Undo/Redo, section-box export, file-based transforms and terrain meshing to OBJ. See [native/README.md](native/README.md) for the current feature status, [native/TEST_DATA.md](native/TEST_DATA.md) for the 1.37 GB AHN6 test set, and [screenshots/](screenshots/) for visual checks.

Run the separate old desktop app locally with `./scripts/classic.sh run`. See [classic/README.md](classic/README.md) for its local release and source-build instructions.

## Earlier application

- Import LAS/LAZ pointcloud files
- Color modes: RGB, Elevation, Classification, Intensity
- Adjustable point size and point budget
- Eye-Dome Lighting (EDL)
- Classification filtering (ASPRS)
- Octree-based LOD rendering
- Dark, Light, Blue, and High Contrast themes

## Getting Started

### Prerequisites

- Node.js 18+
- Rust 1.70+

### Development

```bash
npm install
npm run dev          # Frontend only
npm run tauri dev    # Full Tauri app
```

### Build

```bash
npm run tauri build
```

## License

The original application and native pointcloud core are LGPL-3.0-or-later — see [LICENSE.md](LICENSE.md). The native desktop crate includes adapted OpenCADStudio ribbon code and SVG artwork and is GPL-3.0-only — see [native/desktop/LICENSE-GPL-3.0](native/desktop/LICENSE-GPL-3.0).
