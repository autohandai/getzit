# ADR 1: The graph is git objects and refs

A state is a git tree, a change is a git commit, refs/zit/* is the graph.

**Status:** accepted

## Context

The architecture needs a persistent, content-addressed store of program states and the changes between them, usable from any machine, and it must interoperate with git.

## Decision

- **State** = a git **tree**. Its id is the tree id: identical programs have identical ids.
- **Change** = a git **commit**. Its tree is the resulting state, its parents are the changes it builds on, its message holds the intent, and `Zit-*` trailers hold agent, session and declared reads.
- **The graph** = refs under `refs/zit/`:
  - `refs/zit/current` — the one accepted change.
  - `refs/zit/changes/<id>` — each speculative change.
  - `refs/zit/evidence/<key>` — each evidence record (a blob).

Nothing durable lives outside git. `~/.zit` holds only workspaces and caches, all rebuildable.

## Consequences

- Any git commit is already a valid change (`zit accept main` works). Any change is already a valid git commit (`zit export` is a ref update).
- Replication is `git push`/`git fetch` of `refs/zit/*`. Verified by `tests/replicate.rs`.
- Branches are not a primitive. A "branch" is any commit with more than one child.
- Change metadata is immutable. Anything mutable must be derived ([ADR 2](0002-status-is-derived.md)).
