//! Read-only "document" surface: styled lines that scroll both ways, with
//! text extraction for selections.

pub mod format;
pub mod graph;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use unicode_width::UnicodeWidthStr;

use crate::ui::text::col_to_char;

#[derive(Debug, Clone, Default)]
pub struct DocSurface {
    pub lines: Vec<Line<'static>>,
    pub scroll: usize,
    /// Horizontal scroll in display columns.
    pub hscroll: usize,
    /// Width at the last render.
    pub view_width: usize,
    /// Stay at the bottom when the content is replaced (Top refreshes); set
    /// by scrolling to the end, cleared by scrolling up.
    pub follow: bool,
    /// Height at the last render (for page scrolling and clamping).
    pub view_height: usize,
}

impl DocSurface {
    pub fn new(lines: Vec<Line<'static>>) -> Self {
        Self { lines: printable(lines), ..Default::default() }
    }

    pub fn message(text: impl Into<String>) -> Self {
        Self::new(vec![Line::raw(text.into())])
    }

    /// Replace content, keeping the scroll position when possible.
    pub fn set_lines(&mut self, lines: Vec<Line<'static>>) {
        self.lines = printable(lines);
        self.clamp();
        self.hscroll = self.hscroll.min(self.max_hscroll());
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

    fn max_hscroll(&self) -> usize {
        self.max_line_width().saturating_sub(self.view_width.max(1))
    }

    pub fn hscroll_by(&mut self, delta: i32) {
        self.hscroll = (self.hscroll as i64 + delta as i64).clamp(0, self.max_hscroll() as i64) as usize;
    }

    pub fn scroll_top(&mut self) {
        self.scroll = 0;
        self.follow = false;
    }

    /// Scroll to the end and stay there as content is replaced.
    pub fn scroll_bottom(&mut self) {
        self.scroll = self.max_scroll();
        self.follow = true;
    }

    pub fn render(&mut self, buf: &mut Buffer, area: Rect) {
        self.view_height = area.height as usize;
        self.view_width = area.width as usize;
        self.clamp();
        let marker = Style::default().fg(Color::DarkGray);
        for (i, line) in self.lines.iter().skip(self.scroll).take(area.height as usize).enumerate() {
            let y = area.y + i as u16;
            // As `Buffer::set_line` draws, but skipping the first `hscroll` columns.
            let mut col = 0usize;
            let mut x = area.x;
            for g in line.styled_graphemes(Style::default()) {
                let (symbol, style) = (g.symbol, g.style);
                let w = symbol.width();
                if w == 0 {
                    continue;
                }
                if col >= self.hscroll {
                    if x as usize + w > area.right() as usize {
                        break;
                    }
                    buf[(x, y)].set_symbol(symbol).set_style(style);
                    // A wide symbol hides the cells it covers.
                    for hidden in x + 1..x + w as u16 {
                        buf[(hidden, y)].reset();
                    }
                    x += w as u16;
                }
                col += w;
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
            let to = if row == end.0 { (col_to_char(&chars, end.1) + 1).min(chars.len()) } else { chars.len() };
            let piece: String = chars[from..to.max(from)].iter().collect();
            out.push(piece.trim_end().to_string());
        }
        out.join("\n")
    }
}

/// Make lines safe to draw: inspect strings carry tabs and newlines, which
/// a terminal cell can't show. Tabs become spaces; other controls are escaped.
fn printable(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    for span in lines.iter_mut().flat_map(|l| l.spans.iter_mut()) {
        if span.content.contains(char::is_control) {
            let mut clean = String::with_capacity(span.content.len());
            for c in span.content.chars() {
                match c {
                    '\t' => clean.push_str("    "),
                    c if c.is_control() => clean.extend(c.escape_default()),
                    c => clean.push(c),
                }
            }
            span.content = clean.into();
        }
    }
    lines
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
    fn control_chars_are_made_visible_and_wide_chars_scroll() {
        let mut d = DocSurface::new(vec![Line::from(ratatui::text::Span::raw("a\tb\n")), Line::raw("日本語abc")]);
        assert_eq!(d.line_text(0), "a    b\\n");
        let area = Rect::new(0, 0, 8, 2);
        let mut buf = Buffer::empty(area);
        d.hscroll_by(2);
        d.render(&mut buf, area);
        let row = |y| (0..8).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        assert_eq!(row(0), "   b\\n  ");
        assert_eq!(row(1), "本 語 abc ", "a wide char takes two cells (the second kept blank)");
    }

    #[test]
    fn text_between_stream_selection() {
        let d = DocSurface::new(vec![Line::raw("hello world"), Line::raw("second line"), Line::raw("third")]);
        assert_eq!(d.text_between((0, 6), (1, 5)), "world\nsecond");
        assert_eq!(d.text_between((1, 5), (0, 6)), "world\nsecond", "reversed order");
        assert_eq!(d.text_between((2, 0), (2, 99)), "third");
        // A selection starting or ending on either half of a wide char includes it.
        let d = DocSurface::new(vec![Line::raw("a日b")]);
        assert_eq!(d.text_between((0, 2), (0, 3)), "日b");
        assert_eq!(d.text_between((0, 0), (0, 1)), "a日");
    }
}
