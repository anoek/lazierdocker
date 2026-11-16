use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::*;
use crate::app::actions::Action;
use crate::app::inspect::Tab;
use crate::app::overlay::Overlay;
use crate::docker::model::tests::container;
use crate::docker::model::{derive_projects, ContainerState};
use crate::docker::ops::DockerOp;
use crate::term::TermSurface;
use crate::ui::sidebar::Section;

pub fn demo_snapshot() -> Snapshot {
    let containers = vec![
        container("demo-web-1", ContainerState::Running, Some(("demo", "web"))),
        container("demo-db-1", ContainerState::Running, Some(("demo", "db"))),
        container("loner", ContainerState::Running, None),
        container("exited", ContainerState::Exited, None),
    ];
    Snapshot { projects: derive_projects(&containers, None), containers, ..Default::default() }
}

/// Install a terminal in a pane, as the runtime does for StartLogs.
fn install_term(app: &mut App, pane: PaneId) {
    app.panes.get_mut(&pane).unwrap().content = Content::Term(TermSurface::stream(80, 24, Arc::new(|| {})).unwrap());
}

pub fn loaded_app() -> App {
    let mut app = App::new("tcp://test");
    app.handle_app_event(AppEvent::Snapshot(Box::new(demo_snapshot())));
    app
}

pub fn draw(app: &mut App, w: u16, h: u16) -> Buffer {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    app.render_buf(&mut buf, area);
    buf
}

pub fn row_text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect()
}

pub fn screen(buf: &Buffer) -> String {
    (0..buf.area.height).map(|y| row_text(buf, y)).collect::<Vec<_>>().join("\n")
}

pub fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent { kind, column: col, row, modifiers: KeyModifiers::NONE }
}

pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

/// Press the ctrl+b prefix, then `c`.
fn prefix(app: &mut App, c: char) -> Vec<Effect> {
    app.handle_key(ctrl('b'));
    app.handle_key(key(KeyCode::Char(c)))
}

/// The id of the demo container called `name`.
fn container_id(app: &App, name: &str) -> String {
    app.snap.containers.iter().find(|c| c.name == name).unwrap().id.clone()
}

fn select_loner(app: &mut App) -> NodeKey {
    let k = NodeKey::Container(container_id(app, "loner"));
    app.select_key(&k);
    k
}

/// App with `lines` in the inspector, pinned to a section so sync won't
/// replace them. Returns the pane and its content area.
fn doc_app(lines: impl IntoIterator<Item = impl Into<String>>) -> (App, PaneId, Rect) {
    let mut app = loaded_app();
    app.select_key(&NodeKey::Section(Section::Volumes));
    app.sync_panes();
    let pane = app.layout.focus;
    let lines = lines.into_iter().map(|l| Line::raw(l.into())).collect();
    app.panes.get_mut(&pane).unwrap().content = Content::Doc(crate::doc::DocSurface::new(lines));
    draw(&mut app, 100, 20);
    let r = app.hits.content.iter().find(|(p, _)| *p == pane).unwrap().1;
    (app, pane, r)
}

/// Open a terminal pane running `cat`.
fn cat_pane(app: &mut App) -> PaneId {
    let id = app.open_pane(PaneKind::Terminal { title: "cat".into() }, None);
    let t = TermSurface::spawn(&["cat".into()], &[], 40, 10, Arc::new(|| {}), Arc::new(|_, _| {})).unwrap();
    app.panes.get_mut(&id).unwrap().content = Content::Term(t);
    id
}

fn term_text(app: &App, id: PaneId) -> String {
    match &app.panes[&id].content {
        Content::Term(t) => t.viewport_text().join("\n"),
        _ => String::new(),
    }
}

/// Poll `f` until it holds, for up to two seconds.
fn eventually(mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn render_shows_sections() {
    let mut app = loaded_app();
    let buf = draw(&mut app, 100, 20);
    let s = screen(&buf);
    for want in
        ["▾ Projects", "demo", "web", "▾ Containers", "loner", "▸ Images", "▸ Volumes", "▸ Networks", "tcp://test"]
    {
        assert!(s.contains(want), "missing {want:?} in\n{s}");
    }
    // First leaf selected on first load → inspector titled after it.
    assert_eq!(app.selected(), Some(&NodeKey::Project("demo".into())));
}

#[test]
fn render_tiny_terminal() {
    let mut app = loaded_app();
    for (w, h) in [(1, 1), (10, 3), (20, 4), (25, 5), (40, 6)] {
        let _ = draw(&mut app, w, h);
    }
}

#[test]
fn click_selects_row() {
    let mut app = loaded_app();
    let buf = draw(&mut app, 100, 20);
    let y = (0..20).find(|y| row_text(&buf, *y).contains("loner")).unwrap();
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 8, y));
    assert_eq!(app.selected().map(|k| app.target_label(k)), Some("loner".into()));
    assert_eq!(app.focus, Focus::Sidebar);
}

#[test]
fn click_toggle_collapses_section() {
    let mut app = loaded_app();
    let buf = draw(&mut app, 100, 20);
    let y = (0..20).find(|y| row_text(&buf, *y).contains("▾ Containers")).unwrap();
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, y));
    let buf = draw(&mut app, 100, 20);
    let s = screen(&buf);
    assert!(s.contains("▸ Containers"), "{s}");
    assert!(!s.contains("loner"), "{s}");
}

#[test]
fn divider_drag_changes_width() {
    let mut app = loaded_app();
    draw(&mut app, 100, 20);
    let div = app.hits.sidebar_divider.unwrap();
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), div.x, 5));
    app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 29, 5));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 29, 5));
    assert_eq!(app.sidebar.width, 30);
    assert!(app.drag.is_none());
    // Clamp to minimum.
    draw(&mut app, 100, 20);
    app.last_click = None;
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 29, 5));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 2, 5));
    assert_eq!(app.sidebar.width, crate::ui::sidebar::MIN_WIDTH);
}

#[test]
fn keyboard_navigation_and_filter() {
    let mut app = loaded_app();
    draw(&mut app, 100, 20);
    app.handle_key(key(KeyCode::Char('/')));
    for c in "lon".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(app.mode, Mode::Normal);
    let labels: Vec<_> = app.rows().iter().filter(|r| r.depth > 0).map(|r| r.label.clone()).collect();
    assert_eq!(labels, vec!["loner"]);
    app.handle_key(key(KeyCode::Esc));
    assert!(app.sidebar.filter.is_empty());
    let effects = app.handle_key(key(KeyCode::Char('q')));
    assert_eq!(effects, vec![Effect::Quit]);
}

#[test]
fn selection_moved_off_screen_scrolls_into_view() {
    let mut app = loaded_app();
    // 10 rows in a 6-row list.
    draw(&mut app, 100, 8);
    app.handle_key(key(KeyCode::Char('/')));
    for _ in 0..9 {
        app.handle_key(key(KeyCode::Down));
    }
    let buf = draw(&mut app, 100, 8);
    let sel = app.selected().cloned().unwrap();
    assert!(app.hits.sidebar.rows.iter().any(|(_, k)| *k == sel), "{sel:?} not shown:\n{}", screen(&buf));
    // Wheel scrolling still moves the list away from the selection.
    app.handle_key(key(KeyCode::Enter));
    let (x, y) = (app.hits.sidebar.list_area.x + 1, app.hits.sidebar.list_area.y + 1);
    app.handle_mouse(mouse(MouseEventKind::ScrollUp, x, y));
    draw(&mut app, 100, 8);
    assert_eq!(app.sidebar.scroll, 1);
}

#[test]
fn selection_pushed_down_by_new_rows_stays_in_view() {
    let mut app = loaded_app();
    draw(&mut app, 100, 8);
    // Filter down to "exited" and select it, then clear the filter: the full
    // tree puts it at row 7 of a 6-row list.
    app.handle_key(key(KeyCode::Char('/')));
    for c in "exited".chars() {
        app.handle_key(key(KeyCode::Char(c)));
    }
    app.handle_key(key(KeyCode::Enter));
    let exited = NodeKey::Container(container_id(&app, "exited"));
    app.select_key(&exited);
    draw(&mut app, 100, 8);
    app.handle_key(key(KeyCode::Esc));
    let buf = draw(&mut app, 100, 8);
    assert!(app.hits.sidebar.rows.iter().any(|(_, k)| *k == exited), "not shown:\n{}", screen(&buf));
}

#[test]
fn docker_error_shown_in_status() {
    let mut app = App::new("tcp://nowhere");
    app.handle_app_event(AppEvent::DockerError("connection refused".into()));
    let buf = draw(&mut app, 100, 10);
    assert!(row_text(&buf, 9).contains("⚠ connection refused"));
    assert!(screen(&buf).contains("Cannot reach docker"));
}

#[test]
fn long_docker_error_keeps_status_hints() {
    let mut app = App::new("tcp://nowhere");
    app.handle_app_event(AppEvent::DockerError(format!("listing containers: {}", "very long error text ".repeat(20))));
    let buf = draw(&mut app, 100, 10);
    let status = row_text(&buf, 9);
    assert!(status.contains("select") && status.contains("filter"), "hints still shown: {status:?}");
    assert!(status.contains("⚠ listing containers"), "{status:?}");
    assert!(status.trim_end().ends_with('…'), "{status:?}");
    // The pane wraps the message instead of cutting it.
    assert!(row_text(&buf, 2).contains("very long"), "{}", screen(&buf));
}

mod inspector {
    use super::*;
    use crate::app::inspect::{tabs_for, LoadSpec, TargetKind};

    #[test]
    fn placeholders_say_what_is_missing() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        let pane = app.layout.focus;
        // A service from the compose file that has no container yet.
        let svc = crate::docker::model::Service { name: "cache".into(), project: "demo".into(), replicas: vec![] };
        app.snap.projects[0].services.push(svc);
        app.rebuild_rows();
        app.select_key(&NodeKey::Service("demo".into(), "cache".into()));
        app.set_tab(pane, Tab::Env);
        app.sync_panes();
        assert_eq!(app.content_row_text(pane, 0), "service has no container");
        app.panes.get_mut(&pane).unwrap().kind = PaneKind::Pinned(NodeKey::Container("gone".into()));
        app.sync_panes();
        assert_eq!(app.content_row_text(pane, 0), "container no longer exists");
    }

    #[test]
    fn containers_summary_shows_live_cpu() {
        let mut app = loaded_app();
        let pane = app.layout.focus;
        app.select_key(&NodeKey::Section(Section::Containers));
        app.sync_panes();
        app.cpu.insert(container_id(&app, "loner"), 42.0);
        app.tick += 1;
        app.sync_panes();
        let text: Vec<String> = (0..6).map(|r| app.content_row_text(pane, r)).collect();
        assert!(text.iter().any(|l| l.contains("loner") && l.contains("42.00%")), "{text:#?}");
    }

    #[test]
    fn tabs_for_kinds() {
        assert_eq!(tabs_for(TargetKind::Container), &[Tab::Logs, Tab::Stats, Tab::Env, Tab::Config, Tab::Top]);
        assert_eq!(tabs_for(TargetKind::Project), &[Tab::Logs, Tab::Config]);
        assert_eq!(tabs_for(TargetKind::Image), &[Tab::Config]);
        assert_eq!(tabs_for(TargetKind::Section), &[Tab::Summary]);
    }

    #[test]
    fn config_tab_requests_inspect_and_applies_matching_result() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        app.select_key(&NodeKey::Container(id.clone()));
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Config);
        let effects = app.sync_panes();
        let Some(Effect::Load { key, spec, .. }) = effects.first().cloned() else { panic!("{effects:?}") };
        assert_eq!(spec, LoadSpec::ContainerConfig { id: id.clone(), note: None });
        // Nothing new to load on a second sync.
        assert!(app.sync_panes().is_empty());
        // A stale result (different key) is ignored; the matching one is shown.
        let mut stale = key.clone();
        stale.rev += 1;
        app.handle_app_event(AppEvent::Doc { pane, key: stale, lines: vec![Line::raw("STALE")] });
        app.handle_app_event(AppEvent::Doc { pane, key, lines: vec![Line::raw("ID: loner-id")] });
        let s = screen(&draw(&mut app, 100, 20));
        assert!(s.contains("ID: loner-id") && !s.contains("STALE"), "{s}");
        assert!(s.contains(" Config "), "tab bar shown: {s}");
    }

    #[test]
    fn service_tabs_resolve_to_its_container() {
        let mut app = loaded_app();
        app.select_key(&NodeKey::Service("demo".into(), "web".into()));
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Env);
        let effects = app.sync_panes();
        assert!(
            matches!(&effects[0], Effect::Load { spec: LoadSpec::ContainerEnv { id, note: None }, .. } if id.starts_with("demo-web-1"))
        );
    }

    #[test]
    fn click_tab_and_bracket_keys_switch_tabs() {
        let mut app = loaded_app();
        select_loner(&mut app);
        app.sync_panes();
        draw(&mut app, 100, 20);
        let (rect, pane, _) = *app.hits.tabs.iter().find(|(_, _, t)| *t == Tab::Top).unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), rect.x + 1, rect.y));
        assert_eq!(app.pane_tab(pane), Some(Tab::Top));
        app.handle_key(key(KeyCode::Char(']')));
        assert_eq!(app.pane_tab(pane), Some(Tab::Logs), "wraps around");
        // From the sidebar, [ ] drive the inspector too.
        app.focus = Focus::Sidebar;
        app.handle_key(key(KeyCode::Char('[')));
        assert_eq!(app.pane_tab(pane), Some(Tab::Top));
    }

    #[test]
    fn section_summary_is_immediate() {
        let mut app = loaded_app();
        app.select_key(&NodeKey::Section(Section::Containers));
        assert!(app.sync_panes().is_empty());
        let s = screen(&draw(&mut app, 120, 20));
        assert!(s.contains("STATE") && s.contains("loner"), "{s}");
    }

    #[test]
    fn wheel_scrolls_pane_under_pointer() {
        let mut app = loaded_app();
        let pane = app.layout.focus;
        app.panes.get_mut(&pane).unwrap().content =
            Content::Doc(crate::doc::DocSurface::new((0..100).map(|i| Line::raw(i.to_string())).collect()));
        app.select_key(&NodeKey::Section(Section::Volumes));
        draw(&mut app, 100, 20);
        let (_, r, _) = app.hits.panes[0];
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, r.x + 5, r.y + 5));
        let Content::Doc(d) = &app.panes[&pane].content else { panic!("doc expected") };
        assert_eq!(d.scroll, 3);
    }
}

mod streams {
    use super::*;
    use crate::app::inspect::LoadSpec;
    use crate::app::logs::{LogSource, StreamEnd};
    use crate::doc::format::stats as stats_lines;
    use crate::docker::logs::LogTime;
    use crate::docker::stats::{StatsHistory, StatsSample};

    #[test]
    fn logs_tab_starts_stream_once() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        app.select_key(&NodeKey::Container(id.clone()));
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Logs);
        let effects = app.sync_panes();
        let only_loner = [LogSource { id, prefix: None }];
        assert!(matches!(&effects[..], [Effect::StartLogs { sources, .. }] if *sources == only_loner), "{effects:?}");
        assert!(app.sync_panes().is_empty(), "no restart while nothing changed");
    }

    /// Start a logs stream for `loner` (terminal installed); returns its pane.
    fn start_loner_logs(app: &mut App) -> PaneId {
        select_loner(app);
        app.set_tab(app.layout.focus, Tab::Logs);
        let fx = app.sync_panes();
        let [Effect::StartLogs { pane, .. }] = &fx[..] else { panic!("{fx:?}") };
        install_term(app, *pane);
        *pane
    }

    /// The end report of `container`'s current stream run in a Logs pane.
    fn ended(app: &App, pane: PaneId, container: &str, clean: bool, last: &str) -> AppEvent {
        let run = app.panes[&pane].logs[container].run;
        let end = StreamEnd { container: container.to_string(), clean, last: LogTime::parse(last) };
        AppEvent::LogsEnded { pane, run, end }
    }

    fn with_state(app: &App, name: &str, state: ContainerState) -> Snapshot {
        let mut snap = app.snap.clone();
        snap.containers.iter_mut().find(|c| c.name == name).unwrap().state = state;
        snap
    }

    #[test]
    fn a_restart_appends_to_the_logs_instead_of_reloading_them() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        let pane = start_loner_logs(&mut app);
        app.handle_app_event(AppEvent::Snapshot(Box::new(with_state(&app, "loner", ContainerState::Exited))));
        assert!(app.sync_panes().is_empty(), "stopping doesn't reload the pane");
        app.handle_app_event(ended(&app, pane, &id, true, "2026-11-16T10:00:00.5Z"));
        app.handle_app_event(AppEvent::Snapshot(Box::new(with_state(&app, "loner", ContainerState::Running))));
        let fx = app.sync_panes();
        let [Effect::FollowLogs { source, after, .. }] = &fx[..] else { panic!("{fx:?}") };
        assert_eq!(source.id, id);
        assert_eq!(*after, LogTime::parse("2026-11-16T10:00:00.5Z"), "continues after the last line shown");
        assert!(matches!(app.panes[&pane].content, Content::Term(_)), "same terminal, output kept");
    }

    #[test]
    fn output_after_a_resume_starts_below_the_ended_notice() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        let pane = start_loner_logs(&mut app);
        app.handle_app_event(ended(&app, pane, &id, true, ""));
        let Content::Term(t) = &app.panes[&pane].content else { panic!() };
        t.feeder(false).feed(b"next run\r\n");
        let text = t.viewport_text();
        assert!(text.iter().any(|l| l == "[log stream ended]"), "{text:?}");
        assert!(text.iter().any(|l| l == "next run"), "{text:?}");
    }

    #[test]
    fn logs_resume_after_restart_missed_by_snapshots() {
        // `docker restart` is fast: snapshots may only ever see "running".
        // The ended stream must resume anyway.
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        let pane = start_loner_logs(&mut app);
        app.handle_app_event(ended(&app, pane, &id, true, ""));
        assert!(app.sync_panes().is_empty(), "wait for a snapshot taken after the stream ended");
        app.handle_app_event(AppEvent::Snapshot(Box::new(demo_snapshot())));
        assert!(matches!(&app.sync_panes()[..], [Effect::FollowLogs { .. }]));
        assert!(app.sync_panes().is_empty(), "resume once");
    }

    #[test]
    fn logs_stay_ended_while_container_stopped() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        let pane = start_loner_logs(&mut app);
        let stopped = with_state(&app, "loner", ContainerState::Exited);
        app.handle_app_event(AppEvent::Snapshot(Box::new(stopped.clone())));
        app.handle_app_event(ended(&app, pane, &id, true, ""));
        app.handle_app_event(AppEvent::Snapshot(Box::new(stopped)));
        assert!(app.sync_panes().is_empty());
    }

    #[test]
    fn an_end_reported_by_a_replaced_stream_is_ignored() {
        // `f` reloads the same view (same key). The old stream's task is
        // aborted, but it may already have reported its end.
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        let pane = start_loner_logs(&mut app);
        let stale = ended(&app, pane, &id, true, "2026-11-16T10:00:00Z");
        app.focus = Focus::Panes;
        app.handle_key(crate::app::tests::key(KeyCode::Char('f')));
        assert!(matches!(&app.sync_panes()[..], [Effect::StartLogs { .. }]));
        install_term(&mut app, pane);
        app.handle_app_event(stale);
        app.handle_app_event(AppEvent::Snapshot(Box::new(demo_snapshot())));
        let fx = app.sync_panes();
        assert!(fx.is_empty(), "the new stream is still running; a second one would repeat its lines: {fx:?}");
    }

    #[test]
    fn logs_do_not_resume_after_stream_error() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        let pane = start_loner_logs(&mut app);
        app.handle_app_event(ended(&app, pane, &id, false, ""));
        app.handle_app_event(AppEvent::Snapshot(Box::new(demo_snapshot())));
        assert!(app.sync_panes().is_empty());
    }

    fn starts_full(fx: &[Effect]) -> Option<bool> {
        match fx {
            [Effect::StartLogs { full, .. }] => Some(*full),
            _ => None,
        }
    }

    #[test]
    fn f_reloads_logs_with_full_history_until_view_changes() {
        let mut app = loaded_app();
        let pane = start_loner_logs(&mut app);
        assert!(app.pane_menu(pane).items.iter().any(|i| i.action == Action::FullLogs(pane)));
        app.focus = Focus::Panes;
        app.handle_key(key(KeyCode::Char('f')));
        assert_eq!(starts_full(&app.sync_panes()), Some(true));
        assert!(app.sync_panes().is_empty(), "reload once");
        assert!(!app.pane_menu(pane).items.iter().any(|i| i.action == Action::FullLogs(pane)));
        // A restart resumes the same full view...
        install_term(&mut app, pane);
        app.handle_app_event(ended(&app, pane, &container_id(&app, "loner"), true, ""));
        app.handle_app_event(AppEvent::Snapshot(Box::new(demo_snapshot())));
        assert!(matches!(&app.sync_panes()[..], [Effect::FollowLogs { .. }]));
        assert!(app.panes[&pane].full_logs);
        // ...but not switching to another tab and back.
        app.cycle_tab(pane, 1);
        app.sync_panes();
        app.cycle_tab(pane, -1);
        assert_eq!(starts_full(&app.sync_panes()), Some(false));
    }

    #[test]
    fn full_history_only_for_logs_tab() {
        let mut app = loaded_app();
        let pane = start_loner_logs(&mut app);
        app.set_tab(pane, Tab::Env);
        app.sync_panes();
        assert!(!app.pane_menu(pane).items.iter().any(|i| i.action == Action::FullLogs(pane)));
        app.perform(Action::FullLogs(pane));
        assert!(app.sync_panes().is_empty());
        assert!(app.toast.is_some());
    }

    #[test]
    fn full_project_logs_tail_all() {
        let mut app = loaded_app();
        app.select_key(&NodeKey::Project("demo".into()));
        app.sync_panes();
        app.perform(Action::FullLogs(app.layout.focus));
        let fx = app.sync_panes();
        let [Effect::SpawnTerm { argv, .. }] = &fx[..] else { panic!("{fx:?}") };
        assert!(argv.contains(&"--tail=all".to_string()), "{argv:?}");
    }

    #[test]
    fn project_logs_spawn_compose() {
        let mut app = loaded_app();
        app.select_key(&NodeKey::Project("demo".into()));
        let effects = app.sync_panes();
        let Some(Effect::SpawnTerm { argv, key: Some(_), .. }) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(argv.join(" "), "docker compose -p demo logs --tail=300 --follow");
    }

    #[test]
    fn top_requires_running_container() {
        let mut app = loaded_app();
        app.select_key(&NodeKey::Container(container_id(&app, "exited")));
        app.set_tab(app.layout.focus, Tab::Top);
        assert!(app.sync_panes().is_empty());
        assert!(screen(&draw(&mut app, 100, 20)).contains("container is not running"));
        let id = container_id(&app, "loner");
        app.select_key(&NodeKey::Container(id.clone()));
        app.set_tab(app.layout.focus, Tab::Top);
        let fx = app.sync_panes();
        let [Effect::Load { spec: LoadSpec::Top { containers, labelled: false }, key, pane }] = &fx[..] else {
            panic!("{fx:?}")
        };
        assert_eq!(*containers, [("loner".to_string(), id)]);
        // Top refreshes every tick once the previous result arrived.
        app.handle_app_event(AppEvent::Doc { pane: *pane, key: key.clone(), lines: vec![] });
        app.tick += 1;
        assert_eq!(app.sync_panes().len(), 1);
    }

    #[test]
    fn slow_top_result_is_kept_and_requests_dont_pile_up() {
        let mut app = loaded_app();
        select_loner(&mut app);
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Top);
        let fx = app.sync_panes();
        let Some(Effect::Load { key, .. }) = fx.first().cloned() else { panic!("{fx:?}") };
        // Seconds pass while docker top is slow: no new requests.
        app.tick += 3;
        assert!(app.sync_panes().is_empty(), "request still in flight");
        // The late result is still shown.
        app.handle_app_event(AppEvent::Doc { pane, key: key.clone(), lines: vec![Line::raw("PID  CMD")] });
        assert!(screen(&draw(&mut app, 100, 20)).contains("PID  CMD"));
        // Next tick refreshes with the same key.
        app.tick += 1;
        let fx = app.sync_panes();
        assert!(matches!(&fx[..], [Effect::Load { key: k, .. }] if *k == key), "{fx:?}");
    }

    #[test]
    fn snapshot_requests_stats_for_running() {
        let mut app = App::new("x");
        let effects = app.handle_app_event(AppEvent::Snapshot(Box::new(demo_snapshot())));
        let Some(Effect::SyncStats(ids)) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(ids.len(), 3, "exited container excluded");
    }

    #[test]
    fn stats_tab_renders_graphs() {
        let t = crate::ui::theme::Theme::default();
        let mut h = StatsHistory::default();
        let now = std::time::Instant::now();
        h.push(
            now,
            StatsSample {
                cpu_total: 10,
                precpu_total: 0,
                system: 100,
                presystem: 0,
                online_cpus: 1,
                pids: 3,
                ..Default::default()
            },
        );
        let text: Vec<String> = stats_lines(Some(&h), true, 60, &t)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(text[0].starts_with("CPU (%): 10.00"), "{text:?}");
        assert!(text.iter().any(|l| l.contains("PIDs:") && l.contains('3')));
        assert_eq!(stats_lines(None, false, 60, &t).len(), 1);
    }
}

mod windows {
    use super::*;
    use crate::ui::layout::SplitDir;
    use crossterm::event::Event;

    fn click_pane(app: &mut App, id: PaneId) {
        draw(app, 120, 30);
        let (_, _, inner) = *app.hits.panes.iter().find(|(p, _, _)| *p == id).unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), inner.x + 2, inner.y + 3));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), inner.x + 2, inner.y + 3));
        app.sync_panes();
    }

    /// Pick `k` in the sidebar, as a user would.
    fn pick(app: &mut App, k: &NodeKey) {
        app.focus = Focus::Sidebar;
        app.select_key(k);
        app.sync_panes();
    }

    #[test]
    fn focusing_a_pane_whose_item_has_no_row_settles() {
        let mut app = loaded_app();
        let loner = select_loner(&mut app);
        draw(&mut app, 120, 30);
        app.sync_panes();
        let first = app.layout.focus;
        prefix(&mut app, 'v');
        app.sync_panes();
        // The filter hides the first pane's item; focusing that pane can't select it.
        app.sidebar.filter = "web".into();
        app.rebuild_rows();
        click_pane(&mut app, first);
        assert!(matches!(&app.panes[&first].kind, PaneKind::Pinned(k) if *k == loner));
        app.sync_panes();
        app.dirty = false;
        app.sync_panes();
        assert!(!app.dirty, "nothing changed, so no redraw");
    }

    #[test]
    fn closing_the_last_pane_with_the_sidebar_hidden_keeps_focus_on_panes() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        prefix(&mut app, 'b');
        let id = app.layout.focus;
        app.panes.insert(id, Pane::new(PaneKind::Terminal { title: "t".into() }));
        prefix(&mut app, 'x');
        assert!(matches!(app.panes[&id].kind, PaneKind::Inspector));
        assert_eq!(app.focus, Focus::Panes, "the sidebar is hidden");
    }

    #[test]
    fn prefix_split_creates_pinned_pane() {
        let mut app = loaded_app();
        let k = select_loner(&mut app);
        draw(&mut app, 120, 30);
        prefix(&mut app, 'v');
        assert_eq!(app.layout.panes().len(), 2);
        let new = app.layout.focus;
        assert!(matches!(&app.panes[&new].kind, PaneKind::Pinned(p) if *p == k));
        assert_eq!(app.focus, Focus::Panes);
        let s = screen(&draw(&mut app, 120, 30));
        assert!(s.contains("◆ loner"), "{s}");
        prefix(&mut app, '-');
        assert_eq!(app.layout.splits(app.hits.pane_area).iter().filter(|s| s.dir == SplitDir::Vertical).count(), 1);
    }

    #[test]
    fn sidebar_drives_the_focused_pane() {
        let mut app = loaded_app();
        let loner = select_loner(&mut app);
        draw(&mut app, 120, 30);
        app.sync_panes();
        let first = app.layout.focus;
        prefix(&mut app, 'v');
        let second = app.layout.focus;
        app.sync_panes();
        // Picking an item changes the pane last focused, not the first one.
        let web = NodeKey::Service("demo".into(), "web".into());
        pick(&mut app, &web);
        assert_eq!(app.pane_target(second), Some(web.clone()));
        assert_eq!(app.pane_target(first), Some(loner.clone()));
        let s = screen(&draw(&mut app, 120, 30));
        assert!(s.contains("◆ loner") && !s.contains("◆ demo › web"), "only the other pane is pinned: {s}");
        // Focusing the first pane highlights its item and makes it the one driven.
        click_pane(&mut app, first);
        assert_eq!(app.layout.focus, first);
        assert_eq!(app.selected(), Some(&loner));
        let db = NodeKey::Service("demo".into(), "db".into());
        pick(&mut app, &db);
        assert_eq!(app.pane_target(first), Some(db));
        assert_eq!(app.pane_target(second), Some(web));
    }

    #[test]
    fn sidebar_drives_the_last_content_pane_while_a_terminal_has_focus() {
        let mut app = loaded_app();
        select_loner(&mut app);
        draw(&mut app, 120, 30);
        prefix(&mut app, 'v');
        let second = app.layout.focus;
        app.sync_panes();
        app.perform(Action::RunInPane { title: "sh".into(), argv: vec!["true".into()] });
        assert_ne!(app.layout.focus, second, "the terminal has focus");
        let web = NodeKey::Service("demo".into(), "web".into());
        pick(&mut app, &web);
        assert_eq!(app.pane_target(second), Some(web));
    }

    #[test]
    fn focusing_a_pane_reveals_its_item_in_the_sidebar() {
        let mut app = loaded_app();
        let loner = select_loner(&mut app);
        draw(&mut app, 120, 30);
        let first = app.layout.focus;
        prefix(&mut app, 'v');
        app.sync_panes();
        let web = NodeKey::Service("demo".into(), "web".into());
        pick(&mut app, &web);
        // Collapse the section holding the other pane's item, then focus it.
        app.sidebar.set_expanded(&NodeKey::Section(Section::Containers), false);
        app.rebuild_rows();
        assert!(!app.rows().iter().any(|r| r.key == loner));
        click_pane(&mut app, first);
        assert_eq!(app.selected(), Some(&loner));
        assert!(app.rows().iter().any(|r| r.key == loner), "section expanded again");
    }

    #[test]
    fn close_last_pane_restores_inspector() {
        let mut app = loaded_app();
        select_loner(&mut app);
        draw(&mut app, 120, 30);
        prefix(&mut app, 'v');
        let pinned = app.layout.focus;
        prefix(&mut app, 'h');
        let first = app.layout.focus;
        prefix(&mut app, 'x');
        assert_eq!(app.layout.panes(), vec![pinned]);
        prefix(&mut app, 'x');
        assert_eq!(app.layout.panes(), vec![pinned]);
        assert!(matches!(app.panes[&pinned].kind, PaneKind::Inspector), "last pane becomes inspector");
        assert!(!app.panes.contains_key(&first));
    }

    #[test]
    fn enter_from_sidebar_focuses_inspector_not_shell() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        let inspector = app.layout.focus;
        let shell = cat_pane(&mut app);
        assert_eq!(app.layout.focus, shell);
        app.focus = Focus::Sidebar;
        app.handle_key(key(KeyCode::Enter));
        assert_eq!((app.focus, app.layout.focus), (Focus::Panes, inspector));
    }

    #[test]
    fn drag_border_updates_ratio() {
        let mut app = loaded_app();
        select_loner(&mut app);
        draw(&mut app, 120, 30);
        prefix(&mut app, 'v');
        draw(&mut app, 120, 30);
        let split = app.hits.splits[0].clone();
        let y = split.area.y + 3;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), split.pos, y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), split.pos + 10, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), split.pos + 10, y));
        draw(&mut app, 120, 30);
        assert_eq!(app.hits.splits[0].pos, split.pos + 10);
        // Dragging far beyond clamps at 90%.
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), split.pos + 10, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 500, y));
        draw(&mut app, 120, 30);
        let s = &app.hits.splits[0];
        assert_eq!(s.pos, s.area.x + (s.area.width as f32 * 0.9).round() as u16);
    }

    #[test]
    fn resize_aborts_drag() {
        let mut app = loaded_app();
        draw(&mut app, 100, 20);
        let div = app.hits.sidebar_divider.unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), div.x, 5));
        assert!(app.drag.is_some());
        app.handle_event(Event::Resize(60, 15));
        assert!(app.drag.is_none());
    }

    #[test]
    fn exec_opens_terminal_pane_with_login_shell() {
        let mut app = loaded_app();
        select_loner(&mut app);
        draw(&mut app, 120, 30);
        let effects = app.handle_key(key(KeyCode::Char('E')));
        let Some(Effect::SpawnTerm { argv, key: None, pane }) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(&argv[..3], &["docker", "exec", "-it"]);
        assert!(argv.last().unwrap().contains("/etc/passwd"));
        assert!(matches!(&app.panes[pane].kind, PaneKind::Terminal { title } if title == "exec loner"));
        // Exited containers can't be exec'd into.
        assert!(app.exec_shell(&NodeKey::Container(container_id(&app, "exited"))).is_empty());
        assert!(app.toast.is_some());
    }

    #[test]
    fn m_opens_logs_pane() {
        let mut app = loaded_app();
        select_loner(&mut app);
        draw(&mut app, 120, 30);
        app.handle_key(key(KeyCode::Char('m')));
        let id = app.layout.focus;
        assert_eq!(app.pane_tab(id), Some(Tab::Logs));
        assert!(app.sync_panes().iter().any(|e| matches!(e, Effect::StartLogs { pane, .. } if *pane == id)));
    }

    #[test]
    fn keys_forwarded_to_terminal() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        let id = cat_pane(&mut app);
        // 'q' goes to cat, not quit; ctrl-c goes to cat too.
        assert!(app.handle_key(key(KeyCode::Char('q'))).is_empty());
        app.handle_key(key(KeyCode::Enter));
        assert!(eventually(|| term_text(&app, id).lines().filter(|l| l.starts_with('q')).count() >= 2));
        assert!(app.handle_key(ctrl('c')).is_empty());
        // The prefix still works.
        prefix(&mut app, 'h');
        assert_ne!(app.layout.focus, id);
    }

    #[test]
    fn stale_exit_does_not_mark_new_surface() {
        let mut app = loaded_app();
        let pane = app.layout.focus;
        let t = TermSurface::stream(20, 3, Arc::new(|| {})).unwrap();
        let sid = t.id();
        app.panes.get_mut(&pane).unwrap().content = Content::Term(t);
        app.handle_app_event(AppEvent::TermExited { pane, surface: sid + 1000, code: Some(1) });
        let Content::Term(t) = &app.panes[&pane].content else { panic!() };
        assert!(!t.viewport_text().join("").contains("ended"), "stale exit ignored");
        app.handle_app_event(AppEvent::TermExited { pane, surface: sid, code: Some(1) });
        let Content::Term(t) = &app.panes[&pane].content else { panic!() };
        assert!(t.viewport_text().join("").contains("code 1"));
    }
}

mod overlays {
    use super::*;
    use crate::app::overlay::{Menu, MenuItem};

    fn menu_app() -> App {
        let mut app = loaded_app();
        draw(&mut app, 100, 30);
        app.overlay = Some(Overlay::Menu(Menu::new(
            "t",
            vec![
                MenuItem::new("one", Action::Flash("one".into())),
                MenuItem::new("quit", Action::Copy("q".into())).key("Q"),
                MenuItem::new("three", Action::Flash("one".into())),
            ],
        )));
        draw(&mut app, 100, 30);
        app
    }

    #[test]
    fn menu_shortcuts_ignore_ctrl_chords() {
        let mut app = menu_app();
        let ctrl_q = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        assert!(app.handle_key(ctrl_q).is_empty());
        assert!(app.overlay.is_some());
    }

    #[test]
    fn background_errors_are_shown_one_after_another() {
        let mut app = loaded_app();
        for e in ["one", "two", "three"] {
            app.handle_app_event(AppEvent::OpDone { op: DockerOp::Start("x".into()), result: Err(e.into()) });
        }
        for e in ["one", "two", "three"] {
            assert!(matches!(&app.overlay, Some(Overlay::Message { body, .. }) if body == e), "{:?}", app.overlay);
            app.handle_key(key(KeyCode::Esc));
        }
        assert!(app.overlay.is_none());
    }

    #[test]
    fn async_popup_waits_until_shell_loses_focus() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        cat_pane(&mut app);
        app.handle_app_event(AppEvent::OpDone {
            op: DockerOp::RemoveContainer { id: container_id(&app, "loner"), force: false, volumes: false },
            result: Err("container is running: stop the container before removing or force remove".into()),
        });
        assert!(app.overlay.is_none(), "no modal over a focused shell");
        assert!(screen(&draw(&mut app, 120, 30)).contains("⚠"), "status bar flags the pending message");
        // Typing goes to the shell, not to the confirmation.
        assert!(app.handle_key(key(KeyCode::Char('y'))).is_empty());
        assert!(app.overlay.is_none());
        // Leaving the shell shows it.
        prefix(&mut app, 'h');
        assert!(matches!(app.overlay, Some(Overlay::Confirm { .. })), "{:?}", app.overlay);
    }

    #[test]
    fn menu_keyboard_navigation() {
        let mut app = menu_app();
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        let Some(Overlay::Menu(m)) = &app.overlay else { panic!() };
        assert_eq!(m.highlighted, 0, "wraps");
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), vec![Effect::Copy("q".into())]);
        assert!(app.overlay.is_none());
        // Shortcut key activates directly.
        let mut app = menu_app();
        assert_eq!(app.handle_key(key(KeyCode::Char('Q'))), vec![Effect::Copy("q".into())]);
    }

    #[test]
    fn menu_mouse_hover_and_click() {
        let mut app = menu_app();
        let (r, _) = app.overlay_hits.rows[2];
        app.handle_mouse(mouse(MouseEventKind::Moved, r.x + 1, r.y));
        let Some(Overlay::Menu(m)) = &app.overlay else { panic!() };
        assert_eq!(m.highlighted, 2);
        let (r, _) = app.overlay_hits.rows[1];
        assert_eq!(
            app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y)),
            vec![Effect::Copy("q".into())]
        );
    }

    #[test]
    fn click_outside_closes() {
        let mut app = menu_app();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        assert!(app.overlay.is_none());
        // ...and the click didn't fall through to the sidebar.
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn confirm_y_runs_action() {
        let mut app = loaded_app();
        app.overlay = Some(Overlay::Confirm {
            title: "Quit?".into(),
            body: "sure".into(),
            danger: true,
            action: Action::Copy("q".into()),
        });
        let s = screen(&draw(&mut app, 100, 30));
        assert!(s.contains("↵ confirm") && s.contains("esc cancel"), "{s}");
        assert!(app.handle_key(key(KeyCode::Char('n'))).is_empty());
        assert!(app.overlay.is_none());
        app.overlay = Some(Overlay::Confirm {
            title: "Quit?".into(),
            body: "sure".into(),
            danger: true,
            action: Action::Copy("q".into()),
        });
        assert_eq!(app.handle_key(key(KeyCode::Char('y'))), vec![Effect::Copy("q".into())]);
    }

    #[test]
    fn context_menu_clamped_on_screen() {
        let mut app = loaded_app();
        let buf = draw(&mut app, 100, 30);
        let y = (0..30).find(|y| row_text(&buf, *y).contains("loner")).unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 5, y));
        let Some(Overlay::Menu(m)) = &app.overlay else { panic!() };
        assert!(m.items.iter().any(|i| i.label == "Exec shell"));
        assert_eq!(app.selected().map(|k| app.target_label(k)), Some("loner".into()), "right-click selects");
        // Right-click on a pane near the bottom-right corner: pane menu, kept on screen.
        app.overlay = None;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Right), 98, 27));
        let Some(Overlay::Menu(m)) = &app.overlay else { panic!() };
        assert!(m.items.iter().any(|i| i.label == "Split right"));
        draw(&mut app, 100, 30);
        let p = app.overlay_hits.panel;
        assert!(p.right() <= 100 && p.bottom() <= 30 && p.width > 10, "{p:?}");
    }

    #[test]
    fn help_filters() {
        let mut app = loaded_app();
        draw(&mut app, 100, 40);
        app.handle_key(key(KeyCode::Char('?')));
        for c in "zoom".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        let s = screen(&draw(&mut app, 100, 40));
        assert!(s.contains("zoom") && !s.contains("restart"), "{s}");
        app.handle_key(key(KeyCode::Esc));
        assert!(app.overlay.is_some(), "first esc clears the filter");
        app.handle_key(key(KeyCode::Esc));
        assert!(app.overlay.is_none());
    }
}

mod docker_actions {
    use super::*;

    fn local_app() -> App {
        let mut app = App::new("tcp://test");
        let mut snap = demo_snapshot();
        snap.projects[0].local = true;
        app.compose_files = vec!["compose.yml".into()];
        app.handle_app_event(AppEvent::Snapshot(Box::new(snap)));
        app
    }

    fn has_browser_item(app: &App, key: &NodeKey) -> bool {
        app.item_menu(key).items.iter().any(|i| i.label == "Open in browser")
    }

    #[test]
    fn compose_actions_without_files_explain_instead_of_asking() {
        // The demo project's containers carry no compose file labels.
        let mut app = loaded_app();
        let project = NodeKey::Project("demo".into());
        app.select_key(&project);
        app.handle_key(key(KeyCode::Char('U')));
        assert!(matches!(&app.overlay, Some(Overlay::Message { error: true, .. })), "{:?}", app.overlay);
        let labels: Vec<String> = app.item_menu(&project).items.into_iter().map(|i| i.label).collect();
        assert!(!labels.contains(&"Up".to_string()), "an item that can only fail is left out: {labels:?}");
    }

    #[test]
    fn ctrl_letters_do_not_run_item_actions() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        app.select_key(&NodeKey::Container(id.clone()));
        draw(&mut app, 120, 30);
        // ctrl+r/s/p are find keys in logs panes; ctrl+d scrolls. None may restart, stop, pause or remove.
        for c in ['r', 's', 'p', 'd'] {
            let fx = app.handle_key(ctrl(c));
            assert!(fx.is_empty() && app.overlay.is_none(), "sidebar ctrl+{c}: {fx:?} {:?}", app.overlay);
        }
        app.focus_inspector();
        for c in ['r', 's', 'p', 'd'] {
            let fx = app.handle_key(ctrl(c));
            assert!(fx.is_empty() && app.overlay.is_none(), "pane ctrl+{c}: {fx:?} {:?}", app.overlay);
        }
        // Plain and shifted letters still act.
        assert_eq!(app.handle_key(key(KeyCode::Char('r'))), vec![Effect::Docker(DockerOp::Restart(id.clone()))]);
        let shift_s = KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT);
        assert_eq!(app.handle_key(shift_s), vec![Effect::Docker(DockerOp::Start(id))]);
    }

    #[test]
    fn open_in_browser_only_offered_with_a_published_port() {
        let mut app = loaded_app();
        let k = NodeKey::Container(container_id(&app, "loner"));
        let web = NodeKey::Service("demo".into(), "web".into());
        assert!(!has_browser_item(&app, &k), "loner publishes nothing");
        assert!(!has_browser_item(&app, &web));
        // The w key still explains why nothing opened.
        app.select_key(&k);
        app.handle_key(key(KeyCode::Char('w')));
        assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("no published ports")));

        let mut snap = demo_snapshot();
        for c in snap.containers.iter_mut().filter(|c| c.name == "loner" || c.name == "demo-web-1") {
            c.ports = vec![crate::docker::model::Port {
                ip: Some("0.0.0.0".into()),
                private: 80,
                public: Some(8080),
                proto: "tcp".into(),
            }];
        }
        app.handle_app_event(AppEvent::Snapshot(Box::new(snap)));
        assert!(has_browser_item(&app, &k));
        assert!(has_browser_item(&app, &web));
    }

    #[test]
    fn stop_requires_confirm() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        app.select_key(&NodeKey::Container(id.clone()));
        assert!(app.handle_key(key(KeyCode::Char('s'))).is_empty());
        assert!(matches!(&app.overlay, Some(Overlay::Confirm { .. })));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), vec![Effect::Docker(DockerOp::Stop(id.clone()))]);
        assert_eq!(app.pending_ops, vec!["stopping"]);
        // Restart has no confirmation (lazydocker).
        assert_eq!(app.handle_key(key(KeyCode::Char('r'))), vec![Effect::Docker(DockerOp::Restart(id))]);
    }

    #[test]
    fn remove_menu_items() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        app.select_key(&NodeKey::Container(id.clone()));
        app.handle_key(key(KeyCode::Char('d')));
        let Some(Overlay::Menu(m)) = &app.overlay else { panic!("{:?}", app.overlay) };
        let labels: Vec<_> = m.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["remove", "remove with volumes"]);
        assert!(m.items.iter().all(|i| i.danger));
        assert_eq!(m.items[1].action, Action::Docker(DockerOp::RemoveContainer { id, force: false, volumes: true }));
    }

    #[test]
    fn running_container_remove_offers_force() {
        let mut app = loaded_app();
        let id = container_id(&app, "loner");
        let op = DockerOp::RemoveContainer { id: id.clone(), force: false, volumes: false };
        let fx = app.handle_app_event(AppEvent::OpDone {
            op,
            result: Err(
                "cannot remove container: container is running: stop the container before removing or force remove"
                    .into(),
            ),
        });
        assert_eq!(fx, vec![Effect::RefreshNow]);
        let Some(Overlay::Confirm { action, danger: true, .. }) = &app.overlay else { panic!("{:?}", app.overlay) };
        assert_eq!(*action, Action::Docker(DockerOp::RemoveContainer { id, force: true, volumes: false }));
        // Other errors show an error popup.
        app.overlay = None;
        app.handle_app_event(AppEvent::OpDone { op: DockerOp::Start("x".into()), result: Err("boom".into()) });
        assert!(matches!(&app.overlay, Some(Overlay::Message { error: true, body, .. }) if body == "boom"));
    }

    #[test]
    fn compose_cmd_includes_project() {
        let mut app = local_app();
        app.select_key(&NodeKey::Service("demo".into(), "web".into()));
        let fx = app.handle_key(key(KeyCode::Char('u')));
        assert_eq!(
            fx,
            vec![Effect::Docker(DockerOp::Cli(
                ["docker", "compose", "-f", "compose.yml", "-p", "demo", "up", "-d", "web"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            ))]
        );
    }

    #[test]
    fn non_local_project_uses_labels_or_errors() {
        let mut app = loaded_app();
        app.select_key(&NodeKey::Service("demo".into(), "web".into()));
        // No compose files known: stop falls back to the container (lazydocker).
        app.handle_key(key(KeyCode::Char('s')));
        let Some(Overlay::Confirm { action: Action::Docker(DockerOp::Stop(id)), .. }) = &app.overlay else {
            panic!("{:?}", app.overlay)
        };
        assert!(id.starts_with("demo-web-1"));
        app.overlay = None;
        // ...and compose-file operations explain why they can't run.
        app.handle_key(key(KeyCode::Char('u')));
        assert!(matches!(&app.overlay, Some(Overlay::Message { error: true, .. })));
        // With labels pointing at the files, compose works.
        let mut snap = demo_snapshot();
        for c in &mut snap.containers {
            c.labels.insert("com.docker.compose.project.config_files".into(), "/srv/demo/compose.yml".into());
            c.labels.insert("com.docker.compose.project.working_dir".into(), "/srv/demo".into());
        }
        app.handle_app_event(AppEvent::Snapshot(Box::new(snap)));
        app.overlay = None;
        let fx = app.handle_key(key(KeyCode::Char('u')));
        let Some(Effect::Docker(DockerOp::Cli(argv))) = fx.first() else { panic!("{fx:?}") };
        assert_eq!(
            argv.join(" "),
            "docker compose -f /srv/demo/compose.yml --project-directory /srv/demo -p demo up -d web"
        );
    }

    #[test]
    fn bulk_service_menu_matches_lazydocker() {
        let app = local_app();
        let m = app.bulk_services_menu("demo");
        let labels: Vec<_> = m.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(
            labels,
            vec![
                "up",
                "up (attached)",
                "stop",
                "pull",
                "build",
                "down",
                "down with volumes",
                "down and remove images",
                "down with volumes and remove images"
            ]
        );
        assert!(matches!(&m.items[1].action, Action::RunInPane { .. }), "attached commands run in a pane");
    }

    #[test]
    fn prune_asks_first() {
        let mut app = loaded_app();
        app.select_key(&NodeKey::Section(Section::Images));
        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(key(KeyCode::Enter));
        let Some(Overlay::Confirm { action, danger: true, .. }) = &app.overlay else { panic!("{:?}", app.overlay) };
        assert_eq!(*action, Action::Docker(DockerOp::PruneImages));
    }

    #[test]
    fn context_menu_lists_docker_actions() {
        let app = loaded_app();
        let m = app.item_menu(&NodeKey::Container(container_id(&app, "loner")));
        let labels: Vec<_> = m.items.iter().map(|i| i.label.as_str()).collect();
        for want in ["Stop", "Restart", "Remove…", "Bulk…", "Exec shell"] {
            assert!(labels.contains(&want), "{labels:?}");
        }
    }
}

mod select {
    use super::*;

    const TEXT: [&str; 3] = ["alpha beta gamma", "second line here", "see http://x.io/p ok"];

    #[test]
    fn drag_selects_and_copies() {
        let (mut app, _, r) = doc_app(TEXT);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 6, r.y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 5, r.y + 1));
        let fx = app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 5, r.y + 1));
        assert_eq!(fx, vec![Effect::Copy("beta gamma\nsecond".into())]);
        // Highlight is painted.
        let buf = draw(&mut app, 100, 20);
        assert_eq!(buf[(r.x + 6, r.y)].bg, app.theme.surface1);
        assert_ne!(buf[(r.x + 5, r.y)].bg, app.theme.surface1);
        // Any key clears it.
        app.handle_key(key(KeyCode::Down));
        assert!(app.selection.is_none());
    }

    #[test]
    fn click_without_drag_copies_nothing() {
        let (mut app, pane, r) = doc_app(TEXT);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
        let fx = app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y));
        assert!(fx.is_empty());
        assert!(app.selection.is_none());
        assert_eq!((app.focus, app.layout.focus), (Focus::Panes, pane), "click focuses the pane");
    }

    #[test]
    fn double_click_copies_word_or_url() {
        let (mut app, _, r) = doc_app(TEXT);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 8, r.y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 8, r.y));
        let fx = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 8, r.y));
        assert_eq!(fx, vec![Effect::Copy("beta".into())]);
        app.last_click = None;
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 10, r.y + 2));
        let fx = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 10, r.y + 2));
        assert_eq!(fx, vec![Effect::Copy("http://x.io/p".into())]);
    }

    #[test]
    fn drag_past_edge_autoscrolls() {
        let (mut app, pane, r) = doc_app(TEXT);
        app.panes.get_mut(&pane).unwrap().content =
            Content::Doc(crate::doc::DocSurface::new((0..200).map(|i| Line::raw(format!("row {i}"))).collect()));
        draw(&mut app, 100, 20);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x, r.y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 3, r.bottom() + 2));
        let Content::Doc(d) = &app.panes[&pane].content else { panic!() };
        assert!(d.scroll > 0);
        let fx = app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 3, r.bottom() + 2));
        let Some(Effect::Copy(text)) = fx.first() else { panic!("{fx:?}") };
        assert!(text.starts_with("row 0\nrow 1\n") && text.lines().count() > r.height as usize, "{text}");
    }

    #[test]
    fn terminal_text_selection() {
        let (mut app, pane, r) = doc_app(TEXT);
        let t = TermSurface::stream(r.width, r.height, Arc::new(|| {})).unwrap();
        t.feeder(true).feed(b"hello terminal\nworld\n");
        app.panes.get_mut(&pane).unwrap().content = Content::Term(t);
        draw(&mut app, 100, 20);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 6, r.y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 4, r.y + 1));
        let fx = app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 4, r.y + 1));
        assert_eq!(fx, vec![Effect::Copy("terminal\nworld".into())]);
    }

    #[test]
    fn l_scrolls_doc_right_and_selection_maps_columns() {
        let (mut app, pane, _) = doc_app([format!("{}TARGET{}", "-".repeat(150), "-".repeat(20))]);
        app.focus = Focus::Panes;
        for _ in 0..30 {
            app.handle_key(key(KeyCode::Char('l')));
        }
        let s = screen(&draw(&mut app, 100, 20));
        assert!(s.contains("TARGET"), "{s}");
        let r = app.hits.content.iter().find(|(p, _)| *p == pane).unwrap().1;
        let y = r.y;
        let x = (0..r.width).find(|x| draw(&mut app, 100, 20)[(r.x + x, y)].symbol() == "T").unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + x, y));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + x, y));
        let fx = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + x, y));
        let Some(Effect::Copy(word)) = fx.first() else { panic!("{fx:?}") };
        assert!(word.contains("TARGET"), "{word}");
    }
}

mod modes {
    use super::*;
    use crossterm::event::Event;

    #[test]
    fn copy_mode_yank_line() {
        let (mut app, _, _) = doc_app(["first line", "second line", "third"]);
        app.focus = Focus::Panes;
        prefix(&mut app, '[');
        assert_eq!(app.mode, Mode::Copy);
        app.handle_key(key(KeyCode::Char('g')));
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char('V')));
        app.handle_key(key(KeyCode::Char('j')));
        let s = screen(&draw(&mut app, 100, 20));
        assert!(s.contains(" COPY "), "{s}");
        let fx = app.handle_key(key(KeyCode::Char('y')));
        assert_eq!(fx, vec![Effect::Copy("second line\nthird".into())]);
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn copy_mode_charwise_with_word_motion() {
        let (mut app, _, _) = doc_app(["alpha beta gamma"]);
        app.focus = Focus::Panes;
        prefix(&mut app, '[');
        app.handle_key(key(KeyCode::Char('g')));
        app.handle_key(key(KeyCode::Char('w')));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('e')));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), vec![Effect::Copy("beta".into())]);
    }

    #[test]
    fn copy_mode_scrolls_to_cursor_and_esc_exits() {
        let (mut app, pane, _) = doc_app((0..100).map(|i| format!("line {i}")));
        app.focus = Focus::Panes;
        prefix(&mut app, '[');
        app.handle_key(key(KeyCode::Char('G')));
        let Content::Doc(d) = &app.panes[&pane].content else { panic!() };
        assert!(d.scroll > 50, "{}", d.scroll);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn resize_mode_adjusts_ratio() {
        let mut app = loaded_app();
        select_loner(&mut app);
        draw(&mut app, 120, 30);
        prefix(&mut app, 'v');
        draw(&mut app, 120, 30);
        let before = app.hits.splits[0].pos;
        prefix(&mut app, 'r');
        assert_eq!(app.mode, Mode::Resize);
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Char('h')));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Normal);
        draw(&mut app, 120, 30);
        assert!(app.hits.splits[0].pos < before, "{} !< {before}", app.hits.splits[0].pos);
    }

    #[test]
    fn click_leaves_filter_resize_and_copy_modes() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        let (_, r, _) = app.hits.panes[0];
        for mode in [Mode::Filter, Mode::Resize, Mode::Copy, Mode::Prefix] {
            app.mode = mode;
            if mode == Mode::Filter {
                app.sidebar.filter = "lon".into();
            }
            app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 3, r.y + 3));
            app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 3, r.y + 3));
            assert_eq!(app.mode, Mode::Normal, "{mode:?}");
        }
        assert_eq!(app.sidebar.filter, "lon", "filter is committed, not cleared");
        assert!(app.copy.is_none());
    }

    #[test]
    fn typing_after_click_reaches_shell_not_filter() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        let id = cat_pane(&mut app);
        draw(&mut app, 120, 30);
        app.focus = Focus::Sidebar;
        app.handle_key(key(KeyCode::Char('/')));
        let (_, r) = *app.hits.content.iter().find(|(p, _)| *p == id).unwrap();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y + 1));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 1, r.y + 1));
        app.handle_key(key(KeyCode::Char('z')));
        assert!(app.sidebar.filter.is_empty(), "{:?}", app.sidebar.filter);
        assert!(eventually(|| term_text(&app, id).contains('z')), "{}", term_text(&app, id));
    }

    #[test]
    fn paste_ignored_while_overlay_or_mode_active() {
        let mut app = loaded_app();
        draw(&mut app, 120, 30);
        let id = cat_pane(&mut app);
        app.perform(Action::Help);
        app.handle_event(Event::Paste("PASTED-ONE".into()));
        app.overlay = None;
        app.mode = Mode::Resize;
        app.handle_event(Event::Paste("PASTED-TWO".into()));
        // A paste that does reach the shell marks when the ignored ones would have shown.
        app.mode = Mode::Normal;
        app.handle_event(Event::Paste("SENTINEL".into()));
        assert!(eventually(|| term_text(&app, id).contains("SENTINEL")), "{}", term_text(&app, id));
        assert!(!term_text(&app, id).contains("PASTED"), "{}", term_text(&app, id));
    }
}

mod find {
    use super::*;
    use crate::app::find::Step;

    /// A logs pane holding `n` lines; every tenth (3, 13, …) says ERROR.
    fn logs_app(n: usize) -> (App, PaneId, crate::term::Feeder) {
        let mut app = loaded_app();
        select_loner(&mut app);
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Logs);
        app.sync_panes();
        let t = TermSurface::stream(80, 10, Arc::new(|| {})).unwrap();
        let f = t.feeder(true);
        for i in 0..n {
            f.feed(format!("line {i} {}\n", if i % 10 == 3 { "ERROR boom" } else { "ok" }).as_bytes());
        }
        app.panes.get_mut(&pane).unwrap().content = Content::Term(t);
        app.focus = Focus::Panes;
        draw(&mut app, 100, 20);
        (app, pane, f)
    }

    fn typ(app: &mut App, s: &str) {
        for c in s.chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
    }

    fn current_line(app: &App) -> String {
        let f = app.find.as_ref().unwrap();
        let m = f.matches[f.current.unwrap()];
        app.content_row_text(f.pane, m.start.0)
    }

    #[test]
    fn slash_finds_nearest_match_and_n_steps_older() {
        let (mut app, _, _) = logs_app(100);
        app.handle_key(key(KeyCode::Char('/')));
        assert_eq!(app.mode, Mode::Find);
        typ(&mut app, "error");
        assert_eq!(app.find.as_ref().unwrap().matches.len(), 10, "smart case matches ERROR");
        assert_eq!(current_line(&app), "line 93 ERROR boom");
        assert!(screen(&draw(&mut app, 100, 20)).contains("/error▏  10/10"));
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Normal);
        for _ in 0..5 {
            app.handle_key(key(KeyCode::Char('n')));
        }
        assert_eq!(current_line(&app), "line 43 ERROR boom");
        // The view scrolled to show it.
        let row = app.find.as_ref().map(|f| f.matches[f.current.unwrap()].start.0).unwrap();
        let top = app.content_top(app.layout.focus);
        assert!(row >= top && row < top + 10, "{row} {top}");
        app.handle_key(key(KeyCode::Char('N')));
        assert_eq!(current_line(&app), "line 53 ERROR boom");
        assert!(screen(&draw(&mut app, 100, 20)).contains("/error  6/10"));
    }

    #[test]
    fn stepping_wraps_with_a_notice() {
        let (mut app, _, _) = logs_app(30);
        app.open_find();
        typ(&mut app, "boom");
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char('N')));
        assert_eq!(current_line(&app), "line 3 ERROR boom");
        assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("wrapped to the top")));
        app.step_find(Step::Older);
        assert_eq!(current_line(&app), "line 23 ERROR boom");
    }

    #[test]
    fn matches_are_highlighted() {
        let (mut app, pane, _) = logs_app(20);
        app.handle_key(ctrl('f'));
        assert_eq!(app.mode, Mode::Find, "ctrl+f opens find");
        typ(&mut app, "ok");
        let buf = draw(&mut app, 100, 20);
        let (_, area) = *app.hits.content.iter().find(|(p, _)| *p == pane).unwrap();
        let f = app.find.as_ref().unwrap();
        let top = app.content_top(pane);
        let cell = |m: crate::term::find::Match| &buf[(area.x + m.start.1 as u16, area.y + (m.start.0 - top) as u16)];
        let cur = f.matches[f.current.unwrap()];
        let other = *f.matches.iter().rev().find(|m| m.start.0 >= top && **m != cur).unwrap();
        assert_eq!(cell(cur).bg, app.theme.yellow);
        assert_eq!(cell(cur).symbol(), "o");
        assert_eq!(cell(other).bg, app.theme.surface1);
    }

    #[test]
    fn esc_clears_and_enter_repeats_last_search() {
        let (mut app, _, _) = logs_app(30);
        app.open_find();
        typ(&mut app, "boom");
        app.handle_key(key(KeyCode::Enter));
        // Esc clears the search before it would leave the pane.
        app.handle_key(key(KeyCode::Esc));
        assert!(app.find.is_none());
        assert_eq!(app.focus, Focus::Panes);
        // Enter on an empty prompt repeats the last query.
        app.open_find();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.find.as_ref().map(|f| f.query.as_str()), Some("boom"));
        // Esc while typing cancels.
        app.open_find();
        typ(&mut app, "zz");
        app.handle_key(key(KeyCode::Esc));
        assert!(app.find.is_none() && app.mode == Mode::Normal);
    }

    #[test]
    fn no_match_closes_with_notice() {
        let (mut app, _, _) = logs_app(10);
        app.open_find();
        typ(&mut app, "nothing here");
        assert!(screen(&draw(&mut app, 100, 20)).contains("no matches"));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.find.is_none());
        assert!(app.toast.as_ref().is_some_and(|(m, _)| m == "not found: nothing here"));
    }

    #[test]
    fn find_only_in_logs_and_closes_when_pane_changes() {
        let (mut app, pane, _) = logs_app(10);
        app.open_find();
        typ(&mut app, "boom");
        app.handle_key(key(KeyCode::Enter));
        app.cycle_tab(pane, 1);
        app.sync_panes();
        draw(&mut app, 100, 20);
        assert!(app.find.is_none(), "another tab drops the search");
        app.handle_key(key(KeyCode::Char('/')));
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.toast.as_ref().is_some_and(|(m, _)| m.contains("logs panes")));
    }

    #[test]
    fn clicking_away_keeps_the_query_as_enter_does() {
        let (mut app, _, _) = logs_app(30);
        app.open_find();
        typ(&mut app, "boom");
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.last_find, "boom", "`n` repeats it");
    }

    #[test]
    fn new_output_updates_matches_and_keeps_current() {
        let (mut app, pane, feeder) = logs_app(30);
        app.open_find();
        typ(&mut app, "boom");
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Char('n')));
        assert_eq!(current_line(&app), "line 13 ERROR boom");
        feeder.feed(b"later boom\n");
        app.handle_app_event(AppEvent::TermActivity(pane));
        // The runtime syncs panes after every event; that refreshes matches,
        // at most once per throttle interval.
        let refreshed = eventually(|| {
            app.sync_panes();
            app.find.as_ref().unwrap().matches.len() == 4
        });
        assert!(refreshed, "{} matches", app.find.as_ref().unwrap().matches.len());
        assert_eq!(current_line(&app), "line 13 ERROR boom");
    }

    #[test]
    fn ctrl_g_and_other_motion_keys_step_matches() {
        let (mut app, _, _) = logs_app(60);
        app.open_find();
        typ(&mut app, "boom");
        assert_eq!(current_line(&app), "line 53 ERROR boom");
        // While typing.
        app.handle_key(ctrl('g'));
        assert_eq!(current_line(&app), "line 43 ERROR boom");
        assert_eq!(app.mode, Mode::Find, "stepping keeps the prompt open");
        app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::CONTROL | KeyModifiers::SHIFT));
        assert_eq!(current_line(&app), "line 53 ERROR boom");
        app.handle_key(key(KeyCode::Enter));
        // After enter.
        let older: [KeyEvent; 4] = [ctrl('g'), key(KeyCode::F(3)), ctrl('r'), ctrl('p')];
        for (i, k) in older.into_iter().enumerate() {
            app.handle_key(k);
            assert_eq!(current_line(&app), format!("line {} ERROR boom", 43 - 10 * i), "{k:?}");
        }
        let newer: [KeyEvent; 4] = [
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL | KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::F(3), KeyModifiers::SHIFT),
            ctrl('s'),
            ctrl('n'),
        ];
        for (i, k) in newer.into_iter().enumerate() {
            app.handle_key(k);
            assert_eq!(current_line(&app), format!("line {} ERROR boom", 23 + 10 * i), "{k:?}");
        }
    }

    #[test]
    fn next_keys_repeat_the_last_search() {
        let (mut app, _, _) = logs_app(30);
        // Nothing to repeat yet: n does nothing.
        app.handle_key(key(KeyCode::Char('n')));
        assert!(app.find.is_none());
        app.open_find();
        typ(&mut app, "boom");
        app.handle_key(key(KeyCode::Enter));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.find.is_none());
        app.handle_key(ctrl('g'));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(current_line(&app), "line 23 ERROR boom");
        app.handle_key(key(KeyCode::Esc));
        app.handle_key(key(KeyCode::Char('N')));
        assert_eq!(app.find.as_ref().map(|f| f.matches.len()), Some(3));
    }

    #[test]
    fn paste_goes_into_the_query() {
        let (mut app, _, _) = logs_app(10);
        app.open_find();
        app.handle_event(crossterm::event::Event::Paste("ERROR b\nignored".into()));
        assert_eq!(app.find.as_ref().unwrap().query, "ERROR b");
        assert_eq!(app.find.as_ref().unwrap().matches.len(), 1);
    }
}

mod replicas {
    use super::*;
    use crate::app::inspect::LoadSpec;
    use crate::docker::model::tests::replica;
    use crate::docker::model::Snapshot;
    use crate::docker::stats::{StatsHistory, StatsSample};

    /// demo: web scaled to `n` (all running), plus db.
    fn snapshot(n: u32) -> Snapshot {
        let mut containers: Vec<_> = (1..=n).map(|i| replica("demo", "web", i, ContainerState::Running)).collect();
        containers.push(replica("demo", "db", 1, ContainerState::Running));
        Snapshot { projects: derive_projects(&containers, None), containers, ..Default::default() }
    }

    fn web() -> NodeKey {
        NodeKey::Service("demo".into(), "web".into())
    }

    fn app_on_web(n: u32, tab: Tab) -> (App, PaneId) {
        let mut app = App::new("tcp://test");
        app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot(n))));
        draw(&mut app, 120, 30);
        app.select_key(&web());
        let pane = app.layout.focus;
        app.set_tab(pane, tab);
        (app, pane)
    }

    #[test]
    fn service_logs_combine_every_replica_with_labels() {
        let (mut app, pane) = app_on_web(3, Tab::Logs);
        let fx = app.sync_panes();
        let [Effect::StartLogs { sources, .. }] = &fx[..] else { panic!("{fx:?}") };
        assert_eq!(sources.len(), 3);
        let prefixes: Vec<&str> = sources.iter().map(|s| s.prefix.as_deref().unwrap()).collect();
        assert!(prefixes[0].contains("web-1 │") && prefixes[2].contains("web-3 │"), "{prefixes:?}");
        assert_ne!(prefixes[0][..12], prefixes[1][..12], "each replica has its own colour");
        // A scale-up joins the same view; nothing reloads.
        install_term(&mut app, pane);
        app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot(4))));
        let fx = app.sync_panes();
        let [Effect::FollowLogs { source, after: None, .. }] = &fx[..] else { panic!("{fx:?}") };
        assert!(source.prefix.as_deref().unwrap().contains("web-4"));
        assert!(app.sync_panes().is_empty());
    }

    #[test]
    fn one_replica_restarting_resumes_alone_and_the_notice_waits_for_the_last() {
        use crate::app::logs::StreamEnd;
        let (mut app, pane) = app_on_web(2, Tab::Logs);
        app.sync_panes();
        install_term(&mut app, pane);
        let ids = app.snap.service("demo", "web").unwrap().replicas.clone();
        let end = |app: &App, id: &String| {
            let run = app.panes[&pane].logs[id].run;
            AppEvent::LogsEnded { pane, run, end: StreamEnd { container: id.clone(), clean: true, last: None } }
        };
        let notices = |app: &App| match &app.panes[&pane].content {
            Content::Term(t) => t.viewport_text().iter().filter(|l| l.contains("log stream ended")).count(),
            _ => panic!(),
        };
        app.handle_app_event(end(&app, &ids[0]));
        assert_eq!(notices(&app), 0, "web-2 still streams");
        app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot(2))));
        let fx = app.sync_panes();
        assert!(
            matches!(&fx[..], [Effect::FollowLogs { source, .. }] if source.id == ids[0]),
            "only web-1 resumes: {fx:?}"
        );
        app.handle_app_event(end(&app, &ids[0]));
        app.handle_app_event(end(&app, &ids[1]));
        assert_eq!(notices(&app), 1, "both stopped");
    }

    #[test]
    fn going_from_one_replica_to_two_reloads_with_labels() {
        let (mut app, _) = app_on_web(1, Tab::Logs);
        let fx = app.sync_panes();
        assert!(matches!(&fx[..], [Effect::StartLogs { sources, .. }] if sources[0].prefix.is_none()), "{fx:?}");
        app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot(2))));
        let fx = app.sync_panes();
        assert!(matches!(&fx[..], [Effect::StartLogs { sources, .. }] if sources.iter().all(|s| s.prefix.is_some())));
    }

    #[test]
    fn env_and_config_show_the_first_replica_and_say_so() {
        let (mut app, _) = app_on_web(3, Tab::Env);
        let fx = app.sync_panes();
        let [Effect::Load { spec: LoadSpec::ContainerEnv { id, note }, .. }] = &fx[..] else { panic!("{fx:?}") };
        assert!(id.starts_with("demo-web-1"));
        assert_eq!(note.as_deref(), Some("showing web-1 of 3 replicas"));
    }

    #[test]
    fn top_lists_every_running_replica() {
        let (mut app, _) = app_on_web(2, Tab::Top);
        let fx = app.sync_panes();
        let [Effect::Load { spec: LoadSpec::Top { containers, labelled: true }, .. }] = &fx[..] else {
            panic!("{fx:?}")
        };
        let labels: Vec<&str> = containers.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["web-1", "web-2"]);
    }

    #[test]
    fn stats_sum_the_replicas_and_list_each() {
        let (mut app, pane) = app_on_web(2, Tab::Stats);
        let now = Instant::now();
        for (i, c) in app.snap.containers.clone().iter().filter(|c| c.service.as_deref() == Some("web")).enumerate() {
            let mut h = StatsHistory::default();
            let sample = |cpu: u64| StatsSample {
                cpu_total: cpu,
                system: 1000,
                online_cpus: 1,
                mem_usage: 64 * 1024 * 1024,
                mem_limit: 1024 * 1024 * 1024,
                pids: 3,
                ..Default::default()
            };
            h.push(now - Duration::from_secs(1), sample(0));
            let mut s = sample(100 * (i as u64 + 1));
            s.precpu_total = 0;
            s.presystem = 0;
            h.push(now, s);
            app.stats.insert(c.id.clone(), h);
        }
        app.sync_panes();
        let text: Vec<String> = (0..30).map(|r| app.content_row_text(pane, r)).collect();
        let find = |s: &str| text.iter().find(|l| l.contains(s)).cloned().unwrap_or_default();
        assert!(find("CPU (%)").contains("30.00 across 2 replicas"), "10% + 20%: {text:#?}");
        assert!(find("Memory (MiB)").contains("128.0"), "{text:#?}");
        assert!(find("PIDs:").ends_with('6'), "{text:#?}");
        assert!(find("web-2").contains("20.00%") && find("web-2").contains("6.2%"), "{text:#?}");
    }

    #[test]
    fn service_actions_reach_every_replica() {
        use crate::docker::ops::DockerOp;
        let (mut app, _) = app_on_web(3, Tab::Logs);
        app.focus = Focus::Sidebar;
        // No compose files on this machine: the containers are acted on directly.
        let fx = app.handle_key(key(KeyCode::Char('r')));
        let [Effect::Docker(DockerOp::All(ops))] = &fx[..] else { panic!("{fx:?}") };
        assert_eq!(ops.len(), 3);
        assert!(ops.iter().all(|op| matches!(op, DockerOp::Restart(_))));
        let fx = app.handle_key(key(KeyCode::Char('p')));
        assert!(
            matches!(&fx[..], [Effect::Docker(DockerOp::All(ops))] if ops.len() == 3 && matches!(ops[0], DockerOp::Pause(_)))
        );
        // One container: the plain op, as before.
        let (mut app, _) = app_on_web(1, Tab::Logs);
        app.focus = Focus::Sidebar;
        assert!(matches!(&app.handle_key(key(KeyCode::Char('r')))[..], [Effect::Docker(DockerOp::Restart(_))]));
    }

    #[test]
    fn a_replica_row_is_a_container_row() {
        let (mut app, _) = app_on_web(2, Tab::Logs);
        app.sidebar.toggle(&web());
        app.rebuild_rows();
        let second = NodeKey::Container(app.snap.service("demo", "web").unwrap().replicas[1].clone());
        app.select_key(&second);
        assert_eq!(app.selected(), Some(&second));
        let fx = app.sync_panes();
        assert!(
            matches!(&fx[..], [Effect::StartLogs { sources, .. }] if sources.len() == 1 && sources[0].prefix.is_none())
        );
        assert!(screen(&draw(&mut app, 120, 30)).contains("demo-web-2"));
    }
}

mod project_order {
    use super::*;
    use crate::docker::model::Snapshot;

    /// Projects alpha, bravo, charlie (and `extra`), one service each.
    fn snapshot(extra: &[&str]) -> Snapshot {
        let containers: Vec<_> = ["alpha", "bravo", "charlie"]
            .iter()
            .chain(extra)
            .map(|p| container(&format!("{p}-app-1"), ContainerState::Running, Some((p, "app"))))
            .collect();
        Snapshot { projects: derive_projects(&containers, None), containers, ..Default::default() }
    }

    fn names(app: &App) -> Vec<&str> {
        app.snap.projects.iter().map(|p| p.name.as_str()).collect()
    }

    fn row_of(app: &App, project: &str) -> u16 {
        let key = NodeKey::Project(project.into());
        app.hits.sidebar.rows.iter().find(|(_, k)| *k == key).unwrap().0.y
    }

    fn app() -> App {
        let mut app = App::new("tcp://test");
        app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot(&[]))));
        draw(&mut app, 120, 30);
        app
    }

    #[test]
    fn dragging_a_project_reorders_live_and_saves_on_release() {
        let mut app = app();
        let (from, to) = (row_of(&app, "charlie"), row_of(&app, "alpha"));
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, from));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 4, to));
        assert_eq!(names(&app), ["charlie", "alpha", "bravo"], "moves while dragging");
        draw(&mut app, 120, 30);
        let fx = app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, to));
        assert_eq!(fx, vec![Effect::SaveProjectOrder(vec!["charlie".into(), "alpha".into(), "bravo".into()])]);
        assert_eq!(app.selected(), Some(&NodeKey::Project("charlie".into())), "the moved project stays selected");
        // The next snapshot (in the daemon's order) keeps it; a new project goes last.
        app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot(&["aardvark"]))));
        assert_eq!(names(&app), ["charlie", "alpha", "bravo", "aardvark"]);
    }

    #[test]
    fn dropping_on_a_service_row_uses_its_project_and_a_click_saves_nothing() {
        let mut app = app();
        let click = app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, row_of(&app, "alpha")));
        assert!(click.is_empty());
        assert!(app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 4, row_of(&app, "alpha"))).is_empty());
        let service_of_charlie = row_of(&app, "charlie") + 1;
        app.last_click = None; // a separate gesture, not a double-click
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 4, row_of(&app, "alpha")));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 4, service_of_charlie));
        assert_eq!(names(&app), ["bravo", "charlie", "alpha"]);
    }

    #[test]
    fn a_saved_order_applies_from_the_first_snapshot() {
        let mut app = App::new("tcp://test");
        app.project_order = vec!["bravo".into(), "gone".into(), "alpha".into()];
        app.handle_app_event(AppEvent::Snapshot(Box::new(snapshot(&[]))));
        assert_eq!(names(&app), ["bravo", "alpha", "charlie"], "unplaced projects follow");
    }
}
