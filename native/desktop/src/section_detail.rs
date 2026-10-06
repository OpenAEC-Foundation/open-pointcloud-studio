//! Detail inside the section box.
//!
//! The section box only clips what is drawn: the sets of points read for the
//! view stay on the graphics device whatever the box does, and the shader
//! leaves out what lies outside it. While the box is on, the octree is read
//! once more for the nodes inside it, and the points that this adds to the
//! sets of the view are drawn beside them. The box so gets the budget of what
//! it shows without a point leaving the screen first, and the points it
//! shows only ever grow while the camera stands still. Switched off, the box
//! keeps that detail; when the camera moved while it was on, only the part of
//! the sets outside it is read again.

use std::sync::Arc;

use iced::Task;
use pointcloud_core::{Bounds, IndexedNode, IndexedPoint, OctreeIndex, OrientedBox, PointCloud};
use rayon::prelude::*;

use crate::selection::Projection;
use crate::{
    i18n, CloudEntry, CloudTransform, DetailLayers, LodRefinement, LodSets, Message, Studio,
};

/// What the sets of points were read for besides the section box: the
/// camera, the layers with an octree and the budget.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DetailView {
    pub projection: Projection,
    pub layers: DetailLayers,
    pub budget: usize,
}

/// The points read inside the section box besides the sets of the view.
#[derive(Debug, Clone)]
pub(crate) struct FocusDetail {
    /// The box they were read for.
    pub region: OrientedBox,
    /// The view they were read for; `None` while only a first pass of them
    /// is shown.
    pub view: Option<DetailView>,
}

/// What the next refinement of the view reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DetailPlan {
    /// Nothing to read: the box is off and the sets are those of this view,
    /// or the box is on and was read for this view already.
    Nothing,
    /// The whole view: its sets replace those drawn and the points inside a
    /// box are let go.
    Whole,
    /// The nodes inside the box, whose new points are drawn beside the sets
    /// of the view. `strict` when only the box changed since what is shown
    /// was read: the points read then take the place of those added before
    /// only when they are more, and nothing is read when they could not be.
    Inside { region: OrientedBox, strict: bool },
    /// The nodes outside a box that was switched off after the camera moved
    /// while it was on: the sets keep their points inside it, and those
    /// outside it are read anew when that gives more of them.
    Outside { region: OrientedBox },
}

/// What a refinement of part of the view gives.
#[derive(Debug, Clone)]
pub(crate) enum MergedSets {
    /// Points to draw beside the sets of the view, read inside this box, by
    /// cloud; the clouds left out have none.
    Inside(OrientedBox, LodSets),
    /// The sets of the view, by cloud, with their part outside the box read
    /// anew.
    Outside(LodSets),
    /// Nothing that would add a point: what is shown stays.
    Unchanged,
}

impl Studio {
    /// Take what a refinement of part of the view gives: a set to show in
    /// between, or with `done` what it comes to at its end.
    pub(crate) fn take_merged_sets(&mut self, revision: u64, sets: MergedSets, done: bool) {
        let request = self
            .detail_request
            .clone()
            .filter(|request| request.revision == revision);
        let view = request
            .as_ref()
            .filter(|_| done)
            .map(|request| request.view.clone());
        if let Some((DetailPlan::Inside { region, .. }, view)) = request
            .as_ref()
            .filter(|_| done)
            .map(|request| (request.plan, &request.view))
        {
            self.section_read = Some((region, view.clone()));
        }
        let status = match sets {
            MergedSets::Inside(region, sets) => {
                let mut added: Vec<Option<Arc<[IndexedPoint]>>> = vec![None; self.clouds.len()];
                let mut count = 0;
                for (index, points) in sets {
                    count += points.len();
                    if let Some(slot) = added.get_mut(index) {
                        *slot = (!points.is_empty()).then(|| points.into());
                    }
                }
                for (entry, points) in self.clouds.iter_mut().zip(added) {
                    entry.focus_points = points;
                }
                self.focus = Some(FocusDetail { region, view });
                let count = crate::format_count(count);
                if done {
                    i18n::tr_args(
                        "Section box: {count} points added inside the box",
                        &[("count", &count)],
                    )
                } else {
                    i18n::tr_args(
                        "Section box: {count} points added inside the box; reading on…",
                        &[("count", &count)],
                    )
                }
            }
            MergedSets::Outside(sets) => {
                let count = self.take_view_sets(sets);
                self.detail_view = view;
                let count = crate::format_count(count);
                if done {
                    i18n::tr_args(
                        "Viewport LOD ready outside the section box: {count} points",
                        &[("count", &count)],
                    )
                } else {
                    i18n::tr_args(
                        "Viewport LOD outside the section box: {count} points; adding detail…",
                        &[("count", &count)],
                    )
                }
            }
            MergedSets::Unchanged => {
                let Some(request) = request else {
                    return;
                };
                match request.plan {
                    DetailPlan::Inside { region, .. } => {
                        if let Some(focus) =
                            self.focus.as_mut().filter(|focus| focus.region == region)
                        {
                            focus.view = Some(request.view);
                        }
                        i18n::tr("Nothing to add inside the section box: it shows as many points as the budget allows or the scan has").to_owned()
                    }
                    DetailPlan::Outside { .. } => {
                        self.detail_view = Some(request.view);
                        i18n::tr("Nothing to add outside the section box: the points shown stay")
                            .to_owned()
                    }
                    DetailPlan::Nothing | DetailPlan::Whole => return,
                }
            }
        };
        if self.reports_detail() {
            self.status = status;
        }
    }

    /// What the next refinement reads, for the camera, layers and budget of
    /// `view`.
    pub(crate) fn detail_plan(&self, view: &DetailView) -> DetailPlan {
        if let Some(region) = self.section_box() {
            if self
                .section_read
                .as_ref()
                .is_some_and(|(read, seen)| *read == region && seen == view)
            {
                // This box was read for this view already.
                return DetailPlan::Nothing;
            }
            let strict = match &self.focus {
                Some(focus) => focus.view.as_ref() == Some(view),
                None => self.detail_view.as_ref() == Some(view),
            };
            return DetailPlan::Inside { region, strict };
        }
        if self.detail_view.as_ref() == Some(view) {
            return DetailPlan::Nothing;
        }
        match &self.focus {
            Some(focus) if focus.view.as_ref() == Some(view) => DetailPlan::Outside {
                region: focus.region,
            },
            _ => DetailPlan::Whole,
        }
    }
}

/// The part of the scene that a refinement reads anew.
#[derive(Debug, Clone, Copy)]
enum Part {
    Inside(OrientedBox),
    Outside(OrientedBox),
}

/// How much of a box a part reaches.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reach {
    Nothing,
    Some,
    All,
}

impl Part {
    fn holds(self, xyz: [f64; 3]) -> bool {
        match self {
            Self::Inside(region) => region.contains(xyz),
            Self::Outside(region) => !region.contains(xyz),
        }
    }

    fn reach(self, bounds: Bounds) -> Reach {
        let (region, inside) = match self {
            Self::Inside(region) => (region, true),
            Self::Outside(region) => (region, false),
        };
        let around = region.aabb();
        let apart = (0..3)
            .any(|axis| bounds.max[axis] < around.min[axis] || bounds.min[axis] > around.max[axis]);
        let within = !apart
            && pointcloud_core::bounds_corners(bounds)
                .into_iter()
                .all(|corner| region.contains(corner));
        match (inside, apart, within) {
            (true, true, _) | (false, false, true) => Reach::Nothing,
            (true, false, true) | (false, true, _) => Reach::All,
            _ => Reach::Some,
        }
    }

    /// The most points of a cloud there can be in this part and in view:
    /// those of the nodes that it reaches and that are in view, without
    /// reading any.
    fn capacity(
        self,
        node: &IndexedNode,
        transform: CloudTransform,
        projection: Projection,
    ) -> u64 {
        let bounds = transform.bounds(node.bounds);
        if projection.screen_span(bounds).is_none() {
            return 0;
        }
        match self.reach(bounds) {
            Reach::Nothing => 0,
            Reach::All => node.total_points,
            Reach::Some if node.is_leaf() => node.total_points,
            Reach::Some => node
                .children
                .iter()
                .map(|child| self.capacity(child, transform, projection))
                .sum(),
        }
    }
}

/// A read is worth it when it can add more than this part of the budget.
const LEAST_GAIN: usize = 32;

/// What a layer draws for the view now.
#[derive(Clone)]
pub(crate) enum ShownSet {
    Detail(Arc<[IndexedPoint]>),
    /// The sample the cloud keeps in memory, drawn while no detail was read.
    Sample(Arc<PointCloud>),
}

impl ShownSet {
    pub(crate) fn of(entry: &CloudEntry) -> Self {
        match &entry.detail_points {
            Some(points) => Self::Detail(Arc::clone(points)),
            None => Self::Sample(Arc::clone(&entry.cloud)),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Detail(points) => points.len(),
            Self::Sample(cloud) => cloud.points.len().min(cloud.point_ordinals.len()),
        }
    }

    fn get(&self, index: usize) -> IndexedPoint {
        match self {
            Self::Detail(points) => points[index],
            Self::Sample(cloud) => IndexedPoint {
                point: cloud.points[index],
                ordinal: cloud.point_ordinals[index],
            },
        }
    }
}

/// How the points read for part of the view meet what is drawn.
pub(crate) struct DetailMerge {
    part: Part,
    strict: bool,
    budget: usize,
    /// The renderer draws every so many points of the sets of the view.
    stride: usize,
    /// Per source of the refinement: what it draws for the view, and the
    /// points read inside the box besides that.
    shown: Vec<ShownSet>,
    added: Vec<Option<Arc<[IndexedPoint]>>>,
    /// Per source, the ordinals of its drawn points inside the box, in
    /// order: a point read there that one of them has is not added again.
    held: Vec<Vec<u64>>,
    /// Points of the sets that stay as they are: those inside the box when
    /// the part outside it is read. They count against the budget.
    kept: usize,
    /// The points in view that the merge would replace, as drawn now.
    replaced: usize,
}

impl DetailMerge {
    /// A merge for `plan`, which reads part of the view, with what the
    /// sources draw now.
    pub(crate) fn new(
        plan: DetailPlan,
        budget: usize,
        stride: usize,
        shown: Vec<ShownSet>,
        added: Vec<Option<Arc<[IndexedPoint]>>>,
    ) -> Option<Self> {
        let (part, strict) = match plan {
            DetailPlan::Inside { region, strict } => (Part::Inside(region), strict),
            DetailPlan::Outside { region } => (Part::Outside(region), true),
            DetailPlan::Nothing | DetailPlan::Whole => return None,
        };
        Some(Self {
            part,
            strict,
            budget,
            stride: stride.max(1),
            held: vec![Vec::new(); shown.len()],
            shown,
            added,
            kept: 0,
            replaced: 0,
        })
    }

    /// Look at what is drawn now, before anything is read. False when a read
    /// could add next to nothing: about everything there is of the part in
    /// view is drawn already, or, when only the box changed, the part shows
    /// about a whole budget already.
    fn prepare(
        &mut self,
        sources: &[(usize, Arc<OctreeIndex>, CloudTransform, f32)],
        projection: Projection,
    ) -> bool {
        let part = self.part;
        let in_view = |xyz: [f64; 3]| projection.project(xyz).is_some();
        let mut present = 0usize;
        let mut capacity = 0u64;
        for (slot, (_, tree, transform, _)) in sources.iter().enumerate() {
            let transform = *transform;
            let shown = &self.shown[slot];
            capacity = capacity.saturating_add(part.capacity(&tree.root, transform, projection));
            match part {
                Part::Inside(region) => {
                    let mut held: Vec<u64> = (0..shown.len())
                        .into_par_iter()
                        .map(|index| shown.get(index))
                        .filter(|record| region.contains(transform.xyz(record.point.xyz)))
                        .map(|record| record.ordinal)
                        .collect();
                    present += count_where(shown, |record| {
                        let xyz = transform.xyz(record.point.xyz);
                        region.contains(xyz) && in_view(xyz)
                    }) / self.stride;
                    held.par_sort_unstable();
                    self.held[slot] = held;
                    if let Some(added) = &self.added[slot] {
                        self.replaced += added
                            .par_iter()
                            .filter(|record| {
                                let xyz = transform.xyz(record.point.xyz);
                                region.contains(xyz) && in_view(xyz)
                            })
                            .count();
                    }
                }
                Part::Outside(region) => {
                    self.kept += count_where(shown, |record| {
                        region.contains(transform.xyz(record.point.xyz))
                    });
                    self.replaced += count_where(shown, |record| {
                        let xyz = transform.xyz(record.point.xyz);
                        !region.contains(xyz) && in_view(xyz)
                    }) / self.stride;
                }
            }
        }
        // A read is worth it when it can add more than a small part of the
        // budget: up to the points there are in view, and, when only the box
        // changed, up to the budget.
        let least = (self.budget / LEAST_GAIN).max(1) as u64;
        let (present, most) = match part {
            Part::Inside(_) if self.strict => {
                (present + self.replaced, capacity.min(self.budget as u64))
            }
            Part::Inside(_) => (present + self.replaced, capacity),
            Part::Outside(_) => (self.replaced, capacity.min(self.room() as u64)),
        };
        present as u64 + least <= most
    }

    /// Points the part read anew may hold: the budget less the points that
    /// stay.
    fn room(&self) -> usize {
        match self.part {
            Part::Inside(_) => self.budget,
            Part::Outside(_) => self.budget.saturating_sub(self.kept),
        }
    }

    /// The sets that the points read so far give, by cloud, and how many of
    /// the points they bring in are in view.
    fn candidate(
        &self,
        sources: &[(usize, Arc<OctreeIndex>, CloudTransform, f32)],
        samples: &[Vec<IndexedPoint>],
        projection: Projection,
    ) -> (LodSets, usize) {
        let part = self.part;
        let in_view = |xyz: [f64; 3]| projection.project(xyz).is_some();
        // Per source: the points read that are new in the part.
        let fresh: Vec<Vec<IndexedPoint>> = sources
            .iter()
            .zip(samples)
            .enumerate()
            .map(|(slot, ((_, _, transform, _), points))| {
                let held = &self.held[slot];
                points
                    .par_iter()
                    .filter(|record| {
                        part.holds(transform.xyz(record.point.xyz))
                            && (record.ordinal == u64::MAX
                                || held.binary_search(&record.ordinal).is_err())
                    })
                    .copied()
                    .collect()
            })
            .collect();
        let total: usize = fresh.iter().map(Vec::len).sum();
        let room = self.room();
        let mut count = 0;
        let mut sets = Vec::with_capacity(sources.len());
        for (slot, ((index, _, transform, _), points)) in sources.iter().zip(fresh).enumerate() {
            // The sets of the view stay within the budget, so the renderer
            // never has to thin them: the part outside the box takes what
            // the points kept inside it leave, each source its share.
            let points = if matches!(part, Part::Outside(_)) && total > room {
                let share = points.len() as u128 * room as u128 / total as u128;
                thinned(points, share as usize)
            } else {
                points
            };
            count += points
                .par_iter()
                .filter(|record| in_view(transform.xyz(record.point.xyz)))
                .count();
            let set = match part {
                Part::Inside(_) => points,
                Part::Outside(region) => {
                    let shown = &self.shown[slot];
                    let mut set: Vec<IndexedPoint> = (0..shown.len())
                        .into_par_iter()
                        .map(|index| shown.get(index))
                        .filter(|record| region.contains(transform.xyz(record.point.xyz)))
                        .collect();
                    set.extend(points);
                    set
                }
            };
            sets.push((*index, set));
        }
        (sets, count)
    }

    fn wrap(&self, sets: LodSets) -> MergedSets {
        match self.part {
            Part::Inside(region) => MergedSets::Inside(region, sets),
            Part::Outside(_) => MergedSets::Outside(sets),
        }
    }
}

/// How many of the points of a set pass a test.
fn count_where(shown: &ShownSet, test: impl Fn(IndexedPoint) -> bool + Sync) -> usize {
    (0..shown.len())
        .into_par_iter()
        .filter(|index| test(shown.get(*index)))
        .count()
}

/// `count` of `points`, spread evenly over them.
fn thinned(points: Vec<IndexedPoint>, count: usize) -> Vec<IndexedPoint> {
    let total = points.len();
    if count >= total {
        return points;
    }
    if count == 0 {
        return Vec::new();
    }
    let mut kept = Vec::with_capacity(count);
    for (index, record) in points.into_iter().enumerate() {
        if index * count / total >= kept.len() {
            kept.push(record);
        }
    }
    kept
}

/// Run a refinement of part of the view on worker threads: a message for
/// each set it shows in between, and one for what it comes to.
pub(crate) fn merged_detail_task(revision: u64, refinement: MergeRefinement) -> Task<Message> {
    let stream = iced::futures::stream::unfold(Some(refinement), move |state| async move {
        let mut refinement = state?;
        let outcome = tokio::task::spawn_blocking(move || match refinement.advance() {
            Ok(Some(preview)) => Ok((false, preview, Some(refinement))),
            Ok(None) => Ok((true, refinement.finish(), None)),
            Err(error) => Err(error),
        })
        .await;
        Some(match outcome {
            Ok(Ok((done, sets, next))) => (Message::DetailMerged(revision, done, Ok(sets)), next),
            Ok(Err(error)) => (Message::DetailMerged(revision, true, Err(error)), None),
            Err(error) => (
                Message::DetailMerged(revision, true, Err(error.to_string())),
                None,
            ),
        })
    });
    Task::run(stream, |message| message)
}

/// A refinement of part of the view: it reads the nodes of that part and
/// merges what they give with what is drawn.
pub(crate) struct MergeRefinement {
    pub lod: LodRefinement,
    merge: DetailMerge,
    /// Whether what is drawn was looked at, and whether a read can add to it.
    prepared: Option<bool>,
    /// Whether a set was shown in between.
    previewed: bool,
}

impl MergeRefinement {
    pub(crate) fn new(lod: LodRefinement, merge: DetailMerge) -> Self {
        Self {
            lod,
            merge,
            prepared: None,
            previewed: false,
        }
    }

    /// Read on until there is a set that adds points in view, to be shown in
    /// between, or `None` once the read is over: `finish` then gives what it
    /// comes to.
    pub(crate) fn advance(&mut self) -> Result<Option<MergedSets>, String> {
        let Self {
            lod,
            merge,
            prepared,
            previewed,
        } = self;
        if prepared.is_none() {
            let worth = merge.prepare(&lod.sources, lod.projection);
            *prepared = Some(worth);
            if !worth {
                return Ok(None);
            }
            if merge.room() < lod.budget {
                // The part outside the box shares what the points kept
                // inside it leave of the budget.
                let (room, budget) = (merge.room() as u128, lod.budget.max(1) as u128);
                for limit in &mut lod.requested {
                    *limit = ((*limit as u128 * room / budget) as usize).max(1);
                }
                lod.budget = merge.room();
            }
        }
        if *prepared == Some(false) {
            return Ok(None);
        }
        let mut preview = None;
        let more = lod.advance_until(|lod| {
            let (sets, count) = merge.candidate(&lod.sources, &lod.samples, lod.projection);
            if count > merge.replaced {
                merge.replaced = count;
                preview = Some(sets);
                true
            } else {
                false
            }
        })?;
        *previewed |= more;
        Ok(if more {
            preview.map(|sets| merge.wrap(sets))
        } else {
            None
        })
    }

    /// What the refinement comes to once its read is over: the sets read,
    /// when they add points in view, or when the camera moved since what is
    /// drawn was read; `Unchanged` keeps what is drawn, which may be a set
    /// of this refinement shown in between.
    pub(crate) fn finish(self) -> MergedSets {
        if self.prepared != Some(true) {
            return MergedSets::Unchanged;
        }
        let (sets, count) =
            self.merge
                .candidate(&self.lod.sources, &self.lod.samples, self.lod.projection);
        let adds = count > self.merge.replaced;
        let ahead = !self.merge.strict && (!self.previewed || count >= self.merge.replaced);
        if adds || ahead {
            self.merge.wrap(sets)
        } else {
            MergedSets::Unchanged
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};

    use iced::Size;
    use pointcloud_core::IndexConfig;

    use super::*;
    use crate::gpu_viewport::{drawn_frame, RenderCache};
    use crate::{DetailRun, ScreenFill};

    const BUDGET: u32 = 18_000;

    /// A floor of 300 by 300 points in an octree of small leaves, framed in
    /// a view of 800 by 600 pixels, with a budget of a fifth of its points.
    fn indexed_floor(directory: &Path) -> Studio {
        let source = directory.join("floor.xyz");
        let lines: String = (0..300 * 300)
            .map(|index| {
                format!(
                    "{} {} {}\n",
                    index % 300,
                    index / 300,
                    f64::from(index % 7) * 0.01
                )
            })
            .collect();
        std::fs::write(&source, lines).unwrap();
        let cloud = pointcloud_core::open(&source, 300 * 300).unwrap();
        let config = IndexConfig {
            leaf_points: 2_000,
            ..IndexConfig::default()
        };
        let tree = Arc::new(OctreeIndex::build(&cloud, config).unwrap());
        let mut studio = Studio {
            budget: BUDGET,
            viewport_size: Size::new(800.0, 600.0),
            ..Studio::default()
        };
        let _ = studio.update(crate::Message::Loaded(Ok(Arc::new(cloud))));
        studio.clouds[0].index = Some(tree);
        studio
    }

    /// Run the refinement that the window would start now to its end, and
    /// give each of its messages to the studio as the window does; `frame`
    /// looks at the scene after each. Answers what it read for and whether
    /// it read anything, or `None` when nothing was started.
    fn refine(studio: &mut Studio, mut frame: impl FnMut(&Studio)) -> Option<(DetailPlan, bool)> {
        let run = studio.begin_detail()?;
        let plan = studio.detail_request.as_ref().unwrap().plan;
        let read = match run {
            DetailRun::Whole(revision, mut refinement) => {
                while let Some(preview) = refinement.advance().unwrap() {
                    let _ = studio.update(crate::Message::DetailPreview(revision, preview));
                    frame(studio);
                }
                let _ = studio.update(crate::Message::DetailReady(
                    revision,
                    Ok(refinement.finish()),
                ));
                true
            }
            DetailRun::Merge(revision, mut refinement) => {
                while let Some(preview) = refinement.advance().unwrap() {
                    let _ =
                        studio.update(crate::Message::DetailMerged(revision, false, Ok(preview)));
                    frame(studio);
                }
                let read = refinement.lod.sampled_limits.iter().any(|limit| *limit > 0);
                let _ = studio.update(crate::Message::DetailMerged(
                    revision,
                    true,
                    Ok(refinement.finish()),
                ));
                read
            }
        };
        assert!(!studio.detail_pending);
        assert!(studio.detail_loaded());
        frame(studio);
        Some((plan, read))
    }

    /// The ordinals of a set of points.
    fn ordinals(points: &[IndexedPoint]) -> HashSet<u64> {
        points.iter().map(|record| record.ordinal).collect()
    }

    #[test]
    fn the_box_adds_detail_inside_it_and_never_shows_fewer_points() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = indexed_floor(directory.path());
        let state = RefCell::new(RenderCache::default());
        assert_eq!(refine(&mut studio, |_| {}), Some((DetailPlan::Whole, true)));
        let view_set = Arc::clone(studio.clouds[0].detail_points.as_ref().unwrap());
        assert!((10_000..=BUDGET as usize).contains(&view_set.len()));
        let open = drawn_frame(&studio, &state);

        // The box switched on holds the whole floor, and so the budget
        // already: it reads nothing, now or when it is switched on again.
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        assert_eq!(drawn_frame(&studio, &state), open);
        let region = studio.section_box().unwrap();
        let strict = DetailPlan::Inside {
            region,
            strict: true,
        };
        assert_eq!(refine(&mut studio, |_| {}), Some((strict, false)));
        assert!(studio.clouds[0].focus_points.is_none());
        assert!(refine(&mut studio, |_| {}).is_none());

        // Two faces pulled in leave a ninth of the floor in the box: the
        // frame clips at once and sends nothing.
        let _ = studio.update(crate::Message::SectionMax(0, 30.0));
        let _ = studio.update(crate::Message::SectionMax(1, 30.0));
        let boxed = drawn_frame(&studio, &state);
        assert_eq!((boxed.points, boxed.focus), (open.points, open.focus));
        assert!(boxed.drawn * 5 < open.drawn, "{boxed:?}");
        assert_eq!(boxed.in_view, boxed.drawn);

        // The box is read, and its points come beside those of the view:
        // no frame shows fewer points than the one before.
        let mut last = boxed.in_view;
        let region = studio.section_box().unwrap();
        let (plan, read) = refine(&mut studio, |studio| {
            let frame = drawn_frame(studio, &state);
            assert!(frame.in_view >= last, "{} after {last}", frame.in_view);
            assert_eq!(frame.points, open.points, "the view set was sent again");
            last = frame.in_view;
        })
        .unwrap();
        assert_eq!(
            plan,
            DetailPlan::Inside {
                region,
                strict: true
            }
        );
        assert!(read);
        let added = drawn_frame(&studio, &state);
        assert!(added.drawn > boxed.drawn * 3, "{added:?} {boxed:?}");
        assert!(Arc::ptr_eq(
            studio.clouds[0].detail_points.as_ref().unwrap(),
            &view_set
        ));
        // They lie in the box, repeat none of the view and stay within the
        // budget.
        let focus = Arc::clone(studio.clouds[0].focus_points.as_ref().unwrap());
        assert!(focus.iter().all(|record| region.contains(record.point.xyz)));
        assert!(ordinals(&focus).is_disjoint(&ordinals(&view_set)));
        assert!(focus.len() <= BUDGET as usize);
        assert_eq!(added.drawn, inside(&studio, region));

        // Off: nothing is read or sent, and the detail inside stays.
        let _ = studio.update(crate::Message::SetSectionEnabled(false));
        let off = drawn_frame(&studio, &state);
        assert_eq!((off.points, off.focus), (added.points, added.focus));
        assert_eq!(off.drawn, view_set.len() + focus.len());
        assert!(refine(&mut studio, |_| {}).is_none());
        assert!(studio.detail_loaded());

        // On again: this box was read for this view already.
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        assert_eq!(drawn_frame(&studio, &state), added);
        assert!(refine(&mut studio, |_| {}).is_none());
        assert_eq!(drawn_frame(&studio, &state), added);
    }

    /// The points of the floor drawn inside a box: of the sets of the view
    /// and those read inside the box.
    fn inside(studio: &Studio, region: OrientedBox) -> usize {
        let entry = &studio.clouds[0];
        entry
            .view_records()
            .chain(entry.focus_records())
            .filter(|record| region.contains(record.point.xyz))
            .count()
    }

    #[test]
    fn a_box_switched_off_after_the_camera_moved_reads_only_its_outside() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = indexed_floor(directory.path());
        let state = RefCell::new(RenderCache::default());
        refine(&mut studio, |_| {});
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        let _ = studio.update(crate::Message::SectionMax(0, 30.0));
        let _ = studio.update(crate::Message::SectionMax(1, 30.0));
        refine(&mut studio, |_| {});
        let region = studio.section_box().unwrap();

        // Zoomed in on the box, its detail is read anew for the new view;
        // the sets of the view, hidden by the box, stay as they were.
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let (x, y, _) = studio
            .projection(scene, 800.0, 600.0)
            .project(region.center())
            .unwrap();
        let _ = studio.update(crate::Message::Zoom(5.0, [x, y], Size::new(800.0, 600.0)));
        let view_set = Arc::clone(studio.clouds[0].detail_points.as_ref().unwrap());
        let (plan, read) = refine(&mut studio, |_| {}).unwrap();
        assert_eq!(
            plan,
            DetailPlan::Inside {
                region,
                strict: false
            }
        );
        assert!(read);
        assert!(Arc::ptr_eq(
            studio.clouds[0].detail_points.as_ref().unwrap(),
            &view_set
        ));
        let focus = Arc::clone(studio.clouds[0].focus_points.as_ref().unwrap());

        // Off: only the part outside the box is read, and no frame shows
        // fewer points than the one before.
        let _ = studio.update(crate::Message::SetSectionEnabled(false));
        let off = drawn_frame(&studio, &state);
        let mut last = off.in_view;
        let (plan, read) = refine(&mut studio, |studio| {
            let frame = drawn_frame(studio, &state);
            assert!(frame.in_view >= last, "{} after {last}", frame.in_view);
            assert_eq!(
                frame.focus, off.focus,
                "the detail inside the box was sent again"
            );
            last = frame.in_view;
        })
        .unwrap();
        assert_eq!(plan, DetailPlan::Outside { region });
        assert!(read);
        // The detail inside the box stays, the sets keep their points inside
        // it, and they stay within the budget.
        assert!(Arc::ptr_eq(
            studio.clouds[0].focus_points.as_ref().unwrap(),
            &focus
        ));
        let sets = Arc::clone(studio.clouds[0].detail_points.as_ref().unwrap());
        assert!(!Arc::ptr_eq(&sets, &view_set));
        assert!(sets.len() <= BUDGET as usize);
        let kept: Vec<IndexedPoint> = view_set
            .iter()
            .copied()
            .filter(|record| region.contains(record.point.xyz))
            .collect();
        assert!(ordinals(&kept).is_subset(&ordinals(&sets)));
        assert!(drawn_frame(&studio, &state).in_view > off.in_view);
        // That view is read now.
        assert!(refine(&mut studio, |_| {}).is_none());
    }

    #[test]
    fn a_set_with_fewer_points_in_the_box_is_never_shown() {
        let directory = tempfile::tempdir().unwrap();
        let studio = indexed_floor(directory.path());
        let tree = Arc::clone(studio.clouds[0].index.as_ref().unwrap());
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let projection = studio.projection(scene, 800.0, 600.0);
        let region = OrientedBox::from(Bounds {
            min: [0.0, 0.0, -1.0],
            max: [99.5, 99.5, 1.0],
        });
        // What the box shows now: 5,000 points read inside it earlier.
        let shown: Arc<[IndexedPoint]> = studio.clouds[0]
            .view_records()
            .filter(|record| region.contains(record.point.xyz))
            .take(5_000)
            .collect();
        assert_eq!(shown.len(), 5_000);
        // A read of only a few hundred points there, as a first pass gives.
        let refinement = |strict: bool| {
            let total = usize::try_from(tree.root.total_points).unwrap();
            let lod = LodRefinement {
                sources: vec![(0, Arc::clone(&tree), CloudTransform::default(), 1.0)],
                source_weights: vec![(1.0, total)],
                requested: vec![400],
                sampled_limits: vec![0],
                samples: vec![Vec::new()],
                section: Some(region),
                exclude: None,
                projection,
                cancel: Arc::new(AtomicBool::new(false)),
                budget: 400,
                deep_zoom: false,
                pace: Arc::default(),
                shown: ScreenFill::default(),
                drawn_elsewhere: 0,
                pass: 0,
            };
            let merge = DetailMerge::new(
                DetailPlan::Inside { region, strict },
                BUDGET as usize,
                1,
                vec![ShownSet::Detail(Arc::from(Vec::new()))],
                vec![Some(Arc::clone(&shown))],
            )
            .unwrap();
            MergeRefinement::new(lod, merge)
        };

        // When only the box changed it is neither shown in between nor at
        // the end.
        let mut strict = refinement(true);
        assert!(strict.advance().unwrap().is_none());
        assert!(strict.lod.sampled_limits[0] > 0, "it was read");
        assert!(matches!(strict.finish(), MergedSets::Unchanged));
        // After the camera moved it is the set of the new view.
        let mut moved = refinement(false);
        assert!(moved.advance().unwrap().is_none());
        assert!(matches!(moved.finish(), MergedSets::Inside(_, sets) if sets[0].1.len() <= 400));
    }

    #[test]
    fn dragging_the_box_starts_one_read_at_the_end() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = indexed_floor(directory.path());
        refine(&mut studio, |_| {});
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        let _ = studio.update(crate::Message::SectionMax(0, 60.0));

        // A read of the box runs when a drag of one of its faces begins.
        let _ = studio.update(crate::Message::LoadDetail);
        assert!(studio.detail_pending);
        let running = Arc::clone(&studio.detail_cancel);
        let started = studio.detail_request_revision.unwrap();

        // Every step of the drag moves the face; the first one stops the
        // read, and none starts another.
        let mut steps = Vec::new();
        for _ in 0..10 {
            let _ = studio.update(crate::Message::SectionHandleDelta(0, false, -2.0));
            assert!(running.load(Ordering::Relaxed));
            assert_eq!(studio.detail_request_revision, Some(started));
            steps.push(studio.revision);
        }
        let _ = studio.update(crate::Message::DetailMerged(
            started,
            true,
            Err("Operation cancelled".into()),
        ));
        assert!(!studio.detail_pending);

        // The wait after each step ends: only the last step starts a read.
        for step in &steps[..9] {
            let _ = studio.update(crate::Message::RefreshDetail(*step));
            assert!(!studio.detail_pending, "a read started for an earlier step");
        }
        let _ = studio.update(crate::Message::RefreshDetail(steps[9]));
        assert!(studio.detail_pending);
        assert_eq!(studio.detail_request_revision, Some(steps[9]));
        assert!(matches!(
            studio.detail_request.as_ref().unwrap().plan,
            DetailPlan::Inside { strict: true, .. }
        ));
    }
}
