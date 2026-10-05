# ADR 3: Symbol-granular read and write sets

What a change wrote is derived from the diff at symbol granularity; what it read is declared or inferred by name.

**Status:** accepted

## Context

"Agent C is stale because agent A changed `StripeClient`" requires knowing what each change read and wrote, at a finer grain than files and coarser than lines.

## Decision

A **resource** is one of:

| Text form | Meaning |
|---|---|
| `path` | a whole file |
| `path#Name` | a top-level symbol; methods belong to their type |
| `path#` | module-level code outside any symbol (imports, statements) |

A literal `#` or `%` in a path is written `%23` or `%25`.

A **footprint** of a span `base..tip` has three parts:

- **writes** — derived, never declared. For each changed file, the symbols whose content hash differs. Files without a grammar, or over 1 MiB, are written whole.
- **refs** — inferred reads: every identifier mentioned in the written symbols, except the identifier that declares the symbol itself.
- **reads** — declared reads (`zit read`, `--read`, MCP `zit_read`), stored in the change as `Zit-Read` trailers.

Two concurrent footprints **conflict** when one wrote a resource the other wrote, declared as read, or mentions by name. A declared whole-file read conflicts with any write inside that file.

Symbols are extracted with tree-sitter for Rust, Python, JavaScript, TypeScript/TSX and Go. In Markdown, each heading starts a section named after it; sections are symbols with no references. This was added after ten agents each appended a few lines to the same three documentation files and all conflicted ([Lessons](../docs/lessons.mdx)).

## Consequences

- Two agents editing different functions of the same file do not conflict.
- A change that calls a function another change rewrote is caught even though git merges the two cleanly.
- Inferred reads match by **name**, not by resolved binding. This over-approximates: a mention of `Config` conflicts with a concurrent write to *any* top-level `Config`. It never misses a statically named reference to a top-level symbol; it does miss dynamic dispatch, reflection and string-built names. Checks on the composed state are the backstop ([ADR 4](0004-acceptance.md)).
- A change whose callee's *body* changed is reported stale even if it would still pass. Measured rate in [Benchmarks](../docs/benchmarks.mdx).
