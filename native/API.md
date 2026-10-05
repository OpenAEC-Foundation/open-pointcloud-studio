# Native command API

## Connecting

The Rust desktop app starts a local command server on `127.0.0.1`. It executes
named Rust operations in the running GUI; it does not evaluate JavaScript or
use a webview. The port and a per-process token are written to
`$XDG_CONFIG_HOME/open-pointcloud-studio-native/instances/instance-<pid>.json`
(or `~/.config/open-pointcloud-studio-native/instances/` when XDG_CONFIG_HOME
is unset; on Windows `%APPDATA%\open-pointcloud-studio-native\instances\`). The directory is mode 0700 and the discovery file mode 0600 on
Unix. Besides `port` and `token`, the file holds the process ID `pid`, the
API name `api` (`native-rust-v1`) and `started`, the time the server started
in milliseconds since 1970, so a client can tell several windows apart and
pick the newest. A fixed port can be requested with `--api-port PORT [INPUT ...]`.
[MCP.md](MCP.md) describes `--mcp`, a Model Context Protocol server that finds
windows this way and offers these commands as tools.

`GET /health` returns `{"status":"ok"}`. `GET /info` returns the process ID,
port, version, API name and start time. `POST /exec` accepts one JSON command and requires
the discovery file's token in the `X-OPS-Token` header. A request without a
valid token receives HTTP 403. The legacy `POST /eval` endpoint returns HTTP
410 because script evaluation is not part of the native application.

For example, after reading `port` and `token` from the discovery file:

```bash
curl -H 'Content-Type: application/json' -H 'X-OPS-Token: TOKEN' \
  -d '{"command":"status"}' http://127.0.0.1:PORT/exec
```

## Answers, jobs and imports

Commands use absolute file paths. They return JSON with `ok: true` or
`ok: false` and an `error`. File opening returns `accepted: true` as soon as
the GUI starts loading; poll `status` for the new layer. `open` answers after
a folder or scan project file has been read on a worker thread, with the list
of files it started loading. Exports, section drawings and face detections
return `accepted: true` and a `job_id`. Query `{"command":"job","id":"JOB_ID"}`
for a durable `running`, `complete` (with point count), or `failed` result.
The newest 32 jobs remain queryable even if the GUI status line changes.
Non-LAS/LAZ imports return an `import_id`; `status.result.imports` lists active
imports with decoded finite-point counts and cancellation state. Use
`cancel_import` with that ID to stop a long import. A cancelled import never
adds a partial layer. LAS/LAZ header previews open immediately and have a null
`import_id`.

## Meshing and merging

`mesh` with `mode` `terrain` or `surface` meshes the active layer and writes
an OBJ file; `path` is required. These jobs report `reading`,
`reconstructing`, or `writing` with completed and
total units. `cancel_mesh` requests cancellation; a cancelled mesh leaves an
existing destination untouched. Only one mesh job runs at a time, of whatever
mode.
The complete job has `path`, `mode`, `source_points`, `vertices` and
`triangles`, and what was measured of the mesh: `open_edges`, the edges that
belong to one triangle only (the rims of the surface and of its holes), and
`components`, the parts of the mesh that share no vertex with each other. A
closed surface has no open edges. The distance between the points and the
mesh is not measured for these two modes; the mode `closed` measures it, see
[Closed mesh](#closed-mesh).
A mesh holds at most 4,000,000 vertices and 8,000,000 triangles, from a job
and from a file alike.
`merge_visible` joins all visible LAS/LAZ layers into one `.las` or `.laz` file
in a background task. It preserves original point attributes and applies each
layer's current deletions and affine transform. Sources must have matching LAS
version, point layout, coordinate grid and metadata; incompatible CRS metadata
is rejected instead of silently choosing one. Poll its job or
`status.result.merge` for processed and written point counts. `cancel_merge`
stops the task and leaves an existing destination unchanged.

## Closed mesh

`mesh` with `mode: "closed"` makes a closed mesh, as the Closed mesh block of
Properties does: a surface without overlapping faces from the points inside
the section box, or from the whole layers when the box is off, closed
wherever the scan has points or a gap up to `max_hole` wide. Deleted points
and hidden classes are left out. The mesh goes to the active layer, where it
takes the place of the mesh that layer had, and is kept in the frame of that
layer, so a later `translate` or `scale` takes it along. `path` is optional:
with an absolute `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc` destination in
a folder that exists the mesh is also written there, as the scene shows it
and in the format described under Mesh export; without it the mesh is shown
only, and `export_mesh` saves it later.

The other fields are the settings of the block. A field that is left out
keeps what the block has, and a field that is given is put in the block as
well. `set_closed_mesh_settings` takes the same fields without starting a job
and answers with `settings`. Both check the fields together: when one is
refused, none is taken.

- `voxel`: edge of a voxel in metres, 0.005 to 0.5, or `null` for automatic,
  as the block starts: 0.02 for a region up to 20 m long, 0.03 up to 60 m and
  0.05 beyond. Detail under about two voxels is lost.
- `max_hole`: gaps in the points up to this wide are closed, in metres, 0 to
  3.2 and never more than 32 voxels; 0.25 at the start. Wider openings stay
  open.
- `simplify_mm`: how far simplification may move the surface, in millimetres,
  0 (none) to 1000, or `null` for automatic, as the block starts: 0.15 voxel.
- `sample_percent`: the share of the source points the surface is fitted
  to, in percent, 0.01 to 100; 100 at the start uses every point. A smaller
  share takes the same points on every run, spread over the region. A job
  whose share leaves no point in the region fails with `the source sample
  left no points in the region; raise the source percentage`.
- `sides`: which side of a surface is its front. `automatic` (the start)
  takes the scanner station that measured it where the layer knows its
  stations, and the centre of the region elsewhere. `centre` takes the centre
  of the region for every surface and does not use stations. `upward` turns
  every surface up, for data measured from above. The centre of the region is
  the middle of the section box after it has been cut back to where the
  layers have points.
- `layers`: `active` (the start) takes the points of the active layer,
  `visible` those of every layer whose points are shown and that reaches the
  section box when the box is on. Layers of 3D BAG buildings are left out of
  `visible`. A layer that is left out is not read and does not widen the
  region.

The running job, which `status.result.closed_mesh.job` holds as well, has
`operation` (`mesh`), `mode` (`closed`), `path` (or `null`), `stage`,
`completed`, `total`, `fraction` (or `null` where the stage has no total),
`cancel_requested` and `elapsed_seconds`. The stages are `stations` (reading
a source once to learn which station measured each point, for a layer whose
index does not say), `reading` (reading a layer without an index into
memory), `planning`, `reconstructing` (`completed` of `total` blocks),
`simplifying`, `measuring` and `writing` (with a `path`).

The complete job has, with lengths in metres:

- `vertices`, `triangles`, `triangles_extracted` (the triangles before
  simplification), `open_edges`, `components` and `non_manifold_edges` (edges
  with more than two triangles: none, except where two surfaces lie closer
  together than two voxels or a surface came out torn).
- `deviation_mean`, `deviation_p95` and `deviation_max`: the distance from
  `deviation_samples` points of the region, at most 200,000 spread evenly, to
  the nearest triangle. The largest value counts stray points too.
- `voxel`, `max_hole` and `simplify_tolerance` as they were used, `region`
  (`min` and `max` of the box that was meshed: the section box cut back to
  the points), `points` (the points in the region) and `blocks`.
- `sides`: `stations`, `mixed` or `fallback`, with `surfels` (the pieces of
  surface the points were reduced to), `surfels_by_station`,
  `surfels_without_station` (those that took their side from the centre, from
  upward or from the stationed surface beside them; with `sides` set to
  `centre` or `upward` stations are not used and this is every piece) and
  `surfels_undecided` (those the centre sees edge on: they face up, or one
  fixed direction when upright, which can be the wrong side).
- `advice`: a line of text when the figures call for it, otherwise `null`.
  Its counts are plain numbers, as the fields are.
- `shown` (false when the layer was closed while the job ran), `path`,
  `format` and `origin` (as for `export_mesh`; `null` without a `path`), and
  `seconds`.

`cancel_mesh` stops the job after the block under way; the job becomes
`cancelled`, and the mesh of the layer and an existing file stay as they
were. A job that fails has the reason in `error`: among them a region without
points (`the region holds no points to mesh`) and a result above 4,000,000
vertices or 8,000,000 triangles, which asks for a larger voxel, a larger
simplification tolerance or a smaller section box.

`status.result.closed_mesh` holds `settings` (the fields above; `voxel` and
`simplify_mm` are `null` for automatic, and any number that cannot be read is
`null`), `job` (the running job, or `null`) and `last` (how the last job
ended, as its job reports it, or `null`).

The command is refused without an active layer (`no active cloud`), with
`layers: "visible"` and no visible layer of scan points, or with an active
layer of 3D BAG buildings, which does not take a mesh of the scans (make a
scan the active layer first), for an active layer with a scale of zero on an
axis (`the scan that gets the mesh has a scale of zero`), for a layer that
takes part and is still being imported, for such a layer of more than
5,000,000 points without an index (build it first), for a section box that
none of the layers reaches, for a value
outside the limits above, for a `path` that is not absolute, has another
extension, lies in a folder that does not exist or is the source file of an
open layer, and while another mesh job of any mode is open or running. The
settings are refused with `mode` `terrain` or `surface`. A refused command
changes nothing in the block.

## Mesh export

`export_mesh` saves the mesh the active layer holds: a terrain mesh, a 3D
surface, a closed mesh, the faces of an opened mesh file or downloaded 3D BAG
buildings.
`path` is an absolute destination whose extension chooses the format:

- `.obj`: text, with colours and normals per vertex where the mesh has them.
- `.ply`: binary little-endian, with double coordinates, colours as `uchar`
  and normals as `float` where the mesh has them.
- `.stl`: binary, triangles only, with 32-bit float coordinates.
- `.dxf` and `.dwg`: drawing version R2013 in metres, with the triangles as
  `MESH` entities of at most 65,536 faces each on the layer `OPS-MESH`.
  These are editable meshes; no ACIS solid (`3DSOLID`) is written.
- `.ifc`: IFC4 as a STEP file: a project with a site, building and storey
  holding one `IfcBuildingElementProxy` (object type `Mesh`) with an
  `IfcTriangulatedFaceSet`, closed when every edge has two triangles, and a
  property set `OPS_ScanGeometry` with the counts. Where the coordinates lie
  more than 1,000 m from zero on an axis, the site is placed at the middle of
  the mesh rounded to whole metres and the geometry is relative to it; the
  description of the site names that point. No map conversion is written,
  as the coordinate system of the scan is not known.

The mesh is written as the scene shows it, with the move and scale of its
layer applied. The command answers with a `job_id`; the complete job has
`operation` (`export_mesh`), `path`, `format` (`obj`, `ply`, `stl`, `dxf`,
`dwg` or `ifc`),
`vertices`, `triangles` and `origin`. `origin` is `null` except for an STL
file of a mesh that lies more than 2,048 m from zero on an axis: such an axis
is written relative to a whole-metre origin, so that the floats keep their
precision. `origin` is that point `[x, y, z]`, and the 80-byte header of the
file names it as `origin X Y Z m`. This application adds it again when it
opens the file; another program shows the mesh near zero.

The file is written to a temporary file first and appears under its name
when it is complete; a failed job leaves an existing destination as it was.
The command is refused for a path that is not absolute or has another
extension, for a layer without a mesh (`status.result.clouds[].mesh` is
`null` there), for the source file of the layer as destination, and while
another mesh export is open or running. `status.result.mesh_export_pending`
is true while the file is written.

## Detected faces

`detect_faces` finds the flat faces (floors, ceilings, walls and sloped
planes) and the round columns and pipes in the points inside the section box,
or in the whole layers when the box is off, as the Detect faces block of
Properties does. Deleted points and hidden classes are left out. The faces
are kept with the active layer as a layer of their own beside its mesh, in
the frame of that layer, so a later `translate` or `scale` takes them along;
faces the layer had are replaced. The command answers with a `job_id`.

Its fields are the settings of the block. A field that is left out keeps what
the block has, and a field that is given is put in the block as well.
`set_face_settings` takes the same fields without starting a job and answers
with `settings`. Both check the fields together: when one is refused, none is
taken.

- `distance_tolerance`: how far a point may lie from the plane of its face,
  in metres, 0.001 to 0.5; 0.02 at the start. The block shows it in
  millimetres. About three times the noise of the scan or more.
- `angle_tolerance`: how far the surface at a point may be turned from its
  face, in degrees, 1 to 45; 10 at the start.
- `min_area`: smaller faces are not reported, in square metres, 0.01 to
  10000; 0.25 at the start.
- `cylinders`: whether round columns and pipes are looked for; true at the
  start.
- `layers`: `active` (the start) takes the points of the active layer,
  `visible` those of every layer whose points are shown and that reaches the
  section box when the box is on, without layers of 3D BAG buildings. The
  active layer has to be one of them: it keeps the faces.
- `color`: how the faces are drawn, for every layer: `face` (the start), one
  colour per face by its class, or `deviation`, the mean distance of the scan
  to the face per cell of 5 cm: blue where the scan lies `distance_tolerance`
  or more behind the face, near white on it, red in front of it.

The other values of a detection are fixed: voxels of 0.03 m to start with, a
working set of 1,500,000 voxels (a region with more gets voxels of 0.06,
0.12 m and so on), faces at least 0.15 m wide, an outline grid of 0.05 m,
gaps closed up to 0.10 m, holes filled below 0.05 m2, and cylinders with a
radius of 0.01 to 1 m, at least 0.30 m long and scanned over at least 90
degrees.

The running job, which `status.result.faces.job` holds as well, has
`operation` (`detect_faces`), `stage`, `completed`, `total`, `fraction` (or
`null` where the stage has no total), `cancel_requested` and
`elapsed_seconds`. The stages are `stations` (reading a source once to learn
which station measured each point, for a layer whose index does not say),
`loading` (reading a layer without an index of at most 5,000,000 points into
memory; a larger one is read from its file twice), `reading`, `segmenting`,
`measuring`, `outlining` and `meshing` (the two meshes of the viewer).

The complete job has, with lengths in metres:

- `count`, `planes`, `cylinders`, and per type `floors`, `ceilings`, `walls`
  and `sloped`; `edges`, the stretches of line two flat faces share.
- `voxel_size` as the job ended with it, `coarse` (true when it had to grow,
  which loses narrow faces and faces close together), `density_doublings`
  (how often it grew because the points lie far apart) and `note`, a line of
  text about that or `null`.
- `points`: `read` (in one pass), `source` (of the region, after the
  filters), `working` (voxels) and `assigned` (on a face of the result).
- `region` (`min` and `max` of the box around the points that took part, or
  `null`), `seconds`, `source` (the file name of the layer that keeps the
  faces, `null` when it was closed while the job ran) and `kept` (false when
  nothing was found or that layer was closed: the faces a layer had then stay
  as they were).

`cancel_detect_faces` stops the job after the step under way; the job becomes
`cancelled` and the faces of the layer stay as they were.

`status.result.faces` holds `settings` (the fields above; a number that
cannot be read is `null`), `job` (the running job, or `null`), `last` (how
the last job ended, as its job reports it, or `null`), `export_pending` (true
while a faces file is written or its save dialog is open) and `result`, the
faces of the active layer in figures, or `null`. `status.result.clouds[].faces`
has those figures for every layer: the fields of a complete job from `count`
to `seconds` without `note`, and `selected` (the number of the highlighted face or `null`),
`visible` (the Faces switch of the project list), `drawn` (false when the
faces are hidden, or do not fit the buffers of the graphics device beside
the meshes that are shown), `color` and `stale`.

`stale` is `null` for faces that belong to the points as they are, and
otherwise says why they are out of date: `points` (points of a layer that
took part were deleted, restored or thinned), `index` (the index such a layer
was read through was replaced; the first index of a layer that was read
without one does not count), `scan` (such a layer was removed) or `moved`
(the faces were made from several layers, and one of them was translated or
scaled afterwards, so the layers no longer stand together as they did). A
`translate` or `scale` of a layer does not make faces stale that were made
from that layer alone: they follow it. Stale faces stay listed, drawn and
exportable until `detect_faces` or `clear_faces`.

`list_faces` answers with the faces of the active layer in scene coordinates,
as they stand after a move or scale of the layer: `source`, `count`,
`selected`, `stale`, `region`, `settings`, `points`, `edge_count` and
`faces`, each as in the JSON file below. Without `boundaries: true` the faces
come without their `boundary` and the answer without `edges`. A layer that is
scaled unequally along its axes lists no cylinders.

`select_face` highlights the face with the number `id` in the viewport and in
the list of the block, and answers with `selected` and that `face`, outline
included; without `id`, or with `null`, it takes the highlight off.

`export_faces` saves the faces of the active layer; `path` is an absolute
destination in a folder that exists, and its extension chooses the format:

- `.json`: the document below.
- `.obj`: the faces as triangles in one group per face, named
  `face_0001_wall` after its number and class, with the normal of the face at
  every corner, and a cylinder as the scanned part of its surface in a group
  `face_0007_cylinder`.
- `.dxf` and `.dwg`: 3D geometry for a CAD program, drawing version R2013 in
  metres and scene coordinates. Every flat face is a polyface mesh on the
  layer of its class (`OPS-PLANES-FLOOR`, `OPS-PLANES-CEILING`,
  `OPS-PLANES-WALL`, `OPS-PLANES-SLOPED`) whose inner triangle edges are
  invisible, so that it shows as its outline with its openings. Every
  cylinder is a polyface mesh of its scanned part on `OPS-CYLINDERS`, with
  its axis as a line on `OPS-CYLINDER-AXES`. A polyface mesh holds at most
  32,767 vertices and faces; a larger face is split over several. No ACIS
  solids (`3DSOLID`) are written: a CAD program can turn the meshes into
  surfaces or solids where it offers that.
- `.ifc`: IFC4 as a STEP file, with a project, site, building and storey.
  Every face is an `IfcBuildingElementProxy` with object type
  `Plane (wall)` and so on, or `Cylinder`, as the class is a direction and
  not a building element. A flat face is an `IfcPolygonalFaceSet` with a
  face per part and its openings as inner loops; a cylinder seen from
  outside is an `IfcExtrudedAreaSolid` of an `IfcCircleProfileDef` of its
  radius along its axis over its scanned length, with the axis as an `Axis`
  representation; a cylinder seen from inside is the triangulated scanned
  surface. The property set `OPS_ScanGeometry` holds the class, area,
  coverage and residuals of a face and the radius, length and arc of a
  cylinder. Coordinates far from zero are placed as for `export_mesh`.

The complete job has `operation` (`export_faces`), `path`, `format` (`json`,
`obj`, `dxf`, `dwg` or `ifc`), `planes`, `cylinders`, `edges`, `bytes` and `stale`. The file is
written to a temporary file first and appears under its name when it is
complete. The command is refused for a path that is not absolute or has
another extension, for a folder that does not exist, for the source file of
an open layer as destination, for a layer without faces and while another
faces export is open or running.

`clear_faces` removes the faces of the active layer and answers with
`cleared`, false when it had none.

The JSON file, in scene coordinates and metres:

```text
{
  "format": "open-pointcloud-studio-faces",
  "version": 1,
  "source": "scan.e57",            file name of the layer, no folder
  "units": "metres",
  "region": {"min": [x, y, z], "max": [x, y, z]} or null,
                                   box round the points that took part
  "settings": {
    "distance_tolerance", "angle_tolerance_deg", "min_area",
    "min_plane_width", "max_gap", "min_hole_area", "cylinders",
    "voxel_size",                  as used: the size asked for or a doubling
    "voxel_size_asked",
    "boundary_cell",               as used
    "boundary_cell_asked",
    "coarse": false,               true when the voxels had to grow: narrow
                                   faces and faces close together are lost
    "density_doublings": 0         how often they were doubled because the
                                   points lie far apart
  },
  "points": {"read", "source", "working", "assigned"},
  "faces": [{
    "id": 1,                       from 1, largest face first
    "type": "plane",
    "class": "floor" | "ceiling" | "wall" | "sloped",
                                   by the direction of the normal alone: a
                                   table top is a floor, a cabinet front a wall
    "normal": [x, y, z],           unit, on the side the face was scanned from
    "normal_from": "stations" | "nearest_station" | "open_side" | "centre",
    "point": [x, y, z],            a point of the plane
    "offset": d,                   the plane is normal . x = offset
    "area", "covered_area", "coverage",
    "coplanar_group": n,           equal for faces in one plane
    "boundary": [{                 one entry per connected part
      "outer": [[x, y, z], ...],   counter-clockwise seen from the normal's
                                   side; closed, first corner not repeated
      "holes": [[[x, y, z], ...]]  clockwise
    }],
    "residual": {"points", "inliers", "rms", "mean", "mean_abs", "p95", "max"}
  }, {
    "id": 7,                       the numbers go on after the planes
    "type": "cylinder",
    "axis_start": [x, y, z],       the axis, as far as the surface was scanned
    "axis_end": [x, y, z],
    "radius", "diameter", "length",
    "arc_degrees": a,              how much of the round was scanned
    "arc_start": [x, y, z],        unit direction from the axis to where that
                                   arc begins
    "arc_side": [x, y, z],         unit direction the arc runs towards
    "seen_from_inside": false,     true for the inside of a round shaft
    "area",                        of the scanned part
    "residual": {...}              as for a plane; positive is outside
  }],
  "edges": [{"faces": [a, b], "start": [x, y, z], "end": [x, y, z],
             "length", "angle_deg"}]
}
```

The planes come first and then the cylinders, each largest first. Residuals
are distances of scan points to their face, over the points within three
distance tolerances of it whose surface runs along the face, and those within
one tolerance of what stands against it: `points` of them, `inliers` within
one tolerance; `mean` keeps its sign (positive on the side of the normal),
the others do not. `angle_deg` of an edge is the angle between its two faces
on the side their normals point to: 90 in the corner of a room, 270 around
the corner of a pillar.

`detect_faces` is refused without an active layer (`no active cloud`), for
an active layer with a scale of zero on an axis (`the scan that keeps the
faces has a scale of zero`), for a layer that takes part and is still being
imported or has no points, for a section box that none of the layers reaches, for a value
outside the limits above and while another detection runs. With
`layers: "visible"` it is also refused without a visible layer of scan
points, for an active layer of 3D BAG buildings, and for an active layer
that is hidden or lies outside the section box. A refused command changes
nothing in the block. One detection runs at a time; it can run beside a mesh
job or a section drawing. A job that finds no face is `complete` with
`count` 0 and `kept` false.

## Section drawings

`export_drawing` draws what the section box cuts as a 2D drawing at scale 1:1
and writes it as DXF or DWG; `path` is an absolute destination in a folder
that exists, and its extension (`.dxf` or `.dwg`) chooses the format. The cut
plane is one face of the box, and the drawing holds the slab behind that face,
`thickness` metres deep and never deeper than the box:

| Cut plane | `view` | To the right in the drawing | Up in the drawing |
| --- | --- | --- | --- |
| The top face (Z max), seen from above | `plan` | +X | +Y |
| The face at Y min, looking along +Y | `front` | +X | +Z |
| The face at Y max, looking along -Y | `back` | -X | +Z |
| The face at X min, looking along +X | `left` | -Y | +Z |
| The face at X max, looking along -X | `right` | +Y | +Z |

A section box that is turned about the vertical (`rotation` of
`set_section`) takes its faces along: X and Y in this table are then the own
axes of the box, so a box turned along the walls gives a plan with the walls
along the axes of the drawing and vertical sections parallel to a wall. A plan
of a turned box with `origin` `model` has the model X and Y turned with the
box about the model origin.

The other fields are the choices of the Section drawing block in Properties.
A field that is left out keeps what the block has, and a field that is given
is put in the block as well, so the window shows what was drawn:

- `thickness`: depth of the slab in metres, 0.005 to 5; the block starts at
  0.10.
- `units`: `mm` (the start) or `m`. Scene units are taken as metres.
- `origin`: `model` (the start) keeps model X and Y in a plan and model Z as
  the height in a vertical view, where the horizontal axis starts at the left
  edge of the box as seen; `box` puts the lower left corner of the view at
  zero.
- `fill`: whether the material the cut plane goes through is drawn as filled
  regions with their outlines. Choosing another `view` than the block has
  sets it to true for `plan` and false for the others, unless `fill` is given.
- `square`: whether an edge of the filled cut is turned onto the main
  direction of the building when that moves neither of its ends more than
  30 mm; true at the start.
- `grid`: cell of the grid the filled cut is traced from, in metres, at least
  0.005; 0.02 at the start.
- `max_wall_thickness`: two scanned faces at most this far apart are filled
  as one wall, in metres, above 0 and at most 2; 0.50 at the start. Gaps up
  to this width are closed too, wider ones stay open.
- `color`: `layer` (the start) or `rgb`, the scanned colour of each point.
- `point_layers`: `scan` (the start: a layer per scan file when several are
  drawn) or `class` (a layer per classification).
- `max_points`: the most points in the drawing, 1 to 400,000; 150,000 at the
  start. The points are thinned to one per 5 mm on the cut plane; when that
  leaves more than the limit, the spacing doubles until they fit.
- `version`: `r2004`, `r2010`, `r2013` (the start) or `r2018`.

The drawing is made from every visible layer where it stands in the scene,
without deleted points and hidden classes. A layer with an octree is read
only where its leaves touch the slab; a layer without one is read in full.
With `fill` on, and for a preview, a slab whose layers reach far beyond the
surfaces in it (stray points inside the box) is read a second and at most a
third time, to lay the grid over the surfaces alone.
The layers of the file are `OPS-POINTS` (or `OPS-POINTS-` followed by the
file name of the scan without its extension, or `OPS-POINTS-CLASS-nn` per
class), `OPS-CUT-FILL` with the solid fills,
`OPS-CUT-OUTLINE` with a closed polyline per ring of a fill, `OPS-FRAME` with
the rectangle of the box as the view sees it, and `OPS-INFO` with one line of
text: the view, where the cut plane lies, the slab thickness, the units and
the model position of drawing zero. Scans with the same file name get `~2`,
`~3` after the name of their layer, and a character a layer name cannot hold,
such as `,` `;` or `=`, becomes `_`.

The command answers with a `job_id`. While the job runs, its job and
`status.result.drawing.job` hold `operation` (`export_drawing`), `path`,
`view`, `stage`, `done`, `total`, `fraction`, `cancel_requested` and
`elapsed_seconds`, refreshed four times a second. The stages are `reading` (counted in points read),
`tracing` (the filled cut; it has no measure, so `total` is 0 and `fraction`
is `null`) and `writing` (counted in entities). The complete job has:

- `path`, `format` (`dxf` or `dwg`), `bytes`, and the `view`, `thickness` and
  `units` that were drawn; `thickness` is the depth of the slab in metres,
  which is the depth of the box where the box is shallower than what was
  asked, as the text on `OPS-INFO` says it;
- `slab_points`, the points of the scans that lie in the slab, `read_points`,
  the points read to find them, counted for every read of the slab and so
  two or three times after a second or third read, and `drawn_points`, the
  points in the drawing;
- `point_spacing` in metres, with `point_spacing_raised` true when the point
  limit made it larger than 0.005;
- `regions` and `vertices` of the filled cut, and `dropped_regions`, the
  regions left out because they are smaller than the smallest wall, 50 mm by
  0.30 m;
- `grid_cell` in metres and `direction_degrees`, the main direction the cut
  was traced along (both `null` without a fill), with `grid_cell_raised` true
  when the cell is larger than asked: the points were too sparse for it, or
  the surfaces in the slab span more than 16 million cells.

A job that fails has `error`; a slab without points fails with `the slab
holds no points`, followed by the face of the box that is the cut plane and
what to do about it, and writes nothing. With a box around a whole building
the face a vertical view cuts at often lies outside the walls. `cancel_drawing` stops the job once the
step under way has ended; the job becomes `cancelled`. The file is written to
a temporary file first, so a failed or cancelled job leaves an existing
destination as it was. Writing costs about 2.8 kB of memory per point of the
drawing: about 0.4 GB at 150,000 points and 1.1 GB at 400,000.

`preview_drawing` takes the same fields without `path`. It traces the filled
cut as `export_drawing` would draw it, whatever `fill` says, writes nothing
and lays the regions over the points in the viewport, on the cut plane. Its
complete job has the figures above without the file; `drawn_points` are the
points of the drawing that the preview also builds for the Drawing view (see
below), as an export with the same choices would write it.
The preview goes away when what it was made from changes: the section box,
the visible layers, a layer transform, the deleted points, the classes shown,
or the `view`, `thickness`, `square`, `grid` or `max_wall_thickness` of the
block. A preview that is still being made when that happens is cancelled, and
one that is ready only afterwards is not shown and ends its job as `failed`.
`clear_drawing_preview` takes the preview away and answers with `cleared`,
which is false when there was none.

`status.result.drawing` holds `settings` (the choices of the block under the
field names above; a number that cannot be read is `null`), `job` (the
running job, or `null`), `last` (how the last job ended, as its job reports
it, or `null`), `preview_shown` and `preview_regions`.

Both commands are refused for a destination that is not absolute or has
another extension, a destination folder that does not exist, a destination
that is the source file of an open layer, a value outside the limits above,
a section box that is off (`section box is not enabled`), no visible layer
(`no visible point cloud to draw`), a visible layer that is still being
imported (`a visible point cloud is still loading:` and its file name; wait
for it or hide it), and while another drawing or preview runs or the save
dialog of the window is open. A refused command changes nothing
in the block. One drawing or preview runs at a time; it can run beside a mesh
job.

## Drawing view

The Drawing view shows a 2D drawing in the main area in place of the 3D
scene, on a light sheet: points as dots in their colour or that of their
layer, filled cuts with their holes, polylines, the frame and the texts, with
a switch per layer, pan by dragging, zoom about the pointer with the wheel,
a scale bar and the coordinates under the pointer in the units of the
drawing. It shows the drawing that the last `preview_drawing` or
`export_drawing` made, without making it again, or a DXF or DWG file read by
`open_drawing`. A finished export switches to the view when **Show after
export** is on in the Section drawing block, as it is by default; a preview
only puts its drawing in the view.

`drawing_view` with `show: true` shows the view and with `show: false` the 3D
scene again; it answers with `drawing_view` as the status reports it. It is
refused while Settings is open. `open_drawing` reads an absolute `.dxf` or
`.dwg` file that exists into the view and shows it. It answers with a
`job_id`; the complete job has `path`, `units` (`mm` or `m`), `units_named`
(false for a file without units, which is read as millimetres), the numbers
of `layers`, `points`, `polylines`, `fills`, `texts` and `inserts` (block
references drawn), `skipped` (entities that are not shown, by type) and
`skipped_3d` (meshes, polyface meshes, 3D faces and solids, which are not
shown). Points, lines, polylines with their arcs, circles, arcs, ellipses,
solid fills with their holes, 2D solids, texts, multiline texts, attributes
and block references with their nested blocks are read; splines, leaders and
dimensions are drawn as the lines they consist of. Curves become polylines,
a fill with a pattern is drawn by its boundary, and an entity that is not
shown never makes the read fail. Layers that the file has switched off or
frozen start hidden. Units other than millimetres are shown in metres.

`drawing_zoom_extents` fits the whole drawing in the view and answers with
the `camera`. `set_drawing_layer` shows or hides a layer by its name, in any
case, or every layer with `*`; both fail without a drawing.

`status.result.drawing_view` holds `shown`, `show_after_export`, `reading`
(the file being read, or `null`), `error` (why the last file could not be
read, or `null`), `drawing` (`null`, or its `source`: `preview`, `export` or
`file`, its `path`, `units`, `units_named`, the totals above, `extents` in
drawing units, `inserts`, `skipped`, `skipped_3d` and `layers`, each with
`name`, `visible`, `color` and its `points`, `polylines`, `fills` and
`texts`), `camera` (`center` in drawing units and `pixels_per_unit`) and the
`viewport_size` of the sheet.

## CAD viewer

`open_in_cad_viewer` opens a DXF or DWG file in the CAD viewer, as the button
Open in CAD viewer of the Section drawing, Detect faces, Closed mesh and
Surface mesh blocks does. Without `path` it opens the last file that a section
drawing, faces export, closed mesh or mesh export wrote, through the window or
through this API; `path` is an absolute `.dxf` or `.dwg` file that exists.
The viewer is the program chosen in Settings, otherwise Open CAD Studio where
it is installed: `%ProgramFiles%\Open CAD Studio` or
`%LOCALAPPDATA%\Programs\Open CAD Studio` on Windows,
`/Applications/OpenCADStudio.app` or `~/Applications` on macOS, `/snap/bin`
on Linux, or the search path. It is started with `--read-only` and the file,
without waiting for it; a running Open CAD Studio takes the file as a further
tab. Without a viewer the file goes to the program the system has for it.
The answer has `path`, `viewer` (the program, or `null` for the system
program) and `read_only`. It is refused when no file was exported yet, for a
path that is not absolute, has another extension or does not exist, and when
the program cannot be started.

`status.result.cad_viewer` holds `path` (the program found, or `null`),
`source` (`setting`, `installed` or `system_default`), `chosen` (the program
chosen in Settings, or `null`), `chosen_missing`, `open_after_export` (whether
every DXF or DWG export opens by itself, the switch Open after export of the
blocks) and `last_export`.

## 3D BAG download

`bag3d` downloads the buildings of the Dutch 3D BAG register inside a box and
opens them as a mesh layer, as the 3D BAG panel does, which File > 3D BAG
buildings… opens. `bbox` is `[xmin, ymin, xmax, ymax]` in RD New (EPSG:28992)
with sides longer than 0 and at most 2,000 m, `lod` is `1.2`, `1.3` or `2.2`,
and `path` is an absolute `.obj` destination in a folder that exists; the
file is georeferenced in RD New + NAP and carries the attribution of the
register. The command needs an internet connection and answers with a
`job_id`. While the download runs, its job and
`status.result.bag3d` hold `page` (the pages read), `pages` (the pages the
area needs, or `null` while the service has not said), `buildings`, `lod`,
`path`, `cancel_requested` and `elapsed_seconds`; `status.result.bag3d` is
`null` when no download runs. The complete job has `buildings`, `vertices`,
`triangles`, `pages` and `path`. A page holds about fifty buildings and a
download takes at most a hundred pages, so an area with more than about 5,000
buildings fails after the first page with the number of buildings it holds;
an area whose buildings are too detailed for a mesh of one million vertices
fails after a few pages. A box outside the area of RD New and a destination
whose folder does not exist are refused before anything is asked.
`cancel_bag3d` stops the download once the request under
way has ended; the job becomes `cancelled` and an existing destination stays
as it was. Only one download runs at a time. While the extension is switched
off the command answers `extension bag3d is disabled`.

## Extensions

`list_extensions` lists the optional features built into the application in
`extensions`: for each its `id`, `name`, `version` (the version of the
application), `description`, `author`, `category`, `builtin` (always true: no
code from another source is loaded), `uses_network` and `enabled`.
`set_extension_enabled` switches one on or off and keeps that in
`extensions.json` in the configuration directory; an unknown `id` is refused.
The answer holds `id`, `enabled` and `saved`. When `saved` is false the file
could not be written: the switch holds for this session only and `save_error`
says why. Switching `bag3d` off closes its panel, stops a running download
and disables its entry in the File view.

## File view

`file_view` opens the File view over the model or closes it. With
`open: true` an optional `page` (`new`, `open`, `import`, `export`,
`workspace`, `extensions` or `about`) chooses
the page; without it the view opens on `workspace`, or keeps the page it
shows. The answer holds `file_view` with `open` and `page`, as
`status.result.file_view` does; `page` is `null` while the view is closed. The
3D BAG panel is not part of the File view and is not opened by this command.
A `page` with `open: false`, an unknown page, and opening while the Settings
dialog or the card of the Mesh to Plans wizard is open are refused. The Settings dialog is not opened or closed
through this API.
`status.result.mesh_export_pending` is true from the moment the window asks
where to save a mesh from the File view or Properties, or from the moment
`export_mesh` is accepted, until that file has been written.

## Mesh to Plans

`mesh_to_plans_view` shows the Mesh to Plans wizard or takes it away, as the
button in the MESH TO PLANS group, its tiles in the File view, **Show in
model**, **Back to wizard** and **Close** do. With `open: true` the wizard is
shown as its card over the window, on the step `step` names (`prepare`,
`mesh`, `views`, `walls`, `openings`, `rooms`, `sheet`, `site` or `result`)
or on the step it showed last; with `minimized: true` as well it is shown as
a strip above the scene instead, beside the progress lines. Opening closes the
File view. `open: false` takes the wizard away; what its steps hold stays for
the next time it is shown. A `step` or `minimized` with `open: false`, an
unknown step and opening while the Settings dialog is open are refused.

The card covers the model like the File view does: while it is shown the keys
of the model do nothing, `screenshot` is refused, a view snapshot waits and
`file_view` does not open. The strip covers nothing: the model can be turned,
measured and the section box moved while the wizard waits, and a screenshot
leaves the strip out. Escape closes Settings first, then makes the card the
strip, and closes the File view after that.

The answer holds `mesh_to_plans` as `status.result.mesh_to_plans` does:
`open`, `minimized`, `step` (the id of the step shown), `next_ready` (whether
Next may leave that step), `next_reason` (why not, or `null`) and `steps`,
for every step its `id`, `number`, English `name` and `status`: `not_run`,
`running`, `done` (run, waiting for confirmation), `confirmed`, `skipped`,
`stale` (run on scans that changed since, or with other choices) or `failed`.
It also holds `project` (the project file, or `null` before it was first
written), `project_name`, `project_folder` and `prepare`: `null` before step
0 ran, and after it `rotation_deg` and `second_direction_deg` (the main
directions of the walls, in degrees), `origin` and `peil_z` (the height of P
in the scene), `footprint_area`, `footprint_parts`, `ground_z`,
`below_above_p`, `below_points` and `below_clusters` (the stray points left
out below that height above P, which lies 2 m below the lowest floor or
lower under a sloping site, counted one by one, and the clusters of at
least 25 points among them), `grid` (the cells of the survey), `seconds`, `chosen_core`,
`chosen_rotation`, `selected` (the place of the selected level) and
`levels`: per level its `id` (`00` for the floor that is P, `01` and up
above it, `-01` and down below it, `00M` for a mezzanine, `R` for the
roof), `name`, `kind` (`Basement`, `Ground`, `Storey`, `Partial` or
`Roof`), `floor_z` in the scene, `floor_above_p`, `ceiling_above_p`,
`slab_underside_above_p` (the slab above a suspended ceiling),
`slab_thickness`, `storey_height`, `cut_height` (the cut of its plan above
the floor), `tilt_mm_per_m` (its slope along the two main directions),
`share` (the part of the footprint it covers), `is_peil`, `confidence` and
`status` (`Found` or `Edited`).

**Run this step** and **Run all automatically** start a job that runs steps
one after the other on a worker thread; Run all automatically takes every
step that is not confirmed or skipped and confirms each one as it ends. One
job runs at a time, and it waits while a section drawing, a mesh, a face
detection, a merge or an octree build is under way. Step 0, `prepare`, reads
every shown scan once (an indexed one through its index; one of more than
5,000,000 points without an index is refused until its index is built): it
finds the box around the building without the stray points far out, the main
direction of the walls, the footprint and the levels, and reads every floor
again to the millimetre. The later steps are not built yet and stand in for
their work for about a second. While a job runs, `job` in the status holds `state` (`running`), `operation`
(`mesh_to_plans`), `steps` (their ids, in order), `step` (the one under
way), `place`, `completed`, `total`, `fraction`, `confirm`,
`cancel_requested` and `elapsed_seconds`; it is `null` otherwise. `last`
holds how the last job ended: `state` (`complete`, `cancelled` or `failed`)
with `finished` (the steps that ended, each with its `id` and `seconds`), for
a complete job `seconds`, and for a failed one the `step` and its `error`.
`job_id` names the job of `job` and `wait_for_job` that reports the running
or the last job. A step that ended keeps its result when the job is
cancelled; the step under way goes back to what it was. The progress strip
above the scene has a line for the job with Cancel, and Exit cancels it;
Escape and Close leave it running.

`mesh_to_plans_action` does what a button of the wizard does on the step it
shows, whether the wizard is shown or not: `run` (Run this step), `run_all`
(Run all automatically), `confirm` (Confirm, or Confirm levels on step 0),
`skip`, `cancel`, `back` and `next`. An action whose button would be
disabled is refused with the reason, for example `next` before the step is
confirmed or `run` while a job runs. `folder` sets the absolute folder of a
new project before it is first written; by default that is
`Documents/OPS Mesh to Plans/<name>`, named after the first shown scan, or
`<name> 2` and so on when a project is there. A folder that holds another
project is refused by `run` and `run_all` with the reason, and `folder` is
refused while a job runs. `run`
and `run_all` answer with the `job_id` of the job. `resume` opens the project
in the absolute `folder`, or the project file `folder` names, as **Resume
Mesh to Plans** in the Project Browser does: on the first step that is not
confirmed or skipped, or on step 0 when that is `stale`. A file that is no
project, or one made by a newer version, is refused with the reason.

`mesh_to_plans_level` does what the page of step 0 does with a level once step
0 has run. `level` is its id as `prepare.levels` lists it: `00` for the floor
that is P, `01` and up above it, `-01` and down below it, `00M` for a
mezzanine and `R` for the roof. The level is selected; then `name`,
`cut_height` (the cut of its plan above its floor, 0.3 to 3 m) and
`floor_above_p` (where its floor goes; its ceiling and slab go along, and the
levels are numbered again) are applied when they are given, and then
`action`: `select` (nothing more, the default), `show` (**Show in model**:
the card becomes the strip, the section box takes the storey from just below
its floor to the next floor and the camera frames it; `mesh_to_plans_view`
with `open: true` puts the box back as it was), `set_peil` (a whole floor becomes P), `merge`
(the level takes the one above it in), `remove`, or `add` (a level 3 m above
the selected level or the highest floor; `level` may be left out). A level
that is moved or edited is `Edited`. While the levels are confirmed only
`select` and `show` are accepted; an unknown level, a cut outside its range,
`set_peil` on a level that is no whole floor and `merge` on the highest level
are refused.

A project is the file `project.ops-m2p.json` in its folder, written whole
through a temporary file a moment after every change once step 0 ran. It
keeps the scans with their size, time of change, transform, deleted points
and hidden classes, the frame of the building, the boxes, the NAP height of
P and the north direction when known, the status of every step with the
basis it ran on, what the survey found and the levels. Step 0 also writes
`survey/profile.csv` (the horizontal area per height) and `survey/top.png`
(the view from above) in the folder. The newest eight project files are kept
in the preferences; the Project Browser offers **Resume Mesh to Plans (step
n)** for those whose scans are all open. A resumed project whose scans
changed since step 0 ran has step 0 `stale`.

## Editing

For large clouds, `scale` returns `running: true`. Poll `status.result.scale`
for processed and total source points; it becomes `null` when the transform
finishes or is cancelled. `cancel_scale` stops the scan without applying the
new factors. Repeating Scale after a successful run reuses the exact centroid
until the set of remaining points changes.
`thin` accepts a keep percentage from 1 to 100 and runs in the background.
Poll `status.result.thin_pending`; after it becomes false, the active cloud's
`remaining` and `deleted` counts reflect the exact edit. `undo_delete` restores
the removed points without changing the source file.

## Index

While an uncached octree is built, `status.result.index_progress` reports the
source-read count and known total, then tree records handled, depth and leaf
count, with `settled` for the points that have reached their leaf of the
`total` points in the cloud. `fraction` is how far the current stage is, from
0 to 1, or `null` while the size of the source is unknown. Its `stage` is
`reading_source`, `building_tree` or `ready`, and
`cancelling` shows whether cancellation has been requested. The field becomes
`null` after the build finishes. `cancel_index` stops the build and discards
its temporary files without publishing a partial cache.

## Selection and picking

World-box selection also returns a job ID and uses the same query. Its limits
are inclusive source XYZ coordinates, independent of the viewport camera and
point budget. It selects across visible layers while respecting class filters,
the active section box and previously deleted points. Indexed layers search
intersecting octree leaves; unindexed layers stream their complete sources.
The job and `status.result.selected_points` report exact counts. For very large
selections the viewport draws a representative highlight sample rather than
uploading every selected point again; the native status line reports how many
highlights are shown.
`pick_screen` uses viewport-local pixel coordinates from the top-left corner;
`status.result.viewport_size` gives the current width and height. It first
checks the point records actually drawn in the active layer's current LOD;
when none covers the pointer, it searches the exact source through its octree
when available, or streams the source otherwise. The optional `radius` defaults to 8 pixels and
may be 1–64. The returned job contains the zero-based source ordinal, world
XYZ, RGB, intensity and classification for a hit; a miss completes with zero
points. It honors the section box, class filters and deleted-point mask.
When the displayed point spheres are larger than the requested tolerance,
picking also accepts their visible discs and chooses the frontmost curved
sphere surface where discs overlap. The source search remains exact even when
the viewport shows only a bounded LOD sample.
Selections retain their exact source-coordinate bounds. `zoom_selection` uses
those bounds to frame the points quickly even after a live transform; the
selection highlight follows the current transform. The command leaves the
section box unchanged and `status.result.selection_bounds_pending` reports
whether a bounds calculation is still running.
If an earlier Delete hid other source points, the cached bounds still apply
when its deletion mask has no ordinal in common with the current selection.
`cancel_selection` stops a running world-box or viewport-box scan; the job
becomes `cancelled` and no partial selection replaces the previous one. Escape
or Clear in the native UI also stops an in-progress scan. An in-progress point
pick is discarded when cancelled.

## Measuring

`measure` sets a finished distance or area measurement from scene coordinates,
as if its points had been picked in the viewport. It replaces the current
measurement, leaves the active tool unchanged and returns the measurement.
`distance` takes 2–256 points and `area` 3–256. `status.result.measure` is
`null` without a measurement. Otherwise it holds `mode`, `finished`, the
`points` and the `segments` lengths in order, where an area includes its
closing edge, followed by `length`, `horizontal_length` and
`height_difference` for a distance, or `area`, `plan_area` and `perimeter` for
an area. `length` and `horizontal_length` follow the polyline, the height
difference is the last point's Z minus the first point's, `area` is the true
3D area and `plan_area` the area seen from above. Values are unrounded scene
units. `status.result.measure_mode` is the measuring mode active in the
viewport: `distance`, `area` or `null`. Points picked there appear in
`measure` with `finished: false` until the measurement is finished.

## Views and annotations

A saved view holds the orbit camera (`yaw`, `pitch`, `zoom`, `pan`), the
walking camera in `walk` when it was saved while walking, the scene bounds and
viewport size the camera was relative to in `frame`, the section box in
`section` (`enabled`, `min`, `max` in model coordinates, also while it is
off, and `rotation` in degrees when the box is turned; a view without it has a
box along the axes), the `color_mode`, its `guid`, its `created` time in seconds since 1970,
`snapshot_due: true` while its snapshot is missing or older than the view,
and its `annotations`. An annotation is `{"kind":"note","point":[x,y,z],
"text":…,"guid":…,"created":…}` or `{"kind":"line","from":[x,y,z],
"to":[x,y,z]}`. The view last saved or restored is the active view:
`add_note` and `add_line` add to it, and first save the current view as
"View N" when no view is active. `status.result.views` reports the `active`
view with its annotations (or `null`), the `annotation_tool` in use in the
viewport (`note`, `line` or `null`), a half-placed annotation in `placing`
(its `kind` and picked `point`: a note that waits for its text or the start of
a line, otherwise `null`), and `snapshots_pending`: the views whose snapshot
image is still to be written. `set_annotation_tool`, `annotate_screen` and
`submit_note` drive the annotation tools as the ribbon and the viewport do:
`annotate_screen` picks the exact source point at a viewport pixel like
`pick_screen` and answers `accepted: true` when the search has started; poll
`status.result.selection_pending` until it is false and read `placing`. A snapshot is taken shortly after a view is saved or updated or
its annotations change, once the viewport has drawn the change and, for at
most about 9 seconds, has read the points of the camera and made the caps of
the cut of a mesh, and only
while the viewport shows that view: the camera, the section box, the colour
mode and the scene bounds are as the view was last saved, updated or
restored. After `set_camera`, `set_section` or another change of those,
`add_note`, `add_line` and `delete_annotation` leave the snapshot as it is
and the view keeps `snapshot_due`; `restore_camera_view` shows the view again
and takes the snapshot. A snapshot taken in a viewport of another size than
the view's `frame` rewrites `pan` and `frame` for that size. Wait for `snapshots_pending` to reach 0 before
`export_bcf` when the file should carry the newest images. `export_bcf`
writes all views of the active scan synchronously and answers with the
number of `views` and of `snapshots` in the file, and with `snapshots_due`:
the views whose snapshot is missing or older than the view.

## Screenshots

`screenshot` captures the scene part of the window, without ribbon, panels and
status bar, the way view snapshots are taken. While the Drawing view is shown
it captures the drawing instead, once the sheet has been drawn, and the
answer has `view`: `drawing`, else `model`. It first waits until the
viewport has read the points for the current camera, at most about 4
seconds; `status.result.detail_pending` is true while those points are still
being read, which starts about 0.2 seconds after the camera changed. It
waits as well, within the same time, while the caps of the cut of a mesh by the
section box are being made (`status.result.section_fill.pending`). The answer has the `width` and `height` of the PNG image in
pixels, its size in `bytes`, the `viewport_size` in logical pixels with the
window's `scale_factor`, `detail_pending` (true when the points were still
being read when the picture was taken), `section_caps_pending` (true when the
caps were still being made), the `path` it was written to or
`null`, and `png_base64`, the image as standard base64 text, when `base64`
is true. Without a `path` the image is returned as base64; with a `path` only
when `base64` is true as well. An image whose longer edge exceeds `max_edge`
(16–8192, default 1920 pixels) is scaled down. A file that exists at `path`
is replaced. The command fails while the File view, Settings or the card of
the Mesh to Plans wizard covers the viewport (`file_view` with `open: false`
returns to the model, `mesh_to_plans_view` with `minimized: true` leaves the
wizard as a strip that is not captured), and while the
window is minimised (`"the window is minimised;
restore it to take a screenshot"`); a view snapshot due meanwhile is taken
when the view is restored.

## Commands

| Command | JSON fields | Effect |
| --- | --- | --- |
| `status` | — | Lists clouds (each with `mesh`: `null`, or the `vertices`, `triangles`, `open_edges` and `components` of the mesh the layer holds; for a mesh read from a file the last two count vertices at the same position as one), active imports and decoded counts, selected/deleted counts, the current measurement, edited bounds and transforms, visibility, active layer, camera (`yaw`, `pitch`, `zoom`, `pan`, `view` and `orbit_point`, the point the orbit camera turns about or `null` for the centre of the model) and viewport size, saved views for that layer and the active view with its annotations, theme, `language` (`auto`, `en` or `nl`, as chosen), section box and the fill of its cut (`section_fill`), auto-index and 3D surface settings, index and scale progress, a running mesh, merge or 3D BAG download (`bag3d`), `mesh_export_pending`, the Section drawing tool (`drawing`: its settings, a running job, the last result and whether a preview is shown), the Closed mesh tool (`closed_mesh`: its settings, a running job and the last result), the Detect faces tool (`faces`: its settings, a running job, the last job, `export_pending` and the faces of the active layer in figures; each cloud has `faces`: `null`, or those figures), `detail_pending` while the viewport reads points for its camera, the Drawing view (`drawing_view`: whether it is shown, the drawing it holds with its layers, and its camera), whether the File view covers the model (`file_view`), the Mesh to Plans wizard (`mesh_to_plans`: whether it is shown as card or strip, its step and the status of every step), and current status text |
| `job` | `id` | Reads an export, section drawing, selection, mesh, mesh export, face detection, faces export, merge, 3D BAG download or Mesh to Plans task's state and result |
| `open` | `path` | Opens a point cloud or mesh, every supported file directly inside a folder, or the scans listed by a scan project file (`.rcp`) in the running GUI. Returns `files`, the accepted paths in opening order, with `missing` (listed scans not found) and their names in `missing_names`, `already_open` (scans skipped because they are open or loading), `errors`, and `import_ids` for the full-stream readers; `import_id` is the last of those or null. Fails when nothing can be opened |
| `cancel_import` | `id` | Cancels a running full-stream import without adding a partial layer |
| `remove` | `index` | Removes a layer from the project |
| `set_active` | `index` | Chooses the active layer |
| `set_visible` | `index`, `visible` | Shows or hides a point layer |
| `camera` | `preset` | Chooses `top`, `bottom`, `front`, `back`, `left`, `right` or `isometric` |
| `set_camera` | `yaw`, `pitch`, `zoom`, `pan`, optional `orbit_point` | Sets an exact camera view; angles are radians, zoom is 0.000001–10000, and pan is a two-number screen-pixel array. `orbit_point` `[x, y, z]` in scene coordinates is the point the camera turns about from then on and `null` the centre of the model again; left out, the orbit point stays. Rejects non-finite or out-of-range values without changing the view |
| `orbit` | `yaw`, `pitch` | Turns the orbit camera by these angles in radians (yaw within ±2π, pitch within ±π; the elevation stays within ±1.56), as a left drag in the viewport does: about the orbit point while it is in view, keeping it on its pixel and the picture around it at its scale, otherwise about the centre of the model. Returns `camera`; refused while walking |
| `pick_orbit_point` | `pointer` | Makes the drawn point nearest to the camera within 8 pixels of viewport pixel `[x, y]`, over all visible layers, the orbit point, as a double click in the viewport does; with no drawn point there the camera turns about the centre of the model again. Returns `orbit_point`, the point or `null`; refused while walking |
| `zoom_all` | — | Fits the complete model at the default isometric orientation, matching the ribbon button and `F` shortcut |
| `open_panorama` | `index`, `station` | Stands in a scanner station of a layer and shows its photos; `status.result.walk` reports the view and whether full-resolution photos are loaded |
| `set_panorama` | `yaw`, `pitch`, `field_of_view` | Turns the walking camera; yaw within ±π, pitch within ±1.55 and a horizontal field of view from 0.35 to 2.1 radians |
| `walk` | `eye`, `yaw`, `pitch` | Places the walking camera at a position in scene coordinates, looking along the heading `yaw` and elevation `pitch`; inside a station ball it shows that station's photos |
| `close_panorama` | — | Leaves the walking camera and returns to the orbit view |
| `list_camera_views` | — | Lists the saved views of the active scan with everything they hold, and the name of the `active` view |
| `save_camera_view` | optional `name` | Saves the current view of the active scan (camera, section box, colour mode) and makes it the active view; returns its `name` and `guid`. The name must be unique within that scan and 1–64 characters long; without a name the first free "View 1", "View 2", … is used (maximum 32 views per scan) |
| `update_camera_view` | `name` | Overwrites a named view with the current view, keeping its name, identifier, time and annotations, and makes it the active view |
| `rename_camera_view` | `name`, `new_name` | Renames a view of the active scan |
| `restore_camera_view` | `name` | Restores a named view of the active scan, ignoring name case: its camera, section box and colour mode. It becomes the active view and its annotations are shown; a snapshot that is due is taken |
| `delete_camera_view` | `name` | Deletes a named view of the active scan with its snapshot, ignoring name case |
| `add_note` | `point`, `text` | Adds a note of 1–240 characters at an `[x, y, z]` scene position to the active view and returns the view's `annotations`. The snapshot is renewed when the viewport shows the view, otherwise when the view is next restored |
| `add_line` | `from`, `to` | Adds a line (drawn as an arrow from the first to the second `[x, y, z]` scene position) to the active view and returns the view's `annotations` |
| `delete_annotation` | `index` | Removes the annotation at that zero-based place in the active view's list |
| `set_annotation_tool` | `tool` | Chooses the `note` or `line` tool of the viewport, or leaves it with `null`; a half-placed annotation is dropped |
| `annotate_screen` | `pointer` | Clicks at viewport pixel `[x, y]` with the active annotation tool: the picked point becomes the point of a note that waits for its text, or the start or the end of a line |
| `submit_note` | `text` | Gives the note that waits for its text its text, adds it to the active view and returns the view's `annotations` |
| `export_bcf` | `path` | Writes all views of the active scan as one BCF 2.1 file at an absolute `.bcf` path |
| `set_theme` | `theme` | Chooses and persists `forge`, `light`, `night`, `blueprint` or `contrast`; `openaec` remains an alias for Night Build |
| `set_language` | `language` | Chooses and persists the language of the user interface: `auto` for the language of the system when there is a translation for it, `en` for English or `nl` for Dutch. Returns the `language` now chosen, as `status.result.language` reports it; an unknown value is refused and changes nothing |
| `set_color` | `mode` | Chooses `rgb`, `elevation`, `intensity` or `classification` |
| `set_class_visible` | `code`, `visible` | Shows or hides one classification code in the viewport and exact selection |
| `set_point_size` | `size` | Sets point size from 0.1 to 20 |
| `set_eye_dome` | `enabled` | Enables or disables the depth-based shading pass |
| `set_eye_dome_strength` | `strength` | Sets depth-shading strength from 0 to 5; 1 is the default |
| `set_budget` | `points` | Sets visible point budget from 1,000 to 10,000,000 |
| `set_section` | `min`, `max`, optional `rotation` | Enables an XYZ section box using two three-number arrays inside the visible model bounds. With `rotation`, degrees counter-clockwise seen from above, the box `min`..`max` is turned about the vertical through its centre; it must then have a width and a length and reach the model, and may reach past it in its corners. `status.result.section` and the answer hold `min`, `max` and `rotation`; exports, selections, drawings, Closed mesh and Detect faces keep what lies inside the turned box |
| `clear_section` | — | Disables the section box |
| `set_section_fill` | optional `fill_cut`, `color`, `max_thickness` | How the cut of a mesh by the section box is filled. Where a wall, floor or ceiling has two faces opposite each other no farther apart than `max_thickness` metres (0.01 to 2, default 0.5), the material between them is closed with a flat cap of `color` (`#rrggbb`, default `#585858`, a dark grey) on the faces of the box; a single surface, a loose sheet or the open edge of a mesh gets none. In a mesh made with stations the two faces must look away from each other; where most faces on a face of the box look at one point instead, as in a closed mesh made without stations, the walls are found from the turns of air and material along the cut, and a third face inside a thin wall is left out. `fill_cut` (default true) switches it. What is left out is kept, nothing changes when a value is invalid, and the setting is kept with the display preferences. The answer and `status.result.section_fill` hold `fill_cut`, `color` and `max_thickness`, and `pending`, true while the caps are being made after a change of the box, the meshes or the fill: for a mesh of millions of triangles that takes about a second, during which the cut is shown open. `pending` turns false when they are made, also while the Drawing view or the File view covers the 3D view or the window is minimised; the 3D view shows them when it is drawn again. `screenshot` and view snapshots wait for them |
| `align_section_to_walls` | — | Turns the section box along the main direction of the walls inside it, found in the middle half of its height as the filled cut of a plan finds it; the box keeps its size and centre and turns at most 45 degrees. Answers `started`; `status.result.section_align_pending` is true while the walls are looked for, and afterwards `status.result.section.rotation` holds the new turn and `status.result.status` says the box was turned, or that no walls were found. Refused while the box is off or a search runs |
| `select_world` | `min`, `max` | Selects all exact source points in an inclusive XYZ box, returning a job ID |
| `pick_screen` | `pointer`, optional `radius` | Picks a drawn source point near viewport pixel `[x, y]` when possible, then falls back to the full source; returns a job ID |
| `cancel_selection` | — | Stops a running full-resolution box selection or point-pick source scan |
| `clear_selection` | — | Clears the current point selection |
| `measure` | `mode`, `points` | Sets a finished `distance` (polyline) or `area` (closed polygon) measurement through an array of `[x, y, z]` scene coordinates and returns its computed values |
| `clear_measure` | — | Removes the current measurement |
| `zoom_selection` | — | Frames the exact selected source points in the 3D view without changing the section box; poll `selection_bounds_pending` in status until the camera updates |
| `delete_selection` | — | Hides selected points in the open view; may first queue an octree build for LAZ |
| `undo_delete` | — | Restores the latest deletion batch |
| `redo_delete` | — | Reapplies the latest undone deletion batch |
| `thin` | `percent` | Keeps an exact percentage of the active cloud's remaining points, with Undo support |
| `translate` | `offset` | Applies three finite XYZ offsets to the active cloud view |
| `scale` | `factors` | Scales the active view around the exact centroid of remaining points; large sources stream from the disk octree in the background |
| `cancel_scale` | — | Cancels a running centroid calculation without changing the source |
| `build_index` | — | Starts an octree build for the active unindexed cloud |
| `cancel_index` | — | Cancels a running octree build without publishing a partial index |
| `set_auto_index` | `enabled` | Enables or disables automatic indexing of large clouds |
| `set_surface_settings` | optional `max_vertices`, `neighbors`, `edge_factor`, `mesh_size` | Sets the settings of the 3D surface in Properties atomically: 3–1,000,000 vertices, 3–32 neighbors, a finite positive edge factor and a mesh size of 0 or more in the units of the scan, the width of a voxel in which one point is kept before the vertices are thinned (`0`, the start, leaves the spacing to the vertices). A field that is left out keeps its value; the fields are checked together with the others and none is taken when one is refused. Returns the `surface_settings` |
| `reset_transform` | — | Restores the active cloud's source coordinates |
| `mesh` | `mode`, `path` (optional for `closed`), and for `closed` optional `voxel`, `max_hole`, `simplify_mm`, `sample_percent`, `sides`, `layers` | Starts `terrain` or `surface` reconstruction to an absolute `.obj` path, or a `closed` mesh that is shown and, with a `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc` path, also written, using undeleted points inside the active section box and visible classification filters; surface mode uses the current 3D surface settings, closed mode the Closed mesh settings with the fields given. A layer that is still loading is refused (`a point cloud is still loading:` and its file name). Returns a job ID. The complete job reports the open edges and the connected parts of the mesh, and for `closed` the distance between points and mesh |
| `set_closed_mesh_settings` | optional `voxel`, `max_hole`, `simplify_mm`, `sample_percent`, `sides`, `layers` | Sets the settings of the Closed mesh block atomically: a voxel of 0.005–0.5 m or `null` for automatic, gaps closed up to 0–3.2 m, simplification within 0–1000 mm or `null` for automatic, a share of 0.01–100 percent of the source points, sides `automatic`, `centre` or `upward`, layers `active` or `visible`. Returns the `settings` |
| `cancel_mesh` | — | Requests cancellation of the running mesh task, of whatever mode |
| `export_mesh` | `path` | Saves the mesh the active layer holds to an absolute `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc` path; the extension chooses the format. Returns a job ID |
| `set_face_settings` | optional `distance_tolerance`, `angle_tolerance`, `min_area`, `cylinders`, `layers`, `color` | Sets the settings of the Detect faces block atomically: a distance tolerance of 0.001–0.5 m, an angle tolerance of 1–45 degrees, a smallest face of 0.01–10000 m², cylinders on or off, layers `active` or `visible`, and the colouring `face` or `deviation` of the faces that are shown. Returns the `settings` |
| `detect_faces` | optional `distance_tolerance`, `angle_tolerance`, `min_area`, `cylinders`, `layers`, `color` | Finds the flat faces and the cylinders in the undeleted points inside the active section box and visible classification filters, with the Detect faces settings and the fields given, and keeps them with the active layer beside its mesh. Returns a job ID; the complete job reports the faces per type, the edges, the voxel used and the points on a face |
| `cancel_detect_faces` | — | Requests cancellation of the running face detection |
| `list_faces` | optional `boundaries` | Lists the faces of the active layer in scene coordinates with their class, plane or axis, area and residuals; with `boundaries: true` also their outlines and the edges between them |
| `select_face` | optional `id` | Highlights the face with that number in the viewport and the block and returns it; without `id` or with `null` takes the highlight off |
| `export_faces` | `path` | Saves the faces of the active layer in scene coordinates to an absolute `.json`, `.obj`, `.dxf`, `.dwg` or `.ifc` path; the extension chooses the format. Returns a job ID |
| `clear_faces` | — | Removes the faces of the active layer; `cleared` is false when it had none |
| `merge_visible` | `path` | Merges the visible LAS/LAZ layers to an absolute `.las` or `.laz` path; returns a job ID |
| `cancel_merge` | — | Requests cancellation of the running merge task |
| `bag3d` | `bbox`, `lod`, `path` | Downloads the 3D BAG buildings inside an RD New box `[xmin, ymin, xmax, ymax]` of at most 2 by 2 km at level of detail `1.2`, `1.3` or `2.2` to an absolute `.obj` path in an existing folder and opens them as a layer; returns a job ID |
| `cancel_bag3d` | — | Requests cancellation of the running 3D BAG download |
| `list_extensions` | — | Lists the built-in optional features and whether each is enabled |
| `set_extension_enabled` | `id`, `enabled` | Switches a built-in optional feature (`bag3d`) on or off and persists that; `saved` in the answer is false, with `save_error`, when it could not be persisted |
| `file_view` | `open`, optional `page` | Opens the File view, on the page `new`, `open`, `import`, `export`, `workspace`, `extensions` or `about` when one is named, or closes it and returns to the model |
| `mesh_to_plans_view` | `open`, optional `step`, `minimized` | Shows the Mesh to Plans wizard as its card, on a step when one is named, or with `minimized: true` as a strip above the scene, or takes it away; answers with `mesh_to_plans` |
| `mesh_to_plans_action` | `action`, optional `folder` | Does what a button of the Mesh to Plans wizard does on the step it shows: `run`, `run_all`, `confirm`, `skip`, `cancel`, `back` or `next`; `folder` is the absolute folder of a new project. `resume` opens the project in the absolute `folder` instead. `run` and `run_all` return a job ID; answers with `mesh_to_plans` |
| `mesh_to_plans_level` | optional `level`, `action`, `name`, `cut_height`, `floor_above_p` | Does what the page of step 0 of the Mesh to Plans wizard does with the level whose id is `level`: selects it, gives it `name`, `cut_height` (0.3 to 3 m above its floor) and `floor_above_p`, then does `action`: `select` (the default), `show`, `set_peil`, `add`, `merge` or `remove`; answers with `mesh_to_plans` |
| `export` | `path` | Exports the active source, honoring deleted points |
| `export_section` | `path` | Exports only the current section of the active source, honoring deleted points |
| `export_selection` | `path` | Exports exact selected points from the active source, including points outside the preview |
| `export_minus_selection` | `path` | Exports the active source without selected or deleted points |
| `export_drawing` | `path`, optional `view`, `thickness`, `units`, `origin`, `fill`, `square`, `grid`, `max_wall_thickness`, `color`, `point_layers`, `max_points`, `version` | Draws the slab behind one face of the section box, from every visible layer, as a 2D drawing and writes it to an absolute `.dxf` or `.dwg` path; the extension chooses the format. Returns a job ID; the complete job reports the points, the point spacing, the regions of the filled cut and the file size |
| `preview_drawing` | optional `view`, `thickness`, `units`, `origin`, `fill`, `square`, `grid`, `max_wall_thickness`, `color`, `point_layers`, `max_points`, `version` | Traces the filled cut of that slab and lays it over the points in the viewport without writing a file; returns a job ID |
| `clear_drawing_preview` | — | Takes the preview of the filled cut off the viewport |
| `cancel_drawing` | — | Requests cancellation of the running section drawing or preview |
| `drawing_view` | `show` | Shows the Drawing view in the main area in place of the 3D scene (`true`) or the 3D scene again (`false`); answers with `drawing_view` |
| `open_drawing` | `path` | Reads an absolute `.dxf` or `.dwg` file into the Drawing view and shows it; returns a job ID whose complete job reports units, layers, entities drawn and skipped |
| `drawing_zoom_extents` | — | Fits the whole drawing in the Drawing view; answers with the `camera` |
| `set_drawing_layer` | `layer`, `visible` | Shows or hides a layer of the drawing in the Drawing view by its name, or every layer with `*` |
| `open_in_cad_viewer` | optional `path` | Opens a `.dxf` or `.dwg` file, by default the last one exported, read-only in Open CAD Studio or the program chosen in Settings, else in the system program; returns `path`, `viewer` and `read_only` |
| `screenshot` | optional `path`, `base64`, `max_edge` | Captures the 3D viewport, or the drawing while the Drawing view is shown, as a PNG image: written atomically to an absolute `.png` path, replacing a file there, and/or returned as base64 in `png_base64` |

## Exports, stored settings and the server

The destination extension of a point export selects PLY, XYZ, PTS, CSV, LAS,
LAZ or E57. Export is atomic and scans the complete source rather than the
viewport sample.
`status.result.hidden_classes` lists disabled numeric classification codes.
Color mode, point size, eye-dome settings, point budget and auto-index changes
made through this API also update the native `settings.json` defaults after a
short debounce, so they remain in effect when the app restarts.
The server binds only to loopback, limits request bodies to 64 KiB, and has
no permissive browser CORS headers. After an unclean shutdown, an old
discovery file may remain until the next native launch; clients should check
`/health` and `/info` before using an entry.
