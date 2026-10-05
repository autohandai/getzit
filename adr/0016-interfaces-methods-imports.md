# ADR 16: Interfaces, methods and imports

Readers depend on interfaces; methods and imports are units of their own.

**Status:** accepted. Amends [ADR 3](0003-resources-and-footprints.md). Came out of [Review 01](../feedback/review-01.md).

## Context

Review 01 found the footprint model too coarse in four ways, all confirmed in the code:

1. A callee's body change made every caller stale. The scripted benchmark refused 21–23% of good changes, mostly for this.
2. `cache.get()` counted as reading every top-level `get` in the repository.
3. A method was part of its type, so two agents editing two methods of one class refused each other.
4. Imports were module-level code, so two changes that each added an import refused each other.

## Decision

- **Every unit has two hashes:** its text, and its interface: the text without function bodies and comments. A write whose interface changed is recorded in the footprint's `signatures`. An inferred reader (by name) goes stale only on an interface change. A declared reader (`zit read`) still sees any change.
- **Member access on a value is not a reference.** `x.get` is skipped. A member of an imported module (`lib.price` in Python and JavaScript, `fmt.Println` in Go) is still a reference; the file's imports say which receivers are modules. Rust uses `::` for paths, so `.field` is never one.
- **Methods are units**, named `Type::method`: functions in Rust `impl` blocks, Python and JavaScript/TypeScript class bodies, and Go methods by receiver. The rest of the type stays `Type`. Code that names `Type` depends on all of its methods' interfaces. For claims, a type holds its methods; for writes, a field and a method can change at once.
- **Imports are a unit**, `path#(imports)`. Concurrent writes to it are decided by the text merge, like prose.
- **The footprint cache is versioned** (`cache/footprint-v2`) and keyed by the declared reads, so new rules never read old results.

## Consequences

- A body change that breaks a caller's behaviour without changing its interface is no longer refused up front; the combined state's checks must catch it. That is the trade: fewer false refusals, more reliance on checks.
- A method called on a value whose type is never named in the change (`get_cache().area()`) is not linked to `Type::area`.
- Nested functions remain part of their parent.
- The 21–23% false-refusal figure predates this ADR and has not been re-measured.
