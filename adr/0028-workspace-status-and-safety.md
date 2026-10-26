# ADR 28: Status explains every workspace's task, diff and readiness

**Status:** accepted. Amends [ADR 2](0002-status-is-derived.md),
[ADR 13](0013-byproducts-and-awareness-cost.md), and [ADR 25](0025-status-memoised.md).

## Context

Agents use both Zit views and ordinary Git worktrees. An inventory that omits
the latter, hides the task, or equates a cached empty footprint with cleanliness
cannot answer whether to integrate or dispose of their work. A verified change's
own state also does not prove that its composition with current has passed checks.

## Decision

`zit status` reports tasks, physical diff sizes and independent, reasoned `merge`
and `cleanup` assessments. The JSON/MCP overview retains schema 1 and adds fields:
`changes[].diff/merge`, `workspaces[].diff/merge/cleanup/inspection_error`, and
`git_worktrees[]`. The CLI, TUI and local web ledger present this same model.

Zit workspaces retain their explicit intent, agent and session. Ordinary worktrees
are enumerated from `git worktree list --porcelain -z`, including the primary,
bare, detached, locked, missing and prunable entries. They remain separate from
Zit's unregistered views. Task metadata comes from `branch.<name>.zit-task`, then
the standard branch description, then a clearly labelled inference from the HEAD
commit subject. Agent identity comes only from `branch.<name>.zit-agent` or a
`Zit-Agent` trailer. An absent identity remains unknown. Non-UTF8 worktree paths
have a display string plus `path_bytes` in machine reports.

Diffs count files, added lines, removed lines, and binary files. Renames are
represented as delete/add, so no similarity heuristic changes the count. Managed
views compare final file contents with their recorded base state. Ordinary
worktrees compare final contents with the merge base of HEAD and the explicitly
reported integration target, `refs/zit/current`. Both include non-ignored
untracked files. Staged-only changes still block disposal even when the final
files match the base. Ignored build outputs are excluded from these diffs.

Live contents are captured with a disposable index and object store; status
does not stage the real index, record a change, move refs, or execute checks.
Filters, hooks and fsmonitor are disabled for inspection. Configured custom merge
drivers cause an unknown assessment rather than executing repository commands.
Mutable contents are inspected freshly on each status request; the two-second
awareness cache remains available to claims, but is not evidence for safety.
Failed inspections retain their row, a null diff, an error and unknown readiness.

Assessments use these states:

| State | Meaning |
|---|---|
| `ready` | The snapshot passes the relevant preview or workspace-disposal guards. |
| `blocked` | A known condition must be resolved first. |
| `review` | Further checks or human evidence are required. |
| `not-needed` | Work is already included in current, or there is no checkout to merge. |
| `unknown` | Inspection could not establish the required facts. |

Unrecorded managed edits must be recorded before merge. A recorded base uses the
existing acceptance dry run, including required checks of the composed state.
Missing evidence yields review; staleness, conflicts and failures block. A live
owner blocks both merge and disposal. A clean managed view whose recorded owner
has finished is ready for disposal: the recorded change survives it. Ownerless
manual/MCP views require an external-activity review. This assessment covers that
view, not whole-repository `zit clean` locks or unrelated workspaces.

Ordinary worktree merge previews use Git's three-way merge and always require
review where the tip is not already retained: a text merge does not provide Zit
acceptance evidence. Cleanup blocks primary checkouts, locks, unfinished Git
operations, edits, and tips not retained by ancestry in current. Patch equivalence
alone does not establish retention. A clean retained ordinary worktree still
requires review of ignored files and agent activity, which Git cannot establish.

## Consequences

- Status is advisory and point-in-time. Files, ownership and current can change
  immediately after inspection. `accept`, `dispose` and `clean` retain their own
  behavior; status does not perform them or grant authority to bypass guards.
- Fresh snapshots cost additional Git processes and hashing, proportional to the
  number and size of checkouts. The old warm-status benchmark does not describe
  this expanded inventory. Immutable acceptance status remains memoised.
- Existing integrations retain their fields and gain additive data. Branch task
  metadata is local Git configuration and does not modify commits or refs.
- Regression evidence lives in `tests/status_overview.rs`, `tests/git_worktrees.rs`,
  `tests/ui.rs` and `tests/web.rs`, using real Git repositories and the public CLI/API.
