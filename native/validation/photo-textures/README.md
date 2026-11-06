# Photo texture validation

Core tests exercise calibrated pinhole checkerboards, ray occlusion,
back-facing surfaces, exposure blending, colour disagreement, missing/transparent
images, survey coordinates, reflected meshes, deterministic threading and atomic
export cancellation. An additional integration test writes and reopens an
actual E57 containing a calibrated PNG checkerboard before baking it. Run `cargo test -p pointcloud-core mesh_photo` from `native/`.

`smoke.py` starts its own native window with isolated configuration and cache.
It opens one measured triangle, adds a generated panorama, bakes its pixels,
captures the actual native viewport with the texture enabled and disabled, and
checks the exported OBJ, UVs and material through the independent `trimesh` reader.
It never logs discovery tokens and cleans up its own process. Requires Python
with Pillow, NumPy and trimesh, a display/compositor, and a Vulkan-capable renderer.

```sh
python3 smoke.py /absolute/path/to/open-pointcloud-studio --output /new/output/directory
```

Use a new output directory because export deliberately refuses existing files.
`--e57 /path/to/manitouNoInvalidPoints.e57` additionally reconstructs a 300-vertex
surface from real scanner measurements and bakes the calibrated photographs.
The public fixture is the E57 committee's
[Calabi Yau Systems sample](https://e57-3d-imgfmt.sourceforge.net/data.html),
with SHA-256 `d4f06e2a80e3e54be7391130b911c82af0c6a7857cec133177431f5f578f23cf`,
1,095,702 points and five station photographs. The data and images are not
redistributed here. A reduced surface tests calibration/export functionality;
it is not the large-cloud performance acceptance test.

The first recorded Wayland/software-Vulkan run changed 57,237 viewport pixels
when toggling the baked texture, reported 10,153 photographed interior texels
with no unobserved texels, reopened with intact UV/material references in
trimesh 5.2.0 and retained the source panorama bytes. The same exported file also
reopened and rendered correctly in trimesh’s independent pyglet 1.5.31/OpenGL
viewer; its frame was visually inspected. Reproduce with `python3 viewer.py
/output/photographic.obj /output/trimesh-viewer.png` from an X11/OpenGL session
with `pyglet<2` installed.

The 6 November calibrated E57 run processed all 1,095,702 source points into
300 vertices and 890 triangles. Its atlas reports 45,335 photographed texels,
194,293 explicitly unobserved texels, four used photos and zero failed photos.
The coarse test surface intentionally leaves large unobserved regions; this
is a calibration/export check rather than a claim about reconstruction quality.
The OBJ also reopened and rendered in the independent OpenGL viewer.
`result-2026-11-06.json` records the output, safe cancellation without partial
files, unchanged source SHA-256, and peak native RSS of 310,348 KiB. Increasing
the working set from four to at most eight decoded images, still within a
64 MiB byte budget, reduced this repeated local run from 18.47 to 4.62 seconds.
OBJ, MTL and PNG bytes were identical. These are development-build sanity runs,
not a controlled comparison across hardware or workloads. Export names containing spaces were
separately reopened successfully with their MTL/PNG references.

For larger sources, `--cache-root` selects a private persistent octree cache;
prepare it with `XDG_CACHE_HOME=/private/cache binary --index source.e57`.
`--vertices`, `--texel-size`, `--max-resolution` and `--job-timeout` adjust the
probe. `--cancel-probe` additionally cancels a finer texture export and checks
that no staged or published files remain; `--expected-sha256` verifies the
original source bytes after the probe. The report includes processing times,
bounded progress samples, output counts and Linux peak resident memory.

The 199,186,802-point [Scanner2GO BLK2GO fixture](https://support.scanner2go.de/hc/de/articles/18239199476369-Download-Beispiele-BLK2GO-f%C3%BCr-Import-CAD)
passed the large-source probe on 6 November (`large-result-2026-11-06.json`).
Its complete source was sampled into 5,000 vertices and 15,238 triangles in
48.96 seconds using a private disk octree. Baking took 39.00 seconds and wrote
12,081,126 bytes of OBJ, MTL and one 4096² PNG atlas. Native peak RSS was
662,820 KiB (about 647 MiB); the separate index preparation peaked at
240,828 KiB. Preparation used PR #59's packet-reader fix and unchanged index
format; its wall clock includes compilation pauses and is not a throughput
benchmark. Progress advanced during the bake, cancellation left no partial
files, and the original E57 SHA-256 remained
`e01da70d9cc3a2c8a0cf9e3e00f76b2d96895590d12285240b8148c2eb6a5040`.

Only 3,619 interior texels came from the six station images; 3,814,645 used
stored mesh RGB and none were unobserved. This explicitly exercises fallback
and bounded ingestion/baking; it does not claim full SLAM trajectory-photo
calibration or reconstruction quality. The full input/data/images are not
redistributed.

The measured-point extension adds bounded source-cloud RGB resampling. Its
core tests check a colour ramp within three RGB levels, isolated colour outliers,
wrong surface directions, nearby parallel walls, missing measurements,
source-cloud foreground occlusion outside the mesh, deterministic atlas pixels,
and cancellation. Native tests also check edited/deleted RGB and source identity.
Run `cargo test -p pointcloud-core mesh_point_texture` and
`cargo test -p pointcloud-core measured_` from `native/`.

`smoke.py --measured-point-probe` additionally opens a generated PLY with stale
red mesh corners and a denser green RGB plane, bakes the original point colours,
checks explicitly unobserved gaps, repeats the bake for byte-identical PNGs,
captures the real native viewport and reopens the material in trimesh.

Sampling reuses the disk octree and caps the source at 262,144 points. Without
an index it uses a bounded resident preview; `point_sampling_limited` exposes
both limits. The colour search radius is four times the numeric texel-size
setting in source-coordinate metres; unsupported regions remain grey. Estimated
local normals must agree within an absolute dot product of 0.75; plane distance
is limited to one fifth of the search radius. Colours more than 48 RGB levels
from the nearest-five channel median are excluded, and at least three accepted
observations are needed. A photo's additional point-cloud depth map has at most
512×512 cells, with eight maps cached (8 MiB); empty cells make no claim.
Its visibility accuracy is limited by the angular cell size and the bounded
point sample, with a 2 cm / 1% distance tolerance. Deleted source points no longer
supply RGB, but their original geometry still occludes the original photo.
No claim of visibility from every original scan point is made.

The historical reports above used mesh-vertex fallback; rerun the probes for
the measured-point extension before comparing coverage. Preview pages have a 128 MiB budget per mesh and can be
smaller than the exported atlas pages. Original source photo pixels are never
changed.

The measured-point extension passed the isolated Wayland/Vulkan probes on
6 November (`measured-small-result-2026-11-06.json` and
`measured-large-result-2026-11-06.json`). These runs used a shared AMD Ryzen 7
3700U workstation (4 cores / 8 logical CPUs, 14,596,902,912 bytes physical RAM),
a development binary with core opt-level 2 and `RAYON_NUM_THREADS=2`, Mesa
software Vulkan and a prepared private octree for the large fixture. They are
functional and memory checks, not controlled throughput benchmarks.

| Fixture | Source points | Samples | Photo texels | Measured RGB texels | Unobserved texels |
| --- | ---: | ---: | ---: | ---: | ---: |
| Generated coloured plane | 903 | 903 | 0 | 1,800 | 756 |
| Riegl E57 | 1,095,702 | 100,000 | 43,913 | 0 | 195,715 |
| BLK2GO E57 | 199,186,802 | 262,144 | 324 | 28,363 | 275,186 |

The plane rejects stale red mesh colours, preserves the green measurements,
produces identical PNG bytes on repeated runs and changes 47,957 actual viewport
pixels when toggled against geometry. Its OBJ and the large E57 OBJ also
rendered correctly in trimesh's independent pyglet/X11 OpenGL viewer; both
frames were inspected. The standalone headless-EGL renderer was unavailable on
this machine; the successful independent rendering used X11 OpenGL.

For the large source, the same deliberately coarse 5,000-vertex / 15,238-face
surface took 43.00 seconds to reconstruct. Baking at 0.2 m texel size and 4096²
maximum resolution took 23.63 seconds and wrote 3,196,366 bytes. Native peak RSS
for reconstruction and baking together was 660,152 KiB (about 645 MiB), not
whole-machine/container memory. All six photos were used, with no failed decode.
Progress advanced, cancellation left no files, and the original E57 hash stayed
unchanged. The coarse geometry has 9,432 open edges and intentionally reports
many unsupported texels; this proves bounded ingestion/export and explicit
missing coverage, not reconstruction quality or full SLAM photo coverage.
The historical vertex-fallback run used different atlas settings and should not
be treated as a performance comparison. Neither vendor data nor photos are
redistributed.

Final validation: 1,480 workspace tests passed, none failed and 12 were ignored.
Workspace/all-target and minimal-core Clippy pass with warnings denied;
Rustfmt, probe syntax and diff checks pass.
