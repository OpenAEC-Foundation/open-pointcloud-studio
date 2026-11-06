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
placement, linked panes and complete cross-window dragging remain acceptance
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

The four-pane follow-up can be reproduced with `restart.py BINARY --output
DIRECTORY --drawings`. It creates a model/route-photo/plan/elevation grid,
hides the plan frame while retaining the elevation frame, adds a text annotation
to each drawing while the model is active, and restarts the process. The recorded
run restored both drawing cameras and distinct layer switches, annotations,
photo camera/blend and the full pane tree. Every compared pane interior had zero
changed pixels; adding the annotations changed 1,673 plan pixels and 657 elevation
pixels. `four-panes.png` contains only the synthetic fixture.

The CAD follow-up also keeps first-fit drawing cameras when input returns to the
root tool and refuses to capture a previous document's camera during an
asynchronous drawing open. Without that identity check, reopening a drawing after
a photo could overwrite the photo's restored walk camera.

The initial shared document strip now starts the same document drag on a press,
without a prior split-button or API action. The pointer probe checks this first,
then merges the photo into the model pane and drags that inactive photo back to
an edge. A widget hover update settles the tooltip/scrollable layout before the
press; a single-document strip disables the split button.

Sheets use a separate paper renderer/camera rather than the last drawing canvas.
Run `restart.py BINARY --output DIRECTORY --sheets` to include two different
paper sizes with placed drawings and paper annotations. The lower comparison
regions named `plan` and `elevation` contain A4 portrait and A3 landscape sheets
in this run. Both paper cameras, the placed drawings and annotations survive a
fresh-process restart with zero changed interior pixels; the model and photo
remain unchanged too. `sheet-panes.png` uses only the synthetic fixture.

The sheet follow-up passed 795 native tests (9 explicitly ignored), all-target
workspace Clippy with warnings denied and a full workspace build. Focused tests
also verify that restoring a paper camera preserves shared sheet definitions,
revision counters and pending drawing jobs. Paper images remain shared, and
inactive panes do not display the active tool's selection highlight.
