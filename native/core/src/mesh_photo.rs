//! Calibrated photographs sampled on a measured surface. Image decoding is
//! lazy and bounded; visibility uses the existing survey-relative mesh BVH.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::mesh_quality::TriangleIndex;
use crate::obj_mesh::{MeshGeometry, MAX_TRIANGLES, MAX_VERTICES};
use crate::photo_colour::PhotoPixels;
use crate::{FilePhoto, LoadError, PhotoProjection};

const CACHE_PHOTOS: usize = 4;
pub const MAX_PHOTO_SIDE: u32 = 2048;

#[derive(Debug, Clone, Copy)]
pub struct PhotoTextureOptions {
    pub max_distance: f64,
    pub blend: bool,
}

impl Default for PhotoTextureOptions {
    fn default() -> Self {
        Self {
            max_distance: 20.0,
            blend: true,
        }
    }
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PhotoTextureCoverage {
    pub photograph_texels: u64,
    pub point_colour_texels: u64,
    pub unobserved_texels: u64,
    pub photos_used: Vec<usize>,
    pub photos_failed: usize,
}

pub(crate) struct PhotoBaker<'a> {
    mesh: &'a MeshGeometry,
    photos: &'a [FilePhoto],
    index: TriangleIndex,
    options: PhotoTextureOptions,
    cache: VecDeque<(usize, Arc<PhotoPixels>)>,
    failed: Vec<bool>,
}

struct ReadyPhoto {
    index: usize,
    photo: FilePhoto,
    pixels: Arc<PhotoPixels>,
    gain: f32,
}

pub(crate) struct PhotoChart<'a> {
    index: &'a TriangleIndex,
    vertices: [[f64; 3]; 3],
    normal: [f64; 3],
    colours: Option<[[u8; 3]; 3]>,
    photos: Vec<ReadyPhoto>,
    options: PhotoTextureOptions,
}

/// Counts interior texels only; padding does not inflate coverage.
#[derive(Default)]
pub(crate) struct ChartCoverage {
    pub photographs: u64,
    pub points: u64,
    pub unseen: u64,
    pub used: u8,
}

impl PhotoBaker<'_> {
    pub(crate) fn new<'a>(
        mesh: &'a MeshGeometry,
        photos: &'a [FilePhoto],
        options: PhotoTextureOptions,
    ) -> Result<PhotoBaker<'a>, LoadError> {
        if mesh.vertices.is_empty()
            || mesh.triangles.is_empty()
            || mesh.vertices.len() > MAX_VERTICES
            || mesh.triangles.len() > MAX_TRIANGLES
            || mesh.vertices.iter().flatten().any(|v| !v.is_finite())
            || mesh
                .triangles
                .iter()
                .flatten()
                .any(|i| *i as usize >= mesh.vertices.len())
            || mesh
                .colors
                .as_ref()
                .is_some_and(|c| c.len() != mesh.vertices.len())
            || !options.max_distance.is_finite()
            || !(0.5..=500.0).contains(&options.max_distance)
        {
            return Err(LoadError::InvalidData(
                "invalid mesh or photograph texture settings".into(),
            ));
        }
        let index = TriangleIndex::build(mesh)
            .ok_or_else(|| LoadError::InvalidData("mesh has no usable surface".into()))?;
        Ok(PhotoBaker {
            mesh,
            photos,
            index,
            options,
            cache: VecDeque::new(),
            failed: vec![false; photos.len()],
        })
    }

    pub(crate) fn failed(&self) -> usize {
        self.failed.iter().filter(|failed| **failed).count()
    }

    #[cfg(test)]
    pub(crate) fn chart(
        &mut self,
        face: [u32; 3],
        decode: &mut dyn FnMut(usize) -> Result<PhotoPixels, LoadError>,
    ) -> Result<PhotoChart<'_>, LoadError> {
        self.chart_oriented(face, face, decode)
    }

    pub(crate) fn chart_oriented(
        &mut self,
        face: [u32; 3],
        source_face: [u32; 3],
        decode: &mut dyn FnMut(usize) -> Result<PhotoPixels, LoadError>,
    ) -> Result<PhotoChart<'_>, LoadError> {
        let vertices = face.map(|i| self.mesh.vertices[i as usize]);
        let centre = std::array::from_fn(|a| vertices.iter().map(|v| v[a]).sum::<f64>() / 3.0);
        let orientation = source_face.map(|index| self.mesh.vertices[index as usize]);
        let raw_normal = cross(
            sub(orientation[1], orientation[0]),
            sub(orientation[2], orientation[0]),
        );
        let magnitude = dot(raw_normal, raw_normal).sqrt();
        let normal = raw_normal.map(|v| {
            if magnitude > 1e-15 {
                v / magnitude
            } else {
                0.0
            }
        });
        let mut ranked = self
            .photos
            .iter()
            .enumerate()
            .filter_map(|(index, photo)| {
                if self.failed[index]
                    || photo.width == 0
                    || photo.height == 0
                    || photo
                        .position
                        .iter()
                        .chain(photo.axes.iter().flatten())
                        .any(|v| !v.is_finite())
                {
                    return None;
                }
                if !std::iter::once(centre)
                    .chain(vertices)
                    .any(|point| photo.project(sub(point, photo.position)).is_some())
                {
                    return None;
                }
                let toward = sub(photo.position, centre);
                let distance_squared = dot(toward, toward);
                let facing = dot(normal, toward);
                (distance_squared > 1e-12 && facing > 0.0)
                    .then_some((index, facing / distance_squared.max(1e-6)))
            })
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut photos = Vec::new();
        for (index, _) in ranked {
            if photos.len() == CACHE_PHOTOS {
                break;
            }
            let pixels = if let Some(place) = self.cache.iter().position(|(id, _)| *id == index) {
                let cached = self.cache.remove(place).unwrap();
                let pixels = Arc::clone(&cached.1);
                self.cache.push_back(cached);
                pixels
            } else {
                let pixels = match decode(index) {
                    Ok(pixels) => pixels,
                    Err(LoadError::Cancelled) => return Err(LoadError::Cancelled),
                    Err(_) => {
                        self.failed[index] = true;
                        continue;
                    }
                };
                if !pixels.valid()
                    || pixels.width > MAX_PHOTO_SIDE
                    || pixels.height > MAX_PHOTO_SIDE
                {
                    self.failed[index] = true;
                    continue;
                }
                let pixels = Arc::new(pixels);
                if self.cache.len() == CACHE_PHOTOS {
                    self.cache.pop_front();
                }
                self.cache.push_back((index, Arc::clone(&pixels)));
                pixels
            };
            photos.push(ReadyPhoto {
                index,
                photo: self.photos[index].clone(),
                pixels,
                gain: 1.0,
            });
        }
        let mut chart = PhotoChart {
            index: &self.index,
            vertices,
            normal,
            colours: self
                .mesh
                .colors
                .as_ref()
                .map(|colours| face.map(|i| colours[i as usize])),
            photos,
            options: self.options,
        };
        chart.balance_exposure();
        Ok(chart)
    }
}

impl PhotoChart<'_> {
    pub(crate) fn photo_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.photos.iter().map(|photo| photo.index)
    }

    fn position(&self, barycentric: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|axis| {
            (0..3)
                .map(|i| self.vertices[i][axis] * barycentric[i])
                .sum()
        })
    }

    fn observation(
        &self,
        photo: &ReadyPhoto,
        point: [f64; 3],
        pending: &mut Vec<u32>,
    ) -> Option<([f32; 3], f64)> {
        let direction = sub(point, photo.photo.position);
        let squared = dot(direction, direction);
        let distance = squared.sqrt();
        if distance <= 1e-9 || distance > self.options.max_distance {
            return None;
        }
        let facing = -dot(self.normal, direction) / distance;
        if facing <= 0.0 {
            return None;
        }
        let pixel = photo.photo.project(direction)?;
        if !opaque(photo, pixel)
            || !self
                .index
                .segment_clear(photo.photo.position, point, pending)
        {
            return None;
        }
        let wraps = wraps(&photo.photo);
        let rgb = photo
            .pixels
            .sample(pixel, [photo.photo.width, photo.photo.height], wraps)
            .map(to_linear);
        let edge = ((pixel[0] + 0.5).min(f64::from(photo.photo.width) - 0.5 - pixel[0])
            / f64::from(photo.photo.width))
        .min(
            (pixel[1] + 0.5).min(f64::from(photo.photo.height) - 0.5 - pixel[1])
                / f64::from(photo.photo.height),
        );
        let weight = (facing * (0.25 + edge.max(0.0)) / squared.max(1e-6)).powi(4);
        Some((rgb, weight))
    }

    fn balance_exposure(&mut self) {
        if !self.options.blend || self.photos.len() < 2 {
            return;
        }
        let places = [
            [1., 0., 0.],
            [0., 1., 0.],
            [0., 0., 1.],
            [1. / 3.; 3],
            [0.5, 0.5, 0.],
            [0., 0.5, 0.5],
            [0.5, 0., 0.5],
        ];
        let mut pending = Vec::with_capacity(32);
        for place in 1..self.photos.len() {
            let mut ratios = Vec::new();
            for barycentric in places {
                let point = self.position(barycentric);
                if let (Some((reference, _)), Some((other, _))) = (
                    self.observation(&self.photos[0], point, &mut pending),
                    self.observation(&self.photos[place], point, &mut pending),
                ) {
                    let luma = |rgb: [f32; 3]| 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2];
                    let (a, b) = (luma(reference), luma(other));
                    if a > 0.01 && b > 0.01 && a < 0.9 && b < 0.9 {
                        ratios.push(a / b);
                    }
                }
            }
            if ratios.len() >= 3 {
                ratios.sort_by(f32::total_cmp);
                self.photos[place].gain = ratios[ratios.len() / 2].clamp(0.5, 2.0);
            }
        }
    }

    /// One surface texel, its source category, and which candidate images
    /// contributed. Incompatible colours choose the best observation, so a
    /// foreground edge is not averaged into a second, displaced edge.
    pub(crate) fn sample(
        &self,
        barycentric: [f64; 3],
        pending: &mut Vec<u32>,
    ) -> ([u8; 3], u8, u8) {
        let point = self.position(barycentric);
        let mut found = [(0.0, [0.0; 3], 0u8); CACHE_PHOTOS];
        let mut count = 0;
        for (index, photo) in self.photos.iter().enumerate() {
            if let Some((rgb, weight)) = self.observation(photo, point, pending) {
                found[count] = (
                    weight,
                    rgb.map(|v| (v * photo.gain).clamp(0.0, 1.0)),
                    1 << index,
                );
                count += 1;
            }
        }
        if count > 0 {
            let found = &mut found[..count];
            found.sort_by(|a, b| b.0.total_cmp(&a.0));
            let reference = found[0].1.map(from_linear);
            let mut colour = [0.0; 3];
            let mut total = 0.0;
            let mut used = 0;
            for (place, (weight, rgb, mask)) in found.iter().enumerate() {
                if place > 0
                    && (!self.options.blend
                        || rgb
                            .iter()
                            .zip(reference)
                            .any(|(v, reference)| (from_linear(*v) - reference).abs() > 36.0))
                {
                    continue;
                }
                for channel in 0..3 {
                    colour[channel] += f64::from(rgb[channel]) * weight;
                }
                total += weight;
                used |= mask;
            }
            if total > 0.0 {
                return (
                    colour.map(|v| from_linear((v / total) as f32).round().clamp(0.0, 255.0) as u8),
                    0,
                    used,
                );
            }
        }
        if let Some(colours) = self.colours {
            let rgb = std::array::from_fn(|channel| {
                (0..3)
                    .map(|i| barycentric[i] * f64::from(colours[i][channel]))
                    .sum::<f64>()
                    .round()
                    .clamp(0.0, 255.0) as u8
            });
            (rgb, 1, 0)
        } else {
            ([128; 3], 2, 0)
        }
    }
}

fn wraps(photo: &FilePhoto) -> bool {
    match photo.projection {
        PhotoProjection::Spherical { pixel_size }
        | PhotoProjection::Cylindrical { pixel_size, .. } => {
            pixel_size[0] * f64::from(photo.width) >= std::f64::consts::TAU * (1.0 - 1e-6)
        }
        _ => false,
    }
}

fn opaque(photo: &ReadyPhoto, pixel: [f64; 2]) -> bool {
    let x = (pixel[0] + 0.5) * f64::from(photo.pixels.width) / f64::from(photo.photo.width) - 0.5;
    let y = (pixel[1] + 0.5) * f64::from(photo.pixels.height) / f64::from(photo.photo.height) - 0.5;
    for row in [y.floor() as i64, y.floor() as i64 + 1] {
        for column in [x.floor() as i64, x.floor() as i64 + 1] {
            let column = if wraps(&photo.photo) {
                column.rem_euclid(i64::from(photo.pixels.width))
            } else {
                column.clamp(0, i64::from(photo.pixels.width) - 1)
            };
            let row = row.clamp(0, i64::from(photo.pixels.height) - 1);
            if photo.pixels.rgba
                [(row as usize * photo.pixels.width as usize + column as usize) * 4 + 3]
                < 254
            {
                return false;
            }
        }
    }
    true
}

fn to_linear(value: f32) -> f32 {
    let value = value / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
fn from_linear(value: f32) -> f32 {
    255.0
        * if value <= 0.0031308 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        }
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScanImageFormat;

    fn surface() -> MeshGeometry {
        MeshGeometry {
            vertices: vec![[-1., -1., 0.], [1., -1., 0.], [-1., 1., 0.]],
            triangles: vec![[0, 1, 2]],
            colors: None,
            normals: None,
        }
    }
    fn camera() -> FilePhoto {
        FilePhoto {
            name: Some("calibrated".into()),
            station: None,
            position: [0., 0., 2.],
            axes: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            width: 16,
            height: 16,
            projection: PhotoProjection::Pinhole {
                focal: [8., 8.],
                principal: [7.5, 7.5],
            },
            format: ScanImageFormat::Png,
            offset: 0,
            length: 0,
        }
    }
    fn solid(rgb: [u8; 3]) -> PhotoPixels {
        PhotoPixels {
            width: 16,
            height: 16,
            rgba: (0..256)
                .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
                .collect(),
        }
    }
    fn checkerboard() -> PhotoPixels {
        let mut image = solid([0; 3]);
        for y in 0..16 {
            for x in 0..16 {
                let rgb = if (x / 4 + y / 4) % 2 == 0 {
                    [210, 30, 20]
                } else {
                    [20, 180, 60]
                };
                image.rgba[(y * 16 + x) * 4..][..3].copy_from_slice(&rgb);
            }
        }
        image
    }

    #[test]
    fn calibrated_checkerboard_projects_to_the_expected_surface_texel() {
        let mesh = surface();
        let photos = [camera()];
        let mut baker = PhotoBaker::new(&mesh, &photos, PhotoTextureOptions::default()).unwrap();
        let chart = baker.chart([0, 1, 2], &mut |_| Ok(checkerboard())).unwrap();
        // World [-0.25,-0.25,0] projects to pixel [6.5,8.5], inside a green square.
        let (rgb, kind, mask) = chart.sample([0.25, 0.375, 0.375], &mut Vec::new());
        assert_eq!(rgb, [20, 180, 60]);
        assert_eq!((kind, mask), (0, 1));
    }

    #[test]
    fn back_faces_and_occluded_surface_texels_receive_no_photo_detail() {
        let mut mesh = surface();
        let photos = [camera()];
        mesh.vertices
            .extend([[-0.5, -0.5, 1.], [0.5, -0.5, 1.], [-0.5, 0.5, 1.]]);
        mesh.triangles.push([3, 4, 5]);
        let mut baker = PhotoBaker::new(&mesh, &photos, PhotoTextureOptions::default()).unwrap();
        assert_eq!(
            baker
                .chart([0, 1, 2], &mut |_| Ok(checkerboard()))
                .unwrap()
                .sample([0.25, 0.375, 0.375], &mut Vec::new())
                .1,
            2
        );
        assert_eq!(
            baker
                .chart([3, 4, 5], &mut |_| Ok(checkerboard()))
                .unwrap()
                .sample([0.25, 0.375, 0.375], &mut Vec::new())
                .1,
            0
        );
        assert_eq!(
            baker
                .chart([2, 1, 0], &mut |_| Ok(checkerboard()))
                .unwrap()
                .sample([0.25, 0.375, 0.375], &mut Vec::new())
                .1,
            2
        );
    }

    #[test]
    fn survey_coordinates_keep_the_same_photo_projection_and_visibility() {
        let mut mesh = surface();
        let mut photo = camera();
        let shift = [155_000., 463_000., 10.];
        for point in mesh
            .vertices
            .iter_mut()
            .chain(std::iter::once(&mut photo.position))
        {
            for axis in 0..3 {
                point[axis] += shift[axis];
            }
        }
        let photos = [photo];
        let mut baker = PhotoBaker::new(&mesh, &photos, PhotoTextureOptions::default()).unwrap();
        assert_eq!(
            baker
                .chart([0, 1, 2], &mut |_| Ok(checkerboard()))
                .unwrap()
                .sample([0.25, 0.375, 0.375], &mut Vec::new())
                .0,
            [20, 180, 60]
        );
    }

    #[test]
    fn overlapping_exposures_blend_without_changing_the_source_images() {
        let mesh = surface();
        let photos = [camera(), camera()];
        let mut decodes = 0;
        let mut decode = |index| {
            decodes += 1;
            Ok(solid(if index == 0 { [80; 3] } else { [100; 3] }))
        };
        let mut baker = PhotoBaker::new(&mesh, &photos, PhotoTextureOptions::default()).unwrap();
        for _ in 0..3 {
            let (rgb, kind, mask) = baker
                .chart([0, 1, 2], &mut decode)
                .unwrap()
                .sample([0.25, 0.375, 0.375], &mut Vec::new());
            assert!(rgb.into_iter().all(|v| v.abs_diff(80) <= 1));
            assert_eq!((kind, mask), (0, 3));
        }
        assert_eq!(decodes, 2);
    }

    #[test]
    fn transparent_and_broken_photos_fall_back_to_measured_colours() {
        let mut mesh = surface();
        mesh.colors = Some(vec![[17, 23, 29]; 3]);
        let photos = [camera(), camera()];
        let mut image = solid([255, 0, 0]);
        for alpha in image.rgba.iter_mut().skip(3).step_by(4) {
            *alpha = 0;
        }
        let mut decode = |index| {
            if index == 0 {
                Err(LoadError::InvalidData("broken".into()))
            } else {
                Ok(image.clone())
            }
        };
        let mut baker = PhotoBaker::new(&mesh, &photos, PhotoTextureOptions::default()).unwrap();
        assert_eq!(
            baker
                .chart([0, 1, 2], &mut decode)
                .unwrap()
                .sample([0.25, 0.375, 0.375], &mut Vec::new()),
            ([17, 23, 29], 1, 0)
        );
        assert_eq!(baker.failed(), 1);
    }

    #[test]
    fn a_farther_usable_camera_survives_nearer_wrong_facing_images() {
        let mesh = surface();
        let mut photos = vec![camera(); 5];
        for photo in &mut photos[..4] {
            photo.axes[0] = [-1., 0., 0.];
            photo.axes[2] = [0., 0., -1.];
        }
        let mut baker = PhotoBaker::new(&mesh, &photos, PhotoTextureOptions::default()).unwrap();
        let chart = baker.chart([0, 1, 2], &mut |_| Ok(checkerboard())).unwrap();
        assert_eq!(chart.photo_indices().collect::<Vec<_>>(), [4]);
        assert_eq!(chart.sample([0.25, 0.375, 0.375], &mut Vec::new()).1, 0);
    }

    #[test]
    fn decoder_cancellation_is_propagated_without_publishing_a_fallback() {
        let mesh = surface();
        let photos = [camera()];
        let mut baker = PhotoBaker::new(&mesh, &photos, PhotoTextureOptions::default()).unwrap();
        assert!(matches!(
            baker.chart([0, 1, 2], &mut |_| Err(LoadError::Cancelled)),
            Err(LoadError::Cancelled)
        ));
    }
}

#[cfg(test)]
mod atlas_tests {
    use super::*;
    use crate::{write_photographic_obj_atlas, AtlasOptions, PhotoAtlasRequest, ScanImageFormat};
    use std::fs;

    fn mesh() -> MeshGeometry {
        MeshGeometry {
            vertices: vec![[-1., -1., 0.], [1., -1., 0.], [-1., 1., 0.]],
            triangles: vec![[0, 1, 2]],
            colors: None,
            normals: None,
        }
    }
    fn photo() -> FilePhoto {
        FilePhoto {
            name: Some("checkerboard".into()),
            station: None,
            position: [0., 0., 2.],
            axes: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            width: 16,
            height: 16,
            projection: PhotoProjection::Pinhole {
                focal: [8., 8.],
                principal: [7.5, 7.5],
            },
            format: ScanImageFormat::Png,
            offset: 0,
            length: 0,
        }
    }
    fn pixels() -> PhotoPixels {
        let rgba = (0..16)
            .flat_map(|y| {
                (0..16).flat_map(move |x| {
                    if (x / 4 + y / 4) % 2 == 0 {
                        [210, 30, 20, 255]
                    } else {
                        [20, 180, 60, 255]
                    }
                })
            })
            .collect();
        PhotoPixels {
            width: 16,
            height: 16,
            rgba,
        }
    }
    fn request<'a>(mesh: &'a MeshGeometry, photos: &'a [FilePhoto]) -> PhotoAtlasRequest<'a> {
        PhotoAtlasRequest {
            source_mesh: mesh,
            output_mesh: mesh,
            photos,
            atlas: AtlasOptions {
                texel_size: 0.02,
                max_resolution: 256,
            },
            photographs: PhotoTextureOptions::default(),
        }
    }
    fn decode_png(path: &std::path::Path) -> Vec<u8> {
        let mut reader = png::Decoder::new(fs::File::open(path).unwrap())
            .read_info()
            .unwrap();
        let mut result = vec![0; reader.output_buffer_size()];
        reader.next_frame(&mut result).unwrap();
        result
    }

    #[test]
    fn photographic_obj_has_correct_uv_pixels_and_independent_colour_coverage() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.obj");
        let mesh = mesh();
        let photos = [photo()];
        let mut decodes = 0;
        let report = write_photographic_obj_atlas(
            request(&mesh, &photos),
            &path,
            &mut |_| {
                decodes += 1;
                Ok(pixels())
            },
            &|| false,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(report.coverage.photograph_texels, 142 * 143 / 2);
        assert_eq!(report.coverage.point_colour_texels, 0);
        assert_eq!(report.coverage.unobserved_texels, 0);
        assert_eq!(report.coverage.photos_used, [0]);
        assert_eq!(decodes, 1);
        let pixels = decode_png(&directory.path().join("photo-atlas-000.png"));
        assert_eq!(&pixels[(54 * 256 + 54) * 3..][..3], &[20, 180, 60]);
        let obj = fs::read_to_string(&path).unwrap();
        assert!(obj.contains("f 1/1 2/2 3/3"));
        let reopened = crate::read_obj_mesh(&path).unwrap();
        assert_eq!(reopened.vertices, mesh.vertices);
        assert_eq!(reopened.triangles, mesh.triangles);
    }

    #[test]
    fn mirrored_output_keeps_source_photo_orientation_and_uv_corner_order() {
        let directory = tempfile::tempdir().unwrap();
        let original = mesh();
        let mut mirrored = mesh();
        for vertex in &mut mirrored.vertices {
            vertex[0] *= -2.;
        }
        mirrored.triangles[0] = [0, 2, 1];
        let photos = [photo()];
        let mut parameters = request(&original, &photos);
        parameters.output_mesh = &mirrored;
        let path = directory.path().join("mirror.obj");
        let report = write_photographic_obj_atlas(
            parameters,
            &path,
            &mut |_| Ok(pixels()),
            &|| false,
            &mut |_, _| {},
        )
        .unwrap();
        assert!(report.coverage.photograph_texels > 0);
        assert_eq!(report.coverage.unobserved_texels, 0);
        let reopened = crate::read_obj_mesh(&path).unwrap();
        assert_eq!(reopened.vertices, mirrored.vertices);
        assert_eq!(reopened.triangles, mirrored.triangles);
        let chart = report.charts[0];
        let image = decode_png(&directory.path().join("mirror-atlas-000.png"));
        // UV origin belongs to the first source corner even after reflection.
        let offset = (((chart.position[1] + 1) * 256 + chart.position[0] + 1) * 3) as usize;
        assert_eq!(&image[offset..offset + 3], &[115, 105, 40]);
    }

    #[test]
    fn hidden_mesh_face_stays_unobserved_in_the_written_atlas() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hidden.obj");
        let mut mesh = mesh();
        mesh.vertices
            .extend([[-0.5, -0.5, 1.], [0.5, -0.5, 1.], [-0.5, 0.5, 1.]]);
        mesh.triangles.push([3, 4, 5]);
        let photos = [photo()];
        let report = write_photographic_obj_atlas(
            request(&mesh, &photos),
            &path,
            &mut |_| Ok(pixels()),
            &|| false,
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(report.coverage.unobserved_texels, 142 * 143 / 2);
        assert!(report.coverage.photograph_texels > 0);
        let pixels = decode_png(&directory.path().join("hidden-atlas-000.png"));
        assert_eq!(&pixels[(54 * 256 + 54) * 3..][..3], &[128, 128, 128]);
    }

    #[test]
    fn thread_counts_keep_texture_pixels_uv_assignment_and_coverage_identical() {
        let directory = tempfile::tempdir().unwrap();
        let mesh = mesh();
        let photos = [photo()];
        let mut results = Vec::new();
        for threads in [1, 4] {
            let stem = format!("threads-{threads}");
            let path = directory.path().join(format!("{stem}.obj"));
            let result = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap()
                .install(|| {
                    write_photographic_obj_atlas(
                        request(&mesh, &photos),
                        &path,
                        &mut |_| Ok(pixels()),
                        &|| false,
                        &mut |_, _| {},
                    )
                    .unwrap()
                });
            let png = fs::read(directory.path().join(format!("{stem}-atlas-000.png"))).unwrap();
            let obj = fs::read_to_string(path)
                .unwrap()
                .lines()
                .filter(|line| line.starts_with("vt "))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            results.push((
                png,
                obj,
                result.coverage.photograph_texels,
                result.coverage.photos_used,
            ));
        }
        assert_eq!(results[0], results[1]);
    }

    #[test]
    fn cancelling_during_rasterization_leaves_no_partial_files() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cancel.obj");
        let mesh = mesh();
        let photos = [photo()];
        let calls = AtomicUsize::new(0);
        let result = write_photographic_obj_atlas(
            request(&mesh, &photos),
            &path,
            &mut |_| Ok(pixels()),
            &|| calls.fetch_add(1, Ordering::Relaxed) > 8,
            &mut |_, _| {},
        );
        assert!(matches!(result, Err(LoadError::Cancelled)));
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn an_output_created_by_another_writer_is_never_replaced() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("concurrent.obj");
        let material = path.with_extension("mtl");
        let mesh = mesh();
        let photos = [photo()];
        let result = write_photographic_obj_atlas(
            request(&mesh, &photos),
            &path,
            &mut |_| Ok(pixels()),
            &|| false,
            &mut |_, _| fs::write(&material, "another writer's material").unwrap(),
        );
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(&material).unwrap(),
            "another writer's material"
        );
        assert!(!path.exists());
        assert!(!directory.path().join("concurrent-atlas-000.png").exists());
    }
}
