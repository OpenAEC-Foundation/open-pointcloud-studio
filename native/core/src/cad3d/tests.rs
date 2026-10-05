//! The 3D writers on hand-made models: a wall with an opening, a column and
//! a closed mesh, near zero and at national grid coordinates.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use cadcodec::entities::EntityType;
use cadcodec::{CadDocument, Color, DwgReader, DxfReader};

use super::cad::{
    plane_layer, write_model_cad, CAD_VERSION, MAX_MESH_FACES, MAX_POLYFACE_VERTICES,
};
use super::ifc::{local_origin, write_model_ifc, PROPERTY_SET};
use super::step::{self, check};
use super::*;
use crate::drawing::codec_version;
use crate::{write_mesh, DrawingFormat, MeshFormat};

/// National grid coordinates, as a scan in a survey frame has them.
pub(crate) const FAR: [f64; 3] = [207_000.0, 474_000.0, 3.0];

fn shifted(position: [f64; 3], by: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| position[axis] + by[axis])
}

/// A wall of 4 by 3 m in the plane y = 0 with a window, facing -y; a
/// column of radius 0.2 m and 2.5 m high; and a closed tetrahedron.
pub(crate) fn sample(at: [f64; 3]) -> Model3d {
    let wall_corners = [
        [0.0, 0.0, 0.0],
        [4.0, 0.0, 0.0],
        [4.0, 0.0, 3.0],
        [0.0, 0.0, 3.0],
        [1.0, 0.0, 1.0],
        [1.0, 0.0, 2.0],
        [2.0, 0.0, 2.0],
        [2.0, 0.0, 1.0],
    ];
    // The ring of the opening and the frame round it.
    let wall_triangles = vec![
        [0, 1, 7],
        [0, 7, 4],
        [1, 2, 6],
        [1, 6, 7],
        [2, 3, 5],
        [2, 5, 6],
        [3, 0, 4],
        [3, 4, 5],
    ];
    let polygons = vec![Polygon {
        outer: vec![0, 1, 2, 3],
        holes: vec![vec![4, 5, 6, 7]],
    }];
    let wall = Object3d {
        id: 1,
        name: "Face 1 (wall)".into(),
        kind: Kind::Plane(FaceClass::Wall),
        rgb: [226, 192, 132],
        vertices: wall_corners
            .iter()
            .map(|corner| shifted(*corner, at))
            .collect(),
        hidden_edges: inner_edges(&wall_triangles, &polygons),
        triangles: wall_triangles,
        polygons,
        cylinder: None,
        closed: Some(false),
        properties: vec![
            ("Class", PropertyValue::Label("wall".into())),
            ("Area", PropertyValue::Area(11.0)),
        ],
    };
    // Twelve strips round the column.
    let (centre, radius, height) = ([6.0, 2.0, 0.0], 0.2, 2.5);
    let mut column_corners = Vec::new();
    for strip in 0..=12 {
        let angle = std::f64::consts::TAU * f64::from(strip) / 12.0;
        let x = centre[0] + radius * angle.cos();
        let y = centre[1] + radius * angle.sin();
        column_corners.push(shifted([x, y, 0.0], at));
        column_corners.push(shifted([x, y, height], at));
    }
    let mut column_triangles = Vec::new();
    for strip in 0..12u32 {
        let quad = [2 * strip, 2 * strip + 2, 2 * strip + 3, 2 * strip + 1];
        column_triangles.push([quad[0], quad[1], quad[2]]);
        column_triangles.push([quad[0], quad[2], quad[3]]);
    }
    let column = Object3d {
        id: 2,
        name: "Face 2 (cylinder)".into(),
        kind: Kind::Cylinder,
        rgb: [176, 152, 206],
        vertices: column_corners,
        hidden_edges: column_triangles
            .iter()
            .flat_map(|&[a, b, c]| [edge(a, b), edge(b, c), edge(c, a)])
            .filter(|(low, high)| high - low == 3)
            .collect(),
        triangles: column_triangles,
        polygons: Vec::new(),
        cylinder: Some(Cylinder {
            start: shifted(centre, at),
            end: shifted([centre[0], centre[1], height], at),
            radius,
            across: [1.0, 0.0, 0.0],
            seen_from_inside: false,
        }),
        closed: Some(false),
        properties: vec![("Radius", PropertyValue::Length(radius))],
    };
    let mut model = mesh_model(&tetrahedron(at), "room.e57", &["Units: metres"]);
    model.objects.insert(0, wall);
    model.objects.insert(1, column);
    model
}

pub(crate) fn tetrahedron(at: [f64; 3]) -> MeshGeometry {
    MeshGeometry {
        vertices: [
            [8.0, 0.0, 0.0],
            [9.0, 0.0, 0.0],
            [8.0, 1.0, 0.0],
            [8.0, 0.0, 1.0],
        ]
        .map(|corner| shifted(corner, at))
        .to_vec(),
        triangles: vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
        colors: None,
        normals: None,
    }
}

pub(crate) fn read_cad(path: &Path, format: DrawingFormat) -> CadDocument {
    match format {
        DrawingFormat::Dxf => DxfReader::from_file(path).unwrap().read().unwrap(),
        DrawingFormat::Dwg => DwgReader::from_file(path).unwrap().read().unwrap(),
    }
}

fn near(a: [f64; 3], b: [f64; 3], tolerance: f64) -> bool {
    a.iter().zip(&b).all(|(a, b)| (a - b).abs() <= tolerance)
}

#[test]
fn step_reals_strings_ids_and_times_are_written_as_the_standard_asks() {
    assert_eq!(step::real(0.0), "0.");
    assert_eq!(step::real(1.0), "1.0");
    assert_eq!(step::real(-2.5), "-2.5");
    assert_eq!(step::real(1e-5), "1.E-5");
    assert_eq!(step::real(1.5e-7), "1.5E-7");
    assert_eq!(step::real(2e21), "2.E21");
    assert_eq!(step::length(207_000.123_456_7), "207000.123457");
    assert_eq!(step::length(0.000_000_4), "0.");
    assert_eq!(step::string("it's"), "'it''s'");
    assert_eq!(step::string(r"a\b"), r"'a\\b'");
    assert_eq!(step::string("café 3°"), r"'caf\X2\00E9\X0\ 3\X2\00B0\X0\'");
    assert_eq!(step::string("𝄞\n"), r"'\X2\D834DD1E\X0\'");
    assert_eq!(step::compress_guid(0), "0000000000000000000000");
    assert_eq!(step::compress_guid(u128::MAX), "3$$$$$$$$$$$$$$$$$$$$$");
    let id = step::guid();
    assert_eq!(id.len(), 22);
    assert_ne!(id, step::guid());
    assert_eq!(step::format_time(0), "1970-01-01T00:00:00");
    assert_eq!(step::format_time(951_827_696), "2000-02-29T12:34:56");
    assert_eq!(step::format_time(1_791_158_400), "2026-10-05T00:00:00");
}

#[test]
fn the_model_reads_back_from_dxf_and_dwg_with_layers_and_coordinates() {
    let directory = tempfile::tempdir().unwrap();
    for at in [[0.0; 3], FAR] {
        let model = sample(at);
        for format in DrawingFormat::ALL {
            let path = directory
                .path()
                .join(format!("model.{}", format.extension()));
            let bytes = write_model_cad(&model, &path, format).unwrap();
            assert_eq!(fs::metadata(&path).unwrap().len(), bytes);
            let document = read_cad(&path, format);
            assert_eq!(document.version, codec_version(CAD_VERSION));
            assert_eq!(document.header.insertion_units, 6, "metres");
            for (layer, rgb) in [
                (plane_layer(FaceClass::Wall), [226, 192, 132]),
                (CAD_LAYER_CYLINDERS.to_owned(), [176, 152, 206]),
                (CAD_LAYER_MESH.to_owned(), MESH_RGB),
            ] {
                let entry = document
                    .layers
                    .get(&layer)
                    .unwrap_or_else(|| panic!("{layer}"));
                assert_eq!(entry.color, Color::from_rgb(rgb[0], rgb[1], rgb[2]));
            }
            assert_eq!(
                document.layers.get(CAD_LAYER_CYLINDER_AXES).unwrap().color,
                Color::Index(7)
            );
            let (mut polyfaces, mut meshes, mut lines) = (0, 0, 0);
            for entity in document.entities() {
                let layer = entity.common().layer.as_str();
                assert_eq!(entity.common().color, Color::ByLayer);
                match entity {
                    EntityType::PolyfaceMesh(mesh) => {
                        polyfaces += 1;
                        let object = &model.objects[if layer == plane_layer(FaceClass::Wall) {
                            0
                        } else {
                            assert_eq!(layer, CAD_LAYER_CYLINDERS);
                            1
                        }];
                        assert_eq!(mesh.vertices.len(), object.vertices.len());
                        assert_eq!(mesh.faces.len(), object.triangles.len());
                        for (vertex, expected) in mesh.vertices.iter().zip(&object.vertices) {
                            let found = [vertex.location.x, vertex.location.y, vertex.location.z];
                            assert!(near(found, *expected, 1e-9), "{found:?} {expected:?}");
                        }
                        // The edges drawn are those of the outline and of
                        // the opening, or the lines along the column and
                        // round its ends.
                        let hidden: usize = mesh
                            .faces
                            .iter()
                            .map(|face| {
                                [
                                    face.is_edge1_invisible(),
                                    face.is_edge2_invisible(),
                                    face.is_edge3_invisible(),
                                ]
                                .into_iter()
                                .filter(|hidden| *hidden)
                                .count()
                            })
                            .sum();
                        // Every hidden edge is shared by two triangles.
                        assert_eq!(hidden, 2 * object.hidden_edges.len());
                        assert_eq!(
                            object.hidden_edges.len(),
                            if object.id == 1 { 8 } else { 12 }
                        );
                    }
                    EntityType::Mesh(mesh) => {
                        meshes += 1;
                        assert_eq!(layer, CAD_LAYER_MESH);
                        assert_eq!(mesh.faces.len(), 4);
                        assert_eq!(mesh.edges.len(), 6);
                        let expected = &model.objects[2].vertices;
                        for (vertex, expected) in mesh.vertices.iter().zip(expected) {
                            assert!(near([vertex.x, vertex.y, vertex.z], *expected, 1e-9));
                        }
                    }
                    EntityType::Line(line) => {
                        lines += 1;
                        assert_eq!(layer, CAD_LAYER_CYLINDER_AXES);
                        let shape = model.objects[1].cylinder.unwrap();
                        let start = [line.start.x, line.start.y, line.start.z];
                        let end = [line.end.x, line.end.y, line.end.z];
                        assert!(near(start, shape.start, 1e-9) && near(end, shape.end, 1e-9));
                    }
                    other => panic!("unexpected {other:?}"),
                }
            }
            assert_eq!((polyfaces, meshes, lines), (2, 1, 1), "{format}");
        }
    }
}

/// A flat grid of `side` by `side` vertices as two triangles per cell.
fn grid(side: u32) -> MeshGeometry {
    let mut mesh = MeshGeometry::default();
    for y in 0..side {
        for x in 0..side {
            mesh.vertices
                .push([f64::from(x) * 0.01, f64::from(y) * 0.01, 0.0]);
        }
    }
    for y in 0..side - 1 {
        for x in 0..side - 1 {
            let a = y * side + x;
            mesh.triangles.push([a, a + 1, a + side + 1]);
            mesh.triangles.push([a, a + side + 1, a + side]);
        }
    }
    mesh
}

#[test]
fn a_large_mesh_is_split_into_entities_the_formats_hold() {
    // 2 * 199 * 199 = 79,202 triangles: two MESH entities.
    let mesh = grid(200);
    assert!(mesh.triangles.len() > MAX_MESH_FACES);
    let directory = tempfile::tempdir().unwrap();
    for format in [MeshFormat::Dxf, MeshFormat::Dwg] {
        let path = directory
            .path()
            .join(format!("grid.{}", format.extension()));
        write_mesh(&mesh, &path, format, &["Source: grid.ply"]).unwrap();
        let drawing = if format == MeshFormat::Dxf {
            DrawingFormat::Dxf
        } else {
            DrawingFormat::Dwg
        };
        let document = read_cad(&path, drawing);
        let sizes: Vec<usize> = document
            .entities()
            .map(|entity| match entity {
                EntityType::Mesh(mesh) => mesh.faces.len(),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(
            sizes,
            [MAX_MESH_FACES, mesh.triangles.len() - MAX_MESH_FACES]
        );
        // Each entity holds the vertices of its own triangles, and together
        // they hold every triangle at its place.
        let mut found = HashSet::new();
        for entity in document.entities() {
            let EntityType::Mesh(part) = entity else {
                unreachable!()
            };
            let used: HashSet<usize> = part
                .faces
                .iter()
                .flat_map(|face| face.vertices.clone())
                .collect();
            assert_eq!(used.len(), part.vertices.len());
            for face in &part.faces {
                let key: Vec<[i64; 3]> = face
                    .vertices
                    .iter()
                    .map(|index| {
                        let vertex = part.vertices[*index];
                        [vertex.x, vertex.y, vertex.z].map(|value| (value * 1e4).round() as i64)
                    })
                    .collect();
                assert!(found.insert(key));
            }
        }
        assert_eq!(found.len(), mesh.triangles.len());
    }

    // A polyface mesh numbers its vertices with 16 bits: a face of more
    // corners is split.
    let large = grid(200);
    let object = Object3d {
        id: 1,
        name: "Face 1 (floor)".into(),
        kind: Kind::Plane(FaceClass::Floor),
        rgb: [124, 172, 112],
        vertices: large.vertices,
        triangles: large.triangles,
        polygons: Vec::new(),
        cylinder: None,
        hidden_edges: HashSet::new(),
        closed: Some(false),
        properties: Vec::new(),
    };
    let model = Model3d {
        source: String::new(),
        notes: Vec::new(),
        objects: vec![object],
    };
    let path = directory.path().join("floor.dwg");
    write_model_cad(&model, &path, DrawingFormat::Dwg).unwrap();
    let document = read_cad(&path, DrawingFormat::Dwg);
    let mut faces = 0;
    for entity in document.entities() {
        let EntityType::PolyfaceMesh(part) = entity else {
            panic!("unexpected {entity:?}")
        };
        assert!(part.vertices.len() <= MAX_POLYFACE_VERTICES);
        faces += part.faces.len();
    }
    assert!(document.entities().count() >= 2);
    assert_eq!(faces, model.objects[0].triangles.len());
}

#[test]
fn the_model_as_ifc_is_valid_step_with_its_objects_relative_to_a_local_origin() {
    let directory = tempfile::tempdir().unwrap();
    for at in [[0.0; 3], FAR] {
        let model = sample(at);
        let path = directory.path().join("model.ifc");
        let bytes = write_model_ifc(&model, &path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.len() as u64, bytes);
        assert!(text.is_ascii());
        let instances = check::read(&text).unwrap();
        let of = |name: &str| -> Vec<&check::Instance> {
            instances
                .values()
                .filter(|instance| instance.name == name)
                .collect()
        };
        for single in ["IFCPROJECT", "IFCSITE", "IFCBUILDING", "IFCBUILDINGSTOREY"] {
            assert_eq!(of(single).len(), 1, "{single}");
        }
        assert_eq!(of("IFCRELAGGREGATES").len(), 3);
        let proxies = of("IFCBUILDINGELEMENTPROXY");
        assert_eq!(proxies.len(), 3);
        let types: Vec<&str> = proxies
            .iter()
            .map(|proxy| proxy.attributes[4].as_str())
            .collect();
        assert_eq!(types, ["'Plane (wall)'", "'Cylinder'", "'Mesh'"]);
        for proxy in &proxies {
            assert_eq!(
                proxy.attributes[0].len(),
                24,
                "a quoted id of 22 characters"
            );
            assert_eq!(proxy.attributes[8], ".USERDEFINED.");
        }
        let contained = of("IFCRELCONTAINEDINSPATIALSTRUCTURE");
        assert_eq!(contained.len(), 1);
        assert_eq!(check::references(&contained[0].attributes[4]).len(), 3);
        assert_eq!(of("IFCPROPERTYSET").len(), 3);
        assert!(of("IFCPROPERTYSET")
            .iter()
            .all(|set| set.attributes[2] == format!("'{PROPERTY_SET}'")));
        assert_eq!(of("IFCSTYLEDITEM").len(), 3);

        // The wall: one face with its opening.
        let [with_voids] = of("IFCINDEXEDPOLYGONALFACEWITHVOIDS")[..] else {
            panic!("one face with an opening")
        };
        assert_eq!(with_voids.attributes, ["(1,2,3,4)", "((5,6,7,8))"]);
        assert_eq!(of("IFCPOLYGONALFACESET").len(), 1);
        // The column: an extruded circle of its radius and height.
        let [profile] = of("IFCCIRCLEPROFILEDEF")[..] else {
            panic!("one profile")
        };
        assert_eq!(profile.attributes[3], "0.2");
        let [solid] = of("IFCEXTRUDEDAREASOLID")[..] else {
            panic!("one solid")
        };
        assert_eq!(solid.attributes[3], "2.5");
        assert_eq!(of("IFCPOLYLINE").len(), 1, "the axis");
        // The tetrahedron: closed triangles.
        let [triangulated] = of("IFCTRIANGULATEDFACESET")[..] else {
            panic!("one triangulated set")
        };
        assert_eq!(triangulated.attributes[2], ".T.");
        assert_eq!(
            triangulated.attributes[3],
            "((1,3,2),(1,2,4),(2,3,4),(1,4,3))"
        );

        // The site stands at the local origin; the geometry is relative to it.
        let origin = local_origin(&model);
        if at == FAR {
            // x and y lie far from zero, z does not.
            assert_eq!(origin, [207_005.0, 474_001.0, 0.0]);
        } else {
            assert_eq!(origin, [0.0; 3]);
        }
        let site = of("IFCSITE")[0];
        let placement = &instances[&check::references(&site.attributes[5])[0]];
        assert_eq!(placement.attributes[0], "$");
        let axes = &instances[&check::references(&placement.attributes[1])[0]];
        let point = &instances[&check::references(&axes.attributes[0])[0]];
        let parse = |list: &str| -> Vec<f64> {
            check::members(list)
                .unwrap()
                .iter()
                .map(|value| value.trim_end_matches('.').parse::<f64>().unwrap())
                .collect()
        };
        assert_eq!(parse(&point.attributes[0]), origin.to_vec());
        // The first corner of the wall, relative to the site.
        let lists = of("IFCCARTESIANPOINTLIST3D");
        let first = &check::members(&lists[0].attributes[0]).unwrap()[0];
        let local = parse(first);
        let absolute: Vec<f64> = (0..3).map(|axis| local[axis] + origin[axis]).collect();
        assert!(near(
            [absolute[0], absolute[1], absolute[2]],
            model.objects[0].vertices[0],
            1e-6
        ));
        assert!(local.iter().all(|value| value.abs() < 10.0));
    }
}

#[test]
fn a_shaft_seen_from_inside_is_its_scanned_surface_in_ifc() {
    let mut model = sample([0.0; 3]);
    if let Some(shape) = model.objects[1].cylinder.as_mut() {
        shape.seen_from_inside = true;
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("shaft.ifc");
    write_model_ifc(&model, &path).unwrap();
    let instances = check::read(&fs::read_to_string(&path).unwrap()).unwrap();
    let count = |name: &str| {
        instances
            .values()
            .filter(|instance| instance.name == name)
            .count()
    };
    assert_eq!(count("IFCEXTRUDEDAREASOLID"), 0);
    assert_eq!(count("IFCTRIANGULATEDFACESET"), 2);
    // The axis is there all the same.
    assert_eq!(count("IFCPOLYLINE"), 1);
}

#[test]
fn meshes_write_as_dxf_dwg_and_ifc_through_the_mesh_writer() {
    let directory = tempfile::tempdir().unwrap();
    let mesh = tetrahedron(FAR);
    for format in [MeshFormat::Dxf, MeshFormat::Dwg, MeshFormat::Ifc] {
        let path = directory
            .path()
            .join(format!("mesh.{}", format.extension()));
        assert_eq!(MeshFormat::from_path(&path), Some(format));
        let report = write_mesh(
            &mesh,
            &path,
            format,
            &["Source: C:/scans/hall.e57", "Units: metres"],
        )
        .unwrap();
        assert_eq!(report.origin, None);
        if format == MeshFormat::Ifc {
            let text = fs::read_to_string(&path).unwrap();
            let instances = check::read(&text).unwrap();
            let project = instances
                .values()
                .find(|instance| instance.name == "IFCPROJECT")
                .unwrap();
            assert_eq!(project.attributes[2], "'hall.e57'");
            assert_eq!(project.attributes[3], "'Source: hall.e57; Units: metres'");
        }
    }
    // An open sheet is not closed.
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sheet.ifc");
    write_mesh(&grid(3), &path, MeshFormat::Ifc, &[]).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    let instances = check::read(&text).unwrap();
    let set = instances
        .values()
        .find(|instance| instance.name == "IFCTRIANGULATEDFACESET")
        .unwrap();
    assert_eq!(set.attributes[2], ".F.");
}

#[test]
fn a_failed_export_leaves_the_destination_as_it_was() {
    let directory = tempfile::tempdir().unwrap();
    let mut broken = sample([0.0; 3]);
    broken.objects[0].vertices[2][0] = f64::NAN;
    let empty = Model3d::default();
    for extension in ["dxf", "dwg", "ifc"] {
        let path = directory.path().join(format!("earlier.{extension}"));
        fs::write(&path, b"earlier export").unwrap();
        for model in [&broken, &empty] {
            let result = match extension {
                "dxf" => write_model_cad(model, &path, DrawingFormat::Dxf),
                "dwg" => write_model_cad(model, &path, DrawingFormat::Dwg),
                _ => write_model_ifc(model, &path),
            };
            assert!(matches!(result, Err(crate::LoadError::InvalidData(_))));
            assert_eq!(fs::read(&path).unwrap(), b"earlier export");
        }
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 3);
}
