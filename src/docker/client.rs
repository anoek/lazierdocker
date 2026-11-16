//! Thin wrapper over bollard producing [`Snapshot`]s.

use std::time::Duration;

use anyhow::{Context, Result};
use bollard::query_parameters::{ListContainersOptions, ListImagesOptions, ListNetworksOptions, ListVolumesOptions};

use super::model::{
    derive_projects, sort_containers, sort_images, sort_volumes, Container, Image, LocalCompose, Network, Snapshot,
    Volume,
};

#[derive(Clone)]
pub struct Docker {
    pub api: bollard::Docker,
    /// Where we connected, for the status bar.
    pub host: String,
}

pub const DEFAULT_HOST: &str = "unix:///var/run/docker.sock";

/// Pick the daemon the way the docker CLI does: explicit flag, then
/// `DOCKER_HOST`, then the current docker context, then the default socket.
pub fn resolve_host_with(flag: Option<&str>, env: Option<&str>, context: impl FnOnce() -> Option<String>) -> String {
    if let Some(h) = flag.filter(|h| !h.is_empty()) {
        return h.to_string();
    }
    if let Some(h) = env.filter(|h| !h.is_empty()) {
        return h.to_string();
    }
    context().filter(|h| !h.is_empty()).unwrap_or_else(|| DEFAULT_HOST.to_string())
}

/// The current docker context's endpoint (`docker context inspect`, which
/// honours DOCKER_CONTEXT and ~/.docker/config.json).
fn context_host() -> Option<String> {
    let out = std::process::Command::new("docker")
        .args(["context", "inspect", "--format", "{{.Endpoints.docker.Host}}"])
        .env_remove("DOCKER_HOST")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn resolve_host(flag: Option<&str>) -> String {
    let env = std::env::var("DOCKER_HOST").ok();
    resolve_host_with(flag, env.as_deref(), context_host)
}

impl Docker {
    /// Connect to the daemon the docker CLI would use (`flag` is `-H`).
    pub fn connect(flag: Option<&str>) -> Result<Self> {
        let host = resolve_host(flag);
        let tls = std::env::var("DOCKER_TLS_VERIFY").is_ok_and(|v| !v.is_empty() && v != "0");
        if tls && !host.starts_with("unix://") {
            anyhow::bail!("unsupported docker host {host}: TLS (DOCKER_TLS_VERIFY) is not supported yet");
        }
        Self::connect_to(&host)
    }

    /// Connect to an explicit `unix://` or `tcp://` (alias `http://`) address.
    /// Any other scheme is an error: we never silently connect somewhere else.
    pub fn connect_to(host: &str) -> Result<Self> {
        const TIMEOUT_SECS: u64 = 30;
        let api = if host.starts_with("unix://") {
            bollard::Docker::connect_with_unix(host, TIMEOUT_SECS, bollard::API_DEFAULT_VERSION)
        } else if host.starts_with("tcp://") || host.starts_with("http://") {
            bollard::Docker::connect_with_http(host, TIMEOUT_SECS, bollard::API_DEFAULT_VERSION)
        } else {
            anyhow::bail!(
                "unsupported docker host {host}: use unix:// or tcp:// (for ssh:// hosts, forward the socket: ssh -L /tmp/d.sock:/var/run/docker.sock HOST)"
            );
        }
        .with_context(|| format!("connecting to docker at {host}"))?;
        Ok(Self { api, host: host.to_string() })
    }

    pub async fn snapshot(&self, local: Option<&LocalCompose>) -> Result<Snapshot> {
        let containers_f = self.api.list_containers(Some(ListContainersOptions { all: true, ..Default::default() }));
        let images_f = self.api.list_images(None::<ListImagesOptions>);
        let volumes_f = self.api.list_volumes(None::<ListVolumesOptions>);
        let networks_f = self.api.list_networks(None::<ListNetworksOptions>);
        let (containers, images, volumes, networks) = tokio::join!(containers_f, images_f, volumes_f, networks_f);

        let mut containers: Vec<Container> =
            containers.context("listing containers")?.iter().map(Container::from_summary).collect();
        sort_containers(&mut containers);
        let mut images: Vec<Image> = images.context("listing images")?.iter().map(Image::from_summary).collect();
        sort_images(&mut images);
        let mut volumes: Vec<Volume> =
            volumes.context("listing volumes")?.volumes.unwrap_or_default().iter().map(Volume::from_bollard).collect();
        sort_volumes(&mut volumes);
        let mut networks: Vec<Network> =
            networks.context("listing networks")?.iter().map(Network::from_bollard).collect();
        networks.sort_by(|a, b| a.name.cmp(&b.name));
        let projects = derive_projects(&containers, local);
        Ok(Snapshot { containers, projects, images, volumes, networks })
    }
}

/// Detect the compose project in the current directory (lazydocker: `docker compose config`).
pub async fn detect_local_compose(compose_files: &[String], project: Option<&str>) -> Option<LocalCompose> {
    let mut cmd = tokio::process::Command::new("docker");
    cmd.arg("compose");
    for f in compose_files {
        cmd.arg("-f").arg(f);
    }
    if let Some(p) = project {
        cmd.arg("-p").arg(p);
    }
    cmd.args(["config", "--format", "json"]);
    cmd.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    parse_compose_config(&out.stdout)
}

fn parse_compose_config(json: &[u8]) -> Option<LocalCompose> {
    let v: serde_json::Value = serde_json::from_slice(json).ok()?;
    let project = v.get("name")?.as_str()?.to_string();
    let mut services: Vec<String> =
        v.get("services").and_then(|s| s.as_object()).map(|m| m.keys().cloned().collect()).unwrap_or_default();
    services.sort();
    Some(LocalCompose { project, services })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compose_config_parsed() {
        let json = br#"{"name":"demo","services":{"web":{},"db":{}},"volumes":{}}"#;
        let l = parse_compose_config(json).unwrap();
        assert_eq!(l.project, "demo");
        assert_eq!(l.services, vec!["db", "web"]);
        assert!(parse_compose_config(b"not json").is_none());
    }

    #[test]
    fn unsupported_schemes_are_rejected_not_redirected() {
        for host in ["ssh://prod", "https://h:2376", "myhost:2375", "npipe:////./pipe/docker_engine"] {
            let err = Docker::connect_to(host).err().unwrap_or_else(|| panic!("{host} must not connect"));
            assert!(format!("{err:#}").contains("unsupported"), "{host}: {err:#}");
        }
        assert!(Docker::connect_to("unix:///var/run/docker.sock").is_ok());
        assert!(Docker::connect_to("tcp://127.0.0.1:2375").is_ok());
    }

    #[test]
    fn host_resolution_order() {
        let ctx = || Some("unix:///home/u/.colima/docker.sock".to_string());
        assert_eq!(resolve_host_with(Some("tcp://a:1"), Some("tcp://b:2"), ctx), "tcp://a:1", "flag wins");
        assert_eq!(resolve_host_with(None, Some("tcp://b:2"), ctx), "tcp://b:2", "then DOCKER_HOST");
        assert_eq!(
            resolve_host_with(None, Some(""), ctx),
            "unix:///home/u/.colima/docker.sock",
            "then the current context"
        );
        assert_eq!(resolve_host_with(None, None, || None), DEFAULT_HOST, "then the default socket");
    }
}
