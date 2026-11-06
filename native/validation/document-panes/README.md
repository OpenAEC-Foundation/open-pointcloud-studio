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

The storage and missing-monitor recovery tests currently exercise the separate
layout backend. The application does not yet restore these pane layouts at
startup. Linked panes, initial drag activation and complete cross-window drag
behavior remain acceptance work for issue #16; none is inferred from the
satellite open/dock test.

The recorded 2026-11-06 run passed 785 native tests (9 explicitly ignored),
workspace/all-target Clippy with warnings denied and a full workspace build.
The real station check used the 471-point BLK2GO section fixture, SHA-256
`21b15d28bd44d66a337da20adea0e86f8858cb09dbe90acc4fc9aab00027c8a5`,
from the public [Scanner2GO BLK2GO example](https://support.scanner2go.de/hc/de/articles/18239199476369-Download-Beispiele-BLK2GO-f%C3%BCr-Import-CAD).
Its photos decoded in an inactive visible pane, both cameras restored and the
source hash stayed unchanged. Only the JSON report is checked in for that scan;
`panes.png` contains the synthetic checkerboard fixture.
