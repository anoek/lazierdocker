//! Fetches inspector content (runs on the tokio runtime).

use std::fmt::Display;

use bollard::query_parameters::{InspectNetworkOptions, TopOptions};
use futures_util::future::join_all;
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

/// `lines` after a dim `note` line, if any.
fn noted(note: Option<String>, mut lines: Lines, t: &Theme) -> Lines {
    if let Some(note) = note {
        lines.splice(0..0, [Line::styled(note, Style::default().fg(t.overlay0)), Line::raw("")]);
    }
    lines
}

/// Processes of each (label, container); `labelled` adds a leading
/// CONTAINER column saying whose they are.
async fn top(docker: &Docker, containers: Vec<(String, String)>, labelled: bool, t: &Theme) -> Lines {
    let results = join_all(containers.iter().map(|(_, id)| docker.api.top_processes(id, None::<TopOptions>))).await;
    let mut titles: Option<Vec<String>> = None;
    let mut rows = Vec::new();
    let mut first_error = None;
    for ((label, _), result) in containers.iter().zip(results) {
        match result {
            Ok(top) => {
                titles.get_or_insert_with(|| top.titles.clone().unwrap_or_default());
                for row in top.processes.unwrap_or_default() {
                    rows.push(if labelled { [vec![label.clone()], row].concat() } else { row });
                }
            }
            // A replica that stopped since the last snapshot has no processes.
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    match (titles, first_error) {
        (Some(titles), _) => {
            let titles = if labelled { [vec!["CONTAINER".to_string()], titles].concat() } else { titles };
            format::top(&titles, &rows, t)
        }
        (None, Some(e)) => error_lines(e, t),
        (None, None) => Vec::new(),
    }
}

pub async fn load(docker: &Docker, spec: LoadSpec, t: &Theme) -> Lines {
    let api = &docker.api;
    match spec {
        LoadSpec::ContainerConfig { id, note } => {
            noted(note, show(api.inspect_container(&id, None).await, t, format::container_config), t)
        }
        LoadSpec::ContainerEnv { id, note } => {
            noted(note, show(api.inspect_container(&id, None).await, t, format::env), t)
        }
        LoadSpec::Image { id } => {
            let (inspect, history) = tokio::join!(api.inspect_image(&id), api.image_history(&id));
            let history = history.map(|h| serde_json::to_value(h).unwrap_or(Value::Null)).unwrap_or(Value::Null);
            show(inspect, t, |i, t| format::image_config(i, &history, t))
        }
        LoadSpec::Volume { name } => show(api.inspect_volume(&name).await, t, format::volume_config),
        LoadSpec::Network { id } => {
            show(api.inspect_network(&id, None::<InspectNetworkOptions>).await, t, format::network_config)
        }
        LoadSpec::Top { containers, labelled } => top(docker, containers, labelled, t).await,
        LoadSpec::ComposeConfig { argv } => match cli_output(&docker.host, &argv).await {
            Ok(yaml) => format::yaml_text(&yaml, t),
            Err(e) => error_lines(e, t),
        },
    }
}
