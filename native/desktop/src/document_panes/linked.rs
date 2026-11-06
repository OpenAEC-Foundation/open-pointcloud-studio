//! A linked photo's measured camera pose, without changing the scene camera.
use crate::{PointViewport, Projection};
use iced::widget::canvas::{self, Frame};
use iced::Point;
use pointcloud_core::Bounds;

impl PointViewport<'_> {
    pub(crate) fn draw_linked_photos(
        &self,
        frame: &mut Frame,
        projection: Projection,
        bounds: Bounds,
    ) {
        if self.clean {
            return;
        }
        let reach = (0..3)
            .map(|axis| bounds.max[axis] - bounds.min[axis])
            .fold(0., f64::max)
            .max(0.1)
            * 0.12;
        let project = |point| {
            projection
                .project_unclipped(point)
                .map(|(x, y, _)| Point::new(x, y))
        };
        for camera in self.linked_photos.iter().flatten() {
            let Some(eye) = project(camera.eye) else {
                continue;
            };
            let [right, _, forward] = camera.basis();
            let width = reach * f64::from(camera.field_of_view * 0.5).tan().clamp(0.1, 4.);
            let tip: [f64; 3] =
                std::array::from_fn(|axis| camera.eye[axis] + reach * forward[axis]);
            let left = project(std::array::from_fn(|axis| tip[axis] - width * right[axis]));
            let right = project(std::array::from_fn(|axis| tip[axis] + width * right[axis]));
            let stroke = canvas::Stroke::default()
                .with_color(self.scene.accent)
                .with_width(2.);
            frame.stroke(&canvas::Path::circle(eye, 7.), stroke);
            if let (Some(left), Some(right)) = (left, right) {
                let fan = canvas::Path::new(|path| {
                    path.move_to(eye);
                    path.line_to(left);
                    path.line_to(right);
                    path.close();
                });
                frame.stroke(&fan, stroke);
            }
        }
    }
}
