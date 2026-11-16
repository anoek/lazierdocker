//! Inspector tabs: which tabs exist per item kind, what content each needs,
//! and keeping pane content in sync with the selection.

use ratatui::style::Style;
use ratatui::text::Line;

use super::{App, Content, Effect, PaneKind};
use crate::doc::format::{self, Lines};
use crate::doc::DocSurface;
use crate::docker::model::{Container, ContainerState};
use crate::ui::layout::PaneId;
use crate::ui::sidebar::{NodeKey, Section};

/// Lines of history a container's logs open with (`f` loads everything).
pub const LOG_TAIL_LINES: usize = 1000;
/// Lines of history per service a project's logs open with.
pub const PROJECT_LOG_TAIL_LINES: usize = 300;

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
    ContainerConfig {
        id: String,
    },
    ContainerEnv {
        id: String,
    },
    Image {
        id: String,
    },
    Volume {
        name: String,
    },
    Network {
        id: String,
    },
    /// `docker compose … config`, shown as YAML.
    ComposeConfig {
        argv: Vec<String>,
    },
    Top {
        id: String,
    },
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
        let tabs = tabs_for(kind_of(&target));
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

    /// The pane shows a Logs tab (containers, services and projects have one).
    pub fn shows_logs(&self, id: PaneId) -> bool {
        self.pane_tab(id) == Some(Tab::Logs)
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
    pub fn target_container(&self, key: &NodeKey) -> Option<&Container> {
        match key {
            NodeKey::Container(id) => self.snap.container(id),
            NodeKey::Service(p, s) => {
                self.snap.service(p, s)?.container_id.as_deref().and_then(|id| self.snap.container(id))
            }
            _ => None,
        }
    }

    /// Changes when the target changes in a way that should reload `tab`.
    fn content_rev(&self, pane: PaneId, key: &NodeKey, tab: Tab) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match key {
            NodeKey::Section(s) => {
                self.snap_generation.hash(&mut h);
                // The containers table shows live CPU.
                if *s == Section::Containers {
                    self.tick.hash(&mut h);
                }
            }
            NodeKey::Container(_) | NodeKey::Service(..) => {
                if let Some(c) = self.target_container(key) {
                    c.id.hash(&mut h);
                    c.state.as_str().hash(&mut h);
                    c.exit_code.hash(&mut h);
                    if tab == Tab::Stats {
                        self.stats.get(&c.id).and_then(|s| s.samples.back()).map(|(t, _)| *t).hash(&mut h);
                        self.content_size(pane).0.hash(&mut h);
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
    /// effects for content the runtime must fetch or start. Runs after every
    /// event.
    pub fn sync_panes(&mut self) -> Vec<Effect> {
        self.sync_active_pane();
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        let effects = ids.into_iter().filter_map(|id| self.sync_pane(id)).collect();
        self.refresh_find();
        effects
    }

    fn sync_pane(&mut self, id: PaneId) -> Option<Effect> {
        if matches!(self.panes.get(&id)?.kind, PaneKind::Terminal { .. }) {
            return None;
        }
        let (Some(target), Some(tab)) = (self.pane_target(id), self.pane_tab(id)) else {
            self.clear_pane(id);
            return None;
        };
        let key = ContentKey { rev: self.content_rev(id, &target, tab), target, tab };
        if self.is_current(id, &key) {
            self.refresh_top(id, key)
        } else {
            self.reload_pane(id, key)
        }
    }

    /// Nothing to show (no selection).
    fn clear_pane(&mut self, id: PaneId) {
        if let Some(p) = self.panes.get_mut(&id) {
            if p.loaded.is_some() || !matches!(p.content, Content::Doc(_)) {
                p.loaded = None;
                p.set_content(Content::Doc(DocSurface::message("Nothing selected")));
                self.forget_view(id);
            }
        }
    }

    /// The pane shows `key` and needs no restart.
    fn is_current(&self, id: PaneId, key: &ContentKey) -> bool {
        let Some(p) = self.panes.get(&id) else { return true };
        if p.reload || p.loaded.as_ref() != Some(key) {
            return false;
        }
        // A restart can be too quick for any snapshot to see the container
        // stop, leaving `key` unchanged; resume the ended stream instead.
        let resume_logs = key.tab == Tab::Logs
            && p.logs_ended_at.is_some_and(|g| g < self.snap_generation)
            && self.target_container(&key.target).is_some_and(|c| c.state == ContainerState::Running);
        !resume_logs
    }

    /// Top refreshes every second, one request at a time.
    fn refresh_top(&mut self, id: PaneId, key: ContentKey) -> Option<Effect> {
        let p = self.panes.get(&id)?;
        if key.tab != Tab::Top || p.inflight || p.refreshed_at >= self.tick {
            return None;
        }
        let Plan::Load(spec) = self.plan_content(&key.target, key.tab, id) else { return None };
        let tick = self.tick;
        let p = self.panes.get_mut(&id)?;
        p.inflight = true;
        p.refreshed_at = tick;
        Some(Effect::Load { pane: id, key, spec })
    }

    /// Show `key` in the pane: a different view, or the same one restarted.
    fn reload_pane(&mut self, id: PaneId, key: ContentKey) -> Option<Effect> {
        let same_view =
            self.panes.get(&id)?.loaded.as_ref().is_some_and(|k| k.target == key.target && k.tab == key.tab);
        if !same_view {
            self.panes.get_mut(&id)?.full_logs = false;
        }
        let plan = self.plan_content(&key.target, key.tab, id);
        // New content has new rows: positions in the old content are meaningless.
        if !same_view || matches!(plan, Plan::Logs { .. } | Plan::Spawn { .. }) {
            self.forget_view(id);
        }
        let tick = self.tick;
        self.dirty = true;
        let pane = self.panes.get_mut(&id)?;
        pane.loaded = Some(key.clone());
        pane.logs_ended_at = None;
        pane.reload = false;
        let message = |text: &str| Content::Doc(DocSurface::message(text));
        match plan {
            Plan::Lines(lines) if same_view => pane.show_lines(lines),
            Plan::Lines(lines) => pane.set_content(Content::Doc(DocSurface::new(lines))),
            Plan::Load(spec) => {
                // Reloading the same view keeps the old document until the new one arrives.
                if !same_view || !matches!(pane.content, Content::Doc(_)) {
                    pane.set_content(message("Loading…"));
                }
                pane.inflight = true;
                pane.refreshed_at = tick;
                return Some(Effect::Load { pane: id, key, spec });
            }
            Plan::Logs { container_id } => {
                pane.set_content(message("Loading…"));
                return Some(Effect::StartLogs { pane: id, key, container_id, full: pane.full_logs });
            }
            Plan::Spawn { argv } => {
                pane.set_content(message("Starting…"));
                return Some(Effect::SpawnTerm { pane: id, key: Some(key), argv });
            }
        }
        None
    }

    /// Drop the mouse selection and copy-mode cursor in a pane whose content
    /// is replaced.
    fn forget_view(&mut self, id: PaneId) {
        if self.selection.is_some_and(|s| s.pane == id) {
            self.selection = None;
        }
        if self.copy.is_some_and(|c| c.pane == id) {
            self.exit_copy_mode();
        }
    }

    /// What a (target, tab) needs: immediate lines, a fetch, or a stream.
    fn plan_content(&self, target: &NodeKey, tab: Tab, pane: PaneId) -> Plan {
        let t = &self.theme;
        let msg = |s: &str| Plan::Lines(vec![Line::styled(s.to_string(), Style::default().fg(t.overlay0))]);
        match (target, tab) {
            (NodeKey::Section(s), _) => Plan::Lines(format::summary(*s, &self.snap, &self.cpu, t)),
            (NodeKey::Container(_) | NodeKey::Service(..), _) => {
                let Some(c) = self.target_container(target) else {
                    return msg(match target {
                        NodeKey::Container(_) => "container no longer exists",
                        _ => "service has no container",
                    });
                };
                let id = c.id.clone();
                match tab {
                    Tab::Config => Plan::Load(LoadSpec::ContainerConfig { id }),
                    Tab::Env => Plan::Load(LoadSpec::ContainerEnv { id }),
                    Tab::Top if c.state == ContainerState::Running => Plan::Load(LoadSpec::Top { id }),
                    Tab::Top => msg("container is not running"),
                    Tab::Logs => Plan::Logs { container_id: id },
                    Tab::Stats => {
                        let width = self.content_size(pane).0 as usize;
                        Plan::Lines(format::stats(self.stats.get(&id), c.is_running(), width, t))
                    }
                    // Sections only.
                    Tab::Summary => msg(""),
                }
            }
            (NodeKey::Image(id), _) => Plan::Load(LoadSpec::Image { id: id.clone() }),
            (NodeKey::Volume(name), _) => Plan::Load(LoadSpec::Volume { name: name.clone() }),
            (NodeKey::Network(id), _) => Plan::Load(LoadSpec::Network { id: id.clone() }),
            (NodeKey::Project(p), Tab::Logs) => {
                let full = self.panes.get(&pane).is_some_and(|p| p.full_logs);
                let tail = if full { "--tail=all".to_string() } else { format!("--tail={PROJECT_LOG_TAIL_LINES}") };
                Plan::Spawn {
                    argv: ["docker", "compose", "-p", p, "logs", &tail, "--follow"].map(String::from).to_vec(),
                }
            }
            (NodeKey::Project(p), _) => match self.compose(p, &["config"]) {
                Ok(argv) => Plan::Load(LoadSpec::ComposeConfig { argv }),
                Err(e) => msg(&e),
            },
        }
    }
}

enum Plan {
    Lines(Lines),
    Load(LoadSpec),
    Logs { container_id: String },
    Spawn { argv: Vec<String> },
}
