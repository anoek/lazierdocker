//! Fetches inspector content (runs on the tokio runtime).

use ratatui::style::Style;
use ratatui::text::Line;
use serde_json::Value;

use super::inspect::LoadSpec;
use crate::doc::format;
use crate::docker::client::Docker;
use crate::ui::theme::Theme;

/// A `docker` CLI command targeting the same daemon we are connected to.
pub fn docker_cli(host: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new("docker");
    cmd.env("DOCKER_HOST", host);
    cmd.stdin(std::process::Stdio::null());
    cmd
}

fn error_lines(e: impl std::fmt::Display, t: &Theme) -> Vec<Line<'static>> {
    vec![Line::styled(format!("Error: {e}"), Style::default().fg(t.red))]
}

pub async fn load(docker: &Docker, spec: LoadSpec, compose_files: &[String], t: &Theme) -> Vec<Line<'static>> {
    match spec {
        LoadSpec::ContainerConfig { id } => match docker.api.inspect_container(&id, None).await {
            Ok(r) => format::container_config(&serde_json::to_value(r).unwrap_or(Value::Null), t),
            Err(e) => error_lines(e, t),
        },
        LoadSpec::ContainerEnv { id } => match docker.api.inspect_container(&id, None).await {
            Ok(r) => format::env(&serde_json::to_value(r).unwrap_or(Value::Null), t),
            Err(e) => error_lines(e, t),
        },
        LoadSpec::Image { id } => {
            let (inspect, history) = tokio::join!(docker.api.inspect_image(&id), docker.api.image_history(&id));
            match inspect {
                Ok(i) => {
                    let history =
                        history.map(|h| serde_json::to_value(h).unwrap_or(Value::Null)).unwrap_or(Value::Null);
                    format::image_config(&serde_json::to_value(i).unwrap_or(Value::Null), &history, t)
                }
                Err(e) => error_lines(e, t),
            }
        }
        LoadSpec::Volume { name } => match docker.api.inspect_volume(&name).await {
            Ok(v) => format::volume_config(&serde_json::to_value(v).unwrap_or(Value::Null), t),
            Err(e) => error_lines(e, t),
        },
        LoadSpec::Network { id } => {
            match docker.api.inspect_network(&id, None::<bollard::query_parameters::InspectNetworkOptions>).await {
                Ok(n) => format::network_config(&serde_json::to_value(n).unwrap_or(Value::Null), t),
                Err(e) => error_lines(e, t),
            }
        }
        LoadSpec::Top { id } => {
            match docker.api.top_processes(&id, None::<bollard::query_parameters::TopOptions>).await {
                Ok(top) => format::top(&top.titles.unwrap_or_default(), &top.processes.unwrap_or_default(), t),
                Err(e) => error_lines(e, t),
            }
        }
        LoadSpec::ComposeConfig { project, local, config_files, working_dir } => {
            let mut cmd = docker_cli(&docker.host);
            cmd.arg("compose");
            if local {
                for f in compose_files {
                    cmd.arg("-f").arg(f);
                }
            } else if let Some(files) = &config_files {
                for f in files.split(',').filter(|f| !f.is_empty()) {
                    cmd.arg("-f").arg(f);
                }
                if let Some(dir) = &working_dir {
                    cmd.arg("--project-directory").arg(dir);
                }
            } else {
                return vec![Line::styled(
                    "Compose config is only available for projects whose compose files are on this machine.",
                    Style::default().fg(t.overlay0),
                )];
            }
            cmd.arg("-p").arg(&project).arg("config");
            match cmd.output().await {
                Ok(out) if out.status.success() => format::yaml_text(&String::from_utf8_lossy(&out.stdout), t),
                Ok(out) => error_lines(String::from_utf8_lossy(&out.stderr).trim().to_string(), t),
                Err(e) => error_lines(e, t),
            }
        }
    }
}
