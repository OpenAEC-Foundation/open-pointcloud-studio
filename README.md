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

## Build the native application

```bash
cd native
cargo build --release -p open-pointcloud-studio-native
```

The native executable is `native/target/release/open-pointcloud-studio-native` on Linux and macOS, or `native/target/release/open-pointcloud-studio-native.exe` on Windows. Its source, dependencies, tests and renderer are Rust and WGSL; it does not use the Classic Tauri frontend. The earlier web/Tauri source now lives entirely under [`classic/`](classic/README.md) for comparison and separate use.

## License

The original application and native pointcloud core are LGPL-3.0-or-later — see [LICENSE.md](LICENSE.md). The native desktop crate includes adapted OpenCADStudio ribbon code and SVG artwork and is GPL-3.0-only — see [native/desktop/LICENSE-GPL-3.0](native/desktop/LICENSE-GPL-3.0).
