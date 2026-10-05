//! Writes a [`Drawing2d`] as DXF or DWG. Both formats come from one codec
//! document; only the last step differs.

use std::collections::HashSet;
use std::io::{BufWriter, Write};
use std::path::Path;

use cadcodec::entities::hatch::{BoundaryEdge, BoundaryPath, BoundaryPathFlags, PolylineEdge};
use cadcodec::entities::{EntityType, Hatch, LwPolyline, Point, Text};
use cadcodec::tables::{Layer, TableEntry};
use cadcodec::{CadDocument, Color, DwgWriter, DxfError, DxfVersion, DxfWriter, Vector2, Vector3};

use super::{
    Drawing2d, DrawingEntity, DrawingFormat, DrawingLayer, DrawingVersion, MAX_LAYER_NAME_CHARS,
};
use crate::LoadError;

/// Building the document asks this often whether to go on.
const PROGRESS_ENTITIES: usize = 4_096;

/// Writes the drawing to `destination` and returns the size of the file.
///
/// Coordinates are multiplied by the unit factor of the drawing here, and the
/// header gets the matching insertion units. A drawing in millimetres that is
/// opened again as a point cloud is therefore a thousand times larger than the
/// scan: the DXF reader returns the POINT entities as they are written and
/// skips polylines, fills and text.
///
/// Solid fills are written before everything else, whatever their place in
/// the model, so that outlines, points, the frame and the text are painted
/// over them; all other entities keep their order.
///
/// The file is written beside the destination and put in place when it is
/// complete, so a failed export leaves an earlier file as it was. The whole
/// drawing is in memory until then; see [`MAX_DRAWING_POINTS`] for what a
/// point costs.
///
/// [`MAX_DRAWING_POINTS`]: super::MAX_DRAWING_POINTS
pub fn write_drawing(
    drawing: &Drawing2d,
    destination: &Path,
    format: DrawingFormat,
    version: DrawingVersion,
) -> Result<u64, LoadError> {
    write_drawing_progress(drawing, destination, format, version, |_| Ok(()))
}

/// As [`write_drawing`]. `progress` gets the number of entities built so far,
/// and once more the total just before the file is put in place; an error
/// from it, such as [`LoadError::Cancelled`], stops the export.
pub fn write_drawing_progress(
    drawing: &Drawing2d,
    destination: &Path,
    format: DrawingFormat,
    version: DrawingVersion,
    mut progress: impl FnMut(usize) -> Result<(), LoadError>,
) -> Result<u64, LoadError> {
    drawing.validate()?;
    let document = build_document(drawing, format, version, &mut progress)?;
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
    progress(drawing.entities.len())?;
    let bytes = temporary.as_file().metadata()?.len();
    temporary
        .persist(destination)
        .map_err(|error| LoadError::Io(error.error))?;
    Ok(bytes)
}

fn build_document(
    drawing: &Drawing2d,
    format: DrawingFormat,
    version: DrawingVersion,
    progress: &mut impl FnMut(usize) -> Result<(), LoadError>,
) -> Result<CadDocument, LoadError> {
    let factor = drawing.units.factor();
    let scaled = |uv: [f64; 2]| Vector2::new(uv[0] * factor, uv[1] * factor);
    let mut document = CadDocument::with_version(codec_version(version));
    document.header.insertion_units = drawing.units.insertion_units();
    // Metric, so that linetypes and hatch patterns of a recipient scale right.
    document.header.measurement = 1;
    if let Some([min, max]) = drawing.extents() {
        document.header.model_space_extents_min =
            Vector3::new(min[0] * factor, min[1] * factor, 0.0);
        document.header.model_space_extents_max =
            Vector3::new(max[0] * factor, max[1] * factor, 0.0);
    }

    // A DXF before R2007 is text in the code page its header names, but the
    // codec writes every DXF as UTF-8. Escaped, the file is plain ASCII and
    // reads the same whatever the code page. In a DWG of that age the codec
    // encodes the strings itself.
    let escape = format == DrawingFormat::Dxf && version == DrawingVersion::R2004;
    let text = |value: &str| {
        if escape {
            escape_non_ascii(value, usize::MAX)
        } else {
            value.to_string()
        }
    };

    let layer_names: Vec<String> = if escape {
        escaped_layer_names(&drawing.layers)
    } else {
        drawing
            .layers
            .iter()
            .map(|layer| layer.name.clone())
            .collect()
    };
    for (layer, name) in drawing.layers.iter().zip(&layer_names) {
        let color = layer_color(layer.rgb);
        if let Some(existing) = document.layers.get_mut(name) {
            // Layer "0" is always there.
            existing.color = color;
            continue;
        }
        let mut entry = Layer::new(name);
        entry.color = color;
        // A table entry without a handle of its own can get lost in a DWG.
        entry.set_handle(document.allocate_handle());
        document
            .layers
            .add(entry)
            .map_err(|reason| LoadError::InvalidData(format!("drawing layer: {reason}")))?;
    }

    // A drawing program paints the entities in the order of the file. A solid
    // fill written after the points and outlines of its wall would cover
    // them, so the fills go first, wherever they stand in the model.
    let is_fill = |entity: &DrawingEntity| matches!(entity, DrawingEntity::Fill { .. });
    let fills = drawing
        .entities
        .iter()
        .filter(|(_, entity)| is_fill(entity));
    let others = drawing
        .entities
        .iter()
        .filter(|(_, entity)| !is_fill(entity));
    for (index, (layer, entity)) in fills.chain(others).enumerate() {
        if index % PROGRESS_ENTITIES == 0 {
            progress(index)?;
        }
        let mut color = Color::ByLayer;
        let mut built = match entity {
            DrawingEntity::Point { uv, rgb } => {
                if let Some([r, g, b]) = *rgb {
                    color = Color::from_rgb(r, g, b);
                }
                let at = scaled(*uv);
                EntityType::Point(Point::at(Vector3::new(at.x, at.y, 0.0)))
            }
            DrawingEntity::Polyline { points, closed } => {
                let mut polyline =
                    LwPolyline::from_points(points.iter().copied().map(scaled).collect());
                polyline.is_closed = *closed;
                EntityType::LwPolyline(polyline)
            }
            DrawingEntity::Fill { outer, holes } => {
                let mut hatch = Hatch::solid();
                for (ring_index, ring) in std::iter::once(outer).chain(holes).enumerate() {
                    let mut flags = BoundaryPathFlags::new();
                    flags.set_polyline(true);
                    flags.set_external(ring_index == 0);
                    let mut path = BoundaryPath::with_flags(flags);
                    path.add_edge(BoundaryEdge::Polyline(PolylineEdge::new(
                        ring.iter().copied().map(scaled).collect(),
                        true,
                    )));
                    hatch.add_path(path);
                }
                EntityType::Hatch(hatch)
            }
            DrawingEntity::Text { at, height, value } => {
                let at = scaled(*at);
                EntityType::Text(
                    Text::with_value(text(value), Vector3::new(at.x, at.y, 0.0))
                        .with_height(height * factor),
                )
            }
        };
        let common = built.common_mut();
        common.layer = layer_names[usize::from(*layer)].clone();
        common.color = color;
        document.add_entity(built).map_err(codec_error)?;
    }
    Ok(document)
}

pub(crate) fn codec_version(version: DrawingVersion) -> DxfVersion {
    match version {
        DrawingVersion::R2004 => DxfVersion::AC1018,
        DrawingVersion::R2010 => DxfVersion::AC1024,
        DrawingVersion::R2013 => DxfVersion::AC1027,
        DrawingVersion::R2018 => DxfVersion::AC1032,
    }
}

/// Colour index 7 is black on a light background and white on a dark one.
/// True black or true white would vanish on one of the two.
pub(crate) fn layer_color(rgb: [u8; 3]) -> Color {
    match rgb {
        [0, 0, 0] | [255, 255, 255] => Color::Index(7),
        [r, g, b] => Color::from_rgb(r, g, b),
    }
}

/// Every character outside ASCII as `\U+XXXX`, in UTF-16 units: the way an
/// older DXF names a character that its code page may not have. The result
/// is cut to `limit` characters, between characters of `value` and never
/// inside an escape.
fn escape_non_ascii(value: &str, limit: usize) -> String {
    let mut escaped = String::with_capacity(value.len());
    let mut piece = String::new();
    for character in value.chars() {
        piece.clear();
        if character.is_ascii() {
            piece.push(character);
        } else {
            let mut units = [0u16; 2];
            for unit in character.encode_utf16(&mut units) {
                piece.push_str(&format!(r"\U+{unit:04X}"));
            }
        }
        if escaped.len() + piece.len() > limit {
            break;
        }
        escaped.push_str(&piece);
    }
    escaped
}

/// The layer names of an escaped DXF. An escape is seven characters for one,
/// so a name the model accepted can pass the length a layer table allows; it
/// is cut to that length. Scans often differ in the last characters of their
/// name only, so names that are equal after the cut get a number to stay
/// apart.
fn escaped_layer_names(layers: &[DrawingLayer]) -> Vec<String> {
    let mut taken = HashSet::with_capacity(layers.len());
    let mut names = Vec::with_capacity(layers.len());
    for layer in layers {
        let mut name = escape_non_ascii(&layer.name, MAX_LAYER_NAME_CHARS);
        let mut copy = 1;
        while !taken.insert(name.to_ascii_uppercase()) {
            copy += 1;
            let suffix = format!("~{copy}");
            name = escape_non_ascii(&layer.name, MAX_LAYER_NAME_CHARS - suffix.len()) + &suffix;
        }
        names.push(name);
    }
    names
}

pub(crate) fn codec_error(error: DxfError) -> LoadError {
    match error {
        DxfError::Io(error) => LoadError::Io(error),
        other => LoadError::InvalidData(format!("drawing could not be written: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use cadcodec::{DwgReader, DxfReader};

    use super::*;
    use crate::drawing::{
        grouped, source_point_layer, DrawingUnits, DEFAULT_DRAWING_POINTS, LAYER_CUT_FILL,
        LAYER_CUT_OUTLINE, LAYER_FRAME, LAYER_INFO, LAYER_POINTS, LAYER_RGB_CONTRAST,
        MAX_DRAWING_POINTS,
    };

    const POINTS: [([f64; 2], Option<[u8; 3]>); 4] = [
        ([0.5, 0.25], None),
        ([3.215, 0.25], Some([200, 30, 40])),
        ([1.0, 2.75], Some([0, 0, 0])),
        ([-0.125, 1.5], None),
    ];
    const OUTER: [[f64; 2]; 4] = [[0.0, 0.0], [4.0, 0.0], [4.0, 3.0], [0.0, 3.0]];
    const HOLE: [[f64; 2]; 4] = [[1.0, 1.0], [1.0, 2.0], [2.0, 2.0], [2.0, 1.0]];
    const OPEN_LINE: [[f64; 2]; 3] = [[0.0, -0.5], [2.0, -0.5], [2.0, -1.0]];
    const INFO: &str = "Plan; cut plane Z = 2.500 m";

    /// Points with and without colour, an open polyline, a region with a
    /// hole, the frame and the info text. The points come before the region,
    /// as they do when a caller draws the scan first.
    fn sample(units: DrawingUnits) -> Drawing2d {
        let mut drawing = Drawing2d::new(units);
        let points = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
        for (uv, rgb) in POINTS {
            drawing.add_point(points, uv, rgb);
        }
        drawing.add_polyline(points, OPEN_LINE.to_vec(), false);
        drawing
            .add_cut_region(OUTER.to_vec(), vec![HOLE.to_vec()])
            .unwrap();
        drawing.add_frame([-0.5, -1.5], [4.5, 3.5]).unwrap();
        drawing.add_info([-0.5, -1.75], 0.1, INFO).unwrap();
        drawing
    }

    fn read_back(path: &Path, format: DrawingFormat) -> CadDocument {
        match format {
            DrawingFormat::Dxf => DxfReader::from_file(path).unwrap().read().unwrap(),
            DrawingFormat::Dwg => DwgReader::from_file(path).unwrap().read().unwrap(),
        }
    }

    fn assert_near(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() <= 1e-6,
            "{actual} is not {expected}"
        );
    }

    fn assert_ring(actual: &[[f64; 2]], expected: &[[f64; 2]], factor: f64) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert_near(actual[0], expected[0] * factor);
            assert_near(actual[1], expected[1] * factor);
        }
    }

    /// Compares what the codec reads from a written file with the sample.
    fn assert_sample(document: &CadDocument, units: DrawingUnits) {
        let factor = units.factor();
        assert_eq!(document.header.insertion_units, units.insertion_units());
        for name in [
            LAYER_POINTS,
            LAYER_CUT_FILL,
            LAYER_CUT_OUTLINE,
            LAYER_FRAME,
            LAYER_INFO,
        ] {
            assert!(document.layers.contains(name), "layer {name} is missing");
        }
        assert_eq!(
            document.layers.get(LAYER_CUT_FILL).unwrap().color,
            Color::from_rgb(128, 128, 128)
        );
        assert_eq!(
            document.layers.get(LAYER_CUT_OUTLINE).unwrap().color,
            Color::Index(7)
        );
        assert_near(document.header.model_space_extents_min.x, -0.5 * factor);
        assert_near(document.header.model_space_extents_min.y, -1.75 * factor);
        assert_near(document.header.model_space_extents_max.x, 4.5 * factor);
        assert_near(document.header.model_space_extents_max.y, 3.5 * factor);

        // The fill is the first entity, so that nothing lies under it.
        assert!(matches!(
            document.entities().next(),
            Some(EntityType::Hatch(_))
        ));
        let mut points = Vec::new();
        let mut polylines = Vec::new();
        let mut hatches = 0;
        let mut texts = 0;
        for entity in document.entities() {
            let layer = entity.common().layer.as_str();
            match entity {
                EntityType::Point(point) => {
                    assert_eq!(layer, LAYER_POINTS);
                    assert_near(point.location.z, 0.0);
                    points.push((
                        [point.location.x, point.location.y],
                        point
                            .common
                            .color
                            .rgb()
                            .filter(|_| point.common.color.is_true_color()),
                    ));
                }
                EntityType::LwPolyline(polyline) => {
                    let ring: Vec<_> = polyline
                        .vertices
                        .iter()
                        .map(|vertex| [vertex.location.x, vertex.location.y])
                        .collect();
                    polylines.push((layer.to_string(), polyline.is_closed, ring));
                }
                EntityType::Hatch(hatch) => {
                    hatches += 1;
                    assert_eq!(layer, LAYER_CUT_FILL);
                    assert!(hatch.is_solid);
                    assert_eq!(hatch.paths.len(), 2);
                    assert!(hatch.paths[0].flags.is_external());
                    assert!(!hatch.paths[1].flags.is_external());
                    for (path, expected) in hatch.paths.iter().zip([OUTER, HOLE]) {
                        assert!(path.flags.is_polyline());
                        let [BoundaryEdge::Polyline(edge)] = path.edges.as_slice() else {
                            panic!("a ring is one polyline edge");
                        };
                        let ring: Vec<_> = edge
                            .vertices
                            .iter()
                            .map(|vertex| [vertex.x, vertex.y])
                            .collect();
                        assert_ring(&ring, &expected, factor);
                    }
                }
                EntityType::Text(text) => {
                    texts += 1;
                    assert_eq!(layer, LAYER_INFO);
                    assert_eq!(text.value, INFO);
                    assert_near(text.insertion_point.x, -0.5 * factor);
                    assert_near(text.insertion_point.y, -1.75 * factor);
                    assert_near(text.height, 0.1 * factor);
                }
                other => panic!("unexpected entity {other:?}"),
            }
        }
        assert_eq!(points.len(), POINTS.len());
        for ((at, rgb), (expected, expected_rgb)) in points.iter().zip(POINTS) {
            assert_near(at[0], expected[0] * factor);
            assert_near(at[1], expected[1] * factor);
            assert_eq!(
                *rgb,
                expected_rgb.map(|[r, g, b]| (r, g, b)),
                "point colour"
            );
        }
        // The open line, the two outlines of the region and the frame.
        assert_eq!(polylines.len(), 4);
        let expected_frame = [[-0.5, -1.5], [4.5, -1.5], [4.5, 3.5], [-0.5, 3.5]];
        let expected: [(&str, bool, &[[f64; 2]]); 4] = [
            (LAYER_POINTS, false, &OPEN_LINE),
            (LAYER_CUT_OUTLINE, true, &OUTER),
            (LAYER_CUT_OUTLINE, true, &HOLE),
            (LAYER_FRAME, true, &expected_frame),
        ];
        for ((layer, closed, ring), (expected_layer, expected_closed, expected_ring)) in
            polylines.iter().zip(expected)
        {
            assert_eq!(layer, expected_layer);
            assert_eq!(*closed, expected_closed);
            assert_ring(ring, expected_ring, factor);
        }
        assert_eq!(hatches, 1);
        assert_eq!(texts, 1);
    }

    #[test]
    fn every_format_and_version_reads_back_the_same_drawing() {
        let dir = tempfile::tempdir().unwrap();
        for format in DrawingFormat::ALL {
            for version in DrawingVersion::ALL {
                let path = dir
                    .path()
                    .join(format!("{}.{}", version.key(), format.extension()));
                let drawing = sample(DrawingUnits::Millimetres);
                let bytes = write_drawing(&drawing, &path, format, version).unwrap();
                let content = fs::read(&path).unwrap();
                assert_eq!(bytes, content.len() as u64);
                let tag = version.tag().as_bytes();
                match format {
                    DrawingFormat::Dwg => assert_eq!(&content[..tag.len()], tag),
                    DrawingFormat::Dxf => {
                        // The header holds the version tag on a line of its
                        // own, and the tag of no other version.
                        let text = String::from_utf8(content).unwrap();
                        let tags: Vec<_> = text
                            .lines()
                            .map(str::trim)
                            .filter(|line| DrawingVersion::ALL.iter().any(|any| any.tag() == *line))
                            .collect();
                        assert_eq!(tags, [version.tag()]);
                        // The order in the file itself, not only as the
                        // codec returns it: the fill, then the points and
                        // lines in the order of the model.
                        let entities: Vec<_> = text
                            .lines()
                            .map(str::trim)
                            .skip_while(|line| *line != "ENTITIES")
                            .take_while(|line| *line != "ENDSEC")
                            .filter(|line| ["POINT", "LWPOLYLINE", "HATCH", "TEXT"].contains(line))
                            .collect();
                        let mut expected = vec!["HATCH"];
                        expected.extend(["POINT"; 4]);
                        expected.extend(["LWPOLYLINE"; 4]);
                        expected.push("TEXT");
                        assert_eq!(entities, expected);
                    }
                }
                let document = read_back(&path, format);
                assert_eq!(document.version, codec_version(version));
                assert_sample(&document, DrawingUnits::Millimetres);
            }
        }
        // Nothing but the eight drawings is left in the folder.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 8);
    }

    #[test]
    fn metres_keep_the_coordinates_and_set_the_units() {
        let dir = tempfile::tempdir().unwrap();
        for format in DrawingFormat::ALL {
            let path = dir.path().join(format!("metres.{}", format.extension()));
            write_drawing(
                &sample(DrawingUnits::Metres),
                &path,
                format,
                DrawingVersion::default(),
            )
            .unwrap();
            assert_sample(&read_back(&path, format), DrawingUnits::Metres);
        }
    }

    #[test]
    fn own_dxf_reader_returns_the_points_and_skips_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        for version in DrawingVersion::ALL {
            let path = dir.path().join(format!("{}.dxf", version.key()));
            write_drawing(
                &sample(DrawingUnits::Millimetres),
                &path,
                DrawingFormat::Dxf,
                version,
            )
            .unwrap();
            // Polylines, the fill and the text are in the file and are passed
            // over: only the four POINT entities come back.
            let text = fs::read_to_string(&path).unwrap();
            for entity in ["LWPOLYLINE", "HATCH", "TEXT"] {
                assert!(text.lines().any(|line| line.trim() == entity), "{entity}");
            }
            let cloud = crate::open(&path, 100).unwrap();
            assert_eq!(cloud.total_points, POINTS.len() as u64);
            assert_eq!(cloud.points.len(), POINTS.len());
            for (point, (uv, rgb)) in cloud.points.iter().zip(POINTS) {
                // Millimetres: a thousand times the scan it was drawn from.
                assert_near(point.xyz[0], uv[0] * 1000.0);
                assert_near(point.xyz[1], uv[1] * 1000.0);
                assert_near(point.xyz[2], 0.0);
                assert_eq!(point.rgb, rgb);
            }
            assert!(crate::read_dxf_mesh(&path).unwrap().is_none());
        }
    }

    #[test]
    fn names_outside_ascii_survive_in_every_version() {
        let dir = tempfile::tempdir().unwrap();
        let name = crate::drawing::source_point_layer("café-3°");
        let info = "±0,010 m 𝄞";
        for format in DrawingFormat::ALL {
            for version in DrawingVersion::ALL {
                let path = dir
                    .path()
                    .join(format!("{}.{}", version.key(), format.extension()));
                let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
                let layer = drawing.layer(&name, [0, 200, 0]).unwrap();
                drawing.add_point(layer, [1.0, 2.0], None);
                drawing.add_info([0.0, 0.0], 0.1, info).unwrap();
                write_drawing(&drawing, &path, format, version).unwrap();
                let document = read_back(&path, format);
                let text = document
                    .entities()
                    .find_map(|entity| match entity {
                        EntityType::Text(text) => Some(text.value.clone()),
                        _ => None,
                    })
                    .unwrap();
                let point_layer = document
                    .entities()
                    .find_map(|entity| match entity {
                        EntityType::Point(point) => Some(point.common.layer.clone()),
                        _ => None,
                    })
                    .unwrap();
                if format == DrawingFormat::Dxf && version == DrawingVersion::R2004 {
                    // Escaped: nothing in the file depends on a code page.
                    assert!(fs::read(&path).unwrap().is_ascii());
                    assert_eq!(point_layer, r"OPS-POINTS-caf\U+00E9-3\U+00B0");
                    assert!(document.layers.contains(&point_layer));
                    assert_eq!(text, r"\U+00B10,010 m \U+D834\U+DD1E");
                } else {
                    assert_eq!(point_layer, name, "{format} {version:?}");
                    assert!(document.layers.contains(&name), "{format} {version:?}");
                    assert_eq!(text, info, "{format} {version:?}");
                }
                if format == DrawingFormat::Dxf {
                    assert_eq!(crate::open(&path, 10).unwrap().total_points, 1);
                }
            }
        }
    }

    #[test]
    fn long_names_outside_ascii_stay_within_the_layer_name_length_and_apart() {
        // Three scans whose names differ in the last character only. Escaped,
        // each name is 11 + 40 * 7 + 1 = 292 characters, and the difference
        // lies beyond the 255 that a layer table holds.
        let stems: Vec<String> = (1..=3)
            .map(|scan| format!("{}{scan}", "Ж".repeat(40)))
            .collect();
        let head = format!("{LAYER_POINTS}-{}", r"\U+0416".repeat(34));
        let expected = [head.clone(), format!("{head}~2"), format!("{head}~3")];
        assert!(expected.iter().all(|name| name.len() <= 255));
        assert!(head.len() + 7 > 255);

        let dir = tempfile::tempdir().unwrap();
        for format in DrawingFormat::ALL {
            for version in DrawingVersion::ALL {
                let path = dir
                    .path()
                    .join(format!("{}.{}", version.key(), format.extension()));
                let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
                for (scan, stem) in stems.iter().enumerate() {
                    let layer = drawing
                        .layer(&source_point_layer(stem), [0, 200, 0])
                        .unwrap();
                    drawing.add_point(layer, [scan as f64, 0.0], None);
                }
                write_drawing(&drawing, &path, format, version).unwrap();
                let document = read_back(&path, format);
                let mut point_layers = vec![String::new(); stems.len()];
                for entity in document.entities() {
                    let EntityType::Point(point) = entity else {
                        panic!("unexpected entity {entity:?}");
                    };
                    point_layers[point.location.x.round() as usize / 1000] =
                        point.common.layer.clone();
                }
                let escaped = format == DrawingFormat::Dxf && version == DrawingVersion::R2004;
                for (scan, layer) in point_layers.iter().enumerate() {
                    // Only the escaped file needs the cut; every other one
                    // keeps the name of the scan.
                    let name = if escaped {
                        expected[scan].clone()
                    } else {
                        source_point_layer(&stems[scan])
                    };
                    assert_eq!(*layer, name, "{format} {version:?}");
                    assert!(document.layers.contains(&name), "{format} {version:?}");
                    assert!(name.chars().count() <= 255);
                }
            }
        }

        // A cut falls between characters, never inside an escape, and a
        // character outside the basic plane is two escapes that stay together.
        assert_eq!(escape_non_ascii("ab𝄞c", usize::MAX), r"ab\U+D834\U+DD1Ec");
        assert_eq!(escape_non_ascii("ab𝄞c", 16), r"ab\U+D834\U+DD1E");
        assert_eq!(escape_non_ascii("ab𝄞c", 15), "ab");
        assert_eq!(escape_non_ascii("abc", 2), "ab");
        // Names that are equal after escaping stay apart as well, whatever
        // the case, and a number that is taken is passed over.
        let layer = |name: &str| DrawingLayer {
            name: name.into(),
            rgb: LAYER_RGB_CONTRAST,
        };
        assert_eq!(
            escaped_layer_names(&[
                layer("é"),
                layer(r"\U+00E9~2"),
                layer(r"\u+00e9"),
                layer("plain")
            ]),
            [r"\U+00E9", r"\U+00E9~2", r"\u+00e9~3", "plain"]
        );
    }

    #[test]
    fn coordinates_far_from_the_origin_keep_their_precision() {
        // National grid coordinates in millimetres run into the hundreds of
        // millions; both formats have to hold them to the micrometre.
        let dir = tempfile::tempdir().unwrap();
        let at = [207_000.123_456, 474_000.654_321];
        for format in DrawingFormat::ALL {
            let path = dir.path().join(format!("far.{}", format.extension()));
            let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
            let layer = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
            drawing.add_point(layer, at, None);
            drawing.add_point(layer, [at[0] + 3.215, at[1]], None);
            write_drawing(&drawing, &path, format, DrawingVersion::default()).unwrap();
            let document = read_back(&path, format);
            let points: Vec<_> = document
                .entities()
                .filter_map(|entity| match entity {
                    EntityType::Point(point) => Some(point.location),
                    _ => None,
                })
                .collect();
            assert_eq!(points.len(), 2);
            assert_near(points[0].x, 207_000_123.456);
            assert_near(points[0].y, 474_000_654.321);
            // Two targets 3.215 m apart are 3215 drawing units apart.
            assert_near(points[1].x - points[0].x, 3215.0);
        }
    }

    #[test]
    fn failed_or_cancelled_export_leaves_the_destination_as_it_was() {
        let dir = tempfile::tempdir().unwrap();
        for format in DrawingFormat::ALL {
            let path = dir.path().join(format!("plan.{}", format.extension()));
            fs::write(&path, b"earlier export").unwrap();
            let untouched = || {
                assert_eq!(fs::read(&path).unwrap(), b"earlier export");
                // No temporary file is left beside it.
                assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            };
            let version = DrawingVersion::default();

            let mut broken = sample(DrawingUnits::Millimetres);
            broken.add_point(0, [f64::INFINITY, 0.0], None);
            assert!(matches!(
                write_drawing(&broken, &path, format, version),
                Err(LoadError::InvalidData(_))
            ));
            untouched();

            // Cancelled while the document is built.
            let drawing = sample(DrawingUnits::Millimetres);
            assert!(matches!(
                write_drawing_progress(&drawing, &path, format, version, |_| Err(
                    LoadError::Cancelled
                )),
                Err(LoadError::Cancelled)
            ));
            untouched();

            // Cancelled when the file is complete but not yet in place.
            let total = drawing.entities.len();
            let mut calls = Vec::new();
            assert!(matches!(
                write_drawing_progress(&drawing, &path, format, version, |done| {
                    calls.push(done);
                    if done == total {
                        Err(LoadError::Cancelled)
                    } else {
                        Ok(())
                    }
                }),
                Err(LoadError::Cancelled)
            ));
            assert_eq!(calls, [0, total]);
            untouched();

            // Without a cancel the earlier file is replaced.
            let bytes = write_drawing(&drawing, &path, format, version).unwrap();
            assert_eq!(fs::metadata(&path).unwrap().len(), bytes);
            assert_ne!(fs::read(&path).unwrap(), b"earlier export");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
            fs::remove_file(&path).unwrap();
        }
    }

    #[test]
    fn point_limits_follow_what_the_codec_holds_per_entity() {
        // The document keeps every entity as the entity record of the codec,
        // whatever its kind, so the size of that record is the larger part of
        // what a point costs while writing; its handle index and the buffers
        // of the writer were the other 200 to 400 bytes when measured. The
        // comment on the limit states the whole. A codec whose record grows
        // past it, or a limit raised without measuring, fails here.
        const MEASURED_BYTES_PER_POINT: usize = 2_700;
        assert!(std::mem::size_of::<EntityType>() + 300 <= MEASURED_BYTES_PER_POINT);
        const { assert!(MAX_DRAWING_POINTS * MEASURED_BYTES_PER_POINT <= 1_100_000_000) };
        const { assert!(DEFAULT_DRAWING_POINTS * MEASURED_BYTES_PER_POINT <= 450_000_000) };
    }

    #[test]
    fn more_points_than_the_limit_are_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dense.dxf");
        let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
        let layer = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
        for index in 0..=MAX_DRAWING_POINTS {
            drawing.add_point(layer, [index as f64 * 0.005, 0.0], None);
        }
        let mut asked = false;
        let result = write_drawing_progress(
            &drawing,
            &path,
            DrawingFormat::Dxf,
            DrawingVersion::default(),
            |_| {
                asked = true;
                Ok(())
            },
        );
        assert!(matches!(
            result,
            Err(LoadError::InvalidData(reason)) if reason.contains(&grouped(MAX_DRAWING_POINTS))
        ));
        // Refused before the document is built: nothing is in the folder.
        assert!(!asked);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn progress_is_asked_every_few_thousand_entities() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("points.dxf");
        let mut drawing = Drawing2d::new(DrawingUnits::Metres);
        let layer = drawing.layer(LAYER_POINTS, LAYER_RGB_CONTRAST).unwrap();
        for index in 0..10_000 {
            drawing.add_point(layer, [index as f64 * 0.005, 1.0], None);
        }
        let mut calls = Vec::new();
        write_drawing_progress(
            &drawing,
            &path,
            DrawingFormat::Dxf,
            DrawingVersion::default(),
            |done| {
                calls.push(done);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(calls, [0, 4_096, 8_192, 10_000]);
        assert_eq!(crate::open(&path, 10).unwrap().total_points, 10_000);
    }
}
