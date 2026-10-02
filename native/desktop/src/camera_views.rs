//! Native, per-source camera bookmarks stored outside the scan files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedView {
    pub source: PathBuf,
    pub name: String,
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    pub pan: [f32; 2],
}

impl SavedView {
    fn valid(&self) -> bool {
        !self.source.as_os_str().is_empty()
            && !self.name.trim().is_empty()
            && self.name.chars().count() <= 64
            && self.yaw.is_finite()
            && self.yaw.abs() <= std::f32::consts::TAU
            && self.pitch.is_finite()
            && self.pitch.abs() <= std::f32::consts::FRAC_PI_2
            && self.zoom.is_finite()
            && (0.000_001..=10_000.0).contains(&self.zoom)
            && self.pan.iter().all(|value| value.is_finite())
    }
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

fn config_path() -> Option<PathBuf> {
    crate::preferences::config_directory().map(|directory| directory.join("camera-views.json"))
}

fn load_from(path: &Path) -> Vec<SavedView> {
    let Ok(metadata) = fs::metadata(path) else {
        return Vec::new();
    };
    if metadata.len() > 8 * 1_048_576 {
        return Vec::new();
    }
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<SavedView>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(SavedView::valid)
        .collect()
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
        let view = SavedView {
            source: source_key(&source),
            name: "Entrance".into(),
            yaw: -0.8,
            pitch: 0.6,
            zoom: 3.0,
            pan: [12.0, -4.0],
        };
        save_to(&path, std::slice::from_ref(&view)).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].source, view.source);
        assert_eq!(loaded[0].name, view.name);
        assert_eq!(loaded[0].pan, view.pan);

        fs::write(
            &path,
            serde_json::to_vec(&vec![
                view,
                SavedView {
                    source: source_key(&source),
                    name: "Broken".into(),
                    yaw: 0.0,
                    pitch: 0.0,
                    zoom: 0.0,
                    pan: [0.0, 0.0],
                },
            ])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(load_from(&path).len(), 1);
    }
}
