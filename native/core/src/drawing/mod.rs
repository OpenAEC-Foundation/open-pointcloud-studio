//! 2D drawing model for the section box: what a plan or a vertical section
//! holds before it is written as DXF or DWG.
//!
//! Every coordinate in the model is in scene units (metres) at scale 1:1, in
//! the plane of the cut: `u` to the right and `v` up as the viewer sees it.
//! The unit factor is applied once, by the writer, so a preview and an export
//! are made from the same numbers.

mod outline;
mod read;
mod section;
mod slab;
mod write;

use std::fmt;
use std::path::Path;

use super::LoadError;

pub use outline::{
    trace_cut_regions, CutOutline, CutRegion, OutlineOptions, CUT_MIN_POINTS_PER_CELL,
    DEFAULT_MIN_WALL_LENGTH, MIN_CUT_HOLE_AREA, SQUARE_TOLERANCE,
};
pub use read::{read_drawing, ReadDrawing};
pub use section::{
    export_section_drawing, preview_cut_regions, preview_section_drawing, section_drawing,
    wall_direction, CutPreview, DrawingProgress, DrawingSource, DrawingStage, PreviewRegion,
    WallDirection,
};
pub use slab::{
    collect_slab, slab_from_section, CutGrid, Slab, SlabCut, SlabOptions, SlabPoint,
    MAX_CUT_GRID_CELLS,
};
pub(crate) use write::{codec_error, codec_version, frame_active_view, layer_color};
pub use write::{write_drawing, write_drawing_progress};

/// Thinned scan points. With several scans one layer per scan
/// ([`source_point_layer`]), or one per class ([`class_point_layer`]).
pub const LAYER_POINTS: &str = "OPS-POINTS";
/// Solid fills of the material that the cut plane goes through.
pub const LAYER_CUT_FILL: &str = "OPS-CUT-FILL";
/// One closed polyline per ring of every fill.
pub const LAYER_CUT_OUTLINE: &str = "OPS-CUT-OUTLINE";
/// The rectangle of the section box as seen in the view.
pub const LAYER_FRAME: &str = "OPS-FRAME";
/// One text that says how the drawing was made.
pub const LAYER_INFO: &str = "OPS-INFO";

/// White and black both become the colour that a drawing program shows black
/// on a light background and white on a dark one.
pub const LAYER_RGB_CONTRAST: [u8; 3] = [255, 255, 255];
pub const LAYER_RGB_CUT_FILL: [u8; 3] = [128, 128, 128];
pub const LAYER_RGB_FRAME: [u8; 3] = [255, 170, 0];

/// The writer builds the whole drawing in memory, and the codec keeps every
/// entity as a record that has room for any kind of entity. Measured, a point
/// entity takes about 2.5 kB while a DXF is written and 2.7 kB for a DWG, with
/// or without a colour: about 1.1 GB at this limit and 0.4 GB at the default.
/// More than the limit is refused rather than left to run out of memory.
pub const MAX_DRAWING_POINTS: usize = 400_000;
pub const DEFAULT_DRAWING_POINTS: usize = 150_000;
pub const DEFAULT_POINT_SPACING: f64 = 0.005;
pub const DEFAULT_SLAB_THICKNESS: f64 = 0.10;
pub const MIN_SLAB_THICKNESS: f64 = 0.005;
pub const MAX_SLAB_THICKNESS: f64 = 5.0;
pub const DEFAULT_CUT_GRID: f64 = 0.02;
/// A finer grid than this holds no more than the noise of a scanner, and
/// the work of the filled cut grows with the wall thickness counted in
/// cells.
pub const MIN_CUT_GRID: f64 = 0.005;
pub const DEFAULT_MAX_WALL_THICKNESS: f64 = 0.50;
/// Two faces farther apart than this are never taken as one wall. The gap
/// between them is closed on a grid that is wider by that gap on every side,
/// so this also bounds what the filled cut costs, whatever a request asks.
pub const MAX_WALL_THICKNESS: f64 = 2.0;
pub const DEFAULT_MIN_WALL_THICKNESS: f64 = 0.05;

/// A layer table entry cannot be longer than this.
const MAX_LAYER_NAME_CHARS: usize = 255;

/// Unit of the written drawing. Scene units are taken as metres.
///
/// A drawing in millimetres that is opened again as a point cloud is a
/// thousand times larger than the scan it was made from: the DXF reader takes
/// the POINT coordinates as they are and does not apply the insertion units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawingUnits {
    #[default]
    Millimetres,
    Metres,
}

impl DrawingUnits {
    pub const ALL: [Self; 2] = [Self::Millimetres, Self::Metres];

    /// Drawing units per metre.
    pub fn factor(self) -> f64 {
        match self {
            Self::Millimetres => 1000.0,
            Self::Metres => 1.0,
        }
    }

    /// The insertion units code of the drawing header.
    pub fn insertion_units(self) -> i16 {
        match self {
            Self::Millimetres => 4,
            Self::Metres => 6,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Millimetres => "mm",
            Self::Metres => "m",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|units| units.key() == value)
    }
}

/// Where the zero of the drawing lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawingOrigin {
    /// A plan keeps model X and Y. A vertical view measures `u` from the left
    /// edge of the box as seen and keeps model Z as `v`, so levels read as
    /// true heights.
    #[default]
    Model,
    /// The lower left corner of the view is zero. Keeps the numbers small
    /// when the model lies far from its origin, as with national grid
    /// coordinates in millimetres.
    BoxCorner,
}

impl DrawingOrigin {
    pub const ALL: [Self; 2] = [Self::Model, Self::BoxCorner];

    pub fn key(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::BoxCorner => "box",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|origin| origin.key() == value)
    }
}

/// The face of the section box that is drawn. The cut plane is the face the
/// viewer looks at; the slab runs from that face into the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawingView {
    /// From above: the top face.
    #[default]
    Plan,
    /// Looking along +Y: the face at Y min.
    Front,
    /// Looking along -Y: the face at Y max.
    Back,
    /// Looking along +X: the face at X min.
    Left,
    /// Looking along -X: the face at X max.
    Right,
}

impl DrawingView {
    pub const ALL: [Self; 5] = [Self::Plan, Self::Front, Self::Back, Self::Left, Self::Right];

    pub fn key(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Front => "front",
            Self::Back => "back",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|view| view.key() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Front => "Section, front",
            Self::Back => "Section, back",
            Self::Left => "Section, left",
            Self::Right => "Section, right",
        }
    }

    /// The model axis the viewer looks along: the normal of the cut plane.
    pub fn depth_axis(self) -> usize {
        match self {
            Self::Plan => 2,
            Self::Front | Self::Back => 1,
            Self::Left | Self::Right => 0,
        }
    }
}

/// The cut plane as a 2D coordinate system in the model: `right` and `up` are
/// unit vectors in the plane, `origin` is the model position of drawing zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawingFrame {
    pub right: [f64; 3],
    pub up: [f64; 3],
    pub origin: [f64; 3],
}

impl DrawingFrame {
    /// Projects a model position onto the cut plane.
    pub fn to_uv(&self, xyz: [f64; 3]) -> [f64; 2] {
        let delta: [f64; 3] = std::array::from_fn(|axis| xyz[axis] - self.origin[axis]);
        [dot(delta, self.right), dot(delta, self.up)]
    }

    /// The model position, on the cut plane, of a drawing coordinate.
    pub fn to_world(&self, uv: [f64; 2]) -> [f64; 3] {
        std::array::from_fn(|axis| {
            self.origin[axis] + uv[0] * self.right[axis] + uv[1] * self.up[axis]
        })
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingFormat {
    Dxf,
    Dwg,
}

impl DrawingFormat {
    pub const ALL: [Self; 2] = [Self::Dxf, Self::Dwg];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Dxf => "dxf",
            Self::Dwg => "dwg",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        Self::ALL
            .into_iter()
            .find(|format| format.extension().eq_ignore_ascii_case(extension))
    }
}

impl fmt::Display for DrawingFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dxf => "DXF",
            Self::Dwg => "DWG",
        })
    }
}

/// File version of the drawing, for both formats. True colour exists from
/// R2004 on, so older versions are not offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DrawingVersion {
    R2004,
    R2010,
    #[default]
    R2013,
    R2018,
}

impl DrawingVersion {
    pub const ALL: [Self; 4] = [Self::R2004, Self::R2010, Self::R2013, Self::R2018];

    pub fn key(self) -> &'static str {
        match self {
            Self::R2004 => "r2004",
            Self::R2010 => "r2010",
            Self::R2013 => "r2013",
            Self::R2018 => "r2018",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|version| version.key() == value)
    }

    /// The version tag at the start of a DWG file and in the header of a DXF.
    pub fn tag(self) -> &'static str {
        match self {
            Self::R2004 => "AC1018",
            Self::R2010 => "AC1024",
            Self::R2013 => "AC1027",
            Self::R2018 => "AC1032",
        }
    }
}

/// How the scan points are spread over layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointLayers {
    /// One layer per scan when several scans are drawn, else one layer.
    #[default]
    Source,
    /// One layer per classification.
    Class,
}

impl PointLayers {
    pub const ALL: [Self; 2] = [Self::Source, Self::Class];

    pub fn key(self) -> &'static str {
        match self {
            Self::Source => "scan",
            Self::Class => "class",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|layers| layers.key() == value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointColor {
    /// Points take the colour of their layer.
    #[default]
    Layer,
    /// Points carry their scanned colour as true colour.
    Rgb,
}

impl PointColor {
    pub const ALL: [Self; 2] = [Self::Layer, Self::Rgb];

    pub fn key(self) -> &'static str {
        match self {
            Self::Layer => "layer",
            Self::Rgb => "rgb",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|color| color.key() == value)
    }
}

/// What a 2D drawing of the section box is to hold. Lengths are in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawingRequest {
    pub view: DrawingView,
    /// Depth of the slab behind the cut plane; `None` takes the whole box, as
    /// an elevation does. Clamped to the depth of the box.
    pub thickness: Option<f64>,
    pub points: bool,
    pub fill: bool,
    /// Cell of the occupancy grid that the fills are traced from, at least
    /// `MIN_CUT_GRID`.
    pub grid: f64,
    /// Two scanned faces at most this far apart are one wall; wider gaps stay
    /// open, so door and window openings do. At most `MAX_WALL_THICKNESS`.
    pub max_wall_thickness: f64,
    pub min_wall_thickness: f64,
    pub square: bool,
    pub units: DrawingUnits,
    pub origin: DrawingOrigin,
    /// The points are thinned to one per cell of this size.
    pub point_spacing: f64,
    /// When thinning leaves more points than this, the spacing doubles.
    pub max_points: usize,
    pub point_layers: PointLayers,
    pub color: PointColor,
    pub version: DrawingVersion,
}

impl DrawingRequest {
    /// The defaults for a view. A plan gets its cut filled; a vertical view
    /// starts as points only.
    pub fn for_view(view: DrawingView) -> Self {
        Self {
            view,
            thickness: Some(DEFAULT_SLAB_THICKNESS),
            points: true,
            fill: view == DrawingView::Plan,
            grid: DEFAULT_CUT_GRID,
            max_wall_thickness: DEFAULT_MAX_WALL_THICKNESS,
            min_wall_thickness: DEFAULT_MIN_WALL_THICKNESS,
            square: true,
            units: DrawingUnits::default(),
            origin: DrawingOrigin::default(),
            point_spacing: DEFAULT_POINT_SPACING,
            max_points: DEFAULT_DRAWING_POINTS,
            point_layers: PointLayers::default(),
            color: PointColor::default(),
            version: DrawingVersion::default(),
        }
    }

    /// Refuses a request that cannot give a drawing, before any point is read.
    pub fn validate(&self) -> Result<(), LoadError> {
        let invalid = |reason: &str| Err(LoadError::InvalidData(reason.into()));
        if !self.points && !self.fill {
            return invalid("a drawing needs points, a filled cut or both");
        }
        if let Some(thickness) = self.thickness {
            if !(MIN_SLAB_THICKNESS..=MAX_SLAB_THICKNESS).contains(&thickness) {
                return invalid("slab thickness must be between 0.005 and 5 m");
            }
        }
        let positive = |value: f64| value.is_finite() && value > 0.0;
        if !positive(self.point_spacing) {
            return invalid("point spacing must be above zero");
        }
        // Written so that a NaN fails the test.
        if !(self.grid.is_finite() && self.grid >= MIN_CUT_GRID) {
            return invalid("grid size must be at least 0.005 m");
        }
        if !positive(self.min_wall_thickness) || !positive(self.max_wall_thickness) {
            return invalid("wall thickness must be above zero");
        }
        if self.max_wall_thickness > MAX_WALL_THICKNESS {
            return invalid("largest wall thickness must be at most 2 m");
        }
        if self.min_wall_thickness > self.max_wall_thickness {
            return invalid("smallest wall thickness is above the largest");
        }
        if self.max_points == 0 || self.max_points > MAX_DRAWING_POINTS {
            return invalid(&format!(
                "a drawing holds between 1 and {} points",
                grouped(MAX_DRAWING_POINTS)
            ));
        }
        Ok(())
    }
}

impl Default for DrawingRequest {
    fn default() -> Self {
        Self::for_view(DrawingView::default())
    }
}

/// What an export did. Lengths are in metres, whatever the drawing units.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DrawingStats {
    /// Points of the scans that lie in the slab.
    pub slab_points: u64,
    /// Points that were read to find them: those of the octree leaves that
    /// touch the slab, and all points of a layer without an index.
    pub read_points: u64,
    /// Point entities in the drawing, after thinning.
    pub drawn_points: u64,
    /// The spacing the points were thinned to; above the requested spacing
    /// when the point limit made it double.
    pub point_spacing: f64,
    /// Filled regions, and the vertices of all their rings.
    pub regions: usize,
    pub vertices: usize,
    /// Regions left out because they are smaller than the smallest wall.
    pub dropped_regions: usize,
    /// Grid cell and main direction of the filled cut; `None` without a fill.
    /// The cell is larger than the one asked when the cloud is too sparse
    /// for it, or when the surfaces in the slab span more than the grid
    /// holds at that cell.
    pub grid_cell: Option<f64>,
    pub direction_degrees: Option<f64>,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawingLayer {
    pub name: String,
    pub rgb: [u8; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub enum DrawingEntity {
    /// `rgb` is `None` for a point that takes the colour of its layer.
    Point {
        uv: [f64; 2],
        rgb: Option<[u8; 3]>,
    },
    Polyline {
        points: Vec<[f64; 2]>,
        closed: bool,
    },
    /// A solid fill. A ring is closed by itself: the first vertex is not
    /// repeated at the end.
    Fill {
        outer: Vec<[f64; 2]>,
        holes: Vec<Vec<[f64; 2]>>,
    },
    /// One line of text, `at` its lower left corner, turned by `rotation`
    /// radians counter-clockwise about that corner.
    Text {
        at: [f64; 2],
        height: f64,
        rotation: f64,
        value: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Drawing2d {
    pub units: DrawingUnits,
    pub layers: Vec<DrawingLayer>,
    /// Each entity with the index of its layer.
    pub entities: Vec<(u16, DrawingEntity)>,
}

impl Drawing2d {
    pub fn new(units: DrawingUnits) -> Self {
        Self {
            units,
            layers: Vec::new(),
            entities: Vec::new(),
        }
    }

    /// The index of the layer with this name, added when it is new. Layer
    /// names do not differ by case, and the characters a layer table refuses
    /// are replaced, so a file name can be passed as it is.
    pub fn layer(&mut self, name: &str, rgb: [u8; 3]) -> Result<u16, LoadError> {
        let name = layer_name(name);
        let folded = name.to_uppercase();
        if let Some(index) = self
            .layers
            .iter()
            .position(|layer| layer.name.to_uppercase() == folded)
        {
            return Ok(index as u16);
        }
        let index = u16::try_from(self.layers.len())
            .map_err(|_| LoadError::InvalidData("too many drawing layers".into()))?;
        self.layers.push(DrawingLayer { name, rgb });
        Ok(index)
    }

    pub fn add_point(&mut self, layer: u16, uv: [f64; 2], rgb: Option<[u8; 3]>) {
        self.entities
            .push((layer, DrawingEntity::Point { uv, rgb }));
    }

    pub fn add_polyline(&mut self, layer: u16, points: Vec<[f64; 2]>, closed: bool) {
        let points = if closed { open_ring(points) } else { points };
        self.entities
            .push((layer, DrawingEntity::Polyline { points, closed }));
    }

    pub fn add_fill(&mut self, layer: u16, outer: Vec<[f64; 2]>, holes: Vec<Vec<[f64; 2]>>) {
        self.entities.push((
            layer,
            DrawingEntity::Fill {
                outer: open_ring(outer),
                holes: holes.into_iter().map(open_ring).collect(),
            },
        ));
    }

    pub fn add_text(&mut self, layer: u16, at: [f64; 2], height: f64, value: &str) {
        self.entities.push((
            layer,
            DrawingEntity::Text {
                at,
                height,
                rotation: 0.0,
                value: single_line(value),
            },
        ));
    }

    /// One region of cut material: the solid fill on [`LAYER_CUT_FILL`] and a
    /// closed polyline for the outer ring and for every hole on
    /// [`LAYER_CUT_OUTLINE`]. The fill comes first: a drawing program paints
    /// in the order of the entities, and the outlines lie on the edge of it.
    pub fn add_cut_region(
        &mut self,
        outer: Vec<[f64; 2]>,
        holes: Vec<Vec<[f64; 2]>>,
    ) -> Result<(), LoadError> {
        let fill = self.layer(LAYER_CUT_FILL, LAYER_RGB_CUT_FILL)?;
        let outline = self.layer(LAYER_CUT_OUTLINE, LAYER_RGB_CONTRAST)?;
        self.add_fill(fill, outer.clone(), holes.clone());
        self.add_polyline(outline, outer, true);
        for hole in holes {
            self.add_polyline(outline, hole, true);
        }
        Ok(())
    }

    /// The rectangle of the section box in the view, on [`LAYER_FRAME`].
    pub fn add_frame(&mut self, min: [f64; 2], max: [f64; 2]) -> Result<(), LoadError> {
        let layer = self.layer(LAYER_FRAME, LAYER_RGB_FRAME)?;
        let corners = vec![min, [max[0], min[1]], max, [min[0], max[1]]];
        self.add_polyline(layer, corners, true);
        Ok(())
    }

    /// The text that says how the drawing was made, on [`LAYER_INFO`].
    pub fn add_info(&mut self, at: [f64; 2], height: f64, value: &str) -> Result<(), LoadError> {
        let layer = self.layer(LAYER_INFO, LAYER_RGB_CONTRAST)?;
        self.add_text(layer, at, height, value);
        Ok(())
    }

    pub fn point_count(&self) -> usize {
        self.entities
            .iter()
            .filter(|(_, entity)| matches!(entity, DrawingEntity::Point { .. }))
            .count()
    }

    /// Lower left and upper right corner of everything drawn, in metres. A
    /// text counts with its insertion point only.
    pub fn extents(&self) -> Option<[[f64; 2]; 2]> {
        let mut extents: Option<[[f64; 2]; 2]> = None;
        for (_, entity) in &self.entities {
            entity.for_each_vertex(&mut |uv| {
                let [min, max] = extents.get_or_insert([uv, uv]);
                for axis in 0..2 {
                    min[axis] = min[axis].min(uv[axis]);
                    max[axis] = max[axis].max(uv[axis]);
                }
            });
        }
        extents
    }

    /// Checks what a drawing file cannot hold, so that the writer refuses the
    /// drawing before it touches the destination.
    pub fn validate(&self) -> Result<(), LoadError> {
        let invalid = |reason: &str| Err(LoadError::InvalidData(reason.into()));
        if self.point_count() > MAX_DRAWING_POINTS {
            return invalid(&format!(
                "a drawing holds at most {} points",
                grouped(MAX_DRAWING_POINTS)
            ));
        }
        for (layer, entity) in &self.entities {
            if usize::from(*layer) >= self.layers.len() {
                return invalid("drawing entity on an unknown layer");
            }
            let mut finite = true;
            entity.for_each_vertex(&mut |uv| finite &= uv[0].is_finite() && uv[1].is_finite());
            if !finite {
                return invalid("non-finite drawing coordinate");
            }
            match entity {
                DrawingEntity::Point { .. } => {}
                DrawingEntity::Polyline { points, closed } => {
                    if points.len() < if *closed { 3 } else { 2 } {
                        return invalid("drawing polyline with too few vertices");
                    }
                }
                DrawingEntity::Fill { outer, holes } => {
                    if outer.len() < 3 || holes.iter().any(|hole| hole.len() < 3) {
                        return invalid("drawing fill ring with fewer than three vertices");
                    }
                }
                DrawingEntity::Text { height, value, .. } => {
                    if !height.is_finite() || *height <= 0.0 {
                        return invalid("drawing text height must be above zero");
                    }
                    if value.is_empty() {
                        return invalid("empty drawing text");
                    }
                }
            }
        }
        Ok(())
    }
}

impl DrawingEntity {
    fn for_each_vertex(&self, visit: &mut impl FnMut([f64; 2])) {
        match self {
            Self::Point { uv, .. } => visit(*uv),
            Self::Polyline { points, .. } => points.iter().copied().for_each(visit),
            Self::Fill { outer, holes } => outer
                .iter()
                .chain(holes.iter().flatten())
                .copied()
                .for_each(visit),
            Self::Text { at, .. } => visit(*at),
        }
    }
}

/// A ring is stored without its closing vertex; a traced contour usually
/// ends on the vertex it started from.
fn open_ring(mut ring: Vec<[f64; 2]>) -> Vec<[f64; 2]> {
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    ring
}

/// A count with its thousands apart, for the texts that name a limit.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// A name a layer table accepts: the refused characters become `_`.
pub fn layer_name(raw: &str) -> String {
    let name: String = raw
        .trim()
        .chars()
        .map(|character| match character {
            '<' | '>' | '/' | '\\' | '"' | ':' | ';' | '?' | '*' | '|' | '=' | ',' | '`' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .take(MAX_LAYER_NAME_CHARS)
        .collect();
    if name.is_empty() {
        "_".into()
    } else {
        name
    }
}

/// The point layer of one scan when several scans are drawn, named after the
/// file stem of the scan.
pub fn source_point_layer(stem: &str) -> String {
    layer_name(&format!("{LAYER_POINTS}-{}", stem.trim()))
}

/// The point layer of one classification.
pub fn class_point_layer(classification: u8) -> String {
    format!("{LAYER_POINTS}-CLASS-{classification:02}")
}

/// The line on [`LAYER_INFO`]: the view, where the cut plane lies, how thick
/// the slab is, the units, and the model position of drawing zero.
pub fn drawing_info_text(
    view: DrawingView,
    frame: &DrawingFrame,
    thickness: f64,
    units: DrawingUnits,
) -> String {
    let axis = view.depth_axis();
    let [x, y, z] = frame.origin;
    format!(
        "{}; cut plane {} = {:.3} m; slab {:.3} m; scale 1:1; units {}; \
         drawing zero at model X {x:.3}, Y {y:.3}, Z {z:.3} m",
        view.label(),
        ["X", "Y", "Z"][axis],
        frame.origin[axis],
        thickness,
        units.key(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_round_trip_and_defaults_are_the_planned_ones() {
        for view in DrawingView::ALL {
            assert_eq!(DrawingView::from_key(view.key()), Some(view));
        }
        for units in DrawingUnits::ALL {
            assert_eq!(DrawingUnits::from_key(units.key()), Some(units));
        }
        for origin in DrawingOrigin::ALL {
            assert_eq!(DrawingOrigin::from_key(origin.key()), Some(origin));
        }
        for version in DrawingVersion::ALL {
            assert_eq!(DrawingVersion::from_key(version.key()), Some(version));
        }
        for layers in PointLayers::ALL {
            assert_eq!(PointLayers::from_key(layers.key()), Some(layers));
        }
        for color in PointColor::ALL {
            assert_eq!(PointColor::from_key(color.key()), Some(color));
        }
        assert_eq!(DrawingView::from_key("top"), None);

        let request = DrawingRequest::default();
        assert_eq!(request.view, DrawingView::Plan);
        assert_eq!(request.thickness, Some(0.10));
        assert_eq!(request.point_spacing, 0.005);
        assert_eq!(request.max_points, 150_000);
        assert_eq!(request.grid, 0.02);
        assert_eq!(request.max_wall_thickness, 0.50);
        assert_eq!(request.min_wall_thickness, 0.05);
        assert!(request.points && request.fill && request.square);
        assert_eq!(request.units, DrawingUnits::Millimetres);
        assert_eq!(request.origin, DrawingOrigin::Model);
        assert_eq!(request.version, DrawingVersion::R2013);
        assert!(!DrawingRequest::for_view(DrawingView::Front).fill);

        assert_eq!(DrawingUnits::Millimetres.factor(), 1000.0);
        assert_eq!(DrawingUnits::Millimetres.insertion_units(), 4);
        assert_eq!(DrawingUnits::Metres.factor(), 1.0);
        assert_eq!(DrawingUnits::Metres.insertion_units(), 6);
        assert_eq!(DrawingVersion::R2013.tag(), "AC1027");
    }

    #[test]
    fn format_follows_the_extension() {
        let format = |name: &str| DrawingFormat::from_path(Path::new(name));
        assert_eq!(format("plan.dxf"), Some(DrawingFormat::Dxf));
        assert_eq!(format("plan.DWG"), Some(DrawingFormat::Dwg));
        assert_eq!(format("plan.pdf"), None);
        assert_eq!(format("plan"), None);
        assert_eq!(DrawingFormat::Dwg.to_string(), "DWG");
    }

    #[test]
    fn request_refuses_what_cannot_be_drawn() {
        let valid = DrawingRequest::default();
        assert!(valid.validate().is_ok());
        let refused = |change: fn(&mut DrawingRequest)| {
            let mut request = valid;
            change(&mut request);
            matches!(request.validate(), Err(LoadError::InvalidData(_)))
        };
        assert!(refused(|request| {
            request.points = false;
            request.fill = false;
        }));
        assert!(refused(|request| request.thickness = Some(0.001)));
        assert!(refused(|request| request.thickness = Some(5.5)));
        assert!(refused(|request| request.thickness = Some(f64::NAN)));
        assert!(refused(|request| request.point_spacing = 0.0));
        assert!(refused(|request| request.grid = f64::INFINITY));
        assert!(refused(|request| request.grid = 0.0));
        assert!(refused(|request| request.grid = 0.004));
        assert!(refused(|request| request.min_wall_thickness = 0.8));
        // Half a metre typed as millimetres.
        assert!(refused(|request| request.max_wall_thickness = 500.0));
        assert!(refused(|request| request.max_wall_thickness = 2.5));
        let mut widest = valid;
        widest.grid = MIN_CUT_GRID;
        widest.max_wall_thickness = MAX_WALL_THICKNESS;
        assert!(widest.validate().is_ok());
        assert!(refused(|request| request.max_points = 0));
        assert!(refused(
            |request| request.max_points = MAX_DRAWING_POINTS + 1
        ));
        // The whole box depth, and the largest drawing that is still written.
        let mut elevation = valid;
        elevation.thickness = None;
        elevation.max_points = MAX_DRAWING_POINTS;
        assert!(elevation.validate().is_ok());
    }

    #[test]
    fn frame_projects_onto_the_cut_plane_and_back() {
        // A front view: u along X from the left box edge, v is model Z.
        let frame = DrawingFrame {
            right: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            origin: [10.0, 3.0, 0.0],
        };
        let uv = frame.to_uv([12.5, 4.25, 7.0]);
        assert_eq!(uv, [2.5, 7.0]);
        // Back in the model the depth is that of the cut plane.
        assert_eq!(frame.to_world(uv), [12.5, 3.0, 7.0]);
        assert_eq!(
            drawing_info_text(DrawingView::Front, &frame, 0.1, DrawingUnits::Millimetres),
            "Section, front; cut plane Y = 3.000 m; slab 0.100 m; scale 1:1; units mm; \
             drawing zero at model X 10.000, Y 3.000, Z 0.000 m"
        );
    }

    #[test]
    fn layers_are_found_again_and_get_a_valid_name() {
        let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
        let points = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
        assert_eq!(drawing.layer("ops-points", [1, 2, 3]).unwrap(), points);
        assert_eq!(drawing.layers[0].rgb, LAYER_RGB_CONTRAST);
        let scan = drawing
            .layer(&source_point_layer(" hall: 1/2 "), [0, 255, 0])
            .unwrap();
        assert_eq!(drawing.layers[scan as usize].name, "OPS-POINTS-hall_ 1_2");
        assert_eq!(class_point_layer(2), "OPS-POINTS-CLASS-02");
        assert_eq!(layer_name("  "), "_");
        assert_eq!(layer_name(&"a".repeat(400)).chars().count(), 255);
    }

    #[test]
    fn regions_frame_and_info_land_on_their_layers() {
        let mut drawing = Drawing2d::new(DrawingUnits::Metres);
        assert_eq!(drawing.extents(), None);
        let outer = vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0], [0.0, 0.0]];
        let hole = vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]];
        drawing.add_cut_region(outer, vec![hole]).unwrap();
        drawing.add_frame([-1.0, -1.0], [5.0, 4.0]).unwrap();
        drawing
            .add_info([-1.0, -1.5], 0.1, "Plan\ncut plane\tZ")
            .unwrap();
        let names: Vec<_> = drawing
            .layers
            .iter()
            .map(|layer| layer.name.as_str())
            .collect();
        assert_eq!(
            names,
            [LAYER_CUT_FILL, LAYER_CUT_OUTLINE, LAYER_FRAME, LAYER_INFO]
        );
        // The fill, its two outlines, the frame and the text.
        assert_eq!(drawing.entities.len(), 5);
        let (_, DrawingEntity::Fill { outer, holes }) = &drawing.entities[0] else {
            panic!("the fill comes before its outlines");
        };
        // The repeated closing vertex is dropped.
        assert_eq!(outer.len(), 4);
        assert_eq!(holes.len(), 1);
        for (layer, entity) in &drawing.entities[1..3] {
            assert_eq!(drawing.layers[usize::from(*layer)].name, LAYER_CUT_OUTLINE);
            assert!(matches!(
                entity,
                DrawingEntity::Polyline { points, closed: true } if points.len() == 4
            ));
        }
        let (_, DrawingEntity::Text { value, .. }) = &drawing.entities[4] else {
            panic!("the info text comes last");
        };
        assert_eq!(value, "Plan cut plane Z");
        assert_eq!(drawing.extents(), Some([[-1.0, -1.5], [5.0, 4.0]]));
        assert!(drawing.validate().is_ok());
    }

    #[test]
    fn validate_refuses_what_a_file_cannot_hold() {
        let refused = |build: fn(&mut Drawing2d, u16)| {
            let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
            let layer = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
            build(&mut drawing, layer);
            matches!(drawing.validate(), Err(LoadError::InvalidData(_)))
        };
        assert!(refused(|drawing, layer| drawing.add_point(
            layer + 1,
            [0.0, 0.0],
            None
        )));
        assert!(refused(|drawing, layer| drawing.add_point(
            layer,
            [f64::NAN, 0.0],
            None
        )));
        assert!(refused(|drawing, layer| drawing.add_polyline(
            layer,
            vec![[0.0, 0.0]],
            false
        )));
        assert!(refused(|drawing, layer| {
            drawing.add_polyline(layer, vec![[0.0, 0.0], [1.0, 0.0]], true)
        }));
        assert!(refused(|drawing, layer| {
            drawing.add_fill(layer, vec![[0.0, 0.0], [1.0, 0.0]], Vec::new())
        }));
        assert!(refused(|drawing, layer| drawing.add_text(
            layer,
            [0.0, 0.0],
            0.0,
            "text"
        )));
        assert!(refused(|drawing, layer| drawing.add_text(
            layer,
            [0.0, 0.0],
            0.1,
            " \n "
        )));
    }

    #[test]
    fn point_limit_counts_point_entities_and_is_named_in_the_refusal() {
        let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
        let layer = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
        for index in 0..MAX_DRAWING_POINTS {
            drawing.add_point(layer, [index as f64 * 0.005, 0.0], None);
        }
        // Other entities do not count towards the limit.
        drawing.add_frame([0.0, 0.0], [5000.0, 1.0]).unwrap();
        assert_eq!(drawing.point_count(), MAX_DRAWING_POINTS);
        assert!(drawing.validate().is_ok());
        drawing.add_point(layer, [0.0, 1.0], None);
        // Both refusals name the limit as it is, not a number of their own.
        let limit = grouped(MAX_DRAWING_POINTS);
        assert!(matches!(
            drawing.validate(),
            Err(LoadError::InvalidData(reason)) if reason.contains(&limit)
        ));
        let mut request = DrawingRequest::default();
        assert!(request.max_points <= MAX_DRAWING_POINTS);
        request.max_points = MAX_DRAWING_POINTS + 1;
        assert!(matches!(
            request.validate(),
            Err(LoadError::InvalidData(reason)) if reason.contains(&limit)
        ));
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
