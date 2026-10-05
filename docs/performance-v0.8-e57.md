# E57 reconstruction and DWG benchmark

Measured on 2026-10-05 with the native Rust dev build on an AMD Ryzen 7 3700U (8 logical CPUs, 13 GiB RAM). The source was a 1,146,370,048-byte E57 with 46,589,344 points, 9 scan stations and 54 station photos. Its octree preview/index had already been built. Times are wall time and memory is the measured peak resident set of the CLI process; OS file cache is excluded.

The running GUI was checked with the [full cloud](../screenshots/native-latest-e57-46m-new-build.png) and a [25° rotated section box](../screenshots/native-latest-rotated-vertical-section.png). The native point picker also returned an exact source ordinal and coordinates from this E57 ([close-up selection screenshot](../screenshots/native-latest-e57-46m-point-picked.png)).

| Operation | Result | Time | Peak process RAM |
| --- | --- | ---: | ---: |
| Whole-cloud 3D surface, 10% source sample, 0.10 m minimum mesh size, 50,000-vertex cap, cached octree | 4,658,663 source points accepted; 50,000 vertices; 128,575 triangles; 9.9 MB OBJ | 4.28 s | 96 MB |
| Same operation before reusing the cached octree, decoding the E57 again | 50,000 vertices; 128,550 triangles | 68.00 s | 95 MB |
| Convert the current surface mesh to the upstream native 3D DWG MESH entity | 5.1 MB DWG | 2.49 s | 68 MB |
| 2D plan from a 0.5 m point slab in a 10 × 15 m section box, with region hatches | 1,302,153 slab points; 69,626 drawn; 11 fill regions; 1.9 MB DWG | 1.38 s | 222 MB |
| 25° rotated vertical section from a 0.4 m slab, with region hatches | 1,772,089 slab points; 84,426 drawn; 2 fill regions; 2.3 MB DWG | 1.94 s | 263 MB |
| Whole-cloud closed mesh, 10% source sample, 0.20 m voxels | 74,502 vertices; 138,417 triangles; 81.4 mm reported 95% deviation | 6.53 s | 242 MB |
| Whole-cloud closed mesh, 10% source sample, 0.10 m voxels | 265,761 vertices; 488,157 triangles; 40.2 mm reported 95% deviation | 12.56 s | 602 MB |
| Convert that whole-cloud closed mesh to 3D DWG | 488,157 triangle faces; 16 MB DWG | 9.63 s | 144 MB |
| Whole-cloud closed mesh, 100% source points, 0.10 m voxels | 419,365 vertices; 793,319 triangles; 47.2 mm reported 95% deviation | 28.02 s | 766 MB |
| Closed mesh in the 10 × 15 × 8 m box (5,-40,-2) to (15,-25,6), 0.05 m voxels, 10% source sample | 126,518 vertices; 229,421 triangles; 29.2 mm reported 95% deviation | 13.27 s | 577 MB |
| Same closed-mesh box and voxels, 100% source points | 213,718 vertices; 396,816 triangles; 32.4 mm reported 95% deviation | 35.24 s | 754 MB |

The quick 3D surface is a whole-scan visual approximation: it can have holes and overlapping patches. In this run it had 76,269 open edges and 417 connected parts ([screenshot](../screenshots/native-latest-e57-10pct-surface.png)). Use **Closed mesh** for a more faithful reconstruction ([5 cm room](../screenshots/native-v0.8.0-e57-room-closed-5cm.png), [10 cm whole scan](../screenshots/native-latest-e57-all-closed-10pct-10cm.png)). Its parallel tiled pipeline keeps memory bounded and now supports a deterministic source percentage; lower percentages save fitting work but can lose sparse detail. The whole-scan 10 cm result still has 53,350 open edges and 29 connected parts; it is not a watertight CAD solid. Reported deviations are computed against the points each run accepted, so figures for different source percentages are not a direct quality comparison. Inspect geometry and validate against the full point set before relying on a sampled mesh.

The 3D DWG contains one mesh entity with shared vertices. The 2D files come from **Section drawing** in the latest upstream build: a bounded occupancy grid is traced into a small number of region hatches, rather than one hatch per projected mesh triangle. These drawings are based on the measured point slab, so the 3D mesh can be inspected alongside them without forcing its open edges into the 2D cut.

The source image and point data are not currently baked into a UV texture; see the [surface-texture issue draft](issue-drafts/surface-texture.md).
