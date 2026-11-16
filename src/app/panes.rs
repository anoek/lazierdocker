//! Panes: their kinds and content, and window arrangement (opening, closing
//! and arranging them).

use std::collections::HashMap;

use ratatui::text::Line;

use super::inspect::{self, kind_of, tabs_for, Tab};
use super::{App, Effect, Focus};
use crate::doc::DocSurface;
use crate::term::TermSurface;
use crate::ui::layout::{PaneId, SplitDir};
use crate::ui::sidebar::{section_of, NodeKey};

#[derive(Debug, Clone)]
pub enum PaneKind {
    /// Follows the sidebar selection.
    Inspector,
    /// Shows one item regardless of selection.
    Pinned(NodeKey),
    /// An interactive terminal (exec shell, attach, attached command).
    Terminal { title: String },
}

/// What a pane shows: a read-only document or a terminal. Positions are
/// (absolute row, display column); rows count from the top of the content.
#[derive(Debug)]
pub enum Content {
    Doc(DocSurface),
    Term(TermSurface),
}

impl Content {
    /// Top row in view.
    pub fn top(&self) -> usize {
        match self {
            Content::Doc(d) => d.scroll,
            Content::Term(t) => t.viewport_top(),
        }
    }

    /// Leftmost column in view (documents scroll sideways).
    pub fn left(&self) -> usize {
        match self {
            Content::Doc(d) => d.hscroll,
            Content::Term(_) => 0,
        }
    }

    /// Number of rows (at least 1).
    pub fn rows(&self) -> usize {
        match self {
            Content::Doc(d) => d.lines.len(),
            Content::Term(t) => t.scrollbar().0,
        }
        .max(1)
    }

    pub fn row_text(&self, row: usize) -> String {
        match self {
            Content::Doc(d) => d.line_text(row),
            Content::Term(t) => t.row_text(row),
        }
    }

    /// Text from `start` to `end` inclusive, in either order.
    pub fn text_between(&self, start: (usize, usize), end: (usize, usize)) -> String {
        match self {
            Content::Doc(d) => d.text_between(start, end),
            Content::Term(t) => t.text_between(start, end),
        }
    }

    pub fn scroll_by(&mut self, lines: i32) {
        match self {
            Content::Doc(d) => d.scroll_by(lines),
            Content::Term(t) => t.scroll_by(lines),
        }
    }

    pub fn hscroll_by(&mut self, cols: i32) {
        if let Content::Doc(d) = self {
            d.hscroll_by(cols);
        }
    }

    pub fn scroll_top(&mut self) {
        match self {
            Content::Doc(d) => d.scroll_top(),
            Content::Term(t) => t.scroll_top(),
        }
    }

    /// Scroll to the end and keep following new content.
    pub fn scroll_bottom(&mut self) {
        match self {
            Content::Doc(d) => d.scroll_bottom(),
            Content::Term(t) => t.scroll_bottom(),
        }
    }
}

#[derive(Debug)]
pub struct Pane {
    pub kind: PaneKind,
    /// Selected tab per item kind (lazydocker keeps the tab per panel).
    pub tabs: HashMap<inspect::TargetKind, inspect::Tab>,
    pub content: Content,
    pub loaded: Option<inspect::ContentKey>,
    /// Background task feeding this pane (log stream); aborted when replaced.
    pub task: Option<tokio::task::AbortHandle>,
    /// A Load for `loaded` is outstanding.
    pub inflight: bool,
    /// Tick of the last Load (periodic refresh of Top).
    pub refreshed_at: u64,
    /// The log stream for `loaded` ended cleanly at this snapshot generation;
    /// it resumes once a newer snapshot shows the container running.
    pub logs_ended_at: Option<u64>,
    /// Logs show the whole history instead of the last lines (until the
    /// pane shows something else).
    pub full_logs: bool,
    /// Restart the content on the next sync even though `loaded` is current.
    pub reload: bool,
}

impl Pane {
    pub fn new(kind: PaneKind) -> Self {
        Self {
            kind,
            tabs: HashMap::new(),
            content: Content::Doc(DocSurface::default()),
            loaded: None,
            task: None,
            inflight: false,
            refreshed_at: 0,
            logs_ended_at: None,
            full_logs: false,
            reload: false,
        }
    }

    pub fn stop_task(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }

    /// Replace the content, stopping whatever fed the old one.
    pub fn set_content(&mut self, content: Content) {
        self.stop_task();
        self.content = content;
    }

    /// Show `lines`, keeping the scroll position if a document is showing.
    pub fn show_lines(&mut self, lines: Vec<Line<'static>>) {
        match &mut self.content {
            Content::Doc(d) => d.set_lines(lines),
            Content::Term(_) => self.set_content(Content::Doc(DocSurface::new(lines))),
        }
    }
}

impl Drop for Pane {
    fn drop(&mut self) {
        self.stop_task();
    }
}

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
        self.focus_pane(id);
        id
    }

    /// Open a pane beside the focused one, on the tab the active pane shows.
    pub(super) fn open_split(&mut self, kind: PaneKind, dir: SplitDir) -> PaneId {
        let tab = self.sidebar_pane().and_then(|id| self.pane_tab(id));
        let id = self.open_pane(kind, Some(dir));
        if let Some(tab) = tab {
            self.set_tab(id, tab);
        }
        id
    }

    /// Close a pane. The last pane is never removed; it becomes an inspector.
    pub fn close_pane(&mut self, id: PaneId) {
        if self.layout.remove(id) {
            self.panes.remove(&id);
        } else if !matches!(self.panes.get(&id).map(|p| &p.kind), Some(PaneKind::Inspector)) {
            self.panes.insert(id, Pane::new(PaneKind::Inspector));
            if self.sidebar_visible {
                self.focus = Focus::Sidebar;
            }
        }
        self.dirty = true;
    }

    /// Panes that show docker items (not terminals).
    fn is_content_pane(&self, id: PaneId) -> bool {
        matches!(self.panes.get(&id).map(|p| &p.kind), Some(PaneKind::Inspector | PaneKind::Pinned(_)))
    }

    /// The pane the sidebar drives: the focused pane, or when a terminal has
    /// focus, the previously active pane, else the first content pane.
    pub(super) fn sidebar_pane(&self) -> Option<PaneId> {
        let panes = self.layout.panes();
        [Some(self.layout.focus), self.active]
            .into_iter()
            .flatten()
            .find(|id| panes.contains(id) && self.is_content_pane(*id))
            .or_else(|| panes.into_iter().find(|id| self.is_content_pane(*id)))
    }

    /// Move focus to the panes, preferring the pane the sidebar drives over a
    /// shell (so navigation keys don't land in a terminal by surprise).
    pub fn focus_inspector(&mut self) {
        match self.sidebar_pane() {
            Some(id) => self.focus_pane(id),
            None => self.focus = Focus::Panes,
        }
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
        let mut changed = self.active != Some(active);
        let selected = self.selected().cloned();
        for id in self.layout.panes() {
            if id == active {
                continue;
            }
            if let Some(p) = self.panes.get_mut(&id).filter(|p| matches!(p.kind, PaneKind::Inspector)) {
                if let Some(shown) = p.loaded.as_ref().map(|k| k.target.clone()).or_else(|| selected.clone()) {
                    p.kind = PaneKind::Pinned(shown);
                    changed = true;
                }
            }
        }
        // The active pane follows the sidebar again, unless its item has no
        // row to select (filtered out, hidden or gone): then it stays pinned.
        if let Some(PaneKind::Pinned(key)) = self.panes.get(&active).map(|p| p.kind.clone()) {
            if self.reveal_in_sidebar(&key) {
                self.select_key(&key);
                if let Some(p) = self.panes.get_mut(&active) {
                    p.kind = PaneKind::Inspector;
                }
                changed = true;
            }
        }
        self.active = Some(active);
        if changed {
            self.dirty = true;
        }
    }

    /// Expand the groups holding `key` so its sidebar row exists. False when
    /// it still has no row (filtered out or a hidden exited container).
    fn reveal_in_sidebar(&mut self, key: &NodeKey) -> bool {
        if self.rows.iter().any(|r| &r.key == key) {
            return true;
        }
        let mut opened = self.sidebar.set_expanded(&NodeKey::Section(section_of(key)), true);
        if let NodeKey::Service(p, _) = key {
            opened |= self.sidebar.set_expanded(&NodeKey::Project(p.clone()), true);
        }
        if opened {
            self.rebuild_rows();
        }
        self.rows.iter().any(|r| &r.key == key)
    }

    /// prefix+v / prefix+-: a new pane showing the current selection. It
    /// becomes the active pane; the one it was split from keeps its item.
    pub fn split_focused(&mut self, dir: SplitDir) {
        let kind = match self.selected() {
            Some(key) if !matches!(key, NodeKey::Section(_)) => PaneKind::Pinned(key.clone()),
            _ => PaneKind::Inspector,
        };
        self.open_split(kind, dir);
    }

    /// `m`: a new pane showing `key`'s logs.
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

    /// (id, name) of the running container behind `key`; flashes why not otherwise.
    fn running_container(&mut self, key: &NodeKey) -> Option<(String, String)> {
        let c = self.target_container(key)?;
        let (id, name) = (c.id.clone(), c.name.clone());
        if !c.is_running() {
            self.flash(format!("{name} is not running"));
            return None;
        }
        Some((id, name))
    }

    /// `E`: shell in the selected container (lazydocker's login-shell trick).
    pub fn exec_shell(&mut self, key: &NodeKey) -> Vec<Effect> {
        let Some((id, name)) = self.running_container(key) else { return Vec::new() };
        let argv = ["docker", "exec", "-it", &id, "/bin/sh", "-c", EXEC_SHELL_SCRIPT].map(String::from).to_vec();
        self.open_terminal(format!("exec {name}"), argv)
    }

    /// `a`: attach to the selected container's main process.
    pub fn attach(&mut self, key: &NodeKey) -> Vec<Effect> {
        let Some((id, name)) = self.running_container(key) else { return Vec::new() };
        let argv = ["docker", "attach", "--sig-proxy=false", &id].map(String::from).to_vec();
        self.open_terminal(format!("attach {name} (detach: ctrl-p ctrl-q)"), argv)
    }

    pub fn focus_next_pane(&mut self, delta: i32) {
        let panes = self.layout.panes();
        let i = panes.iter().position(|p| *p == self.layout.focus).unwrap_or(0) as i32;
        let next = panes[(i + delta).rem_euclid(panes.len() as i32) as usize];
        self.focus_pane(next);
    }
}
