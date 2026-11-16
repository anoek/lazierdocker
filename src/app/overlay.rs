//! Modal overlays (one at a time, herdr-style): menus, confirmations, help,
//! messages. Input routing and drawing.

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::actions::Action;
use super::input::contains;
use super::{App, Effect};
use crate::ui::borders::truncate;
use crate::ui::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub key: Option<String>,
    /// Shown dimmed on the right (e.g. the command that will run).
    pub detail: Option<String>,
    pub danger: bool,
    pub action: Action,
}

impl MenuItem {
    pub fn new(label: impl Into<String>, action: Action) -> Self {
        Self { label: label.into(), key: None, detail: None, danger: false, action }
    }
    pub fn key(mut self, k: &str) -> Self {
        self.key = Some(k.to_string());
        self
    }
    pub fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
    pub highlighted: usize,
    /// Context menus open at the pointer; option menus are centred.
    pub anchor: Option<(u16, u16)>,
}

impl Menu {
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self { title: title.into(), items, highlighted: 0, anchor: None }
    }
    pub fn at(mut self, col: u16, row: u16) -> Self {
        self.anchor = Some((col, row));
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Overlay {
    Menu(Menu),
    Confirm { title: String, body: String, danger: bool, action: Action },
    Help { filter: String, scroll: usize },
    Message { title: String, body: String, error: bool },
}

#[derive(Debug, Clone, Default)]
pub struct OverlayHits {
    pub panel: Rect,
    pub rows: Vec<(Rect, usize)>,
    pub primary: Option<Rect>,
    pub cancel: Option<Rect>,
}

/// (group, key, description) — the help overlay's content.
pub const HELP: &[(&str, &str, &str)] = &[
    ("Global", "q / ctrl+c", "quit"),
    ("Global", "ctrl+b", "window prefix (works inside terminals)"),
    ("Global", "x / ?", "this help"),
    ("Global", "tab", "toggle sidebar ⇄ panes"),
    ("Global", "right-click", "context menu"),
    ("Sidebar", "↑↓ / j k", "select"),
    ("Sidebar", "← → / h l", "collapse / expand"),
    ("Sidebar", "space", "toggle group"),
    ("Sidebar", "1-5", "jump to section"),
    ("Sidebar", "/", "filter"),
    ("Sidebar", "e", "show / hide exited containers"),
    ("Sidebar", "[ ]", "previous / next tab"),
    ("Sidebar", "enter", "focus panes"),
    ("Containers", "s", "stop"),
    ("Containers", "r", "restart"),
    ("Containers", "S", "start"),
    ("Containers", "p", "pause / unpause"),
    ("Containers", "d", "remove…"),
    ("Containers", "E", "exec shell in a pane"),
    ("Containers", "a", "attach in a pane"),
    ("Containers", "m", "logs in a new pane"),
    ("Containers", "w", "open published port in browser"),
    ("Containers", "b", "bulk commands…"),
    ("Services", "u", "up service"),
    ("Services", "R", "restart / recreate / rebuild…"),
    ("Projects", "U", "up project"),
    ("Projects", "D", "down…"),
    ("Images / Volumes / Networks", "d", "remove…"),
    ("Images / Volumes / Networks", "b", "prune…"),
    ("Panes", "j k / PgUp PgDn", "scroll"),
    ("Panes", "g G", "top / bottom (follow)"),
    ("Window (ctrl+b then)", "v / -", "split right / down"),
    ("Window (ctrl+b then)", "x", "close pane"),
    ("Window (ctrl+b then)", "z", "zoom"),
    ("Window (ctrl+b then)", "h j k l", "focus pane"),
    ("Window (ctrl+b then)", "H J K L", "swap pane"),
    ("Window (ctrl+b then)", "o", "next pane"),
    ("Window (ctrl+b then)", "r", "resize mode"),
    ("Window (ctrl+b then)", "[", "copy mode"),
    ("Window (ctrl+b then)", "b", "toggle sidebar"),
    ("Mouse", "drag borders", "resize panes and sidebar"),
    ("Mouse", "drag text", "select and copy"),
    ("Mouse", "double-click", "select word"),
];

fn help_lines(filter: &str, t: &Theme) -> Vec<Line<'static>> {
    let f = filter.to_lowercase();
    let mut out = Vec::new();
    let mut group = "";
    for (g, k, d) in HELP {
        if !f.is_empty() && !format!("{g} {k} {d}").to_lowercase().contains(&f) {
            continue;
        }
        if *g != group {
            if !out.is_empty() {
                out.push(Line::raw(""));
            }
            out.push(Line::styled(g.to_string(), Style::default().fg(t.mauve).add_modifier(Modifier::BOLD)));
            group = g;
        }
        out.push(Line::from(vec![
            Span::styled(format!("  {k:<18}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(d.to_string(), Style::default().fg(t.text)),
        ]));
    }
    out
}

/// Centred rect of at most w×h inside `area` (herdr's rule).
pub fn centred(area: Rect, w: u16, h: u16) -> Option<Rect> {
    let w = w.min(area.width.saturating_sub(4));
    let h = h.min(area.height.saturating_sub(2));
    if w < 4 || h < 4 {
        return None;
    }
    Some(Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h))
}

/// Filled panel with a square border and a title.
fn panel(buf: &mut Buffer, r: Rect, title: &str, border: ratatui::style::Color, t: &Theme) {
    let bg = Style::default().bg(t.panel_bg).fg(t.text);
    for y in r.y..r.bottom() {
        for x in r.x..r.right() {
            buf[(x, y)].reset();
            buf[(x, y)].set_symbol(" ").set_style(bg);
        }
    }
    let bs = Style::default().fg(border).bg(t.panel_bg);
    for x in r.x..r.right() {
        let (top, bottom) = if x == r.x {
            ("┌", "└")
        } else if x + 1 == r.right() {
            ("┐", "┘")
        } else {
            ("─", "─")
        };
        buf[(x, r.y)].set_symbol(top).set_style(bs);
        buf[(x, r.bottom() - 1)].set_symbol(bottom).set_style(bs);
    }
    for y in r.y + 1..r.bottom() - 1 {
        buf[(r.x, y)].set_symbol("│").set_style(bs);
        buf[(r.right() - 1, y)].set_symbol("│").set_style(bs);
    }
    if !title.is_empty() && r.width > 6 {
        let title = format!(" {} ", truncate(title, r.width as usize - 6));
        buf.set_string(r.x + 2, r.y, title, bs.add_modifier(Modifier::BOLD));
    }
}

fn dim_all(buf: &mut Buffer) {
    let area = buf.area;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let c = &mut buf[(x, y)];
            c.modifier.insert(Modifier::DIM);
        }
    }
}

fn button(buf: &mut Buffer, x: u16, y: u16, label: &str, style: Style) -> Rect {
    let w = label.chars().count() as u16;
    buf.set_string(x, y, label, style);
    Rect::new(x, y, w, 1)
}

impl App {
    pub fn render_overlay(&mut self, buf: &mut Buffer, area: Rect) {
        let t = self.theme;
        let mut hits = OverlayHits::default();
        let Some(ov) = self.overlay.as_mut() else {
            self.overlay_hits = hits;
            return;
        };
        match ov {
            Overlay::Menu(m) => {
                let label_w = m
                    .items
                    .iter()
                    .map(|i| i.label.chars().count() + i.key.as_ref().map(|k| k.chars().count() + 2).unwrap_or(0))
                    .max()
                    .unwrap_or(0);
                let detail_w =
                    m.items.iter().filter_map(|i| i.detail.as_ref()).map(|d| d.chars().count() + 2).max().unwrap_or(0);
                let w = ((label_w + detail_w + 4).max(m.title.chars().count() + 6).max(18) as u16)
                    .min(area.width.saturating_sub(2));
                let h = (m.items.len() as u16 + 2).min(area.height.saturating_sub(1));
                let r = match m.anchor {
                    Some((x, y)) => {
                        let x = x.min(area.right().saturating_sub(w));
                        let y = y.min(area.bottom().saturating_sub(h));
                        Rect::new(x, y, w, h)
                    }
                    None => {
                        dim_all(buf);
                        match centred(area, w, h) {
                            Some(r) => r,
                            None => return,
                        }
                    }
                };
                if r.width < 4 || r.height < 3 {
                    return;
                }
                panel(buf, r, &m.title, t.accent, &t);
                hits.panel = r;
                let inner_w = (r.width - 2) as usize;
                for (i, item) in m.items.iter().enumerate().take((r.height - 2) as usize) {
                    let y = r.y + 1 + i as u16;
                    let row = Rect::new(r.x + 1, y, r.width - 2, 1);
                    let hl = i == m.highlighted;
                    let base = if hl {
                        Style::default().bg(t.accent).fg(t.panel_bg).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().bg(t.panel_bg).fg(if item.danger { t.red } else { t.text })
                    };
                    buf.set_style(row, base);
                    let mut spans = vec![Span::styled(format!(" {}", item.label), base)];
                    if let Some(k) = &item.key {
                        spans.push(Span::styled(format!("  {k}"), if hl { base } else { base.fg(t.overlay0) }));
                    }
                    buf.set_line(row.x, y, &Line::from(spans), row.width);
                    if let Some(d) = &item.detail {
                        let d = truncate(d, inner_w.saturating_sub(label_w + 3));
                        let dw = d.chars().count() as u16;
                        let color = if item.danger { t.red } else { t.cyan };
                        buf.set_string(
                            row.right().saturating_sub(dw + 1),
                            y,
                            d,
                            if hl { base } else { base.fg(color) },
                        );
                    }
                    hits.rows.push((row, i));
                }
            }
            Overlay::Confirm { title, body, danger, .. } => {
                dim_all(buf);
                let Some(r) = centred(area, 64, 7 + (body.len() as u16 / 58)) else { return };
                let border = if *danger { t.red } else { t.accent };
                panel(buf, r, title, border, &t);
                hits.panel = r;
                let text_w = (r.width - 4) as usize;
                for (y, line) in (r.y + 2..r.bottom().saturating_sub(2)).zip(wrap(body, text_w)) {
                    buf.set_string(r.x + 2, y, line, Style::default().fg(t.text).bg(t.panel_bg));
                }
                let by = r.bottom() - 2;
                let primary_bg = if *danger { t.red } else { t.accent };
                let p = button(
                    buf,
                    r.x + 2,
                    by,
                    " ↵ confirm ",
                    Style::default().bg(primary_bg).fg(t.panel_bg).add_modifier(Modifier::BOLD),
                );
                let c = button(
                    buf,
                    p.right() + 2,
                    by,
                    " esc cancel ",
                    Style::default().bg(t.surface0).fg(t.text).add_modifier(Modifier::BOLD),
                );
                hits.primary = Some(p);
                hits.cancel = Some(c);
            }
            Overlay::Message { title, body, error } => {
                dim_all(buf);
                let lines = wrap(body, 70);
                let Some(r) = centred(area, 76, lines.len() as u16 + 5) else { return };
                panel(buf, r, title, if *error { t.red } else { t.accent }, &t);
                hits.panel = r;
                for (i, line) in lines.iter().enumerate().take((r.height as usize).saturating_sub(4)) {
                    buf.set_string(r.x + 2, r.y + 2 + i as u16, line, Style::default().fg(t.text).bg(t.panel_bg));
                }
                let c = button(
                    buf,
                    r.right().saturating_sub(13),
                    r.y,
                    " esc close ",
                    Style::default().bg(t.surface0).fg(t.text),
                );
                hits.cancel = Some(c);
            }
            Overlay::Help { filter, scroll } => {
                dim_all(buf);
                let Some(r) = centred(area, 76, 30) else { return };
                panel(buf, r, "keybindings", t.accent, &t);
                hits.panel = r;
                let c = button(
                    buf,
                    r.right().saturating_sub(13),
                    r.y,
                    " esc close ",
                    Style::default().bg(t.surface0).fg(t.text),
                );
                hits.cancel = Some(c);
                let search = Line::from(vec![
                    Span::styled(" / ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        if filter.is_empty() { "type to filter".to_string() } else { filter.clone() },
                        Style::default().fg(if filter.is_empty() { t.overlay0 } else { t.text }),
                    ),
                ]);
                buf.set_line(r.x + 1, r.y + 1, &search, r.width - 2);
                let lines = help_lines(filter, &t);
                let h = (r.height - 3) as usize;
                *scroll = (*scroll).min(lines.len().saturating_sub(h));
                for (i, l) in lines.iter().skip(*scroll).take(h).enumerate() {
                    buf.set_line(r.x + 2, r.y + 2 + i as u16, l, r.width - 4);
                }
            }
        }
        self.overlay_hits = hits;
    }

    /// Keys while an overlay is open.
    pub fn overlay_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Some(ov) = self.overlay.as_mut() else { return Vec::new() };
        self.dirty = true;
        match ov {
            Overlay::Menu(m) => match k.code {
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = None,
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    m.highlighted = (m.highlighted + 1) % m.items.len().max(1);
                }
                KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                    m.highlighted = (m.highlighted + m.items.len().max(1) - 1) % m.items.len().max(1);
                }
                KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Char('y') => {
                    let i = m.highlighted;
                    return self.activate_menu(i);
                }
                KeyCode::Char(c) => {
                    // Item shortcut keys.
                    if let Some(i) = m.items.iter().position(|it| it.key.as_deref() == Some(c.to_string().as_str())) {
                        return self.activate_menu(i);
                    }
                }
                _ => {}
            },
            Overlay::Confirm { .. } => match k.code {
                KeyCode::Enter | KeyCode::Char('y') => return self.confirm(),
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => self.overlay = None,
                _ => {}
            },
            Overlay::Message { .. } => {
                if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.overlay = None;
                }
            }
            Overlay::Help { filter, scroll } => match k.code {
                KeyCode::Esc if !filter.is_empty() => filter.clear(),
                KeyCode::Esc => self.overlay = None,
                KeyCode::Backspace => {
                    filter.pop();
                }
                KeyCode::Down => *scroll += 1,
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown => *scroll += 10,
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Char(c) => {
                    filter.push(c);
                    *scroll = 0;
                }
                _ => {}
            },
        }
        Vec::new()
    }

    fn activate_menu(&mut self, i: usize) -> Vec<Effect> {
        let Some(Overlay::Menu(m)) = self.overlay.take() else { return Vec::new() };
        match m.items.get(i) {
            Some(item) => self.perform(item.action.clone()),
            None => Vec::new(),
        }
    }

    fn confirm(&mut self) -> Vec<Effect> {
        match self.overlay.take() {
            Some(Overlay::Confirm { action, .. }) => self.perform(action),
            _ => Vec::new(),
        }
    }

    /// Mouse while an overlay is open: overlays swallow every event.
    pub fn overlay_mouse(&mut self, m: MouseEvent) -> Vec<Effect> {
        let (col, row) = (m.column, m.row);
        let hits = self.overlay_hits.clone();
        match m.kind {
            MouseEventKind::Moved => {
                if let Some(Overlay::Menu(menu)) = self.overlay.as_mut() {
                    if let Some((_, i)) = hits.rows.iter().find(|(r, _)| contains(*r, col, row)) {
                        if menu.highlighted != *i {
                            menu.highlighted = *i;
                            self.dirty = true;
                        }
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.dirty = true;
                if let Some((_, i)) = hits.rows.iter().find(|(r, _)| contains(*r, col, row)) {
                    return self.activate_menu(*i);
                }
                if hits.primary.is_some_and(|r| contains(r, col, row)) {
                    return self.confirm();
                }
                if hits.cancel.is_some_and(|r| contains(r, col, row)) || !contains(hits.panel, col, row) {
                    self.overlay = None;
                }
            }
            MouseEventKind::Down(MouseButton::Right) if !contains(hits.panel, col, row) => {
                self.overlay = None;
                self.dirty = true;
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: i32 = if m.kind == MouseEventKind::ScrollDown { 1 } else { -1 };
                match self.overlay.as_mut() {
                    Some(Overlay::Help { scroll, .. }) => *scroll = (*scroll as i32 + d * 3).max(0) as usize,
                    Some(Overlay::Menu(menu)) if !menu.items.is_empty() => {
                        menu.highlighted = (menu.highlighted as i32 + d).clamp(0, menu.items.len() as i32 - 1) as usize;
                    }
                    _ => {}
                }
                self.dirty = true;
            }
            _ => {}
        }
        Vec::new()
    }

    pub fn show_error(&mut self, body: impl Into<String>) {
        self.overlay = Some(Overlay::Message { title: "Error".into(), body: body.into(), error: true });
        self.dirty = true;
    }
}

/// Greedy word wrap.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}
