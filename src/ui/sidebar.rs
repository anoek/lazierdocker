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
use crate::ui::text::truncate;
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
    /// The parent row is its group's last child (for tree guide lines).
    pub parent_last: bool,
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
    /// Groups the user expanded or collapsed away from their default.
    toggled: HashSet<NodeKey>,
    pub selected: Option<NodeKey>,
    /// Index of the selection in the last computed rows, used to pick a
    /// neighbour when the selected item disappears.
    pub selected_index: usize,
    /// Section of the selection, for the same purpose.
    selected_section: Option<Section>,
    pub scroll: usize,
    /// The selection (key and row) last scrolled into view (see `render`).
    revealed: Option<(NodeKey, usize)>,
    pub filter: String,
    pub show_exited: bool,
    pub width: u16,
}

pub const DEFAULT_WIDTH: u16 = 40;
pub const MIN_WIDTH: u16 = 18;
pub const MAX_WIDTH: u16 = 80;

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            toggled: HashSet::new(),
            selected: None,
            selected_index: 0,
            selected_section: None,
            scroll: 0,
            revealed: None,
            filter: String::new(),
            show_exited: true,
            width: DEFAULT_WIDTH,
        }
    }
}

/// Groups start expanded, except the long resource lists and replicated
/// services (whose row already shows their replicas combined).
fn expanded_by_default(key: &NodeKey) -> bool {
    !matches!(key, NodeKey::Section(Section::Images | Section::Volumes | Section::Networks) | NodeKey::Service(..))
}

/// (project, service) of a container listed under its replicated service.
fn replica_of<'a>(id: &str, snap: &'a Snapshot) -> Option<(&'a str, &'a str)> {
    let c = snap.container(id)?;
    let (p, s) = (c.project.as_deref()?, c.service.as_deref()?);
    let svc = snap.service(p, s)?;
    (svc.is_replicated() && svc.replicas.iter().any(|r| r == id)).then_some((p, s))
}

/// The groups holding `key`'s row, outermost first (none for a section).
pub fn ancestors(key: &NodeKey, snap: &Snapshot) -> Vec<NodeKey> {
    let section = |s| NodeKey::Section(s);
    match key {
        NodeKey::Section(_) => vec![],
        NodeKey::Project(_) => vec![section(Section::Projects)],
        NodeKey::Service(p, _) => vec![section(Section::Projects), NodeKey::Project(p.clone())],
        NodeKey::Container(id) => match replica_of(id, snap) {
            Some((p, s)) => vec![
                section(Section::Projects),
                NodeKey::Project(p.to_string()),
                NodeKey::Service(p.to_string(), s.to_string()),
            ],
            None => vec![section(Section::Containers)],
        },
        NodeKey::Image(_) => vec![section(Section::Images)],
        NodeKey::Volume(_) => vec![section(Section::Volumes)],
        NodeKey::Network(_) => vec![section(Section::Networks)],
    }
}

/// The section a row is in.
fn section_at(rows: &[Row], i: usize) -> Option<Section> {
    rows[..=i.min(rows.len().checked_sub(1)?)].iter().rev().find_map(|r| match r.key {
        NodeKey::Section(s) => Some(s),
        _ => None,
    })
}

/// Standalone containers the Containers section lists (`e` hides exited ones).
fn visible_containers(snap: &Snapshot, show_exited: bool) -> impl Iterator<Item = &Container> {
    snap.standalone_containers().filter(move |c| show_exited || c.state != ContainerState::Exited)
}

/// A node of the full resource tree, before filtering and collapsing.
struct TreeItem {
    row: Row,
    children: Vec<TreeItem>,
}

impl TreeItem {
    fn leaf(key: NodeKey, depth: u8, label: String) -> Self {
        let row = Row { key, depth, expandable: false, expanded: false, last_child: false, parent_last: false, label };
        Self { row, children: Vec::new() }
    }

    fn branch(key: NodeKey, depth: u8, label: String, children: Vec<TreeItem>) -> Self {
        let mut item = Self::leaf(key, depth, label);
        item.row.expandable = true;
        item.children = children;
        item
    }

    /// The item or one of its descendants matches the (lowercase) filter.
    fn matches(&self, needle: &str) -> bool {
        needle.is_empty()
            || self.row.label.to_lowercase().contains(needle)
            || self.children.iter().any(|c| c.matches(needle))
    }
}

/// Every section with its items (Projects ▸ services; the rest flat).
fn tree(snap: &Snapshot, show_exited: bool) -> Vec<TreeItem> {
    let leaf = TreeItem::leaf;
    Section::ALL
        .into_iter()
        .map(|section| {
            let children = match section {
                Section::Projects => snap
                    .projects
                    .iter()
                    .map(|p| {
                        let services = p
                            .services
                            .iter()
                            .map(|s| {
                                let key = NodeKey::Service(p.name.clone(), s.name.clone());
                                if !s.is_replicated() {
                                    return leaf(key, 2, s.name.clone());
                                }
                                let replicas = snap
                                    .replicas(s)
                                    .filter(|c| show_exited || c.state != ContainerState::Exited)
                                    .map(|c| leaf(NodeKey::Container(c.id.clone()), 3, c.name.clone()))
                                    .collect();
                                TreeItem::branch(key, 2, s.name.clone(), replicas)
                            })
                            .collect();
                        TreeItem::branch(NodeKey::Project(p.name.clone()), 1, p.name.clone(), services)
                    })
                    .collect(),
                Section::Containers => visible_containers(snap, show_exited)
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
            TreeItem::branch(NodeKey::Section(section), 0, section.title().to_string(), children)
        })
        .collect()
}

impl Sidebar {
    fn is_expanded(&self, key: &NodeKey) -> bool {
        !self.filter.is_empty() || expanded_by_default(key) != self.toggled.contains(key)
    }

    /// Flatten the visible tree.
    pub fn rows(&self, snap: &Snapshot) -> Vec<Row> {
        let needle = self.filter.to_lowercase();
        let mut out = Vec::new();
        // Sections stay visible while filtering so the tree keeps its shape.
        for section in tree(snap, self.show_exited) {
            self.emit(section, &needle, false, false, &mut out);
        }
        out
    }

    fn emit(&self, item: TreeItem, needle: &str, last_child: bool, parent_last: bool, out: &mut Vec<Row>) {
        let TreeItem { mut row, children } = item;
        row.last_child = last_child;
        row.parent_last = parent_last;
        row.expanded = row.expandable && self.is_expanded(&row.key);
        // While filtering, a matching group (not a section) shows all its children.
        let show_all = row.depth > 0 && !needle.is_empty() && row.label.to_lowercase().contains(needle);
        let expanded = row.expanded;
        out.push(row);
        if !expanded {
            return;
        }
        let kids: Vec<TreeItem> = children.into_iter().filter(|c| show_all || c.matches(needle)).collect();
        let n = kids.len();
        for (i, c) in kids.into_iter().enumerate() {
            self.emit(c, needle, i + 1 == n, last_child, out);
        }
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
                self.selected_section = section_at(rows, i);
                return;
            }
        }
        // The item vanished: pick the row now at its old position, kept within
        // the same section, falling back to that section's header.
        let i = match self.selected.as_ref().and(self.selected_section) {
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
        self.set_selection(rows, i);
    }

    fn set_selection(&mut self, rows: &[Row], i: usize) {
        self.selected = Some(rows[i].key.clone());
        self.selected_index = i;
        self.selected_section = section_at(rows, i);
    }

    pub fn move_selection(&mut self, rows: &[Row], delta: i32) {
        if rows.is_empty() {
            return;
        }
        let cur = self.selected.as_ref().and_then(|k| rows.iter().position(|r| &r.key == k)).unwrap_or(0) as i64;
        let next = (cur + delta as i64).clamp(0, rows.len() as i64 - 1) as usize;
        self.set_selection(rows, next);
    }

    pub fn select(&mut self, rows: &[Row], key: &NodeKey) {
        if let Some(i) = rows.iter().position(|r| &r.key == key) {
            self.set_selection(rows, i);
        }
    }

    pub fn toggle(&mut self, key: &NodeKey) {
        if !self.toggled.remove(key) {
            self.toggled.insert(key.clone());
        }
    }

    /// Expand or collapse a group; true if that changed it.
    pub fn set_expanded(&mut self, key: &NodeKey, expanded: bool) -> bool {
        if expanded == expanded_by_default(key) {
            self.toggled.remove(key)
        } else {
            self.toggled.insert(key.clone())
        }
    }

    /// Parent key of a row (for collapse-on-left).
    pub fn parent_of(rows: &[Row], key: &NodeKey) -> Option<NodeKey> {
        let i = rows.iter().position(|r| &r.key == key)?;
        let depth = rows[i].depth;
        rows[..i].iter().rev().find(|r| r.depth < depth).map(|r| r.key.clone())
    }

    /// Make sure the selection is within the visible window.
    fn ensure_visible(&mut self, height: usize) {
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
        buf.set_line(area.x, area.y, &header, area.width);
        let list = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
        hits.list_area = list;
        let height = list.height as usize;
        // Scroll to the selection whenever it or its row changes, however that
        // happened (keys, filter, new rows above it, a focused pane); the wheel
        // changes neither, so it can still scroll the selection out of view.
        let current = self.selected.clone().map(|k| (k, self.selected_index));
        if current != self.revealed {
            self.ensure_visible(height);
            self.revealed = current;
        }
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
            let (spans, right, toggle_col) = row_spans(row, ctx, self.show_exited);
            let line = Line::from(spans).patch_style(base);
            // Right-aligned extra (cpu / size / driver), dropped when the row is narrow.
            let right_w = right.as_ref().map_or(0, |s| s.width() as u16);
            let left_max = if right_w > 0 && list.width > right_w + 12 { list.width - right_w - 1 } else { list.width };
            buf.set_line(list.x, y, &line, left_max);
            if let Some(mut r) = right.filter(|_| left_max < list.width) {
                if let Some(bg) = base.bg {
                    r.style = r.style.bg(bg);
                }
                buf.set_span(list.x + list.width - right_w, y, &r, right_w);
            }
            hits.rows.push((rect, row.key.clone()));
            if let Some(col) = toggle_col {
                hits.toggles.push((Rect::new(list.x + col, y, 2, 1), row.key.clone()));
            }
        }
        hits
    }

    /// Draw the list's scroll thumb on the divider at column `x` (drawn by the
    /// pane frames), leaving its junction glyphs alone.
    pub fn draw_scrollbar(&self, buf: &mut Buffer, x: u16, list: Rect, rows: usize) {
        let height = list.height as usize;
        if rows <= height || height == 0 || !buf.area.contains((x, list.y).into()) {
            return;
        }
        let thumb_h = ((height * height) / rows).max(1);
        let thumb_y = self.scroll * (height - thumb_h) / (rows - height);
        for dy in thumb_y..(thumb_y + thumb_h).min(height) {
            let cell = &mut buf[(x, list.y + dy as u16)];
            if cell.symbol() == "│" {
                cell.set_symbol("┃");
            }
        }
    }
}

/// Icon, name and substatus of a container row; returns its CPU span when running.
fn container_spans(
    c: &Container,
    name: String,
    ctx: &RenderCtx<'_>,
    spans: &mut Vec<Span<'static>>,
) -> Option<Span<'static>> {
    let t = ctx.theme;
    spans.push(Span::styled(present::state_icon(c.state), Style::default().fg(present::state_color(c, t))));
    spans.push(Span::raw(" "));
    spans.push(Span::raw(name));
    if let Some(sub) = present::substatus(c, t) {
        spans.push(Span::raw(" "));
        spans.push(sub);
    }
    if c.state == ContainerState::Running {
        ctx.cpu.get(&c.id).map(|v| present::cpu_span(*v, t))
    } else {
        None
    }
}

/// Returns (left spans, optional right-aligned span, toggle column offset).
fn row_spans(
    row: &Row,
    ctx: &RenderCtx<'_>,
    show_exited: bool,
) -> (Vec<Span<'static>>, Option<Span<'static>>, Option<u16>) {
    let t = ctx.theme;
    let snap = ctx.snap;
    let caret = |expanded: bool| if expanded { "▾ " } else { "▸ " };
    let dim = Style::default().fg(t.overlay0);
    let guide = Style::default().fg(t.surface1);
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
                Section::Containers => visible_containers(snap, show_exited).count(),
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
                let running = p.services.iter().filter(|s| snap.primary(s).is_some_and(|c| c.is_running())).count();
                spans.push(Span::styled(format!(" {running}/{}", p.services.len()), dim));
                if p.local {
                    spans.push(Span::styled(" ●", Style::default().fg(t.mauve)));
                }
            }
        }
        NodeKey::Service(project, name) => {
            let prefix = if row.last_child { "   └ " } else { "   ├ " };
            spans.push(Span::styled(prefix, guide));
            let svc = snap.service(project, name);
            if row.expandable {
                // Replicated: a group of replica rows, with their running count and total CPU.
                toggle = Some(prefix.chars().count() as u16);
                spans.push(Span::styled(caret(row.expanded).to_string(), Style::default().fg(t.accent)));
                spans.push(Span::styled(name.clone(), Style::default().fg(t.text)));
                if let Some(svc) = svc {
                    let replicas: Vec<&Container> = snap.replicas(svc).collect();
                    let running = replicas.iter().filter(|c| c.is_running()).count();
                    spans.push(Span::styled(format!(" {running}/{}", replicas.len()), dim));
                    let cpu: Vec<f64> = replicas
                        .iter()
                        .filter(|c| c.state == ContainerState::Running)
                        .filter_map(|c| ctx.cpu.get(&c.id).copied())
                        .collect();
                    if !cpu.is_empty() {
                        right = Some(present::cpu_span(cpu.iter().sum(), t));
                    }
                }
                return (spans, right, toggle);
            }
            match svc.and_then(|s| snap.primary(s)) {
                Some(c) => right = container_spans(c, name.clone(), ctx, &mut spans),
                None => {
                    spans.push(Span::styled("·", Style::default().fg(t.blue)));
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(name.clone(), dim));
                }
            }
        }
        NodeKey::Container(id) => {
            if row.depth == 3 {
                // A replica, under its service.
                let stem = if row.parent_last { "     " } else { "   │ " };
                let branch = if row.last_child { "└ " } else { "├ " };
                spans.push(Span::styled(format!("{stem}{branch}"), guide));
            } else {
                spans.push(Span::raw("  "));
            }
            if let Some(c) = snap.container(id) {
                right = container_spans(c, c.name.clone(), ctx, &mut spans);
            }
        }
        NodeKey::Image(id) => {
            spans.push(Span::raw("  "));
            if let Some(i) = snap.image(id) {
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
            if let Some(n) = snap.network(id) {
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
            images: vec![Image { id: "sha256:1".into(), name: "nginx".into(), tag: "alpine".into(), size: 1 }],
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

    /// demo: web scaled to 3 (replica 3 exited), db single.
    fn replicated() -> Snapshot {
        use crate::docker::model::tests::replica;
        let containers = vec![
            replica("demo", "web", 1, ContainerState::Running),
            replica("demo", "web", 2, ContainerState::Running),
            replica("demo", "web", 3, ContainerState::Exited),
            replica("demo", "db", 1, ContainerState::Running),
        ];
        Snapshot { projects: derive_projects(&containers, None), containers, ..Default::default() }
    }

    #[test]
    fn replicated_services_expand_to_their_replicas() {
        let s = replicated();
        let mut sb = Sidebar::default();
        let rows = sb.rows(&s);
        let web = NodeKey::Service("demo".into(), "web".into());
        let row = rows.iter().find(|r| r.key == web).unwrap();
        assert!(row.expandable && !row.expanded, "replica groups start collapsed");
        assert!(!rows.iter().find(|r| r.label == "db").unwrap().expandable, "one container: a plain row");
        assert!(!keys(&rows).contains(&"demo-web-1".to_string()));
        sb.toggle(&web);
        let rows = sb.rows(&s);
        let labels = keys(&rows);
        let i = labels.iter().position(|l| l == "web").unwrap();
        assert_eq!(labels[i + 1..i + 4], ["demo-web-1", "demo-web-2", "demo-web-3"]);
        assert!(rows[i + 1..i + 4].iter().all(|r| r.depth == 3 && matches!(r.key, NodeKey::Container(_))));
        assert!(rows[i + 3].last_child);
        // `e` hides exited replicas too; the group stays a group.
        sb.show_exited = false;
        let labels = keys(&sb.rows(&s));
        assert!(labels.contains(&"demo-web-2".to_string()) && !labels.contains(&"demo-web-3".to_string()));
        // Filtering opens groups and matches replica names.
        let mut sb = Sidebar::default();
        sb.filter = "web-2".into();
        assert!(keys(&sb.rows(&s)).contains(&"demo-web-2".to_string()));
    }

    #[test]
    fn replica_rows_draw_tree_guides_and_the_group_sums_cpu() {
        let s = replicated();
        let mut sb = Sidebar::default();
        sb.toggle(&NodeKey::Service("demo".into(), "web".into()));
        let rows = sb.rows(&s);
        let cpu = HashMap::from([(s.containers[0].id.clone(), 1.5), (s.containers[1].id.clone(), 2.0)]);
        let theme = Theme::default();
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        let ctx = RenderCtx { snap: &s, cpu: &cpu, theme: &theme, focused: false, title: "t", filter_editing: false };
        sb.render(&mut buf, area, &rows, &ctx);
        let text = |y: u16| (0..40).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
        let all: Vec<String> = (1..10).map(text).collect();
        let web = all.iter().find(|l| l.contains(" web ")).unwrap();
        assert!(web.contains("▾ web 2/3") && web.trim_end().ends_with("3.50%"), "{all:#?}");
        // web is the project's last service, so no stem continues past it.
        assert!(
            all.iter().any(|l| l.starts_with("     ├ ▶ demo-web-1") && l.trim_end().ends_with("1.50%")),
            "{all:#?}"
        );
        assert!(all.iter().any(|l| l.starts_with("     └ ⨯ demo-web-3")), "{all:#?}");
    }

    #[test]
    fn ancestors_of_a_replica_are_its_section_project_and_service() {
        let s = replicated();
        let web = |n: usize| NodeKey::Container(s.containers[n].id.clone());
        let groups = ancestors(&web(1), &s);
        assert_eq!(groups.last(), Some(&NodeKey::Service("demo".into(), "web".into())));
        assert_eq!(groups[0], NodeKey::Section(Section::Projects));
        let db = NodeKey::Container(s.containers[3].id.clone());
        assert_eq!(ancestors(&db, &s), vec![NodeKey::Section(Section::Containers)], "not a listed replica");
    }

    #[test]
    fn a_vanished_replica_leaves_the_selection_in_projects() {
        let mut s = replicated();
        let mut sb = Sidebar::default();
        sb.toggle(&NodeKey::Service("demo".into(), "web".into()));
        let rows = sb.rows(&s);
        let gone = NodeKey::Container(s.containers[2].id.clone());
        sb.select(&rows, &gone);
        s.containers.remove(2);
        s.projects = derive_projects(&s.containers, None);
        let rows = sb.rows(&s);
        sb.reconcile(&rows);
        let i = sb.selected_index;
        assert_eq!(section_at(&rows, i), Some(Section::Projects), "{:?}", rows[i].key);
    }

    #[test]
    fn render_rows_and_hits() {
        let mut sb = Sidebar::default();
        let s = snap();
        let rows = sb.rows(&s);
        let loner = s.containers.iter().find(|c| c.name == "loner").unwrap().id.clone();
        let cpu = HashMap::from([(loner, 12.5)]);
        let theme = Theme::default();
        let area = Rect::new(0, 0, 30, 8);
        let mut buf = Buffer::empty(area);
        let ctx =
            RenderCtx { snap: &s, cpu: &cpu, theme: &theme, focused: true, title: "docker", filter_editing: false };
        let hits = sb.render(&mut buf, area, &rows, &ctx);
        assert_eq!(hits.rows.len(), 7, "header takes one row");
        let line1: String = (0..29).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert!(line1.starts_with("▾ Projects 1"), "{line1:?}");
        let loner_row: String = (0..30).map(|x| buf[(x, 6)].symbol().to_string()).collect();
        assert!(loner_row.trim_end().ends_with("12.50%"), "right-aligned CPU: {loner_row:?}");
        assert_eq!(hits.toggles[0].1, NodeKey::Section(Section::Projects));
    }

    #[test]
    fn scroll_thumb_sits_on_the_divider() {
        let sb = Sidebar::default();
        let area = Rect::new(0, 0, 3, 10);
        let mut buf = Buffer::empty(area);
        for y in 0..10 {
            buf[(2, y)].set_symbol(if y == 5 { "├" } else { "│" });
        }
        let list = Rect::new(0, 1, 2, 8);
        sb.draw_scrollbar(&mut buf, 2, list, 16);
        let col: Vec<&str> = (0..10).map(|y| buf[(2, y)].symbol()).collect();
        assert_eq!(col, ["│", "┃", "┃", "┃", "┃", "├", "│", "│", "│", "│"], "thumb at the top, junction kept");
    }
}
