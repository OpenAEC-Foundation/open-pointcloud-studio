//! Geometry from the scan as editable 3D objects in CAD and BIM files.
//!
//! The detected faces and a mesh are first turned into a [`Model3d`]: a
//! list of objects, each a set of positions with its triangles, the
//! outline rings of a flat face and the axis and radius of a cylinder. Two
//! writers take it from there:
//!
//! - [`cad`]: DXF and DWG through the codec of the drawing export. A flat
//!   face is a polyface mesh whose inner triangle edges are hidden, so that
//!   it shows as its outline with its openings; a cylinder is a polyface
//!   mesh of its scanned part with its axis as a line; a mesh is one or more
//!   `MESH` entities. No ACIS solids: a recipient can turn a mesh into a
//!   solid or a surface itself where its program offers that.
//! - [`ifc`]: IFC4 as a STEP physical file. Flat faces are polygonal face
//!   sets with their openings, a cylinder seen from outside is an extruded
//!   circle along its axis, a mesh is a triangulated face set.
//!
//! All positions are scene coordinates in metres.

pub(crate) mod cad;
pub(crate) mod ifc;
pub(crate) mod step;

pub use cad::plane_layer;
pub use ifc::LOCAL_LIMIT as IFC_LOCAL_LIMIT;

use std::collections::HashSet;

use crate::surfaces::FaceClass;
use crate::MeshGeometry;

/// Layer of the flat faces of one class: `OPS-PLANES-WALL` and so on.
pub const CAD_LAYER_PLANES: &str = "OPS-PLANES";
/// Layer of the scanned surface of the cylinders.
pub const CAD_LAYER_CYLINDERS: &str = "OPS-CYLINDERS";
/// Layer of the axes of the cylinders.
pub const CAD_LAYER_CYLINDER_AXES: &str = "OPS-CYLINDER-AXES";
/// Layer of a mesh.
pub const CAD_LAYER_MESH: &str = "OPS-MESH";

/// Colour of a mesh in a CAD file and an IFC file.
pub(crate) const MESH_RGB: [u8; 3] = [170, 170, 170];

/// What an object stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Plane(FaceClass),
    Cylinder,
    Mesh,
}

/// One ring of corners, as positions in the vertices of its object.
pub(crate) type Ring = Vec<u32>;

/// A connected part of a flat face: its outer ring and those of its
/// openings.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Polygon {
    pub outer: Ring,
    pub holes: Vec<Ring>,
}

/// The fitted shape of a cylinder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Cylinder {
    pub start: [f64; 3],
    pub end: [f64; 3],
    pub radius: f64,
    /// Unit direction square to the axis.
    pub across: [f64; 3],
    /// A round shaft rather than a column: there is no solid to fill.
    pub seen_from_inside: bool,
}

/// A value of an object for the property set of the IFC file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PropertyValue {
    Label(String),
    Length(f64),
    Area(f64),
    Real(f64),
    Count(u64),
    Bool(bool),
}

/// One object of the model.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Object3d {
    /// Number of the face, or 1 for a mesh.
    pub id: u32,
    pub name: String,
    pub kind: Kind,
    pub rgb: [u8; 3],
    pub vertices: Vec<[f64; 3]>,
    /// Counter-clockwise seen from the side the object faces.
    pub triangles: Vec<[u32; 3]>,
    /// The outline of a flat face; empty for anything else.
    pub polygons: Vec<Polygon>,
    pub cylinder: Option<Cylinder>,
    /// Triangle edges, lower vertex first, that a CAD program should not
    /// draw: those inside the outline of a flat face and the diagonals of
    /// the strips of a cylinder.
    pub hidden_edges: HashSet<(u32, u32)>,
    /// Whether the triangles close round a volume, where that is known.
    pub closed: Option<bool>,
    pub properties: Vec<(&'static str, PropertyValue)>,
}

impl Object3d {
    /// The type of the object as a short English text, for the IFC file.
    pub fn object_type(&self) -> String {
        match self.kind {
            Kind::Plane(class) => format!("Plane ({})", class.name()),
            Kind::Cylinder => "Cylinder".into(),
            Kind::Mesh => "Mesh".into(),
        }
    }

    /// Every position of the object, the ends of a cylinder's axis
    /// included.
    pub fn positions(&self) -> impl Iterator<Item = [f64; 3]> + '_ {
        self.vertices.iter().copied().chain(
            self.cylinder
                .iter()
                .flat_map(|shape| [shape.start, shape.end]),
        )
    }
}

/// Objects for a file, in the order they are written.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Model3d {
    /// The file name of the scan or the mesh, without its folder.
    pub source: String,
    /// Lines that describe where the model came from.
    pub notes: Vec<String>,
    pub objects: Vec<Object3d>,
}

impl Model3d {
    /// The smallest and the largest coordinate per axis, or nothing for a
    /// model without positions.
    pub fn extents(&self) -> Option<[[f64; 3]; 2]> {
        let mut positions = self.objects.iter().flat_map(Object3d::positions);
        let first = positions.next()?;
        Some(positions.fold([first, first], |[min, max], position| {
            [
                std::array::from_fn(|axis| min[axis].min(position[axis])),
                std::array::from_fn(|axis| max[axis].max(position[axis])),
            ]
        }))
    }
}

/// The triangle edges that lie on no ring: those a CAD program should not
/// draw for a flat face.
pub(crate) fn inner_edges(triangles: &[[u32; 3]], polygons: &[Polygon]) -> HashSet<(u32, u32)> {
    let mut rings = HashSet::new();
    for polygon in polygons {
        for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
            for (index, &corner) in ring.iter().enumerate() {
                rings.insert(edge(corner, ring[(index + 1) % ring.len()]));
            }
        }
    }
    triangles
        .iter()
        .flat_map(|&[a, b, c]| [edge(a, b), edge(b, c), edge(c, a)])
        .filter(|candidate| !rings.contains(candidate))
        .collect()
}

/// An edge with its lower vertex first.
pub(crate) fn edge(a: u32, b: u32) -> (u32, u32) {
    (a.min(b), a.max(b))
}

/// A mesh as a model of one object. Whether it is closed is measured: a
/// mesh is closed when every edge has exactly two triangles.
pub(crate) fn mesh_model(mesh: &MeshGeometry, source: &str, notes: &[&str]) -> Model3d {
    let topology = crate::mesh_topology(mesh);
    let closed = topology.open_edges == 0 && topology.non_manifold_edges == 0;
    Model3d {
        source: file_name(source),
        notes: notes.iter().map(|note| (*note).to_owned()).collect(),
        objects: vec![Object3d {
            id: 1,
            name: "Mesh".into(),
            kind: Kind::Mesh,
            rgb: MESH_RGB,
            vertices: mesh.vertices.clone(),
            triangles: mesh.triangles.clone(),
            polygons: Vec::new(),
            cylinder: None,
            hidden_edges: HashSet::new(),
            closed: Some(closed),
            properties: vec![
                ("Vertices", PropertyValue::Count(mesh.vertices.len() as u64)),
                (
                    "Triangles",
                    PropertyValue::Count(mesh.triangles.len() as u64),
                ),
                ("Closed", PropertyValue::Bool(closed)),
                (
                    "Components",
                    PropertyValue::Count(u64::from(topology.components)),
                ),
            ],
        }],
    }
}

/// The name of a file without its folder, for a model's source.
pub(crate) fn file_name(source: &str) -> String {
    std::path::Path::new(source)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) mod tests;
