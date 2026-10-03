//! Native, clickable 3D view cube. Its world-aligned faces use the same
//! camera basis as the point renderer; the interaction follows OpenCADStudio's
//! face/edge/corner snap concept with a compact OpenAEC palette. The edges of
//! the cube are bevelled and its corners cut off, so each is a facet of its own.

use std::sync::OnceLock;

use iced::widget::canvas::{self, Frame, Path};
use iced::{alignment, Color, Font, Point, Rectangle, Size, Vector};

use crate::i18n::key;
use crate::CameraPreset;

const CUBE_SCALE: f32 = 25.0;
/// Half the width of a face between the bevels along its edges, in cube
/// half-edges.
const FACE_HALF: f32 = 0.85;
/// Half the length of a bevel between the corner facets at its ends, in cube
/// half-edges.
const BEVEL_HALF: f32 = 0.57;
/// A facet turned further away from the viewer than this is neither drawn nor
/// clickable.
const MIN_FACING: f32 = 0.03;
/// Radius of the compass ring around the foot of the cube, in cube half-edges.
const COMPASS_RADIUS: f32 = 1.72;

const TOP_TINT: [f32; 3] = [0.62, 0.45, 0.25];
const BOTTOM_TINT: [f32; 3] = [0.44, 0.40, 0.36];
const SIDE_TINT: [f32; 3] = [0.36, 0.38, 0.44];
const HOVER_TINT: [f32; 3] = [0.86, 0.50, 0.08];

/// What a click on the cube leads to. An edge or corner carries the direction
/// from the middle of the cube towards it, one step per axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CubeTarget {
    Face(CameraPreset),
    Edge([i8; 3]),
    Corner([i8; 3]),
    Home,
}

#[derive(Clone, Copy)]
struct Face {
    normal: [f32; 3],
    corners: [[f32; 3]; 4],
    label: &'static str,
    preset: CameraPreset,
}

const FACES: [Face; 6] = [
    Face {
        normal: [1.0, 0.0, 0.0],
        corners: [
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [1.0, 1.0, 1.0],
            [1.0, -1.0, 1.0],
        ],
        label: "RIGHT",
        preset: CameraPreset::Right,
    },
    Face {
        normal: [-1.0, 0.0, 0.0],
        corners: [
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ],
        label: "LEFT",
        preset: CameraPreset::Left,
    },
    Face {
        normal: [0.0, -1.0, 0.0],
        corners: [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, -1.0, 1.0],
            [-1.0, -1.0, 1.0],
        ],
        label: "FRONT",
        preset: CameraPreset::Front,
    },
    Face {
        normal: [0.0, 1.0, 0.0],
        corners: [
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
        label: "BACK",
        preset: CameraPreset::Back,
    },
    Face {
        normal: [0.0, 0.0, 1.0],
        corners: [
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ],
        label: "TOP",
        preset: CameraPreset::Top,
    },
    Face {
        normal: [0.0, 0.0, -1.0],
        corners: [
            [-1.0, 1.0, -1.0],
            [1.0, 1.0, -1.0],
            [1.0, -1.0, -1.0],
            [-1.0, -1.0, -1.0],
        ],
        label: "BOTTOM",
        preset: CameraPreset::Bottom,
    },
];

/// One flat piece of the cube surface: a face, the bevel along an edge or the
/// facet across a corner. Drawing and hit-testing both work on these.
struct Facet {
    target: CubeTarget,
    /// Unit normal, pointing out of the cube.
    normal: [f32; 3],
    tint: [f32; 3],
    /// Outline in cube half-edges, in order around the facet.
    vertices: Vec<[f32; 3]>,
    /// The face this facet is, with its label.
    face: Option<Face>,
}

impl Facet {
    fn outline(&self, basis: Basis, center: Point) -> Vec<Point> {
        self.vertices
            .iter()
            .map(|vertex| basis.project(*vertex, center))
            .collect()
    }
}

/// Colour of a facet: the mean of the colours of the faces it joins.
fn tint(direction: [i8; 3]) -> [f32; 3] {
    let joined: Vec<[f32; 3]> = direction
        .iter()
        .enumerate()
        .filter(|(_, step)| **step != 0)
        .map(|(axis, step)| {
            if axis < 2 {
                SIDE_TINT
            } else if *step > 0 {
                TOP_TINT
            } else {
                BOTTOM_TINT
            }
        })
        .collect();
    std::array::from_fn(|channel| {
        joined.iter().map(|color| color[channel]).sum::<f32>() / joined.len().max(1) as f32
    })
}

/// The 26 facets of the cube: six faces, a bevel along each of the twelve
/// edges and a facet across each of the eight corners. Every vertex is
/// (1, FACE_HALF, BEVEL_HALF) with its coordinates reordered and signed, so
/// neighbouring facets share their vertices and the surface is closed.
fn facets() -> &'static [Facet] {
    static FACETS: OnceLock<Vec<Facet>> = OnceLock::new();
    FACETS.get_or_init(build_facets)
}

fn build_facets() -> Vec<Facet> {
    let mut facets = Vec::with_capacity(26);
    for face in FACES {
        let [bottom_left, bottom_right, _, top_left] = face.corners;
        let vertices = [
            (FACE_HALF, -BEVEL_HALF),
            (FACE_HALF, BEVEL_HALF),
            (BEVEL_HALF, FACE_HALF),
            (-BEVEL_HALF, FACE_HALF),
            (-FACE_HALF, BEVEL_HALF),
            (-FACE_HALF, -BEVEL_HALF),
            (-BEVEL_HALF, -FACE_HALF),
            (BEVEL_HALF, -FACE_HALF),
        ]
        .map(|(along, up)| {
            std::array::from_fn(|axis| {
                face.normal[axis]
                    + along * (bottom_right[axis] - bottom_left[axis]) * 0.5
                    + up * (top_left[axis] - bottom_left[axis]) * 0.5
            })
        });
        facets.push(Facet {
            target: CubeTarget::Face(face.preset),
            normal: face.normal,
            tint: tint(face.normal.map(|component| component as i8)),
            vertices: vertices.to_vec(),
            face: Some(face),
        });
    }
    for x in [-1_i8, 0, 1] {
        for y in [-1_i8, 0, 1] {
            for z in [-1_i8, 0, 1] {
                let direction = [x, y, z];
                let sign = direction.map(f32::from);
                let steps = direction.iter().filter(|step| **step != 0).count();
                let free = direction.iter().position(|step| *step == 0);
                let (target, vertices): (CubeTarget, Vec<[f32; 3]>) = match (steps, free) {
                    (2, Some(free)) => {
                        let (first, second) = ((free + 1) % 3, (free + 2) % 3);
                        let vertex = |near: f32, far: f32, along: f32| {
                            let mut vertex = [0.0; 3];
                            vertex[first] = sign[first] * near;
                            vertex[second] = sign[second] * far;
                            vertex[free] = along;
                            vertex
                        };
                        (
                            CubeTarget::Edge(direction),
                            vec![
                                vertex(1.0, FACE_HALF, BEVEL_HALF),
                                vertex(FACE_HALF, 1.0, BEVEL_HALF),
                                vertex(FACE_HALF, 1.0, -BEVEL_HALF),
                                vertex(1.0, FACE_HALF, -BEVEL_HALF),
                            ],
                        )
                    }
                    (3, None) => (
                        CubeTarget::Corner(direction),
                        [
                            [1.0, FACE_HALF, BEVEL_HALF],
                            [1.0, BEVEL_HALF, FACE_HALF],
                            [FACE_HALF, BEVEL_HALF, 1.0],
                            [BEVEL_HALF, FACE_HALF, 1.0],
                            [BEVEL_HALF, 1.0, FACE_HALF],
                            [FACE_HALF, 1.0, BEVEL_HALF],
                        ]
                        .map(|vertex: [f32; 3]| {
                            std::array::from_fn(|axis| vertex[axis] * sign[axis])
                        })
                        .to_vec(),
                    ),
                    _ => continue,
                };
                let length = (steps as f32).sqrt();
                facets.push(Facet {
                    target,
                    normal: sign.map(|component| component / length),
                    tint: tint(direction),
                    vertices,
                    face: None,
                });
            }
        }
    }
    facets
}

/// The facets turned towards the viewer. The cube is convex, so these cover
/// its silhouette without overlapping.
fn visible_facets(basis: Basis) -> Vec<&'static Facet> {
    facets()
        .iter()
        .filter(|facet| dot(facet.normal, basis.toward) > MIN_FACING)
        .collect()
}

/// Yaw and pitch of the view that looks straight at the cube from the given
/// edge or corner direction.
pub fn view_from(direction: [i8; 3]) -> (f32, f32) {
    let [x, y, z] = direction.map(f32::from);
    (y.atan2(x), z.atan2(x.hypot(y)))
}

/// Name of the view from an edge: the two faces it joins. The names are
/// English and translated where they are shown.
pub fn edge_label(direction: [i8; 3]) -> &'static str {
    match direction {
        [1, 0, 1] => key("TOP RIGHT"),
        [-1, 0, 1] => key("TOP LEFT"),
        [0, -1, 1] => key("TOP FRONT"),
        [0, 1, 1] => key("TOP BACK"),
        [1, 0, -1] => key("BOTTOM RIGHT"),
        [-1, 0, -1] => key("BOTTOM LEFT"),
        [0, -1, -1] => key("BOTTOM FRONT"),
        [0, 1, -1] => key("BOTTOM BACK"),
        [1, -1, 0] => key("FRONT RIGHT"),
        [-1, -1, 0] => key("FRONT LEFT"),
        [1, 1, 0] => key("BACK RIGHT"),
        [-1, 1, 0] => key("BACK LEFT"),
        _ => key("CUSTOM"),
    }
}

#[derive(Clone, Copy)]
struct Basis {
    right: [f32; 3],
    up: [f32; 3],
    toward: [f32; 3],
}

impl Basis {
    fn new(yaw: f32, pitch: f32) -> Self {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        Self {
            right: [-sy, cy, 0.0],
            up: [-sp * cy, -sp * sy, cp],
            toward: [cp * cy, cp * sy, sp],
        }
    }

    fn project(self, world: [f32; 3], center: Point) -> Point {
        Point::new(
            center.x + dot(world, self.right) * CUBE_SCALE,
            center.y - dot(world, self.up) * CUBE_SCALE,
        )
    }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn center(bounds: Rectangle) -> Point {
    Point::new(bounds.width - 73.0, 69.0)
}

fn home_bounds(bounds: Rectangle) -> Rectangle {
    let point = center(bounds);
    Rectangle::new(Point::new(point.x - 22.0, 125.0), Size::new(44.0, 18.0))
}

/// The full square of a cube face, which carries the frame of its label.
fn face_points(face: Face, basis: Basis, center: Point) -> [Point; 4] {
    face.corners.map(|vertex| basis.project(vertex, center))
}

/// Rotation, stretch and rotation that together place flat text on a face
/// whose bottom edge and left edge project to the given screen vectors.
/// The frame can only rotate and stretch, so the skew of the face is
/// expressed as a rotation, a stretch along the axes and a second rotation.
fn face_text_transform(along: Vector, down: Vector) -> (f32, [f32; 2], f32) {
    let sum = (along.x + down.y) * 0.5;
    let difference = (along.x - down.y) * 0.5;
    let skew = (along.y + down.x) * 0.5;
    let turn = (along.y - down.x) * 0.5;
    let rotation = sum.hypot(turn);
    let stretch = difference.hypot(skew);
    let first = skew.atan2(difference);
    let second = turn.atan2(sum);
    (
        (second + first) * 0.5,
        [rotation + stretch, rotation - stretch],
        (second - first) * 0.5,
    )
}

fn polygon(points: &[Point]) -> Path {
    Path::new(|path| {
        for (index, point) in points.iter().enumerate() {
            if index == 0 {
                path.move_to(*point);
            } else {
                path.line_to(*point);
            }
        }
        path.close();
    })
}

fn contains(points: &[Point], position: Point) -> bool {
    let mut inside = false;
    for (index, a) in points.iter().enumerate() {
        let b = points[(index + 1) % points.len()];
        if (a.y > position.y) != (b.y > position.y)
            && position.x < (b.x - a.x) * (position.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
    }
    inside
}

pub fn hit(position: Point, bounds: Rectangle, yaw: f32, pitch: f32) -> Option<CubeTarget> {
    if home_bounds(bounds).contains(position) {
        return Some(CubeTarget::Home);
    }
    let basis = Basis::new(yaw, pitch);
    let center = center(bounds);
    visible_facets(basis)
        .iter()
        .find(|facet| contains(&facet.outline(basis, center), position))
        .map(|facet| facet.target)
}

fn scaled(color: [f32; 3], factor: f32) -> Color {
    Color::from_rgb(
        (color[0] * factor).min(1.0),
        (color[1] * factor).min(1.0),
        (color[2] * factor).min(1.0),
    )
}

/// A vertex in whole thousandths, so that the two facets along an edge name
/// it alike.
fn vertex_key(vertex: [f32; 3]) -> [i32; 3] {
    vertex.map(|component| (component * 1000.0).round() as i32)
}

/// Compass ring on the ground plane under the cube: north is +Y.
fn draw_compass(frame: &mut Frame, basis: Basis, center: Point) {
    let on_ground = |angle: f32, radius: f32| {
        let (sin, cos) = angle.sin_cos();
        basis.project([radius * sin, radius * cos, -1.0], center)
    };
    let ring = Path::new(|path| {
        path.move_to(on_ground(0.0, COMPASS_RADIUS));
        for step in 1..=72 {
            let angle = step as f32 / 72.0 * std::f32::consts::TAU;
            path.line_to(on_ground(angle, COMPASS_RADIUS));
        }
        path.close();
    });
    frame.fill(&ring, Color::from_rgba8(18, 18, 22, 0.35));
    frame.stroke(
        &ring,
        canvas::Stroke::default()
            .with_color(Color::from_rgba8(190, 192, 200, 0.55))
            .with_width(1.4),
    );
    for (index, label) in ["N", "E", "S", "W"].into_iter().enumerate() {
        let angle = index as f32 * std::f32::consts::FRAC_PI_2;
        let tick = Path::line(
            on_ground(angle, COMPASS_RADIUS - 0.12),
            on_ground(angle, COMPASS_RADIUS + 0.12),
        );
        let north = index == 0;
        let color = if north {
            Color::from_rgb8(245, 158, 11)
        } else {
            Color::from_rgba8(205, 207, 214, 0.85)
        };
        frame.stroke(
            &tick,
            canvas::Stroke::default().with_color(color).with_width(1.4),
        );
        frame.fill_text(canvas::Text {
            content: label.into(),
            position: on_ground(angle, COMPASS_RADIUS + 0.42),
            horizontal_alignment: alignment::Horizontal::Center,
            vertical_alignment: alignment::Vertical::Center,
            size: iced::Pixels(8.5),
            color,
            font: Font::with_name("Space Grotesk"),
            ..canvas::Text::default()
        });
    }
}

pub fn draw(frame: &mut Frame, bounds: Rectangle, yaw: f32, pitch: f32, hovered: Option<Point>) {
    let basis = Basis::new(yaw, pitch);
    let center = center(bounds);
    let hovered_target = hovered.and_then(|point| hit(point, bounds, yaw, pitch));
    frame.fill(
        &Path::rounded_rectangle(
            Point::new(center.x - 61.0, 8.0),
            Size::new(122.0, 142.0),
            10.0.into(),
        ),
        Color::from_rgba8(30, 30, 36, 0.58),
    );

    // Seen from above the ring lies behind the cube, from below in front of it.
    let from_above = basis.toward[2] >= 0.0;
    if from_above {
        draw_compass(frame, basis, center);
    }

    // Light from the upper left of the viewer, so the cube reads as a solid
    // from every side.
    let light: [f32; 3] = std::array::from_fn(|axis| {
        -0.38 * basis.right[axis] + 0.55 * basis.up[axis] + 0.74 * basis.toward[axis]
    });
    let visible = visible_facets(basis);
    for facet in &visible {
        let hover = hovered_target == Some(facet.target);
        let base = if hover { HOVER_TINT } else { facet.tint };
        let lit = 0.66 + 0.50 * dot(facet.normal, light).clamp(0.0, 1.0);
        let path = polygon(&facet.outline(basis, center));
        let Some(face) = facet.face else {
            // A bevel or corner is small, so its highlight is never dimmed.
            let lit = if hover { lit.max(1.0) } else { lit };
            frame.fill(&path, scaled(base, lit));
            continue;
        };
        // A little brighter along the upper edge than along the lower one.
        let points = face_points(face, basis, center);
        let upper = Point::new(
            (points[2].x + points[3].x) * 0.5,
            (points[2].y + points[3].y) * 0.5,
        );
        let lower = Point::new(
            (points[0].x + points[1].x) * 0.5,
            (points[0].y + points[1].y) * 0.5,
        );
        let shading = canvas::gradient::Linear::new(upper, lower)
            .add_stop(0.0, scaled(base, lit * 1.10))
            .add_stop(1.0, scaled(base, lit * 0.86));
        frame.fill(
            &path,
            canvas::Fill {
                style: canvas::Style::Gradient(canvas::Gradient::Linear(shading)),
                ..canvas::Fill::default()
            },
        );
    }

    // Each edge between two facets is one line, however many of the two are
    // visible, so the outlines join without doubling up.
    let mut drawn: Vec<[[i32; 3]; 2]> = Vec::new();
    let outlines = Path::new(|path| {
        for facet in &visible {
            for (index, vertex) in facet.vertices.iter().enumerate() {
                let next = facet.vertices[(index + 1) % facet.vertices.len()];
                let mut edge = [vertex_key(*vertex), vertex_key(next)];
                edge.sort_unstable();
                if drawn.contains(&edge) {
                    continue;
                }
                drawn.push(edge);
                path.move_to(basis.project(*vertex, center));
                path.line_to(basis.project(next, center));
            }
        }
    });
    frame.stroke(
        &outlines,
        canvas::Stroke::default()
            .with_color(Color::from_rgb8(204, 205, 212))
            .with_width(1.0)
            .with_line_cap(canvas::LineCap::Round),
    );

    for face in visible.iter().filter_map(|facet| facet.face) {
        // Each face lists its corners from bottom left, anticlockwise as seen
        // from outside: the first edge runs along the text, the last one up.
        let points = face_points(face, basis, center);
        let along = (points[1] - points[0]) * (0.5 / CUBE_SCALE);
        let down = (points[0] - points[3]) * (0.5 / CUBE_SCALE);
        let (outer, stretch, inner) = face_text_transform(along, down);
        if stretch[1] < 0.12 {
            // Seen almost edge-on the label would collapse into a line.
            continue;
        }
        let label_position = basis.project(face.normal, center);
        frame.with_save(|frame| {
            frame.translate(Vector::new(label_position.x, label_position.y));
            frame.rotate(outer);
            frame.scale_nonuniform(Vector::new(stretch[0], stretch[1]));
            frame.rotate(inner);
            frame.fill_text(canvas::Text {
                content: face.label.into(),
                position: Point::ORIGIN,
                horizontal_alignment: alignment::Horizontal::Center,
                vertical_alignment: alignment::Vertical::Center,
                size: iced::Pixels(9.0),
                color: Color::from_rgb8(248, 248, 246),
                font: Font::with_name("Space Grotesk"),
                ..canvas::Text::default()
            });
        });
    }
    if !from_above {
        draw_compass(frame, basis, center);
    }
    let home = home_bounds(bounds);
    let home_hovered = matches!(hovered_target, Some(CubeTarget::Home));
    let pill = Path::rounded_rectangle(home.position(), home.size(), (home.height * 0.5).into());
    frame.fill(
        &pill,
        if home_hovered {
            Color::from_rgb8(217, 119, 6)
        } else {
            Color::from_rgba8(70, 72, 82, 0.92)
        },
    );
    frame.stroke(
        &pill,
        canvas::Stroke::default()
            .with_color(Color::from_rgba8(236, 236, 240, 0.55))
            .with_width(1.0),
    );
    frame.fill_text(canvas::Text {
        content: "ISO".into(),
        position: Point::new(home.center_x(), home.center_y()),
        horizontal_alignment: alignment::Horizontal::Center,
        vertical_alignment: alignment::Vertical::Center,
        size: iced::Pixels(9.5),
        color: Color::from_rgb8(245, 245, 244),
        font: Font::with_name("Space Grotesk"),
        ..canvas::Text::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_bounds() -> Rectangle {
        Rectangle::new(Point::ORIGIN, Size::new(900.0, 700.0))
    }

    /// Orientations all around the cube, including straight down, straight up
    /// and the views square to a face.
    fn orientations() -> Vec<(f32, f32)> {
        let mut orientations = Vec::new();
        for yaw_step in -18..18 {
            for pitch_step in -9..=9 {
                orientations.push((
                    (yaw_step as f32 * 10.0 + 3.0).to_radians(),
                    (pitch_step as f32 * 10.0).to_radians(),
                ));
                orientations.push((
                    (yaw_step as f32 * 10.0).to_radians(),
                    (pitch_step as f32 * 9.7 + 1.3).to_radians(),
                ));
            }
        }
        for yaw_step in -4..4 {
            for pitch_step in -2..=2 {
                orientations.push((
                    (yaw_step as f32 * 45.0).to_radians(),
                    (pitch_step as f32 * 45.0).to_radians(),
                ));
            }
        }
        orientations
    }

    fn directions(steps: usize) -> Vec<[i8; 3]> {
        let mut directions = Vec::new();
        for x in [-1_i8, 0, 1] {
            for y in [-1_i8, 0, 1] {
                for z in [-1_i8, 0, 1] {
                    if [x, y, z].iter().filter(|step| **step != 0).count() == steps {
                        directions.push([x, y, z]);
                    }
                }
            }
        }
        directions
    }

    #[test]
    fn snaps_each_face_and_a_visible_corner() {
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(900.0, 700.0));
        let middle = center(bounds);
        for preset in [
            CameraPreset::Top,
            CameraPreset::Bottom,
            CameraPreset::Front,
            CameraPreset::Back,
            CameraPreset::Right,
            CameraPreset::Left,
        ] {
            let (yaw, pitch, _) = preset.orientation();
            assert!(matches!(
                hit(middle, bounds, yaw, pitch),
                Some(CubeTarget::Face(face)) if face == preset
            ));
        }
        let (yaw, pitch, _) = CameraPreset::Isometric.orientation();
        // The label of every visible face follows its two projected edges.
        let basis = Basis::new(yaw, pitch);
        for face in FACES
            .iter()
            .filter(|face| dot(face.normal, basis.toward) > 0.03)
        {
            let points = face_points(*face, basis, middle);
            let along = (points[1] - points[0]) * (0.5 / CUBE_SCALE);
            let down = (points[0] - points[3]) * (0.5 / CUBE_SCALE);
            let (outer, stretch, inner) = face_text_transform(along, down);
            let apply = |x: f32, y: f32| {
                let (sin, cos) = inner.sin_cos();
                let (x, y) = (
                    (x * cos - y * sin) * stretch[0],
                    (x * sin + y * cos) * stretch[1],
                );
                let (sin, cos) = outer.sin_cos();
                (x * cos - y * sin, x * sin + y * cos)
            };
            let right = apply(1.0, 0.0);
            let below = apply(0.0, 1.0);
            assert!((right.0 - along.x).abs() < 1e-4 && (right.1 - along.y).abs() < 1e-4);
            assert!((below.0 - down.x).abs() < 1e-4 && (below.1 - down.y).abs() < 1e-4);
            assert!(stretch[1] > 0.0, "{} is mirrored", face.label);
        }
        let corner = Basis::new(yaw, pitch).project([1.0, -1.0, 1.0], middle);
        assert!(matches!(
            hit(corner, bounds, yaw, pitch),
            Some(CubeTarget::Corner([1, -1, 1]))
        ));
    }

    #[test]
    fn the_facets_close_the_cube_without_gaps() {
        let facets = facets();
        assert_eq!(facets.len(), 26);
        let count = |wanted: fn(&CubeTarget) -> bool| {
            facets.iter().filter(|facet| wanted(&facet.target)).count()
        };
        assert_eq!(count(|target| matches!(target, CubeTarget::Face(_))), 6);
        assert_eq!(count(|target| matches!(target, CubeTarget::Edge(_))), 12);
        assert_eq!(count(|target| matches!(target, CubeTarget::Corner(_))), 8);

        let mut edges: Vec<[[i32; 3]; 2]> = Vec::new();
        for facet in facets {
            assert!((dot(facet.normal, facet.normal) - 1.0).abs() < 1e-5);
            // Flat, and no vertex of the cube lies outside its plane.
            let height = dot(facet.vertices[0], facet.normal);
            for vertex in &facet.vertices {
                assert!((dot(*vertex, facet.normal) - height).abs() < 1e-5);
            }
            for other in facets {
                for vertex in &other.vertices {
                    assert!(dot(*vertex, facet.normal) < height + 1e-5);
                }
            }
            for (index, vertex) in facet.vertices.iter().enumerate() {
                let next = facet.vertices[(index + 1) % facet.vertices.len()];
                let mut edge = [vertex_key(*vertex), vertex_key(next)];
                assert_ne!(edge[0], edge[1]);
                edge.sort_unstable();
                edges.push(edge);
            }
        }
        // Every edge lies between exactly two facets.
        for edge in &edges {
            assert_eq!(edges.iter().filter(|other| *other == edge).count(), 2);
        }
        assert_eq!(edges.len(), 2 * 72);
    }

    #[test]
    fn every_point_of_a_drawn_facet_hits_that_facet_only() {
        let bounds = test_bounds();
        let middle = center(bounds);
        for (yaw, pitch) in orientations() {
            let basis = Basis::new(yaw, pitch);
            let visible = visible_facets(basis);
            assert!(!visible.is_empty());
            let outlines: Vec<_> = visible
                .iter()
                .map(|facet| facet.outline(basis, middle))
                .collect();
            for (index, facet) in visible.iter().enumerate() {
                let outline = &outlines[index];
                let corners = outline.len() as f32;
                let centroid = Point::new(
                    outline.iter().map(|point| point.x).sum::<f32>() / corners,
                    outline.iter().map(|point| point.y).sum::<f32>() / corners,
                );
                // The middle, and points close to every corner and every side.
                let mut samples = vec![centroid];
                for (index, point) in outline.iter().enumerate() {
                    let next = outline[(index + 1) % outline.len()];
                    let side = Point::new((point.x + next.x) * 0.5, (point.y + next.y) * 0.5);
                    for towards in [*point, side] {
                        for share in [0.5, 0.9] {
                            samples.push(Point::new(
                                centroid.x + (towards.x - centroid.x) * share,
                                centroid.y + (towards.y - centroid.y) * share,
                            ));
                        }
                    }
                }
                for sample in samples {
                    assert_eq!(
                        hit(sample, bounds, yaw, pitch),
                        Some(facet.target),
                        "yaw {yaw} pitch {pitch} at {sample:?}"
                    );
                    let holders = outlines
                        .iter()
                        .filter(|other| contains(other, sample))
                        .count();
                    assert_eq!(holders, 1, "yaw {yaw} pitch {pitch} at {sample:?}");
                }
            }
        }
    }

    #[test]
    fn face_centres_hit_their_face_and_hidden_facets_hit_nothing() {
        let bounds = test_bounds();
        let middle = center(bounds);
        for (yaw, pitch) in orientations() {
            let basis = Basis::new(yaw, pitch);
            for face in FACES {
                let target = hit(basis.project(face.normal, middle), bounds, yaw, pitch);
                if dot(face.normal, basis.toward) > MIN_FACING {
                    assert_eq!(target, Some(CubeTarget::Face(face.preset)));
                } else {
                    assert_ne!(target, Some(CubeTarget::Face(face.preset)));
                }
            }
            for facet in facets() {
                if dot(facet.normal, basis.toward) > MIN_FACING {
                    continue;
                }
                // The far side of the cube lies behind what is drawn.
                for vertex in &facet.vertices {
                    let towards_middle = vertex.map(|component| component * 0.98);
                    let sample = basis.project(towards_middle, middle);
                    assert_ne!(hit(sample, bounds, yaw, pitch), Some(facet.target));
                }
            }
        }
    }

    #[test]
    fn corners_lead_to_the_eight_isometric_views() {
        use std::f32::consts::{FRAC_PI_4, PI};
        let bounds = test_bounds();
        let elevation = (1.0_f32 / 3.0_f32.sqrt()).asin();
        let expected = [
            ([1, 1, 1], FRAC_PI_4, elevation),
            ([-1, 1, 1], PI - FRAC_PI_4, elevation),
            ([-1, -1, 1], FRAC_PI_4 - PI, elevation),
            ([1, -1, 1], -FRAC_PI_4, elevation),
            ([1, 1, -1], FRAC_PI_4, -elevation),
            ([-1, 1, -1], PI - FRAC_PI_4, -elevation),
            ([-1, -1, -1], FRAC_PI_4 - PI, -elevation),
            ([1, -1, -1], -FRAC_PI_4, -elevation),
        ];
        assert_eq!(directions(3).len(), expected.len());
        for (corner, expected_yaw, expected_pitch) in expected {
            let (yaw, pitch) = view_from(corner);
            assert!((yaw - expected_yaw).abs() < 1e-5, "{corner:?}");
            assert!((pitch - expected_pitch).abs() < 1e-5, "{corner:?}");
            // The viewer looks along the diagonal through that corner, and the
            // corner facet lies in the middle of the cube, square to the view.
            let toward = Basis::new(yaw, pitch).toward;
            for (seen, step) in toward.iter().zip(corner) {
                assert!((seen * 3.0_f32.sqrt() - f32::from(step)).abs() < 1e-5);
            }
            assert_eq!(
                hit(center(bounds), bounds, yaw, pitch),
                Some(CubeTarget::Corner(corner))
            );
            // Three faces meet there, each seen equally.
            let seen: Vec<_> = FACES
                .iter()
                .filter(|face| dot(face.normal, toward) > MIN_FACING)
                .collect();
            assert_eq!(seen.len(), 3);
        }
    }

    #[test]
    fn edges_lead_to_the_view_between_their_two_faces() {
        let bounds = test_bounds();
        let edges = directions(2);
        assert_eq!(edges.len(), 12);
        for edge in edges {
            let (yaw, pitch) = view_from(edge);
            let toward = Basis::new(yaw, pitch).toward;
            for (seen, step) in toward.iter().zip(edge) {
                assert!((seen * 2.0_f32.sqrt() - f32::from(step)).abs() < 1e-5);
            }
            assert_eq!(
                hit(center(bounds), bounds, yaw, pitch),
                Some(CubeTarget::Edge(edge))
            );
            // Only the two faces along the edge are seen, both at 45 degrees,
            // and the view is named after them.
            let seen: Vec<_> = FACES
                .iter()
                .filter(|face| dot(face.normal, toward) > MIN_FACING)
                .collect();
            assert_eq!(seen.len(), 2, "{edge:?}");
            let label = edge_label(edge);
            for face in seen {
                assert!((dot(face.normal, toward) - 0.5_f32.sqrt()).abs() < 1e-5);
                assert!(label.split(' ').any(|word| word == face.label), "{label}");
            }
            assert_eq!(label.split(' ').count(), 2);
        }
        assert_eq!(edge_label([1, 1, 1]), "CUSTOM");
    }
}
