//! Container logs with Docker's timestamps: history from several containers
//! merged in time order, then each container followed from where its
//! history ended, without gaps or repeats.

use anyhow::Result;
use bollard::container::LogOutput;
use bollard::query_parameters::LogsOptions;
use futures_util::StreamExt;

use super::client::Docker;

/// When a log line was written (UTC), from Docker's RFC 3339 timestamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LogTime {
    secs: i64,
    nanos: u32,
}

impl LogTime {
    /// Parse `2026-11-16T00:23:15.283456789Z` (any number of fraction
    /// digits; `Z` or a `±hh:mm` offset).
    pub fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        let num = |r: std::ops::Range<usize>| -> Option<i64> { s.get(r)?.parse().ok() };
        if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
            return None;
        }
        let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
        let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
        let mut i = 19;
        let mut nanos = 0u32;
        if b[i] == b'.' {
            let digits = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            let frac = &s[i + 1..i + 1 + digits];
            // Scale to nanoseconds: pad or cut to 9 digits.
            let padded = format!("{:0<9}", &frac[..digits.min(9)]);
            nanos = padded.parse().ok()?;
            i += 1 + digits;
        }
        let offset = match s.get(i..)? {
            "Z" => 0,
            tz if tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')) => {
                let mins = num(i + 1..i + 3)? * 60 + num(i + 4..i + 6)?;
                if tz.starts_with('+') {
                    mins * 60
                } else {
                    -mins * 60
                }
            }
            _ => return None,
        };
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
            return None;
        }
        let days = days_from_civil(y, mo as u32, d as u32);
        Some(LogTime { secs: days * 86_400 + h * 3600 + mi * 60 + sec - offset, nanos })
    }

    /// Whole seconds since the Unix epoch (what the logs API's `since` takes).
    pub fn unix_secs(self) -> i64 {
        self.secs
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// One log line: when it was written (None if it carried no timestamp) and
/// its bytes, line end included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub time: Option<LogTime>,
    pub text: Vec<u8>,
}

impl LogLine {
    /// Split Docker's `<timestamp> ` prefix off a raw line.
    fn from_raw(raw: Vec<u8>) -> Self {
        let space = raw.iter().position(|&b| b == b' ');
        let time = space.and_then(|i| std::str::from_utf8(&raw[..i]).ok()).and_then(LogTime::parse);
        match (time, space) {
            (Some(time), Some(i)) => LogLine { time: Some(time), text: raw[i + 1..].to_vec() },
            _ => LogLine { time: None, text: raw },
        }
    }
}

/// Cuts a log byte stream into whole lines (chunks don't follow line ends).
#[derive(Debug, Default)]
pub struct LineSplitter {
    partial: Vec<u8>,
}

impl LineSplitter {
    /// The lines `bytes` completes.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<LogLine> {
        let mut out = Vec::new();
        for &b in bytes {
            self.partial.push(b);
            if b == b'\n' {
                out.push(LogLine::from_raw(std::mem::take(&mut self.partial)));
            }
        }
        out
    }

    /// A last line that never got its line end.
    pub fn finish(&mut self) -> Option<LogLine> {
        (!self.partial.is_empty()).then(|| LogLine::from_raw(std::mem::take(&mut self.partial)))
    }
}

/// Merge several containers' histories into one, oldest first, keeping the
/// newest `keep` lines (all when None). Returns (source index, line). A line
/// without a time stays right after the line before it in its source.
pub fn merge(sources: Vec<Vec<LogLine>>, keep: Option<usize>) -> Vec<(usize, LogLine)> {
    let mut all: Vec<(Option<LogTime>, usize, usize, LogLine)> = Vec::new();
    for (src, lines) in sources.into_iter().enumerate() {
        let mut last = None;
        for (seq, line) in lines.into_iter().enumerate() {
            last = line.time.or(last);
            all.push((last, src, seq, line));
        }
    }
    all.sort_by_key(|a| (a.0, a.1, a.2));
    let skip = keep.map_or(0, |k| all.len().saturating_sub(k));
    all.into_iter().skip(skip).map(|(_, src, _, line)| (src, line)).collect()
}

/// A followed line not yet shown: written after `after`. A line without a
/// time always counts.
pub fn is_new(line: &LogLine, after: Option<LogTime>) -> bool {
    match (line.time, after) {
        (Some(t), Some(a)) => t > a,
        _ => true,
    }
}

/// The newest time among `lines`.
pub fn last_time(lines: &[LogLine]) -> Option<LogTime> {
    lines.iter().filter_map(|l| l.time).max()
}

/// Whether a container runs with a TTY (its output then already ends lines with `\r\n`).
pub async fn has_tty(docker: &Docker, id: &str) -> bool {
    docker.api.inspect_container(id, None).await.ok().and_then(|i| i.config?.tty).unwrap_or(false)
}

/// The output bytes of a log stream item.
fn output(item: &LogOutput) -> Option<&[u8]> {
    match item {
        LogOutput::StdOut { message } | LogOutput::StdErr { message } | LogOutput::Console { message } => {
            Some(&message[..])
        }
        LogOutput::StdIn { .. } => None,
    }
}

/// A container's log so far: the last `tail` lines ("all" for everything),
/// stdout and stderr.
pub async fn history(docker: &Docker, id: &str, tail: &str) -> Result<Vec<LogLine>> {
    let opts = LogsOptions {
        follow: false,
        stdout: true,
        stderr: true,
        timestamps: true,
        tail: tail.into(),
        ..Default::default()
    };
    let mut stream = docker.api.logs(id, Some(opts));
    let mut split = LineSplitter::default();
    let mut lines = Vec::new();
    while let Some(item) = stream.next().await {
        if let Some(b) = output(&item?) {
            lines.extend(split.push(b));
        }
    }
    lines.extend(split.finish());
    Ok(lines)
}

/// Follow a container's log, calling `line` for each line written after
/// `after` (the whole log when None), until the stream ends. Returns how it
/// ended (an error message if it failed) and the time of the last line seen.
pub async fn follow(
    docker: &Docker,
    id: &str,
    after: Option<LogTime>,
    mut line: impl FnMut(&LogLine),
) -> (Result<(), String>, Option<LogTime>) {
    // `since` has whole-second precision: ask from that second and drop what
    // was already written.
    let since = after.map_or(0, |t| t.unix_secs().clamp(0, i32::MAX as i64) as i32);
    let opts = LogsOptions {
        follow: true,
        stdout: true,
        stderr: true,
        timestamps: true,
        since,
        tail: "all".into(),
        ..Default::default()
    };
    let mut stream = docker.api.logs(id, Some(opts));
    let mut split = LineSplitter::default();
    let mut last = after;
    let mut emit = |l: LogLine, last: &mut Option<LogTime>| {
        if !is_new(&l, after) {
            return;
        }
        *last = l.time.or(*last);
        line(&l);
    };
    let ended = loop {
        match stream.next().await {
            Some(Ok(item)) => {
                for l in output(&item).map(|b| split.push(b)).unwrap_or_default() {
                    emit(l, &mut last);
                }
            }
            Some(Err(e)) => break Err(e.to_string()),
            None => break Ok(()),
        }
    };
    if let Some(l) = split.finish() {
        emit(l, &mut last);
    }
    (ended, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> LogTime {
        LogTime::parse(s).unwrap()
    }

    #[test]
    fn parses_docker_timestamps() {
        assert_eq!(t("1970-01-01T00:00:00Z").unix_secs(), 0);
        assert_eq!(t("2026-11-16T00:23:15.283456789Z").unix_secs(), 1_794_788_595);
        // Fraction digits vary; order must still be by time, not by text.
        assert!(t("2026-11-16T00:23:15.28Z") < t("2026-11-16T00:23:15.283Z"));
        assert_eq!(t("2026-11-16T00:23:15.5Z"), t("2026-11-16T00:23:15.500000000Z"));
        assert_eq!(t("2026-11-16T01:23:15+01:00"), t("2026-11-16T00:23:15Z"));
        assert_eq!(t("2024-03-01T00:00:00Z").unix_secs() - t("2024-02-28T00:00:00Z").unix_secs(), 2 * 86_400);
        assert!(LogTime::parse("not a time").is_none());
        assert!(LogTime::parse("2026-13-01T00:00:00Z").is_none());
    }

    #[test]
    fn splitter_joins_chunks_into_lines_and_strips_timestamps() {
        let mut s = LineSplitter::default();
        assert!(s.push(b"2026-11-16T00:00:01Z hel").is_empty());
        let lines = s.push(b"lo\n2026-11-16T00:00:02Z world\nraw line\n2026-11-16T00:00:03Z tail");
        assert_eq!(lines.len(), 3);
        assert_eq!((lines[0].time, lines[0].text.as_slice()), (Some(t("2026-11-16T00:00:01Z")), &b"hello\n"[..]));
        assert_eq!(lines[2], LogLine { time: None, text: b"raw line\n".to_vec() });
        let last = s.finish().unwrap();
        assert_eq!(last.text, b"tail");
        assert!(s.finish().is_none());
    }

    fn line(time: &str, text: &str) -> LogLine {
        LogLine { time: LogTime::parse(time), text: text.as_bytes().to_vec() }
    }

    #[test]
    fn merge_orders_by_time_and_keeps_the_newest() {
        let a = vec![line("2026-01-01T00:00:01Z", "a1"), line("x", "a1 cont"), line("2026-01-01T00:00:04Z", "a4")];
        let b = vec![line("2026-01-01T00:00:02Z", "b2"), line("2026-01-01T00:00:03Z", "b3")];
        let text = |m: Vec<(usize, LogLine)>| -> Vec<String> {
            m.into_iter().map(|(src, l)| format!("{src}:{}", String::from_utf8(l.text).unwrap())).collect()
        };
        assert_eq!(text(merge(vec![a.clone(), b.clone()], None)), ["0:a1", "0:a1 cont", "1:b2", "1:b3", "0:a4"]);
        assert_eq!(text(merge(vec![a, b], Some(2))), ["1:b3", "0:a4"], "the newest overall");
        assert_eq!(last_time(&[line("2026-01-01T00:00:02Z", ""), line("x", "")]), Some(t("2026-01-01T00:00:02Z")));
    }

    #[test]
    fn following_skips_what_history_showed() {
        let after = Some(t("2026-01-01T00:00:02.5Z"));
        assert!(!is_new(&line("2026-01-01T00:00:02Z", ""), after), "same second, earlier");
        assert!(!is_new(&line("2026-01-01T00:00:02.5Z", ""), after), "the last line shown");
        assert!(is_new(&line("2026-01-01T00:00:02.500000001Z", ""), after));
        assert!(is_new(&line("no time", ""), after));
        assert!(is_new(&line("2026-01-01T00:00:01Z", ""), None), "nothing shown yet");
    }
}
