//! Actions: what menu items, confirmations and context menus do.

use super::overlay::{Menu, MenuItem, Overlay};
use super::{App, Effect, Focus, PaneKind};
use crate::ui::layout::{PaneId, SplitDir};
use crate::ui::sidebar::NodeKey;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    None,
    Quit,
    OpenLogs(NodeKey),
    Exec(NodeKey),
    Attach(NodeKey),
    /// Open a pane pinned to the item.
    Pin(NodeKey, SplitDir),
    ToggleCollapse(NodeKey),
    SplitPane(PaneId, SplitDir),
    ClosePane(PaneId),
    ZoomPane(PaneId),
    /// Turn a pinned pane back into a follow-the-selection inspector.
    Unpin(PaneId),
    Copy(String),
    Help,
}

impl App {
    pub fn perform(&mut self, action: Action) -> Vec<Effect> {
        self.dirty = true;
        match action {
            Action::None => Vec::new(),
            Action::Quit => vec![Effect::Quit],
            Action::OpenLogs(k) => {
                self.open_logs(k);
                Vec::new()
            }
            Action::Exec(k) => self.exec_shell(&k),
            Action::Attach(k) => self.attach(&k),
            Action::Pin(k, dir) => {
                let tab = self.primary_inspector().and_then(|id| self.pane_tab(id));
                let id = self.open_pane(PaneKind::Pinned(k), Some(dir));
                if let Some(t) = tab {
                    self.set_tab(id, t);
                }
                Vec::new()
            }
            Action::ToggleCollapse(k) => {
                self.sidebar.toggle(&k);
                self.rebuild_rows();
                Vec::new()
            }
            Action::SplitPane(id, dir) => {
                self.layout.focus = id;
                self.split_focused(dir);
                Vec::new()
            }
            Action::ClosePane(id) => {
                self.close_pane(id);
                Vec::new()
            }
            Action::ZoomPane(id) => {
                self.layout.focus = id;
                self.focus = Focus::Panes;
                self.layout.toggle_zoom();
                Vec::new()
            }
            Action::Unpin(id) => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.kind = PaneKind::Inspector;
                    p.loaded = None;
                }
                Vec::new()
            }
            Action::Copy(text) => vec![Effect::Copy(text)],
            Action::Help => {
                self.overlay = Some(Overlay::Help { filter: String::new(), scroll: 0 });
                Vec::new()
            }
        }
    }

    /// Context menu for a sidebar item (right-click).
    pub fn item_menu(&self, key: &NodeKey) -> Menu {
        let mut items = Vec::new();
        let row = self.rows().iter().find(|r| &r.key == key);
        if let Some(r) = row.filter(|r| r.expandable) {
            let label = if r.expanded { "Collapse" } else { "Expand" };
            items.push(MenuItem::new(label, Action::ToggleCollapse(key.clone())).key("space"));
        }
        if !matches!(key, NodeKey::Section(_)) {
            if self.target_container(key).is_some_and(|c| c.is_running()) {
                items.push(MenuItem::new("Exec shell", Action::Exec(key.clone())).key("E"));
                items.push(MenuItem::new("Attach", Action::Attach(key.clone())).key("a"));
            }
            if matches!(key, NodeKey::Container(_) | NodeKey::Service(..) | NodeKey::Project(_)) {
                items.push(MenuItem::new("Logs in new pane", Action::OpenLogs(key.clone())).key("m"));
            }
            items.push(MenuItem::new("Open in split →", Action::Pin(key.clone(), SplitDir::Horizontal)));
            items.push(MenuItem::new("Open in split ↓", Action::Pin(key.clone(), SplitDir::Vertical)));
        }
        items.extend(self.docker_menu_items(key));
        items.push(MenuItem::new("Copy name", Action::Copy(self.copy_name(key))));
        Menu::new(self.target_label(key), items)
    }

    fn copy_name(&self, key: &NodeKey) -> String {
        match key {
            NodeKey::Container(id) => self.snap.container(id).map(|c| c.name.clone()).unwrap_or_default(),
            NodeKey::Service(_, s) => s.clone(),
            NodeKey::Project(p) => p.clone(),
            NodeKey::Image(id) => {
                self.snap.images.iter().find(|i| &i.id == id).map(|i| i.display()).unwrap_or_default()
            }
            NodeKey::Volume(v) => v.clone(),
            NodeKey::Network(id) => {
                self.snap.networks.iter().find(|n| &n.id == id).map(|n| n.name.clone()).unwrap_or_default()
            }
            NodeKey::Section(s) => s.title().to_string(),
        }
    }

    /// Context menu for a pane (right-click on its frame or content).
    pub fn pane_menu(&self, id: PaneId) -> Menu {
        let mut items = vec![
            MenuItem::new("Split right", Action::SplitPane(id, SplitDir::Horizontal)).key("^b v"),
            MenuItem::new("Split down", Action::SplitPane(id, SplitDir::Vertical)).key("^b -"),
        ];
        if self.layout.panes().len() > 1 {
            let label = if self.layout.zoomed { "Unzoom" } else { "Zoom" };
            items.push(MenuItem::new(label, Action::ZoomPane(id)).key("^b z"));
        }
        if matches!(self.panes.get(&id).map(|p| &p.kind), Some(PaneKind::Pinned(_))) {
            items.push(MenuItem::new("Follow selection", Action::Unpin(id)));
        }
        items.push(MenuItem::new("Close pane", Action::ClosePane(id)).key("^b x").danger());
        Menu::new(self.pane_title(id), items)
    }
}

impl App {
    /// Docker operations offered for an item (filled in with lazydocker's actions).
    pub fn docker_menu_items(&self, _key: &NodeKey) -> Vec<MenuItem> {
        Vec::new()
    }
}
