# lazierdocker

A mouse-first terminal UI for managing Docker. It has [lazydocker](https://github.com/jesseduffield/lazydocker)'s
features with [herdr](https://github.com/herdrdev/herdr)'s terminal interaction model: drag-to-resize split panes,
a collapsible resource tree, right-click context menus, modal menus, and text selection that copies to the
clipboard. Shells, `docker attach` and logs run in embedded terminals powered by
[libghostty](https://github.com/ghostty-org/ghostty).

```
 lazierdocker                          ┌─ demo › worker ──────────────────┬─ exec loner ───────────────────┐
▾ Projects 1                           │ Logs   Stats   Env   Config   Top │ / # ls /                       │
 ▾ demo 3/3 ●                          │ CPU (%): 0.08 (2m10s)             │ bin  dev  etc  home  lib  ...  │
   ├ ▶ db                         0.33%│     1.0 │                         │ / #                            │
   ├ ▶ web                        0.00%│         │          ▂▅▄▅▃▅▆▅▄▃▅▄▅  │                                │
   └ ▶ worker                     0.08%│     0.0 │▃▄▅▃▄▅▄▃▅▄▃▄▅▄▃▅▄▅▄▃▅▄▅  │                                │
▾ Containers 2                         │                                   │                                │
  ▶ loner                         0.00%│ Memory (%): 0.00 (2m10s)          │                                │
  ⨯ exited (0)                         │ …                                 │                                │
▸ Images 4                             │                                   │                                │
 ↑↓ select  ←→ fold  / filter  e exited  enter pane  ^b window  q quit                tcp://127.0.0.1:23750
```

## Features

**Resources (lazydocker parity)**
- **Compose projects:** discovered from container labels, plus the compose project in the current directory (marked ●).
- **Services:** with the status of their containers.
- **Other resources:** standalone containers, images, volumes and networks, each in a collapsible section of the
  sidebar tree. Projects can also be collapsed.
- **Inspector tabs:**
  - Containers and services: Logs, Stats (CPU and memory graphs), Env, Config (summary plus the full inspect
    output as YAML) and Top.
  - Projects: Logs and the compose config.
  - Images: Config with layer history.
  - Volumes and networks: Config.
  - Section headers: a summary table.
- **Actions:**
  - Containers: start, stop, restart, pause, remove (with volumes, or force if the container is running).
  - Compose services and projects: up, stop, restart, recreate, rebuild, `rm`, and down (optionally with volumes and images).
  - Images, volumes and networks: remove and prune.
  - Bulk menus.
  - Open a container's published port in a browser.
  - Destructive actions ask for confirmation, as lazydocker does.
- **Refresh:** driven by the Docker events stream, plus a CPU column from live stats streams.

**Interaction (herdr-style)**
- **Mouse:** click to select, focus and switch tabs. Drag split borders and the sidebar edge to resize.
  Double-click the sidebar edge to reset its width. The scroll wheel scrolls whatever is under the pointer.
- **Context menus:** right-click on any item or pane. Menus highlight on hover.
- **Split panes:** split with `ctrl+b v` / `ctrl+b -`. A new pane is pinned to the selected item, so you can watch
  the logs of one container while browsing others. You can also zoom, swap, close and change focus by direction.
- **Embedded terminals (libghostty):**
  - `E` opens an exec shell; `a` attaches to the container.
  - Attached compose commands (rebuild, pull, …) also run in a pane.
  - Keys, mouse, paste (bracketed) and scrollback all work. `ctrl+b` is the only key the app keeps.
- **Copy and paste:**
  - Drag to select text in any pane. It is copied on release.
  - Double-click selects a word or URL.
  - `ctrl+b [` enters keyboard copy mode (vi-style motions, `v`/`V`, `y`).
  - Copying goes to the clipboard via wl-copy, xclip, xsel or pbcopy, falling back to OSC 52 (which is
    preferred over SSH).
- **Help:** press `?` or `x` for a filterable list of keybindings.

## Keys

| Where | Key | Action |
|---|---|---|
| anywhere | `ctrl+b` | window prefix (then `v` `-` `x` `z` `h/j/k/l` `H/J/K/L` `o` `r` `[` `b` `q` `?`) |
| sidebar | `↑↓` `j k` | select |
| sidebar | `←→` `h l` / `space` | collapse / expand |
| sidebar | `1`–`5` | jump to a section |
| sidebar | `/` | filter |
| sidebar | `e` | toggle exited containers |
| sidebar, pane | `[` `]` | previous / next tab |
| pane | `j k` `PgUp PgDn` `g G` | scroll (`G` re-enables follow) |
| pane | `h l` `← →` / shift+wheel | scroll long lines sideways (`›` marks cut-off lines) |
| pane | `f` | logs: load the full history (logs open with the last 1000 lines; projects 300) |
| pane | `/` `ctrl+f` | logs: find (smart case); `enter` keeps the matches, `esc` clears |
| pane | `n` `N` | logs: older / newer match; repeats the last search after `esc` |
| pane | `ctrl+g` `F3` `ctrl+r` `ctrl+p` | logs: older match, also while typing (as is `↑`) |
| pane | `ctrl+shift+g` `shift+F3` `ctrl+s` `ctrl+n` | logs: newer match, also while typing (as is `↓`) |
| containers | `s` `r` `S` `p` `d` | stop, restart, start, pause, remove… |
| containers | `E` `a` `m` `w` `b` | exec, attach, logs pane, browser, bulk… |
| services | `u` `R` `U` `D` | up, restart menu, up project, down… |
| images, volumes, networks | `d` `b` | remove…, prune… |
| anywhere | `?` / `x` | help |
| anywhere | `q` | quit |

## Usage

    lazierdocker [-H HOST] [-p PROJECT] [-f COMPOSE_FILE]...

The Docker host is chosen the way the docker CLI chooses it: `-H`, then `$DOCKER_HOST`, then the current docker
context (`docker context use …`, so Colima, OrbStack and rootless setups work), then `/var/run/docker.sock`. Only
`unix://` and `tcp://` hosts are supported. Anything else, such as `ssh://` or TLS, is refused rather than silently
replaced with another daemon. Compose actions use the
`docker compose` CLI. For projects outside the current directory, lazierdocker finds their compose files
through the `com.docker.compose.project.config_files` label.

## Building

Requires Rust (stable) and **Zig 0.15.2** on `PATH`. The [`libghostty-vt`](https://crates.io/crates/libghostty-vt)
crate uses Zig to build libghostty-vt from a pinned Ghostty commit, which the first build downloads from GitHub.

    make                      # optimised build → target/release/lazierdocker
    make run ARGS="-H tcp://127.0.0.1:23750"
    make install              # → ~/.local/bin/lazierdocker (PREFIX=/usr/local for system-wide)
    make uninstall            # same PREFIX as install

`make test` and `make clean` are also available.

Linux and macOS are supported.

## Testing

    cargo test

The integration tests in `tests/docker_integration.rs` run only when `LAZIERDOCKER_TEST_DOCKER_HOST` points at a
disposable daemon, because they restart containers. To set one up:

    docker run -d --privileged --name lazierdocker-test-dind -p 127.0.0.1:23750:2375 \
        -e DOCKER_TLS_CERTDIR= docker:dind --host=tcp://0.0.0.0:2375
    DOCKER_HOST=tcp://127.0.0.1:23750 docker compose -f tests/fixtures/demo/compose.yml up -d
    DOCKER_HOST=tcp://127.0.0.1:23750 docker run -d --name loner busybox sleep 100000
    DOCKER_HOST=tcp://127.0.0.1:23750 docker run --name exited alpine:3.20 echo bye
    LAZIERDOCKER_TEST_DOCKER_HOST=tcp://127.0.0.1:23750 cargo test

## Credits

- Feature set modelled on lazydocker (MIT).
- Interaction techniques modelled on herdr (Apache-2.0).
- Terminal emulation by libghostty-vt from Ghostty (MIT), through the
  [libghostty-rs](https://github.com/Uzaaft/libghostty-rs) bindings (MIT OR Apache-2.0).
