//! Terminal surface backed by libghostty-vt: an embedded terminal emulator for
//! `docker exec` shells, `docker attach`, attached compose commands and logs.

pub mod keys;
pub mod render;

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crossterm::event::KeyEvent as CtKey;
use ghostty_vt::{
    ffi, ActiveScreen, KeyEncoder, MouseEncoder, MouseEvent, RenderState, RowCells, RowIterator, Terminal,
};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

/// Scrollback budget per terminal, in bytes.
pub const SCROLLBACK_BYTES: usize = 8 * 1024 * 1024;

pub type Wake = Arc<dyn Fn() + Send + Sync>;
pub type OnExit = Arc<dyn Fn(Option<u32>) + Send + Sync>;

pub(crate) struct Core {
    pub term: Terminal,
    pub rs: RenderState,
    pub keys: KeyEncoder,
    pub ri: RowIterator,
    pub rc: RowCells,
}

/// Feeds bytes into a terminal from another thread (logs streams).
#[derive(Clone)]
pub struct Feeder {
    core: Arc<Mutex<Core>>,
    pending: Arc<AtomicBool>,
    wake: Wake,
    /// Convert bare `\n` to `\r\n` (non-PTY streams).
    lf_to_crlf: bool,
}

impl Feeder {
    pub fn feed(&self, bytes: &[u8]) {
        {
            let mut core = self.core.lock().expect("term core");
            if self.lf_to_crlf {
                core.term.write(&normalise_lf(bytes));
            } else {
                core.term.write(bytes);
            }
            let _ = core.term.take_bell_count();
        }
        if !self.pending.swap(true, Ordering::AcqRel) {
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
    core: Arc<Mutex<Core>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    replies: Arc<Mutex<Vec<u8>>>,
    master: Option<Box<dyn MasterPty + Send>>,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    exit: Arc<Mutex<Option<Option<u32>>>>,
    pending: Arc<AtomicBool>,
    wake: Wake,
    size: (u16, u16),
}

impl std::fmt::Debug for TermSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermSurface").field("size", &self.size).finish()
    }
}

fn new_core(cols: u16, rows: u16, replies: Arc<Mutex<Vec<u8>>>) -> Result<Core> {
    let mut term =
        Terminal::new(cols.max(1), rows.max(1), SCROLLBACK_BYTES).map_err(|e| anyhow::anyhow!("terminal: {e:?}"))?;
    term.set_write_pty_callback(move |b| replies.lock().expect("replies").extend_from_slice(b))
        .map_err(|e| anyhow::anyhow!("callback: {e:?}"))?;
    let mut keys = KeyEncoder::new().map_err(|e| anyhow::anyhow!("key encoder: {e:?}"))?;
    keys.set_from_terminal(&term);
    Ok(Core {
        term,
        rs: RenderState::new().map_err(|e| anyhow::anyhow!("render state: {e:?}"))?,
        keys,
        ri: RowIterator::new().map_err(|e| anyhow::anyhow!("row iterator: {e:?}"))?,
        rc: RowCells::new().map_err(|e| anyhow::anyhow!("row cells: {e:?}"))?,
    })
}

impl TermSurface {
    /// A terminal fed from a byte stream (no child process).
    pub fn stream(cols: u16, rows: u16, wake: Wake) -> Result<Self> {
        let replies = Arc::new(Mutex::new(Vec::new()));
        let core = new_core(cols, rows, replies.clone())?;
        Ok(Self {
            core: Arc::new(Mutex::new(core)),
            writer: None,
            replies,
            master: None,
            killer: None,
            exit: Arc::new(Mutex::new(None)),
            pending: Arc::new(AtomicBool::new(false)),
            wake,
            size: (cols, rows),
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
        let (cols, rows) = (cols.max(2), rows.max(2));
        let mut me = Self::stream(cols, rows, wake.clone())?;
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("opening pty")?;
        let mut cmd = CommandBuilder::new(argv.first().context("empty command")?);
        cmd.args(&argv[1..]);
        cmd.env("TERM", ghostty_vt::TERM);
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
        let writer: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(pair.master.take_writer().context("pty writer")?));
        me.killer = Some(child.clone_killer());
        me.master = Some(pair.master);
        me.writer = Some(writer.clone());

        // Pump: child output → terminal; terminal replies → child.
        {
            let core = me.core.clone();
            let replies = me.replies.clone();
            let pending = me.pending.clone();
            let wake = wake.clone();
            std::thread::Builder::new().name("pty-pump".into()).spawn(move || {
                let mut buf = [0u8; 16 * 1024];
                loop {
                    let n = match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    {
                        let mut c = core.lock().expect("term core");
                        c.term.write(&buf[..n]);
                        let _ = c.term.take_bell_count();
                        let _ = c.term.take_pwd_changes();
                    }
                    let out = std::mem::take(&mut *replies.lock().expect("replies"));
                    if !out.is_empty() {
                        let _ = writer.lock().expect("writer").write_all(&out);
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
            std::thread::Builder::new().name("pty-wait".into()).spawn(move || {
                let code = child.wait().ok().map(|s| s.exit_code());
                *exit.lock().expect("exit") = Some(code);
                on_exit(code);
            })?;
        }
        Ok(me)
    }

    pub fn feeder(&self, lf_to_crlf: bool) -> Feeder {
        Feeder { core: self.core.clone(), pending: self.pending.clone(), wake: self.wake.clone(), lf_to_crlf }
    }

    pub fn is_pty(&self) -> bool {
        self.writer.is_some()
    }

    /// `Some(code)` once the child has exited (`code` is None if unknown).
    pub fn exited(&self) -> Option<Option<u32>> {
        *self.exit.lock().expect("exit")
    }

    pub fn kill(&mut self) {
        if let Some(k) = self.killer.as_mut() {
            let _ = k.kill();
        }
    }

    fn write_child(&self, bytes: &[u8]) {
        if let Some(w) = &self.writer {
            let _ = w.lock().expect("writer").write_all(bytes);
        }
    }

    fn flush_replies(&self) {
        let out = std::mem::take(&mut *self.replies.lock().expect("replies"));
        if !out.is_empty() {
            self.write_child(&out);
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(2), rows.max(2));
        if (cols, rows) == self.size {
            return;
        }
        self.size = (cols, rows);
        {
            let mut c = self.core.lock().expect("term core");
            let _ = c.term.resize(cols, rows, 0, 0);
        }
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
            let mut c = self.core.lock().expect("term core");
            c.term.scroll_viewport_bottom();
            let Core { term, keys: enc, .. } = &mut *c;
            keys::encode(enc, term, k)
        };
        if let Some(b) = bytes {
            self.write_child(&b);
        }
    }

    pub fn paste(&mut self, text: &str) {
        let bracketed =
            self.core.lock().expect("term core").term.mode_get(ghostty_vt::MODE_BRACKETED_PASTE).unwrap_or(false);
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
        self.is_pty() && self.core.lock().expect("term core").term.mouse_tracking_enabled().unwrap_or(false)
    }

    /// Forward a mouse event to a child that requested mouse reporting.
    /// `col`/`row` are relative to the terminal's top-left cell.
    pub fn send_mouse(
        &mut self,
        action: ffi::GhosttyMouseAction,
        button: Option<ffi::GhosttyMouseButton>,
        mods: u16,
        col: u16,
        row: u16,
    ) {
        let bytes = {
            let c = self.core.lock().expect("term core");
            let (Ok(mut enc), Ok(mut ev)) = (MouseEncoder::new(), MouseEvent::new()) else { return };
            enc.set_from_terminal(&c.term);
            enc.set_size(self.size.0 as u32, self.size.1 as u32, 1, 1);
            ev.set_action(action);
            match button {
                Some(b) => ev.set_button(b),
                None => ev.clear_button(),
            }
            ev.set_mods(mods);
            ev.set_position(col as f32, row as f32);
            enc.encode(&ev).unwrap_or_default()
        };
        if !bytes.is_empty() {
            self.write_child(&bytes);
        }
    }

    /// Mouse wheel: to the child when it tracks the mouse, as arrow keys on the
    /// alternate screen, otherwise scroll our own scrollback.
    pub fn wheel(&mut self, lines: i32, col: u16, row: u16) {
        if self.mouse_tracking() {
            let button =
                if lines < 0 { ghostty_vt::MOUSE_BUTTON_WHEEL_UP } else { ghostty_vt::MOUSE_BUTTON_WHEEL_DOWN };
            for _ in 0..lines.unsigned_abs().div_ceil(3) {
                self.send_mouse(ghostty_vt::MOUSE_ACTION_PRESS, Some(button), 0, col, row);
            }
            return;
        }
        let alt = {
            let c = self.core.lock().expect("term core");
            c.term.active_screen().ok() == Some(ActiveScreen::Alternate)
                && c.term.mode_get(ghostty_vt::MODE_MOUSE_ALTERNATE_SCROLL).unwrap_or(false)
        };
        if alt && self.is_pty() {
            let code = if lines < 0 { crossterm::event::KeyCode::Up } else { crossterm::event::KeyCode::Down };
            for _ in 0..lines.unsigned_abs() {
                self.send_key(&CtKey::new(code, crossterm::event::KeyModifiers::NONE));
            }
            return;
        }
        self.core.lock().expect("term core").term.scroll_viewport_delta(lines as isize);
    }

    pub fn scroll_lines(&mut self, lines: i32) {
        self.core.lock().expect("term core").term.scroll_viewport_delta(lines as isize);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.core.lock().expect("term core").term.scroll_viewport_bottom();
    }

    pub fn scroll_to_top(&mut self) {
        let mut c = self.core.lock().expect("term core");
        c.term.scroll_viewport_row(0);
    }

    /// Absolute row of the top visible line (for selections).
    pub fn viewport_top(&self) -> usize {
        self.core.lock().expect("term core").term.scrollbar().map(|s| s.offset).unwrap_or(0)
    }

    /// (total rows, viewport offset, viewport length).
    pub fn scrollbar(&self) -> (usize, usize, usize) {
        self.core.lock().expect("term core").term.scrollbar().map(|s| (s.total, s.offset, s.len)).unwrap_or((0, 0, 0))
    }

    /// Text between screen-absolute (row, col) positions, inclusive.
    pub fn text_between(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let (start, end) = if start <= end { (start, end) } else { (end, start) };
        let c = self.core.lock().expect("term core");
        c.term
            .read_text_screen((start.1 as u16, start.0 as u32), (end.1 as u16, end.0 as u32), false)
            .unwrap_or_default()
    }

    /// Plain text of the visible viewport rows (tests and word selection).
    pub fn viewport_text(&self) -> Vec<String> {
        let c = self.core.lock().expect("term core");
        let rows = c.term.rows().unwrap_or(0);
        let cols = c.term.cols().unwrap_or(0);
        (0..rows as u32)
            .map(|r| c.term.read_text_viewport((0, r), (cols.saturating_sub(1), r), false).unwrap_or_default())
            .collect()
    }

    /// Write a status line into the terminal (e.g. "[process exited]").
    pub fn notice(&mut self, text: &str) {
        let mut c = self.core.lock().expect("term core");
        c.term.write(format!("\r\n\x1b[0;2m{text}\x1b[0m").as_bytes());
    }

    /// Mark output as consumed so the next output wakes the app again.
    pub fn ack(&self) {
        self.pending.store(false, Ordering::Release);
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
            Arc::new(|_| {}),
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
            Arc::new(move |c| *e2.lock().unwrap() = Some(c)),
        )
        .unwrap();
        assert!(wait_for(|| t.exited().is_some()));
        assert_eq!(t.exited(), Some(Some(3)));
        assert_eq!(*exited.lock().unwrap(), Some(Some(3)));
    }

    #[test]
    fn keys_reach_child() {
        let mut t = TermSurface::spawn(&["cat".into()], &[], 30, 5, nop_wake(), Arc::new(|_| {})).unwrap();
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
            Arc::new(|_| {}),
        )
        .unwrap();
        t.resize(50, 7);
        assert!(wait_for(|| t.viewport_text().iter().any(|l| l.contains("7 50"))), "{:?}", t.viewport_text());
    }

    #[test]
    fn logs_lf_normalised() {
        let t = TermSurface::stream(20, 4, nop_wake()).unwrap();
        let f = t.feeder(true);
        f.feed(b"one\ntwo\r\nthree\n");
        assert_eq!(&t.viewport_text()[..3], &["one", "two", "three"]);
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
        t.scroll_lines(-2);
        assert_eq!(t.viewport_top(), offset - 2);
        let top = t.viewport_top();
        assert_eq!(t.text_between((top, 0), (top + 1, 9)), "line6\nline7");
    }
}
