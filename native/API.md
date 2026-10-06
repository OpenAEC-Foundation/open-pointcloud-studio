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
of files it started loading. Exports, section drawings, face detections and
colourings from photos return `accepted: true` and a `job_id`. Query `{"command":"job","id":"JOB_ID"}`
for a durable `running`, `complete` (with point count), or `failed` result.
The newest 32 jobs remain queryable even if the GUI status line changes.
Non-LAS/LAZ imports return an `import_id`; `status.result.imports` lists active
imports with decoded finite-point counts and cancellation state. An import
stays there until its source has been read, also while its layer shows the
points read so far and while it builds its octree in the same pass. Use
`cancel_import` with that ID to stop a long import. A cancelled import never
adds a partial layer, and the layer of the points it showed is closed. LAS/LAZ header previews open immediately and have a null
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

`mesh` with `mode: "closed"` makes a closed mesh, as the method Closed mesh
of [Mesh Pointcloud](#mesh-pointcloud) does: a surface without overlapping faces from the points inside
the section box, or from the whole layers when the box is off, closed
wherever the scan has points or a gap up to `max_hole` wide. Deleted points
and hidden classes are left out. The mesh goes to the active layer, where it
takes the place of the mesh that layer had, and is kept in the frame of that
layer, so a later `translate` or `scale` takes it along. `path` is optional:
with an absolute `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc` destination in
a folder that exists the mesh is also written there, as the scene shows it
and in the format described under Mesh export; without it the mesh is shown
only, and `export_mesh` saves it later.

The other fields are the settings of a closed mesh, its options in Mesh
Pointcloud. A field that is left out keeps what the options have, and a
field that is given is put in the options as well. `set_closed_mesh_settings` takes the same fields without starting a job
and answers with `settings`. Both check the fields together: when one is
refused, none is taken.

- `voxel`: edge of a voxel in metres, 0.005 to 0.5, or `null` for automatic,
  as the options start: 0.02 for a region up to 20 m long, 0.03 up to 60 m and
  0.05 beyond. Detail under about two voxels is lost.
- `max_hole`: gaps in the points up to this wide are closed, in metres, 0 to
  3.2 and never more than 32 voxels; 0.25 at the start. Wider openings stay
  open.
- `simplify_mm`: how far simplification may move the surface, in millimetres,
  0 (none) to 1000, or `null` for automatic, as the options start: 0.15 voxel.
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
changes nothing in the options.

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
or in the whole layers when the box is off, as the method Flat faces of Mesh
Pointcloud does. Deleted points and hidden classes are left out. The faces
are kept with the active layer as a layer of their own beside its mesh, in
the frame of that layer, so a later `translate` or `scale` takes them along;
faces the layer had are replaced. The command answers with a `job_id`.

Its fields are the settings of a detection, its options in Mesh Pointcloud.
A field that is left out keeps what the options have, and a field that is
given is put in the options as well.
`set_face_settings` takes the same fields without starting a job and answers
with `settings`. Both check the fields together: when one is refused, none is
taken.

- `distance_tolerance`: how far a point may lie from the plane of its face,
  in metres, 0.001 to 0.5; 0.02 at the start. The options show it in
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
the list of Detected faces in Properties, and answers with `selected` and that
`face`, outline included; without `id`, or with `null`, it takes the highlight
off.

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
nothing in the options. One detection runs at a time; it can run beside a mesh
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
`thinning` (a drawing of `create_drawing` only: the points it read, or kept
from an earlier read, are thinned and counted on the grid; counted in points),
`tracing` (the filled cut; it has no measure, so `total` is 0 and `fraction`
is `null`) and `writing` (counted in entities). The complete job has:

- `path`, `format` (`dxf` or `dwg`), `bytes`, and the `view`, `thickness` and
  `units` that were drawn; `thickness` is the depth of the slab in metres,
  which is the depth of the box where the box is shallower than what was
  asked, as the text on `OPS-INFO` says it;
- `slab_points`, the points of the scans that lie in the slab, `read_points`,
  the points read to find them, counted for every read of the slab and so
  two or three times after a second or third read, `reused_points`, the
  points a drawing of `create_drawing` took from memory instead of reading
  them again (0 for an export and a preview), and `drawn_points`, the
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
read, or `null`), `drawing` (`null`, or its `source`: `preview`, `export`,
`file` or `sheet` for a drawing of `create_drawing`, its `path`, `units`,
`units_named`, the totals above, `extents` in
drawing units, `inserts`, `skipped`, `skipped_3d` and `layers`, each with
`name`, `visible`, `color` and its `points`, `polylines`, `fills` and
`texts`), `camera` (`center` in drawing units and `pixels_per_unit`), the
`viewport_size` of the sheet, `crop_shown` (whether the crop region of a
drawing of `create_drawing` is drawn), `crop_selected` (whether that crop
region is selected), `remaking` (the name of a drawing being made again after
its crop region changed, or `null`) and `kept`: the `drawings` that keep the
points they read in memory, with the `points` and the `bytes` they take
together.

## Drawings of the Project Browser

`create_drawing` makes a plan, an elevation or a section as **Create 2D plan /
elevation / section…** under VIEWS in the Project Browser does. `kind` is
`plan`, `elevation` or `section`; `basis` is `model` (the whole 3D model, the
default), `section_box` (the section box while it is on) or the name of a
saved view of the active scan that has a section box, in any case. `side`
(`front`, `back`, `left` or `right`) is the side an elevation or a section
looks at, front by default. From the model a plan is cut at `height` (1.20
above the floor of the model by default) and a section at `position` along
the axis it looks along (the middle of the model by default); an elevation
takes the whole depth. `thickness` is the slab behind the cut of a plan or a
section in metres, 0.10 by default. `sample_percent`, **Points used (%)** in
the dialog, is the share of the points of the scans the drawing is made from,
0.1 to 100 and 10 by default: the same points whatever the crop region, spread
over every scan, chosen by their place in the file. The points of the drawing
are thinned from that share; the filled cut of a plan is traced from every
point, as a sparse scan needs every point for the fill of its walls, so
`slab_points` of a plan counts every point of its slab. The other settings
are those of the Section drawing block, and the drawing is made from every
visible layer.
Without `name` the drawing is named after its kind and what it was made from;
a name that a drawing of the same scans has gets a number after it. The
answer has a `job_id`; the complete job has `operation: "create_drawing"`,
the `name`, `guid` and `kind` of the drawing and the figures of a preview.
The drawing is shown in the Drawing view and listed under VIEWS by its kind.

How each drawing was made is kept beside the saved views in `drawings.json`:
its `name`, `guid`, `kind`, the `box` it was cut from (`min`, `max` and
`rotation`), the `view` (the face drawn), the slab `thickness`, the other
`settings` with `sample_percent` (100 for a drawing kept before this setting
existed) and the `sources`, the scans it was made from as the saved views
name them.

A drawing keeps the points it read in memory for this session: of every
octree leaf its slab touched, the points between its cut and the depth it
sees, every point for a plan and its share for an elevation or a section.
Made again after its crop region shrank, moved or turned over those leaves,
it is drawn from them without reading the scans (`read_points` 0), and a crop
region that grows reads only the leaves it touches for the first time.
Another cut or view depth, another share of an elevation or a section,
another layer or a layer that moved reads the slab again. A drawing keeps at
most 384 MB of points, 32 bytes each (about 12 million), and all drawings
together 1 GB; the drawing used longest ago lets go of its points first, and a
slab that holds more than that is read each time, as before. The points kept
from a scan go when it is closed. A kept point holds its place exactly as it
was read, so the drawing is the one a read makes; it has no intensity, which a
drawing does not use. The points of a leaf whose ordinals lie 2^30 or more
apart are not kept, and its slab is read each time. Deleted points and hidden classes are left out each time the drawing
is made, so they need no read either. `list_drawings` lists the drawings made from an open scan, each
with those, with its `crop` region and with `made` (true once it is made in
this session) and `shown`, and the `files` of this session: the last preview, the exports and
the opened DXF and DWG files, each with `name`, `source`, `path` and `shown`.
`show_drawing` shows a drawing by its name in any case; one that is not made
in this session yet is made again from how it was made, from its scans,
which must all be open, and the answer then has `accepted: true` and a
`job_id`. `show_drawing` with the name `3D model` shows the 3D model as a
click on its row does: the active view lets go and its annotations are
hidden, and the answer has `shown: "3D model"`. `delete_drawing` forgets a
drawing by its name. The previews,
exports and files are limited to the 16 newest; the drawings of
`create_drawing` stay, however many there are.

### Crop region, duplicates and RO

The crop region of a drawing of `create_drawing` is the face of its box as the
drawing shows it: for a plan the box along its own two horizontal axes, for an
elevation or a section its width along the view and its height. In
`list_drawings` each drawing has `crop`: its `width` and `height` in metres,
its `center` (the model X and Y for a plan; for an elevation or a section its
place along the box, measured from the model origin along the axis of the box
the drawing runs along, and its height), the `rotation` of the box in degrees,
`cut` (the height of the cut of a plan; for an elevation or a section where
the cut lies along the direction it looks, measured along the box), `depth`
(how deep the drawing sees behind the cut: the slab, and the whole box for an
elevation), `rect` (`[[left, bottom], [right, top]]` in the units and
coordinates of the drawing), the `units` and `sample_percent`, the points used.

`set_sheet_crop` changes the crop region of the drawing `name` (in any case),
or of the drawing the Drawing view shows. `rect` sets the region as a drag of
its handles leaves it, in drawing units; or give any of `width` and `height`
(about the centre, at least 0.10 m), `center`, `rotation` (plans only), `cut`
and `depth` (0.005 to 5 m for a plan or a section, whose box grows when it is
shallower; for an elevation the depth of the box), and `sample_percent`
(0.1 to 100). Only the faces of the box
in the plane of the drawing move with `rect`, `width`, `height` and `center`;
the saved views and the section box of the 3D view never change. The drawing
is made again under its name: the answer has `accepted: true`, a `job_id`, its
`name`, `guid` and the new `crop`, or `changed: false` when nothing changed.
The complete job has `operation: "set_sheet_crop"` and the `crop`. While it is
made the Drawing view keeps its camera and its layer switches, and the drawing
cannot change again until it is made. The drawing made takes the place of the
one the Drawing view holds, shown or behind the 3D scene; when the view holds
another drawing by then, or none, it is only listed as made, and the window
stays on what it shows.

`select_crop_region` with `selected: true` selects the crop region of the
drawing the Drawing view shows, as a click on its outline does: it is drawn
thicker with its handles, and the Crop region section of Properties, with its
figures and **Points used (%)**, comes first in Properties. `selected: false`
deselects it, as Escape or a click elsewhere on the sheet does; it is then a
thin line without handles. The answer has `selected` and the `crop`; it is
refused while the Drawing view shows no crop region.

`drag_crop_handle` drags a handle of the crop region of the drawing the
Drawing view shows, as the pointer does, and selects the crop region: `handle` is `left`, `right`,
`bottom`, `top`, `bottom_left`, `bottom_right`, `top_left` or `top_right`,
and `to` the point `[u, v]` of the drawing, in its units and coordinates, the
side or sides go to. The size goes in whole centimetres and is at least
0.10 m. With `release: false` the handle is held there: the region is drawn
as during a drag, with its size, and the answer has `held: true`, the `rect`
and the `width` and `height` in metres. Let go (the default), the drawing is
made again as with `set_sheet_crop`, and the answer has its `job_id`.

`duplicate_view` duplicates a row of VIEWS by its `name` in any case: a saved
view of the active scan, a drawing of `create_drawing`, or `3D model`, which
saves the current 3D view (its camera, and the section box while it is on) as
a view. `kind` (`model`, `view` or `drawing`) picks one when names are alike;
without it a saved view is taken first, then a drawing. The copy is named with
" (2)" after the name, or the next number no other name of the same scans
takes (the copy of "Plan (2)" is "Plan (3)"), is listed right below the
original, is shown and changes on its own. A drawing is copied as it is made,
without making it again; one that is not made in this session yet is made,
and the answer has `accepted: true` and a `job_id`. When it cannot be made
now, because another drawing is being made or not all its scans are open, the
answer is `ok: false` with the reason and no copy is kept. The answer has the
`kind`, `name` and `guid` of the copy.

`rotate_crop` turns what the keys R and then O turn. With `name`, or while the
Drawing view shows a plan of `create_drawing`, it turns the crop region of
that plan by `degrees`, counter-clockwise on the sheet: the box turns as far
counter-clockwise seen from above, about the vertical through the centre of
the region, and the plan is made again upright in it, the model turned the
other way (`operation: "rotate_crop"`, with a `job_id`). An elevation or a
section is refused. In the 3D view it turns the section box by `degrees` about
its centre and answers with the `section`; while walking the pointer does not
turn it, so the window turns it by a typed angle only. A section box set
otherwise while it turns (`restore_camera_view`, `set_section`, Reset box)
ends the turn and stays as set. With `apply: false` the turn
starts as RO starts it and is shown at `degrees` without being applied; it
waits for Enter, a left click, Escape or a right click in the window, or for
`rotate_crop` with no `degrees`, which applies it and answers as a turn by
`degrees` does. A turn of 0 degrees answers `changed: false`.
`status.result.turning` is
`null`, or the turn under way with its `target` (`crop_region` or
`section_box`), `degrees` and what was `typed`.

`set_browser_group` opens (`open: true`) or collapses (`open: false`) a
group of the Project Browser: `scans`, `classes`, `views` or `bcf`, a kind
under VIEWS (`3d`, `plans`, `elevations`, `sections` or `files`), or the
scans of one folder as `folder:` followed by the path of the folder as the
scans lie in it. Collapsing changes nothing that is loaded or shown, and the
window keeps the choice for the next session. `status.result.project_browser`
reports which groups are `open`, the `collapsed` groups, under `views` each
kind VIEWS lists with its `group`, whether it is `open` and the names in its
`rows`, the 3D model first, and as `shown` the name of the row that is
highlighted: the drawing the Drawing view shows, else the active view, else
`3D model`.

## View tabs

The tabs above the main area show the 3D model and the views and drawings
side by side, as the pages of a document. The tab of the 3D model comes first
and never closes; after it come the views and drawings opened from VIEWS of
the Project Browser, in the order they were opened: saved 3D views, plans,
elevations, sections, the preview, exports and opened DXF and DWG files.
Whatever is shown, by a click on a row, by `restore_view`, `show_drawing`,
`open_drawing` or `duplicate_view`, opens its tab, or makes it active when it
is open, and the row of VIEWS of the active tab is the one highlighted.
`create_drawing` opens the tab of its drawing. A view of a scan that is not the
active one, or a drawing of scans that are not open, keeps its tab, which is
listed again with its scan. The open tabs and the active one are kept for the
next session, as the collapsed groups are; the tab of a preview, an export or
a file lasts for the session, as its row does. Once the scans are read, the
tab that was active is shown again unless another was chosen first.

The 3D model keeps its own camera, section box and colour mode while a saved
view is shown in the scene, and gets them back when its tab is shown again. A
saved view that the scene still has comes back as it was left; otherwise its
tab shows it as it was saved, as a click on its row does. A drawing comes back
zoomed as it was left, with its layers as they were, also from its row.

`list_tabs` answers with `tabs`, each with its `index`, `name` (`3D model`
for the 3D model in every language, as `project_browser` names it), `kind`
(`model`, `view`, `drawing` or `file`), the `guid` of a view or drawing or the
`path` of a file (`null` for the preview), `active` and `closable`, and with
`active`, the index of the active tab or `null` while the Drawing view holds
no drawing. `status.result.view_tabs` holds the same. `show_tab` shows an open
tab as a click on it does, by its `index` or its `name` in any case; a drawing
that is not made in this session yet is made, and the answer then has
`accepted: true` and a `job_id`. It is refused while Settings, the dialog of
Create 2D or the card of the Pointcloud to Drawing wizard is open, as Ctrl+Tab does
nothing then. `close_tab` closes a tab as its × does, by
`index` or `name`; the view or drawing stays under VIEWS, and when the tab
was active the tab after it is shown, else the one before it. The tab of the
3D model does not close. Both answer with the tabs as `list_tabs` gives them,
and with `shown` or `closed`, the name of the tab.

## CAD viewer

`open_in_cad_viewer` opens a DXF or DWG file in the CAD viewer, as the button
Open in CAD viewer of the Section drawing block and of the Surface mesh and
Detected faces sections of Properties does. Without `path` it opens the last
file that a section drawing, faces export, closed mesh or mesh export wrote,
through the window or through this API; `path` is an absolute `.dxf` or `.dwg` file that exists.
The viewer is the Open CAD Studio that comes with the application:
`OpenCADStudio` (`OpenCADStudio.exe` on Windows) beside the executable of the
application, as the Windows installer, the archives and the macOS bundle put
it, or in `../lib/open-pointcloud-studio` from the folder of the executable,
as the `.deb` and the AppImage put it. A development build in
`target/PROFILE/` uses the program in `target/open-cad-studio/release/` that
`packaging/build-open-cad-studio.sh` builds. Without it the viewer is the
program chosen in Settings, otherwise Open CAD Studio where it is installed:
`%ProgramFiles%\Open CAD Studio` or `%LOCALAPPDATA%\Programs\Open CAD Studio`
on Windows, `/Applications/OpenCADStudio.app` or `~/Applications` on macOS,
`/snap/bin` on Linux, or the search path. It is started with `--read-only`
and the file, without waiting for it, and without the variables the AppImage
runtime sets (`APPIMAGE`, `APPDIR`, `ARGV0`, `OWD`): from the AppImage, Open
CAD Studio would otherwise register the program `APPIMAGE` names, this
application, as the preview program for DWG files of the desktop and, when
the user agrees, as the program for DWG and DXF files. Open CAD Studio gives
a file opened read-only a window and a process of its own, also while it is
already running. A program inside a mounted AppImage is first copied to
`open-pointcloud-studio-native/open-cad-studio/` in the cache folder
(`$XDG_CACHE_HOME`, else `~/.cache`) and started from there, so that it stays
open when the application ends. The copy is made again in the same place when
the version of the application or the size of the program changed (the file
`version` beside it names the version that made it), so that no copy of an
earlier version is left behind. Without a viewer the file goes to the program
the system has for it. The answer has `path`, `viewer` (the program that was
started, or `null` for the system program) and `read_only`. It is refused
when no file was exported yet, for a path that is not absolute, has another
extension or does not exist, and when the program cannot be started.

`status.result.cad_viewer` holds `path` (the program found, or `null`),
`source` (`bundled` for the Open CAD Studio that comes with the application,
`setting`, `installed` or `system_default`), `chosen` (the program chosen in
Settings, or `null`), `chosen_missing`, `open_after_export` (whether
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

There are two kinds of extension: the optional features built into the
application, such as `bag3d`, and installed extensions. An installed
extension is a separate program in a folder of the configuration directory,
`extensions/<id>/`, that drives the window through this API; it can add
buttons to the EXTENSIONS group of the ribbon and tiles to the New and Export
pages of the File view. [docs/extensions.md](../docs/extensions.md) describes
the folder, its `extension.json` and how to write one.

`list_extensions` lists both in `extensions`: for each its `id`, `name`,
`version`, `description`, `author`, `category`, `builtin`, `uses_network` and
`enabled`. A built-in feature has the version of the application and
`builtin: true`. An installed extension has `builtin: false`, `category`
`Installed`, its `folder`, `homepage`, `min_app_version`, the `command` it
starts on this system, what it declares it `uses` (`network`,
`files_outside_folder` and `commands`, a list or `all`), its `ribbon` buttons
and `file_view` tiles, and `run`: the run under way or `null`. `problems`
lists installed extensions that could not be read, with `id`, `folder` and
`error`. `set_extension_enabled` switches either kind on or off and keeps
that in `extensions.json` in the configuration directory; an unknown `id` is
refused. The answer holds `id`, `enabled` and `saved`. When `saved` is false
the file could not be written: the switch holds for this session only and
`save_error` says why. Switching `bag3d` off closes its panel, stops a
running download and disables its entry in the File view; switching an
installed extension off takes its buttons and tiles away and stops its run.
`extensions.json` holds `disabled`, the ids that are switched off, and
`installed`, the installed extensions with their versions; a file written
by an earlier version, a list of the ids that are switched off, is read as
well, also with a byte-order mark. A window writes only its own change into
the file as it finds it, so windows that share the configuration directory
keep each other's installs and switches.

`install_extension` takes the absolute path of an extension folder, the
`extension.json` in it, or a `.zip` archive of one. It copies the files to a
staging folder beside the installed extensions and checks them there: the
manifest, at most 1000 files and 64 MiB, no links, names that every system
keeps, paths inside the folder, icons of at most 64 KiB that refer to
nothing outside themselves, and for an archive no entry outside the folder
and no more unpacked than allowed. A source that does not fit is refused with
the reason. Otherwise the window shows what the extension declares and asks
the user to confirm, since it installs a program; the answer comes once that
dialog shows, with `confirmation: "shown"`, `extension` (what it declares)
and `replaces` (the version it replaces, or `null`). Only the user can
confirm. `status.result.extensions.dialog` reports the open dialog. An older
version than the one installed is refused; the same or a newer one replaces
it and keeps its logs and its switch.

`run_extension` starts an installed extension as a click on its button does,
with the arguments of the button or tile `entry` when one is named. Its
program runs in its folder with `OPS_API_PORT`, `OPS_API_TOKEN`,
`OPS_API_URL`, `OPS_EXTENSION_ID` and `OPS_CONTEXT`, the path of a JSON file
with what `context` reports. Its output goes to `logs/run-<time>.log` in its
folder; the newest 20 runs keep their logs. The answer holds `run`, `log`
and `context`. `status.result.extensions.running` lists the runs under way
with their `entry`, `seconds`, `percent`, `text`, `stopping`, `log` and
`pid`. An extension runs once at a time. `stop_extension` stops a run and
what it started: at once on Windows, otherwise after 1.5 seconds at the
latest. When the window closes, every run is ended.

The token in `OPS_API_TOKEN` belongs to that run alone and stops working
when it ends. With it the server accepts the commands the extension
declared in `uses.commands` of its `extension.json`, or every command when it
declared `all`, and always `show_message`, `report_progress`, `context`,
`choose_path` and `job`; another command gets HTTP 403. The token of the
discovery file accepts every command, as before.

`show_message` shows `text` (1–300 characters, on one line) in the status
bar; a message from a run starts with the name of its extension and stays
when the run ends well. `report_progress` takes `percent` (0–100) and an
optional `text` (at most 120 characters): for a run it shows beside the name
of the extension in the status bar, with a bar; otherwise in the status line
and in `status.result.extensions.progress`, until 100 is reported.
`choose_path` asks the user for a path with a dialog of the window: `mode`
`open` (an existing file), `save` (a file to write) or `folder`, with an
optional `title`, `filters` (up to 8 of `{"name", "extensions"}`, the
extensions without the dot), `file_name` and `directory`. It answers with a
`job_id`; the job is `complete` with the `path`, or `cancelled`. One such
dialog is open at a time. `context` reports what the window shows: the
`application` version and language, the `active_scan` (`index`, `path`,
`points`, `remaining`, `selected`, `bounds`) or `null`, the number of
`scans`, the `selected_points`, the `section_box` (`min`, `max`,
`rotation`) or `null`, the tab `shown` (as `list_tabs` gives it), and whether
the `drawing_view` and the `file_view` are shown; for a run also its
`extension`. A layer from a file an extension wrote is added with `open`.

## File view

`file_view` opens the File view over the model or closes it. With
`open: true` an optional `page` (`new`, `open`, `import`, `export`,
`workspace`, `extensions` or `about`) chooses
the page; without it the view opens on `workspace`, or keeps the page it
shows. The answer holds `file_view` with `open` and `page`, as
`status.result.file_view` does; `page` is `null` while the view is closed. The
3D BAG panel is not part of the File view and is not opened by this command.
A `page` with `open: false`, an unknown page, and opening while the Settings
dialog, the card of the Pointcloud to Drawing wizard or the card of Mesh Pointcloud is
open are refused. The Settings dialog is not opened or closed
through this API.
`status.result.mesh_export_pending` is true from the moment the window asks
where to save a mesh from the File view or Properties, or from the moment
`export_mesh` is accepted, until that file has been written.

## Mesh Pointcloud

The one button of the SURFACE group, **Mesh Pointcloud**, opens a card over
the window in three steps: **Method**, with the four ways to mesh as cards
(Closed mesh, Terrain mesh, 3D surface and Flat faces) and what they work on;
**Options**, the settings of that method with their defaults, explanations
and a **Use recommended** preset; and **Run**, the progress of its job with
**Cancel**, or its result in figures with **Show in model**, **Export…** and
**Back to options**. The settings stay where the commands above put them:
`set_closed_mesh_settings`, `set_surface_settings` and `set_face_settings`
set the options of the card, and `mesh` and `detect_faces` start the same
jobs as its Run button.

`mesh_wizard` shows the card or takes it away, as the button, **Close**, the
cross and Escape do. With `open: true` the card is shown on the step `step`
names (`method`, `options` or `run`) with the method `method` names
(`closed`, `terrain`, `surface` or `faces`); without them it shows the Run
step of a job of one of the methods that runs, else the step and the method
it showed last. Opening closes the File view and turns the card of Mesh to
Plans into its strip. `open: false` takes the card away; a job goes on, and
the button in the ribbon shows that it runs. A `step` or `method` with
`open: false`, an unknown step or method, and opening while the Settings
dialog is open are refused.

The card covers the model like the File view does: while it is shown the
keys of the model do nothing, `screenshot` of the scene is refused, a view
snapshot waits and `file_view` does not open; `screenshot` with
`window: true` captures the card. Escape closes Settings first, then the
card.

The answer holds `mesh_wizard` as `status.result.mesh_wizard` does: `open`,
`step`, `method`, `running` (the method whose job runs, or `null`; a terrain
mesh or 3D surface also while its save dialog is open), `run_ready` (whether
the method shown can start now), `run_reason` (why not, in the language in
use, or `null`), `recommended` (whether its options are the recommended
ones), `run_state` of the method shown (`idle`, `choosing` while the save
dialog of its OBJ file is open, `running`, `done`, `cancelled` or `failed`),
`result_scan` (the file name of the scan that holds the result of its last
job, which **Show in model** and **Export…** of the Run step act on whichever
scan is active, or `null` when no scan holds it any more or there is none)
and `scope`, what the methods work on while the card is shown, else `null`:
`active` (`name` and `points` of the active layer, or `null`),
`visible_scans` and `visible_points` (the shown layers without 3D BAG
buildings), `section` (`null` while the box is off, else its `size` in
metres and about how many points of the active layer and of the visible
layers lie inside it, `active_points` and `visible_points`, from the
overview sample of each layer) and `selected` (the selected points).

## Pointcloud to Drawing

The wizard was called Mesh to Plans while it was built; its commands, the `mesh_to_plans` part of `status` and the format of its project file keep that name, so that scripts and projects made before go on working.

`mesh_to_plans_view` shows the Pointcloud to Drawing wizard or takes it away, as the
button in the POINTCLOUD TO DRAWING group, its tiles in the File view, **Show in
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
Step 0 is `stale` as soon as one of its scans moves, loses points or has a
class hidden, or the core or the main direction chosen for it changes, and
`done` or `confirmed` again when the change is taken back; confirmed levels
stay locked meanwhile.
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
`Documents/OPS Pointcloud to Drawing/<name>`, named after the first shown scan, or
`<name> 2` and so on when a project is there. A folder that holds another
project is refused by `run` and `run_all` with the reason, and `folder` is
refused while a job runs. `run`
and `run_all` answer with the `job_id` of the job. `resume` opens the project
in the absolute `folder`, or the project file `folder` names, as **Resume
Pointcloud to Drawing** in the Project Browser does: on the first step that is not
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
through a temporary file a moment after every change once step 0 ran, and at
once when the window closes or another project is resumed. It keeps the
scans with their size, time of change, transform, deleted points and hidden
classes, the frame of the building, the boxes, the NAP height of P and the
north direction when known, the status of every step with the basis it ran
on, what the survey found and the levels. A step that runs is kept as it was
before it started. The steps after step 0 compute nothing yet: of them only
`skipped` is kept, and a result without the basis it ran on reads as
`not_run`. Step 0 also writes
`survey/profile.csv` (the horizontal area per height) and `survey/top.png`
(the view from above) in the folder. The newest eight project files are kept
in the preferences; the Project Browser offers **Resume Pointcloud to Drawing (step
n)** for those whose scans are all open, also when a scan was opened by a
path written with other separators. A resumed project whose scans changed
since step 0 ran has step 0 `stale`. Run again, step 0 keeps the levels that
were edited: each takes the place of the level it finds within 1 m of its
floor.

## Editing

For large clouds, `scale` returns `running: true`. Poll `status.result.scale`
for processed and total source points; it becomes `null` when the transform
finishes or is cancelled. `cancel_scale` stops the scan without applying the
new factors. Repeating Scale after a successful run reuses the exact centroid
until the set of remaining points changes.
`thin` accepts a keep percentage from 1 to 100 and runs in the background.
Poll `status.result.thin_pending`; after it becomes false, the active cloud's
`remaining` and `deleted` counts reflect the exact edit. `undo_delete` restores
the removed points without changing the source file. Photo colours given or
removed are an edit as well: `undo_delete` takes back the latest edit of
either kind, at most eight, and `redo_delete` does it again.

## Index

Several octree builds run at the same time. `status.result.index` has
`at_once`, how many this computer runs at once (chosen from its cores and the
memory available when the window started), `builds`, the running builds in
the order they started, and `waiting`, the layers in the queue. Each build
has `path`, `import_id` (the import that reads its source and builds the
octree in the same pass, or `null`), `stage` (`reading_source`,
`building_tree` or `ready`), `completed`, `total`, `fraction` (how far the
current stage is, from 0 to 1, or `null` while the size of the source is
unknown) and `cancelling`. A build that ends gives its place to the next
layer in the queue: those asked for with `build_index` first, then the active
layer, then the others in the order of the list. One file is never built
twice at once.

`status.result.index_progress` reports the first running build: the
source-read count and known total, then tree records handled, depth and leaf
count, with `settled` for the points that have reached their leaf of the
`total` points in the cloud, `fraction`, `stage` and `cancelling` as above.
The field is `null` while no build runs.

`build_index` starts the build of the active layer when a place is free and
otherwise puts the layer in the queue, ahead of the layers that wait to be
indexed automatically; the answer says `queued: true` then. It is refused for a layer whose octree is ready or being built.
`cancel_index` stops every running build and empties the queue, and answers
how many builds it `cancelled` and how many waiting layers it `dequeued`. A
build that reads its source for an import keeps reading and keeps the
checked cloud, without an octree. The layers open at that moment are not
indexed automatically again until `set_auto_index` switches automatic
indexing on anew; `build_index` still builds one of them. A cancelled build
discards its temporary files without publishing a partial cache.

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
`section` when it was on (`enabled: true`, `min`, `max` in model
coordinates, and `rotation` in degrees when the box is turned; a view
without it has a box along the axes), the `color_mode`, its `guid`, its `created` time in seconds since 1970,
`snapshot_due: true` while its snapshot is missing or older than the view,
and its `annotations`. An annotation is `{"kind":"note","point":[x,y,z],
"text":…,"guid":…,"created":…}` or `{"kind":"line","from":[x,y,z],
"to":[x,y,z]}`. Restoring a view puts its section box back, switched on with
its limits and its turn, and switches the box off for a view that has none
(or one saved by an earlier version while the box was off). Section boxes
that an earlier version kept under a name in `section-boxes.json` are taken
over once as views, one per box, named as the box and framing it; that file
is left as it is, and a scan may then have more views than the limit until
some are deleted. The view last saved or restored is the active view:
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
is replaced. The command fails while the File view, Settings, the card of
the Pointcloud to Drawing wizard or the card of Mesh Pointcloud covers the viewport
(`file_view` with `open: false` returns to the model, `mesh_to_plans_view`
with `minimized: true` leaves the wizard as a strip that is not captured, and
`mesh_wizard` with `open: false` takes the card away), and while the
window is minimised (`"the window is minimised;
restore it to take a screenshot"`); a view snapshot due meanwhile is taken
when the view is restored. With `window: true` the image is the whole window
as the application draws it, with ribbon, panels and status bar, also while
the File view, Settings, the Pointcloud to Drawing wizard or the card of Mesh
Pointcloud covers the model; `view` is then `window` and
`viewport_size` the size of the window in logical pixels. It is drawn by the
application itself, so it does not depend on the window being visible on a
screen.

## Photos of a file

An E57 file can hold photos that belong to no scanner station: equirectangular
panoramas taken along a walked path, cylindrical panoramas, and pinhole photos
of a camera that measured no points. A pinhole photo whose pixel size is zero
or empty states its focal length and principal point in pixels; otherwise the
focal length and pixel size are metres, as the standard says. These photos
are listed from the metadata when the file opens and marked in the scene along
their path while Stations is on. `status.result.photos.files` has per layer
the number of `photos`, how many are `pinhole`, `spherical` and `cylindrical`,
the images `skipped` (a preview without a projection, one without a pose) and
the `coordinate_system` the file states; `status.result.clouds[].photos`
counts them too.

`enter_photo` stands at the position of a photo. A panorama is looked around
with the walking camera, which `set_panorama` turns; a pinhole photo is first
seen from its own camera, with its field of view and its turn about its
viewing direction, until `set_panorama` or a drag hands the view to the
walking camera. The photo is decoded in the background, at most 4096 pixels
each way, and its neighbours along the path are read ahead: two at a time,
and one more for the photo that is entered, so that stepping on quickly does
not start a decode for every photo passed; a photo stepped past is not decoded
or kept. `status.result.photos.view` reports the `layer`, `index`, `count`,
`kind`, whether it is `pinned` to its own camera, the `zoom` of that camera,
`shown` (true once the photo is decoded), `failed` (true when it could not be
decoded; it is not asked for again until the photos are left, and its
neighbours are read ahead all the same), `shown_after_ms` (from entering it
until it could be shown) and `decode_ms`; `status.result.photos` also has the
`blend`, the photos `listing` and `decoding`, and the decoded photos kept
(`cached`, at most four, and `cached_bytes`). `pick_screen`, measuring and
annotations work on the points under the photo. `close_panorama` leaves the
photo and puts the camera back where it was before the first photo was
entered, into the station panorama it stood in then, which is decoded again;
`walk` and walking on leave the photo where it was. `screenshot` and view
snapshots wait for the photo as they wait for the points of the camera.

## Colour from photos

`colour_from_photos` gives the points of a layer the colours its photos see
them with: the photos of its file (see [Photos of a file](#photos-of-a-file))
and the pinhole photos of its scanner stations. It takes the remaining points
of the layer inside the section box and the class filters, or all of them
without a section box, and works on a worker thread in parts of at most
8,000,000 points. Without `layer` it takes the active layer when that has
photos, else the first layer with photos; a layer without an index of more
than 5,000,000 points is refused until its index is built.

A photo colours a point when nothing of the layer lies in front of it, seen
from where the photo was taken, and the point lies within `max_distance`
metres of the photo (0.5 to 500, default 20). For every photo a depth image of
about a fifth of a degree per pixel is made from the points of the layer,
read from its octree index at a detail that follows their distance to the
photo; a point further than its pixel says by more than 5 cm, or more for a
point further away, is hidden. A point takes the colour of the nearest photo
that sees it, a pinhole photo seeing what lies in its middle better than what
lies at its edges; with `blend` (the default) every photo that sees it adds
to its colour, weighted strongly to the nearest, which evens out the exposure
of photos taken one after another and a small error in their poses. The
settings left out keep what the Properties block has, and those given stay
in it.

The colours take the place of those of the file: the viewport draws them,
`color_mode` becomes `RGB`, and `export` and the other exports of a layer
write them (an E57 then holds one scan of coordinates, colours and
intensity; points that no photo saw are written black when the file had no
colours). The file itself is not changed. The colours of a later colouring are
laid over those of an earlier one. They are one edit: `undo_delete` takes them
back and `redo_delete` gives them again; `clear_photo_colours` takes the photo
colours of a layer away, as an edit too. Undo keeps at most 1 GiB of photo
colours that the layers no longer show, counting a block of colours that
several of them share once; beyond that a new colouring lets go of the oldest
edits, always keeping the newest, and the status text says how many. Closing
the window cancels a running job.

The running job, which `status.result.colour_from_photos.job` holds as well,
has `state` (`running`), `operation` (`colour_from_photos`), `source`, `stage`
(`loading` for a layer without an index read into memory, `reading` and
`photos`), `part` and `parts`, `completed` and `total` (points read or photos
done), `fraction`, `cancel_requested` and `elapsed_seconds`. The complete job
has `region` (`section_box` or `layer`), `points`, `coloured`, `unseen` and
`unseen_share` (no photo sees them), `photos` (the photos within reach of the
points), `photos_used` (that coloured a point best), `photos_failed` (that
could not be read or decoded, and were left out), `parts`, `max_distance`,
`blend`, `seconds`, `times` in seconds (`reading`, `photos`, and `decoding`,
`depth` and `colouring` added up per photo), `source`, `kept` (false when
nothing was seen or the layer was closed while the job ran) and `compared`:
for the points that had a colour and were seen, the photo colours against
those colours per channel R, G and B, as `mean_difference` (photo minus
stored), `mean_abs_difference` and `median_abs_difference`, or `null` when no
point had a colour. `cancel_colour_from_photos` stops the job after the step
under way; the colours stay as they were. `status.result.colour_from_photos`
holds the `settings` (`max_distance`, `blend`), the running `job` and the
`last` job; `status.result.clouds[].photo_colours` counts the points of a
layer with photo colours.

## Commands

| Command | JSON fields | Effect |
| --- | --- | --- |
| `status` | — | Lists clouds (each with `mesh`: `null`, or the `vertices`, `triangles`, `open_edges` and `components` of the mesh the layer holds; for a mesh read from a file the last two count vertices at the same position as one), active imports and decoded counts, selected/deleted counts, the current measurement, edited bounds and transforms, visibility, active layer, camera (`yaw`, `pitch`, `zoom`, `pan`, `view` and `orbit_point`, the point the orbit camera turns about or `null` for the centre of the model) and viewport size, saved views for that layer and the active view with its annotations, theme, `language` (`auto`, `en` or `nl`, as chosen), section box and the fill of its cut (`section_fill`), auto-index and 3D surface settings, the running and waiting octree builds (`index`), index and scale progress, a running mesh, merge or 3D BAG download (`bag3d`), `mesh_export_pending`, the Section drawing tool (`drawing`: its settings, a running job, the last result and whether a preview is shown), the Closed mesh tool (`closed_mesh`: its settings, a running job and the last result), the Detect faces tool (`faces`: its settings, a running job, the last job, `export_pending` and the faces of the active layer in figures; each cloud has `faces`: `null`, or those figures), `detail_pending` while the viewport reads points for its camera (each cloud has `view_sample`, the points of its set for the view, and `focus_sample`, the points read inside the section box that are kept besides them; these are drawn while the box is on and put aside once it is off and the set was read for the view), the Drawing view (`drawing_view`: whether it is shown, the drawing it holds with its layers, and its camera), the groups of the Project Browser and what VIEWS lists (`project_browser`), the tabs above the main area (`view_tabs`, as `list_tabs` gives them), a turn started with R and then O (`turning`), whether the File view covers the model (`file_view`), the Pointcloud to Drawing wizard (`mesh_to_plans`: whether it is shown as card or strip, its step and the status of every step), the card of Mesh Pointcloud (`mesh_wizard`: whether it is shown, its step and method, the method whose job runs and what the methods work on; see [Mesh Pointcloud](#mesh-pointcloud)), the photos of the files and the one that is entered (`photos`, see [Photos of a file](#photos-of-a-file)), the Colour from photos tool (`colour_from_photos`: its settings, a running job and the last job; each cloud has `photo_colours`, the points with photo colours), the runs of extensions, a progress reported outside a run and the dialog about an install or an uninstall (`extensions`, see [Extensions](#extensions)), and current status text |
| `job` | `id` | Reads an export, section drawing, selection, mesh, mesh export, face detection, faces export, colouring from photos, merge, 3D BAG download, `choose_path` dialog or Pointcloud to Drawing task's state and result |
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
| `close_panorama` | — | Leaves the walking camera and returns to the orbit view; while a photo of a file is entered, leaves it and puts the camera back where it was before the first photo was entered |
| `list_photos` | optional `layer` | Lists the photos of a layer that are not the pinhole photos of its scanner stations: equirectangular (`spherical`) and `cylindrical` panoramas and `pinhole` photos taken along a path. Each has its `index` (the order of the file, which is the order of the path), `kind`, `name` or `null`, `width` and `height` in pixels, `position` and viewing `direction` in scene coordinates with the layer's move and scale applied, and `station` when the file names one. The answer has the `layer`, its `path`, the `coordinate_system` the file states (such as an EPSG code) or `null`, and how many images were `skipped`. Without `layer` the active layer is taken when it has photos, else the first layer with photos |
| `enter_photo` | `index`, optional `layer` | Stands where a photo was taken and lays the photo over the points; answers with the `photo` as `list_photos` gives it and the `walk` camera. See [Photos of a file](#photos-of-a-file) |
| `photo_blend` | `value` | How much of the entered photo covers the points, from 0 (the points only) to 1 (the photo only); it stays for later photos |
| `next_photo` | — | Enters the next photo along the path of the entered photo; refused at the last photo and while no photo is entered |
| `previous_photo` | — | Enters the previous photo along the path; refused at the first photo and while no photo is entered |
| `colour_from_photos` | optional `layer`, `max_distance`, `blend` | Gives the remaining points of a layer in the section box and class filters, or all of them, the colours its photos see them with; returns a job ID. See [Colour from photos](#colour-from-photos) |
| `cancel_colour_from_photos` | — | Stops the running colouring from photos; the colours stay as they were |
| `clear_photo_colours` | optional `layer` | Takes the photo colours of a layer, by default the active one, away as an edit `undo_delete` takes back |
| `list_camera_views` | — | Lists the saved views of the active scan with everything they hold, and the name of the `active` view |
| `save_camera_view` | optional `name` | Saves the current view of the active scan (camera, the section box while it is on, colour mode) and makes it the active view, showing the 3D scene when the Drawing view or the File view was in front; returns its `name` and `guid`. The name must be unique within that scan and 1–64 characters long; without a name the first free "View 1", "View 2", … is used (maximum 64 views per scan) |
| `update_camera_view` | `name` | Overwrites a named view with the current 3D view, keeping its name, identifier, time and annotations, and makes it the active view, showing the 3D scene when the Drawing view or the File view was in front |
| `rename_camera_view` | `name`, `new_name` | Renames a view of the active scan |
| `restore_camera_view` | `name` | Restores a named view of the active scan, ignoring name case: its camera, its section box (switched off when the view has none) and colour mode, in the 3D scene also when a drawing was shown. It becomes the active view and its annotations are shown; a snapshot that is due is taken |
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
| `undo_delete` | — | Takes back the latest edit: restores the latest deletion batch, or gives a layer back the colours it had before its latest photo colours were given or removed |
| `redo_delete` | — | Does the latest edit that was taken back again |
| `thin` | `percent` | Keeps an exact percentage of the active cloud's remaining points, with Undo support |
| `translate` | `offset` | Applies three finite XYZ offsets to the active cloud view |
| `scale` | `factors` | Scales the active view around the exact centroid of remaining points; large sources stream from the disk octree in the background |
| `cancel_scale` | — | Cancels a running centroid calculation without changing the source |
| `build_index` | — | Starts an octree build for the active unindexed cloud, or queues it ahead of the automatic builds while as many builds run as the computer takes at once |
| `cancel_index` | — | Cancels every running octree build without publishing a partial index, and empties the queue |
| `set_auto_index` | `enabled` | Enables or disables automatic indexing of large clouds |
| `set_surface_settings` | optional `max_vertices`, `neighbors`, `edge_factor`, `mesh_size` | Sets the settings of the 3D surface, its options in Mesh Pointcloud, atomically: 3–1,000,000 vertices, 3–32 neighbors, a finite positive edge factor and a mesh size of 0 or more in the units of the scan, the width of a voxel in which one point is kept before the vertices are thinned (`0`, the start, leaves the spacing to the vertices). A field that is left out keeps its value; the fields are checked together with the others and none is taken when one is refused. Returns the `surface_settings` |
| `reset_transform` | — | Restores the active cloud's source coordinates |
| `mesh` | `mode`, `path` (optional for `closed`), and for `closed` optional `voxel`, `max_hole`, `simplify_mm`, `sample_percent`, `sides`, `layers` | Starts `terrain` or `surface` reconstruction to an absolute `.obj` path, or a `closed` mesh that is shown and, with a `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc` path, also written, using undeleted points inside the active section box and visible classification filters; surface mode uses the current 3D surface settings, closed mode the settings of a closed mesh with the fields given; both are the options of those methods in Mesh Pointcloud. A layer that is still loading is refused (`a point cloud is still loading:` and its file name). Returns a job ID. The complete job reports the open edges and the connected parts of the mesh, and for `closed` the distance between points and mesh |
| `set_closed_mesh_settings` | optional `voxel`, `max_hole`, `simplify_mm`, `sample_percent`, `sides`, `layers` | Sets the settings of a closed mesh, its options in Mesh Pointcloud, atomically: a voxel of 0.005–0.5 m or `null` for automatic, gaps closed up to 0–3.2 m, simplification within 0–1000 mm or `null` for automatic, a share of 0.01–100 percent of the source points, sides `automatic`, `centre` or `upward`, layers `active` or `visible`. Returns the `settings` |
| `cancel_mesh` | — | Requests cancellation of the running mesh task, of whatever mode |
| `export_mesh` | `path` | Saves the mesh the active layer holds to an absolute `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc` path; the extension chooses the format. Returns a job ID |
| `set_face_settings` | optional `distance_tolerance`, `angle_tolerance`, `min_area`, `cylinders`, `layers`, `color` | Sets the settings of a face detection, the options of Flat faces in Mesh Pointcloud, atomically: a distance tolerance of 0.001–0.5 m, an angle tolerance of 1–45 degrees, a smallest face of 0.01–10000 m², cylinders on or off, layers `active` or `visible`, and the colouring `face` or `deviation` of the faces that are shown. Returns the `settings` |
| `detect_faces` | optional `distance_tolerance`, `angle_tolerance`, `min_area`, `cylinders`, `layers`, `color` | Finds the flat faces and the cylinders in the undeleted points inside the active section box and visible classification filters, with the settings of a face detection and the fields given, and keeps them with the active layer beside its mesh. Returns a job ID; the complete job reports the faces per type, the edges, the voxel used and the points on a face |
| `cancel_detect_faces` | — | Requests cancellation of the running face detection |
| `list_faces` | optional `boundaries` | Lists the faces of the active layer in scene coordinates with their class, plane or axis, area and residuals; with `boundaries: true` also their outlines and the edges between them |
| `select_face` | optional `id` | Highlights the face with that number in the viewport and in the list of Properties and returns it; without `id` or with `null` takes the highlight off |
| `export_faces` | `path` | Saves the faces of the active layer in scene coordinates to an absolute `.json`, `.obj`, `.dxf`, `.dwg` or `.ifc` path; the extension chooses the format. Returns a job ID |
| `clear_faces` | — | Removes the faces of the active layer; `cleared` is false when it had none |
| `merge_visible` | `path` | Merges the visible LAS/LAZ layers to an absolute `.las` or `.laz` path; returns a job ID |
| `cancel_merge` | — | Requests cancellation of the running merge task |
| `bag3d` | `bbox`, `lod`, `path` | Downloads the 3D BAG buildings inside an RD New box `[xmin, ymin, xmax, ymax]` of at most 2 by 2 km at level of detail `1.2`, `1.3` or `2.2` to an absolute `.obj` path in an existing folder and opens them as a layer; returns a job ID |
| `cancel_bag3d` | — | Requests cancellation of the running 3D BAG download |
| `list_extensions` | — | Lists the built-in optional features and the installed extensions with what each declares, whether it is enabled and its run under way; `problems` lists installed extensions that could not be read. See [Extensions](#extensions) |
| `set_extension_enabled` | `id`, `enabled` | Switches a built-in optional feature (`bag3d`) or an installed extension on or off and persists that; `saved` in the answer is false, with `save_error`, when it could not be persisted |
| `install_extension` | `path` | Copies and checks the extension in an absolute folder, `extension.json` or `.zip` and asks the user in the window to confirm its install; answers when the dialog shows, with what the extension declares |
| `run_extension` | `id`, optional `entry` | Starts an installed extension with the arguments of its button or tile `entry`; returns `run`, `log` and `context` |
| `stop_extension` | `id` | Stops the run of an extension and what it started |
| `show_message` | `text` | Shows a message of 1–300 characters in the status bar, after the name of the extension that sends it |
| `report_progress` | `percent`, optional `text` | Shows how far a task is (0–100) in the status bar |
| `choose_path` | `mode`, optional `title`, `filters`, `file_name`, `directory` | Asks the user for a file to `open`, a file to `save` or a `folder` with a dialog of the window; returns a job ID whose complete job holds the `path` |
| `context` | — | What the window shows: the active scan, the selection, the section box and the tab shown |
| `file_view` | `open`, optional `page` | Opens the File view, on the page `new`, `open`, `import`, `export`, `workspace`, `extensions` or `about` when one is named, or closes it and returns to the model |
| `mesh_wizard` | `open`, optional `step`, `method` | Shows the card of Mesh Pointcloud on a step (`method`, `options` or `run`) with a method (`closed`, `terrain`, `surface` or `faces`), or takes it away; a job goes on. With `open: true` and no `step` the card shows the Run step of a job that runs. Answers with `mesh_wizard` |
| `mesh_to_plans_view` | `open`, optional `step`, `minimized` | Shows the Pointcloud to Drawing wizard as its card, on a step when one is named, or with `minimized: true` as a strip above the scene, or takes it away; answers with `mesh_to_plans` |
| `mesh_to_plans_action` | `action`, optional `folder` | Does what a button of the Pointcloud to Drawing wizard does on the step it shows: `run`, `run_all`, `confirm`, `skip`, `cancel`, `back` or `next`; `folder` is the absolute folder of a new project. `resume` opens the project in the absolute `folder` instead. `run` and `run_all` return a job ID; answers with `mesh_to_plans` |
| `mesh_to_plans_level` | optional `level`, `action`, `name`, `cut_height`, `floor_above_p` | Does what the page of step 0 of the Pointcloud to Drawing wizard does with the level whose id is `level`: selects it, gives it `name`, `cut_height` (0.3 to 3 m above its floor) and `floor_above_p`, then does `action`: `select` (the default), `show`, `set_peil`, `add`, `merge` or `remove`; answers with `mesh_to_plans` |
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
| `create_drawing` | `kind`, optional `basis`, `side`, `height`, `position`, `thickness`, `sample_percent`, `name` | Makes a plan, an elevation or a section as Create 2D plan / elevation / section does, shows it and keeps how it was made; returns a job ID. See [Drawings of the Project Browser](#drawings-of-the-project-browser) |
| `list_drawings` | — | Lists the drawings of `create_drawing` made from an open scan with how each was made, and the previews, exports and files of this session |
| `show_drawing` | `name` | Shows a drawing of `create_drawing`, made again from how it was made when it is not made in this session yet (then with a job ID); `3D model` shows the 3D model as a click on its row does, letting go of the active view |
| `delete_drawing` | `name` | Forgets a drawing of `create_drawing` with how it was made |
| `set_browser_group` | `group`, `open` | Opens or collapses a group of the Project Browser; the window keeps the choice |
| `list_tabs` | — | The tabs above the main area in their order, the 3D model first, and the active one |
| `show_tab` | `name` or `index` | Shows an open tab as a click on it does; a drawing not made in this session yet is made, with a job ID. Refused while Settings, the dialog of Create 2D or the card of the Pointcloud to Drawing wizard is open |
| `close_tab` | `name` or `index` | Closes a tab as its × does; its view or drawing stays, and the tab of the 3D model does not close |
| `set_sheet_crop` | optional `name`, `rect`, `width`, `height`, `center`, `rotation`, `cut`, `depth`, `sample_percent` | Sets the crop region of a drawing of `create_drawing` and makes it again in place, from the points it read before as long as its cut, depth and points used stay; returns a job ID. See [Crop region, duplicates and RO](#crop-region-duplicates-and-ro) |
| `select_crop_region` | `selected` | Selects (`true`) or deselects (`false`) the crop region of the drawing shown, as a click on its outline or Escape does; Properties shows its figures while it is selected |
| `drag_crop_handle` | `handle`, `to`, optional `release` | Drags a handle of the crop region of the drawing shown to a point of the drawing, as the pointer does; held with `release: false`, else the drawing is made again (job ID) |
| `duplicate_view` | `name`, optional `kind` | Duplicates the 3D model, a saved view or a drawing under VIEWS, right below it, and shows the copy |
| `rotate_crop` | optional `name`, `degrees`, `apply` | Turns the crop region of a plan, or the section box in the 3D view, as the keys R and then O do |
| `drawing_zoom_extents` | — | Fits the whole drawing in the Drawing view; answers with the `camera` |
| `set_drawing_layer` | `layer`, `visible` | Shows or hides a layer of the drawing in the Drawing view by its name, or every layer with `*` |
| `open_in_cad_viewer` | optional `path` | Opens a `.dxf` or `.dwg` file, by default the last one exported, read-only in the Open CAD Studio that comes with the application, else in the program chosen in Settings or an installed Open CAD Studio, else in the system program; returns `path`, `viewer` and `read_only` |
| `screenshot` | optional `path`, `base64`, `max_edge`, `window` | Captures the 3D viewport, or the drawing while the Drawing view is shown, or with `window: true` the whole window, as a PNG image: written atomically to an absolute `.png` path, replacing a file there, and/or returned as base64 in `png_base64` |

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
