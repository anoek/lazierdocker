//! Draw a libghostty terminal into a ratatui buffer.

use libghostty_vt::render::Colors;
use libghostty_vt::screen::{Cell, CellContentTag, CellWide};
use libghostty_vt::style::{RgbColor, Style as CellStyle, StyleColor, Underline};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::{Core, TermSurface};

fn rgb(c: RgbColor) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

fn style_color(c: StyleColor) -> Option<Color> {
    match c {
        StyleColor::None => None,
        StyleColor::Palette(i) => Some(Color::Indexed(i.0)),
        StyleColor::Rgb(c) => Some(rgb(c)),
    }
}

/// Background of a cell: its style's, else a colour-only cell's content
/// (erased with a background set), keeping palette indices as indices.
fn cell_bg(st: &CellStyle, raw: Option<Cell>) -> Option<Color> {
    style_color(st.bg_color).or_else(|| match raw?.content_tag().ok()? {
        CellContentTag::BgColorPalette => Some(Color::Indexed(raw?.bg_color_palette().ok()?.0)),
        CellContentTag::BgColorRgb => Some(rgb(raw?.bg_color_rgb().ok()?)),
        _ => None,
    })
}

fn style_for(st: &CellStyle, bg: Option<Color>, colors: Option<&Colors>) -> Style {
    let mut fg = style_color(st.fg_color);
    let mut bg = bg;
    if st.invisible {
        fg = bg.or_else(|| colors.map(|c| rgb(c.background)));
    }
    if st.inverse {
        let dfg = colors.map(|c| rgb(c.foreground)).unwrap_or(Color::Reset);
        let dbg = colors.map(|c| rgb(c.background)).unwrap_or(Color::Reset);
        let (f, b) = (fg.unwrap_or(dfg), bg.unwrap_or(dbg));
        fg = Some(b);
        bg = Some(f);
    }
    let style = Style::default().fg(fg.unwrap_or(Color::Reset)).bg(bg.unwrap_or(Color::Reset));
    let mut m = Modifier::empty();
    if st.bold {
        m |= Modifier::BOLD;
    }
    if st.italic {
        m |= Modifier::ITALIC;
    }
    if st.faint {
        m |= Modifier::DIM;
    }
    if st.blink {
        m |= Modifier::SLOW_BLINK;
    }
    if st.underline != Underline::None {
        m |= Modifier::UNDERLINED;
    }
    if st.strikethrough {
        m |= Modifier::CROSSED_OUT;
    }
    style.add_modifier(m)
}

fn reset_row(buf: &mut Buffer, area: Rect, y: u16, from: u16) {
    for x in from..area.width {
        buf[(area.x + x, area.y + y)].reset();
    }
}

impl TermSurface {
    /// Draw into `area`, resizing the terminal to fit. Returns the cursor
    /// position (absolute) when it is visible.
    pub fn render(&mut self, buf: &mut Buffer, area: Rect) -> Option<(u16, u16)> {
        if area.width == 0 || area.height == 0 {
            return None;
        }
        self.resize(area.width, area.height);
        self.ack();
        let mut guard = self.core();
        let Core { term, rs, rows, cells, .. } = &mut *guard;
        let Ok(snap) = rs.update(term) else { return None };
        let colors = snap.colors().ok();
        let mut cursor = None;
        if snap.cursor_visible().unwrap_or(false) {
            if let Ok(Some(v)) = snap.cursor_viewport() {
                if v.x < area.width && v.y < area.height {
                    cursor = Some((area.x + v.x, area.y + v.y));
                }
            }
        }
        let mut text = String::new();
        let mut y = 0u16;
        if let Ok(mut row_iter) = rows.update(&snap) {
            while y < area.height {
                let Some(row) = row_iter.next() else { break };
                let mut x = 0u16;
                if let Ok(mut cell_iter) = cells.update(row) {
                    while x < area.width {
                        let Some(cell) = cell_iter.next() else { break };
                        let raw = cell.raw_cell().ok();
                        let symbol: &str = match raw.and_then(|r| r.wide().ok()) {
                            Some(CellWide::SpacerTail) => "",
                            Some(CellWide::SpacerHead) => " ",
                            _ => {
                                text.clear();
                                if cell.graphemes_utf8(&mut text).is_err() || text.is_empty() {
                                    " "
                                } else {
                                    text.as_str()
                                }
                            }
                        };
                        let st = cell.style().unwrap_or_default();
                        let style = style_for(&st, cell_bg(&st, raw), colors.as_ref());
                        let c = &mut buf[(area.x + x, area.y + y)];
                        c.reset();
                        c.set_symbol(symbol);
                        c.set_style(style);
                        x += 1;
                    }
                }
                let _ = row.set_dirty(false);
                reset_row(buf, area, y, x);
                y += 1;
            }
        }
        while y < area.height {
            reset_row(buf, area, y, 0);
            y += 1;
        }
        let _ = snap.set_dirty(libghostty_vt::render::Dirty::Clean);
        cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn renders_colours_and_text() {
        let mut t = TermSurface::stream(10, 2, Arc::new(|| {})).unwrap();
        t.feeder(true).feed(b"\x1b[31mred\x1b[0m ok\n\x1b[1mbold");
        let area = Rect::new(2, 1, 10, 2);
        let mut buf = Buffer::empty(Rect::new(0, 0, 14, 4));
        let cursor = t.render(&mut buf, area);
        let row: String = (2..12).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert_eq!(row, "red ok    ");
        assert_eq!(buf[(2, 1)].fg, Color::Indexed(1));
        assert!(buf[(2, 2)].modifier.contains(Modifier::BOLD));
        assert_eq!(cursor, Some((6, 2)));
    }

    #[test]
    fn wide_chars_take_two_cells() {
        let mut t = TermSurface::stream(6, 1, Arc::new(|| {})).unwrap();
        t.feeder(false).feed("日本".as_bytes());
        let area = Rect::new(0, 0, 6, 1);
        let mut buf = Buffer::empty(area);
        t.render(&mut buf, area);
        assert_eq!(buf[(0, 0)].symbol(), "日");
        assert_eq!(buf[(2, 0)].symbol(), "本");
    }
}
