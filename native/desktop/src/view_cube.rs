//! Native, clickable 3D view cube. Its world-aligned faces use the same
//! camera basis as the point renderer; the interaction follows OpenCADStudio's
//! face/corner snap concept with a compact OpenAEC palette.

use iced::widget::canvas::{self, Frame, Path};
use iced::{alignment, Color, Font, Point, Rectangle, Size, Vector};

use crate::CameraPreset;

const CUBE_SCALE: f32 = 25.0;
const CORNER_RADIUS: f32 = 8.0;
/// Radius of the compass ring around the foot of the cube, in cube half-edges.
const COMPASS_RADIUS: f32 = 1.72;

#[derive(Debug, Clone, Copy)]
pub enum CubeTarget {
    Face(CameraPreset),
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

fn polygon(points: [Point; 4]) -> Path {
    Path::new(|path| {
        path.move_to(points[0]);
        for point in points.iter().skip(1) {
            path.line_to(*point);
        }
        path.close();
    })
}

fn contains(points: [Point; 4], position: Point) -> bool {
    let mut inside = false;
    for index in 0..4 {
        let a = points[index];
        let b = points[(index + 1) % 4];
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
    let mut closest = None;
    for x in [-1, 1] {
        for y in [-1, 1] {
            for z in [-1, 1] {
                let vertex = [x as f32, y as f32, z as f32];
                if dot(vertex, basis.toward) <= 0.0 {
                    continue;
                }
                let point = basis.project(vertex, center);
                let distance = (point.x - position.x).hypot(point.y - position.y);
                if distance <= CORNER_RADIUS
                    && closest.is_none_or(|(_, best_distance)| distance < best_distance)
                {
                    closest = Some(([x, y, z], distance));
                }
            }
        }
    }
    if let Some((corner, _)) = closest {
        return Some(CubeTarget::Corner(corner));
    }
    FACES
        .iter()
        .filter(|face| dot(face.normal, basis.toward) > 0.03)
        .filter(|face| contains(face_points(**face, basis, center), position))
        .max_by(|a, b| dot(a.normal, basis.toward).total_cmp(&dot(b.normal, basis.toward)))
        .map(|face| CubeTarget::Face(face.preset))
}

fn scaled(color: [f32; 3], factor: f32) -> Color {
    Color::from_rgb(
        (color[0] * factor).min(1.0),
        (color[1] * factor).min(1.0),
        (color[2] * factor).min(1.0),
    )
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
    let mut visible: Vec<_> = FACES
        .iter()
        .copied()
        .filter(|face| dot(face.normal, basis.toward) > 0.03)
        .collect();
    visible.sort_by(|a, b| dot(a.normal, basis.toward).total_cmp(&dot(b.normal, basis.toward)));
    for face in visible {
        let points = face_points(face, basis, center);
        let hover =
            matches!(hovered_target, Some(CubeTarget::Face(preset)) if preset == face.preset);
        let base = if hover {
            [0.86, 0.50, 0.08]
        } else if face.normal[2] > 0.0 {
            [0.62, 0.45, 0.25]
        } else if face.normal[2] < 0.0 {
            [0.44, 0.40, 0.36]
        } else {
            [0.36, 0.38, 0.44]
        };
        let lit = 0.66 + 0.50 * dot(face.normal, light).clamp(0.0, 1.0);
        let path = polygon(points);
        // A little brighter along the upper edge than along the lower one.
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
        frame.stroke(
            &path,
            canvas::Stroke::default()
                .with_color(Color::from_rgba8(236, 236, 240, 0.72))
                .with_width(1.1)
                .with_line_join(canvas::LineJoin::Round),
        );
        // Each face lists its corners from bottom left, anticlockwise as seen
        // from outside: the first edge runs along the text, the last one up.
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
    if let Some(CubeTarget::Corner(corner)) = hovered_target {
        let point = basis.project(corner.map(f32::from), center);
        frame.fill(&Path::circle(point, 4.5), Color::from_rgb8(245, 158, 11));
        frame.stroke(
            &Path::circle(point, 4.5),
            canvas::Stroke::default()
                .with_color(Color::from_rgb8(42, 42, 50))
                .with_width(1.0),
        );
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
}
