//! Locks: a saved 3D view, a drawing of VIEWS or a view placed on a sheet
//! can be locked, so that it stays as it was set up.
//!
//! - While a locked 3D view is shown, orbiting, panning, zooming, walking,
//!   the view cube and the section box leave it as it is, and the status
//!   bar says "<name> is locked"; Update is refused. So they do while a
//!   drawing or a sheet lies in front of the scene that holds it, so that
//!   its tab shows it as it was locked. It can still be renamed, duplicated
//!   (the copy is unlocked), deleted and annotated.
//! - The crop region of a locked drawing, its turn with RO and the points it
//!   uses stay; it pans, zooms and takes annotations.
//! - A locked viewport on a sheet is not moved, resized, scaled or removed.
//!
//! The lock is kept with the view, the drawing or the sheet. Properties
//! switches it while it is shown or selected, as does the padlock of the
//! shown row of a drawing and that of the tab of what is locked; a saved
//! view keeps the room on its row for its name, and shows its lock in the
//! icon of the row.

use iced::widget::{button, checkbox, container, row, text, tooltip};
use iced::{Element, Fill, Task};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::i18n::tr;
use crate::native_api::ApiCommand;
use crate::{
    flat_tool_style, icon_svg, muted_checkbox_style, opencad_properties, Message, Studio, ToolIcon,
};

/// What a lock is set on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LockTarget {
    /// A saved 3D view, by its identifier.
    View(String),
    /// A drawing of VIEWS, by its identifier.
    Drawing(String),
    /// A view placed on a sheet, by the sheet and the viewport.
    Viewport { sheet: String, id: String },
}

/// What `lock_view` and `unlock_view` of the local API name.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct LockOptions {
    /// A saved view of the active scan or a drawing of VIEWS, by its name in
    /// any case; with `viewport` the sheet, by its guid, number or name.
    #[serde(default)]
    pub name: Option<String>,
    /// `view`, `drawing` or `viewport`.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub sheet: Option<String>,
    #[serde(default)]
    pub viewport: Option<crate::layouts::ViewportRef>,
}

/// Whether a message would change what the 3D scene shows: its camera, a
/// walk or a photo it stands in, or its section box.
fn changes_the_view(message: &Message) -> bool {
    use crate::file_photos::PhotoAction;
    matches!(
        message,
        Message::Orbit(..)
            | Message::Pan(..)
            | Message::FinishPan(..)
            | Message::FinishOrbit(..)
            | Message::Zoom(..)
            | Message::ResetCamera
            | Message::CameraPreset(_)
            | Message::CubeCorner(_)
            | Message::CubeEdge(_)
            | Message::ToggleOrthographic
            | Message::WalkLook(..)
            | Message::WalkZoom(_)
            | Message::WalkKey(_, true)
            | Message::EnterPanorama(..)
            | Message::LeaveWalk
            | Message::CenterScanPose(..)
            | Message::FitScanPoses
            | Message::ZoomToSection
            | Message::ZoomToSelection
            | Message::SetSectionEnabled(_)
            | Message::SectionMin(..)
            | Message::SectionMax(..)
            | Message::SectionHandleDelta(..)
            | Message::TurnSectionBy(_)
            | Message::ApplySectionCoordinates
            | Message::ApplySectionRotation
            | Message::AlignSectionToWalls
            | Message::ResetSectionBox
            | Message::FitSectionToSelection
            | Message::Photos(
                PhotoAction::Enter(..)
                    | PhotoAction::Step(_)
                    | PhotoAction::Look(..)
                    | PhotoAction::Zoom(_)
            )
    )
}

/// The small button of a row that locks or unlocks it.
fn padlock<'a>(locked: bool, message: Message) -> Element<'a, Message> {
    let (icon, tip) = if locked {
        (ToolIcon::Locked, tr("Locked: click to unlock"))
    } else {
        (ToolIcon::Unlocked, tr("Lock"))
    };
    tooltip(
        button(icon_svg(icon, 12.0))
            .on_press(message)
            .style(flat_tool_style)
            .padding([3, 4]),
        crate::project_browser::hint(tip.to_owned()),
        tooltip::Position::Bottom,
    )
    .gap(4)
    .into()
}

/// What the status bar says of a view, a drawing or a viewport that is
/// locked, in the language of the window.
pub(crate) fn locked_status(name: &str) -> String {
    crate::i18n::tr_args("{name} is locked", &[("name", &name)])
}

/// What the status bar says when a lock is set or taken off.
fn lock_status(name: &str, locked: bool) -> String {
    if locked {
        locked_status(name)
    } else {
        crate::i18n::tr_args("{name} is unlocked", &[("name", &name)])
    }
}

impl Studio {
    /// The name of the locked saved view the 3D scene holds: the one it
    /// shows, also while a drawing, a sheet or the File view lies in front
    /// of it. The camera and the section box of the scene are that view's;
    /// a click on its tab shows the scene as it was left.
    pub(crate) fn locked_scene_view(&self) -> Option<String> {
        let index = self.active_view_index()?;
        let view = &self.views.list[index];
        view.locked.then(|| view.name.clone())
    }

    /// Why a message is refused: it would change the locked view the scene
    /// holds, whatever lies in front of it.
    pub(crate) fn locked_refusal(&self, message: &Message) -> Option<String> {
        if !changes_the_view(message) {
            return None;
        }
        // A covered model ignores the walk keys by itself.
        if self.model_covered() && matches!(message, Message::WalkKey(..)) {
            return None;
        }
        self.locked_scene_view().map(|name| locked_status(&name))
    }

    /// The name of the locked view a command of the local API would change:
    /// the one the scene holds, or the one it would update. The command is
    /// refused.
    pub(crate) fn api_locked_refusal(&self, command: &ApiCommand) -> Option<String> {
        if let ApiCommand::UpdateCameraView { name } = command {
            let source = self.active_camera_source()?;
            return self
                .views
                .list
                .iter()
                .find(|view| view.source == source && view.name.eq_ignore_ascii_case(name.trim()))
                .filter(|view| view.locked)
                .map(|view| view.name.clone());
        }
        let changes = match command {
            ApiCommand::Camera { .. }
            | ApiCommand::SetCamera { .. }
            | ApiCommand::Orbit { .. }
            | ApiCommand::ZoomAll
            | ApiCommand::SetSection { .. }
            | ApiCommand::ClearSection
            | ApiCommand::AlignSectionToWalls
            | ApiCommand::Walk { .. }
            | ApiCommand::OpenPanorama { .. }
            | ApiCommand::SetPanorama { .. }
            | ApiCommand::ClosePanorama
            | ApiCommand::ZoomSelection
            | ApiCommand::EnterPhoto { .. }
            | ApiCommand::NextPhoto
            | ApiCommand::PreviousPhoto => true,
            // Without a drawing shown, RO turns the section box.
            ApiCommand::RotateCrop { name: None, .. } => !self.drawing_view.shown,
            _ => false,
        };
        if !changes {
            return None;
        }
        self.locked_scene_view()
    }

    /// Whether what a tab shows is locked.
    pub(crate) fn tab_locked(&self, tab: &crate::view_tabs::TabId) -> bool {
        match tab {
            crate::view_tabs::TabId::View(guid) => self.view_locked(guid),
            crate::view_tabs::TabId::Drawing(guid) => self.drawing_locked(guid),
            _ => false,
        }
    }

    pub(crate) fn view_locked(&self, guid: &str) -> bool {
        self.views
            .list
            .iter()
            .any(|view| view.guid == guid && view.locked)
    }

    pub(crate) fn drawing_locked(&self, guid: &str) -> bool {
        self.drawing_view
            .saved
            .iter()
            .any(|drawing| drawing.guid == guid && drawing.locked)
    }

    /// Whether a target is locked now.
    pub(crate) fn is_locked(&self, target: &LockTarget) -> bool {
        match target {
            LockTarget::View(guid) => self.view_locked(guid),
            LockTarget::Drawing(guid) => self.drawing_locked(guid),
            LockTarget::Viewport { sheet, id } => self
                .layouts
                .layout(sheet)
                .and_then(|layout| layout.viewport(id))
                .is_some_and(|viewport| viewport.locked),
        }
    }

    /// Lock or unlock a view, a drawing or a viewport and keep it; answers
    /// its name.
    pub(crate) fn set_lock(&mut self, target: &LockTarget, locked: bool) -> Result<String, String> {
        match target {
            LockTarget::View(guid) => {
                let index = self
                    .views
                    .list
                    .iter()
                    .position(|view| view.guid == *guid)
                    .ok_or_else(|| "That view is no longer saved".to_owned())?;
                let before = std::mem::replace(&mut self.views.list[index].locked, locked);
                if let Err(error) = crate::camera_views::save(&self.views.list) {
                    self.views.list[index].locked = before;
                    return Err(format!("the views could not be stored: {error}"));
                }
                Ok(self.views.list[index].name.clone())
            }
            LockTarget::Drawing(guid) => {
                let saved = &mut self.drawing_view.saved;
                let index = saved
                    .iter()
                    .position(|drawing| drawing.guid == *guid)
                    .ok_or_else(|| "That drawing is no longer kept".to_owned())?;
                let before = std::mem::replace(&mut saved[index].locked, locked);
                if let Err(error) = crate::saved_drawings::save(saved) {
                    saved[index].locked = before;
                    return Err(format!("The drawings could not be stored: {error}"));
                }
                Ok(saved[index].name.clone())
            }
            LockTarget::Viewport { sheet, id } => {
                let mut name = String::new();
                self.set_layout_field(sheet, |layout| {
                    let viewport = layout
                        .viewport_mut(id)
                        .ok_or_else(|| "That viewport is no longer on the sheet".to_owned())?;
                    viewport.locked = locked;
                    name = viewport.shown_title().to_owned();
                    Ok(())
                })?;
                Ok(name)
            }
        }
    }

    /// A click on a padlock: lock what is unlocked, unlock what is locked.
    pub(crate) fn update_lock(&mut self, target: LockTarget) -> Task<Message> {
        let locked = !self.is_locked(&target);
        match self.set_lock(&target, locked) {
            Ok(name) => self.status = lock_status(&name, locked),
            Err(error) => self.status = error,
        }
        Task::none()
    }

    /// The padlock of a row of VIEWS or of a viewport.
    pub(crate) fn lock_button<'a>(&self, target: LockTarget) -> Element<'a, Message> {
        let locked = self.is_locked(&target);
        padlock(locked, Message::Lock(target))
    }

    /// The icon of a row of VIEWS: a padlock while what it stands for is
    /// locked, so that every row shows it, also those without their
    /// buttons; otherwise the icon of its kind.
    pub(crate) fn row_icon(&self, target: &LockTarget, icon: ToolIcon) -> ToolIcon {
        if self.is_locked(target) {
            ToolIcon::Locked
        } else {
            icon
        }
    }

    /// The padlock on the tab of a locked view or drawing, which unlocks it.
    pub(crate) fn tab_padlock<'a>(
        &self,
        tab: &crate::view_tabs::TabId,
    ) -> Option<Element<'a, Message>> {
        let target = match tab {
            crate::view_tabs::TabId::View(guid) => LockTarget::View(guid.clone()),
            crate::view_tabs::TabId::Drawing(guid) => LockTarget::Drawing(guid.clone()),
            _ => return None,
        };
        self.is_locked(&target).then(|| {
            tooltip(
                button(icon_svg(ToolIcon::Locked, 11.0))
                    .on_press(Message::Lock(target))
                    .style(flat_tool_style)
                    .padding([0, 2]),
                crate::project_browser::hint(tr("Locked: click to unlock").to_owned()),
                tooltip::Position::Bottom,
            )
            .gap(4)
            .into()
        })
    }

    /// The Update button of a saved view, dimmed while the view is locked.
    pub(crate) fn update_view_button<'a>(&self, guid: &str) -> Element<'a, Message> {
        let locked = self.view_locked(guid);
        let tip = if locked {
            tr("Locked: unlock the view to update it")
        } else {
            tr("Update to the current 3D view")
        };
        tooltip(
            button(icon_svg(ToolIcon::Update, 12.0))
                .on_press_maybe(
                    (!locked)
                        .then(|| Message::Views(crate::views::ViewAction::Update(guid.to_owned()))),
                )
                .style(flat_tool_style)
                .padding([3, 4]),
            crate::project_browser::hint(tip.to_owned()),
            tooltip::Position::Bottom,
        )
        .gap(4)
        .into()
    }

    /// A checkbox row of Properties that locks a target.
    pub(crate) fn lock_row<'a>(&self, target: LockTarget) -> Element<'a, Message> {
        let locked = self.is_locked(&target);
        let explanation = match target {
            LockTarget::View(_) => {
                tr("Orbit, zoom, walk and the section box leave the view as it is")
            }
            LockTarget::Drawing(_) => tr("The crop region, its turn and the points used stay"),
            LockTarget::Viewport { .. } => tr("It is not moved, resized, scaled or removed"),
        };
        container(
            row![
                checkbox(tr("Locked"), locked)
                    .on_toggle(move |_| Message::Lock(target.clone()))
                    .style(muted_checkbox_style)
                    .text_size(11)
                    .size(13),
                text(explanation)
                    .size(10)
                    .width(Fill)
                    .color(self.ui_theme.colors().text_muted),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
        )
        .padding([4, 8])
        .into()
    }

    /// The lock of the saved view the scene shows, in Properties.
    pub(crate) fn view_lock_properties(&self) -> Option<Element<'_, Message>> {
        if self.drawing_view.shown {
            return None;
        }
        let index = self.active_view_index()?;
        let guid = self.views.list[index].guid.clone();
        Some(
            iced::widget::column![
                opencad_properties::section_header("Saved view"),
                self.lock_row(LockTarget::View(guid)),
            ]
            .into(),
        )
    }

    /// The `lock_view` and `unlock_view` commands of the local API.
    pub(crate) fn api_lock(
        &mut self,
        options: &LockOptions,
        locked: bool,
    ) -> (Value, Task<Message>) {
        let refuse = |error: String| (json!({"ok": false, "error": error}), Task::none());
        let kind = options
            .kind
            .as_deref()
            .map(|kind| kind.trim().to_ascii_lowercase());
        let target = if options.viewport.is_some() || kind.as_deref() == Some("viewport") {
            let Some(viewport) = &options.viewport else {
                return refuse("a viewport needs viewport, its index or id".into());
            };
            let sheet = options.sheet.as_deref().or(options.name.as_deref());
            let sheet = match self.sheet_asked(sheet) {
                Ok(sheet) => sheet,
                Err(error) => return refuse(error),
            };
            match self.viewport_asked(&sheet, viewport) {
                Ok(id) => LockTarget::Viewport { sheet, id },
                Err(error) => return refuse(error),
            }
        } else {
            if kind
                .as_deref()
                .is_some_and(|kind| !matches!(kind, "view" | "drawing"))
            {
                return refuse("kind must be view, drawing or viewport".into());
            }
            let Some(name) = options.name.as_deref().map(str::trim) else {
                return refuse("give the name of a view or a drawing".into());
            };
            let wants = |wanted: &str| kind.as_deref().is_none_or(|kind| kind == wanted);
            let view = wants("view")
                .then(|| {
                    self.listed_views()
                        .into_iter()
                        .find(|view| view.name.eq_ignore_ascii_case(name))
                        .map(|view| LockTarget::View(view.guid.clone()))
                })
                .flatten();
            match view.or_else(|| {
                wants("drawing")
                    .then(|| self.drawing_named(name).map(LockTarget::Drawing))
                    .flatten()
            }) {
                Some(target) => target,
                None => {
                    return refuse(format!(
                        "no saved view of the active scan or drawing of VIEWS named {name}"
                    ))
                }
            }
        };
        let kind = match &target {
            LockTarget::View(_) => "view",
            LockTarget::Drawing(_) => "drawing",
            LockTarget::Viewport { .. } => "viewport",
        };
        match self.set_lock(&target, locked) {
            Ok(name) => {
                self.status = lock_status(&name, locked);
                (
                    json!({"ok": true, "kind": kind, "name": name, "locked": locked}),
                    Task::none(),
                )
            }
            Err(error) => refuse(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use iced::Size;
    use serde_json::{json, Value};

    use super::*;
    use crate::camera_views;
    use crate::native_api::ApiRequest;
    use crate::views::ViewAction;
    use crate::CameraPreset;

    fn studio_with_scan() -> (Studio, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        camera_views::use_test_directory(&directory.path().join("config"));
        let path = directory.path().join("hall.xyz");
        std::fs::write(&path, "0 0 0\n12 0 0\n12 8 0\n0 8 3\n").unwrap();
        let cloud = Arc::new(pointcloud_core::open(&path, 10).unwrap());
        let mut studio = Studio::default();
        let _ = studio.update(Message::Loaded(Ok(cloud)));
        (studio, directory)
    }

    fn send(studio: &mut Studio, body: Value) -> Value {
        let command: ApiCommand = serde_json::from_value(body).unwrap();
        let (reply, receive) = std::sync::mpsc::channel();
        let _ = studio.handle_api(ApiRequest { command, reply });
        receive.recv().unwrap()
    }

    fn camera(studio: &Studio) -> (f32, f32, f32, [f32; 2]) {
        (studio.yaw, studio.pitch, studio.zoom, studio.pan)
    }

    #[test]
    fn a_locked_view_keeps_its_camera_and_box_for_the_mouse_the_keys_and_the_api() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let (mut studio, _directory) = studio_with_scan();
        let _ = studio.update(Message::SetSectionEnabled(true));
        let _ = studio.update(Message::Views(ViewAction::Save));
        let guid = studio.listed_views()[0].guid.clone();
        let _ = studio.update(Message::Lock(LockTarget::View(guid.clone())));
        assert!(studio.view_locked(&guid));
        assert_eq!(studio.status, "View 1 is locked");
        let shown = camera(&studio);
        let section = studio.section_box();
        for message in [
            Message::Orbit(20.0, 5.0),
            Message::Pan(12.0, 3.0),
            Message::Zoom(1.0, [400.0, 300.0], Size::new(800.0, 600.0)),
            Message::ResetCamera,
            Message::CameraPreset(CameraPreset::Top),
            Message::CubeCorner([1, 1, 1]),
            Message::ToggleOrthographic,
            Message::KeyTyped("f".into(), false),
            Message::KeyTyped("w".into(), false),
            Message::SetSectionEnabled(false),
            Message::SectionMax(0, 50.0),
            Message::TurnSectionBy(15.0),
            Message::ResetSectionBox,
        ] {
            let _ = studio.update(message);
            assert_eq!(camera(&studio), shown);
            assert_eq!(studio.section_box(), section);
            assert!(studio.walk.is_none());
            assert_eq!(studio.status, "View 1 is locked");
        }
        for body in [
            json!({"command": "camera", "preset": "top"}),
            json!({"command": "set_camera", "yaw": 0.5, "pitch": 0.2, "zoom": 2.0, "pan": [0, 0]}),
            json!({"command": "orbit", "yaw": 0.4, "pitch": 0.1}),
            json!({"command": "zoom_all"}),
            json!({"command": "set_section", "min": [0, 0, 0], "max": [1, 1, 1]}),
            json!({"command": "clear_section"}),
            json!({"command": "rotate_crop", "degrees": 10.0}),
            json!({"command": "update_camera_view", "name": "view 1"}),
        ] {
            let answer = send(&mut studio, body.clone());
            assert_eq!(answer["error"], "View 1 is locked", "{body}");
        }
        assert_eq!(camera(&studio), shown);
        assert_eq!(studio.section_box(), section);
        // The view cannot be updated; it can be duplicated, and the copy is
        // free.
        let _ = studio.update(Message::Views(ViewAction::Update(guid.clone())));
        assert_eq!(studio.status, "View 1 is locked");
        let copy = send(
            &mut studio,
            json!({"command": "duplicate_view", "name": "View 1", "kind": "view"}),
        );
        assert_eq!(copy["ok"], true, "{copy}");
        let copy_guid = copy["guid"].as_str().unwrap().to_owned();
        assert!(!studio.view_locked(&copy_guid));
        let _ = studio.update(Message::Orbit(20.0, 5.0));
        assert_ne!(camera(&studio), shown, "the unlocked copy turns");
        // The 3D model has a camera of its own, which turns.
        let _ = studio.update(Message::Views(ViewAction::Restore(guid.clone())));
        let _ = studio.update(Message::Browser(
            crate::project_browser::BrowserAction::ShowModel,
        ));
        let before = camera(&studio);
        let _ = studio.update(Message::Orbit(20.0, 5.0));
        assert_ne!(camera(&studio), before);
        // The lock is kept, and the status reports it.
        assert!(camera_views::load()
            .iter()
            .any(|view| view.guid == guid && view.locked));
        let _ = studio.update(Message::Views(ViewAction::Restore(guid.clone())));
        let status = send(&mut studio, json!({"command": "status"}));
        assert_eq!(status["result"]["views"]["active"]["locked"], true);
        let tabs = send(&mut studio, json!({"command": "list_tabs"}));
        assert!(tabs["tabs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tab| tab["guid"] == guid.as_str() && tab["locked"] == true));
        let unlocked = send(
            &mut studio,
            json!({"command": "unlock_view", "name": "view 1"}),
        );
        assert_eq!(
            unlocked,
            json!({"ok": true, "kind": "view", "name": "View 1", "locked": false})
        );
        let _ = studio.update(Message::Orbit(20.0, 5.0));
        assert_ne!(camera(&studio), shown);
    }

    #[test]
    fn a_locked_view_behind_a_sheet_keeps_its_camera_and_box_and_its_tab_shows_it_so() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        let (mut studio, _directory) = studio_with_scan();
        // A view of its own: turned, zoomed in, with a section box.
        let _ = studio.update(Message::SetSectionEnabled(true));
        let _ = studio.update(Message::Orbit(40.0, 15.0));
        let _ = studio.update(Message::Zoom(2.0, [300.0, 200.0], Size::new(800.0, 600.0)));
        let _ = studio.update(Message::Views(ViewAction::Save));
        let guid = studio.listed_views()[0].guid.clone();
        let _ = studio.update(Message::Lock(LockTarget::View(guid.clone())));
        let shown = camera(&studio);
        let section = studio.section_box();
        assert!(section.is_some());
        // A sheet in front of the scene that holds the locked view.
        let sheet = studio
            .create_layout("01", "Plans", crate::layouts::Paper::A3, true)
            .unwrap();
        let _ = studio.show_layout(&sheet);
        assert!(studio.drawing_view.shown_layout().is_some());
        for message in [
            Message::ResetCamera,
            Message::CameraPreset(CameraPreset::Top),
            Message::ToggleOrthographic,
            Message::SetSectionEnabled(false),
            Message::ResetSectionBox,
            Message::KeyTyped("w".into(), false),
        ] {
            studio.status.clear();
            let _ = studio.update(message);
            assert_eq!(camera(&studio), shown);
            assert_eq!(studio.section_box(), section);
            assert!(studio.walk.is_none());
            assert_eq!(studio.status, "View 1 is locked");
        }
        for body in [
            json!({"command": "orbit", "yaw": 0.4, "pitch": 0.1}),
            json!({"command": "zoom_all"}),
            json!({"command": "set_section", "min": [0, 0, 0], "max": [1, 1, 1]}),
            json!({"command": "clear_section"}),
        ] {
            let answer = send(&mut studio, body.clone());
            assert_eq!(answer["error"], "View 1 is locked", "{body}");
        }
        // The sheet itself still pans and zooms.
        let _ = studio.update(Message::Layouts(crate::layouts::LayoutAction::Pan([
            10.0, 5.0,
        ])));
        assert_eq!(studio.drawing_view.shown_layout(), Some(sheet.as_str()));
        // Its tab shows the view as it was locked.
        let task = studio.show_tab(&crate::view_tabs::TabId::View(guid.clone()));
        assert!(task.is_ok());
        assert!(studio.drawing_view.shown_layout().is_none());
        assert_eq!(camera(&studio), shown);
        assert_eq!(studio.section_box(), section);
        // The 3D model holds the scene of its own, which is free.
        let _ = studio.update(Message::Browser(
            crate::project_browser::BrowserAction::ShowModel,
        ));
        let _ = studio.show_layout(&sheet);
        let before = camera(&studio);
        let _ = studio.update(Message::ResetCamera);
        assert_ne!(studio.status, "View 1 is locked");
        let _ = studio.update(Message::Orbit(20.0, 5.0));
        assert_ne!(camera(&studio), before);
    }

    #[test]
    fn a_locked_drawing_keeps_its_crop_region_and_a_locked_viewport_its_place() {
        // What it reads is in the language of the window; a test in Dutch
        // may run at the same time.
        let _language = crate::i18n::TestLanguage::hold(crate::i18n::Language::English);
        use crate::layouts::{Paper, PlacedKind};
        use pointcloud_core::{Bounds, DrawingRequest, DrawingView, OrientedBox};

        let (mut studio, _directory) = studio_with_scan();
        let sources = studio.open_sources();
        let definition = crate::saved_drawings::SavedDrawing::new(
            "Plan +1.20",
            crate::sheet_dialog::SheetKind::Plan,
            OrientedBox::new(
                Bounds {
                    min: [0.0, 0.0, 0.0],
                    max: [12.0, 8.0, 1.2],
                },
                0.0,
            ),
            &DrawingRequest::for_view(DrawingView::Plan),
            sources,
        );
        let guid = definition.guid.clone();
        studio.keep_saved_drawing(definition);
        let locked = send(
            &mut studio,
            json!({"command": "lock_view", "name": "plan +1.20"}),
        );
        assert_eq!(locked["kind"], "drawing", "{locked}");
        assert!(studio.drawing_locked(&guid));
        for body in [
            json!({"command": "set_sheet_crop", "name": "Plan +1.20", "width": 5.0}),
            json!({"command": "set_sheet_crop", "name": "Plan +1.20", "sample_percent": 50.0}),
            json!({"command": "rotate_crop", "name": "Plan +1.20", "degrees": 10.0}),
        ] {
            let answer = send(&mut studio, body.clone());
            assert_eq!(answer["ok"], false, "{body}");
            assert!(
                answer["error"]
                    .as_str()
                    .unwrap()
                    .contains("Plan +1.20 is locked"),
                "{answer}"
            );
        }
        let kept = crate::saved_drawings::load();
        assert!(kept
            .iter()
            .any(|drawing| drawing.guid == guid && drawing.locked));
        let listed = send(&mut studio, json!({"command": "list_drawings"}));
        assert_eq!(listed["drawings"][0]["locked"], true);

        // A locked viewport stays where it is and on the sheet.
        let sheet = studio
            .create_layout("01", "Plans", Paper::A3, true)
            .unwrap();
        let _ = studio.show_layout(&sheet);
        let id = studio
            .place_on_layout(
                &sheet,
                PlacedKind::Drawing,
                &guid,
                Some([150.0, 150.0]),
                None,
            )
            .unwrap();
        let answer = send(
            &mut studio,
            json!({"command": "lock_view", "kind": "viewport", "viewport": 0}),
        );
        assert_eq!(answer["kind"], "viewport", "{answer}");
        assert!(studio.move_viewport(&sheet, &id, [100.0, 100.0]).is_err());
        assert!(studio.set_viewport_scale(&sheet, &id, 50.0).is_err());
        assert!(studio.remove_viewport(&sheet, &id).is_err());
        let _ = studio.update(Message::Layouts(crate::layouts::LayoutAction::Select(
            Some(id.clone()),
        )));
        let _ = studio.update(Message::NamedKey(iced::keyboard::key::Named::Delete, true));
        let viewport = studio.layouts.list[0].viewport(&id).unwrap().clone();
        assert_eq!((viewport.centre, viewport.scale), ([150.0, 150.0], 100.0));
        assert_eq!(studio.status, "Plan +1.20 is locked");
        let listed = send(&mut studio, json!({"command": "list_sheets"}));
        assert_eq!(listed["sheets"][0]["viewports"][0]["locked"], true);
        let freed = send(
            &mut studio,
            json!({"command": "unlock_view", "sheet": "01", "viewport": 0}),
        );
        assert_eq!(freed["locked"], false);
        assert!(studio.move_viewport(&sheet, &id, [100.0, 100.0]).is_ok());
        let unknown = send(
            &mut studio,
            json!({"command": "lock_view", "name": "nothing"}),
        );
        assert_eq!(unknown["ok"], false);
    }
}
