# lazierdocker — first-cut design

Date: 2026-11-16 · Status: approved to build (user delegated design; kickoff decisions recorded below)

## Intent
A Rust TUI for managing Docker with **lazydocker's feature set** (business logic) and **herdr's
terminal interaction model** (mouse-first, copy/paste, border-drag resizing, hierarchical
collapsing, modal menus, window arrangement). Terminal emulation (shells, attach, logs) uses
**libghostty** (libghostty-vt) through herdr's binding crate.

Reference studies (read these before building): `docs/reference/lazydocker-inventory.md`,
`docs/reference/herdr-ui-techniques.md`, `docs/reference/ghostty-vt-usage.md`.

## Decisions made by the user (kickoff)
- Vendor herdr's `crates/ghostty-vt` + `vendor/libghostty-vt` verbatim (Apache-2.0, with NOTICE);
  write all UI code fresh, using herdr only as a reference.
- Platforms: Linux + macOS. No Windows.
- Commit straight to `main`, small logical commits, local only.
- Don't ask the user to "imagine" — ground choices in lazydocker / herdr.

## Goals
1. lazydocker parity for: projects/services/containers/images/volumes/networks lists; detail tabs
   (Logs, Stats, Env, Config, Top; Config for images/volumes/networks; compose config for projects);
   actions (start/stop/restart/pause/remove/attach/exec/up/down/recreate/rebuild/prune, bulk
   commands, open in browser); confirmations and option menus; filtering; events-driven refresh.
2. herdr interaction model: everything clickable; drag split borders and the sidebar divider;
   right-click context menus; collapsible tree sidebar; modal overlays with dimmed backdrop;
   split/zoom/close/focus-by-direction window arrangement; mouse selection with copy-on-select,
   double-click word, OSC 52 / native clipboard; prefix key (ctrl+b) for window ops.
3. Embedded terminals: `docker exec` shells and `docker attach` run as libghostty panes inside the
   layout (several at once), with key/mouse/paste forwarding and scrollback.

## Non-goals (first cut)
- herdr's detached server/client, persistence of running sessions, remote machines, plugins.
- Windows. In-app docker context switching (DOCKER_HOST / current context only).
- lazydocker's config file compatibility (we have our own small config later).

## Anti-goals
- Never run destructive operations without the lazydocker-equivalent confirmation.
- Tests must never touch the user's real Docker daemon destructively: integration tests run only
  against `LAZIERDOCKER_TEST_DOCKER_HOST` (a docker-in-docker daemon).

## Approaches considered
1. **Lazydocker panel layout + herdr chrome** (fixed 6 stacked side panels, one main panel): closest
   to lazydocker visually, but fixed panels fight hierarchical collapsing and window arrangement.
2. **herdr shell with a resource tree** (chosen): a herdr-style left sidebar holding a collapsible
   resource tree (Projects ▸ services/containers, Standalone containers, Images, Volumes, Networks),
   and a herdr-style split-tree pane area on the right. The default single pane is an *inspector*
   that follows the sidebar selection and shows lazydocker's tabs; more panes can be split off and
   pinned to an item, or be terminal panes (exec/attach). This is the only option that delivers
   both "hierarchical collapsing" and "window arrangement" without inventing new paradigms.
3. **Tabs-only** (each resource opens in a tab): loses lazydocker's glanceable lists.

Docker access: `bollard` (async Docker API) for everything lazydocker does via the SDK; the
`docker` / `docker compose` CLI for compose operations, exec and attach (as lazydocker does).

## Architecture
Single binary, tokio runtime, `crossterm` + `ratatui 0.30`.

```
src/
  main.rs            CLI args (-p project, -f compose files, --host), terminal setup/teardown
  app/               AppState (pure data), actions, event loop, effect execution
  docker/            bollard client, model types, snapshot refresh (events stream + throttle),
                     stats sampler, logs streams, compose CLI templates, presentation (row cells)
  ui/                theme (Catppuccin Mocha palette), sidebar tree, split layout + border glyphs,
                     inspector tabs, overlays (menu/confirm/prompt/help/toast), status/mode bar,
                     hit map
  term/              libghostty terminal pane: PTY spawn, pump thread, render to buffer,
                     key/mouse/paste encoding
  doc/               "document" surface: styled lines with scroll + selection (config/env/top/stats)
  input/             keymap (direct + prefix), mouse routing, selection, clipboard
crates/ghostty-vt/   vendored from herdr (Apache-2.0)
vendor/libghostty-vt vendored libghostty-vt (patched by herdr)
```

Principles (from herdr): state separate from runtime; render is pure; a compose step computes
geometry and a **hit map** each frame; mouse handlers test only the previous frame's hits; side
effects returned as actions and executed after input handling; repaint only when dirty, ≤ 60 fps.

### Surfaces
Every pane shows a *surface*:
- **Terminal surface** (libghostty): exec shell, attach, container logs (bollard stream fed in,
  `\n`→`\r\n` for non-TTY containers), compose logs, attached commands (rebuild, pull…).
- **Document surface**: styled lines (Config YAML, Env, Top table, Stats graphs, image history…).
Both support wheel scrolling, mouse selection → clipboard, double-click word.

### Panes
`PaneKind::Inspector { follows selection, tab }`, `PaneKind::Pinned { target, tab }`,
`PaneKind::Terminal { title, process }`. Split tree with ratios 0.1–0.9, shared divider glyphs
with T-junctions, focused border in accent, titles on the top border, zoom, close, focus-by-
direction, swap.

### Sidebar tree
Sections (collapsible): **Projects** (each compose project collapsible → services, each service
showing its container status), **Containers** (standalone), **Images**, **Volumes**, **Networks**.
Collapsed set is a `HashSet<NodeKey>`; flattened to visible rows each frame. Row cells mirror
lazydocker's presentation (state glyph/color, name, CPU%, ports, image). `/` filters. `e` toggles
exited containers. Width draggable (min 18, max 60), double-click divider resets.

### Keys
- Sidebar focused (default): lazydocker keys per item type (s stop, r restart, d remove menu,
  p pause, a attach, E exec, u up, U up project, D down menu, R restart menu, S start, w browser,
  b bulk, m logs-in-pane, c custom commands later), `[`/`]` tabs, j/k/↑/↓, h/l/←/→ collapse/expand,
  Enter focus the inspector, `/` filter, `x`/`?` help, `q` quit, 1–5 jump to section.
- Prefix `ctrl+b` (works everywhere, including inside terminal panes): `v` split right, `-` split
  down, `x` close pane, `z` zoom, `h/j/k/l` focus, `r` resize mode, `[` copy mode, `b` toggle
  sidebar, `?` help, `o` cycle pane focus, `q` quit.
- Terminal pane focused: all keys forwarded except the prefix.

### Data flow
Docker events stream → throttled (50 ms) full snapshot refresh (list containers/images/volumes/
networks + inspect per container) → `AppState.docker`. Stats: one streaming task per *running*
container, history 3 min. Logs/top/stats for the inspector are tasks keyed by (pane, target, tab),
restarted on change (lazydocker's task model).

## Testing
- Unit tests for pure logic: layout geometry & border glyphs, sidebar flattening/collapse, row
  presentation, keymap, selection model, compose command construction, CPU% math.
- Render tests with ratatui `TestBackend`.
- Integration tests (ignored unless `LAZIERDOCKER_TEST_DOCKER_HOST` is set) against a dind daemon.
- End-to-end: run the binary inside tmux against the dind daemon, drive keys and raw SGR mouse
  sequences, capture the screen.

## Left to the builder
Exact colors beyond the herdr palette, help text wording, config file format (deferred), keyboard
copy mode (phase late), custom commands (phase late).
