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
