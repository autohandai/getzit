# ADR 6: Evidence is keyed by check and input content

A check result is stored under a hash of the check and the tree ids of its inputs, and reused by every state that shares them.

**Status:** accepted

## Decision

Checks are declared in `zit.toml` at the root of the state being checked:

```toml
[[check]]
name = "ui"
run = "npm test --prefix ui"
inputs = ["ui", "package.json"]   # omit to depend on the whole state
```

The evidence key is `hash(name, run, [(input path, git object id of that path in the state)])`. Because a directory's object id is a Merkle hash of everything under it, the key changes exactly when an input changes.

Evidence (pass/fail, exit code, duration, output tail, the state it ran on) is a JSON blob at `refs/zit/evidence/<key>`. Failures are evidence too. A check runs in a verification view of the state ([ADR 10](0010-warm-verification-views.md)).

## Consequences

- A change touching only `api/` does not re-run the `ui` check: its key is unchanged. Verified by `only_checks_whose_inputs_changed_run_again`.
- `inputs` is a promise by the author. A check that reads files outside its declared inputs can be wrongly reused.
- The config is part of the state, so a change can alter its own checks. Its write set then shows `zit.toml`.
- Evidence replicates with the graph ([ADR 1](0001-git-objects-are-the-graph.md)).
