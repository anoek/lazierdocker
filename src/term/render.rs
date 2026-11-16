//! Draw a libghostty terminal into a ratatui buffer.

use ghostty_vt::{CellColor, CellStyle, CellWide, RenderColors, RgbColor};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use super::{Core, TermSurface};

fn rgb(c: RgbColor) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

fn cell_color(c: CellColor) -> Color {
    match c {
        CellColor::Palette(i) => Color::Indexed(i),
        CellColor::Rgb(c) => rgb(c),
    }
}

fn style_for(st: &CellStyle, fg: Option<Color>, bg: Option<Color>, colors: Option<&RenderColors>) -> Style {
    let mut fg = st.fg_color.map(cell_color).or(fg);
    let mut bg = st.bg_color.map(cell_color).or(bg);
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
    let mut style = Style::default().fg(fg.unwrap_or(Color::Reset)).bg(bg.unwrap_or(Color::Reset));
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
    if st.underlined {
        m |= Modifier::UNDERLINED;
    }
    if st.strikethrough {
        m |= Modifier::CROSSED_OUT;
    }
    style = style.add_modifier(m);
    style
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
        let mut guard = self.core.lock().expect("term core");
        let Core { term, rs, ri, rc, .. } = &mut *guard;
        if rs.update(term).is_err() {
            return None;
        }
        let colors = rs.colors().ok();
        let mut cursor = None;
        if let Ok(cur) = rs.cursor() {
            if let (true, Some(v)) = (cur.visible, cur.viewport) {
                if v.x < area.width && v.y < area.height {
                    cursor = Some((area.x + v.x, area.y + v.y));
                }
            }
        }
        let mut gbytes = Vec::new();
        let mut text = String::new();
        let mut y = 0u16;
        if let Ok(mut rows) = rs.populate_row_iterator(ri) {
            while y < area.height && rows.next() {
                let Ok(mut cells) = rows.populate_cells(rc) else { break };
                let mut x = 0u16;
                while x < area.width && cells.next() {
                    let basic = cells.basic_data().unwrap_or_default();
                    let symbol: &str = match basic.wide {
                        CellWide::SpacerTail => "",
                        CellWide::SpacerHead => " ",
                        _ => {
                            if cells.grapheme_text_into(&mut gbytes, &mut text).is_err() || text.is_empty() {
                                " "
                            } else {
                                text.as_str()
                            }
                        }
                    };
                    let bg = match cells.content_bg_color() {
                        Ok(Some(c)) => Some(cell_color(c)),
                        _ => cells.bg_color().ok().flatten().map(rgb),
                    };
                    let fg = cells.fg_color().ok().flatten().map(rgb);
                    let style = style_for(&basic.style, fg, bg, colors.as_ref());
                    let cell = &mut buf[(area.x + x, area.y + y)];
                    cell.reset();
                    cell.set_symbol(symbol);
                    cell.set_style(style);
                    x += 1;
                }
                while x < area.width {
                    buf[(area.x + x, area.y + y)].reset();
                    x += 1;
                }
                y += 1;
            }
        }
        while y < area.height {
            for x in 0..area.width {
                buf[(area.x + x, area.y + y)].reset();
            }
            y += 1;
        }
        let _ = rs.clean();
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
