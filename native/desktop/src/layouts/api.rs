//! The commands of the local API for sheets. A sheet is named by its
//! identifier, its number or its name, in any case; without one the sheet
//! shown. A viewport is named by its place on the sheet or its identifier.

use std::path::PathBuf;

use iced::Task;
use serde::Deserialize;
use serde_json::{json, Value};

use super::model::{dots_per_inch, scale_label, Layout, Paper, PlacedKind, Viewport};
use super::plot::Content;
use super::{Orientation, MADE_FOR_SHEET};
use crate::{Message, Studio};

/// The fields of a sheet a command sets; those left out stay.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SheetOptions {
    #[serde(default)]
    pub sheet: Option<String>,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `a4` to `a0`.
    #[serde(default)]
    pub paper: Option<String>,
    /// `landscape` or `portrait`.
    #[serde(default)]
    pub orientation: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub drawn_by: Option<String>,
}

/// A viewport by its place on the sheet or its identifier.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ViewportRef {
    Index(usize),
    Id(String),
}

/// What `place_view` places and where.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PlaceOptions {
    #[serde(default)]
    pub sheet: Option<String>,
    /// The name of a saved view of the active scan or of a drawing of
    /// VIEWS, in any case.
    pub name: String,
    /// `view` or `drawing`; without it a saved view of that name, else a
    /// drawing.
    #[serde(default)]
    pub kind: Option<String>,
    /// The middle of the viewport on the paper, in millimetres from its
    /// lower left corner.
    #[serde(default)]
    pub at: Option<[f64; 2]>,
    /// The scale of a drawing as the number after "1:", or as "1:100".
    #[serde(default)]
    pub scale: Option<Value>,
}

/// What `update_viewport` changes; what is left out stays.
#[derive(Clone, Debug, Deserialize)]
pub struct ViewportOptions {
    #[serde(default)]
    pub sheet: Option<String>,
    pub viewport: ViewportRef,
    #[serde(default)]
    pub at: Option<[f64; 2]>,
    /// Width and height of the image of a 3D view; either may be null to
    /// keep the proportions.
    #[serde(default)]
    pub size: Option<[Option<f64>; 2]>,
    #[serde(default)]
    pub scale: Option<Value>,
    /// The title under it; empty for the name of the view.
    #[serde(default)]
    pub title: Option<String>,
}

fn refused(error: impl Into<String>) -> (Value, Task<Message>) {
    (json!({"ok": false, "error": error.into()}), Task::none())
}

/// A scale given as a number or as "1:100".
fn scale_of(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number
            .as_f64()
            .filter(|scale| super::model::scale_valid(*scale)),
        Value::String(text) => super::model::parse_scale(text),
        _ => None,
    }
}

impl Studio {
    /// The sheet a command names, else the sheet shown.
    fn sheet_asked(&self, sheet: Option<&str>) -> Result<String, String> {
        match sheet.map(str::trim).filter(|sheet| !sheet.is_empty()) {
            Some(name) => self
                .layouts
                .named(name)
                .map(|layout| layout.guid.clone())
                .ok_or_else(|| format!("no sheet {name}; list_sheets lists the sheets")),
            None => self
                .drawing_view
                .shown_layout()
                .map(str::to_owned)
                .ok_or_else(|| "no sheet is shown; name one with sheet".to_owned()),
        }
    }

    fn viewport_asked(&self, sheet: &str, viewport: &ViewportRef) -> Result<String, String> {
        let layout = self
            .layouts
            .layout(sheet)
            .ok_or_else(|| "That sheet is no longer kept".to_owned())?;
        match viewport {
            ViewportRef::Index(index) => layout
                .viewports
                .get(*index)
                .map(|viewport| viewport.id.clone())
                .ok_or_else(|| {
                    format!(
                        "there is no viewport {index}; the sheet holds {}",
                        layout.viewports.len()
                    )
                }),
            ViewportRef::Id(id) => layout
                .viewport(id)
                .map(|viewport| viewport.id.clone())
                .ok_or_else(|| format!("no viewport {id} on the sheet")),
        }
    }

    fn viewport_value(&self, index: usize, viewport: &Viewport) -> Value {
        let mut value = json!({
            "index": index,
            "id": viewport.id,
            "kind": viewport.kind.key(),
            "guid": viewport.guid,
            "name": viewport.name,
            "title": viewport.shown_title(),
            "centre": viewport.centre,
            "size": viewport.size,
        });
        if viewport.kind == PlacedKind::Drawing {
            value["scale"] = json!(viewport.scale);
            value["scale_label"] = json!(scale_label(viewport.scale));
        }
        match self.viewport_content(viewport) {
            Content::Drawing { .. } => value["shows"] = json!("drawing"),
            Content::Image { pixels, .. } => {
                value["shows"] = json!("image");
                value["pixels"] = json!(pixels);
                value["dpi"] = json!(dots_per_inch(pixels, viewport.size).round());
            }
            Content::Waiting(why) => {
                value["shows"] = json!("waiting");
                value["waiting"] = json!(why);
            }
            Content::Missing => value["shows"] = json!("missing"),
        }
        value
    }

    pub(crate) fn sheet_value(&self, layout: &Layout) -> Value {
        json!({
            "guid": layout.guid,
            "number": layout.number,
            "name": layout.name,
            "paper": layout.paper.key(),
            "orientation": Orientation::of(layout.landscape).key(),
            "size_mm": layout.size(),
            "project": layout.project,
            "date": layout.date,
            "drawn_by": layout.drawn_by,
            "scale": layout.scale_text(),
            "shown": self.drawing_view.shown_layout() == Some(layout.guid.as_str()),
            "viewports": layout
                .viewports
                .iter()
                .enumerate()
                .map(|(index, viewport)| self.viewport_value(index, viewport))
                .collect::<Vec<_>>(),
        })
    }

    /// What `status.result.sheets` reports.
    pub(crate) fn layouts_value(&self) -> Value {
        json!({
            "count": self.layouts.list.len(),
            "shown": self.drawing_view.shown_layout(),
            "selected": self.layouts.selected,
            "making": self.layouts.making,
            "export_pending": self.layouts.export_pending,
        })
    }

    pub(crate) fn api_list_sheets(&mut self) -> (Value, Task<Message>) {
        let task = self.settle_layouts().unwrap_or_else(Task::none);
        let sheets: Vec<Value> = self
            .layouts
            .list
            .iter()
            .map(|layout| self.sheet_value(layout))
            .collect();
        (
            json!({"ok": true, "sheets": sheets, "shown": self.drawing_view.shown_layout()}),
            task,
        )
    }

    fn answer_sheet(&self, guid: &str, task: Task<Message>) -> (Value, Task<Message>) {
        match self.layouts.layout(guid) {
            Some(layout) => (json!({"ok": true, "sheet": self.sheet_value(layout)}), task),
            None => refused("That sheet is no longer kept"),
        }
    }

    pub(crate) fn api_create_sheet(&mut self, options: &SheetOptions) -> (Value, Task<Message>) {
        let form = self.layouts.new_form();
        let paper = match options.paper.as_deref().map(Paper::from_key) {
            Some(None) => return refused("paper must be a4, a3, a2, a1 or a0"),
            Some(Some(paper)) => paper,
            None => form.paper,
        };
        let orientation = match options.orientation.as_deref().map(Orientation::from_key) {
            Some(None) => return refused("orientation must be landscape or portrait"),
            Some(Some(orientation)) => orientation,
            None => form.orientation,
        };
        let number = options.number.clone().unwrap_or(form.number);
        let name = options.name.clone().unwrap_or(form.name);
        let guid = match self.create_layout(&number, &name, paper, orientation.landscape()) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        let fields = SheetOptions {
            sheet: Some(guid.clone()),
            number: None,
            name: None,
            paper: None,
            orientation: None,
            ..options.clone()
        };
        if let Err(error) = self.apply_sheet_options(&guid, &fields) {
            return refused(error);
        }
        let task = self.show_layout(&guid).unwrap_or_else(|_| Task::none());
        let settled = self.settle_tabs().unwrap_or_else(Task::none);
        self.answer_sheet(&guid, Task::batch([task, settled]))
    }

    fn apply_sheet_options(&mut self, guid: &str, options: &SheetOptions) -> Result<(), String> {
        let paper = match options.paper.as_deref().map(Paper::from_key) {
            Some(None) => return Err("paper must be a4, a3, a2, a1 or a0".into()),
            other => other.flatten(),
        };
        let orientation = match options.orientation.as_deref().map(Orientation::from_key) {
            Some(None) => return Err("orientation must be landscape or portrait".into()),
            other => other.flatten(),
        };
        let number = options
            .number
            .as_deref()
            .map(super::checked_number)
            .transpose()?;
        let name = options
            .name
            .as_deref()
            .map(super::checked_name)
            .transpose()?;
        let reshaped = paper.is_some() || orientation.is_some();
        self.set_layout_field(guid, |layout| {
            if let Some(number) = number {
                layout.number = number;
            }
            if let Some(name) = name {
                layout.name = name;
            }
            if let Some(paper) = paper {
                layout.paper = paper;
            }
            if let Some(orientation) = orientation {
                layout.landscape = orientation.landscape();
            }
            let field = |value: &Option<String>, most: usize| {
                value
                    .as_ref()
                    .map(|value| value.chars().take(most).collect::<String>())
            };
            if let Some(project) = field(&options.project, super::MAX_NAME_CHARS) {
                layout.project = project;
            }
            if let Some(date) = field(&options.date, super::MAX_NUMBER_CHARS) {
                layout.date = date;
            }
            if let Some(drawn_by) = field(&options.drawn_by, super::MAX_NAME_CHARS) {
                layout.drawn_by = drawn_by;
            }
            Ok(())
        })?;
        if reshaped && self.drawing_view.shown_layout() == Some(guid) {
            self.layouts.fit_pending.set(true);
        }
        Ok(())
    }

    pub(crate) fn api_update_sheet(&mut self, options: &SheetOptions) -> (Value, Task<Message>) {
        let guid = match self.sheet_asked(options.sheet.as_deref()) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        match self.apply_sheet_options(&guid, options) {
            Ok(()) => self.answer_sheet(&guid, Task::none()),
            Err(error) => refused(error),
        }
    }

    pub(crate) fn api_duplicate_sheet(&mut self, sheet: Option<&str>) -> (Value, Task<Message>) {
        let guid = match self.sheet_asked(sheet) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        match self.duplicate_layout(&guid) {
            Ok(copy) => {
                let task = self.show_layout(&copy).unwrap_or_else(|_| Task::none());
                let settled = self.settle_tabs().unwrap_or_else(Task::none);
                self.answer_sheet(&copy, Task::batch([task, settled]))
            }
            Err(error) => refused(error),
        }
    }

    pub(crate) fn api_delete_sheet(&mut self, sheet: Option<&str>) -> (Value, Task<Message>) {
        let guid = match self.sheet_asked(sheet) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        match self.delete_layout(&guid) {
            Ok(caption) => {
                let settled = self.settle_tabs().unwrap_or_else(Task::none);
                (json!({"ok": true, "deleted": caption}), settled)
            }
            Err(error) => refused(error),
        }
    }

    pub(crate) fn api_show_sheet(&mut self, sheet: Option<&str>) -> (Value, Task<Message>) {
        if self.settings.is_some() {
            return refused("the Settings dialog is open");
        }
        let guid = match self.sheet_asked(sheet) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        match self.show_layout(&guid) {
            Ok(task) => {
                let settled = self.settle_tabs().unwrap_or_else(Task::none);
                let made = self.settle_layouts().unwrap_or_else(Task::none);
                self.answer_sheet(&guid, Task::batch([task, settled, made]))
            }
            Err(error) => refused(error),
        }
    }

    pub(crate) fn api_place_view(&mut self, options: &PlaceOptions) -> (Value, Task<Message>) {
        let guid = match self.sheet_asked(options.sheet.as_deref()) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        let kind = options
            .kind
            .as_deref()
            .map(|kind| kind.trim().to_ascii_lowercase());
        if kind
            .as_deref()
            .is_some_and(|kind| !matches!(kind, "view" | "drawing"))
        {
            return refused("kind must be view or drawing");
        }
        let wants = |wanted: &str| kind.as_deref().is_none_or(|kind| kind == wanted);
        let name = options.name.trim();
        let view = wants("view")
            .then(|| {
                self.listed_views()
                    .into_iter()
                    .find(|view| view.name.eq_ignore_ascii_case(name))
                    .map(|view| (PlacedKind::View, view.guid.clone()))
            })
            .flatten();
        let placed = view.or_else(|| {
            wants("drawing")
                .then(|| {
                    self.drawing_named(name)
                        .map(|guid| (PlacedKind::Drawing, guid))
                })
                .flatten()
        });
        let Some((kind, placed)) = placed else {
            return refused(format!(
                "no saved view of the active scan or drawing of VIEWS named {name}"
            ));
        };
        let scale = match options.scale.as_ref().map(scale_of) {
            Some(None) => return refused("scale must be a number from 1 to 100000, or 1:N"),
            other => other.flatten(),
        };
        match self.place_on_layout(&guid, kind, &placed, options.at, scale) {
            Ok(id) => {
                let task = self.settle_layouts().unwrap_or_else(Task::none);
                let (mut answer, task) = self.answer_sheet(&guid, task);
                answer["viewport"] = json!(id);
                (answer, task)
            }
            Err(error) => refused(error),
        }
    }

    pub(crate) fn api_update_viewport(
        &mut self,
        options: &ViewportOptions,
    ) -> (Value, Task<Message>) {
        let guid = match self.sheet_asked(options.sheet.as_deref()) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        let id = match self.viewport_asked(&guid, &options.viewport) {
            Ok(id) => id,
            Err(error) => return refused(error),
        };
        let scale = match options.scale.as_ref().map(scale_of) {
            Some(None) => return refused("scale must be a number from 1 to 100000, or 1:N"),
            other => other.flatten(),
        };
        let mut result = Ok(());
        if let Some(scale) = scale {
            result = result.and_then(|()| self.set_viewport_scale(&guid, &id, scale));
        }
        if let Some([width, height]) = options.size {
            result = result.and_then(|()| self.set_viewport_size(&guid, &id, width, height));
        }
        if let Some(at) = options.at {
            result = result.and_then(|()| self.move_viewport(&guid, &id, at));
        }
        if let Some(title) = &options.title {
            let title: String = title.chars().take(super::MAX_NAME_CHARS).collect();
            result = result.and_then(|()| {
                self.set_layout_field(&guid, |layout| {
                    if let Some(viewport) = layout.viewport_mut(&id) {
                        viewport.title = Some(title).filter(|title| !title.trim().is_empty());
                    }
                    Ok(())
                })
            });
        }
        match result {
            Ok(()) => self.answer_sheet(&guid, Task::none()),
            Err(error) => refused(error),
        }
    }

    pub(crate) fn api_remove_viewport(
        &mut self,
        sheet: Option<&str>,
        viewport: &ViewportRef,
    ) -> (Value, Task<Message>) {
        let guid = match self.sheet_asked(sheet) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        let id = match self.viewport_asked(&guid, viewport) {
            Ok(id) => id,
            Err(error) => return refused(error),
        };
        match self.remove_viewport(&guid, &id) {
            Ok(name) => {
                let (mut answer, task) = self.answer_sheet(&guid, Task::none());
                answer["removed"] = json!(name);
                (answer, task)
            }
            Err(error) => refused(error),
        }
    }

    pub(crate) fn api_export_sheet_pdf(
        &mut self,
        sheet: Option<&str>,
        path: PathBuf,
    ) -> (Value, Task<Message>) {
        let pdf = path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"));
        if !path.is_absolute() || !pdf {
            return refused("export_sheet_pdf requires an absolute .pdf path");
        }
        if self.layouts.export_pending {
            return refused("a sheet is being written; wait for it");
        }
        let guid = match self.sheet_asked(sheet) {
            Ok(guid) => guid,
            Err(error) => return refused(error),
        };
        let id = self.record_api_job(json!({"state": "running", "operation": "export_sheet_pdf"}));
        match self.start_pdf(&guid, path, Some(id.clone())) {
            Ok(task) => (json!({"ok": true, "accepted": true, "job_id": id}), task),
            Err(error) => {
                self.forget_api_job(&id);
                refused(error)
            }
        }
    }
}

/// Whether a job of the Section drawing tool was started for a sheet.
pub(crate) fn made_for_sheet(operation: &str) -> bool {
    operation == MADE_FOR_SHEET
}
