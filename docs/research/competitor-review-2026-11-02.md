# Point-cloud competitor review and proposed priorities

Initially reviewed: 2 November 2026. Updated: 4 November 2026. Repository baseline: `afe5c0c` (`upstream/main`, including the 2026.45.1 registration and exchange work).

The [weekly competitor watchlist](competitor-watchlist.md) expands this review with products, research questions and an issue deduplication procedure.

## Recommendation

Prioritise **field validation of the implemented registration and exchange workflows**, followed by **repeatable drawing delivery** and **exportable quality evidence**. Keep the native, local workflow and open exchange formats as the foundation.

This is a research proposal, not a release commitment. Registration, ICP, mixed-format export and the ReCap handoff were implemented after the initial review in [PR #47](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/pull/47), with follow-up corrections in [#48](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/pull/48), [#49](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/pull/49) and [#50](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/pull/50). Do not create new feature issues for that existing scope. The strongest demand signal is the user feedback recorded in [issue #44](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/44): aligning static and SLAM scans. Competitor capabilities help define the workflow; they do not establish demand on their own. The earlier [October requirements study, #32](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/32), explicitly says its priorities still need validation with project users.

## Method and limits

Desk research uses the manufacturers' product pages and official documentation linked below, checked on the review date. No competitor was installed or timed. Capabilities are documented claims, not independently measured results. Edition, release and service restrictions can apply; absence from this review does not mean a competitor lacks a feature. Prices are deliberately omitted because licensing, region and bundles need a separate comparison.

The baseline comes from this repository's [README](../../README.md), [command API](../../native/API.md) and the open issues. Read the current main branch before implementation: an open issue or PR can describe work that is already partly present. In particular, straight CAD lines are already documented in main even though [PR #17](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/pull/17) remains open.

## What competitors do

| Product and official evidence | Documented workflow | Implication for Open Pointcloud Studio |
| --- | --- | --- |
| CloudCompare: [alignment and registration](https://www.cloudcompare.org/doc/wiki/index.php/Alignment_and_Registration), [distance computation](https://www.cloudcompare.org/doc/wiki/index.php/Distances_Computation) | Point-pair alignment, ICP refinement and reusable transformation matrices; cloud-to-cloud and cloud-to-mesh distances. The alignment page is older documentation, so verify the current UI before copying interaction details. | Make reference/moving scans explicit; let users inspect a proposed transform and residuals before applying it. Keep distance-to-scan and distance-to-mesh metrics distinct. |
| PointCab Origins: [Layouts & Sections](https://pointcab-software.com/en/learn/layouts_and_sections/) | Plans and sections at different angles, duplication for multiple cuts, sheets, difference analysis and CAD export. | Optimise the complete route from scan to a consistent drawing set. Single-section export and sheets alone do not distinguish us. |
| Autodesk ReCap Pro: [product overview](https://www.autodesk.com/products/recap/overview) | Point-cloud and mesh deliverables, Scan to Mesh, a Revit mesh plug-in and cloud workflows for feature extraction and sharing. | Validate exchange into downstream tools, including an E57 route into ReCap, rather than assuming a filename extension guarantees interoperability. |
| Leica Cyclone 3DR: [BIM analysis](https://leica-geosystems.com/products/laser-scanners/software/leica-cyclone/leica-cyclone-3dr/powerful-bim-analysis), [analysis selection guide](https://rcdocs.leica-geosystems.com/v1/docs/cyclone-3dr-cyclone-3dr-which-bim-analysis-feature-to-use) | Tolerance checking and progress analysis against BIM, reports, colourmaps and BCF notes linked to objects. The vendor also documents offline operation, including classification. | Local operation and BCF are useful but shared capabilities. Aim for traceable quality evidence; distinguish a fitted-plane residual from a check against a design model. |

## Existing baseline and remaining questions

| Area | Already documented in main | Remaining work or uncertainty |
| --- | --- | --- |
| Registration | Point pairs, manual transforms, bounded point-to-plane ICP, residuals, reusable matrices and registered/mixed-format exports. | [#44](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/44) now tracks remaining vendor/indoor field validation; the feature itself is implemented. |
| Drawings | Turned section boxes, plans/elevations/sections, measured contours and straight CAD polylines, DXF/DWG export. | Assess repeated cuts and export consistency across a whole building. Do not propose the existing line simplifier as new work. |
| Drawing delivery | A4–A0 sheets, placed views, title blocks, PDF export, dimensions and annotations. Pointcloud to Drawing also has a persisted project and resume workflow. | Test whether an exported set carries enough source/settings information to reproduce and review it; extend the existing project rather than introducing a second project format. |
| Quality | Detected-face RMS, P95/max and coverage; mesh-quality/deviation data in the API. | Audit which metrics survive export, how sampling is disclosed and whether they can be reviewed with their source and settings. Existing metrics are not a general scan-to-BIM tolerance check. |
| Collaboration | Saved views, annotations and BCF 2.1 export. | Verify object identity and coordinate handling in receiving tools. Do not imply every existing BCF note is linked to an IFC object. |
| Exchange and automation | E57/LAS/LAZ and other open formats; station/photo preservation, panorama editing, ReCap E57 handoff; CLI, desktop API/MCP, service API and C library. | [#42](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/42) retains vendor panorama validation. [#45](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/45) is closed; see the implemented [ReCap transfer and its validation boundary](../recap-interoperability.md). |

## Proposed implementation slices

### 1. Validate the existing registration and exchange workflow

The first review proposed point-pair registration as an implementation slice. Main now includes it, ICP, registered exports and panorama editing. Use [2026.45.1 validation](../validation/2026.45.1.md) as the starting evidence, not as proof of every scanner/workflow combination.

Research acceptance criteria:

- Observe a user aligning static and SLAM scans of the same indoor space; record setup time, confusing steps, failures and corrections.
- Check pair residuals, overlap and independent check points separately. A small fit residual does not establish absolute survey accuracy or remove SLAM drift.
- Validate a vendor SLAM E57 with panoramas and check that image associations, scanner/camera poses and units survive the registered export; keep the remaining checks under #42/#44.
- Complete a real Windows ReCap import of the prepared E57, including supported panoramas. The existing independent E57 checks do not establish a ReCap import result.
- Reuse the existing issues for new evidence unless a separate, confirmed defect or need has a demonstrably different scope.

### 2. Repeatable drawing-set delivery

Audit the existing Pointcloud to Drawing project, drawing settings and sheets against a two-storey task: one plan per floor plus two vertical sections. Preserve cut height, slab thickness, orientation, origin, units, visibility/class filters and line tolerance. Reuse the existing persistence model.

Acceptance criteria:

- Reopening the project restores the settings needed to regenerate the set.
- Each output identifies its source view and settings; changed sources or settings make stale outputs identifiable.
- Exports use consistent names, units and layers. Cancellation/failure identifies incomplete outputs without presenting them as a finished set.
- Reopen representative DXF/DWG files in an independent reader and check geometry, layers, dimensions and units; inspect the PDF sheets at their declared paper size.

### 3. Quality evidence beside the deliverable

First audit existing face and mesh metrics, then add only missing export/report fields. Include source identity, region, units, algorithm/settings, sample count or spacing, metric definition and tool version. Provide machine-readable data and a readable summary using the existing reporting/export facilities where possible.

Acceptance criteria:

- Label RMS, mean, P95 and maximum by what is actually measured. A sampled maximum must not be labelled a whole-scan maximum.
- Show missing coverage/no data separately from passing a tolerance. Never infer design compliance from a fitted surface alone.
- A generated plane/mesh with known offsets verifies the reported quantities; a sparse/no-overlap case verifies the missing-data behaviour.
- Keep BCF viewpoints, images and metric summaries consistent with the selected source and region.

## Prioritisation and evidence to collect

| Order | Reason | Next evidence |
| --- | --- | --- |
| Registration field validation | Direct reported user need in #44; implementation and public-data checks now exist. | Observe the indoor static/SLAM workflow and validate vendor panoramas; record evidence under #42/#44. |
| Repeatable drawing delivery | Builds on the existing product and the PointCab workflow. | Time the two-storey task and count manual corrections and settings that must be re-entered. |
| Quality evidence | Supports review and the open requirements in #32; inspired by CloudCompare and Cyclone workflows. | Ask a receiving engineer which measurements, provenance and formats are required to accept an output. |

Before publishing speed or accuracy comparisons, run the same authorised/public inputs with recorded hardware, versions, settings and output checks. Record import/index time, peak memory, index size, export time and failures separately. Follow [TEST_DATA.md](../../native/TEST_DATA.md); do not commit customer scans. This review contains no comparative benchmark results.

## Positioning to validate

A proposed message is: **a native, open-source route from existing scans to editable drawings and reviewable geometry, with repeatable settings and automation**. Validate it with users. Avoid claiming unique offline processing, unique BCF support, complete scan-to-BIM automation, lossless round-trips or better performance until there is specific evidence.
