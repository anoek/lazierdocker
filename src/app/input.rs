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
        if self.overlay.is_some() || !matches!(self.mode, Mode::Normal | Mode::Filter) {
            return Vec::new();
        }
        if self.mode == Mode::Filter {
            self.sidebar.filter.push_str(text.lines().next().unwrap_or(""));
            self.rebuild_rows();
        } else if let Some(t) = self.focused_live_terminal() {
            t.paste(&text);
        }
        Vec::new()
    }

    /// The focused pane's terminal when it is interactive and its child is alive.
    pub fn focused_live_terminal(&mut self) -> Option<&mut crate::term::TermSurface> {
        if self.focus != Focus::Panes {
            return None;
        }
        match self.panes.get_mut(&self.layout.focus) {
            Some(super::Pane { kind: super::PaneKind::Terminal { .. }, content: super::Content::Term(t), .. })
                if t.exited().is_none() =>
            {
                Some(t)
            }
            _ => None,
        }
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
            KeyCode::Char('h') | KeyCode::Left => self.focus_dir(area, Dir4::Left),
            KeyCode::Char('l') | KeyCode::Right => self.focus_dir(area, Dir4::Right),
            KeyCode::Char('k') | KeyCode::Up => self.focus_dir(area, Dir4::Up),
            KeyCode::Char('j') | KeyCode::Down => self.focus_dir(area, Dir4::Down),
            KeyCode::Char('z') => self.layout.toggle_zoom(),
            KeyCode::Char('?') => return self.perform(super::actions::Action::Help),
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
            // ctrl+b twice sends a literal ctrl+b to a terminal pane.
            KeyCode::Char('b') => {
                if let Some(t) = self.focused_live_terminal() {
                    t.send_key(&k);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn swap_dir(&mut self, area: Rect, dir: Dir4) {
        if let Some(other) = self.layout.neighbour(area, self.layout.focus, dir) {
            let me = self.layout.focus;
            self.layout.swap(me, other);
            self.layout.focus = me;
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
            KeyCode::Char('x') | KeyCode::Char('?') => return self.perform(super::actions::Action::Help),
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
            KeyCode::Enter | KeyCode::Tab => self.focus_inspector(),
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
            KeyCode::Char(c) => {
                if let Some(target) = self.sidebar.selected.clone() {
                    return self.item_key(c, &target);
                }
            }
            _ => {}
        }
        let h = self.hits.sidebar.list_area.height as usize;
        self.sidebar.ensure_visible(h);
        Vec::new()
    }

    /// Per-item lazydocker keys, from the sidebar or a pane showing the item.
    pub fn item_key(&mut self, c: char, target: &NodeKey) -> Vec<Effect> {
        match c {
            'x' | '?' => self.perform(super::actions::Action::Help),
            'E' => self.exec_shell(target),
            'a' => self.attach(target),
            'm' => {
                self.open_logs(target.clone());
                Vec::new()
            }
            _ => self.docker_key(c, target).unwrap_or_default(),
        }
    }

    fn pane_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let id = self.layout.focus;
        let page = self.hits.content.iter().find(|(p, _)| *p == id).map(|(_, r)| r.height.max(1) as i32).unwrap_or(10);
        // A finished terminal pane: any of these closes it.
        if matches!(self.panes.get(&id).map(|p| &p.kind), Some(super::PaneKind::Terminal { .. }))
            && matches!(k.code, KeyCode::Char('q') | KeyCode::Char('x') | KeyCode::Enter | KeyCode::Esc)
        {
            self.close_pane(id);
            return Vec::new();
        }
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
            KeyCode::Char('h') | KeyCode::Left => self.hscroll_pane(id, -8),
            KeyCode::Char('l') | KeyCode::Right => self.hscroll_pane(id, 8),
            KeyCode::Home | KeyCode::Char('g') => self.scroll_pane_top(id),
            KeyCode::End | KeyCode::Char('G') => self.scroll_pane_end(id),
            KeyCode::Char(c) => {
                if let Some(target) = self.pane_target(id) {
                    return self.item_key(c, &target);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn hscroll_pane(&mut self, id: crate::ui::layout::PaneId, delta: i32) {
        if let Some(super::Pane { content: super::Content::Doc(d), .. }) = self.panes.get_mut(&id) {
            d.hscroll_by(delta);
            self.dirty = true;
        }
    }

    fn scroll_pane_top(&mut self, id: crate::ui::layout::PaneId) {
        if let Some(p) = self.panes.get_mut(&id) {
            match &mut p.content {
                super::Content::Doc(d) => d.scroll_top(),
                super::Content::Term(t) => t.scroll_to_top(),
            }
            self.dirty = true;
        }
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
                super::Content::Term(t) => t.scroll_lines(delta),
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
                super::Content::Term(t) => t.scroll_to_bottom(),
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
        self.focus_inspector();
    }

    /// Move focus to the panes, preferring the inspector over a shell (so
    /// navigation keys don't land in a terminal by surprise).
    pub fn focus_inspector(&mut self) {
        if let Some(id) = self.primary_inspector() {
            self.layout.focus = id;
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

    /// A mouse press ends keyboard modes: the filter is committed, resize,
    /// copy and prefix modes are left (so the next keys go where the user clicked).
    fn end_keyboard_mode(&mut self) {
        match self.mode {
            Mode::Normal => {}
            Mode::Copy => {
                self.copy = None;
                self.selection = None;
                self.mode = Mode::Normal;
            }
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
        // Active drags take every event until release.
        if let Some(Drag::TermMouse(pane)) = self.drag.clone() {
            let action = match m.kind {
                MouseEventKind::Drag(_) => Some(ghostty_vt::MOUSE_ACTION_MOTION),
                MouseEventKind::Up(_) => Some(ghostty_vt::MOUSE_ACTION_RELEASE),
                _ => None,
            };
            if let Some(action) = action {
                self.forward_mouse(pane, action, col, row);
                if action == ghostty_vt::MOUSE_ACTION_RELEASE {
                    self.drag = None;
                }
                return Vec::new();
            }
        }
        if let Some(Drag::Select(pane)) = self.drag.clone() {
            match m.kind {
                MouseEventKind::Drag(_) => {
                    self.autoscroll_selection(pane, row);
                    if let (Some(pos), Some(sel)) = (self.content_pos(pane, col, row), self.selection.as_mut()) {
                        sel.drag(pos);
                    }
                    self.dirty = true;
                    return Vec::new();
                }
                MouseEventKind::Up(_) => {
                    self.drag = None;
                    self.dirty = true;
                    match self.selection.as_mut() {
                        Some(sel) if sel.phase == super::selection::Phase::Dragging => {
                            sel.phase = super::selection::Phase::Done;
                            let sel = *sel;
                            let text = self.selection_text(&sel);
                            if !text.is_empty() {
                                return vec![Effect::Copy(text)];
                            }
                        }
                        _ => self.selection = None,
                    }
                    return Vec::new();
                }
                _ => {}
            }
        }
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
            MouseEventKind::Down(MouseButton::Right) => self.right_down(col, row),
            MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight
                if m.modifiers.contains(KeyModifiers::SHIFT)
                    || matches!(m.kind, MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight) =>
            {
                let d = if matches!(m.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft) { -8 } else { 8 };
                if let Some((id, _, _)) = self.hits.panes.iter().find(|(_, r, _)| contains(*r, col, row)).cloned() {
                    self.hscroll_pane(id, d);
                }
                Vec::new()
            }
            MouseEventKind::ScrollUp => self.wheel(col, row, -3),
            MouseEventKind::ScrollDown => self.wheel(col, row, 3),
            _ => Vec::new(),
        }
    }

    /// Dragging past the top/bottom edge scrolls the content (herdr: 3–15 lines
    /// depending on distance; we scroll per drag event).
    fn autoscroll_selection(&mut self, pane: crate::ui::layout::PaneId, row: u16) {
        let Some((_, r)) = self.hits.content.iter().find(|(p, _)| *p == pane).copied() else { return };
        let delta = if row < r.y {
            -((r.y - row) as i32 * 3).clamp(1, 15)
        } else if row >= r.bottom() {
            ((row + 1 - r.bottom()) as i32 * 3).clamp(1, 15)
        } else {
            return;
        };
        self.scroll_pane(pane, delta);
    }

    /// Content-relative cell for a pane, clamped into its content rect.
    fn content_cell(&self, pane: crate::ui::layout::PaneId, col: u16, row: u16) -> Option<(u16, u16)> {
        let (_, r) = self.hits.content.iter().find(|(p, _)| *p == pane)?;
        if r.width == 0 || r.height == 0 {
            return None;
        }
        Some((col.clamp(r.x, r.right() - 1) - r.x, row.clamp(r.y, r.bottom() - 1) - r.y))
    }

    fn forward_mouse(
        &mut self,
        pane: crate::ui::layout::PaneId,
        action: ghostty_vt::ffi::GhosttyMouseAction,
        col: u16,
        row: u16,
    ) {
        let Some((x, y)) = self.content_cell(pane, col, row) else { return };
        if let Some(super::Pane { content: super::Content::Term(t), .. }) = self.panes.get_mut(&pane) {
            t.send_mouse(action, Some(ghostty_vt::MOUSE_BUTTON_LEFT), 0, x, y);
        }
    }

    fn apply_drag(&mut self, drag: &Drag, col: u16, row: u16) {
        match drag {
            Drag::TermMouse(_) | Drag::Select(_) => {}
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
        self.selection = None;
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
        // Terminal panes whose program asked for the mouse get the click.
        if let Some((pane, _)) = self.hits.content.iter().find(|(_, r)| contains(*r, col, row)).copied() {
            let tracking = matches!(self.panes.get(&pane), Some(super::Pane { content: super::Content::Term(t), .. }) if t.mouse_tracking());
            if tracking {
                self.focus = Focus::Panes;
                self.layout.focus = pane;
                self.forward_mouse(pane, ghostty_vt::MOUSE_ACTION_PRESS, col, row);
                self.drag = Some(Drag::TermMouse(pane));
                return Vec::new();
            }
        }
        // Text selection in pane content (double-click selects a word).
        if let Some((pane, _)) = self.hits.content.iter().find(|(_, r)| contains(*r, col, row)).copied() {
            self.focus = Focus::Panes;
            self.layout.focus = pane;
            let Some(pos) = self.content_pos(pane, col, row) else { return Vec::new() };
            if double {
                let line = self.content_row_text(pane, pos.0);
                let idx = crate::doc::col_to_char(&line.chars().collect::<Vec<_>>(), pos.1);
                if let Some((s, e)) = super::selection::word_bounds(&line, idx) {
                    let chars: Vec<char> = line.chars().collect();
                    let col_of = |i: usize| {
                        chars[..i]
                            .iter()
                            .map(|c| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0))
                            .sum::<usize>()
                    };
                    let mut sel = super::selection::Selection::new(pane, (pos.0, col_of(s)));
                    sel.drag((pos.0, col_of(e)));
                    sel.phase = super::selection::Phase::Done;
                    self.selection = Some(sel);
                    return vec![Effect::Copy(chars[s..=e].iter().collect())];
                }
                return Vec::new();
            }
            self.selection = Some(super::selection::Selection::new(pane, pos));
            self.drag = Some(Drag::Select(pane));
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

    /// Right-click: context menu for the sidebar item or pane under the pointer.
    fn right_down(&mut self, col: u16, row: u16) -> Vec<Effect> {
        if let Some((_, key)) = self.hits.sidebar.rows.iter().find(|(r, _)| contains(*r, col, row)).cloned() {
            let rows = self.rows.clone();
            self.sidebar.select(&rows, &key);
            self.focus = Focus::Sidebar;
            self.overlay = Some(super::overlay::Overlay::Menu(self.item_menu(&key).at(col, row)));
            self.dirty = true;
            return Vec::new();
        }
        if let Some((id, _, _)) = self.hits.panes.iter().find(|(_, r, _)| contains(*r, col, row)).cloned() {
            self.layout.focus = id;
            self.focus = Focus::Panes;
            self.overlay = Some(super::overlay::Overlay::Menu(self.pane_menu(id).at(col, row)));
            self.dirty = true;
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
            let cell = self.content_cell(id, col, row).unwrap_or((0, 0));
            match self.panes.get_mut(&id).map(|p| &mut p.content) {
                Some(super::Content::Term(t)) => {
                    t.wheel(delta, cell.0, cell.1);
                    self.dirty = true;
                }
                _ => self.scroll_pane(id, delta),
            }
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
