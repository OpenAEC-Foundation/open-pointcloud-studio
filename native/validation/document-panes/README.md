# Native document pane validation

The optional pane probe reuses the measured triangle and calibrated checkerboard
fixture from `../photo-textures/smoke.py`:

```sh
python3 ../photo-textures/smoke.py /absolute/path/open-pointcloud-studio \
  --output /absolute/path/proof --panes-probe --windows-probe
```

Run it in an isolated compositor (see `../../packaging/managed-wayland-smoke.sh`) and
with the Python dependencies of the photographic texture probe. It verifies
independent rendered cameras by comparing the two pane interiors, opens a real
native satellite, selects the correct window for screenshots, and docks it
without changing its photo camera. `window.log`, PNGs and `result.json` retain
the evidence. The normal native tests also cover inactive decode completion,
stale decode tickets, section sharing, failed mutation atomicity and handling a
native satellite close request.

To exercise tab dragging with actual pointer events, add
`--pointer-probe /absolute/path/virtual-pointer`. This additionally requires
`wlrctl` on PATH and a single maximized labwc output at scale 1. The probe reads
its output dimensions and accounts for the compositor's server decorations.
It first merges the photo into the model pane, then drags the inactive photo tab
to the right edge and verifies that the photo, rather than the selected model,
was moved.

`virtual-pointer.c` is a small test client. Generate `pointer.h` and
`pointer-protocol.c` using `wayland-scanner` and the
[`wlr-virtual-pointer-unstable-v1.xml` protocol](https://git.sr.ht/~brocellous/wlrctl/tree/master/item/protocol/wlr-virtual-pointer-unstable-v1.xml),
then compile it against `libwayland-client`. The client requires an explicit
`WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR`; point those only at the disposable test
compositor, never at a user's desktop. It sends no input outside that compositor.

Pane layouts now save automatically through a bounded atomic writer and on a
graceful application exit. Sources reopen by reference; per-document cameras,
photo index/pinning/zoom/blend, source transforms/visibility, drawing layer
switches and the shared section are kept. Original scan bytes and unsaved point
edits are not stored in this file. Invalid or unsupported layout files are kept
and never overwritten by a fallback session.

Run `restart.py BINARY --output DIRECTORY` in the same isolated compositor to
verify two independent processes using one profile. It checks source reopening,
layout and camera restoration, photo blend, and zero changed interior pixels.
Add `--station-source /absolute/path/section.e57` to repeat with a real calibrated
station panorama. The script uses managed SIGTERM handling for a graceful close
and requires Pillow and NumPy.

Satellite placements currently have no verified platform monitor identifier,
so startup safely docks their documents into the main window. Monitor-aware
placement, linked panes, initial strip drag activation, the full 2 × 2 CAD and
annotation restart case, and complete cross-window dragging remain acceptance
work for issue #16.

The recorded 2026-11-06 run passed 785 native tests (9 explicitly ignored),
workspace/all-target Clippy with warnings denied and a full workspace build.
The real station check used the 471-point BLK2GO section fixture, SHA-256
`21b15d28bd44d66a337da20adea0e86f8858cb09dbe90acc4fc9aab00027c8a5`,
from the public [Scanner2GO BLK2GO example](https://support.scanner2go.de/hc/de/articles/18239199476369-Download-Beispiele-BLK2GO-f%C3%BCr-Import-CAD).
Its photos decoded in an inactive visible pane, both cameras restored and the
source hash stayed unchanged. Only the JSON report is checked in for that scan;
`panes.png` contains the synthetic checkerboard fixture.

The persistence follow-up passed 791 native tests (9 explicitly ignored), a
workspace build and all-target Clippy. Separate fresh-process runs restored a
synthetic route panorama and the real BLK2GO station. Both runs reported zero
changed model/photo interior pixels, retained the source SHA-256 and reopened
the saved pane tree. The route case also retained its 37% photo blend.
