//! Docker domain model, decoupled from bollard's wire types.
//!
//! Project/service derivation follows lazydocker (docs/reference/lazydocker-inventory.md §1).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bollard::models::{
    ContainerSummary, ContainerSummaryStateEnum, ImageSummary, Network as BNetwork, PortSummary, Volume as BVolume,
};

pub const LABEL_PROJECT: &str = "com.docker.compose.project";
pub const LABEL_SERVICE: &str = "com.docker.compose.service";
pub const LABEL_ONEOFF: &str = "com.docker.compose.oneoff";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Exited,
    Removing,
    Dead,
    Stopping,
    Unknown,
}

impl ContainerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Restarting => "restarting",
            Self::Exited => "exited",
            Self::Removing => "removing",
            Self::Dead => "dead",
            Self::Stopping => "stopping",
            Self::Unknown => "unknown",
        }
    }

    /// Sort rank: running first, then exited, created, then everything else.
    fn rank(self) -> u8 {
        match self {
            Self::Running => 0,
            Self::Exited => 1,
            Self::Created => 2,
            _ => 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Healthy,
    Unhealthy,
    Starting,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Port {
    pub ip: Option<String>,
    pub private: u16,
    pub public: Option<u16>,
    pub proto: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Container {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: ContainerState,
    pub exit_code: Option<i64>,
    pub health: Option<Health>,
    pub ports: Vec<Port>,
    pub labels: BTreeMap<String, String>,
    pub project: Option<String>,
    pub service: Option<String>,
    pub oneoff: bool,
}

impl Container {
    pub fn is_running(&self) -> bool {
        matches!(self.state, ContainerState::Running | ContainerState::Restarting | ContainerState::Paused)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub name: String,
    pub project: String,
    pub container_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: String,
    pub services: Vec<Service>,
    /// True if this is the compose project in the current directory.
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub id: String,
    pub name: String,
    pub tag: String,
    pub size: i64,
}

impl Image {
    pub fn short_id(&self) -> &str {
        let id = self.id.strip_prefix("sha256:").unwrap_or(&self.id);
        &id[..id.len().min(12)]
    }

    pub fn display(&self) -> String {
        if self.tag.is_empty() {
            self.name.clone()
        } else {
            format!("{}:{}", self.name, self.tag)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Volume {
    pub name: String,
    pub driver: String,
    pub labelled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Network {
    pub id: String,
    pub name: String,
    pub driver: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub containers: Vec<Container>,
    pub projects: Vec<Project>,
    pub images: Vec<Image>,
    pub volumes: Vec<Volume>,
    pub networks: Vec<Network>,
}

impl Snapshot {
    pub fn container(&self, id: &str) -> Option<&Container> {
        self.containers.iter().find(|c| c.id == id)
    }

    pub fn project(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    pub fn service(&self, project: &str, name: &str) -> Option<&Service> {
        self.project(project)?.services.iter().find(|s| s.name == name)
    }

    pub fn image(&self, id: &str) -> Option<&Image> {
        self.images.iter().find(|i| i.id == id)
    }

    pub fn network(&self, id: &str) -> Option<&Network> {
        self.networks.iter().find(|n| n.id == id)
    }

    /// Containers not represented by a compose service row (one-offs and plain containers).
    pub fn standalone_containers(&self) -> impl Iterator<Item = &Container> {
        self.containers.iter().filter(|c| c.oneoff || c.service.is_none() || c.project.is_none())
    }
}

/// The compose project in the current directory: its name and services (from `docker compose config`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LocalCompose {
    pub project: String,
    pub services: Vec<String>,
}

fn state_from(s: Option<&ContainerSummaryStateEnum>) -> ContainerState {
    match s {
        Some(ContainerSummaryStateEnum::CREATED) => ContainerState::Created,
        Some(ContainerSummaryStateEnum::RUNNING) => ContainerState::Running,
        Some(ContainerSummaryStateEnum::PAUSED) => ContainerState::Paused,
        Some(ContainerSummaryStateEnum::RESTARTING) => ContainerState::Restarting,
        Some(ContainerSummaryStateEnum::EXITED) => ContainerState::Exited,
        Some(ContainerSummaryStateEnum::REMOVING) => ContainerState::Removing,
        Some(ContainerSummaryStateEnum::DEAD) => ContainerState::Dead,
        Some(ContainerSummaryStateEnum::STOPPING) => ContainerState::Stopping,
        _ => ContainerState::Unknown,
    }
}

/// Parse `Exited (137) 2 hours ago` → 137.
pub fn parse_exit_code(status: &str) -> Option<i64> {
    let rest = status.strip_prefix("Exited (")?;
    rest.split(')').next()?.parse().ok()
}

/// Parse `Up 3 minutes (healthy)` → Healthy.
pub fn parse_health(status: &str) -> Option<Health> {
    if status.contains("(healthy)") {
        Some(Health::Healthy)
    } else if status.contains("(unhealthy)") {
        Some(Health::Unhealthy)
    } else if status.contains("(health: starting)") {
        Some(Health::Starting)
    } else {
        None
    }
}

fn port_from(p: &PortSummary) -> Port {
    Port {
        ip: p.ip.clone().filter(|s| !s.is_empty()),
        private: p.private_port,
        public: p.public_port,
        proto: p.typ.map(|t| t.to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "tcp".into()),
    }
}

impl Container {
    pub fn from_summary(s: &ContainerSummary) -> Self {
        let labels: BTreeMap<String, String> = s.labels.clone().unwrap_or_default().into_iter().collect();
        let status = s.status.as_deref().unwrap_or_default();
        let name = s
            .names
            .as_ref()
            .and_then(|n| n.first())
            .map(|n| n.trim_start_matches('/').to_string())
            .unwrap_or_else(|| s.id.clone().unwrap_or_default());
        let mut ports: Vec<Port> = s.ports.iter().flatten().map(port_from).collect();
        ports.sort();
        ports.dedup();
        Container {
            id: s.id.clone().unwrap_or_default(),
            name,
            image: s.image.clone().unwrap_or_default().trim_start_matches("sha256:").to_string(),
            state: state_from(s.state.as_ref()),
            exit_code: parse_exit_code(status),
            health: parse_health(status),
            ports,
            project: labels.get(LABEL_PROJECT).cloned(),
            service: labels.get(LABEL_SERVICE).cloned(),
            oneoff: labels.get(LABEL_ONEOFF).is_some_and(|v| v.eq_ignore_ascii_case("true")),
            labels,
        }
    }
}

impl Image {
    pub fn from_summary(s: &ImageSummary) -> Self {
        let (name, tag) = match s.repo_tags.iter().find(|t| *t != "<none>:<none>") {
            Some(t) => match t.rsplit_once(':') {
                Some((n, tag)) if !tag.contains('/') => (n.to_string(), tag.to_string()),
                _ => (t.clone(), String::new()),
            },
            None => ("<none>".to_string(), String::new()),
        };
        Image { id: s.id.clone(), name, tag, size: s.size }
    }
}

impl Volume {
    pub fn from_bollard(v: &BVolume) -> Self {
        Volume { name: v.name.clone(), driver: v.driver.clone(), labelled: !v.labels.is_empty() }
    }
}

impl Network {
    pub fn from_bollard(n: &BNetwork) -> Self {
        Network {
            id: n.id.clone().unwrap_or_default(),
            name: n.name.clone().unwrap_or_default(),
            driver: n.driver.clone().unwrap_or_default(),
        }
    }
}

/// Containers: running, exited, created, then other states; then by name.
pub fn sort_containers(cs: &mut [Container]) {
    cs.sort_by(|a, b| a.state.rank().cmp(&b.state.rank()).then_with(|| a.name.cmp(&b.name)));
}

/// Images by name, tag, id with untagged (`<none>`) last.
pub fn sort_images(is: &mut [Image]) {
    is.sort_by(|a, b| (a.name == "<none>", &a.name, &a.tag, &a.id).cmp(&(b.name == "<none>", &b.name, &b.tag, &b.id)));
}

/// Volumes with labels first, then by name.
pub fn sort_volumes(vs: &mut [Volume]) {
    vs.sort_by(|a, b| b.labelled.cmp(&a.labelled).then_with(|| a.name.cmp(&b.name)));
}

/// Build compose projects from container labels plus the local compose file's services.
pub fn derive_projects(containers: &[Container], local: Option<&LocalCompose>) -> Vec<Project> {
    let mut services: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for c in containers {
        if c.oneoff {
            continue;
        }
        if let (Some(p), Some(s)) = (&c.project, &c.service) {
            services.entry(p.clone()).or_default().insert(s.clone());
        }
    }
    if let Some(l) = local {
        let set = services.entry(l.project.clone()).or_default();
        set.extend(l.services.iter().cloned());
    }
    // First non-oneoff container per (project, service); prefer a running one.
    let mut by_key: HashMap<(&str, &str), &Container> = HashMap::new();
    for c in containers.iter().filter(|c| !c.oneoff) {
        if let (Some(p), Some(s)) = (&c.project, &c.service) {
            let e = by_key.entry((p.as_str(), s.as_str())).or_insert(c);
            if !e.is_running() && c.is_running() {
                *e = c;
            }
        }
    }
    let mut out: Vec<Project> = services
        .into_iter()
        .map(|(name, svcs)| {
            let mut services: Vec<Service> = svcs
                .into_iter()
                .map(|s| Service {
                    container_id: by_key.get(&(name.as_str(), s.as_str())).map(|c| c.id.clone()),
                    name: s,
                    project: name.clone(),
                })
                .collect();
            services.sort_by(|a, b| {
                a.container_id.is_none().cmp(&b.container_id.is_none()).then_with(|| a.name.cmp(&b.name))
            });
            Project { local: local.is_some_and(|l| l.project == name), name, services }
        })
        .collect();
    // Local project first, then by name.
    out.sort_by(|a, b| b.local.cmp(&a.local).then_with(|| a.name.cmp(&b.name)));
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn container(name: &str, state: ContainerState, project: Option<(&str, &str)>) -> Container {
        let mut labels = BTreeMap::new();
        if let Some((p, s)) = project {
            labels.insert(LABEL_PROJECT.into(), p.into());
            labels.insert(LABEL_SERVICE.into(), s.into());
        }
        Container {
            id: format!("{name}-id-0123456789abcdef"),
            name: name.into(),
            image: "alpine:3.20".into(),
            state,
            exit_code: None,
            health: None,
            ports: vec![],
            project: project.map(|p| p.0.into()),
            service: project.map(|p| p.1.into()),
            oneoff: false,
            labels,
        }
    }

    #[test]
    fn projects_from_labels() {
        let cs = vec![
            container("demo-web-1", ContainerState::Running, Some(("demo", "web"))),
            container("demo-db-1", ContainerState::Exited, Some(("demo", "db"))),
            container("other-api-1", ContainerState::Running, Some(("other", "api"))),
            container("loner", ContainerState::Running, None),
        ];
        let local = LocalCompose { project: "other".into(), services: vec!["api".into(), "cache".into()] };
        let ps = derive_projects(&cs, Some(&local));
        let names: Vec<_> = ps.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["other", "demo"], "local project first");
        let other: Vec<_> = ps[0].services.iter().map(|s| (s.name.as_str(), s.container_id.is_some())).collect();
        assert_eq!(other, vec![("api", true), ("cache", false)]);
        assert_eq!(ps[1].services.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["db", "web"]);
        assert_eq!(ps[1].services[1].container_id.as_deref(), Some("demo-web-1-id-0123456789abcdef"));
    }

    #[test]
    fn oneoff_skipped() {
        let mut run = container("demo-web-run-1", ContainerState::Running, Some(("demo", "migrate")));
        run.oneoff = true;
        let ps = derive_projects(&[run.clone()], None);
        assert!(ps.is_empty());
        let snap = Snapshot { containers: vec![run], ..Default::default() };
        assert_eq!(snap.standalone_containers().count(), 1);
    }

    #[test]
    fn sort_order() {
        let mut cs = vec![
            container("b", ContainerState::Paused, None),
            container("z", ContainerState::Created, None),
            container("a", ContainerState::Exited, None),
            container("c", ContainerState::Running, None),
            container("a2", ContainerState::Running, None),
        ];
        sort_containers(&mut cs);
        let names: Vec<_> = cs.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["a2", "c", "a", "z", "b"]);
    }

    #[test]
    fn status_parsing() {
        assert_eq!(parse_exit_code("Exited (137) 2 hours ago"), Some(137));
        assert_eq!(parse_exit_code("Up 2 hours"), None);
        assert_eq!(parse_health("Up 3 minutes (healthy)"), Some(Health::Healthy));
        assert_eq!(parse_health("Up 3 seconds (health: starting)"), Some(Health::Starting));
        assert_eq!(parse_health("Up 3 minutes"), None);
    }

    #[test]
    fn image_untagged_named_none_sorted_last() {
        let mk = |id: &str, tags: &[&str]| ImageSummary {
            id: id.into(),
            repo_tags: tags.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        let mut is = vec![
            Image::from_summary(&mk("sha256:1", &[])),
            Image::from_summary(&mk("sha256:2", &["redis:7-alpine"])),
            Image::from_summary(&mk("sha256:3", &["localhost:5000/app"])),
            Image::from_summary(&mk("sha256:4", &["<none>:<none>"])),
        ];
        sort_images(&mut is);
        let shown: Vec<_> = is.iter().map(|i| (i.name.as_str(), i.tag.as_str())).collect();
        assert_eq!(shown, vec![("localhost:5000/app", ""), ("redis", "7-alpine"), ("<none>", ""), ("<none>", "")]);
    }

    #[test]
    fn container_from_summary() {
        let s = ContainerSummary {
            id: Some("abc".into()),
            names: Some(vec!["/demo-web-1".into()]),
            image: Some("nginx:alpine".into()),
            state: Some(ContainerSummaryStateEnum::EXITED),
            status: Some("Exited (2) 1 minute ago".into()),
            labels: Some(HashMap::from([
                (LABEL_PROJECT.to_string(), "demo".to_string()),
                (LABEL_SERVICE.to_string(), "web".to_string()),
                (LABEL_ONEOFF.to_string(), "False".to_string()),
            ])),
            ..Default::default()
        };
        let c = Container::from_summary(&s);
        assert_eq!(c.name, "demo-web-1");
        assert_eq!(c.state, ContainerState::Exited);
        assert_eq!(c.exit_code, Some(2));
        assert_eq!((c.project.as_deref(), c.service.as_deref(), c.oneoff), (Some("demo"), Some("web"), false));
    }
}
