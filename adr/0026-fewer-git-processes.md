# ADR 26: Fewer git processes

Immutable objects are read once per process, the git version is remembered, and the object directory is found without asking git.

**Status:** accepted. Amends [ADR 7](0007-git-through-its-cli.md).

## Context

ADR 7 accepts a process per git call for exact parity with git. Its table predates ADRs 11 to 17. At the start of this pass `bench/calls.sh` counted 10 processes for `record`, 14 for a fast-forward `accept` and 29 for a composing one, and `zit status` on an empty graph ran 5. Many of them read what cannot change.

## Decision

- **`Repo::read_immutable`** remembers `<id>:<path>` content for the life of the process; misses are fetched with one `cat-file --batch` per batch. Refs are never remembered.
- **The footprint cache is split**: `cache/footprint-v3/<base>-<tip>.json` holds the derived writes and refs, `reads.json` the declared reads, so a span is parsed once whatever reads are declared. Staleness takes the `Change` it already has: a single-parent span's reads come from the change, with no `git log`.
- **One `git config` call** (`-z --get-regexp`) reads `user.name`, `user.email` and `commit.gpgsign` for a commit; booleans are read as git reads them (`true`, `yes`, `on`, a non-zero number).
- **A git new enough is remembered** at `$ZIT_HOME/git-ok/<hash of path, size, mtime>` after one successful version check. An old git is never remembered, a replaced binary is rechecked, and `zit clean` keeps the marker.
- **The object directory** is `git_dir/objects`, without a `rev-parse` per dirty workspace.
- **Footprint parsing is parallel**: a span's blobs are read in one session and indexed on several threads when it has 8 files or more.

## Consequences

Counted by `bench/calls.sh` through a wrapper git; the counts include `cat-file` sessions:

| Operation | before | after |
|---|---:|---:|
| `record` | 10 | 7 |
| `accept`, fast-forward, one check to run | 14 | 9 |
| `accept`, composing, one check to run | 29 | 16 |

- `zit status` on an empty graph: 5 → 3 processes, 64.6 → 40.2 ms median of 20 (`bench/startup.sh`). `zit --version` ≈ 3 ms and 0 processes, as before.
- A 200-file `record`: ≈ 520 → ≈ 345 ms over 4 rounds.
- 30 dirty workspaces, cold `status` (`bench/awareness.sh`): 216 → 169 ms, 94 → 63 processes.
- Not changed: the store's own `cat-file` session; `prune_accepted` and `speculative` still list refs twice; `core.excludesFile` is read per workspace; there is no long-lived `cat-file` session, since whether it sees objects written after it started is unproven.
