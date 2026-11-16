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

impl Docker {
    /// Connect using DOCKER_HOST or the platform default socket.
    pub fn connect() -> Result<Self> {
        match std::env::var("DOCKER_HOST") {
            Ok(host) if !host.is_empty() => Self::connect_to(&host),
            _ => {
                let api = bollard::Docker::connect_with_defaults().context("connecting to docker")?;
                Ok(Self { api, host: "unix:///var/run/docker.sock".into() })
            }
        }
    }

    /// Connect to an explicit `tcp://`/`http://` or `unix://` address.
    pub fn connect_to(host: &str) -> Result<Self> {
        const TIMEOUT_SECS: u64 = 30;
        let api = if host.starts_with("unix://") {
            bollard::Docker::connect_with_unix(host, TIMEOUT_SECS, bollard::API_DEFAULT_VERSION)
        } else if host.starts_with("tcp://") || host.starts_with("http://") {
            bollard::Docker::connect_with_http(host, TIMEOUT_SECS, bollard::API_DEFAULT_VERSION)
        } else {
            bollard::Docker::connect_with_defaults()
        }
        .with_context(|| format!("connecting to docker at {host}"))?;
        Ok(Self { api, host: host.to_string() })
    }

    pub async fn ping(&self) -> Result<()> {
        self.api.ping().await.context("docker ping")?;
        Ok(())
    }

    pub async fn snapshot(&self, local: Option<&LocalCompose>) -> Result<Snapshot> {
        let containers_f = self.api.list_containers(Some(ListContainersOptions { all: true, ..Default::default() }));
        let images_f = self.api.list_images(Some(ListImagesOptions { all: false, ..Default::default() }));
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
    cmd.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let out = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    parse_compose_config(&out.stdout)
}

pub fn parse_compose_config(json: &[u8]) -> Option<LocalCompose> {
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
}
