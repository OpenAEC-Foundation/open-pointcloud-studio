use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::CloudTransform;
use pointcloud_core::{
    visit_points, Bounds, IndexedPoint, LoadError, OctreeIndex, Point, PointCloud,
};

const HIGHLIGHT_LIMIT: usize = 8_000;

#[derive(Clone, Copy)]
pub(crate) struct PickTarget {
    pub pointer: [f32; 2],
    pub radius: f32,
    pub sphere_radius: f32,
}

type PickHit = (IndexedPoint, bool, f64, f32);

/// A read-only snapshot of one layer's current viewport sample. The renderer
/// and point picker must use the same sample order and deletion mask.
pub(crate) struct PickView {
    pub cloud: Arc<PointCloud>,
    pub detail: Option<Arc<[IndexedPoint]>>,
    pub deleted: Option<Arc<DeletionMask>>,
    pub transform: CloudTransform,
    pub visible: bool,
}

impl PickView {
    fn len(&self) -> usize {
        self.detail
            .as_ref()
            .map_or(self.cloud.points.len(), |points| points.len())
    }

    fn records(&self) -> Box<dyn Iterator<Item = IndexedPoint> + '_> {
        let transform = self.transform;
        if let Some(detail) = &self.detail {
            Box::new(
                detail
                    .iter()
                    .copied()
                    .map(move |record| transform.record(record)),
            )
        } else {
            Box::new(
                self.cloud
                    .points
                    .iter()
                    .copied()
                    .zip(self.cloud.point_ordinals.iter().copied())
                    .map(move |(point, ordinal)| transform.record(IndexedPoint { point, ordinal })),
            )
        }
    }

    fn record_visible(&self, record: IndexedPoint) -> bool {
        self.deleted
            .as_ref()
            .is_none_or(|mask| record.ordinal != u64::MAX && !mask.contains(record.ordinal))
    }
}

#[derive(Debug, Clone)]
pub struct SelectionMask {
    pub bits: Vec<u64>,
    pub count: u64,
    pub highlights: Vec<Point>,
    /// False only when a degenerate transform prevents recovering a picked source point.
    pub highlights_source: bool,
    /// Exact source-coordinate extent, independent of later live transforms.
    pub source_bounds: Option<Bounds>,
}

impl SelectionMask {
    fn new(total_points: u64) -> Result<Self, String> {
        let words = total_points.div_ceil(64);
        let words = usize::try_from(words).map_err(|_| "point count exceeds address space")?;
        Ok(Self {
            bits: vec![0; words],
            count: 0,
            highlights: Vec::with_capacity(HIGHLIGHT_LIMIT),
            highlights_source: true,
            source_bounds: None,
        })
    }

    pub fn contains(&self, ordinal: u64) -> bool {
        let word = usize::try_from(ordinal / 64).ok();
        word.and_then(|index| self.bits.get(index))
            .is_some_and(|bits| bits & (1u64 << (ordinal % 64)) != 0)
    }

    fn insert(&mut self, ordinal: u64, point: Point, source_xyz: [f64; 3], random_state: &mut u64) {
        self.bits[(ordinal / 64) as usize] |= 1u64 << (ordinal % 64);
        self.count += 1;
        if let Some(bounds) = &mut self.source_bounds {
            for (axis, value) in source_xyz.into_iter().enumerate() {
                bounds.min[axis] = bounds.min[axis].min(value);
                bounds.max[axis] = bounds.max[axis].max(value);
            }
        } else {
            self.source_bounds = Some(Bounds {
                min: source_xyz,
                max: source_xyz,
            });
        }
        let mut source_point = point;
        source_point.xyz = source_xyz;
        if self.highlights.len() < HIGHLIGHT_LIMIT {
            self.highlights.push(source_point);
        } else {
            *random_state ^= *random_state << 13;
            *random_state ^= *random_state >> 7;
            *random_state ^= *random_state << 17;
            let replacement = *random_state % self.count;
            if replacement < HIGHLIGHT_LIMIT as u64 {
                self.highlights[replacement as usize] = source_point;
            }
        }
    }

    #[cfg(test)]
    pub fn single(total_points: u64, record: IndexedPoint) -> Result<Self, String> {
        Self::single_with_source(total_points, record, Some(record.point.xyz))
    }

    pub fn single_with_source(
        total_points: u64,
        record: IndexedPoint,
        source_xyz: Option<[f64; 3]>,
    ) -> Result<Self, String> {
        if record.ordinal >= total_points {
            return Err("indexed point number is outside the source".into());
        }
        let mut mask = Self::new(total_points)?;
        mask.insert(
            record.ordinal,
            record.point,
            source_xyz.unwrap_or(record.point.xyz),
            &mut 0,
        );
        if source_xyz.is_none() {
            mask.source_bounds = None;
            mask.highlights_source = false;
        }
        Ok(mask)
    }

    /// Mark an exact, evenly distributed fraction of the currently visible
    /// source ordinals for removal without reading or copying point records.
    pub fn thin_removed(
        total_points: u64,
        deleted: Option<&DeletionMask>,
        percent: u8,
    ) -> Result<Self, String> {
        let expected_words = usize::try_from(total_points.div_ceil(64))
            .map_err(|_| "point count exceeds address space")?;
        if !(1..=100).contains(&percent)
            || deleted.is_some_and(|mask| mask.bits.len() != expected_words)
        {
            return Err("invalid thin percentage or deletion mask".into());
        }
        let remaining = total_points - deleted.map_or(0, |mask| mask.count);
        let target = if remaining == 0 {
            0
        } else {
            ((u128::from(remaining) * u128::from(percent) + 50) / 100) as u64
        }
        .max(u64::from(remaining > 0))
        .min(remaining);
        let mut removed = Self::new(total_points)?;
        let mut seen = 0u64;
        for ordinal in 0..total_points {
            if deleted.is_some_and(|mask| mask.contains(ordinal)) {
                continue;
            }
            let before = u128::from(seen) * u128::from(target) / u128::from(remaining);
            seen += 1;
            let after = u128::from(seen) * u128::from(target) / u128::from(remaining);
            if after == before {
                removed.bits[(ordinal / 64) as usize] |= 1u64 << (ordinal % 64);
                removed.count += 1;
            }
        }
        Ok(removed)
    }
}

/// Non-destructive edit state keyed by the original file's point ordinals.
/// The source and octree remain unchanged, so undo and re-sampling are exact.
#[derive(Debug, Clone)]
pub struct DeletionMask {
    bits: Vec<u64>,
    pub count: u64,
}

impl DeletionMask {
    pub fn new(total_points: u64) -> Result<Self, String> {
        let words = usize::try_from(total_points.div_ceil(64))
            .map_err(|_| "point count exceeds address space")?;
        Ok(Self {
            bits: vec![0; words],
            count: 0,
        })
    }

    pub fn contains(&self, ordinal: u64) -> bool {
        usize::try_from(ordinal / 64)
            .ok()
            .and_then(|index| self.bits.get(index))
            .is_some_and(|bits| bits & (1u64 << (ordinal % 64)) != 0)
    }

    /// A selection can reuse its exact cached bounds when no selected source
    /// ordinal has since been hidden. Compare masks without rereading points.
    pub fn overlaps_selection(&self, selection: &SelectionMask) -> bool {
        self.bits.len() != selection.bits.len()
            || self
                .bits
                .iter()
                .zip(&selection.bits)
                .any(|(deleted, selected)| deleted & selected != 0)
    }

    pub fn apply(&mut self, selection: &SelectionMask) -> Result<u64, String> {
        if self.bits.len() != selection.bits.len() {
            return Err("selection belongs to a different source size".into());
        }
        let mut added = 0;
        for (hidden, selected) in self.bits.iter_mut().zip(&selection.bits) {
            added += (selected & !*hidden).count_ones() as u64;
            *hidden |= selected;
        }
        self.count += added;
        Ok(added)
    }

    pub fn undo(&mut self, selection: &SelectionMask) -> Result<u64, String> {
        if self.bits.len() != selection.bits.len() {
            return Err("selection belongs to a different source size".into());
        }
        let mut restored = 0;
        for (hidden, selected) in self.bits.iter_mut().zip(&selection.bits) {
            restored += (*hidden & selected).count_ones() as u64;
            *hidden &= !selected;
        }
        self.count -= restored;
        Ok(restored)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ClassFilter {
    pub ground: bool,
    pub vegetation: bool,
    pub buildings: bool,
    pub other: bool,
    pub classes: ClassVisibility,
    pub section: Option<Bounds>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassVisibility([u64; 4]);

impl Default for ClassVisibility {
    fn default() -> Self {
        Self([u64::MAX; 4])
    }
}

impl ClassVisibility {
    pub fn allows(self, code: Option<u8>) -> bool {
        code.is_none_or(|code| {
            let word = usize::from(code / 64);
            self.0[word] & (1u64 << (code % 64)) != 0
        })
    }

    pub fn set(&mut self, code: u8, visible: bool) {
        let word = usize::from(code / 64);
        let bit = 1u64 << (code % 64);
        if visible {
            self.0[word] |= bit;
        } else {
            self.0[word] &= !bit;
        }
    }
}

impl ClassFilter {
    pub fn accepts(self, point: &Point) -> bool {
        if let Some(section) = self.section {
            if (0..3).any(|axis| {
                point.xyz[axis] < section.min[axis] || point.xyz[axis] > section.max[axis]
            }) {
                return false;
            }
        }
        if !self.classes.allows(point.classification) {
            return false;
        }
        match point.classification {
            Some(2) => self.ground,
            Some(3..=5) => self.vegetation,
            Some(6) => self.buildings,
            _ => self.other,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScreenRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl ScreenRect {
    pub fn from_corners(start: [f32; 2], end: [f32; 2]) -> Self {
        let (mut left, mut right) = (start[0].min(end[0]), start[0].max(end[0]));
        let (mut top, mut bottom) = (start[1].min(end[1]), start[1].max(end[1]));
        if right - left < 6.0 {
            left -= 3.0;
            right += 3.0;
        }
        if bottom - top < 6.0 {
            top -= 3.0;
            bottom += 3.0;
        }
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    fn contains(self, x: f32, y: f32) -> bool {
        (self.left..=self.right).contains(&x) && (self.top..=self.bottom).contains(&y)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Projection {
    center: [f64; 3],
    pub(crate) right: [f64; 3],
    pub(crate) up: [f64; 3],
    pub(crate) toward_camera: [f64; 3],
    /// Eye position relative to the centre, measured along `right`, `up` and
    /// `toward_camera`. An orbit camera sits `distance` along the last axis.
    pub(crate) eye: [f64; 3],
    /// A quarter of the depth range the renderer resolves.
    pub(crate) distance: f64,
    pub(crate) scale: f64,
    width: f64,
    height: f64,
    pan: [f32; 2],
}

impl Projection {
    pub fn new(
        bounds: Bounds,
        yaw: f32,
        pitch: f32,
        zoom: f32,
        pan: [f32; 2],
        width: f32,
        height: f32,
    ) -> Self {
        let (sy, cy) = (yaw as f64).sin_cos();
        let (sp, cp) = (pitch as f64).sin_cos();
        let distance = bounds.extent().max(0.001) * 1.8;
        Self {
            center: bounds.center(),
            right: [-sy, cy, 0.0],
            up: [-sp * cy, -sp * sy, cp],
            toward_camera: [cp * cy, cp * sy, sp],
            eye: [0.0, 0.0, distance],
            distance,
            scale: height.min(width) as f64 * 1.25 / zoom as f64,
            width: width as f64,
            height: height as f64,
            pan,
        }
    }

    /// A camera at a free eye position: `basis` holds its right, up and
    /// forward directions and `focal` its focal length in pixels.
    pub fn from_eye(
        bounds: Bounds,
        eye: [f64; 3],
        basis: [[f64; 3]; 3],
        focal: f32,
        width: f32,
        height: f32,
    ) -> Self {
        let center = bounds.center();
        let [right, up, forward] = basis;
        let toward_camera = forward.map(|value| -value);
        let offset: [f64; 3] = std::array::from_fn(|axis| eye[axis] - center[axis]);
        // Everything in the scene must fit in the resolved depth range,
        // wherever the eye stands.
        let reach = (0..3)
            .map(|axis| (bounds.max[axis] - bounds.min[axis]).powi(2))
            .sum::<f64>()
            .sqrt()
            + dot(offset, offset).sqrt();
        Self {
            center,
            right,
            up,
            toward_camera,
            eye: [
                dot(offset, right),
                dot(offset, up),
                dot(offset, toward_camera),
            ],
            distance: reach.max(0.004) * 0.25,
            scale: f64::from(focal),
            width: f64::from(width),
            height: f64::from(height),
            pan: [0.0, 0.0],
        }
    }

    pub fn project(self, xyz: [f64; 3]) -> Option<(f32, f32, f64)> {
        let (x, y, depth) = self.project_unclipped(xyz)?;
        if x < 0.0 || x >= self.width as f32 || y < 0.0 || y >= self.height as f32 {
            return None;
        }
        Some((x, y, depth))
    }

    pub fn project_unclipped(self, xyz: [f64; 3]) -> Option<(f32, f32, f64)> {
        let relative = std::array::from_fn(|axis| xyz[axis] - self.center[axis]);
        let depth = self.eye[2] - dot(relative, self.toward_camera);
        if depth <= 0.01 {
            return None;
        }
        let x = self.width * 0.5
            + self.pan[0] as f64
            + (dot(relative, self.right) - self.eye[0]) * self.scale / depth;
        let y = self.height * 0.5 + self.pan[1] as f64
            - (dot(relative, self.up) - self.eye[1]) * self.scale / depth;
        Some((x as f32, y as f32, depth))
    }

    /// Distance of a position in front of the eye along the viewing
    /// direction; `project_unclipped` rejects depths of 0.01 or less.
    pub fn depth(self, xyz: [f64; 3]) -> f64 {
        let relative = std::array::from_fn(|axis| xyz[axis] - self.center[axis]);
        self.eye[2] - dot(relative, self.toward_camera)
    }

    fn projected_extents(self, bounds: Bounds) -> Option<[f32; 4]> {
        let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
        for corner in 0..8 {
            let xyz = std::array::from_fn(|axis| {
                if corner & (1 << axis) == 0 {
                    bounds.min[axis]
                } else {
                    bounds.max[axis]
                }
            });
            if let Some((x, y, _)) = self.project_unclipped(xyz) {
                if x.is_finite() && y.is_finite() {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        min_x.is_finite().then_some([min_x, max_x, min_y, max_y])
    }

    pub fn screen_span(self, bounds: Bounds) -> Option<f32> {
        let Some([min_x, max_x, min_y, max_y]) = self.projected_extents(bounds) else {
            return Some(f32::MAX);
        };
        if max_x < 0.0 || min_x > self.width as f32 || max_y < 0.0 || min_y > self.height as f32 {
            return None;
        }
        Some((max_x - min_x).max(max_y - min_y).max(1.0))
    }

    /// Pixel area covered by a projected node after clipping to the viewport.
    /// Used to share a bounded LOD budget between open scans.
    pub fn screen_coverage(self, bounds: Bounds) -> Option<f32> {
        let Some([min_x, max_x, min_y, max_y]) = self.projected_extents(bounds) else {
            return Some((self.width * self.height).max(1.0) as f32);
        };
        if max_x < 0.0 || min_x > self.width as f32 || max_y < 0.0 || min_y > self.height as f32 {
            return None;
        }
        let width = (max_x.min(self.width as f32) - min_x.max(0.0)).max(0.0);
        let height = (max_y.min(self.height as f32) - min_y.max(0.0)).max(0.0);
        Some((width * height).max(1.0))
    }
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[cfg(test)]
pub fn pick_indexed(
    tree: &OctreeIndex,
    projection: Projection,
    pointer: [f32; 2],
    radius: f32,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
) -> Result<Option<IndexedPoint>, String> {
    pick_indexed_transformed(
        tree,
        projection,
        PickTarget {
            pointer,
            radius,
            sphere_radius: 0.0,
        },
        filter,
        deleted,
        CloudTransform::default(),
        &AtomicBool::new(false),
    )
}

pub fn pick_indexed_transformed(
    tree: &OctreeIndex,
    projection: Projection,
    target: PickTarget,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
    transform: CloudTransform,
    cancel: &AtomicBool,
) -> Result<Option<IndexedPoint>, String> {
    let PickTarget {
        pointer,
        radius,
        sphere_radius,
    } = target;
    validate_pick(pointer, radius)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    let mut best: Option<PickHit> = None;
    let search_radius = radius.max(sphere_radius);
    let mut visited = 0u64;
    tree.visit_intersecting(
        |bounds| {
            !cancel.load(Ordering::Relaxed)
                && node_overlaps_pointer(
                    transform.bounds(bounds),
                    projection,
                    pointer,
                    search_radius,
                )
        },
        |record| {
            visited += 1;
            if visited & 0xfff == 0 && cancel.load(Ordering::Relaxed) {
                return Err(LoadError::Cancelled);
            }
            consider_pick(
                transform.record(record),
                projection,
                target,
                filter,
                deleted,
                &mut best,
            );
            Ok(())
        },
    )
    .map_err(|error| error.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    Ok(best.map(|(record, _, _, _)| record))
}

/// Pick directly from the source when no disk octree has been built yet.
#[cfg(test)]
pub fn pick_full(
    cloud: &PointCloud,
    projection: Projection,
    pointer: [f32; 2],
    radius: f32,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
) -> Result<Option<IndexedPoint>, String> {
    pick_full_transformed(
        cloud,
        projection,
        PickTarget {
            pointer,
            radius,
            sphere_radius: 0.0,
        },
        filter,
        deleted,
        CloudTransform::default(),
        &AtomicBool::new(false),
    )
}

pub fn pick_full_transformed(
    cloud: &PointCloud,
    projection: Projection,
    target: PickTarget,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
    transform: CloudTransform,
    cancel: &AtomicBool,
) -> Result<Option<IndexedPoint>, String> {
    let PickTarget {
        pointer, radius, ..
    } = target;
    validate_pick(pointer, radius)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    let mut best = None;
    let mut ordinal = 0u64;
    visit_points(&cloud.path, &mut |point| {
        if ordinal & 0xfff == 0 && cancel.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        consider_pick(
            transform.record(IndexedPoint { point, ordinal }),
            projection,
            target,
            filter,
            deleted,
            &mut best,
        );
        ordinal += 1;
        Ok(())
    })
    .map_err(|error| error.to_string())?;
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    if ordinal != cloud.total_points {
        return Err(format!("{} changed while picking", cloud.path.display()));
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    Ok(best.map(|(record, _, _, _)| record))
}

fn validate_pick(pointer: [f32; 2], radius: f32) -> Result<(), String> {
    if !pointer.iter().all(|value| value.is_finite()) || !radius.is_finite() || radius <= 0.0 {
        return Err("invalid point-pick position".into());
    }
    Ok(())
}

/// Prefer the point actually drawn under the cursor. The renderer strides the
/// flattened visible-layer stream before class and section filtering, so the
/// same stride and ordering are applied here before any full-source fallback.
pub(crate) fn pick_displayed(
    views: &[PickView],
    active: usize,
    budget: usize,
    projection: Projection,
    target: PickTarget,
    filter: ClassFilter,
    cancel: &AtomicBool,
) -> Result<Option<IndexedPoint>, String> {
    validate_pick(target.pointer, target.radius)?;
    let sampled: usize = views
        .iter()
        .filter(|view| view.visible)
        .map(PickView::len)
        .sum();
    let stride = sampled.div_ceil(budget.max(1)).max(1);
    let mut position = 0usize;
    let mut best = None;
    for (index, view) in views.iter().enumerate() {
        if !view.visible {
            continue;
        }
        for record in view.records().filter(|record| view.record_visible(*record)) {
            if position & 0xfff == 0 && cancel.load(Ordering::Relaxed) {
                return Err(LoadError::Cancelled.to_string());
            }
            if index == active && position.is_multiple_of(stride) {
                consider_pick(record, projection, target, filter, None, &mut best);
            }
            position += 1;
        }
        if index == active {
            break;
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    Ok(best.map(|(record, _, _, _)| record))
}

fn consider_pick(
    record: IndexedPoint,
    projection: Projection,
    target: PickTarget,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
    best: &mut Option<PickHit>,
) {
    let PickTarget {
        pointer,
        radius,
        sphere_radius,
    } = target;
    if deleted.is_some_and(|mask| mask.contains(record.ordinal)) || !filter.accepts(&record.point) {
        return;
    }
    if let Some((x, y, depth)) = projection.project_unclipped(record.point.xyz) {
        let dx = x - pointer[0];
        let dy = y - pointer[1];
        let distance_squared = dx * dx + dy * dy;
        let sphere_hit = sphere_radius > 0.0 && distance_squared <= sphere_radius * sphere_radius;
        if distance_squared > radius.max(sphere_radius).powi(2) {
            return;
        }
        let surface_depth = if sphere_hit {
            let hemisphere = (1.0 - distance_squared / sphere_radius.powi(2))
                .max(0.0)
                .sqrt() as f64;
            (depth - f64::from(sphere_radius) * depth / projection.scale * hemisphere).max(0.01)
        } else {
            depth
        };
        let better = best
            .as_ref()
            .is_none_or(|(_, best_sphere, best_depth, best_distance)| {
                if sphere_hit != *best_sphere {
                    sphere_hit
                } else if sphere_hit {
                    surface_depth < *best_depth
                        || (surface_depth == *best_depth && distance_squared < *best_distance)
                } else {
                    distance_squared < *best_distance
                        || (distance_squared == *best_distance && depth < *best_depth)
                }
            });
        if better {
            *best = Some((record, sphere_hit, surface_depth, distance_squared));
        }
    }
}

fn node_overlaps_pointer(
    bounds: Bounds,
    projection: Projection,
    pointer: [f32; 2],
    radius: f32,
) -> bool {
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    let mut projected = 0usize;
    for corner in 0..8 {
        let xyz = std::array::from_fn(|axis| {
            if corner & (1 << axis) == 0 {
                bounds.min[axis]
            } else {
                bounds.max[axis]
            }
        });
        if let Some((x, y, _)) = projection.project_unclipped(xyz) {
            if x.is_finite() && y.is_finite() {
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
                projected += 1;
            }
        }
    }
    projected < 8
        || (max_x >= pointer[0] - radius
            && min_x <= pointer[0] + radius
            && max_y >= pointer[1] - radius
            && min_y <= pointer[1] + radius)
}

fn node_overlaps_rectangle(bounds: Bounds, projection: Projection, rectangle: ScreenRect) -> bool {
    let (mut min_x, mut max_x) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for corner in 0..8 {
        let xyz = std::array::from_fn(|axis| {
            if corner & (1 << axis) == 0 {
                bounds.min[axis]
            } else {
                bounds.max[axis]
            }
        });
        let Some((x, y, _)) = projection.project_unclipped(xyz) else {
            // Keep near-plane crossings rather than discard possible hits.
            return true;
        };
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    max_x >= rectangle.left
        && min_x <= rectangle.right
        && max_y >= rectangle.top
        && min_y <= rectangle.bottom
}

pub struct SelectionSource {
    pub index: usize,
    pub cloud: Arc<PointCloud>,
    pub tree: Option<Arc<OctreeIndex>>,
    pub deleted: Option<Arc<DeletionMask>>,
    pub transform: CloudTransform,
}

#[cfg(test)]
pub fn select_full(
    sources: Vec<SelectionSource>,
    projection: Projection,
    rectangle: ScreenRect,
    filter: ClassFilter,
) -> Result<Vec<(usize, Arc<SelectionMask>)>, String> {
    select_full_cancellable(
        sources,
        projection,
        rectangle,
        filter,
        Arc::new(AtomicBool::new(false)),
    )
}

pub fn select_full_cancellable(
    sources: Vec<SelectionSource>,
    projection: Projection,
    rectangle: ScreenRect,
    filter: ClassFilter,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<(usize, Arc<SelectionMask>)>, String> {
    let workers: Vec<_> = sources
        .into_iter()
        .map(|source| {
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || select_one(source, projection, rectangle, filter, &cancel))
        })
        .collect();
    let mut result = Vec::with_capacity(workers.len());
    for worker in workers {
        result.push(worker.join().map_err(|_| "selection worker panicked")??);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    Ok(result)
}

/// Select exact source points inside a world-coordinate box. Indexed files
/// only visit intersecting leaves; unindexed files stream the entire source.
#[cfg(test)]
pub fn select_world(
    sources: Vec<SelectionSource>,
    bounds: Bounds,
    filter: ClassFilter,
) -> Result<Vec<(usize, Arc<SelectionMask>)>, String> {
    select_world_cancellable(sources, bounds, filter, Arc::new(AtomicBool::new(false)))
}

pub fn select_world_cancellable(
    sources: Vec<SelectionSource>,
    bounds: Bounds,
    filter: ClassFilter,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<(usize, Arc<SelectionMask>)>, String> {
    if !(0..3).all(|axis| {
        bounds.min[axis].is_finite()
            && bounds.max[axis].is_finite()
            && bounds.min[axis] <= bounds.max[axis]
    }) {
        return Err("selection bounds must be finite and ordered".into());
    }
    let workers: Vec<_> = sources
        .into_iter()
        .map(|source| {
            let cancel = Arc::clone(&cancel);
            std::thread::spawn(move || select_one_world(source, bounds, filter, &cancel))
        })
        .collect();
    let mut result = Vec::with_capacity(workers.len());
    for worker in workers {
        result.push(worker.join().map_err(|_| "selection worker panicked")??);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    Ok(result)
}

fn bounds_overlap(a: Bounds, b: Bounds) -> bool {
    (0..3).all(|axis| a.max[axis] >= b.min[axis] && a.min[axis] <= b.max[axis])
}

fn select_one_world(
    source: SelectionSource,
    bounds: Bounds,
    filter: ClassFilter,
    cancel: &AtomicBool,
) -> Result<(usize, Arc<SelectionMask>), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    let SelectionSource {
        index,
        cloud,
        tree,
        deleted,
        transform,
    } = source;
    cloud.validate_source().map_err(|error| error.to_string())?;
    let mut mask = SelectionMask::new(cloud.total_points)?;
    let mut random_state = 0xd1b5_4a32_d192_ed03u64;
    let mut visited = 0u64;
    let mut consider = |ordinal: u64, source_point: Point| {
        let point = transform.point(source_point);
        if !deleted.as_ref().is_some_and(|mask| mask.contains(ordinal))
            && filter.accepts(&point)
            && (0..3).all(|axis| {
                point.xyz[axis] >= bounds.min[axis] && point.xyz[axis] <= bounds.max[axis]
            })
        {
            mask.insert(ordinal, point, source_point.xyz, &mut random_state);
        }
    };
    if let Some(tree) = tree {
        tree.visit_intersecting(
            |node| {
                !cancel.load(Ordering::Relaxed)
                    && bounds_overlap(transform.bounds(node), bounds)
                    && filter
                        .section
                        .is_none_or(|section| bounds_overlap(transform.bounds(node), section))
            },
            |record| {
                visited += 1;
                if visited & 0xfff == 0 && cancel.load(Ordering::Relaxed) {
                    return Err(LoadError::Cancelled);
                }
                consider(record.ordinal, record.point);
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;
    } else {
        let mut ordinal = 0u64;
        visit_points(&cloud.path, &mut |point| {
            if ordinal & 0xfff == 0 && cancel.load(Ordering::Relaxed) {
                return Err(LoadError::Cancelled);
            }
            consider(ordinal, point);
            ordinal += 1;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        if ordinal != cloud.total_points {
            return Err(format!("{} changed while selecting", cloud.path.display()));
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    Ok((index, Arc::new(mask)))
}

fn select_one(
    source: SelectionSource,
    projection: Projection,
    rectangle: ScreenRect,
    filter: ClassFilter,
    cancel: &AtomicBool,
) -> Result<(usize, Arc<SelectionMask>), String> {
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    let SelectionSource {
        index,
        cloud,
        tree,
        deleted,
        transform,
    } = source;
    cloud.validate_source().map_err(|error| error.to_string())?;
    let mut mask = SelectionMask::new(cloud.total_points)?;
    let mut random_state = 0xd1b5_4a32_d192_ed03u64;
    let mut visited = 0u64;
    let mut consider = |ordinal: u64, source_point: Point| {
        let point = transform.point(source_point);
        if !deleted.as_ref().is_some_and(|mask| mask.contains(ordinal)) && filter.accepts(&point) {
            if let Some((x, y, _)) = projection.project(point.xyz) {
                if rectangle.contains(x, y) {
                    mask.insert(ordinal, point, source_point.xyz, &mut random_state);
                }
            }
        }
    };
    if let Some(tree) = tree {
        tree.visit_intersecting(
            |bounds| {
                if cancel.load(Ordering::Relaxed) {
                    return false;
                }
                if filter.section.is_some_and(|section| {
                    (0..3).any(|axis| {
                        transform.bounds(bounds).max[axis] < section.min[axis]
                            || transform.bounds(bounds).min[axis] > section.max[axis]
                    })
                }) {
                    return false;
                }
                node_overlaps_rectangle(transform.bounds(bounds), projection, rectangle)
            },
            |record| {
                visited += 1;
                if visited & 0xfff == 0 && cancel.load(Ordering::Relaxed) {
                    return Err(LoadError::Cancelled);
                }
                consider(record.ordinal, record.point);
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;
    } else {
        let mut ordinal = 0u64;
        visit_points(&cloud.path, &mut |point| {
            if ordinal & 0xfff == 0 && cancel.load(Ordering::Relaxed) {
                return Err(LoadError::Cancelled);
            }
            consider(ordinal, point);
            ordinal += 1;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        if ordinal != cloud.total_points {
            return Err(format!("{} changed while selecting", cloud.path.display()));
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(LoadError::Cancelled.to_string());
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    Ok((index, Arc::new(mask)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn picks_visible_sphere_surface_before_hidden_center_and_accepts_sphere_edge() {
        let projection = Projection::new(
            Bounds {
                min: [-2.0; 3],
                max: [2.0; 3],
            },
            0.0,
            0.0,
            1.0,
            [0.0; 2],
            800.0,
            600.0,
        );
        let far = IndexedPoint {
            point: Point {
                xyz: [0.0, 0.0, 0.0],
                rgb: None,
                intensity: None,
                classification: None,
            },
            ordinal: 0,
        };
        let near = IndexedPoint {
            point: Point {
                xyz: [1.0, 0.1, 0.0],
                ..far.point
            },
            ordinal: 1,
        };
        let (x, y, _) = projection.project(far.point.xyz).unwrap();
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        };
        for records in [[far, near], [near, far]] {
            let mut best = None;
            for record in records {
                consider_pick(
                    record,
                    projection,
                    PickTarget {
                        pointer: [x, y],
                        radius: 8.0,
                        sphere_radius: 20.0,
                    },
                    filter,
                    None,
                    &mut best,
                );
            }
            assert_eq!(best.unwrap().0.ordinal, near.ordinal);
        }
        let mut edge = None;
        consider_pick(
            far,
            projection,
            PickTarget {
                pointer: [x + 15.0, y],
                radius: 8.0,
                sphere_radius: 20.0,
            },
            filter,
            None,
            &mut edge,
        );
        assert_eq!(edge.unwrap().0.ordinal, far.ordinal);
    }

    #[test]
    fn picks_only_the_viewport_lod_records_after_global_stride() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("two-points.xyz");
        fs::write(&source, "0 0 0\n1 0.1 0\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 2).unwrap());
        let far = IndexedPoint {
            point: cloud.points[0],
            ordinal: cloud.point_ordinals[0],
        };
        let near = IndexedPoint {
            point: cloud.points[1],
            ordinal: cloud.point_ordinals[1],
        };
        let view = PickView {
            cloud: Arc::clone(&cloud),
            detail: Some(vec![far, near].into()),
            deleted: None,
            transform: CloudTransform::default(),
            visible: true,
        };
        let projection = Projection::new(
            Bounds {
                min: [-2.0; 3],
                max: [2.0; 3],
            },
            0.0,
            0.0,
            1.0,
            [0.0; 2],
            800.0,
            600.0,
        );
        let (x, y, _) = projection.project(far.point.xyz).unwrap();
        let target = PickTarget {
            pointer: [x, y],
            radius: 8.0,
            sphere_radius: 20.0,
        };
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        };
        let cancel = AtomicBool::new(false);
        assert_eq!(
            pick_displayed(&[view], 0, 1, projection, target, filter, &cancel)
                .unwrap()
                .unwrap()
                .ordinal,
            far.ordinal
        );
        let prior = PickView {
            cloud: Arc::clone(&cloud),
            detail: Some(
                vec![IndexedPoint {
                    point: Point {
                        xyz: [0.0, 100.0, 0.0],
                        ..far.point
                    },
                    ordinal: far.ordinal,
                }]
                .into(),
            ),
            deleted: None,
            transform: CloudTransform::default(),
            visible: true,
        };
        let active = PickView {
            cloud,
            detail: Some(vec![far, near].into()),
            deleted: None,
            transform: CloudTransform::default(),
            visible: true,
        };
        assert_eq!(
            pick_displayed(&[prior, active], 1, 2, projection, target, filter, &cancel)
                .unwrap()
                .unwrap()
                .ordinal,
            near.ordinal
        );
    }

    #[test]
    fn deleted_and_selected_masks_detect_only_shared_source_ordinals() {
        let point = Point {
            xyz: [1.0, 2.0, 3.0],
            rgb: None,
            intensity: None,
            classification: None,
        };
        let first = SelectionMask::single(130, IndexedPoint { point, ordinal: 5 }).unwrap();
        let second = SelectionMask::single(
            130,
            IndexedPoint {
                point,
                ordinal: 129,
            },
        )
        .unwrap();
        let mut deleted = DeletionMask::new(130).unwrap();
        assert!(!deleted.overlaps_selection(&first));
        deleted.apply(&first).unwrap();
        assert!(deleted.overlaps_selection(&first));
        assert!(!deleted.overlaps_selection(&second));
        deleted.apply(&second).unwrap();
        assert!(deleted.overlaps_selection(&second));
    }

    #[test]
    fn thinning_is_exact_and_undo_preserves_earlier_deletions() {
        let original = SelectionMask::single(
            101,
            IndexedPoint {
                point: Point {
                    xyz: [0.0; 3],
                    rgb: None,
                    intensity: None,
                    classification: None,
                },
                ordinal: 7,
            },
        )
        .unwrap();
        let mut deleted = DeletionMask::new(101).unwrap();
        deleted.apply(&original).unwrap();
        let removed = SelectionMask::thin_removed(101, Some(&deleted), 10).unwrap();
        assert_eq!(removed.count, 90);
        assert!(!removed.contains(7));
        assert_eq!(deleted.apply(&removed).unwrap(), 90);
        assert_eq!(deleted.count, 91);
        assert_eq!(deleted.undo(&removed).unwrap(), 90);
        assert_eq!(deleted.count, 1);
        assert!(deleted.contains(7));
        assert_eq!(
            SelectionMask::thin_removed(101, Some(&deleted), 100)
                .unwrap()
                .count,
            0
        );
    }

    fn selection_source(
        cloud: Arc<PointCloud>,
        tree: Option<Arc<OctreeIndex>>,
        deleted: Option<Arc<DeletionMask>>,
    ) -> SelectionSource {
        SelectionSource {
            index: 0,
            cloud,
            tree,
            deleted,
            transform: CloudTransform::default(),
        }
    }

    #[test]
    fn selects_full_stream_not_preview() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cloud.xyz");
        let mut contents = String::new();
        for x in 0..100 {
            contents.push_str(&format!("{x} 0 0\n"));
        }
        fs::write(&source, contents).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 1).unwrap());
        let camera = Projection::new(cloud.bounds, 0.8, 0.0, 1.0, [0.0, 0.0], 800.0, 600.0);
        let rectangle = ScreenRect::from_corners([0.0, 0.0], [800.0, 600.0]);
        let selected = select_full(
            vec![selection_source(Arc::clone(&cloud), None, None)],
            camera,
            rectangle,
            ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: None,
            },
        )
        .unwrap();
        assert_eq!(selected[0].1.count, 100);
        assert!(selected[0].1.contains(99));
        let tree = Arc::new(
            OctreeIndex::build(
                &cloud,
                pointcloud_core::IndexConfig {
                    leaf_points: 8,
                    preview_points: 4,
                    max_depth: 8,
                    scratch_dir: Some(dir.path().to_path_buf()),
                },
            )
            .unwrap(),
        );
        let indexed = select_full(
            vec![selection_source(
                Arc::clone(&cloud),
                Some(Arc::clone(&tree)),
                None,
            )],
            camera,
            rectangle,
            ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: None,
            },
        )
        .unwrap();
        assert_eq!(indexed[0].1.bits, selected[0].1.bits);

        let (x, y, _) = camera.project([50.0, 0.0, 0.0]).unwrap();
        let small = ScreenRect::from_corners([x - 15.0, y - 15.0], [x + 15.0, y + 15.0]);
        let stream = select_full(
            vec![selection_source(Arc::clone(&cloud), None, None)],
            camera,
            small,
            ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: None,
            },
        )
        .unwrap();
        let indexed = select_full(
            vec![selection_source(cloud, Some(tree), None)],
            camera,
            small,
            ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: None,
            },
        )
        .unwrap();
        assert!(stream[0].1.count > 0 && stream[0].1.count < 100);
        assert_eq!(indexed[0].1.bits, stream[0].1.bits);
    }

    #[test]
    fn world_box_uses_exact_ordinals_with_and_without_index() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("world.xyz");
        fs::write(
            &source,
            (0..100).map(|x| format!("{x} 0 0\n")).collect::<String>(),
        )
        .unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 1).unwrap());
        assert_eq!(cloud.points.len(), 1);
        let tree = Arc::new(
            OctreeIndex::build(
                &cloud,
                pointcloud_core::IndexConfig {
                    leaf_points: 8,
                    preview_points: 4,
                    max_depth: 8,
                    scratch_dir: Some(dir.path().to_path_buf()),
                },
            )
            .unwrap(),
        );
        let removed = SelectionMask::single(
            cloud.total_points,
            IndexedPoint {
                point: Point {
                    xyz: [40.0, 0.0, 0.0],
                    rgb: None,
                    intensity: None,
                    classification: None,
                },
                ordinal: 40,
            },
        )
        .unwrap();
        let mut deleted = DeletionMask::new(cloud.total_points).unwrap();
        deleted.apply(&removed).unwrap();
        let query = Bounds {
            min: [30.0, 0.0, 0.0],
            max: [49.0, 0.0, 0.0],
        };
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: Some(Bounds {
                min: [35.0, 0.0, 0.0],
                max: [45.0, 0.0, 0.0],
            }),
        };
        let stream = select_world(
            vec![selection_source(
                Arc::clone(&cloud),
                None,
                Some(Arc::new(deleted.clone())),
            )],
            query,
            filter,
        )
        .unwrap();
        let indexed = select_world(
            vec![selection_source(cloud, Some(tree), Some(Arc::new(deleted)))],
            query,
            filter,
        )
        .unwrap();
        assert_eq!(stream[0].1.count, 10);
        assert!(stream[0].1.contains(35));
        assert!(stream[0].1.contains(45));
        assert!(!stream[0].1.contains(40));
        assert_eq!(stream[0].1.bits, indexed[0].1.bits);
    }

    #[test]
    fn cancelled_selection_does_not_publish_partial_masks() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cancel.xyz");
        fs::write(&source, "0 0 0\n1 0 0\n2 0 0\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 1).unwrap());
        let cancel = Arc::new(AtomicBool::new(true));
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        };
        let result = select_world_cancellable(
            vec![selection_source(cloud, None, None)],
            Bounds {
                min: [0.0, 0.0, 0.0],
                max: [2.0, 0.0, 0.0],
            },
            filter,
            cancel,
        );
        assert_eq!(result.unwrap_err(), "Operation cancelled");
    }

    #[test]
    fn per_class_visibility_filters_indexed_and_streamed_selection() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("classified.ply");
        fs::write(
            &source,
            "ply\nformat ascii 1.0\nelement vertex 3\nproperty double x\nproperty double y\nproperty double z\nproperty uchar classification\nend_header\n0 0 0 2\n1 0 0 6\n2 0 0 9\n",
        )
        .unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 1).unwrap());
        let tree = Arc::new(
            OctreeIndex::build(
                &cloud,
                pointcloud_core::IndexConfig {
                    leaf_points: 1,
                    preview_points: 1,
                    max_depth: 4,
                    scratch_dir: Some(dir.path().to_path_buf()),
                },
            )
            .unwrap(),
        );
        let mut classes = ClassVisibility::default();
        classes.set(2, false);
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes,
            section: None,
        };
        let bounds = cloud.bounds;
        let streamed = select_world(
            vec![selection_source(Arc::clone(&cloud), None, None)],
            bounds,
            filter,
        )
        .unwrap();
        let indexed = select_world(
            vec![selection_source(cloud, Some(tree), None)],
            bounds,
            filter,
        )
        .unwrap();
        assert_eq!(streamed[0].1.count, 2);
        assert!(!streamed[0].1.contains(0));
        assert!(streamed[0].1.contains(1));
        assert!(streamed[0].1.contains(2));
        assert_eq!(streamed[0].1.bits, indexed[0].1.bits);
    }

    #[test]
    fn deleted_points_are_excluded_from_selection_and_can_be_restored() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("cloud.xyz");
        fs::write(&source, "0 0 0\n1 0 0\n2 0 0\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 1).unwrap());
        let selected = SelectionMask::single(
            cloud.total_points,
            IndexedPoint {
                point: Point {
                    xyz: [1.0, 0.0, 0.0],
                    rgb: None,
                    intensity: None,
                    classification: None,
                },
                ordinal: 1,
            },
        )
        .unwrap();
        let mut deleted = DeletionMask::new(cloud.total_points).unwrap();
        assert_eq!(deleted.apply(&selected).unwrap(), 1);
        assert_eq!(deleted.apply(&selected).unwrap(), 0);
        assert!(deleted.contains(1));
        let camera = Projection::new(cloud.bounds, 0.0, 0.0, 1.0, [0.0, 0.0], 800.0, 600.0);
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        };
        let selection = select_full(
            vec![selection_source(
                Arc::clone(&cloud),
                None,
                Some(Arc::new(deleted.clone())),
            )],
            camera,
            ScreenRect::from_corners([0.0, 0.0], [800.0, 600.0]),
            filter,
        )
        .unwrap();
        assert_eq!(selection[0].1.count, 2);
        assert!(!selection[0].1.contains(1));
        assert_eq!(deleted.undo(&selected).unwrap(), 1);
        assert_eq!(deleted.count, 0);
        assert!(!deleted.contains(1));
    }

    #[test]
    fn section_box_filters_the_full_source_stream() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("section.xyz");
        let contents = (0..100).map(|x| format!("{x} 0 0\n")).collect::<String>();
        fs::write(&source, contents).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 1).unwrap());
        let camera = Projection::new(cloud.bounds, 0.0, 0.0, 1.0, [0.0, 0.0], 800.0, 600.0);
        let selected = select_full(
            vec![selection_source(cloud, None, None)],
            camera,
            ScreenRect::from_corners([0.0, 0.0], [800.0, 600.0]),
            ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: Some(Bounds {
                    min: [30.0, -1.0, -1.0],
                    max: [49.0, 1.0, 1.0],
                }),
            },
        )
        .unwrap();
        assert_eq!(selected[0].1.count, 20);
        assert!(selected[0].1.contains(30));
        assert!(selected[0].1.contains(49));
        assert!(!selected[0].1.contains(50));
    }

    #[test]
    fn deep_zoom_magnifies_without_crossing_cloud() {
        let bounds = Bounds {
            min: [0.0; 3],
            max: [100.0; 3],
        };
        let normal = Projection::new(bounds, 0.0, 0.0, 1.0, [0.0; 2], 800.0, 600.0);
        let close = Projection::new(bounds, 0.0, 0.0, 0.01, [0.0; 2], 800.0, 600.0);
        let center = [50.0; 3];
        let detail = [50.0, 50.2, 50.0];
        assert_eq!(close.project(center).unwrap().0, 400.0);
        let normal_offset = normal.project(detail).unwrap().0 - 400.0;
        let close_offset = close.project(detail).unwrap().0 - 400.0;
        assert!(close_offset.abs() > normal_offset.abs() * 90.0);
    }

    #[test]
    fn projected_node_span_tracks_zoom_and_pan() {
        let bounds = Bounds {
            min: [0.0; 3],
            max: [100.0; 3],
        };
        let node = Bounds {
            min: [40.0; 3],
            max: [60.0; 3],
        };
        let overview = Projection::new(bounds, 0.0, 0.0, 1.0, [0.0; 2], 800.0, 600.0);
        let zoomed = Projection::new(bounds, 0.0, 0.0, 0.1, [0.0; 2], 800.0, 600.0);
        let panned = Projection::new(bounds, 0.0, 0.0, 1.0, [2_000.0, 0.0], 800.0, 600.0);
        assert!(zoomed.screen_span(node).unwrap() > overview.screen_span(node).unwrap());
        assert!(zoomed.screen_coverage(node).unwrap() > overview.screen_coverage(node).unwrap());
        assert!(panned.screen_span(node).is_none());
        assert!(panned.screen_coverage(node).is_none());
    }

    #[test]
    fn picks_frontmost_indexed_point_and_preserves_source_ordinal() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("aligned.xyz");
        fs::write(&source, "0 0 0\n10 0 0\n20 0 0\n").unwrap();
        let cloud = pointcloud_core::open(&source, 1).unwrap();
        let tree = OctreeIndex::build(
            &cloud,
            pointcloud_core::IndexConfig {
                leaf_points: 1,
                preview_points: 1,
                max_depth: 4,
                scratch_dir: Some(dir.path().to_path_buf()),
            },
        )
        .unwrap();
        let camera = Projection::new(cloud.bounds, 0.0, 0.0, 1.0, [0.0, 0.0], 800.0, 600.0);
        let record = pick_indexed(
            &tree,
            camera,
            [400.0, 300.0],
            8.0,
            ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: None,
            },
            None,
        )
        .unwrap()
        .unwrap();
        assert_eq!(record.ordinal, 2);
        let mask = SelectionMask::single(cloud.total_points, record).unwrap();
        assert_eq!(mask.count, 1);
        assert!(mask.contains(2));
        assert!(!mask.contains(0));
        let mut deleted = DeletionMask::new(cloud.total_points).unwrap();
        assert_eq!(deleted.apply(&mask).unwrap(), 1);
        let next = pick_indexed(
            &tree,
            camera,
            [400.0, 300.0],
            8.0,
            ClassFilter {
                ground: true,
                vegetation: true,
                buildings: true,
                other: true,
                classes: ClassVisibility::default(),
                section: None,
            },
            Some(&deleted),
        )
        .unwrap()
        .unwrap();
        assert_eq!(next.ordinal, 1);
        let destination = dir.path().join("picked.ply");
        pointcloud_core::export_where(
            &cloud,
            &destination,
            pointcloud_core::ExportFormat::PlyBinary,
            mask.count,
            |ordinal, _| mask.contains(ordinal),
        )
        .unwrap();
        let exported = pointcloud_core::open(destination, 10).unwrap();
        assert_eq!(exported.total_points, 1);
        assert_eq!(exported.points[0].xyz, [20.0, 0.0, 0.0]);
    }

    #[test]
    fn cancelled_point_pick_stops_indexed_and_source_scans() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pick.xyz");
        fs::write(&path, "0 0 0\n1 0 0\n2 0 0\n").unwrap();
        let cloud = pointcloud_core::open(&path, 1).unwrap();
        let index = OctreeIndex::build(
            &cloud,
            pointcloud_core::IndexConfig {
                leaf_points: 1,
                preview_points: 1,
                max_depth: 4,
                scratch_dir: Some(directory.path().to_path_buf()),
            },
        )
        .unwrap();
        let camera = Projection::new(cloud.bounds, 0.0, 0.0, 1.0, [0.0; 2], 800.0, 600.0);
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        };
        let cancel = AtomicBool::new(true);
        assert_eq!(
            pick_indexed_transformed(
                &index,
                camera,
                PickTarget {
                    pointer: [400.0, 300.0],
                    radius: 8.0,
                    sphere_radius: 0.0,
                },
                filter,
                None,
                CloudTransform::default(),
                &cancel,
            )
            .unwrap_err(),
            "Operation cancelled"
        );
        assert_eq!(
            pick_full_transformed(
                &cloud,
                camera,
                PickTarget {
                    pointer: [400.0, 300.0],
                    radius: 8.0,
                    sphere_radius: 0.0,
                },
                filter,
                None,
                CloudTransform::default(),
                &cancel,
            )
            .unwrap_err(),
            "Operation cancelled"
        );
    }

    #[test]
    fn picks_exact_source_point_without_octree_and_honors_deletions_and_section() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("pick.xyz");
        fs::write(&source, "0 0 0\n10 0 0\n20 0 0\n").unwrap();
        let cloud = pointcloud_core::open(&source, 1).unwrap();
        assert_eq!(cloud.points.len(), 1);
        let camera = Projection::new(cloud.bounds, 0.0, 0.0, 1.0, [0.0, 0.0], 800.0, 600.0);
        let filter = ClassFilter {
            ground: true,
            vegetation: true,
            buildings: true,
            other: true,
            classes: ClassVisibility::default(),
            section: None,
        };
        let picked = pick_full(&cloud, camera, [400.0, 300.0], 8.0, filter, None)
            .unwrap()
            .unwrap();
        assert_eq!(picked.ordinal, 2);
        let mut deleted = DeletionMask::new(cloud.total_points).unwrap();
        deleted
            .apply(&SelectionMask::single(cloud.total_points, picked).unwrap())
            .unwrap();
        let picked = pick_full(&cloud, camera, [400.0, 300.0], 8.0, filter, Some(&deleted))
            .unwrap()
            .unwrap();
        assert_eq!(picked.ordinal, 1);
        let section = ClassFilter {
            section: Some(Bounds {
                min: [0.0, -1.0, -1.0],
                max: [5.0, 1.0, 1.0],
            }),
            ..filter
        };
        assert_eq!(
            pick_full(&cloud, camera, [400.0, 300.0], 8.0, section, None)
                .unwrap()
                .unwrap()
                .ordinal,
            0
        );
    }
}
