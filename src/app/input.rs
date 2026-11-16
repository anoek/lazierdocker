//! Keyboard and mouse routing.

use std::time::Instant;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{App, Click, Drag, Effect, Focus, Mode, DOUBLE_CLICK};
use crate::ui::layout::{clamp_ratio, Dir4, SplitDir};
use crate::ui::sidebar::{NodeKey, Section, Sidebar, DEFAULT_WIDTH, MAX_WIDTH, MIN_WIDTH};

pub fn contains(r: Rect, col: u16, row: u16) -> bool {
    r.contains(Position::new(col, row))
}

pub fn is_prefix(k: &KeyEvent) -> bool {
    k.code == KeyCode::Char('b') && k.modifiers.contains(KeyModifiers::CONTROL)
}

impl App {
    pub fn handle_event(&mut self, ev: Event) -> Vec<Effect> {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.handle_key(k),
            Event::Mouse(m) => self.handle_mouse(m),
            Event::Resize(w, h) => {
                self.size = (w, h);
                self.dirty = true;
                Vec::new()
            }
            Event::Paste(text) => self.handle_paste(text),
            _ => Vec::new(),
        }
    }

    fn handle_paste(&mut self, text: String) -> Vec<Effect> {
        if self.mode == Mode::Filter {
            self.sidebar.filter.push_str(text.lines().next().unwrap_or(""));
            self.rebuild_rows();
        }
        Vec::new()
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        self.dirty = true;
        match self.mode {
            Mode::Prefix => {
                self.mode = Mode::Normal;
                return self.prefix_command(k);
            }
            Mode::Filter => return self.filter_key(k),
            Mode::Normal => {}
        }
        if is_prefix(&k) {
            self.mode = Mode::Prefix;
            return Vec::new();
        }
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return vec![Effect::Quit];
        }
        match self.focus {
            Focus::Sidebar => self.sidebar_key(k),
            Focus::Panes => self.pane_key(k),
        }
    }

    fn prefix_command(&mut self, k: KeyEvent) -> Vec<Effect> {
        let area = self.hits.pane_area;
        match k.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('b') if !k.modifiers.contains(KeyModifiers::CONTROL) => {
                self.sidebar_visible = !self.sidebar_visible;
                if !self.sidebar_visible {
                    self.focus = Focus::Panes;
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.focus_dir(area, Dir4::Left),
            KeyCode::Char('l') | KeyCode::Right => self.focus_dir(area, Dir4::Right),
            KeyCode::Char('k') | KeyCode::Up => self.focus_dir(area, Dir4::Up),
            KeyCode::Char('j') | KeyCode::Down => self.focus_dir(area, Dir4::Down),
            KeyCode::Char('z') => self.layout.toggle_zoom(),
            _ => {}
        }
        Vec::new()
    }

    fn focus_dir(&mut self, area: Rect, dir: Dir4) {
        if self.focus == Focus::Sidebar {
            if dir == Dir4::Right {
                self.focus = Focus::Panes;
            }
            return;
        }
        match self.layout.neighbour(area, self.layout.focus, dir) {
            Some(id) => self.layout.focus = id,
            None if dir == Dir4::Left && self.sidebar_visible => self.focus = Focus::Sidebar,
            None => {}
        }
    }

    fn filter_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        match k.code {
            KeyCode::Esc => {
                self.sidebar.filter.clear();
                self.mode = Mode::Normal;
            }
            KeyCode::Enter => self.mode = Mode::Normal,
            KeyCode::Backspace => {
                self.sidebar.filter.pop();
            }
            KeyCode::Char(c) if !k.modifiers.contains(KeyModifiers::CONTROL) => self.sidebar.filter.push(c),
            KeyCode::Down => self.sidebar.move_selection(&self.rows, 1),
            KeyCode::Up => self.sidebar.move_selection(&self.rows, -1),
            _ => {}
        }
        self.rebuild_rows();
        Vec::new()
    }

    fn sidebar_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let rows = self.rows.clone();
        let page = self.hits.sidebar.list_area.height.max(1) as i32;
        match k.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('j') | KeyCode::Down => self.sidebar.move_selection(&rows, 1),
            KeyCode::Char('k') | KeyCode::Up => self.sidebar.move_selection(&rows, -1),
            KeyCode::PageDown => self.sidebar.move_selection(&rows, page),
            KeyCode::PageUp => self.sidebar.move_selection(&rows, -page),
            KeyCode::Home | KeyCode::Char('g') => self.sidebar.move_selection(&rows, -(rows.len() as i32)),
            KeyCode::End | KeyCode::Char('G') => self.sidebar.move_selection(&rows, rows.len() as i32),
            KeyCode::Char('h') | KeyCode::Left => self.collapse_or_parent(),
            KeyCode::Char('l') | KeyCode::Right => self.expand_or_enter(),
            KeyCode::Char(' ') => {
                if let Some(key) = self.sidebar.selected.clone() {
                    if rows.iter().any(|r| r.key == key && r.expandable) {
                        self.sidebar.toggle(&key);
                        self.rebuild_rows();
                    }
                }
            }
            KeyCode::Enter | KeyCode::Tab => self.focus = Focus::Panes,
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Esc => {
                if !self.sidebar.filter.is_empty() {
                    self.sidebar.filter.clear();
                    self.rebuild_rows();
                }
            }
            KeyCode::Char('[') | KeyCode::Char(']') => {
                if let Some(id) = self.primary_inspector() {
                    self.cycle_tab(id, if k.code == KeyCode::Char('[') { -1 } else { 1 });
                }
            }
            KeyCode::Char('e') => {
                self.sidebar.show_exited = !self.sidebar.show_exited;
                self.rebuild_rows();
            }
            KeyCode::Char(c @ '1'..='5') => {
                let section = Section::ALL[(c as u8 - b'1') as usize];
                let key = NodeKey::Section(section);
                self.sidebar.set_expanded(&key, true);
                self.rebuild_rows();
                let rows = self.rows.clone();
                self.sidebar.select(&rows, &key);
            }
            _ => {}
        }
        let h = self.hits.sidebar.list_area.height as usize;
        self.sidebar.ensure_visible(h);
        Vec::new()
    }

    fn pane_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let id = self.layout.focus;
        let page = self.hits.content.iter().find(|(p, _)| *p == id).map(|(_, r)| r.height.max(1) as i32).unwrap_or(10);
        match k.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Esc | KeyCode::BackTab | KeyCode::Tab => {
                if self.sidebar_visible {
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Char('[') => self.cycle_tab(id, -1),
            KeyCode::Char(']') => self.cycle_tab(id, 1),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_pane(id, 1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_pane(id, -1),
            KeyCode::PageDown | KeyCode::Char('J') => self.scroll_pane(id, page),
            KeyCode::PageUp | KeyCode::Char('K') => self.scroll_pane(id, -page),
            KeyCode::Char('d') if k.modifiers.contains(KeyModifiers::CONTROL) => self.scroll_pane(id, page / 2),
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => self.scroll_pane(id, -page / 2),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_pane(id, i32::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.scroll_pane_end(id),
            _ => {}
        }
        Vec::new()
    }

    /// The pane `[`/`]` act on from the sidebar: the focused pane if it is an
    /// inspector, otherwise the first inspector.
    pub fn primary_inspector(&self) -> Option<crate::ui::layout::PaneId> {
        let focus = self.layout.focus;
        if matches!(self.panes.get(&focus).map(|p| &p.kind), Some(super::PaneKind::Inspector)) {
            return Some(focus);
        }
        self.layout
            .panes()
            .into_iter()
            .find(|id| matches!(self.panes.get(id).map(|p| &p.kind), Some(super::PaneKind::Inspector)))
    }

    pub fn scroll_pane(&mut self, id: crate::ui::layout::PaneId, delta: i32) {
        if let Some(p) = self.panes.get_mut(&id) {
            match &mut p.content {
                super::Content::Doc(d) => d.scroll_by(delta),
            }
            self.dirty = true;
        }
    }

    fn scroll_pane_end(&mut self, id: crate::ui::layout::PaneId) {
        if let Some(p) = self.panes.get_mut(&id) {
            match &mut p.content {
                super::Content::Doc(d) => {
                    d.scroll_bottom();
                    d.follow = true;
                }
            }
            self.dirty = true;
        }
    }

    fn collapse_or_parent(&mut self) {
        let Some(key) = self.sidebar.selected.clone() else {
            return;
        };
        let rows = self.rows.clone();
        if let Some(r) = rows.iter().find(|r| r.key == key) {
            if r.expandable && r.expanded {
                self.sidebar.set_expanded(&key, false);
                self.rebuild_rows();
                return;
            }
        }
        if let Some(parent) = Sidebar::parent_of(&rows, &key) {
            self.sidebar.select(&rows, &parent);
        }
    }

    fn expand_or_enter(&mut self) {
        let Some(key) = self.sidebar.selected.clone() else {
            return;
        };
        if let Some(r) = self.rows.iter().find(|r| r.key == key) {
            if r.expandable && !r.expanded {
                self.sidebar.set_expanded(&key, true);
                self.rebuild_rows();
                return;
            }
            if r.expandable {
                let rows = self.rows.clone();
                self.sidebar.move_selection(&rows, 1);
                return;
            }
        }
        self.focus = Focus::Panes;
    }

    fn is_double_click(&mut self, col: u16, row: u16) -> bool {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|c| {
            now.duration_since(c.at) <= DOUBLE_CLICK && c.col.abs_diff(col) <= 1 && c.row.abs_diff(row) <= 1
        });
        self.last_click = if double { None } else { Some(Click { at: now, col, row }) };
        double
    }

    pub fn handle_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        let (col, row) = (m.column, m.row);
        // Active drags take every event until release.
        if let Some(drag) = self.drag.clone() {
            match m.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.apply_drag(&drag, col, row);
                    self.dirty = true;
                    return Vec::new();
                }
                MouseEventKind::Up(_) => {
                    self.apply_drag(&drag, col, row);
                    self.drag = None;
                    self.dirty = true;
                    return Vec::new();
                }
                _ => {}
            }
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.left_down(col, row),
            MouseEventKind::ScrollUp => self.wheel(col, row, -3),
            MouseEventKind::ScrollDown => self.wheel(col, row, 3),
            _ => Vec::new(),
        }
    }

    fn apply_drag(&mut self, drag: &Drag, col: u16, row: u16) {
        match drag {
            Drag::SidebarWidth => {
                let max = MAX_WIDTH.min(self.size.0.saturating_sub(20)).max(MIN_WIDTH);
                self.sidebar.width = (col + 1).clamp(MIN_WIDTH, max);
            }
            Drag::Split { path, dir, area, grab_offset } => {
                let (pointer, origin, length) = match dir {
                    SplitDir::Horizontal => (col as i32, area.x as i32, area.width),
                    SplitDir::Vertical => (row as i32, area.y as i32, area.height),
                };
                let ratio = (pointer + grab_offset - origin) as f32 / f32::from(length.max(1));
                self.layout.set_ratio(path, clamp_ratio(ratio));
            }
        }
    }

    fn left_down(&mut self, col: u16, row: u16) -> Vec<Effect> {
        self.dirty = true;
        let double = self.is_double_click(col, row);
        if let Some(div) = self.hits.sidebar_divider {
            if contains(div, col, row) {
                if double {
                    self.sidebar.width = DEFAULT_WIDTH;
                } else {
                    self.drag = Some(Drag::SidebarWidth);
                }
                return Vec::new();
            }
        }
        if let Some((_, key)) = self.hits.sidebar.toggles.iter().find(|(r, _)| contains(*r, col, row)).cloned() {
            self.focus = Focus::Sidebar;
            self.sidebar.toggle(&key);
            self.rebuild_rows();
            let rows = self.rows.clone();
            self.sidebar.select(&rows, &key);
            return Vec::new();
        }
        if let Some((_, key)) = self.hits.sidebar.rows.iter().find(|(r, _)| contains(*r, col, row)).cloned() {
            self.focus = Focus::Sidebar;
            let rows = self.rows.clone();
            self.sidebar.select(&rows, &key);
            if double && rows.iter().any(|r| r.key == key && r.expandable) {
                self.sidebar.toggle(&key);
                self.rebuild_rows();
            }
            return Vec::new();
        }
        if let Some(split) = self.hits.splits.iter().find(|s| split_hit(s, col, row)).cloned() {
            let pointer = match split.dir {
                SplitDir::Horizontal => col,
                SplitDir::Vertical => row,
            };
            self.drag = Some(Drag::Split {
                grab_offset: split.pos as i32 - pointer as i32,
                path: split.path,
                dir: split.dir,
                area: split.area,
            });
            return Vec::new();
        }
        if let Some((_, id, tab)) = self.hits.tabs.iter().find(|(r, _, _)| contains(*r, col, row)).cloned() {
            self.focus = Focus::Panes;
            self.layout.focus = id;
            self.set_tab(id, tab);
            return Vec::new();
        }
        if let Some((id, _, _)) = self.hits.panes.iter().find(|(_, r, _)| contains(*r, col, row)).cloned() {
            self.focus = Focus::Panes;
            self.layout.focus = id;
        }
        Vec::new()
    }

    fn wheel(&mut self, col: u16, row: u16, delta: i32) -> Vec<Effect> {
        if contains(self.hits.sidebar_area, col, row) {
            let h = self.hits.sidebar.list_area.height as usize;
            self.sidebar.scroll_by(self.rows.len(), h, delta);
            self.dirty = true;
            return Vec::new();
        }
        if let Some((id, _, _)) = self.hits.panes.iter().find(|(_, r, _)| contains(*r, col, row)).cloned() {
            self.scroll_pane(id, delta);
        }
        Vec::new()
    }
}

/// The 1-cell divider of a split (its first row/col belongs to the second child's frame).
pub fn split_hit(s: &crate::ui::layout::SplitBorder, col: u16, row: u16) -> bool {
    match s.dir {
        SplitDir::Horizontal => col == s.pos && row >= s.area.y && row < s.area.bottom(),
        SplitDir::Vertical => row == s.pos && col >= s.area.x && col < s.area.right(),
    }
}
