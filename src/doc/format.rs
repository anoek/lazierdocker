//! Formatters producing styled lines for the inspector tabs (lazydocker §2 layouts).

use std::collections::HashMap;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::graph;
use crate::docker::model::Snapshot;
use crate::docker::present::{self, size, size_binary};
use crate::docker::stats::StatsHistory;
use crate::ui::sidebar::Section;
use crate::ui::theme::Theme;

/// Styled lines of a document.
pub type Lines = Vec<Line<'static>>;

fn key_style(t: &Theme) -> Style {
    Style::default().fg(t.cyan)
}

/// Render a JSON value as YAML-ish lines: keys cyan, strings green, numbers
/// yellow, booleans magenta, null dim.
pub fn yaml(v: &Value, t: &Theme) -> Lines {
    let mut out = Vec::new();
    yaml_into(v, 0, t, &mut out);
    out
}

fn scalar(v: &Value, t: &Theme) -> Option<Span<'static>> {
    Some(match v {
        Value::Null => Span::styled("null", Style::default().fg(t.overlay0)),
        Value::Bool(b) => Span::styled(b.to_string(), Style::default().fg(t.magenta)),
        Value::Number(n) => Span::styled(n.to_string(), Style::default().fg(t.yellow)),
        Value::String(s) => {
            let needs_quotes = s.is_empty() || s.contains(": ") || s.starts_with(['-', '{', '[', '*', '&', '!', '#']);
            let shown = if needs_quotes { format!("{s:?}") } else { s.clone() };
            Span::styled(shown, Style::default().fg(t.green))
        }
        Value::Array(a) if a.is_empty() => Span::styled("[]", Style::default().fg(t.overlay0)),
        Value::Object(o) if o.is_empty() => Span::styled("{}", Style::default().fg(t.overlay0)),
        _ => return None,
    })
}

fn yaml_into(v: &Value, indent: usize, t: &Theme, out: &mut Lines) {
    let pad = " ".repeat(indent);
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                let key = Span::styled(format!("{pad}{k}:"), key_style(t));
                match scalar(val, t) {
                    Some(s) => out.push(Line::from(vec![key, Span::raw(" "), s])),
                    None => {
                        out.push(Line::from(key));
                        yaml_into(val, indent + 2, t, out);
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                match scalar(item, t) {
                    Some(s) => out.push(Line::from(vec![Span::raw(format!("{pad}- ")), s])),
                    None => {
                        // Nested object/array: first line on the dash.
                        let mut nested = Vec::new();
                        yaml_into(item, indent + 2, t, &mut nested);
                        for (i, mut line) in nested.into_iter().enumerate() {
                            if i == 0 {
                                if let Some(first) = line.spans.first_mut() {
                                    let content = first.content.to_string();
                                    let trimmed = content.get(indent + 2..).unwrap_or(&content).to_string();
                                    first.content = format!("{pad}- {trimmed}").into();
                                }
                            }
                            out.push(line);
                        }
                    }
                }
            }
        }
        other => {
            if let Some(s) = scalar(other, t) {
                out.push(Line::from(vec![Span::raw(pad), s]));
            }
        }
    }
}

/// Colour plain YAML text (e.g. `docker compose config` output): keys cyan.
pub fn yaml_text(text: &str, t: &Theme) -> Lines {
    text.lines()
        .map(|l| {
            let trimmed = l.trim_start();
            let indent = &l[..l.len() - trimmed.len()];
            let (dash, rest) = match trimmed.strip_prefix("- ") {
                Some(r) => ("- ", r),
                None => ("", trimmed),
            };
            match rest.find(':') {
                Some(i) if !rest[..i].contains(' ') || rest[..i].starts_with('"') => {
                    let (k, v) = rest.split_at(i + 1);
                    Line::from(vec![
                        Span::raw(format!("{indent}{dash}")),
                        Span::styled(k.to_string(), key_style(t)),
                        Span::styled(v.to_string(), Style::default().fg(t.green)),
                    ])
                }
                _ => Line::from(vec![
                    Span::raw(format!("{indent}{dash}")),
                    Span::styled(rest.to_string(), Style::default().fg(t.green)),
                ]),
            }
        })
        .collect()
}

fn field(label: &str, value: impl Into<String>, pad: usize, t: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<pad$}", format!("{label}:")),
            Style::default().fg(t.accent).add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.into(), Style::default().fg(t.text)),
    ])
}

fn s(v: &Value, ptr: &str) -> String {
    match v.pointer(ptr) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

fn heading(text: &str, t: &Theme) -> Line<'static> {
    Line::from(Span::styled(text.to_string(), Style::default().fg(t.mauve).add_modifier(Modifier::BOLD)))
}

/// Container Config tab: summary fields then the full inspect as YAML.
pub fn container_config(inspect: &Value, t: &Theme) -> Lines {
    const PAD: usize = 10;
    let mut out = vec![
        field("ID", s(inspect, "/Id"), PAD, t),
        field("Name", s(inspect, "/Name").trim_start_matches('/').to_string(), PAD, t),
        field("Image", s(inspect, "/Config/Image"), PAD, t),
    ];
    let mut cmd = vec![s(inspect, "/Path")];
    if let Some(Value::Array(args)) = inspect.pointer("/Args") {
        cmd.extend(args.iter().filter_map(|a| a.as_str().map(String::from)));
    }
    out.push(field("Command", cmd.join(" ").trim().to_string(), PAD, t));
    // Labels (sorted).
    if let Some(Value::Object(labels)) = inspect.pointer("/Config/Labels") {
        if !labels.is_empty() {
            out.push(field("Labels", "", PAD, t));
            let mut keys: Vec<_> = labels.keys().collect();
            keys.sort();
            for k in keys {
                out.push(Line::from(vec![
                    Span::styled(format!("  {k}: "), Style::default().fg(t.yellow)),
                    Span::raw(labels[k].as_str().unwrap_or_default().to_string()),
                ]));
            }
        }
    }
    if let Some(Value::Array(mounts)) = inspect.pointer("/Mounts") {
        if !mounts.is_empty() {
            out.push(field("Mounts", "", PAD, t));
            for m in mounts {
                let typ = s(m, "/Type");
                let line = if typ == "volume" {
                    format!("  volume: {} → {}", s(m, "/Name"), s(m, "/Destination"))
                } else {
                    format!("  {typ}: {} → {}", s(m, "/Source"), s(m, "/Destination"))
                };
                out.push(Line::raw(line));
            }
        }
    }
    if let Some(Value::Object(ports)) = inspect.pointer("/NetworkSettings/Ports") {
        if !ports.is_empty() {
            out.push(field("Ports", "", PAD, t));
            let mut keys: Vec<_> = ports.keys().collect();
            keys.sort();
            for k in keys {
                let hosts: Vec<String> = match &ports[k] {
                    Value::Array(bs) => {
                        bs.iter().map(|b| format!("{}:{}", s(b, "/HostIp"), s(b, "/HostPort"))).collect()
                    }
                    _ => vec![],
                };
                let host = if hosts.is_empty() { "(not published)".to_string() } else { hosts.join(", ") };
                out.push(Line::from(vec![
                    Span::styled(format!("  {host}"), Style::default().fg(t.yellow)),
                    Span::raw(format!(" → {k}")),
                ]));
            }
        }
    }
    out.push(Line::raw(""));
    out.push(heading("Full details:", t));
    out.push(Line::raw(""));
    out.extend(yaml(inspect, t));
    out
}

/// Env tab: `KEY:` green, value yellow.
pub fn env(inspect: &Value, t: &Theme) -> Lines {
    let vars: Vec<String> = match inspect.pointer("/Config/Env") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        _ => vec![],
    };
    if vars.is_empty() {
        return vec![Line::styled("Nothing to display", Style::default().fg(t.overlay0))];
    }
    let width = vars.iter().map(|v| v.split('=').next().unwrap_or("").len()).max().unwrap_or(0) + 2;
    vars.iter()
        .map(|v| {
            let (k, val) = v.split_once('=').unwrap_or((v.as_str(), ""));
            Line::from(vec![
                Span::styled(format!("{:<width$}", format!("{k}:")), Style::default().fg(t.green)),
                Span::styled(val.to_string(), Style::default().fg(t.yellow)),
            ])
        })
        .collect()
}

/// Image Config tab: header and history table (strips `/bin/sh -c #(nop) `).
pub fn image_config(inspect: &Value, history: &Value, t: &Theme) -> Lines {
    const PAD: usize = 9;
    let tags: Vec<String> = match inspect.pointer("/RepoTags") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
        _ => vec![],
    };
    let mut out = vec![
        field("Name", tags.first().cloned().unwrap_or_else(|| "<none>".into()), PAD, t),
        field("ID", s(inspect, "/Id"), PAD, t),
        Line::from(vec![
            Span::styled(format!("{:<PAD$}", "Tags:"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(tags.join(", "), Style::default().fg(t.green)),
        ]),
        field("Size", inspect.pointer("/Size").and_then(Value::as_i64).map(size).unwrap_or_default(), PAD, t),
        field("Created", s(inspect, "/Created"), PAD, t),
        Line::raw(""),
        Line::styled(
            format!("{:<12} {:<24} {:>10}  {}", "ID", "TAG", "SIZE", "COMMAND"),
            Style::default().fg(t.overlay1).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Value::Array(items) = history {
        for h in items {
            let id_full = s(h, "/Id");
            let id = id_full.strip_prefix("sha256:").unwrap_or(&id_full);
            let id = if id.len() > 10 { &id[..10] } else { id };
            let tag = match h.pointer("/Tags") {
                Some(Value::Array(a)) => a.first().and_then(Value::as_str).unwrap_or("").to_string(),
                _ => String::new(),
            };
            let sz = h.pointer("/Size").and_then(Value::as_i64).unwrap_or(0);
            let cmd = s(h, "/CreatedBy");
            let cmd = cmd.strip_prefix("/bin/sh -c #(nop) ").unwrap_or(&cmd).trim().to_string();
            let (first, rest) = cmd.split_once(' ').unwrap_or((&cmd, ""));
            out.push(Line::from(vec![
                Span::styled(
                    format!("{id:<12} "),
                    Style::default().fg(if id == "<missing>" { t.blue } else { t.subtext0 }),
                ),
                Span::raw(format!("{tag:<24} ")),
                Span::styled(
                    format!("{:>10}  ", size_binary(sz)),
                    Style::default().fg(if sz == 0 { t.blue } else { t.text }),
                ),
                Span::styled(first.to_string(), Style::default().fg(t.yellow)),
                Span::raw(if rest.is_empty() { String::new() } else { format!(" {rest}") }),
            ]));
        }
    }
    out
}

fn map_lines(v: &Value, ptr: &str, t: &Theme) -> Lines {
    match v.pointer(ptr) {
        Some(Value::Object(m)) if !m.is_empty() => {
            let mut keys: Vec<_> = m.keys().collect();
            keys.sort();
            keys.into_iter()
                .map(|k| {
                    let val = match &m[k] {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    Line::from(vec![Span::styled(format!("  {k}: "), Style::default().fg(t.yellow)), Span::raw(val)])
                })
                .collect()
        }
        _ => vec![Line::styled("  none", Style::default().fg(t.overlay0))],
    }
}

/// Volume Config tab.
pub fn volume_config(v: &Value, t: &Theme) -> Lines {
    const PAD: usize = 15;
    let mut out = vec![
        field("Name", s(v, "/Name"), PAD, t),
        field("Driver", s(v, "/Driver"), PAD, t),
        field("Scope", s(v, "/Scope"), PAD, t),
        field("Mountpoint", s(v, "/Mountpoint"), PAD, t),
        field("Created", s(v, "/CreatedAt"), PAD, t),
        field("Labels", "", PAD, t),
    ];
    out.extend(map_lines(v, "/Labels", t));
    out.push(field("Options", "", PAD, t));
    out.extend(map_lines(v, "/Options", t));
    let status = v.pointer("/Status").is_some_and(|s| !s.is_null());
    out.push(field("Status", if status { "" } else { "n/a" }, PAD, t));
    if status {
        out.extend(map_lines(v, "/Status", t));
    }
    if let Some(u) = v.pointer("/UsageData") {
        if !u.is_null() {
            out.push(field("RefCount", s(u, "/RefCount"), PAD, t));
            out.push(field("Size", u.pointer("/Size").and_then(Value::as_i64).map(size).unwrap_or_default(), PAD, t));
        }
    }
    out
}

/// Network Config tab.
pub fn network_config(n: &Value, t: &Theme) -> Lines {
    const PAD: usize = 13;
    let mut out = vec![
        field("ID", s(n, "/Id"), PAD, t),
        field("Name", s(n, "/Name"), PAD, t),
        field("Driver", s(n, "/Driver"), PAD, t),
        field("Scope", s(n, "/Scope"), PAD, t),
        field("EnableIPv6", s(n, "/EnableIPv6"), PAD, t),
        field("Internal", s(n, "/Internal"), PAD, t),
        field("Attachable", s(n, "/Attachable"), PAD, t),
        field("Ingress", s(n, "/Ingress"), PAD, t),
        field("Containers", "", PAD, t),
    ];
    match n.pointer("/Containers") {
        Some(Value::Object(m)) if !m.is_empty() => {
            for (id, c) in m {
                let short = &id[..id.len().min(12)];
                out.push(Line::from(vec![
                    Span::styled(format!("  {}: ", s(c, "/Name")), Style::default().fg(t.yellow)),
                    Span::raw(format!("{} {}", s(c, "/IPv4Address"), short)),
                ]));
            }
        }
        _ => out.push(Line::styled("  none", Style::default().fg(t.overlay0))),
    }
    out.push(field("Labels", "", PAD, t));
    out.extend(map_lines(n, "/Labels", t));
    out.push(field("Options", "", PAD, t));
    out.extend(map_lines(n, "/Options", t));
    out
}

/// Top tab: a table with the titles as the header row.
pub fn top(titles: &[String], processes: &[Vec<String>], t: &Theme) -> Lines {
    let cols = titles.len();
    let mut widths: Vec<usize> = titles.iter().map(|s| s.chars().count()).collect();
    for p in processes {
        for (i, cell) in p.iter().enumerate().take(cols.saturating_sub(1)) {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let row = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| if i + 1 < cols { format!("{c:<w$}", w = widths[i]) } else { c.clone() })
            .collect::<Vec<_>>()
            .join("  ")
    };
    let mut out = vec![Line::styled(row(titles), Style::default().fg(t.overlay1).add_modifier(Modifier::BOLD))];
    out.extend(processes.iter().map(|p| Line::raw(row(p))));
    out
}

/// Stats tab: CPU and memory graphs, PIDs, traffic, memory usage (lazydocker layout).
pub fn stats(h: Option<&StatsHistory>, running: bool, width: usize, t: &Theme) -> Lines {
    let dim = Style::default().fg(t.overlay0);
    let Some(h) = h.filter(|h| !h.samples.is_empty()) else {
        return vec![Line::styled(if running { "Waiting for stats…" } else { "container is not running" }, dim)];
    };
    let latest = h.latest().expect("non-empty");
    let span = graph::span_label(h.span());
    let cpu: Vec<f64> = h.samples.iter().map(|(_, s)| s.cpu_percent()).collect();
    let mem: Vec<f64> = h.samples.iter().map(|(_, s)| s.mem_percent()).collect();
    let mut out = vec![graph::caption(format!("CPU (%): {:.2} ({span})", latest.cpu_percent()), t.cyan)];
    out.extend(graph::bars(&cpu, width, 8, 1.0, t.cyan, dim));
    out.push(Line::raw(""));
    out.push(graph::caption(format!("Memory (%): {:.2} ({span})", latest.mem_percent()), t.green));
    out.extend(graph::bars(&mem, width, 8, 1.0, t.green, dim));
    out.push(Line::raw(""));
    let kv = |k: &str, v: String| {
        Line::from(vec![
            Span::styled(format!("{k:<18}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::raw(v),
        ])
    };
    out.push(kv("PIDs:", latest.pids.to_string()));
    out.push(kv(
        "Memory:",
        format!(
            "{} / {}",
            present::size_binary(latest.mem_usage as i64),
            present::size_binary(latest.mem_limit as i64)
        ),
    ));
    out.push(kv("Traffic received:", present::size(latest.rx_bytes as i64)));
    out.push(kv("Traffic sent:", present::size(latest.tx_bytes as i64)));
    out
}

/// Section summary tables.
pub fn summary(section: Section, snap: &Snapshot, cpu: &HashMap<String, f64>, t: &Theme) -> Lines {
    let head = |s: String| Line::styled(s, Style::default().fg(t.overlay1).add_modifier(Modifier::BOLD));
    let dim = Style::default().fg(t.overlay0);
    let mut out = Vec::new();
    match section {
        Section::Projects => {
            out.push(head(format!("{:<28} {:>9}  {}", "PROJECT", "RUNNING", "SERVICES")));
            for p in &snap.projects {
                let running = p
                    .services
                    .iter()
                    .filter(|s| {
                        s.container_id.as_deref().and_then(|id| snap.container(id)).is_some_and(|c| c.is_running())
                    })
                    .count();
                let names: Vec<&str> = p.services.iter().map(|s| s.name.as_str()).collect();
                out.push(Line::from(vec![
                    Span::raw(format!("{:<28} ", p.name)),
                    Span::raw(format!("{:>9}  ", format!("{running}/{}", p.services.len()))),
                    Span::styled(names.join(", "), dim),
                ]));
            }
        }
        Section::Containers => {
            out.push(head(format!("{:<11} {:<30} {:>8}  {:<28} {}", "STATE", "NAME", "CPU", "PORTS", "IMAGE")));
            for c in &snap.containers {
                let state = match present::substatus(c, t) {
                    Some(sub) => format!("{} {}", c.state.as_str(), sub.content),
                    None => c.state.as_str().to_string(),
                };
                let cpu = cpu.get(&c.id).map(|v| format!("{v:.2}%")).unwrap_or_default();
                out.push(Line::from(vec![
                    Span::styled(format!("{state:<11} "), Style::default().fg(present::state_color(c, t))),
                    Span::raw(format!("{:<30} ", c.name)),
                    Span::raw(format!("{cpu:>8}  ")),
                    Span::styled(format!("{:<28} ", present::ports(&c.ports)), Style::default().fg(t.yellow)),
                    Span::styled(c.image.clone(), Style::default().fg(t.magenta)),
                ]));
            }
        }
        Section::Images => {
            out.push(head(format!("{:<40} {:<20} {:>9}  {}", "NAME", "TAG", "SIZE", "ID")));
            for i in &snap.images {
                out.push(Line::from(vec![
                    Span::raw(format!("{:<40} ", i.name)),
                    Span::raw(format!("{:<20} ", i.tag)),
                    Span::raw(format!("{:>9}  ", present::size(i.size))),
                    Span::styled(i.short_id().to_string(), dim),
                ]));
            }
        }
        Section::Volumes => {
            out.push(head(format!("{:<10} {}", "DRIVER", "NAME")));
            for v in &snap.volumes {
                out.push(Line::from(vec![Span::styled(format!("{:<10} ", v.driver), dim), Span::raw(v.name.clone())]));
            }
        }
        Section::Networks => {
            out.push(head(format!("{:<10} {}", "DRIVER", "NAME")));
            for n in &snap.networks {
                out.push(Line::from(vec![Span::styled(format!("{:<10} ", n.driver), dim), Span::raw(n.name.clone())]));
            }
        }
    }
    if out.len() == 1 {
        out.push(Line::styled("none", dim));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(lines: &Lines) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    #[test]
    fn yaml_colouring_keys_cyan() {
        let t = Theme::default();
        let lines = yaml(
            &json!({"Name": "web", "Count": 2, "On": true, "Tags": ["a", "b"], "Nested": {"X": null}, "Empty": []}),
            &t,
        );
        assert_eq!(
            text(&lines),
            vec!["Count: 2", "Empty: []", "Name: web", "Nested:", "  X: null", "On: true", "Tags:", "  - a", "  - b"]
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(t.cyan));
        assert_eq!(lines[0].spans[2].style.fg, Some(t.yellow));
        assert_eq!(lines[2].spans[2].style.fg, Some(t.green));
    }

    #[test]
    fn yaml_array_of_objects_dash_on_first_key() {
        let t = Theme::default();
        let lines = yaml(&json!({"Mounts": [{"Type": "volume", "Name": "x"}]}), &t);
        assert_eq!(text(&lines), vec!["Mounts:", "  - Name: x", "    Type: volume"]);
    }

    #[test]
    fn env_table() {
        let t = Theme::default();
        let lines = env(&json!({"Config": {"Env": ["PATH=/usr/bin", "GREETING=hello=world"]}}), &t);
        assert_eq!(text(&lines), vec!["PATH:     /usr/bin", "GREETING: hello=world"]);
        assert_eq!(text(&env(&json!({}), &t)), vec!["Nothing to display"]);
    }

    #[test]
    fn container_config_header() {
        let t = Theme::default();
        let v = json!({
            "Id": "abc", "Name": "/web", "Path": "nginx", "Args": ["-g", "daemon off;"],
            "Config": {"Image": "nginx:alpine", "Labels": {"b": "2", "a": "1"}},
            "Mounts": [{"Type": "volume", "Name": "data", "Destination": "/data"}],
            "NetworkSettings": {"Ports": {"80/tcp": [{"HostIp": "0.0.0.0", "HostPort": "8080"}]}}
        });
        let lines = text(&container_config(&v, &t));
        assert_eq!(
            &lines[..4],
            &["ID:       abc", "Name:     web", "Image:    nginx:alpine", "Command:  nginx -g daemon off;"]
        );
        assert!(lines.contains(&"  a: 1".to_string()));
        assert!(lines.contains(&"  volume: data → /data".to_string()));
        assert!(lines.contains(&"  0.0.0.0:8080 → 80/tcp".to_string()));
        assert!(lines.contains(&"Full details:".to_string()));
    }

    #[test]
    fn image_history_strips_nop() {
        let t = Theme::default();
        let lines = text(&image_config(
            &json!({"Id": "sha256:1", "RepoTags": ["nginx:alpine"], "Size": 1000}),
            &json!([{"Id": "sha256:0123456789abcdef", "Tags": ["nginx:alpine"], "Size": 0, "CreatedBy": "/bin/sh -c #(nop)  CMD [\"nginx\"]"}]),
            &t,
        ));
        assert_eq!(lines[0], "Name:    nginx:alpine");
        let last = lines.last().unwrap();
        assert!(last.starts_with("0123456789"), "{last}");
        assert!(last.ends_with("CMD [\"nginx\"]"), "{last}");
    }

    #[test]
    fn yaml_text_keys() {
        let t = Theme::default();
        let lines = yaml_text("name: demo\nservices:\n  web:\n    image: nginx\n    ports:\n      - 8080:80", &t);
        assert_eq!(lines[3].spans[1].content, "image:");
        assert_eq!(lines[3].spans[1].style.fg, Some(t.cyan));
        assert_eq!(text(&lines)[5], "      - 8080:80");
    }

    #[test]
    fn top_table() {
        let t = Theme::default();
        let lines = top(&["PID".into(), "CMD".into()], &[vec!["1".into(), "sleep 100".into()]], &t);
        assert_eq!(text(&lines), vec!["PID  CMD", "1    sleep 100"]);
    }
}
