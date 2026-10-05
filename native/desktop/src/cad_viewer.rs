//! Opening a drawing or 3D geometry that was written as DXF or DWG in a CAD
//! program: Open CAD Studio, the open-source CAD application, when it is
//! installed or chosen in Settings, and otherwise the program the system
//! opens such files with. The window shows the file read-only, so looking
//! at an export never changes it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use iced::widget::{button, checkbox, column, container, row, text};
use iced::{Element, Fill, Task};
use serde_json::{json, Value};

use crate::i18n::{tr, tr_args};
use crate::{flat_tool_style, muted_checkbox_style, Message, Studio};

/// The name of the CAD application in status lines and answers.
pub(crate) const VIEWER_NAME: &str = "Open CAD Studio";

/// The file name of its executable.
const EXECUTABLE: &str = if cfg!(windows) {
    "OpenCADStudio.exe"
} else {
    "OpenCADStudio"
};

/// Further names it has on the search path: the command of its snap.
const OTHER_COMMANDS: &[&str] = if cfg!(windows) {
    &[]
} else {
    &["open-cad-studio"]
};

/// Whether a path names a DXF or DWG file, by its extension.
pub(crate) fn is_cad_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("dxf") || extension.eq_ignore_ascii_case("dwg")
        })
}

/// Where an installed Open CAD Studio is looked for. Read from the system
/// once per lookup; the tests fill it with folders of their own.
#[derive(Debug, Clone, Default)]
pub(crate) struct Places {
    /// Folders of programs installed for every user: `%ProgramFiles%` on
    /// Windows, `/Applications` on macOS.
    pub(crate) system_programs: Vec<PathBuf>,
    /// Folders of programs installed for this user only:
    /// `%LOCALAPPDATA%\Programs` on Windows, `~/Applications` on macOS.
    pub(crate) user_programs: Vec<PathBuf>,
    /// The search path for commands.
    pub(crate) search_path: Option<OsString>,
}

impl Places {
    pub(crate) fn of_system() -> Self {
        let variable = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let mut places = Self {
            search_path: std::env::var_os("PATH"),
            ..Self::default()
        };
        if cfg!(windows) {
            for name in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
                if let Some(folder) = variable(name) {
                    if !places.system_programs.contains(&folder) {
                        places.system_programs.push(folder);
                    }
                }
            }
            if let Some(local) = variable("LOCALAPPDATA") {
                places.user_programs.push(local.join("Programs"));
            }
        } else if cfg!(target_os = "macos") {
            places.system_programs.push(PathBuf::from("/Applications"));
            if let Some(home) = variable("HOME") {
                places.user_programs.push(home.join("Applications"));
            }
        } else {
            // A snap puts its commands here, also when the session path
            // leaves the folder out.
            places.system_programs.push(PathBuf::from("/snap/bin"));
        }
        places
    }

    /// Every path the executable can have, in the order they are tried.
    fn candidates(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let installed: Vec<&PathBuf> = self
            .system_programs
            .iter()
            .chain(&self.user_programs)
            .collect();
        for folder in &installed {
            if cfg!(target_os = "macos") {
                for bundle in ["OpenCADStudio.app", "Open CAD Studio.app"] {
                    found.push(bundle_executable(&folder.join(bundle)));
                }
            } else if cfg!(windows) {
                for name in ["Open CAD Studio", "OpenCADStudio"] {
                    found.push(folder.join(name).join(EXECUTABLE));
                }
            } else {
                for name in std::iter::once(EXECUTABLE).chain(OTHER_COMMANDS.iter().copied()) {
                    found.push(folder.join(name));
                }
            }
        }
        if let Some(search_path) = &self.search_path {
            for folder in std::env::split_paths(search_path) {
                for name in std::iter::once(EXECUTABLE).chain(OTHER_COMMANDS.iter().copied()) {
                    found.push(folder.join(name));
                }
            }
        }
        found
    }
}

/// The program inside a macOS application bundle.
fn bundle_executable(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("MacOS").join("OpenCADStudio")
}

/// The program a chosen path stands for: the path itself, or the program
/// inside it when it is a macOS application bundle.
fn chosen_executable(chosen: &Path) -> Option<PathBuf> {
    if chosen.is_file() {
        return Some(chosen.to_path_buf());
    }
    let inside = bundle_executable(chosen);
    (chosen.is_dir() && inside.is_file()).then_some(inside)
}

/// How the viewer was found, as `status` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    /// The program chosen in Settings.
    Chosen(PathBuf),
    /// An Open CAD Studio found where it is installed or on the search path.
    Installed(PathBuf),
    /// None: files open in the program the system has for them. `missing`
    /// is a chosen program that does not exist.
    SystemDefault { missing: Option<PathBuf> },
}

impl Found {
    pub(crate) fn executable(&self) -> Option<&Path> {
        match self {
            Self::Chosen(path) | Self::Installed(path) => Some(path),
            Self::SystemDefault { .. } => None,
        }
    }

    fn source(&self) -> &'static str {
        match self {
            Self::Chosen(_) => "setting",
            Self::Installed(_) => "installed",
            Self::SystemDefault { .. } => "system_default",
        }
    }
}

/// Find the viewer: the program chosen in Settings when it exists, else an
/// installed Open CAD Studio, else none. A chosen program that is missing
/// does not fall back to another Open CAD Studio, so that the choice is not
/// silently overruled; the system program opens the file instead.
pub(crate) fn find(chosen: Option<&Path>, places: &Places) -> Found {
    if let Some(chosen) = chosen {
        return chosen_executable(chosen).map_or_else(
            || Found::SystemDefault {
                missing: Some(chosen.to_path_buf()),
            },
            Found::Chosen,
        );
    }
    places
        .candidates()
        .into_iter()
        .find(|path| path.is_file())
        .map_or(Found::SystemDefault { missing: None }, Found::Installed)
}

/// What opened a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Opened {
    Viewer(PathBuf),
    SystemDefault,
}

/// The arguments the viewer is started with: read-only, so that saving is
/// off, and the file. A running viewer takes the file as a further tab.
pub(crate) fn viewer_arguments(file: &Path) -> Vec<OsString> {
    vec!["--read-only".into(), file.as_os_str().to_owned()]
}

/// Start the viewer with a file, or hand the file to the system, without
/// waiting for the program to close.
#[cfg(not(test))]
fn launch(found: &Found, file: &Path) -> Result<Opened, String> {
    match found.executable() {
        Some(executable) => {
            let mut child = std::process::Command::new(executable)
                .args(viewer_arguments(file))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|error| format!("{} could not start: {error}", executable.display()))?;
            // Collect its exit so that no finished child is left behind.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(Opened::Viewer(executable.to_path_buf()))
        }
        None => open::that_detached(file)
            .map(|()| Opened::SystemDefault)
            .map_err(|error| format!("the system has no program for it: {error}")),
    }
}

#[cfg(test)]
thread_local! {
    /// What the tests asked to start, in place of starting it.
    pub(crate) static LAUNCHED: std::cell::RefCell<Vec<(Option<PathBuf>, Vec<OsString>)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests start nothing: they note what would have been started.
#[cfg(test)]
fn launch(found: &Found, file: &Path) -> Result<Opened, String> {
    let executable = found.executable().map(Path::to_path_buf);
    LAUNCHED.with(|launched| {
        launched.borrow_mut().push((
            executable.clone(),
            executable.as_ref().map_or_else(
                || vec![file.as_os_str().to_owned()],
                |_| viewer_arguments(file),
            ),
        ));
    });
    Ok(executable.map_or(Opened::SystemDefault, Opened::Viewer))
}

/// The CAD viewer of the window: the program chosen in Settings, the one
/// found, whether an export opens by itself and the last file written.
#[derive(Debug, Clone)]
pub(crate) struct CadViewer {
    /// The text of the field in Settings; empty to look for Open CAD Studio.
    pub(crate) input: String,
    pub(crate) open_after_export: bool,
    /// The last DXF or DWG file a drawing, faces or mesh export wrote.
    pub(crate) last: Option<PathBuf>,
    found: Found,
}

impl CadViewer {
    pub(crate) fn new(chosen: Option<&Path>, open_after_export: bool) -> Self {
        let input = chosen
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        Self {
            found: find(chosen, &Places::of_system()),
            input,
            open_after_export,
            last: None,
        }
    }

    /// The program chosen in Settings, if one is.
    pub(crate) fn chosen(&self) -> Option<PathBuf> {
        let trimmed = self.input.trim().trim_matches('"');
        (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
    }

    /// Look again, after a choice in Settings or before a file is opened:
    /// the program can have been installed or removed in the meantime.
    pub(crate) fn refresh(&mut self) {
        self.refresh_in(&Places::of_system());
    }

    pub(crate) fn refresh_in(&mut self, places: &Places) {
        self.found = find(self.chosen().as_deref(), places);
    }

    pub(crate) fn found(&self) -> &Found {
        &self.found
    }

    /// The viewer as `status` reports it.
    pub(crate) fn value(&self) -> Value {
        json!({
            "path": self.found.executable(),
            "source": self.found.source(),
            "chosen": self.chosen(),
            "chosen_missing": matches!(self.found, Found::SystemDefault { missing: Some(_) }),
            "open_after_export": self.open_after_export,
            "last_export": self.last,
        })
    }
}

/// What the CAD viewer controls of Properties and Settings ask for.
#[derive(Debug, Clone)]
pub enum CadAction {
    /// Open the last DXF or DWG file that was written.
    OpenLast,
    OpenAfterExport(bool),
    /// The field of Settings changed.
    Path(String),
    /// Choose the program in a file dialog.
    Browse,
    Chosen(Option<PathBuf>),
}

impl Studio {
    pub(crate) fn update_cad_viewer(&mut self, action: CadAction) -> Task<Message> {
        match action {
            CadAction::OpenLast => {
                let _ = self.open_in_cad_viewer(None);
            }
            CadAction::OpenAfterExport(on) => {
                self.cad_viewer.open_after_export = on;
                return self.queue_preferences_save();
            }
            CadAction::Path(input) => {
                self.cad_viewer.input = input;
                self.cad_viewer.refresh();
            }
            CadAction::Browse => {
                let mut dialog = rfd::AsyncFileDialog::new().set_title(tr("Choose the CAD viewer"));
                if cfg!(windows) {
                    dialog = dialog.add_filter(tr("Programs"), &["exe"]);
                }
                return Task::perform(
                    async move {
                        dialog
                            .pick_file()
                            .await
                            .map(|file| file.path().to_path_buf())
                    },
                    |path| Message::CadViewer(CadAction::Chosen(path)),
                );
            }
            CadAction::Chosen(Some(path)) => {
                self.cad_viewer.input = path.display().to_string();
                self.cad_viewer.refresh();
            }
            CadAction::Chosen(None) => {}
        }
        Task::none()
    }

    /// A drawing, faces or a mesh was written to `path`: keep it as the file
    /// the viewer opens, and open it when that was asked. The status line
    /// keeps what the export said and adds how it was opened.
    pub(crate) fn cad_file_written(&mut self, path: &Path) {
        if !is_cad_file(path) {
            return;
        }
        self.cad_viewer.last = Some(path.to_path_buf());
        if self.cad_viewer.open_after_export {
            let exported = std::mem::take(&mut self.status);
            let _ = self.open_in_cad_viewer(None);
            let opened = std::mem::take(&mut self.status);
            self.status = format!("{exported}; {opened}");
        }
    }

    /// Open a DXF or DWG file, the last one written when none is given, in
    /// the viewer, and say in the status line how that went.
    pub(crate) fn open_in_cad_viewer(&mut self, path: Option<PathBuf>) -> Result<Value, String> {
        let result = self.open_cad_file(path);
        match &result {
            Ok(answer) => {
                let file = answer["path"].as_str().unwrap_or_default();
                self.status = match answer["viewer"].as_str() {
                    Some(_) => format!("Opened {file} in {VIEWER_NAME}, read-only"),
                    None => format!("Opened {file} in the program the system has for it"),
                };
            }
            Err(error) => self.status = format!("Could not open the drawing: {error}"),
        }
        result
    }

    fn open_cad_file(&mut self, path: Option<PathBuf>) -> Result<Value, String> {
        let file = match path {
            Some(path) => path,
            None => self
                .cad_viewer
                .last
                .clone()
                .ok_or("no DXF or DWG file was exported yet")?,
        };
        if !file.is_absolute() || !is_cad_file(&file) {
            return Err("open_in_cad_viewer requires an absolute .dxf or .dwg path".into());
        }
        if !file.is_file() {
            return Err(format!("{} does not exist", file.display()));
        }
        self.cad_viewer.refresh();
        let opened = launch(self.cad_viewer.found(), &file)?;
        Ok(json!({
            "ok": true,
            "path": file,
            "viewer": match &opened {
                Opened::Viewer(executable) => Some(executable),
                Opened::SystemDefault => None,
            },
            "read_only": matches!(opened, Opened::Viewer(_)),
        }))
    }

    /// The button that opens the last export and the switch that opens
    /// every export, below the result of a tool that writes DXF or DWG.
    pub(crate) fn cad_viewer_controls(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let mut block = column![row![
            button(text(tr("Open in CAD viewer")).size(11))
                .on_press_maybe(
                    self.cad_viewer
                        .last
                        .is_some()
                        .then_some(Message::CadViewer(CadAction::OpenLast)),
                )
                .style(flat_tool_style),
            checkbox(tr("Open after export"), self.cad_viewer.open_after_export)
                .on_toggle(|on| Message::CadViewer(CadAction::OpenAfterExport(on)))
                .style(muted_checkbox_style)
                .text_size(11)
                .size(12),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center)]
        .spacing(2)
        .width(Fill);
        if let Some(name) = self
            .cad_viewer
            .last
            .as_ref()
            .and_then(|path| path.file_name())
        {
            block = block.push(
                text(tr_args(
                    "Last export: {name}",
                    &[("name", &name.to_string_lossy())],
                ))
                .size(10)
                .color(colors.muted),
            );
        }
        container(block).padding([4, 8]).into()
    }

    /// The field of Settings that chooses the viewer, with what was found.
    pub(crate) fn cad_viewer_setting(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let found = match self.cad_viewer.found() {
            Found::Chosen(_) => tr("The chosen program opens exported drawings.").to_owned(),
            Found::Installed(path) => tr_args(
                "Found: {path}",
                &[("path", &path.display().to_string())],
            ),
            Found::SystemDefault { missing: Some(_) } => {
                tr("The chosen program does not exist: exported drawings open in the program the system has for DXF and DWG files.").to_owned()
            }
            Found::SystemDefault { missing: None } => {
                tr("No CAD viewer found: exported drawings open in the program the system has for DXF and DWG files. Install Open CAD Studio or choose a program.").to_owned()
            }
        };
        column![
            row![
                iced::widget::text_input(tr("Find Open CAD Studio"), &self.cad_viewer.input)
                    .on_input(|value| Message::CadViewer(CadAction::Path(value)))
                    .size(12)
                    .padding([4, 6])
                    .width(Fill),
                button(text(tr("Browse…")).size(12))
                    .on_press(Message::CadViewer(CadAction::Browse))
                    .style(flat_tool_style)
                    .padding([4, 10]),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            text(found).size(11).color(colors.muted),
        ]
        .spacing(6)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;
    use crate::native_api::{ApiCommand, ApiRequest};
    use crate::settings_dialog::SettingsAction;

    fn executable_at(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"not a real program").unwrap();
    }

    fn installed_path(folder: &Path) -> PathBuf {
        if cfg!(target_os = "macos") {
            bundle_executable(&folder.join("OpenCADStudio.app"))
        } else if cfg!(windows) {
            folder.join("Open CAD Studio").join(EXECUTABLE)
        } else {
            folder.join(EXECUTABLE)
        }
    }

    #[test]
    fn dxf_and_dwg_are_cad_files_whatever_their_case() {
        assert!(is_cad_file(Path::new("/a/plan.dxf")));
        assert!(is_cad_file(Path::new("/a/plan.DWG")));
        assert!(!is_cad_file(Path::new("/a/faces.ifc")));
        assert!(!is_cad_file(Path::new("/a/dxf")));
    }

    #[test]
    fn viewer_is_found_where_it_is_installed_and_on_the_search_path() {
        let directory = tempfile::tempdir().unwrap();
        let system = directory.path().join("system");
        let user = directory.path().join("user");
        let bin = directory.path().join("bin");
        let places = Places {
            system_programs: vec![system.clone()],
            user_programs: vec![user.clone()],
            search_path: Some(std::env::join_paths([&bin]).unwrap()),
        };
        assert_eq!(find(None, &places), Found::SystemDefault { missing: None });

        let on_path = bin.join(EXECUTABLE);
        executable_at(&on_path);
        assert_eq!(find(None, &places), Found::Installed(on_path));

        // An installed program comes before one on the search path, and one
        // for every user before one for this user.
        let for_user = installed_path(&user);
        executable_at(&for_user);
        assert_eq!(find(None, &places), Found::Installed(for_user));
        let for_all = installed_path(&system);
        executable_at(&for_all);
        assert_eq!(find(None, &places), Found::Installed(for_all));
    }

    #[test]
    fn chosen_program_wins_and_a_missing_one_falls_back_to_the_system() {
        let directory = tempfile::tempdir().unwrap();
        let system = directory.path().join("system");
        executable_at(&installed_path(&system));
        let places = Places {
            system_programs: vec![system],
            ..Places::default()
        };
        let chosen = directory.path().join("tools").join("viewer.exe");
        executable_at(&chosen);
        assert_eq!(find(Some(&chosen), &places), Found::Chosen(chosen.clone()));

        // A macOS bundle stands for the program inside it.
        let bundle = directory.path().join("Viewer.app");
        let inside = bundle_executable(&bundle);
        executable_at(&inside);
        assert_eq!(find(Some(&bundle), &places), Found::Chosen(inside));

        let missing = directory.path().join("gone.exe");
        assert_eq!(
            find(Some(&missing), &places),
            Found::SystemDefault {
                missing: Some(missing)
            }
        );
    }

    #[test]
    fn viewer_starts_read_only_with_the_file() {
        let file = Path::new("/out/plan.dxf");
        assert_eq!(
            viewer_arguments(file),
            [
                OsString::from("--read-only"),
                OsString::from("/out/plan.dxf")
            ]
        );
    }

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    fn open(path: Option<PathBuf>) -> ApiCommand {
        ApiCommand::OpenInCadViewer { path }
    }

    fn launched() -> Vec<(Option<PathBuf>, Vec<OsString>)> {
        LAUNCHED.with(|launched| launched.borrow().clone())
    }

    /// A window whose viewer is a fake program in `directory`, with the
    /// switch off whatever the settings of the user say.
    fn studio_with_viewer(directory: &Path) -> (Studio, PathBuf) {
        LAUNCHED.with(|launched| launched.borrow_mut().clear());
        let viewer = directory.join("viewer").join(EXECUTABLE);
        executable_at(&viewer);
        let mut studio = Studio::default();
        studio.cad_viewer.input = viewer.display().to_string();
        studio.cad_viewer.open_after_export = false;
        studio.cad_viewer.last = None;
        studio.cad_viewer.refresh();
        (studio, viewer)
    }

    #[test]
    fn api_opens_the_last_export_read_only_in_the_chosen_viewer() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, viewer) = studio_with_viewer(directory.path());
        let answer = send(&mut studio, open(None));
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["error"], "no DXF or DWG file was exported yet");
        assert!(studio.status.starts_with("Could not open the drawing: "));

        let drawing = directory.path().join("plan.dxf");
        std::fs::write(&drawing, b"0\nEOF\n").unwrap();
        // Other files that an export writes are no drawing to open.
        studio.cad_file_written(&directory.path().join("faces.ifc"));
        assert_eq!(studio.cad_viewer.last, None);
        studio.status = "Section drawing exported".into();
        studio.cad_file_written(&drawing);
        assert_eq!(studio.cad_viewer.last.as_ref(), Some(&drawing));
        assert_eq!(studio.status, "Section drawing exported");
        assert!(launched().is_empty());

        let answer = send(&mut studio, open(None));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["path"], json!(drawing));
        assert_eq!(answer["viewer"], json!(viewer));
        assert_eq!(answer["read_only"], true);
        assert_eq!(
            launched(),
            [(Some(viewer.clone()), viewer_arguments(&drawing))]
        );
        assert!(studio.status.starts_with("Opened ") && studio.status.contains(VIEWER_NAME));

        let status = send(&mut studio, ApiCommand::Status);
        let reported = &status["result"]["cad_viewer"];
        assert_eq!(reported["path"], json!(viewer));
        assert_eq!(reported["source"], "setting");
        assert_eq!(reported["chosen_missing"], false);
        assert_eq!(reported["open_after_export"], false);
        assert_eq!(reported["last_export"], json!(drawing));
    }

    #[test]
    fn api_refuses_what_is_no_existing_drawing() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_viewer(directory.path());
        let refused = |studio: &mut Studio, path: PathBuf| {
            let answer = send(studio, open(Some(path)));
            assert_eq!(answer["ok"], false);
            answer["error"].as_str().unwrap().to_owned()
        };
        let wrong = "open_in_cad_viewer requires an absolute .dxf or .dwg path";
        assert_eq!(refused(&mut studio, PathBuf::from("plan.dxf")), wrong);
        let model = directory.path().join("faces.ifc");
        std::fs::write(&model, b"ISO").unwrap();
        assert_eq!(refused(&mut studio, model), wrong);
        let missing = directory.path().join("gone.dwg");
        assert!(refused(&mut studio, missing).ends_with("does not exist"));
        assert!(launched().is_empty());
        // A refusal does not become the file the button opens.
        assert_eq!(studio.cad_viewer.last, None);
    }

    #[test]
    fn export_opens_by_itself_when_asked_and_without_a_viewer_in_the_system_program() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_viewer(directory.path());
        let _ = studio.update(Message::CadViewer(CadAction::OpenAfterExport(true)));
        assert!(studio.cad_viewer.open_after_export);
        assert!(studio.preferences().open_after_export);

        // A chosen program that is gone is not replaced by another one.
        let gone = directory.path().join("gone").join(EXECUTABLE);
        let _ = studio.update(Message::CadViewer(CadAction::Chosen(Some(gone.clone()))));
        assert_eq!(
            studio.cad_viewer.found(),
            &Found::SystemDefault {
                missing: Some(gone.clone())
            }
        );
        assert_eq!(studio.preferences().cad_viewer, Some(gone));

        let mesh = directory.path().join("room.DWG");
        std::fs::write(&mesh, b"AC1027").unwrap();
        studio.status = "Exported mesh".into();
        studio.cad_file_written(&mesh);
        assert_eq!(launched(), [(None, vec![mesh.as_os_str().to_owned()])]);
        assert_eq!(
            studio.status,
            format!(
                "Exported mesh; Opened {} in the program the system has for it",
                mesh.display()
            )
        );
        let status = send(&mut studio, ApiCommand::Status);
        assert_eq!(status["result"]["cad_viewer"]["source"], "system_default");
        assert_eq!(status["result"]["cad_viewer"]["chosen_missing"], true);
        assert_eq!(status["result"]["cad_viewer"]["path"], Value::Null);
    }

    #[test]
    fn controls_and_setting_are_shown_in_both_languages() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, _) = studio_with_viewer(directory.path());
        for language in [Language::English, Language::from_key("nl").unwrap()] {
            let _language = crate::i18n::TestLanguage::hold(language);
            let _ = studio.cad_viewer_controls();
            let _ = studio.cad_viewer_setting();
        }
        studio.cad_viewer.last = Some(directory.path().join("plan.dxf"));
        let _ = studio.cad_viewer_controls();
        // The dialog puts the field back on Cancel, and the language it
        // opened with too.
        let _language = crate::i18n::TestLanguage::hold(Language::English);
        let before = studio.cad_viewer.input.clone();
        studio.settings_action(SettingsAction::Open);
        let _ = studio.update(Message::CadViewer(CadAction::Path(String::new())));
        assert_eq!(studio.cad_viewer.chosen(), None);
        studio.settings_action(SettingsAction::Cancel);
        assert_eq!(studio.cad_viewer.input, before);
    }

    #[test]
    fn field_text_names_the_chosen_program() {
        let mut viewer = CadViewer::new(None, false);
        viewer.input = "  \"C:\\Tools\\viewer.exe\" ".into();
        assert_eq!(
            viewer.chosen(),
            Some(PathBuf::from("C:\\Tools\\viewer.exe"))
        );
        viewer.input = "   ".into();
        assert_eq!(viewer.chosen(), None);
        viewer.refresh_in(&Places::default());
        assert_eq!(viewer.value()["source"], "system_default");
        assert_eq!(viewer.value()["path"], Value::Null);
    }
}
