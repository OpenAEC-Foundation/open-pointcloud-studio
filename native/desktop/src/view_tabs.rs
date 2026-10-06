//! The tabs above the viewport, side by side as the pages of a document: the
//! 3D model first, which never closes, and after it every view and drawing
//! opened from VIEWS of the Project Browser, in the order they were opened.
//! A click on a tab shows it; its × or a middle click closes the tab and
//! never the view or the drawing; Ctrl+Tab and Ctrl+Shift+Tab step through
//! the tabs. The tab of what the window shows is the active one, and its row
//! of VIEWS is the one highlighted.
//!
//! The 3D model keeps its own camera, section box and colour mode while a
//! saved view is shown in the scene, and gets them back when its tab is shown
//! again; a drawing keeps where its sheet was zoomed to, which the Drawing
//! view remembers. The open tabs and the active one are kept in the
//! preferences beside the collapsed groups of the Project Browser; the tab of
//! a preview, an export or an opened file lasts for the session, as its row
//! does.

use std::path::{Path, PathBuf};

use iced::widget::scrollable::{self, AbsoluteOffset, Direction, Scrollbar};
use iced::widget::{button, container, mouse_area, row, text, tooltip};
use iced::{Background, Border, Color, Element, Fill, Font, Padding, Task, Theme};
use pointcloud_core::Bounds;
use serde_json::{json, Value};

use crate::i18n::tr;
use crate::project_browser::{ViewKind, ViewRow};
use crate::station_photos::WalkView;
use crate::{icon_svg, ui_theme, ColorMode, Message, Studio, ToolIcon};

/// A tab, by what it shows.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TabId {
    /// The 3D model, the first tab, which never closes.
    Model,
    /// A saved 3D view, by its identifier.
    View(String),
    /// A plan, an elevation or a section of Create 2D, by the identifier of
    /// how it was made.
    Drawing(String),
    /// A preview, an export or an opened DXF or DWG file of this session, by
    /// its path; the preview has none.
    File(Option<PathBuf>),
    /// A sheet of SHEETS, by its identifier.
    Layout(String),
}

/// How the preferences name the 3D model, and how they start the name of a
/// saved view and of a drawing.
const MODEL_KEY: &str = "model";
/// The name of the tab of the 3D model in English, which the local API
/// gives it in every language.
const MODEL_NAME: &str = "3D model";
const VIEW_KEY: &str = "view:";
const DRAWING_KEY: &str = "drawing:";
const LAYOUT_KEY: &str = "layout:";

impl TabId {
    /// The name the preferences keep the tab by; none for a preview, an
    /// export or a file, which last for the session.
    pub fn key(&self) -> Option<String> {
        match self {
            Self::Model => Some(MODEL_KEY.to_owned()),
            Self::View(guid) => Some(format!("{VIEW_KEY}{guid}")),
            Self::Drawing(guid) => Some(format!("{DRAWING_KEY}{guid}")),
            Self::Layout(guid) => Some(format!("{LAYOUT_KEY}{guid}")),
            Self::File(_) => None,
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        let key = key.trim();
        if key == MODEL_KEY {
            return Some(Self::Model);
        }
        let guid = |prefix: &str| {
            key.strip_prefix(prefix)
                .filter(|guid| !guid.is_empty())
                .map(str::to_owned)
        };
        guid(VIEW_KEY)
            .map(Self::View)
            .or_else(|| guid(DRAWING_KEY).map(Self::Drawing))
            .or_else(|| guid(LAYOUT_KEY).map(Self::Layout))
    }

    /// What the local API calls the kind of the tab.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::View(_) => "view",
            Self::Drawing(_) => "drawing",
            Self::File(_) => "file",
            Self::Layout(_) => "sheet",
        }
    }

    /// Every tab but that of the 3D model closes.
    pub fn closable(&self) -> bool {
        !matches!(self, Self::Model)
    }

    /// Whether the tab shows the 3D scene: the 3D model or a saved view.
    pub fn is_3d(&self) -> bool {
        matches!(self, Self::Model | Self::View(_))
    }

    /// The key under which the Drawing view remembers where it looked at
    /// the drawing of this tab.
    fn look_key(&self) -> Option<String> {
        match self {
            Self::Model | Self::View(_) | Self::Layout(_) => None,
            Self::Drawing(guid) => Some(crate::drawing_view::sheet_look_key(guid)),
            Self::File(path) => Some(crate::drawing_view::file_look_key(path.as_deref())),
        }
    }
}

/// At most this many tabs are open after the 3D model; the oldest closes
/// first.
pub const MAX_TABS: usize = 32;
/// The pixels a character of the name of a tab takes, about.
const CHAR_WIDTH: f32 = 6.3;
/// The pixels of a tab besides its name: its padding, its icon, the gap
/// after it and the gap between two tabs.
const TAB_CHROME: f32 = 37.0;
/// The pixels the × of a tab adds.
const CLOSE_WIDTH: f32 = 22.0;
/// A name is shortened to no fewer characters than this before the strip
/// scrolls, and is never shown longer than `MAX_CHARS`.
const MIN_CHARS: usize = 8;
const MAX_CHARS: usize = 32;
/// The height of a tab, without the accent line over the active one.
const TAB_HEIGHT: f32 = 26.0;
/// The caption at the right of the strip is shortened to this many
/// characters.
const CAPTION_CHARS: usize = 28;

/// How wide a tab is with a name of so many characters.
pub fn tab_width(chars: usize, closable: bool) -> f32 {
    TAB_CHROME + chars as f32 * CHAR_WIDTH + if closable { CLOSE_WIDTH } else { 0.0 }
}

/// A name shortened to at most `most` characters, the last of them an
/// ellipsis.
pub fn shortened(name: &str, most: usize) -> String {
    if name.chars().count() <= most {
        return name.to_owned();
    }
    let kept: String = name.chars().take(most.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// A name shortened to at most `most` characters in its middle, so that
/// its end stays: its last word when that leaves three characters before
/// the ellipsis, such as the number a copy gets, else its last few
/// characters.
pub fn shortened_in_the_middle(name: &str, most: usize) -> String {
    let characters: Vec<char> = name.chars().collect();
    if characters.len() <= most {
        return name.to_owned();
    }
    let room = most.saturating_sub(1);
    let last_word = characters
        .iter()
        .rposition(|character| character.is_whitespace())
        .map_or(characters.len(), |space| characters.len() - space - 1);
    let tail = if last_word > 0 && last_word <= room.saturating_sub(3) {
        last_word
    } else {
        room / 2
    };
    let head: String = characters[..room - tail].iter().collect();
    let end: String = characters[characters.len() - tail..].iter().collect();
    format!("{}…{end}", head.trim_end())
}

/// The names of tabs, each with whether it closes, as a strip of
/// `available` pixels shows them: as they are while they fit, else all
/// shortened to the same most characters, which takes the longest first,
/// down to `MIN_CHARS`. Names that would then read the same, as copies of
/// a view do, are shortened in their middle instead. Also whether the tabs
/// are wider than the strip even so, and it scrolls.
pub fn fitted_names(names: &[(String, bool)], available: f32) -> (Vec<String>, bool) {
    let total = |most: usize| -> f32 {
        names
            .iter()
            .map(|(name, closable)| tab_width(name.chars().count().min(most), *closable))
            .sum()
    };
    let mut most = MAX_CHARS;
    while most > MIN_CHARS && total(most) > available {
        most -= 1;
    }
    let mut shown: Vec<String> = names
        .iter()
        .map(|(name, _)| shortened(name, most))
        .collect();
    let alike: Vec<bool> = (0..names.len())
        .map(|place| {
            (0..names.len()).any(|other| {
                other != place && shown[other] == shown[place] && names[other].0 != names[place].0
            })
        })
        .collect();
    for (place, (name, _)) in names.iter().enumerate() {
        if alike[place] {
            shown[place] = shortened_in_the_middle(name, most);
        }
    }
    (shown, total(most) > available)
}

/// Where the strip scrolls to for the tab at `place` of tabs `widths`
/// wide, in a strip `available` wide: to the start of the tab before it,
/// so that a whole tab shows at its left, when both fit; else to the start
/// of the tab itself.
pub fn scroll_offset(widths: &[f32], place: usize, available: f32) -> f32 {
    let before: f32 = widths[..place].iter().sum();
    let shown = widths.get(place).copied().unwrap_or(0.0);
    match place.checked_sub(1).map(|previous| widths[previous]) {
        Some(previous) if previous + shown <= available => before - previous,
        _ => before,
    }
}

/// The tab shown in place of a closed one that was shown: of the tabs as
/// the strip lists them, the one after it, else the one before it.
pub fn after_closing(listed: &[TabId], closed: &TabId) -> TabId {
    let Some(place) = listed.iter().position(|tab| tab == closed) else {
        return TabId::Model;
    };
    listed
        .get(place + 1)
        .or_else(|| place.checked_sub(1).and_then(|before| listed.get(before)))
        .cloned()
        .unwrap_or(TabId::Model)
}

/// The tab Ctrl+Tab (`forward`) or Ctrl+Shift+Tab goes to from the active
/// one, round from the last to the first and back; without an active tab
/// the first or the last.
pub fn cycled(listed: &[TabId], active: Option<&TabId>, forward: bool) -> Option<TabId> {
    let count = listed.len();
    if count == 0 {
        return None;
    }
    let next = match active.and_then(|active| listed.iter().position(|tab| tab == active)) {
        Some(place) if forward => (place + 1) % count,
        Some(place) => (place + count - 1) % count,
        None if forward => 0,
        None => count - 1,
    };
    listed.get(next).cloned()
}

/// What the 3D model shows besides the scans: its camera, its section box
/// and its colour mode. A saved view shown in the scene sets its own; the 3D
/// model gets these back when its tab is shown again.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ModelLook {
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 2],
    orbit_point: Option<[f64; 3]>,
    walk: Option<WalkView>,
    label: &'static str,
    section_enabled: bool,
    section_reference: Option<Bounds>,
    section_min: [f64; 3],
    section_max: [f64; 3],
    section_rotation: f64,
    color_mode: ColorMode,
}

/// The open tabs and what they keep.
#[derive(Debug)]
pub struct ViewTabs {
    /// The tabs after the 3D model, in the order they were opened; also
    /// those of views and drawings of scans that are not open or not active
    /// now, which come back with their scan.
    open: Vec<TabId>,
    /// The tab of what the window showed when the tabs last followed it.
    shown: Option<TabId>,
    /// The tab the preferences say was active: it is shown once what it
    /// shows is there and the scans are read, unless another is chosen
    /// first.
    pending: Option<TabId>,
    /// The 3D tab whose camera the scene has: the 3D model, or the saved
    /// view shown last.
    owner: TabId,
    /// The camera, the section box and the colour mode of the 3D model, as
    /// they were when the scene was last its own.
    model: Option<ModelLook>,
    /// The open tabs and the active one as the preferences last got them.
    kept: (Vec<String>, Option<String>),
    /// A tab opened or closed, or the one kept as active let go, since the
    /// preferences last got the tabs.
    changed: bool,
}

impl Default for ViewTabs {
    fn default() -> Self {
        Self::new(&[], None)
    }
}

impl ViewTabs {
    /// The tabs the preferences kept, and the one that was active.
    pub fn new(keys: &[String], active: Option<&str>) -> Self {
        let mut tabs = Self {
            open: Vec::new(),
            shown: None,
            pending: None,
            owner: TabId::Model,
            model: None,
            kept: (Vec::new(), None),
            changed: false,
        };
        for key in keys {
            if let Some(tab) = TabId::from_key(key) {
                tabs.add(tab);
            }
        }
        tabs.pending = active
            .and_then(TabId::from_key)
            .filter(|tab| *tab != TabId::Model);
        if let Some(tab) = tabs.pending.clone() {
            tabs.add(tab);
        }
        tabs.kept = tabs.kept_state();
        tabs.changed = false;
        tabs
    }

    /// The tabs after the 3D model, in the order they were opened.
    pub fn open(&self) -> &[TabId] {
        &self.open
    }

    /// Open a tab after the others; false when it is open already or is the
    /// 3D model, which always is. Past `MAX_TABS` the oldest closes.
    pub fn add(&mut self, tab: TabId) -> bool {
        if tab == TabId::Model || self.open.contains(&tab) {
            return false;
        }
        self.open.push(tab);
        self.changed = true;
        if self.open.len() > MAX_TABS {
            self.open.remove(0);
        }
        true
    }

    /// Close a tab; false when it was not open.
    pub fn remove(&mut self, tab: &TabId) -> bool {
        let before = self.open.len();
        self.open.retain(|open| open != tab);
        if self.pending.as_ref() == Some(tab) {
            self.drop_pending();
        }
        self.changed |= self.open.len() != before;
        self.open.len() != before
    }

    /// Let go of the tab kept as active: another was chosen, or it is gone.
    fn drop_pending(&mut self) {
        if self.pending.take().is_some() {
            self.changed = true;
        }
    }

    /// The open tabs and the active one as the preferences keep them: the
    /// one waiting to be shown again, else the one shown.
    fn kept_state(&self) -> (Vec<String>, Option<String>) {
        let active = self
            .pending
            .as_ref()
            .or(self.shown.as_ref())
            .and_then(TabId::key);
        (self.open.iter().filter_map(TabId::key).collect(), active)
    }

    /// The open tabs and the active one, as the preferences keep them.
    pub fn kept(&self) -> &(Vec<String>, Option<String>) {
        &self.kept
    }
}

/// How the strip of tabs lays out; see `Studio::strip_layout`.
struct StripLayout {
    listed: Vec<TabId>,
    names: Vec<(String, bool)>,
    fitted: Vec<String>,
    caption: Option<(&'static str, String)>,
    /// The pixels the tabs have.
    available: f32,
}

/// What a tab does on a click, on its × and on the keys.
#[derive(Debug, Clone)]
pub enum TabAction {
    Show(TabId),
    Close(TabId),
    /// Ctrl+Tab steps to the next tab (true), Ctrl+Shift+Tab to the one
    /// before (false).
    Cycle(bool),
}

/// The scrollable that holds the tabs.
fn strip_id() -> scrollable::Id {
    scrollable::Id::new("view-tabs")
}

impl Studio {
    pub(crate) fn update_tabs(&mut self, action: TabAction) -> Task<Message> {
        match action {
            TabAction::Show(tab) => match self.show_tab(&tab) {
                Ok(task) => task,
                Err(reason) => {
                    self.status = reason;
                    Task::none()
                }
            },
            TabAction::Close(tab) => self.close_tab(&tab),
            TabAction::Cycle(forward) => {
                // Not under what covers the tabs or the main area: the File
                // view, Settings, the dialog of Create 2D and the card of
                // the Pointcloud to Drawing wizard.
                if self.tabs_covered() {
                    return Task::none();
                }
                let listed = self.listed_tabs();
                match cycled(&listed, self.shown_tab().as_ref(), forward) {
                    Some(tab) => self.update_tabs(TabAction::Show(tab)),
                    None => Task::none(),
                }
            }
        }
    }

    /// Whether the File view, Settings, the dialog of Create 2D or the card
    /// of the Pointcloud to Drawing wizard lies over the tabs or what they show.
    pub(crate) fn tabs_covered(&self) -> bool {
        self.model_covered() || self.sheet_dialog.is_some()
    }

    /// The tab of what the window shows: the drawing in the Drawing view,
    /// else the active view, else the 3D model; none while the Drawing view
    /// holds no drawing.
    pub(crate) fn shown_tab(&self) -> Option<TabId> {
        if let Some(guid) = self.drawing_view.shown_layout() {
            return Some(TabId::Layout(guid.to_owned()));
        }
        Some(match self.shown_row()? {
            ViewRow::Model => TabId::Model,
            ViewRow::Saved(guid) => TabId::View(guid),
            ViewRow::Drawing(guid) => TabId::Drawing(guid),
            ViewRow::File(place) => TabId::File(
                self.drawing_view
                    .sheets()
                    .get(place)?
                    .source
                    .path()
                    .map(Path::to_path_buf),
            ),
        })
    }

    /// The place under Files of the preview (no path) or of the export or
    /// file with this path.
    fn sheet_place(&self, path: Option<&Path>) -> Option<usize> {
        self.drawing_view
            .sheets()
            .iter()
            .position(|sheet| sheet.source.path() == path)
    }

    /// Whether what a tab shows is still kept: a view or a drawing not
    /// deleted, a file still under Files.
    fn tab_exists(&self, tab: &TabId) -> bool {
        match tab {
            TabId::Model => true,
            TabId::View(guid) => self.views.list.iter().any(|view| view.guid == *guid),
            TabId::Drawing(guid) => self
                .drawing_view
                .saved
                .iter()
                .any(|drawing| drawing.guid == *guid),
            TabId::File(path) => self.sheet_place(path.as_deref()).is_some(),
            TabId::Layout(guid) => self.layouts.layout(guid).is_some(),
        }
    }

    /// The tabs the strip shows, in its order: the 3D model, then the open
    /// tabs of what VIEWS lists now. A view of a scan that is not the active
    /// one, and a drawing of scans that are not open, wait for their scan.
    pub(crate) fn listed_tabs(&self) -> Vec<TabId> {
        let views = self.listed_views();
        let drawings = self.listed_drawings();
        let listed = |tab: &&TabId| match tab {
            TabId::Model => false,
            TabId::View(guid) => views.iter().any(|view| view.guid == *guid),
            TabId::Drawing(guid) => drawings.iter().any(|drawing| drawing.guid == *guid),
            TabId::File(path) => self.sheet_place(path.as_deref()).is_some(),
            TabId::Layout(guid) => self.layouts.layout(guid).is_some(),
        };
        std::iter::once(TabId::Model)
            .chain(self.tabs.open().iter().filter(listed).cloned())
            .collect()
    }

    /// The name of a tab: that of its row under VIEWS.
    pub(crate) fn tab_name(&self, tab: &TabId) -> String {
        match tab {
            TabId::Model => tr("3D model").to_owned(),
            TabId::View(guid) => self
                .views
                .list
                .iter()
                .find(|view| view.guid == *guid)
                .map_or_else(String::new, |view| view.name.clone()),
            TabId::Drawing(guid) => self
                .drawing_view
                .saved
                .iter()
                .find(|drawing| drawing.guid == *guid)
                .map_or_else(String::new, |drawing| drawing.name.clone()),
            TabId::File(path) => self
                .sheet_place(path.as_deref())
                .map_or_else(String::new, |place| {
                    self.drawing_view.sheets()[place].source.caption()
                }),
            TabId::Layout(guid) => self
                .layouts
                .layout(guid)
                .map_or_else(String::new, crate::layouts::Layout::caption),
        }
    }

    /// The name the local API gives a tab: that of its row, and `3D model`
    /// for the 3D model in every language, as `project_browser` names it.
    fn tab_api_name(&self, tab: &TabId) -> String {
        match tab {
            TabId::Model => MODEL_NAME.to_owned(),
            _ => self.tab_name(tab),
        }
    }

    /// The kind of VIEWS a tab is listed under.
    fn tab_kind(&self, tab: &TabId) -> ViewKind {
        match tab {
            TabId::Model | TabId::View(_) => ViewKind::ThreeD,
            TabId::Drawing(guid) => self
                .drawing_view
                .saved
                .iter()
                .find(|drawing| drawing.guid == *guid)
                .map_or(ViewKind::Plans, |drawing| ViewKind::of(drawing.kind)),
            TabId::File(_) | TabId::Layout(_) => ViewKind::Files,
        }
    }

    fn tab_icon(&self, tab: &TabId) -> ToolIcon {
        match tab {
            TabId::Model => ToolIcon::Model,
            TabId::View(_) => ToolIcon::SavedView,
            TabId::Layout(_) => ToolIcon::Sheet,
            _ => self.tab_kind(tab).icon(),
        }
    }

    /// Show what a tab shows, as a click on it does. The 3D model gets its
    /// own camera back; a saved view that the scene still has is shown as it
    /// was left, else as it was saved; a drawing comes back zoomed as it was
    /// left, and one that is not made in this session yet is made.
    pub(crate) fn show_tab(&mut self, tab: &TabId) -> Result<Task<Message>, String> {
        // A tab chosen by hand wins over the one kept as active.
        self.tabs.drop_pending();
        match tab {
            TabId::Model => Ok(self.show_model_tab()),
            TabId::View(guid) => {
                let listed = self.listed_views().iter().any(|view| view.guid == *guid);
                let Some(index) = self
                    .views
                    .list
                    .iter()
                    .position(|view| view.guid == *guid)
                    .filter(|_| listed)
                else {
                    return Err("That view belongs to a scan that is not the active one".into());
                };
                // The scene still has the view: it is shown as it was left.
                let still_shown =
                    self.tabs.owner == *tab && self.active_view_index() == Some(index);
                if still_shown {
                    self.drawing_view.shown = false;
                    self.file_open = false;
                    self.status = format!("View {}", self.views.list[index].name);
                    return Ok(Task::none());
                }
                Ok(self.update_views(crate::views::ViewAction::Restore(guid.clone())))
            }
            TabId::Drawing(guid) => self
                .show_saved_drawing(guid, None)
                .map(|task| task.unwrap_or_else(Task::none)),
            TabId::File(path) => {
                let place = self
                    .sheet_place(path.as_deref())
                    .ok_or_else(|| "That drawing is no longer listed under Files".to_owned())?;
                Ok(self
                    .update_drawing_view(crate::drawing_view::DrawingViewAction::ShowSheet(place)))
            }
            TabId::Layout(guid) => self.show_layout(guid),
        }
    }

    /// Show the 3D model: the scene, with the camera, the section box and
    /// the colour mode of the 3D model when a saved view had the scene. The
    /// active view lets go, so that the row of the 3D model is highlighted.
    pub(crate) fn show_model_tab(&mut self) -> Task<Message> {
        self.tabs.drop_pending();
        self.drawing_view.shown = false;
        self.file_open = false;
        let task = match self.tabs.model.filter(|_| self.tabs.owner != TabId::Model) {
            Some(look) => self.put_back_model_look(look),
            None => Task::none(),
        };
        self.deactivate_view();
        self.tabs.owner = TabId::Model;
        self.status = "3D model".into();
        task
    }

    /// Close a tab, which never deletes its view or drawing. When it was
    /// the one shown, the tab after it is shown, else the one before it. A
    /// closed view that the scene still has gives the scene back to the 3D
    /// model, and a closed drawing starts from its extents when it is opened
    /// again.
    pub(crate) fn close_tab(&mut self, tab: &TabId) -> Task<Message> {
        if !tab.closable() {
            return Task::none();
        }
        let listed = self.listed_tabs();
        let shown = self.shown_tab();
        if !self.tabs.remove(tab) {
            return Task::none();
        }
        if let Some(key) = tab.look_key() {
            self.drawing_view.forget_look(&key);
        }
        let mut tasks = Vec::new();
        if self.tabs.owner == *tab {
            if let Some(look) = self.tabs.model {
                tasks.push(self.put_back_model_look(look));
            }
            self.deactivate_view();
            self.tabs.owner = TabId::Model;
        }
        if shown.as_ref() == Some(tab) {
            let next = after_closing(&listed, tab);
            tasks.push(match self.show_tab(&next) {
                Ok(task) => task,
                Err(reason) => {
                    self.status = reason;
                    self.show_model_tab()
                }
            });
        }
        Task::batch(tasks)
    }

    /// What the 3D model shows now besides the scans.
    fn model_look(&self) -> ModelLook {
        ModelLook {
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
            pan: self.pan,
            orbit_point: self.orbit_point,
            walk: self.walk,
            label: self.view_label,
            section_enabled: self.section_enabled,
            section_reference: self.section_reference_bounds,
            section_min: self.section_min_percent,
            section_max: self.section_max_percent,
            section_rotation: self.section_rotation,
            color_mode: self.color_mode,
        }
    }

    /// Give the 3D model back what it showed: its camera, also standing at
    /// a station, its section box and its colour mode.
    fn put_back_model_look(&mut self, look: ModelLook) -> Task<Message> {
        let mut tasks = Vec::new();
        self.leave_walk();
        self.yaw = look.yaw;
        self.pitch = look.pitch;
        self.zoom = look.zoom;
        self.pan = look.pan;
        self.orbit_point = look.orbit_point;
        self.view_label = look.label;
        if let Some(walk) = look.walk {
            self.walk = Some(walk);
            tasks.push(self.sync_walk_station());
        }
        self.section_enabled = look.section_enabled;
        self.section_reference_bounds = look.section_reference;
        self.section_min_percent = look.section_min;
        self.section_max_percent = look.section_max;
        self.set_section_rotation_value(look.section_rotation);
        self.sync_section_coordinate_inputs();
        if look.color_mode != self.color_mode {
            self.color_mode = look.color_mode;
            tasks.push(self.queue_preferences_save());
        }
        self.revision += 1;
        tasks.push(self.schedule_detail());
        Task::batch(tasks)
    }

    /// Follow what the window shows, after every message: the tab of what
    /// came to be shown opens, the 3D model keeps what it shows while the
    /// scene is its own, the tab the preferences kept as active is shown
    /// once it can be, tabs of what was deleted close, and the preferences
    /// get the tabs when they changed. Answers the work this needs, if any.
    pub(crate) fn settle_tabs(&mut self) -> Option<Task<Message>> {
        let mut tasks = Vec::new();
        // A saved view that had the scene and was deleted gives the scene
        // back to the 3D model, as closing its tab does, before the 3D model
        // takes what the scene shows as its own.
        if matches!(self.tabs.owner, TabId::View(_)) && !self.tab_exists(&self.tabs.owner) {
            if let Some(look) = self.tabs.model {
                tasks.push(self.put_back_model_look(look));
            }
            self.tabs.owner = TabId::Model;
        }
        let mut moved = self.follow_shown_tab();
        if let Some(task) = self.show_pending_tab() {
            tasks.push(task);
            moved |= self.follow_shown_tab();
        }
        let gone: Vec<TabId> = self
            .tabs
            .open()
            .iter()
            .filter(|tab| !self.tab_exists(tab))
            .cloned()
            .collect();
        for tab in gone {
            self.tabs.remove(&tab);
            if let Some(key) = tab.look_key() {
                self.drawing_view.forget_look(&key);
            }
        }
        if moved {
            if let Some(task) = self.scroll_to_shown_tab() {
                tasks.push(task);
            }
        }
        if moved || self.tabs.changed {
            self.tabs.changed = false;
            let kept = self.tabs.kept_state();
            if kept != self.tabs.kept {
                self.tabs.kept = kept;
                tasks.push(self.queue_preferences_save());
            }
        }
        (!tasks.is_empty()).then(|| Task::batch(tasks))
    }

    /// Open the tab of what came to be shown, and keep what the 3D model
    /// shows while the scene is its own. Reports whether another tab is
    /// shown than when the tabs last followed.
    fn follow_shown_tab(&mut self) -> bool {
        let shown = self.shown_tab();
        let moved = shown != self.tabs.shown;
        if moved {
            if let Some(tab) = &shown {
                self.tabs.add(tab.clone());
                // Something else was chosen than the tab kept as active.
                if *tab != TabId::Model {
                    self.tabs.drop_pending();
                }
            }
            self.tabs.shown.clone_from(&shown);
        }
        if let Some(tab) = shown.filter(TabId::is_3d) {
            self.tabs.owner = tab;
        }
        if self.tabs.owner == TabId::Model {
            self.tabs.model = Some(self.model_look());
        }
        moved
    }

    /// Show the tab the preferences kept as active, once what it shows is
    /// listed and no scan is still being read; while the 3D model is still
    /// what the window shows.
    fn show_pending_tab(&mut self) -> Option<Task<Message>> {
        let pending = self.tabs.pending.clone()?;
        if !self.tab_exists(&pending) {
            self.tabs.drop_pending();
            return None;
        }
        let ready = self.shown_tab() == Some(TabId::Model)
            && self.imports.is_empty()
            && !self.drawing.busy()
            && self.listed_tabs().contains(&pending);
        if !ready {
            return None;
        }
        self.tabs.drop_pending();
        Some(self.show_tab(&pending).unwrap_or_else(|reason| {
            self.status = reason;
            Task::none()
        }))
    }

    /// The width the strip has for its tabs and its caption: that of the
    /// window between the Project Browser and Properties, else as the main
    /// area was last drawn.
    fn strip_width(&self) -> f32 {
        if let Some(window) = self.window_size {
            return window.width - crate::project_browser::WIDTH - self.properties_width();
        }
        self.drawing_view
            .canvas_bounds()
            .filter(|_| self.drawing_view.shown)
            .map_or_else(|| self.drawn_viewport().width, |bounds| bounds.width)
    }

    /// The title and the caption at the right of the strip: the model space
    /// and how the camera looks, or the drawing and what it is.
    fn strip_caption(&self) -> (&'static str, String) {
        if let Some(caption) = self.layout_caption() {
            return (tr("SHEET"), shortened(&caption, CAPTION_CHARS));
        }
        if self.drawing_view.shown {
            (
                tr("DRAWING"),
                shortened(&self.drawing_view_caption(), CAPTION_CHARS),
            )
        } else {
            (tr("MODEL SPACE"), self.view_caption().to_owned())
        }
    }

    /// How the strip lays out: the tabs it lists with their names and
    /// whether they close, the names as it shows them, and its caption,
    /// which gives way to the tabs when they do not fit beside it.
    fn strip_layout(&self) -> StripLayout {
        let listed = self.listed_tabs();
        let names: Vec<(String, bool)> = listed
            .iter()
            .map(|tab| (self.tab_name(tab), tab.closable()))
            .collect();
        let width = self.strip_width() - 12.0;
        let (title, caption) = self.strip_caption();
        let caption_width =
            (title.chars().count() + caption.chars().count()) as f32 * CHAR_WIDTH + 40.0;
        let (fitted, scrolls) = fitted_names(&names, width - caption_width);
        let (fitted, caption, available) = if scrolls {
            (fitted_names(&names, width).0, None, width)
        } else {
            (fitted, Some((title, caption)), width - caption_width)
        };
        StripLayout {
            listed,
            names,
            fitted,
            caption,
            available,
        }
    }

    /// Scroll the strip to the tab shown. A strip whose tabs fit does not
    /// move: its offset never passes the width the tabs need.
    fn scroll_to_shown_tab(&self) -> Option<Task<Message>> {
        let shown = self.tabs.shown.as_ref()?;
        let layout = self.strip_layout();
        let place = layout.listed.iter().position(|tab| tab == shown)?;
        let widths: Vec<f32> = layout
            .fitted
            .iter()
            .zip(&layout.names)
            .map(|(name, (_, closable))| tab_width(name.chars().count(), *closable))
            .collect();
        Some(scrollable::scroll_to(
            strip_id(),
            AbsoluteOffset {
                x: scroll_offset(&widths, place, layout.available).max(0.0),
                y: 0.0,
            },
        ))
    }

    /// The strip of tabs above the main area, with at its right what the
    /// main area shows.
    pub(crate) fn tab_strip(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let shown = self.shown_tab();
        let StripLayout {
            listed,
            names,
            fitted,
            caption,
            ..
        } = self.strip_layout();
        let surface = active_surface(self.ui_theme, self.drawing_view.shown);
        let mut tabs = row![].spacing(2).align_y(iced::Alignment::End);
        for ((tab, (full, _)), short) in listed.iter().zip(names).zip(fitted) {
            let active = (shown.as_ref() == Some(tab)).then_some(surface);
            tabs = tabs.push(self.tab_button(tab.clone(), full, short, active));
        }
        let strip = scrollable::Scrollable::with_direction(
            tabs,
            Direction::Horizontal(Scrollbar::new().width(3).scroller_width(3).margin(0)),
        )
        .id(strip_id())
        .width(Fill);
        let mut line = row![strip].align_y(iced::Alignment::End);
        if let Some((title, caption)) = caption {
            line = line.push(
                container(
                    row![
                        text(title)
                            .size(11)
                            .font(Font::with_name("Space Grotesk"))
                            .color(colors.text),
                        text(caption)
                            .size(11)
                            .color(colors.muted)
                            .wrapping(iced::widget::text::Wrapping::None),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                )
                .height(TAB_HEIGHT + 2.0)
                .align_y(iced::Alignment::Center)
                .padding([0, 12]),
            );
        }
        container(line)
            .padding(Padding {
                top: 4.0,
                left: 6.0,
                ..Padding::ZERO
            })
            .width(Fill)
            .style(|theme| {
                container::Style::default().background(ui_theme::colors(theme).panel_alt)
            })
            .into()
    }

    /// One tab: the icon of its kind, its name and its ×, with its full name
    /// and kind in a tooltip. The active one has the colour and the ink of
    /// what is shown under it, under a line of the accent.
    fn tab_button(
        &self,
        tab: TabId,
        full: String,
        short: String,
        active: Option<(Color, Color)>,
    ) -> Element<'_, Message> {
        let closable = tab.closable();
        let mut content = row![
            icon_svg(self.tab_icon(&tab), 13.0),
            text(short)
                .size(11)
                .wrapping(iced::widget::text::Wrapping::None),
        ]
        .spacing(6)
        .align_y(iced::Alignment::Center);
        if closable {
            content = content.push(
                button(text("×").size(12))
                    .on_press(Message::Tabs(TabAction::Close(tab.clone())))
                    .style(move |theme, status| close_style(theme, active, status))
                    .padding([0, 4]),
            );
        }
        let body = button(content)
            .on_press(Message::Tabs(TabAction::Show(tab.clone())))
            .style(move |theme, status| tab_style(theme, active, status))
            .height(TAB_HEIGHT)
            .padding(Padding {
                top: 4.0,
                bottom: 4.0,
                left: 8.0,
                right: if closable { 3.0 } else { 8.0 },
            });
        let framed = container(body)
            .padding(Padding {
                top: 2.0,
                ..Padding::ZERO
            })
            .style(move |theme| {
                let colors = ui_theme::colors(theme);
                container::Style::default()
                    .background(if active.is_some() {
                        colors.accent
                    } else {
                        Color::TRANSPARENT
                    })
                    .border(Border {
                        radius: iced::border::Radius::default().top(4),
                        ..Border::default()
                    })
            });
        let kind = match &tab {
            TabId::Layout(_) => tr("Sheets"),
            _ => tr(self.tab_kind(&tab).label()),
        };
        let tip = format!("{full}\n{kind}");
        let element: Element<'_, Message> = if closable {
            mouse_area(framed)
                .on_middle_press(Message::Tabs(TabAction::Close(tab)))
                .into()
        } else {
            framed.into()
        };
        tooltip(
            element,
            crate::project_browser::hint(tip),
            tooltip::Position::Bottom,
        )
        .gap(4)
        .into()
    }

    /// What `status.result.view_tabs` reports: the tabs the strip shows, in
    /// its order, and the place of the active one.
    pub(crate) fn tabs_value(&self) -> Value {
        let shown = self.shown_tab();
        let tabs: Vec<Value> = self
            .listed_tabs()
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let mut entry = json!({
                    "index": index,
                    "name": self.tab_api_name(tab),
                    "kind": tab.kind(),
                    "active": shown.as_ref() == Some(tab),
                    "closable": tab.closable(),
                });
                match tab {
                    TabId::View(guid) | TabId::Drawing(guid) | TabId::Layout(guid) => {
                        entry["guid"] = json!(guid)
                    }
                    TabId::File(path) => entry["path"] = json!(path),
                    TabId::Model => {}
                }
                entry
            })
            .collect();
        let active = tabs.iter().position(|tab| tab["active"] == true);
        json!({"tabs": tabs, "active": active})
    }

    /// The tab a command of the local API names: by its place as
    /// `list_tabs` gives it, else by its name in any case.
    fn tab_asked(&self, name: Option<&str>, index: Option<usize>) -> Result<TabId, String> {
        let listed = self.listed_tabs();
        if let Some(index) = index {
            return listed.get(index).cloned().ok_or_else(|| {
                format!(
                    "there is no tab {index}; list_tabs lists {} tabs",
                    listed.len()
                )
            });
        }
        let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
            return Err("give the name or the index of a tab as list_tabs gives them".into());
        };
        if name.eq_ignore_ascii_case(MODEL_NAME) || name.eq_ignore_ascii_case(tr(MODEL_NAME)) {
            return Ok(TabId::Model);
        }
        listed
            .into_iter()
            .find(|tab| self.tab_name(tab).eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("no open tab {name}; list_tabs lists the open tabs"))
    }

    /// The `list_tabs` command of the local API.
    pub(crate) fn api_list_tabs(&mut self) -> (Value, Task<Message>) {
        let task = self.settle_tabs().unwrap_or_else(Task::none);
        let mut answer = self.tabs_value();
        answer["ok"] = Value::Bool(true);
        (answer, task)
    }

    /// The `show_tab` command of the local API: as a click on the tab. A
    /// drawing that is not made in this session yet is made, with a job.
    pub(crate) fn api_show_tab(
        &mut self,
        name: Option<&str>,
        index: Option<usize>,
    ) -> (Value, Task<Message>) {
        if self.settings.is_some() {
            return refused("the Settings dialog is open".into());
        }
        if self.sheet_dialog.is_some() {
            return refused("the dialog of Create 2D is open".into());
        }
        if self.mesh_to_plans.covers_model() {
            return refused("the card of the Pointcloud to Drawing wizard is open".into());
        }
        let tab = match self.tab_asked(name, index) {
            Ok(tab) => tab,
            Err(error) => return refused(error),
        };
        let tab_name = self.tab_api_name(&tab);
        let mut job = None;
        let shown = match &tab {
            TabId::Drawing(guid) if self.drawing_view.made(guid).is_none() => {
                let id =
                    self.record_api_job(json!({"state": "running", "operation": "create_drawing"}));
                let shown = self.show_saved_drawing(guid, Some(id.clone()));
                match shown {
                    Ok(task) => {
                        job = Some(id);
                        Ok(task.unwrap_or_else(Task::none))
                    }
                    Err(error) => {
                        self.forget_api_job(&id);
                        Err(error)
                    }
                }
            }
            _ => self.show_tab(&tab),
        };
        let task = match shown {
            Ok(task) => task,
            Err(error) => return refused(error),
        };
        let settled = self.settle_tabs().unwrap_or_else(Task::none);
        let mut answer = self.tabs_value();
        answer["ok"] = Value::Bool(true);
        answer["shown"] = Value::from(tab_name);
        if let Some(id) = job {
            answer["accepted"] = Value::Bool(true);
            answer["job_id"] = Value::from(id);
        }
        (answer, Task::batch([task, settled]))
    }

    /// The `close_tab` command of the local API: as the × of the tab.
    pub(crate) fn api_close_tab(
        &mut self,
        name: Option<&str>,
        index: Option<usize>,
    ) -> (Value, Task<Message>) {
        let tab = match self.tab_asked(name, index) {
            Ok(tab) => tab,
            Err(error) => return refused(error),
        };
        if !tab.closable() {
            return refused("the tab of the 3D model does not close".into());
        }
        let tab_name = self.tab_api_name(&tab);
        let task = self.close_tab(&tab);
        let settled = self.settle_tabs().unwrap_or_else(Task::none);
        let mut answer = self.tabs_value();
        answer["ok"] = Value::Bool(true);
        answer["closed"] = Value::from(tab_name);
        (answer, Task::batch([task, settled]))
    }
}

/// What the active tab lies on, as its colour and its ink: the sheet of a
/// drawing (`drawing`), else the scene.
fn active_surface(theme: ui_theme::UiTheme, drawing: bool) -> (Color, Color) {
    if drawing {
        (
            crate::drawing_view::paper(theme),
            Color::from_rgb8(28, 25, 23),
        )
    } else {
        let colors = theme.colors();
        (colors.scene, colors.scene_text)
    }
}

fn refused(error: String) -> (Value, Task<Message>) {
    (json!({"ok": false, "error": error}), Task::none())
}

/// A tab: the active one in the colour and the ink of what is shown under
/// it, the others quiet until the pointer is over them.
fn tab_style(
    theme: &Theme,
    active: Option<(Color, Color)>,
    status: button::Status,
) -> button::Style {
    let colors = ui_theme::colors(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (background, text_color) = if let Some(surface) = active {
        surface
    } else if hovered {
        (colors.hover, colors.text)
    } else {
        (Color::TRANSPARENT, colors.muted)
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color,
        border: Border {
            radius: if active.is_some() {
                0.0.into()
            } else {
                iced::border::Radius::default().top(3)
            },
            ..Border::default()
        },
        ..button::Style::default()
    }
}

/// The × of a tab: on the active one in the ink of what is shown under it,
/// a little quieter until the pointer is over it, so that it reads as well
/// on the sheet of a drawing as on the scene; on the others in the quiet
/// colour of the strip.
fn close_style(
    theme: &Theme,
    active: Option<(Color, Color)>,
    status: button::Status,
) -> button::Style {
    let colors = ui_theme::colors(theme);
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    let (quiet, hover, ink) = match active {
        Some((surface, ink)) => (mixed(ink, surface, 0.7), Color { a: 0.12, ..ink }, ink),
        None => (colors.muted, colors.ribbon_hover, colors.text),
    };
    button::Style {
        background: hovered.then_some(Background::Color(hover)),
        text_color: if hovered { ink } else { quiet },
        border: Border::default().rounded(3.0),
        ..button::Style::default()
    }
}

/// `share` of `ink` laid over `surface`.
fn mixed(ink: Color, surface: Color, share: f32) -> Color {
    let mix = |a: f32, b: f32| a * share + b * (1.0 - share);
    Color::from_rgb(
        mix(ink.r, surface.r),
        mix(ink.g, surface.g),
        mix(ink.b, surface.b),
    )
}

#[cfg(test)]
mod tests;
