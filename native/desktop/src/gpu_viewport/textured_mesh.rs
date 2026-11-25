//! UV-atlas overlay, retaining indexed source vertices instead of expanding
//! every triangle corner into another large copy of the measured geometry.

use super::{wgpu, GpuMeshVertex, MeshBuffers};
use crate::mesh_texture_preview::BakedTexture;
use std::sync::{
    atomic::{AtomicU32, AtomicU8, Ordering},
    Arc,
};
use wgpu::util::DeviceExt;

const VERTICES_PER_PART: usize = 2_000_000;
static MAX_PAGE_SIDE: AtomicU32 = AtomicU32::new(4096);
pub(crate) fn max_page_side() -> u32 {
    MAX_PAGE_SIDE.load(Ordering::Relaxed)
}
static SUPPORTED: AtomicU8 = AtomicU8::new(0);
pub(crate) fn supported() -> Option<bool> {
    match SUPPORTED.load(Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

pub(super) struct TextureUpload {
    pipeline: Option<wgpu::RenderPipeline>,
    layout: Option<wgpu::BindGroupLayout>,
    uploaded: Option<Arc<MeshBuffers>>,
    meshes: Vec<GpuTexturedMesh>,
}
struct TextureImage {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
}
struct GpuTexturedMesh {
    source: Arc<BakedTexture>,
    image: Arc<TextureImage>,
    _buffers: Vec<wgpu::Buffer>,
    group: wgpu::BindGroup,
    count: u32,
}

impl TextureUpload {
    pub(super) fn new(
        device: &wgpu::Device,
        camera: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
    ) -> Self {
        MAX_PAGE_SIDE.store(
            device.limits().max_texture_dimension_2d / 2,
            Ordering::Relaxed,
        );
        let supported = device.limits().max_storage_buffers_per_shader_stage >= 4
            && device.limits().max_storage_buffer_binding_size >= 128_000_000;
        SUPPORTED.store(if supported { 1 } else { 2 }, Ordering::Relaxed);
        if !supported {
            return Self {
                pipeline: None,
                layout: None,
                uploaded: None,
                meshes: Vec::new(),
            };
        }
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("surface texture layout"),
            entries: &[
                storage(0),
                storage(1),
                storage(2),
                storage(3),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("surface texture pipeline layout"),
            bind_group_layouts: &[camera, &layout],
            push_constant_ranges: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("surface texture shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("textured_mesh.wgsl").into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("surface texture pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview: None,
        });
        Self {
            pipeline: Some(pipeline),
            layout: Some(layout),
            uploaded: None,
            meshes: Vec::new(),
        }
    }

    pub(super) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mesh: &Arc<MeshBuffers>,
    ) {
        if self
            .uploaded
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, mesh))
        {
            return;
        }
        let Some(layout) = &self.layout else {
            self.uploaded = Some(Arc::clone(mesh));
            return;
        };
        let previous = std::mem::take(&mut self.meshes);
        for part in &mesh.textured {
            let source = &part.source;
            if source.pixels.side * 2 > device.limits().max_texture_dimension_2d
                || source.pixels.layers > device.limits().max_texture_array_layers
            {
                continue;
            }
            let image = previous
                .iter()
                .find(|old| Arc::ptr_eq(&old.source, source))
                .map(|old| Arc::clone(&old.image))
                .unwrap_or_else(|| {
                    let size = wgpu::Extent3d {
                        width: source.pixels.side * 2,
                        height: source.pixels.side,
                        depth_or_array_layers: source.pixels.layers,
                    };
                    let texture = device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("surface texture atlas"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    });
                    queue.write_texture(
                        wgpu::ImageCopyTexture {
                            texture: &texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        &source.pixels.rgba,
                        wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(source.pixels.side * 8),
                            rows_per_image: Some(source.pixels.side),
                        },
                        size,
                    );
                    let view = texture.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2Array),
                        ..Default::default()
                    });
                    Arc::new(TextureImage {
                        _texture: texture,
                        view,
                    })
                });
            let vertices = &mesh.vertices[part.first_vertex..part.first_vertex + part.vertex_count];
            let zero = [GpuMeshVertex::zeroed()];
            let first = &vertices[..vertices.len().min(VERTICES_PER_PART)];
            let second = if vertices.len() > VERTICES_PER_PART {
                &vertices[VERTICES_PER_PART..]
            } else {
                &zero
            };
            let buffer = |label, bytes: &[u8], usage| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytes,
                    usage,
                })
            };
            let buffers = vec![
                buffer(
                    "textured vertices 0",
                    bytemuck::cast_slice(first),
                    wgpu::BufferUsages::STORAGE,
                ),
                buffer(
                    "textured vertices 1",
                    bytemuck::cast_slice(second),
                    wgpu::BufferUsages::STORAGE,
                ),
                buffer(
                    "textured faces",
                    bytemuck::cast_slice(&source.faces().collect::<Vec<_>>()),
                    wgpu::BufferUsages::STORAGE,
                ),
                buffer(
                    "texture UV charts",
                    bytemuck::cast_slice(&source.charts),
                    wgpu::BufferUsages::STORAGE,
                ),
                buffer(
                    "texture settings",
                    bytemuck::cast_slice(&[source.report.max_resolution as f32, 0., 0., 0.]),
                    wgpu::BufferUsages::UNIFORM,
                ),
            ];
            let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("surface atlas sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("surface texture group"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffers[0].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: buffers[1].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffers[2].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: buffers[3].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&image.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: buffers[4].as_entire_binding(),
                    },
                ],
            });
            self.meshes.push(GpuTexturedMesh {
                source: Arc::clone(source),
                image,
                _buffers: buffers,
                group,
                count: (source.mesh.triangles.len() * 3) as u32,
            });
        }
        self.uploaded = Some(Arc::clone(mesh));
    }

    pub(super) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let Some(pipeline) = &self.pipeline else {
            return;
        };
        pass.set_pipeline(pipeline);
        for mesh in &self.meshes {
            pass.set_bind_group(1, &mesh.group, &[]);
            pass.draw(0..mesh.count, 0..1);
        }
    }
}
use bytemuck::Zeroable;

#[cfg(test)]
mod tests {
    #[test]
    fn the_native_uv_shader_is_valid_wgsl() {
        use super::wgpu::naga;
        let source = include_str!("textured_mesh.wgsl");
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap_or_else(|error| panic!("{}", error.emit_to_string(source)));
    }
}
