//! Coordinate document ownership while the legacy tools operate on one focused
//! document. Inactive photo tools keep their own cancellation/cache state.
use super::layout::Workspace;
use super::snapshot::Snapshot;
use crate::file_photos::PhotoAction;
use crate::view_tabs::TabId;
use crate::{Message, Studio};
use iced::Task;
use std::collections::HashMap;

pub(crate) struct Runtime {
    pub workspace: Workspace,
    pub snapshots: HashMap<String, Snapshot>,
    pub active: String,
}
impl Runtime {
    pub fn new(studio: &Studio) -> Self {
        Self {
            workspace: Workspace::default(),
            snapshots: HashMap::from([("model".into(), Snapshot::capture(TabId::Model, studio))]),
            active: "model".into(),
        }
    }
    pub fn capture_active(&mut self, studio: &Studio) {
        if let Some(snapshot) = self.snapshots.get_mut(&self.active) {
            snapshot.capture_camera(studio);
        }
    }
    pub fn observe_current(&mut self, studio: &Studio) -> Result<(), String> {
        let tab = studio.shown_tab().ok_or("No document is displayed")?;
        let key = tab
            .document_key()
            .ok_or("This document cannot be referenced in a pane")?;
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
    fn suspend_inactive(&mut self) {
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
        self.capture_active(studio);
        if studio.photos.view.is_some() {
            let mut metadata = studio.photos.metadata_copy();
            // Keep the legacy return-camera context during this atomic focus
            // change, but never share its worker cancellation token.
            metadata.view = studio.photos.view.clone();
            metadata.blend = studio.photos.blend;
            let owned = std::mem::replace(&mut studio.photos, metadata);
            self.snapshots
                .get_mut(&self.active)
                .ok_or("Active document is missing")?
                .photos = Some(owned);
        }
        if let Some(tool) = self
            .snapshots
            .get_mut(key)
            .and_then(|snapshot| snapshot.photos.take())
        {
            studio.photos = tool;
        }
        let section = (
            studio.section_enabled,
            studio.section_reference_bounds,
            studio.section_min_percent,
            studio.section_max_percent,
            studio.section_rotation,
        );
        let shown = studio.show_tab(&tab);
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
        Ok(Task::batch([task, fetch]))
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
