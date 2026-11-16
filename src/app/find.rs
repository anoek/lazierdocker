//! Find in logs panes: `/` or ctrl+f, then `n`/`N` to step through matches.
//!
//! Logs read bottom-up, so the first match is the nearest one at or above
//! the bottom of the view, "next" (`n`, ctrl+g, F3) goes to older (higher)
//! matches and "previous" (`N`, ctrl+shift+g, shift+F3) to newer ones.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{plain_char, App, Content, Effect, Focus, Mode};
use crate::term::find::Match;
use crate::term::TermSurface;
use crate::ui::layout::PaneId;

/// Least time between re-searches while output streams in.
const REFRESH: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub struct Find {
    pub pane: PaneId,
    /// The terminal surface the matches belong to.
    surface: u64,
    pub query: String,
    pub matches: Vec<Match>,
    pub current: Option<usize>,
    /// Row the search started from (bottom of the view when it opened).
    origin: usize,
    /// The view when `matches` was computed.
    view: View,
    /// New output arrived since the last search.
    stale: bool,
    searched_at: Instant,
}

/// Where a terminal's viewport was.
#[derive(Debug, Clone, Copy)]
struct View {
    top: usize,
    /// It followed new output.
    following: bool,
}

impl View {
    fn of(t: &TermSurface) -> Self {
        View { top: t.viewport_top(), following: t.at_bottom() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Older,
    Newer,
}

/// Keys that step between matches both while typing and afterwards:
/// browser (ctrl+g, ctrl+shift+g, F3, shift+F3), shell / emacs reverse and
/// forward search (ctrl+r, ctrl+s) and ctrl+p / ctrl+n.
///
/// Without the kitty keyboard protocol most terminals send ctrl+shift+g as
/// ctrl+g; it is only "previous" when the shift is reported. xterm-style
/// terminals send shift+F3 as `CSI 1;2 R`, which crossterm reads as a cursor
/// position report, so it only arrives from terminals that send it as F15.
pub(super) fn motion(k: &KeyEvent) -> Option<Step> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        KeyCode::Char('G') if ctrl => Some(Step::Newer),
        KeyCode::Char('g') if ctrl && shift => Some(Step::Newer),
        KeyCode::Char('g') if ctrl => Some(Step::Older),
        KeyCode::F(3) if shift => Some(Step::Newer),
        KeyCode::F(15) => Some(Step::Newer),
        KeyCode::F(3) => Some(Step::Older),
        KeyCode::Char('r' | 'p') if ctrl => Some(Step::Older),
        KeyCode::Char('s' | 'n') if ctrl => Some(Step::Newer),
        _ => None,
    }
}

impl App {
    /// The focused pane's logs terminal, if it is showing one.
    fn find_target(&self) -> Option<(PaneId, u64)> {
        if self.focus != Focus::Panes {
            return None;
        }
        let id = self.layout.focus;
        match self.panes.get(&id).map(|p| &p.content) {
            Some(Content::Term(t)) if self.shows_logs(id) => Some((id, t.id())),
            _ => None,
        }
    }

    /// `/` or ctrl+f in a pane.
    pub fn open_find(&mut self) {
        let Some((pane, surface)) = self.find_target() else {
            self.flash("find works in logs panes");
            return;
        };
        let Some(Content::Term(t)) = self.panes.get(&pane).map(|p| &p.content) else { return };
        let (_, top, len) = t.scrollbar();
        let origin = match &self.find {
            Some(f) if f.pane == pane => f.current.map(|i| f.matches[i].start.0),
            _ => None,
        }
        .unwrap_or((top + len).saturating_sub(1));
        self.find = Some(Find {
            pane,
            surface,
            query: String::new(),
            matches: Vec::new(),
            current: None,
            origin,
            view: View::of(t),
            stale: false,
            searched_at: Instant::now(),
        });
        self.mode = Mode::Find;
        self.dirty = true;
    }

    /// Keys while typing the query.
    pub fn find_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        if let Some(step) = motion(&k) {
            self.step_find(step);
            self.dirty = true;
            return Vec::new();
        }
        match k.code {
            KeyCode::Esc => self.close_find(),
            KeyCode::Enter => {
                // Enter on an empty prompt repeats the last search.
                if let Some(f) = self.find.as_mut().filter(|f| f.query.is_empty() && !self.last_find.is_empty()) {
                    f.query = self.last_find.clone();
                    self.search(true);
                }
                self.commit_find();
            }
            KeyCode::Backspace => {
                if let Some(f) = self.find.as_mut() {
                    f.query.pop();
                }
                self.search(true);
            }
            KeyCode::Up => self.step_find(Step::Older),
            KeyCode::Down => self.step_find(Step::Newer),
            KeyCode::Char('f') if k.modifiers.contains(KeyModifiers::CONTROL) => self.step_find(Step::Older),
            _ => {
                if let Some(c) = plain_char(&k) {
                    self.find_type(&c.to_string());
                }
            }
        }
        self.dirty = true;
        Vec::new()
    }

    /// Stop typing and keep the query: remember it for `n`, or close the
    /// search when the query is empty or found nothing.
    pub fn commit_find(&mut self) {
        self.mode = Mode::Normal;
        let Some(f) = self.find.as_ref() else { return };
        if f.query.is_empty() {
            self.close_find();
        } else if f.matches.is_empty() {
            let q = f.query.clone();
            self.close_find();
            self.flash(format!("not found: {q}"));
        } else {
            self.last_find = f.query.clone();
        }
    }

    /// Append typed or pasted text to the query.
    pub fn find_type(&mut self, text: &str) {
        if let Some(f) = self.find.as_mut() {
            f.query.push_str(text);
        }
        self.search(true);
    }

    pub fn close_find(&mut self) {
        self.find = None;
        if self.mode == Mode::Find {
            self.mode = Mode::Normal;
        }
        self.dirty = true;
    }

    /// Find is showing matches in the focused pane (n/N/esc apply).
    pub fn find_active_here(&self) -> bool {
        self.focus == Focus::Panes && self.find.as_ref().is_some_and(|f| f.pane == self.layout.focus)
    }

    /// Re-run the search. `jump`: pick the match nearest the origin and show
    /// it (typing); otherwise keep the current match (new output).
    fn search(&mut self, jump: bool) {
        let Some(f) = self.find.as_ref() else { return };
        let pane = f.pane;
        let Some(Content::Term(t)) = self.panes.get(&pane).map(|p| &p.content) else { return };
        let matches = t.find(&f.query);
        let view = View::of(t);
        let f = self.find.as_mut().expect("checked above");
        let current = if jump {
            // Nearest at or above the origin, else the first below it.
            matches.iter().rposition(|m| m.start.0 <= f.origin).or((!matches.is_empty()).then_some(0))
        } else {
            f.current.map(|i| f.matches[i]).and_then(|old| {
                // A pinned view moves with its content when old rows are
                // dropped; follow that shift. A following view doesn't tell.
                let shift =
                    if f.view.following || view.following { 0 } else { view.top as isize - f.view.top as isize };
                let row = (old.start.0 as isize + shift).max(0) as usize;
                let at = (row, old.start.1);
                matches
                    .iter()
                    .position(|m| m.start == at)
                    .or_else(|| (0..matches.len()).min_by_key(|&i| matches[i].start.0.abs_diff(row)))
            })
        };
        f.matches = matches;
        f.current = current;
        f.view = view;
        f.stale = false;
        f.searched_at = Instant::now();
        if jump {
            self.reveal_current();
        }
        self.dirty = true;
    }

    fn reveal_current(&mut self) {
        let Some(f) = self.find.as_ref() else { return };
        let Some(row) = f.current.map(|i| f.matches[i].start.0) else { return };
        if let Some(Content::Term(t)) = self.panes.get_mut(&f.pane).map(|p| &mut p.content) {
            t.reveal_row(row);
        }
        let Some(Content::Term(t)) = self.panes.get(&f.pane).map(|p| &p.content) else { return };
        let view = View::of(t);
        if let Some(f) = self.find.as_mut() {
            f.view = view;
        }
    }

    /// `n` (older) / `N` (newer), wrapping at either end.
    pub fn step_find(&mut self, step: Step) {
        let Some(f) = self.find.as_mut() else { return };
        let n = f.matches.len();
        if n == 0 {
            return;
        }
        let (next, wrapped) = match (f.current, step) {
            (None, Step::Older) => (n - 1, false),
            (None, Step::Newer) => (0, false),
            (Some(0), Step::Older) => (n - 1, true),
            (Some(i), Step::Older) => (i - 1, false),
            (Some(i), Step::Newer) if i + 1 == n => (0, true),
            (Some(i), Step::Newer) => (i + 1, false),
        };
        f.current = Some(next);
        f.origin = f.matches[next].start.0;
        self.reveal_current();
        if wrapped {
            self.flash(if step == Step::Older { "search wrapped to the bottom" } else { "search wrapped to the top" });
        }
        self.dirty = true;
    }

    /// A "next"/"previous" key in a pane: step through the matches, or with
    /// no search showing, run the last one again (as browsers and vim do).
    /// False when there is nothing to step or repeat here.
    pub fn find_step_or_repeat(&mut self, step: Step) -> bool {
        if self.find_active_here() {
            self.step_find(step);
            return true;
        }
        if self.last_find.is_empty() || self.find_target().is_none() {
            return false;
        }
        self.open_find();
        if let Some(f) = self.find.as_mut() {
            f.query = self.last_find.clone();
        }
        self.search(true);
        self.commit_find();
        true
    }

    /// New output reached a pane: its matches need refreshing.
    pub fn find_output(&mut self, pane: PaneId) {
        if let Some(f) = self.find.as_mut().filter(|f| f.pane == pane) {
            f.stale = true;
        }
    }

    /// Before drawing: drop the search if its pane no longer shows the same
    /// logs, stop typing if focus moved, and refresh matches for new output.
    pub fn refresh_find(&mut self) {
        let Some(f) = self.find.as_ref() else { return };
        let same = matches!(
            self.panes.get(&f.pane).map(|p| &p.content),
            Some(Content::Term(t)) if t.id() == f.surface
        ) && self.shows_logs(f.pane);
        if !same {
            self.close_find();
            return;
        }
        if self.mode == Mode::Find && !(self.focus == Focus::Panes && self.layout.focus == f.pane) {
            self.mode = Mode::Normal;
        }
        if f.stale && f.searched_at.elapsed() >= REFRESH {
            self.search(false);
        }
    }

    /// The search, while its pane still shows the surface it searched. The
    /// runtime can replace the surface after `refresh_find` ran and before the
    /// next frame; then there is nothing to show until it runs again.
    pub(super) fn shown_find(&self) -> Option<&Find> {
        self.find.as_ref().filter(|f| matches!(self.content(f.pane), Some(Content::Term(t)) if t.id() == f.surface))
    }

    /// Status bar text: the query and match position.
    pub fn find_status(&self) -> Option<String> {
        let f = self.shown_find().filter(|f| f.pane == self.layout.focus || self.mode == Mode::Find)?;
        let cursor = if self.mode == Mode::Find { "▏" } else { "" };
        let count = match (f.current, f.matches.len()) {
            _ if f.query.is_empty() => String::new(),
            (_, 0) => "  no matches".into(),
            (Some(i), n) => format!("  {}/{n}", i + 1),
            (None, n) => format!("  {n} matches"),
        };
        Some(format!("/{}{cursor}{count}", f.query))
    }
}
