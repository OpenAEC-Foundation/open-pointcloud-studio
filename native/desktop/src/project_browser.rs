//! The Project Browser at the left of the window: the groups SCANS,
//! CLASSES, VIEWS and BCF, each under a band of its own that opens or
//! collapses it. Scans from more than one folder form a sub-group per
//! folder. VIEWS holds everything that is a view: the 3D model, the saved
//! views, and the 2D drawings by kind. Which groups are collapsed is kept in
//! the preferences; collapsing never changes what is loaded or shown.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use iced::widget::{
    button, checkbox, column, container, mouse_area, progress_bar, row, text, tooltip, Column,
};
use iced::{Color, Element, Fill, Task};
use serde_json::{json, Value};

use crate::camera_views::SavedView;
use crate::drawing_view::DrawingViewAction;
use crate::fonts;
use crate::i18n::{key, tr, tr_args};
use crate::saved_drawings::SavedDrawing;
use crate::sheet_dialog::SheetKind;
use crate::ui_theme;
use crate::{
    compact_count, display_name, faces, flat_tool_style, format_count, icon_svg,
    muted_checkbox_style, opencad_ribbon, project_open, CloudEntry, Message, Studio, ToolIcon,
    ASPRS_CLASSIFICATIONS,
};

/// The groups and sub-groups that open and collapse, by the key the
/// preferences keep them under.
pub const SCANS: &str = "scans";
pub const CLASSES: &str = "classes";
pub const VIEWS: &str = "views";
pub const BCF: &str = "bcf";
/// A sub-group of the scans in one folder: this, then the folder.
const FOLDER: &str = "folder:";
/// The preferences keep at most this many collapsed groups, the oldest
/// collapsed folders going first.
const MAX_COLLAPSED: usize = 64;
/// The width of the Project Browser.
pub const WIDTH: f32 = 272.0;

/// The key of the sub-group of the scans in a folder.
pub fn folder_key(folder: &Path) -> String {
    format!("{FOLDER}{}", folder.display())
}

/// The folder a scan lies in.
pub fn folder_of(entry: &CloudEntry) -> PathBuf {
    entry
        .cloud
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Which groups of the Project Browser are collapsed, and what the browser
/// keeps to draw quickly.
#[derive(Debug, Default)]
pub struct BrowserState {
    collapsed: Vec<String>,
    /// The name each open scan carries in the saved views and drawings, by
    /// its path. Finding it asks the file system, so it is kept.
    sources: RefCell<HashMap<PathBuf, PathBuf>>,
}

impl BrowserState {
    pub fn new(collapsed: Vec<String>) -> Self {
        let mut state = Self::default();
        for key in collapsed {
            state.set_open(&key, false);
        }
        state
    }

    pub fn is_open(&self, key: &str) -> bool {
        !self.collapsed.iter().any(|known| known == key)
    }

    /// Open or collapse a group; reports whether that changed anything.
    pub fn set_open(&mut self, key: &str, open: bool) -> bool {
        if key.is_empty() || open == self.is_open(key) {
            return false;
        }
        if open {
            self.collapsed.retain(|known| known != key);
        } else {
            self.collapsed.push(key.to_owned());
            while self.collapsed.len() > MAX_COLLAPSED {
                let oldest_folder = self
                    .collapsed
                    .iter()
                    .position(|known| known.starts_with(FOLDER))
                    .unwrap_or(0);
                self.collapsed.remove(oldest_folder);
            }
        }
        true
    }

    /// The collapsed groups, as the preferences keep them.
    pub fn collapsed(&self) -> &[String] {
        &self.collapsed
    }

    /// The name a scan carries in the saved views and drawings.
    pub fn source_of(&self, path: &Path) -> PathBuf {
        self.sources
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| crate::camera_views::source_key(path))
            .clone()
    }
}

/// Whether all, some or none of a set of scans is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    All,
    Mixed,
    None,
}

/// What the checkbox of a group says about the scans in it.
pub fn shown_state(visible: impl IntoIterator<Item = bool>) -> Shown {
    let (mut on, mut off) = (false, false);
    for visible in visible {
        if visible {
            on = true;
        } else {
            off = true;
        }
    }
    match (on, off) {
        (true, false) => Shown::All,
        (true, true) => Shown::Mixed,
        (false, _) => Shown::None,
    }
}

/// The scans grouped by the folder they lie in: the folders in name order,
/// the scans of each in the order given.
pub fn by_folder(order: &[usize], folder: impl Fn(usize) -> PathBuf) -> Vec<(PathBuf, Vec<usize>)> {
    let mut groups: Vec<(PathBuf, Vec<usize>)> = Vec::new();
    for &index in order {
        let place = folder(index);
        match groups.iter_mut().find(|(known, _)| *known == place) {
            Some((_, rows)) => rows.push(index),
            None => groups.push((place, vec![index])),
        }
    }
    groups.sort_by(|(a, _), (b, _)| {
        project_open::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy())
    });
    groups
}

/// How far the scans are that are being opened or indexed, as the band of
/// SCANS says it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    /// Scans that are ready.
    pub done: usize,
    /// Scans listed and scans whose import has not given a row yet.
    pub total: usize,
    pub fraction: f32,
}

/// The summary of scans of which each row is ready (`None`) or under way
/// with how far it is when that is known, besides `waiting` imports that
/// have no row yet. `None` while nothing is under way.
pub fn summary(rows: &[Option<Option<f32>>], waiting: usize) -> Option<Summary> {
    if waiting == 0 && rows.iter().all(Option::is_none) {
        return None;
    }
    let done = rows.iter().filter(|row| row.is_none()).count();
    let total = rows.len() + waiting;
    let partly: f32 = rows
        .iter()
        .flatten()
        .map(|fraction| fraction.unwrap_or(0.0).clamp(0.0, 1.0))
        .sum();
    Some(Summary {
        done,
        total,
        fraction: ((done as f32 + partly) / total.max(1) as f32).clamp(0.0, 1.0),
    })
}

/// The kinds of view that VIEWS lists, in the order it lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewKind {
    /// The 3D model and the saved views.
    ThreeD,
    Plans,
    Elevations,
    Sections,
    /// Previews, exports and opened DXF and DWG files.
    Files,
}

impl ViewKind {
    pub const ALL: [Self; 5] = [
        Self::ThreeD,
        Self::Plans,
        Self::Elevations,
        Self::Sections,
        Self::Files,
    ];

    /// The key of the sub-group, as the preferences and the API name it.
    pub fn key(self) -> &'static str {
        match self {
            Self::ThreeD => "views.3d",
            Self::Plans => "views.plans",
            Self::Elevations => "views.elevations",
            Self::Sections => "views.sections",
            Self::Files => "views.files",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ThreeD => key("3D views"),
            Self::Plans => key("Plans"),
            Self::Elevations => key("Elevations"),
            Self::Sections => key("Sections"),
            Self::Files => key("Files"),
        }
    }

    pub(crate) fn icon(self) -> ToolIcon {
        match self {
            Self::ThreeD => ToolIcon::Model,
            Self::Plans => ToolIcon::PlanSheet,
            Self::Elevations => ToolIcon::ElevationSheet,
            Self::Sections => ToolIcon::SectionSheet,
            Self::Files => ToolIcon::DrawingFile,
        }
    }

    pub(crate) fn of(kind: SheetKind) -> Self {
        match kind {
            SheetKind::Plan => Self::Plans,
            SheetKind::Elevation => Self::Elevations,
            SheetKind::Section => Self::Sections,
        }
    }
}

/// A row of VIEWS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewRow {
    /// The default 3D view.
    Model,
    /// A saved view, by its identifier.
    Saved(String),
    /// A drawing of Create 2D, by the identifier of how it was made.
    Drawing(String),
    /// A preview, export or opened file, by its place in the list.
    File(usize),
}

/// The rows of VIEWS by kind, in the order they are listed: 3D views with
/// the 3D model first and then the saved views, plans, elevations,
/// sections and files. A kind without a row is left out.
pub fn view_groups(
    views: &[&SavedView],
    drawings: &[&SavedDrawing],
    files: usize,
) -> Vec<(ViewKind, Vec<ViewRow>)> {
    ViewKind::ALL
        .into_iter()
        .map(|kind| {
            let rows: Vec<ViewRow> = match kind {
                ViewKind::ThreeD => std::iter::once(ViewRow::Model)
                    .chain(views.iter().map(|view| ViewRow::Saved(view.guid.clone())))
                    .collect(),
                ViewKind::Files => (0..files).map(ViewRow::File).collect(),
                _ => drawings
                    .iter()
                    .filter(|drawing| ViewKind::of(drawing.kind) == kind)
                    .map(|drawing| ViewRow::Drawing(drawing.guid.clone()))
                    .collect(),
            };
            (kind, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// The name of a copy: the name with " (2)" after it, or with the next
/// number no name takes. A name that ends in such a number counts on from
/// it, so that the copy of "Plan (2)" is "Plan (3)". The name is shortened
/// to stay within `max_chars`.
pub fn duplicate_name(name: &str, max_chars: usize, taken: impl Fn(&str) -> bool) -> String {
    let name = name.trim();
    let numbered = name
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" ("))
        .filter(|(base, number)| {
            !base.trim().is_empty() && number.parse::<u32>().is_ok_and(|number| number >= 2)
        });
    let base = numbered.map_or(name, |(base, _)| base);
    let base: String = base.chars().take(max_chars.saturating_sub(7)).collect();
    (2..10_000)
        .map(|number| format!("{base} ({number})"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| format!("{base} (2)"))
}

/// The group key an API name stands for.
pub fn group_named(name: &str) -> Option<String> {
    let name = name.trim();
    if let Some(folder) = name.strip_prefix(FOLDER) {
        return (!folder.is_empty()).then(|| folder_key(Path::new(folder)));
    }
    let name = name.to_ascii_lowercase();
    [SCANS, CLASSES, VIEWS, crate::layouts::SHEETS, BCF]
        .into_iter()
        .chain(ViewKind::ALL.map(ViewKind::key))
        .find(|known| *known == name || known.strip_prefix("views.") == Some(name.as_str()))
        .map(str::to_owned)
}

#[derive(Debug, Clone)]
pub enum BrowserAction {
    /// Open or collapse a group by its key.
    Toggle(String),
    /// Show or hide every scan.
    AllScansVisible(bool),
    /// Show or hide every scan in a folder.
    FolderVisible(PathBuf, bool),
    /// Show the 3D scene in place of a drawing.
    ShowModel,
    /// Duplicate a row of VIEWS: the 3D model as a saved view of the
    /// current 3D view, a saved view or a drawing.
    Duplicate(ViewRow),
}

/// The band over a group or a sub-group: a chevron, the icon of what it
/// holds, its caption and a count, which open or collapse it on a click,
/// and controls of its own beside them.
#[allow(clippy::too_many_arguments)]
pub(crate) fn band<'a>(
    group: String,
    open: bool,
    icon: ToolIcon,
    caption: String,
    count: String,
    controls: Vec<Element<'a, Message>>,
    sub: bool,
) -> Element<'a, Message> {
    let chevron = if open {
        ToolIcon::ChevronOpen
    } else {
        ToolIcon::ChevronClosed
    };
    // The head of a group as a section head, a sub-group as a row.
    let label = text(caption)
        .size(11)
        .font(if sub { fonts::REGULAR } else { fonts::SEMIBOLD })
        .wrapping(iced::widget::text::Wrapping::None);
    let toggle = button(
        row![
            icon_svg(chevron, 10.0),
            icon_svg(icon, if sub { 13.0 } else { 15.0 }),
            container(label).width(Fill).clip(true),
            text(count)
                .size(10)
                .font(fonts::SEMIBOLD)
                .wrapping(iced::widget::text::Wrapping::None)
                .style(|theme| text::Style {
                    color: Some(ui_theme::colors(theme).text_muted),
                }),
        ]
        .spacing(5)
        .align_y(iced::Alignment::Center),
    )
    .on_press(Message::Browser(BrowserAction::Toggle(group)))
    .style(move |theme, status| {
        let colors = ui_theme::colors(theme);
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: hovered.then_some(iced::Background::Color(colors.hover)),
            // The head of a group in the amber of a section head, the head
            // of a sub-group as a row of the tree.
            text_color: match (sub, hovered) {
                (true, _) => colors.text,
                (false, true) => colors.accent,
                (false, false) => colors.accent_tint,
            },
            border: iced::Border::default().rounded(3.0),
            ..button::Style::default()
        }
    })
    .padding([2, 4])
    .width(Fill);
    let mut line = row![toggle].spacing(3).align_y(iced::Alignment::Center);
    for control in controls {
        line = line.push(control);
    }
    let height = if sub { 24.0 } else { 28.0 };
    let body = container(line.padding(iced::Padding {
        right: 4.0,
        ..iced::Padding::ZERO
    }))
    .height(height)
    .align_y(iced::Alignment::Center)
    .width(Fill);
    container(body)
        .width(Fill)
        .style(move |theme| {
            let colors = ui_theme::colors(theme);
            if sub {
                container::Style::default().color(colors.text)
            } else {
                container::Style::default()
                    .background(colors.bg_lighter)
                    .color(colors.accent_tint)
            }
        })
        .into()
}

/// A small button with an icon and a tooltip, for the band of a group.
fn icon_button<'a>(icon: ToolIcon, tip: &str, message: Message) -> Element<'a, Message> {
    tooltip(
        button(icon_svg(icon, 15.0))
            .on_press(message)
            .style(flat_tool_style)
            .padding(3),
        hint(tip.to_owned()),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// An action on the band of a group: an icon with its name in a tooltip,
/// disabled when there is no message.
fn band_action<'a>(icon: ToolIcon, tip: &str, message: Option<Message>) -> Element<'a, Message> {
    tooltip(
        button(icon_svg(icon, 15.0))
            .on_press_maybe(message)
            .style(flat_tool_style)
            .padding(3),
        hint(tip.to_owned()),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// The box a tooltip of the Project Browser shows its text in.
pub(crate) fn hint<'a>(content: String) -> Element<'a, Message> {
    container(text(content).size(11))
        .padding([4, 7])
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.tooltip_bg)
                .color(colors.tooltip_text)
                .border(iced::Border::default().rounded(4))
        })
        .into()
}

/// The checkbox of a group of scans: ticked when all are shown, empty when
/// none is and with a dash when some are. A click on a dash shows them all.
fn shown_checkbox<'a>(state: Shown, on: impl Fn(bool) -> Message + 'a) -> Element<'a, Message> {
    let mut boxed = checkbox("", state != Shown::None)
        .on_toggle(move |checked| on(state == Shown::Mixed || checked))
        .style(muted_checkbox_style)
        .spacing(0)
        .size(13);
    if state == Shown::Mixed {
        boxed = boxed.icon(checkbox::Icon {
            font: fonts::REGULAR,
            code_point: '\u{2013}',
            size: None,
            line_height: iced::widget::text::LineHeight::default(),
            shaping: iced::widget::text::Shaping::Basic,
        });
    }
    boxed.into()
}

/// A row of VIEWS: the icon of its kind and its name, which show it on a
/// click, highlighted while it is what the window shows, with controls
/// after it.
pub fn view_row<'a>(
    icon: ToolIcon,
    name: String,
    shown: bool,
    quiet: bool,
    message: Message,
    controls: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    // The actions of a row are on the row of what the window shows only,
    // so that the list reads as a list of names.
    let controls = if shown { controls } else { Vec::new() };
    // A name stays on one line: a long one is cut with an ellipsis and
    // shown whole in the tooltip.
    let room = ROW_NAME_WIDTH - controls.len() as f32 * ROW_CONTROL_WIDTH;
    let fitted = crate::view_tabs::shortened(&name, (room / NAME_CHAR_WIDTH).max(8.0) as usize);
    let cut = fitted != name;
    let label = text(fitted)
        .size(11)
        .wrapping(iced::widget::text::Wrapping::None)
        .style(move |theme| text::Style {
            color: quiet.then(|| ui_theme::colors(theme).text_muted),
        });
    let pick = button(
        row![icon_svg(icon, 14.0), label]
            .spacing(6)
            .align_y(iced::Alignment::Center),
    )
    .on_press(message)
    .style(move |theme, status| opencad_ribbon::tool_btn_style(theme, shown, status))
    .padding([3, 5])
    .width(Fill);
    let pick: Element<'a, Message> = if cut {
        tooltip(pick, hint(name), tooltip::Position::Bottom)
            .gap(4)
            .into()
    } else {
        pick.into()
    };
    let mut line = row![pick].spacing(1).align_y(iced::Alignment::Center);
    for control in controls {
        line = line.push(control);
    }
    line.into()
}

/// How far the rows of a sub-group sit in from its band: one step, so that
/// they read as its contents.
const SUB_ROW_INDENT: f32 = 18.0;

/// The width a row of VIEWS has for its name when it shows no actions.
const ROW_NAME_WIDTH: f32 = 188.0;
/// The width an action button of a row takes.
const ROW_CONTROL_WIDTH: f32 = 21.0;
/// The mean width of a character of a row's name.
const NAME_CHAR_WIDTH: f32 = 5.9;

/// The small button of a row that makes a copy of it.
pub fn duplicate_button<'a>(message: Message) -> Element<'a, Message> {
    row_button(ToolIcon::Duplicate, tr("Duplicate"), message)
}

/// A small button with an icon on a row of VIEWS, with its tooltip.
pub fn row_button<'a>(icon: ToolIcon, tip: &str, message: Message) -> Element<'a, Message> {
    tooltip(
        button(icon_svg(icon, 12.0))
            .on_press(message)
            .style(flat_tool_style)
            .padding([3, 4]),
        hint(tip.to_owned()),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// The × of a row, which takes it off the list.
pub fn remove_button<'a>(message: Message) -> Element<'a, Message> {
    button(text("×").size(11))
        .on_press(message)
        .style(flat_tool_style)
        .padding([3, 6])
        .into()
}

/// Rows under a band, set in a little.
pub fn indented<'a>(content: impl Into<Element<'a, Message>>, left: f32) -> Element<'a, Message> {
    container(content)
        .padding(iced::Padding {
            left,
            ..iced::Padding::ZERO
        })
        .width(Fill)
        .into()
}

impl Studio {
    pub(crate) fn update_browser(&mut self, action: BrowserAction) -> Task<Message> {
        match action {
            BrowserAction::Toggle(group) => {
                let open = !self.browser.is_open(&group);
                if self.browser.set_open(&group, open) {
                    return self.queue_preferences_save();
                }
            }
            BrowserAction::AllScansVisible(visible) => {
                let all: Vec<usize> = (0..self.clouds.len()).collect();
                return self.set_scans_visible(&all, visible);
            }
            BrowserAction::FolderVisible(folder, visible) => {
                let rows: Vec<usize> = (0..self.clouds.len())
                    .filter(|index| folder_of(&self.clouds[*index]) == folder)
                    .collect();
                return self.set_scans_visible(&rows, visible);
            }
            // As a click on its tab: the 3D model is no saved view, so the
            // active one lets go, as Hide under its annotations does, and the
            // row of the 3D model is the one highlighted.
            BrowserAction::ShowModel => return self.show_model_tab(),
            BrowserAction::Duplicate(row) => {
                let done = match row {
                    ViewRow::Model => self.duplicate_model_view(),
                    ViewRow::Saved(guid) => self.duplicate_view(&guid),
                    ViewRow::Drawing(guid) => self
                        .duplicate_saved_drawing(&guid, None)
                        .map(|(_, task)| task.unwrap_or_else(Task::none)),
                    ViewRow::File(_) => Ok(Task::none()),
                };
                match done {
                    Ok(task) => return task,
                    Err(reason) => self.status = reason,
                }
            }
        }
        Task::none()
    }

    /// Show or hide scans, as the checkbox of each of their rows would.
    fn set_scans_visible(&mut self, rows: &[usize], visible: bool) -> Task<Message> {
        if rows.is_empty() {
            return Task::none();
        }
        self.cancel_selection_for_scene_change();
        for index in rows {
            self.clouds[*index].visible = visible;
        }
        self.revision += 1;
        self.schedule_detail()
    }

    /// The names the open scans carry in the saved views and drawings.
    pub(crate) fn open_sources(&self) -> Vec<PathBuf> {
        self.clouds
            .iter()
            .map(|entry| self.browser.source_of(&entry.cloud.path))
            .collect()
    }

    /// The drawings of Create 2D made from an open scan, in the order they
    /// were made.
    pub(crate) fn listed_drawings(&self) -> Vec<&SavedDrawing> {
        let open = self.open_sources();
        self.drawing_view
            .saved
            .iter()
            .filter(|drawing| open.iter().any(|source| drawing.uses(source)))
            .collect()
    }

    /// The rows VIEWS lists, by kind.
    pub(crate) fn view_groups(&self) -> Vec<(ViewKind, Vec<ViewRow>)> {
        view_groups(
            &self.listed_views(),
            &self.listed_drawings(),
            self.drawing_view.sheets().len(),
        )
    }

    /// What `status.result.project_browser` reports: which groups are open
    /// and what VIEWS lists.
    pub(crate) fn browser_value(&self) -> Value {
        let views = self.listed_views();
        let drawings = self.listed_drawings();
        let name_of = |row: &ViewRow| match row {
            ViewRow::Model => "3D model".to_owned(),
            ViewRow::Saved(guid) => views
                .iter()
                .find(|view| view.guid == *guid)
                .map_or_else(String::new, |view| view.name.clone()),
            ViewRow::Drawing(guid) => drawings
                .iter()
                .find(|drawing| drawing.guid == *guid)
                .map_or_else(String::new, |drawing| drawing.name.clone()),
            ViewRow::File(place) => self
                .drawing_view
                .sheets()
                .get(*place)
                .map_or_else(String::new, |sheet| sheet.source.caption()),
        };
        let groups: Vec<Value> = self
            .view_groups()
            .iter()
            .map(|(kind, rows)| {
                json!({
                    "group": kind.key().trim_start_matches("views."),
                    "open": self.browser.is_open(kind.key()),
                    "rows": rows.iter().map(&name_of).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "open": {
                SCANS: self.browser.is_open(SCANS),
                CLASSES: self.browser.is_open(CLASSES),
                VIEWS: self.browser.is_open(VIEWS),
                crate::layouts::SHEETS: self.browser.is_open(crate::layouts::SHEETS),
                BCF: self.browser.is_open(BCF),
            },
            "collapsed": self.browser.collapsed(),
            "views": groups,
            "sheets": self.layouts.list.iter().map(crate::layouts::Layout::caption).collect::<Vec<_>>(),
            "shown": self.shown_row().map(|row| name_of(&row)),
        })
    }

    /// The `set_browser_group` command of the local API.
    pub(crate) fn api_set_browser_group(
        &mut self,
        group: &str,
        open: bool,
    ) -> (Value, Task<Message>) {
        let Some(key) = group_named(group) else {
            return (
                json!({"ok": false, "error": "group must be scans, classes, views, sheets, bcf, 3d, plans, elevations, sections, files or folder: and the path of a folder of scans"}),
                Task::none(),
            );
        };
        let task = if self.browser.set_open(&key, open) {
            self.queue_preferences_save()
        } else {
            Task::none()
        };
        (
            json!({"ok": true, "group": key, "open": open, "project_browser": self.browser_value()}),
            task,
        )
    }

    /// The panel at the left of the window.
    pub(crate) fn project_panel(&self) -> Element<'_, Message> {
        let mut panel =
            column![text(tr("Project Browser")).size(11).font(fonts::SEMIBOLD)].spacing(8);
        panel = panel.push(self.scans_group());
        if let Some(classes) = self.classes_group() {
            panel = panel.push(classes);
        }
        panel = panel
            .push(self.views_group())
            .push(self.sheets_group())
            .push(self.bcf_group());
        // Pointcloud to Drawing projects of the open scans can be taken up again.
        if let Some(resume) = self.mesh_to_plans_browser() {
            panel = panel.push(resume);
        }
        container(iced::widget::scrollable(panel.padding(14)).height(Fill))
            .width(WIDTH)
            .height(Fill)
            .style(crate::sidebar_style)
            .into()
    }

    fn scans_group(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let open = self.browser.is_open(SCANS);
        let total_points: u64 = self.clouds.iter().map(CloudEntry::remaining_count).sum();
        let state = shown_state(self.clouds.iter().map(|entry| entry.visible));
        let mut controls: Vec<Element<'_, Message>> = Vec::new();
        if !self.clouds.is_empty() {
            controls.push(shown_checkbox(state, |visible| {
                Message::Browser(BrowserAction::AllScansVisible(visible))
            }));
        }
        controls.push(icon_button(
            ToolIcon::Open,
            tr("Add a point cloud"),
            Message::Open,
        ));
        controls.push(icon_button(
            ToolIcon::OpenFolder,
            tr("Open scan folder"),
            Message::OpenFolder,
        ));
        let count = if self.clouds.is_empty() {
            String::new()
        } else {
            format!("{} · {}", self.clouds.len(), compact_count(total_points))
        };
        let mut group = column![band(
            SCANS.to_owned(),
            open,
            ToolIcon::Scan,
            tr("SCANS").to_owned(),
            count,
            controls,
            false,
        )]
        .spacing(3);
        // How far the scans are that are opening or indexing, also while
        // the group is collapsed.
        if let Some(summary) = self.scans_summary() {
            group = group.push(indented(
                column![
                    text(tr_args(
                        "{done} of {total} · {percent}%",
                        &[
                            ("done", &summary.done),
                            ("total", &summary.total),
                            ("percent", &((summary.fraction * 100.0).floor() as u32)),
                        ],
                    ))
                    .size(10)
                    .color(colors.text_muted),
                    progress_bar(0.0..=1.0, summary.fraction)
                        .height(2)
                        .style(|theme| {
                            let colors = ui_theme::colors(theme);
                            progress_bar::Style {
                                background: colors.border_strong.into(),
                                bar: colors.accent.into(),
                                border: iced::Border::default(),
                            }
                        }),
                ]
                .spacing(2),
                7.0,
            ));
        }
        if !open {
            return group.into();
        }
        if self.clouds.is_empty() {
            return group
                .push(
                    button(text(tr("+  Add point cloud")).size(12))
                        .on_press(Message::Open)
                        .style(flat_tool_style)
                        .width(Fill),
                )
                .push(
                    button(text(tr("+  Open scan folder…")).size(12))
                        .on_press(Message::OpenFolder)
                        .style(flat_tool_style)
                        .width(Fill),
                )
                .into();
        }
        let picked = self.clouds.iter().filter(|entry| entry.picked).count();
        if picked > 1 {
            group = group.push(indented(
                text(tr_args("{count} selected", &[("count", &picked)]))
                    .size(10)
                    .color(colors.text_muted),
                7.0,
            ));
        }
        let order = self.layer_order();
        let folders = by_folder(&order, |index| folder_of(&self.clouds[index]));
        if folders.len() < 2 {
            let mut rows = column![].spacing(2);
            for index in order {
                rows = rows.push(self.scan_row(index));
            }
            return group.push(rows).into();
        }
        for (folder, rows) in folders {
            let key = folder_key(&folder);
            let folder_open = self.browser.is_open(&key);
            let state = shown_state(rows.iter().map(|index| self.clouds[*index].visible));
            let points: u64 = rows
                .iter()
                .map(|index| self.clouds[*index].remaining_count())
                .sum();
            let name = folder.file_name().map_or_else(
                || folder.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            let target = folder.clone();
            let header = tooltip(
                band(
                    key,
                    folder_open,
                    ToolIcon::OpenFolder,
                    name,
                    format!("{} · {}", rows.len(), compact_count(points)),
                    vec![shown_checkbox(state, move |visible| {
                        Message::Browser(BrowserAction::FolderVisible(target.clone(), visible))
                    })],
                    true,
                ),
                hint(folder.display().to_string()),
                tooltip::Position::FollowCursor,
            )
            .gap(5);
            let mut sub = column![header].spacing(2);
            if folder_open {
                let mut list = column![].spacing(2);
                for index in rows {
                    list = list.push(self.scan_row(index));
                }
                sub = sub.push(indented(list, SUB_ROW_INDENT));
            }
            group = group.push(indented(sub, 4.0));
        }
        group.into()
    }

    /// How far the scans are that are being opened or indexed.
    pub(crate) fn scans_summary(&self) -> Option<Summary> {
        let rows: Vec<Option<Option<f32>>> = self
            .clouds
            .iter()
            .map(|entry| {
                self.layer_progress(entry)
                    .filter(|(note, _)| note != tr(crate::open_progress::INDEX_QUEUED))
                    .map(|(_, fraction)| fraction)
            })
            .collect();
        // An import waits for a row until its metadata or a first look at
        // its points is listed; a one-pass import that shows its points
        // while it still reads has its row already.
        let waiting = self
            .imports
            .keys()
            .filter(|id| {
                let listed = self.clouds.iter().any(|entry| {
                    entry.index_import_id == Some(**id)
                        || self
                            .import_headers
                            .get(id)
                            .is_some_and(|header| entry.matches_source(header))
                });
                !listed
            })
            .count();
        summary(&rows, waiting)
    }

    /// The row of a scan: its visibility, the icon of a scan, its name with
    /// the path and the points in a tooltip, its points and ×, and a second
    /// line only for what needs attention.
    fn scan_row(&self, index: usize) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let entry = &self.clouds[index];
        let name = display_name(&entry.cloud.path);
        let readable_name = name.replace('_', "_\u{200b}");
        let remaining = entry.remaining_count();
        let file_button = tooltip(
            button(
                row![
                    icon_svg(ToolIcon::Scan, 13.0),
                    text(readable_name)
                        .size(12)
                        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                        .width(Fill),
                ]
                .spacing(5)
                .align_y(iced::Alignment::Center),
            )
            .on_press(Message::LayerClick(index))
            .style(flat_tool_style)
            .width(Fill)
            .padding([2, 2]),
            hint(format!(
                "{}\n{} points",
                entry.cloud.path.display(),
                format_count(remaining)
            )),
            tooltip::Position::FollowCursor,
        )
        .gap(5);
        let mut item = column![row![
            checkbox("", entry.visible)
                .on_toggle(move |value| Message::LayerVisible(index, value))
                .style(muted_checkbox_style)
                .size(14),
            file_button,
            text(compact_count(remaining))
                .size(10)
                .color(colors.text_muted),
            button(text("×").size(12))
                .on_press(Message::LayerRemove(index))
                .style(flat_tool_style)
                .padding([1, 5]),
        ]
        .spacing(3)
        .align_y(iced::Alignment::Center)]
        .spacing(1);
        let progress = self.layer_progress(entry);
        let mut notes: Vec<String> = progress.iter().map(|(note, _)| note.clone()).collect();
        let selected = entry.selection.as_ref().map_or(0, |mask| mask.count);
        if selected > 0 {
            notes.push(tr_args(
                "{count} selected",
                &[("count", &format_count(selected))],
            ));
        }
        let deleted = entry.deleted_count();
        if deleted > 0 {
            notes.push(tr_args(
                "{count} deleted",
                &[("count", &format_count(deleted))],
            ));
        }
        if !notes.is_empty() {
            item = item.push(indented(
                text(notes.join("  ·  ")).size(10).color(colors.text_muted),
                20.0,
            ));
        }
        if let Some(fraction) = progress.and_then(|(_, fraction)| fraction) {
            item = item.push(
                container(progress_bar(0.0..=1.0, fraction).height(2).style(|theme| {
                    let colors = ui_theme::colors(theme);
                    progress_bar::Style {
                        background: colors.border_strong.into(),
                        bar: colors.accent.into(),
                        border: iced::Border::default(),
                    }
                }))
                .padding(iced::Padding {
                    left: 20.0,
                    right: 4.0,
                    ..iced::Padding::ZERO
                }),
            );
        }
        if entry.mesh.is_some() {
            item = item.push(
                checkbox(tr("Surface"), entry.mesh_visible)
                    .on_toggle(move |value| Message::SetMeshVisible(index, value))
                    .style(muted_checkbox_style)
                    .text_size(11)
                    .size(12),
            );
        }
        if let Some(switch) = faces::layer_switch(index, entry) {
            item = item.push(switch);
        }
        if let Some(photos) = self.photo_rows(index) {
            item = item.push(photos);
        }
        let active = self.active == Some(index);
        let picked = entry.picked;
        container(item)
            .padding([1, 4])
            .width(Fill)
            .style(move |theme| {
                let colors = ui_theme::colors(theme);
                // The active scan as the active item of a list of the style
                // book, chosen scans as its chosen item.
                container::Style::default()
                    .background(if active {
                        colors.hover_strong
                    } else if picked {
                        colors.dialog_tab_active_bg
                    } else {
                        Color::TRANSPARENT
                    })
                    .border(iced::Border {
                        color: if active {
                            colors.accent
                        } else {
                            Color::TRANSPARENT
                        },
                        width: 1.0,
                        radius: 2.0.into(),
                    })
            })
            .into()
    }

    /// The classes that occur in the open clouds, each shown or hidden like
    /// a layer; no group without them.
    fn classes_group(&self) -> Option<Element<'_, Message>> {
        let classes = self.class_codes();
        if classes.is_empty() {
            return None;
        }
        let open = self.browser.is_open(CLASSES);
        let mut group = column![band(
            CLASSES.to_owned(),
            open,
            ToolIcon::Classes,
            tr("CLASSES").to_owned(),
            classes.len().to_string(),
            Vec::new(),
            false,
        )]
        .spacing(3);
        if open {
            let mut list = column![].spacing(3);
            for code in classes {
                let label = ASPRS_CLASSIFICATIONS
                    .iter()
                    .find(|(known, _)| *known == code)
                    .map_or_else(
                        || format!("{code:02}  {} {code}", tr("Class")),
                        |(_, label)| format!("{code:02}  {}", tr(label)),
                    );
                // The switch, the icon of a class and its name, which
                // switches it as well.
                let shown = self.class_visibility.allows(Some(code));
                list = list.push(
                    row![
                        checkbox("", shown)
                            .on_toggle(move |visible| Message::FilterClass(code, visible))
                            .style(muted_checkbox_style)
                            .spacing(0)
                            .size(13),
                        icon_svg(ToolIcon::Classes, 12.0),
                        mouse_area(text(label).size(11))
                            .on_press(Message::FilterClass(code, !shown)),
                    ]
                    .spacing(5)
                    .align_y(iced::Alignment::Center),
                );
            }
            group = group.push(indented(list, 7.0));
        }
        Some(group.into())
    }

    /// VIEWS: the 3D model, the saved views and the 2D drawings by kind,
    /// then the name field with Save view and the actions that make or open
    /// a drawing, and the annotations of the active view.
    fn views_group(&self) -> Element<'_, Message> {
        let open = self.browser.is_open(VIEWS);
        let groups = self.view_groups();
        let count: usize = groups.iter().map(|(_, rows)| rows.len()).sum();
        let model_shown = !self.drawing_view.shown;
        let show_model = tooltip(
            button(icon_svg(ToolIcon::Model, 15.0))
                .on_press(Message::Browser(BrowserAction::ShowModel))
                .style(move |theme, status| {
                    opencad_ribbon::tool_btn_style(theme, model_shown, status)
                })
                .padding(3),
            hint(tr("Show the 3D model").to_owned()),
            tooltip::Position::Bottom,
        )
        .gap(4);
        // Making and opening drawings are icons on the band, beside the 3D
        // model, so that the group holds only views.
        let has_scan = self.active.is_some();
        let new_drawing = band_action(
            ToolIcon::PlanSheet,
            tr("Create 2D plan / elevation / section…"),
            has_scan.then_some(Message::Sheet(crate::sheet_dialog::SheetAction::Open)),
        );
        let open_drawing = band_action(
            ToolIcon::Open,
            tr("Open drawing…"),
            Some(Message::DrawingView(DrawingViewAction::OpenFile)),
        );
        let group = column![band(
            VIEWS.to_owned(),
            open,
            ToolIcon::Views,
            tr("VIEWS").to_owned(),
            count.to_string(),
            vec![new_drawing, open_drawing, show_model.into()],
            false,
        )]
        .spacing(3);
        if !open {
            return group.into();
        }
        let mut body: Column<'_, Message> = column![self.save_view_row()].spacing(3);
        for (kind, rows) in groups {
            let sub_open = self.browser.is_open(kind.key());
            let mut sub = column![band(
                kind.key().to_owned(),
                sub_open,
                kind.icon(),
                tr(kind.label()).to_owned(),
                rows.len().to_string(),
                Vec::new(),
                true,
            )]
            .spacing(2);
            if sub_open {
                let mut list = column![].spacing(2);
                for listed in rows {
                    list = list.push(self.draggable_row(&listed, self.view_group_row(&listed)));
                }
                sub = sub.push(indented(list, SUB_ROW_INDENT));
            }
            body = body.push(sub);
        }
        if let Some(annotations) = self.annotation_list() {
            body = body.push(annotations);
        }
        group.push(indented(body, 4.0)).into()
    }

    /// The row of VIEWS of what the window shows, which is highlighted: in
    /// the Drawing view its drawing, else the active view, else the 3D
    /// model.
    pub(crate) fn shown_row(&self) -> Option<ViewRow> {
        let view = &self.drawing_view;
        if view.shown {
            // A sheet in front is no row of VIEWS.
            if view.layout.is_some() {
                return None;
            }
            if let Some(guid) = view.shown_guid() {
                return Some(ViewRow::Drawing(guid.to_owned()));
            }
            return view
                .sheets()
                .iter()
                .position(|sheet| view.is_current(sheet))
                .map(ViewRow::File);
        }
        Some(match self.active_view_index() {
            Some(index) => ViewRow::Saved(self.views.list[index].guid.clone()),
            None => ViewRow::Model,
        })
    }

    /// One row of VIEWS.
    fn view_group_row(&self, listed: &ViewRow) -> Element<'_, Message> {
        let shown = self.shown_row();
        let highlighted = shown.as_ref() == Some(listed);
        match listed {
            ViewRow::Model => view_row(
                ToolIcon::Model,
                tr("3D model").to_owned(),
                highlighted,
                false,
                Message::Browser(BrowserAction::ShowModel),
                if self.active.is_some() {
                    vec![duplicate_button(Message::Browser(
                        BrowserAction::Duplicate(ViewRow::Model),
                    ))]
                } else {
                    Vec::new()
                },
            ),
            ViewRow::Saved(guid) => match self.views.list.iter().find(|view| view.guid == *guid) {
                Some(view) => self.saved_view_row(view),
                None => column![].into(),
            },
            ViewRow::Drawing(guid) => {
                let Some(drawing) = self
                    .drawing_view
                    .saved
                    .iter()
                    .find(|drawing| drawing.guid == *guid)
                else {
                    return column![].into();
                };
                // A drawing that is not made yet in this session reads
                // quieter; a click makes it.
                let made = self.drawing_view.made(guid).is_some();
                view_row(
                    self.row_icon(
                        &crate::locks::LockTarget::Drawing(guid.clone()),
                        ViewKind::of(drawing.kind).icon(),
                    ),
                    drawing.name.clone(),
                    highlighted,
                    !made,
                    Message::DrawingView(DrawingViewAction::ShowDrawing(guid.clone())),
                    vec![
                        self.lock_button(crate::locks::LockTarget::Drawing(guid.clone())),
                        duplicate_button(Message::Browser(BrowserAction::Duplicate(
                            ViewRow::Drawing(guid.clone()),
                        ))),
                        remove_button(Message::DrawingView(DrawingViewAction::DeleteDrawing(
                            guid.clone(),
                        ))),
                    ],
                )
            }
            ViewRow::File(place) => {
                let Some(sheet) = self.drawing_view.sheets().get(*place) else {
                    return column![].into();
                };
                view_row(
                    ToolIcon::DrawingFile,
                    sheet.source.caption(),
                    highlighted,
                    false,
                    Message::DrawingView(DrawingViewAction::ShowSheet(*place)),
                    vec![remove_button(Message::DrawingView(
                        DrawingViewAction::RemoveSheet(*place),
                    ))],
                )
            }
        }
    }

    /// BCF: what a BCF file of the active scan would hold, and the button
    /// that writes it.
    fn bcf_group(&self) -> Element<'_, Message> {
        let open = self.browser.is_open(BCF);
        let mut group = column![band(
            BCF.to_owned(),
            open,
            ToolIcon::Bcf,
            "BCF".to_owned(),
            self.listed_views().len().to_string(),
            Vec::new(),
            false,
        )]
        .spacing(3);
        if open {
            group = group.push(indented(self.bcf_body(), 7.0));
        }
        group.into()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::camera_views::{self, SectionBox};

    #[test]
    fn a_collapsed_group_is_remembered_and_old_folders_make_room() {
        let mut state = BrowserState::new(vec![VIEWS.into(), "views.plans".into(), VIEWS.into()]);
        assert!(!state.is_open(VIEWS) && !state.is_open("views.plans"));
        assert!(state.is_open(SCANS));
        assert_eq!(state.collapsed(), [VIEWS, "views.plans"]);
        assert!(state.set_open(SCANS, false));
        assert!(!state.set_open(SCANS, false), "already collapsed");
        assert!(state.set_open(VIEWS, true));
        assert_eq!(state.collapsed(), ["views.plans", SCANS]);
        assert!(!state.set_open("", false));

        // Many collapsed folders: the oldest folder goes, the groups stay.
        for number in 0..MAX_COLLAPSED + 5 {
            state.set_open(&folder_key(Path::new(&format!("C:/scans/{number}"))), false);
        }
        assert_eq!(state.collapsed().len(), MAX_COLLAPSED);
        assert!(!state.is_open(SCANS) && !state.is_open("views.plans"));
        assert!(state.is_open(&folder_key(Path::new("C:/scans/0"))));
        assert!(!state.is_open(&folder_key(Path::new(&format!(
            "C:/scans/{}",
            MAX_COLLAPSED + 4
        )))));
    }

    #[test]
    fn the_checkbox_of_a_group_is_ticked_empty_or_mixed() {
        assert_eq!(shown_state([true, true]), Shown::All);
        assert_eq!(shown_state([false, false]), Shown::None);
        assert_eq!(shown_state([true, false, true]), Shown::Mixed);
        assert_eq!(shown_state([]), Shown::None);
    }

    #[test]
    fn scans_are_grouped_by_folder_in_name_order() {
        let folders = ["C:/b/scans 10", "C:/b/scans 2", "C:/b/scans 10", "C:/a"];
        let groups = by_folder(&[3, 0, 1, 2], |index| PathBuf::from(folders[index]));
        let names: Vec<(&str, Vec<usize>)> = groups
            .iter()
            .map(|(folder, rows)| (folder.to_str().unwrap(), rows.clone()))
            .collect();
        assert_eq!(
            names,
            [
                ("C:/a", vec![3]),
                ("C:/b/scans 2", vec![1]),
                ("C:/b/scans 10", vec![0, 2]),
            ]
        );
        assert_eq!(by_folder(&[0, 1], |_| PathBuf::from("C:/one")).len(), 1);
        assert!(by_folder(&[], |_| PathBuf::new()).is_empty());
    }

    #[test]
    fn the_summary_counts_ready_scans_and_weighs_those_under_way() {
        assert_eq!(summary(&[None, None], 0), None);
        let half = summary(&[None, Some(Some(0.5)), Some(None)], 1).unwrap();
        assert_eq!((half.done, half.total), (1, 4));
        assert!((half.fraction - 1.5 / 4.0).abs() < 1e-6);
        let waiting = summary(&[], 3).unwrap();
        assert_eq!((waiting.done, waiting.total, waiting.fraction), (0, 3, 0.0));
    }

    fn view(name: &str) -> SavedView {
        SavedView::camera(PathBuf::from("scan.laz"), name, 0.1, 0.2, 1.0, [0.0; 2])
    }

    fn drawing(name: &str, kind: SheetKind) -> SavedDrawing {
        let request =
            pointcloud_core::DrawingRequest::for_view(pointcloud_core::DrawingView::Front);
        SavedDrawing::new(
            name,
            kind,
            pointcloud_core::OrientedBox::new(
                pointcloud_core::Bounds {
                    min: [0.0; 3],
                    max: [1.0; 3],
                },
                0.0,
            ),
            &request,
            vec![PathBuf::from("scan.laz")],
        )
    }

    #[test]
    fn views_lists_the_3d_model_first_then_the_kinds_in_order() {
        let entrance = view("Entrance");
        let roof = view("Roof");
        let section = drawing("Section A", SheetKind::Section);
        let plan = drawing("Plan +1.20", SheetKind::Plan);
        let groups = view_groups(&[&entrance, &roof], &[&section, &plan], 1);
        let kinds: Vec<ViewKind> = groups.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(
            kinds,
            [
                ViewKind::ThreeD,
                ViewKind::Plans,
                ViewKind::Sections,
                ViewKind::Files
            ],
            "a kind without rows is left out"
        );
        assert_eq!(
            groups[0].1,
            [
                ViewRow::Model,
                ViewRow::Saved(entrance.guid.clone()),
                ViewRow::Saved(roof.guid.clone()),
            ]
        );
        assert_eq!(groups[1].1, [ViewRow::Drawing(plan.guid.clone())]);
        assert_eq!(groups[3].1, [ViewRow::File(0)]);
        // With nothing saved the 3D model is there all the same.
        let empty = view_groups(&[], &[], 0);
        assert_eq!(empty, [(ViewKind::ThreeD, vec![ViewRow::Model])]);
    }

    #[test]
    fn a_copy_is_named_with_the_next_free_number() {
        let taken = |names: &'static [&'static str]| {
            move |candidate: &str| {
                names
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(candidate))
            }
        };
        assert_eq!(
            duplicate_name("Plan +1.20", 96, taken(&["Plan +1.20"])),
            "Plan +1.20 (2)"
        );
        assert_eq!(
            duplicate_name("Plan +1.20", 96, taken(&["Plan +1.20", "plan +1.20 (2)"])),
            "Plan +1.20 (3)"
        );
        // A copy of a copy counts on from its number.
        assert_eq!(
            duplicate_name(
                "Plan +1.20 (2)",
                96,
                taken(&["Plan +1.20", "Plan +1.20 (2)"])
            ),
            "Plan +1.20 (3)"
        );
        assert_eq!(duplicate_name("3D model", 64, taken(&[])), "3D model (2)");
        assert_eq!(
            duplicate_name("3D model", 64, taken(&["3D model (2)"])),
            "3D model (3)"
        );
        // Brackets that hold no number of a copy are part of the name.
        assert_eq!(duplicate_name("Room (A)", 64, taken(&[])), "Room (A) (2)");
        assert_eq!(duplicate_name("Floor (1)", 64, taken(&[])), "Floor (1) (2)");
        // A long name is shortened to leave room for the number.
        let long = "x".repeat(70);
        let copy = duplicate_name(&long, 64, taken(&[]));
        assert!(
            copy.chars().count() <= 64 && copy.ends_with(" (2)"),
            "{copy}"
        );
    }

    #[test]
    fn api_names_of_groups() {
        assert_eq!(group_named("Scans").as_deref(), Some(SCANS));
        assert_eq!(group_named("plans").as_deref(), Some("views.plans"));
        assert_eq!(group_named("views.files").as_deref(), Some("views.files"));
        assert_eq!(group_named("3d").as_deref(), Some("views.3d"));
        assert_eq!(
            group_named("folder:C:/scans/North").as_deref(),
            Some("folder:C:/scans/North")
        );
        assert_eq!(group_named("folder:"), None);
        assert_eq!(group_named("drawings"), None);
    }

    /// A studio with scans in two folders, written to a folder of the test.
    fn studio_with_folders() -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let mut studio = Studio::default();
        for (folder, name) in [("north", "a.xyz"), ("south", "b.xyz"), ("north", "c.xyz")] {
            let place = directory.path().join(folder);
            std::fs::create_dir_all(&place).unwrap();
            let path = place.join(name);
            std::fs::write(&path, "0 0 0\n1 0 0\n1 1 1\n").unwrap();
            let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
            let _ = studio.update(Message::Loaded(Ok(cloud)));
        }
        (studio, directory)
    }

    #[test]
    fn the_checkboxes_of_the_bands_show_and_hide_scans_without_unloading_them() {
        let (mut studio, directory) = studio_with_folders();
        let visible = |studio: &Studio| -> Vec<bool> {
            studio.clouds.iter().map(|entry| entry.visible).collect()
        };
        let north = directory.path().join("north");
        let _ = studio.update(Message::Browser(BrowserAction::FolderVisible(
            north.clone(),
            false,
        )));
        assert_eq!(visible(&studio), [false, true, false]);
        assert_eq!(
            shown_state(studio.clouds.iter().map(|entry| entry.visible)),
            Shown::Mixed
        );
        // The list follows the folders: north, then south.
        assert_eq!(studio.layer_order(), [0, 2, 1]);
        let _ = studio.view();

        // A click on the dash shows every scan, and another hides them.
        let _ = studio.update(Message::Browser(BrowserAction::AllScansVisible(true)));
        assert_eq!(visible(&studio), [true, true, true]);
        let _ = studio.update(Message::Browser(BrowserAction::AllScansVisible(false)));
        assert_eq!(visible(&studio), [false, false, false]);

        // Collapsing a group or a folder keeps the scans as they are.
        let points: Vec<usize> = studio
            .clouds
            .iter()
            .map(|entry| entry.cloud.points.len())
            .collect();
        for group in [SCANS.to_owned(), folder_key(&north), VIEWS.to_owned()] {
            let _ = studio.update(Message::Browser(BrowserAction::Toggle(group.clone())));
            assert!(!studio.browser.is_open(&group));
            let _ = studio.view();
        }
        assert_eq!(studio.clouds.len(), 3);
        assert_eq!(visible(&studio), [false, false, false]);
        assert_eq!(
            studio
                .clouds
                .iter()
                .map(|entry| entry.cloud.points.len())
                .collect::<Vec<_>>(),
            points
        );
        // The preferences keep what is collapsed.
        let kept = studio.preferences().browser_collapsed;
        assert_eq!(
            kept,
            [SCANS.to_owned(), folder_key(&north), VIEWS.to_owned()]
        );
        let _ = studio.update(Message::Browser(BrowserAction::Toggle(SCANS.into())));
        assert!(studio.browser.is_open(SCANS));
    }

    #[test]
    fn the_3d_model_is_shown_from_a_drawing_and_views_with_a_box_are_marked() {
        let (mut studio, _directory) = studio_with_folders();
        studio.drawing_view.shown = true;
        let _ = studio.update(Message::Browser(BrowserAction::ShowModel));
        assert!(!studio.drawing_view.shown);
        let listed = studio.browser_value();
        assert_eq!(listed["views"][0]["group"], "3d");
        assert_eq!(listed["views"][0]["rows"][0], "3D model");
        assert_eq!(listed["open"][SCANS], true);

        // A view saved with the box on keeps it; one saved with it off has
        // none.
        let _ = studio.update(Message::SetSectionEnabled(true));
        let _ = studio.update(Message::Views(crate::views::ViewAction::Save));
        let _ = studio.update(Message::SetSectionEnabled(false));
        let _ = studio.update(Message::Views(crate::views::ViewAction::Save));
        let views = studio.listed_views();
        assert!(matches!(
            views[0].section,
            Some(SectionBox { enabled: true, .. })
        ));
        assert!(views[1].section.is_none());
        let listed = studio.browser_value();
        assert_eq!(listed["views"][0]["rows"][1], "View 1");
        assert_eq!(listed["views"][0]["rows"][2], "View 2");
        let _ = studio.view();
    }

    #[test]
    fn the_row_of_the_3d_model_is_highlighted_once_it_is_chosen_over_a_view() {
        use crate::views::ViewAction;

        let (mut studio, _directory) = studio_with_folders();
        assert_eq!(studio.shown_row(), Some(ViewRow::Model));
        let _ = studio.update(Message::Views(ViewAction::Save));
        let guid = studio.listed_views()[0].guid.clone();
        let _ = studio.update(Message::Views(ViewAction::Restore(guid.clone())));
        assert_eq!(studio.shown_row(), Some(ViewRow::Saved(guid.clone())));
        assert_eq!(studio.browser_value()["shown"], "View 1");
        // Orbiting keeps the view active; a click on the 3D model lets go
        // of it and highlights the 3D model, in 3D as from a drawing.
        studio.yaw += 0.3;
        let _ = studio.update(Message::Browser(BrowserAction::ShowModel));
        assert_eq!(studio.shown_row(), Some(ViewRow::Model));
        assert_eq!(studio.browser_value()["shown"], "3D model");
        assert!(studio.active_view_index().is_none());
        let _ = studio.view();
        let _ = studio.update(Message::Views(ViewAction::Restore(guid.clone())));
        studio.drawing_view.shown = true;
        assert_eq!(studio.shown_row(), None, "no drawing is held");
        let _ = studio.update(Message::Browser(BrowserAction::ShowModel));
        assert!(!studio.drawing_view.shown);
        assert_eq!(studio.shown_row(), Some(ViewRow::Model));
        // A click on the view shows it again.
        let _ = studio.update(Message::Views(ViewAction::Restore(guid.clone())));
        assert_eq!(studio.shown_row(), Some(ViewRow::Saved(guid)));
        // The local API shows the 3D model by the name of its row.
        let (reply, receive) = std::sync::mpsc::channel();
        let command =
            serde_json::from_str(r#"{"command":"show_drawing","name":"3d MODEL"}"#).unwrap();
        let _ = studio.update(Message::ApiRequest(crate::native_api::ApiRequest {
            command,
            reply,
        }));
        let answer = receive.recv().unwrap();
        assert_eq!(answer["shown"], "3D model", "{answer}");
        assert_eq!(studio.shown_row(), Some(ViewRow::Model));
    }
}
