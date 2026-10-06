//! The `extension.json` of an installed extension: what it is, the program
//! it starts, what it uses and what it adds to the window. Everything is
//! checked strictly; a manifest that does not fit is refused with the field
//! and the reason.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{json, Map, Value};

/// The name of the manifest in the folder of an extension.
pub const MANIFEST: &str = "extension.json";
/// The folder of an installed extension that holds the logs of its runs. It
/// is not part of what is installed and is not copied from a source.
pub const LOGS: &str = "logs";

pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_ICON_BYTES: u64 = 64 * 1024;
/// The most files an extension may hold, and their size together.
pub const MAX_FILES: usize = 1000;
pub const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
/// The deepest a file may lie below the folder of the extension.
pub const MAX_DEPTH: usize = 12;
/// The longest path of a file inside the folder, in characters.
pub const MAX_PATH_CHARS: usize = 200;
const MAX_RIBBON_BUTTONS: usize = 8;
const MAX_FILE_ENTRIES: usize = 4;
const MAX_ARGS: usize = 32;
const MAX_ARG_CHARS: usize = 1024;
const MAX_COMMANDS: usize = 200;

/// The system this build runs on, as the keys of `command` name it.
pub const SYSTEM: &str = if cfg!(windows) {
    "windows"
} else if cfg!(target_os = "macos") {
    "macos"
} else {
    "linux"
};

/// The name of a system for a reason a person reads.
fn system_name(key: &str) -> &str {
    match key {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// A text with a translation per language: English always, others where the
/// author gave them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text(BTreeMap<String, String>);

impl Text {
    /// The text in the language the window shows, else English.
    pub fn get(&self) -> &str {
        self.in_language(crate::i18n::active_code())
    }

    pub fn english(&self) -> &str {
        self.in_language("en")
    }

    fn in_language(&self, code: &str) -> &str {
        self.0
            .get(code)
            .or_else(|| self.0.get("en"))
            .map_or("", String::as_str)
    }
}

/// A program that reads a script of the extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpreter {
    Python,
    PowerShell,
    Node,
    Sh,
}

impl Interpreter {
    pub const ALL: [Self; 4] = [Self::Python, Self::PowerShell, Self::Node, Self::Sh];

    pub fn key(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::PowerShell => "powershell",
            Self::Node => "node",
            Self::Sh => "sh",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|each| each.key() == key)
    }

    /// Whether the interpreter exists on a system at all.
    fn exists_on(self, system: &str) -> bool {
        !(self == Self::Sh && system == "windows")
    }
}

/// How the program of an extension starts on one system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub interpreter: Option<Interpreter>,
    /// The program or script, relative to the folder, with `/` between
    /// folders.
    pub program: String,
    pub args: Vec<String>,
}

impl Launch {
    /// The command line as the confirmation and the list show it.
    pub fn describe(&self) -> String {
        let mut words: Vec<String> = Vec::new();
        if let Some(interpreter) = self.interpreter {
            words.push(interpreter.key().to_owned());
        }
        words.push(self.program.clone());
        words.extend(self.args.iter().map(|arg| {
            if arg.is_empty() || arg.contains(char::is_whitespace) {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        }));
        words.join(" ")
    }
}

/// The commands of the local API an extension says it calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Commands {
    All,
    Listed(BTreeSet<String>),
}

impl Commands {
    /// The commands its token may send; `None` for every command.
    pub fn granted(&self) -> Option<BTreeSet<String>> {
        match self {
            Self::All => None,
            Self::Listed(names) => Some(names.clone()),
        }
    }

    pub fn value(&self) -> Value {
        match self {
            Self::All => json!("all"),
            Self::Listed(names) => json!(names),
        }
    }
}

/// What an extension says it uses besides the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Uses {
    pub network: bool,
    pub files_outside_folder: bool,
    pub commands: Commands,
}

/// A button the extension adds to the EXTENSIONS group of the ribbon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RibbonButton {
    pub id: String,
    pub label: Text,
    /// An SVG file in the folder.
    pub icon: String,
    pub tooltip: Option<Text>,
    /// What the button adds to the arguments of the command.
    pub args: Vec<String>,
}

/// A page of the File view that an extension may add a tile to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryPage {
    New,
    Export,
}

impl EntryPage {
    pub fn key(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Export => "export",
        }
    }
}

/// A tile the extension adds to the New or the Export page of the File view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub id: String,
    pub page: EntryPage,
    pub title: Text,
    pub detail: Option<Text>,
    pub args: Vec<String>,
}

/// A checked `extension.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub name: Text,
    pub version: String,
    pub author: String,
    pub description: Text,
    pub homepage: Option<String>,
    pub min_app_version: String,
    /// How it starts on this system.
    pub launch: Launch,
    pub uses: Uses,
    pub ribbon: Vec<RibbonButton>,
    pub file_view: Vec<FileEntry>,
}

impl Manifest {
    /// A button or tile by its id.
    pub fn args_of(&self, entry: &str) -> Option<&[String]> {
        self.ribbon
            .iter()
            .find(|button| button.id == entry)
            .map(|button| button.args.as_slice())
            .or_else(|| {
                self.file_view
                    .iter()
                    .find(|tile| tile.id == entry)
                    .map(|tile| tile.args.as_slice())
            })
    }

    /// What the manifest declares, as the local API and the confirmation
    /// report it, with its English texts.
    pub fn value(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name.english(),
            "version": self.version,
            "author": self.author,
            "description": self.description.english(),
            "homepage": self.homepage,
            "min_app_version": self.min_app_version,
            "command": self.launch.describe(),
            "uses": {
                "network": self.uses.network,
                "files_outside_folder": self.uses.files_outside_folder,
                "commands": self.uses.commands.value(),
            },
            "ribbon": self.ribbon.iter().map(|button| json!({
                "id": button.id,
                "label": button.label.english(),
                "tooltip": button.tooltip.as_ref().map(Text::english),
                "args": button.args,
            })).collect::<Vec<_>>(),
            "file_view": self.file_view.iter().map(|tile| json!({
                "id": tile.id,
                "page": tile.page.key(),
                "title": tile.title.english(),
                "args": tile.args,
            })).collect::<Vec<_>>(),
        })
    }
}

/// A version such as `1.2.3` or `1.2.0-beta.1`: up to four numbers, then
/// optionally a mark of a version before its release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    numbers: [u64; 4],
    pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty() || text.len() > 32 {
            return None;
        }
        // Build metadata after `+` does not order versions.
        let text = text.split_once('+').map_or(text, |(version, _)| version);
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, Some(pre)),
            None => (text, None),
        };
        if let Some(pre) = pre {
            // Parts of letters and digits between single dots.
            let fits = pre.split('.').all(|part| {
                !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
            });
            if !fits {
                return None;
            }
        }
        let parts: Vec<&str> = numbers.split('.').collect();
        if parts.is_empty() || parts.len() > 4 {
            return None;
        }
        let mut values = [0u64; 4];
        for (value, part) in values.iter_mut().zip(&parts) {
            if part.is_empty() || part.len() > 9 || !part.bytes().all(|byte| byte.is_ascii_digit())
            {
                return None;
            }
            *value = part.parse().ok()?;
        }
        Some(Self {
            numbers: values,
            pre: pre.map(str::to_owned),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        self.numbers.cmp(&other.numbers).then_with(|| {
            match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                // A version before its release comes before the release.
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(left), Some(right)) => compare_marks(left, right),
            }
        })
    }
}

/// The order of two marks before a release, as Semantic Versioning has it:
/// part by part, numbers by their value and before words, words by their
/// letters, and a mark that runs out first comes first. Marks that are
/// equal by that, such as `beta.01` and `beta.1`, are ordered by their text.
fn compare_marks(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    /// A part of digits only, without its leading zeros.
    fn number(part: &str) -> Option<&str> {
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.trim_start_matches('0'))
    }
    let mut lefts = left.split('.');
    let mut rights = right.split('.');
    loop {
        let ordering = match (lefts.next(), rights.next()) {
            (None, None) => return left.cmp(right),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => match (number(left), number(right)) {
                // Without leading zeros, a longer number is the larger one.
                (Some(left), Some(right)) => {
                    left.len().cmp(&right.len()).then_with(|| left.cmp(right))
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => left.cmp(right),
            },
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

/// The version of this application.
pub fn app_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is a version")
}

/// An id of an extension: 3 to 64 lowercase letters and digits in parts
/// joined by single dots or dashes, which is also the name of its folder.
pub fn valid_id(id: &str) -> bool {
    (3..=64).contains(&id.len()) && valid_entry_id(id) && !reserved_name(id)
}

/// An id of a button or a tile: as that of an extension, and as short as
/// one character.
pub fn valid_entry_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id.split(['.', '-']).all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// Names Windows keeps for devices, also with an extension after them.
fn reserved_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_lowercase();
    matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
    ) || ((stem.starts_with("com") || stem.starts_with("lpt"))
        && stem.len() == 4
        && stem
            .chars()
            .last()
            .is_some_and(|last| last.is_ascii_digit() || "¹²³".contains(last)))
}

/// Whether one part of a path is a name every system can keep as it is.
pub fn valid_component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 120
        && name != "."
        && name != ".."
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|character| character.is_control() || "<>:\"/\\|?*".contains(character))
        && !reserved_name(name)
}

/// A relative path inside the folder as the manifest or an archive writes
/// it: parts between `/` that are each a valid name.
pub fn relative_path(text: &str) -> Result<PathBuf, String> {
    if text.is_empty() {
        return Err("is empty".into());
    }
    if text.chars().count() > MAX_PATH_CHARS {
        return Err(format!("is longer than {MAX_PATH_CHARS} characters"));
    }
    if text.starts_with('/') || text.contains('\\') {
        return Err(
            "must be relative to the folder of the extension, with / between folders".into(),
        );
    }
    let mut path = PathBuf::new();
    let parts: Vec<&str> = text.split('/').collect();
    if parts.len() > MAX_DEPTH {
        return Err(format!("lies more than {MAX_DEPTH} folders deep"));
    }
    for part in parts {
        if part == ".." {
            return Err("leads out of the folder of the extension".into());
        }
        if !valid_component(part) {
            return Err(format!("has a name that cannot be used: {part:?}"));
        }
        path.push(part);
    }
    // The parts are names without separators, so the path stays below the
    // folder it is joined to.
    debug_assert!(path
        .components()
        .all(|part| matches!(part, Component::Normal(_))));
    Ok(path)
}

/// A file of the folder that a manifest names: inside it, through no link,
/// and a file.
fn file_inside(folder: &Path, text: &str) -> Result<PathBuf, String> {
    let relative = relative_path(text)?;
    let mut path = folder.to_path_buf();
    for part in relative.components() {
        path.push(part);
        let metadata = fs::symlink_metadata(&path).map_err(|_| "does not exist".to_owned())?;
        if metadata.file_type().is_symlink() {
            return Err("is reached through a link".into());
        }
    }
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
        return Err("is not a file".into());
    }
    let inside = match (fs::canonicalize(folder), fs::canonicalize(&path)) {
        (Ok(folder), Ok(file)) => file.starts_with(folder),
        _ => false,
    };
    if !inside {
        return Err("leads out of the folder of the extension".into());
    }
    Ok(path)
}

/// Whether an SVG may be shown as an icon: of a modest size, and referring
/// to nothing but its own elements.
pub fn check_icon(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() as u64 > MAX_ICON_BYTES {
        return Err(format!("is larger than {} KiB", MAX_ICON_BYTES / 1024));
    }
    let text = std::str::from_utf8(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes))
        .map_err(|_| "is not UTF-8 text".to_owned())?;
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<svg") {
        return Err("is not an SVG image".into());
    }
    for element in [
        "<script",
        "<foreignobject",
        "<image",
        "<!entity",
        "<?xml-stylesheet",
    ] {
        if lower.contains(element) {
            return Err(format!("holds {element}>, which an icon may not"));
        }
    }
    // Links and CSS addresses may only point inside the image.
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("href") {
        let after = rest[at + 4..].trim_start();
        if let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            let target = value.trim_start_matches(['"', '\'']);
            if !target.starts_with('#') {
                return Err("links to something outside the image".into());
            }
        }
        rest = &rest[at + 4..];
    }
    let mut rest = lower.as_str();
    while let Some(at) = rest.find("url(") {
        let target = rest[at + 4..].trim_start().trim_start_matches(['"', '\'']);
        if !target.starts_with('#') {
            return Err("refers to something outside the image".into());
        }
        rest = &rest[at + 4..];
    }
    if lower.contains("@import") {
        return Err("imports a style sheet".into());
    }
    Ok(())
}

/// The fields of one JSON object of the manifest, read with the place of
/// the object for the reasons.
struct Fields<'a> {
    object: &'a Map<String, Value>,
    at: String,
}

impl<'a> Fields<'a> {
    fn new(value: &'a Value, at: &str, allowed: &[&str]) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| format!("{at} must be an object"))?;
        if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
            return Err(format!(
                "{}{unknown} is not a field of the manifest; the fields here are {}",
                prefix(at),
                allowed.join(", ")
            ));
        }
        Ok(Self {
            object,
            at: at.to_owned(),
        })
    }

    fn name(&self, key: &str) -> String {
        format!("{}{key}", prefix(&self.at))
    }

    fn get(&self, key: &str) -> Result<&'a Value, String> {
        self.object
            .get(key)
            .ok_or_else(|| format!("{} is missing", self.name(key)))
    }

    fn string(&self, key: &str, max: usize) -> Result<String, String> {
        plain_text(self.get(key)?, &self.name(key), max)
    }

    fn optional_string(&self, key: &str, max: usize) -> Result<Option<String>, String> {
        self.object
            .get(key)
            .map(|value| plain_text(value, &self.name(key), max))
            .transpose()
    }

    fn text(&self, key: &str, max: usize) -> Result<Text, String> {
        translated_text(self.get(key)?, &self.name(key), max)
    }

    fn optional_text(&self, key: &str, max: usize) -> Result<Option<Text>, String> {
        self.object
            .get(key)
            .map(|value| translated_text(value, &self.name(key), max))
            .transpose()
    }

    fn boolean(&self, key: &str) -> Result<bool, String> {
        self.get(key)?
            .as_bool()
            .ok_or_else(|| format!("{} must be true or false", self.name(key)))
    }

    fn arguments(&self, key: &str) -> Result<Vec<String>, String> {
        let Some(value) = self.object.get(key) else {
            return Ok(Vec::new());
        };
        let name = self.name(key);
        let items = value
            .as_array()
            .ok_or_else(|| format!("{name} must be a list of texts"))?;
        if items.len() > MAX_ARGS {
            return Err(format!("{name} holds more than {MAX_ARGS} arguments"));
        }
        items
            .iter()
            .map(|item| {
                let argument = item
                    .as_str()
                    .ok_or_else(|| format!("{name} must be a list of texts"))?;
                if argument.chars().count() > MAX_ARG_CHARS {
                    return Err(format!(
                        "{name} holds an argument longer than {MAX_ARG_CHARS} characters"
                    ));
                }
                if argument.contains('\0') {
                    return Err(format!("{name} holds an argument with a NUL character"));
                }
                Ok(argument.to_owned())
            })
            .collect()
    }

    fn list(&self, key: &str, max: usize) -> Result<&'a [Value], String> {
        let Some(value) = self.object.get(key) else {
            return Ok(&[]);
        };
        let items = value
            .as_array()
            .ok_or_else(|| format!("{} must be a list", self.name(key)))?;
        if items.len() > max {
            return Err(format!("{} holds more than {max} entries", self.name(key)));
        }
        Ok(items)
    }
}

fn prefix(at: &str) -> String {
    if at.is_empty() {
        String::new()
    } else {
        format!("{at}.")
    }
}

/// A text of one line without control characters, of 1 to `max` characters.
fn plain_text(value: &Value, name: &str, max: usize) -> Result<String, String> {
    let text = value
        .as_str()
        .ok_or_else(|| format!("{name} must be a text"))?;
    if text.trim().is_empty() {
        return Err(format!("{name} is empty"));
    }
    if text.chars().count() > max {
        return Err(format!("{name} is longer than {max} characters"));
    }
    if text.chars().any(char::is_control) {
        return Err(format!(
            "{name} holds a control character such as a line break"
        ));
    }
    Ok(text.trim().to_owned())
}

/// A text, or an object with a text per language code; English is needed.
fn translated_text(value: &Value, name: &str, max: usize) -> Result<Text, String> {
    if value.is_string() {
        let text = plain_text(value, name, max)?;
        return Ok(Text(BTreeMap::from([("en".to_owned(), text)])));
    }
    let object = value
        .as_object()
        .ok_or_else(|| format!("{name} must be a text or an object with a text per language"))?;
    let mut texts = BTreeMap::new();
    for (code, text) in object {
        let fits =
            (2..=3).contains(&code.len()) && code.bytes().all(|byte| byte.is_ascii_lowercase());
        if !fits {
            return Err(format!(
                "{name}.{code} is not a language code such as en or nl"
            ));
        }
        texts.insert(
            code.clone(),
            plain_text(text, &format!("{name}.{code}"), max)?,
        );
    }
    if !texts.contains_key("en") {
        return Err(format!("{name} needs an English text under en"));
    }
    Ok(Text(texts))
}

fn checked_id(value: String, name: &str) -> Result<String, String> {
    if valid_id(&value) {
        Ok(value)
    } else {
        Err(format!(
            "{name} must be 3 to 64 lowercase letters and digits in parts joined by dots or dashes, such as org.example.my-tool"
        ))
    }
}

/// The id of a button or a tile.
fn checked_entry_id(value: String, name: &str) -> Result<String, String> {
    if valid_entry_id(&value) {
        Ok(value)
    } else {
        Err(format!(
            "{name} must be 1 to 64 lowercase letters and digits in parts joined by dots or dashes, such as count"
        ))
    }
}

fn checked_version(value: String, name: &str) -> Result<String, String> {
    if Version::parse(&value).is_some() {
        Ok(value)
    } else {
        Err(format!(
            "{name} must be a version such as 1.0.0 or 1.2.0-beta.1"
        ))
    }
}

/// One way to start the program, checked against the system it is for.
fn launch(value: &Value, at: &str, system: &str, folder: &Path) -> Result<Launch, String> {
    let fields = Fields::new(value, at, &["program", "interpreter", "args"])?;
    let interpreter = match fields.optional_string("interpreter", 20)? {
        None => None,
        Some(key) => {
            let interpreter = Interpreter::from_key(&key).ok_or_else(|| {
                format!(
                    "{} must be one of {}",
                    fields.name("interpreter"),
                    Interpreter::ALL.map(Interpreter::key).join(", ")
                )
            })?;
            if !interpreter.exists_on(system) {
                return Err(format!(
                    "{}: {key} is not available on {}",
                    fields.name("interpreter"),
                    system_name(system)
                ));
            }
            Some(interpreter)
        }
    };
    let program = fields.string("program", MAX_PATH_CHARS)?;
    file_inside(folder, &program)
        .map_err(|reason| format!("{} {program} {reason}", fields.name("program")))?;
    let lower = program.to_ascii_lowercase();
    if interpreter.is_none() && system == "windows" {
        let runs = [".exe", ".com", ".bat", ".cmd"]
            .iter()
            .any(|extension| lower.ends_with(extension));
        if !runs {
            return Err(format!(
                "{}: {program} is not a program Windows starts; name an interpreter such as powershell or python",
                fields.name("program")
            ));
        }
    }
    Ok(Launch {
        interpreter,
        program,
        args: fields.arguments("args")?,
    })
}

/// The way to start on this system: `command` is one launch for every
/// system, or one per system key with `any` for the rest.
fn command(value: &Value, folder: &Path) -> Result<Launch, String> {
    const SYSTEMS: [&str; 4] = ["windows", "macos", "linux", "any"];
    let object = value
        .as_object()
        .ok_or_else(|| "command must be an object".to_owned())?;
    if object.contains_key("program") {
        return launch(value, "command", SYSTEM, folder);
    }
    if object.is_empty() {
        return Err("command needs program, or a launch under windows, macos, linux or any".into());
    }
    let fields = Fields::new(value, "command", &SYSTEMS)?;
    let mut chosen = None;
    for key in SYSTEMS {
        let Some(each) = fields.object.get(key) else {
            continue;
        };
        // Every launch is checked, each against its own system; the one for
        // this system is used.
        let system = if key == "any" { SYSTEM } else { key };
        let checked = launch(each, &format!("command.{key}"), system, folder)?;
        if chosen.is_none() && (key == SYSTEM || key == "any") {
            chosen = Some(checked);
        }
    }
    chosen.ok_or_else(|| {
        format!(
            "it cannot run on {}: command has no launch under {SYSTEM} or any",
            system_name(SYSTEM)
        )
    })
}

fn uses(value: &Value, known: &[&str]) -> Result<Uses, String> {
    let fields = Fields::new(
        value,
        "uses",
        &["network", "files_outside_folder", "commands"],
    )?;
    let commands = match fields.get("commands")? {
        Value::String(word) if word == "all" => Commands::All,
        Value::Array(names) => {
            if names.is_empty() || names.len() > MAX_COMMANDS {
                return Err(format!(
                    "uses.commands must list 1 to {MAX_COMMANDS} commands, or be \"all\""
                ));
            }
            let mut listed = BTreeSet::new();
            for name in names {
                let name = name
                    .as_str()
                    .ok_or_else(|| "uses.commands must list names of commands".to_owned())?;
                if !known.contains(&name) {
                    return Err(format!(
                        "uses.commands: {name} is not a command of the local API"
                    ));
                }
                if !listed.insert(name.to_owned()) {
                    return Err(format!("uses.commands lists {name} twice"));
                }
            }
            Commands::Listed(listed)
        }
        _ => {
            return Err("uses.commands must be a list of commands or \"all\"".into());
        }
    };
    Ok(Uses {
        network: fields.boolean("network")?,
        files_outside_folder: fields.boolean("files_outside_folder")?,
        commands,
    })
}

/// Read and check the manifest of the extension in a folder, with the
/// commands of the local API it may list.
pub fn read(folder: &Path, known_commands: &[&str]) -> Result<Manifest, String> {
    let path = folder.join(MANIFEST);
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| format!("the folder has no {MANIFEST}"))?;
    if !metadata.is_file() {
        return Err(format!("{MANIFEST} is not a file"));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(format!(
            "{MANIFEST} is larger than {} KiB",
            MAX_MANIFEST_BYTES / 1024
        ));
    }
    let bytes = fs::read(&path).map_err(|error| format!("{MANIFEST}: {error}"))?;
    parse(&bytes, folder, known_commands)
}

/// Check the bytes of a manifest for the folder it lies in.
pub fn parse(bytes: &[u8], folder: &Path, known_commands: &[&str]) -> Result<Manifest, String> {
    // A byte-order mark, as some editors write, is not part of the JSON.
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| format!("{MANIFEST} is not valid JSON: {error}"))?;
    let fields = Fields::new(
        &value,
        "",
        &[
            "id",
            "name",
            "version",
            "author",
            "description",
            "homepage",
            "min_app_version",
            "command",
            "uses",
            "contributes",
        ],
    )
    .map_err(|error| {
        if value.is_object() {
            error
        } else {
            format!("{MANIFEST} must hold one JSON object")
        }
    })?;
    let id = checked_id(fields.string("id", 64)?, "id")?;
    if super::BUILT_IN.iter().any(|extension| extension.id == id) {
        return Err(format!("id {id} is the id of a built-in extension"));
    }
    let version = checked_version(fields.string("version", 32)?, "version")?;
    let min_app_version =
        checked_version(fields.string("min_app_version", 32)?, "min_app_version")?;
    let needed = Version::parse(&min_app_version).expect("checked above");
    if needed > app_version() {
        return Err(format!(
            "it needs Open Pointcloud Studio {min_app_version} or newer; this is {}",
            env!("CARGO_PKG_VERSION")
        ));
    }
    let homepage = fields.optional_string("homepage", 200)?;
    if let Some(homepage) = &homepage {
        let web = homepage.starts_with("https://") || homepage.starts_with("http://");
        if !web || homepage.contains(char::is_whitespace) {
            return Err("homepage must be a web address that starts with https://".into());
        }
    }
    let launch = command(fields.get("command")?, folder)?;
    let uses = uses(fields.get("uses")?, known_commands)?;

    let mut ribbon = Vec::new();
    let mut file_view = Vec::new();
    if let Some(contributes) = fields.object.get("contributes") {
        let contributes = Fields::new(contributes, "contributes", &["ribbon", "file_view"])?;
        let mut ids = BTreeSet::new();
        for (index, button) in contributes
            .list("ribbon", MAX_RIBBON_BUTTONS)?
            .iter()
            .enumerate()
        {
            let at = format!("contributes.ribbon[{index}]");
            let fields = Fields::new(button, &at, &["id", "label", "icon", "tooltip", "args"])?;
            let id = checked_entry_id(fields.string("id", 64)?, &fields.name("id"))?;
            if !ids.insert(id.clone()) {
                return Err(format!("{}: {id} is used twice", fields.name("id")));
            }
            let icon = fields.string("icon", MAX_PATH_CHARS)?;
            if !icon.to_ascii_lowercase().ends_with(".svg") {
                return Err(format!(
                    "{}: {icon} is not an .svg file",
                    fields.name("icon")
                ));
            }
            let file = file_inside(folder, &icon)
                .map_err(|reason| format!("{} {icon} {reason}", fields.name("icon")))?;
            let size = fs::metadata(&file).map_or(u64::MAX, |metadata| metadata.len());
            if size > MAX_ICON_BYTES {
                return Err(format!(
                    "{} {icon} is larger than {} KiB",
                    fields.name("icon"),
                    MAX_ICON_BYTES / 1024
                ));
            }
            let bytes = fs::read(&file)
                .map_err(|error| format!("{} {icon}: {error}", fields.name("icon")))?;
            check_icon(&bytes)
                .map_err(|reason| format!("{} {icon} {reason}", fields.name("icon")))?;
            ribbon.push(RibbonButton {
                id,
                label: fields.text("label", 24)?,
                icon,
                tooltip: fields.optional_text("tooltip", 200)?,
                args: fields.arguments("args")?,
            });
        }
        for (index, tile) in contributes
            .list("file_view", MAX_FILE_ENTRIES)?
            .iter()
            .enumerate()
        {
            let at = format!("contributes.file_view[{index}]");
            let fields = Fields::new(tile, &at, &["id", "page", "title", "detail", "args"])?;
            let id = checked_entry_id(fields.string("id", 64)?, &fields.name("id"))?;
            if !ids.insert(id.clone()) {
                return Err(format!("{}: {id} is used twice", fields.name("id")));
            }
            let page = match fields.string("page", 10)?.as_str() {
                "new" => EntryPage::New,
                "export" => EntryPage::Export,
                _ => return Err(format!("{} must be new or export", fields.name("page"))),
            };
            file_view.push(FileEntry {
                id,
                page,
                title: fields.text("title", 60)?,
                detail: fields.optional_text("detail", 160)?,
                args: fields.arguments("args")?,
            });
        }
    }

    Ok(Manifest {
        id,
        name: fields.text("name", 60)?,
        version,
        author: fields.string("author", 100)?,
        description: fields.text("description", 500)?,
        homepage,
        min_app_version,
        launch,
        uses,
        ribbon,
        file_view,
    })
}

/// What a check of a folder found: its files, relative to it, and their
/// size together.
#[derive(Debug, Default)]
pub struct FolderSummary {
    pub files: Vec<PathBuf>,
    pub bytes: u64,
}

/// Walk the folder of an extension: no links, names every system keeps,
/// paths of a modest length and depth, and at most `MAX_FILES` files of at
/// most `MAX_TOTAL_BYTES` together. The folders in `skip` at the top are left
/// out, such as the logs of an installed extension.
pub fn check_folder(folder: &Path, skip: &[&str]) -> Result<FolderSummary, String> {
    let metadata = fs::symlink_metadata(folder).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("the folder is a link".into());
    }
    if !metadata.is_dir() {
        return Err("it is not a folder".into());
    }
    let mut summary = FolderSummary::default();
    let mut pending = vec![(folder.to_path_buf(), PathBuf::new())];
    while let Some((absolute, relative)) = pending.pop() {
        let entries =
            fs::read_dir(&absolute).map_err(|error| format!("{}: {error}", shown(&relative)))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(format!(
                    "{} holds a name that is not Unicode",
                    shown(&relative)
                ));
            };
            if relative.as_os_str().is_empty() && skip.contains(&name) {
                continue;
            }
            let inner = relative.join(name);
            if !valid_component(name) {
                return Err(format!("{} has a name that cannot be used", shown(&inner)));
            }
            if inner.to_string_lossy().chars().count() > MAX_PATH_CHARS {
                return Err(format!(
                    "{} is a path longer than {MAX_PATH_CHARS} characters",
                    shown(&inner)
                ));
            }
            if inner.components().count() > MAX_DEPTH {
                return Err(format!(
                    "{} lies more than {MAX_DEPTH} folders deep",
                    shown(&inner)
                ));
            }
            let kind = fs::symlink_metadata(entry.path()).map_err(|error| error.to_string())?;
            if kind.file_type().is_symlink() {
                return Err(format!(
                    "{} is a link; links are not allowed",
                    shown(&inner)
                ));
            }
            if kind.is_dir() {
                pending.push((entry.path(), inner));
            } else if kind.is_file() {
                summary.bytes += kind.len();
                summary.files.push(inner);
                if summary.files.len() > MAX_FILES {
                    return Err(format!("it holds more than {MAX_FILES} files"));
                }
                if summary.bytes > MAX_TOTAL_BYTES {
                    return Err(format!(
                        "its files are larger than {} MiB together",
                        MAX_TOTAL_BYTES / (1024 * 1024)
                    ));
                }
            } else {
                return Err(format!("{} is neither a file nor a folder", shown(&inner)));
            }
        }
    }
    summary.files.sort();
    Ok(summary)
}

/// A relative path with `/` between its parts, for a reason.
pub fn shown(relative: &Path) -> String {
    if relative.as_os_str().is_empty() {
        return "the folder".into();
    }
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}
