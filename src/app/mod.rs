//! Application state. Pure data: no terminal, no docker connection.
//!
//! Input handlers mutate state and return [`Effect`]s; the runtime executes
//! effects (docker calls, spawning processes, clipboard) and feeds results back
//! as [`AppEvent`]s.

pub mod input;
pub mod inspect;
pub mod loader;
pub mod render;
pub mod run;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::doc::DocSurface;
use crate::docker::model::Snapshot;
use crate::event::AppEvent;
use crate::ui::layout::{Layout, PaneId, SplitBorder, SplitDir};
use crate::ui::sidebar::{NodeKey, Row, Sidebar, SidebarHits};
use crate::ui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Panes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Prefix key pressed; next key is a window command.
    Prefix,
    /// Editing the sidebar filter.
    Filter,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    SidebarWidth,
    Split { path: Vec<bool>, dir: SplitDir, area: Rect, grab_offset: i32 },
}

#[derive(Debug, Clone, Default)]
pub struct Hits {
    pub sidebar: SidebarHits,
    pub sidebar_area: Rect,
    pub sidebar_divider: Option<Rect>,
    pub pane_area: Rect,
    /// (pane, outer rect, inner content rect)
    pub panes: Vec<(PaneId, Rect, Rect)>,
    pub splits: Vec<SplitBorder>,
    pub tabs: Vec<(Rect, PaneId, inspect::Tab)>,
    /// Content rect (below the tab bar) per pane.
    pub content: Vec<(PaneId, Rect)>,
}

/// Work for the runtime. Kept as data so input handling is testable.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Quit,
    RefreshNow,
    Load { pane: PaneId, key: inspect::ContentKey, spec: inspect::LoadSpec },
}

#[derive(Debug, Clone)]
pub enum PaneKind {
    /// Follows the sidebar selection.
    Inspector,
    /// Shows one item regardless of selection.
    Pinned(NodeKey),
}

#[derive(Debug)]
pub enum Content {
    Doc(DocSurface),
}

#[derive(Debug)]
pub struct Pane {
    pub kind: PaneKind,
    /// Selected tab per item kind (lazydocker keeps the tab per panel).
    pub tabs: HashMap<inspect::TargetKind, inspect::Tab>,
    pub content: Content,
    pub loaded: Option<inspect::ContentKey>,
}

impl Pane {
    pub fn new(kind: PaneKind) -> Self {
        Self { kind, tabs: HashMap::new(), content: Content::Doc(DocSurface::default()), loaded: None }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Click {
    pub at: Instant,
    pub col: u16,
    pub row: u16,
}

pub const DOUBLE_CLICK: Duration = Duration::from_millis(350);
pub const MIN_FRAME: Duration = Duration::from_millis(16);

pub struct App {
    pub theme: Theme,
    pub host: String,
    pub snap: Snapshot,
    /// Increments on every snapshot (summary content revision).
    pub snap_generation: u64,
    pub loaded: bool,
    pub docker_error: Option<String>,
    pub sidebar: Sidebar,
    pub sidebar_visible: bool,
    pub focus: Focus,
    pub mode: Mode,
    pub layout: Layout,
    pub panes: HashMap<PaneId, Pane>,
    next_pane: u64,
    pub hits: Hits,
    pub drag: Option<Drag>,
    pub last_click: Option<Click>,
    pub cpu: HashMap<String, f64>,
    pub size: (u16, u16),
    pub dirty: bool,
    pub quit: bool,
    /// Cached flattened sidebar rows, rebuilt when state changes.
    rows: Vec<Row>,
}

impl App {
    pub fn new(host: impl Into<String>) -> Self {
        let first = PaneId(1);
        let mut panes = HashMap::new();
        panes.insert(first, Pane::new(PaneKind::Inspector));
        let mut app = Self {
            theme: Theme::default(),
            host: host.into(),
            snap: Snapshot::default(),
            snap_generation: 0,
            loaded: false,
            docker_error: None,
            sidebar: Sidebar::default(),
            sidebar_visible: true,
            focus: Focus::Sidebar,
            mode: Mode::Normal,
            layout: Layout::new(first),
            panes,
            next_pane: 2,
            hits: Hits::default(),
            drag: None,
            last_click: None,
            cpu: HashMap::new(),
            size: (80, 24),
            dirty: true,
            quit: false,
            rows: Vec::new(),
        };
        app.rebuild_rows();
        app
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn rebuild_rows(&mut self) {
        self.rows = self.sidebar.rows(&self.snap);
        self.sidebar.reconcile(&self.rows);
        // Start on the first item rather than a section header.
        if !self.loaded {
            return;
        }
        self.dirty = true;
    }

    pub fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane += 1;
        id
    }

    pub fn selected(&self) -> Option<&NodeKey> {
        self.sidebar.selected.as_ref()
    }

    pub fn handle_app_event(&mut self, ev: AppEvent) -> Vec<Effect> {
        match ev {
            AppEvent::Snapshot(s) => {
                let first_load = !self.loaded;
                self.snap = *s;
                self.snap_generation += 1;
                self.loaded = true;
                self.docker_error = None;
                self.cpu.retain(|id, _| self.snap.containers.iter().any(|c| &c.id == id));
                self.rebuild_rows();
                if first_load {
                    // Select the first leaf so the inspector has something to show.
                    if let Some(i) = self.rows.iter().position(|r| r.depth > 0) {
                        let key = self.rows[i].key.clone();
                        self.sidebar.select(&self.rows, &key);
                    }
                }
                self.dirty = true;
            }
            AppEvent::DockerError(e) => {
                self.docker_error = Some(e);
                self.dirty = true;
            }
            AppEvent::Doc { pane, key, lines } => {
                if let Some(p) = self.panes.get_mut(&pane) {
                    if p.loaded.as_ref() == Some(&key) {
                        match &mut p.content {
                            Content::Doc(d) => d.set_lines(lines),
                        }
                        self.dirty = true;
                    }
                }
            }
        }
        Vec::new()
    }
}

#[cfg(test)]
pub(crate) mod tests;
