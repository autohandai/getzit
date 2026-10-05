# Architecture decisions

Why Zit is built the way it is, one decision per file, in the order they were made. Later decisions say which earlier ones they amend.

- [ADR 1: The graph is git objects and refs](0001-git-objects-are-the-graph.md)
- [ADR 2: Status is derived, never stored](0002-status-is-derived.md)
- [ADR 3: Symbol-granular read and write sets](0003-resources-and-footprints.md)
- [ADR 4: Acceptance is optimistic concurrency with validation](0004-acceptance.md)
- [ADR 5: Workspaces are copy-on-write clones presented as git views](0005-materialisation.md)
- [ADR 6: Evidence is keyed by check and input content](0006-evidence.md)
- [ADR 7: Git is driven through its CLI](0007-git-through-its-cli.md)
- [ADR 8: Two agent interfaces: a process wrapper and an MCP server](0008-agent-integration.md)
- [ADR 9: Naming](0009-naming.md)
- [ADR 10: Verification runs in warm, stable views](0010-warm-verification-views.md)
- [ADR 11: Claims and in-flight awareness in front of validation](0011-claims-and-awareness.md)
- [ADR 12: Generated files are rebuilt; prose is merged as text](0012-generated-files-and-prose.md)
- [ADR 13: Ignore tool by-products; bound the cost of awareness](0013-byproducts-and-awareness-cost.md)
- [ADR 14: Install dependencies once, clone them into every workspace](0014-prepared-dependencies.md)
- [ADR 15: Trust boundaries](0015-trust-boundaries.md)
- [ADR 16: Interfaces, methods and imports](0016-interfaces-methods-imports.md)
- [ADR 17: Team rules, and what a change cost](0017-team-rules-and-cost.md)
