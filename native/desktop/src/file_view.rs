//! The File view: the backstage the File button opens over the model. Its
//! menu leads to the pages New, Open, Import and Export, which hold the file
//! tasks as tiles, to the workspace with the open scans, the extensions and
//! what the application is, and holds the Settings, Return and Exit entries
//! of the OpenAEC style book.

use std::sync::atomic::Ordering;

use iced::widget::{
    button, column, container, horizontal_space, pick_list, row, scrollable, text, Column, Space,
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
    /// Start again with an empty workspace.
    New,
    /// Open scan files and scan folders.
    Open,
    /// Bring in data from elsewhere, such as the 3D BAG.
    Import,
    /// Every export, grouped by what it writes.
    Export,
    /// The open scans.
    #[default]
    Workspace,
    /// The optional features and their switches.
    Extensions,
    About,
}

impl FilePage {
    /// The pages in the order of the menu.
    pub const ALL: [Self; 7] = [
        Self::New,
        Self::Open,
        Self::Import,
        Self::Export,
        Self::Workspace,
        Self::Extensions,
        Self::About,
    ];

    /// The English name of the page in the menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::New => key("New"),
            Self::Open => key("Open"),
            Self::Import => key("Import"),
            Self::Export => key("Export"),
            Self::Workspace => key("Workspace"),
            Self::Extensions => key("Extensions"),
            Self::About => key("About"),
        }
    }

    /// The name the local API knows the page by.
    pub fn id(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Open => "open",
            Self::Import => "import",
            Self::Export => "export",
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

    /// The pages that do the file tasks; the menu sets them apart from the
    /// pages that show what is open and what the application is.
    fn is_task(self) -> bool {
        matches!(self, Self::New | Self::Open | Self::Import | Self::Export)
    }
}

/// A tile of a File view page that starts a task and closes the view.
#[derive(Debug, Clone, Copy)]
pub enum FileAction {
    /// Close every open scan; the files themselves are not touched.
    NewWorkspace,
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
/// The widest a column of tiles grows on a wide window.
const TILE_COLUMN_W: f32 = 620.0;

impl Studio {
    /// Carry out a tile of the File view and return to the model.
    pub(crate) fn file_action(&mut self, action: FileAction) -> Task<Message> {
        self.file_open = false;
        let message = match action {
            FileAction::NewWorkspace => {
                let task = self.remove_clouds((0..self.clouds.len()).collect());
                self.status = tr("New workspace: the open scans were closed").into();
                return task;
            }
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
        };
        self.update(message)
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
            FilePage::New => self.new_page(),
            FilePage::Open => self.open_page(),
            FilePage::Import => self.import_page(),
            FilePage::Export => self.export_page(),
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

    /// The menu at the left: the task pages, then the pages that show what
    /// is open and what the application is, then the entries every OpenAEC
    /// application has.
    fn file_menu(&self) -> Element<'_, Message> {
        let rule = || {
            container(Space::new(Fill, 1)).style(|theme| {
                container::Style::default().background(ui_theme::colors(theme).border)
            })
        };
        let item = |label: &'static str, message: Message| {
            button(text(tr(label)).size(14))
                .on_press(message)
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(Fill)
                .padding([11, 18])
        };
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
        let pages = |task: bool| {
            FilePage::ALL
                .into_iter()
                .filter(|each| each.is_task() == task)
                .fold(column![].width(Fill), |pages, each| pages.push(page(each)))
        };
        let top = column![
            Space::new(Fill, 14),
            pages(true),
            container(rule()).padding([10, 0]),
            pages(false),
        ]
        .width(Fill);
        let footer = column![
            rule(),
            item(
                "Settings…",
                Message::Settings(settings_dialog::SettingsAction::Open),
            ),
            button(text(tr("←  Return to model")).size(13))
                .on_press(Message::ToggleFile)
                .style(|theme, status| opencad_ribbon::tool_btn_style(theme, false, status))
                .width(Fill)
                .padding([13, 18]),
            // Exit ends the session at once: it stands apart, below the entry
            // that is used most.
            rule(),
            item("Exit", Message::Exit),
        ]
        .width(Fill);

        container(column![scrollable(top).height(Fill), footer].height(Fill))
            .width(260)
            .height(Fill)
            .style(sidebar_style)
            .into()
    }

    /// The title of a page with one line under it that says what it is for.
    fn page_heading(&self, title: &'static str, lead: &'static str) -> Column<'_, Message> {
        column![
            text(tr(title))
                .size(26)
                .font(Font::with_name("Space Grotesk")),
            text(tr(lead)).size(13).color(self.ui_theme.colors().muted),
        ]
        .spacing(6)
    }

    /// A small heading above a group of tiles.
    fn group_caption(&self, label: &'static str) -> Element<'_, Message> {
        container(text(tr(label)).size(10).color(self.ui_theme.colors().muted))
            .padding(iced::Padding {
                top: 26.0,
                bottom: 6.0,
                ..iced::Padding::ZERO
            })
            .into()
    }

    /// A task as a tile: its name, and under it what it writes or what it
    /// needs. A task that cannot run now is shown greyed.
    fn file_tile(
        &self,
        title: &'static str,
        detail: &'static str,
        action: FileAction,
        available: bool,
    ) -> Element<'_, Message> {
        let muted = self.ui_theme.colors().muted;
        button(
            column![
                text(tr(title)).size(15),
                text(tr(detail)).size(12).color(muted),
            ]
            .spacing(3),
        )
        .on_press_maybe(available.then_some(Message::FileAction(action)))
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
        .into()
    }

    /// A column of tiles that does not grow wider than reads well.
    fn tiles<'a>(tiles: impl IntoIterator<Item = Element<'a, Message>>) -> Element<'a, Message> {
        container(Column::with_children(tiles).spacing(8).width(Fill))
            .width(Fill)
            .max_width(TILE_COLUMN_W)
            .into()
    }

    fn new_page(&self) -> Element<'_, Message> {
        column![
            self.page_heading(
                key("New"),
                key("Start again with an empty workspace. The files on disk are not changed."),
            ),
            Space::new(Fill, 18),
            Self::tiles([self.file_tile(
                key("Empty workspace"),
                key("Closes every open scan and mesh"),
                FileAction::NewWorkspace,
                !self.clouds.is_empty(),
            )]),
        ]
        .width(Fill)
        .into()
    }

    fn open_page(&self) -> Element<'_, Message> {
        column![
            self.page_heading(
                key("Open"),
                key("Add scans to the workspace. Each file becomes a layer; several can be chosen at once."),
            ),
            Space::new(Fill, 18),
            Self::tiles([
                self.file_tile(
                    key("Point cloud…"),
                    key("Scan files and scan project files"),
                    FileAction::Import,
                    true,
                ),
                self.file_tile(
                    key("Scan folder…"),
                    key("Every scan file in a folder"),
                    FileAction::ImportFolder,
                    true,
                ),
            ]),
        ]
        .width(Fill)
        .into()
    }

    fn import_page(&self) -> Element<'_, Message> {
        let bag = self.extensions.enabled(extensions::BAG3D);
        column![
            self.page_heading(
                key("Import"),
                key("Bring in data from other sources beside the scans."),
            ),
            Space::new(Fill, 18),
            Self::tiles([self.file_tile(
                key("3D BAG buildings…"),
                if bag {
                    key("Download the buildings of an area in the Netherlands as a mesh")
                } else {
                    key("Switch on the 3D BAG extension under Extensions first")
                },
                FileAction::Bag3d,
                bag,
            )]),
        ]
        .width(Fill)
        .into()
    }

    /// Every export, grouped by what it writes. The format of a point cloud
    /// export and the step of Every Nth point are chosen here as well.
    fn export_page(&self) -> Element<'_, Message> {
        let active_cloud = self.active.and_then(|index| self.clouds.get(index));
        let active = active_cloud.is_some();
        let selected = active_cloud
            .and_then(|entry| entry.selection.as_ref())
            .map_or(0, |selection| selection.count)
            > 0;
        let format = row![
            text(tr("Format")).size(13),
            pick_list(
                ExportFormat::ALL,
                Some(self.export_format),
                Message::ExportFormat,
            )
            .style(themed_pick_list_style)
            .width(220),
            Space::new(18, 1),
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
        .align_y(iced::Alignment::Center);

        let mut page = column![
            self.page_heading(
                key("Export"),
                key("Save the active scan, a part of it, or what was made from it."),
            ),
            self.group_caption(key("POINT CLOUD")),
            container(format).padding(iced::Padding {
                bottom: 8.0,
                ..iced::Padding::ZERO
            }),
            Self::tiles([
                self.file_tile(
                    key("Full resolution…"),
                    key("Every point of the active scan in the chosen format"),
                    FileAction::ExportFull,
                    active,
                ),
                self.file_tile(
                    key("Selected points…"),
                    key("Only the selected points; needs a selection"),
                    FileAction::ExportSelection,
                    selected,
                ),
                self.file_tile(
                    key("Without selected points…"),
                    key("The active scan without the selected points; needs a selection"),
                    FileAction::ExportWithoutSelection,
                    selected,
                ),
                self.file_tile(
                    key("Section box…"),
                    key("The points inside the section box; needs the section box"),
                    FileAction::ExportSection,
                    active && self.section_enabled && !self.section_export_pending,
                ),
                self.file_tile(
                    key("Every Nth point…"),
                    key("A thinned copy that keeps one point in the chosen step"),
                    FileAction::ExportDecimated,
                    active,
                ),
            ]),
            self.group_caption(key("DRAWINGS AND MODELS")),
            Self::tiles([
                self.file_tile(
                    key("Section drawing…"),
                    key("A plan or a vertical section as DXF or DWG; needs the section box"),
                    FileAction::ExportDrawing,
                    self.drawing_entry_enabled(),
                ),
                self.file_tile(
                    key("Surface mesh…"),
                    key("The mesh of the active scan as OBJ, PLY, STL, DXF, DWG or IFC"),
                    FileAction::ExportMesh,
                    active_cloud.is_some_and(|entry| entry.mesh.is_some())
                        && !self.mesh_export_pending,
                ),
                self.file_tile(
                    key("Detected faces…"),
                    key("Planes and cylinders as JSON, OBJ, DXF, DWG or IFC"),
                    FileAction::ExportFaces,
                    self.faces_entry_enabled(),
                ),
            ]),
            self.group_caption(key("COORDINATION")),
            Self::tiles([self.file_tile(
                key("Views as BCF…"),
                key("The saved views with their notes, for issue tracking"),
                FileAction::ExportBcf,
                self.can_export_bcf(),
            )]),
            self.group_caption(key("MERGE")),
            Self::tiles([self.file_tile(
                key("Merge visible LAS/LAZ scans…"),
                key("One LAZ file from every visible LAS or LAZ scan"),
                FileAction::MergeVisible,
                self.merge_job.is_none()
                    && !self.merge_dialog_pending
                    && self.visible_merge_sources().is_ok(),
            )]),
        ]
        .width(Fill);
        if let Some(job) = &self.merge_job {
            let processed = job.control.processed.load(Ordering::Relaxed);
            page = page.push(
                column![
                    text(job.progress_text()).size(13),
                    iced::widget::progress_bar(
                        0.0..=1.0,
                        processed as f32 / job.control.total.max(1) as f32,
                    )
                    .height(8),
                    text(format!(
                        "{} points written to {}",
                        format_count(job.control.written.load(Ordering::Relaxed)),
                        job.path.display()
                    ))
                    .size(11),
                    button(text(tr("Cancel merge")))
                        .on_press(Message::FileAction(FileAction::CancelMerge)),
                ]
                .spacing(8)
                .padding(iced::Padding {
                    top: 12.0,
                    ..iced::Padding::ZERO
                }),
            );
        }
        page.into()
    }

    /// The open scans with their sizes; a click makes one the active scan.
    fn workspace_page(&self) -> Element<'_, Message> {
        let colors = self.ui_theme.colors();
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
        column![
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
            self.group_caption(key("CURRENT SCAN")),
            text(active_name).size(16),
            self.group_caption(key("OPEN SCANS")),
            container(open_scans).width(Fill).max_width(TILE_COLUMN_W),
        ]
        .spacing(4)
        .width(Fill)
        .into()
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
        assert_eq!(
            names,
            [
                "Nieuw",
                "Openen",
                "Importeren",
                "Exporteren",
                "Werkruimte",
                "Extensies",
                "Over"
            ]
        );
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
        assert_eq!(
            ids,
            [
                "new",
                "open",
                "import",
                "export",
                "workspace",
                "extensions",
                "about"
            ]
        );
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
            json!({"ok": false, "error": "unknown page; use new, open, import, export, workspace, extensions, about"})
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
        assert_eq!(tr("Point cloud…"), "Puntenwolk…");
        assert_eq!(tr("Exit"), "Afsluiten");

        // An export entry closes the view.
        let _ = studio.update(Message::FileAction(FileAction::Activate(0)));
        assert!(!studio.file_open);
        assert_eq!(studio.active, Some(0));
    }
}
