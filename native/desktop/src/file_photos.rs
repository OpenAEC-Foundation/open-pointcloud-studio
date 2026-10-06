//! Photos of a file that stand on their own: panoramas and photos taken
//! along a path, apart from the photos of scanner stations. They are listed
//! from the metadata when a scan opens, marked in the scene along their path,
//! and entered: the camera then stands where the photo was taken and the
//! photo is laid over the points. Photos are decoded one at a time on a
//! worker, with their neighbours along the path read ahead.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iced::widget::canvas::{self, Frame};
use iced::widget::{button, column, container, row, slider, text, tooltip};
use iced::{Color, Element, Fill, Point as UiPoint, Size, Task};
use pointcloud_core::{FilePhoto, FilePhotos, PhotoKind, PhotoProjection, ScanImageFormat};
use serde_json::{json, Value};

use crate::i18n::{self, tr, tr_args};
use crate::native_api::ApiCommand;
use crate::selection::Projection;
use crate::station_photos::{self, WalkView};
use crate::{CloudEntry, DragMode, DragState, Message, PointViewport, Studio};

/// Longest edge of a photo on the graphics device. A larger photo is
/// decoded at a half, a quarter or an eighth of its size.
pub const MAX_PHOTO_EDGE: u32 = 4096;
/// Decoded photos kept: the one shown, its neighbours and the one before.
const CACHED_PHOTOS: usize = 4;
/// Photos read ahead at the same time. The photo that is shown takes one
/// place more, so that it never waits for those read ahead.
const PARALLEL_DECODES: usize = 2;
/// How much of a photo covers the points when it is first entered.
const DEFAULT_BLEND: f32 = 0.7;
/// Largest magnification of a pinhole photo seen from its own camera.
const MAX_PINNED_ZOOM: f32 = 8.0;
/// Marks closer than this on screen share a label.
const MARK_GROUP_RADIUS: f32 = 32.0;
/// How near a click must be to a mark to enter its photo.
const MARK_REACH: f32 = 9.0;
/// From inside a photo, or while walking, the nearest photos within this
/// many metres are marked as rings, and a click on one steps into it.
const STEP_REACH: f64 = 40.0;
/// How many photos are marked from inside a photo.
const STEP_COUNT: usize = 12;
/// A photo taken this close to the eye is the one looked through.
const STEP_SAME: f64 = 0.05;
/// Most labels drawn over the marks.
const MAX_MARK_LABELS: usize = 64;

/// The colour of photo marks and their path.
fn mark_color() -> Color {
    Color::from_rgb8(14, 165, 233)
}

/// What a kind of photo is called where it is shown.
pub(crate) fn kind_label(kind: PhotoKind) -> &'static str {
    match kind {
        PhotoKind::Pinhole => i18n::key("pinhole photo"),
        PhotoKind::Spherical => i18n::key("panorama"),
        PhotoKind::Cylindrical => i18n::key("cylindrical panorama"),
    }
}

/// The name of a photo: the one the file gives, else its number.
pub(crate) fn photo_label(photo: &FilePhoto, index: usize) -> String {
    match &photo.name {
        Some(name) => name.clone(),
        None => tr_args("Photo {number}", &[("number", &(index + 1))]),
    }
}

/// One level of a decoded photo: RGBA pixels, row by row.
pub struct PhotoLevel {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// A photo decoded for the graphics device: its full size, at most
/// `MAX_PHOTO_EDGE` pixels each way, and every level below it down to one
/// pixel, each half the size of the one before.
pub struct DecodedPhoto {
    pub source: PathBuf,
    pub index: usize,
    pub levels: Vec<PhotoLevel>,
    /// How long reading and decoding took.
    pub decode_time: Duration,
}

impl fmt::Debug for DecodedPhoto {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecodedPhoto")
            .field("source", &self.source)
            .field("index", &self.index)
            .field(
                "size",
                &self.levels.first().map(|level| (level.width, level.height)),
            )
            .field("levels", &self.levels.len())
            .finish()
    }
}

impl DecodedPhoto {
    pub fn bytes(&self) -> usize {
        self.levels.iter().map(|level| level.pixels.len()).sum()
    }
}

/// The JPEG scale that decodes a photo of this size to at most `max_edge`
/// pixels each way: 1, 2, 4 or 8, the first that fits.
pub(crate) fn jpeg_reduction(width: u32, height: u32, max_edge: u32) -> u32 {
    [1, 2, 4, 8]
        .into_iter()
        .find(|factor| width.div_ceil(*factor) <= max_edge && height.div_ceil(*factor) <= max_edge)
        .unwrap_or(8)
}

/// Decode a stored photo to RGBA, at most `max_edge` pixels each way.
pub(crate) fn decode_pixels(
    bytes: &[u8],
    format: ScanImageFormat,
    max_edge: u32,
) -> Result<::image::RgbaImage, String> {
    use ::image::ImageDecoder;
    let decoded = match format {
        ScanImageFormat::Jpeg => {
            let mut decoder = ::image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))
                .map_err(|error| error.to_string())?;
            let (width, height) = decoder.dimensions();
            let factor = jpeg_reduction(width, height, max_edge);
            if factor > 1 {
                // The decoder takes the smallest scale that gives at least
                // the size asked for.
                let request = |size: u32| size.div_ceil(factor).min(u32::from(u16::MAX)) as u16;
                decoder
                    .scale(request(width), request(height))
                    .map_err(|error| error.to_string())?;
            }
            ::image::DynamicImage::from_decoder(decoder).map_err(|error| error.to_string())?
        }
        ScanImageFormat::Png => {
            ::image::load_from_memory_with_format(bytes, ::image::ImageFormat::Png)
                .map_err(|error| error.to_string())?
        }
    };
    let decoded = if decoded.width() > max_edge || decoded.height() > max_edge {
        decoded.resize(max_edge, max_edge, ::image::imageops::FilterType::Triangle)
    } else {
        decoded
    };
    Ok(decoded.into_rgba8())
}

/// A level half the size of another, each pixel the mean of the two by two
/// it covers; a side of one pixel stays one pixel.
fn halve(level: &PhotoLevel) -> PhotoLevel {
    let width = (level.width / 2).max(1);
    let height = (level.height / 2).max(1);
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let source = |x: u32, y: u32| {
        let x = x.min(level.width - 1);
        let y = y.min(level.height - 1);
        ((y * level.width + x) * 4) as usize
    };
    for y in 0..height {
        for x in 0..width {
            let corners = [
                source(2 * x, 2 * y),
                source(2 * x + 1, 2 * y),
                source(2 * x, 2 * y + 1),
                source(2 * x + 1, 2 * y + 1),
            ];
            let target = ((y * width + x) * 4) as usize;
            for channel in 0..4 {
                let sum: u32 = corners
                    .iter()
                    .map(|corner| u32::from(level.pixels[corner + channel]))
                    .sum();
                pixels[target + channel] = ((sum + 2) / 4) as u8;
            }
        }
    }
    PhotoLevel {
        width,
        height,
        pixels,
    }
}

/// Every level of a photo, from its full size down to one pixel.
pub(crate) fn levels(full: ::image::RgbaImage) -> Vec<PhotoLevel> {
    let mut levels = vec![PhotoLevel {
        width: full.width(),
        height: full.height(),
        pixels: full.into_raw(),
    }];
    while let Some(last) = levels
        .last()
        .filter(|last| last.width > 1 || last.height > 1)
    {
        let next = halve(last);
        levels.push(next);
    }
    levels
}

/// Read and decode one photo of a source; `None` when `wanted` says, before
/// the photo is read and again before it is decoded, that it is no longer
/// needed.
pub(crate) fn decode_photo(
    source: &Path,
    index: usize,
    photo: &FilePhoto,
    wanted: &dyn Fn() -> bool,
) -> Option<Result<DecodedPhoto, String>> {
    decode_read(
        source,
        index,
        photo.format,
        || pointcloud_core::read_file_photo(source, photo).map_err(|error| error.to_string()),
        wanted,
    )
}

/// Decode the bytes `read` gives, unless `wanted` says before the reading
/// or the decoding that the photo is no longer needed.
fn decode_read(
    source: &Path,
    index: usize,
    format: ScanImageFormat,
    read: impl FnOnce() -> Result<Vec<u8>, String>,
    wanted: &dyn Fn() -> bool,
) -> Option<Result<DecodedPhoto, String>> {
    let started = Instant::now();
    if !wanted() {
        return None;
    }
    let bytes = match read() {
        Ok(bytes) => bytes,
        Err(error) => return Some(Err(error)),
    };
    if !wanted() {
        return None;
    }
    Some(
        decode_pixels(&bytes, format, MAX_PHOTO_EDGE).map(|full| DecodedPhoto {
            source: source.to_path_buf(),
            index,
            levels: levels(full),
            decode_time: started.elapsed(),
        }),
    )
}

/// The photo that is shown, as the decodes under way see it: a photo that
/// was stepped past while it waited is not decoded.
#[derive(Debug, Default)]
pub(crate) struct Wanted(Mutex<Option<(PathBuf, usize)>>);

impl Wanted {
    fn set(&self, shown: Option<(&Path, usize)>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) =
            shown.map(|(source, index)| (source.to_path_buf(), index));
    }

    /// Whether a photo is the one that is shown or a neighbour of it.
    fn wants(&self, source: &Path, index: usize) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .is_some_and(|(shown, at)| shown == source && at.abs_diff(index) <= 1)
    }
}

/// Where the camera was before the first photo was entered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ReturnCamera {
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    orbit_point: Option<[f64; 3]>,
    view_label: &'static str,
    walk: Option<WalkView>,
    /// The photo station the walking camera stood in, whose panorama comes
    /// back with it.
    station: Option<(usize, usize)>,
}

impl ReturnCamera {
    fn of(studio: &Studio) -> Self {
        Self {
            yaw: studio.yaw,
            pitch: studio.pitch,
            zoom: studio.zoom,
            pan: studio.pan,
            orbit_point: studio.orbit_point,
            view_label: studio.view_label,
            walk: studio.walk,
            station: studio.walk_station,
        }
    }
}

/// The photo that is entered.
#[derive(Debug, Clone)]
pub(crate) struct PhotoView {
    pub source: PathBuf,
    pub index: usize,
    /// A pinhole photo is first seen exactly from its own camera, turned
    /// about its viewing direction as it was taken; looking around hands
    /// the view to the walking camera.
    pub pinned: bool,
    /// Magnification of a pinned photo.
    pub zoom: f32,
    entered: Instant,
    /// How long the photo took from being entered until it could be shown.
    shown_after: Option<Duration>,
    back: ReturnCamera,
}

/// The photos of the open sources and the one that is entered.
pub(crate) struct PhotoTool {
    /// The photos of each open source that has any, from its metadata.
    pub files: HashMap<PathBuf, Arc<FilePhotos>>,
    /// Sources whose photos are being listed.
    listing: HashSet<PathBuf>,
    pub view: Option<PhotoView>,
    /// How much of a photo covers the points: 0 shows the points only, 1
    /// the photo only.
    pub blend: f32,
    /// Decoded photos, the one used last at the back.
    cache: VecDeque<Arc<DecodedPhoto>>,
    /// Photos being decoded.
    decoding: HashSet<(PathBuf, usize)>,
    /// Photos that could not be decoded.
    failed: HashSet<(PathBuf, usize)>,
    /// The photo that is shown, as the decodes under way see it.
    wanted: Arc<Wanted>,
    /// Sources whose photos the Project Browser lists one by one.
    expanded: HashSet<PathBuf>,
}

impl Default for PhotoTool {
    fn default() -> Self {
        Self {
            files: HashMap::new(),
            listing: HashSet::new(),
            view: None,
            blend: DEFAULT_BLEND,
            cache: VecDeque::new(),
            decoding: HashSet::new(),
            failed: HashSet::new(),
            wanted: Arc::default(),
            expanded: HashSet::new(),
        }
    }
}

impl PhotoTool {
    fn cached(&self, source: &Path, index: usize) -> Option<&Arc<DecodedPhoto>> {
        self.cache
            .iter()
            .find(|photo| photo.index == index && photo.source == source)
    }

    /// Keep a decoded photo, dropping the one used longest ago when there
    /// are too many; the photo that is shown stays.
    fn store(&mut self, photo: Arc<DecodedPhoto>) {
        self.cache
            .retain(|kept| !(kept.index == photo.index && kept.source == photo.source));
        self.cache.push_back(photo);
        while self.cache.len() > CACHED_PHOTOS {
            let shown = self.view.as_ref();
            let Some(oldest) = self.cache.iter().position(|kept| {
                shown.is_none_or(|view| !(kept.index == view.index && kept.source == view.source))
            }) else {
                break;
            };
            self.cache.remove(oldest);
        }
    }

    /// Mark a decoded photo as the one used last.
    fn touch(&mut self, source: &Path, index: usize) {
        if let Some(place) = self
            .cache
            .iter()
            .position(|photo| photo.index == index && photo.source == source)
        {
            if let Some(photo) = self.cache.remove(place) {
                self.cache.push_back(photo);
            }
        }
    }

    /// Leave the photo and let go of the decoded photos.
    pub(crate) fn close(&mut self) {
        self.view = None;
        self.wanted.set(None);
        self.cache.clear();
        self.failed.clear();
    }

    /// Whether a photo is the one that is shown or a neighbour of it along
    /// the path: the photos worth keeping once they are decoded.
    fn near_view(&self, source: &Path, index: usize) -> bool {
        self.view
            .as_ref()
            .is_some_and(|view| view.source == source && view.index.abs_diff(index) <= 1)
    }

    /// Whether the photos of a source are still being listed.
    pub(crate) fn is_listing(&self, source: &Path) -> bool {
        self.listing.contains(source)
    }

    /// Whether the photo that is entered is still being decoded, or waits
    /// for its turn, so that a picture of the view would lack it.
    pub(crate) fn waiting(&self) -> bool {
        self.view.as_ref().is_some_and(|view| {
            self.cached(&view.source, view.index).is_none()
                && !self.failed.contains(&(view.source.clone(), view.index))
        })
    }

    /// Forget the photos of sources that are no longer open.
    fn keep_open(&mut self, clouds: &[CloudEntry]) {
        let open = |path: &Path| clouds.iter().any(|entry| entry.cloud.path == path);
        self.files.retain(|path, _| open(path));
        self.expanded.retain(|path| open(path));
        if self.view.as_ref().is_some_and(|view| !open(&view.source)) {
            self.close();
        }
        self.cache.retain(|photo| open(&photo.source));
    }
}

/// A photo as it is shown from where it was taken.
pub(crate) struct ShownPhoto<'a> {
    pub photo: &'a FilePhoto,
    pub index: usize,
    pub count: usize,
    /// Where it was taken and its camera's axes, in the scene: the layer's
    /// move and scale applied.
    pub eye: [f64; 3],
    pub axes: [[f64; 3]; 3],
    pub decoded: Option<&'a Arc<DecodedPhoto>>,
    /// Whether the photo could not be decoded.
    pub failed: bool,
    pub blend: f32,
    pub pinned: bool,
    pub zoom: f32,
}

impl ShownPhoto<'_> {
    /// The name of the photo under the scene, and whether it is still being
    /// decoded or cannot be shown.
    pub(crate) fn title(&self) -> String {
        let title = tr_args(
            "Photo {number} of {count} · {kind}",
            &[
                ("number", &(self.index + 1)),
                ("count", &self.count),
                ("kind", &tr(kind_label(self.photo.kind()))),
            ],
        );
        if self.failed {
            format!("{title} · {}", tr("cannot be shown"))
        } else if self.decoded.is_none() {
            format!("{title} · {}", tr("loading…"))
        } else {
            title
        }
    }

    /// The camera of a pinned pinhole photo in a viewport of this size: its
    /// right, up and forward directions, and the focal length in pixels at
    /// which the whole photo fits.
    pub(crate) fn pinned_camera(&self, size: Size) -> Option<([[f64; 3]; 3], f32)> {
        let PhotoProjection::Pinhole { focal, .. } = self.photo.projection else {
            return None;
        };
        if !self.pinned || size.width <= 0.0 || size.height <= 0.0 {
            return None;
        }
        let fit = (f64::from(size.width) / f64::from(self.photo.width))
            .min(f64::from(size.height) / f64::from(self.photo.height))
            * f64::from(self.zoom);
        Some((
            [self.axes[0], self.axes[1], self.axes[2].map(|value| -value)],
            (focal[0] * fit) as f32,
        ))
    }

    /// The photo with its camera in the scene.
    #[cfg(test)]
    pub(crate) fn placed(&self) -> FilePhoto {
        FilePhoto {
            position: self.eye,
            axes: self.axes,
            ..self.photo.clone()
        }
    }
}

/// The photo that is entered, as it is shown: none when the walking camera
/// has left the place it was taken.
pub(crate) fn shown<'a>(
    tool: &'a PhotoTool,
    clouds: &'a [CloudEntry],
    walk: Option<WalkView>,
) -> Option<ShownPhoto<'a>> {
    let view = tool.view.as_ref()?;
    let entry = clouds
        .iter()
        .find(|entry| entry.cloud.path == view.source)?;
    let photos = tool.files.get(&view.source)?;
    let photo = photos.photos.get(view.index)?;
    let eye = entry.transform.xyz(photo.position);
    let axes = entry.transform.axes(Some(photo.axes))?;
    if walk?.eye != eye {
        return None;
    }
    Some(ShownPhoto {
        photo,
        index: view.index,
        count: photos.photos.len(),
        eye,
        axes,
        decoded: tool.cached(&view.source, view.index),
        failed: tool.failed.contains(&(view.source.clone(), view.index)),
        blend: tool.blend,
        pinned: view.pinned && photo.kind() == PhotoKind::Pinhole,
        zoom: view.zoom,
    })
}

/// The walking camera that looks the way a photo looks: along a pinhole
/// photo with its field of view, or for a panorama the way `previous` or the
/// orbit camera looked.
fn camera_for(
    photo: &FilePhoto,
    eye: [f64; 3],
    axes: [[f64; 3]; 3],
    previous: Option<WalkView>,
    orbit_yaw: f32,
) -> WalkView {
    match photo.kind() {
        PhotoKind::Pinhole => {
            let forward = axes[2].map(|value| -value);
            let mut view = WalkView::new(eye, forward[1].atan2(forward[0]) as f32);
            view.pitch = (forward[2].clamp(-1.0, 1.0).asin() as f32).clamp(-1.55, 1.55);
            view.field_of_view = (photo.field_of_view()[0] as f32).clamp(
                station_photos::MIN_FIELD_OF_VIEW,
                station_photos::MAX_FIELD_OF_VIEW,
            );
            view
        }
        _ => match previous {
            Some(previous) => WalkView { eye, ..previous },
            None => WalkView::from_orbit(eye, orbit_yaw, 0.0),
        },
    }
}

/// Why a photo cannot be entered: in English for the command API, and in
/// the language of the window for the status line.
#[derive(Debug)]
pub(crate) struct Refusal {
    english: String,
    shown: String,
}

/// A refusal with every `{name}` in it filled in, as `tr_args` does.
fn refusal(text: &'static str, values: &[(&str, &dyn fmt::Display)]) -> Refusal {
    let mut english = text.to_owned();
    for (name, value) in values {
        english = english.replace(&format!("{{{name}}}"), &value.to_string());
    }
    Refusal {
        english,
        shown: tr_args(text, values),
    }
}

/// What the photos ask of the window.
#[derive(Debug, Clone)]
pub enum PhotoAction {
    /// The photos of a source were read from its metadata.
    Listed(PathBuf, Result<Arc<FilePhotos>, String>),
    /// Stand where a photo of a source was taken and show it.
    Enter(PathBuf, usize),
    /// A photo was decoded, or could not be.
    Decoded(PathBuf, usize, Result<Arc<DecodedPhoto>, String>),
    /// A photo was stepped past before it was decoded.
    Skipped(PathBuf, usize),
    /// Go this many photos along the path.
    Step(i64),
    /// How much of the photo covers the points.
    Blend(f32),
    /// Look around from where the photo was taken, as a drag does.
    Look(f32, f32),
    /// Turn the wheel over the photo.
    Zoom(f32),
    /// Open or close the list of photos of a source in the Project Browser.
    ToggleList(PathBuf),
}

impl Studio {
    /// Read the list of photos of a newly opened source in the background.
    pub(crate) fn file_photos_task(
        &mut self,
        cloud: &pointcloud_core::PointCloud,
    ) -> Task<Message> {
        let path = cloud.path.clone();
        let is_e57 = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("e57"));
        if !is_e57 || self.photos.files.contains_key(&path) || self.photos.listing.contains(&path) {
            return Task::none();
        }
        self.photos.listing.insert(path.clone());
        let key = path.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    pointcloud_core::file_photos(&path)
                        .map(Arc::new)
                        .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            move |result| Message::Photos(PhotoAction::Listed(key.clone(), result)),
        )
    }

    /// Forget the photos of sources that are no longer open.
    pub(crate) fn keep_open_photos(&mut self) {
        self.photos.keep_open(&self.clouds);
    }

    /// The photo that is entered, as it is shown.
    pub(crate) fn shown_photo(&self) -> Option<ShownPhoto<'_>> {
        shown(&self.photos, &self.clouds, self.walk)
    }

    pub(crate) fn update_photos(&mut self, action: PhotoAction) -> Task<Message> {
        match action {
            PhotoAction::Listed(path, result) => {
                self.photos.listing.remove(&path);
                let open = self.clouds.iter().any(|entry| entry.cloud.path == path);
                match result {
                    Ok(photos)
                        if open
                            && (!photos.photos.is_empty()
                                || photos.coordinate_system.is_some()) =>
                    {
                        self.photos.files.insert(path, photos);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.status = tr_args(
                            "The photos of {file} could not be read: {error}",
                            &[("file", &crate::display_name(&path)), ("error", &error)],
                        );
                    }
                }
                Task::none()
            }
            PhotoAction::Enter(source, index) => match self.enter_photo(&source, index) {
                Ok(task) => task,
                Err(refused) => {
                    self.status = refused.shown;
                    Task::none()
                }
            },
            PhotoAction::Decoded(source, index, result) => {
                self.photos.decoding.remove(&(source.clone(), index));
                let shown = self
                    .photos
                    .view
                    .as_ref()
                    .is_some_and(|view| view.source == source && view.index == index);
                match result {
                    // A photo that arrives after the photos were left, or
                    // once the camera stepped on past it, is not kept.
                    Ok(_) if !self.photos.near_view(&source, index) => {}
                    Ok(decoded) => {
                        self.photos.store(decoded);
                        if shown {
                            if let Some(view) = &mut self.photos.view {
                                view.shown_after.get_or_insert(view.entered.elapsed());
                            }
                        }
                    }
                    Err(error) => {
                        // It is not asked for again until the photos are left.
                        self.photos.failed.insert((source, index));
                        if shown {
                            self.status = tr_args(
                                "Photo {number} could not be shown: {error}",
                                &[("number", &(index + 1)), ("error", &error)],
                            );
                        }
                    }
                }
                self.fetch_photos()
            }
            PhotoAction::Skipped(source, index) => {
                self.photos.decoding.remove(&(source, index));
                self.fetch_photos()
            }
            // Page Up and Page Down mean nothing without a photo, or while
            // something covers the model.
            PhotoAction::Step(_) if self.shown_photo().is_none() || self.model_covered() => {
                Task::none()
            }
            PhotoAction::Step(steps) => match self.step_photo(steps) {
                Ok(task) => task,
                Err(refused) => {
                    self.status = refused.shown;
                    Task::none()
                }
            },
            PhotoAction::Blend(blend) => {
                if blend.is_finite() {
                    self.photos.blend = blend.clamp(0.0, 1.0);
                }
                Task::none()
            }
            PhotoAction::Look(dx, dy) => {
                let size = self.viewport_size;
                let Some(shown) = self.shown_photo() else {
                    return Task::none();
                };
                // Looking around leaves the camera of a pinhole photo for the
                // walking camera, which looks the same way at the same scale,
                // without the turn about the viewing direction.
                let pinned_focal = shown.pinned_camera(size).map(|(_, focal)| focal);
                if let Some(view) = &mut self.photos.view {
                    view.pinned = false;
                }
                if let Some(view) = &mut self.walk {
                    if let Some(focal) = pinned_focal.filter(|focal| *focal > 0.0) {
                        view.field_of_view = (2.0 * (0.5 * size.width / focal).atan()).clamp(
                            station_photos::MIN_FIELD_OF_VIEW,
                            station_photos::MAX_FIELD_OF_VIEW,
                        );
                    }
                    view.look(dx, dy, size);
                }
                self.revision += 1;
                self.schedule_detail()
            }
            PhotoAction::Zoom(steps) => {
                let pinned = self.shown_photo().is_some_and(|shown| shown.pinned);
                match (&mut self.photos.view, &mut self.walk) {
                    (Some(view), _) if pinned => {
                        view.zoom = (view.zoom * (steps * 0.1).exp()).clamp(1.0, MAX_PINNED_ZOOM);
                    }
                    (_, Some(view)) => view.zoom(steps),
                    _ => return Task::none(),
                }
                self.revision += 1;
                self.schedule_detail()
            }
            PhotoAction::ToggleList(source) => {
                if !self.photos.expanded.remove(&source) {
                    self.photos.expanded.insert(source);
                }
                Task::none()
            }
        }
    }

    /// Stand where a photo was taken and show it over the points.
    pub(crate) fn enter_photo(
        &mut self,
        source: &Path,
        index: usize,
    ) -> Result<Task<Message>, Refusal> {
        let entry = self
            .clouds
            .iter()
            .find(|entry| entry.cloud.path == source)
            .ok_or_else(|| refusal("That scan is not open", &[]))?;
        let photos = self
            .photos
            .files
            .get(source)
            .ok_or_else(|| refusal("That scan has no photos", &[]))?;
        let photo = photos.photos.get(index).ok_or_else(|| {
            refusal(
                "There is no photo {number}: the scan has {count}",
                &[("number", &(index + 1)), ("count", &photos.photos.len())],
            )
        })?;
        let eye = entry.transform.xyz(photo.position);
        let axes = entry
            .transform
            .axes(Some(photo.axes))
            .ok_or_else(|| refusal("The layer is scaled flat: the photo cannot be placed", &[]))?;
        let walk = camera_for(photo, eye, axes, self.walk, self.yaw);
        let kind = photo.kind();
        let count = photos.photos.len();
        let back = match &self.photos.view {
            Some(view) => view.back,
            None => ReturnCamera::of(self),
        };
        self.walk = Some(walk);
        self.walk_station = None;
        self.panorama_photos = None;
        self.walk_keys = [false; 6];
        self.walk_tick = None;
        self.context_menu = None;
        self.box_select = false;
        self.pick_mode = false;
        self.drag_rectangle = None;
        let cached = self.photos.cached(source, index).is_some();
        self.photos.view = Some(PhotoView {
            source: source.to_path_buf(),
            index,
            pinned: kind == PhotoKind::Pinhole,
            zoom: 1.0,
            entered: Instant::now(),
            shown_after: cached.then_some(Duration::ZERO),
            back,
        });
        self.photos.touch(source, index);
        self.revision += 1;
        self.status = tr_args(
            "Photo {number} of {count} · {kind} · drag to look around, scroll to zoom, Page Up and Page Down for the previous and next photo, Esc to go back",
            &[
                ("number", &(index + 1)),
                ("count", &count),
                ("kind", &tr(kind_label(kind))),
            ],
        );
        Ok(Task::batch([self.fetch_photos(), self.schedule_detail()]))
    }

    /// Go to another photo along the path of the one that is entered.
    pub(crate) fn step_photo(&mut self, steps: i64) -> Result<Task<Message>, Refusal> {
        let view = self
            .photos
            .view
            .as_ref()
            .ok_or_else(|| refusal("No photo is entered", &[]))?;
        let count = self
            .photos
            .files
            .get(&view.source)
            .map_or(0, |photos| photos.photos.len());
        let target = view.index as i64 + steps;
        if target < 0 {
            return Err(refusal("This is the first photo along the path", &[]));
        }
        if target >= count as i64 {
            return Err(refusal("This is the last photo along the path", &[]));
        }
        let source = view.source.clone();
        self.enter_photo(&source, target as usize)
    }

    /// Leave the photo that is entered and put the camera back where it was
    /// before; `None` when no photo is entered.
    pub(crate) fn leave_photo(&mut self) -> Option<Task<Message>> {
        let back = self.photos.view.take()?.back;
        self.photos.close();
        self.leave_walk();
        self.yaw = back.yaw;
        self.pitch = back.pitch;
        self.zoom = back.zoom;
        self.pan = back.pan;
        self.orbit_point = back.orbit_point;
        self.view_label = back.view_label;
        self.walk = back.walk;
        // The panorama of the station the photo was entered from is shown
        // again.
        let station = match back.station.filter(|_| back.walk.is_some()) {
            Some((cloud, station)) => {
                self.walk_station = Some((cloud, station));
                self.panorama_task(cloud, station)
            }
            None => Task::none(),
        };
        self.revision += 1;
        self.status = tr("Back where the camera was before the photo").into();
        Some(Task::batch([station, self.schedule_detail()]))
    }

    /// Start decoding what the entered photo needs: the photo itself first,
    /// then its neighbours along the path.
    fn fetch_photos(&mut self) -> Task<Message> {
        let Some(view) = &self.photos.view else {
            return Task::none();
        };
        let Some(photos) = self.photos.files.get(&view.source) else {
            return Task::none();
        };
        let source = view.source.clone();
        self.photos.wanted.set(Some((&source, view.index)));
        let wanted = [
            Some(view.index),
            view.index.checked_add(1),
            view.index.checked_sub(1),
        ];
        // The neighbours wait until the photo that is shown is decoded, or
        // could not be.
        let shown_done = self.photos.cached(&source, view.index).is_some()
            || self.photos.failed.contains(&(source.clone(), view.index));
        let mut tasks = Vec::new();
        for (place, index) in wanted.into_iter().enumerate() {
            let Some(photo) = index.and_then(|index| photos.photos.get(index)) else {
                continue;
            };
            let index = index.unwrap_or_default();
            if place > 0 && !shown_done {
                break;
            }
            let key = (source.clone(), index);
            if self.photos.cached(&source, index).is_some()
                || self.photos.decoding.contains(&key)
                || self.photos.failed.contains(&key)
            {
                continue;
            }
            // A few at a time: while Page Down is held, the photo that is
            // shown waits for a place and is asked for again when a decode
            // ends.
            let places = if place == 0 {
                PARALLEL_DECODES + 1
            } else {
                PARALLEL_DECODES
            };
            if self.photos.decoding.len() >= places {
                break;
            }
            self.photos.decoding.insert(key);
            let photo = photo.clone();
            let path = source.clone();
            let reply = source.clone();
            let still = Arc::clone(&self.photos.wanted);
            tasks.push(Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || {
                        decode_photo(&path, index, &photo, &|| still.wants(&path, index))
                            .map(|result| result.map(Arc::new))
                    })
                    .await
                    .unwrap_or_else(|error| Some(Err(error.to_string())))
                },
                move |result| {
                    Message::Photos(match result {
                        Some(result) => PhotoAction::Decoded(reply.clone(), index, result),
                        None => PhotoAction::Skipped(reply.clone(), index),
                    })
                },
            ));
        }
        Task::batch(tasks)
    }

    /// The layer whose photos a command means: the one it names, else the
    /// active layer when it has photos, else the first layer with photos.
    fn photo_layer(&self, layer: Option<usize>) -> Result<usize, String> {
        let has_photos = |index: usize| {
            self.clouds.get(index).is_some_and(|entry| {
                self.photos
                    .files
                    .get(&entry.cloud.path)
                    .is_some_and(|photos| !photos.photos.is_empty())
            })
        };
        match layer {
            Some(index) if index >= self.clouds.len() => Err("layer index is out of range".into()),
            Some(index) if has_photos(index) => Ok(index),
            Some(_) => Err("that layer has no photos".into()),
            None => self
                .active
                .filter(|index| has_photos(*index))
                .or_else(|| (0..self.clouds.len()).find(|index| has_photos(*index)))
                .ok_or_else(|| "no open layer has photos".into()),
        }
    }

    /// A photo as the command API reports it, in scene coordinates.
    fn photo_value(entry: &CloudEntry, photo: &FilePhoto, index: usize) -> Value {
        let direction = entry.transform.axes(Some(photo.axes)).map(|axes| {
            FilePhoto {
                axes,
                ..photo.clone()
            }
            .view_direction()
        });
        json!({
            "index": index,
            "kind": photo.kind().key(),
            "name": photo.name,
            "width": photo.width,
            "height": photo.height,
            "position": entry.transform.xyz(photo.position),
            "direction": direction,
            "station": photo.station,
        })
    }

    /// The photos as `status` reports them.
    pub(crate) fn photos_value(&self) -> Value {
        let files: Vec<Value> = self
            .clouds
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let photos = self.photos.files.get(&entry.cloud.path)?;
                Some(json!({
                    "layer": index,
                    "photos": photos.photos.len(),
                    "pinhole": photos.count(PhotoKind::Pinhole),
                    "spherical": photos.count(PhotoKind::Spherical),
                    "cylindrical": photos.count(PhotoKind::Cylindrical),
                    "skipped": photos.skipped,
                    "coordinate_system": photos.coordinate_system,
                }))
            })
            .collect();
        let view = self.photos.view.as_ref().map(|view| {
            let shown = self.shown_photo();
            json!({
                "layer": self.clouds.iter().position(|entry| entry.cloud.path == view.source),
                "index": view.index,
                "count": shown.as_ref().map(|shown| shown.count),
                "kind": shown.as_ref().map(|shown| shown.photo.kind().key()),
                "pinned": shown.as_ref().is_some_and(|shown| shown.pinned),
                "zoom": view.zoom,
                "shown": shown.as_ref().is_some_and(|shown| shown.decoded.is_some()),
                "failed": shown.as_ref().is_some_and(|shown| shown.failed),
                "shown_after_ms": view.shown_after.map(|time| time.as_millis() as u64),
                "decode_ms": shown
                    .as_ref()
                    .and_then(|shown| shown.decoded)
                    .map(|decoded| decoded.decode_time.as_millis() as u64),
            })
        });
        json!({
            "files": files,
            "listing": self.photos.listing.len(),
            "decoding": self.photos.decoding.len(),
            "blend": self.photos.blend,
            "cached": self.photos.cache.len(),
            "cached_bytes": self.photos.cache.iter().map(|photo| photo.bytes()).sum::<usize>(),
            "view": view,
        })
    }

    /// The photo commands of the local API.
    pub(crate) fn api_photos(&mut self, command: ApiCommand) -> (Value, Task<Message>) {
        let failed = |error: String| (json!({"ok": false, "error": error}), Task::none());
        match command {
            ApiCommand::ListPhotos { layer } => {
                let layer = match self.photo_layer(layer) {
                    Ok(layer) => layer,
                    Err(error) => return failed(error),
                };
                let entry = &self.clouds[layer];
                let Some(photos) = self.photos.files.get(&entry.cloud.path) else {
                    return failed("that layer has no photos".into());
                };
                let list: Vec<Value> = photos
                    .photos
                    .iter()
                    .enumerate()
                    .map(|(index, photo)| Self::photo_value(entry, photo, index))
                    .collect();
                (
                    json!({
                        "ok": true,
                        "layer": layer,
                        "path": entry.cloud.path,
                        "coordinate_system": photos.coordinate_system,
                        "skipped": photos.skipped,
                        "photos": list,
                    }),
                    Task::none(),
                )
            }
            ApiCommand::EnterPhoto { index, layer } => {
                let layer = match self.photo_layer(layer) {
                    Ok(layer) => layer,
                    Err(error) => return failed(error),
                };
                let source = self.clouds[layer].cloud.path.clone();
                match self.enter_photo(&source, index) {
                    Ok(task) => (self.entered_value(), task),
                    Err(refused) => failed(refused.english),
                }
            }
            ApiCommand::NextPhoto | ApiCommand::PreviousPhoto => {
                let steps = if matches!(command, ApiCommand::NextPhoto) {
                    1
                } else {
                    -1
                };
                match self.step_photo(steps) {
                    Ok(task) => (self.entered_value(), task),
                    Err(refused) => failed(refused.english),
                }
            }
            ApiCommand::PhotoBlend { value } => {
                if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                    return failed("photo_blend requires a value from 0 to 1".into());
                }
                self.photos.blend = value;
                (json!({"ok": true, "blend": value}), Task::none())
            }
            _ => failed("not a photo command".into()),
        }
    }

    /// The answer to a command that entered a photo.
    fn entered_value(&self) -> Value {
        let photo = self.photos.view.as_ref().and_then(|view| {
            let entry = self
                .clouds
                .iter()
                .find(|entry| entry.cloud.path == view.source)?;
            let photo = self
                .photos
                .files
                .get(&view.source)?
                .photos
                .get(view.index)?;
            Some(Self::photo_value(entry, photo, view.index))
        });
        json!({"ok": true, "photo": photo, "walk": self.walk_value()})
    }

    /// The photos of a scan in the Project Browser: one line that opens a
    /// row per photo, closed at first.
    pub(crate) fn photo_rows(&self, cloud: usize) -> Option<Element<'_, Message>> {
        let entry = self.clouds.get(cloud)?;
        let photos = self.photos.files.get(&entry.cloud.path)?;
        if photos.photos.is_empty() {
            return None;
        }
        let source = entry.cloud.path.clone();
        let expanded = self.photos.expanded.contains(&source);
        let header = tr_args("{count} photos", &[("count", &photos.photos.len())]);
        // The chevron of the groups of the Project Browser.
        let chevron = if expanded {
            crate::ToolIcon::ChevronOpen
        } else {
            crate::ToolIcon::ChevronClosed
        };
        let mut rows = column![button(
            row![crate::icon_svg(chevron, 10.0), text(header).size(11)]
                .spacing(4)
                .align_y(iced::Alignment::Center)
        )
        .on_press(Message::Photos(PhotoAction::ToggleList(source.clone())))
        .style(crate::flat_tool_style)
        .padding([1, 4])]
        .spacing(1);
        if expanded {
            let entered = self
                .photos
                .view
                .as_ref()
                .filter(|view| view.source == source)
                .map(|view| view.index);
            for (index, photo) in photos.photos.iter().enumerate() {
                let label = format!(
                    "{} · {}",
                    photo_label(photo, index),
                    tr(kind_label(photo.kind()))
                );
                let current = entered == Some(index);
                rows = rows.push(
                    button(text(label).size(10))
                        .on_press(Message::Photos(PhotoAction::Enter(source.clone(), index)))
                        .style(move |theme, status| {
                            let mut style = crate::flat_tool_style(theme, status);
                            if current {
                                style.text_color = crate::ui_theme::colors(theme).accent;
                            }
                            style
                        })
                        .width(Fill)
                        .padding([0, 4]),
                );
            }
        }
        Some(
            container(rows)
                .padding(iced::Padding {
                    left: 18.0,
                    ..iced::Padding::ZERO
                })
                .into(),
        )
    }

    /// The photos of the active scan in Properties: how many of each kind,
    /// and the coordinate system the file states.
    pub(crate) fn photo_properties(&self, entry: &CloudEntry) -> Option<Element<'_, Message>> {
        use crate::opencad_properties::{property_row, section_header};
        let photos = self.photos.files.get(&entry.cloud.path)?;
        let mut section = column![section_header(if photos.photos.is_empty() {
            i18n::key("Coordinates")
        } else {
            i18n::key("Photos")
        })];
        for (kind, label) in [
            (PhotoKind::Spherical, i18n::key("Panoramas")),
            (PhotoKind::Pinhole, i18n::key("Pinhole photos")),
            (PhotoKind::Cylindrical, i18n::key("Cylindrical panoramas")),
        ] {
            let count = photos.count(kind);
            if count > 0 {
                section = section.push(property_row(label, crate::format_count(count)));
            }
        }
        if let Some(system) = &photos.coordinate_system {
            section = section.push(property_row("Coordinate system", system.clone()));
        }
        Some(section.into())
    }

    /// The controls over the scene while a photo is entered: the previous
    /// and next photo along the path, and how much of the photo covers the
    /// points.
    pub(crate) fn photo_controls(&self) -> Option<Element<'_, Message>> {
        let shown = self.shown_photo()?;
        let step = |label: &'static str, tip: &'static str, steps: i64, enabled: bool| {
            tooltip(
                button(text(label).size(13))
                    .on_press_maybe(enabled.then_some(Message::Photos(PhotoAction::Step(steps))))
                    .style(crate::flat_tool_style)
                    .padding([1, 8]),
                container(text(tr(tip)).size(11))
                    .padding([3, 6])
                    .style(|theme| {
                        let colors = crate::ui_theme::colors(theme);
                        container::Style::default()
                            .background(colors.panel_alt)
                            .color(colors.text)
                    }),
                tooltip::Position::Bottom,
            )
        };
        let panel = row![
            step(
                "◀",
                i18n::key("Previous photo (Page Up)"),
                -1,
                shown.index > 0
            ),
            text(tr_args(
                "Photo {number} of {count}",
                &[("number", &(shown.index + 1)), ("count", &shown.count)],
            ))
            .size(12),
            step(
                "▶",
                i18n::key("Next photo (Page Down)"),
                1,
                shown.index + 1 < shown.count
            ),
            iced::widget::Space::with_width(10),
            text(tr("Photo")).size(12),
            slider(0.0_f32..=1.0, self.photos.blend, |blend| {
                Message::Photos(PhotoAction::Blend(blend))
            })
            .step(0.01_f32)
            .width(120),
            text(format!("{:.0}%", self.photos.blend * 100.0))
                .size(11)
                .width(34),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        Some(
            container(container(panel).padding([4, 8]).style(|theme| {
                let colors = crate::ui_theme::colors(theme);
                container::Style::default()
                    .background(colors.panel)
                    .color(colors.text)
                    .border(iced::Border {
                        color: mark_color(),
                        width: 1.0,
                        radius: 6.0.into(),
                    })
            }))
            .width(Fill)
            .align_x(iced::Alignment::End)
            .padding(10)
            .into(),
        )
    }
}

/// A photo marked on screen.
struct PhotoMark<'a> {
    source: &'a Path,
    index: usize,
    kind: PhotoKind,
    x: f32,
    y: f32,
    /// Where a short line along the viewing direction of a pinhole photo
    /// ends on screen.
    toward: Option<(f32, f32)>,
    label: String,
}

/// Groups of marks that lie close together on screen, each as its first
/// mark and how many it holds.
fn mark_groups(points: &[(f32, f32)]) -> Vec<(usize, usize)> {
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (index, (x, y)) in points.iter().enumerate() {
        match groups.iter_mut().find(|(first, _)| {
            let (fx, fy) = points[*first];
            (fx - x).hypot(fy - y) <= MARK_GROUP_RADIUS
        }) {
            Some(group) => group.1 += 1,
            None => groups.push((index, 1)),
        }
    }
    groups
}

/// Places for labels beside their anchors that cover no other label and no
/// mark, the largest groups first; `None` where there is no room.
fn label_places(
    labels: &[(f32, f32, f32, usize)],
    marks: &[(f32, f32)],
    viewport: Size,
) -> Vec<Option<UiPoint>> {
    let mut places = vec![None; labels.len()];
    let mut taken: Vec<[f32; 4]> = Vec::new();
    let mut order: Vec<usize> = (0..labels.len()).collect();
    order.sort_by_key(|index| std::cmp::Reverse(labels[*index].3));
    for index in order.into_iter().take(MAX_MARK_LABELS) {
        let (x, y, width, _) = labels[index];
        let candidates = [
            (x + 10.0, y - 7.0),
            (x - width - 10.0, y - 7.0),
            (x - width * 0.5, y - 22.0),
            (x - width * 0.5, y + 10.0),
        ];
        for (left, top) in candidates {
            let rect = [left, top, left + width, top + 13.0];
            let outside = left < 2.0
                || top < 2.0
                || rect[2] > viewport.width - 2.0
                || rect[3] > viewport.height - 2.0;
            let overlaps = taken.iter().any(|other| {
                rect[0] < other[2] + 4.0
                    && rect[2] + 4.0 > other[0]
                    && rect[1] < other[3] + 4.0
                    && rect[3] + 4.0 > other[1]
            });
            let covers_mark = marks.iter().any(|(mx, my)| {
                (*mx - x).hypot(*my - y) > 0.5
                    && rect[0] < mx + 6.0
                    && rect[2] > mx - 6.0
                    && rect[1] < my + 6.0
                    && rect[3] > my - 6.0
            });
            if outside || overlaps || covers_mark {
                continue;
            }
            places[index] = Some(UiPoint::new(left, top));
            taken.push(rect);
            break;
        }
    }
    places
}

impl PointViewport<'_> {
    /// The photo that is entered, as it is shown.
    pub(crate) fn shown_photo(&self) -> Option<ShownPhoto<'_>> {
        shown(self.photos, self.clouds, self.walk)
    }

    /// Every photo of the visible layers that is in front of the camera, in
    /// the order of its path.
    fn photo_marks(&self, projection: Projection) -> Vec<PhotoMark<'_>> {
        self.photo_marks_where(projection, |_| true)
    }

    /// The photos in front of the camera whose position `keep` accepts.
    fn photo_marks_where(
        &self,
        projection: Projection,
        keep: impl Fn([f64; 3]) -> bool,
    ) -> Vec<PhotoMark<'_>> {
        let mut marks = Vec::new();
        for entry in self.clouds.iter().filter(|entry| entry.visible) {
            let Some(photos) = self.photos.files.get(&entry.cloud.path) else {
                continue;
            };
            for (index, photo) in photos.photos.iter().enumerate() {
                let position = entry.transform.xyz(photo.position);
                if !keep(position) {
                    continue;
                }
                let Some((x, y, depth)) = projection.project(position) else {
                    continue;
                };
                let toward = (photo.kind() == PhotoKind::Pinhole)
                    .then(|| {
                        let axes = entry.transform.axes(Some(photo.axes))?;
                        let forward = axes[2].map(|value| -value);
                        // A step ahead small enough to stay in front of the eye.
                        let step = depth * 0.05;
                        let ahead: [f64; 3] =
                            std::array::from_fn(|axis| position[axis] + forward[axis] * step);
                        let (ax, ay, _) = projection.project_unclipped(ahead)?;
                        let length = (ax - x).hypot(ay - y);
                        (length > 0.5)
                            .then(|| (x + (ax - x) / length * 13.0, y + (ay - y) / length * 13.0))
                    })
                    .flatten();
                marks.push(PhotoMark {
                    source: &entry.cloud.path,
                    index,
                    kind: photo.kind(),
                    x,
                    y,
                    toward,
                    label: photo_label(photo, index),
                });
            }
        }
        marks
    }

    /// The other photos that can be stepped into from `eye`, the eye of a
    /// photo or of the walking camera: the nearest `STEP_COUNT` within
    /// `STEP_REACH` that are in front of the camera.
    fn photo_steps(&self, projection: Projection, eye: [f64; 3]) -> Vec<PhotoMark<'_>> {
        let distance = |position: [f64; 3]| {
            (0..3)
                .map(|axis| (position[axis] - eye[axis]).powi(2))
                .sum::<f64>()
                .sqrt()
        };
        let mut near: Vec<f64> = self
            .clouds
            .iter()
            .filter(|entry| entry.visible)
            .filter_map(|entry| {
                let photos = self.photos.files.get(&entry.cloud.path)?;
                Some(
                    photos
                        .photos
                        .iter()
                        .map(|photo| distance(entry.transform.xyz(photo.position)))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .filter(|&away| away > STEP_SAME && away <= STEP_REACH)
            .collect();
        near.sort_by(f64::total_cmp);
        let Some(&farthest) = near.get(STEP_COUNT.min(near.len()).saturating_sub(1)) else {
            return Vec::new();
        };
        self.photo_marks_where(projection, |position| {
            let away = distance(position);
            away > STEP_SAME && away <= farthest
        })
    }

    /// The photo to step into under the pointer, from inside a photo or
    /// while walking.
    pub(crate) fn photo_step_at(
        &self,
        projection: Projection,
        eye: [f64; 3],
        pointer: [f32; 2],
    ) -> Option<(PathBuf, usize)> {
        self.photo_steps(projection, eye)
            .into_iter()
            .map(|mark| {
                let distance = (mark.x - pointer[0]).hypot(mark.y - pointer[1]);
                (mark, distance)
            })
            .filter(|(_, distance)| *distance <= MARK_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(mark, _)| (mark.source.to_path_buf(), mark.index))
    }

    /// The photos that can be stepped into, as rings in the colour of the
    /// photo marks.
    pub(crate) fn draw_photo_steps(
        &self,
        frame: &mut Frame,
        projection: Projection,
        eye: [f64; 3],
    ) {
        let color = mark_color();
        for mark in self.photo_steps(projection, eye) {
            let ring = canvas::Path::circle(UiPoint::new(mark.x, mark.y), 7.0);
            frame.fill(&ring, Color::from_rgba8(42, 42, 50, 0.6));
            frame.stroke(
                &ring,
                canvas::Stroke::default().with_color(color).with_width(2.0),
            );
        }
    }

    /// The photo whose mark is under the pointer.
    pub(crate) fn photo_mark_at(
        &self,
        projection: Projection,
        pointer: [f32; 2],
    ) -> Option<(PathBuf, usize)> {
        self.photo_marks(projection)
            .into_iter()
            .map(|mark| {
                let distance = (mark.x - pointer[0]).hypot(mark.y - pointer[1]);
                (mark, distance)
            })
            .filter(|(_, distance)| *distance <= MARK_REACH)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(mark, _)| (mark.source.to_path_buf(), mark.index))
    }

    /// The photos along their path: a line through them, a small ball for a
    /// panorama, a dot with its viewing direction for a pinhole photo, and
    /// labels for the marks or groups of marks that have room.
    pub(crate) fn draw_photo_marks(&self, frame: &mut Frame, projection: Projection, size: Size) {
        let color = mark_color();
        // The path: one line from each photo to the next of the same file.
        for entry in self.clouds.iter().filter(|entry| entry.visible) {
            let Some(photos) = self.photos.files.get(&entry.cloud.path) else {
                continue;
            };
            let mut previous: Option<(f32, f32)> = None;
            for photo in &photos.photos {
                let at = projection
                    .project_unclipped(entry.transform.xyz(photo.position))
                    .map(|(x, y, _)| (x, y));
                if let (Some(from), Some(to)) = (previous, at) {
                    frame.stroke(
                        &canvas::Path::line(UiPoint::new(from.0, from.1), UiPoint::new(to.0, to.1)),
                        canvas::Stroke::default()
                            .with_color(Color { a: 0.55, ..color })
                            .with_width(1.2),
                    );
                }
                previous = at;
            }
        }
        let marks = self.photo_marks(projection);
        for mark in &marks {
            let centre = UiPoint::new(mark.x, mark.y);
            match mark.kind {
                PhotoKind::Pinhole => {
                    if let Some((tx, ty)) = mark.toward {
                        frame.stroke(
                            &canvas::Path::line(centre, UiPoint::new(tx, ty)),
                            canvas::Stroke::default().with_color(color).with_width(1.6),
                        );
                    }
                    let dot = canvas::Path::circle(centre, 3.5);
                    frame.fill(&dot, color);
                    frame.stroke(
                        &dot,
                        canvas::Stroke::default()
                            .with_color(Color::from_rgb8(15, 23, 42))
                            .with_width(1.0),
                    );
                }
                _ => {
                    let ball = canvas::Path::circle(centre, 5.0);
                    frame.fill(&ball, color);
                    frame.fill(
                        &canvas::Path::circle(UiPoint::new(mark.x - 1.5, mark.y - 1.5), 1.8),
                        Color::from_rgba8(255, 255, 255, 0.8),
                    );
                    frame.stroke(
                        &ball,
                        canvas::Stroke::default()
                            .with_color(Color::from_rgb8(15, 23, 42))
                            .with_width(1.0),
                    );
                }
            }
        }
        let points: Vec<(f32, f32)> = marks.iter().map(|mark| (mark.x, mark.y)).collect();
        let groups = mark_groups(&points);
        let texts: Vec<String> = groups
            .iter()
            .map(|(first, count)| {
                if *count == 1 {
                    marks[*first].label.clone()
                } else {
                    tr_args("{count} photos", &[("count", count)])
                }
            })
            .collect();
        let labels: Vec<(f32, f32, f32, usize)> = groups
            .iter()
            .zip(&texts)
            .map(|((first, count), content)| {
                let mark = &marks[*first];
                (
                    mark.x,
                    mark.y,
                    content.chars().count() as f32 * 6.0 + 2.0,
                    *count,
                )
            })
            .collect();
        for ((place, content), (_, _, width, _)) in label_places(&labels, &points, size)
            .into_iter()
            .zip(texts)
            .zip(&labels)
        {
            let Some(place) = place else {
                continue;
            };
            let badge = UiPoint::new(place.x - 3.0, place.y - 2.0);
            let badge_size = Size::new(width + 6.0, 17.0);
            frame.fill_rectangle(badge, badge_size, Color::from_rgba8(15, 23, 42, 0.85));
            frame.stroke_rectangle(
                badge,
                badge_size,
                canvas::Stroke::default()
                    .with_color(Color { a: 0.7, ..color })
                    .with_width(1.0),
            );
            frame.fill_text(canvas::Text {
                content,
                position: place,
                size: iced::Pixels(10.0),
                color: Color::from_rgb8(186, 230, 253),
                ..canvas::Text::default()
            });
        }
    }

    /// What is drawn over the scene while a photo is entered: the tools'
    /// marks and the name of the photo.
    pub(crate) fn draw_photo_overlay(&self, frame: &mut Frame, shown: &ShownPhoto<'_>, size: Size) {
        self.draw_drawing(frame, size);
        self.draw_faces(frame, size);
        self.draw_measure(frame, size);
        self.draw_annotations(frame, size);
        let title = shown.title();
        let width = title.chars().count() as f32 * 6.0 + 2.0;
        let position = UiPoint::new(14.0, size.height - 24.0);
        frame.fill_rectangle(
            UiPoint::new(position.x - 3.0, position.y - 2.0),
            Size::new(width + 6.0, 17.0),
            Color::from_rgba8(15, 23, 42, 0.85),
        );
        frame.fill_text(canvas::Text {
            content: title,
            position,
            size: iced::Pixels(10.0),
            color: Color::from_rgb8(186, 230, 253),
            ..canvas::Text::default()
        });
    }

    /// While a photo is entered a drag looks around and the wheel zooms; a
    /// click without a drag picks a point, as it does in the 3D view, so that
    /// measuring and picking work on the points under the photo.
    pub(crate) fn update_photo(
        &self,
        drag: &mut Option<DragState>,
        event: canvas::Event,
        bounds: iced::Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        use canvas::event::Status;
        use iced::mouse::{Button, Event, ScrollDelta};
        match event {
            canvas::Event::Mouse(Event::ButtonPressed(
                button @ (Button::Left | Button::Right | Button::Middle),
            )) => {
                let Some(position) = cursor.position_in(bounds) else {
                    return (Status::Ignored, None);
                };
                *drag = Some(DragState {
                    start: position,
                    position,
                    mode: if button == Button::Left {
                        DragMode::PlainPending
                    } else {
                        DragMode::Orbit
                    },
                });
                (Status::Captured, None)
            }
            canvas::Event::Mouse(Event::ButtonReleased(
                button @ (Button::Left | Button::Right | Button::Middle),
            )) => {
                let click = drag
                    .take()
                    .filter(|pressed| {
                        button == Button::Left && matches!(pressed.mode, DragMode::PlainPending)
                    })
                    .and_then(|_| cursor.position_from(bounds.position()))
                    .map(|position| {
                        let at = [position.x, position.y];
                        let size = bounds.size();
                        if self.measure.mode.is_some() {
                            Message::Measure(crate::measure::MeasureAction::Click(at, size))
                        } else if self.annotate.tool.is_some() {
                            Message::Views(crate::views::ViewAction::Click(at, size))
                        } else {
                            Message::ClickSelect(at, size)
                        }
                    });
                (Status::Captured, click)
            }
            canvas::Event::Mouse(Event::CursorMoved { .. }) => {
                if let Some(previous) = drag.as_mut() {
                    let Some(position) = cursor.position_from(bounds.position()) else {
                        return (Status::Captured, None);
                    };
                    if matches!(previous.mode, DragMode::PlainPending) {
                        if (position.x - previous.start.x).hypot(position.y - previous.start.y)
                            < 5.0
                        {
                            return (Status::Captured, None);
                        }
                        previous.mode = DragMode::Orbit;
                    }
                    let dx = position.x - previous.position.x;
                    let dy = position.y - previous.position.y;
                    previous.position = position;
                    (
                        Status::Captured,
                        Some(Message::Photos(PhotoAction::Look(dx, dy))),
                    )
                } else {
                    let size = bounds.size();
                    if (size.width - self.viewport_size.width).abs() > 1.0
                        || (size.height - self.viewport_size.height).abs() > 1.0
                    {
                        (Status::Ignored, Some(Message::ViewportSize(size)))
                    } else {
                        (Status::Ignored, None)
                    }
                }
            }
            canvas::Event::Mouse(Event::WheelScrolled { delta }) if cursor.is_over(bounds) => {
                let amount = match delta {
                    ScrollDelta::Lines { y, .. } => y,
                    ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                (
                    Status::Captured,
                    Some(Message::Photos(PhotoAction::Zoom(amount))),
                )
            }
            _ => (Status::Ignored, None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_api::ApiRequest;

    fn send(studio: &mut Studio, command: Value) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let command: ApiCommand = serde_json::from_value(command).unwrap();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    /// A camera that looks along +X with +Z up and +Y on its left.
    const ALONG_X: [[f64; 3]; 3] = [[0.0, -1.0, 0.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0]];
    const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    fn panorama(position: [f64; 3]) -> FilePhoto {
        FilePhoto {
            name: None,
            station: None,
            position,
            axes: IDENTITY,
            width: 64,
            height: 32,
            projection: PhotoProjection::Spherical {
                pixel_size: [std::f64::consts::TAU / 64.0, std::f64::consts::PI / 32.0],
            },
            format: ScanImageFormat::Jpeg,
            offset: 0,
            length: 1,
        }
    }

    fn pinhole(position: [f64; 3], axes: [[f64; 3]; 3]) -> FilePhoto {
        FilePhoto {
            name: Some("Corner".into()),
            station: None,
            position,
            axes,
            width: 300,
            height: 400,
            projection: PhotoProjection::Pinhole {
                focal: [250.0, 250.0],
                principal: [149.5, 199.5],
            },
            format: ScanImageFormat::Png,
            offset: 0,
            length: 1,
        }
    }

    /// A scan of points in a box of ten metres with two panoramas and a
    /// pinhole photo along a path through it.
    fn studio_with_photos() -> (Studio, tempfile::TempDir, PathBuf) {
        studio_with(vec![
            panorama([2.0, 5.0, 1.5]),
            panorama([4.0, 5.0, 1.5]),
            pinhole([6.0, 5.0, 1.5], ALONG_X),
        ])
    }

    /// A scan of points in a box of ten metres with these photos.
    fn studio_with(photos: Vec<FilePhoto>) -> (Studio, tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("path.xyz");
        let mut points = String::new();
        for x in 0..=10 {
            for y in 0..=10 {
                points.push_str(&format!("{x} {y} 0\n{x} {y} 3\n"));
            }
        }
        std::fs::write(&source, points).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&source, 1_000).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let photos = FilePhotos {
            photos,
            coordinate_system: Some("EPSG:28992".into()),
            skipped: 1,
        };
        let _ = studio.update(Message::Photos(PhotoAction::Listed(
            source.clone(),
            Ok(Arc::new(photos)),
        )));
        (studio, directory, source)
    }

    fn decoded(source: &Path, index: usize) -> Arc<DecodedPhoto> {
        Arc::new(DecodedPhoto {
            source: source.to_path_buf(),
            index,
            levels: levels(::image::RgbaImage::new(8, 4)),
            decode_time: Duration::from_millis(3),
        })
    }

    #[test]
    fn large_photos_are_decoded_at_a_jpeg_scale_that_fits() {
        assert_eq!(jpeg_reduction(8_000, 4_000, MAX_PHOTO_EDGE), 2);
        assert_eq!(jpeg_reduction(1_500, 2_000, MAX_PHOTO_EDGE), 1);
        assert_eq!(jpeg_reduction(4_096, 4_096, MAX_PHOTO_EDGE), 1);
        assert_eq!(jpeg_reduction(16_385, 100, MAX_PHOTO_EDGE), 8);
        assert_eq!(jpeg_reduction(100_000, 100, MAX_PHOTO_EDGE), 8);

        let mut source = ::image::RgbImage::new(96, 48);
        for (x, _, pixel) in source.enumerate_pixels_mut() {
            *pixel = ::image::Rgb(if x < 48 { [220, 30, 30] } else { [30, 30, 220] });
        }
        let mut jpeg = Vec::new();
        ::image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
            .encode_image(&source)
            .unwrap();
        let small = decode_pixels(&jpeg, ScanImageFormat::Jpeg, 32).unwrap();
        // A quarter: the first scale whose size fits.
        assert_eq!(small.dimensions(), (24, 12));
        assert!(small.get_pixel(3, 6)[0] > 180 && small.get_pixel(20, 6)[2] > 180);
        let full = decode_pixels(&jpeg, ScanImageFormat::Jpeg, 4_096).unwrap();
        assert_eq!(full.dimensions(), (96, 48));
        let mut png = Vec::new();
        ::image::DynamicImage::ImageRgb8(source)
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                ::image::ImageOutputFormat::Png,
            )
            .unwrap();
        // A PNG is scaled down to fit, keeping its shape.
        assert_eq!(
            decode_pixels(&png, ScanImageFormat::Png, 32)
                .unwrap()
                .dimensions(),
            (32, 16)
        );
        assert!(decode_pixels(b"not a photo", ScanImageFormat::Jpeg, 32).is_err());
    }

    #[test]
    fn every_level_halves_the_one_before_down_to_one_pixel() {
        let mut full = ::image::RgbaImage::new(7, 2);
        for (x, _, pixel) in full.enumerate_pixels_mut() {
            *pixel = ::image::Rgba([x as u8 * 40, 0, 0, 255]);
        }
        let levels = levels(full);
        let sizes: Vec<(u32, u32)> = levels
            .iter()
            .map(|level| (level.width, level.height))
            .collect();
        // The sizes the graphics device expects of a texture of 7 by 2.
        assert_eq!(sizes, [(7, 2), (3, 1), (1, 1)]);
        assert!(levels
            .iter()
            .all(|level| level.pixels.len() == (level.width * level.height * 4) as usize));
        // Each pixel is the mean of the two by two below it.
        assert_eq!(&levels[1].pixels[..4], &[20, 0, 0, 255]);
        assert_eq!(&levels[1].pixels[4..8], &[100, 0, 0, 255]);
    }

    #[test]
    fn a_photo_is_entered_shown_stepped_through_and_left() {
        let _language = i18n::TestLanguage::hold(i18n::Language::English);
        let (mut studio, _directory, source) = studio_with_photos();
        studio.yaw = 0.4;
        studio.pitch = 0.3;
        studio.zoom = 0.5;
        studio.pan = [12.0, -4.0];
        studio.view_label = i18n::key("CUSTOM");

        let entered = send(&mut studio, json!({"command": "enter_photo", "index": 0}));
        assert_eq!(entered["ok"], true, "{entered}");
        assert_eq!(entered["photo"]["kind"], "spherical");
        assert_eq!(entered["photo"]["position"], json!([2.0, 5.0, 1.5]));
        let walk = studio.walk.unwrap();
        assert_eq!(walk.eye, [2.0, 5.0, 1.5]);
        let shown = studio.shown_photo().unwrap();
        assert!(shown.decoded.is_none() && !shown.pinned);
        assert_eq!(shown.title(), "Photo 1 of 3 · panorama · loading…");
        // The photo is decoded first, and a picture waits for it.
        assert!(studio.photos.decoding.contains(&(source.clone(), 0)));
        assert_eq!(studio.photos.decoding.len(), 1);
        assert!(studio.scene_pending());

        let _ = studio.update(Message::Photos(PhotoAction::Decoded(
            source.clone(),
            0,
            Ok(decoded(&source, 0)),
        )));
        assert!(studio.shown_photo().unwrap().decoded.is_some());
        assert!(!studio.photos.waiting());
        // Then its neighbour along the path is read ahead.
        assert!(studio.photos.decoding.contains(&(source.clone(), 1)));
        let status = send(&mut studio, json!({"command": "status"}));
        let photos = &status["result"]["photos"];
        assert_eq!(photos["view"]["index"], 0);
        assert_eq!(photos["view"]["shown"], true);
        assert!(photos["view"]["shown_after_ms"].is_u64());
        assert_eq!(photos["files"][0]["photos"], 3);
        assert_eq!(photos["files"][0]["spherical"], 2);
        assert_eq!(photos["files"][0]["pinhole"], 1);
        assert_eq!(photos["files"][0]["coordinate_system"], "EPSG:28992");
        assert_eq!(status["result"]["clouds"][0]["photos"], 3);

        // The next panorama keeps the way the camera looks.
        let _ = studio.update(Message::Photos(PhotoAction::Look(40.0, 0.0)));
        let turned = studio.walk.unwrap();
        let next = send(&mut studio, json!({"command": "next_photo"}));
        assert_eq!(next["photo"]["index"], 1, "{next}");
        let walk = studio.walk.unwrap();
        assert_eq!(walk.eye, [4.0, 5.0, 1.5]);
        assert_eq!((walk.yaw, walk.pitch), (turned.yaw, turned.pitch));

        // A pinhole photo is first seen from its own camera, with the
        // photo filling the view as it fills the photo.
        let _ = studio.update(Message::Photos(PhotoAction::Step(1)));
        let shown = studio.shown_photo().unwrap();
        assert!(shown.pinned);
        assert_eq!(shown.index, 2);
        let size = Size::new(900.0, 600.0);
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let projection = studio.projection(scene, size.width, size.height);
        let placed = shown.placed();
        let fit = (900.0f64 / 300.0).min(600.0 / 400.0);
        for target in [[9.0, 5.0, 1.5], [9.0, 4.0, 2.2], [8.0, 5.6, 0.9]] {
            let direction: [f64; 3] =
                std::array::from_fn(|axis| target[axis] - placed.position[axis]);
            let pixel = placed.project(direction).unwrap();
            let (x, y, _) = projection.project_unclipped(target).unwrap();
            assert!((f64::from(x) - (450.0 + (pixel[0] - 149.5) * fit)).abs() < 1e-2);
            assert!((f64::from(y) - (300.0 + (pixel[1] - 199.5) * fit)).abs() < 1e-2);
        }
        let last = send(&mut studio, json!({"command": "next_photo"}));
        assert_eq!(last["ok"], false);
        assert_eq!(last["error"], "This is the last photo along the path");

        // Zooming magnifies the photo; looking around leaves its camera.
        let _ = studio.update(Message::Photos(PhotoAction::Zoom(5.0)));
        assert!(studio.photos.view.as_ref().unwrap().zoom > 1.0);
        let _ = studio.update(Message::Photos(PhotoAction::Look(10.0, 0.0)));
        assert!(!studio.shown_photo().unwrap().pinned);

        // How much of the photo covers the points.
        let blend = send(
            &mut studio,
            json!({"command": "photo_blend", "value": 0.25}),
        );
        assert_eq!(blend, json!({"ok": true, "blend": 0.25}));
        assert_eq!(studio.shown_photo().unwrap().blend, 0.25);
        let refused = send(&mut studio, json!({"command": "photo_blend", "value": 1.5}));
        assert_eq!(refused["ok"], false);
        assert_eq!(studio.photos.blend, 0.25);

        // Escape puts the camera back where it was before the first photo.
        let _ = studio.update(Message::Escape);
        assert!(studio.walk.is_none() && studio.photos.view.is_none());
        assert!(studio.photos.cache.is_empty());
        assert_eq!(
            (studio.yaw, studio.pitch, studio.zoom, studio.pan),
            (0.4, 0.3, 0.5, [12.0, -4.0])
        );
        assert_eq!(studio.view_label, "CUSTOM");
        // Page Down says nothing without a photo.
        studio.status = "unchanged".into();
        let _ = studio.update(Message::Photos(PhotoAction::Step(1)));
        assert_eq!(studio.status, "unchanged");
    }

    #[test]
    fn walking_on_leaves_the_photo_and_a_photo_entered_while_walking_returns_there() {
        let (mut studio, _directory, source) = studio_with_photos();
        let walked = send(
            &mut studio,
            json!({"command": "walk", "eye": [1.0, 1.0, 1.6], "yaw": 0.5, "pitch": 0.0}),
        );
        assert_eq!(walked["ok"], true);
        let walking = studio.walk.unwrap();
        let _ = studio.update(Message::Photos(PhotoAction::Enter(source.clone(), 1)));
        assert_eq!(studio.walk.unwrap().eye, [4.0, 5.0, 1.5]);
        let _ = studio.update(Message::LeaveWalk);
        assert_eq!(studio.walk, Some(walking));

        let _ = studio.update(Message::Photos(PhotoAction::Enter(source, 0)));
        assert!(studio.shown_photo().is_some());
        let _ = studio.update(Message::WalkKey(crate::WalkKey::Forward, true));
        let _ = studio.update(Message::WalkTick(Instant::now()));
        let _ = studio.update(Message::WalkTick(
            Instant::now() + Duration::from_millis(50),
        ));
        assert!(studio.photos.view.is_none());
        assert!(studio.walk.is_some());
    }

    #[test]
    fn photos_are_listed_with_their_place_and_direction_and_follow_the_layer() {
        let _language = i18n::TestLanguage::hold(i18n::Language::English);
        let (mut studio, _directory, _source) = studio_with_photos();
        let listed = send(&mut studio, json!({"command": "list_photos"}));
        assert_eq!(listed["ok"], true, "{listed}");
        assert_eq!(listed["layer"], 0);
        assert_eq!(listed["coordinate_system"], "EPSG:28992");
        assert_eq!(listed["skipped"], 1);
        let photos = listed["photos"].as_array().unwrap();
        assert_eq!(photos.len(), 3);
        assert_eq!(photos[0]["direction"], json!([1.0, 0.0, 0.0]));
        assert_eq!(photos[2]["kind"], "pinhole");
        assert_eq!(photos[2]["name"], "Corner");
        assert_eq!(photos[2]["width"], 300);
        assert_eq!(photos[2]["direction"], json!([1.0, 0.0, 0.0]));
        // A layer that is moved takes its photos along.
        studio.clouds[0].transform.offset = [100.0, 0.0, 0.0];
        let moved = send(&mut studio, json!({"command": "list_photos", "layer": 0}));
        assert_eq!(moved["photos"][0]["position"], json!([102.0, 5.0, 1.5]));
        let entered = send(&mut studio, json!({"command": "enter_photo", "index": 2}));
        assert_eq!(entered["photo"]["position"], json!([106.0, 5.0, 1.5]));
        assert_eq!(studio.walk.unwrap().eye, [106.0, 5.0, 1.5]);

        for (command, error) in [
            (
                json!({"command": "list_photos", "layer": 3}),
                "layer index is out of range",
            ),
            (
                json!({"command": "enter_photo", "index": 9}),
                "There is no photo 10: the scan has 3",
            ),
        ] {
            let answer = send(&mut studio, command);
            assert_eq!(answer, json!({"ok": false, "error": error}));
        }
        // Removing the layer forgets its photos and leaves the photo.
        let _ = studio.update(Message::Remove(0));
        assert!(studio.photos.files.is_empty() && studio.photos.view.is_none());
        let none = send(&mut studio, json!({"command": "list_photos"}));
        assert_eq!(none["error"], "no open layer has photos");
        let step = send(&mut studio, json!({"command": "previous_photo"}));
        assert_eq!(step["error"], "No photo is entered");
    }

    #[test]
    fn photo_marks_group_their_labels_and_are_found_under_the_pointer() {
        let groups = mark_groups(&[(10.0, 10.0), (30.0, 12.0), (200.0, 50.0), (12.0, 40.0)]);
        assert_eq!(groups, [(0, 3), (2, 1)]);
        let places = label_places(
            &[(100.0, 100.0, 60.0, 3), (110.0, 104.0, 60.0, 1)],
            &[(100.0, 100.0), (110.0, 104.0)],
            Size::new(400.0, 300.0),
        );
        let [Some(first), Some(second)] = places[..] else {
            panic!("{places:?}");
        };
        // Neither label covers the other.
        assert!((first.y - second.y).abs() >= 13.0 || (first.x - second.x).abs() >= 60.0);
        assert!(label_places(&[(5.0, 5.0, 500.0, 1)], &[], Size::new(100.0, 100.0))[0].is_none());

        let (studio, _directory, source) = studio_with_photos();
        let viewport = studio.point_viewport();
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let projection = Projection::new(scene, -0.8, 0.6, 1.0, [0.0, 0.0], 900.0, 600.0);
        let (x, y, _) = projection.project([4.0, 5.0, 1.5]).unwrap();
        assert_eq!(
            viewport.photo_mark_at(projection, [x + 3.0, y - 2.0]),
            Some((source, 1))
        );
        assert!(viewport.photo_mark_at(projection, [x + 40.0, y]).is_none());
        let marks = viewport.photo_marks(projection);
        assert_eq!(marks.len(), 3);
        assert!(marks[2].toward.is_some() && marks[0].toward.is_none());
    }

    #[test]
    fn from_inside_a_photo_the_photos_ahead_are_rings_to_step_into() {
        let (mut studio, _directory, source) = studio_with_photos();
        let _ = send(&mut studio, json!({"command": "enter_photo", "index": 0}));
        // Look along the path, toward the next photos.
        let view = crate::station_photos::WalkView::new(studio.walk.unwrap().eye, 0.0);
        studio.walk = Some(view);
        let viewport = studio.point_viewport();
        let scene = crate::combined_bounds(&studio.clouds).unwrap();
        let projection = viewport.projection(scene, 900.0, 600.0);
        let eye = viewport.walk_eye(view);
        let steps = viewport.photo_steps(projection, eye);
        // The photo looked through is no ring; the ones ahead are.
        assert!(!steps.is_empty());
        assert!(steps.iter().all(|mark| mark.index != 0));
        let ahead = &steps[0];
        assert_eq!(
            viewport.photo_step_at(projection, eye, [ahead.x + 2.0, ahead.y]),
            Some((source, ahead.index))
        );
        assert!(viewport
            .photo_step_at(projection, eye, [ahead.x + 40.0, ahead.y + 40.0])
            .is_none());
    }

    #[test]
    fn the_browser_lists_the_photos_of_a_scan_closed_at_first() {
        let (mut studio, _directory, source) = studio_with_photos();
        assert!(studio.photo_rows(0).is_some());
        assert!(studio.photo_rows(1).is_none());
        assert!(!studio.photos.expanded.contains(&source));
        let _ = studio.update(Message::Photos(PhotoAction::ToggleList(source.clone())));
        assert!(studio.photos.expanded.contains(&source));
        let _ = studio.update(Message::Photos(PhotoAction::ToggleList(source.clone())));
        assert!(!studio.photos.expanded.contains(&source));
        let entry = &studio.clouds[0];
        assert!(studio.photo_properties(entry).is_some());
        assert!(studio.photo_controls().is_none());
        let _ = studio.update(Message::Photos(PhotoAction::Enter(source, 0)));
        assert!(studio.photo_controls().is_some());
    }

    #[test]
    fn a_photo_that_cannot_be_decoded_is_not_asked_for_again() {
        let _language = i18n::TestLanguage::hold(i18n::Language::English);
        let (mut studio, _directory, source) = studio_with_photos();
        let _ = studio.update(Message::Photos(PhotoAction::Enter(source.clone(), 1)));
        assert!(studio.photos.waiting());
        let _ = studio.update(Message::Photos(PhotoAction::Decoded(
            source.clone(),
            1,
            Err("broken".into()),
        )));
        assert_eq!(studio.status, "Photo 2 could not be shown: broken");
        assert!(!studio.photos.waiting());
        // The scene says so, instead of waiting for it.
        let shown = studio.shown_photo().unwrap();
        assert!(shown.failed && shown.decoded.is_none());
        assert_eq!(shown.title(), "Photo 2 of 3 · panorama · cannot be shown");
        let status = send(&mut studio, json!({"command": "status"}));
        assert_eq!(status["result"]["photos"]["view"]["failed"], true);
        assert_eq!(status["result"]["photos"]["view"]["shown"], false);
        // Its neighbours are read ahead all the same, and it is not asked
        // for again.
        let decoding: HashSet<usize> = studio
            .photos
            .decoding
            .iter()
            .map(|(_, index)| *index)
            .collect();
        assert_eq!(decoding, HashSet::from([0, 2]));
        let _ = studio.update(Message::Photos(PhotoAction::Decoded(
            source.clone(),
            2,
            Ok(decoded(&source, 2)),
        )));
        assert!(studio.photos.cached(&source, 2).is_some());
        assert!(!studio.photos.decoding.contains(&(source.clone(), 1)));
        // A photo that arrives once the photos are left is let go.
        let _ = studio.update(Message::Escape);
        let _ = studio.update(Message::Photos(PhotoAction::Decoded(
            source.clone(),
            0,
            Ok(decoded(&source, 0)),
        )));
        assert!(studio.photos.cache.is_empty());
    }

    #[test]
    fn holding_page_down_decodes_a_few_photos_at_a_time_and_keeps_those_near_the_one_shown() {
        let path = (0..32)
            .map(|index| panorama([1.0 + 0.25 * f64::from(index), 5.0, 1.5]))
            .collect();
        let (mut studio, _directory, source) = studio_with(path);
        let _ = studio.update(Message::Photos(PhotoAction::Enter(source.clone(), 0)));
        // Key repeat steps on while nothing is decoded yet.
        for _ in 0..30 {
            let _ = studio.update(Message::Photos(PhotoAction::Step(1)));
            assert!(
                studio.photos.decoding.len() <= PARALLEL_DECODES + 1,
                "{:?}",
                studio.photos.decoding
            );
        }
        assert_eq!(studio.photos.view.as_ref().unwrap().index, 30);
        let decoding = |studio: &Studio| -> HashSet<usize> {
            studio
                .photos
                .decoding
                .iter()
                .map(|(_, index)| *index)
                .collect()
        };
        assert_eq!(decoding(&studio), HashSet::from([0, 1, 2]));
        // The photo shown waits for a place, and a picture waits for it.
        assert!(studio.photos.waiting());
        // The decodes under way were stepped past: those that have yet to
        // read or decode their photo stop there.
        let wanted = &studio.photos.wanted;
        assert!(!wanted.wants(&source, 2));
        assert!(wanted.wants(&source, 29) && wanted.wants(&source, 31));
        assert!(!wanted.wants(Path::new("other.e57"), 30));

        // A photo stepped past is let go when it arrives, and the photo
        // shown takes its place.
        let _ = studio.update(Message::Photos(PhotoAction::Decoded(
            source.clone(),
            0,
            Ok(decoded(&source, 0)),
        )));
        assert!(studio.photos.cache.is_empty());
        assert_eq!(decoding(&studio), HashSet::from([1, 2, 30]));
        let _ = studio.update(Message::Photos(PhotoAction::Skipped(source.clone(), 1)));
        // Its neighbours wait for it.
        assert_eq!(decoding(&studio), HashSet::from([2, 30]));
        let _ = studio.update(Message::Photos(PhotoAction::Decoded(
            source.clone(),
            30,
            Ok(decoded(&source, 30)),
        )));
        assert!(studio.shown_photo().unwrap().decoded.is_some());
        assert_eq!(decoding(&studio), HashSet::from([2, 31]));
        let _ = studio.update(Message::Photos(PhotoAction::Decoded(
            source.clone(),
            2,
            Err("stepped past".into()),
        )));
        assert_eq!(decoding(&studio), HashSet::from([29, 31]));
        let kept: Vec<usize> = studio
            .photos
            .cache
            .iter()
            .map(|photo| photo.index)
            .collect();
        assert_eq!(kept, [30]);
    }

    #[test]
    fn a_decode_stops_where_its_photo_is_no_longer_wanted() {
        let photo = panorama([0.0; 3]);
        let missing = Path::new("missing.e57");
        // Stepped past before it was read: nothing is read.
        assert!(decode_photo(missing, 0, &photo, &|| false).is_none());
        assert!(matches!(
            decode_photo(missing, 0, &photo, &|| true),
            Some(Err(_))
        ));
        let mut png = Vec::new();
        ::image::DynamicImage::ImageRgb8(::image::RgbImage::new(8, 4))
            .write_to(
                &mut std::io::Cursor::new(&mut png),
                ::image::ImageOutputFormat::Png,
            )
            .unwrap();
        let read = || Ok(png.clone());
        // Stepped past while it was read: it is not decoded.
        let asked = std::cell::Cell::new(0);
        let once = || {
            asked.set(asked.get() + 1);
            asked.get() == 1
        };
        assert!(decode_read(missing, 3, ScanImageFormat::Png, read, &once).is_none());
        assert_eq!(asked.get(), 2);
        let decoded = decode_read(missing, 3, ScanImageFormat::Png, read, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!((decoded.index, decoded.levels[0].width), (3, 8));
    }

    #[test]
    fn leaving_a_photo_entered_from_a_station_panorama_shows_that_panorama_again() {
        let (mut studio, _directory, _source) = studio_with_photos();
        let mut cloud = (*studio.clouds[0].cloud).clone();
        cloud.scan_poses = vec![pointcloud_core::ScanPose {
            label: "Station 1".into(),
            position: [8.0, 5.0, 1.5],
            axes: None,
        }];
        cloud.scan_images = vec![pointcloud_core::ScanImage {
            station: Some(0),
            position: [8.0, 5.0, 1.5],
            axes: ALONG_X,
            width: 64,
            height: 64,
            focal: [32.0, 32.0],
            principal: [31.5, 31.5],
            format: ScanImageFormat::Jpeg,
            offset: 0,
            length: 1,
        }];
        studio.clouds[0].cloud = Arc::new(cloud);
        let opened = send(
            &mut studio,
            json!({"command": "open_panorama", "index": 0, "station": 0}),
        );
        assert_eq!(opened["ok"], true, "{opened}");
        let turned = send(
            &mut studio,
            json!({"command": "set_panorama", "yaw": 0.8, "pitch": 0.1, "field_of_view": 1.2}),
        );
        assert_eq!(turned["ok"], true, "{turned}");
        let at_station = studio.walk.unwrap();

        let entered = send(&mut studio, json!({"command": "enter_photo", "index": 1}));
        assert_eq!(entered["ok"], true, "{entered}");
        assert_eq!(entered["walk"]["station"], Value::Null);
        // Esc goes back to the station, with its panorama.
        let closed = send(&mut studio, json!({"command": "close_panorama"}));
        assert_eq!(closed["ok"], true);
        assert_eq!(studio.walk, Some(at_station));
        assert_eq!(studio.walk_station, Some((0, 0)));
        let status = send(&mut studio, json!({"command": "status"}));
        assert_eq!(status["result"]["walk"]["station"]["label"], "Station 1");
        // A second time leaves the panorama.
        let _ = send(&mut studio, json!({"command": "close_panorama"}));
        assert!(studio.walk.is_none() && studio.walk_station.is_none());
    }

    #[test]
    fn the_api_refuses_in_english_and_the_status_line_in_the_language_of_the_window() {
        let _language = i18n::TestLanguage::hold(i18n::Language::Table(0));
        let (mut studio, _directory, source) = studio_with_photos();
        let answer = send(&mut studio, json!({"command": "next_photo"}));
        assert_eq!(answer["error"], "No photo is entered");
        let _ = studio.update(Message::Photos(PhotoAction::Enter(source, 7)));
        assert_eq!(studio.status, "Er is geen foto 8: de scan heeft er 3");
    }

    #[test]
    fn the_cache_keeps_the_shown_photo_and_drops_the_oldest() {
        let mut tool = PhotoTool::default();
        let source = PathBuf::from("a.e57");
        tool.view = Some(PhotoView {
            source: source.clone(),
            index: 0,
            pinned: false,
            zoom: 1.0,
            entered: Instant::now(),
            shown_after: None,
            back: ReturnCamera::of(&Studio::default()),
        });
        for index in 0..6 {
            tool.store(decoded(&source, index));
        }
        let kept: Vec<usize> = tool.cache.iter().map(|photo| photo.index).collect();
        assert_eq!(kept, [0, 3, 4, 5]);
        tool.touch(&source, 3);
        tool.store(decoded(&source, 6));
        let kept: Vec<usize> = tool.cache.iter().map(|photo| photo.index).collect();
        assert_eq!(kept, [0, 5, 3, 6]);
        assert!(decoded(&source, 0).bytes() > 8 * 4 * 4);
    }
}
