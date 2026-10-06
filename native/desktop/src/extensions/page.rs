//! What extensions look like in the window: the Extensions page of the File
//! view, the dialogs that confirm an install and an uninstall, the
//! EXTENSIONS group of the ribbon, the runs in the status bar and the tiles
//! extensions add to the New and Export pages.

use iced::widget::{
    button, center, checkbox, column, container, horizontal_space, mouse_area, opaque,
    progress_bar, row, svg, text, tooltip, Column, Space,
};
use iced::{Border, Color, Element, Fill, Font, Length};

use super::manifest::{Commands, EntryPage, Manifest};
use super::{Dialog, ExtensionAction, Installed, BUILT_IN};
use crate::file_view::FilePage;
use crate::i18n::{tr, tr_args};
use crate::{
    flat_tool_style, muted_checkbox_style, opencad_ribbon, ui_theme, Message, Studio, VERSION_LABEL,
};

/// The widest the cards of the page grow.
const CARD_W: f32 = 620.0;

fn send(action: ExtensionAction) -> Message {
    Message::Extension(action)
}

/// A card of the page: a panel with a thin border.
fn card<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(content)
        .padding(14)
        .width(Fill)
        .style(|theme| {
            let colors = ui_theme::colors(theme);
            container::Style::default()
                .background(colors.panel)
                .border(Border {
                    color: colors.border,
                    width: 1.0,
                    radius: 6.0.into(),
                })
        })
        .into()
}

/// A small mark beside the name of an extension.
fn chip<'a>(label: String) -> Element<'a, Message> {
    container(text(label).size(10))
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
        })
        .into()
}

/// A small button of a card.
fn small_button<'a>(label: &str, message: Option<Message>) -> Element<'a, Message> {
    button(text(label.to_owned()).size(12))
        .on_press_maybe(message)
        .style(|theme, status| {
            let mut style = opencad_ribbon::tool_btn_style(theme, false, status);
            style.border.color = ui_theme::colors(theme).border;
            style
        })
        .padding([4, 10])
        .into()
}

/// What an extension says it uses, in a short list for a card and the
/// confirmation.
fn uses_lines(manifest: &Manifest) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(if manifest.uses.network {
        tr("Uses the internet").to_owned()
    } else {
        tr("Does not use the internet").to_owned()
    });
    lines.push(if manifest.uses.files_outside_folder {
        tr("Reads or writes files outside its folder").to_owned()
    } else {
        tr("Keeps to its own folder").to_owned()
    });
    lines.push(match &manifest.uses.commands {
        Commands::All => tr("May send every command of the local API").to_owned(),
        Commands::Listed(names) => tr_args(
            "Commands of the local API: {commands}",
            &[(
                "commands",
                &names.iter().cloned().collect::<Vec<_>>().join(", "),
            )],
        ),
    });
    lines
}

/// What an extension adds to the window, in one line.
fn adds_line(manifest: &Manifest) -> Option<String> {
    let mut parts = Vec::new();
    match manifest.ribbon.len() {
        0 => {}
        1 => parts.push(tr("1 button in the ribbon").to_owned()),
        count => parts.push(tr_args(
            "{count} buttons in the ribbon",
            &[("count", &count)],
        )),
    }
    for page in [EntryPage::New, EntryPage::Export] {
        if manifest.file_view.iter().any(|tile| tile.page == page) {
            parts.push(match page {
                EntryPage::New => tr("a tile on the New page").to_owned(),
                EntryPage::Export => tr("a tile on the Export page").to_owned(),
            });
        }
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

impl Studio {
    /// The Extensions page of the File view: Install extension…, a card for
    /// every built-in and installed extension, and what extensions are.
    pub(crate) fn extensions_page(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let host = &self.extension_host;
        let caption = |label: &str| {
            container(text(label.to_owned()).size(10).color(colors.muted)).padding(iced::Padding {
                top: 18.0,
                bottom: 4.0,
                ..iced::Padding::ZERO
            })
        };

        let install_label = if host.preparing {
            tr("Checking the extension…")
        } else {
            tr("Install extension…")
        };
        let mut install_row = row![button(text(install_label).size(13))
            .on_press_maybe(
                (host.root.is_some() && !host.preparing && host.dialog.is_none())
                    .then_some(send(ExtensionAction::Install)),
            )
            .style(opencad_ribbon::primary_btn_style)
            .padding([7, 16])]
        .spacing(12)
        .align_y(iced::Alignment::Center);
        if host.root.is_none() {
            install_row = install_row.push(
                text(tr("There is no settings folder to install extensions in."))
                    .size(12)
                    .color(colors.muted),
            );
        }
        let mut top = column![
            text(tr(FilePage::Extensions.label()))
                .size(26)
                .font(Font::with_name("Space Grotesk")),
            text(tr(
                "Features you can switch off, and programs that add buttons and work through the local API of the application."
            ))
            .size(13)
            .color(colors.muted),
            Space::with_height(10),
            install_row,
        ]
        .spacing(6)
        .width(Fill);
        if let Some(error) = &host.last_error {
            top = top.push(
                text(tr_args(
                    "Could not install the extension: {error}",
                    &[("error", error)],
                ))
                .size(12)
                .color(colors.accent),
            );
        }

        let built_in =
            BUILT_IN
                .iter()
                .fold(column![].spacing(10).width(Fill), |cards, extension| {
                    let enabled = self.extensions.enabled(extension.id);
                    let mut origin =
                        row![text(extension.author).size(11).color(colors.muted)].spacing(14);
                    if extension.uses_network {
                        origin =
                            origin.push(text(tr("Uses the internet")).size(11).color(colors.muted));
                    }
                    cards.push(card(
                        column![
                            row![
                                text(tr(extension.name)).size(15),
                                chip(tr(extension.category).to_owned()),
                                horizontal_space(),
                                checkbox(tr("Enabled"), enabled)
                                    .on_toggle(move |enabled| Message::ExtensionEnabled(
                                        extension.id,
                                        enabled
                                    ))
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
                        .spacing(6),
                    ))
                });

        let mut installed = column![].spacing(10).width(Fill);
        for each in &host.installed {
            installed = installed.push(self.installed_card(each));
        }
        for problem in &host.problems {
            installed = installed.push(card(
                column![
                    row![
                        text(problem.id.clone()).size(15),
                        chip(tr("Not loaded").to_owned()),
                    ]
                    .spacing(10)
                    .align_y(iced::Alignment::Center),
                    text(tr_args(
                        "Could not be read: {error}",
                        &[("error", &problem.error)]
                    ))
                    .size(12)
                    .color(colors.accent),
                    row![
                        small_button(
                            tr("Show folder"),
                            Some(send(ExtensionAction::ShowFolder(problem.id.clone())))
                        ),
                        small_button(
                            tr("Uninstall…"),
                            Some(send(ExtensionAction::Uninstall(problem.id.clone())))
                        ),
                    ]
                    .spacing(8),
                ]
                .spacing(6),
            ));
        }
        if host.installed.is_empty() && host.problems.is_empty() {
            installed = installed.push(
                text(tr(
                    "No extensions are installed. An extension is a folder with an extension.json, or a .zip archive of one."
                ))
                .size(12)
                .color(colors.muted),
            );
        }

        column![
            top,
            caption(tr("BUILT IN")),
            container(built_in).width(Fill).max_width(CARD_W),
            caption(tr("INSTALLED")),
            container(installed).width(Fill).max_width(CARD_W),
            container(
                text(tr(
                    "An installed extension is a program that runs with your rights when you start it. Install only extensions whose author you trust. Each run writes a log in the logs folder of the extension."
                ))
                .size(12)
                .color(colors.muted),
            )
            .max_width(CARD_W)
            .padding(iced::Padding {
                top: 14.0,
                ..iced::Padding::ZERO
            }),
        ]
        .spacing(8)
        .width(Fill)
        .into()
    }

    /// The card of an installed extension: what it is, what it runs and
    /// uses, its switch, and Run or Stop, Show folder and Uninstall.
    fn installed_card<'a>(&'a self, installed: &'a Installed) -> Element<'a, Message> {
        let colors = self.ui_theme.colors();
        let manifest = &installed.manifest;
        let id = manifest.id.clone();
        let enabled = self.extensions.enabled(&id);
        let run = self.extension_host.runs.get(&id);
        let mut details = column![
            row![
                text(manifest.name.get().to_owned()).size(15),
                chip(tr("Installed").to_owned()),
                horizontal_space(),
                checkbox(tr("Enabled"), enabled)
                    .on_toggle({
                        let id = id.clone();
                        move |enabled| send(ExtensionAction::SetEnabled(id.clone(), enabled))
                    })
                    .style(muted_checkbox_style)
                    .text_size(12)
                    .size(15),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center),
            text(format!(
                "v{} · {} · {}",
                manifest.version, manifest.author, manifest.id
            ))
            .size(11)
            .color(colors.muted),
            text(manifest.description.get().to_owned())
                .size(12)
                .color(if enabled { colors.text } else { colors.muted }),
            text(tr_args(
                "Runs: {command}",
                &[("command", &manifest.launch.describe())]
            ))
            .size(11)
            .color(colors.muted),
        ]
        .spacing(6);
        for line in uses_lines(manifest) {
            details = details.push(text(line).size(11).color(colors.muted));
        }
        if let Some(adds) = adds_line(manifest) {
            details = details.push(
                text(tr_args("Adds {what}", &[("what", &adds)]))
                    .size(11)
                    .color(colors.muted),
            );
        }
        if let Some(run) = run {
            let seconds = run.started.elapsed().as_secs();
            let mut line = tr_args("Runs for {seconds} s", &[("seconds", &seconds)]);
            if let Some((percent, progress)) = &run.progress {
                line = format!("{line} · {percent:.0}% {progress}");
            }
            details = details.push(text(line).size(12).color(colors.accent));
        }
        let mut actions = row![].spacing(8);
        actions = actions.push(match run {
            Some(run) => small_button(
                tr("Stop"),
                (!run.control.stopping()).then(|| send(ExtensionAction::Stop(id.clone()))),
            ),
            None => small_button(
                tr("Run"),
                enabled.then(|| send(ExtensionAction::Press(id.clone(), None))),
            ),
        });
        actions = actions.push(small_button(
            tr("Show folder"),
            Some(send(ExtensionAction::ShowFolder(id.clone()))),
        ));
        if manifest.homepage.is_some() {
            actions = actions.push(small_button(
                tr("Homepage ↗"),
                Some(send(ExtensionAction::OpenHomepage(id.clone()))),
            ));
        }
        actions = actions.push(horizontal_space());
        actions = actions.push(small_button(
            tr("Uninstall…"),
            Some(send(ExtensionAction::Uninstall(id))),
        ));
        card(details.push(Space::with_height(2)).push(actions))
    }

    /// The dialog over the window that confirms an install or an
    /// uninstall, while one is open.
    pub(crate) fn extension_dialog_view(&self) -> Option<Element<'_, Message>> {
        let dialog = self.extension_host.dialog.as_ref()?;
        let colors = self.ui_theme.colors();
        let label = |name: &str| {
            text(name.to_owned())
                .size(12)
                .color(colors.muted)
                .width(150)
        };
        let (title, body, confirm): (&str, Element<'_, Message>, &str) = match dialog {
            Dialog::Install { staged, installed } => {
                let manifest = &staged.manifest;
                let version = match installed {
                    Some(old) if *old == manifest.version => {
                        tr_args("{version}, installed again", &[("version", old)])
                    }
                    Some(old) => tr_args(
                        "{version}, replaces {old}",
                        &[("version", &manifest.version), ("old", old)],
                    ),
                    None => manifest.version.clone(),
                };
                let mut facts: Vec<Element<'_, Message>> = Vec::new();
                let mut fact = |name: &str, lines: Vec<String>| {
                    let values = lines
                        .into_iter()
                        .fold(column![].spacing(2), |values, line| {
                            values.push(text(line).size(12))
                        });
                    facts.push(row![label(name), values.width(Fill)].spacing(12).into());
                };
                fact(tr("Author"), vec![manifest.author.clone()]);
                fact(tr("Version"), vec![version]);
                fact(tr("Id"), vec![manifest.id.clone()]);
                if let Some(homepage) = &manifest.homepage {
                    fact(tr("Homepage"), vec![homepage.clone()]);
                }
                fact(tr("Starts"), vec![manifest.launch.describe()]);
                fact(tr("Uses"), uses_lines(manifest));
                if let Some(adds) = adds_line(manifest) {
                    fact(tr("Adds"), vec![adds]);
                }
                fact(
                    tr("Size"),
                    vec![tr_args(
                        "{files} files, {kib} KiB",
                        &[
                            ("files", &staged.files),
                            ("kib", &staged.bytes.div_ceil(1024)),
                        ],
                    )],
                );
                fact(tr("From"), vec![staged.source.display().to_string()]);
                let body = column![
                    text(manifest.name.get().to_owned())
                        .size(20)
                        .font(Font::with_name("Space Grotesk")),
                    text(manifest.description.get().to_owned()).size(12),
                    container(Space::new(Fill, 1)).style(|theme| {
                        container::Style::default().background(ui_theme::colors(theme).border)
                    }),
                    Column::with_children(facts).spacing(8),
                    container(Space::new(Fill, 1)).style(|theme| {
                        container::Style::default().background(ui_theme::colors(theme).border)
                    }),
                    text(tr(
                        "An extension is a program. It runs with your rights when you start it, and can read and change your files. Install it only when you trust its author."
                    ))
                    .size(12)
                    .color(colors.accent),
                ]
                .spacing(10)
                .into();
                (tr("Install extension?"), body, tr("Install"))
            }
            Dialog::Uninstall(id) => {
                let name = self.extension_host.name_of(id);
                let body = text(tr_args(
                    "{name} is removed with its folder and the logs of its runs. A run under way is stopped first.",
                    &[("name", &name)],
                ))
                .size(13)
                .into();
                (tr("Uninstall extension?"), body, tr("Uninstall"))
            }
        };
        let confirm_action = match dialog {
            Dialog::Install { .. } => ExtensionAction::ConfirmInstall,
            Dialog::Uninstall(_) => ExtensionAction::ConfirmUninstall,
        };
        let card = container(
            column![
                row![
                    text(title.to_owned()).size(15),
                    horizontal_space(),
                    button(text("×").size(14))
                        .on_press(send(ExtensionAction::CloseDialog))
                        .style(flat_tool_style)
                        .padding([1, 8]),
                ]
                .align_y(iced::Alignment::Center),
                body,
                row![
                    horizontal_space(),
                    button(text(tr("Cancel")).size(12))
                        .on_press(send(ExtensionAction::CloseDialog))
                        .style(flat_tool_style)
                        .padding([5, 12]),
                    button(text(confirm.to_owned()).size(12))
                        .on_press(send(confirm_action))
                        .style(|theme, status| opencad_ribbon::file_tab_style(theme, false, status))
                        .padding([5, 16]),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(16),
        )
        .width(560)
        .padding(20)
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
            .on_press(send(ExtensionAction::CloseDialog)),
        ))
    }

    /// The EXTENSIONS group of the ribbon: the buttons of the enabled
    /// installed extensions, highlighted while their extension runs. A
    /// click on one of them then stops the run.
    pub(crate) fn extensions_ribbon(&self) -> Option<Element<'_, Message>> {
        let mut items = Vec::new();
        for installed in &self.extension_host.installed {
            let manifest = &installed.manifest;
            if !self.extensions.enabled(&manifest.id) {
                continue;
            }
            let running = self.extension_host.runs.contains_key(&manifest.id);
            for each in &manifest.ribbon {
                let icon: Element<'_, Message> = match installed.icons.get(&each.id) {
                    Some(handle) => svg(handle.clone()).width(32).height(32).into(),
                    None => Space::new(32, 32).into(),
                };
                let control = button(
                    column![
                        icon,
                        Space::with_height(4),
                        text(each.label.get().to_owned())
                            .size(11)
                            .wrapping(iced::widget::text::Wrapping::None),
                        Space::with_width(46),
                    ]
                    .align_x(iced::Alignment::Center),
                )
                .on_press(send(ExtensionAction::Press(
                    manifest.id.clone(),
                    Some(each.id.clone()),
                )))
                .style(move |theme, status| opencad_ribbon::tool_btn_style(theme, running, status))
                .height(Fill)
                .padding([6, 3]);
                let mut tip = each
                    .tooltip
                    .as_ref()
                    .map_or_else(|| each.label.get().to_owned(), |tip| tip.get().to_owned());
                tip = format!("{tip}\n{}", manifest.name.get());
                if running {
                    tip = format!("{tip}\n{}", tr("Runs; click to stop it"));
                }
                items.push(opencad_ribbon::RibbonItem::Large(
                    tooltip(
                        control,
                        container(text(tip).size(11))
                            .padding([4, 7])
                            .style(|theme| {
                                let colors = ui_theme::colors(theme);
                                container::Style::default()
                                    .background(colors.panel_alt)
                                    .color(colors.text)
                            }),
                        tooltip::Position::Bottom,
                    )
                    .into(),
                ));
            }
        }
        (!items.is_empty()).then(|| opencad_ribbon::render_group_items("EXTENSIONS", items))
    }

    /// The runs of extensions in the status bar: the name, how far it is,
    /// and Stop.
    pub(crate) fn extension_runs_status(&self) -> Option<Element<'_, Message>> {
        let host = &self.extension_host;
        let (id, run) = host.runs.iter().next()?;
        let mut line = host.name_of(id);
        if let Some((_, text)) = &run.progress {
            if !text.is_empty() {
                line = format!("{line} · {text}");
            }
        }
        if host.runs.len() > 1 {
            line = tr_args(
                "{name} and {more} more",
                &[("name", &line), ("more", &(host.runs.len() - 1))],
            );
        }
        let mut segment = row![text(format!("▶ {line}"))
            .size(11)
            .wrapping(iced::widget::text::Wrapping::None)]
        .spacing(8)
        .align_y(iced::Alignment::Center);
        if let Some((percent, _)) = &run.progress {
            segment = segment.push(
                progress_bar(0.0..=100.0, *percent as f32)
                    .width(Length::Fixed(70.0))
                    .height(6),
            );
            segment = segment.push(text(format!("{percent:.0}%")).size(11));
        }
        segment = segment.push(
            button(text(tr("Stop")).size(11))
                .on_press_maybe(
                    (!run.control.stopping()).then(|| send(ExtensionAction::Stop(id.clone()))),
                )
                .style(flat_tool_style)
                .padding([1, 6]),
        );
        Some(segment.into())
    }

    /// The tiles the enabled extensions add to a page of the File view.
    pub(crate) fn extension_tiles(&self, page: EntryPage) -> Vec<Element<'_, Message>> {
        let muted = self.ui_theme.colors().muted;
        let mut tiles = Vec::new();
        for installed in &self.extension_host.installed {
            let manifest = &installed.manifest;
            if !self.extensions.enabled(&manifest.id) {
                continue;
            }
            let running = self.extension_host.runs.contains_key(&manifest.id);
            for tile in manifest.file_view.iter().filter(|tile| tile.page == page) {
                let detail = tile.detail.as_ref().map_or_else(
                    || manifest.name.get().to_owned(),
                    |detail| detail.get().to_owned(),
                );
                let detail = if running {
                    tr_args("{name} runs", &[("name", &manifest.name.get())])
                } else {
                    detail
                };
                tiles.push(
                    button(
                        column![
                            text(tile.title.get().to_owned()).size(15),
                            text(detail).size(12).color(muted),
                        ]
                        .spacing(3),
                    )
                    .on_press_maybe((!running).then(|| {
                        send(ExtensionAction::Press(
                            manifest.id.clone(),
                            Some(tile.id.clone()),
                        ))
                    }))
                    .style(|theme, status| {
                        let mut style = opencad_ribbon::tool_btn_style(theme, false, status);
                        let colors = ui_theme::colors(theme);
                        style.border.color = colors.border;
                        style.border.width = 1.0;
                        style.border.radius = 4.0.into();
                        if style.background.is_none() {
                            style.background = Some(colors.panel.into());
                        }
                        style
                    })
                    .width(Fill)
                    .padding([12, 16])
                    .into(),
                );
            }
        }
        tiles
    }
}
