//! Sheets: paper of the A series with a border and a title block, on which
//! saved 3D views, plans, elevations and sections are placed, to be printed
//! as a PDF. In the code a "sheet" is a drawing of Create 2D, as in
//! `sheet_dialog`; the paper is a layout here, and "Sheets" in the window.
//!
//! The sheets are listed under SHEETS in the Project Browser, between VIEWS
//! and BCF, and kept in `sheets.json` beside the saved views. A sheet opens
//! as a tab and shows in the main area in place of a drawing: the Drawing
//! view stays shown, and `DrawingViewTool::layout` names the sheet in front
//! of its drawing. A drawing is placed at a scale and shows its crop region
//! with every layer as it is made; a 3D view shows the snapshot of the view,
//! the picture the viewport took when it last showed the view. A viewport
//! follows its view: a drawing made again, a crop region changed, a view
//! renamed or updated show on the sheet, and a deleted view leaves "view
//! missing". A drawing placed on a sheet that is not made in this session
//! is made in the background while the sheet is shown.

mod api;
mod canvas;
mod drag;
pub(crate) mod model;
mod panels;
mod pdf;
pub(crate) mod plot;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use iced::widget::{canvas as iced_canvas, text_input};
use iced::{Size, Task};

use crate::drawing_crop::{crop_frame, Remake};
use crate::drawing_view::ViewCamera;
use crate::i18n::{tr, tr_args};
use crate::saved_drawings::SavedDrawing;
use crate::{Message, Studio};

pub(crate) use api::{made_for_sheet, PlaceOptions, SheetOptions, ViewportOptions, ViewportRef};
use model::{
    drawing_size, image_size, parse_scale, scale_label, scale_valid, MARGIN, MAX_NAME_CHARS,
    MAX_NUMBER_CHARS, MAX_SHEETS, MAX_VIEWPORTS, TITLE_BLOCK,
};
pub(crate) use model::{Layout, Paper, PaperNote, PlacedKind, Viewport};
use plot::{Content, Plot};

/// The group of the Project Browser, by the key the preferences keep it
/// under.
pub(crate) const SHEETS: &str = "sheets";
/// The picture of a 3D view is looked at again at most this often.
const PICTURE_CHECK: Duration = Duration::from_millis(400);
/// The pixels a snapshot is taken to have before it is read.
const PICTURE_GUESS: [u32; 2] = [1600, 1000];
/// The operation a drawing made for a sheet reports.
pub(crate) const MADE_FOR_SHEET: &str = "place_on_sheet";

/// A paper lying on its long side or standing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Orientation {
    Landscape,
    Portrait,
}

impl Orientation {
    pub const ALL: [Self; 2] = [Self::Landscape, Self::Portrait];

    pub fn landscape(self) -> bool {
        self == Self::Landscape
    }

    pub fn of(landscape: bool) -> Self {
        if landscape {
            Self::Landscape
        } else {
            Self::Portrait
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Landscape => "landscape",
            Self::Portrait => "portrait",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|orientation| orientation.key().eq_ignore_ascii_case(value.trim()))
    }
}

impl std::fmt::Display for Orientation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Landscape => tr("Landscape"),
            Self::Portrait => tr("Portrait"),
        })
    }
}

/// A view or drawing that can be placed, as the list in Properties offers
/// it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Placeable {
    pub kind: PlacedKind,
    pub guid: String,
    pub name: String,
}

impl std::fmt::Display for Placeable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            PlacedKind::View => tr("3D view"),
            PlacedKind::Drawing => tr("drawing"),
        };
        write!(f, "{} ({kind})", self.name)
    }
}

/// A scale the list of a viewport offers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScaleChoice(pub f64);

impl std::fmt::Display for ScaleChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&scale_label(self.0))
    }
}

/// The fields of a selected viewport that take a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Field {
    X,
    Y,
    Width,
    Height,
    Scale,
}

/// What "New sheet…" asks for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct NewSheet {
    pub number: String,
    pub name: String,
    pub paper: Paper,
    pub orientation: Orientation,
}

/// The picture of a 3D view, read from its snapshot.
#[derive(Debug, Clone)]
pub(crate) struct Picture {
    pub handle: iced::widget::image::Handle,
    pub pixels: [u32; 2],
    pub png: Arc<Vec<u8>>,
    stamp: Option<SystemTime>,
}

/// The sheets and what the sheet shown is doing.
pub(crate) struct LayoutTool {
    pub(crate) list: Vec<Layout>,
    /// The selected viewport of the sheet shown.
    pub(crate) selected: Option<String>,
    /// Where each sheet was looked at, by its identifier.
    looks: HashMap<String, ViewCamera>,
    pub(crate) camera: Cell<ViewCamera>,
    pub(crate) fit_pending: Cell<bool>,
    /// Where the canvas lies in the window, as it was last drawn.
    pub(crate) bounds: Cell<Option<iced::Rectangle>>,
    pub(crate) form: Option<NewSheet>,
    renaming: Option<(String, String)>,
    /// What is typed in the number fields of the selected viewport.
    edits: (Option<String>, Vec<(Field, String)>),
    /// A row of VIEWS pressed while a sheet is shown, which is placed where
    /// it is let go over the paper.
    pub(crate) drag_row: Option<(PlacedKind, String)>,
    pub(crate) place: Option<Placeable>,
    pub(crate) images: HashMap<String, Picture>,
    pictures_checked: Option<Instant>,
    /// The drawing being made for the sheet shown, and those that could not
    /// be made, with why.
    pub(crate) making: Option<String>,
    pub(crate) unmade: HashMap<String, String>,
    /// Changes when what the sheet shows may have changed.
    revision: u64,
    fingerprint: u64,
    pub(crate) cache: iced_canvas::Cache,
    pub(crate) built: Cell<Option<canvas::Built>>,
    plot: RefCell<Option<(String, u64, Arc<Plot>)>>,
    pub(crate) export_pending: bool,
}

impl Default for LayoutTool {
    fn default() -> Self {
        Self::with(Vec::new())
    }
}

impl std::fmt::Debug for LayoutTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LayoutTool")
            .field("sheets", &self.list.len())
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

impl LayoutTool {
    pub(crate) fn load() -> Self {
        Self::with(model::load())
    }

    fn with(list: Vec<Layout>) -> Self {
        Self {
            list,
            selected: None,
            looks: HashMap::new(),
            camera: Cell::new(ViewCamera::default()),
            fit_pending: Cell::new(true),
            bounds: Cell::new(None),
            form: None,
            renaming: None,
            edits: (None, Vec::new()),
            drag_row: None,
            place: None,
            images: HashMap::new(),
            pictures_checked: None,
            making: None,
            unmade: HashMap::new(),
            revision: 0,
            fingerprint: 0,
            cache: iced_canvas::Cache::new(),
            built: Cell::new(None),
            plot: RefCell::new(None),
            export_pending: false,
        }
    }

    pub(crate) fn layout(&self, guid: &str) -> Option<&Layout> {
        self.list.iter().find(|layout| layout.guid == guid)
    }

    /// What the geometry of the canvas was built from.
    pub(crate) fn drawn_revision(&self) -> u64 {
        self.revision
    }

    /// What the sheet shows changed.
    fn changed(&mut self) {
        self.revision += 1;
    }

    /// The annotations of a drawing or a sheet changed: the sheet shown
    /// draws them again.
    pub(crate) fn annotations_changed(&mut self) {
        self.changed();
    }

    fn store(&self) -> Result<(), String> {
        model::save(&self.list).map_err(|error| format!("The sheets could not be stored: {error}"))
    }

    /// What "New sheet…" starts with: the next number and name, and A3
    /// landscape.
    fn new_form(&self) -> NewSheet {
        let taken = |number: &str| {
            self.list
                .iter()
                .any(|layout| layout.number.eq_ignore_ascii_case(number))
        };
        let number = (1..=MAX_SHEETS + 1)
            .map(|place| format!("{place:02}"))
            .find(|number| !taken(number))
            .unwrap_or_default();
        let name = (1..=MAX_SHEETS + 1)
            .map(|place| format!("Sheet {place}"))
            .find(|name| {
                !self
                    .list
                    .iter()
                    .any(|layout| layout.name.eq_ignore_ascii_case(name))
            })
            .unwrap_or_else(|| "Sheet".into());
        NewSheet {
            number,
            name,
            paper: Paper::A3,
            orientation: Orientation::Landscape,
        }
    }

    /// A sheet by its identifier, its number or its name, in any case.
    pub(crate) fn named(&self, name: &str) -> Option<&Layout> {
        let name = name.trim();
        self.list
            .iter()
            .find(|layout| layout.guid == name)
            .or_else(|| {
                self.list.iter().find(|layout| {
                    layout.number.eq_ignore_ascii_case(name)
                        || layout.name.eq_ignore_ascii_case(name)
                        || layout.caption().eq_ignore_ascii_case(name)
                })
            })
    }
}

/// Everything the sheets react to. Sheets and viewports are named by their
/// identifier.
#[derive(Debug, Clone)]
pub(crate) enum LayoutAction {
    /// Open "New sheet…", or close it.
    NewSheet,
    FormNumber(String),
    FormName(String),
    FormPaper(Paper),
    FormOrientation(Orientation),
    Create,
    Show(String),
    StartRename(String),
    RenameText(String),
    FinishRename,
    CancelRename,
    Duplicate(String),
    Delete(String),
    /// The fields of the sheet shown.
    Number(String),
    Name(String),
    SetPaper(Paper),
    SetOrientation(Orientation),
    Project(String),
    Date(String),
    DrawnBy(String),
    /// The paper was dragged by some pixels.
    Pan([f32; 2]),
    /// Zoom by a factor about a pixel of a canvas of a size.
    Zoom(f32, [f32; 2], Size),
    Fit,
    Select(Option<String>),
    /// Move a viewport so that its middle lies here on the paper.
    Move(String, [f64; 2]),
    Remove(String),
    /// Delete was pressed while the sheet is shown.
    DeleteSelected,
    ScaleChosen(ScaleChoice),
    Typed(Field, String),
    Title(String),
    PlaceChoice(Placeable),
    Place,
    /// A row of VIEWS was pressed while a sheet is shown.
    DragRow(PlacedKind, String),
    /// It was let go over the paper here, or elsewhere.
    Drop([f64; 2]),
    DragEnd,
    ExportPdf,
    PdfPathChosen(Option<PathBuf>),
    PdfWritten(Option<String>, Result<(PathBuf, u64), String>),
}

/// Why a text cannot be the name or the number of a sheet.
fn checked_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "A sheet needs a name of 1 to {MAX_NAME_CHARS} characters"
        ));
    }
    Ok(name.to_owned())
}

fn checked_number(number: &str) -> Result<String, String> {
    let number = number.trim();
    if number.chars().count() > MAX_NUMBER_CHARS {
        return Err(format!(
            "The number of a sheet has at most {MAX_NUMBER_CHARS} characters"
        ));
    }
    Ok(number.to_owned())
}

/// The crop region of a drawing in metres in the frame of the drawing.
pub(crate) fn drawing_rect(definition: &SavedDrawing) -> Option<[[f64; 2]; 2]> {
    let request = definition.request()?;
    crop_frame(definition.oriented(), request.view, request.origin).map(|crop| crop.rect)
}

/// The room an image of a 3D view gets on a paper when it is placed.
fn image_room(layout: &Layout) -> [f64; 2] {
    let [width, height] = layout.size();
    [
        (width - 2.0 * MARGIN) * 0.55,
        (height - 2.0 * MARGIN - TITLE_BLOCK[1]) * 0.6,
    ]
}

impl Studio {
    pub(crate) fn update_layouts(&mut self, action: LayoutAction) -> Task<Message> {
        let shown = self.drawing_view.shown_layout().map(str::to_owned);
        match action {
            LayoutAction::NewSheet => {
                self.layouts.form = match self.layouts.form {
                    Some(_) => None,
                    None => Some(self.layouts.new_form()),
                };
                if self.layouts.form.is_some() && self.browser.set_open(SHEETS, true) {
                    return self.queue_preferences_save();
                }
            }
            LayoutAction::FormNumber(value) => {
                if let Some(form) = &mut self.layouts.form {
                    form.number = value;
                }
            }
            LayoutAction::FormName(value) => {
                if let Some(form) = &mut self.layouts.form {
                    form.name = value;
                }
            }
            LayoutAction::FormPaper(paper) => {
                if let Some(form) = &mut self.layouts.form {
                    form.paper = paper;
                }
            }
            LayoutAction::FormOrientation(orientation) => {
                if let Some(form) = &mut self.layouts.form {
                    form.orientation = orientation;
                }
            }
            LayoutAction::Create => {
                let Some(form) = self.layouts.form.clone() else {
                    return Task::none();
                };
                match self.create_layout(
                    &form.number,
                    &form.name,
                    form.paper,
                    form.orientation.landscape(),
                ) {
                    Ok(guid) => {
                        self.layouts.form = None;
                        return self.show_layout(&guid).unwrap_or_else(|error| {
                            self.status = error;
                            Task::none()
                        });
                    }
                    Err(error) => self.status = error,
                }
            }
            LayoutAction::Show(guid) => match self.show_layout(&guid) {
                Ok(task) => return task,
                Err(error) => self.status = error,
            },
            LayoutAction::StartRename(guid) => {
                if let Some(layout) = self.layouts.layout(&guid) {
                    self.layouts.renaming = Some((guid, layout.name.clone()));
                    return text_input::focus(rename_input_id());
                }
            }
            LayoutAction::RenameText(value) => {
                if let Some((_, name)) = &mut self.layouts.renaming {
                    *name = value;
                }
            }
            LayoutAction::FinishRename => {
                if let Some((guid, name)) = self.layouts.renaming.clone() {
                    match self.set_layout_field(&guid, |layout| {
                        layout.name = checked_name(&name)?;
                        Ok(())
                    }) {
                        Ok(()) => self.layouts.renaming = None,
                        Err(error) => self.status = error,
                    }
                }
            }
            LayoutAction::CancelRename => self.layouts.renaming = None,
            LayoutAction::Duplicate(guid) => match self.duplicate_layout(&guid) {
                Ok(copy) => {
                    return self.show_layout(&copy).unwrap_or_else(|error| {
                        self.status = error;
                        Task::none()
                    })
                }
                Err(error) => self.status = error,
            },
            LayoutAction::Delete(guid) => {
                if let Err(error) = self.delete_layout(&guid) {
                    self.status = error;
                }
            }
            LayoutAction::Number(value) => {
                if let Some(guid) = shown {
                    self.edit_layout(&guid, |layout| {
                        layout.number = checked_number(&value)?;
                        Ok(())
                    });
                }
            }
            LayoutAction::Name(value) => {
                if let Some(guid) = shown {
                    self.edit_layout(&guid, |layout| {
                        layout.name = checked_name(&value)?;
                        Ok(())
                    });
                }
            }
            LayoutAction::SetPaper(paper) => {
                if let Some(guid) = shown {
                    self.edit_layout(&guid, |layout| {
                        layout.paper = paper;
                        Ok(())
                    });
                    self.layouts.fit_pending.set(true);
                }
            }
            LayoutAction::SetOrientation(orientation) => {
                if let Some(guid) = shown {
                    self.edit_layout(&guid, |layout| {
                        layout.landscape = orientation.landscape();
                        Ok(())
                    });
                    self.layouts.fit_pending.set(true);
                }
            }
            LayoutAction::Project(value) => {
                if let Some(guid) = shown {
                    self.edit_layout(&guid, |layout| {
                        layout.project = value.chars().take(MAX_NAME_CHARS).collect();
                        Ok(())
                    });
                }
            }
            LayoutAction::Date(value) => {
                if let Some(guid) = shown {
                    self.edit_layout(&guid, |layout| {
                        layout.date = value.chars().take(MAX_NUMBER_CHARS).collect();
                        Ok(())
                    });
                }
            }
            LayoutAction::DrawnBy(value) => {
                if let Some(guid) = shown {
                    self.edit_layout(&guid, |layout| {
                        layout.drawn_by = value.chars().take(MAX_NAME_CHARS).collect();
                        Ok(())
                    });
                }
            }
            LayoutAction::Pan(delta) => {
                let camera = self.layouts.camera.get().panned(delta);
                self.layouts.camera.set(camera);
            }
            LayoutAction::Zoom(factor, pixel, size) => {
                let camera = self.layouts.camera.get();
                let zoomed = camera.zoomed(f64::from(factor), pixel, size);
                // From a hundredth of a pixel to two hundred pixels a
                // millimetre.
                if (0.01..=200.0).contains(&zoomed.scale) {
                    self.layouts.camera.set(zoomed);
                }
            }
            LayoutAction::Fit => self.layouts.fit_pending.set(true),
            LayoutAction::Select(id) => self.select_viewport(id),
            LayoutAction::Move(id, centre) => {
                if let Some(guid) = shown {
                    if let Err(error) = self.move_viewport(&guid, &id, centre) {
                        self.status = error;
                    }
                    self.select_viewport(Some(id));
                }
            }
            LayoutAction::Remove(id) => {
                if let Some(guid) = shown {
                    if let Err(error) = self.remove_viewport(&guid, &id) {
                        self.status = error;
                    }
                }
            }
            LayoutAction::DeleteSelected => {
                if let (Some(guid), Some(id)) = (shown, self.layouts.selected.clone()) {
                    if let Err(error) = self.remove_viewport(&guid, &id) {
                        self.status = error;
                    }
                }
            }
            LayoutAction::ScaleChosen(ScaleChoice(scale)) => {
                if let (Some(guid), Some(id)) = (shown, self.layouts.selected.clone()) {
                    self.layouts
                        .edits
                        .1
                        .retain(|(field, _)| *field != Field::Scale);
                    if let Err(error) = self.set_viewport_scale(&guid, &id, scale) {
                        self.status = error;
                    }
                }
            }
            LayoutAction::Typed(field, value) => {
                if let (Some(guid), Some(id)) = (shown, self.layouts.selected.clone()) {
                    self.typed_field(&guid, &id, field, value);
                }
            }
            LayoutAction::Title(value) => {
                if let (Some(guid), Some(id)) = (shown, self.layouts.selected.clone()) {
                    let title = value.chars().take(MAX_NAME_CHARS).collect::<String>();
                    self.edit_layout(&guid, |layout| {
                        let viewport = layout
                            .viewport_mut(&id)
                            .ok_or_else(|| "That viewport is no longer on the sheet".to_owned())?;
                        viewport.title = Some(title);
                        Ok(())
                    });
                }
            }
            LayoutAction::PlaceChoice(choice) => self.layouts.place = Some(choice),
            LayoutAction::Place => {
                if let (Some(guid), Some(choice)) = (shown, self.layouts.place.clone()) {
                    match self.place_on_layout(&guid, choice.kind, &choice.guid, None, None) {
                        Ok(_) => self.layouts.place = None,
                        Err(error) => self.status = error,
                    }
                }
            }
            LayoutAction::DragRow(kind, guid) => {
                if shown.is_some() {
                    self.layouts.drag_row = Some((kind, guid));
                }
            }
            LayoutAction::Drop(at) => {
                if let (Some(sheet), Some((kind, guid))) = (shown, self.layouts.drag_row.take()) {
                    if let Err(error) = self.place_on_layout(&sheet, kind, &guid, Some(at), None) {
                        self.status = error;
                    }
                }
            }
            LayoutAction::DragEnd => self.layouts.drag_row = None,
            LayoutAction::ExportPdf => {
                let Some(guid) = shown else {
                    return Task::none();
                };
                if self.layouts.export_pending {
                    return Task::none();
                }
                let Some(layout) = self.layouts.layout(&guid) else {
                    return Task::none();
                };
                let suggested = format!("{}.pdf", file_stem(&layout.caption()));
                self.layouts.export_pending = true;
                self.status = "Choose where to save the sheet as PDF…".into();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .add_filter("PDF", &["pdf"])
                            .set_file_name(suggested)
                            .save_file()
                            .await
                            .map(|chosen| chosen.path().to_path_buf())
                    },
                    |path| Message::Layouts(LayoutAction::PdfPathChosen(path)),
                );
            }
            LayoutAction::PdfPathChosen(path) => {
                self.layouts.export_pending = false;
                let (Some(mut path), Some(guid)) = (path, shown) else {
                    self.status = "PDF export cancelled".into();
                    return Task::none();
                };
                if !path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
                {
                    path.set_extension("pdf");
                }
                return match self.start_pdf(&guid, path, None) {
                    Ok(task) => task,
                    Err(error) => {
                        self.status = error;
                        Task::none()
                    }
                };
            }
            LayoutAction::PdfWritten(job, result) => {
                self.layouts.export_pending = false;
                let value = match &result {
                    Ok((path, bytes)) => {
                        self.status = format!(
                            "Sheet written to {} ({})",
                            path.display(),
                            crate::drawing::size_text(*bytes)
                        );
                        serde_json::json!({"state": "complete", "operation": "export_sheet_pdf", "path": path, "bytes": bytes})
                    }
                    Err(error) => {
                        self.status = format!("The PDF could not be written: {error}");
                        serde_json::json!({"state": "failed", "operation": "export_sheet_pdf", "error": error})
                    }
                };
                if let Some(entry) = job.and_then(|id| self.api_jobs.get_mut(&id)) {
                    *entry = value;
                }
            }
        }
        Task::none()
    }

    /// Change a field of a sheet with `change`, keep it, and say why when
    /// it cannot be changed.
    fn edit_layout(&mut self, guid: &str, change: impl FnOnce(&mut Layout) -> Result<(), String>) {
        if let Err(error) = self.set_layout_field(guid, change) {
            self.status = error;
        }
    }

    /// Change a sheet with `change` and keep it; nothing changes when it is
    /// refused or cannot be kept.
    pub(crate) fn set_layout_field(
        &mut self,
        guid: &str,
        change: impl FnOnce(&mut Layout) -> Result<(), String>,
    ) -> Result<(), String> {
        let place = self
            .layouts
            .list
            .iter()
            .position(|layout| layout.guid == guid)
            .ok_or_else(|| "That sheet is no longer kept".to_owned())?;
        let before = self.layouts.list[place].clone();
        change(&mut self.layouts.list[place])?;
        if self.layouts.list[place] == before {
            return Ok(());
        }
        if let Err(error) = self.layouts.store() {
            self.layouts.list[place] = before;
            return Err(error);
        }
        self.layouts.changed();
        Ok(())
    }

    /// Make a new sheet and keep it; answers its identifier.
    pub(crate) fn create_layout(
        &mut self,
        number: &str,
        name: &str,
        paper: Paper,
        landscape: bool,
    ) -> Result<String, String> {
        if self.layouts.list.len() >= MAX_SHEETS {
            return Err(format!("At most {MAX_SHEETS} sheets are kept"));
        }
        let layout = Layout::new(
            &checked_number(number)?,
            &checked_name(name)?,
            paper,
            landscape,
        );
        let guid = layout.guid.clone();
        self.layouts.list.push(layout);
        if let Err(error) = self.layouts.store() {
            self.layouts.list.pop();
            return Err(error);
        }
        self.status = format!(
            "Sheet {} made",
            self.layouts
                .list
                .last()
                .map_or_else(String::new, Layout::caption)
        );
        Ok(guid)
    }

    /// A copy of a sheet with its viewports, named with the next free
    /// number, right below it.
    pub(crate) fn duplicate_layout(&mut self, guid: &str) -> Result<String, String> {
        if self.layouts.list.len() >= MAX_SHEETS {
            return Err(format!("At most {MAX_SHEETS} sheets are kept"));
        }
        let place = self
            .layouts
            .list
            .iter()
            .position(|layout| layout.guid == guid)
            .ok_or_else(|| "That sheet is no longer kept".to_owned())?;
        let original = self.layouts.list[place].clone();
        let list = &self.layouts.list;
        let name = crate::project_browser::duplicate_name(&original.name, MAX_NAME_CHARS, |name| {
            list.iter()
                .any(|layout| layout.name.eq_ignore_ascii_case(name))
        });
        let mut copy = original.clone();
        copy.guid = crate::camera_views::new_guid();
        copy.name = name;
        copy.created = crate::camera_views::now_seconds();
        for viewport in &mut copy.viewports {
            viewport.id = crate::camera_views::new_guid();
        }
        // Its notes are its own, to be named apart from those of the
        // original.
        copy.notes = original.notes.iter().map(PaperNote::renewed).collect();
        let guid = copy.guid.clone();
        self.layouts.list.insert(place + 1, copy);
        if let Err(error) = self.layouts.store() {
            self.layouts.list.remove(place + 1);
            return Err(error);
        }
        self.status = format!("Sheet {} duplicated", original.caption());
        Ok(guid)
    }

    /// Forget a sheet; its tab closes with it.
    pub(crate) fn delete_layout(&mut self, guid: &str) -> Result<String, String> {
        let place = self
            .layouts
            .list
            .iter()
            .position(|layout| layout.guid == guid)
            .ok_or_else(|| "That sheet is no longer kept".to_owned())?;
        let removed = self.layouts.list.remove(place);
        if let Err(error) = self.layouts.store() {
            self.layouts.list.insert(place, removed);
            return Err(error);
        }
        self.layouts.looks.remove(guid);
        if self
            .layouts
            .renaming
            .as_ref()
            .is_some_and(|(renamed, _)| renamed == guid)
        {
            self.layouts.renaming = None;
        }
        if self.drawing_view.layout.as_deref() == Some(guid) {
            self.drawing_view.layout = None;
            self.drawing_view.shown = false;
            self.layouts.selected = None;
        }
        self.status = format!("Sheet {} deleted", removed.caption());
        Ok(removed.caption())
    }

    /// Show a sheet in the main area, looked at as it was left.
    pub(crate) fn show_layout(&mut self, guid: &str) -> Result<Task<Message>, String> {
        let caption = self
            .layouts
            .layout(guid)
            .map(Layout::caption)
            .ok_or_else(|| "That sheet is no longer kept".to_owned())?;
        if self.drawing_view.shown_layout() != Some(guid) {
            if let Some(earlier) = self.drawing_view.shown_layout().map(str::to_owned) {
                let camera = self.layouts.camera.get();
                self.layouts.looks.insert(earlier, camera);
            }
            match self.layouts.looks.get(guid) {
                Some(camera) => {
                    self.layouts.camera.set(*camera);
                    self.layouts.fit_pending.set(false);
                }
                None => self.layouts.fit_pending.set(true),
            }
            self.layouts.selected = None;
            self.layouts.edits = (None, Vec::new());
        }
        self.layouts.unmade.clear();
        self.layouts.pictures_checked = None;
        self.drawing_view.layout = Some(guid.to_owned());
        self.drawing_view.shown = true;
        self.file_open = false;
        self.layouts.changed();
        self.tabs
            .add(crate::view_tabs::TabId::Layout(guid.to_owned()));
        self.status = format!("Sheet {caption}");
        Ok(Task::none())
    }

    /// Select a viewport of the sheet shown, or none. A viewport selected
    /// lets go of the selected note, so that Delete takes the viewport.
    pub(crate) fn select_viewport(&mut self, id: Option<String>) {
        if self.layouts.selected != id {
            self.layouts.edits = (id.clone(), Vec::new());
        }
        if id.is_some() {
            self.viewport_selected();
        }
        self.layouts.selected = id;
    }

    /// Place a view or a drawing on a sheet: at `at` on the paper, else at
    /// the first free place; a drawing at `scale`, else at 1:100. Answers
    /// the identifier of the viewport, which is selected.
    pub(crate) fn place_on_layout(
        &mut self,
        sheet: &str,
        kind: PlacedKind,
        guid: &str,
        at: Option<[f64; 2]>,
        scale: Option<f64>,
    ) -> Result<String, String> {
        let layout = self
            .layouts
            .layout(sheet)
            .ok_or_else(|| "That sheet is no longer kept".to_owned())?;
        if layout.viewports.len() >= MAX_VIEWPORTS {
            return Err(format!("A sheet holds at most {MAX_VIEWPORTS} views"));
        }
        let scale = scale.unwrap_or(model::DEFAULT_SCALE);
        if !scale_valid(scale) {
            return Err("A scale lies between 1:1 and 1:100000".into());
        }
        let (name, size) = match kind {
            PlacedKind::Drawing => {
                let definition = self
                    .drawing_view
                    .saved
                    .iter()
                    .find(|drawing| drawing.guid == guid)
                    .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
                let rect = drawing_rect(definition)
                    .ok_or_else(|| format!("The drawing {} has no crop region", definition.name))?;
                (definition.name.clone(), drawing_size(rect, scale))
            }
            PlacedKind::View => {
                let view = self
                    .views
                    .list
                    .iter()
                    .find(|view| view.guid == guid)
                    .ok_or_else(|| "That view is no longer saved".to_owned())?;
                let pixels = self.picture_pixels(guid);
                (view.name.clone(), image_size(pixels, image_room(layout)))
            }
        };
        let [width, height] = layout.size();
        let centre = match at {
            Some(at) => [at[0].clamp(0.0, width), at[1].clamp(0.0, height)],
            None => layout.free_place(size, None),
        };
        let mut viewport = Viewport::new(kind, guid, &name, centre, size);
        viewport.scale = scale;
        let id = viewport.id.clone();
        let caption = layout.caption();
        self.set_layout_field(sheet, |layout| {
            layout.viewports.push(viewport);
            Ok(())
        })?;
        self.select_viewport(Some(id.clone()));
        self.status = format!("{name} placed on sheet {caption}");
        Ok(id)
    }

    /// The pixels of the picture of a 3D view: as read, else from the
    /// header of its snapshot, else a guess.
    fn picture_pixels(&self, guid: &str) -> [u32; 2] {
        if let Some(picture) = self.layouts.images.get(guid) {
            return picture.pixels;
        }
        crate::camera_views::read_snapshot(guid)
            .and_then(|png| model::png_size(&png))
            .unwrap_or(PICTURE_GUESS)
    }

    pub(crate) fn move_viewport(
        &mut self,
        sheet: &str,
        id: &str,
        centre: [f64; 2],
    ) -> Result<(), String> {
        if !centre.iter().all(|value| value.is_finite()) {
            return Err("A place on the paper needs two finite numbers".into());
        }
        self.set_layout_field(sheet, |layout| {
            let [width, height] = layout.size();
            let viewport = layout
                .viewport_mut(id)
                .ok_or_else(|| "That viewport is no longer on the sheet".to_owned())?;
            refuse_locked(viewport)?;
            viewport.centre = [centre[0].clamp(0.0, width), centre[1].clamp(0.0, height)];
            Ok(())
        })
    }

    pub(crate) fn remove_viewport(&mut self, sheet: &str, id: &str) -> Result<String, String> {
        let mut name = String::new();
        self.set_layout_field(sheet, |layout| {
            let place = layout
                .viewports
                .iter()
                .position(|viewport| viewport.id == id)
                .ok_or_else(|| "That viewport is no longer on the sheet".to_owned())?;
            refuse_locked(&layout.viewports[place])?;
            name = layout.viewports.remove(place).shown_title().to_owned();
            Ok(())
        })?;
        if self.layouts.selected.as_deref() == Some(id) {
            self.select_viewport(None);
        }
        self.status = format!("{name} taken off the sheet");
        Ok(name)
    }

    /// The scale of a drawing on a sheet; its size follows.
    pub(crate) fn set_viewport_scale(
        &mut self,
        sheet: &str,
        id: &str,
        scale: f64,
    ) -> Result<(), String> {
        if !scale_valid(scale) {
            return Err("A scale lies between 1:1 and 1:100000".into());
        }
        let rects: HashMap<String, [[f64; 2]; 2]> = self
            .drawing_view
            .saved
            .iter()
            .filter_map(|drawing| Some((drawing.guid.clone(), drawing_rect(drawing)?)))
            .collect();
        self.set_layout_field(sheet, |layout| {
            let viewport = layout
                .viewport_mut(id)
                .ok_or_else(|| "That viewport is no longer on the sheet".to_owned())?;
            refuse_locked(viewport)?;
            if viewport.kind != PlacedKind::Drawing {
                return Err("A 3D view on a sheet has no scale; set its size instead".into());
            }
            viewport.scale = scale;
            if let Some(rect) = rects.get(&viewport.guid) {
                viewport.size = drawing_size(*rect, scale);
            }
            Ok(())
        })
    }

    /// The size of the image of a 3D view on a sheet, its proportions kept
    /// by the side given: a width or a height; with both, the largest that
    /// fits within them.
    pub(crate) fn set_viewport_size(
        &mut self,
        sheet: &str,
        id: &str,
        width: Option<f64>,
        height: Option<f64>,
    ) -> Result<(), String> {
        let valid = |side: f64| side.is_finite() && (model::MIN_SIDE..=2000.0).contains(&side);
        if width.is_some_and(|side| !valid(side)) || height.is_some_and(|side| !valid(side)) {
            return Err("A side of a viewport lies between 5 and 2000 mm".into());
        }
        self.set_layout_field(sheet, |layout| {
            let viewport = layout
                .viewport_mut(id)
                .ok_or_else(|| "That viewport is no longer on the sheet".to_owned())?;
            refuse_locked(viewport)?;
            if viewport.kind != PlacedKind::View {
                return Err(
                    "A drawing on a sheet takes the size of its crop region at its scale".into(),
                );
            }
            let ratio = viewport.size[1] / viewport.size[0].max(1e-9);
            viewport.size = match (width, height) {
                (Some(width), Some(height)) => {
                    if width * ratio <= height {
                        [width, width * ratio]
                    } else {
                        [height / ratio, height]
                    }
                }
                (Some(width), None) => [width, width * ratio],
                (None, Some(height)) => [height / ratio, height],
                (None, None) => viewport.size,
            };
            Ok(())
        })
    }

    /// A number typed in a field of the selected viewport, applied while it
    /// reads as one.
    fn typed_field(&mut self, sheet: &str, id: &str, field: Field, value: String) {
        let edits = &mut self.layouts.edits;
        if edits.0.as_deref() != Some(id) {
            *edits = (Some(id.to_owned()), Vec::new());
        }
        edits.1.retain(|(known, _)| *known != field);
        edits.1.push((field, value.clone()));
        let Some(viewport) = self
            .layouts
            .layout(sheet)
            .and_then(|layout| layout.viewport(id))
            .cloned()
        else {
            return;
        };
        let number = value.trim().replace(',', ".").parse::<f64>().ok();
        let result = match (field, number) {
            (Field::Scale, _) => match parse_scale(&value) {
                Some(scale) => self.set_viewport_scale(sheet, id, scale),
                None => return,
            },
            (_, None) => return,
            (Field::X, Some(x)) => self.move_viewport(sheet, id, [x, viewport.centre[1]]),
            (Field::Y, Some(y)) => self.move_viewport(sheet, id, [viewport.centre[0], y]),
            (Field::Width, Some(width)) => self.set_viewport_size(sheet, id, Some(width), None),
            (Field::Height, Some(height)) => self.set_viewport_size(sheet, id, None, Some(height)),
        };
        if let Err(error) = result {
            self.status = error;
        }
    }

    /// What a viewport shows now.
    fn viewport_content(&self, viewport: &Viewport) -> Content<'_> {
        match viewport.kind {
            PlacedKind::Drawing => {
                let Some(definition) = self
                    .drawing_view
                    .saved
                    .iter()
                    .find(|drawing| drawing.guid == viewport.guid)
                else {
                    return Content::Missing;
                };
                let Some(rect) = drawing_rect(definition) else {
                    return Content::Waiting(tr("This drawing has no crop region").to_owned());
                };
                match (
                    self.drawing_view.made(&viewport.guid),
                    crate::drawing_notes::drawing_frame(definition),
                ) {
                    (Some(scene), Some(frame)) => Content::Drawing {
                        scene,
                        rect,
                        notes: &definition.annotations,
                        frame,
                        value_scale: crate::drawing_notes::drawing_scale(definition),
                    },
                    _ => Content::Waiting(self.why_unmade(definition)),
                }
            }
            PlacedKind::View => {
                if !self
                    .views
                    .list
                    .iter()
                    .any(|view| view.guid == viewport.guid)
                {
                    return Content::Missing;
                }
                match self.layouts.images.get(&viewport.guid) {
                    Some(picture) => Content::Image {
                        key: viewport.guid.clone(),
                        pixels: picture.pixels,
                    },
                    None => Content::Waiting(
                        tr("No picture of this view yet: show it once in the 3D model").to_owned(),
                    ),
                }
            }
        }
    }

    /// Why a drawing on a sheet is not shown yet.
    fn why_unmade(&self, definition: &SavedDrawing) -> String {
        if self.layouts.making.as_deref() == Some(definition.guid.as_str()) {
            return tr("Being made…").to_owned();
        }
        if let Some(why) = self.layouts.unmade.get(&definition.guid) {
            return why.clone();
        }
        let open = self.open_sources();
        let missing: Vec<String> = definition
            .sources
            .iter()
            .filter(|source| !open.contains(source))
            .map(|source| crate::display_name(source).to_owned())
            .collect();
        if !missing.is_empty() {
            return tr_args(
                "Open {scans} to show this drawing",
                &[("scans", &missing.join(", "))],
            );
        }
        tr("Waiting to be made…").to_owned()
    }

    /// The plot of the sheet shown, built again when what it shows changed.
    pub(crate) fn shown_plot(&self) -> Option<Arc<Plot>> {
        let guid = self.drawing_view.shown_layout()?;
        let revision = self.layouts.revision;
        if let Some((known, built, plot)) = &*self.layouts.plot.borrow() {
            if known == guid && *built == revision {
                return Some(Arc::clone(plot));
            }
        }
        let layout = self.layouts.layout(guid)?;
        let plot = Arc::new(plot::plot(layout, |viewport| {
            self.viewport_content(viewport)
        }));
        *self.layouts.plot.borrow_mut() = Some((guid.to_owned(), revision, Arc::clone(&plot)));
        Some(plot)
    }

    /// Follow what the sheet shown shows, after every message: a drawing
    /// made for it arrived or could not be made, a view was renamed, a crop
    /// region or a picture changed. Starts making the next drawing it lacks
    /// and answers that job.
    pub(crate) fn settle_layouts(&mut self) -> Option<Task<Message>> {
        let Some(guid) = self.drawing_view.shown_layout().map(str::to_owned) else {
            self.layouts.drag_row = None;
            return None;
        };
        if self.layouts.layout(&guid).is_none() {
            self.drawing_view.layout = None;
            self.drawing_view.shown = false;
            return None;
        }
        if let Some(making) = self.layouts.making.clone() {
            if self.drawing_view.made(&making).is_some() {
                self.layouts.making = None;
            } else if self.drawing.running_sheet() != Some(making.as_str()) {
                self.layouts.making = None;
                self.layouts.unmade.insert(making, self.status.clone());
            }
        }
        if self.pictures_due() {
            self.read_pictures(&guid);
        }
        self.follow_views(&guid);
        let task = self.make_next_drawing(&guid);
        let fingerprint = self.sheet_fingerprint(&guid);
        if fingerprint != self.layouts.fingerprint {
            self.layouts.fingerprint = fingerprint;
            self.layouts.changed();
        }
        task
    }

    /// Keep the names of the views on a sheet as they are now, the sizes of
    /// its drawings as their crop regions make them, and the images of its
    /// 3D views in the proportions of their pictures as read, their widths
    /// kept.
    pub(crate) fn follow_views(&mut self, sheet: &str) {
        let Some(layout) = self.layouts.layout(sheet) else {
            return;
        };
        let mut updates: Vec<(String, String, Option<[f64; 2]>)> = Vec::new();
        for viewport in &layout.viewports {
            let (name, size) = match viewport.kind {
                PlacedKind::View => match self
                    .views
                    .list
                    .iter()
                    .find(|view| view.guid == viewport.guid)
                {
                    Some(view) => (
                        view.name.clone(),
                        self.layouts
                            .images
                            .get(&viewport.guid)
                            .map(|picture| model::proportioned(viewport.size, picture.pixels)),
                    ),
                    None => continue,
                },
                PlacedKind::Drawing => match self
                    .drawing_view
                    .saved
                    .iter()
                    .find(|drawing| drawing.guid == viewport.guid)
                {
                    Some(drawing) => (
                        drawing.name.clone(),
                        drawing_rect(drawing).map(|rect| drawing_size(rect, viewport.scale)),
                    ),
                    None => continue,
                },
            };
            let size_changed = size.is_some_and(|size| {
                (0..2).any(|axis| (size[axis] - viewport.size[axis]).abs() > 1e-6)
            });
            if name != viewport.name || size_changed {
                updates.push((viewport.id.clone(), name, size));
            }
        }
        if updates.is_empty() {
            return;
        }
        let result = self.set_layout_field(sheet, |layout| {
            for (id, name, size) in updates {
                if let Some(viewport) = layout.viewport_mut(&id) {
                    viewport.name = name;
                    if let Some(size) = size {
                        viewport.size = size;
                    }
                }
            }
            Ok(())
        });
        if let Err(error) = result {
            self.status = error;
        }
    }

    /// Whether the pictures of the sheet shown are looked at again: now and
    /// then.
    fn pictures_due(&mut self) -> bool {
        let now = Instant::now();
        if self
            .layouts
            .pictures_checked
            .is_some_and(|checked| now.duration_since(checked) < PICTURE_CHECK)
        {
            return false;
        }
        self.layouts.pictures_checked = Some(now);
        true
    }

    /// Read the snapshots of the 3D views on a sheet that are new or
    /// changed since they were read.
    fn read_pictures(&mut self, sheet: &str) {
        let Some(layout) = self.layouts.layout(sheet) else {
            return;
        };
        let wanted: Vec<String> = layout
            .viewports
            .iter()
            .filter(|viewport| viewport.kind == PlacedKind::View)
            .map(|viewport| viewport.guid.clone())
            .collect();
        for guid in wanted {
            let stamp = crate::camera_views::snapshot_path(&guid)
                .and_then(|path| std::fs::metadata(path).ok())
                .and_then(|metadata| metadata.modified().ok());
            let known = self.layouts.images.get(&guid).map(|picture| picture.stamp);
            if stamp.is_none() {
                if self.layouts.images.remove(&guid).is_some() {
                    self.layouts.changed();
                }
                continue;
            }
            if known == Some(stamp) {
                continue;
            }
            let Some(png) = crate::camera_views::read_snapshot(&guid) else {
                continue;
            };
            let Some(pixels) = model::png_size(&png) else {
                continue;
            };
            let png = Arc::new(png);
            self.layouts.images.insert(
                guid,
                Picture {
                    handle: iced::widget::image::Handle::from_bytes(png.as_ref().clone()),
                    pixels,
                    png,
                    stamp,
                },
            );
            self.layouts.changed();
        }
    }

    /// Start making the first drawing on a sheet that is not made yet,
    /// while nothing else is made.
    fn make_next_drawing(&mut self, sheet: &str) -> Option<Task<Message>> {
        if self.layouts.making.is_some() || self.drawing.busy() || self.model_covered() {
            return None;
        }
        let layout = self.layouts.layout(sheet)?;
        let open = self.open_sources();
        let next = layout
            .viewports
            .iter()
            .filter(|viewport| viewport.kind == PlacedKind::Drawing)
            .filter(|viewport| self.drawing_view.made(&viewport.guid).is_none())
            .filter(|viewport| !self.layouts.unmade.contains_key(&viewport.guid))
            .find_map(|viewport| {
                self.drawing_view
                    .saved
                    .iter()
                    .find(|drawing| drawing.guid == viewport.guid)
                    .filter(|drawing| drawing.sources.iter().all(|source| open.contains(source)))
                    .cloned()
            })?;
        let guid = next.guid.clone();
        match self.remake_sheet(next.clone(), None) {
            Ok(task) => {
                self.drawing_view.remake = Some(Remake {
                    guid: guid.clone(),
                    definition: next,
                    camera: None,
                    operation: MADE_FOR_SHEET,
                    layers: Vec::new(),
                });
                self.layouts.making = Some(guid);
                self.layouts.changed();
                Some(task)
            }
            Err(why) => {
                self.layouts.unmade.insert(guid, why);
                self.layouts.changed();
                None
            }
        }
    }

    /// What the sheet shown shows, to see whether it changed.
    fn sheet_fingerprint(&self, sheet: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let Some(layout) = self.layouts.layout(sheet) else {
            return 0;
        };
        for viewport in &layout.viewports {
            viewport.id.hash(&mut hasher);
            match self.viewport_content(viewport) {
                Content::Drawing {
                    scene, rect, notes, ..
                } => {
                    (scene as *const _ as usize).hash(&mut hasher);
                    notes.len().hash(&mut hasher);
                    for value in rect.iter().flatten() {
                        value.to_bits().hash(&mut hasher);
                    }
                }
                Content::Image { pixels, key } => {
                    pixels.hash(&mut hasher);
                    key.hash(&mut hasher);
                    self.layouts
                        .images
                        .get(&key)
                        .map(|picture| picture.stamp)
                        .hash(&mut hasher);
                }
                Content::Waiting(why) => why.hash(&mut hasher),
                Content::Missing => 1u8.hash(&mut hasher),
            }
        }
        crate::i18n::choice().key().hash(&mut hasher);
        hasher.finish()
    }

    /// Start writing the sheet as a PDF, on a worker thread. The drawings
    /// on it must be made.
    pub(crate) fn start_pdf(
        &mut self,
        sheet: &str,
        path: PathBuf,
        job: Option<String>,
    ) -> Result<Task<Message>, String> {
        // The sheet as it shows now, also when it is not the one shown: the
        // pictures as they are, and the names and sizes as its views have
        // them.
        self.read_pictures(sheet);
        self.follow_views(sheet);
        let layout = self
            .layouts
            .layout(sheet)
            .cloned()
            .ok_or_else(|| "That sheet is no longer kept".to_owned())?;
        let waiting: Vec<String> = layout
            .viewports
            .iter()
            .filter(|viewport| viewport.kind == PlacedKind::Drawing)
            .filter(|viewport| {
                self.drawing_view
                    .saved
                    .iter()
                    .any(|drawing| drawing.guid == viewport.guid)
                    && self.drawing_view.made(&viewport.guid).is_none()
            })
            .map(|viewport| viewport.shown_title().to_owned())
            .collect();
        if !waiting.is_empty() {
            return Err(format!(
                "Show the sheet until its drawings are made before writing it: {}",
                waiting.join(", ")
            ));
        }
        // The pictures as the sheet shows them.
        let pictures: HashMap<String, Arc<Vec<u8>>> = layout
            .viewports
            .iter()
            .filter(|viewport| viewport.kind == PlacedKind::View)
            .filter_map(|viewport| {
                let picture = self.layouts.images.get(&viewport.guid)?;
                Some((viewport.guid.clone(), Arc::clone(&picture.png)))
            })
            .collect();
        let plot = plot::plot(&layout, |viewport| self.viewport_content(viewport));
        let title = layout.caption();
        self.layouts.export_pending = true;
        self.status = format!("Writing the sheet {title} as PDF…");
        Ok(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    pdf::write_pdf(&path, &plot, &pictures, &title).map(|bytes| (path, bytes))
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            move |result| Message::Layouts(LayoutAction::PdfWritten(job.clone(), result)),
        ))
    }

    /// The caption of the sheet shown, for the strip of tabs.
    pub(crate) fn layout_caption(&self) -> Option<String> {
        let guid = self.drawing_view.shown_layout()?;
        self.layouts.layout(guid).map(Layout::caption)
    }

    /// Escape on a sheet: a dragged row is let go, else the selected
    /// viewport. Reports whether there was either.
    pub(crate) fn layout_escape(&mut self) -> bool {
        if self.drawing_view.shown_layout().is_none() {
            return false;
        }
        if self.layouts.drag_row.take().is_some() {
            return true;
        }
        if self.layouts.selected.is_some() {
            self.select_viewport(None);
            return true;
        }
        false
    }
}

/// A locked viewport is not moved, resized, scaled or removed.
fn refuse_locked(viewport: &Viewport) -> Result<(), String> {
    if viewport.locked {
        return Err(format!("{} is locked", viewport.shown_title()));
    }
    Ok(())
}

fn rename_input_id() -> text_input::Id {
    text_input::Id::new("ops-sheet-rename")
}

/// A name for a file from the caption of a sheet: what a file name cannot
/// hold becomes a dash.
pub(crate) fn file_stem(caption: &str) -> String {
    let stem: String = caption
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || " -_.+()".contains(character) {
                character
            } else {
                '-'
            }
        })
        .collect();
    let stem = stem.trim().trim_matches('.');
    if stem.is_empty() {
        "sheet".to_owned()
    } else {
        stem.to_owned()
    }
}
