//! Keyboard and mouse routing.

use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::actions::Action;
use super::find::{motion, Step};
use super::selection::{word_bounds, Phase, Selection};
use super::{plain_char, App, Click, Content, Drag, Effect, Focus, Mode, PaneKind};
use crate::term::{Mods, MouseAction, MouseButton as TermButton};
use crate::ui::layout::{clamp_ratio, Dir4, PaneId, SplitBorder, SplitDir};
use crate::ui::sidebar::{NodeKey, Section, Sidebar, DEFAULT_WIDTH, MAX_WIDTH, MIN_WIDTH};
use crate::ui::text::{char_to_col, col_to_char};

/// Two clicks this close together (in time, and within a cell) are a double-click.
const DOUBLE_CLICK: Duration = Duration::from_millis(350);

pub fn contains(r: Rect, col: u16, row: u16) -> bool {
    r.contains(Position::new(col, row))
}

fn is_prefix(k: &KeyEvent) -> bool {
    k.code == KeyCode::Char('b') && k.modifiers.contains(KeyModifiers::CONTROL)
}

impl SplitBorder {
    /// The 1-cell divider of a split (its first row/col belongs to the second child's frame).
    fn hit(&self, col: u16, row: u16) -> bool {
        match self.dir {
            SplitDir::Horizontal => col == self.pos && row >= self.area.y && row < self.area.bottom(),
            SplitDir::Vertical => row == self.pos && col >= self.area.x && col < self.area.right(),
        }
    }
}

impl App {
    pub fn handle_event(&mut self, ev: Event) -> Vec<Effect> {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.handle_key(k),
            Event::Mouse(m) => self.handle_mouse(m),
            Event::Resize(w, h) => {
                self.size = (w, h);
                // Geometry changed under any drag: abort it.
                self.drag = None;
                self.dirty = true;
                Vec::new()
            }
            Event::Paste(text) => self.handle_paste(text),
            _ => Vec::new(),
        }
    }

    fn handle_paste(&mut self, text: String) -> Vec<Effect> {
        if self.overlay.is_some() {
            return Vec::new();
        }
        let first_line = text.lines().next().unwrap_or("");
        match self.mode {
            Mode::Find => self.find_type(first_line),
            Mode::Filter => {
                self.sidebar.filter.push_str(first_line);
                self.rebuild_rows();
            }
            Mode::Normal => {
                if let Some(t) = self.focused_live_terminal() {
                    t.paste(&text);
                }
            }
            Mode::Prefix | Mode::Resize | Mode::Copy => {}
        }
        Vec::new()
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let fx = self.handle_key_inner(k);
        self.flush_deferred();
        fx
    }

    fn handle_key_inner(&mut self, k: KeyEvent) -> Vec<Effect> {
        self.dirty = true;
        if self.overlay.is_some() {
            return self.overlay_key(k);
        }
        if self.mode != Mode::Copy {
            self.selection = None;
        }
        match self.mode {
            Mode::Prefix => {
                self.mode = Mode::Normal;
                return self.prefix_command(k);
            }
            Mode::Filter => return self.filter_key(k),
            Mode::Resize => return self.resize_mode_key(k),
            Mode::Copy => return self.copy_mode_key(k),
            Mode::Find => return self.find_key(k),
            Mode::Normal => {}
        }
        if is_prefix(&k) {
            self.mode = Mode::Prefix;
            return Vec::new();
        }
        if let Some(t) = self.focused_live_terminal() {
            t.send_key(&k);
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
            // ctrl+b twice sends a literal ctrl+b to a terminal pane.
            KeyCode::Char('b') => {
                if let Some(t) = self.focused_live_terminal() {
                    t.send_key(&k);
                }
            }
            KeyCode::Char('h') | KeyCode::Left => self.focus_dir(area, Dir4::Left),
            KeyCode::Char('l') | KeyCode::Right => self.focus_dir(area, Dir4::Right),
            KeyCode::Char('k') | KeyCode::Up => self.focus_dir(area, Dir4::Up),
            KeyCode::Char('j') | KeyCode::Down => self.focus_dir(area, Dir4::Down),
            KeyCode::Char('z') => self.layout.toggle_zoom(),
            KeyCode::Char('?') => return self.perform(Action::Help),
            KeyCode::Char('r') => self.mode = Mode::Resize,
            KeyCode::Char('[') => {
                self.focus = Focus::Panes;
                self.enter_copy_mode();
            }
            KeyCode::Char('v') | KeyCode::Char('%') => self.split_focused(SplitDir::Horizontal),
            KeyCode::Char('-') | KeyCode::Char('"') => self.split_focused(SplitDir::Vertical),
            KeyCode::Char('x') => self.close_pane(self.layout.focus),
            KeyCode::Char('o') | KeyCode::Tab => self.focus_next_pane(1),
            KeyCode::BackTab => self.focus_next_pane(-1),
            KeyCode::Char('H') => self.swap_dir(area, Dir4::Left),
            KeyCode::Char('L') => self.swap_dir(area, Dir4::Right),
            KeyCode::Char('K') => self.swap_dir(area, Dir4::Up),
            KeyCode::Char('J') => self.swap_dir(area, Dir4::Down),
            _ => {}
        }
        Vec::new()
    }

    /// Swap the focused pane with its neighbour; focus moves with it.
    fn swap_dir(&mut self, area: Rect, dir: Dir4) {
        if let Some(other) = self.layout.neighbour(area, self.layout.focus, dir) {
            self.layout.swap(self.layout.focus, other);
        }
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

    /// Keys in resize mode (prefix+r).
    fn resize_mode_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        const STEP: f32 = 0.05;
        let area = self.hits.pane_area;
        match k.code {
            KeyCode::Char('h') | KeyCode::Left => self.layout.resize_focused(area, Dir4::Left, STEP),
            KeyCode::Char('l') | KeyCode::Right => self.layout.resize_focused(area, Dir4::Right, STEP),
            KeyCode::Char('k') | KeyCode::Up => self.layout.resize_focused(area, Dir4::Up, STEP),
            KeyCode::Char('j') | KeyCode::Down => self.layout.resize_focused(area, Dir4::Down, STEP),
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('q') => self.mode = Mode::Normal,
            _ => {}
        }
        Vec::new()
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
            KeyCode::Down => self.sidebar.move_selection(&self.rows, 1),
            KeyCode::Up => self.sidebar.move_selection(&self.rows, -1),
            _ => {
                if let Some(c) = plain_char(&k) {
                    self.sidebar.filter.push(c);
                }
            }
        }
        self.rebuild_rows();
        Vec::new()
    }

    fn sidebar_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let rows = &self.rows;
        let page = self.hits.sidebar.list_area.height.max(1) as i32;
        let all = rows.len() as i32;
        match k.code {
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('x') | KeyCode::Char('?') => return self.perform(Action::Help),
            KeyCode::Char('j') | KeyCode::Down => self.sidebar.move_selection(rows, 1),
            KeyCode::Char('k') | KeyCode::Up => self.sidebar.move_selection(rows, -1),
            KeyCode::PageDown => self.sidebar.move_selection(rows, page),
            KeyCode::PageUp => self.sidebar.move_selection(rows, -page),
            KeyCode::Home | KeyCode::Char('g') => self.sidebar.move_selection(rows, -all),
            KeyCode::End | KeyCode::Char('G') => self.sidebar.move_selection(rows, all),
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
            KeyCode::Enter | KeyCode::Tab => self.focus_inspector(),
            KeyCode::Char('/') => self.mode = Mode::Filter,
            KeyCode::Esc => {
                if !self.sidebar.filter.is_empty() {
                    self.sidebar.filter.clear();
                    self.rebuild_rows();
                }
            }
            KeyCode::Char('[') | KeyCode::Char(']') => {
                if let Some(id) = self.sidebar_pane() {
                    self.cycle_tab(id, if k.code == KeyCode::Char('[') { -1 } else { 1 });
                }
            }
            KeyCode::Char('e') => {
                self.sidebar.show_exited = !self.sidebar.show_exited;
                self.rebuild_rows();
            }
            KeyCode::Char(c @ '1'..='5') => {
                let key = NodeKey::Section(Section::ALL[(c as u8 - b'1') as usize]);
                self.sidebar.set_expanded(&key, true);
                self.rebuild_rows();
                self.select_key(&key);
            }
            _ => {
                if let (Some(c), Some(target)) = (plain_char(&k), self.sidebar.selected.clone()) {
                    return self.item_key(c, &target);
                }
            }
        }
        Vec::new()
    }

    /// Per-item lazydocker keys, from the sidebar or a pane showing the item.
    fn item_key(&mut self, c: char, target: &NodeKey) -> Vec<Effect> {
        match c {
            'E' => self.exec_shell(target),
            'a' => self.attach(target),
            'm' => {
                self.open_logs(target.clone());
                Vec::new()
            }
            _ => self.docker_key_action(c, target).map(|a| self.perform(a)).unwrap_or_default(),
        }
    }

    fn pane_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let id = self.layout.focus;
        let page = self.content_size(id).1 as i32;
        // A finished terminal pane: any of these closes it.
        if matches!(self.panes.get(&id).map(|p| &p.kind), Some(PaneKind::Terminal { .. }))
            && matches!(k.code, KeyCode::Char('q') | KeyCode::Char('x') | KeyCode::Enter | KeyCode::Esc)
        {
            self.close_pane(id);
            return Vec::new();
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let step = motion(&k).or(match k.code {
            KeyCode::Char('n') if !ctrl => Some(Step::Older),
            KeyCode::Char('N') if !ctrl => Some(Step::Newer),
            _ => None,
        });
        if let Some(step) = step {
            if self.find_step_or_repeat(step) {
                return Vec::new();
            }
        }
        match k.code {
            KeyCode::Char('/') => self.open_find(),
            KeyCode::Char('f') if ctrl => self.open_find(),
            KeyCode::Esc if self.find_active_here() => self.close_find(),
            KeyCode::Char('q') => return vec![Effect::Quit],
            KeyCode::Char('x') | KeyCode::Char('?') => return self.perform(Action::Help),
            KeyCode::Esc | KeyCode::BackTab | KeyCode::Tab => {
                if self.sidebar_visible {
                    self.focus = Focus::Sidebar;
                }
            }
            KeyCode::Char('[') => self.cycle_tab(id, -1),
            KeyCode::Char(']') => self.cycle_tab(id, 1),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_content(id, |c| c.scroll_by(1)),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_content(id, |c| c.scroll_by(-1)),
            KeyCode::PageDown | KeyCode::Char('J') => self.scroll_content(id, |c| c.scroll_by(page)),
            KeyCode::PageUp | KeyCode::Char('K') => self.scroll_content(id, |c| c.scroll_by(-page)),
            KeyCode::Char('d') if ctrl => self.scroll_content(id, |c| c.scroll_by(page / 2)),
            KeyCode::Char('u') if ctrl => self.scroll_content(id, |c| c.scroll_by(-page / 2)),
            KeyCode::Char('h') | KeyCode::Left => self.scroll_content(id, |c| c.hscroll_by(-8)),
            KeyCode::Char('l') | KeyCode::Right => self.scroll_content(id, |c| c.hscroll_by(8)),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_content(id, Content::scroll_top),
            KeyCode::End | KeyCode::Char('G') => self.scroll_content(id, Content::scroll_bottom),
            KeyCode::Char('f') => self.load_full_logs(id),
            _ => {
                if let (Some(c), Some(target)) = (plain_char(&k), self.pane_target(id)) {
                    return self.item_key(c, &target);
                }
            }
        }
        Vec::new()
    }

    fn collapse_or_parent(&mut self) {
        let Some(key) = self.sidebar.selected.clone() else { return };
        if self.rows.iter().any(|r| r.key == key && r.expandable && r.expanded) {
            self.sidebar.set_expanded(&key, false);
            self.rebuild_rows();
        } else if let Some(parent) = Sidebar::parent_of(&self.rows, &key) {
            self.select_key(&parent);
        }
    }

    fn expand_or_enter(&mut self) {
        let Some(key) = self.sidebar.selected.clone() else { return };
        match self.rows.iter().find(|r| r.key == key) {
            Some(r) if r.expandable && !r.expanded => {
                self.sidebar.set_expanded(&key, true);
                self.rebuild_rows();
            }
            Some(r) if r.expandable => self.sidebar.move_selection(&self.rows, 1),
            _ => self.focus_inspector(),
        }
    }

    fn is_double_click(&mut self, col: u16, row: u16) -> bool {
        let now = Instant::now();
        let double = self.last_click.is_some_and(|c| {
            now.duration_since(c.at) <= DOUBLE_CLICK && c.col.abs_diff(col) <= 1 && c.row.abs_diff(row) <= 1
        });
        self.last_click = if double { None } else { Some(Click { at: now, col, row }) };
        double
    }

    /// A mouse press ends keyboard modes: the filter and find query are
    /// committed, and resize, copy and prefix modes are left, so the next keys
    /// go where the user clicked.
    fn end_keyboard_mode(&mut self) {
        match self.mode {
            Mode::Normal => {}
            Mode::Copy => self.exit_copy_mode(),
            Mode::Find => self.commit_find(),
            Mode::Filter | Mode::Resize | Mode::Prefix => self.mode = Mode::Normal,
        }
    }

    pub fn handle_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        let fx = self.handle_mouse_inner(m);
        self.flush_deferred();
        fx
    }

    fn handle_mouse_inner(&mut self, m: MouseEvent) -> Vec<Effect> {
        let (col, row) = (m.column, m.row);
        if matches!(m.kind, MouseEventKind::Down(_)) && self.overlay.is_none() {
            self.end_keyboard_mode();
        }
        if self.overlay.is_some() && self.drag.is_none() {
            return self.overlay_mouse(m);
        }
        // An active drag takes every motion and the release.
        if let Some(drag) = self.drag.clone() {
            match m.kind {
                MouseEventKind::Drag(_) => return self.drag_to(drag, col, row),
                MouseEventKind::Up(_) => return self.end_drag(drag, col, row),
                _ => {}
            }
        }
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => self.left_down(col, row),
            MouseEventKind::Down(MouseButton::Right) => self.right_down(col, row),
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
                if m.modifiers.contains(KeyModifiers::SHIFT)
                    || matches!(m.kind, MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight) =>
            {
                let d = if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft) { -8 } else { 8 };
                if let Some(id) = self.hits.pane_at(col, row) {
                    self.scroll_content(id, |c| c.hscroll_by(d));
                }
                Vec::new()
            }
            MouseEventKind::ScrollUp => self.wheel(col, row, -3),
            MouseEventKind::ScrollDown => self.wheel(col, row, 3),
            _ => Vec::new(),
        }
    }

    fn drag_to(&mut self, drag: Drag, col: u16, row: u16) -> Vec<Effect> {
        self.dirty = true;
        match drag {
            Drag::TermMouse(pane) => self.forward_mouse(pane, MouseAction::Motion, col, row),
            Drag::Select(pane) => {
                self.autoscroll_selection(pane, row);
                if let (Some(pos), Some(sel)) = (self.content_pos(pane, col, row), self.selection.as_mut()) {
                    sel.drag(pos);
                }
            }
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
                self.layout.set_ratio(&path, clamp_ratio(ratio));
            }
        }
        Vec::new()
    }

    fn end_drag(&mut self, drag: Drag, col: u16, row: u16) -> Vec<Effect> {
        self.drag = None;
        self.dirty = true;
        match drag {
            Drag::TermMouse(pane) => self.forward_mouse(pane, MouseAction::Release, col, row),
            // Copy on release; a click that never moved is not a selection.
            Drag::Select(_) => match self.selection.as_mut() {
                Some(sel) if sel.phase == Phase::Dragging => {
                    sel.phase = Phase::Done;
                    let sel = *sel;
                    let text = self.selection_text(&sel);
                    if !text.is_empty() {
                        return vec![Effect::Copy(text)];
                    }
                }
                _ => self.selection = None,
            },
            Drag::SidebarWidth | Drag::Split { .. } => {
                self.drag_to(drag, col, row);
            }
        }
        Vec::new()
    }

    /// Dragging past the top/bottom edge scrolls the content (herdr: 3–15 lines
    /// depending on distance; we scroll per drag event).
    fn autoscroll_selection(&mut self, pane: PaneId, row: u16) {
        let Some(r) = self.hits.content_rect(pane) else { return };
        let delta = if row < r.y {
            -((r.y - row) as i32 * 3).clamp(1, 15)
        } else if row >= r.bottom() {
            ((row + 1 - r.bottom()) as i32 * 3).clamp(1, 15)
        } else {
            return;
        };
        self.scroll_content(pane, |c| c.scroll_by(delta));
    }

    fn forward_mouse(&mut self, pane: PaneId, action: MouseAction, col: u16, row: u16) {
        let Some((x, y)) = self.content_cell(pane, col, row) else { return };
        if let Some(Content::Term(t)) = self.panes.get_mut(&pane).map(|p| &mut p.content) {
            t.send_mouse(action, Some(TermButton::Left), Mods::empty(), x, y);
        }
    }

    fn left_down(&mut self, col: u16, row: u16) -> Vec<Effect> {
        self.dirty = true;
        self.selection = None;
        let double = self.is_double_click(col, row);
        if self.hits.sidebar_divider.is_some_and(|div| contains(div, col, row)) {
            if double {
                self.sidebar.width = DEFAULT_WIDTH;
            } else {
                self.drag = Some(Drag::SidebarWidth);
            }
            return Vec::new();
        }
        if let Some((_, key)) = self.hits.sidebar.toggles.iter().find(|(r, _)| contains(*r, col, row)).cloned() {
            self.focus = Focus::Sidebar;
            self.sidebar.toggle(&key);
            self.rebuild_rows();
            self.select_key(&key);
            return Vec::new();
        }
        if let Some((_, key)) = self.hits.sidebar.rows.iter().find(|(r, _)| contains(*r, col, row)).cloned() {
            self.focus = Focus::Sidebar;
            self.select_key(&key);
            if double && self.rows.iter().any(|r| r.key == key && r.expandable) {
                self.sidebar.toggle(&key);
                self.rebuild_rows();
            }
            return Vec::new();
        }
        if let Some(split) = self.hits.splits.iter().find(|s| s.hit(col, row)).cloned() {
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
        if let Some(pane) = self.hits.content_at(col, row) {
            self.focus_pane(pane);
            // Terminal programs that asked for the mouse get the click.
            if matches!(self.content(pane), Some(Content::Term(t)) if t.mouse_tracking()) {
                self.forward_mouse(pane, MouseAction::Press, col, row);
                self.drag = Some(Drag::TermMouse(pane));
                return Vec::new();
            }
            let Some(pos) = self.content_pos(pane, col, row) else { return Vec::new() };
            if double {
                return self.select_word(pane, pos);
            }
            self.selection = Some(Selection::new(pane, pos));
            self.drag = Some(Drag::Select(pane));
            return Vec::new();
        }
        if let Some((_, id, tab)) = self.hits.tabs.iter().find(|(r, _, _)| contains(*r, col, row)).cloned() {
            self.focus_pane(id);
            self.set_tab(id, tab);
            return Vec::new();
        }
        if let Some(id) = self.hits.pane_at(col, row) {
            self.focus_pane(id);
        }
        Vec::new()
    }

    /// Double-click: select and copy the word or URL at `pos`.
    fn select_word(&mut self, pane: PaneId, pos: (usize, usize)) -> Vec<Effect> {
        let chars: Vec<char> = self.content_row_text(pane, pos.0).chars().collect();
        let Some((s, e)) = word_bounds(&chars, col_to_char(&chars, pos.1)) else { return Vec::new() };
        let mut sel = Selection::new(pane, (pos.0, char_to_col(&chars, s)));
        sel.drag((pos.0, char_to_col(&chars, e + 1).saturating_sub(1)));
        sel.phase = Phase::Done;
        self.selection = Some(sel);
        vec![Effect::Copy(chars[s..=e].iter().collect())]
    }

    /// Right-click: context menu for the sidebar item or pane under the pointer.
    fn right_down(&mut self, col: u16, row: u16) -> Vec<Effect> {
        if let Some((_, key)) = self.hits.sidebar.rows.iter().find(|(r, _)| contains(*r, col, row)).cloned() {
            self.select_key(&key);
            self.focus = Focus::Sidebar;
            return self.perform(Action::Menu(self.item_menu(&key).at(col, row)));
        }
        if let Some(id) = self.hits.pane_at(col, row) {
            self.focus_pane(id);
            return self.perform(Action::Menu(self.pane_menu(id).at(col, row)));
        }
        Vec::new()
    }

    fn wheel(&mut self, col: u16, row: u16, delta: i32) -> Vec<Effect> {
        if contains(self.hits.sidebar_area, col, row) {
            let h = self.hits.sidebar.list_area.height as usize;
            self.sidebar.scroll_by(self.rows.len(), h, delta);
            self.dirty = true;
        } else if let Some(id) = self.hits.pane_at(col, row) {
            let (x, y) = self.content_cell(id, col, row).unwrap_or((0, 0));
            self.scroll_content(id, |c| match c {
                Content::Term(t) => t.wheel(delta, x, y),
                Content::Doc(d) => d.scroll_by(delta),
            });
        }
        Vec::new()
    }
}
