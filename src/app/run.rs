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

use super::{App, Effect, MIN_FRAME};
use crate::docker::client::{detect_local_compose, Docker};
use crate::docker::refresh::{spawn_refresh, Refresher};
use crate::event::{EventRx, EventTx};

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
    let docker = match &opts.host {
        Some(h) => Docker::connect_to(h)?,
        None => Docker::connect()?,
    };
    let (tx, mut rx): (EventTx, EventRx) = tokio::sync::mpsc::unbounded_channel();
    let local = detect_local_compose(&opts.compose_files, opts.project.as_deref()).await;
    let refresher = spawn_refresh(docker.clone(), local, tx.clone());
    let rt = Runtime { docker: docker.clone(), tx, refresher };

    let mut app = App::new(docker.host.clone());
    let mut term = setup_terminal()?;
    let size = term.size()?;
    app.size = (size.width, size.height);
    let mut events = EventStream::new();
    let mut last_draw = Instant::now() - MIN_FRAME;

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
            };
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
    pub fn execute(&self, app: &mut App, effect: Effect) {
        match effect {
            Effect::Quit => app.quit = true,
            Effect::RefreshNow => self.refresher.poke(),
        }
    }
}
