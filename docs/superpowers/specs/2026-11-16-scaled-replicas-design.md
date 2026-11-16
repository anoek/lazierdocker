# Scaled service replicas — design

Date: 2026-11-16 · Status: implemented (owner delegated the remaining calls)

## Intent

A compose service can run several containers (`deploy.replicas: 3`, `docker compose up --scale web=3`).
Today lazierdocker shows only one of them: a service maps to a single container (the first running one), the
service's tabs show that container, and the other replicas have no sidebar row at all (they are excluded from
the Containers section too). The only way to see the others is the project's combined compose logs.

The owner wants the replicas visible and their logs readable: a service with replicas can be expanded to list
them; the service row shows **all** replicas combined; a replica row shows just that replica.

## Goals

1. A service with two or more containers gets a ▸/▾ arrow (like projects and sections). Expanded, it lists one
   row per replica. Services with one container look and behave exactly as today.
2. Selecting the service row shows the replicas combined, **where combining makes sense** (owner's choice):
   - **Logs:** every replica's logs in one pane, interleaved, each line prefixed with a coloured replica label.
   - **Stats:** CPU and memory graphs of the sum across running replicas, plus a per-replica table.
   - **Top:** one process table across running replicas, with a CONTAINER column.
   - **Env, Config:** the first replica, saying so ("showing web-1 of 3"). Compose creates replicas
     identically, so summing or merging these is meaningless.
3. Selecting a replica row behaves exactly like a container row today (all tabs, actions, exec, attach, `m`).
4. Combined logs behave like container logs do today: last 1000 lines, `f` loads the full history, find works,
   a replica that restarts resumes on its own, a replica added by a scale-up appears without reopening.
5. Combined log history is in true time order across replicas (not one replica's block after another).

## Non-goals

- Project Logs stay on `docker compose logs` as today (could move to the same mechanism later).
- No aggregation for Env/Config, no per-replica diffing.
- No new actions; replica rows get the container actions that already exist.
- No change for services with a single container, except the restart behaviour below (they share the new
  code path as "one replica").

## Anti-goals (a failure even if it technically works)

- Duplicated or missing log lines where history meets the live stream, or after a replica resumes.
- One replica restarting clears or reloads the whole combined view (losing the others' scrollback).
- Lines from two replicas spliced into one line, or a prefix in the middle of a line.
- Any action applied to "all replicas" without the same confirmation the single-container action has.

## Approaches considered (combined logs)

- **A. `docker compose logs <service>` in a terminal pane** (as project logs work). Small, and compose
  prefixes/colours lines and works without compose files (verified against a real daemon). But it behaves like
  project logs, not container logs: 300 lines per replica, a `docker compose` process per pane, restart
  handling left to compose, history printed one replica's block after another, and two log mechanisms side by
  side (service rows via compose, replica rows via the Docker API).
- **B. Merge Docker API log streams in the app — chosen by the owner.** One stream per replica feeds one
  terminal surface. Same behaviour as container logs (1000 lines, `f`, resume, new replicas), history merged
  in time order, no compose process, and one log mechanism for everything: a plain container is the
  one-replica case. Costs more: per-stream line splitting and prefixing, and a pane tracks several streams.

## Decided design

### Model (`src/docker/model.rs`)

- A **replica** is a non-one-off container with both compose project and service labels.
- `Service` holds all its replicas instead of one `container_id`: an ordered list of container ids, ordered
  by the `com.docker.compose.container-number` label (`LABEL_NUMBER`), then name. `Snapshot::primary` gives
  "the replica a single-container view uses": first running, else first.
- `Snapshot::standalone_containers` stays as it is: replicas belong to their service, never to the
  Containers section.
- Replica label (used in prefixes and the "showing … of N" line): `<service>-<container-number>`, e.g.
  `web-2`; fall back to the container name when the label is missing.
- **A replica is a slot, identified by its container number** (my call). `compose up --force-recreate`
  briefly leaves an old and a new container with the same number; they are the same replica: the slot shows
  the running container (else the first the daemon lists), and a logs pane follows the new container under the same label. So a
  recreate never makes a one-container service look replicated.

### Sidebar (`src/ui/sidebar.rs`)

```
 ▾ demo 2/2
   ├ ▾ web 2/3                       0.18%
   │   ├ ▶ demo-web-1                0.10%
   │   ├ ▶ demo-web-2                0.08%
   │   └ ⨯ demo-web-3 (137)
   └ ▶ db                            0.02%
```

- Service rows with ≥ 2 replicas are expandable; their children are replica rows (depth 3) showing the
  container name, state icon, substatus and CPU, like container rows.
- Replica groups start **collapsed** (my call: the service row already shows them combined, and the tree
  stays as compact as today until someone wants a single replica).
- The service row then shows caret, name, running/total replicas and summed CPU of running replicas.
- The project row and the Projects summary keep today's rule (my call): services with at least one running
  replica, out of all services.
- **Replica rows use `NodeKey::Container(id)`**, so every container behaviour (tabs, actions, menus, exec,
  attach, pinning) works unchanged. The places that infer a row's place from its key alone
  (`section_of` in selection reconcile, `reveal_in_sidebar`) must handle a container that lives under a
  service (look it up in the snapshot) so revealing/pinning a replica expands project and service.
- `e` (hide exited) hides exited replicas as it hides exited standalone containers.
- Filtering matches replica names; a matching service shows all its replicas (existing group rule).

### Tabs on a service row (`src/app/inspect.rs`, `src/doc/format.rs`, `src/app/loader.rs`)

The service target resolves to its replicas. With one replica every tab is the one-replica case of the below
and must look as today (no prefixes, no "of 1" line, no per-replica table).

- **Logs:** combined stream (next section).
- **Stats:** built from the per-container stats histories that already stream for every running container.
  Graphs: CPU % summed across running replicas, aligned per second over the history window; memory as summed
  **usage in bytes** (my call: without a memory limit docker reports host RAM as each replica's limit, so a
  summed-limit percentage would be N times too small). The figures below sum PIDs and traffic. Then a
  per-replica table: replica, CPU, memory usage and % of its limit, PIDs.
- **Top:** `docker top` for each running replica, one table with a leading CONTAINER column; refreshes once a
  second as today. No running replica → "no replica is running".
- **Env, Config:** the single-container view of the first running replica (else first), with a first line
  "showing web-1 of 3 replicas".
- The content revision must not change when only one replica's state changes for Logs (see below); for the
  other tabs, the existing "reload when the target changes" rule applies across the replica set.

### Combined logs (`src/app/run.rs`, plus a new pure module for merging/prefixing)

- A logs pane owns one stream per replica of its target, all writing into one terminal surface. A container
  or replica row is the one-replica case (no prefix).
- **Owner's decision: restarts append everywhere.** Today a plain container's Logs pane reloads when the
  container restarts (state and exit code are in the Logs content revision): output cleared, tail fetched
  again. With this design a restart appends the new run's lines to the existing output, for single
  containers as for replicas, so scrollback survives a restart. Container state therefore leaves the Logs
  revision; the test asserting the reload (`logs_restart_when_container_restarts`) is rewritten to assert
  the append.
- **History:** fetch each replica's logs without following, with timestamps and the tail (1000, or all after
  `f`). Merge all lines by timestamp, keep the newest 1000 overall (all with `f`), strip timestamps, write.
- **Follow:** then follow each replica with timestamps, from its last history timestamp. The API's `since`
  has whole-second precision, so drop followed lines at or before that replica's last written timestamp.
- **Prefixes:** with ≥ 2 replicas (slots) each line starts with the replica label padded to a common width and a
  separator, in one of a small cycle of theme colours per replica. Streams are split into whole lines per
  replica before prefixing; a partial line is held until its newline (and flushed when the stream ends).
  TTY replicas' raw output and non-TTY `\n` → `\r\n` conversion keep today's rules per replica.
- Whether lines are prefixed is part of the pane's content revision (my call): when a service goes from one
  replica to two or back, the pane reloads once so prefixes are all-or-nothing. Nothing else about replica
  changes reloads it.
- **Lifecycle:** each sync compares the pane's streams with the target's current replicas:
  - a new replica (scale-up) starts streaming from its creation;
  - a replica whose stream ended cleanly resumes, from its last timestamp, once a newer snapshot shows it
    running (today's resume rule, per replica);
  - a removed replica's stream simply ends; its lines stay.
  None of these reload the pane or clear other replicas' output. The `[log stream ended]` notice is written
  only when the last stream ends (all replicas stopped); stream errors behave as today.
- Replacing the pane's content (other tab, other target, `f`) stops all its streams.

### Actions on a service row with replicas (`src/app/docker_actions.rs`, `src/app/panes.rs`)

- Compose-based actions already address the service, i.e. all replicas — unchanged.
- The fallbacks used when the project's compose files aren't on this machine (stop, restart, start) apply
  to every replica, behind the same confirmation as today.
- `p` (my call) uses the Docker API with or without compose files, as today: it pauses every running replica,
  or, when none is running, unpauses every paused one. No confirmation, as today.
- `E`/`a` (exec, attach) on the service row use the first running replica and name it in the pane title.
  `w` opens the first published port, preferring running replicas.

### Tests

- Pure unit tests: timestamp parsing and history merge (ordering, tail cut, `f`), the since-boundary dedupe,
  line splitting with partial lines across chunks, prefix padding/colour assignment.
- Model/sidebar: replicas derived and ordered by container number; service expandable only with ≥ 2;
  hidden-exited and filtering; reveal of a replica row.
- App: tabs for a replicated service; one-replica services render exactly as before (existing tests keep
  passing, apart from the restart test above and tests that name removed fields such as
  `Service::container_id`, which are updated without loosening what they check); stats sums; top merge;
  lifecycle decisions (start/resume/stop per replica) as pure state-transition tests.
- Integration (docker-in-docker): give the fixture's `worker` service `deploy.replicas: 2` (service names
  unchanged); assert the snapshot has two replicas and that combined history is time-ordered.

## Left to the builder

- Exact structure of per-pane stream state (replacing `Pane::task` / `logs_ended_at` with a per-replica map),
  and where the pure merge/prefix code lives.
- Colour cycle, separator glyph, label padding rules.
- Stats bucket alignment details and how missing samples are treated within a bucket.
- How default-collapsed service groups are stored (today "absent from `collapsed`" means expanded).
- README and help text updates for the new rows and behaviour.

## Implementation notes (added after building)

- Each stream start has a run number; a stream's end report counts only if it matches the run now feeding
  that container. Reloading the same view (`f`) keeps the content key, and an aborted task can still have
  reported its end; without run numbers that report started a second follower (regression test
  `an_end_reported_by_a_replaced_stream_is_ignored`).
- History written when a view opens is capped at about the terminal's scrollback (8 MiB) and fed in 64 KiB
  pieces, so `f` on a large log never stalls a frame.
- The `[log stream ended]` notice ends its line, as output can now follow it.
- End-to-end check (a throwaway two-replica project on a real daemon): combined history in time order,
  live following without gaps or repeats across four restarts of one replica, a scale-up to three joining
  without a reload, summed Stats and the replica tree.
- Known, not caused by this feature: in the first ~15 s after launch, stats histories can miss samples (the
  combined Stats graph shows a blank stretch that scrolls away). Docker's stats streams were verified
  healthy under the same load (45 concurrent streams); the cause in the app's stats plumbing is not yet
  found.

