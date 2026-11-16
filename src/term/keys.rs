//! crossterm key events → libghostty key events → bytes for the child.

use crossterm::event::{KeyCode, KeyEvent as CtKey, KeyEventKind, KeyModifiers};
use ghostty_vt::{ffi, KeyEncoder, KeyEvent, Terminal, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_SUPER};

fn mods(m: KeyModifiers) -> u16 {
    let mut out = 0;
    if m.contains(KeyModifiers::SHIFT) {
        out |= MOD_SHIFT;
    }
    if m.contains(KeyModifiers::CONTROL) {
        out |= MOD_CTRL;
    }
    if m.contains(KeyModifiers::ALT) {
        out |= MOD_ALT;
    }
    if m.contains(KeyModifiers::SUPER) {
        out |= MOD_SUPER;
    }
    out
}

fn key_from_char(c: char) -> ffi::GhosttyKey {
    use ffi::*;
    match c.to_ascii_lowercase() {
        'a' => GhosttyKey_GHOSTTY_KEY_A,
        'b' => GhosttyKey_GHOSTTY_KEY_B,
        'c' => GhosttyKey_GHOSTTY_KEY_C,
        'd' => GhosttyKey_GHOSTTY_KEY_D,
        'e' => GhosttyKey_GHOSTTY_KEY_E,
        'f' => GhosttyKey_GHOSTTY_KEY_F,
        'g' => GhosttyKey_GHOSTTY_KEY_G,
        'h' => GhosttyKey_GHOSTTY_KEY_H,
        'i' => GhosttyKey_GHOSTTY_KEY_I,
        'j' => GhosttyKey_GHOSTTY_KEY_J,
        'k' => GhosttyKey_GHOSTTY_KEY_K,
        'l' => GhosttyKey_GHOSTTY_KEY_L,
        'm' => GhosttyKey_GHOSTTY_KEY_M,
        'n' => GhosttyKey_GHOSTTY_KEY_N,
        'o' => GhosttyKey_GHOSTTY_KEY_O,
        'p' => GhosttyKey_GHOSTTY_KEY_P,
        'q' => GhosttyKey_GHOSTTY_KEY_Q,
        'r' => GhosttyKey_GHOSTTY_KEY_R,
        's' => GhosttyKey_GHOSTTY_KEY_S,
        't' => GhosttyKey_GHOSTTY_KEY_T,
        'u' => GhosttyKey_GHOSTTY_KEY_U,
        'v' => GhosttyKey_GHOSTTY_KEY_V,
        'w' => GhosttyKey_GHOSTTY_KEY_W,
        'x' => GhosttyKey_GHOSTTY_KEY_X,
        'y' => GhosttyKey_GHOSTTY_KEY_Y,
        'z' => GhosttyKey_GHOSTTY_KEY_Z,
        '0' | ')' => GhosttyKey_GHOSTTY_KEY_DIGIT_0,
        '1' | '!' => GhosttyKey_GHOSTTY_KEY_DIGIT_1,
        '2' | '@' => GhosttyKey_GHOSTTY_KEY_DIGIT_2,
        '3' | '#' => GhosttyKey_GHOSTTY_KEY_DIGIT_3,
        '4' | '$' => GhosttyKey_GHOSTTY_KEY_DIGIT_4,
        '5' | '%' => GhosttyKey_GHOSTTY_KEY_DIGIT_5,
        '6' | '^' => GhosttyKey_GHOSTTY_KEY_DIGIT_6,
        '7' | '&' => GhosttyKey_GHOSTTY_KEY_DIGIT_7,
        '8' | '*' => GhosttyKey_GHOSTTY_KEY_DIGIT_8,
        '9' | '(' => GhosttyKey_GHOSTTY_KEY_DIGIT_9,
        '`' | '~' => GhosttyKey_GHOSTTY_KEY_BACKQUOTE,
        '\\' | '|' => GhosttyKey_GHOSTTY_KEY_BACKSLASH,
        '[' | '{' => GhosttyKey_GHOSTTY_KEY_BRACKET_LEFT,
        ']' | '}' => GhosttyKey_GHOSTTY_KEY_BRACKET_RIGHT,
        ',' | '<' => GhosttyKey_GHOSTTY_KEY_COMMA,
        '=' | '+' => GhosttyKey_GHOSTTY_KEY_EQUAL,
        '-' | '_' => GhosttyKey_GHOSTTY_KEY_MINUS,
        '.' | '>' => GhosttyKey_GHOSTTY_KEY_PERIOD,
        '\'' | '"' => GhosttyKey_GHOSTTY_KEY_QUOTE,
        ';' | ':' => GhosttyKey_GHOSTTY_KEY_SEMICOLON,
        '/' | '?' => GhosttyKey_GHOSTTY_KEY_SLASH,
        ' ' => GhosttyKey_GHOSTTY_KEY_SPACE,
        _ => GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED,
    }
}

fn key_from_code(code: KeyCode) -> Option<ffi::GhosttyKey> {
    use ffi::*;
    Some(match code {
        KeyCode::Backspace => GhosttyKey_GHOSTTY_KEY_BACKSPACE,
        KeyCode::Enter => GhosttyKey_GHOSTTY_KEY_ENTER,
        KeyCode::Left => GhosttyKey_GHOSTTY_KEY_ARROW_LEFT,
        KeyCode::Right => GhosttyKey_GHOSTTY_KEY_ARROW_RIGHT,
        KeyCode::Up => GhosttyKey_GHOSTTY_KEY_ARROW_UP,
        KeyCode::Down => GhosttyKey_GHOSTTY_KEY_ARROW_DOWN,
        KeyCode::Home => GhosttyKey_GHOSTTY_KEY_HOME,
        KeyCode::End => GhosttyKey_GHOSTTY_KEY_END,
        KeyCode::PageUp => GhosttyKey_GHOSTTY_KEY_PAGE_UP,
        KeyCode::PageDown => GhosttyKey_GHOSTTY_KEY_PAGE_DOWN,
        KeyCode::Tab | KeyCode::BackTab => GhosttyKey_GHOSTTY_KEY_TAB,
        KeyCode::Delete => GhosttyKey_GHOSTTY_KEY_DELETE,
        KeyCode::Insert => GhosttyKey_GHOSTTY_KEY_INSERT,
        KeyCode::Esc => GhosttyKey_GHOSTTY_KEY_ESCAPE,
        KeyCode::F(n) => match n {
            1 => GhosttyKey_GHOSTTY_KEY_F1,
            2 => GhosttyKey_GHOSTTY_KEY_F2,
            3 => GhosttyKey_GHOSTTY_KEY_F3,
            4 => GhosttyKey_GHOSTTY_KEY_F4,
            5 => GhosttyKey_GHOSTTY_KEY_F5,
            6 => GhosttyKey_GHOSTTY_KEY_F6,
            7 => GhosttyKey_GHOSTTY_KEY_F7,
            8 => GhosttyKey_GHOSTTY_KEY_F8,
            9 => GhosttyKey_GHOSTTY_KEY_F9,
            10 => GhosttyKey_GHOSTTY_KEY_F10,
            11 => GhosttyKey_GHOSTTY_KEY_F11,
            12 => GhosttyKey_GHOSTTY_KEY_F12,
            _ => return None,
        },
        _ => return None,
    })
}

/// Encode a crossterm key for the child, using the terminal's negotiated modes.
pub fn encode(enc: &mut KeyEncoder, term: &Terminal, k: &CtKey) -> Option<Vec<u8>> {
    enc.set_from_terminal(term);
    enc.set_macos_option_as_alt(true);
    let mut ev = KeyEvent::new().ok()?;
    ev.set_action(match k.kind {
        KeyEventKind::Press => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS,
        KeyEventKind::Repeat => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_REPEAT,
        KeyEventKind::Release => ffi::GhosttyKeyAction_GHOSTTY_KEY_ACTION_RELEASE,
    });
    let mut m = mods(k.modifiers);
    let legacy = term.kitty_keyboard_flags().unwrap_or(0) == 0 && !term.modify_other_keys_enabled().unwrap_or(false);
    match k.code {
        KeyCode::Char(c) => {
            let base = if c.is_ascii_uppercase() { c.to_ascii_lowercase() } else { c };
            if base != c {
                m |= MOD_SHIFT;
            }
            // Legacy apps: Ctrl+Shift+letter is plain Ctrl+letter.
            if legacy && m & MOD_CTRL != 0 && base.is_ascii_alphabetic() {
                m &= !MOD_SHIFT;
            }
            ev.set_key(key_from_char(base));
            ev.set_unshifted_codepoint(base as u32);
            if m & (MOD_CTRL | MOD_ALT | MOD_SUPER) == 0 || m & MOD_ALT != 0 && m & MOD_CTRL == 0 {
                ev.set_utf8(&c.to_string());
                if m & MOD_SHIFT != 0 {
                    ev.set_consumed_mods(MOD_SHIFT);
                }
            }
        }
        KeyCode::BackTab => {
            m |= MOD_SHIFT;
            ev.set_key(ffi::GhosttyKey_GHOSTTY_KEY_TAB);
        }
        KeyCode::Enter if legacy => {
            // Modified Enter means nothing to plain shells; send a plain CR.
            m = 0;
            ev.set_key(ffi::GhosttyKey_GHOSTTY_KEY_ENTER);
        }
        other => ev.set_key(key_from_code(other)?),
    }
    ev.set_mods(m);
    enc.encode(&ev).ok().filter(|b| !b.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc(code: KeyCode, mods: KeyModifiers) -> String {
        let term = Terminal::new(80, 24, 1 << 16).unwrap();
        let mut e = KeyEncoder::new().unwrap();
        let bytes = encode(&mut e, &term, &CtKey::new(code, mods)).unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
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
    fn app_cursor_mode_changes_arrows() {
        let mut term = Terminal::new(80, 24, 1 << 16).unwrap();
        term.write(b"\x1b[?1h");
        let mut e = KeyEncoder::new().unwrap();
        let bytes = encode(&mut e, &term, &CtKey::new(KeyCode::Up, KeyModifiers::NONE)).unwrap();
        assert_eq!(bytes, b"\x1bOA");
    }
}
