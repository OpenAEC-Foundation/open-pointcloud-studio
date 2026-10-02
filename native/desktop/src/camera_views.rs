//! Views saved per source scan, stored outside the scan files: the camera,
//! the section box, the colour mode and the annotations placed on the view,
//! with one snapshot image per view beside the list.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::ColorMode;

pub const MAX_VIEWS_PER_SOURCE: usize = 32;
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

/// The section box of a view, with its limits in model coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SectionBox {
    pub enabled: bool,
    pub min: Xyz,
    pub max: Xyz,
}

impl SectionBox {
    fn valid(&self) -> bool {
        finite(self.min) && finite(self.max) && (0..3).all(|axis| self.min[axis] <= self.max[axis])
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
        }
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

pub fn load() -> Vec<SavedView> {
    config_path().map_or_else(Vec::new, |path| load_from(&path))
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
fn directory() -> Option<PathBuf> {
    TEST_DIRECTORY.with(|slot| slot.borrow().clone())
}

#[cfg(not(test))]
fn directory() -> Option<PathBuf> {
    crate::preferences::config_directory()
}

fn config_path() -> Option<PathBuf> {
    directory().map(|directory| directory.join("camera-views.json"))
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

fn load_from(path: &Path) -> Vec<SavedView> {
    let Ok(metadata) = fs::metadata(path) else {
        return Vec::new();
    };
    // Every view is read on its own, so one that this version cannot read
    // does not take the others with it.
    let entries = (metadata.len() <= 8 * 1_048_576)
        .then(|| fs::read(path).ok())
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok());
    let Some(entries) = entries else {
        let _ = fs::copy(path, unreadable_path(path));
        return Vec::new();
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
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("camera view path has no parent"))?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), views).map_err(io::Error::other)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
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

    #[test]
    fn derived_identifiers_are_stable_and_distinct() {
        let first = stable_guid(&[b"ab", b"c"]);
        assert_eq!(first, stable_guid(&[b"ab", b"c"]));
        assert_ne!(first, stable_guid(&[b"a", b"bc"]));
        assert!(is_guid(&first));
        assert!(!is_guid("6f9619ff"));
    }
}
