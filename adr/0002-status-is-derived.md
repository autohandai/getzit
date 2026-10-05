# ADR 2: Status is derived, never stored

A change's status is computed from the graph and the evidence on demand.

**Status:** accepted

## Context

A change moves through speculative, verified, invalid, accepted, current. Storing that as a field means it can disagree with reality whenever current moves or evidence arrives.

## Decision

Status is a pure function of the graph, computed when asked:

| Status | Condition |
|---|---|
| `current` | it is `refs/zit/current` |
| `accepted` | it is an ancestor of current |
| `invalid: stale` | its read/write sets conflict with what current changed since their common ancestor |
| `invalid: conflict` | no semantic conflict, but its text does not merge with current |
| `invalid: failed` | evidence exists for its state and a check failed |
| `invalid: error` | its state cannot be evaluated, for example an unparseable `zit.toml` |
| `verified` | none of the above, and every check of **its own** state has passing evidence |
| `speculative` | otherwise |

`verified` describes the change's own state. When current has moved, `accept` composes and verifies the composed state again; a verified change can still be rejected there.

"Proposed" is a workspace that has not been recorded yet; it appears in the workspace list, not as a change.

## Consequences

- Invalidation is automatic: when current moves, every speculative change's status changes with it, with no bookkeeping.
- There is no status to migrate, corrupt or replicate.
- Cost: `zit status` does work proportional to the number of speculative changes (measured in [Benchmarks](../docs/benchmarks.mdx)).
