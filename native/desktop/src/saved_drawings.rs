//! The 2D drawings made with Create 2D plan / elevation / section, kept
//! beside the saved views in `drawings.json`: how each one was made, so that
//! it is listed under VIEWS after a restart and can be made again from its
//! scans.

use std::io;
use std::path::{Path, PathBuf};

use pointcloud_core::{
    Bounds, DrawingOrigin, DrawingRequest, DrawingUnits, DrawingVersion, DrawingView, OrientedBox,
    PointColor, PointLayers,
};
use serde::{Deserialize, Serialize};

use crate::camera_views::{self, Entries};
use crate::sheet_dialog::SheetKind;

/// The longest name of a drawing, in characters.
pub const MAX_NAME_CHARS: usize = 96;

/// The box a drawing was cut from: its limits before the turn and the turn
/// about the vertical through its centre, in degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DrawingBox {
    pub min: [f64; 3],
    pub max: [f64; 3],
    #[serde(default)]
    pub rotation: f64,
}

/// The settings of the drawing besides its view and slab, with every choice
/// written as the key the command API uses for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRequest {
    pub points: bool,
    pub fill: bool,
    pub grid: f64,
    pub max_wall_thickness: f64,
    pub min_wall_thickness: f64,
    pub square: bool,
    pub units: String,
    pub origin: String,
    pub point_spacing: f64,
    pub max_points: usize,
    /// The points used, in percent. A drawing kept before this setting was
    /// made from every point.
    #[serde(default = "every_point")]
    pub sample_percent: f64,
    pub point_layers: String,
    pub color: String,
    pub version: String,
}

fn every_point() -> f64 {
    100.0
}

/// How a drawing of the Project Browser was made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedDrawing {
    pub guid: String,
    pub name: String,
    pub kind: SheetKind,
    #[serde(rename = "box")]
    pub section: DrawingBox,
    /// The face of the box that is drawn, as its key: plan, front, back,
    /// left or right.
    pub view: String,
    /// Depth of the slab behind the cut in metres; none for an elevation.
    pub thickness: Option<f64>,
    pub request: StoredRequest,
    /// The scans it was drawn from, as the saved views name them.
    pub sources: Vec<PathBuf>,
    /// Seconds since 1970, UTC.
    #[serde(default)]
    pub created: u64,
}

impl SavedDrawing {
    pub fn new(
        name: &str,
        kind: SheetKind,
        section: OrientedBox,
        request: &DrawingRequest,
        sources: Vec<PathBuf>,
    ) -> Self {
        Self {
            guid: camera_views::new_guid(),
            name: name.to_owned(),
            kind,
            section: DrawingBox {
                min: section.bounds.min,
                max: section.bounds.max,
                rotation: section.rotation_degrees,
            },
            view: request.view.key().to_owned(),
            thickness: request.thickness,
            request: StoredRequest {
                points: request.points,
                fill: request.fill,
                grid: request.grid,
                max_wall_thickness: request.max_wall_thickness,
                min_wall_thickness: request.min_wall_thickness,
                square: request.square,
                units: request.units.key().to_owned(),
                origin: request.origin.key().to_owned(),
                point_spacing: request.point_spacing,
                max_points: request.max_points,
                sample_percent: request.sample_percent,
                point_layers: request.point_layers.key().to_owned(),
                color: request.color.key().to_owned(),
                version: request.version.key().to_owned(),
            },
            sources,
            created: camera_views::now_seconds(),
        }
    }

    /// The box with its turn.
    pub fn oriented(&self) -> OrientedBox {
        OrientedBox::new(
            Bounds {
                min: self.section.min,
                max: self.section.max,
            },
            self.section.rotation,
        )
    }

    /// What the core is asked for to make the drawing again, or `None` when
    /// a choice is one this version does not know or the settings give no
    /// drawing.
    pub fn request(&self) -> Option<DrawingRequest> {
        let stored = &self.request;
        let request = DrawingRequest {
            view: DrawingView::from_key(&self.view)?,
            thickness: self.thickness,
            points: stored.points,
            fill: stored.fill,
            grid: stored.grid,
            max_wall_thickness: stored.max_wall_thickness,
            min_wall_thickness: stored.min_wall_thickness,
            square: stored.square,
            units: DrawingUnits::from_key(&stored.units)?,
            origin: DrawingOrigin::from_key(&stored.origin)?,
            point_spacing: stored.point_spacing,
            max_points: stored.max_points,
            sample_percent: stored.sample_percent,
            point_layers: PointLayers::from_key(&stored.point_layers)?,
            color: PointColor::from_key(&stored.color)?,
            version: DrawingVersion::from_key(&stored.version)?,
        };
        request.validate().ok().map(|()| request)
    }

    /// Whether the drawing was made from this scan.
    pub fn uses(&self, source: &Path) -> bool {
        self.sources.iter().any(|known| known == source)
    }

    fn valid(&self) -> bool {
        let finite = |xyz: [f64; 3]| xyz.iter().all(|value| value.is_finite());
        camera_views::is_guid(&self.guid)
            && !self.name.trim().is_empty()
            && self.name.chars().count() <= MAX_NAME_CHARS
            && finite(self.section.min)
            && finite(self.section.max)
            && self.section.rotation.is_finite()
            && (0..3).all(|axis| self.section.min[axis] <= self.section.max[axis])
            && !self.sources.is_empty()
            && self.request().is_some()
    }
}

/// A name for a new drawing that no drawing of the same scans has, without
/// regard to case: the name itself, else the name with a number after it.
pub fn free_name(drawings: &[SavedDrawing], sources: &[PathBuf], name: &str) -> String {
    let base: String = name.trim().chars().take(MAX_NAME_CHARS - 4).collect();
    let taken = |candidate: &str| {
        drawings.iter().any(|drawing| {
            drawing.name.eq_ignore_ascii_case(candidate)
                && drawing
                    .sources
                    .iter()
                    .any(|source| sources.contains(source))
        })
    };
    std::iter::once(base.clone())
        .chain((2..1000).map(|number| format!("{base} ({number})")))
        .find(|candidate| !taken(candidate))
        .unwrap_or(base)
}

fn drawings_path() -> Option<PathBuf> {
    camera_views::directory().map(|directory| directory.join("drawings.json"))
}

pub fn load() -> Vec<SavedDrawing> {
    drawings_path().map_or_else(Vec::new, |path| load_from(&path))
}

pub fn save(drawings: &[SavedDrawing]) -> io::Result<()> {
    let path = drawings_path().ok_or_else(|| io::Error::other("no user config directory"))?;
    camera_views::write_json(&path, &drawings)
}

/// The drawings in a file, each read on its own, so that one this version
/// cannot read does not take the others with it. A file that cannot be read
/// at all is kept beside it before the next save replaces it.
fn load_from(path: &Path) -> Vec<SavedDrawing> {
    let entries = match camera_views::read_entries(path) {
        Entries::Missing => return Vec::new(),
        Entries::Unreadable => {
            let _ = std::fs::copy(path, path.with_extension("unreadable.json"));
            return Vec::new();
        }
        Entries::Read(entries) => entries,
    };
    let mut drawings: Vec<SavedDrawing> = Vec::new();
    for drawing in entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<SavedDrawing>(entry).ok())
        .filter(SavedDrawing::valid)
    {
        if !drawings.iter().any(|known| known.guid == drawing.guid) {
            drawings.push(drawing);
        }
    }
    drawings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(name: &str, sources: Vec<PathBuf>) -> SavedDrawing {
        let mut request = DrawingRequest::for_view(DrawingView::Plan);
        request.thickness = Some(0.25);
        request.units = DrawingUnits::Metres;
        request.max_points = 1234;
        SavedDrawing::new(
            name,
            SheetKind::Plan,
            OrientedBox::new(
                Bounds {
                    min: [1.0, 2.0, 0.0],
                    max: [9.0, 7.5, 1.2],
                },
                22.5,
            ),
            &request,
            sources,
        )
    }

    #[test]
    fn a_drawing_keeps_how_it_was_made_through_a_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings/drawings.json");
        let first = plan("Plan +1.20", vec![PathBuf::from("C:/scans/office.laz")]);
        let mut section = first.clone();
        section.guid = camera_views::new_guid();
        section.name = "Section Front y=4.00".into();
        section.kind = SheetKind::Section;
        section.view = "front".into();
        section.request.fill = false;
        section.sources.push(PathBuf::from("C:/scans/annex.e57"));
        camera_views::write_json(&path, &vec![first.clone(), section.clone()]).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded, vec![first.clone(), section.clone()]);

        // The box, the view, the slab and every setting come back as made.
        let request = loaded[0].request().unwrap();
        assert_eq!(request.view, DrawingView::Plan);
        assert_eq!(request.thickness, Some(0.25));
        assert_eq!(request.units, DrawingUnits::Metres);
        assert_eq!(request.max_points, 1234);
        assert!(request.fill);
        let turned = loaded[0].oriented();
        assert_eq!(turned.rotation_degrees, 22.5);
        assert_eq!(turned.bounds.max, [9.0, 7.5, 1.2]);
        assert_eq!(loaded[1].request().unwrap().view, DrawingView::Front);
        assert!(loaded[1].uses(Path::new("C:/scans/annex.e57")));
        assert!(!loaded[0].uses(Path::new("C:/scans/annex.e57")));

        // A file a text editor saved with a byte order mark still reads.
        let mut marked = b"\xEF\xBB\xBF".to_vec();
        marked.extend(std::fs::read(&path).unwrap());
        std::fs::write(&path, marked).unwrap();
        assert_eq!(load_from(&path), loaded);

        // A drawing this version cannot make again does not take the others
        // with it; a list that cannot be read is kept beside it.
        let mut stored = serde_json::to_value(vec![first.clone(), section]).unwrap();
        stored[1]["view"] = serde_json::json!("from below");
        stored[0]["kind"] = serde_json::json!("plan");
        std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
        assert_eq!(load_from(&path), vec![first]);
        std::fs::write(&path, b"[{\"guid\"").unwrap();
        assert!(load_from(&path).is_empty());
        assert!(path.with_extension("unreadable.json").is_file());
        assert!(load_from(&directory.path().join("none.json")).is_empty());
    }

    #[test]
    fn names_stay_unique_among_the_drawings_of_the_same_scans() {
        let office = vec![PathBuf::from("office.laz")];
        let other = vec![PathBuf::from("other.laz")];
        let drawings = vec![plan("Plan +1.20", office.clone())];
        assert_eq!(
            free_name(&drawings, &office, "plan +1.20"),
            "plan +1.20 (2)"
        );
        assert_eq!(free_name(&drawings, &other, "Plan +1.20"), "Plan +1.20");
        let more = vec![drawings[0].clone(), plan("Plan +1.20 (2)", office.clone())];
        assert_eq!(free_name(&more, &office, "Plan +1.20"), "Plan +1.20 (3)");
    }
}
