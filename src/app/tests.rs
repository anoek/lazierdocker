use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::docker::model::tests::container;
use crate::docker::model::{derive_projects, ContainerState};

pub fn demo_snapshot() -> Snapshot {
    let containers = vec![
        container("demo-web-1", ContainerState::Running, Some(("demo", "web"))),
        container("demo-db-1", ContainerState::Running, Some(("demo", "db"))),
        container("loner", ContainerState::Running, None),
        container("exited", ContainerState::Exited, None),
    ];
    Snapshot { projects: derive_projects(&containers, None), containers, ..Default::default() }
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
fn docker_error_shown_in_status() {
    let mut app = App::new("tcp://nowhere");
    app.handle_app_event(AppEvent::DockerError("connection refused".into()));
    let buf = draw(&mut app, 100, 10);
    assert!(row_text(&buf, 9).contains("⚠ connection refused"));
    assert!(screen(&buf).contains("Cannot reach docker"));
}

mod inspector {
    use super::*;
    use crate::app::inspect::{tabs_for, LoadSpec, Tab, TargetKind};
    use ratatui::text::Line;

    fn select(app: &mut App, key: NodeKey) {
        let rows = app.rows().to_vec();
        app.sidebar.select(&rows, &key);
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
        let id = app.snap.containers.iter().find(|c| c.name == "loner").unwrap().id.clone();
        select(&mut app, NodeKey::Container(id.clone()));
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Config);
        let effects = app.sync_panes();
        let Some(Effect::Load { key, spec, .. }) = effects.first().cloned() else { panic!("{effects:?}") };
        assert_eq!(spec, LoadSpec::ContainerConfig { id: id.clone() });
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
        select(&mut app, NodeKey::Service("demo".into(), "web".into()));
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Env);
        let effects = app.sync_panes();
        assert!(
            matches!(&effects[0], Effect::Load { spec: LoadSpec::ContainerEnv { id }, .. } if id.starts_with("demo-web-1"))
        );
    }

    #[test]
    fn click_tab_and_bracket_keys_switch_tabs() {
        let mut app = loaded_app();
        let id = app.snap.containers.iter().find(|c| c.name == "loner").unwrap().id.clone();
        select(&mut app, NodeKey::Container(id));
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
        select(&mut app, NodeKey::Section(crate::ui::sidebar::Section::Containers));
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
        app.panes.get_mut(&pane).unwrap().loaded = None;
        // Pin the loaded key so sync doesn't replace our content.
        select(&mut app, NodeKey::Section(crate::ui::sidebar::Section::Volumes));
        draw(&mut app, 100, 20);
        let (_, r, _) = app.hits.panes[0];
        app.handle_mouse(mouse(MouseEventKind::ScrollDown, r.x + 5, r.y + 5));
        let Content::Doc(d) = &app.panes[&pane].content else { panic!("doc expected") };
        assert_eq!(d.scroll, 3);
    }
}

mod streams {
    use super::*;
    use crate::app::inspect::{stats_lines, LoadSpec, Tab};
    use crate::docker::stats::{StatsHistory, StatsSample};

    fn select(app: &mut App, key: NodeKey) {
        let rows = app.rows().to_vec();
        app.sidebar.select(&rows, &key);
    }

    fn loner(app: &App) -> String {
        app.snap.containers.iter().find(|c| c.name == "loner").unwrap().id.clone()
    }

    #[test]
    fn logs_tab_starts_stream_once() {
        let mut app = loaded_app();
        let id = loner(&app);
        select(&mut app, NodeKey::Container(id.clone()));
        let pane = app.layout.focus;
        app.set_tab(pane, Tab::Logs);
        let effects = app.sync_panes();
        assert!(matches!(&effects[..], [Effect::StartLogs { container_id, .. }] if *container_id == id), "{effects:?}");
        assert!(app.sync_panes().is_empty(), "no restart while nothing changed");
    }

    #[test]
    fn logs_restart_when_container_restarts() {
        let mut app = loaded_app();
        let id = loner(&app);
        select(&mut app, NodeKey::Container(id.clone()));
        app.set_tab(app.layout.focus, Tab::Logs);
        app.sync_panes();
        let mut snap = demo_snapshot();
        snap.containers.iter_mut().find(|c| c.id == id).unwrap().state = ContainerState::Exited;
        app.handle_app_event(AppEvent::Snapshot(Box::new(snap)));
        assert!(matches!(&app.sync_panes()[..], [Effect::StartLogs { .. }]));
    }

    #[test]
    fn project_logs_spawn_compose() {
        let mut app = loaded_app();
        select(&mut app, NodeKey::Project("demo".into()));
        let effects = app.sync_panes();
        let Some(Effect::SpawnTerm { argv, key: Some(_), .. }) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(argv.join(" "), "docker compose -p demo logs --tail=300 --follow");
    }

    #[test]
    fn top_requires_running_container() {
        let mut app = loaded_app();
        let exited = app.snap.containers.iter().find(|c| c.name == "exited").unwrap().id.clone();
        select(&mut app, NodeKey::Container(exited));
        app.set_tab(app.layout.focus, Tab::Top);
        assert!(app.sync_panes().is_empty());
        assert!(screen(&draw(&mut app, 100, 20)).contains("container is not running"));
        let id = loner(&app);
        select(&mut app, NodeKey::Container(id.clone()));
        app.set_tab(app.layout.focus, Tab::Top);
        assert!(matches!(&app.sync_panes()[..], [Effect::Load { spec: LoadSpec::Top { id: i }, .. }] if *i == id));
        // Top refreshes every tick.
        app.tick += 1;
        assert_eq!(app.sync_panes().len(), 1);
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
    use crate::app::inspect::Tab;
    use crate::ui::layout::SplitDir;

    fn prefix(app: &mut App, c: char) -> Vec<Effect> {
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char(c)))
    }

    fn select_loner(app: &mut App) -> NodeKey {
        let id = app.snap.containers.iter().find(|c| c.name == "loner").unwrap().id.clone();
        let k = NodeKey::Container(id);
        let rows = app.rows().to_vec();
        app.sidebar.select(&rows, &k);
        k
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
    fn exec_opens_terminal_pane_with_login_shell() {
        let mut app = loaded_app();
        let k = select_loner(&mut app);
        draw(&mut app, 120, 30);
        let effects = app.handle_key(key(KeyCode::Char('E')));
        let Some(Effect::SpawnTerm { argv, key: None, pane }) = effects.first() else { panic!("{effects:?}") };
        assert_eq!(&argv[..3], &["docker", "exec", "-it"]);
        assert!(argv.last().unwrap().contains("/etc/passwd"));
        assert!(matches!(&app.panes[pane].kind, PaneKind::Terminal { title } if title == "exec loner"));
        // Exited containers can't be exec'd into.
        let exited = app.snap.containers.iter().find(|c| c.name == "exited").unwrap().id.clone();
        assert!(app.exec_shell(&NodeKey::Container(exited)).is_empty());
        assert!(app.toast.is_some());
        let _ = k;
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
        let id = app.open_pane(PaneKind::Terminal { title: "cat".into() }, None);
        let t = crate::term::TermSurface::spawn(
            &["cat".into()],
            &[],
            40,
            10,
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(|_, _| {}),
        )
        .unwrap();
        app.panes.get_mut(&id).unwrap().content = Content::Term(t);
        // 'q' goes to cat, not quit; ctrl-c goes to cat too.
        assert!(app.handle_key(key(KeyCode::Char('q'))).is_empty());
        app.handle_key(key(KeyCode::Enter));
        let ok = (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            matches!(&app.panes[&id].content, Content::Term(t) if t.viewport_text().iter().filter(|l| l.starts_with('q')).count() >= 2)
        });
        assert!(ok);
        assert!(app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)).is_empty());
        // The prefix still works.
        prefix(&mut app, 'h');
        assert_ne!(app.layout.focus, id);
    }
}

mod exits {
    use super::*;

    #[test]
    fn stale_exit_does_not_mark_new_surface() {
        let mut app = loaded_app();
        let pane = app.layout.focus;
        let t = crate::term::TermSurface::stream(20, 3, std::sync::Arc::new(|| {})).unwrap();
        let sid = t.id();
        app.panes.get_mut(&pane).unwrap().content = Content::Term(t);
        app.handle_app_event(AppEvent::TermExited(pane, sid + 1000, Some(1)));
        let Content::Term(t) = &app.panes[&pane].content else { panic!() };
        assert!(!t.viewport_text().join("").contains("ended"), "stale exit ignored");
        app.handle_app_event(AppEvent::TermExited(pane, sid, Some(1)));
        let Content::Term(t) = &app.panes[&pane].content else { panic!() };
        assert!(t.viewport_text().join("").contains("code 1"));
    }
}

mod overlays {
    use super::*;
    use crate::app::actions::Action;
    use crate::app::overlay::{Menu, MenuItem, Overlay};

    fn menu_app() -> App {
        let mut app = loaded_app();
        draw(&mut app, 100, 30);
        app.overlay = Some(Overlay::Menu(Menu::new(
            "t",
            vec![
                MenuItem::new("one", Action::None),
                MenuItem::new("quit", Action::Quit).key("Q"),
                MenuItem::new("three", Action::None),
            ],
        )));
        draw(&mut app, 100, 30);
        app
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
        assert_eq!(app.handle_key(key(KeyCode::Enter)), vec![Effect::Quit]);
        assert!(app.overlay.is_none());
        // Shortcut key activates directly.
        let mut app = menu_app();
        assert_eq!(app.handle_key(key(KeyCode::Char('Q'))), vec![Effect::Quit]);
    }

    #[test]
    fn menu_mouse_hover_and_click() {
        let mut app = menu_app();
        let (r, _) = app.overlay_hits.rows[2];
        app.handle_mouse(mouse(MouseEventKind::Moved, r.x + 1, r.y));
        let Some(Overlay::Menu(m)) = &app.overlay else { panic!() };
        assert_eq!(m.highlighted, 2);
        let (r, _) = app.overlay_hits.rows[1];
        assert_eq!(app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 1, r.y)), vec![Effect::Quit]);
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
        app.overlay =
            Some(Overlay::Confirm { title: "Quit?".into(), body: "sure".into(), danger: true, action: Action::Quit });
        let s = screen(&draw(&mut app, 100, 30));
        assert!(s.contains("↵ confirm") && s.contains("esc cancel"), "{s}");
        assert!(app.handle_key(key(KeyCode::Char('n'))).is_empty());
        assert!(app.overlay.is_none());
        app.overlay =
            Some(Overlay::Confirm { title: "Quit?".into(), body: "sure".into(), danger: true, action: Action::Quit });
        assert_eq!(app.handle_key(key(KeyCode::Char('y'))), vec![Effect::Quit]);
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
    use crate::app::actions::Action;
    use crate::app::overlay::Overlay;
    use crate::docker::ops::DockerOp;

    fn select(app: &mut App, key: NodeKey) {
        let rows = app.rows().to_vec();
        app.sidebar.select(&rows, &key);
    }

    fn loner(app: &App) -> String {
        app.snap.containers.iter().find(|c| c.name == "loner").unwrap().id.clone()
    }

    fn local_app() -> App {
        let mut app = App::new("tcp://test");
        let mut snap = demo_snapshot();
        snap.projects[0].local = true;
        app.compose_files = vec!["compose.yml".into()];
        app.handle_app_event(AppEvent::Snapshot(Box::new(snap)));
        app
    }

    #[test]
    fn stop_requires_confirm() {
        let mut app = loaded_app();
        let id = loner(&app);
        select(&mut app, NodeKey::Container(id.clone()));
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
        let id = loner(&app);
        select(&mut app, NodeKey::Container(id.clone()));
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
        let id = loner(&app);
        let op = DockerOp::RemoveContainer { id: id.clone(), force: false, volumes: false };
        let fx = app.handle_app_event(AppEvent::OpDone {
            op,
            desc: "removing".into(),
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
        app.handle_app_event(AppEvent::OpDone {
            op: DockerOp::Start("x".into()),
            desc: "starting".into(),
            result: Err("boom".into()),
        });
        assert!(matches!(&app.overlay, Some(Overlay::Message { error: true, body, .. }) if body == "boom"));
    }

    #[test]
    fn compose_cmd_includes_project() {
        let mut app = local_app();
        select(&mut app, NodeKey::Service("demo".into(), "web".into()));
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
        select(&mut app, NodeKey::Service("demo".into(), "web".into()));
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
        select(&mut app, NodeKey::Section(crate::ui::sidebar::Section::Images));
        app.handle_key(key(KeyCode::Char('b')));
        app.handle_key(key(KeyCode::Enter));
        let Some(Overlay::Confirm { action, danger: true, .. }) = &app.overlay else { panic!("{:?}", app.overlay) };
        assert_eq!(*action, Action::Docker(DockerOp::PruneImages));
    }

    #[test]
    fn context_menu_lists_docker_actions() {
        let app = loaded_app();
        let m = app.item_menu(&NodeKey::Container(loner(&app)));
        let labels: Vec<_> = m.items.iter().map(|i| i.label.as_str()).collect();
        for want in ["Stop", "Restart", "Remove…", "Bulk…", "Exec shell"] {
            assert!(labels.contains(&want), "{labels:?}");
        }
    }
}

mod select {
    use super::*;
    use ratatui::text::Line;

    /// App with a known document in the inspector, pinned to a section so sync won't replace it.
    fn doc_app() -> (App, crate::ui::layout::PaneId, Rect) {
        let mut app = loaded_app();
        let rows = app.rows().to_vec();
        app.sidebar.select(&rows, &NodeKey::Section(crate::ui::sidebar::Section::Volumes));
        app.sync_panes();
        let pane = app.layout.focus;
        let lines =
            vec![Line::raw("alpha beta gamma"), Line::raw("second line here"), Line::raw("see http://x.io/p ok")];
        app.panes.get_mut(&pane).unwrap().content = Content::Doc(crate::doc::DocSurface::new(lines));
        draw(&mut app, 100, 20);
        let r = app.hits.content.iter().find(|(p, _)| *p == pane).unwrap().1;
        (app, pane, r)
    }

    #[test]
    fn drag_selects_and_copies() {
        let (mut app, _, r) = doc_app();
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
        let (mut app, pane, r) = doc_app();
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 2, r.y));
        let fx = app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 2, r.y));
        assert!(fx.is_empty());
        assert!(app.selection.is_none());
        assert_eq!((app.focus, app.layout.focus), (Focus::Panes, pane), "click focuses the pane");
    }

    #[test]
    fn double_click_copies_word_or_url() {
        let (mut app, _, r) = doc_app();
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
        let (mut app, pane, r) = doc_app();
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
        let (mut app, pane, r) = doc_app();
        let t = crate::term::TermSurface::stream(r.width, r.height, std::sync::Arc::new(|| {})).unwrap();
        t.feeder(true).feed(b"hello terminal\nworld\n");
        app.panes.get_mut(&pane).unwrap().content = Content::Term(t);
        draw(&mut app, 100, 20);
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), r.x + 6, r.y));
        app.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), r.x + 4, r.y + 1));
        let fx = app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), r.x + 4, r.y + 1));
        assert_eq!(fx, vec![Effect::Copy("terminal\nworld".into())]);
    }
}

mod modes {
    use super::*;
    use ratatui::text::Line;

    fn prefix(app: &mut App, c: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        app.handle_key(key(KeyCode::Char(c)));
    }

    fn doc_app(lines: Vec<&str>) -> App {
        let mut app = loaded_app();
        let rows = app.rows().to_vec();
        app.sidebar.select(&rows, &NodeKey::Section(crate::ui::sidebar::Section::Volumes));
        app.sync_panes();
        let pane = app.layout.focus;
        app.panes.get_mut(&pane).unwrap().content =
            Content::Doc(crate::doc::DocSurface::new(lines.into_iter().map(|l| Line::raw(l.to_string())).collect()));
        draw(&mut app, 100, 20);
        app.focus = Focus::Panes;
        app
    }

    #[test]
    fn copy_mode_yank_line() {
        let mut app = doc_app(vec!["first line", "second line", "third"]);
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
        let mut app = doc_app(vec!["alpha beta gamma"]);
        prefix(&mut app, '[');
        app.handle_key(key(KeyCode::Char('g')));
        app.handle_key(key(KeyCode::Char('w')));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('e')));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), vec![Effect::Copy("beta".into())]);
    }

    #[test]
    fn copy_mode_scrolls_to_cursor_and_esc_exits() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        let mut app = doc_app(lines.iter().map(|s| s.as_str()).collect());
        let pane = app.layout.focus;
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
        let rows = app.rows().to_vec();
        let id = app.snap.containers.iter().find(|c| c.name == "loner").unwrap().id.clone();
        app.sidebar.select(&rows, &NodeKey::Container(id));
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
}
