//! Step 0 of Mesh to Plans, the preparation. The worker finds the box
//! around what was scanned, the main direction of the walls, reads the scene
//! once into a volume of occupied cells, proposes the footprint and finds
//! and refines the levels. The page shows what it found: a histogram of the
//! horizontal area per height with a line per level that can be dragged
//! (in steps of 5 cm, freely with Shift), a view from above with the boxes
//! of the core, the building and the site, the footprint and the main
//! directions, and the table of the levels. There the levels are named,
//! added, merged, removed and given their cut, P is chosen, a level is shown
//! in the model through the section box, and Confirm levels locks the step.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use iced::alignment;
use iced::widget::canvas::{self, event, Canvas, Frame, Geometry};
use iced::widget::{button, column, container, image, row, scrollable, stack, text, text_input};
use iced::{
    keyboard, mouse, Border, Color, Element, Fill, Point, Rectangle, Renderer, Size, Task, Theme,
};
use pointcloud_core::plans::{
    building_frame, detect_levels, refine_levels, robust_bounds, second_direction,
    slab_thicknesses, stray_limit, strays_below, survey_scene, BuildingFrame, Confidence,
    FootprintConfig, Level, LevelConfig, LevelKind, LevelStatus, PlanRegion, RefineConfig,
    RobustBoundsConfig, SceneSurvey, StrayConfig, SurveyConfig,
};
use pointcloud_core::region_source::{resident_points, RegionSource};
use pointcloud_core::{Bounds, IndexedPoint, LoadError, OrientedBox};

use serde_json::{json, Value};

use super::project::{BoxRecord, Regions, SurveyRecord};
use super::{StepStatus, WizardAction, WizardStep};
use crate::closed_mesh::{Sentence, UNINDEXED_LIMIT};
use crate::i18n::{key, tr, tr_args};
use crate::job_scene::{JobLayer, JobScene};
use crate::{opencad_ribbon, Message, Studio};

/// What `mesh_to_plans_level` of the local API does with a level.
pub(crate) const LEVEL_ACTIONS: [&str; 6] =
    ["select", "show", "set_peil", "add", "merge", "remove"];

/// Levels snap to heights of this step above P while they are dragged.
pub(crate) const SNAP: f64 = 0.05;
/// A level line is taken by the pointer this many pixels away.
const GRAB: f32 = 6.0;
/// The building box reaches this far around the footprint.
const BUILDING_MARGIN: f64 = 1.0;

/// What the preparation reads: the layers, and what the user fixed.
pub(crate) struct PrepareInput {
    pub(crate) scene: JobScene,
    /// The part of the scene to survey; none for the box around the scans
    /// without the stray points far out.
    pub(crate) core: Option<OrientedBox>,
    /// The main direction of the walls; none to find it.
    pub(crate) rotation: Option<f64>,
    /// The project folder, where `survey/profile.csv` and `survey/top.png`
    /// go; none to write nothing.
    pub(crate) folder: Option<PathBuf>,
}

impl fmt::Debug for PrepareInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrepareInput")
            .field("layers", &self.scene.layers.len())
            .field("core", &self.core)
            .field("rotation", &self.rotation)
            .field("folder", &self.folder)
            .finish()
    }
}

/// The view from above of a survey: per column of the grid a colour for the
/// height of its highest cell, the walls dark; row 0 is the highest v.
#[derive(Clone, PartialEq)]
pub(crate) struct TopImage {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Arc<Vec<u8>>,
}

impl fmt::Debug for TopImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "TopImage({} by {})", self.width, self.height)
    }
}

impl TopImage {
    pub(crate) fn handle(&self) -> image::Handle {
        image::Handle::from_rgba(self.width, self.height, self.rgba.as_ref().clone())
    }
}

/// What the preparation found.
#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub(crate) survey: SurveyRecord,
    /// The box that was surveyed, the one around the building and the one
    /// around the site.
    pub(crate) core: OrientedBox,
    pub(crate) building: Option<OrientedBox>,
    pub(crate) site: OrientedBox,
    /// As found: their names are their codes.
    pub(crate) levels: Vec<Level>,
    pub(crate) top: Option<TopImage>,
    /// The files written in the project folder, and why one could not be.
    pub(crate) written: Vec<PathBuf>,
    pub(crate) write_error: Option<String>,
}

/// How far the parts of the preparation reach, in thousandths of the step.
const AFTER_READING: u64 = 50;
const AFTER_BOUNDS: u64 = 60;
const AFTER_FRAME: u64 = 120;
const AFTER_SURVEY: u64 = 800;
const AFTER_LEVELS: u64 = 960;
const AFTER_STRAYS: u64 = 980;

/// The part of the step between `from` and `to` that a fraction is.
fn between(from: u64, to: u64, fraction: f32) -> u64 {
    from + ((to - from) as f64 * f64::from(fraction.clamp(0.0, 1.0))).round() as u64
}

/// Run the preparation. This runs on the worker of the wizard; `report`
/// takes how far it is in thousandths and stops it when it was cancelled.
pub(crate) fn run(
    input: &PrepareInput,
    report: &(dyn Fn(u64) -> Result<(), LoadError> + Sync),
) -> Result<Prepared, LoadError> {
    let started = Instant::now();
    let scene = &input.scene;
    scene.validate()?;
    report(0)?;
    // The layers without an index are read into memory once.
    let mut resident: Vec<Option<Vec<IndexedPoint>>> = Vec::with_capacity(scene.layers.len());
    for layer in &scene.layers {
        resident.push(match layer.index {
            Some(_) => None,
            None => Some(resident_points(&layer.cloud, &mut |progress| {
                report(between(0, AFTER_READING, progress.fraction()))
            })?),
        });
    }
    let sources: Vec<RegionSource<'_>> = scene
        .layers
        .iter()
        .zip(&resident)
        .map(|(layer, resident)| {
            let transform = layer.source_transform();
            match resident {
                Some(points) => RegionSource::resident(points, transform),
                None => RegionSource::new(&layer.cloud, layer.index.as_deref(), transform),
            }
        })
        .collect();
    let keeps = |position: usize, ordinal: u64, point: &pointcloud_core::Point| {
        scene.keeps(position, ordinal, point)
    };
    report(AFTER_READING)?;
    let robust = robust_bounds(&sources, &RobustBoundsConfig::default())?
        .ok_or_else(|| LoadError::InvalidData("the scans hold no points".into()))?;
    let site = OrientedBox::from(robust.bounds);
    let core = input.core.unwrap_or(site);
    report(AFTER_BOUNDS)?;
    let center = core.center();
    let rounded = [center[0].round(), center[1].round()];
    let frame = match input.rotation {
        Some(rotation) => BuildingFrame::new(rotation, rounded),
        None => building_frame(&sources, core.aabb(), &keeps, &mut |_| report(AFTER_BOUNDS))?
            .unwrap_or_else(|| BuildingFrame::new(0.0, rounded)),
    };
    report(AFTER_FRAME)?;
    let mut survey = survey_scene(
        &sources,
        core,
        &frame,
        &keeps,
        &SurveyConfig::default(),
        &mut |progress| report(between(AFTER_FRAME, AFTER_SURVEY, progress.fraction())),
    )?;
    let footprint_config = FootprintConfig::default();
    let walls = survey.wall_columns(footprint_config.wall_height);
    survey.frame.second_direction_deg = second_direction(&survey, &walls);
    let footprint = survey.footprint_proposal(&footprint_config);
    let mut detection = detect_levels(&survey, &footprint, &LevelConfig::default());
    let count = detection.levels.len().max(1) as u64;
    refine_levels(
        &sources,
        &survey,
        &footprint,
        &mut detection,
        &keeps,
        &RefineConfig::default(),
        &mut |place, progress| {
            let share = (place as f64 + f64::from(progress.fraction())) / count as f64;
            report(between(AFTER_SURVEY, AFTER_LEVELS, share as f32))
        },
    )?;
    if let Some(peil) = detection.peil_z {
        survey.frame.peil_z = peil;
    }
    report(AFTER_LEVELS)?;
    // The stray points below the scene, one by one.
    let limit = stray_limit(
        robust.bounds.min[2],
        detection.levels.iter().map(|level| level.floor_z),
    );
    let strays = strays_below(
        &sources,
        limit,
        &keeps,
        &StrayConfig::default(),
        &mut |progress| report(between(AFTER_LEVELS, AFTER_STRAYS, progress.fraction())),
    )?;
    let top = top_image(&survey, &walls);
    let building = building_box(&survey.frame, &footprint.regions, &detection.levels);
    let record = SurveyRecord {
        grid: survey.grid,
        frame: survey.frame,
        level_histogram: detection.area_histogram.clone(),
        footprint: footprint.regions.clone(),
        footprint_area: footprint.area(),
        ground_z: detection.ground_z,
        below_z: Some(strays.limit_z),
        below_points: strays.points,
        below_clusters: strays.clusters,
        stats: survey.stats,
        seconds: started.elapsed().as_secs_f64(),
    };
    report(AFTER_STRAYS)?;
    let mut prepared = Prepared {
        survey: record,
        core,
        building,
        site,
        levels: detection.levels,
        top: Some(top),
        written: Vec::new(),
        write_error: None,
    };
    if let Some(folder) = &input.folder {
        match write_outputs(folder, &prepared) {
            Ok(written) => prepared.written = written,
            Err(error) => prepared.write_error = Some(error.to_string()),
        }
    }
    prepared.survey.seconds = started.elapsed().as_secs_f64();
    report(1000)?;
    Ok(prepared)
}

/// The box around the footprint with a margin, from below the lowest level
/// to above the highest, in the frame; none without a footprint.
fn building_box(
    frame: &BuildingFrame,
    footprint: &[PlanRegion],
    levels: &[Level],
) -> Option<OrientedBox> {
    let mut corners = footprint.iter().flat_map(|region| region.outer.iter());
    let first = *corners.next()?;
    let (mut low, mut high) = (first, first);
    for corner in corners {
        low = [low[0].min(corner[0]), low[1].min(corner[1])];
        high = [high[0].max(corner[0]), high[1].max(corner[1])];
    }
    let bottom = levels
        .iter()
        .map(|level| level.floor_z)
        .fold(f64::INFINITY, f64::min);
    let top = levels
        .iter()
        .map(|level| level.ceiling_z.unwrap_or(level.floor_z))
        .fold(f64::NEG_INFINITY, f64::max);
    let (bottom, top) = if bottom.is_finite() && top.is_finite() {
        (bottom - 0.5, top + 1.5)
    } else {
        (frame.peil_z - 1.0, frame.peil_z + 10.0)
    };
    Some(frame.oriented_box(Bounds {
        min: [
            low[0] - BUILDING_MARGIN,
            low[1] - BUILDING_MARGIN,
            bottom - frame.peil_z,
        ],
        max: [
            high[0] + BUILDING_MARGIN,
            high[1] + BUILDING_MARGIN,
            top - frame.peil_z,
        ],
    }))
}

/// A colour for a share of the heights: a cool grey-blue low, sand in the
/// middle and brick high.
fn height_colour(share: f64) -> [u8; 3] {
    const STOPS: [(f64, [f64; 3]); 3] = [
        (0.0, [96.0, 118.0, 140.0]),
        (0.45, [214.0, 205.0, 178.0]),
        (1.0, [168.0, 72.0, 44.0]),
    ];
    let share = share.clamp(0.0, 1.0);
    let upper = STOPS
        .iter()
        .position(|(at, _)| *at >= share)
        .unwrap_or(STOPS.len() - 1)
        .max(1);
    let (low_at, low) = STOPS[upper - 1];
    let (high_at, high) = STOPS[upper];
    let along = ((share - low_at) / (high_at - low_at)).clamp(0.0, 1.0);
    std::array::from_fn(|channel| {
        (low[channel] + (high[channel] - low[channel]) * along).round() as u8
    })
}

/// The view from above of a survey.
fn top_image(survey: &SceneSurvey, walls: &pointcloud_core::grid2d::Mask) -> TopImage {
    let grid = survey.grid;
    let [width, height, _] = grid.size;
    let highest: Vec<Option<u32>> = (0..grid.columns())
        .map(|column| survey.highest_bin(column))
        .collect();
    let mut sorted: Vec<u32> = highest.iter().flatten().copied().collect();
    sorted.sort_unstable();
    // The heights between the 2nd and the 99.5th percentile span the colours.
    let at = |share: f64| {
        sorted
            .get(((sorted.len().saturating_sub(1)) as f64 * share) as usize)
            .copied()
            .unwrap_or(0)
    };
    let (low, high) = (at(0.02), at(0.995).max(at(0.02) + 1));
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let column = grid.column(x, y);
            let row = (height - 1 - y) as usize;
            let pixel = (row * width as usize + x as usize) * 4;
            let Some(bin) = highest[column] else {
                continue;
            };
            let colour = if walls.get(x as i64, y as i64) {
                [44, 44, 50]
            } else {
                height_colour((bin.saturating_sub(low)) as f64 / (high - low) as f64)
            };
            rgba[pixel..pixel + 4].copy_from_slice(&[colour[0], colour[1], colour[2], 255]);
        }
    }
    TopImage {
        width,
        height,
        rgba: Arc::new(rgba),
    }
}

/// Write what the survey found beside the project file: the area per
/// height and the view from above.
fn write_outputs(folder: &Path, prepared: &Prepared) -> std::io::Result<Vec<PathBuf>> {
    let directory = folder.join("survey");
    std::fs::create_dir_all(&directory)?;
    let survey = &prepared.survey;
    let grid = survey.grid;
    let peil = survey.frame.peil_z;
    let cell_area = grid.cell_xy * grid.cell_xy;
    let mut csv = String::from("z_m,above_p_m,area_m2\n");
    for (bin, count) in survey.level_histogram.iter().enumerate() {
        let z = grid.bin_center(bin as u32);
        csv.push_str(&format!(
            "{z:.3},{:.3},{:.3}\n",
            z - peil,
            *count as f64 * cell_area
        ));
    }
    let profile = directory.join("profile.csv");
    std::fs::write(&profile, csv)?;
    let mut written = vec![profile];
    if let Some(top) = &prepared.top {
        let picture = directory.join("top.png");
        ::image::RgbaImage::from_raw(top.width, top.height, top.rgba.as_ref().clone())
            .ok_or_else(|| std::io::Error::other("the view from above has no pixels"))?
            .save(&picture)
            .map_err(std::io::Error::other)?;
        written.push(picture);
    }
    Ok(written)
}

/// Read the view from above that an earlier run wrote in a project folder.
pub(crate) fn read_top(folder: &Path) -> Option<TopImage> {
    let picture = ::image::open(folder.join("survey").join("top.png"))
        .ok()?
        .to_rgba8();
    Some(TopImage {
        width: picture.width(),
        height: picture.height(),
        rgba: Arc::new(picture.into_raw()),
    })
}

/// The code of a level `number` storeys above P: "00", "01", "-01".
fn level_code(number: i64) -> String {
    if number < 0 {
        format!("-{:02}", -number)
    } else {
        format!("{number:02}")
    }
}

/// The default name of a level, in the language in use.
pub(crate) fn default_name(level: &Level) -> String {
    let number = level
        .id
        .trim_start_matches('-')
        .trim_end_matches(|character: char| !character.is_ascii_digit())
        .parse::<i64>()
        .unwrap_or(0);
    match level.kind {
        LevelKind::Ground => tr("Ground floor").to_owned(),
        LevelKind::Storey => tr_args("Floor {number}", &[("number", &number)]),
        LevelKind::Basement => tr_args("Basement {number}", &[("number", &number)]),
        LevelKind::Partial => tr_args("Mezzanine {code}", &[("code", &level.id)]),
        LevelKind::Roof => tr("Roof").to_owned(),
    }
}

/// Whether a level keeps the name it was given by default.
fn named_by_default(level: &Level) -> bool {
    level.name.is_empty() || level.name == level.id || level.name == default_name(level)
}

/// Sort the levels from the lowest up with the roof last, keep one P, and
/// give the floors their codes and kinds from P and those still named by
/// default their names. Returns where the level that was at `follow` went.
pub(crate) fn renumber(levels: &mut Vec<Level>, follow: Option<usize>) -> Option<usize> {
    let mut order: Vec<usize> = (0..levels.len()).collect();
    order.sort_by(|a, b| {
        let (a, b) = (&levels[*a], &levels[*b]);
        (a.kind == LevelKind::Roof)
            .cmp(&(b.kind == LevelKind::Roof))
            .then(a.floor_z.total_cmp(&b.floor_z))
    });
    let followed = follow.and_then(|follow| order.iter().position(|at| *at == follow));
    let mut sorted: Vec<Level> = order.iter().map(|at| levels[*at].clone()).collect();
    let peil = sorted
        .iter()
        .position(|level| level.is_peil && level.is_storey())
        .or_else(|| sorted.iter().position(Level::is_storey));
    for (place, level) in sorted.iter_mut().enumerate() {
        level.is_peil = Some(place) == peil;
    }
    let storeys: Vec<usize> = (0..sorted.len())
        .filter(|place| sorted[*place].is_storey())
        .collect();
    let rank_of_peil = storeys
        .iter()
        .position(|place| Some(*place) == peil)
        .unwrap_or(0) as i64;
    let rename = |level: &mut Level, kind: LevelKind, id: String| {
        let by_default = named_by_default(level);
        level.kind = kind;
        level.id = id;
        if by_default {
            level.name = default_name(level);
        }
    };
    for (rank, place) in storeys.iter().enumerate() {
        let number = rank as i64 - rank_of_peil;
        let kind = match number.cmp(&0) {
            std::cmp::Ordering::Less => LevelKind::Basement,
            std::cmp::Ordering::Equal => LevelKind::Ground,
            std::cmp::Ordering::Greater => LevelKind::Storey,
        };
        rename(&mut sorted[*place], kind, level_code(number));
    }
    for place in 0..sorted.len() {
        match sorted[place].kind {
            LevelKind::Partial => {
                let base = sorted[..place]
                    .iter()
                    .rev()
                    .find(|level| level.is_storey())
                    .map_or_else(|| "00".to_owned(), |level| level.id.clone());
                let taken = sorted[..place]
                    .iter()
                    .filter(|level| {
                        level.kind == LevelKind::Partial
                            && level.id.starts_with(&format!("{base}M"))
                    })
                    .count();
                let id = if taken == 0 {
                    format!("{base}M")
                } else {
                    format!("{base}M{}", taken + 1)
                };
                rename(&mut sorted[place], LevelKind::Partial, id);
            }
            LevelKind::Roof => rename(&mut sorted[place], LevelKind::Roof, "R".into()),
            _ => {}
        }
    }
    *levels = sorted;
    followed
}

/// A height while a level line is dragged: in steps of `SNAP` above P, or
/// to the millimetre with `free`.
pub(crate) fn snap(z: f64, peil: f64, free: bool) -> f64 {
    if free {
        (z * 1000.0).round() / 1000.0
    } else {
        ((z - peil) / SNAP).round() * SNAP + peil
    }
}

/// From a floor to the next whole floor or the roof above it.
pub(crate) fn storey_height(levels: &[Level], place: usize) -> Option<f64> {
    let level = levels.get(place)?;
    if !level.is_storey() {
        return None;
    }
    levels[place + 1..]
        .iter()
        .find(|above| above.is_storey() || above.kind == LevelKind::Roof)
        .map(|above| above.floor_z - level.floor_z)
}

/// The section box before Show in model put a level in it: none when the
/// box was off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PutBack {
    pub(crate) section: Option<OrientedBox>,
}

/// What step 0 holds in the window.
#[derive(Debug, Default)]
pub(crate) struct Prepare {
    /// What the survey found, and the view from above.
    pub(crate) survey: Option<SurveyRecord>,
    pub(crate) top: Option<TopImage>,
    pub(crate) top_handle: Option<image::Handle>,
    pub(crate) regions: Regions,
    /// The levels as the user sees and changes them.
    pub(crate) levels: Vec<Level>,
    pub(crate) selected: Option<usize>,
    /// The fields of the page, kept as they are typed.
    pub(crate) name: String,
    pub(crate) cut: String,
    pub(crate) rotation: String,
    pub(crate) nap_offset: String,
    pub(crate) north: String,
    pub(crate) put_back: Option<PutBack>,
}

impl Prepare {
    /// The height of P: the floor of the level that is P, or that of the
    /// survey.
    pub(crate) fn peil_z(&self) -> f64 {
        self.levels
            .iter()
            .find(|level| level.is_peil)
            .map(|level| level.floor_z)
            .or(self.survey.as_ref().map(|survey| survey.frame.peil_z))
            .unwrap_or(0.0)
    }

    /// The frame of the building with P where the levels have it.
    pub(crate) fn frame(&self) -> Option<BuildingFrame> {
        self.survey.as_ref().map(|survey| BuildingFrame {
            peil_z: self.peil_z(),
            ..survey.frame
        })
    }

    /// Take what a run found: the levels get their names in the language in
    /// use.
    pub(crate) fn take(&mut self, prepared: &Prepared) {
        let mut levels = prepared.levels.clone();
        for level in &mut levels {
            level.name = default_name(level);
        }
        self.levels = levels;
        self.survey = Some(prepared.survey.clone());
        self.top_handle = prepared.top.as_ref().map(TopImage::handle);
        self.top = prepared.top.clone();
        self.regions.core = Some(prepared.core.into());
        self.regions.building = prepared.building.map(BoxRecord::from);
        self.regions.site = Some(prepared.site.into());
        self.select(self.levels.iter().position(|level| level.is_peil));
    }

    pub(crate) fn select(&mut self, place: Option<usize>) {
        self.selected = place.filter(|place| *place < self.levels.len());
        if let Some(level) = self.selected.map(|place| &self.levels[place]) {
            self.name = level.name.clone();
            self.cut = format!("{:.2}", level.cut_height);
        } else {
            self.name.clear();
            self.cut.clear();
        }
    }

    /// The main direction the user typed, if one.
    pub(crate) fn chosen_rotation(&self) -> Option<f64> {
        parse_number(&self.rotation).filter(|degrees| degrees.abs() <= 360.0)
    }

    /// Mark the selected level as changed by hand.
    fn edited(&mut self) {
        if let Some(level) = self.selected.and_then(|place| self.levels.get_mut(place)) {
            level.status = LevelStatus::Edited;
        }
    }
}

/// A number as it is typed, with a point or a comma.
pub(crate) fn parse_number(typed: &str) -> Option<f64> {
    let typed = typed.trim().replace(',', ".");
    (!typed.is_empty())
        .then(|| typed.parse::<f64>().ok())
        .flatten()
        .filter(|value| value.is_finite())
}

/// What the page of step 0 does.
#[derive(Debug, Clone)]
pub enum PrepareAction {
    Select(usize),
    /// A level line was dragged to this scene height.
    Move(usize, f64),
    Name(String),
    Cut(String),
    Rotation(String),
    NapOffset(String),
    North(String),
    /// The selected level becomes P.
    SetPeil,
    Add,
    /// The selected level takes the one above it in.
    Merge,
    Remove,
    /// The core of the next survey becomes the section box, or the whole
    /// scan again.
    CoreFromSection,
    CoreWhole,
    /// The section box goes around the building.
    SectionToBuilding,
    /// Show a level in the model through the section box.
    Show(usize),
    ProjectName(String),
    ProjectFolder(String),
    ChooseFolder,
    FolderChosen(Option<PathBuf>),
    /// Confirmed levels can be changed again.
    Unlock,
}

fn send(action: PrepareAction) -> Message {
    Message::MeshToPlans(WizardAction::Prepare(action))
}

impl Studio {
    /// Whether the levels are confirmed and so locked.
    pub(crate) fn levels_locked(&self) -> bool {
        matches!(
            self.mesh_to_plans.status(WizardStep::Prepare),
            StepStatus::Confirmed | StepStatus::Running
        )
    }

    /// What the preparation of a job reads, or why it cannot start: every
    /// layer that is shown, as Closed mesh takes them.
    pub(crate) fn prepare_input(&self, folder: Option<PathBuf>) -> Result<PrepareInput, Sentence> {
        let shown: Vec<&crate::CloudEntry> = self
            .clouds
            .iter()
            .filter(|entry| entry.visible && entry.cloud.total_points > 0 && !entry.bag_source)
            .collect();
        if shown.is_empty() {
            return Err(Sentence::plain(key("Open a scan of the building first")));
        }
        for entry in &shown {
            let name = crate::display_name(&entry.cloud.path).to_owned();
            if entry.cloud.provisional {
                return Err(Sentence::with(
                    key("Wait until {name} has loaded"),
                    &[("name", name)],
                ));
            }
            if entry.index.is_none() && entry.cloud.total_points > UNINDEXED_LIMIT {
                return Err(Sentence::with(
                    key("Build the index of {name} first"),
                    &[("name", name)],
                ));
            }
        }
        let layers = shown
            .into_iter()
            .map(|entry| JobLayer::of(entry, Arc::clone(&entry.cloud)))
            .collect();
        let mut filter = self.mesh_filter();
        filter.section = None;
        let prepare = &self.mesh_to_plans.prepare;
        Ok(PrepareInput {
            scene: JobScene::new(None, filter, layers),
            core: prepare.regions.chosen_core.map(BoxRecord::shape),
            rotation: prepare.regions.chosen_rotation,
            folder,
        })
    }

    pub(crate) fn update_prepare(&mut self, action: PrepareAction) -> Task<Message> {
        let locked = self.levels_locked();
        let changes_levels = matches!(
            action,
            PrepareAction::Move(..)
                | PrepareAction::Name(_)
                | PrepareAction::Cut(_)
                | PrepareAction::SetPeil
                | PrepareAction::Add
                | PrepareAction::Merge
                | PrepareAction::Remove
        );
        if locked && changes_levels {
            self.status = tr("Edit levels first: they are confirmed").into();
            return Task::none();
        }
        let prepare = &mut self.mesh_to_plans.prepare;
        let mut save = changes_levels;
        match action {
            PrepareAction::Select(place) => prepare.select(Some(place)),
            PrepareAction::Move(place, z) => {
                if let Some(level) = prepare.levels.get_mut(place) {
                    let rise = z - level.floor_z;
                    level.floor_z = z;
                    level.ceiling_z = level.ceiling_z.map(|ceiling| ceiling + rise);
                    level.slab_underside = level.slab_underside.map(|underside| underside + rise);
                    level.status = LevelStatus::Edited;
                    let followed = renumber(&mut prepare.levels, Some(place));
                    slab_thicknesses(&mut prepare.levels, &LevelConfig::default());
                    prepare.select(followed);
                }
            }
            PrepareAction::Name(name) => {
                prepare.name = name.chars().take(60).collect();
                let name = prepare.name.trim().to_owned();
                if let Some(level) = prepare
                    .selected
                    .and_then(|place| prepare.levels.get_mut(place))
                {
                    if !name.is_empty() {
                        level.name = name;
                    }
                }
                prepare.edited();
            }
            PrepareAction::Cut(typed) => {
                prepare.cut = typed;
                let height =
                    parse_number(&prepare.cut).filter(|height| (0.3..=3.0).contains(height));
                if let (Some(height), Some(level)) = (
                    height,
                    prepare
                        .selected
                        .and_then(|place| prepare.levels.get_mut(place)),
                ) {
                    level.cut_height = height;
                    prepare.edited();
                } else {
                    save = false;
                }
            }
            PrepareAction::Rotation(typed) => {
                prepare.rotation = typed;
                prepare.regions.chosen_rotation = prepare.chosen_rotation();
                save = true;
            }
            PrepareAction::NapOffset(typed) => {
                prepare.nap_offset = typed;
                save = true;
            }
            PrepareAction::North(typed) => {
                prepare.north = typed;
                save = true;
            }
            PrepareAction::SetPeil => {
                if let Some(place) = prepare
                    .selected
                    .filter(|place| prepare.levels[*place].is_storey())
                {
                    for (at, level) in prepare.levels.iter_mut().enumerate() {
                        level.is_peil = at == place;
                    }
                    prepare.edited();
                    let followed = renumber(&mut prepare.levels, Some(place));
                    prepare.select(followed);
                } else {
                    self.status = tr("Only a whole floor can be P").into();
                    save = false;
                }
            }
            PrepareAction::Add => {
                let base = prepare
                    .selected
                    .and_then(|place| prepare.levels.get(place))
                    .or_else(|| prepare.levels.iter().rev().find(|level| level.is_storey()));
                let floor_z = base.map_or(prepare.peil_z(), |level| level.floor_z + 3.0);
                prepare.levels.push(Level {
                    id: String::new(),
                    name: String::new(),
                    kind: LevelKind::Storey,
                    floor_z,
                    ceiling_z: None,
                    slab_underside: None,
                    slab_thickness: None,
                    cut_height: 1.2,
                    tilt_mm_per_m: None,
                    share: 0.0,
                    is_peil: !prepare.levels.iter().any(|level| level.is_peil),
                    confidence: Confidence::certain(),
                    status: LevelStatus::Edited,
                });
                let added = prepare.levels.len() - 1;
                let followed = renumber(&mut prepare.levels, Some(added));
                slab_thicknesses(&mut prepare.levels, &LevelConfig::default());
                prepare.select(followed);
            }
            PrepareAction::Merge => {
                let Some(place) = prepare
                    .selected
                    .filter(|place| place + 1 < prepare.levels.len())
                else {
                    self.status = tr("Select a level with one above it").into();
                    return Task::none();
                };
                let upper = prepare.levels.remove(place + 1);
                let level = &mut prepare.levels[place];
                level.ceiling_z = upper.ceiling_z.or(level.ceiling_z);
                level.slab_underside = upper.slab_underside.or(level.slab_underside);
                level.slab_thickness = upper.slab_thickness.or(level.slab_thickness);
                level.is_peil |= upper.is_peil;
                level.status = LevelStatus::Edited;
                let followed = renumber(&mut prepare.levels, Some(place));
                slab_thicknesses(&mut prepare.levels, &LevelConfig::default());
                prepare.select(followed);
            }
            PrepareAction::Remove => {
                let Some(place) = prepare
                    .selected
                    .filter(|place| *place < prepare.levels.len())
                else {
                    return Task::none();
                };
                prepare.levels.remove(place);
                renumber(&mut prepare.levels, None);
                slab_thicknesses(&mut prepare.levels, &LevelConfig::default());
                let next = place.min(prepare.levels.len().saturating_sub(1));
                prepare.select((!prepare.levels.is_empty()).then_some(next));
            }
            PrepareAction::CoreFromSection => match self.section_box() {
                Some(section) => {
                    self.mesh_to_plans.prepare.regions.chosen_core = Some(section.into());
                    self.status = tr("The next survey reads the section box").into();
                    save = true;
                }
                None => {
                    self.status = tr("Switch on the section box first").into();
                }
            },
            PrepareAction::CoreWhole => {
                prepare.regions.chosen_core = None;
                save = true;
            }
            PrepareAction::SectionToBuilding => {
                let building = prepare.regions.building.map(BoxRecord::shape);
                return match building {
                    Some(building) => self.put_section_box(Some(building)),
                    None => {
                        self.status = tr("Run this step first").into();
                        Task::none()
                    }
                };
            }
            PrepareAction::Show(place) => return self.show_level_in_model(place),
            PrepareAction::ProjectName(name) => {
                let wizard = &mut self.mesh_to_plans;
                let before = super::project::folder_name(&wizard.project_name);
                wizard.project_name = name.chars().take(80).collect();
                // The folder follows the name while it is the default one.
                if wizard.project.is_none() {
                    if let Some(root) = super::project::default_root() {
                        if wizard.project_folder.is_empty()
                            || Path::new(&wizard.project_folder) == root.join(&before)
                        {
                            wizard.project_folder = root
                                .join(super::project::folder_name(&wizard.project_name))
                                .display()
                                .to_string();
                        }
                    }
                }
                save = self.mesh_to_plans.project.is_some();
            }
            PrepareAction::ProjectFolder(folder) => {
                if self.mesh_to_plans.project.is_none() {
                    self.mesh_to_plans.project_folder = folder;
                }
            }
            PrepareAction::ChooseFolder => {
                let start = PathBuf::from(&self.mesh_to_plans.project_folder);
                return Task::perform(
                    async move {
                        let mut dialog = rfd::AsyncFileDialog::new();
                        if let Some(parent) = start.parent().filter(|parent| parent.is_dir()) {
                            dialog = dialog.set_directory(parent);
                        }
                        dialog
                            .pick_folder()
                            .await
                            .map(|folder| folder.path().to_path_buf())
                    },
                    |folder| send(PrepareAction::FolderChosen(folder)),
                );
            }
            PrepareAction::FolderChosen(folder) => {
                if let Some(folder) = folder {
                    if self.mesh_to_plans.project.is_none() {
                        self.mesh_to_plans.project_folder = folder.display().to_string();
                    }
                }
            }
            PrepareAction::Unlock => {
                if *self.mesh_to_plans.status(WizardStep::Prepare) == StepStatus::Confirmed {
                    self.mesh_to_plans
                        .set_status(WizardStep::Prepare, StepStatus::Done);
                    save = true;
                }
            }
        }
        if save {
            self.queue_project_save()
        } else {
            Task::none()
        }
    }

    /// The `mesh_to_plans_level` command of the local API: what the page of
    /// step 0 does with a level. The level named by its id is selected, then
    /// given its name, its cut and the height of its floor when they are
    /// given, and then `action` is done: `select` (nothing more), `show`
    /// (Show in model), `set_peil`, `merge` (with the level above it),
    /// `remove`, or `add` (a level above the selected or highest floor, for
    /// which no level is needed). Confirmed levels are only selected and
    /// shown.
    pub(crate) fn api_mesh_to_plans_level(
        &mut self,
        level: Option<&str>,
        action: Option<&str>,
        name: Option<String>,
        cut_height: Option<f64>,
        floor_above_p: Option<f64>,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let action = action.unwrap_or("select").to_ascii_lowercase();
        if !LEVEL_ACTIONS.contains(&action.as_str()) {
            return refuse(format!("unknown action; use {}", LEVEL_ACTIONS.join(", ")));
        }
        let prepare = &self.mesh_to_plans.prepare;
        if prepare.survey.is_none() {
            return refuse("there are no levels yet: run step 0 first".into());
        }
        let edits = name.is_some()
            || cut_height.is_some()
            || floor_above_p.is_some()
            || !matches!(action.as_str(), "select" | "show");
        if edits && self.levels_locked() {
            let running = *self.mesh_to_plans.status(WizardStep::Prepare) == StepStatus::Running;
            return refuse(if running {
                "step 0 is running".into()
            } else {
                "the levels are confirmed: use Edit levels first".into()
            });
        }
        let place = match level {
            Some(id) => match prepare.levels.iter().position(|known| known.id == id) {
                Some(place) => Some(place),
                None => {
                    let ids: Vec<&str> = prepare
                        .levels
                        .iter()
                        .map(|known| known.id.as_str())
                        .collect();
                    return refuse(format!("no level {id}; the levels are {}", ids.join(", ")));
                }
            },
            None if action == "add" => None,
            None => return refuse("level is required".into()),
        };
        if name.as_deref().is_some_and(|name| name.trim().is_empty()) {
            return refuse("name must not be empty".into());
        }
        if cut_height.is_some_and(|height| !(0.3..=3.0).contains(&height)) {
            return refuse("cut_height must lie between 0.3 and 3.0 m".into());
        }
        if floor_above_p.is_some_and(|height| !height.is_finite() || height.abs() > 1000.0) {
            return refuse("floor_above_p must be a height within 1000 m of P".into());
        }
        if let Some(place) = place {
            let found = &prepare.levels[place];
            if action == "set_peil" && !found.is_storey() {
                return refuse("only a whole floor can be P".into());
            }
            if action == "merge" && place + 1 >= prepare.levels.len() {
                return refuse("the level has no level above it to merge with".into());
            }
        }
        let mut tasks = Vec::new();
        if let Some(place) = place {
            tasks.push(self.update_prepare(PrepareAction::Select(place)));
            if let Some(name) = name {
                tasks.push(self.update_prepare(PrepareAction::Name(name)));
            }
            if let Some(height) = cut_height {
                tasks.push(self.update_prepare(PrepareAction::Cut(format!("{height}"))));
            }
            if let Some(above) = floor_above_p {
                let prepare = &self.mesh_to_plans.prepare;
                let z = prepare.peil_z() + above;
                let place = prepare.selected.unwrap_or(place);
                tasks.push(self.update_prepare(PrepareAction::Move(place, z)));
            }
        }
        let selected = self.mesh_to_plans.prepare.selected;
        let done = match (action.as_str(), selected) {
            ("show", Some(place)) => Some(PrepareAction::Show(place)),
            ("set_peil", Some(_)) => Some(PrepareAction::SetPeil),
            ("merge", Some(_)) => Some(PrepareAction::Merge),
            ("remove", Some(_)) => Some(PrepareAction::Remove),
            ("add", _) => Some(PrepareAction::Add),
            _ => None,
        };
        if let Some(done) = done {
            tasks.push(self.update_prepare(done));
        }
        (
            json!({"ok": true, "mesh_to_plans": self.mesh_to_plans.value()}),
            Task::batch(tasks),
        )
    }

    /// Put the section box around a box, or switch it off.
    pub(crate) fn put_section_box(&mut self, section: Option<OrientedBox>) -> Task<Message> {
        let Some(section) = section else {
            if self.section_enabled {
                return self.update(Message::SetSectionEnabled(false));
            }
            return Task::none();
        };
        let (reply, answer) = std::sync::mpsc::channel();
        let task = self.handle_api(crate::native_api::ApiRequest {
            command: crate::native_api::ApiCommand::SetSection {
                min: section.bounds.min,
                max: section.bounds.max,
                rotation: Some(section.rotation_degrees),
            },
            reply,
        });
        let placed = answer
            .try_recv()
            .ok()
            .is_some_and(|answer| answer["ok"] == true);
        if !placed {
            self.status = tr("The box lies outside the open scans").into();
        }
        task
    }

    /// Show in model for a level: the card becomes the strip, the section
    /// box takes the storey, from just below its floor to the next floor,
    /// and the camera frames it; Back to wizard puts the box back as it was.
    fn show_level_in_model(&mut self, place: usize) -> Task<Message> {
        let prepare = &self.mesh_to_plans.prepare;
        let (Some(frame), Some(building), Some(level)) = (
            prepare.frame(),
            prepare.regions.building.map(BoxRecord::shape),
            prepare.levels.get(place),
        ) else {
            return Task::none();
        };
        let above = prepare.levels[place + 1..]
            .iter()
            .map(|level| level.floor_z)
            .next()
            .unwrap_or(level.floor_z + 3.0);
        let plan = frame
            .frame_bounds(building.corners())
            .unwrap_or(building.bounds);
        let storey = frame.oriented_box(Bounds {
            min: [plan.min[0], plan.min[1], level.floor_z - 0.1 - frame.peil_z],
            max: [
                plan.max[0],
                plan.max[1],
                above.max(level.floor_z + 0.5) - 0.05 - frame.peil_z,
            ],
        });
        if self.mesh_to_plans.prepare.put_back.is_none() {
            self.mesh_to_plans.prepare.put_back = Some(PutBack {
                section: self.section_box(),
            });
        }
        self.mesh_to_plans.prepare.select(Some(place));
        self.mesh_to_plans.minimize();
        let placed = self.put_section_box(Some(storey));
        // The camera keeps its direction and frames the storey.
        let framed = crate::combined_bounds(&self.clouds).and_then(|scene| {
            crate::camera_to_frame_bounds(
                scene,
                storey.aabb(),
                self.yaw,
                self.pitch,
                self.viewport_size,
            )
        });
        match framed {
            Some((zoom, pan)) => {
                self.zoom = zoom;
                self.pan = pan;
                self.revision += 1;
                Task::batch([placed, self.schedule_detail()])
            }
            None => placed,
        }
    }

    /// The settings and lists of step 0, in the middle column.
    pub(crate) fn prepare_settings(&self) -> Element<'_, Message> {
        let wizard = &self.mesh_to_plans;
        let prepare = &wizard.prepare;
        let colors = self.ui_theme.colors();
        let locked = self.levels_locked();
        let heading = |label: &'static str| text(tr(label)).size(12).color(colors.accent);
        let note = |content: String| text(content).size(11).color(colors.muted);
        let field =
            |label: &'static str, input: Element<'static, Message>| -> Element<'static, Message> {
                row![text(tr(label)).size(11).width(110), input]
                    .spacing(6)
                    .align_y(iced::Alignment::Center)
                    .into()
            };
        let input =
            |placeholder: &'static str, value: &str, on_input: fn(String) -> PrepareAction| {
                text_input(tr(placeholder), value)
                    .on_input(move |typed| send(on_input(typed)))
                    .size(11)
                    .padding([3, 5])
                    .width(Fill)
            };
        let plain = |label: &'static str, message: Option<Message>| {
            button(text(tr(label)).size(11))
                .on_press_maybe(message)
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .padding([3, 8])
        };

        // The project.
        let mut page = column![heading(key("Project"))].spacing(6).width(Fill);
        page = page.push(field(
            key("Name"),
            input(
                key("Name"),
                &wizard.project_name,
                PrepareAction::ProjectName,
            )
            .into(),
        ));
        let folder_input = text_input(tr("Folder"), &wizard.project_folder)
            .on_input_maybe(
                wizard
                    .project
                    .is_none()
                    .then_some(|typed| send(PrepareAction::ProjectFolder(typed))),
            )
            .size(11)
            .padding([3, 5])
            .width(Fill);
        page = page.push(field(
            key("Folder"),
            row![
                folder_input,
                plain(
                    key("Choose…"),
                    wizard
                        .project
                        .is_none()
                        .then_some(send(PrepareAction::ChooseFolder)),
                ),
            ]
            .spacing(4)
            .align_y(iced::Alignment::Center)
            .into(),
        ));
        page = page.push(note(match &wizard.project {
            Some(project) => tr_args("Saved in {file}", &[("file", &project.file.display())]),
            None => tr("The project is written there when this step runs.").to_owned(),
        }));

        // The scans.
        page = page.push(heading(key("Scans")));
        let shown: Vec<&crate::CloudEntry> = self
            .clouds
            .iter()
            .filter(|entry| entry.visible && !entry.bag_source)
            .collect();
        if shown.is_empty() {
            page = page.push(note(tr("Open a scan of the building first").into()));
        }
        let mut needs_index = false;
        for entry in &shown {
            let indexed = entry.index.is_some();
            needs_index |= !indexed && entry.cloud.total_points > UNINDEXED_LIMIT;
            page = page.push(note(tr_args(
                "{name}: {points} points, {index}",
                &[
                    ("name", &crate::display_name(&entry.cloud.path)),
                    ("points", &crate::format_count(entry.cloud.total_points)),
                    (
                        "index",
                        &if indexed {
                            tr("indexed")
                        } else if self.index_pending {
                            tr("index being built")
                        } else {
                            tr("no index")
                        },
                    ),
                ],
            )));
        }
        if needs_index && !self.index_pending {
            page = page.push(plain(key("Build index"), Some(Message::BuildIndex)));
        }
        let stations: usize = shown.iter().map(|entry| entry.cloud.scan_poses.len()).sum();
        page = page.push(note(if stations == 0 {
            tr("Stations: none. Sides are derived from free space.").to_owned()
        } else {
            tr_args("Stations: {count}", &[("count", &stations)])
        }));

        // The frame and the datum.
        page = page.push(heading(key("Frame")));
        page = page.push(field(
            key("Main direction (°)"),
            input(key("automatic"), &prepare.rotation, PrepareAction::Rotation).into(),
        ));
        if let Some(frame) = prepare.frame() {
            page = page.push(note(match frame.second_direction_deg {
                Some(second) => tr_args(
                    "Found {main}°, second direction {second}°",
                    &[
                        ("main", &format!("{:.2}", frame.rotation_deg)),
                        ("second", &format!("{second:.1}")),
                    ],
                ),
                None => tr_args(
                    "Found {main}°",
                    &[("main", &format!("{:.2}", frame.rotation_deg))],
                ),
            }));
        }
        page = page.push(note(if prepare.regions.chosen_core.is_some() {
            tr("Core: the section box").to_owned()
        } else {
            tr("Core: the scans without stray points far out").to_owned()
        }));
        page = page.push(
            row![
                plain(
                    key("From section box"),
                    Some(send(PrepareAction::CoreFromSection))
                ),
                plain(
                    key("Whole scan"),
                    prepare
                        .regions
                        .chosen_core
                        .is_some()
                        .then_some(send(PrepareAction::CoreWhole)),
                ),
            ]
            .spacing(4),
        );
        page = page.push(plain(
            key("Section box to building"),
            prepare
                .regions
                .building
                .is_some()
                .then_some(send(PrepareAction::SectionToBuilding)),
        ));
        page = page.push(field(
            key("NAP of P (m)"),
            input(
                key("unknown"),
                &prepare.nap_offset,
                PrepareAction::NapOffset,
            )
            .into(),
        ));
        page = page.push(field(
            key("North (°)"),
            input(key("unknown"), &prepare.north, PrepareAction::North).into(),
        ));
        if let Some(survey) = &prepare.survey {
            if let (Some(below), true) = (survey.below_z, survey.below_points > 0) {
                let points = crate::format_count(survey.below_points);
                let height = format!("{:+.2}", below - prepare.peil_z());
                let values = [
                    ("points", &points as &dyn std::fmt::Display),
                    ("height", &height),
                    ("clusters", &survey.below_clusters),
                ];
                page = page.push(note(match survey.below_clusters {
                    0 => tr_args("{points} stray points below {height} m left out", &values),
                    1 => tr_args(
                        "{points} stray points below {height} m left out, among them one cluster",
                        &values,
                    ),
                    _ => tr_args(
                        "{points} stray points below {height} m left out, among them {clusters} clusters",
                        &values,
                    ),
                }));
            }
        }

        // The selected level.
        if let Some(level) = prepare.selected.and_then(|place| prepare.levels.get(place)) {
            let place = prepare.selected.unwrap_or_default();
            page = page.push(heading(key("Level")));
            page = page.push(note(format!("{}  {}", level.id, level.name)));
            let editable =
                |message: fn(String) -> PrepareAction| move |typed: String| send(message(typed));
            page = page.push(field(
                key("Name"),
                text_input(tr("Name"), &prepare.name)
                    .on_input_maybe((!locked).then_some(editable(PrepareAction::Name)))
                    .size(11)
                    .padding([3, 5])
                    .width(Fill)
                    .into(),
            ));
            page = page.push(field(
                key("Cut height (m)"),
                text_input("1.20", &prepare.cut)
                    .on_input_maybe((!locked).then_some(editable(PrepareAction::Cut)))
                    .size(11)
                    .padding([3, 5])
                    .width(Fill)
                    .into(),
            ));
            let open = !locked;
            page = page.push(
                row![
                    plain(
                        key("Set as P"),
                        (open && level.is_storey() && !level.is_peil)
                            .then_some(send(PrepareAction::SetPeil)),
                    ),
                    plain(key("Show in model"), Some(send(PrepareAction::Show(place)))),
                ]
                .spacing(4),
            );
            page = page.push(
                row![
                    plain(key("Add level"), open.then_some(send(PrepareAction::Add))),
                    plain(key("Remove"), open.then_some(send(PrepareAction::Remove))),
                ]
                .spacing(4),
            );
            page = page.push(plain(
                key("Merge with above"),
                (open && place + 1 < prepare.levels.len()).then_some(send(PrepareAction::Merge)),
            ));
        } else if prepare.survey.is_some() {
            page = page.push(plain(
                key("Add level"),
                (!locked).then_some(send(PrepareAction::Add)),
            ));
        }
        if locked && *wizard.status(WizardStep::Prepare) == StepStatus::Confirmed {
            page = page.push(plain(key("Edit levels"), Some(send(PrepareAction::Unlock))));
        }
        page.into()
    }

    /// The preview of step 0: the histogram with the level lines, the view
    /// from above and the table of the levels.
    pub(crate) fn prepare_preview(&self) -> Element<'_, Message> {
        let prepare = &self.mesh_to_plans.prepare;
        let Some(survey) = &prepare.survey else {
            return container(
                text(tr(
                    "Run this step to survey the scans and find the levels of the building.",
                ))
                .size(12)
                .color(INK_MUTED),
            )
            .center(Fill)
            .into();
        };
        let peil = prepare.peil_z();
        let histogram = Canvas::new(Histogram {
            survey,
            levels: &prepare.levels,
            selected: prepare.selected,
            peil,
            locked: self.levels_locked(),
        })
        .width(300)
        .height(Fill);
        // The picture and the lines over it are two canvases in a stack: an
        // image in a canvas would be drawn over the lines of the same one.
        let top = |part: TopPart| {
            Canvas::new(TopView {
                survey,
                frame: prepare.frame().unwrap_or(survey.frame),
                top: prepare.top_handle.as_ref(),
                regions: &prepare.regions,
                part,
            })
            .width(Fill)
            .height(Fill)
        };
        let top = stack![top(TopPart::Picture), top(TopPart::Lines)]
            .width(Fill)
            .height(Fill);
        column![
            row![histogram, top].spacing(8).height(Fill),
            self.level_table(),
        ]
        .spacing(8)
        .padding(8)
        .into()
    }

    /// The table of the levels: a row per level, a click selects it.
    fn level_table(&self) -> Element<'_, Message> {
        let prepare = &self.mesh_to_plans.prepare;
        let peil = prepare.peil_z();
        let cell = |content: String, width: f32| text(content).size(11).color(INK).width(width);
        let widths = [170.0, 68.0, 68.0, 76.0, 56.0, 48.0, 124.0, 104.0, 70.0];
        let header_names = [
            key("Level"),
            key("Floor"),
            key("Ceiling"),
            key("Storey"),
            key("Slab"),
            key("Cut"),
            key("Slope (mm/m)"),
            key("Confidence"),
            key("Source"),
        ];
        let mut header = row![].spacing(4);
        for (name, width) in header_names.into_iter().zip(widths) {
            header = header.push(text(tr(name)).size(10).color(INK_MUTED).width(width));
        }
        let mut table = column![header].spacing(1);
        let height =
            |z: Option<f64>| z.map_or_else(|| "–".to_owned(), |z| format!("{:+.3}", z - peil));
        for (place, level) in prepare.levels.iter().enumerate() {
            let selected = prepare.selected == Some(place);
            let mark = if level.is_peil { " (P)" } else { "" };
            let values = [
                format!("{}  {}{mark}", level.id, level.name),
                height(Some(level.floor_z)),
                height(level.ceiling_z),
                storey_height(&prepare.levels, place)
                    .map_or_else(|| "–".to_owned(), |rise| format!("{rise:.3}")),
                level
                    .slab_thickness
                    .map_or_else(|| "–".to_owned(), |thickness| format!("{thickness:.3}")),
                format!("{:.2}", level.cut_height),
                level.tilt_mm_per_m.map_or_else(
                    || "–".to_owned(),
                    |[along_u, along_v]| format!("{along_u:.1} / {along_v:.1}"),
                ),
                format!("{:.2}", level.confidence.score),
                match level.status {
                    LevelStatus::Found => tr("found").to_owned(),
                    LevelStatus::Edited => tr("edited").to_owned(),
                },
            ];
            let mut line = row![].spacing(4);
            for (value, width) in values.into_iter().zip(widths) {
                line = line.push(cell(value, width));
            }
            table = table.push(
                button(line)
                    .on_press(send(PrepareAction::Select(place)))
                    .padding([2, 4])
                    .width(Fill)
                    .style(move |_, status| {
                        let hovered = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: (selected || hovered).then_some(
                                if selected {
                                    Color::from_rgba8(37, 99, 235, 0.16)
                                } else {
                                    Color::from_rgba8(0, 0, 0, 0.05)
                                }
                                .into(),
                            ),
                            text_color: INK,
                            border: Border::default().rounded(3),
                            ..button::Style::default()
                        }
                    }),
            );
        }
        container(scrollable(table).height(iced::Length::Shrink))
            .max_height(200)
            .width(Fill)
            .into()
    }
}

/// Ink on the paper of the preview, whatever the theme.
const INK: Color = Color::from_rgb(0.16, 0.16, 0.18);
const INK_MUTED: Color = Color::from_rgb(0.47, 0.44, 0.42);
const LEVEL: Color = Color::from_rgb(0.15, 0.39, 0.92);
const SELECTED: Color = Color::from_rgb(0.85, 0.47, 0.02);
const CEILING: Color = Color::from_rgb(0.55, 0.55, 0.6);
const GROUND: Color = Color::from_rgb(0.42, 0.55, 0.25);
const BARS: Color = Color::from_rgb(0.62, 0.66, 0.72);

/// The histogram of the horizontal area per height, with a line per level.
pub(crate) struct Histogram<'a> {
    pub(crate) survey: &'a SurveyRecord,
    pub(crate) levels: &'a [Level],
    pub(crate) selected: Option<usize>,
    pub(crate) peil: f64,
    pub(crate) locked: bool,
}

/// A level line being dragged, and whether Shift is held.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub(crate) struct HistogramState {
    pub(crate) drag: Option<(usize, f64)>,
    pub(crate) free: bool,
}

/// Where the histogram is drawn in its bounds.
#[derive(Debug, Clone, Copy)]
struct Plot {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
    low: f64,
    high: f64,
}

impl Plot {
    fn y(&self, z: f64) -> f32 {
        self.top + ((self.high - z) / (self.high - self.low)) as f32 * (self.bottom - self.top)
    }

    fn z(&self, y: f32) -> f64 {
        self.high - f64::from((y - self.top) / (self.bottom - self.top)) * (self.high - self.low)
    }
}

impl Histogram<'_> {
    fn plot(&self, size: Size) -> Plot {
        let grid = &self.survey.grid;
        let bottom = grid.origin[2];
        let top = grid.origin[2] + grid.size[2] as f64 * grid.cell_z;
        let mut low = f64::INFINITY;
        let mut high = f64::NEG_INFINITY;
        for level in self.levels {
            low = low.min(level.floor_z);
            high = high.max(level.ceiling_z.unwrap_or(level.floor_z));
        }
        if let Some(ground) = self.survey.ground_z {
            low = low.min(ground);
        }
        let (low, high) = if low.is_finite() && high - low > 0.5 {
            ((low - 0.8).max(bottom), (high + 1.2).min(top))
        } else {
            (bottom, top)
        };
        Plot {
            left: 6.0,
            right: size.width - 92.0,
            top: 8.0,
            bottom: size.height - 18.0,
            low,
            high: high.max(low + 0.5),
        }
    }

    /// Where a height lies on the screen in bounds of `size`.
    #[cfg(test)]
    pub(crate) fn screen_y(&self, size: Size, z: f64) -> f32 {
        self.plot(size).y(z)
    }

    /// The level line under a height on the screen, the nearest within
    /// `GRAB` pixels.
    fn level_at(&self, plot: &Plot, y: f32) -> Option<usize> {
        self.levels
            .iter()
            .enumerate()
            .map(|(place, level)| (place, (plot.y(level.floor_z) - y).abs()))
            .filter(|(_, distance)| *distance <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(place, _)| place)
    }
}

impl canvas::Program<Message> for Histogram<'_> {
    type State = HistogramState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (event::Status, Option<Message>) {
        let plot = self.plot(bounds.size());
        match event {
            canvas::Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.free = modifiers.shift();
                (event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(point) = cursor.position_in(bounds) else {
                    return (event::Status::Ignored, None);
                };
                let Some(place) = self.level_at(&plot, point.y) else {
                    return (event::Status::Ignored, None);
                };
                if !self.locked {
                    state.drag = Some((place, self.levels[place].floor_z));
                }
                (
                    event::Status::Captured,
                    Some(send(PrepareAction::Select(place))),
                )
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some((place, _)) = state.drag else {
                    return (event::Status::Ignored, None);
                };
                let Some(point) = cursor.position_in(bounds) else {
                    return (event::Status::Captured, None);
                };
                let y = point.y.clamp(plot.top, plot.bottom);
                state.drag = Some((place, plot.z(y)));
                (event::Status::Captured, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let Some((place, z)) = state.drag.take() else {
                    return (event::Status::Ignored, None);
                };
                let level = &self.levels[place];
                let snapped = snap(z, self.peil, state.free);
                // A click that hardly moved the line leaves it where it is.
                if (plot.y(z) - plot.y(level.floor_z)).abs() < 2.0
                    || (snapped - level.floor_z).abs() < 5e-4
                {
                    return (event::Status::Captured, None);
                }
                (
                    event::Status::Captured,
                    Some(send(PrepareAction::Move(place, snapped))),
                )
            }
            _ => (event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let plot = self.plot(bounds.size());
        let grid = &self.survey.grid;
        let cell_area = grid.cell_xy * grid.cell_xy;
        let largest = self
            .survey
            .level_histogram
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .max(1) as f64
            * cell_area;
        let width = plot.right - plot.left;
        // The area per height as bars from the left.
        for (bin, count) in self.survey.level_histogram.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            let z = grid.bin_center(bin as u32);
            if z < plot.low || z > plot.high {
                continue;
            }
            let y0 = plot.y(z + grid.cell_z * 0.5);
            let y1 = plot.y(z - grid.cell_z * 0.5);
            let length = (*count as f64 * cell_area / largest) as f32 * width;
            frame.fill_rectangle(
                Point::new(plot.left, y0),
                Size::new(length.max(1.0), (y1 - y0).max(1.0)),
                BARS,
            );
        }
        let line = |frame: &mut Frame, z: f64, color: Color, thickness: f32, dash: bool| {
            let y = plot.y(z);
            let path = canvas::Path::line(Point::new(plot.left, y), Point::new(plot.right, y));
            let mut stroke = canvas::Stroke::default()
                .with_color(color)
                .with_width(thickness);
            if dash {
                stroke.line_dash = canvas::LineDash {
                    segments: &[5.0, 4.0],
                    offset: 0,
                };
            }
            frame.stroke(&path, stroke);
        };
        let label = |frame: &mut Frame, z: f64, content: String, color: Color| {
            frame.fill_text(canvas::Text {
                content,
                position: Point::new(plot.right + 4.0, plot.y(z)),
                size: iced::Pixels(10.0),
                color,
                vertical_alignment: alignment::Vertical::Center,
                ..canvas::Text::default()
            });
        };
        if let Some(ground) = self.survey.ground_z {
            line(&mut frame, ground, GROUND, 1.0, true);
            label(&mut frame, ground, tr("ground").to_owned(), GROUND);
        }
        for (place, level) in self.levels.iter().enumerate() {
            for ceiling in [level.ceiling_z, level.slab_underside]
                .into_iter()
                .flatten()
            {
                line(&mut frame, ceiling, CEILING, 1.0, true);
            }
            let dragged = state.drag.filter(|(at, _)| *at == place);
            let z = dragged.map_or(level.floor_z, |(_, z)| snap(z, self.peil, state.free));
            let color = if self.selected == Some(place) || dragged.is_some() {
                SELECTED
            } else {
                LEVEL
            };
            line(&mut frame, z, color, 2.0, false);
            label(
                &mut frame,
                z,
                format!("{} {:+.3}", level.id, z - self.peil),
                color,
            );
        }
        frame.fill_text(canvas::Text {
            content: format!("{} {:.0} m²", tr("area up to"), largest),
            position: Point::new(plot.left, bounds.height - 4.0),
            size: iced::Pixels(10.0),
            color: INK_MUTED,
            vertical_alignment: alignment::Vertical::Bottom,
            ..canvas::Text::default()
        });
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if state.drag.is_some() {
            return mouse::Interaction::ResizingVertically;
        }
        let over = cursor
            .position_in(bounds)
            .and_then(|point| self.level_at(&self.plot(bounds.size()), point.y));
        match over {
            Some(_) if !self.locked => mouse::Interaction::ResizingVertically,
            Some(_) => mouse::Interaction::Pointer,
            None => mouse::Interaction::default(),
        }
    }
}

/// The view from above: the heights of the survey, the boxes of the site,
/// the core and the building, the footprint and the main directions.
struct TopView<'a> {
    survey: &'a SurveyRecord,
    frame: BuildingFrame,
    top: Option<&'a image::Handle>,
    regions: &'a Regions,
    part: TopPart,
}

/// What a canvas of the view from above draws: the picture of the heights,
/// or the lines and labels that go over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopPart {
    Picture,
    Lines,
}

impl TopView<'_> {
    /// A box of the scene as the four corners of its bottom on the plan.
    fn outline(&self, shape: OrientedBox) -> Vec<[f64; 2]> {
        shape.corners()[..4]
            .iter()
            .map(|corner| self.frame.to_plan([corner[0], corner[1]]))
            .collect()
    }
}

impl canvas::Program<Message> for TopView<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = Frame::new(renderer, bounds.size());
        let grid = self.survey.grid;
        let grid_min = [grid.origin[0], grid.origin[1]];
        let grid_max = [
            grid.origin[0] + grid.size[0] as f64 * grid.cell_xy,
            grid.origin[1] + grid.size[1] as f64 * grid.cell_xy,
        ];
        let boxes: Vec<(Vec<[f64; 2]>, Color, &'static str)> = [
            (
                self.regions.site,
                Color::from_rgb(0.55, 0.55, 0.55),
                key("Site"),
            ),
            (
                self.regions.core,
                Color::from_rgb(0.15, 0.39, 0.92),
                key("Core"),
            ),
            (
                self.regions.building,
                Color::from_rgb(0.85, 0.47, 0.02),
                key("Building"),
            ),
        ]
        .into_iter()
        .filter_map(|(shape, color, name)| {
            shape.map(|shape| (self.outline(shape.shape()), color, name))
        })
        .collect();
        let (mut low, mut high) = (grid_min, grid_max);
        for (corners, _, _) in &boxes {
            for corner in corners {
                low = [low[0].min(corner[0]), low[1].min(corner[1])];
                high = [high[0].max(corner[0]), high[1].max(corner[1])];
            }
        }
        let margin = 10.0;
        let room = Size::new(
            (bounds.width - 2.0 * margin).max(1.0),
            (bounds.height - 2.0 * margin - 14.0).max(1.0),
        );
        let span = [(high[0] - low[0]).max(1e-6), (high[1] - low[1]).max(1e-6)];
        let scale = (f64::from(room.width) / span[0]).min(f64::from(room.height) / span[1]);
        let middle = [(low[0] + high[0]) * 0.5, (low[1] + high[1]) * 0.5];
        let centre = Point::new(bounds.width * 0.5, margin + room.height * 0.5);
        let at = |uv: [f64; 2]| {
            Point::new(
                centre.x + ((uv[0] - middle[0]) * scale) as f32,
                centre.y - ((uv[1] - middle[1]) * scale) as f32,
            )
        };
        if self.part == TopPart::Picture {
            if let Some(top) = self.top {
                let corner = at([grid_min[0], grid_max[1]]);
                let far = at([grid_max[0], grid_min[1]]);
                frame.draw_image(
                    Rectangle::new(corner, Size::new(far.x - corner.x, far.y - corner.y)),
                    top,
                );
            }
            return vec![frame.into_geometry()];
        }
        let ring = |frame: &mut Frame, corners: &[[f64; 2]], color: Color, width: f32| {
            if corners.len() < 2 {
                return;
            }
            let path = canvas::Path::new(|builder| {
                builder.move_to(at(corners[0]));
                for corner in &corners[1..] {
                    builder.line_to(at(*corner));
                }
                builder.close();
            });
            frame.stroke(
                &path,
                canvas::Stroke::default()
                    .with_color(color)
                    .with_width(width),
            );
        };
        // Boxes that coincide, as the core and the site do when the whole
        // scan is read, get their labels one under the other.
        let mut labels: Vec<Point> = Vec::new();
        for (corners, color, name) in &boxes {
            ring(&mut frame, corners, *color, 1.5);
            let top_left = corners
                .iter()
                .map(|corner| at(*corner))
                .min_by(|a, b| (a.x + a.y).total_cmp(&(b.x + b.y)))
                .unwrap_or(Point::ORIGIN);
            let mut position = Point::new(top_left.x + 3.0, top_left.y + 2.0);
            while labels.iter().any(|label| {
                (label.x - position.x).abs() < 60.0 && (label.y - position.y).abs() < 12.0
            }) {
                position.y += 12.0;
            }
            labels.push(position);
            frame.fill_text(canvas::Text {
                content: tr(name).to_owned(),
                position,
                size: iced::Pixels(10.0),
                color: *color,
                ..canvas::Text::default()
            });
        }
        let brick = Color::from_rgb(0.62, 0.12, 0.10);
        for region in &self.survey.footprint {
            ring(&mut frame, &region.outer, brick, 2.0);
            for hole in &region.holes {
                ring(&mut frame, hole, brick, 1.0);
            }
        }
        // The main directions, from the middle of the footprint.
        let mut middle_of = [0.0, 0.0];
        let mut count = 0.0;
        for corner in self
            .survey
            .footprint
            .iter()
            .flat_map(|region| &region.outer)
        {
            middle_of = [middle_of[0] + corner[0], middle_of[1] + corner[1]];
            count += 1.0;
        }
        if count > 0.0 {
            let from = [middle_of[0] / count, middle_of[1] / count];
            let reach = span[0].min(span[1]) * 0.18;
            let mut arrow = |degrees: f64, content: String| {
                let turn = (degrees - self.frame.rotation_deg).to_radians();
                let to = [from[0] + reach * turn.cos(), from[1] + reach * turn.sin()];
                let line = canvas::Path::line(at(from), at(to));
                frame.stroke(
                    &line,
                    canvas::Stroke::default()
                        .with_color(Color::WHITE)
                        .with_width(4.0),
                );
                frame.stroke(
                    &line,
                    canvas::Stroke::default().with_color(INK).with_width(2.0),
                );
                frame.fill_text(canvas::Text {
                    content,
                    position: at(to),
                    size: iced::Pixels(11.0),
                    color: INK,
                    ..canvas::Text::default()
                });
            };
            arrow(
                self.frame.rotation_deg,
                format!("{:.2}°", self.frame.rotation_deg),
            );
            if let Some(second) = self.frame.second_direction_deg {
                arrow(second, format!("{second:.1}°"));
            }
        }
        // A scale bar of ten metres.
        let bar = (10.0 * scale) as f32;
        let base = Point::new(margin, bounds.height - 8.0);
        frame.stroke(
            &canvas::Path::line(base, Point::new(base.x + bar, base.y)),
            canvas::Stroke::default().with_color(INK).with_width(2.0),
        );
        frame.fill_text(canvas::Text {
            content: format!(
                "10 m   {}",
                tr_args(
                    "footprint {area} m²",
                    &[("area", &format!("{:.1}", self.survey.footprint_area))]
                )
            ),
            position: Point::new(base.x + bar + 6.0, base.y),
            size: iced::Pixels(10.0),
            color: INK,
            vertical_alignment: alignment::Vertical::Center,
            ..canvas::Text::default()
        });
        vec![frame.into_geometry()]
    }
}
