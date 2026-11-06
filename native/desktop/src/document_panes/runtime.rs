//! Coordinate document ownership while the legacy tools operate on one focused
//! document. Inactive photo tools keep their own cancellation/cache state.
use super::layout::Workspace;
use super::snapshot::Snapshot;
use crate::file_photos::PhotoAction;
use crate::view_tabs::TabId;
use crate::{Message, Studio};
use iced::Task;
use std::cell::Cell;
use std::collections::{HashMap, HashSet};

pub(crate) struct Drag {
    pub ticket: u64,
    key: String,
    source: Option<iced::window::Id>,
    origin: iced::Point,
    moved: bool,
    pointer: Option<(iced::window::Id, iced::Point)>,
    release: Option<(iced::window::Id, Option<iced::Point>)>,
    target: Option<(iced::window::Id, iced::Point)>,
}
pub(crate) struct Runtime {
    pub drag: Option<Drag>,
    drag_serial: u64,
    pub pointers: HashMap<iced::window::Id, iced::Point>,
    pub pane_bounds: HashMap<u64, Cell<Option<iced::Rectangle>>>,
    pub workspace: Workspace,
    pub snapshots: HashMap<String, Snapshot>,
    pub active: String,
    pub outside_windows: HashSet<iced::window::Id>,
    pub native_windows: HashMap<u64, iced::window::Id>,
    pub grids: HashMap<u64, iced::widget::pane_grid::State<u64>>,
}
impl Runtime {
    pub fn new(studio: &Studio) -> Self {
        let mut workspace = Workspace::default();
        for tab in studio.listed_tabs() {
            if let Some(key) = tab.document_key() {
                let _ = workspace.open(key, 1);
            }
        }
        let tab = studio.shown_tab().unwrap_or(TabId::Model);
        let active = tab.document_key().unwrap_or_else(|| "model".into());
        let _ = workspace.open(active.clone(), 1);
        let mut snapshots = HashMap::from([("model".into(), studio.retained_model_snapshot())]);
        if active != "model" {
            snapshots.insert(active.clone(), Snapshot::capture(tab, studio));
        }
        Self {
            drag: None,
            drag_serial: 0,
            pointers: HashMap::new(),
            pane_bounds: HashMap::new(),
            workspace,
            snapshots,
            active,
            outside_windows: HashSet::new(),
            native_windows: studio
                .main_window
                .map(|id| HashMap::from([(0, id)]))
                .unwrap_or_default(),
            grids: HashMap::new(),
        }
    }
    pub fn hydrate(&mut self, studio: &mut Studio) -> Task<Message> {
        let original = self.active.clone();
        let missing: Vec<_> = self
            .workspace
            .panes()
            .into_iter()
            .flat_map(|pane| &pane.tabs)
            .filter(|key| !self.snapshots.contains_key(*key))
            .cloned()
            .collect();
        let mut tasks = Vec::new();
        for key in missing {
            match self.focus(studio, &key) {
                Ok(task) => tasks.push(task),
                Err(error) => {
                    studio.status = error;
                    let _ = self.workspace.close_tab(&key);
                }
            }
        }
        match self.focus(studio, &original) {
            Ok(task) => tasks.push(task),
            Err(error) => studio.status = error,
        }
        self.refresh_grids();
        Task::batch(tasks)
    }
    pub fn refresh_grids(&mut self) {
        let panes: HashSet<_> = self
            .workspace
            .panes()
            .into_iter()
            .map(|pane| pane.id)
            .collect();
        self.pane_bounds.retain(|id, _| panes.contains(id));
        for id in panes {
            self.pane_bounds
                .entry(id)
                .or_insert_with(|| Cell::new(None));
        }
        self.grids = self
            .workspace
            .windows
            .iter()
            .map(|window| {
                (
                    window.id,
                    iced::widget::pane_grid::State::with_configuration(super::configuration(
                        &window.root,
                    )),
                )
            })
            .collect();
    }
    pub fn begin_drag(&mut self, key: &str) -> Result<(), String> {
        let pane = self.workspace.owner(key).ok_or("Document has no pane")?;
        let source = self
            .workspace
            .window_of(pane)
            .and_then(|window| self.native_windows.get(&window).copied());
        let origin = source
            .and_then(|source| self.pointers.get(&source).copied())
            .unwrap_or(iced::Point::ORIGIN);
        self.drag_serial = self.drag_serial.wrapping_add(1);
        self.drag = Some(Drag {
            ticket: self.drag_serial,
            key: key.into(),
            source,
            origin,
            moved: false,
            pointer: source.map(|source| (source, origin)),
            release: None,
            target: None,
        });
        Ok(())
    }
    pub fn pointer(&mut self, window: iced::window::Id, position: iced::Point) {
        self.pointers.insert(window, position);
        if let Some(drag) = &mut self.drag {
            drag.pointer = Some((window, position));
            drag.moved |= drag.source != Some(window) || drag.origin.distance(position) >= 6.;
            if drag.release.is_some() && drag.target.is_none() && drag.source != Some(window) {
                drag.target = Some((window, position));
            }
        }
    }
    fn drop_target(&self, native: iced::window::Id) -> Option<(u64, Option<super::layout::Side>)> {
        self.drop_target_at(native, *self.pointers.get(&native)?)
    }
    fn drop_target_at(
        &self,
        native: iced::window::Id,
        point: iced::Point,
    ) -> Option<(u64, Option<super::layout::Side>)> {
        let window = self
            .native_windows
            .iter()
            .find_map(|(window, id)| (*id == native).then_some(*window))?;
        self.workspace
            .panes()
            .into_iter()
            .filter(|pane| self.workspace.window_of(pane.id) == Some(window))
            .find_map(|pane| {
                let bounds = self.pane_bounds.get(&pane.id)?.get()?;
                let including_header = iced::Rectangle {
                    y: bounds.y - 30.,
                    height: bounds.height + 30.,
                    ..bounds
                };
                if !including_header.contains(point) {
                    return None;
                }
                let side = if point.y < bounds.y {
                    None
                } else if point.x < bounds.x + bounds.width * 0.18 {
                    Some(super::layout::Side::Left)
                } else if point.x > bounds.x + bounds.width * 0.82 {
                    Some(super::layout::Side::Right)
                } else if point.y < bounds.y + bounds.height * 0.18 {
                    Some(super::layout::Side::Above)
                } else if point.y > bounds.y + bounds.height * 0.82 {
                    Some(super::layout::Side::Below)
                } else {
                    None
                };
                Some((pane.id, side))
            })
    }
    pub fn release_drag(
        &mut self,
        studio: &mut Studio,
        native: iced::window::Id,
    ) -> Result<Task<Message>, String> {
        let Some(drag) = &self.drag else {
            return Ok(Task::none());
        };
        if !drag.moved {
            return self.finish_drag(studio, native);
        }
        let target = drag
            .pointer
            .and_then(|(window, point)| self.drop_target_at(window, point));
        if target.is_some() {
            return self.finish_drag_to(studio, target);
        }
        let ticket = drag.ticket;
        let location = drag
            .pointer
            .map(|(window, point)| (window, Some(point)))
            .unwrap_or((native, self.pointers.get(&native).copied()));
        self.drag.as_mut().expect("drag exists").release = Some(location);
        // The compositor delivers release to the original surface during an
        // implicit pointer grab. Enter/motion for the target window follow it.
        Ok(Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                ticket
            },
            |ticket| Message::Documents(super::Action::CompleteDrag(ticket)),
        ))
    }
    pub fn complete_drag(
        &mut self,
        studio: &mut Studio,
        ticket: u64,
    ) -> Result<Task<Message>, String> {
        if self.drag.as_ref().is_none_or(|drag| drag.ticket != ticket) {
            return Ok(Task::none());
        }
        let drag = self.drag.as_ref().expect("matching drag");
        let location = drag.target.or_else(|| {
            drag.release
                .and_then(|(native, point)| point.map(|point| (native, point)))
        });
        let target = location.and_then(|(native, point)| self.drop_target_at(native, point));
        self.finish_drag_to(studio, target)
    }
    pub fn finish_drag(
        &mut self,
        studio: &mut Studio,
        native: iced::window::Id,
    ) -> Result<Task<Message>, String> {
        let target = self.drop_target(native);
        self.finish_drag_to(studio, target)
    }
    fn finish_drag_to(
        &mut self,
        studio: &mut Studio,
        target: Option<(u64, Option<super::layout::Side>)>,
    ) -> Result<Task<Message>, String> {
        let Some(drag) = self.drag.take().filter(|drag| drag.moved) else {
            return Ok(Task::none());
        };
        if let Some((target, side)) = target {
            match side {
                Some(side) => {
                    self.workspace.split_tab(&drag.key, target, side)?;
                }
                None => self.workspace.move_tab(&drag.key, target)?,
            }
            let task = self.focus(studio, &drag.key)?;
            self.suspend_inactive();
            return Ok(task);
        }
        if drag
            .source
            .is_some_and(|source| self.outside_windows.contains(&source))
        {
            return self.tear_off(studio, &drag.key);
        }
        Ok(Task::none())
    }
    pub fn capture_active(&mut self, studio: &Studio) {
        if let Some(snapshot) = self.snapshots.get_mut(&self.active) {
            // An asynchronous drawing open can already have changed the root
            // tool while the previous document still owns the runtime focus.
            if studio.shown_tab().as_ref() == Some(&snapshot.tab) {
                snapshot.capture_camera(studio);
            }
        }
    }
    pub fn observe_current(&mut self, studio: &mut Studio) -> Result<(), String> {
        let tab = studio.shown_tab().ok_or("No document is displayed")?;
        let key = tab
            .document_key()
            .ok_or("This document cannot be referenced in a pane")?;
        if key == self.active {
            if let Some(mut tool) = self
                .snapshots
                .get_mut(&key)
                .and_then(|snapshot| snapshot.photos.take())
            {
                tool.sync_metadata(&studio.photos);
                studio.photos = tool;
                studio.photos.resume();
            }
        }
        if self.workspace.owner(&key).is_none() {
            self.workspace
                .open(key.clone(), self.workspace.active_pane)?;
        }
        let owner = self
            .workspace
            .owner(&key)
            .ok_or("Document owner is missing")?;
        self.workspace
            .pane_mut(owner)
            .ok_or("Pane is missing")?
            .active = Some(key.clone());
        self.workspace.active_pane = owner;
        self.snapshots
            .entry(key.clone())
            .and_modify(|snapshot| snapshot.capture_camera(studio))
            .or_insert_with(|| Snapshot::capture(tab, studio));
        self.active = key;
        self.workspace.validate()
    }
    pub fn reconcile(&mut self, studio: &Studio) {
        let closed: Vec<_> = self
            .workspace
            .panes()
            .into_iter()
            .flat_map(|pane| &pane.tabs)
            .filter(|key| {
                *key != "model"
                    && self
                        .snapshots
                        .get(*key)
                        .is_some_and(|snapshot| !studio.tabs.open().contains(&snapshot.tab))
            })
            .cloned()
            .collect();
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| closed.contains(&drag.key))
        {
            self.drag = None;
        }
        for key in closed {
            let _ = self.workspace.close_tab(&key);
            if let Some(mut snapshot) = self.snapshots.remove(&key) {
                if let Some(photos) = &mut snapshot.photos {
                    photos.suspend(false);
                }
            }
        }
        self.suspend_inactive();
        self.refresh_grids();
    }
    pub fn tear_off(&mut self, studio: &mut Studio, key: &str) -> Result<Task<Message>, String> {
        let focus = self.focus(studio, key)?;
        let window = self.workspace.tear_off(key, None)?;
        let (native, opened) = iced::window::open(iced::window::Settings {
            size: iced::Size::new(900., 700.),
            min_size: Some(iced::Size::new(320., 240.)),
            exit_on_close_request: false,
            ..Default::default()
        });
        self.native_windows.insert(window, native);
        self.suspend_inactive();
        Ok(Task::batch([
            focus,
            opened
                .map(super::Action::NativeFocused)
                .map(Message::Documents),
        ]))
    }
    pub fn settle_windows(&mut self) -> Task<Message> {
        let gone: Vec<_> = self
            .native_windows
            .keys()
            .copied()
            .filter(|id| !self.workspace.windows.iter().any(|window| window.id == *id))
            .collect();
        let closed: Vec<_> = gone
            .into_iter()
            .filter_map(|id| self.native_windows.remove(&id))
            .collect();
        self.outside_windows
            .retain(|native| self.native_windows.values().any(|id| id == native));
        self.pointers
            .retain(|native, _| self.native_windows.values().any(|id| id == native));
        Task::batch(closed.into_iter().map(iced::window::close))
    }
    pub fn suspend_inactive(&mut self) {
        for (key, snapshot) in &mut self.snapshots {
            if let Some(photos) = &mut snapshot.photos {
                let visible = self
                    .workspace
                    .panes()
                    .iter()
                    .any(|pane| pane.active.as_ref() == Some(key));
                photos.suspend(visible);
            }
        }
    }
    pub fn detach_active_photo(&mut self, studio: &mut Studio) {
        if studio.photos.view.is_none()
            || self
                .snapshots
                .get(&self.active)
                .is_none_or(|snapshot| snapshot.photos.is_some())
        {
            return;
        }
        let mut metadata = studio.photos.metadata_copy();
        metadata.view = studio.photos.view.clone();
        metadata.blend = studio.photos.blend;
        let owned = std::mem::replace(&mut studio.photos, metadata);
        self.snapshots
            .get_mut(&self.active)
            .expect("active snapshot")
            .photos = Some(owned);
    }
    pub fn background_panorama(
        &mut self,
        source: &std::path::Path,
        station: usize,
        result: &Result<std::sync::Arc<crate::station_photos::PhotoSet>, String>,
    ) -> bool {
        self.snapshots
            .iter_mut()
            .filter(|(key, _)| **key != self.active)
            .any(|(_, snapshot)| snapshot.accept_panorama(source, station, result))
    }
    pub fn focus(&mut self, studio: &mut Studio, key: &str) -> Result<Task<Message>, String> {
        if self.active == key {
            return Ok(Task::none());
        }
        let owner = self.workspace.owner(key).ok_or("Document has no pane")?;
        let tab = self
            .snapshots
            .get(key)
            .map(|snapshot| snapshot.tab.clone())
            .or_else(|| TabId::from_document_key(key))
            .ok_or("Invalid document reference")?;
        let target_source = if let TabId::View(guid) = &tab {
            let source = &studio
                .views
                .list
                .iter()
                .find(|view| view.guid == *guid)
                .ok_or("Saved view is unavailable")?
                .source;
            Some(
                studio
                    .clouds
                    .iter()
                    .position(|entry| crate::camera_views::source_key(&entry.cloud.path) == *source)
                    .ok_or("That view's scan is not open")?,
            )
        } else {
            None
        };
        self.capture_active(studio);
        self.detach_active_photo(studio);
        let previous_source = studio.active;
        if let Some(source) = target_source {
            studio.active = Some(source);
        }
        if let Some(mut tool) = self
            .snapshots
            .get_mut(key)
            .and_then(|snapshot| snapshot.photos.take())
        {
            tool.sync_metadata(&studio.photos);
            studio.photos = tool;
        }
        let section = (
            studio.section_enabled,
            studio.section_reference_bounds,
            studio.section_min_percent,
            studio.section_max_percent,
            studio.section_rotation,
        );
        let shown = studio.show_tab_legacy(&tab);
        (
            studio.section_enabled,
            studio.section_reference_bounds,
            studio.section_min_percent,
            studio.section_max_percent,
            studio.section_rotation,
        ) = section;
        studio.sync_section_coordinate_inputs();
        let task = match shown {
            Ok(task) => task,
            Err(error) => {
                studio.active = previous_source;
                if let Some(target) = self.snapshots.get_mut(key) {
                    if studio.photos.view.is_some() {
                        let metadata = studio.photos.metadata_copy();
                        target.photos = Some(std::mem::replace(&mut studio.photos, metadata));
                    }
                }
                if let Some(previous) = self.snapshots.get_mut(&self.active) {
                    if let Some(tool) = previous.photos.take() {
                        studio.photos = tool;
                    }
                    previous.restore_camera(studio);
                }
                return Err(error);
            }
        };
        if let Some(snapshot) = self.snapshots.get(key) {
            snapshot.restore_camera(studio);
        } else {
            self.snapshots
                .insert(key.into(), Snapshot::capture(tab, studio));
        }
        self.workspace
            .pane_mut(owner)
            .ok_or("Pane is missing")?
            .active = Some(key.into());
        self.workspace.active_pane = owner;
        self.active = key.into();
        studio.photos.resume();
        let fetch = studio.fetch_photos();
        self.suspend_inactive();
        self.workspace.validate()?;
        studio.revision += 1;
        Ok(Task::batch([task, fetch, studio.schedule_detail()]))
    }
    /// Route a completed read to the document that started it, without
    /// changing the active camera, tool or document strip.
    pub fn background_photo(
        &mut self,
        studio: &mut Studio,
        action: &PhotoAction,
    ) -> Option<Task<Message>> {
        let snapshot = self.snapshots.values_mut().find(|snapshot| {
            snapshot
                .photos
                .as_ref()
                .is_some_and(|tool| tool.owns_decode(action))
        })?;
        let owned = snapshot.photos.take()?;
        let current = std::mem::replace(&mut studio.photos, owned);
        let task = studio.update_photos(action.clone());
        snapshot.photos = Some(std::mem::replace(&mut studio.photos, current));
        Some(task)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_uses_the_latest_target_motion_instead_of_a_stale_source_position() {
        let main = iced::window::Id::unique();
        let target = iced::window::Id::unique();
        let mut studio = Studio {
            main_window: Some(main),
            ..Studio::default()
        };
        let mut runtime = Runtime::new(&studio);
        runtime.workspace.open("other".into(), 1).unwrap();
        let window = runtime.workspace.tear_off("other", None).unwrap();
        runtime.native_windows.insert(window, target);
        runtime.refresh_grids();
        let target_pane = runtime.workspace.owner("other").unwrap();
        for id in [1, target_pane] {
            runtime.pane_bounds[&id].set(Some(iced::Rectangle {
                x: 0.,
                y: 30.,
                width: 800.,
                height: 570.,
            }));
        }
        runtime.pointer(main, iced::Point::new(50., 15.));
        runtime.begin_drag("model").unwrap();
        runtime.pointer(target, iced::Point::new(400., 300.));
        let _ = runtime.release_drag(&mut studio, main).unwrap();
        assert_eq!(runtime.workspace.owner("model"), Some(target_pane));
        assert!(runtime.drag.is_none());
    }

    #[test]
    fn a_late_drop_completion_does_not_finish_a_newer_drag() {
        let mut studio = Studio::default();
        let mut runtime = Runtime::new(&studio);
        runtime.begin_drag("model").unwrap();
        let old = runtime.drag.as_ref().unwrap().ticket;
        runtime.begin_drag("model").unwrap();
        let new = runtime.drag.as_ref().unwrap().ticket;
        assert_ne!(old, new);
        let _ = runtime.complete_drag(&mut studio, old).unwrap();
        assert_eq!(runtime.drag.as_ref().unwrap().ticket, new);
    }

    #[test]
    fn closed_windows_release_all_pointer_tracking() {
        let mut studio = Studio::default();
        let mut runtime = Runtime::new(&studio);
        let closed = iced::window::Id::unique();
        runtime.native_windows.insert(77, closed);
        runtime.pointers.insert(closed, iced::Point::new(20., 30.));
        runtime.outside_windows.insert(closed);
        let _ = runtime.settle_windows();
        assert!(!runtime.native_windows.contains_key(&77));
        assert!(!runtime.pointers.contains_key(&closed));
        assert!(!runtime.outside_windows.contains(&closed));
        studio.documents = Some(runtime);
        let _ = studio.update(Message::Documents(super::super::Action::Pointer(
            closed,
            iced::Point::new(10., 20.),
        )));
        assert!(!studio
            .documents
            .as_ref()
            .unwrap()
            .pointers
            .contains_key(&closed));
    }

    #[test]
    fn an_inflight_drawing_open_cannot_capture_the_previous_document_camera() {
        let mut studio = Studio {
            yaw: 0.4,
            ..Studio::default()
        };
        let mut runtime = Runtime::new(&studio);
        studio.drawing_view.shown = true;
        studio.yaw = 1.2;
        assert_ne!(studio.shown_tab(), Some(TabId::Model));
        runtime.capture_active(&studio);
        let super::super::store::Look::Scene { camera } = runtime.snapshots["model"]
            .stored_look(&studio.photos)
            .unwrap()
        else {
            panic!("model camera");
        };
        assert_eq!(camera.yaw, 0.4);
    }
}
