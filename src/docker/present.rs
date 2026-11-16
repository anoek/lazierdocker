//! Row presentation (lazydocker's list columns and colours).

use ratatui::style::{Color, Style};
use ratatui::text::Span;

use super::model::{Container, ContainerState, Health, Port};
use crate::ui::theme::Theme;

/// lazydocker "icon" status style.
pub fn state_icon(state: ContainerState) -> &'static str {
    match state {
        ContainerState::Paused => "◫",
        ContainerState::Exited => "⨯",
        ContainerState::Created => "+",
        ContainerState::Removing => "−",
        ContainerState::Restarting => "⟳",
        ContainerState::Running => "▶",
        ContainerState::Dead => "!",
        ContainerState::Stopping => "■",
        ContainerState::Unknown => "?",
    }
}

pub fn state_color(c: &Container, theme: &Theme) -> Color {
    match c.state {
        ContainerState::Exited => {
            if c.exit_code.unwrap_or(0) == 0 {
                theme.yellow
            } else {
                theme.red
            }
        }
        ContainerState::Created => theme.cyan,
        ContainerState::Running => theme.green,
        ContainerState::Paused | ContainerState::Stopping => theme.yellow,
        ContainerState::Dead => theme.red,
        ContainerState::Restarting => theme.blue,
        ContainerState::Removing => theme.magenta,
        ContainerState::Unknown => theme.overlay0,
    }
}

pub fn health_span(h: Health, theme: &Theme) -> Span<'static> {
    match h {
        Health::Healthy => Span::styled("✔", Style::default().fg(theme.green)),
        Health::Unhealthy => Span::styled("?", Style::default().fg(theme.red)),
        Health::Starting => Span::styled("…", Style::default().fg(theme.yellow)),
    }
}

/// Substatus: `(exitCode)` for exited containers, health glyph for running ones.
pub fn substatus(c: &Container, theme: &Theme) -> Option<Span<'static>> {
    match c.state {
        ContainerState::Exited => {
            c.exit_code.map(|code| Span::styled(format!("({code})"), Style::default().fg(state_color(c, theme))))
        }
        _ => c.health.map(|h| health_span(h, theme)),
    }
}

pub fn cpu_span(cpu: f64, theme: &Theme) -> Span<'static> {
    let color = if cpu > 90.0 {
        theme.red
    } else if cpu > 50.0 {
        theme.yellow
    } else {
        theme.subtext0
    };
    Span::styled(format!("{cpu:.2}%"), Style::default().fg(color))
}

/// `[ip:]public->private/proto` (ip omitted when 0.0.0.0/::) or `private/proto`; sorted, joined ", ".
pub fn ports(ps: &[Port]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for p in ps {
        let s = match p.public {
            Some(public) => {
                let ip = p.ip.as_deref().filter(|ip| *ip != "0.0.0.0" && *ip != "::");
                match ip {
                    Some(ip) => format!("{ip}:{public}->{}/{}", p.private, p.proto),
                    None => format!("{public}->{}/{}", p.private, p.proto),
                }
            }
            None => format!("{}/{}", p.private, p.proto),
        };
        if !parts.contains(&s) {
            parts.push(s);
        }
    }
    parts.join(", ")
}

/// First published port as a browser URL (lazydocker `w`): 0.0.0.0 → localhost.
pub fn browser_url(ps: &[Port]) -> Option<String> {
    let p = ps.iter().find(|p| p.public.is_some())?;
    let ip = match p.ip.as_deref() {
        None | Some("0.0.0.0") | Some("::") | Some("") => "localhost",
        Some(ip) => ip,
    };
    Some(format!("http://{ip}:{}/", p.public?))
}

/// Human-readable size in decimal units (lazydocker list columns).
pub fn size(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["B", "kB", "MB", "GB", "TB", "PB"];
    let mut v = bytes.max(0) as f64;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0}{}", UNITS[i])
    } else {
        format!("{v:.1}{}", UNITS[i])
    }
}

/// Binary units (image history).
pub fn size_binary(bytes: i64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = bytes.max(0) as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{v:.0}{}", UNITS[i])
    } else {
        format!("{v:.1}{}", UNITS[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::model::tests::container;

    fn port(ip: Option<&str>, private: u16, public: Option<u16>) -> Port {
        Port { ip: ip.map(Into::into), private, public, proto: "tcp".into() }
    }

    #[test]
    fn ports_format() {
        let ps = vec![
            port(Some("0.0.0.0"), 80, Some(8080)),
            port(Some("::"), 80, Some(8080)),
            port(Some("127.0.0.1"), 5432, Some(5432)),
            port(None, 6379, None),
        ];
        assert_eq!(ports(&ps), "8080->80/tcp, 127.0.0.1:5432->5432/tcp, 6379/tcp");
    }

    #[test]
    fn browser_url_first_published() {
        assert_eq!(
            browser_url(&[port(None, 6379, None), port(Some("0.0.0.0"), 80, Some(8080))]).as_deref(),
            Some("http://localhost:8080/")
        );
        assert_eq!(browser_url(&[port(None, 6379, None)]), None);
    }

    #[test]
    fn exited_colour_depends_on_code() {
        let t = Theme::default();
        let mut c = container("x", ContainerState::Exited, None);
        c.exit_code = Some(0);
        assert_eq!(state_color(&c, &t), t.yellow);
        c.exit_code = Some(1);
        assert_eq!(state_color(&c, &t), t.red);
        assert_eq!(substatus(&c, &t).unwrap().content, "(1)");
    }

    #[test]
    fn sizes() {
        assert_eq!(size(999), "999B");
        assert_eq!(size(12_300_000), "12.3MB");
        assert_eq!(size_binary(2048), "2.0KiB");
    }
}
