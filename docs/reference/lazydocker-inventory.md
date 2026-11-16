# lazydocker feature inventory (reference for parity)

Studied from jesseduffield/lazydocker HEAD 596fd59 (2026-11-15), MIT. This checkout supports
**multiple compose projects**: the Project panel lists projects discovered from container labels,
and Services/Containers are filtered by the selected project.

## 1. Side panels
Order: [1] Project, [2] Services, [3] Containers ("Standalone Containers" when project-scoped and
showAllContainers=false), [4] Images, [5] Volumes, [6] Networks.

Project-scoped = cwd is a compose project (`docker compose config --quiet` succeeds) or `-p` given.
If not scoped: Project+Services hidden, containers flat, focus starts on Containers.

**Project**: names = distinct `com.docker.compose.project` labels across all containers (sorted),
plus the local project prepended if it has no containers. Selecting re-filters Services/Containers.

**Services**: union of `docker compose config --services` (cwd, local project, may have no
container) and (project, service) pairs from container labels (skip oneoff). Container mapping =
first non-oneoff container with matching `com.docker.compose.service` + `.project`.
Labels used: `com.docker.compose.service`, `.project`, `.container-number`, `.oneoff == "True"`.
Sort: services with a container first, then by name. Row = same columns as containers; with no
container show `none` in blue.

**Containers** columns: state, substatus (`(exitCode)` exited / `(health)` running), name,
CPU% (`%.2f%%`, red >90, yellow >50), ports (yellow; `[ip:]public->private/proto`, omit ip when
0.0.0.0, sorted, joined ", "), image (magenta, strip `sha256:`).
State colors: exited yellow if code 0 else red; created cyan; running green; paused yellow; dead
red; restarting blue; removing magenta. Health healthy/unhealthy/starting = green/red/yellow.
Sort: running, exited, created, then name (fix lazydocker quirk: other states should not sort first).
Filter: `e` toggles showing exited. When scoped and !showAllContainers, hide containers belonging to
a known service; hide containers of other projects (no-project containers always kept).

**Images**: name, tag, size. From RepoTags[0] split on last ':'; untagged → `<none>`.
Sort name, tag, id (put `<none>` last).
**Volumes**: driver, name; labelled first, then name. **Networks**: driver, name; by name.

**Filtering**: `/` opens filter, case-insensitive substring over row text (lazydocker is
case-sensitive over ANSI cells — we improve). Enter commits, Esc clears. Changing panel clears.

## 2. Main panel tabs (per-panel tab index; `[` `]` cycle; click tab)
| Panel | Tabs |
|---|---|
| Project | Logs, Compose Config, About |
| Services | Logs, Stats, Env, Config, Top |
| Containers | Logs, Stats, Env, Config, Top |
| Images / Volumes / Networks | Config |

- **Logs (container)**: SDK logs {stdout, stderr, timestamps, since=60m, tail, follow}. TTY → raw;
  else demux. When stream ends, poll until running again. Autoscroll.
- **Logs (project)**: `docker compose -p P logs --tail=300 --follow`.
- **Stats**: graphs every 1s: "CPU (%)" cyan, "Memory (%)" green; caption
  `"<caption>: <last %.2f> (<history span>)"`; then PIDs, Traffic received/sent (eth0), then YAML of
  latest stats. CPU% = Δcpu.total*100/Δsystem (lazydocker doesn't scale by online CPUs; docker CLI
  does: ×online_cpus — we follow docker CLI). Mem% = usage*100/limit. History maxDuration 3m.
- **Env**: `KEY:` green, value yellow, from inspect Config.Env.
- **Config (container)**: ID, Name, Image, Command, Labels (sorted), Mounts, Ports, then
  "Full details:" inspect as colored YAML.
- **Top**: container top every 1s as table; "container is not running" when stopped.
- **Image Config**: Name, ID, Tags, Size, Created, then history table ID/TAG/SIZE/COMMAND
  (strip `/bin/sh -c #(nop) `).
- **Volume Config**: Name, Driver, Scope, Mountpoint, Labels, Options, Status, RefCount/Size.
- **Network Config**: ID, Name, Driver, Scope, EnabledIPv6, Internal, Attachable, Ingress,
  Containers (name: endpoint), Labels, Options.
- **Compose Config**: `docker compose config` output as colored YAML (local project only).

## 3. Actions
**Containers**: `d` remove menu (remove / remove with volumes; on "must stop" offer force);
`e` toggle stopped; `p` pause/unpause; `s` stop (confirm); `r` restart; `a` attach
(`docker attach --sig-proxy=false <id>`, needs OpenStdin, not exited); `m` view logs;
`E` exec shell (`docker exec -it <id> /bin/sh -c 'eval $(grep ^$(id -un): /etc/passwd | cut -d : -f 7-)'`);
`c` custom commands; `b` bulk; `w` open first published port in browser (0.0.0.0→localhost).

**Services** (compose CLI with `-p <project>`): `u` up service; `d` remove menu
(`rm --stop --force svc` / `rm --stop --force -v svc`); `s` stop (confirm); `p` pause; `r` restart;
`S` start; `a` attach; `m` logs; `U` up project (confirm); `D` down menu (down / down --volumes);
`R` menu restart / recreate (`up -d --force-recreate`) / rebuild (`up -d --build`, attached);
`c` custom; `b` bulk; `E` exec; `w` browser. Non-local-project services: fall back to SDK on the
container for s/r/S; refuse compose-file ops.

**Project**: `e` edit config, `o` open config, `m` all logs.
**Images**: `d` menu: rm / --no-prune / --force / --no-prune --force; `b` bulk (prune); `c` custom.
**Volumes**: `d` menu remove / force remove; `b` (prune). **Networks**: `d` remove; `b` (prune).

Compose templates: up `up -d`; down `down`; downWithVolumes `down --volumes`; upService
`up -d S`; rebuild `up -d --build S`; recreate `up -d --force-recreate S`; stop `stop S`;
restart `restart S`; start `start S`; logs `logs --follow S`; allLogs `logs --tail=300 --follow`;
config `config`; top `top S`. `docker compose`, falling back to `docker-compose`.

## 4. Menus & confirmations
Confirm (Enter/y vs Esc/n): container stop, service stop, project up, force-remove retry, all
prunes, stop all, remove all, quit (if confirmOnQuit). Errors show in a red "Error" popup.
Option menus: remove menus, restart menu, down menu, custom, bulk, help (`x`/`?`). Menus get a
trailing "cancel" item; rows show label + command (cyan; red for removals).
Bulk containers: stop all, remove all (forced), prune exited + user commands. Bulk services:
up, up (attached), stop, pull (attached), build --parallel --force-rm (attached), down,
down --volumes, down --rmi all, down --volumes --rmi all. Images/Volumes/Networks: prune.
`X`: free-text command prompt, run via `bash -c` (attached).

## 5. Docker access
SDK for list/inspect/start/stop/restart/pause/unpause/remove/top/logs/stats/prune/events.
CLI for compose, attach, exec. Host: DOCKER_HOST → DOCKER_CONTEXT/current context → default.
Refresh: docker events stream; any event → full refresh throttled to 50 ms; on stream error show
error, retry every 2 s. Polls: inspect every container 1s; CPU column re-render 1s.
Stats: one streaming stats task per container; history trimmed to maxDuration.

## 6. Config highlights
gui: scrollHeight 2, theme activeBorderColor green bold, inactive default, selectedLineBg blue,
optionsTextColor blue; showAllContainers false; wrapMainPanel true; sidePanelWidth 0.3333;
screenMode normal|half|fullscreen; containerStatusHealthStyle long|short|icon;
border rounded|single|double|hidden. logs: timestamps false, since 60m, tail "".
stats.graphs config; replacements.imageNamePrefixes; ignore[]; customCommands/bulkCommands per
type with {name, attach, shell, command, serviceNames}. CLI flags: -c, -d, -f (repeat), -p.

## 7. Global keys
q/Ctrl-C quit; Esc clear filter/back; x/? keybinding menu; X run command; PgUp/PgDn/Ctrl-U/Ctrl-D
scroll main; Home/End top / re-enable autoscroll; +/_ screen mode; 1–6 focus panel;
←/→/h/l/Tab move panels; j/k/↑/↓ select; Enter focus main; [ ] tabs; / filter.
Bottom line: key hints, status spinner, version.
Layout: portrait (side above main) when width ≤ 84 and height > 45; small heights collapse
unfocused panels.
