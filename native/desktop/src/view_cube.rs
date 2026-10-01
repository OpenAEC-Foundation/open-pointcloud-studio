//! Native, clickable 3D view cube. Its world-aligned faces use the same
//! camera basis as the point renderer; the interaction follows OpenCADStudio's
//! face/corner snap concept with a compact OpenAEC palette.

use iced::widget::canvas::{self, Frame, Path};
use iced::{alignment, Color, Point, Rectangle, Size};

use crate::CameraPreset;

const CUBE_SCALE: f32 = 30.0;
const CORNER_RADIUS: f32 = 8.0;

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
    Rectangle::new(Point::new(point.x - 25.0, 121.0), Size::new(50.0, 21.0))
}

fn face_points(face: Face, basis: Basis, center: Point) -> [Point; 4] {
    face.corners.map(|vertex| basis.project(vertex, center))
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

pub fn draw(frame: &mut Frame, bounds: Rectangle, yaw: f32, pitch: f32, hovered: Option<Point>) {
    let basis = Basis::new(yaw, pitch);
    let center = center(bounds);
    let hovered_target = hovered.and_then(|point| hit(point, bounds, yaw, pitch));
    frame.fill_rectangle(
        Point::new(center.x - 61.0, 8.0),
        Size::new(122.0, 142.0),
        Color::from_rgba8(42, 42, 50, 0.88),
    );
    frame.stroke_rectangle(
        Point::new(center.x - 61.0, 8.0),
        Size::new(122.0, 142.0),
        canvas::Stroke::default()
            .with_color(Color::from_rgb8(72, 72, 80))
            .with_width(1.0),
    );

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
        let color = if hover {
            Color::from_rgb8(164, 99, 24)
        } else if face.normal[2] > 0.0 {
            Color::from_rgb8(92, 75, 56)
        } else {
            Color::from_rgb8(65, 65, 73)
        };
        let path = polygon(points);
        frame.fill(&path, color);
        frame.stroke(
            &path,
            canvas::Stroke::default()
                .with_color(Color::from_rgb8(177, 177, 183))
                .with_width(1.0),
        );
        let label_position = basis.project(face.normal, center);
        frame.fill_text(canvas::Text {
            content: face.label.into(),
            position: Point::new(label_position.x, label_position.y + 3.0),
            horizontal_alignment: alignment::Horizontal::Center,
            size: iced::Pixels(10.0),
            color: Color::from_rgb8(241, 241, 240),
            ..canvas::Text::default()
        });
    }
    if let Some(CubeTarget::Corner(corner)) = hovered_target {
        let point = basis.project(corner.map(f32::from), center);
        frame.fill(&Path::circle(point, 4.0), Color::from_rgb8(217, 119, 6));
    }
    let home = home_bounds(bounds);
    let home_hovered = matches!(hovered_target, Some(CubeTarget::Home));
    frame.fill_rectangle(
        home.position(),
        home.size(),
        if home_hovered {
            Color::from_rgb8(145, 86, 19)
        } else {
            Color::from_rgb8(54, 54, 62)
        },
    );
    frame.stroke_rectangle(
        home.position(),
        home.size(),
        canvas::Stroke::default()
            .with_color(Color::from_rgb8(161, 161, 170))
            .with_width(1.0),
    );
    frame.fill_text(canvas::Text {
        content: "ISO".into(),
        position: Point::new(center.x, home.y + 14.0),
        horizontal_alignment: alignment::Horizontal::Center,
        size: iced::Pixels(10.0),
        color: Color::from_rgb8(235, 235, 236),
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
        let corner = Basis::new(yaw, pitch).project([1.0, -1.0, 1.0], middle);
        assert!(matches!(
            hit(corner, bounds, yaw, pitch),
            Some(CubeTarget::Corner([1, -1, 1]))
        ));
    }
}
