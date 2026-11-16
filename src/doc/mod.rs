//! Read-only "document" surface: styled lines with vertical scroll.

pub mod format;
pub mod graph;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Default)]
pub struct DocSurface {
    pub lines: Vec<Line<'static>>,
    pub scroll: usize,
    /// Horizontal scroll in display columns.
    pub hscroll: usize,
    /// Width at the last render.
    pub view_width: usize,
    /// Keep the view pinned to the bottom as lines arrive.
    pub follow: bool,
    /// Height at the last render (for page scrolling and clamping).
    pub view_height: usize,
}

impl DocSurface {
    pub fn new(lines: Vec<Line<'static>>) -> Self {
        Self { lines, ..Default::default() }
    }

    pub fn message(text: impl Into<String>) -> Self {
        Self::new(vec![Line::raw(text.into())])
    }

    /// Replace content, keeping the scroll position when possible.
    pub fn set_lines(&mut self, lines: Vec<Line<'static>>) {
        self.lines = lines;
        self.clamp();
    }

    fn max_scroll(&self) -> usize {
        self.lines.len().saturating_sub(self.view_height.max(1))
    }

    fn clamp(&mut self) {
        if self.follow {
            self.scroll = self.max_scroll();
        } else {
            self.scroll = self.scroll.min(self.max_scroll());
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let next = (self.scroll as i64 + delta as i64).clamp(0, self.max_scroll() as i64) as usize;
        self.scroll = next;
        self.follow = false;
    }

    fn max_line_width(&self) -> usize {
        self.lines.iter().map(|l| l.width()).max().unwrap_or(0)
    }

    pub fn hscroll_by(&mut self, delta: i32) {
        let max = self.max_line_width().saturating_sub(self.view_width.max(1));
        self.hscroll = (self.hscroll as i64 + delta as i64).clamp(0, max as i64) as usize;
    }

    pub fn scroll_top(&mut self) {
        self.scroll = 0;
        self.follow = false;
    }

    pub fn scroll_bottom(&mut self) {
        self.scroll = self.max_scroll();
    }

    pub fn render(&mut self, buf: &mut Buffer, area: Rect) {
        self.view_height = area.height as usize;
        self.view_width = area.width as usize;
        self.clamp();
        let marker = Style::default().fg(Color::DarkGray);
        for (i, line) in self.lines.iter().skip(self.scroll).take(area.height as usize).enumerate() {
            let y = area.y + i as u16;
            if self.hscroll == 0 {
                buf.set_line(area.x, y, line, area.width);
            } else {
                // Draw cell by cell, skipping the first `hscroll` columns.
                let mut col = 0usize;
                let mut x = area.x;
                'spans: for span in &line.spans {
                    let style = line.style.patch(span.style);
                    for ch in span.content.chars() {
                        let w = ch.width().unwrap_or(0);
                        if col >= self.hscroll {
                            if x + w as u16 > area.right() {
                                break 'spans;
                            }
                            buf[(x, y)].set_char(ch).set_style(style);
                            x += w as u16;
                        }
                        col += w;
                    }
                }
            }
            if line.width() > self.hscroll + area.width as usize && area.width > 0 {
                buf[(area.right() - 1, y)].set_symbol("›").set_style(marker);
            }
        }
    }

    /// Plain text of a line.
    pub fn line_text(&self, row: usize) -> String {
        self.lines.get(row).map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).unwrap_or_default()
    }

    /// Text between two (row, col) positions in document coordinates, inclusive,
    /// as a stream selection (partial first/last lines).
    pub fn text_between(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let (start, end) = if start <= end { (start, end) } else { (end, start) };
        let mut out = Vec::new();
        for row in start.0..=end.0.min(self.lines.len().saturating_sub(1)) {
            let text = self.line_text(row);
            let chars: Vec<char> = text.chars().collect();
            let from = if row == start.0 { col_to_char(&chars, start.1) } else { 0 };
            let to = if row == end.0 { col_to_char(&chars, end.1 + 1) } else { chars.len() };
            let piece: String =
                chars[from.min(chars.len())..to.min(chars.len()).max(from.min(chars.len()))].iter().collect();
            out.push(piece.trim_end().to_string());
        }
        out.join("\n")
    }
}

/// Convert a display column to a char index (accounting for wide chars).
pub fn col_to_char(chars: &[char], col: usize) -> usize {
    use unicode_width::UnicodeWidthChar;
    let mut w = 0;
    for (i, c) in chars.iter().enumerate() {
        if w >= col {
            return i;
        }
        w += c.width().unwrap_or(0);
    }
    chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(n: usize) -> DocSurface {
        DocSurface::new((0..n).map(|i| Line::raw(format!("line {i}"))).collect())
    }

    #[test]
    fn scroll_clamps_to_content() {
        let mut d = doc(10);
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 4));
        d.render(&mut buf, Rect::new(0, 0, 10, 4));
        d.scroll_by(100);
        assert_eq!(d.scroll, 6);
        d.scroll_by(-100);
        assert_eq!(d.scroll, 0);
        d.render(&mut buf, Rect::new(0, 0, 10, 4));
        assert_eq!(buf[(0, 0)].symbol(), "l");
    }

    #[test]
    fn follow_sticks_to_bottom() {
        let mut d = doc(3);
        d.follow = true;
        d.view_height = 2;
        d.set_lines((0..10).map(|i| Line::raw(i.to_string())).collect());
        assert_eq!(d.scroll, 8);
    }

    #[test]
    fn horizontal_scroll_and_overflow_marker() {
        let mut d = DocSurface::new(vec![Line::raw("0123456789abcdefghij"), Line::raw("short")]);
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        d.render(&mut buf, area);
        let row = |buf: &Buffer, y| (0..8).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert_eq!(row(&buf, 0), "0123456›", "cut-off lines are marked");
        d.hscroll_by(10);
        let mut buf = Buffer::empty(area);
        d.render(&mut buf, area);
        assert_eq!(row(&buf, 0), "abcdefg›");
        assert_eq!(row(&buf, 1), "        ");
        d.hscroll_by(100);
        assert_eq!(d.hscroll, 12, "clamped to the widest line");
        d.hscroll_by(-100);
        assert_eq!(d.hscroll, 0);
    }

    #[test]
    fn text_between_stream_selection() {
        let d = DocSurface::new(vec![Line::raw("hello world"), Line::raw("second line"), Line::raw("third")]);
        assert_eq!(d.text_between((0, 6), (1, 5)), "world\nsecond");
        assert_eq!(d.text_between((1, 5), (0, 6)), "world\nsecond", "reversed order");
        assert_eq!(d.text_between((2, 0), (2, 99)), "third");
    }
}
