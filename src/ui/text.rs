//! Plain-text helpers by display width: truncation, word wrap, and mapping
//! between display columns and char indices (wide chars take two columns).

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Truncate to `max` display columns, adding `…` when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for ch in s.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > max - 1 {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push('…');
    out
}

/// Greedy word wrap.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.lines() {
        let mut line = String::new();
        for word in para.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

/// Index of the char covering display column `col` (`chars.len()` past the end).
pub fn col_to_char(chars: &[char], col: usize) -> usize {
    let mut end = 0;
    for (i, c) in chars.iter().enumerate() {
        end += c.width().unwrap_or(0);
        if col < end {
            return i;
        }
    }
    chars.len()
}

/// Display column where char `i` starts.
pub fn char_to_col(chars: &[char], i: usize) -> usize {
    chars[..i.min(chars.len())].iter().map(|c| c.width().unwrap_or(0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_chars() {
        let chars: Vec<char> = "a日b".chars().collect();
        // Columns: a=0, 日=1..2, b=3.
        assert_eq!([0, 1, 2, 3, 4].map(|c| col_to_char(&chars, c)), [0, 1, 1, 2, 3]);
        assert_eq!([0, 1, 2, 3].map(|i| char_to_col(&chars, i)), [0, 1, 3, 4]);
    }

    #[test]
    fn truncate_wide_chars() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 4), "hel…");
        assert_eq!(truncate("日本語", 4), "日…");
    }

    #[test]
    fn wrap_words() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("one\n\ntwo", 10), vec!["one", "", "two"]);
    }
}
