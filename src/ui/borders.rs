//! Pane frames with shared dividers and correct T-junction glyphs.
//!
//! Each pane owns its left/top edge; its right/bottom edge is drawn only on the
//! outer boundary of the pane area, so neighbours share one divider line. Glyphs
//! are derived per cell from the four directions it connects to.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};

use super::layout::PaneId;
use super::theme::Theme;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
struct LineCell {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
    focused: bool,
}

pub struct PaneFrame<'a> {
    pub id: PaneId,
    pub rect: Rect,
    pub title: &'a str,
    pub focused: bool,
}

/// Outline (left, top, right, bottom) line coordinates for a pane rect inside `area`.
fn outline(area: Rect, r: Rect) -> Option<(u16, u16, u16, u16)> {
    if r.width == 0 || r.height == 0 {
        return None;
    }
    let right = if r.right() >= area.right() { r.right() - 1 } else { r.right() };
    let bottom = if r.bottom() >= area.bottom() { r.bottom() - 1 } else { r.bottom() };
    Some((r.x, r.y, right, bottom))
}

/// Inner (content) rect for a pane rect inside `area`.
pub fn inner(area: Rect, r: Rect) -> Rect {
    match outline(area, r) {
        Some((l, t, rt, b)) if rt > l + 1 && b > t + 1 => Rect::new(l + 1, t + 1, rt - l - 1, b - t - 1),
        Some((l, t, _, _)) => Rect::new(l, t, 0, 0),
        None => Rect::new(r.x, r.y, 0, 0),
    }
}

fn glyph(c: LineCell) -> &'static str {
    match (c.up, c.down, c.left, c.right) {
        (true, true, true, true) => "┼",
        (true, true, true, false) => "┤",
        (true, true, false, true) => "├",
        (true, false, true, true) => "┴",
        (false, true, true, true) => "┬",
        (true, true, false, false) | (true, false, false, false) | (false, true, false, false) => "│",
        (false, false, true, true) | (false, false, true, false) | (false, false, false, true) => "─",
        (false, true, false, true) => "┌",
        (false, true, true, false) => "┐",
        (true, false, false, true) => "└",
        (true, false, true, false) => "┘",
        (false, false, false, false) => " ",
    }
}

/// Draw all pane frames and titles. Returns the inner rect of each pane.
pub fn draw(buf: &mut Buffer, area: Rect, panes: &[PaneFrame<'_>], theme: &Theme) -> HashMap<PaneId, Rect> {
    let mut cells: HashMap<(u16, u16), LineCell> = HashMap::new();
    let mut inners = HashMap::new();
    for p in panes {
        inners.insert(p.id, inner(area, p.rect));
        let Some((l, t, r, b)) = outline(area, p.rect) else {
            continue;
        };
        let hline = |y: u16, cells: &mut HashMap<(u16, u16), LineCell>| {
            for x in l..=r {
                let c = cells.entry((x, y)).or_default();
                if x > l {
                    c.left = true;
                }
                if x < r {
                    c.right = true;
                }
                c.focused |= p.focused;
            }
        };
        hline(t, &mut cells);
        hline(b, &mut cells);
        let vline = |x: u16, cells: &mut HashMap<(u16, u16), LineCell>| {
            for y in t..=b {
                let c = cells.entry((x, y)).or_default();
                if y > t {
                    c.up = true;
                }
                if y < b {
                    c.down = true;
                }
                c.focused |= p.focused;
            }
        };
        vline(l, &mut cells);
        vline(r, &mut cells);
    }
    let normal = Style::default().fg(theme.overlay0);
    let focused = Style::default().fg(theme.accent);
    for ((x, y), c) in &cells {
        if !area.contains((*x, *y).into()) {
            continue;
        }
        if let Some(cell) = buf.cell_mut((*x, *y)) {
            cell.set_symbol(glyph(*c));
            cell.set_style(if c.focused { focused } else { normal });
        }
    }
    for p in panes {
        let Some((l, t, r, _)) = outline(area, p.rect) else {
            continue;
        };
        if r <= l + 4 || p.title.is_empty() {
            continue;
        }
        let max = (r - l - 2) as usize;
        let title = truncate(p.title, max.saturating_sub(2));
        let label = format!(" {title} ");
        let style = if p.focused {
            Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.overlay0)
        };
        buf.set_stringn(l + 2, t, &label, max, style);
    }
    inners
}

/// Truncate to `max` display columns, adding `…` when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    use unicode_width::UnicodeWidthChar;
    let total: usize = s.chars().map(|c| c.width().unwrap_or(0)).sum();
    if total <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout::{Layout, SplitDir};

    fn sym(buf: &Buffer, x: u16, y: u16) -> &str {
        buf[(x, y)].symbol()
    }

    #[test]
    fn junction_glyphs() {
        let area = Rect::new(0, 0, 20, 10);
        let mut l = Layout::new(PaneId(1));
        l.split(PaneId(1), SplitDir::Horizontal, PaneId(2));
        l.split(PaneId(1), SplitDir::Vertical, PaneId(3));
        l.split(PaneId(2), SplitDir::Vertical, PaneId(4));
        let frames: Vec<PaneFrame> =
            l.rects(area).into_iter().map(|(id, rect)| PaneFrame { id, rect, title: "", focused: false }).collect();
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &frames, &Theme::default());
        assert_eq!(sym(&buf, 0, 0), "┌");
        assert_eq!(sym(&buf, 10, 0), "┬");
        assert_eq!(sym(&buf, 19, 0), "┐");
        assert_eq!(sym(&buf, 0, 5), "├");
        assert_eq!(sym(&buf, 10, 5), "┼");
        assert_eq!(sym(&buf, 19, 5), "┤");
        assert_eq!(sym(&buf, 10, 9), "┴");
        assert_eq!(sym(&buf, 19, 9), "┘");
        assert_eq!(sym(&buf, 5, 5), "─");
        assert_eq!(sym(&buf, 10, 2), "│");
    }

    #[test]
    fn inner_rects_exclude_shared_divider() {
        let area = Rect::new(0, 0, 20, 10);
        let left = inner(area, Rect::new(0, 0, 10, 10));
        let right = inner(area, Rect::new(10, 0, 10, 10));
        assert_eq!(left, Rect::new(1, 1, 9, 8));
        assert_eq!(right, Rect::new(11, 1, 8, 8));
    }

    #[test]
    fn title_drawn_on_top_border_truncated() {
        let area = Rect::new(0, 0, 12, 4);
        let frames = [PaneFrame { id: PaneId(1), rect: area, title: "a-very-long-title", focused: true }];
        let mut buf = Buffer::empty(area);
        draw(&mut buf, area, &frames, &Theme::default());
        let top: String = (0..12).map(|x| sym(&buf, x, 0).to_string()).collect();
        assert_eq!(top, "┌─ a-very… ┐");
    }

    #[test]
    fn truncate_wide_chars() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("日本語", 4), "日…");
    }
}
