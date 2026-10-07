//! Colour from photos: give the points of a scan the colours its photos see
//! them with. The job reads the points of the active scan in the section box,
//! or all of them, and colours every point from the photos of the file that
//! see it unhidden, nearest first; see `pointcloud_core::photo_colour`. The
//! colours are kept with the layer in place of those of its file: they are
//! drawn and written by an export, and Undo takes them back.

use std::collections::HashSet;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{button, column, container, row, text};
use iced::{Element, Fill, Task};
use pointcloud_core::photo_colour::{
    colour_from_photos, ColourComparison, ColourProgress, ColourStage, ColourTimes,
    PhotoColourConfig, PhotoColouring, PhotoPixels, PointColours, DEFAULT_MAX_DISTANCE,
    MAX_MAX_DISTANCE, MIN_MAX_DISTANCE,
};
use pointcloud_core::region_source::{resident_points, RegionSource, SourceTransform, EVERYWHERE};
use pointcloud_core::{Bounds, FilePhoto, LoadError, OrientedBox, Point, ScanImage};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::bag_panel::plain_reason;
use crate::closed_mesh::{number, uncapitalised, Sentence, UNINDEXED_LIMIT};
use crate::cloud_transform::CloudTransform;
use crate::file_photos::{decode_pixels, MAX_PHOTO_EDGE};
use crate::i18n::{key, tr, tr_args};
use crate::job_scene::JobLayer;
use crate::open_progress::{Line, Phase};
use crate::selection::ClassFilter;
use crate::ui_style;
use crate::{
    compact_count, display_name, format_count, opencad_properties, CloudEntry, ColorMode,
    EditBatch, Message, Studio,
};

const BUSY: &str = key("Points are already being coloured from photos");

/// Most memory the photo colours that only Undo keeps may take: about three
/// colourings of a scan of a hundred million points. Beyond it the oldest
/// edits are let go; the last one always stays.
const UNDO_COLOUR_BUDGET: usize = 1 << 30;

/// Reads the stored bytes of a photo of the layer: from its file, or what
/// a test hands over.
type PhotoRead = Arc<dyn Fn(&FilePhoto) -> Result<Vec<u8>, String> + Send + Sync>;

/// The settings of the Properties block. The distance is kept as it was
/// typed, so that it is not rewritten while it is being typed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ColourSettings {
    /// Largest distance from a photo to a point it colours, in metres.
    max_distance: String,
    blend: bool,
}

impl Default for ColourSettings {
    fn default() -> Self {
        Self {
            max_distance: format!("{DEFAULT_MAX_DISTANCE}"),
            blend: PhotoColourConfig::default().blend,
        }
    }
}

impl ColourSettings {
    fn config(&self) -> Result<PhotoColourConfig, Sentence> {
        let max_distance = number(&self.max_distance)
            .filter(|value| (MIN_MAX_DISTANCE..=MAX_MAX_DISTANCE).contains(value))
            .ok_or_else(|| {
                Sentence::with(
                    key("The largest distance must be a number of metres from {min} to {max}"),
                    &[
                        ("min", MIN_MAX_DISTANCE.to_string()),
                        ("max", MAX_MAX_DISTANCE.to_string()),
                    ],
                )
            })?;
        Ok(PhotoColourConfig {
            max_distance,
            blend: self.blend,
            ..PhotoColourConfig::default()
        })
    }

    /// These settings with the fields a command names, or why one of them
    /// is refused.
    fn with(&self, options: &ColourOptions) -> Result<Self, String> {
        let mut settings = self.clone();
        if let Some(distance) = options.max_distance {
            settings.max_distance = distance.to_string();
        }
        if let Some(blend) = options.blend {
            settings.blend = blend;
        }
        settings
            .config()
            .map_err(|problem| uncapitalised(&problem.english()))?;
        Ok(settings)
    }

    fn value(&self) -> Value {
        json!({
            "max_distance": number(&self.max_distance),
            "blend": self.blend,
        })
    }
}

/// What a command of the local API may name; a setting that is left out
/// keeps what the Properties block has.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ColourOptions {
    /// The layer to colour; without it the active layer when it has photos,
    /// else the first layer with photos.
    pub layer: Option<usize>,
    /// Largest distance from a photo to a point it colours, in metres.
    pub max_distance: Option<f64>,
    /// Blend every photo that sees a point, weighted to the best, instead of
    /// taking the best one alone.
    pub blend: Option<bool>,
}

/// What a job is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Stage {
    /// Reading a scan without an index into memory.
    Loading,
    Reading,
    Photos,
}

impl Stage {
    const ALL: [Self; 3] = [Self::Loading, Self::Reading, Self::Photos];

    fn name(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::Reading => "reading",
            Self::Photos => "photos",
        }
    }
}

/// How far a job is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Step {
    stage: Stage,
    part: u64,
    parts: u64,
    done: u64,
    total: u64,
}

impl Step {
    fn fraction(self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f64 / self.total as f64).min(1.0) as f32)
    }

    /// The stage in words, with how far it is.
    fn text(self) -> String {
        let part = if self.parts > 1 {
            format!("part {} of {}, ", self.part, self.parts)
        } else {
            String::new()
        };
        match self.stage {
            Stage::Loading => format!(
                "reading a scan without an index, {} of {} points",
                compact_count(self.done.min(self.total)),
                compact_count(self.total)
            ),
            Stage::Reading => format!(
                "{part}reading the points, {} of {}",
                compact_count(self.done.min(self.total)),
                compact_count(self.total)
            ),
            Stage::Photos => format!(
                "{part}photo {} of {}",
                self.done.min(self.total),
                self.total
            ),
        }
    }
}

/// What the worker of a job tells the window, and the window the worker.
#[derive(Default)]
struct Control {
    cancelled: AtomicBool,
    stage: AtomicU8,
    part: AtomicU64,
    parts: AtomicU64,
    done: AtomicU64,
    total: AtomicU64,
}

impl Control {
    fn report(&self, stage: Stage, done: u64, total: u64) -> Result<(), LoadError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        self.stage.store(stage as u8, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        Ok(())
    }

    fn progress(&self, progress: ColourProgress) -> Result<(), LoadError> {
        self.part.store(progress.part as u64, Ordering::Relaxed);
        self.parts.store(progress.parts as u64, Ordering::Relaxed);
        let stage = match progress.stage {
            ColourStage::Reading => Stage::Reading,
            ColourStage::Photos => Stage::Photos,
        };
        self.report(stage, progress.done, progress.total)
    }

    fn snapshot(&self) -> Step {
        let stage = self.stage.load(Ordering::Relaxed);
        Step {
            stage: Stage::ALL
                .into_iter()
                .find(|known| *known as u8 == stage)
                .unwrap_or(Stage::Reading),
            part: self.part.load(Ordering::Relaxed),
            parts: self.parts.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }
}

/// Everything a job was started with.
pub(crate) struct JobInput {
    layer: JobLayer,
    /// The box the job colours in, in the scene; none for the whole scan.
    section: Option<OrientedBox>,
    /// The classes shown; its own box is none.
    filter: ClassFilter,
    photos: Vec<FilePhoto>,
    /// The photo colours the layer had when the job started; the new ones
    /// are laid over them.
    previous: Option<Arc<PointColours>>,
    config: PhotoColourConfig,
    read: PhotoRead,
}

/// What a finished job hands back.
#[derive(Debug, Clone)]
pub struct Finished {
    colours: Arc<PointColours>,
    result: PhotoColouring,
    seconds: f64,
}

/// The box in source coordinates around a box of the scene, for a layer
/// that stands there with `transform`; everything when the layer has a
/// scale of zero and cannot be taken back.
fn source_region(world: Bounds, transform: CloudTransform) -> Bounds {
    match (
        transform.source_xyz(world.min),
        transform.source_xyz(world.max),
    ) {
        (Some(a), Some(b)) => Bounds {
            min: std::array::from_fn(|axis| a[axis].min(b[axis])),
            max: std::array::from_fn(|axis| a[axis].max(b[axis])),
        },
        _ => EVERYWHERE,
    }
}

/// Read and decode a photo for the job, at most `MAX_PHOTO_EDGE` pixels
/// each way, as the viewer shows it.
fn decode(read: &PhotoRead, photo: &FilePhoto) -> Result<PhotoPixels, LoadError> {
    let bytes = read(photo).map_err(LoadError::InvalidData)?;
    let image =
        decode_pixels(&bytes, photo.format, MAX_PHOTO_EDGE).map_err(LoadError::InvalidData)?;
    Ok(PhotoPixels {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

/// Read the layer and colour it. This runs on a worker thread.
fn run(input: &JobInput, control: &Control) -> Result<Finished, LoadError> {
    let started = Instant::now();
    let layer = &input.layer;
    if layer.index.is_some() {
        layer.cloud.validate_source()?;
    }
    let resident = if layer.resident() {
        Some(resident_points(&layer.cloud, &mut |progress| {
            control.report(Stage::Loading, progress.read, progress.total)
        })?)
    } else {
        None
    };
    // The photos stand in source coordinates; the points are taken there
    // too, and the box of the scene is brought back to them.
    let source = match &resident {
        Some(points) => RegionSource::resident(points, SourceTransform::default()),
        None => RegionSource::new(
            &layer.cloud,
            layer.index.as_deref(),
            SourceTransform::default(),
        ),
    };
    let transform = layer.transform;
    let region = input.section.map_or(EVERYWHERE, |section| {
        source_region(section.aabb(), transform)
    });
    let accept = |_: usize, ordinal: u64, point: &Point| {
        layer
            .deleted
            .as_ref()
            .is_none_or(|mask| !mask.contains(ordinal))
            && input.filter.accepts(point)
            && input
                .section
                .is_none_or(|section| section.contains(transform.xyz(point.xyz)))
    };
    let mut colours = input
        .previous
        .as_deref()
        .cloned()
        .unwrap_or_else(|| PointColours::new(layer.cloud.total_points));
    let result = colour_from_photos(
        source,
        region,
        &accept,
        &input.photos,
        &|index| decode(&input.read, &input.photos[index]),
        &input.config,
        &mut |progress| control.progress(progress),
        &mut |part| {
            for (ordinal, rgb) in part {
                colours.set(*ordinal, *rgb);
            }
            Ok(())
        },
        &|| control.cancelled.load(Ordering::Relaxed),
    )?;
    Ok(Finished {
        colours: Arc::new(colours),
        result,
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// How a job ended, as the worker tells the window.
#[derive(Debug, Clone)]
pub enum ColourEnd {
    Done(Arc<Finished>),
    Cancelled,
    Failed(String),
}

impl ColourEnd {
    fn of(result: Result<Finished, LoadError>) -> Self {
        match result {
            Ok(finished) => Self::Done(Arc::new(finished)),
            Err(LoadError::Cancelled) => Self::Cancelled,
            Err(error) => Self::Failed(plain_reason(&error.to_string()).to_owned()),
        }
    }
}

/// A colouring that is under way.
pub(crate) struct ColourJob {
    /// Tells this job from an earlier one whose answer is still on its way.
    serial: u64,
    input: Arc<JobInput>,
    control: Arc<Control>,
    started: Instant,
    api_job_id: Option<String>,
    /// The line last written to the status bar.
    reported: String,
}

impl ColourJob {
    fn cancelling(&self) -> bool {
        self.control.cancelled.load(Ordering::Relaxed)
    }

    fn status_text(&self) -> String {
        if self.cancelling() {
            return "Cancelling the colouring from photos…".into();
        }
        format!(
            "Colouring {} from photos: {}…",
            self.input.layer.name,
            self.control.snapshot().text()
        )
    }

    /// The job as `status` and `job` of the local API report it.
    fn progress_value(&self) -> Value {
        let step = self.control.snapshot();
        json!({
            "state": "running",
            "operation": "colour_from_photos",
            "source": self.input.layer.name,
            "stage": step.stage.name(),
            "part": step.part,
            "parts": step.parts,
            "completed": step.done,
            "total": step.total,
            "fraction": step.fraction(),
            "cancel_requested": self.cancelling(),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }
}

/// A line of the status bar, with what Undo let go of to save memory.
fn with_undo_note(line: String, dropped: usize) -> String {
    match dropped {
        0 => line,
        1 => format!("{line}. To save memory, Undo let go of the oldest edit"),
        _ => format!("{line}. To save memory, Undo let go of the {dropped} oldest edits"),
    }
}

/// A number with one decimal, for the figures of a comparison.
fn tenths(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// What a finished job did, for the Properties block and the local API.
#[derive(Debug, Clone, PartialEq)]
struct Summary {
    points: u64,
    seen: u64,
    photos: usize,
    photos_used: usize,
    photos_failed: usize,
    parts: usize,
    compared: Option<ColourComparison>,
    times: ColourTimes,
    seconds: f64,
    /// Whether the section box chose the points.
    boxed: bool,
    max_distance: f64,
    blend: bool,
}

impl Summary {
    fn of(finished: &Finished, input: &JobInput) -> Self {
        let result = &finished.result;
        Self {
            points: result.points,
            seen: result.seen,
            photos: result.photos,
            photos_used: result.photos_used,
            photos_failed: result.photos_failed,
            parts: result.parts,
            compared: result.compared,
            times: result.times,
            seconds: finished.seconds,
            boxed: input.section.is_some(),
            max_distance: input.config.max_distance,
            blend: input.config.blend,
        }
    }

    fn unseen_share(&self) -> f64 {
        if self.points == 0 {
            0.0
        } else {
            (self.points - self.seen) as f64 / self.points as f64
        }
    }

    fn value(&self) -> Value {
        json!({
            "region": if self.boxed { "section_box" } else { "layer" },
            "points": self.points,
            "coloured": self.seen,
            "unseen": self.points - self.seen,
            "unseen_share": (self.unseen_share() * 10_000.0).round() / 10_000.0,
            "photos": self.photos,
            "photos_used": self.photos_used,
            "photos_failed": self.photos_failed,
            "parts": self.parts,
            "max_distance": self.max_distance,
            "blend": self.blend,
            "seconds": tenths(self.seconds),
            "times": {
                "reading": tenths(self.times.reading),
                "photos": tenths(self.times.photos),
                "decoding": tenths(self.times.decoding),
                "depth": tenths(self.times.depth),
                "colouring": tenths(self.times.colouring),
            },
            "compared": self.compared.map(|compared| json!({
                "points": compared.points,
                "mean_difference": compared.mean_difference.map(tenths),
                "mean_abs_difference": compared.mean_abs_difference.map(tenths),
                "median_abs_difference": compared.median_abs_difference,
            })),
        })
    }

    /// The figures in a line of the status bar.
    fn line(&self) -> String {
        format!(
            "{} of {} points from {} photos in {:.1} s; {:.1}% seen by no photo",
            format_count(self.seen),
            format_count(self.points),
            self.photos_used,
            self.seconds,
            self.unseen_share() * 100.0
        )
    }
}

/// How the last job ended.
#[derive(Debug, Clone, PartialEq)]
enum Last {
    Done {
        summary: Summary,
        /// The file name of the scan, or nothing when it was closed while
        /// the job ran.
        kept_with: Option<String>,
    },
    Cancelled,
    Failed(String),
}

impl Last {
    fn value(&self) -> Value {
        match self {
            Self::Done { summary, kept_with } => {
                let mut value = summary.value();
                value["state"] = "complete".into();
                value["operation"] = "colour_from_photos".into();
                value["source"] = json!(kept_with);
                value["kept"] = (kept_with.is_some() && summary.seen > 0).into();
                value
            }
            Self::Cancelled => json!({"state": "cancelled", "operation": "colour_from_photos"}),
            Self::Failed(error) => json!({
                "state": "failed",
                "operation": "colour_from_photos",
                "error": error,
            }),
        }
    }

    fn status(&self) -> String {
        match self {
            Self::Done { summary, kept_with } if summary.seen == 0 => format!(
                "No photo sees the {} points of {}: check the section box and the largest distance",
                format_count(summary.points),
                kept_with.as_deref().unwrap_or("the scan")
            ),
            Self::Done {
                summary,
                kept_with: Some(name),
            } => format!(
                "Coloured {name} from photos: {}. Undo takes the colours back",
                summary.line()
            ),
            Self::Done {
                summary,
                kept_with: None,
            } => format!(
                "Coloured from photos, but the scan was closed and nothing is kept: {}",
                summary.line()
            ),
            Self::Cancelled => {
                "Colouring from photos cancelled; the colours stay as they were".into()
            }
            Self::Failed(error) => format!("Colouring from photos failed: {error}"),
        }
    }
}

/// What the Colour from photos block holds: its settings, a job under way
/// and how the last job ended. The colours themselves are kept with their
/// layer.
pub(crate) struct PhotoColourTool {
    settings: ColourSettings,
    job: Option<ColourJob>,
    next_serial: u64,
    last: Option<Last>,
    /// Most memory the photo colours that only Undo keeps may take.
    undo_budget: usize,
}

impl Default for PhotoColourTool {
    fn default() -> Self {
        Self {
            settings: ColourSettings::default(),
            job: None,
            next_serial: 0,
            last: None,
            undo_budget: UNDO_COLOUR_BUDGET,
        }
    }
}

impl PhotoColourTool {
    pub(crate) fn is_running(&self) -> bool {
        self.job.is_some()
    }

    fn cancel(&self) -> bool {
        match &self.job {
            Some(job) => {
                job.control.cancelled.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// The line of the strip above the scene while a job runs.
    pub(crate) fn progress_line(&self) -> Option<Line> {
        let job = self.job.as_ref()?;
        let step = job.control.snapshot();
        let cancelling = job.cancelling();
        Some(Line {
            phase: Phase::PhotoColours,
            title: if cancelling {
                "Cancelling…".to_owned()
            } else {
                format!("Colour {} from photos", job.input.layer.name)
            },
            detail: step.text(),
            fraction: step.fraction(),
            timed: step.stage == Stage::Photos,
            cancel: (!cancelling).then_some(Message::PhotoColours(PhotoColourAction::Cancel)),
        })
    }
}

/// Everything the Colour from photos block reacts to.
#[derive(Debug, Clone)]
pub enum PhotoColourAction {
    MaxDistance(String),
    Blend(bool),
    Start,
    Poll,
    Cancel,
    Finished(u64, ColourEnd),
    /// Take the photo colours of the active scan away, with Undo.
    Clear,
}

/// Why no job can be started, in the words of the status bar and of the
/// local API.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Refusal {
    Busy,
    /// No layer has photos, or the one asked for does not exist.
    NoLayer,
    /// The layer asked for has no photos; the name of its file.
    NoPhotos(String),
    /// The photos of the file are still being listed.
    Listing(String),
    Loading(String),
    NeedsIndex(String),
    /// The section box holds nothing of the layer.
    Outside(String),
    Setting(Sentence),
}

impl Refusal {
    fn sentence(&self) -> Sentence {
        let named = |text, name: &String| Sentence::with(text, &[("name", name.clone())]);
        match self {
            Self::Busy => Sentence::plain(BUSY),
            Self::NoLayer => Sentence::plain(key("Select a scan with photos first")),
            Self::NoPhotos(name) => named(key("{name} has no photos to colour its points from"), name),
            Self::Listing(name) => named(key("The photos of {name} are still being listed"), name),
            Self::Loading(name) => named(
                key("{name} is still loading; wait for it before colouring its points"),
                name,
            ),
            Self::NeedsIndex(name) => Sentence::with(
                key("{name} has more than {limit} points and no index: build the index first (INDEX > Build index)"),
                &[
                    ("name", name.clone()),
                    ("limit", format_count(UNINDEXED_LIMIT)),
                ],
            ),
            Self::Outside(name) => named(key("The section box holds no part of {name}"), name),
            Self::Setting(sentence) => sentence.clone(),
        }
    }

    fn status(&self) -> String {
        self.sentence().english()
    }

    fn api(&self) -> String {
        match self {
            Self::Busy => uncapitalised(BUSY),
            Self::NoLayer => "no layer with photos".into(),
            Self::NoPhotos(name) => format!("the layer has no photos: {name}"),
            Self::Listing(name) => {
                format!("the photos of a point cloud are still being listed: {name}")
            }
            Self::Loading(name) => format!("a point cloud is still loading: {name}"),
            Self::NeedsIndex(name) => format!(
                "a point cloud of more than {UNINDEXED_LIMIT} points has no index, build it \
                 first: {name}"
            ),
            Self::Outside(name) => {
                format!("the section box holds no part of the point cloud to colour: {name}")
            }
            Self::Setting(sentence) => uncapitalised(&sentence.english()),
        }
    }
}

/// The photos of a layer that can colour its points: those of its file,
/// then those of its stations.
fn photos_of(files: Option<&[FilePhoto]>, entry: &CloudEntry) -> Vec<FilePhoto> {
    files
        .unwrap_or_default()
        .iter()
        .cloned()
        .chain(entry.cloud.scan_images.iter().map(ScanImage::as_file_photo))
        .collect()
}

impl Studio {
    /// The photos of the layer at a place in the project list.
    fn colour_photos(&self, index: usize) -> Vec<FilePhoto> {
        let Some(entry) = self.clouds.get(index) else {
            return Vec::new();
        };
        let files = self
            .photos
            .files
            .get(&entry.cloud.path)
            .map(|photos| photos.photos.as_slice());
        photos_of(files, entry)
    }

    /// The layer a command means: the one it names, or the active layer
    /// when that has photos, else the first layer with photos.
    fn colour_layer(&self, asked: Option<usize>) -> Result<usize, Refusal> {
        if let Some(index) = asked {
            return (index < self.clouds.len())
                .then_some(index)
                .ok_or(Refusal::NoLayer);
        }
        let has_photos = |index: &usize| !self.colour_photos(*index).is_empty();
        self.active
            .filter(|index| *index < self.clouds.len())
            .filter(has_photos)
            .or_else(|| (0..self.clouds.len()).find(has_photos))
            .ok_or(Refusal::NoLayer)
    }

    /// What a job on a layer would be made from, or why there is none.
    fn colour_input(&self, index: usize, config: PhotoColourConfig) -> Result<JobInput, Refusal> {
        let entry = self.clouds.get(index).ok_or(Refusal::NoLayer)?;
        let name = display_name(&entry.cloud.path).to_owned();
        if self.photos.is_listing(&entry.cloud.path) {
            return Err(Refusal::Listing(name));
        }
        let photos = self.colour_photos(index);
        if photos.is_empty() {
            return Err(Refusal::NoPhotos(name));
        }
        if entry.cloud.provisional {
            return Err(Refusal::Loading(name));
        }
        let layer = JobLayer::of(entry, Arc::clone(&entry.cloud));
        if layer.streamed() {
            return Err(Refusal::NeedsIndex(name));
        }
        let section = self.section_box();
        if let Some(section) = section {
            let around = section.aabb();
            let bounds = entry.bounds();
            if !(0..3).all(|axis| {
                bounds.min[axis] <= around.max[axis] && bounds.max[axis] >= around.min[axis]
            }) {
                return Err(Refusal::Outside(name));
            }
        }
        let source = entry.cloud.path.clone();
        Ok(JobInput {
            layer,
            section,
            filter: ClassFilter {
                section: None,
                ..self.mesh_filter()
            },
            photos,
            previous: entry.colours.as_ref().map(Arc::clone),
            config,
            read: Arc::new(move |photo: &FilePhoto| {
                pointcloud_core::read_file_photo(&source, photo).map_err(|error| error.to_string())
            }),
        })
    }

    /// The job the block would start on the active layer, or why it cannot.
    fn colour_request(&self, asked: Option<usize>) -> Result<JobInput, Refusal> {
        if self.photo_colours.is_running() {
            return Err(Refusal::Busy);
        }
        let config = self
            .photo_colours
            .settings
            .config()
            .map_err(Refusal::Setting)?;
        let index = self.colour_layer(asked)?;
        self.colour_input(index, config)
    }

    fn colour_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |()| Message::PhotoColours(PhotoColourAction::Poll),
        )
    }

    /// Start a job on a worker thread; the window reads its progress four
    /// times a second until `PhotoColourAction::Finished` arrives.
    pub(crate) fn start_colour_job(
        &mut self,
        input: JobInput,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        let control = Arc::new(Control::default());
        let input = Arc::new(input);
        let serial = self.photo_colours.next_serial;
        self.photo_colours.next_serial += 1;
        let first = if input.layer.resident() {
            Stage::Loading
        } else {
            Stage::Reading
        };
        let _ = control.report(first, 0, 0);
        let mut job = ColourJob {
            serial,
            input: Arc::clone(&input),
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id,
            reported: String::new(),
        };
        job.reported = job.status_text();
        self.status.clone_from(&job.reported);
        self.photo_colours.job = Some(job);
        self.photo_colours.last = None;
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || ColourEnd::of(run(&input, &control)))
                    .await
                    .unwrap_or_else(|error| ColourEnd::Failed(error.to_string()))
            },
            move |end| Message::PhotoColours(PhotoColourAction::Finished(serial, end)),
        );
        Task::batch([worker, Self::colour_poll_task()])
    }

    pub(crate) fn update_photo_colours(&mut self, action: PhotoColourAction) -> Task<Message> {
        match action {
            PhotoColourAction::MaxDistance(value) => {
                self.photo_colours.settings.max_distance = value;
            }
            PhotoColourAction::Blend(blend) => self.photo_colours.settings.blend = blend,
            PhotoColourAction::Start => match self.colour_request(self.active) {
                Ok(input) => return self.start_colour_job(input, None),
                Err(refusal) => self.status = refusal.status(),
            },
            PhotoColourAction::Poll => {
                let Some(job) = &mut self.photo_colours.job else {
                    return Task::none();
                };
                let text = job.status_text();
                if text != job.reported {
                    self.status.clone_from(&text);
                    job.reported = text;
                }
                if let Some(entry) = job
                    .api_job_id
                    .as_ref()
                    .and_then(|id| self.api_jobs.get_mut(id))
                {
                    *entry = job.progress_value();
                }
                return Self::colour_poll_task();
            }
            PhotoColourAction::Cancel => self.cancel_photo_colours(),
            PhotoColourAction::Finished(serial, end) => {
                let Some(job) = self.photo_colours.job.take_if(|job| job.serial == serial) else {
                    return Task::none();
                };
                self.photo_colours_finished(job, end);
            }
            PhotoColourAction::Clear => {
                self.status = match self
                    .active
                    .and_then(|index| self.clear_photo_colours(index))
                {
                    Some((name, dropped)) => with_undo_note(
                        format!("Photo colours of {name} removed; Undo brings them back"),
                        dropped,
                    ),
                    None => "The active scan has no photo colours to remove".into(),
                };
            }
        }
        Task::none()
    }

    /// Ask a running job to stop; the colours stay as they were.
    pub(crate) fn cancel_photo_colours(&mut self) {
        if self.photo_colours.cancel() {
            if let Some(job) = &mut self.photo_colours.job {
                job.reported = job.status_text();
                self.status.clone_from(&job.reported);
            }
        }
    }

    /// Give a layer other photo colours, or none, as one edit that Undo
    /// takes back; how many of the oldest edits Undo let go of to keep its
    /// photo colours within the budget.
    fn set_photo_colours(&mut self, index: usize, colours: Option<Arc<PointColours>>) -> usize {
        let Some(entry) = self.clouds.get_mut(index) else {
            return 0;
        };
        let before = std::mem::replace(&mut entry.colours, colours);
        let identity = Arc::clone(&entry.load_identity);
        self.push_edit(EditBatch {
            members: Vec::new(),
            colours: vec![(identity, before)],
        });
        self.trim_undo_colours(self.photo_colours.undo_budget)
    }

    /// Let go of the oldest edits while the photo colours that only Undo
    /// keeps take more than `budget` bytes: the blocks of a colour table
    /// that the layers or a newer edit share are counted once. The newest
    /// edit always stays. How many edits were let go.
    fn trim_undo_colours(&mut self, budget: usize) -> usize {
        let mut held = HashSet::new();
        for colours in self
            .clouds
            .iter()
            .filter_map(|entry| entry.colours.as_deref())
        {
            colours.bytes_beside(&mut held);
        }
        let mut kept = 0usize;
        let newest = self.undo_deletions.len().saturating_sub(1);
        for (place, batch) in self.undo_deletions.iter().enumerate().rev() {
            kept += batch
                .colours
                .iter()
                .filter_map(|(_, colours)| colours.as_deref())
                .map(|colours| colours.bytes_beside(&mut held))
                .sum::<usize>();
            if kept > budget && place < newest {
                self.undo_deletions.drain(..=place);
                return place + 1;
            }
        }
        0
    }

    /// Take the photo colours of a layer away; the name of its file when it
    /// had any, and how many of the oldest edits Undo let go of.
    fn clear_photo_colours(&mut self, index: usize) -> Option<(String, usize)> {
        let entry = self.clouds.get(index)?;
        entry.colours.as_ref()?;
        let name = display_name(&entry.cloud.path).to_owned();
        let dropped = self.set_photo_colours(index, None);
        Some((name, dropped))
    }

    /// A job ended: give the colours to their layer, keep what the job
    /// reports and tell the job of the local API.
    fn photo_colours_finished(&mut self, job: ColourJob, end: ColourEnd) {
        let mut dropped = 0;
        let last = match end {
            ColourEnd::Done(finished) => {
                let summary = Summary::of(&finished, &job.input);
                let place = self
                    .clouds
                    .iter()
                    .position(|entry| Arc::ptr_eq(&entry.load_identity, &job.input.layer.identity));
                let kept_with = place.map(|place| {
                    if summary.seen > 0 {
                        dropped =
                            self.set_photo_colours(place, Some(Arc::clone(&finished.colours)));
                        // The colours are what the job was for.
                        self.color_mode = ColorMode::Rgb;
                    }
                    display_name(&self.clouds[place].cloud.path).to_owned()
                });
                Last::Done { summary, kept_with }
            }
            ColourEnd::Cancelled => Last::Cancelled,
            ColourEnd::Failed(error) => Last::Failed(error),
        };
        if let Some(entry) = job
            .api_job_id
            .as_ref()
            .and_then(|id| self.api_jobs.get_mut(id))
        {
            *entry = last.value();
        }
        self.status = with_undo_note(last.status(), dropped);
        self.photo_colours.last = Some(last);
    }

    /// The tool as `status` of the local API reports it.
    pub(crate) fn photo_colours_value(&self) -> Value {
        let tool = &self.photo_colours;
        json!({
            "settings": tool.settings.value(),
            "job": tool.job.as_ref().map(ColourJob::progress_value),
            "last": tool.last.as_ref().map(Last::value),
        })
    }

    /// The `colour_from_photos` command of the local API: put the settings
    /// it names in the Properties block and colour with what the block then
    /// holds.
    pub(crate) fn api_colour_from_photos(
        &mut self,
        options: &ColourOptions,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        if self.photo_colours.is_running() {
            return refuse(Refusal::Busy.api());
        }
        let settings = match self.photo_colours.settings.with(options) {
            Ok(settings) => settings,
            Err(problem) => return refuse(problem),
        };
        let input = match settings
            .config()
            .map_err(Refusal::Setting)
            .and_then(|config| {
                let index = self.colour_layer(options.layer)?;
                self.colour_input(index, config)
            }) {
            Ok(input) => input,
            Err(refusal) => return refuse(refusal.api()),
        };
        self.photo_colours.settings = settings;
        let id =
            self.record_api_job(json!({"state": "running", "operation": "colour_from_photos"}));
        let task = self.start_colour_job(input, Some(id.clone()));
        (json!({"ok": true, "accepted": true, "job_id": id}), task)
    }

    /// The `cancel_colour_from_photos` command of the local API.
    pub(crate) fn api_cancel_colour_from_photos(&mut self) -> Value {
        if !self.photo_colours.is_running() {
            return json!({"ok": false, "error": "no colouring from photos is running"});
        }
        self.cancel_photo_colours();
        json!({"ok": true, "cancel_requested": true})
    }

    /// The `clear_photo_colours` command of the local API.
    pub(crate) fn api_clear_photo_colours(&mut self, layer: Option<usize>) -> Value {
        let Some(index) = layer
            .or(self.active)
            .filter(|index| *index < self.clouds.len())
        else {
            return json!({"ok": false, "error": "no active cloud"});
        };
        match self.clear_photo_colours(index) {
            Some((name, dropped)) => {
                self.status = with_undo_note(
                    format!("Photo colours of {name} removed; Undo brings them back"),
                    dropped,
                );
                json!({"ok": true, "layer": index, "source": name})
            }
            None => json!({"ok": false, "error": "the layer has no photo colours"}),
        }
    }

    /// The Colour from photos block in Properties, for the active scan when
    /// it has photos.
    pub(crate) fn photo_colour_properties(&self, index: usize) -> Option<Element<'_, Message>> {
        let entry = self.clouds.get(index)?;
        let photos = self.colour_photos(index);
        if photos.is_empty() {
            return None;
        }
        let tool = &self.photo_colours;
        let settings = &tool.settings;
        let colors = self.ui_theme.colors();
        let note = |content: String| {
            container(text(content).size(10).color(colors.text_muted)).padding([4, 8])
        };
        let warning = |content: String| {
            container(text(content).size(10).color(colors.accent)).padding([4, 8])
        };
        let mut block = column![
            opencad_properties::section_header("Colour from photos"),
            opencad_properties::property_input(
                "Largest distance (m)",
                "20",
                &settings.max_distance,
                |value| Message::PhotoColours(PhotoColourAction::MaxDistance(value)),
            ),
            opencad_properties::property_control(
                "Photos",
                opencad_properties::explained(
                    ui_style::checkbox(tr("Blend"), settings.blend)
                        .on_toggle(|value| Message::PhotoColours(PhotoColourAction::Blend(value))),
                    vec![tr(
                        "Every photo that sees a point adds to its colour, weighted strongly to the nearest: this evens out the exposure of photos taken one after another. Without it the nearest photo alone gives the colour."
                    )
                    .to_owned()],
                ),
            ),
        ]
        .spacing(0)
        .width(Fill);

        if let Some(job) = &tool.job {
            let cancelling = job.cancelling();
            let state = if cancelling {
                tr("Cancelling…").to_owned()
            } else {
                job.control.snapshot().text()
            };
            block = block
                .push(container(text(state).size(11)).padding([6, 8]))
                .push(
                    container(
                        button(tr("Cancel"))
                            .on_press_maybe(
                                (!cancelling)
                                    .then_some(Message::PhotoColours(PhotoColourAction::Cancel)),
                            )
                            .style(ui_style::tool),
                    )
                    .padding([3, 8]),
                );
        } else {
            let name = display_name(&entry.cloud.path).to_owned();
            let count = photos.len();
            let values: [(&str, &dyn fmt::Display); 2] = [("name", &name), ("count", &count)];
            let mut explanation = vec![if self.section_box().is_some() {
                tr_args(
                    "Gives the points of {name} in the section box the colours {count} photos see them with.",
                    &values,
                )
            } else {
                tr_args(
                    "Gives every point of {name} the colours {count} photos see them with.",
                    &values,
                )
            }];
            explanation.push(
                tr("A photo colours a point when nothing of the scan hides it, the nearest photo first. Points no photo sees keep their colour. Undo takes the colours back; an export writes them.")
                    .to_owned(),
            );
            let mut warnings = Vec::new();
            let ready = match self.colour_request(Some(index)) {
                Ok(_) => true,
                Err(refusal) => {
                    warnings.push(refusal.sentence().translated());
                    false
                }
            };
            let mut buttons = row![opencad_properties::explained(
                button(tr("Colour points"))
                    .on_press_maybe(
                        ready.then_some(Message::PhotoColours(PhotoColourAction::Start))
                    )
                    .style(|theme, status| ui_style::ribbon_button(theme, false, status)),
                explanation,
            )]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            if entry.colours.is_some() && self.active == Some(index) {
                buttons = buttons.push(
                    button(tr("Remove photo colours"))
                        .on_press(Message::PhotoColours(PhotoColourAction::Clear))
                        .style(ui_style::tool),
                );
            }
            if let Some(mark) = opencad_properties::warning_mark(warnings) {
                buttons = buttons.push(mark);
            }
            block = block.push(container(buttons).padding([3, 8]));
        }

        if let Some(colours) = &entry.colours {
            block = block.push(opencad_properties::property_row(
                "Points with photo colours",
                format_count(colours.len()),
            ));
        }
        match &tool.last {
            Some(Last::Failed(error)) => {
                block = block
                    .push(note(
                        tr("The last colouring from photos failed:").to_owned(),
                    ))
                    .push(warning(error.clone()));
            }
            Some(Last::Cancelled) => {
                block = block.push(note(
                    tr("The last colouring from photos was cancelled.").to_owned(),
                ));
            }
            Some(Last::Done { summary, .. }) if summary.seen == 0 => {
                block = block.push(warning(
                    tr("No photo saw the points of the last colouring. Check the section box and the largest distance.")
                        .to_owned(),
                ));
            }
            Some(Last::Done { summary, .. }) => {
                for part in summary_rows(summary) {
                    block = block.push(part);
                }
            }
            None => {}
        }
        Some(block.into())
    }
}

/// The figures of the last colouring in Properties.
fn summary_rows(summary: &Summary) -> Vec<Element<'static, Message>> {
    let mut rows = vec![
        opencad_properties::property_row(
            "Last colouring",
            tr_args(
                "{seconds} s, {used} of {photos} photos",
                &[
                    ("seconds", &format!("{:.1}", summary.seconds)),
                    ("used", &summary.photos_used),
                    ("photos", &summary.photos),
                ],
            ),
        ),
        opencad_properties::property_row(
            "Coloured",
            tr_args(
                "{seen} of {points} points",
                &[
                    ("seen", &format_count(summary.seen)),
                    ("points", &format_count(summary.points)),
                ],
            ),
        ),
        opencad_properties::property_row(
            "Seen by no photo",
            format!("{:.1}%", summary.unseen_share() * 100.0),
        ),
    ];
    if let Some(compared) = summary.compared {
        let three = |values: [String; 3]| values.join("  ");
        rows.push(opencad_properties::property_row(
            "Mean difference R G B",
            three(
                compared
                    .mean_abs_difference
                    .map(|value| format!("{value:.1}")),
            ),
        ));
        rows.push(opencad_properties::property_row(
            "Median difference R G B",
            three(
                compared
                    .median_abs_difference
                    .map(|value| value.to_string()),
            ),
        ));
    }
    rows
}

#[cfg(test)]
mod tests;
