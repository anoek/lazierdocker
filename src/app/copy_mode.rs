//! Keyboard copy mode (prefix+[), herdr-style.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::selection::{Phase, Selection};
use super::{App, Content, Effect, Mode};
use crate::ui::layout::PaneId;
use crate::ui::text::{char_to_col, col_to_char};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyCursor {
    pub pane: PaneId,
    /// (absolute row, column)
    pub pos: (usize, usize),
    pub anchor: Option<(usize, usize)>,
    pub linewise: bool,
}

impl App {
    pub fn enter_copy_mode(&mut self) {
        let pane = self.layout.focus;
        let Some(rows) = self.content(pane).map(Content::rows) else { return };
        let (_, h) = self.content_size(pane);
        // Start at the bottom-left of the view (where the newest output is).
        let row = (self.content_top(pane) + h as usize).saturating_sub(1).min(rows - 1);
        self.copy = Some(CopyCursor { pane, pos: (row, 0), anchor: None, linewise: false });
        self.mode = Mode::Copy;
        self.selection = None;
        self.dirty = true;
    }

    pub(super) fn exit_copy_mode(&mut self) {
        self.copy = None;
        self.mode = Mode::Normal;
        self.selection = None;
    }

    /// Keep the cursor's row in view by scrolling the content.
    fn reveal(&mut self, pane: PaneId, row: usize) {
        let h = self.content_size(pane).1 as usize;
        let top = self.content_top(pane);
        if row < top {
            self.scroll_content(pane, |c| c.scroll_by(-((top - row) as i32)));
        } else if row >= top + h {
            self.scroll_content(pane, |c| c.scroll_by((row + 1 - top - h) as i32));
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
        let max_row = self.content(c.pane).map_or(1, Content::rows) - 1;
        let h = self.content_size(c.pane).1 as usize;
        // Word and line motions work on chars; the cursor is a display column.
        let chars: Vec<char> = self.content_row_text(c.pane, c.pos.0).chars().collect();
        let at = col_to_char(&chars, c.pos.1);
        let col_of = |i: usize| char_to_col(&chars, i);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
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
            KeyCode::Char('h') | KeyCode::Left => c.pos.1 = col_of(at.saturating_sub(1)),
            KeyCode::Char('l') | KeyCode::Right => c.pos.1 = col_of((at + 1).min(chars.len().saturating_sub(1))),
            KeyCode::Char('k') | KeyCode::Up => c.pos.0 = c.pos.0.saturating_sub(1),
            KeyCode::Char('j') | KeyCode::Down => c.pos.0 = (c.pos.0 + 1).min(max_row),
            KeyCode::Char('0') | KeyCode::Home => c.pos.1 = 0,
            KeyCode::Char('$') | KeyCode::End => c.pos.1 = col_of(chars.len().saturating_sub(1)),
            KeyCode::Char('^') => c.pos.1 = col_of(chars.iter().position(|ch| !ch.is_whitespace()).unwrap_or(0)),
            KeyCode::Char('g') => c.pos = (0, 0),
            KeyCode::Char('G') => c.pos = (max_row, 0),
            KeyCode::Char('u') if ctrl => c.pos.0 = c.pos.0.saturating_sub(h / 2),
            KeyCode::Char('d') if ctrl => c.pos.0 = (c.pos.0 + h / 2).min(max_row),
            KeyCode::Char('b') if ctrl => c.pos.0 = c.pos.0.saturating_sub(h),
            KeyCode::Char('f') if ctrl => c.pos.0 = (c.pos.0 + h).min(max_row),
            KeyCode::PageUp => c.pos.0 = c.pos.0.saturating_sub(h),
            KeyCode::PageDown => c.pos.0 = (c.pos.0 + h).min(max_row),
            KeyCode::Char('w') => c.pos.1 = col_of(next_word_start(&chars, at)),
            KeyCode::Char('b') => c.pos.1 = col_of(prev_word_start(&chars, at)),
            KeyCode::Char('e') => c.pos.1 = col_of(word_end(&chars, at)),
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
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn next_word_start(chars: &[char], col: usize) -> usize {
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

fn prev_word_start(chars: &[char], col: usize) -> usize {
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

fn word_end(chars: &[char], col: usize) -> usize {
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
        let l: &[char] = &"foo bar.baz  qux".chars().collect::<Vec<_>>();
        assert_eq!(next_word_start(l, 0), 4);
        assert_eq!(next_word_start(l, 4), 7, "punctuation is its own word");
        assert_eq!(next_word_start(l, 8), 13);
        assert_eq!(prev_word_start(l, 13), 8);
        assert_eq!(prev_word_start(l, 4), 0);
        assert_eq!(word_end(l, 0), 2);
        assert_eq!(word_end(l, 2), 6);
    }
}
