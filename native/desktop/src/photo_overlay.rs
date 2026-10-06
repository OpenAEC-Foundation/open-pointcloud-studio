//! The photo that is entered, laid over the points by the graphics device:
//! a pass over the whole view after the points, which looks up every pixel's
//! direction in the photo and blends the photo in.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced_wgpu::wgpu;
use pointcloud_core::PhotoProjection;

use crate::file_photos::{DecodedPhoto, ShownPhoto};

/// The photo as the shader reads it; see `photo_overlay.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GpuPhoto {
    /// The camera's X axis in the scene; w: 0 pinhole, 1 spherical, 2
    /// cylindrical.
    pub axis_x: [f32; 4],
    /// The camera's Y axis; w: how much of the photo covers the points.
    pub axis_y: [f32; 4],
    /// The camera's Z axis; w: texels of the texture per radian.
    pub axis_z: [f32; 4],
    /// Width and height as stated, whether the columns go round, and the
    /// number of levels of the texture.
    pub size: [f32; 4],
    /// Pinhole: focal length across and down, principal column and row.
    /// Spherical: angle of a pixel across and down. Cylindrical: angle
    /// across, height down, radius and principal row.
    pub lens: [f32; 4],
}

/// The photo the next frame shows over the points.
#[derive(Debug)]
pub struct PhotoFrame {
    pub decoded: Arc<DecodedPhoto>,
    pub uniform: GpuPhoto,
}

/// The photo as the shader reads it, for a texture `texture_width` pixels
/// wide with `levels` levels.
pub fn uniform(shown: &ShownPhoto<'_>, texture_width: u32, levels: u32) -> GpuPhoto {
    let photo = shown.photo;
    let width = f64::from(photo.width);
    let scale = f64::from(texture_width) / width;
    let axis =
        |axis: [f64; 3], last: f64| [axis[0] as f32, axis[1] as f32, axis[2] as f32, last as f32];
    let (kind, wraps, lens, texels) = match photo.projection {
        PhotoProjection::Pinhole { focal, principal } => (
            0.0,
            false,
            [focal[0], focal[1], principal[0], principal[1]],
            focal[0] * scale,
        ),
        PhotoProjection::Spherical { pixel_size } => (
            1.0,
            pixel_size[0] * width >= std::f64::consts::TAU * (1.0 - 1e-6),
            [pixel_size[0], pixel_size[1], 0.0, 0.0],
            scale / pixel_size[0],
        ),
        PhotoProjection::Cylindrical {
            pixel_size,
            radius,
            principal_row,
        } => (
            2.0,
            pixel_size[0] * width >= std::f64::consts::TAU * (1.0 - 1e-6),
            [pixel_size[0], pixel_size[1], radius, principal_row],
            scale / pixel_size[0],
        ),
    };
    GpuPhoto {
        axis_x: axis(shown.axes[0], kind),
        axis_y: axis(shown.axes[1], f64::from(shown.blend)),
        axis_z: axis(shown.axes[2], texels),
        size: [
            photo.width as f32,
            photo.height as f32,
            if wraps { 1.0 } else { 0.0 },
            levels as f32,
        ],
        lens: lens.map(|value| value as f32),
    }
}

/// The frame of a photo that has been decoded.
pub fn frame(shown: &ShownPhoto<'_>) -> Option<PhotoFrame> {
    let decoded = shown.decoded?;
    let base = decoded.levels.first()?;
    Some(PhotoFrame {
        uniform: uniform(shown, base.width, decoded.levels.len() as u32),
        decoded: Arc::clone(decoded),
    })
}

/// The texture coordinates the shader takes for a direction in the scene,
/// or none where the photo does not see it. It follows `place` in
/// `photo_overlay.wgsl` step by step, so that tests can hold the two to the
/// photo's own projection.
#[cfg(test)]
pub fn texture_position(photo: &GpuPhoto, direction: [f32; 3]) -> Option<[f32; 2]> {
    let dot =
        |axis: [f32; 4]| direction[0] * axis[0] + direction[1] * axis[1] + direction[2] * axis[2];
    let local = [dot(photo.axis_x), dot(photo.axis_y), dot(photo.axis_z)];
    let kind = photo.axis_x[3];
    let [width, height, wraps, _] = photo.size;
    let lens = photo.lens;
    let (column, row) = if kind < 0.5 {
        let depth = -local[2];
        if depth <= 0.0 {
            return None;
        }
        (
            lens[2] + 0.5 + lens[0] * local[0] / depth,
            lens[3] + 0.5 - lens[1] * local[1] / depth,
        )
    } else {
        let across = local[0].hypot(local[1]);
        let mut column = width * 0.5 - local[1].atan2(local[0]) / lens[0];
        if wraps > 0.5 {
            column = (column - width * (column / width).floor()).clamp(0.0, width * 0.99999);
        }
        let row = if kind < 1.5 {
            height * 0.5 - local[2].atan2(across) / lens[1]
        } else {
            if across <= 0.0 {
                return None;
            }
            lens[3] + 0.5 - lens[2] * local[2] / across / lens[1]
        };
        (column, row)
    };
    (column >= 0.0 && row >= 0.0 && column < width && row < height)
        .then_some([column / width, row / height])
}

/// A photo on the device: its texture and what binds it to the pass.
struct Uploaded {
    decoded: Arc<DecodedPhoto>,
    /// Levels of the decoded photo left out because the device takes no
    /// texture that large.
    skipped: u32,
    group: wgpu::BindGroup,
    _texture: wgpu::Texture,
}

/// The pass that lays the photo over the points, and the photo it has on
/// the device.
pub struct PhotoOverlay {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniform: wgpu::Buffer,
    /// Panoramas repeat across their seam; other photos end at their edges.
    around: wgpu::Sampler,
    edged: wgpu::Sampler,
    uploaded: Option<Uploaded>,
}

impl PhotoOverlay {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("photo overlay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("photo_overlay.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("photo overlay layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("photo overlay pipeline layout"),
            bind_group_layouts: &[camera_layout, &layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("photo overlay pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_photo",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_photo",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("photo overlay placement"),
            size: std::mem::size_of::<GpuPhoto>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sampler = |address_mode_u| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("photo overlay sampler"),
                address_mode_u,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::FilterMode::Linear,
                ..wgpu::SamplerDescriptor::default()
            })
        };
        Self {
            pipeline,
            layout,
            uniform,
            around: sampler(wgpu::AddressMode::Repeat),
            edged: sampler(wgpu::AddressMode::ClampToEdge),
            uploaded: None,
        }
    }

    /// Put the photo of the next frame on the device, once, and its
    /// placement every frame.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &PhotoFrame) {
        let wraps = frame.uniform.size[2] > 0.5;
        if self
            .uploaded
            .as_ref()
            .is_none_or(|uploaded| !Arc::ptr_eq(&uploaded.decoded, &frame.decoded))
        {
            self.uploaded = self.upload(device, queue, &frame.decoded, wraps);
        }
        let Some(uploaded) = &self.uploaded else {
            return;
        };
        // Levels left out halve the texels per radian each.
        let mut uniform = frame.uniform;
        uniform.axis_z[3] /= (1u32 << uploaded.skipped.min(31)) as f32;
        uniform.size[3] -= uploaded.skipped as f32;
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
    }

    fn upload(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        decoded: &Arc<DecodedPhoto>,
        wraps: bool,
    ) -> Option<Uploaded> {
        let limit = device.limits().max_texture_dimension_2d;
        let skipped = decoded
            .levels
            .iter()
            .position(|level| level.width <= limit && level.height <= limit)?;
        let levels = &decoded.levels[skipped..];
        let base = levels.first()?;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("photo overlay"),
            size: wgpu::Extent3d {
                width: base.width,
                height: base.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, data) in levels.iter().enumerate() {
            queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data.pixels,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(data.width * 4),
                    rows_per_image: Some(data.height),
                },
                wgpu::Extent3d {
                    width: data.width,
                    height: data.height,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("photo overlay group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(if wraps {
                        &self.around
                    } else {
                        &self.edged
                    }),
                },
            ],
        });
        Some(Uploaded {
            decoded: Arc::clone(decoded),
            skipped: skipped as u32,
            group,
            _texture: texture,
        })
    }

    /// Let go of the photo on the device once no photo is shown.
    pub fn release(&mut self) {
        self.uploaded = None;
    }

    /// Lay the photo over what the target holds.
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
        camera_group: &wgpu::BindGroup,
    ) {
        let Some(uploaded) = &self.uploaded else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("photo overlay pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera_group, &[]);
        pass.set_bind_group(1, &uploaded.group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointcloud_core::{FilePhoto, ScanImageFormat};

    /// The axes of a camera turned about the vertical, then tilted and
    /// rolled.
    fn turned(yaw: f64, pitch: f64, roll: f64) -> [[f64; 3]; 3] {
        let rotate = |vector: [f64; 3], axis: usize, angle: f64| {
            let (sin, cos) = angle.sin_cos();
            let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
            let mut turned = vector;
            turned[a] = vector[a] * cos - vector[b] * sin;
            turned[b] = vector[a] * sin + vector[b] * cos;
            turned
        };
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .map(|axis| rotate(rotate(rotate(axis, 0, roll), 1, pitch), 2, yaw))
    }

    fn photo(width: u32, height: u32, projection: PhotoProjection) -> FilePhoto {
        FilePhoto {
            name: None,
            station: None,
            position: [100_000.0, 400_000.0, 12.0],
            axes: turned(0.7, -0.3, 0.2),
            width,
            height,
            projection,
            format: ScanImageFormat::Jpeg,
            offset: 0,
            length: 1,
        }
    }

    /// The shader finds for every direction the pixel the photo's own
    /// projection gives, with the same edges.
    fn shader_follows_the_photo(photo: &FilePhoto) {
        let shown = ShownPhoto {
            photo,
            index: 0,
            count: 1,
            eye: photo.position,
            axes: photo.axes,
            decoded: None,
            blend: 0.6,
            pinned: false,
            zoom: 1.0,
        };
        let gpu = uniform(&shown, photo.width / 2, 5);
        assert_eq!(gpu.axis_y[3], 0.6);
        assert_eq!(gpu.size[3], 5.0);
        let (width, height) = (f64::from(photo.width), f64::from(photo.height));
        let mut seen = 0;
        for step in 0..2_000 {
            let azimuth = f64::from(step) * 0.731;
            let elevation = (f64::from(step) * 0.173).sin() * 1.5;
            let direction = [
                elevation.cos() * azimuth.cos(),
                elevation.cos() * azimuth.sin(),
                elevation.sin(),
            ];
            let expected = photo
                .project(direction)
                .map(|pixel| [(pixel[0] + 0.5) / width, (pixel[1] + 0.5) / height]);
            let found = texture_position(&gpu, direction.map(|value| value as f32));
            match (expected, found) {
                (Some(expected), Some(found)) => {
                    seen += 1;
                    for axis in 0..2 {
                        let difference = (expected[axis] - f64::from(found[axis])).abs();
                        // Across the seam of a panorama 0 and 1 are the same place.
                        assert!(
                            difference < 1e-4 || (difference - 1.0).abs() < 1e-4,
                            "{direction:?}: {expected:?} {found:?}"
                        );
                    }
                }
                (None, None) => {}
                // Only on an edge may the two roundings disagree.
                (Some(edge), None) => assert!(
                    edge.iter()
                        .any(|value| *value < 1e-4 || *value > 1.0 - 1e-4),
                    "{direction:?}: {edge:?}"
                ),
                (None, Some(edge)) => assert!(
                    edge.iter()
                        .any(|value| *value < 1e-4 || *value > 1.0 - 1e-4),
                    "{direction:?}: {edge:?}"
                ),
            }
        }
        assert!(seen > 100, "{seen}");
    }

    #[test]
    fn the_shader_places_every_kind_of_photo_as_the_photo_does() {
        let pinhole = photo(
            1200,
            900,
            PhotoProjection::Pinhole {
                focal: [1000.0, 1000.0],
                principal: [600.0, 450.0],
            },
        );
        shader_follows_the_photo(&pinhole);
        let panorama = photo(
            8000,
            4000,
            PhotoProjection::Spherical {
                pixel_size: [
                    std::f64::consts::TAU / 8000.0,
                    std::f64::consts::PI / 4000.0,
                ],
            },
        );
        shader_follows_the_photo(&panorama);
        let cylinder = photo(
            3600,
            1000,
            PhotoProjection::Cylindrical {
                pixel_size: [std::f64::consts::TAU / 3600.0, 0.002],
                radius: 1.0,
                principal_row: 499.5,
            },
        );
        shader_follows_the_photo(&cylinder);
        // A panorama that does not go all the way round has edges at the
        // sides as well.
        let partial = photo(
            2000,
            1000,
            PhotoProjection::Spherical {
                pixel_size: [0.002, 0.002],
            },
        );
        shader_follows_the_photo(&partial);
    }

    #[test]
    fn the_level_of_detail_follows_the_texels_per_radian() {
        let pinhole = photo(
            1200,
            900,
            PhotoProjection::Pinhole {
                focal: [1000.0, 1000.0],
                principal: [599.5, 449.5],
            },
        );
        let shown = |photo| ShownPhoto {
            photo,
            index: 0,
            count: 1,
            eye: [0.0; 3],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            decoded: None,
            blend: 1.0,
            pinned: true,
            zoom: 1.0,
        };
        // A texture of half the width has half the texels per radian.
        assert_eq!(uniform(&shown(&pinhole), 600, 11).axis_z[3], 500.0);
        let panorama = photo(
            4000,
            2000,
            PhotoProjection::Spherical {
                pixel_size: [
                    std::f64::consts::TAU / 4000.0,
                    std::f64::consts::PI / 2000.0,
                ],
            },
        );
        let gpu = uniform(&shown(&panorama), 2000, 12);
        assert!((f64::from(gpu.axis_z[3]) - 2000.0 / std::f64::consts::TAU).abs() < 1e-3);
        assert_eq!(gpu.size[2], 1.0);
        assert_eq!(gpu.axis_x[3], 1.0);
    }
}
