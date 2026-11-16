# Using libghostty-vt (crates.io)

Dependency: [`libghostty-vt`](https://crates.io/crates/libghostty-vt) 0.2.2, the safe Rust API from
[libghostty-rs](https://github.com/Uzaaft/libghostty-rs) (MIT OR Apache-2.0, by Uzaaft and Ghostty
maintainer pluiedev). Its `libghostty-vt-sys` crate builds libghostty-vt from Ghostty commit
`a887df42` (1.3.2-dev).

## Build
- Needs **Zig 0.15.2** on `PATH` (Ghostty's `minimum_zig_version` at that commit; the build script
  runs plain `zig`). libghostty-rs's git HEAD has moved to Zig 0.17; the crates.io release has not.
- The first build clones Ghostty into `OUT_DIR` and lets Zig fetch its packages, so it needs network
  access. Offline: `GHOSTTY_SOURCE_DIR` (a Ghostty checkout) and `GHOSTTY_ZIG_SYSTEM_DIR`.
- Zig's optimize mode follows the Cargo profile (dev → Debug); `LIBGHOSTTY_VT_SYS_OPTIMIZE` overrides.
- Linked statically by default.

## Threading
All handles are `!Send + !Sync` (the C API may keep thread-local state). Each `TermSurface` owns its
`Terminal` on the UI thread. The PTY pump thread and log `Feeder`s only send bytes over a channel
and wake the app; `TermSurface::core()` parses queued bytes (at most 4 MB per call) before any
render or query, then writes query replies back to the child.

## API used (src/term)
```rust
Terminal::new(TerminalOptions { cols, rows, max_scrollback })  // max_scrollback is BYTES in Ghostty
                                                                // (PageList.max_size), despite the header
term.vt_write(&bytes); term.resize(cols, rows, 1, 1)
term.on_pty_write(|_, bytes| ..)        // query replies (DA, DSR, …): must reach the child
term.mode(Mode::BRACKETED_PASTE / ALT_SCROLL); term.is_mouse_tracking(); term.active_screen()
term.kitty_keyboard_flags(); term.scrollbar() -> {total, offset, len} (u64)
term.scroll_viewport(ScrollViewport::{Top, Bottom, Delta(isize), Row(usize)})
// text: grid_ref(Point::Screen|Viewport) ×2 → Selection::new(a, b, false)
//       → term.format_selection_alloc(None, FormatOptions{Plain, unwrap, trim, selection})
let snap = rs.update(&term)?;           // RenderState
snap.colors(); snap.cursor_visible(); snap.cursor_viewport(); snap.set_dirty(Dirty::Clean)
let mut it = rows.update(&snap)?; while let Some(row) = it.next() {
    let mut cells = cell_iter.update(row)?; while let Some(c) = cells.next() {
        c.raw_cell()?.wide(); c.graphemes_utf8(&mut s) /* appends: clear first */; c.style()
    }
    row.set_dirty(false)
}
key::Encoder: set_options_from_terminal(&term), then set_macos_option_as_alt(True), encode_to_vec
key::Event: set_action/key/mods/consumed_mods/utf8/unshifted_codepoint
mouse::Encoder: set_options_from_terminal + set_size(EncoderSize{cell 1×1}); mouse::Event position = cell
```

## Render mapping (per cell)
- wide: SpacerTail → "" ; SpacerHead → " " ; empty → " ".
- fg: `style.fg_color` (Palette(i) → Indexed(i), Rgb → Rgb, None → Reset).
- bg: `style.bg_color`, else the raw cell's colour-only content (`BgColorPalette` / `BgColorRgb`).
- invisible: fg = bg; inverse: resolve defaults from `snap.colors()` then swap.
- bold → BOLD, italic → ITALIC, faint → DIM, blink → SLOW_BLINK, underline ≠ None → UNDERLINED,
  strikethrough → CROSSED_OUT.

## Input
- Keys: crossterm → `Key` + `Mods`; chars: key = lowercase base, unshifted = base, utf8 = the
  character (always, even with Ctrl: Ghostty derives control bytes itself and modifyOtherKeys
  needs the text), consumed = SHIFT when shift produced plain text. BackTab = Tab + SHIFT.
- Legacy apps (no Kitty flags, no modifyOtherKeys): Ctrl+Shift+letter → Ctrl+letter, modified
  Enter → Enter. libghostty-vt has no modifyOtherKeys getter, so `keys::is_legacy` compares the
  encoder's Ctrl+Shift+A output with and without `modify_other_keys_state_2`.
- Mouse: forwarded only when `is_mouse_tracking()`. Wheel: tracking → buttons Four/Five;
  alternate screen + mode 1007 → arrow keys; else scroll the viewport.
- Paste: wrap in `ESC[200~ … ESC[201~` when mode 2004 is set.
- Non-PTY streams (logs of non-tty containers): `\n` → `\r\n` before feeding.
