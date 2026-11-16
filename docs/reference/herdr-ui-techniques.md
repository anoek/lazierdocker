# herdr UI techniques (reference; re-implemented, not copied)

Studied from herdr ee5bfb5 (Apache-2.0). We re-implement these techniques in our own code.

## Layout
- Binary split tree: `Node::Pane(id) | Node::Split{direction, ratio /*first child share*/, first, second}`.
  ratio clamped 0.1..=0.9. `Direction::Horizontal` = side by side (vertical divider).
- Geometry: recursive; `first_w = round(width*ratio)`. `collect_splits` emits
  `SplitBorder{pos, direction, area, path: Vec<bool>}` — path from root is the stable handle.
- Neighbours share one divider line. Border glyphs computed per cell from
  `LineCell{up,down,left,right}` → `┼ ┤ ├ ┴ ┬ │ ─ ┌ ┐ └ ┘` so T-junctions are correct.
- Focused pane: junction/border cells in `accent`, others `overlay0`. Titles on top border
  `" {label} "`, truncated with `…`; focused = accent+BOLD.
- Focus by direction: candidates strictly on that side with perpendicular overlap; minimise
  (edge distance, -overlap, centre distance). Keyboard resize ±0.05 on nearest matching split.
- Zoom: a bool per tab; layout returns one rect.
- Sidebar left, default width 26 (min 18, max 36), draggable divider `│` (surface_dim) at its last
  column; double-click divider (350 ms) resets width.

## Mouse
- Enable: clear stale modes, `EnableMouseCapture`, `EnableBracketedPaste`, `EnableFocusChange`.
- **Render returns a hit map** (rects for every clickable thing); mouse handlers only test the
  previous frame's hits. Routing cascade top-most first: active drag → overlay (swallows all; click
  outside closes) → selection drag → normal click/scroll/move.
- Split drag: on Down store `grab_offset = border_pos - pointer` + split path; on Drag
  `ratio = ((pointer + grab_offset - origin) / length).clamp(0.1, 0.9)`; on Up finalise.
  Hit zone = the 1-cell divider. No hover styling on borders.
- Row press vs drag: press records start; release without movement activates; movement ≥1 cell
  turns it into a drag-reorder.
- Double-click: same target, ≤350 ms, within 1 cell.
- Wheel routes to whatever is under the pointer (not the focused pane).
- Right-click opens a context menu at the pointer for the item under it.
- Hover (Moved events) highlights menu rows only.

## Copy/paste
- Selection phases Anchored → Dragging → Done; a plain click is not a selection. Rows stored as
  absolute buffer rows. Stream selection (partial first/last line).
- Selected cells: `fg = black/white (contrast), bg = terminal bg mixed 28% toward white` (dark).
- Autoscroll when dragging past the edge: step = distance*3 clamped 3..15 lines every 30 ms.
- Double-click selects a word (URL > quoted path > token; separators whitespace and `|()[]{},;!`).
- Copy-on-select (default on): on mouse-up copies and clears; toast "● copied to clipboard"
  (green border box, 2 s).
- Clipboard: native first (`wl-copy` if WAYLAND_DISPLAY, `xclip -selection clipboard -in`,
  `xsel --clipboard --input`, macOS `pbcopy`), OSC 52 `\x1b]52;c;<b64>\x07` fallback and
  preferred over SSH.
- Copy mode (keyboard): hjkl/arrows, w/b/e, 0/$/^, g/G, PgUp/PgDn, C-u/C-d, v/space char select,
  V line select, / ? n N search, y/Enter copy, q exit, Esc clear then exit. Cursor bg=accent.
  Bottom bar shows " COPY ".
- Paste: bracketed paste → focused text input if any, else focused pane.

## Hierarchical collapse
- Collapsed state is a `HashSet<key>` (not stored on nodes); toggle = remove-or-insert; persisted.
- Visible tree flattened to `Vec<Entry>` each render (parent, then children with `last_child`);
  collapsed parent emits only itself. Scroll and hits computed over the flat list.
- Glyphs `▸` collapsed / `▾` expanded (accent); children connectors `├─ └─ │` in overlay0.
  Collapsed parent shows the most urgent child status.
- Click toggle glyph to toggle; context menu "Expand/Collapse".
- Row styles: focused bg active_row_bg + text BOLD; cursor bg selection_bg; normal subtext0.

## Modals / menus
- One overlay at a time (Option), drawn last; hit rects copied to hit map.
- Full modals dim everything behind (`Modifier::DIM` on all cells); menus don't.
- Panel: fill bg panel_bg, remove DIM, square border `┌─┐│└┘` in accent (red for destructive).
- Centred: w=min(w, W-4), h=min(h, H-2).
- Buttons: primary `bg=accent fg=panel_bg BOLD` e.g. `" ↵ confirm "`; secondary `bg=surface0`;
  top-right `" esc close "`.
- Context menu at pointer, clamped on screen; width=longest+4 (min 14); highlighted row
  `fg=panel_bg bg=accent BOLD`. Esc close, ↑/↓/j/k move, Enter activate, hover highlights,
  click activates, click outside closes.
- Help overlay lists grouped keybindings with `/` filter.

## Keys
- Prefix key (ctrl+b) + direct bindings. Modes: Terminal (keys go to pane), Prefix (one key),
  Resize, Copy. Mode bar on bottom row only while in a non-normal mode: pill `" PREFIX "`
  (bg accent), keys accent+BOLD, descriptions overlay0.

## Render loop
- Render is pure; a compose step computes geometry + hit map; repaint only when something
  changed; ≤ 60 fps (16 ms min interval); throttle drag updates.
- Side effects (clipboard, docker calls) are returned as actions and executed after handling.

## Palette (Catppuccin Mocha)
accent 137,180,250 · panel_bg 24,24,37 · active_row_bg 30,30,46 · surface0 49,50,68 ·
surface1 69,71,90 · surface_dim 30,30,46 · overlay0 108,112,134 · overlay1 127,132,156 ·
text 205,214,244 · subtext0 166,173,200 · mauve 203,166,247 · green 166,227,161 ·
yellow 249,226,175 · red 243,139,168 · sidebar bg = terminal default.
