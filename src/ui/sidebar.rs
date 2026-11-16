//! Collapsible resource tree (herdr's sidebar technique, lazydocker's resources).
//!
//! Collapsed state is a set of keys, not stored on nodes; the visible tree is
//! flattened into rows each frame.

use std::collections::{HashMap, HashSet};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::docker::model::{Container, ContainerState, Snapshot};
use crate::docker::present;
use crate::ui::borders::truncate;
use crate::ui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Section {
    Projects,
    Containers,
    Images,
    Volumes,
    Networks,
}

impl Section {
    pub const ALL: [Section; 5] =
        [Section::Projects, Section::Containers, Section::Images, Section::Volumes, Section::Networks];

    pub fn title(self) -> &'static str {
        match self {
            Section::Projects => "Projects",
            Section::Containers => "Containers",
            Section::Images => "Images",
            Section::Volumes => "Volumes",
            Section::Networks => "Networks",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeKey {
    Section(Section),
    Project(String),
    /// (project, service)
    Service(String, String),
    /// container id
    Container(String),
    /// image id
    Image(String),
    /// volume name
    Volume(String),
    /// network id
    Network(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub key: NodeKey,
    pub depth: u8,
    pub expandable: bool,
    pub expanded: bool,
    pub last_child: bool,
    /// Text used for filtering and as the default label.
    pub label: String,
}

#[derive(Debug, Clone, Default)]
pub struct SidebarHits {
    pub rows: Vec<(Rect, NodeKey)>,
    pub toggles: Vec<(Rect, NodeKey)>,
    pub list_area: Rect,
}

#[derive(Debug, Clone)]
pub struct Sidebar {
    pub collapsed: HashSet<NodeKey>,
    pub selected: Option<NodeKey>,
    /// Index of the selection in the last computed rows, used to pick a
    /// neighbour when the selected item disappears.
    pub selected_index: usize,
    pub scroll: usize,
    pub filter: String,
    pub show_exited: bool,
    pub width: u16,
}

pub const DEFAULT_WIDTH: u16 = 40;
pub const MIN_WIDTH: u16 = 18;
pub const MAX_WIDTH: u16 = 80;

impl Default for Sidebar {
    fn default() -> Self {
        let mut collapsed = HashSet::new();
        collapsed.insert(NodeKey::Section(Section::Images));
        collapsed.insert(NodeKey::Section(Section::Volumes));
        collapsed.insert(NodeKey::Section(Section::Networks));
        Self {
            collapsed,
            selected: None,
            selected_index: 0,
            scroll: 0,
            filter: String::new(),
            show_exited: true,
            width: DEFAULT_WIDTH,
        }
    }
}

pub fn section_of(key: &NodeKey) -> Section {
    match key {
        NodeKey::Section(s) => *s,
        NodeKey::Project(_) | NodeKey::Service(..) => Section::Projects,
        NodeKey::Container(_) => Section::Containers,
        NodeKey::Image(_) => Section::Images,
        NodeKey::Volume(_) => Section::Volumes,
        NodeKey::Network(_) => Section::Networks,
    }
}

fn container_visible(c: &Container, show_exited: bool) -> bool {
    show_exited || c.state != ContainerState::Exited
}

impl Sidebar {
    fn is_expanded(&self, key: &NodeKey) -> bool {
        !self.filter.is_empty() || !self.collapsed.contains(key)
    }

    /// Flatten the visible tree.
    pub fn rows(&self, snap: &Snapshot) -> Vec<Row> {
        // Build the full tree first as (row, children) then filter/collapse.
        struct Item {
            row: Row,
            children: Vec<Item>,
        }
        let leaf = |key: NodeKey, depth: u8, label: String| Item {
            row: Row { key, depth, expandable: false, expanded: false, last_child: false, label },
            children: vec![],
        };
        let mut sections = Vec::new();
        for section in Section::ALL {
            let children: Vec<Item> = match section {
                Section::Projects => snap
                    .projects
                    .iter()
                    .map(|p| Item {
                        row: Row {
                            key: NodeKey::Project(p.name.clone()),
                            depth: 1,
                            expandable: true,
                            expanded: false,
                            last_child: false,
                            label: p.name.clone(),
                        },
                        children: p
                            .services
                            .iter()
                            .map(|s| leaf(NodeKey::Service(p.name.clone(), s.name.clone()), 2, s.name.clone()))
                            .collect(),
                    })
                    .collect(),
                Section::Containers => snap
                    .standalone_containers()
                    .filter(|c| container_visible(c, self.show_exited))
                    .map(|c| leaf(NodeKey::Container(c.id.clone()), 1, c.name.clone()))
                    .collect(),
                Section::Images => {
                    snap.images.iter().map(|i| leaf(NodeKey::Image(i.id.clone()), 1, i.display())).collect()
                }
                Section::Volumes => {
                    snap.volumes.iter().map(|v| leaf(NodeKey::Volume(v.name.clone()), 1, v.name.clone())).collect()
                }
                Section::Networks => {
                    snap.networks.iter().map(|n| leaf(NodeKey::Network(n.id.clone()), 1, n.name.clone())).collect()
                }
            };
            sections.push(Item {
                row: Row {
                    key: NodeKey::Section(section),
                    depth: 0,
                    expandable: true,
                    expanded: false,
                    last_child: false,
                    label: section.title().to_string(),
                },
                children,
            });
        }

        let needle = self.filter.to_lowercase();
        fn matches(item: &Item, needle: &str) -> bool {
            needle.is_empty()
                || item.row.label.to_lowercase().contains(needle)
                || item.children.iter().any(|c| matches(c, needle))
        }
        fn emit(item: Item, sb: &Sidebar, needle: &str, out: &mut Vec<Row>, is_last: bool) {
            let Item { mut row, children } = item;
            row.last_child = is_last;
            row.expanded = row.expandable && sb.is_expanded(&row.key);
            let expanded = row.expanded;
            let depth = row.depth;
            out.push(row);
            if expanded {
                // While filtering, a matching parent shows all its children.
                let self_match =
                    !needle.is_empty() && out.last().is_some_and(|r| r.label.to_lowercase().contains(needle));
                let kids: Vec<Item> = children
                    .into_iter()
                    .filter(|c| depth == 0 && needle.is_empty() || self_match && depth > 0 || matches(c, needle))
                    .collect();
                let n = kids.len();
                for (i, c) in kids.into_iter().enumerate() {
                    emit(c, sb, needle, out, i + 1 == n);
                }
            }
        }
        let mut out = Vec::new();
        for s in sections {
            // Sections stay visible while filtering so the tree keeps its shape.
            emit(s, self, &needle, &mut out, false);
        }
        out
    }

    /// Re-anchor the selection after a refresh: keep the key if still visible,
    /// otherwise select the row now at the previous index.
    pub fn reconcile(&mut self, rows: &[Row]) {
        if rows.is_empty() {
            self.selected = None;
            self.selected_index = 0;
            return;
        }
        if let Some(sel) = &self.selected {
            if let Some(i) = rows.iter().position(|r| &r.key == sel) {
                self.selected_index = i;
                return;
            }
        }
        // The item vanished: pick the nearest row at or before its old position within
        // the same section, falling back to that section's header.
        let i = match self.selected.as_ref().map(section_of) {
            Some(section) => {
                let start = rows.iter().position(|r| r.key == NodeKey::Section(section)).unwrap_or(0);
                let end =
                    rows[start + 1..].iter().position(|r| r.depth == 0).map(|p| start + 1 + p).unwrap_or(rows.len());
                if end > start + 1 {
                    self.selected_index.clamp(start + 1, end - 1)
                } else {
                    start
                }
            }
            None => self.selected_index.min(rows.len() - 1),
        };
        self.selected = Some(rows[i].key.clone());
        self.selected_index = i;
    }

    pub fn move_selection(&mut self, rows: &[Row], delta: i32) {
        if rows.is_empty() {
            return;
        }
        let cur = self.selected.as_ref().and_then(|k| rows.iter().position(|r| &r.key == k)).unwrap_or(0) as i64;
        let next = (cur + delta as i64).clamp(0, rows.len() as i64 - 1) as usize;
        self.selected = Some(rows[next].key.clone());
        self.selected_index = next;
    }

    pub fn select(&mut self, rows: &[Row], key: &NodeKey) {
        if let Some(i) = rows.iter().position(|r| &r.key == key) {
            self.selected = Some(key.clone());
            self.selected_index = i;
        }
    }

    pub fn toggle(&mut self, key: &NodeKey) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.clone());
        }
    }

    pub fn set_expanded(&mut self, key: &NodeKey, expanded: bool) {
        if expanded {
            self.collapsed.remove(key);
        } else {
            self.collapsed.insert(key.clone());
        }
    }

    /// Parent key of a row (for collapse-on-left).
    pub fn parent_of(rows: &[Row], key: &NodeKey) -> Option<NodeKey> {
        let i = rows.iter().position(|r| &r.key == key)?;
        let depth = rows[i].depth;
        rows[..i].iter().rev().find(|r| r.depth < depth).map(|r| r.key.clone())
    }

    /// Make sure the selection is within the visible window.
    pub fn ensure_visible(&mut self, height: usize) {
        if height == 0 {
            return;
        }
        if self.selected_index < self.scroll {
            self.scroll = self.selected_index;
        } else if self.selected_index >= self.scroll + height {
            self.scroll = self.selected_index + 1 - height;
        }
    }

    pub fn scroll_by(&mut self, rows: usize, height: usize, delta: i32) {
        let max = rows.saturating_sub(height);
        self.scroll = (self.scroll as i64 + delta as i64).clamp(0, max as i64) as usize;
    }
}

pub struct RenderCtx<'a> {
    pub snap: &'a Snapshot,
    pub cpu: &'a HashMap<String, f64>,
    pub theme: &'a Theme,
    pub focused: bool,
    pub title: &'a str,
    pub filter_editing: bool,
}

impl Sidebar {
    /// Render into `area` (content only; the divider belongs to the pane frames).
    pub fn render(&mut self, buf: &mut Buffer, area: Rect, rows: &[Row], ctx: &RenderCtx<'_>) -> SidebarHits {
        let t = ctx.theme;
        let mut hits = SidebarHits::default();
        if area.width < 2 || area.height < 2 {
            return hits;
        }
        // The divider is drawn by the pane frames to our right; we own [x, right).
        let inner = area;
        let div_x = area.right() - 1;
        // Header.
        let header = if ctx.filter_editing || !self.filter.is_empty() {
            Line::from(vec![
                Span::styled(" / ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(self.filter.clone(), Style::default().fg(t.text)),
                Span::styled(if ctx.filter_editing { "▏" } else { "" }, Style::default().fg(t.accent)),
            ])
        } else {
            Line::from(vec![Span::styled(
                format!(" {}", ctx.title),
                Style::default().fg(t.overlay1).add_modifier(Modifier::BOLD),
            )])
        };
        buf.set_line(inner.x, inner.y, &header, inner.width);
        let list = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
        hits.list_area = list;
        let height = list.height as usize;
        self.scroll = self.scroll.min(rows.len().saturating_sub(height));

        for (i, row) in rows.iter().enumerate().skip(self.scroll).take(height) {
            let y = list.y + (i - self.scroll) as u16;
            let rect = Rect::new(list.x, y, list.width, 1);
            let selected = self.selected.as_ref() == Some(&row.key);
            let base = if selected {
                if ctx.focused {
                    Style::default().bg(t.surface0).fg(t.text).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().bg(t.active_row_bg).fg(t.text)
                }
            } else {
                Style::default().fg(t.subtext0)
            };
            buf.set_style(rect, base);
            let (spans, right, toggle_col) = row_spans(row, ctx);
            let mut line = Line::from(spans);
            line = line.patch_style(base.remove_modifier(Modifier::empty()));
            // Right-aligned extra (cpu / size / driver).
            let right_w =
                right.as_ref().map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()) as u16).unwrap_or(0);
            let left_max = if right_w > 0 && list.width > right_w + 12 { list.width - right_w - 1 } else { list.width };
            buf.set_line(list.x, y, &line, left_max);
            if let Some(r) = right {
                if left_max < list.width {
                    let mut s = r;
                    if selected {
                        s.style = s.style.bg(if ctx.focused { t.surface0 } else { t.active_row_bg });
                    }
                    buf.set_span(list.right() - right_w, y, &s, right_w);
                }
            }
            hits.rows.push((rect, row.key.clone()));
            if let Some(col) = toggle_col {
                hits.toggles.push((Rect::new(list.x + col, y, 2, 1), row.key.clone()));
            }
        }
        // Scrollbar hint when content overflows.
        if rows.len() > height && height > 0 {
            let thumb_h = ((height * height) / rows.len()).max(1);
            let max_scroll = rows.len() - height;
            let thumb_y = (self.scroll * (height - thumb_h)).checked_div(max_scroll).unwrap_or(0);
            for dy in 0..height {
                let on = dy >= thumb_y && dy < thumb_y + thumb_h;
                buf[(div_x, list.y + dy as u16)]
                    .set_symbol(if on { "┃" } else { "│" })
                    .set_style(Style::default().fg(if on { t.overlay0 } else { t.surface1 }));
            }
        }
        hits
    }
}

/// Returns (left spans, optional right-aligned span, toggle column offset).
fn row_spans(row: &Row, ctx: &RenderCtx<'_>) -> (Vec<Span<'static>>, Option<Span<'static>>, Option<u16>) {
    let t = ctx.theme;
    let snap = ctx.snap;
    let caret = |expanded: bool| if expanded { "▾ " } else { "▸ " };
    let dim = Style::default().fg(t.overlay0);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut right = None;
    let mut toggle = None;
    match &row.key {
        NodeKey::Section(s) => {
            toggle = Some(0);
            spans.push(Span::styled(caret(row.expanded).to_string(), Style::default().fg(t.accent)));
            spans.push(Span::styled(s.title().to_string(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)));
            let count = match s {
                Section::Projects => snap.projects.len(),
                Section::Containers => snap.standalone_containers().count(),
                Section::Images => snap.images.len(),
                Section::Volumes => snap.volumes.len(),
                Section::Networks => snap.networks.len(),
            };
            spans.push(Span::styled(format!(" {count}"), dim));
        }
        NodeKey::Project(name) => {
            toggle = Some(1);
            spans.push(Span::raw(" "));
            spans.push(Span::styled(caret(row.expanded).to_string(), Style::default().fg(t.accent)));
            let project = snap.project(name);
            spans.push(Span::styled(name.clone(), Style::default().fg(t.text)));
            if let Some(p) = project {
                let running = p
                    .services
                    .iter()
                    .filter(|s| {
                        s.container_id.as_deref().and_then(|id| snap.container(id)).is_some_and(|c| c.is_running())
                    })
                    .count();
                spans.push(Span::styled(format!(" {running}/{}", p.services.len()), dim));
                if p.local {
                    spans.push(Span::styled(" ●", Style::default().fg(t.mauve)));
                }
            }
        }
        NodeKey::Service(project, name) => {
            let prefix = if row.last_child { "   └ " } else { "   ├ " };
            spans.push(Span::styled(prefix, Style::default().fg(t.surface1)));
            let c =
                snap.service(project, name).and_then(|s| s.container_id.as_deref()).and_then(|id| snap.container(id));
            match c {
                Some(c) => {
                    spans.push(Span::styled(
                        present::state_icon(c.state),
                        Style::default().fg(present::state_color(c, t)),
                    ));
                    spans.push(Span::raw(" "));
                    spans.push(Span::raw(name.clone()));
                    if let Some(sub) = present::substatus(c, t) {
                        spans.push(Span::raw(" "));
                        spans.push(sub);
                    }
                    if c.state == ContainerState::Running {
                        right = ctx.cpu.get(&c.id).map(|v| present::cpu_span(*v, t));
                    }
                }
                None => {
                    spans.push(Span::styled("·", Style::default().fg(t.blue)));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(name.clone(), dim));
                }
            }
        }
        NodeKey::Container(id) => {
            spans.push(Span::raw("  "));
            if let Some(c) = snap.container(id) {
                spans.push(Span::styled(present::state_icon(c.state), Style::default().fg(present::state_color(c, t))));
                spans.push(Span::raw(" "));
                spans.push(Span::raw(c.name.clone()));
                if let Some(sub) = present::substatus(c, t) {
                    spans.push(Span::raw(" "));
                    spans.push(sub);
                }
                if c.state == ContainerState::Running {
                    right = ctx.cpu.get(&c.id).map(|v| present::cpu_span(*v, t));
                }
            }
        }
        NodeKey::Image(id) => {
            spans.push(Span::raw("  "));
            if let Some(i) = snap.images.iter().find(|i| &i.id == id) {
                spans.push(Span::raw(i.name.clone()));
                if !i.tag.is_empty() {
                    spans.push(Span::styled(format!(":{}", i.tag), dim));
                }
                right = Some(Span::styled(present::size(i.size), dim));
            }
        }
        NodeKey::Volume(name) => {
            spans.push(Span::raw("  "));
            spans.push(Span::raw(truncate(name, 48)));
            if let Some(v) = snap.volumes.iter().find(|v| &v.name == name) {
                right = Some(Span::styled(v.driver.clone(), dim));
            }
        }
        NodeKey::Network(id) => {
            spans.push(Span::raw("  "));
            if let Some(n) = snap.networks.iter().find(|n| &n.id == id) {
                spans.push(Span::raw(n.name.clone()));
                right = Some(Span::styled(n.driver.clone(), dim));
            }
        }
    }
    (spans, right, toggle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::model::tests::container;
    use crate::docker::model::{derive_projects, Image, Network, Volume};

    fn snap() -> Snapshot {
        let containers = vec![
            container("demo-web-1", ContainerState::Running, Some(("demo", "web"))),
            container("demo-db-1", ContainerState::Running, Some(("demo", "db"))),
            container("loner", ContainerState::Running, None),
            container("exited", ContainerState::Exited, None),
        ];
        Snapshot {
            projects: derive_projects(&containers, None),
            containers,
            images: vec![Image {
                id: "sha256:1".into(),
                name: "nginx".into(),
                tag: "alpine".into(),
                size: 1,
                created: 0,
                containers: 1,
            }],
            volumes: vec![Volume { name: "demo_dbdata".into(), driver: "local".into(), labelled: true }],
            networks: vec![Network { id: "n1".into(), name: "bridge".into(), driver: "bridge".into() }],
        }
    }

    fn keys(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|r| match &r.key {
                NodeKey::Section(s) => format!("#{}", s.title()),
                _ => r.label.clone(),
            })
            .collect()
    }

    #[test]
    fn default_tree_shape() {
        let sb = Sidebar::default();
        let rows = sb.rows(&snap());
        assert_eq!(
            keys(&rows),
            vec![
                "#Projects",
                "demo",
                "db",
                "web",
                "#Containers",
                "loner",
                "exited",
                "#Images",
                "#Volumes",
                "#Networks"
            ]
        );
        assert!(rows[3].last_child);
    }

    #[test]
    fn collapsed_section_hides_children() {
        let mut sb = Sidebar::default();
        sb.toggle(&NodeKey::Project("demo".into()));
        sb.toggle(&NodeKey::Section(Section::Images));
        let rows = sb.rows(&snap());
        assert_eq!(
            keys(&rows),
            vec![
                "#Projects",
                "demo",
                "#Containers",
                "loner",
                "exited",
                "#Images",
                "nginx:alpine",
                "#Volumes",
                "#Networks"
            ]
        );
        assert!(!rows[1].expanded);
    }

    #[test]
    fn filter_matches_case_insensitive() {
        let mut sb = Sidebar::default();
        sb.filter = "WEB".into();
        let rows = sb.rows(&snap());
        assert_eq!(keys(&rows), vec!["#Projects", "demo", "web", "#Containers", "#Images", "#Volumes", "#Networks"]);
        sb.filter = "demo".into();
        let rows = sb.rows(&snap());
        // A matching parent shows all its children; collapsed sections open while filtering.
        assert_eq!(
            keys(&rows),
            vec!["#Projects", "demo", "db", "web", "#Containers", "#Images", "#Volumes", "demo_dbdata", "#Networks"]
        );
    }

    #[test]
    fn show_exited_toggle() {
        let mut sb = Sidebar::default();
        sb.show_exited = false;
        let rows = sb.rows(&snap());
        assert!(!keys(&rows).contains(&"exited".to_string()));
    }

    #[test]
    fn selection_survives_removal() {
        let mut sb = Sidebar::default();
        let mut s = snap();
        let rows = sb.rows(&s);
        let loner = rows.iter().find(|r| r.label == "loner").unwrap().key.clone();
        sb.select(&rows, &loner);
        // loner removed
        s.containers.retain(|c| c.name != "loner");
        let rows = sb.rows(&s);
        sb.reconcile(&rows);
        assert_eq!(rows[sb.selected_index].label, "exited");
        // section emptied → its header
        s.containers.retain(|c| c.name != "exited");
        let rows = sb.rows(&s);
        sb.reconcile(&rows);
        assert_eq!(sb.selected, Some(NodeKey::Section(Section::Containers)));
    }

    #[test]
    fn move_selection_clamps() {
        let mut sb = Sidebar::default();
        let rows = sb.rows(&snap());
        sb.move_selection(&rows, -5);
        assert_eq!(sb.selected_index, 0);
        sb.move_selection(&rows, 100);
        assert_eq!(sb.selected_index, rows.len() - 1);
        assert_eq!(
            Sidebar::parent_of(&rows, &NodeKey::Service("demo".into(), "web".into())),
            Some(NodeKey::Project("demo".into()))
        );
    }

    #[test]
    fn render_rows_and_hits() {
        let mut sb = Sidebar::default();
        let s = snap();
        let rows = sb.rows(&s);
        let cpu = HashMap::new();
        let theme = Theme::default();
        let area = Rect::new(0, 0, 30, 8);
        let mut buf = Buffer::empty(area);
        let ctx =
            RenderCtx { snap: &s, cpu: &cpu, theme: &theme, focused: true, title: "docker", filter_editing: false };
        let hits = sb.render(&mut buf, area, &rows, &ctx);
        assert_eq!(hits.rows.len(), 7, "header takes one row");
        let line1: String = (0..29).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert!(line1.starts_with("▾ Projects 1"), "{line1:?}");
        assert_eq!(buf[(29, 3)].symbol(), "┃", "scrollbar thumb on divider");
        assert_eq!(hits.toggles[0].1, NodeKey::Section(Section::Projects));
    }
}
