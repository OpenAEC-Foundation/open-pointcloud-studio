//! The Settings dialog of the OpenAEC style book: the language, the theme and
//! what the application is. A choice shows at once; Cancel puts back what was
//! in use when the dialog opened and Save keeps it for later sessions.

use iced::widget::{
    button, center, column, container, horizontal_space, mouse_area, opaque, pick_list, row, text,
    Space,
};
use iced::{Border, Color, Element, Fill};

use crate::i18n::{self, tr, Language};
use crate::ui_theme::{self, UiTheme};
use crate::{flat_tool_style, opencad_ribbon, themed_pick_list_style, Message, Studio};

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
    Reset,
    Save,
    Cancel,
}

/// The dialog while it is open: its tab, and what Cancel goes back to.
#[derive(Debug, Clone, Copy)]
pub struct SettingsDialog {
    tab: SettingsTab,
    language: Language,
    theme: UiTheme,
}

impl Studio {
    pub(crate) fn settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::Open => {
                self.file_open = false;
                self.settings = Some(SettingsDialog {
                    tab: SettingsTab::General,
                    language: i18n::choice(),
                    theme: self.ui_theme,
                });
            }
            SettingsAction::Tab(tab) => {
                if let Some(dialog) = &mut self.settings {
                    dialog.tab = tab;
                }
            }
            SettingsAction::Language(language) => i18n::set(language),
            SettingsAction::Theme(theme) => self.preview_theme(theme),
            SettingsAction::Reset => {
                i18n::set(Language::Auto);
                self.preview_theme(UiTheme::Light);
            }
            SettingsAction::Save => {
                if self.settings.take().is_some() {
                    i18n::save(i18n::choice());
                    self.ui_theme.save();
                }
            }
            SettingsAction::Cancel => {
                if let Some(dialog) = self.settings.take() {
                    i18n::set(dialog.language);
                    self.preview_theme(dialog.theme);
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
        let dialog = self.settings?;
        let colors = self.ui_theme.colors();
        let send = |action| Message::Settings(action);

        let tab = |label: &'static str, tab: SettingsTab| {
            let active = dialog.tab == tab;
            button(text(tr(label)).size(12))
                .on_press(send(SettingsAction::Tab(tab)))
                .width(Fill)
                .padding([7, 12])
                .style(move |theme, status| {
                    let colors = ui_theme::colors(theme);
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: (active || hovered).then_some(
                            if active {
                                colors.panel_alt
                            } else {
                                colors.hover
                            }
                            .into(),
                        ),
                        text_color: if active { colors.accent } else { colors.text },
                        border: Border::default().rounded(4),
                        ..button::Style::default()
                    }
                })
        };
        let sidebar = column![
            tab("General", SettingsTab::General),
            tab("Appearance", SettingsTab::Appearance),
            tab("About", SettingsTab::About),
        ]
        .spacing(2)
        .width(150);

        let heading = |label: &'static str| text(tr(label)).size(13).color(colors.accent);
        let label = |name: &'static str| text(tr(name)).size(12).color(colors.muted).width(120);
        let content: Element<'_, Message> = match dialog.tab {
            SettingsTab::General => column![
                heading("Application"),
                row![
                    label("Language"),
                    pick_list(Language::ALL, Some(i18n::choice()), move |language| send(
                        SettingsAction::Language(language)
                    ))
                    .style(themed_pick_list_style)
                    .text_size(12)
                    .width(190),
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(14)
            .into(),
            SettingsTab::Appearance => {
                let mut themes = column![
                    heading("Theme"),
                    text(tr("Choose a color theme for the application."))
                        .size(11)
                        .color(colors.muted),
                ]
                .spacing(8);
                for theme in UiTheme::ALL {
                    let palette = theme.colors();
                    let swatch = |color: Color| {
                        container(Space::new(14, 14)).style(move |_| {
                            container::Style::default()
                                .background(color)
                                .border(Border {
                                    color: Color::from_rgba8(128, 128, 128, 0.6),
                                    width: 1.0,
                                    radius: 2.0.into(),
                                })
                        })
                    };
                    let chosen = theme == self.ui_theme;
                    themes = themes.push(
                        button(
                            row![
                                swatch(palette.shell),
                                swatch(palette.panel),
                                swatch(palette.accent),
                                swatch(palette.text),
                                text(tr(&theme.to_string()).to_owned()).size(12),
                            ]
                            .spacing(6)
                            .align_y(iced::Alignment::Center),
                        )
                        .on_press(send(SettingsAction::Theme(theme)))
                        .width(Fill)
                        .padding([6, 10])
                        .style(move |theme, status| {
                            let colors = ui_theme::colors(theme);
                            let hovered = matches!(status, button::Status::Hovered);
                            button::Style {
                                background: (chosen || hovered).then_some(
                                    if chosen {
                                        colors.panel_alt
                                    } else {
                                        colors.hover
                                    }
                                    .into(),
                                ),
                                text_color: colors.text,
                                border: Border {
                                    color: if chosen {
                                        colors.accent
                                    } else {
                                        Color::TRANSPARENT
                                    },
                                    width: 1.0,
                                    radius: 4.0.into(),
                                },
                                ..button::Style::default()
                            }
                        }),
                    );
                }
                themes.into()
            }
            SettingsTab::About => {
                let fact = |name: &'static str, value: &'static str| {
                    row![label(name), text(value).size(12)].spacing(12)
                };
                column![
                    text("Open Pointcloud Studio").size(16),
                    text(tr("Native viewer and editor for point clouds."))
                        .size(12)
                        .color(colors.muted),
                    fact("Version", env!("CARGO_PKG_VERSION")),
                    fact("Framework", "Rust · iced · wgpu"),
                    fact("License", "GPL-3.0-only · LGPL-3.0-or-later"),
                ]
                .spacing(10)
                .into()
            }
        };

        let footer = row![
            button(text(tr("Reset to Defaults")).size(12))
                .on_press(send(SettingsAction::Reset))
                .style(flat_tool_style)
                .padding([5, 12]),
            horizontal_space(),
            button(text(tr("Cancel")).size(12))
                .on_press(send(SettingsAction::Cancel))
                .style(flat_tool_style)
                .padding([5, 12]),
            button(text(tr("Save")).size(12))
                .on_press(send(SettingsAction::Save))
                .style(|theme, status| opencad_ribbon::file_tab_style(theme, false, status))
                .padding([5, 16]),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        let card = container(
            column![
                row![
                    text(tr("Settings")).size(15),
                    horizontal_space(),
                    button(text("×").size(14))
                        .on_press(send(SettingsAction::Cancel))
                        .style(flat_tool_style)
                        .padding([1, 8]),
                ]
                .align_y(iced::Alignment::Center),
                row![
                    sidebar,
                    container(content).padding([4, 18]).width(Fill).height(Fill)
                ]
                .height(Fill),
                footer,
            ]
            .spacing(14),
        )
        .width(640)
        .height(380)
        .padding(18)
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.panel)
                .color(colors.text)
                .border(Border {
                    color: colors.border,
                    width: 1.0,
                    radius: 8.0.into(),
                })
        });

        Some(opaque(
            mouse_area(center(opaque(card)).style(|_| {
                container::Style::default().background(Color::from_rgba8(0, 0, 0, 0.55))
            }))
            .on_press(send(SettingsAction::Cancel)),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_puts_the_theme_back_and_save_keeps_it() {
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
}
