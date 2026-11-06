# Station panorama tabs

`smoke.py` starts one native GUI with private configuration/cache directories and
an unused API port. It opens a real E57 with calibrated station images, changes
the photo direction, switches between photo and model tabs, and closes the photo
tab. It checks both camera restorations, the retained scan and the unchanged
source SHA-256. It captures the actual native window in `station.png` and writes
`result.json`. It stops only the process it started. Tokens stay in memory.

```sh
python3 native/validation/station-photo-tabs/smoke.py \
  /absolute/path/open-pointcloud-studio /absolute/path/station.e57 \
  --output /absolute/path/results
```

A graphical display is required. For a headless Linux runner, use the same
labwc/pixman and Mesa lavapipe Vulkan environment described by
`native/ci/managed-wayland-smoke.sh` in the managed-session PR. Read-only API
requests can retry a 504 while software Vulkan initializes; mutations never retry.

The public Scanner2GO BLK2GO example is described in
`../e57-photo-export/README.md` in PR #59. A small section retaining its station
images is sufficient for this camera/tab regression. The data is downloaded
locally and is not redistributed with this test.

This covers station/photo tab ownership. General per-pane tab strips, arbitrary
split layouts and tear-off windows in issue #16 are separate work and remain open.
