//! Keyboard copy mode (prefix+[) and resize mode (prefix+r), herdr-style.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::selection::{Phase, Selection};
use super::{App, Content, Effect, Mode};
use crate::ui::layout::{Dir4, PaneId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyCursor {
    pub pane: PaneId,
    /// (absolute row, column)
    pub pos: (usize, usize),
    pub anchor: Option<(usize, usize)>,
    pub linewise: bool,
}

impl App {
    fn content_rows(&self, pane: PaneId) -> usize {
        match self.panes.get(&pane).map(|p| &p.content) {
            Some(Content::Doc(d)) => d.lines.len().max(1),
            Some(Content::Term(t)) => t.scrollbar().0.max(1),
            None => 1,
        }
    }

    pub fn enter_copy_mode(&mut self) {
        let pane = self.layout.focus;
        if !self.panes.contains_key(&pane) {
            return;
        }
        let (w, h) = self.content_size(pane);
        let top = self.content_top(pane);
        // Start at the bottom-left of the view (where the newest output is).
        let row = (top + h as usize).saturating_sub(1).min(self.content_rows(pane) - 1);
        let _ = w;
        self.copy = Some(CopyCursor { pane, pos: (row, 0), anchor: None, linewise: false });
        self.mode = Mode::Copy;
        self.selection = None;
        self.dirty = true;
    }

    fn exit_copy_mode(&mut self) {
        self.copy = None;
        self.mode = Mode::Normal;
        self.selection = None;
    }

    /// Keep the cursor's row in view by scrolling the content.
    fn reveal(&mut self, pane: PaneId, row: usize) {
        let (_, h) = self.content_size(pane);
        let top = self.content_top(pane);
        let h = h as usize;
        if row < top {
            self.scroll_pane(pane, -((top - row) as i32));
        } else if row >= top + h {
            self.scroll_pane(pane, (row + 1 - top - h) as i32);
        }
    }

    fn sync_copy_selection(&mut self) {
        let Some(c) = self.copy else { return };
        self.selection = c.anchor.map(|a| {
            let (w, _) = self.content_size(c.pane);
            let (mut from, mut to) = if a <= c.pos { (a, c.pos) } else { (c.pos, a) };
            if c.linewise {
                from.1 = 0;
                to.1 = w as usize;
            }
            Selection { pane: c.pane, anchor: from, head: to, phase: Phase::Dragging }
        });
    }

    pub fn copy_mode_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let Some(mut c) = self.copy else {
            self.mode = Mode::Normal;
            return Vec::new();
        };
        let rows = self.content_rows(c.pane);
        let (w, h) = self.content_size(c.pane);
        let line = self.content_row_text(c.pane, c.pos.0);
        let line_len = line.chars().count();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let max_row = rows - 1;
        match k.code {
            KeyCode::Char('q') => {
                self.exit_copy_mode();
                return Vec::new();
            }
            KeyCode::Esc => {
                if c.anchor.is_some() {
                    c.anchor = None;
                } else {
                    self.exit_copy_mode();
                    return Vec::new();
                }
            }
            KeyCode::Char('h') | KeyCode::Left => c.pos.1 = c.pos.1.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => c.pos.1 = (c.pos.1 + 1).min(w.saturating_sub(1) as usize),
            KeyCode::Char('k') | KeyCode::Up => c.pos.0 = c.pos.0.saturating_sub(1),
            KeyCode::Char('j') | KeyCode::Down => c.pos.0 = (c.pos.0 + 1).min(max_row),
            KeyCode::Char('0') | KeyCode::Home => c.pos.1 = 0,
            KeyCode::Char('$') | KeyCode::End => c.pos.1 = line_len.saturating_sub(1),
            KeyCode::Char('^') => c.pos.1 = line.chars().position(|ch| !ch.is_whitespace()).unwrap_or(0),
            KeyCode::Char('g') => c.pos = (0, 0),
            KeyCode::Char('G') => c.pos = (max_row, 0),
            KeyCode::Char('u') if ctrl => c.pos.0 = c.pos.0.saturating_sub(h as usize / 2),
            KeyCode::Char('d') if ctrl => c.pos.0 = (c.pos.0 + h as usize / 2).min(max_row),
            KeyCode::Char('b') if ctrl => c.pos.0 = c.pos.0.saturating_sub(h as usize),
            KeyCode::Char('f') if ctrl => c.pos.0 = (c.pos.0 + h as usize).min(max_row),
            KeyCode::PageUp => c.pos.0 = c.pos.0.saturating_sub(h as usize),
            KeyCode::PageDown => c.pos.0 = (c.pos.0 + h as usize).min(max_row),
            KeyCode::Char('w') => c.pos.1 = next_word_start(&line, c.pos.1),
            KeyCode::Char('b') => c.pos.1 = prev_word_start(&line, c.pos.1),
            KeyCode::Char('e') => c.pos.1 = word_end(&line, c.pos.1),
            KeyCode::Char('v') | KeyCode::Char(' ') => {
                c.linewise = false;
                c.anchor = if c.anchor.is_some() { None } else { Some(c.pos) };
            }
            KeyCode::Char('V') => {
                c.linewise = true;
                c.anchor = if c.anchor.is_some() { None } else { Some(c.pos) };
            }
            KeyCode::Char('y') | KeyCode::Enter => {
                self.copy = Some(c);
                self.sync_copy_selection();
                let text = self.selection.map(|s| self.selection_text(&s)).unwrap_or_default();
                self.exit_copy_mode();
                return if text.is_empty() { Vec::new() } else { vec![Effect::Copy(text)] };
            }
            _ => {}
        }
        self.copy = Some(c);
        self.reveal(c.pane, c.pos.0);
        self.sync_copy_selection();
        self.dirty = true;
        Vec::new()
    }

    pub fn resize_mode_key(&mut self, k: KeyEvent) -> Vec<Effect> {
        let area = self.hits.pane_area;
        const STEP: f32 = 0.05;
        match k.code {
            KeyCode::Char('h') | KeyCode::Left => self.layout.resize_focused(area, Dir4::Left, STEP),
            KeyCode::Char('l') | KeyCode::Right => self.layout.resize_focused(area, Dir4::Right, STEP),
            KeyCode::Char('k') | KeyCode::Up => self.layout.resize_focused(area, Dir4::Up, STEP),
            KeyCode::Char('j') | KeyCode::Down => self.layout.resize_focused(area, Dir4::Down, STEP),
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('q') => self.mode = Mode::Normal,
            _ => {}
        }
        self.dirty = true;
        Vec::new()
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn next_word_start(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut i = col;
    if i >= chars.len() {
        return col;
    }
    let start_word = is_word(chars[i]);
    while i < chars.len() && !chars[i].is_whitespace() && is_word(chars[i]) == start_word {
        i += 1;
    }
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    i.min(chars.len().saturating_sub(1))
}

fn prev_word_start(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut i = col.min(chars.len());
    while i > 0 && chars.get(i - 1).is_some_and(|c| c.is_whitespace()) {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    let w = is_word(chars[i - 1]);
    while i > 0 && !chars[i - 1].is_whitespace() && is_word(chars[i - 1]) == w {
        i -= 1;
    }
    i
}

fn word_end(line: &str, col: usize) -> usize {
    let chars: Vec<char> = line.chars().collect();
    let mut i = col + 1;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }
    if i >= chars.len() {
        return col;
    }
    let w = is_word(chars[i]);
    while i + 1 < chars.len() && !chars[i + 1].is_whitespace() && is_word(chars[i + 1]) == w {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_motions() {
        let l = "foo bar.baz  qux";
        assert_eq!(next_word_start(l, 0), 4);
        assert_eq!(next_word_start(l, 4), 7, "punctuation is its own word");
        assert_eq!(next_word_start(l, 8), 13);
        assert_eq!(prev_word_start(l, 13), 8);
        assert_eq!(prev_word_start(l, 4), 0);
        assert_eq!(word_end(l, 0), 2);
        assert_eq!(word_end(l, 2), 6);
    }
}
