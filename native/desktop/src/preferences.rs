//! Native display and indexing preferences stored outside source scans.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ColorMode;

pub(crate) const MIN_POINT_BUDGET: u32 = 1_000;
pub(crate) const MAX_POINT_BUDGET: u32 = 10_000_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Preferences {
    pub color_mode: ColorMode,
    pub point_size: f32,
    pub eye_dome: bool,
    pub eye_dome_strength: f32,
    pub show_scan_poses: bool,
    pub budget: u32,
    pub auto_index: bool,
    pub filter_ground: bool,
    pub filter_vegetation: bool,
    pub filter_buildings: bool,
    pub filter_other: bool,
    /// The program that opens exported DXF and DWG files; `None` looks for
    /// Open CAD Studio.
    pub cad_viewer: Option<PathBuf>,
    /// Whether every exported DXF or DWG file opens in that program.
    pub open_after_export: bool,
    /// Whether the Drawing view is shown when a section drawing has been
    /// exported.
    pub show_drawing_after_export: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            color_mode: ColorMode::Rgb,
            point_size: 2.0,
            eye_dome: true,
            eye_dome_strength: 1.0,
            show_scan_poses: true,
            budget: 250_000,
            auto_index: true,
            filter_ground: true,
            filter_vegetation: true,
            filter_buildings: true,
            filter_other: true,
            cad_viewer: None,
            open_after_export: false,
            show_drawing_after_export: true,
        }
    }
}

impl Preferences {
    fn validated(mut self) -> Self {
        let defaults = Self::default();
        if !self.point_size.is_finite() || !(0.1..=20.0).contains(&self.point_size) {
            self.point_size = defaults.point_size;
        }
        if !self.eye_dome_strength.is_finite() || !(0.0..=5.0).contains(&self.eye_dome_strength) {
            self.eye_dome_strength = defaults.eye_dome_strength;
        }
        if !(MIN_POINT_BUDGET..=MAX_POINT_BUDGET).contains(&self.budget) {
            self.budget = defaults.budget;
        }
        self
    }
}

pub(crate) fn load() -> Preferences {
    config_path().map_or_else(Preferences::default, |path| load_from(&path))
}

pub(crate) fn save(preferences: &Preferences) -> io::Result<()> {
    let path = config_path().ok_or_else(|| io::Error::other("no user config directory"))?;
    save_to(&path, preferences)
}

/// Directory for the settings of this application: `XDG_CONFIG_HOME` when
/// set, otherwise the roaming application data folder on Windows and
/// `~/.config` elsewhere.
pub(crate) fn config_directory() -> Option<PathBuf> {
    let set = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = set("XDG_CONFIG_HOME")
        .or_else(|| if cfg!(windows) { set("APPDATA") } else { None })
        .or_else(|| set("HOME").map(|home| home.join(".config")))?;
    Some(base.join("open-pointcloud-studio-native"))
}

fn config_path() -> Option<PathBuf> {
    config_directory().map(|directory| directory.join("settings.json"))
}

fn load_from(path: &Path) -> Preferences {
    let Ok(metadata) = fs::metadata(path) else {
        return Preferences::default();
    };
    if metadata.len() > 64 * 1024 {
        return Preferences::default();
    }
    fs::read(path)
        .ok()
        .and_then(|bytes| {
            // A file saved by a text editor may start with a byte order mark,
            // which the JSON reader refuses.
            let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            serde_json::from_slice::<Preferences>(json).ok()
        })
        .unwrap_or_default()
        .validated()
}

fn save_to(path: &Path, preferences: &Preferences) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("settings path has no parent"))?;
    fs::create_dir_all(directory)?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), &preferences.clone().validated())
        .map_err(io::Error::other)?;
    temporary.as_file_mut().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_a_byte_order_mark_is_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, b"\xEF\xBB\xBF{\"budget\": 3000000}").unwrap();
        assert_eq!(load_from(&path).budget, 3_000_000);
    }

    #[test]
    fn settings_round_trip_and_invalid_values_use_safe_defaults() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config/settings.json");
        let settings = Preferences {
            color_mode: ColorMode::Classification,
            point_size: 4.5,
            budget: 220_000,
            auto_index: false,
            filter_ground: false,
            cad_viewer: Some(PathBuf::from("/opt/viewer/OpenCADStudio")),
            open_after_export: true,
            show_drawing_after_export: false,
            ..Preferences::default()
        };
        save_to(&path, &settings).unwrap();
        assert_eq!(load_from(&path), settings);

        fs::write(
            &path,
            r#"{"point_size": -2, "eye_dome_strength": 99, "budget": 0, "auto_index": false}"#,
        )
        .unwrap();
        let repaired = load_from(&path);
        assert_eq!(repaired.point_size, 2.0);
        assert_eq!(repaired.eye_dome_strength, 1.0);
        assert_eq!(repaired.budget, 250_000);
        assert!(!repaired.auto_index);

        let high_budget = Preferences {
            budget: MAX_POINT_BUDGET,
            ..Preferences::default()
        };
        save_to(&path, &high_budget).unwrap();
        assert_eq!(load_from(&path).budget, MAX_POINT_BUDGET);

        fs::write(&path, r#"{"budget": 10000001}"#).unwrap();
        assert_eq!(load_from(&path).budget, 250_000);
    }
}
