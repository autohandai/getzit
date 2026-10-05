# ADR 7: Git is driven through its CLI

Zit calls git plumbing commands rather than linking a git library.

**Status:** accepted

## Decision

Every object and ref operation is a `git` plumbing call (`commit-tree`, `merge-tree --write-tree`, `diff-tree`, `update-ref --stdin`, `cat-file --batch`). Requires git 2.38 or newer. `$ZIT_GIT` selects the binary.

## Why

- Composition must match git exactly, or "git would merge this" stops being a meaningful statement. `git merge-tree` *is* git's merge.
- No second implementation of the object format, index or ref transactions to keep in step with git.

## Cost, measured

Each git call is a process; how long one takes on the benchmark machine is printed with every result in [Benchmarks](../docs/benchmarks.mdx). Counted with `ZIT_TRACE`:

| Operation | git calls |
|---|---:|
| `materialise` | 1, or 2 when the state is not cached |
| `record` | 6 |
| `accept`, fast-forward, one check to run | 8 |
| `accept`, composing, one check to run | 10, plus 3 when current's footprint is not cached yet |

A worktree workflow needs 4 calls per agent turn (`worktree add`, `add`, `commit`, `worktree remove`) and 1 to merge. Zit's per-change overhead is therefore higher than raw git's; the benchmarks show by how much.

`ZIT_TRACE=1` prints each git call and its duration; the one or two `cat-file --batch` read sessions per operation are counted above but not printed.

## Consequences

- If per-operation latency matters more than exact parity, the calls in `src/git.rs`, `src/accept.rs` and `src/footprint.rs` are the seam for an in-process object database.
