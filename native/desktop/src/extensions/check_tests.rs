//! Tests of what is checked before an extension is installed: its manifest,
//! its icons, its folder and the archive it may come in.

use std::io::Write;

use super::tests::{example_folder, known, EXAMPLE};
use super::*;
use crate::i18n::{Language, TestLanguage};

const ICON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><defs><linearGradient id="g"/></defs><circle cx="12" cy="12" r="5" fill="url(#g)"/><use href="#g"/></svg>"##;

/// A manifest that fits, for a folder made by `tool_folder`.
fn base_manifest() -> Value {
    json!({
        "id": "org.example.tool",
        "name": "Tool",
        "version": "1.0.0",
        "author": "Example",
        "description": "Does a thing.",
        "min_app_version": "0.1.0",
        "command": {"interpreter": "python", "program": "tool.py"},
        "uses": {"network": false, "files_outside_folder": false, "commands": ["status"]},
        "contributes": {
            "ribbon": [{"id": "go", "label": "Go", "icon": "icon.svg"}],
            "file_view": [{"id": "save", "page": "export", "title": "Save…"}],
        },
    })
}

/// A folder of an extension with a script, an icon and a manifest.
fn tool_folder(parent: &Path, manifest: &Value) -> PathBuf {
    let folder = parent.join("tool");
    fs::create_dir_all(folder.join("lib")).unwrap();
    fs::write(folder.join("tool.py"), "print('tool')\n").unwrap();
    fs::write(folder.join("lib/helper.py"), "\n").unwrap();
    fs::write(folder.join("icon.svg"), ICON).unwrap();
    fs::write(
        folder.join(manifest::MANIFEST),
        serde_json::to_vec_pretty(manifest).unwrap(),
    )
    .unwrap();
    folder
}

fn read_manifest(manifest: &Value) -> Result<Manifest, String> {
    let directory = tempfile::tempdir().unwrap();
    let folder = tool_folder(directory.path(), manifest);
    manifest::read(&folder, &known())
}

#[test]
fn the_example_manifest_fits() {
    let manifest = manifest::read(&example_folder(), &known()).unwrap();
    assert_eq!(manifest.id, EXAMPLE);
    assert_eq!(manifest.name.english(), "Point count report");
    assert_eq!(manifest.version, "1.0.0");
    let expected = if cfg!(windows) {
        ("powershell", "report.ps1")
    } else {
        ("python", "report.py")
    };
    assert_eq!(
        (
            manifest.launch.interpreter.map(manifest::Interpreter::key),
            manifest.launch.program.as_str()
        ),
        (Some(expected.0), expected.1)
    );
    assert_eq!(manifest.ribbon.len(), 1);
    assert_eq!(manifest.ribbon[0].id, "count");
    assert_eq!(manifest.file_view.len(), 1);
    assert_eq!(manifest.file_view[0].page, manifest::EntryPage::Export);
    assert_eq!(manifest.args_of("report"), Some(&["--save".to_owned()][..]));
    assert_eq!(manifest.args_of("count"), Some(&[][..]));
    assert_eq!(manifest.args_of("other"), None);
    assert!(!manifest.uses.network);
    assert!(manifest.uses.files_outside_folder);
    assert_eq!(
        manifest
            .uses
            .commands
            .granted()
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        [
            "choose_path",
            "job",
            "report_progress",
            "show_message",
            "status"
        ]
    );
    // Its texts have a Dutch translation, shown when the window is Dutch.
    let _language = TestLanguage::hold(Language::Table(0));
    assert_eq!(manifest.name.get(), "Puntentelling");
    assert_eq!(manifest.ribbon[0].label.get(), "Punten tellen");
}

#[test]
fn a_manifest_that_fits_is_read_with_its_defaults_and_a_byte_order_mark() {
    let manifest = read_manifest(&base_manifest()).unwrap();
    assert_eq!(manifest.homepage, None);
    assert_eq!(manifest.launch.args, Vec::<String>::new());
    assert_eq!(manifest.launch.describe(), "python tool.py");
    assert_eq!(manifest.ribbon[0].tooltip, None);
    assert_eq!(manifest.value()["uses"]["commands"], json!(["status"]));

    let directory = tempfile::tempdir().unwrap();
    let folder = tool_folder(directory.path(), &base_manifest());
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend(serde_json::to_vec(&base_manifest()).unwrap());
    assert_eq!(
        manifest::parse(&bytes, &folder, &known()).unwrap(),
        manifest
    );

    let mut all = base_manifest();
    all["uses"]["commands"] = json!("all");
    all["command"]["args"] = json!(["--mode", "two words"]);
    all["homepage"] = json!("https://example.org/tool");
    let manifest = read_manifest(&all).unwrap();
    assert_eq!(manifest.uses.commands.granted(), None);
    assert_eq!(
        manifest.launch.describe(),
        "python tool.py --mode \"two words\""
    );
}

#[test]
fn manifests_that_do_not_fit_are_refused_with_the_field_and_the_reason() {
    type Change = fn(&mut Value);
    let cases: Vec<(Change, &str)> = vec![
        (|m| *m = json!([1, 2]), "must hold one JSON object"),
        (
            |m| m["colour"] = json!("red"),
            "colour is not a field of the manifest",
        ),
        (
            |m| m["id"] = json!("My Tool"),
            "id must be 3 to 64 lowercase letters",
        ),
        (|m| m["id"] = json!("org..tool"), "id must be 3 to 64"),
        (|m| m["id"] = json!("aux.tool"), "id must be 3 to 64"),
        (
            |m| m["id"] = json!("bag3d"),
            "the id of a built-in extension",
        ),
        (|m| m["version"] = json!("one"), "version must be a version"),
        (
            |m| m["min_app_version"] = json!("99.0"),
            "needs Open Pointcloud Studio 99.0 or newer",
        ),
        (
            |m| m["homepage"] = json!("ftp://example.org"),
            "homepage must be a web address",
        ),
        (
            |m| m["name"] = json!("x".repeat(61)),
            "name is longer than 60 characters",
        ),
        (
            |m| m["name"] = json!("Two\nlines"),
            "name holds a control character",
        ),
        (|m| m["name"] = json!(""), "name is empty"),
        (
            |m| m["description"] = json!({"nl": "Doet iets."}),
            "description needs an English text under en",
        ),
        (
            |m| m["description"] = json!({"en": "x", "dutch": "y"}),
            "description.dutch is not a language code",
        ),
        (|m| m["author"] = json!(7), "author must be a text"),
        (
            |m| {
                m.as_object_mut().unwrap().remove("command");
            },
            "command is missing",
        ),
        (
            |m| {
                m.as_object_mut().unwrap().remove("uses");
            },
            "uses is missing",
        ),
        (
            |m| m["command"]["program"] = json!("../outside.py"),
            "leads out of the folder",
        ),
        (
            |m| m["command"]["program"] = json!("/tool.py"),
            "must be relative to the folder",
        ),
        (
            |m| m["command"]["program"] = json!("lib\\helper.py"),
            "must be relative to the folder",
        ),
        (
            |m| m["command"]["program"] = json!("missing.py"),
            "command.program missing.py does not exist",
        ),
        (
            |m| m["command"]["program"] = json!("lib"),
            "command.program lib is not a file",
        ),
        (
            |m| m["command"]["interpreter"] = json!("ruby"),
            "command.interpreter must be one of python, powershell, node, sh",
        ),
        (
            |m| m["command"]["args"] = json!([1]),
            "command.args must be a list of texts",
        ),
        (
            |m| m["command"]["shell"] = json!(true),
            "command.shell is not a field of the manifest",
        ),
        (|m| m["command"] = json!({}), "command needs program"),
        (
            |m| m["command"] = json!({"windows": {"interpreter": "sh", "program": "tool.py"}}),
            "sh is not available on Windows",
        ),
        (
            |m| m["uses"]["commands"] = json!(["format_disk"]),
            "format_disk is not a command of the local API",
        ),
        (
            |m| m["uses"]["commands"] = json!([]),
            "uses.commands must list 1 to",
        ),
        (
            |m| m["uses"]["commands"] = json!(["status", "status"]),
            "uses.commands lists status twice",
        ),
        (
            |m| m["uses"]["commands"] = json!("some"),
            "uses.commands must be a list of commands or \"all\"",
        ),
        (
            |m| {
                m["uses"].as_object_mut().unwrap().remove("network");
            },
            "uses.network is missing",
        ),
        (
            |m| m["uses"]["files_outside_folder"] = json!("no"),
            "uses.files_outside_folder must be true or false",
        ),
        (
            |m| m["contributes"]["ribbon"][0]["icon"] = json!("tool.py"),
            "is not an .svg file",
        ),
        (
            |m| m["contributes"]["ribbon"][0]["icon"] = json!("missing.svg"),
            "contributes.ribbon[0].icon missing.svg does not exist",
        ),
        (
            |m| m["contributes"]["ribbon"][0]["label"] = json!("x".repeat(25)),
            "contributes.ribbon[0].label is longer than 24 characters",
        ),
        (
            |m| m["contributes"]["ribbon"][0]["id"] = json!("G"),
            "contributes.ribbon[0].id must be 1 to 64",
        ),
        (
            |m| {
                m["contributes"]["ribbon"] = json!([
                    {"id": "go", "label": "A", "icon": "icon.svg"},
                    {"id": "go", "label": "B", "icon": "icon.svg"},
                ]);
            },
            "contributes.ribbon[1].id: go is used twice",
        ),
        (
            |m| m["contributes"]["file_view"][0]["id"] = json!("go"),
            "contributes.file_view[0].id: go is used twice",
        ),
        (
            |m| {
                m["contributes"]["ribbon"] = json!(vec![
                    json!({"id": "go", "label": "A", "icon": "icon.svg"});
                    9
                ]);
            },
            "contributes.ribbon holds more than 8 entries",
        ),
        (
            |m| m["contributes"]["file_view"][0]["page"] = json!("import"),
            "contributes.file_view[0].page must be new or export",
        ),
        (
            |m| m["contributes"]["menu"] = json!([]),
            "contributes.menu is not a field of the manifest",
        ),
    ];
    for (change, expected) in cases {
        let mut manifest = base_manifest();
        change(&mut manifest);
        let error = read_manifest(&manifest).unwrap_err();
        assert!(error.contains(expected), "{expected:?} not in {error:?}");
    }

    // A manifest without a launch for this system cannot run here.
    let other_system = if manifest::SYSTEM == "macos" {
        "linux"
    } else {
        "macos"
    };
    let mut manifest = base_manifest();
    manifest["command"] = json!({other_system: {"interpreter": "python", "program": "tool.py"}});
    let error = read_manifest(&manifest).unwrap_err();
    assert!(error.starts_with("it cannot run on"), "{error}");
    // Each launch is checked, also the one for another system.
    manifest["command"] = json!({
        "any": {"interpreter": "python", "program": "tool.py"},
        other_system: {"interpreter": "python", "program": "gone.py"},
    });
    assert!(read_manifest(&manifest)
        .unwrap_err()
        .contains("gone.py does not exist"));
    manifest["command"][other_system]["program"] = json!("tool.py");
    assert!(read_manifest(&manifest).is_ok());

    // Windows starts programs, not scripts, without an interpreter.
    let mut manifest = base_manifest();
    manifest["command"] = json!({"program": "tool.py"});
    let read = read_manifest(&manifest);
    if cfg!(windows) {
        assert!(read
            .unwrap_err()
            .contains("is not a program Windows starts"));
    } else {
        assert_eq!(read.unwrap().launch.interpreter, None);
    }

    // Not JSON at all, a manifest too large to be one, and none.
    let directory = tempfile::tempdir().unwrap();
    let folder = tool_folder(directory.path(), &base_manifest());
    fs::write(folder.join(manifest::MANIFEST), "{ id: tool }").unwrap();
    assert!(manifest::read(&folder, &known())
        .unwrap_err()
        .starts_with("extension.json is not valid JSON"));
    fs::write(
        folder.join(manifest::MANIFEST),
        " ".repeat(manifest::MAX_MANIFEST_BYTES as usize + 1),
    )
    .unwrap();
    assert_eq!(
        manifest::read(&folder, &known()).unwrap_err(),
        "extension.json is larger than 64 KiB"
    );
    fs::remove_file(folder.join(manifest::MANIFEST)).unwrap();
    assert_eq!(
        manifest::read(&folder, &known()).unwrap_err(),
        "the folder has no extension.json"
    );
}

#[test]
fn icons_may_only_refer_to_their_own_elements() {
    manifest::check_icon(ICON.as_bytes()).unwrap();
    manifest::check_icon(b"\xEF\xBB\xBF<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    for (icon, reason) in [
        ("<html></html>", "is not an SVG image"),
        ("<svg><script>alert(1)</script></svg>", "holds <script>"),
        ("<svg><foreignObject/></svg>", "holds <foreignobject>"),
        ("<svg><image href=\"photo.png\"/></svg>", "holds <image>"),
        (
            "<svg><use href=\"other.svg#a\"/></svg>",
            "links to something outside the image",
        ),
        (
            "<svg><use xlink:href = 'https://example.org/a'/></svg>",
            "links to something outside",
        ),
        (
            "<svg><rect fill=\"url(http://example.org/x)\"/></svg>",
            "refers to something outside",
        ),
        (
            "<svg><style>@import 'x.css';</style></svg>",
            "imports a style sheet",
        ),
        (
            "<!DOCTYPE svg [<!ENTITY x SYSTEM \"file:///etc/passwd\">]><svg/>",
            "holds <!entity>",
        ),
    ] {
        let error = manifest::check_icon(icon.as_bytes()).unwrap_err();
        assert!(error.contains(reason), "{icon}: {error}");
    }
    let large = format!(
        "<svg>{}</svg>",
        " ".repeat(manifest::MAX_ICON_BYTES as usize)
    );
    assert_eq!(
        manifest::check_icon(large.as_bytes()).unwrap_err(),
        "is larger than 64 KiB"
    );
    assert_eq!(
        manifest::check_icon(b"<svg>\xFF</svg>").unwrap_err(),
        "is not UTF-8 text"
    );
    // A manifest names the icon with the reason.
    let directory = tempfile::tempdir().unwrap();
    let folder = tool_folder(directory.path(), &base_manifest());
    fs::write(folder.join("icon.svg"), "<svg><script/></svg>").unwrap();
    assert_eq!(
        manifest::read(&folder, &known()).unwrap_err(),
        "contributes.ribbon[0].icon icon.svg holds <script>, which an icon may not"
    );
}

#[test]
fn versions_are_ordered_by_their_numbers() {
    use manifest::Version;
    let parse = |text| Version::parse(text).unwrap_or_else(|| panic!("{text}"));
    assert!(parse("1.0.0") < parse("1.0.1"));
    assert!(parse("1.2") == parse("1.2.0"));
    assert!(parse("1.10.0") > parse("1.9.9"));
    assert!(parse("2.0.0-beta.1") < parse("2.0.0"));
    assert!(parse("2.0.0-alpha") < parse("2.0.0-beta"));
    assert!(parse("1.0.0+build.5") == parse("1.0.0"));
    assert!(parse("1.2.3.4") > parse("1.2.3"));
    for wrong in [
        "",
        "v1.0",
        "1..0",
        "1.0.0.0.0",
        "1.0-",
        "1.0-be ta",
        "x",
        "1234567890.0",
    ] {
        assert_eq!(Version::parse(wrong), None, "{wrong}");
    }
    assert!(manifest::app_version() >= parse("0.9.1"));
}

#[test]
fn ids_and_names_are_ones_every_system_keeps() {
    for id in ["org.example.tool", "abc", "my-tool2", "a1.b2-c3"] {
        assert!(manifest::valid_id(id), "{id}");
    }
    let long = "x".repeat(65);
    for id in [
        "ab",
        "Org.tool",
        "org..tool",
        ".tool",
        "tool-",
        "my_tool",
        "con",
        "lpt1.tool",
        long.as_str(),
    ] {
        assert!(!manifest::valid_id(id), "{id}");
    }
    for name in ["report.ps1", ".gitignore", "a b.txt", "Ünïcode.txt"] {
        assert!(manifest::valid_component(name), "{name}");
    }
    for name in [
        "",
        ".",
        "..",
        "nul",
        "NUL.txt",
        "com3.log",
        "a:b",
        "a*b",
        "trailing.",
        "trailing ",
        "tab\there",
    ] {
        assert!(!manifest::valid_component(name), "{name}");
    }
    assert_eq!(
        manifest::relative_path("lib/helper.py").unwrap(),
        Path::new("lib").join("helper.py")
    );
    for (path, reason) in [
        ("", "is empty"),
        ("/etc/passwd", "must be relative"),
        ("lib\\helper.py", "must be relative"),
        ("lib/../../x", "leads out of the folder"),
        ("lib//x", "has a name that cannot be used"),
        ("C:/x", "has a name that cannot be used"),
    ] {
        let error = manifest::relative_path(path).unwrap_err();
        assert!(error.contains(reason), "{path}: {error}");
    }
}

#[test]
fn a_folder_with_links_or_too_much_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let folder = tool_folder(directory.path(), &base_manifest());
    let summary = manifest::check_folder(&folder, &[]).unwrap();
    assert_eq!(
        summary
            .files
            .iter()
            .map(|path| manifest::shown(path))
            .collect::<Vec<_>>(),
        ["extension.json", "icon.svg", "lib/helper.py", "tool.py"]
    );

    // A folder of logs is left out when it is skipped.
    fs::create_dir_all(folder.join("logs")).unwrap();
    fs::write(folder.join("logs/run.log"), "x").unwrap();
    assert_eq!(
        manifest::check_folder(&folder, &["logs"])
            .unwrap()
            .files
            .len(),
        4
    );
    assert_eq!(manifest::check_folder(&folder, &[]).unwrap().files.len(), 5);

    // Too large together: the size of the files counts, not what they hold.
    let large = fs::File::create(folder.join("large.bin")).unwrap();
    large.set_len(manifest::MAX_TOTAL_BYTES + 1).unwrap();
    drop(large);
    assert_eq!(
        manifest::check_folder(&folder, &[]).unwrap_err(),
        "its files are larger than 64 MiB together"
    );
    fs::remove_file(folder.join("large.bin")).unwrap();

    let deep =
        (0..manifest::MAX_DEPTH).fold(folder.clone(), |path, level| path.join(format!("d{level}")));
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("x"), "").unwrap();
    assert!(manifest::check_folder(&folder, &[])
        .unwrap_err()
        .contains("folders deep"));
    fs::remove_dir_all(folder.join("d0")).unwrap();

    let many = folder.join("many");
    fs::create_dir_all(&many).unwrap();
    for number in 0..=manifest::MAX_FILES {
        fs::write(many.join(format!("{number}.txt")), "").unwrap();
    }
    assert_eq!(
        manifest::check_folder(&folder, &[]).unwrap_err(),
        "it holds more than 1000 files"
    );
    fs::remove_dir_all(&many).unwrap();

    // A link, to a file or a folder outside, is refused wherever it lies.
    let outside = directory.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "secret").unwrap();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, folder.join("lib/outside")).is_ok();
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_dir(&outside, folder.join("lib/outside")).is_ok();
    if linked {
        assert_eq!(
            manifest::check_folder(&folder, &[]).unwrap_err(),
            "lib/outside is a link; links are not allowed"
        );
        let root = directory.path().join("extensions");
        let error = install::stage(&folder, &root, &known()).unwrap_err();
        assert_eq!(error, "lib/outside is a link; links are not allowed");
    } else {
        eprintln!("skipped the link: this account may not make links");
    }
}

// ---------------------------------------------------------------------------
// Archives

/// One file of a test archive.
struct Packed<'a> {
    name: &'a str,
    data: &'a [u8],
    /// The size the archive states; the true size without it.
    declared: Option<u32>,
    flags: u16,
    method: u16,
    /// The kind and permissions of a Unix file, kept in the high half of
    /// the external attributes.
    mode: Option<u32>,
}

fn packed<'a>(name: &'a str, data: &'a [u8]) -> Packed<'a> {
    Packed {
        name,
        data,
        declared: None,
        flags: 0,
        method: 8,
        mode: None,
    }
}

/// A ZIP archive of the files, as the common tools write one.
fn archive(files: &[Packed<'_>]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut directory = Vec::new();
    for file in files {
        let data = if file.method == 8 {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(file.data).unwrap();
            encoder.finish().unwrap()
        } else {
            file.data.to_vec()
        };
        let mut crc = flate2::Crc::new();
        crc.update(file.data);
        let size = file.declared.unwrap_or(file.data.len() as u32);
        let offset = bytes.len() as u32;
        let mut fields = Vec::new();
        fields.extend(20u16.to_le_bytes());
        fields.extend(file.flags.to_le_bytes());
        fields.extend(file.method.to_le_bytes());
        fields.extend([0u8; 4]);
        fields.extend(crc.sum().to_le_bytes());
        fields.extend((data.len() as u32).to_le_bytes());
        fields.extend(size.to_le_bytes());
        fields.extend((file.name.len() as u16).to_le_bytes());
        fields.extend(0u16.to_le_bytes());
        bytes.extend(b"PK\x03\x04");
        bytes.extend(&fields);
        bytes.extend(file.name.as_bytes());
        bytes.extend(&data);
        let made_by: u16 = if file.mode.is_some() {
            (3 << 8) | 20
        } else {
            20
        };
        directory.extend(b"PK\x01\x02");
        directory.extend(made_by.to_le_bytes());
        directory.extend(&fields);
        directory.extend([0u8; 6]);
        directory.extend((file.mode.unwrap_or(0) << 16).to_le_bytes());
        directory.extend(offset.to_le_bytes());
        directory.extend(file.name.as_bytes());
    }
    let start = bytes.len() as u32;
    let count = files.len() as u16;
    bytes.extend(&directory);
    bytes.extend(b"PK\x05\x06");
    bytes.extend([0u8; 4]);
    bytes.extend(count.to_le_bytes());
    bytes.extend(count.to_le_bytes());
    bytes.extend((directory.len() as u32).to_le_bytes());
    bytes.extend(start.to_le_bytes());
    bytes.extend(0u16.to_le_bytes());
    bytes
}

/// The files of the example extension, with their names in the archive.
fn example_files(prefix: &str) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(example_folder())
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| {
            (
                format!("{prefix}{}", entry.file_name().to_string_lossy()),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

/// Stage an archive of the example files and further files.
fn stage_archive(
    extra: &[Packed<'_>],
    prefix: &str,
) -> (tempfile::TempDir, Result<Staged, String>) {
    let directory = tempfile::tempdir().unwrap();
    let files = example_files(prefix);
    let mut entries: Vec<Packed<'_>> = files
        .iter()
        .map(|(name, data)| packed(name, data))
        .collect();
    for file in extra {
        entries.push(Packed {
            name: file.name,
            data: file.data,
            declared: file.declared,
            flags: file.flags,
            method: file.method,
            mode: file.mode,
        });
    }
    let path = directory.path().join("extension.zip");
    fs::write(&path, archive(&entries)).unwrap();
    let root = directory.path().join("extensions");
    let staged = install::stage(&path, &root, &known());
    (directory, staged)
}

/// What lies in a folder, as relative paths with `/`.
fn listing(folder: &Path) -> Vec<String> {
    manifest::check_folder(folder, &[])
        .unwrap()
        .files
        .iter()
        .map(|path| manifest::shown(path))
        .collect()
}

#[test]
fn an_archive_is_unpacked_from_its_one_folder_and_without_the_extras_of_macos() {
    for prefix in ["", "point-count-report/"] {
        let folder_entry = packed("point-count-report/", b"");
        let resource = packed("__MACOSX/point-count-report/._report.ps1", b"fork");
        let logs = packed("logs/run-1.log", b"an old run");
        let extra = if prefix.is_empty() {
            vec![logs]
        } else {
            vec![folder_entry, resource]
        };
        let (directory, staged) = stage_archive(&extra, prefix);
        let staged = staged.unwrap();
        assert_eq!(staged.manifest.id, EXAMPLE);
        let expected: Vec<String> = example_files("")
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(listing(&staged.folder), expected, "{prefix}");
        assert_eq!(staged.files, expected.len());
        // The staging folder lies with the installed extensions until it is
        // confirmed or thrown away.
        assert!(staged
            .folder
            .starts_with(directory.path().join("extensions")));
        staged.discard();
        assert!(!staged.folder.exists());
    }
}

#[test]
fn archives_that_reach_outside_hide_links_or_are_too_large_are_refused() {
    let link = Packed {
        mode: Some(0o120_777),
        ..packed("lib/outside", b"../../../outside")
    };
    let encrypted = Packed {
        flags: 1,
        ..packed("secret.txt", b"secret")
    };
    let bzip = Packed {
        method: 12,
        ..packed("other.txt", b"other")
    };
    let damaged = Packed {
        declared: Some(3),
        ..packed("short.txt", b"short")
    };
    let cases: Vec<(Vec<Packed<'_>>, &str)> = vec![
        (
            vec![packed("../evil.txt", b"evil")],
            "../evil.txt leads out of the folder",
        ),
        (
            vec![packed("lib/../../evil.txt", b"evil")],
            "leads out of the folder",
        ),
        (
            vec![packed("/etc/evil", b"evil")],
            "/etc/evil is an absolute path",
        ),
        (
            vec![packed("C:/evil.txt", b"evil")],
            "C:/evil.txt is an absolute path",
        ),
        (
            vec![packed("lib\\evil.txt", b"evil")],
            "uses \\ between folders",
        ),
        (
            vec![packed("nul.txt", b"evil")],
            "has a name that cannot be used",
        ),
        (vec![link], "lib/outside is a link; links are not allowed"),
        (vec![encrypted], "secret.txt is encrypted"),
        (vec![bzip], "other.txt uses compression method 12"),
        (vec![damaged], "short.txt is damaged"),
        (
            vec![packed("Report.PS1", b"twice")],
            "is in the archive twice",
        ),
    ];
    for (extra, expected) in cases {
        let (directory, staged) = stage_archive(&extra, "");
        let error = staged.unwrap_err();
        assert!(error.contains(expected), "{expected:?} not in {error:?}");
        // Nothing was written outside, and the staging folder is gone.
        assert!(!directory.path().join("evil.txt").exists());
        assert_eq!(
            fs::read_dir(directory.path().join("extensions"))
                .unwrap()
                .count(),
            0,
            "{expected}"
        );
    }

    // More files than an extension may hold.
    let names: Vec<String> = (0..=manifest::MAX_FILES)
        .map(|number| format!("many/{number}.txt"))
        .collect();
    let many: Vec<Packed<'_>> = names.iter().map(|name| packed(name, b"")).collect();
    let (_directory, staged) = stage_archive(&many, "");
    assert_eq!(
        staged.unwrap_err(),
        "the archive holds more than 1000 files"
    );

    // A file that unpacks to more than may be unpacked, whether the archive
    // says so or not: inflating stops past the limit.
    let zeros = vec![0u8; manifest::MAX_TOTAL_BYTES as usize + 1];
    for declared in [None, Some(10)] {
        let bomb = Packed {
            declared,
            ..packed("zeros.bin", &zeros)
        };
        let (_directory, staged) = stage_archive(&[bomb], "");
        assert_eq!(
            staged.unwrap_err(),
            "its files are larger than 64 MiB together"
        );
    }

    // What is no archive, or too large to be one.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("extensions");
    let text = directory.path().join("text.zip");
    fs::write(&text, "hello").unwrap();
    assert_eq!(
        install::stage(&text, &root, &known()).unwrap_err(),
        "it is not a ZIP archive"
    );
    let large = directory.path().join("large.zip");
    fs::File::create(&large)
        .unwrap()
        .set_len(install::MAX_ARCHIVE_BYTES + 1)
        .unwrap();
    assert_eq!(
        install::stage(&large, &root, &known()).unwrap_err(),
        "the archive is larger than 64 MiB"
    );
    let other = directory.path().join("notes.txt");
    fs::write(&other, "notes").unwrap();
    assert_eq!(
        install::stage(&other, &root, &known()).unwrap_err(),
        "choose a .zip archive or the extension.json of an extension folder"
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
}

#[test]
fn a_folder_is_staged_from_itself_or_from_its_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("extensions");
    let expected: Vec<String> = example_files("")
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for source in [example_folder(), example_folder().join(manifest::MANIFEST)] {
        let staged = install::stage(&source, &root, &known()).unwrap();
        assert_eq!(staged.manifest.id, EXAMPLE);
        assert_eq!(listing(&staged.folder), expected);
        assert_eq!(staged.source, source);
        staged.discard();
    }
    // The history of a version control system and old logs stay behind.
    let source = directory.path().join("source");
    fs::create_dir_all(source.join(".git/objects")).unwrap();
    fs::write(source.join(".git/objects/a"), "x").unwrap();
    fs::create_dir_all(source.join("logs")).unwrap();
    fs::write(source.join("logs/run-1.log"), "x").unwrap();
    for (name, _) in example_files("") {
        fs::copy(example_folder().join(&name), source.join(&name)).unwrap();
    }
    let staged = install::stage(&source, &root, &known()).unwrap();
    assert_eq!(listing(&staged.folder), expected);
    staged.discard();
}
