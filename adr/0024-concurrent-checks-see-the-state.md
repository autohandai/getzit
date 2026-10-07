# ADR 24: Checks run concurrently when asked, and each sees the state

`[accept] jobs = N` runs up to N checks of one verification at once; the view is restored to the state before a check whenever nothing runs alongside; inputs are paths; a nearly full volume is refused; evidence is published with current.

**Status:** accepted. Amends [ADR 6](0006-evidence.md) and [ADR 10](0010-warm-verification-views.md).

## Context

Checks of one verification ran one after another in one view. Two audits of that found:

- A check that edited tracked files (a formatter, a generator) changed what the next check tested, while that check's evidence was recorded against the state.
- A check input written as a pattern (`src/*.rs`) matched nothing in `ls-tree`, so its key never changed and its evidence was reused forever: a silent permanent pass.
- Nothing stopped a materialisation or a view move on a volume with no space left, where git fails half-way.
- Fresh evidence was stored in its own ref updates before current moved.

## Decision

- **Concurrency is the gate's choice.** `[accept] jobs = N` in current's `zit.toml` (default 1) runs up to N of a verification's pending checks at once, in declared order, in the same view. `[[check]] serial = true` makes a check wait for the others and run alone. Verdict order and evidence keys are unchanged; `jobs` is not part of the key.
- **Each check sees the state.** Before a check starts, when no other check is running in the view, the view is restored: `git reset --hard` and `git clean -fd`; ignored files stay. With `jobs = 1` that is before every check after the first, two git calls each. With `jobs > 1` the checks running together share the view and whatever they write.
- **Inputs are paths.** An input that is a pattern is refused: ``zit.toml: input `src/*.rs` is a pattern; inputs are paths``.
- **Free space.** `git config zit.minFreeMB` (default 512; 0 or negative disables) is checked with `statfs` on Zit's home before any workspace build (materialise, retry, run, cache clones) and before every view: ``refusing to materialise: N MB free on the volume holding <home>, less than zit.minFreeMB = 512 (`git config zit.minFreeMB 0` disables this guard)``.
- **Evidence lands with current.** Fresh evidence is published in the same `update-ref` transaction that moves current. If the move fails, the evidence is applied alone.

## Consequences

- Two `sleep 1` checks: 2.45 s one after the other, 1.19 s with `jobs = 2` (one Mac, release build, test timing).
- Checks that run together share the view directory and `TMPDIR`; a check that writes into the tree needs `serial = true`.
- A check that ran alone ran against the state, not against an earlier check's edits.
- The 512 MB default is not exercised by a test.
