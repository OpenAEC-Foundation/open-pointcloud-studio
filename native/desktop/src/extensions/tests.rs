use super::*;
use crate::file_view::FilePage;
use crate::i18n::{Language, TestLanguage};
use crate::native_api::{ApiCommand, ApiRequest};
use manifest::EntryPage;

pub(super) fn send(studio: &mut Studio, command: ApiCommand) -> Value {
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.handle_api(ApiRequest { command, reply });
    receive.recv().unwrap()
}

/// A request sent with the token of a run of an extension.
pub(super) fn send_as(studio: &mut Studio, caller: &str, command: ApiCommand) -> Value {
    let (reply, receive) = std::sync::mpsc::channel();
    let _ = studio.update(Message::Extension(ExtensionAction::Api(
        caller.to_owned(),
        Box::new(ApiRequest { command, reply }),
    )));
    receive.recv().unwrap()
}

/// The commands of the local API, as an extension may declare them.
pub(super) fn known() -> Vec<&'static str> {
    known_commands()
}

/// The folder of the example extension in the repository.
pub(super) fn example_folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../extensions/examples/point-count-report")
}

pub(super) const EXAMPLE: &str = "org.openaec.point-count-report";

/// A copy of the example extension with another version and, when given,
/// another command.
pub(super) fn example_copy(parent: &Path, version: &str, command: Option<Value>) -> PathBuf {
    let folder = parent.join(format!("example-{version}"));
    fs::create_dir_all(&folder).unwrap();
    for entry in fs::read_dir(example_folder()).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_file() {
            fs::copy(entry.path(), folder.join(entry.file_name())).unwrap();
        }
    }
    let path = folder.join(manifest::MANIFEST);
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["version"] = json!(version);
    if let Some(command) = command {
        manifest["command"] = command;
    }
    fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    folder
}

/// The settings folder of a window: the installed extensions and the
/// settings file, in a temporary folder.
pub(super) struct Bench {
    pub directory: tempfile::TempDir,
    pub root: PathBuf,
    pub settings: PathBuf,
}

impl Bench {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("extensions");
        let settings = directory.path().join("extensions.json");
        Self {
            directory,
            root,
            settings,
        }
    }

    /// A window that reads the extensions of the bench, as one started anew
    /// does.
    pub fn studio(&self) -> Studio {
        let mut studio = Studio::default();
        studio.extensions = Extensions::load_from(&self.settings);
        studio.extension_host = Host::at(
            Some(self.root.clone()),
            Some(self.settings.clone()),
            &studio.extensions,
        );
        studio
    }

    pub fn stored(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.settings).unwrap()).unwrap()
    }

    /// What lies in the folder of the installed extensions.
    pub fn installed_folders(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.root)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

/// Copy and check an extension as Install extension… does, up to the
/// confirmation.
pub(super) fn stage_into(studio: &mut Studio, source: &Path) {
    let root = studio.extension_host.root.clone().unwrap();
    let staged = install::stage(source, &root, &known()).map(Box::new);
    let _ = studio.update(Message::Extension(ExtensionAction::Staged(staged, None)));
}

/// Install an extension and confirm it, as the user does.
pub(super) fn install_confirmed(studio: &mut Studio, source: &Path) {
    stage_into(studio, source);
    assert!(
        matches!(studio.extension_host.dialog, Some(Dialog::Install { .. })),
        "{}",
        studio.status
    );
    let _ = studio.update(Message::Extension(ExtensionAction::ConfirmInstall));
}

#[test]
fn everything_is_enabled_without_a_readable_settings_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config/extensions.json");
    assert_eq!(Extensions::load_from(&path), Extensions::default());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for damaged in ["", "{", r#"{"bag3d": false}"#, "[1, 2]", "null"] {
        fs::write(&path, damaged).unwrap();
        let extensions = Extensions::load_from(&path);
        assert!(extensions.enabled(BAG3D), "{damaged}");
    }
    // A file far larger than this application writes is not read.
    let padding = " ".repeat(MAX_FILE_BYTES as usize);
    fs::write(&path, format!(r#"["bag3d"{padding}]"#)).unwrap();
    assert!(Extensions::load_from(&path).enabled(BAG3D));
    // The tests themselves never touch the settings of the user.
    assert_eq!(settings_path(), None);
    assert_eq!(extensions_root(), None);
    assert_eq!(Extensions::load(), Extensions::default());
}

#[test]
fn a_switched_off_extension_is_kept_in_the_settings_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config/extensions.json");
    let mut extensions = Extensions::default();
    extensions.set_enabled(BAG3D, false).unwrap();
    assert!(!extensions.enabled(BAG3D));
    extensions.save_to(&path).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        json!({"disabled": ["bag3d"], "installed": {}})
    );
    let loaded = Extensions::load_from(&path);
    assert_eq!(loaded, extensions);

    let mut loaded = loaded;
    loaded.set_enabled(BAG3D, true).unwrap();
    loaded.save_to(&path).unwrap();
    assert!(Extensions::load_from(&path).enabled(BAG3D));

    // The choice for an extension of a newer version survives a save.
    fs::write(&path, r#"["bag3d", "later"]"#).unwrap();
    let mut newer = Extensions::load_from(&path);
    newer.set_enabled(BAG3D, true).unwrap();
    newer.save_to(&path).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
        json!({"disabled": ["later"], "installed": {}})
    );
}

#[test]
fn the_settings_file_is_read_with_a_byte_order_mark_and_in_both_forms() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("extensions.json");
    // An editor that writes a byte-order mark keeps the choices.
    fs::write(&path, b"\xEF\xBB\xBF[\"bag3d\"]").unwrap();
    assert!(!Extensions::load_from(&path).enabled(BAG3D));
    fs::write(
        &path,
        "\u{FEFF}{\"disabled\": [\"org.example.tool\"], \"installed\": {\"org.example.tool\": \"1.2.0\", \"Bad Id\": \"1.0\", \"org.example.other\": \"not a version\"}}",
    )
    .unwrap();
    let loaded = Extensions::load_from(&path);
    assert!(loaded.is_installed("org.example.tool"));
    assert!(!loaded.enabled("org.example.tool"));
    assert!(loaded.enabled(BAG3D));
    // Entries that no install could have written are left out.
    assert!(!loaded.is_installed("Bad Id"));
    assert!(!loaded.is_installed("org.example.other"));
    assert_eq!(loaded.installed.len(), 1);
}

#[test]
fn an_unknown_extension_is_refused() {
    let mut extensions = Extensions::default();
    assert_eq!(
        extensions.set_enabled("nothing", false),
        Err("unknown extension nothing".into())
    );
    assert_eq!(extensions, Extensions::default());
    let mut ids = ids();
    ids.dedup();
    assert_eq!(ids, [BAG3D], "every extension has its own id");
}

#[test]
fn extensions_are_listed_with_the_version_of_the_application() {
    let mut studio = Studio::default();
    let listed = send(&mut studio, ApiCommand::ListExtensions);
    assert_eq!(listed["ok"], true);
    assert_eq!(
        listed["extensions"],
        json!([{
            "id": "bag3d",
            "name": "3D BAG",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Download building models for an area in RD New and show them as a mesh layer.",
            "author": "OpenAEC Foundation",
            "category": "Import",
            "builtin": true,
            "uses_network": true,
            "enabled": true,
        }])
    );
    assert_eq!(listed["problems"], json!([]));

    let switched = send(
        &mut studio,
        ApiCommand::SetExtensionEnabled {
            id: "bag3d".into(),
            enabled: false,
        },
    );
    assert_eq!(
        switched,
        json!({"ok": true, "id": "bag3d", "enabled": false, "saved": true})
    );
    assert_eq!(studio.status, "3D BAG switched off");
    let listed = send(&mut studio, ApiCommand::ListExtensions);
    assert_eq!(listed["extensions"][0]["enabled"], false);

    let unknown = send(
        &mut studio,
        ApiCommand::SetExtensionEnabled {
            id: "nothing".into(),
            enabled: true,
        },
    );
    assert_eq!(
        unknown,
        json!({"ok": false, "error": "unknown extension nothing"})
    );
}

#[test]
fn a_choice_that_could_not_be_saved_is_reported_to_the_caller() {
    // A configuration folder that cannot be written: its place is taken
    // by a file.
    let directory = tempfile::tempdir().unwrap();
    let blocked = directory.path().join("config");
    fs::write(&blocked, "").unwrap();
    let mut studio = Studio::default();
    studio.extensions.set_enabled(BAG3D, false).unwrap();
    let unsaved = studio
        .extensions
        .save_to(&blocked.join("extensions.json"))
        .err()
        .map(|error| error.to_string());
    let reason = unsaved.clone().expect("the save fails");

    // The switch took effect for this session, and the window says that
    // it was not kept.
    let unsaved = studio.extension_switched(BAG3D, false, unsaved);
    assert_eq!(
        studio.status,
        format!("Could not save the extension settings: {reason}")
    );
    assert!(!studio.extensions.enabled(BAG3D));
    assert_eq!(
        switched_answer(BAG3D, false, unsaved),
        json!({
            "ok": true,
            "id": "bag3d",
            "enabled": false,
            "saved": false,
            "save_error": reason,
        })
    );
    assert_eq!(
        switched_answer(BAG3D, true, None),
        json!({"ok": true, "id": "bag3d", "enabled": true, "saved": true})
    );
}

#[test]
fn page_lists_the_extensions_and_its_switch_takes_effect() {
    let _language = TestLanguage::hold(Language::Table(0));
    let mut studio = Studio::default();
    let _ = studio.update(Message::ToggleFile);
    let _ = studio.update(Message::FilePage(FilePage::Extensions));
    let _ = studio.view();
    for extension in BUILT_IN {
        for text in [extension.name, extension.description, extension.category] {
            // These reach `tr` through the constant, so a text that is
            // not marked with `key` would be missed by the scan of the
            // sources.
            assert!(crate::i18n::has_entry(text), "{text}");
        }
    }
    assert_eq!(tr(FilePage::Extensions.label()), "Extensies");
    assert_eq!(tr("Enabled"), "Ingeschakeld");
    assert_eq!(tr("Install extension…"), "Extensie installeren…");

    let _ = studio.update(Message::ExtensionEnabled(BAG3D, false));
    assert!(!studio.extensions.enabled(BAG3D));
    assert!(studio.file_open, "the page stays open");
    let _ = studio.view();
    let _ = studio.update(Message::ExtensionEnabled(BAG3D, true));
    assert!(studio.extensions.enabled(BAG3D));

    // Without a settings folder nothing can be installed, and the page says
    // so.
    assert!(studio.extension_host.root.is_none());
    let _ = studio.update(Message::Extension(ExtensionAction::Install));
    assert_eq!(
        studio.status,
        "Extensions cannot be installed: there is no settings folder"
    );
}

// ---------------------------------------------------------------------------
// Installing, updating, switching and uninstalling

#[test]
fn installs_updates_switches_and_uninstalls_are_kept_across_restarts() {
    let _language = TestLanguage::hold(Language::English);
    let bench = Bench::new();
    let mut studio = bench.studio();

    // The dialog shows what the extension declares; cancelling it leaves
    // nothing behind.
    stage_into(&mut studio, &example_folder());
    assert_eq!(
        studio.status,
        "Confirm the install of Point count report in the window"
    );
    let dialog = send(&mut studio, ApiCommand::Status)["result"]["extensions"]["dialog"].clone();
    assert_eq!(dialog["kind"], "install");
    assert_eq!(dialog["extension"]["id"], EXAMPLE);
    assert_eq!(dialog["extension"]["author"], "OpenAEC Foundation");
    assert_eq!(dialog["extension"]["uses"]["network"], false);
    assert_eq!(dialog["replaces"], Value::Null);
    assert!(dialog["files"].as_u64().unwrap() >= 4);
    let _ = studio.view();
    let _ = studio.update(Message::Escape);
    assert!(studio.extension_host.dialog.is_none());
    assert_eq!(studio.status, "Install cancelled");
    assert!(
        bench.installed_folders().is_empty(),
        "{:?}",
        bench.installed_folders()
    );

    install_confirmed(&mut studio, &example_folder());
    assert_eq!(studio.status, "Installed Point count report 1.0.0");
    assert_eq!(bench.installed_folders(), [EXAMPLE]);
    assert!(bench.root.join(EXAMPLE).join("report.ps1").is_file());
    assert_eq!(
        bench.stored(),
        json!({"disabled": [], "installed": {EXAMPLE: "1.0.0"}})
    );
    let listed = send(&mut studio, ApiCommand::ListExtensions);
    let entry = &listed["extensions"][1];
    assert_eq!(entry["id"], EXAMPLE);
    assert_eq!(entry["builtin"], false);
    assert_eq!(entry["category"], "Installed");
    assert_eq!(entry["enabled"], true);
    assert_eq!(entry["run"], Value::Null);
    assert_eq!(entry["ribbon"][0]["id"], "count");
    assert_eq!(entry["file_view"][0]["page"], "export");
    // Its button and its tile are offered, and everything draws.
    assert!(studio.extensions_ribbon().is_some());
    assert_eq!(studio.extension_tiles(EntryPage::Export).len(), 1);
    assert!(studio.extension_tiles(EntryPage::New).is_empty());
    let _ = studio.view();
    for page in [FilePage::Extensions, FilePage::Export, FilePage::New] {
        let _ = send(
            &mut studio,
            ApiCommand::FileView {
                open: true,
                page: Some(page.id().into()),
            },
        );
        let _ = studio.view();
    }

    // A new window finds it installed and enabled.
    let mut restarted = bench.studio();
    assert!(restarted.extension_host.find(EXAMPLE).is_some());
    assert!(restarted.extensions.enabled(EXAMPLE));
    let _ = restarted.update(Message::Extension(ExtensionAction::SetEnabled(
        EXAMPLE.into(),
        false,
    )));
    assert_eq!(restarted.status, "Point count report switched off");
    assert!(restarted.extensions_ribbon().is_none());
    assert!(restarted.extension_tiles(EntryPage::Export).is_empty());
    assert_eq!(
        restarted.start_extension(EXAMPLE, None).err().unwrap(),
        "Point count report is switched off"
    );

    // Switched off it stays after a restart, and through an update, which
    // keeps the logs of its runs.
    let mut again = bench.studio();
    assert!(!again.extensions.enabled(EXAMPLE));
    let logs = bench.root.join(EXAMPLE).join("logs");
    fs::create_dir_all(&logs).unwrap();
    fs::write(logs.join("run-1.log"), "an earlier run").unwrap();
    let newer = example_copy(bench.directory.path(), "1.1.0", None);
    stage_into(&mut again, &newer);
    let dialog = send(&mut again, ApiCommand::Status)["result"]["extensions"]["dialog"].clone();
    assert_eq!(dialog["replaces"], "1.0.0");
    let _ = again.view();
    let _ = again.update(Message::Extension(ExtensionAction::ConfirmInstall));
    assert_eq!(
        again.status,
        "Updated Point count report from 1.0.0 to 1.1.0"
    );
    assert_eq!(
        again.extension_host.find(EXAMPLE).unwrap().manifest.version,
        "1.1.0"
    );
    assert!(!again.extensions.enabled(EXAMPLE));
    assert_eq!(
        fs::read_to_string(logs.join("run-1.log")).unwrap(),
        "an earlier run"
    );
    assert_eq!(
        bench.stored(),
        json!({"disabled": [EXAMPLE], "installed": {EXAMPLE: "1.1.0"}})
    );
    assert_eq!(bench.installed_folders(), [EXAMPLE]);

    // An older version is refused, and leaves nothing behind.
    let older = example_copy(bench.directory.path(), "0.9.0", None);
    stage_into(&mut again, &older);
    assert!(again.extension_host.dialog.is_none());
    assert_eq!(
        again.extension_host.last_error.as_deref(),
        Some("version 1.1.0 is installed and this is the older version 0.9.0; uninstall it first to go back")
    );
    assert_eq!(bench.installed_folders(), [EXAMPLE]);
    let _ = again.view();

    // Uninstall asks first, then removes the folder with its logs.
    let _ = again.update(Message::Extension(ExtensionAction::Uninstall(
        EXAMPLE.into(),
    )));
    assert!(matches!(
        again.extension_host.dialog,
        Some(Dialog::Uninstall(_))
    ));
    assert_eq!(
        send(&mut again, ApiCommand::Status)["result"]["extensions"]["dialog"],
        json!({"kind": "uninstall", "id": EXAMPLE})
    );
    let _ = again.view();
    let _ = again.update(Message::Extension(ExtensionAction::ConfirmUninstall));
    assert_eq!(again.status, "Uninstalled Point count report");
    assert!(bench.installed_folders().is_empty());
    assert_eq!(bench.stored(), json!({"disabled": [], "installed": {}}));
    assert!(bench.studio().extension_host.installed.is_empty());
}

#[test]
fn a_later_version_before_its_release_replaces_an_earlier_one() {
    let _language = TestLanguage::hold(Language::English);
    let bench = Bench::new();
    let mut studio = bench.studio();
    install_confirmed(
        &mut studio,
        &example_copy(bench.directory.path(), "1.0.0-beta.9", None),
    );
    assert_eq!(studio.status, "Installed Point count report 1.0.0-beta.9");

    // beta.10 comes after beta.9, although its text sorts before it.
    install_confirmed(
        &mut studio,
        &example_copy(bench.directory.path(), "1.0.0-beta.10", None),
    );
    assert_eq!(
        studio.status,
        "Updated Point count report from 1.0.0-beta.9 to 1.0.0-beta.10"
    );
    assert_eq!(
        bench.stored()["installed"],
        json!({EXAMPLE: "1.0.0-beta.10"})
    );

    stage_into(
        &mut studio,
        &example_copy(bench.directory.path(), "1.0.0-beta.2", None),
    );
    assert!(studio.extension_host.dialog.is_none());
    assert_eq!(
        studio.extension_host.last_error.as_deref(),
        Some("version 1.0.0-beta.10 is installed and this is the older version 1.0.0-beta.2; uninstall it first to go back")
    );

    install_confirmed(
        &mut studio,
        &example_copy(bench.directory.path(), "1.0.0", None),
    );
    assert_eq!(
        studio.status,
        "Updated Point count report from 1.0.0-beta.10 to 1.0.0"
    );
}

#[test]
fn an_extension_that_cannot_be_read_is_listed_with_the_reason() {
    let _language = TestLanguage::hold(Language::English);
    let bench = Bench::new();
    fs::create_dir_all(bench.root.join("org.example.broken")).unwrap();
    fs::write(
        bench
            .root
            .join("org.example.broken")
            .join(manifest::MANIFEST),
        "{",
    )
    .unwrap();
    // A folder whose manifest names another extension.
    let other = bench.root.join("org.example.other");
    fs::create_dir_all(&other).unwrap();
    for entry in fs::read_dir(example_folder()).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), other.join(entry.file_name())).unwrap();
    }
    // A leftover of an install that ended halfway is cleared.
    fs::create_dir_all(bench.root.join(".staging-1234")).unwrap();
    fs::write(
        &bench.settings,
        json!({"installed": {
            "org.example.gone": "1.0.0",
            "org.example.broken": "1.0.0",
            "org.example.other": "1.0.0",
        }})
        .to_string(),
    )
    .unwrap();
    let mut studio = bench.studio();
    assert!(!bench.root.join(".staging-1234").exists());
    let problems: Vec<(String, String)> = studio
        .extension_host
        .problems
        .iter()
        .map(|problem| (problem.id.clone(), problem.error.clone()))
        .collect();
    assert_eq!(problems.len(), 3);
    assert_eq!(
        problems[1],
        ("org.example.gone".into(), "its folder is missing".into())
    );
    assert!(
        problems[0]
            .1
            .starts_with("extension.json is not valid JSON"),
        "{problems:?}"
    );
    assert_eq!(
        problems[2].1,
        format!("its extension.json names another id, {EXAMPLE}")
    );
    let listed = send(&mut studio, ApiCommand::ListExtensions);
    assert_eq!(listed["problems"].as_array().unwrap().len(), 3);
    assert_eq!(listed["extensions"].as_array().unwrap().len(), 1);
    assert_eq!(
        studio
            .start_extension("org.example.gone", None)
            .err()
            .unwrap(),
        "extension org.example.gone could not be read; see the Extensions page"
    );
    let _ = studio.update(Message::ToggleFile);
    let _ = studio.update(Message::FilePage(FilePage::Extensions));
    let _ = studio.view();

    // It can be uninstalled all the same.
    let _ = studio.update(Message::Extension(ExtensionAction::Uninstall(
        "org.example.broken".into(),
    )));
    let _ = studio.update(Message::Extension(ExtensionAction::ConfirmUninstall));
    assert_eq!(studio.status, "Uninstalled org.example.broken");
    assert!(!bench.root.join("org.example.broken").exists());
    assert_eq!(studio.extension_host.problems.len(), 2);
    assert_eq!(
        bench.stored()["installed"],
        json!({"org.example.gone": "1.0.0", "org.example.other": "1.0.0"})
    );
}

#[test]
fn an_install_through_the_api_waits_for_the_user() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    let relative = send(
        &mut studio,
        ApiCommand::InstallExtension {
            path: "extension.zip".into(),
        },
    );
    assert_eq!(
        relative["error"],
        "install_extension needs the absolute path of a folder, an extension.json or a .zip archive"
    );
    let nowhere = send(
        &mut studio,
        ApiCommand::InstallExtension {
            path: example_folder(),
        },
    );
    assert_eq!(nowhere["error"], "there is no settings folder");

    // The answer comes once the extension was checked and the dialog shows.
    let bench = Bench::new();
    let mut studio = bench.studio();
    let (reply, receive) = std::sync::mpsc::channel();
    let staged = install::stage(&example_folder(), &bench.root, &known()).map(Box::new);
    let _ = studio.update(Message::Extension(ExtensionAction::Staged(
        staged,
        Some(reply),
    )));
    let answer = receive.recv().unwrap();
    assert_eq!(answer["ok"], true);
    assert_eq!(answer["confirmation"], "shown");
    assert_eq!(answer["extension"]["id"], EXAMPLE);
    assert_eq!(answer["replaces"], Value::Null);
    // A second install waits until the first is answered.
    let second = send(
        &mut studio,
        ApiCommand::InstallExtension {
            path: example_folder(),
        },
    );
    assert_eq!(
        second["error"],
        "another install waits for its confirmation in the window"
    );
    // A source that is no extension is refused with the reason.
    let _ = studio.update(Message::Extension(ExtensionAction::CloseDialog));
    let (reply, receive) = std::sync::mpsc::channel();
    let staged = install::stage(&bench.settings, &bench.root, &known()).map(Box::new);
    let _ = studio.update(Message::Extension(ExtensionAction::Staged(
        staged,
        Some(reply),
    )));
    let refused = receive.recv().unwrap();
    assert_eq!(refused["ok"], false);
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("extensions.json"));
    assert!(studio.extension_host.dialog.is_none());
    assert!(studio
        .status
        .starts_with("Could not install the extension: "));
}

// ---------------------------------------------------------------------------
// Commands an extension uses

#[test]
fn messages_and_progress_are_checked_and_name_the_extension() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    let shown = send(
        &mut studio,
        ApiCommand::ShowMessage {
            text: "  12 scans\nread \t ".into(),
        },
    );
    assert_eq!(shown, json!({"ok": true, "status": "12 scans read"}));
    assert_eq!(studio.status, "12 scans read");
    for (text, error) in [
        (" \n ".to_owned(), "text is empty".to_owned()),
        (
            "x".repeat(301),
            "text is longer than 300 characters".to_owned(),
        ),
    ] {
        let refused = send(&mut studio, ApiCommand::ShowMessage { text });
        assert_eq!(refused, json!({"ok": false, "error": error}));
    }
    assert_eq!(studio.status, "12 scans read", "a refusal changes nothing");

    // Outside a run the progress shows in the status line until it is done.
    let progress = send(
        &mut studio,
        ApiCommand::ReportProgress {
            percent: 40.0,
            text: Some("Counting".into()),
        },
    );
    assert_eq!(
        progress,
        json!({"ok": true, "percent": 40.0, "text": "Counting"})
    );
    assert_eq!(studio.status, "Counting (40%)");
    let status = send(&mut studio, ApiCommand::Status);
    assert_eq!(
        status["result"]["extensions"]["progress"],
        json!({"percent": 40.0, "text": "Counting"})
    );
    let _ = send(
        &mut studio,
        ApiCommand::ReportProgress {
            percent: 100.0,
            text: None,
        },
    );
    assert_eq!(studio.status, "100%");
    assert_eq!(studio.extension_host.progress, None);
    for percent in [-1.0, 100.5] {
        let refused = send(
            &mut studio,
            ApiCommand::ReportProgress {
                percent,
                text: None,
            },
        );
        assert_eq!(refused["error"], "percent must be a number from 0 to 100");
    }
    let long = send(
        &mut studio,
        ApiCommand::ReportProgress {
            percent: 1.0,
            text: Some("x".repeat(121)),
        },
    );
    assert_eq!(long["error"], "text is longer than 120 characters");

    // A message of an extension starts with its name.
    let said = send_as(
        &mut studio,
        "org.example.tool",
        ApiCommand::ShowMessage {
            text: "done".into(),
        },
    );
    assert_eq!(said["status"], "org.example.tool: done");
    assert_eq!(
        studio.extension_host.caller, None,
        "the caller is forgotten"
    );
}

#[test]
fn choose_path_is_a_job_that_ends_with_the_path_or_cancelled() {
    let _language = TestLanguage::hold(Language::English);
    let mut studio = Studio::default();
    let choose = |options: Value| ApiCommand::ChoosePath {
        options: serde_json::from_value(options).unwrap(),
    };
    for (options, error) in [
        (
            json!({"mode": "delete"}),
            "mode must be open, save or folder",
        ),
        (
            json!({"mode": "save", "title": "x".repeat(121)}),
            "title is longer than 120 characters",
        ),
        (
            json!({"mode": "save", "filters": [{"name": "CSV", "extensions": [".csv"]}]}),
            "a filter needs 1 to 16 extensions of letters and digits, without the dot",
        ),
        (
            json!({"mode": "save", "filters": [{"name": " ", "extensions": ["csv"]}]}),
            "the name of a filter must be 1 to 60 characters",
        ),
        (
            json!({"mode": "save", "file_name": "../report.csv"}),
            "file_name must be a file name without folders",
        ),
        (
            json!({"mode": "folder", "directory": "relative"}),
            "directory must be an absolute path",
        ),
    ] {
        let refused = send(&mut studio, choose(options));
        assert_eq!(refused, json!({"ok": false, "error": error}));
    }
    assert!(studio.api_jobs.is_empty(), "a refusal starts no job");

    let accepted = send(
        &mut studio,
        choose(json!({
            "mode": "save",
            "title": "Save the report",
            "file_name": "report.csv",
            "filters": [{"name": "CSV table", "extensions": ["csv"]}],
        })),
    );
    assert_eq!(accepted["ok"], true);
    let job = accepted["job_id"].as_str().unwrap().to_owned();
    let read = |studio: &mut Studio, job: &str| {
        send(studio, ApiCommand::Job { id: job.into() })["job"].clone()
    };
    assert_eq!(
        read(&mut studio, &job),
        json!({"state": "running", "operation": "choose_path"})
    );
    assert_eq!(
        send(&mut studio, ApiCommand::Status)["result"]["extensions"]["path_dialog"],
        true
    );
    let busy = send(&mut studio, choose(json!({"mode": "open"})));
    assert_eq!(busy["error"], "a dialog of choose_path is open already");

    let chosen = std::env::temp_dir().join("report.csv");
    let _ = studio.update(Message::Extension(ExtensionAction::PathChosen(
        job.clone(),
        Some(chosen.clone()),
    )));
    assert_eq!(
        read(&mut studio, &job),
        json!({"state": "complete", "operation": "choose_path", "path": chosen})
    );
    // An extension may ask, also when it declared no other command.
    let again = send_as(
        &mut studio,
        "org.example.tool",
        choose(json!({"mode": "folder"})),
    );
    let job = again["job_id"].as_str().unwrap().to_owned();
    let _ = studio.update(Message::Extension(ExtensionAction::PathChosen(
        job.clone(),
        None,
    )));
    assert_eq!(
        read(&mut studio, &job),
        json!({"state": "cancelled", "operation": "choose_path"})
    );
}

#[test]
fn context_reports_what_the_window_shows() {
    let mut studio = Studio::default();
    let empty = send(&mut studio, ApiCommand::Context);
    assert_eq!(empty["ok"], true);
    let context = &empty["context"];
    assert_eq!(context["application"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(context["active_scan"], Value::Null);
    assert_eq!(context["scans"], 0);
    assert_eq!(context["selected_points"], 0);
    assert_eq!(context["section_box"], Value::Null);
    assert_eq!(context["shown"]["kind"], "model");
    assert_eq!(context["drawing_view"], false);
    assert_eq!(context.get("extension"), None);

    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hall.xyz");
    fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
    let _ = studio.update(Message::Loaded(Ok(cloud)));
    let _ = studio.update(Message::SetSectionEnabled(true));
    let context = send(&mut studio, ApiCommand::Context)["context"].clone();
    assert_eq!(context["scans"], 1);
    assert_eq!(context["active_scan"]["index"], 0);
    assert_eq!(context["active_scan"]["path"], json!(path));
    assert_eq!(context["active_scan"]["points"], 4);
    assert_eq!(context["active_scan"]["remaining"], 4);
    assert_eq!(
        context["active_scan"]["bounds"]["max"],
        json!([4.0, 3.0, 2.0])
    );
    assert!(context["section_box"]["min"].is_array());
}

#[test]
fn runs_are_refused_while_nothing_can_run_them() {
    let _language = TestLanguage::hold(Language::English);
    let bench = Bench::new();
    let mut studio = bench.studio();
    install_confirmed(&mut studio, &example_folder());
    let unknown = send(
        &mut studio,
        ApiCommand::RunExtension {
            id: "org.example.none".into(),
            entry: None,
        },
    );
    assert_eq!(unknown["error"], "unknown extension org.example.none");
    let entry = send(
        &mut studio,
        ApiCommand::RunExtension {
            id: EXAMPLE.into(),
            entry: Some("nothing".into()),
        },
    );
    assert_eq!(
        entry["error"],
        "Point count report has no button or tile nothing"
    );
    // A window whose local API did not start has no port to give.
    let no_api = send(
        &mut studio,
        ApiCommand::RunExtension {
            id: EXAMPLE.into(),
            entry: None,
        },
    );
    assert_eq!(
        no_api["error"],
        "The local API is not running, so extensions cannot run"
    );
    let _ = studio.update(Message::Extension(ExtensionAction::Press(
        EXAMPLE.into(),
        Some("count".into()),
    )));
    assert_eq!(
        studio.status,
        "The local API is not running, so extensions cannot run"
    );
    assert!(studio.extension_host.runs.is_empty());
    let stop = send(
        &mut studio,
        ApiCommand::StopExtension { id: EXAMPLE.into() },
    );
    assert_eq!(stop["error"], format!("extension {EXAMPLE} is not running"));
}
