//! Reads a DXF or DWG file into the model of a 2D drawing, so that a drawing
//! made elsewhere, or one written earlier, can be looked at in the
//! application.
//!
//! What a plan or a section needs is read: points, lines, polylines with
//! their arcs, circles, arcs, ellipses, solid fills with their holes, two
//! dimensional solids, texts and block references, with the blocks they
//! insert. Curves become polylines. Everything else is counted by its type,
//! and 3D content such as meshes and polyface meshes is counted apart.

use std::collections::{BTreeMap, HashMap};
use std::f64::consts::TAU;
use std::path::Path;

use cadcodec::entities::hatch::{BoundaryEdge, BoundaryPath};
use cadcodec::entities::mtext_format::{parse_mtext, parse_plain_text};
use cadcodec::entities::{
    AttachmentPoint, Dimension, EntityType, Hatch, Insert, MText, Text, TextHorizontalAlignment,
    TextVerticalAlignment,
};
use cadcodec::{CadDocument, Color, DwgReader, DxfError, DxfReader, Vector3};

use super::{
    dimension_axes, dimension_shape, Drawing2d, DrawingEntity, DrawingFormat, DrawingUnits,
    LAYER_RGB_CONTRAST,
};
use crate::LoadError;

/// Blocks inside blocks are followed this deep; a deeper or circular
/// reference is counted and left out.
const MAX_BLOCK_DEPTH: usize = 16;
/// Block references are expanded until the drawing holds this many
/// entities, so that a large array cannot fill the memory.
const MAX_ENTITIES: usize = 4_000_000;
/// A full circle becomes this many segments; an arc as many as its share.
const CIRCLE_SEGMENTS: f64 = 72.0;
/// The width of a character as a part of the text height, to place texts
/// that are centred or aligned right.
const CHARACTER_WIDTH: f64 = 0.6;

/// A drawing file as it was read.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadDrawing {
    /// Coordinates in metres, as every drawing model has them; `units` are
    /// those the file names, millimetres when it names none.
    pub drawing: Drawing2d,
    /// Whether the header of the file names its units.
    pub units_named: bool,
    /// Entities that are not shown, by their type, with how many there were.
    pub skipped: BTreeMap<String, usize>,
    /// 3D entities, such as meshes, polyface meshes and solids, left out.
    pub skipped_3d: usize,
    /// Block references that were drawn, nested ones included.
    pub inserts: usize,
    /// Layers that are switched off or frozen in the file.
    pub hidden_layers: Vec<String>,
}

/// Read a `.dxf` or `.dwg` file, whichever its extension names.
pub fn read_drawing(path: &Path) -> Result<ReadDrawing, LoadError> {
    let format = DrawingFormat::from_path(path).ok_or_else(|| {
        LoadError::UnsupportedFormat(
            path.extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase(),
        )
    })?;
    let document = match format {
        DrawingFormat::Dxf => DxfReader::from_file(path)
            .and_then(DxfReader::read)
            .map_err(read_error)?,
        DrawingFormat::Dwg => DwgReader::from_file(path)
            .and_then(|mut reader| reader.read())
            .map_err(read_error)?,
    };
    Ok(read_document(&document))
}

fn read_error(error: DxfError) -> LoadError {
    match error {
        DxfError::Io(error) => LoadError::Io(error),
        other => LoadError::InvalidData(format!("drawing could not be read: {other}")),
    }
}

/// Metres per drawing unit for the insertion units of a header, `None` for
/// a drawing without units.
fn metres_per_unit(code: i16) -> Option<f64> {
    Some(match code {
        1 => 0.0254,
        2 => 0.3048,
        3 => 1609.344,
        4 => 0.001,
        5 => 0.01,
        6 => 1.0,
        7 => 1000.0,
        8 => 0.0254e-6,
        9 => 0.0254e-3,
        10 => 0.9144,
        11 => 1e-10,
        12 => 1e-9,
        13 => 1e-6,
        14 => 0.1,
        15 => 10.0,
        16 => 100.0,
        17 => 1e9,
        21 => 1200.0 / 3937.0,
        _ => return None,
    })
}

/// The model of a drawing that the codec has read.
pub(crate) fn read_document(document: &CadDocument) -> ReadDrawing {
    let code = document.header.insertion_units;
    let named = metres_per_unit(code);
    // Without units a drawing of a building is most likely in millimetres.
    let to_metres = named.unwrap_or(0.001);
    let units = if code == 4 || named.is_none() {
        DrawingUnits::Millimetres
    } else {
        DrawingUnits::Metres
    };
    let mut reader = Reader {
        document,
        drawing: Drawing2d::new(units),
        layers: HashMap::new(),
        skipped: BTreeMap::new(),
        skipped_3d: 0,
        inserts: 0,
        blocks: Vec::new(),
    };
    let base = Affine::scale(to_metres, to_metres);
    let mut model: Vec<&EntityType> = document.model_space_entities().collect();
    if model.is_empty() {
        // A file whose model space lists no entities: every entity that no
        // other block owns is taken as drawn.
        let blocks: std::collections::HashSet<_> = document
            .block_records
            .iter()
            .filter(|record| !record.name.eq_ignore_ascii_case("*Model_Space"))
            .flat_map(|record| record.entity_handles.iter().copied())
            .collect();
        model = document
            .entities()
            .filter(|entity| !blocks.contains(&entity.common().handle))
            .collect();
    }
    for entity in model {
        reader.entity(entity, &base, None);
    }
    let mut drawing = reader.drawing;
    in_table_order(&mut drawing, document);
    let hidden_layers = document
        .layers
        .iter()
        .filter(|layer| layer.is_off() || layer.is_frozen())
        .map(|layer| super::layer_name(&layer.name))
        .collect();
    ReadDrawing {
        drawing,
        units_named: named.is_some(),
        skipped: reader.skipped,
        skipped_3d: reader.skipped_3d,
        inserts: reader.inserts,
        hidden_layers,
    }
}

/// Put the layers of a drawing in the order of the layer table of the file,
/// as a drawing program lists them, rather than in the order their first
/// entity was met. A layer the table does not hold comes last.
fn in_table_order(drawing: &mut Drawing2d, document: &CadDocument) {
    let table: HashMap<String, usize> = document
        .layers
        .iter()
        .enumerate()
        .map(|(place, layer)| (super::layer_name(&layer.name).to_uppercase(), place))
        .collect();
    let mut order: Vec<usize> = (0..drawing.layers.len()).collect();
    order.sort_by_key(|index| {
        table
            .get(&drawing.layers[*index].name.to_uppercase())
            .copied()
            .unwrap_or(usize::MAX)
    });
    let mut new_index = vec![0u16; order.len()];
    for (place, old) in order.iter().enumerate() {
        new_index[*old] = place as u16;
    }
    let mut layers: Vec<Option<_>> = drawing.layers.drain(..).map(Some).collect();
    drawing.layers = order.iter().filter_map(|old| layers[*old].take()).collect();
    for (layer, _) in &mut drawing.entities {
        *layer = new_index[usize::from(*layer)];
    }
}

/// A plane affine map: `x' = m[0][0] x + m[0][1] y + m[0][2]`, and so on.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Affine {
    m: [[f64; 3]; 2],
}

impl Affine {
    fn scale(x: f64, y: f64) -> Self {
        Self {
            m: [[x, 0.0, 0.0], [0.0, y, 0.0]],
        }
    }

    fn translate(x: f64, y: f64) -> Self {
        Self {
            m: [[1.0, 0.0, x], [0.0, 1.0, y]],
        }
    }

    fn rotate(angle: f64) -> Self {
        let (sin, cos) = angle.sin_cos();
        Self {
            m: [[cos, -sin, 0.0], [sin, cos, 0.0]],
        }
    }

    /// First `other`, then `self`.
    fn then_after(&self, other: &Self) -> Self {
        let a = &self.m;
        let b = &other.m;
        Self {
            m: std::array::from_fn(|row| {
                [
                    a[row][0] * b[0][0] + a[row][1] * b[1][0],
                    a[row][0] * b[0][1] + a[row][1] * b[1][1],
                    a[row][0] * b[0][2] + a[row][1] * b[1][2] + a[row][2],
                ]
            }),
        }
    }

    fn apply(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        let m = &self.m;
        [
            m[0][0] * x + m[0][1] * y + m[0][2],
            m[1][0] * x + m[1][1] * y + m[1][2],
        ]
    }

    /// How much a length grows, the mean of the two axes.
    fn length_scale(&self) -> f64 {
        let m = &self.m;
        (m[0][0] * m[1][1] - m[0][1] * m[1][0]).abs().sqrt()
    }
}

/// The object coordinate system of an entity with an extrusion direction:
/// its points lie in the plane of `normal`, by the arbitrary axis rule.
#[derive(Debug, Clone, Copy)]
struct Ocs {
    x: [f64; 3],
    y: [f64; 3],
    z: [f64; 3],
}

impl Ocs {
    fn of(normal: &Vector3) -> Self {
        let length = (normal.x * normal.x + normal.y * normal.y + normal.z * normal.z).sqrt();
        // No usable direction, or straight up: the plan itself.
        let unusable = length.is_nan() || length <= 1e-12;
        let up = normal.x.abs() < 1e-12 && normal.y.abs() < 1e-12 && normal.z > 0.0;
        if unusable || up {
            return Self {
                x: [1.0, 0.0, 0.0],
                y: [0.0, 1.0, 0.0],
                z: [0.0, 0.0, 1.0],
            };
        }
        let n = [normal.x / length, normal.y / length, normal.z / length];
        let limit = 1.0 / 64.0;
        let world = if n[0].abs() < limit && n[1].abs() < limit {
            [0.0, 1.0, 0.0]
        } else {
            [0.0, 0.0, 1.0]
        };
        let x = unit(cross(world, n));
        let y = unit(cross(n, x));
        Self { x, y, z: n }
    }

    /// The plan position of a point given in the coordinates of the entity.
    fn plan(&self, [x, y]: [f64; 2], elevation: f64) -> [f64; 2] {
        [
            x * self.x[0] + y * self.y[0] + elevation * self.z[0],
            x * self.x[1] + y * self.y[1] + elevation * self.z[1],
        ]
    }

    /// The turn in plan of a direction at `angle` in the entity.
    fn plan_angle(&self, angle: f64) -> f64 {
        let [x, y] = self.plan([angle.cos(), angle.sin()], 0.0);
        y.atan2(x)
    }
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length > 0.0 {
        [v[0] / length, v[1] / length, v[2] / length]
    } else {
        v
    }
}

fn xy(v: &Vector3) -> [f64; 2] {
    [v.x, v.y]
}

/// Segments for a sweep of `angle` radians.
fn segments(angle: f64) -> usize {
    ((angle.abs() / TAU * CIRCLE_SEGMENTS).ceil() as usize).clamp(1, 720)
}

/// The points of an arc from `start` over `sweep` radians, both ends
/// included; a negative sweep runs clockwise.
fn arc_points(center: [f64; 2], radius: f64, start: f64, sweep: f64) -> Vec<[f64; 2]> {
    let count = segments(sweep);
    (0..=count)
        .map(|step| {
            let angle = start + sweep * step as f64 / count as f64;
            [
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            ]
        })
        .collect()
}

/// The counter-clockwise sweep from one angle to another, in (0, 2π].
fn ccw_sweep(start: f64, end: f64) -> f64 {
    let sweep = (end - start).rem_euclid(TAU);
    if sweep <= 1e-12 {
        TAU
    } else {
        sweep
    }
}

/// The points of a segment from `from` to `to` that bends by `bulge`, the
/// tangent of a quarter of its angle; `to` itself is left out.
fn bulge_points(from: [f64; 2], to: [f64; 2], bulge: f64, out: &mut Vec<[f64; 2]>) {
    out.push(from);
    if bulge.abs() < 1e-9 || !bulge.is_finite() {
        return;
    }
    let chord = [to[0] - from[0], to[1] - from[1]];
    let length = (chord[0] * chord[0] + chord[1] * chord[1]).sqrt();
    if length < 1e-12 {
        return;
    }
    let sweep = 4.0 * bulge.atan();
    let radius = length / (2.0 * (sweep / 2.0).sin());
    // The centre lies on the perpendicular of the chord at its middle.
    let middle = [(from[0] + to[0]) / 2.0, (from[1] + to[1]) / 2.0];
    let offset = radius * (sweep / 2.0).cos();
    let normal = [-chord[1] / length, chord[0] / length];
    let center = [
        middle[0] + normal[0] * offset,
        middle[1] + normal[1] * offset,
    ];
    let start = (from[1] - center[1]).atan2(from[0] - center[0]);
    let points = arc_points(center, radius.abs(), start, sweep);
    out.extend(&points[1..points.len() - 1]);
}

/// The rings of a polyline with bulges: every segment from a vertex to the
/// next, and back to the first when it is closed.
fn bulged(vertices: &[([f64; 2], f64)], closed: bool) -> Vec<[f64; 2]> {
    let mut points = Vec::with_capacity(vertices.len());
    for (index, (at, bulge)) in vertices.iter().enumerate() {
        match vertices.get(index + 1) {
            Some((next, _)) => bulge_points(*at, *next, *bulge, &mut points),
            None if closed && vertices.len() > 1 => {
                bulge_points(*at, vertices[0].0, *bulge, &mut points)
            }
            None => points.push(*at),
        }
    }
    points
}

fn plain(value: &str) -> String {
    parse_plain_text(value).to_plain_text()
}

/// The colour of an entity that has one of its own.
fn own_color(color: &Color) -> Option<[u8; 3]> {
    match color {
        Color::ByLayer | Color::ByBlock => None,
        other => other.rgb().map(|(r, g, b)| [r, g, b]),
    }
}

struct Reader<'a> {
    document: &'a CadDocument,
    drawing: Drawing2d,
    /// Drawing layer per layer name of the file, in capitals.
    layers: HashMap<String, u16>,
    skipped: BTreeMap<String, usize>,
    skipped_3d: usize,
    inserts: usize,
    /// The blocks being inserted, to stop a block that inserts itself.
    blocks: Vec<String>,
}

impl Reader<'_> {
    fn skip(&mut self, kind: &str) {
        *self.skipped.entry(kind.to_owned()).or_default() += 1;
    }

    /// The drawing layer of an entity. Inside a block an entity on layer 0
    /// takes the layer of the reference.
    fn layer(&mut self, name: &str, inherited: Option<&str>) -> u16 {
        let name = match inherited {
            Some(outer) if name == "0" || name.is_empty() => outer,
            _ if name.is_empty() => "0",
            _ => name,
        };
        let folded = name.to_uppercase();
        if let Some(index) = self.layers.get(&folded) {
            return *index;
        }
        let rgb = self
            .document
            .layers
            .get(name)
            .and_then(|layer| layer.color.rgb())
            .map_or(LAYER_RGB_CONTRAST, |(r, g, b)| [r, g, b]);
        // A file holds fewer layers than a drawing can; a drawing that is
        // full puts the rest on its first layer.
        let index = self.drawing.layer(name, rgb).unwrap_or(0);
        self.layers.insert(folded, index);
        index
    }

    fn full(&self) -> bool {
        self.drawing.entities.len() >= MAX_ENTITIES
    }

    fn polyline(&mut self, layer: u16, at: &Affine, points: &[[f64; 2]], closed: bool) {
        let points: Vec<[f64; 2]> = points.iter().map(|point| at.apply(*point)).collect();
        if points.len() < 2 {
            return;
        }
        self.drawing.add_polyline(layer, points, closed);
    }

    fn text(
        &mut self,
        layer: u16,
        at: &Affine,
        position: [f64; 2],
        height: f64,
        rotation: f64,
        value: &str,
    ) {
        let value = value.trim();
        if value.is_empty() || !(height.is_finite() && height > 0.0) {
            return;
        }
        let direction = at.apply([rotation.cos(), rotation.sin()]);
        let origin = at.apply([0.0, 0.0]);
        let turned = (direction[1] - origin[1]).atan2(direction[0] - origin[0]);
        self.drawing.entities.push((
            layer,
            DrawingEntity::Text {
                at: at.apply(position),
                height: height * at.length_scale(),
                rotation: if turned.abs() < 1e-12 { 0.0 } else { turned },
                value: super::single_line(value),
            },
        ));
    }

    fn entity(&mut self, entity: &EntityType, at: &Affine, inherited: Option<&str>) {
        if self.full() {
            self.skip("over the entity limit");
            return;
        }
        let common = entity.common();
        if common.invisible {
            return;
        }
        let layer_name = common.layer.as_str();
        match entity {
            EntityType::Point(point) => {
                let layer = self.layer(layer_name, inherited);
                self.drawing.add_point(
                    layer,
                    at.apply(xy(&point.location)),
                    own_color(&point.common.color),
                );
            }
            EntityType::Line(line) => {
                let layer = self.layer(layer_name, inherited);
                self.polyline(layer, at, &[xy(&line.start), xy(&line.end)], false);
            }
            EntityType::Circle(circle) => {
                let ocs = Ocs::of(&circle.normal);
                let layer = self.layer(layer_name, inherited);
                let mut ring = arc_points(xy(&circle.center), circle.radius, 0.0, TAU);
                ring.pop();
                let ring: Vec<_> = ring
                    .into_iter()
                    .map(|point| ocs.plan(point, circle.center.z))
                    .collect();
                self.polyline(layer, at, &ring, true);
            }
            EntityType::Arc(arc) => {
                let ocs = Ocs::of(&arc.normal);
                let layer = self.layer(layer_name, inherited);
                let points: Vec<_> = arc_points(
                    xy(&arc.center),
                    arc.radius,
                    arc.start_angle,
                    ccw_sweep(arc.start_angle, arc.end_angle),
                )
                .into_iter()
                .map(|point| ocs.plan(point, arc.center.z))
                .collect();
                self.polyline(layer, at, &points, false);
            }
            EntityType::Ellipse(ellipse) => {
                let layer = self.layer(layer_name, inherited);
                let major = [
                    ellipse.major_axis.x,
                    ellipse.major_axis.y,
                    ellipse.major_axis.z,
                ];
                let normal = [ellipse.normal.x, ellipse.normal.y, ellipse.normal.z];
                let minor = cross(normal, major).map(|value| value * ellipse.minor_axis_ratio);
                let start = ellipse.start_parameter;
                let full = ellipse.is_full();
                let sweep = if full {
                    TAU
                } else {
                    ccw_sweep(start, ellipse.end_parameter)
                };
                let count = segments(sweep);
                let mut points: Vec<[f64; 2]> = (0..=count)
                    .map(|step| {
                        let t = start + sweep * step as f64 / count as f64;
                        [
                            ellipse.center.x + t.cos() * major[0] + t.sin() * minor[0],
                            ellipse.center.y + t.cos() * major[1] + t.sin() * minor[1],
                        ]
                    })
                    .collect();
                if full {
                    points.pop();
                }
                self.polyline(layer, at, &points, full);
            }
            EntityType::LwPolyline(polyline) => {
                let ocs = Ocs::of(&polyline.normal);
                let layer = self.layer(layer_name, inherited);
                let vertices: Vec<_> = polyline
                    .vertices
                    .iter()
                    .map(|vertex| ([vertex.location.x, vertex.location.y], vertex.bulge))
                    .collect();
                let points: Vec<_> = bulged(&vertices, polyline.is_closed)
                    .into_iter()
                    .map(|point| ocs.plan(point, polyline.elevation))
                    .collect();
                self.polyline(layer, at, &points, polyline.is_closed);
            }
            EntityType::Polyline2D(polyline) => {
                let ocs = Ocs::of(&polyline.normal);
                let layer = self.layer(layer_name, inherited);
                let vertices: Vec<_> = polyline
                    .vertices
                    .iter()
                    .map(|vertex| (xy(&vertex.location), vertex.bulge))
                    .collect();
                let closed = polyline.is_closed();
                let points: Vec<_> = bulged(&vertices, closed)
                    .into_iter()
                    .map(|point| ocs.plan(point, polyline.elevation))
                    .collect();
                self.polyline(layer, at, &points, closed);
            }
            EntityType::Polyline(polyline) => {
                // Bit 16 is a polygon mesh, bit 64 a polyface mesh.
                if polyline.flags.bits() & (16 | 64) != 0 {
                    self.skipped_3d += 1;
                    return;
                }
                let layer = self.layer(layer_name, inherited);
                let points: Vec<_> = polyline
                    .vertices
                    .iter()
                    .map(|vertex| xy(&vertex.location))
                    .collect();
                self.polyline(layer, at, &points, polyline.is_closed());
            }
            EntityType::Polyline3D(polyline) => {
                if polyline.flags.is_3d_mesh || polyline.flags.is_polyface_mesh {
                    self.skipped_3d += 1;
                    return;
                }
                let layer = self.layer(layer_name, inherited);
                let points: Vec<_> = polyline
                    .vertices
                    .iter()
                    .map(|vertex| xy(&vertex.position))
                    .collect();
                self.polyline(layer, at, &points, polyline.flags.closed);
            }
            EntityType::Text(text) => {
                let layer = self.layer(layer_name, inherited);
                self.single_text(layer, at, text);
            }
            EntityType::MText(text) => {
                let layer = self.layer(layer_name, inherited);
                self.multi_text(layer, at, text);
            }
            EntityType::AttributeEntity(attribute) => {
                // Bit 1 hides an attribute.
                if !attribute.flags.invisible {
                    let layer = self.layer(layer_name, inherited);
                    let ocs = Ocs::of(&attribute.normal);
                    let position =
                        ocs.plan(xy(&attribute.insertion_point), attribute.insertion_point.z);
                    self.text(
                        layer,
                        at,
                        position,
                        attribute.height,
                        ocs.plan_angle(attribute.rotation),
                        &plain(&attribute.value),
                    );
                }
            }
            EntityType::Hatch(hatch) => {
                let layer = self.layer(layer_name, inherited);
                self.hatch(layer, at, hatch);
            }
            EntityType::Solid(solid) => {
                let ocs = Ocs::of(&solid.normal);
                let layer = self.layer(layer_name, inherited);
                // The corners of a solid run 1, 2, 4, 3 around it; a
                // triangle repeats its third corner.
                let mut ring: Vec<[f64; 2]> = [
                    &solid.first_corner,
                    &solid.second_corner,
                    &solid.fourth_corner,
                    &solid.third_corner,
                ]
                .into_iter()
                .map(|corner| at.apply(ocs.plan(xy(corner), corner.z)))
                .collect();
                ring.dedup();
                if ring.len() >= 3 {
                    self.drawing.add_fill(layer, ring, Vec::new());
                }
            }
            EntityType::Insert(insert) => self.insert(insert, at, inherited),
            EntityType::Dimension(dimension) => {
                let block = dimension.base().block_name.clone();
                if !block.is_empty() && self.document.block_records.get(&block).is_some() {
                    let layer = super::layer_name(layer_name);
                    self.block(&block, at, Some(&layer));
                } else if !self.aligned_dimension(dimension, at, layer_name, inherited) {
                    self.exploded(entity, at, inherited);
                }
            }
            EntityType::Mesh(_)
            | EntityType::PolyfaceMesh(_)
            | EntityType::PolygonMesh(_)
            | EntityType::Face3D(_)
            | EntityType::Solid3D(_)
            | EntityType::Region(_)
            | EntityType::Body(_)
            | EntityType::Surface(_) => self.skipped_3d += 1,
            // Markers and definitions that are not drawn themselves.
            EntityType::Block(_)
            | EntityType::BlockEnd(_)
            | EntityType::Seqend(_)
            | EntityType::AttributeDefinition(_)
            | EntityType::Viewport(_) => {}
            EntityType::Spline(_)
            | EntityType::Helix(_)
            | EntityType::Leader(_)
            | EntityType::MultiLeader(_)
            | EntityType::MLine(_) => self.exploded(entity, at, inherited),
            other => self.skip(other.as_entity().entity_type()),
        }
    }

    /// An aligned dimension without the block of its geometry, as a drawing
    /// program draws it from its points and its style: its extension lines,
    /// its line with ticks and its value. False for another kind.
    fn aligned_dimension(
        &mut self,
        dimension: &Dimension,
        at: &Affine,
        layer_name: &str,
        inherited: Option<&str>,
    ) -> bool {
        let Dimension::Aligned(aligned) = dimension else {
            return false;
        };
        let base = dimension.base();
        let [first, second, line] = [
            &aligned.first_point,
            &aligned.second_point,
            &aligned.definition_point,
        ]
        .map(xy);
        let Some((normal, _)) = dimension_axes(first, second) else {
            return false;
        };
        let style = self.document.dim_styles.get(&base.style_name);
        let scale = style
            .map(|style| style.dimscale)
            .filter(|scale| scale.is_finite() && *scale > 0.0)
            .unwrap_or(1.0);
        let height = style.map_or(0.18, |style| style.dimtxt) * scale;
        let offset = (line[0] - second[0]) * normal[0] + (line[1] - second[1]) * normal[1];
        let Some(shape) = dimension_shape(first, second, offset, height) else {
            return false;
        };
        let value = match base.text_override().filter(|text| *text != "<>") {
            Some(text) => plain(text),
            None => {
                let length = (second[0] - first[0]).hypot(second[1] - first[1])
                    * style.map_or(1.0, |style| style.dimlfac);
                let step = style.map_or(0.0, |style| style.dimrnd);
                let rounded = if step > 0.0 {
                    (length / step).round() * step
                } else {
                    length
                };
                let decimals = style.map_or(2, |style| style.dimdec.clamp(0, 8)) as usize;
                format!("{rounded:.decimals$}")
            }
        };
        let layer = self.layer(layer_name, inherited);
        for [from, to] in &shape.lines {
            self.polyline(layer, at, &[*from, *to], false);
        }
        let width = estimated_width(&value, height);
        let (sin, cos) = shape.text_rotation.sin_cos();
        let start = [
            shape.text_at[0] - cos * width / 2.0,
            shape.text_at[1] - sin * width / 2.0,
        ];
        self.text(layer, at, start, height, shape.text_rotation, &value);
        true
    }

    /// An entity drawn as the simpler entities the codec breaks it into.
    fn exploded(&mut self, entity: &EntityType, at: &Affine, inherited: Option<&str>) {
        let parts = entity.explode();
        if parts.is_empty() {
            self.skip(entity.as_entity().entity_type());
            return;
        }
        for part in &parts {
            self.entity(part, at, inherited);
        }
    }

    fn single_text(&mut self, layer: u16, at: &Affine, text: &Text) {
        let ocs = Ocs::of(&text.normal);
        let value = plain(&text.value);
        let height = text.height;
        let width = estimated_width(&value, height) * text.width_factor.clamp(0.1, 10.0);
        let aligned = !matches!(text.horizontal_alignment, TextHorizontalAlignment::Left)
            || !matches!(text.vertical_alignment, TextVerticalAlignment::Baseline);
        let anchor = match (aligned, &text.alignment_point) {
            (true, Some(point)) => xy(point),
            _ => xy(&text.insertion_point),
        };
        let shift_x = match text.horizontal_alignment {
            TextHorizontalAlignment::Center | TextHorizontalAlignment::Middle => -width / 2.0,
            TextHorizontalAlignment::Right => -width,
            _ => 0.0,
        };
        let shift_y = match (&text.horizontal_alignment, &text.vertical_alignment) {
            (TextHorizontalAlignment::Middle, _) | (_, TextVerticalAlignment::Middle) => {
                -height / 2.0
            }
            (_, TextVerticalAlignment::Top) => -height,
            _ => 0.0,
        };
        let (sin, cos) = text.rotation.sin_cos();
        let local = [
            anchor[0] + shift_x * cos - shift_y * sin,
            anchor[1] + shift_x * sin + shift_y * cos,
        ];
        let position = ocs.plan(local, text.insertion_point.z);
        self.text(
            layer,
            at,
            position,
            height,
            ocs.plan_angle(text.rotation),
            &value,
        );
    }

    fn multi_text(&mut self, layer: u16, at: &Affine, text: &MText) {
        let value = parse_mtext(&text.value, true).to_plain_text();
        let lines: Vec<&str> = value.lines().collect();
        if lines.is_empty() {
            return;
        }
        let height = text.height;
        let pitch = height * 5.0 / 3.0 * text.line_spacing_factor.clamp(0.25, 4.0);
        let rotation = match &text.dwg_x_direction {
            Some(direction) if direction.x != 0.0 || direction.y != 0.0 => {
                direction.y.atan2(direction.x)
            }
            _ => text.rotation,
        };
        use AttachmentPoint as A;
        let block = height + pitch * (lines.len() - 1) as f64;
        // The baseline of the first line below the attachment point.
        let first = match text.attachment_point {
            A::TopLeft | A::TopCenter | A::TopRight => -height,
            A::MiddleLeft | A::MiddleCenter | A::MiddleRight => block / 2.0 - height,
            A::BottomLeft | A::BottomCenter | A::BottomRight => block - height,
        };
        let (sin, cos) = rotation.sin_cos();
        let origin = xy(&text.insertion_point);
        for (index, line) in lines.iter().enumerate() {
            let width = estimated_width(line, height);
            let shift_x = match text.attachment_point {
                A::TopCenter | A::MiddleCenter | A::BottomCenter => -width / 2.0,
                A::TopRight | A::MiddleRight | A::BottomRight => -width,
                _ => 0.0,
            };
            let shift_y = first - pitch * index as f64;
            let position = [
                origin[0] + shift_x * cos - shift_y * sin,
                origin[1] + shift_x * sin + shift_y * cos,
            ];
            self.text(layer, at, position, height, rotation, line);
        }
    }

    fn hatch(&mut self, layer: u16, at: &Affine, hatch: &Hatch) {
        let ocs = Ocs::of(&hatch.normal);
        let rings: Vec<Vec<[f64; 2]>> = hatch
            .paths
            .iter()
            .map(path_ring)
            .map(|ring| {
                ring.into_iter()
                    .map(|point| at.apply(ocs.plan(point, hatch.elevation)))
                    .collect::<Vec<_>>()
            })
            .filter(|ring| ring.len() >= 3)
            .collect();
        if rings.is_empty() {
            return;
        }
        if hatch.is_solid || hatch.gradient_color.enabled {
            let mut rings = rings.into_iter();
            if let Some(outer) = rings.next() {
                self.drawing.add_fill(layer, outer, rings.collect());
            }
        } else {
            // A pattern is not drawn; its boundary is.
            for ring in rings {
                self.drawing.add_polyline(layer, ring, true);
            }
        }
    }

    fn insert(&mut self, insert: &Insert, at: &Affine, inherited: Option<&str>) {
        let layer = match inherited {
            Some(outer) if insert.common.layer == "0" => outer.to_owned(),
            _ => super::layer_name(&insert.common.layer),
        };
        let ocs = Ocs::of(&insert.normal);
        let rows = insert.row_count.max(1);
        let columns = insert.column_count.max(1);
        let base = self.base_point(&insert.block_name);
        let placed = ocs.plan(xy(&insert.insert_point), insert.insert_point.z);
        let rotation = ocs.plan_angle(insert.rotation);
        // A mirrored object coordinate system mirrors the block as well.
        let mirror = if ocs.x[0] * ocs.y[1] - ocs.x[1] * ocs.y[0] < 0.0 {
            -1.0
        } else {
            1.0
        };
        for row in 0..rows {
            for column in 0..columns {
                let offset = [
                    f64::from(column) * insert.column_spacing,
                    f64::from(row) * insert.row_spacing,
                ];
                let local = Affine::translate(placed[0], placed[1])
                    .then_after(&Affine::rotate(rotation))
                    .then_after(&Affine::translate(offset[0], offset[1]))
                    .then_after(&Affine::scale(insert.x_scale() * mirror, insert.y_scale()))
                    .then_after(&Affine::translate(-base[0], -base[1]));
                let transform = at.then_after(&local);
                self.block(&insert.block_name, &transform, Some(&layer));
            }
        }
        for attribute in &insert.attributes {
            self.entity(
                &EntityType::AttributeEntity(attribute.clone()),
                at,
                Some(&layer),
            );
        }
    }

    fn base_point(&self, block: &str) -> [f64; 2] {
        let Some(record) = self.document.block_records.get(block) else {
            return [0.0, 0.0];
        };
        match self.document.get_entity(record.block_entity_handle) {
            Some(EntityType::Block(start)) => xy(&start.base_point),
            _ => xy(&record.base_point),
        }
    }

    /// The entities of a block, placed by `at`.
    fn block(&mut self, name: &str, at: &Affine, layer: Option<&str>) {
        let Some(record) = self.document.block_records.get(name) else {
            self.skip("INSERT of a missing block");
            return;
        };
        if !record.xref_path.is_empty() {
            self.skip("external reference");
            return;
        }
        let folded = name.to_uppercase();
        if self.blocks.len() >= MAX_BLOCK_DEPTH || self.blocks.contains(&folded) {
            self.skip("INSERT nested too deep");
            return;
        }
        self.inserts += 1;
        self.blocks.push(folded);
        let entities: Vec<&EntityType> = self.document.entities_in_block(name).collect();
        for entity in entities {
            self.entity(entity, at, layer);
        }
        self.blocks.pop();
    }
}

/// About how wide a line of text is.
pub(crate) fn estimated_width(value: &str, height: f64) -> f64 {
    value.chars().count() as f64 * height * CHARACTER_WIDTH
}

/// The ring of a boundary path of a fill, in the coordinates of the fill.
fn path_ring(path: &BoundaryPath) -> Vec<[f64; 2]> {
    let mut ring: Vec<[f64; 2]> = Vec::new();
    for edge in &path.edges {
        match edge {
            BoundaryEdge::Polyline(polyline) => {
                let vertices: Vec<_> = polyline
                    .vertices
                    .iter()
                    .map(|vertex| ([vertex.x, vertex.y], vertex.z))
                    .collect();
                ring.extend(bulged(&vertices, polyline.is_closed));
            }
            BoundaryEdge::Line(line) => {
                ring.push([line.start.x, line.start.y]);
                ring.push([line.end.x, line.end.y]);
            }
            BoundaryEdge::CircularArc(arc) => {
                let (start, sweep) =
                    edge_sweep(arc.start_angle, arc.end_angle, arc.counter_clockwise);
                ring.extend(arc_points(
                    [arc.center.x, arc.center.y],
                    arc.radius,
                    start,
                    sweep,
                ));
            }
            BoundaryEdge::EllipticArc(arc) => {
                let major = [arc.major_axis_endpoint.x, arc.major_axis_endpoint.y];
                let minor = [
                    -major[1] * arc.minor_axis_ratio,
                    major[0] * arc.minor_axis_ratio,
                ];
                let (start, sweep) =
                    edge_sweep(arc.start_angle, arc.end_angle, arc.counter_clockwise);
                let count = segments(sweep);
                ring.extend((0..=count).map(|step| {
                    let t = start + sweep * step as f64 / count as f64;
                    [
                        arc.center.x + t.cos() * major[0] + t.sin() * minor[0],
                        arc.center.y + t.cos() * major[1] + t.sin() * minor[1],
                    ]
                }));
            }
            BoundaryEdge::Spline(spline) => {
                if spline.fit_points.is_empty() {
                    ring.extend(spline.control_points.iter().map(xy));
                } else {
                    ring.extend(spline.fit_points.iter().map(|point| [point.x, point.y]));
                }
            }
        }
    }
    ring.dedup();
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }
    ring
}

/// Start and sweep of an arc edge of a fill. A clockwise edge keeps its
/// angles as they would be counter-clockwise, mirrored.
fn edge_sweep(start: f64, end: f64, counter_clockwise: bool) -> (f64, f64) {
    if counter_clockwise {
        (start, ccw_sweep(start, end))
    } else {
        (-start, -ccw_sweep(start, end))
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use super::*;
    use crate::drawing::{
        write_drawing, DrawingVersion, LAYER_CUT_FILL, LAYER_CUT_OUTLINE, LAYER_FRAME, LAYER_INFO,
        LAYER_POINTS,
    };

    fn sample() -> Drawing2d {
        let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
        let points = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
        drawing.add_point(points, [0.5, 0.25], None);
        drawing.add_point(points, [3.25, 0.25], Some([200, 30, 40]));
        drawing.add_point(points, [-0.125, 1.5], None);
        drawing
            .add_cut_region(
                vec![[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]],
                vec![vec![[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]]],
            )
            .unwrap();
        drawing.add_frame([-0.5, -1.5], [4.5, 3.5]).unwrap();
        drawing
            .add_info([-0.5, -1.75], 0.1, "Plan; cut plane Z")
            .unwrap();
        drawing
    }

    fn counts(drawing: &Drawing2d) -> BTreeMap<(String, &'static str), usize> {
        let mut counts = BTreeMap::new();
        for (layer, entity) in &drawing.entities {
            let kind = match entity {
                DrawingEntity::Point { .. } => "point",
                DrawingEntity::Polyline { .. } => "polyline",
                DrawingEntity::Fill { .. } => "fill",
                DrawingEntity::Text { .. } => "text",
                DrawingEntity::Dimension { .. } => "dimension",
                DrawingEntity::Leader { .. } => "leader",
            };
            *counts
                .entry((drawing.layers[usize::from(*layer)].name.clone(), kind))
                .or_default() += 1;
        }
        counts
    }

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn a_drawing_written_as_dxf_or_dwg_reads_back_with_the_same_entities() {
        let dir = tempfile::tempdir().unwrap();
        for units in DrawingUnits::ALL {
            let mut written = sample();
            written.units = units;
            for format in DrawingFormat::ALL {
                let path = dir
                    .path()
                    .join(format!("plan-{}.{}", units.key(), format.extension()));
                write_drawing(&written, &path, format, DrawingVersion::default()).unwrap();
                let read = read_drawing(&path).unwrap();
                assert!(read.units_named);
                assert_eq!(read.drawing.units, units, "{format} in {}", units.key());
                assert!(read.skipped.is_empty(), "{:?}", read.skipped);
                assert_eq!(read.skipped_3d, 0);
                assert_eq!(counts(&read.drawing), counts(&written), "{format}");
                let [min, max] = read.drawing.extents().unwrap();
                let [wmin, wmax] = written.extents().unwrap();
                assert!(near(min, wmin) && near(max, wmax), "{format}");
                // The colour of a point and the layer colours come back.
                assert!(read.drawing.entities.iter().any(|(_, entity)| matches!(
                    entity,
                    DrawingEntity::Point {
                        rgb: Some([200, 30, 40]),
                        ..
                    }
                )));
                let fill = read
                    .drawing
                    .layers
                    .iter()
                    .find(|layer| layer.name == LAYER_CUT_FILL)
                    .unwrap();
                assert_eq!(fill.rgb, [128, 128, 128]);
                let text = read
                    .drawing
                    .entities
                    .iter()
                    .find_map(|(_, entity)| match entity {
                        DrawingEntity::Text { value, height, .. } => Some((value.clone(), *height)),
                        _ => None,
                    });
                let (value, height) = text.unwrap();
                assert_eq!(value, "Plan; cut plane Z");
                assert!((height - 0.1).abs() < 1e-9);
                for name in [LAYER_CUT_OUTLINE, LAYER_FRAME, LAYER_INFO] {
                    assert!(read.drawing.layers.iter().any(|layer| layer.name == name));
                }
            }
        }
    }

    #[test]
    fn unknown_extension_is_refused() {
        assert!(matches!(
            read_drawing(Path::new("plan.pdf")),
            Err(LoadError::UnsupportedFormat(extension)) if extension == "pdf"
        ));
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken.dxf");
        std::fs::write(&broken, b"not a drawing").unwrap();
        assert!(read_drawing(&broken).is_err());
    }

    #[test]
    fn bulges_arcs_and_affine_maps_land_where_they_should() {
        // A half circle of radius 1 from (1, 0) to (-1, 0) counter-clockwise.
        let ring = bulged(&[([1.0, 0.0], 1.0), ([-1.0, 0.0], 0.0)], false);
        assert!(near(ring[0], [1.0, 0.0]));
        assert!(near(*ring.last().unwrap(), [-1.0, 0.0]));
        let top = ring.iter().copied().fold([0.0, f64::MIN], |best, point| {
            if point[1] > best[1] {
                point
            } else {
                best
            }
        });
        assert!(near(top, [0.0, 1.0]), "{top:?}");
        assert!(ring
            .iter()
            .all(|point| (point[0].hypot(point[1]) - 1.0).abs() < 1e-9));

        assert!((ccw_sweep(0.0, PI / 2.0) - PI / 2.0).abs() < 1e-12);
        assert!((ccw_sweep(PI / 2.0, 0.0) - 1.5 * PI).abs() < 1e-12);
        assert!((ccw_sweep(1.0, 1.0) - TAU).abs() < 1e-12);

        let map = Affine::translate(10.0, 0.0)
            .then_after(&Affine::rotate(PI / 2.0))
            .then_after(&Affine::scale(2.0, 2.0));
        assert!(near(map.apply([1.0, 0.0]), [10.0, 2.0]));
        assert!((map.length_scale() - 2.0).abs() < 1e-12);

        // An extrusion along -Z mirrors X.
        let mirrored = Ocs::of(&Vector3::new(0.0, 0.0, -1.0));
        assert!(near(mirrored.plan([2.0, 3.0], 0.0), [-2.0, 3.0]));
        assert!(near(
            Ocs::of(&Vector3::new(0.0, 0.0, 1.0)).plan([2.0, 3.0], 5.0),
            [2.0, 3.0]
        ));
    }

    #[test]
    fn blocks_lines_circles_and_unknown_entities_are_read() {
        use cadcodec::entities::{Circle, Line, Mesh};
        use cadcodec::tables::BlockRecord;
        let mut document = CadDocument::new();
        document.header.insertion_units = 6;
        let mut line = Line::from_points(Vector3::new(0.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
        line.common.layer = "WALLS".into();
        document.add_entity(EntityType::Line(line)).unwrap();
        let circle = Circle::from_center_radius(Vector3::new(5.0, 5.0, 0.0), 1.0);
        document.add_entity(EntityType::Circle(circle)).unwrap();
        document.add_entity(EntityType::Mesh(Mesh::new())).unwrap();
        // A block of one line from (0, 0) to (1, 0), inserted turned a
        // quarter at (10, 10) and twice as large.
        let mut record = BlockRecord::new("DOOR");
        record.handle = document.allocate_handle();
        let owner = record.handle;
        document.block_records.add(record).unwrap();
        let mut block_line =
            Line::from_points(Vector3::new(0.0, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0));
        block_line.common.owner_handle = owner;
        document.add_entity(EntityType::Line(block_line)).unwrap();
        let mut insert = Insert::new("DOOR", Vector3::new(10.0, 10.0, 0.0));
        insert.rotation = PI / 2.0;
        insert.set_x_scale(2.0);
        insert.set_y_scale(2.0);
        insert.common.layer = "DOORS".into();
        document.add_entity(EntityType::Insert(insert)).unwrap();

        let read = read_document(&document);
        assert_eq!(read.drawing.units, DrawingUnits::Metres);
        assert_eq!(read.skipped_3d, 1);
        assert_eq!(read.inserts, 1);
        let lines: Vec<(String, Vec<[f64; 2]>)> = read
            .drawing
            .entities
            .iter()
            .filter_map(|(layer, entity)| match entity {
                DrawingEntity::Polyline { points, .. } => Some((
                    read.drawing.layers[usize::from(*layer)].name.clone(),
                    points.clone(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0].0, "WALLS");
        // The circle is closed and has its radius.
        assert!(lines[1].1.len() >= 36);
        assert!(lines[1]
            .1
            .iter()
            .all(|point| ((point[0] - 5.0).hypot(point[1] - 5.0) - 1.0).abs() < 1e-9));
        // The line of the block lies on the layer of the reference, turned
        // and scaled.
        assert_eq!(lines[2].0, "DOORS");
        assert!(near(lines[2].1[0], [10.0, 10.0]));
        assert!(near(lines[2].1[1], [10.0, 12.0]), "{:?}", lines[2].1);
    }
}
