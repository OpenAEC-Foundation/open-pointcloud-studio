//! The worker of the Mesh to Plans wizard. One job at a time runs the steps
//! it was given one after the other on a thread of its own, so that the
//! machine is not swamped and the window stays free to look at other steps.
//! The window reads how far it is four times a second, and a cancel stops it
//! at the next look the worker takes, well within one of those.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iced::Task;
use pointcloud_core::LoadError;
use serde_json::{json, Value};

use super::{StepStatus, WizardAction, WizardStep};
use crate::bag_panel::plain_reason;
use crate::open_progress::{Line, Phase};
use crate::{Message, Studio};

/// How often the window reads how far a job is.
pub(crate) const POLL: Duration = Duration::from_millis(250);

/// What the steps of a job do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Work {
    /// Stands in for the work of the steps until they are built: it counts
    /// to `ticks`, one tick at a time, and looks for a cancel at each tick.
    Placeholder { ticks: u32, tick: Duration },
}

/// The work of a step while the steps compute nothing yet: a second each.
pub(crate) const PLACEHOLDER: Work = Work::Placeholder {
    ticks: 20,
    tick: Duration::from_millis(50),
};

impl Default for Work {
    fn default() -> Self {
        PLACEHOLDER
    }
}

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
}

/// Run the steps of a job one after the other. This runs on a worker
/// thread.
pub(crate) fn run(input: &JobInput, control: &Control) -> Result<(), LoadError> {
    for (place, step) in input.steps.iter().enumerate() {
        let started = Instant::now();
        match input.work {
            Work::Placeholder { ticks, tick } => {
                let total = u64::from(ticks);
                for count in 0..total {
                    control.report(place, count, total)?;
                    std::thread::sleep(tick);
                }
                control.report(place, total, total)?;
            }
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
    /// The step under way: the first that did not end yet.
    pub(crate) fn current(&self) -> Option<WizardStep> {
        self.input.steps.get(self.control.finished_count()).copied()
    }

    /// The line of the status bar while the job runs.
    fn status_text(&self) -> String {
        if self.control.cancelling() {
            return "Cancelling Mesh to Plans…".into();
        }
        match self.current() {
            Some(step) => format!("Mesh to Plans: {} {}…", step.number(), step.label()),
            None => "Mesh to Plans…".into(),
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
        if progress.total > 0 {
            detail.push_str(&format!(
                "  ·  {} of {}",
                progress.done.min(progress.total),
                progress.total
            ));
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
                "Mesh to Plans: {} done in {:.1} s",
                counted(finished.len()),
                finished.iter().map(|done| done.seconds).sum::<f64>()
            ),
            Self::Cancelled(finished) if finished.is_empty() => "Mesh to Plans cancelled".into(),
            Self::Cancelled(finished) => format!(
                "Mesh to Plans cancelled after {}; those keep their result",
                counted(finished.len())
            ),
            Self::Failed { step, error, .. } => format!(
                "Mesh to Plans failed in {} {}: {error}",
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
        } else if self.index_pending {
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
            self.status = "Mesh to Plans is already running a step".into();
            return Task::none();
        }
        if let Some(task) = self.heavy_work() {
            self.status = format!(
                "Mesh to Plans waits: {task} is being made; wait for it or cancel it first"
            );
            return Task::none();
        }
        let Some(first) = steps.first().copied() else {
            self.status = "Every step of Mesh to Plans is confirmed or skipped".into();
            return Task::none();
        };
        let input = Arc::new(JobInput {
            steps,
            work: self.mesh_to_plans.work,
            confirm,
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

    /// Mark the steps a job finished, and the step under way as running.
    fn mesh_to_plans_follow(&mut self) {
        let Some(job) = &self.mesh_to_plans.job else {
            return;
        };
        let finished = job.control.finished();
        let confirm = job.input.confirm;
        let current = job.input.steps.get(finished.len()).copied();
        for done in finished {
            let status = if confirm {
                StepStatus::Confirmed
            } else {
                StepStatus::Done
            };
            self.mesh_to_plans.set_status(done.step, status);
        }
        if let Some(step) = current {
            self.mesh_to_plans.set_status(step, StepStatus::Running);
        }
    }

    /// Four times a second while a job runs: its steps in the sidebar, the
    /// status bar and the job of the local API.
    pub(crate) fn mesh_to_plans_poll(&mut self) -> Task<Message> {
        self.mesh_to_plans_follow();
        let Some(job) = &self.mesh_to_plans.job else {
            return Task::none();
        };
        let text = job.status_text();
        let value = job.progress_value();
        if let Some(entry) = self.api_jobs.get_mut(&job.api_job_id) {
            *entry = value;
        }
        if self.status != text {
            self.status = text;
        }
        Self::mesh_to_plans_poll_task()
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
    pub(crate) fn mesh_to_plans_finished(&mut self, serial: u64, end: PipelineEnd) {
        if self
            .mesh_to_plans
            .job
            .as_ref()
            .is_none_or(|job| job.serial != serial)
        {
            return;
        }
        self.mesh_to_plans_follow();
        let Some(job) = self.mesh_to_plans.job.take() else {
            return;
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
                "Mesh to Plans".to_owned()
            },
            detail: job.detail(),
            fraction: job.control.snapshot().fraction(),
            timed: true,
            cancel: (!cancelling).then_some(Message::MeshToPlans(WizardAction::Cancel)),
        })
    }
}
