//! Docker operations (lazydocker §3): SDK calls and compose CLI commands.

use anyhow::{anyhow, Result};
use bollard::query_parameters::{
    PruneContainersOptions, PruneImagesOptions, PruneNetworksOptions, PruneVolumesOptions, RemoveContainerOptions,
    RemoveImageOptions, RemoveVolumeOptions, RestartContainerOptions, StartContainerOptions, StopContainerOptions,
};

use super::client::Docker;
use super::present::size;

#[derive(Debug, Clone, PartialEq)]
pub enum DockerOp {
    Start(String),
    Stop(String),
    Restart(String),
    Pause(String),
    Unpause(String),
    RemoveContainer {
        id: String,
        force: bool,
        volumes: bool,
    },
    RemoveImage {
        id: String,
        force: bool,
        no_prune: bool,
    },
    RemoveVolume {
        name: String,
        force: bool,
    },
    RemoveNetwork {
        id: String,
    },
    PruneContainers,
    PruneImages,
    PruneVolumes,
    PruneNetworks,
    /// Several operations at once (bulk commands, every replica of a service).
    All(Vec<DockerOp>),
    /// A non-interactive compose/docker CLI command (argv).
    Cli(Vec<String>),
}

impl DockerOp {
    /// `op` on each container: the plain op for one, `All` for several.
    pub fn each(mut ids: Vec<String>, op: impl Fn(String) -> DockerOp) -> DockerOp {
        if ids.len() == 1 {
            op(ids.remove(0))
        } else {
            DockerOp::All(ids.into_iter().map(op).collect())
        }
    }

    /// Present-tense label for the status bar.
    pub fn describe(&self) -> String {
        match self {
            DockerOp::Start(_) => "starting".into(),
            DockerOp::Stop(_) => "stopping".into(),
            DockerOp::Restart(_) => "restarting".into(),
            DockerOp::Pause(_) => "pausing".into(),
            DockerOp::Unpause(_) => "unpausing".into(),
            DockerOp::RemoveContainer { .. }
            | DockerOp::RemoveImage { .. }
            | DockerOp::RemoveVolume { .. }
            | DockerOp::RemoveNetwork { .. } => "removing".into(),
            DockerOp::PruneContainers | DockerOp::PruneImages | DockerOp::PruneVolumes | DockerOp::PruneNetworks => {
                "pruning".into()
            }
            DockerOp::All(ops) => match ops.first() {
                Some(op) => format!("{} {} containers", op.describe(), ops.len()),
                None => "nothing to do".into(),
            },
            DockerOp::Cli(argv) => argv.join(" "),
        }
    }
}

/// Error text that means "stop it first or force it" (lazydocker retries with force).
pub fn is_must_stop(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("must stop")
        || e.contains("stop the container before")
        || e.contains("cannot remove a running container")
}

/// Await every request; fail with all their errors, one per line.
async fn all_ok<T, E: std::fmt::Display>(
    futs: impl IntoIterator<Item = impl std::future::Future<Output = Result<T, E>>>,
) -> Result<()> {
    let errs: Vec<String> =
        futures_util::future::join_all(futs).await.into_iter().filter_map(|r| r.err().map(|e| e.to_string())).collect();
    if errs.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(errs.join("\n")))
    }
}

fn pruned(n: usize, what: &str, reclaimed: Option<i64>) -> Option<String> {
    Some(match reclaimed {
        Some(bytes) => format!("pruned {n} {what}, reclaimed {}", size(bytes)),
        None => format!("pruned {n} {what}"),
    })
}

/// Run an operation. Returns an optional success message.
pub async fn run(docker: &Docker, op: DockerOp) -> Result<Option<String>> {
    let api = &docker.api;
    match op {
        DockerOp::Start(id) => api.start_container(&id, None::<StartContainerOptions>).await?,
        DockerOp::Stop(id) => api.stop_container(&id, None::<StopContainerOptions>).await?,
        DockerOp::Restart(id) => api.restart_container(&id, None::<RestartContainerOptions>).await?,
        DockerOp::Pause(id) => api.pause_container(&id).await?,
        DockerOp::Unpause(id) => api.unpause_container(&id).await?,
        DockerOp::RemoveContainer { id, force, volumes } => {
            api.remove_container(&id, Some(RemoveContainerOptions { force, v: volumes, link: false })).await?
        }
        DockerOp::RemoveImage { id, force, no_prune } => {
            api.remove_image(&id, Some(RemoveImageOptions { force, noprune: no_prune, ..Default::default() }), None)
                .await?;
        }
        DockerOp::RemoveVolume { name, force } => api.remove_volume(&name, Some(RemoveVolumeOptions { force })).await?,
        DockerOp::RemoveNetwork { id } => api.remove_network(&id).await?,
        DockerOp::PruneContainers => {
            let r = api.prune_containers(None::<PruneContainersOptions>).await?;
            let n = r.containers_deleted.map_or(0, |v| v.len());
            return Ok(pruned(n, "containers", Some(r.space_reclaimed.unwrap_or(0))));
        }
        DockerOp::PruneImages => {
            let r = api.prune_images(None::<PruneImagesOptions>).await?;
            let n = r.images_deleted.map_or(0, |v| v.len());
            return Ok(pruned(n, "images", Some(r.space_reclaimed.unwrap_or(0))));
        }
        DockerOp::PruneVolumes => {
            let r = api.prune_volumes(None::<PruneVolumesOptions>).await?;
            let n = r.volumes_deleted.map_or(0, |v| v.len());
            return Ok(pruned(n, "volumes", Some(r.space_reclaimed.unwrap_or(0))));
        }
        DockerOp::PruneNetworks => {
            let r = api.prune_networks(None::<PruneNetworksOptions>).await?;
            let n = r.networks_deleted.map_or(0, |v| v.len());
            return Ok(pruned(n, "networks", None));
        }
        DockerOp::All(ops) => all_ok(ops.into_iter().map(|op| Box::pin(run(docker, op)))).await?,
        DockerOp::Cli(argv) => {
            cli_output(&docker.host, &argv).await?;
        }
    }
    Ok(None)
}

/// Run a CLI command against the daemon at `host`; its stdout, or its
/// stderr as the error.
pub async fn cli_output(host: &str, argv: &[String]) -> Result<String> {
    let (prog, args) = argv.split_first().ok_or_else(|| anyhow!("empty command"))?;
    let out = tokio::process::Command::new(prog)
        .args(args)
        .env("DOCKER_HOST", host)
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(anyhow!(if err.is_empty() { format!("{} failed", argv.join(" ")) } else { err }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn must_stop_detection() {
        assert!(is_must_stop("Error response from daemon: cannot remove container \"/x\": container is running: stop the container before removing or force remove"));
        assert!(is_must_stop(
            "You cannot remove a running container abc. Stop the container before attempting removal or force remove"
        ));
        assert!(!is_must_stop("No such container: x"));
    }
}
