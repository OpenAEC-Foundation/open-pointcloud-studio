# Managed native session validation

Run `bash native/packaging/managed-wayland-smoke.sh /path/to/AppRun /path/to/results`.
Requires Python 3, labwc and Mesa's lavapipe Vulkan driver. No FUSE, GPU or
X server is used. The script creates an isolated headless Wayland compositor;
the probe creates isolated configuration, cache and mounted sample directories.
It exercises the real desktop's tools over authenticated MCP HTTP, rather than
a mock of the point-cloud operations. Outputs include JSON, window screenshots
and logs. Bearer keys are random per run and excluded from reports.

`result-2026-11-05.json` records a successful local amd64 Linux run with labwc
0.7.1 and the development binary. Idle RSS was measured after the empty window
rendered: 253,172 KiB (about 247 MiB). This is not a release AppImage or arm64
result; those are checked separately by the package workflow. Initial software
shader compilation can exceed the desktop's ten-second command timeout, so the
probe retries only the read-only status and screenshot commands for up to ninety
seconds. Mutating tools are never retried automatically.

The probe covers environment settings, bearer authentication, refusal of browser
Origins, session lifecycle, all tool names, open/save, screenshots, successful
SIGTERM and unhealthy status after exit. It does not claim that unsaved point
edits have a persistent recovery format or that browser-only SuperCloud route A
has been implemented. This integration is the supported native route B.

## Combined MCP access-control regression

Run `bash native/packaging/managed-wayland-smoke.sh BINARY RESULTS --paused-access`
(or `smoke.py BINARY --output RESULTS --paused-access` in the same graphical
environment) to start a real managed GUI with its persisted MCP checkbox off.
The authenticated health endpoint must still return 200 while an MCP status
call is refused as paused; after SIGTERM the endpoint must return 503. This
protects the combination of the status-bar access control and container health.
The GUI sends a heartbeat every two seconds; `/gui-health` expires after thirty
seconds without a GUI tick, without accepting any external command or exposing
scan data. The ordinary mode continues to exercise open/save and screenshots.
