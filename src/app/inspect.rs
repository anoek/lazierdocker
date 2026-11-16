//! Inspector tabs: which tabs exist per item kind, what content each needs,
//! and keeping pane content in sync with the selection.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::{App, Content, Effect, PaneKind};
use crate::doc::{graph, DocSurface};
use crate::docker::model::Snapshot;
use crate::docker::present;
use crate::docker::stats::StatsHistory;
use crate::ui::layout::PaneId;
use crate::ui::sidebar::{NodeKey, Section};
use crate::ui::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tab {
    Summary,
    Logs,
    Stats,
    Env,
    Config,
    Top,
}

impl Tab {
    pub fn title(self) -> &'static str {
        match self {
            Tab::Summary => "Summary",
            Tab::Logs => "Logs",
            Tab::Stats => "Stats",
            Tab::Env => "Env",
            Tab::Config => "Config",
            Tab::Top => "Top",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    Section,
    Project,
    Service,
    Container,
    Image,
    Volume,
    Network,
}

pub fn kind_of(key: &NodeKey) -> TargetKind {
    match key {
        NodeKey::Section(_) => TargetKind::Section,
        NodeKey::Project(_) => TargetKind::Project,
        NodeKey::Service(..) => TargetKind::Service,
        NodeKey::Container(_) => TargetKind::Container,
        NodeKey::Image(_) => TargetKind::Image,
        NodeKey::Volume(_) => TargetKind::Volume,
        NodeKey::Network(_) => TargetKind::Network,
    }
}

/// Tabs per kind (lazydocker §2; sections get a summary table).
pub fn tabs_for(kind: TargetKind) -> &'static [Tab] {
    match kind {
        TargetKind::Section => &[Tab::Summary],
        TargetKind::Project => &[Tab::Logs, Tab::Config],
        TargetKind::Service | TargetKind::Container => &[Tab::Logs, Tab::Stats, Tab::Env, Tab::Config, Tab::Top],
        TargetKind::Image | TargetKind::Volume | TargetKind::Network => &[Tab::Config],
    }
}

/// Identifies what a pane's content was loaded for. `rev` changes when the
/// target changes in a way that should reload the content.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentKey {
    pub target: NodeKey,
    pub tab: Tab,
    pub rev: u64,
}

/// What the runtime must fetch to fill a pane.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadSpec {
    ContainerConfig { id: String },
    ContainerEnv { id: String },
    Image { id: String },
    Volume { name: String },
    Network { id: String },
    ComposeConfig { project: String, local: bool, config_files: Option<String>, working_dir: Option<String> },
    Top { id: String },
}

impl App {
    /// The item a pane shows.
    pub fn pane_target(&self, id: PaneId) -> Option<NodeKey> {
        match &self.panes.get(&id)?.kind {
            PaneKind::Inspector => self.selected().cloned(),
            PaneKind::Pinned(k) => Some(k.clone()),
            PaneKind::Terminal { .. } => None,
        }
    }

    pub fn pane_tab(&self, id: PaneId) -> Option<Tab> {
        let target = self.pane_target(id)?;
        let kind = kind_of(&target);
        let tabs = tabs_for(kind);
        let pane = self.panes.get(&id)?;
        Some(pane.tabs.get(&kind).copied().filter(|t| tabs.contains(t)).unwrap_or(tabs[0]))
    }

    pub fn cycle_tab(&mut self, id: PaneId, delta: i32) {
        let (Some(target), Some(cur)) = (self.pane_target(id), self.pane_tab(id)) else { return };
        let kind = kind_of(&target);
        let tabs = tabs_for(kind);
        let i = tabs.iter().position(|t| *t == cur).unwrap_or(0) as i32;
        let next = tabs[(i + delta).rem_euclid(tabs.len() as i32) as usize];
        self.set_tab(id, next);
    }

    pub fn set_tab(&mut self, id: PaneId, tab: Tab) {
        let Some(target) = self.pane_target(id) else { return };
        if let Some(p) = self.panes.get_mut(&id) {
            p.tabs.insert(kind_of(&target), tab);
        }
        self.dirty = true;
    }

    /// The pane shows a container, service or project's Logs tab.
    pub fn shows_logs(&self, id: PaneId) -> bool {
        self.pane_tab(id) == Some(Tab::Logs)
            && matches!(self.pane_target(id), Some(NodeKey::Container(_) | NodeKey::Service(..) | NodeKey::Project(_)))
    }

    /// Reload a logs pane with the whole history instead of the last lines.
    pub fn load_full_logs(&mut self, id: PaneId) {
        if !self.shows_logs(id) {
            self.flash("full history is only for the Logs tab");
            return;
        }
        if let Some(p) = self.panes.get_mut(&id) {
            p.full_logs = true;
            p.reload = true;
            self.dirty = true;
        }
    }

    /// Container backing a target (containers and services).
    pub fn target_container(&self, key: &NodeKey) -> Option<&crate::docker::model::Container> {
        match key {
            NodeKey::Container(id) => self.snap.container(id),
            NodeKey::Service(p, s) => {
                self.snap.service(p, s)?.container_id.as_deref().and_then(|id| self.snap.container(id))
            }
            _ => None,
        }
    }

    fn content_rev(&self, key: &NodeKey, tab: Tab) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match key {
            NodeKey::Section(_) => self.snap_generation.hash(&mut h),
            NodeKey::Container(_) | NodeKey::Service(..) => {
                if let Some(c) = self.target_container(key) {
                    c.id.hash(&mut h);
                    c.state.as_str().hash(&mut h);
                    c.exit_code.hash(&mut h);
                    if tab == Tab::Stats {
                        self.stats.get(&c.id).and_then(|s| s.samples.back()).map(|(t, _)| *t).hash(&mut h);
                        self.hits.content.iter().map(|(_, r)| r.width).max().hash(&mut h);
                    }
                } else {
                    0u8.hash(&mut h);
                }
            }
            NodeKey::Project(p) if tab == Tab::Config => {
                self.snap.project(p).map(|p| p.services.len()).hash(&mut h);
            }
            _ => {}
        }
        h.finish()
    }

    /// Bring every pane's content in line with what it should show. Returns
    /// effects for content the runtime must fetch or start.
    pub fn sync_panes(&mut self) -> Vec<Effect> {
        self.sync_active_pane();
        let mut effects = Vec::new();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            if matches!(self.panes.get(&id).map(|p| &p.kind), Some(PaneKind::Terminal { .. })) {
                continue;
            }
            let (Some(target), Some(tab)) = (self.pane_target(id), self.pane_tab(id)) else {
                if let Some(p) = self.panes.get_mut(&id) {
                    if p.loaded.is_some() || !matches!(p.content, Content::Doc(_)) {
                        p.stop_task();
                        p.loaded = None;
                        p.content = Content::Doc(DocSurface::message("Nothing selected"));
                    }
                }
                continue;
            };
            let key = ContentKey { rev: self.content_rev(&target, tab), target: target.clone(), tab };
            // A restart can be too quick for any snapshot to see the container
            // stop, leaving `key` unchanged; resume the ended stream instead.
            let resume_logs = tab == Tab::Logs
                && self.panes.get(&id).and_then(|p| p.logs_ended_at).is_some_and(|g| g < self.snap_generation)
                && self
                    .target_container(&target)
                    .is_some_and(|c| c.state == crate::docker::model::ContainerState::Running);
            let reload = resume_logs || self.panes.get(&id).is_some_and(|p| p.reload);
            if !reload && self.panes.get(&id).and_then(|p| p.loaded.as_ref()) == Some(&key) {
                // Top refreshes every second, one request at a time.
                let tick = self.tick;
                let due = self.panes.get(&id).is_some_and(|p| tab == Tab::Top && !p.inflight && p.refreshed_at < tick);
                if due {
                    if let Plan::Load(spec) = self.plan_content(&target, tab, id) {
                        let p = self.panes.get_mut(&id).expect("pane exists");
                        p.inflight = true;
                        p.refreshed_at = tick;
                        effects.push(Effect::Load { pane: id, key, spec });
                    }
                }
                continue;
            }
            let same_view = self
                .panes
                .get(&id)
                .and_then(|p| p.loaded.as_ref())
                .is_some_and(|k| k.target == key.target && k.tab == key.tab);
            if !same_view {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.full_logs = false;
                }
            }
            let plan = self.plan_content(&target, tab, id);
            if !same_view && self.selection.is_some_and(|s| s.pane == id) {
                self.selection = None;
            }
            let pane = self.panes.get_mut(&id).expect("pane exists");
            pane.loaded = Some(key.clone());
            pane.logs_ended_at = None;
            pane.reload = false;
            match plan {
                Plan::Lines(lines) => {
                    if same_view {
                        if let Content::Doc(d) = &mut pane.content {
                            d.set_lines(lines);
                        } else {
                            pane.content = Content::Doc(DocSurface::new(lines));
                        }
                    } else {
                        pane.stop_task();
                        pane.content = Content::Doc(DocSurface::new(lines));
                    }
                }
                Plan::Load(spec) => {
                    if !same_view || !matches!(pane.content, Content::Doc(_)) {
                        pane.stop_task();
                        pane.content = Content::Doc(DocSurface::message("Loading…"));
                    }
                    pane.inflight = true;
                    pane.refreshed_at = self.tick;
                    effects.push(Effect::Load { pane: id, key, spec });
                }
                Plan::Logs { container_id } => {
                    pane.stop_task();
                    pane.content = Content::Doc(DocSurface::message("Loading…"));
                    effects.push(Effect::StartLogs { pane: id, key, container_id, full: pane.full_logs });
                }
                Plan::Spawn { argv } => {
                    pane.stop_task();
                    pane.content = Content::Doc(DocSurface::message("Starting…"));
                    effects.push(Effect::SpawnTerm { pane: id, key: Some(key), argv });
                }
            }
            self.dirty = true;
        }
        effects
    }

    /// What a (target, tab) needs: immediate lines, a fetch, or a stream.
    fn plan_content(&self, target: &NodeKey, tab: Tab, pane: PaneId) -> Plan {
        let t = &self.theme;
        let msg = |s: &str| Plan::Lines(vec![Line::styled(s.to_string(), Style::default().fg(t.overlay0))]);
        match (target, tab) {
            (NodeKey::Section(s), _) => Plan::Lines(summary(*s, &self.snap, &self.cpu, t)),
            (NodeKey::Container(_) | NodeKey::Service(..), _) => {
                let Some(c) = self.target_container(target) else {
                    return msg("No logs to show; service is not associated with a container");
                };
                let id = c.id.clone();
                match tab {
                    Tab::Config => Plan::Load(LoadSpec::ContainerConfig { id }),
                    Tab::Env => Plan::Load(LoadSpec::ContainerEnv { id }),
                    Tab::Top if c.state == crate::docker::model::ContainerState::Running => {
                        Plan::Load(LoadSpec::Top { id })
                    }
                    Tab::Top => msg("container is not running"),
                    Tab::Logs => Plan::Logs { container_id: id },
                    Tab::Stats => {
                        let width =
                            self.hits.content.iter().find(|(p, _)| *p == pane).map(|(_, r)| r.width).unwrap_or(80);
                        Plan::Lines(stats_lines(self.stats.get(&id), c.is_running(), width as usize, t))
                    }
                    Tab::Summary => msg(""),
                }
            }
            (NodeKey::Image(id), _) => Plan::Load(LoadSpec::Image { id: id.clone() }),
            (NodeKey::Volume(name), _) => Plan::Load(LoadSpec::Volume { name: name.clone() }),
            (NodeKey::Network(id), _) => Plan::Load(LoadSpec::Network { id: id.clone() }),
            (NodeKey::Project(p), Tab::Logs) => {
                let full = self.panes.get(&pane).is_some_and(|p| p.full_logs);
                let tail = if full { "--tail=all" } else { "--tail=300" };
                Plan::Spawn {
                    argv: ["docker", "compose", "-p", p, "logs", tail, "--follow"]
                        .iter()
                        .map(|s| s.to_string())
                        .collect(),
                }
            }
            (NodeKey::Project(p), _) => {
                let project = self.snap.project(p);
                let local = project.is_some_and(|p| p.local);
                let label = |name: &str| {
                    self.snap
                        .containers
                        .iter()
                        .find(|c| c.project.as_deref() == Some(p.as_str()))
                        .and_then(|c| c.labels.get(name).cloned())
                };
                Plan::Load(LoadSpec::ComposeConfig {
                    project: p.clone(),
                    local,
                    config_files: label("com.docker.compose.project.config_files"),
                    working_dir: label("com.docker.compose.project.working_dir"),
                })
            }
        }
    }
}

enum Plan {
    Lines(Vec<Line<'static>>),
    Load(LoadSpec),
    Logs { container_id: String },
    Spawn { argv: Vec<String> },
}

/// Stats tab: CPU and memory graphs, PIDs, traffic, memory usage (lazydocker layout).
pub fn stats_lines(h: Option<&StatsHistory>, running: bool, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let dim = Style::default().fg(t.overlay0);
    let Some(h) = h.filter(|h| !h.samples.is_empty()) else {
        return vec![Line::styled(if running { "Waiting for stats…" } else { "container is not running" }, dim)];
    };
    let latest = h.latest().expect("non-empty");
    let span = graph::span_label(h.span());
    let cpu: Vec<f64> = h.samples.iter().map(|(_, s)| s.cpu_percent()).collect();
    let mem: Vec<f64> = h.samples.iter().map(|(_, s)| s.mem_percent()).collect();
    let mut out = vec![graph::caption(format!("CPU (%): {:.2} ({span})", latest.cpu_percent()), t.cyan)];
    out.extend(graph::bars(&cpu, width, 8, 1.0, t.cyan, dim));
    out.push(Line::raw(""));
    out.push(graph::caption(format!("Memory (%): {:.2} ({span})", latest.mem_percent()), t.green));
    out.extend(graph::bars(&mem, width, 8, 1.0, t.green, dim));
    out.push(Line::raw(""));
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<18}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::raw(v),
        ])
    };
    out.push(kv("PIDs:", latest.pids.to_string()));
    out.push(kv(
        "Memory:",
        format!(
            "{} / {}",
            present::size_binary(latest.mem_usage as i64),
            present::size_binary(latest.mem_limit as i64)
        ),
    ));
    out.push(kv("Traffic received:", present::size(latest.rx_bytes as i64)));
    out.push(kv("Traffic sent:", present::size(latest.tx_bytes as i64)));
    out
}

/// Section summary tables.
pub fn summary(
    section: Section,
    snap: &Snapshot,
    cpu: &std::collections::HashMap<String, f64>,
    t: &Theme,
) -> Vec<Line<'static>> {
    let head = |s: String| Line::styled(s, Style::default().fg(t.overlay1).add_modifier(Modifier::BOLD));
    let dim = Style::default().fg(t.overlay0);
    let mut out = Vec::new();
    match section {
        Section::Projects => {
            out.push(head(format!("{:<28} {:>9}  {}", "PROJECT", "RUNNING", "SERVICES")));
            for p in &snap.projects {
                let running = p
                    .services
                    .iter()
                    .filter(|s| {
                        s.container_id.as_deref().and_then(|id| snap.container(id)).is_some_and(|c| c.is_running())
                    })
                    .count();
                let names: Vec<&str> = p.services.iter().map(|s| s.name.as_str()).collect();
                out.push(Line::from(vec![
                    Span::raw(format!("{:<28} ", p.name)),
                    Span::raw(format!("{:>9}  ", format!("{running}/{}", p.services.len()))),
                    Span::styled(names.join(", "), dim),
                ]));
            }
        }
        Section::Containers => {
            out.push(head(format!("{:<11} {:<30} {:>8}  {:<28} {}", "STATE", "NAME", "CPU", "PORTS", "IMAGE")));
            for c in &snap.containers {
                let state = match present::substatus(c, t) {
                    Some(sub) => format!("{} {}", c.state.as_str(), sub.content),
                    None => c.state.as_str().to_string(),
                };
                let cpu = cpu.get(&c.id).map(|v| format!("{v:.2}%")).unwrap_or_default();
                out.push(Line::from(vec![
                    Span::styled(format!("{state:<11} "), Style::default().fg(present::state_color(c, t))),
                    Span::raw(format!("{:<30} ", c.name)),
                    Span::raw(format!("{cpu:>8}  ")),
                    Span::styled(format!("{:<28} ", present::ports(&c.ports)), Style::default().fg(t.yellow)),
                    Span::styled(c.image.clone(), Style::default().fg(t.magenta)),
                ]));
            }
        }
        Section::Images => {
            out.push(head(format!("{:<40} {:<20} {:>9}  {}", "NAME", "TAG", "SIZE", "ID")));
            for i in &snap.images {
                out.push(Line::from(vec![
                    Span::raw(format!("{:<40} ", i.name)),
                    Span::raw(format!("{:<20} ", i.tag)),
                    Span::raw(format!("{:>9}  ", present::size(i.size))),
                    Span::styled(i.short_id().to_string(), dim),
                ]));
            }
        }
        Section::Volumes => {
            out.push(head(format!("{:<10} {}", "DRIVER", "NAME")));
            for v in &snap.volumes {
                out.push(Line::from(vec![Span::styled(format!("{:<10} ", v.driver), dim), Span::raw(v.name.clone())]));
            }
        }
        Section::Networks => {
            out.push(head(format!("{:<10} {}", "DRIVER", "NAME")));
            for n in &snap.networks {
                out.push(Line::from(vec![Span::styled(format!("{:<10} ", n.driver), dim), Span::raw(n.name.clone())]));
            }
        }
    }
    if out.len() == 1 {
        out.push(Line::styled("none", dim));
    }
    out
}
