//! The `screenshot` command of the command API: a PNG image of the scene
//! part of the window, cut from a window screenshot as view snapshots are.
//! While the Drawing view is shown it is the drawing that is captured.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use iced::window::Screenshot;
use iced::{Rectangle, Task};
use serde_json::{json, Value};

use crate::{views, Message, Studio};

/// Why no screenshot is taken while something covers the scene.
pub const COVERED: &str = "the File view, Settings or the Mesh to Plans wizard covers the viewport";
/// Longest edge of a screenshot when the command names none.
pub const DEFAULT_MAX_EDGE: u32 = 1920;
pub const MIN_MAX_EDGE: u32 = 16;
pub const MAX_MAX_EDGE: u32 = 8192;
/// Pause between two looks at whether the viewport still reads points or
/// makes the caps of the cut.
const SETTLE_STEP: Duration = Duration::from_millis(100);
/// Looks before the screenshot is taken while the viewport is still busy.
const SETTLE_STEPS: u8 = 40;
/// Looks in a row without points being read or caps being made before the
/// screenshot is taken. Reading starts 220 ms after a camera change and the
/// points that arrived last must have been drawn, so this spans longer than
/// that.
const QUIET_STEPS: u8 = 4;

/// A screenshot command on its way, with where its answer goes.
#[derive(Debug, Clone)]
pub struct Request {
    reply: Sender<Value>,
    path: Option<PathBuf>,
    base64: bool,
    max_edge: u32,
}

#[derive(Debug, Clone)]
pub enum Step {
    /// Look whether the viewport still reads the points of its camera or
    /// makes the caps of the cut.
    Settle {
        request: Request,
        remaining: u8,
        quiet: u8,
    },
    Captured(Request, Result<Screenshot, Uncaptured>),
}

/// Why the window was not captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uncaptured {
    NoWindow,
    /// Minimised, or without any area.
    Minimised,
}

impl Uncaptured {
    fn message(self) -> &'static str {
        match self {
            Self::NoWindow => "the viewport could not be captured",
            Self::Minimised => "the window is minimised; restore it to take a screenshot",
        }
    }
}

/// A screenshot of the window, unless it is minimised or has no area: the
/// renderer cannot make an image of zero size and would stop the
/// application. The size is looked at just before the screenshot is asked;
/// a window minimised in the moment between the two is not caught.
pub fn capture_window() -> Task<Result<Screenshot, Uncaptured>> {
    iced::window::get_latest().then(|window| {
        let Some(window) = window else {
            return Task::done(Err(Uncaptured::NoWindow));
        };
        iced::window::get_minimized(window).then(move |minimized| {
            if minimized == Some(true) {
                return Task::done(Err(Uncaptured::Minimised));
            }
            iced::window::get_size(window).then(move |size| {
                if size.width < 1.0 || size.height < 1.0 {
                    Task::done(Err(Uncaptured::Minimised))
                } else {
                    iced::window::screenshot(window).map(Ok)
                }
            })
        })
    })
}

fn settle_timer(request: Request, remaining: u8, quiet: u8) -> Task<Message> {
    Task::perform(async { tokio::time::sleep(SETTLE_STEP).await }, move |()| {
        Message::ApiScreenshot(Step::Settle {
            request: request.clone(),
            remaining,
            quiet,
        })
    })
}

impl Studio {
    /// Whether the 3D view is still completing its picture: reading the
    /// points of its camera, or making the caps where the section box cuts
    /// a mesh. A picture taken now may lack some of either.
    pub(crate) fn scene_pending(&self) -> bool {
        self.detail_pending || self.section_fill.cap_jobs.running()
    }

    /// Where the part of the window that a screenshot captures lies: the
    /// sheet of the Drawing view while it is shown, else the 3D scene.
    pub(crate) fn shown_canvas_bounds(&self) -> Option<Rectangle> {
        if self.drawing_view.shown {
            self.drawing_view.canvas_bounds()
        } else {
            self.views.canvas_bounds()
        }
    }

    /// Start a screenshot for the command API; the answer is sent once the
    /// image has been taken and encoded.
    pub fn api_screenshot(
        &mut self,
        reply: Sender<Value>,
        path: Option<PathBuf>,
        base64: Option<bool>,
        max_edge: Option<u32>,
    ) -> Task<Message> {
        let failed = |error: &str| {
            let _ = reply.send(json!({"ok": false, "error": error}));
            Task::none()
        };
        if path.as_deref().is_some_and(|path| !is_png_path(path)) {
            return failed("screenshot requires an absolute .png path");
        }
        let max_edge = max_edge.unwrap_or(DEFAULT_MAX_EDGE);
        if !(MIN_MAX_EDGE..=MAX_MAX_EDGE).contains(&max_edge) {
            return failed("max_edge must be from 16 to 8192 pixels");
        }
        if self.model_covered() {
            return failed(COVERED);
        }
        if self.shown_canvas_bounds().is_none() && !self.drawing_view.shown {
            return failed("the viewport has not been drawn yet");
        }
        let request = Request {
            base64: base64.unwrap_or(path.is_none()),
            reply,
            path,
            max_edge,
        };
        settle_timer(request, SETTLE_STEPS, 0)
    }

    pub fn screenshot_step(&mut self, step: Step) -> Task<Message> {
        match step {
            Step::Settle {
                request,
                remaining,
                quiet,
            } => {
                // The drawing has no points or caps to wait for, only its
                // first frame after it was switched on.
                let waiting = if self.drawing_view.shown {
                    self.shown_canvas_bounds().is_none()
                } else {
                    self.scene_pending()
                };
                let quiet = if waiting { 0 } else { quiet + 1 };
                if quiet < QUIET_STEPS && remaining > 0 {
                    return settle_timer(request, remaining - 1, quiet);
                }
                capture_window().map(move |captured| {
                    Message::ApiScreenshot(Step::Captured(request.clone(), captured))
                })
            }
            Step::Captured(request, captured) => {
                let screenshot = match captured {
                    Ok(screenshot) => screenshot,
                    Err(reason) => {
                        let _ = request
                            .reply
                            .send(json!({"ok": false, "error": reason.message()}));
                        return Task::none();
                    }
                };
                let canvas = self.shown_canvas_bounds();
                let view = if self.drawing_view.shown {
                    "drawing"
                } else {
                    "model"
                };
                let (Some(canvas), false) = (canvas, self.model_covered()) else {
                    let _ = request
                        .reply
                        .send(json!({"ok": false, "error": Uncaptured::NoWindow.message()}));
                    return Task::none();
                };
                let model = !self.drawing_view.shown;
                let pending = Pending {
                    detail: self.detail_pending && model,
                    section_caps: self.section_fill.cap_jobs.running() && model,
                };
                Task::future(async move {
                    let reply = request.reply.clone();
                    let answer = tokio::task::spawn_blocking(move || {
                        let mut value = answer(&request, &screenshot, canvas, pending);
                        if value["ok"] == true {
                            value["view"] = view.into();
                        }
                        value
                    })
                    .await
                    .unwrap_or_else(|error| json!({"ok": false, "error": error.to_string()}));
                    let _ = reply.send(answer);
                })
                .discard()
            }
        }
    }
}

fn is_png_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
}

/// What the 3D view was still busy with when a screenshot was taken.
#[derive(Debug, Clone, Copy, Default)]
struct Pending {
    /// Reading the points of its camera.
    detail: bool,
    /// Making the caps of the cut of a mesh.
    section_caps: bool,
}

/// The answer to a screenshot command: the image cut from the window, as a
/// file and/or base64 text, with its size.
fn answer(
    request: &Request,
    screenshot: &Screenshot,
    canvas: Rectangle,
    pending: Pending,
) -> Value {
    let encoded = views::encode_viewport(
        &screenshot.bytes,
        screenshot.size.width,
        screenshot.size.height,
        screenshot.scale_factor,
        canvas,
        request.max_edge,
    );
    let (png, [width, height]) = match encoded {
        Ok(encoded) => encoded,
        Err(error) => return json!({"ok": false, "error": error}),
    };
    if let Some(path) = &request.path {
        if let Err(error) = write_atomically(path, &png) {
            return json!({"ok": false, "error": format!("could not write {}: {error}", path.display())});
        }
    }
    let mut value = json!({
        "ok": true,
        "width": width,
        "height": height,
        "bytes": png.len(),
        "path": request.path,
        "viewport_size": [canvas.width, canvas.height],
        "scale_factor": screenshot.scale_factor,
        "detail_pending": pending.detail,
        "section_caps_pending": pending.section_caps,
    });
    if request.base64 {
        value["png_base64"] = Value::String(base64(&png));
    }
    value
}

fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| std::io::Error::other("the path has no folder"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut push = |group: &[u8]| {
        let byte = |place: usize| u32::from(group.get(place).copied().unwrap_or(0));
        let value = (byte(0) << 16) | (byte(1) << 8) | byte(2);
        for place in 0..4 {
            if place <= group.len() {
                let index = (value >> (18 - 6 * place)) & 63;
                text.push(char::from(ALPHABET[index as usize]));
            } else {
                text.push('=');
            }
        }
    };
    let (groups, rest) = bytes.as_chunks::<3>();
    for group in groups {
        push(group);
    }
    if !rest.is_empty() {
        push(rest);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{Point, Size};

    fn request(path: Option<PathBuf>, base64: bool) -> (Request, std::sync::mpsc::Receiver<Value>) {
        let (reply, receive) = std::sync::mpsc::channel();
        (
            Request {
                reply,
                path,
                base64,
                max_edge: DEFAULT_MAX_EDGE,
            },
            receive,
        )
    }

    #[test]
    fn base64_matches_the_standard_alphabet_and_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
    }

    #[test]
    fn answer_cuts_the_canvas_and_writes_the_file_and_base64() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("scene.png");
        let screenshot = Screenshot::new(vec![90u8; 400 * 300 * 4], Size::new(400, 300), 2.0);
        let canvas = Rectangle::new(Point::new(10.0, 20.0), Size::new(150.0, 100.0));
        let (with_both, _) = request(Some(path.clone()), true);
        let value = answer(&with_both, &screenshot, canvas, Pending::default());
        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["width"], 300);
        assert_eq!(value["height"], 200);
        assert_eq!(value["viewport_size"], json!([150.0, 100.0]));
        let written = std::fs::read(&path).unwrap();
        assert_eq!(value["bytes"], written.len());
        assert_eq!(value["png_base64"], base64(&written));
        assert!(written.starts_with(b"\x89PNG\r\n\x1a\n"));

        let (file_only, _) = request(Some(path), false);
        let busy = Pending {
            detail: true,
            section_caps: false,
        };
        let value = answer(&file_only, &screenshot, canvas, busy);
        assert!(value.get("png_base64").is_none());
        assert_eq!(value["detail_pending"], true);
        assert_eq!(value["section_caps_pending"], false);
        let busy = Pending {
            detail: false,
            section_caps: true,
        };
        let value = answer(&file_only, &screenshot, canvas, busy);
        assert_eq!(value["detail_pending"], false);
        assert_eq!(value["section_caps_pending"], true);

        let outside = Rectangle::new(Point::new(500.0, 400.0), Size::new(10.0, 10.0));
        let (base64_only, _) = request(None, true);
        assert_eq!(
            answer(&base64_only, &screenshot, outside, Pending::default())["ok"],
            false
        );
    }

    #[test]
    fn pictures_wait_for_the_points_and_the_caps_of_the_view() {
        let mut studio = Studio::default();
        assert!(!studio.scene_pending());
        let caps = studio.section_fill.cap_jobs.start();
        assert!(studio.scene_pending());
        drop(caps);
        assert!(!studio.scene_pending());
        studio.detail_pending = true;
        assert!(studio.scene_pending());
    }

    #[test]
    fn invalid_requests_are_answered_at_once() {
        let mut studio = Studio::default();
        for (path, max_edge) in [
            (Some(PathBuf::from("relative.png")), None),
            (Some(std::env::temp_dir().join("scene.jpg")), None),
            (None, Some(8)),
            (None, Some(10_000)),
        ] {
            let (reply, receive) = std::sync::mpsc::channel();
            let _ = studio.api_screenshot(reply, path, None, max_edge);
            assert_eq!(receive.try_recv().unwrap()["ok"], false);
        }
        // Nothing has been drawn in a studio without a window.
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.api_screenshot(reply, None, None, None);
        let value = receive.try_recv().unwrap();
        assert_eq!(value["error"], "the viewport has not been drawn yet");
    }

    #[test]
    fn a_window_that_was_not_captured_is_answered_with_the_reason() {
        let mut studio = Studio::default();
        for (reason, error) in [
            (
                Uncaptured::Minimised,
                "the window is minimised; restore it to take a screenshot",
            ),
            (Uncaptured::NoWindow, "the viewport could not be captured"),
        ] {
            let (pending, receive) = request(None, true);
            let _ = studio.screenshot_step(Step::Captured(pending, Err(reason)));
            assert_eq!(
                receive.try_recv().unwrap(),
                json!({"ok": false, "error": error})
            );
        }
    }
}
