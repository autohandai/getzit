# ADR 25: Status is memoised, and still derived

Each change's status is kept with the current and the evidence it was derived from, and reused only while both are the same.

**Status:** accepted. Amends [ADR 2](0002-status-is-derived.md).

## Context

ADR 2 computes every speculative change's status on every `zit status`. The answer cannot change until current moves or evidence arrives, but the cost was paid each time: with 100 changes, 304 to 400 git processes per call.

## Decision

Status is still a pure function of (change, current, evidence). Its value is memoised at `<repo>/cache/status/<id>` as `{current, evidence fingerprint, status}` and reused when the current id and the fingerprint match. The fingerprint hashes the names and ids under `refs/zit/evidence`, the ledger names and `zit.trustFetchedEvidence`; it is computed only when there are speculative changes. An `invalid: error` status is never memoised, since the failure may be transient. A memo is written whole and renamed into place, so a concurrent listing never reads half of one.

Two derivations were also trimmed: the scan for linearly landed ancestors ([ADR 20](0020-linear-acceptance-lands-spans.md)) is skipped when the change's base is current, and `overview` loads current in one `log` call.

## Consequences

Measured by `bench/status.sh`: 100 changes on a 200-file Python project with one check, interleaved A/B runs under load on one machine.

| `zit status` | before | after |
|---|---:|---:|
| warm, every change on current | 462 ms | 52 ms |
| warm, after one accept | 713 ms | 53 ms |
| git processes, warm | 304–400 | 5 |
| cold, every change on current | 413 ms | 308 ms |
| cold, after one accept | 652 ms | 464 ms |

- The first `status` after current moves still evaluates every change. A memo under a different key is not an answer.
- The memo is a cache under Zit's home: rebuildable, not part of the graph ([ADR 1](0001-git-objects-are-the-graph.md)).
