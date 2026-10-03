//! What the window tells about scans that are still being opened or indexed
//! and about a section drawing that is being made: a line per task with how
//! far it is and how long it will still take.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::{button, column, container, progress_bar, row, text};
use iced::{Color, Element, Fill};
use pointcloud_core::IndexStage;

use crate::{compact_count, display_name, flat_tool_style, ui_theme, CloudEntry, Message, Studio};

/// A task is timed from when it was first seen; a shorter time or a smaller
/// advance than these says nothing yet about how long the rest will take.
const MIN_TIMED: Duration = Duration::from_secs(3);
const MIN_ADVANCE: f32 = 0.01;

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
        format!("about {} s left", seconds.div_ceil(5).max(1) * 5)
    } else {
        format!("about {} min left", (seconds + 30) / 60)
    }
}

fn points_text(done: u64, total: Option<u64>) -> String {
    match total {
        Some(total) => format!(
            "{} of {} points",
            compact_count(done.min(total)),
            compact_count(total)
        ),
        None => format!("{} points read", compact_count(done)),
    }
}

impl Studio {
    /// The import that also builds the octree of its scan, if one is reading.
    fn indexed_import(&self) -> Option<u64> {
        self.imports
            .iter()
            .find(|(_, job)| self.index_pending && Arc::ptr_eq(&job.cancel, &self.index_cancel))
            .map(|(id, _)| *id)
    }

    /// The tasks that are opening or indexing scans right now.
    pub(crate) fn progress_lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        let indexed = self.indexed_import();
        let plain: Vec<_> = self
            .imports
            .iter()
            .filter(|(id, _)| Some(**id) != indexed)
            .collect();
        if !plain.is_empty() {
            let files = self.opening_total.max(plain.len());
            let done = files - plain.len();
            let read: u64 = plain
                .iter()
                .map(|(_, job)| job.decoded.load(Ordering::Relaxed))
                .sum();
            let expected: Option<u64> = plain
                .iter()
                .map(|(id, _)| self.import_expected.get(*id).copied())
                .sum();
            // Scans that are done count in full, the others as far as they are.
            let reading: f32 = plain
                .iter()
                .filter_map(|(id, job)| {
                    fraction(
                        job.decoded.load(Ordering::Relaxed),
                        *self.import_expected.get(*id)?,
                    )
                })
                .sum();
            let cancelling = plain
                .iter()
                .all(|(_, job)| job.cancel.load(Ordering::Relaxed));
            let mut detail = points_text(read, expected);
            let title = if cancelling {
                "Cancelling…".to_owned()
            } else if files > 1 {
                detail = format!("{done} of {files} done  ·  {detail}");
                format!("Opening {files} scans")
            } else {
                format!("Opening {}", display_name(&plain[0].1.path))
            };
            lines.push(Line {
                phase: Phase::Opening,
                title,
                detail,
                fraction: (expected.is_some() || files > 1)
                    .then(|| ((done as f32 + reading) / files as f32).min(1.0)),
                // Scans of different sizes count alike in the bar of a
                // batch, so its pace says little about the time left.
                timed: files == 1,
                cancel: (!cancelling).then_some(Message::CancelOpening),
            });
        }

        let progress = self
            .index_progress
            .as_ref()
            .filter(|_| self.index_pending)
            .and_then(|progress| progress.lock().ok().map(|progress| *progress));
        if let Some(progress) = progress {
            let building = self
                .clouds
                .iter()
                .find(|entry| entry.index_building || entry.index_import_id.is_some());
            let name = building
                .map(|entry| display_name(&entry.cloud.path))
                .or_else(|| indexed.map(|id| display_name(&self.imports[&id].path)));
            // An import reads and indexes in one go; an open scan only gets
            // its octree.
            let importing =
                indexed.is_some() || building.is_some_and(|entry| entry.index_import_id.is_some());
            let verb = if importing { "Opening" } else { "Indexing" };
            let cancelling = self.index_cancel.load(Ordering::Relaxed);
            let waiting = self
                .clouds
                .iter()
                .filter(|entry| entry.auto_index_queued)
                .count();
            let (phase, mut detail, fraction) = match progress.stage {
                // Nothing read yet: the source is being opened, or an octree
                // kept from an earlier session is being attached.
                IndexStage::ReadingSource if progress.completed == 0 => {
                    (Phase::Reading, "Preparing…".to_owned(), None)
                }
                IndexStage::ReadingSource => (
                    Phase::Reading,
                    format!(
                        "Step 1 of 2  ·  reading  ·  {}",
                        points_text(
                            progress.completed,
                            (progress.total > 0).then_some(progress.total)
                        )
                    ),
                    progress.fraction(),
                ),
                IndexStage::BuildingTree | IndexStage::Ready => (
                    Phase::Building,
                    format!(
                        "Step 2 of 2  ·  building the octree  ·  {} of {} points placed",
                        compact_count(progress.settled.min(progress.total)),
                        compact_count(progress.total)
                    ),
                    progress.fraction(),
                ),
            };
            if waiting > 0 {
                detail.push_str(&format!("  ·  {waiting} more waiting"));
            }
            lines.push(Line {
                phase,
                title: match (cancelling, name) {
                    (true, _) => "Cancelling…".to_owned(),
                    (false, Some(name)) => format!("{verb} {name}"),
                    (false, None) => format!("{verb} a scan"),
                },
                detail,
                fraction,
                timed: true,
                cancel: (!cancelling).then(|| match indexed {
                    Some(id) => Message::CancelImport(id),
                    None => Message::CancelIndex,
                }),
            });
        }
        lines.extend(self.drawing.progress_line());
        lines
    }

    /// Remember when each task started, for the time it still takes, and
    /// forget what belonged to tasks that ended.
    pub(crate) fn track_progress(&mut self) {
        if self.imports.is_empty() {
            self.opening_total = 0;
            self.import_expected.clear();
            if !self.index_pending && !self.drawing.is_running() {
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
        let import = self
            .import_headers
            .iter()
            .find(|(_, header)| entry.matches_source(header))
            .map(|(id, _)| *id);
        if entry.index_building
            || entry.index_import_id.is_some()
            || (import.is_some() && import == self.indexed_import())
        {
            let progress = self
                .index_progress
                .as_ref()
                .and_then(|progress| progress.lock().ok().map(|progress| *progress));
            return match progress {
                Some(progress) if progress.stage == IndexStage::ReadingSource => {
                    percent("reading", progress.fraction())
                }
                Some(progress) => percent("indexing", progress.fraction()),
                None => percent("indexing", None),
            };
        }
        if let Some(job) = import.and_then(|id| self.imports.get(&id)) {
            let expected = import.and_then(|id| self.import_expected.get(&id));
            return percent(
                "reading",
                expected
                    .and_then(|expected| fraction(job.decoded.load(Ordering::Relaxed), *expected)),
            );
        }
        if entry.cloud.points.is_empty() && entry.cloud.total_points > 0 && entry.mesh.is_none() {
            return percent("loading points", None);
        }
        entry
            .auto_index_queued
            .then(|| ("index queued".to_owned(), None))
    }

    /// The strip above the scene with a line per task.
    pub(crate) fn progress_strip(&self) -> Option<Element<'_, Message>> {
        let lines = self.progress_lines();
        if lines.is_empty() {
            return None;
        }
        let now = Instant::now();
        let accent = self.ui_theme.colors().accent;
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
                    .color(Color::from_rgb8(250, 250, 249))
                    .height(16)
                    .width(Fill),
                text(pace).size(11).color(Color::from_rgb8(250, 250, 249)),
            ]
            .spacing(14)
            .align_y(iced::Alignment::Center);
            if let Some(cancel) = line.cancel {
                heading = heading.push(
                    button(text(crate::i18n::tr("Cancel")).size(11))
                        .on_press(cancel)
                        .style(flat_tool_style)
                        .padding([1, 8]),
                );
            }
            // One line each: a narrow scene cuts the text off instead of
            // pushing the scene down.
            let mut task = column![
                container(heading).height(20).clip(true),
                container(
                    text(line.detail)
                        .size(11)
                        .color(Color::from_rgb8(190, 190, 198))
                )
                .height(15)
                .width(Fill)
                .clip(true),
            ]
            .spacing(3);
            if let Some(fraction) = line.fraction {
                task = task.push(progress_bar(0.0..=1.0, fraction).height(5).style(move |_| {
                    progress_bar::Style {
                        background: Color::from_rgba8(255, 255, 255, 0.12).into(),
                        bar: accent.into(),
                        border: iced::Border::default().rounded(2.5),
                    }
                }));
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
    use std::sync::Mutex;

    use pointcloud_core::IndexProgress;

    use super::*;
    use crate::ImportJob;

    fn job(name: &str, decoded: u64) -> ImportJob {
        ImportJob {
            path: PathBuf::from(name),
            decoded: Arc::new(AtomicU64::new(decoded)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn time_left_follows_the_pace_since_the_task_was_first_seen() {
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
        let mut studio = Studio::default();
        let mut import = job("merged.e57", 0);
        import.cancel = Arc::clone(&studio.index_cancel);
        studio.imports.insert(7, import);
        studio.index_pending = true;
        let progress = Arc::new(Mutex::new(IndexProgress {
            stage: IndexStage::ReadingSource,
            completed: 100_000_000,
            total: 400_000_000,
            depth: 0,
            leaves: 0,
            settled: 0,
        }));
        studio.index_progress = Some(Arc::clone(&progress));

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

        // The next octree in the queue starts its own clock.
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

        studio.index_cancel.store(true, Ordering::Relaxed);
        let lines = studio.progress_lines();
        assert_eq!(lines[0].title, "Cancelling…");
        assert!(lines[0].cancel.is_none());
    }
}
