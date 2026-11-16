//! Runtime: terminal setup, the event loop, and effect execution.

use std::io::{self, Write};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange,
    EnableMouseCapture, EventStream,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use std::collections::HashMap;
use std::sync::Arc;

use super::inspect::ContentKey;
use super::{App, Content, Effect, MIN_FRAME};
use crate::doc::DocSurface;
use crate::docker::client::{detect_local_compose, Docker};
use crate::docker::refresh::{spawn_refresh, Refresher};
use crate::docker::stats::StatsSample;
use crate::event::AppEvent;
use crate::event::{EventRx, EventTx};
use crate::term::{Feeder, TermSurface, Wake};
use crate::ui::layout::PaneId;

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub host: Option<String>,
    pub project: Option<String>,
    pub compose_files: Vec<String>,
}

pub struct Runtime {
    pub docker: Docker,
    pub tx: EventTx,
    pub refresher: Refresher,
    pub compose_files: Vec<String>,
    stats_tasks: HashMap<String, tokio::task::AbortHandle>,
}

/// Restore the terminal (also used by the panic hook).
pub fn restore_terminal() {
    let mut out = io::stdout();
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    // Clear stale mouse modes first (herdr does the same), then enable ours.
    out.write_all(b"\x1b[?1006l\x1b[?1015l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l")?;
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste, EnableFocusChange)?;
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

pub async fn run(opts: Options) -> Result<()> {
    let docker = Docker::connect_to(&crate::docker::client::resolve_host(opts.host.as_deref()))?;
    let (tx, mut rx): (EventTx, EventRx) = tokio::sync::mpsc::unbounded_channel();
    let local = detect_local_compose(&opts.compose_files, opts.project.as_deref()).await;
    let refresher = spawn_refresh(docker.clone(), local, tx.clone());
    let mut rt = Runtime {
        docker: docker.clone(),
        tx,
        refresher,
        compose_files: opts.compose_files.clone(),
        stats_tasks: HashMap::new(),
    };

    let mut app = App::new(docker.host.clone());
    app.compose_files = opts.compose_files.clone();
    let mut term = setup_terminal()?;
    let size = term.size()?;
    app.size = (size.width, size.height);
    let mut events = EventStream::new();
    let mut last_draw = Instant::now() - MIN_FRAME;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let result: Result<()> = async {
        loop {
            if app.dirty && last_draw.elapsed() >= MIN_FRAME {
                term.draw(|f| app.render(f))?;
                app.dirty = false;
                last_draw = Instant::now();
            }
            let wait =
                if app.dirty { MIN_FRAME.saturating_sub(last_draw.elapsed()) } else { Duration::from_millis(250) };
            let effects = tokio::select! {
                ev = events.next() => match ev {
                    Some(Ok(ev)) => app.handle_event(ev),
                    Some(Err(e)) => return Err(e.into()),
                    None => vec![Effect::Quit],
                },
                Some(ev) = rx.recv() => app.handle_app_event(ev),
                _ = tokio::time::sleep(wait) => Vec::new(),
                _ = ticker.tick() => {
                    app.tick += 1;
                    app.dirty = true;
                    Vec::new()
                }
            };
            let mut effects = effects;
            effects.extend(app.sync_panes());
            for e in effects {
                rt.execute(&mut app, e);
            }
            if app.quit {
                return Ok(());
            }
        }
    }
    .await;
    restore_terminal();
    result
}

impl Runtime {
    fn wake_for(&self, pane: PaneId) -> Wake {
        let tx = self.tx.clone();
        Arc::new(move || {
            let _ = tx.send(AppEvent::TermActivity(pane));
        })
    }

    /// Install `content` into `pane` if it still wants `key` (None: always).
    fn install(app: &mut App, pane: PaneId, key: &Option<ContentKey>, content: Content) -> bool {
        match app.panes.get_mut(&pane) {
            Some(p) if key.is_none() || p.loaded == *key => {
                p.stop_task();
                p.content = content;
                app.dirty = true;
                true
            }
            _ => false,
        }
    }

    pub fn execute(&mut self, app: &mut App, effect: Effect) {
        match effect {
            Effect::Quit => app.quit = true,
            Effect::RefreshNow => self.refresher.poke(),
            Effect::Docker(op) => {
                let docker = self.docker.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let desc = op.describe();
                    let result = crate::docker::ops::run(&docker, op.clone()).await.map_err(|e| format!("{e:#}"));
                    let _ = tx.send(AppEvent::OpDone { op, desc, result });
                });
            }
            Effect::OpenUrl(url) => {
                let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
                let spawned = std::process::Command::new(opener)
                    .arg(&url)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                match spawned {
                    Ok(_) => app.flash(format!("opened {url}")),
                    Err(e) => app.show_error(format!("could not open {url}: {e}")),
                }
            }
            Effect::Copy(text) => {
                let how = crate::clipboard::copy(&text);
                let n = text.chars().count();
                app.flash(format!("copied {n} chars ({how})"));
            }
            Effect::Load { pane, key, spec } => {
                let docker = self.docker.clone();
                let tx = self.tx.clone();
                let files = self.compose_files.clone();
                let theme = app.theme;
                tokio::spawn(async move {
                    let lines = super::loader::load(&docker, spec, &files, &theme).await;
                    let _ = tx.send(AppEvent::Doc { pane, key, lines });
                });
            }
            Effect::StartLogs { pane, key, container_id } => {
                let (cols, rows) = app.content_size(pane);
                let surface = match TermSurface::stream(cols, rows, self.wake_for(pane)) {
                    Ok(s) => s,
                    Err(e) => {
                        Self::install(
                            app,
                            pane,
                            &Some(key),
                            Content::Doc(DocSurface::message(format!("Error: {e:#}"))),
                        );
                        return;
                    }
                };
                let (raw, lf) = (surface.feeder(false), surface.feeder(true));
                if !Self::install(app, pane, &Some(key.clone()), Content::Term(surface)) {
                    return;
                }
                let docker = self.docker.clone();
                let tx = self.tx.clone();
                let handle = tokio::spawn(async move {
                    stream_logs(&docker, &container_id, &raw, &lf).await;
                    let _ = tx.send(AppEvent::LogsEnded(pane, key));
                });
                if let Some(p) = app.panes.get_mut(&pane) {
                    p.task = Some(handle.abort_handle());
                }
            }
            Effect::SpawnTerm { pane, key, argv } => {
                let (cols, rows) = app.content_size(pane);
                let tx = self.tx.clone();
                let on_exit = Arc::new(move |sid, code| {
                    let _ = tx.send(AppEvent::TermExited(pane, sid, code));
                });
                let env = [("DOCKER_HOST".to_string(), self.docker.host.clone())];
                let content = match TermSurface::spawn(&argv, &env, cols, rows, self.wake_for(pane), on_exit) {
                    Ok(s) => Content::Term(s),
                    Err(e) => Content::Doc(DocSurface::message(format!("Error: {e:#}"))),
                };
                Self::install(app, pane, &key, content);
            }
            Effect::SyncStats(running) => {
                self.stats_tasks.retain(|id, h| {
                    let keep = running.contains(id) && !h.is_finished();
                    if !keep {
                        h.abort();
                    }
                    keep
                });
                for id in running {
                    if self.stats_tasks.contains_key(&id) {
                        continue;
                    }
                    let docker = self.docker.clone();
                    let tx = self.tx.clone();
                    let cid = id.clone();
                    let handle = tokio::spawn(async move {
                        let opts = bollard::query_parameters::StatsOptions { stream: true, one_shot: false };
                        let mut stream = docker.api.stats(&cid, Some(opts));
                        while let Some(Ok(s)) = stream.next().await {
                            if tx.send(AppEvent::Stats(cid.clone(), StatsSample::from_bollard(&s))).is_err() {
                                break;
                            }
                        }
                    });
                    self.stats_tasks.insert(id, handle.abort_handle());
                }
            }
        }
    }
}

/// Follow a container's logs (lazydocker: since 60m, stdout+stderr).
async fn stream_logs(docker: &Docker, id: &str, raw: &Feeder, lf: &Feeder) {
    use bollard::container::LogOutput;
    let tty =
        docker.api.inspect_container(id, None).await.ok().and_then(|i| i.config.and_then(|c| c.tty)).unwrap_or(false);
    let feeder = if tty { raw } else { lf };
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(3600) as i32)
        .unwrap_or(0);
    let opts = bollard::query_parameters::LogsOptions {
        follow: true,
        stdout: true,
        stderr: true,
        since,
        tail: "all".into(),
        ..Default::default()
    };
    let mut stream = docker.api.logs(id, Some(opts));
    while let Some(item) = stream.next().await {
        match item {
            Ok(LogOutput::StdOut { message } | LogOutput::StdErr { message } | LogOutput::Console { message }) => {
                feeder.feed(&message)
            }
            Ok(LogOutput::StdIn { .. }) => {}
            Err(e) => {
                feeder.feed(format!("\r\n\x1b[31m{e}\x1b[0m\r\n").as_bytes());
                break;
            }
        }
    }
}
