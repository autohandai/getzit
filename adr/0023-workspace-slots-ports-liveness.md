# ADR 23: Workspaces: stable paths, a port each, liveness by lock

Workspaces can be reused at fixed paths; each gets a free TCP port; a workspace is running while its `zit run` holds a lock; orphans are disposed on request; an agent can set the intent of its change and keep its output with it.

**Status:** accepted. Amends [ADR 5](0005-materialisation.md), [ADR 8](0008-agent-integration.md), [ADR 13](0013-byproducts-and-awareness-cost.md) and [ADR 17](0017-team-rules-and-cost.md).

## Context

ADR 10 left two things open: agent workspaces are created at fresh paths, so each agent rebuilds a compiled project once; and ports are machine-wide state that `TMPDIR` does not isolate. A hostile pass over workspace handling found that liveness by `kill(pid, 0)` (ADR 13) took a reused pid for a running agent; that a workspace whose `zit run` had died stayed until someone disposed it by id; that an agent could say what its change was for only through the `--intent` it was started with; and that nothing kept what the agent printed.

## Decision

- **Stable paths.** With `[workspace] stable_paths = true` in the `zit.toml` of the change being materialised, or `git config zit.stablePaths true`, a workspace lives at `<home>/ws/slot-N` with id `slot-N`, the lowest slot free. Dispose keeps `tree/` and `git/` and renames `meta.json` to `retired.json`; the next materialisation in the slot does `git reset --hard` and `git clean -fd`: tracked files reset, untracked files removed, ignored files kept. The slot is locked by `ws/.slot-N.lock`.
- **A port each.** At materialisation a free TCP port is chosen (bind `127.0.0.1:0`, retried so that no live workspace or view holds it), stored as `port` in `meta.json`, exported as `ZIT_PORT` to `zit run` agents and to checks, and shown by `status --json` and `materialise --json`.
- **Liveness by lock.** `zit run` holds a `flock` on `ws/<id>/run.lock` while it runs. `clean`, `dispose --orphaned` and the `alive` field probe the lock, retrying for up to 250 ms because a process spawned on macOS briefly holds every open descriptor. Where no lock is held, `meta.json` also records `pid_started` (macOS `proc_pidinfo`, Linux `/proc/<pid>/stat` field 22), and an owner is alive only when the pid exists and its start time matches. Metadata without the field falls back to the pid.
- **Orphans.** `zit dispose --orphaned` deletes every workspace whose owner is gone and that has no unrecorded edits; `--force` also those with edits. A workspace without an owner (made by hand or through MCP) is never touched. Output: `disposed <id> (<agent>)`, `kept <id> (<agent>): unrecorded edits; record it or pass --force`, or `no orphaned workspaces`; JSON `{disposed, kept}`.
- **`ZIT_INTENT_FILE`.** `zit run` exports the path `<ws>/intent.txt` (`{ZIT_INTENT_FILE}` in a preset); non-blank content there overrides `--intent` as the recorded intent.
- **Run logs.** `zit run --log` stores the last 64 KB of the agent's stdout and stderr as a blob at `refs/zit/logs/<change id>`. `zit show CHANGE --log` prints it (`--json`: `{change, log}`); without one, exit 2: ``zit: no log recorded for <id>: run the agent with `zit run --log` ``.

## Consequences

- Stable paths are off by default. With them an agent inherits the previous occupant's ignored files: a warm build, and stale caches. A different `[prepare]` key rebuilds the slot. Slots grow to the peak concurrency; `zit clean` removes them all.
- A port is chosen, not reserved: another program can take it before the agent binds.
- `status` with N running agents spends up to N × 250 ms on liveness probes.
- The relative order of stdout and stderr in a run log is not preserved. `discard` does not delete the log ref.
- ADR 17 said Pi reported no usage. The Pi preset is now `pi --mode json -p <intent>`, and `zit run` reads tokens (input, cache read and cache write in; output out) and `usage.cost.total` from its `message_end` events; the last text is the account. Autohand Code still reports nothing parseable.
