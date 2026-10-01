use std::sync::Arc;

use crate::CloudTransform;
use pointcloud_core::{visit_points, Bounds, IndexedPoint, OctreeIndex, Point, PointCloud};

const HIGHLIGHT_LIMIT: usize = 8_000;

#[derive(Debug, Clone)]
pub struct SelectionMask {
    pub bits: Vec<u64>,
    pub count: u64,
    pub highlights: Vec<Point>,
}

impl SelectionMask {
    fn new(total_points: u64) -> Result<Self, String> {
        let words = total_points.div_ceil(64);
        let words = usize::try_from(words).map_err(|_| "point count exceeds address space")?;
        Ok(Self {
            bits: vec![0; words],
            count: 0,
            highlights: Vec::with_capacity(HIGHLIGHT_LIMIT),
        })
    }

    pub fn contains(&self, ordinal: u64) -> bool {
        let word = usize::try_from(ordinal / 64).ok();
        word.and_then(|index| self.bits.get(index))
            .is_some_and(|bits| bits & (1u64 << (ordinal % 64)) != 0)
    }

    fn insert(&mut self, ordinal: u64, point: Point, random_state: &mut u64) {
        self.bits[(ordinal / 64) as usize] |= 1u64 << (ordinal % 64);
        self.count += 1;
        if self.highlights.len() < HIGHLIGHT_LIMIT {
            self.highlights.push(point);
        } else {
            *random_state ^= *random_state << 13;
            *random_state ^= *random_state >> 7;
            *random_state ^= *random_state << 17;
            let replacement = *random_state % self.count;
            if replacement < HIGHLIGHT_LIMIT as u64 {
                self.highlights[replacement as usize] = point;
            }
        }
    }

    pub fn single(total_points: u64, record: IndexedPoint) -> Result<Self, String> {
        if record.ordinal >= total_points {
            return Err("indexed point number is outside the source".into());
        }
        let mut mask = Self::new(total_points)?;
        mask.insert(record.ordinal, record.point, &mut 0);
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
        Self {
            center: bounds.center(),
            right: [-sy, cy, 0.0],
            up: [-sp * cy, -sp * sy, cp],
            toward_camera: [cp * cy, cp * sy, sp],
            distance: bounds.extent().max(0.001) * 1.8,
            scale: height.min(width) as f64 * 1.25 / zoom as f64,
            width: width as f64,
            height: height as f64,
            pan,
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
        let depth = self.distance - dot(relative, self.toward_camera);
        if depth <= 0.01 {
            return None;
        }
        let x =
            self.width * 0.5 + self.pan[0] as f64 + dot(relative, self.right) * self.scale / depth;
        let y =
            self.height * 0.5 + self.pan[1] as f64 - dot(relative, self.up) * self.scale / depth;
        Some((x as f32, y as f32, depth))
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
        pointer,
        radius,
        filter,
        deleted,
        CloudTransform::default(),
    )
}

pub fn pick_indexed_transformed(
    tree: &OctreeIndex,
    projection: Projection,
    pointer: [f32; 2],
    radius: f32,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
    transform: CloudTransform,
) -> Result<Option<IndexedPoint>, String> {
    validate_pick(pointer, radius)?;
    let mut best: Option<(IndexedPoint, f32, f64)> = None;
    tree.visit_intersecting(
        |bounds| node_overlaps_pointer(transform.bounds(bounds), projection, pointer, radius),
        |record| {
            consider_pick(
                transform.record(record),
                projection,
                pointer,
                radius,
                filter,
                deleted,
                &mut best,
            );
            Ok(())
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(best.map(|(record, _, _)| record))
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
        pointer,
        radius,
        filter,
        deleted,
        CloudTransform::default(),
    )
}

pub fn pick_full_transformed(
    cloud: &PointCloud,
    projection: Projection,
    pointer: [f32; 2],
    radius: f32,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
    transform: CloudTransform,
) -> Result<Option<IndexedPoint>, String> {
    validate_pick(pointer, radius)?;
    cloud.validate_source().map_err(|error| error.to_string())?;
    let mut best = None;
    let mut ordinal = 0u64;
    visit_points(&cloud.path, &mut |point| {
        consider_pick(
            transform.record(IndexedPoint { point, ordinal }),
            projection,
            pointer,
            radius,
            filter,
            deleted,
            &mut best,
        );
        ordinal += 1;
        Ok(())
    })
    .map_err(|error| error.to_string())?;
    if ordinal != cloud.total_points {
        return Err(format!("{} changed while picking", cloud.path.display()));
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    Ok(best.map(|(record, _, _)| record))
}

fn validate_pick(pointer: [f32; 2], radius: f32) -> Result<(), String> {
    if !pointer.iter().all(|value| value.is_finite()) || !radius.is_finite() || radius <= 0.0 {
        return Err("invalid point-pick position".into());
    }
    Ok(())
}

fn consider_pick(
    record: IndexedPoint,
    projection: Projection,
    pointer: [f32; 2],
    radius: f32,
    filter: ClassFilter,
    deleted: Option<&DeletionMask>,
    best: &mut Option<(IndexedPoint, f32, f64)>,
) {
    if deleted.is_some_and(|mask| mask.contains(record.ordinal)) || !filter.accepts(&record.point) {
        return;
    }
    if let Some((x, y, depth)) = projection.project(record.point.xyz) {
        let dx = x - pointer[0];
        let dy = y - pointer[1];
        let distance_squared = dx * dx + dy * dy;
        if distance_squared <= radius * radius
            && best.as_ref().is_none_or(|(_, best_distance, best_depth)| {
                distance_squared < *best_distance
                    || (distance_squared == *best_distance && depth < *best_depth)
            })
        {
            *best = Some((record, distance_squared, depth));
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

pub fn select_full(
    sources: Vec<SelectionSource>,
    projection: Projection,
    rectangle: ScreenRect,
    filter: ClassFilter,
) -> Result<Vec<(usize, Arc<SelectionMask>)>, String> {
    let workers: Vec<_> = sources
        .into_iter()
        .map(|source| std::thread::spawn(move || select_one(source, projection, rectangle, filter)))
        .collect();
    let mut result = Vec::with_capacity(workers.len());
    for worker in workers {
        result.push(worker.join().map_err(|_| "selection worker panicked")??);
    }
    Ok(result)
}

/// Select exact source points inside a world-coordinate box. Indexed files
/// only visit intersecting leaves; unindexed files stream the entire source.
pub fn select_world(
    sources: Vec<SelectionSource>,
    bounds: Bounds,
    filter: ClassFilter,
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
        .map(|source| std::thread::spawn(move || select_one_world(source, bounds, filter)))
        .collect();
    let mut result = Vec::with_capacity(workers.len());
    for worker in workers {
        result.push(worker.join().map_err(|_| "selection worker panicked")??);
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
) -> Result<(usize, Arc<SelectionMask>), String> {
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
    let mut consider = |ordinal: u64, point: Point| {
        let point = transform.point(point);
        if !deleted.as_ref().is_some_and(|mask| mask.contains(ordinal))
            && filter.accepts(&point)
            && (0..3).all(|axis| {
                point.xyz[axis] >= bounds.min[axis] && point.xyz[axis] <= bounds.max[axis]
            })
        {
            mask.insert(ordinal, point, &mut random_state);
        }
    };
    if let Some(tree) = tree {
        tree.visit_intersecting(
            |node| {
                bounds_overlap(transform.bounds(node), bounds)
                    && filter
                        .section
                        .is_none_or(|section| bounds_overlap(transform.bounds(node), section))
            },
            |record| {
                consider(record.ordinal, record.point);
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;
    } else {
        let mut ordinal = 0u64;
        visit_points(&cloud.path, &mut |point| {
            consider(ordinal, point);
            ordinal += 1;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        if ordinal != cloud.total_points {
            return Err(format!("{} changed while selecting", cloud.path.display()));
        }
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    Ok((index, Arc::new(mask)))
}

fn select_one(
    source: SelectionSource,
    projection: Projection,
    rectangle: ScreenRect,
    filter: ClassFilter,
) -> Result<(usize, Arc<SelectionMask>), String> {
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
    let mut consider = |ordinal: u64, point: Point| {
        let point = transform.point(point);
        if !deleted.as_ref().is_some_and(|mask| mask.contains(ordinal)) && filter.accepts(&point) {
            if let Some((x, y, _)) = projection.project(point.xyz) {
                if rectangle.contains(x, y) {
                    mask.insert(ordinal, point, &mut random_state);
                }
            }
        }
    };
    if let Some(tree) = tree {
        tree.visit_intersecting(
            |bounds| {
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
                consider(record.ordinal, record.point);
                Ok(())
            },
        )
        .map_err(|error| error.to_string())?;
    } else {
        let mut ordinal = 0u64;
        visit_points(&cloud.path, &mut |point| {
            consider(ordinal, point);
            ordinal += 1;
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        if ordinal != cloud.total_points {
            return Err(format!("{} changed while selecting", cloud.path.display()));
        }
    }
    cloud.validate_source().map_err(|error| error.to_string())?;
    Ok((index, Arc::new(mask)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
