//! Extensions: the optional features built into the application, which can
//! be switched off, and installed extensions. An installed extension is a
//! program in a folder of the settings, `extensions/<id>`, that drives the
//! window through the local API. It runs only when the user starts it, as a
//! process of its own, so that its failure never takes the window down.
//! This module holds the choices of the user, the extensions that are
//! installed and their runs, and the commands of the local API for them.

#[cfg(test)]
mod check_tests;
mod install;
pub(crate) mod manifest;
mod page;
mod run;
#[cfg(test)]
mod run_tests;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::svg;
use iced::Task;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::i18n::{key, tr};
use crate::native_api::{ApiRequest, Grant, PathRequest};
use crate::{Message, Studio};

pub use install::Staged;
pub use manifest::Manifest;
pub use run::RunEnd;

/// A feature that is built into the application and can be switched off.
pub struct Extension {
    /// The name the settings file and the local API know it by.
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub author: &'static str,
    pub category: &'static str,
    /// Whether it asks a service on the internet for data.
    pub uses_network: bool,
}

/// Downloading buildings of the Dutch 3D BAG register.
pub const BAG3D: &str = "bag3d";

/// Every built-in extension, in the order the page lists them. A further
/// one is an entry here and a check of `Extensions::enabled` where it is
/// offered.
pub const BUILT_IN: &[Extension] = &[Extension {
    id: BAG3D,
    name: key("3D BAG"),
    description: key(
        "Download building models for an area in RD New and show them as a mesh layer.",
    ),
    author: "OpenAEC Foundation",
    category: key("Import"),
    uses_network: true,
}];

/// A settings file larger than this is not one this application wrote.
const MAX_FILE_BYTES: u64 = 64 * 1024;

/// The longest message `show_message` shows and the longest text of a
/// progress.
const MAX_MESSAGE_CHARS: usize = 300;
const MAX_PROGRESS_CHARS: usize = 120;

/// Which extensions the user switched off, and which are installed with the
/// version installed. Everything else is on, so an extension that a later
/// version adds starts enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extensions {
    disabled: BTreeSet<String>,
    installed: BTreeMap<String, String>,
}

/// The settings file as it is read: an earlier version kept only the ids
/// that are switched off.
#[derive(Deserialize)]
#[serde(untagged)]
enum Stored {
    Disabled(BTreeSet<String>),
    Full {
        #[serde(default)]
        disabled: BTreeSet<String>,
        #[serde(default)]
        installed: BTreeMap<String, String>,
    },
}

/// One change a window makes to the settings. Another window with the same
/// settings folder may have changed the file since this one read it, so a
/// change is made to the file as it is then, and nothing else of it is
/// written over.
#[derive(Debug, Clone, Copy)]
enum Change<'a> {
    Switched(&'a str, bool),
    Installed(&'a str, &'a str),
    Forgotten(&'a str),
}

fn find(id: &str) -> Option<&'static Extension> {
    BUILT_IN.iter().find(|extension| extension.id == id)
}

/// The ids of the built-in extensions.
#[cfg(test)]
pub fn ids() -> Vec<&'static str> {
    BUILT_IN.iter().map(|extension| extension.id).collect()
}

fn settings_path() -> Option<PathBuf> {
    // Tests run beside an installed application and leave its settings as
    // they are.
    if cfg!(test) {
        return None;
    }
    crate::preferences::config_directory().map(|directory| directory.join("extensions.json"))
}

/// The folder installed extensions are kept in.
fn extensions_root() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    crate::preferences::config_directory().map(|directory| directory.join("extensions"))
}

/// The commands of the local API, which an extension may declare.
fn known_commands() -> Vec<&'static str> {
    crate::mcp::command_names()
}

impl Extensions {
    /// The choices of an earlier session; everything enabled without them.
    pub fn load() -> Self {
        settings_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// A missing, oversized or damaged file switches nothing off and
    /// installs nothing.
    fn load_from(path: &Path) -> Self {
        Self::read_from(path).unwrap_or_default()
    }

    /// The settings in a file, unless it is missing, oversized or damaged.
    /// A byte-order mark, as an editor may write, is skipped.
    fn read_from(path: &Path) -> Option<Self> {
        let small = fs::metadata(path).is_ok_and(|metadata| metadata.len() <= MAX_FILE_BYTES);
        let stored = small
            .then(|| fs::read(path).ok())
            .flatten()
            .and_then(|bytes| {
                let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
                serde_json::from_slice::<Stored>(bytes).ok()
            })?;
        Some(match stored {
            Stored::Disabled(disabled) => Self {
                disabled,
                installed: BTreeMap::new(),
            },
            Stored::Full {
                disabled,
                installed,
            } => Self {
                disabled,
                installed: installed
                    .into_iter()
                    .filter(|(id, version)| {
                        manifest::valid_id(id) && manifest::Version::parse(version).is_some()
                    })
                    .collect(),
            },
        })
    }

    pub fn save(&self) -> io::Result<()> {
        if cfg!(test) {
            return Ok(());
        }
        let path = settings_path().ok_or_else(|| io::Error::other("no user config directory"))?;
        self.save_to(&path)
    }

    /// Make one change to the settings file as it is now, which another
    /// window may have changed since this one read it. A file that cannot
    /// be read is replaced by the settings of this window.
    fn save_change(&self, path: &Path, change: Change<'_>) -> io::Result<()> {
        let mut stored = Self::read_from(path).unwrap_or_else(|| self.clone());
        stored.apply(change);
        stored.save_to(path)
    }

    fn apply(&mut self, change: Change<'_>) {
        match change {
            Change::Switched(id, true) => {
                self.disabled.remove(id);
            }
            Change::Switched(id, false) => {
                self.disabled.insert(id.to_owned());
            }
            Change::Installed(id, version) => self.record_install(id, version),
            Change::Forgotten(id) => self.forget(id),
        }
    }

    /// The ids that are switched off and the installed extensions, replaced
    /// in one step so a crash never leaves half a file.
    fn save_to(&self, path: &Path) -> io::Result<()> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("settings path has no parent"))?;
        fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(
            temporary.as_file_mut(),
            &json!({"disabled": self.disabled, "installed": self.installed}),
        )
        .map_err(io::Error::other)?;
        temporary.as_file_mut().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn enabled(&self, id: &str) -> bool {
        !self.disabled.contains(id)
    }

    pub fn is_installed(&self, id: &str) -> bool {
        self.installed.contains_key(id)
    }

    /// Switch an extension on or off. An id that is no extension is refused;
    /// ids a newer version stored are kept as they are.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        if find(id).is_none() && !self.is_installed(id) {
            return Err(format!("unknown extension {id}"));
        }
        self.apply(Change::Switched(id, enabled));
        Ok(())
    }

    /// Keep an installed version. A first install starts enabled; an update
    /// keeps the choice of the user.
    fn record_install(&mut self, id: &str, version: &str) {
        if self
            .installed
            .insert(id.to_owned(), version.to_owned())
            .is_none()
        {
            self.disabled.remove(id);
        }
    }

    /// Forget an extension that was uninstalled.
    fn forget(&mut self, id: &str) {
        self.installed.remove(id);
        self.disabled.remove(id);
    }

    /// The built-in extensions as the local API lists them, with their
    /// English texts.
    pub fn list(&self) -> Value {
        BUILT_IN
            .iter()
            .map(|extension| {
                json!({
                    "id": extension.id,
                    "name": extension.name,
                    "version": env!("CARGO_PKG_VERSION"),
                    "description": extension.description,
                    "author": extension.author,
                    "category": extension.category,
                    "builtin": true,
                    "uses_network": extension.uses_network,
                    "enabled": self.enabled(extension.id),
                })
            })
            .collect()
    }
}

/// An installed extension that was read and checked.
pub(crate) struct Installed {
    pub manifest: Manifest,
    pub folder: PathBuf,
    /// The icons of its ribbon buttons, by the id of the button.
    pub icons: BTreeMap<String, svg::Handle>,
}

/// An installed extension that could not be read, with the reason.
pub(crate) struct Problem {
    pub id: String,
    pub folder: PathBuf,
    pub error: String,
}

/// A run of an extension under way.
pub(crate) struct Run {
    pub number: u64,
    pub control: Arc<run::RunControl>,
    /// The token the run was given for the local API.
    pub token: String,
    /// The button or tile that started it.
    pub entry: Option<String>,
    pub started: Instant,
    /// What it reported last with `report_progress`.
    pub progress: Option<(f64, String)>,
    /// Whether it showed a message, which then stays when it ends well.
    pub said: bool,
}

/// What the dialog of the Extensions page asks.
pub(crate) enum Dialog {
    /// Confirm the install of a checked extension, over the version that is
    /// installed, if any.
    Install {
        staged: Box<Staged>,
        installed: Option<String>,
    },
    Uninstall(String),
}

/// The installed extensions, their runs and the dialogs about them.
#[derive(Default)]
pub struct Host {
    /// The folder the extensions are installed in and the settings file;
    /// none in tests and where the system has no settings folder.
    root: Option<PathBuf>,
    settings: Option<PathBuf>,
    pub(crate) installed: Vec<Installed>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) runs: BTreeMap<String, Run>,
    next_run: u64,
    pub(crate) dialog: Option<Dialog>,
    /// An extension is being copied and checked.
    pub(crate) preparing: bool,
    /// Why the last install or uninstall failed, for the page.
    pub(crate) last_error: Option<String>,
    /// A dialog of `choose_path` is open.
    path_dialog: bool,
    /// A progress reported through the local API outside a run.
    pub(crate) progress: Option<(f64, String)>,
    /// The extension whose request is handled, while it is.
    caller: Option<String>,
}

impl Host {
    /// The installed extensions of the settings.
    pub fn load(settings: &Extensions) -> Self {
        Self::at(extensions_root(), settings_path(), settings)
    }

    /// The installed extensions in a folder, with the settings file the
    /// choices are kept in.
    fn at(root: Option<PathBuf>, settings_file: Option<PathBuf>, settings: &Extensions) -> Self {
        let mut host = Self {
            root,
            settings: settings_file,
            ..Self::default()
        };
        if let Some(root) = &host.root {
            install::remove_leftovers(root);
        }
        for id in settings.installed.keys() {
            host.read(id);
        }
        host
    }

    /// Read an installed extension, or keep why it could not be read.
    fn read(&mut self, id: &str) {
        self.installed.retain(|each| each.manifest.id != id);
        self.problems.retain(|problem| problem.id != id);
        let Some(root) = &self.root else { return };
        let folder = root.join(id);
        match load_installed(&folder, id) {
            Ok(installed) => {
                self.installed.push(installed);
                self.installed.sort_by_key(|each| {
                    (
                        each.manifest.name.english().to_lowercase(),
                        each.manifest.id.clone(),
                    )
                });
            }
            Err(error) => self.problems.push(Problem {
                id: id.to_owned(),
                folder,
                error,
            }),
        }
    }

    pub(crate) fn find(&self, id: &str) -> Option<&Installed> {
        self.installed.iter().find(|each| each.manifest.id == id)
    }

    /// The name of an extension as the window shows it.
    fn name_of(&self, id: &str) -> String {
        match (find(id), self.find(id)) {
            (Some(extension), _) => tr(extension.name).to_owned(),
            (None, Some(installed)) => installed.manifest.name.get().to_owned(),
            (None, None) => id.to_owned(),
        }
    }

    /// Close the dialog; a staged install is thrown away.
    pub(crate) fn close_dialog(&mut self) -> bool {
        match self.dialog.take() {
            Some(Dialog::Install { staged, .. }) => {
                staged.discard();
                true
            }
            Some(Dialog::Uninstall(_)) => true,
            None => false,
        }
    }
}

/// Read and check an installed extension in its folder.
fn load_installed(folder: &Path, id: &str) -> Result<Installed, String> {
    if !folder.is_dir() {
        return Err("its folder is missing".into());
    }
    manifest::check_folder(folder, &[manifest::LOGS])?;
    let manifest = manifest::read(folder, &known_commands())?;
    if manifest.id != id {
        return Err(format!(
            "its {} names another id, {}",
            manifest::MANIFEST,
            manifest.id
        ));
    }
    let mut icons = BTreeMap::new();
    for button in &manifest.ribbon {
        let path = button
            .icon
            .split('/')
            .fold(folder.to_path_buf(), |path, part| path.join(part));
        let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", button.icon))?;
        icons.insert(button.id.clone(), svg::Handle::from_memory(bytes));
    }
    Ok(Installed {
        manifest,
        folder: folder.to_path_buf(),
        icons,
    })
}

/// What the Extensions page, its dialogs, the buttons and tiles of
/// extensions and the local API ask of them.
#[derive(Debug, Clone)]
pub enum ExtensionAction {
    /// Ask for the folder or archive of an extension to install.
    Install,
    InstallChosen(Option<PathBuf>),
    /// An extension was copied and checked, or why not; with the reply of
    /// `install_extension` when that asked.
    Staged(Result<Box<Staged>, String>, Option<Sender<Value>>),
    ConfirmInstall,
    /// Cancel the dialog that is open.
    CloseDialog,
    /// Ask whether to uninstall an extension.
    Uninstall(String),
    ConfirmUninstall,
    SetEnabled(String, bool),
    ShowFolder(String),
    OpenHomepage(String),
    /// A button or tile of an extension: start it with the arguments of
    /// that entry, or stop it while it runs.
    Press(String, Option<String>),
    Stop(String),
    /// A run ended: the extension, the number of the run and how.
    Ended(String, u64, RunEnd),
    /// The path a dialog of `choose_path` gave for a job, or none.
    PathChosen(String, Option<PathBuf>),
    /// A request of the local API sent with the token of a run of an
    /// extension.
    Api(String, Box<ApiRequest>),
}

/// A text of one line, without control characters and runs of spaces.
fn one_line(text: &str) -> String {
    text.split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn declined(error: impl Into<String>) -> Value {
    json!({"ok": false, "error": error.into()})
}

/// The answer of the local API to a switch. A caller cannot see the status
/// line, so it is told here when the choice holds for this session only.
fn switched_answer(id: &str, enabled: bool, unsaved: Option<String>) -> Value {
    let mut answer = json!({
        "ok": true,
        "id": id,
        "enabled": enabled,
        "saved": unsaved.is_none(),
    });
    if let Some(error) = unsaved {
        answer["save_error"] = json!(error);
    }
    answer
}

impl Studio {
    /// Keep a change to the extensions for later sessions, beside what other
    /// windows changed.
    fn save_extension_settings(&self, change: Change<'_>) -> Result<(), String> {
        match &self.extension_host.settings {
            Some(path) => self.extensions.save_change(path, change),
            None => self.extensions.save(),
        }
        .map_err(|error| error.to_string())
    }

    /// Switch an extension on or off, keep the choice for later sessions and
    /// take away what a switched-off extension had open. The switch holds for
    /// this session also when it could not be kept; the answer is then why
    /// not.
    pub(crate) fn set_extension_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<Option<String>, String> {
        self.extensions.set_enabled(id, enabled)?;
        let unsaved = self
            .save_extension_settings(Change::Switched(id, enabled))
            .err();
        Ok(self.extension_switched(id, enabled, unsaved))
    }

    /// What follows a switch in the window, given why the choice could not
    /// be saved, if so.
    fn extension_switched(
        &mut self,
        id: &str,
        enabled: bool,
        unsaved: Option<String>,
    ) -> Option<String> {
        let name = self.extension_host.name_of(id);
        self.status = match &unsaved {
            None if enabled => format!("{name} switched on"),
            None => format!("{name} switched off"),
            Some(error) => format!("Could not save the extension settings: {error}"),
        };
        if id == BAG3D && !enabled {
            self.leave_bag3d();
        }
        if !enabled {
            if let Some(run) = self.extension_host.runs.get(id) {
                run.control.stop();
            }
        }
        unsaved
    }

    pub(crate) fn api_set_extension_enabled(&mut self, id: &str, enabled: bool) -> Value {
        match self.set_extension_enabled(id, enabled) {
            Ok(unsaved) => switched_answer(id, self.extensions.enabled(id), unsaved),
            Err(error) => declined(error),
        }
    }

    /// The built-in and installed extensions as `list_extensions` lists
    /// them, with the installed extensions that could not be read.
    pub(crate) fn extensions_value(&self) -> Value {
        let mut list = match self.extensions.list() {
            Value::Array(list) => list,
            _ => Vec::new(),
        };
        for installed in &self.extension_host.installed {
            let manifest = &installed.manifest;
            let mut entry = manifest.value();
            entry["category"] = json!("Installed");
            entry["builtin"] = json!(false);
            entry["uses_network"] = json!(manifest.uses.network);
            entry["enabled"] = json!(self.extensions.enabled(&manifest.id));
            entry["folder"] = json!(installed.folder);
            entry["run"] = self
                .extension_host
                .runs
                .get(&manifest.id)
                .map_or(Value::Null, |run| self.run_value(&manifest.id, run));
            list.push(entry);
        }
        let problems: Vec<Value> = self
            .extension_host
            .problems
            .iter()
            .map(|problem| {
                json!({
                    "id": problem.id,
                    "folder": problem.folder,
                    "error": problem.error,
                    "enabled": self.extensions.enabled(&problem.id),
                })
            })
            .collect();
        json!({"ok": true, "extensions": list, "problems": problems})
    }

    fn run_value(&self, id: &str, run: &Run) -> Value {
        json!({
            "id": id,
            "name": self.extension_host.name_of(id),
            "run": run.number,
            "entry": run.entry,
            "seconds": (run.started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
            "percent": run.progress.as_ref().map(|(percent, _)| percent),
            "text": run.progress.as_ref().map(|(_, text)| text),
            "stopping": run.control.stopping(),
            "log": run.control.log_path,
            "pid": run.control.pid,
        })
    }

    /// What `status.result.extensions` reports: the runs under way, a
    /// progress reported outside a run and the dialog that is open.
    pub(crate) fn extensions_status(&self) -> Value {
        let host = &self.extension_host;
        let dialog = match &host.dialog {
            Some(Dialog::Install { staged, installed }) => json!({
                "kind": "install",
                "extension": staged.manifest.value(),
                "replaces": installed,
                "files": staged.files,
                "bytes": staged.bytes,
            }),
            Some(Dialog::Uninstall(id)) => json!({"kind": "uninstall", "id": id}),
            None => Value::Null,
        };
        json!({
            "running": host
                .runs
                .iter()
                .map(|(id, run)| self.run_value(id, run))
                .collect::<Vec<_>>(),
            "progress": host
                .progress
                .as_ref()
                .map(|(percent, text)| json!({"percent": percent, "text": text})),
            "dialog": dialog,
            "preparing": host.preparing,
            "path_dialog": host.path_dialog,
            "last_error": host.last_error,
        })
    }

    /// What the window shows, as `context` and the context file of a run
    /// give it.
    pub(crate) fn extension_context(&self) -> Value {
        let active = self
            .active
            .and_then(|index| self.clouds.get(index).map(|entry| (index, entry)));
        let tabs = self.tabs_value();
        let shown = tabs["active"]
            .as_u64()
            .and_then(|index| tabs["tabs"].get(usize::try_from(index).ok()?))
            .cloned()
            .unwrap_or(Value::Null);
        json!({
            "application": {
                "version": env!("CARGO_PKG_VERSION"),
                "language": crate::i18n::active_code(),
            },
            "active_scan": active.map(|(index, entry)| json!({
                "index": index,
                "path": entry.cloud.path,
                "points": entry.cloud.total_points,
                "remaining": entry.remaining_count(),
                "selected": entry.selection.as_ref().map_or(0, |mask| mask.count),
                "bounds": {"min": entry.bounds().min, "max": entry.bounds().max},
            })),
            "scans": self.clouds.len(),
            "selected_points": self.selected_total(),
            "section_box": self.section_value(),
            "shown": shown,
            "drawing_view": self.drawing_view.shown,
            "file_view": self.file_open,
        })
    }

    /// Carry out an action of the Extensions page, a button or tile of an
    /// extension, or a request of a run.
    pub(crate) fn extension_action(&mut self, action: ExtensionAction) -> Task<Message> {
        match action {
            ExtensionAction::Install => {
                if self.extension_host.root.is_none() {
                    self.status =
                        "Extensions cannot be installed: there is no settings folder".into();
                    return Task::none();
                }
                if self.extension_host.preparing || self.extension_host.dialog.is_some() {
                    return Task::none();
                }
                let title = tr("Install extension").to_owned();
                let filter = tr("Extension (.zip or extension.json)").to_owned();
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(title)
                            .add_filter(filter, &["zip", "json"])
                            .pick_file()
                            .await
                            .map(|file| file.path().to_path_buf())
                    },
                    |path| Message::Extension(ExtensionAction::InstallChosen(path)),
                );
            }
            ExtensionAction::InstallChosen(Some(path)) => return self.prepare_install(path, None),
            ExtensionAction::InstallChosen(None) => {}
            ExtensionAction::Staged(result, reply) => self.staged(result, reply),
            ExtensionAction::ConfirmInstall => self.confirm_install(),
            ExtensionAction::CloseDialog => {
                if matches!(self.extension_host.dialog, Some(Dialog::Install { .. })) {
                    self.status = "Install cancelled".into();
                }
                self.extension_host.close_dialog();
            }
            ExtensionAction::Uninstall(id) => {
                let known = self.extensions.is_installed(&id);
                if known && self.extension_host.dialog.is_none() {
                    self.extension_host.dialog = Some(Dialog::Uninstall(id));
                }
            }
            ExtensionAction::ConfirmUninstall => self.confirm_uninstall(),
            ExtensionAction::SetEnabled(id, enabled) => {
                if let Err(error) = self.set_extension_enabled(&id, enabled) {
                    self.status = error;
                }
            }
            ExtensionAction::ShowFolder(id) => {
                let host = &self.extension_host;
                let folder = host
                    .find(&id)
                    .map(|installed| installed.folder.clone())
                    .or_else(|| {
                        host.problems
                            .iter()
                            .find(|problem| problem.id == id)
                            .map(|problem| problem.folder.clone())
                    });
                if let Some(folder) = folder {
                    if let Err(error) = open::that(&folder) {
                        self.status = format!("Could not show {}: {error}", folder.display());
                    }
                }
            }
            ExtensionAction::OpenHomepage(id) => {
                let homepage = self
                    .extension_host
                    .find(&id)
                    .and_then(|installed| installed.manifest.homepage.clone());
                if let Some(homepage) = homepage {
                    if let Err(error) = open::that(&homepage) {
                        self.status = format!("Could not open {homepage}: {error}");
                    }
                }
            }
            ExtensionAction::Press(id, entry) => {
                if self.extension_host.runs.contains_key(&id) {
                    self.stop_extension(&id);
                } else {
                    // A tile of the File view returns to the model, as the
                    // other tiles do.
                    if entry.is_some() {
                        self.file_open = false;
                    }
                    match self.start_extension(&id, entry.as_deref()) {
                        Ok(task) => return task,
                        Err(error) => self.status = error,
                    }
                }
            }
            ExtensionAction::Stop(id) => {
                self.stop_extension(&id);
            }
            ExtensionAction::Ended(id, number, end) => self.extension_ended(&id, number, &end),
            ExtensionAction::PathChosen(job, path) => {
                self.extension_host.path_dialog = false;
                let state = match path {
                    Some(path) => {
                        json!({"state": "complete", "operation": "choose_path", "path": path})
                    }
                    None => json!({"state": "cancelled", "operation": "choose_path"}),
                };
                if let Some(entry) = self.api_jobs.get_mut(&job) {
                    *entry = state;
                }
            }
            ExtensionAction::Api(caller, request) => {
                self.extension_host.caller = Some(caller);
                let task = self.handle_api(*request);
                self.extension_host.caller = None;
                return task;
            }
        }
        Task::none()
    }

    /// Copy and check an extension off this thread; the dialog opens when
    /// that is done.
    fn prepare_install(&mut self, source: PathBuf, reply: Option<Sender<Value>>) -> Task<Message> {
        let refuse = |studio: &mut Self, error: String, reply: Option<Sender<Value>>| {
            if let Some(reply) = reply {
                let _ = reply.send(declined(error.clone()));
            }
            studio.status = format!("Could not install the extension: {error}");
            Task::none()
        };
        let Some(root) = self.extension_host.root.clone() else {
            return refuse(self, "there is no settings folder".into(), reply);
        };
        if self.extension_host.preparing || self.extension_host.dialog.is_some() {
            return refuse(
                self,
                "another install waits for its confirmation in the window".into(),
                reply,
            );
        }
        self.extension_host.preparing = true;
        self.extension_host.last_error = None;
        self.status = format!("Checking the extension in {}…", source.display());
        let known = known_commands();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || install::stage(&source, &root, &known))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|staged| staged)
                    .map(Box::new)
            },
            move |result| Message::Extension(ExtensionAction::Staged(result, reply.clone())),
        )
    }

    /// A copied and checked extension waits for the user; one that is older
    /// than the installed version, or could not be read, is refused.
    fn staged(&mut self, result: Result<Box<Staged>, String>, reply: Option<Sender<Value>>) {
        self.extension_host.preparing = false;
        let staged = result.and_then(|staged| {
            let installed = self
                .extension_host
                .find(&staged.manifest.id)
                .map(|installed| installed.manifest.version.clone());
            let older = installed.as_deref().is_some_and(|installed| {
                manifest::Version::parse(&staged.manifest.version)
                    < manifest::Version::parse(installed)
            });
            if older || self.extension_host.dialog.is_some() {
                staged.discard();
                return Err(if older {
                    format!(
                        "version {} is installed and this is the older version {}; uninstall it first to go back",
                        installed.unwrap_or_default(),
                        staged.manifest.version
                    )
                } else {
                    "another install waits for its confirmation in the window".into()
                });
            }
            Ok((staged, installed))
        });
        match staged {
            Ok((staged, installed)) => {
                if let Some(reply) = reply {
                    let _ = reply.send(json!({
                        "ok": true,
                        "confirmation": "shown",
                        "extension": staged.manifest.value(),
                        "replaces": installed,
                    }));
                }
                self.status = format!(
                    "Confirm the install of {} in the window",
                    staged.manifest.name.get()
                );
                self.extension_host.dialog = Some(Dialog::Install { staged, installed });
            }
            Err(error) => {
                if let Some(reply) = reply {
                    let _ = reply.send(declined(error.clone()));
                }
                self.status = format!("Could not install the extension: {error}");
                self.extension_host.last_error = Some(error);
            }
        }
    }

    /// Put the extension of the dialog in its place and keep it installed.
    fn confirm_install(&mut self) {
        let Some(Dialog::Install { staged, installed }) = self.extension_host.dialog.take() else {
            return;
        };
        let Some(root) = self.extension_host.root.clone() else {
            staged.discard();
            return;
        };
        let id = staged.manifest.id.clone();
        self.end_run_now(&id);
        match install::commit(&staged, &root) {
            Ok(_) => {
                let change = Change::Installed(&id, &staged.manifest.version);
                self.extensions.apply(change);
                let unsaved = self.save_extension_settings(change).err();
                self.extension_host.read(&id);
                self.extension_host.last_error = None;
                let name = staged.manifest.name.get();
                let version = &staged.manifest.version;
                self.status = match (unsaved, installed) {
                    (Some(error), _) => format!(
                        "Installed {name} {version}, but the extension settings could not be saved: {error}"
                    ),
                    (None, Some(old)) if old != *version => {
                        format!("Updated {name} from {old} to {version}")
                    }
                    (None, _) => format!("Installed {name} {version}"),
                };
            }
            Err(error) => {
                staged.discard();
                self.status = format!("Could not install the extension: {error}");
                self.extension_host.last_error = Some(error);
            }
        }
    }

    fn confirm_uninstall(&mut self) {
        let Some(Dialog::Uninstall(id)) = self.extension_host.dialog.take() else {
            return;
        };
        let Some(root) = self.extension_host.root.clone() else {
            return;
        };
        let name = self.extension_host.name_of(&id);
        self.end_run_now(&id);
        match install::remove(&root, &id) {
            Ok(()) => {
                self.extensions.apply(Change::Forgotten(&id));
                let unsaved = self.save_extension_settings(Change::Forgotten(&id)).err();
                self.extension_host
                    .installed
                    .retain(|each| each.manifest.id != id);
                self.extension_host
                    .problems
                    .retain(|problem| problem.id != id);
                self.extension_host.last_error = None;
                self.status = match unsaved {
                    None => format!("Uninstalled {name}"),
                    Some(error) => format!(
                        "Uninstalled {name}, but the extension settings could not be saved: {error}"
                    ),
                };
            }
            Err(error) => {
                self.status = format!("Could not uninstall {name}: {error}");
                self.extension_host.last_error = Some(error);
            }
        }
    }

    /// Start an installed extension, with the arguments of one of its
    /// buttons or tiles. The task waits for the end of the run.
    pub(crate) fn start_extension(
        &mut self,
        id: &str,
        entry: Option<&str>,
    ) -> Result<Task<Message>, String> {
        let host = &self.extension_host;
        let installed = host.find(id).ok_or_else(|| {
            if host.problems.iter().any(|problem| problem.id == id) {
                format!("extension {id} could not be read; see the Extensions page")
            } else {
                format!("unknown extension {id}")
            }
        })?;
        let name = installed.manifest.name.get().to_owned();
        if !self.extensions.enabled(id) {
            return Err(format!("{name} is switched off"));
        }
        if host.runs.contains_key(id) {
            return Err(format!("{name} runs already"));
        }
        let extra = match entry {
            Some(entry) => installed
                .manifest
                .args_of(entry)
                .ok_or_else(|| format!("{name} has no button or tile {entry}"))?
                .to_vec(),
            None => Vec::new(),
        };
        let Some(api) = self.api_handle.as_ref() else {
            return Err("The local API is not running, so extensions cannot run".into());
        };
        let token = uuid::Uuid::new_v4().to_string();
        let mut context = self.extension_context();
        context["extension"] = json!({
            "id": id,
            "version": installed.manifest.version,
            "folder": installed.folder,
        });
        context["entry"] = json!(entry);
        // The token may be used as soon as the program starts.
        api.grants.insert(
            token.clone(),
            Grant {
                extension: id.to_owned(),
                commands: installed.manifest.uses.commands.granted(),
            },
        );
        let started = run::start(&run::RunRequest {
            id,
            name: &name,
            version: &installed.manifest.version,
            folder: &installed.folder,
            launch: &installed.manifest.launch,
            extra_args: &extra,
            port: api.port,
            token: &token,
            context: &context,
        });
        let control = match started {
            Ok(control) => control,
            Err(error) => {
                api.grants.remove(&token);
                return Err(format!("{name} could not start: {error}"));
            }
        };
        self.extension_host.next_run += 1;
        let number = self.extension_host.next_run;
        self.extension_host.runs.insert(
            id.to_owned(),
            Run {
                number,
                control: Arc::clone(&control),
                token,
                entry: entry.map(str::to_owned),
                started: Instant::now(),
                progress: None,
                said: false,
            },
        );
        self.status = format!("{name} runs…");
        let id = id.to_owned();
        Ok(Task::perform(
            async move {
                tokio::task::spawn_blocking(move || control.wait())
                    .await
                    .unwrap_or_else(|error| RunEnd {
                        code: None,
                        stopped: false,
                        stderr_tail: error.to_string(),
                    })
            },
            move |end| Message::Extension(ExtensionAction::Ended(id.clone(), number, end)),
        ))
    }

    /// Ask a run to stop; it ends with `Ended` like any run.
    fn stop_extension(&mut self, id: &str) -> bool {
        let Some(run) = self.extension_host.runs.get(id) else {
            return false;
        };
        run.control.stop();
        self.status = format!("Stopping {}…", self.extension_host.name_of(id));
        true
    }

    /// End a run at once and wait a moment for it, before its folder is
    /// replaced or removed.
    fn end_run_now(&mut self, id: &str) {
        let Some(run) = self.extension_host.runs.remove(id) else {
            return;
        };
        run.control.stop();
        run.control.kill();
        let deadline = Instant::now() + Duration::from_secs(3);
        while run.control.try_end().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        if let Some(api) = &self.api_handle {
            api.grants.remove(&run.token);
        }
    }

    /// End every run, as the window closes.
    pub(crate) fn end_extension_runs(&mut self) {
        for run in self.extension_host.runs.values() {
            run.control.stop();
            run.control.kill();
        }
    }

    /// A run ended: its token stops working and the status bar says how it
    /// ended, unless it ended well after a message of its own.
    fn extension_ended(&mut self, id: &str, number: u64, end: &RunEnd) {
        let current = self
            .extension_host
            .runs
            .get(id)
            .is_some_and(|run| run.number == number);
        if !current {
            return;
        }
        let Some(run) = self.extension_host.runs.remove(id) else {
            return;
        };
        if let Some(api) = &self.api_handle {
            api.grants.remove(&run.token);
        }
        let name = self.extension_host.name_of(id);
        if end.stopped {
            self.status = format!("{name} stopped");
        } else if end.succeeded() {
            if !run.said {
                self.status = format!("{name} finished");
            }
        } else {
            let how = match end.code {
                Some(code) => format!("{name} failed with exit code {code}"),
                None => format!("{name} was ended by a signal"),
            };
            self.status = if end.stderr_tail.is_empty() {
                how
            } else {
                format!("{how}: {}", end.stderr_tail)
            };
        }
    }

    /// The name of the extension whose run sent the request under way.
    fn caller_name(&self) -> Option<String> {
        self.extension_host
            .caller
            .as_deref()
            .map(|id| self.extension_host.name_of(id))
    }

    /// `install_extension`: copy and check, then ask the user. The answer
    /// comes once the dialog is shown, or with the reason it is not.
    pub(crate) fn api_install_extension(
        &mut self,
        path: PathBuf,
        reply: Sender<Value>,
    ) -> Task<Message> {
        if !path.is_absolute() {
            let _ = reply.send(declined(
                "install_extension needs the absolute path of a folder, an extension.json or a .zip archive",
            ));
            return Task::none();
        }
        self.prepare_install(path, Some(reply))
    }

    pub(crate) fn api_run_extension(
        &mut self,
        id: &str,
        entry: Option<&str>,
    ) -> (Value, Task<Message>) {
        match self.start_extension(id, entry) {
            Ok(task) => {
                let run = &self.extension_host.runs[id];
                (
                    json!({
                        "ok": true,
                        "id": id,
                        "run": run.number,
                        "log": run.control.log_path,
                        "context": run.control.context_path,
                    }),
                    task,
                )
            }
            Err(error) => (declined(error), Task::none()),
        }
    }

    pub(crate) fn api_stop_extension(&mut self, id: &str) -> Value {
        if self.stop_extension(id) {
            json!({"ok": true, "id": id, "stopping": true})
        } else {
            declined(format!("extension {id} is not running"))
        }
    }

    /// `show_message`: a message in the status bar, after the name of the
    /// extension that sends it.
    pub(crate) fn api_show_message(&mut self, text: &str) -> Value {
        let text = one_line(text);
        if text.is_empty() {
            return declined("text is empty");
        }
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return declined(format!(
                "text is longer than {MAX_MESSAGE_CHARS} characters"
            ));
        }
        self.status = match self.caller_name() {
            Some(name) => format!("{name}: {text}"),
            None => text,
        };
        if let Some(run) = self
            .extension_host
            .caller
            .clone()
            .and_then(|id| self.extension_host.runs.get_mut(&id))
        {
            run.said = true;
        }
        json!({"ok": true, "status": self.status})
    }

    /// `report_progress`: how far a run is, beside its name in the status
    /// bar; outside a run, in the status line.
    pub(crate) fn api_report_progress(&mut self, percent: f64, text: Option<&str>) -> Value {
        if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
            return declined("percent must be a number from 0 to 100");
        }
        let text = text.map(one_line).unwrap_or_default();
        if text.chars().count() > MAX_PROGRESS_CHARS {
            return declined(format!(
                "text is longer than {MAX_PROGRESS_CHARS} characters"
            ));
        }
        let caller = self.extension_host.caller.clone();
        match caller.and_then(|id| self.extension_host.runs.get_mut(&id)) {
            Some(run) => run.progress = Some((percent, text.clone())),
            None => {
                self.status = if text.is_empty() {
                    format!("{percent:.0}%")
                } else {
                    format!("{text} ({percent:.0}%)")
                };
                // A task that reports it is done leaves no progress behind.
                self.extension_host.progress = (percent < 100.0).then(|| (percent, text.clone()));
            }
        }
        json!({"ok": true, "percent": percent, "text": text})
    }

    /// `choose_path`: a dialog of the window that asks for a file or a
    /// folder; the path arrives as the result of a job.
    pub(crate) fn api_choose_path(&mut self, options: PathRequest) -> (Value, Task<Message>) {
        let refuse = |error: String| (declined(error), Task::none());
        let mode = options.mode.to_ascii_lowercase();
        if !["open", "save", "folder"].contains(&mode.as_str()) {
            return refuse("mode must be open, save or folder".into());
        }
        let title = options.title.as_deref().map(one_line).unwrap_or_default();
        if title.chars().count() > 120 {
            return refuse("title is longer than 120 characters".into());
        }
        if options.filters.len() > 8 {
            return refuse("give at most 8 filters".into());
        }
        for filter in &options.filters {
            let name = filter.name.trim();
            if name.is_empty() || name.chars().count() > 60 {
                return refuse("the name of a filter must be 1 to 60 characters".into());
            }
            let fits = !filter.extensions.is_empty()
                && filter.extensions.len() <= 16
                && filter.extensions.iter().all(|extension| {
                    (1..=16).contains(&extension.len())
                        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
                });
            if !fits {
                return refuse(
                    "a filter needs 1 to 16 extensions of letters and digits, without the dot"
                        .into(),
                );
            }
        }
        if let Some(name) = &options.file_name {
            if !manifest::valid_component(name) {
                return refuse("file_name must be a file name without folders".into());
            }
        }
        if options
            .directory
            .as_ref()
            .is_some_and(|directory| !directory.is_absolute())
        {
            return refuse("directory must be an absolute path".into());
        }
        if self.extension_host.path_dialog {
            return refuse("a dialog of choose_path is open already".into());
        }
        let title = if title.is_empty() {
            tr(match mode.as_str() {
                "open" => key("Choose a file"),
                "save" => key("Save as"),
                _ => key("Choose a folder"),
            })
            .to_owned()
        } else {
            title
        };
        // The dialog names the extension that asks.
        let title = match self.caller_name() {
            Some(name) => format!("{name}: {title}"),
            None => title,
        };
        let id = self.record_api_job(json!({"state": "running", "operation": "choose_path"}));
        self.extension_host.path_dialog = true;
        let job = id.clone();
        let task = Task::perform(
            async move {
                let mut dialog = rfd::AsyncFileDialog::new().set_title(title);
                for filter in &options.filters {
                    dialog = dialog.add_filter(filter.name.trim(), &filter.extensions);
                }
                if let Some(name) = &options.file_name {
                    dialog = dialog.set_file_name(name);
                }
                if let Some(directory) = &options.directory {
                    dialog = dialog.set_directory(directory);
                }
                let chosen = match mode.as_str() {
                    "open" => dialog.pick_file().await,
                    "save" => dialog.save_file().await,
                    _ => dialog.pick_folder().await,
                };
                chosen.map(|handle| handle.path().to_path_buf())
            },
            move |path| Message::Extension(ExtensionAction::PathChosen(job.clone(), path)),
        );
        (json!({"ok": true, "accepted": true, "job_id": id}), task)
    }

    /// `context`: what the window shows, and for a run its extension.
    pub(crate) fn api_context(&self) -> Value {
        let mut context = self.extension_context();
        if let Some(id) = &self.extension_host.caller {
            if let Some(installed) = self.extension_host.find(id) {
                context["extension"] = json!({
                    "id": id,
                    "version": installed.manifest.version,
                    "folder": installed.folder,
                });
            }
        }
        json!({"ok": true, "context": context})
    }
}
