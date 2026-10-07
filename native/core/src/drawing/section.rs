//! The two jobs that make a 2D drawing of the section box from the scans:
//! the export of a plan or a vertical section as DXF or DWG, and the preview
//! of its filled cut in the viewer.
//!
//! Both read the slab once, from the octree leaves that touch it, and keep
//! only the thinned points and the count grid, so their cost follows the
//! slab and the drawing, not the size of the scans.

use std::path::Path;
use std::time::{Duration, Instant};

use super::kept::{collect_slab_kept, KeptSlab};
use super::outline::{main_direction_of, trace_cut_regions, CutOutline, CutRegion, OutlineOptions};
use super::slab::{collect_slab, slab_from_section, Slab, SlabCut, SlabOptions, SlabPoint};
use super::{
    class_point_layer, drawing_info_text, source_point_layer, write_drawing_progress, Drawing2d,
    DrawingFormat, DrawingRequest, DrawingStats, PointColor, PointLayers, LAYER_POINTS,
    LAYER_RGB_CONTRAST,
};
use crate::region_source::{RegionFilter, RegionSource};
use crate::{normalized_degrees, LoadError, OrientedBox};

/// Layer colours for the points of several scans or classes, told apart on
/// a dark and on a light background. The frame is amber and the fill grey,
/// so neither is among them.
const POINT_LAYER_RGB: [[u8; 3]; 8] = [
    [60, 160, 230],
    [230, 80, 60],
    [80, 190, 90],
    [170, 100, 220],
    [60, 200, 200],
    [230, 110, 180],
    [200, 200, 80],
    [150, 110, 80],
];

/// The least time between two questions of the trace that reach the job.
const TRACE_QUESTION_INTERVAL: Duration = Duration::from_millis(20);

/// One layer of the scene to draw from.
#[derive(Debug, Clone, Copy)]
pub struct DrawingSource<'a> {
    /// Where its points are read from and where it stands in the scene.
    pub source: RegionSource<'a>,
    /// The file stem of the scan: with several scans each gets a point layer
    /// of its own, named after it.
    pub name: &'a str,
}

/// What a drawing job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingStage {
    /// Reading the points of the slab; counted in points read.
    Reading,
    /// Thinning the points of the slab that were read or kept from an
    /// earlier read, and counting them on the grid of the filled cut;
    /// counted in points. Only a drawing that keeps its points has it.
    Thinning,
    /// Tracing the filled cut. It has no measure: `total` is zero.
    Tracing,
    /// Writing the file; counted in entities.
    Writing,
}

/// How far a drawing job is. The callback that receives it also decides
/// whether the job goes on: an error from it, such as
/// `LoadError::Cancelled`, stops the job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawingProgress {
    pub stage: DrawingStage,
    pub done: u64,
    pub total: u64,
}

impl DrawingProgress {
    /// The part of the stage that is done, from 0 to 1; nothing for a stage
    /// without a measure.
    pub fn fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f64 / self.total as f64).min(1.0) as f32)
    }
}

/// A region of the filled cut in scene coordinates, on the cut plane: the
/// outer ring counter-clockwise as the view sees it, and the rings of its
/// holes. A ring is closed by itself.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewRegion {
    pub outer: Vec<[f64; 3]>,
    pub holes: Vec<Vec<[f64; 3]>>,
}

/// The filled cut of a slab, to lay over the points in the viewer.
#[derive(Debug, Clone, PartialEq)]
pub struct CutPreview {
    /// The slab the regions were traced from, with its cut plane.
    pub slab: Slab,
    pub regions: Vec<PreviewRegion>,
    /// As an export reports them; no points are drawn and nothing is written.
    pub stats: DrawingStats,
}

/// The slab as it was read and, when asked, traced.
struct SectionCut {
    slab: Slab,
    cut: SlabCut,
    outline: Option<CutOutline>,
    straight: Option<super::StraightLines>,
}

impl SectionCut {
    fn stats(&self) -> DrawingStats {
        DrawingStats {
            slab_points: self.cut.slab_points,
            read_points: self.cut.read_points,
            reused_points: self.cut.reused_points,
            drawn_points: self.cut.points.len() as u64,
            point_spacing: self.cut.spacing,
            regions: self.outline.as_ref().map_or(0, |cut| cut.regions.len()),
            vertices: self.outline.as_ref().map_or(0, |cut| {
                cut.regions.iter().map(|region| region.vertices()).sum()
            }),
            dropped_regions: self.outline.as_ref().map_or(0, |cut| cut.dropped),
            grid_cell: self.outline.as_ref().map(|cut| cut.cell),
            direction_degrees: self.outline.as_ref().map(|cut| cut.direction_degrees),
            bytes: 0,
            straight_lines: self.straight.as_ref().map(|lines| lines.stats),
        }
    }
}

/// Read the slab of the section box, from the points kept in `kept` where
/// it holds them, and trace its filled cut when asked.
fn cut_section(
    sources: &[DrawingSource<'_>],
    section: OrientedBox,
    request: &DrawingRequest,
    (points, fill): (bool, bool),
    accept: &RegionFilter<'_>,
    kept: Option<&mut KeptSlab>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<SectionCut, LoadError> {
    let slab = slab_from_section(section, request.view, request.thickness, request.origin)?;
    let layers: Vec<RegionSource<'_>> = sources.iter().map(|layer| layer.source).collect();
    let options = SlabOptions {
        point_spacing: points.then_some(request.point_spacing),
        max_points: request.max_points,
        grid: fill.then_some(request.grid),
        sample_percent: request.sample_percent,
        // A filled cut is traced from every point: on a sparse scan a tenth
        // of them leaves the walls without their fill.
        grid_percent: if request.fill || request.straight_lines.is_some() {
            100.0
        } else {
            request.sample_percent
        },
    };
    let mut cut = match kept {
        Some(kept) => collect_slab_kept(&layers, &slab, &options, accept, kept, progress)?,
        None => collect_slab(&layers, &slab, &options, accept, &mut |step| {
            progress(DrawingProgress {
                stage: DrawingStage::Reading,
                done: step.read,
                total: step.total,
            })
        })?,
    };
    if cut.slab_points == 0 {
        return Err(LoadError::InvalidData("the slab holds no points".into()));
    }
    // The grid has served once the regions are traced; the writer needs the
    // room.
    let outline = match cut.grid.take() {
        Some(grid) => {
            // Reducing an outline asks very often; the job hears of it when
            // a moment has passed since it last did. By time and not by
            // count, so that the question after a long step on a large grid
            // is always passed on.
            let mut heard: Option<Instant> = None;
            Some(trace_cut_regions(
                &grid,
                &OutlineOptions::for_request(request),
                &mut || {
                    if heard.is_some_and(|at| at.elapsed() < TRACE_QUESTION_INTERVAL) {
                        return Ok(());
                    }
                    heard = Some(Instant::now());
                    progress(DrawingProgress {
                        stage: DrawingStage::Tracing,
                        done: 0,
                        total: 0,
                    })
                },
            )?)
        }
        None => None,
    };
    let straight = match (&outline, request.straight_lines) {
        (Some(outline), Some(options)) => Some(super::straighten_cut_regions(
            &outline.regions,
            options,
            &mut || {
                progress(DrawingProgress {
                    stage: DrawingStage::Tracing,
                    done: 0,
                    total: 0,
                })
            },
        )?),
        _ => None,
    };
    Ok(SectionCut {
        slab,
        cut,
        outline,
        straight,
    })
}

/// The point layer of every source and class, made when its first point is
/// drawn, so that a scan without points in the slab leaves no empty layer.
struct PointLayerTable {
    /// Per source the name of its layer and its colour.
    sources: Vec<(String, [u8; 3])>,
    by_source: Vec<Option<u16>>,
    by_class: Vec<Option<u16>>,
    by: PointLayers,
}

impl PointLayerTable {
    fn new(sources: &[DrawingSource<'_>], by: PointLayers) -> Self {
        let mut named: Vec<(String, [u8; 3])> = Vec::with_capacity(sources.len());
        for (position, layer) in sources.iter().enumerate() {
            if sources.len() == 1 || by == PointLayers::Class {
                named.push((LAYER_POINTS.into(), LAYER_RGB_CONTRAST));
                continue;
            }
            // Two scans of the same name keep layers of their own.
            let base = source_point_layer(layer.name);
            let mut name = base.clone();
            let mut copy = 1;
            while named
                .iter()
                .any(|(taken, _)| taken.to_uppercase() == name.to_uppercase())
            {
                copy += 1;
                name = format!("{base}~{copy}");
            }
            named.push((name, POINT_LAYER_RGB[position % POINT_LAYER_RGB.len()]));
        }
        Self {
            by_source: vec![None; sources.len()],
            sources: named,
            by_class: vec![None; 256],
            by,
        }
    }

    fn layer(&mut self, drawing: &mut Drawing2d, point: &SlabPoint) -> Result<u16, LoadError> {
        if let (PointLayers::Class, Some(class)) = (self.by, point.classification) {
            let slot = &mut self.by_class[usize::from(class)];
            if slot.is_none() {
                *slot = Some(drawing.layer(
                    &class_point_layer(class),
                    POINT_LAYER_RGB[usize::from(class) % POINT_LAYER_RGB.len()],
                )?);
            }
            return Ok(slot.unwrap_or_default());
        }
        let source = point.source as usize;
        if self.by_source[source].is_none() {
            let (name, rgb) = &self.sources[source];
            self.by_source[source] = Some(drawing.layer(name, *rgb)?);
        }
        Ok(self.by_source[source].unwrap_or_default())
    }
}

/// Read the slab of the section box and build its drawing, without writing
/// it: the thinned points on their layers, the filled cut with its outlines
/// when the request asks for it, the frame of the box and the info text.
/// See [`export_section_drawing`] for the arguments.
pub fn section_drawing(
    sources: &[DrawingSource<'_>],
    section: impl Into<OrientedBox>,
    request: &DrawingRequest,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<(Drawing2d, DrawingStats), LoadError> {
    request.validate()?;
    let section = cut_section(
        sources,
        section.into(),
        request,
        (
            request.points,
            request.fill || request.straight_lines.is_some(),
        ),
        accept,
        None,
        progress,
    )?;
    let stats = section.stats();
    let SectionCut {
        slab,
        cut,
        outline,
        straight,
    } = section;
    let regions = outline.map(|cut| cut.regions).unwrap_or_default();
    let drawing = build_drawing(
        sources,
        &slab,
        &cut.points,
        regions,
        straight.as_ref(),
        request,
    )?;
    Ok((drawing, stats))
}

/// The drawing of a slab: the thinned points on their layers, the regions
/// of the filled cut with their outlines, the frame of the box and the info
/// text.
fn build_drawing(
    sources: &[DrawingSource<'_>],
    slab: &Slab,
    points: &[SlabPoint],
    regions: Vec<CutRegion>,
    straight: Option<&super::StraightLines>,
    request: &DrawingRequest,
) -> Result<Drawing2d, LoadError> {
    let mut drawing = Drawing2d::new(request.units);
    let mut layers = PointLayerTable::new(sources, request.point_layers);
    for point in points {
        let layer = layers.layer(&mut drawing, point)?;
        let rgb = point.rgb.filter(|_| request.color == PointColor::Rgb);
        drawing.add_point(layer, point.uv, rgb);
    }
    for region in regions {
        if request.fill {
            drawing.add_cut_region(region.outer, region.holes)?;
        } else {
            let layer = drawing.layer(super::LAYER_CUT_OUTLINE, LAYER_RGB_CONTRAST)?;
            drawing.add_polyline(layer, region.outer, true);
            for hole in region.holes {
                drawing.add_polyline(layer, hole, true);
            }
        }
    }
    if let Some(lines) = straight {
        let layer = drawing.layer(super::LAYER_STRAIGHT_LINES, [40, 110, 210])?;
        for ring in &lines.rings {
            drawing.add_polyline(layer, ring.clone(), true);
        }
    }
    let [min, max] = slab.extent;
    drawing.add_frame(min, max)?;
    // A text of a hundredth of the frame reads at the zoom that shows it all.
    let height = ((max[0] - min[0]).max(max[1] - min[1]) / 100.0).clamp(0.02, 0.5);
    drawing.add_info(
        [min[0], min[1] - 2.0 * height],
        height,
        &drawing_info_text(request.view, &slab.frame, slab.thickness, request.units),
    )?;
    Ok(drawing)
}

/// Draw the slab behind one face of the section box and write it as DXF or
/// DWG, whichever the extension of `destination` names.
///
/// - `sources` are the layers to draw from, each with its place in the scene
///   and, when it has one, its octree index.
/// - `section` is the section box in scene coordinates, axis-aligned or
///   turned about the vertical; a turned box is drawn along its own axes.
/// - `request` holds the view, the slab thickness and every other choice;
///   `DrawingRequest::for_view` gives the defaults.
/// - `accept` is asked for every point in the slab, with the position of its
///   layer in `sources`, its ordinal in the source file and the point in
///   scene coordinates. Deleted points and hidden classes are left out there.
/// - `progress` is called through all three stages and decides whether the
///   job goes on: an error from it, such as `LoadError::Cancelled`, stops it.
///
/// The file is put in place when it is complete; a job that fails or is
/// cancelled leaves an earlier file as it was. A slab without points is an
/// error, and nothing is written.
///
/// Time and memory of the read follow the slab: only the octree leaves that
/// touch it are read, and only a layer without an index is read in full.
/// What is held is the thinned points, at most `request.max_points`, and the
/// count grid of the filled cut, at most `MAX_CUT_GRID_CELLS` cells over the
/// part of the cut plane where the slab holds enough points to be drawn; a
/// slab whose layers reach much farther than that is read a second time to
/// lay the grid there. The writer then holds
/// the whole drawing, which is what the job needs most memory for: measured
/// 2.8 kB per point, 0.1 GB for 33,000 points and 1.0 GB for 360,000.
pub fn export_section_drawing(
    sources: &[DrawingSource<'_>],
    section: impl Into<OrientedBox>,
    request: &DrawingRequest,
    destination: &Path,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<DrawingStats, LoadError> {
    let format = DrawingFormat::from_path(destination).ok_or_else(|| {
        LoadError::UnsupportedFormat(
            destination
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase(),
        )
    })?;
    let (drawing, mut stats) = section_drawing(sources, section, request, accept, progress)?;
    let total = drawing.entities.len() as u64;
    stats.bytes = write_drawing_progress(&drawing, destination, format, request.version, |done| {
        progress(DrawingProgress {
            stage: DrawingStage::Writing,
            done: done as u64,
            total,
        })
    })?;
    Ok(stats)
}

/// Trace the filled cut of the slab, as an export with a fill would draw it,
/// and return its regions in scene coordinates on the cut plane, for an
/// overlay in the viewer. No points are collected and nothing is written;
/// whether the request asks for points or a fill makes no difference. The
/// arguments are those of [`export_section_drawing`].
pub fn preview_cut_regions(
    sources: &[DrawingSource<'_>],
    section: impl Into<OrientedBox>,
    request: &DrawingRequest,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<CutPreview, LoadError> {
    DrawingRequest {
        fill: true,
        ..*request
    }
    .validate()?;
    let section = cut_section(
        sources,
        section.into(),
        request,
        (false, true),
        accept,
        None,
        progress,
    )?;
    let stats = section.stats();
    let slab = section.slab;
    let world = |ring: Vec<[f64; 2]>| -> Vec<[f64; 3]> {
        ring.into_iter().map(|uv| slab.frame.to_world(uv)).collect()
    };
    let regions = section
        .outline
        .into_iter()
        .flat_map(|cut| cut.regions)
        .map(|region| PreviewRegion {
            outer: world(region.outer),
            holes: region.holes.into_iter().map(world).collect(),
        })
        .collect();
    Ok(CutPreview {
        slab,
        regions,
        stats,
    })
}

/// As [`preview_cut_regions`], and also the drawing an export with the same
/// request would write, built from the same read of the slab: the points
/// are collected when the request asks for them, and the filled cut is in
/// the drawing when it asks for a fill.
pub fn preview_section_drawing(
    sources: &[DrawingSource<'_>],
    section: impl Into<OrientedBox>,
    request: &DrawingRequest,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<(CutPreview, Drawing2d), LoadError> {
    preview_section(sources, section.into(), request, accept, None, progress)
}

/// As [`preview_section_drawing`], from the points of the slab that `kept`
/// holds from an earlier call, and keeping those it reads for the next one:
/// see [`KeptSlab`]. A drawing whose crop region changes, and so its box in
/// the plane of the drawing, is made again without reading what was read.
pub fn preview_section_drawing_kept(
    sources: &[DrawingSource<'_>],
    section: impl Into<OrientedBox>,
    request: &DrawingRequest,
    accept: &RegionFilter<'_>,
    kept: &mut KeptSlab,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<(CutPreview, Drawing2d), LoadError> {
    preview_section(
        sources,
        section.into(),
        request,
        accept,
        Some(kept),
        progress,
    )
}

fn preview_section(
    sources: &[DrawingSource<'_>],
    section: OrientedBox,
    request: &DrawingRequest,
    accept: &RegionFilter<'_>,
    kept: Option<&mut KeptSlab>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<(CutPreview, Drawing2d), LoadError> {
    DrawingRequest {
        fill: true,
        ..*request
    }
    .validate()?;
    let section = cut_section(
        sources,
        section,
        request,
        (request.points, true),
        accept,
        kept,
        progress,
    )?;
    let stats = section.stats();
    let SectionCut {
        slab,
        cut,
        outline,
        straight,
    } = section;
    let regions = outline.map(|cut| cut.regions).unwrap_or_default();
    let world = |ring: &[[f64; 2]]| -> Vec<[f64; 3]> {
        ring.iter().map(|uv| slab.frame.to_world(*uv)).collect()
    };
    let preview_regions = regions
        .iter()
        .map(|region| PreviewRegion {
            outer: world(&region.outer),
            holes: region.holes.iter().map(|hole| world(hole)).collect(),
        })
        .collect();
    let drawn = if request.fill || request.straight_lines.is_some() {
        regions
    } else {
        Vec::new()
    };
    let drawing = build_drawing(
        sources,
        &slab,
        &cut.points,
        drawn,
        straight.as_ref(),
        request,
    )?;
    Ok((
        CutPreview {
            slab,
            regions: preview_regions,
            stats,
        },
        drawing,
    ))
}

/// The turn of a section box that sets its axes along the walls inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallDirection {
    /// The turn of the box along the walls, in degrees counter-clockwise as
    /// seen from above, between -180 and 180: the turn the box had and the
    /// change.
    pub rotation_degrees: f64,
    /// How far the walls run from the axes of the box as it was: between
    /// -45 and 45 degrees.
    pub change_degrees: f64,
    /// Points in the part of the box that was read.
    pub slab_points: u64,
}

/// Find the direction of the walls in a section box, so that the box can be
/// turned to follow them.
///
/// The middle half of the height of the box is read as a plan, away from a
/// floor and a ceiling that would fill every cell, and the main direction of
/// the faces in it is found as the filled cut of a plan finds it. The box
/// keeps its rough direction: the change is at most 45 degrees either way.
/// Nothing when that part of the box holds no faces. The arguments are those
/// of [`export_section_drawing`]; progress is reported as reading.
pub fn wall_direction(
    sources: &[DrawingSource<'_>],
    section: impl Into<OrientedBox>,
    accept: &RegionFilter<'_>,
    progress: &mut dyn FnMut(DrawingProgress) -> Result<(), LoadError>,
) -> Result<Option<WallDirection>, LoadError> {
    let section: OrientedBox = section.into();
    if !section.is_valid() {
        return Err(LoadError::InvalidData(
            "the section box is not a box".into(),
        ));
    }
    let local = section.bounds;
    let height = local.max[2] - local.min[2];
    let (middle, thickness) = if height > 0.0 {
        let mut middle = local;
        middle.max[2] = local.min[2] + 0.75 * height;
        (middle, Some(0.5 * height))
    } else {
        (local, None)
    };
    let slab = slab_from_section(
        section.part(middle),
        super::DrawingView::Plan,
        thickness,
        super::DrawingOrigin::BoxCorner,
    )?;
    let layers: Vec<RegionSource<'_>> = sources.iter().map(|layer| layer.source).collect();
    let options = SlabOptions {
        point_spacing: None,
        max_points: 1,
        grid: Some(super::DEFAULT_CUT_GRID),
        sample_percent: 100.0,
        grid_percent: 100.0,
    };
    let cut = collect_slab(&layers, &slab, &options, accept, &mut |step| {
        progress(DrawingProgress {
            stage: DrawingStage::Reading,
            done: step.read,
            total: step.total,
        })
    })?;
    let Some(grid) = cut.grid.as_ref().filter(|_| cut.slab_points > 0) else {
        return Ok(None);
    };
    let found = main_direction_of(
        grid,
        &OutlineOptions::for_request(&DrawingRequest::default()),
    );
    Ok(found.map(|change| WallDirection {
        rotation_degrees: normalized_degrees(section.rotation_degrees + change),
        change_degrees: change,
        slab_points: cut.slab_points,
    }))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cadcodec::entities::hatch::BoundaryEdge;
    use cadcodec::entities::EntityType;
    use cadcodec::{CadDocument, DwgReader, DxfReader};

    use super::*;
    use crate::drawing::outline::probe::{
        crossings, furnished_room, room_errors, section_shape, worst_corner, worst_error,
    };
    use crate::drawing::outline::CutRegion;
    use crate::drawing::{
        DrawingEntity, DrawingOrigin, DrawingUnits, DrawingView, DEFAULT_DRAWING_POINTS,
        LAYER_CUT_FILL, LAYER_CUT_OUTLINE, LAYER_FRAME, LAYER_INFO, MAX_DRAWING_POINTS,
    };
    use crate::region_source::SourceTransform;
    use crate::test_shapes::{
        box_room, indexed_cloud, IndexedCloud, Noise, Opening, RoomSpec, Wall,
    };
    use crate::{Bounds, IndexedPoint, Point};

    /// The section box of the plan: around the furnished room, with its top
    /// face at 1.1 m, so that a slab of 0.1 m holds the heights 1.0 to 1.1.
    const PLAN_BOX: Bounds = Bounds {
        min: [-0.5, -0.5, 0.0],
        max: [4.5, 3.5, 1.1],
    };

    fn everything(_: usize, _: u64, _: &Point) -> bool {
        true
    }

    #[test]
    fn straight_lines_keep_the_reference_and_roundtrip_in_plan_and_section() {
        for view in [DrawingView::Plan, DrawingView::Front] {
            let mut points = room_points();
            let bounds = if view == DrawingView::Front {
                for point in &mut points {
                    point.xyz.swap(1, 2);
                }
                Bounds {
                    min: [-0.5, 1.0, -0.5],
                    max: [4.5, 2.0, 3.5],
                }
            } else {
                PLAN_BOX
            };
            let directory = tempfile::tempdir().unwrap();
            let scan = indexed_cloud(&points, 5_000);
            let sources = [DrawingSource {
                source: indexed(&scan),
                name: "room",
            }];
            let reference = DrawingRequest {
                view,
                fill: true,
                points: false,
                ..Default::default()
            };
            let (original, _) =
                section_drawing(&sources, bounds, &reference, &everything, &mut |_| Ok(()))
                    .unwrap();
            let request = DrawingRequest {
                straight_lines: Some(super::super::StraightLineOptions::default()),
                ..reference
            };
            let (drawing, stats) =
                section_drawing(&sources, bounds, &request, &everything, &mut |_| Ok(())).unwrap();
            assert_eq!(fills(&original), fills(&drawing));
            let report = stats.straight_lines.unwrap();
            assert!(report.segments > 0 && report.segments <= report.input_segments);
            assert!(report.max_deviation <= 0.010001);
            let layer_name = super::super::LAYER_STRAIGHT_LINES;
            let index = drawing
                .layers
                .iter()
                .position(|layer| layer.name == layer_name)
                .unwrap() as u16;
            let lines: Vec<_> = drawing
                .entities
                .iter()
                .filter(|(layer, _)| *layer == index)
                .collect();
            assert!(lines
                .iter()
                .all(|(_, entity)| matches!(entity, DrawingEntity::Polyline { closed: true, .. })));
            for extension in ["dxf", "dwg"] {
                let path = directory.path().join(format!("straight.{extension}"));
                super::super::write_drawing(
                    &drawing,
                    &path,
                    DrawingFormat::from_path(&path).unwrap(),
                    request.version,
                )
                .unwrap();
                let read = read_back(&path, request.units);
                assert_eq!(
                    read.polylines
                        .iter()
                        .filter(|(layer, _, _)| layer == layer_name)
                        .count(),
                    lines.len()
                );
                assert_eq!(read.fills.len(), fills(&original).len());
            }
        }
    }

    /// The furnished room as a scan: every point with a colour and one of two
    /// classes.
    fn room_points() -> Vec<Point> {
        let mut points = furnished_room().cloud_points();
        for (ordinal, point) in points.iter_mut().enumerate() {
            point.classification = Some(if ordinal % 2 == 0 { 2 } else { 6 });
        }
        points
    }

    fn records(points: &[Point]) -> Vec<IndexedPoint> {
        points
            .iter()
            .enumerate()
            .map(|(ordinal, point)| IndexedPoint {
                point: *point,
                ordinal: ordinal as u64,
            })
            .collect()
    }

    fn indexed(cloud: &IndexedCloud) -> RegionSource<'_> {
        RegionSource::new(&cloud.cloud, Some(&cloud.index), SourceTransform::default())
    }

    /// The filled regions of a drawing, in metres.
    fn fills(drawing: &Drawing2d) -> Vec<CutRegion> {
        drawing
            .entities
            .iter()
            .filter_map(|(_, entity)| match entity {
                DrawingEntity::Fill { outer, holes } => Some(CutRegion {
                    outer: outer.clone(),
                    holes: holes.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// What a written drawing holds, as the codec reads it back.
    #[derive(Default)]
    struct Read {
        layers: Vec<String>,
        /// Points per layer, with how many carry a colour of their own.
        points: Vec<(String, usize, usize)>,
        /// The fills in metres.
        fills: Vec<CutRegion>,
        /// Layer, whether closed, and corners in drawing units.
        polylines: Vec<(String, bool, Vec<[f64; 2]>)>,
        texts: Vec<(String, String)>,
        insertion_units: i16,
    }

    impl Read {
        fn point_count(&self) -> usize {
            self.points.iter().map(|(_, count, _)| count).sum()
        }
    }

    fn read_back(path: &Path, units: DrawingUnits) -> Read {
        let document: CadDocument = match DrawingFormat::from_path(path).unwrap() {
            DrawingFormat::Dxf => DxfReader::from_file(path).unwrap().read().unwrap(),
            DrawingFormat::Dwg => DwgReader::from_file(path).unwrap().read().unwrap(),
        };
        let mut read = Read {
            insertion_units: document.header.insertion_units,
            ..Read::default()
        };
        let factor = units.factor();
        for entity in document.entities() {
            let layer = entity.common().layer.clone();
            if !read.layers.contains(&layer) {
                assert!(
                    document.layers.contains(&layer),
                    "layer {layer} is not in the table"
                );
                read.layers.push(layer.clone());
            }
            match entity {
                EntityType::Point(point) => {
                    let coloured = usize::from(point.common.color.is_true_color());
                    match read.points.iter_mut().find(|(name, _, _)| *name == layer) {
                        Some((_, count, colours)) => {
                            *count += 1;
                            *colours += coloured;
                        }
                        None => read.points.push((layer, 1, coloured)),
                    }
                }
                EntityType::Hatch(hatch) => {
                    assert_eq!(layer, LAYER_CUT_FILL);
                    assert!(hatch.is_solid);
                    let mut rings = hatch.paths.iter().map(|path| {
                        let [BoundaryEdge::Polyline(edge)] = path.edges.as_slice() else {
                            panic!("a ring is one polyline edge");
                        };
                        edge.vertices
                            .iter()
                            .map(|vertex| [vertex.x / factor, vertex.y / factor])
                            .collect::<Vec<_>>()
                    });
                    read.fills.push(CutRegion {
                        outer: rings.next().unwrap(),
                        holes: rings.collect(),
                    });
                }
                EntityType::LwPolyline(polyline) => read.polylines.push((
                    layer,
                    polyline.is_closed,
                    polyline
                        .vertices
                        .iter()
                        .map(|vertex| [vertex.location.x, vertex.location.y])
                        .collect(),
                )),
                EntityType::Text(text) => read.texts.push((layer, text.value.clone())),
                other => panic!("unexpected entity {other:?}"),
            }
        }
        read
    }

    #[test]
    fn plan_of_a_room_is_written_as_dwg_and_dxf_and_reads_back_with_its_walls() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 4_096);
        let sources = [DrawingSource {
            source: indexed(&cloud),
            name: "hall",
        }];
        let directory = tempfile::tempdir().unwrap();

        // DWG, with every default: plan, slab of 0.1 m, points and fill.
        let request = DrawingRequest::default();
        let path = directory.path().join("plan.dwg");
        let mut stages = Vec::new();
        let stats = export_section_drawing(
            &sources,
            PLAN_BOX,
            &request,
            &path,
            &everything,
            &mut |step| {
                if stages.last() != Some(&step.stage) {
                    stages.push(step.stage);
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            stages,
            [
                DrawingStage::Reading,
                DrawingStage::Tracing,
                DrawingStage::Writing
            ]
        );
        // Every point of the scan lies in the slab.
        assert_eq!(stats.slab_points, points.len() as u64);
        assert_eq!(stats.read_points, points.len() as u64);
        assert_eq!(stats.point_spacing, 0.005);
        // One point per 5 mm of face in plan, over the two or three cells
        // that 2 mm of noise spreads a face across: measured 11,979.
        assert!(stats.drawn_points > 10_000 && stats.drawn_points < 14_000);
        assert_eq!((stats.regions, stats.vertices), (3, 20));
        assert_eq!(stats.dropped_regions, 0);
        assert_eq!(stats.grid_cell, Some(0.02));
        assert_eq!(stats.direction_degrees, Some(0.0));
        assert_eq!(stats.bytes, fs::metadata(&path).unwrap().len());

        let read = read_back(&path, DrawingUnits::Millimetres);
        assert_eq!(read.insertion_units, 4);
        let mut layers = read.layers.clone();
        layers.sort();
        assert_eq!(
            layers,
            [
                LAYER_CUT_FILL,
                LAYER_CUT_OUTLINE,
                LAYER_FRAME,
                LAYER_INFO,
                LAYER_POINTS
            ]
        );
        // One scan: all points on the one point layer, in the layer colour.
        assert_eq!(
            read.points,
            [(LAYER_POINTS.to_string(), stats.drawn_points as usize, 0)]
        );
        // The walls as they are in the file: two parts and the column, the
        // door and the window open, every corner square.
        assert_eq!(read.fills.len(), 3);
        assert!(read.fills.iter().all(|fill| fill.holes.is_empty()));
        // Measured in the file: faces within 0.06 mm and jambs within 2.3 mm
        // of the truth.
        let (faces, openings) = room_errors(&read.fills);
        assert!(faces < 0.001, "{faces}");
        assert!(openings < 0.006, "{openings}");
        assert!(worst_corner(&read.fills) < 1e-6);
        // An outline per ring, and the frame of the box in millimetres.
        let outlines: Vec<_> = read
            .polylines
            .iter()
            .filter(|(layer, _, _)| layer == LAYER_CUT_OUTLINE)
            .collect();
        assert_eq!(outlines.len(), 3);
        assert_eq!(
            outlines
                .iter()
                .map(|(_, _, corners)| corners.len())
                .sum::<usize>(),
            20
        );
        assert!(read.polylines.iter().all(|(_, closed, _)| *closed));
        let frame: Vec<_> = read
            .polylines
            .iter()
            .filter(|(layer, _, _)| layer == LAYER_FRAME)
            .collect();
        assert_eq!(frame.len(), 1);
        assert_eq!(
            frame[0].2,
            [
                [-500.0, -500.0],
                [4500.0, -500.0],
                [4500.0, 3500.0],
                [-500.0, 3500.0]
            ]
        );
        assert_eq!(
            read.texts,
            [(
                LAYER_INFO.to_string(),
                "Plan; cut plane Z = 1.100 m; slab 0.100 m; scale 1:1; units mm; \
                 drawing zero at model X 0.000, Y 0.000, Z 1.100 m"
                    .to_string()
            )]
        );

        // DXF, with few points: the reader of the codec is slow on many.
        let request = DrawingRequest {
            max_points: 2_000,
            ..request
        };
        let path = directory.path().join("plan.dxf");
        let stats = export_section_drawing(
            &sources,
            PLAN_BOX,
            &request,
            &path,
            &everything,
            &mut |_| Ok(()),
        )
        .unwrap();
        // Measured: 1,174 points at a spacing of 40 mm.
        assert!(stats.drawn_points <= 2_000 && stats.drawn_points > 500);
        assert!(stats.point_spacing > 0.005);
        assert_eq!((stats.regions, stats.vertices), (3, 20));
        let read = read_back(&path, DrawingUnits::Millimetres);
        assert_eq!(read.point_count(), stats.drawn_points as usize);
        assert_eq!(read.fills.len(), 3);
        let (faces, openings) = room_errors(&read.fills);
        assert!(faces < 0.001 && openings < 0.006);
        assert_eq!(read.polylines.len(), 4);
        assert_eq!(read.texts.len(), 1);
        // The own reader takes the points and passes over the rest; in
        // millimetres they are a thousand times the scan.
        let reopened = crate::open(&path, 10).unwrap();
        assert_eq!(reopened.total_points, stats.drawn_points);
        assert!(reopened.bounds.min[0] >= -110.0 && reopened.bounds.max[0] <= 4110.0);
        assert!(reopened.bounds.min[1] >= -110.0 && reopened.bounds.max[1] <= 3110.0);
        assert_eq!((reopened.bounds.min[2], reopened.bounds.max[2]), (0.0, 0.0));
        // Nothing but the two drawings is left in the folder.
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn plan_of_a_turned_room_at_grid_coordinates_follows_the_room() {
        // The room turned 30 degrees and moved to national grid coordinates,
        // read from its index.
        let moved = [207_000.0, 474_000.0];
        let room = furnished_room().transformed(30.0, [moved[0], moved[1], 0.0]);
        let cloud = indexed_cloud(&room.cloud_points(), 4_096);
        let sources = [DrawingSource {
            source: indexed(&cloud),
            name: "hall",
        }];
        let section = Bounds {
            min: [moved[0] - 10.0, moved[1] - 10.0, 0.0],
            max: [moved[0] + 10.0, moved[1] + 10.0, 1.1],
        };
        let (sin, cos) = 30f64.to_radians().sin_cos();
        // The regions as the room was before it was turned and moved.
        let back = |drawing: &Drawing2d| -> Vec<CutRegion> {
            fills(drawing)
                .iter()
                .map(|region| CutRegion {
                    outer: region
                        .outer
                        .iter()
                        .map(|at| {
                            let (x, y) = (at[0] - moved[0], at[1] - moved[1]);
                            [cos * x + sin * y, cos * y - sin * x]
                        })
                        .collect(),
                    holes: Vec::new(),
                })
                .collect()
        };
        for square in [true, false] {
            let request = DrawingRequest {
                square,
                ..DrawingRequest::default()
            };
            let (drawing, stats) =
                section_drawing(&sources, section, &request, &everything, &mut |_| Ok(())).unwrap();
            assert_eq!(stats.regions, 3, "square {square}");
            let direction = stats.direction_degrees.unwrap();
            assert!((direction - 30.0).abs() < 0.01, "{direction}");
            let regions = back(&drawing);
            let (faces, openings) = room_errors(&regions);
            // Measured: faces within 0.25 mm, jambs within 1.1 mm, and
            // without squaring corners within 0.25 degree of square.
            assert!(faces < 0.001, "square {square}: {faces}");
            assert!(openings < 0.004, "square {square}: {openings}");
            assert!(worst_corner(&regions) < if square { 1e-6 } else { 1.0 });
            // The points are drawn where they are in the model.
            assert!(drawing.entities.iter().all(|(_, entity)| match entity {
                DrawingEntity::Point { uv, .. } =>
                    (uv[0] - moved[0]).abs() < 10.0 && (uv[1] - moved[1]).abs() < 10.0,
                _ => true,
            }));
        }
    }

    /// A band of the room for vertical sections, 0.7 m wide across `axis`
    /// around the slab that starts at `from`.
    fn section_band(axis: usize, from: f64) -> Vec<Point> {
        let room = section_shape();
        let mut points = room.cloud_points();
        points.retain(|point| point.xyz[axis] >= from - 0.3 && point.xyz[axis] <= from + 0.4);
        points
    }

    #[test]
    fn vertical_sections_are_drawn_as_seen_from_their_side() {
        let request = DrawingRequest {
            fill: true,
            points: false,
            ..DrawingRequest::for_view(DrawingView::Front)
        };
        // Along x, through the window in the east wall.
        let points = section_band(1, 1.5);
        let cloud = indexed_cloud(&points, 4_096);
        let sources = [DrawingSource {
            source: indexed(&cloud),
            name: "hall",
        }];
        let section = Bounds {
            min: [-0.5, 1.5, -0.5],
            max: [4.5, 1.6, 3.0],
        };
        let draw = |request: &DrawingRequest| {
            section_drawing(&sources, section, request, &everything, &mut |_| Ok(())).unwrap()
        };
        let worst = |regions: &[CutRegion], lines: &[(usize, f64, [f64; 4])]| {
            lines
                .iter()
                .map(|(axis, level, truth)| worst_error(&crossings(regions, *axis, *level), truth))
                .fold(0.0, f64::max)
        };

        // From the front u runs with x from the left edge of the box, and v
        // is the height: floor and ceiling of 0.25 m, walls of 0.1 m, the
        // window on the right.
        let (drawing, stats) = draw(&request);
        assert_eq!(stats.drawn_points, 0);
        assert_eq!(stats.regions, 1);
        assert_eq!(stats.direction_degrees, Some(0.0));
        assert_eq!(drawing.point_count(), 0);
        let front = fills(&drawing);
        let seen_from_front = [
            (0, 2.5, [-0.25, 0.0, 2.6, 2.85]),
            (1, 0.5, [0.4, 0.5, 4.5, 4.6]),
            (0, 4.55, [-0.25, 0.9, 2.1, 2.85]),
        ];
        // Measured within 1.1 mm.
        let error = worst(&front, &seen_from_front);
        assert!(error < 0.003, "{error}");
        assert_eq!(crossings(&front, 0, 0.45).len(), 2);
        assert!(worst_corner(&front) < 1e-9);

        // From the back the window is on the left.
        let back_request = DrawingRequest {
            view: DrawingView::Back,
            ..request
        };
        let back = fills(&draw(&back_request).0);
        let seen_from_back = [
            (0, 2.5, [-0.25, 0.0, 2.6, 2.85]),
            (1, 0.5, [0.4, 0.5, 4.5, 4.6]),
            (0, 0.45, [-0.25, 0.9, 2.1, 2.85]),
        ];
        let error = worst(&back, &seen_from_back);
        assert!(error < 0.003, "{error}");
        assert_eq!(crossings(&back, 0, 4.55).len(), 2);

        // With zero at the corner of the box the heights count from its
        // lower face, half a metre below the floor.
        let corner_request = DrawingRequest {
            origin: DrawingOrigin::BoxCorner,
            ..request
        };
        let (drawing, _) = draw(&corner_request);
        let corner = fills(&drawing);
        let from_corner = [
            (0, 2.5, [0.25, 0.5, 3.1, 3.35]),
            (1, 1.0, [0.4, 0.5, 4.5, 4.6]),
            (0, 4.55, [0.25, 1.4, 2.6, 3.35]),
        ];
        assert!(worst(&corner, &from_corner) < 0.003);
        // The frame is the box as seen: 5.0 by 3.5 m.
        let frame = drawing
            .entities
            .iter()
            .find_map(|(layer, entity)| match entity {
                DrawingEntity::Polyline { points, .. }
                    if drawing.layers[usize::from(*layer)].name == LAYER_FRAME =>
                {
                    Some(points.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(frame, [[0.0, 0.0], [5.0, 0.0], [5.0, 3.5], [0.0, 3.5]]);

        // Along y, seen from the right: u runs with y from the south edge of
        // the box, through the south and north wall.
        let points = section_band(0, 2.0);
        let cloud = indexed_cloud(&points, 4_096);
        let sources = [DrawingSource {
            source: indexed(&cloud),
            name: "hall",
        }];
        let section = Bounds {
            min: [1.0, -0.5, -0.5],
            max: [2.1, 3.5, 3.0],
        };
        let right_request = DrawingRequest {
            view: DrawingView::Right,
            ..request
        };
        let (drawing, stats) =
            section_drawing(&sources, section, &right_request, &everything, &mut |_| {
                Ok(())
            })
            .unwrap();
        // The slab is the 0.1 m behind the face at x = 2.1.
        let in_slab = points
            .iter()
            .filter(|point| point.xyz[0] >= 2.0 && point.xyz[0] <= 2.1)
            .count();
        assert_eq!(stats.slab_points, in_slab as u64);
        let right = fills(&drawing);
        // A closed ring of floor, walls and ceiling around the room.
        assert_eq!(right.len(), 1);
        assert_eq!(right[0].holes.len(), 1);
        let seen_from_right = [
            (0, 2.0, [-0.25, 0.0, 2.6, 2.85]),
            (1, 1.0, [0.4, 0.5, 3.5, 3.6]),
        ];
        // Measured within 0.8 mm.
        let error = worst(&right, &seen_from_right);
        assert!(error < 0.003, "{error}");
    }

    #[test]
    fn points_go_on_a_layer_per_scan_or_per_class_with_or_without_their_colour() {
        let points = room_points();
        let in_memory = records(&points);
        let layer = |x: f64, y: f64| {
            RegionSource::resident(
                &in_memory,
                SourceTransform {
                    scale: [1.0; 3],
                    offset: [x, y, 0.0],
                },
            )
        };
        // Three scans, two of them with the same name.
        let sources = [
            DrawingSource {
                source: layer(0.0, 0.0),
                name: "hall",
            },
            DrawingSource {
                source: layer(6.0, 0.0),
                name: "Hall",
            },
            DrawingSource {
                source: layer(0.0, 5.0),
                name: "annexe: 2",
            },
        ];
        let section = Bounds {
            min: [-1.0, -1.0, 0.0],
            max: [11.0, 9.0, 1.1],
        };
        let request = DrawingRequest {
            fill: false,
            ..DrawingRequest::default()
        };
        // Points per layer, with how many carry a colour of their own.
        let per_layer = |drawing: &Drawing2d| -> Vec<(String, usize, usize)> {
            let mut counts: Vec<(String, usize, usize)> = drawing
                .layers
                .iter()
                .map(|layer| (layer.name.clone(), 0, 0))
                .collect();
            for (layer, entity) in &drawing.entities {
                if let DrawingEntity::Point { rgb, .. } = entity {
                    counts[usize::from(*layer)].1 += 1;
                    counts[usize::from(*layer)].2 += usize::from(rgb.is_some());
                }
            }
            counts.retain(|(_, points, _)| *points > 0);
            counts
        };
        let draw = |sources: &[DrawingSource<'_>], request: &DrawingRequest| {
            section_drawing(sources, section, request, &everything, &mut |_| Ok(())).unwrap()
        };

        let (drawing, stats) = draw(&sources, &request);
        assert_eq!(stats.slab_points, 3 * points.len() as u64);
        assert_eq!((stats.regions, stats.grid_cell), (0, None));
        let layers = per_layer(&drawing);
        // The copies stand whole cells apart, so each draws the same points.
        let each = stats.drawn_points as usize / 3;
        assert_eq!(
            layers,
            [
                ("OPS-POINTS-hall".to_string(), each, 0),
                ("OPS-POINTS-Hall~2".to_string(), each, 0),
                ("OPS-POINTS-annexe_ 2".to_string(), each, 0),
            ]
        );
        // Each scan has a layer colour of its own.
        let colours: Vec<[u8; 3]> = drawing
            .layers
            .iter()
            .take(3)
            .map(|layer| layer.rgb)
            .collect();
        assert!(colours[0] != colours[1] && colours[1] != colours[2] && colours[0] != colours[2]);

        // One scan alone needs no layer of its own, and with the scanned
        // colours every point carries one.
        let coloured = DrawingRequest {
            color: PointColor::Rgb,
            ..request
        };
        let (drawing, _) = draw(&sources[..1], &coloured);
        assert_eq!(
            per_layer(&drawing),
            [(LAYER_POINTS.to_string(), each, each)]
        );

        // Per class: the two classes of the scan, whatever scan a point is
        // from.
        let by_class = DrawingRequest {
            point_layers: PointLayers::Class,
            ..request
        };
        let (drawing, stats) = draw(&sources, &by_class);
        let layers = per_layer(&drawing);
        assert_eq!(layers.len(), 2);
        let names: Vec<&str> = layers.iter().map(|(name, _, _)| name.as_str()).collect();
        assert!(names.contains(&"OPS-POINTS-CLASS-02") && names.contains(&"OPS-POINTS-CLASS-06"));
        assert_eq!(layers[0].1 + layers[1].1, stats.drawn_points as usize);
        assert!(layers[0].1 > each && layers[1].1 > each);
        // A scan without classes stays on the one point layer.
        let mut plain = points.clone();
        plain
            .iter_mut()
            .for_each(|point| point.classification = None);
        let plain = records(&plain);
        let sources = [DrawingSource {
            source: RegionSource::resident(&plain, SourceTransform::default()),
            name: "hall",
        }];
        let (drawing, _) = draw(&sources, &by_class);
        assert_eq!(per_layer(&drawing), [(LAYER_POINTS.to_string(), each, 0)]);
    }

    #[test]
    fn drawing_is_at_scale_one_to_one_in_millimetres_or_metres() {
        // Two targets 3.215 m apart, and a third point to give the cloud a
        // size across.
        let targets = [[1.0, 2.0, 1.05], [4.215, 2.0, 1.05], [2.0, 3.0, 1.02]];
        let in_memory = records(&targets.map(|xyz| Point {
            xyz,
            rgb: None,
            intensity: None,
            classification: None,
        }));
        let sources = [DrawingSource {
            source: RegionSource::resident(&in_memory, SourceTransform::default()),
            name: "targets",
        }];
        let section = Bounds {
            min: [0.5, 1.5, 0.0],
            max: [5.0, 3.5, 1.1],
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("targets.dxf");
        let drawn = |units: DrawingUnits, origin: DrawingOrigin| -> Vec<[f64; 3]> {
            let request = DrawingRequest {
                fill: false,
                units,
                origin,
                ..DrawingRequest::default()
            };
            let stats = export_section_drawing(
                &sources,
                section,
                &request,
                &path,
                &everything,
                &mut |_| Ok(()),
            )
            .unwrap();
            assert_eq!((stats.slab_points, stats.drawn_points), (3, 3));
            let cloud = crate::open(&path, 10).unwrap();
            cloud.points.iter().map(|point| point.xyz).collect()
        };
        let close = |a: [f64; 3], b: [f64; 3]| (0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-6);
        // Row by row from the bottom of the view: model X and Y times a
        // thousand, and 3215.0 between the targets.
        let points = drawn(DrawingUnits::Millimetres, DrawingOrigin::Model);
        assert!(close(points[0], [1000.0, 2000.0, 0.0]));
        assert!(close(points[1], [4215.0, 2000.0, 0.0]));
        assert!(close(points[2], [2000.0, 3000.0, 0.0]));
        assert!((points[1][0] - points[0][0] - 3215.0).abs() < 1e-6);
        let points = drawn(DrawingUnits::Metres, DrawingOrigin::Model);
        assert!(close(points[0], [1.0, 2.0, 0.0]));
        assert!((points[1][0] - points[0][0] - 3.215).abs() < 1e-9);
        // From the corner of the box the distance is the same.
        let points = drawn(DrawingUnits::Millimetres, DrawingOrigin::BoxCorner);
        assert!(close(points[0], [500.0, 500.0, 0.0]));
        assert!(close(points[1], [3715.0, 500.0, 0.0]));
    }

    #[test]
    fn point_limit_doubles_the_spacing_and_is_never_passed() {
        let points = room_points();
        let in_memory = records(&points);
        let sources = [DrawingSource {
            source: RegionSource::resident(&in_memory, SourceTransform::default()),
            name: "hall",
        }];
        let draw = |max_points: usize| {
            let request = DrawingRequest {
                fill: false,
                max_points,
                ..DrawingRequest::default()
            };
            section_drawing(&sources, PLAN_BOX, &request, &everything, &mut |_| Ok(()))
        };
        assert_eq!(DrawingRequest::default().max_points, DEFAULT_DRAWING_POINTS);
        let (full, full_stats) = draw(DEFAULT_DRAWING_POINTS).unwrap();
        assert_eq!(full_stats.point_spacing, 0.005);
        assert_eq!(full.point_count() as u64, full_stats.drawn_points);
        let mut last = full_stats.drawn_points;
        for max_points in [8_000, 2_000, 500, 1] {
            let (drawing, stats) = draw(max_points).unwrap();
            assert!(stats.drawn_points as usize <= max_points, "{max_points}");
            assert_eq!(drawing.point_count() as u64, stats.drawn_points);
            assert!(stats.drawn_points < last && stats.drawn_points > 0);
            last = stats.drawn_points;
            // The spacing that was used is reported: the one asked, doubled
            // a number of times.
            let doublings = (stats.point_spacing / 0.005).log2();
            assert!(
                doublings >= 1.0 && doublings.fract() == 0.0,
                "{}",
                stats.point_spacing
            );
            // Half the spacing would not have fitted.
            assert_eq!(stats.slab_points, points.len() as u64);
        }
        // More than a drawing holds is refused before anything is read.
        let request = DrawingRequest {
            max_points: MAX_DRAWING_POINTS + 1,
            ..DrawingRequest::default()
        };
        let mut asked = false;
        let result = section_drawing(&sources, PLAN_BOX, &request, &everything, &mut |_| {
            asked = true;
            Ok(())
        });
        assert!(matches!(result, Err(LoadError::InvalidData(_))));
        assert!(!asked);
        assert!(draw(MAX_DRAWING_POINTS).is_ok());
    }

    #[test]
    fn empty_slab_or_cancelled_job_writes_nothing_and_keeps_an_earlier_file() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 4_096);
        let sources = [DrawingSource {
            source: indexed(&cloud),
            name: "hall",
        }];
        let directory = tempfile::tempdir().unwrap();
        let request = DrawingRequest::default();
        for extension in ["dxf", "dwg"] {
            let path = directory.path().join(format!("plan.{extension}"));
            fs::write(&path, b"earlier export").unwrap();
            let untouched = || {
                assert_eq!(fs::read(&path).unwrap(), b"earlier export");
                assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
            };
            // A box above the scan, which reads nothing, and a scan of
            // which every point was deleted.
            let above = Bounds {
                min: [-0.5, -0.5, 5.0],
                max: [4.5, 3.5, 6.0],
            };
            let deleted = |_: usize, _: u64, _: &Point| false;
            let empty: [(Bounds, &RegionFilter<'_>); 2] =
                [(above, &everything), (PLAN_BOX, &deleted)];
            for (section, accept) in empty {
                let result =
                    export_section_drawing(&sources, section, &request, &path, accept, &mut |_| {
                        Ok(())
                    });
                assert!(
                    matches!(&result, Err(LoadError::InvalidData(reason)) if reason.contains("no points")),
                    "{result:?}"
                );
                untouched();
                assert!(matches!(
                    preview_cut_regions(&sources, section, &request, accept, &mut |_| Ok(())),
                    Err(LoadError::InvalidData(_))
                ));
            }
            // Cancelled while reading, while tracing and while writing.
            for stage in [
                DrawingStage::Reading,
                DrawingStage::Tracing,
                DrawingStage::Writing,
            ] {
                let mut seen = Vec::new();
                let result = export_section_drawing(
                    &sources,
                    PLAN_BOX,
                    &request,
                    &path,
                    &everything,
                    &mut |step| {
                        seen.push(step.stage);
                        if step.stage == stage {
                            Err(LoadError::Cancelled)
                        } else {
                            Ok(())
                        }
                    },
                );
                assert!(matches!(result, Err(LoadError::Cancelled)), "{stage:?}");
                // Nothing of a later stage was begun.
                assert_eq!(seen.last(), Some(&stage));
                assert_eq!(seen.iter().filter(|seen| **seen == stage).count(), 1);
                untouched();
            }
            // A request that cannot be drawn.
            let nothing = DrawingRequest {
                points: false,
                fill: false,
                ..request
            };
            assert!(matches!(
                export_section_drawing(
                    &sources,
                    PLAN_BOX,
                    &nothing,
                    &path,
                    &everything,
                    &mut |_| panic!("nothing is read for a refused request"),
                ),
                Err(LoadError::InvalidData(_))
            ));
            untouched();
            // Without a cancel the earlier file is replaced.
            let stats = export_section_drawing(
                &sources,
                PLAN_BOX,
                &DrawingRequest {
                    max_points: 500,
                    ..request
                },
                &path,
                &everything,
                &mut |_| Ok(()),
            )
            .unwrap();
            assert_eq!(fs::metadata(&path).unwrap().len(), stats.bytes);
            assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
            fs::remove_file(&path).unwrap();
        }
        // Only DXF and DWG are written; the extension is looked at first.
        let result = export_section_drawing(
            &sources,
            PLAN_BOX,
            &request,
            &directory.path().join("plan.pdf"),
            &everything,
            &mut |_| panic!("nothing is read for a format that is not written"),
        );
        assert!(
            matches!(result, Err(LoadError::UnsupportedFormat(extension)) if extension == "pdf")
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn preview_gives_the_regions_of_an_export_in_the_scene_on_the_cut_plane() {
        let points = room_points();
        let cloud = indexed_cloud(&points, 4_096);
        // The scan stands elsewhere in the scene, mirrored in x.
        let transform = SourceTransform {
            scale: [-1.0, 1.0, 1.0],
            offset: [100.0, 200.0, 10.0],
        };
        let sources = [DrawingSource {
            source: RegionSource::new(&cloud.cloud, Some(&cloud.index), transform),
            name: "hall",
        }];
        let section = Bounds {
            min: [95.0, 199.0, 10.0],
            max: [101.0, 204.0, 11.1],
        };
        // Whether the request asks for points or a fill makes no difference
        // to a preview.
        let request = DrawingRequest {
            points: false,
            fill: false,
            ..DrawingRequest::default()
        };
        let mut stages = Vec::new();
        let preview = preview_cut_regions(&sources, section, &request, &everything, &mut |step| {
            if stages.last() != Some(&step.stage) {
                stages.push(step.stage);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(stages, [DrawingStage::Reading, DrawingStage::Tracing]);
        assert_eq!(preview.slab.view, DrawingView::Plan);
        assert_eq!(preview.stats.regions, 3);
        assert_eq!(preview.stats.drawn_points, 0);
        assert_eq!(preview.stats.bytes, 0);
        assert_eq!(preview.stats.slab_points, points.len() as u64);
        assert_eq!(preview.regions.len(), 3);
        // Every corner lies on the cut plane, the top face of the box.
        let corners = preview
            .regions
            .iter()
            .flat_map(|region| region.outer.iter().chain(region.holes.iter().flatten()));
        assert!(corners.clone().count() == 20 && corners.clone().all(|at| at[2] == 11.1));
        // Brought back to where the scan was, the walls are those of the
        // room.
        let back: Vec<CutRegion> = preview
            .regions
            .iter()
            .map(|region| CutRegion {
                outer: region
                    .outer
                    .iter()
                    .map(|at| [100.0 - at[0], at[1] - 200.0])
                    .collect(),
                holes: Vec::new(),
            })
            .collect();
        // Measured within 0.06 mm on the faces and 2.5 mm at the jambs.
        let (faces, openings) = room_errors(&back);
        assert!(faces < 0.001, "{faces}");
        assert!(openings < 0.006, "{openings}");

        // An export draws the same regions.
        let export = DrawingRequest {
            fill: true,
            ..request
        };
        let (drawing, stats) =
            section_drawing(&sources, section, &export, &everything, &mut |_| Ok(())).unwrap();
        let drawn = fills(&drawing);
        assert_eq!(drawn.len(), 3);
        for (fill, region) in drawn.iter().zip(&preview.regions) {
            let plan: Vec<[f64; 2]> = region.outer.iter().map(|at| [at[0], at[1]]).collect();
            assert_eq!(fill.outer, plan);
        }
        assert_eq!(
            DrawingStats {
                drawn_points: 0,
                point_spacing: 0.0,
                ..stats
            },
            preview.stats
        );

        // A vertical view lies on the face it looks at.
        let front = DrawingRequest::for_view(DrawingView::Front);
        let section = Bounds {
            min: [95.0, 201.5, 10.0],
            max: [101.0, 204.0, 11.1],
        };
        let preview =
            preview_cut_regions(&sources, section, &front, &everything, &mut |_| Ok(())).unwrap();
        assert!(!preview.regions.is_empty());
        assert!(preview
            .regions
            .iter()
            .flat_map(|region| &region.outer)
            .all(|at| at[1] == 201.5 && at[2] >= 10.0 && at[2] <= 11.1));
        assert_eq!(
            DrawingProgress {
                stage: DrawingStage::Reading,
                done: 1,
                total: 4
            }
            .fraction(),
            Some(0.25)
        );
        assert_eq!(
            DrawingProgress {
                stage: DrawingStage::Tracing,
                done: 0,
                total: 0
            }
            .fraction(),
            None
        );
    }

    #[test]
    fn a_stray_point_far_away_leaves_the_filled_cut_as_it_was() {
        // The section box spans the whole scan in plan, as one that is only
        // moved up and down does.
        let section = Bounds {
            min: [-3_000.0, -3_000.0, 0.0],
            max: [3_000.0, 3_000.0, 1.1],
        };
        let request = DrawingRequest::default();
        let preview = |points: &[Point]| {
            let cloud = indexed_cloud(points, 4_096);
            let sources = [DrawingSource {
                source: indexed(&cloud),
                name: "hall",
            }];
            preview_cut_regions(&sources, section, &request, &everything, &mut |_| Ok(())).unwrap()
        };
        let points = room_points();
        let clean = preview(&points);
        assert_eq!(clean.stats.grid_cell, Some(0.02));
        // One point through a window: 5 m above the slab, in the slab, and
        // 2 km away.
        for stray in [
            [300.0, 300.0, 6.0],
            [300.0, 300.0, 1.05],
            [2_000.0, 2_000.0, 6.0],
        ] {
            let mut with_stray = points.clone();
            with_stray.push(Point {
                xyz: stray,
                ..points[0]
            });
            let found = preview(&with_stray);
            // The cell that was asked, the walls where they are, the door
            // and the window open. Before, the grid lay over the bounds of
            // the scan: cells of 80 mm and faces 40 mm off at 300 m, and
            // at 2 km cells of 640 mm, the door closed and the faces 0.3 m
            // off.
            assert_eq!(found.stats.grid_cell, Some(0.02), "{stray:?}");
            assert_eq!(found.stats.direction_degrees, Some(0.0));
            assert_eq!((found.stats.regions, found.stats.vertices), (3, 20));
            assert!(found.stats.read_points > clean.stats.read_points);
            let regions: Vec<CutRegion> = found
                .regions
                .iter()
                .map(|region| CutRegion {
                    outer: region.outer.iter().map(|at| [at[0], at[1]]).collect(),
                    holes: Vec::new(),
                })
                .collect();
            // Measured as without the stray point: 0.06 mm and 2.3 mm.
            let (faces, openings) = room_errors(&regions);
            assert!(faces < 0.001, "{stray:?}: {faces}");
            assert!(openings < 0.006, "{stray:?}: {openings}");
        }
    }

    #[test]
    fn a_wall_thickness_or_grid_that_costs_too_much_is_refused_before_anything_is_read() {
        let points = room_points();
        let in_memory = records(&points);
        let sources = [DrawingSource {
            source: RegionSource::resident(&in_memory, SourceTransform::default()),
            name: "hall",
        }];
        // Half a metre typed as millimetres, and a grid of half a
        // millimetre.
        for request in [
            DrawingRequest {
                max_wall_thickness: 500.0,
                ..DrawingRequest::default()
            },
            DrawingRequest {
                grid: 0.0005,
                ..DrawingRequest::default()
            },
        ] {
            let mut unread = |_: DrawingProgress| -> Result<(), LoadError> {
                panic!("nothing is read for a refused request")
            };
            assert!(matches!(
                preview_cut_regions(&sources, PLAN_BOX, &request, &everything, &mut unread),
                Err(LoadError::InvalidData(_))
            ));
            assert!(matches!(
                section_drawing(&sources, PLAN_BOX, &request, &everything, &mut unread),
                Err(LoadError::InvalidData(_))
            ));
        }
    }

    #[test]
    fn a_cancel_during_the_trace_of_a_wide_grid_is_heard_at_the_next_step() {
        // The same room twice, 75 m apart in both directions: a grid of
        // 79 by 78 m, 15 million cells, of which every step of the trace
        // takes a tenth of a second and more.
        let points = room_points();
        let cloud = indexed_cloud(&points, 4_096);
        let sources = [[0.0, 0.0], [75.0, 75.0]].map(|[x, y]| DrawingSource {
            source: RegionSource::new(
                &cloud.cloud,
                Some(&cloud.index),
                SourceTransform {
                    scale: [1.0; 3],
                    offset: [x, y, 0.0],
                },
            ),
            name: "hall",
        });
        let section = Bounds {
            min: [-1.0, -1.0, 0.0],
            max: [80.0, 80.0, 1.1],
        };
        let mut tracing = 0;
        let result = preview_cut_regions(
            &sources,
            section,
            &DrawingRequest::default(),
            &everything,
            &mut |step| {
                if step.stage == DrawingStage::Tracing {
                    tracing += 1;
                    if tracing > 1 {
                        return Err(LoadError::Cancelled);
                    }
                }
                Ok(())
            },
        );
        // The first question is let through, the second one stops the
        // job. Counted in questions, only every 256th was passed on: the
        // steps between were never asked about, and this trace ran to its
        // end, 1.4 s later.
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(tracing, 2);
    }

    #[test]
    fn a_box_turned_with_a_room_draws_its_walls_along_the_axes_of_the_drawing() {
        // A room of 4.0 by 3.0 by 2.6 m turned 30 degrees and moved away
        // from the origin. The same room unturned tells which points each
        // slab must hold.
        let plain = box_room(&RoomSpec::default()).cloud_points();
        let turned_room = box_room(&RoomSpec::default()).transformed(30.0, [100.0, 200.0, 0.0]);
        let points = turned_room.cloud_points();
        let cloud = indexed_cloud(&points, 4_096);
        let sources = [DrawingSource {
            source: indexed(&cloud),
            name: "room",
        }];
        let count = |test: &dyn Fn([f64; 3]) -> bool| -> u64 {
            plain.iter().filter(|point| test(point.xyz)).count() as u64
        };
        let (sin, cos) = 30f64.to_radians().sin_cos();
        let center = [100.0 + 2.0 * cos - 1.5 * sin, 200.0 + 2.0 * sin + 1.5 * cos];
        let around = |half: [f64; 2], z: [f64; 2]| {
            OrientedBox::new(
                Bounds {
                    min: [center[0] - half[0], center[1] - half[1], z[0]],
                    max: [center[0] + half[0], center[1] + half[1], z[1]],
                },
                30.0,
            )
        };

        // A box along the model axes around the room finds the walls at 30
        // degrees.
        let found = wall_direction(&sources, turned_room.bounds(), &everything, &mut |_| Ok(()))
            .unwrap()
            .unwrap();
        assert!((found.rotation_degrees - 30.0).abs() < 0.05, "{found:?}");
        assert_eq!(found.change_degrees, found.rotation_degrees);
        assert!(found.slab_points > 0);
        // Turned that way, it finds nothing more to turn.
        let again = wall_direction(
            &sources,
            around([2.5, 2.0], [0.0, 2.6]),
            &everything,
            &mut |_| Ok(()),
        )
        .unwrap()
        .unwrap();
        assert!(again.change_degrees.abs() < 0.05, "{again:?}");
        assert!((again.rotation_degrees - 30.0).abs() < 0.05);

        // The plan of the turned box: the walls between 1.0 and 1.1 m, along
        // u and v, half a metre in from the corner of the box.
        let request = DrawingRequest {
            origin: DrawingOrigin::BoxCorner,
            ..DrawingRequest::default()
        };
        let plan_box = around([2.5, 2.0], [0.0, 1.1]);
        let (drawing, stats) =
            section_drawing(&sources, plan_box, &request, &everything, &mut |_| Ok(())).unwrap();
        assert_eq!(
            stats.slab_points,
            count(&|xyz| xyz[2] >= 1.0 && xyz[2] <= 1.1)
        );
        assert_eq!(stats.direction_degrees, Some(0.0));
        let regions = fills(&drawing);
        assert!(!regions.is_empty());
        let (mut low, mut high) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for region in &regions {
            let ring = &region.outer;
            for (index, at) in ring.iter().enumerate() {
                let next = ring[(index + 1) % ring.len()];
                let step = [next[0] - at[0], next[1] - at[1]];
                assert!(
                    step[0].abs() < 0.002 || step[1].abs() < 0.002,
                    "an edge off the axes: {at:?} to {next:?}"
                );
                for axis in 0..2 {
                    low[axis] = low[axis].min(at[axis]);
                    high[axis] = high[axis].max(at[axis]);
                }
            }
        }
        for (value, truth) in [(low[0], 0.5), (low[1], 0.5), (high[0], 4.5), (high[1], 3.5)] {
            assert!((value - truth).abs() < 0.06, "{low:?} {high:?}");
        }

        // A vertical section of 40 mm at the face of the box just in front
        // of the south wall: the wall itself, parallel to the cut plane, and
        // the first points of the two walls beside it.
        let front_box = around([2.5, 1.52], [0.5, 1.1]);
        let front = DrawingRequest {
            view: DrawingView::Front,
            thickness: Some(0.04),
            ..request
        };
        let slab =
            slab_from_section(front_box, DrawingView::Front, Some(0.04), front.origin).unwrap();
        assert!((slab.frame.right[0] - cos).abs() < 1e-12);
        assert!((slab.frame.right[1] - sin).abs() < 1e-12);
        assert_eq!(slab.frame.up, [0.0, 0.0, 1.0]);
        let (drawing, stats) =
            section_drawing(&sources, front_box, &front, &everything, &mut |_| Ok(())).unwrap();
        let truth = count(&|xyz| {
            xyz[1].abs() <= 0.02 && (0.5..=1.1).contains(&xyz[2]) && (-0.5..=4.5).contains(&xyz[0])
        });
        // 200 points along the wall in each of 30 rows, and a row of each
        // wall beside it.
        assert_eq!(truth, 6_060);
        assert_eq!(stats.slab_points, truth);
        // The wall runs along the drawing: from half a metre to 4.5 m.
        let along: Vec<f64> = drawing
            .entities
            .iter()
            .filter_map(|(_, entity)| match entity {
                DrawingEntity::Point { uv, .. } => Some(uv[0]),
                _ => None,
            })
            .collect();
        assert!(!along.is_empty());
        assert!(along.iter().all(|u| (0.49..=4.51).contains(u)), "{along:?}");
    }

    /// A storey of eight rooms in a row, each a scan of its own of 4.0 by
    /// 3.0 by 2.6 m with walls scanned on both faces, a door and a window,
    /// and a point every 12.5 mm: 4.8 million points in all.
    fn storey() -> Vec<IndexedCloud> {
        (0..8)
            .map(|room| {
                let shape = box_room(&RoomSpec {
                    spacing: 0.0125,
                    wall_thickness: Some(0.1),
                    openings: vec![
                        Opening::door(Wall::South, 1.0),
                        Opening::window(Wall::East, 0.8),
                    ],
                    ..RoomSpec::default()
                })
                .with_noise(Noise::Gaussian(0.002), room)
                .transformed(0.0, [5.0 * room as f64, 0.0, 0.0]);
                let points: Vec<Point> = shape
                    .points
                    .iter()
                    .map(|xyz| Point {
                        xyz: *xyz,
                        rgb: None,
                        intensity: None,
                        classification: None,
                    })
                    .collect();
                // Leaves of the size the application builds.
                indexed_cloud(&points, 65_536)
            })
            .collect()
    }

    /// Not a check but a measurement: what the jobs cost on a cloud of
    /// several million points. Every phase prints when it starts and ends,
    /// so that the memory of the process can be read beside it, and what
    /// one run of the job took.
    #[test]
    #[ignore = "measures time on 4.8 million points; run with --ignored --nocapture"]
    fn measure_a_storey_of_several_million_points() {
        use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

        let built = Instant::now();
        let clouds = storey();
        let total: u64 = clouds.iter().map(|cloud| cloud.cloud.total_points).sum();
        let leaves: usize = clouds
            .iter()
            .map(|cloud| cloud.index.intersecting_leaves(|_| true).len())
            .sum();
        eprintln!(
            "storey: {total} points in {} scans, {leaves} leaves, built in {:.1} s",
            clouds.len(),
            built.elapsed().as_secs_f64()
        );
        let with_index: Vec<DrawingSource<'_>> = clouds
            .iter()
            .map(|cloud| DrawingSource {
                source: indexed(cloud),
                name: "room",
            })
            .collect();
        let streamed: Vec<DrawingSource<'_>> = clouds
            .iter()
            .map(|cloud| DrawingSource {
                source: RegionSource::new(&cloud.cloud, None, SourceTransform::default()),
                name: "room",
            })
            .collect();
        let directory = tempfile::tempdir().unwrap();
        let clock = || {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis()
        };
        let phase = |name: &str, repeat: u32, job: &mut dyn FnMut() -> DrawingStats| {
            eprintln!("PHASE {name} start {}", clock());
            let start = Instant::now();
            let mut stats = DrawingStats::default();
            for _ in 0..repeat {
                stats = job();
            }
            let each = start.elapsed().as_secs_f64() * 1000.0 / repeat as f64;
            eprintln!(
                "PHASE {name} end {} : {each:.1} ms each, {stats:?}",
                clock()
            );
        };
        let storey_box = Bounds {
            min: [-1.0, -1.0, 0.0],
            max: [40.0, 4.0, 1.1],
        };
        let two_rooms = Bounds {
            min: [-1.0, -1.0, 0.0],
            max: [9.5, 4.0, 1.1],
        };
        let plan = DrawingRequest::default();
        let front = DrawingRequest {
            fill: true,
            ..DrawingRequest::for_view(DrawingView::Front)
        };
        let section_box = Bounds {
            min: [-1.0, 1.5, -1.0],
            max: [40.0, 4.0, 3.0],
        };
        let preview = |sources: &[DrawingSource<'_>], section: Bounds, request: &DrawingRequest| {
            preview_cut_regions(sources, section, request, &everything, &mut |_| Ok(()))
                .unwrap()
                .stats
        };
        let export = |section: Bounds, request: &DrawingRequest, name: &str| {
            export_section_drawing(
                &with_index,
                section,
                request,
                &directory.path().join(name),
                &everything,
                &mut |_| Ok(()),
            )
            .unwrap()
        };

        phase("idle", 1, &mut || {
            std::thread::sleep(Duration::from_millis(600));
            DrawingStats::default()
        });
        phase("preview plan, storey", 10, &mut || {
            preview(&with_index, storey_box, &plan)
        });
        phase("preview plan, two rooms", 10, &mut || {
            preview(&with_index, two_rooms, &plan)
        });
        phase("preview plan, two rooms, no index", 3, &mut || {
            preview(&streamed, two_rooms, &plan)
        });
        phase("preview section, storey", 10, &mut || {
            preview(&with_index, section_box, &front)
        });
        // An elevation: every point of the storey is in the slab.
        let elevation = DrawingRequest {
            thickness: None,
            ..front
        };
        let all = Bounds {
            min: [-1.0, -1.0, -1.0],
            max: [40.0, 4.0, 3.0],
        };
        phase("preview elevation, storey", 3, &mut || {
            preview(&with_index, all, &elevation)
        });
        // The same storey with one more room 75 m away in both directions:
        // the grid of the filled cut then spans 79 by 78 m, 15.4 million
        // cells, close to the most it may have.
        let mut spread = with_index.clone();
        spread.push(DrawingSource {
            source: RegionSource::new(
                &clouds[0].cloud,
                Some(&clouds[0].index),
                SourceTransform {
                    scale: [1.0; 3],
                    offset: [75.0, 75.0, 0.0],
                },
            ),
            name: "far",
        });
        let wide = Bounds {
            min: [-1.0, -1.0, 0.0],
            max: [80.0, 80.0, 1.1],
        };
        phase("preview plan, 79 by 78 m", 2, &mut || {
            preview(&spread, wide, &plan)
        });
        phase("export plan, storey, dwg", 3, &mut || {
            export(storey_box, &plan, "plan.dwg")
        });
        phase("export plan, storey, dxf", 3, &mut || {
            export(storey_box, &plan, "plan.dxf")
        });
        // The whole height of the box as the slab: the floor is in it, the
        // thinning meets its limit and the fill covers the floor.
        let deep = DrawingRequest {
            thickness: None,
            ..plan
        };
        phase("export plan of the whole height, dwg", 2, &mut || {
            export(storey_box, &deep, "deep.dwg")
        });
        let most = DrawingRequest {
            max_points: MAX_DRAWING_POINTS,
            ..deep
        };
        phase(
            "export plan of the whole height, most points, dwg",
            1,
            &mut || export(storey_box, &most, "most.dwg"),
        );
        phase("idle", 1, &mut || {
            std::thread::sleep(Duration::from_millis(600));
            DrawingStats::default()
        });
    }
}
