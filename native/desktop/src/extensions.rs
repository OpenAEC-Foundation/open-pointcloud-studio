//! The optional features of the application that can be switched off: which
//! there are, which the user switched off, the page of the File view that
//! lists them and the commands of the local API for them. Every extension is
//! part of the application; no code from another source is loaded.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use iced::widget::{checkbox, column, container, horizontal_space, row, text};
use iced::{Border, Element, Fill, Font};
use serde_json::{json, Value};

use crate::file_view::FilePage;
use crate::i18n::{key, tr};
use crate::{muted_checkbox_style, ui_theme, Message, Studio, VERSION_LABEL};

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

/// Every extension, in the order the page lists them. A further one is an
/// entry here and a check of `Extensions::enabled` where it is offered.
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

/// Which extensions the user switched off. Everything else is on, so an
/// extension that a later version adds starts enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extensions {
    disabled: BTreeSet<String>,
}

fn find(id: &str) -> Option<&'static Extension> {
    BUILT_IN.iter().find(|extension| extension.id == id)
}

/// The ids of the extensions, for the command that switches one.
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

impl Extensions {
    /// The choices of an earlier session; everything enabled without them.
    pub fn load() -> Self {
        settings_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// A missing, oversized or damaged file switches nothing off.
    fn load_from(path: &Path) -> Self {
        let small = fs::metadata(path).is_ok_and(|metadata| metadata.len() <= MAX_FILE_BYTES);
        let disabled = small
            .then(|| fs::read(path).ok())
            .flatten()
            .and_then(|bytes| serde_json::from_slice::<BTreeSet<String>>(&bytes).ok())
            .unwrap_or_default();
        Self { disabled }
    }

    pub fn save(&self) -> io::Result<()> {
        if cfg!(test) {
            return Ok(());
        }
        let path = settings_path().ok_or_else(|| io::Error::other("no user config directory"))?;
        self.save_to(&path)
    }

    /// The ids that are switched off as a JSON array, replaced in one step so
    /// a crash never leaves half a file.
    fn save_to(&self, path: &Path) -> io::Result<()> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("settings path has no parent"))?;
        fs::create_dir_all(directory)?;
        let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
        serde_json::to_writer_pretty(temporary.as_file_mut(), &self.disabled)
            .map_err(io::Error::other)?;
        temporary.as_file_mut().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        Ok(())
    }

    pub fn enabled(&self, id: &str) -> bool {
        !self.disabled.contains(id)
    }

    /// Switch an extension on or off. An id that is no extension is refused;
    /// ids a newer version stored are kept as they are.
    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), String> {
        let extension = find(id).ok_or_else(|| format!("unknown extension {id}"))?;
        if enabled {
            self.disabled.remove(extension.id);
        } else {
            self.disabled.insert(extension.id.to_owned());
        }
        Ok(())
    }

    /// The extensions as the local API lists them, with their English texts.
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
        let unsaved = self.extensions.save().err().map(|error| error.to_string());
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
        let name = find(id).map_or(id, |extension| extension.name);
        self.status = match &unsaved {
            None if enabled => format!("{name} switched on"),
            None => format!("{name} switched off"),
            Some(error) => format!("Could not save the extension settings: {error}"),
        };
        if id == BAG3D && !enabled {
            self.leave_bag3d();
        }
        unsaved
    }

    pub(crate) fn api_set_extension_enabled(&mut self, id: &str, enabled: bool) -> Value {
        match self.set_extension_enabled(id, enabled) {
            Ok(unsaved) => switched_answer(id, self.extensions.enabled(id), unsaved),
            Err(error) => json!({"ok": false, "error": error}),
        }
    }

    /// The Extensions page of the File view: a card for every extension with
    /// its switch.
    pub(crate) fn extensions_page(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let mut cards = column![].spacing(10).width(Fill);
        for extension in BUILT_IN {
            let enabled = self.extensions.enabled(extension.id);
            let mut origin = row![text(extension.author).size(11).color(colors.muted)].spacing(14);
            if extension.uses_network {
                origin = origin.push(text(tr("Uses the internet")).size(11).color(colors.muted));
            }
            let card = column![
                row![
                    text(tr(extension.name)).size(15),
                    container(text(tr(extension.category)).size(10))
                        .padding([2, 8])
                        .style(|theme| {
                            let colors = ui_theme::colors(theme);
                            container::Style::default()
                                .background(colors.panel_alt)
                                .color(colors.accent)
                                .border(Border {
                                    color: colors.border,
                                    width: 1.0,
                                    radius: 9.0.into(),
                                })
                        }),
                    horizontal_space(),
                    checkbox(tr("Enabled"), enabled)
                        .on_toggle(move |enabled| Message::ExtensionEnabled(extension.id, enabled))
                        .style(muted_checkbox_style)
                        .text_size(12)
                        .size(15),
                ]
                .spacing(10)
                .align_y(iced::Alignment::Center),
                text(format!("{VERSION_LABEL} · {}", tr("built in")))
                    .size(11)
                    .color(colors.muted),
                // A switched-off extension reads as set aside.
                text(tr(extension.description)).size(12).color(if enabled {
                    colors.text
                } else {
                    colors.muted
                }),
                origin,
            ]
            .spacing(6);
            cards = cards.push(container(card).padding(14).width(Fill).style(|theme| {
                let colors = ui_theme::colors(theme);
                container::Style::default()
                    .background(colors.panel)
                    .border(Border {
                        color: colors.border,
                        width: 1.0,
                        radius: 6.0.into(),
                    })
            }));
        }
        column![
            text(tr(FilePage::Extensions.label()))
                .size(26)
                .font(Font::with_name("Space Grotesk")),
            container(text(tr("INSTALLED")).size(10).color(colors.muted)).padding(iced::Padding {
                top: 14.0,
                bottom: 4.0,
                ..iced::Padding::ZERO
            }),
            container(cards).width(Fill).max_width(560),
            container(
                text(tr(
                    "These features are part of the application. No code from other sources is \
                     loaded, and extensions from other sources cannot be installed yet."
                ))
                .size(12)
                .color(colors.muted),
            )
            .max_width(560)
            .padding(iced::Padding {
                top: 14.0,
                ..iced::Padding::ZERO
            }),
        ]
        .spacing(8)
        .width(Fill)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Language, TestLanguage};
    use crate::native_api::{ApiCommand, ApiRequest};

    fn send(studio: &mut Studio, command: ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
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
            json!(["bag3d"])
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
            json!(["later"])
        );
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

        let _ = studio.update(Message::ExtensionEnabled(BAG3D, false));
        assert!(!studio.extensions.enabled(BAG3D));
        assert!(studio.file_open, "the page stays open");
        let _ = studio.view();
        let _ = studio.update(Message::ExtensionEnabled(BAG3D, true));
        assert!(studio.extensions.enabled(BAG3D));
    }
}
