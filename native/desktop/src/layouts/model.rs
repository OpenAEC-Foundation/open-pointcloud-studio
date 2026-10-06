//! What a sheet is: its paper, its title block and the views placed on it,
//! with the figures that follow from them, and how the sheets are kept in
//! `sheets.json` beside the saved views. Every length on a sheet is in
//! millimetres on the paper, from its lower left corner, X to the right and
//! Y up.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::camera_views::{self, Entries};

/// The sheets kept at most.
pub const MAX_SHEETS: usize = 500;
/// The longest name, number, title block field and title of a viewport.
pub const MAX_NAME_CHARS: usize = 96;
pub const MAX_NUMBER_CHARS: usize = 48;
/// The views a sheet holds at most.
pub const MAX_VIEWPORTS: usize = 48;
/// The scales a drawing is placed at by choice; any other is typed.
pub const SCALES: [f64; 5] = [20.0, 50.0, 100.0, 200.0, 500.0];
/// A drawing is placed at 1:100 unless another scale is chosen.
pub const DEFAULT_SCALE: f64 = 100.0;
/// The scales that can be typed, from 1:1 to 1:100 000.
pub const MIN_SCALE: f64 = 1.0;
pub const MAX_SCALE: f64 = 100_000.0;
/// The border lies this far inside the edge of the paper.
pub const MARGIN: f64 = 10.0;
/// The title block in the lower right corner, inside the border.
pub const TITLE_BLOCK: [f64; 2] = [180.0, 32.0];
/// An image of a 3D view is placed at about this many dots per inch.
pub const IMAGE_DPI: f64 = 200.0;
/// The smallest side of a viewport, in millimetres.
pub const MIN_SIDE: f64 = 5.0;
/// Viewports placed by the application keep this much paper between them.
const GAP: f64 = 8.0;
/// The paper under a viewport that its title takes.
pub const TITLE_ROOM: f64 = 9.0;

/// The paper of a sheet, from the A series.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Paper {
    A4,
    A3,
    A2,
    A1,
    A0,
}

impl Paper {
    pub const ALL: [Self; 5] = [Self::A4, Self::A3, Self::A2, Self::A1, Self::A0];

    pub fn key(self) -> &'static str {
        match self {
            Self::A4 => "a4",
            Self::A3 => "a3",
            Self::A2 => "a2",
            Self::A1 => "a1",
            Self::A0 => "a0",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        let value = value.trim();
        Self::ALL
            .into_iter()
            .find(|paper| paper.key().eq_ignore_ascii_case(value))
    }

    /// Width and height standing upright, in millimetres.
    pub fn portrait(self) -> [f64; 2] {
        match self {
            Self::A4 => [210.0, 297.0],
            Self::A3 => [297.0, 420.0],
            Self::A2 => [420.0, 594.0],
            Self::A1 => [594.0, 841.0],
            Self::A0 => [841.0, 1189.0],
        }
    }

    /// Width and height lying on its long side (`landscape`) or standing.
    pub fn size(self, landscape: bool) -> [f64; 2] {
        let [short, long] = self.portrait();
        if landscape {
            [long, short]
        } else {
            [short, long]
        }
    }
}

impl std::fmt::Display for Paper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.key().to_ascii_uppercase())
    }
}

/// What a viewport shows: a saved 3D view or a drawing of VIEWS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacedKind {
    View,
    Drawing,
}

impl PlacedKind {
    pub fn key(self) -> &'static str {
        match self {
            Self::View => "view",
            Self::Drawing => "drawing",
        }
    }
}

/// A view placed on a sheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Viewport {
    pub id: String,
    pub kind: PlacedKind,
    /// The identifier of the view or the drawing.
    pub guid: String,
    /// The middle of the viewport on the paper.
    pub centre: [f64; 2],
    /// Its width and height. A drawing takes the size of its crop region at
    /// its scale, which is kept here as well for when it is gone.
    pub size: [f64; 2],
    /// The scale of a drawing, as the number after "1:".
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// The title under it; without one the name of the view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The name the view had when it was last seen, for when it is gone.
    #[serde(default)]
    pub name: String,
}

fn default_scale() -> f64 {
    DEFAULT_SCALE
}

impl Viewport {
    pub fn new(kind: PlacedKind, guid: &str, name: &str, centre: [f64; 2], size: [f64; 2]) -> Self {
        Self {
            id: camera_views::new_guid(),
            kind,
            guid: guid.to_owned(),
            centre,
            size,
            scale: DEFAULT_SCALE,
            title: None,
            name: name.to_owned(),
        }
    }

    /// Its lower left and upper right corner.
    pub fn rect(&self) -> [[f64; 2]; 2] {
        rect_around(self.centre, self.size)
    }

    /// The title it shows: the one typed, else the name of the view.
    pub fn shown_title(&self) -> &str {
        self.title
            .as_deref()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or(&self.name)
    }

    fn valid(&self) -> bool {
        let finite = |pair: [f64; 2]| pair.iter().all(|value| value.is_finite());
        camera_views::is_guid(&self.id)
            && camera_views::is_guid(&self.guid)
            && finite(self.centre)
            && finite(self.size)
            && self.size.iter().all(|side| *side > 0.0)
            && scale_valid(self.scale)
    }
}

/// A sheet: paper with a border, a title block and the views placed on it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layout {
    pub guid: String,
    pub number: String,
    pub name: String,
    pub paper: Paper,
    pub landscape: bool,
    /// The fields of the title block besides the name, the number and the
    /// scale.
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub drawn_by: String,
    #[serde(default)]
    pub viewports: Vec<Viewport>,
    /// Seconds since 1970, UTC.
    #[serde(default)]
    pub created: u64,
}

impl Layout {
    pub fn new(number: &str, name: &str, paper: Paper, landscape: bool) -> Self {
        let created = camera_views::now_seconds();
        Self {
            guid: camera_views::new_guid(),
            number: number.trim().to_owned(),
            name: name.trim().to_owned(),
            paper,
            landscape,
            project: String::new(),
            date: crate::bcf::timestamp(created)[..10].to_owned(),
            drawn_by: crate::views::author(),
            viewports: Vec::new(),
            created,
        }
    }

    /// Width and height of the paper.
    pub fn size(&self) -> [f64; 2] {
        self.paper.size(self.landscape)
    }

    /// The border: the paper less `MARGIN` on every side.
    pub fn border(&self) -> [[f64; 2]; 2] {
        let [width, height] = self.size();
        [[MARGIN, MARGIN], [width - MARGIN, height - MARGIN]]
    }

    /// The title block in the lower right corner inside the border.
    pub fn title_block(&self) -> [[f64; 2]; 2] {
        let [_, max] = self.border();
        [
            [max[0] - TITLE_BLOCK[0], MARGIN],
            [max[0], MARGIN + TITLE_BLOCK[1]],
        ]
    }

    /// What the title block says the scale is: the scale of the drawings
    /// on it when they share one, else "as indicated"; nothing without a
    /// drawing.
    pub fn scale_text(&self) -> Option<String> {
        let mut scales: Vec<f64> = self
            .viewports
            .iter()
            .filter(|viewport| viewport.kind == PlacedKind::Drawing)
            .map(|viewport| viewport.scale)
            .collect();
        scales.sort_by(f64::total_cmp);
        scales.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        match scales.as_slice() {
            [] => None,
            [only] => Some(scale_label(*only)),
            _ => Some(crate::i18n::tr("as indicated").to_owned()),
        }
    }

    pub fn viewport(&self, id: &str) -> Option<&Viewport> {
        self.viewports.iter().find(|viewport| viewport.id == id)
    }

    pub fn viewport_mut(&mut self, id: &str) -> Option<&mut Viewport> {
        self.viewports.iter_mut().find(|viewport| viewport.id == id)
    }

    /// The name a tab and a row show: the number and the name.
    pub fn caption(&self) -> String {
        if self.number.is_empty() {
            self.name.clone()
        } else {
            format!("{} {}", self.number, self.name)
        }
    }

    /// Where a viewport of `size` goes when no place is given: the first
    /// spot, from the upper left, inside the border that leaves the title
    /// block and the other viewports free; else the middle of the paper
    /// above the title block.
    pub fn free_place(&self, size: [f64; 2], except: Option<&str>) -> [f64; 2] {
        let [min, max] = self.border();
        let taken: Vec<[[f64; 2]; 2]> = self
            .viewports
            .iter()
            .filter(|viewport| Some(viewport.id.as_str()) != except)
            .map(Viewport::rect)
            .chain(std::iter::once(self.title_block()))
            .collect();
        let step = 5.0;
        let inner = [[min[0] + GAP, min[1] + GAP], [max[0] - GAP, max[1] - GAP]];
        let [width, height] = size;
        let mut y = inner[1][1] - height / 2.0;
        while y - height / 2.0 - TITLE_ROOM >= inner[0][1] {
            let mut x = inner[0][0] + width / 2.0;
            while x + width / 2.0 <= inner[1][0] {
                // The viewport with the room for its title under it.
                let rect = [
                    [x - width / 2.0, y - height / 2.0 - TITLE_ROOM],
                    [x + width / 2.0, y + height / 2.0],
                ];
                if taken.iter().all(|other| !overlap(grown(*other, GAP), rect)) {
                    return [x, y];
                }
                x += step;
            }
            y -= step;
        }
        let block = self.title_block();
        [(min[0] + max[0]) / 2.0, (block[1][1] + max[1]) / 2.0]
    }

    fn valid(&self) -> bool {
        camera_views::is_guid(&self.guid)
            && !self.name.trim().is_empty()
            && self.name.chars().count() <= MAX_NAME_CHARS
            && self.number.chars().count() <= MAX_NUMBER_CHARS
    }

    /// Drop the viewports this version cannot use.
    fn repaired(mut self) -> Self {
        self.viewports.retain(Viewport::valid);
        self.viewports.truncate(MAX_VIEWPORTS);
        self
    }
}

/// "1:100" for a scale of 100.
pub fn scale_label(scale: f64) -> String {
    if (scale - scale.round()).abs() < 1e-9 {
        format!("1:{}", scale.round() as u64)
    } else {
        format!("1:{scale}")
    }
}

/// The scale a text names: "1:100", "100" or "1/100".
pub fn parse_scale(text: &str) -> Option<f64> {
    let text = text.trim().replace(' ', "");
    let number = text
        .strip_prefix("1:")
        .or_else(|| text.strip_prefix("1/"))
        .unwrap_or(&text);
    let scale: f64 = number.replace(',', ".").parse().ok()?;
    scale_valid(scale).then_some(scale)
}

pub fn scale_valid(scale: f64) -> bool {
    scale.is_finite() && (MIN_SCALE..=MAX_SCALE).contains(&scale)
}

/// Millimetres on the paper for `metres` in the model at 1:`scale`: a metre
/// is ten millimetres at 1:100.
pub fn paper_mm(metres: f64, scale: f64) -> f64 {
    metres * 1000.0 / scale
}

/// The size of a drawing on the paper: its crop region, `rect` in metres,
/// at 1:`scale`.
pub fn drawing_size(rect: [[f64; 2]; 2], scale: f64) -> [f64; 2] {
    [
        paper_mm((rect[1][0] - rect[0][0]).abs(), scale),
        paper_mm((rect[1][1] - rect[0][1]).abs(), scale),
    ]
}

/// The size of the image of a 3D view of `pixels` at `IMAGE_DPI`, made
/// smaller to fit within `room` with its proportions kept.
pub fn image_size(pixels: [u32; 2], room: [f64; 2]) -> [f64; 2] {
    let [width, height] = pixels.map(|pixels| f64::from(pixels.max(1)) * 25.4 / IMAGE_DPI);
    let fit = (room[0] / width).min(room[1] / height).min(1.0);
    [(width * fit).max(MIN_SIDE), (height * fit).max(MIN_SIDE)]
}

/// The dots per inch an image of `pixels` gets at `size` millimetres.
pub fn dots_per_inch(pixels: [u32; 2], size: [f64; 2]) -> f64 {
    let across = f64::from(pixels[0].max(1)) / (size[0].max(1e-9) / 25.4);
    let up = f64::from(pixels[1].max(1)) / (size[1].max(1e-9) / 25.4);
    across.min(up)
}

pub fn rect_around(centre: [f64; 2], size: [f64; 2]) -> [[f64; 2]; 2] {
    [
        [centre[0] - size[0] / 2.0, centre[1] - size[1] / 2.0],
        [centre[0] + size[0] / 2.0, centre[1] + size[1] / 2.0],
    ]
}

fn grown(rect: [[f64; 2]; 2], by: f64) -> [[f64; 2]; 2] {
    [
        [rect[0][0] - by, rect[0][1] - by],
        [rect[1][0] + by, rect[1][1] + by],
    ]
}

pub fn overlap(a: [[f64; 2]; 2], b: [[f64; 2]; 2]) -> bool {
    (0..2).all(|axis| a[0][axis] < b[1][axis] && b[0][axis] < a[1][axis])
}

pub fn contains(rect: [[f64; 2]; 2], point: [f64; 2]) -> bool {
    (0..2).all(|axis| rect[0][axis] <= point[axis] && point[axis] <= rect[1][axis])
}

/// The width and height of a PNG image, from its header.
pub fn png_size(png: &[u8]) -> Option<[u32; 2]> {
    if png.len() < 24 || &png[..8] != b"\x89PNG\r\n\x1a\n" || &png[12..16] != b"IHDR" {
        return None;
    }
    let read = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    Some([read(16), read(20)]).filter(|size| size[0] > 0 && size[1] > 0)
}

fn sheets_path() -> Option<PathBuf> {
    camera_views::directory().map(|directory| directory.join("sheets.json"))
}

pub fn load() -> Vec<Layout> {
    sheets_path().map_or_else(Vec::new, |path| load_from(&path))
}

pub fn save(layouts: &[Layout]) -> io::Result<()> {
    let path = sheets_path().ok_or_else(|| io::Error::other("no user config directory"))?;
    camera_views::write_json(&path, &layouts)
}

/// The sheets in a file, each read on its own, so that one this version
/// cannot read does not take the others with it. A file that cannot be
/// read at all is kept beside it before the next save replaces it.
pub(crate) fn load_from(path: &Path) -> Vec<Layout> {
    let entries = match camera_views::read_entries(path) {
        Entries::Missing => return Vec::new(),
        Entries::Unreadable => {
            let _ = std::fs::copy(path, path.with_extension("unreadable.json"));
            return Vec::new();
        }
        Entries::Read(entries) => entries,
    };
    let mut layouts: Vec<Layout> = Vec::new();
    for layout in entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value::<Layout>(entry).ok())
        .filter(Layout::valid)
        .map(Layout::repaired)
    {
        if !layouts.iter().any(|known| known.guid == layout.guid) && layouts.len() < MAX_SHEETS {
            layouts.push(layout);
        }
    }
    layouts
}
