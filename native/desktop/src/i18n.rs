//! The language of the user interface. The English text is the key: other
//! languages are tables from the English text, and a text without a
//! translation stays English.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

/// What the user chose in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// The language of the system when there is a table for it.
    Auto,
    English,
    Dutch,
}

impl Language {
    pub const ALL: [Self; 3] = [Self::Auto, Self::English, Self::Dutch];

    pub fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Dutch => "nl",
        }
    }

    pub fn from_key(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|language| language.key() == value)
    }

    fn code(self) -> u8 {
        match self {
            Self::Auto => 0,
            Self::English => 1,
            Self::Dutch => 2,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Auto => tr("Auto-detect"),
            Self::English => "English",
            Self::Dutch => "Nederlands",
        })
    }
}

static CHOICE: AtomicU8 = AtomicU8::new(1);
/// Whether texts are looked up in the Dutch table.
static DUTCH: AtomicU8 = AtomicU8::new(0);

fn dutch() -> &'static HashMap<String, String> {
    static TABLE: OnceLock<HashMap<String, String>> = OnceLock::new();
    TABLE.get_or_init(|| {
        serde_json::from_str(include_str!("../../assets/locales/nl.json")).unwrap_or_default()
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

#[cfg(not(windows))]
fn system_locale() -> String {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default()
}

/// Use a language from now on. Views show it the next time they are built.
pub fn set(language: Language) {
    let dutch = match language {
        Language::Auto => system_locale().to_ascii_lowercase().starts_with("nl"),
        Language::English => false,
        Language::Dutch => true,
    };
    CHOICE.store(language.code(), Ordering::Relaxed);
    DUTCH.store(u8::from(dutch), Ordering::Relaxed);
}

pub fn choice() -> Language {
    Language::ALL
        .into_iter()
        .find(|language| language.code() == CHOICE.load(Ordering::Relaxed))
        .unwrap_or(Language::English)
}

/// A text in the language in use.
pub fn tr(text: &str) -> &str {
    if DUTCH.load(Ordering::Relaxed) == 0 {
        return text;
    }
    dutch().get(text).map_or(text, String::as_str)
}

fn language_path() -> Option<PathBuf> {
    crate::preferences::config_directory().map(|directory| directory.join("language"))
}

/// The language chosen in an earlier session; the system's otherwise.
pub fn load() -> Language {
    language_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|value| Language::from_key(value.trim()))
        .unwrap_or(Language::Auto)
}

pub fn save(language: Language) {
    if let Some(path) = language_path() {
        if let Some(directory) = path.parent() {
            if std::fs::create_dir_all(directory).is_ok() {
                let _ = std::fs::write(path, language.key());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dutch_table_is_valid_and_keeps_untranslated_text_english() {
        assert!(dutch().len() > 100);
        assert_eq!(
            dutch().get("Settings").map(String::as_str),
            Some("Instellingen")
        );
        // No entry maps a text to nothing, and none repeats the English text
        // with only different spacing around it.
        for (english, translated) in dutch() {
            assert!(!translated.trim().is_empty(), "{english}");
            assert_eq!(translated.trim(), translated, "{english}");
        }
        for language in Language::ALL {
            assert_eq!(Language::from_key(language.key()), Some(language));
        }
        assert_eq!(Language::from_key("xx"), None);
    }
}
