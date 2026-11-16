//! Runtime: terminal setup, the event loop, and effect execution.

use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, EventStream,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{cursor, execute};
use futures_util::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use super::inspect::{ContentKey, LOG_TAIL_LINES};
use super::{App, Content, Effect};
use crate::doc::DocSurface;
use crate::docker::client::{detect_local_compose, Docker};
use crate::docker::ops;
use crate::docker::refresh::{spawn_refresh, Refresher};
use crate::docker::stats::StatsStreams;
use crate::event::{AppEvent, EventRx, EventTx};
use crate::term::{Feeder, TermSurface, Wake};
use crate::ui::layout::PaneId;

/// Shortest time between frames (about 60 fps).
const MIN_FRAME: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Default)]
pub struct Options {
    pub host: Option<String>,
    pub project: Option<String>,
    pub compose_files: Vec<String>,
}

/// Executes effects: the side of the app that talks to docker and processes.
struct Runtime {
    docker: Docker,
    tx: EventTx,
    refresher: Refresher,
    stats: StatsStreams,
}

type Term = Terminal<CrosstermBackend<io::Stdout>>;

/// Restore the terminal (also used by the panic hook).
fn restore_terminal() {
    let mut out = io::stdout();
    let _ = execute!(out, DisableMouseCapture, DisableBracketedPaste, LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

fn setup_terminal() -> Result<Term> {
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    // Clear stale mouse modes first (herdr does the same), then enable ours.
    out.write_all(b"\x1b[?1006l\x1b[?1015l\x1b[?1005l\x1b[?1003l\x1b[?1002l\x1b[?1000l")?;
    execute!(out, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        prev(info);
    }));
    Ok(Terminal::new(CrosstermBackend::new(out))?)
}

pub async fn run(opts: Options) -> Result<()> {
    let docker = Docker::connect(opts.host.as_deref())?;
    let (tx, rx): (EventTx, EventRx) = tokio::sync::mpsc::unbounded_channel();
    let local = detect_local_compose(&opts.compose_files, opts.project.as_deref()).await;
    let refresher = spawn_refresh(docker.clone(), local, tx.clone());
    let mut app = App::new(docker.host.clone());
    app.compose_files = opts.compose_files;
    let mut rt = Runtime { docker, tx, refresher, stats: StatsStreams::default() };
    // Restore the terminal however we leave, including a failed setup.
    let result = match setup_terminal() {
        Ok(term) => event_loop(term, &mut app, &mut rt, rx).await,
        Err(e) => Err(e),
    };
    restore_terminal();
    result
}

async fn event_loop(mut term: Term, app: &mut App, rt: &mut Runtime, mut rx: EventRx) -> Result<()> {
    let mut events = EventStream::new();
    let mut last_draw = Instant::now() - MIN_FRAME;
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if app.dirty && last_draw.elapsed() >= MIN_FRAME {
            term.draw(|f| app.render(f))?;
            app.dirty = false;
            last_draw = Instant::now();
        }
        // Without a pending redraw, still wake now and then: content syncs
        // (Top refreshes, the find refresh) run after every wake-up.
        let wait = if app.dirty { MIN_FRAME.saturating_sub(last_draw.elapsed()) } else { Duration::from_millis(250) };
        let mut effects = tokio::select! {
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
        effects.extend(app.sync_panes());
        for e in effects {
            rt.execute(app, e);
        }
        if app.quit {
            return Ok(());
        }
    }
}

impl Runtime {
    fn wake_for(&self, pane: PaneId) -> Wake {
        let tx = self.tx.clone();
        Arc::new(move || {
            let _ = tx.send(AppEvent::TermActivity(pane));
        })
    }

    /// Install `content` into `pane` if it still wants `key` (None: always).
    fn install(app: &mut App, pane: PaneId, key: Option<&ContentKey>, content: Content) -> bool {
        match app.panes.get_mut(&pane) {
            Some(p) if key.is_none() || p.loaded.as_ref() == key => {
                p.set_content(content);
                app.dirty = true;
                true
            }
            _ => false,
        }
    }

    fn execute(&mut self, app: &mut App, effect: Effect) {
        match effect {
            Effect::Quit => app.quit = true,
            Effect::RefreshNow => self.refresher.poke(),
            Effect::Docker(op) => {
                let docker = self.docker.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let result = ops::run(&docker, op.clone()).await.map_err(|e| format!("{e:#}"));
                    let _ = tx.send(AppEvent::OpDone { op, result });
                });
            }
            Effect::OpenUrl(url) => open_url(app, &url),
            Effect::Copy(text) => {
                let how = crate::clipboard::copy(&text);
                let n = text.chars().count();
                app.flash(format!("copied {n} chars ({how})"));
            }
            Effect::Load { pane, key, spec } => {
                let docker = self.docker.clone();
                let tx = self.tx.clone();
                let theme = app.theme;
                tokio::spawn(async move {
                    let lines = super::loader::load(&docker, spec, &theme).await;
                    let _ = tx.send(AppEvent::Doc { pane, key, lines });
                });
            }
            Effect::StartLogs { pane, key, container_id, full } => self.start_logs(app, pane, key, container_id, full),
            Effect::SpawnTerm { pane, key, argv } => {
                let (cols, rows) = app.content_size(pane);
                let tx = self.tx.clone();
                let on_exit = Arc::new(move |surface, code| {
                    let _ = tx.send(AppEvent::TermExited { pane, surface, code });
                });
                let env = [("DOCKER_HOST".to_string(), self.docker.host.clone())];
                let content = match TermSurface::spawn(&argv, &env, cols, rows, self.wake_for(pane), on_exit) {
                    Ok(s) => Content::Term(s),
                    Err(e) => Content::Doc(DocSurface::message(format!("Error: {e:#}"))),
                };
                Self::install(app, pane, key.as_ref(), content);
            }
            Effect::SyncStats(running) => self.stats.sync(&running, &self.docker, &self.tx),
        }
    }

    /// Show a container's logs in a terminal surface fed by a bollard stream.
    fn start_logs(&mut self, app: &mut App, pane: PaneId, key: ContentKey, container_id: String, full: bool) {
        let (cols, rows) = app.content_size(pane);
        let surface = match TermSurface::stream(cols, rows, self.wake_for(pane)) {
            Ok(s) => s,
            Err(e) => {
                let message = Content::Doc(DocSurface::message(format!("Error: {e:#}")));
                Self::install(app, pane, Some(&key), message);
                return;
            }
        };
        let (raw, lf) = (surface.feeder(false), surface.feeder(true));
        if !Self::install(app, pane, Some(&key), Content::Term(surface)) {
            return;
        }
        let docker = self.docker.clone();
        let tx = self.tx.clone();
        let task = tokio::spawn(async move {
            let clean = stream_logs(&docker, &container_id, full, &raw, &lf).await;
            let _ = tx.send(AppEvent::LogsEnded { pane, key, clean });
        });
        if let Some(p) = app.panes.get_mut(&pane) {
            p.task = Some(task.abort_handle());
        }
    }
}

/// Open a URL in the desktop's browser.
fn open_url(app: &mut App, url: &str) {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let spawned = std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(mut child) => {
            // Reap it so it doesn't linger as a zombie.
            std::thread::spawn(move || child.wait());
            app.flash(format!("opened {url}"));
        }
        Err(e) => app.show_error(format!("could not open {url}: {e}")),
    }
}

/// Follow a container's logs (last `LOG_TAIL_LINES` lines, or everything
/// when `full`; stdout+stderr). Returns false if the stream ended with an error.
async fn stream_logs(docker: &Docker, id: &str, full: bool, raw: &Feeder, lf: &Feeder) -> bool {
    use bollard::container::LogOutput;
    // Without a TTY, output has bare `\n` line ends.
    let tty =
        docker.api.inspect_container(id, None).await.ok().and_then(|i| i.config.and_then(|c| c.tty)).unwrap_or(false);
    let feeder = if tty { raw } else { lf };
    let opts = bollard::query_parameters::LogsOptions {
        follow: true,
        stdout: true,
        stderr: true,
        tail: if full { "all".into() } else { LOG_TAIL_LINES.to_string() },
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
                return false;
            }
        }
    }
    true
}
