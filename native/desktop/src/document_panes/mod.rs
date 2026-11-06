//! Document ownership and native pane presentation.
pub(crate) mod layout;
pub(crate) mod persistence;
pub(crate) mod runtime;
pub(crate) mod snapshot;
pub(crate) mod store;
pub(crate) mod widget;

use crate::view_tabs::TabId;
use crate::{Message, Studio};
use iced::widget::pane_grid;
use iced::{Element, Task};
use layout::{Node, Side};

#[derive(Debug, Clone)]
pub(crate) enum Action {
    HoverTab,
    SplitCurrent(Side),
    Stored(Result<(), String>),
    SplitDocument(String, Side),
    TearOff(String),
    DockWindow(u64),
    NativeClosed(iced::window::Id),
    NativeCursor(iced::window::Id, bool),
    NativeFocused(iced::window::Id),
    NativeResized(iced::window::Id, iced::Size),
    Input(String, Box<Message>),
    Focus(u64),
    BeginDrag(String),
    Pointer(iced::window::Id, iced::Point),
    FinishDrag(iced::window::Id),
    CompleteDrag(u64),
    Resize(u64, pane_grid::ResizeEvent),
}

fn configuration(node: &Node) -> pane_grid::Configuration<u64> {
    match node {
        Node::Pane { pane } => pane_grid::Configuration::Pane(pane.id),
        Node::Split {
            axis,
            ratio,
            first,
            second,
        } => pane_grid::Configuration::Split {
            axis: match axis {
                layout::Axis::Horizontal => pane_grid::Axis::Horizontal,
                layout::Axis::Vertical => pane_grid::Axis::Vertical,
            },
            ratio: *ratio,
            a: Box::new(configuration(first)),
            b: Box::new(configuration(second)),
        },
    }
}
fn split_path(node: &pane_grid::Node, split: pane_grid::Split, path: &mut Vec<bool>) -> bool {
    if let pane_grid::Node::Split { id, a, b, .. } = node {
        if *id == split {
            return true;
        }
        path.push(false);
        if split_path(a, split, path) {
            return true;
        }
        *path.last_mut().unwrap() = true;
        if split_path(b, split, path) {
            return true;
        }
        path.pop();
    }
    false
}

impl Studio {
    pub(crate) fn update_documents(&mut self, action: Action) -> Task<Message> {
        if matches!(action, Action::HoverTab) {
            return Task::none();
        }
        if let Action::Stored(result) = action {
            self.document_persistence.saved(result);
            if let Some(error) = &self.document_persistence.error {
                self.status = error.clone();
            }
            return Task::none();
        }
        // Passive OS events never turn on document tiling.
        match &action {
            Action::NativeResized(id, size) if self.main_window.is_none_or(|main| main == *id) => {
                return self.update(Message::WindowResized(*size))
            }
            Action::NativeClosed(id) if self.main_window.is_none_or(|main| main == *id) => {
                return self.update(Message::Exit)
            }
            Action::NativeFocused(_)
            | Action::NativeResized(_, _)
            | Action::NativeClosed(_)
            | Action::NativeCursor(_, _)
            | Action::CompleteDrag(_)
                if self.documents.is_none() =>
            {
                return Task::none()
            }
            _ => {}
        }
        let (mut documents, hydration) = match self.documents.take() {
            Some(documents) => (documents, Task::none()),
            None => {
                let mut documents = runtime::Runtime::new(self);
                let hydration = documents.hydrate(self);
                (documents, hydration)
            }
        };
        if let Action::Input(key, message) = action {
            let focus = documents.focus(self, &key);
            self.documents = Some(documents);
            return match focus {
                Ok(focus) => Task::batch([hydration, focus, self.update(*message)]),
                Err(error) => {
                    self.status = error;
                    hydration
                }
            };
        }
        let result = (|| -> Result<Task<Message>, String> {
            match action {
                Action::HoverTab | Action::Stored(_) => unreachable!("handled above"),
                Action::Input(_, _) => unreachable!("scoped input is handled above"),
                Action::Focus(pane) => {
                    let key = documents
                        .workspace
                        .pane(pane)
                        .and_then(|pane| pane.active.clone())
                        .ok_or("Pane has no active document")?;
                    documents.focus(self, &key)
                }
                Action::NativeClosed(native) => {
                    let window = documents
                        .native_windows
                        .iter()
                        .find_map(|(window, id)| (*id == native).then_some(*window))
                        .ok_or("Window is unavailable")?;
                    documents.workspace.dock_window(window)?;
                    documents.native_windows.remove(&window);
                    Ok(iced::window::close(native))
                }
                Action::NativeFocused(native) => {
                    let window = documents
                        .native_windows
                        .iter()
                        .find_map(|(window, id)| (*id == native).then_some(*window))
                        .ok_or("Window is unavailable")?;
                    let mut panes = Vec::new();
                    documents
                        .workspace
                        .windows
                        .iter()
                        .find(|entry| entry.id == window)
                        .ok_or("Window is unavailable")?
                        .root
                        .panes(&mut panes);
                    let key = panes
                        .iter()
                        .find(|pane| pane.id == documents.workspace.active_pane)
                        .or_else(|| panes.first())
                        .and_then(|pane| pane.active.clone())
                        .ok_or("Pane is empty")?;
                    documents.focus(self, &key)
                }
                Action::NativeResized(_, _) => Ok(Task::none()),
                Action::NativeCursor(native, outside) => {
                    if outside {
                        documents.outside_windows.insert(native);
                    } else {
                        documents.outside_windows.remove(&native);
                    }
                    Ok(Task::none())
                }
                Action::TearOff(key) => documents.tear_off(self, &key),
                Action::DockWindow(window) => {
                    documents.workspace.dock_window(window)?;
                    let closed = documents
                        .native_windows
                        .remove(&window)
                        .map(iced::window::close)
                        .unwrap_or_else(Task::none);
                    Ok(closed)
                }
                Action::SplitDocument(key, side) => {
                    let focus = documents.focus(self, &key)?;
                    let target = documents
                        .workspace
                        .owner(&key)
                        .ok_or("Document has no pane")?;
                    documents.workspace.split_tab(&key, target, side)?;
                    documents.suspend_inactive();
                    Ok(focus)
                }
                Action::SplitCurrent(side) => {
                    let key = self
                        .shown_tab()
                        .and_then(|tab| tab.document_key())
                        .ok_or("No document is displayed")?;
                    let target = documents
                        .workspace
                        .owner(&key)
                        .ok_or("Document has no pane")?;
                    documents.workspace.split_tab(&key, target, side)?;
                    documents.suspend_inactive();
                    Ok(Task::none())
                }
                Action::Resize(window, event) => {
                    let grid = documents
                        .grids
                        .get(&window)
                        .ok_or("Window is unavailable")?;
                    let mut path = Vec::new();
                    if !split_path(grid.layout(), event.split, &mut path) {
                        return Err("Divider is unavailable".into());
                    }
                    documents
                        .workspace
                        .resize(window, &path, event.ratio.clamp(0.1, 0.9))?;
                    Ok(Task::none())
                }
                Action::BeginDrag(key) => {
                    let focus = documents.focus(self, &key)?;
                    documents.begin_drag(&key)?;
                    Ok(focus)
                }
                Action::Pointer(native, position) => {
                    documents.pointer(native, position);
                    Ok(Task::none())
                }
                Action::FinishDrag(native) => documents.release_drag(self, native),
                Action::CompleteDrag(ticket) => documents.complete_drag(self, ticket),
            }
        })();
        documents.refresh_grids();
        self.documents = Some(documents);
        match result {
            Ok(task) => Task::batch([hydration, task]),
            Err(error) => {
                self.status = error;
                hydration
            }
        }
    }

    pub(crate) fn document_grid(&self) -> Option<Element<'_, Message>> {
        self.document_grid_for(0)
    }

    pub(crate) fn document_grid_for(&self, window: u64) -> Option<Element<'_, Message>> {
        let documents = self.documents.as_ref()?;
        let grid = documents.grids.get(&window)?;
        let live = ((1u16 << documents.workspace.panes().len()) - 1) as u8;
        let pane_slots = documents.workspace.panes();
        Some(
            pane_grid::PaneGrid::new(grid, |_, id, _| {
                let pane = documents.workspace.pane(*id).expect("validated pane");
                let mut tabs = iced::widget::row![].spacing(1);
                for key in &pane.tabs {
                    if let Some(tab) = TabId::from_document_key(key) {
                        let name = self.tab_name(&tab);
                        tabs = tabs.push(self.tab_button(
                            tab,
                            name.clone(),
                            crate::view_tabs::shortened(&name, 22),
                            pane.active.as_ref() == Some(key),
                            true,
                        ));
                    }
                }
                let mut controls =
                    iced::widget::row![iced::widget::button("▥")
                        .style(crate::ui_style::tool)
                        .padding([3, 6])
                        .on_press_maybe(pane.active.clone().map(|key| Message::Documents(
                            Action::SplitDocument(key, Side::Right)
                        )),)];
                if window == 0 {
                    controls = controls.push(
                        iced::widget::button("↗")
                            .style(crate::ui_style::tool)
                            .padding([3, 6])
                            .on_press_maybe(
                                pane.active
                                    .clone()
                                    .map(|key| Message::Documents(Action::TearOff(key))),
                            ),
                    );
                } else {
                    controls = controls.push(
                        iced::widget::button("↙")
                            .style(crate::ui_style::tool)
                            .padding([3, 6])
                            .on_press(Message::Documents(Action::DockWindow(window))),
                    );
                }
                let pane_bounds = documents
                    .pane_bounds
                    .get(id)
                    .expect("validated pane bounds");
                let key = pane.active.as_ref();
                let body: Element<'_, Message> = if let Some((key, snapshot)) =
                    key.and_then(|key| documents.snapshots.get(key).map(|snapshot| (key, snapshot)))
                {
                    let slot = pane_slots.iter().position(|pane| pane.id == *id).unwrap();
                    if let (TabId::Layout(guid), Some(layout)) = (&snapshot.tab, &snapshot.layout) {
                        self.pane_layout_canvas(
                            guid,
                            layout,
                            key,
                            &snapshot.bounds,
                            pane_bounds,
                            documents.active == *key,
                        )
                        .unwrap_or_else(|| iced::widget::text("Sheet unavailable").into())
                    } else if let Some(drawing) = &snapshot.drawing {
                        let sheet =
                            drawing.pane_sheet(self.ui_theme, key, &snapshot.bounds, pane_bounds);
                        match self.pane_notes_layer(
                            drawing,
                            key,
                            &snapshot.bounds,
                            pane_bounds,
                            documents.active == *key,
                        ) {
                            Some(notes) => iced::widget::stack![sheet, notes].into(),
                            None => sheet,
                        }
                    } else {
                        let viewport = snapshot.viewport(self, slot, live);
                        iced::widget::stack![
                            crate::gpu_viewport::GpuViewport { overlay: viewport }
                                .widget()
                                .width(iced::Fill)
                                .height(iced::Fill),
                            iced::widget::canvas(widget::Scoped {
                                inner: viewport,
                                key,
                                bounds: &snapshot.bounds,
                                pane_bounds,
                            })
                            .width(iced::Fill)
                            .height(iced::Fill),
                        ]
                        .width(iced::Fill)
                        .height(iced::Fill)
                        .into()
                    }
                } else {
                    iced::widget::canvas(widget::Empty(pane_bounds))
                        .width(iced::Fill)
                        .height(iced::Fill)
                        .into()
                };
                let body = if let Some(error) = key
                    .and_then(|key| documents.snapshots.get(key))
                    .and_then(|snapshot| snapshot.load_error.as_deref())
                {
                    iced::widget::stack![
                        body,
                        iced::widget::container(iced::widget::text(error).size(12)).padding(8)
                    ]
                    .into()
                } else {
                    body
                };
                pane_grid::Content::new(body).title_bar(
                    pane_grid::TitleBar::new(tabs)
                        .controls(pane_grid::Controls::new(controls))
                        .always_show_controls()
                        .padding(2),
                )
            })
            .spacing(4)
            .width(iced::Fill)
            .height(iced::Fill)
            .on_click(|pane| Message::Documents(Action::Focus(*grid.get(pane).expect("grid pane"))))
            .on_resize(8, move |event| {
                Message::Documents(Action::Resize(window, event))
            })
            .into(),
        )
    }
}

impl Studio {
    pub(crate) fn panes_value(&self) -> serde_json::Value {
        let mut answer = match &self.documents {
            Some(documents) => serde_json::json!({"ok": true, "enabled": true,
                "workspace": documents.workspace, "active_document": documents.active,
                "documents": documents.snapshots.iter().map(|(key, snapshot)| serde_json::json!({
                    "key": key, "station_panorama_ready": snapshot.panorama_ready(), "photo_ready": snapshot.photo_ready(&self.photos), "error": snapshot.load_error,
                    "look": snapshot.stored_look(&self.photos),
                    "viewport": snapshot.bounds.get().map(|bounds| [bounds.width,bounds.height])
                })).collect::<Vec<_>>()}),
            None => serde_json::json!({"ok": true, "enabled": false,
                "workspace": layout::Workspace::default(),
                "active_document": self.shown_tab().and_then(|tab| tab.document_key())}),
        };
        answer["persistence"] = serde_json::json!({"restoring": self.document_persistence.restoring(), "error": self.document_persistence.error});
        answer
    }
    pub(crate) fn api_split_pane(&mut self, side: Side) -> (serde_json::Value, Task<Message>) {
        if self.tabs_covered() {
            return (
                serde_json::json!({"ok": false, "error": "a dialog covers the documents"}),
                Task::none(),
            );
        }
        let key = self.shown_tab().and_then(|tab| tab.document_key());
        let mut layout = match &self.documents {
            Some(documents) => documents.workspace.clone(),
            None => {
                let mut documents = runtime::Runtime::new(self);
                if let Err(error) = documents.observe_current(self) {
                    return (
                        serde_json::json!({"ok": false, "error": error}),
                        Task::none(),
                    );
                }
                documents.workspace
            }
        };
        let result = key
            .as_ref()
            .ok_or("No document is displayed".to_owned())
            .and_then(|key| {
                let target = layout.owner(key).ok_or("Document has no pane")?;
                layout.split_tab(key, target, side)
            });
        match result {
            Err(error) => (
                serde_json::json!({"ok": false, "error": error}),
                Task::none(),
            ),
            Ok(_) => {
                let task = self.update(Message::Documents(Action::SplitCurrent(side)));
                (self.panes_value(), task)
            }
        }
    }
}

impl Studio {
    pub(crate) fn active_document_window(&self) -> Option<iced::window::Id> {
        self.documents
            .as_ref()
            .and_then(|documents| {
                documents
                    .workspace
                    .window_of(documents.workspace.active_pane)
                    .and_then(|window| documents.native_windows.get(&window).copied())
            })
            .or(self.main_window)
    }
}

impl Studio {
    pub(crate) fn api_pane_window(
        &mut self,
        dock: Option<u64>,
    ) -> (serde_json::Value, Task<Message>) {
        if self.tabs_covered() {
            return (
                serde_json::json!({"ok": false, "error": "a dialog covers the documents"}),
                Task::none(),
            );
        }
        let key = self.shown_tab().and_then(|tab| tab.document_key());
        let mut layout = match &self.documents {
            Some(documents) => documents.workspace.clone(),
            None => {
                let mut documents = runtime::Runtime::new(self);
                if let Err(error) = documents.observe_current(self) {
                    return (
                        serde_json::json!({"ok": false, "error": error}),
                        Task::none(),
                    );
                }
                documents.workspace
            }
        };
        let action = match dock {
            Some(window) => layout
                .dock_window(window)
                .map(|()| Action::DockWindow(window)),
            None => key
                .ok_or("No document is displayed".into())
                .and_then(|key| layout.tear_off(&key, None).map(|_| Action::TearOff(key))),
        };
        match action {
            Err(error) => (
                serde_json::json!({"ok": false, "error": error}),
                Task::none(),
            ),
            Ok(action) => {
                let task = self.update(Message::Documents(action));
                (self.panes_value(), task)
            }
        }
    }
}
