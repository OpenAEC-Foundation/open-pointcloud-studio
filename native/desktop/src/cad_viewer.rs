//! Opening a drawing or 3D geometry that was written as DXF or DWG in a CAD
//! program: Open CAD Studio, the open-source CAD application, which every
//! package carries beside the application. Without it, the program chosen in
//! Settings, an installed Open CAD Studio, or else the program the system
//! opens such files with. The window shows the file read-only, so looking at
//! an export never changes it.

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

/// The folder under `lib` beside the folder of the executable where the
/// Linux packages keep the Open CAD Studio they carry, out of the search path.
const PRIVATE_FOLDER: &str = "open-pointcloud-studio";

/// Where the Open CAD Studio that comes with the application can be, for the
/// executable of the application at `own`, in the order they are tried: on
/// Linux the private folder of the .deb and the AppImage, which the search
/// path does not reach; beside the executable (the Windows installer, the
/// archives and the macOS bundle); and for a development build in
/// `target/PROFILE/` the program that `packaging/build-open-cad-studio.sh`
/// writes to `target/open-cad-studio/release/`.
pub(crate) fn bundled_candidates(own: &Path) -> Vec<PathBuf> {
    let Some(folder) = own.parent() else {
        return Vec::new();
    };
    let above = folder.parent();
    let mut found = Vec::new();
    if cfg!(all(unix, not(target_os = "macos"))) {
        if let Some(above) = above {
            found.push(above.join("lib").join(PRIVATE_FOLDER).join(EXECUTABLE));
        }
    }
    found.push(folder.join(EXECUTABLE));
    if let Some(above) = above {
        found.push(
            above
                .join("open-cad-studio")
                .join("release")
                .join(EXECUTABLE),
        );
    }
    found
}

#[cfg(test)]
thread_local! {
    /// What the tests let `Places::of_system` find as the Open CAD Studio
    /// that comes with the application. Nothing is looked for beside the
    /// test program, so a build on the machine cannot change the outcome.
    pub(crate) static BUNDLED_IN_TESTS: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Where Open CAD Studio is looked for. Read from the system once per
/// lookup; the tests fill it with folders of their own.
#[derive(Debug, Clone, Default)]
pub(crate) struct Places {
    /// Where the Open CAD Studio that comes with the application can be:
    /// [`bundled_candidates`] of this executable.
    pub(crate) bundled: Vec<PathBuf>,
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
        #[cfg(not(test))]
        let bundled = std::env::current_exe()
            .map(|own| bundled_candidates(&own))
            .unwrap_or_default();
        #[cfg(test)]
        let bundled = BUNDLED_IN_TESTS.with(|bundled| bundled.borrow().clone());
        let mut places = Self {
            bundled,
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
    /// The Open CAD Studio that comes with the application.
    Bundled(PathBuf),
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
            Self::Bundled(path) | Self::Chosen(path) | Self::Installed(path) => Some(path),
            Self::SystemDefault { .. } => None,
        }
    }

    fn source(&self) -> &'static str {
        match self {
            Self::Bundled(_) => "bundled",
            Self::Chosen(_) => "setting",
            Self::Installed(_) => "installed",
            Self::SystemDefault { .. } => "system_default",
        }
    }
}

/// Find the viewer: the Open CAD Studio that comes with the application,
/// else the program chosen in Settings when it exists, else an installed
/// Open CAD Studio, else none. A chosen program that is missing does not
/// fall back to an installed Open CAD Studio, so that the choice is not
/// silently overruled; the system program opens the file instead.
pub(crate) fn find(chosen: Option<&Path>, places: &Places) -> Found {
    if let Some(bundled) = places.bundled.iter().find(|path| path.is_file()) {
        return Found::Bundled(bundled.clone());
    }
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
/// off, and the file. Open CAD Studio gives a file opened read-only a
/// process of its own, also while another one is running.
pub(crate) fn viewer_arguments(file: &Path) -> Vec<OsString> {
    vec!["--read-only".into(), file.as_os_str().to_owned()]
}

/// The variables the AppImage runtime sets for the program of its AppImage.
/// Every program that program starts inherits them and would take them for
/// its own: at each start Open CAD Studio registers the program `APPIMAGE`
/// names as the preview program for DWG files of the desktop, and as the
/// program for DWG and DXF files when the user agrees. From the AppImage of
/// this application that would be this application, which does not
/// understand the arguments they are started with. An AppImage started from
/// here sets them again for itself.
const APPIMAGE_VARIABLES: [&str; 4] = ["APPIMAGE", "APPDIR", "ARGV0", "OWD"];

/// How `program` is started as the viewer of `file`: with
/// [`viewer_arguments`], without input or output, and without
/// [`APPIMAGE_VARIABLES`].
fn viewer_command(program: &Path, file: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    command
        .args(viewer_arguments(file))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for name in APPIMAGE_VARIABLES {
        command.env_remove(name);
    }
    command
}

/// The folder for files the application can make again at any time:
/// `XDG_CACHE_HOME`, else `~/.cache`, with a folder of the application.
/// `variable` reads the environment.
fn cache_directory(variable: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let set = |name: &str| {
        variable(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let base = set("XDG_CACHE_HOME").or_else(|| set("HOME").map(|home| home.join(".cache")))?;
    Some(base.join("open-pointcloud-studio-native"))
}

/// The folder in the cache folder that holds the copy of the viewer, and the
/// file in it that names the version of the application that made the copy.
const COPY_FOLDER: &str = "open-cad-studio";
const COPY_VERSION: &str = "version";

/// The program to start for `executable`. A program inside a mounted
/// AppImage lies in a mount that goes away when the application ends, and a
/// viewer started from there would end with it; it is started from a copy in
/// `cache` instead. `appdir` is the folder the AppImage runtime announces.
/// Every program an AppImage starts inherits it, so it counts only when the
/// executable lies in it and it is a mount.
///
/// The copy has one place for every version of the application. It is made
/// again when `version`, the version of the application, or the size of the
/// program changed, and takes the place of the copy before it, so that no
/// copy of an earlier version is left in the cache, and what the viewer
/// registers for itself with the desktop (a preview program for DWG files,
/// and the program for DWG and DXF files when the user agrees) still exists
/// after an update. A viewer that still runs from the copy before keeps its
/// program: the copy is written beside and renamed over it.
fn runnable(
    executable: &Path,
    appdir: Option<&Path>,
    is_mount: impl Fn(&Path) -> bool,
    cache: Option<&Path>,
    version: &str,
) -> Result<PathBuf, String> {
    let in_mount =
        appdir.is_some_and(|appdir| crate::mcp::lies_in(executable, appdir) && is_mount(appdir));
    if !in_mount {
        return Ok(executable.to_path_buf());
    }
    let folder = cache
        .ok_or("there is no cache folder to start it from outside the AppImage")?
        .join(COPY_FOLDER);
    let copy = folder.join(EXECUTABLE);
    let made_by = folder.join(COPY_VERSION);
    let size = |path: &Path| std::fs::metadata(path).map(|metadata| metadata.len()).ok();
    let current = std::fs::read_to_string(&made_by).is_ok_and(|made| made == version)
        && size(&copy).is_some()
        && size(&copy) == size(executable);
    if current {
        return Ok(copy);
    }
    // Renamed into place only when it is whole, so that a copy cut short is
    // never run; named after this process, so that two windows that open a
    // drawing at the same moment do not write into one file. The version is
    // taken away first and written last, so that a copy that was cut short
    // after the rename is made again.
    let partial = folder.join(format!("{EXECUTABLE}.{}.partial", std::process::id()));
    let gone = |result: std::io::Result<()>| match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    let made = std::fs::create_dir_all(&folder)
        .and_then(|()| std::fs::copy(executable, &partial))
        .and_then(|_| gone(std::fs::remove_file(&made_by)))
        .and_then(|()| std::fs::rename(&partial, &copy))
        .and_then(|()| std::fs::write(&made_by, version));
    if let Err(error) = made {
        let _ = std::fs::remove_file(&partial);
        return Err(format!(
            "{} could not be copied out of the AppImage: {error}",
            executable.display()
        ));
    }
    Ok(copy)
}

/// Start the viewer with a file, or hand the file to the system, without
/// waiting for the program to close.
#[cfg(not(test))]
fn launch(found: &Found, file: &Path) -> Result<Opened, String> {
    match found.executable() {
        Some(executable) => {
            let program = runnable(
                executable,
                std::env::var_os("APPDIR").map(PathBuf::from).as_deref(),
                crate::mcp::is_mount_point,
                cache_directory(|name| std::env::var_os(name)).as_deref(),
                env!("CARGO_PKG_VERSION"),
            )?;
            let mut child = viewer_command(&program, file)
                .spawn()
                .map_err(|error| format!("{} could not start: {error}", program.display()))?;
            // Collect its exit so that no finished child is left behind.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(Opened::Viewer(program))
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
                .color(colors.text_muted),
            );
        }
        container(block).padding([4, 8]).into()
    }

    /// The field of Settings that chooses the viewer, with what was found.
    pub(crate) fn cad_viewer_setting(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let found = match self.cad_viewer.found() {
            Found::Bundled(path) => tr_args(
                "The Open CAD Studio that comes with the application opens exported drawings: {path}",
                &[("path", &path.display().to_string())],
            ),
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
            text(found).size(11).color(colors.text_muted),
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
            bundled: Vec::new(),
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
    fn bundled_viewer_lies_beside_the_application_or_in_the_build_folder() {
        let bin = Path::new("/opt/ops/bin");
        let candidates = bundled_candidates(&bin.join("open-pointcloud-studio"));
        let private = Path::new("/opt/ops/lib/open-pointcloud-studio").join(EXECUTABLE);
        if cfg!(all(unix, not(target_os = "macos"))) {
            assert_eq!(candidates[..2], [private, bin.join(EXECUTABLE)]);
        } else {
            assert_eq!(candidates.first(), Some(&bin.join(EXECUTABLE)));
            assert!(!candidates.contains(&private));
        }
        // A development build in target/debug finds the program that the
        // build script writes to target/open-cad-studio.
        let target = Path::new("/src/native/target");
        let candidates = bundled_candidates(&target.join("debug").join("open-pointcloud-studio"));
        assert_eq!(
            candidates.last(),
            Some(
                &target
                    .join("open-cad-studio")
                    .join("release")
                    .join(EXECUTABLE)
            )
        );
        assert!(bundled_candidates(Path::new("")).is_empty());
    }

    #[test]
    fn bundled_viewer_comes_before_the_chosen_and_the_installed_one() {
        let directory = tempfile::tempdir().unwrap();
        let system = directory.path().join("system");
        let installed = installed_path(&system);
        executable_at(&installed);
        let chosen = directory.path().join("tools").join("viewer.exe");
        executable_at(&chosen);
        let application = directory.path().join("app").join("bin");
        let places = Places {
            bundled: bundled_candidates(&application.join("open-pointcloud-studio")),
            system_programs: vec![system],
            ..Places::default()
        };
        assert_eq!(find(Some(&chosen), &places), Found::Chosen(chosen.clone()));
        assert_eq!(find(None, &places), Found::Installed(installed));

        let built = directory
            .path()
            .join("app")
            .join("open-cad-studio")
            .join("release")
            .join(EXECUTABLE);
        executable_at(&built);
        assert_eq!(find(Some(&chosen), &places), Found::Bundled(built.clone()));
        assert_eq!(find(None, &places), Found::Bundled(built));

        // The program beside the application comes before the build, and a
        // chosen program that is missing changes nothing while it is there.
        let beside = application.join(EXECUTABLE);
        executable_at(&beside);
        let missing = directory.path().join("gone.exe");
        assert_eq!(
            find(Some(&missing), &places),
            Found::Bundled(beside.clone())
        );
        assert_eq!(find(Some(&chosen), &places), Found::Bundled(beside));
    }

    #[test]
    fn viewer_inside_a_mounted_appimage_runs_from_a_copy() {
        let directory = tempfile::tempdir().unwrap();
        let mount = directory.path().join("mount");
        let inside = mount
            .join("usr")
            .join("lib")
            .join("open-pointcloud-studio")
            .join(EXECUTABLE);
        executable_at(&inside);
        let cache = directory.path().join("cache");
        let mounted = |_: &Path| true;
        let version = "1.0.0";

        // Outside the folder the runtime announces, or when that folder is no
        // mount, the program runs where it is.
        assert_eq!(
            runnable(&inside, None, mounted, Some(&cache), version),
            Ok(inside.clone())
        );
        assert_eq!(
            runnable(
                &inside,
                Some(&mount),
                |_: &Path| false,
                Some(&cache),
                version
            ),
            Ok(inside.clone())
        );
        let elsewhere = directory.path().join("opt").join(EXECUTABLE);
        assert_eq!(
            runnable(&elsewhere, Some(&mount), mounted, Some(&cache), version),
            Ok(elsewhere)
        );

        let copy = runnable(&inside, Some(&mount), mounted, Some(&cache), version).unwrap();
        assert_eq!(copy, cache.join(COPY_FOLDER).join(EXECUTABLE));
        assert_eq!(std::fs::read(&copy).unwrap(), b"not a real program");
        // Made once: a copy of the same size is used as it is.
        std::fs::write(&copy, b"NOT A REAL PROGRAM").unwrap();
        assert_eq!(
            runnable(&inside, Some(&mount), mounted, Some(&cache), version),
            Ok(copy.clone())
        );
        assert_eq!(std::fs::read(&copy).unwrap(), b"NOT A REAL PROGRAM");
        // Another program is copied again.
        std::fs::write(&inside, b"another program").unwrap();
        runnable(&inside, Some(&mount), mounted, Some(&cache), version).unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), b"another program");

        assert!(runnable(&inside, Some(&mount), mounted, None, version).is_err());
    }

    #[test]
    fn copy_of_the_viewer_is_replaced_in_its_place_by_another_version() {
        let directory = tempfile::tempdir().unwrap();
        let mount = directory.path().join("mount");
        let inside = mount.join("usr").join("bin").join(EXECUTABLE);
        executable_at(&inside);
        let cache = directory.path().join("cache");
        let mounted = |_: &Path| true;
        let copy_of =
            |version: &str| runnable(&inside, Some(&mount), mounted, Some(&cache), version);

        let first = copy_of("1.0.0").unwrap();
        let folder = cache.join(COPY_FOLDER);
        let made_by = || std::fs::read_to_string(folder.join(COPY_VERSION)).unwrap();
        assert_eq!(made_by(), "1.0.0");
        // The program of the next version has the same size; the copy is made
        // again all the same, in the same place, and nothing of the copy
        // before it is left.
        std::fs::write(&inside, b"NOT A REAL PROGRAM").unwrap();
        assert_eq!(copy_of("1.0.1"), Ok(first.clone()));
        assert_eq!(std::fs::read(&first).unwrap(), b"NOT A REAL PROGRAM");
        assert_eq!(made_by(), "1.0.1");
        let mut left: Vec<_> = std::fs::read_dir(&cache)
            .unwrap()
            .chain(std::fs::read_dir(&folder).unwrap())
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        let mut expected = [COPY_FOLDER, COPY_VERSION, EXECUTABLE].map(OsString::from);
        expected.sort();
        assert_eq!(left, expected);

        // A copy that was cut short after it was renamed into place has no
        // version yet and is made again, also when its size is right.
        std::fs::remove_file(folder.join(COPY_VERSION)).unwrap();
        std::fs::write(&inside, b"not a real program").unwrap();
        assert_eq!(copy_of("1.0.1"), Ok(first.clone()));
        assert_eq!(std::fs::read(&first).unwrap(), b"not a real program");
        assert_eq!(made_by(), "1.0.1");

        // A copy that cannot take its place is not left behind, and without
        // the version the next start makes it again.
        std::fs::remove_file(&first).unwrap();
        std::fs::create_dir_all(first.join("in the way")).unwrap();
        assert!(copy_of("1.0.2")
            .unwrap_err()
            .contains("could not be copied"));
        let left: Vec<_> = std::fs::read_dir(&folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, [OsString::from(EXECUTABLE)]);
        std::fs::remove_file(&inside).unwrap();
        assert!(copy_of("1.0.2").is_err());
    }

    #[test]
    fn cache_folder_follows_the_environment() {
        let environment = |pairs: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        };
        let folder = "open-pointcloud-studio-native";
        assert_eq!(
            cache_directory(environment(&[("XDG_CACHE_HOME", "/c"), ("HOME", "/h")])),
            Some(Path::new("/c").join(folder))
        );
        assert_eq!(
            cache_directory(environment(&[("XDG_CACHE_HOME", ""), ("HOME", "/h")])),
            Some(Path::new("/h").join(".cache").join(folder))
        );
        assert_eq!(cache_directory(environment(&[])), None);
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

    #[test]
    fn viewer_starts_without_the_variables_of_the_appimage_runtime() {
        let program = Path::new("/home/u/.cache/open-pointcloud-studio-native")
            .join("open-cad-studio")
            .join(EXECUTABLE);
        let file = Path::new("/out/plan.dxf");
        let command = viewer_command(&program, file);
        assert_eq!(command.get_program(), program.as_os_str());
        assert!(command.get_args().eq(viewer_arguments(file).iter()));
        // Each of them is taken out of what the program inherits, and
        // nothing else is changed.
        let mut changed: Vec<(String, Option<OsString>)> = command
            .get_envs()
            .map(|(name, value)| (name.to_string_lossy().into_owned(), value.map(Into::into)))
            .collect();
        changed.sort();
        let removed = ["APPDIR", "APPIMAGE", "ARGV0", "OWD"].map(|name| (name.to_owned(), None));
        assert_eq!(changed, removed);
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
        BUNDLED_IN_TESTS.with(|bundled| bundled.borrow_mut().clear());
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
    fn api_opens_drawings_in_the_bundled_viewer_before_the_chosen_one() {
        let directory = tempfile::tempdir().unwrap();
        let (mut studio, chosen) = studio_with_viewer(directory.path());
        let bundled = directory.path().join("application").join(EXECUTABLE);
        executable_at(&bundled);
        BUNDLED_IN_TESTS.with(|candidates| *candidates.borrow_mut() = vec![bundled.clone()]);

        let drawing = directory.path().join("plan.dxf");
        std::fs::write(&drawing, b"0\nEOF\n").unwrap();
        let answer = send(&mut studio, open(Some(drawing.clone())));
        assert_eq!(answer["ok"], true, "{answer}");
        assert_eq!(answer["viewer"], json!(bundled));
        assert_eq!(answer["read_only"], true);
        assert_eq!(
            launched(),
            [(Some(bundled.clone()), viewer_arguments(&drawing))]
        );

        let status = send(&mut studio, ApiCommand::Status);
        let reported = &status["result"]["cad_viewer"];
        assert_eq!(reported["source"], "bundled");
        assert_eq!(reported["path"], json!(bundled));
        assert_eq!(reported["chosen"], json!(chosen));
        assert_eq!(reported["chosen_missing"], false);
        for language in [Language::English, Language::from_key("nl").unwrap()] {
            let _language = crate::i18n::TestLanguage::hold(language);
            let _ = studio.cad_viewer_setting();
        }

        // Without it the chosen program is used again.
        BUNDLED_IN_TESTS.with(|candidates| candidates.borrow_mut().clear());
        studio.cad_viewer.refresh();
        assert_eq!(studio.cad_viewer.found(), &Found::Chosen(chosen));
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
