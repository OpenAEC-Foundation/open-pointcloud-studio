//! The language of the user interface. The English text is the key: other
//! languages are tables from the English text, and a text without a
//! translation stays English.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// A language other than English: the code a locale starts with, the name the
/// language has for itself and its table from the English texts.
struct Table {
    code: &'static str,
    name: &'static str,
    json: &'static str,
}

/// Every language with a table. A further language is one JSON file in
/// `assets/locales` and one line here; Settings, the command that sets the
/// language and the tests follow this list. Only the rows of `set_language`
/// in API.md and MCP.md name the codes by hand.
const TABLES: &[Table] = &[Table {
    code: "nl",
    name: "Nederlands",
    json: include_str!("../../assets/locales/nl.json"),
}];

/// What the user chose in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// The language of the system when there is a table for it.
    Auto,
    English,
    /// A language of `TABLES`, by its place in that list.
    Table(u8),
}

impl Language {
    /// Every choice, in the order Settings offers them.
    pub fn all() -> Vec<Self> {
        let tables = (0..TABLES.len()).filter_map(|index| u8::try_from(index).ok());
        [Self::Auto, Self::English]
            .into_iter()
            .chain(tables.map(Self::Table))
            .collect()
    }

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Table(index) => TABLES
                .get(usize::from(index))
                .map_or("en", |table| table.code),
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|language| language.key() == value)
    }

    /// Every key `from_key` accepts, for the command that sets the language.
    pub fn keys() -> Vec<&'static str> {
        Self::all().into_iter().map(Self::key).collect()
    }

    fn code(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::English => 1,
            Self::Table(index) => index.saturating_add(2),
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => tr("Auto-detect"),
            Self::English => "English",
            Self::Table(index) => TABLES
                .get(usize::from(*index))
                .map_or("English", |table| table.name),
        })
    }
}

static CHOICE: AtomicU8 = AtomicU8::new(1);
/// The table texts are looked up in: zero for none, otherwise its place in
/// `TABLES` plus one.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

fn tables() -> &'static [HashMap<String, String>] {
    static LOADED: OnceLock<Vec<HashMap<String, String>>> = OnceLock::new();
    LOADED.get_or_init(|| {
        TABLES
            .iter()
            .map(|table| serde_json::from_str(table.json).unwrap_or_default())
            .collect()
    })
}

/// The language tag of the system, such as `nl-NL`.
#[cfg(windows)]
fn system_locale() -> String {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetUserDefaultLocaleName(name: *mut u16, length: i32) -> i32;
    }
    let mut name = [0u16; 85];
    // SAFETY: the buffer holds the 85 characters a locale name can take.
    let length = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
    String::from_utf16_lossy(&name[..(length.max(1) - 1) as usize])
}

/// The locale the environment names; empty when it names none.
#[cfg(not(windows))]
fn environment_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default()
}

/// The first language of the user's own list, such as `nl-NL`. An
/// application started from the Finder or the Dock has no locale in its
/// environment, so the environment is asked only when that list is empty.
#[cfg(target_os = "macos")]
fn system_locale() -> String {
    objc2_foundation::NSLocale::preferredLanguages()
        .firstObject()
        .map(|language| language.to_string())
        .filter(|language| !language.is_empty())
        .unwrap_or_else(environment_locale)
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_locale() -> String {
    environment_locale()
}

/// The table for a locale such as `nl-NL` or `nl_BE.UTF-8`: the first whose
/// code the locale starts with.
fn table_for_locale(locale: &str) -> Option<u8> {
    let locale = locale.to_ascii_lowercase();
    TABLES
        .iter()
        .position(|table| {
            locale.strip_prefix(table.code).is_some_and(|rest| {
                // `nl` must not claim a longer language code.
                !rest.starts_with(|next: char| next.is_ascii_alphabetic())
            })
        })
        .and_then(|index| u8::try_from(index).ok())
}

/// Use a language from now on. Views show it the next time they are built.
pub fn set(language: Language) {
    let table = match language {
        Language::Auto => table_for_locale(&system_locale()),
        Language::English => None,
        Language::Table(index) => Some(index),
    };
    CHOICE.store(language.code(), Ordering::Relaxed);
    ACTIVE.store(
        table.map_or(0, |index| index.saturating_add(1)),
        Ordering::Relaxed,
    );
}

pub fn choice() -> Language {
    let code = CHOICE.load(Ordering::Relaxed);
    Language::all()
        .into_iter()
        .find(|language| language.code() == code)
        .unwrap_or(Language::English)
}

/// The code of the language texts are shown in: `en`, or the code of the
/// table in use.
pub fn active_code() -> &'static str {
    usize::from(ACTIVE.load(Ordering::Relaxed))
        .checked_sub(1)
        .and_then(|index| TABLES.get(index))
        .map_or("en", |table| table.code)
}

/// A text in the language in use.
pub fn tr(text: &str) -> &str {
    let Some(index) = usize::from(ACTIVE.load(Ordering::Relaxed)).checked_sub(1) else {
        return text;
    };
    tables()
        .get(index)
        .and_then(|table| table.get(text))
        .map_or(text, String::as_str)
}

/// A text in the language in use, with every `{name}` in it replaced by the
/// value given for that name. A text with numbers in it is translated as one
/// sentence, so each language puts the numbers where it needs them.
pub fn tr_args(text: &str, values: &[(&str, &dyn fmt::Display)]) -> String {
    let mut filled = tr(text).to_owned();
    for (name, value) in values {
        filled = filled.replace(&format!("{{{name}}}"), &value.to_string());
    }
    filled
}

/// Whether every table has an entry for a text, for tests of texts that
/// reach `tr` through a constant rather than as a literal. `tr` itself falls
/// back to English, so its answer does not show a missing entry.
#[cfg(test)]
pub(crate) fn has_entry(text: &str) -> bool {
    tables().iter().all(|table| table.contains_key(text))
}

/// Marks an English text that is translated where it is shown rather than
/// here, such as a label in a table of rows. It returns the text unchanged;
/// the mark lets the test of the tables find the text.
pub const fn key(text: &'static str) -> &'static str {
    text
}

fn language_path() -> Option<PathBuf> {
    // Tests run beside an installed application and leave its settings as
    // they are.
    if cfg!(test) {
        return None;
    }
    crate::preferences::config_directory().map(|directory| directory.join("language"))
}

fn load_from(path: &Path) -> Option<Language> {
    Language::from_key(std::fs::read_to_string(path).ok()?.trim())
}

fn save_to(path: &Path, language: Language) {
    if let Some(directory) = path.parent() {
        if std::fs::create_dir_all(directory).is_ok() {
            let _ = std::fs::write(path, language.key());
        }
    }
}

/// The language chosen in an earlier session; the system's otherwise.
pub fn load() -> Language {
    language_path()
        .and_then(|path| load_from(&path))
        .unwrap_or(Language::Auto)
}

pub fn save(language: Language) {
    if let Some(path) = language_path() {
        save_to(&path, language);
    }
}

/// The language is one setting of the whole process, so tests that change it
/// or read translated text hold this while they do, and leave the language as
/// they found it.
#[cfg(test)]
pub(crate) struct TestLanguage {
    before: Language,
    _turn: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl TestLanguage {
    pub(crate) fn hold(language: Language) -> Self {
        static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let turn = TURN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = choice();
        set(language);
        Self {
            before,
            _turn: turn,
        }
    }
}

#[cfg(test)]
impl Drop for TestLanguage {
    fn drop(&mut self) {
        set(self.before);
    }
}

/// Reading texts out of the source files, for the tests that compare the
/// sources with a list or a table.
#[cfg(test)]
pub(crate) mod source_scan {
    /// The string literal that starts at the opening quote at `start`: its
    /// text with the escapes resolved, and where the source goes on after its
    /// closing quote. `None` when no literal starts there or it does not end.
    pub(crate) fn literal_at(source: &str, start: usize) -> Option<(String, usize)> {
        let body = source.get(start..)?.strip_prefix('"')?;
        let mut characters = body.chars();
        let mut text = String::new();
        loop {
            match characters.next()? {
                '"' => return Some((text, source.len() - characters.as_str().len())),
                '\\' => match characters.next()? {
                    'n' => text.push('\n'),
                    't' => text.push('\t'),
                    'u' => {
                        let digits: String = characters
                            .by_ref()
                            .skip_while(|character| *character == '{')
                            .take_while(|character| *character != '}')
                            .collect();
                        text.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
                    }
                    // A backslash at the end of a line joins the next line
                    // without its indentation.
                    '\n' | '\r' => characters = characters.as_str().trim_start().chars(),
                    other => text.push(other),
                },
                other => text.push(other),
            }
        }
    }

    /// Where argument number `position` starts in an argument list, given
    /// where the list starts after its opening parenthesis. `None` when the
    /// list has fewer arguments.
    fn argument_start(source: &str, list: usize, position: usize) -> Option<usize> {
        let mut at = list;
        let mut depth = 0usize;
        let mut number = 0usize;
        loop {
            let rest = source.get(at..)?;
            if depth == 0 && number == position {
                return Some(source.len() - rest.trim_start().len());
            }
            let character = rest.chars().next()?;
            at += character.len_utf8();
            match character {
                '"' => at = literal_at(source, at - 1)?.1,
                // A comment may hold quotes and brackets that mean nothing.
                '/' if rest.starts_with("//") => at += rest.find('\n')?,
                // A character such as `'('`; a lifetime has no closing quote.
                '\'' => {
                    let mut characters = rest[1..].chars();
                    let first = characters.next()?;
                    if first == '\\' {
                        at += 1 + rest[2..].find('\'')? + 1;
                    } else if characters.next() == Some('\'') {
                        at += first.len_utf8() + 1;
                    }
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.checked_sub(1)?,
                ',' if depth == 0 => number += 1,
                _ => {}
            }
        }
    }

    /// Every string literal that is argument number `position`, counted from
    /// zero, of a call of `name`.
    pub(crate) fn literal_arguments(source: &str, name: &str, position: usize) -> Vec<String> {
        let mut found = Vec::new();
        for (at, _) in source.match_indices(name) {
            // `str(` is not a call of `tr`.
            let part_of_longer_name = source[..at]
                .chars()
                .next_back()
                .is_some_and(|before| before.is_alphanumeric() || before == '_');
            let list = at + name.len() + 1;
            if part_of_longer_name || !source[at + name.len()..].starts_with('(') {
                continue;
            }
            let literal = argument_start(source, list, position)
                .and_then(|argument| literal_at(source, argument));
            if let Some((text, _)) = literal {
                found.push(text);
            }
        }
        found
    }

    #[test]
    fn literals_are_read_with_their_escapes() {
        let source =
            "a(\"one\"); tr(\n    \"two \\\"x\\\" \\u{2026}\",\n); str(\"three\"); tr(name)";
        assert_eq!(literal_arguments(source, "a", 0), ["one"]);
        assert_eq!(literal_arguments(source, "tr", 0), ["two \"x\" …"]);
        assert_eq!(literal_at("\"open", 0), None);
        let joined = "\"a\\\n      b\" rest";
        let (text, end) = literal_at(joined, 0).unwrap();
        assert_eq!((text.as_str(), &joined[end..]), ("ab", " rest"));
    }

    #[test]
    fn later_arguments_are_found_past_calls_texts_and_comments() {
        let source = "tip(icon(Kind::Open, 20.0), \"Open\", None);\n\
                      tip(\n    // the user's icon (large)\n    icon(')', \"x, y\"),\n    \"Save\",\n);\n\
                      tip(icon(), label, None); tip(one)";
        assert_eq!(literal_arguments(source, "tip", 1), ["Open", "Save"]);
        assert_eq!(literal_arguments(source, "icon", 1), ["x, y"]);
        assert!(literal_arguments(source, "tip", 3).is_empty());
        let lifetime = "row(|name: &'static str| name, \"Label\")";
        assert_eq!(literal_arguments(lifetime, "row", 1), ["Label"]);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::source_scan::literal_arguments;
    use super::*;

    /// The source files with texts the user sees. A new module with such
    /// texts is one line here.
    const SOURCES: &[(&str, &str)] = &[
        ("main.rs", include_str!("main.rs")),
        ("bag_panel.rs", include_str!("bag_panel.rs")),
        ("cad_viewer.rs", include_str!("cad_viewer.rs")),
        ("cli_help.rs", include_str!("cli_help.rs")),
        ("closed_mesh.rs", include_str!("closed_mesh.rs")),
        ("drawing.rs", include_str!("drawing.rs")),
        ("drawing_crop.rs", include_str!("drawing_crop.rs")),
        ("drawing_view.rs", include_str!("drawing_view.rs")),
        ("extensions/mod.rs", include_str!("extensions/mod.rs")),
        ("extensions/page.rs", include_str!("extensions/page.rs")),
        ("faces.rs", include_str!("faces.rs")),
        ("file_photos.rs", include_str!("file_photos.rs")),
        ("file_view.rs", include_str!("file_view.rs")),
        ("i18n.rs", include_str!("i18n.rs")),
        ("index_jobs.rs", include_str!("index_jobs.rs")),
        ("measure.rs", include_str!("measure.rs")),
        ("mesh_export.rs", include_str!("mesh_export.rs")),
        ("mesh_wizard.rs", include_str!("mesh_wizard.rs")),
        ("mesh_to_plans/mod.rs", include_str!("mesh_to_plans/mod.rs")),
        (
            "mesh_to_plans/pipeline.rs",
            include_str!("mesh_to_plans/pipeline.rs"),
        ),
        (
            "mesh_to_plans/prepare.rs",
            include_str!("mesh_to_plans/prepare.rs"),
        ),
        (
            "mesh_to_plans/project.rs",
            include_str!("mesh_to_plans/project.rs"),
        ),
        (
            "mesh_to_plans/strip.rs",
            include_str!("mesh_to_plans/strip.rs"),
        ),
        ("open_progress.rs", include_str!("open_progress.rs")),
        ("photo_colours.rs", include_str!("photo_colours.rs")),
        (
            "opencad_properties.rs",
            include_str!("opencad_properties.rs"),
        ),
        ("opencad_ribbon.rs", include_str!("opencad_ribbon.rs")),
        ("section_detail.rs", include_str!("section_detail.rs")),
        ("section_fill.rs", include_str!("section_fill.rs")),
        ("settings_dialog.rs", include_str!("settings_dialog.rs")),
        ("status_bar.rs", include_str!("status_bar.rs")),
        ("station_photos.rs", include_str!("station_photos.rs")),
        ("ui_theme.rs", include_str!("ui_theme.rs")),
        ("view_cube.rs", include_str!("view_cube.rs")),
        ("view_tabs.rs", include_str!("view_tabs.rs")),
        ("project_browser.rs", include_str!("project_browser.rs")),
        ("sheet_dialog.rs", include_str!("sheet_dialog.rs")),
        ("views.rs", include_str!("views.rs")),
        ("layouts/mod.rs", include_str!("layouts/mod.rs")),
        ("layouts/model.rs", include_str!("layouts/model.rs")),
        ("layouts/panels.rs", include_str!("layouts/panels.rs")),
        ("layouts/plot.rs", include_str!("layouts/plot.rs")),
        ("locks.rs", include_str!("locks.rs")),
        ("drawing_notes.rs", include_str!("drawing_notes.rs")),
    ];

    /// The functions and local helpers that translate a text they are given,
    /// with the place of that text among their arguments, counted from zero.
    /// `key` marks a text that is translated where it is shown.
    const TRANSLATING: &[(&str, usize)] = &[
        ("tr", 0),
        ("tr_args", 0),
        ("key", 0),
        // main.rs
        ("small_tool_button", 0),
        ("small_tool_button_when", 0),
        ("icon_tool_button_when", 0),
        ("large_tool_button", 0),
        ("large_tool_button_when", 0),
        ("small_color_button", 0),
        ("ribbon_group", 0),
        ("preset", 0),
        // opencad_ribbon.rs and opencad_properties.rs
        ("quick_access_btn", 1),
        ("render_group", 0),
        ("render_group_items", 0),
        ("property_row", 0),
        ("property_input", 0),
        ("property_control", 0),
        ("section_header", 0),
        // mesh_wizard.rs
        ("option_input", 0),
        ("option_control", 0),
        // file_view.rs
        ("item", 0),
        ("entry", 0),
        ("caption", 0),
        // settings_dialog.rs
        ("tab", 0),
        ("heading", 0),
        ("label", 0),
        ("fact", 0),
        // views.rs
        ("tool", 0),
        ("small", 0),
        // file_photos.rs
        ("refusal", 0),
    ];

    /// Every text the sources hand to the translation, with the file it was
    /// found in.
    fn translated_literals() -> Vec<(&'static str, String)> {
        let mut found = Vec::new();
        for (file, source) in SOURCES {
            for (name, position) in TRANSLATING {
                for text in literal_arguments(source, name, *position) {
                    found.push((*file, text));
                }
            }
        }
        found
    }

    #[test]
    fn tables_are_valid_and_keep_untranslated_text_english() {
        assert_eq!(tables().len(), TABLES.len());
        for (table, texts) in TABLES.iter().zip(tables()) {
            assert!(texts.len() > 100, "{}", table.code);
            // No entry maps a text to nothing, and none differs from the text
            // it should be by the spacing around it alone.
            for (english, translated) in texts {
                assert!(!translated.trim().is_empty(), "{english}");
                assert_eq!(translated.trim(), translated, "{english}");
            }
        }
        let _language = TestLanguage::hold(Language::Table(0));
        assert_eq!(tr("Settings"), "Instellingen");
        let unknown = "A text without an entry";
        assert_eq!(tr(unknown), unknown);
        set(Language::English);
        assert_eq!(tr("Settings"), "Settings");
    }

    #[test]
    fn language_keys_round_trip_and_follow_the_tables() {
        // The list of languages is the table, so a further table needs no
        // change here.
        let expected: Vec<&str> = ["auto", "en"]
            .into_iter()
            .chain(TABLES.iter().map(|table| table.code))
            .collect();
        assert_eq!(Language::keys(), expected);
        let codes: BTreeSet<&str> = expected.iter().copied().collect();
        assert_eq!(codes.len(), expected.len(), "a code is used twice");
        for language in Language::all() {
            assert_eq!(Language::from_key(language.key()), Some(language));
        }
        assert_eq!(Language::from_key("xx"), None);
        assert_eq!(Language::from_key("nl"), Some(Language::Table(0)));
        assert_eq!(Language::Table(0).to_string(), "Nederlands");

        let _language = TestLanguage::hold(Language::Table(0));
        assert_eq!(choice(), Language::Table(0));
        set(Language::English);
        assert_eq!(choice(), Language::English);
    }

    #[test]
    fn chosen_language_is_kept_in_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings/language");
        assert_eq!(load_from(&path), None);
        for language in Language::all() {
            save_to(&path, language);
            assert_eq!(load_from(&path), Some(language));
        }
        std::fs::write(&path, "nl\n").unwrap();
        assert_eq!(load_from(&path), Some(Language::Table(0)));
        std::fs::write(&path, "klingon").unwrap();
        assert_eq!(load_from(&path), None);
        // The tests themselves never touch the settings of the user.
        assert_eq!(language_path(), None);
        assert_eq!(load(), Language::Auto);
    }

    #[test]
    fn auto_follows_the_locale_of_the_system() {
        assert_eq!(table_for_locale("nl-NL"), Some(0));
        assert_eq!(table_for_locale("nl_BE.UTF-8"), Some(0));
        assert_eq!(table_for_locale("NL"), Some(0));
        assert_eq!(table_for_locale("de-DE"), None);
        assert_eq!(table_for_locale("en-US"), None);
        assert_eq!(table_for_locale("nln"), None);
        assert_eq!(table_for_locale(""), None);
    }

    /// The `{name}` marks of a text, which `tr_args` fills in.
    fn placeholders(text: &str) -> BTreeSet<&str> {
        text.split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}'))
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn values_are_filled_into_a_translated_sentence() {
        let _language = TestLanguage::hold(Language::Table(0));
        let (page, pages) = (3, 12);
        let filled = tr_args(
            "Page {page} of {pages} · {buildings} buildings",
            &[("page", &page), ("pages", &pages), ("buildings", &140)],
        );
        assert_eq!(filled, "Pagina 3 van 12 · 140 gebouwen");
        set(Language::English);
        assert_eq!(
            tr_args("Page {page} · {buildings} buildings", &[("page", &page)]),
            "Page 3 · {buildings} buildings",
            "a name without a value stays visible"
        );
        // A translation that drops or renames a mark would show it unfilled.
        for (table, texts) in TABLES.iter().zip(tables()) {
            for (english, translated) in texts {
                assert_eq!(
                    placeholders(english),
                    placeholders(translated),
                    "{}: {english}",
                    table.code
                );
            }
        }
    }

    /// The texts of `literals` that a table has no entry for, with their file.
    fn missing_entries(
        literals: &[(&'static str, String)],
        texts: &HashMap<String, String>,
    ) -> BTreeSet<String> {
        literals
            .iter()
            .filter(|(_, text)| !texts.contains_key(text))
            .map(|(file, text)| format!("{file}: {text:?}"))
            .collect()
    }

    /// The entries of a table that are not among the texts in `used`.
    fn unused_entries<'a>(
        used: &BTreeSet<String>,
        texts: &'a HashMap<String, String>,
    ) -> BTreeSet<&'a String> {
        texts
            .keys()
            .filter(|english| !used.contains(*english))
            .collect()
    }

    #[test]
    fn every_translated_literal_has_an_entry_in_every_table() {
        let literals = translated_literals();
        assert!(literals.len() > 150, "only {} texts found", literals.len());
        for (table, texts) in TABLES.iter().zip(tables()) {
            let missing = missing_entries(&literals, texts);
            assert!(
                missing.is_empty(),
                "texts without an entry in assets/locales/{}.json:\n{}",
                table.code,
                missing.into_iter().collect::<Vec<_>>().join("\n")
            );
        }
    }

    #[test]
    fn no_table_entry_is_unused() {
        let used: BTreeSet<String> = translated_literals()
            .into_iter()
            .map(|(_, text)| text)
            .collect();
        for (table, texts) in TABLES.iter().zip(tables()) {
            let unused = unused_entries(&used, texts);
            assert!(
                unused.is_empty(),
                "entries of assets/locales/{}.json that no source asks for: {unused:?}",
                table.code
            );
        }
    }

    #[test]
    fn a_half_filled_or_stale_table_is_found() {
        // What the two tests above report for a table other than the first:
        // one text of the sources is missing and one entry is asked for by
        // no source.
        let literals = translated_literals();
        let used: BTreeSet<String> = literals.iter().map(|(_, text)| text.clone()).collect();
        let mut texts = tables()[0].clone();
        assert!(texts.remove("Settings").is_some());
        texts.insert("A text no source asks for".into(), "x".into());
        let missing = missing_entries(&literals, &texts);
        assert!(!missing.is_empty());
        assert!(
            missing.iter().all(|entry| entry.ends_with("\"Settings\"")),
            "{missing:?}"
        );
        let unused = unused_entries(&used, &texts);
        assert_eq!(
            unused.into_iter().collect::<Vec<_>>(),
            ["A text no source asks for"]
        );
    }
}
