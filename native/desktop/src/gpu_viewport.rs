//! Native WGPU point-sprite renderer embedded in Iced's shader widget.
//! Survey coordinates are rebased in f64 before f32 upload to retain precision.

use std::cell::RefCell;
use std::sync::Arc;

use crate::selection::{ClassVisibility, DeletionMask};
use crate::station_photos::{self, PhotoAtlas, PhotoSet};
use crate::CloudTransform;
use crate::{combined_bounds, CloudEntry, ColorMode, Message, PointViewport};
use bytemuck::{Pod, Zeroable};
use iced::mouse;
use iced::widget::shader::{self, Shader};
use iced::Rectangle;
use iced_wgpu::primitive::{Primitive, Storage};
use iced_wgpu::wgpu;
use pointcloud_core::{Bounds, IndexedPoint, MeshGeometry, PointCloud};

// Keep each upload below conservative WGPU adapter buffer limits. The full
// viewport budget can span multiple draw calls without losing detail.
const POINTS_PER_BUFFER: usize = 2_000_000;

// At close range the fixed-size sprites become single-pixel specks even when
// the octree supplies exact points. Grow their screen radius gently so the
// hemisphere lighting remains legible, without making large user sizes explode.
pub(crate) fn display_point_radius(point_size: f32, zoom: f32) -> f32 {
    let close_up = (-zoom.max(0.000_001).log10() * 1.2).clamp(0.0, 4.0);
    point_size + close_up
}

fn derived_mesh_normals(mesh: &MeshGeometry) -> Vec<[f32; 3]> {
    let mut normals = vec![[0.0_f64; 3]; mesh.vertices.len()];
    for &[a, b, c] in &mesh.triangles {
        let (Some(&pa), Some(&pb), Some(&pc)) = (
            mesh.vertices.get(a as usize),
            mesh.vertices.get(b as usize),
            mesh.vertices.get(c as usize),
        ) else {
            continue;
        };
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let normal = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for index in [a, b, c] {
            for axis in 0..3 {
                normals[index as usize][axis] += normal[axis];
            }
        }
    }
    normals
        .into_iter()
        .map(|normal| {
            let length = normal.iter().map(|value| value * value).sum::<f64>().sqrt();
            if length.is_finite() && length > f64::EPSILON {
                normal.map(|value| (value / length) as f32)
            } else {
                [0.0; 3]
            }
        })
        .collect()
}

#[derive(Clone, Copy)]
pub struct GpuViewport<'a> {
    pub overlay: PointViewport<'a>,
}

#[derive(Default)]
pub struct RenderCache {
    key: Option<SceneKey>,
    geometry: Option<Arc<RenderGeometry>>,
}

struct SceneKey {
    clouds: Vec<CloudKey>,
    bounds: Option<Bounds>,
    section: Option<Bounds>,
    color_mode: ColorMode,
    budget: usize,
    filters: [bool; 4],
    class_visibility: ClassVisibility,
}

struct CloudKey {
    source: Arc<PointCloud>,
    transform: CloudTransform,
    detail: Option<Arc<[IndexedPoint]>>,
    deleted: Option<Arc<DeletionMask>>,
    mesh: Option<Arc<MeshGeometry>>,
    visible: bool,
    mesh_visible: bool,
}

fn same_arc<T: ?Sized>(left: &Option<Arc<T>>, right: &Option<Arc<T>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

impl SceneKey {
    fn capture(view: PointViewport<'_>, bounds: Option<Bounds>) -> Self {
        Self {
            clouds: view
                .clouds
                .iter()
                .map(|entry| CloudKey {
                    source: Arc::clone(&entry.cloud),
                    transform: entry.transform,
                    detail: entry.detail_points.clone(),
                    deleted: entry.deleted.clone(),
                    mesh: entry.mesh.clone(),
                    visible: entry.visible,
                    mesh_visible: entry.mesh_visible,
                })
                .collect(),
            bounds,
            section: view.section,
            color_mode: view.color_mode,
            budget: view.budget,
            filters: [
                view.filter_ground,
                view.filter_vegetation,
                view.filter_buildings,
                view.filter_other,
            ],
            class_visibility: view.class_visibility,
        }
    }

    fn matches(&self, view: PointViewport<'_>, bounds: Option<Bounds>) -> bool {
        self.bounds == bounds
            && self.section == view.section
            && self.color_mode == view.color_mode
            && self.budget == view.budget
            && self.filters
                == [
                    view.filter_ground,
                    view.filter_vegetation,
                    view.filter_buildings,
                    view.filter_other,
                ]
            && self.class_visibility == view.class_visibility
            && self.clouds.len() == view.clouds.len()
            && self
                .clouds
                .iter()
                .zip(view.clouds)
                .all(|(key, entry)| key.matches(entry))
    }
}

impl CloudKey {
    fn matches(&self, entry: &CloudEntry) -> bool {
        Arc::ptr_eq(&self.source, &entry.cloud)
            && self.transform == entry.transform
            && same_arc(&self.detail, &entry.detail_points)
            && same_arc(&self.deleted, &entry.deleted)
            && same_arc(&self.mesh, &entry.mesh)
            && self.visible == entry.visible
            && self.mesh_visible == entry.mesh_visible
    }
}

impl<'a> GpuViewport<'a> {
    pub fn widget(self) -> Shader<Message, Self> {
        Shader::new(self)
    }

    fn build_geometry(&self, overall_bounds: Option<Bounds>) -> RenderGeometry {
        let mut points = Vec::new();
        let mut mesh_vertices = Vec::new();
        let mut mesh_indices = Vec::new();
        if let Some(overall_bounds) = overall_bounds {
            let center = overall_bounds.center();
            let sampled: usize = self
                .overlay
                .clouds
                .iter()
                .filter(|entry| entry.visible)
                .map(CloudEntry::view_len)
                .sum();
            let stride = sampled.div_ceil(self.overlay.budget.max(1)).max(1);
            points.reserve(sampled.div_ceil(stride));
            for record in self
                .overlay
                .clouds
                .iter()
                .filter(|entry| entry.visible)
                .flat_map(|entry| {
                    entry
                        .view_records()
                        .filter(move |record| entry.record_visible(*record))
                })
                .step_by(stride)
            {
                let point = &record.point;
                if !self.overlay.accepts(point) {
                    continue;
                }
                let color = self.overlay.color(point, overall_bounds);
                points.push(GpuPoint {
                    relative: [
                        (point.xyz[0] - center[0]) as f32,
                        (point.xyz[1] - center[1]) as f32,
                        (point.xyz[2] - center[2]) as f32,
                        0.0,
                    ],
                    color: [color.r, color.g, color.b, color.a],
                });
            }
            for entry in self
                .overlay
                .clouds
                .iter()
                .filter(|entry| entry.mesh_visible)
            {
                let Some(mesh) = entry.mesh.as_deref() else {
                    continue;
                };
                let Ok(base) = u32::try_from(mesh_vertices.len()) else {
                    break;
                };
                if mesh_vertices.len() + mesh.vertices.len() > u32::MAX as usize
                    || mesh_indices.len() + mesh.triangles.len() * 3 > u32::MAX as usize
                {
                    break;
                }
                mesh_vertices.reserve(mesh.vertices.len());
                let derived_normals = mesh
                    .normals
                    .as_ref()
                    .is_none_or(|normals| normals.len() != mesh.vertices.len())
                    .then(|| derived_mesh_normals(mesh));
                let normals = mesh
                    .normals
                    .as_deref()
                    .filter(|normals| normals.len() == mesh.vertices.len())
                    .or(derived_normals.as_deref())
                    .expect("mesh normals available");
                for (index, xyz) in mesh.vertices.iter().enumerate() {
                    let xyz = entry.transform.xyz(*xyz);
                    let color = mesh
                        .colors
                        .as_ref()
                        .and_then(|colors| colors.get(index))
                        .map_or([0.56, 0.55, 0.51, 0.82], |rgb| {
                            [
                                f32::from(rgb[0]) / 255.0,
                                f32::from(rgb[1]) / 255.0,
                                f32::from(rgb[2]) / 255.0,
                                0.82,
                            ]
                        });
                    let normal = entry.transform.normal(normals[index]).unwrap_or([0.0; 3]);
                    mesh_vertices.push(GpuMeshVertex {
                        relative: [
                            (xyz[0] - center[0]) as f32,
                            (xyz[1] - center[1]) as f32,
                            (xyz[2] - center[2]) as f32,
                            0.0,
                        ],
                        color,
                        normal: [normal[0], normal[1], normal[2], 0.0],
                    });
                }
                mesh_indices.reserve(mesh.triangles.len() * 3);
                for face in &mesh.triangles {
                    mesh_indices.extend(face.map(|index| index + base));
                }
            }
        }
        RenderGeometry {
            points,
            mesh_vertices,
            mesh_indices,
        }
    }
}

impl shader::Program<Message> for GpuViewport<'_> {
    type State = RefCell<RenderCache>;
    type Primitive = CloudPrimitive;

    fn draw(
        &self,
        state: &Self::State,
        _cursor: mouse::Cursor,
        bounds: Rectangle,
    ) -> Self::Primitive {
        let overall_bounds = combined_bounds(self.overlay.clouds);
        let geometry = {
            let mut cache = state.borrow_mut();
            if !cache
                .key
                .as_ref()
                .is_some_and(|key| key.matches(self.overlay, overall_bounds))
            {
                cache.geometry = Some(Arc::new(self.build_geometry(overall_bounds)));
                cache.key = Some(SceneKey::capture(self.overlay, overall_bounds));
            }
            Arc::clone(cache.geometry.as_ref().expect("render geometry cached"))
        };

        let mut camera = CameraUniform::zeroed();
        if let Some(overall_bounds) = overall_bounds {
            let projection = self
                .overlay
                .projection(overall_bounds, bounds.width, bounds.height);
            let center = overall_bounds.center();
            // The eye position rides along in the spare components.
            let axis = |direction: [f64; 3], eye: f64| {
                [
                    direction[0] as f32,
                    direction[1] as f32,
                    direction[2] as f32,
                    eye as f32,
                ]
            };
            camera.right = axis(projection.right, projection.eye[0]);
            camera.up = axis(projection.up, projection.eye[1]);
            camera.toward = axis(projection.toward_camera, projection.eye[2]);
            camera.projection = [
                bounds.width,
                bounds.height,
                projection.scale as f32,
                projection.distance as f32,
            ];
            camera.view = match self.overlay.walk {
                // Walking is always close to the points: use the close-up size.
                Some(_) => [
                    0.0,
                    0.0,
                    display_point_radius(self.overlay.point_size, 0.05),
                    1.0,
                ],
                None => [
                    self.overlay.pan[0],
                    self.overlay.pan[1],
                    display_point_radius(self.overlay.point_size, self.overlay.zoom),
                    1.0,
                ],
            };
            camera.clip_enabled[1] = if self.overlay.eye_dome { 1.0 } else { 0.0 };
            camera.clip_enabled[2] = self.overlay.eye_dome_strength;
            if let Some(section) = self.overlay.section {
                camera.clip_min = [
                    (section.min[0] - center[0]) as f32,
                    (section.min[1] - center[1]) as f32,
                    (section.min[2] - center[2]) as f32,
                    0.0,
                ];
                camera.clip_max = [
                    (section.max[0] - center[0]) as f32,
                    (section.max[1] - center[1]) as f32,
                    (section.max[2] - center[2]) as f32,
                    0.0,
                ];
                camera.clip_enabled[0] = 1.0;
            }
        }
        let mut photos = PhotoFrame {
            atlas: self.overlay.photo_atlas.cloned(),
            ..PhotoFrame::default()
        };
        if let (Some(view), Some((cloud, station))) = (self.overlay.walk, self.overlay.walk_station)
        {
            // Standing in a station: only its photos are drawn.
            let [right, up, forward] = view.basis();
            camera = CameraUniform::zeroed();
            camera.right = vec4(right);
            camera.up = vec4(up);
            camera.toward = vec4(forward.map(|value| -value));
            camera.projection = [bounds.width, bounds.height, view.focal(bounds.size()), 1.0];
            camera.view = [0.0, 0.0, 0.0, 1.0];
            let full = self
                .overlay
                .panorama_photos
                .filter(|set| !set.faces.is_empty())
                .cloned();
            let slot = match &full {
                Some(set) => Some((0, set.faces.len() as u32)),
                None => self
                    .overlay
                    .clouds
                    .get(cloud)
                    .zip(photos.atlas.as_deref())
                    .and_then(|(entry, atlas)| atlas.slot(&entry.cloud.path, station)),
            };
            if let Some((first, count)) = slot {
                camera.clip_min[3] = first as f32;
                camera.clip_max[3] = count as f32;
            }
            photos.panorama = Some(PanoramaFrame {
                full,
                visible: slot.is_some(),
            });
        } else if let (true, Some(overall_bounds), Some(atlas)) = (
            self.overlay.show_scan_poses,
            overall_bounds,
            photos.atlas.as_deref(),
        ) {
            let center = overall_bounds.center();
            for entry in self.overlay.clouds.iter().filter(|entry| entry.visible) {
                for (station, pose) in entry.cloud.scan_poses.iter().enumerate() {
                    let Some((first, count)) = atlas.slot(&entry.cloud.path, station) else {
                        continue;
                    };
                    let xyz = entry.transform.xyz(pose.position);
                    photos.balls.push(GpuBall {
                        placement: [
                            (xyz[0] - center[0]) as f32,
                            (xyz[1] - center[1]) as f32,
                            (xyz[2] - center[2]) as f32,
                            station_photos::BALL_RADIUS as f32,
                        ],
                        photos: [
                            first as f32,
                            count as f32,
                            station_photos::BALL_MIN_PIXELS,
                            station_photos::BALL_MAX_PIXELS,
                        ],
                    });
                }
            }
            photos.balls.truncate(MAX_BALLS);
        }
        CloudPrimitive {
            geometry,
            camera,
            photos,
        }
    }
}

fn vec4(value: [f64; 3]) -> [f32; 4] {
    [value[0] as f32, value[1] as f32, value[2] as f32, 0.0]
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuPoint {
    relative: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuMeshVertex {
    relative: [f32; 4],
    color: [f32; 4],
    normal: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct CameraUniform {
    right: [f32; 4],
    up: [f32; 4],
    toward: [f32; 4],
    projection: [f32; 4],
    view: [f32; 4],
    target: [f32; 4],
    clip_min: [f32; 4],
    clip_max: [f32; 4],
    clip_enabled: [f32; 4],
}

#[derive(Debug)]
struct RenderGeometry {
    points: Vec<GpuPoint>,
    mesh_vertices: Vec<GpuMeshVertex>,
    mesh_indices: Vec<u32>,
}

struct PointBufferChunk {
    buffer: wgpu::Buffer,
    capacity: u64,
    count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct GpuBall {
    /// Centre relative to the scene centre, and the ball radius in scene units.
    placement: [f32; 4],
    /// First photo, photo count, smallest and largest radius in pixels.
    photos: [f32; 4],
}

/// Placement of one photo for the shader, one entry per texture layer.
type GpuFace = [[f32; 4]; 3];

const MAX_BALLS: usize = station_photos::MAX_BALL_PHOTOS;
const FACE_TABLE_LENGTH: usize = 256;

#[derive(Debug, Default)]
struct PhotoFrame {
    atlas: Option<Arc<PhotoAtlas>>,
    balls: Vec<GpuBall>,
    panorama: Option<PanoramaFrame>,
}

#[derive(Debug)]
struct PanoramaFrame {
    /// Full-size photos of the station; the ball photos stand in until loaded.
    full: Option<Arc<PhotoSet>>,
    visible: bool,
}

#[derive(Debug)]
pub struct CloudPrimitive {
    geometry: Arc<RenderGeometry>,
    camera: CameraUniform,
    photos: PhotoFrame,
}

struct PhotoTexture {
    texture: wgpu::Texture,
    group: wgpu::BindGroup,
    faces: wgpu::Buffer,
    size: u32,
    layers: u32,
}

impl PhotoTexture {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        size: u32,
        layers: u32,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("station photos"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..wgpu::TextureViewDescriptor::default()
        });
        let faces = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("station photo placement"),
            size: (FACE_TABLE_LENGTH * std::mem::size_of::<GpuFace>()) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("station photo group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: faces.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        Self {
            texture,
            group,
            faces,
            size,
            layers,
        }
    }

    fn write_layer(&self, queue: &wgpu::Queue, layer: u32, pixels: &[u8]) {
        queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            pixels,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(self.size * 4),
                rows_per_image: Some(self.size),
            },
            wgpu::Extent3d {
                width: self.size,
                height: self.size,
                depth_or_array_layers: 1,
            },
        );
    }

    fn write_faces(&self, queue: &wgpu::Queue, faces: &[GpuFace]) {
        let count = faces.len().min(FACE_TABLE_LENGTH);
        queue.write_buffer(&self.faces, 0, bytemuck::cast_slice(&faces[..count]));
    }
}

struct GpuState {
    pipeline: wgpu::RenderPipeline,
    mesh_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    ball_pipeline: wgpu::RenderPipeline,
    sky_pipeline: wgpu::RenderPipeline,
    photo_layout: wgpu::BindGroupLayout,
    photo_sampler: wgpu::Sampler,
    ball_photos: Option<PhotoTexture>,
    uploaded_ball_sets: Vec<Arc<PhotoSet>>,
    panorama_photos: Option<PhotoTexture>,
    uploaded_panorama: Option<Arc<PhotoSet>>,
    ball_buffer: wgpu::Buffer,
    scene_layout: wgpu::BindGroupLayout,
    scene_group: Option<wgpu::BindGroup>,
    camera_buffer: wgpu::Buffer,
    camera_group: wgpu::BindGroup,
    point_buffers: Vec<PointBufferChunk>,
    mesh_vertex_buffer: wgpu::Buffer,
    mesh_vertex_capacity: u64,
    mesh_index_buffer: wgpu::Buffer,
    mesh_index_capacity: u64,
    mesh_index_count: u32,
    uploaded_geometry: Option<Arc<RenderGeometry>>,
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    color_texture: Option<wgpu::Texture>,
    color_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
}

impl GpuState {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let scene_format = wgpu::TextureFormat::Rgba8Unorm;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("pointcloud sprites"),
            source: wgpu::ShaderSource::Wgsl(include_str!("points.wgsl").into()),
        });
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pointcloud camera layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("pointcloud camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pointcloud camera group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pointcloud pipeline layout"),
            bind_group_layouts: &[&camera_layout],
            push_constant_ranges: &[],
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let mesh_attributes =
            wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pointcloud sprite pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuPoint>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_point",
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain mesh pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_mesh",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuMeshVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &mesh_attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("pointcloud scene textures layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pointcloud eye-dome layout"),
            bind_group_layouts: &[&camera_layout, &scene_layout],
            push_constant_ranges: &[],
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pointcloud eye-dome pipeline"),
            layout: Some(&composite_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_composite",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_composite",
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
        let photo_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("station photos"),
            source: wgpu::ShaderSource::Wgsl(include_str!("photos.wgsl").into()),
        });
        let photo_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("station photo layout"),
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
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
        let photo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("station photo sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        let photo_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("station photo pipeline layout"),
                bind_group_layouts: &[&camera_layout, &photo_layout],
                push_constant_ranges: &[],
            });
        let ball_attributes = wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4];
        let ball_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("station ball pipeline"),
            layout: Some(&photo_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &photo_shader,
                entry_point: "vs_ball",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<GpuBall>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &ball_attributes,
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &photo_shader,
                entry_point: "fs_ball",
                targets: &[Some(wgpu::ColorTargetState {
                    format: scene_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..wgpu::PrimitiveState::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth24Plus,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::LessEqual,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("station panorama pipeline"),
            layout: Some(&photo_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &photo_shader,
                entry_point: "vs_sky",
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &photo_shader,
                entry_point: "fs_sky",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        let ball_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("station balls"),
            size: (MAX_BALLS * std::mem::size_of::<GpuBall>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mesh_vertex_capacity = std::mem::size_of::<GpuMeshVertex>() as u64;
        let mesh_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain mesh vertices"),
            size: mesh_vertex_capacity,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mesh_index_capacity = std::mem::size_of::<u32>() as u64;
        let mesh_index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terrain mesh indices"),
            size: mesh_index_capacity,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            mesh_pipeline,
            composite_pipeline,
            ball_pipeline,
            sky_pipeline,
            photo_layout,
            photo_sampler,
            ball_photos: None,
            uploaded_ball_sets: Vec::new(),
            panorama_photos: None,
            uploaded_panorama: None,
            ball_buffer,
            scene_layout,
            scene_group: None,
            camera_buffer,
            camera_group,
            point_buffers: Vec::new(),
            mesh_vertex_buffer,
            mesh_vertex_capacity,
            mesh_index_buffer,
            mesh_index_capacity,
            mesh_index_count: 0,
            uploaded_geometry: None,
            depth_texture: None,
            depth_view: None,
            color_texture: None,
            color_view: None,
            depth_size: (0, 0),
        }
    }

    /// Upload ball photos that are not on the GPU yet. Sets keep their order,
    /// so everything before the first difference stays in place.
    fn upload_ball_photos(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: &PhotoAtlas,
    ) {
        let mut kept = self
            .uploaded_ball_sets
            .iter()
            .zip(&atlas.sets)
            .take_while(|(uploaded, wanted)| Arc::ptr_eq(uploaded, wanted))
            .count();
        if kept == atlas.sets.len() && kept == self.uploaded_ball_sets.len() {
            return;
        }
        let layers: usize = atlas.sets.iter().map(|set| set.faces.len()).sum();
        if self
            .ball_photos
            .as_ref()
            .is_none_or(|photos| (photos.layers as usize) < layers)
        {
            // Grow in steps so a project that opens scan by scan re-uploads rarely.
            let capacity = layers.max(48).next_power_of_two().min(FACE_TABLE_LENGTH);
            self.ball_photos = Some(PhotoTexture::new(
                device,
                &self.photo_layout,
                &self.photo_sampler,
                station_photos::BALL_PHOTO_SIZE,
                capacity as u32,
            ));
            kept = 0;
        }
        let photos = self.ball_photos.as_ref().expect("ball photo texture");
        let mut table = Vec::with_capacity(layers);
        for (index, set) in atlas.sets.iter().enumerate() {
            let first = atlas.first_face(index);
            for (face, image) in set.faces.iter().enumerate() {
                let layer = first + face as u32;
                if index >= kept {
                    photos.write_layer(queue, layer, set.layer(face));
                }
                table.push(station_photos::face_uniform(image, layer));
            }
        }
        photos.write_faces(queue, &table);
        self.uploaded_ball_sets = atlas.sets.clone();
    }

    fn upload_panorama(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, set: &Arc<PhotoSet>) {
        if self
            .uploaded_panorama
            .as_ref()
            .is_some_and(|uploaded| Arc::ptr_eq(uploaded, set))
        {
            return;
        }
        let layers = set.faces.len() as u32;
        if self
            .panorama_photos
            .as_ref()
            .is_none_or(|photos| photos.size != set.size || photos.layers != layers)
        {
            self.panorama_photos = Some(PhotoTexture::new(
                device,
                &self.photo_layout,
                &self.photo_sampler,
                set.size,
                layers,
            ));
        }
        let photos = self.panorama_photos.as_ref().expect("panorama texture");
        let table: Vec<GpuFace> = set
            .faces
            .iter()
            .enumerate()
            .map(|(face, image)| {
                photos.write_layer(queue, face as u32, set.layer(face));
                station_photos::face_uniform(image, face as u32)
            })
            .collect();
        photos.write_faces(queue, &table);
        self.uploaded_panorama = Some(Arc::clone(set));
    }

    fn resize_depth(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.depth_size == size {
            return;
        }
        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pointcloud scene color"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = color_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("pointcloud depth"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth24Plus,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pointcloud scene textures"),
            layout: &self.scene_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
            ],
        });
        self.color_texture = Some(color_texture);
        self.color_view = Some(color_view);
        self.depth_texture = Some(texture);
        self.depth_view = Some(view);
        self.scene_group = Some(scene_group);
        self.depth_size = size;
    }
}

impl Primitive for CloudPrimitive {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        storage: &mut Storage,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        if !storage.has::<GpuState>() {
            storage.store(GpuState::new(device, format));
        }
        let state = storage.get_mut::<GpuState>().expect("pointcloud GPU state");
        if state
            .uploaded_geometry
            .as_ref()
            .is_none_or(|previous| !Arc::ptr_eq(previous, &self.geometry))
        {
            let geometry = &self.geometry;
            for (index, points) in geometry.points.chunks(POINTS_PER_BUFFER).enumerate() {
                let byte_count = std::mem::size_of_val(points) as u64;
                let capacity = byte_count.next_power_of_two();
                if index == state.point_buffers.len() {
                    state.point_buffers.push(PointBufferChunk {
                        buffer: device.create_buffer(&wgpu::BufferDescriptor {
                            label: Some("pointcloud points"),
                            size: capacity,
                            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        }),
                        capacity,
                        count: 0,
                    });
                }
                let chunk = &mut state.point_buffers[index];
                if byte_count > chunk.capacity {
                    chunk.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("pointcloud points"),
                        size: capacity,
                        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    chunk.capacity = capacity;
                }
                queue.write_buffer(&chunk.buffer, 0, bytemuck::cast_slice(points));
                chunk.count = points.len() as u32;
            }
            state
                .point_buffers
                .truncate(geometry.points.len().div_ceil(POINTS_PER_BUFFER));
            let vertex_bytes =
                (geometry.mesh_vertices.len() * std::mem::size_of::<GpuMeshVertex>()) as u64;
            if vertex_bytes > state.mesh_vertex_capacity {
                state.mesh_vertex_capacity = vertex_bytes.next_power_of_two();
                state.mesh_vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("terrain mesh vertices"),
                    size: state.mesh_vertex_capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            if !geometry.mesh_vertices.is_empty() {
                queue.write_buffer(
                    &state.mesh_vertex_buffer,
                    0,
                    bytemuck::cast_slice(&geometry.mesh_vertices),
                );
            }
            let index_bytes = (geometry.mesh_indices.len() * std::mem::size_of::<u32>()) as u64;
            if index_bytes > state.mesh_index_capacity {
                state.mesh_index_capacity = index_bytes.next_power_of_two();
                state.mesh_index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("terrain mesh indices"),
                    size: state.mesh_index_capacity,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            if !geometry.mesh_indices.is_empty() {
                queue.write_buffer(
                    &state.mesh_index_buffer,
                    0,
                    bytemuck::cast_slice(&geometry.mesh_indices),
                );
            }
            state.mesh_index_count = geometry.mesh_indices.len() as u32;
            state.uploaded_geometry = Some(Arc::clone(&self.geometry));
        }
        if let Some(atlas) = &self.photos.atlas {
            state.upload_ball_photos(device, queue, atlas);
        }
        match self.photos.panorama.as_ref().map(|frame| &frame.full) {
            Some(Some(set)) => state.upload_panorama(device, queue, set),
            // Release the large photos once the station view is left.
            None => {
                state.panorama_photos = None;
                state.uploaded_panorama = None;
            }
            Some(None) => {}
        }
        if !self.photos.balls.is_empty() {
            queue.write_buffer(
                &state.ball_buffer,
                0,
                bytemuck::cast_slice(&self.photos.balls),
            );
        }
        let size = viewport.physical_size();
        state.resize_depth(device, (size.width, size.height));
        let mut camera = self.camera;
        camera.view[3] = viewport.scale_factor() as f32;
        camera.target = [bounds.x, bounds.y, size.width as f32, size.height as f32];
        camera.clip_enabled[3] = if format.is_srgb() { 1.0 } else { 0.0 };
        queue.write_buffer(&state.camera_buffer, 0, bytemuck::bytes_of(&camera));
    }

    fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        storage: &Storage,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let state = storage.get::<GpuState>().expect("pointcloud GPU state");
        if clip_bounds.width == 0 || clip_bounds.height == 0 {
            return;
        }
        if let Some(panorama) = &self.photos.panorama {
            let photos = match &panorama.full {
                Some(set)
                    if state
                        .uploaded_panorama
                        .as_ref()
                        .is_some_and(|uploaded| Arc::ptr_eq(uploaded, set)) =>
                {
                    state.panorama_photos.as_ref()
                }
                Some(_) => None,
                None => state.ball_photos.as_ref(),
            };
            let (true, Some(photos)) = (panorama.visible, photos) else {
                return;
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("station panorama pass"),
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
            pass.set_pipeline(&state.sky_pipeline);
            pass.set_bind_group(0, &state.camera_group, &[]);
            pass.set_bind_group(1, &photos.group, &[]);
            pass.draw(0..3, 0..1);
            return;
        }
        let balls = state
            .ball_photos
            .as_ref()
            .filter(|_| !self.photos.balls.is_empty());
        if state.point_buffers.is_empty() && state.mesh_index_count == 0 && balls.is_none() {
            return;
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("pointcloud scene pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: state.color_view.as_ref().expect("pointcloud color view"),
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: state.depth_view.as_ref().expect("pointcloud depth view"),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_scissor_rect(
                clip_bounds.x,
                clip_bounds.y,
                clip_bounds.width,
                clip_bounds.height,
            );
            pass.set_bind_group(0, &state.camera_group, &[]);
            if state.mesh_index_count > 0 {
                pass.set_pipeline(&state.mesh_pipeline);
                pass.set_vertex_buffer(0, state.mesh_vertex_buffer.slice(..));
                pass.set_index_buffer(state.mesh_index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..state.mesh_index_count, 0, 0..1);
            }
            if !state.point_buffers.is_empty() {
                pass.set_pipeline(&state.pipeline);
                for chunk in &state.point_buffers {
                    pass.set_vertex_buffer(0, chunk.buffer.slice(..));
                    pass.draw(0..6, 0..chunk.count);
                }
            }
            if let Some(photos) = balls {
                pass.set_pipeline(&state.ball_pipeline);
                pass.set_bind_group(1, &photos.group, &[]);
                pass.set_vertex_buffer(0, state.ball_buffer.slice(..));
                pass.draw(0..6, 0..self.photos.balls.len() as u32);
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("pointcloud eye-dome pass"),
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
        pass.set_pipeline(&state.composite_pipeline);
        pass.set_bind_group(0, &state.camera_group, &[]);
        pass.set_bind_group(
            1,
            state.scene_group.as_ref().expect("pointcloud scene group"),
            &[],
        );
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Studio;

    #[test]
    fn close_up_spheres_grow_without_overriding_point_size() {
        assert_eq!(display_point_radius(2.0, 1.0), 2.0);
        assert_eq!(display_point_radius(8.0, 2.0), 8.0);
        assert!(display_point_radius(2.0, 1.0 / 270.0) > 4.0);
        assert_eq!(display_point_radius(2.0, 0.000_001), 6.0);
        assert_eq!(display_point_radius(8.0, 0.000_001), 12.0);
    }

    #[test]
    fn camera_redraw_reuses_geometry_and_data_changes_invalidate_it() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("camera.xyz");
        std::fs::write(&source, "0 0 0\n1 0 0\n0 1 0\n1 1 1\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 4).unwrap());
        let mut studio = Studio::default();
        studio.clouds.push(CloudEntry {
            load_identity: Arc::clone(&cloud),
            index_import_id: None,
            cloud,
            transform: CloudTransform::default(),
            centroid_cache: None,
            mesh: None,
            mesh_visible: false,
            bag_source: false,
            visible: true,
            selection: None,
            deleted: None,
            index: None,
            auto_index_queued: false,
            index_building: false,
            detail_points: None,
        });
        let state = RefCell::new(RenderCache::default());
        let bounds = Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let draw = |studio: &Studio| {
            let viewport = GpuViewport {
                overlay: studio.point_viewport(),
            };
            shader::Program::draw(&viewport, &state, mouse::Cursor::Unavailable, bounds)
        };

        let first = draw(&studio);
        studio.yaw += 0.2;
        studio.pan[0] += 30.0;
        let camera_moved = draw(&studio);
        assert!(Arc::ptr_eq(&first.geometry, &camera_moved.geometry));
        assert_ne!(first.camera.right, camera_moved.camera.right);

        studio.zoom = 1.0 / 270.0;
        let close_up = draw(&studio);
        assert!(Arc::ptr_eq(&camera_moved.geometry, &close_up.geometry));
        assert!(close_up.camera.view[2] > camera_moved.camera.view[2]);

        studio.point_size = 4.0;
        studio.eye_dome = false;
        let display_changed = draw(&studio);
        assert!(Arc::ptr_eq(
            &camera_moved.geometry,
            &display_changed.geometry
        ));
        assert_ne!(camera_moved.camera.view, display_changed.camera.view);

        studio.eye_dome = true;
        studio.eye_dome_strength = 3.0;
        let depth_changed = draw(&studio);
        assert!(Arc::ptr_eq(
            &display_changed.geometry,
            &depth_changed.geometry
        ));
        assert_eq!(depth_changed.camera.clip_enabled[1], 1.0);
        assert_eq!(depth_changed.camera.clip_enabled[2], 3.0);

        studio.color_mode = ColorMode::Elevation;
        let recolored = draw(&studio);
        assert!(!Arc::ptr_eq(&depth_changed.geometry, &recolored.geometry));

        studio.filter_other = false;
        let filtered = draw(&studio);
        assert!(!Arc::ptr_eq(&recolored.geometry, &filtered.geometry));
        assert!(filtered.geometry.points.is_empty());

        studio.filter_other = true;
        studio.section_enabled = true;
        studio.section_reference_bounds = Some(studio.clouds[0].cloud.bounds);
        studio.section_max_percent[0] = 0.0;
        let clipped = draw(&studio);
        assert!(!Arc::ptr_eq(&filtered.geometry, &clipped.geometry));
        assert_eq!(clipped.geometry.points.len(), 2);

        studio.section_enabled = false;
        studio.clouds[0].detail_points = Some(
            vec![IndexedPoint {
                point: studio.clouds[0].cloud.points[0],
                ordinal: 0,
            }]
            .into(),
        );
        let detailed = draw(&studio);
        assert!(!Arc::ptr_eq(&clipped.geometry, &detailed.geometry));
        assert_eq!(detailed.geometry.points.len(), 1);

        studio.clouds[0].mesh = Some(Arc::new(MeshGeometry {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            colors: Some(vec![[255, 0, 0], [0, 128, 0], [0, 0, 255]]),
            normals: None,
        }));
        studio.clouds[0].mesh_visible = true;
        let colored_mesh = draw(&studio);
        assert_eq!(colored_mesh.geometry.mesh_vertices.len(), 3);
        assert_eq!(
            colored_mesh.geometry.mesh_vertices[0].color,
            [1.0, 0.0, 0.0, 0.82]
        );
        assert_eq!(
            colored_mesh.geometry.mesh_vertices[1].color[1],
            128.0 / 255.0
        );
        assert_eq!(
            colored_mesh.geometry.mesh_vertices[0].normal,
            [0.0, 0.0, 1.0, 0.0]
        );

        studio.clouds[0].transform.scale = [-1.0, 2.0, 1.0];
        let reflected_mesh = draw(&studio);
        assert_eq!(
            reflected_mesh.geometry.mesh_vertices[0].normal,
            [0.0, 0.0, -1.0, 0.0]
        );
    }
}
