//! Smoke test of the libghostty-vt binding on its own, so a build or link
//! problem with the native library shows up here rather than deep in app tests.

use libghostty_vt::terminal::{Point, PointCoordinate};
use libghostty_vt::{Terminal, TerminalOptions};

#[test]
fn terminal_roundtrip() {
    let mut term = Terminal::new(TerminalOptions { cols: 20, rows: 5, max_scrollback: 1 << 20 }).expect("terminal");
    term.vt_write(b"hi\r\nthere");
    let at = |x, y| term.grid_ref(Point::Screen(PointCoordinate { x, y })).expect("grid ref");
    let mut grapheme = ['\0'; 1];
    at(0, 1).graphemes(&mut grapheme).expect("graphemes");
    assert_eq!(grapheme[0], 't');
    assert_eq!(term.cursor_x().unwrap(), 5);
}
