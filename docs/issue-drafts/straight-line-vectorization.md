## Problem

The Section drawing tool already traces cut regions, fits straight segments, and can square near-orthogonal corners (`native/core/src/drawing/outline.rs`). The user needs a separate, controllable step that turns both plan and vertical section results into a clean, editable CAD line drawing. Current fill/outline output should remain available as the measured reference.

## Proposed behavior

- Add a **Straight lines** option to the Section drawing workflow for both plan and front/back/left/right vertical views, including a rotated section box for an arbitrarily oriented cut.
- Fit a small set of connected line segments to the cut contours, with user-set maximum positional deviation and minimum segment length. Offer optional orthogonal snapping relative to the dominant direction of the drawing; retain genuinely angled walls.
- Preserve room boundaries, door/window openings, columns, islands and holes. Never bridge a gap simply to reduce the line count.
- Show the simplified lines over the measured contour in the native preview. Export them to a dedicated CAD layer in DXF and DWG as editable LINE/LWPOLYLINE geometry, while keeping the original fill and outline layers selectable.
- Report segment count and maximum/RMS deviation. Work in bounded memory and allow cancellation on large sections.

## Acceptance tests

1. A noisy rectangular room with a doorway becomes four straight walls and accurate jambs, within the chosen deviation; the doorway remains open.
2. A rotated room keeps its rotation, and orthogonal snapping works relative to that rotation rather than world X/Y.
3. A real angled wall stays angled; round columns retain their curved or multi-segment outline instead of becoming rectangles.
4. The same options work for a plan and a vertical section; exported DWG/DXF reopens with the expected editable lines and unchanged fill.
5. Repeated runs produce the same geometry and no duplicate/intersecting segments at shared corners.

Related: #9 (2D DWG section export), #11 (solid plan fill).
