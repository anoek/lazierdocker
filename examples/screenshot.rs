//! Render the README screenshot from made-up Docker state (no daemon needed).
//!
//!     cargo run --example screenshot -- screenshot.html [COLS ROWS]
//!
//! Writes the screen as HTML; `make screenshot` turns it into
//! `docs/screenshot.png` with headless Chromium and ImageMagick.

use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use lazierdocker::app::actions::Action;
use lazierdocker::app::inspect::Tab;
use lazierdocker::app::{App, Content};
use lazierdocker::docker::model::{
    derive_projects, Container, ContainerState, Health, Image, LocalCompose, Network, Port, Snapshot, Volume,
    LABEL_PROJECT, LABEL_SERVICE,
};
use lazierdocker::docker::stats::{StatsHistory, StatsSample};
use lazierdocker::event::AppEvent;
use lazierdocker::term::TermSurface;
use lazierdocker::ui::layout::SplitDir;
use lazierdocker::ui::sidebar::NodeKey;

/// Terminal background behind cells that keep the default colour.
const BG: &str = "#1e1e2e";
const FG: &str = "#cdd6f4";

fn container(name: &str, image: &str, state: ContainerState, status: &str, svc: Option<(&str, &str)>) -> Container {
    let mut labels = BTreeMap::new();
    if let Some((p, s)) = svc {
        labels.insert(LABEL_PROJECT.into(), p.into());
        labels.insert(LABEL_SERVICE.into(), s.into());
    }
    let id: String =
        format!("{:x}", name.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3)));
    Container {
        id: format!("{id}{id}"),
        name: name.into(),
        image: image.into(),
        state,
        status: status.into(),
        exit_code: (state == ContainerState::Exited).then_some(0),
        health: None,
        ports: vec![],
        labels,
        project: svc.map(|s| s.0.into()),
        service: svc.map(|s| s.1.into()),
        oneoff: false,
        created: 0,
    }
}

fn snapshot() -> Snapshot {
    use ContainerState::*;
    let mut containers = vec![
        container("shop-web-1", "nginx:1.27-alpine", Running, "Up 3 hours", Some(("shop", "web"))),
        container("shop-api-1", "shop/api:dev", Running, "Up 3 hours", Some(("shop", "api"))),
        container("shop-worker-1", "shop/api:dev", Running, "Up 3 hours", Some(("shop", "worker"))),
        container("shop-postgres-1", "postgres:17", Running, "Up 3 hours (healthy)", Some(("shop", "postgres"))),
        container("shop-redis-1", "redis:7.4", Running, "Up 3 hours", Some(("shop", "redis"))),
        container("obs-grafana-1", "grafana/grafana:11.3", Running, "Up 2 days", Some(("observability", "grafana"))),
        container(
            "obs-prometheus-1",
            "prom/prometheus:v3.0",
            Running,
            "Up 2 days",
            Some(("observability", "prometheus")),
        ),
        container("obs-loki-1", "grafana/loki:3.2", Running, "Up 2 days", Some(("observability", "loki"))),
        container("buildkitd", "moby/buildkit:v0.17", Running, "Up 5 days", None),
        container("pg-backup", "postgres:17", Exited, "Exited (0) 40 minutes ago", None),
    ];
    containers[0].ports =
        vec![Port { ip: Some("0.0.0.0".into()), private: 80, public: Some(8080), proto: "tcp".into() }];
    containers[3].health = Some(Health::Healthy);
    let local = LocalCompose {
        project: "shop".into(),
        services: ["web", "api", "worker", "postgres", "redis"].map(String::from).to_vec(),
    };
    let image = |name: &str, tag: &str, mb: i64| Image {
        id: format!("sha256:{:064x}", mb),
        name: name.into(),
        tag: tag.into(),
        size: mb * 1_000_000,
        created: 0,
        containers: 1,
    };
    Snapshot {
        projects: derive_projects(&containers, Some(&local)),
        containers,
        images: vec![
            image("shop/api", "dev", 412),
            image("postgres", "17", 438),
            image("nginx", "1.27-alpine", 48),
            image("redis", "7.4", 117),
            image("grafana/grafana", "11.3", 474),
            image("prom/prometheus", "v3.0", 296),
        ],
        volumes: ["shop_pgdata", "shop_redis", "obs_grafana", "obs_prometheus"]
            .map(|n| Volume { name: n.into(), driver: "local".into(), labelled: true })
            .to_vec(),
        networks: ["shop_default", "observability_default", "bridge"]
            .map(|n| Network { id: format!("{n}-id"), name: n.into(), driver: "bridge".into() })
            .to_vec(),
    }
}

/// A sample whose CPU and memory percentages come out as given.
fn sample(cpu: f64, mem_mb: u64, pids: u64) -> StatsSample {
    let cpus = 8;
    let system_delta = 1_000_000_000u64 * cpus;
    StatsSample {
        cpu_total: 1_000_000_000 + (cpu / 100.0 / cpus as f64 * system_delta as f64) as u64,
        precpu_total: 1_000_000_000,
        system: 10 * system_delta,
        presystem: 9 * system_delta,
        online_cpus: cpus as u32,
        mem_usage: mem_mb * 1024 * 1024,
        mem_limit: 2048 * 1024 * 1024,
        pids,
        rx_bytes: 0,
        tx_bytes: 0,
    }
}

/// Two and a half minutes of plausible postgres load.
fn history() -> StatsHistory {
    let now = Instant::now();
    let mut h = StatsHistory { samples: VecDeque::new() };
    for i in 0..150u64 {
        let t = i as f64;
        let cpu =
            6.0 + 4.0 * (t / 9.0).sin() + 2.5 * (t / 3.1).cos().abs() + if (60..72).contains(&i) { 18.0 } else { 0.0 };
        let mem = 180 + (i % 50) * 4 + (12.0 * (t / 2.3).sin()) as u64 + i;
        h.push(now - Duration::from_secs(150 - i), sample(cpu, mem, 14 + (i / 30)));
    }
    h
}

fn logs() -> String {
    const PATHS: [&str; 6] =
        ["/api/cart", "/api/products?page=2", "/api/orders/8812", "/api/session", "/api/checkout", "/healthz"];
    let mut out = String::new();
    for i in 0..60u32 {
        let ms_total = 14 * 60_000 + i * 2_400 + (i * 397) % 1_900;
        let (min, sec, ms) = (ms_total / 60_000, ms_total / 1000 % 60, ms_total % 1000);
        let ts = format!("\x1b[2m2026-11-16T09:{min:02}:{sec:02}.{ms:03}Z\x1b[0m");
        let line = match i % 12 {
            4 => format!("{ts} \x1b[33mWARN \x1b[0m payments: upstream timeout after 5000ms, retrying (attempt 2/3)"),
            9 if i > 40 => format!(
                "{ts} \x1b[31mERROR\x1b[0m checkout: payment provider timeout, order \x1b[36m#88{i:02}\x1b[0m queued for retry"
            ),
            7 => format!("{ts} \x1b[32mINFO \x1b[0m worker: processed \x1b[36m{}\x1b[0m jobs in 1.2s", 20 + i % 7),
            _ => {
                let path = PATHS[(i as usize * 5) % PATHS.len()];
                let ms = 3 + (i * 29) % 140;
                format!("{ts} \x1b[32mINFO \x1b[0m http: GET {path} \x1b[32m200\x1b[0m {ms}ms")
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn draw(app: &mut App, w: u16, h: u16) -> Buffer {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    app.render_buf(&mut buf, area);
    buf
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "screenshot.html".into());
    let cols: u16 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(150);
    let rows: u16 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(42);

    let mut app = App::new("unix:///var/run/docker.sock");
    app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot())));
    let cpu = [
        ("shop-web-1", 0.4),
        ("shop-api-1", 12.7),
        ("shop-worker-1", 3.1),
        ("shop-redis-1", 0.9),
        ("obs-grafana-1", 1.6),
        ("obs-prometheus-1", 2.3),
        ("obs-loki-1", 1.1),
        ("buildkitd", 0.0),
    ];
    for (name, pct) in cpu {
        let id = app.snap.containers.iter().find(|c| c.name == name).unwrap().id.clone();
        app.cpu.insert(id, pct);
    }
    let pg = app.snap.containers.iter().find(|c| c.name == "shop-postgres-1").unwrap().id.clone();
    let pg_history = history();
    app.cpu.insert(pg.clone(), pg_history.latest().map(|s| s.cpu_percent()).unwrap_or_default());
    app.stats.insert(pg, pg_history);

    app.sidebar.toggle(&NodeKey::Section(lazierdocker::ui::sidebar::Section::Images));
    app.rebuild_rows();

    // Top: the api's logs (selected in the sidebar). Bottom: postgres stats.
    let api = NodeKey::Service("shop".into(), "api".into());
    let rows_now = app.rows().to_vec();
    app.sidebar.select(&rows_now, &api);
    let top = app.layout.focus;
    app.set_tab(top, Tab::Logs);
    app.perform(Action::Pin(NodeKey::Service("shop".into(), "postgres".into()), SplitDir::Vertical));
    let bottom = app.layout.focus;
    app.set_tab(bottom, Tab::Stats);
    app.layout.focus = top;
    app.sync_panes();
    draw(&mut app, cols, rows);

    // Fill the logs pane as the stream would.
    let term = TermSurface::stream(80, 20, Arc::new(|| {})).expect("terminal");
    term.feeder(true).feed(logs().as_bytes());
    app.panes.get_mut(&top).unwrap().content = Content::Term(term);
    draw(&mut app, cols, rows);

    // Search the logs.
    app.focus = lazierdocker::app::Focus::Panes;
    app.open_find();
    for c in "timeout".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('n')));
    app.sync_panes();
    draw(&mut app, cols, rows);
    let buf = draw(&mut app, cols, rows);
    std::fs::write(&out, html(&buf)).expect("write html");
    eprintln!("wrote {out}");
}

/// xterm palette, with the 16 base colours from Catppuccin Mocha.
fn indexed(i: u8) -> String {
    const BASE: [&str; 16] = [
        "#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de", "#585b70", "#f38ba8",
        "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8",
    ];
    match i {
        0..=15 => BASE[i as usize].into(),
        16..=231 => {
            let v = |n: u8| if n == 0 { 0 } else { 55 + n * 40 };
            let n = i - 16;
            format!("#{:02x}{:02x}{:02x}", v(n / 36), v((n / 6) % 6), v(n % 6))
        }
        _ => {
            let g = 8 + (i - 232) * 10;
            format!("#{g:02x}{g:02x}{g:02x}")
        }
    }
}

fn css(c: Color, default: &str) -> String {
    match c {
        Color::Reset => default.into(),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => indexed(i),
        Color::Black => indexed(0),
        Color::Red => indexed(1),
        Color::Green => indexed(2),
        Color::Yellow => indexed(3),
        Color::Blue => indexed(4),
        Color::Magenta => indexed(5),
        Color::Cyan => indexed(6),
        Color::Gray => indexed(7),
        Color::DarkGray => indexed(8),
        Color::LightRed => indexed(9),
        Color::LightGreen => indexed(10),
        Color::LightYellow => indexed(11),
        Color::LightBlue => indexed(12),
        Color::LightMagenta => indexed(13),
        Color::LightCyan => indexed(14),
        Color::White => indexed(15),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Each cell is a fixed-width box so box drawing, braille and wide glyphs
/// stay on the grid whatever font supplies them.
fn html(buf: &Buffer) -> String {
    let area = buf.area;
    let mut body = String::new();
    for y in 0..area.height {
        body.push_str("<div class=r>");
        let mut x = 0;
        while x < area.width {
            let cell = &buf[(x, y)];
            let wide = unicode_width::UnicodeWidthStr::width(cell.symbol()).max(1) as u16;
            let mut fg = css(cell.fg, FG);
            let mut bg = css(cell.bg, BG);
            if cell.modifier.contains(Modifier::REVERSED) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if cell.modifier.contains(Modifier::DIM) {
                fg = format!("color-mix(in srgb,{fg} 55%,{bg})");
            }
            let mut style = format!("color:{fg};background:{bg}");
            if cell.modifier.contains(Modifier::BOLD) {
                style.push_str(";font-weight:700");
            }

            if cell.modifier.contains(Modifier::ITALIC) {
                style.push_str(";font-style:italic");
            }
            if cell.modifier.contains(Modifier::UNDERLINED) {
                style.push_str(";text-decoration:underline");
            }
            // Draw block elements as fills: font glyphs leave seams between columns.
            let eighths = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"].iter().position(|b| *b == cell.symbol());
            if let Some(n) = eighths {
                let pct = (n + 1) * 100 / 8;
                let _ = write!(
                    body,
                    "<span class=c style=\"width:1ch;background:linear-gradient(to top,{fg} {pct}%,{bg} {pct}%)\"> </span>"
                );
                x += 1;
                continue;
            }
            let sym = if cell.symbol().is_empty() { " " } else { cell.symbol() };
            let _ = write!(body, "<span class=c style=\"{style};width:{}ch\">{}</span>", wide, escape(sym));
            x += wide;
        }
        body.push_str("</div>\n");
    }
    format!(
        "<!doctype html><meta charset=utf-8><style>\
         html,body{{margin:0;background:{BG}}}\
         .t{{padding:14px 16px;display:inline-block;background:{BG};font:15px/1.25 'JetBrains Mono','DejaVu Sans Mono',monospace;font-variant-ligatures:none}}\
         .r{{height:1.25em;white-space:pre;display:flex}}\
         .c{{display:inline-block;overflow:hidden;text-align:center}}\
         </style><div class=t id=t>{body}</div>"
    )
}
