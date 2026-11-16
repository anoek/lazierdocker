//! Compose (geometry + hit map) and draw. The only place that writes `hits`.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use super::inspect::{kind_of, tabs_for};
use super::{App, Content, Focus, Hits, Mode, PaneKind};
use crate::ui::borders::{self, PaneFrame};
use crate::ui::layout::PaneId;
use crate::ui::sidebar::{NodeKey, RenderCtx};
use crate::ui::statusbar::{self, Right, Status};
use crate::ui::text::wrap;

/// How long a status-bar message stays.
const TOAST: Duration = Duration::from_secs(4);

type StatusHints<'a> = (Option<(&'a str, ratatui::style::Color)>, &'a [(&'a str, &'a str)]);

impl App {
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        self.render_buf(frame.buffer_mut(), area);
        if let Some((x, y)) = self.cursor {
            frame.set_cursor_position((x, y));
        }
    }

    pub fn render_buf(&mut self, buf: &mut Buffer, area: Rect) {
        self.size = (area.width, area.height);
        self.cursor = None;
        let mut hits = Hits::default();
        let t = self.theme;
        buf.set_style(area, Style::default().fg(t.text));
        if area.width < 20 || area.height < 4 {
            buf.set_stringn(area.x, area.y, "terminal too small", area.width as usize, Style::default().fg(t.red));
            self.hits = hits;
            return;
        }
        let status_area = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        let main = Rect::new(area.x, area.y, area.width, area.height - 1);

        let sidebar_w = if self.sidebar_visible { self.sidebar.width.min(main.width.saturating_sub(20)) } else { 0 };
        // The sidebar's last column is shared with the panes' left frame line.
        let sidebar_area = Rect::new(main.x, main.y, sidebar_w.saturating_sub(1), main.height);
        let pane_x = main.x + sidebar_w.saturating_sub(1);
        let pane_area = Rect::new(pane_x, main.y, main.right() - pane_x, main.height);
        hits.sidebar_area = sidebar_area;
        hits.pane_area = pane_area;

        if sidebar_w >= 3 {
            let ctx = RenderCtx {
                snap: &self.snap,
                cpu: &self.cpu,
                theme: &t,
                focused: self.focus == Focus::Sidebar,
                title: "lazierdocker",
                filter_editing: self.mode == Mode::Filter,
            };
            hits.sidebar = self.sidebar.render(buf, sidebar_area, &self.rows, &ctx);
            hits.sidebar_divider = Some(Rect::new(pane_area.x, pane_area.y, 1, pane_area.height));
        }

        // Panes.
        let rects = self.layout.rects(pane_area);
        let titles: Vec<String> = rects.iter().map(|(id, _)| self.pane_title(*id)).collect();
        let frames: Vec<PaneFrame<'_>> = rects
            .iter()
            .zip(titles.iter())
            .map(|((id, rect), title)| PaneFrame {
                rect: *rect,
                title,
                focused: self.focus == Focus::Panes && *id == self.layout.focus,
            })
            .collect();
        borders::draw(buf, pane_area, &frames, &t);
        if let Some(div) = hits.sidebar_divider {
            self.sidebar.draw_scrollbar(buf, div.x, hits.sidebar.list_area, self.rows.len());
        }
        for (id, rect) in &rects {
            let inner = borders::inner(pane_area, *rect);
            hits.panes.push((*id, *rect, inner));
            self.render_pane(buf, *id, inner, &mut hits);
        }
        hits.splits = self.layout.splits(pane_area);

        self.render_status(buf, status_area);
        self.hits = hits;
        self.render_overlay(buf, area);
    }

    pub fn pane_title(&self, id: PaneId) -> String {
        match self.panes.get(&id).map(|p| &p.kind) {
            Some(PaneKind::Inspector) => match self.selected() {
                Some(key) => self.target_label(key),
                None => "inspector".into(),
            },
            Some(PaneKind::Pinned(key)) => format!("◆ {}", self.target_label(key)),
            Some(PaneKind::Terminal { title }) => match self.panes.get(&id).map(|p| &p.content) {
                Some(Content::Term(t)) if t.exited().is_some() => format!("{title} (exited)"),
                _ => title.clone(),
            },
            None => String::new(),
        }
    }

    pub fn target_label(&self, key: &NodeKey) -> String {
        match key {
            NodeKey::Section(s) => s.title().to_string(),
            NodeKey::Project(p) => format!("project {p}"),
            NodeKey::Service(p, s) => format!("{p} › {s}"),
            NodeKey::Container(id) => {
                self.snap.container(id).map(|c| c.name.clone()).unwrap_or_else(|| "container (gone)".into())
            }
            NodeKey::Image(id) => self.snap.image(id).map(|i| i.display()).unwrap_or_else(|| "image (gone)".into()),
            NodeKey::Volume(n) => format!("volume {n}"),
            NodeKey::Network(id) => {
                self.snap.network(id).map(|n| format!("network {}", n.name)).unwrap_or_else(|| "network (gone)".into())
            }
        }
    }

    fn render_pane(&mut self, buf: &mut Buffer, id: PaneId, inner: Rect, hits: &mut Hits) {
        if inner.width == 0 || inner.height == 0 {
            return;
        }
        let t = self.theme;
        if !self.loaded {
            let msg = match &self.docker_error {
                Some(e) => format!("Cannot reach docker: {e}"),
                None => "Connecting to docker…".to_string(),
            };
            let width = inner.width.saturating_sub(2).max(1) as usize;
            for (i, line) in wrap(&msg, width).iter().take(inner.height as usize).enumerate() {
                buf.set_stringn(inner.x + 1, inner.y + i as u16, line, width, Style::default().fg(t.subtext0));
            }
            return;
        }
        let mut content_area = inner;
        if let (Some(target), Some(active)) = (self.pane_target(id), self.pane_tab(id)) {
            let tabs = tabs_for(kind_of(&target));
            let mut x = inner.x;
            for tab in tabs {
                let label = format!(" {} ", tab.title());
                let w = label.chars().count() as u16;
                if x + w > inner.right() {
                    break;
                }
                let style = if *tab == active {
                    Style::default().bg(t.accent).fg(t.panel_bg).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.overlay1)
                };
                buf.set_string(x, inner.y, &label, style);
                hits.tabs.push((Rect::new(x, inner.y, w, 1), id, *tab));
                x += w + 1;
            }
            content_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height.saturating_sub(1));
        }
        // One column of breathing room from the left frame.
        if content_area.width > 2 {
            content_area = Rect::new(content_area.x + 1, content_area.y, content_area.width - 1, content_area.height);
        }
        let focused = self.focus == Focus::Panes && self.layout.focus == id;
        if let Some(pane) = self.panes.get_mut(&id) {
            let interactive = matches!(pane.kind, PaneKind::Terminal { .. });
            match &mut pane.content {
                Content::Doc(d) => d.render(buf, content_area),
                Content::Term(term) => {
                    let cursor = term.render(buf, content_area);
                    if focused && interactive && term.exited().is_none() {
                        self.cursor = cursor;
                    }
                }
            }
        }
        hits.content.push((id, content_area));
        self.paint_find(buf, id, content_area);
        self.paint_selection(buf, id, content_area);
        self.paint_copy_cursor(buf, id, content_area);
    }

    fn paint_copy_cursor(&self, buf: &mut Buffer, id: PaneId, area: Rect) {
        let Some(c) = self.copy.filter(|c| c.pane == id) else { return };
        let top = self.content_top(id);
        let left = self.content_left(id);
        if c.pos.0 < top
            || c.pos.0 >= top + area.height as usize
            || c.pos.1 < left
            || c.pos.1 >= left + area.width as usize
        {
            return;
        }
        let t = self.theme;
        let cell = &mut buf[(area.x + (c.pos.1 - left) as u16, area.y + (c.pos.0 - top) as u16)];
        cell.set_style(Style::default().bg(t.accent).fg(t.panel_bg).add_modifier(Modifier::BOLD));
    }

    fn paint_find(&self, buf: &mut Buffer, id: PaneId, area: Rect) {
        let Some(f) = self.shown_find().filter(|f| f.pane == id) else { return };
        let t = self.theme;
        let origin = (self.content_top(id), self.content_left(id));
        let other = Style::default().bg(t.surface1).fg(t.yellow);
        let current = Style::default().bg(t.yellow).fg(t.panel_bg).add_modifier(Modifier::BOLD);
        let first = f.matches.partition_point(|m| m.end.0 < origin.0);
        for (i, m) in f.matches.iter().enumerate().skip(first) {
            if m.start.0 >= origin.0 + area.height as usize {
                break;
            }
            let style = if f.current == Some(i) { current } else { other };
            paint_range(buf, area, origin, m.start, m.end, style);
        }
    }

    fn paint_selection(&self, buf: &mut Buffer, id: PaneId, area: Rect) {
        let Some(sel) = self.selection.filter(|s| s.pane == id && s.is_visible()) else { return };
        let (start, end) = sel.ordered();
        let origin = (self.content_top(id), self.content_left(id));
        paint_range(buf, area, origin, start, end, Style::default().bg(self.theme.surface1).fg(self.theme.text));
    }

    fn render_status(&self, buf: &mut Buffer, area: Rect) {
        let t = self.theme;
        let (pill, hints): StatusHints<'_> = match self.mode {
            Mode::Prefix => (
                Some(("PREFIX", t.accent)),
                &[
                    ("v", "split →"),
                    ("-", "split ↓"),
                    ("x", "close"),
                    ("z", "zoom"),
                    ("hjkl", "focus"),
                    ("b", "sidebar"),
                    ("q", "quit"),
                    ("esc", "cancel"),
                ],
            ),
            Mode::Filter => (Some(("FILTER", t.mauve)), &[("enter", "keep"), ("esc", "clear")]),
            Mode::Find => (Some(("FIND", t.yellow)), &[("enter", "keep"), ("↑↓", "older/newer"), ("esc", "clear")]),
            Mode::Resize => (Some(("RESIZE", t.mauve)), &[("h/l", "width"), ("j/k", "height"), ("esc", "done")]),
            Mode::Copy => (
                Some(("COPY", t.yellow)),
                &[
                    ("hjkl", "move"),
                    ("w/b/e", "word"),
                    ("0/$", "line"),
                    ("g/G", "top/bottom"),
                    ("v/V", "select"),
                    ("y", "copy"),
                    ("q", "exit"),
                ],
            ),
            Mode::Normal => match self.focus {
                Focus::Sidebar => (
                    None,
                    &[
                        ("↑↓", "select"),
                        ("←→", "fold"),
                        ("/", "filter"),
                        ("e", "exited"),
                        ("enter", "pane"),
                        ("^b", "window"),
                        ("q", "quit"),
                    ],
                ),
                Focus::Panes
                    if matches!(
                        self.panes.get(&self.layout.focus).map(|p| &p.kind),
                        Some(PaneKind::Terminal { .. })
                    ) =>
                {
                    (
                        Some(("TERMINAL", t.green)),
                        &[("^b", "window"), ("^b x", "close"), ("^b h", "← focus"), ("^b z", "zoom")],
                    )
                }
                Focus::Panes if self.find_active_here() => (
                    Some(("FIND", t.yellow)),
                    &[("n ^g", "older"), ("N", "newer"), ("/", "new search"), ("esc", "clear")],
                ),
                Focus::Panes => (
                    None,
                    &[
                        ("tab", "sidebar"),
                        ("[ ]", "tabs"),
                        ("j/k", "scroll"),
                        ("m", "logs pane"),
                        ("E", "exec"),
                        ("^b", "window"),
                        ("q", "quit"),
                    ],
                ),
            },
        };
        let toast = self.toast.as_ref().filter(|(_, at)| at.elapsed() < TOAST);
        const SPIN: [&str; 4] = ["|", "/", "-", "\\"];
        let (text, error) = match (&self.docker_error, toast) {
            _ if !self.deferred.is_empty() => ("⚠ message waiting: leave the terminal (^b h) to see it".into(), true),
            _ if !self.pending_ops.is_empty() => {
                (format!("{} {}…", SPIN[(self.tick % 4) as usize], self.pending_ops.join(", ")), false)
            }
            (_, Some((msg, _))) => (format!("● {msg}"), false),
            (Some(e), None) => (format!("⚠ {e}"), true),
            (None, None) => (self.host.clone(), false),
        };
        let right = Right { text, error };
        let prompt = self.find_status();
        statusbar::render(buf, area, &Status { pill, prompt, hints, right }, &t);
    }
}

/// Style the stream range `start..=end` of content (rows, columns) drawn in
/// `area` with `origin` (top row, left column) at its top-left corner.
fn paint_range(
    buf: &mut Buffer,
    area: Rect,
    origin: (usize, usize),
    start: (usize, usize),
    end: (usize, usize),
    style: Style,
) {
    let (top, left) = origin;
    let (rows, cols) = (area.height as usize, area.width as usize);
    if rows == 0 || cols == 0 {
        return;
    }
    for row in start.0.max(top)..=end.0.min(top + rows - 1) {
        let from = if row == start.0 { start.1 } else { 0 }.max(left);
        let to = if row == end.0 { end.1 } else { usize::MAX }.min(left + cols - 1);
        for col in from..=to {
            buf[(area.x + (col - left) as u16, area.y + (row - top) as u16)].set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    #[test]
    fn paint_range_is_a_stream_selection() {
        let area = Rect::new(0, 0, 10, 4);
        let mark = Style::default().bg(Color::Red);
        let on = |buf: &Buffer, x: u16, y: u16| buf[(x, y)].bg == Color::Red;
        // Content rows 2..=4 show at screen rows 1..=3; columns from 0.
        let mut buf = Buffer::empty(area);
        paint_range(&mut buf, area, (1, 0), (2, 5), (4, 3), mark);
        assert!(on(&buf, 5, 1) && on(&buf, 9, 1) && !on(&buf, 4, 1), "rest of the first line");
        assert!(on(&buf, 0, 2) && on(&buf, 9, 2), "middle lines whole");
        assert!(on(&buf, 3, 3) && !on(&buf, 4, 3), "last line up to the end");
        assert!(!on(&buf, 0, 0), "rows outside the range");
        // Scrolled sideways by 4: column 5 shows at x 1.
        let mut buf = Buffer::empty(area);
        paint_range(&mut buf, area, (0, 4), (0, 5), (0, 6), mark);
        assert!(!on(&buf, 0, 0) && on(&buf, 1, 0) && on(&buf, 2, 0) && !on(&buf, 3, 0));
    }
}
