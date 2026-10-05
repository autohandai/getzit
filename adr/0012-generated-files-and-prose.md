# ADR 12: Generated files are rebuilt; prose is merged as text

Declared generated files never conflict and are regenerated on compose; write-write on Markdown sections is decided by the text merge.

**Status:** accepted. Amends [ADR 3](0003-resources-and-footprints.md) and [ADR 4](0004-acceptance.md). Came out of [Lesson 02](../docs/lessons.mdx).

## Context

Twenty developers adding entries to one catalogue: Zit landed 12, plain git landed 20. Almost every Zit rejection was one of two things.

- A **generated file** (`registry.json`, built by a script from the agent files) was written by every change, so every pair of changes conflicted on it. Its correct content can always be recomputed.
- Two contributors adding different bullets to **one Markdown section** were a write-write conflict at section granularity, although git merges the text and nothing reads prose.

## Decision

**Generated files.** `zit.toml` declares them:

```toml
[[derive]]
path = "registry.json"
run = "python3 scripts/generate_registry.py"
```

- A derived path is never a reason to be stale.
- Text conflicts inside a derived path do not count.
- A derived path cannot be claimed or held.
- When a change is composed onto current, Zit commits the text merge, moves a verification view to it, runs every `derive` command there, and takes only the declared paths from the result. That is the composed state that is checked and accepted. A failing command rejects the change as `failed: derive <path>`.

**Prose.** A write-write conflict on a Markdown resource (`.md`, `.mdx`, `.markdown`) is dropped from validation; the text merge decides. Read-write conflicts from declared reads still apply.

## Consequences

- Same 20 developers, same seed: 20 of 20 landed in 21 attempts. The remaining rejection was a genuine text conflict.
- A derived file's content in a change is ignored on compose. Hand edits to a generated file are lost, as they would be the next time the generator runs.
- Code keeps its stricter rule: two edits to one function are refused even when the text merges.
- Adjacent insertions into a sorted list are still a text conflict; git has the same limit.
