//! The File view: the backstage the File button opens over the model. Its
//! menu starts imports and exports, chooses a page and holds the Settings,
//! About and Exit entries of the OpenAEC style book; the pages are the
//! workspace with the open scans, the extensions and what the application is.

use std::sync::atomic::Ordering;

use iced::widget::{
    button, column, container, horizontal_space, pick_list, row, scrollable, text, Space,
};
use iced::{Element, Fill, Font, Task};
use pointcloud_core::ExportFormat;
use serde_json::{json, Value};

use crate::i18n::{key, tr};
use crate::{
    display_name, extensions, format_count, opencad_ribbon, settings_dialog, sidebar_style,
    themed_pick_list_style, ui_theme, views, CloudEntry, Message, Studio,
};

/// A page of the File view. A further page is a variant here, its names in
/// `label` and `id`, its place in `ALL` and its content in
/// `Studio::file_page_view`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilePage {
    /// The open scans and the export settings.
    #[default]
    Workspace,
    /// The optional features and their switches.
    Extensions,
    About,
}

impl FilePage {
    /// The pages in the order of the menu.
    pub const ALL: [Self; 3] = [Self::Workspace, Self::Extensions, Self::About];

    /// The English name of the page in the menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => key("Workspace"),
            Self::Extensions => key("Extensions"),
            Self::About => key("About"),
        }
    }

    /// The name the local API knows the page by.
    pub fn id(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Extensions => "extensions",
            Self::About => "about",
        }
    }

    pub fn from_id(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|page| page.id() == value)
    }

    /// Every id `from_id` accepts, for the command that shows a page.
    pub fn ids() -> Vec<&'static str> {
        Self::ALL.into_iter().map(Self::id).collect()
    }
}

/// An entry of the File view menu that starts a task and closes the view.
#[derive(Debug, Clone, Copy)]
pub enum FileAction {
    Import,
    ImportFolder,
    /// Show the panel that downloads buildings of the 3D BAG.
    Bag3d,
    Activate(usize),
    ExportFull,
    ExportSelection,
    ExportWithoutSelection,
    ExportSection,
    /// Draw what the section box cuts as a 2D drawing: the block of the
    /// Section drawing tool opens first, so that the view can be chosen.
    ExportDrawing,
    ExportDecimated,
    ExportMesh,
    /// Ask where to save the faces detected in the active scan.
    ExportFaces,
    ExportBcf,
    MergeVisible,
    CancelMerge,
}

/// The height of a menu row that leads to a page.
const PAGE_ROW_H: f32 = 40.0;

impl Studio {
    /// Carry out an entry of the File view menu and return to the model.
    pub(crate) fn file_action(&mut self, action: FileAction) -> Task<Message> {
        self.file_open = false;
        self.update(match action {
            FileAction::Import => Message::Open,
            FileAction::ImportFolder => Message::OpenFolder,
            FileAction::Bag3d => Message::ShowBagPanel,
            FileAction::Activate(index) => Message::Select(index),
            FileAction::ExportFull => Message::Export,
            FileAction::ExportSelection => Message::ExportSelection,
            FileAction::ExportWithoutSelection => Message::RemoveSelection,
            FileAction::ExportSection => Message::ExportSection,
            FileAction::ExportDrawing => {
                Message::Drawing(crate::drawing::DrawingAction::ExportFromFile)
            }
            FileAction::ExportDecimated => Message::Decimate,
            FileAction::ExportMesh => Message::ExportMesh,
            FileAction::ExportFaces => Message::Faces(crate::faces::FaceAction::Export),
            FileAction::ExportBcf => Message::Views(views::ViewAction::ExportBcf),
            FileAction::MergeVisible => Message::MergeVisible,
            FileAction::CancelMerge => Message::CancelMerge,
        })
    }

    pub(crate) fn file_view(&self) -> Element<'_, Message> {
        row![
            self.file_menu(),
            container(scrollable(self.file_page_view()).height(Fill))
                .padding([30, 40])
                .width(Fill)
                .height(Fill)
                .style(|theme| container::Style::default()
                    .background(ui_theme::colors(theme).panel_alt)),
        ]
        .height(Fill)
        .into()
    }

    /// The page the menu has chosen.
    fn file_page_view(&self) -> Element<'_, Message> {
        match self.file_page {
            FilePage::Workspace => self.workspace_page(),
            FilePage::Extensions => self.extensions_page(),
            FilePage::About => self.about_page(),
        }
    }

    /// Whether the File view covers the model, and with which page, as
    /// `status` of the local API reports it.
    pub(crate) fn file_view_value(&self) -> Value {
        json!({
            "open": self.file_open,
            "page": self.file_open.then(|| self.file_page.id()),
        })
    }

    /// The `file_view` command of the local API: open the File view, on a
    /// page when one is named, or return to the model. A screenshot shows the
    /// model only, so a caller closes the view before it takes one.
    pub(crate) fn api_file_view(&mut self, open: bool, page: Option<&str>) -> Value {
        let refuse = |error: String| json!({"ok": false, "error": error});
        let page = match page.map(|id| FilePage::from_id(&id.to_ascii_lowercase())) {
            Some(None) => {
                return refuse(format!("unknown page; use {}", FilePage::ids().join(", ")))
            }
            Some(found) => found,
            None => None,
        };
        if !open && page.is_some() {
            return refuse("a page can only be shown with open: true".into());
        }
        // The dialog lies over the whole window and closes the File view when
        // it opens, so the two are never shown together.
        if open && self.settings.is_some() {
            return refuse("the Settings dialog is open".into());
        }
        if open != self.file_open {
            self.file_open = open;
            self.file_page = FilePage::default();
            self.ribbon_viewport = None;
        }
        if let Some(page) = page {
            self.file_page = page;
        }
        json!({"ok": true, "file_view": self.file_view_value()})
    }

    /// The menu at the left: the tasks, which scroll when the window is low,
    /// above the pages and the entries every OpenAEC application has.
    fn file_menu(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let item = |label: &'static str, message: Option<Message>| {
            button(text(tr(label)).size(14))
                .on_press_maybe(message)
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(Fill)
                .padding([11, 18])
        };
        let entry = |label: &'static str, action: FileAction, available: bool| {
            item(label, available.then_some(Message::FileAction(action)))
        };
        let active_cloud = self.active.and_then(|index| self.clouds.get(index));
        let active_selected = active_cloud
            .and_then(|entry| entry.selection.as_ref())
            .map_or(0, |selection| selection.count);
        let tasks = column![
            container(text(tr("FILE")).size(12).color(colors.accent)).padding([20, 18]),
            entry("Import point cloud…", FileAction::Import, true),
            entry("Open scan folder…", FileAction::ImportFolder, true),
            entry(
                "3D BAG buildings…",
                FileAction::Bag3d,
                self.extensions.enabled(extensions::BAG3D),
            ),
            container(text(tr("EXPORT")).size(10).color(colors.muted)).padding(iced::Padding {
                top: 22.0,
                right: 18.0,
                bottom: 7.0,
                left: 18.0,
            }),
            entry(
                "Full resolution…",
                FileAction::ExportFull,
                active_cloud.is_some()
            ),
            entry(
                "Selected points…",
                FileAction::ExportSelection,
                active_selected > 0
            ),
            entry(
                "Without selected points…",
                FileAction::ExportWithoutSelection,
                active_selected > 0
            ),
            entry(
                "Section box…",
                FileAction::ExportSection,
                active_cloud.is_some() && self.section_enabled && !self.section_export_pending,
            ),
            entry(
                "Section drawing…",
                FileAction::ExportDrawing,
                self.drawing_entry_enabled(),
            ),
            entry(
                "Every Nth point…",
                FileAction::ExportDecimated,
                active_cloud.is_some()
            ),
            entry(
                "Merge visible LAS/LAZ scans…",
                FileAction::MergeVisible,
                self.merge_job.is_none()
                    && !self.merge_dialog_pending
                    && self.visible_merge_sources().is_ok(),
            ),
            entry(
                "Cancel merge",
                FileAction::CancelMerge,
                self.merge_job.is_some(),
            ),
            entry(
                "Surface mesh…",
                FileAction::ExportMesh,
                active_cloud.is_some_and(|entry| entry.mesh.is_some()) && !self.mesh_export_pending,
            ),
            entry(
                "Detected faces…",
                FileAction::ExportFaces,
                self.faces_entry_enabled(),
            ),
            entry(
                "Views as BCF…",
                FileAction::ExportBcf,
                self.can_export_bcf(),
            ),
        ]
        .width(Fill);

        // The open page carries the accent bar of the style book at its left.
        let page = |page: FilePage| {
            let open = self.file_page == page;
            row![
                container(Space::new(3, Fill)).style(move |theme| {
                    let colors = ui_theme::colors(theme);
                    container::Style::default().background(if open {
                        colors.accent
                    } else {
                        colors.panel
                    })
                }),
                button(text(tr(page.label())).size(14))
                    .on_press(Message::FilePage(page))
                    .style(move |theme, status| opencad_ribbon::tool_btn_style(theme, open, status))
                    .width(Fill)
                    .height(Fill)
                    .padding([11, 15]),
            ]
            .height(PAGE_ROW_H)
        };
        let pages = FilePage::ALL
            .into_iter()
            .fold(column![].width(Fill), |pages, each| pages.push(page(each)));
        let footer = column![
            container(Space::new(Fill, 1)).style(
                |theme| container::Style::default().background(ui_theme::colors(theme).border)
            ),
            pages,
            item(
                "Settings…",
                Some(Message::Settings(settings_dialog::SettingsAction::Open)),
            ),
            button(text(tr("←  Return to model")).size(13))
                .on_press(Message::ToggleFile)
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(Fill)
                .padding([13, 18]),
            // Exit ends the session at once: it stands apart, below the entry
            // that is used most.
            container(Space::new(Fill, 1)).style(
                |theme| container::Style::default().background(ui_theme::colors(theme).border)
            ),
            item("Exit", Some(Message::Exit)),
        ]
        .width(Fill);

        container(column![scrollable(tasks).height(Fill), footer].height(Fill))
            .width(260)
            .height(Fill)
            .style(sidebar_style)
            .into()
    }

    /// The open scans with their sizes, and what an export writes.
    fn workspace_page(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
        let caption = |label: &'static str| {
            container(text(tr(label)).size(10).color(colors.muted)).padding(iced::Padding {
                top: 28.0,
                bottom: 4.0,
                ..iced::Padding::ZERO
            })
        };
        let active_cloud = self.active.and_then(|index| self.clouds.get(index));
        let total_points: u64 = self.clouds.iter().map(CloudEntry::remaining_count).sum();
        let active_name = active_cloud.map_or(tr("No active scan"), |entry| {
            display_name(&entry.cloud.path)
        });
        let open_scans = self.clouds.iter().enumerate().fold(
            column![].spacing(3).width(Fill),
            |rows, (index, entry)| {
                rows.push(
                    button(
                        row![
                            text(display_name(&entry.cloud.path)).size(13),
                            horizontal_space(),
                            text(format!("{} points", format_count(entry.remaining_count())))
                                .size(11)
                                .color(colors.muted),
                        ]
                        .spacing(16)
                        .align_y(iced::Alignment::Center),
                    )
                    .on_press(Message::FileAction(FileAction::Activate(index)))
                    .style(move |theme, status| {
                        opencad_ribbon::tool_btn_style(theme, self.active == Some(index), status)
                    })
                    .width(Fill)
                    .padding([9, 12]),
                )
            },
        );
        let mut details = column![
            text(tr("Point cloud workspace"))
                .size(26)
                .font(Font::with_name("Space Grotesk")),
            text(format!(
                "{} files  ·  {} points  ·  {} selected",
                self.clouds.len(),
                format_count(total_points),
                format_count(self.selected_total()),
            ))
            .size(13)
            .color(colors.muted),
            caption("CURRENT SCAN"),
            text(active_name).size(16),
            caption("OPEN SCANS"),
            container(open_scans).width(Fill).max_width(560),
            caption("EXPORT FORMAT"),
            pick_list(
                ExportFormat::ALL,
                Some(self.export_format),
                Message::ExportFormat,
            )
            .style(themed_pick_list_style)
            .width(240),
            caption("EVERY NTH POINT"),
            row![
                text(tr("Keep 1 in")).size(13),
                pick_list(
                    [2u64, 5, 10, 20, 50, 100],
                    Some(self.decimation_stride),
                    Message::DecimationStride
                )
                .style(themed_pick_list_style)
                .width(90),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
            container(
                text(tr(
                    "Choose an export format, then save the active scan or selection."
                ))
                .size(12)
                .color(colors.muted),
            )
            .padding(iced::Padding {
                top: 32.0,
                ..iced::Padding::ZERO
            }),
        ]
        .spacing(8)
        .width(Fill);
        if let Some(job) = &self.merge_job {
            let processed = job.control.processed.load(Ordering::Relaxed);
            details = details
                .push(text(job.progress_text()).size(13))
                .push(
                    iced::widget::progress_bar(
                        0.0..=1.0,
                        processed as f32 / job.control.total.max(1) as f32,
                    )
                    .height(8),
                )
                .push(
                    text(format!(
                        "{} points written to {}",
                        format_count(job.control.written.load(Ordering::Relaxed)),
                        job.path.display()
                    ))
                    .size(11),
                )
                .push(button(tr("Cancel merge")).on_press(Message::CancelMerge));
        }
        details.into()
    }

    /// What the application is, as the About tab of Settings shows it.
    fn about_page(&self) -> Element<'_, Message> {
        column![
            text(tr(FilePage::About.label()))
                .size(26)
                .font(Font::with_name("Space Grotesk")),
            settings_dialog::about(self.ui_theme.colors()),
        ]
        .spacing(18)
        .width(Fill)
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_view_opens_on_the_workspace_and_shows_each_page() {
        let mut studio = Studio::default();
        let _ = studio.update(Message::ToggleFile);
        assert_eq!(studio.file_page, FilePage::Workspace);
        for page in FilePage::ALL {
            let _ = studio.update(Message::FilePage(page));
            assert!(studio.file_open, "choosing a page keeps the File view");
            assert_eq!(studio.file_page, page);
            let _ = studio.view();
        }
        assert_eq!(studio.file_page, FilePage::About);

        // Leaving and opening again starts at the workspace.
        let _ = studio.update(Message::ToggleFile);
        let _ = studio.update(Message::ToggleFile);
        assert!(studio.file_open);
        assert_eq!(studio.file_page, FilePage::Workspace);
    }

    #[test]
    fn every_page_has_a_translated_name() {
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::Table(0));
        let names: Vec<&str> = FilePage::ALL
            .into_iter()
            .map(|page| tr(page.label()))
            .collect();
        assert_eq!(names, ["Werkruimte", "Extensies", "Over"]);
    }

    fn send(studio: &mut Studio, command: crate::native_api::ApiCommand) -> Value {
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(crate::native_api::ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    fn file_view(studio: &mut Studio, open: bool, page: Option<&str>) -> Value {
        send(
            studio,
            crate::native_api::ApiCommand::FileView {
                open,
                page: page.map(str::to_owned),
            },
        )
    }

    fn reported(studio: &mut Studio) -> Value {
        send(studio, crate::native_api::ApiCommand::Status)["result"]["file_view"].clone()
    }

    #[test]
    fn page_ids_are_unique_and_round_trip() {
        let ids = FilePage::ids();
        assert_eq!(ids, ["workspace", "extensions", "about"]);
        for page in FilePage::ALL {
            assert_eq!(FilePage::from_id(page.id()), Some(page));
        }
        assert_eq!(FilePage::from_id("settings"), None);
    }

    #[test]
    fn api_opens_the_file_view_on_a_page_and_closes_it() {
        let mut studio = Studio::default();
        assert_eq!(reported(&mut studio), json!({"open": false, "page": null}));

        let opened = file_view(&mut studio, true, None);
        assert_eq!(
            opened,
            json!({"ok": true, "file_view": {"open": true, "page": "workspace"}})
        );
        assert!(studio.file_open);
        assert_eq!(reported(&mut studio), opened["file_view"]);

        for page in FilePage::ALL {
            let shown = file_view(&mut studio, true, Some(page.id()));
            assert_eq!(shown["file_view"]["page"], page.id());
            assert_eq!(studio.file_page, page);
            let _ = studio.view();
        }
        // Opening the open view without a page keeps the page it shows.
        let again = file_view(&mut studio, true, None);
        assert_eq!(again["file_view"]["page"], "about");
        // The id is read without regard to case, as a theme is.
        assert_eq!(
            file_view(&mut studio, true, Some("Extensions"))["file_view"]["page"],
            "extensions"
        );

        let closed = file_view(&mut studio, false, None);
        assert_eq!(
            closed,
            json!({"ok": true, "file_view": {"open": false, "page": null}})
        );
        assert!(!studio.file_open);
        assert_eq!(file_view(&mut studio, false, None)["ok"], true);
        // A view opened again starts on a page of its own choice.
        let _ = file_view(&mut studio, true, Some("extensions"));
        assert_eq!(studio.file_page, FilePage::Extensions);
        let _ = file_view(&mut studio, false, None);
        let _ = studio.update(Message::ToggleFile);
        assert_eq!(studio.file_page, FilePage::Workspace);
    }

    #[test]
    fn api_refuses_what_the_window_cannot_show() {
        // Leaving the dialog puts the language back, which other tests set.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let mut studio = Studio::default();
        let unknown = file_view(&mut studio, true, Some("settings"));
        assert_eq!(
            unknown,
            json!({"ok": false, "error": "unknown page; use workspace, extensions, about"})
        );
        assert!(!studio.file_open);
        let _ = file_view(&mut studio, true, Some("about"));
        let closing = file_view(&mut studio, false, Some("about"));
        assert_eq!(closing["error"], "a page can only be shown with open: true");
        assert!(studio.file_open, "a refused command changes nothing");
        assert_eq!(studio.file_page, FilePage::About);

        let _ = studio.update(Message::Settings(settings_dialog::SettingsAction::Open));
        assert!(!studio.file_open);
        let covered = file_view(&mut studio, true, None);
        assert_eq!(covered["error"], "the Settings dialog is open");
        assert!(!studio.file_open);
        assert_eq!(file_view(&mut studio, false, None)["ok"], true);
        let _ = studio.update(Message::Escape);
    }

    #[test]
    fn settings_entry_leaves_the_file_view_for_the_dialog() {
        // Leaving the dialog puts the language back, which other tests set.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let mut studio = Studio::default();
        let _ = studio.update(Message::ToggleFile);
        let _ = studio.update(Message::Settings(settings_dialog::SettingsAction::Open));
        assert!(!studio.file_open);
        assert!(studio.settings_view().is_some());
        let _ = studio.view();
        let _ = studio.update(Message::Escape);
        assert!(studio.settings_view().is_none());
    }

    #[test]
    fn file_view_builds_with_a_scan_in_dutch() {
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::Table(0));
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("hall.xyz");
        std::fs::write(&path, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
        let cloud = std::sync::Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        let _ = studio.update(Message::ToggleFile);
        let _ = studio.view();
        assert_eq!(tr("Import point cloud…"), "Puntenwolk importeren…");
        assert_eq!(tr("Exit"), "Afsluiten");

        // An export entry closes the view.
        let _ = studio.update(Message::FileAction(FileAction::Activate(0)));
        assert!(!studio.file_open);
        assert_eq!(studio.active, Some(0));
    }
}
