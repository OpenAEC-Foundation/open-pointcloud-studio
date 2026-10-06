//! The annotations of a 2D drawing: dimensions and leaders, with the
//! geometry they are drawn with and the value a dimension shows.
//!
//! A dimension is aligned: it measures the distance between its two points,
//! along a dimension line parallel to them at an offset, with an extension
//! line from each point and a tick across each end, its value above the
//! middle of the line in millimetres. Its sizes are those of the paper at
//! the scale of the drawing: text 2.5 mm high at 1:100 is 0.25 m in the
//! model. A leader is an arrow from a point to its text.

/// The layers annotations are written on, named like the other layers of a
/// drawing.
pub const LAYER_DIMENSIONS: &str = "OPS-DIMENSIONS";
pub const LAYER_TEXT: &str = "OPS-TEXT";
pub const LAYER_LEADERS: &str = "OPS-LEADERS";
pub const LAYER_LINES: &str = "OPS-LINES";
/// The height of a text on the paper, in millimetres, unless another is
/// given.
pub const DEFAULT_TEXT_HEIGHT: f64 = 2.5;

/// The lines and the text of a dimension, in the units of its points.
#[derive(Debug, Clone, PartialEq)]
pub struct DimensionShape {
    /// The two extension lines, the dimension line and the two ticks.
    pub lines: Vec<[[f64; 2]; 2]>,
    /// The two ends of the dimension line.
    pub ends: [[f64; 2]; 2],
    /// The middle of the baseline of the text.
    pub text_at: [f64; 2],
    /// The turn of the text in radians, so that it reads from the left or
    /// from below.
    pub text_rotation: f64,
}

/// The lines, the arrow and the place of the text of a leader.
#[derive(Debug, Clone, PartialEq)]
pub struct LeaderShape {
    /// From the arrow point to the end of the landing under the text.
    pub line: Vec<[f64; 2]>,
    /// The filled arrowhead at the first point.
    pub arrow: [[f64; 2]; 3],
    /// The start of the baseline of the text, which runs to the left of
    /// this point when `right` is set.
    pub text_at: [f64; 2],
    pub right: bool,
}

fn length(vector: [f64; 2]) -> f64 {
    vector[0].hypot(vector[1])
}

fn along(from: [f64; 2], direction: [f64; 2], distance: f64) -> [f64; 2] {
    [
        from[0] + direction[0] * distance,
        from[1] + direction[1] * distance,
    ]
}

/// The unit vector to the left of the direction from `from` to `to`, and
/// that direction; nothing for two points at one place.
pub fn dimension_axes(from: [f64; 2], to: [f64; 2]) -> Option<([f64; 2], [f64; 2])> {
    let delta = [to[0] - from[0], to[1] - from[1]];
    let size = length(delta);
    if !(size.is_finite() && size > 1e-12) {
        return None;
    }
    let direction = [delta[0] / size, delta[1] / size];
    Some(([-direction[1], direction[0]], direction))
}

/// The geometry of an aligned dimension from `from` to `to`, its line
/// `offset` to the left of the direction from the one to the other (to the
/// right when negative), with text `height` high; the other sizes follow
/// from that height. Nothing for two points at one place.
pub fn dimension_shape(
    from: [f64; 2],
    to: [f64; 2],
    offset: f64,
    height: f64,
) -> Option<DimensionShape> {
    let (normal, direction) = dimension_axes(from, to)?;
    let side = if offset < 0.0 { -1.0 } else { 1.0 };
    let gap = height * 0.4;
    let beyond = height * 0.6;
    let tick = height * 0.5;
    let start = along(from, normal, offset);
    let end = along(to, normal, offset);
    let mut lines = Vec::with_capacity(5);
    for (point, foot) in [(from, start), (to, end)] {
        // An extension line from a little off the point to a little past
        // the dimension line; none when the line lies on the point.
        if offset.abs() > gap {
            lines.push([
                along(point, normal, side * gap),
                along(foot, normal, side * beyond),
            ]);
        }
    }
    lines.push([start, end]);
    // The ticks run at 45 degrees across the ends.
    let slant = [
        (direction[0] + normal[0]) / 2f64.sqrt(),
        (direction[1] + normal[1]) / 2f64.sqrt(),
    ];
    for foot in [start, end] {
        lines.push([along(foot, slant, -tick), along(foot, slant, tick)]);
    }
    let mut rotation = direction[1].atan2(direction[0]);
    if rotation > std::f64::consts::FRAC_PI_2 + 1e-9 {
        rotation -= std::f64::consts::PI;
    } else if rotation <= -std::f64::consts::FRAC_PI_2 + 1e-9 {
        rotation += std::f64::consts::PI;
    }
    // Above the line as the text reads.
    let up = [-rotation.sin(), rotation.cos()];
    let middle = [(start[0] + end[0]) / 2.0, (start[1] + end[1]) / 2.0];
    Some(DimensionShape {
        lines,
        ends: [start, end],
        text_at: along(middle, up, height * 0.5),
        text_rotation: rotation,
    })
}

/// The geometry of a leader from its arrow point `tip` to the point `end`
/// where its text stands, with text `height` high: a landing as long as
/// the text is high runs from `end` away from the tip, and the text starts
/// past it.
pub fn leader_shape(tip: [f64; 2], end: [f64; 2], height: f64) -> LeaderShape {
    let right = end[0] < tip[0];
    let way = if right { -1.0 } else { 1.0 };
    let landing = [end[0] + way * height, end[1]];
    let delta = [tip[0] - end[0], tip[1] - end[1]];
    let size = length(delta).max(1e-12);
    let back = [-delta[0] / size, -delta[1] / size];
    let side = [-back[1], back[0]];
    let long = height;
    let wide = height / 3.0;
    let base = along(tip, back, long);
    LeaderShape {
        line: vec![tip, end, landing],
        arrow: [
            tip,
            along(base, side, wide / 2.0),
            along(base, side, -wide / 2.0),
        ],
        text_at: [landing[0] + way * height * 0.3, end[1] - height * 0.5],
        right,
    }
}

/// The rounding of the value of a dimension in millimetres at a scale:
/// a millimetre at 1:20 and finer, 5 mm at 1:50, 10 mm at 1:100 and
/// coarser.
pub fn dimension_step(scale: f64) -> f64 {
    if scale <= 20.0 {
        1.0
    } else if scale <= 50.0 {
        5.0
    } else {
        10.0
    }
}

/// The value a dimension of `metres` shows at a scale: the distance in
/// whole millimetres, rounded to the step of the scale.
pub fn dimension_value(metres: f64, scale: f64) -> String {
    let step = dimension_step(scale);
    let millimetres = (metres * 1000.0 / step).round() * step;
    format!("{}", millimetres.round() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f64; 2], b: [f64; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
    }

    #[test]
    fn a_dimension_shows_millimetres_rounded_to_the_step_of_its_scale() {
        assert_eq!(dimension_value(3.4512, 100.0), "3450");
        assert_eq!(dimension_value(3.4562, 100.0), "3460");
        assert_eq!(dimension_value(3.4512, 50.0), "3450");
        assert_eq!(dimension_value(3.4538, 50.0), "3455");
        assert_eq!(dimension_value(3.4538, 20.0), "3454");
        assert_eq!(dimension_value(0.0, 100.0), "0");
        assert_eq!(dimension_step(200.0), 10.0);
    }

    #[test]
    fn a_dimension_has_its_line_at_its_offset_with_ticks_and_text_above_it() {
        let shape = dimension_shape([0.0, 0.0], [4.0, 0.0], 1.0, 0.25).unwrap();
        assert!(near(shape.ends[0], [0.0, 1.0]) && near(shape.ends[1], [4.0, 1.0]));
        // Two extension lines, the dimension line and two ticks.
        assert_eq!(shape.lines.len(), 5);
        assert!(near(shape.lines[0][0], [0.0, 0.1]));
        assert!(near(shape.lines[0][1], [0.0, 1.15]));
        assert!(near(shape.text_at, [2.0, 1.125]));
        assert_eq!(shape.text_rotation, 0.0);
        // Measured from right to left and below, the text still reads from
        // the left and stands above its line.
        let back = dimension_shape([4.0, 0.0], [0.0, 0.0], 1.0, 0.25).unwrap();
        assert!(near(back.ends[0], [4.0, -1.0]));
        assert!(back.text_rotation.abs() < 1e-12);
        assert!(near(back.text_at, [2.0, -0.875]));
        // Upright, the text reads from below.
        let up = dimension_shape([0.0, 0.0], [0.0, 3.0], -0.5, 0.25).unwrap();
        assert!((up.text_rotation - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!(dimension_shape([1.0, 1.0], [1.0, 1.0], 1.0, 0.25).is_none());
        // A line on the points has no extension lines.
        assert_eq!(
            dimension_shape([0.0, 0.0], [4.0, 0.0], 0.0, 0.25)
                .unwrap()
                .lines
                .len(),
            3
        );
    }

    /// A plan in millimetres with a wall outline, a dimension of 3.4512 m
    /// at 1:100, a leader, a text and a line on the layers of annotations.
    fn annotated() -> crate::Drawing2d {
        use crate::{Drawing2d, DrawingUnits, LAYER_RGB_CONTRAST};
        let mut drawing = Drawing2d::new(DrawingUnits::Millimetres);
        let outline = drawing
            .layer("OPS-CUT-OUTLINE", LAYER_RGB_CONTRAST)
            .unwrap();
        drawing.add_polyline(
            outline,
            vec![[0.0, 0.0], [3.4512, 0.0], [3.4512, 2.0]],
            false,
        );
        let dimensions = drawing.layer(LAYER_DIMENSIONS, LAYER_RGB_CONTRAST).unwrap();
        drawing.add_dimension(
            dimensions,
            [[0.0, 0.0], [3.4512, 0.0]],
            -0.8,
            [0.25, 100.0],
            None,
        );
        drawing.add_dimension(
            dimensions,
            [[3.4512, 0.0], [3.4512, 2.0]],
            -0.8,
            [0.25, 100.0],
            Some("2000 typed"),
        );
        let leaders = drawing.layer(LAYER_LEADERS, LAYER_RGB_CONTRAST).unwrap();
        drawing.add_leader(
            leaders,
            [[1.0, 1.0], [2.0, 1.6]],
            [0.25, 100.0],
            "Brick wall",
        );
        let texts = drawing.layer(LAYER_TEXT, LAYER_RGB_CONTRAST).unwrap();
        drawing.add_text(texts, [0.5, 3.0], 0.25, "Office 0.01");
        let lines = drawing.layer(LAYER_LINES, LAYER_RGB_CONTRAST).unwrap();
        drawing.add_polyline(lines, vec![[0.0, 2.5], [3.0, 2.5]], false);
        drawing
    }

    #[test]
    fn dimensions_and_leaders_are_written_as_such_and_read_back() {
        use cadcodec::entities::{Dimension, EntityType};
        use cadcodec::{DwgReader, DxfReader};

        use crate::{read_drawing, write_drawing, DrawingEntity, DrawingFormat, DrawingVersion};

        let directory = tempfile::tempdir().unwrap();
        for format in DrawingFormat::ALL {
            let path = directory
                .path()
                .join(format!("plan.{}", format.extension()));
            write_drawing(&annotated(), &path, format, DrawingVersion::default()).unwrap();
            let document = match format {
                DrawingFormat::Dxf => DxfReader::from_file(&path).unwrap().read().unwrap(),
                DrawingFormat::Dwg => DwgReader::from_file(&path).unwrap().read().unwrap(),
            };
            let style = document
                .dim_styles
                .get("OPS-1-100")
                .expect("the style of 1:100");
            assert_eq!(style.dimscale, 100.0, "{format}");
            assert!((style.dimtxt - 2.5).abs() < 1e-9, "{format}");
            assert_eq!(style.dimrnd, 10.0, "{format}");
            let dimensions: Vec<_> = document
                .entities()
                .filter_map(|entity| match entity {
                    EntityType::Dimension(Dimension::Aligned(aligned)) => Some(aligned),
                    _ => None,
                })
                .collect();
            assert_eq!(dimensions.len(), 2, "{format}");
            let first = dimensions[0];
            assert_eq!(first.base.common.layer, LAYER_DIMENSIONS);
            assert_eq!(first.base.style_name, "OPS-1-100");
            // The definition points in millimetres: the two measured points,
            // and the line 0.8 m below them.
            assert!((first.first_point.x).abs() < 1e-6 && (first.first_point.y).abs() < 1e-6);
            assert!((first.second_point.x - 3451.2).abs() < 1e-6, "{format}");
            assert!((first.definition_point.x - 3451.2).abs() < 1e-6, "{format}");
            assert!((first.definition_point.y + 800.0).abs() < 1e-6, "{format}");
            assert_eq!(dimensions[1].base.text_override(), Some("2000 typed"));
            // Each has the block of its picture: its five lines and its
            // value.
            let block = &first.base.block_name;
            assert!(block.starts_with("*D"), "{format}: {block}");
            assert!(document.block_records.get(block).is_some(), "{format}");
            let parts: Vec<_> = document.entities_in_block(block).collect();
            assert_eq!(parts.len(), 6, "{format}");
            assert!(parts
                .iter()
                .any(|part| matches!(part, EntityType::Text(text) if text.value == "3450")));
            assert!(
                document
                    .entities()
                    .any(|entity| matches!(entity, EntityType::Leader(leader) if leader.vertices.len() == 3)),
                "{format}"
            );

            // The reader of the application draws the dimension with its
            // value, as a drawing program does.
            let read = read_drawing(&path).unwrap().drawing;
            let layer = read
                .layers
                .iter()
                .position(|layer| layer.name == LAYER_DIMENSIONS)
                .unwrap() as u16;
            let texts: Vec<&str> = read
                .entities
                .iter()
                .filter_map(|(on, entity)| match entity {
                    DrawingEntity::Text { value, .. } if *on == layer => Some(value.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(texts, ["3450", "2000 typed"], "{format}");
            let lines = read
                .entities
                .iter()
                .filter(|(on, entity)| {
                    *on == layer && matches!(entity, DrawingEntity::Polyline { .. })
                })
                .count();
            assert_eq!(lines, 10, "two extension lines, a line and two ticks each");
            assert!(read
                .entities
                .iter()
                .any(|(_, entity)| matches!(entity, DrawingEntity::Text { value, .. } if value == "Brick wall")));
        }
    }

    #[test]
    fn a_leader_points_its_arrow_at_its_tip_and_its_text_away_from_it() {
        let shape = leader_shape([0.0, 0.0], [2.0, 1.0], 0.3);
        assert_eq!(shape.line, vec![[0.0, 0.0], [2.0, 1.0], [2.3, 1.0]]);
        assert!(!shape.right);
        assert_eq!(shape.arrow[0], [0.0, 0.0]);
        let left = leader_shape([0.0, 0.0], [-2.0, 1.0], 0.3);
        assert!(left.right);
        assert!(near(left.line[2], [-2.3, 1.0]));
    }
}
