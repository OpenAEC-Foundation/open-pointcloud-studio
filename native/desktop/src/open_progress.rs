//! What the window tells about scans that are still being opened or indexed
//! and about a section drawing, a closed mesh, a face detection or a step of
//! Pointcloud to Drawing that is under way: a line per task with how far it is and
//! how long it will still take.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use iced::widget::{column, container, row, text};
use iced::{Element, Fill};
use pointcloud_core::IndexStage;

use crate::index_jobs::IndexJob;
use crate::ui_style;
use crate::{compact_count, display_name, i18n, ui_theme, CloudEntry, Message, Studio};

/// A task is timed from when it was first seen; a shorter time or a smaller
/// advance than these says nothing yet about how long the rest will take.
const MIN_TIMED: Duration = Duration::from_secs(3);
const MIN_ADVANCE: f32 = 0.01;

/// The height of the title line of a task, and of one with Cancel: the
/// height of a button of the style book.
const TITLE_LINE: f32 = 20.0;
const CANCEL_LINE: f32 = 27.0;

/// The tasks that can be under way at once, each timed on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Imports that read their source without building an octree.
    Opening,
    /// The source pass of an import or index build.
    Reading,
    /// Partitioning the octree on disk.
    Building,
    /// Reading, tracing and writing a section drawing or its preview.
    Drawing,
    /// The stages of a closed mesh; the bar starts again with each stage.
    ClosedMesh,
    /// The stages of a face detection; the bar starts again with each stage.
    Faces,
    /// The steps of a Pointcloud to Drawing job; the bar starts again with each step.
    MeshToPlans,
    /// Several octree builds, side by side or waiting for a place.
    Indexing,
    /// Colouring points from photos; the bar starts again with each part.
    PhotoColours,
}

/// When a task was first seen, and how far it was then.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mark {
    since: Instant,
    fraction: f32,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub phase: Phase,
    pub title: String,
    pub detail: String,
    /// How far the task is, when its size is known.
    pub fraction: Option<f32>,
    /// Whether the pace so far says how long the rest takes.
    pub timed: bool,
    pub cancel: Option<Message>,
}

/// What the row of a scan says while its octree waits for a place.
pub(crate) const INDEX_QUEUED: &str = i18n::key("index queued");

fn fraction(done: u64, total: u64) -> Option<f32> {
    (total > 0).then(|| (done as f64 / total as f64).min(1.0) as f32)
}

/// Time the rest of a task takes at the pace since it was first seen.
fn time_left(mark: Mark, fraction: f32, now: Instant) -> Option<Duration> {
    let elapsed = now.checked_duration_since(mark.since)?;
    let advanced = fraction - mark.fraction;
    if elapsed < MIN_TIMED || advanced < MIN_ADVANCE || fraction >= 1.0 {
        return None;
    }
    Some(elapsed.mul_f64(f64::from((1.0 - fraction) / advanced)))
}

fn time_text(left: Duration) -> String {
    let seconds = left.as_secs();
    if seconds < 55 {
        let seconds = seconds.div_ceil(5).max(1) * 5;
        i18n::tr_args("about {seconds} s left", &[("seconds", &seconds)])
    } else {
        let minutes = (seconds + 30) / 60;
        i18n::tr_args("about {minutes} min left", &[("minutes", &minutes)])
    }
}

/// How far a build is as a whole: reading its source is the first half,
/// building the tree the second.
fn build_fraction(job: &IndexJob) -> f32 {
    let Some(progress) = job.progress() else {
        return 0.0;
    };
    let stage = progress.fraction().unwrap_or(0.0);
    match progress.stage {
        IndexStage::WaitingToRead => 0.0,
        IndexStage::ReadingSource => stage * 0.5,
        IndexStage::WaitingToBuild => 0.5,
        IndexStage::BuildingTree => 0.5 + stage * 0.5,
        IndexStage::Ready => 1.0,
    }
}

fn points_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => i18n::tr_args(
            "{done} of {total} points",
            &[
                ("done", &compact_count(done.min(total))),
                ("total", &compact_count(total)),
            ],
        ),
        None => i18n::tr_args("{count} points read", &[("count", &compact_count(done))]),
    }
}

impl Studio {
    /// The tasks that are opening or indexing scans right now.
    pub(crate) fn progress_lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        // Imports that build an octree in the same pass are told with the
        // builds.
        let plain: Vec<_> = self
            .imports
            .iter()
            .filter(|(_, job)| !self.import_builds_index(&job.cancel))
            .collect();
        if !plain.is_empty() {
            let files = self.opening_total.max(plain.len());
            let done = files - plain.len();
            let read: u64 = plain
                .iter()
                .map(|(_, job)| job.decoded.load(Ordering::Relaxed))
                .sum();
            let expected: Option<u64> =
                plain.iter().map(|(id, _)| self.expected_points(**id)).sum();
            // Scans that are done count in full, the others as far as they are.
            let reading: f32 = plain
                .iter()
                .filter_map(|(id, job)| {
                    fraction(
                        job.decoded.load(Ordering::Relaxed),
                        self.expected_points(**id)?,
                    )
                })
                .sum();
            let cancelling = plain
                .iter()
                .all(|(_, job)| job.cancel.load(Ordering::Relaxed));
            // Large scans that wait for their turn to be read.
            let waiting = plain
                .iter()
                .filter(|(_, job)| job.waiting.load(Ordering::Relaxed))
                .count();
            let mut detail = points_text(read, expected);
            let title = if cancelling {
                i18n::tr("Cancelling…").to_owned()
            } else if files > 1 {
                let counted = i18n::tr_args(
                    "{done} of {files} done",
                    &[("done", &done), ("files", &files)],
                );
                detail = if waiting > 0 {
                    format!(
                        "{counted}  ·  {}  ·  {detail}",
                        i18n::tr_args("{waiting} waiting to be read", &[("waiting", &waiting)])
                    )
                } else {
                    format!("{counted}  ·  {detail}")
                };
                i18n::tr_args("Opening {count} scans", &[("count", &files)])
            } else {
                if waiting > 0 {
                    detail =
                        i18n::tr("Waiting for the scan opened before it on this disk").to_owned();
                }
                i18n::tr_args(
                    "Opening {name}",
                    &[("name", &display_name(&plain[0].1.path))],
                )
            };
            lines.push(Line {
                phase: Phase::Opening,
                title,
                detail,
                // A scan that waits for its turn has no pace yet.
                fraction: (files > 1 || (expected.is_some() && waiting == 0))
                    .then(|| ((done as f32 + reading) / files as f32).min(1.0)),
                // Scans of different sizes count alike in the bar of a
                // batch, so its pace says little about the time left.
                timed: files == 1,
                cancel: (!cancelling).then_some(Message::CancelOpening),
            });
        }

        // A one-pass import whose octree was cancelled is told with the
        // plain imports above.
        let waiting = self.index_waiting();
        match self.tree_builds().as_slice() {
            [] => {}
            [job] if waiting == 0 => lines.extend(self.index_line(job)),
            jobs => lines.push(self.index_batch_line(jobs, waiting)),
        }
        lines.extend(self.drawing.progress_line());
        lines.extend(self.closed_mesh.progress_line());
        lines.extend(self.faces.progress_line());
        lines.extend(self.photo_colours.progress_line());
        lines.extend(self.mesh_to_plans_progress_line());
        lines
    }

    /// The line of the one octree build that runs: its two steps.
    fn index_line(&self, job: &IndexJob) -> Option<Line> {
        let progress = job.progress()?;
        // An import reads and indexes in one go; an open scan only gets its
        // octree.
        let import = job.import_id.filter(|id| self.imports.contains_key(id));
        let name = match import.and_then(|id| self.imports.get(&id)) {
            Some(import) => display_name(&import.path),
            None => display_name(&job.path),
        };
        let title = if job.import_id.is_some() {
            i18n::tr_args("Opening {name}", &[("name", &name)])
        } else {
            i18n::tr_args("Indexing {name}", &[("name", &name)])
        };
        let cancelling = job.cancelling();
        let (phase, detail, fraction) = match progress.stage {
            // A large scan waits for the large scans opened before it.
            IndexStage::WaitingToRead => (
                Phase::Reading,
                i18n::tr("Step 1 of 2  ·  waiting for the scan before it on this disk").to_owned(),
                None,
            ),
            IndexStage::WaitingToBuild => (
                Phase::Building,
                i18n::tr("Step 2 of 2  ·  waiting for the octree before it").to_owned(),
                None,
            ),
            // Nothing read yet: the source is being opened, or an octree
            // kept from an earlier session is being attached.
            IndexStage::ReadingSource if progress.completed == 0 => {
                (Phase::Reading, i18n::tr("Preparing…").to_owned(), None)
            }
            IndexStage::ReadingSource => (
                Phase::Reading,
                i18n::tr_args(
                    "Step 1 of 2  ·  reading  ·  {points}",
                    &[(
                        "points",
                        &points_text(
                            progress.completed,
                            (progress.total > 0).then_some(progress.total),
                        ),
                    )],
                ),
                progress.fraction(),
            ),
            IndexStage::BuildingTree | IndexStage::Ready => (
                Phase::Building,
                i18n::tr_args(
                    "Step 2 of 2  ·  building the octree  ·  {placed} of {total} points placed",
                    &[
                        (
                            "placed",
                            &compact_count(progress.settled.min(progress.total)),
                        ),
                        ("total", &compact_count(progress.total)),
                    ],
                ),
                progress.fraction(),
            ),
        };
        Some(Line {
            phase,
            title: if cancelling {
                i18n::tr("Cancelling…").to_owned()
            } else {
                title
            },
            detail,
            fraction,
            timed: true,
            cancel: (!cancelling).then(|| match import {
                Some(id) => Message::CancelImport(id),
                None => Message::CancelIndex,
            }),
        })
    }

    /// The line of builds that run side by side or wait for a place: how
    /// many there are, how many are ready and how far the rest are.
    fn index_batch_line(&self, jobs: &[&IndexJob], queued: usize) -> Line {
        let count = self.index_finished + jobs.len() + queued;
        let ready = self.index_finished;
        let running: f32 = jobs.iter().map(|job| build_fraction(job)).sum();
        let cancelling = jobs.iter().all(|job| job.cancelling());
        // Builds that wait for their turn are told with the queue.
        let turns = jobs.iter().filter(|job| job.waits_for_turn()).count();
        let waiting = queued + turns;
        let values: [(&str, &dyn std::fmt::Display); 4] = [
            ("ready", &ready),
            ("count", &count),
            ("running", &(jobs.len() - turns)),
            ("waiting", &waiting),
        ];
        let detail = match (jobs.len() - turns, waiting) {
            (_, 0) => i18n::tr_args("{ready} of {count} ready  ·  {running} at once", &values),
            (0, _) => i18n::tr_args("{ready} of {count} ready  ·  {waiting} waiting", &values),
            (1, _) => i18n::tr_args(
                "{ready} of {count} ready  ·  1 building  ·  {waiting} waiting",
                &values,
            ),
            _ => i18n::tr_args(
                "{ready} of {count} ready  ·  {running} at once  ·  {waiting} waiting",
                &values,
            ),
        };
        Line {
            phase: Phase::Indexing,
            title: if cancelling {
                i18n::tr("Cancelling…").to_owned()
            } else {
                i18n::tr_args("Indexing {count} scans", &[("count", &count)])
            },
            detail,
            fraction: Some(((ready as f32 + running) / count.max(1) as f32).min(1.0)),
            // Builds of different sizes side by side say little about the
            // time the rest takes.
            timed: false,
            cancel: (!cancelling).then_some(Message::CancelIndex),
        }
    }

    /// Remember when each task started, for the time it still takes, and
    /// forget what belonged to tasks that ended.
    pub(crate) fn track_progress(&mut self) {
        if self.index_jobs.is_empty() && self.index_waiting() == 0 {
            self.index_finished = 0;
        }
        if self.imports.is_empty() {
            self.opening_total = 0;
            self.import_expected.clear();
            if !self.index_pending()
                && !self.drawing.is_running()
                && !self.closed_mesh.is_running()
                && !self.faces.is_running()
                && !self.photo_colours.is_running()
                && !self.mesh_to_plans.is_running()
            {
                self.progress_marks.clear();
                return;
            }
        }
        let now = Instant::now();
        let lines = self.progress_lines();
        self.progress_marks
            .retain(|phase, _| lines.iter().any(|line| line.phase == *phase));
        for line in &lines {
            let Some(fraction) = line.fraction.filter(|fraction| *fraction > 0.0) else {
                continue;
            };
            let mark = self.progress_marks.entry(line.phase).or_insert(Mark {
                since: now,
                fraction,
            });
            // A bar that went back belongs to the next task of its kind.
            if fraction < mark.fraction {
                *mark = Mark {
                    since: now,
                    fraction,
                };
            }
        }
    }

    /// What a row of the project list says about its scan while it is being
    /// opened or indexed, and how far that is when known.
    pub(crate) fn layer_progress(&self, entry: &CloudEntry) -> Option<(String, Option<f32>)> {
        let percent = |label: &str, fraction: Option<f32>| {
            let text = match fraction {
                Some(fraction) => format!("{label} {:.0}%", (fraction * 100.0).floor()),
                None => format!("{label}…"),
            };
            Some((text, fraction))
        };
        if let Some(job) = self.index_job_of(entry) {
            return match job.progress().map(|progress| (progress.stage, progress)) {
                Some((IndexStage::WaitingToRead, _)) => percent(i18n::tr("waiting to read"), None),
                Some((IndexStage::ReadingSource, progress)) => {
                    percent(i18n::tr("reading"), progress.fraction())
                }
                Some((IndexStage::WaitingToBuild, _)) => {
                    percent(i18n::tr("waiting to index"), None)
                }
                Some((_, progress)) => percent(i18n::tr("indexing"), progress.fraction()),
                None => percent(i18n::tr("indexing"), None),
            };
        }
        if entry.index_building || entry.index_import_id.is_some() {
            return percent(i18n::tr("indexing"), None);
        }
        let import = self
            .import_headers
            .iter()
            .find(|(_, header)| entry.matches_source(header))
            .map(|(id, _)| *id);
        if let Some(job) = import.and_then(|id| self.imports.get(&id)) {
            if job.waiting.load(Ordering::Relaxed) {
                return percent(i18n::tr("waiting to read"), None);
            }
            let expected = import.and_then(|id| self.import_expected.get(&id));
            return percent(
                i18n::tr("reading"),
                expected
                    .and_then(|expected| fraction(job.decoded.load(Ordering::Relaxed), *expected)),
            );
        }
        if entry.cloud.points.is_empty() && entry.cloud.total_points > 0 && entry.mesh.is_none() {
            return percent(i18n::tr("loading points"), None);
        }
        self.index_queued(entry)
            .then(|| (i18n::tr(INDEX_QUEUED).to_owned(), None))
    }

    /// The strip above the scene with a line per task.
    pub(crate) fn progress_strip(&self) -> Option<Element<'_, Message>> {
        let lines = self.progress_lines();
        if lines.is_empty() {
            return None;
        }
        let now = Instant::now();
        // The strip lies on the scene, which is white in the light theme.
        let colors = self.ui_theme.colors();
        let mut strip = column![].spacing(7);
        for line in lines {
            // How far and how long still, beside the title.
            let mut pace = String::new();
            if let Some(fraction) = line.fraction {
                pace = format!("{:.0}%", (fraction * 100.0).floor());
                let left = self
                    .progress_marks
                    .get(&line.phase)
                    .filter(|_| line.timed)
                    .and_then(|mark| time_left(*mark, fraction, now));
                if let Some(left) = left {
                    pace.push_str("  ·  ");
                    pace.push_str(&time_text(left));
                }
            }
            let mut heading = row![
                text(line.title)
                    .size(12)
                    .color(colors.dom.scene_text)
                    .height(16)
                    .width(Fill),
                text(pace).size(11).color(colors.dom.scene_text),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center);
            let mut heading_height = TITLE_LINE;
            if let Some(cancel) = line.cancel {
                heading = heading
                    .push(ui_style::secondary_button(crate::i18n::tr("Cancel")).on_press(cancel));
                heading_height = CANCEL_LINE;
            }
            // One line each: a narrow scene cuts the text off instead of
            // pushing the scene down.
            let mut task = column![
                container(heading).height(heading_height).clip(true),
                container(text(line.detail).size(11).color(colors.dom.scene_muted))
                    .height(15)
                    .width(Fill)
                    .clip(true),
            ]
            .spacing(3);
            if let Some(fraction) = line.fraction {
                task = task.push(ui_style::progress_bar(0.0..=1.0, fraction));
            }
            strip = strip.push(task);
        }
        Some(
            container(strip)
                .padding(iced::Padding {
                    top: 2.0,
                    right: 14.0,
                    bottom: 9.0,
                    left: 14.0,
                })
                .width(Fill)
                .style(|theme| container::Style::default().color(ui_theme::colors(theme).text))
                .into(),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64};
    use std::sync::{Arc, Mutex};

    use pointcloud_core::{IndexProgress, PointCloud};

    use super::*;
    use crate::i18n::{Language, TestLanguage};
    use crate::ImportJob;

    fn index_job(
        source: Option<Arc<PointCloud>>,
        import_id: Option<u64>,
        path: &str,
        progress: &Arc<Mutex<IndexProgress>>,
        cancel: &Arc<AtomicBool>,
    ) -> IndexJob {
        IndexJob {
            source,
            import_id,
            path: PathBuf::from(path),
            progress: Arc::clone(progress),
            cancel: Arc::clone(cancel),
            stop_tree: Arc::new(AtomicBool::new(false)),
        }
    }

    fn reading(completed: u64, total: u64) -> IndexProgress {
        IndexProgress {
            stage: IndexStage::ReadingSource,
            completed,
            total,
            depth: 0,
            leaves: 0,
            settled: 0,
        }
    }

    fn job(name: &str, decoded: u64) -> ImportJob {
        ImportJob {
            path: PathBuf::from(name),
            decoded: Arc::new(AtomicU64::new(decoded)),
            cancel: Arc::new(AtomicBool::new(false)),
            waiting: Default::default(),
        }
    }

    #[test]
    fn time_left_follows_the_pace_since_the_task_was_first_seen() {
        let _language = TestLanguage::hold(Language::English);
        let start = Instant::now();
        let mark = Mark {
            since: start,
            fraction: 0.2,
        };
        // Too early, or too little advance, says nothing.
        assert_eq!(time_left(mark, 0.5, start + Duration::from_secs(2)), None);
        assert_eq!(time_left(mark, 0.205, start + Duration::from_secs(9)), None);
        // 30% in 10 s leaves 50% for about 17 s.
        let left = time_left(mark, 0.5, start + Duration::from_secs(10)).unwrap();
        assert!((left.as_secs_f64() - 16.667).abs() < 0.01);
        assert_eq!(time_left(mark, 1.0, start + Duration::from_secs(10)), None);

        assert_eq!(time_text(Duration::from_secs(1)), "about 5 s left");
        assert_eq!(time_text(Duration::from_secs(17)), "about 20 s left");
        assert_eq!(time_text(Duration::from_secs(54)), "about 55 s left");
        assert_eq!(time_text(Duration::from_secs(55)), "about 1 min left");
        assert_eq!(time_text(Duration::from_secs(200)), "about 3 min left");

        assert_eq!(fraction(5, 0), None);
        assert_eq!(fraction(5, 10), Some(0.5));
        assert_eq!(fraction(15, 10), Some(1.0));
    }

    #[test]
    fn imports_report_how_many_scans_are_ready_and_how_far_the_rest_is() {
        let _language = TestLanguage::hold(Language::English);
        let mut studio = Studio::default();
        assert!(studio.progress_lines().is_empty());

        // One scan whose size is not known yet.
        studio.imports.insert(1, job("first.e57", 2_500_000));
        studio.opening_total = 1;
        let lines = studio.progress_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].title, "Opening first.e57");
        assert_eq!(lines[0].detail, "2.5M points read");
        assert_eq!(lines[0].fraction, None);
        assert!(matches!(lines[0].cancel, Some(Message::CancelOpening)));

        // Its metadata states ten million points.
        studio.import_expected.insert(1, 10_000_000);
        let lines = studio.progress_lines();
        assert_eq!(lines[0].detail, "2.5M of 10.0M points");
        assert_eq!(lines[0].fraction, Some(0.25));

        // Four scans were opened together; two are ready, one is halfway and
        // one has not stated its size.
        studio.imports.insert(2, job("second.e57", 0));
        studio
            .imports
            .get(&1)
            .unwrap()
            .decoded
            .store(5_000_000, Ordering::Relaxed);
        studio.opening_total = 4;
        let lines = studio.progress_lines();
        assert_eq!(lines[0].title, "Opening 4 scans");
        assert_eq!(lines[0].detail, "2 of 4 done  ·  5.0M points read");
        assert_eq!(lines[0].fraction, Some(0.625));
        assert!(!lines[0].timed);

        // Cancelling stops both scans that are still being read.
        let _ = studio.update(Message::CancelOpening);
        assert!(studio
            .imports
            .values()
            .all(|job| job.cancel.load(Ordering::Relaxed)));
        assert_eq!(studio.progress_lines()[0].title, "Cancelling…");
        for job in studio.imports.values() {
            job.cancel.store(false, Ordering::Relaxed);
        }

        // Tracking starts the clock and forgets it when the imports end.
        studio.track_progress();
        assert!(studio.progress_marks.contains_key(&Phase::Opening));
        studio.imports.clear();
        studio.track_progress();
        assert!(studio.progress_marks.is_empty());
        assert_eq!(studio.opening_total, 0);
        assert!(studio.import_expected.is_empty());
        assert!(studio.progress_strip().is_none());
    }

    #[test]
    fn an_indexed_import_reports_its_two_steps() {
        let _language = TestLanguage::hold(Language::English);
        let mut studio = Studio::default();
        let import = job("merged.e57", 0);
        let progress = Arc::new(Mutex::new(reading(100_000_000, 400_000_000)));
        studio.index_jobs.push(index_job(
            None,
            Some(7),
            "merged.e57",
            &progress,
            &import.cancel,
        ));
        studio.imports.insert(7, import);

        // The import that builds the octree has its own line, not the one of
        // plain imports.
        let lines = studio.progress_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].phase, Phase::Reading);
        assert_eq!(lines[0].title, "Opening merged.e57");
        assert_eq!(
            lines[0].detail,
            "Step 1 of 2  ·  reading  ·  100.0M of 400.0M points"
        );
        assert_eq!(lines[0].fraction, Some(0.25));
        assert!(matches!(lines[0].cancel, Some(Message::CancelImport(7))));
        assert!(studio.progress_strip().is_some());

        *progress.lock().unwrap() = IndexProgress {
            stage: IndexStage::BuildingTree,
            completed: 900_000_000,
            total: 400_000_000,
            depth: 3,
            leaves: 120,
            settled: 300_000_000,
        };
        studio.imports.clear();
        let lines = studio.progress_lines();
        assert_eq!(lines[0].phase, Phase::Building);
        assert_eq!(
            lines[0].detail,
            "Step 2 of 2  ·  building the octree  ·  300.0M of 400.0M points placed"
        );
        // The root has been split and three quarters of the points placed.
        assert_eq!(lines[0].fraction, Some(0.8));
        assert!(matches!(lines[0].cancel, Some(Message::CancelIndex)));

        // The next octree starts its own clock.
        studio.track_progress();
        let first = studio.progress_marks[&Phase::Building];
        *progress.lock().unwrap() = IndexProgress {
            stage: IndexStage::BuildingTree,
            completed: 40_000_000,
            total: 400_000_000,
            depth: 0,
            leaves: 0,
            settled: 0,
        };
        studio.track_progress();
        assert!(studio.progress_marks[&Phase::Building] != first);

        // Cancel index stops the octree of the import.
        let _ = studio.update(Message::CancelIndex);
        assert!(studio.index_jobs[0].stop_tree.load(Ordering::Relaxed));
        let lines = studio.progress_lines();
        assert_eq!(lines[0].title, "Cancelling…");
        assert!(lines[0].cancel.is_none());

        // Cancelled while the source is read, the import reads on as an
        // opening of its own, which can still be cancelled.
        let import = job("station.e57", 30_000_000);
        let progress = Arc::new(Mutex::new(reading(30_000_000, 120_000_000)));
        studio.index_jobs.clear();
        studio.index_jobs.push(index_job(
            None,
            Some(8),
            "station.e57",
            &progress,
            &import.cancel,
        ));
        studio.imports.insert(8, import);
        let _ = studio.update(Message::CancelIndex);
        let lines = studio.progress_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].phase, Phase::Opening);
        assert_eq!(lines[0].title, "Opening station.e57");
        assert_eq!(lines[0].detail, "30.0M of 120.0M points");
        assert_eq!(lines[0].fraction, Some(0.25));
        assert!(matches!(lines[0].cancel, Some(Message::CancelOpening)));
        let _ = studio.update(Message::CancelOpening);
        assert!(studio.imports[&8].cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn builds_side_by_side_report_how_many_run_and_wait() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        let mut studio = Studio {
            auto_index: true,
            ..Studio::default()
        };
        for name in ["a.xyz", "b.xyz", "c.xyz"] {
            let path = directory.path().join(name);
            std::fs::write(&path, "0 0 1\n1 0 1\n").unwrap();
            let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
            let _ = studio.update(Message::Loaded(Ok(cloud)));
        }
        let first = Arc::new(Mutex::new(reading(250, 1_000)));
        let second = Arc::new(Mutex::new(IndexProgress {
            stage: IndexStage::BuildingTree,
            completed: 1_000,
            total: 1_000,
            depth: 2,
            leaves: 9,
            settled: 500,
        }));
        for (index, progress) in [(0, &first), (1, &second)] {
            let source = Arc::clone(&studio.clouds[index].cloud);
            studio.clouds[index].index_building = true;
            studio.index_jobs.push(index_job(
                Some(source),
                None,
                &format!("{index}.xyz"),
                progress,
                &Arc::new(AtomicBool::new(false)),
            ));
        }
        studio.clouds[2].auto_index_queued = true;
        // One build ended before.
        studio.index_finished = 1;

        let lines = studio.progress_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].phase, Phase::Indexing);
        assert_eq!(lines[0].title, "Indexing 4 scans");
        assert_eq!(lines[0].detail, "1 of 4 ready  ·  2 at once  ·  1 waiting");
        // One done, a quarter of the reading of one and the tree of the
        // other at 60%.
        let fraction = lines[0].fraction.unwrap();
        assert!(
            (fraction - (1.0 + 0.125 + 0.8) / 4.0).abs() < 1e-6,
            "{fraction}"
        );
        assert!(!lines[0].timed);
        assert!(matches!(lines[0].cancel, Some(Message::CancelIndex)));

        // Every row tells its own build.
        assert_eq!(
            studio.layer_progress(&studio.clouds[0]),
            Some(("reading 25%".to_owned(), Some(0.25)))
        );
        assert_eq!(
            studio.layer_progress(&studio.clouds[1]),
            Some(("indexing 60%".to_owned(), Some(0.6)))
        );
        assert_eq!(
            studio.layer_progress(&studio.clouds[2]),
            Some(("index queued".to_owned(), None))
        );

        // Nothing waiting: the count leaves the waiting out.
        studio.clouds[2].auto_index_queued = false;
        assert_eq!(
            studio.progress_lines()[0].detail,
            "1 of 3 ready  ·  2 at once"
        );
        // A single build without a queue has the line of its two steps.
        studio.index_jobs.remove(1);
        assert_eq!(studio.progress_lines()[0].title, "Indexing 0.xyz");
        studio.index_jobs[0].cancel.store(true, Ordering::Relaxed);
        assert_eq!(studio.progress_lines()[0].title, "Cancelling…");
    }

    #[test]
    fn large_scans_that_wait_for_their_turn_are_told_as_waiting() {
        let _language = TestLanguage::hold(Language::English);
        let mut studio = Studio::default();
        let progress = |stage, completed, total| {
            Arc::new(Mutex::new(IndexProgress {
                stage,
                ..reading(completed, total)
            }))
        };
        // The first of two large scans opened together is read; the second
        // waits until it has been read.
        let first = progress(IndexStage::ReadingSource, 30_000_000, 120_000_000);
        let second = progress(IndexStage::WaitingToRead, 0, 130_000_000);
        for (id, name, progress) in [(1, "first.e57", &first), (2, "second.e57", &second)] {
            let import = job(name, 0);
            studio
                .index_jobs
                .push(index_job(None, Some(id), name, progress, &import.cancel));
            studio.imports.insert(id, import);
        }
        let lines = studio.progress_lines();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].title, "Indexing 2 scans");
        assert_eq!(lines[0].detail, "0 of 2 ready  ·  1 building  ·  1 waiting");
        assert_eq!(lines[0].fraction, Some(0.125 / 2.0));
        let _ = studio.update(Message::IndexPoll);
        assert_eq!(studio.status, "Building 1 octree; 1 waiting");
        // Both wait, the first for a scan opened before it that is read
        // without an octree.
        first.lock().unwrap().stage = IndexStage::WaitingToRead;
        assert_eq!(
            studio.progress_lines()[0].detail,
            "0 of 2 ready  ·  2 waiting"
        );
        let _ = studio.update(Message::IndexPoll);
        assert_eq!(studio.status, "2 octrees waiting for their turn");
        first.lock().unwrap().stage = IndexStage::ReadingSource;

        // Alone, the waiting scan has a line of its own that says so.
        let alone = studio.index_jobs.remove(1);
        let mut waiting = Studio::default();
        waiting
            .imports
            .insert(2, studio.imports.remove(&2).unwrap());
        waiting.index_jobs.push(alone);
        let lines = waiting.progress_lines();
        assert_eq!(lines[0].phase, Phase::Reading);
        assert_eq!(lines[0].title, "Opening second.e57");
        assert_eq!(
            lines[0].detail,
            "Step 1 of 2  ·  waiting for the scan before it on this disk"
        );
        assert_eq!(lines[0].fraction, None);
        assert_eq!(
            Studio::index_progress_text(*second.lock().unwrap()),
            "Waiting to read: the scan opened before it on this disk is read first"
        );

        // Read, its octree waits for the octree of the first.
        *second.lock().unwrap() = IndexProgress {
            stage: IndexStage::WaitingToBuild,
            ..reading(0, 130_000_000)
        };
        let lines = waiting.progress_lines();
        assert_eq!(lines[0].phase, Phase::Building);
        assert_eq!(
            lines[0].detail,
            "Step 2 of 2  ·  waiting for the octree before it"
        );
        assert_eq!(build_fraction(&waiting.index_jobs[0]), 0.5);
        assert_eq!(
            Studio::index_progress_text(*second.lock().unwrap()),
            "Read; its octree is built once the octree before it is ready"
        );
    }

    #[test]
    fn the_status_bar_counts_builds_in_the_singular_and_the_plural() {
        let _language = TestLanguage::hold(Language::English);
        let texts = || {
            [(3, 0), (2, 1), (1, 1), (1, 2), (0, 2)]
                .map(|(running, waiting)| crate::index_jobs::builds_status(running, waiting))
        };
        assert_eq!(
            texts(),
            [
                "Building 3 octrees at once",
                "Building 2 octrees at once; 1 waiting",
                "Building 1 octree; 1 waiting",
                "Building 1 octree; 2 waiting",
                "2 octrees waiting for their turn",
            ]
        );
        crate::i18n::set(Language::from_key("nl").unwrap());
        assert_eq!(
            texts(),
            [
                "3 indexen tegelijk in opbouw",
                "2 indexen tegelijk in opbouw; 1 wacht",
                "1 index in opbouw; 1 wacht",
                "1 index in opbouw; 2 wachten",
                "2 indexen wachten op hun beurt",
            ]
        );
    }

    #[test]
    fn a_layer_tells_that_its_scan_waits_for_its_turn() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("large.xyz");
        std::fs::write(&path, "0 0 1\n1 0 1\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&cloud))));
        let progress = Arc::new(Mutex::new(IndexProgress {
            stage: IndexStage::WaitingToRead,
            ..reading(0, 2)
        }));
        studio.index_jobs.push(index_job(
            Some(Arc::clone(&cloud)),
            None,
            "large.xyz",
            &progress,
            &Arc::new(AtomicBool::new(false)),
        ));
        let row = |studio: &Studio| studio.layer_progress(&studio.clouds[0]);
        assert_eq!(row(&studio), Some(("waiting to read…".to_owned(), None)));
        progress.lock().unwrap().stage = IndexStage::ReadingSource;
        assert_eq!(row(&studio), Some(("reading 0%".to_owned(), Some(0.0))));
        progress.lock().unwrap().stage = IndexStage::WaitingToBuild;
        assert_eq!(row(&studio), Some(("waiting to index…".to_owned(), None)));
        progress.lock().unwrap().stage = IndexStage::BuildingTree;
        assert_eq!(row(&studio), Some(("indexing 0%".to_owned(), Some(0.0))));
    }

    #[test]
    fn an_import_that_waits_for_its_turn_to_be_read_says_so() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("second.e57");
        let mut studio = Studio::default();
        // The layer of the metadata of a large scan that is read without an
        // octree, after the scan opened before it on the same disk.
        let mut header = pointcloud_core::open(write_points(&directory), 10).unwrap();
        header.path = path;
        header.total_points = 130_000_000;
        let header = Arc::new(header);
        let _ = studio.update(Message::Loaded(Ok(Arc::clone(&header))));
        let import = job("second.e57", 0);
        import.waiting.store(true, Ordering::Relaxed);
        studio.imports.insert(7, import);
        studio.import_headers.insert(7, Arc::clone(&header));
        studio.import_expected.insert(7, 130_000_000);
        studio.opening_total = 1;
        let lines = studio.progress_lines();
        assert_eq!(lines[0].title, "Opening second.e57");
        assert_eq!(
            lines[0].detail,
            "Waiting for the scan opened before it on this disk"
        );
        assert_eq!(lines[0].fraction, None);
        let row = |studio: &Studio| studio.layer_progress(&studio.clouds[0]);
        assert_eq!(row(&studio), Some(("waiting to read…".to_owned(), None)));
        let _ = studio.update(Message::OpenProgress(7));
        assert_eq!(
            studio.status,
            "second.e57 waits for the scan opened before it on this disk"
        );

        // Opened with another scan that is being read.
        studio.imports.insert(6, job("first.e57", 30_000_000));
        studio.opening_total = 2;
        assert_eq!(
            studio.progress_lines()[0].detail,
            "0 of 2 done  ·  1 waiting to be read  ·  30.0M points read"
        );

        // Its read started.
        let import = &studio.imports[&7];
        import.waiting.store(false, Ordering::Relaxed);
        import.decoded.store(13_000_000, Ordering::Relaxed);
        studio.imports.remove(&6);
        studio.opening_total = 1;
        assert_eq!(studio.progress_lines()[0].detail, "13.0M of 130.0M points");
        assert_eq!(row(&studio), Some(("reading 10%".to_owned(), Some(0.1))));
    }

    /// A text file of two points in `directory`.
    fn write_points(directory: &tempfile::TempDir) -> PathBuf {
        let path = directory.path().join("points.xyz");
        std::fs::write(&path, "0 0 1\n1 0 1\n").unwrap();
        path
    }
}
