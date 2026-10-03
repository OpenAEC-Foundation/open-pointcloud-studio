# Changelog

What changed in each release of Open Pointcloud Studio, newest first.

<!--
The release workflow copies the section of the version being released into
the release notes and refuses a release without one. A section starts with
"## X.Y.Z - YYYY-MM-DD" and lists its changes as "- " items in the words of
someone using the application. The foundation's website shows these items and
drops those that begin with the name of an operating system or with
"Download", so begin an item with what changed. New work goes under
"Unreleased"; the release step gives that section its version and date.
-->

## Unreleased

- **Section drawing** in the SECTION BOX group makes a 2D drawing at scale 1:1 of what the section box cuts and saves it as DXF or DWG: a plan from the slab under the top face of the box, or a vertical section from the slab behind one of its sides. The drawing holds the points of the slab from every visible scan, thinned to one per 5 mm, on layers per scan or per class, in millimetres or metres.
- A section drawing can fill what the slab cuts: walls, columns and floors become filled regions with outlines, and door and window openings stay open. **Preview** shows these regions over the points before a file is saved. A wall that was scanned from one side is drawn as a thin strip, and gaps under about half a metre are closed; the user guide lists the limits.
- **Section drawing…** in the File view saves the drawing directly, the strip above the scene shows the job with a button to cancel it, and the status bar and Properties report the points in the slab and in the drawing, the point spacing, the regions, the grid cell, the main direction and the file size.
- The command API and the MCP server have the commands `export_drawing`, `preview_drawing`, `clear_drawing_preview` and `cancel_drawing`, `status` reports the Section drawing tool, and `--drawing` on the command line draws a box of a scan file as DXF or DWG.
- A mesh can be saved as OBJ, as binary PLY or as binary STL: **Surface mesh…** in the File view and **Export mesh…** in Properties offer the three formats, and the extension of the file name chooses. This holds for a terrain mesh, a 3D surface, an opened mesh file and downloaded 3D BAG buildings. PLY keeps survey coordinates in double precision with colours and normals; an STL file of a mesh far from zero is written relative to a whole-metre origin that the status bar and the file header name.
- Properties shows the open edges and the number of connected parts of a mesh, next to its vertices and triangles; the status bar gives them when a terrain mesh or 3D surface is finished. For a mesh that was opened from a file, vertices at the same position count as one, so a closed surface shows no open edges however the file numbers its vertices.
- A scan that is mirrored by a negative scale factor keeps its outside in a saved mesh: the triangles and normals of the file face outward, also in the OBJ file that a mesh job writes.
- Buildings from 3D BAG that were saved as OBJ or PLY keep their credit when that file is opened and saved again; a PLY file carries it in plain ASCII.
- The command API and the MCP server have an `export_mesh` command, `status` lists the mesh of each layer, the result of a mesh job reports open edges and connected parts, and `--mesh-export` on the command line writes OBJ, PLY or STL by the extension of the output.
- Packages for macOS: a disk image with the application for arm64 and x86-64 processors in one file, for macOS 11 and later. It is signed ad hoc, not with a developer certificate, so the first start has to be allowed once; the image and the release notes say how.
- Packages for Linux: a `.deb` for Debian, Ubuntu and their relatives and an AppImage for any distribution, both for x86-64 and, as an experiment, for 64-bit ARM. They add a menu entry with the application icon and offer the application for E57, LAS, LAZ, PLY, PCD, PTX and PTS files and scan project files.
- The installer for Windows now offers the application under "Open with" for scan project files (`.rcp`) too.
- Release files have new names of the form `open-pointcloud-studio_VERSION_TARGET`, for example `open-pointcloud-studio_0.8.0_x64-setup.exe`; the archives with only the binary remain, now also for 64-bit ARM Linux and with both processor types in the macOS one.
- Release notes list what changed, and a release is published only after the tests have passed on all three systems and every package has been installed and started on a build machine.

## 0.7.0 - 2026-10-03

- A Model Context Protocol server: `open-pointcloud-studio --mcp` gives a client program or a script a tool for every command of the local command API, a screenshot tool that returns the 3D view as an image, tools that wait for long tasks, and tools to list, choose and start windows.
- A screenshot command in the local command API, which saves the 3D view as a PNG once its points are loaded.

## 0.6.0 - 2026-10-02

- An installer for Windows, in English or Dutch: it installs for the current user without administrator rights or for all users, adds a Start menu entry and an optional desktop icon, offers the application under "Open with" for point-cloud files without changing their default program, and brings an uninstaller.
- Settings with a choice of language (English, Dutch, or that of the system) and of theme, and an About page with the version. The interface is available in Dutch.
- Saved views now keep everything needed to come back to them: the camera, orbiting or walking, the section box, the colour mode and their annotations.
- Notes and arrows can be placed on exact points of a scan and are saved with the active view.
- All saved views of a scan export as one BCF 2.1 file, each with its notes, camera, clipping planes and a snapshot of the scene.
- The top strip shows a Home tab beside File and ends with a Settings button; the rows of the ribbon are taller, with larger icons and labels.

## 0.5.0 - 2026-10-02

- A strip above the scene shows how far every opening or indexing task is, how long it still takes, and a button to cancel it; each row of the project list shows the progress of its own scan.
- A large merged E57 cloud shows a preview of two million points within seconds, while the rest of the file is still being read.
- A source of 512 MiB or more shows the points read so far every few seconds while it is opening.
- Building the index of a large cloud uses several processor cores and is about seven times faster.
- Several scans can be selected in the project list with Shift-click and Ctrl-click, and then hidden, shown or closed together.
- Walking forward and back follows the viewing direction, so looking down a stairwell and pressing W goes down it; points are drawn thicker while walking.
- All tools sit on one ribbon without tabs, and the view cube shows its corners.
- The application logo is drawn as nine points, and the title bar on Windows has the colours of the chosen theme.

## 0.4.2 - 2026-10-02

- The window, the top strip and the executable on Windows carry the application logo instead of a generic icon.
- The project panel lists the clouds in name order on one compact row each, and below them the classification codes that occur, each of which can be shown or hidden like a layer.
- LAS and LAZ files whose quick preview cannot be read now open through a full pass instead of failing.

## 0.4.1 - 2026-10-02

- Distances and areas can be measured between picked points: a distance reports every segment, the total length, the horizontal length and the height difference; an area reports the true area in its own plane, the plan area and the perimeter.

## 0.4.0 - 2026-10-02

- A new native desktop application replaces the earlier one, which is archived. It needs no web view and comes as one executable for Windows, Linux and macOS.
- Opens point clouds in LAS, LAZ, E57, PLY, PCD, PTX, PTS and text formats, and meshes in OBJ, OFF, STL and DXF.
- Surveys of many gigabytes stay usable: a preview appears first, detail for the current view is read from an index on disk, and a scan that was opened before reopens from a cache.
- A whole scan project opens at once from its folder, from its scan project file (`.rcp`), or by dropping either on the window.
- The 3D view has a view cube, camera presets, colouring by stored colour, elevation, intensity or classification, eye-dome lighting and a point budget of up to ten million points.
- Scanner stations of E57, PCD and PTX scans are shown in the scene. Stations with photos are drawn as balls: click one to stand in that station and look around, and walk through the scene with W, A, S and D.
- Points can be selected exactly by box or by picking, deleted with undo and redo, thinned, moved and scaled; a section box limits what is shown and exported.
- Exports the whole cloud, the selection, everything but the selection, the section box or every Nth point as LAS, LAZ, E57, PLY, XYZ, PTS or CSV, and merges several LAS or LAZ scans into one file.
- Builds a terrain mesh or a 3D surface from the points and saves it as OBJ.
- Camera views can be saved per scan and restored.
- Five colour themes, a File view for opening, exporting and merging, and a ribbon with all tools.
- A local command API drives a running window from scripts, and command-line modes export, cut a section, index, merge and mesh without opening a window.
- Each release archive carries the licence texts and has a checksum file beside it.

## 0.3.0 - 2026-03-03

Release of the earlier application, which was built with web technology and is no longer developed.

- Opens PCD, PTX, OFF, STL, DXF and ASC files besides the formats of 0.2.0.
- E57 files with data that is not aligned to pages, and with unsigned 32-bit values, are read correctly.
- A Zoom all button and the F key fit the camera to the geometry.
- The default point budget went from five million to one million points for smoother navigation.

## 0.2.0 - 2026-02-26

First published release of the earlier application.

- Opens LAS and LAZ point clouds and shows them with level of detail from an octree.
- Tools for working on point clouds, surface reconstruction and export to several formats.
- An extension that loads 3D building models of the Netherlands for the area of a scan.
- Installers for Windows, macOS and Linux.
