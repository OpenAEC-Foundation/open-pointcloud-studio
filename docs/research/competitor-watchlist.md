# Weekly point-cloud competitor watchlist

Established: 4 November 2026. Scope: OpenAEC-Foundation/open-pointcloud-studio.

This list extends the [competitor review](competitor-review-2026-11-02.md). Product descriptions below are based on official documentation; research questions are proposals, not verified gaps in Open Pointcloud Studio. No competitor has been benchmarked as part of this list. Check current editions, platforms and licensing before drawing conclusions.

## Direct and specialist competitors

| Product and official starting source | Why follow it | Questions to investigate |
| --- | --- | --- |
| [CloudCompare](https://www.cloudcompare.org/doc/wiki/index.php/Methods) | Registration, segmentation, filters and distance analysis. | How does a user judge registration success and overlap? Which distance definition, sampling and uncertainty information must accompany an exported result? |
| [PointCab Origins](https://pointcab-software.com/en/learn/layouts_and_sections/) | Plans, sections, duplicated cuts, sheets and CAD delivery. | How many actions does a two-storey drawing set take? Which settings carry across cuts? How are changed source data, layers and coordinate origins handled? |
| [Autodesk ReCap Pro](https://www.autodesk.com/products/recap/overview) | Autodesk handoff, scan-to-mesh and downstream Revit workflows. | Does our E57 handoff import correctly in ReCap, with supported panoramas and poses? What is lost in downstream use, and which cloud services or licensed components does a workflow require? |
| [Leica Cyclone 3DR](https://leica-geosystems.com/products/laser-scanners/software/leica-cyclone/leica-cyclone-3dr/powerful-bim-analysis) | Scan-to-BIM tolerance analysis, coverage, reports and BCF. | How are missing observations distinguished from failures? Can a reviewer trace a colourmap/report to its region, reference model, units and settings? |
| [Trimble RealWorks](https://www.trimble.com/en/products/building-construction-field-systems/trimble-realworks) | Registration, segmentation, modelling and deliverables. | How does a user organise many scans, review alignment and continue interrupted work? Which modelling steps reduce manual cleanup without concealing source uncertainty? |
| [FARO SCENE](https://knowledge.faro.com/Software/FARO_SCENE/SCENE/Processing_Scans_in_SCENE) | Target-based, top-view and cloud-to-cloud registration, image processing and registration reports. | How are poor target distribution and weak overlap explained? How do station images, colours and registration statistics stay associated with the source scan? |
| [RIEGL RiSCAN PRO](https://www.riegl.com/en-canada/products/detail/riscan-pro) | Registration, georeferencing, filtering and batch processing of terrestrial scans. | How are static and kinematic datasets combined and independently checked? How are coordinate frames, control points and batch failures made visible? |
| [Terrasolid TerraScan](https://terrasolid.com/products/terrascan/) | Large survey datasets, classification, terrain and feature extraction. | Which classification/edit tools matter for our building and terrain users? What happens at tile boundaries? Can users inspect and reverse classification changes? |
| [3DF Zephyr](https://www.3dflow.net/zephyr-doc/en/Extractingamesh.html) | Adjacent photogrammetry/mesh workflow, including imported point clouds in supported editions. | When do photographs improve geometry or texture? How do meshing settings trade holes, invented surfaces, detail, processing cost and deviation from scan points? |

Also track Leica Cyclone REGISTER 360 PLUS when deep-diving registration; verify its own current documentation and edition separately from Cyclone 3DR's analysis features.

## Technical references

These are complementary references, not equivalent desktop competitors.

| Reference | Questions to investigate |
| --- | --- |
| [Potree](https://potree.org/potree/examples/github.html) | Which navigation, measurement, clipping and progressive-display interactions help people review large clouds? Treat browser sharing as an adjacent workflow, not a decision to replace the native application. |
| [PDAL](https://pdal.io/en/latest/stages/filters.html) | Which reproducible filtering/conversion pipelines provide useful independent checks? How are dimensions, CRS, scale/offset and rejected points handled? |
| [MeshLab](https://www.meshlab.net/) | How can mesh cleanup, simplification and topology checks expose quality loss? Compare against source geometry and retain colour/texture where required. |

Continue the existing monitor's COPC and Cesium/3D Tiles standards/streaming checks. Read the current specifications before proposing interoperability work; standards are not competitors.

## Weekly research procedure

The existing **Pointcloud Studio volgen** automation is scheduled every Monday at **09:00 Europe/Amsterdam**. Its saved configuration controls execution; this document is the research brief. The next scheduled Monday after this list was established is 9 November 2026.

1. Read the latest main branch, release, issues and PRs, and the previous monitor state. A feature can already be implemented while its original issue remains open for validation.
2. Scan official release notes, changelogs and documentation for all listed products. Record the exact source, product/edition/version, publication date when available and check date. Separate released capabilities from previews, roadmap promises and marketing claims.
3. Deep-dive two or three products or themes per week in rotation. Keep the rotation in the existing automation's `monitor-state.json`, alongside each product's last checked release/source and linked issue/PR. Do not reset history each week.
4. Compare a concrete workflow with the current implementation. Prioritise user impact, reliability, interoperability and performance on large clouds. Record questions when evidence is incomplete; do not turn every competitor feature into an issue.
5. For measured comparisons, use the same authorised/public inputs and document hardware, software versions, settings, time, peak RAM and output quality. Do not claim measured gains from documentation alone or commit customer scans.
6. Create an issue only for a relevant, concrete, previously uncovered problem. Report meaningful new findings and issue links in Dutch; stay quiet when there is no meaningful change.

## Prevent duplicate issues

Before **each** issue creation:

- Read related open and closed issues and open, closed and merged PRs, including descriptions and relevant comments. Search synonyms, workflow terms and technical names, not just matching titles.
- Check main's code and documentation: a stale open issue is not evidence that the feature is missing. Compare the proposed problem, expected outcome and acceptance criteria with existing scopes.
- Recheck immediately before creation to catch concurrent issues. Maintain a record of the topic and any created/reused issue URL in the existing monitor state.
- If already covered, link the existing item. Add a comment only for substantive new evidence, reproduction data or a clearly identified remaining gap; do not add repetitive weekly comments or reopen a resolved issue automatically.
- If partially covered, describe the genuinely separate scope and its relationship before deciding on a new issue. Do not split a single problem into overlapping issues.
- If access is insufficient to establish whether it is a duplicate, record the finding and access problem without creating an issue.

A new issue must contain the user problem, official source links and dates, current behaviour/evidence on main, impact, checked related issues/PRs and why their scope does not cover it, a feasible direction for the Rust application, and testable acceptance criteria. Do not label research questions as confirmed defects.

As of 4 November, registration and mixed-format export are already implemented in PRs #47–#50. Issues #42/#44 retain vendor/indoor validation; ReCap investigation #45 is closed and its [E57 handoff](../recap-interoperability.md) is documented. Reuse these existing scopes for matching evidence. The [October requirements, #32](https://github.com/OpenAEC-Foundation/open-pointcloud-studio/issues/32), also cover broad reliability, CRS, large-file and quality-report concerns. Refresh this mapping every run.
