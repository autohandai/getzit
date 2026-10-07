# Lesson 05: Eight agents, one build directory

Date: 2026-10-07. Harness: eight coding agents in parallel git worktrees of this repository, merged by hand into `main`. Results: the 74 commits between `4e893ad` and the eight merges, and ADRs 18 to 27.

This pass was not an experiment on Zit; it was an improvement pass made with the kind of workflow Zit is for, and it hit the problem ADR 10 left open. It is recorded here because the finding is about Zit, not about this repository.

## The setup

- **Repository:** this one. Check: `cargo clippy` clean and `cargo test --release` green before every commit.
- **Agents:** eight at once, each in its own `git worktree`, each with one area (acceptance, workspaces, symbols, the CLI, performance, and three hostile audits of the first three) and a brief that said: one commit per bug or feature, test first, do not edit the docs.
- **Build state:** the machine had 5 GB free, so the brief told every lane to share one `CARGO_TARGET_DIR` and build the release profile only. Cargo serialises builds in one directory with a file lock; the brief said to wait for it.
- **Integration:** eight `git merge --no-ff` into `main`, one per lane (`merge.sh`), conflicts resolved by hand, then `cargo fmt --check`, clippy and the full test suite after each.

## Result

| | |
|---|---:|
| Lanes | 8 |
| Commits merged | 74 |
| Tests, before → after (`#[test]` in `tests/*.rs` and `src/*.rs`) | 187 → 291 |
| Merges with conflicts | 6 of 8 |
| Files in conflict that only one lane had edited | 0 |
| Tests green on their lane, red after merge | 2 |

The two post-merge failures: one assertion that depended on the order of two lanes' additions to one list, and one expectation a second lane had made stale.

## What this shows

1. **A shared build directory is not a shared build.** Cargo keys its artifacts by path relative to the worktree root, so eight worktrees of one crate in one `target/` overwrote each other's output, and lanes ran each other's binaries. Their test results were false until each lane got its own build directory, at about 600 MB of disk per lane, which is what the shared directory was meant to avoid.
2. **This is ADR 10's open point.** ADR 10 gave checks a warm view at a fixed path because build tools key caches on paths and times; it left agent workspaces at fresh paths, so each one rebuilds once, and noted that a pool of stable-path workspaces would remove that. This pass paid both costs at once: fresh paths for correctness would have cost a rebuild per lane, and one shared path for speed cost correctness. Build state has to be per workspace *and* reusable, which is what `[workspace] stable_paths = true` now does ([ADR 23](../adr/0023-workspace-slots-ports-liveness.md)): a slot keeps its ignored files between occupants, and no two live workspaces share one.
3. **Ports are the same problem.** The brief's advice for an address-in-use failure was "another lane's test was using the port: re-run it". `ZIT_PORT` exists so that a workspace has a port of its own instead of a retry.
4. **Conflicts followed the files, not the areas.** Every conflict in the six conflicting merges was in a file two lanes had both edited, although each lane had a distinct area. An area is not a resource; a file or a symbol is. That is why claims are on resources ([ADR 11](../adr/0011-claims-and-awareness.md)); these lanes had none, since they ran under plain worktrees.
5. **Green on the lane says nothing about green after the merge.** Two of 291 tests were order-dependent or stale in ways no single lane could see. Zit's rule that the combined state is what gets checked ([ADR 4](../adr/0004-acceptance.md)), and now a batch checked once ([ADR 18](../adr/0018-batch-acceptance.md)), is this finding in the design.

## Limits of this result

- One repository, one language, one pass, eight agents of one kind. Counts, not a statistical study.
- The 600 MB per lane is approximate and was not measured per lane.
- The lanes used git worktrees, not Zit workspaces, so this says what Zit should do, not what it did.

## Changes made

`[workspace] stable_paths` and `ZIT_PORT` ([ADR 23](../adr/0023-workspace-slots-ports-liveness.md)) were built by the workspace lane in this same pass, before this lesson was written down. Nothing in this lesson has been measured with them on.
