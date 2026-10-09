//! Mesh Pointcloud: the wizard behind the one button of the SURFACE group.
//! It is a card over the window in three steps: the method (a closed mesh, a
//! terrain mesh, a 3D surface or the flat faces) with what it works on, the
//! options of that method, and the run with its progress and its result.
//!
//! The settings stay with the tools that use them (`closed_mesh`, `faces`
//! and the 3D surface of `main`), so a method keeps its options when another
//! one is chosen, and the commands of the local API that set them and start
//! the jobs work as before. A job goes on when the card is closed: the
//! button in the ribbon shows that it runs, and the card opens again on the
//! Run step of that job.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use iced::widget::{
    button, center, column, container, horizontal_space, mouse_area, opaque, row, text, Space,
};
use iced::{Border, Element, Fill, Length, Task, Theme};
use pointcloud_core::{IndexedPoint, MeshTopology, SurfaceMeshConfig};
use serde_json::{json, Value};

use crate::closed_mesh::{ClosedMeshAction, Layers, MeshOf};
use crate::faces::FaceAction;
use crate::i18n::{key, tr, tr_args};
use crate::selection::ClassFilter;
use crate::ui_style;
use crate::{
    display_name, format_count, icon_svg, opencad_properties, opencad_ribbon, ui_theme, CloudEntry,
    MeshMode, Message, Studio, ToolIcon,
};

#[cfg(test)]
mod tests;

/// What the Run step says when no scan holds the result it shows.
const NOT_HELD: &str = key(
    "No scan holds this result any more: it was replaced or cleared, or its scan was closed. Run the method again to make it anew.",
);

/// The size of the card where the window has room for it.
const CARD_WIDTH: f32 = 900.0;
const CARD_HEIGHT: f32 = 660.0;
/// The width of the name of a setting, of its field and of its default.
const LABEL_W: f32 = 210.0;
const FIELD_W: f32 = 190.0;
const DEFAULT_W: f32 = 220.0;

/// A way to mesh the points, as the first step offers it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeshMethod {
    #[default]
    Closed,
    Terrain,
    Surface,
    Faces,
}

impl MeshMethod {
    pub const ALL: [Self; 4] = [Self::Closed, Self::Terrain, Self::Surface, Self::Faces];

    /// The name the local API knows the method by.
    pub fn id(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Terrain => "terrain",
            Self::Surface => "surface",
            Self::Faces => "faces",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|method| method.id() == value)
    }

    /// Every id `from_id` accepts.
    pub fn ids() -> Vec<&'static str> {
        Self::ALL.map(Self::id).to_vec()
    }

    /// The English name of the method.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Closed => key("Closed mesh"),
            Self::Terrain => key("Terrain mesh"),
            Self::Surface => key("3D surface"),
            Self::Faces => key("Flat faces"),
        }
    }

    /// What the method makes.
    fn makes(self) -> &'static str {
        match self {
            Self::Closed => key(
                "A closed solid of a room or a building: a surface without overlaps, closed wherever the scan has points, with the distance between points and mesh measured.",
            ),
            Self::Terrain => key(
                "A 2.5D ground surface: the lowest points of a grid seen from above, joined into triangles. No walls and no overhangs.",
            ),
            Self::Surface => key(
                "An open surface that follows the points, walls and overhangs included. It leaves holes and is not watertight.",
            ),
            Self::Faces => key(
                "The planar faces of walls, floors and roofs, each with its outline, area and residual, and round columns and pipes.",
            ),
        }
    }

    /// When to use it.
    fn when(self) -> &'static str {
        match self {
            Self::Closed => key(
                "Use it for a room or a part of a building of which the surface has to be right.",
            ),
            Self::Terrain => key("Use it for ground, streets and other surfaces seen from above."),
            Self::Surface => key("Use it for a quick impression of a whole scan."),
            Self::Faces => key(
                "Use it to measure and draw: areas, heights, whether a wall is plumb. It makes no mesh.",
            ),
        }
    }

    /// The recommended settings, in a few words.
    fn preset(self) -> &'static str {
        match self {
            Self::Closed => key(
                "Recommended for a room or a building: automatic voxel, holes closed up to 0.25 m, automatic simplification, every source point and automatic sides: from the stations where the scan knows them, else towards the centre.",
            ),
            Self::Terrain => key("A terrain mesh has no settings to choose."),
            Self::Surface => key(
                "Recommended: 50,000 vertices, 12 neighbors, edge factor 4 and an automatic mesh size.",
            ),
            Self::Faces => key(
                "Recommended for a building: 20 mm, 10 degrees, faces from 0.25 m², with columns and pipes.",
            ),
        }
    }

    fn icon(self) -> ToolIcon {
        match self {
            Self::Closed => ToolIcon::ClosedMesh,
            Self::Terrain => ToolIcon::MeshTerrain,
            Self::Surface => ToolIcon::MeshSurface,
            Self::Faces => ToolIcon::Faces,
        }
    }

    /// The mode of the mesh job that writes an OBJ file, for the two
    /// methods that make one.
    fn mode(self) -> Option<MeshMode> {
        match self {
            Self::Terrain => Some(MeshMode::Terrain),
            Self::Surface => Some(MeshMode::Surface),
            Self::Closed | Self::Faces => None,
        }
    }
}

/// A step of the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WizardStep {
    #[default]
    Method,
    Options,
    Run,
}

impl WizardStep {
    pub const ALL: [Self; 3] = [Self::Method, Self::Options, Self::Run];

    /// The name the local API knows the step by.
    pub fn id(self) -> &'static str {
        match self {
            Self::Method => "method",
            Self::Options => "options",
            Self::Run => "run",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.id() == value)
    }

    pub fn ids() -> Vec<&'static str> {
        Self::ALL.map(Self::id).to_vec()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Method => key("Method"),
            Self::Options => key("Options"),
            Self::Run => key("Run"),
        }
    }

    fn number(self) -> usize {
        match self {
            Self::Method => 1,
            Self::Options => 2,
            Self::Run => 3,
        }
    }

    fn previous(self) -> Option<Self> {
        match self {
            Self::Method => None,
            Self::Options => Some(Self::Method),
            Self::Run => Some(Self::Options),
        }
    }

    fn next(self) -> Option<Self> {
        match self {
            Self::Method => Some(Self::Options),
            Self::Options => Some(Self::Run),
            Self::Run => None,
        }
    }
}

/// How the last terrain mesh or 3D surface ended. The two share one job of
/// `main`, which writes an OBJ file and gives the mesh to the active scan;
/// the card keeps the end of each apart.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FileMeshLast {
    Done {
        path: PathBuf,
        source_points: u64,
        vertices: usize,
        triangles: usize,
        topology: MeshTopology,
        seconds: f64,
        /// The file name of the scan that got the mesh, or nothing when it
        /// was closed while the job ran.
        shown_on: Option<String>,
        /// The mesh it got, to find the scan that still holds it.
        mesh: MeshOf,
    },
    Cancelled,
    Failed(String),
}

/// The place of the end of a mode among those the card keeps.
fn slot(mode: MeshMode) -> usize {
    match mode {
        MeshMode::Terrain => 0,
        MeshMode::Surface => 1,
    }
}

/// What the methods work on: the active scan, the visible scans, the section
/// box and the selection, with their points.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Scope {
    /// The file name of the active scan and the points it has left.
    active: Option<(String, u64)>,
    /// How many scans are shown, without layers of 3D BAG buildings, and
    /// their points.
    visible: (usize, u64),
    /// The section box while it is on: its size in metres, and about how
    /// many points of the active scan and of the visible scans lie in it.
    section: Option<([f64; 3], u64, u64)>,
    /// The selected points of all scans.
    selected: u64,
}

/// The card: whether it is shown, its step and its method, and what the
/// methods work on.
#[derive(Debug, Default)]
pub(crate) struct MeshWizard {
    open: bool,
    step: WizardStep,
    method: MeshMethod,
    /// The method whose save dialog the card opened, while it is open.
    asked: Option<MeshMethod>,
    /// What the methods work on, with a print of what it was worked out from.
    scope: Option<(u64, Scope)>,
    /// How the last terrain mesh and the last 3D surface ended, each in the
    /// place `slot` gives its mode.
    file_last: [Option<FileMeshLast>; 2],
}

impl MeshWizard {
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// How the last terrain mesh or 3D surface ended.
    pub(crate) fn file_last(&self, mode: MeshMode) -> Option<&FileMeshLast> {
        self.file_last[slot(mode)].as_ref()
    }

    /// Take the card away; whether it was shown. A job goes on.
    pub(crate) fn close(&mut self) -> bool {
        std::mem::replace(&mut self.open, false)
    }

    /// The save dialog of a terrain mesh or a 3D surface closed without a
    /// file: the card goes back to the options it was started from.
    pub(crate) fn save_cancelled(&mut self) {
        if self.asked.take() == Some(self.method) && self.step == WizardStep::Run {
            self.step = WizardStep::Options;
        }
    }

    /// The save dialog closed with a file.
    pub(crate) fn save_chosen(&mut self) {
        self.asked = None;
    }
}

/// What the Options step says of a method before it runs.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Notes {
    /// What it would do, and on what, in the language in use.
    pub(crate) lines: Vec<String>,
    /// What may go wrong.
    pub(crate) warnings: Vec<String>,
    /// Why it cannot start now.
    pub(crate) refusal: Option<String>,
}

/// How far a job is, as the Run step shows it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Progress {
    /// What it does now, in the language in use.
    pub(crate) stage: String,
    /// The step it is at, of how many.
    pub(crate) steps: Option<(usize, usize)>,
    pub(crate) fraction: Option<f32>,
    pub(crate) seconds: u64,
    pub(crate) cancelling: bool,
}

/// What the Run step shows of a method.
pub(crate) enum RunState {
    /// Nothing ran with it in this session.
    Idle,
    /// The save dialog of its OBJ file is open.
    Choosing,
    Running(Progress),
    /// Its figures, what became of the result, and advice. `kept` is the
    /// line that says to which scan the job gave its result, when it gave
    /// it to one.
    Done {
        rows: Vec<Element<'static, Message>>,
        kept: Option<String>,
        lines: Vec<String>,
        warnings: Vec<String>,
    },
    Cancelled(String),
    Failed(String),
}

impl RunState {
    /// The name of the state in the local API.
    fn key(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Choosing => "choosing",
            Self::Running(_) => "running",
            Self::Done { .. } => "done",
            Self::Cancelled(_) => "cancelled",
            Self::Failed(_) => "failed",
        }
    }
}

/// Everything the card reacts to.
#[derive(Debug, Clone)]
pub enum MeshWizardAction {
    /// Show the card: on the Run step of a job that runs, else where it was.
    Open,
    /// Take the card away; a job goes on.
    Close,
    Step(WizardStep),
    Method(MeshMethod),
    Back,
    Next,
    /// Put the recommended settings of the method in its options.
    Recommended,
    /// Start the method.
    Run,
    Cancel,
    /// Close the card and show the result in the scene.
    ShowInModel,
    /// Save the result of the method, as Export mesh… or Export faces… do.
    Export,
}

/// A setting of the Options step: its name with its unit, the field with
/// the default as placeholder, the default beside it, and the explanation
/// as the tooltip of the row. `label` is translated here, `tip` is marked
/// with `key` where it is written.
pub(crate) fn option_input<'a>(
    label: &'static str,
    value: &'a str,
    placeholder: &'static str,
    default: String,
    on_input: impl Fn(String) -> Message + 'a,
    tip: &'static str,
) -> Element<'a, Message> {
    let field = ui_style::text_input(placeholder, value)
        .on_input(on_input)
        .width(FIELD_W);
    option_row(tr(label), field.into(), default, tip)
}

/// A setting with a control of its own, such as a choice list.
pub(crate) fn option_control<'a>(
    label: &'static str,
    control: Element<'a, Message>,
    default: String,
    tip: &'static str,
) -> Element<'a, Message> {
    option_row(
        tr(label),
        container(control).width(FIELD_W).into(),
        default,
        tip,
    )
}

fn option_row<'a>(
    label: &str,
    control: Element<'a, Message>,
    default: String,
    tip: &'static str,
) -> Element<'a, Message> {
    let line = row![
        text(label.to_owned())
            .size(12)
            .width(LABEL_W)
            .style(|theme: &Theme| text::Style {
                color: Some(ui_theme::colors(theme).dialog_content_secondary),
            }),
        control,
        text(tr_args("Default: {value}", &[("value", &default)]))
            .size(11)
            .width(DEFAULT_W)
            .style(|theme: &Theme| text::Style {
                color: Some(ui_theme::colors(theme).text_muted),
            }),
        // The mark says that the row explains itself under the pointer.
        container(text("?").size(10))
            .width(16)
            .height(16)
            .center(16)
            .style(|theme: &Theme| {
                let colors = ui_theme::colors(theme);
                container::Style::default()
                    .color(colors.text_muted)
                    .border(Border {
                        color: colors.border,
                        width: 1.0,
                        radius: 8.0.into(),
                    })
            }),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center)
    .padding([2, 4]);
    opencad_properties::explained(line, vec![tr(tip).to_owned()])
}

/// About how many points of a scan lie inside the box of `filter` and are
/// shown: the share of its overview sample, applied to all of its points.
fn points_inside(entry: &CloudEntry, filter: &ClassFilter) -> u64 {
    let sample = entry
        .cloud
        .points
        .len()
        .min(entry.cloud.point_ordinals.len());
    if sample == 0 {
        return 0;
    }
    let kept = entry
        .cloud
        .points
        .iter()
        .zip(&entry.cloud.point_ordinals)
        .filter(|(point, ordinal)| {
            entry.record_visible(IndexedPoint {
                point: **point,
                ordinal: **ordinal,
            }) && filter.accepts(&entry.transform.point(**point))
        })
        .count();
    (kept as f64 / sample as f64 * entry.cloud.total_points as f64).round() as u64
}

/// A row of what a method works on: what it is, its value and a button.
fn scope_row<'a>(
    name: String,
    value: String,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut line = row![
        text(name)
            .size(12)
            .width(LABEL_W)
            .style(|theme: &Theme| text::Style {
                color: Some(ui_theme::colors(theme).dialog_content_secondary),
            }),
        text(value).size(12).width(Fill),
    ]
    .spacing(12)
    .align_y(iced::Alignment::Center);
    if let Some(action) = action {
        line = line.push(action);
    }
    line.into()
}

/// A plain button of the card, outlined so that it reads as a button among
/// the text of a page.
fn plain_button<'a>(label: &str, message: Option<Message>) -> Element<'a, Message> {
    ui_style::secondary_button(label.to_owned())
        .on_press_maybe(message)
        .into()
}

/// The button that goes on.
fn primary_button<'a>(label: &str, message: Option<Message>) -> Element<'a, Message> {
    ui_style::primary_button(label.to_owned())
        .on_press_maybe(message)
        .into()
}

/// A line of text in the muted colour, or in the accent colour for a
/// warning.
fn note<'a>(content: String, warning: bool) -> Element<'a, Message> {
    text(content)
        .size(11)
        .style(move |theme: &Theme| {
            let colors = ui_theme::colors(theme);
            text::Style {
                color: Some(if warning {
                    colors.accent
                } else {
                    colors.text_muted
                }),
            }
        })
        .into()
}

fn rule<'a>() -> Element<'a, Message> {
    container(Space::new(Fill, 1))
        .style(|theme| {
            container::Style::default().background(ui_theme::colors(theme).dialog_section_border)
        })
        .into()
}

impl Studio {
    /// Whether the job of a method runs, or its save dialog is open.
    fn method_runs(&self, method: MeshMethod) -> bool {
        match method {
            MeshMethod::Closed => self.closed_mesh.is_running(),
            MeshMethod::Faces => self.faces.is_running(),
            MeshMethod::Terrain | MeshMethod::Surface => {
                self.mesh_job
                    .as_ref()
                    .is_some_and(|job| Some(job.mode) == method.mode())
                    || (self.mesh_dialog_pending && self.mesh_wizard.asked == Some(method))
            }
        }
    }

    /// The method whose job runs: the one the card shows when it runs, else
    /// the first one that does.
    pub(crate) fn mesh_running(&self) -> Option<MeshMethod> {
        let shown = self.mesh_wizard.method;
        if self.method_runs(shown) {
            return Some(shown);
        }
        MeshMethod::ALL
            .into_iter()
            .find(|method| self.method_runs(*method))
    }

    pub(crate) fn update_mesh_wizard(&mut self, action: MeshWizardAction) -> Task<Message> {
        match action {
            MeshWizardAction::Open => self.open_mesh_wizard(),
            MeshWizardAction::Close => {
                self.mesh_wizard.close();
            }
            MeshWizardAction::Step(step) => self.mesh_wizard.step = step,
            MeshWizardAction::Method(method) => self.mesh_wizard.method = method,
            MeshWizardAction::Back => {
                if let Some(previous) = self.mesh_wizard.step.previous() {
                    self.mesh_wizard.step = previous;
                }
            }
            MeshWizardAction::Next => {
                if let Some(next) = self.mesh_wizard.step.next() {
                    self.mesh_wizard.step = next;
                }
            }
            MeshWizardAction::Recommended => match self.mesh_wizard.method {
                MeshMethod::Closed => self.recommend_closed_mesh(),
                MeshMethod::Faces => self.recommend_faces(),
                MeshMethod::Surface => self.set_surface_mesh_config(SurfaceMeshConfig::default()),
                MeshMethod::Terrain => {}
            },
            MeshWizardAction::Run => return self.run_mesh_method(),
            MeshWizardAction::Cancel => self.cancel_mesh_method(self.mesh_wizard.method),
            MeshWizardAction::ShowInModel => return self.show_mesh_result(),
            MeshWizardAction::Export => {
                // The result the Run step shows, whichever scan is active.
                let method = self.mesh_wizard.method;
                let Some(holder) = self.mesh_result_holder(method) else {
                    self.status = NOT_HELD.into();
                    return Task::none();
                };
                return match method {
                    MeshMethod::Faces => self.export_faces_of(Some(holder)),
                    _ => self.export_mesh_of(Some(holder)),
                };
            }
        }
        Task::none()
    }

    /// Show the card. A job that runs is shown on the Run step; otherwise
    /// the card shows the step and the method it showed last. The File view
    /// steps aside, and the card of Pointcloud to Drawing becomes its strip.
    fn open_mesh_wizard(&mut self) {
        self.file_open = false;
        if self.mesh_to_plans.minimize() {
            self.show_model();
        }
        if let Some(method) = self.mesh_running() {
            self.mesh_wizard.method = method;
            self.mesh_wizard.step = WizardStep::Run;
        }
        self.mesh_wizard.open = true;
        self.mesh_wizard.scope = None;
        self.settle_mesh_wizard();
    }

    /// Why a method cannot start now, in the language in use.
    pub(crate) fn mesh_refusal(&self, method: MeshMethod) -> Option<String> {
        self.method_notes(method).refusal
    }

    /// What the Options step says of a method.
    fn method_notes(&self, method: MeshMethod) -> Notes {
        match method {
            MeshMethod::Closed => self.closed_mesh_notes(),
            MeshMethod::Faces => self.faces_notes(),
            MeshMethod::Terrain | MeshMethod::Surface => self.file_mesh_notes(method),
        }
    }

    /// What the Options step says of a terrain mesh or a 3D surface.
    fn file_mesh_notes(&self, method: MeshMethod) -> Notes {
        let mut notes = Notes {
            lines: vec![
                tr("Run asks where to save the OBJ file. The mesh then becomes the mesh of the active scan and takes the place of a mesh it has.")
                    .to_owned(),
                tr("The job reads every point of the scan once; only the points inside the section box and of the classes shown take part.")
                    .to_owned(),
            ],
            ..Notes::default()
        };
        let active = self.active.and_then(|index| self.clouds.get(index));
        notes.refusal =
            if self.mesh_job.is_some() || self.mesh_dialog_pending || self.closed_mesh.is_running()
            {
                Some(tr("A mesh task is already open or running").to_owned())
            } else if let Some(entry) = active {
                match crate::still_loading(&entry.cloud) {
                    Some(name) => Some(tr_args(
                        "{name} is still loading; wait for it before meshing",
                        &[("name", &name)],
                    )),
                    None if method == MeshMethod::Surface => self.surface_problem(),
                    None => None,
                }
            } else {
                Some(tr("Select a scan first: the mesh becomes its mesh").to_owned())
            };
        notes
    }

    /// Why the settings of the 3D surface give no mesh, in the language in
    /// use; nothing when they are right.
    fn surface_problem(&self) -> Option<String> {
        let [vertices, neighbors, factor, size] = &self.surface_settings;
        let whole = |value: &str| value.trim().parse::<usize>().ok();
        let number = |value: &str| crate::closed_mesh::number(value);
        if !whole(vertices).is_some_and(|value| (3..=1_000_000).contains(&value)) {
            return Some(tr("Max vertices must be a whole number from 3 to 1,000,000").to_owned());
        }
        if !whole(neighbors).is_some_and(|value| (3..=32).contains(&value)) {
            return Some(tr("Neighbors must be a whole number from 3 to 32").to_owned());
        }
        if !number(factor).is_some_and(|value| value > 0.0) {
            return Some(tr("The edge factor must be a number above 0").to_owned());
        }
        if !number(size).is_some_and(|value| value >= 0.0) {
            return Some(tr("The mesh size must be a number of 0 or more").to_owned());
        }
        // What the core checks further, in its own words.
        self.surface_mesh_config().err()
    }

    /// Start the method the card shows, and show its Run step.
    fn run_mesh_method(&mut self) -> Task<Message> {
        let method = self.mesh_wizard.method;
        if let Some(reason) = self.mesh_refusal(method) {
            self.status = reason;
            return Task::none();
        }
        let task = match method {
            MeshMethod::Closed => self.update_closed_mesh(ClosedMeshAction::Start),
            MeshMethod::Faces => self.update_faces(FaceAction::Start),
            MeshMethod::Terrain | MeshMethod::Surface => {
                let Some(mode) = method.mode() else {
                    return Task::none();
                };
                self.mesh_wizard.asked = Some(method);
                let task = self.handle(Message::MeshRequest(mode));
                if !self.mesh_dialog_pending {
                    self.mesh_wizard.asked = None;
                }
                task
            }
        };
        if self.method_runs(method) {
            self.mesh_wizard.step = WizardStep::Run;
        }
        task
    }

    /// Ask the job of a method to stop.
    fn cancel_mesh_method(&mut self, method: MeshMethod) {
        match method {
            MeshMethod::Closed => self.cancel_closed_mesh(),
            MeshMethod::Faces => self.cancel_faces(),
            MeshMethod::Terrain | MeshMethod::Surface => {
                if self
                    .mesh_job
                    .as_ref()
                    .is_some_and(|job| Some(job.mode) == method.mode())
                {
                    let _ = self.handle(Message::CancelMesh);
                }
            }
        }
    }

    /// Close the card and show the result the Run step shows: the model
    /// instead of a drawing, and the mesh or the faces switched on of the
    /// scan that holds them, whichever scan is active.
    fn show_mesh_result(&mut self) -> Task<Message> {
        self.mesh_wizard.close();
        self.show_model();
        let method = self.mesh_wizard.method;
        let Some(index) = self.mesh_result_holder(method) else {
            return Task::none();
        };
        match method {
            MeshMethod::Faces => return self.update_faces(FaceAction::Visible(index, true)),
            _ => self.clouds[index].mesh_visible = true,
        }
        Task::none()
    }

    /// The scan that holds the result the Run step of a method shows, by
    /// its place: the mesh or the faces its last job made. Nothing when no
    /// scan holds them any more, because they were replaced or cleared or
    /// their scan was closed.
    pub(crate) fn mesh_result_holder(&self, method: MeshMethod) -> Option<usize> {
        match method {
            MeshMethod::Closed => self.closed_mesh_holder(),
            MeshMethod::Faces => self.faces_holder(),
            MeshMethod::Terrain | MeshMethod::Surface => {
                match self.mesh_wizard.file_last(method.mode()?)? {
                    FileMeshLast::Done { mesh, .. } => mesh.holder(&self.clouds),
                    FileMeshLast::Cancelled | FileMeshLast::Failed(_) => None,
                }
            }
        }
    }

    /// Whether the result of a method can be saved now: a scan holds it and
    /// no save is under way.
    fn mesh_result_exportable(&self, method: MeshMethod) -> bool {
        let pending = match method {
            MeshMethod::Faces => self.faces.is_exporting(),
            _ => self.mesh_export_pending,
        };
        !pending && self.mesh_result_holder(method).is_some()
    }

    /// The name of the button of the Run step that starts the method: Run
    /// while it has no result, Run again once it has one.
    fn run_label(&self, method: MeshMethod) -> &'static str {
        if matches!(self.method_run(method), RunState::Done { .. }) {
            key("Run again")
        } else {
            key("Run")
        }
    }

    /// A job of a method under way, or how its last one ended.
    fn method_run(&self, method: MeshMethod) -> RunState {
        match method {
            MeshMethod::Closed => self.closed_mesh_run(),
            MeshMethod::Faces => self.faces_run(),
            MeshMethod::Terrain | MeshMethod::Surface => self.file_mesh_run(method),
        }
    }

    /// The terrain mesh or the 3D surface under way, or how the last one
    /// ended.
    fn file_mesh_run(&self, method: MeshMethod) -> RunState {
        let Some(mode) = method.mode() else {
            return RunState::Idle;
        };
        if let Some(job) = self.mesh_job.as_ref().filter(|job| job.mode == mode) {
            let progress = job.control.snapshot();
            let cancelling = job.control.cancelled.load(Ordering::Relaxed);
            let (stage, place) = match progress.stage {
                pointcloud_core::MeshStage::Reading => (tr("Reading the points…"), 1),
                pointcloud_core::MeshStage::Reconstructing => (tr("Making the triangles…"), 2),
                pointcloud_core::MeshStage::Writing => (tr("Writing the OBJ file…"), 3),
            };
            return RunState::Running(Progress {
                stage: if cancelling {
                    tr("Cancelling…").to_owned()
                } else {
                    stage.to_owned()
                },
                steps: Some((place, 3)),
                fraction: (progress.total > 0)
                    .then(|| (progress.completed as f64 / progress.total as f64).min(1.0) as f32),
                seconds: job.started.elapsed().as_secs(),
                cancelling,
            });
        }
        if self.mesh_dialog_pending && self.mesh_wizard.asked == Some(method) {
            return RunState::Choosing;
        }
        match self.mesh_wizard.file_last(mode) {
            None => RunState::Idle,
            Some(FileMeshLast::Done {
                path,
                source_points,
                vertices,
                triangles,
                topology,
                seconds,
                shown_on,
                ..
            }) => RunState::Done {
                rows: vec![
                    opencad_properties::property_row(
                        "Last mesh",
                        tr_args("{seconds} s", &[("seconds", &format!("{seconds:.1}"))]),
                    ),
                    opencad_properties::property_row(
                        "Source points",
                        format_count(*source_points),
                    ),
                    opencad_properties::property_row("Vertices", format_count(*vertices)),
                    opencad_properties::property_row("Triangles", format_count(*triangles)),
                    opencad_properties::property_row(
                        "Open edges",
                        format_count(topology.open_edges),
                    ),
                    opencad_properties::property_row(
                        "Connected parts",
                        format_count(topology.components),
                    ),
                ],
                kept: shown_on.as_ref().map(|name| {
                    tr_args(
                        "Shown as the mesh of {name}; it takes the place of the mesh that scan had.",
                        &[("name", name)],
                    )
                }),
                lines: [
                    shown_on.is_none().then(|| {
                        tr("Its scan was closed while the job ran, so nothing is shown.")
                            .to_owned()
                    }),
                    Some(tr_args("Written to {path}.", &[("path", &path.display())])),
                ]
                .into_iter()
                .flatten()
                .collect(),
                warnings: Vec::new(),
            },
            Some(FileMeshLast::Cancelled) => RunState::Cancelled(
                tr("The last mesh was cancelled; an existing file was left as it was.")
                    .to_owned(),
            ),
            Some(FileMeshLast::Failed(error)) => RunState::Failed(error.clone()),
        }
    }

    /// Keep how a terrain mesh or a 3D surface ended, for the Run step.
    pub(crate) fn keep_file_mesh_end(
        &mut self,
        mode: MeshMode,
        seconds: f64,
        result: &Result<
            (
                std::sync::Arc<pointcloud_core::PointCloud>,
                PathBuf,
                pointcloud_core::MeshStats,
                crate::mesh_export::MeasuredMesh,
            ),
            String,
        >,
    ) {
        self.mesh_wizard.file_last[slot(mode)] = Some(match result {
            Ok((source, path, stats, measured)) => FileMeshLast::Done {
                path: path.clone(),
                source_points: stats.source_points,
                vertices: stats.vertices,
                triangles: stats.triangles,
                topology: measured.topology,
                seconds,
                shown_on: self
                    .clouds
                    .iter()
                    .find(|entry| entry.matches_source(source))
                    .map(|entry| display_name(&entry.cloud.path).to_owned()),
                mesh: MeshOf::of(&measured.mesh),
            },
            Err(error) if error == "Operation cancelled" => FileMeshLast::Cancelled,
            Err(error) => FileMeshLast::Failed(error.clone()),
        });
    }

    /// A print of what the methods work on: the active scan, the scans, how
    /// they stand and what of them is deleted, selected and shown, and the
    /// section box.
    fn scope_print(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.active.hash(&mut hasher);
        format!("{:?}", self.mesh_filter()).hash(&mut hasher);
        for entry in &self.clouds {
            (
                std::sync::Arc::as_ptr(&entry.cloud) as usize,
                entry.visible,
                entry.bag_source,
                entry
                    .deleted
                    .as_ref()
                    .map(|mask| (std::sync::Arc::as_ptr(mask) as usize, mask.count)),
                entry.selection.as_ref().map(|mask| mask.count),
                entry.transform.scale.map(f64::to_bits),
                entry.transform.offset.map(f64::to_bits),
            )
                .hash(&mut hasher);
        }
        hasher.finish()
    }

    /// What the methods work on, as it is now.
    fn mesh_scope(&self) -> Scope {
        let filter = self.mesh_filter();
        let active = self.active.and_then(|index| self.clouds.get(index));
        let shown: Vec<&CloudEntry> = self
            .clouds
            .iter()
            .filter(|entry| entry.visible && !entry.bag_source && entry.cloud.total_points > 0)
            .collect();
        Scope {
            active: active.map(|entry| {
                (
                    display_name(&entry.cloud.path).to_owned(),
                    entry.remaining_count(),
                )
            }),
            visible: (
                shown.len(),
                shown.iter().map(|entry| entry.remaining_count()).sum(),
            ),
            section: self.section_box().map(|section| {
                let size =
                    std::array::from_fn(|axis| section.bounds.max[axis] - section.bounds.min[axis]);
                (
                    size,
                    active.map_or(0, |entry| points_inside(entry, &filter)),
                    shown
                        .iter()
                        .map(|entry| points_inside(entry, &filter))
                        .sum(),
                )
            }),
            selected: self.selected_total(),
        }
    }

    /// After every message, while the card is shown: work out again what
    /// the methods work on when that changed.
    pub(crate) fn settle_mesh_wizard(&mut self) {
        if !self.mesh_wizard.open {
            self.mesh_wizard.scope = None;
            return;
        }
        let print = self.scope_print();
        if self.mesh_wizard.scope.as_ref().map(|(known, _)| *known) != Some(print) {
            let scope = self.mesh_scope();
            self.mesh_wizard.scope = Some((print, scope));
        }
    }

    /// Whether the options of a method hold its recommended settings.
    fn mesh_recommended(&self, method: MeshMethod) -> bool {
        match method {
            MeshMethod::Closed => self.closed_mesh_recommended(),
            MeshMethod::Faces => self.faces_recommended(),
            MeshMethod::Surface => {
                let config = SurfaceMeshConfig::default();
                let defaults = [
                    config.max_vertices.to_string(),
                    config.neighbors.to_string(),
                    config.max_edge_factor.to_string(),
                    config.mesh_size.to_string(),
                ];
                self.surface_mesh_config().is_ok_and(|now| {
                    [
                        now.max_vertices.to_string(),
                        now.neighbors.to_string(),
                        now.max_edge_factor.to_string(),
                        now.mesh_size.to_string(),
                    ] == defaults
                })
            }
            MeshMethod::Terrain => true,
        }
    }

    /// The card as `status` of the local API reports it: whether it is
    /// shown, its step and method, the method whose job runs, whether the
    /// method shown can run and why not, whether its options are the
    /// recommended ones, the state of its Run step and the scan that holds
    /// its result.
    pub(crate) fn mesh_wizard_value(&self) -> Value {
        let wizard = &self.mesh_wizard;
        let refusal = self.mesh_refusal(wizard.method);
        let scope = wizard.scope.as_ref().map(|(_, scope)| scope);
        json!({
            "open": wizard.open,
            "step": wizard.step.id(),
            "method": wizard.method.id(),
            "running": self.mesh_running().map(MeshMethod::id),
            "run_ready": refusal.is_none(),
            "run_reason": refusal,
            "recommended": self.mesh_recommended(wizard.method),
            "run_state": self.method_run(wizard.method).key(),
            "result_scan": self
                .mesh_result_holder(wizard.method)
                .and_then(|index| self.clouds.get(index))
                .map(|entry| display_name(&entry.cloud.path)),
            "scope": scope.map(|scope| json!({
                "active": scope.active.as_ref().map(|(name, points)| json!({"name": name, "points": points})),
                "visible_scans": scope.visible.0,
                "visible_points": scope.visible.1,
                "section": scope.section.map(|(size, active, visible)| json!({
                    "size": size,
                    "active_points": active,
                    "visible_points": visible,
                })),
                "selected": scope.selected,
            })),
        })
    }

    /// The `mesh_wizard` command of the local API: show the card, with a
    /// method and on a step when they are named, or take it away.
    pub(crate) fn api_mesh_wizard(
        &mut self,
        open: bool,
        step: Option<&str>,
        method: Option<&str>,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let step = match step.map(|id| WizardStep::from_id(&id.to_ascii_lowercase())) {
            Some(None) => {
                return refuse(format!(
                    "unknown step; use {}",
                    WizardStep::ids().join(", ")
                ))
            }
            Some(found) => found,
            None => None,
        };
        let method = match method.map(|id| MeshMethod::from_id(&id.to_ascii_lowercase())) {
            Some(None) => {
                return refuse(format!(
                    "unknown method; use {}",
                    MeshMethod::ids().join(", ")
                ))
            }
            Some(found) => found,
            None => None,
        };
        if !open && (step.is_some() || method.is_some()) {
            return refuse("step and method can only be given with open: true".into());
        }
        if open && self.settings.is_some() {
            return refuse("the Settings dialog is open".into());
        }
        if open {
            self.open_mesh_wizard();
            if let Some(method) = method {
                self.mesh_wizard.method = method;
            }
            if let Some(step) = step {
                self.mesh_wizard.step = step;
            }
        } else {
            self.mesh_wizard.close();
        }
        (
            json!({"ok": true, "mesh_wizard": self.mesh_wizard_value()}),
            Task::none(),
        )
    }

    /// What the button of the SURFACE group says while a job of one of the
    /// methods runs: the step under way, of how many, and how far that
    /// step is. The percentage is that of the step, not of the whole job.
    pub(crate) fn mesh_busy_label(&self) -> Option<String> {
        let method = self.mesh_running()?;
        Some(match self.method_run(method) {
            RunState::Running(progress) => {
                let percent = progress
                    .fraction
                    .map(|fraction| format!("{:.0}", (fraction * 100.0).floor()));
                match (progress.steps, percent) {
                    (Some((place, count)), Some(percent)) => tr_args(
                        "Step {place}/{count} · {percent}%",
                        &[("place", &place), ("count", &count), ("percent", &percent)],
                    ),
                    (Some((place, count)), None) => tr_args(
                        "Step {place}/{count}",
                        &[("place", &place), ("count", &count)],
                    ),
                    (None, Some(percent)) => {
                        tr_args("Running {percent}%", &[("percent", &percent)])
                    }
                    (None, None) => tr("Running").to_owned(),
                }
            }
            _ => tr("Running").to_owned(),
        })
    }

    /// The one button of the SURFACE group. While a job of one of the
    /// methods runs it is highlighted and says how far the job is.
    pub(crate) fn mesh_wizard_ribbon_item(&self) -> opencad_ribbon::RibbonItem<'static> {
        let busy = self.mesh_busy_label();
        let open = self.mesh_wizard.open;
        let active = open || busy.is_some();
        let enabled = self.active.is_some() || active;
        let tip = busy.map_or_else(
            || tr("Mesh Pointcloud").to_owned(),
            |busy| format!("{}\n{busy}", tr("Mesh Pointcloud")),
        );
        opencad_ribbon::RibbonItem::Large(
            ui_style::ribbon_tooltip(
                button(container(icon_svg(ToolIcon::MeshPointcloud, 26.0)).center(Fill))
                    .on_press_maybe(enabled.then_some(Message::MeshWizard(MeshWizardAction::Open)))
                    .style(move |theme, status| ui_style::ribbon_button(theme, active, status))
                    .width(crate::LARGE_TOOL_MIN_WIDTH)
                    .height(Fill)
                    .padding([3, 2]),
                tip,
            )
            .into(),
        )
    }

    /// The card over the dimmed window, while it is shown. A click beside it
    /// closes it, as Escape does.
    pub(crate) fn mesh_wizard_view(&self) -> Option<Element<'_, Message>> {
        let wizard = &self.mesh_wizard;
        if !wizard.open {
            return None;
        }
        let send = Message::MeshWizard;
        let header = row![
            icon_svg(ToolIcon::MeshPointcloud, 22.0),
            text(tr("Mesh Pointcloud"))
                .size(15)
                .font(crate::fonts::SEMIBOLD),
            horizontal_space(),
            button(text("×").size(14))
                .on_press(send(MeshWizardAction::Close))
                .style(ui_style::tool)
                .padding([1, 8]),
        ]
        .spacing(10)
        .align_y(iced::Alignment::Center);
        let page = match wizard.step {
            WizardStep::Method => self.method_page(),
            WizardStep::Options => self.options_page(),
            WizardStep::Run => self.run_page(),
        };
        let card = container(
            column![
                header,
                self.mesh_wizard_steps(),
                rule(),
                ui_style::scrollable(container(page).padding(iced::Padding {
                    right: 14.0,
                    ..iced::Padding::ZERO
                }))
                .height(Fill),
                rule(),
                self.mesh_wizard_footer(),
            ]
            .spacing(12),
        )
        .width(Fill)
        .height(Fill)
        .max_width(CARD_WIDTH)
        .max_height(CARD_HEIGHT)
        .padding(18)
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.dialog_bg)
                .color(colors.dialog_content_text)
                .border(Border {
                    color: colors.dialog_border,
                    width: 1.0,
                    radius: 8.0.into(),
                })
        });
        Some(opaque(
            mouse_area(center(opaque(card)).padding(20).style(|theme| {
                container::Style::default().background(ui_theme::colors(theme).dialog_overlay)
            }))
            .on_press(send(MeshWizardAction::Close)),
        ))
    }

    /// The three steps, each with its number; the one shown stands out.
    fn mesh_wizard_steps(&self) -> Element<'_, Message> {
        let shown = self.mesh_wizard.step;
        let mut bar = row![].spacing(8).align_y(iced::Alignment::Center);
        for step in WizardStep::ALL {
            if step != WizardStep::Method {
                bar = bar.push(container(Space::new(32, 1)).style(|theme| {
                    container::Style::default().background(ui_theme::colors(theme).border)
                }));
            }
            let active = shown == step;
            let number = container(text(step.number().to_string()).size(11))
                .width(20)
                .height(20)
                .center(20)
                .style(move |theme: &Theme| {
                    let colors = ui_theme::colors(theme);
                    container::Style::default()
                        .background(if active {
                            colors.accent
                        } else {
                            colors.btn_secondary_bg
                        })
                        .color(if active {
                            colors.accent_text
                        } else {
                            colors.text
                        })
                        .border(Border {
                            color: if active { colors.accent } else { colors.border },
                            width: 1.0,
                            radius: 10.0.into(),
                        })
                });
            bar = bar.push(
                button(
                    row![number, text(tr(step.label())).size(12)]
                        .spacing(6)
                        .align_y(iced::Alignment::Center),
                )
                .on_press(Message::MeshWizard(MeshWizardAction::Step(step)))
                .padding([3, 8])
                .style(move |theme: &Theme, status| {
                    let colors = ui_theme::colors(theme);
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: hovered.then_some(colors.hover.into()),
                        text_color: if active { colors.accent } else { colors.text },
                        border: Border::default().rounded(4),
                        ..button::Style::default()
                    }
                }),
            );
        }
        bar.into()
    }

    /// Step 1: the four methods as cards, and what they work on.
    fn method_page(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let cards = MeshMethod::ALL
            .into_iter()
            .fold(row![].spacing(10).height(236), |cards, method| {
                cards.push(self.method_card(method))
            });
        column![
            text(tr(
                "Choose how to mesh the points. Each method keeps its own options."
            ))
            .size(12)
            .color(colors.dialog_content_secondary),
            cards,
            self.scope_view(),
        ]
        .spacing(16)
        .into()
    }

    /// A method as a card: its icon and name, what it makes and when to use
    /// it. The chosen one has an outline in the accent colour.
    fn method_card(&self, method: MeshMethod) -> Element<'_, Message> {
        let chosen = self.mesh_wizard.method == method;
        let mut heading = row![
            icon_svg(method.icon(), 28.0),
            text(tr(method.label()))
                .size(14)
                .font(crate::fonts::SEMIBOLD),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        if self.method_runs(method) {
            heading = heading
                .push(horizontal_space())
                .push(note(tr("Running").to_owned(), true));
        }
        button(
            column![
                heading,
                text(tr(method.makes())).size(11),
                note(tr(method.when()).to_owned(), false),
            ]
            .spacing(8),
        )
        .on_press(Message::MeshWizard(MeshWizardAction::Method(method)))
        .width(Fill)
        .height(Fill)
        .padding(12)
        .style(move |theme: &Theme, status| {
            let colors = ui_theme::colors(theme);
            let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
            button::Style {
                background: Some(
                    if chosen {
                        colors.hover_strong
                    } else if hovered {
                        colors.hover
                    } else {
                        colors.accent_soft
                    }
                    .into(),
                ),
                text_color: colors.text,
                border: Border {
                    color: if chosen { colors.accent } else { colors.border },
                    width: if chosen { 2.0 } else { 1.0 },
                    radius: 6.0.into(),
                },
                ..button::Style::default()
            }
        })
        .into()
    }

    /// Which scans the method shown reads: the active one, or every visible
    /// one when its options say so.
    fn reads_visible(&self, method: MeshMethod) -> bool {
        match method {
            MeshMethod::Closed => self.closed_mesh.layers() == Layers::Visible,
            MeshMethod::Faces => self.faces.layers() == Layers::Visible,
            MeshMethod::Terrain | MeshMethod::Surface => false,
        }
    }

    /// What the method shown works on: the scans with their points, the
    /// section box with about how many points lie in it, and the selection,
    /// with what fits the section box to it.
    fn scope_view(&self) -> Element<'_, Message> {
        let wizard = &self.mesh_wizard;
        let Some((_, scope)) = &wizard.scope else {
            return Space::new(0, 0).into();
        };
        let visible = self.reads_visible(wizard.method);
        let points = |count: u64| format_count(count);
        let mut rows = column![text(tr("What it works on")).size(13)].spacing(8);
        rows = rows.push(scope_row(
            tr("Active scan").to_owned(),
            match &scope.active {
                Some((name, count)) => tr_args(
                    "{name}: {points} points",
                    &[("name", name), ("points", &points(*count))],
                ),
                None => tr("No scan is active: click a scan in the Project Browser").to_owned(),
            },
            None,
        ));
        if visible {
            rows = rows.push(scope_row(
                tr("Visible scans").to_owned(),
                tr_args(
                    "{count} scans: {points} points",
                    &[
                        ("count", &scope.visible.0),
                        ("points", &points(scope.visible.1)),
                    ],
                ),
                None,
            ));
        }
        let (section, toggle) = match scope.section {
            Some((size, active, all)) => (
                tr_args(
                    "On, {size} m: about {points} points inside",
                    &[
                        (
                            "size",
                            &format!("{:.1} × {:.1} × {:.1}", size[0], size[1], size[2]),
                        ),
                        ("points", &points(if visible { all } else { active })),
                    ],
                ),
                plain_button(tr("Switch off"), Some(Message::SetSectionEnabled(false))),
            ),
            None => (
                tr("Off: the whole scan takes part").to_owned(),
                plain_button(tr("Switch on"), Some(Message::SetSectionEnabled(true))),
            ),
        };
        rows = rows.push(scope_row(
            tr("Section box").to_owned(),
            section,
            Some(toggle),
        ));
        rows = rows.push(scope_row(
            tr("Selection").to_owned(),
            if scope.selected == 0 {
                tr("No points selected").to_owned()
            } else {
                tr_args(
                    "{points} points selected",
                    &[("points", &points(scope.selected))],
                )
            },
            Some(plain_button(
                tr("Fit the section box to it"),
                (scope.selected > 0 && !self.selection_bounds_pending)
                    .then_some(Message::FitSectionToSelection),
            )),
        ));
        rows = rows.push(note(
            tr("Only what lies inside the section box takes part, without deleted points and hidden classes. The selection itself is not meshed: fit the section box to it to mesh that part.")
                .to_owned(),
            false,
        ));
        container(rows)
            .padding(12)
            .width(Fill)
            .style(|theme| {
                let colors = ui_theme::colors(theme);
                container::Style::default()
                    .background(colors.accent_soft)
                    .border(Border {
                        color: colors.border,
                        width: 1.0,
                        radius: 6.0.into(),
                    })
            })
            .into()
    }

    /// Step 2: the settings of the method shown with their defaults and
    /// explanations, the recommended preset, and what a job would do.
    fn options_page(&self) -> Element<'_, Message> {
        let method = self.mesh_wizard.method;
        let colors = self.ui_theme.colors();
        let fields: Element<'_, Message> = match method {
            MeshMethod::Closed => self.closed_mesh_options(),
            MeshMethod::Faces => self.faces_options(),
            MeshMethod::Surface => self.surface_options(),
            MeshMethod::Terrain => note(
                tr("A terrain mesh has no settings of its own: every point of the scan passes through a grid seen from above, the lowest point of each cell becomes a vertex, at most 100,000 of them, and long edges across gaps are left out.")
                    .to_owned(),
                false,
            ),
        };
        let recommended = self.mesh_recommended(method);
        let mut preset = row![].spacing(12).align_y(iced::Alignment::Center);
        if method != MeshMethod::Terrain {
            preset = preset.push(plain_button(
                tr("Use recommended"),
                (!recommended).then_some(Message::MeshWizard(MeshWizardAction::Recommended)),
            ));
        }
        preset = preset.push(note(
            if recommended && method != MeshMethod::Terrain {
                tr("The recommended settings are in use.").to_owned()
            } else {
                tr(method.preset()).to_owned()
            },
            false,
        ));
        let notes = self.method_notes(method);
        let mut said = column![].spacing(6);
        for line in notes.lines {
            said = said.push(note(line, false));
        }
        for line in notes.warnings {
            said = said.push(note(line, true));
        }
        if let Some(refusal) = notes.refusal {
            said = said.push(note(refusal, true));
        }
        column![
            row![
                icon_svg(method.icon(), 22.0),
                text(tr(method.label()))
                    .size(14)
                    .font(crate::fonts::SEMIBOLD)
                    .color(colors.accent),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            note(tr(method.makes()).to_owned(), false),
            fields,
            preset,
            rule(),
            said,
        ]
        .spacing(12)
        .into()
    }

    /// The settings of the 3D surface, as Properties held them before.
    fn surface_options(&self) -> Element<'_, Message> {
        let config = SurfaceMeshConfig::default();
        let settings = &self.surface_settings;
        column![
            option_input(
                "Max vertices",
                &settings[0],
                "50000",
                config.max_vertices.to_string(),
                |value| Message::SurfaceSetting(0, value),
                key("The most vertices the surface gets, from 3 to 1,000,000. More vertices follow more detail and take longer."),
            ),
            option_input(
                "Neighbors",
                &settings[1],
                "12",
                config.neighbors.to_string(),
                |value| Message::SurfaceSetting(1, value),
                key("How many nearby vertices each vertex is joined with, from 3 to 32."),
            ),
            option_input(
                "Edge factor",
                &settings[2],
                "4",
                config.max_edge_factor.to_string(),
                |value| Message::SurfaceSetting(2, value),
                key("Triangles with an edge longer than this many times the usual distance between the vertices are left out, so that gaps stay open. Above 0."),
            ),
            option_input(
                "Mesh size (m)",
                &settings[3],
                "0 = auto",
                config.mesh_size.to_string(),
                |value| Message::SurfaceSetting(3, value),
                key("The width of a voxel in which one point is kept before the vertices are thinned, so that no two vertices lie much closer together; 0 leaves the spacing to the number of vertices."),
            ),
        ]
        .spacing(4)
        .into()
    }

    /// Step 3: the job under way with its progress and Cancel, or how the
    /// last one ended, with its figures and what to do with the result.
    fn run_page(&self) -> Element<'_, Message> {
        let method = self.mesh_wizard.method;
        let colors = self.ui_theme.colors();
        let send = Message::MeshWizard;
        let back = plain_button(
            tr("Back to options"),
            Some(send(MeshWizardAction::Step(WizardStep::Options))),
        );
        let mut page = column![row![
            icon_svg(method.icon(), 22.0),
            text(tr(method.label()))
                .size(14)
                .font(crate::fonts::SEMIBOLD)
                .color(colors.accent),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)]
        .spacing(12);
        match self.method_run(method) {
            RunState::Idle => {
                page = page
                    .push(note(
                        tr("Nothing has run with this method in this session yet. Check the options and choose Run.")
                            .to_owned(),
                        false,
                    ))
                    .push(back);
            }
            RunState::Choosing => {
                page = page.push(text(tr("Choose where to save the OBJ file…")).size(13));
            }
            RunState::Running(progress) => {
                let mut figures = row![].spacing(16);
                if let Some((place, count)) = progress.steps {
                    figures = figures.push(note(
                        tr_args(
                            "Step {place} of {count}",
                            &[("place", &place), ("count", &count)],
                        ),
                        false,
                    ));
                }
                if let Some(fraction) = progress.fraction {
                    figures = figures.push(note(
                        tr_args(
                            "{percent}% of this step",
                            &[("percent", &format!("{:.0}", (fraction * 100.0).floor()))],
                        ),
                        false,
                    ));
                }
                figures = figures.push(note(
                    tr_args("{seconds} s", &[("seconds", &progress.seconds)]),
                    false,
                ));
                page = page
                    .push(text(progress.stage).size(13))
                    .push(figures)
                    .push(
                        ui_style::progress_bar(0.0..=1.0, progress.fraction.unwrap_or(0.0)),
                    )
                    .push(note(
                        tr("The job goes on when the card is closed: the Mesh Pointcloud button shows that it runs, and opens the card here again.")
                            .to_owned(),
                        false,
                    ))
                    .push(plain_button(
                        tr("Cancel"),
                        (!progress.cancelling).then_some(send(MeshWizardAction::Cancel)),
                    ));
            }
            RunState::Done {
                rows,
                kept,
                lines,
                warnings,
            } => {
                // Show in model and Export act on the scan that holds this
                // result, which need not be the active scan. Once no scan
                // holds it, the step says so instead of where it went.
                let holder = self.mesh_result_holder(method);
                page = page.push(container(column(rows).spacing(0)).width(Length::Fixed(460.0)));
                match kept {
                    Some(line) if holder.is_some() => page = page.push(note(line, false)),
                    Some(_) => page = page.push(note(tr(NOT_HELD).to_owned(), true)),
                    None => {}
                }
                for line in lines {
                    page = page.push(note(line, false));
                }
                for line in warnings {
                    page = page.push(note(line, true));
                }
                page = page.push(
                    row![
                        primary_button(
                            tr("Show in model"),
                            holder.map(|_| send(MeshWizardAction::ShowInModel)),
                        ),
                        plain_button(
                            tr("Export…"),
                            self.mesh_result_exportable(method)
                                .then_some(send(MeshWizardAction::Export)),
                        ),
                        back,
                    ]
                    .spacing(8),
                );
            }
            RunState::Cancelled(line) => {
                page = page.push(note(line, false)).push(back);
            }
            RunState::Failed(error) => {
                page = page
                    .push(note(
                        tr("The last job of this method failed:").to_owned(),
                        false,
                    ))
                    .push(note(error, true))
                    .push(back);
            }
        }
        page.into()
    }

    /// Close at the left; at the right Back and the button that goes on:
    /// Next on the first step and Run on the options, with the reason it
    /// waits for.
    fn mesh_wizard_footer(&self) -> Element<'_, Message> {
        let wizard = &self.mesh_wizard;
        let send = Message::MeshWizard;
        let mut footer = row![
            plain_button(tr("Close"), Some(send(MeshWizardAction::Close))),
            horizontal_space()
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        let refusal = (wizard.step == WizardStep::Options)
            .then(|| self.mesh_refusal(wizard.method))
            .flatten();
        if let Some(reason) = &refusal {
            footer = footer.push(
                container(note(reason.clone(), false))
                    .max_width(420.0)
                    .align_y(iced::Alignment::Center),
            );
        }
        if wizard.step != WizardStep::Method {
            footer = footer.push(plain_button(
                tr("Previous"),
                Some(send(MeshWizardAction::Back)),
            ));
        }
        footer = match wizard.step {
            WizardStep::Method => footer.push(primary_button(
                tr("Next"),
                Some(send(MeshWizardAction::Next)),
            )),
            WizardStep::Options => footer.push(primary_button(
                tr("Run"),
                refusal.is_none().then_some(send(MeshWizardAction::Run)),
            )),
            // Show in model is what goes on from a result.
            WizardStep::Run => {
                let idle = !self.method_runs(wizard.method);
                footer.push(plain_button(
                    tr(self.run_label(wizard.method)),
                    (idle && self.mesh_refusal(wizard.method).is_none())
                        .then_some(send(MeshWizardAction::Run)),
                ))
            }
        };
        footer.into()
    }
}
