# lazierdocker

A mouse-first terminal UI for managing Docker, with lazydocker's features and herdr's
terminal interaction model. Embedded terminals are powered by libghostty.

## Building

Requires Rust (stable) and **Zig 0.16.0** on `PATH` (used to build the vendored libghostty-vt).

    cargo build --release
