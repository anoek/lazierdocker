//! crossterm key events → libghostty key events → bytes for the child.

use crossterm::event::{KeyCode, KeyEvent as CtKey, KeyEventKind, KeyModifiers};
use libghostty_vt::key::{Action, Encoder, Event, Key, KittyKeyFlags, Mods, OptionAsAlt};
use libghostty_vt::Terminal;

fn mods(m: KeyModifiers) -> Mods {
    let mut out = Mods::empty();
    if m.contains(KeyModifiers::SHIFT) {
        out |= Mods::SHIFT;
    }
    if m.contains(KeyModifiers::CONTROL) {
        out |= Mods::CTRL;
    }
    if m.contains(KeyModifiers::ALT) {
        out |= Mods::ALT;
    }
    if m.contains(KeyModifiers::SUPER) {
        out |= Mods::SUPER;
    }
    out
}

fn key_from_char(c: char) -> Key {
    use Key::*;
    match c.to_ascii_lowercase() {
        'a' => A,
        'b' => B,
        'c' => C,
        'd' => D,
        'e' => E,
        'f' => F,
        'g' => G,
        'h' => H,
        'i' => I,
        'j' => J,
        'k' => K,
        'l' => L,
        'm' => M,
        'n' => N,
        'o' => O,
        'p' => P,
        'q' => Q,
        'r' => R,
        's' => S,
        't' => T,
        'u' => U,
        'v' => V,
        'w' => W,
        'x' => X,
        'y' => Y,
        'z' => Z,
        '0' | ')' => Digit0,
        '1' | '!' => Digit1,
        '2' | '@' => Digit2,
        '3' | '#' => Digit3,
        '4' | '$' => Digit4,
        '5' | '%' => Digit5,
        '6' | '^' => Digit6,
        '7' | '&' => Digit7,
        '8' | '*' => Digit8,
        '9' | '(' => Digit9,
        '`' | '~' => Backquote,
        '\\' | '|' => Backslash,
        '[' | '{' => BracketLeft,
        ']' | '}' => BracketRight,
        ',' | '<' => Comma,
        '=' | '+' => Equal,
        '-' | '_' => Minus,
        '.' | '>' => Period,
        '\'' | '"' => Quote,
        ';' | ':' => Semicolon,
        '/' | '?' => Slash,
        ' ' => Space,
        _ => Unidentified,
    }
}

fn key_from_code(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Enter => Key::Enter,
        KeyCode::Left => Key::ArrowLeft,
        KeyCode::Right => Key::ArrowRight,
        KeyCode::Up => Key::ArrowUp,
        KeyCode::Down => Key::ArrowDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Tab | KeyCode::BackTab => Key::Tab,
        KeyCode::Delete => Key::Delete,
        KeyCode::Insert => Key::Insert,
        KeyCode::Esc => Key::Escape,
        KeyCode::F(n) => match n {
            1 => Key::F1,
            2 => Key::F2,
            3 => Key::F3,
            4 => Key::F4,
            5 => Key::F5,
            6 => Key::F6,
            7 => Key::F7,
            8 => Key::F8,
            9 => Key::F9,
            10 => Key::F10,
            11 => Key::F11,
            12 => Key::F12,
            _ => return None,
        },
        _ => return None,
    })
}

/// Ctrl+Shift+A, which encodes differently once the child turns on
/// xterm's modifyOtherKeys (libghostty-vt has no getter for that state).
fn probe(enc: &mut Encoder) -> Vec<u8> {
    let mut out = Vec::new();
    if let Ok(mut ev) = Event::new() {
        ev.set_action(Action::Press)
            .set_key(Key::A)
            .set_mods(Mods::CTRL | Mods::SHIFT)
            .set_unshifted_codepoint('a')
            .set_utf8(Some("A"));
        let _ = enc.encode_to_vec(&ev, &mut out);
    }
    out
}

/// Whether the child only understands legacy key encoding: no Kitty keyboard
/// protocol and no modifyOtherKeys.
fn is_legacy(enc: &mut Encoder, term: &Terminal<'_, '_>) -> bool {
    if !term.kitty_keyboard_flags().unwrap_or(KittyKeyFlags::DISABLED).is_empty() {
        return false;
    }
    let negotiated = probe(enc);
    enc.set_modify_other_keys_state_2(false);
    let plain = probe(enc);
    enc.set_options_from_terminal(term);
    negotiated == plain
}

/// Encode a crossterm key for the child, using the terminal's negotiated modes.
pub fn encode(enc: &mut Encoder, term: &Terminal<'_, '_>, k: &CtKey) -> Option<Vec<u8>> {
    enc.set_options_from_terminal(term);
    let legacy = is_legacy(enc, term);
    enc.set_macos_option_as_alt(OptionAsAlt::True);
    let mut ev = Event::new().ok()?;
    ev.set_action(match k.kind {
        KeyEventKind::Press => Action::Press,
        KeyEventKind::Repeat => Action::Repeat,
        KeyEventKind::Release => Action::Release,
    });
    let mut m = mods(k.modifiers);
    match k.code {
        KeyCode::Char(c) => {
            let base = if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c };
            if base != c {
                m |= Mods::SHIFT;
            }
            // Legacy apps: Ctrl+Shift+letter is plain Ctrl+letter.
            if legacy && m.contains(Mods::CTRL) && base.is_ascii_alphabetic() {
                m.remove(Mods::SHIFT);
            }
            ev.set_key(key_from_char(base));
            ev.set_unshifted_codepoint(base);
            // The text goes along even with Ctrl held: Ghostty derives the
            // control byte itself, and modifyOtherKeys needs the character.
            ev.set_utf8(Some(c.to_string()));
            let plain = !m.intersects(Mods::CTRL | Mods::ALT | Mods::SUPER);
            let alt_only = m.contains(Mods::ALT) && !m.contains(Mods::CTRL);
            if (plain || alt_only) && m.contains(Mods::SHIFT) {
                ev.set_consumed_mods(Mods::SHIFT);
            }
        }
        KeyCode::BackTab => {
            m |= Mods::SHIFT;
            ev.set_key(Key::Tab);
        }
        KeyCode::Enter if legacy => {
            // Modified Enter means nothing to plain shells; send a plain CR.
            m = Mods::empty();
            ev.set_key(Key::Enter);
        }
        other => {
            ev.set_key(key_from_code(other)?);
        }
    }
    ev.set_mods(m);
    let mut out = Vec::new();
    enc.encode_to_vec(&ev, &mut out).ok()?;
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libghostty_vt::TerminalOptions;

    fn term() -> Terminal<'static, 'static> {
        Terminal::new(TerminalOptions { cols: 80, rows: 24, max_scrollback: 1 << 16 }).unwrap()
    }

    fn enc_in(term: &Terminal<'_, '_>, code: KeyCode, mods: KeyModifiers) -> String {
        let mut e = Encoder::new().unwrap();
        let bytes = encode(&mut e, term, &CtKey::new(code, mods)).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn enc(code: KeyCode, mods: KeyModifiers) -> String {
        enc_in(&term(), code, mods)
    }

    #[test]
    fn key_encoding_basic() {
        assert_eq!(enc(KeyCode::Char('a'), KeyModifiers::NONE), "a");
        assert_eq!(enc(KeyCode::Char('A'), KeyModifiers::SHIFT), "A");
        assert_eq!(enc(KeyCode::Enter, KeyModifiers::NONE), "\r");
        assert_eq!(enc(KeyCode::Char('c'), KeyModifiers::CONTROL), "\x03");
        assert_eq!(enc(KeyCode::Up, KeyModifiers::NONE), "\x1b[A");
        assert_eq!(enc(KeyCode::BackTab, KeyModifiers::SHIFT), "\x1b[Z");
        assert_eq!(enc(KeyCode::Char('x'), KeyModifiers::ALT), "\x1bx");
        assert_eq!(enc(KeyCode::Backspace, KeyModifiers::NONE), "\x7f");
        assert_eq!(enc(KeyCode::Char('é'), KeyModifiers::NONE), "é");
        assert_eq!(enc(KeyCode::Esc, KeyModifiers::NONE), "\x1b");
    }

    #[test]
    fn legacy_ctrl_shift_letter_is_ctrl_letter() {
        assert_eq!(enc(KeyCode::Char('C'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), "\x03");
        assert_eq!(enc(KeyCode::Enter, KeyModifiers::SHIFT), "\r");
    }

    #[test]
    fn modify_other_keys_keeps_shift() {
        let mut t = term();
        t.vt_write(b"\x1b[>4;2m");
        assert_eq!(enc_in(&t, KeyCode::Char('C'), KeyModifiers::CONTROL | KeyModifiers::SHIFT), "\x1b[27;6;67~");
    }

    #[test]
    fn app_cursor_mode_changes_arrows() {
        let mut t = term();
        t.vt_write(b"\x1b[?1h");
        assert_eq!(enc_in(&t, KeyCode::Up, KeyModifiers::NONE), "\x1bOA");
    }
}
