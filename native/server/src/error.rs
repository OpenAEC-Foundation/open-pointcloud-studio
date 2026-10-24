//! Every answer that is not a success: the JSON triple `{error_code,
//! melding, remedie}` of the shared contract, with its HTTP status, and the
//! scrubbing of the service's own folders out of every `melding`.

use std::path::{Path, PathBuf};

use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use pointcloud_core::ops;
use pointcloud_core::LoadError;
use serde_json::{json, Value};

/// Every HTTP status with the `error_code` it carries, as
/// `docs/cloud-api.md` lists them.
pub const CODES: [(u16, &str); 14] = [
    (400, "ARGUMENT_ONGELDIG"),
    (400, "LICHAAM_ONLEESBAAR"),
    (401, "TOEGANG_GEWEIGERD"),
    (403, "HERKOMST_GEWEIGERD"),
    (404, "TOOL_ONBEKEND"),
    (404, "ROUTE_ONBEKEND"),
    (405, "METHODE_NIET_TOEGESTAAN"),
    (413, "LICHAAM_TE_GROOT"),
    (422, "TOOL_MISLUKT"),
    (422, "UITKOMST_TE_GROOT"),
    (500, "INTERNE_FOUT"),
    (503, "DRUK"),
    (503, "WERK_VASTGELOPEN"),
    (504, "TIJDSLIMIET"),
];

/// An error as the service answers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    /// What went wrong, in English.
    pub melding: String,
    /// What to do about it, when something can be done.
    pub remedie: Option<String>,
    /// One header the answer carries besides its type.
    pub header: Option<Header>,
}

/// The headers an error may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Header {
    /// `WWW-Authenticate: Bearer`, with 401.
    WwwAuthenticate,
    /// `Retry-After: 10`, with 503.
    RetryAfter,
}

impl ApiError {
    pub fn new(
        status: StatusCode,
        code: &'static str,
        melding: impl Into<String>,
        remedie: Option<&str>,
    ) -> Self {
        Self {
            status,
            code,
            melding: melding.into(),
            remedie: remedie.map(str::to_owned),
            header: None,
        }
    }

    /// The same error with one header more.
    pub fn with_header(mut self, header: Header) -> Self {
        self.header = Some(header);
        self
    }

    /// `{error_code, melding, remedie}`, with `remedie: null` without one.
    pub fn json(&self) -> Value {
        json!({
            "error_code": self.code,
            "melding": self.melding,
            "remedie": self.remedie,
        })
    }

    /// The same error with the folders of `scrub` taken out of its texts.
    pub fn scrubbed(mut self, scrub: &Scrub) -> Self {
        self.melding = scrub.text(&self.melding);
        self.remedie = self.remedie.map(|remedie| scrub.text(&remedie));
        self
    }

    /// 400: an argument that is wrong in type, range or form.
    pub fn argument(melding: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
            melding,
            Some("Send the arguments the input schema of the tool describes (GET /v1/tools)."),
        )
    }

    /// 400: a body that could not be read.
    pub fn unreadable(melding: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "LICHAAM_ONLEESBAAR",
            melding,
            Some("Send one JSON object as the body."),
        )
    }

    /// 401, with `WWW-Authenticate: Bearer`.
    pub fn no_access() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "TOEGANG_GEWEIGERD",
            "No valid API key.",
            Some("Send the key as `Authorization: Bearer <key>`."),
        )
        .with_header(Header::WwwAuthenticate)
    }

    /// 403: an `Origin` that is not listed.
    pub fn origin() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "HERKOMST_GEWEIGERD",
            "Requests from this origin are not allowed.",
            Some("Call the service from a program, or have the origin added to OPENAEC_API_HERKOMSTEN."),
        )
    }

    /// 404: a tool the service does not offer.
    pub fn unknown_tool(name: &str) -> Self {
        let shown: String = name.chars().take(64).collect();
        Self::new(
            StatusCode::NOT_FOUND,
            "TOOL_ONBEKEND",
            format!("The cloud API has no tool `{shown}`."),
            Some("Ask for the list with GET /v1/tools or tools/list."),
        )
    }

    /// 404: a route the service does not have.
    pub fn unknown_route() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "ROUTE_ONBEKEND",
            "The service has no such route.",
            Some("The routes are listed in GET /v1/openapi.json."),
        )
    }

    /// 405; the router adds `Allow`.
    pub fn wrong_method() -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "METHODE_NIET_TOEGESTAAN",
            "The route does not take this method.",
            Some("Use a method the Allow header names."),
        )
    }

    /// 413: a body above the limit.
    pub fn too_large(limit: usize) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "LICHAAM_TE_GROOT",
            format!(
                "The request is larger than the limit of {} MiB.",
                limit / (1024 * 1024)
            ),
            Some("Send a smaller file: crop it, thin it or compress it as LAZ first."),
        )
    }

    /// 422: a result above the output limit, known before or after the work.
    pub fn output_too_large(limit: u64) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "UITKOMST_TE_GROOT",
            format!(
                "The result would be larger than the output limit of {} MiB.",
                limit / (1024 * 1024)
            ),
            Some("Crop with box, or choose laz; for a drawing or a mesh, take a smaller box or fewer vertices."),
        )
    }

    /// 422: the core refused the work or failed in it. A slab without
    /// points gets the hint of its view as the remedy.
    pub fn tool_failed(error: &LoadError, view: Option<pointcloud_core::DrawingView>) -> Self {
        if let LoadError::InvalidData(reason) = error {
            if reason.starts_with(ops::EMPTY_SLAB) {
                let hint = view.map(ops::empty_slab_hint);
                return Self::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "TOOL_MISLUKT",
                    format!("Invalid point cloud: {}", ops::EMPTY_SLAB),
                    hint,
                );
            }
        }
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "TOOL_MISLUKT",
            error.to_string(),
            None,
        )
    }

    /// 500: a panic or a failure of the service itself.
    pub fn internal(melding: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNE_FOUT",
            melding,
            Some("Report this to the OpenAEC Foundation with the request."),
        )
    }

    /// 503 with `Retry-After: 10`: no place to compute.
    pub fn busy() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "DRUK",
            "Every place to compute is taken.",
            Some("Try again after the seconds Retry-After names."),
        )
        .with_header(Header::RetryAfter)
    }

    /// 504: the time limit passed; the work is cancelled.
    pub fn timeout(seconds: u64) -> Self {
        Self::new(
            StatusCode::GATEWAY_TIMEOUT,
            "TIJDSLIMIET",
            format!("The work took longer than the time limit of {seconds} s and was cancelled."),
            Some("Send a smaller file or a smaller box."),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&self.json()).unwrap_or_default();
        let mut response = (
            self.status,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/json"),
            )],
            body,
        )
            .into_response();
        match self.header {
            Some(Header::WwwAuthenticate) => {
                response
                    .headers_mut()
                    .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            }
            Some(Header::RetryAfter) => {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from_static("10"));
            }
            None => {}
        }
        response
    }
}

/// The folders whose names never leave the service: the work folder of a
/// call, the folder of all work folders and the system temp folder. A text
/// that names a file in one of them names the file alone.
#[derive(Debug, Clone, Default)]
pub struct Scrub {
    /// Longest first, so that a folder inside another goes first.
    folders: Vec<String>,
}

impl Scrub {
    pub fn new(folders: &[&Path]) -> Self {
        let mut texts: Vec<String> = Vec::new();
        for folder in folders {
            let forms: [Option<PathBuf>; 2] =
                [Some(folder.to_path_buf()), folder.canonicalize().ok()];
            for form in forms.into_iter().flatten() {
                let text = form.display().to_string();
                let text = text.trim_end_matches(['/', '\\']).to_owned();
                if !text.is_empty() && !texts.contains(&text) {
                    texts.push(text);
                }
            }
        }
        texts.sort_by_key(|text| std::cmp::Reverse(text.len()));
        Self { folders: texts }
    }

    /// `text` without the folders: `/work/abc/input.las changed` becomes
    /// `input.las changed`.
    pub fn text(&self, text: &str) -> String {
        let mut text = text.to_owned();
        for folder in &self.folders {
            for separator in ["/", "\\"] {
                text = text.replace(&format!("{folder}{separator}"), "");
            }
            text = text.replace(folder.as_str(), "");
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scrubbed_text_names_files_without_their_folders() {
        let scrub = Scrub::new(&[Path::new("/work/abc"), Path::new("/work")]);
        assert_eq!(
            scrub.text("/work/abc/input.las changed while it was read"),
            "input.las changed while it was read"
        );
        assert_eq!(
            scrub.text("cannot write /work/abc/out/input.dxf"),
            "cannot write out/input.dxf"
        );
        assert_eq!(scrub.text("/work/other/x.las"), "other/x.las");
        assert_eq!(scrub.text("C:\\work\\abc"), "C:\\work\\abc");
        let windows = Scrub::new(&[Path::new("C:\\Temp\\call-1")]);
        assert_eq!(
            windows.text("C:\\Temp\\call-1\\input.ply: gone"),
            "input.ply: gone"
        );
    }

    #[test]
    fn the_triple_has_its_three_keys() {
        let error = ApiError::unknown_route();
        let json = error.json();
        assert_eq!(json["error_code"], "ROUTE_ONBEKEND");
        assert!(json["melding"].is_string() && json["remedie"].is_string());
        let bare = ApiError::new(StatusCode::BAD_REQUEST, "X", "m", None);
        assert_eq!(bare.json()["remedie"], Value::Null);
    }
}
