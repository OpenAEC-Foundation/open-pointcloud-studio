//! Detail inside the section box.
//!
//! The section box only clips what is drawn: the sets of points read for the
//! view stay on the graphics device whatever the box does, and the shader
//! leaves out what lies outside it. While the box is on, the octree is read
//! once more for the nodes inside it, and the points that this adds to the
//! sets of the view are drawn beside them, so that all the box shows stays
//! within the budget. The box so gets the detail of a read of its own without
//! a point leaving the screen first, and the points it shows only ever grow
//! while the camera stands still.
//!
//! Switched off, the box shows the sets of the view again. When they were
//! read for the view as it is, the detail read inside the box is put aside
//! and only the sets are drawn: switched on again, the box shows that detail
//! at once. When the camera moved while the box was on, the detail stays in
//! sight until the whole view has been read again, and is then let go.

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
    /// Put aside while the box is off: the sets of the view were read for
    /// the view as it is, and they alone are drawn.
    pub hidden: bool,
}

/// A section box that was read in full for a view.
#[derive(Debug, Clone)]
pub(crate) struct SectionRead {
    pub region: OrientedBox,
    pub view: DetailView,
    /// The sets that were drawn then, as `Studio::sets_revision` counts
    /// them: while they still are, reading the box again for that view
    /// gives nothing new.
    pub sets: u64,
}

/// What the next refinement of the view reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DetailPlan {
    /// Nothing to read: the box is off and the sets are those of this view,
    /// or the box is on and was read for this view and these sets already.
    Nothing,
    /// The whole view: its sets replace those drawn and the points read
    /// inside a box are let go.
    Whole,
    /// The nodes inside the box, whose new points are drawn beside the sets
    /// of the view. `strict` when only the box changed since what is shown
    /// was read: the points read then take the place of those added before
    /// only when they are more, and nothing is read when they could not be.
    Inside { region: OrientedBox, strict: bool },
}

/// What a refinement inside the section box gives.
#[derive(Debug, Clone)]
pub(crate) enum MergedSets {
    /// Points to draw beside the sets of the view, read inside this box, by
    /// cloud; the clouds left out have none.
    Inside(OrientedBox, LodSets),
    /// Nothing that would add a point: what is shown stays.
    Unchanged,
}

impl Studio {
    /// Take what a refinement inside the section box gives: a set to show
    /// in between, or with `done` what it comes to at its end.
    pub(crate) fn take_merged_sets(&mut self, revision: u64, sets: MergedSets, done: bool) {
        let request = self
            .detail_request
            .clone()
            .filter(|request| request.revision == revision);
        let view = request
            .as_ref()
            .filter(|_| done)
            .map(|request| request.view.clone());
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
                self.sets_revision += 1;
                self.focus = Some(FocusDetail {
                    region,
                    view,
                    hidden: false,
                });
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
            MergedSets::Unchanged => {
                let Some((DetailPlan::Inside { region, .. }, seen)) = request
                    .as_ref()
                    .map(|request| (request.plan, &request.view))
                else {
                    return;
                };
                if let Some(focus) = self.focus.as_mut().filter(|focus| focus.region == region) {
                    focus.view = Some(seen.clone());
                }
                i18n::tr("Nothing to add inside the section box: it shows as many points as the budget allows or the scan has").to_owned()
            }
        };
        // The box was read in full for this view: it needs no read again
        // while these sets are drawn.
        if let Some((DetailPlan::Inside { region, .. }, view)) = request
            .filter(|_| done)
            .map(|request| (request.plan, request.view))
        {
            self.section_read = Some(SectionRead {
                region,
                view,
                sets: self.sets_revision,
            });
        }
        if self.reports_detail() {
            self.status = status;
        }
    }

    /// What the next refinement reads, for the camera, layers and budget of
    /// `view`.
    pub(crate) fn detail_plan(&self, view: &DetailView) -> DetailPlan {
        let Some(region) = self.section_box() else {
            return if self.detail_view.as_ref() == Some(view) {
                DetailPlan::Nothing
            } else {
                DetailPlan::Whole
            };
        };
        if self.section_read.as_ref().is_some_and(|read| {
            read.region == region && read.view == *view && read.sets == self.sets_revision
        }) {
            // This box was read for this view, and nothing drawn changed
            // since.
            return DetailPlan::Nothing;
        }
        let strict = match &self.focus {
            Some(focus) => focus.view.as_ref() == Some(view),
            None => self.detail_view.as_ref() == Some(view),
        };
        DetailPlan::Inside { region, strict }
    }

    /// Whether the points read inside the section box are drawn: while the
    /// box is on, and after it went off until they are put aside or let go.
    pub(crate) fn focus_drawn(&self) -> bool {
        self.section_box().is_some() || !self.focus.as_ref().is_some_and(|focus| focus.hidden)
    }

    /// Put the points read inside the section box aside once the box is off
    /// and the sets of the view were read for the view as it is, so that the
    /// view draws no more than its budget; take them back when the box comes
    /// on again.
    pub(crate) fn settle_focus(&mut self) {
        let boxed = self.section_box().is_some();
        let Some(focus) = &self.focus else {
            return;
        };
        if boxed || focus.hidden {
            if let Some(focus) = self.focus.as_mut().filter(|_| boxed) {
                focus.hidden = false;
            }
            return;
        }
        if self.detail_view.is_some() && self.detail_view == self.detail_view_now() {
            if let Some(focus) = &mut self.focus {
                focus.hidden = true;
            }
        }
    }
}

/// How much of the section box an octree node reaches.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Reach {
    Nothing,
    Some,
    All,
}

fn reach(region: OrientedBox, bounds: Bounds) -> Reach {
    let around = region.aabb();
    if (0..3).any(|axis| bounds.max[axis] < around.min[axis] || bounds.min[axis] > around.max[axis])
    {
        return Reach::Nothing;
    }
    if pointcloud_core::bounds_corners(bounds)
        .into_iter()
        .all(|corner| region.contains(corner))
    {
        Reach::All
    } else {
        Reach::Some
    }
}

/// The most points of a cloud there can be inside the box and in view: those
/// of the nodes that reach into it and are in view, without reading any.
fn capacity(
    region: OrientedBox,
    node: &IndexedNode,
    transform: CloudTransform,
    projection: Projection,
) -> u64 {
    let bounds = transform.bounds(node.bounds);
    if projection.screen_span(bounds).is_none() {
        return 0;
    }
    match reach(region, bounds) {
        Reach::Nothing => 0,
        Reach::All => node.total_points,
        Reach::Some if node.is_leaf() => node.total_points,
        Reach::Some => node
            .children
            .iter()
            .map(|child| capacity(region, child, transform, projection))
            .sum(),
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

/// How the points read inside the section box meet what is drawn.
pub(crate) struct DetailMerge {
    region: OrientedBox,
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
    /// The points of the sets of the view inside the box, as the renderer
    /// draws them: they are part of what the box shows, and so of its
    /// budget.
    kept: usize,
    /// The points in view that the merge would replace, as drawn now.
    replaced: usize,
}

impl DetailMerge {
    /// A merge for `plan`, which reads inside the box, with what the sources
    /// draw now.
    pub(crate) fn new(
        plan: DetailPlan,
        budget: usize,
        stride: usize,
        shown: Vec<ShownSet>,
        added: Vec<Option<Arc<[IndexedPoint]>>>,
    ) -> Option<Self> {
        let DetailPlan::Inside { region, strict } = plan else {
            return None;
        };
        Some(Self {
            region,
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
    /// could add next to nothing: the sets of the view take about the whole
    /// budget in the box already, about everything there is in the box in
    /// view is drawn already, or, when only the box changed, it shows about
    /// a whole budget already.
    fn prepare(
        &mut self,
        sources: &[(usize, Arc<OctreeIndex>, CloudTransform, f32)],
        projection: Projection,
    ) -> bool {
        let region = self.region;
        let in_view = |xyz: [f64; 3]| projection.project(xyz).is_some();
        let mut present = 0usize;
        let mut kept = 0usize;
        let mut most = 0u64;
        for (slot, (_, tree, transform, _)) in sources.iter().enumerate() {
            let transform = *transform;
            let shown = &self.shown[slot];
            most = most.saturating_add(capacity(region, &tree.root, transform, projection));
            let mut held: Vec<u64> = (0..shown.len())
                .into_par_iter()
                .map(|index| shown.get(index))
                .filter(|record| region.contains(transform.xyz(record.point.xyz)))
                .map(|record| record.ordinal)
                .collect();
            kept += held.len();
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
        self.kept = kept / self.stride;
        // A read is worth it when it can add more than a small part of the
        // budget: up to the points there are in view, and, when only the box
        // changed, up to the budget.
        let least = (self.budget / LEAST_GAIN).max(1);
        if self.strict {
            most = most.min(self.budget as u64);
        }
        self.room() >= least && (present + self.replaced + least) as u64 <= most
    }

    /// Points the read inside the box may add: the budget less the points
    /// of the sets of the view in it.
    fn room(&self) -> usize {
        self.budget.saturating_sub(self.kept)
    }

    /// The sets that the points read so far give, by cloud, and how many of
    /// the points they bring in are in view.
    fn candidate(
        &self,
        sources: &[(usize, Arc<OctreeIndex>, CloudTransform, f32)],
        samples: &[Vec<IndexedPoint>],
        projection: Projection,
    ) -> (LodSets, usize) {
        let region = self.region;
        let in_view = |xyz: [f64; 3]| projection.project(xyz).is_some();
        // Per source: the points read that are new in the box.
        let fresh: Vec<Vec<IndexedPoint>> = sources
            .iter()
            .zip(samples)
            .enumerate()
            .map(|(slot, ((_, _, transform, _), points))| {
                let held = &self.held[slot];
                points
                    .par_iter()
                    .filter(|record| {
                        region.contains(transform.xyz(record.point.xyz))
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
        for ((index, _, transform, _), points) in sources.iter().zip(fresh) {
            // All that the box shows stays within the budget: the points read
            // take what the sets of the view leave of it in the box, each
            // source its share.
            let points = if total > room {
                let share = points.len() as u128 * room as u128 / total as u128;
                thinned(points, share as usize)
            } else {
                points
            };
            count += points
                .par_iter()
                .filter(|record| in_view(transform.xyz(record.point.xyz)))
                .count();
            sets.push((*index, points));
        }
        (sets, count)
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

/// Run a refinement inside the section box on worker threads: a message for
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

/// A refinement inside the section box: it reads the nodes inside the box
/// and merges what they give with what is drawn.
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
        let worth = *prepared.get_or_insert_with(|| merge.prepare(&lod.sources, lod.projection));
        if !worth {
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
            preview.map(|sets| MergedSets::Inside(merge.region, sets))
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
            MergedSets::Inside(self.merge.region, sets)
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
    use crate::{CameraPreset, DetailRun, ScreenFill};

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

    /// What Properties shows as the view sample once the count that the
    /// last change asked for was made.
    fn view_sample(studio: &mut Studio) -> usize {
        if let Some((_, asked)) = studio.box_sample_asked.clone() {
            let _ = studio.update(crate::Message::CountBoxSample(asked));
        }
        studio.view_sample()
    }

    /// The box over the middle ninth of the floor.
    fn small_box(studio: &mut Studio) -> OrientedBox {
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        let _ = studio.update(crate::Message::SectionMax(0, 30.0));
        let _ = studio.update(crate::Message::SectionMax(1, 30.0));
        studio.section_box().unwrap()
    }

    #[test]
    fn the_box_adds_detail_inside_it_within_the_budget_and_never_shows_fewer_points() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = indexed_floor(directory.path());
        let state = RefCell::new(RenderCache::default());
        assert_eq!(refine(&mut studio, |_| {}), Some((DetailPlan::Whole, true)));
        let view_set = Arc::clone(studio.clouds[0].detail_points.as_ref().unwrap());
        assert!((10_000..=BUDGET as usize).contains(&view_set.len()));
        let open = drawn_frame(&studio, &state);
        assert_eq!(view_sample(&mut studio), view_set.len());

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
        let region = small_box(&mut studio);
        let boxed = drawn_frame(&studio, &state);
        assert_eq!((boxed.points, boxed.focus), (open.points, open.focus));
        assert!(boxed.drawn * 5 < open.drawn, "{boxed:?}");
        assert_eq!(boxed.in_view, boxed.drawn);
        assert_eq!(view_sample(&mut studio), boxed.drawn);

        // The box is read, and its points come beside those of the view:
        // no frame shows fewer points than the one before.
        let mut last = boxed.in_view;
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
        // They lie in the box and repeat none of the view, and the box
        // shows no more than the budget.
        let focus = Arc::clone(studio.clouds[0].focus_points.as_ref().unwrap());
        assert!(focus.iter().all(|record| region.contains(record.point.xyz)));
        assert!(ordinals(&focus).is_disjoint(&ordinals(&view_set)));
        assert_eq!(added.drawn, inside(&studio, region));
        assert!(added.drawn <= BUDGET as usize);
        assert_eq!(view_sample(&mut studio), added.drawn);

        // Off with the camera where the sets were read: those sets alone
        // are drawn, within the budget, while the detail inside the box
        // stays on the device; nothing is read or sent.
        let _ = studio.update(crate::Message::SetSectionEnabled(false));
        assert!(!studio.focus_drawn());
        let off = drawn_frame(&studio, &state);
        assert_eq!((off.points, off.focus), (added.points, added.focus));
        assert_eq!(off.drawn, view_set.len());
        assert_eq!((off.drawn, off.in_view), (open.drawn, open.in_view));
        assert!(refine(&mut studio, |_| {}).is_none());
        assert!(studio.detail_loaded());
        assert_eq!(view_sample(&mut studio), view_set.len());

        // On again: the detail shows at once, and this box was read for
        // this view and these sets already.
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        assert!(studio.focus_drawn());
        assert_eq!(drawn_frame(&studio, &state), added);
        assert!(refine(&mut studio, |_| {}).is_none());
        assert_eq!(drawn_frame(&studio, &state), added);
        assert_eq!(view_sample(&mut studio), added.drawn);
    }

    #[test]
    fn a_box_is_read_again_once_the_view_was_read_anew() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = indexed_floor(directory.path());
        let state = RefCell::new(RenderCache::default());
        let _ = studio.update(crate::Message::CameraPreset(CameraPreset::Top));
        refine(&mut studio, |_| {});
        let region = small_box(&mut studio);
        let (plan, read) = refine(&mut studio, |_| {}).unwrap();
        assert!(matches!(plan, DetailPlan::Inside { .. }) && read);
        let _ = studio.update(crate::Message::SetSectionEnabled(false));
        assert!(refine(&mut studio, |_| {}).is_none());

        // Another view and back, each read in full with the box off: the
        // points read inside the box are let go.
        let _ = studio.update(crate::Message::CameraPreset(CameraPreset::Front));
        assert_eq!(refine(&mut studio, |_| {}), Some((DetailPlan::Whole, true)));
        assert!(studio.focus.is_none());
        let _ = studio.update(crate::Message::CameraPreset(CameraPreset::Top));
        assert_eq!(refine(&mut studio, |_| {}), Some((DetailPlan::Whole, true)));
        let view = drawn_frame(&studio, &state);

        // The box on again for the view it was read for before: it is read
        // again, as the sets it was read beside are gone.
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        assert_eq!(studio.section_box(), Some(region));
        let boxed = drawn_frame(&studio, &state);
        assert_eq!(
            refine(&mut studio, |_| {}),
            Some((
                DetailPlan::Inside {
                    region,
                    strict: true
                },
                true
            ))
        );
        let added = drawn_frame(&studio, &state);
        assert_eq!(added.points, view.points);
        assert!(added.drawn > boxed.drawn * 3, "{added:?} {boxed:?}");
        assert!(added.drawn <= BUDGET as usize);
    }

    /// The points of a frame that lie outside a box, in view.
    fn outside_in_view(studio: &Studio, region: OrientedBox) -> usize {
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let projection = studio.projection(scene, 800.0, 600.0);
        studio.clouds[0]
            .view_records()
            .filter(|record| {
                !region.contains(record.point.xyz) && projection.project(record.point.xyz).is_some()
            })
            .count()
    }

    #[test]
    fn a_box_switched_off_after_the_camera_moved_shows_its_detail_until_the_view_is_read() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = indexed_floor(directory.path());
        let state = RefCell::new(RenderCache::default());
        let _ = studio.update(crate::Message::CameraPreset(CameraPreset::Top));
        refine(&mut studio, |_| {});

        // A box over all but the edges of the floor, and the view zoomed in
        // on it with the box off: the sets of the view lie nearly all in the
        // box, and hold most of the budget.
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        let _ = studio.update(crate::Message::SectionMin(0, 5.0));
        let _ = studio.update(crate::Message::SectionMin(1, 5.0));
        let _ = studio.update(crate::Message::SectionMax(0, 95.0));
        let _ = studio.update(crate::Message::SectionMax(1, 95.0));
        let region = studio.section_box().unwrap();
        let _ = studio.update(crate::Message::SetSectionEnabled(false));
        let framed = outside_in_view(&studio, region);
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let (x, y, _) = studio
            .projection(scene, 800.0, 600.0)
            .project(region.center())
            .unwrap();
        let _ = studio.update(crate::Message::Zoom(8.0, [x, y], Size::new(800.0, 600.0)));
        assert_eq!(refine(&mut studio, |_| {}), Some((DetailPlan::Whole, true)));
        let near = Arc::clone(studio.clouds[0].detail_points.as_ref().unwrap());
        let held = near
            .iter()
            .filter(|record| region.contains(record.point.xyz))
            .count();
        // What they leave of the budget is less than the part of the view
        // outside the box had before.
        assert!(
            (BUDGET as usize - held) * 10 < framed * 9,
            "{held} of {}",
            near.len()
        );

        // The box on, then the view zoomed out again: the box gets what the
        // sets of the view leave of the budget in it, and no more.
        let _ = studio.update(crate::Message::SetSectionEnabled(true));
        refine(&mut studio, |_| {});
        let _ = studio.update(crate::Message::Zoom(-8.0, [x, y], Size::new(800.0, 600.0)));
        let (plan, _) = refine(&mut studio, |_| {}).unwrap();
        assert_eq!(
            plan,
            DetailPlan::Inside {
                region,
                strict: false
            }
        );
        assert!(studio.clouds[0].focus_points.is_some());
        assert!(inside(&studio, region) <= BUDGET as usize);
        assert!(Arc::ptr_eq(
            studio.clouds[0].detail_points.as_ref().unwrap(),
            &near
        ));

        // Off: the sets of the view were read for another view, so the
        // detail inside the box stays in sight while the whole view is read
        // again, and is let go once it has been.
        let _ = studio.update(crate::Message::SetSectionEnabled(false));
        assert!(studio.focus_drawn());
        let off = drawn_frame(&studio, &state);
        assert_eq!(off.drawn, near.len() + studio.clouds[0].focus_len());
        let (plan, read) = refine(&mut studio, |studio| {
            if studio.focus.is_some() {
                let frame = drawn_frame(studio, &state);
                assert_eq!(
                    frame.focus, off.focus,
                    "the detail inside the box was sent again"
                );
                assert!(studio.focus_drawn());
            }
        })
        .unwrap();
        assert_eq!((plan, read), (DetailPlan::Whole, true));
        assert!(studio.focus.is_none() && studio.clouds[0].focus_points.is_none());

        // The whole view is read with the budget: the part outside the box
        // gets about as much as the read of that view before the box gave.
        let read = drawn_frame(&studio, &state);
        assert!(read.drawn <= BUDGET as usize);
        assert!((10_000..=BUDGET as usize).contains(&studio.clouds[0].view_len()));
        let outside = outside_in_view(&studio, region);
        assert!(outside * 10 >= framed * 9, "{outside} after {framed}");
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
    fn the_points_read_in_the_box_take_only_what_the_view_leaves_of_the_budget() {
        let directory = tempfile::tempdir().unwrap();
        let studio = indexed_floor(directory.path());
        let tree = Arc::clone(studio.clouds[0].index.as_ref().unwrap());
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let projection = studio.projection(scene, 800.0, 600.0);
        let region = OrientedBox::from(Bounds {
            min: [0.0, 0.0, -1.0],
            max: [149.5, 149.5, 1.0],
        });
        // The sets of the view hold 15,000 points in the box, of a budget
        // of 18,000 that a read inside it would fill.
        let view: Arc<[IndexedPoint]> = (0..150u32 * 150)
            .filter(|index| index % 3 != 0)
            .map(|index| IndexedPoint {
                point: pointcloud_core::Point {
                    xyz: [f64::from(index % 150), f64::from(index / 150), 0.0],
                    rgb: None,
                    intensity: None,
                    classification: None,
                },
                ordinal: u64::from(index % 150 + index / 150 * 300),
            })
            .collect();
        assert_eq!(view.len(), 15_000);
        let total = usize::try_from(tree.root.total_points).unwrap();
        let lod = LodRefinement {
            sources: vec![(0, Arc::clone(&tree), CloudTransform::default(), 1.0)],
            source_weights: vec![(1.0, total)],
            requested: vec![BUDGET as usize],
            sampled_limits: vec![0],
            samples: vec![Vec::new()],
            section: Some(region),
            projection,
            cancel: Arc::new(AtomicBool::new(false)),
            budget: BUDGET as usize,
            deep_zoom: false,
            pace: Arc::default(),
            shown: ScreenFill::default(),
            drawn_elsewhere: 0,
            pass: 0,
        };
        let merge = DetailMerge::new(
            DetailPlan::Inside {
                region,
                strict: false,
            },
            BUDGET as usize,
            1,
            vec![ShownSet::Detail(Arc::clone(&view))],
            vec![None],
        )
        .unwrap();
        let mut refinement = MergeRefinement::new(lod, merge);
        while refinement.advance().unwrap().is_some() {}
        let MergedSets::Inside(_, sets) = refinement.finish() else {
            panic!("nothing read");
        };
        let added = &sets[0].1;
        assert!(!added.is_empty());
        assert!(
            added.len() <= BUDGET as usize - view.len(),
            "{}",
            added.len()
        );
        assert!(ordinals(added).is_disjoint(&ordinals(&view)));
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

        // The count of the points drawn in the box waits for the drag too:
        // only the last step has it made.
        let asked = studio.box_sample_asked.clone().unwrap();
        let _ = studio.update(crate::Message::CountBoxSample(asked.1 - 1));
        assert!(studio.box_sample.is_none());
        let _ = studio.update(crate::Message::CountBoxSample(asked.1));
        assert_eq!(
            studio.box_sample.as_ref().map(|(key, _)| key),
            Some(&asked.0)
        );
    }
}
