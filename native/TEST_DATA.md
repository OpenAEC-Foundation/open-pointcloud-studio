# Large point-cloud test data

The files themselves are deliberately kept outside the repository. Check the
source terms before redistributing a dataset or screenshots from it.

On 1 October 2026, the merged 114,174,907-point AHN6 LAZ was exported to a
3,425,247,452-byte binary PLY. The previous native reader scanned this file
in 56.61 s on one core; after bounded, parallel binary decoding it took
18.13 s with 32,836 KiB peak RSS. A second full-source visit counted exactly
114,174,907 points. The source PLY remains at
`/tmp/open-pointcloud-AHN6-merged-114m-parallel.ply` for native GUI tests.
The native GUI loaded its 100,000-point preview and reported the full count;
see the [3.2 GiB PLY overview](../screenshots/native-3.2gib-ply-114m.png).
Without an index, a full-source world-box selection over X 208000–208100,
Y 475000–475100 and Z 0–100 found 432,975 exact points. The
[overview](../screenshots/native-3.2gib-ply-432k-selection.png) and
[19.4× selection view](../screenshots/native-3.2gib-ply-432k-selection-zoom.png)
show the selection in the Rust GUI.
During the subsequent manual index build, a physical right-drag moved camera
pan by exactly `[60, 30]` pixels while keeping the 432,975-point selection.
The [index-progress view](../screenshots/native-3.2gib-ply-index-progress.png)
shows the GUI and its cancellable source-read progress.
The completed PLY octree has 7,852 nodes, 6,770 leaves and depth 6. Reopening
the 3.2 GiB file in the latest native build attached the cached index and
returned a 250,000-point LOD. Repeating the same exact world selection took
0.56 s including API polling and again found 432,975 points; the unindexed
scan had taken about 29 s. The [indexed close-up](../screenshots/native-3.2gib-ply-indexed-selection-latest.png)
shows the selection with detail from the full disk octree.
On that indexed scan, typing `f` into the camera-view name left zoom at 19.4×.
Pressing `F` outside a text field fitted the whole model at 1.0× while
retaining all 432,975 selected points; the
[keyboard Zoom All screenshot](../screenshots/native-f-zoom-all-114m-ply.png)
shows the resulting overview.
The native command API then set an exact camera pose on the indexed PLY
(`yaw=0.25`, `pitch=0.4`, `zoom=0.05`, `pan=[120,-80]`). An invalid pitch of
2.0 radians was rejected without changing that pose. `zoom_all` restored the
isometric 1.0× view and zero pan. Another indexed world-box selection found
the same 432,975 points, and `zoom_selection` framed them at 19.4× in the
[latest native dev build](../screenshots/native-114m-ply-camera-selection.png).
The indexed PLY cache now includes exact cloud metadata. A first `--index`
run with the new build took 18.50 s to read the source and backfill that
metadata; the next run reopened and validated the same 7,852-node index in
0.15 s, using 18,712 KiB peak RSS. A fresh native GUI instance reached its
250,000-point octree LOD without scanning the 3.2 GiB source, then selected
the same 432,975 exact points and framed them at 19.4×. See the
[cached reopen screenshot](../screenshots/native-cached-3p2gib-ply-114m.png).
A full-bounds indexed selection over the same 114,174,907-point PLY was
started and its layer removed while the job was pending. The job reached
`cancelled` in 0.196 s from request, with no cloud or selection left in the
viewer. Repeating the test by hiding the layer cancelled in 0.085 s and left
zero selected points. Both changes stop the disk scan instead of waiting for
the full-source result to be discarded.
The refreshed native dev build remains open on that PLY with a 250,000-point
LOD and the 432,975-point exact selection; see the
[current screenshot](../screenshots/native-current-114m-ply-selection.png).
After adding native camera-view commands, a fresh development instance reopened
the same indexed 3.2 GiB PLY and displayed its 250,000-point LOD. The exact
world-box selection over X 208000–208100, Y 475000–475100 and Z 0–100 again
found 432,975 points in 0.45 s including API polling. `zoom_selection` framed
them at 19.38×, and `save_camera_view` persisted the pose for that source.
The [current native screenshot](../screenshots/native-current-114m-ply-camera-api.png)
shows the selected points and saved view in Properties.

On 1 October 2026, the native GUI opened a separate 1,200,000-point LAS test
source and reported octree source-reading progress. `cancel_index` stopped its
first cold build, leaving the layer unindexed and no completed cache for that
source. Reopening the same file then showed `reading_source`, `building_tree`
and `ready` through the native API; the final disk index had 64 leaves and the
layer became indexed. The live Properties progress bar and Cancel control are
shown in [`native-index-progress.png`](../screenshots/native-index-progress.png).
The same LAS source exercised native `select_world`, `export_selection` and
`export_minus_selection`: the exact world box selected 150,258 points, and the
two XYZ exports contained 150,258 and 1,049,742 lines respectively. All
1,200,000 output points were on the expected side of the selection boundary;
their counts sum to the complete source. An export request without an active
selection was rejected before creating an output file.
In a later native dev build, that 1,200,000-point LAS was opened twice as
separate layers. Both received a 100,000-point preview and their own cached
index. With the first copy hidden, a world-box selection picked all 1,200,000
points in the second copy only. Delete, Undo and Redo changed only that copy;
the first kept zero selected and deleted points throughout. The
[two-layer screenshot](../screenshots/native-duplicate-las-layer-selection.png)
shows the independent selection after Undo.

On 1 October 2026, the bounded multithreaded text writer exported a
1,200,000-point LAS fixture to CSV in 2.54 s with 27,556 KiB peak RSS. The
previous point-by-point writer took 3.75 s and 20,028 KiB on the same file.
Both outputs had 1,200,001 lines including the header and were byte-identical
(SHA-256 `5460d40b74a725a1961e83dea66a19809e4bddec9197e533f83759de7292d8ab`).
An order/count regression test crosses the 65,536-point batch boundary for
XYZ, PTS, CSV and ASCII PLY.
The same writer exported the public AHN6 `207000_474000` LAZ tile to XYZ:
45,839,678 output lines, 2,582,457,357 bytes, 456.10 s wall time and
41,268 KiB peak RSS. The final file appeared only after the temporary output
was complete. Its SHA-256 is
`142bec5664af80226ca0447fb56e80c8a82a4395c3c15d7da053122f8b12e09f`.
The native viewer then exercised cancellable octree LOD reads on that same
45.8-million-point tile. A 2,000,000-point viewport request was interrupted
by successive Top, Front, Right and Isometric camera commands; the final
Isometric request reached `Viewport LOD ready: 2000000 points from disk octree`
3.49 seconds after the last command. Two physical right-drag gestures changed
the camera pan to `[45, 124]` and returned a fresh 2,000,000-point LOD.
[`native-stale-lod-cancel-ahn2m.png`](../screenshots/native-stale-lod-cancel-ahn2m.png)
shows the final dense view. The core test also cancels a leaf-preview build
after it has begun reading, verifies that no partial preview is published,
and successfully rebuilds it on the next request.
The Tools ribbon was checked visually against the separately running Classic
v0.3 build for button coverage and against the OpenAEC-styled native ribbon for
appearance. At 1440 px, the left and right controls expose both the first
editing groups and the final 3D BAG and Export groups; an 1100 px resize kept
the same controls usable. See [`start`](../screenshots/native-ribbon-scroll-tools-start.png),
[`end`](../screenshots/native-ribbon-scroll-tools-end.png), and
[`1100 px`](../screenshots/native-ribbon-scroll-tools-narrow.png).
The latest native ribbon was also checked against OpenCADStudio's Rust
quick-access row. Its Import, Export, Undo and Redo actions now precede the
tabs in the same compact pattern; disabled actions remain visible. The
45.8-million-point AHN6 viewer with this top strip is shown in
[`native-ribbon-quick-access-opencad.png`](../screenshots/native-ribbon-quick-access-opencad.png).
The same development build opened three adjacent public AHN6 LAZ tiles
(1,374,660,640 source bytes and 129,398,587 points), attached all three disk
indexes and rendered a combined 79,998-point LOD; see
[`native-ribbon-quick-access-three-ahn.png`](../screenshots/native-ribbon-quick-access-three-ahn.png).
With these same three scans open at 1440×900, the compact native
[View ribbon](../screenshots/native-view-ribbon-compact-ahn6.png) displays all
camera, scanner, point display, depth, section, budget and classification
groups at once. The [Tools ribbon](../screenshots/native-tools-ribbon-compact-ahn6.png)
shows LOD, auto-index, translate, scale, thin, decimate, surface and 3D BAG
without clipping; export format remains in the File backstage and Properties.
Clicking Bottom and then Isometric in the View ribbon changed the native camera
status to `BOTTOM` and `ISOMETRIC` respectively.
An exact world box across the first two tile boundaries selected 5,365 and
6,416 points respectively (11,781 total), with zero from the third tile.

A larger exact world selection used X 207250–207750, Y 474250–474750 and Z
-100–1000 with the three AHN6 tiles open. It selected 11,019,954 points from
the first tile in 6.58 seconds; the other two contributed zero. The viewer
retains the exact ordinal mask and shows a representative 8,000-point
highlight preview. [The selection screenshot](../screenshots/native-ahn6-11m-exact-selection.png)
shows the full count in Properties. Native Delete, Undo, Redo and Undo changed
the first tile's deleted count from 0 to 11,019,954 and back without changing
its 45,839,678 source points. Export Selection wrote an 118,442,155-byte LAZ
in 26.72 seconds; a full reread counted the same 11,019,954 points in 4.28
seconds. The selected LAZ remains locally at
`/tmp/ops-ahn6-selection-11m.laz` for follow-up tests. The development view
was returned to zero selected and zero deleted points afterward.

A full-scene world selection across the same three indexed AHN6 files was
started, then cancelled by each of the native API, the Select ribbon's
**Cancel selection** button, and Escape. Each 129,398,587-point search ended
with a `cancelled` job and zero selected points, without publishing a partial
mask. The [ribbon screenshot](../screenshots/native-cancel-selection-ribbon.png)
shows the button while the exact scan is running. A new selection immediately
after Escape completed with the same 11,019,954 exact points in 6.87 seconds;
the development view was cleared afterward.

Native Thin was run through the local command API on the first AHN6 tile with
45,839,678 source points. Keeping 50% hid exactly 22,919,839 points in 2.16
seconds; a second run took 4.11 seconds while the GUI was active. Properties
showed 22,919,839 remaining and 22,919,839 deleted in the
[Tools screenshot](../screenshots/native-ahn6-thin-45m-50-percent.png). Undo
restored all 45,839,678 points in the open view. The source LAZ was not edited.
The API rejected keep percentages 0 and 101.

At a two-million-point viewport budget, the three cached AHN6 octrees first
returned a new 250,000-point view after right-drag pan in 0.16–0.46 seconds,
then refined to the full 2,000,000 points in 2.28–2.41 seconds. The previous
single-pass pan took 2.18 seconds before new octree detail appeared. The
[first-pass screenshot](../screenshots/native-ahn6-progressive-preview-2m.png)
and [full-detail screenshot](../screenshots/native-ahn6-progressive-full-2m.png)
show the same camera position during and after refinement. The old sample
remains visible during the first load, so navigation itself is immediate.

The same three indexed tiles were used to check deep-zoom LOD allocation at
0.08046× zoom. An equal split of the 80,000-point budget rendered 32,810
points because two tiles could supply only 2,048 and 4,096 visible points.
Weighting by projected viewport coverage and returning unused capacity to the
tile under the camera rendered all 80,000 points (73,856 from that tile).
After a 100 × 40 pixel pan, an initial 41,000–42,000-point sample appeared after
about 0.27 seconds and the full 80,000-point refinement after about 0.60 seconds on
this development machine. The final view is in
[`native-ahn6-progressive-lod-final.png`](../screenshots/native-ahn6-progressive-lod-final.png).

At the same three-tile overview, increasing the viewport budget from 80,000
to 250,000 points took about 0.43 seconds to reach the full LOD; 500,000 took
about 0.54 seconds. After a controlled 100 × 40 pixel pan with warm disk
caches, refinement finished in about 0.21 and 0.32 seconds respectively.
The 250,000-point view, chosen as the new default for fresh settings, is in
[`native-ahn6-250k-default.png`](../screenshots/native-ahn6-250k-default.png).

On 1 October 2026, the native GUI mesher reported reading progress on the
45,839,678-point AHN6 tile while the viewport remained responsive. Cancelling
after more than two million points stopped the job and preserved an existing
OBJ destination byte for byte. The visible progress and Cancel control are
shown in [`native-ahn-mesh-progress.png`](../screenshots/native-ahn-mesh-progress.png).
A separate 30 × 30 XYZ grid exposed an overly strict default terrain edge
limit; after using the median local Delaunay neighbor spacing, the same native
API mesh command completed with 900 vertices and 1,682 triangles. The test
for two distant scan patches still confirms that their gap is not bridged.

| Dataset | Size / scale | Format | Intended test |
| --- | --- | --- | --- |
| [AHN6 tile `207000_474000`](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_207000_474000.LAZ) | 485,317,717 bytes; 45,839,678 points | LAZ 1.4 | Large classified geographic scan and octree |
| [AHN6 tile `208000_474000`](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_208000_474000.LAZ) | 300,181,990 bytes; 29,688,001 points | LAZ 1.4 | Adjacent horizontal tile |
| [AHN6 tile `207000_475000`](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_207000_475000.LAZ) | 589,160,933 bytes; 53,870,908 points | LAZ 1.4 | Adjacent vertical tile |
| [AHN6 tile `208000_475000`](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_208000_475000.LAZ) | 318,219,261 bytes; 30,615,998 points | LAZ 1.4 | Third tile with matching compound CRS for a >1 GB merged-file test |
| [Stanford 3D Scanning Repository](https://graphics.stanford.edu/data/3Dscanrep/) | Dragon scans: 2,748,318 points; Lucy raw scans: 58,241,932 points | PLY range data for Dragon; Lucy raw scans use SD | Object scan and mesh fidelity; check the repository's non-commercial terms |
| [OpenTopography Mariposa Grove mobile lidar](https://portal.opentopography.org/dataspace/dataset?opentopoID=OTDS.112025.32611.1) | One listed LAZ scan: 786.31 MB and 153,181,399 points | LAZ | Follow-up stress test at higher density |
| [pye57 test scans](https://github.com/davidcaron/pye57/tree/master/tests/test_data) | `test.e57`: 160,838 valid points; `testSpherical.e57`: 155,201 valid points | E57 | Cartesian and spherical scan decoding, pose handling |
| [PCL couch](https://github.com/PointCloudLibrary/data/blob/master/tutorials/kinfu_large_scale/Tutorial_Cloud_Couch_bin_compressed.pcd) | 7,529,032 bytes; 968,520 points | LZF-compressed PCD | Official producer sample with an all-zero `VIEWPOINT` quaternion |
| [PCL region-growing RGB](https://github.com/PointCloudLibrary/data/blob/master/tutorials/region_growing_rgb_tutorial.pcd) | 2,286,562 bytes; 307,200 records, 259,847 finite XYZ points | LZF-compressed PCD | Organized RGB sample with a valid rotated `VIEWPOINT` |
| [PCL room scan](https://github.com/PointCloudLibrary/data/blob/master/tutorials/room_scan1.pcd) | 603,904 bytes; 112,586 points | LZF-compressed PCD | Ordinary producer scan with identity `VIEWPOINT` |
| [3DBAG API](https://docs.3dbag.nl/nl/delivery/webservices/) | 1 km RD bounding box: 81 buildings, 3,414 vertices, 3,800 triangles across four pages | CityJSONFeatures | Native building import, page transforms, LoD 2.2, license credit |
| [PDOK BRT-A WMTS](https://www.pdok.nl/ogc-webservices/-/article/basisregistratie-topografie-achtergrondkaarten-brt-a-) | 256×256 raster tiles in EPSG:28992 | PNG | Native RD area map, rectangle drawing, pan and zoom |

On 1 October 2026, the Rust reader visited all 968,520 finite points in the
PCL couch file, all 112,586 in the room scan, and 259,847 finite points in the
organized RGB file (the remaining records contain non-finite XYZ). The couch
sample previously failed at its all-zero `VIEWPOINT`; it now opens with a
neutral orientation and no invented station marker. The RGB file rendered in
the native WGPU view with its color and valid sensor axes, shown from above in
[`native-pcl-rgb-lzf-top.png`](../screenshots/native-pcl-rgb-lzf-top.png).
The local files can be downloaded from the linked PCL data repository and
checked with `cargo run -p pointcloud-core --example visit_bench -- FILE.pcd 2000000`.

The [AHN dataroom](https://www.ahn.nl/dataroom) describes the point-cloud
products and their download map. The three adjacent AHN6 tiles total
1,374,660,640 bytes (1.28 GiB) and 129,398,587 points. Their direct URLs
were checked on 30 September 2026.

Example local test:

```bash
for tile in 207000_474000 208000_474000 207000_475000; do
  curl --fail --location \
    --output "/tmp/AHN6_2025_C_${tile}.LAZ" \
    "https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_${tile}.LAZ"
done
cd native
cargo run -p open-pointcloud-studio-native -- /tmp/AHN6_2025_C_*.LAZ
```

The GUI attaches completed disk-backed octrees from the user cache. If an
uncached scan has at least one million points, it starts an index build after
loading the preview. Only one build runs at a time; the project panel marks
each file as `Indexing` or `LOD queued`. The Tools ribbon can turn automatic
indexing off or start a manual build. Indexes can occupy several gigabytes.
The optional `--index INPUT.laz` command builds or reuses an index headlessly
and prints its point, node and leaf totals. The viewport selects visible
octree nodes by projected screen size and refreshes up to the chosen point
budget after camera movement.

On 30 September 2026, the AHN6 tile indexed successfully: 45,839,678 points,
2,948 nodes, 2,543 leaves, depth 5, 449.3 seconds in the initial native
development build. On 1 October, the cached index build completed in 72.3
seconds with 16 MB peak RSS and a second CLI run reopened it in 0.07 seconds.
Both pye57 scans also indexed successfully.

On 1 October, the 45,839,678-point AHN6 tile also exercised compact leaf LOD
previews. The existing octree was reused; its first 80,000-point sample built
281 leaf preview files in 1.64 s. After those files existed, alternating
previous-reader and new-reader runs with `POSIX_FADV_DONTNEED` applied to the
index files measured 1.55–1.59 s versus 0.54–0.55 s respectively. The same
`/usr/bin/time -v` runs reported about 784,000 versus 105,000 file-system
input blocks. Both readers returned exactly 80,000 indexed points. A
2,000,000-point request completed in 2.09 s with 96,716 KiB peak RSS. The
repeatable benchmark command is:

```bash
cd native
cargo run -p pointcloud-core --example lod_bench -- /tmp/open-pointcloud-AHN6_2025_C_207000_474000.LAZ 80000
```

The running WGPU viewport was checked after geometry caching was added. A
middle-button pan moved the 80,000-point AHN6 view while keeping the cloud
visible and LOD ready; see
[`native-ahn-pan-render-cache.png`](../screenshots/native-ahn-pan-render-cache.png).
The renderer test checks that camera, point-size and eye-dome redraws reuse
the same CPU geometry, while color, classification filters, section clipping
and replacement LOD points rebuild it. GPU vertex/index uploads follow that
geometry identity; the camera uniform still updates on every redraw.
During a continuous middle-button pan on the header-only 45.8-million-point
AHN6 LAZ, the previous 80,000-point LOD sample remained visible while the
background request was delayed and refreshed. Properties showed **View sample
80,000** instead of the source header's zero preview points; see
[`native-ahn-continuous-pan-lod.png`](../screenshots/native-ahn-continuous-pan-lod.png).
A native test verifies that orbit, pan, zoom, budget and section changes retain
an existing sample when the source has no preview, reject a stale LOD result,
and replace the sample only when a current result arrives.

The native 3DBAG client was checked against the live
[`pand/items` API](https://api.3dbag.nl/collections/pand/items) on 1 October
2026. The RD box `91440,398430,91460,398450` returned one LoD 2.2 building
with 62 vertices and 56 triangles. The larger box
`91000,398000,92000,399000` returned 81 buildings, 3,414 vertices and 3,800
triangles across four API pages in 0.78 seconds with 24,308 KiB peak RSS.
The GUI save flow attached the downloaded OBJ as a separate native mesh layer;
see [`native-3dbag-gui-download.png`](../screenshots/native-3dbag-gui-download.png).
The four-page mesh reopened with the required 3DBAG credit visible in
[`native-3dbag-81-buildings-faces.png`](../screenshots/native-3dbag-81-buildings-faces.png).
The same building mesh was clipped by the native section box at X minimum
34%; see [`native-3dbag-section-box.png`](../screenshots/native-3dbag-section-box.png).
The native 3D BAG area map was then checked against the live PDOK BRT-A
EPSG:28992 WMTS. Drawing a 235 × 218 m rectangle in central Amsterdam set
the RD box to `120764.80,486925.58,121000.00,487143.98`. Pan and zoom moved
the map while the selected world area stayed fixed. Downloading the drawn box
at LoD 2.2 loaded 155 buildings, 17,345 vertices and 21,528 triangles from
seven API pages into the GPU view. The OBJ retains 3DBAG attribution; the map
shows Kadaster/PDOK attribution and a license link. See
[`native-3dbag-native-map-selected.png`](../screenshots/native-3dbag-native-map-selected.png)
and [`native-3dbag-native-map-download.png`](../screenshots/native-3dbag-native-map-download.png).
The saved OBJ reopened in the final dev build beside the native map, with
the building extent fitted in RD and both PDOK and 3DBAG credits visible:
[`native-3dbag-native-map-final.png`](../screenshots/native-3dbag-native-map-final.png).

The three AHN6 tiles were also opened together in the native desktop: the
status bar reported 129,398,587 source points. A viewport box selection
scanned all three original LAZ files and selected exactly 59,914,918 points
(36,320,671 + 6,002,741 + 17,591,506). The process used about 182 MB RSS
after selection. This validates the bitset-based full-source selection, but
the viewport initially uses a bounded preview. On 1 October 2026, the first
45.8-million-point tile was indexed through the GUI and a deep zoom loaded
80,000 local detail points from the octree while the process used about
168 MB RSS. Automatic refresh on zoom and pan was then verified with a
250,000-point synthetic point cloud; a deep zoom loaded 5,776 local points
without pressing a detail button. On 1 October, the native viewport also
attached the cached 45.8-million-point AHN index automatically at initial
fit, then displayed 80,000 points from visible disk octree nodes. The visual
result is [`native-auto-lod-ahn.png`](../screenshots/native-auto-lod-ahn.png).
The latest dev build reopened all three tiles together (129,398,587 source
points, about 166 MB RSS). Its cached first-tile index was attached before
the other two LAZ previews finished decoding. Zoom and right-drag pan each
refreshed the bounded octree sample. Enabling the section box and moving its
X-maximum slider to 56% clipped the combined scene and updated the orange
3D bounds; see
[`native-section-box-three-tiles-clipped.png`](../screenshots/native-section-box-three-tiles-clipped.png).
On 1 October, a full-resolution section export of the first 45,839,678-point
AHN6 LAZ tile used X minimum 44% (RD X = 207440), with the other five bounds
at their full extent. The native GUI wrote a 802,922,263-byte binary PLY with
26,764,067 points. Its patched vertex-count header matches exactly the
26,764,067 fixed-width records on disk. A separate complete record scan found
X = 207440.000…207999.999, Y = 474000.000…474999.999 and
Z = 3.236…68.503, all inside the box. The dev process remained around
166 MiB RSS while the LAZ was read once. See
[`native-section-box-45m-export-complete.png`](../screenshots/native-section-box-45m-export-complete.png).
The saved PLY also reopened in the latest native dev build with the same
26,764,067 points and X range 207440…208000; see
[`native-section-box-45m-reopened.png`](../screenshots/native-section-box-45m-reopened.png).
The XYZ coordinate fields were also exercised in the native GUI on the
45,839,678-point AHN6 tile: X = 207250…207750 and Y = 474250…474750 clipped
the WGPU view and updated the six sliders to 25…75%. See
[`native-section-box-xyz-input-45m.png`](../screenshots/native-section-box-xyz-input-45m.png).
The native Select ribbon was then tested on this 45,839,678-point LAZ tile
with its cached disk octree. A drawn rectangle selected 14,696,666 exact
source ordinals through intersecting octree leaves. Delete left 31,143,012
points in the open view, Undo restored all 45,839,678, and Redo removed the
same 14,696,666 again. Re-selecting the deleted rectangle returned zero
points. The source LAZ was left unchanged. Screenshots:
[`selected`](../screenshots/native-edit-45m-selected.png),
[`deleted`](../screenshots/native-edit-45m-deleted.png),
[`undo`](../screenshots/native-edit-45m-undo.png),
[`redo`](../screenshots/native-edit-45m-redo.png).
The native ribbon was then checked against the local OpenCADStudio Rust ribbon
source and OpenAEC dark-theme tokens. The Home and Tools views are captured in
[`native-ribbon-ocad-openaec-home.png`](../screenshots/native-ribbon-ocad-openaec-home.png)
and [`native-ribbon-ocad-openaec-tools.png`](../screenshots/native-ribbon-ocad-openaec-tools.png).
The compact Tools translation group shows X, Y and Z without clipping. Raising
the Home point budget to about 1.9 million loaded **1,897,694** visible points
from the cached octree on the 45,839,678-point tile; the debug process used
about **298 MiB RSS** afterward. See
[`native-45m-budget-1p9m.png`](../screenshots/native-45m-budget-1p9m.png).
At a 900-pixel-wide window, mouse-wheel scrolling across the Tools ribbon
revealed the previously hidden Surface, 3D BAG and Export groups; see the
[`start`](../screenshots/native-ribbon-narrow-tools-start.png) and
[`scrolled`](../screenshots/native-ribbon-narrow-tools-scrolled.png) views.
The native theme picker was exercised in the running 45.8-million-point GUI.
Screenshots show [Deep Forge](../screenshots/native-theme-forge-45m.png),
[Blueprint Light](../screenshots/native-theme-light-45m.png),
[Night Build](../screenshots/native-theme-night-45m.png),
[Blueprint Blue](../screenshots/native-theme-blue-45m.png) and
[High Contrast](../screenshots/native-theme-contrast-45m.png). The point cloud
and view cube remained visible in each theme. The High Contrast choice survived
a process restart from the XDG config file; Deep Forge was restored afterward.
The Classic Tools screenshot also exposed independent X/Y/Z scale inputs that
were missing in the native ribbon. They now appear beside XYZ translation in
[`native-scale-xyz-tools-45m.png`](../screenshots/native-scale-xyz-tools-45m.png).
The native GUI exported a two-point XYZ fixture with scale X=2, Y=-1, Z=0.5
to binary PLY; decoding that file gave `(0, 4, 3.5)` and `(4, 2, 4.5)` with
the correct two-vertex header. The UI result is in
[`native-scale-xyz-ui-export.png`](../screenshots/native-scale-xyz-ui-export.png).
On 1 October, a fresh native GUI run automatically built the two remaining
AHN6 tile indexes in sequence while keeping the viewport interactive. All
three project rows then showed `LOD ready` for 129,398,587 source points;
the on-disk index cache occupied 6.4 GiB and the running process used about
166 MiB RSS. A restart attached all three caches immediately from their
headers. The corrected Tools ribbon and a clipped section-box view of the
fully indexed scene are shown in
[`native-auto-index-tools-ribbon-fixed.png`](../screenshots/native-auto-index-tools-ribbon-fixed.png)
and [`native-auto-index-three-ahn-section-box-final.png`](../screenshots/native-auto-index-three-ahn-section-box-final.png).

The native terrain mesher streamed the complete `207000_474000` LAZ tile
(45,839,678 points) into an 8,384,293-byte OBJ with 95,872 vertices and
191,104 validated faces. This took 612.66 seconds and peaked at 46,800 KiB
RSS in the development build. The mesh has a bounded number of vertices and
rejects long edges across gaps. A separate E57 test scan of 160,838 points
produced 7,793 vertices and 15,381 faces.
The general 3D surface mesher scanned that same E57 file and produced a
5.4 MB OBJ with 50,000 sampled vertices and 161,907 triangles in 5.99 seconds,
peaking at 50,740 KiB RSS. Its GPU-rendered surface is shown in
[`native-e57-3d-surface-faces.png`](../screenshots/native-e57-3d-surface-faces.png),
and the native Tools ribbon exposes both mesh modes in
[`native-3d-surface-tools-ribbon.png`](../screenshots/native-3d-surface-tools-ribbon.png).
The E57 scan was also reconstructed through the native Tools button and GTK
save dialog. The generated OBJ was attached to the original scan in the same
session as a separately toggled surface layer; see
[`native-e57-surface-from-gui.png`](../screenshots/native-e57-surface-from-gui.png).
The general 3D mesher also streamed all 45,839,678 points of AHN6 tile
`207000_474000` and wrote a 4.7 MB OBJ with 50,000 vertices and 126,716
triangles in 389.94 seconds, peaking at 50,248 KiB RSS. It reopened as a
native GPU surface; see
[`native-ahn6-3d-surface-faces.png`](../screenshots/native-ahn6-3d-surface-faces.png).
This sampled mesh has gaps, so the terrain TIN remains the preferred mode for
AHN ground coverage.
On 1 October, the 3D mesher used a bounded 200,000-point candidate reservoir
and spatial thinning to choose its final 50,000 vertices. Reprocessing the
same 45,839,678-point AHN6 tile produced 153,658 triangles in 417.36 seconds
with 67,964 KiB peak RSS. The old mesh used 126,716 triangles in 389.94
seconds with 50,248 KiB peak RSS. With both vertex sets projected onto the
same 100-by-100 XY grid, occupied cells rose from 9,360 to 9,568; on a
200-by-200 grid they rose from 26,222 to 30,237. The GPU-rendered result with
the point layer hidden is in
[`native-ahn6-surface-spatial-faces.png`](../screenshots/native-ahn6-surface-spatial-faces.png).
The E57 test scan yielded 50,000 vertices and 172,820 triangles in 5.90
seconds with 60,968 KiB peak RSS. These coverage counts measure selected
vertices, not a watertight surface guarantee; terrain TIN remains preferable
for continuous ground coverage.
The native E57 writer was checked with a full copy of the public pye57
`test.e57` fixture: the 14 MB copy has the same SHA-256 as its source, including
all scan poses and metadata. A separate section export streamed all 160,838
valid points through the writer into one world-coordinate E57 scan in 3.20
seconds with 25,304 KiB peak RSS. Exporting the original and this new E57 to
binary PLY yielded byte-identical 160,838-point files (XYZ and intensity).
With the scan-preserving filtered writer, the native dev build exported the
section `301358.2,5042487.0,89.0,301358.7,5042489.0,91.0` from the same
public file. Reopening the E57 found 85,865 points and all four original scan
positions and orientations. A sequential comparison against the same section
of the source XYZ stream found zero world-coordinate deviation and identical
point attributes for every exported point. The reopened E57 also rendered in
the native dev build with four stations listed in Properties; see
[`native-e57-filtered-scans.png`](../screenshots/native-e57-filtered-scans.png).
The public spherical `testSpherical.e57` was also filtered through the same
writer with section `-1,-5,-2,1,-1,2`: 102,616 of 155,201 valid points were
retained. Reopening and exporting them to XYZ gave the same 102,616 points,
with zero coordinate deviation and identical attributes compared point by
point to the matching source stream.
The filtered writer now copies selected raw E57 records using each source
scan's original prototype and value types. A synthetic two-station test checks
that 16-bit RGB and a row-index field survive alongside poses and per-scan
names. Re-running both public sections with the new writer produced 85,865 and
102,616 points, kept the first file's four scan positions, and yielded XYZ
streams byte-identical to the earlier verified outputs.
The 1,200,000-point LAS fixture exported to E57 in 5.13 seconds with 22,288
KiB peak RSS, and the E57 reopened and exported to a 1,200,000-point PLY in
5.00 seconds with 21,736 KiB peak RSS. Comparing that PLY with a direct LAS
export found zero differences in XYZ or intensity across all points. This LAS
fixture has no RGB; the E57 reader now leaves RGB absent instead of deriving a
synthetic grey color from intensity. The writer emits RGB and intensity when
present, but new E57 exports flatten scan stations and do not carry LAS
classification; a same-format unedited E57 copy preserves all original bytes.
The same 1,200,000-point LAS fixture was also exported through the native GUI's
E57 picker and GTK save dialog. Reopening the saved E57 and comparing its PLY
output with direct LAS-to-PLY again found zero XYZ or intensity differences.
The earlier Tools export picker with E57 and the 45.8-million-point AHN6 tile
is preserved in
[`native-e57-export-ribbon.png`](../screenshots/native-e57-export-ribbon.png);
the current export-format picker is in File and Properties.
The AHN6 OBJ was reopened in the native GUI and its 191,104 triangles were
rendered with the source point layer hidden. The visual result is saved as
[`native-terrain-mesh-faces.png`](../screenshots/native-terrain-mesh-faces.png).
The same terrain mesh was converted to a 4.8 MB little-endian binary PLY and
reopened in the native GUI. Its 95,872 vertices and 191,104 polygon triangles
rendered as a continuous surface with the point layer hidden; see
[`native-ply-mesh-ahn-faces.png`](../screenshots/native-ply-mesh-ahn-faces.png).
An offset copy of that PLY was then opened together with the OBJ. Both
191,104-triangle surfaces rendered at once; point visibility and surface
visibility could be changed independently for each file. See
[`native-multiple-mesh-surfaces-only.png`](../screenshots/native-multiple-mesh-surfaces-only.png)
and [`native-multiple-mesh-one-hidden.png`](../screenshots/native-multiple-mesh-one-hidden.png).
The same terrain was converted to OFF (7.3 MB), binary STL (9.6 MB) and
ASCII DXF 3DFACE (28.3 MB). The native viewer rendered 191,104 triangles
from each format. Binary STL and DXF repeated face vertices in their point
streams, but their resident meshes deduplicated those to 95,872 vertices.
The visible surfaces are shown in
[`native-off-stl-surfaces-only.png`](../screenshots/native-off-stl-surfaces-only.png)
and [`native-dxf-3dface-surface-only.png`](../screenshots/native-dxf-3dface-surface-only.png).
The WGPU eye-dome pass was checked on the 160,838-point E57 scan and on the
three-tile AHN6 scene. Turning it on changed 38,607 viewport pixels at the
initial AHN fit and 64,815 after zooming, with unchanged camera and source
data. On the 191,104-triangle AHN terrain mesh, the pass reveals relief lines
that disappear when disabled. The section box still clips the shaded mesh at
X maximum 56%; see [`native-edl-mesh-on.png`](../screenshots/native-edl-mesh-on.png),
[`native-edl-mesh-off.png`](../screenshots/native-edl-mesh-off.png) and
[`native-edl-section-mesh-clipped.png`](../screenshots/native-edl-section-mesh-clipped.png).
The 160,838-point E57 scan was used to check the interactive view cube,
right-click menu, Escape handling, right-drag pan, spherical point sprites,
and section-box clipping. Visual checks are in
[`../screenshots/`](../screenshots/).
Dragging the X-minus face handle on the AHN terrain OBJ changed the X minimum
from 0% to 23% and clipped the mesh immediately; see
[`native-section-handle-drag.png`](../screenshots/native-section-handle-drag.png).
The View ribbon and Properties panel now include **Fit box to selection**.
A native test loads only one preview point from a 10,000-point XYZ file,
selects source ordinals 2 and 9,999, then verifies the fitted box contains
their exact full-source bounds on all three axes. Selected ranges remain
available even when the preview omitted them. In the 45,839,678-point AHN6
GUI, a source point was picked through the disk octree and the new ribbon
action fitted the six XYZ limits around that point; see
[`native-section-box-fit-selection-ribbon-45m.png`](../screenshots/native-section-box-fit-selection-ribbon-45m.png)
and [`native-section-box-fit-one-selected-45m.png`](../screenshots/native-section-box-fit-one-selected-45m.png).
The **Zoom box** command then framed the clipped volume inside the 45.8-million-point
viewport without moving its world-space XYZ limits. A narrower X/Y/Z section
filled about three quarters of the viewport; see
[`native-section-box-zoom-detailed-45m.png`](../screenshots/native-section-box-zoom-detailed-45m.png).
A 10-point GUI fixture also
verified the new Thin percentage control: 30% exported exactly source points
3, 6 and 9 to binary PLY; see
[`native-thin-30-percent-ui-export.png`](../screenshots/native-thin-30-percent-ui-export.png).
LAS and LAZ were added to the native export picker and the `--export` command.
A GUI export of the 10-point XYZ fixture to LAZ was reopened headlessly and
contained exactly 10 points; see
[`native-laz-export-gui.png`](../screenshots/native-laz-export-gui.png).
Core tests also exported a preview-limited PLY source to LAS and LAZ while
retaining RGB, intensity and classification. Full LAS-to-LAS and LAZ-to-LAZ
exports were byte-identical to their sources. LAS-to-LAZ and LAZ-to-LAS
conversions stream original LAS point records; a round trip test checks GPS
time, return numbers, 16-bit RGB and projection metadata. An out-of-range
coordinate failed without publishing a partial LAS file. Filtered and
transformed LAS/LAZ output now writes the original LAS point records with only
the edited fields changed. The source point format, GPS time, return data,
16-bit RGB, projection records and coordinate grid remain intact. The common
point model for non-LAS input still contains only XYZ, RGB8, intensity and
classification.

Parallel LAZ compression was exercised with 1,200,000 deterministic XYZ points:
the generated LAZ header reported exactly 1,200,000 points and the native
octree index reopened all of them. On this host an eight-worker run took
13.83 seconds (115% total CPU, 102 MiB peak RSS), versus 15.04 seconds
(99% CPU, 87 MiB RSS) with `RAYON_NUM_THREADS=1`. Input text parsing dominates
this small benchmark, so these figures do not predict throughput for larger
LAS/LAZ conversions. The batch size remains bounded at 400,000 points.

The native `--section` command was then run on the public 463 MB AHN6 tile with
45,839,678 points, selecting X = 207980..208000, Y = 474000..475000 and Z =
-100..1000. It wrote an 8.6 MB LAZ with 876,086 points in 418 seconds and
130 MiB peak RSS. The native octree index reopened all 876,086 points. The
output retained the source's compressed point format 7, 42-byte point record,
0.001 coordinate scales and zero offsets. Its LAS header X range was
207980.000..207999.999, inside the requested section.

PCD `VIEWPOINT` now rotates and translates every point in ASCII, binary and
binary-compressed files. A 90° Z-rotation plus translation was checked in all
three modes; a zero quaternion is rejected. A separate native camera-view
storage test verifies per-source save/reload and rejects an invalid zoom.
In the running native GUI, a view of the 45.8-million-point AHN6 scan was
saved as `View 1`, the camera was switched to Top, then `View 1` restored.
After restarting the dev build, `View 1` was still listed for that scan.
Screenshots: [`native-camera-views-saved.png`](../screenshots/native-camera-views-saved.png)
and [`native-camera-view-restored.png`](../screenshots/native-camera-view-restored.png).

The pye57 `test.e57` fixture exposes four scanner positions through the E57
scan transforms. The native Properties panel lists four stations and the View
ribbon's **Fit stations** action frames their markers together with the
160,838-point cloud; see
[`native-e57-scan-positions-framed.png`](../screenshots/native-e57-scan-positions-framed.png).
Two E57 scans share one position; the latest view groups their overlapping
screen markers as **2 stations** while Properties keeps all four records:
[`native-e57-scan-positions-grouped.png`](../screenshots/native-e57-scan-positions-grouped.png).
The expanded Properties list's **Center** action moved Scan 4 to the middle
of the viewport without changing the -46°/34° orbit angle or 3.510× zoom;
clicking another station marker then centered that station as well. See
[`native-e57-center-station.png`](../screenshots/native-e57-center-station.png).
The same position collection is checked by PTX and PCD parser tests.
The native reader now also retains E57 quaternion, PCD `VIEWPOINT` quaternion
and PTX registered basis directions. `--scans` on `test.e57` reported four
orientations; Scan 1's X axis was `(-0.444356, -0.895850, 0)` and Scan 4's
was `(0.999620, 0.027552, 0)`. The GUI showed the corresponding small axes
at each ungrouped marker and in the expanded Properties list. Grouped markers
do not show one scan's orientation on behalf of several scans. See
[`native-e57-scan-orientation.png`](../screenshots/native-e57-scan-orientation.png).

The GUI also opened a 22,801-point XYZ grid with **Indexed: No**. Pick point
selected source point 11,550 at X -2.000, Y 1.000, Z 0.593 by scanning the
source in the background; the Properties panel displayed those coordinates.
The native test separately verifies a picked ordinal beyond a one-point
preview, deletion filtering and section-box filtering. See
[`native-pick-without-index.png`](../screenshots/native-pick-without-index.png).

The native GUI imported the AHN-derived OFF terrain mesh (95,872 vertices,
191,104 triangles), displayed its faces and exported the resident mesh as
OBJ through the Tools ribbon. The GUI output matched the independent CLI
`--mesh-export` output byte for byte; see
[`native-mesh-export-ahn-off.png`](../screenshots/native-mesh-export-ahn-off.png).
The final CLI writer converted the 7.3 MB OFF input to a 6.4 MB OBJ in 1.65
seconds with 24,352 KiB peak RSS. Reimporting and exporting that OBJ again
gave the same SHA-256 hash (`0edab73e…f6ee42`). A 3DBAG OBJ converted through
the same path retained its copyright, CC BY 4.0 URL and EPSG:7415 header.

The native command API was exercised against a running build with the cached
45,839,678-point AHN6 octree attached. `select_world` on XYZ bounds
`[207950, 474000, -100]` to `[208000, 474050, 1000]` selected 125,763
source ordinals in about 0.52 seconds; the selection job reached `complete`
and `status.selected_points` agreed. A separate 10-point XYZ layer selected
ordinals 3 through 6 by world coordinates. API Delete, Undo and Redo changed
its remaining count from 10 to 6, back to 10, then to 6. The small layer was
removed and the AHN6 layer restored to visible afterward.
A second AHN6 API selection covered X 207400..207600 and Y 474400..474600,
returning 1,569,701 exact source ordinals. The top-view GUI highlighted a
bounded 8,000-point sample of that selection over a 250,000-point viewport
LOD; see [`native-api-select-world-ahn45m.png`](../screenshots/native-api-select-world-ahn45m.png).

The native per-code ASPRS controls were then checked on the same indexed AHN6
tile. With every class visible, the 200 × 200 m world box selected 1,569,701
points. Disabling class 2 (Ground) through the typed API left 1,015,681
selected points, a difference of 554,020. `status.hidden_classes` reported
`[2]`. At unchanged top camera and 250,000-point viewport budget, 98,963
pixels changed in the model-space portion of the screenshots. The full-class,
ground-hidden and scrolled Properties views are saved as
[`all classes`](../screenshots/native-class-filter-all-ahn45m.png),
[`without Ground`](../screenshots/native-class-filter-no-ground-ahn45m.png) and
[`class controls`](../screenshots/native-class-filter-controls-ahn45m.png).

The native WGPU eye-dome strength control was checked on the same 45.8-million-
point AHN6 tile at an unchanged isometric camera and 250,000-point viewport
sample. Setting strength 0 and then 5 through the native API changed 124,279
pixels in the model-space viewport crop; the ribbon label and slider showed
both requested values. See [`strength 0`](../screenshots/native-eye-dome-strength-0-ahn45m.png)
and [`strength 5`](../screenshots/native-eye-dome-strength-5-ahn45m.png).
The live development build was returned to the default strength 1 afterward.
The expanded native point-size range also accepted 0.1 and 20, rejected 0.05,
and was restored to 2 before the screenshots.

On 1 October 2026, `--mesh` streamed the complete public AHN6
`207000_474000` LAZ tile (45,839,678 points) into a colored terrain OBJ.
The run produced 95,872 vertices, 95,872 per-vertex normals and 191,104
triangles in 383.52 seconds with 59,040 KiB peak RSS. The 18 MB OBJ was
opened in the running Rust GUI beside the three indexed AHN6 tiles; the
other tiles were hidden temporarily to inspect the colored faces in
[`native-ahn6-45m-rgb-terrain-mesh.png`](../screenshots/native-ahn6-45m-rgb-terrain-mesh.png).
All three source tiles were then made visible again (129,398,587 points).
`--mesh-export` reimported and rewrote the generated OBJ in 2.51 seconds with
28,020 KiB peak RSS. An independent record comparison found identical values
for all vertex coordinates and RGB channels, identical face indices, and a
maximum normal-component difference of `5.0e-8` from decimal formatting.
The next native WGPU build used those normals for interpolated directional
lighting. With the OBJ's point layer hidden, its 191,104 colored faces
remained visible in
[`native-ahn6-45m-lit-terrain-faces.png`](../screenshots/native-ahn6-45m-lit-terrain-faces.png).
The desktop geometry test also checks derived normals for a mesh without
normal attributes and their orientation after a reflected scale.

The 3D surface mesher was also run on the complete AHN6 `207000_474000` LAZ
tile on 1 October 2026. Its first run read all 45,839,678 points and saved
50,000 colored vertices, 50,000 normals and 153,658 triangles in 323.92
seconds with 84,972 KiB peak RSS. The native GUI rendered the faces without
the point layer in
[`native-ahn6-45m-3d-surface-faces.png`](../screenshots/native-ahn6-45m-3d-surface-faces.png).
The screenshot shows substantial holes; this sampled local reconstruction is
not a substitute for the continuous 2.5D terrain TIN on this aerial scan.
An independent edge audit found no edges used by more than two faces, but
44,660 shared edges were traversed in the same direction by both faces.

After adding connected-patch winding propagation and recalculating vertex
normals from the final faces, a second complete LAZ run produced the same
50,000 vertices and 153,658 triangle sets in 349.12 seconds with 109,104 KiB
peak RSS. It reoriented 72,528 triangles without removing any; the shared
edges with matching directions fell to 12,228, while the 89,144 open boundary
edges remained. The revised result is shown in
[`native-ahn6-45m-3d-surface-oriented.png`](../screenshots/native-ahn6-45m-3d-surface-oriented.png).
Contradictory cycles and sampling holes still need a stronger reconstruction
algorithm.

The current native Tools and API meshing jobs now honor the active section box
and visible classifications as well as deleted points. On the public pye57
`test.e57`, a centered XYZ section retained 17,591 of 160,838 source points;
the 3D surface job produced 5,000 vertices and 16,497 triangles, and every
OBJ vertex stayed inside the requested world-coordinate section. On the public
45,839,678-point AHN6 `207000_474000` LAZ, a 100 m by 100 m section retained
350,550 points with all classes visible. Hiding ground class 2 reduced this
to 229,603 points. The filtered 3D surface had 5,000 vertices and 13,939
triangles; the filtered terrain TIN used the same 229,603 points and had 987
vertices and 1,934 triangles. All output vertices were inside the section.
See the native [surface](../screenshots/native-ahn6-section-class-mesh-45m.png)
and [terrain](../screenshots/native-ahn6-section-terrain-45m.png) views with
the section box active.

The 50,000 selected XYZ vertices were replayed as a separate source to tune
the local triangulation in about 3–8 seconds per setting. Raising the edge
factor from 4 to 6 or 8 added only 260 or 300 faces and increased open edges.
Raising neighbor count from 12 to 16, 24 or 32 increased faces to 191,635,
255,714 or 302,355 but also increased open edges to 117,199, 167,858 or
205,331. The default remains 12 neighbors and edge factor 4. The headless
`--surface` command now accepts `--max-vertices`, `--neighbors` and
`--edge-factor` for further controlled tests.

The native Tools-tab Properties panel was then exercised with a colored
50,000-point XYZ fixture extracted from that AHN6 surface sample. Its
Max vertices, Neighbors and Edge factor fields displayed `25000`, `12` and
`4` in
[`native-surface-settings-tools-25k.png`](../screenshots/native-surface-settings-tools-25k.png).
The typed native API rejected a zero vertex limit without changing the
settings, then accepted 25,000. Starting `surface` meshing in the same running
GUI returned a job ID and completed with exactly 25,000 vertices, 25,000
normals and 76,875 triangles from all 50,000 source points. The point layer
was hidden while its resulting colored mesh and chosen settings were captured
in [`native-surface-settings-25k-mesh.png`](../screenshots/native-surface-settings-25k-mesh.png).

The shared LAS/LAZ full-source reader was changed from single-point calls to
bounded `read_points_into` batches. This activates `las`'s parallel LAZ
decompressor on full scans while retaining ordered callbacks and cancellation.
On the same public 45,839,678-point AHN6 `207000_474000` tile, reading the
first 2,000,000 points fell from 2.360 to 0.852 seconds in the native debug
build. A full read took 17.645 seconds with 111,316 KiB peak RSS. The complete
3D surface command then fell from the earlier 349.12 to 145.70 seconds with
137,460 KiB peak RSS. Its 50,000-vertex, 153,658-triangle OBJ was byte-for-byte
identical to the earlier oriented mesh (SHA-256
`089cc49965a17b09de32b51038796847b0db0f2eae952f5556ddc54af14b0742`).
Repeat the bounded reader timing with
`cargo run -p pointcloud-core --example visit_bench -- FILE.laz 2000000`.

A phase-timed core run of the same 3D surface job took 20.165 seconds: 18.777
for reading and sampling, 0.308 for spatial thinning, 0.526 for neighbor
search, 0.036 for normal estimation, 0.418 for triangulation/orientation and
0.100 for OBJ writing. The native CLI repeated its slower 145.89-second time
before the mesh functions were changed to call non-generic internal functions
compiled in `pointcloud-core`'s optimized development profile. With that
change, the native `--surface` command took 19.09 seconds and `--mesh` terrain
took 20.74 seconds on the same 45,839,678-point tile. Both new OBJ files were
byte-for-byte identical to their respective earlier outputs; the terrain OBJ
hash is `b5b832ff2750a1ec542895fe8dd01355ac262310ba7f5ccbe8ead99c64305453`.
Peak RSS was 135,064 KiB for 3D surface and 135,640 KiB for terrain.
Repeat the phase measurement with
`cargo run -p pointcloud-core --example surface_bench -- FILE.laz OUTPUT.obj`.
The rebuilt native GUI reopened all three indexed AHN6 tiles (129,398,587
source points). Its typed `mesh` command reconstructed terrain from the active
45,839,678-point tile as a background job in 29.878 seconds, yielding the same
95,872 vertices and 191,104 triangles. Its OBJ was byte-for-byte identical to
the earlier terrain output. With all point layers temporarily hidden, the
GPU-rendered surface was captured in
[`native-ahn6-45m-optimized-gui-terrain.png`](../screenshots/native-ahn6-45m-optimized-gui-terrain.png).
The three point layers were then made visible again.

A filtered LAZ export scanned all 45,839,678 source points but wrote only the
125,763 points inside X 207950..208000, Y 474000..474050 and the full Z range.
Before batching the raw LAS/LAZ reader and moving the generic export scan into
the optimized core, the native `--section` command took 323.06 seconds with
65,304 KiB peak RSS. The revised command took 17.58 seconds with 148,976 KiB
peak RSS. Both LAZ files were byte-for-byte identical (SHA-256
`562a55682dde3d6f8c673e3ec5c1747a792260f4b8ff3944a271f3e8908b694e`).
The rebuilt GUI then used its native section box and background export job to
write the same 125,763-point LAZ in 26.666 seconds, also with the identical
hash. The section box was cleared after the job; all three indexed source
tiles remain open in the dev build.

On the three indexed AHN6 tiles (129,398,587 points), a native right-button
drag moved the camera pan by 100 × 40 screen pixels. On mouse release the
viewport began refining its octree sample within 0.077 seconds and reported
79,998 ready points by 0.131 seconds. Status checks at 0.253 and 0.503 seconds
remained ready, showing that the older 220 ms debounce timer did not start a
duplicate read. The panned native view is in
[`native-ahn6-immediate-pan-lod-three-tiles.png`](../screenshots/native-ahn6-immediate-pan-lod-three-tiles.png).
A left-button orbit changed yaw by 0.40 radians and pitch by 0.16 radians;
its new detail request appeared at 0.012 seconds after release and was ready
by the 0.253-second status check.

The native viewport also uses the pointer position at mouse release to finish
right/middle-button pan, left-button orbit and box selection. This preserves
the final movement when the release arrives before a separate move event is
processed. A controlled 100 × 40 pixel right-button drag in the three-tile
AHN6 scene changed camera pan by exactly that amount, left no context menu
open, and returned to an 80,000-point LOD. See
[`native-right-pan-release-ahn6.png`](../screenshots/native-right-pan-release-ahn6.png).

The native multi-scan writer merged the visible-compatible AHN6 tiles
`207000_475000`, `208000_474000` and `208000_475000` (1,207,562,184 source
bytes, 114,174,907 points) into one 1,207,954,772-byte LAS 1.4 LAZ at
`/tmp/open-pointcloud-AHN6-merged-114m.laz`. The final header reports all
114,174,907 points and preserves the 0.001 m coordinate grid. An attempted
merge with `207000_474000` was rejected before writing because that tile has
different CRS records (horizontal-only versus compound RD New + NAP). In the
native GUI, the [File view](../screenshots/native-merge-file-view-83m.png)
shows the merge action for two visible scans. An API-triggered merge displayed
[live progress](../screenshots/native-merge-progress-ahn6.png); cancellation
after 3,600,000 processed points returned a `cancelled` job and preserved the
existing destination byte for byte. The core regression fixture uses LAS 1.4
point format 7 with GPS time, 16-bit RGB, return fields, scanner channel and
four extra bytes. It checks those raw attributes after a filtered and
transformed LAZ merge.
The merged file was then fully reread to build its disk octree: 114,174,907
points, 7,852 nodes, 6,770 leaves and depth 6 in 169.7 seconds. The native
viewer opened it alone, attached the cached index, and rendered a 250,000-point
LOD, shown in [the 1.2 GB viewer screenshot](../screenshots/native-merged-114m-1p2gb-view.png).
An exact world box crossing the source-tile boundary (X 207950–208050,
Y 474950–475050, Z -100–1000) selected 373,382 points in the merged file.
The viewer retained the exact selection and displayed 8,000 representative
highlights; see [the selection screenshot](../screenshots/native-merged-114m-selection-373k.png).

For an edited-cloud selection check, the native viewer deleted 303,276 source
points in X 208200–208300, Y 475200–475300. It then selected the disjoint
373,382 points in X 207950–208050, Y 474950–475050 and framed them at 18.6×.
The `zoom_selection` API call and completion poll took 135 ms on this host.
The exact cached source bounds were reused after comparing selection and
deletion bitmasks, without decoding the 1.208 GB LAZ again. The
[screenshot](../screenshots/native-selected-after-disjoint-delete-114m.png)
shows both counts; Undo restored all 303,276 deleted points afterward.

The same indexed 114,174,907-point LAZ was opened in a rebuilt native dev
viewer with a 10,000,000-point budget. The current isometric view returned
5,451,630 visible LOD points in about four seconds, requiring three WGPU
point buffers (at most 2,000,000 points per buffer). The renderer stayed live,
used about 641 MiB process RSS after refinement, and displayed that sample in
[the high-budget screenshot](../screenshots/native-10m-budget-114m-ahn6.png).
The local command API rejected a budget of 10,000,001, and restoring the
250,000-point default returned a ready viewport sample. The requested budget
is a ceiling; view-dependent octree culling can return fewer points.

At 270× zoom into the same merged AHN6 LAZ, the former node-preview path
returned 250,000 candidate points at the default budget; raising that budget
to ten million returned 559,825 candidates. Many candidates projected off
screen, leaving the [default-budget view](../screenshots/native-deep-zoom-node-preview-114m.png)
visibly thin. The new bounded
exact-leaf path read the intersecting leaves, filtered source points through
the current camera and retained 12,400 points actually inside the viewport
at the default 250,000-point budget. The
[deep-zoom screenshot](../screenshots/native-deep-zoom-exact-visible-114m.png)
shows the denser result at the same camera position. A right-button pan of
50 × 20 pixels at this zoom changed the camera by exactly that amount and
refreshed to 12,633 visible points within the 0.3-second polling interval.
Zoom All restored the overview and its 250,000-point LOD.

The native point-pick button was exercised directly in the 114,174,907-point
GUI scene. One click selected exact source ordinal 89,630,851 (displayed as
point 89,630,852) at X 208096.792, Y 475119.231, Z 14.385 in about 0.5–0.75
seconds. The [selected-point screenshot](../screenshots/native-pick-exact-114m.png)
shows its source attributes and one-point count after Escape exited the tool.
On a second click, Escape was pressed while the octree lookup was running:
the pending flag cleared within the first 0.05-second status poll and the
[cancelled-pick screenshot](../screenshots/native-pick-cancel-114m.png) shows
zero selected points. A subsequent normal pick still selected the same exact
source ordinal, confirming that cancellation did not poison the next search.
A cancelled replacement pick also left the previously selected point intact.

On 1 October, the release 3D surface mesher was checked before and after
parallelizing its nearest-neighbor and normal calculations. For the public
160,838-point E57 test scan at 30,000 vertices, both runs produced 100,691
triangles and byte-identical OBJ files (SHA-256
`dbd0be442ab33f893ca5ec022549c652418b798fb6d3f0ce9ac2350a00e2dbce`).
Elapsed times on this host were 0.81 and 0.73 seconds. The public 45,839,678-
point AHN6 LAZ at 100,000 vertices produced 302,592 triangles and the same
byte-identical OBJ before and after the change (SHA-256
`0a24ddc678975c7aff3391f37303a74e4fa1730f60d92a6e0be1ffa7a355266a`);
elapsed times were 17.56 and 15.71 seconds. This whole-command measure also
includes reading and sampling every source point, so it is not a standalone
measure of the parallel reconstruction stage. A core regression test cancels
between parallel neighborhood batches and verifies the destination OBJ is
left untouched. The rebuilt native dev GUI reopened the E57 input and its
30,000-vertex OBJ; the [mesh screenshot](../screenshots/native-dev-e57-parallel-mesher.png)
shows the source points hidden and the generated 100,691 faces visible.
