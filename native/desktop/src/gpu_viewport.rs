//! Native WGPU point-sprite renderer embedded in Iced's shader widget.
//! Survey coordinates are rebased in f64 before f32 upload to retain precision.

use std::cell::RefCell;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use crate::section_fill::CapStyle;
use crate::selection::{ClassVisibility, DeletionMask};
use crate::station_photos::{self, PhotoAtlas, PhotoSet};
use crate::CloudTransform;
use crate::{combined_bounds, CloudEntry, ColorMode, Message, PointViewport};
use bytemuck::{Pod, Zeroable};
use iced::advanced::Shell;
use iced::widget::shader::{self, Shader};
use iced::{event, mouse, window, Rectangle};
use iced_wgpu::primitive::{Primitive, Storage};
use iced_wgpu::wgpu;
use pointcloud_core::photo_colour::PointColours;
use pointcloud_core::{
    section_caps, Bounds, CapOptions, IndexedPoint, MeshGeometry, OrientedBox, PointCloud,
};

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

// Walking is always close to the points, where a fixed size on screen leaves
// the nearest surfaces thin. Points are drawn thicker there and keep a size
// in the scene, per unit of the chosen point size, up to a few times their
// distant size.
const WALK_POINT_SCALE: f32 = 1.3;
const WALK_POINT_SCENE_RADIUS: f32 = 0.0075;
const WALK_POINT_GROWTH: f32 = 3.5;

/// Smallest radius on screen, radius in the scene and largest radius on
/// screen of a point in the walking view.
pub(crate) fn walk_point_radii(point_size: f32) -> [f32; 3] {
    let smallest = display_point_radius(point_size, 0.05) * WALK_POINT_SCALE;
    [
        smallest,
        WALK_POINT_SCENE_RADIUS * point_size,
        smallest * WALK_POINT_GROWTH,
    ]
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
    mesh_key: Option<MeshKey>,
    mesh: Option<Arc<MeshBuffers>>,
    /// What `caps` were made from; `None` while they are not those of the
    /// scene.
    caps_key: Option<CapKey>,
    caps: Arc<MeshBuffers>,
    /// The caps being made on a worker thread.
    caps_job: Option<CapJob>,
}

/// Caps that are being made: for a mesh of millions of triangles that takes
/// a moment, which the window does not wait for.
struct CapJob {
    key: CapKey,
    result: mpsc::Receiver<MeshBuffers>,
}

/// What the caps over the cut of the meshes are made from: the meshes, the
/// section box and how the cut is filled.
struct CapKey {
    meshes: MeshKey,
    section: OrientedBox,
    style: CapStyle,
}

impl CapKey {
    fn matches(
        &self,
        clouds: &[CloudEntry],
        bounds: Option<Bounds>,
        section: OrientedBox,
        style: CapStyle,
    ) -> bool {
        self.section == section && self.style == style && self.meshes.matches(clouds, bounds)
    }
}

/// What the buffers of the meshes are made from. The points on screen, the
/// section box, the class filters and the deletions are not among it: a mesh
/// of millions of triangles is not built and sent to the graphics device
/// again each time the points are refined.
struct MeshKey {
    /// The centre of the scene, which the vertices are relative to.
    center: Option<[f64; 3]>,
    /// Every mesh that is switched on, in the order of the layers, with
    /// where its layer stands: the mesh of a layer and then its detected
    /// faces.
    shown: Vec<(Arc<MeshGeometry>, CloudTransform)>,
}

impl MeshKey {
    fn shown(clouds: &[CloudEntry]) -> impl Iterator<Item = (&Arc<MeshGeometry>, CloudTransform)> {
        shown_meshes(clouds).map(|(index, _, mesh)| (mesh, clouds[index].transform))
    }

    fn capture(clouds: &[CloudEntry], bounds: Option<Bounds>) -> Self {
        Self {
            center: bounds.map(|bounds| bounds.center()),
            shown: Self::shown(clouds)
                .map(|(mesh, transform)| (Arc::clone(mesh), transform))
                .collect(),
        }
    }

    fn matches(&self, clouds: &[CloudEntry], bounds: Option<Bounds>) -> bool {
        let mut shown = Self::shown(clouds);
        // Without a mesh to draw the centre of the scene does not matter.
        (self.shown.is_empty() || self.center == bounds.map(|bounds| bounds.center()))
            && self.shown.iter().all(|(mesh, transform)| {
                shown
                    .next()
                    .is_some_and(|(other, stands)| Arc::ptr_eq(mesh, other) && *transform == stands)
            })
            && shown.next().is_none()
    }
}

/// What the points on the graphics device are made from. The section box is
/// not among it: the shader clips the points to the box, so switching the box
/// on or off, dragging a face or turning it changes no point and sends none
/// to the device again.
struct SceneKey {
    clouds: Vec<CloudKey>,
    bounds: Option<Bounds>,
    color_mode: ColorMode,
    budget: usize,
    filters: [bool; 4],
    class_visibility: ClassVisibility,
}

struct CloudKey {
    source: Arc<PointCloud>,
    transform: CloudTransform,
    detail: Option<Arc<[IndexedPoint]>>,
    /// The points read inside the section box besides `detail`.
    focus: Option<Arc<[IndexedPoint]>>,
    deleted: Option<Arc<DeletionMask>>,
    colours: Option<Arc<PointColours>>,
    mesh: Option<Arc<MeshGeometry>>,
    /// The mesh of the detected faces as it is shown: nothing while the
    /// faces are switched off.
    faces: Option<Arc<MeshGeometry>>,
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
                    focus: entry.focus_points.clone(),
                    deleted: entry.deleted.clone(),
                    colours: entry.colours.clone(),
                    mesh: entry.mesh.clone(),
                    faces: shown_faces(entry).cloned(),
                    visible: entry.visible,
                    mesh_visible: entry.mesh_visible,
                })
                .collect(),
            bounds,
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
        self.budget == view.budget
            && self.matches_where(view, bounds, |key, entry| {
                key.points_match(entry) && key.focus_matches(entry) && key.meshes_match(entry)
            })
    }

    /// Whether the points drawn for the view would be the same.
    fn points_match(&self, view: PointViewport<'_>, bounds: Option<Bounds>) -> bool {
        self.budget == view.budget
            && self.matches_where(view, bounds, |key, entry| key.points_match(entry))
    }

    /// Whether the points read inside the section box would be the same.
    fn focus_match(&self, view: PointViewport<'_>, bounds: Option<Bounds>) -> bool {
        self.matches_where(view, bounds, |key, entry| key.focus_matches(entry))
    }

    /// The settings that every point depends on, and `cloud` for each layer.
    fn matches_where(
        &self,
        view: PointViewport<'_>,
        bounds: Option<Bounds>,
        cloud: impl Fn(&CloudKey, &CloudEntry) -> bool,
    ) -> bool {
        self.bounds == bounds
            && self.color_mode == view.color_mode
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
                .all(|(key, entry)| cloud(key, entry))
    }
}

impl CloudKey {
    /// What a point of the layer is drawn from: its place, its colour and
    /// whether it was deleted.
    fn drawn_alike(&self, entry: &CloudEntry) -> bool {
        Arc::ptr_eq(&self.source, &entry.cloud)
            && self.transform == entry.transform
            && same_arc(&self.deleted, &entry.deleted)
            && same_arc(&self.colours, &entry.colours)
            && self.visible == entry.visible
    }

    fn points_match(&self, entry: &CloudEntry) -> bool {
        self.drawn_alike(entry) && same_arc(&self.detail, &entry.detail_points)
    }

    fn focus_matches(&self, entry: &CloudEntry) -> bool {
        self.drawn_alike(entry) && same_arc(&self.focus, &entry.focus_points)
    }

    fn meshes_match(&self, entry: &CloudEntry) -> bool {
        same_arc(&self.mesh, &entry.mesh)
            && match (&self.faces, shown_faces(entry)) {
                (Some(kept), Some(shown)) => Arc::ptr_eq(kept, shown),
                (None, None) => true,
                _ => false,
            }
            && self.mesh_visible == entry.mesh_visible
    }
}

/// The largest buffer the graphics device takes: the default limit of the
/// device, which is what the window asks for.
const MAX_BUFFER_BYTES: usize = 256 << 20;
/// All meshes are drawn from one vertex buffer and one index buffer, so
/// together they hold at most this many vertices and corners of triangles.
/// One mesh at the limits of a mesh file always fits.
const MAX_DRAWN_MESH_VERTICES: usize = MAX_BUFFER_BYTES / std::mem::size_of::<GpuMeshVertex>();
const MAX_DRAWN_MESH_INDICES: usize = MAX_BUFFER_BYTES / std::mem::size_of::<u32>();
const _: () = assert!(
    pointcloud_core::MAX_MESH_VERTICES <= MAX_DRAWN_MESH_VERTICES
        && pointcloud_core::MAX_MESH_TRIANGLES * 3 <= MAX_DRAWN_MESH_INDICES
);

/// Which meshes of a row fit the buffers together, each given by its place,
/// its vertices and its triangles: every one that fits beside those taken
/// before it. A larger buffer would be refused by the device and end the
/// application, so a mesh that does not fit is left out and a smaller one
/// after it is still taken.
fn meshes_that_fit(sizes: impl IntoIterator<Item = (usize, usize, usize)>) -> Vec<usize> {
    let mut taken = Vec::new();
    let (mut vertices, mut indices) = (0usize, 0usize);
    for (place, mesh_vertices, mesh_triangles) in sizes {
        let mesh_indices = mesh_triangles.saturating_mul(3);
        if vertices.saturating_add(mesh_vertices) > MAX_DRAWN_MESH_VERTICES
            || indices.saturating_add(mesh_indices) > MAX_DRAWN_MESH_INDICES
        {
            continue;
        }
        vertices += mesh_vertices;
        indices += mesh_indices;
        taken.push(place);
    }
    taken
}

/// What a mesh of a layer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeshPart {
    /// The mesh the layer holds: from a mesh job or from its file.
    Surface,
    /// The faces detected in the layer, in the colouring that was chosen.
    Faces,
}

/// The mesh of the detected faces of a layer, when they are switched on.
fn shown_faces(entry: &CloudEntry) -> Option<&Arc<MeshGeometry>> {
    entry
        .faces
        .as_ref()
        .and_then(crate::faces::FaceLayer::shown)
}

/// Every mesh that is switched on, with the place of its layer: per layer
/// its own mesh and then its detected faces.
fn shown_meshes(
    clouds: &[CloudEntry],
) -> impl Iterator<Item = (usize, MeshPart, &Arc<MeshGeometry>)> {
    clouds.iter().enumerate().flat_map(|(index, entry)| {
        let surface = entry.mesh.as_ref().filter(|_| entry.mesh_visible);
        [
            surface.map(|mesh| (index, MeshPart::Surface, mesh)),
            shown_faces(entry).map(|mesh| (index, MeshPart::Faces, mesh)),
        ]
        .into_iter()
        .flatten()
    })
}

/// The meshes that are drawn: every one that is switched on, in the order
/// of `shown_meshes`, as long as it fits the buffers beside those before it.
fn drawn(clouds: &[CloudEntry]) -> Vec<(usize, MeshPart, &Arc<MeshGeometry>)> {
    let shown: Vec<_> = shown_meshes(clouds).collect();
    let fit = meshes_that_fit(
        shown
            .iter()
            .enumerate()
            .map(|(place, (_, _, mesh))| (place, mesh.vertices.len(), mesh.triangles.len())),
    );
    fit.into_iter().map(|place| shown[place]).collect()
}

fn drawn_part(clouds: &[CloudEntry], part: MeshPart) -> Vec<usize> {
    drawn(clouds)
        .into_iter()
        .filter(|(_, drawn, _)| *drawn == part)
        .map(|(index, _, _)| index)
        .collect()
}

/// The layers whose mesh is drawn: every mesh that is switched on, in the
/// order of the layers, as long as it fits the buffers.
pub(crate) fn drawn_meshes(clouds: &[CloudEntry]) -> Vec<usize> {
    drawn_part(clouds, MeshPart::Surface)
}

/// The layers whose detected faces are drawn. They share the buffers with
/// the meshes, so they are counted with them.
pub(crate) fn drawn_faces(clouds: &[CloudEntry]) -> Vec<usize> {
    drawn_part(clouds, MeshPart::Faces)
}

impl<'a> GpuViewport<'a> {
    pub fn widget(self) -> Shader<Message, Self> {
        Shader::new(self)
    }

    /// The points of the sets read for the view, thinned to the budget. The
    /// section box does not drop any: the shader clips them, so that the box
    /// can change without a point being built or sent again.
    fn build_points(&self, overall_bounds: Option<Bounds>) -> Vec<GpuPoint> {
        let mut points = Vec::new();
        if let Some(overall_bounds) = overall_bounds {
            let sampled: usize = self
                .overlay
                .clouds
                .iter()
                .filter(|entry| entry.visible)
                .map(CloudEntry::view_len)
                .sum();
            let stride = sampled.div_ceil(self.overlay.budget.max(1)).max(1);
            points.reserve(sampled.div_ceil(stride));
            let started = Instant::now();
            self.push_points(
                &mut points,
                overall_bounds,
                self.overlay
                    .clouds
                    .iter()
                    .filter(|entry| entry.visible)
                    .flat_map(|entry| {
                        entry
                            .view_records()
                            .filter(move |record| entry.record_visible(*record))
                    })
                    .step_by(stride),
            );
            // The pace counts the records walked, not the points kept: class
            // filters and deletions drop a record after the work of reaching
            // it. A build thinned to the budget skips records unseen, so only
            // a full one is a measure.
            if stride == 1 {
                self.overlay
                    .lod_pace
                    .record_build(sampled, started.elapsed());
            }
        }
        points
    }

    /// The points read inside the section box besides the sets of the view,
    /// all of them: the refinement that read them kept them within the
    /// budget of what the box shows.
    fn build_focus_points(&self, overall_bounds: Option<Bounds>) -> Vec<GpuPoint> {
        let mut points = Vec::new();
        if let Some(overall_bounds) = overall_bounds {
            let shown = || self.overlay.clouds.iter().filter(|entry| entry.visible);
            points.reserve(shown().map(CloudEntry::focus_len).sum());
            self.push_points(
                &mut points,
                overall_bounds,
                shown().flat_map(|entry| {
                    entry
                        .focus_records()
                        .filter(move |record| entry.record_visible(*record))
                }),
            );
        }
        points
    }

    /// Add the records that the class filters let through, relative to the
    /// centre of the scene and in the chosen colours.
    fn push_points(
        &self,
        points: &mut Vec<GpuPoint>,
        overall_bounds: Bounds,
        records: impl Iterator<Item = IndexedPoint>,
    ) {
        let center = overall_bounds.center();
        for record in records {
            let point = &record.point;
            if !self.overlay.accepts_class(point) {
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
    }

    /// The buffers of every mesh that is drawn, relative to the centre of
    /// the scene.
    fn build_mesh(&self, overall_bounds: Option<Bounds>) -> MeshBuffers {
        let mut mesh_vertices = Vec::new();
        let mut mesh_indices = Vec::new();
        if let Some(overall_bounds) = overall_bounds {
            let center = overall_bounds.center();
            for (index, _, mesh) in drawn(self.overlay.clouds) {
                let entry = &self.overlay.clouds[index];
                let mesh: &MeshGeometry = mesh;
                // What `drawn` lets through stays far below the range of a
                // 32-bit index.
                let Ok(base) = u32::try_from(mesh_vertices.len()) else {
                    break;
                };
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
        MeshBuffers {
            vertices: mesh_vertices,
            indices: mesh_indices,
        }
    }

    /// Bring the caps up to date with the scene as far as they can be now:
    /// take caps that were made, start making them when they are not those
    /// of the scene, and leave out caps that no longer fit it. Answers
    /// whether caps are still being made for this view. The window learns
    /// it from `section_cap_jobs`, which counts a job until its thread ends,
    /// also when no frame comes to take what it made.
    fn advance_caps(&self, cache: &mut RenderCache, overall_bounds: Option<Bounds>) -> bool {
        let clouds = self.overlay.clouds;
        let wanted = self.overlay.section.zip(self.overlay.section_fill);
        let current = |key: &CapKey| {
            wanted
                .is_some_and(|(section, style)| key.matches(clouds, overall_bounds, section, style))
        };
        if let Some(job) = &cache.caps_job {
            match job.result.try_recv() {
                Ok(caps) => {
                    let job = cache.caps_job.take().expect("cap job");
                    if current(&job.key) {
                        cache.caps = Arc::new(caps);
                        cache.caps_key = Some(job.key);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => cache.caps_job = None,
            }
        }
        if cache.caps_key.as_ref().is_some_and(current) {
            return false;
        }
        // Caps of another box or of other meshes are not shown.
        if cache.caps_key.take().is_some() {
            cache.caps = Arc::default();
        }
        let Some((section, style)) = wanted else {
            return false;
        };
        if cache.caps_job.is_some() {
            // Caps of an earlier box are still being made; those of this
            // one are started once they are done, so that a box that is
            // dragged keeps one job going and not one for every step.
            return true;
        }
        let key = CapKey {
            meshes: MeshKey::capture(clouds, overall_bounds),
            section,
            style,
        };
        let meshes: Vec<_> = drawn(clouds)
            .into_iter()
            .filter(|(_, part, _)| *part == MeshPart::Surface)
            .map(|(index, _, mesh)| (Arc::clone(mesh), clouds[index].transform))
            .collect();
        let center = overall_bounds.map(|bounds| bounds.center());
        let (Some(center), false) = (center, meshes.is_empty()) else {
            cache.caps_key = Some(key);
            return false;
        };
        let (send, result) = mpsc::channel();
        let running = self.overlay.section_cap_jobs.start();
        let started = std::thread::Builder::new()
            .name("section caps".into())
            .spawn(move || {
                let _ = send.send(build_caps(center, &meshes, section, style));
                // Only once the caps are sent: when nothing is pending any
                // more, the next frame finds them.
                drop(running);
            });
        if started.is_err() {
            // Without a thread the cut is shown open, as without the fill.
            cache.caps_key = Some(key);
            return false;
        }
        cache.caps_job = Some(CapJob { key, result });
        true
    }
}

/// The caps where the section box cuts meshes, in the cap colour and
/// without a normal, so they are drawn flat; relative to the centre of the
/// scene. Detected faces are single surfaces and are not among the meshes.
fn build_caps(
    center: [f64; 3],
    meshes: &[(Arc<MeshGeometry>, CloudTransform)],
    section: OrientedBox,
    style: CapStyle,
) -> MeshBuffers {
    let mut caps = MeshBuffers::default();
    let color = [
        f32::from(style.color[0]) / 255.0,
        f32::from(style.color[1]) / 255.0,
        f32::from(style.color[2]) / 255.0,
        1.0,
    ];
    let options = CapOptions {
        max_thickness: style.max_thickness,
    };
    for (mesh, transform) in meshes {
        let found = section_caps(mesh, |xyz| transform.xyz(xyz), &section, options);
        let Ok(base) = u32::try_from(caps.vertices.len()) else {
            break;
        };
        caps.vertices
            .extend(found.vertices.iter().map(|xyz| GpuMeshVertex {
                relative: [
                    (xyz[0] - center[0]) as f32,
                    (xyz[1] - center[1]) as f32,
                    (xyz[2] - center[2]) as f32,
                    0.0,
                ],
                color,
                normal: [0.0; 4],
            }));
        caps.indices.extend(
            found
                .triangles
                .iter()
                .flat_map(|face| face.map(|index| index + base)),
        );
    }
    caps
}

impl shader::Program<Message> for GpuViewport<'_> {
    type State = RefCell<RenderCache>;
    type Primitive = CloudPrimitive;

    /// While caps are being made the window keeps drawing, so that they
    /// show as soon as they are done.
    fn update(
        &self,
        state: &mut Self::State,
        event: shader::Event,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
        shell: &mut Shell<'_, Message>,
    ) -> (event::Status, Option<Message>) {
        if let shader::Event::RedrawRequested(_) = event {
            let overall_bounds = combined_bounds(self.overlay.clouds);
            if self.advance_caps(state.get_mut(), overall_bounds) {
                shell.request_redraw(window::RedrawRequest::NextFrame);
            }
        }
        (event::Status::Ignored, None)
    }

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
                let clouds = self.overlay.clouds;
                let kept = cache
                    .mesh
                    .as_ref()
                    .filter(|_| {
                        cache
                            .mesh_key
                            .as_ref()
                            .is_some_and(|key| key.matches(clouds, overall_bounds))
                    })
                    .map(Arc::clone);
                let mesh = kept.unwrap_or_else(|| {
                    let mesh = Arc::new(self.build_mesh(overall_bounds));
                    cache.mesh_key = Some(MeshKey::capture(clouds, overall_bounds));
                    cache.mesh = Some(Arc::clone(&mesh));
                    mesh
                });
                // The points of the view and those read inside the section
                // box are built and sent on their own: new detail in the box
                // leaves the points of the view on the device.
                let previous = cache.key.as_ref().zip(cache.geometry.as_ref());
                let points = previous
                    .filter(|(key, _)| key.points_match(self.overlay, overall_bounds))
                    .map(|(_, geometry)| Arc::clone(&geometry.points))
                    .unwrap_or_else(|| self.build_points(overall_bounds).into());
                let focus = previous
                    .filter(|(key, _)| key.focus_match(self.overlay, overall_bounds))
                    .map(|(_, geometry)| Arc::clone(&geometry.focus))
                    .unwrap_or_else(|| self.build_focus_points(overall_bounds).into());
                cache.geometry = Some(Arc::new(RenderGeometry {
                    points,
                    focus,
                    mesh,
                }));
                cache.key = Some(SceneKey::capture(self.overlay, overall_bounds));
            }
            Arc::clone(cache.geometry.as_ref().expect("render geometry cached"))
        };
        let caps = {
            let mut cache = state.borrow_mut();
            self.advance_caps(&mut cache, overall_bounds);
            Arc::clone(&cache.caps)
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
                Some(_) => {
                    let [smallest, scene, largest] = walk_point_radii(self.overlay.point_size);
                    camera.splat = [scene, largest, 0.0, 0.0];
                    [0.0, 0.0, smallest, 1.0]
                }
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
                let (min, max) = (section.bounds.min, section.bounds.max);
                // A turned box carries the sine and cosine of its turn in
                // the spare components; the shader turns each fragment into
                // the frame of the box before it tests the limits.
                let (sin, cos) = if section.is_turned() {
                    section.rotation_degrees.to_radians().sin_cos()
                } else {
                    (0.0, 1.0)
                };
                camera.clip_min = [
                    (min[0] - center[0]) as f32,
                    (min[1] - center[1]) as f32,
                    (min[2] - center[2]) as f32,
                    sin as f32,
                ];
                camera.clip_max = [
                    (max[0] - center[0]) as f32,
                    (max[1] - center[1]) as f32,
                    (max[2] - center[2]) as f32,
                    cos as f32,
                ];
                camera.clip_enabled[0] = if section.is_turned() { 2.0 } else { 1.0 };
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
        // A photo of the file seen from where it was taken lies over the
        // points, unless a station's own photos fill the view.
        let photo = self
            .overlay
            .shown_photo()
            .filter(|_| photos.panorama.is_none())
            .and_then(|shown| crate::photo_overlay::frame(&shown));
        CloudPrimitive {
            geometry,
            caps,
            camera,
            photos,
            photo,
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
    splat: [f32; 4],
}

#[derive(Debug)]
struct RenderGeometry {
    /// The points of the sets read for the view.
    points: Arc<[GpuPoint]>,
    /// The points read inside the section box besides them. Each of the two
    /// is shared with the geometry before it while it did not change, and
    /// is then not sent to the device again.
    focus: Arc<[GpuPoint]>,
    /// Shared with the geometry before it when no mesh, layer transform or
    /// scene centre changed.
    mesh: Arc<MeshBuffers>,
}

/// The vertices and the corners of the triangles of all drawn meshes, as the
/// graphics device takes them.
#[derive(Debug, Default)]
struct MeshBuffers {
    vertices: Vec<GpuMeshVertex>,
    indices: Vec<u32>,
}

struct PointBufferChunk {
    buffer: wgpu::Buffer,
    capacity: u64,
    count: u32,
}

/// Points on the device, in buffers of at most `POINTS_PER_BUFFER` points.
/// They are sent again only when the set they are made from changes.
#[derive(Default)]
struct PointUpload {
    chunks: Vec<PointBufferChunk>,
    uploaded: Option<Arc<[GpuPoint]>>,
}

impl PointUpload {
    /// Send `points` unless they are on the device already; answers the
    /// bytes sent.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        points: &Arc<[GpuPoint]>,
    ) -> u64 {
        if self
            .uploaded
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, points))
        {
            return 0;
        }
        let mut sent = 0;
        for (index, part) in points.chunks(POINTS_PER_BUFFER).enumerate() {
            let byte_count = std::mem::size_of_val(part) as u64;
            let capacity = byte_count.next_power_of_two();
            let buffer = || {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("pointcloud points"),
                    size: capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            };
            if index == self.chunks.len() {
                self.chunks.push(PointBufferChunk {
                    buffer: buffer(),
                    capacity,
                    count: 0,
                });
            }
            let chunk = &mut self.chunks[index];
            if byte_count > chunk.capacity {
                chunk.buffer = buffer();
                chunk.capacity = capacity;
            }
            queue.write_buffer(&chunk.buffer, 0, bytemuck::cast_slice(part));
            chunk.count = part.len() as u32;
            sent += byte_count;
        }
        self.chunks
            .truncate(points.len().div_ceil(POINTS_PER_BUFFER));
        self.uploaded = Some(Arc::clone(points));
        sent
    }
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
    /// The caps over the cut of the meshes by the section box.
    caps: Arc<MeshBuffers>,
    camera: CameraUniform,
    photos: PhotoFrame,
    /// The photo of a file laid over the points.
    photo: Option<crate::photo_overlay::PhotoFrame>,
}

/// Vertex and index buffers of meshes on the device. They are sent again
/// only when the buffers they are made from change.
struct MeshUpload {
    label: &'static str,
    vertex_buffer: wgpu::Buffer,
    vertex_capacity: u64,
    index_buffer: wgpu::Buffer,
    index_capacity: u64,
    index_count: u32,
    uploaded: Option<Arc<MeshBuffers>>,
}

impl MeshUpload {
    fn new(device: &wgpu::Device, label: &'static str) -> Self {
        let vertex_capacity = std::mem::size_of::<GpuMeshVertex>() as u64;
        let index_capacity = std::mem::size_of::<u32>() as u64;
        Self {
            label,
            vertex_buffer: Self::buffer(device, label, vertex_capacity, wgpu::BufferUsages::VERTEX),
            vertex_capacity,
            index_buffer: Self::buffer(device, label, index_capacity, wgpu::BufferUsages::INDEX),
            index_capacity,
            index_count: 0,
            uploaded: None,
        }
    }

    fn buffer(
        device: &wgpu::Device,
        label: &'static str,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mesh: &Arc<MeshBuffers>) {
        if self
            .uploaded
            .as_ref()
            .is_some_and(|previous| Arc::ptr_eq(previous, mesh))
        {
            return;
        }
        let vertex_bytes = std::mem::size_of_val(mesh.vertices.as_slice()) as u64;
        if vertex_bytes > self.vertex_capacity {
            self.vertex_capacity = vertex_bytes.next_power_of_two();
            self.vertex_buffer = Self::buffer(
                device,
                self.label,
                self.vertex_capacity,
                wgpu::BufferUsages::VERTEX,
            );
        }
        if !mesh.vertices.is_empty() {
            queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&mesh.vertices));
        }
        let index_bytes = std::mem::size_of_val(mesh.indices.as_slice()) as u64;
        if index_bytes > self.index_capacity {
            self.index_capacity = index_bytes.next_power_of_two();
            self.index_buffer = Self::buffer(
                device,
                self.label,
                self.index_capacity,
                wgpu::BufferUsages::INDEX,
            );
        }
        if !mesh.indices.is_empty() {
            queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&mesh.indices));
        }
        self.index_count = mesh.indices.len() as u32;
        self.uploaded = Some(Arc::clone(mesh));
    }
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
    /// The points of the sets read for the view.
    points: PointUpload,
    /// The points read inside the section box besides them.
    focus: PointUpload,
    mesh: MeshUpload,
    caps: MeshUpload,
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    color_texture: Option<wgpu::Texture>,
    color_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
    photo_overlay: crate::photo_overlay::PhotoOverlay,
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
            points: PointUpload::default(),
            focus: PointUpload::default(),
            mesh: MeshUpload::new(device, "terrain mesh"),
            caps: MeshUpload::new(device, "section caps"),
            depth_texture: None,
            depth_view: None,
            color_texture: None,
            color_view: None,
            depth_size: (0, 0),
            photo_overlay: crate::photo_overlay::PhotoOverlay::new(device, format, &camera_layout),
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
        // Each set of points is sent only when it changed: a change of the
        // section box sends none, and detail read inside the box sends only
        // that detail.
        state.points.upload(device, queue, &self.geometry.points);
        state.focus.upload(device, queue, &self.geometry.focus);
        // The meshes are sent only when they changed: a refinement of the
        // points keeps the buffers of a mesh of millions of triangles.
        state.mesh.upload(device, queue, &self.geometry.mesh);
        state.caps.upload(device, queue, &self.caps);
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
        match &self.photo {
            Some(frame) => state.photo_overlay.prepare(device, queue, frame),
            // A photo on the device is large: it goes once none is shown.
            None => state.photo_overlay.release(),
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
        if state.points.chunks.is_empty()
            && state.focus.chunks.is_empty()
            && state.mesh.index_count == 0
            && state.caps.index_count == 0
            && balls.is_none()
            && self.photo.is_none()
        {
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
            // The caps are drawn with the meshes: flat, as they carry no
            // normal.
            for meshes in [&state.mesh, &state.caps] {
                if meshes.index_count > 0 {
                    pass.set_pipeline(&state.mesh_pipeline);
                    pass.set_vertex_buffer(0, meshes.vertex_buffer.slice(..));
                    pass.set_index_buffer(meshes.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..meshes.index_count, 0, 0..1);
                }
            }
            let chunks = || state.points.chunks.iter().chain(&state.focus.chunks);
            if chunks().next().is_some() {
                pass.set_pipeline(&state.pipeline);
                for chunk in chunks() {
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
        drop(pass);
        if self.photo.is_some() {
            state
                .photo_overlay
                .render(encoder, target, clip_bounds, &state.camera_group);
        }
    }
}

/// A frame of the 3D view as a test elsewhere sees it: which sets of points
/// it sends to the device, and how many of their points the shader draws.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DrawnFrame {
    /// The points of the sets of the view, by identity: a frame that has
    /// the same ones sends none of them to the device.
    pub points: usize,
    /// The points read inside the section box, by identity.
    pub focus: usize,
    /// The points of both that the shader draws: those inside the section
    /// box while it is on.
    pub drawn: usize,
    /// Those of them that are on the screen.
    pub in_view: usize,
}

/// Draw a frame of the 3D view of `studio` with the cache of earlier frames.
#[cfg(test)]
pub(crate) fn drawn_frame(studio: &crate::Studio, state: &RefCell<RenderCache>) -> DrawnFrame {
    let viewport = GpuViewport {
        overlay: studio.point_viewport(),
    };
    let bounds = Rectangle::new(iced::Point::ORIGIN, studio.viewport_size);
    let frame = shader::Program::draw(&viewport, state, mouse::Cursor::Unavailable, bounds);
    let geometry = &frame.geometry;
    let drawn: Vec<_> = geometry
        .points
        .iter()
        .chain(geometry.focus.iter())
        .filter(|point| !clipped(&frame.camera, point.relative))
        .collect();
    let in_view = combined_bounds(&studio.clouds).map_or(0, |scene| {
        let center = scene.center();
        let projection = studio.projection(scene, bounds.width, bounds.height);
        drawn
            .iter()
            .filter(|point| {
                let xyz =
                    std::array::from_fn(|axis| center[axis] + f64::from(point.relative[axis]));
                projection.project(xyz).is_some()
            })
            .count()
    });
    DrawnFrame {
        points: Arc::as_ptr(&geometry.points) as *const GpuPoint as usize,
        focus: Arc::as_ptr(&geometry.focus) as *const GpuPoint as usize,
        drawn: drawn.len(),
        in_view,
    }
}

/// Whether the shader leaves out a point at `relative`: what
/// `outside_section` in points.wgsl does, in the same order and precision.
#[cfg(test)]
fn clipped(camera: &CameraUniform, relative: [f32; 4]) -> bool {
    if camera.clip_enabled[0] < 0.5 {
        return false;
    }
    let mut at = [relative[0], relative[1], relative[2]];
    if camera.clip_enabled[0] > 1.5 {
        let center = [
            (camera.clip_min[0] + camera.clip_max[0]) * 0.5,
            (camera.clip_min[1] + camera.clip_max[1]) * 0.5,
        ];
        let offset = [at[0] - center[0], at[1] - center[1]];
        let (sine, cosine) = (camera.clip_min[3], camera.clip_max[3]);
        at[0] = center[0] + cosine * offset[0] + sine * offset[1];
        at[1] = center[1] - sine * offset[0] + cosine * offset[1];
    }
    (0..3).any(|axis| at[axis] < camera.clip_min[axis] || at[axis] > camera.clip_max[axis])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Studio;

    #[test]
    fn meshes_are_drawn_as_far_as_the_buffers_of_the_device_hold_them() {
        // A vertex takes 48 bytes of a buffer of at most 256 MiB.
        assert_eq!(std::mem::size_of::<GpuMeshVertex>(), 48);
        assert_eq!(MAX_DRAWN_MESH_VERTICES, 5_592_405);
        assert_eq!(MAX_DRAWN_MESH_INDICES, 67_108_864);
        let (vertices, triangles) = (
            pointcloud_core::MAX_MESH_VERTICES,
            pointcloud_core::MAX_MESH_TRIANGLES,
        );
        // One mesh at the limits of a mesh file fits, with room to spare for
        // smaller ones.
        assert_eq!(meshes_that_fit([(0, vertices, triangles)]), [0]);
        assert_eq!(
            meshes_that_fit([(0, vertices, triangles), (3, 1_000_000, 2_000_000)]),
            [0, 3]
        );
        // A second one of that size does not: it is left out, and the small
        // mesh after it is still drawn.
        assert_eq!(
            meshes_that_fit([
                (0, vertices, triangles),
                (1, vertices, triangles),
                (2, 500_000, 1_000_000)
            ]),
            [0, 2]
        );
        // The indices have a limit of their own.
        assert_eq!(
            meshes_that_fit([(0, 3, MAX_DRAWN_MESH_INDICES / 3 + 1), (1, 3, 1)]),
            [1]
        );
        assert!(meshes_that_fit([(0, usize::MAX, 1)]).is_empty());

        // A layer whose mesh is switched off takes no room.
        let mut studio = Studio::default();
        assert!(drawn_meshes(&studio.clouds).is_empty());
        let triangle = || {
            Some(Arc::new(MeshGeometry {
                vertices: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                triangles: vec![[0, 1, 2]],
                colors: None,
                normals: None,
            }))
        };
        let directory = tempfile::tempdir().unwrap();
        for name in ["a.xyz", "b.xyz", "c.xyz"] {
            let source = directory.path().join(name);
            std::fs::write(&source, "0 0 0\n1 0 0\n0 1 0\n").unwrap();
            let cloud = Arc::new(pointcloud_core::open(&source, 3).unwrap());
            let _ = studio.update(Message::Loaded(Ok(cloud)));
        }
        assert_eq!(studio.clouds.len(), 3);
        studio.clouds[0].mesh = triangle();
        studio.clouds[2].mesh = triangle();
        studio.clouds[0].mesh_visible = false;
        studio.clouds[2].mesh_visible = true;
        assert_eq!(drawn_meshes(&studio.clouds), [2]);

        // The detected faces of a layer are one more mesh in the same
        // buffers: after the mesh of their layer, and counted with the
        // meshes. Faces that are switched off take no room.
        let faces = |vertices: usize, visible: bool| {
            let mesh = MeshGeometry {
                vertices: vec![[0.0; 3]; vertices],
                triangles: vec![[0, 1, 2]],
                colors: None,
                normals: None,
            };
            Some(crate::faces::FaceLayer::showing(Arc::new(mesh), visible))
        };
        assert!(drawn_faces(&studio.clouds).is_empty());
        studio.clouds[1].faces = faces(3, true);
        studio.clouds[2].faces = faces(3, true);
        studio.clouds[0].faces = faces(3, false);
        let order: Vec<(usize, MeshPart)> = drawn(&studio.clouds)
            .into_iter()
            .map(|(index, part, _)| (index, part))
            .collect();
        assert_eq!(
            order,
            [
                (1, MeshPart::Faces),
                (2, MeshPart::Surface),
                (2, MeshPart::Faces)
            ]
        );
        assert_eq!(drawn_meshes(&studio.clouds), [2]);
        assert_eq!(drawn_faces(&studio.clouds), [1, 2]);
        // Faces that fill the buffer leave no room for what comes after
        // them, and faces that do not fit are left out themselves while the
        // smaller meshes after them are still drawn.
        studio.clouds[1].faces = faces(MAX_DRAWN_MESH_VERTICES, true);
        assert_eq!(drawn_faces(&studio.clouds), [1]);
        assert!(drawn_meshes(&studio.clouds).is_empty());
        studio.clouds[1].faces = None;
        studio.clouds[1].faces = faces(MAX_DRAWN_MESH_VERTICES + 1, true);
        assert_eq!(drawn_faces(&studio.clouds), [2]);
        assert_eq!(drawn_meshes(&studio.clouds), [2]);
    }

    #[test]
    fn close_up_spheres_grow_without_overriding_point_size() {
        assert_eq!(display_point_radius(2.0, 1.0), 2.0);
        assert_eq!(display_point_radius(8.0, 2.0), 8.0);
        assert!(display_point_radius(2.0, 1.0 / 270.0) > 4.0);
        assert_eq!(display_point_radius(2.0, 0.000_001), 6.0);
        assert_eq!(display_point_radius(8.0, 0.000_001), 12.0);
    }

    /// A scan whose layer holds the mesh of a wall of 0.2 from x 0 to 4 and
    /// z 0 to 2: its outer face looks to -y, its inner face to +y.
    fn studio_with_wall() -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("wall.xyz");
        std::fs::write(&source, "0 -1 0\n4 1 2\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 2).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let wall = MeshGeometry {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [4.0, 0.0, 0.0],
                [4.0, 0.0, 2.0],
                [0.0, 0.0, 2.0],
                [0.0, 0.2, 0.0],
                [4.0, 0.2, 0.0],
                [4.0, 0.2, 2.0],
                [0.0, 0.2, 2.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6]],
            colors: None,
            normals: None,
        };
        studio.clouds[0].mesh = Some(Arc::new(wall));
        studio.clouds[0].mesh_visible = true;
        (studio, directory)
    }

    /// Cut the wall at half its height.
    fn cut_at_half_height(studio: &mut Studio) {
        studio.section_enabled = true;
        studio.section_reference_bounds = Some(studio.clouds[0].cloud.bounds);
        studio.section_max_percent[2] = 50.0;
    }

    fn viewport_bounds() -> Rectangle {
        Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0))
    }

    #[test]
    fn the_cut_of_a_wall_is_capped_while_the_box_and_the_fill_are_on() {
        let (mut studio, _directory) = studio_with_wall();
        let state = RefCell::new(RenderCache::default());
        let bounds = viewport_bounds();
        // Frames until the caps that are being made are done.
        let draw = |studio: &Studio| {
            let viewport = GpuViewport {
                overlay: studio.point_viewport(),
            };
            loop {
                let frame =
                    shader::Program::draw(&viewport, &state, mouse::Cursor::Unavailable, bounds);
                if state.borrow().caps_job.is_none() {
                    return frame;
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };

        // Without a section box nothing is cut.
        assert!(draw(&studio).caps.indices.is_empty());
        cut_at_half_height(&mut studio);
        // The first frame does not wait for the caps.
        let viewport = GpuViewport {
            overlay: studio.point_viewport(),
        };
        let first = shader::Program::draw(&viewport, &state, mouse::Cursor::Unavailable, bounds);
        assert!(first.caps.indices.is_empty());
        assert!(state.borrow().caps_job.is_some());
        let capped = draw(&studio);
        assert!(!capped.caps.indices.is_empty());
        let grey = 88.0 / 255.0;
        for vertex in &capped.caps.vertices {
            assert_eq!(vertex.color, [grey, grey, grey, 1.0]);
            assert_eq!(vertex.normal, [0.0; 4]);
        }
        // A turn of the camera keeps the caps; a change of the fill does
        // not.
        studio.yaw += 0.3;
        assert!(Arc::ptr_eq(&capped.caps, &draw(&studio).caps));
        let _ = studio.update(Message::SectionFill(
            crate::section_fill::FillAction::Color("#204060".into()),
        ));
        let blue = draw(&studio);
        assert!(!Arc::ptr_eq(&capped.caps, &blue.caps));
        assert_eq!(
            blue.caps.vertices[0].color,
            [32.0 / 255.0, 64.0 / 255.0, 96.0 / 255.0, 1.0]
        );
        // A wall thicker than the largest thickness is not filled.
        let _ = studio.update(Message::SectionFill(
            crate::section_fill::FillAction::MaxThickness("0.1".into()),
        ));
        assert!(draw(&studio).caps.indices.is_empty());
        let _ = studio.update(Message::SectionFill(
            crate::section_fill::FillAction::MaxThickness("0.5".into()),
        ));
        assert!(!draw(&studio).caps.indices.is_empty());
        let _ = studio.update(Message::SectionFill(
            crate::section_fill::FillAction::Enabled(false),
        ));
        assert!(draw(&studio).caps.indices.is_empty());
    }

    fn send(studio: &mut Studio, command: crate::native_api::ApiCommand) -> serde_json::Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(crate::native_api::ApiRequest { command, reply });
        receive.try_recv().unwrap()
    }

    /// Whether the status reports caps pending, looked at until it no
    /// longer does or ten seconds have passed.
    fn pending_after_waiting(studio: &mut Studio) -> bool {
        let started = std::time::Instant::now();
        loop {
            let status = send(studio, crate::native_api::ApiCommand::Status);
            let pending = &status["result"]["section_fill"]["pending"];
            assert!(pending.is_boolean(), "{status}");
            if *pending == false || started.elapsed() > std::time::Duration::from_secs(10) {
                return *pending == true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn caps_made_while_no_frame_comes_are_no_longer_pending() {
        let (mut studio, _directory) = studio_with_wall();
        cut_at_half_height(&mut studio);
        let state = RefCell::new(RenderCache::default());
        let bounds = viewport_bounds();
        let frame = |studio: &Studio, state: &RefCell<RenderCache>| {
            let viewport = GpuViewport {
                overlay: studio.point_viewport(),
            };
            shader::Program::draw(&viewport, state, mouse::Cursor::Unavailable, bounds)
        };
        // A minimised window: the frame that starts the caps is the last
        // one for a while.
        assert!(frame(&studio, &state).caps.indices.is_empty());
        assert!(state.borrow().caps_job.is_some());
        assert!(!pending_after_waiting(&mut studio));
        // The next frame shows them.
        assert!(!frame(&studio, &state).caps.indices.is_empty());
        assert!(state.borrow().caps_job.is_none());

        // The Drawing view replaces the 3D view while the caps of another
        // box are being made: the view and what it waits for are dropped,
        // and no frame of it follows.
        studio.section_max_percent[2] = 60.0;
        assert!(frame(&studio, &state).caps.indices.is_empty());
        assert!(state.borrow().caps_job.is_some());
        drop(state);
        let shown = send(
            &mut studio,
            crate::native_api::ApiCommand::DrawingView { show: true },
        );
        assert_eq!(shown["ok"], true, "{shown}");
        assert!(!pending_after_waiting(&mut studio));
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
            mesh_topology: None,
            mesh_visible: false,
            faces: None,
            bag_source: false,
            visible: true,
            selection: None,
            deleted: None,
            colours: None,
            index: None,
            auto_index_queued: false,
            picked: false,
            index_building: false,
            detail_points: None,
            focus_points: None,
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
        let unboxed = draw(&studio);
        assert!(!Arc::ptr_eq(&filtered.geometry, &unboxed.geometry));
        assert_eq!(unboxed.geometry.points.len(), 4);
        // The section box changes no point: it is the clip of the shader.
        studio.section_enabled = true;
        studio.section_reference_bounds = Some(studio.clouds[0].cloud.bounds);
        studio.section_max_percent[0] = 0.0;
        let clipped = draw(&studio);
        assert!(Arc::ptr_eq(&unboxed.geometry, &clipped.geometry));
        assert_eq!(clipped.camera.clip_enabled[0], 1.0);
        assert_eq!(unboxed.camera.clip_enabled[0], 0.0);

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
        assert_eq!(colored_mesh.geometry.mesh.vertices.len(), 3);
        assert_eq!(
            colored_mesh.geometry.mesh.vertices[0].color,
            [1.0, 0.0, 0.0, 0.82]
        );
        assert_eq!(
            colored_mesh.geometry.mesh.vertices[1].color[1],
            128.0 / 255.0
        );
        assert_eq!(
            colored_mesh.geometry.mesh.vertices[0].normal,
            [0.0, 0.0, 1.0, 0.0]
        );

        studio.clouds[0].transform.scale = [-1.0, 2.0, 1.0];
        let reflected_mesh = draw(&studio);
        assert_eq!(
            reflected_mesh.geometry.mesh.vertices[0].normal,
            [0.0, 0.0, -1.0, 0.0]
        );

        // The buffers of a mesh are kept while only the points change: a
        // refinement from the octree or a class filter; the section box
        // changes neither. A mesh of millions of triangles is then not built
        // and sent again.
        studio.clouds[0].detail_points = Some(
            vec![IndexedPoint {
                point: studio.clouds[0].cloud.points[1],
                ordinal: 1,
            }]
            .into(),
        );
        let refined = draw(&studio);
        assert!(!Arc::ptr_eq(&reflected_mesh.geometry, &refined.geometry));
        assert!(Arc::ptr_eq(
            &reflected_mesh.geometry.mesh,
            &refined.geometry.mesh
        ));
        studio.section_enabled = true;
        let cut = draw(&studio);
        assert!(Arc::ptr_eq(&refined.geometry, &cut.geometry));
        studio.filter_other = false;
        let unfiltered = draw(&studio);
        assert!(!Arc::ptr_eq(&cut.geometry, &unfiltered.geometry));
        assert!(Arc::ptr_eq(&cut.geometry.mesh, &unfiltered.geometry.mesh));

        // A layer that is scaled, another mesh and a mesh switched off give
        // new buffers.
        studio.clouds[0].transform.scale = [1.0, 2.0, 1.0];
        let scaled = draw(&studio);
        assert!(!Arc::ptr_eq(
            &unfiltered.geometry.mesh,
            &scaled.geometry.mesh
        ));
        assert_eq!(
            scaled.geometry.mesh.vertices[0].normal,
            [0.0, 0.0, 1.0, 0.0]
        );
        studio.clouds[0].mesh = Some(Arc::new(MeshGeometry {
            vertices: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            triangles: vec![[0, 1, 2]],
            colors: None,
            normals: None,
        }));
        let replaced = draw(&studio);
        assert!(!Arc::ptr_eq(&scaled.geometry.mesh, &replaced.geometry.mesh));
        assert_eq!(replaced.geometry.mesh.indices, [0, 1, 2]);
        studio.clouds[0].mesh_visible = false;
        let hidden = draw(&studio);
        assert!(!Arc::ptr_eq(&replaced.geometry.mesh, &hidden.geometry.mesh));
        assert!(hidden.geometry.mesh.indices.is_empty());

        // Detected faces are drawn as one more mesh, placed with their
        // layer, and go when they are switched off.
        let square = Arc::new(MeshGeometry {
            vertices: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            colors: Some(vec![[0, 255, 0]; 4]),
            normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
        });
        studio.clouds[0].faces = Some(crate::faces::FaceLayer::showing(square, true));
        let with_faces = draw(&studio);
        assert!(!Arc::ptr_eq(&hidden.geometry, &with_faces.geometry));
        assert_eq!(with_faces.geometry.mesh.indices, [0, 1, 2, 0, 2, 3]);
        assert_eq!(
            with_faces.geometry.mesh.vertices[0].color,
            [0.0, 1.0, 0.0, 0.82]
        );
        // The layer is scaled by two along Y: so are its faces.
        let corner = with_faces.geometry.mesh.vertices[2].relative;
        let origin = with_faces.geometry.mesh.vertices[0].relative;
        assert_eq!([corner[0] - origin[0], corner[1] - origin[1]], [1.0, 2.0]);
        // Beside the mesh of the layer they come second.
        studio.clouds[0].mesh_visible = true;
        let both = draw(&studio);
        assert_eq!(both.geometry.mesh.indices, [0, 1, 2, 3, 4, 5, 3, 5, 6]);
        studio.clouds[0].mesh_visible = false;
        studio.clouds[0].faces = None;
        let without = draw(&studio);
        assert!(without.geometry.mesh.indices.is_empty());
    }

    #[test]
    fn geometry_build_feeds_the_pace() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("pace.xyz");
        std::fs::write(&source, "0 0 0\n1 0 0\n0 1 0\n1 1 1\n").unwrap();
        let cloud = pointcloud_core::open(&source, 4).unwrap();
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::new(cloud))));
        let bounds = Rectangle::new(iced::Point::ORIGIN, iced::Size::new(800.0, 600.0));
        let draw = |studio: &Studio| {
            let viewport = GpuViewport {
                overlay: studio.point_viewport(),
            };
            let state = RefCell::new(RenderCache::default());
            shader::Program::draw(&viewport, &state, mouse::Cursor::Unavailable, bounds)
        };

        // Too few points to say anything about the pace.
        assert_eq!(draw(&studio).geometry.points.len(), 4);
        assert_eq!(studio.lod_pace.build_points_per_ms(), None);

        let detail: Vec<IndexedPoint> = (0..140_000u64)
            .map(|ordinal| IndexedPoint {
                point: studio.clouds[0].cloud.points[(ordinal % 4) as usize],
                ordinal,
            })
            .collect();
        studio.clouds[0].detail_points = Some(detail.into());
        // A build thinned to the budget is not a measure of a full one.
        studio.budget = 70_000;
        assert_eq!(draw(&studio).geometry.points.len(), 70_000);
        assert_eq!(studio.lod_pace.build_points_per_ms(), None);

        // A section box that shows a quarter of the points: all of them are
        // built, as the shader clips them, and walked for the pace.
        studio.budget = 140_000;
        studio.section_enabled = true;
        studio.section_min_percent[2] = 50.0;
        assert_eq!(draw(&studio).geometry.points.len(), 140_000);
        assert!(studio.lod_pace.build_points_per_ms().is_some());

        studio.section_enabled = false;
        assert_eq!(draw(&studio).geometry.points.len(), 140_000);
        assert!(studio
            .lod_pace
            .build_points_per_ms()
            .is_some_and(|pace| pace > 0.0));
    }
}

#[cfg(test)]
mod section_box_tests {
    use super::*;
    use crate::selection::{pick_displayed, pick_surface, PickTarget, PickView};
    use crate::{CameraPreset, Studio};
    use std::sync::atomic::AtomicBool;

    /// A scan of 10 by 10 by 10 points in the middle of the unit cubes from
    /// 0 to 10, and a column of ten more at the middle of the scene, seen in
    /// a view of 800 by 600 pixels.
    fn studio_with_grid() -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("grid.xyz");
        let mut text = String::new();
        for x in 0..10 {
            for y in 0..10 {
                for z in 0..10 {
                    text.push_str(&format!("{x}.5 {y}.5 {z}.5\n"));
                }
            }
        }
        for z in 0..10 {
            text.push_str(&format!("5 5 {z}.5\n"));
        }
        std::fs::write(&source, text).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 2_000).unwrap());
        assert_eq!(cloud.points.len(), 1_010);
        let mut studio = Studio {
            viewport_size: iced::Size::new(800.0, 600.0),
            ..Studio::default()
        };
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        (studio, directory)
    }

    /// The records of the column in the middle and those of the grid.
    fn column_and_grid(studio: &Studio) -> (Vec<IndexedPoint>, Vec<IndexedPoint>) {
        let (column, grid): (Vec<_>, Vec<_>) = studio.clouds[0]
            .view_records()
            .partition(|record| record.point.xyz[..2] == [5.0, 5.0]);
        assert_eq!((column.len(), grid.len()), (10, 1_000));
        (column, grid)
    }

    /// The points of the scan that lie inside the section box, all of them
    /// while it is off.
    fn inside(studio: &Studio) -> usize {
        let section = studio.section_box();
        studio.clouds[0]
            .view_records()
            .chain(studio.clouds[0].focus_records())
            .filter(|record| section.is_none_or(|section| section.contains(record.point.xyz)))
            .count()
    }

    #[test]
    fn the_section_box_is_a_clip_of_the_shader_and_sends_no_point() {
        let (mut studio, _directory) = studio_with_grid();
        let state = RefCell::new(RenderCache::default());
        let open = drawn_frame(&studio, &state);
        assert_eq!(open.drawn, 1_010);

        // Switching the box on, moving a face with its slider and with its
        // handle, turning it and switching it off: every frame draws the
        // points it drew before, and the shader keeps exactly those inside
        // the box.
        type Change = Box<dyn Fn(&mut Studio)>;
        let changes: [(&str, Change); 5] = [
            (
                "on",
                Box::new(|studio| {
                    let _ = studio.update(Message::SetSectionEnabled(true));
                }),
            ),
            (
                "slider",
                Box::new(|studio| {
                    let _ = studio.update(Message::SectionMax(0, 55.0));
                }),
            ),
            (
                "handle",
                Box::new(|studio| {
                    let _ = studio.update(Message::SectionHandleDelta(2, true, 31.0));
                }),
            ),
            (
                "turn",
                Box::new(|studio| assert!(studio.turn_section(30.0))),
            ),
            (
                "off",
                Box::new(|studio| {
                    let _ = studio.update(Message::SetSectionEnabled(false));
                }),
            ),
        ];
        let mut counts = Vec::new();
        for (change, apply) in changes {
            apply(&mut studio);
            let frame = drawn_frame(&studio, &state);
            assert_eq!(
                (frame.points, frame.focus),
                (open.points, open.focus),
                "{change} sent points to the device"
            );
            assert_eq!(frame.drawn, inside(&studio), "{change}");
            counts.push(frame.drawn);
        }
        // The box did clip: on it holds everything, the slider and the
        // handle take points away, the turn changes what is inside, and off
        // shows all again.
        assert_eq!(counts[0], 1_010);
        assert!(counts[1] < counts[0] && counts[2] < counts[1], "{counts:?}");
        assert_ne!(counts[3], counts[2]);
        assert_eq!(counts[4], 1_010);
    }

    #[test]
    fn detail_read_inside_the_box_is_sent_on_its_own() {
        let (mut studio, _directory) = studio_with_grid();
        let state = RefCell::new(RenderCache::default());
        let (column, grid) = column_and_grid(&studio);
        studio.clouds[0].detail_points = Some(grid.clone().into());
        let view = drawn_frame(&studio, &state);
        assert_eq!(view.drawn, 1_000);

        // The column arrives as detail read inside the box: only it is sent.
        studio.clouds[0].focus_points = Some(column.into());
        let added = drawn_frame(&studio, &state);
        assert_eq!(added.points, view.points);
        assert_ne!(added.focus, view.focus);
        assert_eq!(added.drawn, 1_010);

        // New sets for the view leave it on the device.
        studio.clouds[0].detail_points = Some(grid[..900].into());
        let refined = drawn_frame(&studio, &state);
        assert_ne!(refined.points, added.points);
        assert_eq!(refined.focus, added.focus);
        assert_eq!(refined.drawn, 910);

        // A smaller budget thins the sets of the view at once, and leaves
        // the points read inside the box as they are.
        studio.budget = 450;
        let thinned = drawn_frame(&studio, &state);
        assert_ne!(thinned.points, refined.points);
        assert_eq!(thinned.focus, refined.focus);
        assert_eq!(thinned.drawn, 460);

        // A colour mode or a class filter changes the points of both.
        studio.color_mode = ColorMode::Elevation;
        let recoloured = drawn_frame(&studio, &state);
        assert_ne!(recoloured.points, refined.points);
        assert_ne!(recoloured.focus, refined.focus);
        studio.filter_other = false;
        assert_eq!(drawn_frame(&studio, &state).drawn, 0);
    }

    #[test]
    fn picking_honours_the_box_that_only_the_shader_applies() {
        let (mut studio, _directory) = studio_with_grid();
        // From above, the column in the middle of the scene stands on one
        // pixel, and its highest point is the nearest to the eye.
        let (yaw, pitch, _) = CameraPreset::Top.orientation();
        (studio.yaw, studio.pitch) = (yaw, pitch);
        let picked = |studio: &Studio| {
            let scene = crate::combined_bounds(&studio.clouds).unwrap();
            let projection = studio.projection(scene, 800.0, 600.0);
            let (x, y, _) = projection.project([5.0, 5.0, 0.5]).unwrap();
            let views: Vec<_> = studio.clouds.iter().map(PickView::of).collect();
            let target = PickTarget {
                pointer: [x, y],
                radius: 1.0,
                sphere_radius: 0.0,
            };
            let budget = studio.budget as usize;
            let filter = studio.mesh_filter();
            let point = pick_displayed(
                &views,
                0,
                budget,
                projection,
                target,
                filter,
                &AtomicBool::new(false),
            )
            .unwrap()
            .map(|record| record.point.xyz);
            let surface = pick_surface(&views, budget, projection, [x, y], 1.0, filter);
            assert_eq!(point, surface);
            point.map(|xyz| xyz[2])
        };
        assert_eq!(picked(&studio), Some(9.5));

        // The box ends at half the height: the points above it are still
        // sent to the device, but neither drawn nor picked.
        let _ = studio.update(Message::SetSectionEnabled(true));
        let _ = studio.update(Message::SectionMax(2, 50.0));
        let state = RefCell::new(RenderCache::default());
        let frame = drawn_frame(&studio, &state);
        assert_eq!(frame.drawn, inside(&studio));
        assert!(frame.drawn < 1_010);
        assert_eq!(picked(&studio), Some(4.5));

        // The column read inside the box beside the sets of the view is
        // drawn whole, and picked as such.
        let (column, grid) = column_and_grid(&studio);
        studio.clouds[0].detail_points = Some(grid.into());
        studio.clouds[0].focus_points = Some(column.into());
        assert_eq!(picked(&studio), Some(4.5));
        let _ = studio.update(Message::SetSectionEnabled(false));
        assert_eq!(picked(&studio), Some(9.5));
    }
}
