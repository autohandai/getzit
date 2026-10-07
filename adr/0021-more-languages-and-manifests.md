# ADR 21: Symbol units in more languages, and in manifests

Java, Ruby and C# get the units of the first five languages; TOML tables and JSON keys are units hashed by parsed content.

**Status:** accepted. Amends [ADR 3](0003-resources-and-footprints.md) and [ADR 16](0016-interfaces-methods-imports.md).

## Context

ADR 3 extracts symbols for Rust, Python, JavaScript, TypeScript/TSX and Go. Every other file is written whole, so two agents editing one Java file always refuse each other. Manifests (`Cargo.toml`, `package.json`) are the files most changes touch: written whole, a dependency added by one change made every change that bumped the version stale.

A hostile pass over ADR 16's rules also found: receiver and impl types were named by splitting text, so `func (s *Stack[T, U]) Push` became `U]::Push` and `impl Tr for &'a Foo` became `'a Foo::…`; comments inside a type with methods, and its doc comment, were part of its interface, so a comment staled every reader; TypeScript overload signatures, `declare` and `namespace` were module-level code; an empty Markdown heading (`# `) hid edits to the text before it.

## Decision

- **Java** (tree-sitter-java 0.23.5): class, interface, enum, record and `@interface` are units; methods and constructors are `Type::method`; imports are `(imports)`; the `package` line is module-level. `Util.max()` with `Util` imported is a reference, `it.cost` is not, an unqualified call is.
- **Ruby** (tree-sitter-ruby 0.23.1): class, module, top-level `def`, `def self.x` and top-level constants are units; methods inside a class or module are `Class::method`; `require`, `require_relative` and `load` are `(imports)`. Constants count as identifiers: a call on a constant receiver (`Lib.tax`) is a reference, a call on a value is not.
- **C#** (tree-sitter-c-sharp 0.23.5): class, interface, struct, record, enum and delegate are units; methods and constructors, expression-bodied ones included, are `Type::method`; `using` is `(imports)`. Declarations inside a block namespace are top level; the namespace name, and a file-scoped namespace, are module-level. Member access on an imported name is a reference.
- **TypeScript**: abstract method signatures and overload signatures are `Class::method` units; `declare …` and `namespace N {}` are symbols.
- **Comments are never part of an interface**, in any language, also inside a type with methods.
- **Receiver and impl types are named from the parse tree**, through pointers, lifetimes and generics.
- **Manifests.** In a `.toml` file every top-level key is a unit: `Cargo.toml#package`, `Cargo.toml#dependencies`; `[profile.*]` tables are under `#profile`, `[[bin]]` under `#bin`. In a `.json` file whose root is an object every top-level key is a unit (`package.json#scripts`); any other JSON is the one unit `path#`. A unit is hashed from its parsed value re-serialised, so formatting, key order and comments are not content. Manifests have no references. A manifest that does not parse is a whole-file resource.
- An empty Markdown heading is module-level text, not a section.
- The footprint cache moves to `cache/footprint-v3`, so none of the old results are read.

## Consequences

- Eight languages (Rust, Python, JavaScript, TypeScript/TSX, Go, Java, Ruby, C#), Markdown by heading, manifests by table or key. C, C++, Kotlin, PHP, Swift and Scala are still written whole, as is any file over 1 MiB. `zit record` no longer warns about Java, Ruby and C# files.
- Proved by tests: adding a dependency writes only `Cargo.toml#dependencies`, bumping the version only `#package`, and the two compose; two additions to `[dependencies]` are write-write; reformatting `package.json` writes nothing.
- Manifests are section-level only. A comment-only change to a manifest records no write. `Cargo.lock` is effectively one unit, `Cargo.lock#package`.
- Java: lambda bodies count as interface; a static nested class is part of its outer type. Ruby: `class << self` and `define_method` are module-level or part of the class; a singleton and an instance method of one name share a unit. C#: properties and their accessor bodies, operators, destructors and events are part of their type.
- Ruby's and C#'s tests were written in one pass with Java's and were not watched fail first.
