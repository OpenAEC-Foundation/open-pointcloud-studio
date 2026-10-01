# Large point-cloud test data

The files themselves are deliberately kept outside the repository. Check the
source terms before redistributing a dataset or screenshots from it.

| Dataset | Size / scale | Format | Intended test |
| --- | --- | --- | --- |
| [AHN6 tile `207000_474000`](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_207000_474000.LAZ) | 485,317,717 bytes; 45,839,678 points | LAZ 1.4 | Large classified geographic scan and octree |
| [AHN6 tile `208000_474000`](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_208000_474000.LAZ) | 300,181,990 bytes; 29,688,001 points | LAZ 1.4 | Adjacent horizontal tile |
| [AHN6 tile `207000_475000`](https://fsn1.your-objectstorage.com/hwh-ahn/AHN6/01_LAZ/AHN6_2025_C_207000_475000.LAZ) | 589,160,933 bytes; 53,870,908 points | LAZ 1.4 | Adjacent vertical tile |
| [Stanford 3D Scanning Repository](https://graphics.stanford.edu/data/3Dscanrep/) | Dragon scans: 2,748,318 points; Lucy raw scans: 58,241,932 points | PLY range data for Dragon; Lucy raw scans use SD | Object scan and mesh fidelity; check the repository's non-commercial terms |
| [OpenTopography Mariposa Grove mobile lidar](https://portal.opentopography.org/dataspace/dataset?opentopoID=OTDS.112025.32611.1) | One listed LAZ scan: 786.31 MB and 153,181,399 points | LAZ | Follow-up stress test at higher density |
| [pye57 test scans](https://github.com/davidcaron/pye57/tree/master/tests/test_data) | `test.e57`: 160,838 valid points; `testSpherical.e57`: 155,201 valid points | E57 | Cartesian and spherical scan decoding, pose handling |
| [3DBAG API](https://docs.3dbag.nl/nl/delivery/webservices/) | 1 km RD bounding box: 81 buildings, 3,414 vertices, 3,800 triangles across four pages | CityJSONFeatures | Native building import, page transforms, LoD 2.2, license credit |
| [PDOK BRT-A WMTS](https://www.pdok.nl/ogc-webservices/-/article/basisregistratie-topografie-achtergrondkaarten-brt-a-) | 256×256 raster tiles in EPSG:28992 | PNG | Native RD area map, rectangle drawing, pan and zoom |

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
