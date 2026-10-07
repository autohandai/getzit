# ADR 13: Ignore tool by-products; bound the cost of awareness

Built-in and project ignore patterns for every workspace; live write sets reused for two seconds; the claim scan moved out of the lock.

**Status:** accepted. Amends [ADR 5](0005-materialisation.md) and [ADR 11](0011-claims-and-awareness.md). Came out of [Lesson 02](../docs/lessons.mdx).

Amended by [ADR 23](0023-workspace-slots-ports-liveness.md).

## Context

With 30 agents and 20 developers on one repository:

- Every agent that ran a Python script left `__pycache__/*.pyc` behind. The project did not ignore it, so it was recorded, landed in `main`, and made nine later changes stale.
- `zit status` and `zit claim` recomputed every open workspace's live write set, at about four git processes each, on every call. With 30 agents calling them, the machine's load average reached 407 and agents waited minutes. Claims did the scan while holding the claims lock, so they also queued.
- Inside Codex's sandbox, probing a workspace owner with `kill(pid, 0)` returns `EPERM`, which Zit read as "process gone".

## Decision

**Ignores.** Every git call Zit makes in a workspace uses an exclude file built from:

1. the user's global excludes (`core.excludesFile`, or `~/.config/git/ignore`),
2. built-in patterns for files that are never source: `.DS_Store`, `__pycache__/`, `*.py[cod]`, `.pytest_cache/`, `.mypy_cache/`, `.ruff_cache/`, `*.swp`,
3. the project's `ignore = [...]` in `zit.toml`.

As with any gitignore, tracked files are unaffected.

**Awareness window.** A workspace's live write set is stored with the time and the base it was computed from, and reused for 2 seconds. A different base (the workspace recorded) invalidates it immediately. The information is advisory: acceptance still validates exactly, whatever a claim saw.

**Claim lock.** The slow part (live write sets and recorded changes' writes) is computed before the lock. Under the lock only the claims files are read and written, so of several racing claims exactly one still wins.

**Liveness.** `EPERM` from `kill(pid, 0)` means the process exists.

**Sandboxed agents.** `zit run --agent codex` grants Codex's sandbox the repository's Zit home and git directory (`--add-dir`), which `zit claim` and `zit status` write to. The live snapshot's temporary index is kept in the workspace's temp directory, not its git admin directory, which Codex protects. A workspace whose edits cannot be seen is reported when `ZIT_TRACE` is set.

## Consequences

- 30 open workspaces, all edited (`bench/awareness.sh`): repeated `status` 552 → 174 ms, a claim 566 → 165 ms, ten simultaneous claims 3,022 → 296 ms.
- Re-run of the simulation: no `.pyc` in the final state, peak load 69, no agent reported waiting on Zit.
- A claim can be refused or granted on a write set up to 2 seconds old.
- A by-product that someone does want tracked must already be tracked; ignoring never hides tracked files.
