//! The orbit point: a point of the scene the orbit camera turns about once
//! it has been picked with a double click, instead of the centre of the
//! scene.

use std::time::{Duration, Instant};

use iced::widget::canvas::{self, Frame};
use iced::{Color, Point as UiPoint, Size};
use pointcloud_core::Bounds;

use crate::selection::Projection;

/// Two clicks count as a double click within this time and distance.
pub const DOUBLE_CLICK_TIME: Duration = Duration::from_millis(450);
pub const DOUBLE_CLICK_REACH: f32 = 5.0;
/// How far from the pointer a drawn point may lie to become the orbit point,
/// in pixels: the reach of picking a point.
pub const PICK_RADIUS: f32 = 8.0;

/// The orbit camera as the window keeps it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub pan: [f32; 2],
}

impl OrbitCamera {
    pub fn projection(self, scene: Bounds, size: Size) -> Projection {
        Projection::new(
            scene,
            self.yaw,
            self.pitch,
            self.zoom,
            self.pan,
            size.width,
            size.height,
        )
    }
}

/// The camera turned to `yaw` and `pitch` about `point`: the zoom and pan
/// that keep the point on its pixel, with the picture around it at the same
/// scale. `None` when the point is not in view, or the camera cannot keep it
/// there; the camera then turns about the centre of the scene.
pub fn turn_about(
    scene: Bounds,
    from: OrbitCamera,
    yaw: f32,
    pitch: f32,
    point: [f64; 3],
    size: Size,
) -> Option<OrbitCamera> {
    let (x, y, depth) = from.projection(scene, size).project(point)?;
    let turned = OrbitCamera {
        yaw,
        pitch,
        pan: [0.0; 2],
        ..from
    };
    // The orbit camera stands at a fixed distance from the centre of the
    // scene, so a point off the centre comes nearer or goes further while
    // the camera turns. The zoom makes up for that.
    let turned_depth = turned.projection(scene, size).depth(point);
    if turned_depth <= 0.01 {
        return None;
    }
    let zoom = (f64::from(from.zoom) * depth / turned_depth).clamp(0.000_001, 10_000.0) as f32;
    let turned = OrbitCamera { zoom, ..turned };
    let (turned_x, turned_y, _) = turned.projection(scene, size).project_unclipped(point)?;
    let pan = [x - turned_x, y - turned_y];
    pan.iter()
        .all(|value| value.is_finite())
        .then_some(OrbitCamera { pan, ..turned })
}

/// Whether a click at `position` at `now` makes a double click with the
/// click before it.
pub fn is_double_click(
    previous: Option<(Instant, UiPoint)>,
    now: Instant,
    position: UiPoint,
) -> bool {
    previous.is_some_and(|(at, place)| {
        now.saturating_duration_since(at) <= DOUBLE_CLICK_TIME
            && (position.x - place.x).hypot(position.y - place.y) <= DOUBLE_CLICK_REACH
    })
}

/// A small target over the orbit point while the camera turns about it.
pub fn draw_marker(frame: &mut Frame, at: UiPoint) {
    let accent = Color::from_rgb8(245, 158, 11);
    let shadow = Color::from_rgba8(0, 0, 0, 0.55);
    let ring = canvas::Path::circle(at, 7.0);
    frame.stroke(
        &ring,
        canvas::Stroke::default().with_color(shadow).with_width(3.5),
    );
    frame.stroke(
        &ring,
        canvas::Stroke::default().with_color(accent).with_width(1.6),
    );
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        let tick = canvas::Path::line(
            UiPoint::new(at.x + dx * 10.0, at.y + dy * 10.0),
            UiPoint::new(at.x + dx * 14.0, at.y + dy * 14.0),
        );
        frame.stroke(
            &tick,
            canvas::Stroke::default().with_color(accent).with_width(1.6),
        );
    }
    frame.fill(&canvas::Path::circle(at, 2.0), accent);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> Bounds {
        Bounds {
            min: [1_000.0, 2_000.0, 0.0],
            max: [1_040.0, 2_020.0, 6.0],
        }
    }

    const SIZE: Size = Size {
        width: 800.0,
        height: 600.0,
    };

    #[test]
    fn turning_about_a_point_keeps_it_on_its_pixel_at_its_scale() {
        // A point near a corner of the scene, far from its centre, panned to
        // a little left of and above the middle of the view.
        let point = [1_036.0, 2_017.0, 1.0];
        let mut from = OrbitCamera {
            yaw: -0.8,
            pitch: 0.6,
            zoom: 0.05,
            pan: [0.0; 2],
        };
        let (x, y, _) = from
            .projection(scene(), SIZE)
            .project_unclipped(point)
            .unwrap();
        from.pan = [350.0 - x, 260.0 - y];
        let before = from.projection(scene(), SIZE);
        let (x, y, depth) = before.project(point).expect("the point is in view");
        // The length on screen of a vertical step of 10 cm at the point.
        let step = |projection: Projection| {
            let (ax, ay, _) = projection.project_unclipped(point).unwrap();
            let up = [point[0], point[1], point[2] + 0.1];
            let (bx, by, _) = projection.project_unclipped(up).unwrap();
            (ax - bx).hypot(ay - by)
        };
        for (yaw, pitch) in [(-0.6, 0.6), (0.4, 0.2), (2.5, -0.3), (-3.0, 1.4)] {
            let turned = turn_about(scene(), from, yaw, pitch, point, SIZE).unwrap();
            assert_eq!((turned.yaw, turned.pitch), (yaw, pitch));
            let after = turned.projection(scene(), SIZE);
            let (turned_x, turned_y, turned_depth) = after.project(point).unwrap();
            assert!((turned_x - x).abs() < 0.01 && (turned_y - y).abs() < 0.01);
            // The scale at the point stays: the zoom follows its depth.
            assert!(
                ((turned.zoom / from.zoom) as f64 - depth / turned_depth).abs() < 1e-4,
                "{yaw} {pitch}"
            );
            // Turned about the vertical, the step keeps its length within
            // what the shifted picture changes in perspective.
            if pitch == from.pitch {
                let ratio = step(after) / step(before);
                assert!((0.95..1.05).contains(&ratio), "{ratio}");
            }
        }
    }

    #[test]
    fn a_point_out_of_view_leaves_the_turn_to_the_scene_centre() {
        let from = OrbitCamera {
            yaw: -0.8,
            pitch: 0.6,
            zoom: 0.05,
            pan: [0.0, 0.0],
        };
        assert!(from
            .projection(scene(), SIZE)
            .project(scene().min)
            .is_none());
        assert_eq!(turn_about(scene(), from, 0.0, 0.6, scene().min, SIZE), None);
    }

    #[test]
    fn double_click_needs_two_clicks_close_in_time_and_place() {
        let first = Instant::now();
        let at = UiPoint::new(100.0, 100.0);
        assert!(!is_double_click(None, first, at));
        let later = first + Duration::from_millis(200);
        assert!(is_double_click(
            Some((first, at)),
            later,
            UiPoint::new(103.0, 102.0)
        ));
        assert!(!is_double_click(
            Some((first, at)),
            later,
            UiPoint::new(110.0, 100.0)
        ));
        let slow = first + Duration::from_millis(900);
        assert!(!is_double_click(Some((first, at)), slow, at));
    }
}
