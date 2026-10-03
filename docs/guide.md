# User guide

How each part of Open Pointcloud Studio works. The [README](../README.md) has the installation, a first walk through the application and the file formats; this guide goes into each tool.

The interface is in English or Dutch. The guide uses the English names.

## Contents

- [The window](#the-window)
- [Opening scans](#opening-scans)
- [Looking around](#looking-around)
- [Display](#display)
- [Scanner stations](#scanner-stations)
- [Station photos and walking](#station-photos-and-walking)
- [Section box](#section-box)
- [Selecting and editing](#selecting-and-editing)
- [Measuring distances and areas](#measuring-distances-and-areas)
- [Saved views, annotations and BCF](#saved-views-annotations-and-bcf)
- [Exporting and merging](#exporting-and-merging)
- [Meshing](#meshing)
- [3D BAG buildings](#3d-bag-buildings)
- [Index and level of detail](#index-and-level-of-detail)
- [Settings, language and extensions](#settings-language-and-extensions)
- [Where settings and indexes are stored](#where-settings-and-indexes-are-stored)
- [Keys and mouse](#keys-and-mouse)

<!-- A new tool gets a section of its own here and a line in the list above. -->

## The window

- The **top strip** starts with the application logo, the **File** button and the **Home** tab. At its right end are five quick-access buttons, shown as icons whose names appear when the pointer rests on them (Import point cloud, Open scan folder, Export active point cloud, Undo delete and Redo delete), and the **Settings** button. Actions that are not available are shown muted.
- The **ribbon** holds all tools on one row of groups: VIEW, DISPLAY, SECTION BOX, SELECTION, MEASURE, VIEWS, EDIT, SURFACE and INDEX. At the default window width of 1440 pixels the whole ribbon is visible. In a narrower window the groups scroll sideways, with the wheel, the scrollbar or the arrow buttons that appear at both ends.
- The **project panel** at the left lists the open clouds in name order, one row each with a visibility switch, the point count and a button to close the cloud. A second line appears only while a cloud is loading or indexing, or when it has selected or deleted points. Below the clouds, the classes that occur in them are listed.
- The **scene** in the middle is the 3D view, with the view cube in a corner.
- The **Properties panel** at the right shows what the active scan is and holds, and the settings that belong to what is in use: the saved views and the annotations of the active view, the current measurement, the selected point, the list of stations, the limits of the section box while it is on, the strength of eye-dome lighting while that is on, the 3D surface settings, and the progress of mesh, merge and scale jobs with their cancel buttons.
- The **status bar** at the bottom says what is going on, how many files and points are open and how many points are selected, and ends with the version.
- The **File view** opens with the File button and covers the ribbon and the scene. Its menu has the import entries, **3D BAG buildings…**, the exports and the merge, above the pages Workspace, Extensions and About and the entries **Settings…**, **Return to model** and **Exit**. The Workspace page lists the open scans (click one to make it the active scan, the same as a click on its row in the project panel) and has the export format and the "every Nth point" setting. Escape or Return to model closes the File view.

The title of the window is the file name of the active scan followed by the name and the version of the application, such as `Open Pointcloud Studio v0.8.0`.

In the project panel, click a row to make that scan the **active scan**; its row gets a coloured outline. The active scan is the one Properties describes, the one points are picked from (Pick point, Distance, Area and the annotation tools), the one Thin, Move, Scale and the meshers work on, and the one the exports and the saved views belong to. Shift-click marks every row from the active one up to the clicked one, and Ctrl-click (Command-click on macOS) adds or drops one row. The visibility switch and the close button of a marked row then act on all marked rows, so a set of scans can be hidden, shown or closed at once.

## Opening scans

A file opens in any of these ways:

- **Import point cloud…** in the File view, the Import point cloud icon in the top strip, or **+ Add point cloud** in the project panel.
- Dropping files, folders or scan project files on the window.
- Naming them on the command line: `open-pointcloud-studio scan.laz other.e57`.
- The `open` command of the [command API](../native/API.md).

Several files can be open at once. A scan that is already open or still loading is skipped.

### A whole scan project

A scan project is usually a folder with one scan file per station, often with a scan project file (`.rcp`) that lists them. Either opens in one step:

- **Open scan folder…** in the File view or the project panel, or a folder on the command line, opens every supported point-cloud or mesh file directly inside that folder. Sub-folders are not searched. The files are ordered by name with numbers compared as numbers, so `scan 2` comes before `scan 10`.
- A scan project file can be picked in the Import dialog or given on the command line. It is a ZIP container holding an XML document, and only that document is read. Every listed scan is looked up beside the project file, first by its stored relative path and then as `<scan name>.<extension>`, ignoring letter case. The absolute paths stored in a project belong to the computer that wrote it and are never used.

The status line reports how many scans are being opened and how many listed scans were not found, with the names of the first three, so it is clear which scans to put beside the project file. A folder and its project file together add every scan once.

A project file opens the scans it lists that exist beside it as E57, LAS, LAZ, PLY, PCD, PTX, PTS or text. The indexed scan copy (`.rcs`) that a project keeps of each scan, beside the project file or in its `<project name> Support` folder, is a closed format without a public specification and is not read. When a project has only those copies, the application says so instead of reporting missing scans: export the scans as E57, LAS or LAZ files named after the scans and put them beside the project file.

Folders and project files are read in the background, which keeps the window responsive on a network share. `open-pointcloud-studio --list-scans PATH [PATH ...]` prints the files that would be opened, one per line, without opening a window.

### What appears first

- **LAS and LAZ** open from their header at once, with a sample of points taken from places spread through the file.
- **Other formats** are read in full the first time, to learn the bounds and the number of points. While that runs, the scene shows the number of points read so far, and the import can be cancelled; a cancelled import adds nothing.
- A file that was indexed before reopens from its index without being read again; see [Index and level of detail](#index-and-level-of-detail).
- Up to 100,000 points of each file are kept in memory as its first picture. The detail beyond that comes from the index.

### Files of 512 MiB and more

A merged cloud of several gigabytes takes minutes to read in full, and a network share sets the pace.

An **E57 file** of that size first shows a preview of up to one million points taken from data packets spread evenly through the file. They are read on several threads while everything in between stays unread. Sampling stops after six seconds on a source that seeks slowly and shows the packets read by then, which are spread through the file as well. This needs record fields that all fill whole bytes and packets that all hold the same number of records, which merged clouds usually have. Station scans with a packed row and column index do not get this preview and show the points as they are read.

Any source of that size that is read in full (PLY, E57, PCD, PTX and the text formats) is shown while it is being read: every few seconds the scene gets the points known so far, which are the spread preview together with an even sample of up to two million points of what has been read. The scene can be turned, sectioned and measured in the meantime, and the camera stays where it was put. These clouds are provisional: they cannot be indexed, and they are closed again when the reading fails or is cancelled. When the reading ends the complete cloud takes their place, and keeps that sample so the scene does not thin out. At most two sources are shown this way at once; further ones appear when they have been read.

### Progress

A strip above the scene has a line for every import that reads its source and for every index being built: the name of the scan, the step it is in, the points done of the total, a bar with the percentage, the time left at the pace so far and a button to cancel. An import that also builds an index reports two steps, reading and building. Scans opened together share a line that says how many are done, with one button that cancels the rest.

The percentage needs a known total: an E57 file states its record count, and an index build knows the points of its cloud. Other formats show the points read so far without a bar. Each row of the project panel shows the percentage of its own scan with a thin bar underneath. The status bar has a **Cancel import** button while an import runs.

## Looking around

- Drag with the left button to orbit. Shift with the middle button orbits as well.
- Drag with the middle or the right button to pan.
- Turn the wheel to zoom at the pointer.
- `F` or **Zoom all** returns to the isometric overview of the whole model: it resets the direction, the zoom and the pan.
- The **VIEW** group has **Isometric**, **Top**, **Front**, **Right**, **Bottom**, **Back** and **Left**.
- The **view cube** follows the camera. Click a face, a visible corner or its ISO button to turn the view; the zoom and the pan stay as they are.
- A right click without dragging opens a menu with Orbit, Box select, Pick point, Section box, Zoom all and Clear selection.

When a scan arrives, the scene is framed again unless the camera has been moved since the application last framed it; the first scan is always framed. Scans with stations are framed around their stations; without stations, a scene whose bounds are stretched by a few stray far points is framed around the bulk of its points.

Drags continue when the pointer leaves the scene. Survey coordinates such as national grid coordinates are handled in double precision, so points far from the origin stay sharp.

## Display

The **DISPLAY** group sets how points are drawn:

- **RGB**, **Elevation**, **Intensity** and **Classification** choose the colours. A cloud without stored colours or intensity is drawn in a neutral grey in those modes.
- **Eye-dome** switches eye-dome lighting on or off: a shading by depth that makes edges and relief visible. While it is on, Properties has its **Strength**, from 0 to 5.
- **Stations** shows or hides the station markers.
- **Size** sets the point size from 0.1 to 20.
- **Budget** sets the most points drawn at once, from 100,000 to 10,000,000. The budget is a ceiling: a view that needs fewer points gets fewer.

Points are drawn as small lit spheres. Close by they grow a little on screen so that their shading stays visible.

Under **CLASSES** the project panel lists the classification codes that occur in the open clouds, with the standard LAS names. Each can be shown or hidden like a layer. A hidden class is also left out of selections and meshes.

The choices of the DISPLAY group and the Auto-index switch are kept in `settings.json` and are in effect again at the next start. Invalid stored values fall back to the defaults. Which classes are hidden is not kept: every class is shown again at the next start.

## Scanner stations

The scan poses of E57 scans, valid `VIEWPOINT` headers of PCD files and the scanner positions of PTX files appear as station markers in the scene. A PCD header with an all-zero orientation opens with a neutral orientation and gets no marker with invented axes.

The only scan of an E57 file, when it has neither a scanner sweep (a row and column index or spherical coordinates) nor a name, is treated as a merged cloud: its pose places the points but shows no station marker.

- For a station on its own, small X, Y and Z axes show the registered orientation of the scanner.
- In a distant overview, stations that lie within 32 pixels of each other on screen share one marker with their number. Labels move around neighbouring markers. Zooming in separates the stations again.
- **Fit stations** in the VIEW group frames the stations together with the cloud.
- Properties shows the number of stations and of station photos under **Scan positions**; **Show list** lists every station with its coordinates and axis directions. Click a marker, or **Center** beside a station in that list, to pan the view to that position without changing its angle or zoom.

`open-pointcloud-studio --scans INPUT` prints the positions and orientations.

## Station photos and walking

E57 scans often carry the photos taken at each station as pinhole images, for example six cube faces of 90 degrees. They are listed from the file metadata when a scan opens; nothing is decoded until it is shown. Every station with photos is drawn as a ball that shows its surroundings, and stays visible through walls and roofs like the station markers.

Click a ball, or **Photo** beside a station in the list under **Scan positions**, to stand in that station: drag to look around, scroll to zoom, and click another station to step over to it. The photos are looked up per pixel in the image that sees each direction, so any set of pinhole photos with a pose works, not only complete cubes. Spherical and cylindrical photos are not shown.

`W`, `A`, `S` and `D` walk through the scene, `Q` and `E` move down and up, and Shift walks faster. Forward and back follow the viewing direction, so looking down a stairwell and pressing `W` goes down it; sideways stays level. While walking, points are drawn thicker and keep a size in the scene, so that surfaces close by fill in.

Walking starts from the current orbit view, or from inside a station. Walking out of a station leaves its photo and continues through the point cloud with the station behind you; walking into another ball enters its photo. Escape, **Back to 3D view**, Zoom all or a camera direction returns to the orbit view. The selection tools are not available while walking.

`open-pointcloud-studio --photos INPUT OUTPUT_DIRECTORY` saves the stored photos of every station and prints where each one looks.

## Section box

**Section box** in the SECTION BOX group switches on a box that clips what is shown: points and meshes outside it are not drawn.

- Drag one of the six handles on its faces to move that face.
- While the box is on, Properties has a **Section box** section with a slider for each of the six limits, **Min** and **Max** fields for X, Y and Z in model coordinates with **Apply XYZ limits**, and **Zoom box**, which frames the clipped volume.
- **Fit selection** fits the box around the selected points, using the selected source points themselves, also those that are not on screen.
- **Reset box** opens the box to the whole model again.

The limits are coordinates in the model. They stay where they are when another layer is shown or hidden.

The box also limits box selection, point picking and both meshers. **Section box…** among the exports of the File view writes every source point of the active scan inside the box; see [Exporting and merging](#exporting-and-merging).

The box is aligned to the X, Y and Z axes of the model and cannot be rotated. A vertical cut through a building that stands at an angle to those axes is therefore a cut at that angle, not one along its walls.

## Selecting and editing

### Selecting

- **Box select**: draw a rectangle in the scene. Every source point of every visible scan inside it is selected, not only the points on screen. With an index the search visits only the parts of the index that the rectangle touches; without one it reads the whole source. While a search runs, **Cancel selection** takes the place of Zoom selection; a cancelled search leaves the previous selection as it was.
- **Pick point**: click a point. The nearest point of the active scan within eight pixels of the pointer is selected, and Properties shows its coordinates, colour, intensity and class under **Selected point**. A left drag does not orbit while Pick point is on: the point under the pointer is picked when the button is released. Pan with a middle or right drag, or orbit with Shift and a middle drag.
- **Clear** drops the selection. Escape leaves the selection tool and drops the selection as well.
- **Zoom selection** frames the selected points without changing the section box.

Selections honour the section box, the hidden classes and the points deleted before. For a very large selection the scene highlights a sample of the selected points; the count in Properties and in the status bar is the exact one.

### Editing

- **Delete**, or the Delete key, hides the selected points. **Undo** and **Redo** (the Undo delete and Redo delete icons in the top strip, Ctrl+Z and Ctrl+Y or Ctrl+Shift+Z; Command on macOS) restore and repeat up to eight deletions.
- **Thin** keeps the percentage set with the **Keep** slider, from 1 to 100, of the points that remain. It counts as a deletion and can be undone.
- **Move** shifts the active scan by the X, Y and Z values typed beside it.
- **Scale** scales the active scan by the X, Y and Z factors typed beside it, around the centre of gravity of the points that remain. For a large cloud that centre is calculated in the background, with progress in Properties and a **Cancel** button in place of Scale.
- **Reset transform**, under **Live transform** in Properties, puts the cloud back at its source coordinates.

Every scan has its own deletions, and Thin, Move and Scale act on the active scan.

None of these change the source file. The scene, the section box, the station markers, selections, meshes and exports all use the edited cloud, and an export writes it to a new file.

## Measuring distances and areas

The MEASURE group has **Distance** and **Area**. In either mode a left click picks the source point under the pointer from the active scan, with the same search and eight-pixel reach as Pick point. A drag still orbits and pan and zoom work as usual, so the view can be turned between two points. Backspace removes the last point, Enter finishes the measurement, and Escape stops measuring and drops an unfinished measurement. In Area mode a click on the first point also finishes.

**Distance** measures a polyline: each segment shows its length, and the total 3D length, the horizontal length as seen from above and the height difference between the first and the last point are reported.

**Area** measures the closed polygon through the points: its true area, so a sloped roof or a vertical wall is measured in its own plane, the plan area as seen from above, and the perimeter. The area is the length of the polygon's vector area; for points that are not in one plane that is the largest area the polygon shows from any direction.

The measurement is drawn over the points with a label on every segment and one for the total or the area (a single segment carries one label), and it is listed under **Measure** in Properties. Values are shown with three decimals and the unit m (m² for areas). The application takes the coordinates of a scan as metres and does not convert them: a scan stored in another unit shows its own numbers under that label. A measurement keeps its scene coordinates and stays visible while walking; new points are picked in the orbit view. A finished measurement remains until **Clear** in the MEASURE group or the first point of the next one. Switching to a selection tool keeps a measurement that has enough points and drops one that has not. One measurement holds at most 256 points and is not saved with the scan.

## Saved views, annotations and BCF

### Views

A view holds everything needed to come back to it: the camera (the orbit camera, or the walking camera when the view was saved while walking), the section box with its limits in model coordinates and whether it was on, the colour mode, the time it was saved, an identifier and its annotations.

**Save view** in the VIEWS group, or Save beside the name field under **Views** in Properties, saves what the scene shows. Without a name the view becomes "View 1", "View 2", and so on. Properties lists the views of the active scan: a click on a name restores the view, **Rename** changes its name, **Update** overwrites it with the current view and × deletes it. A scan has at most 32 views, with names of at most 64 characters that are unique within it.

Restoring puts the camera, the section box and the colour mode back. The orbit camera is relative to the bounds of the scene and to the size of the 3D view, so a view also keeps those: in a 3D view of another size the pan scales with the picture, and when other scans have been opened or closed since, the camera is moved to show what it showed.

Views are stored per source scan in `camera-views.json` in the settings folder. Every view in the file is read on its own, so a view that this version cannot read does not take the others with it, and a file that cannot be read at all is copied to `camera-views.unreadable.json` before the next save replaces it.

### Annotations

The view last saved or restored is the active view, shown highlighted in the list. Its annotations are drawn over the scene and stay on their points while the camera moves, in the orbit view and while walking; **Hide** stops showing them.

**Note** and **Line** in the VIEWS group place annotations by picking source points, with the same search and eight-pixel reach as the measuring tool; a drag still orbits.

- A note is a picked point with a text: after the click a field over the scene takes the text, and Enter or Add places a marker with a label and a leader.
- A line is two picked points, drawn as an arrow from the first to the second.

Escape cancels a half-placed annotation and, pressed again, leaves the tool. An annotation placed while no view is active first saves the current view. The annotations of the active view are listed in Properties, each with × to delete it. A view holds at most 64, and a note at most 240 characters. The label of a note stays inside the scene and moves away from its point past the markers and the labels of other notes close by. The annotation tools, the measuring tools and the selection tools exclude each other. A half-placed annotation is dropped when another scan becomes the active one, and the tool is left when the last scan is closed.

### Snapshots

Each view has a snapshot: a PNG image of the scene alone, with the points, the section box and the annotations. It is taken shortly after a view is saved or updated and again when its annotations change, once the change has been drawn and the points have refined. Snapshots are stored as `view-snapshots/<identifier>.png` beside `camera-views.json`, at most 1920 pixels along their longest edge, and are removed with their view. When a snapshot cannot be taken the view is saved all the same.

A snapshot is only ever taken while the scene shows the view: the view is the active one, and the camera, the section box, the colour mode and the bounds of the scene are what they were when the view was last saved, updated or restored. The camera of a view does not follow the scene. After turning or zooming to reach a point, placing or removing an annotation changes the view and leaves its snapshot as it is; the status line says that the snapshot is renewed when the view is restored, and restoring the view takes it. The same goes for a snapshot that could not be taken, because the camera moved before it was or because capturing failed.

When only the size of the 3D view has changed, with the window or with a status text of more lines, the pan first follows the picture as it does on restoring, and the snapshot is taken of that. A snapshot taken in a 3D view of another size, or in a scene with other bounds, than the view was saved with stores the view relative to that size and scene, so that the picture and the camera of the view keep belonging together.

### BCF export

**Export BCF** in the VIEWS group, or **Views as BCF…** among the exports of the File view, writes all views of the active scan as one BCF 2.1 file (`.bcf`, the BIM Collaboration Format of buildingSMART). The file is a ZIP container with `bcf.version` and one folder per view, named by the view's identifier:

- `markup.bcf`: a topic with that identifier, the name of the view as its title, its creation date and the account name of the user as author, the file name of the scan in its header, and one comment per note, each referring to the viewpoint.
- `viewpoint.bcfv`: a perspective camera, the six clipping planes of the section box when it was on (each on a face, pointing at the side that is cut away), and lines: each line annotation, and a short upright line of 0.25 m at the point of each note.
- `snapshot.png`, when the view has a snapshot.

The status line reports how many views were written, how many of them with a snapshot, and how many changed after their snapshot was taken and wait to be restored. Coordinates are model coordinates in metres.

### How the exported camera relates to the view

The exported camera reproduces the view: it stands where the application's camera stands, with the true vertical field of view. The walking camera maps directly. The orbit camera stands 1.8 scene extents from the scene centre. Panning shifts its picture instead of turning it, which a BCF camera cannot express, so the exported camera is turned in place towards what is in the middle of the 3D view.

Without pan the two pictures are the same. With pan they agree in the middle and drift apart towards the edges, by about the distance from the middle squared times the pan, divided by the focal length squared, where the focal length is 1.25 times the shorter side of the 3D view divided by the zoom. In a view of 800 by 600 pixels at zoom 1, a pan of 50 pixels gives 0.7 pixels at 100 pixels from the middle, 3 at 200 and 14 at the left and right edges; a pan of 200 pixels gives 7 at 200 pixels from the middle and 46 to 59 at the edges. A view that must match its snapshot to the edge is saved without pan.

The BCF 2.1 schema limits `FieldOfView` to 45–60 degrees and announces that readers should expect values outside that range. The file states the true angle, which lies outside it for most views: the orbit camera at zoom 1 has about 44 degrees and narrows as it zooms in.

## Exporting and merging

The exports are in the File view under **EXPORT**. Each asks where to save and then runs in the background. The format is the one chosen under **EXPORT FORMAT** on the Workspace page: binary or ASCII PLY, LAS, LAZ, E57, XYZ, PTS or CSV.

| Entry | What it writes |
| --- | --- |
| **Full resolution…** | The active scan without its deleted points, moved and scaled as in the scene. The Export active point cloud icon in the top strip does the same |
| **Selected points…** | The selected points of the active scan |
| **Without selected points…** | The active scan without the selected and the deleted points |
| **Section box…** | The points of the active scan inside the section box; available while the box is on |
| **Every Nth point…** | One point of the active scan in 2, 5, 10, 20, 50 or 100, as set under **EVERY NTH POINT** on the Workspace page |
| **Surface mesh…** | The mesh of the active scan as OBJ; see [Meshing](#meshing) |
| **Views as BCF…** | The saved views of the active scan; see [BCF export](#bcf-export) |

Every export reads the source again from start to end, so points that are not on screen are written too. The file appears under its name only when it is complete.

What an export keeps depends on the formats:

- **Same format, nothing edited**: an unedited LAS, LAZ or E57 cloud exported in full to its own format is a byte-for-byte copy.
- **LAS and LAZ to LAS or LAZ**: the original point records are written, with their point format, GPS time, return data, 16-bit colours, projection records and coordinate grid. Edits change only the fields they concern.
- **E57 to E57, filtered**: each source scan keeps its scanner pose, its name, its record types and values, and its custom fields. **Move** and a **Scale** with one positive factor for all three axes keep the scans when every scan has a pose.
- **Everything else**: position, 8-bit colour, intensity and class are written. A new E57 file made this way holds one scan in model coordinates; this writer has no class field for E57 and writes zero where a point has no colour or intensity.

**Merge visible LAS/LAZ scans…** joins the visible LAS and LAZ layers into one `.las` or `.laz` file in the background, with their deletions and their moves and scales applied and the original point attributes kept. The Workspace page shows the progress and a **Cancel merge** button; a cancelled merge leaves an existing file as it was. Sources with a different point layout, coordinate grid or coordinate system are refused before anything is written.

The command line does the same without a window: `--export`, `--section` and `--merge`; see the [README](../README.md#command-line).

## Meshing

The SURFACE group has two meshers. Both use the points that remain inside the section box and whose class is visible, ask for an `.obj` file, run in the background and then show the result in the scene as faces. Properties shows the progress; **Cancel mesh** stops the job and leaves an existing file as it was. One mesh job runs at a time.

- **Terrain mesh** passes every source point through a grid seen from above, keeps the lowest point in each cell and connects those to a 2.5D surface of at most 100,000 vertices. Long edges across gaps are left out. It suits ground and other surfaces seen from above.
- **3D surface** takes a sample of the source, thins it evenly to the number of vertices asked for, estimates a normal at each and connects neighbours in their tangent planes. It can follow vertical walls and overhangs. Sparse parts leave holes, and neighbouring patches can disagree, so the result is not watertight. While a scan is active, Properties has its settings: **Max vertices** (3 to 1,000,000; 50,000 by default), **Neighbors** (3 to 32; 12 by default) and a positive **Edge factor** (4 by default).

The OBJ file has the colours of the source where it has them and a normal per vertex.

A scan holds one mesh: a new mesh takes the place of the previous one, and Undo does not apply to meshes. The project panel has a separate **Surface** switch per layer, so the points can be hidden while the faces stay.

OBJ, PLY, OFF and STL files open as meshes and are drawn as faces too, several at once. Of a DXF file, which must be an ASCII DXF, the POINT entities open as points and the 3DFACE entities as faces; other entities are skipped. Colours per vertex of OBJ and PLY are shown; OBJ files also get the diffuse colours of their material file. Texture images are not drawn. A mesh shown in the scene has at most one million vertices and two million triangles per file. **Surface mesh…** in the File view, or **Export mesh as OBJ** under **Surface mesh** in Properties, saves the mesh of the active scan as OBJ.

Without a window: `--mesh`, `--surface` and `--mesh-export`.

## 3D BAG buildings

**3D BAG buildings…** in the File view opens a panel in the place of Properties that downloads building models of the Netherlands from the public 3D BAG register and shows them as a mesh layer.

- Type a bounding box in RD New coordinates, take it over from the active scan or the section box, or draw a rectangle on the map in the panel. The map can be panned and zoomed. The area of a scan or section box is only taken over when it lies where RD New coordinates lie; a scan in local coordinates leaves the fields and the map as they are.
- Choose the level of detail: 1.2, 1.3 or 2.2.
- The panel shows the size of the chosen area and, before **Download OBJ** is pressed, why an area cannot be downloaded: a side longer than 2,000 m, or coordinates that are not RD New.
- The download runs in the background and shows the page it is at, of how many, with the buildings read so far, and a **Cancel** button. Cancelling takes effect when the page under way has arrived and leaves an existing file as it was.

One download takes at most 2 by 2 km and about 5,000 buildings. A denser area is refused after the first page with the number of buildings it holds, and an old city centre whose buildings are very detailed can be refused after a few pages below that number; both need a smaller area. The reason of a failed download stays in the panel.

The result is saved as an OBJ file in RD New and NAP heights. The file keeps the [3DBAG attribution](https://docs.3dbag.nl/nl/copyright/) (CC BY 4.0), and the scene shows the credit while the buildings are visible. The map uses the background map of [Kadaster through PDOK](https://www.pdok.nl/copyright/) (CC BY 4.0) and shows that credit.

This needs an internet connection. 3D BAG is a built-in extension and can be switched off; see [Settings, language and extensions](#settings-language-and-extensions). Without a window: `--bag3d XMIN,YMIN,XMAX,YMAX 1.2|1.3|2.2 OUTPUT.obj`.

## Index and level of detail

A large cloud is not held in memory. The application builds an index on disk, an octree, and reads from it the points that the current camera needs, up to the point budget.

- A cloud of one million points or more is indexed automatically after it opens, one cloud at a time. **Auto-index** in the INDEX group switches that off.
- **Build index** starts a build for the active scan by hand. While a build runs, **Cancel index** takes its place; a cancelled build leaves nothing behind.
- **Refresh LOD** reads the detail for the current view again.
- The index is kept on disk and found again when the same unchanged file is opened later. A PLY, E57, PCD, PTX or text file then opens from its index without being read again. A changed file is read and indexed anew.
- An index can take several gigabytes for a large survey; see [Where settings and indexes are stored](#where-settings-and-indexes-are-stored).

While the camera moves, the points on screen stay. Shortly after it stops, the detail for the new view is read and takes their place, more of it for what is near and large on screen. At deep zoom the points inside the view are read from the index directly, so a close view shows the source points that are there.

**Indexed** under General in Properties says whether the active scan has its index, and **View sample** how many of its points are drawn.

`open-pointcloud-studio --index INPUT` builds or checks the index without a window and prints its number of points.

## Settings, language and extensions

The **Settings** button at the right of the top strip, **Settings…** in the File view and Ctrl+, (Command+, on macOS) open the same dialog:

- **General** has the language: Auto-detect, English or Nederlands. With Auto-detect the language is that of the system: the first of the user's preferred languages on macOS, the locale of the user on Windows and of the environment on Linux.
- **Appearance** has the theme: Deep Forge, Blueprint Light, Night Build, Blueprint Blue or High Contrast. A new installation starts in Blueprint Light. The scene stays dark in every theme.
- **About** has the name, the version, what the application is built with, the licences and a link to the source code.

A choice shows at once. **Save** keeps it, **Cancel** or Escape puts back what was in use, and **Reset to Defaults** chooses Auto-detect and Blueprint Light.

A text without a Dutch translation stays English. That holds for the messages in the status bar and for most texts with a count in them.

The **Extensions** page of the File view has a card for every built-in optional feature with its name, the version of the application, a description, its author, its category, whether it uses the internet and an **Enabled** switch. The only one is 3D BAG. Switching it off disables the **3D BAG buildings…** entry, closes the panel and stops a running download; `--bag3d` on the command line keeps working. Every extension is part of the application: no code from another source is loaded, and extensions from other sources cannot be installed.

## Where settings and indexes are stored

Settings are in the folder `open-pointcloud-studio-native`:

| System | Folder |
| --- | --- |
| Windows | `%APPDATA%\open-pointcloud-studio-native` |
| Linux and macOS | `~/.config/open-pointcloud-studio-native` |

When the environment variable `XDG_CONFIG_HOME` is set, on any system, the folder is `$XDG_CONFIG_HOME/open-pointcloud-studio-native`.

| File | What it holds |
| --- | --- |
| `settings.json` | Colour mode, point size, eye-dome lighting and its strength, station markers, point budget, Auto-index |
| `theme`, `language` | The theme and the language |
| `extensions.json` | The extensions that are switched off |
| `camera-views.json`, `view-snapshots/` | The saved views of every scan and their snapshots |
| `instances/` | The discovery file of each running window, for the [command API](../native/API.md) |

Indexes are in the folder `open-pointcloud-studio/indexes`:

| System | Folder |
| --- | --- |
| Windows | `%LOCALAPPDATA%\open-pointcloud-studio\indexes` |
| Linux and macOS | `~/.cache/open-pointcloud-studio/indexes` |

When `XDG_CACHE_HOME` is set, the folder is `$XDG_CACHE_HOME/open-pointcloud-studio/indexes`. The folder can be deleted while the application is closed; the indexes are built again when the scans are opened.

Nothing is written beside the scans, and the scans themselves are never changed.

## Keys and mouse

| Input | What it does |
| --- | --- |
| Left drag | Orbit; in Box select, draw the rectangle; in Pick point, the view does not turn and the point under the pointer is picked on release |
| Left click | Pick a point in Pick point, Distance, Area, Note and Line; click a station marker, a station ball or the view cube |
| Middle or right drag | Pan |
| Shift + middle drag | Orbit |
| Wheel | Zoom at the pointer |
| Right click | Menu of the scene |
| `F` | Isometric overview of the whole model (Zoom all) |
| Delete | Hide the selected points |
| Ctrl+Z | Undo the last deletion |
| Ctrl+Y, Ctrl+Shift+Z | Redo |
| Enter | Finish a measurement; place a note |
| Backspace | Remove the last point of a measurement |
| Escape | Close Settings or the File view; cancel a half-placed annotation; leave walking; otherwise leave the active tool, stop a running selection and drop the selection |
| `W` `A` `S` `D` | Walk forward, left, back and right |
| `Q` `E` | Move down and up |
| Shift, while walking | Walk faster |
| Ctrl+, | Open Settings |

On macOS the Command key takes the place of Ctrl. The letter keys work while no text field has the focus.
