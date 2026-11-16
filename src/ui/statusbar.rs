//! Bottom status line: mode pill, key hints, docker host / error.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use unicode_width::UnicodeWidthStr;

use crate::ui::text::truncate;
use crate::ui::theme::Theme;

pub struct Status<'a> {
    pub pill: Option<(&'a str, Color)>,
    /// Text being typed or a search summary, shown after the pill.
    pub prompt: Option<String>,
    pub hints: &'a [(&'a str, &'a str)],
    pub right: Right,
}

/// Right-aligned status: the docker host, a message, or an error.
pub struct Right {
    pub text: String,
    pub error: bool,
}

pub fn render(buf: &mut Buffer, area: Rect, s: &Status<'_>, t: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    buf.set_style(area, Style::default().bg(t.panel_bg));
    let mut spans = Vec::new();
    if let Some((label, color)) = s.pill {
        spans.push(Span::styled(
            format!(" {label} "),
            Style::default().bg(color).fg(t.panel_bg).add_modifier(Modifier::BOLD),
        ));
    }
    spans.push(Span::raw(" "));
    if let Some(p) = &s.prompt {
        spans.push(Span::styled(p.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw("   "));
    }
    for (i, (key, desc)) in s.hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(key.to_string(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(format!(" {desc}"), Style::default().fg(t.overlay1)));
    }
    let left = Line::from(spans);
    // The right side never takes more than half the bar, so hints stay visible.
    let right = truncate(&s.right.text, (area.width / 2) as usize);
    let right_w = right.width() as u16 + 1;
    let left_max = area.width.saturating_sub(right_w);
    buf.set_line(area.x, area.y, &left, left_max);
    let style = if s.right.error {
        Style::default().fg(t.red).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.overlay0)
    };
    buf.set_stringn(area.right() - right_w, area.y, &right, right_w as usize, style.bg(t.panel_bg));
}
