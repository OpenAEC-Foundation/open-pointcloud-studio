# Open Pointcloud Studio

A cross-platform pointcloud viewer built with Tauri, React, and Three.js.

## Features

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

LGPL-3.0-or-later — see [LICENSE.md](LICENSE.md).
