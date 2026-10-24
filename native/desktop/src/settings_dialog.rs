//! The Settings dialog of the OpenAEC style book: the language, the theme and
//! what the application is. A choice shows at once; Cancel puts back what was
//! in use when the dialog opened and Save keeps it for later sessions.

use iced::widget::{button, column, horizontal_space, row, stack, text};
use iced::{Element, Fill};

use crate::i18n::{self, tr, Language};
use crate::ui_style;
use crate::ui_theme::{UiColors, UiTheme};
use crate::{Message, Studio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Appearance,
    About,
}

#[derive(Debug, Clone, Copy)]
pub enum SettingsAction {
    Open,
    Tab(SettingsTab),
    Language(Language),
    Theme(UiTheme),
    ThemeMenu(bool),
    ResetPrompt(bool),
    Reset,
    Save,
    Cancel,
    Escape,
}

/// The dialog while it is open: its tab, and what Cancel goes back to.
#[derive(Debug, Clone)]
pub struct SettingsDialog {
    tab: SettingsTab,
    language: Language,
    theme: UiTheme,
    theme_menu_open: bool,
    reset_confirm: bool,
    cad_viewer: String,
}

/// What the application is: its name and version, what it is built with and
/// on, its licences and where its source code is. The About tab of the dialog
/// and the About page of the File view show the same.
pub(crate) fn about<'a>(colors: UiColors) -> iced::widget::Column<'a, Message> {
    let fact = |name: &'static str, value: &'static str| {
        row![
            text(tr(name))
                .size(12)
                .color(colors.text_secondary)
                .width(120),
            text(value).size(12)
        ]
        .spacing(12)
    };
    column![
        text(crate::APP_NAME)
            .size(20)
            .font(crate::fonts::SEMIBOLD)
            .color(colors.accent),
        text(tr("Native viewer and editor for point clouds."))
            .size(12)
            .color(colors.text_secondary),
        fact("Version", env!("CARGO_PKG_VERSION")),
        fact("Framework", "Rust · iced · wgpu"),
        fact("License", "GPL-3.0-only · LGPL-3.0-or-later"),
        text(tr("Built on the OpenAEC platform"))
            .size(12)
            .color(colors.text_secondary),
        button(text(tr("Source code ↗")).size(12))
            .on_press(Message::OpenUrl(crate::SOURCE_URL))
            .style(ui_style::link)
            .padding([4, 8]),
    ]
    .spacing(10)
}

impl Studio {
    /// Use a language chosen outside the dialog and keep it for later
    /// sessions. An open dialog then no longer goes back to the language it
    /// opened with.
    pub(crate) fn choose_language(&mut self, language: Language) {
        i18n::set(language);
        i18n::save(language);
        if let Some(dialog) = &mut self.settings {
            dialog.language = language;
        }
    }

    pub(crate) fn settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::Open => {
                self.file_open = false;
                // The shortcut also arrives while the dialog is open, which
                // must not forget what Cancel goes back to.
                if self.settings.is_none() {
                    self.settings = Some(SettingsDialog {
                        tab: SettingsTab::General,
                        language: i18n::choice(),
                        theme: self.ui_theme,
                        theme_menu_open: false,
                        reset_confirm: false,
                        cad_viewer: self.cad_viewer.input.clone(),
                    });
                }
            }
            SettingsAction::Tab(tab) => {
                if let Some(dialog) = &mut self.settings {
                    dialog.tab = tab;
                    dialog.theme_menu_open = false;
                    dialog.reset_confirm = false;
                }
            }
            SettingsAction::ThemeMenu(open) => {
                if let Some(dialog) = &mut self.settings {
                    dialog.theme_menu_open = open;
                }
            }
            SettingsAction::ResetPrompt(open) => {
                if let Some(dialog) = &mut self.settings {
                    dialog.reset_confirm = open;
                    if open {
                        dialog.theme_menu_open = false;
                    }
                }
            }
            SettingsAction::Language(language) => i18n::set(language),
            SettingsAction::Theme(theme) => {
                self.preview_theme(theme);
                if let Some(dialog) = &mut self.settings {
                    dialog.theme_menu_open = false;
                }
            }
            SettingsAction::Reset => {
                if self
                    .settings
                    .as_ref()
                    .is_some_and(|dialog| dialog.reset_confirm)
                {
                    i18n::set(Language::Auto);
                    self.preview_theme(UiTheme::Light);
                    self.cad_viewer.input.clear();
                    self.cad_viewer.refresh();
                    if let Some(dialog) = &mut self.settings {
                        dialog.reset_confirm = false;
                    }
                }
            }
            SettingsAction::Save => {
                if self.settings.take().is_some() {
                    i18n::save(i18n::choice());
                    self.ui_theme.save();
                    if let Err(error) = crate::preferences::save(&self.preferences()) {
                        self.status = format!("Could not save settings: {error}");
                    }
                }
            }
            SettingsAction::Cancel => {
                if let Some(dialog) = self.settings.take() {
                    i18n::set(dialog.language);
                    self.preview_theme(dialog.theme);
                    self.cad_viewer.input = dialog.cad_viewer;
                    self.cad_viewer.refresh();
                }
            }
            SettingsAction::Escape => {
                if self
                    .settings
                    .as_ref()
                    .is_some_and(|dialog| dialog.reset_confirm)
                {
                    self.settings_action(SettingsAction::ResetPrompt(false));
                } else {
                    self.settings_action(SettingsAction::Cancel);
                }
            }
        }
    }

    fn preview_theme(&mut self, theme: UiTheme) {
        self.ui_theme = theme;
        let _ = self.sync_window_chrome();
    }

    /// The dialog over the dimmed window, while it is open.
    pub(crate) fn settings_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.settings.as_ref()?;
        let colors = self.ui_theme.colors();
        let send = |action| Message::Settings(action);

        let tab = |label: &'static str, tab: SettingsTab| {
            let active = dialog.tab == tab;
            openaec_ui::widgets::settings_tab(
                tr(label),
                active,
                send(SettingsAction::Tab(tab)),
                self.ui_theme.shared(),
            )
        };
        let sidebar = column![
            tab("General", SettingsTab::General),
            tab("Appearance", SettingsTab::Appearance),
            tab("About", SettingsTab::About),
        ]
        .spacing(2)
        .width(Fill);

        let heading = |label: &'static str| {
            openaec_ui::widgets::settings_section_heading(tr(label), self.ui_theme.shared())
        };
        let label = |name: &'static str| {
            text(tr(name))
                .size(12)
                .color(colors.dialog_content_secondary)
                .width(120)
        };
        let content: Element<'_, Message> = match dialog.tab {
            SettingsTab::General => column![
                heading("Application"),
                row![
                    label("Language"),
                    ui_style::pick_list(Language::all(), Some(i18n::choice()), move |language| send(
                        SettingsAction::Language(language)
                    ))
                    .width(190),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
                heading("CAD viewer"),
                text(tr("The program that opens exported DXF and DWG drawings when the Open CAD Studio that comes with the application is missing. Leave it empty to use Open CAD Studio where it is installed."))
                    .size(11)
                    .color(colors.dialog_content_secondary),
                self.cad_viewer_setting(),
            ]
            .spacing(14)
            .into(),
            SettingsTab::Appearance => {
                column![
                    heading("Theme"),
                    row![
                        horizontal_space(),
                        openaec_ui::widgets::settings_theme_picker(
                            UiTheme::ALL.into_iter().map(|theme| (
                                theme.shared(),
                                tr(&theme.to_string()).to_owned(),
                                send(SettingsAction::Theme(theme)),
                            )).collect(),
                            self.ui_theme.shared(),
                            self.ui_theme.shared(),
                            dialog.theme_menu_open,
                            send(SettingsAction::ThemeMenu(!dialog.theme_menu_open)),
                        ),
                    ],
                ]
                .spacing(8)
                .into()
            }
            SettingsTab::About => about(colors).into(),
        };

        let footer = row![
            ui_style::secondary_button(tr("Reset to Defaults"))
                .on_press(send(SettingsAction::ResetPrompt(true))),
            horizontal_space(),
            ui_style::secondary_button(tr("Cancel")).on_press(send(SettingsAction::Cancel)),
            ui_style::primary_button(tr("Save")).on_press(send(SettingsAction::Save)),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let modal = openaec_ui::widgets::settings_modal(
            tr("Settings"),
            sidebar.into(),
            content,
            footer.into(),
            send(SettingsAction::Cancel),
            self.ui_theme.shared(),
        );
        if dialog.reset_confirm {
            Some(
                stack![
                    modal,
                    openaec_ui::widgets::settings_reset_confirmation(
                        tr("Reset to Defaults"),
                        tr("Are you sure you want to reset all settings to their default values?"),
                        tr("Cancel"),
                        tr("Reset to Defaults"),
                        send(SettingsAction::ResetPrompt(false)),
                        send(SettingsAction::Reset),
                        self.ui_theme.shared(),
                    ),
                ]
                .width(Fill)
                .height(Fill)
                .into(),
            )
        } else {
            Some(modal)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_puts_the_theme_back_and_save_keeps_it() {
        // Cancel also puts the language back, which other tests set.
        let _language = i18n::TestLanguage::hold(Language::English);
        let mut studio = Studio::default();
        let before = studio.ui_theme;
        let other = UiTheme::ALL
            .into_iter()
            .find(|theme| *theme != before)
            .unwrap();
        studio.settings_action(SettingsAction::Open);
        assert!(studio.settings_view().is_some());
        studio.settings_action(SettingsAction::Tab(SettingsTab::Appearance));
        studio.settings_action(SettingsAction::Theme(other));
        assert_eq!(studio.ui_theme, other);
        assert!(studio.settings_view().is_some());
        // Asking for the open dialog again keeps what Cancel goes back to.
        studio.settings_action(SettingsAction::Open);
        studio.settings_action(SettingsAction::Cancel);
        assert_eq!(studio.ui_theme, before);
        assert!(studio.settings.is_none() && studio.settings_view().is_none());

        // Opening from the File view closes that view.
        studio.file_open = true;
        studio.settings_action(SettingsAction::Open);
        assert!(!studio.file_open);
        studio.settings_action(SettingsAction::Tab(SettingsTab::About));
        assert!(studio.settings_view().is_some());
        studio.settings_action(SettingsAction::Cancel);
    }

    #[test]
    fn language_chosen_outside_the_dialog_survives_its_cancel() {
        let _language = i18n::TestLanguage::hold(Language::English);
        let dutch = Language::from_key("nl").unwrap();
        let mut studio = Studio::default();
        studio.settings_action(SettingsAction::Open);
        // Inside the dialog a choice is a preview that Cancel takes back.
        studio.settings_action(SettingsAction::Language(dutch));
        assert_eq!(tr("Settings"), "Instellingen");
        assert!(studio.settings_view().is_some());
        studio.settings_action(SettingsAction::Cancel);
        assert_eq!(i18n::choice(), Language::English);

        studio.settings_action(SettingsAction::Open);
        studio.choose_language(dutch);
        studio.settings_action(SettingsAction::Cancel);
        assert_eq!(i18n::choice(), dutch);
        assert_eq!(tr("Cancel"), "Annuleren");
    }

    #[test]
    fn theme_menu_opens_selects_and_closes_without_committing() {
        let mut studio = Studio::default();
        let original = studio.ui_theme;
        let next = UiTheme::ALL
            .into_iter()
            .find(|theme| *theme != original)
            .unwrap();
        studio.settings_action(SettingsAction::Open);
        studio.settings_action(SettingsAction::Tab(SettingsTab::Appearance));
        studio.settings_action(SettingsAction::ThemeMenu(true));
        assert!(studio.settings.as_ref().unwrap().theme_menu_open);
        studio.settings_action(SettingsAction::Theme(next));
        assert_eq!(studio.ui_theme, next);
        assert!(!studio.settings.as_ref().unwrap().theme_menu_open);
        studio.settings_action(SettingsAction::Cancel);
        assert_eq!(studio.ui_theme, original);
    }

    #[test]
    fn reset_confirmation_stays_above_settings_and_cancel_restores_preview() {
        let mut studio = Studio {
            ui_theme: UiTheme::Forge,
            ..Studio::default()
        };
        studio.settings_action(SettingsAction::Open);
        studio.settings_action(SettingsAction::Reset);
        assert_eq!(studio.ui_theme, UiTheme::Forge);
        studio.settings_action(SettingsAction::ThemeMenu(true));
        studio.settings_action(SettingsAction::ResetPrompt(true));
        assert!(studio.settings.as_ref().unwrap().reset_confirm);
        assert!(!studio.settings.as_ref().unwrap().theme_menu_open);
        assert!(studio.settings_view().is_some());
        studio.settings_action(SettingsAction::Escape);
        assert!(studio.settings.is_some());
        assert!(!studio.settings.as_ref().unwrap().reset_confirm);
        studio.settings_action(SettingsAction::ResetPrompt(true));
        studio.settings_action(SettingsAction::Reset);
        assert_eq!(studio.ui_theme, UiTheme::Light);
        studio.settings_action(SettingsAction::Cancel);
        assert_eq!(studio.ui_theme, UiTheme::Forge);
    }
}
