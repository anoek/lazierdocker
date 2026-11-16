# lazierdocker First Cut Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A working Rust TUI Docker manager with lazydocker's features and herdr's interaction model, with libghostty-backed terminal panes.

**Architecture:** Single tokio binary. Pure `AppState` + compose step (geometry + hit map) + pure render (ratatui 0.30, crossterm 0.29). Docker via bollard, plus the `docker`/`docker compose` CLI for compose/exec/attach. Terminal panes via vendored herdr `ghostty-vt` crate over `portable-pty`.

**Tech Stack:** Rust stable (1.100), ratatui 0.30, crossterm 0.29 (event-stream), tokio 1, bollard 0.21, portable-pty 0.9, ghostty-vt (vendored, needs Zig 0.16), base64, serde/serde_yaml (YAML rendering of inspect output), unicode-width.

**Spec:** `docs/superpowers/specs/2026-11-16-lazierdocker-design.md` (+ `docs/reference/*.md`)

## Global Constraints
- Linux + macOS only; no `cfg(windows)` code paths.
- `crates/ghostty-vt` and `vendor/libghostty-vt` copied verbatim from herdr ee5bfb5; NOTICE file credits herdr (Apache-2.0) and libghostty (MIT). No other herdr code is copied.
- Palette = herdr Catppuccin Mocha values listed in `docs/reference/herdr-ui-techniques.md`.
- Split ratios clamped to 0.1..=0.9. Double-click window 350 ms, within 1 cell. Min render interval 16 ms. Events refresh throttle 50 ms. Stats history 3 min.
- Destructive docker operations always go through the lazydocker-equivalent confirmation/menu.
- Integration tests only run when `LAZIERDOCKER_TEST_DOCKER_HOST` is set; never against the default daemon.
- Commit to `main` after each task; `cargo test` and `cargo clippy -- -D warnings` clean before each commit.

## Review Focus
1. Terminal too small (e.g. 20×6) or resized while dragging — layout must not panic (saturating math), drags must abort cleanly. → Task 2 test `layout_tiny_area_no_panic`, Task 6 test `render_tiny_terminal`.
2. Docker daemon unreachable at start or events stream dropping — app must start, show an error banner and reconnect every 2 s, not exit. → Task 4 test `refresh_reports_unreachable_daemon`.
3. Selected item disappears (container removed) — selection must move to a neighbour, pinned panes show "gone" instead of panicking. → Task 5 test `selection_survives_removal`.
4. Child process in a terminal pane exits (shell `exit`) — pane shows "[process exited]" and can be closed; no zombie or hang. → Task 8 test `terminal_child_exit_marks_pane`.
5. Non-TTY container logs with bare `\n` and stderr interleaving — rendered as separate lines starting at column 0. → Task 9 test `logs_lf_normalised`.

---

### Task 1: Scaffold + vendored libghostty
**Files:** Create `Cargo.toml` (package `lazierdocker`, workspace member `crates/ghostty-vt`), `crates/ghostty-vt/**`, `vendor/libghostty-vt/**`, `vendor/libghostty-vt.vendor.json`, `NOTICE`, `README.md`, `src/main.rs`, `rust-toolchain.toml` (stable), `justfile`-free; `.gitignore` add `target/`.
**Produces:** buildable crate; `ghostty_vt` importable.
- [ ] Copy vendored dirs from herdr checkout; write NOTICE.
- [ ] Test `tests/ghostty_smoke.rs::terminal_roundtrip`: `Terminal::new(20,5,1<<20)`, `write(b"hi\r\nthere")`, `read_text_screen((0,0),(19,1),false)` contains `"hi"` and `"there"`.
- [ ] `cargo test` passes; commit.

### Task 2: Split layout tree + border glyphs
**Files:** `src/ui/layout.rs`, `src/ui/borders.rs`, `src/ui/theme.rs`.
**Produces:**
- `PaneId(u64)`, `enum Dir { Horizontal /*side by side*/, Vertical }`, `enum Node { Pane(PaneId), Split{dir, ratio:f32, first:Box<Node>, second:Box<Node>} }`
- `Layout { root: Node, focus: PaneId, zoomed: bool }` with `split(target, dir, new) `, `remove(id) -> bool`, `rects(area) -> Vec<(PaneId, Rect)>`, `splits(area) -> Vec<SplitBorder{pos:u16, dir, area:Rect, path:Vec<bool>}>`, `set_ratio(path:&[bool], ratio)`, `neighbour(from, Direction4) -> Option<PaneId>`, `resize_focused(Direction4, delta)`, `swap(a,b)`.
- `borders::draw(buf, area, panes:&[(PaneId,Rect,title,focused)], theme)` — shared dividers, T-junction glyphs from per-cell `{up,down,left,right}`; returns inner rects `HashMap<PaneId,Rect>`.
- `Theme` struct with the Mocha palette fields.
- [ ] Tests: `split_and_rects_cover_area`, `remove_collapses_parent`, `ratio_clamped`, `neighbour_prefers_overlap`, `junction_glyphs` (2×2 grid has `┼` at centre), `layout_tiny_area_no_panic`.
- [ ] Implement; tests pass; commit.

### Task 3: Docker model + presentation
**Files:** `src/docker/model.rs`, `src/docker/present.rs`.
**Produces:** `Snapshot { containers: Vec<Container>, images, volumes, networks, projects: Vec<Project>, local_project: Option<String> }`; `Container { id, name, image, state: ContainerState, status, exit_code:Option<i64>, health:Option<Health>, ports:Vec<Port>, labels, project:Option<String>, service:Option<String>, oneoff:bool, tty:bool, open_stdin:bool, created }`; `Project { name, services: Vec<Service> }`; `Service { name, project, container_id:Option<String> }`; `fn derive_projects(containers, local_services:&[String], local_project) -> Vec<Project>`; `fn sort_containers(&mut [Container])` (running, exited, created, then other states, then name); `present::container_cells(&Container, cpu:Option<f64>, theme) -> Vec<Span>` per lazydocker columns; `present::ports(&[Port]) -> String`; `cpu_percent(&Stats) -> f64` docker-CLI formula (× online cpus).
- [ ] Tests: `projects_from_labels`, `oneoff_skipped`, `sort_order`, `ports_format` (`"0.0.0.0:8080->80/tcp"` → `"8080->80/tcp"`), `cpu_percent_formula`, `image_untagged_named_none_sorted_last`.
- [ ] Implement; commit.

### Task 4: Docker client + refresh loop
**Files:** `src/docker/client.rs`, `src/docker/refresh.rs`.
**Produces:** `Docker::connect() -> Result<Docker>` (bollard `connect_with_defaults`, honours DOCKER_HOST); `async fn snapshot(&self) -> Result<Snapshot>`; `fn spawn_refresh(docker, tx: mpsc::Sender<AppEvent>)` emitting `AppEvent::Snapshot(Snapshot)` / `AppEvent::DockerError(String)`; events stream triggers throttled refresh (50 ms), reconnect every 2 s on error. Compose local project detection: `docker compose config --services` in cwd (if it succeeds).
- [ ] Integration test (env-gated) `snapshot_sees_demo_project` against dind fixture (`demo` project, `loner`, `exited`).
- [ ] Test `refresh_reports_unreachable_daemon` (DOCKER_HOST=tcp://127.0.0.1:1 → DockerError event within 3 s).
- [ ] Commit.

### Task 5: Sidebar tree
**Files:** `src/ui/sidebar.rs`.
**Produces:** `enum NodeKey { Section(Section), Project(String), Service(String,String), Container(String), Image(String), Volume(String), Network(String) }`; `Sidebar { collapsed: HashSet<NodeKey>, selected: Option<NodeKey>, scroll: usize, filter: String, show_exited: bool, width: u16 }`; `fn rows(&self, snap:&Snapshot) -> Vec<Row{key, depth, expandable, expanded, last_child}>`; `move_selection(delta)`, `toggle(key)`, `render(buf, area, snap, stats, theme, focused) -> Vec<(Rect, NodeKey, Option<Rect /*toggle cell*/>)>`.
Sections: Projects, Containers (standalone), Images, Volumes, Networks. `▾/▸` glyphs, `├─ └─` connectors in overlay0, header counts.
- [ ] Tests: `collapsed_section_hides_children`, `filter_matches_case_insensitive`, `selection_survives_removal`, `show_exited_toggle`.
- [ ] Commit.

### Task 6: App shell, event loop, mouse basics
**Files:** `src/app/mod.rs` (AppState, AppEvent), `src/app/run.rs` (loop), `src/app/hits.rs`, `src/ui/render.rs`, `src/ui/statusbar.rs`, `src/main.rs`.
**Produces:** `AppState::new`, `handle_event(&mut self, Event) -> Vec<Effect>`, `compose_and_render(&mut self, frame)`; hit map `Hits { sidebar_rows, sidebar_divider, pane_rects, split_borders, tabs, overlay... }`; loop with `tokio::select!` over crossterm `EventStream`, app channel, render deadline (≥16 ms between frames, repaint only when dirty). Terminal setup: alt screen, raw mode, mouse capture, bracketed paste, focus change; restored on panic.
Mouse: click selects row, click toggle glyph collapses, wheel scrolls under pointer, sidebar divider drag (min 18, max 60), double-click divider resets to 34.
Status bar: key hints (lazydocker style), docker host, error banner.
- [ ] Tests (TestBackend): `render_shows_sections`, `render_tiny_terminal`, `click_selects_row`, `divider_drag_changes_width`.
- [ ] e2e smoke in tmux vs dind; commit.

### Task 7: Document surface + inspector tabs
**Files:** `src/doc/mod.rs` (DocSurface), `src/doc/format.rs` (yaml colouring, env table, config views), `src/ui/inspector.rs`.
**Produces:** `DocSurface { lines: Vec<Line<'static>>, scroll: usize }` with `render(buf, area)`, `scroll_by(i32)`, `text_range(..)->String`; tabs per kind per lazydocker table; `Tab` enum; tab bar with hits; `[`/`]`; Config/Env/Image history/Volume/Network/Compose-config renderers fetched via inspect (async effect → `AppEvent::Doc`).
- [ ] Tests: `yaml_colouring_keys_cyan`, `env_table`, `image_history_strips_nop`, `tabs_for_kinds`.
- [ ] Commit.

### Task 8: Terminal surface (libghostty)
**Files:** `src/term/mod.rs` (TermSurface), `src/term/render.rs`, `src/term/keys.rs`, `src/term/mouse.rs`.
**Produces:** `TermSurface::spawn(cmd: Vec<String>, cols, rows, wake: Sender<AppEvent>) -> Result<Self>`; `TermSurface::stream(cols, rows) -> Self` + `feed(&[u8])` for non-PTY content; `render(buf, area, focused)`; `send_key(KeyEvent)`, `send_mouse(MouseEvent, local_col,row) -> bool`, `wheel(lines:i32, at)`, `paste(&str)`, `resize(cols, rows)`, `exited() -> Option<i32>`, `text(start,end) -> String`, `scrollback position`. Query replies written back to PTY.
- [ ] Tests: `spawn_echo_renders_text`, `key_encoding_basic` ('a'→"a", Enter→"\r", Ctrl-C→"\x03", Up→"\x1b[A"), `terminal_child_exit_marks_pane`, `resize_propagates`.
- [ ] Commit.

### Task 9: Logs, Top, Stats
**Files:** `src/docker/logs.rs`, `src/docker/stats.rs`, `src/doc/graph.rs`.
**Produces:** logs task (bollard logs follow, since 60m, demux, `\n`→`\r\n` when !tty) feeding TermSurface; restart on target change and when container restarts; top polling 1 s → DocSurface table; stats sampler per running container (history 3 min) + braille/line graphs CPU (cyan) / Memory (green) with lazydocker caption format, PIDs, traffic.
- [ ] Tests: `logs_lf_normalised`, `graph_scales_to_height`, `stats_history_trimmed`.
- [ ] Commit.

### Task 10: Panes & window arrangement
**Files:** `src/app/panes.rs`, extend `src/app/mod.rs`, `src/input/keymap.rs`.
**Produces:** `PaneKind { Inspector{tab}, Pinned{target:NodeKey, tab}, Terminal{title, surface} }`; prefix (ctrl+b) bindings: v split right, - split down, x close, z zoom, h/j/k/l focus, o next, r resize mode, b toggle sidebar, q quit, ? help; border drag with grab offset; click focuses pane; keys to focused terminal pane except prefix; "Open in split" pins current item; `E` exec and `a` attach open terminal panes (split right of focused); `m` opens logs pane.
- [ ] Tests: `prefix_split_creates_pane`, `close_last_pane_restores_inspector`, `drag_border_updates_ratio`, `keys_forwarded_to_terminal`.
- [ ] Commit.

### Task 11: Overlays
**Files:** `src/ui/overlay.rs`, `src/app/overlay.rs`.
**Produces:** `Overlay { Menu{title, items:Vec<MenuItem{label, detail, danger, action}>, highlighted, anchor:Option<(u16,u16)>}, Confirm{title, body, danger, action}, Prompt{title, input, purpose}, Help{filter}, Error{msg} }` + toast; centred panel with dimmed backdrop (menus at anchor without dim); keyboard (j/k/↑/↓/Enter/Esc/y/n) and mouse (hover highlight, click, outside-click closes); right-click context menus for sidebar rows and panes.
- [ ] Tests: `menu_keyboard_navigation`, `click_outside_closes`, `confirm_y_runs_action`, `context_menu_clamped_on_screen`.
- [ ] Commit.

### Task 12: Docker actions (lazydocker parity)
**Files:** `src/docker/actions.rs`, `src/docker/compose.rs`, `src/app/actions.rs`.
**Produces:** `enum DockerOp` (Start/Stop/Restart/Pause/Unpause/Remove{force, volumes}/ImageRemove{force,no_prune}/VolumeRemove{force}/NetworkRemove/Prune(kind)/StopAll/RemoveAll/Compose(Vec<String> args, attached:bool)); `compose::cmd(project, args) -> Vec<String>` (`docker compose -p P ...`, fallback `docker-compose`); per-item key handlers producing menus/confirms per inventory §3–4; bulk menus; `w` open browser via `xdg-open`/`open`; attached compose commands run in a terminal pane; background op status in status bar; errors → Error overlay; "must stop" → force-remove confirm.
- [ ] Tests: `compose_cmd_includes_project`, `remove_menu_items`, `bulk_service_menu_matches_lazydocker`, `stop_requires_confirm`.
- [ ] Integration (env-gated): `restart_container_via_op`.
- [ ] Commit.

### Task 13: Selection & clipboard
**Files:** `src/input/selection.rs`, `src/input/clipboard.rs`, `src/input/words.rs`.
**Produces:** `Selection { pane, anchor:(row_abs,col), head, phase }`, stream `contains`; mouse drag selection on Doc and Term surfaces (forward to child instead when it tracks mouse), autoscroll at edges; copy-on-select → `Effect::Copy(String)`; double-click word; clipboard: wl-copy/xclip/xsel/pbcopy then OSC 52 (prefer OSC 52 over SSH); toast "● copied to clipboard" 2 s.
- [ ] Tests: `selection_stream_contains`, `word_bounds_url_and_token`, `click_is_not_selection`.
- [ ] Commit.

### Task 14: Copy mode, resize mode, mode bar, help
**Files:** `src/input/copy_mode.rs`, `src/ui/modebar.rs`, help content in `src/ui/overlay.rs`.
**Produces:** prefix+[ copy mode (h/j/k/l, w/b, 0/$, g/G, PgUp/PgDn, v/V, y/Enter copy, q/Esc exit, / search n/N); prefix+r resize mode (h/j/k/l ±0.05, Esc); mode bar pills; help overlay listing bindings with `/` filter.
- [ ] Tests: `copy_mode_yank_line`, `resize_mode_adjusts_ratio`.
- [ ] Commit.

### Task 15: Polish & verification
- [ ] README (features, keys, build requirements: Zig 0.16).
- [ ] Full e2e session in tmux against dind covering each Review Focus item; fix findings.
- [ ] Whole-repo review by fresh reviewer agent; address findings; commit.
