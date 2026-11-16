//! Text search over a terminal's rows (scrollback included).

use unicode_width::UnicodeWidthChar;

/// One match: inclusive screen-absolute (row, col) range, as selections use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

/// Find every non-overlapping occurrence of `query` in `rows`, top to bottom.
///
/// `rows` holds each row's text (trailing blanks trimmed) and whether it
/// soft-wraps into the next row; wrapped rows are searched as one line, so a
/// match may span rows. Smart case: case-insensitive unless `query` has an
/// uppercase letter.
pub fn find_in_rows(rows: &[(String, bool)], cols: usize, query: &str) -> Vec<Match> {
    let fold_case = !query.chars().any(char::is_uppercase);
    let fold = |c: char| if fold_case { c.to_lowercase().next().unwrap_or(c) } else { c };
    let needle: Vec<char> = query.chars().map(fold).collect();
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    // The current logical line: folded chars and the (row, col, width) of each.
    let mut chars = Vec::new();
    let mut cells: Vec<(usize, usize, usize)> = Vec::new();
    for (row, (text, wrapped)) in rows.iter().enumerate() {
        let mut col = 0;
        for c in text.chars() {
            // Zero-width marks (combining accents) share the previous cell.
            let w = c.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            chars.push(fold(c));
            cells.push((row, col, w));
            col += w;
        }
        if *wrapped {
            // Trimmed blanks at the wrap point are real spaces in the line.
            while col < cols {
                chars.push(' ');
                cells.push((row, col, 1));
                col += 1;
            }
            continue;
        }
        scan(&chars, &cells, &needle, &mut out);
        chars.clear();
        cells.clear();
    }
    scan(&chars, &cells, &needle, &mut out);
    out
}

fn scan(chars: &[char], cells: &[(usize, usize, usize)], needle: &[char], out: &mut Vec<Match>) {
    let mut i = 0;
    while i + needle.len() <= chars.len() {
        if chars[i..i + needle.len()] == *needle {
            let (r0, c0, _) = cells[i];
            let (r1, c1, w1) = cells[i + needle.len() - 1];
            out.push(Match { start: (r0, c0), end: (r1, c1 + w1 - 1) });
            i += needle.len();
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[(&str, bool)]) -> Vec<(String, bool)> {
        lines.iter().map(|(t, w)| (t.to_string(), *w)).collect()
    }

    #[test]
    fn finds_all_matches_top_to_bottom() {
        let r = rows(&[("error one", false), ("ok", false), ("an error, another error", false)]);
        let m = find_in_rows(&r, 40, "error");
        assert_eq!(
            m,
            vec![
                Match { start: (0, 0), end: (0, 4) },
                Match { start: (2, 3), end: (2, 7) },
                Match { start: (2, 18), end: (2, 22) },
            ]
        );
    }

    #[test]
    fn smart_case() {
        let r = rows(&[("Error", false), ("error", false)]);
        assert_eq!(find_in_rows(&r, 10, "error").len(), 2);
        assert_eq!(find_in_rows(&r, 10, "Error"), vec![Match { start: (0, 0), end: (0, 4) }]);
    }

    #[test]
    fn matches_span_soft_wraps_but_not_hard_lines() {
        // "hello wo" wraps at 8 columns into "rld".
        let r = rows(&[("hello wo", true), ("rld", false), ("wo", false), ("rld", false)]);
        assert_eq!(find_in_rows(&r, 8, "world"), vec![Match { start: (0, 6), end: (1, 2) }]);
        // A space trimmed at the wrap point still separates words.
        let r = rows(&[("abcdefg", true), ("hij", false)]);
        assert_eq!(find_in_rows(&r, 8, "g h"), vec![Match { start: (0, 6), end: (1, 0) }]);
    }

    #[test]
    fn wide_chars_take_two_columns() {
        let r = rows(&[("日本 error", false)]);
        assert_eq!(find_in_rows(&r, 20, "error"), vec![Match { start: (0, 5), end: (0, 9) }]);
        assert_eq!(find_in_rows(&r, 20, "本"), vec![Match { start: (0, 2), end: (0, 3) }]);
    }

    #[test]
    fn empty_query_and_overlaps() {
        let r = rows(&[("aaaa", false)]);
        assert!(find_in_rows(&r, 10, "").is_empty());
        assert_eq!(find_in_rows(&r, 10, "aa").len(), 2);
    }
}
