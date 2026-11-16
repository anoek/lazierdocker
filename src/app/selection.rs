//! Mouse text selection over pane content (herdr's stream selection model).
//! Rows are absolute content rows (document line / terminal screen row) so a
//! selection stays put while the view scrolls.

use crate::ui::layout::PaneId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Mouse down, not moved yet (a plain click, not a selection).
    Anchored,
    Dragging,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub pane: PaneId,
    /// (absolute row, column)
    pub anchor: (usize, usize),
    pub head: (usize, usize),
    pub phase: Phase,
}

impl Selection {
    pub fn new(pane: PaneId, at: (usize, usize)) -> Self {
        Self { pane, anchor: at, head: at, phase: Phase::Anchored }
    }

    pub fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    pub fn drag(&mut self, to: (usize, usize)) {
        if to != self.anchor || self.phase == Phase::Dragging {
            self.phase = Phase::Dragging;
        }
        self.head = to;
    }

    pub fn is_visible(&self) -> bool {
        self.phase != Phase::Anchored
    }

    /// Stream selection: partial first line, full middle lines, partial last line.
    pub fn contains(&self, row: usize, col: usize) -> bool {
        if !self.is_visible() {
            return false;
        }
        let (s, e) = self.ordered();
        if row < s.0 || row > e.0 {
            return false;
        }
        let after_start = row > s.0 || col >= s.1;
        let before_end = row < e.0 || col <= e.1;
        after_start && before_end
    }
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || "|()[]{},;!<>\"'`".contains(c)
}

/// Inclusive char-index bounds of the word at `idx` in `line`: a URL if one
/// covers the position, otherwise a run of non-separator characters.
pub fn word_bounds(line: &str, idx: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = line.chars().collect();
    if idx >= chars.len() || is_separator(chars[idx]) {
        return None;
    }
    // URLs: extend across separators that are legal in URLs ( ) [ ] etc. until whitespace.
    let text: String = chars.iter().collect();
    for scheme in ["https://", "http://", "file://"] {
        let mut from = 0;
        while let Some(pos) = text[from..].find(scheme) {
            let start_b = from + pos;
            let start = text[..start_b].chars().count();
            let mut end = start;
            while end + 1 < chars.len() && !chars[end + 1].is_whitespace() && !"\"'<>`".contains(chars[end + 1]) {
                end += 1;
            }
            // Trailing punctuation usually isn't part of the URL.
            while end > start && ".,;:)]}".contains(chars[end]) {
                end -= 1;
            }
            if idx >= start && idx <= end {
                return Some((start, end));
            }
            from = start_b + scheme.len();
        }
    }
    let mut s = idx;
    while s > 0 && !is_separator(chars[s - 1]) {
        s -= 1;
    }
    let mut e = idx;
    while e + 1 < chars.len() && !is_separator(chars[e + 1]) {
        e += 1;
    }
    Some((s, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_stream_contains() {
        let mut s = Selection::new(PaneId(1), (2, 5));
        assert!(!s.contains(2, 5), "a plain click selects nothing");
        s.drag((4, 3));
        assert!(s.contains(2, 5) && s.contains(2, 80), "rest of first line");
        assert!(!s.contains(2, 4));
        assert!(s.contains(3, 0), "middle lines whole");
        assert!(s.contains(4, 3) && !s.contains(4, 4), "last line up to head");
        // Dragging backwards.
        s.drag((1, 7));
        assert!(s.contains(1, 7) && s.contains(2, 5) && !s.contains(2, 6));
    }

    #[test]
    fn click_is_not_selection() {
        let s = Selection::new(PaneId(1), (0, 0));
        assert!(!s.is_visible());
        let mut s2 = s;
        s2.drag((0, 0));
        assert_eq!(s2.phase, Phase::Anchored, "no movement, still a click");
    }

    #[test]
    fn word_bounds_url_and_token() {
        let line = "see https://example.com/a(b)/c?x=1, then demo-web-1 (ok)";
        let url_at = line.find("example").unwrap();
        let (s, e) = word_bounds(line, url_at).unwrap();
        assert_eq!(&line[s..=e], "https://example.com/a(b)/c?x=1");
        let tok = line.find("web").unwrap();
        let (s, e) = word_bounds(line, tok).unwrap();
        assert_eq!(&line[s..=e], "demo-web-1");
        let (s, e) = word_bounds(line, line.find("ok").unwrap()).unwrap();
        assert_eq!(&line[s..=e], "ok");
        assert_eq!(word_bounds(line, 3), None, "on a space");
        assert_eq!(word_bounds(line, 999), None);
    }
}
