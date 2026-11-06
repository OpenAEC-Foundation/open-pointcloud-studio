//! Serializable document ownership, independent of widget/window runtime IDs.
//! A document belongs to exactly one pane; removing its last tab collapses
//! that pane. The main window remains as an empty docking target when needed.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_PANES: usize = 8;
pub const MAX_TABS: usize = 33;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Above,
    Below,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}
impl Side {
    fn axis(self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Vertical,
            Self::Above | Self::Below => Axis::Horizontal,
        }
    }
    fn before(self) -> bool {
        matches!(self, Self::Left | Self::Above)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: u64,
    pub tabs: Vec<String>,
    pub active: Option<String>,
    #[serde(default)]
    pub linked: bool,
}
impl Pane {
    fn new(id: u64, tabs: Vec<String>) -> Self {
        Self {
            id,
            active: tabs.first().cloned(),
            tabs,
            linked: false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Node {
    Pane {
        pane: Pane,
    },
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}
impl Node {
    pub fn panes<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        self.collect(out);
    }
    fn collect<'a>(&'a self, out: &mut Vec<&'a Pane>) {
        match self {
            Self::Pane { pane } => out.push(pane),
            Self::Split { first, second, .. } => {
                first.collect(out);
                second.collect(out);
            }
        }
    }
    pub fn pane(&self, id: u64) -> Option<&Pane> {
        match self {
            Self::Pane { pane } => (pane.id == id).then_some(pane),
            Self::Split { first, second, .. } => first.pane(id).or_else(|| second.pane(id)),
        }
    }
    pub fn pane_mut(&mut self, id: u64) -> Option<&mut Pane> {
        match self {
            Self::Pane { pane } => (pane.id == id).then_some(pane),
            Self::Split { first, second, .. } => first.pane_mut(id).or_else(|| second.pane_mut(id)),
        }
    }
    fn split(&mut self, id: u64, side: Side, added: Pane) -> bool {
        match self {
            Self::Pane { pane } if pane.id == id => {
                let old = std::mem::replace(self, Self::Pane { pane: added });
                let new = std::mem::replace(
                    self,
                    Self::Pane {
                        pane: Pane::new(u64::MAX, vec![]),
                    },
                );
                let (first, second) = if side.before() {
                    (new, old)
                } else {
                    (old, new)
                };
                *self = Self::Split {
                    axis: side.axis(),
                    ratio: 0.5,
                    first: Box::new(first),
                    second: Box::new(second),
                };
                true
            }
            Self::Split { first, second, .. } => {
                if first.pane(id).is_some() {
                    first.split(id, side, added)
                } else {
                    second.split(id, side, added)
                }
            }
            _ => false,
        }
    }
    fn collapse(self) -> Option<Self> {
        match self {
            Self::Pane { ref pane } if pane.tabs.is_empty() => None,
            Self::Pane { .. } => Some(self),
            Self::Split {
                axis,
                ratio,
                first,
                second,
            } => match (first.collapse(), second.collapse()) {
                (Some(a), Some(b)) => Some(Self::Split {
                    axis,
                    ratio,
                    first: Box::new(a),
                    second: Box::new(b),
                }),
                (a, b) => a.or(b),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Placement {
    pub monitor: String,
    pub position: [f32; 2],
    pub size: [f32; 2],
    pub maximized: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Window {
    pub id: u64,
    pub root: Node,
    pub placement: Option<Placement>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub windows: Vec<Window>,
    pub active_pane: u64,
    next_id: u64,
}
impl Default for Workspace {
    fn default() -> Self {
        Self {
            windows: vec![Window {
                id: 0,
                root: Node::Pane {
                    pane: Pane::new(1, vec!["model".into()]),
                },
                placement: None,
            }],
            active_pane: 1,
            next_id: 2,
        }
    }
}

impl Workspace {
    pub fn panes(&self) -> Vec<&Pane> {
        let mut panes = Vec::new();
        for window in &self.windows {
            window.root.collect(&mut panes);
        }
        panes
    }
    pub fn pane(&self, id: u64) -> Option<&Pane> {
        self.windows.iter().find_map(|w| w.root.pane(id))
    }
    pub fn pane_mut(&mut self, id: u64) -> Option<&mut Pane> {
        self.windows.iter_mut().find_map(|w| w.root.pane_mut(id))
    }
    pub fn owner(&self, tab: &str) -> Option<u64> {
        self.panes()
            .into_iter()
            .find(|p| p.tabs.iter().any(|s| s == tab))
            .map(|p| p.id)
    }
    pub fn window_of(&self, pane: u64) -> Option<u64> {
        self.windows
            .iter()
            .find(|w| w.root.pane(pane).is_some())
            .map(|w| w.id)
    }
    fn allocate(&mut self) -> Result<u64, String> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("Workspace identifiers exhausted")?;
        Ok(id)
    }
    fn transaction<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        self.validate()?;
        let mut changed = self.clone();
        let result = operation(&mut changed)?;
        changed.validate()?;
        *self = changed;
        Ok(result)
    }
    pub fn open(&mut self, tab: String, pane: u64) -> Result<(), String> {
        self.transaction(|work| {
            if let Some(owner) = work.owner(&tab) {
                work.active_pane = owner;
                work.pane_mut(owner).unwrap().active = Some(tab);
            } else {
                if tab.is_empty() || tab.len() > 32768 || tab.contains('\0') {
                    return Err("Invalid document identifier".into());
                }
                let target = work.pane_mut(pane).ok_or("Pane is unavailable")?;
                target.tabs.push(tab.clone());
                target.active = Some(tab);
                work.active_pane = pane;
            }
            Ok(())
        })
    }
    pub fn move_tab(&mut self, tab: &str, target: u64) -> Result<(), String> {
        self.transaction(|work| work.move_inner(tab, target))
    }
    fn move_inner(&mut self, tab: &str, target: u64) -> Result<(), String> {
        self.pane(target).ok_or("Target pane is unavailable")?;
        let owner = self.owner(tab).ok_or("Document is unavailable")?;
        if owner != target {
            self.remove_inner(tab)?;
            let pane = self.pane_mut(target).ok_or("Target pane is unavailable")?;
            pane.tabs.push(tab.into());
            pane.active = Some(tab.into());
            self.collapse_empty();
        }
        self.active_pane = target;
        self.pane_mut(target).unwrap().active = Some(tab.into());
        Ok(())
    }
    fn remove_inner(&mut self, tab: &str) -> Result<(), String> {
        let owner = self.owner(tab).ok_or("Document is unavailable")?;
        let pane = self.pane_mut(owner).unwrap();
        let index = pane.tabs.iter().position(|key| key == tab).unwrap();
        pane.tabs.remove(index);
        if pane.active.as_deref() == Some(tab) {
            pane.active = pane
                .tabs
                .get(index.min(pane.tabs.len().saturating_sub(1)))
                .cloned();
        }
        Ok(())
    }
    pub fn close_tab(&mut self, tab: &str) -> Result<(), String> {
        if tab == "model" {
            return Err("The 3D model tab stays open".into());
        }
        self.transaction(|work| {
            work.remove_inner(tab)?;
            work.collapse_empty();
            Ok(())
        })
    }
    pub fn split_tab(&mut self, tab: &str, target: u64, side: Side) -> Result<u64, String> {
        self.transaction(|work| {
            let window = work.window_of(target).ok_or("Target pane is unavailable")?;
            let owner = work.owner(tab).ok_or("Document is unavailable")?;
            if owner == target && work.pane(target).unwrap().tabs.len() == 1 {
                return Err("Splitting needs another tab".into());
            }
            work.remove_inner(tab)?;
            let id = work.allocate()?;
            let added = Pane::new(id, vec![tab.into()]);
            assert!(work
                .windows
                .iter_mut()
                .find(|w| w.id == window)
                .unwrap()
                .root
                .split(target, side, added));
            work.active_pane = id;
            work.collapse_empty();
            Ok(id)
        })
    }
    pub fn resize(&mut self, window: u64, path: &[bool], ratio: f32) -> Result<(), String> {
        self.transaction(|work| {
            if !ratio.is_finite() || !(0.1..=0.9).contains(&ratio) {
                return Err("Divider ratio must be between 0.1 and 0.9".into());
            }
            let mut node = &mut work
                .windows
                .iter_mut()
                .find(|w| w.id == window)
                .ok_or("Window is unavailable")?
                .root;
            for second_child in path {
                node = match node {
                    Node::Split { first, second, .. } => {
                        if *second_child {
                            second
                        } else {
                            first
                        }
                    }
                    Node::Pane { .. } => return Err("Divider is unavailable".into()),
                };
            }
            match node {
                Node::Split { ratio: value, .. } => *value = ratio,
                Node::Pane { .. } => return Err("Divider is unavailable".into()),
            }
            Ok(())
        })
    }

    pub fn tear_off(&mut self, tab: &str, placement: Option<Placement>) -> Result<u64, String> {
        self.transaction(|work| {
            work.remove_inner(tab)?;
            let window = work.allocate()?;
            let pane = work.allocate()?;
            work.windows.push(Window {
                id: window,
                root: Node::Pane {
                    pane: Pane::new(pane, vec![tab.into()]),
                },
                placement,
            });
            work.active_pane = pane;
            work.collapse_empty();
            Ok(window)
        })
    }
    pub fn dock_window(&mut self, id: u64) -> Result<(), String> {
        if id == 0 {
            return Err("The main window cannot dock into itself".into());
        }
        self.transaction(|work| {
            let position = work
                .windows
                .iter()
                .position(|w| w.id == id)
                .ok_or("Window is unavailable")?;
            let window = work.windows.remove(position);
            let mut panes = Vec::new();
            window.root.collect(&mut panes);
            let tabs: Vec<_> = panes.into_iter().flat_map(|p| p.tabs.clone()).collect();
            let main = work.windows.iter().find(|w| w.id == 0).unwrap();
            let mut main_panes = Vec::new();
            main.root.collect(&mut main_panes);
            let target = main_panes[0].id;
            let pane = work.pane_mut(target).unwrap();
            pane.tabs.extend(tabs);
            pane.active = pane.tabs.last().cloned();
            work.active_pane = target;
            Ok(())
        })
    }
    /// A missing or unverifiable screen returns every document to the main
    /// window. Runtime code supplies platform monitor identifiers.
    #[cfg(test)]
    pub fn recover_monitors(&mut self, available: &[String]) -> Result<(), String> {
        let missing: Vec<_> = self
            .windows
            .iter()
            .filter(|w| {
                w.id != 0
                    && w.placement
                        .as_ref()
                        .is_none_or(|p| !available.contains(&p.monitor))
            })
            .map(|w| w.id)
            .collect();
        for id in missing {
            self.dock_window(id)?;
        }
        Ok(())
    }
    fn collapse_empty(&mut self) {
        self.windows = self
            .windows
            .drain(..)
            .filter_map(|window| {
                let root = window.root.collapse();
                match root {
                    Some(root) => Some(Window { root, ..window }),
                    None if window.id == 0 => Some(Window {
                        id: 0,
                        root: Node::Pane {
                            pane: Pane::new(1, vec![]),
                        },
                        placement: window.placement,
                    }),
                    None => None,
                }
            })
            .collect();
        if self.pane(self.active_pane).is_none() {
            self.active_pane = self.panes()[0].id;
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.windows.first().map(|w| w.id) != Some(0) {
            return Err("Main window is missing".into());
        }
        let panes = self.panes();
        if panes.is_empty() || panes.len() > MAX_PANES {
            return Err("Invalid pane count".into());
        }
        let mut ids = HashSet::new();
        let mut tabs = HashSet::new();
        for window in &self.windows {
            if !ids.insert(window.id) {
                return Err("Duplicate workspace identifier".into());
            }
            if let Some(p) = &window.placement {
                if p.monitor.is_empty()
                    || p.position.iter().chain(&p.size).any(|v| !v.is_finite())
                    || p.size.iter().any(|v| *v < 100. || *v > 32768.)
                {
                    return Err("Invalid window placement".into());
                }
            }
            fn splits(node: &Node) -> bool {
                match node {
                    Node::Pane { .. } => true,
                    Node::Split {
                        ratio,
                        first,
                        second,
                        ..
                    } => {
                        ratio.is_finite()
                            && (0.1..=0.9).contains(ratio)
                            && splits(first)
                            && splits(second)
                    }
                }
            }
            if !splits(&window.root) {
                return Err("Invalid pane divider".into());
            }
        }
        for pane in panes {
            if !ids.insert(pane.id) {
                return Err("Duplicate workspace identifier".into());
            }
            if pane.tabs.is_empty()
                && !matches!(&self.windows[0].root, Node::Pane { pane: root } if root.id==pane.id)
            {
                return Err("Empty satellite pane".into());
            }
            if pane
                .active
                .as_ref()
                .is_some_and(|tab| !pane.tabs.contains(tab))
                || (!pane.tabs.is_empty() && pane.active.is_none())
            {
                return Err("Active document is missing".into());
            }
            for tab in &pane.tabs {
                if tab.is_empty() || tab.len() > 32768 || tab.contains('\0') || !tabs.insert(tab) {
                    return Err("Invalid or duplicate document".into());
                }
            }
        }
        if tabs.len() > MAX_TABS || !tabs.contains(&"model".to_owned()) {
            return Err("Invalid document count".into());
        }
        if self.pane(self.active_pane).is_none() {
            return Err("Active pane is missing".into());
        }
        if ids.iter().any(|id| *id >= self.next_id) {
            return Err("Workspace identifier sequence is invalid".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_grid_moves_documents_without_duplication_and_survives_json() {
        let mut w = Workspace::default();
        for tab in ["plan", "elevation", "photo"] {
            w.open(tab.into(), 1).unwrap();
        }
        let right = w.split_tab("photo", 1, Side::Right).unwrap();
        w.split_tab("elevation", right, Side::Below).unwrap();
        w.split_tab("plan", 1, Side::Below).unwrap();
        assert_eq!(w.panes().len(), 4);
        assert_eq!(w.owner("model"), Some(1));
        let saved = serde_json::to_string(&w).unwrap();
        assert_eq!(serde_json::from_str::<Workspace>(&saved).unwrap(), w);
        w.validate().unwrap();
    }
    #[test]
    fn moving_the_last_tab_collapses_a_pane_and_keeps_the_remaining_ids() {
        let mut w = Workspace::default();
        w.open("photo".into(), 1).unwrap();
        let right = w.split_tab("photo", 1, Side::Right).unwrap();
        w.move_tab("photo", 1).unwrap();
        assert_eq!(w.panes().len(), 1);
        assert!(w.pane(right).is_none());
        assert_eq!(w.pane(1).unwrap().tabs, ["model", "photo"]);
    }
    #[test]
    fn tear_off_dock_and_missing_monitors_keep_every_document() {
        let mut w = Workspace::default();
        w.open("photo".into(), 1).unwrap();
        let window = w
            .tear_off(
                "photo",
                Some(Placement {
                    monitor: "second".into(),
                    position: [1920., 0.],
                    size: [1000., 700.],
                    maximized: false,
                }),
            )
            .unwrap();
        w.recover_monitors(&["first".into(), "second".into()])
            .unwrap();
        assert_eq!(w.windows.len(), 2);
        w.recover_monitors(&["first".into()]).unwrap();
        assert_eq!(w.windows.len(), 1);
        assert_eq!(w.owner("photo"), Some(1));
        assert!(w.dock_window(window).is_err());
    }
    #[test]
    fn refused_operations_leave_the_whole_layout_unchanged() {
        let mut w = Workspace::default();
        let before = w.clone();
        assert!(w.split_tab("model", 1, Side::Right).is_err());
        assert!(w.move_tab("model", 999).is_err());
        assert!(w.close_tab("model").is_err());
        assert_eq!(w, before);
    }
    #[test]
    fn the_main_window_remains_a_docking_target_when_its_last_tab_tears_off() {
        let mut w = Workspace::default();
        let window = w.tear_off("model", None).unwrap();
        assert_eq!(w.windows.len(), 2);
        assert!(w.pane(1).unwrap().tabs.is_empty());
        w.dock_window(window).unwrap();
        assert_eq!(w.pane(1).unwrap().tabs, ["model"]);
    }
    #[test]
    fn a_full_workspace_can_rearrange_existing_panes_but_refuses_a_ninth() {
        let mut w = Workspace::default();
        for index in 0..7 {
            w.open(format!("doc{index}"), 1).unwrap();
        }
        for index in 0..7 {
            w.split_tab(&format!("doc{index}"), 1, Side::Right).unwrap();
        }
        assert_eq!(w.panes().len(), 8);
        let target = w.owner("doc1").unwrap();
        w.split_tab("doc0", target, Side::Above).unwrap();
        assert_eq!(w.panes().len(), 8);
        w.open("extra".into(), 1).unwrap();
        let before = w.clone();
        assert!(w.split_tab("extra", 1, Side::Below).is_err());
        assert_eq!(w, before);
    }
    #[test]
    fn dividers_are_persisted_and_invalid_changes_are_atomic() {
        let mut w = Workspace::default();
        w.open("plan".into(), 1).unwrap();
        w.split_tab("plan", 1, Side::Below).unwrap();
        w.resize(0, &[], 0.7).unwrap();
        let before = w.clone();
        assert!(w.resize(0, &[], f32::NAN).is_err());
        assert!(w.resize(0, &[true], 0.5).is_err());
        assert_eq!(w, before);
        assert_eq!(
            serde_json::from_str::<Workspace>(&serde_json::to_string(&w).unwrap()).unwrap(),
            w
        );
    }
    #[test]
    fn malformed_restored_state_is_refused_without_panicking_or_mutating() {
        let mut w = Workspace::default();
        w.windows.clear();
        let before = w.clone();
        assert!(w.open("photo".into(), 1).is_err());
        assert_eq!(w, before);
    }
}
