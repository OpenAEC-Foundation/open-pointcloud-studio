//! The 3D BAG panel and its download: choosing an area in RD New on the map
//! or in the fields, the job that reads the buildings page by page with its
//! progress and its cancel, and the commands of the local API for it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::canvas::Canvas;
use iced::widget::{
    button, column, container, image, pick_list, progress_bar, row, stack, text, text_input,
};
use iced::{Element, Fill, Font, Point as UiPoint, Task};
use pointcloud_core::{BagBounds, BagLod, BagProgress, BagStats};
use serde_json::{json, Value};

use crate::bag_map::{self, BagMap};
use crate::extensions;
use crate::i18n::{key, tr, tr_args};
use crate::{flat_tool_style, opencad_ribbon, themed_pick_list_style, CloudEntry, Message, Studio};

/// What the core answers when a download was stopped on request.
const CANCELLED: &str = "Operation cancelled";

/// The status line for an area in the fields that does not lie in RD New.
const NOT_RD_NEW: &str = "Coordinates must be RD New (EPSG:28992)";

/// What the worker of a download tells the window, and the window the worker.
#[derive(Default)]
pub(crate) struct BagControl {
    cancelled: AtomicBool,
    page: AtomicUsize,
    /// Pages the area needs; zero while the service has not said how many.
    pages: AtomicUsize,
    buildings: AtomicUsize,
}

impl BagControl {
    fn report(&self, progress: BagProgress) {
        self.page.store(progress.page, Ordering::Relaxed);
        self.pages
            .store(progress.pages.unwrap_or(0), Ordering::Relaxed);
        self.buildings.store(progress.buildings, Ordering::Relaxed);
    }

    fn snapshot(&self) -> BagProgress {
        let pages = self.pages.load(Ordering::Relaxed);
        BagProgress {
            page: self.page.load(Ordering::Relaxed),
            pages: (pages > 0).then_some(pages),
            buildings: self.buildings.load(Ordering::Relaxed),
        }
    }
}

/// A download of buildings that is under way.
pub(crate) struct BagJob {
    path: PathBuf,
    lod: BagLod,
    control: Arc<BagControl>,
    started: Instant,
    api_job_id: Option<String>,
    /// The line last written to the status bar, so a look that finds nothing
    /// new leaves the messages of other work readable.
    reported: String,
}

impl BagJob {
    /// The job of the local API that follows this download, if it was
    /// started through the API.
    pub(crate) fn api_job_id(&self) -> Option<&str> {
        self.api_job_id.as_deref()
    }

    fn cancelling(&self) -> bool {
        self.control.cancelled.load(Ordering::Relaxed)
    }

    /// The job as `status` and `job` of the local API report it.
    pub(crate) fn progress_value(&self) -> Value {
        let progress = self.control.snapshot();
        json!({
            "state": "running",
            "operation": "bag3d",
            "path": self.path,
            "lod": self.lod.to_string(),
            "page": progress.page,
            "pages": progress.pages,
            "buildings": progress.buildings,
            "cancel_requested": self.cancelling(),
            "elapsed_seconds": self.started.elapsed().as_secs(),
        })
    }

    /// The line of the status bar while the download runs.
    fn status_text(&self) -> String {
        if self.cancelling() {
            return "Cancelling the 3DBAG download…".into();
        }
        let progress = self.control.snapshot();
        match (progress.page, progress.pages) {
            (0, _) => format!("Downloading 3DBAG LoD {} buildings…", self.lod),
            (page, Some(pages)) => format!(
                "Downloading 3DBAG LoD {}: page {page} of {pages}, {} buildings",
                self.lod, progress.buildings
            ),
            (page, None) => format!(
                "Downloading 3DBAG LoD {}: page {page}, {} buildings",
                self.lod, progress.buildings
            ),
        }
    }
}

/// How far a download is, as the panel words it in the language in use.
fn progress_text(progress: BagProgress, cancelling: bool) -> String {
    if cancelling {
        // The request under way is not interrupted, so this can last as long
        // as one page takes.
        return tr("Cancelling…").to_owned();
    }
    match (progress.page, progress.pages) {
        (0, _) => tr("Downloading…").to_owned(),
        (page, Some(pages)) => tr_args(
            "Page {page} of {pages} · {buildings} buildings",
            &[
                ("page", &page),
                ("pages", &pages),
                ("buildings", &progress.buildings),
            ],
        ),
        (page, None) => tr_args(
            "Page {page} · {buildings} buildings",
            &[("page", &page), ("buildings", &progress.buildings)],
        ),
    }
}

/// The part of the pages that has been read, when the service said how many
/// the area needs.
fn progress_fraction(progress: BagProgress) -> Option<f32> {
    progress
        .pages
        .map(|pages| (progress.page as f32 / pages.max(1) as f32).min(1.0))
}

/// Why an area cannot be downloaded, in words for the panel.
fn area_problem(bounds: BagBounds) -> Option<&'static str> {
    if !bounds.within_rd_new() {
        Some(key(
            "These are not RD New coordinates (EPSG:28992). Draw the area on the map.",
        ))
    } else if bounds.validate().is_err() {
        Some(key(
            "Too large: a side may be at most 2,000 m. Choose a smaller area.",
        ))
    } else {
        None
    }
}

/// The reason of a failure without the words the core puts before every
/// refusal of data, which name a point cloud where there is none.
pub(crate) fn plain_reason(error: &str) -> &str {
    error.strip_prefix("Invalid point cloud: ").unwrap_or(error)
}

fn is_obj_path(path: &std::path::Path) -> bool {
    path.is_absolute()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"))
}

impl Studio {
    /// Show or hide the panel. A panel that opens without an area takes the
    /// one of the scan or the section box, and its map starts loading.
    pub(crate) fn set_bag_panel(&mut self, open: bool) -> Task<Message> {
        if open && !self.extensions.enabled(extensions::BAG3D) {
            self.status = "3D BAG is switched off; switch it on under File > Extensions".into();
            return Task::none();
        }
        let opening = open && !self.bag_panel;
        self.bag_panel = open;
        if !open {
            self.bag_map_drawing = false;
            return Task::none();
        }
        let prefill = if opening && self.bag_fields.iter().all(String::is_empty) {
            self.bag_from_section()
        } else {
            Task::none()
        };
        Task::batch([prefill, self.schedule_bag_map()])
    }

    /// What switching the extension off takes away: the panel and a download
    /// that is under way.
    pub(crate) fn leave_bag3d(&mut self) {
        self.bag_panel = false;
        self.bag_map_drawing = false;
        if let Some(job) = &self.bag_job {
            job.control.cancelled.store(true, Ordering::Relaxed);
        }
    }

    /// The area of the section box, or else of the active scan.
    fn bag_scan_area(&self) -> Option<BagBounds> {
        let bounds = self.section_bounds().or_else(|| {
            self.active
                .and_then(|index| self.clouds.get(index))
                .map(CloudEntry::bounds)
        })?;
        Some(BagBounds {
            min_x: bounds.min[0],
            min_y: bounds.min[1],
            max_x: bounds.max[0],
            max_y: bounds.max[1],
        })
    }

    /// Take the area of the section box, or else of the active scan, when
    /// that lies where RD New coordinates lie. A scan in local coordinates
    /// would centre the map far outside the country, on tiles that do not
    /// exist.
    pub(crate) fn bag_from_section(&mut self) -> Task<Message> {
        let Some(area) = self.bag_scan_area() else {
            self.status = "Draw an area on the map or enter RD coordinates".into();
            return Task::none();
        };
        if !area.within_rd_new() {
            self.status = "The scan is not in RD New coordinates; draw the area on the map".into();
            return Task::none();
        }
        self.bag_fields = [
            format!("{:.2}", area.min_x),
            format!("{:.2}", area.min_y),
            format!("{:.2}", area.max_x),
            format!("{:.2}", area.max_y),
        ];
        self.status = "3DBAG area copied from scan / section box".into();
        let mut view = self.bag_map_view();
        view.fit(area);
        self.bag_map_center = view.center;
        self.bag_map_zoom = view.zoom;
        self.schedule_bag_map()
    }

    /// Show the area of the fields on the map. A box outside RD New would
    /// centre the map on tiles that do not exist and leave it blank.
    pub(crate) fn bag_fit_fields(&mut self) -> Task<Message> {
        let bounds = match BagBounds::parse(&self.bag_fields.join(",")) {
            Ok(bounds) => bounds,
            Err(error) => {
                self.status = format!("Map area: {}", plain_reason(&error.to_string()));
                return Task::none();
            }
        };
        if !bounds.within_rd_new() {
            self.status = NOT_RD_NEW.into();
            return Task::none();
        }
        let mut view = self.bag_map_view();
        view.fit(bounds);
        self.bag_map_center = view.center;
        self.bag_map_zoom = view.zoom;
        self.schedule_bag_map()
    }

    /// The size of the area in the fields, and why it cannot be downloaded
    /// when it cannot, so the user reads it before pressing Download.
    pub(crate) fn bag_area_note(&self) -> Option<(String, Option<&'static str>)> {
        let bounds = self.bag_fields_bounds()?;
        let width = format!("{:.0}", bounds.max_x - bounds.min_x);
        let height = format!("{:.0}", bounds.max_y - bounds.min_y);
        let size = tr_args(
            "Area: {width} × {height} m",
            &[("width", &width), ("height", &height)],
        );
        Some((size, area_problem(bounds)))
    }

    /// Ask where to save the buildings of the area in the fields.
    pub(crate) fn bag_download(&mut self) -> Task<Message> {
        if self.bag_job.is_some() || self.bag_dialog_pending {
            return Task::none();
        }
        let bounds = match BagBounds::parse(&self.bag_fields.join(",")) {
            Ok(bounds) => bounds,
            Err(error) => {
                self.status = format!("3DBAG area: {}", plain_reason(&error.to_string()));
                return Task::none();
            }
        };
        if !bounds.within_rd_new() {
            self.status = NOT_RD_NEW.into();
            return Task::none();
        }
        let lod = self.bag_lod;
        self.bag_dialog_pending = true;
        self.status = "Choose where to save the 3DBAG OBJ…".into();
        Task::perform(
            async {
                rfd::AsyncFileDialog::new()
                    .add_filter("OBJ mesh", &["obj"])
                    .set_file_name("3dbag-buildings.obj")
                    .save_file()
                    .await
                    .map(|selection| selection.path().to_path_buf())
            },
            move |path| Message::BagPathChosen(bounds, lod, path),
        )
    }

    pub(crate) fn bag_path_chosen(
        &mut self,
        bounds: BagBounds,
        lod: BagLod,
        path: Option<PathBuf>,
    ) -> Task<Message> {
        self.bag_dialog_pending = false;
        match path {
            // The extension may have been switched off while the dialog was open.
            Some(_) if !self.extensions.enabled(extensions::BAG3D) => Task::none(),
            Some(path) => self.start_bag_job(bounds, lod, path, None),
            None => {
                self.status = "3DBAG save cancelled".into();
                Task::none()
            }
        }
    }

    /// Start the download on a worker thread. The window reads its progress
    /// four times a second until `Message::BagReady` arrives.
    fn start_bag_job(
        &mut self,
        bounds: BagBounds,
        lod: BagLod,
        path: PathBuf,
        api_job_id: Option<String>,
    ) -> Task<Message> {
        let control = Arc::new(BagControl::default());
        let mut job = BagJob {
            path: path.clone(),
            lod,
            control: Arc::clone(&control),
            started: Instant::now(),
            api_job_id,
            reported: String::new(),
        };
        job.reported = job.status_text();
        self.status.clone_from(&job.reported);
        self.bag_job = Some(job);
        self.bag_last_error = None;
        // The figures of an earlier download would read as the result of
        // this one.
        self.bag_last_stats = None;
        let worker = Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    pointcloud_core::fetch_bag3d_obj_with(
                        bounds,
                        lod,
                        &path,
                        &|progress| control.report(progress),
                        &control.cancelled,
                    )
                    .map(|stats| (path, stats))
                    .map_err(|error| error.to_string())
                })
                .await
                .map_err(|error| error.to_string())
                .and_then(|result| result)
            },
            Message::BagReady,
        );
        Task::batch([worker, Self::bag_poll_task()])
    }

    fn bag_poll_task() -> Task<Message> {
        Task::perform(
            async { tokio::time::sleep(Duration::from_millis(250)).await },
            |()| Message::BagPoll,
        )
    }

    /// Show how far the download is, in the panel, the status bar and the
    /// job of the local API. The status bar is written only when the download
    /// has something new to say: a page takes far longer than a look, and
    /// the user keeps working meanwhile.
    pub(crate) fn bag_poll(&mut self) -> Task<Message> {
        let Some(job) = &mut self.bag_job else {
            return Task::none();
        };
        let text = job.status_text();
        if text != job.reported {
            self.status.clone_from(&text);
            job.reported = text;
        }
        if let Some(id) = &job.api_job_id {
            if let Some(entry) = self.api_jobs.get_mut(id) {
                *entry = job.progress_value();
            }
        }
        Self::bag_poll_task()
    }

    pub(crate) fn cancel_bag(&mut self) {
        if let Some(job) = &mut self.bag_job {
            job.control.cancelled.store(true, Ordering::Relaxed);
            job.reported = job.status_text();
            self.status.clone_from(&job.reported);
        }
    }

    /// The download ended: open the buildings as a layer, or say why there
    /// are none.
    pub(crate) fn bag_ready(
        &mut self,
        result: Result<(PathBuf, BagStats), String>,
    ) -> Task<Message> {
        let job = self.bag_job.take();
        let state = match &result {
            Ok((path, stats)) => json!({
                "state": "complete",
                "operation": "bag3d",
                "path": path,
                "buildings": stats.buildings,
                "vertices": stats.vertices,
                "triangles": stats.triangles,
                "pages": stats.pages,
            }),
            Err(error) if error == CANCELLED => json!({
                "state": "cancelled",
                "operation": "bag3d",
                "path": job.as_ref().map(|job| &job.path),
            }),
            Err(error) => json!({
                "state": "failed",
                "operation": "bag3d",
                "error": plain_reason(error),
            }),
        };
        if let Some(id) = job.and_then(|job| job.api_job_id) {
            if let Some(entry) = self.api_jobs.get_mut(&id) {
                *entry = state;
            }
        }
        match result {
            Ok((path, stats)) => {
                self.bag_last_stats = Some(stats);
                let task = self.load(path);
                self.status = format!(
                    "3DBAG downloaded: {} buildings, {} triangles from {} page(s)",
                    stats.buildings, stats.triangles, stats.pages
                );
                task
            }
            Err(error) if error == CANCELLED => {
                self.status = "3DBAG download cancelled; output left unchanged".into();
                Task::none()
            }
            Err(error) => {
                let reason = plain_reason(&error);
                self.status = format!("3DBAG failed: {reason}");
                self.bag_last_error = Some(reason.to_owned());
                Task::none()
            }
        }
    }

    /// The `bag3d` command of the local API: the same download as the panel
    /// starts, to a path the caller names.
    pub(crate) fn api_bag3d(
        &mut self,
        bbox: [f64; 4],
        lod: &str,
        path: PathBuf,
    ) -> (Value, Task<Message>) {
        let bounds = BagBounds {
            min_x: bbox[0],
            min_y: bbox[1],
            max_x: bbox[2],
            max_y: bbox[3],
        };
        let lod = BagLod::parse(lod);
        let refusal = if !self.extensions.enabled(extensions::BAG3D) {
            Some("extension bag3d is disabled".to_owned())
        } else if self.bag_job.is_some() || self.bag_dialog_pending {
            Some("a 3D BAG download is already open or running".to_owned())
        } else if !is_obj_path(&path) {
            Some("bag3d requires an absolute .obj destination".to_owned())
        } else if !path.parent().is_some_and(std::path::Path::is_dir) {
            // The core opens its temporary file there only after the last
            // page, so a mistyped folder would cost the whole download.
            Some("the folder of the bag3d destination does not exist".to_owned())
        } else if lod.is_err() {
            Some("lod must be 1.2, 1.3 or 2.2".to_owned())
        } else if let Err(error) = bounds.validate() {
            Some(plain_reason(&error.to_string()).to_owned())
        } else if !bounds.within_rd_new() {
            Some("bbox must be in RD New coordinates (EPSG:28992)".to_owned())
        } else {
            None
        };
        match (refusal, lod) {
            (None, Ok(lod)) => {
                let id = self.record_api_job(json!({
                    "state": "running", "operation": "bag3d", "path": path, "lod": lod.to_string()
                }));
                let task = self.start_bag_job(bounds, lod, path.clone(), Some(id.clone()));
                (
                    json!({"ok": true, "accepted": true, "job_id": id, "path": path}),
                    task,
                )
            }
            (refusal, _) => (
                json!({"ok": false, "error": refusal.unwrap_or_default()}),
                Task::none(),
            ),
        }
    }

    pub(crate) fn api_cancel_bag3d(&mut self) -> Value {
        if self.bag_job.is_none() {
            return json!({"ok": false, "error": "no 3D BAG download is running"});
        }
        self.cancel_bag();
        json!({"ok": true, "cancel_requested": true})
    }

    pub(crate) fn bag_panel_view(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        // The area of a scan in local coordinates is not offered.
        let scan_in_rd = self.bag_scan_area().is_none_or(|area| area.within_rd_new());
        let map = stack![
            image(self.bag_map_raster.clone())
                .width(Fill)
                .height(bag_map::HEIGHT)
                .content_fit(iced::ContentFit::Fill),
            Canvas::new(BagMap {
                center: self.bag_map_center,
                zoom: self.bag_map_zoom,
                drawing: self.bag_map_drawing,
                selected: self.bag_fields_bounds(),
            })
            .width(Fill)
            .height(bag_map::HEIGHT),
        ]
        .width(Fill)
        .height(bag_map::HEIGHT);
        let mut panel = column![
            row![
                text(tr("3D BAG"))
                    .size(15)
                    .font(Font::with_name("Space Grotesk"))
                    .width(Fill),
                button("×")
                    .on_press(Message::ToggleBagPanel)
                    .style(flat_tool_style),
            ]
            .align_y(iced::Alignment::Center),
            text(tr("Download buildings in RD New + NAP (EPSG:7415)."))
                .size(11)
                .color(colors.text_muted),
            text(tr(
                "At most 2 × 2 km and about 5,000 buildings per download."
            ))
            .size(11)
            .color(colors.text_muted),
            container(map)
                .width(Fill)
                .height(bag_map::HEIGHT)
                .clip(true),
            row![
                button(if self.bag_map_drawing {
                    tr("Cancel draw")
                } else {
                    tr("Draw area")
                })
                .on_press(Message::BagMapDraw(!self.bag_map_drawing))
                .style(flat_tool_style),
                button(tr("Fit area"))
                    .on_press(Message::BagMapFitFields)
                    .style(flat_tool_style),
                button(tr("Amsterdam"))
                    .on_press(Message::BagMapHome)
                    .style(flat_tool_style),
            ]
            .spacing(6),
            row![
                button("−")
                    .on_press(Message::BagMapZoom(
                        -1.0,
                        UiPoint::new(bag_map::WIDTH * 0.5, bag_map::HEIGHT * 0.5),
                    ))
                    .style(flat_tool_style),
                button("+")
                    .on_press(Message::BagMapZoom(
                        1.0,
                        UiPoint::new(bag_map::WIDTH * 0.5, bag_map::HEIGHT * 0.5),
                    ))
                    .style(flat_tool_style),
                text(tr("Drag to pan · scroll to zoom")).size(10),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            row![
                text(tr("© Kadaster (BRT) via PDOK · CC BY 4.0")).size(10),
                button(tr("License ↗"))
                    .on_press(Message::OpenPdokLicense)
                    .style(flat_tool_style),
            ]
            .spacing(5)
            .align_y(iced::Alignment::Center),
            button(tr("Use scan / section box"))
                .on_press_maybe(scan_in_rd.then_some(Message::BagFromSection))
                .style(flat_tool_style),
        ]
        .spacing(10)
        .padding(12)
        .width(Fill);
        if !scan_in_rd {
            panel = panel.push(
                text(tr(
                    "The scan is not in RD New coordinates; draw the area on the map.",
                ))
                .size(11)
                .color(colors.text_muted),
            );
        }
        for (index, name) in [key("X min"), key("Y min"), key("X max"), key("Y max")]
            .into_iter()
            .enumerate()
        {
            let name = tr(name);
            panel = panel.push(
                column![
                    text(name).size(11),
                    text_input(name, &self.bag_fields[index])
                        .on_input(move |value| Message::BagField(index, value))
                        .width(Fill),
                ]
                .spacing(3),
            );
        }
        let area = self.bag_area_note();
        let problem = area.as_ref().and_then(|(_, problem)| *problem);
        if let Some((size, _)) = area {
            panel = panel.push(text(size).size(11).color(colors.text_muted));
        }
        if let Some(problem) = problem {
            panel = panel.push(text(tr(problem)).size(11).color(colors.accent));
        }
        panel = panel.push(text(tr("Level of detail")).size(11)).push(
            pick_list(BagLod::ALL, Some(self.bag_lod), Message::BagLod)
                .style(themed_pick_list_style),
        );
        if let Some(job) = &self.bag_job {
            let progress = job.control.snapshot();
            let cancelling = job.cancelling();
            panel = panel.push(text(progress_text(progress, cancelling)).size(12));
            if let Some(fraction) = progress_fraction(progress) {
                panel = panel.push(progress_bar(0.0..=1.0, fraction).height(6));
            }
            panel = panel.push(
                button(tr("Cancel"))
                    .on_press_maybe((!cancelling).then_some(Message::CancelBag))
                    .style(flat_tool_style),
            );
        } else {
            let ready = problem.is_none() && !self.bag_dialog_pending;
            panel = panel.push(
                button(tr("Download OBJ"))
                    .on_press_maybe(ready.then_some(Message::BagDownload))
                    .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status)),
            );
        }
        if let Some(error) = &self.bag_last_error {
            panel = panel
                .push(text(tr("The last download failed:")).size(11))
                .push(text(error.as_str()).size(11).color(colors.accent));
        }
        if let Some(stats) = self.bag_last_stats {
            panel = panel.push(
                text(tr_args(
                    "{buildings} buildings · {triangles} triangles · {pages} pages",
                    &[
                        ("buildings", &stats.buildings),
                        ("triangles", &stats.triangles),
                        ("pages", &stats.pages),
                    ],
                ))
                .size(11),
            );
        }
        panel
            .push(text(tr("© 3DBAG by tudelft3d and 3DGI")).size(10))
            .push(
                button(tr("CC BY 4.0 · source and license ↗"))
                    .on_press(Message::OpenBagLicense)
                    .style(flat_tool_style),
            )
            .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_view::FileAction;
    use crate::i18n::{Language, TestLanguage};
    use crate::native_api::{ApiCommand, ApiRequest};

    const AMSTERDAM: [f64; 2] = [121_000.0, 487_000.0];
    /// An area of 100 by 100 m in the centre of Amsterdam.
    const AREA: BagBounds = BagBounds {
        min_x: 121_000.0,
        min_y: 487_000.0,
        max_x: 121_100.0,
        max_y: 487_100.0,
    };

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    fn bag3d(studio: &mut Studio, bbox: [f64; 4], lod: &str, path: &std::path::Path) -> Value {
        send(
            studio,
            ApiCommand::Bag3d {
                bbox,
                lod: lod.into(),
                path: path.into(),
            },
        )
    }

    /// A studio with one scan whose points are the given text lines.
    fn studio_with_scan(points: &str) -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scan.xyz");
        std::fs::write(&path, points).unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        (studio, directory)
    }

    fn set_fields(studio: &mut Studio, fields: [&str; 4]) {
        for (index, value) in fields.into_iter().enumerate() {
            let _ = studio.update(Message::BagField(index, value.into()));
        }
    }

    #[test]
    fn file_view_entry_opens_the_panel_and_leaves_the_file_view() {
        let mut studio = Studio::default();
        let _ = studio.update(Message::ToggleFile);
        let _ = studio.view();
        let _ = studio.update(Message::FileAction(FileAction::Bag3d));
        assert!(!studio.file_open);
        assert!(studio.bag_panel);
        assert_eq!(
            studio.status,
            "Draw an area on the map or enter RD coordinates"
        );
        let _ = studio.view();
        // Asking again keeps it open; its own button closes it.
        let _ = studio.update(Message::ShowBagPanel);
        assert!(studio.bag_panel);
        let _ = studio.update(Message::ToggleBagPanel);
        assert!(!studio.bag_panel);
    }

    #[test]
    fn switched_off_extension_takes_the_panel_and_the_command_away() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("buildings.obj");
        let mut studio = Studio::default();
        let _ = studio.update(Message::ShowBagPanel);
        let _ = studio.start_bag_job(AREA, BagLod::Lod22, path.clone(), None);
        assert!(studio.bag_panel && studio.bag_job.is_some());

        let _ = studio.update(Message::ExtensionEnabled(extensions::BAG3D, false));
        assert!(!studio.bag_panel, "the panel closes");
        assert!(
            studio.bag_job.as_ref().unwrap().cancelling(),
            "a download under way is stopped"
        );
        let _ = studio.update(Message::BagReady(Err(CANCELLED.into())));

        let _ = studio.update(Message::ToggleFile);
        let _ = studio.view();
        let _ = studio.update(Message::FileAction(FileAction::Bag3d));
        assert!(!studio.bag_panel);
        assert_eq!(
            studio.status,
            "3D BAG is switched off; switch it on under File > Extensions"
        );
        let refused = bag3d(
            &mut studio,
            [121_000.0, 487_000.0, 121_100.0, 487_100.0],
            "2.2",
            &path,
        );
        assert_eq!(
            refused,
            json!({"ok": false, "error": "extension bag3d is disabled"})
        );
        assert!(studio.bag_job.is_none());

        let _ = studio.update(Message::ExtensionEnabled(extensions::BAG3D, true));
        let _ = studio.update(Message::ShowBagPanel);
        assert!(studio.bag_panel);
    }

    #[test]
    fn area_is_taken_from_a_scan_in_rd_new_only() {
        let _language = TestLanguage::hold(Language::English);
        // A scan in local coordinates leaves the fields and the map alone.
        let (mut studio, _directory) = studio_with_scan("0 0 0\n50 0 0\n50 40 0\n0 40 3\n");
        let _ = studio.update(Message::ShowBagPanel);
        assert!(studio.bag_panel);
        assert!(studio.bag_fields.iter().all(String::is_empty));
        assert_eq!(studio.bag_map_center, AMSTERDAM);
        assert_eq!(studio.bag_map_zoom, 11);
        assert_eq!(
            studio.status,
            "The scan is not in RD New coordinates; draw the area on the map"
        );
        let _ = studio.update(Message::BagFromSection);
        assert!(studio.bag_fields.iter().all(String::is_empty));
        assert_eq!(studio.bag_map_center, AMSTERDAM);
        let _ = studio.view();

        let (mut studio, _directory) =
            studio_with_scan("91440 398430 0\n91460 398430 0\n91460 398450 0\n91440 398450 3\n");
        let _ = studio.update(Message::ShowBagPanel);
        assert_eq!(
            studio.bag_fields,
            ["91440.00", "398430.00", "91460.00", "398450.00"]
        );
        assert_eq!(studio.bag_map_center, [91_450.0, 398_440.0]);
        assert_eq!(studio.status, "3DBAG area copied from scan / section box");
        assert_eq!(
            studio.bag_area_note(),
            Some(("Area: 20 × 20 m".into(), None))
        );
    }

    #[test]
    fn panel_says_why_an_area_cannot_be_downloaded_before_the_download() {
        let _language = TestLanguage::hold(Language::English);
        let mut studio = Studio::default();
        let _ = studio.update(Message::ShowBagPanel);
        assert_eq!(studio.bag_area_note(), None, "no area yet");

        // Larger than the service allows in one download.
        set_fields(&mut studio, ["120000", "486000", "122350", " 488180 "]);
        let (size, problem) = studio.bag_area_note().unwrap();
        assert_eq!(size, "Area: 2350 × 2180 m");
        assert_eq!(
            problem,
            Some("Too large: a side may be at most 2,000 m. Choose a smaller area.")
        );
        let _ = studio.view();
        let _ = studio.update(Message::BagDownload);
        assert!(!studio.bag_dialog_pending);
        assert_eq!(
            studio.status,
            "3DBAG area: 3DBAG bbox must have positive sides no longer than 2 km"
        );

        // A box in local coordinates, or in degrees.
        for fields in [["0", "0", "50", "40"], ["4.84", "52.35", "4.95", "52.40"]] {
            set_fields(&mut studio, fields);
            let (_, problem) = studio.bag_area_note().unwrap();
            assert_eq!(
                problem,
                Some("These are not RD New coordinates (EPSG:28992). Draw the area on the map.")
            );
            let _ = studio.update(Message::BagDownload);
            assert!(!studio.bag_dialog_pending);
            assert_eq!(studio.status, "Coordinates must be RD New (EPSG:28992)");
        }

        // An area that fits is asked a destination for.
        set_fields(&mut studio, ["121000", "487000", "121100", "487100"]);
        assert_eq!(
            studio.bag_area_note(),
            Some(("Area: 100 × 100 m".into(), None))
        );
        let _ = studio.update(Message::BagDownload);
        assert!(studio.bag_dialog_pending);
        let _ = studio.view();
        let _ = studio.update(Message::BagPathChosen(AREA, BagLod::Lod22, None));
        assert!(!studio.bag_dialog_pending);
        assert!(studio.bag_job.is_none());
        assert_eq!(studio.status, "3DBAG save cancelled");

        crate::i18n::set(Language::Table(0));
        assert_eq!(studio.bag_area_note().unwrap().0, "Gebied: 100 × 100 m");
    }

    #[test]
    fn fit_area_leaves_the_map_alone_outside_rd_new() {
        let mut studio = Studio::default();
        let _ = studio.update(Message::ShowBagPanel);
        // A box in local coordinates, or in degrees, has no map tiles.
        for fields in [["0", "0", "50", "40"], ["4.84", "52.35", "4.95", "52.40"]] {
            set_fields(&mut studio, fields);
            let _ = studio.update(Message::BagMapFitFields);
            assert_eq!(studio.bag_map_center, AMSTERDAM, "{fields:?}");
            assert_eq!(studio.bag_map_zoom, 11, "{fields:?}");
            assert_eq!(studio.status, "Coordinates must be RD New (EPSG:28992)");
            assert!(
                !studio.bag_map_view().visible_tiles().is_empty(),
                "the map still shows tiles"
            );
        }

        set_fields(&mut studio, ["121000", "487000", "121100", ""]);
        let _ = studio.update(Message::BagMapFitFields);
        assert_eq!(studio.bag_map_center, AMSTERDAM);
        assert!(studio.status.starts_with("Map area: "), "{}", studio.status);

        set_fields(&mut studio, ["91440", "398430", "91460", "398450"]);
        let _ = studio.update(Message::BagMapFitFields);
        assert_eq!(studio.bag_map_center, [91_450.0, 398_440.0]);
        assert!(!studio.bag_map_view().visible_tiles().is_empty());
    }

    #[test]
    fn download_reports_its_pages_and_can_be_cancelled() {
        let _language = TestLanguage::hold(Language::English);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("buildings.obj");
        let mut studio = Studio::default();
        let _ = studio.update(Message::ShowBagPanel);
        let _ = studio.update(Message::BagPathChosen(
            AREA,
            BagLod::Lod22,
            Some(path.clone()),
        ));
        let control = Arc::clone(&studio.bag_job.as_ref().expect("a job runs").control);
        assert_eq!(studio.status, "Downloading 3DBAG LoD 2.2 buildings…");
        assert_eq!(progress_text(control.snapshot(), false), "Downloading…");
        assert_eq!(progress_fraction(control.snapshot()), None);
        let _ = studio.view();
        // A second download waits for the first.
        let _ = studio.update(Message::BagDownload);
        assert!(!studio.bag_dialog_pending);

        // The worker reports a page; the next look of the window shows it.
        control.report(BagProgress {
            page: 3,
            pages: Some(12),
            buildings: 140,
        });
        let _ = studio.update(Message::BagPoll);
        assert_eq!(
            studio.status,
            "Downloading 3DBAG LoD 2.2: page 3 of 12, 140 buildings"
        );
        assert_eq!(
            progress_text(control.snapshot(), false),
            "Page 3 of 12 · 140 buildings"
        );
        assert_eq!(progress_fraction(control.snapshot()), Some(0.25));
        let _ = studio.view();
        // A look that finds the same page leaves the message of other work,
        // and still asks for the next look.
        studio.status = "Opening 1 scan; 5 listed scans not found".into();
        let _ = studio.update(Message::BagPoll);
        assert_eq!(studio.status, "Opening 1 scan; 5 listed scans not found");
        crate::i18n::set(Language::Table(0));
        assert_eq!(
            progress_text(control.snapshot(), false),
            "Pagina 3 van 12 · 140 gebouwen"
        );
        assert_eq!(
            progress_text(control.snapshot(), true),
            "Bezig met annuleren…"
        );
        crate::i18n::set(Language::English);

        // A service that does not say how many objects match.
        control.report(BagProgress {
            page: 4,
            pages: None,
            buildings: 190,
        });
        assert_eq!(
            progress_text(control.snapshot(), false),
            "Page 4 · 190 buildings"
        );
        assert_eq!(progress_fraction(control.snapshot()), None);
        let _ = studio.update(Message::BagPoll);
        assert_eq!(
            studio.status,
            "Downloading 3DBAG LoD 2.2: page 4, 190 buildings"
        );

        let _ = studio.update(Message::CancelBag);
        assert!(control.cancelled.load(Ordering::Relaxed));
        assert_eq!(studio.status, "Cancelling the 3DBAG download…");
        // The cancel is said once, not again at every look.
        studio.status = "Layer hidden".into();
        let _ = studio.update(Message::BagPoll);
        assert_eq!(studio.status, "Layer hidden");
        let _ = studio.view();
        let _ = studio.update(Message::BagReady(Err(CANCELLED.into())));
        assert!(studio.bag_job.is_none());
        assert_eq!(
            studio.status,
            "3DBAG download cancelled; output left unchanged"
        );
        assert_eq!(studio.bag_last_error, None);
        assert!(!path.exists());
        // A look that arrives after the end starts no further one.
        let _ = studio.update(Message::BagPoll);
        assert!(studio.clouds.is_empty());
    }

    #[test]
    fn failed_download_says_why_in_the_panel() {
        let directory = tempfile::tempdir().unwrap();
        let mut studio = Studio::default();
        let _ = studio.update(Message::BagPathChosen(
            AREA,
            BagLod::Lod13,
            Some(directory.path().join("buildings.obj")),
        ));
        let dense = "this area holds about 11989 3DBAG buildings; at most 5000 can be \
                     downloaded at once, choose a smaller area";
        let _ = studio.update(Message::BagReady(Err(format!(
            "Invalid point cloud: {dense}"
        ))));
        assert!(studio.bag_job.is_none());
        assert_eq!(studio.status, format!("3DBAG failed: {dense}"));
        assert_eq!(studio.bag_last_error.as_deref(), Some(dense));
        let _ = studio.update(Message::ShowBagPanel);
        let _ = studio.view();

        // The next download starts without the old failure.
        let _ = studio.update(Message::BagPathChosen(
            AREA,
            BagLod::Lod13,
            Some(directory.path().join("buildings.obj")),
        ));
        assert_eq!(studio.bag_last_error, None);

        // The figures of a download that succeeded are not shown under a
        // later one, neither while it runs nor when it fails.
        let path = directory.path().join("buildings.obj");
        std::fs::write(
            &path,
            "v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
",
        )
        .unwrap();
        let stats = BagStats {
            buildings: 44,
            vertices: 4_928,
            triangles: 6_116,
            pages: 1,
        };
        let _ = studio.update(Message::BagReady(Ok((path.clone(), stats))));
        assert_eq!(studio.bag_last_stats, Some(stats));
        let _ = studio.update(Message::BagPathChosen(AREA, BagLod::Lod13, Some(path)));
        assert_eq!(studio.bag_last_stats, None);
        let _ = studio.update(Message::BagReady(Err(format!(
            "Invalid point cloud: {dense}"
        ))));
        assert_eq!(studio.bag_last_error.as_deref(), Some(dense));
        assert_eq!(studio.bag_last_stats, None);
        let _ = studio.view();
        assert_eq!(plain_reason("I/O error: disk full"), "I/O error: disk full");
    }

    #[test]
    fn finished_download_is_opened_as_a_layer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("buildings.obj");
        let mut studio = Studio::default();
        let _ = studio.update(Message::BagPathChosen(
            AREA,
            BagLod::Lod22,
            Some(path.clone()),
        ));
        std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let stats = BagStats {
            buildings: 1,
            vertices: 3,
            triangles: 1,
            pages: 1,
        };
        assert!(studio.imports.is_empty());
        let _ = studio.update(Message::BagReady(Ok((path.clone(), stats))));
        assert!(studio.bag_job.is_none());
        assert_eq!(studio.bag_last_stats, Some(stats));
        // The buildings are handed to the loader, which registers the import
        // before its task runs.
        assert_eq!(studio.imports.len(), 1);
        assert!(studio.imports.values().all(|job| job.path == path));
        assert_eq!(
            studio.status,
            "3DBAG downloaded: 1 buildings, 1 triangles from 1 page(s)"
        );
        let _ = studio.update(Message::ShowBagPanel);
        let _ = studio.view();
    }

    #[test]
    fn leaving_the_application_stops_the_work_under_way() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("buildings.obj");
        let mut studio = Studio::default();
        // An import that is still being read, and a download beside it.
        let _ = studio.load(path.clone());
        let _ = studio.update(Message::BagPathChosen(AREA, BagLod::Lod22, Some(path)));
        let control = Arc::clone(&studio.bag_job.as_ref().unwrap().control);
        assert!(!control.cancelled.load(Ordering::Relaxed));

        // Exit in the File view and closing the window both arrive as this.
        let _ = studio.update(Message::Exit);
        assert!(
            control.cancelled.load(Ordering::Relaxed),
            "the download stops at its next page and writes nothing"
        );
        assert_eq!(studio.imports.len(), 1);
        assert!(studio
            .imports
            .values()
            .all(|job| job.cancel.load(Ordering::Relaxed)));
    }

    #[test]
    fn api_starts_a_download_as_a_job_and_cancels_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("buildings.obj");
        let area = [121_000.0, 487_000.0, 121_100.0, 487_100.0];
        let mut studio = Studio::default();
        let idle = send(&mut studio, ApiCommand::Status);
        assert!(idle["result"]["bag3d"].is_null());
        assert_eq!(
            send(&mut studio, ApiCommand::CancelBag3d),
            json!({"ok": false, "error": "no 3D BAG download is running"})
        );

        for (bbox, lod, destination, error) in [
            (
                area,
                "2.2",
                std::path::Path::new("buildings.obj"),
                "bag3d requires an absolute .obj destination",
            ),
            (
                area,
                "2.2",
                directory.path().join("buildings.ply").as_path(),
                "bag3d requires an absolute .obj destination",
            ),
            (
                area,
                "2.2",
                directory
                    .path()
                    .join("missing")
                    .join("buildings.obj")
                    .as_path(),
                "the folder of the bag3d destination does not exist",
            ),
            (area, "3", path.as_path(), "lod must be 1.2, 1.3 or 2.2"),
            (
                [120_000.0, 486_000.0, 122_350.0, 488_180.0],
                "2.2",
                path.as_path(),
                "3DBAG bbox must have positive sides no longer than 2 km",
            ),
            (
                [121_100.0, 487_000.0, 121_000.0, 487_100.0],
                "2.2",
                path.as_path(),
                "3DBAG bbox must have positive sides no longer than 2 km",
            ),
            (
                [f64::NAN, 487_000.0, 121_100.0, 487_100.0],
                "2.2",
                path.as_path(),
                "3DBAG bbox must have positive sides no longer than 2 km",
            ),
            (
                [4.84, 52.35, 4.95, 52.40],
                "2.2",
                path.as_path(),
                "bbox must be in RD New coordinates (EPSG:28992)",
            ),
        ] {
            let refused = bag3d(&mut studio, bbox, lod, destination);
            assert_eq!(refused, json!({"ok": false, "error": error}), "{bbox:?}");
            assert!(studio.bag_job.is_none());
        }
        assert!(studio.api_jobs.is_empty());

        let accepted = bag3d(&mut studio, area, "1.2", &path);
        assert_eq!(accepted["ok"], true);
        assert_eq!(accepted["accepted"], true);
        let id = accepted["job_id"].as_str().unwrap().to_owned();
        assert_eq!(
            bag3d(&mut studio, area, "1.2", &path)["error"],
            "a 3D BAG download is already open or running"
        );

        let control = Arc::clone(&studio.bag_job.as_ref().unwrap().control);
        control.report(BagProgress {
            page: 2,
            pages: Some(5),
            buildings: 61,
        });
        // The job is read as it is now, also between two looks of the window.
        let job = send(&mut studio, ApiCommand::Job { id: id.clone() })["job"].clone();
        assert_eq!(job["state"], "running");
        assert_eq!(job["operation"], "bag3d");
        assert_eq!(job["lod"], "1.2");
        assert_eq!((&job["page"], &job["pages"]), (&json!(2), &json!(5)));
        assert_eq!(job["buildings"], 61);
        assert_eq!(job["cancel_requested"], false);
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["bag3d"]["page"], 2);
        assert_eq!(
            crate::mcp::busy(&status["result"]),
            ["bag3d"],
            "waiting until idle waits for the download"
        );

        assert_eq!(
            send(&mut studio, ApiCommand::CancelBag3d),
            json!({"ok": true, "cancel_requested": true})
        );
        let job = send(&mut studio, ApiCommand::Job { id: id.clone() })["job"].clone();
        assert_eq!(job["cancel_requested"], true);
        let _ = studio.update(Message::BagReady(Err(CANCELLED.into())));
        let job = send(&mut studio, ApiCommand::Job { id })["job"].clone();
        assert_eq!(job["state"], "cancelled");
        assert_eq!(job["path"], json!(path));
        let status = send(&mut studio, ApiCommand::Status);
        assert!(status["result"]["bag3d"].is_null());
        assert!(crate::mcp::busy(&status["result"]).is_empty());
    }

    #[test]
    fn api_job_ends_with_the_buildings_or_with_the_reason() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("buildings.obj");
        let area = [121_000.0, 487_000.0, 121_100.0, 487_100.0];
        let mut studio = Studio::default();

        let id = bag3d(&mut studio, area, "2.2", &path)["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let _ = studio.update(Message::BagReady(Err(
            "Invalid point cloud: no 3DBAG buildings with LoD 2.2 in this area".into(),
        )));
        assert_eq!(
            send(&mut studio, ApiCommand::Job { id })["job"],
            json!({
                "state": "failed",
                "operation": "bag3d",
                "error": "no 3DBAG buildings with LoD 2.2 in this area",
            })
        );

        let id = bag3d(&mut studio, area, "2.2", &path)["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        std::fs::write(&path, "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        let stats = BagStats {
            buildings: 44,
            vertices: 4_928,
            triangles: 6_116,
            pages: 1,
        };
        let _ = studio.update(Message::BagReady(Ok((path.clone(), stats))));
        assert_eq!(
            send(&mut studio, ApiCommand::Job { id })["job"],
            json!({
                "state": "complete",
                "operation": "bag3d",
                "path": path,
                "buildings": 44,
                "vertices": 4_928,
                "triangles": 6_116,
                "pages": 1,
            })
        );
    }

    #[test]
    fn commands_are_read_from_their_json_form() {
        let command: ApiCommand = serde_json::from_str(
            r#"{"command": "bag3d", "bbox": [121000, 487000, 121100, 487100], "lod": "2.2", "path": "/tmp/b.obj"}"#,
        )
        .unwrap();
        assert!(matches!(
            command,
            ApiCommand::Bag3d { bbox, lod, .. } if bbox[2] == 121_100.0 && lod == "2.2"
        ));
        assert!(matches!(
            serde_json::from_str::<ApiCommand>(r#"{"command": "cancel_bag3d"}"#).unwrap(),
            ApiCommand::CancelBag3d
        ));
        // A box needs its four numbers.
        assert!(serde_json::from_str::<ApiCommand>(
            r#"{"command": "bag3d", "bbox": [121000, 487000, 121100], "lod": "2.2", "path": "/tmp/b.obj"}"#,
        )
        .is_err());
    }
}
