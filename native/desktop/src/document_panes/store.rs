//! Bounded atomic storage for window layouts and camera/source references.
//! Source scan bytes and transient point edits are never stored in this file.
use super::layout::Workspace;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

pub const FILE_NAME: &str = "document-layout-v1.json";
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Walking {
    pub eye: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub field_of_view: f32,
}
impl Walking {
    fn valid(&self) -> bool {
        self.eye.iter().all(|v| v.is_finite())
            && self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.pitch.abs() < std::f32::consts::FRAC_PI_2
            && self.field_of_view.is_finite()
            && (0.0..std::f32::consts::PI).contains(&self.field_of_view)
            && self.field_of_view > 0.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    #[serde(default = "reference_size")]
    pub reference_size: [f32; 2],
    #[serde(default = "rgb")]
    pub colour: String,
    #[serde(default)]
    pub label: String,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub pan: [f32; 2],
    pub orbit_point: Option<[f64; 3]>,
    pub walk: Option<Walking>,
}
fn reference_size() -> [f32; 2] {
    [1., 1.]
}
fn rgb() -> String {
    "rgb".into()
}
impl Camera {
    fn valid(&self) -> bool {
        self.reference_size
            .iter()
            .all(|value| value.is_finite() && *value > 0. && *value <= 1_000_000.)
            && ["rgb", "elevation", "intensity", "classification"].contains(&self.colour.as_str())
            && self.label.len() <= 64
            && !self.label.contains('\0')
            && self.yaw.is_finite()
            && self.pitch.is_finite()
            && self.pitch.abs() <= std::f32::consts::FRAC_PI_2 + 0.001
            && self.zoom.is_finite()
            && self.zoom > 0.0
            && self.pan.iter().all(|v| v.is_finite())
            && self
                .orbit_point
                .is_none_or(|p| p.iter().all(|v| v.is_finite()))
            && self.walk.as_ref().is_none_or(Walking::valid)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Look {
    Scene {
        camera: Camera,
    },
    Photo {
        index: usize,
        camera: Walking,
        pinned: bool,
        zoom: f32,
        blend: f32,
    },
    Station {
        station: usize,
        camera: Walking,
    },
    Drawing {
        center: [f64; 2],
        scale: f64,
        #[serde(default)]
        layers: Vec<(String, bool)>,
    },
}
impl Look {
    fn valid(&self) -> bool {
        match self {
            Self::Scene { camera } => camera.valid(),
            Self::Photo {
                camera,
                zoom,
                blend,
                ..
            } => {
                camera.valid()
                    && zoom.is_finite()
                    && *zoom > 0.0
                    && blend.is_finite()
                    && (0.0..=1.0).contains(blend)
            }
            Self::Station { camera, .. } => camera.valid(),
            Self::Drawing {
                center,
                scale,
                layers,
            } => {
                center.iter().all(|v| v.is_finite())
                    && scale.is_finite()
                    && *scale > 0.0
                    && layers.len() <= 4096
                    && layers
                        .iter()
                        .all(|(name, _)| name.len() <= 32768 && !name.contains('\0'))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub path: PathBuf,
    pub scale: [f64; 3],
    pub offset: [f64; 3],
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub enabled: bool,
    pub reference: Option<([f64; 3], [f64; 3])>,
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub rotation: f64,
}
impl Section {
    fn valid(&self) -> bool {
        fn bounds(min: [f64; 3], max: [f64; 3]) -> bool {
            min.iter().chain(&max).all(|value| value.is_finite())
                && (0..3).all(|axis| min[axis] <= max[axis])
        }
        bounds(self.min, self.max)
            && self.rotation.is_finite()
            && self.reference.is_none_or(|(min, max)| bounds(min, max))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    pub version: u32,
    pub workspace: Workspace,
    pub sources: Vec<Source>,
    pub looks: BTreeMap<String, Look>,
    pub section: Option<Section>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            version: 1,
            workspace: Workspace::default(),
            sources: Vec::new(),
            looks: BTreeMap::new(),
            section: None,
        }
    }
}
fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}
impl State {
    pub fn validate(&self) -> io::Result<()> {
        if self.version != 1 {
            return Err(invalid("Unsupported document layout version"));
        }
        self.workspace.validate().map_err(invalid)?;
        if self
            .section
            .as_ref()
            .is_some_and(|section| !section.valid())
        {
            return Err(invalid("Invalid shared section geometry"));
        }
        let mut paths = HashSet::new();
        if self.sources.len() > 4096 {
            return Err(invalid("Too many layout sources"));
        }
        let mut reference_bytes = 0usize;
        for source in &self.sources {
            let path = source
                .path
                .to_str()
                .ok_or_else(|| invalid("A layout path is not Unicode"))?;
            if !source.path.is_absolute()
                || path.is_empty()
                || path.len() > 32_768
                || path.contains('\0')
                || !paths.insert(&source.path)
                || source
                    .scale
                    .iter()
                    .chain(&source.offset)
                    .any(|v| !v.is_finite())
            {
                return Err(invalid("Invalid or duplicate layout source"));
            }
            reference_bytes = reference_bytes.saturating_add(path.len());
        }
        for (key, look) in &self.looks {
            if self.workspace.owner(key).is_none() || !look.valid() {
                return Err(invalid("Invalid or orphaned document camera"));
            }
            reference_bytes = reference_bytes.saturating_add(key.len());
        }
        if reference_bytes > MAX_BYTES {
            return Err(invalid("Document layout is too large"));
        }
        Ok(())
    }
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
            return Err(invalid("Document layout is too large or is not a file"));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        fs::File::open(path)?
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("Document layout is too large"));
        }
        let state: Self = serde_json::from_slice(&bytes).map_err(invalid)?;
        state.validate()?;
        Ok(Some(state))
    }
    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(invalid)?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("Document layout is too large"));
        }
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| invalid("A layout needs a configuration directory"))?;
        fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }
}

/// Background saves may finish out of order. The lock covers publication,
/// so a slow, older request can never replace a newer saved layout.
pub struct Writer {
    path: PathBuf,
    last_revision: Mutex<Option<u64>>,
}
impl Writer {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            last_revision: Mutex::new(None),
        }
    }
    pub fn save_if_newer(&self, revision: u64, state: &State) -> io::Result<bool> {
        let mut last = self
            .last_revision
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|last| revision <= last) {
            return Ok(false);
        }
        state.save(&self.path)?;
        *last = Some(revision);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::super::layout::Side;
    use super::*;
    use std::sync::{Arc, Barrier};

    fn fixture(directory: &Path) -> State {
        let mut state = State::default();
        state.workspace.open("photo".into(), 1).unwrap();
        state.workspace.split_tab("photo", 1, Side::Right).unwrap();
        state.sources.push(Source {
            path: directory.join("not-currently-mounted.e57"),
            scale: [-1., 2., 1.],
            offset: [100., 200., 300.],
            visible: true,
        });
        state.looks.insert(
            "photo".into(),
            Look::Station {
                station: 7,
                camera: Walking {
                    eye: [6_000_000., 3_000_000., 1.6],
                    yaw: 0.8,
                    pitch: 0.1,
                    field_of_view: 1.2,
                },
            },
        );
        state
    }
    #[test]
    fn layouts_sources_and_photo_looks_round_trip_without_loading_scans() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        assert_eq!(State::load(&path).unwrap(), None);
        let state = fixture(directory.path());
        state.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap(), Some(state));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn invalid_state_does_not_replace_a_previously_saved_layout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let mut state = fixture(directory.path());
        state.save(&path).unwrap();
        let original = fs::read(&path).unwrap();
        state.looks.insert(
            "model".into(),
            Look::Drawing {
                center: [f64::NAN, 0.],
                scale: 1.,
                layers: Vec::new(),
            },
        );
        assert!(state.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        state.looks.remove("model");
        state.version = 2;
        assert!(state.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
    #[test]
    fn broken_oversized_and_unknown_version_files_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        for bytes in [
            b"not JSON".to_vec(),
            b"{\"version\":2}".to_vec(),
            vec![b' '; MAX_BYTES + 1],
        ] {
            fs::write(&path, &bytes).unwrap();
            assert!(State::load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }
    #[test]
    fn a_late_older_save_cannot_replace_a_newer_layout() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let writer = Writer::new(path.clone());
        let old = fixture(directory.path());
        let mut new = old.clone();
        new.workspace.move_tab("photo", 1).unwrap();
        assert!(writer.save_if_newer(2, &new).unwrap());
        assert!(!writer.save_if_newer(1, &old).unwrap());
        assert!(!writer.save_if_newer(2, &old).unwrap());
        assert_eq!(State::load(&path).unwrap(), Some(new));
    }
    #[test]
    fn concurrent_saves_keep_the_greatest_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        let writer = Arc::new(Writer::new(path.clone()));
        let barrier = Arc::new(Barrier::new(3));
        let old = fixture(directory.path());
        let mut new = old.clone();
        new.workspace.move_tab("photo", 1).unwrap();
        let expected = new.clone();
        std::thread::scope(|scope| {
            for (revision, state) in [(1, old), (2, new)] {
                let (writer, barrier) = (Arc::clone(&writer), Arc::clone(&barrier));
                scope.spawn(move || {
                    barrier.wait();
                    writer.save_if_newer(revision, &state).unwrap();
                });
            }
            barrier.wait();
        });
        assert_eq!(State::load(&path).unwrap(), Some(expected));
    }
}
