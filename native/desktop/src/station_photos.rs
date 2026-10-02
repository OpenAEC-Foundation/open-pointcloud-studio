//! Photos taken at scanner stations: decoding, the shared set shown as balls
//! in the 3D view, and the walking camera that can stand inside a station
//! and step out of it into the scene.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iced::Size;
use pointcloud_core::{ScanImage, ScanImageFormat};
use rayon::prelude::*;

/// Edge length of the small photos shown on station balls.
pub const BALL_PHOTO_SIZE: u32 = 256;
/// Largest edge length kept for the station being viewed from inside.
pub const PANORAMA_PHOTO_SIZE: u32 = 2048;
/// Texture layers available for ball photos on every supported adapter.
pub const MAX_BALL_PHOTOS: usize = 252;
/// A station with more photos than this is not a panorama this viewer shows.
const MAX_STATION_PHOTOS: usize = 16;

/// Radius of a station ball in scene units, and its limits on screen.
pub const BALL_RADIUS: f64 = 0.18;
pub const BALL_MIN_PIXELS: f32 = 15.0;
pub const BALL_MAX_PIXELS: f32 = 90.0;

pub const MIN_FIELD_OF_VIEW: f32 = 0.35;
pub const MAX_FIELD_OF_VIEW: f32 = 2.1;
const DEFAULT_FIELD_OF_VIEW: f32 = 1.5;
const MAX_WALK_PITCH: f32 = 1.55;

/// On-screen radius of a station ball at a camera depth.
pub fn ball_pixel_radius(scale: f64, depth: f64) -> f32 {
    ((BALL_RADIUS * scale / depth) as f32).clamp(BALL_MIN_PIXELS, BALL_MAX_PIXELS)
}

/// The decoded photos of one station, all resampled to one square size.
pub struct PhotoSet {
    pub source: PathBuf,
    pub station: usize,
    pub faces: Vec<ScanImage>,
    pub size: u32,
    /// One RGBA layer of `size` by `size` pixels per face, in face order.
    pub pixels: Vec<u8>,
}

impl fmt::Debug for PhotoSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PhotoSet")
            .field("source", &self.source)
            .field("station", &self.station)
            .field("faces", &self.faces.len())
            .field("size", &self.size)
            .finish()
    }
}

impl PhotoSet {
    pub fn layer(&self, face: usize) -> &[u8] {
        let length = (self.size * self.size * 4) as usize;
        &self.pixels[face * length..(face + 1) * length]
    }
}

/// Camera placement of one photo as the shader reads it: a direction maps to
/// image fractions with one division by its depth along the viewing axis.
pub fn face_uniform(image: &ScanImage, layer: u32) -> [[f32; 4]; 3] {
    let width = f64::from(image.width);
    let height = f64::from(image.height);
    let row = |axis: [f64; 3], scale: f64, last: f64| {
        [
            (axis[0] * scale) as f32,
            (axis[1] * scale) as f32,
            (axis[2] * scale) as f32,
            last as f32,
        ]
    };
    [
        row(
            image.axes[0],
            image.focal[0] / width,
            (image.principal[0] + 0.5) / width,
        ),
        row(
            image.axes[1],
            image.focal[1] / height,
            (image.principal[1] + 0.5) / height,
        ),
        row(image.axes[2], 1.0, f64::from(layer)),
    ]
}

/// Every station photo set that is drawn as a ball, in upload order. Sets are
/// only appended, so photos already on the GPU keep their texture layers.
#[derive(Debug, Default)]
pub struct PhotoAtlas {
    pub sets: Vec<Arc<PhotoSet>>,
    first_face: Vec<u32>,
    stations: HashMap<PathBuf, Vec<Option<u32>>>,
}

impl PhotoAtlas {
    /// Collect sets up to the available texture layers; returns how many sets
    /// did not fit.
    pub fn build(sets: impl IntoIterator<Item = Arc<PhotoSet>>) -> (Self, usize) {
        let mut atlas = Self::default();
        let mut faces = 0usize;
        let mut dropped = 0usize;
        for set in sets {
            if set.faces.is_empty()
                || set.size != BALL_PHOTO_SIZE
                || faces + set.faces.len() > MAX_BALL_PHOTOS
            {
                dropped += 1;
                continue;
            }
            let slots = atlas.stations.entry(set.source.clone()).or_default();
            if slots.len() <= set.station {
                slots.resize(set.station + 1, None);
            }
            if slots[set.station].is_some() {
                continue;
            }
            slots[set.station] = Some(atlas.sets.len() as u32);
            atlas.first_face.push(faces as u32);
            faces += set.faces.len();
            atlas.sets.push(set);
        }
        (atlas, dropped)
    }

    /// First photo and photo count of a station's ball.
    pub fn slot(&self, source: &Path, station: usize) -> Option<(u32, u32)> {
        let index = (*self.stations.get(source)?.get(station)?)? as usize;
        Some((self.first_face[index], self.sets[index].faces.len() as u32))
    }

    pub fn first_face(&self, set: usize) -> u32 {
        self.first_face[set]
    }
}

fn decode_face(bytes: &[u8], format: ScanImageFormat, size: u32) -> Result<Vec<u8>, String> {
    let decoded = match format {
        ScanImageFormat::Jpeg => {
            let mut decoder = ::image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))
                .map_err(|error| error.to_string())?;
            // Decode straight to the nearest larger DCT scale; a ball photo
            // needs an eighth of the stored resolution.
            let request = size.min(u32::from(u16::MAX)) as u16;
            decoder
                .scale(request, request)
                .map_err(|error| error.to_string())?;
            ::image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?
        }
        ScanImageFormat::Png => {
            ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Png)
                .map_err(|error| error.to_string())?
        }
    };
    let resized = if decoded.width() == size && decoded.height() == size {
        decoded
    } else {
        decoded.resize_exact(size, size, ::image::imageops::FilterType::Triangle)
    };
    Ok(resized.into_rgba8().into_raw())
}

fn decode_set(
    source: &Path,
    station: usize,
    faces: Vec<ScanImage>,
    size: u32,
) -> Result<PhotoSet, String> {
    let encoded =
        pointcloud_core::read_scan_images(source, &faces).map_err(|error| error.to_string())?;
    let layers: Result<Vec<Vec<u8>>, String> = encoded
        .par_iter()
        .zip(faces.par_iter())
        .map(|(bytes, face)| decode_face(bytes, face.format, size))
        .collect();
    Ok(PhotoSet {
        source: source.to_path_buf(),
        station,
        pixels: layers?.concat(),
        faces,
        size,
    })
}

fn station_faces(images: &[ScanImage]) -> BTreeMap<usize, Vec<ScanImage>> {
    let mut stations = BTreeMap::<usize, Vec<ScanImage>>::new();
    for image in images {
        if let Some(station) = image.station {
            stations.entry(station).or_default().push(image.clone());
        }
    }
    stations.retain(|_, faces| faces.len() <= MAX_STATION_PHOTOS);
    stations
}

/// Decode small ball photos for every station of a source that has photos.
pub fn load_ball_photos(source: &Path, images: &[ScanImage]) -> Result<Vec<PhotoSet>, String> {
    station_faces(images)
        .into_iter()
        .map(|(station, faces)| decode_set(source, station, faces, BALL_PHOTO_SIZE))
        .collect()
}

/// Decode the photos of one station at the resolution used inside it.
pub fn load_panorama(
    source: &Path,
    station: usize,
    images: &[ScanImage],
) -> Result<PhotoSet, String> {
    let faces = station_faces(images)
        .remove(&station)
        .ok_or("this station has no photos")?;
    let size = faces
        .iter()
        .map(|face| face.width.max(face.height))
        .max()
        .unwrap_or(BALL_PHOTO_SIZE)
        .clamp(BALL_PHOTO_SIZE, PANORAMA_PHOTO_SIZE);
    decode_set(source, station, faces, size)
}

/// First-person camera: an eye position in the scene and a viewing direction.
/// Standing at a scanner station it looks around the photos taken there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WalkView {
    pub eye: [f64; 3],
    /// Heading of the viewing direction about the vertical, from +X towards +Y.
    pub yaw: f32,
    /// Elevation of the viewing direction above the horizontal.
    pub pitch: f32,
    /// Horizontal field of view in radians.
    pub field_of_view: f32,
}

impl WalkView {
    pub fn new(eye: [f64; 3], yaw: f32) -> Self {
        Self {
            eye,
            yaw,
            pitch: 0.0,
            field_of_view: DEFAULT_FIELD_OF_VIEW,
        }
    }

    /// The view that looks the same way as an orbit camera with these angles.
    pub fn from_orbit(eye: [f64; 3], yaw: f32, pitch: f32) -> Self {
        Self {
            eye,
            yaw: wrap_angle(yaw + std::f32::consts::PI),
            pitch: (-pitch).clamp(-MAX_WALK_PITCH, MAX_WALK_PITCH),
            field_of_view: DEFAULT_FIELD_OF_VIEW,
        }
    }

    /// Right, up and forward directions of the view.
    pub fn basis(self) -> [[f64; 3]; 3] {
        let (sin_yaw, cos_yaw) = f64::from(self.yaw).sin_cos();
        let (sin_pitch, cos_pitch) = f64::from(self.pitch).sin_cos();
        [
            [sin_yaw, -cos_yaw, 0.0],
            [-sin_pitch * cos_yaw, -sin_pitch * sin_yaw, cos_pitch],
            [cos_pitch * cos_yaw, cos_pitch * sin_yaw, sin_pitch],
        ]
    }

    /// Focal length in logical pixels for a viewport.
    pub fn focal(self, size: Size) -> f32 {
        0.5 * size.width.max(1.0) / (self.field_of_view * 0.5).tan()
    }

    /// Screen position of a point, when it is in front of the eye.
    pub fn project(self, target: [f64; 3], size: Size) -> Option<(f32, f32)> {
        let [right, up, forward] = self.basis();
        let offset: [f64; 3] = std::array::from_fn(|axis| target[axis] - self.eye[axis]);
        let along =
            |axis: [f64; 3]| axis[0] * offset[0] + axis[1] * offset[1] + axis[2] * offset[2];
        let depth = along(forward);
        if depth <= 0.05 {
            return None;
        }
        let focal = f64::from(self.focal(size));
        Some((
            (f64::from(size.width) * 0.5 + along(right) * focal / depth) as f32,
            (f64::from(size.height) * 0.5 - along(up) * focal / depth) as f32,
        ))
    }

    pub fn distance_to(self, target: [f64; 3]) -> f64 {
        (0..3)
            .map(|axis| (target[axis] - self.eye[axis]).powi(2))
            .sum::<f64>()
            .sqrt()
    }

    /// Turn the view as if the scene were dragged with the pointer.
    pub fn look(&mut self, dx: f32, dy: f32, size: Size) {
        let focal = self.focal(size);
        self.yaw = wrap_angle(self.yaw + dx / focal);
        self.pitch = (self.pitch + dy / focal).clamp(-MAX_WALK_PITCH, MAX_WALK_PITCH);
    }

    pub fn zoom(&mut self, steps: f32) {
        self.field_of_view =
            (self.field_of_view * (-steps * 0.1).exp()).clamp(MIN_FIELD_OF_VIEW, MAX_FIELD_OF_VIEW);
    }

    /// Walk over level ground: forward follows the heading whatever the tilt
    /// of the view, sideways is to the right of it and up is vertical.
    pub fn advance(&mut self, forward: f64, right: f64, up: f64) {
        let (sin_yaw, cos_yaw) = f64::from(self.yaw).sin_cos();
        self.eye[0] += cos_yaw * forward + sin_yaw * right;
        self.eye[1] += sin_yaw * forward - cos_yaw * right;
        self.eye[2] += up;
    }
}

fn wrap_angle(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(axes: [[f64; 3]; 3], station: Option<usize>) -> ScanImage {
        ScanImage {
            station,
            position: [1.0, 2.0, 3.0],
            axes,
            width: 2048,
            height: 2048,
            focal: [1023.5, 1023.5],
            principal: [1023.5, 1023.5],
            format: ScanImageFormat::Jpeg,
            offset: 0,
            length: 1,
        }
    }

    /// A photo looking along +X with +Z up.
    fn forward_face() -> ScanImage {
        face(
            [[0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]],
            Some(0),
        )
    }

    fn set(source: &str, station: usize, faces: usize) -> Arc<PhotoSet> {
        Arc::new(PhotoSet {
            source: PathBuf::from(source),
            station,
            faces: vec![forward_face(); faces],
            size: BALL_PHOTO_SIZE,
            pixels: Vec::new(),
        })
    }

    #[test]
    fn shader_face_rows_reproduce_the_pinhole_projection() {
        let image = forward_face();
        let rows = face_uniform(&image, 7);
        assert_eq!(rows[2][3], 7.0);
        for direction in [[1.0, 0.0, 0.0], [1.0, 0.4, -0.7], [1.0, -0.95, 0.95]] {
            let pixel = image.project(direction).unwrap();
            let along = |row: [f32; 4]| {
                f64::from(row[0]) * direction[0]
                    + f64::from(row[1]) * direction[1]
                    + f64::from(row[2]) * direction[2]
            };
            let depth = -along(rows[2]);
            let column = along(rows[0]) / depth + f64::from(rows[0][3]);
            let row = -along(rows[1]) / depth + f64::from(rows[1][3]);
            assert!((column - (pixel[0] + 0.5) / 2048.0).abs() < 1e-5);
            assert!((row - (pixel[1] + 0.5) / 2048.0).abs() < 1e-5);
        }
    }

    #[test]
    fn atlas_appends_sets_and_reports_what_does_not_fit() {
        let (atlas, dropped) =
            PhotoAtlas::build([set("a.e57", 0, 6), set("b.e57", 2, 6), set("a.e57", 0, 6)]);
        assert_eq!(dropped, 0);
        assert_eq!(atlas.sets.len(), 2);
        assert_eq!(atlas.slot(Path::new("a.e57"), 0), Some((0, 6)));
        assert_eq!(atlas.slot(Path::new("b.e57"), 2), Some((6, 6)));
        assert_eq!(atlas.slot(Path::new("b.e57"), 0), None);
        assert_eq!(atlas.slot(Path::new("c.e57"), 0), None);

        let many = (0..50).map(|index| set("many.e57", index, 6));
        let (atlas, dropped) = PhotoAtlas::build(many);
        assert_eq!(atlas.sets.len(), MAX_BALL_PHOTOS / 6);
        assert_eq!(dropped, 50 - MAX_BALL_PHOTOS / 6);
    }

    #[test]
    fn photos_are_grouped_by_station_and_decoded_to_one_size() {
        let images = [
            face(forward_face().axes, Some(1)),
            face(forward_face().axes, None),
            face(forward_face().axes, Some(1)),
            face(forward_face().axes, Some(0)),
        ];
        let stations = station_faces(&images);
        assert_eq!(stations.keys().copied().collect::<Vec<_>>(), [0, 1]);
        assert_eq!(stations[&1].len(), 2);

        let mut source = ::image::RgbImage::new(64, 32);
        for (x, _, pixel) in source.enumerate_pixels_mut() {
            *pixel = ::image::Rgb(if x < 32 { [220, 30, 30] } else { [30, 30, 220] });
        }
        let mut jpeg = Vec::new();
        ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&source)
            .unwrap();
        let pixels = decode_face(&jpeg, ScanImageFormat::Jpeg, 16).unwrap();
        assert_eq!(pixels.len(), 16 * 16 * 4);
        let pixel = |x: usize, y: usize| &pixels[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
        assert!(pixel(2, 8)[0] > 180 && pixel(2, 8)[2] < 80);
        assert!(pixel(13, 8)[2] > 180 && pixel(13, 8)[0] < 80);
        assert_eq!(pixel(2, 8)[3], 255);
        assert!(decode_face(b"not an image", ScanImageFormat::Jpeg, 16).is_err());
    }

    #[test]
    fn walk_view_looks_where_it_is_dragged() {
        let size = Size::new(1000.0, 600.0);
        let origin = [10.0, 20.0, 1.5];
        let mut view = WalkView::new(origin, 0.0);
        let centre = view.project([15.0, 20.0, 1.5], size).unwrap();
        assert!((centre.0 - 500.0).abs() < 1e-3 && (centre.1 - 300.0).abs() < 1e-3);
        // +Y lies to the left of a view along +X, and up is up.
        let left = view.project([15.0, 21.0, 1.5], size).unwrap();
        assert!(left.0 < 500.0);
        let above = view.project([15.0, 20.0, 2.5], size).unwrap();
        assert!(above.1 < 300.0);
        assert!(view.project([5.0, 20.0, 1.5], size).is_none());

        // Dragging the photo to the right brings what was on the left to the centre.
        view.look(120.0, 0.0, size);
        let moved = view.project([15.0, 21.0, 1.5], size).unwrap();
        assert!(moved.0 > left.0);
        view.look(0.0, 5_000.0, size);
        assert!(view.pitch <= MAX_WALK_PITCH);

        let wide = view.focal(size);
        view.zoom(3.0);
        assert!(view.focal(size) > wide);
        view.zoom(1_000.0);
        assert_eq!(view.field_of_view, MIN_FIELD_OF_VIEW);
    }

    #[test]
    fn walking_stays_level_and_follows_the_heading() {
        let mut view = WalkView::new([0.0, 0.0, 1.6], std::f32::consts::FRAC_PI_2);
        view.pitch = 1.0;
        // Forward follows the heading (+Y here) however far the view tilts.
        view.advance(2.0, 0.0, 0.0);
        assert!(view.eye[0].abs() < 1e-6 && (view.eye[1] - 2.0).abs() < 1e-6);
        assert_eq!(view.eye[2], 1.6);
        // To the right of a +Y heading lies +X; up is the vertical.
        view.advance(0.0, 1.0, 0.5);
        assert!((view.eye[0] - 1.0).abs() < 1e-6 && (view.eye[1] - 2.0).abs() < 1e-6);
        assert!((view.eye[2] - 2.1).abs() < 1e-9);
        assert!((view.distance_to([1.0, 2.0, 0.1]) - 2.0).abs() < 1e-9);
    }

    #[test]
    fn walk_view_matches_the_orbit_camera_it_starts_from() {
        // An orbit camera looks along the negative of its toward-camera axis.
        let (yaw, pitch) = (-0.8f32, 0.6f32);
        let view = WalkView::from_orbit([0.0; 3], yaw, pitch);
        let [right, up, forward] = view.basis();
        let (sy, cy) = f64::from(yaw).sin_cos();
        let (sp, cp) = f64::from(pitch).sin_cos();
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-6);
        assert!(near(right, [-sy, cy, 0.0]));
        assert!(near(up, [-sp * cy, -sp * sy, cp]));
        assert!(near(forward, [-cp * cy, -cp * sy, -sp]));
    }

    #[test]
    fn walk_basis_matches_the_photo_camera_frame() {
        // Looking along +X the view axes equal those of a photo taken that way.
        let [right, up, forward] = WalkView::new([0.0; 3], 0.0).basis();
        let image = forward_face();
        let near = |a: [f64; 3], b: [f64; 3]| (0..3).all(|axis| (a[axis] - b[axis]).abs() < 1e-9);
        assert!(near(right, image.axes[0]));
        assert!(near(up, image.axes[1]));
        assert!(near(forward, image.view_direction()));
    }

    #[test]
    fn ball_radius_is_bounded_on_screen() {
        assert_eq!(ball_pixel_radius(800.0, 1_000.0), BALL_MIN_PIXELS);
        assert_eq!(ball_pixel_radius(800.0, 0.5), BALL_MAX_PIXELS);
        let middle = ball_pixel_radius(800.0, 6.0);
        assert!(middle > BALL_MIN_PIXELS && middle < BALL_MAX_PIXELS);
    }
}
