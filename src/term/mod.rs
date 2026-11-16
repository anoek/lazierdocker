//! Terminal surface backed by libghostty-vt: an embedded terminal emulator for
//! `docker exec` shells, `docker attach`, attached compose commands and logs.
//!
//! libghostty-vt handles are `!Send`, so each terminal lives on the UI thread.
//! The PTY pump thread and log feeders only move bytes: they send them over a
//! channel and wake the app, and the surface parses whatever is queued before
//! it is drawn or queried.

pub mod find;
pub mod keys;
pub mod render;

use std::cell::{RefCell, RefMut};
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crossterm::event::KeyEvent as CtKey;
use libghostty_vt::fmt::Format;
use libghostty_vt::key::Encoder as KeyEncoder;
use libghostty_vt::render::{CellIterator, RowIterator};
use libghostty_vt::screen::Screen;
use libghostty_vt::selection::FormatOptions;
use libghostty_vt::terminal::{Mode, Point, PointCoordinate, ScrollViewport};
use libghostty_vt::{mouse, RenderState, Terminal, TerminalOptions};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

pub use libghostty_vt::key::Mods;
pub use libghostty_vt::mouse::{Action as MouseAction, Button as MouseButton};

/// `TERM` for children: every container image has xterm-256color's terminfo.
const TERM: &str = "xterm-256color";
/// Scrollback budget per terminal, in bytes. libghostty-vt's header calls
/// `max_scrollback` a line count, but Ghostty passes it to the page list as a
/// byte limit (`PageList.max_size`).
const SCROLLBACK_BYTES: usize = 8 * 1024 * 1024;
/// Smallest terminal size; libghostty and PTYs reject zero.
const MIN_SIZE: u16 = 2;
/// Most bytes parsed per drain, so a flooding child cannot stall a frame.
const DRAIN_BUDGET: usize = 4 * 1024 * 1024;

pub type Wake = Arc<dyn Fn() + Send + Sync>;
/// Called with (surface id, exit code) when the child exits.
pub type OnExit = Arc<dyn Fn(u64, Option<u32>) + Send + Sync>;

static NEXT_SURFACE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

struct Core {
    term: Terminal<'static, 'static>,
    rs: RenderState<'static>,
    keys: KeyEncoder<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
}

/// Feeds bytes into a terminal from another thread (logs streams).
#[derive(Clone)]
pub struct Feeder {
    tx: Sender<Vec<u8>>,
    pending: Arc<AtomicBool>,
    wake: Wake,
    /// Convert bare `\n` to `\r\n` (non-PTY streams).
    lf_to_crlf: bool,
}

impl Feeder {
    pub fn feed(&self, bytes: &[u8]) {
        let bytes = if self.lf_to_crlf { normalise_lf(bytes) } else { bytes.to_vec() };
        if self.tx.send(bytes).is_ok() && !self.pending.swap(true, Ordering::AcqRel) {
            (self.wake)();
        }
    }
}

/// Convert lone `\n` into `\r\n` so non-TTY output starts each line at column 0.
pub fn normalise_lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 16);
    let mut prev = 0u8;
    for &b in bytes {
        if b == b'\n' && prev != b'\r' {
            out.push(b'\r');
        }
        out.push(b);
        prev = b;
    }
    out
}

pub struct TermSurface {
    core: RefCell<Core>,
    /// Output waiting to be parsed (from the PTY pump or feeders).
    rx: Receiver<Vec<u8>>,
    tx: Sender<Vec<u8>>,
    /// Bytes for the child, written by a dedicated thread so a child that
    /// stops reading never blocks the UI.
    writer: Option<Sender<Vec<u8>>>,
    /// Query replies (DA, DSR, …) the terminal produced while parsing.
    replies: Rc<RefCell<Vec<u8>>>,
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    exit: Arc<Mutex<Option<Option<u32>>>>,
    pending: Arc<AtomicBool>,
    wake: Wake,
    size: (u16, u16),
    id: u64,
}

impl std::fmt::Debug for TermSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermSurface").field("size", &self.size).finish()
    }
}

fn err(what: &str) -> impl FnOnce(libghostty_vt::Error) -> anyhow::Error + '_ {
    move |e| anyhow::anyhow!("{what}: {e:?}")
}

fn new_core(cols: u16, rows: u16, replies: Rc<RefCell<Vec<u8>>>) -> Result<Core> {
    let mut term = Terminal::new(TerminalOptions {
        cols: cols.max(MIN_SIZE),
        rows: rows.max(MIN_SIZE),
        max_scrollback: SCROLLBACK_BYTES,
    })
    .map_err(err("terminal"))?;
    term.on_pty_write(move |_, b| replies.borrow_mut().extend_from_slice(b)).map_err(err("callback"))?;
    Ok(Core {
        term,
        rs: RenderState::new().map_err(err("render state"))?,
        keys: KeyEncoder::new().map_err(err("key encoder"))?,
        rows: RowIterator::new().map_err(err("row iterator"))?,
        cells: CellIterator::new().map_err(err("cell iterator"))?,
    })
}

impl TermSurface {
    /// A terminal fed from a byte stream (no child process).
    pub fn stream(cols: u16, rows: u16, wake: Wake) -> Result<Self> {
        let replies = Rc::new(RefCell::new(Vec::new()));
        let core = new_core(cols, rows, replies.clone())?;
        let (tx, rx) = std::sync::mpsc::channel();
        Ok(Self {
            core: RefCell::new(core),
            rx,
            tx,
            writer: None,
            replies,
            master: None,
            killer: None,
            exit: Arc::new(Mutex::new(None)),
            pending: Arc::new(AtomicBool::new(false)),
            wake,
            size: (cols, rows),
            id: NEXT_SURFACE_ID.fetch_add(1, Ordering::Relaxed),
        })
    }

    /// Spawn `argv` in a PTY of `cols`×`rows`.
    pub fn spawn(
        argv: &[String],
        env: &[(String, String)],
        cols: u16,
        rows: u16,
        wake: Wake,
        on_exit: OnExit,
    ) -> Result<Self> {
        let (cols, rows) = (cols.max(MIN_SIZE), rows.max(MIN_SIZE));
        let mut me = Self::stream(cols, rows, wake.clone())?;
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("opening pty")?;
        let mut cmd = CommandBuilder::new(argv.first().context("empty command")?);
        cmd.args(&argv[1..]);
        cmd.env("TERM", TERM);
        cmd.env("COLORTERM", "truecolor");
        for k in ["TMUX", "TMUX_PANE", "KITTY_WINDOW_ID", "TERM_PROGRAM", "TERM_PROGRAM_VERSION"] {
            cmd.env_remove(k);
        }
        for (k, v) in env {
            cmd.env(k, v);
        }
        if let Ok(dir) = std::env::current_dir() {
            cmd.cwd(dir);
        }
        let mut child = pair.slave.spawn_command(cmd).with_context(|| format!("spawning {}", argv.join(" ")))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().context("pty reader")?;
        let mut pty_writer = pair.master.take_writer().context("pty writer")?;
        let (writer, write_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new().name("pty-write".into()).spawn(move || {
            while let Ok(bytes) = write_rx.recv() {
                if pty_writer.write_all(&bytes).is_err() {
                    break;
                }
            }
        })?;
        me.killer = Some(child.clone_killer());
        me.master = Some(pair.master);
        me.writer = Some(writer);

        // Pump: child output → the surface's queue.
        {
            let tx = me.tx.clone();
            let pending = me.pending.clone();
            std::thread::Builder::new().name("pty-pump".into()).spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                    if !pending.swap(true, Ordering::AcqRel) {
                        wake();
                    }
                }
            })?;
        }
        // Reaper.
        {
            let exit = me.exit.clone();
            let sid = me.id;
            std::thread::Builder::new().name("pty-wait".into()).spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code());
                *exit.lock().expect("exit") = Some(code);
                on_exit(sid, code);
            })?;
        }
        Ok(me)
    }

    pub fn feeder(&self, lf_to_crlf: bool) -> Feeder {
        Feeder { tx: self.tx.clone(), pending: self.pending.clone(), wake: self.wake.clone(), lf_to_crlf }
    }

    /// The terminal, with all queued output parsed and its replies sent.
    fn core(&self) -> RefMut<'_, Core> {
        let mut core = self.core.borrow_mut();
        let mut parsed = 0;
        while parsed < DRAIN_BUDGET {
            match self.rx.try_recv() {
                Ok(bytes) => {
                    core.term.vt_write(&bytes);
                    parsed += bytes.len();
                }
                Err(_) => break,
            }
        }
        if parsed >= DRAIN_BUDGET {
            // More is queued: come back for it next frame.
            self.pending.store(true, Ordering::Release);
            (self.wake)();
        }
        self.flush_replies();
        core
    }

    /// Unique identity of this surface (distinguishes replaced content).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The surface runs a child process (rather than showing a stream).
    fn is_pty(&self) -> bool {
        self.writer.is_some()
    }

    /// `Some(code)` once the child has exited (`code` is None if unknown).
    pub fn exited(&self) -> Option<Option<u32>> {
        *self.exit.lock().expect("exit")
    }

    fn kill(&mut self) {
        if let Some(k) = self.killer.as_mut() {
            let _ = k.kill();
        }
    }

    fn write_child(&self, bytes: &[u8]) {
        if let Some(w) = &self.writer {
            let _ = w.send(bytes.to_vec());
        }
    }

    fn flush_replies(&self) {
        let out = std::mem::take(&mut *self.replies.borrow_mut());
        if !out.is_empty() {
            self.write_child(&out);
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(MIN_SIZE), rows.max(MIN_SIZE));
        if (cols, rows) == self.size {
            return;
        }
        self.size = (cols, rows);
        let _ = self.core().term.resize(cols, rows, 1, 1);
        if let Some(m) = &self.master {
            let _ = m.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
        self.flush_replies();
    }

    pub fn send_key(&mut self, k: &CtKey) {
        if self.exited().is_some() {
            return;
        }
        let bytes = {
            let mut c = self.core();
            c.term.scroll_viewport(ScrollViewport::Bottom);
            let Core { term, keys: enc, .. } = &mut *c;
            keys::encode(enc, term, k)
        };
        if let Some(b) = bytes {
            self.write_child(&b);
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed = self.core().term.mode(Mode::BRACKETED_PASTE).unwrap_or(false);
        let mut out = Vec::new();
        if bracketed {
            out.extend_from_slice(b"\x1b[200~");
            // Never let pasted text terminate the bracket early.
            out.extend_from_slice(text.replace("\x1b[201~", "").as_bytes());
            out.extend_from_slice(b"\x1b[201~");
        } else {
            out.extend_from_slice(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
        self.write_child(&out);
    }

    pub fn mouse_tracking(&self) -> bool {
        self.is_pty() && self.core().term.is_mouse_tracking().unwrap_or(false)
    }

    /// Forward a mouse event to a child that requested mouse reporting.
    /// `col`/`row` are relative to the terminal's top-left cell.
    pub fn send_mouse(&mut self, action: MouseAction, button: Option<MouseButton>, mods: Mods, col: u16, row: u16) {
        let mut bytes = Vec::new();
        {
            let c = self.core();
            let (Ok(mut enc), Ok(mut ev)) = (mouse::Encoder::new(), mouse::Event::new()) else { return };
            // One pixel per cell: positions are cell coordinates.
            enc.set_options_from_terminal(&c.term).set_size(mouse::EncoderSize {
                screen_width: self.size.0 as u32,
                screen_height: self.size.1 as u32,
                cell_width: 1,
                cell_height: 1,
                padding_top: 0,
                padding_bottom: 0,
                padding_right: 0,
                padding_left: 0,
            });
            ev.set_action(action)
                .set_button(button)
                .set_mods(mods)
                .set_position(mouse::Position { x: col as f32, y: row as f32 });
            let _ = enc.encode_to_vec(&ev, &mut bytes);
        }
        if !bytes.is_empty() {
            self.write_child(&bytes);
        }
    }

    /// Mouse wheel: to the child when it tracks the mouse, as arrow keys on the
    /// alternate screen, otherwise scroll our own scrollback.
    pub fn wheel(&mut self, lines: i32, col: u16, row: u16) {
        if self.mouse_tracking() {
            let button = if lines < 0 { MouseButton::Four } else { MouseButton::Five };
            for _ in 0..lines.unsigned_abs().div_ceil(3) {
                self.send_mouse(MouseAction::Press, Some(button), Mods::empty(), col, row);
            }
            return;
        }
        let alt = {
            let c = self.core();
            c.term.active_screen().ok() == Some(Screen::Alternate) && c.term.mode(Mode::ALT_SCROLL).unwrap_or(false)
        };
        if alt && self.is_pty() {
            let code = if lines < 0 { crossterm::event::KeyCode::Up } else { crossterm::event::KeyCode::Down };
            for _ in 0..lines.unsigned_abs() {
                self.send_key(&CtKey::new(code, crossterm::event::KeyModifiers::NONE));
            }
            return;
        }
        self.scroll_by(lines);
    }

    pub fn scroll_by(&mut self, lines: i32) {
        self.core().term.scroll_viewport(ScrollViewport::Delta(lines as isize));
    }

    /// Show the newest output (new output then keeps it in view).
    pub fn scroll_bottom(&mut self) {
        self.core().term.scroll_viewport(ScrollViewport::Bottom);
    }

    pub fn scroll_top(&mut self) {
        self.core().term.scroll_viewport(ScrollViewport::Top);
    }

    /// Absolute row of the top visible line (for selections).
    pub fn viewport_top(&self) -> usize {
        self.scrollbar().1
    }

    /// (total rows, viewport offset, viewport length).
    pub fn scrollbar(&self) -> (usize, usize, usize) {
        self.core().term.scrollbar().map(|s| (s.total as usize, s.offset as usize, s.len as usize)).unwrap_or((0, 0, 0))
    }

    /// Text between screen-absolute (row, col) positions, inclusive, in either order.
    pub fn text_between(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let (start, end) = if start <= end { (start, end) } else { (end, start) };
        let at = |(row, col): (usize, usize)| Point::Screen(PointCoordinate { x: col as u16, y: row as u32 });
        read_text(&self.core().term, at(start), at(end))
    }

    /// Text of a screen-absolute row.
    pub fn row_text(&self, row: usize) -> String {
        let core = self.core();
        row_text(&core.term, row)
    }

    /// Every match of `query` in the scrollback and screen, top to bottom.
    pub fn find(&self, query: &str) -> Vec<find::Match> {
        let c = self.core();
        let total = c.term.scrollbar().map(|s| s.total as usize).unwrap_or(0);
        let cols = c.term.cols().unwrap_or(1).max(1);
        let rows: Vec<(String, bool)> = (0..total)
            .map(|y| {
                let wrapped = c
                    .term
                    .grid_ref(Point::Screen(PointCoordinate { x: 0, y: y as u32 }))
                    .ok()
                    .and_then(|g| g.row().ok())
                    .and_then(|r| r.is_wrapped().ok())
                    .unwrap_or(false);
                (row_text(&c.term, y), wrapped)
            })
            .collect();
        find::find_in_rows(&rows, cols as usize, query)
    }

    /// Scroll so `row` (screen-absolute) is visible, centred if it was not.
    pub fn reveal_row(&mut self, row: usize) {
        let (_, top, len) = self.scrollbar();
        if row >= top && row < top + len {
            return;
        }
        self.core().term.scroll_viewport(ScrollViewport::Row(row.saturating_sub(len / 2)));
    }

    /// The viewport shows the newest output (new output scrolls it).
    pub fn at_bottom(&self) -> bool {
        let (total, top, len) = self.scrollbar();
        top + len >= total
    }

    /// Plain text of the visible viewport rows.
    #[cfg(test)]
    pub fn viewport_text(&self) -> Vec<String> {
        let top = self.viewport_top();
        let rows = self.core().term.rows().unwrap_or(0) as usize;
        (top..top + rows).map(|r| self.row_text(r)).collect()
    }

    /// Write a status line into the terminal (e.g. "[process exited]").
    pub fn notice(&mut self, text: &str) {
        // Ends the line too: a log view's resumed output follows it.
        self.core().term.vt_write(format!("\r\n\x1b[0;2m{text}\x1b[0m\r\n").as_bytes());
    }

    /// Mark output as consumed so the next output wakes the app again.
    pub fn ack(&self) {
        self.pending.store(false, Ordering::Release);
    }
}

/// Plain text of one screen-absolute row (empty past the end).
fn row_text(term: &Terminal<'_, '_>, row: usize) -> String {
    let last = term.cols().unwrap_or(1).saturating_sub(1);
    let at = |x: u16| Point::Screen(PointCoordinate { x, y: row as u32 });
    read_text(term, at(0), at(last))
}

/// Plain text of the inclusive range between two points, as copied to the
/// clipboard (soft wraps joined, trailing blanks trimmed).
fn read_text(term: &Terminal<'_, '_>, start: Point, end: Point) -> String {
    let (Ok(start), Ok(end)) = (term.grid_ref(start), term.grid_ref(end)) else { return String::new() };
    let sel = libghostty_vt::selection::Selection::new(start, end, false);
    let opts =
        FormatOptions::new().with_emit_format(Format::Plain).with_unwrap(true).with_trim(true).with_selection(&sel);
    match term.format_selection_alloc(None, opts) {
        Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
        _ => String::new(),
    }
}

impl Drop for TermSurface {
    fn drop(&mut self) {
        if self.exited().is_none() {
            self.kill();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn nop_wake() -> Wake {
        Arc::new(|| {})
    }

    fn wait_for(mut f: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn spawn_echo_renders_text() {
        let t = TermSurface::spawn(
            &["sh".into(), "-c".into(), "printf 'hello\\033[1m world\\033[0m'; sleep 5".into()],
            &[],
            40,
            5,
            nop_wake(),
            Arc::new(|_, _| {}),
        )
        .unwrap();
        assert!(
            wait_for(|| t.viewport_text().first().is_some_and(|l| l.contains("hello world"))),
            "{:?}",
            t.viewport_text()
        );
    }

    #[test]
    fn terminal_child_exit_marks_pane() {
        let exited = Arc::new(Mutex::new(None));
        let e2 = exited.clone();
        let t = TermSurface::spawn(
            &["sh".into(), "-c".into(), "exit 3".into()],
            &[],
            20,
            5,
            nop_wake(),
            Arc::new(move |_, c| *e2.lock().unwrap() = Some(c)),
        )
        .unwrap();
        assert!(wait_for(|| t.exited().is_some()));
        assert_eq!(t.exited(), Some(Some(3)));
        assert_eq!(*exited.lock().unwrap(), Some(Some(3)));
    }

    #[test]
    fn keys_reach_child() {
        let mut t = TermSurface::spawn(&["cat".into()], &[], 30, 5, nop_wake(), Arc::new(|_, _| {})).unwrap();
        for c in "ping".chars() {
            t.send_key(&CtKey::new(crossterm::event::KeyCode::Char(c), crossterm::event::KeyModifiers::NONE));
        }
        t.send_key(&CtKey::new(crossterm::event::KeyCode::Enter, crossterm::event::KeyModifiers::NONE));
        assert!(
            wait_for(|| t.viewport_text().iter().filter(|l| l.contains("ping")).count() >= 2),
            "{:?}",
            t.viewport_text()
        );
    }

    #[test]
    fn resize_propagates() {
        let mut t = TermSurface::spawn(
            &["sh".into(), "-c".into(), "sleep 0.3; stty size; sleep 5".into()],
            &[],
            30,
            5,
            nop_wake(),
            Arc::new(|_, _| {}),
        )
        .unwrap();
        t.resize(50, 7);
        assert!(wait_for(|| t.viewport_text().iter().any(|l| l.contains("7 50"))), "{:?}", t.viewport_text());
    }

    #[test]
    fn query_replies_reach_child() {
        // Primary device attributes: the terminal must answer, or programs that
        // probe the terminal (vim, fish) hang waiting.
        let t = TermSurface::spawn(
            &[
                "sh".into(),
                "-c".into(),
                r#"stty raw -echo; printf '\033[c'; r=$(dd bs=1 count=3 2>/dev/null | od -An -c | tr -d ' '); printf '\r\ngot:%s' "$r"; sleep 5"#.into(),
            ],
            &[],
            40,
            5,
            nop_wake(),
            Arc::new(|_, _| {}),
        )
        .unwrap();
        assert!(wait_for(|| t.viewport_text().iter().any(|l| l.contains("got:033[?"))), "{:?}", t.viewport_text());
    }

    #[test]
    fn writes_never_block_the_caller() {
        // `sleep` never reads stdin, so the PTY input buffer fills up.
        let mut t = TermSurface::spawn(
            &["sh".into(), "-c".into(), "stty raw -echo; sleep 30".into()],
            &[],
            20,
            5,
            nop_wake(),
            Arc::new(|_, _| {}),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        // Surfaces are !Send, so a watchdog fails the run if the paste blocks.
        let done = Arc::new(AtomicBool::new(false));
        let d2 = done.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(5));
            if !d2.load(Ordering::Acquire) {
                eprintln!("paste blocked the caller");
                std::process::exit(101);
            }
        });
        let start = Instant::now();
        t.paste(&"x".repeat(1 << 20));
        done.store(true, Ordering::Release);
        assert!(start.elapsed() < Duration::from_secs(2), "paste blocked the caller");
    }

    #[test]
    fn logs_lf_normalised() {
        let t = TermSurface::stream(20, 4, nop_wake()).unwrap();
        let f = t.feeder(true);
        f.feed(b"one\ntwo\r\nthree\n");
        assert_eq!(&t.viewport_text()[..3], &["one", "two", "three"]);
    }

    #[test]
    fn feeders_work_from_other_threads() {
        let woken = Arc::new(AtomicBool::new(false));
        let w2 = woken.clone();
        let t = TermSurface::stream(20, 3, Arc::new(move || w2.store(true, Ordering::Release))).unwrap();
        let f = t.feeder(true);
        std::thread::spawn(move || f.feed(b"from a thread\n")).join().unwrap();
        assert!(woken.load(Ordering::Acquire), "feeding wakes the app");
        assert_eq!(t.viewport_text()[0], "from a thread");
    }

    #[test]
    fn scrollback_and_selection_text() {
        let mut t = TermSurface::stream(10, 3, nop_wake()).unwrap();
        let f = t.feeder(true);
        for i in 0..10 {
            f.feed(format!("line{i}\n").as_bytes());
        }
        let (total, offset, len) = t.scrollbar();
        assert_eq!(len, 3);
        assert!(total >= 10 && offset + len == total, "{total} {offset} {len}");
        t.scroll_by(-2);
        assert_eq!(t.viewport_top(), offset - 2);
        let top = t.viewport_top();
        assert_eq!(t.text_between((top, 0), (top + 1, 9)), "line6\nline7");
    }

    #[test]
    fn find_searches_scrollback_and_soft_wraps() {
        let mut t = TermSurface::stream(10, 3, nop_wake()).unwrap();
        let f = t.feeder(true);
        f.feed(b"needle one\n");
        for i in 0..10 {
            f.feed(format!("line{i}\n").as_bytes());
        }
        // 14 chars in a 10-column terminal: "needle" wraps across rows.
        f.feed(b"abcdefneedle x\n");
        let m = t.find("needle");
        assert_eq!(m.len(), 2, "{m:?}");
        assert_eq!(m[0].start, (0, 0));
        assert_eq!((m[1].start.1, m[1].end.0 - m[1].start.0, m[1].end.1), (6, 1, 1));
        // Revealing an off-screen match scrolls to it.
        assert!(t.at_bottom());
        t.reveal_row(0);
        assert_eq!(t.viewport_top(), 0);
        assert!(!t.at_bottom());
    }
}
