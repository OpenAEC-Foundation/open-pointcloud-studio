//! Photos stored with scanner stations and the camera geometry to place them.
//!
//! A station panorama is usually stored as several pinhole photos that share
//! the scanner position, for example six 90-degree cube faces. Nothing here
//! assumes a face order or count: every photo carries its own pose.

/// Encoding of a stored station photo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanImageFormat {
    Jpeg,
    Png,
}

/// One pinhole photo taken at a scanner station.
///
/// The camera frame follows the E57 pinhole convention: +X is image right,
/// +Y is image up and the camera looks along -Z. Pixel coordinates count
/// columns to the right and rows downward from the top-left pixel, with
/// integer values at pixel centres.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanImage {
    /// Index into `PointCloud::scan_poses` of the station that took the photo.
    pub station: Option<usize>,
    pub position: [f64; 3],
    /// Registered unit directions of the camera's right, up and backward axes.
    pub axes: [[f64; 3]; 3],
    pub width: u32,
    pub height: u32,
    /// Focal length in pixels along the image columns and rows.
    pub focal: [f64; 2],
    /// Pixel where the viewing axis meets the image.
    pub principal: [f64; 2],
    pub format: ScanImageFormat,
    /// Location of the encoded photo inside the source file.
    pub offset: u64,
    pub length: u64,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl ScanImage {
    /// Registered direction the camera looks along.
    pub fn view_direction(&self) -> [f64; 3] {
        self.axes[2].map(|value| -value)
    }

    /// Pixel that sees a registered direction from the station, when the
    /// direction is in front of the camera and inside the photo.
    pub fn project(&self, direction: [f64; 3]) -> Option<[f64; 2]> {
        let depth = -dot(direction, self.axes[2]);
        if depth.is_nan() || depth <= 0.0 {
            return None;
        }
        let column = self.principal[0] + self.focal[0] * dot(direction, self.axes[0]) / depth;
        let row = self.principal[1] - self.focal[1] * dot(direction, self.axes[1]) / depth;
        (column >= -0.5
            && row >= -0.5
            && column <= f64::from(self.width) - 0.5
            && row <= f64::from(self.height) - 0.5)
            .then_some([column, row])
    }

    /// Registered direction seen by a pixel; not normalised.
    pub fn ray(&self, column: f64, row: f64) -> [f64; 3] {
        let right = (column - self.principal[0]) / self.focal[0];
        let up = -(row - self.principal[1]) / self.focal[1];
        std::array::from_fn(|axis| {
            self.axes[0][axis] * right + self.axes[1][axis] * up - self.axes[2][axis]
        })
    }

    /// Distance, in image fractions, from a pixel to the nearest photo edge.
    fn border_margin(&self, pixel: [f64; 2]) -> f64 {
        let column = (pixel[0] + 0.5) / f64::from(self.width);
        let row = (pixel[1] + 0.5) / f64::from(self.height);
        column.min(1.0 - column).min(row).min(1.0 - row)
    }
}

/// Choose the photo that sees a direction furthest from its own border, so
/// neighbouring photos switch on their shared bisector.
pub fn select_scan_image(images: &[ScanImage], direction: [f64; 3]) -> Option<(usize, [f64; 2])> {
    images
        .iter()
        .enumerate()
        .filter_map(|(index, image)| {
            let pixel = image.project(direction)?;
            Some((index, pixel, image.border_margin(pixel)))
        })
        .max_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(index, pixel, _)| (index, pixel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quaternion_axes;

    /// Rotations of the six photos of one real station whose scanner heading
    /// is 27.44 degrees about the vertical.
    const FACES: [[f64; 4]; 6] = [
        [0.6869, 0.6869, 0.1677, 0.1677],
        [0.6043, 0.6043, -0.3671, -0.3671],
        [-0.1677, -0.1677, 0.6869, 0.6869],
        [0.3671, 0.3671, 0.6043, 0.6043],
        [-0.0, 0.9715, 0.2372, 0.0],
        [0.9715, 0.0, -0.0, 0.2372],
    ];

    fn station() -> Vec<ScanImage> {
        FACES
            .iter()
            .map(|rotation| ScanImage {
                station: Some(0),
                position: [-2.19, 3.81, 0.0],
                axes: quaternion_axes(*rotation).unwrap(),
                width: 2048,
                height: 2048,
                focal: [1023.5, 1023.5],
                principal: [1023.5, 1023.5],
                format: ScanImageFormat::Jpeg,
                offset: 0,
                length: 0,
            })
            .collect()
    }

    fn close(a: [f64; 3], b: [f64; 3]) -> bool {
        (0..3).all(|axis| (a[axis] - b[axis]).abs() < 2e-3)
    }

    #[test]
    fn six_cube_faces_cover_the_scanner_axes_with_upright_horizontals() {
        let heading = 27.44f64.to_radians();
        let forward = [heading.cos(), heading.sin(), 0.0];
        let left = [-heading.sin(), heading.cos(), 0.0];
        let back = forward.map(|value| -value);
        let right = left.map(|value| -value);
        let images = station();
        let views: Vec<_> = images.iter().map(ScanImage::view_direction).collect();
        assert!(close(views[0], left));
        assert!(close(views[1], forward));
        assert!(close(views[2], right));
        assert!(close(views[3], back));
        assert!(close(views[4], [0.0, 0.0, 1.0]));
        assert!(close(views[5], [0.0, 0.0, -1.0]));
        for image in &images[..4] {
            assert!(close(image.axes[1], [0.0, 0.0, 1.0]));
        }
    }

    #[test]
    fn neighbouring_faces_share_their_edge_pixels() {
        let heading = 27.44f64.to_radians();
        let forward = [heading.cos(), heading.sin(), 0.0];
        let left = [-heading.sin(), heading.cos(), 0.0];
        let images = station();
        let pixel = |index: usize, direction: [f64; 3]| images[index].project(direction).unwrap();
        let near =
            |a: [f64; 2], b: [f64; 2]| (a[0] - b[0]).abs() < 2.0 && (a[1] - b[1]).abs() < 2.0;

        assert!(near(pixel(0, left), [1023.5, 1023.5]));
        let up_edge: [f64; 3] = std::array::from_fn(|axis| left[axis] + [0.0, 0.0, 1.0][axis]);
        assert!(near(pixel(0, up_edge), [1023.5, 0.0]));
        assert!(near(pixel(4, up_edge), [1023.5, 2047.0]));
        let corner: [f64; 3] = std::array::from_fn(|axis| left[axis] + forward[axis]);
        assert!(near(pixel(0, corner), [2047.0, 1023.5]));
        assert!(near(pixel(1, corner), [0.0, 1023.5]));
        let down_edge: [f64; 3] = std::array::from_fn(|axis| left[axis] - [0.0, 0.0, 1.0][axis]);
        assert!(near(pixel(0, down_edge), [1023.5, 2047.0]));
        assert!(near(pixel(5, down_edge), [1023.5, 0.0]));
        assert!(images[0].project(forward.map(|value| -value)).is_none());
    }

    #[test]
    fn every_direction_selects_one_face_and_rays_invert_projection() {
        let images = station();
        for step in 0..400 {
            let azimuth = step as f64 * 0.37;
            let elevation = ((step as f64 * 0.113).sin()) * 1.5;
            let direction = [
                elevation.cos() * azimuth.cos(),
                elevation.cos() * azimuth.sin(),
                elevation.sin(),
            ];
            let (index, pixel) = select_scan_image(&images, direction).unwrap();
            let ray = images[index].ray(pixel[0], pixel[1]);
            let length = dot(ray, ray).sqrt();
            assert!(close(ray.map(|value| value / length), direction));
            // The chosen face is the one looking most directly at the direction.
            let best = images
                .iter()
                .map(|image| dot(image.view_direction(), direction))
                .fold(f64::MIN, f64::max);
            assert!(dot(images[index].view_direction(), direction) > best - 2e-3);
        }
    }
}
