## Problem

The reconstructed surface is geometry only. For visual inspection and presentation, users need the measured appearance baked onto the mesh as an image texture, so that it resembles a photograph rather than a flat-coloured surface. E57 scans may contain both per-point colour and calibrated station photographs.

## Proposed behaviour

- Add a **Texture surface** action after surface or closed-mesh reconstruction. Offer **Point colours** and **E57 photographs** as sources; use point colours as the fallback where photos are missing or unusable.
- For point colours, project the RGB samples onto the surface with distance/normal-aware filtering, robust outlier rejection, and explicit handling for unobserved areas. Do not invent photo detail where there is no measurement.
- For E57 photographs, use each image's camera pose and pinhole calibration already parsed by the app (`scan_image.rs`). Assign visible surface patches to suitable photos, reject occluded/back-facing projections, and blend seams/exposure differences without smearing across depth discontinuities.
- Generate UV coordinates and texture atlases with adjustable texel size / maximum resolution. Save images beside the mesh and export a textured format such as glTF/GLB or OBJ+MTL+images. State clearly in the UI when the chosen CAD export cannot preserve texture images.
- Preview the textured surface in the native Rust viewport, with a toggle between texture and geometry/point colours. Report coverage and source photos used.

## Performance and reliability

- Process large clouds/photos in bounded-memory tiles. Reuse the existing spatial index; decode source images lazily and cache only a bounded set. Rasterize/project in parallel where deterministic output can be retained.
- Keep original E57 images unchanged. Cancel safely and write outputs atomically. Repeated runs with the same settings should generate the same UV assignment and pixels.

## Acceptance tests

1. A small coloured cloud produces a textured mesh whose sampled pixels match the source colours within a specified tolerance.
2. A synthetic E57 image/pose projects a known checkerboard onto the correct faces; back-facing and occluded faces do not receive it.
3. Two overlapping photos blend across their seam without doubling or ghosting a foreground edge.
4. The 46M-point E57 sample can be textured without loading all points and photos into RAM; progress, cancellation, and output size are reported.
5. The exported textured model reopens in another viewer with intact UVs and image references; gaps remain marked as unobserved or use the documented point-colour fallback.

Related: #10 (point cloud to mesh), #12 (surface reconstruction).
