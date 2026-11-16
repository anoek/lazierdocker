//! System clipboard: native tools first, OSC 52 as the fallback (herdr's
//! approach). OSC 52 is preferred over SSH, where native tools would copy on
//! the remote machine.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::Engine;

fn over_ssh() -> bool {
    std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some()
}

fn native_candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        return vec![("pbcopy", vec![])];
    }
    let mut v = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        v.push(("wl-copy", vec![]));
    }
    if std::env::var_os("DISPLAY").is_some() {
        v.push(("xclip", vec!["-selection", "clipboard", "-in"]));
        v.push(("xsel", vec!["--clipboard", "--input"]));
    }
    v
}

fn native_copy(text: &str) -> bool {
    for (prog, args) in native_candidates() {
        let Ok(mut child) =
            Command::new(prog).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        // xclip forks and keeps serving the selection; only wait briefly.
        let start = std::time::Instant::now();
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return true,
                Ok(Some(_)) => break,
                Ok(None) if start.elapsed() < Duration::from_millis(500) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Ok(None) => {
                    // Still serving the selection: reap it whenever it exits.
                    std::thread::spawn(move || child.wait());
                    return true;
                }
                Err(_) => break,
            }
        }
    }
    false
}

/// The OSC 52 sequence that sets the clipboard to `text`.
fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(text.as_bytes()))
}

/// Copy to the system clipboard. Returns a short description of how.
pub fn copy(text: &str) -> &'static str {
    if !over_ssh() && native_copy(text) {
        return "clipboard";
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(osc52(text).as_bytes());
    let _ = out.flush();
    "OSC 52"
}

#[cfg(test)]
mod tests {
    #[test]
    fn osc52_encodes_base64() {
        assert_eq!(super::osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}
