# Autodesk ReCap `.rcp` / `.rcs` — what is known

Reverse-engineered from files produced by Leica Cyclone REGISTER 360 → ReCap
(2023–2025), validated where stated against the E57 export of the same scan.
Analysis for interoperability only; no Autodesk code or documentation was used.

Status: **`.rcp` fully readable. `.rcs` container and node table decoded;
point encoding not decoded.** Do not claim `.rcs` support in the app.

## `.rcp` — project file

A ZIP archive (`PK\x03\x04`, deflate) containing a single XML file named by
GUID. Fully implemented in `src/engine/pointcloud/RCPParser.ts`.

Relevant structure:

```xml
<Autodesk Version="1.0">
  <ProjectMetaData MajorVersion="1" MinorVersion="6" .../>
  <Project ver="1.0.0" app="Autodesk ReCap" cache="...Support\Temporary Cache Files">
    <ShotInfo name="Rijksstraatweg 68-1" id="<guid>" PCPid="{…}"
              path="…\Rijksstraatweg 68-1.rcc" rawScanPath="…\Rijksstraatweg 68-1.rcs">
      <tform>
        <T x=".." y=".." z=".."/>
        <R xx=".." xy=".." xz=".." yx=".." yy=".." yz=".." zx=".." zy=".." zz=".."/>
      </tform>
      <ScanImportSettings> … NoiseFilter, RangeClip, IntensityClip, upDirection … </ScanImportSettings>
    </ShotInfo>
    …
  </Project>
</Autodesk>
```

`tform` is the scan's registration: translation `T` and a 3×3 rotation `R`
(row-major `xx xy xz / yx yy yz / zx zy zz`). Paths are absolute Windows paths
from the machine that saved the project; only the file name is reliable.

## `.rcs` — scan file

Little-endian throughout.

### Header (offset 0)

| offset | type | meaning |
|---|---|---|
| 0 | `char[5]` | magic `ADOCT` (AutoDesk OCTree), 3 bytes padding |
| 8 | u32 | 3 (version?) |
| 12 | u32 | 2 |
| 16 | f64×3 | scan pose translation — **identical to the E57 `pose/translation`** of the same scan |
| 40 | f64×3 | small values, third ≈ 0.4355 in one file, 33.78 in another; unknown |
| 64 | f64×3 | 1, 1, 1 (scale) |
| 88 | f64×3 | 0, 0, 0 |
| 112 | f64×6 | octree cube: min xyz, max xyz (±72 or ±144; a power-of-two-ish cube) |
| 160 | f64×6 | data bounds: min xyz, max xyz, scanner-local metres — matches ReCap's clipped extent, tighter than the E57 `cartesianBounds` |
| 208 | u32 | 163 in both files |
| 212 | u32 | 134 / 136 |
| 216 | u32 | **point count** |
| 220 | u32 | 0 |
| 224 | | 3 bytes `01 01 01`, then `{guid}` string: the `PCPid` of the scan in the `.rcp` |
| 297 | u32 | section count `N` |
| 301 | `N × { u64 tag, u64 offset, u64 length }` | section directory; sections are contiguous and end exactly at EOF |

### Sections

| tag | content | evidence |
|---|---|---|
| 2 | small metadata, ~78 % zero bytes | |
| 3 | small metadata, starts `01 00 00 00` | |
| 1 | **point data**, ~91 % of the file | entropy 4.6–5.8 bits/byte: packed, not entropy-coded |
| 6 | **node table**: u32 count, then count × 80-byte records | exact size match in both files |
| 7 | per-point sidecar stream, 0.6–1.4 bytes/point, small values; begins with node count | not a table (no stride fits) |
| 11 | tiny trailer | |
| 5 | JPEG preview (`ff d8 ff e0`), present in one file | |

### Node record (section 6, 80 bytes)

| offset | type | meaning |
|---|---|---|
| 0 | u64 | 0 |
| 8 | f32×4 | **range-image window**: column start, row start, column end, row end. Extents match the E57 `indexBounds` (5083 columns, 2082 rows) exactly |
| 24 | f64×3 | xyz minimum, scanner-local |
| 48 | f64×3 | xyz maximum |
| 72 | u64 | 0 |

Node 0 covers rows 4–855 across all columns (the ceiling); the last node is a
37×25-cell floor patch. Windows overlap; the table is not a strict tree and
holds no offsets or counts.

### Point data (section 1) — partially understood

The section is not in node-table order and contains at least two layouts.

Over most of the file, 16-byte records at a byte phase that varies per
region (find it by scoring `r,g,b` plausibility):

```
u32  key      slowly varying, only a few distinct values per region (14846, 14897 …);
              NOT a per-point range — a cell/voxel identifier is the best fit
u64  packed   four 18-bit fields at bits 0, 18, 36, 54, each < 1024 in practice;
              on a flat ceiling field 2 is constant to ±1 (≈3 mm) while 0 and 1
              spread — consistent with (x, y, z) offsets in ~1 mm inside the cell
u8   intensity  0–255
u8×3 r, g, b   matches the E57 colours of the same surface
```

Near the start of the section the data is 4-byte `(u16, u16)` pairs where the
first value increases and resets (runs of 7–243); this is probably an index or
LOD layer, not points. The second `.rcs` (6.5 bytes/point) is more compact
still and was not analysed.

What was tried and failed (each against the E57 of the same scan):

- **u32 as range in 0.1 mm** — keys `(range, r, g, b)` unique on both sides
  give 564 correspondences where thousands are expected, with intensity
  agreeing at chance level (12.8 %). Not a range.
- **u32 as a cell/voxel key** — over the whole section the value changes on
  almost every record (6.7 M records, 6.2 M runs); it was only constant on
  one flat ceiling patch. Not a contiguous cell key.
- **u32 as linear z** — an affine map fitted to the ceiling and floor peaks
  predicts none of the smaller planes (errors 350–1350 units). The low 16
  bits peak at `0x0801` on 11 % of records: bit flags, not a coordinate.
- **packed fields as window offsets** `(c0 + fa, r0 + fb)` — values reach
  ~560 while most windows are < 240 wide; no node matches.
- **fixed-size block structures** in sections 1 and 7 — every "exact"
  landing is an artefact of zero-count blocks.

What was established on the way: the ReCap grid is the E57 grid with the
column axis **mirrored**, `col_e57 ≈ (K − col_rcs) mod 5084` with
K ≈ 4200–4220, and rows offset by about +17. Measured by finding, for each
node's stored xyz box, the E57 cells inside it and comparing their row/column
extents with the node window (spans match; node 1: window 178 wide, E57 185).

### How to finish it

Black-box statistics against a 10-million-point scan have been exhausted;
every plausible reading of the 16-byte record has been falsified. The
efficient route is a **controlled specimen**:

1. In ReCap, import a tiny synthetic point cloud with known coordinates
   (an E57 or PTS of 8 points at round numbers, e.g. the corners of a 1 m
   cube, then 64 points on a grid, then one with distinct colours) and keep
   the resulting `.rcs`. With a handful of records, the packed fields can be
   read off directly against the known xyz, and the u32 and section 7
   explained by elimination.
2. Repeat with the same cloud translated and scaled, to separate quantisation
   from offset.
3. Only then return to real scans to confirm the node → record grouping.

Section 7 remains unexplained (0.6–1.4 bytes per point, small values,
begins with the node count, no fixed-stride table).

Specimens: `Z:\02_automatisering\60 3D scans\2004 Renovatie Boskoop\Warmoeskade 2- 047.rcs`
with `4vis\Warmoeskade2\Warmoeskade 2- 047.e57`, and
`2461 Rijksstraatweg 68 Dordrecht\4visualization_e57-en-recap_*` (32 scans in one E57).
Probe scripts used for this analysis are not in the repo; the findings above
are the deliverable.
