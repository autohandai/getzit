# ADR 4: Acceptance is optimistic concurrency with validation

Accept composes a change onto current if footprints do not conflict, verifies the result, then moves current atomically.

**Status:** accepted

Amended by [ADR 18](0018-batch-acceptance.md), [ADR 19](0019-composed-state-must-parse.md) and [ADR 20](0020-linear-acceptance-lands-spans.md).

## Decision

`accept(change)`:

1. Already in current's history: nothing to do.
2. Built directly on current: the candidate is the change itself.
3. Otherwise:
   - If its footprint conflicts with what current changed since their merge base: **rejected, stale**, with each resource and the accepted change responsible.
   - If the text does not merge: **rejected, conflict**.
   - Else a **compose change** is created with parents `[current, change]`.
4. The candidate's checks run; evidence is reused where inputs are unchanged. Any failure: **rejected, failed**.
5. `refs/zit/current` moves from the observed current to the candidate in one atomic ref transaction. If another accept won the race, start again from 1.

Rejection deletes nothing. `zit retry` rebuilds a rejected change on current in a new workspace; recording it yields a change with parents `[current, original]`.

`--allow-stale` skips the footprint test in step 3 and lets the checks decide. It exists because the acceptance authority may know that an overlap is harmless. Text conflicts and failing checks still reject.

`--rerun` ignores existing evidence in step 4. A failure is evidence like any other, so a flaky failure otherwise blocks the same state until it is re-run.

If the ref transaction in step 5 fails ten times while current has not moved, accept stops with an error instead of retrying forever (a stale lock file, a permissions problem).

## Consequences

- Merge is not a primitive: it is a change with two parents, created only when it is known to compose.
- Current only ever moves to a state whose checks passed.
- Racing accepts are safe: verified by `racing_accepts_all_land_exactly_once`.
- Default strictness costs retries for overlaps that tests would have passed. Measured in [Benchmarks](../docs/benchmarks.mdx).
