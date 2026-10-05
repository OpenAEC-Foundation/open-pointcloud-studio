//! Detected faces: the flat surfaces in a region of one or more point
//! clouds, each with its outline, its class, the edges it shares with its
//! neighbours and the measured distance of the scan points to it; and the
//! round columns and pipes among what no flat face took.
//!
//! The work is two passes over the points of the region, read through
//! `region_source`, so from the leaves of an octree index that touch the
//! region or from one pass over a source without an index:
//!
//! 1. The points are reduced to one mean position per occupied voxel. When
//!    more voxels are occupied than the budget allows, the voxel size is
//!    doubled and reading goes on, so memory follows the budget and not the
//!    number of points. A scan whose points lie further apart than a voxel
//!    is wide gets larger voxels as well, up to four times the size asked
//!    for. Normals, region growing, the plane fits, the merging and the
//!    search for cylinders all work on this set.
//! 2. Every raw point is measured against the plane or cylinder of its
//!    voxel or of the voxels round it. That gives the residuals of every
//!    face over all its points, and a grid in the plane of every face that
//!    says where it has points and how far they lie from it. The outline of
//!    a face, with its openings, is traced from that grid.
//!
//! Coordinates in the result are those of the scene, for the placement of
//! the anchor layer stated in it. A layer that is moved afterwards takes its
//! faces along: the viewer meshes are built in the source frame of the
//! anchor layer, and `DetectedSurfaces::placed` gives the same faces for
//! another placement.

mod chart;
mod cylinder;
mod edges;
mod export;
mod segment;
mod voxel_cloud;

#[cfg(test)]
mod tests;

use crate::grid2d::{ring_signed_area, Mask, Region};
use crate::local_fit::{dot, unit};
use crate::region_source::{
    visit_region, visit_region_parallel, world_bounds, RegionFilter, RegionSource, SourceTransform,
};
use crate::{Bounds, LoadError, PointCloud};

use chart::{Charts, Frame, Tally, MAX_CHART_CELLS};
use segment::{estimate_normals, grow, refine, Sides};
use voxel_cloud::{VoxelCloud, NO_STATION};

#[cfg(test)]
pub(crate) use export::faces_model;
pub use export::{
    class_color, cylinder_color, deviation_color, deviation_legend, deviation_mesh, face_color,
    faces_json, flat_mesh, write_faces_cad, write_faces_ifc, write_faces_json, write_faces_obj,
    DeviationStop, DEFAULT_DEVIATION_CELLS, FACES_JSON_FORMAT, FACES_JSON_VERSION,
};

/// A raw point belongs to a face when it lies within this many times the
/// distance tolerance of its plane and the surface it lies in runs along
/// that plane. The residuals of a face are taken over those points, so no
/// deviation is larger than this window. Of a surface that stands square to
/// the plane, such as the reveal of an opening, only the points within the
/// tolerance itself count.
pub const MEASURE_WINDOW: f64 = 3.0;
/// A face within this angle of level is a floor or a ceiling, and one
/// within this angle of upright is a wall.
pub const FLAT_ANGLE_DEG: f64 = 10.0;
/// Two faces share an edge when their planes are at least this far from
/// parallel.
pub const MIN_EDGE_ANGLE_DEG: f64 = 20.0;
/// Shorter stretches of an edge are left out.
pub const MIN_EDGE_LENGTH: f64 = 0.10;
/// The most faces one job returns; of more, the largest are kept.
pub const MAX_FACES: usize = 4_096;
/// The widest gap in a face that can be asked to be closed. A wider one is
/// a setting in the wrong unit rather than a gap.
pub const MAX_GAP: f64 = 5.0;
/// The voxel lattice is laid out from a corner that is a whole number of
/// these voxels from the origin of the scene, so that it does not move with
/// the exact extent of the points.
const LATTICE_VOXELS: f64 = 4_096.0;

/// Settings of a detection. Lengths are in scene units, taken as metres.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceDetectConfig {
    /// How far a point may lie from a plane and still belong to it.
    pub distance_tolerance: f64,
    /// How far the surface at a point may be turned from a plane.
    pub angle_tolerance_deg: f64,
    /// Smaller faces are not reported.
    pub min_region_area: f64,
    /// Size of the voxels of the working set to start from. It is doubled
    /// for a region that holds more than the budget, and for points that
    /// lie further apart than a voxel is wide; `DetectedSurfaces::voxel_size`
    /// is the size used.
    pub voxel_size: f64,
    /// The most voxels the working set holds; the voxel size is doubled
    /// until the region fits.
    pub max_working_points: usize,
    /// A region only starts at a point where the surface is this flat (see
    /// `local_fit::surface_variation`).
    pub curvature_max: f64,
    /// Narrower strips are no face: the side of a beam, a part of a column.
    pub min_plane_width: f64,
    /// Cell size of the grid that the outline of a face is traced on.
    pub boundary_cell: f64,
    /// Gaps in a face up to this width are closed before tracing; at most
    /// `MAX_GAP`.
    pub max_gap: f64,
    /// Smaller holes in a face are filled.
    pub min_hole_area: f64,
    /// Whether round columns and pipes are looked for among the points that
    /// no flat face took.
    pub detect_cylinders: bool,
    /// The smallest and the largest radius of a cylinder.
    pub min_radius: f64,
    pub max_radius: f64,
    /// Shorter cylinders are not reported.
    pub min_cylinder_length: f64,
    /// A cylinder is reported when at least this much of its round was
    /// scanned.
    pub min_arc_deg: f64,
}

impl Default for SurfaceDetectConfig {
    fn default() -> Self {
        Self {
            distance_tolerance: 0.02,
            angle_tolerance_deg: 10.0,
            min_region_area: 0.25,
            voxel_size: 0.03,
            max_working_points: 1_500_000,
            curvature_max: 0.02,
            min_plane_width: 0.15,
            boundary_cell: 0.05,
            max_gap: 0.10,
            min_hole_area: 0.05,
            detect_cylinders: true,
            min_radius: 0.01,
            max_radius: 1.0,
            min_cylinder_length: 0.30,
            min_arc_deg: 90.0,
        }
    }
}

impl SurfaceDetectConfig {
    /// Refuse settings that cannot give a result.
    pub fn validate(&self) -> Result<(), LoadError> {
        let positive = [
            ("distance tolerance", self.distance_tolerance),
            ("minimum area", self.min_region_area),
            ("voxel size", self.voxel_size),
            ("flatness limit", self.curvature_max),
            ("minimum width", self.min_plane_width),
            ("boundary cell", self.boundary_cell),
            ("smallest radius", self.min_radius),
            ("cylinder length", self.min_cylinder_length),
        ];
        for (name, value) in positive {
            if !(value.is_finite() && value > 0.0) {
                return Err(LoadError::InvalidData(format!(
                    "the {name} of the face detection must be above zero"
                )));
            }
        }
        if !(self.min_hole_area.is_finite() && self.min_hole_area >= 0.0) {
            return Err(LoadError::InvalidData(
                "the hole area of the face detection cannot be negative".into(),
            ));
        }
        // Written so that a NaN fails the test.
        if !(0.0..=MAX_GAP).contains(&self.max_gap) {
            return Err(LoadError::InvalidData(format!(
                "the gap of the face detection must be 0 to {MAX_GAP} m"
            )));
        }
        if !(1.0..=45.0).contains(&self.angle_tolerance_deg) {
            return Err(LoadError::InvalidData(
                "the angle tolerance of the face detection must be 1 to 45 degrees".into(),
            ));
        }
        if !(self.max_radius.is_finite() && self.max_radius >= self.min_radius) {
            return Err(LoadError::InvalidData(
                "the largest radius of the face detection is below the smallest".into(),
            ));
        }
        if !(10.0..=360.0).contains(&self.min_arc_deg) {
            return Err(LoadError::InvalidData(
                "the arc of a cylinder must be 10 to 360 degrees".into(),
            ));
        }
        if self.max_working_points < 1_000 {
            return Err(LoadError::InvalidData(
                "the face detection needs room for at least 1,000 working points".into(),
            ));
        }
        Ok(())
    }
}

/// The steps of a detection, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceStage {
    /// First pass over the points: the working set.
    Reading,
    /// Normals, regions and planes.
    Segmenting,
    /// Second pass over the points: residuals and grids.
    Measuring,
    /// Outlines and edges.
    Outlining,
}

/// How far a detection is: `completed` of `total` within its stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceProgress {
    pub stage: SurfaceStage,
    pub completed: u64,
    pub total: u64,
}

impl SurfaceProgress {
    /// The part of the whole job that is done, from 0 to 1. The stages
    /// count for fixed shares; the two passes over the points take most.
    pub fn fraction(&self) -> f32 {
        let (before, share) = match self.stage {
            SurfaceStage::Reading => (0.0, 0.35),
            SurfaceStage::Segmenting => (0.35, 0.25),
            SurfaceStage::Measuring => (0.60, 0.35),
            SurfaceStage::Outlining => (0.95, 0.05),
        };
        let within = if self.total == 0 {
            1.0
        } else {
            (self.completed as f64 / self.total as f64).min(1.0)
        };
        (before + share * within) as f32
    }
}

/// One layer to detect faces in.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceSource<'a> {
    /// Where its points are read from and where the layer stands.
    pub points: RegionSource<'a>,
    /// The cloud of the layer, which knows the station that measured each
    /// point. Without it the faces of this layer are oriented by position.
    pub cloud: Option<&'a PointCloud>,
}

/// The distance of the points of a face to its plane.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Residuals {
    /// Raw points of the face: those within `MEASURE_WINDOW` tolerances
    /// whose surface runs along the face, and those within one tolerance of
    /// what stands against it.
    pub points: u64,
    /// Those of them within the distance tolerance.
    pub inliers: u64,
    /// Root of the mean squared distance.
    pub rms: f64,
    /// Mean distance with its sign: positive on the side of the normal.
    pub mean: f64,
    /// Mean distance without its sign.
    pub mean_abs: f64,
    /// 95 of 100 points lie nearer than this.
    pub p95: f64,
    /// The largest distance.
    pub max: f64,
}

/// What a face is, by the direction of its normal alone, not by what it
/// belongs to: the top of a table, a sill or a box is a `Floor` and the
/// front of a cabinet or an opened door leaf is a `Wall`. A caller that
/// sums areas by class has to tell the building from its contents itself,
/// by the height (`PlaneFace::origin`), the area and the edges of a face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FaceClass {
    /// Level, seen from above.
    Floor,
    /// Level, seen from below.
    Ceiling,
    /// Upright.
    Wall,
    /// Anything between: a roof plane, a ramp.
    Sloped,
}

impl FaceClass {
    /// The class of a face with this unit normal.
    pub fn of(normal: [f64; 3]) -> Self {
        let flat = FLAT_ANGLE_DEG.to_radians();
        if normal[2] >= flat.cos() {
            Self::Floor
        } else if normal[2] <= -flat.cos() {
            Self::Ceiling
        } else if normal[2].abs() <= flat.sin() {
            Self::Wall
        } else {
            Self::Sloped
        }
    }

    /// A lower-case English name, as used in the exports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Ceiling => "ceiling",
            Self::Wall => "wall",
            Self::Sloped => "sloped",
        }
    }
}

/// What decided the side a normal points to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalSource {
    /// The stations that measured the points of the face.
    Stations,
    /// The nearest station in the region; it is not known which station
    /// measured which point.
    NearestStation,
    /// No station is known: the side on which the next surface lies further
    /// from the face than on the other side, or on which alone there is one.
    /// Right for a face with the room it was scanned from in front of it; a
    /// face of a thin wall seen from the open is a guess.
    OpenSide,
    /// The middle of the box round the points in the region: no station is
    /// known and nothing lies within sight of the face. A guess.
    Centre,
}

impl NormalSource {
    /// A lower-case English name, as used in the exports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Stations => "stations",
            Self::NearestStation => "nearest_station",
            Self::OpenSide => "open_side",
            Self::Centre => "centre",
        }
    }
}

/// The grid in the plane of a face that its points were counted in.
/// Cell `(x, y)` has its corner at `origin + x * step_u + y * step_v`, and
/// cells are stored row by row from `y = 0`.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviationGrid {
    pub origin: [f64; 3],
    /// One cell to the right.
    pub step_u: [f64; 3],
    /// One cell up.
    pub step_v: [f64; 3],
    pub width: u32,
    pub height: u32,
    /// Raw points per cell.
    pub counts: Vec<u32>,
    /// Mean distance of the points of a cell to the plane, positive on the
    /// side of the normal; zero for a cell without points.
    pub means: Vec<f32>,
}

impl DeviationGrid {
    /// The position of a cell corner; corner `(x, y)` is the lower left one
    /// of cell `(x, y)`.
    pub fn corner(&self, x: u32, y: u32) -> [f64; 3] {
        std::array::from_fn(|axis| {
            self.origin[axis] + f64::from(x) * self.step_u[axis] + f64::from(y) * self.step_v[axis]
        })
    }

    /// Position of cell `(x, y)` in `counts` and `means`.
    pub fn index(&self, x: u32, y: u32) -> usize {
        y as usize * self.width as usize + x as usize
    }
}

/// One detected flat face.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaneFace {
    /// Number of the face, from 1, largest face first.
    pub id: u32,
    pub class: FaceClass,
    /// Unit normal, on the side the face was scanned from.
    pub normal: [f64; 3],
    /// A point of the plane, and the origin of the outline coordinates.
    pub origin: [f64; 3],
    /// Direction of the first outline coordinate: to the right for someone
    /// who looks at the face, and level. On a floor or ceiling it follows
    /// the largest wall the face touches, and x when it touches none.
    pub u: [f64; 3],
    /// Direction of the second outline coordinate: up.
    pub v: [f64; 3],
    /// The connected parts of the face, as rings in outline coordinates:
    /// an outer ring that runs counter-clockwise and the rings of its
    /// openings, clockwise. A ring is closed; its first corner is not
    /// repeated. Nearly always one part.
    pub patches: Vec<Region>,
    /// Area inside the outlines, openings left out.
    pub area: f64,
    /// The part of `area` that holds scan points: the rest was closed over
    /// a gap or filled in a small hole.
    pub covered_area: f64,
    /// Faces with the same number lie in one plane: the parts of a wall on
    /// either side of an opening.
    pub coplanar_group: u32,
    pub normal_source: NormalSource,
    pub residuals: Residuals,
    pub deviation: DeviationGrid,
}

impl PlaneFace {
    /// The plane is `normal . x = offset`.
    pub fn offset(&self) -> f64 {
        dot(self.normal, self.origin)
    }

    /// The part of the area that holds scan points, from 0 to 1. Low where
    /// a face was closed over gaps or seen through clutter.
    pub fn coverage(&self) -> f64 {
        if self.area > 0.0 {
            (self.covered_area / self.area).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The position of a point given in outline coordinates.
    pub fn point(&self, uv: [f64; 2]) -> [f64; 3] {
        std::array::from_fn(|axis| self.origin[axis] + uv[0] * self.u[axis] + uv[1] * self.v[axis])
    }

    /// Every ring of the outline as positions: per part its outer ring and
    /// then the rings of its openings. A ring is closed; its first corner
    /// is not repeated.
    pub fn rings(&self) -> Vec<Vec<[f64; 3]>> {
        self.patches
            .iter()
            .flat_map(|patch| std::iter::once(&patch.outer).chain(&patch.holes))
            .map(|ring| ring.iter().map(|corner| self.point(*corner)).collect())
            .collect()
    }

    /// Distance of a position to the plane, positive on the normal's side.
    pub fn signed_distance(&self, point: [f64; 3]) -> f64 {
        dot(self.normal, point) - self.offset()
    }
}

/// The grid round a cylinder that its points were counted in. Cell
/// `(x, y)` covers the arc from `first_angle + x * 360 / columns` degrees
/// from `arc_start` to one column further, and the length from
/// `first_along + y * step` to one `step` further along the axis from
/// `axis_start`. Cells are stored row by row from `y = 0`.
#[derive(Debug, Clone, PartialEq)]
pub struct RoundGrid {
    pub columns: u32,
    pub rows: u32,
    /// Length of a cell along the axis, and along the arc.
    pub step: f64,
    /// Where the first column begins, in degrees: at or just before
    /// `arc_start`.
    pub first_angle: f64,
    /// Where the first row begins: at or just before `axis_start`.
    pub first_along: f64,
    /// Raw points per cell.
    pub counts: Vec<u32>,
    /// Mean distance of the points of a cell to the surface, positive
    /// outside it; zero for a cell without points.
    pub means: Vec<f32>,
}

/// One detected cylinder: a round column, or a pipe.
#[derive(Debug, Clone, PartialEq)]
pub struct CylinderFace {
    /// Number of the cylinder; the numbers go on after those of the planes.
    pub id: u32,
    /// The ends of the axis, as far as the surface was scanned.
    pub axis_start: [f64; 3],
    pub axis_end: [f64; 3],
    pub radius: f64,
    /// Unit direction from the axis to where the scanned arc begins.
    pub arc_start: [f64; 3],
    /// Unit direction, square to the axis and to `arc_start`, that the arc
    /// runs towards from there.
    pub arc_side: [f64; 3],
    /// How much of the round was scanned, from `arc_start`: 360 minus the
    /// widest gap.
    pub arc_deg: f64,
    /// Whether the stations lie inside: a round shaft and not a column.
    /// Without stations a cylinder is taken as seen from outside.
    pub seen_from_inside: bool,
    /// Distances of the points to the surface, positive outside it.
    pub residuals: Residuals,
    pub deviation: RoundGrid,
}

impl CylinderFace {
    pub fn length(&self) -> f64 {
        edges::apart(self.axis_start, self.axis_end)
    }

    pub fn diameter(&self) -> f64 {
        2.0 * self.radius
    }

    /// Unit direction of the axis, from its start to its end.
    pub fn axis(&self) -> [f64; 3] {
        unit(crate::local_fit::difference(self.axis_end, self.axis_start))
            .unwrap_or([0.0, 0.0, 1.0])
    }

    /// Area of the scanned part of the surface.
    pub fn area(&self) -> f64 {
        self.radius * self.arc_deg.to_radians() * self.length()
    }

    /// Unit direction from the axis to the surface, an angle from
    /// `arc_start` along the arc.
    pub fn outward(&self, angle_deg: f64) -> [f64; 3] {
        let (sin, cos) = angle_deg.to_radians().sin_cos();
        std::array::from_fn(|axis| cos * self.arc_start[axis] + sin * self.arc_side[axis])
    }

    /// The position on the surface at an angle from `arc_start` and a
    /// distance along the axis from `axis_start`.
    pub fn point(&self, angle_deg: f64, along: f64) -> [f64; 3] {
        let (axis, out) = (self.axis(), self.outward(angle_deg));
        std::array::from_fn(|index| {
            self.axis_start[index] + along * axis[index] + self.radius * out[index]
        })
    }
}

/// A stretch of the line two faces share along which both are present.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceEdge {
    /// The numbers of the two faces, lower first.
    pub faces: [u32; 2],
    pub start: [f64; 3],
    pub end: [f64; 3],
    /// The angle between the faces on the side their normals point to: 90
    /// in the corner of a room, 270 round the corner of a pillar.
    pub angle_deg: f64,
}

impl SurfaceEdge {
    pub fn length(&self) -> f64 {
        edges::apart(self.start, self.end)
    }
}

/// The result of a detection.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedSurfaces {
    /// The faces, largest first.
    pub planes: Vec<PlaneFace>,
    /// The cylinders, largest first.
    pub cylinders: Vec<CylinderFace>,
    pub edges: Vec<SurfaceEdge>,
    /// The box round the points that took part; nothing without points.
    pub region: Option<Bounds>,
    /// The voxel size the working set ended with: the size asked for, or a
    /// doubling of it when the region held more than the budget or the
    /// points lie further apart than a voxel is wide.
    pub voxel_size: f64,
    /// How often the voxel size was doubled because the points lie further
    /// apart than a voxel is wide. A smaller region does not take these
    /// doublings back; the rest of the growth it does.
    pub density_doublings: u32,
    /// The cell size the outlines were traced with.
    pub boundary_cell: f64,
    /// Points read from the sources in one pass: those of the octree leaves
    /// that touch the region, or all points of a source without an index.
    pub read_points: u64,
    /// Points of the region that passed the filter.
    pub source_points: u64,
    /// Voxels of the working set.
    pub working_points: u64,
    /// Points that belong to a face or a cylinder of the result.
    pub assigned_points: u64,
    pub config: SurfaceDetectConfig,
    /// The placement of the anchor layer that the coordinates hold for.
    pub placement: SourceTransform,
}

impl DetectedSurfaces {
    fn empty(config: &SurfaceDetectConfig, placement: SourceTransform) -> Self {
        Self {
            planes: Vec::new(),
            cylinders: Vec::new(),
            edges: Vec::new(),
            region: None,
            voxel_size: config.voxel_size,
            density_doublings: 0,
            boundary_cell: config.boundary_cell,
            read_points: 0,
            source_points: 0,
            working_points: 0,
            assigned_points: 0,
            config: config.clone(),
            placement,
        }
    }

    /// The face with a number.
    pub fn face(&self, id: u32) -> Option<&PlaneFace> {
        self.planes
            .get((id as usize).checked_sub(1)?)
            .filter(|face| face.id == id)
    }

    /// The cylinder with a number.
    pub fn cylinder(&self, id: u32) -> Option<&CylinderFace> {
        self.cylinders
            .get((id as usize).checked_sub(1 + self.planes.len())?)
            .filter(|face| face.id == id)
    }

    /// Whether the voxels had to grow beyond the size asked for. Narrow
    /// faces are lost with every doubling, and faces in one plane on either
    /// side of a partition thinner than `max_gap` plus a voxel become one.
    /// A smaller region brings them back, as far as the growth came from
    /// the budget and not from the density (see `density_doublings`).
    pub fn is_coarse(&self) -> bool {
        self.voxel_size > self.config.voxel_size
    }

    /// The same faces after the anchor layer was moved or scaled to `now`:
    /// every position follows the layer, and areas and residuals are those
    /// of the new scale. Nothing for a placement with a zero scale.
    ///
    /// `voxel_size`, `boundary_cell` and `config` keep the values of the
    /// detection. A layer scaled unequally along its axes loses its
    /// cylinders: they would no longer be round.
    pub fn placed(&self, now: SourceTransform) -> Option<Self> {
        let from = self.placement;
        let factor: [f64; 3] = std::array::from_fn(|axis| now.scale[axis] / from.scale[axis]);
        if !factor
            .iter()
            .all(|value| value.is_finite() && value.abs() > f64::EPSILON)
        {
            return None;
        }
        let position = |xyz: [f64; 3]| -> [f64; 3] {
            std::array::from_fn(|axis| {
                (xyz[axis] - from.offset[axis]) * factor[axis] + now.offset[axis]
            })
        };
        let vector =
            |xyz: [f64; 3]| -> [f64; 3] { std::array::from_fn(|axis| xyz[axis] * factor[axis]) };
        let mut planes = Vec::with_capacity(self.planes.len());
        for face in &self.planes {
            // A plane n . x = d becomes (n / factor) . x' = d'; distances to
            // it shrink by the length of that vector.
            let turned: [f64; 3] = std::array::from_fn(|axis| face.normal[axis] / factor[axis]);
            let stretch = 1.0 / dot(turned, turned).sqrt();
            let normal = unit(turned)?;
            let frame = Frame::new(position(face.origin), normal);
            // A floor or ceiling stays laid out along the wall it was laid
            // out along.
            let frame = if chart::is_level(normal) {
                frame.turned_to(chart::towards_x(vector(face.u)))
            } else {
                frame
            };
            let ring = |ring: &Vec<[f64; 2]>| -> Vec<[f64; 2]> {
                ring.iter()
                    .map(|corner| frame.uv(position(face.point(*corner))))
                    .collect()
            };
            let patches: Vec<Region> = face
                .patches
                .iter()
                .map(|patch| {
                    let mut outer = ring(&patch.outer);
                    let mut holes: Vec<_> = patch.holes.iter().map(ring).collect();
                    // A mirrored layer turns every ring round.
                    if ring_signed_area(&outer) < 0.0 {
                        outer.reverse();
                        holes.iter_mut().for_each(|hole| hole.reverse());
                    }
                    Region { outer, holes }
                })
                .collect();
            let area: f64 = patches.iter().map(Region::area).sum();
            let scaled = |value: f64| value * stretch;
            planes.push(PlaneFace {
                id: face.id,
                class: FaceClass::of(normal),
                normal,
                origin: frame.origin,
                u: frame.u,
                v: frame.v,
                patches,
                area,
                covered_area: if face.area > 0.0 {
                    face.covered_area * area / face.area
                } else {
                    0.0
                },
                coplanar_group: face.coplanar_group,
                normal_source: face.normal_source,
                residuals: Residuals {
                    points: face.residuals.points,
                    inliers: face.residuals.inliers,
                    rms: scaled(face.residuals.rms),
                    mean: scaled(face.residuals.mean),
                    mean_abs: scaled(face.residuals.mean_abs),
                    p95: scaled(face.residuals.p95),
                    max: scaled(face.residuals.max),
                },
                deviation: DeviationGrid {
                    origin: position(face.deviation.origin),
                    step_u: vector(face.deviation.step_u),
                    step_v: vector(face.deviation.step_v),
                    width: face.deviation.width,
                    height: face.deviation.height,
                    counts: face.deviation.counts.clone(),
                    means: face
                        .deviation
                        .means
                        .iter()
                        .map(|value| (f64::from(*value) * stretch) as f32)
                        .collect(),
                },
            });
        }
        let edges = self
            .edges
            .iter()
            .map(|edge| {
                // Unequal scales change the angle between two faces; which
                // side is hollow they do not.
                let normal = |id: u32| planes.get(id as usize - 1).map(|face| face.normal);
                let angle_deg = match (normal(edge.faces[0]), normal(edge.faces[1])) {
                    (Some(a), Some(b)) => {
                        let between = dot(a, b).clamp(-1.0, 1.0).acos().to_degrees();
                        if edge.angle_deg < 180.0 {
                            180.0 - between
                        } else {
                            180.0 + between
                        }
                    }
                    _ => edge.angle_deg,
                };
                SurfaceEdge {
                    faces: edge.faces,
                    start: position(edge.start),
                    end: position(edge.end),
                    angle_deg,
                }
            })
            .collect();
        let region = self.region.map(|region| {
            let (a, b) = (position(region.min), position(region.max));
            Bounds {
                min: std::array::from_fn(|axis| a[axis].min(b[axis])),
                max: std::array::from_fn(|axis| a[axis].max(b[axis])),
            }
        });
        // A cylinder stays one when every axis is scaled alike, mirrored
        // or not.
        let size = factor[0].abs();
        let alike = factor
            .iter()
            .all(|value| (value.abs() - size).abs() <= 1e-12 * size);
        let direction = |xyz: [f64; 3]| unit(vector(xyz));
        let cylinders = if alike {
            self.cylinders.as_slice()
        } else {
            &[]
        }
        .iter()
        .filter_map(|face| {
            Some(CylinderFace {
                id: face.id,
                axis_start: position(face.axis_start),
                axis_end: position(face.axis_end),
                radius: face.radius * size,
                arc_start: direction(face.arc_start)?,
                arc_side: direction(face.arc_side)?,
                arc_deg: face.arc_deg,
                seen_from_inside: face.seen_from_inside,
                residuals: Residuals {
                    points: face.residuals.points,
                    inliers: face.residuals.inliers,
                    rms: face.residuals.rms * size,
                    mean: face.residuals.mean * size,
                    mean_abs: face.residuals.mean_abs * size,
                    p95: face.residuals.p95 * size,
                    max: face.residuals.max * size,
                },
                deviation: RoundGrid {
                    columns: face.deviation.columns,
                    rows: face.deviation.rows,
                    step: face.deviation.step * size,
                    first_angle: face.deviation.first_angle,
                    first_along: face.deviation.first_along * size,
                    counts: face.deviation.counts.clone(),
                    means: face
                        .deviation
                        .means
                        .iter()
                        .map(|value| (f64::from(*value) * size) as f32)
                        .collect(),
                },
            })
        })
        .collect();
        Some(Self {
            planes,
            cylinders,
            edges,
            region,
            voxel_size: self.voxel_size,
            density_doublings: self.density_doublings,
            boundary_cell: self.boundary_cell,
            read_points: self.read_points,
            source_points: self.source_points,
            working_points: self.working_points,
            assigned_points: self.assigned_points,
            config: self.config.clone(),
            placement: now,
        })
    }
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Detect the flat faces in a region of the scene.
///
/// - `sources` are the layers to take points from; a room is often spread
///   over several scan files. `anchor` is the position in `sources` of the
///   layer the result belongs to: the result holds for its placement and
///   follows it afterwards (see `DetectedSurfaces::placed`).
/// - `region` is a box in scene coordinates, faces included;
///   `region_source::EVERYWHERE` takes all points. With an index only the
///   leaves that touch the region are read, twice.
/// - `accept` decides per point, as in `region_source::visit_region`: this
///   is where deleted points and hidden classes are left out. It is called
///   from several threads in the second pass.
/// - `progress` is called often, by one thread at a time. Returning an
///   error, such as `LoadError::Cancelled`, stops the job; that error is
///   returned.
///
/// The result is the same on every run and for every number of threads. A
/// region without points, or without a face, gives an empty result.
pub fn detect_surfaces(
    sources: &[SurfaceSource<'_>],
    anchor: usize,
    region: Bounds,
    config: &SurfaceDetectConfig,
    accept: &RegionFilter<'_>,
    progress: &mut (dyn FnMut(SurfaceProgress) -> Result<(), LoadError> + Send),
) -> Result<DetectedSurfaces, LoadError> {
    let limits = Limits {
        faces: MAX_FACES,
        chart_cells: MAX_CHART_CELLS,
    };
    detect_within(sources, anchor, region, config, accept, progress, limits)
}

/// What a job may hold at most. Fixed for a real job; a test sets them low
/// to reach them with a small cloud.
#[derive(Debug, Clone, Copy)]
struct Limits {
    faces: usize,
    chart_cells: usize,
}

fn detect_within(
    sources: &[SurfaceSource<'_>],
    anchor: usize,
    region: Bounds,
    config: &SurfaceDetectConfig,
    accept: &RegionFilter<'_>,
    progress: &mut (dyn FnMut(SurfaceProgress) -> Result<(), LoadError> + Send),
    limits: Limits,
) -> Result<DetectedSurfaces, LoadError> {
    config.validate()?;
    let placement = sources
        .get(anchor)
        .map(|source| source.points.transform)
        .ok_or_else(|| LoadError::InvalidData("no layer to detect faces in".into()))?;
    // Written so that a NaN fails the test.
    if !(0..3).all(|axis| region.min[axis] <= region.max[axis]) {
        return Err(LoadError::InvalidData(
            "the region is not a box with its minimum below its maximum".into(),
        ));
    }
    let layers: Vec<RegionSource<'_>> = sources.iter().map(|source| source.points).collect();
    let mut result = DetectedSurfaces::empty(config, placement);
    let mut report = |stage: SurfaceStage, completed: u64, total: u64| {
        progress(SurfaceProgress {
            stage,
            completed,
            total,
        })
    };
    report(SurfaceStage::Reading, 0, 1)?;
    let Some(all) = world_bounds(&layers) else {
        return Ok(result);
    };
    let low: [f64; 3] = std::array::from_fn(|axis| region.min[axis].max(all.min[axis]));
    let high: [f64; 3] = std::array::from_fn(|axis| region.max[axis].min(all.max[axis]));
    if !(0..3)
        .all(|axis| low[axis] <= high[axis] && low[axis].is_finite() && high[axis].is_finite())
    {
        return Ok(result);
    }
    let lattice = config.voxel_size * LATTICE_VOXELS;
    let origin = low.map(|value| (value / lattice).floor() * lattice);
    let mut cloud = VoxelCloud::new(origin, config.voxel_size, config.max_working_points);
    cloud.fit_extent(
        (0..3)
            .map(|axis| high[axis] - origin[axis])
            .fold(0.0, f64::max),
    );

    // All stations in one list, in scene coordinates; a working point holds
    // a position in it.
    let mut stations: Vec<[f64; 3]> = Vec::new();
    let first_station: Vec<u32> = sources
        .iter()
        .map(|source| {
            let first = stations.len() as u32;
            if let Some(layer) = source.cloud {
                stations.extend(
                    layer
                        .scan_poses
                        .iter()
                        .map(|pose| source.points.transform.xyz(pose.position)),
                );
            }
            first
        })
        .collect();

    let mut taken: Option<Bounds> = None;
    let read = visit_region(
        &layers,
        region,
        accept,
        &mut |state| report(SurfaceStage::Reading, state.read, state.total),
        &mut |source, batch| {
            let layer = sources[source].cloud;
            for record in batch {
                let xyz = record.point.xyz;
                if !xyz.iter().all(|value| value.is_finite()) {
                    continue;
                }
                let station = layer
                    .and_then(|layer| layer.station_of(record.ordinal))
                    .map_or(NO_STATION, |station| first_station[source] + station as u32);
                cloud.add(xyz, station);
                let bounds = taken.get_or_insert(Bounds { min: xyz, max: xyz });
                for (axis, value) in xyz.into_iter().enumerate() {
                    bounds.min[axis] = bounds.min[axis].min(value);
                    bounds.max[axis] = bounds.max[axis].max(value);
                }
            }
            Ok(())
        },
    )?;
    result.read_points = read.read;
    result.source_points = read.accepted;
    result.voxel_size = cloud.voxel();
    let Some(taken) = taken else {
        return Ok(result);
    };
    // Regions grow from voxel to neighbouring voxel, so a scan that is
    // thinner than the voxels gets larger ones.
    result.density_doublings = cloud.fit_density();
    result.voxel_size = cloud.voxel();
    cloud.finish();
    result.region = Some(taken);
    result.working_points = cloud.len() as u64;

    // Normals, growing and the rest each count for a third.
    let working = cloud.len() as u64;
    let normals = estimate_normals(&cloud, config.distance_tolerance, &mut |done| {
        report(SurfaceStage::Segmenting, done, 3 * working)
    })?;
    let grown = grow(
        &cloud,
        &normals,
        config,
        limits.faces,
        &mut |tried, seeds| {
            let share = if seeds == 0 {
                working
            } else {
                (tried as f64 / seeds as f64 * working as f64) as u64
            };
            report(SurfaceStage::Segmenting, working + share, 3 * working)
        },
    )?;
    report(SurfaceStage::Segmenting, 2 * working, 3 * working)?;
    let to_local =
        |xyz: [f64; 3]| -> [f64; 3] { std::array::from_fn(|axis| xyz[axis] - origin[axis]) };
    let local_stations: Vec<[f64; 3]> = stations.iter().map(|station| to_local(*station)).collect();
    let inside: Vec<bool> = stations
        .iter()
        .map(|station| crate::region_source::contains(region, *station))
        .collect();
    let sides = Sides {
        stations: &local_stations,
        inside: &inside,
        centre: to_local(taken.center()),
    };
    let mut segmentation = refine(&cloud, grown, config, &sides);
    if config.detect_cylinders {
        let found = cylinder::detect(&cloud, &normals, &segmentation.labels, config, &mut || {
            report(SurfaceStage::Segmenting, 2 * working, 3 * working)
        })?;
        segmentation.add_cylinders(&cloud, found, &sides);
    }
    segmentation.link(&cloud);
    report(SurfaceStage::Segmenting, 3 * working, 3 * working)?;
    if segmentation.planes.is_empty() && segmentation.cylinders.is_empty() {
        return Ok(result);
    }

    let charts = Charts::new(&cloud, &segmentation, config, limits.chart_cells)?;
    let (tallies, _) = visit_region_parallel(
        &layers,
        region,
        accept,
        &mut |state| report(SurfaceStage::Measuring, state.read, state.total),
        &|| vec![Tally::new(); charts.len()],
        &|tallies, _, batch| {
            for record in batch {
                charts.measure(
                    tallies,
                    &cloud,
                    &segmentation,
                    &normals,
                    to_local(record.point.xyz),
                );
            }
            Ok(())
        },
    )?;
    drop(normals);
    let mut totals = vec![Tally::new(); charts.len()];
    for tallies in &tallies {
        for (total, tally) in totals.iter_mut().zip(tallies) {
            total.merge(tally);
        }
    }
    drop(tallies);
    let window = MEASURE_WINDOW * config.distance_tolerance;
    let (mut measured, rounds) = charts.finish();
    result.boundary_cell = measured
        .first()
        .map_or(config.boundary_cell, |m| m.grid.cell);

    // Outlines, and with them the faces that remain.
    let count = measured.len() as u64;
    let mut outlines = Vec::with_capacity(measured.len());
    let mut steps = 0u64;
    for (index, chart) in measured.iter().enumerate() {
        report(SurfaceStage::Outlining, index as u64, count)?;
        outlines.push(chart.outline(config, &mut || {
            steps += 1;
            if steps.is_multiple_of(4_096) {
                report(SurfaceStage::Outlining, index as u64, count)
            } else {
                Ok(())
            }
        })?);
    }
    let kept: Vec<usize> = (0..measured.len())
        .filter(|index| !outlines[*index].patches.is_empty() && totals[*index].points() > 0)
        .collect();
    let position_of: Vec<Option<usize>> = {
        let mut positions = vec![None; measured.len()];
        for (position, index) in kept.iter().enumerate() {
            positions[*index] = Some(position);
        }
        positions
    };
    let frames: Vec<Frame> = kept.iter().map(|index| measured[*index].frame).collect();
    let masks: Vec<&Mask> = kept.iter().map(|index| &outlines[*index].mask).collect();
    let pairs: Vec<[usize; 2]> = segmentation
        .pairs
        .iter()
        .filter_map(|(a, b)| Some([position_of[*a as usize - 1]?, position_of[*b as usize - 1]?]))
        .collect();
    let reach = 2.0 * result.boundary_cell + config.distance_tolerance;
    let mut lines = edges::intersections(&frames, &masks, &pairs, reach);
    // An end at a corner of three faces is put on it here, and stays there
    // even if an outline could not be moved onto the edge.
    edges::snap_ends(&frames, &mut lines, reach);
    let mut patches: Vec<Vec<Region>> = kept
        .iter()
        .map(|index| std::mem::take(&mut outlines[*index].patches))
        .collect();
    // The corners of an outline lie at the outermost scan points whatever
    // the cell, so they are moved no further than with the cell asked for:
    // with the reach of a coarsened cell a door head would be put on the
    // line of the ceiling.
    let across = reach.min(2.0 * config.boundary_cell + config.distance_tolerance);
    edges::snap_outlines(
        &frames,
        &mut patches,
        &lines,
        across,
        reach,
        result.boundary_cell,
    );
    edges::tighten_ends(&frames, &patches, &mut lines, reach);
    edges::keep_edges(&mut lines);

    // Largest first; the numbers follow that order. The outline decides
    // whether a face is large enough: the count of voxels overstates it,
    // and moving an outline onto the edges can leave nothing of a narrow
    // one.
    let areas: Vec<f64> = patches
        .iter()
        .map(|patches| patches.iter().map(Region::area).sum())
        .collect();
    let mut order: Vec<usize> = (0..kept.len())
        .filter(|position| areas[*position] >= config.min_region_area)
        .collect();
    order.sort_by(|a, b| areas[*b].total_cmp(&areas[*a]).then(a.cmp(b)));
    let mut id_of = vec![0u32; kept.len()];
    for (rank, position) in order.iter().enumerate() {
        id_of[*position] = rank as u32 + 1;
    }
    let mut group_numbers: Vec<u32> = Vec::new();
    for position in &order {
        let index = kept[*position];
        let chart = &mut measured[index];
        let frame = chart.frame;
        let group = segmentation.coplanar_groups[index];
        let coplanar_group = match group_numbers.iter().position(|known| *known == group) {
            Some(found) => found as u32 + 1,
            None => {
                group_numbers.push(group);
                group_numbers.len() as u32
            }
        };
        let cell = chart.grid.cell;
        let step = |direction: [f64; 3]| direction.map(|value| value * cell);
        result.planes.push(PlaneFace {
            id: id_of[*position],
            class: FaceClass::of(frame.normal),
            normal: frame.normal,
            origin: add(frame.origin, origin),
            u: frame.u,
            v: frame.v,
            patches: std::mem::take(&mut patches[*position]),
            area: areas[*position],
            covered_area: outlines[index].covered_share * areas[*position],
            coplanar_group,
            normal_source: segmentation.normal_sources[index],
            residuals: totals[index].residuals(window),
            deviation: DeviationGrid {
                origin: add(frame.point(chart.grid.origin), origin),
                step_u: step(frame.u),
                step_v: step(frame.v),
                width: chart.grid.width,
                height: chart.grid.height,
                counts: std::mem::take(&mut chart.counts),
                means: std::mem::take(&mut chart.means),
            },
        });
    }
    for line in &lines {
        let mut faces = [id_of[line.faces[0]], id_of[line.faces[1]]];
        // An edge with a face that was dropped for its area.
        if faces.contains(&0) {
            continue;
        }
        faces.sort_unstable();
        for segment in &line.segments {
            let [start, end] = edges::segment_ends(line, *segment);
            result.edges.push(SurfaceEdge {
                faces,
                start: add(start, origin),
                end: add(end, origin),
                angle_deg: line.angle_deg,
            });
        }
    }
    // The cylinders, as far as they were scanned: largest first, numbered
    // on from the planes.
    for (index, round) in rounds.iter().enumerate() {
        let Some(mantle) = round.mantle(config) else {
            continue;
        };
        let tally = &totals[measured.len() + index];
        let length = mantle.along[1] - mantle.along[0];
        if tally.points() == 0
            || length < config.min_cylinder_length
            || mantle.arc_deg < config.min_arc_deg
        {
            continue;
        }
        let on_axis = |along: f64| -> [f64; 3] {
            std::array::from_fn(|axis| {
                mantle.cylinder.point[axis] + along * mantle.cylinder.axis[axis] + origin[axis]
            })
        };
        result.cylinders.push(CylinderFace {
            id: 0,
            axis_start: on_axis(mantle.along[0]),
            axis_end: on_axis(mantle.along[1]),
            radius: mantle.cylinder.radius,
            arc_start: mantle.arc_start,
            arc_side: mantle.side,
            arc_deg: mantle.arc_deg,
            seen_from_inside: segmentation.hollow[index],
            residuals: tally.residuals(window),
            deviation: RoundGrid {
                columns: mantle.columns,
                rows: mantle.rows,
                step: mantle.step,
                first_angle: mantle.first_angle,
                first_along: mantle.first_along - mantle.along[0],
                counts: mantle.counts,
                means: mantle.means,
            },
        });
    }
    result
        .cylinders
        .sort_by(|a, b| b.area().total_cmp(&a.area()));
    let planes = result.planes.len() as u32;
    for (rank, face) in result.cylinders.iter_mut().enumerate() {
        face.id = planes + rank as u32 + 1;
    }
    // Only what is in the result counts: a plane whose outline came out
    // too small and a cylinder that was too short keep their points out.
    result.assigned_points = result
        .planes
        .iter()
        .map(|face| face.residuals.points)
        .chain(result.cylinders.iter().map(|face| face.residuals.points))
        .sum();
    result.edges.sort_by(|a, b| {
        a.faces.cmp(&b.faces).then(
            a.start
                .iter()
                .zip(&b.start)
                .map(|(a, b)| a.total_cmp(b))
                .find(|order| order.is_ne())
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    report(SurfaceStage::Outlining, count, count)?;
    Ok(result)
}
