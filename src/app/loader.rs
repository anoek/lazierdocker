//! Fetches inspector content (runs on the tokio runtime).

use std::fmt::Display;

use bollard::query_parameters::{InspectNetworkOptions, TopOptions};
use ratatui::style::Style;
use ratatui::text::Line;
use serde::Serialize;
use serde_json::Value;

use super::inspect::LoadSpec;
use crate::doc::format::{self, Lines};
use crate::docker::client::Docker;
use crate::docker::ops::cli_output;
use crate::ui::theme::Theme;

fn error_lines(e: impl std::fmt::Display, t: &Theme) -> Vec<Line<'static>> {
    vec![Line::styled(format!("Error: {e}"), Style::default().fg(t.red))]
}

/// Format an inspect result, or show its error.
fn show<T: Serialize, E: Display>(r: Result<T, E>, t: &Theme, format: impl Fn(&Value, &Theme) -> Lines) -> Lines {
    match r {
        Ok(v) => format(&serde_json::to_value(v).unwrap_or(Value::Null), t),
        Err(e) => error_lines(e, t),
    }
}

pub async fn load(docker: &Docker, spec: LoadSpec, t: &Theme) -> Lines {
    let api = &docker.api;
    match spec {
        LoadSpec::ContainerConfig { id } => show(api.inspect_container(&id, None).await, t, format::container_config),
        LoadSpec::ContainerEnv { id } => show(api.inspect_container(&id, None).await, t, format::env),
        LoadSpec::Image { id } => {
            let (inspect, history) = tokio::join!(api.inspect_image(&id), api.image_history(&id));
            let history = history.map(|h| serde_json::to_value(h).unwrap_or(Value::Null)).unwrap_or(Value::Null);
            show(inspect, t, |i, t| format::image_config(i, &history, t))
        }
        LoadSpec::Volume { name } => show(api.inspect_volume(&name).await, t, format::volume_config),
        LoadSpec::Network { id } => {
            show(api.inspect_network(&id, None::<InspectNetworkOptions>).await, t, format::network_config)
        }
        LoadSpec::Top { id } => match api.top_processes(&id, None::<TopOptions>).await {
            Ok(top) => format::top(&top.titles.unwrap_or_default(), &top.processes.unwrap_or_default(), t),
            Err(e) => error_lines(e, t),
        },
        LoadSpec::ComposeConfig { argv } => match cli_output(&docker.host, &argv).await {
            Ok(yaml) => format::yaml_text(&yaml, t),
            Err(e) => error_lines(e, t),
        },
    }
}
