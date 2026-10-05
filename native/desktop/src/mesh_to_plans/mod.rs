//! Mesh to Plans: the wizard that makes plans, sections, elevations, a site
//! plan and a model of a building from its scan, one step at a time. It is a
//! card over the window with the steps in a sidebar, the settings of the
//! step in the middle, its preview at the right and the buttons that move
//! through the steps at the bottom. Show in model makes the card a strip
//! above the scene, so that the model can be looked at and the section box
//! moved while the wizard waits. The steps run on the worker of
//! `pipeline`, one job at a time.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Tree, Widget};
use iced::advanced::{overlay, renderer, Clipboard, Shell};
use iced::widget::{
    button, column, container, horizontal_space, opaque, progress_bar, row, scrollable, text, Space,
};
use iced::{event, mouse, Border, Color, Element, Event, Fill, Length, Rectangle, Size, Task};
use iced::{Theme, Vector};
use serde_json::{json, Value};

use crate::closed_mesh::Sentence;
use crate::i18n::{key, tr};
use crate::{drawing_view, flat_tool_style, opencad_ribbon, ui_theme, Message, Studio};

mod pipeline;
mod strip;
#[cfg(test)]
mod tests;

pub use pipeline::PipelineEnd;
use pipeline::{Last, PipelineJob, Work};

/// The share of the window the card takes, and the least it takes when the
/// window is large enough for that.
const CARD_SHARE: f32 = 0.9;
const CARD_MIN: Size = Size::new(960.0, 640.0);
/// The width of the sidebar with the steps and of the column with the
/// settings of a step.
const SIDEBAR_W: f32 = 210.0;
const SETTINGS_W: f32 = 320.0;

/// A step of the wizard, in the order of the sidebar. The plans of step 3
/// are made in four parts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WizardStep {
    /// The scans, the project folder, the frame of the building and its
    /// levels.
    #[default]
    Prepare,
    /// Closed meshes of the building and of its storeys.
    Mesh,
    /// Raw plans, sections and elevations straight from the points.
    Views,
    /// 3a: walls with their axis and thickness.
    Walls,
    /// 3b: doors and windows.
    Openings,
    /// 3c: stairs, rooms, voids and the lines below and above the cut.
    Rooms,
    /// 3d: scale, paper, dimensions and the title block.
    Sheet,
    /// The terrain and the site plan.
    Site,
    /// What was made, the check report and the IFC file.
    Result,
}

impl WizardStep {
    pub const ALL: [Self; 9] = [
        Self::Prepare,
        Self::Mesh,
        Self::Views,
        Self::Walls,
        Self::Openings,
        Self::Rooms,
        Self::Sheet,
        Self::Site,
        Self::Result,
    ];

    /// The name the local API knows the step by.
    pub fn id(self) -> &'static str {
        match self {
            Self::Prepare => "prepare",
            Self::Mesh => "mesh",
            Self::Views => "views",
            Self::Walls => "walls",
            Self::Openings => "openings",
            Self::Rooms => "rooms",
            Self::Sheet => "sheet",
            Self::Site => "site",
            Self::Result => "result",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.id() == value)
    }

    /// Every id `from_id` accepts, for the command that shows a step.
    pub fn ids() -> Vec<&'static str> {
        Self::ALL.into_iter().map(Self::id).collect()
    }

    /// The number of the step in the sidebar.
    pub fn number(self) -> &'static str {
        match self {
            Self::Prepare => "0",
            Self::Mesh => "1",
            Self::Views => "2",
            Self::Walls => "3a",
            Self::Openings => "3b",
            Self::Rooms => "3c",
            Self::Sheet => "3d",
            Self::Site => "4",
            Self::Result => "5",
        }
    }

    /// The English name of the step.
    pub fn label(self) -> &'static str {
        match self {
            Self::Prepare => key("Preparation"),
            Self::Mesh => key("Mesh"),
            Self::Views => key("Sections, elevations and raw plans"),
            Self::Walls => key("Walls"),
            Self::Openings => key("Openings"),
            Self::Rooms => key("Stairs, rooms, voids and lines"),
            Self::Sheet => key("Sheet"),
            Self::Site => key("Terrain and site plan"),
            Self::Result => key("Result and IFC"),
        }
    }

    /// What the step makes, in one sentence.
    fn lead(self) -> &'static str {
        match self {
            Self::Prepare => key(
                "Choose the scans and the project folder, and find the frame and the levels of the building.",
            ),
            Self::Mesh => key(
                "A closed mesh of the whole building and one per storey, as a 3D product and as evidence.",
            ),
            Self::Views => key(
                "A raw plan per level, the sections A-A and B-B and the elevations, straight from the points.",
            ),
            Self::Walls => key(
                "Walls with their axis and thickness, each with its evidence and its confidence.",
            ),
            Self::Openings => key("Doors and windows in their walls, with their sizes and their swing."),
            Self::Rooms => key(
                "Stairs, rooms with their floor area, voids, and the lines below and above the cut.",
            ),
            Self::Sheet => key(
                "Scale, paper, dimensions, room stamps and the title block of every plan.",
            ),
            Self::Site => key(
                "The terrain with its contours and spot heights, kerbs, trees and paving.",
            ),
            Self::Result => key(
                "Every drawing and model with its state, the check report and the IFC file.",
            ),
        }
    }

    fn place(self) -> usize {
        Self::ALL
            .iter()
            .position(|step| *step == self)
            .unwrap_or_default()
    }

    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.place() + 1).copied()
    }

    pub fn previous(self) -> Option<Self> {
        self.place()
            .checked_sub(1)
            .and_then(|place| Self::ALL.get(place).copied())
    }

    /// Whether the wizard can do without the step: a plan needs no mesh.
    fn optional(self) -> bool {
        self == Self::Mesh
    }

    /// Whether the step is one of the four parts of the plans.
    fn in_plans(self) -> bool {
        matches!(
            self,
            Self::Walls | Self::Openings | Self::Rooms | Self::Sheet
        )
    }
}

/// Where a step stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StepStatus {
    #[default]
    NotRun,
    Running,
    /// Run, and waiting for the user to confirm what it proposes.
    Done,
    Confirmed,
    Skipped,
    /// Failed for this reason.
    Failed(String),
}

impl StepStatus {
    /// The name of the status in the local API.
    pub fn key(&self) -> &'static str {
        match self {
            Self::NotRun => "not_run",
            Self::Running => "running",
            Self::Done => "done",
            Self::Confirmed => "confirmed",
            Self::Skipped => "skipped",
            Self::Failed(_) => "failed",
        }
    }

    /// The status in words, in the language in use.
    fn text(&self) -> String {
        match self {
            Self::NotRun => tr("Not run").into(),
            Self::Running => tr("Running").into(),
            Self::Done => tr("Waiting for confirmation").into(),
            Self::Confirmed => tr("Confirmed").into(),
            Self::Skipped => tr("Skipped").into(),
            Self::Failed(_) => tr("Failed").into(),
        }
    }

    /// The colour of the dot of the status in the sidebar, and whether the
    /// dot is filled.
    fn dot(&self, theme: &Theme) -> (Color, bool) {
        let colors = ui_theme::colors(theme);
        let palette = theme.palette();
        match self {
            Self::NotRun => (colors.muted, false),
            Self::Running => (colors.accent, true),
            Self::Done => (palette.success, false),
            Self::Confirmed => (palette.success, true),
            Self::Skipped => (colors.muted, true),
            Self::Failed(_) => (palette.danger, true),
        }
    }
}

/// Everything the wizard reacts to.
#[derive(Debug, Clone)]
pub enum WizardAction {
    /// Show the card, on the step it showed last, also when it is a strip.
    Open,
    /// Take the card or the strip away; what the steps hold stays.
    Close,
    /// Show in model: make the card a strip above the scene.
    Minimize,
    /// Back to wizard: make the strip the card again.
    Restore,
    /// Show a step.
    Step(WizardStep),
    Back,
    Next,
    /// Go on without the shown step, where the wizard can do without it.
    Skip,
    /// Run the shown step.
    Run,
    /// Run every step that is not confirmed or skipped, in their order,
    /// and confirm each one as it ends.
    RunAll,
    /// Take what the shown step proposes.
    Confirm,
    Poll,
    Cancel,
    Finished(u64, PipelineEnd),
}

/// The wizard: whether it is shown, as a card or as a strip, the step it
/// shows and where every step stands.
#[derive(Debug, Default)]
pub(crate) struct Wizard {
    open: bool,
    /// Shown as a strip above the scene instead of as a card.
    minimized: bool,
    step: WizardStep,
    states: [StepStatus; WizardStep::ALL.len()],
    /// The job under way, how the last one ended and what the steps of a
    /// job do.
    job: Option<PipelineJob>,
    next_serial: u64,
    last: Option<Last>,
    /// The job of the local API that reports the last job.
    last_job_id: Option<String>,
    work: Work,
}

impl Wizard {
    /// Whether the wizard is shown, as a card or as a strip.
    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    /// Whether its card lies over the window and hides the model.
    pub(crate) fn covers_model(&self) -> bool {
        self.open && !self.minimized
    }

    fn status(&self, step: WizardStep) -> &StepStatus {
        &self.states[step.place()]
    }

    fn set_status(&mut self, step: WizardStep, status: StepStatus) {
        self.states[step.place()] = status;
    }

    /// Whether a job runs steps.
    pub(crate) fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// Make the card a strip, if the card is shown; whether it was.
    pub(crate) fn minimize(&mut self) -> bool {
        let covering = self.covers_model();
        self.minimized |= covering;
        covering
    }

    /// The wizard as `status` of the local API reports it: whether it is
    /// shown and how, the step it shows, whether Next may leave that step
    /// and why not, and where every step stands.
    pub(crate) fn value(&self) -> Value {
        let ready = self.step_ready();
        json!({
            "open": self.open,
            "minimized": self.open && self.minimized,
            "step": self.step.id(),
            "next_ready": ready.is_ok(),
            "next_reason": ready.err().map(|reason| reason.english()),
            "steps": WizardStep::ALL.into_iter().map(|step| json!({
                "id": step.id(),
                "number": step.number(),
                "name": step.label(),
                "status": self.status(step).key(),
            })).collect::<Vec<_>>(),
            "job": self.job.as_ref().map(PipelineJob::progress_value),
            "last": self.last.as_ref().map(Last::value),
            "job_id": self
                .job
                .as_ref()
                .map(|job| job.api_job_id.as_str())
                .or(self.last_job_id.as_deref()),
        })
    }

    /// Whether Next may leave the shown step, and why not: the step has to
    /// be confirmed or skipped first.
    pub(crate) fn step_ready(&self) -> Result<(), Sentence> {
        if self.step.next().is_none() {
            return Err(Sentence::plain(key("This is the last step")));
        }
        match self.status(self.step) {
            StepStatus::Confirmed | StepStatus::Skipped => Ok(()),
            StepStatus::NotRun => Err(Sentence::plain(key("Run this step first"))),
            StepStatus::Running => Err(Sentence::plain(key("Wait until this step has finished"))),
            StepStatus::Done => Err(Sentence::plain(key(
                "Confirm the result of this step first",
            ))),
            StepStatus::Failed(reason) => Err(Sentence::with(
                key("This step failed: {reason}"),
                &[("reason", reason.clone())],
            )),
        }
    }
}

impl Studio {
    pub(crate) fn update_mesh_to_plans(&mut self, action: WizardAction) -> Task<Message> {
        let wizard = &mut self.mesh_to_plans;
        match action {
            WizardAction::Open => {
                // The card lies over the window; the File view it may have
                // been opened from steps aside.
                self.file_open = false;
                wizard.open = true;
                wizard.minimized = false;
            }
            WizardAction::Close => {
                wizard.open = false;
                wizard.minimized = false;
            }
            WizardAction::Minimize => {
                wizard.minimize();
            }
            WizardAction::Restore => {
                if wizard.open {
                    self.file_open = false;
                    wizard.minimized = false;
                }
            }
            WizardAction::Step(step) => wizard.step = step,
            WizardAction::Back => {
                if let Some(previous) = wizard.step.previous() {
                    wizard.step = previous;
                }
            }
            WizardAction::Next => {
                if wizard.step_ready().is_ok() {
                    if let Some(next) = wizard.step.next() {
                        wizard.step = next;
                    }
                }
            }
            WizardAction::Skip => {
                // A step that runs is left alone until its job ends.
                let running = *wizard.status(wizard.step) == StepStatus::Running;
                if wizard.step.optional() && !running {
                    wizard.set_status(wizard.step, StepStatus::Skipped);
                }
            }
            WizardAction::Run => {
                let step = wizard.step;
                return self.start_mesh_to_plans_job(vec![step], false);
            }
            WizardAction::RunAll => {
                let steps = WizardStep::ALL
                    .into_iter()
                    .filter(|step| {
                        !matches!(
                            wizard.status(*step),
                            StepStatus::Confirmed | StepStatus::Skipped
                        )
                    })
                    .collect();
                return self.start_mesh_to_plans_job(steps, true);
            }
            WizardAction::Confirm => {
                if *wizard.status(wizard.step) == StepStatus::Done {
                    wizard.set_status(wizard.step, StepStatus::Confirmed);
                }
            }
            WizardAction::Poll => return self.mesh_to_plans_poll(),
            WizardAction::Cancel => self.cancel_mesh_to_plans(),
            WizardAction::Finished(serial, end) => self.mesh_to_plans_finished(serial, end),
        }
        Task::none()
    }

    /// The `mesh_to_plans_view` command of the local API: show the wizard,
    /// on a step when one is named and as the strip when `minimized` is
    /// true, or take it away.
    pub(crate) fn api_mesh_to_plans_view(
        &mut self,
        open: bool,
        step: Option<&str>,
        minimized: Option<bool>,
    ) -> Value {
        let refuse = |error: String| json!({"ok": false, "error": error});
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
        if !open && (step.is_some() || minimized.is_some()) {
            return refuse("step and minimized can only be given with open: true".into());
        }
        // The dialog lies over the card, and closes nothing when it opens.
        if open && self.settings.is_some() {
            return refuse("the Settings dialog is open".into());
        }
        let action = if open {
            WizardAction::Open
        } else {
            WizardAction::Close
        };
        let _ = self.update_mesh_to_plans(action);
        if let Some(step) = step {
            self.mesh_to_plans.step = step;
        }
        if minimized == Some(true) {
            self.mesh_to_plans.minimize();
        }
        json!({"ok": true, "mesh_to_plans": self.mesh_to_plans.value()})
    }

    /// The card over the dimmed window, while it is shown as a card.
    pub(crate) fn mesh_to_plans_view(&self) -> Option<Element<'_, Message>> {
        let wizard = &self.mesh_to_plans;
        if !wizard.covers_model() {
            return None;
        }
        let send = Message::MeshToPlans;
        let colors = self.ui_theme.colors();
        let header = row![
            text(tr("Mesh to Plans")).size(15),
            text(format!(
                "{}  {}",
                wizard.step.number(),
                tr(wizard.step.label())
            ))
            .size(12)
            .color(colors.muted),
            horizontal_space(),
            button(text(tr("Show in model")).size(12))
                .on_press(send(WizardAction::Minimize))
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .padding([4, 12]),
            button(text("×").size(14))
                .on_press(send(WizardAction::Close))
                .style(flat_tool_style)
                .padding([1, 8]),
        ]
        .spacing(14)
        .align_y(iced::Alignment::Center);

        let body = row![
            self.mesh_to_plans_sidebar(),
            rule_vertical(),
            container(scrollable(self.mesh_to_plans_settings()).height(Fill))
                .padding([4, 16])
                .width(SETTINGS_W)
                .height(Fill),
            self.mesh_to_plans_preview(),
        ]
        .spacing(0)
        .height(Fill);

        let card = container(
            column![header, body, self.mesh_to_plans_footer()]
                .spacing(14)
                .height(Fill),
        )
        .width(Fill)
        .height(Fill)
        .padding(18)
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.panel)
                .color(colors.text)
                .border(Border {
                    color: colors.border,
                    width: 1.0,
                    radius: 8.0.into(),
                })
        });

        Some(opaque(
            container(Share::new(opaque(card), CARD_SHARE, CARD_MIN))
                .width(Fill)
                .height(Fill)
                .style(|_| {
                    container::Style::default().background(Color::from_rgba8(0, 0, 0, 0.55))
                }),
        ))
    }

    /// The steps with the dot of their status. The parts of the plans stand
    /// under a heading of their own.
    fn mesh_to_plans_sidebar(&self) -> Element<'_, Message> {
        let wizard = &self.mesh_to_plans;
        let mut steps = column![].spacing(2).width(SIDEBAR_W);
        for step in WizardStep::ALL {
            if step == WizardStep::Walls {
                steps = steps.push(
                    container(
                        row![text("3").size(12).width(26), text(tr("Plans")).size(12),]
                            .align_y(iced::Alignment::Center),
                    )
                    .padding([7, 12])
                    .style(|theme| {
                        container::Style::default().color(ui_theme::colors(theme).muted)
                    }),
                );
            }
            let active = wizard.step == step;
            let status = wizard.status(step).clone();
            let indent = if step.in_plans() { 14.0 } else { 0.0 };
            steps = steps.push(
                button(
                    row![
                        Space::with_width(indent),
                        text(step.number()).size(12).width(26),
                        text(tr(step.label())).size(12).width(Fill),
                        status_dot(status),
                    ]
                    .spacing(4)
                    .align_y(iced::Alignment::Center),
                )
                .on_press(Message::MeshToPlans(WizardAction::Step(step)))
                .width(Fill)
                .padding([7, 12])
                .style(move |theme, status| {
                    let colors = ui_theme::colors(theme);
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: (active || hovered).then_some(
                            if active {
                                colors.panel_alt
                            } else {
                                colors.hover
                            }
                            .into(),
                        ),
                        text_color: if active { colors.accent } else { colors.text },
                        border: Border::default().rounded(4),
                        ..button::Style::default()
                    }
                }),
            );
        }
        container(scrollable(steps).height(Fill))
            .padding(iced::Padding {
                right: 10.0,
                ..iced::Padding::ZERO
            })
            .height(Fill)
            .into()
    }

    /// The settings and lists of the shown step.
    fn mesh_to_plans_settings(&self) -> Element<'_, Message> {
        let wizard = &self.mesh_to_plans;
        let step = wizard.step;
        let colors = self.ui_theme.colors();
        let mut page = column![
            text(tr(step.label())).size(14).color(colors.accent),
            text(tr(step.lead())).size(12).color(colors.muted),
            row![
                text(tr("Status")).size(12).color(colors.muted).width(90),
                text(wizard.status(step).text()).size(12),
            ]
            .spacing(8),
            text(tr("The settings of this step come in a later version."))
                .size(11)
                .color(colors.muted),
        ]
        .spacing(12)
        .width(Fill);
        if let Some(line) = self.mesh_to_plans_progress_line() {
            page = page.push(
                column![
                    text(line.detail).size(11).color(colors.muted),
                    progress_bar(0.0..=1.0, line.fraction.unwrap_or(0.0)).height(6),
                ]
                .spacing(4),
            );
        }
        if *wizard.status(step) == StepStatus::Done {
            page = page.push(
                button(text(tr("Confirm")).size(12))
                    .on_press(Message::MeshToPlans(WizardAction::Confirm))
                    .style(|theme, status| opencad_ribbon::file_tab_style(theme, false, status))
                    .padding([5, 16]),
            );
        }
        let running = *wizard.status(step) == StepStatus::Running;
        if step.optional() && !running && *wizard.status(step) != StepStatus::Skipped {
            page = page.push(
                button(text(tr("Skip this step")).size(12))
                    .on_press(Message::MeshToPlans(WizardAction::Skip))
                    .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                    .padding([5, 12]),
            );
        }
        page.into()
    }

    /// The preview of the shown step, on the paper of the Drawing view.
    fn mesh_to_plans_preview(&self) -> Element<'_, Message> {
        let paper = drawing_view::paper(self.ui_theme);
        container(
            text(tr("The preview of this step appears here."))
                .size(12)
                .color(Color::from_rgb8(120, 113, 108)),
        )
        .center(Fill)
        .style(move |theme| {
            container::Style::default()
                .background(paper)
                .border(Border {
                    color: ui_theme::colors(theme).border,
                    width: 1.0,
                    radius: 4.0.into(),
                })
        })
        .into()
    }

    /// Close, Previous, Run, Next and Run all, with the reason Next waits for.
    fn mesh_to_plans_footer(&self) -> Element<'_, Message> {
        let wizard = &self.mesh_to_plans;
        let send = Message::MeshToPlans;
        let ready = wizard.step_ready();
        let reason = match &ready {
            Ok(()) => String::new(),
            Err(sentence) => sentence.translated(),
        };
        let plain = |label: &'static str, message: Option<Message>| {
            button(text(tr(label)).size(12))
                .on_press_maybe(message)
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .padding([5, 12])
        };
        row![
            plain(key("Close"), Some(send(WizardAction::Close))),
            horizontal_space(),
            text(reason)
                .size(11)
                .color(self.ui_theme.colors().muted)
                .width(Length::Shrink),
            plain(
                key("Previous"),
                wizard.step.previous().map(|_| send(WizardAction::Back)),
            ),
            if wizard.is_running() {
                plain(key("Cancel"), Some(send(WizardAction::Cancel)))
            } else {
                plain(key("Run this step"), Some(send(WizardAction::Run)))
            },
            button(text(tr("Next")).size(12))
                .on_press_maybe(ready.is_ok().then_some(send(WizardAction::Next)))
                .style(|theme, status| opencad_ribbon::file_tab_style(theme, false, status))
                .padding([5, 16]),
            plain(
                key("Run all automatically"),
                (!wizard.is_running()).then_some(send(WizardAction::RunAll)),
            ),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)
        .into()
    }
}

/// The dot that tells the status of a step.
fn status_dot(status: StepStatus) -> Element<'static, Message> {
    container(Space::new(8, 8))
        .style(move |theme: &Theme| {
            let (color, filled) = status.dot(theme);
            container::Style::default()
                .background(if filled { color } else { Color::TRANSPARENT })
                .border(Border {
                    color,
                    width: 1.5,
                    radius: 4.0.into(),
                })
        })
        .into()
}

fn rule_vertical<'a>() -> Element<'a, Message> {
    container(Space::new(1, Fill))
        .style(|theme| container::Style::default().background(ui_theme::colors(theme).border))
        .into()
}

/// A share of the space it is given, but no less than a least size that
/// fits, for content centred in it: the card takes nine tenths of the
/// window and at least 960 by 640 pixels where the window has them.
struct Share<'a> {
    content: Element<'a, Message>,
    share: f32,
    least: Size,
}

impl<'a> Share<'a> {
    fn new(content: impl Into<Element<'a, Message>>, share: f32, least: Size) -> Self {
        Self {
            content: content.into(),
            share,
            least,
        }
    }

    /// The size of the content in a space of `space`.
    fn size_in(&self, space: Size) -> Size {
        let side = |space: f32, least: f32| (space * self.share).max(least).min(space);
        Size::new(
            side(space.width, self.least.width),
            side(space.height, self.least.height),
        )
    }
}

impl Widget<Message, Theme, iced::Renderer> for Share<'_> {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let space = limits.max();
        let size = self.size_in(space);
        let content = self.content.as_widget().layout(
            &mut tree.children[0],
            renderer,
            &layout::Limits::new(size, size),
        );
        let placed = content.size();
        let content = content.move_to(iced::Point::new(
            ((space.width - placed.width) / 2.0).max(0.0),
            ((space.height - placed.height) / 2.0).max(0.0),
        ));
        layout::Node::with_children(space, vec![content])
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if let Some(content) = layout.children().next() {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                content,
                cursor,
                viewport,
            );
        }
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        if let Some(content) = layout.children().next() {
            self.content
                .as_widget()
                .operate(&mut tree.children[0], content, renderer, operation);
        }
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        match layout.children().next() {
            Some(content) => self.content.as_widget_mut().on_event(
                &mut tree.children[0],
                event,
                content,
                cursor,
                renderer,
                clipboard,
                shell,
                viewport,
            ),
            None => event::Status::Ignored,
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        layout
            .children()
            .next()
            .map_or(mouse::Interaction::None, |content| {
                self.content.as_widget().mouse_interaction(
                    &tree.children[0],
                    content,
                    cursor,
                    viewport,
                    renderer,
                )
            })
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, iced::Renderer>> {
        let content = layout.children().next()?;
        self.content
            .as_widget_mut()
            .overlay(&mut tree.children[0], content, renderer, translation)
    }
}

impl<'a> From<Share<'a>> for Element<'a, Message> {
    fn from(share: Share<'a>) -> Self {
        Element::new(share)
    }
}
