# ADR 18: Several changes land as one batch

`zit accept A B C`, or `--batch`, composes the changes in order, verifies the combined state once, and moves current once.

**Status:** accepted. Amends [ADR 4](0004-acceptance.md).

## Context

ADR 4 accepts one change at a time: compose, verify, move current. With several verified changes waiting, each acceptance runs the checks again on a state that differs from the last one by one change, and current moves once per change. An integrator that would land a set of changes together pays for N verifications where one would do, and has no way to ask what accepting a change would do without doing it.

## Decision

- `zit accept A B C` composes the changes onto current in the order given. `zit accept --batch` takes every verified, non-conflicting change, in `status` order.
- Each member is composed as in ADR 4, against the state the earlier members produced. A member that cannot be composed is **skipped with a reason** and the batch goes on: already accepted, stale (also against an earlier member, which `by` names), text conflict, parse failure ([ADR 19](0019-composed-state-must-parse.md)), derive failure.
- The checks of the combined state run **once**: current's and every landed member's, evidence reused as usual. A failing check lands nothing: `rejected: failed checks: <names> on the combined state of <ids>; nothing landed`, exit 1, followed by `next: accept one at a time to find the change responsible`.
- `refs/zit/current` moves once, by the same compare-and-swap as ADR 4. If current moved meanwhile, the whole batch starts again.
- Each member keeps its own commit and its own reason: a compose commit per member, or one commit per member with its `Zit-Change` trailer under `--linear`.
- `--allow-stale`, `--rerun` and `--linear` apply to the whole batch. `--dry-run` and `--batch` are mutually exclusive.
- **`zit accept --dry-run <change>`** does the staleness test, the text merge and the parse check, then names the checks that would run (current's and the change's) and which of them would be reused. It moves nothing and runs nothing. Human output: `would accept <id> (fast-forward|composed onto current); checks: t (run), scoped (reused)`, `already accepted`, or `rejected: ...` with exit 1. JSON: `{"outcome":"would-accept","composed":bool,"checks":[{check,run}]}`, `{"outcome":"already-accepted"}` or `{"outcome":"rejected",...}`. `--rerun` marks every check as run; `--allow-stale` applies.
- **`zit check <change> --only NAME`** (repeatable) runs only the named checks. A name no check declares is an error, exit 2: ``zit: no check named `nope`; declared: ui, api``. `accept` is unaffected.

Output of a batch: a `landed <short> <subject>` line per member, `skipped <short> invalid: stale` with the reason, then `current is <short> (2 landed, 1 skipped; checks: 1 run, 0 reused)`. JSON: `{"outcome":"accepted","current","landed":[],"skipped":[{change,reason,detail}],"verdicts"}` or `{"outcome":"rejected","failed":[],"tried":[],"skipped":[]}`.

## Consequences

- One verification for N changes instead of N. The saving is not measured.
- A failing combined check does not say which member is responsible; the message says to accept one at a time to find out.
- Members are composed against each other, so a change that is verified on its own can be skipped as stale against an earlier member of the same batch.
- `--dry-run` does not run `[[derive]]` rules, so a check with a generated input may be shown as `run` where `accept` would reuse its evidence.
