//! A [`Model3d`] as DXF or DWG: polyface meshes, `MESH` entities and lines
//! on a layer per kind of object, in metres and scene coordinates. A
//! `MESH` entity has one colour, so a mesh with colours is written as an
//! entity per colour of its palette, each in that true colour.

use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::path::Path;

use cadcodec::entities::{EntityType, Line, Mesh, PolyfaceFace, PolyfaceMesh, PolyfaceVertex};
use cadcodec::tables::{Layer, TableEntry};
use cadcodec::{CadDocument, Color, DwgWriter, DxfWriter, Vector3};

use super::palette::face_colours;
use super::{
    edge, Kind, Model3d, Object3d, CAD_LAYER_CYLINDERS, CAD_LAYER_CYLINDER_AXES, CAD_LAYER_MESH,
    CAD_LAYER_PLANES,
};
use crate::drawing::{codec_error, codec_version, frame_active_view, layer_color};
use crate::surfaces::{class_color, FaceClass};
use crate::{DrawingFormat, DrawingUnits, DrawingVersion, LoadError};

/// A polyface mesh numbers its vertices with 16-bit integers from 1, and a
/// DWG stores its counts of vertices and faces in as many bits.
pub(crate) const MAX_POLYFACE_VERTICES: usize = i16::MAX as usize;
pub(crate) const MAX_POLYFACE_FACES: usize = i16::MAX as usize;
/// The most faces of one `MESH` entity. The codec wrote and read back a
/// single entity of 179,000 triangles in a DWG and lost one of 318,000, so a
/// larger mesh is split well below that.
pub(crate) const MAX_MESH_FACES: usize = 65_536;
/// The most vertices of one `MESH` entity. The codec reads no list of more
/// than 100,000 items from a DWG, and a mesh of more vertices comes back
/// without its faces. A mesh in one colour stays well below this; the
/// entities of a mesh with colours, whose vertices on the border of two
/// colours are in both, can reach it.
pub(crate) const MAX_MESH_VERTICES: usize = 65_536;
/// Colour of the axis of a cylinder: black or white, after the background.
const AXIS_RGB: [u8; 3] = [0, 0, 0];
/// Colour of the scanned surface of a cylinder.
const CYLINDER_RGB: [u8; 3] = [176, 152, 206];

/// The version the 3D files are written in: the default of the drawing
/// export, which has the `MESH` entity.
pub(crate) const CAD_VERSION: DrawingVersion = DrawingVersion::R2013;

/// The layer of the flat faces of a class.
pub fn plane_layer(class: FaceClass) -> String {
    format!("{CAD_LAYER_PLANES}-{}", class.name().to_ascii_uppercase())
}

/// Write the model to `destination` and return the size of the file. The
/// file is written beside it and put in place when complete.
pub(crate) fn write_model_cad(
    model: &Model3d,
    destination: &Path,
    format: DrawingFormat,
) -> Result<u64, LoadError> {
    validate(model)?;
    let document = build(model)?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = BufWriter::new(temporary.as_file());
    match format {
        DrawingFormat::Dxf => DxfWriter::new(&document).write_to_writer(&mut writer),
        DrawingFormat::Dwg => DwgWriter::write_to_writer(&mut writer, &document),
    }
    .map_err(codec_error)?;
    writer.flush()?;
    drop(writer);
    drop(document);
    temporary.as_file().sync_all()?;
    let bytes = temporary.as_file().metadata()?.len();
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(bytes)
}

/// Refuse a model a file could not hold before anything is written.
pub(crate) fn validate(model: &Model3d) -> Result<(), LoadError> {
    if model.objects.is_empty() {
        return Err(LoadError::InvalidData("there is nothing to export".into()));
    }
    for object in &model.objects {
        let count = object.vertices.len();
        let finite = object
            .positions()
            .all(|position| position.iter().all(|value| value.is_finite()));
        let indexed = object
            .triangles
            .iter()
            .flatten()
            .chain(
                object
                    .polygons
                    .iter()
                    .flat_map(|polygon| polygon.outer.iter().chain(polygon.holes.iter().flatten())),
            )
            .all(|index| (*index as usize) < count);
        let shaped = object.cylinder.is_none_or(|shape| {
            shape.radius.is_finite() && shape.radius > 0.0 && shape.start != shape.end
        });
        if !finite
            || !indexed
            || !shaped
            || (object.triangles.is_empty() && object.cylinder.is_none())
        {
            return Err(LoadError::InvalidData(format!(
                "{} cannot be exported",
                object.name
            )));
        }
    }
    Ok(())
}

fn build(model: &Model3d) -> Result<CadDocument, LoadError> {
    let mut document = CadDocument::with_version(codec_version(CAD_VERSION));
    document.header.insertion_units = DrawingUnits::Metres.insertion_units();
    document.header.measurement = 1;
    if let Some([min, max]) = model.extents() {
        document.header.model_space_extents_min = Vector3::new(min[0], min[1], min[2]);
        document.header.model_space_extents_max = Vector3::new(max[0], max[1], max[2]);
        frame_active_view(&mut document, [min[0], min[1]], [max[0], max[1]]);
    }
    let mut layers: HashSet<String> = HashSet::new();
    let mut add_layer = |document: &mut CadDocument, name: &str, rgb: [u8; 3]| {
        if !layers.insert(name.to_owned()) {
            return Ok(());
        }
        let mut entry = Layer::new(name);
        entry.color = layer_color(rgb);
        entry.set_handle(document.allocate_handle());
        document
            .layers
            .add(entry)
            .map_err(|reason| LoadError::InvalidData(format!("layer {name}: {reason}")))
    };
    for object in &model.objects {
        let (layer, rgb) = match object.kind {
            Kind::Plane(class) => (plane_layer(class), class_color(class)),
            Kind::Cylinder => (CAD_LAYER_CYLINDERS.to_owned(), CYLINDER_RGB),
            Kind::Mesh => (CAD_LAYER_MESH.to_owned(), object.rgb),
        };
        add_layer(&mut document, &layer, rgb)?;
        let entities: Vec<(EntityType, Color)> = match object.kind {
            Kind::Mesh => mesh_entities(object),
            Kind::Plane(_) | Kind::Cylinder => polyface_entities(object)
                .into_iter()
                .map(|entity| (entity, Color::ByLayer))
                .collect(),
        };
        for (mut entity, color) in entities {
            let common = entity.common_mut();
            common.layer = layer.clone();
            common.color = color;
            document.add_entity(entity).map_err(codec_error)?;
        }
        if let (Kind::Cylinder, Some(shape)) = (object.kind, object.cylinder) {
            add_layer(&mut document, CAD_LAYER_CYLINDER_AXES, AXIS_RGB)?;
            let mut axis =
                EntityType::Line(Line::from_points(vector(shape.start), vector(shape.end)));
            let common = axis.common_mut();
            common.layer = CAD_LAYER_CYLINDER_AXES.to_owned();
            common.color = Color::ByLayer;
            document.add_entity(axis).map_err(codec_error)?;
        }
    }
    Ok(document)
}

fn vector(position: [f64; 3]) -> Vector3 {
    Vector3::new(position[0], position[1], position[2])
}

/// The triangles of an object as polyface meshes of at most
/// `MAX_POLYFACE_VERTICES` vertices and `MAX_POLYFACE_FACES` faces, with
/// the edges the object hides made invisible.
fn polyface_entities(object: &Object3d) -> Vec<EntityType> {
    chunks(
        object.vertices.len(),
        object.triangles.iter().copied(),
        MAX_POLYFACE_VERTICES,
        MAX_POLYFACE_FACES,
    )
    .into_iter()
    .map(|(vertices, triangles)| {
        let mut mesh = PolyfaceMesh::new();
        for &original in &vertices {
            mesh.add_vertex(PolyfaceVertex::new(vector(
                object.vertices[original as usize],
            )));
        }
        for (local, original) in triangles {
            // Polyface vertices count from 1.
            let [a, b, c] = local.map(|index| index as i16 + 1);
            let mut face = PolyfaceFace::triangle(a, b, c);
            for (slot, (from, to)) in [
                (original[0], original[1]),
                (original[1], original[2]),
                (original[2], original[0]),
            ]
            .into_iter()
            .enumerate()
            {
                if object.hidden_edges.contains(&edge(from, to)) {
                    face.set_edge_visibility(slot, false);
                }
            }
            mesh.add_face(face);
        }
        EntityType::PolyfaceMesh(mesh)
    })
    .collect()
}

/// The triangles of a mesh as `MESH` entities of at most `MAX_MESH_FACES`
/// faces and `MAX_MESH_VERTICES` vertices each, with the colour each is
/// drawn in. A mesh in one colour takes the colour of its layer. A mesh with colours gets an entity of a true
/// colour per palette entry, and more where that entry has more triangles
/// than one entity holds; a vertex on the border of two colours is in the
/// entities of both.
fn mesh_entities(object: &Object3d) -> Vec<(EntityType, Color)> {
    let count = object.vertices.len();
    let groups: Vec<(Vec<Chunk>, Color)> =
        match face_colours(object.colors.as_deref(), count, &object.triangles) {
            None => vec![(
                chunks(
                    count,
                    object.triangles.iter().copied(),
                    MAX_MESH_VERTICES,
                    MAX_MESH_FACES,
                ),
                Color::ByLayer,
            )],
            Some(colours) => colours
                .groups()
                .into_iter()
                .zip(&colours.palette)
                .filter(|(group, _)| !group.is_empty())
                .map(|(group, &[r, g, b])| {
                    let triangles = group
                        .iter()
                        .map(|&triangle| object.triangles[triangle as usize]);
                    (
                        chunks(count, triangles, MAX_MESH_VERTICES, MAX_MESH_FACES),
                        Color::Rgb { r, g, b },
                    )
                })
                .collect(),
        };
    groups
        .into_iter()
        .flat_map(|(parts, color)| parts.into_iter().map(move |part| (part, color)))
        .map(|((vertices, triangles), color)| {
            let mut mesh = Mesh::new();
            mesh.vertices = vertices
                .iter()
                .map(|&original| vector(object.vertices[original as usize]))
                .collect();
            for (local, _) in triangles {
                mesh.add_triangle(local[0] as usize, local[1] as usize, local[2] as usize);
            }
            mesh.compute_edges();
            (EntityType::Mesh(mesh), color)
        })
        .collect()
}

/// A chunk: the original vertex of every local one, and every triangle in
/// local and in original vertices.
type Chunk = (Vec<u32>, Vec<([u32; 3], [u32; 3])>);

/// Triangles over `vertex_count` vertices in order, cut into chunks of at
/// most `max_vertices` vertices and `max_faces` triangles. Triangles that
/// fit as a whole and use every vertex keep their vertices as they are;
/// otherwise every chunk holds the vertices of its own triangles only.
fn chunks(
    vertex_count: usize,
    triangles: impl ExactSizeIterator<Item = [u32; 3]> + Clone,
    max_vertices: usize,
    max_faces: usize,
) -> Vec<Chunk> {
    const UNSET: u32 = u32::MAX;
    if vertex_count <= max_vertices && triangles.len() <= max_faces {
        let mut used = vec![false; vertex_count];
        for corner in triangles.clone().flatten() {
            used[corner as usize] = true;
        }
        if used.iter().all(|used| *used) {
            let vertices = (0..vertex_count as u32).collect();
            let triangles = triangles.map(|triangle| (triangle, triangle)).collect();
            return vec![(vertices, triangles)];
        }
    }
    let mut done = Vec::new();
    let mut local_of = vec![UNSET; vertex_count];
    let mut current: Chunk = (Vec::new(), Vec::new());
    for triangle in triangles {
        let new = (0..3)
            .filter(|&slot| {
                local_of[triangle[slot] as usize] == UNSET
                    && !triangle[..slot].contains(&triangle[slot])
            })
            .count();
        if current.1.len() + 1 > max_faces || current.0.len() + new > max_vertices {
            for &original in &current.0 {
                local_of[original as usize] = UNSET;
            }
            done.push(std::mem::take(&mut current));
        }
        let local = triangle.map(|original| {
            let slot = &mut local_of[original as usize];
            if *slot == UNSET {
                current.0.push(original);
                *slot = (current.0.len() - 1) as u32;
            }
            *slot
        });
        current.1.push((local, triangle));
    }
    if !current.1.is_empty() {
        done.push(current);
    }
    done
}
