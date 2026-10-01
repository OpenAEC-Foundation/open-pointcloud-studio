# Native Rust rebuild

This workspace is the active all-Rust replacement for the Tauri, React and Three.js application. The existing application stays available as a separate [Classic desktop tool](../classic/README.md) so each workflow can be checked against it.

The design follows [OpenCADStudio](https://github.com/HakanSeven12/OpenCADStudio): a Rust document and I/O core, a native `iced` user interface, and a viewport. The reference was cloned beside this repository and inspected at commit `1fec34d`. The native desktop ribbon now contains adapted source from `src/ui/ribbon/mod.rs`, `widgets.rs` and `draw_panel.rs`: its three-row panel packing, large/small tool columns, active-tab treatment, and tool button styling. The properties panel adopts the two-column rows from `src/ui/properties.rs`. See [`desktop/src/opencad_ribbon.rs`](desktop/src/opencad_ribbon.rs) and [`desktop/src/opencad_properties.rs`](desktop/src/opencad_properties.rs) for source attribution and adaptation notes. OpenCADStudio is GPL-3.0, so the native desktop crate is GPL-3.0-only; its license text is at [`desktop/LICENSE-GPL-3.0`](desktop/LICENSE-GPL-3.0). The separate pointcloud core remains LGPL-3.0-or-later.

The [OpenAEC style book](https://github.com/OpenAEC-Foundation/OpenAEC-style-book) was cloned beside this repository (commit `dfdcd41`). The native ribbon uses the old application's compact button grouping, OpenCADStudio's Rust three-row ribbon primitives and quick-access pattern, and OpenAEC's Deep Forge, Night Build, Scaffold Gray, Construction Amber and Warm Gold tokens. Its top strip now keeps Import, Export, Undo and Redo before the tabs, with muted disabled actions and tooltips. Its tab strip, group captions and active/hover states follow the style-book ribbon tokens; wide tool groups scroll horizontally with visible left/right controls instead of being clipped. Unavailable Cancel mesh and Export mesh actions do not consume ribbon space; they appear when a mesh job or surface makes them relevant. The Home ribbon has native Deep Forge, Blueprint Light, Night Build, Blueprint Blue and High Contrast choices. The selection persists in `open-pointcloud-studio-native/theme` under the XDG configuration directory (or `~/.config`); the CAD viewport stays dark across themes. The old web ribbon's CSS and TypeScript components are not used in the native build. Inter and Space Grotesk are bundled as OFL-licensed native font assets. Visual checks are saved in [`../screenshots/`](../screenshots/).

On X11, the Rust desktop sets the system title bar's light/dark theme hint to
match the chosen native palette while retaining normal window-manager drag,
resize and controls. The local command API's `set_theme` command changes the
same palette and persists it, making both theme and window chrome testable in
the running development build.
The verified [light theme](../screenshots/native-blueprint-light-native-titlebar-114m.png)
and [dark theme with exact selection](../screenshots/native-theme-api-forge-114m-exact-selection.png)
screenshots show the 114,174,907-point merged AHN6 cloud in that build.

At the default 1440-pixel window width, the View tab packs all six axial
camera directions, isometric view, scanner and
section actions into OpenCADStudio's three-row small-tool columns and puts the
four class switches in a two-by-two block. The Tools tab keeps editing,
meshing and 3D BAG in view; the duplicate general export controls remain in
File, Home and Properties. Both tabs show their regular controls without
horizontal scrolling at this width.

The amber File tab opens a native backstage view with the currently open scans, direct scan activation, import, full/selected/section/mesh export, format choice and appearance choice. The File view covers the tool ribbon and model space, while keeping quick access and the status bar visible; Escape or Return to model closes it. Unavailable exports appear muted. This uses the existing Rust import/export commands and no web components.

Native display and indexing defaults now persist in `settings.json` under the same configuration directory as the theme: color mode, point size, eye-dome switch and strength, scanner-marker visibility, point budget, auto-index and the four broad classification groups. New configurations start at a 250,000-point viewport budget; the ribbon can raise it to ten million, matching Classic. Changes from the ribbon or local command API are saved after a short debounce; invalid stored numeric values fall back to safe defaults. Source scans and their per-file edits are unaffected.

OpenCADStudio's SVG icons under `assets/icons/` were copied into [`assets/opencad-icons/`](assets/opencad-icons/) and are embedded by Rust `iced::widget::svg`. No HTML, CSS, JavaScript or webview is used in the native desktop crate.

Camera views can be named and saved from the View ribbon or Properties panel, then restored or deleted from Properties. They persist per source scan in `camera-views.json` under the native XDG configuration directory.
E57 scan transforms, valid PCD `VIEWPOINT` headers and PTX scanner positions appear as station markers in the native 3D view. A PCD header with an all-zero orientation, found in public PCL samples, opens with identity orientation but has no invented scanner axes. For isolated stations, the small X/Y/Z axes show the registered scanner orientation; nearby stations grouped into one marker do not imply a shared orientation. The View ribbon frames stations together with the cloud; Properties lists each station's coordinates and axis directions. Click a marker or a station's **Center** button to pan the current view to that position without changing its angle or zoom. Use `--scans INPUT` to print positions and orientations.

The [opencadcodec](https://github.com/HakanSeven12/opencadcodec) repository was inspected at commit `5ef9376` (MPL-2.0). Its `PointCloudData`, `PointCloudExData`, definitions, clips and color maps model *DWG/DXF point-cloud references* and scan placement. Its `source_filename`/`source_files` fields link to scan data; this is not a LAS/LAZ/E57 point decoder or point-processing kernel. OpenCADStudio itself still reports `POINTCLOUDATTACH` as unimplemented and renders existing point-cloud CAD entities as frames/wires. Its `opencadkernel` dependency handles CAD curves and B-rep geometry, not the point stream. Our existing streaming decoders and disk octree therefore remain the scan engine. A future CAD-reference workflow should use `opencadcodec` to resolve and display attached scans and apply its transforms/crops, while keeping scan points on disk.

LAZ writing uses the `las` crate's parallel compressor with bounded 400,000-point
batches (eight default compression chunks). LAS/LAZ conversion also reads
source records in batches, preserving their original attributes and
coordinate transforms. Exact same-format LAS/LAZ export copies the source
bytes without recompression. Filtered or transformed exports keep the
original LAS coordinate grid when the source is LAS/LAZ.
All full-source LAS/LAZ operations now read bounded batches too: LAZ uses
400,000 points per request so the parallel decompressor can process several
compression chunks at once, while LAS uses 16,384-point batches. Callbacks
still receive points in source order and can cancel mid-batch.
Filtered LAS/LAZ exports use the same bounded input batches while retaining
the source's native LAS attributes and coordinate grid. The export scan itself
stays in the optimized Rust core when the desktop supplies a section, edit or
selection predicate.
Both meshers keep their full-source loops inside the optimized Rust core in
development builds; the native desktop supplies edit and progress callbacks
without recompiling those loops at the desktop's lower optimization level.
XYZ, PTS, CSV and ASCII PLY exports format bounded 65,536-point batches on
multiple Rust threads and write the finished chunks in source order. The
temporary output is published only after the full source and any selected
point count have been verified.

## Build

```bash
cd native
cargo run -p open-pointcloud-studio-native
cargo run -p open-pointcloud-studio-native -- /path/to/scan.laz
cargo run -p open-pointcloud-studio-native -- --export /path/to/scan.laz /path/to/scan.ply
cargo run -p open-pointcloud-studio-native -- --export /path/to/scan.las /path/to/scan.e57
cargo run -p open-pointcloud-studio-native -- --section /path/to/scan.laz 207440,474000,-100,208000,475000,1000 /path/to/crop.laz
cargo run -p open-pointcloud-studio-native -- --mesh /path/to/scan.laz /path/to/terrain.obj
cargo run -p open-pointcloud-studio-native -- --mesh-export /path/to/surface.off /path/to/surface.obj
cargo run -p open-pointcloud-studio-native -- --surface /path/to/scan.e57 /path/to/surface.obj
cargo run -p open-pointcloud-studio-native -- --surface /path/to/scan.e57 /path/to/surface.obj --max-vertices 100000 --neighbors 12 --edge-factor 4
cargo run -p open-pointcloud-studio-native -- --scans /path/to/scan.e57
cargo run -p open-pointcloud-studio-native -- --bag3d 91000,398000,92000,399000 2.2 /path/to/buildings.obj
cargo test --workspace
```

See [TEST_DATA.md](TEST_DATA.md) for large public datasets and repeatable
45.8-million-point and 129.4-million-point AHN6 stress tests.
The running Rust GUI also exposes a token-protected, local
[native command API](API.md) for status, layer and camera control, section
boxes, exact world-coordinate point selection, deletion/undo/redo, and full
or clipped exports. Its old JavaScript `/eval` bridge is not used.

The current native slice opens LAS, LAZ, PLY (ASCII and little-endian binary), PCD (ASCII, interleaved binary and disk-backed LZF binary-compressed), PTX, OBJ, OFF, STL, DXF, E57, XYZ, ASC, TXT, CSV and PTS files. LAS/LAZ metadata opens immediately from the header and a bounded preview samples spaced ranges without decoding every point. Other formats stream the entire source to calculate bounds and counts. At most 100,000 preview points are retained per file. The viewport draws round, lit point sprites with WGPU and a depth buffer, rebasing survey coordinates in double precision before GPU upload. Mesh faces use interpolated per-vertex normals for directional lighting; meshes without normals derive them from their triangles, and non-uniform or reflected live scales map them into world space. The viewport retains CPU geometry and uploaded GPU buffers across camera-only redraws; changed visibility, LOD points, colors, filters, edits or section bounds rebuild them. A screen-space eye-dome pass shades points and mesh faces by neighboring depth; it can be toggled in the View ribbon. The viewer supports multiple files, visibility, orbit with left drag, pan with middle or right drag, deep zoom around the cursor, and RGB/elevation/intensity/classification colors. A native 3D view cube follows the camera; click its six faces, visible corners or ISO button to snap the view while retaining the current zoom and pan. Right click without dragging opens a viewport menu. Escape closes the menu and exits box/pick selection. The section box in the View ribbon or right-click menu has six draggable 3D face handles, percentage sliders and precise XYZ coordinate fields in Properties. Apply XYZ limits to clip point and mesh rendering; the world-space limits remain fixed when another layer is shown or hidden. Full-resolution selection honors the same limits. **Export section** streams the source once, including points outside the preview sample, and patches the exact count into PLY/PTS headers or closes the LAS/LAZ/E57 writer with its final count before saving. Box selection visits exact source points in intersecting octree leaves when an index exists and otherwise streams the full source; compact bitsets retain original point ordinals. Select > Delete hides points immediately without changing the source. Undo and Redo restore or reapply up to eight deletion batches. Full and section export honor these edits, and decimation, translation, scale and both meshers also use the remaining source points. XYZ, PTS, CSV, PLY, LAS, LAZ and E57 export re-reads the source stream, so output does not lose points to preview sampling.

The Tools ribbon includes two meshers. Terrain mesh streams every source point through a bounded XY grid, keeps the lowest point in each cell, triangulates a 2.5D TIN in Rust and rejects long edges across gaps. Its default cap is 100,000 vertices; use `--mesh INPUT OUTPUT.obj` headlessly. The 3D surface command streams the complete source into a bounded reservoir of up to 200,000 candidates by default, spatially thins them to 50,000 vertices, estimates local normals and triangulates tangent-plane neighborhoods. It propagates face orientation across connected patches and recalculates normals from the final triangles. This can reconstruct vertical walls and overhangs, but sparse sampling leaves holes and independent neighborhoods can still create contradictory face cycles; the result is not guaranteed watertight. The native Tools tab shows Max vertices (3–1,000,000), Neighbors (3–32) and a positive Edge factor in Properties; invalid values are rejected before opening the save dialog. Use `--surface INPUT OUTPUT.obj` headlessly with the equivalent `--max-vertices`, `--neighbors` and `--edge-factor` options. Both atomically save Wavefront OBJ with source RGB where available and per-vertex normals, then show the result as GPU-rendered triangles. OBJ, ASCII or little-endian binary PLY, OFF, ASCII or binary STL, and DXF 3DFACE files can also display native GPU faces. Imported OBJ and PLY vertex colors render on the faces; aligned OBJ and PLY vertex normals survive OBJ export. Each imported mesh belongs to its source entry, so several meshes can render together. Points and surfaces have separate visibility controls in the project panel. Resident meshes are bounded to one million distinct vertices and two million triangles per file.

The Tools ribbon also opens a native 3D BAG panel. Enter a bounding box in RD New coordinates or copy it from the active scan/section box, choose LoD 1.2, 1.3 or 2.2, and save a georeferenced OBJ. The Rust client follows API pagination, applies each page's CityJSON transform, triangulates polygon holes, and loads the result as a separate surface layer. The panel now includes a fully native RD New map using [Kadaster BRT-A raster tiles via PDOK](https://www.pdok.nl/ogc-webservices/-/article/basisregistratie-topografie-achtergrondkaarten-brt-a-): draw a rectangle, pan, zoom or fit typed RD bounds, then download that exact area. The map displays [Kadaster/PDOK CC BY 4.0 attribution](https://www.pdok.nl/copyright/). Exported OBJ files preserve the [3DBAG CC BY 4.0 attribution](https://docs.3dbag.nl/nl/copyright/), and the viewer displays the required credit and license link while the buildings are visible.

File open and save dialogs use `rfd::AsyncFileDialog`, so the native UI stays responsive while the operating-system dialog is open. The running development build now displays one 1.208 GB merged AHN6 LAZ containing 114,174,907 points, with its full disk index attached and an exact 373,382-point selection highlighted. Three separate AHN6 tiles totaling 1.28 GiB and 129,398,587 points were also checked earlier.
The [Select ribbon screenshot](../screenshots/native-select-ribbon-zoom-selection-114m.png)
shows the selected area framed at 19.4× after the new Zoom selection action.

| Workflow in the existing app | Native status |
| --- | --- |
| LAS/LAZ, PLY, XYZ/ASC/TXT/CSV, PTS import | Implemented for point data; bounded ASCII and little-endian binary PLY polygon meshes also render |
| PCD, PTX, OBJ, OFF, STL, DXF, E57 import | Point vertices implemented, including PCD LZF compression and VIEWPOINT transforms in all three PCD storage modes; OBJ, OFF, STL and DXF 3DFACE geometry also renders as triangles |
| Multiple clouds, visibility, orbit, pan, deep zoom, 3D view cube, right-click menu, rounded points, colors, point size, budget, class groups | Native implementation; named camera views save and restore yaw, pitch, zoom and pan per source scan. Per-code classification switches in Properties filter both rendering and exact selection, alongside the View ribbon groups. Advanced navigation polish remains |
| Section box | Three-axis clipping with visible wireframe, six draggable face handles, six limit sliders and precise XYZ fields. Fit box to selection uses exact selected source points, including points outside the preview; Zoom box frames the clipped volume in the viewport. Clipping applies to GPU rendering, full-resolution selection and a separate clipped export |
| Octree LOD and eye-dome lighting | Existing disk-backed octrees attach when a scan opens. Uncached scans with at least one million points are indexed automatically, one at a time, after their preview loads; the Tools ribbon can disable this or start a manual build. Source-read and tree-build progress appear in the status and Properties panels, and a running build can be cancelled without retaining a partial cache. Camera movement selects visible nodes by projected size and refreshes a bounded point sample while retaining the previous sample until its replacement is ready; stale requests cancel during node reads and leaf-preview generation. The point-budget control reaches 10 million, with point uploads split into bounded WGPU buffers. Compact per-leaf LOD previews make repeated cold-cache navigation cheaper; old indexes create these previews on first use without a full rebuild. Native screen-space eye-dome shading has an on/off switch and an adjustable 0–5 strength in View and Properties |
| Full-resolution point selection | Index-guided exact box selection when available, full-source fallback, exact single-point picking with or without an index, selected point properties and selected export. The Select ribbon's Zoom selection action frames the exact selected bounds without changing the section box; selection masks retain source-coordinate bounds so a later live transform and a gigabyte scan do not require a second full-source read. Long box scans can be cancelled from the Select ribbon, Escape or local API without applying partial results |
| Editing | Native Delete/Undo/Redo on original source ordinals across multiple clouds; exact-percentage Thin edits the open view and can be undone without copying the source. Translate and independent XYZ Scale now edit the open view through a lazy affine transform. The 3D view, disk-octree selection, section box, scan markers, exports and mesh display use the transformed coordinates. Scale uses the exact centroid of all remaining points as pivot, streaming large scans from the octree with progress and cancellation. Reset Transform restores source coordinates and reopens the full section if the old box no longer intersects the cloud. Export saves the edited coordinates; the source file stays unchanged. Crop and save-minus remain file-based |
| Surface reconstruction | Full-source 2.5D terrain TIN and bounded 3D local surface reconstruction to OBJ with native GPU face display; watertight and adaptive reconstruction remain |
| PLY, LAS, LAZ, E57, XYZ, PTS, CSV export | Full same-format LAS/LAZ/E57 export copies the original file byte-for-byte; full LAS↔LAZ conversion streams native LAS records with their metadata and point attributes. Filtered or transformed LAS/LAZ output preserves the original point format, GPS time, return data, 16-bit RGB, projection records and coordinate grid while applying edits to source records. New E57 output streams XYZ, RGB8 and intensity as one world-coordinate scan; E57 has no standard classification field in this writer, and absent individual color/intensity values are written as zero. Non-LAS input uses the common XYZ, RGB8, intensity and classification model |
| Multi-scan LAS/LAZ merge | The native File view and local API combine visible LAS/LAZ layers in the background, applying deletion and transform edits while preserving original point attributes. The task reports progress, supports cancellation, and publishes the output atomically. Sources with incompatible point layout, coordinate grid or metadata are rejected. The `--merge OUTPUT.laz INPUT1.las INPUT2.laz [...]` command exercises the same streaming core without launching the GUI |
| OBJ mesh export | Terrain and 3D surface meshers save RGB and per-vertex normals in OBJ; any resident OBJ, PLY, OFF, STL or DXF triangle mesh can also be exported from the Tools ribbon or Properties. Imported OBJ/PLY colors and aligned normals survive conversion. The writer saves atomically and keeps 3DBAG attribution where applicable |
| 3DBAG | Native RD map with PDOK raster tiles, rectangle drawing, pan/zoom, typed/scan/section-box bounds, LoD choice, paginated CityJSONFeatures import and GPU mesh display |
| Themes | Five native OpenAEC palettes, selected from Home and persisted locally; model space remains dark |
| Settings and automation API | Theme, display/indexing defaults and named camera views persist in native configuration files. A local token-protected Rust command API controls open layers, camera, visibility, section boxes, exact point selection, deletion/undo/redo, percentage thinning and exports; more commands and settings remain to port |

Mesh export writes all vertices and faces from the mesh currently held by the viewer, validates indices before touching the destination, and saves atomically. The Tools ribbon and Properties panel expose it for imported OBJ, PLY, OFF, STL and DXF meshes. The `--mesh-export INPUT OUTPUT.obj` command supports batch conversion; 3DBAG output retains the required attribution header.

## Migration work remaining

1. Improve viewport LOD with predictive loading and smooth transitions between node levels. The native app already builds and reuses disk-backed indexes automatically for large scans, reads compact leaf previews for repeated camera movements, and cancels stale LOD reads during navigation. Releasing an orbit or pan drag starts the latest detail request immediately; an older in-flight request is cancelled and the delayed timer cannot launch a duplicate. Indexed clouds share the viewport budget by projected coverage; any unused allocation is returned to visible clouds. Budgets above 500,000 points now display a 250,000-point first pass before refining to the requested budget. The existing Rust octree and binary IPC code in `src-tauri/src/pointcloud/` is a reference, but its all-points-in-memory build is unsuitable for large surveys.
2. Broaden PCD testing beyond the public PCL XYZ and RGB LZF samples to more producer fields, and validate meshes from real producer variants, materials and large models. Keep each decoder in `core`.
3. Improve the bounded 3D surface mesher toward watertight output and richer source attributes. Broaden the native settings UI as remaining workflows migrate.
4. Expand the documented native command API to remaining editing and selection actions, then retire the old frontend and Tauri packaging after feature parity checks.

No existing application files are removed by this first slice.
