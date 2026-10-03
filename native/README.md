# Native Rust rebuild

This workspace is the active all-Rust replacement for the Tauri, React and Three.js application. The earlier application is archived: its source stays under [classic/](../classic/README.md) for reference, and the former main branch is tagged `archive/classic-tauri-main`.

The design follows [OpenCADStudio](https://github.com/HakanSeven12/OpenCADStudio): a Rust document and I/O core, a native `iced` user interface, and a viewport. The reference was cloned beside this repository and inspected at commit `1fec34d`. The native desktop ribbon now contains adapted source from `src/ui/ribbon/mod.rs`, `widgets.rs` and `draw_panel.rs`: its three-row panel packing, large/small tool columns and tool button styling. The properties panel adopts the two-column rows from `src/ui/properties.rs`. See [`desktop/src/opencad_ribbon.rs`](desktop/src/opencad_ribbon.rs) and [`desktop/src/opencad_properties.rs`](desktop/src/opencad_properties.rs) for source attribution and adaptation notes. OpenCADStudio is GPL-3.0, so the native desktop crate is GPL-3.0-only; its license text is at [`desktop/LICENSE-GPL-3.0`](desktop/LICENSE-GPL-3.0). The separate pointcloud core remains LGPL-3.0-or-later.

The [OpenAEC style book](https://github.com/OpenAEC-Foundation/OpenAEC-style-book) was cloned beside this repository (commit `dfdcd41`). The native ribbon uses the old application's compact button grouping, OpenCADStudio's Rust three-row ribbon primitives and quick-access pattern, and OpenAEC's Deep Forge, Night Build, Scaffold Gray, Construction Amber and Warm Gold tokens. Its top strip holds the File button and, at the right end, the quick-access actions Import, Open scan folder, Export, Undo and Redo, with muted disabled actions and tooltips. Group captions and active/hover states follow the style-book ribbon tokens; in a narrow window the tool groups scroll horizontally with visible left/right controls instead of being clipped. All tools sit on one ribbon without tabs, in the groups View, Display, Section box, Selection, Measure, Views, Edit, Surface and Index, and every action appears there once. The 3D BAG panel is not offered in the ribbon, while `--bag3d` remains available. The cancel actions for a running selection scan, scale, mesh or index build take the place of their start buttons or appear only during the job. The File view has native Deep Forge, Blueprint Light, Night Build, Blueprint Blue and High Contrast choices. The selection persists in `open-pointcloud-studio-native/theme` under the XDG configuration directory (or `~/.config`; on Windows the roaming application data folder unless `XDG_CONFIG_HOME` is set, with disk indexes under the local application data folder); the CAD viewport stays dark across themes. The old web ribbon's CSS and TypeScript components are not used in the native build. Inter and Space Grotesk are bundled as OFL-licensed native font assets. Visual checks are saved in [`../screenshots/`](../screenshots/).
The ribbon's active text, group-label opacity, separators, hover and File-tab colors now match the style book's `themes.css` tokens per palette. A new configuration starts in Blueprint Light, the style book's default; saved theme choices remain in effect. Compare the native [light](../screenshots/native-openaec-ribbon-tokens-light.png) and [Deep Forge](../screenshots/native-openaec-ribbon-tokens-forge.png) screenshots.
The ribbon's RGB, Elevation, Intensity and Classification controls now
use native Iced line drawings at OpenCADStudio's small-tool icon scale. Inactive
glyphs stay neutral; the selected color mode gets the OpenAEC accent. Compare
the Deep Forge and
Blueprint Light views.

On X11, the Rust desktop sets the system title bar's light/dark theme hint to
match the chosen native palette while retaining normal window-manager drag,
resize and controls. The local command API's `set_theme` command changes the
same palette and persists it, making both theme and window chrome testable in
the running development build.
The verified [light theme](../screenshots/native-blueprint-light-native-titlebar-114m.png)
and [dark theme with exact selection](../screenshots/native-theme-api-forge-114m-exact-selection.png)
screenshots show the 114,174,907-point merged AHN6 cloud in that build.

At the default 1440-pixel window width the whole ribbon is visible without
scrolling. From left to right it holds Zoom all, Fit stations and the seven
camera directions; the four colour modes, eye-dome, station markers, point
size and point budget; the section box; the selection tools; Distance and
Area; one Edit group with a Move row, a Scale row and a Thin row; the two
meshers; and the index controls. Small tools stand in OpenCADStudio's three
rows, and an active toggle carries the accent colour. At narrower widths the
same controls remain available through the horizontal scrollbar or the arrow
buttons. The ribbon measures the actual scrollable content, so the arrows
appear only when groups are cut off, the right arrow disables at the end, and
resizing back to a wide window removes them again.

Commands that are used rarely or need several parameters are offered once
outside the ribbon. The File view has the export variants (full resolution,
selected points, without selected points, section box, every Nth point,
surface mesh and the saved views as BCF), merging of visible LAS/LAZ scans
and the export format.
Properties repeats nothing the ribbon offers: besides what a scan is
and holds, it has the saved views with their name field and the annotations
of the active view, the strength of eye-dome lighting
while that is on, the limits of the section box with Zoom box while the box
is on, the 3D surface settings, the list of stations and the cancel buttons
of mesh, merge and scale jobs.

The ribbon screenshots under `../screenshots/` were taken with the earlier
ribbon, which spread these tools over Home, View, Select and Tools tabs: the
[Classic tool order](../screenshots/classic-v0.3-tools.jpg) beside the
[former Tools tab](../screenshots/native-tools-ribbon-classic-order-114m.png),
[its compact layout at 1440 pixels](../screenshots/native-tools-ribbon-compact-1440.png),
the former [Home](../screenshots/native-ribbon-overflow-home-wide.png),
[View](../screenshots/native-ribbon-overflow-view-wide.png) and
[Tools](../screenshots/native-ribbon-overflow-tools-wide.png) tabs without
navigation arrows, and the [arrows at 900 pixels](../screenshots/native-ribbon-overflow-tools-narrow.png)
with the [right arrow disabled at the end](../screenshots/native-ribbon-overflow-tools-scrolled.png).

The amber File button opens a native backstage view with the currently open scans, direct scan activation, import, the export variants (full resolution, selected points, without selected points, section box, every Nth point, surface mesh and saved views as BCF), merging of visible LAS/LAZ scans, format choice and appearance choice. The File view covers the tool ribbon and model space, while keeping quick access and the status bar visible; Escape or Return to model closes it. Unavailable exports appear muted. This uses the existing Rust import/export commands and no web components.

The top strip has the File button, the Home tab of the ribbon beside it and, at its right end after the quick-access actions, a Settings button. Settings follows the dialog of the OpenAEC style book: General has the language (Auto-detect, English, Nederlands), Appearance the theme with its swatches, and About the version, framework and licence. A choice shows at once; Cancel or Escape puts back what was in use and Save keeps it, the language in a `language` file beside the theme. Texts are looked up by their English wording in `assets/locales/nl.json`; a text without an entry stays English, which still holds for status messages. The theme is no longer chosen in the File view.

Native display and indexing defaults now persist in `settings.json` under the same configuration directory as the theme: color mode, point size, eye-dome switch and strength, scanner-marker visibility, point budget and auto-index. New configurations start at a 250,000-point viewport budget; the ribbon can raise it to ten million, matching Classic. Changes from the ribbon or local command API are saved after a short debounce; invalid stored numeric values fall back to safe defaults. Source scans and their per-file edits are unaffected.

OpenCADStudio's SVG icons under `assets/icons/` were copied into [`assets/opencad-icons/`](assets/opencad-icons/) and are embedded by Rust `iced::widget::svg`. No HTML, CSS, JavaScript or webview is used in the native desktop crate.

Views can be saved with the camera, the section box, the colour mode and annotations, restored, and exported as BCF; see [Saved views, annotations and BCF](#saved-views-annotations-and-bcf). They persist per source scan in `camera-views.json` under the native XDG configuration directory.
E57 scan transforms, valid PCD `VIEWPOINT` headers and PTX scanner positions appear as station markers in the native 3D view. A PCD header with an all-zero orientation, found in public PCL samples, opens with identity orientation but has no invented scanner axes. For isolated stations, the small X/Y/Z axes show the registered scanner orientation; nearby stations grouped into one marker do not imply a shared orientation. The ribbon's **Fit stations** frames stations together with the cloud; Properties lists each station's coordinates and axis directions. Click a marker or a station's **Center** button to pan the current view to that position without changing its angle or zoom. Use `--scans INPUT` to print positions and orientations.

The project panel lists the open clouds in name order, one compact row each
with a visibility switch, the point count and a remove button; a second line
appears only while a cloud is loading or indexing, or when it has selected or
deleted points. Click a row to select it, Shift-click to select every row up
to it and Ctrl-click to add or drop one; the visibility switch and the remove
button of a selected row then act on the whole selection, so a set of scans
can be hidden, shown or closed at once. The top strip starts with the application logo and the File
button, with the quick-access actions at its right end. The same logo is the
window icon and, on Windows, the icon of the executable.

### Opening a whole scan project

A scan project is usually a folder with one scan file per station, often with
a scan project file (`.rcp`) that lists them. Either opens in one step:

- **Open scan folder…** in the File view or the project panel, or a folder on
  the command line, opens every supported point-cloud or mesh file directly
  inside that folder. Sub-folders are not searched. The files are ordered by
  name with numbers compared as numbers, so `scan 2` comes before `scan 10`.
- A scan project file can be picked in the Import dialog or given on the
  command line. It is a ZIP container holding an XML document, and only that
  document is read. Every listed scan is looked up beside the project file,
  first by its stored relative path and then as `<scan name>.<extension>`,
  ignoring letter case. The absolute paths stored in a project belong to the
  machine that wrote it and are never used.
- Files, folders and project files can be dropped on the window, and the
  [command API](API.md) `open` command accepts all three.

The status line reports how many scans are being opened and how many listed
scans were not found. A scan that is already open or still loading is
skipped, so a folder and its project file together add every scan once.
Folders and project files are read on a worker thread, which keeps the window
responsive on a network share. Use `--list-scans PATH [PATH ...]` to print the
files that would be opened, one per line, without starting the GUI.

### Station photos and walking

E57 scans often carry the photos taken at each station as pinhole images, for
example six 90-degree cube faces. They are listed from the file metadata when
a scan opens; nothing is decoded until it is shown. Every station with photos
is drawn as a ball that shows its surroundings, and stays visible through
walls and roofs like the station markers. Click a ball, or **Photo** in the
station list, to stand in that station: drag to look around, scroll to zoom,
and click another station to step over to it. The photos are looked up per
pixel in the image that sees each direction, so any set of pinhole photos with
a pose works, not only complete cubes. Spherical and cylindrical photos are not
shown yet.

`W`, `A`, `S` and `D` walk through the scene, `Q` and `E` move down and up, and
Shift walks faster. Forward and back follow the viewing direction, so looking
down a stairwell and pressing `W` goes down it; sideways stays level. While
walking, points are drawn thicker and keep a size in the scene, so that
surfaces close by fill in. Walking starts from the current orbit view, or from inside
a station: walking out of a station leaves its photo and continues through the
point cloud with the station behind you; walking into another ball enters its
photo. Escape, **Back to 3D view**, Zoom all or a camera preset return to the
orbit view. Selection tools are not available while walking.

Use `--photos INPUT OUTPUT_DIRECTORY` to save the stored photos of every
station and print where each one looks.

E57 files are read through a buffered window, because the format is decoded
in 1 KiB pages and a seek per page is very slow on a network share. One
10-million-point scan on a share that takes about a minute page by page now
decodes in a few seconds.

A merged cloud of several gigabytes still takes minutes to read in full, and
a network share sets the pace. From 512 MiB such a file first shows a preview
of two million points taken from data packets spread evenly through the file,
read on several threads while everything in between stays unread; an 8.7 GB
cloud of 455 million points on a share that reads 100 MB/s shows up in about
ten seconds. Sampling stops after six seconds on a source that seeks slowly
and shows the packets read by then, which are spread through the file as
well. The sampled records are decoded by the same reader as a full pass.
This needs record fields that all fill whole bytes and
packets that all hold the same number of records, which merged clouds
usually have; station scans with a packed row and column index open as
before. Only the packets that are read can be checked, so the preview is
provisional: it cannot be indexed, and it is closed again when the full pass
fails or is cancelled. The only scan of a file, when it has neither a scanner
sweep nor a name, is treated as a merged cloud: its pose places the points
but shows no station marker.
Without stations, a scene whose bounds are stretched by a few stray far
points is framed around the bulk of its points.

A source of 512 MiB or more that is read in full (PLY, E57, PCD, PTX and the
text formats; LAS and LAZ open from their header instead) is shown while it
is being read: every few seconds the scene gets the points known so far, the
spread preview together with an even sample of up to two million points of
what the pass has read. The scene can be turned, sectioned and measured in
the meantime, and the camera stays where it was put. These clouds are
provisional; the checked cloud takes their place when the pass ends, before
its octree is built, and keeps that sample so the scene does not thin out.
At most two sources are shown this way at once, which bounds the memory
snapshots need; further ones appear when they have been read.

### Progress while opening

A strip above the scene has a line for every import that reads its source
and for the octree being built: the name of the scan, the step it is in, the
points done of the total, a bar with the percentage, the time left at the
pace so far and a button to cancel. An import that also builds an octree
reports two steps, reading and building. Scans opened together share a line
that says how many are done, with one button that cancels the rest.
The percentage needs a known total: an E57 file states its record count, and
a tree build knows the points of its cloud; other formats show the points
read so far without a bar. Each row of the project list shows the percentage
of its own scan with a thin bar underneath.

In a distant overview, stations projected within 32 pixels share a count marker;
labels move around neighboring markers and use a dark badge for contrast over
light point data. Zooming in separates the stations again, while Properties
always retains the individual poses.

### Measuring distances and areas

The ribbon's Measure group has **Distance** and **Area** next to the selection tools.
In either mode a left click picks the exact source point under the pointer
from the active layer, with the same search and eight-pixel reach as the
point-pick tool. A drag still orbits and pan and zoom work as usual, so the
view can be turned between two points. Backspace removes the last point,
Enter finishes the measurement, and Escape stops measuring and drops an
unfinished measurement. In Area mode a click on the first point also finishes.

Distance measures a polyline: each segment shows its length, and the total 3D
length, the horizontal length as seen from above and the height difference
between the first and the last point are reported. Area measures the closed
polygon through the points: its true area, so a sloped roof or a vertical wall
is measured in its own plane, the plan area as seen from above, and the
perimeter. The area is the length of the polygon's vector area; for points
that are not in one plane that is the largest area the polygon shows from any
direction.

The measurement is drawn over the points with a label on every segment and one
for the total or the area (a single segment carries one label), and it is
listed under **Measure** in Properties with a Clear button. Values are in scene units with three decimals. A
measurement keeps its scene coordinates and stays visible while walking; new
points are picked in the orbit view. A finished measurement remains until
**Clear** in the Measure group or the first point of the next one. Switching to a
selection tool keeps a measurement that has enough points and drops one that
has not. One measurement holds at most 256 points and is not saved with the
scan.

### Saved views, annotations and BCF

A view holds everything needed to come back to it: the camera (the orbit
camera, or the walking camera when the view was saved while walking), the
section box with its limits in model coordinates and whether it was on, the
colour mode, the time it was saved, an identifier and its annotations.
**Save view** in the ribbon's Views group, or Save beside the name field
under **Views** in Properties, saves what the viewport shows; without a name
the view becomes "View 1", "View 2", …. Properties lists the views of the
active scan: a click on a name restores the view, **Rename** changes its
name, **Update** overwrites it with the current view and × deletes it. A scan
has at most 32 views with names that are unique within it. Views are stored
per source scan in `camera-views.json` in the configuration directory; files
written before views held more than the orbit camera still load. Every view
in the file is read on its own, so a view that this version cannot read does
not take the others with it, and a file that cannot be read at all is copied
to `camera-views.unreadable.json` before the next save replaces it.

Restoring puts the camera, the section box and the colour mode back. The
orbit camera is relative to the bounds of the scene and to the viewport, so a
view also keeps those: in a viewport of another size the pan scales with the
picture, and when other scans have been opened or closed since, the camera is
moved to show what it showed.

The view last saved or restored is the active view, shown highlighted in the
list. Its annotations are drawn over the scene and stay on their points while
the camera moves, in the orbit view and while walking; **Hide** stops showing
them. **Note** and **Line** in the Views group place annotations by picking
exact points, with the same search and eight-pixel reach as the measuring
tool; a drag still orbits. A note is a picked point with a text: after the
click a field over the viewport takes the text, and Enter or Add places a
marker with a label and a leader. A line is two picked points, drawn as an
arrow from the first to the second. Escape cancels a half-placed annotation
and, pressed again, leaves the tool. An annotation placed while no view is
active first saves the current view. The annotations of the active view are
listed in Properties, each with × to delete it; a view holds at most 64, and
a note at most 240 characters. The label of a note stays inside the viewport
and moves away from its point past the markers and the labels of other notes
close by. The annotation tools, the measuring tools and the selection tools
exclude each other. A half-placed annotation is dropped when another scan
becomes the active one, and the tool is left when the last scan is closed.

Each view has a snapshot: a PNG image of the viewport alone, with the points,
the section box and the annotations, cut from a screenshot of the window. It
is taken shortly after a view is saved or updated and again when its
annotations change, once the change has been drawn and the points have
refined. A snapshot is only ever taken while the viewport shows the view: the
view is the active one, and the camera, the section box, the colour mode and
the bounds of the scene are what they were when the view was last saved,
updated or restored. The camera of a view does not follow the viewport, so
after turning or zooming to reach a point, placing or removing an annotation
changes the view and leaves its snapshot as it is; the status line says that
the snapshot is renewed when the view is restored, and restoring the view
takes it. The same goes for a snapshot that could not be taken, because the
camera moved before it was or because capturing failed, and for a view from a
list written before snapshots existed: a view whose snapshot is missing or
older than the view has `snapshot_due` in the list, and restoring it takes
the snapshot. When only the size of the viewport has changed, with the window
or with a status text of more lines, the pan first follows the picture as it
does on restoring, and the snapshot is taken of that. A snapshot taken in a
viewport of another size, or in a scene with other bounds, than the view was
saved with stores the view relative to that viewport and scene, so that the
picture and the camera of the view keep belonging together. Snapshots are
stored as `view-snapshots/<identifier>.png` beside `camera-views.json`, at
most 1920 pixels along their longest edge, and are removed with their view.
When a snapshot cannot be taken the view is saved all the same.

**Export BCF** in the Views group, or **Views as BCF…** among the exports of
the File view, writes all views of the active scan as one BCF 2.1 file
(`.bcf`, the BIM Collaboration Format of buildingSMART). The file is a ZIP
container with `bcf.version` and one folder per view, named by the view's
identifier:

- `markup.bcf`: a topic with that identifier, the name of the view as its
  title, its creation date and the account name of the user as author, the
  file name of the scan in its header, and one comment per note, each
  referring to the viewpoint.
- `viewpoint.bcfv`: a perspective camera, the six clipping planes of the
  section box when it was on (each on a face, pointing at the side that is
  cut away), and lines: each line annotation, and a short upright line of
  0.25 m at the point of each note.
- `snapshot.png`, when the view has a snapshot.

The status line reports how many views were written, how many of them with a
snapshot, and how many changed after their snapshot was taken and wait to be
restored. A view from a list written before times were kept gets the time of
its first export as its creation date and keeps it.

Coordinates are model coordinates in metres. The camera reproduces the view:
it stands where the application's camera stands, with the true vertical field
of view. The walking camera maps directly. The orbit camera stands 1.8 scene
extents from the scene centre; panning shifts its picture instead of turning
it, which a BCF camera cannot express, so the exported camera is turned in
place towards what is in the middle of the viewport. Without pan the two
pictures are the same. With pan they agree in the middle and drift apart
towards the edges, by about the distance from the middle squared times the
pan, divided by the focal length squared, where the focal length is 1.25
times the shorter side of the viewport divided by the zoom. In a viewport of
800 by 600 pixels at zoom 1, a pan of 50 pixels gives 0.7 pixels at 100
pixels from the middle, 3 at 200 and 14 at the left and right edges; a pan
of 200 pixels gives 7 at 200 pixels from the middle and 46 to 59 at the
edges. A view that must match its snapshot to the edge is saved without pan.
The BCF 2.1 schema limits `FieldOfView` to 45–60
degrees and announces that readers should expect values outside that range;
the file states the true angle, which lies outside it for most views (the
orbit camera at zoom 1 has about 44 degrees and narrows as it zooms in).
The ZIP container is written by the application itself, with deflated XML and
stored images.

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
The disk octree is built on several threads. A node is read in blocks of
65,536 of its fixed 40-byte records, which are assigned to the eight children
in parallel and written in block order, and subtrees are built side by side
on a bounded pool; large nodes are split one at a time so that the disk does
not hold every level at once. The files are identical to those of a
single-threaded build, which the tests keep as a reference, with exact source
ordinals and cancellation between blocks. On a synthetic cloud of 256 million
points the build went from 336 to about 50 seconds.
Completed PLY, E57, PCD, PTX, XYZ, ASC, TXT, CSV and PTS indexes also keep
exact source bounds, count and attribute flags in an atomic cache manifest. Reopening an
unchanged indexed file in these formats reads that manifest and a small octree
preview instead of scanning the source file again. E57 scanner positions come
from the file's metadata, while PCD `VIEWPOINT` comes from its short text
header; neither requires decoding point records. PTX scanner positions and axes
are stored in the bounded cache manifest because each scan block has its own
pose. Older PTX manifests are refreshed after one full source read. A missing,
stale or damaged manifest falls back to the full reader.
For a cold explicit `--index` run, non-LAS formats now collect the bounded
preview and exact metadata while writing the octree root in one source pass.
With automatic indexing enabled, the native GUI uses the same one-pass route
for supported non-LAS sources of at least 64 MiB. It displays the checked
preview after that source pass while the disk tree is still building; smaller
imports retain the existing background auto-index path.
Both meshers keep their full-source loops inside the optimized Rust core in
development builds; the native desktop supplies edit and progress callbacks
without recompiling those loops at the desktop's lower optimization level.
The 3D surface mesher searches nearest neighbors and estimates local normals
on multiple Rust threads in bounded batches. Progress and cancellation remain
available between batches, and the resulting OBJ keeps deterministic vertex
and face order.
XYZ, PTS, CSV and both ASCII and binary PLY exports encode bounded
65,536-point batches on multiple Rust threads and write the finished chunks
in source order. The
temporary output is published only after the full source and any selected
point count have been verified.
On the local eight-core development machine, exporting the 114,174,907-point
AHN6 LAZ to binary PLY produced a 3,425,247,452-byte file in 59.23 seconds,
with 130,924 KiB peak process memory. The PLY header count and calculated
record length matched the final file size; first, middle and last records
decoded within the source bounds.
Before the binary PLY reader was batched, reopening and scanning the full
3.2 GiB file took 56.61 seconds with 18,568 KiB peak process memory. The
bounded parallel reader reduced that full scan to 18.13 seconds with
32,836 KiB peak memory on the same machine. A separate full-source visit
counted all 114,174,907 records in order.
With the completed index present, the same `--index` command reopened that
3.2 GiB PLY in 0.15 seconds with 18,712 KiB peak process memory, including
index validation; the first run after adding its cache manifest took 18.50
seconds to read the source and backfill metadata.

## Windows installer

Releases carry a Windows installer beside the archives: the file ending in
`windows-setup.exe`. It installs for the current user without elevation, or
for all users when chosen in its first dialog, adds a Start menu entry and
optionally a desktop icon, lists the application under "Open with" for
E57, LAS, LAZ, PLY, PCD, PTX, PTS and XYZ files without changing what opens
them by default, and comes with an uninstaller. Its dialogs are in English
or Dutch. The application needs nothing besides Windows 10 or 11 itself.

The release workflow builds it from [`installer/windows.iss`](installer/windows.iss)
with the same folder it packs into the archive, and signs it when signing
is configured. To build it by hand, put the executable and the licence
texts in a folder and compile the script with the version and that folder:
`ISCC /DAppVersion=0.6.0 /DSourceDir=path\to\folder installer\windows.iss`.

## Build

```bash
cd native
cargo run -p open-pointcloud-studio-native
cargo run -p open-pointcloud-studio-native -- /path/to/scan.laz
cargo run -p open-pointcloud-studio-native -- /path/to/scan-folder
cargo run -p open-pointcloud-studio-native -- /path/to/project.rcp
cargo run -p open-pointcloud-studio-native -- --list-scans /path/to/scan-folder /path/to/project.rcp
cargo run -p open-pointcloud-studio-native -- --export /path/to/scan.laz /path/to/scan.ply
cargo run -p open-pointcloud-studio-native -- --export /path/to/scan.las /path/to/scan.e57
cargo run -p open-pointcloud-studio-native -- --section /path/to/scan.laz 207440,474000,-100,208000,475000,1000 /path/to/crop.laz
cargo run -p open-pointcloud-studio-native -- --mesh /path/to/scan.laz /path/to/terrain.obj
cargo run -p open-pointcloud-studio-native -- --mesh-export /path/to/surface.off /path/to/surface.obj
cargo run -p open-pointcloud-studio-native -- --surface /path/to/scan.e57 /path/to/surface.obj
cargo run -p open-pointcloud-studio-native -- --surface /path/to/scan.e57 /path/to/surface.obj --max-vertices 100000 --neighbors 12 --edge-factor 4
cargo run -p open-pointcloud-studio-native -- --scans /path/to/scan.e57
cargo run -p open-pointcloud-studio-native -- --photos /path/to/scan.e57 /path/to/photo-directory
cargo run -p open-pointcloud-studio-native -- --bag3d 91000,398000,92000,399000 2.2 /path/to/buildings.obj
cargo run -p open-pointcloud-studio-native -- --mcp
cargo test --workspace
```

See [TEST_DATA.md](TEST_DATA.md) for large public datasets and repeatable
45.8-million-point and 129.4-million-point AHN6 stress tests.
The running Rust GUI also exposes a token-protected, local
[native command API](API.md) for status, layer and camera control, section
boxes, exact world-coordinate point selection, deletion/undo/redo, and full
or clipped exports. Its old JavaScript `/eval` bridge is not used.
`--mcp` runs a [Model Context Protocol server](MCP.md) on standard input and
output whose tools drive a running window, or a window it starts, through
that API, including screenshots of the 3D view.

The current native slice opens LAS, LAZ, PLY (ASCII and little-endian binary), PCD (ASCII, interleaved binary and disk-backed LZF binary-compressed), PTX, OBJ, OFF, STL, DXF, E57, XYZ, ASC, TXT, CSV and PTS files. LAS/LAZ metadata opens immediately from the header and a bounded preview samples spaced ranges without decoding every point. Uncached non-LAS/LAZ formats stream the entire source to calculate bounds and counts. At most 100,000 preview points are retained per file. The viewport draws lit sphere impostors with WGPU; each sphere writes its curved front surface to the depth buffer, and exact point picking uses the displayed sphere radius and front depth. Survey coordinates are rebased in double precision before GPU upload. Mesh faces use interpolated per-vertex normals for directional lighting; meshes without normals derive them from their triangles, and non-uniform or reflected live scales map them into world space. The viewport retains CPU geometry and uploaded GPU buffers across camera-only redraws; changed visibility, LOD points, colors, filters, edits or section bounds rebuild them. A screen-space eye-dome pass shades points and mesh faces by neighboring depth; it can be toggled in the View ribbon. The viewer supports multiple files, visibility, orbit with left drag or Shift + middle drag, pan with middle or right drag, deep zoom around the cursor, and RGB/elevation/intensity/classification colors. A native 3D view cube follows the camera; click its six faces, visible corners or ISO button to snap the view while retaining the current zoom and pan. Right click without dragging opens a viewport menu. Escape closes the menu, exits box/pick selection and clears the selection. The section box in the View ribbon or right-click menu has six draggable 3D face handles, percentage sliders and precise XYZ coordinate fields in Properties. Apply XYZ limits to clip point and mesh rendering; the world-space limits remain fixed when another layer is shown or hidden. Full-resolution selection honors the same limits. **Export section** streams the source once, including points outside the preview sample, and patches the exact count into PLY/PTS headers or closes the LAS/LAZ/E57 writer with its final count before saving. Box selection visits exact source points in intersecting octree leaves when an index exists and otherwise streams the full source; compact bitsets retain original point ordinals. Select > Delete hides points immediately without changing the source. Undo and Redo restore or reapply up to eight deletion batches. Full and section export honor these edits, and decimation, translation, scale and both meshers also use the remaining source points. XYZ, PTS, CSV, PLY, LAS, LAZ and E57 export re-reads the source stream, so output does not lose points to preview sampling.

As in Classic, `F` fits the whole model while focus is outside text inputs.
Right-drag panning now applies the whole gesture when it crosses the
right-click threshold, and orbit, pan and section-handle drags continue while
the cursor crosses the model-space boundary.
Opening a scan that is already open or still loading is skipped. Preview and
octree jobs, meshing, deletion and Undo/Redo match the specific layer
instance instead of the shared file path and timestamp.
The point-pick tool searches the full source or disk octree for the nearest source point within eight screen pixels. Escape exits the tool while keeping an existing selection; during a search it now stops the indexed or streamed scan and discards the unfinished result. The [114-million-point pick screenshot](../screenshots/native-pick-exact-114m.png) shows one exact selected point after Escape.
Opening, hiding or removing a layer also cancels an in-flight full-resolution
selection immediately, instead of continuing to scan a source whose result
will be discarded.

The ribbon's Surface group includes two meshers. Terrain mesh streams every source point through a bounded XY grid, keeps the lowest point in each cell, triangulates a 2.5D TIN in Rust and rejects long edges across gaps. Its default cap is 100,000 vertices; use `--mesh INPUT OUTPUT.obj` headlessly. The 3D surface command streams the complete source into a bounded reservoir of up to 200,000 candidates by default, spatially thins them to 50,000 vertices, estimates local normals and triangulates tangent-plane neighborhoods. It propagates face orientation across connected patches, fills short planar inner loops and well-aligned triangular gaps, then recalculates normals from the final triangles. This can reconstruct vertical walls and overhangs, but sparse sampling leaves many holes and independent neighborhoods can still create contradictory face cycles; the result is not guaranteed watertight. While a scan is active, Properties shows Max vertices (3–1,000,000), Neighbors (3–32) and a positive Edge factor; invalid values are rejected before opening the save dialog. Use `--surface INPUT OUTPUT.obj` headlessly with the equivalent `--max-vertices`, `--neighbors` and `--edge-factor` options. Both atomically save Wavefront OBJ with source RGB where available and per-vertex normals, then show the result as GPU-rendered triangles. OBJ, ASCII or little-endian binary PLY, OFF, ASCII or binary STL, and DXF 3DFACE files can also display native GPU faces. Imported OBJ and PLY vertex colors render on the faces; OBJ also reads local `mtllib` sidecars and applies their `Kd` diffuse colors per face, splitting shared vertices at material boundaries. Explicit OBJ vertex colors take precedence, and OBJ export writes the resolved colors into its vertices. Texture maps are not rendered. Aligned OBJ and PLY vertex normals survive OBJ export. Each imported mesh belongs to its source entry, so several meshes can render together. Points and surfaces have separate visibility controls in the project panel. Resident meshes are bounded to one million distinct vertices and two million triangles per file.

The desktop crate also contains a native 3D BAG panel, which the ribbon does not offer. Enter a bounding box in RD New coordinates or copy it from the active scan/section box, choose LoD 1.2, 1.3 or 2.2, and save a georeferenced OBJ. The Rust client follows API pagination, applies each page's CityJSON transform, triangulates polygon holes, and loads the result as a separate surface layer. The panel now includes a fully native RD New map using [Kadaster BRT-A raster tiles via PDOK](https://www.pdok.nl/ogc-webservices/-/article/basisregistratie-topografie-achtergrondkaarten-brt-a-): draw a rectangle, pan, zoom or fit typed RD bounds, then download that exact area. The map displays [Kadaster/PDOK CC BY 4.0 attribution](https://www.pdok.nl/copyright/). Exported OBJ files preserve the [3DBAG CC BY 4.0 attribution](https://docs.3dbag.nl/nl/copyright/), and the viewer displays the required credit and license link while the buildings are visible.

File open and save dialogs use `rfd::AsyncFileDialog`, so the native UI stays responsive while the operating-system dialog is open. The running development build now displays one 1.208 GB merged AHN6 LAZ containing 114,174,907 points, with its full disk index attached and an exact 373,382-point selection highlighted. Three separate AHN6 tiles totaling 1.28 GiB and 129,398,587 points were also checked earlier.
Full-stream imports such as E57 report their decoded-point count during loading,
and the empty viewport shows import progress instead of an open-file prompt.
The status bar and local API can cancel an individual full-stream import without
adding a partial layer. A
public 1.15 GB E57 with 46,589,344 points and nine scanner positions was
opened, indexed, navigated and point-picked in the native dev build; see the
[test record](TEST_DATA.md).
A [screenshot with the earlier Select tab](../screenshots/native-select-ribbon-zoom-selection-114m.png)
shows the selected area framed at 19.4× after the Zoom selection action.

| Workflow in the existing app | Native status |
| --- | --- |
| LAS/LAZ, PLY, XYZ/ASC/TXT/CSV, PTS import | Implemented for point data; bounded ASCII and little-endian binary PLY polygon meshes also render |
| PCD, PTX, OBJ, OFF, STL, DXF, E57 import | Point vertices implemented, including PCD LZF compression and VIEWPOINT transforms in all three PCD storage modes; official PCL XYZ, RGB, label/RGBA, intensity/extra-field, padding and organized captures were validated. OBJ, OFF, STL and DXF 3DFACE geometry also renders as triangles |
| Multiple clouds, visibility, orbit, pan, deep zoom, 3D view cube, right-click menu, rounded points, colors, point size, budget, classes | Native implementation; close-up point spheres grow gently on screen so their lighting stays visible, with a capped increase over the chosen point size. Saved views keep the camera, the section box, the colour mode and note and line annotations per source scan, each with a snapshot, and export as BCF 2.1. The project panel lists the classification codes that occur in the open clouds; each can be shown or hidden like a layer, which filters both rendering and exact selection. Advanced navigation polish remains |
| Section box | Three-axis clipping with visible wireframe, six draggable face handles, six limit sliders and precise XYZ fields. Fit box to selection uses exact selected source points, including points outside the preview; Zoom box frames the clipped volume in the viewport. Clipping applies to GPU rendering, full-resolution selection and a separate clipped export |
| Octree LOD and eye-dome lighting | Existing disk-backed octrees attach when a scan opens. Uncached scans with at least one million points are indexed automatically, one at a time, after their preview loads; the ribbon's Index group can disable this or start a manual build. Source-read and tree-build progress appear in the strip above the scene and in the status bar, and a running build can be cancelled without retaining a partial cache. Camera movement selects visible nodes by projected size and refreshes a bounded point sample while retaining the previous sample until its replacement is ready; stale requests cancel during node reads and leaf-preview generation. The point-budget control reaches 10 million, with point uploads split into bounded WGPU buffers. Compact per-leaf LOD previews make repeated cold-cache navigation cheaper; old indexes create these previews on first use without a full rebuild. Native screen-space eye-dome shading has an on/off switch and an adjustable 0–5 strength in View and Properties |
| Full-resolution point selection | Index-guided exact box selection when available, full-source fallback, single-point picking that prefers the displayed LOD and falls back to the exact full source with or without an index, selected point properties and selected export. The local API also picks by viewport pixel and returns the source ordinal and attributes. The ribbon's Zoom selection action frames the exact selected bounds without changing the section box; selection masks retain source-coordinate bounds so a later live transform and a gigabyte scan do not require a second full-source read. Long box scans can be cancelled from the ribbon, Escape or local API without applying partial results |
| Measuring | Distance along a polyline and area of a polygon between exact picked source points, with segment lengths, horizontal length, height difference, true and plan area and perimeter in the viewport, Properties and the local API |
| Editing | Native Delete/Undo/Redo on original source ordinals across multiple clouds; exact-percentage Thin edits the open view and can be undone without copying the source. Translate and independent XYZ Scale now edit the open view through a lazy affine transform. The 3D view, disk-octree selection, section box, scan markers, exports and mesh display use the transformed coordinates. Scale uses the exact centroid of all remaining points as pivot, streaming large scans from the octree with progress and cancellation. Reset Transform restores source coordinates and reopens the full section if the old box no longer intersects the cloud. Export saves the edited coordinates; the source file stays unchanged. Exports of the selected points, or of the cloud without them, remain file-based |
| Surface reconstruction | Full-source 2.5D terrain TIN and bounded 3D local surface reconstruction to OBJ with native GPU face display. Both modes honor deleted points, the active section box and visible classifications; watertight and adaptive reconstruction remain |
| PLY, LAS, LAZ, E57, XYZ, PTS, CSV export | Full same-format LAS/LAZ/E57 export copies the original file byte-for-byte; full LAS↔LAZ conversion streams native LAS records with their metadata and point attributes. Filtered or transformed LAS/LAZ output preserves the original point format, GPS time, return data, 16-bit RGB, projection records and coordinate grid while applying edits to source records. Filtered E57 output retains each source scan, scanner pose, name, original record types and values, color/intensity limits, custom point fields and coordinate metadata. Translate and positive uniform Scale keep the scan structure when every source scan has a pose; Scale writes local Cartesian coordinates or spherical ranges as doubles while preserving other raw fields. New E57 output and transforms that cannot retain a valid scan pose stream XYZ, RGB8 and intensity as one world-coordinate scan; E57 has no standard classification field in this writer, and absent individual color/intensity values are written as zero. Non-LAS input uses the common XYZ, RGB8, intensity and classification model |
| Multi-scan LAS/LAZ merge | The native File view and local API combine visible LAS/LAZ layers in the background, applying deletion and transform edits while preserving original point attributes. The task reports progress, supports cancellation, and publishes the output atomically. Sources with incompatible point layout, coordinate grid or metadata are rejected. The `--merge OUTPUT.laz INPUT1.las INPUT2.laz [...]` command exercises the same streaming core without launching the GUI |
| OBJ mesh export | Terrain and 3D surface meshers save RGB and per-vertex normals in OBJ; any resident OBJ, PLY, OFF, STL or DXF triangle mesh can also be exported from the File view or Properties. Imported OBJ/PLY colors and aligned normals survive conversion. The writer saves atomically and keeps 3DBAG attribution where applicable |
| 3DBAG | Native RD map with PDOK raster tiles, rectangle drawing, pan/zoom, typed/scan/section-box bounds, LoD choice, paginated CityJSONFeatures import and GPU mesh display |
| Themes | Five native OpenAEC palettes, selected in the File view and persisted locally; model space remains dark |
| Settings and automation API | Theme, display/indexing defaults and saved views persist in native configuration files. A local token-protected Rust command API controls open layers, camera, visibility, section boxes, exact point selection, deletion/undo/redo, percentage thinning, exports and viewport screenshots; `--mcp` offers the same commands as Model Context Protocol tools; more commands and settings remain to port |

Mesh export writes all vertices and faces from the mesh currently held by the viewer, validates indices before touching the destination, and saves atomically. The File view and Properties panel expose it for imported OBJ, PLY, OFF, STL and DXF meshes. The `--mesh-export INPUT OUTPUT.obj` command supports batch conversion; 3DBAG output retains the required attribution header.

## Migration work remaining

1. Improve viewport LOD with predictive loading and smooth transitions between node levels. The native app already builds and reuses disk-backed indexes automatically for large scans, reads compact leaf previews for repeated camera movements, and cancels stale LOD reads during navigation. Releasing an orbit or pan drag starts the latest detail request immediately; an older in-flight request is cancelled and the delayed timer cannot launch a duplicate. Indexed clouds share the viewport budget by projected coverage; any unused allocation is returned to visible clouds. Concurrent sampling across indexed layers uses Rayon's bounded worker pool instead of starting one operating-system thread per layer. At 20× zoom or closer, a bounded exact-leaf scan keeps only points that project inside the viewport; scenes requiring more than two million candidate source points retain the regular preview LOD. For budgets above 500,000 points a first pass is sized from the read and build pace measured on this computer (at least 250,000 points) and kept within what the octree nodes in view give from their previews; it is shown only when it improves on the sample already on screen, and it is skipped when it would not or when those previews hold too little. The existing Rust octree and binary IPC code in `classic/src-tauri/src/pointcloud/` is a reference, but its all-points-in-memory build is unsuitable for large surveys.
2. Validate meshes from more real producer variants, materials and large models. The PCD reader has now been checked against official PCL XYZ, RGB, label/RGBA, intensity/extra-field, padding and organized captures; keep decoder work in `core`.
3. Improve the bounded 3D surface mesher toward watertight output and richer source attributes. Broaden the native settings UI as remaining workflows migrate.
4. Expand the documented native command API to remaining editing and selection actions, then retire the old frontend and Tauri packaging after feature parity checks.

The earlier application source remains under `classic/` for reference and is tagged `archive/classic-tauri-main`.
