# Open Pointcloud Studio

An open-source point-cloud studio: a native Rust desktop application for opening, viewing, editing and exporting laser scans and other point clouds. Its source, tests and renderer are Rust and WGSL, in the workspace under [native/](native/README.md).

It opens LAS/LAZ, E57, PLY, PCD, PTX, text and mesh formats, handles multi-gigabyte surveys with bounded previews, a disk octree and WGPU rendering, and offers an OpenAEC-styled ribbon, a 3D view cube, a section box, full-resolution point selection with Delete/Undo/Redo, exports and terrain and surface meshing. Scanner stations of E57 projects are shown with their photos: click a station to stand in its panorama, and walk through the scene with `W`, `A`, `S` and `D`. See [native/README.md](native/README.md) for the feature status, [native/API.md](native/API.md) for the local command API, [native/TEST_DATA.md](native/TEST_DATA.md) for the test data sets and [screenshots/](screenshots/) for visual checks.

## Download

Builds for Windows, Linux and macOS are attached to the [releases](../../releases) of this repository. Each archive holds the application binary and the licence texts.

## Build from source

```bash
cd native
cargo build --release -p open-pointcloud-studio-native
cargo test --workspace
```

The executable is `native/target/release/open-pointcloud-studio` on Linux and macOS, or `native/target/release/open-pointcloud-studio.exe` on Windows. Start it with one or more files to open them directly.

## Earlier application

The earlier Tauri, React and Three.js application is archived. Its source is kept for reference under [classic/](classic/README.md) and is no longer developed; the state of the former main branch is tagged `archive/classic-tauri-main` and its last development work `archive/classic-tauri-dev`.

## License

The point-cloud core is LGPL-3.0-or-later — see [LICENSE.md](LICENSE.md). The desktop crate includes adapted OpenCADStudio ribbon code and SVG artwork and is GPL-3.0-only — see [native/desktop/LICENSE-GPL-3.0](native/desktop/LICENSE-GPL-3.0).
