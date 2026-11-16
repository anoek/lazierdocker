//! Small line-free bar graphs for the Stats tab (lazydocker draws asciigraph
//! plots; we draw block columns which read well in any terminal).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

const BLOCKS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

/// Plot the last `width` values as columns `height` rows tall, scaled to
/// `[0, max]` where max is at least `floor_max`. Returns the rows top-down,
/// each prefixed by a y-axis label column.
pub fn bars(
    values: &[f64],
    width: usize,
    height: usize,
    floor_max: f64,
    color: Color,
    axis: Style,
) -> Vec<Line<'static>> {
    let height = height.max(1);
    let label_w = 8;
    let plot_w = width.saturating_sub(label_w + 1).max(1);
    let start = values.len().saturating_sub(plot_w);
    let shown = &values[start..];
    let max = shown.iter().copied().fold(floor_max, f64::max).max(f64::EPSILON);
    let mut rows = Vec::with_capacity(height);
    for r in 0..height {
        // Row r covers [lo, hi) in eighths, counted from the bottom.
        let level_from_bottom = height - 1 - r;
        let label = if r == 0 {
            format!("{max:>7.1} ")
        } else if r == height - 1 {
            format!("{:>7.1} ", 0.0)
        } else {
            " ".repeat(label_w)
        };
        let mut s = String::with_capacity(plot_w * 3);
        // Right-align the data so the newest sample is at the right edge.
        for _ in shown.len()..plot_w {
            s.push(' ');
        }
        for v in shown {
            let eighths = ((v.max(0.0) / max) * (height * 8) as f64).round() as usize;
            let in_row = eighths.saturating_sub(level_from_bottom * 8).min(8);
            s.push_str(BLOCKS[in_row]);
        }
        rows.push(Line::from(vec![
            Span::styled(label, axis),
            Span::styled("│", axis),
            Span::styled(s, Style::default().fg(color)),
        ]));
    }
    rows
}

pub fn caption(text: String, color: Color) -> Line<'static> {
    Line::styled(text, Style::default().fg(color).add_modifier(Modifier::BOLD))
}

/// `3m0s`-style duration.
pub fn span_label(d: std::time::Duration) -> String {
    let s = d.as_secs();
    if s >= 60 {
        format!("{}m{}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line<'_>) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn graph_scales_to_height() {
        let rows = bars(&[0.0, 50.0, 100.0], 12, 2, 0.0, Color::Cyan, Style::default());
        assert_eq!(rows.len(), 2);
        // plot width = 12 - 9 = 3; 100 → full both rows, 50 → bottom row full.
        assert_eq!(text(&rows[0]), "  100.0 │  █");
        assert_eq!(text(&rows[1]), "    0.0 │ ██");
    }

    #[test]
    fn graph_floor_max_and_truncation() {
        let rows = bars(&[1.0; 50], 14, 1, 8.0, Color::Cyan, Style::default());
        assert_eq!(text(&rows[0]), "    8.0 │▁▁▁▁▁");
    }

    #[test]
    fn span_labels() {
        assert_eq!(span_label(std::time::Duration::from_secs(180)), "3m0s");
        assert_eq!(span_label(std::time::Duration::from_secs(42)), "42s");
    }
}
