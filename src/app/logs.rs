//! Logs views. A Logs pane shows one container, or every replica of a
//! service with each line labelled: their history merged in time order, then
//! each container followed. A container that restarts resumes where it left
//! off, and a replica added by a scale-up joins in; neither reloads the pane.

use std::sync::Arc;

use futures_util::future::join_all;
use ratatui::style::Color;

use super::inspect::{ContentKey, LOG_TAIL_LINES};
use super::{App, Content, Effect};
use crate::docker::client::Docker;
use crate::docker::logs::{self, LogLine, LogTime};
use crate::docker::model::{Container, ContainerState};
use crate::docker::present::replica_label;
use crate::term::{normalise_lf, Feeder};
use crate::ui::layout::PaneId;
use crate::ui::sidebar::NodeKey;

/// A container a Logs pane shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSource {
    pub id: String,
    /// Written before each of its lines (its coloured replica label) when the
    /// pane shows several containers.
    pub prefix: Option<String>,
}

/// One container's log stream in a Logs pane.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LogStream {
    /// The run feeding it: only that run's end report counts (a replaced
    /// run's task may report after it was aborted).
    pub run: u64,
    /// Its stream is running, or about to start.
    pub following: bool,
    /// Its stream ended cleanly at this snapshot generation; it resumes once a
    /// newer snapshot shows the container running.
    pub ended_at: Option<u64>,
    /// Time of its last line shown: a resumed stream continues after it.
    pub last: Option<LogTime>,
}

/// How a container's stream in a Logs view ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEnd {
    pub container: String,
    /// It ended without an error (the container stopped).
    pub clean: bool,
    /// Time of its last line.
    pub last: Option<LogTime>,
}

/// Told when each container's stream of a run ends.
pub type Ended = Arc<dyn Fn(StreamEnd) + Send + Sync>;

/// ANSI foreground colour for a theme colour.
fn ansi_fg(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        Color::Indexed(i) => format!("\x1b[38;5;{i}m"),
        _ => String::new(),
    }
}

impl App {
    /// The containers a Logs view of `target` shows; labelled when combined.
    pub(super) fn log_sources(&self, target: &NodeKey) -> Vec<LogSource> {
        let containers = self.target_containers(target);
        if !combined(&containers) {
            return containers.iter().map(|c| LogSource { id: c.id.clone(), prefix: None }).collect();
        }
        let t = &self.theme;
        let colors = [t.cyan, t.green, t.yellow, t.mauve, t.blue, t.magenta];
        let labels: Vec<String> = containers.iter().map(|c| replica_label(c)).collect();
        let width = labels.iter().map(|l| l.chars().count()).max().unwrap_or(0);
        containers
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (c, label))| {
                // By replica number, so a replica keeps its colour as others come and go.
                let color = colors[c.number.map_or(i, |n| n as usize) % colors.len()];
                LogSource { id: c.id.clone(), prefix: Some(format!("{}{label:<width$} │\x1b[0m ", ansi_fg(color))) }
            })
            .collect()
    }

    /// Keep a showing Logs view's streams in step with its containers: follow
    /// a new one (a scale-up) from its start, and resume one that ended
    /// cleanly, after its last line, once a newer snapshot shows it running
    /// (a restart).
    pub(super) fn sync_log_streams(&mut self, id: PaneId, key: &ContentKey) -> Vec<Effect> {
        // Until the runtime installs the terminal there is nothing to follow into.
        let Some(pane) = self.panes.get(&id).filter(|p| matches!(p.content, Content::Term(_))) else {
            return Vec::new();
        };
        let generation = self.snap_generation;
        let starts: Vec<(String, Option<LogTime>)> = self
            .target_containers(&key.target)
            .iter()
            .filter_map(|c| match pane.logs.get(&c.id) {
                None => Some((c.id.clone(), None)),
                Some(s)
                    if !s.following
                        && s.ended_at.is_some_and(|g| g < generation)
                        && c.state == ContainerState::Running =>
                {
                    Some((c.id.clone(), s.last))
                }
                Some(_) => None,
            })
            .collect();
        if starts.is_empty() {
            return Vec::new();
        }
        let sources = self.log_sources(&key.target);
        let mut effects = Vec::new();
        for (container, after) in starts {
            let Some(source) = sources.iter().find(|s| s.id == container).cloned() else { continue };
            self.log_runs += 1;
            let run = self.log_runs;
            let Some(pane) = self.panes.get_mut(&id) else { break };
            let stream = pane.logs.entry(container).or_default();
            stream.run = run;
            stream.following = true;
            stream.ended_at = None;
            effects.push(Effect::FollowLogs { pane: id, key: key.clone(), run, source, after });
        }
        effects
    }

    /// Run `run` of a container's stream in a Logs pane ended.
    pub(super) fn on_logs_ended(&mut self, pane: PaneId, run: u64, end: StreamEnd) {
        let generation = self.snap_generation;
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let Some(s) = p.logs.get_mut(&end.container).filter(|s| s.run == run) else { return };
        s.following = false;
        s.last = end.last.or(s.last);
        s.ended_at = end.clean.then_some(generation);
        if !p.logs.values().any(|s| s.following) {
            if let Content::Term(t) = &mut p.content {
                t.notice("[log stream ended]");
            }
        }
        self.dirty = true;
    }
}

/// A target with several containers (a scaled service) shows them combined:
/// labelled logs, summed stats, one process table.
pub(super) fn combined(containers: &[&Container]) -> bool {
    containers.len() > 1
}

/// A log line as written to the terminal: its prefix, then the line, which
/// always ends the terminal line (`\n` alone becomes `\r\n` without a TTY).
fn terminal_line(prefix: Option<&str>, text: &[u8], tty: bool) -> Vec<u8> {
    let mut out = prefix.map(|p| p.as_bytes().to_vec()).unwrap_or_default();
    if tty {
        out.extend_from_slice(text);
    } else {
        out.extend(normalise_lf(text));
    }
    if !out.ends_with(b"\n") {
        out.extend_from_slice(b"\r\n");
    }
    out
}

fn error_line(prefix: Option<&str>, error: &str) -> Vec<u8> {
    terminal_line(prefix, format!("\x1b[31m{error}\x1b[0m").as_bytes(), false)
}

/// Most history written when a Logs view opens: about what the terminal's
/// scrollback keeps anyway.
const HISTORY_BYTES: usize = 8 * 1024 * 1024;
/// History is written in pieces this big, so parsing it never stalls a frame.
const FEED_CHUNK: usize = 64 * 1024;

/// One container's part of a Logs view as it opens.
struct Opening {
    tty: bool,
    lines: Vec<LogLine>,
    error: Option<String>,
}

/// Fill a Logs terminal: `sources`' history merged in time order (the newest
/// `LOG_TAIL_LINES` lines, or all of it when `full`), then each followed.
pub(super) async fn stream_logs(docker: Docker, sources: Vec<LogSource>, full: bool, out: Feeder, ended: Ended) {
    let tail = if full { "all".to_string() } else { LOG_TAIL_LINES.to_string() };
    let mut openings = join_all(sources.iter().map(|s| async {
        let tty = logs::has_tty(&docker, &s.id).await;
        match logs::history(&docker, &s.id, &tail).await {
            Ok(lines) => Opening { tty, lines, error: None },
            Err(e) => Opening { tty, lines: Vec::new(), error: Some(e.to_string()) },
        }
    }))
    .await;
    let lasts: Vec<Option<LogTime>> = openings.iter().map(|o| logs::last_time(&o.lines)).collect();
    let merged = logs::merge(
        openings.iter_mut().map(|o| std::mem::take(&mut o.lines)).collect(),
        (!full).then_some(LOG_TAIL_LINES),
    );
    // The newest lines that fit in HISTORY_BYTES.
    let mut bytes = 0;
    let keep = merged.iter().rev().take_while(|(_, l)| {
        bytes += l.text.len();
        bytes <= HISTORY_BYTES
    });
    let skip = merged.len() - keep.count();
    let mut text = Vec::new();
    for (i, line) in merged.into_iter().skip(skip) {
        text.extend(terminal_line(sources[i].prefix.as_deref(), &line.text, openings[i].tty));
        if text.len() >= FEED_CHUNK {
            out.feed(&std::mem::take(&mut text));
        }
    }
    for (source, opening) in sources.iter().zip(&openings) {
        if let Some(e) = &opening.error {
            text.extend(error_line(source.prefix.as_deref(), e));
        }
    }
    out.feed(&text);
    let follows = sources.iter().zip(&openings).zip(lasts).map(|((source, opening), after)| {
        let (docker, out, ended) = (&docker, &out, &ended);
        async move {
            if opening.error.is_some() {
                ended(StreamEnd { container: source.id.clone(), clean: false, last: None });
            } else {
                follow_into(docker, source, Some(opening.tty), after, out, ended).await;
            }
        }
    });
    join_all(follows).await;
}

/// Follow one container into a Logs terminal from after `after` (its whole
/// log when None), then report how it ended. `tty` is looked up when unknown.
pub(super) async fn follow_into(
    docker: &Docker,
    source: &LogSource,
    tty: Option<bool>,
    after: Option<LogTime>,
    out: &Feeder,
    ended: &Ended,
) {
    let tty = match tty {
        Some(t) => t,
        None => logs::has_tty(docker, &source.id).await,
    };
    let prefix = source.prefix.as_deref();
    let (result, last) =
        logs::follow(docker, &source.id, after, |l| out.feed(&terminal_line(prefix, &l.text, tty))).await;
    if let Err(e) = &result {
        out.feed(&error_line(prefix, e));
    }
    ended(StreamEnd { container: source.id.clone(), clean: result.is_ok(), last });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_lines_carry_the_prefix_and_end_the_line() {
        assert_eq!(terminal_line(Some("web-1 │ "), b"hi\n", false), b"web-1 \xe2\x94\x82 hi\r\n");
        assert_eq!(terminal_line(None, b"tty\r\n", true), b"tty\r\n");
        assert_eq!(terminal_line(None, b"no end", false), b"no end\r\n", "a cut-off last line still ends");
    }
}
