//! The project of Pointcloud to Drawing: a folder the user chooses, by default
//! `Documents\OPS Pointcloud to Drawing\<name>`, with `project.ops-m2p.json` in it
//! and what the steps write beside it. The file keeps the scans it was made
//! from, the frame of the building, its datum, its boxes, the settings, the
//! state of every step with the basis it was computed on, what the survey
//! found and the levels. It is written whole to a temporary file that then
//! takes the place of the old one, so that a crash never leaves half a file.
//!
//! The basis of a step is a stable hash of what it was computed from: the
//! scans with their size, time of change, transform, deleted points and
//! hidden classes, and what the user fixed for it. When a scan changes, the
//! basis does as well, and the step is out of date.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pointcloud_core::plans::{BuildingFrame, Level, PlanRegion, SurveyGrid, SurveyStats};
use pointcloud_core::stable_hash::{hash_hex, StableHasher};
use pointcloud_core::{Bounds, OrientedBox, Point};
use serde::{Deserialize, Serialize};

use super::{StepStatus, WizardStep};
use crate::selection::ClassFilter;
use crate::CloudEntry;

/// The name of the format of a project file, and its version.
pub(crate) const FORMAT: &str = "open-pointcloud-studio-mesh-to-plans";
pub(crate) const VERSION: u32 = 1;
/// The file of a project in its folder.
pub(crate) const FILE_NAME: &str = "project.ops-m2p.json";
/// The folder in Documents that holds the projects by default.
pub(crate) const PROJECTS_FOLDER: &str = "OPS Pointcloud to Drawing";
/// How many projects the list of recent ones keeps.
pub(crate) const MAX_RECENT: usize = 8;
/// A larger file is no project.
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Changes when the survey or the levels are computed otherwise, so that
/// what an earlier version computed counts as out of date.
const PREPARE_VERSION: &str = "prepare-1";

/// A box of the scene, turned or not, as a file keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct BoxRecord {
    pub(crate) min: [f64; 3],
    pub(crate) max: [f64; 3],
    pub(crate) rotation_deg: f64,
}

impl From<OrientedBox> for BoxRecord {
    fn from(shape: OrientedBox) -> Self {
        Self {
            min: shape.bounds.min,
            max: shape.bounds.max,
            rotation_deg: shape.rotation_degrees,
        }
    }
}

impl BoxRecord {
    pub(crate) fn shape(self) -> OrientedBox {
        OrientedBox::new(
            Bounds {
                min: self.min,
                max: self.max,
            },
            self.rotation_deg,
        )
    }
}

/// A scan the project was made from, with what tells whether it changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceRef {
    /// The path the scan was opened by.
    pub(crate) path: PathBuf,
    pub(crate) file_len: u64,
    /// Seconds since 1970 of its last change.
    pub(crate) mtime: u64,
    pub(crate) points: u64,
    /// Where its points go in the scene.
    pub(crate) scale: [f64; 3],
    pub(crate) offset: [f64; 3],
    pub(crate) deleted_points: u64,
    /// See `DeletionMask::digest`, as 16 hexadecimal digits.
    pub(crate) deletions_digest: String,
    /// The classes that are not read.
    pub(crate) hidden_classes: Vec<u8>,
}

impl SourceRef {
    /// A layer of the window as it stands, with the classes `filter` hides.
    pub(crate) fn of(entry: &CloudEntry, filter: &ClassFilter) -> Self {
        let path = entry.cloud.path.clone();
        let metadata = fs::metadata(&path).ok();
        let mtime = metadata
            .as_ref()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());
        // A mask from which every deletion was undone deletes nothing.
        let deleted = entry.deleted.as_ref().filter(|mask| mask.count > 0);
        Self {
            path,
            file_len: metadata.map_or(0, |metadata| metadata.len()),
            mtime,
            points: entry.cloud.total_points,
            scale: entry.transform.scale,
            offset: entry.transform.offset,
            deleted_points: deleted.map_or(0, |mask| mask.count),
            deletions_digest: format!("{:016x}", deleted.map_or(0, |mask| mask.digest())),
            hidden_classes: hidden_classes(filter),
        }
    }

    fn hash_into(&self, hasher: &mut StableHasher) {
        hasher
            .str(&path_key(&self.path))
            .u64(self.file_len)
            .u64(self.mtime)
            .u64(self.points)
            .f64s(&self.scale)
            .f64s(&self.offset)
            .u64(self.deleted_points)
            .str(&self.deletions_digest)
            .bytes(&self.hidden_classes);
    }
}

/// The classes a filter does not read.
pub(crate) fn hidden_classes(filter: &ClassFilter) -> Vec<u8> {
    (0..=u8::MAX)
        .filter(|code| {
            !filter.accepts(&Point {
                xyz: [0.0; 3],
                rgb: None,
                intensity: None,
                classification: Some(*code),
            })
        })
        .collect()
}

/// A path as one text whatever separators it was typed with, and on Windows,
/// whose file names do not tell capitals, in small letters: the same file
/// opened through the file dialog or through the local API has one.
pub(crate) fn path_key(path: &Path) -> String {
    use std::path::Component;
    let mut key = String::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::RootDir => key.push('/'),
            Component::Prefix(prefix) => key.push_str(&prefix.as_os_str().to_string_lossy()),
            other => {
                if !key.is_empty() && !key.ends_with('/') {
                    key.push('/');
                }
                key.push_str(&other.as_os_str().to_string_lossy());
            }
        }
    }
    if cfg!(windows) {
        key.to_lowercase()
    } else {
        key
    }
}

/// Whether two paths name the same file as `path_key` tells it.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    a == b || path_key(a) == path_key(b)
}

/// Where the building lies in the world, as far as the user knows it.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct Datum {
    /// The NAP height of P, in metres.
    pub(crate) nap_offset: Option<f64>,
    /// The direction of north, in degrees counter-clockwise from the scene
    /// Y axis as seen from above.
    pub(crate) north_deg: Option<f64>,
}

/// The boxes of the building: the part of the scene that was surveyed, the
/// box around the building and the box around the whole site, and what the
/// user fixed for the next survey.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub(crate) struct Regions {
    pub(crate) core: Option<BoxRecord>,
    pub(crate) building: Option<BoxRecord>,
    pub(crate) site: Option<BoxRecord>,
    /// The core the user chose; none for the box around the scans.
    pub(crate) chosen_core: Option<BoxRecord>,
    /// The main direction the user fixed; none to find it.
    pub(crate) chosen_rotation: Option<f64>,
}

/// The settings of the plans: the owner's choices as defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Settings {
    /// The cut of a plan above its floor, in metres.
    pub(crate) cut_height_default: f64,
    /// 100 for 1:100.
    pub(crate) plan_scale: u32,
    pub(crate) paper: String,
    /// The bands of confidence: sure from, and to be checked from.
    pub(crate) sure: f32,
    pub(crate) check: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            cut_height_default: 1.2,
            plan_scale: 100,
            paper: "A3".into(),
            sure: pointcloud_core::plans::SURE,
            check: pointcloud_core::plans::CHECK,
        }
    }
}

/// Where a step stands in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct StepRecord {
    /// As `StepStatus::key` names it.
    pub(crate) status: String,
    /// Why it failed.
    #[serde(default)]
    pub(crate) reason: Option<String>,
    /// The basis it was computed on, as 32 hexadecimal digits.
    #[serde(default)]
    pub(crate) basis: Option<String>,
    /// When it ended, in seconds since 1970, and how long it took.
    #[serde(default)]
    pub(crate) finished: Option<u64>,
    #[serde(default)]
    pub(crate) seconds: Option<f64>,
}

/// What is kept of the survey: enough to show its histogram and footprint
/// again without reading the scans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SurveyRecord {
    pub(crate) grid: SurveyGrid,
    /// The frame the survey was read in.
    pub(crate) frame: BuildingFrame,
    /// Columns of the footprint, without the walls, occupied per height of
    /// the grid: the histogram of step 0.
    pub(crate) level_histogram: Vec<u32>,
    pub(crate) footprint: Vec<PlanRegion>,
    pub(crate) footprint_area: f64,
    pub(crate) ground_z: Option<f64>,
    /// The scene height below which points are strays, the stray points
    /// below it and the clusters among them.
    #[serde(default)]
    pub(crate) below_z: Option<f64>,
    pub(crate) below_points: u64,
    #[serde(default)]
    pub(crate) below_clusters: u32,
    pub(crate) stats: SurveyStats,
    pub(crate) seconds: f64,
}

/// The project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct MeshToPlansProject {
    pub(crate) format: String,
    pub(crate) version: u32,
    /// Made once, when the project is created.
    pub(crate) id: String,
    pub(crate) name: String,
    /// Seconds since 1970.
    pub(crate) created: u64,
    pub(crate) modified: u64,
    pub(crate) sources: Vec<SourceRef>,
    #[serde(default)]
    pub(crate) frame: Option<BuildingFrame>,
    #[serde(default)]
    pub(crate) datum: Datum,
    #[serde(default)]
    pub(crate) regions: Regions,
    #[serde(default)]
    pub(crate) settings: Settings,
    /// Per step id.
    #[serde(default)]
    pub(crate) steps: BTreeMap<String, StepRecord>,
    #[serde(default)]
    pub(crate) survey: Option<SurveyRecord>,
    #[serde(default)]
    pub(crate) levels: Vec<Level>,
}

impl MeshToPlansProject {
    /// A new project with a new id.
    pub(crate) fn new(name: &str) -> Self {
        let now = now_seconds();
        Self {
            format: FORMAT.into(),
            version: VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            created: now,
            modified: now,
            sources: Vec::new(),
            frame: None,
            datum: Datum::default(),
            regions: Regions::default(),
            settings: Settings::default(),
            steps: BTreeMap::new(),
            survey: None,
            levels: Vec::new(),
        }
    }

    /// The status of a step as the file keeps it. A result without the
    /// basis it was computed on is no result: it was made by a version that
    /// did not compute it, or stood in for the work.
    pub(crate) fn status(&self, step: WizardStep) -> StepStatus {
        self.steps
            .get(step.id())
            .map_or(StepStatus::NotRun, |record| {
                match StepStatus::from_key(&record.status, record.reason.as_deref()) {
                    StepStatus::Done | StepStatus::Confirmed | StepStatus::Stale
                        if record.basis.is_none() =>
                    {
                        StepStatus::NotRun
                    }
                    status => status,
                }
            })
    }

    /// The step to go on with: the first that is neither confirmed nor
    /// skipped, or the last one.
    pub(crate) fn resume_step(&self) -> WizardStep {
        WizardStep::ALL
            .into_iter()
            .find(|step| {
                !matches!(
                    self.status(*step),
                    StepStatus::Confirmed | StepStatus::Skipped
                )
            })
            .unwrap_or(WizardStep::Result)
    }
}

/// Seconds since 1970.
pub(crate) fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// The folder that holds the projects by default: `OPS Pointcloud to Drawing` in the
/// Documents folder of the user. The tests give one of their own, or none.
pub(crate) fn default_root() -> Option<PathBuf> {
    if cfg!(test) {
        return TEST_ROOT.with(|root| root.borrow().clone());
    }
    Some(documents_folder()?.join(PROJECTS_FOLDER))
}

thread_local! {
    /// The folder of the projects of a test, which runs on a thread of its
    /// own: the Documents folder of the user stays out of the tests.
    pub(crate) static TEST_ROOT: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

/// The Documents folder of the user: where Windows keeps it, also when it
/// was moved elsewhere, and `Documents` in the home folder
/// otherwise.
fn documents_folder() -> Option<PathBuf> {
    #[cfg(windows)]
    if let Some(folder) = known_documents_folder() {
        return Some(folder);
    }
    let set = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let home = if cfg!(windows) {
        set("USERPROFILE")
    } else {
        set("HOME")
    }?;
    Some(home.join("Documents"))
}

/// The Documents folder as Windows knows it, as Explorer shows it.
#[cfg(windows)]
fn known_documents_folder() -> Option<PathBuf> {
    use std::ffi::{c_void, OsString};
    use std::os::windows::ffi::OsStringExt;

    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }
    /// FOLDERID_Documents, {FDD39AD0-238F-46AF-ADB4-6C85480369C7}.
    const DOCUMENTS: Guid = Guid {
        data1: 0xFDD3_9AD0,
        data2: 0x238F,
        data3: 0x46AF,
        data4: [0xAD, 0xB4, 0x6C, 0x85, 0x48, 0x03, 0x69, 0xC7],
    };
    #[link(name = "shell32")]
    extern "system" {
        fn SHGetKnownFolderPath(
            id: *const Guid,
            flags: u32,
            token: *mut c_void,
            path: *mut *mut u16,
        ) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoTaskMemFree(memory: *mut c_void);
    }
    let mut path: *mut u16 = std::ptr::null_mut();
    // SAFETY: the id is a GUID that lives through the call, no token asks for
    // the folder of the user that runs the process, and `path` receives a
    // string the shell allocates.
    let result = unsafe { SHGetKnownFolderPath(&DOCUMENTS, 0, std::ptr::null_mut(), &mut path) };
    let folder = (result >= 0 && !path.is_null()).then(|| {
        let mut length = 0;
        // SAFETY: the shell returned a string that ends in a zero.
        while unsafe { *path.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` characters before that zero were just read.
        OsString::from_wide(unsafe { std::slice::from_raw_parts(path, length) })
    });
    // SAFETY: the string was allocated by the shell, and freeing nothing does
    // nothing.
    unsafe { CoTaskMemFree(path.cast()) };
    folder
        .map(PathBuf::from)
        .filter(|folder| folder.is_absolute())
}

/// A name as a folder can be called: without the characters a file name
/// cannot hold, and not empty.
pub(crate) fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() || r#"<>:"/\|?*"#.contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|character| character == '.' || character == ' ');
    if cleaned.is_empty() {
        "Project".into()
    } else {
        cleaned.chars().take(80).collect()
    }
}

/// The file of the project in a folder.
pub(crate) fn project_file(folder: &Path) -> PathBuf {
    folder.join(FILE_NAME)
}

/// The folder for a new project of this name in `root`: the one named after
/// it, or, when a project is there already, the first of `<name> 2`,
/// `<name> 3` and so on without one.
pub(crate) fn free_folder(root: &Path, name: &str) -> PathBuf {
    let base = folder_name(name);
    (1..1000)
        .map(|number| {
            root.join(if number == 1 {
                base.clone()
            } else {
                format!("{base} {number}")
            })
        })
        .find(|folder| !project_file(folder).exists())
        .unwrap_or_else(|| root.join(base))
}

/// Write a project whole, through a temporary file in its folder.
pub(crate) fn save(path: &Path, project: &MeshToPlansProject) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("the project file has no folder"))?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), project).map_err(io::Error::other)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Read a project file, refusing what is not one or is too new.
pub(crate) fn load(path: &Path) -> Result<MeshToPlansProject, String> {
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err("the file is too large for a project".into());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let value: serde_json::Value =
        serde_json::from_slice(json).map_err(|error| error.to_string())?;
    if value["format"] != FORMAT {
        return Err("the file is no Pointcloud to Drawing project".into());
    }
    if value["version"]
        .as_u64()
        .is_none_or(|version| version > u64::from(VERSION))
    {
        return Err("the project was made by a newer version".into());
    }
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// The basis of step 0: the scans as they are and what the user fixed for
/// the survey.
pub(crate) fn prepare_basis(sources: &[SourceRef], regions: &Regions) -> u128 {
    let mut hasher = StableHasher::new();
    hasher.str(PREPARE_VERSION).u64(sources.len() as u64);
    for source in sources {
        source.hash_into(&mut hasher);
    }
    hasher.option(regions.chosen_core, |hasher, core| {
        hasher
            .f64s(&core.min)
            .f64s(&core.max)
            .f64(core.rotation_deg);
    });
    hasher.option(regions.chosen_rotation, |hasher, rotation| {
        hasher.f64(rotation);
    });
    hasher.finish()
}

/// A basis as the file keeps it.
pub(crate) fn basis_text(basis: u128) -> String {
    hash_hex(basis)
}

/// A project in the list of recent ones, as far as the Project Browser
/// needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RecentProject {
    pub(crate) file: PathBuf,
    pub(crate) name: String,
    /// The paths of its scans.
    pub(crate) sources: Vec<PathBuf>,
    /// The step to go on with.
    pub(crate) step: WizardStep,
}

impl RecentProject {
    pub(crate) fn of(file: &Path, project: &MeshToPlansProject) -> Self {
        Self {
            file: file.to_path_buf(),
            name: project.name.clone(),
            sources: project
                .sources
                .iter()
                .map(|source| source.path.clone())
                .collect(),
            step: project.resume_step(),
        }
    }
}

/// The recent projects that can still be read, newest first.
pub(crate) fn read_recent(files: &[PathBuf]) -> Vec<RecentProject> {
    files
        .iter()
        .filter_map(|file| {
            load(file)
                .ok()
                .map(|project| RecentProject::of(file, &project))
        })
        .collect()
}

/// The list of recent project files with `file` first, without doubles and
/// no longer than `MAX_RECENT`.
pub(crate) fn remember(recent: &mut Vec<PathBuf>, file: &Path) {
    recent.retain(|known| known != file);
    recent.insert(0, file.to_path_buf());
    recent.truncate(MAX_RECENT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pointcloud_core::plans::{Confidence, LevelKind, LevelStatus};

    fn source(mtime: u64) -> SourceRef {
        SourceRef {
            path: PathBuf::from("C:/scans/office.laz"),
            file_len: 353_578_041,
            mtime,
            points: 54_373_904,
            scale: [1.0; 3],
            offset: [0.0; 3],
            deleted_points: 0,
            deletions_digest: format!("{:016x}", 0),
            hidden_classes: vec![7, 18],
        }
    }

    fn level(id: &str, kind: LevelKind, floor_z: f64) -> Level {
        Level {
            id: id.into(),
            name: id.into(),
            kind,
            floor_z,
            ceiling_z: Some(floor_z + 2.95),
            slab_underside: None,
            slab_thickness: Some(0.25),
            cut_height: 1.2,
            tilt_mm_per_m: Some([0.1, -0.2]),
            share: 0.8,
            is_peil: floor_z == 0.0,
            confidence: Confidence::certain(),
            status: LevelStatus::Found,
        }
    }

    fn sample() -> MeshToPlansProject {
        let mut project = MeshToPlansProject::new("Office");
        project.sources.push(source(1_759_700_000));
        project.frame = Some(BuildingFrame {
            second_direction_deg: Some(107.0),
            peil_z: 0.0,
            ..BuildingFrame::new(17.0, [0.0, 3.0])
        });
        project.datum.nap_offset = Some(1.85);
        project.regions.core = Some(BoxRecord {
            min: [-12.0, -9.0, -1.3],
            max: [32.0, 28.0, 10.8],
            rotation_deg: 0.0,
        });
        project.levels = vec![
            level("00", LevelKind::Ground, 0.0),
            level("01", LevelKind::Storey, 3.2),
            level("R", LevelKind::Roof, 9.6),
        ];
        project.steps.insert(
            "prepare".into(),
            StepRecord {
                status: "confirmed".into(),
                reason: None,
                basis: Some(basis_text(prepare_basis(
                    &project.sources,
                    &project.regions,
                ))),
                finished: Some(1_759_700_100),
                seconds: Some(11.2),
            },
        );
        project.survey = Some(SurveyRecord {
            grid: SurveyGrid {
                origin: [-30.0, -20.0, -1.3],
                cell_xy: 0.05,
                cell_z: 0.02,
                size: [4, 3, 5],
            },
            frame: BuildingFrame::new(17.0, [0.0, 3.0]),
            level_histogram: vec![0, 12, 3, 0, 9],
            footprint: vec![PlanRegion {
                outer: vec![
                    [0.0, 0.0],
                    [24.0, 0.0],
                    [24.0, 14.0],
                    [0.0, 14.0],
                    [0.0, 0.0],
                ],
                holes: Vec::new(),
            }],
            footprint_area: 336.0,
            ground_z: Some(-0.16),
            below_z: Some(-2.0),
            below_points: 3652,
            below_clusters: 3,
            stats: SurveyStats::default(),
            seconds: 11.1,
        });
        project
    }

    #[test]
    fn a_project_goes_through_its_file_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let file = project_file(&directory.path().join("Office"));
        let project = sample();
        save(&file, &project).unwrap();
        let read = load(&file).unwrap();
        assert_eq!(read, project);
        // Written again over the old file, with nothing left beside it.
        save(&file, &read).unwrap();
        let names: Vec<String> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [FILE_NAME]);
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"format\": \"open-pointcloud-studio-mesh-to-plans\""));
        assert_eq!(read.status(WizardStep::Prepare), StepStatus::Confirmed);
        assert_eq!(read.status(WizardStep::Mesh), StepStatus::NotRun);
        assert_eq!(read.resume_step(), WizardStep::Mesh);
        let recent = RecentProject::of(&file, &read);
        assert_eq!(recent.name, "Office");
        assert_eq!(recent.step, WizardStep::Mesh);
        assert_eq!(
            read_recent(&[file.clone(), directory.path().join("gone.json")]),
            [recent]
        );
    }

    #[test]
    fn a_result_without_its_basis_counts_as_not_run() {
        let mut project = sample();
        let record = |status: &str| StepRecord {
            status: status.into(),
            reason: None,
            basis: None,
            finished: Some(1_759_700_200),
            seconds: Some(1.0),
        };
        // As an earlier build kept the steps that only stood in for theirs.
        for step in ["mesh", "views", "walls"] {
            project.steps.insert(step.into(), record("confirmed"));
        }
        project.steps.insert("sheet".into(), record("skipped"));
        assert_eq!(project.status(WizardStep::Prepare), StepStatus::Confirmed);
        assert_eq!(project.status(WizardStep::Mesh), StepStatus::NotRun);
        assert_eq!(project.status(WizardStep::Sheet), StepStatus::Skipped);
        assert_eq!(project.resume_step(), WizardStep::Mesh);
        project.steps.get_mut("prepare").unwrap().basis = None;
        assert_eq!(project.status(WizardStep::Prepare), StepStatus::NotRun);
        assert_eq!(project.resume_step(), WizardStep::Prepare);
    }

    #[test]
    fn what_is_no_project_or_too_new_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join(FILE_NAME);
        fs::write(&file, "{\"format\": \"something else\", \"version\": 1}").unwrap();
        assert_eq!(
            load(&file).unwrap_err(),
            "the file is no Pointcloud to Drawing project"
        );
        let mut newer = serde_json::to_value(sample()).unwrap();
        newer["version"] = serde_json::json!(VERSION + 1);
        fs::write(&file, newer.to_string()).unwrap();
        assert_eq!(
            load(&file).unwrap_err(),
            "the project was made by a newer version"
        );
        // A file saved with a byte order mark is read.
        let project = sample();
        let mut text = b"\xEF\xBB\xBF".to_vec();
        text.extend(serde_json::to_vec(&project).unwrap());
        fs::write(&file, text).unwrap();
        assert_eq!(load(&file).unwrap(), project);
        assert!(load(&directory.path().join("missing.json")).is_err());
    }

    #[test]
    fn the_basis_follows_the_scans_and_what_was_fixed() {
        let regions = Regions::default();
        let first = prepare_basis(&[source(1_759_700_000)], &regions);
        assert_eq!(first, prepare_basis(&[source(1_759_700_000)], &regions));
        // Another time of change, another basis.
        assert_ne!(first, prepare_basis(&[source(1_759_700_001)], &regions));
        let mut deleted = source(1_759_700_000);
        deleted.deleted_points = 12;
        assert_ne!(first, prepare_basis(&[deleted], &regions));
        let mut hidden = source(1_759_700_000);
        hidden.hidden_classes.clear();
        assert_ne!(first, prepare_basis(&[hidden], &regions));
        let turned = Regions {
            chosen_rotation: Some(17.0),
            ..regions
        };
        assert_ne!(first, prepare_basis(&[source(1_759_700_000)], &turned));
        assert_eq!(basis_text(first).len(), 32);
    }

    #[test]
    fn projects_go_to_the_documents_folder_that_windows_knows() {
        let documents = documents_folder().expect("a Documents folder");
        assert!(documents.is_absolute(), "{documents:?}");
        // The tests keep their projects elsewhere.
        assert_eq!(default_root(), None);
        #[cfg(windows)]
        {
            // Where Explorer shows it, which need not be in the profile.
            let known = known_documents_folder().expect("Windows knows it");
            assert_eq!(documents, known);
            assert!(known.is_dir(), "{known:?}");
        }
    }

    #[test]
    fn a_new_project_takes_a_folder_without_a_project() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert_eq!(
            free_folder(root, "Office: north"),
            root.join("Office_ north")
        );
        save(&project_file(&root.join("Office")), &sample()).unwrap();
        assert_eq!(free_folder(root, "Office"), root.join("Office 2"));
        // A folder without a project file is free.
        fs::create_dir_all(root.join("Office 2").join("survey")).unwrap();
        assert_eq!(free_folder(root, "Office"), root.join("Office 2"));
        save(&project_file(&root.join("Office 2")), &sample()).unwrap();
        assert_eq!(free_folder(root, "Office"), root.join("Office 3"));
    }

    #[test]
    fn a_scan_has_one_basis_however_its_path_was_typed() {
        let regions = Regions::default();
        let first = prepare_basis(&[source(1_759_700_000)], &regions);
        let mut typed = source(1_759_700_000);
        typed.path = PathBuf::from("C:/scans/./office.laz");
        assert_eq!(prepare_basis(&[typed], &regions), first);
        assert!(same_path(
            Path::new("C:/scans/office.laz"),
            Path::new("C:/scans/./office.laz")
        ));
        assert!(!same_path(
            Path::new("C:/scans/office.laz"),
            Path::new("C:/scans/other.laz")
        ));
        #[cfg(windows)]
        {
            let mut typed = source(1_759_700_000);
            typed.path = PathBuf::from(r"c:\Scans\Office.LAZ");
            assert_eq!(prepare_basis(&[typed], &regions), first);
            assert!(same_path(
                Path::new(r"C:\scans\office.laz"),
                Path::new("c:/Scans/office.laz")
            ));
            assert_eq!(
                path_key(Path::new(r"C:\scans\office.laz")),
                "c:/scans/office.laz"
            );
        }
    }

    #[test]
    fn folder_names_and_the_recent_list_are_kept_tidy() {
        assert_eq!(folder_name("  Office: north/wing? "), "Office_ north_wing_");
        assert_eq!(folder_name("..."), "Project");
        assert_eq!(folder_name(""), "Project");
        let mut recent: Vec<PathBuf> = (0..MAX_RECENT)
            .map(|place| PathBuf::from(format!("{place}.json")))
            .collect();
        remember(&mut recent, Path::new("3.json"));
        assert_eq!(recent[0], PathBuf::from("3.json"));
        assert_eq!(recent.len(), MAX_RECENT);
        remember(&mut recent, Path::new("new.json"));
        assert_eq!(recent.len(), MAX_RECENT);
        assert_eq!(recent[0], PathBuf::from("new.json"));
        assert!(!recent.contains(&PathBuf::from(format!("{}.json", MAX_RECENT - 1))));
    }
}
