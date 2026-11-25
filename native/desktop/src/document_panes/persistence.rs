//! Small layout references, separate from source files and point-edit data.
use super::{layout::Workspace, runtime::Runtime, snapshot::Snapshot, store};
use crate::view_tabs::TabId;
use crate::{Message, Studio};
use iced::Task;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct Persistence {
    writer: Option<Arc<store::Writer>>,
    boot: Option<store::State>,
    restore: Option<Restore>,
    queued: Option<store::State>,
    revision: u64,
    saving: bool,
    blocked: bool,
    pub error: Option<String>,
}
struct Restore {
    state: store::State,
    workspace: Workspace,
    active: String,
    queue: VecDeque<String>,
    waiting: Option<(String, Instant)>,
    initialized: bool,
    started: Instant,
}
impl Persistence {
    pub fn new() -> Self {
        let Some(directory) = crate::preferences::config_directory() else {
            return Self::default();
        };
        Self::at(directory.join(store::FILE_NAME))
    }
    fn at(path: PathBuf) -> Self {
        match store::State::load(&path).and_then(|state| {
            if let Some(state) = &state {
                validate_documents(state)?;
            }
            Ok(state)
        }) {
            Ok(boot) => Self {
                writer: Some(Arc::new(store::Writer::new(path))),
                boot,
                ..Self::default()
            },
            Err(error) => Self {
                blocked: true,
                error: Some(format!("Saved layout was kept: {error}")),
                ..Self::default()
            },
        }
    }
    pub fn start(&mut self) -> Vec<PathBuf> {
        let Some(state) = self.boot.take() else {
            return Vec::new();
        };
        let active = state
            .workspace
            .pane(state.workspace.active_pane)
            .and_then(|pane| pane.active.clone())
            .unwrap_or_else(|| "model".into());
        let mut workspace = state.workspace.clone();
        // No monitor identifier is guessed. Unverifiable satellite placements
        // dock safely; platform inventory can retain verified screens later.
        if let Err(error) = workspace.recover_monitors(&[]) {
            self.fail(error);
            return Vec::new();
        }
        let queue = workspace
            .panes()
            .into_iter()
            .flat_map(|pane| pane.tabs.clone())
            .collect();
        let paths = state
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect();
        self.restore = Some(Restore {
            state,
            workspace,
            active,
            queue,
            waiting: None,
            initialized: false,
            started: Instant::now(),
        });
        paths
    }
    fn fail(&mut self, error: impl std::fmt::Display) {
        self.blocked = true;
        self.restore = None;
        self.error = Some(format!("Saved layout was kept: {error}"));
    }
    pub fn restoring(&self) -> bool {
        self.restore.is_some()
    }
    pub fn step(&mut self, studio: &mut Studio) -> Task<Message> {
        let Some(mut restore) = self.restore.take() else {
            return Task::none();
        };
        if restore.started.elapsed() > Duration::from_secs(180) {
            self.fail("some layout sources or documents could not be reopened");
            studio.status = self.error.clone().unwrap_or_default();
            return Task::none();
        }
        if restore.state.sources.iter().any(|source| {
            !studio
                .clouds
                .iter()
                .any(|entry| entry.cloud.path == source.path)
        }) {
            self.restore = Some(restore);
            return Task::none();
        }
        if !restore.initialized {
            for source in &restore.state.sources {
                if let Some(entry) = studio
                    .clouds
                    .iter_mut()
                    .find(|entry| entry.cloud.path == source.path)
                {
                    entry.transform.scale = source.scale;
                    entry.transform.offset = source.offset;
                    entry.visible = source.visible;
                }
            }
            let mut documents = Runtime::new(studio);
            documents.workspace = restore.workspace.clone();
            documents.snapshots.clear();
            let mut model = studio.retained_model_snapshot();
            if let Some(look) = restore.state.looks.get("model") {
                model.apply_stored(look);
            }
            model.restore_camera(studio);
            documents.snapshots.insert("model".into(), model);
            documents.active = "model".into();
            documents.refresh_grids();
            studio.documents = Some(documents);
            studio.auto_camera = None;
            restore.initialized = true;
        }
        let mut tasks = Vec::new();
        loop {
            if let Some((key, since)) = restore.waiting.take() {
                let tab = TabId::from_document_key(&key).expect("validated document");
                let ready = studio.shown_tab().as_ref() == Some(&tab)
                    && (!matches!(tab, TabId::Drawing(_) | TabId::File(_) | TabId::Layout(_))
                        || (!studio.drawing_view.is_reading()
                            && (studio.drawing_view.scene().is_some()
                                || matches!(tab, TabId::Layout(_)))));
                if !ready {
                    if since.elapsed() > Duration::from_secs(90) {
                        self.fail("a saved drawing could not be reopened");
                        studio.status = self.error.clone().unwrap_or_default();
                        return Task::batch(tasks);
                    }
                    restore.waiting = Some((key, since));
                    self.restore = Some(restore);
                    return Task::batch(tasks);
                }
                if let (
                    TabId::Photo(source),
                    Some(store::Look::Photo {
                        pinned,
                        zoom,
                        blend,
                        ..
                    }),
                ) = (&tab, restore.state.looks.get(&key))
                {
                    if let Some(view) = studio
                        .photos
                        .view
                        .as_mut()
                        .filter(|view| view.source == *source)
                    {
                        view.pinned = *pinned;
                        view.zoom = *zoom;
                        studio.photos.blend = *blend;
                    }
                }
                let mut snapshot = Snapshot::capture(tab, studio);
                if let Some(look) = restore.state.looks.get(&key) {
                    snapshot.apply_stored(look);
                }
                snapshot.restore_camera(studio);
                if let Some(mut documents) = studio.documents.take() {
                    documents.snapshots.insert(key.clone(), snapshot);
                    documents.active = key;
                    studio.documents = Some(documents);
                }
            }
            let Some(key) = restore.queue.pop_front() else {
                break;
            };
            let tab = TabId::from_document_key(&key).expect("validated document");
            if let TabId::Photo(source) = &tab {
                if !studio.photos.files.contains_key(source) {
                    restore.queue.push_front(key);
                    self.restore = Some(restore);
                    return Task::batch(tasks);
                }
            }
            prepare_look(studio, &tab, restore.state.looks.get(&key));
            let mut documents = studio.documents.take().expect("initialized documents");
            documents.capture_active(studio);
            documents.detach_active_photo(studio);
            let opened = match &tab {
                TabId::File(Some(path)) if !studio.tab_exists(&tab) => {
                    let (answer, task) = studio.api_open_drawing(path.clone());
                    if answer["ok"] == true {
                        Ok(task)
                    } else {
                        Err(answer["error"].to_string())
                    }
                }
                _ => studio.show_tab_legacy(&tab),
            };
            studio.documents = Some(documents);
            match opened {
                Ok(task) => tasks.push(task),
                Err(error) => {
                    self.fail(error);
                    studio.status = self.error.clone().unwrap_or_default();
                    return Task::batch(tasks);
                }
            }
            restore.waiting = Some((key, Instant::now()));
        }
        if let Some(section) = &restore.state.section {
            studio.section_enabled = section.enabled;
            studio.section_reference_bounds = section
                .reference
                .map(|(min, max)| pointcloud_core::Bounds { min, max });
            studio.section_min_percent = section.min;
            studio.section_max_percent = section.max;
            studio.section_rotation = section.rotation;
            studio.sync_section_coordinate_inputs();
        }
        let mut documents = studio.documents.take().expect("restored documents");
        documents.workspace = restore.workspace;
        let selected = if documents.workspace.owner(&restore.active).is_some() {
            restore.active
        } else {
            "model".into()
        };
        match documents.focus(studio, &selected) {
            Ok(task) => tasks.push(task),
            Err(error) => {
                self.fail(error);
                studio.status = self.error.clone().unwrap_or_default();
            }
        }
        documents.suspend_inactive();
        documents.refresh_grids();
        studio.documents = Some(documents);
        studio.auto_camera = None;
        studio.revision += 1;
        tasks.push(studio.schedule_detail());
        Task::batch(tasks)
    }
    pub fn queue(&mut self, studio: &Studio) -> Task<Message> {
        if self.blocked || self.restore.is_some() || self.saving {
            return Task::none();
        }
        let Some(writer) = &self.writer else {
            return Task::none();
        };
        let Some(state) = state_for(studio) else {
            return Task::none();
        };
        if self.queued.as_ref() == Some(&state) {
            return Task::none();
        }
        self.revision = self.revision.saturating_add(1);
        self.queued = Some(state.clone());
        self.saving = true;
        let revision = self.revision;
        let writer = writer.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    writer
                        .save_if_newer(revision, &state)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                })
                .await
                .unwrap_or_else(|error| Err(error.to_string()))
            },
            |result| Message::Documents(super::Action::Stored(result)),
        )
    }
    pub fn flush(&mut self, studio: &Studio) {
        if self.blocked || self.restore.is_some() {
            return;
        }
        let (Some(writer), Some(state)) = (&self.writer, state_for(studio)) else {
            return;
        };
        self.revision = self.revision.saturating_add(1);
        if let Err(error) = writer.save_if_newer(self.revision, &state) {
            self.error = Some(error.to_string());
        }
    }
    pub fn saved(&mut self, result: Result<(), String>) {
        self.saving = false;
        self.error = None;
        if let Err(error) = result {
            self.queued = None;
            self.error = Some(format!("Layout could not be saved: {error}"));
        }
    }
}
fn validate_documents(state: &store::State) -> std::io::Result<()> {
    for key in state
        .workspace
        .panes()
        .into_iter()
        .flat_map(|pane| &pane.tabs)
    {
        let tab = TabId::from_document_key(key)
            .ok_or_else(|| std::io::Error::other("invalid document reference"))?;
        if let Some(look) = state.looks.get(key) {
            let matches = matches!(
                (&tab, look),
                (TabId::Model | TabId::View(_), store::Look::Scene { .. })
                    | (TabId::Photo(_), store::Look::Photo { .. })
                    | (TabId::StationPhoto(_), store::Look::Station { .. })
                    | (
                        TabId::Drawing(_) | TabId::File(_) | TabId::Layout(_),
                        store::Look::Drawing { .. }
                    )
            );
            if !matches {
                return Err(std::io::Error::other("camera does not match its document"));
            }
        }
    }
    Ok(())
}
fn state_for(studio: &Studio) -> Option<store::State> {
    let documents = studio.documents.as_ref()?;
    let mut workspace = documents.workspace.clone();
    let ephemeral: Vec<_> = workspace
        .panes()
        .into_iter()
        .flat_map(|pane| &pane.tabs)
        .filter(|key| TabId::from_document_key(key) == Some(TabId::File(None)))
        .cloned()
        .collect();
    for key in ephemeral {
        let _ = workspace.close_tab(&key);
    }
    let looks = documents
        .snapshots
        .iter()
        .filter(|(key, _)| workspace.owner(key).is_some())
        .filter_map(|(key, snapshot)| {
            snapshot
                .stored_look(&studio.photos)
                .map(|look| (key.clone(), look))
        })
        .collect::<BTreeMap<_, _>>();
    Some(store::State {
        version: 1,
        workspace,
        looks,
        sources: studio
            .clouds
            .iter()
            .filter_map(|entry| {
                crate::view_tabs::document_path(&entry.cloud.path).map(|path| store::Source {
                    path,
                    scale: entry.transform.scale,
                    offset: entry.transform.offset,
                    visible: entry.visible,
                })
            })
            .collect(),
        section: Some(store::Section {
            enabled: studio.section_enabled,
            reference: studio
                .section_reference_bounds
                .map(|bounds| (bounds.min, bounds.max)),
            min: studio.section_min_percent,
            max: studio.section_max_percent,
            rotation: studio.section_rotation,
        }),
    })
}
fn prepare_look(studio: &mut Studio, tab: &TabId, look: Option<&store::Look>) {
    match (tab, look) {
        (
            TabId::Photo(source),
            Some(store::Look::Photo {
                index,
                camera,
                pinned,
                zoom,
                blend,
            }),
        ) => {
            studio.tabs.photo_indexes.insert(source.clone(), *index);
            studio.tabs.photo_looks.insert(
                source.clone(),
                crate::view_tabs::PhotoTabLook {
                    index: *index,
                    walk: walking(camera),
                    pinned: *pinned,
                    zoom: *zoom,
                    blend: *blend,
                },
            );
        }
        (TabId::StationPhoto(source), Some(store::Look::Station { station, camera })) => {
            studio
                .tabs
                .station_looks
                .insert(source.clone(), (*station, walking(camera)));
        }
        (TabId::View(guid), _) => {
            if let Some(view) = studio.views.list.iter().find(|view| view.guid == *guid) {
                studio.active = studio.clouds.iter().position(|entry| {
                    crate::camera_views::source_key(&entry.cloud.path) == view.source
                });
            }
        }
        _ => {}
    }
}
pub(super) fn walking(camera: &store::Walking) -> crate::station_photos::WalkView {
    crate::station_photos::WalkView {
        eye: camera.eye,
        yaw: camera.yaw,
        pitch: camera.pitch,
        field_of_view: camera.field_of_view,
    }
}

impl Studio {
    pub(crate) fn open_with_document_layout(&mut self, files: Vec<PathBuf>) -> Task<Message> {
        if let Some(error) = &self.document_persistence.error {
            self.status = error.clone();
        }
        let paths = if files.is_empty() {
            self.document_persistence.start()
        } else {
            files
        };
        self.open_paths(paths)
    }
    pub(crate) fn settle_document_restore(&mut self) -> Task<Message> {
        let mut persistence = std::mem::take(&mut self.document_persistence);
        let task = persistence.step(self);
        self.document_persistence = persistence;
        task
    }
    pub(crate) fn queue_document_layout(&mut self) -> Task<Message> {
        let mut persistence = std::mem::take(&mut self.document_persistence);
        let task = persistence.queue(self);
        self.document_persistence = persistence;
        task
    }
    pub(crate) fn flush_document_layout(&mut self) {
        let mut persistence = std::mem::take(&mut self.document_persistence);
        persistence.flush(self);
        self.document_persistence = persistence;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn application_layout_restores_camera_reference_and_shared_section() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(store::FILE_NAME);
        let mut studio = Studio {
            yaw: 0.4,
            pan: [18., -6.],
            section_enabled: true,
            section_min_percent: [10., 20., 30.],
            ..Studio::default()
        };
        studio.documents = Some(Runtime::new(&studio));
        let mut persistence = Persistence::at(path.clone());
        persistence.flush(&studio);
        let saved = store::State::load(&path).unwrap().unwrap();
        let mut reopened = Studio::default();
        let mut persistence = Persistence::at(path);
        assert!(persistence.start().is_empty());
        assert!(persistence.restoring());
        let _ = persistence.step(&mut reopened);
        assert!(!persistence.restoring(), "{:?}", persistence.error);
        assert_eq!(reopened.yaw, 0.4);
        assert_eq!(reopened.pan, [18., -6.]);
        assert_eq!(reopened.section_min_percent, [10., 20., 30.]);
        assert!(reopened.section_enabled);
        assert_eq!(
            reopened.documents.as_ref().unwrap().workspace,
            saved.workspace
        );
    }
    #[test]
    fn unsupported_layout_is_preserved_instead_of_replaced_on_exit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(store::FILE_NAME);
        let state = store::State {
            version: 999,
            ..store::State::default()
        };
        let bytes = serde_json::to_vec(&state).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let mut persistence = Persistence::at(path.clone());
        assert!(persistence.error.is_some());
        let mut studio = Studio::default();
        studio.documents = Some(Runtime::new(&studio));
        persistence.flush(&studio);
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
    #[test]
    fn a_camera_for_a_different_document_kind_is_refused_before_opening_sources() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(store::FILE_NAME);
        let mut state = store::State::default();
        state.looks.insert(
            "model".into(),
            store::Look::Station {
                station: 0,
                camera: store::Walking {
                    eye: [0., 0., 2.],
                    yaw: 0.,
                    pitch: 0.,
                    field_of_view: 1.2,
                },
            },
        );
        state.save(&path).unwrap();
        let mut persistence = Persistence::at(path);
        assert!(persistence
            .error
            .as_deref()
            .unwrap()
            .contains("camera does not match"));
        assert!(persistence.start().is_empty());
        assert!(!persistence.restoring());
    }
}
