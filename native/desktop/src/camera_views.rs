//! Views saved per source scan, stored outside the scan files: the camera,
//! the section box, the colour mode and the annotations placed on the view,
//! with one snapshot image per view beside the list. Section boxes that an
//! earlier version kept under a name are taken over as views once.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pointcloud_core::{Bounds, OrientedBox};
use serde::{Deserialize, Serialize};

use crate::ColorMode;

/// A scan keeps this many views; section boxes taken over from an earlier
/// version may bring it past that, and then only saving a new view waits.
pub const MAX_VIEWS_PER_SOURCE: usize = 64;
pub const MAX_NAME_CHARS: usize = 64;
pub const MAX_ANNOTATIONS: usize = 64;
pub const MAX_NOTE_CHARS: usize = 240;

type Xyz = [f64; 3];

/// The walking camera of a view that was saved while walking.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WalkCamera {
    pub eye: Xyz,
    pub yaw: f32,
    pub pitch: f32,
    /// Horizontal field of view in radians.
    pub field_of_view: f32,
}

impl WalkCamera {
    fn valid(&self) -> bool {
        finite(self.eye)
            && self.yaw.is_finite()
            && self.yaw.abs() <= std::f32::consts::TAU
            && self.pitch.is_finite()
            && self.pitch.abs() <= 1.56
            && (crate::station_photos::MIN_FIELD_OF_VIEW..=crate::station_photos::MAX_FIELD_OF_VIEW)
                .contains(&self.field_of_view)
    }
}

/// What the orbit camera of a view was relative to: the bounds of the scene
/// and the size of the viewport when it was saved.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ViewFrame {
    pub scene_min: Xyz,
    pub scene_max: Xyz,
    pub viewport: [f32; 2],
}

impl ViewFrame {
    fn valid(&self) -> bool {
        finite(self.scene_min)
            && finite(self.scene_max)
            && (0..3).all(|axis| self.scene_min[axis] <= self.scene_max[axis])
            && self
                .viewport
                .iter()
                .all(|value| value.is_finite() && *value >= 1.0)
    }
}

/// The section box of a view, with its limits in model coordinates before
/// the box is turned, and the turn about the vertical through its centre.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SectionBox {
    pub enabled: bool,
    pub min: Xyz,
    pub max: Xyz,
    /// Degrees counter-clockwise from above; a view saved before the box
    /// could turn has none.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
}

fn is_zero(value: &f64) -> bool {
    *value == 0.0
}

impl SectionBox {
    fn valid(&self) -> bool {
        finite(self.min)
            && finite(self.max)
            && self.rotation.is_finite()
            && (0..3).all(|axis| self.min[axis] <= self.max[axis])
    }

    /// The box with its turn.
    pub fn oriented(&self) -> OrientedBox {
        OrientedBox::new(
            Bounds {
                min: self.min,
                max: self.max,
            },
            self.rotation,
        )
    }
}

/// A remark placed on a view at exact model positions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Annotation {
    /// A text at a point.
    Note {
        point: Xyz,
        text: String,
        #[serde(default)]
        guid: String,
        /// Seconds since 1970, UTC.
        #[serde(default)]
        created: u64,
    },
    /// An arrow from one point to another.
    Line { from: Xyz, to: Xyz },
}

impl Annotation {
    pub fn note(point: Xyz, text: &str) -> Self {
        Self::Note {
            point,
            text: text.to_owned(),
            guid: new_guid(),
            created: now_seconds(),
        }
    }

    fn valid(&self) -> bool {
        match self {
            Self::Note { point, text, .. } => {
                finite(*point) && !text.trim().is_empty() && text.chars().count() <= MAX_NOTE_CHARS
            }
            Self::Line { from, to } => finite(*from) && finite(*to),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedView {
    pub source: PathBuf,
    pub name: String,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub pan: [f32; 2],
    #[serde(default)]
    pub guid: String,
    /// Seconds since 1970, UTC; zero for a view saved before times were kept.
    #[serde(default)]
    pub created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub walk: Option<WalkCamera>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<ViewFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<SectionBox>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mode: Option<ColorMode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Annotation>,
    /// The view changed after its snapshot was taken, or has none yet. The
    /// snapshot is taken when the viewport next shows the view.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub snapshot_due: bool,
    /// Orbit, zoom, walk and the section box leave the view as it is while
    /// it is shown, and Update is refused; see `locks`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
}

impl SavedView {
    /// A view of the orbit camera alone, as saved before views held more.
    pub fn camera(
        source: PathBuf,
        name: &str,
        yaw: f32,
        pitch: f32,
        zoom: f32,
        pan: [f32; 2],
    ) -> Self {
        Self {
            source,
            name: name.to_owned(),
            yaw,
            pitch,
            zoom,
            pan,
            guid: new_guid(),
            created: now_seconds(),
            walk: None,
            frame: None,
            section: None,
            color_mode: None,
            annotations: Vec::new(),
            snapshot_due: false,
            locked: false,
        }
    }

    /// The section box the view switches on when it is applied; a view
    /// without one switches the box off.
    pub fn section_box(&self) -> Option<SectionBox> {
        self.section.filter(|section| section.enabled)
    }

    fn valid(&self) -> bool {
        !self.source.as_os_str().is_empty()
            && !self.name.trim().is_empty()
            && self.name.chars().count() <= MAX_NAME_CHARS
            && self.yaw.is_finite()
            && self.yaw.abs() <= std::f32::consts::TAU
            && self.pitch.is_finite()
            && self.pitch.abs() <= std::f32::consts::FRAC_PI_2
            && self.zoom.is_finite()
            && (0.000_001..=10_000.0).contains(&self.zoom)
            && self.pan.iter().all(|value| value.is_finite())
    }

    /// Drop the parts of a stored view that cannot be used, and give a view
    /// without an identifier one that is the same on every load.
    fn repaired(mut self) -> Self {
        if !is_guid(&self.guid) {
            self.guid = stable_guid(&[
                self.source.to_string_lossy().as_bytes(),
                self.name.as_bytes(),
            ]);
        }
        self.walk = self.walk.filter(WalkCamera::valid);
        self.frame = self.frame.filter(ViewFrame::valid);
        self.section = self.section.filter(SectionBox::valid);
        self.annotations.retain(Annotation::valid);
        self.annotations.truncate(MAX_ANNOTATIONS);
        let view_guid = self.guid.clone();
        for (index, annotation) in self.annotations.iter_mut().enumerate() {
            if let Annotation::Note { guid, .. } = annotation {
                if !is_guid(guid) {
                    *guid = stable_guid(&[view_guid.as_bytes(), &index.to_le_bytes()]);
                }
            }
        }
        self
    }
}

fn finite(xyz: Xyz) -> bool {
    xyz.iter().all(|value| value.is_finite())
}

pub fn new_guid() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn is_guid(value: &str) -> bool {
    value.len() == 36 && uuid::Uuid::try_parse(value).is_ok()
}

/// An identifier in the form of a random one, derived from the given bytes.
pub fn stable_guid(parts: &[&[u8]]) -> String {
    let mut bytes = [0u8; 16];
    for (half, seed) in [0xcbf2_9ce4_8422_2325u64, 0x6c62_272e_07bb_0142]
        .into_iter()
        .enumerate()
    {
        // FNV-1a; a separator keeps ("ab", "c") apart from ("a", "bc").
        let mut hash = seed;
        for part in parts {
            for byte in part.iter().chain(&[0xff]) {
                hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        bytes[half * 8..half * 8 + 8].copy_from_slice(&hash.to_be_bytes());
    }
    uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
}

pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub fn source_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The saved views, with the section boxes of an earlier version that were
/// not taken over yet added as views.
pub fn load() -> Vec<SavedView> {
    let Some(path) = config_path() else {
        return Vec::new();
    };
    let mut views = load_from(&path);
    if let (Some(sections), Some(taken)) = (sections_path(), taken_over_path()) {
        take_over_from(&path, &mut views, &sections, &taken);
    }
    views
}

pub fn save(views: &[SavedView]) -> io::Result<()> {
    let path = config_path().ok_or_else(|| io::Error::other("no user config directory"))?;
    save_to(&path, views)
}

#[cfg(test)]
thread_local! {
    static TEST_DIRECTORY: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// Keep the views and snapshots of the tests on this thread in a directory
/// of their own. Without one, tests have nowhere to store them.
#[cfg(test)]
pub fn use_test_directory(directory: &Path) {
    TEST_DIRECTORY.with(|slot| *slot.borrow_mut() = Some(directory.to_path_buf()));
}

#[cfg(test)]
pub(crate) fn directory() -> Option<PathBuf> {
    TEST_DIRECTORY.with(|slot| slot.borrow().clone())
}

/// The folder that holds the views, their snapshots and the drawings made
/// beside them.
#[cfg(not(test))]
pub(crate) fn directory() -> Option<PathBuf> {
    crate::preferences::config_directory()
}

fn config_path() -> Option<PathBuf> {
    directory().map(|directory| directory.join("camera-views.json"))
}

/// Where an earlier version kept section boxes under a name. The file is
/// read, never written.
fn sections_path() -> Option<PathBuf> {
    directory().map(|directory| directory.join("section-boxes.json"))
}

/// The identifiers of the section boxes that were taken over as views.
fn taken_over_path() -> Option<PathBuf> {
    directory().map(|directory| directory.join("section-boxes-taken-over.json"))
}

/// A section box that an earlier version kept under a name: its limits
/// before the turn, and the turn about the vertical through its centre in
/// degrees.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSection {
    /// The scan the box belongs to, as the saved views name it.
    pub source: PathBuf,
    pub name: String,
    pub min: Xyz,
    pub max: Xyz,
    #[serde(default)]
    pub rotation: f64,
}

impl SavedSection {
    fn shape(&self) -> SectionBox {
        SectionBox {
            enabled: true,
            min: self.min,
            max: self.max,
            rotation: self.rotation,
        }
    }

    /// The identifier of the view the box becomes: the same for the same
    /// box on every start.
    fn view_guid(&self) -> String {
        let numbers: Vec<u8> = self
            .min
            .iter()
            .chain(&self.max)
            .chain(std::iter::once(&self.rotation))
            .flat_map(|value| value.to_le_bytes())
            .collect();
        stable_guid(&[
            b"section box",
            self.source.to_string_lossy().as_bytes(),
            self.name.as_bytes(),
            &numbers,
        ])
    }
}

/// The camera a view starts with when it is made from a section box: from
/// the corner the window starts with, framed the way zoom all frames a
/// scene of the size of the box in a viewport of the size the window starts
/// with.
const BOX_VIEW_YAW: f32 = -0.8;
const BOX_VIEW_PITCH: f32 = 0.6;
const BOX_VIEW_VIEWPORT: [f32; 2] = [915.0, 743.0];

/// The view a named section box becomes: the box, switched on, and a camera
/// that frames it.
pub fn section_view(section: &SavedSection, name: &str) -> SavedView {
    let shape = section.shape();
    let around = shape.oriented().aabb();
    let mut view = SavedView::camera(
        section.source.clone(),
        name,
        BOX_VIEW_YAW,
        BOX_VIEW_PITCH,
        1.0,
        [0.0; 2],
    );
    view.guid = section.view_guid();
    view.frame = Some(ViewFrame {
        scene_min: around.min,
        scene_max: around.max,
        viewport: BOX_VIEW_VIEWPORT,
    });
    view.section = Some(shape);
    view.snapshot_due = true;
    view
}

/// A name for a view of a scan that no other view of that scan has, without
/// regard to case: the name itself, else the name with a number after it.
pub fn free_view_name(views: &[SavedView], source: &Path, name: &str) -> String {
    let base: String = name
        .trim()
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_NAME_CHARS - 4)
        .collect();
    let base = if base.is_empty() {
        "Section".to_owned()
    } else {
        base
    };
    let taken = |candidate: &str| {
        views
            .iter()
            .any(|view| view.source == source && view.name.eq_ignore_ascii_case(candidate))
    };
    std::iter::once(base.clone())
        .chain((2..1000).map(|number| format!("{base} {number}")))
        .find(|candidate| !taken(candidate))
        .unwrap_or(base)
}

/// Take over the section boxes that were not taken over yet, a view each,
/// named as the box unless its scan has a view of that name already. The
/// boxes in `taken` are left alone, so that a view the user deleted does
/// not come back. Returns the identifiers of the boxes taken over now,
/// including those whose view is already in the list.
pub fn take_over_sections(
    views: &mut Vec<SavedView>,
    sections: &[SavedSection],
    taken: &[String],
) -> Vec<String> {
    let mut now: Vec<String> = Vec::new();
    for section in sections {
        if !section.shape().valid() || section.source.as_os_str().is_empty() {
            continue;
        }
        let guid = section.view_guid();
        if taken.contains(&guid) || now.contains(&guid) {
            continue;
        }
        if !views.iter().any(|view| view.guid == guid) {
            let name = free_view_name(views, &section.source, &section.name);
            views.push(section_view(section, &name));
        }
        now.push(guid);
    }
    now
}

/// The section boxes of an earlier version, each read on its own.
fn load_sections_from(path: &Path) -> Vec<SavedSection> {
    match read_entries(path) {
        Entries::Read(entries) => entries
            .into_iter()
            .filter_map(|entry| serde_json::from_value(entry).ok())
            .collect(),
        Entries::Missing | Entries::Unreadable => Vec::new(),
    }
}

/// Add the section boxes that are not taken over yet to the views and store
/// both lists. When the views cannot be stored, nothing is taken over and
/// the next start tries again; the file of the boxes is never written.
fn take_over_from(views_path: &Path, views: &mut Vec<SavedView>, sections: &Path, marker: &Path) {
    let boxes = load_sections_from(sections);
    if boxes.is_empty() {
        return;
    }
    let mut taken: Vec<String> = match read_entries(marker) {
        Entries::Read(entries) => entries
            .into_iter()
            .filter_map(|entry| entry.as_str().map(str::to_owned))
            .collect(),
        Entries::Missing | Entries::Unreadable => Vec::new(),
    };
    let before = views.len();
    let now = take_over_sections(views, &boxes, &taken);
    if now.is_empty() {
        return;
    }
    if views.len() > before && save_to(views_path, views).is_err() {
        views.truncate(before);
        return;
    }
    taken.extend(now);
    let _ = write_json(marker, &taken);
}

/// Where the snapshot image of a view is kept.
pub fn snapshot_path(guid: &str) -> Option<PathBuf> {
    is_guid(guid)
        .then(directory)
        .flatten()
        .map(|directory| directory.join("view-snapshots").join(format!("{guid}.png")))
}

pub fn write_snapshot(guid: &str, png: &[u8]) -> io::Result<()> {
    let path = snapshot_path(guid).ok_or_else(|| io::Error::other("no user config directory"))?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("snapshot path has no parent"))?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    io::Write::write_all(&mut temporary, png)?;
    temporary.persist(&path).map_err(|error| error.error)?;
    Ok(())
}

pub fn has_snapshot(guid: &str) -> bool {
    snapshot_path(guid).is_some_and(|path| path.is_file())
}

pub fn read_snapshot(guid: &str) -> Option<Vec<u8>> {
    let path = snapshot_path(guid)?;
    let metadata = fs::metadata(&path).ok()?;
    if metadata.len() > 64 * 1_048_576 {
        return None;
    }
    fs::read(path).ok()
}

pub fn remove_snapshot(guid: &str) {
    if let Some(path) = snapshot_path(guid) {
        let _ = fs::remove_file(path);
    }
}

/// Where a list that could not be read is kept, so that the next save does
/// not replace the only copy of it.
fn unreadable_path(path: &Path) -> PathBuf {
    path.with_extension("unreadable.json")
}

/// What a list file held.
pub(crate) enum Entries {
    Missing,
    /// Too large, or not a JSON list.
    Unreadable,
    Read(Vec<serde_json::Value>),
}

/// The entries of a JSON list file, each to be read on its own. A byte
/// order mark that a text editor put in front is skipped.
pub(crate) fn read_entries(path: &Path) -> Entries {
    let Ok(metadata) = fs::metadata(path) else {
        return Entries::Missing;
    };
    let entries = (metadata.len() <= 8 * 1_048_576)
        .then(|| fs::read(path).ok())
        .flatten()
        .and_then(|bytes| {
            let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            serde_json::from_slice::<Vec<serde_json::Value>>(json).ok()
        });
    entries.map_or(Entries::Unreadable, Entries::Read)
}

/// Write a value as JSON in place of a file, all at once: a reader finds
/// the old file or the new one, never a part.
pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("the path has no parent"))?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value).map_err(io::Error::other)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn load_from(path: &Path) -> Vec<SavedView> {
    // Every view is read on its own, so one that this version cannot read
    // does not take the others with it.
    let entries = match read_entries(path) {
        Entries::Missing => return Vec::new(),
        Entries::Unreadable => {
            let _ = fs::copy(path, unreadable_path(path));
            return Vec::new();
        }
        Entries::Read(entries) => entries,
    };
    let mut views: Vec<SavedView> = entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<SavedView>(entry).ok())
        .filter(SavedView::valid)
        .map(SavedView::repaired)
        .collect();
    // Snapshots and exported topics are named by the identifier.
    for index in 1..views.len() {
        if views[..index]
            .iter()
            .any(|earlier| earlier.guid == views[index].guid)
        {
            views[index].guid = stable_guid(&[views[index].guid.as_bytes(), &index.to_le_bytes()]);
        }
    }
    views
}

fn save_to(path: &Path, views: &[SavedView]) -> io::Result<()> {
    write_json(path, &views)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_views_per_source_and_ignores_invalid_entries() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("scan.pcd");
        fs::write(&source, b"scan").unwrap();
        let path = directory.path().join("settings/camera-views.json");
        let view = SavedView::camera(
            source_key(&source),
            "Entrance",
            -0.8,
            0.6,
            3.0,
            [12.0, -4.0],
        );
        save_to(&path, std::slice::from_ref(&view)).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded, vec![view.clone()]);

        fs::write(
            &path,
            serde_json::to_vec(&vec![
                view,
                SavedView::camera(source_key(&source), "Broken", 0.0, 0.0, 0.0, [0.0, 0.0]),
            ])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(load_from(&path).len(), 1);
    }

    #[test]
    fn a_list_saved_before_views_held_more_than_the_camera_still_loads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("camera-views.json");
        fs::write(
            &path,
            r#"[
              {"source": "C:/scans/hall.e57", "name": "Entrance", "yaw": -0.8, "pitch": 0.6, "zoom": 3.0, "pan": [12.0, -4.0]},
              {"source": "C:/scans/hall.e57", "name": "Roof", "yaw": 0.5, "pitch": 1.2, "zoom": 0.4, "pan": [0.0, 0.0]},
              {"source": "C:/scans/hall.e57", "name": "Entrance", "yaw": 0.0, "pitch": 0.0, "zoom": 1.0, "pan": [0.0, 0.0]}
            ]"#,
        )
        .unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.len(), 3);
        let entrance = &loaded[0];
        assert_eq!(entrance.name, "Entrance");
        assert_eq!(
            (entrance.yaw, entrance.pitch, entrance.zoom),
            (-0.8, 0.6, 3.0)
        );
        assert_eq!(entrance.pan, [12.0, -4.0]);
        assert_eq!(entrance.created, 0);
        assert!(entrance.walk.is_none() && entrance.frame.is_none());
        assert!(entrance.section.is_none() && entrance.color_mode.is_none());
        assert!(entrance.annotations.is_empty());
        // Every view has its own identifier, the same on every load.
        assert!(loaded.iter().all(|view| is_guid(&view.guid)));
        assert_ne!(loaded[0].guid, loaded[1].guid);
        assert_ne!(loaded[0].guid, loaded[2].guid);
        let again = load_from(&path);
        assert_eq!(again, loaded);

        // Saved again, the list holds the new fields and still loads.
        save_to(&path, &loaded).unwrap();
        assert_eq!(load_from(&path), loaded);
    }

    #[test]
    fn a_full_view_round_trips_and_unusable_parts_are_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("camera-views.json");
        let mut view = SavedView::camera(
            PathBuf::from("scan.e57"),
            "Stairs",
            0.3,
            -0.2,
            0.5,
            [3.0, 4.0],
        );
        view.walk = Some(WalkCamera {
            eye: [1.0, 2.0, 1.6],
            yaw: 0.7,
            pitch: -0.1,
            field_of_view: 1.5,
        });
        view.frame = Some(ViewFrame {
            scene_min: [0.0; 3],
            scene_max: [10.0, 8.0, 3.0],
            viewport: [915.0, 743.0],
        });
        view.section = Some(SectionBox {
            enabled: true,
            min: [1.0, 1.0, 0.0],
            max: [4.0, 5.0, 2.5],
            rotation: 0.0,
        });
        view.color_mode = Some(ColorMode::Intensity);
        view.annotations = vec![
            Annotation::note([2.0, 2.0, 1.0], "Crack in the wall <3 mm> & \"damp\""),
            Annotation::Line {
                from: [2.0, 2.0, 1.0],
                to: [3.0, 2.0, 1.5],
            },
        ];
        save_to(&path, std::slice::from_ref(&view)).unwrap();
        assert_eq!(load_from(&path), vec![view.clone()]);

        let mut stored = serde_json::to_value(vec![view]).unwrap();
        stored[0]["walk"]["field_of_view"] = serde_json::json!(9.0);
        stored[0]["section"]["min"] = serde_json::json!([5.0, 1.0, 0.0]);
        stored[0]["frame"]["viewport"] = serde_json::json!([0.0, 743.0]);
        stored[0]["annotations"][0]["text"] = serde_json::json!("  ");
        stored[0]["guid"] = serde_json::json!("not an identifier");
        fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        let repaired = load_from(&path);
        assert_eq!(repaired.len(), 1);
        assert!(repaired[0].walk.is_none());
        assert!(repaired[0].section.is_none());
        assert!(repaired[0].frame.is_none());
        assert_eq!(repaired[0].annotations.len(), 1);
        assert!(matches!(
            repaired[0].annotations[0],
            Annotation::Line { .. }
        ));
        assert!(is_guid(&repaired[0].guid));
        assert_eq!(repaired[0].color_mode, Some(ColorMode::Intensity));
    }

    #[test]
    fn the_turn_of_a_section_box_is_kept_and_an_old_view_has_none() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("camera-views.json");
        let mut view = SavedView::camera(
            PathBuf::from("scan.e57"),
            "Along the wall",
            0.1,
            0.2,
            1.0,
            [0.0; 2],
        );
        let section = SectionBox {
            enabled: true,
            min: [1.0, 1.0, 0.0],
            max: [4.0, 5.0, 2.5],
            rotation: 30.0,
        };
        view.section = Some(section);
        save_to(&path, std::slice::from_ref(&view)).unwrap();
        assert_eq!(load_from(&path)[0].section, Some(section));
        assert_eq!(section.oriented().rotation_degrees, 30.0);

        // A box without a turn is written as before, without the field, and
        // a view written before boxes could turn reads as not turned.
        let mut stored = serde_json::to_value(vec![view]).unwrap();
        stored[0]["section"]
            .as_object_mut()
            .unwrap()
            .remove("rotation");
        fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        let old = load_from(&path);
        assert_eq!(old[0].section.unwrap().rotation, 0.0);
        let written = serde_json::to_value(&old).unwrap();
        assert!(written[0]["section"].get("rotation").is_none());
        // A turn that is no number makes the box unusable.
        stored[0]["section"]["rotation"] = serde_json::json!("north");
        fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        assert!(load_from(&path)
            .first()
            .is_none_or(|view| view.section.is_none()));
    }

    #[test]
    fn a_view_that_cannot_be_read_does_not_take_the_others_with_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("camera-views.json");
        let view = |name: &str, extra: &str| {
            format!(
                r#"{{"source": "scan.e57", "name": "{name}", "yaw": 0.1, "pitch": 0.2, "zoom": 1.0, "pan": [0.0, 0.0]{extra}}}"#
            )
        };
        fs::write(
            &path,
            format!(
                "[{}, {}, {}, {}, 7]",
                view("Hall", ""),
                view(
                    "Later colours",
                    r#", "color_mode": "a mode of a later version""#
                ),
                view(
                    "Later remark",
                    r#", "annotations": [{"kind": "cloud", "points": []}]"#
                ),
                view("Roof", r#", "snapshot_due": true"#),
            ),
        )
        .unwrap();
        let loaded = load_from(&path);
        let names: Vec<&str> = loaded.iter().map(|view| view.name.as_str()).collect();
        assert_eq!(names, ["Hall", "Roof"]);
        assert!(!loaded[0].snapshot_due && loaded[1].snapshot_due);
        assert!(!unreadable_path(&path).exists());
        // The flag is written only while a snapshot is due.
        save_to(&path, &loaded).unwrap();
        let stored = fs::read_to_string(&path).unwrap();
        assert_eq!(stored.matches("snapshot_due").count(), 1);
        assert_eq!(load_from(&path), loaded);

        // A list that cannot be read at all is kept beside the list, so the
        // next save does not replace the only copy.
        let cut = &stored[..stored.len() / 2];
        fs::write(&path, cut).unwrap();
        assert!(load_from(&path).is_empty());
        assert_eq!(fs::read_to_string(unreadable_path(&path)).unwrap(), cut);
        assert_eq!(
            unreadable_path(&path).file_name().unwrap(),
            "camera-views.unreadable.json"
        );
        // A list that is not there is neither read nor copied.
        let missing = directory.path().join("none/camera-views.json");
        assert!(load_from(&missing).is_empty());
        assert!(!unreadable_path(&missing).exists());
    }

    #[test]
    fn snapshots_are_stored_by_identifier_and_removed() {
        let directory = tempfile::tempdir().unwrap();
        assert!(snapshot_path(&new_guid()).is_none());
        use_test_directory(directory.path());
        let guid = new_guid();
        assert!(read_snapshot(&guid).is_none() && !has_snapshot(&guid));
        write_snapshot(&guid, b"image").unwrap();
        assert!(has_snapshot(&guid));
        assert_eq!(read_snapshot(&guid).as_deref(), Some(&b"image"[..]));
        assert!(directory
            .path()
            .join("view-snapshots")
            .join(format!("{guid}.png"))
            .is_file());
        remove_snapshot(&guid);
        assert!(read_snapshot(&guid).is_none());
        // Removing one that is not there is not an error.
        remove_snapshot(&guid);
        // A text that is no identifier names no file.
        assert!(snapshot_path("../outside").is_none());
        assert!(write_snapshot("../outside", b"image").is_err());
    }

    fn named_box(source: &str, name: &str, rotation: f64) -> SavedSection {
        SavedSection {
            source: PathBuf::from(source),
            name: name.into(),
            min: [1.0, 2.0, 0.0],
            max: [6.0, 5.0, 2.5],
            rotation,
        }
    }

    #[test]
    fn named_section_boxes_become_views_that_frame_them() {
        let mut views = vec![SavedView::camera(
            PathBuf::from("C:/scans/hall.e57"),
            "Hall",
            0.1,
            0.2,
            1.0,
            [0.0; 2],
        )];
        let boxes = [
            named_box("C:/scans/hall.e57", "Hall", 30.0),
            named_box("C:/scans/hall.e57", "Stairs", 0.0),
            named_box("C:/scans/annex.e57", "Hall", 0.0),
            // A box that is no box is left out, and an exact copy is one.
            SavedSection {
                min: [9.0, 0.0, 0.0],
                ..named_box("C:/scans/hall.e57", "Broken", 0.0)
            },
            named_box("C:/scans/hall.e57", "Stairs", 0.0),
        ];
        let taken = take_over_sections(&mut views, &boxes, &[]);
        assert_eq!(taken.len(), 3);
        let names: Vec<(&str, &str)> = views
            .iter()
            .map(|view| (view.source.to_str().unwrap(), view.name.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("C:/scans/hall.e57", "Hall"),
                ("C:/scans/hall.e57", "Hall 2"),
                ("C:/scans/hall.e57", "Stairs"),
                ("C:/scans/annex.e57", "Hall"),
            ]
        );
        // Each view switches its box on, turned as it was, and frames it.
        let turned = &views[1];
        assert_eq!(
            turned.section,
            Some(SectionBox {
                enabled: true,
                min: [1.0, 2.0, 0.0],
                max: [6.0, 5.0, 2.5],
                rotation: 30.0,
            })
        );
        let around = turned.section.unwrap().oriented().aabb();
        let frame = turned.frame.unwrap();
        assert_eq!((frame.scene_min, frame.scene_max), (around.min, around.max));
        assert_eq!((turned.zoom, turned.pan), (1.0, [0.0; 2]));
        assert!(turned.snapshot_due && turned.valid());

        // Taken over once: neither again, nor after its view was deleted.
        views.remove(1);
        assert!(take_over_sections(&mut views, &boxes, &taken).is_empty());
        assert_eq!(views.len(), 3);
    }

    #[test]
    fn saved_section_boxes_are_taken_over_once_and_their_file_stays() {
        let directory = tempfile::tempdir().unwrap();
        let views_path = directory.path().join("camera-views.json");
        let sections = directory.path().join("section-boxes.json");
        let marker = directory.path().join("section-boxes-taken-over.json");
        let earlier = SavedView::camera(
            PathBuf::from("C:/scans/hall.e57"),
            "Entrance",
            0.1,
            0.2,
            1.0,
            [0.0; 2],
        );
        save_to(&views_path, std::slice::from_ref(&earlier)).unwrap();
        // As an earlier version wrote it, with a byte order mark in front.
        let mut written = b"\xEF\xBB\xBF".to_vec();
        written.extend(
            serde_json::to_vec(&[
                named_box("C:/scans/hall.e57", "Hall", 15.0),
                named_box("C:/scans/hall.e57", "Entrance", 0.0),
            ])
            .unwrap(),
        );
        fs::write(&sections, &written).unwrap();

        let mut views = load_from(&views_path);
        take_over_from(&views_path, &mut views, &sections, &marker);
        let names: Vec<&str> = views.iter().map(|view| view.name.as_str()).collect();
        assert_eq!(names, ["Entrance", "Hall", "Entrance 2"]);
        assert_eq!(load_from(&views_path), views, "the views are stored");
        assert_eq!(fs::read(&sections).unwrap(), written, "the boxes stay");

        // Without its record of what was taken over, a view that is still
        // there is not made twice.
        fs::remove_file(&marker).unwrap();
        let mut once = load_from(&views_path);
        take_over_from(&views_path, &mut once, &sections, &marker);
        assert_eq!(once, views);
        assert!(marker.is_file());

        // The next start leaves the views as they are, also when one of the
        // views made from a box was deleted.
        views.retain(|view| view.name != "Hall");
        save_to(&views_path, &views).unwrap();
        let mut again = load_from(&views_path);
        take_over_from(&views_path, &mut again, &sections, &marker);
        assert_eq!(again, views);

        // No file of boxes, nothing to take over.
        let mut none = load_from(&views_path);
        take_over_from(
            &views_path,
            &mut none,
            &directory.path().join("missing.json"),
            &marker,
        );
        assert_eq!(none, views);
    }

    #[test]
    fn derived_identifiers_are_stable_and_distinct() {
        let first = stable_guid(&[b"ab", b"c"]);
        assert_eq!(first, stable_guid(&[b"ab", b"c"]));
        assert_ne!(first, stable_guid(&[b"a", b"bc"]));
        assert!(is_guid(&first));
        assert!(!is_guid("6f9619ff"));
    }
}
