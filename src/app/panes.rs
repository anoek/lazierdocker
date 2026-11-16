//! Window arrangement: opening, closing and arranging panes.

use super::inspect::{kind_of, tabs_for, Tab};
use super::{App, Effect, Focus, Pane, PaneKind};
use crate::ui::layout::{PaneId, SplitDir};
use crate::ui::sidebar::NodeKey;

/// Start the container user's login shell (lazydocker's trick), falling back to
/// /bin/sh when the user has no passwd entry (common with `--user 1001` images).
pub const EXEC_SHELL_SCRIPT: &str =
    r#"s=$(grep "^$(id -un 2>/dev/null):" /etc/passwd 2>/dev/null | cut -d: -f7); [ -x "$s" ] || s=/bin/sh; exec "$s""#;

impl App {
    /// Split direction for a pane: along its longer visual dimension
    /// (terminal cells are about twice as tall as wide).
    fn split_dir_for(&self, pane: PaneId) -> SplitDir {
        match self.hits.panes.iter().find(|(id, _, _)| *id == pane) {
            Some((_, r, _)) if r.width < r.height * 2 => SplitDir::Vertical,
            _ => SplitDir::Horizontal,
        }
    }

    /// Open a new pane next to the focused one and focus it.
    pub fn open_pane(&mut self, kind: PaneKind, dir: Option<SplitDir>) -> PaneId {
        let id = self.alloc_pane_id();
        let target = self.layout.focus;
        let dir = dir.unwrap_or_else(|| self.split_dir_for(target));
        self.layout.split(target, dir, id);
        self.panes.insert(id, Pane::new(kind));
        self.focus = Focus::Panes;
        self.dirty = true;
        id
    }

    /// Close a pane. The last pane is never removed; it becomes an inspector.
    pub fn close_pane(&mut self, id: PaneId) {
        if self.layout.remove(id) {
            self.panes.remove(&id);
        } else if !matches!(self.panes.get(&id).map(|p| &p.kind), Some(PaneKind::Inspector)) {
            self.panes.insert(id, Pane::new(PaneKind::Inspector));
            self.focus = Focus::Sidebar;
        }
        self.dirty = true;
    }

    /// Panes that show docker items (not terminals).
    fn is_content_pane(&self, id: PaneId) -> bool {
        matches!(self.panes.get(&id).map(|p| &p.kind), Some(PaneKind::Inspector | PaneKind::Pinned(_)))
    }

    /// The pane the sidebar drives: the focused pane, or when a terminal has
    /// focus, the previously active pane, else the first content pane.
    fn sidebar_pane(&self) -> Option<PaneId> {
        let panes = self.layout.panes();
        [Some(self.layout.focus), self.active]
            .into_iter()
            .flatten()
            .find(|id| panes.contains(id) && self.is_content_pane(*id))
            .or_else(|| panes.into_iter().find(|id| self.is_content_pane(*id)))
    }

    /// Keep exactly one pane following the sidebar: the active one. Other
    /// panes are pinned to what they show, and the sidebar highlights the
    /// active pane's item, so picking an item changes the pane you are in.
    pub fn sync_active_pane(&mut self) {
        let Some(active) = self.sidebar_pane() else { return };
        let follows = matches!(self.panes.get(&active).map(|p| &p.kind), Some(PaneKind::Inspector));
        if self.active == Some(active) && follows {
            return;
        }
        let selected = self.selected().cloned();
        for id in self.layout.panes() {
            if id == active {
                continue;
            }
            if let Some(p) = self.panes.get_mut(&id).filter(|p| matches!(p.kind, PaneKind::Inspector)) {
                if let Some(shown) = p.loaded.as_ref().map(|k| k.target.clone()).or_else(|| selected.clone()) {
                    p.kind = PaneKind::Pinned(shown);
                }
            }
        }
        if let Some(PaneKind::Pinned(key)) = self.panes.get(&active).map(|p| p.kind.clone()) {
            if self.reveal_in_sidebar(&key) {
                let rows = self.rows.clone();
                self.sidebar.select(&rows, &key);
                if let Some(p) = self.panes.get_mut(&active) {
                    p.kind = PaneKind::Inspector;
                }
            }
        }
        self.active = Some(active);
        self.dirty = true;
    }

    /// Expand the groups holding `key` so its sidebar row exists. False when
    /// it still has no row (filtered out or a hidden exited container).
    fn reveal_in_sidebar(&mut self, key: &NodeKey) -> bool {
        if !self.rows.iter().any(|r| &r.key == key) {
            self.sidebar.set_expanded(&NodeKey::Section(crate::ui::sidebar::section_of(key)), true);
            if let NodeKey::Service(p, _) = key {
                self.sidebar.set_expanded(&NodeKey::Project(p.clone()), true);
            }
            self.rebuild_rows();
        }
        self.rows.iter().any(|r| &r.key == key)
    }

    /// prefix+v / prefix+-: a new pane pinned to the current selection.
    pub fn split_focused(&mut self, dir: SplitDir) {
        let kind = match self.selected() {
            Some(key) if !matches!(key, NodeKey::Section(_)) => PaneKind::Pinned(key.clone()),
            _ => PaneKind::Inspector,
        };
        let tab = self.layout_focus_tab();
        let id = self.open_pane(kind, Some(dir));
        if let Some(tab) = tab {
            self.set_tab(id, tab);
        }
    }

    fn layout_focus_tab(&self) -> Option<Tab> {
        let id = self.primary_inspector()?;
        self.pane_tab(id)
    }

    /// `m`: a pane pinned to `key` showing its logs.
    pub fn open_logs(&mut self, key: NodeKey) {
        if !tabs_for(kind_of(&key)).contains(&Tab::Logs) {
            return;
        }
        let id = self.open_pane(PaneKind::Pinned(key), None);
        self.set_tab(id, Tab::Logs);
    }

    /// Open an interactive terminal pane running `argv`.
    pub fn open_terminal(&mut self, title: String, argv: Vec<String>) -> Vec<Effect> {
        let id = self.open_pane(PaneKind::Terminal { title }, None);
        vec![Effect::SpawnTerm { pane: id, key: None, argv }]
    }

    /// `E`: shell in the selected container (lazydocker's login-shell trick).
    pub fn exec_shell(&mut self, key: &NodeKey) -> Vec<Effect> {
        let Some(c) = self.target_container(key) else { return Vec::new() };
        if !c.is_running() {
            self.flash(format!("{} is not running", c.name));
            return Vec::new();
        }
        let (id, name) = (c.id.clone(), c.name.clone());
        let argv = vec![
            "docker".into(),
            "exec".into(),
            "-it".into(),
            id,
            "/bin/sh".into(),
            "-c".into(),
            EXEC_SHELL_SCRIPT.into(),
        ];
        self.open_terminal(format!("exec {name}"), argv)
    }

    /// `a`: attach to the selected container's main process.
    pub fn attach(&mut self, key: &NodeKey) -> Vec<Effect> {
        let Some(c) = self.target_container(key) else { return Vec::new() };
        if !c.is_running() {
            self.flash(format!("{} is not running", c.name));
            return Vec::new();
        }
        let (id, name) = (c.id.clone(), c.name.clone());
        let argv = vec!["docker".into(), "attach".into(), "--sig-proxy=false".into(), id];
        self.open_terminal(format!("attach {name} (detach: ctrl-p ctrl-q)"), argv)
    }

    pub fn focus_next_pane(&mut self, delta: i32) {
        let panes = self.layout.panes();
        if panes.is_empty() {
            return;
        }
        let i = panes.iter().position(|p| *p == self.layout.focus).unwrap_or(0) as i32;
        self.layout.focus = panes[(i + delta).rem_euclid(panes.len() as i32) as usize];
        self.focus = Focus::Panes;
        self.dirty = true;
    }

    /// Transient message in the status bar.
    pub fn flash(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), std::time::Instant::now()));
        self.dirty = true;
    }
}
