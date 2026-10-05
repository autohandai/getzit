# ADR 5: Workspaces are copy-on-write clones presented as git views

A workspace is a disposable directory cloned from a cached checkout, with an unregistered git admin directory.

**Status:** accepted

## Context

Agents need a real directory, and their tools expect it to be a git repository (Codex refuses to run outside one). `git worktree` gives that, but registers the worktree in the repository, wants a branch, and checks out every file.

## Decision

A workspace lives at `~/.zit/<repo>/ws/<id>/`:

```
tree/        the files the agent edits; tree/.git is a pointer file
git/         HEAD, index, commondir -> the repository's .git
meta.json    base change, intent, agent, session, owner pid
reads        declared reads, one per line
claims       claimed resources, one per line
inflight.json  what it was writing, and when that was computed (ADR 13)
tmp/         the workspace's own TMPDIR
git/zit-ignore  excludes used by every git call zit makes here (ADR 13)
```

- The `git/` directory makes `tree/` a working git view (`git status`, `git diff`, `git log` work) without registering a worktree or creating a branch.
- Files come from a cache of pristine checkouts, `~/.zit/<repo>/trees/<state>/`, cloned with APFS `clonefile(2)`. A state not in the cache is produced by cloning the nearest cached state and updating only the paths that differ.
- The cached git index is cloned along with the files, and git is told to trust size and mtime (`core.checkStat=minimal`), so recording does not re-hash unchanged files.
- Where the filesystem cannot clone, the fallback is a plain checkout from the object database.

## Consequences

- `git worktree list` and `git branch` stay untouched. Verified by `materialise_registers_no_git_worktree_and_no_branch`.
- A workspace can be deleted at any time; recorded changes are unaffected.
- The clone strategy is implemented and tested on macOS/APFS only. Other platforms use the checkout fallback. A Linux reflink strategy is not implemented.
- `record` snapshots the files, whatever git commands the agent ran. But the workspace shares the repository's refs: `git checkout -b`, `git stash` or `git commit` on a branch inside a workspace create those refs in the real repository.
- The cache keeps the two newest states. Clones take a shared `flock`, eviction an exclusive one, so an entry is never deleted while another process is cloning it.
