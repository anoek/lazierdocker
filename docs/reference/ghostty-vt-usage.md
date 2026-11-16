# Using the vendored ghostty-vt crate (from herdr, Apache-2.0)

Source: herdr `crates/ghostty-vt` + `vendor/libghostty-vt` (libghostty-vt 1.3.2-HEAD+44f2a44df,
herdr patches already applied). Vendored verbatim; see NOTICE.

## Build
- Layout must stay `<root>/crates/ghostty-vt` + `<root>/vendor/` (build.rs:37 and lib.rs:2564 use
  relative paths).
- Needs **Zig 0.16.0** on PATH (or `ZIG=`). First build fetches the `uucode` zig dep from the
  network into zig's global cache.
- Env: `LIBGHOSTTY_VT_OPTIMIZE` (default ReleaseFast), `LIBGHOSTTY_VT_SIMD`,
  `LIBGHOSTTY_VT_ZIG_SYSTEM_DIR` (offline deps).
- Upstream `portable-pty 0.9.0` is fine on unix (herdr's patches are Windows-only).

## API essentials (crates/ghostty-vt/src/lib.rs)
```rust
Terminal::new(cols, rows, max_scrollback_BYTES)
term.write(&bytes)                      // feed child output
term.resize(cols, rows, cell_w_px, cell_h_px)   // cols first
term.set_write_pty_callback(|b| ...)    // query replies (DA/DSR/...) — MUST be written back to the PTY
term.take_clipboard_writes() / take_bell_count() / take_pwd_changes()
term.mode_get(u16) / mouse_tracking_enabled() / active_screen() / kitty_keyboard_flags()
term.scrollbar() -> {total, offset, len}; scroll_viewport_delta(isize /*neg=up*/);
term.scroll_viewport_bottom(); scroll_viewport_row(row)
term.read_text_screen((col,row),(col,row), rectangle) -> String   // screen-absolute rows
RenderState::new(); rs.update(&term); rs.colors(); rs.cursor(); rs.clean()
rs.populate_row_iterator(&mut RowIterator) -> RowIter { next(), populate_cells(&mut RowCells) }
RowCellIter { next(), basic_data() -> {wide, style}, fg_color(), bg_color(), content_bg_color(),
              grapheme_text_into(&mut Vec<u8>, &mut String) }
KeyEvent::new(); set_action/set_key/set_mods/set_utf8/set_unshifted_codepoint/set_consumed_mods
KeyEncoder::new(); set_from_terminal(&term); set_macos_option_as_alt(true) /*re-set after sync*/; encode(&ev)
MouseEvent / MouseEncoder { set_from_terminal, set_size(cols,rows,1,1), encode }
```
- Modes: 1 app cursor, 1004 focus, 1006 SGR mouse, 1007 alt scroll, 2004 bracketed paste,
  2026 synchronized output. Mouse tracking modes 9/1000/1002/1003 not exported as consts.
- Send: Terminal, RenderState, KeyEncoder, RowIterator, RowCells. None Sync. KeyEvent/MouseEvent/
  MouseEncoder not Send → create per call. Keep Terminal+RenderState in one Mutex.
- No public title getter (parse OSC 0/2 ourselves or add getter).

## Render mapping (per cell)
- wide: SpacerTail → "" ; SpacerHead → " " ; empty → " ".
- fg: style.fg_color (Palette(i)→Indexed(i), Rgb→Rgb) else cells.fg_color() else default(None→Reset).
- bg: content_bg_color() then style.bg_color then cells.bg_color() else default.
- invisible: fg=bg; inverse: resolve defaults from rs.colors() then swap.
- bold→BOLD, italic→ITALIC, faint→DIM, blink→SLOW_BLINK, underlined→UNDERLINED,
  strikethrough→CROSSED_OUT.
- Cursor: rs.cursor().viewport Some && visible → frame.set_cursor_position.
- While mode 2026 set, keep showing last frame.

## Input
- Keys: map crossterm → ghostty key + mods; chars: key = lowercase base, unshifted = base, utf8 =
  produced text, consumed_mods=SHIFT if shift produced it. BackTab = Tab+SHIFT. When kitty flags==0
  and !modifyOtherKeys, remap e.g. modified Enter→Enter. Scroll to bottom on key input.
- Mouse: forward only if mouse_tracking_enabled(); encoder.set_size(cols,rows,1,1), position=(col,row).
- Wheel: tracking on → encoded mouse wheel; alt screen + mode 1007 → arrow keys; else
  scroll_viewport_delta.
- Paste: wrap in ESC[200~ … ESC[201~ when mode 2004.
- Selection: store screen-absolute rows (abs = scrollbar.offset + viewport_row); extract with
  read_text_screen.
- Resize: term.resize then master.resize; flush replies.
- Non-PTY streams (docker logs of non-tty containers): convert `\n` → `\r\n` before feeding.
