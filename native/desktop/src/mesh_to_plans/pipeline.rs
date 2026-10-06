//! The worker of the Pointcloud to Drawing wizard. One job at a time runs the steps
//! it was given one after the other on a thread of its own, so that the
//! machine is not swamped and the window stays free to look at other steps.
//! The window reads how far it is four times a second, and a cancel stops it
//! at the next look the worker takes, well within one of those. Step 0, the
//! preparation, does its work; the steps after it are not built yet and
//! stand in for theirs for about a second.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iced::Task;
use pointcloud_core::LoadError;
use serde_json::{json, Value};

use super::prepare::{PrepareInput, Prepared};
use super::project::{now_seconds, SourceRef};
use super::{StepRun, StepStatus, WizardAction, WizardStep};
use crate::bag_panel::plain_reason;
use crate::open_progress::{Line, Phase};
use crate::{Message, Studio};

/// How often the window reads how far a job is.
pub(crate) const POLL: Duration = Duration::from_millis(250);

/// What the steps of a job do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Work {
    /// Every step that is built does its work, and the others stand in for
    /// theirs as `PLACEHOLDER` does.
    #[default]
    Steps,
    /// Stands in for the work of every step, as the tests do: it counts to
    /// `ticks`, one tick at a time, and looks for a cancel at each tick.
    #[cfg_attr(not(test), allow(dead_code))]
    Placeholder { ticks: u32, tick: Duration },
}

/// The work of a step that is not built yet: a second each.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) const PLACEHOLDER: Work = Work::Placeholder {
    ticks: STAND_IN_TICKS,
    tick: STAND_IN_TICK,
};
const STAND_IN_TICKS: u32 = 20;
const STAND_IN_TICK: Duration = Duration::from_millis(50);

/// A step that a job finished, and how long it took.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StepDone {
    pub(crate) step: WizardStep,
    pub(crate) seconds: f64,
}

/// How far a job is: the place of the step under way among the steps of
/// the job, and how far that step is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Progress {
    pub(crate) place: usize,
    pub(crate) done: u64,
    pub(crate) total: u64,
}

impl Progress {
    fn fraction(self) -> Option<f32> {
        (self.total > 0).then(|| (self.done as f64 / self.total as f64).min(1.0) as f32)
    }
}

/// What the worker of a job tells the window, and the window the worker.
#[derive(Debug, Default)]
pub(crate) struct Control {
    cancelled: AtomicBool,
    place: AtomicUsize,
    done: AtomicU64,
    total: AtomicU64,
    /// The steps that ended, kept here and not in the end of the job, so
    /// that a job that is cancelled or fails keeps what it finished.
    finished: Mutex<Vec<StepDone>>,
    /// What the preparation found, until the window takes it.
    prepared: Mutex<Option<Prepared>>,
}

impl Control {
    /// Keep how far the job is, and stop it when that was asked.
    pub(crate) fn report(&self, place: usize, done: u64, total: u64) -> Result<(), LoadError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(LoadError::Cancelled);
        }
        self.place.store(place, Ordering::Relaxed);
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
        Ok(())
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    pub(crate) fn cancelling(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    pub(crate) fn snapshot(&self) -> Progress {
        Progress {
            place: self.place.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            total: self.total.load(Ordering::Relaxed),
        }
    }

    fn finish(&self, step: StepDone) {
        self.finished
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(step);
    }

    /// The steps that ended so far, in their order.
    pub(crate) fn finished(&self) -> Vec<StepDone> {
        self.finished
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn keep_prepared(&self, prepared: Prepared) {
        *self.prepared.lock().unwrap_or_else(PoisonError::into_inner) = Some(prepared);
    }

    /// What the preparation found, once.
    pub(crate) fn take_prepared(&self) -> Option<Prepared> {
        self.prepared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn finished_count(&self) -> usize {
        self.finished
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

/// Everything a job was started with.
#[derive(Debug)]
pub(crate) struct JobInput {
    /// The steps, in the order they run.
    pub(crate) steps: Vec<WizardStep>,
    pub(crate) work: Work,
    /// Whether a step that ends is confirmed at once, as Run all
    /// automatically does, or waits for the user.
    pub(crate) confirm: bool,
    /// What the preparation reads, the scans as they were when the job
    /// started and the basis of the step on them.
    pub(crate) prepare: Option<PrepareInput>,
    pub(crate) sources: Vec<SourceRef>,
    pub(crate) prepare_basis: Option<u128>,
}

/// Count to `ticks` for a step, looking for a cancel at each tick.
fn stand_in(place: usize, ticks: u32, tick: Duration, control: &Control) -> Result<(), LoadError> {
    let total = u64::from(ticks);
    for count in 0..total {
        control.report(place, count, total)?;
        std::thread::sleep(tick);
    }
    control.report(place, total, total)
}

/// Run the steps of a job one after the other. This runs on a worker
/// thread.
pub(crate) fn run(input: &JobInput, control: &Control) -> Result<(), LoadError> {
    for (place, step) in input.steps.iter().enumerate() {
        let started = Instant::now();
        match (input.work, step) {
            (Work::Placeholder { ticks, tick }, _) => stand_in(place, ticks, tick, control)?,
            (Work::Steps, WizardStep::Prepare) => {
                let prepare = input
                    .prepare
                    .as_ref()
                    .ok_or_else(|| LoadError::InvalidData("there is no scan to prepare".into()))?;
                let report = |done: u64| control.report(place, done, 1000);
                let prepared = super::prepare::run(prepare, &report)?;
                control.keep_prepared(prepared);
            }
            (Work::Steps, _) => stand_in(place, STAND_IN_TICKS, STAND_IN_TICK, control)?,
        }
        control.finish(StepDone {
            step: *step,
            seconds: started.elapsed().as_secs_f64(),
        });
    }
    Ok(())
}

/// How a job ended, as the worker tells the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PipelineEnd {
    Done,
    Cancelled,
    Failed(String),
}

impl PipelineEnd {
    pub(crate) fn of(result: Result<(), LoadError>) -> Self {
        match result {
            Ok(()) => Self::Done,
            Err(LoadError::Cancelled) => Self::Cancelled,
            Err(error) => Self::Failed(plain_reason(&error.to_string()).to_owned()),
        }
    }
}

/// A job that is under way.
#[derive(Debug)]
pub(crate) struct PipelineJob {
    /// Tells this job from an earlier one whose answer is still on its way.
    pub(crate) serial: u64,
    pub(crate) input: Arc<JobInput>,
    pub(crate) control: Arc<Control>,
    started: Instant,
    /// The job of the local API that reports it.
    pub(crate) api_job_id: String,
    /// What the step under way was before the job started, to go back to
    /// when the job stops in it.
    before: Vec<StepStatus>,
}

impl PipelineJob {
    /// What a step of the job was before the job started.
    pub(crate) fn before(&self, step: WizardStep) -> Option<&StepStatus> {
        let place = self.input.steps.iter().position(|known| *known == step)?;
        self.before.get(place)
    }

    /// The step under way: the first that did not end yet.
    pub(crate) fn current(&self) -> Option<WizardStep> {
        self.input.steps.get(self.control.finished_count()).copied()
    }

    /// The line of the status bar while the job runs.
    fn status_text(&self) -> String {
        if self.control.cancelling() {
            return "Cancelling Pointcloud to Drawing…".into();
        }
        match self.current() {
            Some(step) => format!("Pointcloud to Drawing: {} {}…", step.number(), step.label()),
            None => "Pointcloud to Drawing…".into(),
        }
    }

    /// The step under way and how far it is, in words.
    fn detail(&self) -> String {
        let progress = self.control.snapshot();
        let count = self.input.steps.len();
        let place = self.control.finished_count().min(count.saturating_sub(1));
        let step = self.input.steps.get(place).copied().unwrap_or_default();
        let mut detail = format!(
            "Step {} of {count}  ·  {} {}",
            place + 1,
            step.number(),
            step.label()
        );
        if let Some(percent) = (progress.done.min(progress.total) * 100).checked_div(progress.total)
        {
            detail.push_str(&format!("  ·  {percent}%"));
        }
        detail
    }

    /// The job as `status` and `job` of the local API report it.
    pub(crate) fn progress_value(&self) -> Value {
        let progress = self.control.snapshot();
        json!({
            "state": "running",
            "operation": "mesh_to_plans",
            "steps": self.input.steps.iter().map(|step| step.id()).collect::<Vec<_>>(),
            "step": self.current().map(WizardStep::id),
            "place": progress.place,
            "completed": progress.done,
            "total": progress.total,
            "fraction": progress.fraction(),
            "confirm": self.input.confirm,
            "cancel_requested": self.control.cancelling(),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }
}

/// How the last job ended, for the local API and the status bar.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Last {
    Done(Vec<StepDone>),
    /// Cancelled after these steps had ended.
    Cancelled(Vec<StepDone>),
    /// A step failed after these steps had ended.
    Failed {
        finished: Vec<StepDone>,
        step: WizardStep,
        error: String,
    },
}

fn steps_value(steps: &[StepDone]) -> Value {
    steps
        .iter()
        .map(|done| json!({"id": done.step.id(), "seconds": done.seconds}))
        .collect()
}

impl Last {
    pub(crate) fn value(&self) -> Value {
        match self {
            Self::Done(finished) => json!({
                "state": "complete",
                "operation": "mesh_to_plans",
                "finished": steps_value(finished),
                "seconds": finished.iter().map(|done| done.seconds).sum::<f64>(),
            }),
            Self::Cancelled(finished) => json!({
                "state": "cancelled",
                "operation": "mesh_to_plans",
                "finished": steps_value(finished),
            }),
            Self::Failed {
                finished,
                step,
                error,
            } => json!({
                "state": "failed",
                "operation": "mesh_to_plans",
                "finished": steps_value(finished),
                "step": step.id(),
                "error": error,
            }),
        }
    }

    fn status(&self) -> String {
        let counted = |count: usize| match count {
            1 => "1 step".to_owned(),
            _ => format!("{count} steps"),
        };
        match self {
            Self::Done(finished) => format!(
                "Pointcloud to Drawing: {} done in {:.1} s",
                counted(finished.len()),
                finished.iter().map(|done| done.seconds).sum::<f64>()
            ),
            Self::Cancelled(finished) if finished.is_empty() => {
                "Pointcloud to Drawing cancelled".into()
            }
            Self::Cancelled(finished) => format!(
                "Pointcloud to Drawing cancelled after {}; those keep their result",
                counted(finished.len())
            ),
            Self::Failed { step, error, .. } => format!(
                "Pointcloud to Drawing failed in {} {}: {error}",
                step.number(),
                step.label()
            ),
        }
    }
}

impl Studio {
    /// The task under way that the wizard waits for before it starts a job:
    /// one heavy job at a time.
    fn heavy_work(&self) -> Option<&'static str> {
        if self.drawing.is_running() {
            Some("a section drawing")
        } else if self.closed_mesh.is_running() || self.mesh_job.is_some() {
            Some("a mesh")
        } else if self.faces.is_running() {
            Some("a face detection")
        } else if self.merge_job.is_some() {
            Some("a merge")
        } else if self.index_pending() {
            Some("an octree")
        } else {
            None
        }
    }

    fn mesh_to_plans_poll_task() -> Task<Message> {
        Task::perform(async { tokio::time::sleep(POLL).await }, |()| {
            Message::MeshToPlans(WizardAction::Poll)
        })
    }

    /// Start a job for these steps on a worker thread, unless one runs or
    /// another heavy task does. The window reads how far it is until
    /// `WizardAction::Finished` arrives.
    pub(crate) fn start_mesh_to_plans_job(
        &mut self,
        steps: Vec<WizardStep>,
        confirm: bool,
    ) -> Task<Message> {
        if self.mesh_to_plans.job.is_some() {
            self.status = "Pointcloud to Drawing is already running a step".into();
            return Task::none();
        }
        if let Some(task) = self.heavy_work() {
            self.status = format!(
                "Pointcloud to Drawing waits: {task} is being made; wait for it or cancel it first"
            );
            return Task::none();
        }
        let Some(first) = steps.first().copied() else {
            self.status = "Every step of Pointcloud to Drawing is confirmed or skipped".into();
            return Task::none();
        };
        let work = self.mesh_to_plans.work;
        let (mut prepare, mut sources, mut prepare_basis) = (None, Vec::new(), None);
        if work == Work::Steps && steps.contains(&WizardStep::Prepare) {
            self.default_project_place();
            // What step 0 writes never goes over another project.
            if let Some(taken) = self.folder_taken() {
                self.status = format!(
                    "Pointcloud to Drawing cannot prepare: {}",
                    taken.translated()
                );
                return Task::none();
            }
            match self.prepare_input(self.project_folder()) {
                Ok(input) => {
                    let filter = input.scene.filter;
                    sources = self
                        .clouds
                        .iter()
                        .filter(|entry| {
                            input
                                .scene
                                .layers
                                .iter()
                                .any(|layer| Arc::ptr_eq(&layer.identity, &entry.load_identity))
                        })
                        .map(|entry| SourceRef::of(entry, &filter))
                        .collect();
                    prepare_basis = Some(super::project::prepare_basis(
                        &sources,
                        &self.mesh_to_plans.prepare.regions,
                    ));
                    prepare = Some(input);
                }
                Err(reason) => {
                    let reason = reason.translated();
                    self.status = format!("Pointcloud to Drawing cannot prepare: {reason}");
                    self.mesh_to_plans
                        .set_status(WizardStep::Prepare, StepStatus::Failed(reason));
                    return Task::none();
                }
            }
        }
        let input = Arc::new(JobInput {
            steps,
            work,
            confirm,
            prepare,
            sources,
            prepare_basis,
        });
        let control = Arc::new(Control::default());
        let serial = self.mesh_to_plans.next_serial;
        self.mesh_to_plans.next_serial += 1;
        let before = input
            .steps
            .iter()
            .map(|step| self.mesh_to_plans.status(*step).clone())
            .collect();
        let mut job = PipelineJob {
            serial,
            input: Arc::clone(&input),
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id: String::new(),
            before,
        };
        job.api_job_id = self.record_api_job(job.progress_value());
        self.status = job.status_text();
        self.mesh_to_plans.set_status(first, StepStatus::Running);
        self.mesh_to_plans.job = Some(job);
        // The result of an earlier job would read as the result of this one.
        self.mesh_to_plans.last = None;
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || PipelineEnd::of(run(&input, &control)))
                    .await
                    .unwrap_or_else(|error| PipelineEnd::Failed(error.to_string()))
            },
            move |end| Message::MeshToPlans(WizardAction::Finished(serial, end)),
        );
        Task::batch([worker, Self::mesh_to_plans_poll_task()])
    }

    /// Mark the steps a job finished, and the step under way as running,
    /// and take what the preparation found. Returns whether a step ended
    /// since the last look, so that the project is written.
    fn mesh_to_plans_follow(&mut self) -> bool {
        let Some(job) = &self.mesh_to_plans.job else {
            return false;
        };
        let finished = job.control.finished();
        let confirm = job.input.confirm;
        let current = job.input.steps.get(finished.len()).copied();
        let prepared = job.control.take_prepared();
        let (sources, basis) = (job.input.sources.clone(), job.input.prepare_basis);
        let mut ended = false;
        for done in &finished {
            let status = if confirm {
                StepStatus::Confirmed
            } else {
                StepStatus::Done
            };
            let wizard = &mut self.mesh_to_plans;
            if *wizard.status(done.step) == StepStatus::Running {
                ended = true;
                if done.step == WizardStep::Prepare {
                    // Run again on the scans as they are: up to date.
                    wizard.stale_from = None;
                    wizard.watched = None;
                }
                wizard.runs[done.step.place()] = StepRun {
                    basis: if done.step == WizardStep::Prepare {
                        basis
                    } else {
                        None
                    },
                    finished: Some(now_seconds()),
                    seconds: Some(done.seconds),
                };
            }
            wizard.set_status(done.step, status);
        }
        if let Some(step) = current {
            self.mesh_to_plans.set_status(step, StepStatus::Running);
        }
        if let Some(prepared) = prepared {
            self.mesh_to_plans.prepare.take(&prepared);
            self.mesh_to_plans.sources = sources;
            if let Some(error) = &prepared.write_error {
                self.status = format!("The survey could not be written: {error}");
            }
            ended = true;
        }
        ended
    }

    /// Four times a second while a job runs: its steps in the sidebar, the
    /// status bar and the job of the local API.
    pub(crate) fn mesh_to_plans_poll(&mut self) -> Task<Message> {
        let ended = self.mesh_to_plans_follow();
        let save = if ended {
            self.queue_project_save()
        } else {
            Task::none()
        };
        let Some(job) = &self.mesh_to_plans.job else {
            return save;
        };
        let text = job.status_text();
        let value = job.progress_value();
        if let Some(entry) = self.api_jobs.get_mut(&job.api_job_id) {
            *entry = value;
        }
        if self.status != text {
            self.status = text;
        }
        Task::batch([save, Self::mesh_to_plans_poll_task()])
    }

    /// Before the window closes: take the steps the worker finished since
    /// the last look, and write the project now when a change waits.
    pub(crate) fn flush_mesh_to_plans(&mut self) {
        if self.mesh_to_plans_follow() {
            let _ = self.queue_project_save();
        }
        if self.mesh_to_plans.save_waiting {
            let _ = self.save_project_now();
        }
    }

    /// Ask the worker of a running job to stop; Exit does too.
    pub(crate) fn cancel_mesh_to_plans(&mut self) {
        if let Some(job) = &self.mesh_to_plans.job {
            job.control.cancel();
            self.status = job.status_text();
        }
    }

    /// A job ended: its finished steps keep their result, the step it
    /// stopped in goes back to what it was or is marked failed, and the job
    /// of the local API says how it went.
    pub(crate) fn mesh_to_plans_finished(
        &mut self,
        serial: u64,
        end: PipelineEnd,
    ) -> Task<Message> {
        if self
            .mesh_to_plans
            .job
            .as_ref()
            .is_none_or(|job| job.serial != serial)
        {
            return Task::none();
        }
        self.mesh_to_plans_follow();
        let Some(job) = self.mesh_to_plans.job.take() else {
            return Task::none();
        };
        let finished = job.control.finished();
        let stopped = job.input.steps.get(finished.len()).copied();
        let last = match end {
            PipelineEnd::Done => Last::Done(finished),
            PipelineEnd::Cancelled => Last::Cancelled(finished),
            PipelineEnd::Failed(error) => Last::Failed {
                finished,
                step: stopped.unwrap_or_default(),
                error,
            },
        };
        // The step the job stopped in goes back to what it was, or is
        // marked failed; the steps it did not reach keep what they had.
        for (step, before) in job.input.steps.iter().zip(&job.before) {
            if *self.mesh_to_plans.status(*step) == StepStatus::Running {
                self.mesh_to_plans.set_status(*step, before.clone());
            }
        }
        if let (Last::Failed { step, error, .. }, Some(_)) = (&last, stopped) {
            self.mesh_to_plans
                .set_status(*step, StepStatus::Failed(error.clone()));
        }
        if let Some(entry) = self.api_jobs.get_mut(&job.api_job_id) {
            *entry = last.value();
        }
        self.status = last.status();
        self.mesh_to_plans.last = Some(last);
        self.mesh_to_plans.last_job_id = Some(job.api_job_id);
        // Every job ends with the project written, also one that failed.
        self.queue_project_save()
    }

    /// The line of the strip above the scene while a job runs.
    pub(crate) fn mesh_to_plans_progress_line(&self) -> Option<Line> {
        let job = self.mesh_to_plans.job.as_ref()?;
        let cancelling = job.control.cancelling();
        Some(Line {
            phase: Phase::MeshToPlans,
            title: if cancelling {
                "Cancelling…".to_owned()
            } else {
                "Pointcloud to Drawing".to_owned()
            },
            detail: job.detail(),
            fraction: job.control.snapshot().fraction(),
            timed: true,
            cancel: (!cancelling).then_some(Message::MeshToPlans(WizardAction::Cancel)),
        })
    }
}
