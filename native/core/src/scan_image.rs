//! Photos stored with scans and the camera geometry to place them.
//!
//! A station panorama is usually stored as several pinhole photos that share
//! the scanner position, for example six 90-degree cube faces. Nothing here
//! assumes a face order or count: every photo carries its own pose.
//!
//! Other photos of a file stand on their own: equirectangular or cylindrical
//! panoramas, and pinhole photos taken along a path by a camera that measured
//! no points. They are `FilePhoto`s.

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

    /// The same photo as a photo of the file: a pinhole photo with the
    /// station's number, which projects and casts rays as this one does.
    pub fn as_file_photo(&self) -> FilePhoto {
        FilePhoto {
            name: None,
            station: self.station,
            position: self.position,
            axes: self.axes,
            width: self.width,
            height: self.height,
            projection: PhotoProjection::Pinhole {
                focal: self.focal,
                principal: self.principal,
            },
            format: self.format,
            offset: self.offset,
            length: self.length,
        }
    }

    /// Distance, in image fractions, from a pixel to the nearest photo edge.
    fn border_margin(&self, pixel: [f64; 2]) -> f64 {
        let column = (pixel[0] + 0.5) / f64::from(self.width);
        let row = (pixel[1] + 0.5) / f64::from(self.height);
        column.min(1.0 - column).min(row).min(1.0 - row)
    }
}

/// The kind of camera that took a photo of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhotoKind {
    /// A frame camera: one picture through a lens.
    Pinhole,
    /// An equirectangular panorama of the whole sphere around the camera.
    Spherical,
    /// A panorama on a cylinder around the camera's vertical axis.
    Cylindrical,
}

impl PhotoKind {
    pub const ALL: [Self; 3] = [Self::Pinhole, Self::Spherical, Self::Cylindrical];

    /// The name the command API uses.
    pub fn key(self) -> &'static str {
        match self {
            Self::Pinhole => "pinhole",
            Self::Spherical => "spherical",
            Self::Cylindrical => "cylindrical",
        }
    }
}

/// How a photo maps the directions around its camera to its pixels. Pixel
/// coordinates count columns to the right and rows down from the top-left
/// pixel, with integer values at pixel centres, as for `ScanImage`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhotoProjection {
    /// The E57 pinhole convention of `ScanImage`: +X is image right, +Y is
    /// image up and the camera looks along -Z. Focal length and principal
    /// point in pixels.
    Pinhole {
        focal: [f64; 2],
        principal: [f64; 2],
    },
    /// An equirectangular panorama with pixels of these angles across and
    /// down, in radians. The middle of the photo looks along the camera's +X
    /// axis, its top is the camera's +Z axis, and the columns run clockwise
    /// seen from above: the photo is the view from inside the sphere, not its
    /// mirror image.
    Spherical { pixel_size: [f64; 2] },
    /// A panorama on a cylinder of `radius` metres about the camera's Z
    /// axis. Columns are as for a spherical photo, `pixel_size[0]` radians
    /// each; rows are `pixel_size[1]` metres high on the cylinder, and the
    /// camera's horizontal plane meets it at row `principal_row`.
    Cylindrical {
        pixel_size: [f64; 2],
        radius: f64,
        principal_row: f64,
    },
}

/// A photo of a file with its own pose that is not one of the pinhole
/// photos of a scanner station: a panorama, or a photo taken along a path by
/// a camera that measured no points.
#[derive(Debug, Clone, PartialEq)]
pub struct FilePhoto {
    /// The name the file gives the photo.
    pub name: Option<String>,
    /// Index into `PointCloud::scan_poses` of the station that took the
    /// photo, when the file tells.
    pub station: Option<usize>,
    pub position: [f64; 3],
    /// Registered unit directions of the camera's X, Y and Z axes.
    pub axes: [[f64; 3]; 3],
    pub width: u32,
    pub height: u32,
    pub projection: PhotoProjection,
    pub format: ScanImageFormat,
    /// Location of the encoded photo inside the source file.
    pub offset: u64,
    pub length: u64,
}

impl FilePhoto {
    pub fn kind(&self) -> PhotoKind {
        match self.projection {
            PhotoProjection::Pinhole { .. } => PhotoKind::Pinhole,
            PhotoProjection::Spherical { .. } => PhotoKind::Spherical,
            PhotoProjection::Cylindrical { .. } => PhotoKind::Cylindrical,
        }
    }

    /// Registered direction the middle of the photo looks along.
    pub fn view_direction(&self) -> [f64; 3] {
        match self.projection {
            PhotoProjection::Pinhole { .. } => self.axes[2].map(|value| -value),
            _ => self.axes[0],
        }
    }

    /// Registered direction of the top of the photo.
    pub fn up_direction(&self) -> [f64; 3] {
        match self.projection {
            PhotoProjection::Pinhole { .. } => self.axes[1],
            _ => self.axes[2],
        }
    }

    /// Whether the columns of a panorama go all the way round.
    fn wraps(&self, pixel_width: f64) -> bool {
        pixel_width * f64::from(self.width) >= std::f64::consts::TAU * (1.0 - 1e-6)
    }

    /// Pixel that sees a registered direction from the camera, when the
    /// photo covers it.
    pub fn project(&self, direction: [f64; 3]) -> Option<[f64; 2]> {
        let local = self.axes.map(|axis| dot(direction, axis));
        let width = f64::from(self.width);
        let height = f64::from(self.height);
        // Edge coordinates first: 0 at the left or top edge of the photo.
        let (column, row) = match self.projection {
            PhotoProjection::Pinhole { focal, principal } => {
                let depth = -local[2];
                if depth.is_nan() || depth <= 0.0 {
                    return None;
                }
                (
                    principal[0] + 0.5 + focal[0] * local[0] / depth,
                    principal[1] + 0.5 - focal[1] * local[1] / depth,
                )
            }
            PhotoProjection::Spherical { pixel_size } => {
                let azimuth = local[1].atan2(local[0]);
                let elevation = local[2].atan2(local[0].hypot(local[1]));
                (
                    self.around(width * 0.5 - azimuth / pixel_size[0], pixel_size[0]),
                    height * 0.5 - elevation / pixel_size[1],
                )
            }
            PhotoProjection::Cylindrical {
                pixel_size,
                radius,
                principal_row,
            } => {
                let across = local[0].hypot(local[1]);
                if across.is_nan() || across <= 0.0 {
                    return None;
                }
                let azimuth = local[1].atan2(local[0]);
                (
                    self.around(width * 0.5 - azimuth / pixel_size[0], pixel_size[0]),
                    principal_row + 0.5 - radius * local[2] / across / pixel_size[1],
                )
            }
        };
        (column >= 0.0 && row >= 0.0 && column < width && row < height)
            .then_some([column - 0.5, row - 0.5])
    }

    /// A column of a panorama that goes all the way round, brought within
    /// the photo.
    fn around(&self, column: f64, pixel_width: f64) -> f64 {
        let width = f64::from(self.width);
        if !self.wraps(pixel_width) {
            return column;
        }
        // A remainder may round up to the width itself, just left of the seam.
        let around = column.rem_euclid(width);
        if around < width {
            around
        } else {
            0.0
        }
    }

    /// Registered direction seen by a pixel; not normalised.
    pub fn ray(&self, column: f64, row: f64) -> [f64; 3] {
        let width = f64::from(self.width);
        let height = f64::from(self.height);
        let local = match self.projection {
            PhotoProjection::Pinhole { focal, principal } => [
                (column - principal[0]) / focal[0],
                -(row - principal[1]) / focal[1],
                -1.0,
            ],
            PhotoProjection::Spherical { pixel_size } => {
                let azimuth = (width * 0.5 - (column + 0.5)) * pixel_size[0];
                let elevation = (height * 0.5 - (row + 0.5)) * pixel_size[1];
                [
                    elevation.cos() * azimuth.cos(),
                    elevation.cos() * azimuth.sin(),
                    elevation.sin(),
                ]
            }
            PhotoProjection::Cylindrical {
                pixel_size,
                radius,
                principal_row,
            } => {
                let azimuth = (width * 0.5 - (column + 0.5)) * pixel_size[0];
                [
                    azimuth.cos(),
                    azimuth.sin(),
                    (principal_row - row) * pixel_size[1] / radius,
                ]
            }
        };
        std::array::from_fn(|axis| {
            self.axes[0][axis] * local[0]
                + self.axes[1][axis] * local[1]
                + self.axes[2][axis] * local[2]
        })
    }

    /// Angles the photo spans across and down, in radians.
    pub fn field_of_view(&self) -> [f64; 2] {
        let width = f64::from(self.width);
        let height = f64::from(self.height);
        match self.projection {
            PhotoProjection::Pinhole { focal, principal } => [
                ((principal[0] + 0.5) / focal[0]).atan()
                    + ((width - principal[0] - 0.5) / focal[0]).atan(),
                ((principal[1] + 0.5) / focal[1]).atan()
                    + ((height - principal[1] - 0.5) / focal[1]).atan(),
            ],
            PhotoProjection::Spherical { pixel_size } => {
                [pixel_size[0] * width, pixel_size[1] * height]
            }
            PhotoProjection::Cylindrical {
                pixel_size,
                radius,
                principal_row,
            } => [
                pixel_size[0] * width,
                ((principal_row + 0.5) * pixel_size[1] / radius).atan()
                    + ((height - principal_row - 0.5) * pixel_size[1] / radius).atan(),
            ],
        }
    }
}

/// The photos of a file that are not the pinhole photos of a scanner
/// station, and what the file states about its coordinates.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilePhotos {
    /// In the order of the file, which is the order they were taken in.
    pub photos: Vec<FilePhoto>,
    /// The coordinate reference system the file states, such as an EPSG code.
    pub coordinate_system: Option<String>,
    /// Images that are not listed: a preview without a projection, one
    /// without a pose, or one whose values cannot place it.
    pub skipped: usize,
}

impl FilePhotos {
    /// How many photos are of a kind.
    pub fn count(&self, kind: PhotoKind) -> usize {
        self.photos
            .iter()
            .filter(|photo| photo.kind() == kind)
            .count()
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
    fn a_station_photo_as_a_file_photo_sees_what_it_sees() {
        let heading = 27.44f64.to_radians();
        let directions = [
            [heading.cos(), heading.sin(), 0.1],
            [-heading.sin(), heading.cos(), 0.3],
            [0.2, -0.1, 1.0],
            [0.3, 0.2, -1.0],
            [-1.0, 0.4, -0.2],
        ];
        for image in station() {
            let photo = image.as_file_photo();
            assert_eq!(photo.kind(), PhotoKind::Pinhole);
            assert_eq!(photo.view_direction(), image.view_direction());
            for direction in directions {
                match (image.project(direction), photo.project(direction)) {
                    (Some(a), Some(b)) => {
                        assert!((a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9);
                    }
                    (a, b) => assert_eq!(a.is_some(), b.is_some()),
                }
            }
            for pixel in [[0.0, 0.0], [1023.5, 1023.5], [2047.0, 100.0]] {
                let (a, b) = (image.ray(pixel[0], pixel[1]), photo.ray(pixel[0], pixel[1]));
                assert!((0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-12));
            }
        }
    }

    fn photo(
        axes: [[f64; 3]; 3],
        width: u32,
        height: u32,
        projection: PhotoProjection,
    ) -> FilePhoto {
        FilePhoto {
            name: None,
            station: None,
            position: [100.0, 200.0, 3.0],
            axes,
            width,
            height,
            projection,
            format: ScanImageFormat::Jpeg,
            offset: 0,
            length: 1,
        }
    }

    /// A camera turned a quarter turn counter-clockwise about the vertical:
    /// its X axis is the scene's +Y and its Y axis the scene's -X.
    fn quarter_turn() -> [[f64; 3]; 3] {
        let half = std::f64::consts::FRAC_1_SQRT_2;
        quaternion_axes([half, 0.0, 0.0, half]).unwrap()
    }

    fn near(pixel: Option<[f64; 2]>, expected: [f64; 2]) -> bool {
        pixel.is_some_and(|pixel| (0..2).all(|axis| (pixel[axis] - expected[axis]).abs() < 1e-6))
    }

    fn assert_rays_invert(photo: &FilePhoto, directions: &[[f64; 3]]) {
        for direction in directions {
            let pixel = photo.project(*direction).unwrap();
            let ray = photo.ray(pixel[0], pixel[1]);
            let length = dot(ray, ray).sqrt();
            let along = dot(*direction, *direction).sqrt();
            assert!(
                close(
                    ray.map(|value| value / length),
                    direction.map(|value| value / along)
                ),
                "{direction:?}"
            );
        }
    }

    /// The conventions of an equirectangular panorama. They are the ones
    /// under which the photos of files that hold both agree with the colours
    /// stored with their points; the other ways to read the columns, rows and
    /// pose do not.
    #[test]
    fn a_panorama_looks_along_its_x_axis_with_columns_clockwise_and_its_top_up() {
        let turn = std::f64::consts::TAU / 64.0;
        let photo = photo(
            quarter_turn(),
            64,
            32,
            PhotoProjection::Spherical {
                pixel_size: [turn, std::f64::consts::PI / 32.0],
            },
        );
        assert_eq!(photo.kind(), PhotoKind::Spherical);
        assert!(close(photo.view_direction(), [0.0, 1.0, 0.0]));
        assert!(close(photo.up_direction(), [0.0, 0.0, 1.0]));
        // The middle of the photo looks along the camera's X axis, here +Y.
        assert!(near(photo.project([0.0, 1.0, 0.0]), [31.5, 15.5]));
        // What lies to the left of that view (-X here) is left in the photo,
        // what lies to its right is right: seen from inside, not mirrored.
        assert!(near(photo.project([-1.0, 0.0, 0.0]), [15.5, 15.5]));
        assert!(near(photo.project([1.0, 0.0, 0.0]), [47.5, 15.5]));
        // Straight behind is the seam at the left and right edges.
        assert!(near(photo.project([0.0, -1.0, 0.0]), [-0.5, 15.5]));
        // The top of the photo is up: 45 degrees up is a quarter of the way.
        assert!(near(photo.project([0.0, 1.0, 1.0]), [31.5, 7.5]));
        assert!(near(photo.project([0.0, 1.0, -1.0]), [31.5, 23.5]));
        assert_rays_invert(
            &photo,
            &[
                [0.3, 0.9, 0.2],
                [-0.7, -0.2, -0.5],
                [0.1, -0.9, 0.6],
                [0.0, 1.0, -0.99],
            ],
        );
        let [across, down] = photo.field_of_view();
        assert!((across - std::f64::consts::TAU).abs() < 1e-12);
        assert!((down - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn a_partial_panorama_does_not_wrap_round() {
        let photo = photo(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            32,
            16,
            PhotoProjection::Spherical {
                pixel_size: [std::f64::consts::PI / 64.0, std::f64::consts::PI / 64.0],
            },
        );
        // Half a turn of azimuth over 32 columns: a quarter turn each way.
        assert!(near(photo.project([1.0, 0.0, 0.0]), [15.5, 7.5]));
        assert!(photo.project([0.0, 1.0, 0.0]).is_none());
        assert!(photo.project([-1.0, 0.0, 0.0]).is_none());
        assert!(photo.project([1.0, 0.0, 1.0]).is_none());
    }

    /// A pinhole photo of a file keeps the convention of the station photos.
    #[test]
    fn a_pinhole_photo_of_a_file_looks_along_minus_z_with_y_up() {
        // Tilted down and turned, as a camera held by hand is.
        let axes = quaternion_axes([0.86, 0.33, -0.13, -0.36]).unwrap();
        let photo = photo(
            axes,
            1500,
            2000,
            PhotoProjection::Pinhole {
                focal: [1100.0, 1100.0],
                principal: [749.5, 999.5],
            },
        );
        assert_eq!(photo.kind(), PhotoKind::Pinhole);
        let forward = photo.view_direction();
        assert!(near(photo.project(forward), [749.5, 999.5]));
        let right: [f64; 3] = std::array::from_fn(|axis| forward[axis] + 0.1 * axes[0][axis]);
        let up: [f64; 3] = std::array::from_fn(|axis| forward[axis] + 0.1 * axes[1][axis]);
        assert!(near(photo.project(right), [859.5, 999.5]));
        assert!(near(photo.project(up), [749.5, 889.5]));
        assert!(photo.project(forward.map(|value| -value)).is_none());
        assert!(close(photo.up_direction(), axes[1]));
        assert_rays_invert(&photo, &[forward, right, up]);
        let [across, down] = photo.field_of_view();
        assert!((across - 2.0 * (750.0f64 / 1100.0).atan()).abs() < 1e-12);
        assert!((down - 2.0 * (1000.0f64 / 1100.0).atan()).abs() < 1e-12);
    }

    #[test]
    fn a_cylindrical_photo_has_columns_like_a_panorama_and_rows_on_its_cylinder() {
        let photo = photo(
            quarter_turn(),
            360,
            100,
            PhotoProjection::Cylindrical {
                pixel_size: [std::f64::consts::TAU / 360.0, 0.01],
                radius: 1.0,
                principal_row: 50.0,
            },
        );
        assert_eq!(photo.kind(), PhotoKind::Cylindrical);
        assert!(near(photo.project([0.0, 1.0, 0.0]), [179.5, 50.0]));
        assert!(near(photo.project([-1.0, 0.0, 0.0]), [89.5, 50.0]));
        // A quarter of a metre up at a metre: 25 rows of a centimetre.
        assert!(near(photo.project([0.0, 2.0, 0.5]), [179.5, 25.0]));
        assert!(photo.project([0.0, 1.0, 0.6]).is_none());
        assert!(photo.project([0.0, 0.0, 1.0]).is_none());
        assert_rays_invert(&photo, &[[0.2, 1.0, 0.3], [-1.0, -0.4, -0.2]]);
        let down = photo.field_of_view()[1];
        assert!((down - (0.505f64.atan() + 0.495f64.atan())).abs() < 1e-12);
    }

    #[test]
    fn photos_are_counted_by_kind() {
        let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let spherical = PhotoProjection::Spherical {
            pixel_size: [0.1, 0.1],
        };
        let pinhole = PhotoProjection::Pinhole {
            focal: [10.0, 10.0],
            principal: [5.0, 5.0],
        };
        let photos = FilePhotos {
            photos: vec![
                photo(identity, 10, 10, spherical),
                photo(identity, 10, 10, pinhole),
                photo(identity, 10, 10, spherical),
            ],
            coordinate_system: None,
            skipped: 0,
        };
        assert_eq!(photos.count(PhotoKind::Spherical), 2);
        assert_eq!(photos.count(PhotoKind::Pinhole), 1);
        assert_eq!(photos.count(PhotoKind::Cylindrical), 0);
        assert_eq!(
            PhotoKind::ALL.map(PhotoKind::key),
            ["pinhole", "spherical", "cylindrical"]
        );
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
