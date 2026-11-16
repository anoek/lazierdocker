//! Application state. Pure data: no terminal, no docker connection.
//!
//! Input handlers mutate state and return [`Effect`]s. The runtime (`run`)
//! executes them: it starts docker calls, processes and streams, installs the
//! content they create into panes, and feeds later results back as
//! [`AppEvent`]s.

pub mod actions;
pub mod copy_mode;
pub mod docker_actions;
pub mod find;
pub mod input;
pub mod inspect;
pub mod loader;
pub mod logs;
pub mod overlay;
pub mod panes;
pub mod render;
pub mod run;
pub mod selection;

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};

pub use panes::{Content, Pane, PaneKind};

use crate::docker::logs::LogTime;
use crate::docker::model::Snapshot;
use crate::docker::ops::{is_must_stop, DockerOp};
use crate::docker::stats::StatsHistory;
use crate::event::AppEvent;
use crate::term::TermSurface;
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
    /// Resizing the focused pane with the keyboard.
    Resize,
    /// Keyboard selection in the focused pane.
    Copy,
    /// Typing a find query for the focused logs pane.
    Find,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Drag {
    SidebarWidth,
    Split {
        path: Vec<bool>,
        dir: SplitDir,
        area: Rect,
        grab_offset: i32,
    },
    /// Mouse gesture forwarded to a terminal pane's program.
    TermMouse(PaneId),
    /// Selecting text in a pane.
    Select(PaneId),
}

#[derive(Debug, Clone, Default)]
pub struct Hits {
    pub sidebar: SidebarHits,
    pub sidebar_area: Rect,
    pub sidebar_divider: Option<Rect>,
    pub pane_area: Rect,
    /// (pane, outer rect, rect inside its frame)
    pub panes: Vec<(PaneId, Rect, Rect)>,
    pub splits: Vec<SplitBorder>,
    pub tabs: Vec<(Rect, PaneId, inspect::Tab)>,
    /// Content rect (below the tab bar) per pane.
    pub content: Vec<(PaneId, Rect)>,
}

impl Hits {
    /// A pane's content rect at the last render.
    pub fn content_rect(&self, pane: PaneId) -> Option<Rect> {
        self.content.iter().find(|(p, _)| *p == pane).map(|(_, r)| *r)
    }

    /// The pane whose frame is under the pointer.
    pub fn pane_at(&self, col: u16, row: u16) -> Option<PaneId> {
        self.panes.iter().find(|(_, r, _)| r.contains(Position::new(col, row))).map(|(id, _, _)| *id)
    }

    /// The pane whose content is under the pointer.
    pub fn content_at(&self, col: u16, row: u16) -> Option<PaneId> {
        self.content.iter().find(|(_, r)| r.contains(Position::new(col, row))).map(|(id, _)| *id)
    }
}

/// Work for the runtime. Kept as data so input handling is testable.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Quit,
    RefreshNow,
    Load {
        pane: PaneId,
        key: inspect::ContentKey,
        spec: inspect::LoadSpec,
    },
    /// Show containers' logs in the pane: their history merged in time
    /// order (all of it when `full`), then each followed.
    StartLogs {
        pane: PaneId,
        key: inspect::ContentKey,
        /// Stream run number (see `LogStream::run`).
        run: u64,
        sources: Vec<logs::LogSource>,
        full: bool,
    },
    /// Follow one more container into a pane's logs, after `after` (its
    /// whole log when None): a replica that restarted or was added.
    FollowLogs {
        pane: PaneId,
        key: inspect::ContentKey,
        run: u64,
        source: logs::LogSource,
        after: Option<LogTime>,
    },
    /// Run a command in a PTY inside the pane. `key` is set for inspector
    /// content (installed only if still current); None for terminal panes.
    SpawnTerm {
        pane: PaneId,
        key: Option<inspect::ContentKey>,
        argv: Vec<String>,
    },
    /// Keep one stats stream per running container.
    SyncStats(HashSet<String>),
    /// Put text on the system clipboard.
    Copy(String),
    Docker(DockerOp),
    OpenUrl(String),
}

#[derive(Debug, Clone, Copy)]
pub struct Click {
    pub at: Instant,
    pub col: u16,
    pub row: u16,
}

/// The character typed, if no modifier other than shift is held. Item
/// actions, menu shortcuts and typed text use it, so they don't fire on
/// ctrl/alt chords (ctrl+r is find, not restart).
pub fn plain_char(k: &KeyEvent) -> Option<char> {
    match k.code {
        KeyCode::Char(c) if k.modifiers.difference(KeyModifiers::SHIFT).is_empty() => Some(c),
        _ => None,
    }
}

pub struct App {
    pub theme: Theme,
    pub host: String,
    pub snap: Snapshot,
    /// Increments on every snapshot (summary content revision).
    pub snap_generation: u64,
    /// The first snapshot has arrived.
    pub loaded: bool,
    pub docker_error: Option<String>,
    pub sidebar: Sidebar,
    pub sidebar_visible: bool,
    pub focus: Focus,
    pub mode: Mode,
    pub layout: Layout,
    pub panes: HashMap<PaneId, Pane>,
    /// The pane the sidebar drives (see `sync_active_pane`).
    pub active: Option<PaneId>,
    next_pane: u64,
    pub hits: Hits,
    pub drag: Option<Drag>,
    pub last_click: Option<Click>,
    pub cpu: HashMap<String, f64>,
    pub stats: HashMap<String, StatsHistory>,
    /// Seconds since start (drives 1 s refreshes such as Top).
    pub tick: u64,
    /// Cursor to show (focused terminal pane).
    pub cursor: Option<(u16, u16)>,
    /// Transient status message.
    pub toast: Option<(String, Instant)>,
    pub overlay: Option<overlay::Overlay>,
    /// Popups raised by background events while a shell had focus; shown,
    /// oldest first, once focus leaves the shell so they can't capture typing.
    pub deferred: VecDeque<overlay::Overlay>,
    pub selection: Option<selection::Selection>,
    pub copy: Option<copy_mode::CopyCursor>,
    /// Find in a logs pane.
    pub find: Option<find::Find>,
    /// The last query confirmed (`n` / enter on an empty prompt repeat it).
    pub last_find: String,
    /// Running docker operations (status bar).
    pub pending_ops: Vec<String>,
    /// Compose files given on the command line (local project).
    pub compose_files: Vec<String>,
    pub overlay_hits: overlay::OverlayHits,
    pub size: (u16, u16),
    pub dirty: bool,
    pub quit: bool,
    /// Cached flattened sidebar rows, rebuilt when state changes.
    rows: Vec<Row>,
    /// Log stream runs started so far (see `logs::LogStream::run`).
    log_runs: u64,
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
            active: None,
            next_pane: first.0 + 1,
            hits: Hits::default(),
            drag: None,
            last_click: None,
            cpu: HashMap::new(),
            stats: HashMap::new(),
            tick: 0,
            cursor: None,
            toast: None,
            overlay: None,
            deferred: VecDeque::new(),
            selection: None,
            copy: None,
            find: None,
            last_find: String::new(),
            pending_ops: Vec::new(),
            compose_files: Vec::new(),
            overlay_hits: overlay::OverlayHits::default(),
            size: (80, 24),
            dirty: true,
            quit: false,
            rows: Vec::new(),
            log_runs: 0,
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
        self.dirty = true;
    }

    /// Select `key` in the sidebar, if it has a row.
    pub fn select_key(&mut self, key: &NodeKey) {
        self.sidebar.select(&self.rows, key);
        self.dirty = true;
    }

    pub fn selected(&self) -> Option<&NodeKey> {
        self.sidebar.selected.as_ref()
    }

    /// The focused pane, when focus is in the pane area.
    pub fn focused_pane(&self) -> Option<PaneId> {
        (self.focus == Focus::Panes).then_some(self.layout.focus)
    }

    /// Give a pane keyboard focus.
    pub fn focus_pane(&mut self, id: PaneId) {
        self.focus = Focus::Panes;
        self.layout.focus = id;
        self.dirty = true;
    }

    /// The focused pane's terminal when it is interactive and its child is alive.
    fn focused_live_terminal(&mut self) -> Option<&mut TermSurface> {
        let id = self.focused_pane()?;
        match self.panes.get_mut(&id)? {
            Pane { kind: PaneKind::Terminal { .. }, content: Content::Term(t), .. } if t.exited().is_none() => Some(t),
            _ => None,
        }
    }

    fn live_terminal_focused(&self) -> bool {
        let pane = self.focused_pane().and_then(|id| self.panes.get(&id));
        matches!(pane, Some(Pane { kind: PaneKind::Terminal { .. }, content: Content::Term(t), .. }) if t.exited().is_none())
    }

    /// Show a popup that wasn't asked for by the current keystroke: deferred
    /// while a live shell has focus or another popup is open.
    fn raise(&mut self, ov: overlay::Overlay) {
        if self.live_terminal_focused() || self.overlay.is_some() {
            self.deferred.push_back(ov);
        } else {
            self.overlay = Some(ov);
        }
        self.dirty = true;
    }

    pub fn flush_deferred(&mut self) {
        if self.overlay.is_none() && !self.live_terminal_focused() {
            if let Some(ov) = self.deferred.pop_front() {
                self.overlay = Some(ov);
                self.dirty = true;
            }
        }
    }

    /// Transient message in the status bar.
    pub fn flash(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
        self.dirty = true;
    }

    pub fn show_error(&mut self, body: impl Into<String>) {
        self.overlay = Some(overlay::Overlay::Message { title: "Error".into(), body: body.into(), error: true });
        self.dirty = true;
    }

    pub fn content(&self, pane: PaneId) -> Option<&Content> {
        self.panes.get(&pane).map(|p| &p.content)
    }

    /// Change how a pane's content is scrolled, and redraw.
    pub fn scroll_content(&mut self, pane: PaneId, f: impl FnOnce(&mut Content)) {
        if let Some(p) = self.panes.get_mut(&pane) {
            f(&mut p.content);
            self.dirty = true;
        }
    }

    /// Top absolute row currently shown in a pane's content.
    pub fn content_top(&self, pane: PaneId) -> usize {
        self.content(pane).map_or(0, Content::top)
    }

    /// Leftmost content column shown (documents scroll horizontally).
    pub fn content_left(&self, pane: PaneId) -> usize {
        self.content(pane).map_or(0, Content::left)
    }

    /// Text of an absolute content row.
    pub fn content_row_text(&self, pane: PaneId, row: usize) -> String {
        self.content(pane).map(|c| c.row_text(row)).unwrap_or_default()
    }

    pub fn selection_text(&self, sel: &selection::Selection) -> String {
        let (s, e) = sel.ordered();
        self.content(sel.pane).map(|c| c.text_between(s, e)).unwrap_or_default()
    }

    /// Cell of the pane's content under a screen position, clamped into it.
    fn content_cell(&self, pane: PaneId, col: u16, row: u16) -> Option<(u16, u16)> {
        let r = self.hits.content_rect(pane).filter(|r| r.width > 0 && r.height > 0)?;
        Some((col.clamp(r.x, r.right() - 1) - r.x, row.clamp(r.y, r.bottom() - 1) - r.y))
    }

    /// Absolute (row, col) under a screen position, clamped into the pane's content.
    pub fn content_pos(&self, pane: PaneId, col: u16, row: u16) -> Option<(usize, usize)> {
        let (x, y) = self.content_cell(pane, col, row)?;
        Some((self.content_top(pane) + y as usize, self.content_left(pane) + x as usize))
    }

    /// Size of a pane's content area at the last render (default 80×24).
    pub fn content_size(&self, pane: PaneId) -> (u16, u16) {
        self.hits.content_rect(pane).map_or((80, 24), |r| (r.width.max(2), r.height.max(2)))
    }

    pub fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane);
        self.next_pane += 1;
        id
    }

    pub fn handle_app_event(&mut self, ev: AppEvent) -> Vec<Effect> {
        match ev {
            AppEvent::TermActivity(pane) => {
                self.find_output(pane);
                self.dirty = true;
            }
            AppEvent::TermExited { pane, surface, code } => {
                let found =
                    self.panes.get_mut(&pane).filter(|p| matches!(&p.content, Content::Term(t) if t.id() == surface));
                if let Some(Pane { content: Content::Term(t), kind, .. }) = found {
                    let what =
                        if matches!(kind, PaneKind::Terminal { .. }) { "process exited" } else { "stream ended" };
                    match code {
                        Some(c) => t.notice(&format!("[{what} with code {c}]")),
                        None => t.notice(&format!("[{what}]")),
                    }
                    self.dirty = true;
                }
            }
            AppEvent::LogsEnded { pane, run, end } => self.on_logs_ended(pane, run, end),
            AppEvent::OpDone { op, result } => return self.on_op_done(op, result),
            AppEvent::Stats(id, sample) => {
                self.cpu.insert(id.clone(), sample.cpu_percent());
                self.stats.entry(id).or_default().push(Instant::now(), sample);
                self.dirty = true;
            }
            AppEvent::Snapshot(s) => return self.on_snapshot(*s),
            AppEvent::DockerError(e) => {
                self.docker_error = Some(e);
                self.dirty = true;
            }
            AppEvent::Doc { pane, key, lines } => {
                if let Some(p) = self.panes.get_mut(&pane).filter(|p| p.loaded.as_ref() == Some(&key)) {
                    p.inflight = false;
                    p.show_lines(lines);
                    self.dirty = true;
                }
            }
        }
        Vec::new()
    }

    fn on_snapshot(&mut self, snap: Snapshot) -> Vec<Effect> {
        let first_load = !self.loaded;
        self.snap = snap;
        self.snap_generation += 1;
        self.loaded = true;
        self.docker_error = None;
        self.cpu.retain(|id, _| self.snap.containers.iter().any(|c| &c.id == id && c.is_running()));
        self.stats.retain(|id, _| self.snap.containers.iter().any(|c| &c.id == id));
        self.rebuild_rows();
        if first_load {
            // Start on the first item rather than a section header, so the
            // inspector has something to show.
            if let Some(key) = self.rows.iter().find(|r| r.depth > 0).map(|r| r.key.clone()) {
                self.select_key(&key);
            }
        }
        let running = self.snap.containers.iter().filter(|c| c.is_running()).map(|c| c.id.clone()).collect();
        vec![Effect::SyncStats(running)]
    }

    fn on_op_done(&mut self, op: DockerOp, result: Result<Option<String>, String>) -> Vec<Effect> {
        let desc = op.describe();
        if let Some(i) = self.pending_ops.iter().position(|d| *d == desc) {
            self.pending_ops.remove(i);
        }
        match result {
            Ok(Some(msg)) => self.flash(msg),
            Ok(None) => self.flash(format!("done: {desc}")),
            Err(e) => match op {
                // lazydocker offers to force-remove a running container.
                DockerOp::RemoveContainer { id, volumes, force: false } if is_must_stop(&e) => {
                    let name = self.container_name(&id);
                    self.raise(overlay::Overlay::Confirm {
                        title: "Force remove".into(),
                        body: format!("{name} is running. Stop and remove it?"),
                        danger: true,
                        action: actions::Action::Docker(DockerOp::RemoveContainer { id, volumes, force: true }),
                    });
                }
                _ => self.raise(overlay::Overlay::Message { title: "Error".into(), body: e, error: true }),
            },
        }
        vec![Effect::RefreshNow]
    }
}

#[cfg(test)]
pub(crate) mod tests;
